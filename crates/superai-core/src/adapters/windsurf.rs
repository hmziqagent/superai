//! Windsurf adapter: IDE `--user-data-dir` isolation, MCP JSON under
//! `~/.codeium/windsurf/`, project rules dirs; `Constrained`.

use std::path::{Path, PathBuf};

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
pub const HARNESS_ID_STR: &str = "windsurf";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Windsurf/Devin Desktop";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "windsurf";

/// Alternative executable (Devin converged).
pub const EXECUTABLE_ALT: &str = "devin";

/// Flags for IDE isolation.
pub const USER_DATA_DIR_FLAG: &str = "--user-data-dir";

/// Extensions dir flag.
pub const EXTENSIONS_DIR_FLAG: &str = "--extensions-dir";

/// Default MCP config fallback.
pub const DEFAULT_MCP_FALLBACK: &str = "~/.codeium/windsurf/mcp_config.json";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/windsurf.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Selectors superai owns on the MCP surface; other keys round-trip.
pub const MCP_OWNED_SELECTORS: &[&str] = &["mcpServers"];

/// Concrete adapter for Windsurf.
#[derive(Debug, Clone)]
pub struct WindsurfAdapter {
    id: HarnessId,
}

impl WindsurfAdapter {
    /// Create a new adapter.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    fn default_mcp_path() -> Option<PathBuf> {
        let home = super::home_dir()?;
        Some(
            home.join(".codeium")
                .join("windsurf")
                .join("mcp_config.json"),
        )
    }

    fn default_user_data_root() -> Option<PathBuf> {
        let home = super::home_dir()?;
        if cfg!(target_os = "macos") {
            Some(
                home.join("Library")
                    .join("Application Support")
                    .join("Windsurf")
                    .join("User"),
            )
        } else if cfg!(windows) {
            if let Ok(appdata) = std::env::var("APPDATA")
                && !appdata.trim().is_empty()
            {
                return Some(PathBuf::from(appdata).join("Windsurf").join("User"));
            }
            Some(
                home.join("AppData")
                    .join("Roaming")
                    .join("Windsurf")
                    .join("User"),
            )
        } else {
            Some(home.join(".config").join("Windsurf").join("User"))
        }
    }

    #[expect(clippy::excessive_nesting, reason = "evidence explicit")]
    #[expect(clippy::unused_self, reason = "adapter method")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_mcp_path() {
            Some(p) => {
                if p.exists() {
                    evidence.push(format!("mcp_config.json exists at {}", p.display()));
                    match std::fs::read_to_string(&p) {
                        Ok(text) if text.contains("mcpServers") => {
                            evidence.push("mcp_config.json contains mcpServers".to_owned());
                        }
                        Ok(_) => {}
                        Err(err) => {
                            evidence.push(format!("config unreadable at {}: {err}", p.display()));
                        }
                    }
                } else {
                    evidence.push(format!("mcp_config.json missing at {}", p.display()));
                }
            }
            None => evidence.push("could not resolve mcp path (no HOME)".to_owned()),
        }
        for rule_path in [
            Path::new(".windsurf/rules"),
            Path::new(".devin/rules"),
            Path::new(".windsurfrules"),
        ] {
            if rule_path.exists() {
                evidence.push(format!("rules present at {}", rule_path.display()));
            }
        }
        if Path::new("AGENTS.md").exists() {
            evidence.push("AGENTS.md present".to_owned());
        }
        if let Some(root) = Self::default_user_data_root()
            && root.exists()
        {
            evidence.push(format!("Windsurf User data exists at {}", root.display()));
        }
        if Path::new(".codeium/windsurf/memories").exists() || Path::new(".devin/memories").exists()
        {
            evidence.push("memories dir present".to_owned());
        }
    }
}

impl Default for WindsurfAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for WindsurfAdapter {
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

    #[expect(clippy::single_match_else, reason = "detection branching explicit")]
    fn detection(&self) -> DetectionResult {
        let mut evidence = Vec::new();
        let mut version: Option<String> = None;
        let mut binary_path: Option<PathBuf> = None;
        match super::find_in_path(&[EXECUTABLE, EXECUTABLE_ALT]) {
            Some(path) => {
                evidence.push(format!(
                    "found binary `{}` at {}",
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("windsurf"),
                    path.display()
                ));
                match super::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `--version`"));
                        version = Some(v);
                    }
                    None => {
                        evidence.push("version probe failed for `--version` (timeout)".to_owned());
                    }
                }
                binary_path = Some(path);
            }
            None => {
                evidence.push(format!("binary `{EXECUTABLE}` not found in PATH"));
                evidence.push(format!("binary `{EXECUTABLE_ALT}` not found in PATH"));
            }
        }
        self.collect_config_evidence(&mut evidence);
        let present = super::install_presence(binary_path.is_some(), version.is_some());
        let confidence =
            super::detection_confidence(binary_path.is_some(), version.is_some(), false);
        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution_from(&self, detection: &DetectionResult) -> VersionResolution {
        super::resolution_from_detection(detection, "windsurf", SCHEMA_VERSION_STR)
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let mcp_resolver = PathResolver::new(
            Some("~/.codeium/windsurf/mcp_config.json"),
            Some("~/.codeium/windsurf/mcp_config.json"),
            Some("%USERPROFILE%\\.codeium\\windsurf\\mcp_config.json"),
            "~/.codeium/windsurf/mcp_config.json",
        );
        let mut mcp = ConfigSurface::new(
            "mcp_config.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp.precedence = 10;
        mcp.owned_selectors = MCP_OWNED_SELECTORS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        mcp.backup_required = true;
        mcp.restart_behavior = RestartBehavior::Reload;
        surfaces.push(mcp);

        let memories_resolver = PathResolver::fallback_only("~/.codeium/windsurf/memories/");
        let mut memories = ConfigSurface::new(
            "memories",
            memories_resolver,
            DocumentKind::Opaque,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        memories.precedence = 5;
        memories.backup_required = false;
        surfaces.push(memories);

        let windsurf_rules_resolver =
            PathResolver::fallback_only(".windsurf/rules/*.md / .devin/rules/*.md");
        let mut rules = ConfigSurface::new(
            ".windsurf/rules",
            windsurf_rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        rules.precedence = 12;
        rules.backup_required = false;
        surfaces.push(rules);

        let legacy_rules_resolver =
            PathResolver::fallback_only(".windsurfrules (legacy) / .windsumrfrules");
        let mut legacy = ConfigSurface::new(
            ".windsurfrules",
            legacy_rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        legacy.precedence = 8;
        legacy.backup_required = false;
        surfaces.push(legacy);

        let agents_resolver =
            PathResolver::fallback_only("AGENTS.md (any directory, hierarchical)");
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

        let ignore_resolver = PathResolver::fallback_only(".codeiumignore / .devinignore");
        let mut ignore = ConfigSurface::new(
            ".codeiumignore",
            ignore_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        ignore.precedence = 5;
        ignore.backup_required = false;
        surfaces.push(ignore);

        let vscode_resolver = PathResolver::new(
            Some("~/.config/Windsurf/User/settings.json"),
            Some("~/Library/Application Support/Windsurf/User/settings.json"),
            Some("%APPDATA%\\Windsurf\\User\\settings.json"),
            "~/.config/Windsurf/User/settings.json",
        );
        let mut settings = ConfigSurface::new(
            "windsurf settings.json",
            vscode_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        settings.precedence = 7;
        settings.backup_required = true;
        surfaces.push(settings);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        vec![
            ("detect".to_owned(), AdapterSupport::Constrained),
            ("read_config".to_owned(), AdapterSupport::Constrained),
            ("write_config".to_owned(), AdapterSupport::Constrained),
            ("manage_skills".to_owned(), AdapterSupport::Constrained),
            ("manage_mcp".to_owned(), AdapterSupport::Constrained),
            ("manage_plugins".to_owned(), AdapterSupport::Constrained),
            ("configure_provider".to_owned(), AdapterSupport::Constrained),
            ("plan_mirror".to_owned(), AdapterSupport::Constrained),
            ("plan_wrapper".to_owned(), AdapterSupport::Constrained),
            ("scan_candidates".to_owned(), AdapterSupport::Constrained),
            ("validate_instance".to_owned(), AdapterSupport::Constrained),
        ]
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "memories/*".to_owned(),
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "tmp/*".to_owned(),
            "*.lock".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new(
            "ide-user-data via --user-data-dir (MCP JSON + rules/skills IDE storage)",
        );
        // No config-dir env var exists; the env var below records the isolated
        // MCP path as evidence only.
        let mcp_isolated = instance
            .config_root
            .as_path()
            .join("codeium")
            .join("windsurf")
            .join("mcp_config.json");
        plan.env_vars.push((
            "WINDSURF_MCP_CONFIG".to_owned(),
            mcp_isolated.display().to_string(),
        ));
        let user_data = instance.config_root.as_path().join("vscode-data");
        let extensions = instance.config_root.as_path().join("extensions");
        plan.args.push(USER_DATA_DIR_FLAG.to_owned());
        plan.args.push(user_data.display().to_string());
        plan.args.push(EXTENSIONS_DIR_FLAG.to_owned());
        plan.args.push(extensions.display().to_string());
        plan.description = format!(
            " Wrapper execs `{} {} {}` with extensions {} and isolated MCP {}",
            EXECUTABLE,
            USER_DATA_DIR_FLAG,
            user_data.display(),
            extensions.display(),
            mcp_isolated.display()
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.codeium/windsurf/mcp_config.json".to_owned(),
            "~/.codeium/windsurf/memories".to_owned(),
            ".windsurf/rules".to_owned(),
            ".devin/rules".to_owned(),
            ".windsurfrules".to_owned(),
            "AGENTS.md".to_owned(),
            "--user-data-dir".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::IdeUserData | Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("windsurf requires isolation ide_user_data, got {other}"),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        super::skill_modes_link_first()
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            "mcp_config.json",
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::User,
            RestartBehavior::Reload,
        ))
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "plugins managed via the Customize page/Marketplace; no file-staged mechanism documented (windsurf.md)",
        )
    }
    fn usage_surface(&self) -> Option<UsageSurface> {
        Some(
            UsageSurface::new(UsageReaderKind::PlanCacheSqlite, Some("windsurf")).with_artifact(
                "plan-cache",
                PathResolver::fallback_only(
                    "~/Library/Application Support/Windsurf/User/globalStorage/state.vscdb \
                         (ItemTable key windsurf.settings.cachedPlanInfo)",
                ),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{HARNESS_ID_STR, USER_DATA_DIR_FLAG, WindsurfAdapter};
    use crate::adapter::{Adapter, DocumentKind};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> WindsurfAdapter {
        WindsurfAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-windsurf-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::IdeUserData,
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
    fn detection_has_evidence() {
        let a = adapter();
        let r = a.detection();
        assert!(!r.evidence.is_empty());
        match r.present {
            InstallPresence::Absent => assert!(r.version.is_none()),
            _ => assert!(!r.evidence.is_empty()),
        }
    }

    #[test]
    fn parse_version_ok() {
        assert_eq!(
            crate::adapters::parse_version_output("windsurf 1.0.0").as_deref(),
            Some("1.0.0")
        );
        assert_eq!(crate::adapters::parse_version_output(""), None);
    }

    #[test]
    fn surfaces_include_mcp() {
        let a = adapter();
        let s = a.config_surfaces();
        assert!(s.iter().any(|x| x.id == "mcp_config.json"));
        assert!(s.iter().any(|x| x.id == ".windsurf/rules"));
        assert!(s.iter().any(|x| x.kind == DocumentKind::Json));
    }

    #[test]
    fn operations_constrained() {
        let a = adapter();
        for (_, sup) in a.supported_operations() {
            assert_eq!(sup, AdapterSupport::Constrained);
        }
    }

    #[test]
    fn wrapper_has_user_data_dir() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".windsurf-work"));
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(plan.args.contains(&USER_DATA_DIR_FLAG.to_owned()));
        let idx = plan
            .args
            .iter()
            .position(|x| x == USER_DATA_DIR_FLAG)
            .unwrap();
        #[expect(clippy::get_unwrap, reason = "test index vetted")]
        let path = plan.args.get(idx + 1).unwrap();
        assert!(path.contains(".windsurf-work"));
        assert!(!plan.description.is_empty());
    }

    #[test]
    fn wrapper_rejects_wrong_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".windsurf-work"));
        inst.harness = HarnessId::new("cursor").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scan_candidates_cover_windsurf() {
        let a = adapter();
        let c = a.scan_candidates();
        assert!(c.iter().any(|s| s.contains("mcp_config.json")));
        assert!(c.iter().any(|s| s.contains(USER_DATA_DIR_FLAG)));
    }

    #[test]
    fn validate_accepts_ide_user_data() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".windsurf-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".windsurf-work"));
        inst.isolation = Isolation::EnvOnly;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn adapter_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
    }

    #[test]
    fn fixture_populated_loads_with_documented_keys() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/windsurf")
            .join("mcp_config.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let value = superai_config::json::load(&path).unwrap();
        assert!(
            value.contains_key("mcpServers"),
            "fixture must document `mcpServers`"
        );
        let report = crate::verification::fixture_report(path.parent().unwrap());
        assert!(report.validity_pass, "windsurf corpus validity");
        assert!(report.secret_free_pass, "windsurf corpus secret-free");
    }
}
