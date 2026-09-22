//! Kilo Code adapter: layered JSONC (global `~/.config/kilo/kilo.jsonc`, then
//! project `kilo.jsonc`); isolation via HOME/`XDG_CONFIG_HOME` plus `--user-data-dir`.

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

/// Harness identifier for Kilo Code.
pub const HARNESS_ID_STR: &str = "kilo-code";

/// Human display name.
pub const DISPLAY_NAME: &str = "Kilo Code extension and CLI";

/// Primary executable (CLI).
pub const EXECUTABLE: &str = "kilo";

/// Alternative executable name.
pub const EXECUTABLE_ALT: &str = "kilo-code";

/// Env var for inline config override (highest precedence).
pub const INLINE_CONFIG_ENV_VAR: &str = "KILO_CONFIG_CONTENT";

/// XDG config home for global isolation.
pub const XDG_CONFIG_ENV_VAR: &str = "XDG_CONFIG_HOME";

/// Flag for IDE isolation.
pub const USER_DATA_DIR_FLAG: &str = "--user-data-dir";

/// Extensions dir flag.
pub const EXTENSIONS_DIR_FLAG: &str = "--extensions-dir";

/// Default config root fallback.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.config/kilo";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/kilo-code.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside `kilo.jsonc` (JSONC).
pub const OWNED_SELECTORS: &[&str] = &[
    "model",
    "provider",
    "mcp",
    "permission",
    "agent",
    "disabled_providers",
    "enabled_providers",
    "experimental",
];

/// MCP selectors.
pub const MCP_OWNED_SELECTORS: &[&str] = &["mcp"];

/// Concrete adapter for Kilo Code.
#[derive(Debug, Clone)]
pub struct KiloAdapter {
    id: HarnessId,
}

impl KiloAdapter {
    /// Create a new adapter.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    /// Borrow harness id.
    pub fn harness_id(&self) -> &HarnessId {
        &self.id
    }

    /// Executable name.
    pub fn executable_name(&self) -> &str {
        EXECUTABLE
    }

    /// Inline env var.
    pub fn inline_env_var(&self) -> &str {
        INLINE_CONFIG_ENV_VAR
    }

    #[expect(clippy::unused_self, reason = "adapter uses instance constants")]
    fn find_binary_in_path(&self) -> Option<PathBuf> {
        if let Some(kilo) =
            super::find_in_path(&[EXECUTABLE]).or_else(|| super::find_in_path(&[EXECUTABLE_ALT]))
        {
            return Some(kilo);
        }
        // `code` is only fallback evidence; there is no .exe spelling, so
        // this misses the Windows binary.
        let path_var = std::env::var("PATH").ok()?;
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(sep) {
            if dir.is_empty() {
                continue;
            }
            let code = Path::new(dir).join("code");
            if code.is_file() {
                return Some(code);
            }
        }
        None
    }

    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(XDG_CONFIG_ENV_VAR)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir).join("kilo"));
        }
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".config").join("kilo"))
    }

    #[expect(clippy::excessive_nesting, reason = "evidence explicit")]
    #[expect(clippy::unused_self, reason = "adapter method")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let global = root.join("kilo.jsonc");
                    let global_json = root.join("kilo.json");
                    if global.exists() {
                        evidence.push(format!("kilo.jsonc found at {}", global.display()));
                        if let Ok(text) = std::fs::read_to_string(&global)
                            && text.contains("mcp")
                        {
                            evidence.push("kilo.jsonc contains mcp".to_owned());
                        }
                    } else if global_json.exists() {
                        evidence.push(format!("kilo.json found at {}", global_json.display()));
                    } else {
                        evidence.push(format!("kilo.jsonc missing at {}", global.display()));
                    }
                    let legacy_global = PathBuf::from(std::env::var("HOME").unwrap_or_default())
                        .join(".config")
                        .join("Code")
                        .join("User")
                        .join("globalStorage")
                        .join("kilocode.kilo-code")
                        .join("settings");
                    if legacy_global.exists() {
                        evidence.push(format!(
                            "legacy globalStorage present at {}",
                            legacy_global.display()
                        ));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => evidence.push("could not resolve config root (no HOME)".to_owned()),
        }
        for p in [
            Path::new("./kilo.jsonc"),
            Path::new("./kilo.json"),
            Path::new("./.kilo/kilo.jsonc"),
            Path::new("./.kilo/kilo.json"),
        ] {
            if p.exists() {
                evidence.push(format!("project config found at {}", p.display()));
            }
        }
        if Path::new(".kilocoderules").exists() || Path::new(".clinerules").exists() {
            evidence.push("rules file .kilocoderules/.clinerules present".to_owned());
        }
        if Path::new(".kiloignore").exists() {
            evidence.push(".kiloignore present".to_owned());
        }
        if let Ok(val) = std::env::var(INLINE_CONFIG_ENV_VAR)
            && !val.trim().is_empty()
        {
            evidence.push(format!("{INLINE_CONFIG_ENV_VAR} is set"));
        }
        if let Ok(val) = std::env::var("HOME")
            && !val.trim().is_empty()
        {
            evidence.push(format!("HOME is set to {val}"));
        }
    }
}

impl Default for KiloAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "kilo-code is static valid")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for KiloAdapter {
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

    #[expect(clippy::excessive_nesting, reason = "detection branches explicit")]
    fn detection(&self) -> DetectionResult {
        let mut evidence = Vec::new();
        let mut version: Option<String> = None;
        let mut binary_path: Option<PathBuf> = None;
        match self.find_binary_in_path() {
            Some(path) => {
                evidence.push(format!(
                    "found binary `{}` at {}",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("kilo"),
                    path.display()
                ));
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_owned();
                if file_name.contains("kilo") {
                    match super::probe_version(&path) {
                        Some(v) => {
                            evidence.push(format!("version `{v}` via `--version`"));
                            version = Some(v);
                        }
                        None => evidence
                            .push("version probe failed for `kilo --version` (timeout)".to_owned()),
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
        // Absent forces High, so the Low "config root exists" arm can never
        // survive.
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
            notes.push(format!("detected kilo version {v}"));
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

        let global_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/kilo/kilo.jsonc"),
            Some("$XDG_CONFIG_HOME/kilo/kilo.jsonc"),
            Some("%XDG_CONFIG_HOME%\\kilo\\kilo.jsonc"),
            "~/.config/kilo/kilo.jsonc",
        );
        let mut global = ConfigSurface::new(
            "global kilo.jsonc",
            global_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        global.precedence = 10;
        global.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        global.backup_required = true;
        global.restart_behavior = RestartBehavior::Reload;
        surfaces.push(global);

        let project_resolver = PathResolver::fallback_only("./kilo.jsonc / ./.kilo/kilo.jsonc");
        let mut project = ConfigSurface::new(
            "project kilo.jsonc",
            project_resolver,
            DocumentKind::Jsonc,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project.precedence = 20;
        project.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        project.backup_required = true;
        surfaces.push(project);

        let tui_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/kilo/tui.jsonc"),
            Some("$XDG_CONFIG_HOME/kilo/tui.jsonc"),
            Some("%XDG_CONFIG_HOME%\\kilo\\tui.jsonc"),
            "~/.config/kilo/tui.jsonc",
        );
        let mut tui = ConfigSurface::new(
            "tui.jsonc",
            tui_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        tui.precedence = 8;
        tui.backup_required = false;
        surfaces.push(tui);

        let agents_resolver =
            PathResolver::fallback_only(".kilo/agents/*.md / ~/.config/kilo/agent/*.md");
        let mut agents = ConfigSurface::new(
            "agents",
            agents_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        agents.precedence = 15;
        agents.backup_required = false;
        surfaces.push(agents);

        let mcp_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/kilo/kilo.jsonc (mcp key)"),
            Some("$XDG_CONFIG_HOME/kilo/kilo.jsonc (mcp key)"),
            Some("%XDG_CONFIG_HOME%\\kilo\\kilo.jsonc (mcp key)"),
            "~/.config/kilo/kilo.jsonc (mcp key)",
        );
        let mut mcp = ConfigSurface::new(
            "mcp in kilo.jsonc",
            mcp_resolver,
            DocumentKind::Jsonc,
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

        let rules_resolver =
            PathResolver::fallback_only(".kilocoderules / .clinerules / AGENTS.md");
        let mut rules = ConfigSurface::new(
            ".kilocoderules",
            rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        rules.precedence = 18;
        rules.backup_required = false;
        surfaces.push(rules);

        let ignore_resolver = PathResolver::fallback_only(".kiloignore");
        let mut ignore = ConfigSurface::new(
            ".kiloignore",
            ignore_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        ignore.precedence = 5;
        ignore.backup_required = false;
        surfaces.push(ignore);

        let vscode_settings_resolver = PathResolver::new(
            Some("~/.config/Code/User/globalStorage/kilocode.kilo-code/settings"),
            Some(
                "~/Library/Application Support/Code/User/globalStorage/kilocode.kilo-code/settings",
            ),
            Some("%APPDATA%\\Code\\User\\globalStorage\\kilocode.kilo-code\\settings"),
            "~/.config/Code/User/globalStorage/kilocode.kilo-code/settings",
        );
        let mut vscode = ConfigSurface::new(
            "vscode globalStorage",
            vscode_settings_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        vscode.precedence = 6;
        vscode.backup_required = false;
        surfaces.push(vscode);

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
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "tmp/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
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
        let mut plan = WrapperPlan::new(
            "ide-user-data via HOME/XDG_CONFIG_HOME + --user-data-dir (inline until verified)",
        );
        plan.env_vars
            .push(("HOME".to_owned(), instance.config_root.to_string()));
        plan.env_vars.push((
            XDG_CONFIG_ENV_VAR.to_owned(),
            Path::new(&instance.config_root.to_string())
                .join("config")
                .display()
                .to_string(),
        ));
        plan.env_vars.push((
            INLINE_CONFIG_ENV_VAR.to_owned(),
            "{{\"remote_control\": true}}".to_owned(),
        ));
        let user_data = Path::new(&instance.config_root.to_string()).join("vscode-data");
        let extensions = Path::new(&instance.config_root.to_string()).join("extensions");
        plan.args.push(USER_DATA_DIR_FLAG.to_owned());
        plan.args.push(user_data.display().to_string());
        plan.args.push(EXTENSIONS_DIR_FLAG.to_owned());
        plan.args.push(extensions.display().to_string());
        plan.description = format!(
            " Wrapper sets HOME={} {}={}/config {}={} and execs `code {} {}` with extensions {} (inline until verified)",
            instance.config_root,
            XDG_CONFIG_ENV_VAR,
            instance.config_root,
            INLINE_CONFIG_ENV_VAR,
            "{{remote_control}}",
            USER_DATA_DIR_FLAG,
            user_data.display(),
            extensions.display()
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/kilo/kilo.jsonc".to_owned(),
            "~/.config/kilo/kilo.json".to_owned(),
            "./kilo.jsonc".to_owned(),
            "./.kilo/kilo.jsonc".to_owned(),
            "$XDG_CONFIG_HOME/kilo/kilo.jsonc".to_owned(),
            "$KILO_CONFIG_CONTENT".to_owned(),
            "--user-data-dir".to_owned(),
            ".kilocoderules".to_owned(),
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
            Isolation::IdeUserData
            | Isolation::RelocatedRoot
            | Isolation::ExplicitConfig
            | Isolation::ProjectScope
            | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("kilo-code requires isolation ide_user_data, got {other}"),
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

    /// Top-level `mcp` key in kilo.jsonc, managed via the `kilo mcp` CLI; commands are argv arrays.
    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            "global kilo.jsonc",
            "mcp",
            DocumentKind::Jsonc,
            ConfigScope::User,
            RestartBehavior::Reload,
        ).with_read_only("jsonc writes refuse (LossyWrite) until a preserving codec exists; inspect/diff only"))
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "modes/marketplaces managed through the Settings webview; no file-staged plugin mechanism documented (kilo-code.md)",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, KiloAdapter, RESEARCH_DOC, USER_DATA_DIR_FLAG,
    };
    use crate::adapter::{Adapter, DocumentKind, ProductStatus};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};
    use std::collections::HashSet;

    fn adapter() -> KiloAdapter {
        KiloAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-kilo-1").unwrap(),
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
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(a.display_name(), DISPLAY_NAME);
        assert_eq!(a.executable_name(), EXECUTABLE);
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
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
            crate::adapters::parse_version_output("kilo 1.2.3").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(crate::adapters::parse_version_output(""), None);
    }

    #[test]
    fn surfaces_include_jsonc() {
        let a = adapter();
        let s = a.config_surfaces();
        assert!(s.iter().any(|x| x.id == "global kilo.jsonc"));
        assert!(s.iter().any(|x| x.kind == DocumentKind::Jsonc));
        assert!(s.iter().any(|x| x.id == ".kilocoderules"));
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
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kilo-work"));
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(plan.args.contains(&USER_DATA_DIR_FLAG.to_owned()));
        assert!(plan.env_vars.iter().any(|(k, _)| k == "HOME"));
        assert!(plan.env_vars.iter().any(|(k, _)| k == "XDG_CONFIG_HOME"));
        assert!(!plan.description.is_empty());
    }

    #[test]
    fn wrapper_rejects_wrong_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kilo-work"));
        inst.harness = HarnessId::new("cursor").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scan_candidates_cover_kilo() {
        let a = adapter();
        let c = a.scan_candidates();
        assert!(c.iter().any(|s| s.contains("kilo.jsonc")));
        assert!(c.iter().any(|s| s.contains(USER_DATA_DIR_FLAG)));
    }

    #[test]
    fn validate_accepts_ide_user_data() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kilo-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_rejects_env_only() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kilo-work"));
        inst.isolation = Isolation::EnvOnly;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn skill_modes_supported() {
        let a = adapter();
        let modes = a.supported_skill_modes();
        assert_eq!(modes.len(), 3);
        let set: HashSet<String> = modes.iter().map(ToString::to_string).collect();
        assert!(set.contains("link_all"));
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
            .join("fixtures/kilo")
            .join("kilo.populated.jsonc");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let value = superai_config::jsonc::load_value(&path).unwrap();
        assert!(
            value.get("model").is_some() || value.get("provider").is_some(),
            "fixture must document model/provider"
        );
        let report = crate::verification::fixture_report(path.parent().unwrap());
        assert!(report.validity_pass, "kilo corpus validity");
        assert!(report.secret_free_pass, "kilo corpus secret-free");
    }
}
