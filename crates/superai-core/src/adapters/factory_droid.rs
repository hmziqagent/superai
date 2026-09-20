//! Factory Droid adapter: `droid` binary, layered `~/.factory/settings.json`
//! with project overlay, `project-scope` isolation via HOME relocation.
//! Research source: `docs/harness-configs/factory-droid.md` (last verified
//! 2026-08-25; MCP destination live-verified 2026-09-18).

use std::path::{Path, PathBuf};

use crate::adapter::{
    ADAPTER_REVISION, Adapter, Arch, ConfigScope, ConfigSurface, DetectionConfidence,
    DetectionResult, DocumentKind, Os, PathResolver, Platform, ProductStatus, RestartBehavior,
    SurfaceOwnership, VersionResolution, WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, InstallPresence, Isolation};

/// Harness identifier for Factory Droid.
pub const HARNESS_ID_STR: &str = "factory-droid";

/// Human display name.
pub const DISPLAY_NAME: &str = "Factory Droid";

/// Primary executable name.
pub const EXECUTABLE: &str = "droid";

/// Alternative executable name.
pub const EXECUTABLE_ALT: &str = "factory";

/// Environment variable for API key auth.
pub const API_KEY_ENV_VAR: &str = "FACTORY_API_KEY";

/// Default config root fallback.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.factory";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/factory-droid.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current config shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for Factory Droid inside `settings.json`: local
/// customModels and tool gating only; hosted/org policy keys stay foreign.
pub const OWNED_SELECTORS: &[&str] = &[
    "customModels",
    "model",
    "reasoningEffort",
    "commandAllowlist",
    "commandDenylist",
    "commandBlocklist",
    "disabledSkills",
    "mcpServers",
    "modelFallbacks",
];

/// Concrete adapter for Factory Droid.
#[derive(Debug, Clone)]
pub struct FactoryDroidAdapter {
    id: HarnessId,
}

impl FactoryDroidAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    /// Borrow the harness id.
    pub fn harness_id(&self) -> &HarnessId {
        &self.id
    }

    /// Executable name for this harness.
    pub fn executable_name(&self) -> &str {
        EXECUTABLE
    }

    /// Resolve the default config root: `~/.factory`.
    fn default_config_root() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".factory"))
    }

    /// Build the settings.json path for a given root.
    fn settings_path_for_root(root: &Path) -> PathBuf {
        root.join("settings.json")
    }

    /// Collect config evidence.
    #[expect(clippy::excessive_nesting, reason = "detection branches are explicit")]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let settings = Self::settings_path_for_root(&root);
                    if settings.exists() {
                        evidence.push(format!("settings.json found at {}", settings.display()));
                        if let Ok(text) = std::fs::read_to_string(&settings)
                            && text.contains("customModels")
                        {
                            evidence.push("settings.json contains customModels".to_owned());
                        }
                    } else {
                        evidence.push(format!("settings.json missing at {}", settings.display()));
                    }
                    if root.join("mcp.json").exists() {
                        evidence.push(format!(
                            "mcp.json present at {}",
                            root.join("mcp.json").display()
                        ));
                    }
                    if root.join("skills").exists() {
                        evidence.push("skills directory present".to_owned());
                    }
                    if root.join("droids").exists() {
                        evidence.push("droids directory present".to_owned());
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
        if let Ok(val) = std::env::var(API_KEY_ENV_VAR)
            && !val.trim().is_empty()
        {
            evidence.push(format!("{API_KEY_ENV_VAR} is set (len {})", val.len()));
        } else {
            evidence.push(format!("{API_KEY_ENV_VAR} not set"));
        }
        if Path::new(".factory/settings.json").exists() {
            evidence.push("project .factory/settings.json present".to_owned());
        }
        if Path::new(".droid.yaml").exists() {
            evidence.push("legacy .droid.yaml present (deprecated)".to_owned());
        }
    }
}

impl Default for FactoryDroidAdapter {
    fn default() -> Self {
        #[expect(
            clippy::unwrap_used,
            reason = "factory-droid is static valid HarnessId"
        )]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for FactoryDroidAdapter {
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
        vec![
            Platform::new(Os::Linux, Arch::Any),
            Platform::new(Os::Macos, Arch::Any),
            Platform::new(Os::Windows, Arch::Any),
        ]
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

        match super::find_in_path(&[EXECUTABLE, EXECUTABLE_ALT]) {
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

        let present = match (&binary_path, &version) {
            (Some(_), Some(_)) => InstallPresence::Present,
            (Some(_), None) => InstallPresence::UnknownVersion,
            (None, _) => InstallPresence::Absent,
        };

        // Absent forces High: the Low "config root exists" arm below can
        // never survive that override.
        let confidence = match (&binary_path, &version) {
            (Some(_), None) => DetectionConfidence::Medium,
            (Some(_), Some(_)) | (None, _) => DetectionConfidence::High,
        };

        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution(&self) -> VersionResolution {
        let detection = self.detection();
        if let Some(v) = detection.version {
            let mut notes = Vec::new();
            notes.push(format!("detected factory-droid version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            let mut res =
                VersionResolution::new(Some(v), Some(SCHEMA_VERSION_STR.to_owned()), true);
            res.notes = notes;
            res
        } else {
            let mut res = VersionResolution::unknown();
            res.notes = detection.evidence;
            res
        }
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let settings_resolver = PathResolver::new(
            Some("~/.factory/settings.json"),
            Some("~/.factory/settings.json"),
            Some("%USERPROFILE%\\.factory\\settings.json"),
            "~/.factory/settings.json",
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

        let local_resolver = PathResolver::new(
            Some("~/.factory/settings.local.json"),
            Some("~/.factory/settings.local.json"),
            Some("%USERPROFILE%\\.factory\\settings.local.json"),
            "~/.factory/settings.local.json",
        );
        let mut local = ConfigSurface::new(
            "settings.local.json",
            local_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        local.precedence = 11;
        local.backup_required = true;
        surfaces.push(local);

        let project_resolver = PathResolver::new(
            Some(".factory/settings.json"),
            Some(".factory/settings.json"),
            Some(".factory\\settings.json"),
            ".factory/settings.json",
        );
        let mut project = ConfigSurface::new(
            ".factory/settings.json",
            project_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project.precedence = 12;
        project.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        project.backup_required = true;
        surfaces.push(project);

        let mcp_resolver = PathResolver::new(
            Some("~/.factory/mcp.json"),
            Some("~/.factory/mcp.json"),
            Some("%USERPROFILE%\\.factory\\mcp.json"),
            "~/.factory/mcp.json",
        );
        let mut mcp = ConfigSurface::new(
            "mcp.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp.precedence = 9;
        mcp.owned_selectors = vec!["mcpServers".to_owned()];
        surfaces.push(mcp);

        let skills_resolver = PathResolver::new(
            Some("~/.factory/skills/<name>/SKILL.md"),
            Some("~/.factory/skills/<name>/SKILL.md"),
            Some("%USERPROFILE%\\.factory\\skills\\<name>\\SKILL.md"),
            "~/.factory/skills/<name>/SKILL.md",
        );
        let mut skills = ConfigSurface::new(
            "skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills.precedence = 8;
        surfaces.push(skills);

        let agents_resolver = PathResolver::new(
            Some("~/.factory/droids/<name>.md"),
            Some("~/.factory/droids/<name>.md"),
            Some("%USERPROFILE%\\.factory\\droids\\<name>.md"),
            "~/.factory/droids/<name>.md",
        );
        let mut agents = ConfigSurface::new(
            "droids",
            agents_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        agents.precedence = 7;
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
            "worktrees/*".to_owned(),
            "specs/*".to_owned(),
            "sessions/*".to_owned(),
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
        if instance.harness != self.id {
            return Err(CoreError::Validation {
                field: "harness".to_owned(),
                reason: format!(
                    "instance harness `{}` does not match adapter `{}`",
                    instance.harness, self.id
                ),
            });
        }
        instance.validate()?;
        let mut plan = WrapperPlan::new("project/HOME via HOME relocation and FACTORY_API_KEY");
        plan.env_vars
            .push(("HOME".to_owned(), instance.config_root.to_string()));
        plan.description = format!(
            " Wrapper sets HOME={} and relies on {API_KEY_ENV_VAR} for auth (hosted policy excluded)",
            instance.config_root
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.factory/settings.json".to_owned(),
            "~/.factory/settings.local.json".to_owned(),
            "~/.factory/mcp.json".to_owned(),
            ".factory/settings.json".to_owned(),
            "$HOME/.factory via HOME relocation".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        if instance.harness != self.id {
            return Err(CoreError::Validation {
                field: "harness".to_owned(),
                reason: format!("expected harness `{}`, got `{}`", self.id, instance.harness),
            });
        }
        instance.validate()?;
        match instance.isolation {
            Isolation::ProjectScope
            | Isolation::ExplicitConfig
            | Isolation::RelocatedRoot
            | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("factory-droid requires isolation project_scope, got {other}"),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        vec![
            crate::adapter::SkillMode::LinkAll,
            crate::adapter::SkillMode::LinkSelected,
            crate::adapter::SkillMode::CopySelected,
        ]
    }

    /// EXT-08/09: `droid mcp add` (own writer) puts top-level `mcpServers`
    /// into `~/.factory/mcp.json` (live-verified droid 0.222.0, 2026-09-18).
    /// The dest nests under `.factory/` because the wrapper relocates HOME to
    /// the instance root and the binary reads `$HOME/.factory/mcp.json` only;
    /// a flat dest would never be read.
    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            ".factory/mcp.json",
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::User,
            RestartBehavior::Reload,
        ))
    }

    /// EXT-06: explicit plugin-mechanism absence (corpus-grounded).
    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some("no plugin mechanism documented (factory-droid.md)")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    use super::{
        DISPLAY_NAME, EXECUTABLE, FactoryDroidAdapter, HARNESS_ID_STR, OWNED_SELECTORS,
        RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> FactoryDroidAdapter {
        FactoryDroidAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-factory-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::ProjectScope,
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
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(a.display_name(), DISPLAY_NAME);
        assert_eq!(a.executable_name(), EXECUTABLE);
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert_eq!(a.adapter_revision(), crate::adapter::ADAPTER_REVISION);
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
        assert_ne!(result.confidence.to_string(), "");
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
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("droid 0.3.0", Some("0.3.0")),
            ("factory 1.0.0", Some("1.0.0")),
            ("v1.2.3", Some("1.2.3")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = crate::adapters::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_layered_json() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let settings = surfaces
            .iter()
            .find(|s| s.id == "settings.json")
            .expect("settings.json");
        assert_eq!(settings.kind, DocumentKind::Json);
        assert_eq!(settings.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(settings.scope, ConfigScope::User);
        for sel in ["customModels", "mcpServers"] {
            assert!(settings.owned_selectors.contains(&sel.to_owned()));
        }
        let mcp = surfaces
            .iter()
            .find(|s| s.id == "mcp.json")
            .expect("mcp.json");
        assert_eq!(mcp.kind, DocumentKind::Json);
        let project = surfaces
            .iter()
            .find(|s| s.id == ".factory/settings.json")
            .expect("project");
        assert_eq!(project.scope, ConfigScope::ProjectWorkspace);
    }

    #[test]
    fn owned_selectors_are_stable() {
        assert!(OWNED_SELECTORS.len() >= 5);
        let set: HashSet<&str> = OWNED_SELECTORS.iter().copied().collect();
        assert_eq!(set.len(), OWNED_SELECTORS.len());
    }

    #[test]
    fn supported_operations_are_constrained() {
        let a = adapter();
        let ops = a.supported_operations();
        for (_, support) in &ops {
            assert_eq!(*support, AdapterSupport::Constrained);
        }
    }

    #[test]
    fn plan_mirror_exclusions_cover_worktrees_and_locks() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(exclusions.contains(&"worktrees/*".to_owned()));
        assert!(exclusions.contains(&"*.lock".to_owned()));
    }

    #[test]
    fn plan_wrapper_sets_home() {
        let tmp_root = crate::test_util::tmp_abs_str(".factory-work");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == "HOME" && v == tmp_root.as_str())
        );
        assert!(plan.description.contains("HOME"));
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let tmp_root = crate::test_util::tmp_abs_str("my factory work");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        let val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == "HOME")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(val, tmp_root.as_str());
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".factory-work"));
        inst.harness = HarnessId::new("codex-cli").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_default_root() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains(".factory")));
    }

    #[test]
    fn validate_instance_accepts_project_scope() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".factory-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".factory-work"));
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
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".factory-work"));
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/factory_droid")
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
        assert!(map.is_empty() || map.contains_key("customModels") || map.len() <= 2);
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(
            map.contains_key("customModels")
                || map.contains_key("model")
                || map.contains_key("mcpServers")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("factory");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("factory.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert("customModels".to_owned(), serde_json::Value::Array(vec![]));
            assert!(map.contains_key("foreignKey") || map.contains_key("unknownTopLevel"));
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert!(after.contains_key("foreignKey") || after.contains_key("unknownTopLevel"));
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
    fn fixture_mcp_minimal_parses() {
        let path = fixture_path("mcp.minimal.json");
        assert!(path.exists(), "mcp minimal missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.is_empty() || map.contains_key("mcpServers") || !map.is_empty());
    }

    /// Live-probe pin (droid 0.222.0): the own writer puts `mcpServers` into
    /// `<root>/.factory/mcp.json` under HOME relocation.
    #[test]
    fn mcp_decl_pins_live_writer_destination() {
        let a = adapter();
        let decl = a.mcp_decl().expect("factory-droid declares an MCP dest");
        assert_eq!(decl.dest_file, ".factory/mcp.json");
        assert_eq!(decl.dest_key, "mcpServers");
        assert!(
            decl.read_only.is_none(),
            "dest is writable: the binary's own writer verified it"
        );
        assert!(a.mcp_absence_reason().is_none());
    }

    /// Wrapper HOME and the MCP dest must agree: droid reads
    /// `$HOME/.factory/mcp.json`, never a flat `$HOME/mcp.json`.
    #[test]
    fn plan_env_and_mcp_dest_agree_under_relocated_home() {
        let tmp_root = crate::test_util::tmp_abs_str(".factory-live");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        let home = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == "HOME")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(home, tmp_root.as_str());

        let decl = a.mcp_decl().unwrap();
        let dest = inst.config_root.join(&decl.dest_file).unwrap();
        assert_eq!(
            dest.as_path(),
            Path::new(home).join(".factory").join("mcp.json"),
            "seeded dest must be exactly what the binary reads under HOME=<root>"
        );
        // The user-scope surface models the same file HOME-relative.
        let mcp = a
            .config_surfaces()
            .into_iter()
            .find(|s| s.id == "mcp.json")
            .expect("mcp surface");
        assert!(
            mcp.path_resolver
                .linux
                .as_deref()
                .is_some_and(|p| p == "~/.factory/mcp.json")
        );
    }

    /// The populated MCP fixture documents the live writer's entry shape.
    #[test]
    fn fixture_mcp_populated_matches_live_writer_shape() {
        let path = fixture_path("mcp.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        let servers = map.get("mcpServers").expect("top-level mcpServers key");
        let entries = servers
            .as_object()
            .expect("mcpServers is a server-name map");
        assert!(!entries.is_empty());
        for (name, entry) in entries {
            let fields = entry
                .as_object()
                .unwrap_or_else(|| panic!("{name} entry not an object"));
            assert!(
                fields.contains_key("command"),
                "{name} stdio entry needs `command` (live writer shape)"
            );
        }
        let report = crate::verification::fixture_report(path.parent().unwrap());
        assert!(report.validity_pass, "factory_droid corpus validity");
        assert!(report.secret_free_pass, "factory_droid corpus secret-free");
    }
}
