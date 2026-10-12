//! Oh My `OpenCode` adapter (retired plugin lineage): the opensoft fork of
//! the opencode plugin renamed upstream to oh-my-openagent (the `OmO` CLI,
//! catalogued as omo-ai). Migration-only entry. Research:
//! docs/harness-configs/oh-my-opencode.md.

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
pub const HARNESS_ID_STR: &str = "oh-my-opencode";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Oh My OpenCode (plugin)";

/// Host binary the plugin loads inside (the opencode CLI).
pub const HOST_EXECUTABLE: &str = "opencode";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/oh-my-opencode.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Harness id migration guidance points users to.
pub const SUCCESSOR_ID: &str = "omo-ai";

/// Migration tip carried by evidence and the wrapper description.
pub const MIGRATION_TIP: &str = "the npm name oh-my-opencode now publishes the OmO CLI (successor omo-ai), not the plugin; the plugin lineage survives only as the dormant opensoft fork (last push 2026-01-26); `omo setup` imports OpenCode-edition config, keys, MCP servers, and skills";

/// Concrete adapter for the retired plugin lineage.
#[derive(Debug, Clone)]
pub struct OhMyOpencodeAdapter {
    id: HarnessId,
}

impl OhMyOpencodeAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }
}

impl Default for OhMyOpencodeAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for OhMyOpencodeAdapter {
    fn id(&self) -> HarnessId {
        self.id.clone()
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    fn product_status(&self) -> ProductStatus {
        ProductStatus::Retired
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
        let mut evidence = vec![format!(
            "retired lineage; successor {SUCCESSOR_ID} ({MIGRATION_TIP})"
        )];
        let mut version: Option<String> = None;
        let mut host_found = false;

        match super::find_in_path(&[HOST_EXECUTABLE]) {
            Some(path) => {
                host_found = true;
                evidence.push(format!(
                    "host binary `{HOST_EXECUTABLE}` found at {}; the plugin itself ships no executable",
                    path.display()
                ));
                match super::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!(
                            "host opencode version `{v}`; plugin v3.0 requires newer than 1.0.132"
                        ));
                        version = Some(v);
                    }
                    None => evidence.push("host opencode version probe failed".to_owned()),
                }
            }
            None => {
                evidence.push(format!("host binary `{HOST_EXECUTABLE}` not found in PATH"));
            }
        }

        let present = super::install_presence(host_found, version.is_some());
        let confidence = super::detection_confidence(host_found, version.is_some(), false);
        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution_from(&self, detection: &DetectionResult) -> VersionResolution {
        super::resolution_from_detection(detection, HARNESS_ID_STR, SCHEMA_VERSION_STR)
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        vec![ConfigSurface::new(
            "oh-my-opencode.json",
            PathResolver::fallback_only(
                "~/.config/opencode/oh-my-opencode.json and .opencode/oh-my-opencode.json (JSONC)",
            ),
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        )]
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        super::operations_at(AdapterSupport::MigrationOnly)
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        Vec::new()
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("retired plugin lineage; migrate to omo-ai");
        plan.description = format!(
            " {MIGRATION_TIP}; the plugin owned no state outside OpenCode's config tree, so \
             config_root {} is informative only and isolation reduces to OpenCode's own levers \
             (oh-my-opencode.md sections 2, 4)",
            instance.config_root
        );
        plan.shared_state_warnings.push(
            "the opensoft fork is dormant; the npm name now installs a different product (the \
             OmO CLI)"
                .to_owned(),
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/opencode/oh-my-opencode.json".to_owned(),
            ".opencode/oh-my-opencode.json".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::RelocatedRoot
            | Isolation::ProjectScope
            | Isolation::ExplicitConfig
            | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("oh-my-opencode requires isolation relocated_root, got {other}"),
            }),
        }
    }

    fn mcp_absence_reason(&self) -> Option<&'static str> {
        Some(
            "curated MCP servers are configured through OpenCode's own mcp surface, owned by the opencode adapter; the plugin has no MCP destination of its own (oh-my-opencode.md section 3)",
        )
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "the product IS an OpenCode plugin, installed as an entry in OpenCode's plugin array; that mechanism belongs to the opencode adapter and the lineage is retired (oh-my-opencode.md sections 1-2)",
        )
    }
    fn usage_surface(&self) -> Option<UsageSurface> {
        Some(
            UsageSurface::new(UsageReaderKind::HistorySqlite, Some("opencode.ai"))
                .with_artifact(
                    "history-db",
                    PathResolver::new(
                        Some("$OPENCODE_DB, else $XDG_DATA_HOME/opencode/opencode.db"),
                        Some("$OPENCODE_DB, else $XDG_DATA_HOME/opencode/opencode.db"),
                        None,
                        "~/.local/share/opencode/opencode.db (channel-suffixed on non-default channels)",
                    ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{HARNESS_ID_STR, OhMyOpencodeAdapter, SUCCESSOR_ID};
    use crate::adapter::Adapter;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> OhMyOpencodeAdapter {
        OhMyOpencodeAdapter::new().unwrap()
    }

    fn sample_instance() -> Instance {
        Instance {
            id: InstanceId::new("test-omo-plugin-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(&crate::test_util::tmp_abs_str(".omo-plugin-work"))
                .unwrap(),
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
        assert_eq!(a.product_status(), entry.product_status);
        assert_eq!(a.last_verified_date(), entry.last_verified);
    }

    #[test]
    fn detection_carries_the_rename_and_successor() {
        let result = adapter().detection();
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.contains(SUCCESSOR_ID) && e.contains("retired"))
        );
    }

    #[test]
    fn wrapper_sets_no_env_vars() {
        let plan = adapter().plan_wrapper(&sample_instance()).unwrap();
        assert!(
            plan.env_vars.is_empty(),
            "the plugin owns no state outside OpenCode's tree: {:?}",
            plan.env_vars
        );
    }

    #[test]
    fn validate_instance_accepts_matching_harness() {
        adapter().validate_instance(&sample_instance()).unwrap();
        let mut inst = sample_instance();
        inst.harness = HarnessId::new("omo-ai").unwrap();
        assert!(adapter().validate_instance(&inst).is_err());
    }

    #[test]
    fn mcp_and_plugin_absence_delegate_to_opencode() {
        let a = adapter();
        assert!(a.mcp_absence_reason().unwrap().contains("opencode"));
        assert!(a.plugin_absence_reason().unwrap().contains("opencode"));
        assert!(a.mcp_decl().is_none());
        assert!(
            a.supported_operations()
                .iter()
                .all(|(_, s)| *s == AdapterSupport::MigrationOnly)
        );
    }
}
