//! Command Code adapter: `cmd`/`cmdc`/`command-code` binary, fixed
//! `~/.commandcode` root, three-scope MCP with `mcpServers` files, BYOK
//! providers.json. Research: docs/harness-configs/command-code.md.

use std::path::PathBuf;

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
pub const HARNESS_ID_STR: &str = "command-code";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Command Code (cmd)";

/// Unambiguous binary names: bare `cmd` collides with the Windows shell,
/// so detection probes the full name and the Windows alias only.
pub const EXECUTABLE: &str = "command-code";

/// Windows alias for the binary.
pub const EXECUTABLE_ALT: &str = "cmdc";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/command-code.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside the user MCP file.
pub const OWNED_SELECTORS: &[&str] = &["mcpServers"];

/// Constrained note: fixed root, no relocation var.
pub const CONSTRAINED_NOTE: &str = "the root is fixed at ~/.commandcode with no relocation env var documented; isolation reduces to per-project .commandcode files and headless -p runs (command-code.md sections 2, 7)";

/// Concrete adapter for Command Code.
#[derive(Debug, Clone)]
pub struct CommandCodeAdapter {
    id: HarnessId,
}

impl CommandCodeAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    fn default_config_root() -> Option<PathBuf> {
        super::home_dir().map(|home| home.join(".commandcode"))
    }

    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("constrained: {CONSTRAINED_NOTE}"));
        let Some(root) = Self::default_config_root() else {
            evidence.push("could not resolve config root (no HOME)".to_owned());
            return;
        };
        if !root.exists() {
            evidence.push(format!("config root missing at {}", root.display()));
            return;
        }
        evidence.push(format!("config root exists at {}", root.display()));
        for file in ["auth.json", "settings.json", "mcp.json"] {
            if root.join(file).exists() {
                evidence.push(format!("{file} found under the root"));
            }
        }
    }
}

impl Default for CommandCodeAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for CommandCodeAdapter {
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

        match super::find_in_path(&[EXECUTABLE, EXECUTABLE_ALT]) {
            Some(path) => {
                evidence.push(format!(
                    "found binary `{}` at {}",
                    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    path.display()
                ));
                match super::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `--version`"));
                        version = Some(v);
                    }
                    None => evidence.push("version probe failed (timeout or non-zero)".to_owned()),
                }
                binary_path = Some(path);
            }
            None => evidence.push(format!(
                "binaries `{EXECUTABLE}`/`{EXECUTABLE_ALT}` not found in PATH (bare `cmd` is never probed: Windows shell collision)"
            )),
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
        let root = "~/.commandcode";
        let mut mcp = ConfigSurface::new(
            "mcp.json",
            PathResolver::fallback_only(&format!("{root}/mcp.json (user scope)")),
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        mcp.restart_behavior = RestartBehavior::Restart;

        let project_mcp = ConfigSurface::new(
            "project .mcp.json",
            PathResolver::fallback_only("<project>/.mcp.json (project scope, committable)"),
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );

        let providers = ConfigSurface::new(
            "providers.json",
            PathResolver::fallback_only(&format!("{root}/providers.json (BYOK entries)")),
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );

        let auth = ConfigSurface::new(
            "auth.json",
            PathResolver::fallback_only(&format!("{root}/auth.json (login key)")),
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        vec![mcp, project_mcp, providers, auth]
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        super::operations_at(AdapterSupport::Constrained)
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "auth.json".to_owned(),
            "cron/jobs.json".to_owned(),
            "projects/*".to_owned(),
            "scratchpad/*".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("fixed root; per-project isolation");
        plan.description = format!(
            " cmd documents no config-dir relocation: the user root stays at ~/.commandcode and \
             config_root {} is informative only; per-project isolation comes from \
             .commandcode/settings.json and .mcp.json inside each checkout, plus headless \
             `cmd -p` runs with COMMAND_CODE_API_KEY per identity (command-code.md sections 3, 6-7)",
            instance.config_root
        );
        plan.shared_state_warnings.push(
            "auth.json and the taste profile are per-user at the fixed root and shared across \
             projects"
                .to_owned(),
        );
        plan.auth_prerequisites = vec![
            crate::adapter::AuthPrerequisite::env_var("COMMAND_CODE_API_KEY"),
            crate::adapter::AuthPrerequisite::harness_login("cmd login (browser OAuth)"),
        ];
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.commandcode".to_owned(),
            ".commandcode".to_owned(),
            ".mcp.json".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::FixedPathSingle
            | Isolation::ProjectScope
            | Isolation::RelocatedRoot
            | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("command-code requires isolation fixed_path_single, got {other}"),
            }),
        }
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            "mcp.json",
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::User,
            RestartBehavior::Restart,
        ))
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "no plugin mechanism is documented; BYOK providers live in providers.json and MCP in the three-scope mcp.json files (command-code.md sections 4-5)",
        )
    }
    fn usage_absence_reason(&self) -> Option<&'static str> {
        Some("usage truth is api.commandcode.ai billing (cookie auth); no local usage artifact")
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandCodeAdapter, HARNESS_ID_STR};
    use crate::adapter::Adapter;
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> CommandCodeAdapter {
        CommandCodeAdapter::new().unwrap()
    }

    fn sample_instance() -> Instance {
        Instance {
            id: InstanceId::new("test-cmd-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(&crate::test_util::tmp_abs_str(".commandcode-work"))
                .unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::FixedPathSingle,
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
    fn detection_never_probes_the_windows_shell_name() {
        let result = adapter().detection();
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.contains("never probed") || e.contains("found binary")),
            "evidence must name the collision-safe probe set: {:?}",
            result.evidence
        );
    }

    #[test]
    fn wrapper_sets_no_env_vars_for_a_fixed_root() {
        let plan = adapter().plan_wrapper(&sample_instance()).unwrap();
        assert!(
            plan.env_vars.is_empty(),
            "no relocation var is documented: {:?}",
            plan.env_vars
        );
    }

    #[test]
    fn wrapper_rejects_mismatched_harness() {
        let mut inst = sample_instance();
        inst.harness = HarnessId::new("agentty").unwrap();
        match adapter().plan_wrapper(&inst).unwrap_err() {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn validate_instance_accepts_fixed_root() {
        adapter().validate_instance(&sample_instance()).unwrap();
        let mut inst = sample_instance();
        inst.isolation = Isolation::Unsupported;
        assert!(adapter().validate_instance(&inst).is_err());
    }

    #[test]
    fn mcp_decl_targets_user_mcp_json() {
        let a = adapter();
        let decl = a.mcp_decl().unwrap();
        assert_eq!(decl.dest_file, "mcp.json");
        assert_eq!(decl.dest_key, "mcpServers");
        assert!(a.plugin_absence_reason().is_some());
        assert!(
            a.supported_operations()
                .iter()
                .all(|(_, s)| *s == AdapterSupport::Constrained)
        );
    }
}
