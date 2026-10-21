//! API-key usage pollers (USG-09, subplan 15) under the PRV-07 probe
//! discipline: every poll is one explicit, bounded, redacted request that
//! returns; nothing here schedules or repeats itself. Response shapes are
//! ported from the CodexBar snapshot's provider docs and fetchers.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use superai_core::health::{MAX_RESPONSE_BYTES, redact_url, validate_timeout};
use superai_core::template_fetch::USER_AGENT;

use crate::error::UsageError;
use crate::window::{LaneReading, RateWindow};

/// Everything a poll can fail with. Reasons are static text and redacted
/// URLs; no variant ever carries a credential or response-body bytes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PollError {
    /// The request was not sent: bad URL, bad header value, or a refused
    /// endpoint override.
    #[error("invalid poll request: {reason}")]
    Invalid {
        /// What was refused.
        reason: String,
    },
    /// No credential was supplied for an endpoint that needs one.
    #[error("no credential was supplied for the poll")]
    MissingCredential,
    /// The endpoint rejected the credential (401/403).
    #[error("credential rejected (http {status})")]
    Auth {
        /// Rejection status.
        status: u16,
    },
    /// The endpoint rate limited the request (429).
    #[error("rate limited (http 429)")]
    RateLimited,
    /// The endpoint failed on its side (5xx).
    #[error("provider server error (http {status})")]
    Server {
        /// Failure status.
        status: u16,
    },
    /// A status outside every documented contract.
    #[error("unexpected http status {status}")]
    Status {
        /// Status the endpoint returned.
        status: u16,
    },
    /// A redirect was refused instead of followed: poll endpoints are
    /// documented absolute URLs, and following one would need to re-decide
    /// credential forwarding on every hop.
    #[error("redirect refused for {url}")]
    Redirect {
        /// Redacted URL that answered with a redirect.
        url: String,
    },
    /// The response exceeded the byte cap.
    #[error("response exceeded the {limit}-byte cap")]
    Oversized {
        /// Cap that was exceeded.
        limit: usize,
    },
    /// The whole request exceeded its timeout.
    #[error("request exceeded its timeout")]
    Timeout,
    /// Transport failure before a status existed.
    #[error("transport failure ({class})")]
    Transport {
        /// Failure family: dns, tls, io, or network.
        class: &'static str,
    },
    /// A 2xx response did not match the provider's documented shape.
    #[error("response did not match the documented shape: {reason}")]
    Parse {
        /// Which part of the shape was violated; static text only.
        reason: String,
    },
}

/// One poll's credential, built from a key, an environment value, or the
/// `ClinePass` session file. `Debug`, `Display`, and `Serialize` never reveal the
/// value, and no public accessor hands it out of this module.
#[derive(Clone)]
pub struct PollSecret(String);

impl PollSecret {
    /// Wrap a raw credential; the poll functions refuse empty input before
    /// any request exists.
    pub fn new(value: &str) -> Self {
        Self(value.to_owned())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PollSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PollSecret([REDACTED])")
    }
}

impl std::fmt::Display for PollSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Serialize for PollSecret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str("[REDACTED]")
    }
}

/// A named quota lane beyond the three standard positions
/// (`UsageFetcher.swift`'s `NamedRateWindow`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NamedLane {
    /// Stable lane id, e.g. `factory-core-5h`.
    pub id: String,
    /// Display title.
    pub title: String,
    /// The lane reading.
    pub reading: LaneReading,
}

/// Provider-reported spend inside a period, distinct from a quota window.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostReading {
    /// Amount spent in the period.
    pub used: f64,
    /// Spending cap, when the provider reports one.
    pub limit: Option<f64>,
    /// Currency code.
    pub currency: String,
    /// Period label, e.g. `current billing period`.
    pub period: String,
}

/// Provider-reported account balance.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BalanceReading {
    /// Spendable amount left.
    pub available: f64,
    /// Currency code.
    pub currency: String,
    /// Granted (free) portion, when the provider splits it out.
    pub granted: Option<f64>,
    /// Paid (topped-up) portion, when the provider splits it out.
    pub paid: Option<f64>,
}

/// `UsageSnapshot`-equivalent output of one poll: three standard lanes,
/// named extras, and cost/balance lanes when the provider reports them.
/// Unreported lanes are `Unavailable`, never a phantom zero.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderUsageSnapshot {
    /// Provider id the snapshot belongs to.
    pub provider: &'static str,
    /// ISO-8601 instant the poll completed at.
    pub fetched_at: String,
    /// Primary window lane.
    pub primary: LaneReading,
    /// Secondary window lane.
    pub secondary: LaneReading,
    /// Tertiary window lane.
    pub tertiary: LaneReading,
    /// Named lanes beyond the standard three.
    pub extras: Vec<NamedLane>,
    /// Spend lane, when reported.
    pub cost: Option<CostReading>,
    /// Balance lane, when reported.
    pub balance: Option<BalanceReading>,
}

impl ProviderUsageSnapshot {
    /// An all-unavailable snapshot for `provider`; every poll fills only the
    /// lanes its endpoint actually reported.
    fn empty(provider: &'static str) -> Self {
        Self {
            provider,
            fetched_at: Timestamp::now().to_string(),
            primary: LaneReading::Unavailable,
            secondary: LaneReading::Unavailable,
            tertiary: LaneReading::Unavailable,
            extras: Vec::new(),
            cost: None,
            balance: None,
        }
    }
}

fn parse_err(reason: &str) -> PollError {
    PollError::Parse {
        reason: reason.to_owned(),
    }
}

fn lane(
    used_percent: f64,
    window_minutes: Option<u32>,
    resets_at: Option<Timestamp>,
    reset_description: Option<String>,
) -> LaneReading {
    LaneReading::Measured(RateWindow {
        used_percent,
        window_minutes,
        resets_at,
        reset_description,
        next_regen_percent: None,
    })
}

fn finite_f64(value: f64) -> Option<f64> {
    value.is_finite().then_some(value)
}

fn field<'v>(value: &'v Value, key: &str) -> Option<&'v Value> {
    value.get(key).filter(|inner| !inner.is_null())
}

fn obj_field<'v>(value: &'v Value, key: &str) -> Option<&'v Value> {
    value.get(key).filter(|inner| inner.is_object())
}

fn number_field(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64).and_then(finite_f64)
}

fn text_field<'v>(value: &'v Value, key: &str) -> Option<&'v str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn parse_json(body: &str) -> Result<Value, PollError> {
    serde_json::from_str(body).map_err(|_error| parse_err("response was not valid json"))
}

fn root_object(body: &str) -> Result<Value, PollError> {
    let value = parse_json(body)?;
    value
        .is_object()
        .then_some(value)
        .ok_or_else(|| parse_err("response root must be an object"))
}

fn used_percent(used: f64, total: f64) -> f64 {
    used / total * 100.0
}

/// Coerce a provider field to bool the way `WarpUsageFetcher` does: literal
/// bools, 1/0 numbers, and the words true/1/yes.
fn coerce_bool(value: Option<&Value>) -> bool {
    let Some(value) = value else {
        return false;
    };
    match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64() == Some(1.0),
        Value::String(word) => matches!(
            word.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "yes"
        ),
        _ => false,
    }
}

/// Coerce a provider field to an integer: numbers or numeric strings.
fn coerce_int(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64(),
        Value::String(digits) => digits.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Parse a provider-reported reset stamp; a malformed one is a parse
/// refusal, never a silently dropped window boundary.
fn reset_instant(text: &str) -> Result<Timestamp, PollError> {
    text.trim()
        .parse()
        .map_err(|_error| parse_err("reset time must be an RFC 3339 timestamp"))
}

/// ISO-8601 instant for a provider-reported epoch, keeping MiniMax/Factory's
/// seconds-versus-milliseconds heuristic (`> 1e12` means milliseconds). An
/// epoch outside both ranges is a parse refusal, not a silent None.
#[expect(
    clippy::cast_possible_truncation,
    reason = "epoch magnitudes are bounded by the heuristic before the cast"
)]
fn epoch_iso(epoch: f64) -> Result<Timestamp, PollError> {
    let out_of_range = || parse_err("epoch reset time is outside the second or millisecond ranges");
    if !epoch.is_finite() || epoch <= 0.0 {
        return Err(out_of_range());
    }
    if epoch > 1e12 {
        Timestamp::from_millisecond(epoch as i64).map_err(|_error| out_of_range())
    } else if epoch > 1e9 {
        Timestamp::from_second(epoch as i64).map_err(|_error| out_of_range())
    } else {
        Err(out_of_range())
    }
}

/// Whole minutes between two provider-reported epochs.
#[expect(
    clippy::cast_possible_truncation,
    reason = "minute magnitudes are tiny after the 60-second floor"
)]
fn epoch_minutes(start: f64, end: f64) -> Option<u32> {
    if !start.is_finite() || !end.is_finite() || end <= start {
        return None;
    }
    let minutes = (end - start) / 60.0;
    if minutes < 1.0 {
        return None;
    }
    u32::try_from(minutes as i64).ok()
}

struct PollRequest {
    url: String,
    headers: Vec<(&'static str, String)>,
    body: Option<String>,
}

impl PollRequest {
    fn get(url: &str, headers: Vec<(&'static str, String)>) -> Self {
        Self {
            url: url.to_owned(),
            headers,
            body: None,
        }
    }

    fn post(url: &str, headers: Vec<(&'static str, String)>, body: String) -> Self {
        Self {
            url: url.to_owned(),
            headers,
            body: Some(body),
        }
    }
}

/// URL authority (host and optional port) with the scheme stripped.
fn authority_of(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest.get(..end)
}

/// Host part of a URL authority with brackets and port stripped.
fn host_of(url: &str) -> Option<&str> {
    let authority = authority_of(url)?;
    if let Some(bracketed) = authority.strip_prefix('[') {
        let end = bracketed.find(']')?;
        return bracketed.get(..end);
    }
    authority.split(':').next()
}

/// Exact loopback names only; `127.0.0.1.attacker.com` is not loopback.
fn is_loopback(host: &str) -> bool {
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

/// Poll URL gate, mirroring the registry's endpoint rule: https everywhere,
/// plain http only on an exact loopback host, no userinfo, no control
/// characters, a named host.
fn validate_poll_url(url: &str) -> Result<(), PollError> {
    let invalid = |reason: &str| PollError::Invalid {
        reason: reason.to_owned(),
    };
    if url.trim().is_empty() {
        return Err(invalid("url must not be empty"));
    }
    if url.chars().any(char::is_control) || url.contains(' ') {
        return Err(invalid("url must not contain control characters or spaces"));
    }
    let https = url.starts_with("https://");
    if !https && !url.starts_with("http://") {
        return Err(invalid("url must be https:// (http:// only on loopback)"));
    }
    let host = host_of(url).ok_or_else(|| invalid("url must name a host"))?;
    if host.is_empty() {
        return Err(invalid("url must name a host"));
    }
    if !https && !is_loopback(host) {
        return Err(invalid("plain http is allowed only on a loopback host"));
    }
    if authority_of(url).is_some_and(|authority| authority.contains('@')) {
        return Err(invalid("url must not carry userinfo"));
    }
    Ok(())
}

fn status_error(status: u16) -> PollError {
    match status {
        401 | 403 => PollError::Auth { status },
        429 => PollError::RateLimited,
        500..=599 => PollError::Server { status },
        _ => PollError::Status { status },
    }
}

fn transport_error(err: ureq::Error) -> PollError {
    match err {
        ureq::Error::StatusCode(status) => status_error(status),
        ureq::Error::Timeout(_) => PollError::Timeout,
        ureq::Error::HostNotFound => PollError::Transport { class: "dns" },
        other => {
            let msg = format!("{other}").to_ascii_lowercase();
            let class = if msg.contains("tls") || msg.contains("certificate") {
                "tls"
            } else {
                "network"
            };
            PollError::Transport { class }
        }
    }
}

/// One bounded request. Redirects are refused, the byte cap is enforced
/// against Content-Length and the streamed body, and only lossy-UTF-8 text
/// is handed to the caller's parser.
fn dispatch(request: &PollRequest, limits: &PollLimits) -> Result<String, PollError> {
    validate_poll_url(&request.url)?;
    for (name, value) in &request.headers {
        if name.chars().any(char::is_control)
            || name.contains(' ')
            || value.chars().any(char::is_control)
        {
            return Err(PollError::Invalid {
                reason: "header names and values must not carry control characters".to_owned(),
            });
        }
    }
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(limits.timeout))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .user_agent(USER_AGENT)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let outcome = if let Some(body) = &request.body {
        let mut sent = agent.post(&request.url);
        for (name, value) in &request.headers {
            sent = sent.header(*name, value.as_str());
        }
        sent.send(body.as_bytes()).map_err(transport_error)
    } else {
        let mut sent = agent.get(&request.url);
        for (name, value) in &request.headers {
            sent = sent.header(*name, value.as_str());
        }
        sent.call().map_err(transport_error)
    };
    let mut response = outcome?;
    let status = response.status().as_u16();
    if matches!(status, 300..=399) {
        return Err(PollError::Redirect {
            url: redact_url(&request.url),
        });
    }
    if !matches!(status, 200..=299) {
        return Err(status_error(status));
    }
    let max_bytes = limits.max_response_bytes;
    if let Some(len) = response
        .headers()
        .get("Content-Length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        && len > max_bytes
    {
        return Err(PollError::Oversized { limit: max_bytes });
    }
    let mut bytes: Vec<u8> = Vec::new();
    let reader = response.body_mut().as_reader();
    let mut capped = reader.take((max_bytes + 1) as u64);
    capped
        .read_to_end(&mut bytes)
        .map_err(|_source| PollError::Transport { class: "network" })?;
    if bytes.len() > max_bytes {
        return Err(PollError::Oversized { limit: max_bytes });
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn bearer(secret: &PollSecret) -> Result<Vec<(&'static str, String)>, PollError> {
    require_secret(secret)?;
    Ok(vec![
        ("Authorization", format!("Bearer {}", secret.expose())),
        ("Accept", "application/json".to_owned()),
    ])
}

fn require_secret(secret: &PollSecret) -> Result<(), PollError> {
    if secret.expose().trim().is_empty() {
        Err(PollError::MissingCredential)
    } else {
        Ok(())
    }
}

fn join_endpoint(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path)
}

/// Window lengths `ClinePass` reports, keyed by its `type` field
/// (`clinepass.ts` maps `five_hour`/`weekly`/`monthly` onto the three lanes).
const CLINEPASS_WINDOW_MINUTES: [(&str, u32); 3] = [
    ("five_hour", 5 * 60),
    ("weekly", 7 * 24 * 60),
    ("monthly", 30 * 24 * 60),
];

/// The credential held in Cline's own `providers.json`; `is_oauth` marks
/// the `workos:` browser-session token as distinct from a stored API key.
#[derive(Debug, Clone)]
pub struct ClinePassSession {
    secret: PollSecret,
    is_oauth: bool,
}

impl ClinePassSession {
    /// Whether the session came from `auth.accessToken` (browser sign-in).
    pub fn is_oauth(&self) -> bool {
        self.is_oauth
    }

    /// The wrapped credential, ready for [`poll_clinepass`].
    pub fn secret(&self) -> &PollSecret {
        &self.secret
    }
}

/// Resolve the Cline providers file the way `cline auth` does:
/// `CLINE_PROVIDER_SETTINGS_PATH` (a file), then `CLINE_DATA_DIR`
/// (`settings/providers.json` inside it), then `CLINE_DIR`
/// (`data/settings/providers.json` inside it), then
/// `~/.cline/data/settings/providers.json`. `~` and `~/` expand against
/// `home`; nothing is read or written here.
pub fn clinepass_providers_path(
    provider_settings_path: Option<&str>,
    data_dir: Option<&str>,
    cline_dir: Option<&str>,
    home: &Path,
) -> PathBuf {
    let expand = |value: &str| -> PathBuf {
        if value == "~" {
            home.to_path_buf()
        } else if let Some(rest) = value.strip_prefix("~/") {
            home.join(rest)
        } else {
            PathBuf::from(value)
        }
    };
    let cleaned = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(expand)
    };
    if let Some(file) = cleaned(provider_settings_path) {
        return file;
    }
    match cleaned(data_dir) {
        Some(dir) => dir.join("settings").join("providers.json"),
        None => match cleaned(cline_dir) {
            Some(dir) => dir.join("data").join("settings").join("providers.json"),
            None => home
                .join(".cline")
                .join("data")
                .join("settings")
                .join("providers.json"),
        },
    }
}

/// Read the Cline session, read-only, from `path`. A missing file and a
/// malformed or credential-less one both yield `None`; a non-regular file is
/// refused. Only the `cline` entry's `settings` is consulted, with Cline's
/// own precedence (`auth.accessToken`, then `apiKey`, then `auth.apiKey`).
pub fn clinepass_session(
    path: &Path,
    max_bytes: usize,
) -> Result<Option<ClinePassSession>, PollError> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(None);
    };
    // Stat before open, never the reverse: opening a FIFO blocks before any
    // read exists to bound, so non-regular paths (symlinks included) are
    // refused first (the registry loader guards the same hazard).
    if !meta.is_file() {
        return Err(PollError::Invalid {
            reason: format!("{} is not a regular file", path.display()),
        });
    }
    let file = File::open(path).map_err(|_source| PollError::Transport { class: "io" })?;
    let mut text = String::new();
    let mut capped = file.take((max_bytes + 1) as u64);
    capped
        .read_to_string(&mut text)
        .map_err(|_source| PollError::Transport { class: "io" })?;
    if text.len() > max_bytes {
        return Err(PollError::Oversized { limit: max_bytes });
    }
    Ok(parse_clinepass_session(&text))
}

fn parse_clinepass_session(text: &str) -> Option<ClinePassSession> {
    let root = parse_json(text).ok()?;
    let settings = root.get("providers")?.get("cline")?.get("settings")?;
    let auth = settings.get("auth").filter(|auth| auth.is_object());
    if let Some(access) = auth.and_then(|auth| text_field(auth, "accessToken")) {
        let token = match access.strip_prefix("workos:") {
            Some(_) => access.to_owned(),
            None => format!("workos:{access}"),
        };
        return Some(ClinePassSession {
            secret: PollSecret::new(&token),
            is_oauth: true,
        });
    }
    let key = text_field(settings, "apiKey")
        .or_else(|| auth.and_then(|auth| text_field(auth, "apiKey")))?;
    Some(ClinePassSession {
        secret: PollSecret::new(key),
        is_oauth: false,
    })
}

/// Parse the `usage-limits` response: `success` must be true and
/// `data.limits` an array; recognized window types fill their lane with the
/// raw `percentUsed` (over-quota preserved) and optional `resetsAt`.
pub fn parse_clinepass_limits(body: &str) -> Result<ProviderUsageSnapshot, PollError> {
    let root = root_object(body)?;
    if root.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(parse_err("success must be true"));
    }
    let limits = root
        .get("data")
        .and_then(|data| data.get("limits"))
        .and_then(Value::as_array)
        .ok_or_else(|| parse_err("data.limits must be an array"))?;
    let mut windows: Vec<(&str, LaneReading)> = Vec::new();
    for raw in limits {
        let Some(kind) = raw.get("type").and_then(Value::as_str) else {
            return Err(parse_err("limit type must be a string"));
        };
        let Some((_, minutes)) = CLINEPASS_WINDOW_MINUTES
            .iter()
            .find(|(known, _)| *known == kind)
        else {
            continue;
        };
        let Some(percent) = number_field(raw, "percentUsed") else {
            return Err(parse_err("percentUsed must be a finite number"));
        };
        let reading = match raw.get("resetsAt") {
            None | Some(Value::Null) => lane(percent, Some(*minutes), None, None),
            Some(Value::String(reset)) => {
                lane(percent, Some(*minutes), Some(reset_instant(reset)?), None)
            }
            Some(_) => return Err(parse_err("resetsAt must be a timestamp or null")),
        };
        windows.push((kind, reading));
    }
    let lane_for = |kind: &str| -> LaneReading {
        windows
            .iter()
            .find(|(known, _)| *known == kind)
            .map_or(LaneReading::Unavailable, |(_, reading)| reading.clone())
    };
    let mut snapshot = ProviderUsageSnapshot::empty("cline-pass");
    snapshot.primary = lane_for("five_hour");
    snapshot.secondary = lane_for("weekly");
    snapshot.tertiary = lane_for("monthly");
    Ok(snapshot)
}

/// Poll `endpoint` (the documented
/// `https://api.cline.bot/api/v1/users/me/plan/usage-limits` URL) with a
/// `ClinePass` API key or the session token from [`clinepass_session`].
pub fn poll_clinepass(
    endpoint: &str,
    secret: &PollSecret,
    limits: &PollLimits,
) -> Result<ProviderUsageSnapshot, PollError> {
    let body = dispatch(&PollRequest::get(endpoint, bearer(secret)?), limits)?;
    parse_clinepass_limits(&body)
}

/// Parse the `/user/balance` response: per-currency `balance_infos`; USD is
/// preferred when present, otherwise the first entry wins.
pub fn parse_deepseek_balance(body: &str) -> Result<ProviderUsageSnapshot, PollError> {
    let root = root_object(body)?;
    let infos = root
        .get("balance_infos")
        .and_then(Value::as_array)
        .ok_or_else(|| parse_err("balance_infos must be an array"))?;
    let entry = infos
        .iter()
        .find(|info| text_field(info, "currency") == Some("USD"))
        .or_else(|| infos.first())
        .ok_or_else(|| parse_err("balance_infos must hold at least one entry"))?;
    let currency = text_field(entry, "currency").unwrap_or_default().to_owned();
    let amount = |key: &str| -> Result<Option<f64>, PollError> {
        match entry.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(digits)) => digits
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(finite_f64)
                .map(Some)
                .ok_or_else(|| parse_err("balance amounts must be finite numbers")),
            Some(_) => Err(parse_err("balance amounts must be strings")),
        }
    };
    let total =
        amount("total_balance")?.ok_or_else(|| parse_err("total_balance must be present"))?;
    let mut snapshot = ProviderUsageSnapshot::empty("deepseek");
    snapshot.balance = Some(BalanceReading {
        available: total,
        currency,
        granted: amount("granted_balance")?,
        paid: amount("topped_up_balance")?,
    });
    Ok(snapshot)
}

/// Poll `endpoint` (the documented `https://api.deepseek.com/user/balance`)
/// with a `DeepSeek` API key.
pub fn poll_deepseek(
    endpoint: &str,
    secret: &PollSecret,
    limits: &PollLimits,
) -> Result<ProviderUsageSnapshot, PollError> {
    let body = dispatch(&PollRequest::get(endpoint, bearer(secret)?), limits)?;
    parse_deepseek_balance(&body)
}

/// Parse the `/api/v1/key` and (optional) `/api/v1/credits` responses. A
/// capped key fills the primary lane from `limit`/`limit_remaining`; an
/// uncapped one fills the cost lane from its reported spend counters.
pub fn parse_openrouter_responses(
    key_body: &str,
    credits_body: Option<&str>,
) -> Result<ProviderUsageSnapshot, PollError> {
    let key_root = root_object(key_body)?;
    let key =
        obj_field(&key_root, "data").ok_or_else(|| parse_err("key.data must be an object"))?;
    let limit = number_field(key, "limit").filter(|limit| *limit > 0.0);
    let usage = number_field(key, "usage");
    let usage_monthly = number_field(key, "usage_monthly");
    let mut snapshot = ProviderUsageSnapshot::empty("openrouter");
    if let Some(limit) = limit {
        let used = if let Some(remaining) = number_field(key, "limit_remaining") {
            limit - remaining.clamp(0.0, limit)
        } else {
            let window_key = match text_field(key, "limit_reset") {
                Some("daily") => "usage_daily",
                Some("weekly") => "usage_weekly",
                Some("monthly") => "usage_monthly",
                _ => "",
            };
            number_field(key, window_key).or(usage).unwrap_or(0.0)
        };
        snapshot.primary = lane(used_percent(used, limit), None, None, None);
    } else if let Some(spend) = usage_monthly.or(usage) {
        snapshot.cost = Some(CostReading {
            used: spend.max(0.0),
            limit: None,
            currency: "USD".to_owned(),
            period: if usage_monthly.is_some() {
                "api key this month".to_owned()
            } else {
                "api key total".to_owned()
            },
        });
    }
    if let Some(credits_body) = credits_body {
        let credits_root = root_object(credits_body)?;
        let credits = obj_field(&credits_root, "data")
            .ok_or_else(|| parse_err("credits.data must be an object"))?;
        let total = number_field(credits, "total_credits")
            .ok_or_else(|| parse_err("credits.total_credits must be a finite number"))?;
        let spent = number_field(credits, "total_usage")
            .ok_or_else(|| parse_err("credits.total_usage must be a finite number"))?;
        snapshot.balance = Some(BalanceReading {
            available: (total - spent).max(0.0),
            currency: "USD".to_owned(),
            granted: None,
            paid: None,
        });
    }
    Ok(snapshot)
}

/// Poll `base` (the documented `https://openrouter.ai/api/v1`) with an
/// `OpenRouter` API key: `/key` is required, `/credits` is a second one-shot
/// request whose HTTP-level failure degrades the snapshot to key data only.
pub fn poll_openrouter(
    base: &str,
    secret: &PollSecret,
    limits: &PollLimits,
) -> Result<ProviderUsageSnapshot, PollError> {
    let headers = bearer(secret)?;
    let key_body = dispatch(
        &PollRequest::get(&join_endpoint(base, "key"), headers.clone()),
        limits,
    )?;
    let credits_body = match dispatch(
        &PollRequest::get(&join_endpoint(base, "credits"), headers),
        limits,
    ) {
        Ok(body) => Some(body),
        Err(
            PollError::Auth { .. }
            | PollError::RateLimited
            | PollError::Server { .. }
            | PollError::Status { .. },
        ) => None,
        Err(error) => return Err(error),
    };
    parse_openrouter_responses(&key_body, credits_body.as_deref())
}

/// Parse the `/frontend/billing/usage` response:
/// `current_usage.edit_predictions` fills the primary lane (`limit` null or
/// `"unlimited"` means unlimited), `token_spend` fills the cost lane from
/// its cent-denominated fields.
#[expect(
    clippy::cast_precision_loss,
    reason = "prediction counters are small integers"
)]
pub fn parse_zed_billing(body: &str) -> Result<ProviderUsageSnapshot, PollError> {
    let root = root_object(body)?;
    text_field(&root, "plan").ok_or_else(|| parse_err("plan must be a non-empty string"))?;
    let usage = obj_field(&root, "current_usage")
        .ok_or_else(|| parse_err("current_usage must be an object"))?;
    let predictions = obj_field(usage, "edit_predictions")
        .ok_or_else(|| parse_err("current_usage.edit_predictions must be an object"))?;
    let used = predictions
        .get("used")
        .and_then(Value::as_i64)
        .filter(|used| *used >= 0)
        .map(|used| used as f64)
        .ok_or_else(|| parse_err("edit_predictions.used must be a non-negative integer"))?;
    let mut snapshot = ProviderUsageSnapshot::empty("zed");
    snapshot.primary = match predictions.get("limit") {
        None | Some(Value::Null) => lane(0.0, None, None, Some("Unlimited".to_owned())),
        Some(Value::String(word)) if word == "unlimited" => {
            lane(0.0, None, None, Some("Unlimited".to_owned()))
        }
        Some(Value::Number(number)) => {
            let limit = number
                .as_f64()
                .and_then(finite_f64)
                .filter(|limit| *limit > 0.0)
                .ok_or_else(|| parse_err("edit_predictions.limit must be a positive integer"))?;
            lane(
                used_percent(used, limit),
                None,
                None,
                Some(format!("{} / {} predictions", used.min(limit), limit)),
            )
        }
        Some(_) => {
            return Err(parse_err(
                "edit_predictions.limit must be a number, null, or \"unlimited\"",
            ));
        }
    };
    if let Some(spend) = obj_field(usage, "token_spend") {
        let spent = number_field(spend, "spend_in_cents")
            .filter(|cents| *cents >= 0.0)
            .ok_or_else(|| parse_err("token_spend.spend_in_cents must be a non-negative number"))?;
        if let Some(cap) = number_field(spend, "limit_in_cents").filter(|cents| *cents >= 0.0) {
            snapshot.cost = Some(CostReading {
                used: spent / 100.0,
                limit: Some(cap / 100.0),
                currency: "USD".to_owned(),
                period: "current billing period".to_owned(),
            });
        }
    }
    Ok(snapshot)
}

/// Poll `endpoint` (the documented
/// `https://cloud.zed.dev/frontend/billing/usage`) with the full `Cookie`
/// request-header value (`zed.session=...`, exactly what `zed.js` sends and
/// what `CodexBar`'s manual cookie source stores); it reaches the wire
/// header verbatim and nothing else.
pub fn poll_zed(
    endpoint: &str,
    session_cookie: &PollSecret,
    limits: &PollLimits,
) -> Result<ProviderUsageSnapshot, PollError> {
    require_secret(session_cookie)?;
    let headers = vec![
        ("Cookie", session_cookie.expose().to_owned()),
        ("Accept", "application/json".to_owned()),
    ];
    let body = dispatch(&PollRequest::get(endpoint, headers), limits)?;
    parse_zed_billing(&body)
}

/// Warp's documented query, ported verbatim from `WarpUsageFetcher.swift`.
const WARP_GRAPHQL_QUERY: &str = "query GetRequestLimitInfo($requestContext: RequestContext!) { user(requestContext: $requestContext) { __typename ... on UserOutput { user { requestLimitInfo { isUnlimited nextRefreshTime requestLimit requestsUsedSinceLastRefresh } bonusGrants { requestCreditsGranted requestCreditsRemaining expiration } workspaces { bonusGrantsInfo { grants { requestCreditsGranted requestCreditsRemaining expiration } } } } } } }";
/// Warp's edge limiter 429s anything without a Warp-shaped User-Agent.
const WARP_USER_AGENT: &str = "Warp/1.0";

/// Parse the GraphQL response: `data.user.user.requestLimitInfo` fills the
/// primary lane, and user-level plus workspace bonus grants combine into the
/// secondary add-on lane.
#[expect(
    clippy::cast_precision_loss,
    reason = "credit counters are small integers"
)]
pub fn parse_warp_graphql(body: &str) -> Result<ProviderUsageSnapshot, PollError> {
    let root = root_object(body)?;
    if root
        .get("errors")
        .and_then(Value::as_array)
        .is_some_and(|errors| !errors.is_empty())
    {
        return Err(parse_err("graphql errors were returned"));
    }
    let user = root
        .get("data")
        .and_then(|data| data.get("user"))
        .and_then(|user| user.get("user"))
        .ok_or_else(|| parse_err("data.user.user is missing"))?;
    let info = obj_field(user, "requestLimitInfo")
        .ok_or_else(|| parse_err("data.user.user.requestLimitInfo must be an object"))?;
    let unlimited = coerce_bool(info.get("isUnlimited"));
    let limit = coerce_int(info.get("requestLimit")).unwrap_or(0);
    let requests_used = coerce_int(info.get("requestsUsedSinceLastRefresh")).unwrap_or(0);
    let reset = match info.get("nextRefreshTime") {
        None | Some(Value::Null) => None,
        Some(Value::String(reset)) => Some(reset_instant(reset)?),
        Some(_) => return Err(parse_err("nextRefreshTime must be a timestamp or null")),
    };
    let percent = if unlimited || limit <= 0 {
        0.0
    } else {
        used_percent(requests_used as f64, limit as f64)
    };
    let mut snapshot = ProviderUsageSnapshot::empty("warp");
    if unlimited {
        snapshot.primary = lane(0.0, None, None, Some("Unlimited".to_owned()));
    } else {
        snapshot.primary = lane(
            percent,
            None,
            reset,
            Some(format!("{requests_used} / {limit} credits")),
        );
    }
    let mut granted = 0i64;
    let mut remaining = 0i64;
    let mut grants_total = |grants: Option<&Vec<Value>>| {
        if let Some(grants) = grants {
            for grant in grants {
                granted += coerce_int(grant.get("requestCreditsGranted")).unwrap_or(0);
                remaining += coerce_int(grant.get("requestCreditsRemaining")).unwrap_or(0);
            }
        }
    };
    grants_total(user.get("bonusGrants").and_then(Value::as_array));
    if let Some(workspaces) = user.get("workspaces").and_then(Value::as_array) {
        for workspace in workspaces {
            let grants = workspace
                .get("bonusGrantsInfo")
                .and_then(|info| info.get("grants"))
                .and_then(Value::as_array);
            grants_total(grants);
        }
    }
    if granted > 0 || remaining > 0 {
        let percent = if granted > 0 {
            used_percent((granted - remaining) as f64, granted as f64)
        } else {
            0.0
        };
        snapshot.secondary = lane(percent, None, None, Some("add-on credits".to_owned()));
    }
    Ok(snapshot)
}

/// Poll `endpoint` (the documented
/// `https://app.warp.dev/graphql/v2?op=GetRequestLimitInfo`) with a Warp API
/// key (`WARP_API_KEY`).
pub fn poll_warp(
    endpoint: &str,
    secret: &PollSecret,
    limits: &PollLimits,
) -> Result<ProviderUsageSnapshot, PollError> {
    require_secret(secret)?;
    let os = std::env::consts::OS;
    let body = serde_json::json!({
        "query": WARP_GRAPHQL_QUERY,
        "variables": {
            "requestContext": {
                "clientContext": {},
                "osContext": { "category": os, "name": os, "version": "0" }
            }
        },
        "operationName": "GetRequestLimitInfo",
    })
    .to_string();
    let headers = vec![
        ("Content-Type", "application/json".to_owned()),
        ("Accept", "application/json".to_owned()),
        ("Authorization", format!("Bearer {}", secret.expose())),
        ("User-Agent", WARP_USER_AGENT.to_owned()),
        ("x-warp-client-id", "warp-app".to_owned()),
        ("x-warp-os-category", os.to_owned()),
        ("x-warp-os-name", os.to_owned()),
    ];
    let body = dispatch(&PollRequest::post(endpoint, headers, body), limits)?;
    parse_warp_graphql(&body)
}

fn factory_lane(window: Option<&Value>, minutes: Option<u32>) -> Result<LaneReading, PollError> {
    let Some(window) = window else {
        return Ok(LaneReading::Unavailable);
    };
    let percent = number_field(window, "usedPercent")
        .ok_or_else(|| parse_err("usedPercent must be a finite number"))?;
    let resets_at = match window.get("windowEnd") {
        None | Some(Value::Null) => None,
        Some(Value::String(reset)) => Some(reset_instant(reset)?),
        Some(Value::Number(number)) => number
            .as_f64()
            .and_then(finite_f64)
            .map(epoch_iso)
            .transpose()?,
        Some(_) => return Err(parse_err("windowEnd must be a timestamp or epoch number")),
    };
    let description = number_field(window, "secondsRemaining")
        .filter(|seconds| *seconds > 0.0)
        .map(|seconds| format!("resets in {seconds}s"));
    Ok(lane(percent, minutes, resets_at, description))
}

fn factory_pool_has_data(pool: &Value) -> bool {
    ["fiveHour", "weekly", "monthly"].iter().any(|key| {
        pool.get(key).is_some_and(|window| {
            number_field(window, "usedPercent").is_some_and(|percent| percent > 0.0)
                || field(window, "windowEnd").is_some()
                || field(window, "secondsRemaining").is_some()
        })
    })
}

/// Parse the `/api/billing/limits` response: the `standard` pool fills the
/// three lanes (5h/weekly/monthly), the `core` pool becomes named extras,
/// and `extraUsageBalanceCents` becomes the balance lane. Without a
/// `limits` object every window stays unavailable and only the balance
/// survives.
pub fn parse_factory_limits(body: &str) -> Result<ProviderUsageSnapshot, PollError> {
    let root = root_object(body)?;
    let mut snapshot = ProviderUsageSnapshot::empty("factory");
    if let Some(cents) = number_field(&root, "extraUsageBalanceCents") {
        snapshot.balance = Some(BalanceReading {
            available: cents / 100.0,
            currency: "USD".to_owned(),
            granted: None,
            paid: None,
        });
    }
    let Some(limits) = obj_field(&root, "limits") else {
        return Ok(snapshot);
    };
    let standard = obj_field(limits, "standard")
        .ok_or_else(|| parse_err("limits.standard must be an object"))?;
    snapshot.primary = factory_lane(standard.get("fiveHour"), Some(5 * 60))?;
    snapshot.secondary = factory_lane(standard.get("weekly"), Some(7 * 24 * 60))?;
    snapshot.tertiary = factory_lane(standard.get("monthly"), None)?;
    if let Some(core) = obj_field(limits, "core").filter(|core| factory_pool_has_data(core)) {
        for (key, title, minutes) in [
            ("fiveHour", "Core 5h", Some(5 * 60)),
            ("weekly", "Core 7-day", Some(7 * 24 * 60)),
            ("monthly", "Core monthly", None),
        ] {
            let reading = factory_lane(core.get(key), minutes)?;
            if reading != LaneReading::Unavailable {
                snapshot.extras.push(NamedLane {
                    id: format!("factory-core-{key}"),
                    title: title.to_owned(),
                    reading,
                });
            }
        }
    }
    Ok(snapshot)
}

/// Poll `endpoint` (the documented `https://api.factory.ai/api/billing/limits`)
/// with a Factory API key (`FACTORY_API_KEY` or `~/.factory/.env`).
pub fn poll_factory(
    endpoint: &str,
    secret: &PollSecret,
    limits: &PollLimits,
) -> Result<ProviderUsageSnapshot, PollError> {
    let body = dispatch(&PollRequest::get(endpoint, bearer(secret)?), limits)?;
    parse_factory_limits(&body)
}

/// Parse the tRPC batch response for `user.getCreditBlocks`: credit blocks
/// are micro-USD (`amount_mUsd` granted, `balance_mUsd` remaining), summed
/// into one credits window.
pub fn parse_kilo_credits(body: &str) -> Result<ProviderUsageSnapshot, PollError> {
    let root = parse_json(body)?;
    let payload = root
        .as_array()
        .and_then(|entries| entries.first())
        .and_then(|entry| entry.get("result"))
        .and_then(|result| result.get("data"))
        .and_then(|data| data.get("json"))
        .ok_or_else(|| parse_err("result.data.json is missing"))?;
    let blocks = payload
        .get("creditBlocks")
        .and_then(Value::as_array)
        .ok_or_else(|| parse_err("creditBlocks must be an array"))?;
    let mut total = 0.0;
    let mut remaining = 0.0;
    let mut saw_total = false;
    let mut saw_remaining = false;
    for block in blocks {
        if let Some(amount) = number_field(block, "amount_mUsd") {
            total += amount / 1_000_000.0;
            saw_total = true;
        }
        if let Some(balance) = number_field(block, "balance_mUsd") {
            remaining += balance / 1_000_000.0;
            saw_remaining = true;
        }
    }
    if !saw_total && !saw_remaining {
        return Err(parse_err("credit blocks held no amounts"));
    }
    let mut snapshot = ProviderUsageSnapshot::empty("kilo");
    if saw_total && saw_remaining && total > 0.0 {
        let used = (total - remaining).max(0.0);
        snapshot.primary = lane(
            used_percent(used, total),
            None,
            None,
            Some(format!("{used:.2} / {total:.2} credits")),
        );
    }
    Ok(snapshot)
}

/// Poll the tRPC base (the documented `https://app.kilo.ai/api/trpc`) with a
/// Kilo API key (`KILO_API_KEY`), requesting `user.getCreditBlocks` in batch
/// form like `CodexBar`'s fetcher does.
pub fn poll_kilo(
    base: &str,
    secret: &PollSecret,
    limits: &PollLimits,
) -> Result<ProviderUsageSnapshot, PollError> {
    let endpoint = format!(
        "{}/user.getCreditBlocks?batch=1&input=%7B%220%22%3A%7B%22json%22%3Anull%7D%7D",
        base.trim_end_matches('/')
    );
    let body = dispatch(&PollRequest::get(&endpoint, bearer(secret)?), limits)?;
    parse_kilo_credits(&body)
}

/// MiniMax-owned domains the strict host policy accepts.
pub const MINIMAX_PROVIDER_DOMAINS: [&str; 2] = ["minimax.io", "minimaxi.com"];
/// Default global Coding Plan host (`www.minimaxi.com` is `CodexBar`'s China
/// mainland region host).
pub const MINIMAX_DEFAULT_HOST: &str = "https://www.minimax.io";
/// Coding Plan remains path on any accepted host.
pub const MINIMAX_REMAINS_PATH: &str = "/v1/api/openplatform/coding_plan/remains";

/// Ported `MiniMax` host policy (`docs/minimax.md` "Security policy"): an
/// endpoint override must be https (a bare host is https-qualified), carry
/// no userinfo, and name a host in ASCII letters, digits, hyphens, and dots
/// only, which refuses literal and percent-encoded delimiters and non-ASCII
/// homographs outright. With `provider_owned_only`
/// (`MINIMAX_REQUIRE_PROVIDER_ENDPOINT_OVERRIDES`) the host must also be a
/// MiniMax-owned DNS name. Returns the normalized `https://` origin.
pub fn validate_minimax_endpoint_override(
    raw: &str,
    provider_owned_only: bool,
) -> Result<String, PollError> {
    let refused = |reason: &str| PollError::Invalid {
        reason: format!("minimax endpoint override refused: {reason}"),
    };
    let cleaned = raw.trim();
    if cleaned.is_empty() {
        return Err(refused("must not be empty"));
    }
    if cleaned.contains("://") && !cleaned.starts_with("https://") {
        return Err(refused("must use https"));
    }
    let rest = cleaned.strip_prefix("https://").unwrap_or(cleaned);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let Some(authority) = rest.get(..authority_end) else {
        return Err(refused("must name a host"));
    };
    if authority.is_empty() {
        return Err(refused("must name a host"));
    }
    if authority.contains('@') {
        return Err(refused("must not carry userinfo"));
    }
    let (host_name, tail) = if let Some(bracketed) = authority.strip_prefix('[') {
        let Some(end) = bracketed.find(']') else {
            return Err(refused("bracketed host must close"));
        };
        let inner = bracketed.get(..end).unwrap_or_default();
        if inner.is_empty()
            || !inner
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        {
            return Err(refused("bracketed host must be an IPv6 literal"));
        }
        (inner, bracketed.get(end + 1..).unwrap_or_default())
    } else {
        let split = authority.rfind(':').unwrap_or(authority.len());
        (
            authority.get(..split).unwrap_or_default(),
            authority.get(split..).unwrap_or_default(),
        )
    };
    if !tail.is_empty()
        && !tail
            .strip_prefix(':')
            .is_some_and(|port| !port.is_empty() && port.chars().all(|d| d.is_ascii_digit()))
    {
        return Err(refused("port must be digits only"));
    }
    let lowered = host_name.to_ascii_lowercase();
    let bracketed = authority.starts_with('[');
    // A DNS host is ASCII letters, digits, hyphens, and dots and nothing
    // else. Refusing every other character closes literal delimiters, the
    // percent-encoded ones, and non-ASCII homographs of MiniMax domains in
    // one fail-closed rule; IPv6 literals only ever arrive bracketed.
    if lowered.is_empty()
        || (!bracketed
            && !lowered
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
    {
        return Err(refused(
            "host must be ASCII letters, digits, hyphens, and dots",
        ));
    }
    if provider_owned_only
        && (bracketed
            || !MINIMAX_PROVIDER_DOMAINS
                .iter()
                .any(|domain| lowered == *domain || lowered.ends_with(&format!(".{domain}"))))
    {
        return Err(refused("strict mode accepts MiniMax-owned DNS hosts only"));
    }
    let authority = if bracketed {
        format!("[{lowered}]{tail}")
    } else {
        format!("{lowered}{tail}")
    };
    Ok(format!("https://{authority}"))
}

fn minimax_percent(entry: &Value) -> Option<f64> {
    if let Some(remaining) = number_field(entry, "current_interval_remaining_percent") {
        return Some(100.0 - remaining);
    }
    let total = number_field(entry, "current_interval_total_count");
    let remaining = number_field(entry, "current_interval_usage_count");
    match (total, remaining) {
        (Some(total), Some(remaining)) if total > 0.0 => {
            Some(used_percent((total - remaining).max(0.0), total))
        }
        _ => None,
    }
}

fn minimax_window(entry: &Value) -> Result<(Option<u32>, Option<Timestamp>), PollError> {
    let Some(end) = number_field(entry, "end_time") else {
        return Ok((None, None));
    };
    let minutes = number_field(entry, "start_time").and_then(|start| epoch_minutes(start, end));
    match minutes {
        Some(minutes) => Ok((Some(minutes), Some(epoch_iso(end)?))),
        None => Ok((None, None)),
    }
}

/// Parse the coding-plan `remains` response: a non-zero `base_resp` status
/// is an error, `model_remains` entries fill the primary lane from the first
/// entry (`100 - current_interval_remaining_percent`, falling back to
/// total-minus-usage counts), and further named models become extras.
pub fn parse_minimax_remains(body: &str) -> Result<ProviderUsageSnapshot, PollError> {
    let root = root_object(body)?;
    let data = obj_field(&root, "data").ok_or_else(|| parse_err("data must be an object"))?;
    let base = data.get("base_resp").or_else(|| root.get("base_resp"));
    if let Some(status) = base
        .and_then(|base| base.get("status_code"))
        .and_then(Value::as_i64)
        && status != 0
    {
        return Err(parse_err("base_resp reported a non-zero status"));
    }
    let models = data
        .get("model_remains")
        .and_then(Value::as_array)
        .ok_or_else(|| parse_err("data.model_remains must be an array"))?;
    let Some(first) = models.first() else {
        return Err(parse_err("model_remains must hold at least one entry"));
    };
    let mut snapshot = ProviderUsageSnapshot::empty("minimax");
    if let Some(percent) = minimax_percent(first) {
        let (minutes, reset) = minimax_window(first)?;
        snapshot.primary = lane(percent, minutes, reset, None);
    }
    for model in models.iter().skip(1) {
        let Some(name) = text_field(model, "model_name") else {
            continue;
        };
        if let Some(percent) = minimax_percent(model) {
            let (minutes, reset) = minimax_window(model)?;
            snapshot.extras.push(NamedLane {
                id: name.to_owned(),
                title: name.to_owned(),
                reading: lane(percent, minutes, reset, None),
            });
        }
    }
    Ok(snapshot)
}

/// Poll the coding-plan remains endpoint with a `MiniMax` Coding Plan API
/// token. `host_override` is the MINIMAX_HOST-style override validated by
/// [`validate_minimax_endpoint_override`]; `provider_owned_only` mirrors
/// `MINIMAX_REQUIRE_PROVIDER_ENDPOINT_OVERRIDES`.
pub fn poll_minimax(
    secret: &PollSecret,
    host_override: Option<&str>,
    provider_owned_only: bool,
    limits: &PollLimits,
) -> Result<ProviderUsageSnapshot, PollError> {
    let base = match host_override {
        Some(raw) => validate_minimax_endpoint_override(raw, provider_owned_only)?,
        None => MINIMAX_DEFAULT_HOST.to_owned(),
    };
    let mut headers = bearer(secret)?;
    headers.push(("Content-Type", "application/json".to_owned()));
    headers.push(("MM-API-Source", "superai".to_owned()));
    let endpoint = format!("{base}{MINIMAX_REMAINS_PATH}");
    let body = dispatch(&PollRequest::get(&endpoint, headers), limits)?;
    parse_minimax_remains(&body)
}

/// Parse the `/api/billing/credits` response: available credits are
/// `wallet_amount + credit_notes_amount - ongoing_usage_balance`, currency
/// uppercased with `XXX` when absent, as `CodexBar` does. The window lanes
/// stay provider-only: they live behind endpoints this poller does not
/// call.
pub fn parse_mistral_credits(body: &str) -> Result<ProviderUsageSnapshot, PollError> {
    let root = root_object(body)?;
    let wallet = number_field(&root, "wallet_amount")
        .ok_or_else(|| parse_err("wallet_amount must be a finite number"))?;
    let optional = |key: &str| -> Result<f64, PollError> {
        number_field(&root, key)
            .ok_or_else(|| parse_err("credit amounts must be finite numbers when present"))
    };
    let notes = match root.get("credit_notes_amount") {
        None | Some(Value::Null) => 0.0,
        Some(_) => optional("credit_notes_amount")?,
    };
    let ongoing = match root.get("ongoing_usage_balance") {
        None | Some(Value::Null) => 0.0,
        Some(_) => optional("ongoing_usage_balance")?,
    };
    let available = wallet + notes - ongoing;
    if !available.is_finite() {
        return Err(parse_err("credit amounts must stay finite"));
    }
    let currency = match root.get("currency").and_then(Value::as_str) {
        Some(code) if !code.trim().is_empty() => code.trim().to_ascii_uppercase(),
        _ => "XXX".to_owned(),
    };
    let mut snapshot = ProviderUsageSnapshot::empty("mistral");
    snapshot.primary = LaneReading::ProviderOnly;
    snapshot.balance = Some(BalanceReading {
        available,
        currency,
        granted: None,
        paid: None,
    });
    Ok(snapshot)
}

/// Poll `endpoint` (the documented
/// `https://admin.mistral.ai/api/billing/credits`) with the admin session
/// cookie header value.
pub fn poll_mistral(
    endpoint: &str,
    cookie_header: &PollSecret,
    limits: &PollLimits,
) -> Result<ProviderUsageSnapshot, PollError> {
    require_secret(cookie_header)?;
    let headers = vec![
        ("Cookie", cookie_header.expose().to_owned()),
        ("Accept", "*/*".to_owned()),
        (
            "Referer",
            "https://admin.mistral.ai/organization/billing".to_owned(),
        ),
        ("Origin", "https://admin.mistral.ai".to_owned()),
    ];
    let body = dispatch(&PollRequest::get(endpoint, headers), limits)?;
    parse_mistral_credits(&body)
}

/// Bounds for one explicit poller invocation, held to the window the health
/// probe already enforces (subplan 07's shipped PRV-07 discipline).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollLimits {
    /// Whole-request timeout.
    pub timeout: Duration,
    /// Response bytes the poller stops at.
    pub max_response_bytes: usize,
}

impl PollLimits {
    /// Accept only bounds the health probe would accept: timeout within
    /// 1..=30s, response cap within 1..=1 MiB.
    pub fn new(timeout: Duration, max_response_bytes: usize) -> Result<Self, UsageError> {
        validate_timeout(timeout).map_err(|e| UsageError::InvalidPollLimits {
            reason: e.to_string(),
        })?;
        if max_response_bytes == 0 || max_response_bytes > MAX_RESPONSE_BYTES {
            return Err(UsageError::InvalidPollLimits {
                reason: format!(
                    "response cap {max_response_bytes} outside 1..={MAX_RESPONSE_BYTES}"
                ),
            });
        }
        Ok(Self {
            timeout,
            max_response_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "explicit Ok/Err checks in tests"
    )]
    use super::*;

    #[test]
    fn bounds_outside_the_probe_window_are_rejected() {
        let cases = [
            (Duration::ZERO, 1024),
            (Duration::from_millis(500), 1024),
            (Duration::from_secs(31), 1024),
            (Duration::from_secs(5), 0),
            (Duration::from_secs(5), MAX_RESPONSE_BYTES + 1),
        ];
        for (timeout, cap) in cases {
            let result = PollLimits::new(timeout, cap);
            assert!(
                matches!(result, Err(UsageError::InvalidPollLimits { .. })),
                "accepted timeout {timeout:?} cap {cap}"
            );
        }
    }

    #[test]
    fn probe_window_edges_are_accepted_as_given() {
        let limits = PollLimits::new(Duration::from_secs(1), 1)
            .unwrap_or_else(|e| panic!("probe floor rejected: {e:?}"));
        assert_eq!(limits.timeout, Duration::from_secs(1));
        assert_eq!(limits.max_response_bytes, 1);
    }

    #[test]
    fn poll_url_gate_keeps_public_traffic_on_https() {
        for url in [
            "http://api.example.com/user/balance",
            "ftp://api.example.com/usage",
            "https://user:pw@api.example.com/usage",
            "https://api.example.com/usa\tge",
            "https:///usage",
        ] {
            assert!(validate_poll_url(url).is_err(), "{url} passed the gate");
        }
        assert!(validate_poll_url("https://api.example.com/user/balance").is_ok());
        assert!(validate_poll_url("http://127.0.0.1:8080/usage/summary").is_ok());
        assert!(validate_poll_url("http://localhost:9/x").is_ok());
        assert!(validate_poll_url("http://127.0.0.1.attacker.com/x").is_err());
    }

    #[test]
    fn secrets_stay_redacted_in_every_rendering() {
        let secret = PollSecret::new("sk-superai-test-sentinel-12345-fake");
        let session = ClinePassSession {
            secret: secret.clone(),
            is_oauth: true,
        };
        for rendered in [
            format!("{secret:?}"),
            format!("{secret}"),
            format!("{session:?}"),
            serde_json::to_string(&secret).unwrap_or_default(),
        ] {
            assert!(
                !rendered.contains("sk-superai-test-sentinel-12345-fake"),
                "leaked in {rendered}"
            );
        }
        assert!(format!("{secret:?}").contains("[REDACTED]"));
    }
}
