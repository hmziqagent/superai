//! omp (oh-my-pi) adapter: pi-mono fork with `~/.omp/agent/{config.yml,
//! models.yml}`, `--config` YAML overlay, ACP/RPC entry points; first run
//! inherits rules, skills, and MCP from eight foreign harness dirs.
//! Research: docs/harness-configs/omp.md.

use std::path::PathBuf;

use crate::adapter::{
    ADAPTER_REVISION, Adapter, ConfigScope, ConfigSurface, DetectionResult, DocumentKind,
    PathResolver, Platform, ProductStatus, RestartBehavior, SurfaceOwnership, UsageReaderKind,
    UsageSurface, VersionResolution, WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, Isolation};

/// Catalog id superai registers this harness under.
pub const HARNESS_ID_STR: &str = "omp";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "omp (oh-my-pi)";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "omp";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/omp.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside models.yml.
pub const MODELS_OWNED_SELECTORS: &[&str] = &["providers"];

/// Owned selectors inside config.yml.
pub const CONFIG_OWNED_SELECTORS: &[&str] = &["modelRoles", "mcp"];

/// Constrained note: first-run inheritance reads half the catalog.
pub const CONSTRAINED_NOTE: &str = "on first run omp inherits rules, skills, and MCP from .claude, .cursor, .windsurf, .gemini, .codex, .cline, .github/copilot, and .vscode with no migration script, making it a read-consumer of most other harnesses (omp.md section 4)";

/// Concrete adapter for omp.
#[derive(Debug, Clone)]
pub struct OmpAdapter {
    id: HarnessId,
}

impl OmpAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    fn default_config_root() -> Option<PathBuf> {
        super::home_dir().map(|home| home.join(".omp").join("agent"))
    }

    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("constrained: {CONSTRAINED_NOTE}"));
        let Some(root) = Self::default_config_root() else {
            evidence.push("could not resolve config root (no HOME)".to_owned());
            return;
        };
        if !root.exists() {
            evidence.push(format!("agent config root missing at {}", root.display()));
            return;
        }
        evidence.push(format!("agent config root exists at {}", root.display()));
        for file in ["config.yml", "models.yml"] {
            if root.join(file).exists() {
                evidence.push(format!("{file} found under the agent root"));
            }
        }
    }
}

impl Default for OmpAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for OmpAdapter {
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
                    None => {
                        evidence.push(format!("version probe failed for `{EXECUTABLE} --version`"));
                    }
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
        let mut models = ConfigSurface::new(
            "models.yml",
            PathResolver::fallback_only(
                "~/.omp/agent/models.yml (custom providers patched onto the catalog)",
            ),
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        models.owned_selectors = MODELS_OWNED_SELECTORS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        models.restart_behavior = RestartBehavior::Restart;

        let mut config = ConfigSurface::new(
            "config.yml",
            PathResolver::fallback_only(
                "~/.omp/agent/config.yml (modelRoles, fallbackChains, path-scoped model sets)",
            ),
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        config.owned_selectors = CONFIG_OWNED_SELECTORS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        config.restart_behavior = RestartBehavior::Restart;
        vec![models, config]
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        super::operations_at(AdapterSupport::Constrained)
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        Vec::new()
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("--config YAML overlay; no root relocation var");
        plan.description = format!(
            " omp documents no config-dir relocation for ~/.omp itself; per-invocation isolation \
             is the `--config <file>` YAML overlay pointing at config_root {}, which can pin \
             model routing and disable project inputs (mcp.enableProjectConfig: false); \
             --no-extensions or --trusted-extension <path> gate the extension surface \
             (omp.md sections 2, 4)",
            instance.config_root
        );
        plan.args.push("--config".to_owned());
        plan.args.push(
            instance
                .config_root
                .as_path()
                .join("config.yml")
                .display()
                .to_string(),
        );
        plan.shared_state_warnings.push(
            "first run inherits rules, skills, and MCP from .claude/.cursor/.windsurf/.gemini/\
             .codex/.cline/.github/copilot/.vscode; those foreign trees stay shared reads"
                .to_owned(),
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec!["~/.omp".to_owned()]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::ExplicitConfig
            | Isolation::RelocatedRoot
            | Isolation::ProjectScope
            | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("omp requires isolation explicit_config, got {other}"),
            }),
        }
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(
            crate::adapter::McpAdapterDecl::new(
                ".mcp.json",
                "mcpServers",
                DocumentKind::Json,
                ConfigScope::ProjectWorkspace,
                RestartBehavior::Restart,
            )
            .with_read_only(
                "omp inherits project .mcp.json and MCP from eight foreign harness dirs on first run; no omp-owned MCP write path is documented, so superai inspects but never writes through omp (omp.md section 4)",
            ),
        )
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "extensions load from trusted paths (--trusted-extension allowlist) but no install destination or manifest format is documented (omp.md sections 4, 6)",
        )
    }
    fn usage_surface(&self) -> Option<UsageSurface> {
        Some(
            UsageSurface::new(UsageReaderKind::TranscriptJsonl, None).with_artifact(
                "agent-sessions",
                PathResolver::fallback_only(
                    "~/.omp/agent session stores (OMP XDG roots; OMP_PROFILE-selected)",
                ),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{HARNESS_ID_STR, OmpAdapter};
    use crate::adapter::Adapter;
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> OmpAdapter {
        OmpAdapter::new().unwrap()
    }

    fn sample_instance() -> Instance {
        Instance {
            id: InstanceId::new("test-omp-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(&crate::test_util::tmp_abs_str(".omp-work")).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::ExplicitConfig,
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
    fn detection_names_the_inheritance_reads() {
        let result = adapter().detection();
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.contains(".claude") && e.contains(".codex"))
        );
    }

    #[test]
    fn wrapper_uses_a_config_overlay_per_instance() {
        let plan = adapter().plan_wrapper(&sample_instance()).unwrap();
        assert!(plan.args.contains(&"--config".to_owned()));
        assert!(plan.env_vars.is_empty());
    }

    #[test]
    fn wrapper_rejects_mismatched_harness() {
        let mut inst = sample_instance();
        inst.harness = HarnessId::new("pi").unwrap();
        match adapter().plan_wrapper(&inst).unwrap_err() {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn validate_instance_accepts_explicit_config() {
        adapter().validate_instance(&sample_instance()).unwrap();
        let mut inst = sample_instance();
        inst.isolation = Isolation::Unsupported;
        assert!(adapter().validate_instance(&inst).is_err());
    }

    #[test]
    fn mcp_decl_is_read_only_and_plugins_absent() {
        let a = adapter();
        let decl = a.mcp_decl().unwrap();
        assert_eq!(decl.dest_file, ".mcp.json");
        assert!(decl.read_only.is_some());
        assert!(a.plugin_absence_reason().is_some());
        assert!(
            a.supported_operations()
                .iter()
                .all(|(_, s)| *s == AdapterSupport::Constrained)
        );
    }
}
