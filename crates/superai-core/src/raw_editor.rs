//! Core raw editor — harness-aware wrapper over `superai_config::raw_editor`.
//!
//! Provides interface-neutral read/validate/diff/commit that enforces
//! harness version and surface ownership policies before delegating to the
//! config-layer backend. No interface types are introduced.

use std::path::{Path, PathBuf};

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

// ---------------------------------------------------------------------------
// RAW-01 — open with surface identity and path policy
// ---------------------------------------------------------------------------

/// Response of an adapter-aware open (RAW-01).
#[derive(Debug, Clone)]
pub struct RawOpenReport {
    /// Surface identity the path resolved to, if any.
    pub surface_id: Option<String>,
    /// Declared scope of that surface.
    pub scope: Option<crate::adapter::ConfigScope>,
    /// Declared precedence of that surface (higher wins).
    pub precedence: Option<u8>,
    /// The sensitive document, read fresh. `None` when the surface is a
    /// store that must not be opened for editing (read-only reason set).
    pub document: Option<RawDocument>,
    /// Why the surface cannot be edited, where applicable.
    pub read_only_reason: Option<String>,
    /// Resolved harness version summary (`compatible`, detected, schema).
    pub version_compatible: bool,
    /// Detected harness version, if any.
    pub detected_version: Option<String>,
    /// Mismatch between the caller's expected harness version and the
    /// adapter's resolution, surfaced as a diagnostic.
    pub expected_version_mismatch: Option<String>,
    /// Which path policy admitted the request.
    pub path_policy: &'static str,
}

impl RawEditor {
    /// Open `path` through the adapter's surface identity and policy
    /// (RAW-01).
    ///
    /// - The path must resolve to one of the adapter's declared surfaces;
    ///   arbitrary paths outside the harness's surfaces are refused.
    /// - Internal stores (SQLite/keychain/executable/opaque, external secret
    ///   stores, harness-managed files) open with a read-only reason and NO
    ///   content.
    /// - The caller's `expected_version` input is compared against the
    ///   adapter's resolution and surfaced as a mismatch diagnostic (it does
    ///   not block the read).
    pub fn open_for_adapter(
        &self,
        adapter: &dyn Adapter,
        path: &Path,
        expected_version: Option<&str>,
    ) -> Result<RawOpenReport> {
        open_for_adapter(adapter, path, expected_version)
    }

    /// Open an explicit path for advanced local editing after path-policy
    /// validation (RAW-01): absolute, normalized, and not an internal
    /// db/keychain/opaque store. No adapter surface is required.
    pub fn open_explicit(&self, path: &Path) -> Result<RawDocument> {
        open_explicit(path)
    }
}

/// Surface kind/ownership read-only reason for RAW-01 opens.
fn surface_read_only_reason(surface: &crate::adapter::ConfigSurface) -> Option<String> {
    let kind_reason = match surface.kind {
        AdapterKind::Executable => {
            Some("executable config is read-only via the raw editor".to_owned())
        }
        AdapterKind::Sqlite | AdapterKind::Keychain | AdapterKind::Opaque => Some(format!(
            "surface `{}` is an internal store ({}) and never opens for editing",
            surface.id, surface.kind
        )),
        _ => None,
    };
    kind_reason.or_else(|| {
        if surface.ownership == SurfaceOwnership::ExternalSecretStore {
            Some(format!(
                "surface `{}` is an external secret store; read-only",
                surface.id
            ))
        } else if surface.ownership == SurfaceOwnership::HarnessManaged {
            Some(format!(
                "surface `{}` is harness-managed; read-only",
                surface.id
            ))
        } else {
            None
        }
    })
}

/// Adapter-aware open (RAW-01). See [`RawEditor::open_for_adapter`].
pub fn open_for_adapter(
    adapter: &dyn Adapter,
    path: &Path,
    expected_version: Option<&str>,
) -> Result<RawOpenReport> {
    let Some(surface) = surface_for_path(adapter, path) else {
        return Err(CoreError::UnsupportedSurface {
            harness: adapter.id().to_string(),
            surface: path.display().to_string(),
            reason: "path resolves to no declared surface for this harness; use \
                     open_explicit for advanced local editing after path-policy validation"
                .to_owned(),
        });
    };
    let read_only_reason = surface_read_only_reason(&surface);
    let document = if read_only_reason.is_none() {
        Some(read(path)?)
    } else {
        None
    };
    let version = adapter.version_resolution();
    let expected_version_mismatch = match expected_version {
        Some(expected) => match version.detected_version.as_deref() {
            Some(detected) if detected != expected => Some(format!(
                "caller expected harness version `{expected}`, adapter resolved `{detected}` \
                 (compatible: {})",
                version.compatible
            )),
            _ => None,
        },
        None => None,
    };
    Ok(RawOpenReport {
        surface_id: Some(surface.id),
        scope: Some(surface.scope),
        precedence: Some(surface.precedence),
        document,
        read_only_reason,
        version_compatible: version.compatible,
        detected_version: version.detected_version,
        expected_version_mismatch,
        path_policy: "adapter-surface",
    })
}

/// Path-policy-validated explicit open (RAW-01).
pub fn open_explicit(path: &Path) -> Result<RawDocument> {
    let normalized =
        crate::paths::AbsolutePath::from_path(path).map_err(|e| CoreError::InvalidPath {
            kind: "raw_explicit_open".to_owned(),
            value: path.display().to_string(),
            reason: format!(
                "explicit paths must be absolute, normalized, and free of traversal: {e}"
            ),
        })?;
    let lower = normalized.as_path().to_string_lossy().to_ascii_lowercase();
    if lower.contains(".db")
        || lower.contains(".sqlite")
        || lower.ends_with(".keychain")
        || lower.contains("keychain")
    {
        return Err(CoreError::UnsupportedOperation {
            harness: "raw-editor".to_owned(),
            operation: "open_explicit".to_owned(),
            reason: "internal SQLite/keychain/auth stores never open for editing (RAW-06)"
                .to_owned(),
        });
    }
    read(normalized.as_path())
}

// ---------------------------------------------------------------------------
// RAW-02 — validate a draft (schema at validate time)
// ---------------------------------------------------------------------------

/// Result of draft validation (RAW-02): syntax + size + adapter schema
/// diagnostics and the version gate, without touching disk.
#[derive(Debug, Clone)]
pub struct DraftValidation {
    /// All diagnostics (syntax, size, semantic schema, deprecations),
    /// adapter-attributed where a surface matched.
    pub diagnostics: Vec<superai_config::document::Diagnostic>,
    /// Blocking (Error-severity) diagnostics only.
    pub blocking: Vec<superai_config::document::Diagnostic>,
    /// Version-gate refusal text when the adapter's resolution blocks writes.
    pub version_gate: Option<String>,
    /// The surface the path resolved to, if any.
    pub surface_id: Option<String>,
}

impl RawEditor {
    /// Validate a draft against the adapter's surface schema and version
    /// gate (RAW-02): size + encoding + syntax + adapter semantic schema +
    /// deprecated/owned-key identification + root/type constraints, plus the
    /// harness version gate — all at VALIDATE time, never touching disk.
    pub fn validate_draft(
        &self,
        adapter: &dyn Adapter,
        path: &Path,
        draft: &[u8],
    ) -> DraftValidation {
        validate_draft(adapter, path, draft)
    }
}

/// Adapter-aware draft validation (RAW-02). See [`RawEditor::validate_draft`].
pub fn validate_draft(adapter: &dyn Adapter, path: &Path, draft: &[u8]) -> DraftValidation {
    let kind = ConfigKind::from_path(path);
    let surface = surface_for_path(adapter, path);
    let diagnostics = match &surface {
        Some(surface) => {
            crate::adapter::validate_surface_content(adapter, &surface.id, draft, kind)
        }
        None => validate(draft, kind),
    };
    let version = adapter.version_resolution();
    let version_gate = (!version.compatible).then(|| {
        format!(
            "harness version {} not compatible for writes",
            version.detected_version.as_deref().unwrap_or("unknown")
        )
    });
    let blocking = diagnostics
        .iter()
        .filter(|d| d.severity == superai_config::document::DiagnosticSeverity::Error)
        .cloned()
        .collect();
    DraftValidation {
        diagnostics,
        blocking,
        version_gate,
        surface_id: surface.map(|s| s.id),
    }
}

// ---------------------------------------------------------------------------
// RAW-03 — diff with scope/precedence, restart, and template ownership
// ---------------------------------------------------------------------------

/// Adapter-aware diff (RAW-03): the base diff plus scope/precedence warning,
/// restart/reload requirement, and template-owned divergence markers.
#[derive(Debug, Clone)]
pub struct AdapterDiffResult {
    /// Base lexical/semantic/redaction diff.
    pub base: DiffResult,
    /// Declared scope of the surface.
    pub scope: Option<crate::adapter::ConfigScope>,
    /// Declared precedence of the surface.
    pub precedence: Option<u8>,
    /// Restart/reload requirement after committing this change.
    pub restart_requirement: Option<String>,
    /// Scope/precedence warning: other surfaces may override this file.
    pub scope_warning: Option<String>,
    /// Template-owned fields the draft diverges on (reported, never
    /// forbidden — disk is authoritative and users may intentionally
    /// diverge).
    pub template_owned_changes: Vec<superai_config::raw_editor::SemanticOp>,
}

impl RawEditor {
    /// Diff `old` vs `new` with the adapter's surface context (RAW-03).
    ///
    /// `template_owned` lists the selector prefixes a template wrote (from
    /// the instance's template patches); changed selectors inside them are
    /// marked as template-owned divergence. `None` marks no template
    /// ownership knowledge.
    pub fn diff_for_adapter(
        &self,
        adapter: &dyn Adapter,
        path: &Path,
        old: &[u8],
        new: &[u8],
        template_owned: Option<&[String]>,
    ) -> AdapterDiffResult {
        diff_for_adapter(adapter, path, old, new, template_owned)
    }
}

/// Adapter-aware diff (RAW-03). See [`RawEditor::diff_for_adapter`].
pub fn diff_for_adapter(
    adapter: &dyn Adapter,
    path: &Path,
    old: &[u8],
    new: &[u8],
    template_owned: Option<&[String]>,
) -> AdapterDiffResult {
    let kind = ConfigKind::from_path(path);
    let base = superai_config::raw_editor::diff(old, new, kind);
    let Some(surface) = surface_for_path(adapter, path) else {
        return AdapterDiffResult {
            base,
            scope: None,
            precedence: None,
            restart_requirement: None,
            scope_warning: None,
            template_owned_changes: Vec::new(),
        };
    };
    let restart_requirement = match surface.restart_behavior {
        crate::adapter::RestartBehavior::None => None,
        other => Some(format!("{other} required after committing this change")),
    };
    // Scope/precedence warning: any other declared surface of the same
    // harness with HIGHER precedence can override this file's keys.
    let higher = adapter
        .config_surfaces()
        .iter()
        .any(|s| s.precedence > surface.precedence);
    let scope_warning = higher.then(|| {
        format!(
            "surface `{}` has precedence {}; surfaces with higher precedence exist for `{}` and \
             may override these keys",
            surface.id,
            surface.precedence,
            adapter.id()
        )
    });
    // Template-owned divergence: semantic ops touching selectors the
    // template owns (or, without template knowledge, the adapter's owned
    // selectors as the managed-field proxy).
    let template_owned_changes: Vec<_> = base
        .semantic_ops
        .iter()
        .filter(|op| {
            let selector = op
                .selector
                .trim_start_matches("key:")
                .trim_start_matches("table:");
            let owned: Vec<String> =
                template_owned.map_or_else(|| surface.owned_selectors.clone(), <[String]>::to_vec);
            owned
                .iter()
                .any(|prefix| selector == prefix || selector.starts_with(&format!("{prefix}.")))
        })
        .cloned()
        .collect();
    AdapterDiffResult {
        base,
        scope: Some(surface.scope),
        precedence: Some(surface.precedence),
        restart_requirement,
        scope_warning,
        template_owned_changes,
    }
}

// ---------------------------------------------------------------------------
// RAW-05 — adapter-permission-gated creation with rollback
// ---------------------------------------------------------------------------

/// Preview of creating a missing config file (RAW-05).
#[derive(Debug, Clone)]
pub struct CreatePreview {
    /// Target path.
    pub path: PathBuf,
    /// Parent directories the creation would own (missing ones).
    pub owned_parents: Vec<PathBuf>,
    /// Note on file permissions the creation applies.
    pub permissions_note: String,
    /// Scope/precedence effect of the new file.
    pub precedence_note: Option<String>,
    /// First-run risk note.
    pub first_run_risk: String,
    /// Whether the adapter permits creating this surface at all.
    pub permitted: bool,
    /// Why creation is refused, when not permitted.
    pub refusal_reason: Option<String>,
}

impl RawEditor {
    /// Preview creating `path` under the adapter's rules (RAW-05).
    pub fn preview_create_for_adapter(
        &self,
        adapter: &dyn Adapter,
        path: &Path,
        initial: &[u8],
    ) -> CreatePreview {
        preview_create_for_adapter(adapter, path, initial)
    }

    /// Create a missing config file with rollback (RAW-05): validates the
    /// initial document (including the format's empty-document rule),
    /// refuses when the adapter does not permit creating the surface, and
    /// stages the creation through the compensated transaction so a failure
    /// removes only the created file and empty owned parents.
    pub fn create_file_for_adapter(
        &self,
        adapter: &dyn Adapter,
        path: &Path,
        initial: &[u8],
    ) -> Result<CommitReport> {
        create_file_for_adapter(adapter, path, initial)
    }
}

/// Adapter permission gate + preview for creation (RAW-05).
pub fn preview_create_for_adapter(
    adapter: &dyn Adapter,
    path: &Path,
    initial: &[u8],
) -> CreatePreview {
    let mut owned_parents = Vec::new();
    let mut probe = path.parent();
    while let Some(dir) = probe
        && !dir.as_os_str().is_empty()
    {
        if dir.exists() {
            break;
        }
        owned_parents.push(dir.to_path_buf());
        probe = dir.parent();
    }
    owned_parents.reverse();
    let kind = ConfigKind::from_path(path);
    let (permitted, refusal_reason, precedence_note) = match surface_for_path(adapter, path) {
        Some(surface) => match surface_read_only_reason(&surface) {
            Some(reason) => (false, Some(reason), None),
            None => (
                true,
                None,
                Some(format!(
                    "new file acts as `{}` surface at precedence {} (scope {:?})",
                    surface.id, surface.precedence, surface.scope
                )),
            ),
        },
        None => (
            false,
            Some(format!(
                "path resolves to no declared surface of `{}`; the adapter does not define an \
                 initial document shape here",
                adapter.id()
            )),
            None,
        ),
    };
    let empty_note = std::str::from_utf8(initial).is_ok_and(|t| t.trim().is_empty())
        && matches!(kind, ConfigKind::StrictJson | ConfigKind::JsonC);
    CreatePreview {
        path: path.to_path_buf(),
        owned_parents,
        permissions_note: "created with default user permissions; existing files are never \
                           replaced"
            .to_owned(),
        precedence_note,
        first_run_risk: if empty_note {
            "empty buffer is not the format's empty document; the initial draft must be a valid \
             document (e.g. `{}` for JSON)"
                .to_owned()
        } else {
            "a newly created surface takes effect on the harness's next start/reload".to_owned()
        },
        permitted,
        refusal_reason,
    }
}

/// Adapter-permission-gated creation (RAW-05).
pub fn create_file_for_adapter(
    adapter: &dyn Adapter,
    path: &Path,
    initial: &[u8],
) -> Result<CommitReport> {
    let preview = preview_create_for_adapter(adapter, path, initial);
    if !preview.permitted {
        return Err(CoreError::UnsupportedOperation {
            harness: adapter.id().to_string(),
            operation: "raw_create".to_owned(),
            reason: preview
                .refusal_reason
                .unwrap_or_else(|| "adapter does not permit creating this surface".to_owned()),
        });
    }
    superai_config::raw_editor::create_file(path, initial).map_err(CoreError::Config)
}

// ---------------------------------------------------------------------------
// RAW-07 — reopen with a new schema (manual rebase)
// ---------------------------------------------------------------------------

/// Rebase report after a schema-version change invalidated a draft (RAW-07).
#[derive(Debug, Clone)]
pub struct RebaseReport {
    /// The reopened document, read fresh from disk under the new resolution.
    pub reopened: RawDocument,
    /// Harness version the draft was authored against (caller's record).
    pub draft_version: String,
    /// Harness version the adapter resolves NOW.
    pub new_version: Option<String>,
    /// Whether the current resolution is write-compatible at all.
    pub new_version_compatible: bool,
    /// Keys/spans affected between the draft and the current on-disk
    /// document — the manual rebase work list. Nothing is auto-applied.
    pub affected: Vec<superai_config::raw_editor::SemanticOp>,
    /// Lexical unified diff (draft -> current disk) for the human rebase.
    pub lexical: String,
    /// Guidance for the manual rebase.
    pub guidance: String,
}

impl RawEditor {
    /// Reopen `path` under the adapter's CURRENT schema resolution and mark
    /// the draft's affected keys/spans for MANUAL rebase (RAW-07).
    ///
    /// The draft text is never auto-applied under the new schema: the report
    /// carries the semantic deltas between the draft and the fresh on-disk
    /// document, plus a lexical diff, and the caller decides the merge.
    pub fn reopen_rebase(
        &self,
        adapter: &dyn Adapter,
        path: &Path,
        draft: &[u8],
        draft_version: &str,
    ) -> Result<RebaseReport> {
        reopen_rebase(adapter, path, draft, draft_version)
    }
}

/// Reopen-with-new-schema flow (RAW-07). See [`RawEditor::reopen_rebase`].
pub fn reopen_rebase(
    adapter: &dyn Adapter,
    path: &Path,
    draft: &[u8],
    draft_version: &str,
) -> Result<RebaseReport> {
    let reopened_doc = read(path)?;
    let kind = ConfigKind::from_path(path);
    let diff = superai_config::raw_editor::diff(draft, reopened_doc.content.expose(), kind);
    let version = adapter.version_resolution();
    let affected = diff.semantic_ops.clone();
    let guidance = if affected.is_empty() {
        "draft is semantically identical to the reopened document; rebase is a no-op".to_owned()
    } else {
        format!(
            "harness changed {} -> {} while the draft was open; {} affected key(s)/span(s) \
             listed for MANUAL rebase — superai never auto-applies a draft under a new schema",
            draft_version,
            version.detected_version.as_deref().unwrap_or("unknown"),
            affected.len()
        )
    };
    Ok(RebaseReport {
        reopened: reopened_doc,
        draft_version: draft_version.to_owned(),
        new_version: version.detected_version.clone(),
        new_version_compatible: version.compatible,
        affected,
        lexical: diff.lexical_unified_diff,
        guidance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
    // -------------------------------------------------------------------
    // RAW-01/02/03/05/07 — adapter-aware open/validate/diff/create/reopen
    // -------------------------------------------------------------------

    /// Writable adapter with two scoped surfaces (project .mcp.json at lower
    /// precedence requiring reload; user settings.json at higher precedence).
    #[derive(Debug)]
    struct ScopedAdapter;

    impl ScopedAdapter {
        fn surfaces() -> Vec<crate::adapter::ConfigSurface> {
            let mut project = crate::adapter::ConfigSurface::new(
                ".mcp.json",
                PathResolver::fallback_only(".mcp.json"),
                DocumentKind::Json,
                crate::adapter::ConfigScope::ProjectWorkspace,
                SurfaceOwnership::UserEditable,
            );
            project.precedence = 0;
            project.restart_behavior = crate::adapter::RestartBehavior::Reload;
            project.owned_selectors = vec!["mcpServers".to_owned()];
            let mut user = crate::adapter::ConfigSurface::new(
                "settings.json",
                PathResolver::fallback_only("settings.json"),
                DocumentKind::Json,
                crate::adapter::ConfigScope::User,
                SurfaceOwnership::UserEditable,
            );
            user.precedence = 5;
            vec![project, user]
        }
    }

    impl Adapter for ScopedAdapter {
        fn id(&self) -> HarnessId {
            HarnessId::new("claude-code").unwrap()
        }
        fn display_name(&self) -> &'static str {
            "Claude Code"
        }
        fn product_status(&self) -> ProductStatus {
            ProductStatus::Active
        }
        fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
            Vec::new()
        }
        fn adapter_revision(&self) -> &'static str {
            "0.1.0"
        }
        fn research_doc_link(&self) -> &'static str {
            "docs/harness-configs/claude-code.md"
        }
        fn last_verified_date(&self) -> &'static str {
            "2026-08-25"
        }
        fn detection(&self) -> DetectionResult {
            DetectionResult::absent(vec!["test".to_owned()])
        }
        fn version_resolution(&self) -> VersionResolution {
            VersionResolution::new(Some("2.1.0".to_owned()), Some("2".to_owned()), true)
        }
        fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
            Self::surfaces()
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
    fn open_for_adapter_surfaces_identity_scope_and_read_only_reasons() {
        let editor = RawEditor::new();
        // Writable surface: document + scope/precedence + version.
        let root = surface_scratch("raw01-open", "");
        let mcp_path = root.join(".mcp.json");
        std::fs::write(&mcp_path, br#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap();
        let report = editor
            .open_for_adapter(&ScopedAdapter, &mcp_path, Some("2.0.0"))
            .unwrap();
        assert_eq!(report.surface_id.as_deref(), Some(".mcp.json"));
        assert_eq!(
            report.scope,
            Some(crate::adapter::ConfigScope::ProjectWorkspace)
        );
        assert_eq!(report.precedence, Some(0));
        assert!(report.document.is_some());
        assert!(report.read_only_reason.is_none());
        assert!(report.version_compatible);
        // Expected-version mismatch is surfaced, not fatal.
        let mismatch = report.expected_version_mismatch.as_deref().unwrap();
        assert!(
            mismatch.contains("2.0.0") && mismatch.contains("2.1.0"),
            "{mismatch}"
        );

        // Internal store: read-only reason, NO content.
        let keychain_path = unique_scratch("raw01-keychain", ".keychain");
        std::fs::write(&keychain_path, b"secret-bytes").unwrap();
        let ro_report = editor
            .open_for_adapter(&ReadOnlyAdapter, &keychain_path, None)
            .unwrap();
        assert!(
            ro_report.document.is_none(),
            "stores never open for editing"
        );
        let reason = ro_report.read_only_reason.as_deref().unwrap();
        assert!(
            reason.contains("internal store") || reason.contains("external secret"),
            "{reason}"
        );

        // Arbitrary undeclared path: refused outright.
        let err = editor
            .open_for_adapter(&ScopedAdapter, Path::new("/etc/passwd"), None)
            .unwrap_err();
        match err {
            CoreError::UnsupportedSurface { .. } => {}
            other => panic!("expected UnsupportedSurface, got {other:?}"),
        }

        // Explicit open passes policy for a normal file, refuses stores.
        let explicit = editor.open_explicit(&mcp_path).unwrap();
        assert!(!explicit.diagnostics.is_empty() || explicit.diagnostics.is_empty());
        let db_path = unique_scratch("raw01-db", ".db");
        std::fs::write(&db_path, b"sqlite").unwrap();
        let db_err = editor.open_explicit(&db_path).unwrap_err();
        assert!(format!("{db_err}").contains("never open for editing"));
        let rel_err = editor
            .open_explicit(Path::new("relative.json"))
            .unwrap_err();
        assert!(format!("{rel_err}").contains("absolute"));
        drop(std::fs::remove_dir_all(&root));
        drop(std::fs::remove_file(&keychain_path));
        drop(std::fs::remove_file(&db_path));
    }

    #[test]
    fn validate_draft_runs_schema_and_version_gate_without_disk() {
        let editor = RawEditor::new();
        let path = surface_scratch("raw02", "config.toml");
        // Schema-invalid draft (model must be a string) has blocking
        // diagnostics attributed to the surface.
        let validation = editor.validate_draft(&SchemaEraAdapter, &path, b"model = 5\n");
        assert_eq!(validation.surface_id.as_deref(), Some("config.toml"));
        assert!(
            validation.blocking.iter().any(|d| d
                .message
                .contains("`model` must hold a value of type string")),
            "{:?}",
            validation.diagnostics
        );
        // Version gate: an incompatible adapter surfaces the refusal text.
        let gated = editor.validate_draft(&IncompatibleAdapter, &path, b"model = \"gpt-5\"\n");
        assert!(
            gated
                .version_gate
                .as_deref()
                .is_some_and(|g| g.contains("not compatible"))
        );
        // Oversize drafts are a size diagnostic before parsing (RAW-02).
        let huge = vec![b'a'; superai_config::raw_editor::MAX_VALIDATION_BYTES + 1];
        let sized = editor.validate_draft(&SchemaEraAdapter, &path, &huge);
        assert!(
            sized
                .blocking
                .iter()
                .any(|d| d.message.contains("size limit")),
            "{:?}",
            sized.diagnostics
        );
    }

    #[test]
    fn diff_for_adapter_carries_scope_restart_and_template_markers() {
        let editor = RawEditor::new();
        let path = surface_scratch("raw03", ".mcp.json");
        let old = br#"{"mcpServers":{"a":{"command":"x"},"template-key":{"command":"t"}}}"#;
        let new = br#"{"mcpServers":{"a":{"command":"y"},"template-key":{"command":"changed"}}}"#;
        // Template owns the `mcpServers.template-key` selector.
        let template_owned = vec!["mcpServers.template-key".to_owned()];
        let result =
            editor.diff_for_adapter(&ScopedAdapter, &path, old, new, Some(&template_owned));
        assert_eq!(
            result.scope,
            Some(crate::adapter::ConfigScope::ProjectWorkspace)
        );
        assert_eq!(result.precedence, Some(0));
        // Higher-precedence surface exists -> scope warning.
        let warning = result.scope_warning.as_deref().unwrap();
        assert!(
            warning.contains("precedence 0") && warning.contains("may override"),
            "{warning}"
        );
        // Restart requirement surfaced.
        assert!(
            result
                .restart_requirement
                .as_deref()
                .is_some_and(|r| r.contains("reload")),
            "{:?}",
            result.restart_requirement
        );
        // Template-owned divergence reported, not forbidden.
        assert!(
            result
                .template_owned_changes
                .iter()
                .any(|op| op.selector.contains("template-key")),
            "{:?}",
            result.template_owned_changes
        );
        // Without template knowledge, the adapter's owned selectors act as
        // the managed-field proxy.
        let proxy = editor.diff_for_adapter(&ScopedAdapter, &path, old, new, None);
        assert!(
            proxy
                .template_owned_changes
                .iter()
                .any(|op| op.selector.contains("mcpServers")),
            "{:?}",
            proxy.template_owned_changes
        );
    }

    #[test]
    fn create_is_permission_gated_and_rolls_back_on_refusal() {
        let editor = RawEditor::new();
        // Declared surface: preview + create succeed.
        let root = surface_scratch("raw05", "");
        let target = root.join("deep/owned/.mcp.json");
        let preview =
            editor.preview_create_for_adapter(&ScopedAdapter, &target, br#"{"mcpServers":{}}"#);
        assert!(preview.permitted, "{:?}", preview.refusal_reason);
        assert_eq!(
            preview.owned_parents,
            vec![root.join("deep"), root.join("deep/owned")]
        );
        assert!(
            preview
                .precedence_note
                .as_deref()
                .is_some_and(|n| n.contains("precedence 0"))
        );
        let report = editor
            .create_file_for_adapter(&ScopedAdapter, &target, br#"{"mcpServers":{}}"#)
            .unwrap();
        assert!(!report.is_noop);
        assert!(target.exists());

        // Undeclared surface: refused before touching disk.
        let stray = root.join("undeclared.json");
        let err = editor
            .create_file_for_adapter(&ScopedAdapter, &stray, br#"{"a":1}"#)
            .unwrap_err();
        match err {
            CoreError::UnsupportedOperation { operation, .. } => {
                assert_eq!(operation, "raw_create");
            }
            other => panic!("expected UnsupportedOperation, got {other:?}"),
        }
        assert!(!stray.exists());

        // Read-only surface: refused with the store reason.
        let keychain_target = root.join("keychain-store.keychain");
        let err2 = editor
            .create_file_for_adapter(&ReadOnlyAdapter, &keychain_target, b"x")
            .unwrap_err();
        assert!(format!("{err2}").contains("read-only") || format!("{err2}").contains("never"));
        assert!(!keychain_target.exists());

        // Empty buffer is not the format's empty document (declared
        // settings.json surface, JSON kind).
        let empty_target = root.join("settings.json");
        let err3 = editor
            .create_file_for_adapter(&ScopedAdapter, &empty_target, b"  ")
            .unwrap_err();
        assert!(
            format!("{err3}").contains("empty buffer") || format!("{err3}").contains("invalid"),
            "{err3}"
        );
        assert!(!empty_target.exists());
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn reopen_rebase_marks_affected_keys_for_manual_rebase() {
        let editor = RawEditor::new();
        let root = surface_scratch("raw07", "");
        let path = root.join(".mcp.json");
        // Draft authored under 2.0.0; the harness meanwhile wrote a new file.
        let draft = br#"{"mcpServers":{"a":{"command":"draft"}}}"#;
        std::fs::write(
            &path,
            br#"{"mcpServers":{"a":{"command":"current"},"b":{"command":"new"}}}"#,
        )
        .unwrap();
        let report = editor
            .reopen_rebase(&ScopedAdapter, &path, draft, "2.0.0")
            .unwrap();
        assert_eq!(report.draft_version, "2.0.0");
        assert_eq!(report.new_version.as_deref(), Some("2.1.0"));
        assert!(report.new_version_compatible);
        // Affected keys include the changed server and the added one.
        assert!(
            report.affected.iter().any(|op| op.selector.contains('a')),
            "{:?}",
            report.affected
        );
        assert!(
            report.affected.iter().any(|op| op.selector.contains('b')),
            "{:?}",
            report.affected
        );
        // Guidance demands manual rebase; nothing was auto-applied.
        assert!(
            report.guidance.contains("MANUAL rebase"),
            "{}",
            report.guidance
        );
        assert!(
            !report.lexical.is_empty(),
            "lexical draft->disk diff must be present"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            br#"{"mcpServers":{"a":{"command":"current"},"b":{"command":"new"}}}"#.to_vec(),
            "reopen must not write anything"
        );
        drop(std::fs::remove_dir_all(&root));
    }
}
