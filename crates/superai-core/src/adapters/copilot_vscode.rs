//! VS Code Copilot Agent Mode adapter: the IDE surface of GitHub Copilot
//! (settings keys, workspace `.vscode/mcp.json`, portable `.mcp.json`);
//! read-shares `~/.copilot` with copilot-cli. Research:
//! docs/harness-configs/copilot-vscode.md.

use crate::adapter::{
    ADAPTER_REVISION, Adapter, ConfigScope, ConfigSurface, DetectionResult, DocumentKind,
    PathResolver, Platform, ProductStatus, RestartBehavior, SurfaceOwnership, VersionResolution,
    WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, Isolation};

/// Catalog id superai registers this harness under.
pub const HARNESS_ID_STR: &str = "copilot-vscode";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "VS Code Copilot Agent Mode";

/// Host editor binary probed during detection; the agent ships inside VS Code.
pub const EXECUTABLE: &str = "code";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/copilot-vscode.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside the IDE settings surface.
pub const OWNED_SELECTORS: &[&str] = &[
    "chat.agent.enabled",
    "github.copilot.chat.codeGeneration.useInstructionFiles",
];

/// Constrained note: IDE user-data isolation, shared ~/.copilot reads.
pub const CONSTRAINED_NOTE: &str = "isolation is VS Code's own --user-data-dir/profile machinery; the surface read-shares $COPILOT_HOME/mcp-config.json and ~/.copilot/copilot-instructions.md with copilot-cli read-only (copilot-vscode.md sections 4-5)";

/// Concrete adapter for VS Code Copilot Agent Mode.
#[derive(Debug, Clone)]
pub struct CopilotVscodeAdapter {
    id: HarnessId,
}

impl CopilotVscodeAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }
}

impl Default for CopilotVscodeAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for CopilotVscodeAdapter {
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
        let mut evidence = vec![format!("constrained: {CONSTRAINED_NOTE}")];
        let mut version: Option<String> = None;
        let mut binary_found = false;

        match super::find_in_path(&[EXECUTABLE]) {
            Some(path) => {
                binary_found = true;
                evidence.push(format!(
                    "host binary `{EXECUTABLE}` found at {}",
                    path.display()
                ));
                match super::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("VS Code version `{v}` via `code --version`"));
                        version = Some(v);
                    }
                    None => evidence.push("`code --version` probe failed".to_owned()),
                }
            }
            None => evidence.push(format!("host binary `{EXECUTABLE}` not found in PATH")),
        }
        for marker in [
            ".vscode/mcp.json",
            ".mcp.json",
            ".github/copilot-instructions.md",
        ] {
            if std::path::Path::new(marker).exists() {
                evidence.push(format!("{marker} present in cwd (workspace scope)"));
            }
        }

        let present = super::install_presence(binary_found, version.is_some());
        let confidence = super::detection_confidence(binary_found, version.is_some(), false);
        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution_from(&self, detection: &DetectionResult) -> VersionResolution {
        super::resolution_from_detection(detection, HARNESS_ID_STR, SCHEMA_VERSION_STR)
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut settings = ConfigSurface::new(
            "settings.json",
            PathResolver::fallback_only(
                "VS Code settings.json at user, profile, or workspace scope",
            ),
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        settings.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        settings.restart_behavior = RestartBehavior::Restart;

        let workspace_mcp = ConfigSurface::new(
            ".vscode/mcp.json",
            PathResolver::fallback_only("<workspace>/.vscode/mcp.json (VS Code servers shape)"),
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );

        let portable_mcp = ConfigSurface::new(
            ".mcp.json",
            PathResolver::fallback_only("<repo>/.mcp.json (portable mcpServers shape)"),
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );

        let instructions = ConfigSurface::new(
            "copilot-instructions.md",
            PathResolver::fallback_only(
                ".github/copilot-instructions.md, AGENTS.md, or ~/.copilot/copilot-instructions.md",
            ),
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        vec![settings, workspace_mcp, portable_mcp, instructions]
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
        let user_data = instance.config_root.as_path().join("vscode-data");
        let mut plan = WrapperPlan::new("ide-user-data via --user-data-dir");
        plan.description = format!(
            " VS Code agent mode isolates through the IDE: `code --user-data-dir {}` gives the \
             instance its own extensions, settings, profiles, and user-profile mcp.json; \
             workspace files (.vscode/mcp.json, .mcp.json, instruction files) travel with the \
             checkout; ~/.copilot is read-shared with copilot-cli, never written here \
             (copilot-vscode.md sections 4-5)",
            user_data.display()
        );
        plan.args.push("--user-data-dir".to_owned());
        plan.args.push(user_data.display().to_string());
        plan.state_paths
            .push(format!("user-data-dir={}", user_data.display()));
        plan.shared_state_warnings.push(
            "the Copilot subscription is account-level and shared across every user-data dir"
                .to_owned(),
        );
        plan.auth_prerequisites
            .push(crate::adapter::AuthPrerequisite::harness_login(
                "VS Code sign-in with a Copilot subscription",
            ));
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            ".vscode/mcp.json".to_owned(),
            ".mcp.json".to_owned(),
            ".github/copilot-instructions.md".to_owned(),
            ".github/instructions".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::IdeUserData
            | Isolation::ProjectScope
            | Isolation::RelocatedRoot
            | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("copilot-vscode requires isolation ide_user_data, got {other}"),
            }),
        }
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            ".vscode/mcp.json",
            "servers",
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            RestartBehavior::Restart,
        ))
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "extensions install through the VS Code marketplace under the user-data dir; no Copilot-agent plugin file surface exists for superai to stage (copilot-vscode.md section 1)",
        )
    }
    fn usage_absence_reason(&self) -> Option<&'static str> {
        Some("usage truth is GitHub's internal usage API; no local usage artifact")
    }
}

#[cfg(test)]
mod tests {
    use super::{CopilotVscodeAdapter, HARNESS_ID_STR};
    use crate::adapter::Adapter;
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> CopilotVscodeAdapter {
        CopilotVscodeAdapter::new().unwrap()
    }

    fn sample_instance() -> Instance {
        Instance {
            id: InstanceId::new("test-copilot-vscode-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(&crate::test_util::tmp_abs_str(".copilot-vscode-work"))
                .unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::IdeUserData,
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
    fn detection_names_the_read_shared_copilot_home() {
        let result = adapter().detection();
        assert!(result.evidence.iter().any(|e| e.contains("copilot-cli")));
    }

    #[test]
    fn wrapper_isolates_through_the_ide_user_data_dir() {
        let plan = adapter().plan_wrapper(&sample_instance()).unwrap();
        assert!(plan.args.contains(&"--user-data-dir".to_owned()));
        assert!(plan.env_vars.is_empty());
    }

    #[test]
    fn wrapper_rejects_mismatched_harness() {
        let mut inst = sample_instance();
        inst.harness = HarnessId::new("copilot-cli").unwrap();
        match adapter().plan_wrapper(&inst).unwrap_err() {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn validate_instance_accepts_ide_user_data() {
        adapter().validate_instance(&sample_instance()).unwrap();
        let mut inst = sample_instance();
        inst.isolation = Isolation::FixedPathSingle;
        assert!(adapter().validate_instance(&inst).is_err());
    }

    #[test]
    fn mcp_decl_uses_the_vscode_servers_shape() {
        let a = adapter();
        let decl = a.mcp_decl().unwrap();
        assert_eq!(decl.dest_file, ".vscode/mcp.json");
        assert_eq!(decl.dest_key, "servers");
        assert!(decl.read_only.is_none());
        assert!(a.plugin_absence_reason().is_some());
        assert!(
            a.supported_operations()
                .iter()
                .all(|(_, s)| *s == AdapterSupport::Constrained)
        );
    }
}
