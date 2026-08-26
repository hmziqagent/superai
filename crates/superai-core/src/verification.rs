//! Verification harness for plan 13 gates: fixture loading, secret-free checks, platform gates.
//!
//! Interface-neutral, no GPUI types. Every check reads fresh from disk and
//! preserves unmodelled keys. Helpers are deterministic and parallel-safe.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use superai_config::document::{Diagnostic, DocumentKind, SourceDocument};
use superai_config::raw_editor::{find_redaction_spans, validate};

use crate::adapter::{Adapter, Arch, Os, Platform};
use crate::harness_catalog;

// ---------------------------------------------------------------------------
// Secret-free logic
// ---------------------------------------------------------------------------

/// Markers that indicate a credential is intentionally fake and allowed in fixtures.
const FAKE_MARKERS: &[&str] = &[
    "test",
    "fake",
    "example",
    "placeholder",
    "dummy",
    "not-real",
    "not_real",
    "xxx",
    "sk-test",
    "sk_fake",
    "harness_test",
    "superai-test",
];

fn contains_fake_marker(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    FAKE_MARKERS
        .iter()
        .any(|m| lower.contains(&m.to_ascii_lowercase()))
}

/// Whether `text` contains a real-looking secret value.
///
/// Scans via `find_redaction_spans`; a span is considered real only when the
/// extracted value does not contain any `FAKE_MARKERS`. Binary or empty is
/// considered secret-free. Case-insensitive key detection is delegated to the
/// raw editor's redaction logic.
#[expect(clippy::manual_let_else, reason = "explicit match clearer")]
pub fn contains_real_secret(content: &[u8], kind: DocumentKind) -> bool {
    let spans = find_redaction_spans(content, kind);
    if spans.is_empty() {
        return false;
    }
    let text = match std::str::from_utf8(content) {
        Ok(s) => s,
        Err(_) => return false,
    };
    for span in spans {
        let Some(slice) = text.get(span.start..span.end) else {
            continue;
        };
        // Trim quotes and whitespace for marker check
        let trimmed = slice.trim().trim_matches('"').trim_matches('\'').trim();
        if !contains_fake_marker(trimmed) && !trimmed.is_empty() {
            // Value looks real: no fake marker, non-empty secret-bearing span
            // Distinguish obviously short placeholder like "" or "x"
            if trimmed.len() >= 4 && !is_obviously_fake(trimmed) {
                return true;
            }
            // Even short non-fake could be real in fixtures — be conservative
            // but allow empty/redacted placeholders
            if !trimmed.eq_ignore_ascii_case("[REDACTED]") && trimmed.len() > 2 {
                return true;
            }
        }
    }
    false
}

fn is_obviously_fake(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower == "fake" || lower == "test" || lower == "example" || lower == "placeholder"
}

/// Whether `content` is secret-free (no real secret, fakes allowed).
pub fn is_secret_free_content(content: &[u8], kind: DocumentKind) -> bool {
    !contains_real_secret(content, kind)
}

/// Whether `content` is secret-free when treated as UTF-8 string.
/// Wrapper for text-only checks.
pub fn is_secret_free_str(text: &str) -> bool {
    // Heuristic: try each kind; if any considers it secret-free, pass.
    // For str we default to StrictJson scanning which covers json-like secrets.
    let bytes = text.as_bytes();
    // Check via generic scan (Env kind widest)
    !contains_real_secret(bytes, DocumentKind::StrictJson)
        && !contains_real_secret(bytes, DocumentKind::Env)
        && !contains_real_secret(bytes, DocumentKind::Yaml)
}

// ---------------------------------------------------------------------------
// Fixture loading and verification
// ---------------------------------------------------------------------------

/// Outcome of checking a single fixture file.
#[expect(
    clippy::struct_excessive_bools,
    reason = "outcome needs multiple flags"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureOutcome {
    /// Path that was checked.
    pub path: PathBuf,
    /// Detected document kind.
    pub kind: DocumentKind,
    /// Whether the file exists.
    pub exists: bool,
    /// Diagnostics from validation (empty means syntactically valid).
    pub diagnostics: Vec<Diagnostic>,
    /// Whether parsing succeeded (diagnostics empty).
    pub is_valid: bool,
    /// Whether the file is secret-free (no real secret).
    pub secret_free: bool,
    /// Whether the file was expected to be valid (based on name).
    pub expected_valid: bool,
}

/// Infer whether a fixture file is expected to be valid from its name.
/// Files containing "malformed" are expected to be invalid; others valid.
fn expected_valid_for_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    !name.contains("malformed")
}

/// Verify a single fixture file at `path`.
#[expect(clippy::manual_let_else, reason = "explicit match clearer")]
pub fn verify_fixture_file(path: &Path) -> FixtureOutcome {
    let kind = DocumentKind::from_path(path);
    let expected_valid = expected_valid_for_path(path);
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => {
            return FixtureOutcome {
                path: path.to_path_buf(),
                kind,
                exists: false,
                diagnostics: Vec::new(),
                is_valid: false,
                secret_free: true,
                expected_valid,
            };
        }
    };
    let diagnostics = validate(&bytes, kind);
    let secret_free = is_secret_free_content(&bytes, kind);
    // For text kind, also ensure UTF-8 validity via SourceDocument diagnostics
    let source = SourceDocument::from_bytes(path, bytes);
    let mut all_diagnostics = diagnostics;
    // Merge encoding diagnostics if any and not already present
    for d in &source.diagnostics {
        if !all_diagnostics.contains(d) {
            all_diagnostics.push(d.clone());
        }
    }
    let is_valid = all_diagnostics.is_empty();
    FixtureOutcome {
        path: path.to_path_buf(),
        kind,
        exists: true,
        diagnostics: all_diagnostics,
        is_valid,
        secret_free,
        expected_valid,
    }
}

/// Load a fixture fresh from disk as `SourceDocument`.
///
/// Returns `Err` for missing file; empty file yields `Ok` with zero bytes.
/// Diagnostics are included in the returned `SourceDocument`.
pub fn load_fixture(path: &Path) -> Result<SourceDocument, superai_config::ConfigError> {
    SourceDocument::load(path)
}

/// Verify all fixtures recursively under `dir`.
#[expect(clippy::manual_let_else, reason = "explicit match clearer")]
pub fn verify_fixtures_in_dir(dir: &Path) -> Vec<FixtureOutcome> {
    let mut outcomes = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return outcomes,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if let Ok(meta) = entry.metadata() {
            if meta.is_dir() {
                outcomes.extend(verify_fixtures_in_dir(&p));
            } else if meta.is_file() {
                outcomes.push(verify_fixture_file(&p));
            }
        }
    }
    outcomes
}

/// Aggregate report for a fixture directory scan.
#[derive(Debug, Clone)]
pub struct FixtureReport {
    /// Per-file outcomes.
    pub outcomes: Vec<FixtureOutcome>,
    /// Whether all fixtures matched expected validity.
    pub validity_pass: bool,
    /// Whether all fixtures are secret-free.
    pub secret_free_pass: bool,
    /// Count of valid fixtures.
    pub valid_count: usize,
    /// Count of invalid (malformed) fixtures correctly flagged.
    pub malformed_count: usize,
}

/// Build a report for fixtures under `dir`.
pub fn fixture_report(dir: &Path) -> FixtureReport {
    let outcomes = verify_fixtures_in_dir(dir);
    let mut validity_pass = true;
    let mut secret_free_pass = true;
    let mut valid_count = 0usize;
    let mut malformed_count = 0usize;
    for o in &outcomes {
        if o.is_valid {
            valid_count = valid_count.saturating_add(1);
        }
        if !o.expected_valid && !o.is_valid {
            malformed_count = malformed_count.saturating_add(1);
        }
        if o.exists && o.is_valid != o.expected_valid {
            validity_pass = false;
        }
        if !o.secret_free {
            secret_free_pass = false;
        }
    }
    FixtureReport {
        outcomes,
        validity_pass,
        secret_free_pass,
        valid_count,
        malformed_count,
    }
}

// ---------------------------------------------------------------------------
// Platform gates (QAL-08/09)
// ---------------------------------------------------------------------------

/// Verdict for a platform gate check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlatformVerdict {
    /// Current platform is explicitly supported.
    Supported,
    /// Current platform is not in the adapter's supported list.
    Unsupported,
    /// Adapter support is constrained on this platform (e.g., IDE user-data caveats).
    Constrained,
    /// No platform list declared; treat as unknown.
    Unknown,
}

impl std::fmt::Display for PlatformVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::Constrained => "constrained",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

/// Result of checking the current OS/arch against an adapter's supported platforms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformGate {
    /// Harness id that was checked.
    pub harness: String,
    /// Current platform derived from `std::env::consts`.
    pub current: Platform,
    /// Verdict.
    pub verdict: PlatformVerdict,
    /// Human reason.
    pub reason: String,
}

/// Determine the current host platform from `std::env::consts`.
pub fn current_platform() -> Platform {
    #[expect(clippy::match_same_arms, reason = "unknown OS defaults to Linux")]
    let os = match std::env::consts::OS {
        "linux" => Os::Linux,
        "macos" | "darwin" => Os::Macos,
        "windows" => Os::Windows,
        _ => Os::Linux,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" | "amd64" => Arch::X86_64,
        "aarch64" | "arm64" => Arch::Aarch64,
        _ => Arch::Any,
    };
    Platform::new(os, arch)
}

fn platform_matches(current: Platform, supported: Platform) -> bool {
    if current.os != supported.os {
        return false;
    }
    supported.arch == Arch::Any || current.arch == supported.arch || current.arch == Arch::Any
}

/// Check whether the current platform is supported by `adapter`.
pub fn platform_gate_for_adapter(adapter: &dyn Adapter) -> PlatformGate {
    let current = current_platform();
    let supported = adapter.supported_platforms();
    if supported.is_empty() {
        return PlatformGate {
            harness: adapter.id().to_string(),
            current,
            verdict: PlatformVerdict::Unknown,
            reason: "adapter declares no platforms".to_owned(),
        };
    }
    let mut matched = false;
    for p in &supported {
        if platform_matches(current, *p) {
            matched = true;
            break;
        }
    }
    let verdict = if matched {
        PlatformVerdict::Supported
    } else {
        PlatformVerdict::Unsupported
    };
    let reason = if matched {
        format!("current {current} is supported")
    } else {
        format!("current {current} not in supported list {supported:?}")
    };
    // Constrained could be refined per-adapter, but generic check is Supported/Unsupported
    PlatformGate {
        harness: adapter.id().to_string(),
        current,
        verdict,
        reason,
    }
}

/// Run platform gates for every catalog entry.
pub fn catalog_platform_gates() -> Vec<PlatformGate> {
    let mut gates = Vec::new();
    for entry in harness_catalog::ENTRIES {
        // Construct generic adapter for platform check; real adapters may override
        // but generic covers ledger-level gate.
        let Ok(id) = crate::ids::HarnessId::new(entry.id) else {
            continue;
        };
        let adapter = crate::adapter::GenericAdapter::new(
            id,
            entry.display_name,
            entry.product_status,
            entry.research_doc,
            entry.last_verified,
            entry.support,
            entry.reason,
            entry.source,
        );
        gates.push(platform_gate_for_adapter(&adapter));
    }
    gates
}

// ---------------------------------------------------------------------------
// Ledger coverage helpers (QAL-13)
// ---------------------------------------------------------------------------

/// Whether every harness entry has a fixture directory (best-effort check).
pub fn ledger_fixture_coverage(fixtures_root: &Path) -> Vec<(String, bool)> {
    let mut coverage = Vec::new();
    for entry in harness_catalog::ENTRIES {
        let dir = fixtures_root.join(entry.id.replace('-', "_"));
        // Also try hyphen vs underscore variants and original id
        let alt = fixtures_root.join(entry.id);
        let exists = dir.exists() || alt.exists();
        coverage.push((entry.id.to_owned(), exists));
    }
    coverage
}

// ---------------------------------------------------------------------------
// Isolated filesystem helper (QAL-01)
// ---------------------------------------------------------------------------

/// Create a unique temporary directory for a test, parallel-safe.
///
/// Uses `std::env::temp_dir` plus a nanosecond timestamp, pid, and `prefix`.
/// Returns the path; caller is responsible for cleanup. Platform path handling
/// is explicit; no global `HOME` mutation is performed.
pub fn unique_temp_dir(prefix: &str) -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("superai-verification-{prefix}-{now}-{pid}"));
    // Best-effort create; caller may check existence.
    drop(std::fs::create_dir_all(&dir));
    dir
}

/// Collect a set of obviously secret-bearing keys for scanning.
/// Used by QAL-10 sentinel injection tests.
pub fn secret_key_patterns() -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    for k in &[
        "api_key",
        "apikey",
        "secret",
        "token",
        "password",
        "bearer",
        "authorization",
        "auth",
    ] {
        s.insert((*k).to_owned());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::adapter::{DetectionResult, ProductStatus, VersionResolution};
    use crate::ids::HarnessId;
    use crate::state::AdapterSupport;

    #[expect(dead_code, reason = "helper for ad-hoc debugging")]
    fn unique_scratch(prefix: &str, suffix: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let pid = std::process::id();
        let dir = std::env::temp_dir().join("superai-verification-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{prefix}-{now}-{pid}{suffix}"))
    }

    #[derive(Debug)]
    struct DummyAdapter {
        id: &'static str,
        platforms: Vec<Platform>,
    }

    impl Adapter for DummyAdapter {
        fn id(&self) -> HarnessId {
            HarnessId::new(self.id).unwrap()
        }
        #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
        fn display_name(&self) -> &str {
            "Dummy"
        }
        fn product_status(&self) -> ProductStatus {
            ProductStatus::Active
        }
        fn supported_platforms(&self) -> Vec<Platform> {
            self.platforms.clone()
        }
        #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
        fn adapter_revision(&self) -> &str {
            "0.1.0"
        }
        #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
        fn research_doc_link(&self) -> &str {
            "https://example.com"
        }
        #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
        fn last_verified_date(&self) -> &str {
            "2026-01-01"
        }
        fn detection(&self) -> DetectionResult {
            DetectionResult::absent(vec!["test".to_owned()])
        }
        fn version_resolution(&self) -> VersionResolution {
            VersionResolution {
                detected_version: Some("1.0.0".to_owned()),
                schema_version: Some("1.0.0".to_owned()),
                compatible: true,
                notes: Vec::new(),
            }
        }
        fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
            Vec::new()
        }
        fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
            Vec::new()
        }
        fn plan_mirror_exclusions(&self) -> Vec<String> {
            Vec::new()
        }
        fn plan_wrapper(
            &self,
            _instance: &crate::instance::Instance,
        ) -> Result<crate::adapter::WrapperPlan, crate::error::CoreError> {
            Ok(crate::adapter::WrapperPlan::new("test"))
        }
        fn scan_candidates(&self) -> Vec<String> {
            Vec::new()
        }
        fn validate_instance(
            &self,
            _instance: &crate::instance::Instance,
        ) -> crate::error::Result<()> {
            Ok(())
        }
    }

    // ---- secret-free ----

    #[test]
    fn fake_credentials_are_secret_free() {
        let content = br#"{"api_key":"sk-test-fake-12345","model":"opus"}"#;
        assert!(
            is_secret_free_content(content, DocumentKind::StrictJson),
            "fake should be free"
        );
        assert!(!contains_real_secret(content, DocumentKind::StrictJson));
    }

    #[test]
    fn real_secret_is_detected() {
        let content = br#"{"api_key":"sk-live-abc123realvalue","model":"opus"}"#;
        assert!(!is_secret_free_content(content, DocumentKind::StrictJson));
        assert!(contains_real_secret(content, DocumentKind::StrictJson));
    }

    #[test]
    fn non_secret_is_free() {
        let content = br#"{"model":"opus","other":"value"}"#;
        assert!(is_secret_free_content(content, DocumentKind::StrictJson));
    }

    #[test]
    fn env_fake_is_secret_free() {
        let content = b"API_KEY=sk-test-fake-value\nMODEL=opus\n";
        assert!(is_secret_free_content(content, DocumentKind::Env));
    }

    #[test]
    fn env_real_is_not_free() {
        let content = b"API_KEY=sk-live-real-xyz-123\n";
        assert!(!is_secret_free_content(content, DocumentKind::Env));
    }

    #[test]
    fn yaml_fake_is_secret_free() {
        let content = b"api_key: fake-test-value\nmodel: opus\n";
        assert!(is_secret_free_content(content, DocumentKind::Yaml));
    }

    // ---- fixture loading ----

    #[test]
    fn fixture_minimal_json_loads_and_is_valid() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/claude_code");
        let path = root.join("settings.minimal.json");
        assert!(path.exists(), "fixture missing {}", path.display());
        let outcome = verify_fixture_file(&path);
        assert!(outcome.exists);
        assert!(
            outcome.is_valid,
            "minimal should be valid: {:?}",
            outcome.diagnostics
        );
        assert!(outcome.expected_valid);
        assert!(outcome.secret_free, "fixture should be secret-free");
    }

    #[test]
    fn fixture_malformed_is_invalid() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/claude_code");
        let path = root.join("settings.malformed.json");
        assert!(path.exists());
        let outcome = verify_fixture_file(&path);
        assert!(outcome.exists);
        assert!(!outcome.is_valid, "malformed should be invalid");
        assert!(!outcome.expected_valid);
    }

    #[test]
    fn fixture_populated_valid_and_secret_free() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/claude_code");
        let path = root.join("settings.populated.json");
        let outcome = verify_fixture_file(&path);
        assert!(outcome.is_valid);
        assert!(outcome.secret_free);
    }

    #[test]
    fn fixture_toml_minimal_valid() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/codex_cli");
        let path = root.join("config.minimal.toml");
        assert!(path.exists());
        let outcome = verify_fixture_file(&path);
        assert!(
            outcome.is_valid,
            "toml minimal should be valid: {:?}",
            outcome.diagnostics
        );
        assert!(outcome.secret_free);
    }

    #[test]
    fn fixture_yaml_minimal_valid() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/aider");
        let path = root.join("aider.minimal.yml");
        assert!(path.exists());
        let outcome = verify_fixture_file(&path);
        assert!(
            outcome.is_valid,
            "yaml minimal should be valid: {:?}",
            outcome.diagnostics
        );
        assert!(outcome.secret_free);
    }

    #[test]
    fn fixtures_in_dir_report_is_consistent() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/claude_code");
        let report = fixture_report(&root);
        assert!(
            report.validity_pass,
            "validity should pass for claude_code fixtures"
        );
        assert!(report.secret_free_pass, "fixtures should be secret-free");
        assert!(report.valid_count >= 2);
        assert!(report.malformed_count >= 1);
    }

    #[test]
    fn load_fixture_returns_source_document() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/claude_code");
        let path = root.join("settings.minimal.json");
        let doc = load_fixture(&path).unwrap();
        assert_eq!(doc.kind, DocumentKind::StrictJson);
        assert!(!doc.has_diagnostics());
    }

    // ---- platform gates ----

    #[test]
    fn current_platform_is_deterministic() {
        let a = current_platform();
        let b = current_platform();
        assert_eq!(a, b);
    }

    #[test]
    fn platform_gate_supported_when_current_in_list() {
        let current = current_platform();
        let adapter = DummyAdapter {
            id: "aider",
            platforms: vec![current],
        };
        let gate = platform_gate_for_adapter(&adapter);
        assert_eq!(gate.verdict, PlatformVerdict::Supported);
        assert_eq!(gate.harness, "aider");
    }

    #[test]
    fn platform_gate_unsupported_when_not_in_list() {
        // Pick a platform that is not current: if current is Linux, use Windows
        let current = current_platform();
        let other_os = match current.os {
            Os::Linux => Os::Windows,
            Os::Windows => Os::Macos,
            Os::Macos => Os::Linux,
        };
        let other = Platform::new(other_os, Arch::Any);
        let adapter = DummyAdapter {
            id: "aider",
            platforms: vec![other],
        };
        let gate = platform_gate_for_adapter(&adapter);
        assert_eq!(gate.verdict, PlatformVerdict::Unsupported);
    }

    #[test]
    fn platform_gate_unknown_when_empty() {
        let adapter = DummyAdapter {
            id: "aider",
            platforms: Vec::new(),
        };
        let gate = platform_gate_for_adapter(&adapter);
        assert_eq!(gate.verdict, PlatformVerdict::Unknown);
    }

    #[test]
    fn catalog_gates_cover_all_entries() {
        let gates = catalog_platform_gates();
        assert_eq!(gates.len(), harness_catalog::ENTRIES.len());
        // At least one gate should be supported on this host (generic has all platforms)
        assert!(
            gates
                .iter()
                .any(|g| g.verdict == PlatformVerdict::Supported)
        );
    }

    // ---- ledger coverage ----

    #[test]
    fn ledger_coverage_has_known_harnesses() {
        let fixtures_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let coverage = ledger_fixture_coverage(&fixtures_root);
        assert_eq!(coverage.len(), harness_catalog::ENTRIES.len());
        // claude_code should have fixtures
        let claude = coverage.iter().find(|(id, _)| *id == "claude-code");
        assert!(claude.is_some());
        assert!(claude.unwrap().1, "claude-code should have fixture dir");
    }

    // ---- isolated temp dir ----

    #[test]
    fn unique_temp_dir_is_created() {
        let dir = unique_temp_dir("test-iso");
        assert!(dir.exists());
        assert!(dir.is_dir());
        drop(std::fs::remove_dir_all(&dir));
    }

    // ---- secret key patterns ----

    #[test]
    fn secret_patterns_include_expected_keys() {
        let patterns = secret_key_patterns();
        assert!(patterns.contains("api_key"));
        assert!(patterns.contains("token"));
        assert!(patterns.contains("password"));
    }

    // ---- raw_editor kind helpers via verification ----

    #[test]
    fn validate_each_kind_via_raw_editor() {
        let cases: Vec<(&[u8], DocumentKind, bool)> = vec![
            (br#"{"a":1}"#, DocumentKind::StrictJson, true),
            (b"{ invalid json }", DocumentKind::StrictJson, false),
            (br#"{"a":1,} // comment"#, DocumentKind::JsonC, true),
            (b"a = 1\n", DocumentKind::Toml, true),
            (b"a = [\n", DocumentKind::Toml, false),
            (b"a: 1\n", DocumentKind::Yaml, true),
            (b"a: [unclosed\n", DocumentKind::Yaml, false),
            (b"API_KEY=val\n", DocumentKind::Env, true),
            (b"INVALID LINE\n", DocumentKind::Env, false),
            (b"just text fragment\n", DocumentKind::TextFragment, true),
        ];
        for (content, kind, should_be_valid) in cases {
            let diags = validate(content, kind);
            assert_eq!(
                diags.is_empty(),
                should_be_valid,
                "kind {kind:?} content {:?}",
                String::from_utf8_lossy(content)
            );
        }
    }

    #[test]
    fn invalid_rejection_would_be_blocked_by_validate() {
        // Simulate what commit would do: validate before write
        let invalid = b"{ bad json }";
        let diags = validate(invalid, DocumentKind::StrictJson);
        assert!(
            !diags.is_empty(),
            "invalid json must be caught before commit"
        );
        // Also for yaml
        let invalid_yaml = b"a: [unclosed\n";
        let diags_y = validate(invalid_yaml, DocumentKind::Yaml);
        assert!(!diags_y.is_empty());
    }

    #[test]
    fn redaction_spans_cover_all_secret_keys() {
        let keys = [
            "api_key",
            "apiKey",
            "secret",
            "token",
            "password",
            "bearer",
            "authorization",
        ];
        for key in keys {
            let content = format!("{{\"{key}\":\"super-secret-value\"}}");
            let spans = find_redaction_spans(content.as_bytes(), DocumentKind::StrictJson);
            assert!(!spans.is_empty(), "key {key} should be redacted");
        }
    }

    #[test]
    fn redaction_is_kind_aware() {
        // Env
        let env = b"SECRET_TOKEN=realvalue\n";
        let spans = find_redaction_spans(env, DocumentKind::Env);
        assert!(!spans.is_empty());
        // Toml
        let toml = b"api_key = \"secret123\"\n";
        let spans_t = find_redaction_spans(toml, DocumentKind::Toml);
        assert!(!spans_t.is_empty());
        // Yaml
        let yaml = b"password: mysecret\n";
        let spans_y = find_redaction_spans(yaml, DocumentKind::Yaml);
        assert!(!spans_y.is_empty());
    }
}
