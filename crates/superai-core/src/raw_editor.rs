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

    commit(path, new_content, expected_digest)
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
}
