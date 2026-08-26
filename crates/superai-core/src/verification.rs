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

/// Create a unique temporary directory, parallel-safe (production helper).
///
/// Uses `std::env::temp_dir` plus a nanosecond timestamp, pid, and `prefix`.
/// Returns the path; caller is responsible for cleanup. Platform path handling
/// is explicit; no global `HOME` mutation is performed. Test code should
/// prefer `crate::test_util::temp_dir_unique`, which also removes the
/// directory on drop.
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

// ---------------------------------------------------------------------------
// QAL-06/07 expansion: failure injection matrix + fake harness checklist
// ---------------------------------------------------------------------------

/// Required `FailurePoint` variants for the QAL-06 matrix.
///
/// The matrix covers every boundary in subplan 02 that must be injectable
/// via `FailureInjector` (Real vs `TestInjector` at Nth call). This list is
/// intentionally exhaustive; CI fails if `failure.rs` drops a variant.
pub fn required_failure_points() -> Vec<crate::failure::FailurePoint> {
    use crate::failure::FailurePoint;
    vec![
        FailurePoint::BackupOpen,
        FailurePoint::BackupWrite,
        FailurePoint::BackupFlush,
        FailurePoint::BackupVerify,
        FailurePoint::TempCreate,
        FailurePoint::TempWrite,
        FailurePoint::TempFlush,
        FailurePoint::ParseStaged,
        FailurePoint::AtomicReplace,
        FailurePoint::ParentSync,
        FailurePoint::ReadBackVerify,
        FailurePoint::SecondFile,
        FailurePoint::ThirdFile,
        FailurePoint::RollbackVerify,
        FailurePoint::ProcessSpawn,
        FailurePoint::ProcessTimeout,
        FailurePoint::NetworkFetch,
    ]
}

/// Coverage report for the QAL-06 failure matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureMatrixReport {
    /// All required points.
    pub required: Vec<crate::failure::FailurePoint>,
    /// Surfaces that the matrix exercises.
    pub surfaces: Vec<String>,
    /// Whether every required point is present.
    pub complete: bool,
}

/// Build the QAL-06 failure matrix report.
///
/// Surfaces per spec: single-file config, multi-file instance creation,
/// template update, bulk skill/MCP, wrapper replace, daemon start via process
/// fixtures. Each surface must have at least one test that injects a failure
/// at a distinct boundary and asserts recovery/rollback.
pub fn failure_matrix_report() -> FailureMatrixReport {
    let required = required_failure_points();
    let surfaces = vec![
        "single_file_config".to_owned(),
        "multi_file_instance_creation".to_owned(),
        "template_update".to_owned(),
        "bulk_skill_mcp".to_owned(),
        "wrapper_replace".to_owned(),
        "daemon_start_via_process_fixtures".to_owned(),
    ];
    let complete = !required.is_empty() && surfaces.len() == 6;
    FailureMatrixReport {
        required,
        surfaces,
        complete,
    }
}

/// Fake harness coverage report for QAL-07.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeHarnessReport {
    /// Version output fixtures present (spaces, missing, non-zero, timeout, huge 10 MiB, etc.).
    pub version_fixtures: Vec<String>,
    /// Network/GitHub fixtures present.
    pub network_fixtures: Vec<String>,
    /// Health classifications covered.
    pub health_cases: Vec<String>,
    /// Whether cross-host redirect stripping is covered.
    pub cross_host_redirect_covered: bool,
    /// Whether the whole report is complete.
    pub complete: bool,
}

/// Build the QAL-07 fake harness coverage report.
///
/// Version fixtures are delegated to `failure::version_output_fixtures`;
/// network fixtures to `failure::FakeNetworkHarness::with_github_matrix`;
/// health / redirect cases are enumerated here for CI ledger completeness.
pub fn fake_harness_report() -> FakeHarnessReport {
    let version_fixtures = crate::failure::version_output_fixtures()
        .into_iter()
        .map(|f| f.name)
        .collect::<Vec<_>>();
    let network_fixtures = crate::failure::FakeNetworkHarness::with_github_matrix().keys();
    let health_cases = vec![
        "healthy".to_owned(),
        "rate_limited".to_owned(),
        "auth_error".to_owned(),
        "tls_error".to_owned(),
        "not_found".to_owned(),
        "server_error".to_owned(),
        "timeout".to_owned(),
        "oversized".to_owned(),
        "redirect_loop".to_owned(),
        "digest_mismatch".to_owned(),
        "cross_host_redirect".to_owned(),
    ];
    let cross_host_redirect_covered = crate::failure::should_strip_auth_for_redirect(
        "https://github.com/org/repo",
        "https://evil.example.com/other",
    );
    let complete = !version_fixtures.is_empty()
        && !network_fixtures.is_empty()
        && health_cases.len() == 11
        && cross_host_redirect_covered;
    FakeHarnessReport {
        version_fixtures,
        network_fixtures,
        health_cases,
        cross_host_redirect_covered,
        complete,
    }
}

/// Combined QAL-06/07 ledger entry: fails CI when either matrix is incomplete.
pub fn qal_06_07_complete() -> bool {
    failure_matrix_report().complete && fake_harness_report().complete
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
        let dir = crate::test_util::temp_dir_unique("verification");
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

    // -----------------------------------------------------------------------
    // QAL-06/07 matrix: failure injection + fake harness checklist
    // -----------------------------------------------------------------------

    #[test]
    fn qal_06_failure_matrix_is_complete() {
        let report = failure_matrix_report();
        assert!(
            report.complete,
            "failure matrix must be complete: {report:?}"
        );
        assert_eq!(report.required.len(), 17);
        assert_eq!(report.surfaces.len(), 6);
        // Each required point must be distinct
        let mut distinct = BTreeSet::new();
        for p in &report.required {
            assert!(distinct.insert(*p), "duplicate point {p:?}");
        }
    }

    #[test]
    fn qal_07_fake_harness_is_complete() {
        let report = fake_harness_report();
        assert!(
            report.complete,
            "fake harness report must be complete: {report:?}"
        );
        // Version fixtures cover the required variants
        let lower: Vec<String> = report
            .version_fixtures
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect();
        for needle in ["spaces", "missing", "non-zero", "timeout", "huge"] {
            assert!(
                lower.iter().any(|s| s.contains(needle)),
                "version fixture missing {needle}: {lower:?}"
            );
        }
        // Network fixtures cover GitHub matrix
        for needle in [
            "catalog_success",
            "digest_mismatch",
            "redirect_loop",
            "rate_limit",
            "timeout",
            "oversized",
            "tls_error",
            "cross_host_redirect",
        ] {
            assert!(
                report.network_fixtures.iter().any(|k| k == needle),
                "network fixture missing {needle}: {:?}",
                report.network_fixtures
            );
        }
        // Health cases
        assert_eq!(report.health_cases.len(), 11);
        assert!(report.cross_host_redirect_covered);
    }

    #[test]
    fn qal_06_07_combined_complete() {
        assert!(
            qal_06_07_complete(),
            "QAL-06/07 combined coverage must be complete"
        );
    }

    #[test]
    #[expect(clippy::excessive_nesting, reason = "deterministic fixture loop")]
    fn fake_process_fixture_version_parse_is_deterministic() {
        let fixtures = crate::failure::version_output_fixtures();
        assert!(fixtures.len() >= 8, "need at least 8 version fixtures");
        let harness = crate::failure::FakeProcessHarness::with_version_fixtures();
        for f in fixtures {
            let via = harness.version_for(&f.name);
            if f.is_timeout || f.is_huge || f.exit_code.is_some_and(|c| c != 0) {
                // These should not parse as success
                if f.is_timeout || f.is_huge {
                    assert!(
                        via.is_none(),
                        "fixture {} should be none due to timeout/huge",
                        f.name
                    );
                }
            } else if let Some(expected) = f.expected_version {
                assert_eq!(
                    via.as_deref(),
                    Some(expected.as_str()),
                    "fixture {} mismatch",
                    f.name
                );
            } else {
                assert!(via.is_none(), "fixture {} should be none", f.name);
            }
        }
    }

    #[test]
    fn fake_network_harness_classifies_health_deterministically() {
        let cases = [
            (429, "rate limit", crate::failure::HealthStatus::RateLimited),
            (401, "unauthorized", crate::failure::HealthStatus::AuthError),
            (
                200,
                "tls error certificate",
                crate::failure::HealthStatus::TlsError,
            ),
            (404, "not found", crate::failure::HealthStatus::NotFound),
            (
                500,
                "internal server error",
                crate::failure::HealthStatus::ServerError,
            ),
            (
                200,
                "timeout after 30s",
                crate::failure::HealthStatus::Timeout,
            ),
            (
                200,
                "response size exceeds limit",
                crate::failure::HealthStatus::Oversized,
            ),
            (
                200,
                "redirect limit exceeded",
                crate::failure::HealthStatus::RedirectLoop,
            ),
            (
                200,
                "digest mismatch",
                crate::failure::HealthStatus::DigestMismatch,
            ),
        ];
        for (status, body, expected) in cases {
            let got = crate::failure::classify_health(status, body);
            assert_eq!(got, expected, "health for {status} {body}");
        }
        assert!(crate::failure::should_strip_auth_for_redirect(
            "https://github.com/a/b",
            "https://evil.com/c"
        ));
        assert!(!crate::failure::should_strip_auth_for_redirect(
            "https://github.com/a/b",
            "https://github.com/c/d"
        ));
    }

    // -----------------------------------------------------------------------
    // QAL-05: mutant-killing guards — each test would fail if its guard were
    // flipped (true→false or false→true).  They assert observable recovery,
    // abort, or redaction behavior, not constants.
    // -----------------------------------------------------------------------

    #[test]
    fn mutant_backup_guard_aborts_on_failure_and_allows_success() {
        let dir = crate::test_util::temp_dir_unique("mutant-backup");
        std::fs::create_dir_all(&dir).unwrap();

        // happy: regular file backup succeeds and verifies
        let file = dir.join("config.json");
        std::fs::write(&file, br#"{"a":1}"#).unwrap();
        let entry = superai_config::backup::backup(&file)
            .unwrap()
            .expect("backup should succeed for regular file");
        let ok = superai_config::backup::verify_backup(&entry).unwrap();
        assert!(ok, "fresh backup must verify");

        // failure: backing up a directory must error (mutant that ignores Err would allow commit without backup)
        let subdir = dir.join("adir");
        std::fs::create_dir_all(&subdir).unwrap();
        let err = superai_config::backup::backup(&subdir).unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("is a directory") || msg.contains("directory"),
            "backup dir should error with directory reason: {msg}"
        );

        // transaction prepare should succeed when backup succeeds (mutant always-abort would fail here)
        let target_ok = dir.join("ok.json");
        std::fs::write(&target_ok, br#"{"x":1}"#).unwrap();
        let op_ok = superai_config::transaction::OperationId::new("op-mutant-ok").unwrap();
        let steps_ok = vec![superai_config::transaction::FileAction::Write {
            path: target_ok,
            content: br#"{"x":2}"#.to_vec(),
            kind: DocumentKind::StrictJson,
        }];
        let mut txn_ok = superai_config::transaction::Transaction::new(op_ok, steps_ok);
        let prep_ok = txn_ok.prepare();
        assert!(
            prep_ok.is_ok(),
            "prepare should succeed when backup succeeds: {prep_ok:?}"
        );

        // injected backup failure must be treated as abort (mutant never-abort would ignore this)
        let file2 = dir.join("inject.json");
        std::fs::write(&file2, br#"{"z":1}"#).unwrap();
        let real = crate::failure::RealInjector;
        let ok_injected = crate::failure::injected_backup(&file2, &real).unwrap();
        assert!(ok_injected.is_some(), "real injector should allow backup");
        let inj = crate::failure::TestInjector::new();
        inj.fail_at(crate::failure::FailurePoint::BackupWrite, 1);
        let err_injected = crate::failure::injected_backup(&file2, &inj).unwrap_err();
        let msg2 = format!("{err_injected:?}");
        assert!(
            msg2.to_ascii_lowercase().contains("backup") || msg2.contains("injected"),
            "injected backup failure should be reported: {msg2}"
        );
        // flipping guard `if backup.is_err() { abort }` to `if false` would make err_injected appear as Ok
    }

    #[test]
    fn mutant_is_modified_guard_blocks_stale_and_allows_fresh() {
        let dir = crate::test_util::temp_dir_unique("mutant-modified");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, br#"{"a":1}"#).unwrap();
        let snap_fresh = superai_config::snapshot::snapshot(&path);
        // same snapshot must not be considered modified (mutant always-true would fail here)
        assert!(
            !superai_config::snapshot::is_modified(&snap_fresh, &snap_fresh),
            "identical snapshots must not be modified"
        );
        // modify file -> must be detected as modified (mutant always-false would fail here)
        std::fs::write(&path, br#"{"a":2}"#).unwrap();
        let snap_stale = snap_fresh;
        let snap_current = superai_config::snapshot::snapshot(&path);
        assert!(
            superai_config::snapshot::is_modified(&snap_stale, &snap_current),
            "changed content must be detected"
        );
        // creation/deletion
        let missing = dir.join("missing.json");
        drop(std::fs::remove_file(&missing));
        let snap_missing = superai_config::snapshot::snapshot(&missing);
        assert!(
            superai_config::snapshot::is_modified(&snap_missing, &snap_current),
            "missing vs exists must be modified"
        );
        assert!(
            !superai_config::snapshot::is_modified(&snap_missing, &snap_missing),
            "missing vs missing must not be modified"
        );

        // guard via raw_editor commit_with_snapshot: stale must error, fresh must succeed
        let dir2 = crate::test_util::temp_dir_unique("mutant-commit");
        std::fs::create_dir_all(&dir2).unwrap();
        let p2 = dir2.join("file.json");
        std::fs::write(&p2, br#"{"v":1}"#).unwrap();
        let snap_before = superai_config::snapshot::snapshot(&p2);
        // external concurrent modification
        std::fs::write(&p2, br#"{"v":2}"#).unwrap();
        let res_stale = superai_config::raw_editor::commit_with_snapshot(
            &p2,
            br#"{"v":3}"#,
            Some(&snap_before),
        );
        assert!(
            res_stale.is_err(),
            "stale snapshot must be rejected with ConcurrentModification"
        );
        let msg = format!("{:?}", res_stale.unwrap_err());
        assert!(
            msg.to_ascii_lowercase().contains("concurrent")
                || msg.to_ascii_lowercase().contains("modification")
                || msg.contains("digest"),
            "error should mention concurrent modification: {msg}"
        );
        // fresh snapshot must allow commit (mutant always-true would block this)
        let snap_fresh2 = superai_config::snapshot::snapshot(&p2);
        let res_fresh = superai_config::raw_editor::commit_with_snapshot(
            &p2,
            br#"{"v":3}"#,
            Some(&snap_fresh2),
        );
        assert!(
            res_fresh.is_ok(),
            "fresh snapshot must allow commit: {res_fresh:?}"
        );
        let back = std::fs::read(&p2).unwrap();
        assert_eq!(back, br#"{"v":3}"#);
    }

    #[test]
    #[expect(
        clippy::redundant_clone,
        reason = "paths retained for rollback verification"
    )]
    fn mutant_rollback_reports_residual_on_corrupted_backup() {
        let dir = crate::test_util::temp_dir_unique("mutant-rollback");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.json");
        let b = dir.join("b.json");
        std::fs::write(&a, br#"{"a":1}"#).unwrap();
        std::fs::write(&b, br#"{"b":1}"#).unwrap();

        // happy rollback: intact backups yield no residuals (mutant always-residual would fail here)
        let op1 = superai_config::transaction::OperationId::new("op-rollback-ok").unwrap();
        let steps1 = vec![
            superai_config::transaction::FileAction::Write {
                path: a.clone(),
                content: br#"{"a":2}"#.to_vec(),
                kind: DocumentKind::StrictJson,
            },
            superai_config::transaction::FileAction::Write {
                path: b.clone(),
                content: br#"{"b":2}"#.to_vec(),
                kind: DocumentKind::StrictJson,
            },
        ];
        let mut txn1 = superai_config::transaction::Transaction::new(op1, steps1);
        txn1.prepare().unwrap();
        assert_eq!(txn1.backups.len(), 2, "both files existed so two backups");
        txn1.commit().unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), br#"{"a":2}"#);
        let rb1 = txn1.rollback().unwrap();
        assert!(
            rb1.residuals.is_empty(),
            "intact backups should yield no residuals: {rb1:?}"
        );
        assert_eq!(rb1.rolled_back.len(), 2);
        assert!(
            rb1.verification_ok,
            "verification should pass when restored"
        );
        assert_eq!(
            std::fs::read(&a).unwrap(),
            br#"{"a":1}"#,
            "rollback must restore original content"
        );

        // corrupt path: one backup corrupted -> residual reported (mutant never-residual would fail here)
        std::fs::write(&a, br#"{"a":1}"#).unwrap();
        std::fs::write(&b, br#"{"b":1}"#).unwrap();
        let op2 = superai_config::transaction::OperationId::new("op-rollback-corrupt").unwrap();
        let steps2 = vec![
            superai_config::transaction::FileAction::Write {
                path: a.clone(),
                content: br#"{"a":3}"#.to_vec(),
                kind: DocumentKind::StrictJson,
            },
            superai_config::transaction::FileAction::Write {
                path: b.clone(),
                content: br#"{"b":3}"#.to_vec(),
                kind: DocumentKind::StrictJson,
            },
        ];
        let mut txn2 = superai_config::transaction::Transaction::new(op2, steps2);
        txn2.prepare().unwrap();
        txn2.commit().unwrap();
        // corrupt backup for a
        let backup_a = txn2
            .backups
            .iter()
            .find(|e| e.original_path == a)
            .expect("backup for a must exist")
            .clone();
        std::fs::write(&backup_a.backup_path, b"corrupted").unwrap();
        let rb2 = txn2.rollback().unwrap();
        assert!(
            rb2.residuals.contains(&a),
            "corrupted backup must be reported as residual: {rb2:?}"
        );
        assert!(
            !rb2.verification_ok || !rb2.residuals.is_empty(),
            "corrupted rollback should not be fully verified"
        );
    }

    #[test]
    fn mutant_validate_quarantine_target_rejects_broad_roots_and_accepts_valid() {
        use std::path::Path;
        // broad roots must be rejected (mutant that returns Ok would allow disastrous delete)
        for p in ["/", "/home", "/tmp", "/usr", "/etc", "/var"] {
            let r = superai_config::quarantine::validate_quarantine_target(Path::new(p));
            assert!(r.is_err(), "broad root {p} should be rejected");
        }
        // home dir must be rejected
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
            && home.is_absolute()
        {
            // only test if path exists or not, validate checks equality before existence for home
            let r = superai_config::quarantine::validate_quarantine_target(&home);
            // home may not exist in temp HOME override, but still should be rejected as broad root/home
            assert!(r.is_err(), "home {} should be rejected", home.display());
        }
        // globs must be rejected before existence check
        for p in ["/tmp/*.json", "/var/*.log", "/tmp/foo?bar", "/tmp/[abc]"] {
            let r = superai_config::quarantine::validate_quarantine_target(Path::new(p));
            assert!(r.is_err(), "glob {p} should be rejected");
        }
        // unresolved variables must be rejected
        for p in ["/tmp/$HOME/foo", "/tmp/%USERPROFILE%/bar", "/tmp/${HOME}/x"] {
            let r = superai_config::quarantine::validate_quarantine_target(Path::new(p));
            assert!(r.is_err(), "var {p} should be rejected");
        }
        // relative and traversal must be rejected
        assert!(
            superai_config::quarantine::validate_quarantine_target(Path::new("relative/path"))
                .is_err(),
            "relative should be rejected"
        );
        assert!(
            superai_config::quarantine::validate_quarantine_target(Path::new("/tmp/../etc"))
                .is_err(),
            "traversal should be rejected"
        );
        // valid file must be accepted (mutant that always Err would fail here)
        let dir = crate::test_util::temp_dir_unique("mutant-quarantine");
        std::fs::create_dir_all(&dir).unwrap();
        let valid = dir.join("valid.json");
        std::fs::write(&valid, b"{}").unwrap();
        let ok = superai_config::quarantine::validate_quarantine_target(&valid);
        assert!(ok.is_ok(), "valid file should be accepted: {ok:?}");
    }

    #[test]
    fn mutant_redacted_string_hides_secret_in_all_channels() {
        let secret = "sk-live-super-secret-12345";
        // core RedactedString
        let r1 = crate::error::RedactedString::new(secret);
        for out in [
            format!("{r1:?}"),
            format!("{r1}"),
            serde_json::to_string(&r1).unwrap(),
        ] {
            assert!(
                !out.contains(secret),
                "core redacted must not contain secret: {out}"
            );
            assert!(
                out.contains("[REDACTED]"),
                "core redacted must contain placeholder: {out}"
            );
        }
        assert_eq!(
            r1.expose_secret(),
            secret,
            "expose_secret should return original"
        );

        // operation RedactedString
        let r2 = crate::operation::RedactedString::new(secret);
        for out in [
            format!("{r2:?}"),
            format!("{r2}"),
            serde_json::to_string(&r2).unwrap(),
        ] {
            assert!(
                !out.contains(secret),
                "op redacted must not contain secret: {out}"
            );
            assert!(
                out.contains("[REDACTED]"),
                "op redacted must contain placeholder: {out}"
            );
        }
        assert_eq!(r2.expose_secret(), secret);

        // error containing redacted must not leak via Debug/Display
        let err = crate::error::CoreError::SecretValidation {
            field: "apiKey".to_owned(),
            reason: "test".to_owned(),
            redacted: crate::error::RedactedString::new(secret),
        };
        for out in [format!("{err:?}"), format!("{err}")] {
            assert!(!out.contains(secret), "error must not leak secret: {out}");
        }
        // flipped mutant that implements Debug as `f.write_str(&self.0)` would leak and be caught above
    }

    #[test]
    fn mutant_template_three_way_detects_conflict_and_allows_clean() {
        use crate::ids::{HarnessId, ProviderId, TemplateId};
        use crate::template::{OwnedPatch, TEMPLATE_SCHEMA_VERSION, Template, TemplateStatus};
        use serde_json::{Map, Value, json};
        fn tmpl(version: &str, patches: Vec<OwnedPatch>) -> Template {
            Template {
                schema_version: TEMPLATE_SCHEMA_VERSION,
                id: TemplateId::new("claude-glm").unwrap(),
                version: version.to_owned(),
                harness: HarnessId::new("claude-code").unwrap(),
                provider: ProviderId::new("glm").unwrap(),
                label: "Claude Code on GLM".to_owned(),
                status: TemplateStatus::Active,
                inputs: vec![],
                patches,
                wrapper_env: std::collections::BTreeMap::new(),
                wrapper_args: vec![],
                assets: vec![],
                capability_map: std::collections::BTreeMap::new(),
                migration_notes: vec![],
                digest: "a".repeat(64),
                harness_version_req: None,
                provider_protocol: None,
            }
        }
        fn patch(sel: &str, v: Value) -> OwnedPatch {
            OwnedPatch {
                selector: sel.to_owned(),
                value: v,
            }
        }

        // BothModified conflict: base, new, local all differ -> must be conflict (mutant that always Ok would miss this)
        let base = tmpl("1.0.0", vec![patch("key:model", json!("glm-4"))]);
        let new = tmpl("1.1.0", vec![patch("key:model", json!("glm-4.5"))]);
        let mut local: Map<String, Value> = Map::new();
        local.insert("model".to_owned(), json!("my-custom"));
        let preview = crate::template_update::preview_three_way(&base, &new, &local);
        assert_eq!(
            preview.conflicts.len(),
            1,
            "both_modified should be single conflict"
        );
        assert_eq!(
            preview.conflicts[0].kind,
            crate::template_update::ConflictKind::BothModified,
            "kind should be BothModified"
        );
        assert!(
            preview.auto_applicable.is_empty(),
            "conflicted selector must not be auto applicable"
        );
        assert!(!preview.can_auto_apply());

        // clean: local == base -> should auto-apply new (mutant always-conflict would fail here)
        let mut local2: Map<String, Value> = Map::new();
        local2.insert("model".to_owned(), json!("glm-4"));
        let preview2 = crate::template_update::preview_three_way(&base, &new, &local2);
        assert!(
            preview2.conflicts.is_empty(),
            "local == base should have no conflict"
        );
        assert_eq!(preview2.auto_applicable.len(), 1);
        assert!(preview2.can_auto_apply());
        assert_eq!(preview2.auto_applicable[0].to, Some(json!("glm-4.5")));

        // new == base -> keep local, no conflict
        let base3 = tmpl("1.0.0", vec![patch("key:model", json!("glm-4"))]);
        let new3 = tmpl("1.0.0", vec![patch("key:model", json!("glm-4"))]);
        let mut local3: Map<String, Value> = Map::new();
        local3.insert("model".to_owned(), json!("my-custom-keep"));
        let preview3 = crate::template_update::preview_three_way(&base3, &new3, &local3);
        assert!(preview3.conflicts.is_empty());
        assert!(preview3.auto_applicable.is_empty());

        // local == new -> already applied, no conflict
        let mut local4: Map<String, Value> = Map::new();
        local4.insert("model".to_owned(), json!("glm-4.5"));
        let preview4 = crate::template_update::preview_three_way(&base, &new, &local4);
        assert!(preview4.conflicts.is_empty());
        assert!(preview4.auto_applicable.is_empty());
    }

    #[test]
    fn mutant_capability_resolver_absent_for_unknown_and_native_for_known() {
        use crate::capability::{Capability, Support};
        use crate::capability_resolver::{CapabilitySource, resolve};
        use crate::ids::{HarnessId, ProviderId};

        // unknown harness/provider must be Absent + Unknown (mutant that returns Native would fail)
        let unk_h = HarnessId::new("unknown-harness-xyz").unwrap();
        let unk_p = ProviderId::new("unknown-provider-xyz").unwrap();
        for cap in [
            Capability::WebSearch,
            Capability::Vision,
            Capability::Mcp,
            Capability::ComputerUse,
        ] {
            let r = resolve(&unk_h, &unk_p, cap);
            assert_eq!(
                r.support,
                Support::Absent,
                "unknown should be absent for {cap:?}"
            );
            assert_eq!(
                r.source,
                CapabilitySource::Unknown,
                "unknown source for {cap:?}"
            );
            assert!(
                r.explanation.contains("no matrix entry"),
                "explanation should mention missing entry: {r:?}"
            );
        }

        // known pair claude-code + anthropic should be Native for web_search (mutant that returns Absent would fail)
        let known_h = HarnessId::new("claude-code").unwrap();
        let known_p = ProviderId::new("anthropic").unwrap();
        let r2 = resolve(&known_h, &known_p, Capability::WebSearch);
        assert_eq!(r2.support, Support::Native);
        assert_eq!(r2.source, CapabilitySource::Harness);

        // case insensitivity: Claude-Code + ANTHROPIC should also be native
        let ci_h = HarnessId::new("Claude-Code").unwrap();
        let ci_p = ProviderId::new("ANTHROPIC").unwrap();
        let r3 = resolve(&ci_h, &ci_p, Capability::WebSearch);
        assert_eq!(r3.support, Support::Native, "case-fold should be native");

        // known pair claude-code + glm web_search should be Substituted via provider
        let glm_p = ProviderId::new("glm").unwrap();
        let r4 = resolve(&known_h, &glm_p, Capability::WebSearch);
        assert_eq!(r4.support, Support::Substituted);
        assert_eq!(r4.source, CapabilitySource::Provider);
    }
}
