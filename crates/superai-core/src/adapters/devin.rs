//! Devin CLI adapter: local Cognition CLI with JSONC config at
//! `~/.config/devin/config.json`, project `.devin/` files, dedicated
//! `mcp_config*.json`; no config-dir env var, org settings outrank user.
//! Research: docs/harness-configs/devin.md.

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
pub const HARNESS_ID_STR: &str = "devin";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Devin CLI";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "devin";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/devin.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside the user config.
pub const OWNED_SELECTORS: &[&str] = &[
    "agent",
    "permissions",
    "mcpServers",
    "hooks",
    "read_config_from",
];

/// Constrained note: no relocation var, org settings outrank everything.
pub const CONSTRAINED_NOTE: &str = "no config-dir override env var is documented and organization/enterprise settings outrank everything a user or repo can write; the CLI runs locally but hands sessions to cloud VMs on --cloud (devin.md sections 2-3, 7)";

/// Concrete adapter for the Devin CLI.
#[derive(Debug, Clone)]
pub struct DevinAdapter {
    id: HarnessId,
}

impl DevinAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var("XDG_CONFIG_HOME")
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir).join("devin"));
        }
        super::home_dir().map(|home| home.join(".config").join("devin"))
    }

    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("constrained: {CONSTRAINED_NOTE}"));
        if std::path::Path::new(".devin").join("config.json").exists() {
            evidence.push(".devin/config.json present in cwd (project scope)".to_owned());
        }
        let Some(root) = Self::default_config_root() else {
            evidence.push("could not resolve user config root (no HOME)".to_owned());
            return;
        };
        if !root.exists() {
            evidence.push(format!("user config root missing at {}", root.display()));
            return;
        }
        evidence.push(format!("user config root exists at {}", root.display()));
        for file in ["config.json", "AGENTS.md"] {
            if root.join(file).exists() {
                evidence.push(format!("{file} found under the user root"));
            }
        }
    }
}

impl Default for DevinAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for DevinAdapter {
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
                match super::probe_version_with_fallback(
                    &path,
                    &["--version"],
                    &["version"],
                    std::time::Duration::from_secs(2),
                ) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `{EXECUTABLE} --version`"));
                        version = Some(v);
                    }
                    None => evidence.push(format!(
                        "version probe failed for `{EXECUTABLE} --version`/`version`"
                    )),
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
        let mut user_config = ConfigSurface::new(
            "config.json",
            PathResolver::new(
                Some("$XDG_CONFIG_HOME/devin/config.json"),
                Some("$XDG_CONFIG_HOME/devin/config.json"),
                Some("%APPDATA%\\devin\\config.json"),
                "~/.config/devin/config.json",
            ),
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        user_config.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        user_config.restart_behavior = RestartBehavior::Restart;

        let project_config = ConfigSurface::new(
            "project config.json",
            PathResolver::fallback_only("<repo>/.devin/config.json (committed)"),
            DocumentKind::Jsonc,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        let project_local = ConfigSurface::new(
            "project config.local.json",
            PathResolver::fallback_only("<repo>/.devin/config.local.json (auto-gitignored)"),
            DocumentKind::Jsonc,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        let agents = ConfigSurface::new(
            "AGENTS.md",
            PathResolver::fallback_only("$XDG_CONFIG_HOME/devin/AGENTS.md (global rules)"),
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        vec![user_config, project_config, project_local, agents]
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
        let mut plan = WrapperPlan::new("explicit config flag; no relocation var");
        plan.description = format!(
            " devin documents no config-dir env var; per-invocation isolation is `devin --config \
             <PATH>` over config_root {}, plus DEVIN_MODEL/DEVIN_PERMISSION_MODE/DEVIN_SANDBOX \
             overrides; org/enterprise settings outrank all of it and cannot be overridden, and \
             full state separation needs an unverified XDG_CONFIG_HOME swap (devin.md sections 3, 7)",
            instance.config_root
        );
        plan.args.push("--config".to_owned());
        plan.args.push(
            instance
                .config_root
                .as_path()
                .join("config.json")
                .display()
                .to_string(),
        );
        plan.shared_state_warnings = vec![
            "org/enterprise settings outrank user and repo config everywhere".to_owned(),
            "devin auth login state is shared across profiles; WINDSURF_API_KEY serves the ACP path".to_owned(),
        ];
        plan.auth_prerequisites
            .push(crate::adapter::AuthPrerequisite::harness_login(
                "devin auth login (browser, or --force-manual-token-flow on SSH-only machines)",
            ));
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/devin".to_owned(),
            "$XDG_CONFIG_HOME/devin".to_owned(),
            ".devin".to_owned(),
            ".devin/mcp_config.json".to_owned(),
        ]
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
                reason: format!("devin requires isolation explicit_config, got {other}"),
            }),
        }
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            ".devin/mcp_config.json",
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            RestartBehavior::Restart,
        ))
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "`devin plugins` is a first-class subcommand but the on-disk plugin layout was not published on fetched pages; no staging path may be invented (devin.md sections 6, 8)",
        )
    }
    fn usage_absence_reason(&self) -> Option<&'static str> {
        Some(
            "usage truth is Devin's quota endpoint over a localStorage session; no readable local usage artifact",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{DevinAdapter, HARNESS_ID_STR};
    use crate::adapter::Adapter;
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> DevinAdapter {
        DevinAdapter::new().unwrap()
    }

    fn sample_instance() -> Instance {
        Instance {
            id: InstanceId::new("test-devin-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(&crate::test_util::tmp_abs_str(".devin-work")).unwrap(),
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
    fn detection_names_the_org_settings_ceiling() {
        let result = adapter().detection();
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.contains("organization") || e.contains("constrained"))
        );
    }

    #[test]
    fn wrapper_uses_the_config_flag_not_an_invented_env_var() {
        let plan = adapter().plan_wrapper(&sample_instance()).unwrap();
        assert!(
            plan.env_vars.is_empty(),
            "no relocation env var is documented: {:?}",
            plan.env_vars
        );
        assert!(plan.args.contains(&"--config".to_owned()));
    }

    #[test]
    fn wrapper_rejects_mismatched_harness() {
        let mut inst = sample_instance();
        inst.harness = HarnessId::new("jules").unwrap();
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
    fn mcp_decl_targets_the_dedicated_project_file() {
        let a = adapter();
        let decl = a.mcp_decl().unwrap();
        assert_eq!(decl.dest_file, ".devin/mcp_config.json");
        assert_eq!(decl.dest_key, "mcpServers");
        assert!(a.plugin_absence_reason().is_some());
        assert!(
            a.supported_operations()
                .iter()
                .all(|(_, s)| *s == AdapterSupport::Constrained)
        );
    }
}
