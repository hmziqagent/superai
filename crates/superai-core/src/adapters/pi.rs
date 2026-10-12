//! Pi adapter: relocated-root via `PI_CODING_AGENT_DIR`; JSON
//! settings/auth/models surfaces under the config root.

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
pub const HARNESS_ID_STR: &str = "pi";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Pi";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "pi";

/// Environment variable that relocates the config root.
pub const CONFIG_ENV_VAR: &str = "PI_CODING_AGENT_DIR";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/pi.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider/model/mcp mutation inside `settings.json` / `models.json`.
pub const OWNED_SELECTORS: &[&str] = &[
    "providers",
    "models",
    "theme",
    "skills",
    "defaultProjectTrust",
    "thinkingLevel",
    "extensions",
];

/// Concrete adapter for Pi.
#[derive(Debug, Clone)]
pub struct PiAdapter {
    id: HarnessId,
}

impl PiAdapter {
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
        Some(home.join(".pi").join("agent"))
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
                            Ok(text) if (text.contains("providers") || text.contains("models")) => {
                                evidence.push("settings.json contains providers/models".to_owned());
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
                    let auth = root.join("auth.json");
                    if auth.exists() {
                        evidence.push(format!("auth.json present at {}", auth.display()));
                    }
                    let models = root.join("models.json");
                    if models.exists() {
                        evidence.push(format!("models.json present at {}", models.display()));
                    }
                    let trust = root.join("trust.json");
                    if trust.exists() {
                        evidence.push(format!("trust.json present at {}", trust.display()));
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
            evidence.push(format!("{CONFIG_ENV_VAR} not set, using ~/.pi/agent"));
        }
        let project_settings = Path::new(".pi").join("settings.json");
        if project_settings.exists() {
            evidence.push(format!(
                "project settings found at {}",
                project_settings.display()
            ));
        }
        if Path::new(".pi").exists() {
            evidence.push(".pi directory present in cwd".to_owned());
        }
    }
}

impl Default for PiAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for PiAdapter {
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
        super::resolution_from_detection(detection, "pi", SCHEMA_VERSION_STR)
    }

    #[expect(clippy::too_many_lines, reason = "surfaces are declarative")]
    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let settings_resolver = PathResolver::new(
            Some("$PI_CODING_AGENT_DIR/settings.json"),
            Some("$PI_CODING_AGENT_DIR/settings.json"),
            Some("%PI_CODING_AGENT_DIR%\\settings.json"),
            "~/.pi/agent/settings.json",
        );
        let mut settings_surface = ConfigSurface::new(
            "settings.json",
            settings_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        settings_surface.precedence = 10;
        settings_surface.owned_selectors =
            OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        settings_surface.backup_required = true;
        settings_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(settings_surface);

        let auth_resolver = PathResolver::new(
            Some("$PI_CODING_AGENT_DIR/auth.json"),
            Some("$PI_CODING_AGENT_DIR/auth.json"),
            Some("%PI_CODING_AGENT_DIR%\\auth.json"),
            "~/.pi/agent/auth.json",
        );
        let mut auth_surface = ConfigSurface::new(
            "auth.json",
            auth_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        auth_surface.precedence = 12;
        auth_surface.backup_required = false;
        auth_surface.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(auth_surface);

        let models_resolver = PathResolver::new(
            Some("$PI_CODING_AGENT_DIR/models.json"),
            Some("$PI_CODING_AGENT_DIR/models.json"),
            Some("%PI_CODING_AGENT_DIR%\\models.json"),
            "~/.pi/agent/models.json",
        );
        let mut models_surface = ConfigSurface::new(
            "models.json",
            models_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        models_surface.precedence = 11;
        models_surface.owned_selectors = vec!["providers".to_owned(), "models".to_owned()];
        models_surface.backup_required = true;
        surfaces.push(models_surface);

        let project_settings_resolver = PathResolver::fallback_only(".pi/settings.json (project)");
        let mut project_settings = ConfigSurface::new(
            "project.settings.json",
            project_settings_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_settings.precedence = 15;
        project_settings.backup_required = false;
        surfaces.push(project_settings);

        let skills_resolver = PathResolver::new(
            Some("$PI_CODING_AGENT_DIR/skills/<name>/SKILL.md"),
            Some("$PI_CODING_AGENT_DIR/skills/<name>/SKILL.md"),
            Some("%PI_CODING_AGENT_DIR%\\skills\\<name>\\SKILL.md"),
            "~/.pi/agent/skills/<name>/SKILL.md",
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

        let extensions_resolver = PathResolver::new(
            Some("$PI_CODING_AGENT_DIR/extensions/<name>"),
            Some("$PI_CODING_AGENT_DIR/extensions/<name>"),
            Some("%PI_CODING_AGENT_DIR%\\extensions\\<name>"),
            "~/.pi/agent/extensions/<name>",
        );
        let mut extensions = ConfigSurface::new(
            "extensions",
            extensions_resolver,
            DocumentKind::Opaque,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        extensions.precedence = 6;
        extensions.backup_required = false;
        surfaces.push(extensions);

        let trust_resolver = PathResolver::new(
            Some("$PI_CODING_AGENT_DIR/trust.json"),
            Some("$PI_CODING_AGENT_DIR/trust.json"),
            Some("%PI_CODING_AGENT_DIR%\\trust.json"),
            "~/.pi/agent/trust.json",
        );
        let mut trust = ConfigSurface::new(
            "trust.json",
            trust_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        trust.precedence = 5;
        trust.backup_required = false;
        surfaces.push(trust);

        let sessions_resolver = PathResolver::new(
            Some("$PI_CODING_AGENT_DIR/sessions"),
            Some("$PI_CODING_AGENT_DIR/sessions"),
            Some("%PI_CODING_AGENT_DIR%\\sessions"),
            "~/.pi/agent/sessions",
        );
        let mut sessions = ConfigSurface::new(
            "sessions",
            sessions_resolver,
            DocumentKind::Opaque,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        sessions.precedence = 0;
        sessions.backup_required = false;
        surfaces.push(sessions);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        super::all_operations_full()
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "sessions/*".to_owned(),
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "tmp/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "state/*".to_owned(),
            "telemetry/*".to_owned(),
            "*.lock".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("relocated-root via PI_CODING_AGENT_DIR");
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
            "~/.pi/agent".to_owned(),
            "~/.pi".to_owned(),
            "$PI_CODING_AGENT_DIR".to_owned(),
            ".pi/settings.json".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("pi requires isolation relocated_root, got {other}"),
            }),
        }
    }

    fn capability_declarations(&self) -> Vec<crate::adapter::AdapterCapabilityDecl> {
        vec![
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::WebSearch,
                crate::capability::Support::Native,
                "built-in web search tool",
            ),
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::Vision,
                crate::capability::Support::Native,
                "image input on the chat transport",
            ),
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::ComputerUse,
                crate::capability::Support::Absent,
                "no computer-use loop in the pi harness",
            ),
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::Mcp,
                crate::capability::Support::Absent,
                "no native MCP support; a verified extension may substitute",
            ),
        ]
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        super::skill_modes_link_first()
    }

    fn mcp_absence_reason(&self) -> Option<&'static str> {
        Some(
            "MCP is intentionally not built in; integrations come from extensions/packages (pi.md: intentionally does not include built-in MCP)",
        )
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "extensions are TypeScript modules loaded via -e/--extension flags/npm/git; no file-staged plugin mechanism (pi.md)",
        )
    }
    fn usage_surface(&self) -> Option<UsageSurface> {
        Some(
            UsageSurface::new(UsageReaderKind::TranscriptJsonl, None).with_artifact(
                "sessions",
                PathResolver::new(
                    Some("$PI_CODING_AGENT_DIR/sessions"),
                    Some("$PI_CODING_AGENT_DIR/sessions"),
                    Some("%PI_CODING_AGENT_DIR%\\sessions"),
                    "~/.pi/agent/sessions (PI_CODING_AGENT_SESSION_DIR overrides)",
                ),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{CONFIG_ENV_VAR, HARNESS_ID_STR, OWNED_SELECTORS, PiAdapter};
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> PiAdapter {
        PiAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-pi-1").unwrap(),
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
    fn config_surfaces_include_writable_settings_and_models() {
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
        for selector in ["providers", "models"] {
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

        let auth = surfaces
            .iter()
            .find(|s| s.id == "auth.json")
            .expect("auth.json");
        assert_eq!(auth.kind, DocumentKind::Json);
        assert_eq!(auth.ownership, SurfaceOwnership::ExternalSecretStore);

        let models = surfaces
            .iter()
            .find(|s| s.id == "models.json")
            .expect("models.json");
        assert_eq!(models.kind, DocumentKind::Json);
        assert_eq!(models.ownership, SurfaceOwnership::UserEditable);
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
        let must_contain = ["sessions/*", "logs/*", "cache/*", "*.lock"];
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
        assert!(!is_excluded("models.json"));
        assert!(is_excluded("sessions/abc.jsonl"));
        assert!(is_excluded("cache/data"));
        assert!(is_excluded("logs/pi.log"));
    }

    #[test]
    fn plan_wrapper_sets_pi_coding_agent_dir() {
        let tmp_root = crate::test_util::tmp_abs_str(".pi-work");
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
        let tmp_root = crate::test_util::tmp_abs_str("my pi work");
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
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".pi-work"));
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
        assert!(candidates.iter().any(|c| c.contains(".pi")));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".pi-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".pi-work"));
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
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".pi-work"));
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let settings = surfaces.iter().find(|s| s.id == "settings.json").unwrap();
        assert_eq!(settings.path_resolver.fallback, "~/.pi/agent/settings.json");
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
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/pi")
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
        assert!(map.is_empty() || map.len() <= 2);
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(
            map.contains_key("theme")
                || map.contains_key("providers")
                || map.contains_key("defaultProjectTrust")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("pi");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("settings.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "theme".to_owned(),
                serde_json::Value::String("dark".to_owned()),
            );
            assert!(map.contains_key("foreignKey") || map.contains_key("unknownTopLevel"));
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert_eq!(after["theme"], serde_json::Value::String("dark".to_owned()));
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
        let tmp_root = crate::test_util::tmp_abs_str("user/.pi-isolated");
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
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".pi-work"));
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
