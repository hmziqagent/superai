//! Cursor adapter: `CURSOR_CONFIG_DIR` plus IDE `--user-data-dir` isolation.
//! Research source: `docs/harness-configs/cursor.md` (last verified 2026-08-25).

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
pub const HARNESS_ID_STR: &str = "cursor";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Cursor IDE and Agent CLI";

/// Primary IDE executable.
pub const EXECUTABLE: &str = "cursor";

/// CLI primary executable.
pub const EXECUTABLE_CLI: &str = "agent";

/// Legacy CLI executable.
pub const EXECUTABLE_LEGACY: &str = "cursor-agent";

/// Environment variable that relocates the CLI config dir.
pub const CONFIG_ENV_VAR: &str = "CURSOR_CONFIG_DIR";

/// API key env var for CLI auth.
pub const API_KEY_ENV_VAR: &str = "CURSOR_API_KEY";

/// Flag for IDE user-data isolation.
pub const USER_DATA_DIR_FLAG: &str = "--user-data-dir";

/// Flag for extensions dir isolation.
pub const EXTENSIONS_DIR_FLAG: &str = "--extensions-dir";

/// The agent ignores `CURSOR_CONFIG_DIR`/`XDG_CONFIG_HOME` for `mcp.json` (probe-verified 2026-09-18, mcp-readpath-r6.log).
pub const MCP_READ_PATH_FALLBACK: &str = "~/.cursor/mcp.json";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/cursor.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside `cli-config.json`.
pub const OWNED_SELECTORS: &[&str] = &[
    "permissions.allow",
    "permissions.deny",
    "model",
    "editor.vimMode",
    "sandbox.mode",
    "mcpServers",
    "permissions",
];

/// Selectors superai owns on the MCP surface; other keys round-trip.
pub const MCP_OWNED_SELECTORS: &[&str] = &["mcpServers"];

/// Concrete adapter for Cursor.
#[derive(Debug, Clone)]
pub struct CursorAdapter {
    id: HarnessId,
}

impl CursorAdapter {
    /// Create a new adapter instance.
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
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
            && !xdg.trim().is_empty()
        {
            return Some(PathBuf::from(xdg).join("cursor"));
        }
        let home = super::home_dir()?;
        Some(home.join(".cursor"))
    }

    fn mcp_read_path() -> Option<PathBuf> {
        let home = super::home_dir()?;
        Some(home.join(".cursor").join("mcp.json"))
    }

    fn default_user_data_root() -> Option<PathBuf> {
        let home = super::home_dir()?;
        if cfg!(target_os = "macos") {
            Some(
                home.join("Library")
                    .join("Application Support")
                    .join("Cursor")
                    .join("User"),
            )
        } else if cfg!(windows) {
            if let Ok(appdata) = std::env::var("APPDATA")
                && !appdata.trim().is_empty()
            {
                return Some(PathBuf::from(appdata).join("Cursor").join("User"));
            }
            Some(
                home.join("AppData")
                    .join("Roaming")
                    .join("Cursor")
                    .join("User"),
            )
        } else {
            Some(home.join(".config").join("Cursor").join("User"))
        }
    }

    #[expect(clippy::excessive_nesting, reason = "evidence branches explicit")]
    #[expect(clippy::unused_self, reason = "uses via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let cli_config = root.join("cli-config.json");
                    let alt = root.join("cli.json");
                    if cli_config.exists() {
                        evidence.push(format!("cli-config.json found at {}", cli_config.display()));
                        match std::fs::read_to_string(&cli_config) {
                            Ok(text) if text.contains("permissions") => {
                                evidence.push("cli-config.json contains permissions".to_owned());
                            }
                            Ok(_) => {}
                            Err(err) => evidence.push(format!(
                                "config unreadable at {}: {err}",
                                cli_config.display()
                            )),
                        }
                    } else if alt.exists() {
                        evidence.push(format!("cli.json found at {}", alt.display()));
                    } else {
                        evidence.push(format!(
                            "cli-config.json missing at {}",
                            cli_config.display()
                        ));
                    }
                    let rules = Path::new(".cursor").join("rules");
                    if rules.exists() {
                        evidence.push(format!(".cursor/rules present at {}", rules.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => evidence.push("could not resolve config root (no HOME)".to_owned()),
        }
        // `mcp.json` lives at the HOME-based live read path, NOT under the
        // (env-relocated) config root that holds `cli-config.json`.
        if let Some(mcp) = Self::mcp_read_path()
            && mcp.exists()
        {
            evidence.push(format!("mcp.json found at {}", mcp.display()));
        }
        if let Some(user_data) = Self::default_user_data_root() {
            if user_data.exists() {
                evidence.push(format!(
                    "Cursor User data exists at {}",
                    user_data.display()
                ));
                let settings = user_data.join("settings.json");
                if settings.exists() {
                    evidence.push(format!("settings.json found at {}", settings.display()));
                }
            } else {
                evidence.push(format!(
                    "Cursor User data missing at {}",
                    user_data.display()
                ));
            }
        }
        if let Ok(val) = std::env::var(CONFIG_ENV_VAR)
            && !val.trim().is_empty()
        {
            evidence.push(format!("{CONFIG_ENV_VAR} set to {val}"));
        } else {
            evidence.push(format!("{CONFIG_ENV_VAR} not set, using ~/.cursor"));
        }
        for p in [Path::new(".cursorignore"), Path::new(".cursor/mcp.json")] {
            if p.exists() {
                evidence.push(format!("{} exists", p.display()));
            }
        }
        if let Ok(val) = std::env::var(API_KEY_ENV_VAR)
            && !val.trim().is_empty()
        {
            evidence.push(format!("{API_KEY_ENV_VAR} is set (len {})", val.len()));
        }
    }
}

impl Default for CursorAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for CursorAdapter {
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
        match super::find_in_path(&[EXECUTABLE, EXECUTABLE_CLI, EXECUTABLE_LEGACY]) {
            Some(path) => {
                evidence.push(format!(
                    "found binary `{}` at {}",
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("cursor"),
                    path.display()
                ));
                match super::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `--version`"));
                        version = Some(v);
                    }
                    None => evidence.push(
                        "version probe failed for `--version` (timeout or non-zero)".to_owned(),
                    ),
                }
                binary_path = Some(path);
            }
            None => {
                evidence.push(format!("binary `{EXECUTABLE}` not found in PATH"));
                evidence.push(format!("binary `{EXECUTABLE_CLI}` not found in PATH"));
            }
        }
        self.collect_config_evidence(&mut evidence);
        let present = super::install_presence(binary_path.is_some(), version.is_some());
        let confidence =
            super::detection_confidence(binary_path.is_some(), version.is_some(), false);
        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution_from(&self, detection: &DetectionResult) -> VersionResolution {
        super::resolution_from_detection(detection, "cursor", SCHEMA_VERSION_STR)
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let cli_resolver = PathResolver::new(
            Some("$CURSOR_CONFIG_DIR/cli-config.json"),
            Some("$CURSOR_CONFIG_DIR/cli-config.json"),
            Some("%CURSOR_CONFIG_DIR%\\cli-config.json"),
            "~/.cursor/cli-config.json",
        );
        let mut cli = ConfigSurface::new(
            "cli-config.json",
            cli_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        cli.precedence = 10;
        cli.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        cli.backup_required = true;
        cli.restart_behavior = RestartBehavior::Reload;
        surfaces.push(cli);

        let project_cli_resolver = PathResolver::fallback_only(".cursor/cli.json (project)");
        let mut project_cli = ConfigSurface::new(
            "project cli.json",
            project_cli_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_cli.precedence = 12;
        project_cli.owned_selectors = vec!["permissions".to_owned()];
        project_cli.backup_required = true;
        surfaces.push(project_cli);

        // `agent mcp list` reads only $HOME/.cursor/mcp.json (plus the
        // project copy); env-relocated mcp.json copies are ignored.
        let mcp_resolver = PathResolver::new(
            Some(MCP_READ_PATH_FALLBACK),
            Some(MCP_READ_PATH_FALLBACK),
            Some("%USERPROFILE%\\.cursor\\mcp.json"),
            MCP_READ_PATH_FALLBACK,
        );
        let mut mcp = ConfigSurface::new(
            "mcp.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp.precedence = 11;
        mcp.owned_selectors = MCP_OWNED_SELECTORS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        mcp.backup_required = true;
        surfaces.push(mcp);

        let project_mcp_resolver = PathResolver::fallback_only(".cursor/mcp.json (project)");
        let mut project_mcp = ConfigSurface::new(
            "project mcp.json",
            project_mcp_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_mcp.precedence = 13;
        project_mcp.owned_selectors = MCP_OWNED_SELECTORS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        project_mcp.backup_required = true;
        surfaces.push(project_mcp);

        let cursor_settings_resolver = PathResolver::new(
            Some("~/.config/Cursor/User/settings.json"),
            Some("~/Library/Application Support/Cursor/User/settings.json"),
            Some("%APPDATA%\\Cursor\\User\\settings.json"),
            "~/.config/Cursor/User/settings.json",
        );
        let mut settings = ConfigSurface::new(
            "cursor settings.json",
            cursor_settings_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        settings.precedence = 8;
        settings.backup_required = true;
        surfaces.push(settings);

        let rules_resolver = PathResolver::fallback_only(".cursor/rules/*.mdc");
        let mut rules = ConfigSurface::new(
            ".cursor/rules",
            rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        rules.precedence = 14;
        rules.backup_required = false;
        surfaces.push(rules);

        let ignore_resolver = PathResolver::fallback_only(".cursorignore (project root)");
        let mut ignore = ConfigSurface::new(
            ".cursorignore",
            ignore_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        ignore.precedence = 5;
        ignore.backup_required = false;
        surfaces.push(ignore);

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
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "tmp/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "*.lock".to_owned(),
            "history/*".to_owned(),
            "worktrees.json".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("ide-user-data via --user-data-dir + CURSOR_CONFIG_DIR");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        let user_data = instance.config_root.as_path().join("vscode-data");
        let extensions = instance.config_root.as_path().join("extensions");
        plan.args.push(USER_DATA_DIR_FLAG.to_owned());
        plan.args.push(user_data.display().to_string());
        plan.args.push(EXTENSIONS_DIR_FLAG.to_owned());
        plan.args.push(extensions.display().to_string());
        plan.description = format!(
            " Wrapper sets {}={} and execs `{} {} {}` with extensions {}",
            CONFIG_ENV_VAR,
            instance.config_root,
            EXECUTABLE,
            USER_DATA_DIR_FLAG,
            user_data.display(),
            extensions.display()
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.cursor/cli-config.json".to_owned(),
            "~/.cursor/mcp.json".to_owned(),
            "$CURSOR_CONFIG_DIR/cli-config.json".to_owned(),
            "$HOME/.cursor/mcp.json (mcp.json ignores CURSOR_CONFIG_DIR)".to_owned(),
            ".cursor/cli.json".to_owned(),
            ".cursor/mcp.json".to_owned(),
            "~/.config/Cursor/User/settings.json".to_owned(),
            "--user-data-dir".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::IdeUserData
            | Isolation::RelocatedRoot
            | Isolation::ExplicitConfig
            | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("cursor requires isolation ide_user_data, got {other}"),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        super::skill_modes_link_first()
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            "mcp.json",
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::User,
            RestartBehavior::Reload,
        ))
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "plugins managed via the Cursor Marketplace UI; no file-staged mechanism documented (cursor.md)",
        )
    }
    fn usage_absence_reason(&self) -> Option<&'static str> {
        Some("usage truth is Cursor's account usage API; no local usage artifact")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONFIG_ENV_VAR, CursorAdapter, HARNESS_ID_STR, MCP_READ_PATH_FALLBACK, OWNED_SELECTORS,
        USER_DATA_DIR_FLAG,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};
    use std::collections::HashSet;

    fn adapter() -> CursorAdapter {
        CursorAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-cursor-1").unwrap(),
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
    fn detection_returns_evidence() {
        let a = adapter();
        let r = a.detection();
        assert!(!r.evidence.is_empty());
        match r.present {
            InstallPresence::Absent => assert!(r.version.is_none()),
            InstallPresence::Present => assert!(r.version.is_some()),
            InstallPresence::UnknownVersion | InstallPresence::Broken => {
                assert!(!r.evidence.is_empty());
            }
        }
    }

    #[test]
    fn parse_version_cases() {
        assert_eq!(
            crate::adapters::parse_version_output("cursor 1.2.3").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            crate::adapters::parse_version_output("agent 0.5.0").as_deref(),
            Some("0.5.0")
        );
        assert_eq!(crate::adapters::parse_version_output(""), None);
        assert_eq!(crate::adapters::parse_version_output("not a version"), None);
    }

    #[test]
    fn config_surfaces_include_cli_and_mcp() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 5);
        let cli = surfaces.iter().find(|s| s.id == "cli-config.json").unwrap();
        assert_eq!(cli.kind, DocumentKind::Json);
        assert_eq!(cli.scope, ConfigScope::User);
        assert_eq!(cli.ownership, SurfaceOwnership::UserEditable);
        assert!(cli.backup_required);
        for sel in OWNED_SELECTORS {
            assert!(cli.owned_selectors.contains(&(*sel).to_owned()));
        }
        let mcp = surfaces.iter().find(|s| s.id == "mcp.json").unwrap();
        assert!(mcp.owned_selectors.contains(&"mcpServers".to_owned()));
    }

    /// Live-probe pin (2026-09-18, agent 2026.09.15-d2fe57e): mcp.json is read
    /// only from $HOME/.cursor/mcp.json (+ project copy), never `$CURSOR_CONFIG_DIR`.
    #[test]
    fn mcp_surface_pins_home_cursor_read_path() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let mcp = surfaces
            .iter()
            .find(|s| s.id == "mcp.json")
            .expect("mcp.json surface");
        assert_eq!(
            mcp.path_resolver.linux.as_deref(),
            Some(MCP_READ_PATH_FALLBACK)
        );
        assert_eq!(
            mcp.path_resolver.macos.as_deref(),
            Some(MCP_READ_PATH_FALLBACK)
        );
        assert_eq!(
            mcp.path_resolver.windows.as_deref(),
            Some("%USERPROFILE%\\.cursor\\mcp.json")
        );
        let hints = [
            mcp.path_resolver.linux.as_deref(),
            mcp.path_resolver.macos.as_deref(),
            mcp.path_resolver.windows.as_deref(),
            Some(mcp.path_resolver.fallback.as_str()),
        ];
        for hint in hints.into_iter().flatten() {
            assert!(
                !hint.contains(CONFIG_ENV_VAR),
                "live agent ignores {CONFIG_ENV_VAR} for mcp.json, hint: {hint}"
            );
        }
        // The env-relocated config root remains correct for cli-config.json.
        let cli = surfaces
            .iter()
            .find(|s| s.id == "cli-config.json")
            .expect("cli-config.json surface");
        assert_eq!(
            cli.path_resolver.linux.as_deref(),
            Some("$CURSOR_CONFIG_DIR/cli-config.json")
        );
    }

    #[test]
    fn scan_candidates_do_not_claim_env_relocated_mcp_json() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("mcp.json")));
        assert!(
            !candidates
                .iter()
                .any(|c| c == "$CURSOR_CONFIG_DIR/mcp.json"),
            "scan must not hint the env-relocated mcp.json path the live agent ignores"
        );
    }

    #[test]
    fn supported_operations_constrained() {
        let a = adapter();
        for (_, support) in a.supported_operations() {
            assert_eq!(support, AdapterSupport::Constrained);
        }
    }

    #[test]
    fn plan_wrapper_sets_env_and_user_data_dir() {
        let tmp_root = crate::test_util::tmp_abs_str(".cursor-work");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == tmp_root.as_str())
        );
        assert!(plan.args.contains(&USER_DATA_DIR_FLAG.to_owned()));
        let idx = plan
            .args
            .iter()
            .position(|x| x == USER_DATA_DIR_FLAG)
            .unwrap();
        #[expect(clippy::get_unwrap, reason = "test index vetted")]
        let path = plan.args.get(idx + 1).unwrap();
        assert!(path.contains(".cursor-work"));
        assert!(!plan.description.is_empty());
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".cursor-work"));
        inst.harness = HarnessId::new("claude-code").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_cursor_paths() {
        let a = adapter();
        let c = a.scan_candidates();
        assert!(c.iter().any(|s| s.contains(".cursor")));
        assert!(c.iter().any(|s| s.contains(CONFIG_ENV_VAR)));
        assert!(c.iter().any(|s| s.contains(USER_DATA_DIR_FLAG)));
    }

    #[test]
    fn validate_instance_accepts_ide_user_data() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".cursor-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".cursor-work"));
        inst.isolation = Isolation::EnvOnly;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn owned_selectors_unique() {
        let set: HashSet<&str> = OWNED_SELECTORS.iter().copied().collect();
        assert_eq!(set.len(), OWNED_SELECTORS.len());
    }

    #[test]
    fn adapter_is_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
        assert!(!boxed.config_surfaces().is_empty());
    }

    #[test]
    fn fixture_populated_loads_with_documented_keys() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/cursor")
            .join("cli-config.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let value = superai_config::json::load(&path).unwrap();
        assert!(value.contains_key("model"), "fixture must document `model`");
        let report = crate::verification::fixture_report(path.parent().unwrap());
        assert!(report.validity_pass, "cursor corpus validity");
        assert!(report.secret_free_pass, "cursor corpus secret-free");
    }
}
