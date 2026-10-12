//! Kimi Code adapter: relocated root via `KIMI_CODE_HOME`, writable
//! `config.toml` plus MCP JSON.

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
pub const HARNESS_ID_STR: &str = "kimi-code";

/// Ledger alias for compatibility with `harness_catalog::ENTRIES` (`kimi-code-cli`).
pub const HARNESS_ID_LEDGER_ALIAS: &str = "kimi-code-cli";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Kimi Code CLI";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "kimi";

/// Environment variable that relocates the config root.
pub const CONFIG_ENV_VAR: &str = "KIMI_CODE_HOME";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/kimi-cli.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider/model mutation inside `config.toml`.
pub const OWNED_SELECTORS: &[&str] = &[
    "default_model",
    "providers",
    "models",
    "thinking",
    "loop_control",
    "mcp",
    "default_permission_mode",
    "model",
];

/// Concrete adapter for Kimi Code.
#[derive(Debug, Clone)]
pub struct KimiCodeAdapter {
    id: HarnessId,
}

impl KimiCodeAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    /// Create an adapter instance with an explicit ledger id (for catalog alias).
    pub fn from_ledger_id(id: HarnessId) -> Self {
        Self { id }
    }

    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(CONFIG_ENV_VAR)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir));
        }
        let home = super::home_dir()?;
        Some(home.join(".kimi-code"))
    }

    fn config_path_for_root(root: &Path) -> PathBuf {
        root.join("config.toml")
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
                    let cfg = Self::config_path_for_root(&root);
                    if cfg.exists() {
                        evidence.push(format!("config.toml found at {}", cfg.display()));
                        match std::fs::read_to_string(&cfg) {
                            Ok(text)
                                if (text.contains("default_model")
                                    || text.contains("[providers")) =>
                            {
                                evidence.push(
                                    "config.toml contains default_model/providers".to_owned(),
                                );
                            }
                            Ok(_) => {}
                            Err(err) => evidence
                                .push(format!("config unreadable at {}: {err}", cfg.display())),
                        }
                    } else {
                        evidence.push(format!("config.toml missing at {}", cfg.display()));
                    }
                    let mcp = root.join("mcp.json");
                    if mcp.exists() {
                        evidence.push(format!("mcp.json present at {}", mcp.display()));
                    }
                    let tui = root.join("tui.toml");
                    if tui.exists() {
                        evidence.push(format!("tui.toml present at {}", tui.display()));
                    }
                    let creds = root.join("credentials.json");
                    if creds.exists() {
                        evidence.push(format!("credentials present at {}", creds.display()));
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
            evidence.push(format!("{CONFIG_ENV_VAR} not set, using ~/.kimi-code"));
        }
        evidence.push(format!(
            "ledger alias {HARNESS_ID_LEDGER_ALIAS} maps to {HARNESS_ID_STR}"
        ));
    }
}

impl Default for KimiCodeAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for KimiCodeAdapter {
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
        super::resolution_from_detection(detection, "kimi-code", SCHEMA_VERSION_STR)
    }

    #[expect(clippy::too_many_lines, reason = "surfaces are declarative")]
    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let config_resolver = PathResolver::new(
            Some("$KIMI_CODE_HOME/config.toml"),
            Some("$KIMI_CODE_HOME/config.toml"),
            Some("%KIMI_CODE_HOME%\\config.toml"),
            "~/.kimi-code/config.toml",
        );
        let mut config_surface = ConfigSurface::new(
            "config.toml",
            config_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        config_surface.precedence = 10;
        config_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        config_surface.backup_required = true;
        config_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(config_surface);

        let mcp_resolver = PathResolver::new(
            Some("$KIMI_CODE_HOME/mcp.json"),
            Some("$KIMI_CODE_HOME/mcp.json"),
            Some("%KIMI_CODE_HOME%\\mcp.json"),
            "~/.kimi-code/mcp.json",
        );
        let mut mcp_surface = ConfigSurface::new(
            "mcp.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp_surface.precedence = 20;
        mcp_surface.owned_selectors = vec!["mcpServers".to_owned()];
        mcp_surface.backup_required = true;
        mcp_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(mcp_surface);

        let tui_resolver = PathResolver::new(
            Some("$KIMI_CODE_HOME/tui.toml"),
            Some("$KIMI_CODE_HOME/tui.toml"),
            Some("%KIMI_CODE_HOME%\\tui.toml"),
            "~/.kimi-code/tui.toml",
        );
        let mut tui_surface = ConfigSurface::new(
            "tui.toml",
            tui_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        tui_surface.precedence = 5;
        tui_surface.backup_required = false;
        surfaces.push(tui_surface);

        let local_resolver = PathResolver::fallback_only(".kimi-code/local.toml (project)");
        let mut local_surface = ConfigSurface::new(
            "local.toml",
            local_resolver,
            DocumentKind::Toml,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        local_surface.precedence = 15;
        local_surface.backup_required = false;
        surfaces.push(local_surface);

        let project_mcp_resolver = PathResolver::fallback_only(".kimi-code/mcp.json (project)");
        let mut project_mcp = ConfigSurface::new(
            "project.mcp.json",
            project_mcp_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_mcp.precedence = 16;
        project_mcp.backup_required = false;
        surfaces.push(project_mcp);

        let skills_resolver = PathResolver::new(
            Some("$KIMI_CODE_HOME/skills/<name>/SKILL.md"),
            Some("$KIMI_CODE_HOME/skills/<name>/SKILL.md"),
            Some("%KIMI_CODE_HOME%\\skills\\<name>\\SKILL.md"),
            "~/.kimi-code/skills/<name>/SKILL.md",
        );
        let mut skills = ConfigSurface::new(
            "skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills.precedence = 8;
        skills.backup_required = false;
        surfaces.push(skills);

        let hooks_resolver = PathResolver::new(
            Some("$KIMI_CODE_HOME/hooks/<name>.sh"),
            Some("$KIMI_CODE_HOME/hooks/<name>.sh"),
            Some("%KIMI_CODE_HOME%\\hooks\\<name>.sh"),
            "~/.kimi-code/hooks/<name>.sh",
        );
        let mut hooks = ConfigSurface::new(
            "hooks",
            hooks_resolver,
            DocumentKind::Executable,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        hooks.precedence = 7;
        hooks.backup_required = false;
        surfaces.push(hooks);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        super::all_operations_full()
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "sessions/*".to_owned(),
            "history/*".to_owned(),
            "logs/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "tmp/*".to_owned(),
            "*.lock".to_owned(),
            "state/*".to_owned(),
            "data/*".to_owned(),
            "telemetry/*".to_owned(),
            "background/*".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("relocated-root via KIMI_CODE_HOME");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        plan.description = format!(
            " Wrapper sets {}={} and execs `{}`",
            CONFIG_ENV_VAR, instance.config_root, EXECUTABLE
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.kimi-code".to_owned(),
            "~/.kimi-code-work".to_owned(),
            "~/.kimi".to_owned(),
            "$KIMI_CODE_HOME".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("kimi-code requires isolation relocated_root, got {other}"),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        super::skill_modes_link_first()
    }

    /// MCP in `$KIMI_CODE_HOME/mcp.json`; project `.kimi-code/mcp.json` overrides servers by name.
    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            "mcp.json",
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::User,
            RestartBehavior::Reload,
        ))
    }

    /// Marketplace plugins bundle skills, MCP servers, and data sources in one record.
    fn plugin_decl(&self) -> Option<crate::adapter::PluginAdapterDecl> {
        Some(crate::adapter::PluginAdapterDecl::requires_execution(
            "kimi plugins marketplace (/plugins)",
            "config.toml",
            crate::adapter::PluginKind::MarketplaceRecord,
            RestartBehavior::Restart,
        ))
    }
    fn usage_absence_reason(&self) -> Option<&'static str> {
        Some(
            "usage truth is Moonshot's weekly quota endpoint (cookie auth); no local usage artifact",
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{
        CONFIG_ENV_VAR, HARNESS_ID_LEDGER_ALIAS, HARNESS_ID_STR, KimiCodeAdapter, OWNED_SELECTORS,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> KimiCodeAdapter {
        KimiCodeAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-kimi-1").unwrap(),
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
        // Catalog registration via the ledger alias: the entry predates the
        // id shortening, and an unknown id can never reconcile instances.
        let entry = crate::harness_catalog::find_by_id(HARNESS_ID_LEDGER_ALIAS).unwrap();
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(entry.id, HARNESS_ID_LEDGER_ALIAS);
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
    fn config_surfaces_include_writable_toml() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let config = surfaces
            .iter()
            .find(|s| s.id == "config.toml")
            .expect("config.toml surface must exist");
        assert_eq!(config.kind, DocumentKind::Toml);
        assert_eq!(config.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(config.scope, ConfigScope::User);
        assert!(config.backup_required);
        for selector in ["default_model", "providers", "models"] {
            assert!(
                config.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in &config.owned_selectors {
            assert!(!sel.is_empty());
        }
        for sel in OWNED_SELECTORS {
            assert!(config.owned_selectors.contains(&(*sel).to_owned()));
        }

        let mcp = surfaces
            .iter()
            .find(|s| s.id == "mcp.json")
            .expect("mcp.json");
        assert_eq!(mcp.kind, DocumentKind::Json);
        assert_eq!(mcp.ownership, SurfaceOwnership::UserEditable);

        let hooks = surfaces.iter().find(|s| s.id == "hooks").expect("hooks");
        assert_eq!(hooks.kind, DocumentKind::Executable);
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
        assert!(!exclusions.contains(&"config.toml".to_owned()));
    }

    #[test]
    fn plan_mirror_includes_config_and_excludes_sessions() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        let is_excluded = |file: &str| crate::adapters::exclusion_matches(&exclusions, file);
        assert!(!is_excluded("config.toml"));
        assert!(!is_excluded("mcp.json"));
        assert!(is_excluded("sessions/abc.jsonl"));
        assert!(is_excluded("history/history.jsonl"));
        assert!(is_excluded("logs/kimi.log"));
        assert!(is_excluded("state/state.db"));
    }

    #[test]
    fn plan_wrapper_sets_kimi_code_home() {
        let tmp_root = crate::test_util::tmp_abs_str(".kimi-code-work");
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
        let tmp_root = crate::test_util::tmp_abs_str("my kimi work");
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
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kimi-code-work"));
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
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("kimi-code") || c.contains("kimi"))
        );
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kimi-code-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kimi-code-work"));
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
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kimi-code-work"));
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let config = surfaces.iter().find(|s| s.id == "config.toml").unwrap();
        assert_eq!(config.path_resolver.fallback, "~/.kimi-code/config.toml");
        let resolver = &config.path_resolver;
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
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/kimi_code")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.toml");
        let doc = superai_config::toml_file::load(&path).unwrap();
        assert!(doc.is_empty());
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("config.minimal.toml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::toml_file::load(&path).unwrap();
        assert!(map.is_empty() || map.contains_key("default_model") || map.len() <= 2);
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("config.populated.toml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::toml_file::load(&path).unwrap();
        assert!(
            map.contains_key("default_model")
                || map.contains_key("providers")
                || map.contains_key("models")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("config.foreign.toml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::toml_file::load(&path).unwrap();
        assert!(
            original.contains_key("foreignKey")
                || original.contains_key("unknownTopLevel")
                || original.contains_key("customField")
        );
        let dir = crate::test_util::temp_dir_unique("kimi");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("config.foreign.copy.toml");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::toml_file::edit(&tmp, |doc| {
            doc["default_model"] = toml_edit::value("kimi-for-coding");
            assert!(
                doc.to_string().contains("foreignKey")
                    || doc.to_string().contains("unknownTopLevel")
                    || doc.to_string().contains("customField")
            );
        })
        .unwrap();
        let after = superai_config::toml_file::load(&tmp).unwrap();
        assert!(after.contains_key("default_model"));
        let text = after.to_string();
        let foreign_preserved = text.contains("foreignKey")
            || text.contains("unknownTopLevel")
            || text.contains("customField");
        assert!(
            foreign_preserved,
            "foreign keys must be preserved, got {after:?}"
        );
        drop(std::fs::remove_file(&tmp));
    }

    #[test]
    fn fixture_malformed_fails_to_parse() {
        let path = fixture_path("config.malformed.toml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::toml_file::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn wrapper_env_var_isolation_is_relocated_root() {
        let tmp_root = crate::test_util::tmp_abs_str("user/.kimi-code-isolated");
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
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kimi-code-work"));
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

    /// `fixtures/kimi_code_cli` must stay a byte-copy of `fixtures/kimi_code`
    /// so the alias catalog id cannot silently drift.
    #[test]
    fn alias_corpus_dir_stays_in_sync_with_kimi_code() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let primary = root.join("kimi_code");
        let alias = root.join("kimi_code_cli");
        let primary_names: Vec<String> = std::fs::read_dir(&primary)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let alias_names: Vec<String> = std::fs::read_dir(&alias)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(primary_names, alias_names, "alias must mirror the file set");
        for name in primary_names {
            let from = primary.join(&name);
            let to = alias.join(&name);
            if !from.is_file() {
                assert!(to.is_dir(), "alias entry {name} must mirror the kind");
                continue;
            }
            let a = std::fs::read(&from).unwrap();
            let b = std::fs::read(&to).unwrap();
            assert_eq!(a, b, "alias file {name} drifted from kimi_code");
        }
        let report = crate::verification::fixture_report(&alias);
        assert!(report.validity_pass, "alias corpus validity");
        assert!(report.secret_free_pass, "alias corpus secret-free");
    }
}
