//! Muse Code adapter: `muse` binary, fixed `~/.config/muse` root with
//! writable `settings.json` (`mcp_servers` block); Meta-backend-only auth.

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
pub const HARNESS_ID_STR: &str = "muse-code";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Muse Code";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "muse";

/// Environment variable holding the Meta Model API key for CI auth.
pub const API_KEY_ENV_VAR: &str = "META_API_KEY";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/muse-code.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for mutation inside `settings.json`.
pub const OWNED_SELECTORS: &[&str] = &[
    "model",
    "tui",
    "permissions",
    "mcp_servers",
    "hooks",
    "managed_hooks_path",
    "runtime_capabilities",
    "telemetry",
    "agents",
];

/// Constrained note: no config-dir relocation var exists in the docs.
pub const CONSTRAINED_NOTE: &str = "no config-dir override documented; meta-backend-only auth (browser session or META_API_KEY), no BYO endpoint; beta since 2026-08";

/// Concrete adapter for Muse Code.
#[derive(Debug, Clone)]
pub struct MuseCodeAdapter {
    id: HarnessId,
}

impl MuseCodeAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var("XDG_CONFIG_HOME")
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir).join("muse"));
        }
        let home = super::home_dir()?;
        Some(home.join(".config").join("muse"))
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
        evidence.push(format!("constrained: {CONSTRAINED_NOTE}"));
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let cfg = Self::settings_path_for_root(&root);
                    if cfg.exists() {
                        evidence.push(format!("settings.json found at {}", cfg.display()));
                        match std::fs::read_to_string(&cfg) {
                            Ok(text) if text.contains("schema_version") => {
                                evidence.push("settings.json carries schema_version".to_owned());
                            }
                            Ok(_) => {}
                            Err(err) => evidence
                                .push(format!("config unreadable at {}: {err}", cfg.display())),
                        }
                    } else {
                        evidence.push(format!("settings.json missing at {}", cfg.display()));
                    }
                    let skills = root.join("skills");
                    if skills.exists() {
                        evidence.push(format!("skills dir present at {}", skills.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
        if Path::new(".muse").exists() {
            evidence.push(".muse directory present in cwd".to_owned());
        }
        if Path::new(".agents/skills").exists() {
            evidence.push(".agents/skills present in cwd (project)".to_owned());
        }
    }
}

impl Default for MuseCodeAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for MuseCodeAdapter {
    fn id(&self) -> HarnessId {
        self.id.clone()
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    fn product_status(&self) -> ProductStatus {
        ProductStatus::Preview
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
        super::resolution_from_detection(detection, "muse-code", SCHEMA_VERSION_STR)
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let settings_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/muse/settings.json"),
            Some("$XDG_CONFIG_HOME/muse/settings.json"),
            Some("%USERPROFILE%\\.config\\muse\\settings.json"),
            "~/.config/muse/settings.json",
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
        settings.restart_behavior = RestartBehavior::Restart;
        surfaces.push(settings);

        let project_hooks_resolver = PathResolver::fallback_only(".muse/hooks.json (project)");
        let mut project_hooks = ConfigSurface::new(
            "project .muse/hooks.json",
            project_hooks_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_hooks.precedence = 14;
        project_hooks.owned_selectors = vec!["hooks".to_owned()];
        project_hooks.backup_required = false;
        surfaces.push(project_hooks);

        let skills_resolver =
            PathResolver::fallback_only("$XDG_CONFIG_HOME/muse/skills/<name>/SKILL.md");
        let mut skills = ConfigSurface::new(
            "skills (user)",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills.precedence = 7;
        skills.backup_required = false;
        surfaces.push(skills);

        let project_skills_resolver =
            PathResolver::fallback_only(".agents/skills/<skill-id>/SKILL.md (project)");
        let mut project_skills = ConfigSurface::new(
            "skills (project)",
            project_skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_skills.precedence = 13;
        project_skills.backup_required = false;
        surfaces.push(project_skills);

        let agents_resolver = PathResolver::fallback_only("<repo>/AGENTS.md (lookup chain)");
        let mut agents = ConfigSurface::new(
            "AGENTS.md",
            agents_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        agents.precedence = 12;
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
            ".tmp/*".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("fixed root, meta-backend auth");
        plan.description = format!(
            " muse reads a fixed ~/.config/muse root (no relocation env documented); \
             config_root {} is informative only; auth is the shared Meta browser session \
             or {} (muse-code.md sections 2-3)",
            instance.config_root, API_KEY_ENV_VAR
        );
        plan.shared_state_warnings = vec![
            format!("auth (browser session or {API_KEY_ENV_VAR}) is shared across every profile"),
            "no BYO endpoint: the agent talks only to the Meta Model API".to_owned(),
        ];
        plan.auth_prerequisites = vec![
            crate::adapter::AuthPrerequisite::env_var(API_KEY_ENV_VAR),
            crate::adapter::AuthPrerequisite::harness_login(
                "muse auth set or first-run browser sign-in",
            ),
        ];
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/muse".to_owned(),
            "$XDG_CONFIG_HOME/muse".to_owned(),
            "~/.agents/skills".to_owned(),
            ".muse/hooks.json".to_owned(),
            "AGENTS.md".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::FixedPathSingle | Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("muse-code requires isolation fixed_path_single, got {other}"),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        super::skill_modes_link_first()
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            "settings.json",
            "mcp_servers",
            DocumentKind::Json,
            ConfigScope::User,
            RestartBehavior::Restart,
        ))
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "plugin bundles load as a skills source but no plugin path or manifest is documented (muse-code.md section 5)",
        )
    }
    fn usage_surface(&self) -> Option<UsageSurface> {
        Some(
            UsageSurface::new(UsageReaderKind::TranscriptJsonl, Some("meta")).with_artifact(
                "session-logs",
                PathResolver::new(
                    Some("$MUSE_SESSIONS_DIR, else $XDG_DATA_HOME/muse/sessions"),
                    Some("$MUSE_SESSIONS_DIR, else $XDG_DATA_HOME/muse/sessions"),
                    None,
                    "~/.local/share/muse/sessions (YYYY/MM/DD/session/session.jsonl)",
                ),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{API_KEY_ENV_VAR, HARNESS_ID_STR, MuseCodeAdapter, OWNED_SELECTORS};
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> MuseCodeAdapter {
        MuseCodeAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-muse-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::FixedPathSingle,
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
        let platforms = a.supported_platforms();
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
    fn config_surfaces_include_writable_settings_and_skills() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 5);
        let settings = surfaces
            .iter()
            .find(|s| s.id == "settings.json")
            .expect("settings.json surface must exist");
        assert_eq!(settings.kind, DocumentKind::Json);
        assert_eq!(settings.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(settings.scope, ConfigScope::User);
        assert!(settings.backup_required);
        for selector in ["mcp_servers", "permissions", "hooks"] {
            assert!(
                settings.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in OWNED_SELECTORS {
            assert!(settings.owned_selectors.contains(&(*sel).to_owned()));
        }
        assert!(
            !settings
                .owned_selectors
                .contains(&"schema_version".to_owned())
        );

        let skills = surfaces
            .iter()
            .find(|s| s.id == "skills (user)")
            .expect("skills (user)");
        assert_eq!(skills.kind, DocumentKind::TextFragment);
    }

    #[test]
    fn supported_operations_are_constrained() {
        let a = adapter();
        let ops = a.supported_operations();
        assert!(!ops.is_empty());
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
    fn plan_mirror_exclusions_cover_history_and_caches() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        let is_excluded = |file: &str| crate::adapters::exclusion_matches(&exclusions, file);
        assert!(!is_excluded("settings.json"));
        assert!(is_excluded("sessions/abc.jsonl"));
        assert!(is_excluded("logs/muse.log"));
        assert!(is_excluded("cache/data"));
    }

    #[test]
    fn plan_wrapper_declares_no_relocation_env_vars() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".muse-work"));
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars.is_empty(),
            "no relocation env vars may be fabricated: {:?}",
            plan.env_vars
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(".config/muse"));
        assert!(
            plan.shared_state_warnings
                .iter()
                .any(|w| w.contains("shared"))
        );
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".muse-work"));
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
        assert!(candidates.iter().any(|c| c.contains(".config/muse")));
        assert!(candidates.iter().any(|c| c.contains("XDG_CONFIG_HOME")));
    }

    #[test]
    fn validate_instance_accepts_fixed_and_relocated() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".muse-work"));
        a.validate_instance(&inst).unwrap();
        let mut relocated = sample_instance_with_root(&crate::test_util::tmp_abs_str(".muse-work"));
        relocated.isolation = Isolation::RelocatedRoot;
        a.validate_instance(&relocated).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".muse-work"));
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
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".muse-work"));
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let settings = surfaces.iter().find(|s| s.id == "settings.json").unwrap();
        assert_eq!(
            settings.path_resolver.fallback,
            "~/.config/muse/settings.json"
        );
        assert!(
            settings
                .path_resolver
                .linux
                .as_deref()
                .unwrap()
                .contains("XDG_CONFIG_HOME")
        );
    }

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/muse_code")
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
        assert!(map.is_empty() || map.contains_key("schema_version"));
    }

    #[test]
    fn fixture_populated_carries_schema_version_and_mcp() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert_eq!(
            map.get("schema_version"),
            Some(&serde_json::json!(1)),
            "wrong schema_version fails startup per muse-code.md section 2"
        );
        assert!(map.contains_key("mcp_servers"));
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(
            original.contains_key("vendor_future_key")
                || original.contains_key("foreignKey")
                || original.contains_key("keepMe")
        );
        let dir = crate::test_util::temp_dir_unique("muse");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("settings.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "tui".to_owned(),
                serde_json::json!({"terminal_background": "dark"}),
            );
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert!(after.contains_key("vendor_future_key") || after.contains_key("foreignKey"));
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
    fn mcp_decl_targets_settings_json_mcp_servers() {
        let a = adapter();
        let decl = a.mcp_decl().unwrap();
        assert_eq!(decl.dest_file, "settings.json");
        assert_eq!(decl.dest_key, "mcp_servers");
        assert_eq!(decl.kind, DocumentKind::Json);
        assert!(decl.read_only.is_none());
        assert!(a.mcp_absence_reason().is_none());
    }

    #[test]
    fn plugin_absence_names_the_gap() {
        let a = adapter();
        let reason = a.plugin_absence_reason().unwrap();
        assert!(reason.contains("muse-code.md"));
        assert!(a.plugin_decl().is_none());
    }

    #[test]
    fn adapter_is_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
        assert!(!boxed.config_surfaces().is_empty());
        assert!(!boxed.plan_mirror_exclusions().is_empty());
        assert!(
            boxed
                .plan_wrapper(&sample_instance_with_root(&crate::test_util::tmp_abs_str(
                    ".muse-work"
                )))
                .unwrap()
                .auth_prerequisites
                .iter()
                .any(|p| p.reference.contains(API_KEY_ENV_VAR))
        );
    }
}
