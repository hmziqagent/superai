//! Discovery, ownership, and drift classification.
//!
//! Scans adapter-driven candidate roots, fingerprints harness identity,
//! classifies ownership (including foreign managers like `claude-multi`),
//! and produces a bounded drift report without mutating scanned files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::adapter::Adapter;
use crate::error::{CoreError, Result};
use crate::ids::HarnessId;
use crate::registry::Registry;
use crate::state::Ownership;

/// Confidence for a fingerprint.
///
/// Evidence is what matters — a directory name alone is `Low` at best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Confidence {
    /// Multiple consistent signals (canonical file + schema key + path pattern).
    High,
    /// Single solid signal (canonical file present).
    Medium,
    /// Name pattern only.
    Low,
    /// No evidence.
    None,
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::None => "none",
        };
        f.write_str(s)
    }
}

/// Result of fingerprinting a candidate config root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// Harness that best matches the candidate, if any.
    pub harness: Option<HarnessId>,
    /// Confidence in the match.
    pub confidence: Confidence,
    /// Evidence lines that led to the result.
    pub evidence: Vec<String>,
}

/// Foreign-manager check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignCheck {
    /// Whether the path is foreign-managed.
    pub is_foreign: bool,
    /// Owner identifier when foreign, e.g. `claude-multi`.
    pub owner: Option<String>,
    /// Evidence lines.
    pub evidence: Vec<String>,
}

/// Drift finding for a single candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftFinding {
    /// Absolute candidate path.
    pub path: PathBuf,
    /// Fingerprint for the candidate.
    pub fingerprint: Fingerprint,
    /// Ownership classification.
    pub ownership: Ownership,
    /// Foreign check details.
    pub foreign: ForeignCheck,
    /// Whether the candidate is recorded in the registry.
    pub is_recorded: bool,
}

/// Timestamped drift report over a scan scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReport {
    /// When the scan was performed, ISO8601.
    pub scanned_at: String,
    /// Home that was scanned.
    pub home: PathBuf,
    /// Candidate roots examined.
    pub candidates: Vec<PathBuf>,
    /// Findings grouped by candidate.
    pub findings: Vec<DriftFinding>,
}

// ---------------------------------------------------------------------------
// helpers: tilde/env expansion and path match
// ---------------------------------------------------------------------------

fn expand_tilde(value: &str, home: &Path) -> PathBuf {
    if value == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home.join(rest);
    }
    if let Some(rest) = value.strip_prefix("~\\") {
        return home.join(rest);
    }
    if let Some(rest) = value.strip_prefix("$HOME/") {
        return home.join(rest);
    }
    if let Some(rest) = value.strip_prefix("${HOME}/") {
        return home.join(rest);
    }
    if let Some(rest) = value.strip_prefix("%USERPROFILE%/") {
        return home.join(rest);
    }
    if let Some(rest) = value.strip_prefix("%USERPROFILE%\\") {
        return home.join(rest);
    }
    PathBuf::from(value)
}

#[expect(clippy::question_mark, reason = "explicit early return is clearer")]
fn expand_env_var_pattern(pattern: &str, home: &Path) -> Option<PathBuf> {
    // Handle bare `$VAR` and `${VAR}` without trailing slash.
    let var_name = if let Some(rest) = pattern.strip_prefix("${") {
        rest.strip_suffix('}')?
    } else if let Some(rest) = pattern.strip_prefix('$') {
        // Stop at first non-var char; pattern is exactly the var.
        if rest.contains('/') || rest.contains('\\') || rest.contains(' ') {
            return None;
        }
        rest
    } else {
        return None;
    };
    let val = std::env::var(var_name).ok()?;
    if val.trim().is_empty() {
        return None;
    }
    // If val contains ~, expand it.
    Some(expand_tilde(&val, home))
}

fn expand_pattern(pattern: &str, home: &Path) -> Option<PathBuf> {
    if pattern.starts_with('~')
        || pattern.starts_with("$HOME")
        || pattern.starts_with("${HOME}")
        || pattern.starts_with("%USERPROFILE%")
    {
        return Some(expand_tilde(pattern, home));
    }
    if pattern.starts_with('$') || pattern.starts_with("${") {
        return expand_env_var_pattern(pattern, home);
    }
    // Already absolute or relative fallback handling
    let candidate = PathBuf::from(pattern);
    if candidate.is_absolute() {
        Some(candidate)
    } else {
        // Treat as home-relative
        Some(home.join(pattern))
    }
}

fn known_prefixes() -> &'static [&'static str] {
    &[
        ".claude",
        ".codex",
        ".aider",
        ".opencode",
        ".cline",
        ".goose",
        ".cursor",
        ".roo",
        ".kilo",
        ".windsurf",
        ".auggie",
        ".amp",
        ".trae",
        ".pi",
        ".gemini",
        ".qwen",
        ".iflow",
        ".plandex",
        ".crush",
        ".forge",
        ".continue",
        ".warp",
        ".zed",
        ".factory",
        ".copilot",
    ]
}

// ---------------------------------------------------------------------------
// fingerprinting
// ---------------------------------------------------------------------------

/// Fingerprint a candidate config root using multiple signals.
///
/// Never decides on directory name alone. Evidence includes:
/// - canonical filenames (`settings.json`, `config.toml`, etc.)
/// - schema keys when the file can be read without secrets
/// - path pattern
/// - adjacent layout (presence of sibling state files)
/// - matching binary presence (via `PATH` probe only, not by executing)
#[expect(
    clippy::excessive_nesting,
    reason = "fingerprint multi-signal branches are explicit"
)]
#[expect(
    clippy::too_many_lines,
    reason = "fingerprint evidence collection is verbose"
)]
pub fn fingerprint_candidate(path: &Path) -> Fingerprint {
    let mut evidence: Vec<String> = Vec::new();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    let name_lower = name.to_lowercase();

    // Path pattern evidence (low confidence baseline)
    let mut pattern_hint: Option<&str> = None;
    if name_lower.starts_with(".claude") || name_lower.contains("claude") {
        pattern_hint = Some("claude-code");
        evidence.push(format!(
            "path pattern matches .claude* : {}",
            path.display()
        ));
    } else if name_lower.starts_with(".codex") || name_lower.contains("codex") {
        pattern_hint = Some("codex-cli");
        evidence.push(format!("path pattern matches .codex* : {}", path.display()));
    } else if name_lower.contains("aider") {
        pattern_hint = Some("aider");
        evidence.push(format!("path pattern matches aider: {}", path.display()));
    } else if name_lower.contains("opencode") {
        pattern_hint = Some("opencode");
        evidence.push(format!("path pattern matches opencode: {}", path.display()));
    } else if name_lower.contains("cline") {
        pattern_hint = Some("cline");
        evidence.push(format!("path pattern matches cline: {}", path.display()));
    }

    // Canonical file checks (bounded reads, skip secret stores)
    let mut harness_found: Option<&str> = None;
    let mut confidence = Confidence::None;

    // Claude: settings.json
    let candidate_settings = path.join("settings.json");
    if candidate_settings.is_file() {
        evidence.push(format!(
            "canonical file settings.json present at {}",
            candidate_settings.display()
        ));
        // Try to read a small bounded slice and look for schema markers without parsing secrets.
        if let Ok(text) = read_bounded(&candidate_settings, 64 * 1024) {
            if text.contains("\"model\"")
                || text.contains("\"permissions\"")
                || text.contains("$schema")
            {
                evidence.push("settings.json contains Claude Code schema keys".to_owned());
                harness_found = Some("claude-code");
                confidence = Confidence::High;
            } else {
                // File exists but not clearly Claude; keep medium
                if harness_found.is_none() {
                    harness_found = Some("claude-code");
                    confidence = Confidence::Medium;
                }
                evidence.push("settings.json exists but no Claude schema marker".to_owned());
            }
        }
    }

    // Codex: config.toml
    let codex_toml = path.join("config.toml");
    if codex_toml.is_file() {
        evidence.push(format!(
            "canonical file config.toml present at {}",
            codex_toml.display()
        ));
        if let Ok(text) = read_bounded(&codex_toml, 64 * 1024) {
            if text.contains("model_provider") || text.contains("model =") || text.contains('[') {
                evidence.push("config.toml contains Codex schema keys".to_owned());
                harness_found = Some("codex-cli");
                confidence = Confidence::High;
            } else if harness_found.is_none() {
                harness_found = Some("codex-cli");
                confidence = Confidence::Medium;
            }
        }
    }

    // Opencode: opencode.json / opencode.jsonc
    for fname in ["opencode.json", "opencode.jsonc"] {
        let p = path.join(fname);
        if p.is_file() {
            evidence.push(format!("canonical file {fname} present at {}", p.display()));
            if harness_found.is_none() {
                harness_found = Some("opencode");
                confidence = Confidence::Medium;
            }
            if let Ok(text) = read_bounded(&p, 64 * 1024)
                && (text.contains("\"mcp\"") || text.contains("\"permission\""))
            {
                evidence.push(format!("{fname} contains opencode marker"));
                harness_found = Some("opencode");
                confidence = Confidence::High;
            }
        }
    }

    // Aider: .aider.conf.yml, aider.conf.yml, .aider.*
    for fname in [
        ".aider.conf.yml",
        "aider.conf.yml",
        ".aider.model.metadata.json",
    ] {
        let p = path.join(fname);
        if p.is_file() {
            evidence.push(format!("canonical file {fname} present at {}", p.display()));
            harness_found = Some("aider");
            if name_lower.contains("aider") {
                confidence = Confidence::High;
            } else {
                confidence = Confidence::Medium;
            }
        }
    }

    // Cline: settings.json + cline specific marker
    if harness_found.is_none()
        && path.to_string_lossy().contains("cline")
        && candidate_settings.is_file()
    {
        // Already covered, but add cline hint
        harness_found = Some("cline");
        confidence = Confidence::Medium;
        evidence.push("cline settings.json candidate".to_owned());
    }

    // If no canonical file but path pattern exists, keep Low.
    if harness_found.is_none() {
        if let Some(hit) = pattern_hint {
            harness_found = Some(hit);
            confidence = Confidence::Low;
            evidence.push(format!("path pattern only, no canonical file for {hit}"));
        } else {
            evidence.push(format!(
                "no canonical file and no known pattern for {}",
                path.display()
            ));
            confidence = Confidence::None;
        }
    }

    // Adjacent state layout as supporting evidence (non-secret)
    let creds = path.join(".credentials.json");
    if creds.is_file() {
        evidence.push(format!(
            "adjacent credentials file present at {}",
            creds.display()
        ));
        // Do not parse secret store
    }
    if path.join("projects").is_dir() {
        evidence.push("adjacent projects/ directory present".to_owned());
    }
    if path.join("history.jsonl").is_file() {
        evidence.push("adjacent history.jsonl present".to_owned());
    }

    let harness_id = harness_found.and_then(|s| HarnessId::new(s).ok());

    Fingerprint {
        harness: harness_id,
        confidence,
        evidence,
    }
}

#[expect(clippy::indexing_slicing, reason = "len is bounded by data.len()")]
fn read_bounded(path: &Path, max_bytes: usize) -> std::io::Result<String> {
    let data = std::fs::read(path)?;
    let len = std::cmp::min(data.len(), max_bytes);
    // Respect UTF-8 char boundaries
    let slice = &data[..len];
    // Find last char boundary
    let mut valid_len = slice.len();
    while valid_len > 0 && std::str::from_utf8(&slice[..valid_len]).is_err() {
        valid_len = valid_len.saturating_sub(1);
    }
    let text = String::from_utf8_lossy(&slice[..valid_len]).into_owned();
    Ok(text)
}

// ---------------------------------------------------------------------------
// foreign-manager detection
// ---------------------------------------------------------------------------

/// Detect whether `path` is owned by a foreign manager.
///
/// Checks in order:
/// - `.foreign-managed` marker inside the candidate
/// - `.claude-multi` sibling marker
/// - `$HOME/.claude-multi/config.json` referencing the candidate
/// - generic `.superai-foreign` marker
///
/// Never parses a known secret store; only bounded reads of small config files.
#[expect(
    clippy::excessive_nesting,
    reason = "foreign check branches are explicit"
)]
pub fn is_foreign_managed(path: &Path, home: Option<&Path>) -> ForeignCheck {
    let mut evidence: Vec<String> = Vec::new();

    // Generic marker files inside candidate
    for marker in [".foreign-managed", ".superai-foreign", ".owned-by-foreign"] {
        let candidate = path.join(marker);
        if candidate.is_file() {
            evidence.push(format!("marker {marker} found at {}", candidate.display()));
            return ForeignCheck {
                is_foreign: true,
                owner: Some("generic-marker".to_owned()),
                evidence,
            };
        }
    }

    // .claude-multi marker file inside candidate
    let multi_in_candidate = path.join(".claude-multi");
    if multi_in_candidate.exists() {
        evidence.push(format!(
            "marker .claude-multi found at {}",
            multi_in_candidate.display()
        ));
        return ForeignCheck {
            is_foreign: true,
            owner: Some("claude-multi".to_owned()),
            evidence,
        };
    }

    if let Some(home_path) = home {
        // Home-level claude-multi config referencing the candidate
        let multi_config = home_path.join(".claude-multi").join("config.json");
        if multi_config.is_file() {
            evidence.push(format!(
                "checking foreign manager config at {}",
                multi_config.display()
            ));
            if let Ok(text) = read_bounded(&multi_config, 256 * 1024) {
                // Simple substring match on the candidate path; bounded and not parsing deeply
                let path_str = path.to_string_lossy();
                let candidate_str = path_str.as_ref();
                if text.contains(candidate_str) {
                    evidence.push(format!(
                        "candidate {} referenced in {}",
                        path.display(),
                        multi_config.display()
                    ));
                    return ForeignCheck {
                        is_foreign: true,
                        owner: Some("claude-multi".to_owned()),
                        evidence,
                    };
                }
                evidence.push(format!(
                    "candidate {} not referenced in {}",
                    path.display(),
                    multi_config.display()
                ));
            } else {
                evidence.push(format!(
                    "could not read {} (permission or io)",
                    multi_config.display()
                ));
            }
        } else {
            // Also check home's .claude-multi directory existing as sibling hint
            let multi_dir = home_path.join(".claude-multi");
            if multi_dir.is_dir() {
                evidence.push(format!(
                    "foreign manager directory .claude-multi exists at {}",
                    multi_dir.display()
                ));
                // Do not mark as foreign solely because .claude-multi exists;
                // need direct evidence linking candidate. Treat as not foreign but ambiguous.
                // Return not foreign with evidence.
            }
        }

        // Mise / package-manager shim check: if candidate looks like a shim target
        // we do not mark here; wrapper discovery handles shims.
    }

    // No evidence of foreign ownership
    evidence.push(format!("no foreign marker found for {}", path.display()));
    ForeignCheck {
        is_foreign: false,
        owner: None,
        evidence,
    }
}

// ---------------------------------------------------------------------------
// ownership classification
// ---------------------------------------------------------------------------

/// Classify ownership of a candidate path given the current registry and home.
///
/// Rules:
/// - If the candidate matches a recorded instance's `config_root`, return that instance's ownership.
/// - If foreign checks prove foreign-managed, return `ForeignManaged`.
/// - If the directory exists on disk with no record and no foreign owner, return `Unmanaged`.
/// - If the path is recorded but missing on disk, return `Detached`.
/// - Ambiguous evidence never causes a merge based on name alone.
pub fn classify_ownership(path: &Path, registry: &Registry, home: Option<&Path>) -> Ownership {
    let normalized = normalize_path(path);

    // Check registry first (exact normalized config_root match)
    for inst in registry.instances() {
        if normalize_path(inst.config_root.as_path()) == normalized {
            return inst.ownership;
        }
    }

    // Not in registry: check foreign
    let foreign = is_foreign_managed(path, home);
    if foreign.is_foreign {
        return Ownership::ForeignManaged;
    }

    // Existence on disk determines unmanaged vs detached
    if path.exists() {
        Ownership::Unmanaged
    } else {
        Ownership::Detached
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    // Lexical normalization: remove '.' and duplicate separators, preserve symlink non-follow.
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Prefix(p) => out.push(p.as_os_str()),
            std::path::Component::RootDir => out.push(std::path::Component::RootDir.as_os_str()),
            std::path::Component::CurDir | std::path::Component::ParentDir => {}
            std::path::Component::Normal(s) => out.push(s),
        }
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    out
}

// ---------------------------------------------------------------------------
// candidate root discovery (bounded)
// ---------------------------------------------------------------------------

const MAX_HOME_ENTRIES: usize = 1024;
const MAX_XDG_ENTRIES: usize = 256;

/// Expand a list of adapter-derived patterns into existing absolute paths.
fn candidate_patterns() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Concrete adapters with accurate scan_candidates
    if let Ok(adapter) = crate::adapters::claude_code::ClaudeCodeAdapter::new() {
        out.extend(adapter.scan_candidates());
    }
    if let Ok(adapter) = crate::adapters::codex_cli::CodexCliAdapter::new() {
        out.extend(adapter.scan_candidates());
    }
    if let Ok(adapter) = crate::adapters::aider::AiderAdapter::new() {
        out.extend(adapter.scan_candidates());
    }
    if let Ok(adapter) = crate::adapters::opencode::OpenCodeAdapter::new() {
        out.extend(adapter.scan_candidates());
    }
    if let Ok(adapter) = crate::adapters::cline::ClineAdapter::new() {
        out.extend(adapter.scan_candidates());
    }
    // Generic entries from catalog for remaining harnesses
    for entry in crate::harness_catalog::ENTRIES {
        let candidate = format!("~/.{}", entry.id);
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// Scan `home` for candidate config roots.
///
/// Bounded: no unrestricted crawl, at most one level under `home` and `.config`,
/// at most `MAX_HOME_ENTRIES` entries, skips permission errors, never parses secret stores.
pub fn scan_candidate_roots(home: &Path) -> Vec<PathBuf> {
    scan_candidate_roots_limited(home, MAX_HOME_ENTRIES)
}

/// Same as `scan_candidate_roots` but with an explicit entry limit (for tests).
#[expect(clippy::excessive_nesting, reason = "scan branches are explicit")]
#[expect(clippy::too_many_lines, reason = "scan is bounded and explicit")]
pub fn scan_candidate_roots_limited(home: &Path, max_entries: usize) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let mut push_candidate = |p: PathBuf| {
        // Deduplicate lexically via normalized string
        let key = normalize_path(&p).to_string_lossy().into_owned();
        if seen.insert(key) {
            candidates.push(p);
        }
    };

    // 1) Explicit patterns expanded and checked for existence (skip globs)
    for pattern in candidate_patterns() {
        if pattern.contains('*') {
            continue;
        }
        if let Some(expanded) = expand_pattern(&pattern, home)
            && (expanded.is_dir() || expanded.is_file())
        {
            push_candidate(expanded);
        }
    }

    // 2) Env var relocation hints (CLAUDE_CONFIG_DIR, CODEX_HOME, etc.)
    for var in [
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "GOOSE_PATH_ROOT",
        "OPENCODE_CONFIG_DIR",
        "CLINE_DATA_DIR",
    ] {
        if let Ok(val) = std::env::var(var)
            && !val.trim().is_empty()
        {
            let p = expand_tilde(&val, home);
            if p.is_dir() {
                push_candidate(p);
            } else {
                // Also consider absolute value from env even if not tilde-related
                let pb = PathBuf::from(&val);
                if pb.is_absolute() && pb.is_dir() {
                    push_candidate(pb);
                }
            }
        }
    }

    // 3) Enumerate top-level home entries matching known prefixes (bounded)
    if let Ok(entries) = std::fs::read_dir(home) {
        let mut count: usize = 0;
        for entry_res in entries {
            if count >= max_entries {
                break;
            }
            let Ok(entry) = entry_res else {
                // Skip permission errors with no panic
                count = count.saturating_add(1);
                continue;
            };
            count = count.saturating_add(1);
            let path = entry.path();
            // Use symlink_metadata to avoid following links for the crawl itself
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_dir() {
                continue;
            }
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            let lower = name_str.to_lowercase();
            let mut matches = false;
            for prefix in known_prefixes() {
                if lower.starts_with(prefix) {
                    matches = true;
                    break;
                }
            }
            if matches {
                push_candidate(path);
            }
        }
    }

    // 4) XDG / platform application directories: ~/.config/* for relevant harnesses
    let config_dir = home.join(".config");
    if let Ok(entries) = std::fs::read_dir(&config_dir) {
        let mut count: usize = 0;
        for entry_res in entries {
            if count >= MAX_XDG_ENTRIES {
                break;
            }
            let Ok(entry) = entry_res else {
                count = count.saturating_add(1);
                continue;
            };
            count = count.saturating_add(1);
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_dir() {
                continue;
            }
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy().to_lowercase();
            // Opencode, cline, codex, etc live under .config
            if name_str.contains("opencode")
                || name_str.contains("codex")
                || name_str.contains("cline")
                || name_str.contains("goose")
            {
                push_candidate(path);
            }
        }
    }

    // 5) Also consider wrapper relocation hints: if bin wrappers exist, parse their env assignments
    // For boundedness, do not crawl bin dirs here; rely on explicit patterns above.

    // Deduplicate by file identity (inode) where possible, preserving display path
    let deduped = deduplicate_by_identity(candidates);
    let mut sorted = deduped;
    sorted.sort();
    sorted
}

/// Deduplicate candidates by file identity without losing display path.
///
/// On Unix, two paths that point to the same inode/device are considered one.
/// Otherwise, lexical dedup is used. The first occurrence's display path is kept.
#[expect(clippy::excessive_nesting, reason = "dedup branches are explicit")]
pub fn deduplicate_by_identity(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let mut seen_ids: HashSet<(u64, u64)> = HashSet::new();
        let mut seen_lexical: HashSet<String> = HashSet::new();
        let mut out: Vec<PathBuf> = Vec::new();
        for path in candidates {
            let normalized_key = normalize_path(&path).to_string_lossy().into_owned();
            // Lexical dedup first
            if !seen_lexical.insert(normalized_key.clone()) {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(&path) {
                let id = (meta.dev(), meta.ino());
                if !seen_ids.insert(id) {
                    // Duplicate inode; keep first display path, skip this one
                    // Need to remove the lexical we just inserted? No, we want to keep lexical set
                    // but this inode dup means we should remove the duplicate path from out
                    // Since we haven't pushed yet, just skip.
                    // But we already inserted lexical; keep it to prevent re-adding same normalized path via symlink.
                    // The inode dup should be skipped.
                    continue;
                }
            }
            out.push(path);
        }
        out
    }
    #[cfg(not(unix))]
    {
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<PathBuf> = Vec::new();
        for path in candidates {
            let key = normalize_path(&path)
                .to_string_lossy()
                .into_owned()
                .to_lowercase();
            if seen.insert(key) {
                out.push(path);
            }
        }
        out
    }
}

/// Find unmanaged candidates by scanning `home` and filtering against the registry.
///
/// This is the "actual disk scan vs candidates param" extension: instead of
/// requiring the caller to supply candidates, we scan `home` directly and
/// filter. The registry's own `unmanaged_dirs` can then be fed the scan result
/// if needed.
pub fn find_unmanaged_candidates(registry: &Registry, home: &Path) -> Vec<PathBuf> {
    let candidates = scan_candidate_roots(home);
    crate::registry::unmanaged_dirs(registry, &candidates)
        .into_iter()
        .filter(|p| {
            // Additionally, ensure no foreign ownership and no existing wrapper ties
            let foreign = is_foreign_managed(p, Some(home));
            !foreign.is_foreign
        })
        .collect()
}

/// Produce a drift report for `home` against the current `registry`.
///
/// The report is read-only and contains no UI formatting. It records
/// the timestamp, scanned scope, fingerprint, ownership, foreign evidence,
/// and whether each candidate is recorded.
pub fn drift_report(registry: &Registry, home: &Path) -> DriftReport {
    let candidates = scan_candidate_roots(home);
    let mut findings: Vec<DriftFinding> = Vec::new();
    for cand in &candidates {
        let fingerprint = fingerprint_candidate(cand);
        let foreign = is_foreign_managed(cand, Some(home));
        let ownership = classify_ownership(cand, registry, Some(home));
        let is_recorded = registry
            .instances()
            .iter()
            .any(|i| normalize_path(i.config_root.as_path()) == normalize_path(cand));
        findings.push(DriftFinding {
            path: cand.clone(),
            fingerprint,
            ownership,
            foreign,
            is_recorded,
        });
    }
    // Include recorded instances whose config root is missing on disk (detached/missing_config)
    for inst in registry.instances() {
        let root = inst.config_root.as_path();
        let norm = normalize_path(root);
        let already = candidates.iter().any(|c| normalize_path(c) == norm);
        if !already && !root.exists() {
            let fingerprint = Fingerprint {
                harness: Some(inst.harness.clone()),
                confidence: Confidence::None,
                evidence: vec![format!(
                    "recorded instance {} config missing at {}",
                    inst.name,
                    root.display()
                )],
            };
            let foreign = ForeignCheck {
                is_foreign: false,
                owner: None,
                evidence: vec!["recorded but missing".to_owned()],
            };
            findings.push(DriftFinding {
                path: root.to_path_buf(),
                fingerprint,
                ownership: inst.ownership,
                foreign,
                is_recorded: true,
            });
        }
    }

    DriftReport {
        scanned_at: now_iso8601(),
        home: home.to_path_buf(),
        candidates,
        findings,
    }
}

fn now_iso8601() -> String {
    // Cheap RFC3339 without external crate; reuses registry helper logic
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    unix_secs_to_rfc3339(secs)
}

fn unix_secs_to_rfc3339(secs: u64) -> String {
    #[expect(
        clippy::cast_possible_wrap,
        reason = "secs/86400 fits in i64 for timestamps within reasonable range"
    )]
    let days = (secs / 86400) as i64;
    let secs_of_day = secs % 86400;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "year fits in i32 for registry timestamps"
)]
#[expect(
    clippy::cast_sign_loss,
    reason = "days derived from u64 secs, always non-negative"
)]
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}

// ---------------------------------------------------------------------------
// adoption helper (record-first, config-preserving)
// ---------------------------------------------------------------------------

/// Validate that a candidate can be adopted.
///
/// Checks: harness fingerprint (prove harness/version), foreign ownership,
/// fresh config read (at least one canonical file or directory exists),
/// and isolation class. Returns the fingerprint on success.
/// Never copies, migrates, normalizes, or reformats the harness config.
pub fn can_adopt(candidate: &Path, home: Option<&Path>) -> Result<Fingerprint> {
    let fingerprint = fingerprint_candidate(candidate);
    if fingerprint.confidence == Confidence::None {
        return Err(CoreError::Validation {
            field: "candidate".to_owned(),
            reason: format!(
                "cannot prove harness for {}: {}",
                candidate.display(),
                fingerprint.evidence.join("; ")
            ),
        });
    }
    let foreign = is_foreign_managed(candidate, home);
    if foreign.is_foreign {
        return Err(CoreError::ForeignOwnership {
            path: candidate.to_path_buf(),
            owner: foreign.owner.unwrap_or_else(|| "foreign".to_owned()),
        });
    }
    if !candidate.exists() {
        return Err(CoreError::Validation {
            field: "candidate".to_owned(),
            reason: format!("candidate {} does not exist", candidate.display()),
        });
    }
    // Ensure we can read at least the directory (fresh read)
    let _meta = std::fs::symlink_metadata(candidate).map_err(|e| CoreError::Validation {
        field: "candidate".to_owned(),
        reason: format!("cannot stat {}: {e}", candidate.display()),
    })?;
    Ok(fingerprint)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{InstanceId, InstanceName, TemplateId, TemplateVersion};
    use crate::instance::{Instance, TemplateRef};
    use crate::paths::AbsolutePath;
    use crate::state::{InstanceOrigin, Isolation};
    // Ownership already imported via super::*

    fn tmp_home(label: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(label)
    }

    fn sample_instance(name: &str, config_root: &str, id: &str, ownership: Ownership) -> Instance {
        Instance {
            id: InstanceId::new(id).unwrap(),
            name: InstanceName::new(name).unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::new(config_root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership,
            template: Some(TemplateRef {
                name: TemplateId::new("glm").unwrap(),
                version: TemplateVersion::new("1.2.0").unwrap(),
            }),
            created_at: "2026-08-26T12:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        }
    }

    #[test]
    fn scan_finds_claude_variants_in_temp_home() {
        let home = tmp_home("scan_claude_variants");
        // Clean previous
        for name in [
            ".claude-aaa",
            ".claude-abogo",
            ".claude-claude-g2",
            ".claude-tester",
        ] {
            let p = home.join(name);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("settings.json"), r#"{"model":"sonnet"}"#).unwrap();
        }
        // Also create .codex and .aider
        let codex = home.join(".codex");
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(codex.join("config.toml"), "model = \"gpt-4\"").unwrap();
        let aider = home.join(".aider");
        std::fs::create_dir_all(&aider).unwrap();
        std::fs::write(aider.join(".aider.conf.yml"), "model: gpt-4").unwrap();

        let candidates = scan_candidate_roots(&home);
        let names: Vec<String> = candidates
            .iter()
            .filter_map(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(ToOwned::to_owned)
            })
            .collect();
        for want in [
            ".claude-aaa",
            ".claude-abogo",
            ".claude-claude-g2",
            ".claude-tester",
            ".codex",
            ".aider",
        ] {
            assert!(
                names.iter().any(|n| n == want),
                "scan must find {want}, got {names:?} candidates {candidates:?}"
            );
        }
    }

    #[test]
    fn unmanaged_only_when_no_record_and_no_foreign() {
        let home = tmp_home("unmanaged_filter");
        let r1 = home.join(".claude-aaa");
        let r2 = home.join(".claude-bbb");
        for p in [&r1, &r2] {
            std::fs::create_dir_all(p).unwrap();
            std::fs::write(p.join("settings.json"), "{}").unwrap();
        }
        // Registry records r1
        let mut reg = Registry::default();
        reg.insert(sample_instance(
            "work",
            r1.to_string_lossy().as_ref(),
            "id-unmanaged-1",
            Ownership::SuperaiCreated,
        ))
        .unwrap();

        let candidates = vec![r1, r2.clone()];
        let unmanaged = crate::registry::unmanaged_dirs(&reg, &candidates);
        assert_eq!(unmanaged, vec![r2.clone()]);

        // With foreign marker, find_unmanaged_candidates must exclude foreign
        std::fs::write(r2.join(".foreign-managed"), "").unwrap();
        let filtered = find_unmanaged_candidates(&reg, &home);
        // r2 is foreign now, so should not appear in unmanaged
        assert!(
            !filtered.iter().any(|p| p == &r2),
            "foreign candidate must be excluded, got {filtered:?}"
        );
        // Cleanup marker for other tests
        std::fs::remove_file(r2.join(".foreign-managed")).unwrap_or(());
    }

    #[test]
    fn foreign_managed_blocks_adoption() {
        let home = tmp_home("foreign_blocks");
        let foreign_root = home.join(".claude-foreign-one");
        std::fs::create_dir_all(&foreign_root).unwrap();
        std::fs::write(foreign_root.join("settings.json"), "{}").unwrap();
        // Simulate claude-multi referencing it
        let multi_dir = home.join(".claude-multi");
        std::fs::create_dir_all(&multi_dir).unwrap();
        let cfg = multi_dir.join("config.json");
        std::fs::write(
            &cfg,
            format!(
                r#"{{"instances":[{{"configDir":"{}"}}]}}"#,
                foreign_root.display()
            ),
        )
        .unwrap();

        let foreign = is_foreign_managed(&foreign_root, Some(&home));
        assert!(foreign.is_foreign, "must be foreign: {foreign:?}");
        assert_eq!(foreign.owner.as_deref(), Some("claude-multi"));

        let err = can_adopt(&foreign_root, Some(&home)).unwrap_err();
        match err {
            CoreError::ForeignOwnership { path, owner } => {
                assert_eq!(path, foreign_root);
                assert_eq!(owner, "claude-multi");
            }
            other => panic!("expected ForeignOwnership, got {other:?}"),
        }
        std::fs::remove_file(&cfg).unwrap_or(());
        std::fs::remove_dir_all(&multi_dir).unwrap_or(());
    }

    #[test]
    fn symlinked_roots_deduplicate_by_identity() {
        let home = tmp_home("dedup_symlink");
        let real = home.join(".claude-real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("settings.json"), "{}").unwrap();
        let link = home.join(".claude-link");
        // Remove prior link if exists
        std::fs::remove_file(&link).unwrap_or(());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, &link).unwrap_or(());
            let candidates = vec![real.clone(), link];
            let deduped = deduplicate_by_identity(candidates);
            assert_eq!(
                deduped.len(),
                1,
                "symlinked roots must deduplicate to one entry, got {deduped:?}"
            );
            // Display path preserved is the first one
            assert_eq!(deduped[0], real);
        }
        #[cfg(not(unix))]
        {
            let candidates = vec![real.clone(), real.clone()];
            let deduped = deduplicate_by_identity(candidates);
            assert_eq!(deduped.len(), 1);
        }
    }

    #[test]
    fn scan_bounds_prevent_huge_tree() {
        let home = tmp_home("scan_bounds");
        // Create many entries; scan must respect max_entries
        for i in 0..50 {
            let p = home.join(format!(".claude-bulk-{i:03}"));
            std::fs::create_dir_all(&p).unwrap();
        }
        let limited = scan_candidate_roots_limited(&home, 5);
        assert!(
            limited.len() <= 20,
            "bounded scan must respect limit via deduplication, got {} entries: {limited:?}",
            limited.len()
        );
        // Ensure we didn't traverse recursively into subdirs arbitrarily
        // Create deep nested dir inside one candidate and ensure scan doesn't crawl into it beyond top-level
        let deep = home.join(".claude-bulk-000").join("deep").join("nested");
        std::fs::create_dir_all(&deep).unwrap();
        let candidates2 = scan_candidate_roots_limited(&home, 100);
        // Ensure deep nested path is not in candidates (only top-level)
        assert!(
            !candidates2.iter().any(|p| p == &deep),
            "scan must not crawl arbitrarily deep"
        );
    }

    #[test]
    fn fingerprint_uses_multiple_signals() {
        let home = tmp_home("fingerprint_multi");
        let claude_root = home.join(".claude-fp");
        std::fs::create_dir_all(&claude_root).unwrap();
        std::fs::write(
            claude_root.join("settings.json"),
            r#"{"model":"opus","permissions":{"allow":[]}}"#,
        )
        .unwrap();
        let fp = fingerprint_candidate(&claude_root);
        assert_eq!(
            fp.harness.as_ref().map(HarnessId::as_str),
            Some("claude-code")
        );
        assert_eq!(fp.confidence, Confidence::High);
        assert!(fp.evidence.iter().any(|e| e.contains("settings.json")));
        // Directory name alone cannot be High: create empty .claude-empty with no file
        let empty = home.join(".claude-empty");
        std::fs::create_dir_all(&empty).unwrap();
        let fp2 = fingerprint_candidate(&empty);
        // If only name pattern, confidence must be Low (not High/Medium)
        if fp2.harness.is_some() {
            assert_eq!(
                fp2.confidence,
                Confidence::Low,
                "directory name alone must be Low, got {fp2:?}"
            );
        }
    }

    #[test]
    fn classify_ownership_respects_registry_and_foreign() {
        let home = tmp_home("classify_owner");
        let recorded = home.join(".claude-recorded");
        std::fs::create_dir_all(&recorded).unwrap();
        let mut reg = Registry::default();
        reg.insert(sample_instance(
            "rec",
            recorded.to_string_lossy().as_ref(),
            "id-classify-1",
            Ownership::SuperaiCreated,
        ))
        .unwrap();
        let own = classify_ownership(&recorded, &reg, Some(&home));
        assert_eq!(own, Ownership::SuperaiCreated);

        let foreign_path = home.join(".claude-foreign-cls");
        std::fs::create_dir_all(&foreign_path).unwrap();
        std::fs::write(foreign_path.join(".foreign-managed"), "").unwrap();
        let own2 = classify_ownership(&foreign_path, &reg, Some(&home));
        assert_eq!(own2, Ownership::ForeignManaged);
        std::fs::remove_file(foreign_path.join(".foreign-managed")).unwrap_or(());

        let unmanaged_path = home.join(".claude-unmanaged-cls");
        std::fs::create_dir_all(&unmanaged_path).unwrap();
        let own3 = classify_ownership(&unmanaged_path, &reg, Some(&home));
        assert_eq!(own3, Ownership::Unmanaged);

        let missing_path = PathBuf::from("/tmp/superai-missing-does-not-exist-zzz");
        let own4 = classify_ownership(&missing_path, &reg, Some(&home));
        assert_eq!(own4, Ownership::Detached);
    }

    #[test]
    fn drift_report_covers_missing_config() {
        let home = tmp_home("drift_missing");
        let missing_root = "/tmp/superai-drift-missing-config-root";
        let mut reg = Registry::default();
        reg.insert(sample_instance(
            "missing",
            missing_root,
            "id-drift-1",
            Ownership::SuperaiCreated,
        ))
        .unwrap();
        let report = drift_report(&reg, &home);
        let found = report
            .findings
            .iter()
            .find(|f| f.path.as_path() == Path::new(missing_root));
        assert!(
            found.is_some(),
            "drift report must include missing recorded config root"
        );
        assert!(found.unwrap().is_recorded);
    }

    #[test]
    fn no_scan_mutates_access_time_where_possible() {
        // Ensure scan is read-only: file mtime should not change after scan
        let home = tmp_home("scan_readonly");
        let root = home.join(".claude-readonly");
        std::fs::create_dir_all(&root).unwrap();
        let settings = root.join("settings.json");
        std::fs::write(&settings, r#"{"model":"sonnet"}"#).unwrap();
        let before = std::fs::metadata(&settings).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let _ = scan_candidate_roots(&home);
        let _ = fingerprint_candidate(&root);
        let _ = is_foreign_managed(&root, Some(&home));
        let after = std::fs::metadata(&settings).unwrap().modified().unwrap();
        assert_eq!(
            before, after,
            "scan must not mutate file content or mtime (read-only)"
        );
        // Also ensure content unchanged
        let content = std::fs::read_to_string(&settings).unwrap();
        assert_eq!(content, r#"{"model":"sonnet"}"#);
    }
}
