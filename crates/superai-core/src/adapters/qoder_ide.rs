//! Qoder IDE/Desktop adapter: UI-driven settings with undocumented storage
//! paths; the MCP JSON editor (mcpServers shape) is the one file-backed
//! write and even its on-disk path is unpublished. Research:
//! docs/harness-configs/qoder-ide.md.

use crate::adapter::{
    ADAPTER_REVISION, Adapter, ConfigSurface, DetectionResult, Platform, ProductStatus,
    VersionResolution, WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, Isolation};

/// Catalog id superai registers this harness under.
pub const HARNESS_ID_STR: &str = "qoder-ide";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Qoder IDE and Desktop";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/qoder-ide.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Constrained note: storage paths unpublished.
pub const CONSTRAINED_NOTE: &str = "no settings file path is documented for any settings family; the configurable local storage path in General settings is the only relocation-ish knob, and the CLI sibling (qoder-cli) owns the documented file surfaces (qoder-ide.md sections 2, 5-6)";

/// Concrete adapter for the Qoder IDE and Desktop surfaces.
#[derive(Debug, Clone)]
pub struct QoderIdeAdapter {
    id: HarnessId,
}

impl QoderIdeAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }
}

impl Default for QoderIdeAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for QoderIdeAdapter {
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
        let evidence = vec![
            format!("constrained: {CONSTRAINED_NOTE}"),
            "desktop app and IDE install from qoder.com after sign-up; no standalone version probe is documented".to_owned(),
        ];
        DetectionResult::new(
            crate::state::InstallPresence::Absent,
            None,
            evidence,
            crate::adapter::DetectionConfidence::Low,
        )
    }

    fn version_resolution_from(&self, detection: &DetectionResult) -> VersionResolution {
        super::resolution_from_detection(detection, HARNESS_ID_STR, SCHEMA_VERSION_STR)
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        Vec::new()
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
        let mut plan = WrapperPlan::new("UI-driven settings; storage paths unpublished");
        plan.description = format!(
            " Qoder IDE/Desktop settings are edited through the app UI with no documented \
             on-disk paths; the MCP JSON editor writes an mcpServers-shaped document at an \
             unpublished location, so config_root {} is informative only; the General local \
             storage path is the one documented relocation knob, and automation belongs to the \
             qoder-cli sibling (qoder-ide.md sections 2-3, 5-6)",
            instance.config_root
        );
        plan.shared_state_warnings.push(
            "Pro trial metering, memory, and custom agents are account-level app state; \
             enterprise admins can control MCP access org-wide"
                .to_owned(),
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        Vec::new()
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::Unknown | Isolation::RelocatedRoot | Isolation::IdeUserData => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "qoder-ide isolation is unknown (no documented storage paths), got {other}"
                ),
            }),
        }
    }

    fn mcp_absence_reason(&self) -> Option<&'static str> {
        Some(
            "the IDE's MCP JSON editor (mcpServers shape, My Servers + MCP Square) has no published on-disk path; superai cannot manage a destination it cannot locate (qoder-ide.md sections 3, 6)",
        )
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "Extension Management (skills, plugins, connectors) is a Desktop settings pane with no documented filesystem surface; skills upload as SKILL.md or ZIP through the UI (qoder-ide.md sections 4-5)",
        )
    }
    fn usage_absence_reason(&self) -> Option<&'static str> {
        Some(
            "usage truth is the qoder.com account dashboards (cookie auth); token-cost history is unsupported",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{HARNESS_ID_STR, QoderIdeAdapter};
    use crate::adapter::Adapter;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> QoderIdeAdapter {
        QoderIdeAdapter::new().unwrap()
    }

    fn sample_instance() -> Instance {
        Instance {
            id: InstanceId::new("test-qoder-ide-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(&crate::test_util::tmp_abs_str(".qoder-ide-work"))
                .unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::Unknown,
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
    fn detection_never_claims_presence_without_storage_paths() {
        let result = adapter().detection();
        assert_eq!(result.present, crate::state::InstallPresence::Absent);
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.contains("no settings file path"))
        );
    }

    #[test]
    fn config_surfaces_are_empty_by_contract() {
        assert!(
            adapter().config_surfaces().is_empty(),
            "no documented storage paths; surfaces must stay empty"
        );
    }

    #[test]
    fn wrapper_sets_no_env_vars() {
        let plan = adapter().plan_wrapper(&sample_instance()).unwrap();
        assert!(plan.env_vars.is_empty());
    }

    #[test]
    fn validate_instance_accepts_unknown_isolation() {
        adapter().validate_instance(&sample_instance()).unwrap();
        let mut inst = sample_instance();
        inst.harness = HarnessId::new("qoder-cli").unwrap();
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
