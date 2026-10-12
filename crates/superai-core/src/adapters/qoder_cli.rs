//! Qoder CLI adapter: `qoder` binary, config root `~/.qoder` or
//! `$QODER_CONFIG_DIR`, layered JSON settings with top-level `mcpServers`.

use std::path::{Path, PathBuf};

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
pub const HARNESS_ID_STR: &str = "qoder-cli";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Qoder CLI";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "qoder";

/// Environment variable that relocates the config root.
pub const CONFIG_ENV_VAR: &str = "QODER_CONFIG_DIR";

/// Environment variable carrying an automation auth token (reference only).
pub const TOKEN_ENV_VAR: &str = "QODER_PERSONAL_ACCESS_TOKEN";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/qoder-cli.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside `settings.json`; the full key set is unpublished,
/// so unmodelled keys must survive writes verbatim.
pub const OWNED_SELECTORS: &[&str] = &[
    "mcpServers",
    "autoMemoryEnabled",
    "outputStyle",
    "language",
    "agent",
    "enabledPlugins",
    "hooks",
    "general",
    "ui",
    "model",
    "modelConfigs",
    "context",
    "tools",
    "mcp",
    "security",
    "permissions",
    "privacy",
    "advanced",
    "statusLine",
    "spellcheck",
];

/// Concrete adapter for Qoder CLI.
#[derive(Debug, Clone)]
pub struct QoderCliAdapter {
    id: HarnessId,
}

impl QoderCliAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(CONFIG_ENV_VAR)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir));
        }
        let home = super::home_dir()?;
        Some(home.join(".qoder"))
    }

    fn settings_path_for_root(root: &Path) -> PathBuf {
        root.join("settings.json")
    }

    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let cfg = Self::settings_path_for_root(&root);
                    if cfg.exists() {
                        evidence.push(format!("settings.json found at {}", cfg.display()));
                        match std::fs::read_to_string(&cfg) {
                            Ok(text) if text.contains("mcpServers") || text.contains("general") => {
                                evidence
                                    .push("settings.json contains mcpServers/general".to_owned());
                            }
                            Ok(_) => {}
                            Err(err) => evidence
                                .push(format!("config unreadable at {}: {err}", cfg.display())),
                        }
                    } else {
                        evidence.push(format!("settings.json missing at {}", cfg.display()));
                    }
                    for dir in ["skills", "commands"] {
                        let path = root.join(dir);
                        if path.exists() {
                            evidence.push(format!("{dir} dir present at {}", path.display()));
                        }
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
        if let Ok(dir) = std::env::var(CONFIG_ENV_VAR)
            && !dir.trim().is_empty()
        {
            evidence.push(format!("{CONFIG_ENV_VAR} set to {dir}"));
        } else {
            evidence.push(format!("{CONFIG_ENV_VAR} not set, using ~/.qoder"));
        }
        let project_settings = Path::new(".qoder").join("settings.json");
        if project_settings.exists() {
            evidence.push(format!(
                "project settings found at {}",
                project_settings.display()
            ));
        }
        if Path::new(".mcp.json").exists() {
            evidence.push("project .mcp.json present in cwd".to_owned());
        }
    }
}

impl Default for QoderCliAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for QoderCliAdapter {
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
                evidence.push(format!(
                    "found binary `{}` at {}",
                    EXECUTABLE,
                    path.display()
                ));
                match super::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `{EXECUTABLE} --version`"));
                        version = Some(v);
                    }
                    None => {
                        evidence.push(format!(
                            "version probe failed for `{EXECUTABLE} --version` (timeout or non-zero)"
                        ));
                    }
                }
                binary_path = Some(path);
            }
            None => {
                evidence.push(format!("binary `{EXECUTABLE}` not found in PATH"));
            }
        }

        self.collect_config_evidence(&mut evidence);

        let present = super::install_presence(binary_path.is_some(), version.is_some());

        let confidence =
            super::detection_confidence(binary_path.is_some(), version.is_some(), false);
        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution_from(&self, detection: &DetectionResult) -> VersionResolution {
        super::resolution_from_detection(detection, "qoder-cli", SCHEMA_VERSION_STR)
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let settings_resolver = PathResolver::new(
            Some("$QODER_CONFIG_DIR/settings.json"),
            Some("$QODER_CONFIG_DIR/settings.json"),
            Some("%QODER_CONFIG_DIR%\\settings.json"),
            "~/.qoder/settings.json",
        );
        let mut settings = ConfigSurface::new(
            "settings.json",
            settings_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        settings.precedence = 10;
        settings.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        settings.backup_required = true;
        settings.restart_behavior = RestartBehavior::Reload;
        surfaces.push(settings);

        let project_resolver = PathResolver::fallback_only(".qoder/settings.json (project)");
        let mut project = ConfigSurface::new(
            "project settings.json",
            project_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project.precedence = 12;
        project.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        project.backup_required = false;
        surfaces.push(project);

        let local_resolver = PathResolver::fallback_only(".qoder/settings.local.json (project)");
        let mut local = ConfigSurface::new(
            "settings.local.json",
            local_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        local.precedence = 13;
        local.owned_selectors = vec!["mcpServers".to_owned()];
        local.backup_required = false;
        surfaces.push(local);

        let project_mcp_resolver = PathResolver::fallback_only(".mcp.json (project)");
        let mut project_mcp = ConfigSurface::new(
            "project .mcp.json",
            project_mcp_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_mcp.precedence = 14;
        project_mcp.owned_selectors = vec!["mcpServers".to_owned()];
        project_mcp.backup_required = false;
        surfaces.push(project_mcp);

        let skills_resolver = PathResolver::new(
            Some("$QODER_CONFIG_DIR/skills/<name>/SKILL.md"),
            Some("$QODER_CONFIG_DIR/skills/<name>/SKILL.md"),
            Some("%QODER_CONFIG_DIR%\\skills\\<name>\\SKILL.md"),
            "~/.qoder/skills/<name>/SKILL.md",
        );
        let mut skills = ConfigSurface::new(
            "skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills.precedence = 7;
        skills.backup_required = false;
        surfaces.push(skills);

        let commands_resolver = PathResolver::new(
            Some("$QODER_CONFIG_DIR/commands/<name>.md"),
            Some("$QODER_CONFIG_DIR/commands/<name>.md"),
            Some("%QODER_CONFIG_DIR%\\commands\\<name>.md"),
            "~/.qoder/commands/<name>.md",
        );
        let mut commands = ConfigSurface::new(
            "commands",
            commands_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        commands.precedence = 6;
        commands.backup_required = false;
        surfaces.push(commands);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        super::all_operations_full()
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "sessions/*".to_owned(),
            "logs/*".to_owned(),
            "cache/*".to_owned(),
            "tmp/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "*.lock".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("relocated config via QODER_CONFIG_DIR");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        plan.description = format!(
            " Wrapper sets {}={} and execs `{}` (project files stay with the checkout)",
            CONFIG_ENV_VAR, instance.config_root, EXECUTABLE
        );
        plan.shared_state_warnings = vec![format!(
            "Qoder account sign-in is shared across profiles; {TOKEN_ENV_VAR} can carry a per-instance token"
        )];
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.qoder".to_owned(),
            "$QODER_CONFIG_DIR".to_owned(),
            ".qoder/settings.json".to_owned(),
            ".qoder/settings.local.json".to_owned(),
            ".mcp.json".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("qoder-cli requires isolation relocated_root, got {other}"),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        super::skill_modes_link_first()
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            "settings.json",
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::User,
            RestartBehavior::Reload,
        ))
    }

    fn plugin_decl(&self) -> Option<crate::adapter::PluginAdapterDecl> {
        Some(crate::adapter::PluginAdapterDecl::requires_execution(
            "qoder plugins marketplace (qoder plugins install|enable|disable)",
            "settings.json",
            crate::adapter::PluginKind::MarketplaceRecord,
            RestartBehavior::Reload,
        ))
    }
    fn usage_absence_reason(&self) -> Option<&'static str> {
        Some(
            "usage truth is the qoder.com account dashboards (cookie auth); token-cost history is unsupported",
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{CONFIG_ENV_VAR, HARNESS_ID_STR, OWNED_SELECTORS, QoderCliAdapter};
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> QoderCliAdapter {
        QoderCliAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-qoder-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
        }
    }

    #[test]
    fn adapter_identity() {
        let a = adapter();
        let entry = crate::harness_catalog::find_by_id(HARNESS_ID_STR).unwrap();
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(a.display_name(), entry.display_name);
        assert_eq!(a.product_status(), entry.product_status);
        assert_eq!(a.research_doc_link(), entry.research_doc);
        assert_eq!(a.last_verified_date(), entry.last_verified);
    }

    #[test]
    fn supported_platforms_covers_all() {
        let a = adapter();
        let os_set: HashSet<String> = a
            .supported_platforms()
            .iter()
            .map(|p| p.os.to_string())
            .collect();
        assert!(os_set.contains("linux"));
        assert!(os_set.contains("macos"));
    }

    #[test]
    fn detection_returns_evidence_and_confidence() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        match result.present {
            InstallPresence::Absent => {
                assert!(result.version.is_none());
            }
            InstallPresence::Present => {
                assert!(result.version.is_some());
            }
            InstallPresence::UnknownVersion => {
                assert!(result.evidence.iter().any(|e| e.contains("found binary")));
            }
            InstallPresence::Broken => {
                assert!(!result.evidence.is_empty());
            }
        }
    }

    #[test]
    fn version_resolution_maps_detected() {
        let a = adapter();
        let res = a.version_resolution();
        if res.detected_version.is_some() {
            assert_eq!(
                res.schema_version.as_deref(),
                Some(super::SCHEMA_VERSION_STR)
            );
            assert!(res.compatible);
        } else {
            assert!(!res.compatible);
            assert!(res.schema_version.is_none());
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn config_surfaces_include_layered_settings() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 6);
        let settings = surfaces
            .iter()
            .find(|s| s.id == "settings.json")
            .expect("settings.json surface must exist");
        assert_eq!(settings.kind, DocumentKind::Json);
        assert_eq!(settings.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(settings.scope, ConfigScope::User);
        assert!(settings.backup_required);
        for selector in ["mcpServers", "enabledPlugins", "general"] {
            assert!(
                settings.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in OWNED_SELECTORS {
            assert!(settings.owned_selectors.contains(&(*sel).to_owned()));
        }
        for other in ["settings.local.json", "project .mcp.json"] {
            assert!(surfaces.iter().any(|s| s.id == other), "missing {other}");
        }
    }

    #[test]
    fn supported_operations_cover_full() {
        let a = adapter();
        let ops = a.supported_operations();
        for (_, support) in &ops {
            assert_eq!(*support, AdapterSupport::Full);
        }
        let names: HashSet<String> = ops.iter().map(|(n, _)| n.clone()).collect();
        for required in [
            "detect",
            "read_config",
            "write_config",
            "manage_mcp",
            "manage_plugins",
            "plan_wrapper",
        ] {
            assert!(names.contains(required), "missing op {required}");
        }
    }

    #[test]
    fn plan_mirror_exclusions_cover_history_and_locks() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        let is_excluded = |file: &str| crate::adapters::exclusion_matches(&exclusions, file);
        assert!(!is_excluded("settings.json"));
        assert!(!is_excluded(".mcp.json"));
        assert!(is_excluded("sessions/abc.jsonl"));
        assert!(is_excluded("logs/qoder.log"));
    }

    #[test]
    fn plan_wrapper_sets_qoder_config_dir() {
        let tmp_root = crate::test_util::tmp_abs_str(".qoder-work");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == tmp_root.as_str())
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(CONFIG_ENV_VAR));
        assert!(plan.args.is_empty());
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let tmp_root = crate::test_util::tmp_abs_str("my qoder work");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == CONFIG_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, tmp_root.as_str());
        assert!(!env_val.contains('"'));
        assert!(env_val.contains(' '));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".qoder-work"));
        inst.harness = HarnessId::new("codex-cli").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_default_root() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains(".qoder")));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".qoder-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".qoder-work"));
        inst.isolation = Isolation::EnvOnly;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn validate_instance_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".qoder-work"));
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let settings = surfaces.iter().find(|s| s.id == "settings.json").unwrap();
        assert_eq!(settings.path_resolver.fallback, "~/.qoder/settings.json");
        assert!(
            settings
                .path_resolver
                .linux
                .as_deref()
                .unwrap()
                .contains(CONFIG_ENV_VAR)
        );
    }

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/qoder_cli")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.json");
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("settings.minimal.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.contains_key("mcpServers"));
        assert!(map.contains_key("enabledPlugins"));
        assert!(map.contains_key("general"));
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(
            original.contains_key("vendorKey") || original.contains_key("keepMe"),
            "foreign fixture must carry a foreign key"
        );
        let dir = crate::test_util::temp_dir_unique("qoder");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("settings.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "language".to_owned(),
                serde_json::Value::String("en".to_owned()),
            );
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert!(
            after.contains_key("vendorKey") || after.contains_key("keepMe"),
            "foreign keys must be preserved, got {after:?}"
        );
        drop(std::fs::remove_file(&tmp));
    }

    #[test]
    fn fixture_malformed_fails_to_parse() {
        let path = fixture_path("settings.malformed.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::json::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn mcp_decl_targets_user_settings_mcp_servers() {
        let a = adapter();
        let decl = a.mcp_decl().unwrap();
        assert_eq!(decl.dest_file, "settings.json");
        assert_eq!(decl.dest_key, "mcpServers");
        assert!(decl.read_only.is_none());
        assert!(a.mcp_absence_reason().is_none());
    }

    #[test]
    fn plugin_decl_requires_execution_against_marketplace() {
        let a = adapter();
        let decl = a.plugin_decl().unwrap();
        assert!(decl.requires_execution);
        assert!(decl.source_hint.contains("qoder plugins"));
        assert!(a.plugin_absence_reason().is_none());
    }

    #[test]
    fn adapter_is_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
        assert!(!boxed.config_surfaces().is_empty());
        assert!(!boxed.plan_mirror_exclusions().is_empty());
    }
}
