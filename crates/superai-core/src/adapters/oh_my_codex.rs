//! oh-my-codex (`omx`) adapter: workflow layer wrapping the codex binary;
//! `.omx/` project state, `.omx-config.json` routing, `OMX_ROOT` state
//! override; MCP is off by default and Codex-side. Research:
//! docs/harness-configs/oh-my-codex.md.

use std::path::PathBuf;

use crate::adapter::{
    ADAPTER_REVISION, Adapter, ConfigScope, ConfigSurface, DetectionResult, DocumentKind,
    PathResolver, Platform, ProductStatus, SurfaceOwnership, UsageReaderKind, UsageSurface,
    VersionResolution, WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, Isolation};

/// Catalog id superai registers this harness under.
pub const HARNESS_ID_STR: &str = "oh-my-codex";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "oh-my-codex (omx)";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "omx";

/// Env var overriding the OMX state root.
pub const STATE_ENV_VAR: &str = "OMX_ROOT";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/oh-my-codex.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Constrained note: writes into Codex's own config tree.
pub const CONSTRAINED_NOTE: &str = "omx writes into Codex's own .codex/config.toml and hooks.json, so backups and preservation rules that apply to the codex-cli adapter apply here too; MCP compatibility servers ship disabled by default (oh-my-codex.md sections 2, 4)";

/// Concrete adapter for oh-my-codex.
#[derive(Debug, Clone)]
pub struct OhMyCodexAdapter {
    id: HarnessId,
}

impl OhMyCodexAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("constrained: {CONSTRAINED_NOTE}"));
        if std::path::Path::new(".omx").exists() {
            evidence
                .push(".omx state dir present in cwd (plans, logs, memory, team state)".to_owned());
        }
        if std::path::Path::new(".omx-config.json").exists() {
            evidence.push(".omx-config.json present in cwd (model and env routing)".to_owned());
        }
        if let Some(home) = super::home_dir()
            && home.join(".codex").exists()
        {
            evidence.push(
                "~/.codex present: omx reads and edits Codex config there; treat as shared with codex-cli"
                    .to_owned(),
            );
        }
    }
}

impl Default for OhMyCodexAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for OhMyCodexAdapter {
    fn id(&self) -> HarnessId {
        self.id.clone()
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    fn product_status(&self) -> ProductStatus {
        ProductStatus::Active
    }

    fn supported_platforms(&self) -> Vec<Platform> {
        super::desktop_platforms()
    }

    fn adapter_revision(&self) -> &str {
        ADAPTER_REVISION
    }

    fn research_doc_link(&self) -> &str {
        RESEARCH_DOC
    }

    fn last_verified_date(&self) -> &str {
        LAST_VERIFIED
    }

    fn detection(&self) -> DetectionResult {
        let mut evidence = Vec::new();
        let mut version: Option<String> = None;
        let mut binary_path: Option<PathBuf> = None;

        match super::find_in_path(&[EXECUTABLE]) {
            Some(path) => {
                evidence.push(format!("found binary `{EXECUTABLE}` at {}", path.display()));
                match super::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `{EXECUTABLE} --version`"));
                        version = Some(v);
                    }
                    None => evidence.push(
                        "version probe failed; the HUD prints the running version but no dedicated version command is documented (oh-my-codex.md section 1)"
                            .to_owned(),
                    ),
                }
                binary_path = Some(path);
            }
            None => evidence.push(format!("binary `{EXECUTABLE}` not found in PATH")),
        }

        self.collect_config_evidence(&mut evidence);

        let present = super::install_presence(binary_path.is_some(), version.is_some());
        let confidence =
            super::detection_confidence(binary_path.is_some(), version.is_some(), false);
        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution_from(&self, detection: &DetectionResult) -> VersionResolution {
        super::resolution_from_detection(detection, HARNESS_ID_STR, SCHEMA_VERSION_STR)
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let routing = ConfigSurface::new(
            ".omx-config.json",
            PathResolver::fallback_only("<repo>/.omx-config.json (model and env routing)"),
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        let state = ConfigSurface::new(
            ".omx state",
            PathResolver::fallback_only(
                "$OMX_ROOT or <repo>/.omx/ (plans, missions, hooks, hud-config, team state)",
            ),
            DocumentKind::Opaque,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::HarnessManaged,
        );
        vec![routing, state]
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        super::operations_at(AdapterSupport::Constrained)
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![".omx/missions/*".to_owned(), ".omx/logs/*".to_owned()]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("OMX_ROOT state override over CODEX_HOME isolation");
        plan.description = format!(
            " omx state moves with {STATE_ENV_VAR} (config_root {}); provider and account \
             separation defers to Codex itself, so pair this with CODEX_HOME relocation; omx \
             worktrees (--worktree) are the documented parallelism story, and skills/hooks live \
             per checkout under .omx/ (oh-my-codex.md sections 3, 5)",
            instance.config_root
        );
        plan.env_vars
            .push((STATE_ENV_VAR.to_owned(), instance.config_root.to_string()));
        plan.shared_state_warnings.push(
            "omx edits .codex/config.toml and hooks.json inside CODEX_HOME; the codex-cli \
             adapter's backup rules apply to every write"
                .to_owned(),
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            ".omx".to_owned(),
            ".omx-config.json".to_owned(),
            "$OMX_ROOT".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::RelocatedRoot
            | Isolation::ProjectScope
            | Isolation::EnvOnly
            | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("oh-my-codex requires isolation relocated_root, got {other}"),
            }),
        }
    }

    fn mcp_absence_reason(&self) -> Option<&'static str> {
        Some(
            "no OMX-specific MCP config file is documented: compatibility servers ship disabled by default and Codex-side servers follow the codex-cli config surface (oh-my-codex.md section 4)",
        )
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "the plugin cache under CODEX_HOME is Codex-owned state; omx adds no plugin mechanism of its own (oh-my-codex.md section 2)",
        )
    }
    fn usage_surface(&self) -> Option<UsageSurface> {
        Some(
            UsageSurface::new(UsageReaderKind::TranscriptJsonl, Some("openai-chatgpt"))
                .with_artifact(
                    "rollouts",
                    PathResolver::new(
                        Some("$CODEX_HOME/sessions"),
                        Some("$CODEX_HOME/sessions"),
                        Some("%CODEX_HOME%\\sessions"),
                        "~/.codex/sessions",
                    ),
                )
                .with_artifact(
                    "rollouts-archived",
                    PathResolver::new(
                        Some("$CODEX_HOME/archived_sessions"),
                        Some("$CODEX_HOME/archived_sessions"),
                        Some("%CODEX_HOME%\\archived_sessions"),
                        "~/.codex/archived_sessions",
                    ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{HARNESS_ID_STR, OhMyCodexAdapter, STATE_ENV_VAR};
    use crate::adapter::Adapter;
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> OhMyCodexAdapter {
        OhMyCodexAdapter::new().unwrap()
    }

    fn sample_instance() -> Instance {
        Instance {
            id: InstanceId::new("test-omx-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(&crate::test_util::tmp_abs_str(".omx-work")).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-10-12T00:00:00Z".to_owned(),
            adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
        }
    }

    #[test]
    fn adapter_identity_matches_catalog() {
        let a = adapter();
        let entry = crate::harness_catalog::find_by_id(HARNESS_ID_STR).unwrap();
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(a.display_name(), entry.display_name);
        assert_eq!(a.last_verified_date(), entry.last_verified);
    }

    #[test]
    fn detection_names_the_codex_write_coupling() {
        let result = adapter().detection();
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.contains(".codex/config.toml"))
        );
    }

    #[test]
    fn wrapper_sets_only_the_state_root_var() {
        let plan = adapter().plan_wrapper(&sample_instance()).unwrap();
        assert_eq!(plan.env_vars.len(), 1, "{:?}", plan.env_vars);
        assert_eq!(plan.env_vars[0].0, STATE_ENV_VAR);
    }

    #[test]
    fn wrapper_rejects_mismatched_harness() {
        let mut inst = sample_instance();
        inst.harness = HarnessId::new("codex-cli").unwrap();
        match adapter().plan_wrapper(&inst).unwrap_err() {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        adapter().validate_instance(&sample_instance()).unwrap();
        let mut inst = sample_instance();
        inst.isolation = Isolation::Unsupported;
        assert!(adapter().validate_instance(&inst).is_err());
    }

    #[test]
    fn mcp_and_plugin_absence_are_explicit() {
        let a = adapter();
        assert!(a.mcp_absence_reason().is_some());
        assert!(a.mcp_decl().is_none());
        assert!(a.plugin_absence_reason().is_some());
        assert!(
            a.supported_operations()
                .iter()
                .all(|(_, s)| *s == AdapterSupport::Constrained)
        );
    }
}
