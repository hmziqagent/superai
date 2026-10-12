//! Cursor Agent CLI adapter: `agent` binary (alias `cursor-agent`),
//! `cli-config.json` relocated by `CURSOR_CONFIG_DIR`, user `mcp.json` at the
//! fixed `~/.cursor` root (the override does not move it).

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
pub const HARNESS_ID_STR: &str = "cursor-agent";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Cursor Agent CLI";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "agent";

/// Historical alias binary shipped by the same installer.
pub const EXECUTABLE_ALIAS: &str = "cursor-agent";

/// Environment variable that relocates `cli-config.json` (only).
pub const CONFIG_ENV_VAR: &str = "CURSOR_CONFIG_DIR";

/// Environment variable carrying an API key for CI auth (reference only).
pub const API_KEY_ENV_VAR: &str = "CURSOR_API_KEY";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/cursor-agent.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside `cli-config.json`.
pub const OWNED_SELECTORS: &[&str] = &[
    "version",
    "editor",
    "permissions",
    "channel",
    "model",
    "maxMode",
    "notifications",
    "hints",
    "rewind",
    "suggestNextPrompt",
    "display",
    "approvalMode",
    "sandbox",
    "network",
    "attribution",
];

/// Constrained note: Cursor-backend-only auth; MCP files ignore the override.
pub const CONSTRAINED_NOTE: &str = "auth is exclusively the Cursor backend (browser login or CURSOR_API_KEY; IDE BYOK does not extend to the CLI); CURSOR_CONFIG_DIR moves cli-config.json only — mcp.json still reads $HOME/.cursor and $PWD/.cursor (live-probed, cursor-agent.md section 2)";

/// Concrete adapter for the Cursor Agent CLI.
#[derive(Debug, Clone)]
pub struct CursorAgentAdapter {
    id: HarnessId,
}

impl CursorAgentAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    fn config_evidence_paths(
        config_env: Option<&str>,
        home: Option<&Path>,
    ) -> (Option<PathBuf>, Option<PathBuf>) {
        let overridden = config_env.is_some_and(|dir| !dir.trim().is_empty());
        let cli_config = if overridden {
            config_env.map(|dir| Path::new(dir).join("cli-config.json"))
        } else {
            home.map(|h| h.join(".cursor").join("cli-config.json"))
        };
        // mcp.json ignores CURSOR_CONFIG_DIR (live-probed; CONSTRAINED_NOTE).
        let mcp = home.map(|h| h.join(".cursor").join("mcp.json"));
        (cli_config, mcp)
    }

    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("constrained: {CONSTRAINED_NOTE}"));
        let config_env = std::env::var(CONFIG_ENV_VAR).ok();
        let home = super::home_dir();
        let (cli_config, mcp) = Self::config_evidence_paths(config_env.as_deref(), home.as_deref());
        match cli_config {
            Some(cfg) => Self::push_cli_config_evidence(&cfg, evidence),
            None => {
                evidence.push("could not resolve cli-config.json path (no HOME)".to_owned());
            }
        }
        if let Some(path) = mcp
            && path.exists()
        {
            evidence.push(format!(
                "mcp.json found at fixed {} (override ignored)",
                path.display()
            ));
        }
        if let Some(dir) = config_env.filter(|dir| !dir.trim().is_empty()) {
            evidence.push(format!(
                "{CONFIG_ENV_VAR} set to {dir} (moves cli-config.json only)"
            ));
        }
        if Path::new(".cursor/mcp.json").exists() {
            evidence.push("project .cursor/mcp.json present in cwd".to_owned());
        }
        if Path::new(".cursor/cli.json").exists() {
            evidence.push("project .cursor/cli.json present in cwd".to_owned());
        }
    }

    fn push_cli_config_evidence(cfg: &Path, evidence: &mut Vec<String>) {
        if !cfg.exists() {
            evidence.push(format!("cli-config.json missing at {}", cfg.display()));
            return;
        }
        evidence.push(format!("cli-config.json found at {}", cfg.display()));
        match std::fs::read_to_string(cfg) {
            Ok(text) if text.contains("version") => {
                evidence.push("cli-config.json carries version".to_owned());
            }
            Ok(_) => {}
            Err(err) => {
                evidence.push(format!("config unreadable at {}: {err}", cfg.display()));
            }
        }
    }
}

impl Default for CursorAgentAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for CursorAgentAdapter {
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

        match super::find_in_path(&[EXECUTABLE, EXECUTABLE_ALIAS]) {
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
                evidence.push(format!(
                    "binaries `{EXECUTABLE}`/`{EXECUTABLE_ALIAS}` not found in PATH"
                ));
            }
        }

        self.collect_config_evidence(&mut evidence);

        let present = super::install_presence(binary_path.is_some(), version.is_some());

        let confidence =
            super::detection_confidence(binary_path.is_some(), version.is_some(), false);
        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution_from(&self, detection: &DetectionResult) -> VersionResolution {
        super::resolution_from_detection(detection, "cursor-agent", SCHEMA_VERSION_STR)
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let cli_config_resolver = PathResolver::new(
            Some("$CURSOR_CONFIG_DIR/cli-config.json"),
            Some("$CURSOR_CONFIG_DIR/cli-config.json"),
            Some("%USERPROFILE%\\.cursor\\cli-config.json"),
            "~/.cursor/cli-config.json",
        );
        let mut cli_config = ConfigSurface::new(
            "cli-config.json",
            cli_config_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        cli_config.precedence = 10;
        cli_config.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        cli_config.backup_required = true;
        cli_config.restart_behavior = RestartBehavior::Reload;
        surfaces.push(cli_config);

        let user_mcp_resolver = PathResolver::fallback_only("~/.cursor/mcp.json (fixed $HOME)");
        let mut user_mcp = ConfigSurface::new(
            "mcp.json",
            user_mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        user_mcp.precedence = 11;
        user_mcp.owned_selectors = vec!["mcpServers".to_owned()];
        user_mcp.backup_required = true;
        user_mcp.restart_behavior = RestartBehavior::Reload;
        surfaces.push(user_mcp);

        let project_cli_resolver = PathResolver::fallback_only(".cursor/cli.json (project)");
        let mut project_cli = ConfigSurface::new(
            "project .cursor/cli.json",
            project_cli_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_cli.precedence = 14;
        project_cli.owned_selectors = vec!["permissions".to_owned()];
        project_cli.backup_required = false;
        surfaces.push(project_cli);

        let project_mcp_resolver = PathResolver::fallback_only(".cursor/mcp.json (project)");
        let mut project_mcp = ConfigSurface::new(
            "project .cursor/mcp.json",
            project_mcp_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_mcp.precedence = 15;
        project_mcp.owned_selectors = vec!["mcpServers".to_owned()];
        project_mcp.backup_required = false;
        surfaces.push(project_mcp);

        let rules_resolver = PathResolver::fallback_only(".cursor/rules/*.mdc (project)");
        let mut rules = ConfigSurface::new(
            "rules",
            rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        rules.precedence = 12;
        rules.backup_required = false;
        surfaces.push(rules);

        let agents_resolver =
            PathResolver::fallback_only("AGENTS.md (project; CLAUDE.md also read)");
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
            "sessions/*".to_owned(),
            "logs/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("relocated cli-config via CURSOR_CONFIG_DIR");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        plan.description = format!(
            " Wrapper sets {}={} and execs `{}` ({} moves cli-config.json only)",
            CONFIG_ENV_VAR, instance.config_root, EXECUTABLE, CONFIG_ENV_VAR
        );
        plan.shared_state_warnings = vec![
            "mcp.json ignores CURSOR_CONFIG_DIR and still reads $HOME/.cursor plus $PWD/.cursor: MCP isolation needs per-project files or a HOME swap"
                .to_owned(),
            format!("auth is Cursor-backend only (browser login or {API_KEY_ENV_VAR}); IDE BYOK does not extend to the CLI"),
        ];
        plan.auth_prerequisites = vec![
            crate::adapter::AuthPrerequisite::env_var(API_KEY_ENV_VAR),
            crate::adapter::AuthPrerequisite::harness_login("agent login (browser OAuth)"),
        ];
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.cursor/cli-config.json".to_owned(),
            "$CURSOR_CONFIG_DIR".to_owned(),
            ".cursor/cli.json".to_owned(),
            ".cursor/mcp.json".to_owned(),
            ".cursor/rules".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("cursor-agent requires isolation relocated_root, got {other}"),
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
            "a --plugin-dir flag exists but no plugin layout or manifest is published (cursor-agent.md sections 5, 8)",
        )
    }
    fn usage_absence_reason(&self) -> Option<&'static str> {
        Some("usage truth is Cursor's account usage API; no local usage artifact")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    use super::{
        API_KEY_ENV_VAR, CONFIG_ENV_VAR, CursorAgentAdapter, HARNESS_ID_STR, OWNED_SELECTORS,
    };
    use crate::adapter::{Adapter, DocumentKind, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> CursorAgentAdapter {
        CursorAgentAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-cursor-agent-1").unwrap(),
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
        assert!(result.evidence.iter().any(|e| e.contains("constrained")));
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
    fn evidence_probe_moves_cli_config_but_not_mcp_json() {
        let (cli, mcp) = CursorAgentAdapter::config_evidence_paths(
            Some("/tmp/cursor-override"),
            Some(Path::new("/home/tester")),
        );
        assert_eq!(
            cli,
            Some(PathBuf::from("/tmp/cursor-override/cli-config.json"))
        );
        assert_eq!(
            mcp,
            Some(PathBuf::from("/home/tester/.cursor/mcp.json")),
            "mcp.json must stay on the fixed $HOME root, never the override dir"
        );
    }

    #[test]
    fn evidence_probe_without_override_uses_home_for_both() {
        let (cli, mcp) =
            CursorAgentAdapter::config_evidence_paths(None, Some(Path::new("/home/tester")));
        assert_eq!(
            cli,
            Some(PathBuf::from("/home/tester/.cursor/cli-config.json"))
        );
        assert_eq!(mcp, Some(PathBuf::from("/home/tester/.cursor/mcp.json")));
    }

    #[test]
    fn evidence_probe_blank_override_falls_back_to_home() {
        let (cli, _) =
            CursorAgentAdapter::config_evidence_paths(Some("  "), Some(Path::new("/home/tester")));
        assert_eq!(
            cli,
            Some(PathBuf::from("/home/tester/.cursor/cli-config.json"))
        );
    }

    #[test]
    fn evidence_probe_without_home_has_no_mcp_target() {
        let (cli, mcp) =
            CursorAgentAdapter::config_evidence_paths(Some("/tmp/cursor-override"), None);
        assert_eq!(
            cli,
            Some(PathBuf::from("/tmp/cursor-override/cli-config.json"))
        );
        assert_eq!(mcp, None);
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
    fn config_surfaces_split_relocated_and_fixed() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 6);
        let cli_config = surfaces
            .iter()
            .find(|s| s.id == "cli-config.json")
            .expect("cli-config.json surface must exist");
        assert_eq!(cli_config.kind, DocumentKind::Json);
        assert_eq!(cli_config.ownership, SurfaceOwnership::UserEditable);
        assert!(cli_config.backup_required);
        for sel in OWNED_SELECTORS {
            assert!(cli_config.owned_selectors.contains(&(*sel).to_owned()));
        }
        assert!(
            cli_config
                .path_resolver
                .linux
                .as_deref()
                .unwrap()
                .contains(CONFIG_ENV_VAR),
            "cli-config.json must resolve through the override"
        );

        let user_mcp = surfaces
            .iter()
            .find(|s| s.id == "mcp.json")
            .expect("user mcp.json surface");
        assert!(
            !user_mcp.path_resolver.fallback.contains(CONFIG_ENV_VAR),
            "mcp.json ignores CURSOR_CONFIG_DIR (live-probed)"
        );

        for other in [
            "project .cursor/cli.json",
            "project .cursor/mcp.json",
            "rules",
            "AGENTS.md",
        ] {
            assert!(surfaces.iter().any(|s| s.id == other), "missing {other}");
        }
    }

    #[test]
    fn supported_operations_are_constrained() {
        let a = adapter();
        let ops = a.supported_operations();
        for (name, support) in &ops {
            assert_eq!(
                *support,
                AdapterSupport::Constrained,
                "operation {name} should be Constrained"
            );
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
    fn plan_mirror_exclusions_cover_sessions_and_logs() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        let is_excluded = |file: &str| crate::adapters::exclusion_matches(&exclusions, file);
        assert!(!is_excluded("cli-config.json"));
        assert!(!is_excluded("mcp.json"));
        assert!(is_excluded("sessions/abc.json"));
        assert!(is_excluded("logs/agent.log"));
    }

    #[test]
    fn plan_wrapper_sets_cursor_config_dir() {
        let tmp_root = crate::test_util::tmp_abs_str(".cursor-agent-work");
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
        assert!(
            plan.shared_state_warnings
                .iter()
                .any(|w| w.contains("mcp.json ignores"))
        );
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let tmp_root = crate::test_util::tmp_abs_str("my cursor agent work");
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
        let mut inst =
            sample_instance_with_root(&crate::test_util::tmp_abs_str(".cursor-agent-work"));
        inst.harness = HarnessId::new("cursor").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_cli_config_and_project_files() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("cli-config.json")));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
        assert!(candidates.iter().any(|c| c.contains(".cursor/mcp.json")));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".cursor-agent-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst =
            sample_instance_with_root(&crate::test_util::tmp_abs_str(".cursor-agent-work"));
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
        let mut inst =
            sample_instance_with_root(&crate::test_util::tmp_abs_str(".cursor-agent-work"));
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/cursor_agent")
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
        let path = fixture_path("cli-config.minimal.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert_eq!(
            map.get("version"),
            Some(&serde_json::json!(1)),
            "cli-config.json requires version: 1"
        );
    }

    #[test]
    fn fixture_populated_carries_permissions_and_editor() {
        let path = fixture_path("cli-config.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.contains_key("permissions"));
        assert!(map.contains_key("editor"));
        assert!(map.contains_key("approvalMode"));
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("cli-config.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(
            original.contains_key("vendorKey") || original.contains_key("foreignKey"),
            "foreign fixture must carry a foreign key"
        );
        let dir = crate::test_util::temp_dir_unique("cursor-agent");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("cli-config.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "channel".to_owned(),
                serde_json::Value::String("stable".to_owned()),
            );
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert!(
            after.contains_key("vendorKey") || after.contains_key("foreignKey"),
            "foreign keys must be preserved, got {after:?}"
        );
        drop(std::fs::remove_file(&tmp));
    }

    #[test]
    fn fixture_malformed_fails_to_parse() {
        let path = fixture_path("cli-config.malformed.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::json::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn mcp_decl_targets_user_mcp_json() {
        let a = adapter();
        let decl = a.mcp_decl().unwrap();
        assert_eq!(decl.dest_file, "mcp.json");
        assert_eq!(decl.dest_key, "mcpServers");
        assert!(decl.read_only.is_none());
        assert!(a.mcp_absence_reason().is_none());
    }

    #[test]
    fn plugin_absence_names_the_flag_only_gap() {
        let a = adapter();
        let reason = a.plugin_absence_reason().unwrap();
        assert!(reason.contains("--plugin-dir"));
        assert!(a.plugin_decl().is_none());
    }

    #[test]
    fn adapter_is_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
        assert!(!boxed.config_surfaces().is_empty());
        assert!(
            boxed
                .plan_wrapper(&sample_instance_with_root(&crate::test_util::tmp_abs_str(
                    ".cursor-agent-work"
                )))
                .unwrap()
                .auth_prerequisites
                .iter()
                .any(|p| p.reference.contains(API_KEY_ENV_VAR))
        );
    }
}
