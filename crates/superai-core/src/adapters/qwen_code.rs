//! Qwen Code adapter: relocated-root via `QWEN_HOME`/`QWEN_CODE_*`; layered
//! JSON settings, env, and MCP surfaces under the config root.

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
pub const HARNESS_ID_STR: &str = "qwen-code";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Qwen Code";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "qwen";

/// Primary environment variable that relocates the config root.
pub const CONFIG_ENV_VAR: &str = "QWEN_HOME";

/// System defaults path override.
pub const SYSTEM_DEFAULTS_ENV_VAR: &str = "QWEN_CODE_SYSTEM_DEFAULTS_PATH";

/// System settings path override.
pub const SYSTEM_SETTINGS_ENV_VAR: &str = "QWEN_CODE_SYSTEM_SETTINGS_PATH";

/// Runtime dir override.
pub const RUNTIME_DIR_ENV_VAR: &str = "QWEN_RUNTIME_DIR";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/qwen-code.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider/model/mcp mutation inside `settings.json`.
pub const OWNED_SELECTORS: &[&str] = &[
    "model.name",
    "modelProviders",
    "mcpServers",
    "security.auth.selectedType",
    "tools.approvalMode",
    "context.fileName",
    "permissions",
];

/// Concrete adapter for Qwen Code.
#[derive(Debug, Clone)]
pub struct QwenCodeAdapter {
    id: HarnessId,
}

impl QwenCodeAdapter {
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
        Some(home.join(".qwen"))
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
                    let settings = Self::settings_path_for_root(&root);
                    if settings.exists() {
                        evidence.push(format!("settings.json found at {}", settings.display()));
                        match std::fs::read_to_string(&settings) {
                            Ok(text)
                                if (text.contains("mcpServers")
                                    || text.contains("modelProviders")) =>
                            {
                                evidence.push(
                                    "settings.json contains mcpServers/modelProviders".to_owned(),
                                );
                            }
                            Ok(_) => {}
                            Err(err) => evidence.push(format!(
                                "config unreadable at {}: {err}",
                                settings.display()
                            )),
                        }
                    } else {
                        evidence.push(format!("settings.json missing at {}", settings.display()));
                    }
                    let env_file = root.join(".env");
                    if env_file.exists() {
                        evidence.push(format!(".env present at {}", env_file.display()));
                    }
                    if root.join("QWEN.md").exists() {
                        evidence.push(format!("QWEN.md present at {}", root.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
        for var in [
            CONFIG_ENV_VAR,
            SYSTEM_DEFAULTS_ENV_VAR,
            SYSTEM_SETTINGS_ENV_VAR,
            RUNTIME_DIR_ENV_VAR,
        ] {
            if let Ok(dir) = std::env::var(var)
                && !dir.trim().is_empty()
            {
                evidence.push(format!("{var} set to {dir}"));
            } else {
                evidence.push(format!("{var} not set"));
            }
        }
        let project_settings = Path::new(".qwen").join("settings.json");
        if project_settings.exists() {
            evidence.push(format!(
                "project settings found at {}",
                project_settings.display()
            ));
        }
        let project_qwen_md = Path::new("QWEN.md");
        if project_qwen_md.exists() {
            evidence.push(format!("QWEN.md found at {}", project_qwen_md.display()));
        }
    }
}

impl Default for QwenCodeAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for QwenCodeAdapter {
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
        super::resolution_from_detection(detection, "qwen-code", SCHEMA_VERSION_STR)
    }

    #[expect(clippy::too_many_lines, reason = "surfaces are declarative")]
    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let user_resolver = PathResolver::new(
            Some("$QWEN_HOME/settings.json"),
            Some("$QWEN_HOME/settings.json"),
            Some("%QWEN_HOME%\\settings.json"),
            "~/.qwen/settings.json",
        );
        let mut user_settings = ConfigSurface::new(
            "settings.json",
            user_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        user_settings.precedence = 10;
        user_settings.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        user_settings.backup_required = true;
        user_settings.restart_behavior = RestartBehavior::Reload;
        surfaces.push(user_settings);

        let project_resolver = PathResolver::fallback_only(".qwen/settings.json (project)");
        let mut project_settings = ConfigSurface::new(
            "project.settings.json",
            project_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_settings.precedence = 20;
        project_settings.owned_selectors = vec!["mcpServers".to_owned(), "model".to_owned()];
        project_settings.backup_required = true;
        surfaces.push(project_settings);

        let defaults_resolver = PathResolver::new(
            Some("$QWEN_CODE_SYSTEM_DEFAULTS_PATH"),
            Some("$QWEN_CODE_SYSTEM_DEFAULTS_PATH"),
            Some("%QWEN_CODE_SYSTEM_DEFAULTS_PATH%"),
            "/etc/qwen-code/system-defaults.json",
        );
        let mut system_defaults = ConfigSurface::new(
            "system-defaults.json",
            defaults_resolver,
            DocumentKind::Json,
            ConfigScope::SystemManaged,
            SurfaceOwnership::HarnessManaged,
        );
        system_defaults.precedence = 1;
        system_defaults.backup_required = false;
        system_defaults.restart_behavior = RestartBehavior::None;
        surfaces.push(system_defaults);

        let system_resolver = PathResolver::new(
            Some("$QWEN_CODE_SYSTEM_SETTINGS_PATH"),
            Some("$QWEN_CODE_SYSTEM_SETTINGS_PATH"),
            Some("%QWEN_CODE_SYSTEM_SETTINGS_PATH%"),
            "/etc/qwen-code/settings.json",
        );
        let mut system_settings = ConfigSurface::new(
            "system.settings.json",
            system_resolver,
            DocumentKind::Json,
            ConfigScope::SystemManaged,
            SurfaceOwnership::HarnessManaged,
        );
        system_settings.precedence = 2;
        system_settings.backup_required = false;
        surfaces.push(system_settings);

        let env_resolver = PathResolver::new(
            Some("$QWEN_HOME/.env"),
            Some("$QWEN_HOME/.env"),
            Some("%QWEN_HOME%\\.env"),
            "~/.qwen/.env",
        );
        let mut env_surface = ConfigSurface::new(
            ".env",
            env_resolver,
            DocumentKind::Env,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        env_surface.precedence = 15;
        env_surface.backup_required = true;
        surfaces.push(env_surface);

        let mcp_resolver = PathResolver::new(
            Some("$QWEN_HOME/settings.json (mcpServers)"),
            Some("$QWEN_HOME/settings.json (mcpServers)"),
            Some("%QWEN_HOME%\\settings.json (mcpServers)"),
            "~/.qwen/settings.json (mcpServers)",
        );
        let mut mcp = ConfigSurface::new(
            "mcpServers",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp.precedence = 12;
        mcp.owned_selectors = vec!["mcpServers".to_owned()];
        mcp.backup_required = true;
        surfaces.push(mcp);

        let qwen_md_resolver = PathResolver::new(
            Some("$QWEN_HOME/QWEN.md"),
            Some("$QWEN_HOME/QWEN.md"),
            Some("%QWEN_HOME%\\QWEN.md"),
            "~/.qwen/QWEN.md",
        );
        let mut qwen_md = ConfigSurface::new(
            "QWEN.md",
            qwen_md_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        qwen_md.precedence = 8;
        qwen_md.backup_required = false;
        surfaces.push(qwen_md);

        let runtime_resolver = PathResolver::new(
            Some("$QWEN_RUNTIME_DIR"),
            Some("$QWEN_RUNTIME_DIR"),
            Some("%QWEN_RUNTIME_DIR%"),
            "~/.qwen/runtime",
        );
        let mut runtime = ConfigSurface::new(
            "runtime",
            runtime_resolver,
            DocumentKind::Opaque,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        runtime.precedence = 0;
        runtime.backup_required = false;
        surfaces.push(runtime);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        super::all_operations_full()
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "sessions/*".to_owned(),
            "history/*".to_owned(),
            "conversations/*".to_owned(),
            "logs/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "tmp/*".to_owned(),
            "*.lock".to_owned(),
            "telemetry/*".to_owned(),
            "runtime/*".to_owned(),
            "state/*".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("relocated-root via QWEN_HOME with runtime isolation");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        // Separate runtime dir per instance to avoid session pollution.
        let runtime_dir = format!("{}/runtime", instance.config_root);
        plan.env_vars
            .push((RUNTIME_DIR_ENV_VAR.to_owned(), runtime_dir));
        plan.description = format!(
            " Wrapper sets {}={} {}={}/runtime and execs `{}`",
            CONFIG_ENV_VAR,
            instance.config_root,
            RUNTIME_DIR_ENV_VAR,
            instance.config_root,
            EXECUTABLE
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.qwen".to_owned(),
            "~/.qwen-work".to_owned(),
            "~/.qwen-glm".to_owned(),
            "$QWEN_HOME".to_owned(),
            "$QWEN_CODE_SYSTEM_DEFAULTS_PATH".to_owned(),
            "$QWEN_CODE_SYSTEM_SETTINGS_PATH".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("qwen-code requires isolation relocated_root, got {other}"),
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

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some("no plugin mechanism documented (qwen-code.md)")
    }
    fn usage_absence_reason(&self) -> Option<&'static str> {
        Some(
            "usage truth is Qwen's Team/Individual quota endpoints (cookie auth); no local usage artifact",
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::QwenCodeAdapter;
    use super::{CONFIG_ENV_VAR, HARNESS_ID_STR, OWNED_SELECTORS, RUNTIME_DIR_ENV_VAR};
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> QwenCodeAdapter {
        QwenCodeAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-qwen-1").unwrap(),
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
        assert_eq!(a.product_status(), entry.product_status);
    }

    #[test]
    fn supported_platforms_covers_all() {
        let a = adapter();
        let platforms = a.supported_platforms();
        assert!(platforms.len() >= 3);
        let os_set: HashSet<String> = platforms.iter().map(|p| p.os.to_string()).collect();
        assert!(os_set.contains("linux"));
        assert!(os_set.contains("macos"));
        assert!(os_set.contains("windows"));
    }

    #[test]
    fn detection_returns_evidence_and_confidence() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        match result.present {
            InstallPresence::Absent => {
                assert!(result.version.is_none());
                assert!(!result.evidence.is_empty());
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
    fn config_surfaces_include_writable_settings() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let settings = surfaces
            .iter()
            .find(|s| s.id == "settings.json")
            .expect("settings.json surface must exist");
        assert_eq!(settings.kind, DocumentKind::Json);
        assert_eq!(settings.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(settings.scope, ConfigScope::User);
        assert!(settings.backup_required);
        for selector in ["model.name", "modelProviders", "mcpServers"] {
            assert!(
                settings.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in &settings.owned_selectors {
            assert!(!sel.is_empty());
        }
        for sel in OWNED_SELECTORS {
            assert!(settings.owned_selectors.contains(&(*sel).to_owned()));
        }

        let system = surfaces
            .iter()
            .find(|s| s.id == "system-defaults.json")
            .expect("system-defaults.json");
        assert_eq!(system.scope, ConfigScope::SystemManaged);

        let env = surfaces.iter().find(|s| s.id == ".env").expect(".env");
        assert_eq!(env.kind, DocumentKind::Env);
    }

    #[test]
    fn supported_operations_cover_full() {
        let a = adapter();
        let ops = a.supported_operations();
        assert!(!ops.is_empty());
        for (_, support) in &ops {
            assert_eq!(*support, AdapterSupport::Full);
        }
        let names: HashSet<String> = ops.iter().map(|(n, _)| n.clone()).collect();
        for required in [
            "detect",
            "read_config",
            "write_config",
            "manage_mcp",
            "plan_wrapper",
        ] {
            assert!(names.contains(required), "missing op {required}");
        }
    }

    #[test]
    fn plan_mirror_exclusions_cover_history_and_locks() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        let must_contain = ["sessions/*", "history/*", "logs/*", "cache/*", "*.lock"];
        for pat in must_contain {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"settings.json".to_owned()));
    }

    #[test]
    fn plan_mirror_includes_settings_and_excludes_sessions() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        let is_excluded = |file: &str| crate::adapters::exclusion_matches(&exclusions, file);
        assert!(!is_excluded("settings.json"));
        assert!(!is_excluded("QWEN.md"));
        assert!(is_excluded("sessions/abc.jsonl"));
        assert!(is_excluded("history/history.jsonl"));
        assert!(is_excluded("logs/qwen.log"));
        assert!(is_excluded("runtime/todo.json"));
    }

    #[test]
    fn plan_wrapper_sets_qwen_home_and_runtime() {
        let tmp_root = crate::test_util::tmp_abs_str(".qwen-work");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == tmp_root.as_str())
        );
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == RUNTIME_DIR_ENV_VAR && v.contains(tmp_root.as_str()))
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(CONFIG_ENV_VAR));
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let tmp_root = crate::test_util::tmp_abs_str("my qwen work");
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
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".qwen-work"));
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
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.contains(".qwen")));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".qwen-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".qwen-work"));
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
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".qwen-work"));
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let settings = surfaces.iter().find(|s| s.id == "settings.json").unwrap();
        assert_eq!(settings.path_resolver.fallback, "~/.qwen/settings.json");
        let resolver = &settings.path_resolver;
        assert!(resolver.linux.as_deref().unwrap().contains(CONFIG_ENV_VAR));
        assert!(resolver.macos.as_deref().unwrap().contains(CONFIG_ENV_VAR));
        assert!(
            resolver
                .windows
                .as_deref()
                .unwrap()
                .contains(CONFIG_ENV_VAR)
        );
    }

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/qwen_code")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.json");
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.is_empty());
        let value = superai_config::json::load_value(&path).unwrap();
        assert_eq!(value, serde_json::Value::Object(serde_json::Map::default()));
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("settings.minimal.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.is_empty() || map.contains_key("model") || map.len() <= 2);
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(
            map.contains_key("model")
                || map.contains_key("modelProviders")
                || map.contains_key("mcpServers")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("qwen");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("settings.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("qwen3-coder-plus".to_owned()),
            );
            assert!(map.contains_key("foreignKey") || map.contains_key("unknownTopLevel"));
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert_eq!(
            after["model"],
            serde_json::Value::String("qwen3-coder-plus".to_owned())
        );
        let foreign_preserved = after.contains_key("foreignKey")
            || after.contains_key("unknownTopLevel")
            || after.contains_key("customField");
        assert!(
            foreign_preserved,
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
    fn wrapper_env_var_isolation_is_relocated_root() {
        let tmp_root = crate::test_util::tmp_abs_str("user/.qwen-isolated");
        let a = adapter();
        assert!(a.scan_candidates().len() >= 3);
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(!plan.env_vars.is_empty());
        let (key, val) = &plan.env_vars[0];
        assert_eq!(key, CONFIG_ENV_VAR);
        assert_eq!(val, tmp_root.as_str());
    }

    #[test]
    fn registry_no_harness_value_leak() {
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".qwen-work"));
        let json = serde_json::to_string(&inst).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let forbidden = [
            "model", "endpoint", "api_key", "skill", "plugin", "mcp", "baseUrl", "base_url",
        ];
        let text = json.to_lowercase();
        for field in forbidden {
            if let serde_json::Value::Object(map) = &v {
                assert!(
                    !map.contains_key(field),
                    "forbidden field `{field}` must not be emitted, json: {json}"
                );
            }
            assert!(
                !text.contains(&format!("\"{field}\"")),
                "forbidden field `{field}` appears in json: {json}"
            );
        }
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
