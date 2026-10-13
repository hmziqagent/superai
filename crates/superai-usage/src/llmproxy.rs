//! Client for a deployed llmproxy's `/usage` ledger (USG-08, subplan 15;
//! endpoint semantics frozen in docs/plans/export/llmproxy-usage-ledger.md
//! PRX-03). PRX-03 fixes semantics, not JSON keys: the shapes recorded in
//! `tests/fixtures/llmproxy/` are the assumed baseline for drift checking,
//! unknown fields are ignored, and missing or off-format fields surface as
//! `None` rather than failing the response.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use superai_core::template_fetch::{MAX_REDIRECTS, USER_AGENT};

use crate::cost::Usd;
use crate::poller::PollLimits;

/// Aggregate endpoint a deployed ledger serves (PRX-03).
pub const SUMMARY_PATH: &str = "/usage/summary";
/// Raw-row endpoint a deployed ledger serves (PRX-03); responses carry no
/// request bodies, ever.
pub const REQUESTS_PATH: &str = "/usage/requests";
/// Caller-list endpoint a deployed ledger serves (PRX-03), admin only.
pub const KEYS_PATH: &str = "/usage/keys";
/// Environment variable discovery reads the proxy base URL from.
pub const PROXY_URL_ENV_VAR: &str = "SUPERAI_LLMPROXY_URL";
/// Environment variable the admin token is read from.
pub const ADMIN_TOKEN_ENV_VAR: &str = "SUPERAI_LLMPROXY_ADMIN_TOKEN";
/// A bearer token longer than this is not a token file.
const TOKEN_FILE_MAX_BYTES: usize = 4096;

/// Everything the ledger client can fail with; no variant ever carries the
/// admin token.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// The proxy base URL is not usable.
    #[error("invalid proxy base URL: {reason}")]
    InvalidBase {
        /// What was wrong.
        reason: String,
    },
    /// A request parameter was rejected before any network call.
    #[error("invalid ledger request: {reason}")]
    InvalidParam {
        /// What was wrong.
        reason: String,
    },
    /// Neither the token environment variable nor a token file was given.
    #[error("no admin token: set {ADMIN_TOKEN_ENV_VAR} or pass a 0600 token file")]
    MissingToken,
    /// The token file exists but was refused.
    #[error("token file {path}: {reason}")]
    TokenFile {
        /// File that was refused.
        path: String,
        /// Why it was refused.
        reason: String,
    },
    /// The ledger could not be reached or did not answer in time.
    #[error("ledger unreachable: {reason}")]
    Unreachable {
        /// Transport failure, URL only.
        reason: String,
    },
    /// The ledger answered with a non-success status.
    #[error("ledger returned http {code}")]
    Status {
        /// Status code the ledger returned.
        code: u16,
    },
    /// The response body exceeded the byte cap.
    #[error("ledger response exceeded {max_bytes} bytes")]
    TooLarge {
        /// Cap that was exceeded.
        max_bytes: usize,
    },
    /// The ledger redirected beyond the follow limit.
    #[error("ledger redirected more than {MAX_REDIRECTS} times")]
    RedirectLimit,
    /// The body did not match the assumed response shape.
    #[error("ledger response did not match the assumed shape: {reason}")]
    Parse {
        /// Bounded serde reason.
        reason: String,
    },
}

/// Grouping `/usage/summary` accepts for its `group_by` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryGroupBy {
    /// One row per provider.
    Provider,
    /// One row per model.
    Model,
    /// One row per client key id.
    Caller,
}

impl SummaryGroupBy {
    /// Wire value for the `group_by` query parameter.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Model => "model",
            Self::Caller => "caller",
        }
    }
}

/// The ledger's admin bearer token. `Debug` and `Display` never show the
/// bytes; the raw value leaves this type only at the Authorization header.
#[derive(Clone, PartialEq, Eq)]
pub struct AdminToken(String);

impl AdminToken {
    /// Wrap a token value already read by the caller (an env var).
    pub fn new(secret: &str) -> Self {
        Self(secret.to_owned())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AdminToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AdminToken([REDACTED])")
    }
}

impl fmt::Display for AdminToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Serialize for AdminToken {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("[REDACTED]")
    }
}

/// Cost as the ledger reported it: an answer, or the reason none arrived.
/// `Reported(None)` means the ledger answered but its cost field was
/// missing or unparseable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerCost {
    /// The proxy could not be reached; local numbers stand alone.
    Unreachable {
        /// Transport failure, URL only.
        reason: String,
    },
    /// The ledger answered; the amount is `None` when unrepresentable.
    Reported(Option<Usd>),
}

/// USG-08 merge: the ledger number wins for routed instances and the local
/// scan is carried as a cross-check; the two are never summed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerCrossCheck {
    /// Ledger-side cost.
    pub ledger: LedgerCost,
    /// Local-scan cost, `None` when the scan could not price.
    pub local: Option<Usd>,
}

impl LedgerCrossCheck {
    /// Pair the two numbers; neither is altered.
    pub fn new(ledger: LedgerCost, local: Option<Usd>) -> Self {
        Self { ledger, local }
    }

    /// The amount display should show: the ledger number only, never a
    /// local substitute or a blend.
    pub fn winning_cost(&self) -> Option<Usd> {
        match &self.ledger {
            LedgerCost::Unreachable { .. } => None,
            LedgerCost::Reported(cost) => *cost,
        }
    }

    /// Absolute ledger-minus-local difference, present only when both
    /// sides carry a priced amount.
    pub fn divergence(&self) -> Option<Usd> {
        let ledger = self.winning_cost()?;
        let local = self.local?;
        let (hi, lo) = if ledger.as_picos() >= local.as_picos() {
            (ledger.as_picos(), local.as_picos())
        } else {
            (local.as_picos(), ledger.as_picos())
        };
        Some(Usd::from_picos(hi - lo))
    }

    /// Whether the divergence line should be shown.
    pub fn diverged(&self) -> bool {
        self.divergence().is_some_and(|delta| delta != Usd::ZERO)
    }
}

/// One aggregated row of `/usage/summary`, keyed by the `group_by` value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SummaryRow {
    /// Value of the grouping key (provider, model, or caller id).
    #[serde(default)]
    pub group: Option<String>,
    /// Requests counted in the window.
    #[serde(default)]
    pub requests: Option<u64>,
    /// Failed requests in the window.
    #[serde(default)]
    pub errors: Option<u64>,
    /// Input tokens in the window.
    #[serde(default)]
    pub input_tokens: Option<u64>,
    /// Output tokens in the window.
    #[serde(default)]
    pub output_tokens: Option<u64>,
    /// Cache-read tokens in the window.
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    /// Cache-creation tokens in the window.
    #[serde(default)]
    pub cache_write_tokens: Option<u64>,
    /// Exact recorded cost; `None` when missing or unparseable.
    #[serde(default, deserialize_with = "deserialize_cost_option")]
    pub cost_usd: Option<Usd>,
}

/// Parsed `/usage/summary` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    /// Window echo, when the ledger returns one.
    #[serde(default)]
    pub window: Option<String>,
    /// One row per group.
    pub rows: Vec<SummaryRow>,
}

impl UsageSummary {
    /// Sum of row costs; `None` when any row's cost is unrepresentable, so
    /// a partial total can never pose as the whole one.
    pub fn total_cost(&self) -> Option<Usd> {
        self.rows
            .iter()
            .try_fold(Usd::ZERO, |sum, row| sum.checked_add(row.cost_usd?))
    }
}

/// One raw row of `/usage/requests`; request bodies never appear here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestRow {
    /// Ledger's request id.
    #[serde(default)]
    pub request_id: Option<String>,
    /// When the proxy received the request.
    #[serde(default, deserialize_with = "deserialize_timestamp_option")]
    pub received_at: Option<Timestamp>,
    /// When the proxy completed the request.
    #[serde(default, deserialize_with = "deserialize_timestamp_option")]
    pub completed_at: Option<Timestamp>,
    /// Upstream provider the request routed to.
    #[serde(default)]
    pub provider: Option<String>,
    /// Requested model id.
    #[serde(default)]
    pub model: Option<String>,
    /// Model the upstream actually served, when it differs.
    #[serde(default)]
    pub upstream_model: Option<String>,
    /// Route kind (chat, completions, embeddings, ...).
    #[serde(default)]
    pub route_kind: Option<String>,
    /// Client protocol spoken to the proxy.
    #[serde(default)]
    pub client_protocol: Option<String>,
    /// Whether the response streamed.
    #[serde(default)]
    pub streaming: Option<bool>,
    /// Client key id or `"anonymous"`.
    #[serde(default)]
    pub caller: Option<String>,
    /// Input tokens.
    #[serde(default)]
    pub input_tokens: Option<u64>,
    /// Output tokens.
    #[serde(default)]
    pub output_tokens: Option<u64>,
    /// Cache-read tokens.
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    /// Cache-creation tokens.
    #[serde(default)]
    pub cache_write_tokens: Option<u64>,
    /// Exact recorded cost; `None` when missing or unparseable.
    #[serde(default, deserialize_with = "deserialize_cost_option")]
    pub cost_usd: Option<Usd>,
    /// Upstream stop reason.
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// Failure classification, for failed requests.
    #[serde(default)]
    pub error_kind: Option<String>,
    /// HTTP status returned to the client.
    #[serde(default)]
    pub http_status: Option<u16>,
    /// Whole-request latency.
    #[serde(default)]
    pub latency_ms: Option<u64>,
    /// Dedup-safe body digest; never the body itself.
    #[serde(default)]
    pub body_hash: Option<String>,
}

/// Parsed `/usage/requests` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRequests {
    /// Bounded rows the ledger returned.
    pub rows: Vec<RequestRow>,
}

/// One caller row of `/usage/keys`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyRow {
    /// Client key id.
    #[serde(default)]
    pub caller: Option<String>,
    /// When the key was last seen.
    #[serde(default, deserialize_with = "deserialize_timestamp_option")]
    pub last_seen: Option<Timestamp>,
}

/// Parsed `/usage/keys` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageKeys {
    /// Known callers.
    pub keys: Vec<KeyRow>,
}

/// Window and filters for one `/usage/requests` call. `since`/`until` are
/// sent as RFC 3339 strings, the shape the fixtures record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestsQuery<'a> {
    /// Inclusive lower bound.
    pub since: Option<Timestamp>,
    /// Inclusive upper bound.
    pub until: Option<Timestamp>,
    /// Restrict to one provider id.
    pub provider: Option<&'a str>,
    /// Maximum rows the ledger may return.
    pub limit: Option<u32>,
}

impl RequestsQuery<'_> {
    /// Empty query: whole ledger history within its retention, unfiltered.
    pub const ALL: Self = Self {
        since: None,
        until: None,
        provider: None,
        limit: None,
    };
}

/// Client for one deployed llmproxy's `/usage` endpoints. Holds the admin
/// token in [`AdminToken`]; errors never expose it.
#[derive(Debug)]
pub struct LedgerClient {
    base_url: String,
    token: AdminToken,
    limits: PollLimits,
    agent: ureq::Agent,
}

impl LedgerClient {
    /// Build a client for `base_url` (an origin such as
    /// `http://127.0.0.1:8080`, optionally carrying one of the known
    /// `/usage` paths, which is stripped). Plain http is accepted only on
    /// a loopback host, mirroring the registry's endpoint rule.
    pub fn new(
        base_url: &str,
        token: &AdminToken,
        limits: PollLimits,
    ) -> Result<Self, LedgerError> {
        Ok(Self {
            base_url: normalize_base(base_url)?,
            token: token.clone(),
            limits,
            agent: build_agent(limits.timeout),
        })
    }

    /// Fetch the window aggregate grouped by `group_by`.
    pub fn summary(
        &self,
        window: &str,
        group_by: SummaryGroupBy,
    ) -> Result<UsageSummary, LedgerError> {
        if window.trim().is_empty() {
            return Err(LedgerError::InvalidParam {
                reason: "window must not be empty".to_owned(),
            });
        }
        let query = [
            ("window", window.to_owned()),
            ("group_by", group_by.as_str().to_owned()),
        ];
        let bytes = self.get_bounded(SUMMARY_PATH, &query)?;
        serde_json::from_slice(&bytes).map_err(|e| LedgerError::Parse {
            reason: bound_reason(&e.to_string()),
        })
    }

    /// Fetch bounded raw request rows; no request body is ever returned.
    pub fn requests(&self, query: &RequestsQuery<'_>) -> Result<UsageRequests, LedgerError> {
        let mut pairs: Vec<(&str, String)> = Vec::new();
        if let Some(since) = query.since {
            pairs.push(("since", since.to_string()));
        }
        if let Some(until) = query.until {
            pairs.push(("until", until.to_string()));
        }
        if let Some(provider) = query.provider {
            if provider.trim().is_empty() {
                return Err(LedgerError::InvalidParam {
                    reason: "provider must not be empty".to_owned(),
                });
            }
            pairs.push(("provider", provider.to_owned()));
        }
        if let Some(limit) = query.limit {
            pairs.push(("limit", limit.to_string()));
        }
        let bytes = self.get_bounded(REQUESTS_PATH, &pairs)?;
        serde_json::from_slice(&bytes).map_err(|e| LedgerError::Parse {
            reason: bound_reason(&e.to_string()),
        })
    }

    /// Fetch the caller list (admin).
    pub fn keys(&self) -> Result<UsageKeys, LedgerError> {
        let bytes = self.get_bounded(KEYS_PATH, &[])?;
        serde_json::from_slice(&bytes).map_err(|e| LedgerError::Parse {
            reason: bound_reason(&e.to_string()),
        })
    }

    fn get_bounded(&self, path: &str, query: &[(&str, String)]) -> Result<Vec<u8>, LedgerError> {
        let mut current = format!("{}{path}", self.base_url);
        for _ in 0..=MAX_REDIRECTS {
            let mut request = self
                .agent
                .get(&current)
                .header("Authorization", format!("Bearer {}", self.token.expose()))
                .header("Accept", "application/json");
            for (key, value) in query {
                request = request.query(key, value);
            }
            let mut response = request.call().map_err(|e| match e {
                ureq::Error::StatusCode(code) => LedgerError::Status { code },
                other => LedgerError::Unreachable {
                    reason: format!("{other} for `{current}`"),
                },
            })?;
            let status = response.status().as_u16();
            if (300..400).contains(&status) {
                current = redirect_target(&current, &response)?;
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(LedgerError::Status { code: status });
            }
            if let Some(len) = content_length(response.headers())
                && len > self.limits.max_response_bytes
            {
                return Err(LedgerError::TooLarge {
                    max_bytes: self.limits.max_response_bytes,
                });
            }
            let mut body = Vec::new();
            let reader = response.body_mut().as_reader();
            let mut limited = reader.take(self.limits.max_response_bytes as u64 + 1);
            limited
                .read_to_end(&mut body)
                .map_err(|e| LedgerError::Unreachable {
                    reason: format!("read failed for `{current}`: {e}"),
                })?;
            if body.len() > self.limits.max_response_bytes {
                return Err(LedgerError::TooLarge {
                    max_bytes: self.limits.max_response_bytes,
                });
            }
            return Ok(body);
        }
        Err(LedgerError::RedirectLimit)
    }
}

/// Read the admin token: a non-empty `env_value` wins, then a 0600-mode
/// regular file, else an error. The token is returned wrapped and never
/// logged or stored in instance records.
pub fn load_admin_token(
    env_value: Option<&str>,
    token_file: Option<&Path>,
) -> Result<AdminToken, LedgerError> {
    if let Some(value) = env_value.map(str::trim).filter(|value| !value.is_empty()) {
        return Ok(AdminToken::new(value));
    }
    let Some(path) = token_file else {
        return Err(LedgerError::MissingToken);
    };
    read_token_file(path)
}

/// Discovery order (USG-08): an explicit flag, then the environment
/// variable's value, then the first usable base among instance provider
/// configs known to point at an llmproxy deployment. Bases from the last
/// group that fail validation are skipped, not fatal; the first two are
/// operator input and fail loudly.
pub fn resolve_base_url(
    explicit: Option<&str>,
    env_url: Option<&str>,
    routed_instance_bases: &[&str],
) -> Result<Option<String>, LedgerError> {
    if let Some(url) = explicit {
        return Ok(Some(normalize_base(url)?));
    }
    if let Some(url) = env_url.map(str::trim).filter(|value| !value.is_empty()) {
        return Ok(Some(normalize_base(url)?));
    }
    for base in routed_instance_bases {
        if let Ok(normalized) = normalize_base(base) {
            return Ok(Some(normalized));
        }
    }
    Ok(None)
}

fn read_token_file(path: &Path) -> Result<AdminToken, LedgerError> {
    let refused = |reason: String| LedgerError::TokenFile {
        path: path.display().to_string(),
        reason,
    };
    // Pre-open stat refuses FIFOs and symlinks without following; the open
    // fd's own metadata stays the authority on mode and size.
    let stat = std::fs::symlink_metadata(path).map_err(|e| refused(format!("unreadable: {e}")))?;
    if !stat.is_file() {
        return Err(refused("not a regular file".to_owned()));
    }
    let file = File::open(path).map_err(|e| refused(format!("open failed: {e}")))?;
    let meta = file
        .metadata()
        .map_err(|e| refused(format!("stat failed: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 {
            return Err(refused(format!("mode {mode:o} is not 0600")));
        }
    }
    if meta.len() > TOKEN_FILE_MAX_BYTES as u64 {
        return Err(refused(format!(
            "{} bytes exceeds the {} byte cap",
            meta.len(),
            TOKEN_FILE_MAX_BYTES
        )));
    }
    let mut text = String::new();
    let mut limited = file.take(TOKEN_FILE_MAX_BYTES as u64 + 1);
    limited
        .read_to_string(&mut text)
        .map_err(|e| refused(format!("read failed: {e}")))?;
    if text.len() > TOKEN_FILE_MAX_BYTES {
        return Err(refused("grew past the byte cap while reading".to_owned()));
    }
    let token = text.trim();
    if token.is_empty() {
        return Err(refused("holds no token".to_owned()));
    }
    Ok(AdminToken::new(token))
}

fn build_agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        // Redirects are followed by `get_bounded` so each hop re-passes
        // base validation before the token is sent.
        .max_redirects(0)
        .max_redirects_will_error(false)
        .user_agent(USER_AGENT)
        .build();
    ureq::Agent::new_with_config(config)
}

fn content_length(headers: &ureq::http::HeaderMap) -> Option<usize> {
    headers
        .get("Content-Length")
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.parse::<usize>().ok())
}

fn redirect_target(
    current: &str,
    response: &ureq::http::Response<ureq::Body>,
) -> Result<String, LedgerError> {
    let Some(location) = response
        .headers()
        .get("Location")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    else {
        return Err(LedgerError::Unreachable {
            reason: format!(
                "redirect {} without a Location header for `{current}`",
                response.status().as_u16()
            ),
        });
    };
    resolve_redirect(current, &location)
}

/// Resolve a redirect `Location`: absolute http(s), or an absolute path on
/// the current origin; anything else is refused. The target re-passes
/// endpoint validation, queries allowed, before the next request.
fn resolve_redirect(base: &str, location: &str) -> Result<String, LedgerError> {
    let refused = |reason: String| LedgerError::InvalidBase {
        reason: format!("redirect target `{location}` refused: {reason}"),
    };
    let resolved = if location.starts_with("http://") || location.starts_with("https://") {
        location.to_owned()
    } else if location.starts_with('/') {
        origin_of(base)
            .map(|origin| format!("{origin}{location}"))
            .ok_or_else(|| LedgerError::InvalidBase {
                reason: format!("redirect from `{base}` has no origin"),
            })?
    } else {
        return Err(refused("location must be absolute".to_owned()));
    };
    endpoint_url_reason(&resolved).map_or(Ok(resolved), |reason| Err(refused(reason)))
}

fn origin_of(url: &str) -> Option<&str> {
    let rest = url.get(url.find("://")? + 3..)?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest.get(..authority_end)
}

fn normalize_base(raw: &str) -> Result<String, LedgerError> {
    if let Some(reason) = base_url_reason(raw) {
        return Err(LedgerError::InvalidBase { reason });
    }
    let mut base = raw.trim_end_matches('/');
    for path in [SUMMARY_PATH, REQUESTS_PATH, KEYS_PATH] {
        if let Some(stripped) = base.strip_suffix(path) {
            base = stripped.trim_end_matches('/');
        }
    }
    Ok(base.to_owned())
}

/// Same rule the registry pins for endpoint locators: https anywhere, plain
/// http only on an exact loopback host, no userinfo, query, or control
/// characters.
fn base_url_reason(url: &str) -> Option<String> {
    if let Some(reason) = endpoint_url_reason(url) {
        return Some(reason);
    }
    if url.contains(['?', '#']) {
        return Some("must be a base URL without query or fragment".to_owned());
    }
    None
}

fn endpoint_url_reason(url: &str) -> Option<String> {
    let (rest, https) = if let Some(rest) = url.strip_prefix("https://") {
        (rest, true)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (rest, false)
    } else {
        return Some("must be an http or https URL".to_owned());
    };
    if url
        .chars()
        .any(|c| char::is_control(c) || c.is_whitespace())
    {
        return Some("must not contain control or whitespace characters".to_owned());
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest.get(..authority_end).unwrap_or_default();
    if authority.contains('@') {
        return Some("must not carry userinfo".to_owned());
    }
    let host = strip_port(authority);
    if host.is_empty() {
        return Some("must name a host".to_owned());
    }
    if !https && !is_loopback_host(host) {
        return Some("plain http is allowed only on a loopback host".to_owned());
    }
    None
}

/// Exact loopback match only: `127.0.0.1.attacker.com` is a DNS name an
/// attacker controls, not a loopback address.
fn is_loopback_host(host: &str) -> bool {
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

fn strip_port(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default();
    }
    authority.split(':').next().unwrap_or_default()
}

fn bound_reason(reason: &str) -> String {
    reason.chars().take(160).collect()
}

fn deserialize_cost_option<'de, D>(deserializer: D) -> Result<Option<Usd>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| match value {
        serde_json::Value::String(text) => Usd::parse(&text).ok(),
        serde_json::Value::Number(number) => Usd::parse(&number.to_string()).ok(),
        _ => None,
    }))
}

fn deserialize_timestamp_option<'de, D>(deserializer: D) -> Result<Option<Timestamp>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| match value {
        serde_json::Value::String(text) => Timestamp::from_str(&text).ok(),
        serde_json::Value::Number(number) => number
            .as_i64()
            .and_then(|millis| Timestamp::from_millisecond(millis).ok()),
        _ => None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls_off_loopback_plain_http_or_with_secrets_are_refused() {
        for url in [
            "http://usage.example.com",
            "http://127.0.0.1.attacker.com",
            "https://user:secret@example.com",
            "https://user@example.com",
            "https://example.com/usage?a=b",
            "https://example.com/usage#frag",
            "https:///usage",
            "ftp://127.0.0.1:8080",
            "https://127.0.0.1:8080/u\u{7}sage",
        ] {
            assert!(base_url_reason(url).is_some(), "`{url}` passed validation");
        }
        for url in [
            "https://proxy.example.com",
            "http://127.0.0.1:8080",
            "http://localhost",
            "http://[::1]:9910",
        ] {
            assert!(base_url_reason(url).is_none(), "`{url}` was refused");
        }
    }

    #[test]
    fn known_usage_paths_are_stripped_from_a_base_once() {
        let normalized =
            |url: &str| normalize_base(url).unwrap_or_else(|e| panic!("`{url}` refused: {e:?}"));
        assert_eq!(
            normalized("http://127.0.0.1:8080/usage/summary/"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            normalized("http://127.0.0.1:8080/usage/keys"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            normalized("http://localhost:8080///"),
            "http://localhost:8080"
        );
        assert_eq!(
            normalized("http://127.0.0.1:8080/v1"),
            "http://127.0.0.1:8080/v1"
        );
        assert!(matches!(
            normalize_base("http://far.example.com"),
            Err(LedgerError::InvalidBase { .. })
        ));
    }

    #[test]
    fn discovery_prefers_flag_then_env_then_any_usable_routed_base() {
        let resolved = |explicit: Option<&str>, env: Option<&str>, routed: &[&str]| {
            resolve_base_url(explicit, env, routed)
                .unwrap_or_else(|e| panic!("valid discovery inputs refused: {e:?}"))
        };
        assert_eq!(
            resolved(
                Some("http://127.0.0.1:9000"),
                Some("http://127.0.0.1:9001"),
                &["https://routed.example.com"]
            )
            .as_deref(),
            Some("http://127.0.0.1:9000")
        );
        assert_eq!(
            resolved(None, Some("http://127.0.0.1:9001"), &[]).as_deref(),
            Some("http://127.0.0.1:9001")
        );
        assert_eq!(
            resolved(None, None, &["not a url", "https://routed.example.com"]).as_deref(),
            Some("https://routed.example.com")
        );
        assert_eq!(resolved(None, None, &["not a url"]), None);
        assert!(matches!(
            resolve_base_url(Some("http://far.example.com"), None, &[]),
            Err(LedgerError::InvalidBase { .. })
        ));
        assert!(matches!(
            resolve_base_url(None, Some("http://far.example.com"), &[]),
            Err(LedgerError::InvalidBase { .. })
        ));
    }

    #[test]
    fn cross_check_never_blends_or_swaps_the_two_sides() {
        let ledger = Usd::from_picos(3_645_500_000_000);
        let local = Usd::from_picos(3_000_000_000_000);
        let check = LedgerCrossCheck::new(LedgerCost::Reported(Some(ledger)), Some(local));
        assert_eq!(check.winning_cost(), Some(ledger));
        assert_eq!(check.divergence(), Some(Usd::from_picos(645_500_000_000)));
        assert!(check.diverged());

        let agreeing = LedgerCrossCheck::new(LedgerCost::Reported(Some(local)), Some(local));
        assert!(!agreeing.diverged());
        assert_eq!(agreeing.divergence(), Some(Usd::ZERO));

        let unreachable = LedgerCrossCheck::new(
            LedgerCost::Unreachable {
                reason: "refused".to_owned(),
            },
            Some(local),
        );
        assert_eq!(unreachable.winning_cost(), None);
        assert_eq!(unreachable.divergence(), None);
        assert!(!unreachable.diverged());
        assert_eq!(unreachable.local, Some(local));

        let unpriced = LedgerCrossCheck::new(LedgerCost::Reported(None), Some(local));
        assert_eq!(unpriced.winning_cost(), None);
        assert!(!unpriced.diverged());
    }

    #[test]
    fn summary_total_is_none_when_any_row_lacks_a_cost() {
        let mut summary = UsageSummary {
            window: Some("24h".to_owned()),
            rows: vec![SummaryRow {
                cost_usd: Some(Usd::from_picos(10)),
                ..SummaryRow::default()
            }],
        };
        assert_eq!(summary.total_cost(), Some(Usd::from_picos(10)));
        summary.rows.push(SummaryRow::default());
        assert_eq!(summary.total_cost(), None);
    }

    #[test]
    fn off_format_costs_and_timestamps_parse_to_none() {
        let row: SummaryRow =
            serde_json::from_str(r#"{"group":"anthropic","cost_usd":"not-a-number"}"#)
                .unwrap_or_else(|e| panic!("tolerant parse failed: {e:?}"));
        assert_eq!(row.group.as_deref(), Some("anthropic"));
        assert_eq!(row.cost_usd, None);

        let row: RequestRow = serde_json::from_str(
            r#"{"received_at":"yesterday","completed_at":"2026-10-13T08:00:03Z","latency_ms":3200}"#,
        )
        .unwrap_or_else(|e| panic!("tolerant parse failed: {e:?}"));
        assert_eq!(row.received_at, None);
        assert!(row.completed_at.is_some());
        assert_eq!(row.latency_ms, Some(3200));
    }
}
