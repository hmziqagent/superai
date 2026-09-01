//! Core raw editor — harness-aware wrapper over `superai_config::raw_editor`.
//!
//! Provides interface-neutral read/validate/diff/commit that enforces
//! harness version and surface ownership policies before delegating to the
//! config-layer backend. No interface types are introduced.

use std::path::Path;

use superai_config::document::DocumentKind as ConfigKind;
use superai_config::raw_editor::{CommitReport, DiffResult, RawDocument};

use crate::adapter::{Adapter, DocumentKind as AdapterKind, SurfaceOwnership};
use crate::error::{CoreError, Result};

/// Re-export sensitive wrapper and document types for core consumers.
pub use superai_config::raw_editor::{find_redaction_spans, validate};

/// Interface-agnostic raw editor service for core.
///
/// Wraps the config-layer `RawEditor` and enforces harness version and
/// surface-ownership policies before delegating. Disk is the truth on every
/// `open`; `validate` never touches disk; `diff` returns redacted lexical
/// diff plus semantic ops; `commit` validates, checks conflict, backs up,
/// atomically replaces, and verifies. No GPUI types.
#[derive(Debug, Clone, Default)]
pub struct RawEditor {
    inner: superai_config::raw_editor::RawEditor,
}

impl RawEditor {
    /// Create a stateless service handle.
    pub fn new() -> Self {
        Self {
            inner: superai_config::raw_editor::RawEditor::new(),
        }
    }

    /// Open `path` fresh from disk, detecting kind via `DocumentKind::from_path`.
    ///
    /// Returns a neutral `SourceDocument` envelope. Missing file is an error.
    pub fn open(&self, path: &Path) -> Result<superai_config::document::SourceDocument> {
        self.inner.open(path).map_err(CoreError::Config)
    }

    /// Open via the sensitive `RawDocument` wrapper (preserves `Snapshot` token).
    pub fn open_raw(&self, path: &Path) -> Result<RawDocument> {
        read(path)
    }

    /// Validate `content` for `kind` without touching disk.
    pub fn validate(
        &self,
        content: &[u8],
        kind: ConfigKind,
    ) -> Vec<superai_config::document::Diagnostic> {
        self.inner.validate(content, kind)
    }

    /// Diff `old` vs `new` for `kind`, producing redacted lexical diff and semantic ops.
    pub fn diff(&self, old: &[u8], new: &[u8], kind: ConfigKind) -> DiffResult {
        self.inner.diff(old, new, kind)
    }

    /// Find secret-bearing spans in `content` for UI redaction.
    pub fn find_redaction_spans(
        &self,
        content: &[u8],
        kind: ConfigKind,
    ) -> Vec<superai_config::raw_editor::RedactionSpan> {
        self.inner.find_redaction_spans(content, kind)
    }

    /// Commit `new_content` to `path` after validation and conflict check.
    pub fn commit(
        &self,
        path: &Path,
        new_content: &[u8],
        expected_digest: Option<&str>,
    ) -> Result<CommitReport> {
        commit(path, new_content, expected_digest)
    }

    /// Commit with explicit `Snapshot` conflict token.
    pub fn commit_with_snapshot(
        &self,
        path: &Path,
        new_content: &[u8],
        expected: Option<&superai_config::snapshot::Snapshot>,
    ) -> Result<CommitReport> {
        commit_with_snapshot(path, new_content, expected)
    }

    /// Commit that also enforces harness version and surface ownership.
    pub fn commit_for_adapter(
        &self,
        path: &Path,
        new_content: &[u8],
        expected_digest: Option<&str>,
        adapter: &dyn Adapter,
    ) -> Result<CommitReport> {
        commit_for_adapter(path, new_content, expected_digest, adapter)
    }

    /// Validate `content` for `path` against the matching surface's declared
    /// schema (HAD-03); adapter-attributed diagnostics, never touches disk.
    pub fn validate_for_adapter(
        &self,
        adapter: &dyn Adapter,
        path: &Path,
        content: &[u8],
    ) -> Vec<superai_config::document::Diagnostic> {
        validate_for_adapter(adapter, path, content)
    }
}

/// Read a document fresh from disk, detecting kind via extension.
///
/// Delegates to `superai_config::raw_editor::read` and maps errors to
/// `CoreError`.
pub fn read(path: &Path) -> Result<RawDocument> {
    superai_config::raw_editor::read(path).map_err(CoreError::Config)
}

/// Produce semantic ops, lexical diff, and redaction spans for `old` vs `new`.
///
/// Delegates to `superai_config::raw_editor::diff`.
pub fn diff(old: &[u8], new: &[u8], kind: ConfigKind) -> DiffResult {
    superai_config::raw_editor::diff(old, new, kind)
}

/// Commit `new_content` to `path` after validation and conflict check.
///
/// Delegates to `superai_config::raw_editor::commit`.
pub fn commit(
    path: &Path,
    new_content: &[u8],
    expected_digest: Option<&str>,
) -> Result<CommitReport> {
    superai_config::raw_editor::commit(path, new_content, expected_digest)
        .map_err(CoreError::Config)
}

/// Commit with a snapshot conflict token.
pub fn commit_with_snapshot(
    path: &Path,
    new_content: &[u8],
    expected: Option<&superai_config::snapshot::Snapshot>,
) -> Result<CommitReport> {
    superai_config::raw_editor::commit_with_snapshot(path, new_content, expected)
        .map_err(CoreError::Config)
}

/// Find the adapter surface whose id or fallback hint appears in `path`.
///
/// Mirrors the matching used by [`commit_for_adapter`]: the most specific
/// surface whose id (case-insensitive) is contained in the path, or whose
/// non-empty path-resolver fallback is contained in it.
pub fn surface_for_path(
    adapter: &dyn Adapter,
    path: &Path,
) -> Option<crate::adapter::ConfigSurface> {
    let path_str = path.to_string_lossy();
    let path_lower = path_str.to_ascii_lowercase();
    for surface in adapter.config_surfaces() {
        let id_lower = surface.id.to_ascii_lowercase();
        let fallback_lower = surface.path_resolver.fallback.to_ascii_lowercase();
        let matches = path_lower.contains(id_lower.as_str())
            || (!fallback_lower.is_empty() && path_lower.contains(fallback_lower.as_str()))
            || path_lower.ends_with(id_lower.as_str());
        if matches {
            return Some(surface);
        }
    }
    None
}

/// Validate `content` for `path` against the surface schema the adapter
/// declares for the matching surface (HAD-03).
///
/// Syntax + semantic + deprecation diagnostics, each attributed to the
/// harness and surface. When no surface matches, or the matching surface
/// declares no schema, plain syntax diagnostics for the detected kind are
/// returned. Never touches disk.
pub fn validate_for_adapter(
    adapter: &dyn Adapter,
    path: &Path,
    content: &[u8],
) -> Vec<superai_config::document::Diagnostic> {
    let kind = ConfigKind::from_path(path);
    match surface_for_path(adapter, path) {
        Some(surface) => {
            crate::adapter::validate_surface_content(adapter, &surface.id, content, kind)
        }
        None => validate(content, kind),
    }
}

/// Commit that also enforces harness version and surface ownership.
///
/// Checks `adapter.version_resolution().compatible` and the target surface's
/// kind/ownership before delegating to the config backend. Wrong version and
/// read-only internal/keychain surfaces are blocked without touching disk.
#[expect(
    clippy::excessive_nesting,
    reason = "surface policy needs nested matching"
)]
pub fn commit_for_adapter(
    path: &Path,
    new_content: &[u8],
    expected_digest: Option<&str>,
    adapter: &dyn Adapter,
) -> Result<CommitReport> {
    // Version gate (RAW-07 / HAD version policy)
    let version = adapter.version_resolution();
    if !version.compatible {
        let ver = version
            .detected_version
            .as_deref()
            .unwrap_or("unknown")
            .to_owned();
        return Err(CoreError::UnsupportedVersion {
            harness: adapter.id().to_string(),
            version: ver,
            reason: "harness version not compatible for writes".to_owned(),
        });
    }

    // Surface ownership gate (RAW-06)
    // Find the most specific surface whose id or fallback appears in the path.
    let path_str = path.to_string_lossy();
    let path_lower = path_str.to_ascii_lowercase();
    for surface in adapter.config_surfaces() {
        let id_lower = surface.id.to_ascii_lowercase();
        let fallback_lower = surface.path_resolver.fallback.to_ascii_lowercase();
        let matches = path_lower.contains(id_lower.as_str())
            || (!fallback_lower.is_empty() && path_lower.contains(fallback_lower.as_str()))
            || path_lower.ends_with(id_lower.as_str());
        if matches {
            match surface.kind {
                AdapterKind::Executable => {
                    return Err(CoreError::ResearchBlocked {
                        harness: adapter.id().to_string(),
                        surface: surface.id,
                        reason: "executable config is read-only via raw editor".to_owned(),
                    });
                }
                AdapterKind::Sqlite | AdapterKind::Keychain | AdapterKind::Opaque => {
                    return Err(CoreError::UnsupportedOperation {
                        harness: adapter.id().to_string(),
                        operation: "raw_commit".to_owned(),
                        reason: format!("surface `{}` is read-only ({})", surface.id, surface.kind),
                    });
                }
                _ => {
                    if surface.ownership == SurfaceOwnership::ExternalSecretStore
                        || surface.ownership == SurfaceOwnership::HarnessManaged
                    {
                        // Harness-managed or external secret stores are not writable via raw editor
                        // unless the adapter explicitly marks them user-editable.
                        // For now, block external secret store surfaces.
                        if surface.ownership == SurfaceOwnership::ExternalSecretStore {
                            return Err(CoreError::UnsupportedOperation {
                                harness: adapter.id().to_string(),
                                operation: "raw_commit".to_owned(),
                                reason: format!(
                                    "surface `{}` is externally managed ({})",
                                    surface.id, surface.ownership
                                ),
                            });
                        }
                    }
                }
            }
            // Found matching writable surface, stop searching.
            break;
        }
    }

    // Also block config-level Opaque detection (e.g. unknown binary)
    let config_kind = ConfigKind::from_path(path);
    if config_kind == ConfigKind::Opaque {
        // If adapter has no matching surface, still block opaque via config kind
        // to satisfy RAW-06 for internal SQLite/keychain stores.
        let known_surface = adapter
            .config_surfaces()
            .iter()
            .any(|s| path_lower.contains(s.id.to_ascii_lowercase().as_str()));
        if !known_surface {
            // Unknown file with opaque kind: allow only if adapter says it's not internal.
            // For safety, treat generic opaque as read-only when no surface matches
            // and path looks like a db/keychain.
            if path_lower.contains(".db")
                || path_lower.contains(".sqlite")
                || path_lower.contains("keychain")
            {
                return Err(CoreError::UnsupportedOperation {
                    harness: adapter.id().to_string(),
                    operation: "raw_commit".to_owned(),
                    reason: "opaque/internal store is read-only".to_owned(),
                });
            }
        }
    }

    // HAD-05/HAD-03: era-conflict refusal + semantic schema rejection.
    surface_gates_for_adapter(path, new_content, adapter, &version, config_kind)?;

    commit(path, new_content, expected_digest)
}

/// Era-conflict and semantic-schema gates shared by the adapter-aware commit
/// (HAD-05 step 5 + HAD-03). Both refuse before any disk mutation, with
/// adapter-attributed diagnostics.
fn surface_gates_for_adapter(
    path: &Path,
    new_content: &[u8],
    adapter: &dyn Adapter,
    version: &crate::adapter::VersionResolution,
    config_kind: ConfigKind,
) -> Result<()> {
    let Some(surface) = surface_for_path(adapter, path) else {
        return Ok(());
    };
    // Config-era gate: refuse writes on conflicting era.
    if let Some(reason) = adapter.era_conflict_reason(&surface.id, new_content) {
        return Err(CoreError::UnsupportedVersion {
            harness: adapter.id().to_string(),
            version: version
                .schema_version
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            reason,
        });
    }
    // Semantic schema gate: reject schema-invalid content with adapter-
    // attributed diagnostics.
    let Some(schema) = adapter.surface_schema(&surface.id) else {
        return Ok(());
    };
    let engine = schema.semantic_schema();
    let diagnostics =
        superai_config::raw_editor::validate_with_schema(new_content, config_kind, Some(&engine));
    let errors: Vec<String> = diagnostics
        .into_iter()
        .filter(|d| d.severity == superai_config::document::DiagnosticSeverity::Error)
        .map(|d| format!("[{}/{}] {}", adapter.id(), surface.id, d.message))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CoreError::SchemaValidation {
            path: path.to_path_buf(),
            details: errors.join("; "),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::adapter::{
        DetectionResult, DocumentKind, PathResolver, ProductStatus, VersionResolution,
    };
    use crate::ids::HarnessId;
    use crate::state::AdapterSupport;

    fn unique_scratch(prefix: &str, suffix: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let pid = std::process::id();
        let dir = crate::test_util::temp_dir_unique("core-raw-editor");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{prefix}-{now}-{pid}{suffix}"))
    }

    #[derive(Debug)]
    struct IncompatibleAdapter;

    impl Adapter for IncompatibleAdapter {
        fn id(&self) -> HarnessId {
            HarnessId::new("claude-code").unwrap()
        }
        #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
        fn display_name(&self) -> &str {
            "Claude Code"
        }
        fn product_status(&self) -> ProductStatus {
            ProductStatus::Active
        }
        fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
            Vec::new()
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
                detected_version: Some("0.0.1".to_owned()),
                schema_version: None,
                compatible: false,
                notes: vec!["incompatible".to_owned()],
            }
        }
        fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
            vec![crate::adapter::ConfigSurface::new(
                "settings.json",
                PathResolver::fallback_only("settings.json"),
                DocumentKind::Json,
                crate::adapter::ConfigScope::User,
                SurfaceOwnership::UserEditable,
            )]
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
        ) -> std::result::Result<crate::adapter::WrapperPlan, CoreError> {
            Ok(crate::adapter::WrapperPlan::new("test"))
        }
        fn scan_candidates(&self) -> Vec<String> {
            Vec::new()
        }
        fn validate_instance(&self, _instance: &crate::instance::Instance) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct ReadOnlyAdapter;

    impl Adapter for ReadOnlyAdapter {
        fn id(&self) -> HarnessId {
            HarnessId::new("opencode").unwrap()
        }
        #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
        fn display_name(&self) -> &str {
            "OpenCode"
        }
        fn product_status(&self) -> ProductStatus {
            ProductStatus::Active
        }
        fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
            Vec::new()
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
            vec![crate::adapter::ConfigSurface {
                id: "keychain".to_owned(),
                path_resolver: PathResolver::fallback_only("keychain"),
                kind: DocumentKind::Keychain,
                scope: crate::adapter::ConfigScope::Internal,
                ownership: SurfaceOwnership::ExternalSecretStore,
                precedence: 0,
                owned_selectors: Vec::new(),
                backup_required: true,
                restart_behavior: crate::adapter::RestartBehavior::None,
            }]
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
        ) -> std::result::Result<crate::adapter::WrapperPlan, CoreError> {
            Ok(crate::adapter::WrapperPlan::new("test"))
        }
        fn scan_candidates(&self) -> Vec<String> {
            Vec::new()
        }
        fn validate_instance(&self, _instance: &crate::instance::Instance) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn wrong_version_blocks_commit() {
        let path = unique_scratch("version-block", ".json");
        std::fs::write(&path, br#"{"a":1}"#).unwrap();
        let adapter = IncompatibleAdapter;
        let err = commit_for_adapter(&path, br#"{"a":2}"#, None, &adapter).unwrap_err();
        match err {
            CoreError::UnsupportedVersion { .. } => {}
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
        // File untouched
        let after = std::fs::read(&path).unwrap();
        assert_eq!(after, br#"{"a":1}"#);
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn read_only_surface_blocks_commit() {
        let path = unique_scratch("keychain-block", ".keychain");
        // Path contains "keychain" so it matches the read-only surface
        std::fs::write(&path, b"secret").unwrap();
        let adapter = ReadOnlyAdapter;
        let err = commit_for_adapter(&path, b"new", None, &adapter).unwrap_err();
        match err {
            CoreError::UnsupportedOperation { .. } | CoreError::ResearchBlocked { .. } => {}
            other => panic!("expected read-only error, got {other:?}"),
        }
        let after = std::fs::read(&path).unwrap();
        assert_eq!(after, b"secret");
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn valid_commit_through_adapter_succeeds() {
        let path = unique_scratch("adapter-ok", ".json");
        std::fs::write(&path, br#"{"a":1}"#).unwrap();
        let adapter = ReadOnlyAdapter; // this adapter has keychain surface, not json, so json path is allowed
        // Use a path that does not match keychain surface, should succeed
        let json_path = unique_scratch("adapter-ok-json", ".json");
        std::fs::write(&json_path, br#"{"a":1}"#).unwrap();
        let res = commit_for_adapter(&json_path, br#"{"a":2}"#, None, &adapter);
        assert!(res.is_ok(), "json commit should succeed: {:?}", res.err());
        let after = std::fs::read(&json_path).unwrap();
        assert_eq!(after, br#"{"a":2}"#);
        if let Ok(report) = res
            && let Some(b) = report.backup
        {
            drop(std::fs::remove_file(b.backup_path));
        }
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&json_path));
    }

    #[test]
    fn jsonc_and_yaml_commits_are_byte_verbatim() {
        // codec-honesty (DOC-05/DOC-06): the raw byte committer never
        // re-serializes, so JSONC/YAML commits preserve caller bytes exactly;
        // the lossy-write refusal lives in the value codecs, not here.
        let jsonc_path = unique_scratch("verbatim", ".jsonc");
        let new_jsonc: &[u8] = b"{\"a\":1, // keep\n}";
        let report = commit(&jsonc_path, new_jsonc, None).unwrap();
        assert!(!report.is_noop);
        assert_eq!(std::fs::read(&jsonc_path).unwrap(), new_jsonc);

        let yaml_path = unique_scratch("verbatim", ".yaml");
        let new_yaml: &[u8] = b"a: 1 # keep\n";
        let report = commit(&yaml_path, new_yaml, None).unwrap();
        assert!(!report.is_noop);
        assert_eq!(std::fs::read(&yaml_path).unwrap(), new_yaml);

        drop(std::fs::remove_file(&jsonc_path));
        drop(std::fs::remove_file(&yaml_path));
    }

    // -------------------------------------------------------------------
    // HAD-03 schema gate + HAD-05 era gate at the commit boundary
    // -------------------------------------------------------------------

    use crate::adapter::{RootShape, SurfaceSchema};

    /// Adapter with a compatible version, writable TOML + JSON surfaces, a
    /// table-root schema with typed owned keys, and a profile-era conflict
    /// detector (mirrors the codex-cli declaration).
    #[derive(Debug)]
    struct SchemaEraAdapter;

    impl SchemaEraAdapter {
        fn schema() -> SurfaceSchema {
            SurfaceSchema::new()
                .with_root_shape(RootShape::Table)
                .with_owned_key("model", superai_config::document::ValueType::String)
        }

        fn json_schema() -> SurfaceSchema {
            SurfaceSchema::new().with_root_shape(RootShape::Object)
        }
    }

    impl Adapter for SchemaEraAdapter {
        fn id(&self) -> HarnessId {
            HarnessId::new("codex-cli").unwrap()
        }
        #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
        fn display_name(&self) -> &str {
            "Codex CLI"
        }
        fn product_status(&self) -> ProductStatus {
            ProductStatus::Active
        }
        fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
            Vec::new()
        }
        #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
        fn adapter_revision(&self) -> &str {
            "0.1.0"
        }
        #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
        fn research_doc_link(&self) -> &str {
            "docs/harness-configs/codex-cli.md"
        }
        #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
        fn last_verified_date(&self) -> &str {
            "2026-08-25"
        }
        fn detection(&self) -> DetectionResult {
            DetectionResult::absent(vec!["test".to_owned()])
        }
        fn version_resolution(&self) -> VersionResolution {
            VersionResolution::new(Some("0.134.0".to_owned()), Some("1".to_owned()), true)
        }
        fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
            vec![
                crate::adapter::ConfigSurface::new(
                    "config.toml",
                    PathResolver::fallback_only("~/.codex/config.toml"),
                    DocumentKind::Toml,
                    crate::adapter::ConfigScope::User,
                    SurfaceOwnership::UserEditable,
                ),
                crate::adapter::ConfigSurface::new(
                    "settings.json",
                    PathResolver::fallback_only("~/settings.json"),
                    DocumentKind::Json,
                    crate::adapter::ConfigScope::User,
                    SurfaceOwnership::UserEditable,
                ),
            ]
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
        ) -> std::result::Result<crate::adapter::WrapperPlan, CoreError> {
            Ok(crate::adapter::WrapperPlan::new("test"))
        }
        fn scan_candidates(&self) -> Vec<String> {
            Vec::new()
        }
        fn validate_instance(&self, _instance: &crate::instance::Instance) -> Result<()> {
            Ok(())
        }
        fn surface_schema(&self, surface_id: &str) -> Option<SurfaceSchema> {
            match surface_id {
                "config.toml" => Some(Self::schema()),
                "settings.json" => Some(Self::json_schema()),
                _ => None,
            }
        }
        fn era_conflict_reason(&self, surface_id: &str, content: &[u8]) -> Option<String> {
            if surface_id != "config.toml" {
                return None;
            }
            let text = String::from_utf8_lossy(content);
            (text.contains("[profiles."))
                .then(|| "legacy inline [profiles.*] tables conflict with the >=0.134 profile-file era; migrate before writing".to_owned())
        }
    }

    /// Scratch file whose name contains `name` so surface matching applies.
    fn surface_scratch(dir_prefix: &str, name: &str) -> PathBuf {
        let dir = crate::test_util::temp_dir_unique(dir_prefix);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn schema_invalid_commit_is_rejected_without_touching_disk() {
        let path = surface_scratch("schema-block", "config.toml");
        let original: &[u8] = b"model = \"gpt-5\"\n";
        std::fs::write(&path, original).unwrap();
        let adapter = SchemaEraAdapter;
        // model must be a string; an inline table violates the schema.
        let bad = b"model = { nested = true }\n";
        let err = commit_for_adapter(&path, bad, None, &adapter).unwrap_err();
        match err {
            CoreError::SchemaValidation { details, .. } => {
                assert!(details.contains("[codex-cli/config.toml]"), "{details}");
                assert!(
                    details.contains("`model` must hold a value of type string"),
                    "{details}"
                );
            }
            other => panic!("expected SchemaValidation, got {other:?}"),
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn array_root_commit_is_rejected_by_root_shape() {
        let path = surface_scratch("schema-root", "settings.json");
        std::fs::write(&path, br#"{"a":1}"#).unwrap();
        let adapter = SchemaEraAdapter;
        // A JSON array root violates the declared object root shape.
        let err = commit_for_adapter(&path, b"[1, 2]", None, &adapter).unwrap_err();
        match err {
            CoreError::SchemaValidation { details, .. } => {
                assert!(details.contains("root must be a object"), "{details}");
            }
            other => panic!("expected SchemaValidation, got {other:?}"),
        }
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn schema_valid_commit_succeeds() {
        let path = surface_scratch("schema-ok", "config.toml");
        std::fs::write(&path, b"model = \"gpt-4\"\n").unwrap();
        let adapter = SchemaEraAdapter;
        commit_for_adapter(&path, b"model = \"gpt-5\"\n", None, &adapter).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"model = \"gpt-5\"\n");
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn era_conflict_commit_is_refused_without_touching_disk() {
        let path = surface_scratch("era-block", "config.toml");
        let original: &[u8] = b"model = \"gpt-5\"\n";
        std::fs::write(&path, original).unwrap();
        let adapter = SchemaEraAdapter;
        // Legacy-era inline profiles conflict with the resolved >=0.134 era.
        let legacy = b"model = \"gpt-4\"\n[profiles.o3]\nmodel = \"o3\"\n";
        let err = commit_for_adapter(&path, legacy, None, &adapter).unwrap_err();
        match err {
            CoreError::UnsupportedVersion { reason, .. } => {
                assert!(reason.contains("[profiles.*]"), "{reason}");
                assert!(reason.contains("0.134"), "{reason}");
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn validate_for_adapter_attributes_diagnostics_to_surface() {
        let adapter = SchemaEraAdapter;
        let diags = validate_for_adapter(
            &adapter,
            Path::new("/tmp/whatever/config.toml"),
            b"model = 5\n",
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.starts_with("[codex-cli/config.toml]")
                    && d.message
                        .contains("`model` must hold a value of type string")),
            "diags: {diags:?}"
        );
        // No matching surface: plain syntax diagnostics for the kind.
        let plain = validate_for_adapter(&adapter, Path::new("/tmp/other.json"), b"{ bad");
        assert!(!plain.is_empty());
    }

    #[test]
    fn surface_for_path_matches_surface_id_in_path() {
        let adapter = SchemaEraAdapter;
        let surface = surface_for_path(&adapter, Path::new("/tmp/root/config.toml")).unwrap();
        assert_eq!(surface.id, "config.toml");
        assert!(surface_for_path(&adapter, Path::new("/tmp/root/other.txt")).is_none());
    }
}
