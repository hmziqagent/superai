//! Antigravity CLI adapter: `HOME` workaround, `ResearchBlocked`.
//! Research source: `docs/harness-configs/antigravity-cli.md` (last verified 2026-08-25).

use std::path::PathBuf;

use crate::adapter::{
    ADAPTER_REVISION, Adapter, ConfigScope, ConfigSurface, DetectionResult, DocumentKind,
    PathResolver, Platform, ProductStatus, RestartBehavior, SurfaceOwnership, UsageReaderKind,
    UsageSurface, VersionResolution, WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::AdapterSupport;

/// Catalog id superai registers this harness under.
pub const HARNESS_ID_STR: &str = "antigravity-cli";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Antigravity CLI";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "agy";

/// Alternative binary name (installer).
pub const EXECUTABLE_ALT: &str = "antigravity";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/antigravity-cli.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// HOME workaround note.
pub const HOME_WORKAROUND_NOTE: &str = "no documented AGY_HOME/CONFIG_DIR; HOME relocation is undocumented workaround: verify before relying, especially on macOS keychain";

/// Detect only; writes blocked pending settings schema, per-OS keyring, and HOME-concurrency verification.
#[derive(Debug, Clone)]
pub struct AntigravityAdapter {
    id: HarnessId,
}

impl AntigravityAdapter {
    /// Create a new adapter.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    fn default_home() -> Option<PathBuf> {
        super::home_dir()
    }

    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("research blocked: {HOME_WORKAROUND_NOTE}"));
        evidence.push("gaps: settings schema, auth/keyring per OS, plugin/skill/MCP paths, sandbox, HOME concurrency".to_owned());
        evidence
            .push("successor of gemini-cli; migration via `agy plugin import gemini`".to_owned());
        match Self::default_home() {
            Some(home) => {
                let root = home.join(".gemini").join("antigravity-cli");
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let settings = root.join("settings.json");
                    if settings.exists() {
                        evidence.push(format!("settings.json found at {}", settings.display()));
                    } else {
                        evidence.push(format!("settings.json missing at {}", settings.display()));
                    }
                    let skills = root.join("skills");
                    if skills.exists() {
                        evidence.push(format!("skills dir present at {}", skills.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
                let global_mcp = home.join(".gemini").join("config").join("mcp_config.json");
                if global_mcp.exists() {
                    evidence.push(format!(
                        "global mcp_config.json found at {}",
                        global_mcp.display()
                    ));
                }
            }
            None => {
                evidence.push("could not resolve home for antigravity lookup".to_owned());
            }
        }
    }
}

impl Default for AntigravityAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for AntigravityAdapter {
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

        match super::find_in_path_dir_first(&[EXECUTABLE, EXECUTABLE_ALT]) {
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
                        evidence.push(format!("version probe failed for `{EXECUTABLE} --version`"));
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

        let confidence = super::detection_confidence(
            binary_path.is_some(),
            version.is_some(),
            evidence.iter().any(|e| e.contains("config root exists")),
        );

        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution_from(&self, detection: &DetectionResult) -> VersionResolution {
        if let Some(v) = detection.version.clone() {
            let mut notes = Vec::new();
            notes.push(format!("detected antigravity-cli version {v}"));
            notes.push(format!("research blocked: {HOME_WORKAROUND_NOTE}"));
            let mut res = VersionResolution::new(Some(v), None, false);
            res.notes = notes;
            res
        } else {
            let mut res = VersionResolution::unknown();
            res.notes.clone_from(&detection.evidence);
            res.notes
                .push(format!("research blocked: {HOME_WORKAROUND_NOTE}"));
            res
        }
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let settings_resolver = PathResolver::new(
            Some("~/.gemini/antigravity-cli/settings.json"),
            Some("~/.gemini/antigravity-cli/settings.json"),
            Some("%USERPROFILE%\\.gemini\\antigravity-cli\\settings.json"),
            "~/.gemini/antigravity-cli/settings.json",
        );
        let mut settings_surface = ConfigSurface::new(
            "settings.json",
            settings_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        settings_surface.precedence = 10;
        settings_surface.owned_selectors = Vec::new();
        settings_surface.backup_required = true;
        settings_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(settings_surface);

        let mcp_resolver = PathResolver::new(
            Some("~/.gemini/config/mcp_config.json"),
            Some("~/.gemini/config/mcp_config.json"),
            Some("%USERPROFILE%\\.gemini\\config\\mcp_config.json"),
            "~/.gemini/config/mcp_config.json",
        );
        let mut mcp_surface = ConfigSurface::new(
            "mcp_config.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp_surface.precedence = 12;
        mcp_surface.owned_selectors = vec!["mcpServers".to_owned()];
        mcp_surface.backup_required = true;
        surfaces.push(mcp_surface);

        let skills_resolver = PathResolver::new(
            Some("~/.gemini/antigravity-cli/skills/<name>.md"),
            Some("~/.gemini/antigravity-cli/skills/<name>.md"),
            Some("%USERPROFILE%\\.gemini\\antigravity-cli\\skills\\<name>.md"),
            "~/.gemini/antigravity-cli/skills/<name>.md",
        );
        let mut skills_surface = ConfigSurface::new(
            "skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills_surface.precedence = 8;
        skills_surface.backup_required = false;
        surfaces.push(skills_surface);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        vec![
            ("detect".to_owned(), AdapterSupport::ResearchBlocked),
            ("read_config".to_owned(), AdapterSupport::ResearchBlocked),
            ("write_config".to_owned(), AdapterSupport::ResearchBlocked),
            ("manage_skills".to_owned(), AdapterSupport::ResearchBlocked),
            ("manage_mcp".to_owned(), AdapterSupport::ResearchBlocked),
            ("manage_plugins".to_owned(), AdapterSupport::ResearchBlocked),
            (
                "configure_provider".to_owned(),
                AdapterSupport::ResearchBlocked,
            ),
            ("plan_mirror".to_owned(), AdapterSupport::ResearchBlocked),
            ("plan_wrapper".to_owned(), AdapterSupport::ResearchBlocked),
            (
                "scan_candidates".to_owned(),
                AdapterSupport::ResearchBlocked,
            ),
            (
                "validate_instance".to_owned(),
                AdapterSupport::ResearchBlocked,
            ),
        ]
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "history/*".to_owned(),
            "sessions/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        Err(CoreError::ResearchBlocked {
            harness: self.id.to_string(),
            surface: "wrapper".to_owned(),
            reason: format!(
                "ResearchBlocked: {HOME_WORKAROUND_NOTE}: detect only; HOME workaround not verified for concurrent wrappers"
            ),
        })
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.gemini/antigravity-cli/settings.json".to_owned(),
            "~/.gemini/antigravity-cli/skills".to_owned(),
            "~/.gemini/config/mcp_config.json".to_owned(),
            ".agents/skills (project)".to_owned(),
            ".agents/mcp_config.json (project)".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        Err(CoreError::ResearchBlocked {
            harness: self.id.to_string(),
            surface: "validate_instance".to_owned(),
            reason: format!(
                "ResearchBlocked: {HOME_WORKAROUND_NOTE}: validate blocked until HOME isolation verified"
            ),
        })
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        Vec::new()
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            "mcp_config.json",
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::User,
            RestartBehavior::Reload,
        ).with_read_only("research-blocked harness; remote entries use the serverUrl schema (antigravity-cli.md 5); inspect/diff only"))
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "plugin bundles documented (`plugin.json` under ~/.gemini/antigravity-cli/plugins/) but the harness is ResearchBlocked; revisit when research completes (antigravity-cli.md 5)",
        )
    }
    fn usage_surface(&self) -> Option<UsageSurface> {
        Some(
            UsageSurface::new(UsageReaderKind::HistorySqlite, Some("google"))
                .with_artifact(
                    "cli-conversations-db",
                    PathResolver::new(
                        Some("$GEMINI_CLI_HOME/antigravity-cli/conversations/*.db"),
                        Some("$GEMINI_CLI_HOME/antigravity-cli/conversations/*.db"),
                        Some("%GEMINI_CLI_HOME%\\antigravity-cli\\conversations\\*.db"),
                        "~/.gemini/antigravity-cli/conversations/*.db",
                    ),
                )
                .with_artifact(
                    "ide-db",
                    PathResolver::new(
                        Some("$GEMINI_CLI_HOME/antigravity/*.db"),
                        Some("$GEMINI_CLI_HOME/antigravity/*.db"),
                        Some("%GEMINI_CLI_HOME%\\antigravity\\*.db"),
                        "~/.gemini/antigravity/*.db",
                    ),
                )
                .with_artifact(
                    "ide-conversations-db",
                    PathResolver::new(
                        Some("$GEMINI_CLI_HOME/antigravity/conversations/*.db"),
                        Some("$GEMINI_CLI_HOME/antigravity/conversations/*.db"),
                        Some("%GEMINI_CLI_HOME%\\antigravity\\conversations\\*.db"),
                        "~/.gemini/antigravity/conversations/*.db",
                    ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{AntigravityAdapter, HARNESS_ID_STR};
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> AntigravityAdapter {
        AntigravityAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-antigravity-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::OsBound,
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
    fn detection_returns_evidence_with_blocked_reason() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.contains("research blocked"))
        );
        assert!(result.evidence.iter().any(|e| e.contains("HOME")));
    }

    #[test]
    fn version_resolution_is_not_compatible() {
        let a = adapter();
        let res = a.version_resolution();
        assert!(!res.compatible);
        assert!(res.schema_version.is_none());
        assert!(!res.notes.is_empty());
        assert!(
            res.notes
                .iter()
                .any(|n| n.contains("research blocked") || n.contains("HOME"))
        );
    }

    #[test]
    fn config_surfaces_exist() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(!surfaces.is_empty());
        let settings = surfaces
            .iter()
            .find(|s| s.id == "settings.json")
            .expect("settings.json must exist");
        assert_eq!(settings.kind, DocumentKind::Json);
        assert_eq!(settings.ownership, SurfaceOwnership::HarnessManaged);
        assert_eq!(settings.scope, ConfigScope::User);
    }

    #[test]
    fn supported_operations_are_research_blocked() {
        let a = adapter();
        let ops = a.supported_operations();
        for (_, support) in ops {
            assert_eq!(support, AdapterSupport::ResearchBlocked);
        }
    }

    #[test]
    fn plan_wrapper_is_research_blocked() {
        let a = adapter();
        let inst =
            sample_instance_with_root(&crate::test_util::tmp_abs_str(".gemini-antigravity-work"));
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::ResearchBlocked { reason, .. } => {
                assert!(reason.contains("HOME") || reason.contains("ResearchBlocked"));
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn validate_instance_is_research_blocked() {
        let a = adapter();
        let inst =
            sample_instance_with_root(&crate::test_util::tmp_abs_str(".gemini-antigravity-work"));
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::ResearchBlocked { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_antigravity_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("antigravity-cli")));
        assert!(candidates.iter().any(|c| c.contains("mcp_config")));
    }

    #[test]
    fn supported_skill_modes_is_empty() {
        let a = adapter();
        assert!(a.supported_skill_modes().is_empty());
    }
}
