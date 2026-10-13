//! JSONL transcript and rollout readers (USG-03): claude-code projects,
//! codex-cli rollouts, pi/omp sessions. Zero network, zero writes; token
//! counts and model ids are the only artifact bytes that ever leave a file.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use jiff::Timestamp;
use serde_json::Value;

use super::{
    CLAUDE_LINE_CAP, CODEX_LINE_CAP, PI_LINE_CAP, ScanOutcome, for_each_line, jsonl_files,
};
use crate::cost::UsageEvent;

const ANTHROPIC: &str = "anthropic";
const OPENAI: &str = "openai";
const AMAZON_BEDROCK: &str = "amazon-bedrock";

/// `CodexBar`'s identifier cap: longer model or session strings are refused
/// whole, never truncated into a lookalike id.
const MAX_IDENTIFIER_BYTES: usize = 1024;

const PROFILE_REFUSAL: &str = "OMP_PROFILE/PI_PROFILE is not a valid profile name";
const CONFIG_REFUSAL: &str = "PI_CONFIG_DIR must be a relative path inside HOME";

const PI_INPUT_KEYS: &[&str] = &[
    "input",
    "inputTokens",
    "input_tokens",
    "promptTokens",
    "prompt_tokens",
];
const PI_CACHE_READ_KEYS: &[&str] = &[
    "cacheRead",
    "cacheReadTokens",
    "cache_read",
    "cache_read_tokens",
    "cacheReadInputTokens",
    "cache_read_input_tokens",
];
const PI_CACHE_WRITE_KEYS: &[&str] = &[
    "cacheWrite",
    "cacheWriteTokens",
    "cache_write",
    "cache_write_tokens",
    "cacheCreationTokens",
    "cache_creation_tokens",
    "cacheCreationInputTokens",
    "cache_creation_input_tokens",
];
const PI_OUTPUT_KEYS: &[&str] = &[
    "output",
    "outputTokens",
    "output_tokens",
    "completionTokens",
    "completion_tokens",
];

/// Claude transcript roots (`CodexBar`'s `ClaudeConfigPaths.costProjectsRoots`):
/// the configured root's `projects/` when `CLAUDE_CONFIG_DIR` is set, else
/// `~/.config/claude/projects` ahead of `~/.claude/projects`. Cswap slot
/// roots join either way; upstream's slot loop sits outside the env guard.
pub fn claude_projects_roots(
    claude_config_dir: Option<&str>,
    xdg_data_home: Option<&Path>,
    home: &Path,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(configured) = claude_config_dir.filter(|dir| !dir.is_empty()) {
        roots.push(Path::new(configured).join("projects"));
    } else {
        roots.push(home.join(".config").join("claude").join("projects"));
        roots.push(home.join(".claude").join("projects"));
    }
    let legacy = home.join(".claude-swap-backup").join("sessions");
    push_swap_slots(&legacy, &mut roots);
    let xdg_base = absolute_or(xdg_data_home, &home.join(".local").join("share"));
    push_swap_slots(&xdg_base.join("claude-swap").join("sessions"), &mut roots);
    dedup_roots(roots)
}

/// Codex rollout roots (`CodexBar`'s `CostUsageScanner`): `CODEX_HOME/sessions`
/// else `~/.codex/sessions`, plus the `archived_sessions` sibling.
pub fn codex_sessions_roots(codex_home: Option<&str>, home: &Path) -> Vec<PathBuf> {
    let base = match codex_home.map(str::trim).filter(|dir| !dir.is_empty()) {
        Some(dir) => Path::new(dir).to_path_buf(),
        None => home.join(".codex"),
    };
    vec![base.join("sessions"), base.join("archived_sessions")]
}

/// Environment selectors for pi/omp session roots; each field is the
/// matching variable's raw value, `None` when unset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PiSelectors<'a> {
    /// `PI_CODING_AGENT_SESSION_DIR`: an explicit sessions directory.
    pub session_dir: Option<&'a str>,
    /// `PI_CODING_AGENT_DIR`: an agent root whose `sessions/` is scanned.
    pub agent_dir: Option<&'a str>,
    /// `PI_CONFIG_DIR`: OMP's config root, relative to HOME.
    pub config_dir: Option<&'a str>,
    /// `OMP_PROFILE`, the preferred profile selector.
    pub omp_profile: Option<&'a str>,
    /// `PI_PROFILE`, used only when `OMP_PROFILE` is absent.
    pub pi_profile: Option<&'a str>,
    /// `XDG_DATA_HOME`; relative values are ignored per the XDG rule.
    pub xdg_data_home: Option<&'a Path>,
}

/// Resolved pi/omp session roots plus any selector refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiSessionRoots {
    /// Session directories to scan, deduplicated.
    pub roots: Vec<PathBuf>,
    /// Display-safe refusal when an explicit selector could not be
    /// honored; `roots` is empty and nothing may be read.
    pub refusal: Option<&'static str>,
    /// Discovery dropped profiles past its 64-profile cap; totals over
    /// these roots are floors, not complete coverage.
    pub profile_cap_hit: bool,
}

/// pi/omp session roots (`CodexBar`'s `PiFamilySessionScanner`): an explicit
/// session dir wins outright; otherwise the pi agent root joins the OMP
/// agent store, every on-disk profile when no profile selector is set, or
/// just the named profile when one is.
pub fn pi_session_roots(selectors: &PiSelectors<'_>, home: &Path) -> PiSessionRoots {
    if let Some(dir) = trimmed(selectors.session_dir) {
        return PiSessionRoots {
            roots: dedup_roots(vec![PathBuf::from(dir)]),
            refusal: None,
            profile_cap_hit: false,
        };
    }
    let refused = |reason: &'static str| PiSessionRoots {
        roots: Vec::new(),
        refusal: Some(reason),
        profile_cap_hit: false,
    };
    let Some(config) = omp_config_root(selectors.config_dir, home) else {
        return refused(CONFIG_REFUSAL);
    };
    let xdg_base = absolute_or(selectors.xdg_data_home, &home.join(".local").join("share"));
    let agent_dir = trimmed(selectors.agent_dir);
    let mut roots = Vec::new();
    match agent_dir {
        Some(dir) => roots.push(Path::new(dir).join("sessions")),
        None => roots.push(home.join(".pi").join("agent").join("sessions")),
    }
    let profile_selector = selectors.omp_profile.or(selectors.pi_profile);
    let mut profile_cap_hit = false;
    match normalize_profile(profile_selector) {
        ProfileValue::Invalid => return refused(PROFILE_REFUSAL),
        ProfileValue::Default => {
            if agent_dir.is_none() {
                roots.push(config.join("agent").join("sessions"));
                let xdg_sessions = xdg_base.join("omp").join("sessions");
                if xdg_sessions.is_dir() {
                    roots.push(xdg_sessions);
                }
            }
            if profile_selector.is_none() {
                profile_cap_hit |= discover_profile_roots(&config.join("profiles"), &mut roots);
                if agent_dir.is_none() {
                    profile_cap_hit |=
                        discover_profile_roots(&xdg_base.join("omp").join("profiles"), &mut roots);
                }
            }
        }
        ProfileValue::Named(name) => {
            push_named_profile_roots(&mut roots, &config, &xdg_base, &name);
        }
    }
    PiSessionRoots {
        roots: dedup_roots(roots),
        refusal: None,
        profile_cap_hit,
    }
}

/// Scan claude-code transcript JSONL under the resolved project roots.
/// Streaming chunks repeat cumulative usage; the most complete chunk per
/// message wins, totals only break ties within one completeness class, and
/// a copied transcript under two roots counts once.
pub fn scan_claude_transcripts(roots: &[PathBuf]) -> ScanOutcome {
    let mut outcome = ScanOutcome::default();
    let mut keyed: BTreeMap<ClaudeKey, KeptClaude> = BTreeMap::new();
    let mut unkeyed = Vec::new();
    for path in jsonl_files(roots, &mut outcome.unreadable_artifacts) {
        for_each_line(
            &path,
            &mut outcome,
            CLAUDE_LINE_CAP,
            |bytes, counts| match claude_line(bytes) {
                LineOutcome::Parsed => counts.parsed += 1,
                LineOutcome::Skipped => counts.skipped += 1,
                LineOutcome::Row(row) => {
                    counts.parsed += 1;
                    keep_claude_row(&mut keyed, &mut unkeyed, row);
                }
                LineOutcome::RowWithGap(row) => {
                    counts.skipped += 1;
                    keep_claude_row(&mut keyed, &mut unkeyed, row);
                }
            },
        );
    }
    outcome.events = keyed
        .into_values()
        .map(|kept| kept.event)
        .chain(unkeyed)
        .collect();
    outcome
}

/// Scan codex-cli rollout JSONL under the resolved session roots. Usage is
/// the per-file delta of cumulative token totals (`total_token_usage`, the
/// `last_token_usage` fallback, or a `token_usage_record` ledger line's
/// `thread_token_usage`), so replays and token-count/ledger mirror pairs
/// count once.
/// Codex records cached reads as a subset of `input_tokens`; events split
/// them so the input rate covers only the uncached remainder.
pub fn scan_codex_rollouts(roots: &[PathBuf]) -> ScanOutcome {
    let mut outcome = ScanOutcome::default();
    let mut events = Vec::new();
    for path in jsonl_files(roots, &mut outcome.unreadable_artifacts) {
        let mut file = CodexFile::default();
        for_each_line(
            &path,
            &mut outcome,
            CODEX_LINE_CAP,
            |bytes, counts| match codex_line(bytes, &mut file) {
                LineOutcome::Parsed => counts.parsed += 1,
                LineOutcome::Skipped => counts.skipped += 1,
                LineOutcome::Row(event) | LineOutcome::RowWithGap(event) => {
                    counts.parsed += 1;
                    events.push(event);
                }
            },
        );
    }
    outcome.events = events;
    outcome
}

/// Scan pi/omp session JSONL under the resolved roots. Costs are rate
/// estimates over recorded assistant usage; an entry id repeated under
/// overlapping roots counts once.
pub fn scan_pi_sessions(roots: &[PathBuf]) -> ScanOutcome {
    let mut outcome = ScanOutcome::default();
    let mut events = Vec::new();
    let mut seen_entries: BTreeSet<(String, String)> = BTreeSet::new();
    for path in jsonl_files(roots, &mut outcome.unreadable_artifacts) {
        let mut file = PiFile::default();
        for_each_line(
            &path,
            &mut outcome,
            PI_LINE_CAP,
            |bytes, counts| match pi_line(bytes, &mut file) {
                LineOutcome::Parsed => counts.parsed += 1,
                LineOutcome::Skipped => counts.skipped += 1,
                LineOutcome::Row(row) | LineOutcome::RowWithGap(row) => {
                    counts.parsed += 1;
                    keep_pi_row(row, &mut seen_entries, &mut events);
                }
            },
        );
    }
    outcome.events = events;
    outcome
}

/// On-disk profile discovery over at most 64 profiles per pass (`CodexBar`
/// caps its own discovery at 64 roots); symlinked entries are skipped.
/// Returns whether the cap dropped any profile.
fn discover_profile_roots(profiles_dir: &Path, roots: &mut Vec<PathBuf>) -> bool {
    let Ok(entries) = std::fs::read_dir(profiles_dir) else {
        return false;
    };
    let mut profiles = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            profiles.push(entry.path());
        }
    }
    // Sort before capping so a full directory always keeps the same
    // profiles regardless of read_dir order.
    profiles.sort();
    let cap_hit = profiles.len() > 64;
    for profile in profiles.into_iter().take(64) {
        for layout in [
            profile.join("sessions"),
            profile.join("agent").join("sessions"),
        ] {
            if layout.is_dir() {
                roots.push(layout);
            }
        }
    }
    cap_hit
}

fn push_named_profile_roots(roots: &mut Vec<PathBuf>, config: &Path, xdg_base: &Path, name: &str) {
    let mut found = false;
    for profile_root in [
        config.join("profiles").join(name),
        xdg_base.join("omp").join("profiles").join(name),
    ] {
        for layout in [
            profile_root.join("sessions"),
            profile_root.join("agent").join("sessions"),
        ] {
            if layout.is_dir() {
                roots.push(layout);
                found = true;
            }
        }
    }
    if !found {
        roots.push(
            config
                .join("profiles")
                .join(name)
                .join("agent")
                .join("sessions"),
        );
    }
}

fn keep_pi_row(
    row: PiRow,
    seen_entries: &mut BTreeSet<(String, String)>,
    events: &mut Vec<UsageEvent>,
) {
    let fresh = match row.key {
        Some(key) => seen_entries.insert(key),
        None => true,
    };
    if fresh {
        events.push(row.event);
    }
}

fn push_swap_slots(sessions: &Path, roots: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(sessions) else {
        return;
    };
    let mut slots = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some((number, _rest)) = name.split_once('-') else {
            continue;
        };
        if number.parse::<u32>().is_ok_and(|n| n > 0) {
            slots.push(entry.path());
        }
    }
    slots.sort();
    for slot in slots {
        let projects = slot.join("projects");
        if projects.is_dir() {
            roots.push(projects);
        }
    }
}

fn dedup_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    roots
        .into_iter()
        .filter(|root| seen.insert(root.canonicalize().unwrap_or_else(|_| root.clone())))
        .collect()
}

fn absolute_or(xdg: Option<&Path>, default: &Path) -> PathBuf {
    match xdg {
        Some(dir) if dir.is_absolute() => dir.to_path_buf(),
        _ => default.to_path_buf(),
    }
}

fn trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn omp_config_root(config_dir: Option<&str>, home: &Path) -> Option<PathBuf> {
    let name = match trimmed(config_dir) {
        Some(name) if !name.starts_with('/') => name,
        Some(_) => return None,
        None => ".omp",
    };
    let root = home.join(name);
    match root.canonicalize() {
        // Symlinks resolve before the containment check, so a config dir
        // pointing outside HOME fails closed the way the contract does.
        Ok(resolved) => {
            let home = home.canonicalize().ok()?;
            resolved.starts_with(&home).then_some(resolved)
        }
        Err(_) => lexically_within(&root, home).then_some(root),
    }
}

fn lexically_within(child: &Path, root: &Path) -> bool {
    fn resolve(path: &Path) -> Vec<Component<'_>> {
        let mut stack = Vec::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    stack.pop();
                }
                other => stack.push(other),
            }
        }
        stack
    }
    let child = resolve(child);
    let root = resolve(root);
    child.starts_with(&root[..])
}

enum ProfileValue {
    Default,
    Named(String),
    Invalid,
}

fn normalize_profile(value: Option<&str>) -> ProfileValue {
    let Some(value) = trimmed(value) else {
        return ProfileValue::Default;
    };
    if value == "default" {
        return ProfileValue::Default;
    }
    let valid = value.chars().count() <= 64
        && value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && value
            .chars()
            .skip(1)
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !matches!(value, "." | "..")
        && !value.ends_with('.');
    if valid {
        ProfileValue::Named(value.to_owned())
    } else {
        ProfileValue::Invalid
    }
}

fn identifier(text: &str) -> Option<&str> {
    (!text.is_empty() && text.len() <= MAX_IDENTIFIER_BYTES).then_some(text)
}

fn parse_object(bytes: &[u8]) -> Option<Value> {
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    value.is_object().then_some(value)
}

fn text_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(key))
        .and_then(|found| found.as_str())
        .map(str::trim)
        .and_then(identifier)
}

/// One consumed counter: absent reads as 0 (the harness omits it), a
/// present-but-unreadable value makes the whole payload unreadable.
fn strict_counter(usage: &Value, key: &str) -> Option<u64> {
    match chain(usage, &[key]) {
        Chain::Value(n) => Some(n),
        Chain::Absent => Some(0),
        Chain::Invalid => None,
    }
}

enum Chain {
    Absent,
    Invalid,
    Value(u64),
}

fn chain(usage: &Value, keys: &[&str]) -> Chain {
    for key in keys {
        let Some(found) = usage.get(key) else {
            continue;
        };
        return match found.as_u64() {
            Some(n) => Chain::Value(n),
            None => Chain::Invalid,
        };
    }
    Chain::Absent
}

fn rfc3339(text: &str) -> Option<Timestamp> {
    text.parse::<Timestamp>().ok()
}

fn epoch_timestamp(value: Option<&Value>) -> Option<Timestamp> {
    match value? {
        Value::Number(number) => epoch_from_int(number.as_i64()?),
        Value::String(text) => match text.trim().parse::<i64>() {
            Ok(epoch) => epoch_from_int(epoch),
            Err(_) => rfc3339(text.trim()),
        },
        _ => None,
    }
}

/// Pi records epoch seconds or milliseconds; the split mirrors `CodexBar`'s
/// `1_000_000_000_000` heuristic.
fn epoch_from_int(epoch: i64) -> Option<Timestamp> {
    if epoch > 1_000_000_000_000 {
        Timestamp::from_millisecond(epoch).ok()
    } else {
        Timestamp::from_second(epoch).ok()
    }
}

enum LineOutcome<Row> {
    Parsed,
    Row(Row),
    RowWithGap(Row),
    Skipped,
}

struct ClaudeRow {
    event: UsageEvent,
    key: Option<ClaudeKey>,
    estimate_only: bool,
}

struct KeptClaude {
    event: UsageEvent,
    estimate_only: bool,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum ClaudeKey {
    Request { message: String, request: String },
    Session { message: String, session: String },
}

fn claude_session_id<'a>(line: &'a Value, message: &'a Value) -> Option<&'a str> {
    for source in [line, message] {
        let found = source
            .get("sessionId")
            .or_else(|| source.get("session_id"))
            .or_else(|| source.get("metadata").and_then(|m| m.get("sessionId")))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty());
        if let Some(found) = found.and_then(identifier) {
            return Some(found);
        }
    }
    None
}

fn claude_line(bytes: &[u8]) -> LineOutcome<ClaudeRow> {
    let Some(line) = parse_object(bytes) else {
        return LineOutcome::Skipped;
    };
    let Some(kind) = line.get("type").and_then(Value::as_str) else {
        return LineOutcome::Skipped;
    };
    if kind != "assistant" {
        return LineOutcome::Parsed;
    }
    let Some(timestamp) = line
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(rfc3339)
    else {
        return LineOutcome::Skipped;
    };
    let Some(message) = line.get("message").filter(|m| m.is_object()) else {
        return LineOutcome::Skipped;
    };
    let Some(model) = text_field(message, &["model"]) else {
        return LineOutcome::Skipped;
    };
    let Some(usage) = message.get("usage").filter(|u| u.is_object()) else {
        return LineOutcome::Skipped;
    };
    let Some(input_tokens) = strict_counter(usage, "input_tokens") else {
        return LineOutcome::Skipped;
    };
    let Some(cache_read_tokens) = strict_counter(usage, "cache_read_input_tokens") else {
        return LineOutcome::Skipped;
    };
    let Some(cache_write_tokens) = strict_counter(usage, "cache_creation_input_tokens") else {
        return LineOutcome::Skipped;
    };
    let Some(output_tokens) = strict_counter(usage, "output_tokens") else {
        return LineOutcome::Skipped;
    };
    if input_tokens == 0 && cache_read_tokens == 0 && cache_write_tokens == 0 && output_tokens == 0
    {
        return LineOutcome::Parsed;
    }
    let cache_counters_absent = usage.as_object().is_some_and(|usage| {
        !usage.contains_key("cache_read_input_tokens")
            && !usage.contains_key("cache_creation_input_tokens")
    });
    // A proxy-era estimate carries input only and never a stop reason;
    // its tokens stay but the scan must stay a flagged floor.
    let estimate_only = message.get("stop_reason") == Some(&Value::Null)
        && input_tokens > 0
        && output_tokens == 0
        && cache_counters_absent;
    let event = UsageEvent {
        timestamp,
        provider: ANTHROPIC.to_owned(),
        model: model.to_owned(),
        input_tokens,
        output_tokens,
        cache_read_tokens: Some(cache_read_tokens),
        cache_write_tokens: Some(cache_write_tokens),
    };
    let key = message
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .and_then(identifier)
        .and_then(|message_id| {
            let message_id = message_id.to_owned();
            let request = line
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|request| !request.is_empty())
                .and_then(identifier);
            match request {
                Some(request) => Some(ClaudeKey::Request {
                    message: message_id,
                    request: request.to_owned(),
                }),
                None => claude_session_id(&line, message).map(|session| ClaudeKey::Session {
                    message: message_id,
                    session: session.to_owned(),
                }),
            }
        });
    let row = ClaudeRow {
        event,
        key,
        estimate_only,
    };
    if estimate_only {
        LineOutcome::RowWithGap(row)
    } else {
        LineOutcome::Row(row)
    }
}

fn event_total(event: &UsageEvent) -> u128 {
    u128::from(event.input_tokens)
        + u128::from(event.output_tokens)
        + event.cache_read_tokens.map_or(0, u128::from)
        + event.cache_write_tokens.map_or(0, u128::from)
}

fn keep_claude_row(
    keyed: &mut BTreeMap<ClaudeKey, KeptClaude>,
    unkeyed: &mut Vec<UsageEvent>,
    row: ClaudeRow,
) {
    let ClaudeRow {
        key,
        event,
        estimate_only,
    } = row;
    let Some(key) = key else {
        unkeyed.push(event);
        return;
    };
    let keep = keyed.get(&key).is_none_or(|kept| {
        (!estimate_only && kept.estimate_only)
            || (estimate_only == kept.estimate_only
                && event_total(&kept.event) < event_total(&event))
    });
    if keep {
        keyed.insert(
            key,
            KeptClaude {
                event,
                estimate_only,
            },
        );
    }
}

/// The two cached-read spellings name one counter: the larger valid value
/// wins, and any present-but-unreadable spelling spoils the payload.
fn cached_counter(usage: &Value) -> Option<u64> {
    let mut best = 0;
    for key in ["cached_input_tokens", "cache_read_input_tokens"] {
        match chain(usage, &[key]) {
            Chain::Value(n) => best = best.max(n),
            Chain::Absent => {}
            Chain::Invalid => return None,
        }
    }
    Some(best)
}

#[derive(Clone, Copy, Default)]
struct CodexTotals {
    input: u64,
    cache_read: u64,
    output: u64,
}

impl CodexTotals {
    fn read(usage: &Value) -> Option<Self> {
        Some(Self {
            input: strict_counter(usage, "input_tokens")?,
            cache_read: cached_counter(usage)?,
            output: strict_counter(usage, "output_tokens")?,
        })
    }

    fn is_zero(self) -> bool {
        self.input == 0 && self.cache_read == 0 && self.output == 0
    }
}

#[derive(Default)]
struct CodexFile {
    model: Option<String>,
    watermark: Option<CodexTotals>,
}

/// A regressing total is a stale replay; the watermark holds the high
/// point so a replay or a mirrored ledger record re-emits nothing.
fn advance(watermark: &mut Option<CodexTotals>, total: CodexTotals) -> CodexTotals {
    let Some(prev) = *watermark else {
        *watermark = Some(total);
        return total;
    };
    *watermark = Some(CodexTotals {
        input: prev.input.max(total.input),
        cache_read: prev.cache_read.max(total.cache_read),
        output: prev.output.max(total.output),
    });
    CodexTotals {
        input: total.input.saturating_sub(prev.input),
        cache_read: total.cache_read.saturating_sub(prev.cache_read),
        output: total.output.saturating_sub(prev.output),
    }
}

fn codex_row(timestamp: Timestamp, model: &str, tokens: CodexTotals) -> LineOutcome<UsageEvent> {
    if tokens.is_zero() {
        return LineOutcome::Parsed;
    }
    LineOutcome::Row(UsageEvent {
        timestamp,
        provider: OPENAI.to_owned(),
        model: model.to_owned(),
        input_tokens: tokens.input.saturating_sub(tokens.cache_read),
        output_tokens: tokens.output,
        cache_read_tokens: Some(tokens.cache_read),
        cache_write_tokens: None,
    })
}

fn codex_line(bytes: &[u8], file: &mut CodexFile) -> LineOutcome<UsageEvent> {
    let Some(line) = parse_object(bytes) else {
        return LineOutcome::Skipped;
    };
    let Some(kind) = line.get("type").and_then(Value::as_str) else {
        return LineOutcome::Skipped;
    };
    let payload = line.get("payload");
    match kind {
        "token_usage_record" => {
            let Some(payload) = payload.filter(|p| p.is_object()) else {
                return LineOutcome::Skipped;
            };
            let Some(timestamp) = line
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(rfc3339)
            else {
                return LineOutcome::Skipped;
            };
            let model = text_field(payload, &["model", "model_name"])
                .or(text_field(&line, &["model", "model_name"]))
                .or(file.model.as_deref());
            let Some(model) = model else {
                return LineOutcome::Skipped;
            };
            let Some(total) = payload
                .get("thread_token_usage")
                .filter(|t| t.is_object())
                .and_then(CodexTotals::read)
            else {
                return LineOutcome::Skipped;
            };
            codex_row(timestamp, model, advance(&mut file.watermark, total))
        }
        "turn_context" => {
            let model = payload
                .and_then(|payload| text_field(payload, &["model", "model_name"]))
                .or(payload
                    .and_then(|payload| payload.get("info"))
                    .and_then(|info| text_field(info, &["model", "model_name"])));
            if let Some(model) = model {
                file.model = Some(model.to_owned());
            }
            LineOutcome::Parsed
        }
        "event_msg" => {
            let Some(payload) = payload.filter(|p| p.is_object()) else {
                return LineOutcome::Skipped;
            };
            let Some(payload_type) = payload.get("type").and_then(Value::as_str) else {
                return LineOutcome::Skipped;
            };
            if payload_type != "token_count" {
                return LineOutcome::Parsed;
            }
            let Some(timestamp) = line
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(rfc3339)
            else {
                return LineOutcome::Skipped;
            };
            let Some(info) = payload.get("info").filter(|i| i.is_object()) else {
                return LineOutcome::Skipped;
            };
            let model = text_field(info, &["model", "model_name"])
                .or(text_field(payload, &["model", "model_name"]))
                .or(text_field(&line, &["model", "model_name"]))
                .or(file.model.as_deref());
            let Some(model) = model else {
                return LineOutcome::Skipped;
            };
            let total = match info.get("total_token_usage").filter(|t| t.is_object()) {
                Some(raw) => match CodexTotals::read(raw) {
                    Some(total) => Some(total),
                    None => return LineOutcome::Skipped,
                },
                None => None,
            };
            let tokens = match total {
                Some(total) => advance(&mut file.watermark, total),
                None => match info
                    .get("last_token_usage")
                    .filter(|l| l.is_object())
                    .and_then(CodexTotals::read)
                {
                    Some(last) => last,
                    None => return LineOutcome::Skipped,
                },
            };
            codex_row(timestamp, model, tokens)
        }
        _ => LineOutcome::Parsed,
    }
}

enum PiContext {
    None,
    Unsupported,
    Model {
        provider: &'static str,
        model: String,
    },
}

struct PiFile {
    session: Option<String>,
    context: PiContext,
}

impl Default for PiFile {
    fn default() -> Self {
        Self {
            session: None,
            context: PiContext::None,
        }
    }
}

struct PiRow {
    event: UsageEvent,
    key: Option<(String, String)>,
}

fn pi_provider(text: &str) -> Option<&'static str> {
    match text.trim().to_ascii_lowercase().as_str() {
        "openai-codex" => Some(OPENAI),
        "anthropic" => Some(ANTHROPIC),
        "amazon-bedrock" => Some(AMAZON_BEDROCK),
        _ => None,
    }
}

fn pi_line(bytes: &[u8], file: &mut PiFile) -> LineOutcome<PiRow> {
    let Some(line) = parse_object(bytes) else {
        return LineOutcome::Skipped;
    };
    let Some(kind) = line.get("type").and_then(Value::as_str) else {
        return LineOutcome::Skipped;
    };
    match kind {
        "session" => {
            if file.session.is_none() {
                file.session =
                    text_field(&line, &["id", "sessionId", "session_id"]).map(str::to_owned);
            }
            LineOutcome::Parsed
        }
        "model_change" => {
            file.context = match line
                .get("provider")
                .and_then(Value::as_str)
                .and_then(pi_provider)
            {
                Some(provider) => match text_field(&line, &["modelId", "model"]) {
                    Some(model) => PiContext::Model {
                        provider,
                        model: model.to_owned(),
                    },
                    None => PiContext::None,
                },
                None => PiContext::Unsupported,
            };
            LineOutcome::Parsed
        }
        "message" => {
            let Some(message) = line.get("message").filter(|m| m.is_object()) else {
                return LineOutcome::Parsed;
            };
            if message.get("role").and_then(Value::as_str) != Some("assistant") {
                return LineOutcome::Parsed;
            }
            pi_assistant(&line, message, file)
        }
        _ => LineOutcome::Parsed,
    }
}

fn pi_assistant(line: &Value, message: &Value, file: &PiFile) -> LineOutcome<PiRow> {
    let context = match &file.context {
        PiContext::Model { provider, model } => Some((*provider, model.as_str())),
        PiContext::None | PiContext::Unsupported => None,
    };
    // A present-but-unmapped provider is an unsupported backend; it must
    // not quietly borrow another backend's pricing.
    let provider = match text_field(message, &["provider"]).or(text_field(line, &["provider"])) {
        Some(text) => match pi_provider(text) {
            Some(provider) => Some(provider),
            None => return LineOutcome::Skipped,
        },
        None => context.map(|(provider, _)| provider),
    };
    let model = text_field(message, &["model"])
        .or(text_field(line, &["model"]))
        .or(text_field(message, &["modelId"]))
        .or(text_field(line, &["modelId"]))
        .or(context.map(|(_, model)| model));
    let (Some(provider), Some(model)) = (provider, model) else {
        return LineOutcome::Skipped;
    };
    let timestamp =
        epoch_timestamp(message.get("timestamp")).or(epoch_timestamp(line.get("timestamp")));
    let Some(timestamp) = timestamp else {
        return LineOutcome::Skipped;
    };
    let Some(usage) = message.get("usage").filter(|u| u.is_object()) else {
        return LineOutcome::Skipped;
    };
    let mut has_counter = false;
    let mut read = |keys: &[&str]| -> Option<u64> {
        match chain(usage, keys) {
            Chain::Value(n) => {
                has_counter = true;
                Some(n)
            }
            Chain::Invalid => None,
            Chain::Absent => Some(0),
        }
    };
    let (Some(input), Some(cache_read), Some(cache_write), Some(output)) = (
        read(PI_INPUT_KEYS),
        read(PI_CACHE_READ_KEYS),
        read(PI_CACHE_WRITE_KEYS),
        read(PI_OUTPUT_KEYS),
    ) else {
        return LineOutcome::Skipped;
    };
    if !has_counter {
        return LineOutcome::Skipped;
    }
    match one_hour_chain(usage) {
        Chain::Invalid => return LineOutcome::Skipped,
        Chain::Value(n) if n > cache_write => return LineOutcome::Skipped,
        Chain::Value(_) | Chain::Absent => {}
    }
    // No openai cache-write tariff exists here, so codex-backend writes fold
    // into input at the input rate; CodexBar's own 1.25x write tariff runs higher.
    let (input_tokens, cache_write_tokens) = if provider == OPENAI {
        (input.saturating_add(cache_write), None)
    } else {
        (input, Some(cache_write))
    };
    let event = UsageEvent {
        timestamp,
        provider: provider.to_owned(),
        model: model.to_owned(),
        input_tokens,
        output_tokens: output,
        cache_read_tokens: Some(cache_read),
        cache_write_tokens,
    };
    let entry = text_field(line, &["id"]);
    let key = match (&file.session, entry) {
        (Some(session), Some(entry)) => Some((session.clone(), entry.to_owned())),
        _ => None,
    };
    LineOutcome::Row(PiRow { event, key })
}

fn one_hour_chain(usage: &Value) -> Chain {
    match chain(usage, &["cacheWrite1h", "cache_write_1h"]) {
        Chain::Absent => match usage.get("cttl").filter(|c| c.is_object()) {
            Some(cttl) => chain(cttl, &["ephemeral1h", "ephemeral_1h"]),
            None => Chain::Absent,
        },
        found => found,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_accept_documented_shapes_only() {
        assert!(matches!(normalize_profile(None), ProfileValue::Default));
        assert!(matches!(
            normalize_profile(Some("  ")),
            ProfileValue::Default
        ));
        assert!(matches!(
            normalize_profile(Some("default")),
            ProfileValue::Default
        ));
        assert!(matches!(
            normalize_profile(Some("work")),
            ProfileValue::Named(name) if name == "work"
        ));
        assert!(matches!(
            normalize_profile(Some("team.2026_v2")),
            ProfileValue::Named(_)
        ));
        for invalid in [
            "../escape",
            ".",
            "..",
            "a.",
            "/abs",
            "has space",
            &"x".repeat(65),
        ] {
            assert!(
                matches!(normalize_profile(Some(invalid)), ProfileValue::Invalid),
                "`{invalid}` should be refused"
            );
        }
    }

    #[test]
    fn epoch_timestamps_split_seconds_from_milliseconds() {
        let seconds = epoch_timestamp(Some(&serde_json::json!(1_760_000_000)))
            .unwrap_or_else(|| panic!("seconds rejected"));
        let millis = epoch_timestamp(Some(&serde_json::json!(1_760_000_000_000_i64)))
            .unwrap_or_else(|| panic!("millis rejected"));
        assert_eq!(seconds, millis);
        let text = epoch_timestamp(Some(&serde_json::json!("1760000000")))
            .unwrap_or_else(|| panic!("numeric string rejected"));
        assert_eq!(text, seconds);
    }

    #[test]
    fn iso_timestamps_parse_and_non_numeric_shapes_do_not() {
        assert!(epoch_timestamp(Some(&serde_json::json!("2026-10-13T10:00:00Z"))).is_some());
        assert!(epoch_timestamp(Some(&serde_json::json!(true))).is_none());
        assert!(epoch_timestamp(Some(&serde_json::json!("1e999"))).is_none());
        assert!(epoch_timestamp(Some(&serde_json::json!(1.5))).is_none());
    }

    #[test]
    fn pi_provider_mapping_covers_the_three_documented_backends() {
        assert_eq!(pi_provider("OpenAI-Codex"), Some(OPENAI));
        assert_eq!(pi_provider("anthropic"), Some(ANTHROPIC));
        assert_eq!(pi_provider("amazon-bedrock"), Some(AMAZON_BEDROCK));
        assert_eq!(pi_provider("google-gemini"), None);
    }
}
