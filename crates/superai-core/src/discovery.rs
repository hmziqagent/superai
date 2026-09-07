//! Discovery, ownership, and drift classification.
//!
//! Scans adapter-driven candidate roots, fingerprints harness identity,
//! classifies ownership (including foreign managers like `claude-multi`),
//! and produces a bounded drift report without mutating scanned files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

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
    /// Whether the evidence is AMBIGUOUS (DRF-04): a foreign manager is
    /// plausibly present but no direct link proves ownership. Ambiguity
    /// blocks adopt/remove — it never silently resolves to unmanaged.
    pub ambiguous: bool,
}

/// Drift category for a finding (DRF drift-category list — never collapse
/// everything into "unmanaged").
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DriftCategory {
    /// Recorded instance whose root, wrapper, binary and version all check out.
    RecordedHealthy,
    /// Recorded config root missing on disk.
    RecordedConfigMissing,
    /// Recorded absolute binary path missing.
    RecordedBinaryMissing,
    /// Recorded wrapper file missing.
    RecordedWrapperMissing,
    /// Recorded wrapper content no longer matches its recorded digest.
    WrapperChanged,
    /// Recorded harness version incompatible for writes.
    RecordedVersionUnsupported,
    /// Default config root present but unrecorded.
    DefaultUnrecorded,
    /// Candidate on disk, no record, no foreign manager.
    CandidateUnmanaged,
    /// Another manager owns the path (evidence present).
    ForeignManaged,
    /// Evidence does not decide an owner (DRF-04).
    AmbiguousOwnership,
    /// A superai wrapper whose instance id has no registry record.
    OrphanWrapper,
    /// Two records share one config root.
    DuplicateRoot,
    /// Two records share one wrapper command (case-folded).
    DuplicateWrapper,
    /// Fixed-path instance has no active profile identity.
    FixedPathProfileInactive,
    /// Daemon-class instance has no live daemon identity.
    DaemonStopped,
}

impl std::fmt::Display for DriftCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::RecordedHealthy => "recorded_healthy",
            Self::RecordedConfigMissing => "recorded_config_missing",
            Self::RecordedBinaryMissing => "recorded_binary_missing",
            Self::RecordedWrapperMissing => "recorded_wrapper_missing",
            Self::WrapperChanged => "wrapper_changed",
            Self::RecordedVersionUnsupported => "recorded_version_unsupported",
            Self::DefaultUnrecorded => "default_unrecorded",
            Self::CandidateUnmanaged => "candidate_unmanaged",
            Self::ForeignManaged => "foreign_managed",
            Self::AmbiguousOwnership => "ambiguous_ownership",
            Self::OrphanWrapper => "orphan_wrapper",
            Self::DuplicateRoot => "duplicate_root",
            Self::DuplicateWrapper => "duplicate_wrapper",
            Self::FixedPathProfileInactive => "fixed_path_profile_inactive",
            Self::DaemonStopped => "daemon_stopped",
        };
        f.write_str(s)
    }
}

/// Risk level for a finding or group (DRF-08).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RiskLevel {
    /// Nothing wrong; informational only.
    Info,
    /// Cosmetic or self-healing; act when convenient.
    Low,
    /// Instance degraded (missing/broken piece); act soon.
    Medium,
    /// Data-loss or wrong-owner hazard; act before further mutation.
    High,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        };
        f.write_str(s)
    }
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
    /// Primary drift category for this candidate (DRF-08).
    pub category: DriftCategory,
    /// Risk level for this finding.
    pub risk: RiskLevel,
    /// Recommended next operations (stable identifiers, no UI formatting).
    pub next_operations: Vec<String>,
}

/// One wrapper file found in a configured bin directory (DRF-03).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapperFinding {
    /// Absolute wrapper file path.
    pub path: PathBuf,
    /// What the file is.
    pub kind: WrapperFindingKind,
    /// For a superai wrapper: whether its instance id has a registry record.
    pub recorded: bool,
    /// Risk level.
    pub risk: RiskLevel,
    /// Recommended next operations.
    pub next_operations: Vec<String>,
}

/// Classification of a file found in a wrapper directory (DRF-03/04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WrapperFindingKind {
    /// A superai-generated wrapper with a parseable marker.
    SuperaiWrapper {
        /// Instance id embedded in the marker.
        instance_id: String,
        /// Digest embedded in the marker.
        digest: String,
    },
    /// A user-owned wrapper matching a known isolation recipe.
    Foreign {
        /// Why it is foreign.
        reason: String,
    },
    /// A package-manager shim (mise/asdf style) — never an instance wrapper.
    PackageShim {
        /// Manager the shim belongs to.
        manager: String,
    },
    /// Unrecognized content; never executed during scan.
    Opaque {
        /// Why it is opaque.
        reason: String,
    },
}

/// One group of findings sharing a harness and (when matched) an instance
/// (DRF-08: findings grouped by harness/instance with support/version,
/// risk and next operations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftGroup {
    /// Harness the group belongs to.
    pub harness: HarnessId,
    /// Registry record the group is attached to, when one matched.
    pub instance: Option<crate::ids::InstanceId>,
    /// Highest-severity category in the group.
    pub category: DriftCategory,
    /// Highest risk across the group's findings.
    pub risk: RiskLevel,
    /// Catalog support state for the harness (`None` when uncataloged).
    pub adapter_support: Option<crate::state::AdapterSupport>,
    /// Adapter-declared revision / detected version note.
    pub adapter_version: Option<String>,
    /// Union of recommended next operations.
    pub next_operations: Vec<String>,
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
    /// Wrapper files found in configured bin directories (DRF-03).
    pub wrapper_findings: Vec<WrapperFinding>,
    /// Findings grouped by harness/instance (DRF-08).
    pub groups: Vec<DriftGroup>,
    /// Permission errors skipped during the scan, surfaced as diagnostics.
    pub permission_diagnostics: Vec<String>,
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

    // Version marker (DRF-02 signal): a `version.txt` beside the canonical
    // config is the corpora's install-era marker. It corroborates a
    // canonical-file identification (Medium -> High, multiple consistent
    // signals) but can never promote a name-pattern-only match.
    let version_marker = path.join("version.txt");
    if version_marker.is_file() {
        evidence.push(format!(
            "version marker present at {}",
            version_marker.display()
        ));
        if let Ok(text) = read_bounded(&version_marker, 4096)
            && !text.trim().is_empty()
        {
            evidence.push(format!("version marker reads {}", text.trim()));
            if confidence == Confidence::Medium && harness_found.is_some() {
                confidence = Confidence::High;
            }
        }
    }

    // Matching binary/app install (DRF-02 signal): PATH lookup ONLY — the
    // binary is never executed during fingerprinting. Corroborating
    // evidence; a directory name alone still cannot establish the harness.
    if let Some(harness) = harness_found.and_then(|s| HarnessId::new(s).ok()) {
        let exe = crate::wrapper::executable_for_harness(&harness);
        if let Some(found) = binary_on_path_from_env(&exe) {
            evidence.push(format!(
                "matching binary `{exe}` found on PATH at {} (not executed)",
                found.display()
            ));
        }
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

/// Orchestrator workspace markers (DRF-04) per
/// docs/harness-configs/orchestrators.md: each GUI orchestrator keeps its
/// managed workspaces under a well-known root. A candidate living under one
/// of those roots is orchestrator-managed — superai never adopts or removes
/// another manager's workspace.
const ORCHESTRATOR_WORKSPACE_MARKERS: &[(&str, &str)] = &[
    // Vibe Kanban: worktrees live under `.vibe-kanban-workspaces/`
    // (configurable in Settings → General, but the default is the marker).
    (".vibe-kanban-workspaces", "vibe-kanban"),
    // Conductor: workspaces live under `~/conductor/workspaces/`
    // (docs/concepts/workspaces-and-branches + troubleshooting).
    ("conductor/workspaces", "conductor"),
    // Sculptor: workspaces are git worktrees under
    // `~/.sculptor/workspaces/<id>/code/`.
    (".sculptor/workspaces", "sculptor"),
];

/// Detect whether `path` is a workspace managed by a known GUI orchestrator
/// (DRF-04 "orchestrator-managed profiles where local evidence exists").
///
/// Evidence is structural and local only: a path component sequence matching
/// a documented orchestrator workspace root (compared case-insensitively on
/// the separator-normalized path, so Windows separators match too), or a
/// home-level orchestrator settings file (`~/.conductor/settings.toml`,
/// `~/.sculptor/.env`) that references the candidate (bounded read, the
/// same discipline as the claude-multi config check). Nothing is executed.
fn detect_orchestrator_manager(path: &Path, home: Option<&Path>) -> Option<(&'static str, String)> {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let lowered = normalized.to_ascii_lowercase();
    for (marker, owner) in ORCHESTRATOR_WORKSPACE_MARKERS {
        if lowered.contains(marker) {
            return Some((
                owner,
                format!(
                    "candidate {} lies under the documented {owner} workspace root `{marker}`",
                    path.display()
                ),
            ));
        }
    }
    let home_path = home?;
    // Conductor user settings referencing the candidate (bounded, no parse).
    let conductor_settings = home_path.join(".conductor").join("settings.toml");
    if conductor_settings.is_file()
        && let Ok(text) = read_bounded(&conductor_settings, 256 * 1024)
        && text.contains(path.to_string_lossy().as_ref())
    {
        return Some((
            "conductor",
            format!(
                "candidate {} referenced in {}",
                path.display(),
                conductor_settings.display()
            ),
        ));
    }
    // Sculptor global env referencing the candidate (bounded, no parse).
    let sculptor_env = home_path.join(".sculptor").join(".env");
    if sculptor_env.is_file()
        && let Ok(text) = read_bounded(&sculptor_env, 64 * 1024)
        && text.contains(path.to_string_lossy().as_ref())
    {
        return Some((
            "sculptor",
            format!(
                "candidate {} referenced in {}",
                path.display(),
                sculptor_env.display()
            ),
        ));
    }
    None
}

/// Detect whether `path` is owned by a foreign manager.
///
/// Checks in order:
/// - `.foreign-managed` marker inside the candidate
/// - `.claude-multi` sibling marker
/// - `$HOME/.claude-multi/config.json` referencing the candidate
/// - orchestrator-managed workspace roots / settings (DRF-04)
/// - generic `.superai-foreign` marker
///
/// Never parses a known secret store; only bounded reads of small config files.
#[expect(
    clippy::excessive_nesting,
    reason = "foreign check branches are explicit"
)]
pub fn is_foreign_managed(path: &Path, home: Option<&Path>) -> ForeignCheck {
    let mut evidence: Vec<String> = Vec::new();
    let mut ambiguous = false;

    // Generic marker files inside candidate
    for marker in [".foreign-managed", ".superai-foreign", ".owned-by-foreign"] {
        let candidate = path.join(marker);
        if candidate.is_file() {
            evidence.push(format!("marker {marker} found at {}", candidate.display()));
            return ForeignCheck {
                is_foreign: true,
                owner: Some("generic-marker".to_owned()),
                evidence,
                ambiguous: false,
            };
        }
    }

    // Orchestrator-managed workspaces (DRF-04): structural evidence first so
    // an orchestrator workspace is never adoptable, regardless of what other
    // markers sit beside it.
    if let Some((owner, reason)) = detect_orchestrator_manager(path, home) {
        evidence.push(reason);
        return ForeignCheck {
            is_foreign: true,
            owner: Some(owner.to_owned()),
            evidence,
            ambiguous: false,
        };
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
            ambiguous: false,
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
                        ambiguous: false,
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
                    "foreign manager directory .claude-multi exists at {} but does not link {}",
                    multi_dir.display(),
                    path.display()
                ));
                // DRF-04: a foreign manager is plausibly present but nothing
                // links THIS candidate to it — ambiguous evidence, never a
                // silent unmanaged classification.
                ambiguous = true;
            }
        }

        // Mise / package-manager shim check (DRF-04): a candidate that is
        // itself a package-manager shim is neither a config root nor ours.
        if let Some(manager) = detect_shim_manager(path) {
            evidence.push(format!(
                "path {} looks like a package-manager shim ({manager})",
                path.display()
            ));
            ambiguous = true;
        }
    }

    // No evidence of foreign ownership
    evidence.push(format!("no foreign marker found for {}", path.display()));
    ForeignCheck {
        is_foreign: false,
        owner: None,
        evidence,
        ambiguous,
    }
}

// ---------------------------------------------------------------------------
// PATH adjacency + package-manager shim detection (DRF-02/04)
// ---------------------------------------------------------------------------

/// Find `name` as an executable file on a `PATH`-shaped string (lookup
/// only; nothing is executed). Pure over its inputs so tests can pass a
/// synthetic PATH.
pub fn binary_on_path(path_var: &str, name: &str) -> Option<PathBuf> {
    let separator = if cfg!(windows) { ';' } else { ':' };
    for dir in path_var.split(separator) {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
        // Windows executability is extension-based: when the bare name has
        // no extension, probe the core PATHEXT extensions so a lookup of
        // `claude` resolves the installed `claude.exe`/`claude.cmd`.
        #[cfg(windows)]
        {
            if Path::new(name).extension().is_none() {
                for ext in ["exe", "cmd", "bat", "com"] {
                    let candidate = Path::new(dir).join(format!("{name}.{ext}"));
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    None
}

/// Unix: any exec bit set. Windows: extension-based (PATHEXT core set), so
/// an extensionless data file never counts as a PATH binary.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "exe" | "cmd" | "bat" | "com"
            )
        })
}

#[cfg(not(any(unix, windows)))]
fn is_executable(_path: &Path) -> bool {
    true
}

/// [`binary_on_path`] over the process `PATH`.
fn binary_on_path_from_env(name: &str) -> Option<PathBuf> {
    binary_on_path(&std::env::var("PATH").ok()?, name)
}

/// Detect whether `path` is a package-manager shim rather than an instance
/// wrapper (DRF-04). mise shims are tiny generated shell scripts that exec
/// `mise x --` / `mise run`; asdf shims look the same shape. A shim is
/// NEVER a superai wrapper and NEVER a config root — ownership stays with
/// the package manager.
pub fn detect_shim_manager(path: &Path) -> Option<&'static str> {
    let data = std::fs::read(path).ok()?;
    if data.len() > 16 * 1024 {
        return None;
    }
    let text = String::from_utf8_lossy(&data);
    if text.contains("mise x --")
        || text.contains("mise run")
        || text.contains("exec mise ")
        || text.contains("mise hook")
    {
        return Some("mise");
    }
    if text.contains(".asdf/shims") || text.contains("asdf exec") {
        return Some("asdf");
    }
    None
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

/// Collect adapter-derived candidate patterns for the scan.
///
/// Patterns come from every adapter in the harness catalog via
/// [`crate::harness_catalog::all_adapters`]: concrete adapters contribute
/// their `scan_candidates`, and a harness without a concrete adapter falls
/// back to the generic `~/.<id>` hints of [`crate::adapter::GenericAdapter`].
/// The scanner keeps only patterns that expand to an existing path (see
/// [`scan_candidate_roots_limited`]).
fn candidate_patterns() -> Vec<String> {
    crate::harness_catalog::all_adapters()
        .iter()
        .flat_map(|adapter| adapter.scan_candidates())
        .collect()
}

/// Scan `home` for candidate config roots.
///
/// Bounded: no unrestricted crawl, at most one level under `home` and `.config`,
/// at most `MAX_HOME_ENTRIES` entries, skips permission errors, never parses secret stores.
pub fn scan_candidate_roots(home: &Path) -> Vec<PathBuf> {
    scan_candidate_roots_limited(home, MAX_HOME_ENTRIES)
}

/// Same as `scan_candidate_roots` but with an explicit entry limit (for tests).
pub fn scan_candidate_roots_limited(home: &Path, max_entries: usize) -> Vec<PathBuf> {
    scan_with_diagnostics(home, &ScanOptions::with_entry_limit(max_entries)).candidates
}

/// Scan inputs (DRF-01): adapter defaults and globs, env hints, plus
/// USER-SPECIFIED roots and the wrapper directories to scan for orphan
/// launchers. Everything stays bounded.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// User-specified extra scan roots (absolute paths). Included as-is when
    /// they exist; bounded by `max_entries`.
    pub extra_roots: Vec<PathBuf>,
    /// Configured wrapper bin directories to scan one level deep (DRF-03).
    pub wrapper_dirs: Vec<PathBuf>,
    /// Entry budget for the home/XDG crawls.
    pub max_entries: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            extra_roots: Vec::new(),
            wrapper_dirs: Vec::new(),
            max_entries: MAX_HOME_ENTRIES,
        }
    }
}

impl ScanOptions {
    /// Options with only the entry budget changed.
    pub fn with_entry_limit(max_entries: usize) -> Self {
        Self {
            max_entries,
            ..Self::default()
        }
    }
}

/// Default user-owned bin directories wrappers are installed into
/// (DRF-03 "configured wrapper directories"): `~/.local/bin` (XDG) and
/// superai's own `~/.superai/bin`. Only these get the bounded one-level
/// wrapper scan — never a PATH crawl.
pub fn default_wrapper_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".local").join("bin"),
        home.join(".superai").join("bin"),
    ]
}

/// Result of a bounded scan with surfaced diagnostics (DRF-01).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanReport {
    /// Candidate roots found (deduplicated, sorted).
    pub candidates: Vec<PathBuf>,
    /// Permission errors skipped during the scan (surfaced, not swallowed).
    pub permission_diagnostics: Vec<String>,
}

/// Expand a single-level glob pattern (`prefix/*.ext` or `prefix/*`) under
/// `home` into at most `MAX_GLOB_ENTRIES` existing paths. Patterns without
/// `*` are handled by [`expand_pattern`]; a glob in any component other
/// than the last is not expanded (bounded by design).
fn expand_glob_candidates(pattern: &str, home: &Path) -> Vec<PathBuf> {
    const MAX_GLOB_ENTRIES: usize = 256;
    let Some(star) = pattern.find('*') else {
        return Vec::new();
    };
    // Only a final-component glob is expandable: no further '/' after the '*'.
    let tail = pattern.get(star + 1..).unwrap_or_default();
    if tail.contains('/') || tail.contains('\\') {
        return Vec::new();
    }
    let dir_part = pattern.get(..star).unwrap_or_default();
    let dir = if dir_part.is_empty() {
        home.to_path_buf()
    } else {
        match expand_pattern(dir_part, home) {
            Some(d) => d,
            None => return Vec::new(),
        }
    };
    let suffix = tail.trim_start_matches('*');
    let mut out: Vec<PathBuf> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    for entry in entries {
        if out.len() >= MAX_GLOB_ENTRIES {
            break;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if name_str.starts_with('.') {
            continue;
        }
        if suffix.is_empty() || name_str.ends_with(suffix) {
            let p = entry.path();
            if p.is_dir() || p.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Bounded scan with user-specified roots, glob expansion, a wall-clock
/// budget, and permission diagnostics surfaced instead of swallowed
/// (DRF-01). Never parses known secret stores.
#[expect(clippy::excessive_nesting, reason = "scan branches are explicit")]
#[expect(clippy::too_many_lines, reason = "scan is bounded and explicit")]
pub fn scan_with_diagnostics(home: &Path, options: &ScanOptions) -> ScanReport {
    const SCAN_TIME_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);
    let deadline = std::time::Instant::now() + SCAN_TIME_BUDGET;
    let mut diagnostics: Vec<String> = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let mut push_candidate = |p: PathBuf| {
        // Deduplicate lexically via normalized string
        let key = normalize_path(&p).to_string_lossy().into_owned();
        if seen.insert(key) {
            candidates.push(p);
        }
    };

    // 0) User-specified scan roots (DRF-01): honored before everything else.
    for root in &options.extra_roots {
        if std::time::Instant::now() >= deadline {
            diagnostics.push("scan time budget reached before user-specified roots".to_owned());
            break;
        }
        if root.is_dir() || root.is_file() {
            push_candidate(root.clone());
        }
    }

    // 1) Explicit adapter patterns: plain patterns expand directly; glob
    // patterns (amazon-q `~/.aws/amazonq/cli-agents/*.json`, warp
    // `~/.warp/workflows/*.yaml`, …) get a bounded single-level expansion
    // instead of being silently dropped.
    for pattern in candidate_patterns() {
        if std::time::Instant::now() >= deadline {
            diagnostics.push("scan time budget reached during pattern expansion".to_owned());
            break;
        }
        if pattern.contains('*') {
            for expanded in expand_glob_candidates(&pattern, home) {
                push_candidate(expanded);
            }
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
    match std::fs::read_dir(home) {
        Ok(entries) => {
            let mut count: usize = 0;
            for entry_res in entries {
                if count >= options.max_entries || std::time::Instant::now() >= deadline {
                    break;
                }
                match entry_res {
                    Ok(entry) => {
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
                    Err(e) => {
                        count = count.saturating_add(1);
                        diagnostics.push(format!(
                            "skipped unreadable entry under {}: {e}",
                            home.display()
                        ));
                    }
                }
            }
        }
        Err(e) => diagnostics.push(format!("cannot read {}: {e}", home.display())),
    }

    // 4) XDG / platform application directories: ~/.config/* for relevant harnesses
    let config_dir = home.join(".config");
    match std::fs::read_dir(&config_dir) {
        Ok(entries) => {
            let mut count: usize = 0;
            for entry_res in entries {
                if count >= MAX_XDG_ENTRIES || std::time::Instant::now() >= deadline {
                    break;
                }
                match entry_res {
                    Ok(entry) => {
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
                    Err(e) => {
                        count = count.saturating_add(1);
                        diagnostics.push(format!(
                            "skipped unreadable entry under {}: {e}",
                            config_dir.display()
                        ));
                    }
                }
            }
        }
        Err(e) => diagnostics.push(format!("cannot read {}: {e}", config_dir.display())),
    }

    // Deduplicate by file identity (inode) where possible, preserving display path
    let deduped = deduplicate_by_identity(candidates);
    let mut sorted = deduped;
    sorted.sort();
    ScanReport {
        candidates: sorted,
        permission_diagnostics: diagnostics,
    }
}

/// Deduplicate candidates by file identity without losing display path.
///
/// On Unix, two paths that point to the same inode/device are considered one.
/// Otherwise, lexical dedup is used. The first occurrence's display path is kept.
pub fn deduplicate_by_identity(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        dedup_by_identity_unix(candidates)
    }
    #[cfg(not(unix))]
    {
        dedup_by_identity_lexical(candidates)
    }
}

/// Unix: same inode/device is one entry; the first display path is kept.
#[cfg(unix)]
fn dedup_by_identity_unix(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
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

/// Non-unix: no inode identity; case-folded lexical dedup (Windows filesystems
/// are case-insensitive by default).
#[cfg(not(unix))]
fn dedup_by_identity_lexical(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
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

/// Scan the CONFIGURED wrapper directories only (DRF-03): a bounded,
/// one-level-deep listing of each dir (at most 512 entries), classifying
/// every file without executing it. superai wrappers whose instance id has
/// no registry record are `OrphanWrapper` findings; package-manager shims are
/// distinguished from wrappers (DRF-04).
#[expect(
    clippy::excessive_nesting,
    reason = "wrapper-dir classification branches are explicit"
)]
pub fn scan_wrapper_dirs(dirs: &[PathBuf], registry: &Registry) -> Vec<WrapperFinding> {
    const MAX_WRAPPER_DIR_ENTRIES: usize = 512;
    let mut findings: Vec<WrapperFinding> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut count: usize = 0;
        for entry in entries {
            if count >= MAX_WRAPPER_DIR_ENTRIES {
                break;
            }
            count = count.saturating_add(1);
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if let Some(manager) = detect_shim_manager(&path) {
                findings.push(WrapperFinding {
                    path: path.clone(),
                    kind: WrapperFindingKind::PackageShim {
                        manager: manager.to_owned(),
                    },
                    recorded: false,
                    risk: RiskLevel::Info,
                    next_operations: vec![format!(
                        "none: package-manager `{manager}` shim; manage it with {manager}"
                    )],
                });
                continue;
            }
            match crate::wrapper::detect_wrapper_kind(&path) {
                crate::wrapper::WrapperKind::SuperaiOwned {
                    digest,
                    instance_id,
                } => {
                    let recorded = instance_id
                        .as_deref()
                        .is_some_and(|id| registry.get_by_id(id).is_some());
                    findings.push(WrapperFinding {
                        path: path.clone(),
                        kind: WrapperFindingKind::SuperaiWrapper {
                            instance_id: instance_id.clone().unwrap_or_default(),
                            digest,
                        },
                        recorded,
                        risk: if recorded {
                            RiskLevel::Info
                        } else {
                            RiskLevel::Medium
                        },
                        next_operations: if recorded {
                            vec!["none: wrapper matches its instance record".to_owned()]
                        } else {
                            vec![
                                "adopt_orphan_wrapper: record the instance the marker names"
                                    .to_owned(),
                                "quarantine_orphan_wrapper: move it aside if target proves safe"
                                    .to_owned(),
                            ]
                        },
                    });
                }
                crate::wrapper::WrapperKind::Foreign { reason } => {
                    findings.push(WrapperFinding {
                        path: path.clone(),
                        kind: WrapperFindingKind::Foreign {
                            reason: reason.clone(),
                        },
                        recorded: false,
                        risk: RiskLevel::Low,
                        next_operations: vec![
                            "none: user-owned wrapper is never adopted or overwritten".to_owned(),
                        ],
                    });
                }
                crate::wrapper::WrapperKind::Opaque { reason } => {
                    findings.push(WrapperFinding {
                        path: path.clone(),
                        kind: WrapperFindingKind::Opaque {
                            reason: reason.clone(),
                        },
                        recorded: false,
                        risk: RiskLevel::Info,
                        next_operations: vec![
                            "none: opaque launcher; never executed or rewritten by scan".to_owned(),
                        ],
                    });
                }
                crate::wrapper::WrapperKind::Missing => {}
            }
        }
    }
    findings.sort_by(|a, b| a.path.cmp(&b.path));
    findings
}

// ---------------------------------------------------------------------------
// registry reconciliation (DRF-05)
// ---------------------------------------------------------------------------

/// Marker file name superai writes into config roots it creates; carries
/// the stable `InstanceId` so reconciliation matches identity FIRST, before
/// any path comparison (DRF-05).
pub const INSTANCE_MARKER_FILE: &str = ".superai-instance";

/// Read the `InstanceId` marker from a config root, when present.
pub fn read_instance_marker(root: &Path) -> Option<crate::ids::InstanceId> {
    let text = std::fs::read_to_string(root.join(INSTANCE_MARKER_FILE)).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    crate::ids::InstanceId::new(trimmed).ok()
}

/// How a registry record was matched to a discovered candidate (DRF-05).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchBasis {
    /// The candidate's `.superai-instance` marker names the record's id.
    InstanceMarker,
    /// Exact normalized config-root equality.
    ExactConfigRoot,
    /// The record's owned wrapper metadata matches a discovered wrapper.
    OwnedWrapperMetadata,
}

/// One reconciliation row: a registry record and the candidate it matches
/// (or none). Records are NEVER merged on user-facing name alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciliation {
    /// Registry record id.
    pub instance: crate::ids::InstanceId,
    /// Registry record name (display only — never a match key).
    pub name: crate::ids::InstanceName,
    /// Matched candidate root, when one matched.
    pub candidate: Option<PathBuf>,
    /// How the match was made.
    pub basis: Option<MatchBasis>,
}

/// Match stable `InstanceId` marker first, then exact normalized config root,
/// then exact owned wrapper metadata (DRF-05). Read-only.
#[expect(
    clippy::excessive_nesting,
    reason = "three-basis reconciliation matches are explicit"
)]
pub fn reconcile(
    registry: &Registry,
    candidates: &[PathBuf],
    wrapper_findings: &[WrapperFinding],
) -> Vec<Reconciliation> {
    let mut out: Vec<Reconciliation> = Vec::new();
    for inst in registry.instances() {
        // 1) marker first: any candidate whose marker names this record
        let mut matched: Option<(PathBuf, MatchBasis)> = None;
        for cand in candidates {
            if read_instance_marker(cand).is_some_and(|id| id == inst.id) {
                matched = Some((cand.clone(), MatchBasis::InstanceMarker));
                break;
            }
        }
        // 2) exact normalized config root
        let norm = normalize_path(inst.config_root.as_path());
        if matched.is_none() {
            for cand in candidates {
                if normalize_path(cand) == norm {
                    matched = Some((cand.clone(), MatchBasis::ExactConfigRoot));
                    break;
                }
            }
        }
        // 3) exact owned wrapper metadata: the record's wrapper path +
        //    content digest match a discovered superai wrapper
        if matched.is_none()
            && let Some(wrapper) = &inst.wrapper
        {
            for finding in wrapper_findings {
                if let WrapperFindingKind::SuperaiWrapper { digest, .. } = &finding.kind
                    && finding.path == wrapper.path.as_path()
                    && digest == &wrapper.content_digest
                {
                    matched = Some((
                        inst.config_root.as_path().to_path_buf(),
                        MatchBasis::OwnedWrapperMetadata,
                    ));
                    break;
                }
            }
        }
        out.push(Reconciliation {
            instance: inst.id.clone(),
            name: inst.name.clone(),
            candidate: matched.as_ref().map(|(path, _)| path.clone()),
            basis: matched.as_ref().map(|(_, basis)| basis.clone()),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// drift report (DRF-08)
// ---------------------------------------------------------------------------

/// Classify one candidate into a drift category + risk + next operations
/// (DRF-08). Pure over its inputs.
#[expect(
    clippy::excessive_nesting,
    reason = "per-state drift classification is explicit"
)]
#[expect(
    clippy::too_many_lines,
    reason = "per-state drift classification is explicit"
)]
fn classify_finding(
    is_recorded: bool,
    path: &Path,
    foreign: &ForeignCheck,
    fingerprint: &Fingerprint,
    registry: &Registry,
    home: &Path,
) -> (DriftCategory, RiskLevel, Vec<String>) {
    if foreign.is_foreign {
        return (
            DriftCategory::ForeignManaged,
            RiskLevel::High,
            vec![
                "block: foreign manager owns the path; adoption and removal are refused".to_owned(),
            ],
        );
    }
    if foreign.ambiguous {
        return (
            DriftCategory::AmbiguousOwnership,
            RiskLevel::Medium,
            vec![
                "block: ownership evidence is ambiguous; adopt/remove need explicit resolution"
                    .to_owned(),
            ],
        );
    }
    if is_recorded {
        // Per-record checks: config root exists? wrapper healthy?
        let inst = registry
            .instances()
            .iter()
            .find(|i| normalize_path(i.config_root.as_path()) == normalize_path(path));
        if let Some(inst) = inst {
            if !path.exists() {
                return (
                    DriftCategory::RecordedConfigMissing,
                    RiskLevel::Medium,
                    vec![
                        "repair: recreate the config root (INS-09)".to_owned(),
                        "detach: drop the record if the root is gone for good".to_owned(),
                    ],
                );
            }
            if let Some(wrapper) = &inst.wrapper {
                let wpath = wrapper.path.as_path();
                if !wpath.exists() {
                    return (
                        DriftCategory::RecordedWrapperMissing,
                        RiskLevel::Medium,
                        vec!["repair: regenerate the wrapper (INS-09)".to_owned()],
                    );
                }
                // Strict ownership check (WRP-08 discipline, matching the
                // repair path's full-content comparison): parseable marker +
                // digest EQUALITY. A merely-edited wrapper that still happens
                // to contain the digest string is drift, not health.
                if !crate::wrapper::is_owned_wrapper(wpath, Some(&wrapper.content_digest)) {
                    return (
                        DriftCategory::WrapperChanged,
                        RiskLevel::Medium,
                        vec![
                            "repair: wrapper drift (ownership-aware, INS-09)".to_owned(),
                            "detach: explicit detach if the edit was deliberate".to_owned(),
                        ],
                    );
                }
            }
            if let Some(binary) = &inst.binary
                && let Some(abs) = binary.as_absolute_path()
                && !abs.as_path().exists()
            {
                return (
                    DriftCategory::RecordedBinaryMissing,
                    RiskLevel::Medium,
                    vec![
                        "repair: re-detect or clear the pinned binary (INS-09)".to_owned(),
                        "detach: retain data without superai tracking".to_owned(),
                    ],
                );
            }
            if fingerprint.harness.is_some() {
                return (
                    DriftCategory::RecordedHealthy,
                    RiskLevel::Info,
                    vec!["none: healthy".to_owned()],
                );
            }
        }
        return (
            DriftCategory::RecordedHealthy,
            RiskLevel::Info,
            vec!["none: healthy".to_owned()],
        );
    }
    // Unrecorded: default-root shape or generic candidate.
    let is_default_root = home.join(format!(
        ".{}",
        fingerprint.harness.as_ref().map_or("", |h| h.as_str())
    )) == normalize_path(path);
    if is_default_root {
        return (
            DriftCategory::DefaultUnrecorded,
            RiskLevel::Low,
            vec!["register_default: record the default install without touching it".to_owned()],
        );
    }
    if path.exists() {
        return (
            DriftCategory::CandidateUnmanaged,
            RiskLevel::Low,
            vec![
                "adopt: record-first, config-preserving adoption".to_owned(),
                "ignore: leave the candidate unmanaged".to_owned(),
            ],
        );
    }
    (
        DriftCategory::RecordedConfigMissing,
        RiskLevel::Medium,
        vec!["repair or detach: recorded root missing".to_owned()],
    )
}

/// Highest risk across findings.
fn max_risk(risks: &[RiskLevel]) -> RiskLevel {
    risks.iter().copied().max().unwrap_or(RiskLevel::Info)
}

/// Build the DRF-08 groups: findings grouped by harness/instance with
/// adapter support, adapter version note, risk, and next operations.
#[expect(
    clippy::excessive_nesting,
    reason = "group assembly walks findings per record"
)]
fn build_groups(
    registry: &Registry,
    findings: &[DriftFinding],
    wrapper_findings: &[WrapperFinding],
    reconciliations: &[Reconciliation],
) -> Vec<DriftGroup> {
    let mut groups: Vec<DriftGroup> = Vec::new();
    for rec in reconciliations {
        let Some(inst) = registry.get_by_id(rec.instance.as_str()) else {
            continue;
        };
        let mut categories: Vec<DriftCategory> = Vec::new();
        let mut risks: Vec<RiskLevel> = Vec::new();
        let mut next_ops: Vec<String> = Vec::new();
        for finding in findings {
            if normalize_path(&finding.path) != normalize_path(inst.config_root.as_path()) {
                continue;
            }
            categories.push(finding.category.clone());
            risks.push(finding.risk);
            for op in &finding.next_operations {
                if !next_ops.contains(op) {
                    next_ops.push(op.clone());
                }
            }
        }
        for finding in wrapper_findings {
            if let WrapperFindingKind::SuperaiWrapper { instance_id, .. } = &finding.kind
                && instance_id == inst.id.as_str()
            {
                if !finding.recorded {
                    categories.push(DriftCategory::OrphanWrapper);
                }
                risks.push(finding.risk);
                for op in &finding.next_operations {
                    if !next_ops.contains(op) {
                        next_ops.push(op.clone());
                    }
                }
            }
        }
        let entry = crate::harness_catalog::find_by_id(inst.harness.as_str());
        let adapter_version = crate::harness_catalog::concrete_adapter_for(inst.harness.as_str())
            .map(|adapter| {
                let resolution = adapter.version_resolution();
                resolution
                    .detected_version
                    .unwrap_or_else(|| "version unknown".to_owned())
            });
        if categories.is_empty() {
            categories.push(DriftCategory::RecordedHealthy);
        }
        groups.push(DriftGroup {
            harness: inst.harness.clone(),
            instance: Some(inst.id.clone()),
            category: pick_primary_category(&categories),
            risk: max_risk(&risks),
            adapter_support: entry.map(|e| e.support),
            adapter_version,
            next_operations: next_ops,
        });
    }
    // Groups for unmanaged/unrecorded candidates with no record.
    for finding in findings {
        if finding.is_recorded {
            continue;
        }
        if groups.iter().any(|g| {
            g.instance.is_some()
                && registry
                    .get_by_id(g.instance.as_ref().map_or("", |i| i.as_str()))
                    .is_some_and(|inst| {
                        normalize_path(inst.config_root.as_path()) == normalize_path(&finding.path)
                    })
        }) {
            continue;
        }
        let Some(harness) = finding.fingerprint.harness.clone() else {
            continue;
        };
        let entry = crate::harness_catalog::find_by_id(harness.as_str());
        groups.push(DriftGroup {
            harness,
            instance: None,
            category: finding.category.clone(),
            risk: finding.risk,
            adapter_support: entry.map(|e| e.support),
            adapter_version: crate::harness_catalog::concrete_adapter_for(
                finding
                    .fingerprint
                    .harness
                    .as_ref()
                    .map_or(String::new(), |h| h.as_str().to_owned())
                    .as_str(),
            )
            .map(|adapter| {
                adapter
                    .version_resolution()
                    .detected_version
                    .unwrap_or_else(|| "version unknown".to_owned())
            }),
            next_operations: finding.next_operations.clone(),
        });
    }
    groups
}

/// Pick the category a group is summarized under: the most severe, by a
/// fixed severity order, so a healthy group with one broken wrapper still
/// surfaces the break.
fn pick_primary_category(categories: &[DriftCategory]) -> DriftCategory {
    let order = [
        DriftCategory::ForeignManaged,
        DriftCategory::AmbiguousOwnership,
        DriftCategory::DuplicateRoot,
        DriftCategory::DuplicateWrapper,
        DriftCategory::RecordedConfigMissing,
        DriftCategory::RecordedBinaryMissing,
        DriftCategory::RecordedWrapperMissing,
        DriftCategory::WrapperChanged,
        DriftCategory::RecordedVersionUnsupported,
        DriftCategory::FixedPathProfileInactive,
        DriftCategory::DaemonStopped,
        DriftCategory::OrphanWrapper,
        DriftCategory::DefaultUnrecorded,
        DriftCategory::CandidateUnmanaged,
        DriftCategory::RecordedHealthy,
    ];
    for candidate in order {
        if categories.contains(&candidate) {
            return candidate;
        }
    }
    DriftCategory::RecordedHealthy
}

/// Detect duplicate config roots and duplicate wrapper commands among the
/// registry records (DRF drift categories).
fn detect_record_duplicates(registry: &Registry) -> Vec<(DriftCategory, RiskLevel, String)> {
    let mut out: Vec<(DriftCategory, RiskLevel, String)> = Vec::new();
    let instances = registry.instances();
    for (i, a) in instances.iter().enumerate() {
        for b in instances.iter().skip(i + 1) {
            if normalize_path(a.config_root.as_path()) == normalize_path(b.config_root.as_path()) {
                out.push((
                    DriftCategory::DuplicateRoot,
                    RiskLevel::High,
                    format!(
                        "instances {} and {} share config root {}",
                        a.name, b.name, a.config_root
                    ),
                ));
            }
            if let (Some(wa), Some(wb)) = (&a.wrapper, &b.wrapper)
                && wa.command_name.normalized() == wb.command_name.normalized()
            {
                out.push((
                    DriftCategory::DuplicateWrapper,
                    RiskLevel::High,
                    format!(
                        "instances {} and {} share wrapper command {}",
                        a.name, b.name, wa.command_name
                    ),
                ));
            }
        }
    }
    out
}

/// Produce a drift report for `home` against the current `registry`.
///
/// The report is read-only and contains no UI formatting. It records
/// the timestamp, scanned scope, fingerprint, ownership, foreign evidence,
/// whether each candidate is recorded, the drift category/risk/next
/// operations per finding, wrapper-directory findings (DRF-03), and
/// harness/instance groups (DRF-08).
pub fn drift_report(registry: &Registry, home: &Path) -> DriftReport {
    drift_report_with_options(
        registry,
        home,
        &ScanOptions {
            wrapper_dirs: default_wrapper_dirs(home),
            ..ScanOptions::default()
        },
    )
}

/// [`drift_report`] with explicit scan options (user roots, wrapper dirs).
pub fn drift_report_with_options(
    registry: &Registry,
    home: &Path,
    options: &ScanOptions,
) -> DriftReport {
    let scan = scan_with_diagnostics(home, options);
    let candidates = scan.candidates;
    let wrapper_findings = scan_wrapper_dirs(&options.wrapper_dirs, registry);
    let mut findings: Vec<DriftFinding> = Vec::new();
    for cand in &candidates {
        let fingerprint = fingerprint_candidate(cand);
        let foreign = is_foreign_managed(cand, Some(home));
        let ownership = classify_ownership(cand, registry, Some(home));
        let is_recorded = registry
            .instances()
            .iter()
            .any(|i| normalize_path(i.config_root.as_path()) == normalize_path(cand));
        let (category, risk, next_operations) =
            classify_finding(is_recorded, cand, &foreign, &fingerprint, registry, home);
        findings.push(DriftFinding {
            path: cand.clone(),
            fingerprint,
            ownership,
            foreign,
            is_recorded,
            category,
            risk,
            next_operations,
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
                ambiguous: false,
            };
            findings.push(DriftFinding {
                path: root.to_path_buf(),
                fingerprint,
                ownership: inst.ownership,
                foreign,
                is_recorded: true,
                category: DriftCategory::RecordedConfigMissing,
                risk: RiskLevel::Medium,
                next_operations: vec![
                    "repair: recreate the config root (INS-09)".to_owned(),
                    "detach: drop the record if the root is gone for good".to_owned(),
                ],
            });
        }
    }

    // Duplicate records (DRF drift categories).
    let duplicates = detect_record_duplicates(registry);
    for (category, risk, description) in &duplicates {
        let target = registry.instances().first().map_or_else(
            || home.to_path_buf(),
            |i| i.config_root.as_path().to_path_buf(),
        );
        findings.push(DriftFinding {
            path: target,
            fingerprint: Fingerprint {
                harness: None,
                confidence: Confidence::None,
                evidence: vec![description.clone()],
            },
            ownership: Ownership::Unmanaged,
            foreign: ForeignCheck {
                is_foreign: false,
                owner: None,
                evidence: Vec::new(),
                ambiguous: false,
            },
            is_recorded: true,
            category: category.clone(),
            risk: *risk,
            next_operations: vec![
                "resolve: rename or remove one of the colliding records".to_owned(),
            ],
        });
    }

    let reconciliations = reconcile(registry, &candidates, &wrapper_findings);
    let groups = build_groups(registry, &findings, &wrapper_findings, &reconciliations);

    DriftReport {
        scanned_at: now_iso8601(),
        home: home.to_path_buf(),
        candidates,
        findings,
        wrapper_findings,
        groups,
        permission_diagnostics: scan.permission_diagnostics,
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

/// Minimum fingerprint confidence adoption requires.
///
/// [`Confidence::Medium`] is the lowest level that requires a canonical config
/// file to be present: `fingerprint_candidate` only assigns `Medium` or
/// `High` inside a canonical-file branch, while [`Confidence::Low`] is
/// name-pattern only and [`Confidence::None`] is no evidence at all. DRF-02
/// forbids a directory name alone from establishing harness identity, so
/// `Low` can never satisfy this floor.
pub const ADOPTION_CONFIDENCE_FLOOR: Confidence = Confidence::Medium;

/// Whether `confidence` carries more than a name-pattern match.
fn meets_adoption_floor(confidence: Confidence) -> bool {
    matches!(confidence, Confidence::High | Confidence::Medium)
}

/// Validate that a candidate can be adopted.
///
/// Checks: harness fingerprint at or above
/// [`ADOPTION_CONFIDENCE_FLOOR`] (a canonical config file must prove the
/// harness — a directory name alone never does), a readable canonical config
/// file (so the preview→commit conflict token is enforceable rather than
/// vacuously empty), foreign ownership, and a fresh readable candidate.
/// Returns the fingerprint on success.
/// Never copies, migrates, normalizes, or reformats the harness config.
pub fn can_adopt(candidate: &Path, home: Option<&Path>) -> Result<Fingerprint> {
    let fingerprint = fingerprint_candidate(candidate);
    if !meets_adoption_floor(fingerprint.confidence) {
        return Err(CoreError::InsufficientEvidence {
            path: candidate.to_path_buf(),
            required: ADOPTION_CONFIDENCE_FLOOR.to_string(),
            observed: fingerprint.confidence.to_string(),
            evidence: fingerprint.evidence,
        });
    }
    // A Medium+ fingerprint implies a canonical file exists; it must also be
    // READABLE, or the digest token adoption compares between preview and
    // commit would be empty and that check would pass vacuously.
    if canonical_config_digests(candidate).is_empty() {
        return Err(CoreError::InsufficientEvidence {
            path: candidate.to_path_buf(),
            required: format!(
                "{ADOPTION_CONFIDENCE_FLOOR} confidence with a readable canonical config file"
            ),
            observed: format!(
                "{} confidence with no readable canonical config file",
                fingerprint.confidence
            ),
            evidence: fingerprint.evidence,
        });
    }
    let foreign = is_foreign_managed(candidate, home);
    if foreign.is_foreign {
        return Err(CoreError::ForeignOwnership {
            path: candidate.to_path_buf(),
            owner: foreign.owner.unwrap_or_else(|| "foreign".to_owned()),
        });
    }
    // DRF-04: ambiguous evidence blocks adopt — it never silently resolves
    // to unmanaged.
    if foreign.ambiguous {
        return Err(CoreError::AmbiguousOwnership {
            path: candidate.to_path_buf(),
            evidence: foreign.evidence,
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

/// Canonical config file names adoption uses as its conflict token.
///
/// These are the readable harness files [`fingerprint_candidate`] proves
/// identity from — never a secret store — so a digest over exactly this set
/// is the minimal token that says "the proof still stands".
const ADOPTION_TOKEN_FILES: &[&str] = &[
    "settings.json",
    "config.toml",
    "opencode.json",
    "opencode.jsonc",
    ".aider.conf.yml",
    "aider.conf.yml",
    ".aider.model.metadata.json",
];

/// Fresh digests of a candidate's canonical config files.
///
/// Returns one `(file name, digest)` pair per canonical file that is present
/// and readable, in [`ADOPTION_TOKEN_FILES`] order. Never reads a secret
/// store. Adoption compares this set between preview and commit: the same
/// names with the same digests mean the fingerprint proof still holds for the
/// bytes it was proven on. A canonical file that exists but cannot be read
/// contributes no pair (its content was never part of the proof either).
pub fn canonical_config_digests(candidate: &Path) -> Vec<(String, String)> {
    let mut tokens: Vec<(String, String)> = Vec::new();
    for name in ADOPTION_TOKEN_FILES {
        let snap = superai_config::snapshot::snapshot(&candidate.join(name));
        if let Some(digest) = snap.digest {
            tokens.push(((*name).to_owned(), digest));
        }
    }
    tokens
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

    /// Platform: Linux, macOS, Windows — `scan_candidate_roots` via XDG `~/.config` and home dotfiles; Linux/macOS use `/home/...`, Windows uses `C:\Users\...` via `AbsolutePath::expand_home`. Finds `.claude-*`, `.codex`, `.aider` on all.
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

    /// DRF-04: orchestrator-managed workspace roots (Vibe Kanban /
    /// Conductor / Sculptor per docs/harness-configs/orchestrators.md) are
    /// classified foreign-owned with the orchestrator named, and adoption is
    /// refused — superai never takes over another manager's workspace.
    #[test]
    fn orchestrator_workspaces_are_foreign_managed_and_block_adoption() {
        let home = tmp_home("orchestrator_foreign");

        // Structural markers: the candidate lives under a documented
        // orchestrator workspace root.
        let cases = [
            (
                home.join("repo")
                    .join(".vibe-kanban-workspaces")
                    .join("vk-abc"),
                "vibe-kanban",
            ),
            (
                home.join("conductor").join("workspaces").join("task-1"),
                "conductor",
            ),
            (
                home.join(".sculptor")
                    .join("workspaces")
                    .join("w7")
                    .join("code"),
                "sculptor",
            ),
        ];
        for (candidate, owner) in &cases {
            std::fs::create_dir_all(candidate).unwrap();
            // settings.json gives the adoption floor's Medium fingerprint so
            // the foreign check is actually reached.
            std::fs::write(candidate.join("settings.json"), "{}").unwrap();
            let foreign = is_foreign_managed(candidate, Some(&home));
            assert!(foreign.is_foreign, "{owner}: must be foreign: {foreign:?}");
            assert_eq!(foreign.owner.as_deref(), Some(*owner), "{foreign:?}");
            assert!(!foreign.ambiguous);
            let err = can_adopt(candidate, Some(&home)).unwrap_err();
            match err {
                CoreError::ForeignOwnership {
                    path, owner: named, ..
                } => {
                    assert_eq!(path, *candidate);
                    assert_eq!(named, *owner);
                }
                other => panic!("{owner}: expected ForeignOwnership, got {other:?}"),
            }
        }

        // Conductor user settings referencing the candidate: foreign even
        // though the path itself carries no marker component.
        let referenced = home.join(".claude-from-conductor");
        std::fs::create_dir_all(&referenced).unwrap();
        std::fs::write(referenced.join("settings.json"), "{}").unwrap();
        let conductor_home = home.join(".conductor");
        std::fs::create_dir_all(&conductor_home).unwrap();
        let settings = conductor_home.join("settings.toml");
        std::fs::write(
            &settings,
            format!("[[workspaces]]\npath = '{}'\n", referenced.display()),
        )
        .unwrap();
        let foreign = is_foreign_managed(&referenced, Some(&home));
        assert!(foreign.is_foreign, "{foreign:?}");
        assert_eq!(foreign.owner.as_deref(), Some("conductor"));
        std::fs::remove_file(&settings).unwrap_or(());

        // Negative: an ordinary unmanaged root is neither foreign nor ambiguous.
        let plain = home.join(".claude-plain");
        std::fs::create_dir_all(&plain).unwrap();
        let clean = is_foreign_managed(&plain, Some(&home));
        assert!(!clean.is_foreign, "{clean:?}");
        assert!(!clean.ambiguous, "{clean:?}");
    }

    /// Platform: Linux/macOS — dedup by `(dev, ino)` via `MetadataExt` for symlinked roots; Windows — lexical dedup (no `MetadataExt`), hardlinks/junctions not resolved. Test asserts one entry on each via `#[cfg(unix)]`/`#[cfg(not(unix))]`.
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
            let candidates = vec![real.clone(), real];
            let deduped = deduplicate_by_identity(candidates);
            assert_eq!(deduped.len(), 1);
        }
    }

    /// Platform: all — bounded scan `scan_candidate_roots_limited` prevents huge tree traversal on Linux, macOS, and Windows by capping entries before deduplication.
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

    /// Platform: all — fingerprint uses multiple signals (exists, size, mtime) independent of OS; Windows mtime granularity differs but still deterministic.
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

    /// Platform: all — ownership classification via registry and foreign marker (`claude-multi`) is FS-agnostic; Windows path case-insensitivity handled via lexical compare, not OS case folding.
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

        let missing_path = crate::test_util::tmp_abs("disc-missing-parent")
            .join("superai-missing-does-not-exist-zzz");
        let own4 = classify_ownership(&missing_path, &reg, Some(&home));
        assert_eq!(own4, Ownership::Detached);
    }

    #[test]
    fn drift_report_covers_missing_config() {
        let home = tmp_home("drift_missing");
        let missing_root = crate::test_util::tmp_abs("drift-missing-parent")
            .join("superai-drift-missing-config-root")
            .to_string_lossy()
            .into_owned();
        let mut reg = Registry::default();
        reg.insert(sample_instance(
            "missing",
            &missing_root,
            "id-drift-1",
            Ownership::SuperaiCreated,
        ))
        .unwrap();
        let report = drift_report(&reg, &home);
        let found = report
            .findings
            .iter()
            .find(|f| f.path.as_path() == Path::new(&missing_root));
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

    /// Platform: all — wiring is pure pattern-string assembly, no FS access.
    /// Every catalog harness must resolve to its concrete adapter and every
    /// adapter-specific candidate must be part of the discovery pattern set
    /// (HAD-09/10/11): a future catalog row without a concrete adapter fails
    /// here instead of silently falling back to generic `~/.<id>` hints.
    #[test]
    fn scan_patterns_cover_every_catalog_concrete_adapter() {
        let patterns = candidate_patterns();
        for entry in crate::harness_catalog::all_entries() {
            let adapter = crate::harness_catalog::concrete_adapter_for(entry.id)
                .unwrap_or_else(|| {
                    panic!(
                        "catalog id `{}` has no concrete adapter; discovery would silently fall back to generic `~/.{}` candidates",
                        entry.id, entry.id
                    )
                });
            for candidate in adapter.scan_candidates() {
                assert!(
                    patterns.contains(&candidate),
                    "discovery patterns must include `{candidate}` from adapter `{}`",
                    entry.id
                );
            }
        }
    }

    /// Platform: all — `~/.config/warp-terminal/cli/settings.toml` under a temp
    /// home; tilde expansion via `expand_tilde` is uniform.
    /// A previously generic-only harness now gets adapter-specific candidates:
    /// warp's CLI settings file is reachable only through
    /// `WarpAdapter::scan_candidates` — the generic `~/.<id>` fallback never
    /// listed it, no known home prefix matches, and the XDG crawl ignores
    /// `warp-terminal`.
    #[test]
    fn scan_finds_adapter_specific_warp_candidate() {
        let home = tmp_home("scan_warp_cli");
        let settings = home
            .join(".config")
            .join("warp-terminal")
            .join("cli")
            .join("settings.toml");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings, "model = \"sonnet\"\n").unwrap();
        let candidates = scan_candidate_roots(&home);
        assert!(
            candidates.iter().any(|p| p == &settings),
            "scan must find warp's adapter-specific settings via scan_candidates, got {candidates:?}"
        );
    }

    /// swe-agent's project-layout candidate `config/default.yaml` is likewise
    /// adapter-specific: the generic fallback only ever offered
    /// `~/.swe-agent`, and no known home prefix matches `config`.
    #[test]
    fn scan_finds_adapter_specific_swe_agent_candidate() {
        let home = tmp_home("scan_swe_agent");
        let cfg = home.join("config").join("default.yaml");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, "model:\n  name: sonnet\n").unwrap();
        let candidates = scan_candidate_roots(&home);
        assert!(
            candidates.iter().any(|p| p == &cfg),
            "scan must find swe-agent's config/default.yaml via scan_candidates, got {candidates:?}"
        );
    }

    /// DRF-01: user-specified scan roots are honored and adapter glob
    /// patterns are EXPANDED (bounded, single level) instead of dropped.
    #[test]
    fn user_scan_roots_and_glob_patterns_are_scanned() {
        let home = tmp_home("scan_user_roots");
        // A user-specified root outside every known pattern.
        let custom = home.join("customs").join("harness-config");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(custom.join("settings.json"), r#"{"model":"x"}"#).unwrap();
        // A glob candidate the adapter corpus really declares (swe-agent
        // and trae-agent): `trajectories/*` — previously dropped by
        // `contains('*') { continue }`.
        let workspace = home.join("trajectories").join("run-1");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("traj.json"), "{}\n").unwrap();

        let options = ScanOptions {
            extra_roots: vec![custom.clone()],
            ..ScanOptions::default()
        };
        let report = scan_with_diagnostics(&home, &options);
        assert!(
            report.candidates.contains(&custom),
            "user-specified root must be scanned: {:?}",
            report.candidates
        );
        assert!(
            report.candidates.contains(&workspace),
            "glob pattern trajectories/* must expand to it: {:?}",
            report.candidates
        );
    }

    /// DRF-02: version marker and binary adjacency are fingerprint signals —
    /// the marker promotes a canonical-file match to High, the PATH lookup
    /// adds evidence without executing anything, and a name pattern alone
    /// still never rises above Low.
    #[test]
    fn fingerprint_uses_version_marker_and_binary_adjacency() {
        let home = tmp_home("fp_signals");
        let root = home.join(".claude-fp");
        std::fs::create_dir_all(&root).unwrap();
        // A canonical settings file with no schema marker: Medium alone.
        std::fs::write(root.join("settings.json"), "{}").unwrap();
        let medium = fingerprint_candidate(&root);
        assert_eq!(medium.confidence, Confidence::Medium);
        // Adding the install-era version marker corroborates: High.
        std::fs::write(root.join("version.txt"), "2.0.14\n").unwrap();
        let high = fingerprint_candidate(&root);
        assert_eq!(high.confidence, Confidence::High);
        assert!(
            high.evidence.iter().any(|e| e.contains("version marker")),
            "evidence: {:?}",
            high.evidence
        );

        // PATH adjacency: pure lookup over a synthetic PATH string.
        let bin_dir = home.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        // Windows executability is extension-based, so the fixture binary
        // carries a PATHEXT extension there and the lookup finds it through
        // the extension probe.
        let claude_name = if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        };
        let claude = bin_dir.join(claude_name);
        std::fs::write(&claude, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&claude).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&claude, perms).unwrap();
        }
        // PATH entries are ';'-separated on Windows; mirror the platform
        // splitter used by `binary_on_path`.
        let sep = if cfg!(windows) { ';' } else { ':' };
        let path_var = format!(
            "{}{sep}{}",
            bin_dir.display(),
            Path::new("/nonexistent").display()
        );
        assert_eq!(
            binary_on_path(&path_var, "claude"),
            Some(claude),
            "adjacency must find the binary by lookup"
        );
        assert_eq!(binary_on_path(&path_var, "codex"), None);
        // Non-executable files never count: an extensionless data file is
        // not executable on Windows either.
        let plain = bin_dir.join("plain-tool");
        std::fs::write(&plain, "data").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&plain).unwrap().permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(&plain, perms).unwrap();
        }
        assert_eq!(binary_on_path(&path_var, "plain-tool"), None);
    }

    /// DRF-03/04: the wrapper-directory scan classifies superai wrappers,
    /// records, orphans, package-manager shims, and foreign launchers — and
    /// never executes anything.
    #[test]
    fn wrapper_dir_scan_classifies_shims_wrappers_and_orphans() {
        let home = tmp_home("scan_wrapper_dirs");
        let bin = home.join(".local").join("bin");
        std::fs::create_dir_all(&bin).unwrap();

        // A superai wrapper whose instance is recorded.
        let recorded_inst = sample_instance(
            "recorded",
            &crate::test_util::tmp_abs_str(".claude-recorded"),
            "id-scan-rec",
            Ownership::SuperaiCreated,
        );
        let mut registry = Registry::default();
        registry.insert(recorded_inst).unwrap();
        // An orphan superai wrapper for an unrecorded instance.
        let orphan_inst = sample_instance(
            "orphaned",
            &crate::test_util::tmp_abs_str(".claude-orphaned"),
            "id-scan-orph",
            Ownership::SuperaiCreated,
        );

        let make_wrapper = |inst: &Instance, file: &str| {
            let mut plan = crate::adapter::WrapperPlan::new("test");
            plan.env_vars
                .push(("CLAUDE_CONFIG_DIR".to_owned(), inst.config_root.to_string()));
            let (content, _) = crate::wrapper::generate_shell_wrapper(inst, &plan);
            std::fs::write(bin.join(file), content).unwrap();
        };
        make_wrapper(registry.get("recorded").unwrap(), "recorded-tool");
        make_wrapper(&orphan_inst, "orphan-tool");

        // A mise shim.
        std::fs::write(
            bin.join("some-tool"),
            "#!/bin/sh\nexec mise x -- node \"$@\"\n",
        )
        .unwrap();

        // A foreign user wrapper (known recipe, no marker).
        std::fs::write(
            bin.join("user-tool"),
            format!(
                "#!/bin/sh\nexport CLAUDE_CONFIG_DIR='{}'\nexec claude \"$@\"\n",
                crate::test_util::tmp_abs_str(".claude-user")
            ),
        )
        .unwrap();

        let findings = scan_wrapper_dirs(std::slice::from_ref(&bin), &registry);
        let by_path = |name: &str| {
            findings
                .iter()
                .find(|f| f.path == bin.join(name))
                .unwrap_or_else(|| panic!("no finding for {name}: {findings:?}"))
        };

        let recorded = by_path("recorded-tool");
        assert!(matches!(
            recorded.kind,
            WrapperFindingKind::SuperaiWrapper { .. }
        ));
        assert!(recorded.recorded);
        assert_eq!(recorded.risk, RiskLevel::Info);

        let orphan = by_path("orphan-tool");
        assert!(matches!(
            orphan.kind,
            WrapperFindingKind::SuperaiWrapper { .. }
        ));
        assert!(!orphan.recorded, "unrecorded wrapper is an orphan");
        assert_eq!(orphan.risk, RiskLevel::Medium);
        assert!(
            orphan
                .next_operations
                .iter()
                .any(|op| op.contains("adopt_orphan_wrapper")),
            "{:?}",
            orphan.next_operations
        );

        let shim = by_path("some-tool");
        assert_eq!(
            shim.kind,
            WrapperFindingKind::PackageShim {
                manager: "mise".to_owned()
            },
            "mise shims are distinguished from wrappers (DRF-04)"
        );

        let foreign = by_path("user-tool");
        assert!(matches!(foreign.kind, WrapperFindingKind::Foreign { .. }));
    }

    /// DRF-04: ambiguous ownership evidence (a foreign manager is present but
    /// links nothing) BLOCKS adoption instead of silently resolving to
    /// unmanaged.
    #[test]
    fn ambiguous_ownership_blocks_adoption() {
        let home = tmp_home("ambiguous_blocks");
        let candidate = home.join(".claude-amb");
        std::fs::create_dir_all(&candidate).unwrap();
        std::fs::write(candidate.join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        // A claude-multi installation exists but never links this candidate.
        std::fs::create_dir_all(home.join(".claude-multi")).unwrap();

        let check = is_foreign_managed(&candidate, Some(&home));
        assert!(!check.is_foreign);
        assert!(check.ambiguous, "evidence: {:?}", check.evidence);

        match can_adopt(&candidate, Some(&home)) {
            Err(CoreError::AmbiguousOwnership { path, .. }) => {
                assert_eq!(path, candidate);
            }
            other => panic!("expected AmbiguousOwnership, got {other:?}"),
        }
        assert!(
            !home.join(".superai").exists() || std::fs::read_dir(home.join(".superai")).is_err(),
            "refused adoption must not write records"
        );
    }

    /// DRF-05: reconciliation matches the `InstanceId` marker FIRST — a moved
    /// root still matches its record before any path comparison — and never
    /// merges on the display name.
    #[test]
    fn reconcile_matches_marker_first() {
        let home = tmp_home("reconcile_marker");
        let original = home.join(".claude-work");
        std::fs::create_dir_all(&original).unwrap();
        std::fs::write(original.join("settings.json"), r#"{"model":"x"}"#).unwrap();

        let inst = sample_instance(
            "work",
            &original.to_string_lossy(),
            "id-recon-1",
            Ownership::SuperaiCreated,
        );
        let marker_id = inst.id.clone();
        let mut registry = Registry::default();
        registry.insert(inst).unwrap();

        // The root moved on disk but kept the superai marker.
        let moved = home.join(".claude-moved");
        std::fs::create_dir_all(&moved).unwrap();
        std::fs::write(moved.join("settings.json"), r#"{"model":"x"}"#).unwrap();
        std::fs::write(moved.join(INSTANCE_MARKER_FILE), format!("{marker_id}\n")).unwrap();

        let rows = reconcile(&registry, std::slice::from_ref(&moved), &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].candidate.as_deref(), Some(moved.as_path()));
        assert_eq!(rows[0].basis, Some(MatchBasis::InstanceMarker));

        // read_instance_marker round-trips the id.
        assert_eq!(read_instance_marker(&moved), Some(marker_id));

        // Without the marker, the same mismatched paths do NOT match.
        std::fs::remove_file(moved.join(INSTANCE_MARKER_FILE)).unwrap();
        let rows2 = reconcile(&registry, std::slice::from_ref(&moved), &[]);
        assert!(
            rows2[0].candidate.is_none(),
            "no marker and no path equality means no match: {:?}",
            rows2[0]
        );
    }

    /// DRF-08: the drift report groups findings by harness/instance with risk
    /// levels, adapter support/version, and recommended next operations.
    #[test]
    fn drift_report_groups_by_harness_with_risk_and_next_ops() {
        let home = tmp_home("drift_groups");
        let root = home.join(".claude-work");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("settings.json"), r#"{"model":"x"}"#).unwrap();

        // A recorded instance whose wrapper file is MISSING.
        let mut inst = sample_instance(
            "work",
            &root.to_string_lossy(),
            "id-group-1",
            Ownership::SuperaiCreated,
        );
        inst.wrapper = Some(crate::instance::WrapperRef {
            path: crate::paths::WrapperPath::new(&home.join("bin").join("work").to_string_lossy())
                .unwrap(),
            command_name: InstanceName::new("work").unwrap(),
            generator_version: "0.1.0".to_owned(),
            content_digest: "abc".to_owned(),
        });
        let mut registry = Registry::default();
        registry.insert(inst).unwrap();

        let report = drift_report(&registry, &home);
        let group = report
            .groups
            .iter()
            .find(|g| g.harness.as_str() == "claude-code")
            .expect("a claude-code group exists");
        assert_eq!(group.instance, Some(InstanceId::new("id-group-1").unwrap()));
        assert_eq!(group.category, DriftCategory::RecordedWrapperMissing);
        assert_eq!(group.risk, RiskLevel::Medium);
        assert!(group.adapter_support.is_some(), "adapter support surfaced");
        assert!(
            group.next_operations.iter().any(|op| op.contains("repair")),
            "next ops: {:?}",
            group.next_operations
        );

        // The finding carries the category too.
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.category == DriftCategory::RecordedWrapperMissing),
            "findings: {:?}",
            report
                .findings
                .iter()
                .map(|f| f.category.clone())
                .collect::<Vec<_>>()
        );
    }
}
