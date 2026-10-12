//! `MiniMax` Code adapter: `mcode` binary, data dir `~/.minimax` or
//! `$MINIMAX_DATA_DIR` (fallback `$MAVIS_DATA_DIR`), `config.yaml` BYOK
//! providers, project-scope `.mcp.json`.

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
pub const HARNESS_ID_STR: &str = "minimax-code";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "MiniMax Code";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "mcode";

/// Environment variable that relocates the data dir.
pub const CONFIG_ENV_VAR: &str = "MINIMAX_DATA_DIR";

/// Legacy alias of the data-dir override; non-empty wins when primary is unset.
pub const FALLBACK_ENV_VAR: &str = "MAVIS_DATA_DIR";

/// Environment variable carrying a custom-provider API key (reference only).
pub const PROVIDER_KEY_ENV_VAR: &str = "MCODE_PROVIDER_API_KEY";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/minimax-code.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider mutation inside `config.yaml`.
pub const OWNED_SELECTORS: &[&str] = &["minimax_api", "custom_provider"];

/// Concrete adapter for `MiniMax` Code.
#[derive(Debug, Clone)]
pub struct MiniMaxCodeAdapter {
    id: HarnessId,
}

impl MiniMaxCodeAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    fn default_data_dir() -> Option<PathBuf> {
        for var in [CONFIG_ENV_VAR, FALLBACK_ENV_VAR] {
            if let Ok(dir) = std::env::var(var)
                && !dir.trim().is_empty()
            {
                return Some(PathBuf::from(dir));
            }
        }
        let home = super::home_dir()?;
        Some(home.join(".minimax"))
    }

    fn config_path_for_root(root: &Path) -> PathBuf {
        root.join("config.yaml")
    }

    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_data_dir() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("data dir exists at {}", root.display()));
                    let cfg = Self::config_path_for_root(&root);
                    if cfg.exists() {
                        evidence.push(format!("config.yaml found at {}", cfg.display()));
                        match std::fs::read_to_string(&cfg) {
                            Ok(text)
                                if text.contains("minimax_api")
                                    || text.contains("custom_provider") =>
                            {
                                evidence.push("config.yaml carries provider config".to_owned());
                            }
                            Ok(_) => {}
                            Err(err) => evidence
                                .push(format!("config unreadable at {}: {err}", cfg.display())),
                        }
                    } else {
                        evidence.push(format!("config.yaml missing at {}", cfg.display()));
                    }
                } else {
                    evidence.push(format!("data dir missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default data dir (no HOME)".to_owned());
            }
        }
        for var in [CONFIG_ENV_VAR, FALLBACK_ENV_VAR] {
            if let Ok(dir) = std::env::var(var)
                && !dir.trim().is_empty()
            {
                evidence.push(format!("{var} set to {dir}"));
            }
        }
        if Path::new(".mcp.json").exists() {
            evidence.push("project .mcp.json present in cwd".to_owned());
        }
        if Path::new("AGENTS.md").exists() {
            evidence.push("AGENTS.md present in cwd".to_owned());
        }
    }
}

impl Default for MiniMaxCodeAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for MiniMaxCodeAdapter {
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
        super::resolution_from_detection(detection, "minimax-code", SCHEMA_VERSION_STR)
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let config_resolver = PathResolver::new(
            Some("$MINIMAX_DATA_DIR/config.yaml"),
            Some("$MINIMAX_DATA_DIR/config.yaml"),
            Some("%MINIMAX_DATA_DIR%\\config.yaml"),
            "~/.minimax/config.yaml",
        );
        let mut config = ConfigSurface::new(
            "config.yaml",
            config_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        config.precedence = 10;
        config.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        config.backup_required = true;
        config.restart_behavior = RestartBehavior::Reload;
        surfaces.push(config);

        let mcp_resolver = PathResolver::fallback_only(".mcp.json (workspace root)");
        let mut mcp = ConfigSurface::new(
            "project .mcp.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        mcp.precedence = 14;
        mcp.owned_selectors = vec!["mcpServers".to_owned()];
        mcp.backup_required = false;
        surfaces.push(mcp);

        for (id, hint, precedence) in [
            (
                "skills (.minimax)",
                ".minimax/skills/<name>/SKILL.md (workspace)",
                12,
            ),
            (
                "skills (.agents)",
                ".agents/skills/<name>/SKILL.md (project)",
                12,
            ),
            (
                "skills (.claude)",
                ".claude/skills/<name>/SKILL.md (project)",
                12,
            ),
        ] {
            let mut surface = ConfigSurface::new(
                id,
                PathResolver::fallback_only(hint),
                DocumentKind::TextFragment,
                ConfigScope::ProjectWorkspace,
                SurfaceOwnership::UserEditable,
            );
            surface.precedence = precedence;
            surface.backup_required = false;
            surfaces.push(surface);
        }

        let agents_resolver = PathResolver::fallback_only("AGENTS.md (project, mcode init)");
        let mut agents = ConfigSurface::new(
            "AGENTS.md",
            agents_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        agents.precedence = 11;
        agents.backup_required = false;
        surfaces.push(agents);

        let data_resolver =
            PathResolver::fallback_only("$MINIMAX_DATA_DIR (login state, caches, sessions)");
        let mut data = ConfigSurface::new(
            "data dir",
            data_resolver,
            DocumentKind::Opaque,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        data.precedence = 0;
        data.backup_required = false;
        surfaces.push(data);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        super::all_operations_full()
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "sessions/*".to_owned(),
            "cache/*".to_owned(),
            "caches/*".to_owned(),
            "logs/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            "*.sqlite".to_owned(),
            "*.lock".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("relocated data dir via MINIMAX_DATA_DIR");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        plan.description = format!(
            " Wrapper sets {}={} and execs `{}` (BYOK needs no login; project .mcp.json stays with the checkout)",
            CONFIG_ENV_VAR, instance.config_root, EXECUTABLE
        );
        plan.shared_state_warnings = vec![format!(
            "MiniMax account login state lives in the data dir; BYOK providers read keys from {} per provider",
            PROVIDER_KEY_ENV_VAR
        )];
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.minimax".to_owned(),
            "$MINIMAX_DATA_DIR".to_owned(),
            "$MAVIS_DATA_DIR".to_owned(),
            ".minimax/skills".to_owned(),
            ".mcp.json".to_owned(),
            "AGENTS.md".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("minimax-code requires isolation relocated_root, got {other}"),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        super::skill_modes_link_first()
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            ".mcp.json",
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            RestartBehavior::Reload,
        ))
    }

    fn plugin_decl(&self) -> Option<crate::adapter::PluginAdapterDecl> {
        Some(crate::adapter::PluginAdapterDecl::requires_execution(
            "mcode plugin marketplace (mcode plugin add <name>@official)",
            "mcode-managed plugin state (persistence path undocumented)",
            crate::adapter::PluginKind::MarketplaceRecord,
            RestartBehavior::Reload,
        ))
    }
    fn usage_absence_reason(&self) -> Option<&'static str> {
        Some("usage truth is MiniMax's API token endpoints; no local usage artifact")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{CONFIG_ENV_VAR, HARNESS_ID_STR, MiniMaxCodeAdapter, OWNED_SELECTORS};
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> MiniMaxCodeAdapter {
        MiniMaxCodeAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-minimax-1").unwrap(),
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
    fn config_surfaces_include_yaml_and_project_mcp() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 7);
        let config = surfaces
            .iter()
            .find(|s| s.id == "config.yaml")
            .expect("config.yaml surface must exist");
        assert_eq!(config.kind, DocumentKind::Yaml);
        assert_eq!(config.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(config.scope, ConfigScope::User);
        assert!(config.backup_required);
        for sel in OWNED_SELECTORS {
            assert!(config.owned_selectors.contains(&(*sel).to_owned()));
        }

        let mcp = surfaces
            .iter()
            .find(|s| s.id == "project .mcp.json")
            .expect("project .mcp.json");
        assert_eq!(mcp.kind, DocumentKind::Json);
        assert_eq!(mcp.scope, ConfigScope::ProjectWorkspace);
        assert!(mcp.owned_selectors.contains(&"mcpServers".to_owned()));

        let skills = surfaces
            .iter()
            .find(|s| s.id == "skills (.minimax)")
            .expect("skills (.minimax)");
        assert_eq!(
            skills.scope,
            ConfigScope::ProjectWorkspace,
            "minimax-code.md section 5 lists .minimax/skills as a workspace root"
        );
        assert!(
            !skills.path_resolver.fallback.starts_with('~'),
            "no user-home skills root: {}",
            skills.path_resolver.fallback
        );
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
    fn plan_mirror_exclusions_cover_sessions_and_sqlite() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        let is_excluded = |file: &str| crate::adapters::exclusion_matches(&exclusions, file);
        assert!(!is_excluded("config.yaml"));
        assert!(is_excluded("sessions/abc.json"));
        assert!(is_excluded("caches/blob.bin"));
        assert!(is_excluded("state.sqlite"));
    }

    #[test]
    fn plan_wrapper_sets_data_dir() {
        let tmp_root = crate::test_util::tmp_abs_str(".minimax-work");
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
        let tmp_root = crate::test_util::tmp_abs_str("my minimax work");
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
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".minimax-work"));
        inst.harness = HarnessId::new("codex-cli").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_data_dir_and_env() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains(".minimax")));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
        assert!(candidates.iter().any(|c| c.contains("MAVIS_DATA_DIR")));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".minimax-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".minimax-work"));
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
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".minimax-work"));
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/minimax_code")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.yaml");
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("config.minimal.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn fixture_populated_carries_provider_sections() {
        let path = fixture_path("config.populated.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(map.contains_key("minimax_api"));
        assert!(map.contains_key("custom_provider"));
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("config.foreign.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::yaml::load(&path).unwrap();
        assert!(
            original.contains_key("vendor_key") || original.contains_key("foreignKey"),
            "foreign fixture must carry a foreign key"
        );
        let dir = crate::test_util::temp_dir_unique("minimax");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("config.foreign.copy.yaml");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::yaml::edit(&tmp, |doc| {
            doc.insert("vendor_key".to_owned(), serde_json::json!({"nested": true}));
            assert!(doc.contains_key("vendor_key"));
        })
        .unwrap();
        let after = superai_config::yaml::load(&tmp).unwrap();
        assert!(
            after.contains_key("vendor_key") || after.contains_key("foreignKey"),
            "foreign keys must be preserved, got {after:?}"
        );
        drop(std::fs::remove_file(&tmp));
    }

    #[test]
    fn fixture_changing_yaml_edit_refuses_lossy() {
        let path = fixture_path("config.foreign.yaml");
        let dir = crate::test_util::temp_dir_unique("minimax-lossy");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("config.lossy.yaml");
        std::fs::copy(&path, &tmp).unwrap();
        let result = superai_config::yaml::edit(&tmp, |doc| {
            doc.insert(
                "custom_provider".to_owned(),
                serde_json::json!({"relay": {"base_url": "https://example.com/v1"}}),
            );
        });
        assert!(
            result.is_err(),
            "yaml round-trips are lossy; a changing edit on an existing file must refuse (DOC-06)"
        );
        drop(std::fs::remove_file(&tmp));
    }

    #[test]
    fn fixture_malformed_fails_to_parse() {
        let path = fixture_path("config.malformed.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::yaml::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn mcp_decl_targets_project_mcp_json() {
        let a = adapter();
        let decl = a.mcp_decl().unwrap();
        assert_eq!(decl.dest_file, ".mcp.json");
        assert_eq!(decl.dest_key, "mcpServers");
        assert_eq!(decl.scope, ConfigScope::ProjectWorkspace);
        assert!(decl.read_only.is_none());
        assert!(a.mcp_absence_reason().is_none());
    }

    #[test]
    fn plugin_decl_requires_execution_against_marketplace() {
        let a = adapter();
        let decl = a.plugin_decl().unwrap();
        assert!(decl.requires_execution);
        assert!(decl.source_hint.contains("mcode plugin"));
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
