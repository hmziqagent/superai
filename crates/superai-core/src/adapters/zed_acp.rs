//! Zed ACP adapter — JSON settings with ACP wrapper registrations and MCP.
//!
//! Research source: `docs/harness-configs/zed-acp.md` (last verified 2026-08-25).
//! Executable `zed` (editor) hosting external agents via Agent Client Protocol,
//! config `~/.config/zed/settings.json` (JSON), keys `agent_servers.*` with
//! `command`/`args`/`env` for ACP wrappers, `context_servers` / `language_models`
//! for MCP and models, isolation `ide-user-data` via `--user-data-dir` (wrapped),
//! constrained with wrapper registrations.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::adapter::{
    ADAPTER_REVISION, Adapter, Arch, ConfigScope, ConfigSurface, DetectionConfidence,
    DetectionResult, DocumentKind, Os, PathResolver, Platform, ProductStatus, RestartBehavior,
    SurfaceOwnership, VersionResolution, WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, InstallPresence, Isolation};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Harness identifier for Zed ACP.
pub const HARNESS_ID_STR: &str = "zed-acp";

/// Human display name.
pub const DISPLAY_NAME: &str = "Zed AI/ACP";

/// Primary executable.
pub const EXECUTABLE: &str = "zed";

/// Alternative binary name.
pub const EXECUTABLE_ALT: &str = "zeditor";

/// Flag for IDE isolation (wrapped Zed launch).
pub const USER_DATA_DIR_FLAG: &str = "--user-data-dir";

/// Extensions dir flag (Zed extensions isolation).
pub const EXTENSIONS_DIR_FLAG: &str = "--extensions-dir";

/// Default settings fallback.
pub const DEFAULT_SETTINGS_FALLBACK: &str = "~/.config/zed/settings.json";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/zed-acp.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside `settings.json` for Zed AI.
pub const OWNED_SELECTORS: &[&str] = &[
    "agent_servers",
    "context_servers",
    "language_models",
    "agent",
    "agent.profiles",
];

/// MCP owned selectors.
pub const MCP_OWNED_SELECTORS: &[&str] = &["context_servers", "agent_servers"];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Zed ACP.
#[derive(Debug, Clone)]
pub struct ZedAcpAdapter {
    id: HarnessId,
}

impl ZedAcpAdapter {
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

    #[expect(clippy::unused_self, reason = "adapter uses instance constants")]
    #[expect(clippy::excessive_nesting, reason = "PATH scan branches are explicit")]
    fn find_binary_in_path(&self) -> Option<PathBuf> {
        let path_var = std::env::var("PATH").ok()?;
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(sep) {
            if dir.is_empty() {
                continue;
            }
            let candidate = Path::new(dir).join(EXECUTABLE);
            if candidate.is_file() {
                return Some(candidate);
            }
            if cfg!(windows) {
                let exe_candidate = Path::new(dir).join(format!("{EXECUTABLE}.exe"));
                if exe_candidate.is_file() {
                    return Some(exe_candidate);
                }
            }
        }
        for dir in path_var.split(sep) {
            if dir.is_empty() {
                continue;
            }
            let candidate = Path::new(dir).join(EXECUTABLE_ALT);
            if candidate.is_file() {
                return Some(candidate);
            }
            if cfg!(windows) {
                let exe_candidate = Path::new(dir).join(format!("{EXECUTABLE_ALT}.exe"));
                if exe_candidate.is_file() {
                    return Some(exe_candidate);
                }
            }
        }
        None
    }

    fn probe_version(binary: &Path) -> Option<String> {
        let owned = binary.to_path_buf();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let output = Command::new(&owned)
                .arg("--version")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();
            drop(tx.send(output));
        });
        let Ok(Ok(output)) = rx.recv_timeout(Duration::from_secs(2)) else {
            return None;
        };
        if !output.status.success() && output.stdout.is_empty() && output.stderr.is_empty() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = if stdout.trim().is_empty() {
            stderr.into_owned()
        } else if stderr.trim().is_empty() {
            stdout.into_owned()
        } else {
            format!("{stdout} {stderr}")
        };
        Self::parse_version_output(&combined)
    }

    #[expect(clippy::excessive_nesting, reason = "version parsing explicit")]
    fn parse_version_output(output: &str) -> Option<String> {
        let trimmed = output.trim();
        if trimmed.is_empty() {
            return None;
        }
        for token in trimmed.split_whitespace() {
            let mut candidate = token;
            if let Some(stripped) = candidate.strip_prefix('v') {
                candidate = stripped;
            } else if let Some(stripped) = candidate.strip_prefix('V') {
                candidate = stripped;
            }
            let cleaned = candidate.trim_matches(|c: char| c == ',' || c == ')' || c == '(');
            if cleaned.is_empty() {
                continue;
            }
            let has_dot = cleaned.contains('.');
            let starts_digit = cleaned.chars().next().is_some_and(|c| c.is_ascii_digit());
            if has_dot && starts_digit {
                let is_version_like = cleaned
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+');
                if is_version_like {
                    return Some(cleaned.to_owned());
                }
                let mut version_part = String::new();
                for ch in cleaned.chars() {
                    if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '+' {
                        version_part.push(ch);
                    } else {
                        break;
                    }
                }
                if version_part.contains('.') && !version_part.is_empty() {
                    return Some(version_part);
                }
            }
        }
        None
    }

    fn default_settings_path() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        if cfg!(windows) {
            if let Ok(appdata) = std::env::var("APPDATA")
                && !appdata.trim().is_empty()
            {
                return Some(PathBuf::from(appdata).join("Zed").join("settings.json"));
            }
            Some(
                PathBuf::from(home)
                    .join("AppData")
                    .join("Roaming")
                    .join("Zed")
                    .join("settings.json"),
            )
        } else {
            Some(
                PathBuf::from(home)
                    .join(".config")
                    .join("zed")
                    .join("settings.json"),
            )
        }
    }

    #[expect(clippy::excessive_nesting, reason = "evidence explicit")]
    #[expect(clippy::unused_self, reason = "adapter method")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_settings_path() {
            Some(p) => {
                if p.exists() {
                    evidence.push(format!("settings.json exists at {}", p.display()));
                    if let Ok(text) = std::fs::read_to_string(&p) {
                        if text.contains("agent_servers") {
                            evidence.push("settings.json contains agent_servers".to_owned());
                        }
                        if text.contains("context_servers") {
                            evidence.push("settings.json contains context_servers".to_owned());
                        }
                        if text.contains("language_models") {
                            evidence.push("settings.json contains language_models".to_owned());
                        }
                    }
                } else {
                    evidence.push(format!("settings.json missing at {}", p.display()));
                }
                let parent = p.parent().unwrap_or(Path::new("."));
                if parent.join("keymap.json").exists() {
                    evidence.push(format!("keymap.json present at {}", parent.display()));
                }
            }
            None => evidence.push("could not resolve settings path (no HOME)".to_owned()),
        }
        for p in [Path::new(".rules"), Path::new("AGENTS.md")] {
            if p.exists() {
                evidence.push(format!("{} present", p.display()));
            }
        }
    }
}

impl Default for ZedAcpAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "zed-acp is static valid")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for ZedAcpAdapter {
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
        match self.find_binary_in_path() {
            Some(path) => {
                evidence.push(format!(
                    "found binary `{}` at {}",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("zed"),
                    path.display()
                ));
                match Self::probe_version(&path) {
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
            }
        }
        self.collect_config_evidence(&mut evidence);
        let present = match (&binary_path, &version) {
            (Some(_), Some(_)) => InstallPresence::Present,
            (Some(_), None) => InstallPresence::UnknownVersion,
            (None, _) => InstallPresence::Absent,
        };
        let confidence = match (
            &binary_path,
            &version,
            evidence.iter().any(|e| e.contains("settings.json exists")),
        ) {
            (Some(_), None, _) => DetectionConfidence::Medium,
            (None, _, true) => DetectionConfidence::Low,
            (Some(_), Some(_), _) | (None, _, false) => DetectionConfidence::High,
        };
        let confidence = if present == InstallPresence::Absent {
            DetectionConfidence::High
        } else {
            confidence
        };
        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution(&self) -> VersionResolution {
        let detection = self.detection();
        if let Some(v) = detection.version {
            let mut notes = Vec::new();
            notes.push(format!("detected zed version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            // Zed gates ACP wrapper registrations on version; note minimal gate
            notes.push("ACP agent_servers requires Zed >=0.180, MCP forwarding >=0.185".to_owned());
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
            Some("~/.config/zed/settings.json"),
            Some("~/.config/zed/settings.json"),
            Some("%APPDATA%\\Zed\\settings.json"),
            "~/.config/zed/settings.json",
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

        let keymap_resolver = PathResolver::new(
            Some("~/.config/zed/keymap.json"),
            Some("~/.config/zed/keymap.json"),
            Some("%APPDATA%\\Zed\\keymap.json"),
            "~/.config/zed/keymap.json",
        );
        let mut keymap = ConfigSurface::new(
            "keymap.json",
            keymap_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        keymap.precedence = 5;
        keymap.backup_required = false;
        surfaces.push(keymap);

        let agents_resolver = PathResolver::new(
            Some("~/.config/zed/settings.json (agent_servers)"),
            Some("~/.config/zed/settings.json (agent_servers)"),
            Some("%APPDATA%\\Zed\\settings.json (agent_servers)"),
            "~/.config/zed/settings.json (agent_servers wrapper registrations)",
        );
        let mut agents = ConfigSurface::new(
            "agent_servers",
            agents_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        agents.precedence = 12;
        agents.owned_selectors = vec!["agent_servers".to_owned()];
        agents.backup_required = true;
        surfaces.push(agents);

        let mcp_resolver = PathResolver::new(
            Some("~/.config/zed/settings.json (context_servers)"),
            Some("~/.config/zed/settings.json (context_servers)"),
            Some("%APPDATA%\\Zed\\settings.json (context_servers)"),
            "~/.config/zed/settings.json (context_servers MCP)",
        );
        let mut mcp = ConfigSurface::new(
            "context_servers",
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

        let rules_resolver = PathResolver::fallback_only(
            ".rules / AGENTS.md / CLAUDE.md (project, per Zed instructions)",
        );
        let mut rules = ConfigSurface::new(
            ".rules",
            rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        rules.precedence = 8;
        rules.backup_required = false;
        surfaces.push(rules);

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
            "extensions/*".to_owned(),
            "db/*".to_owned(),
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
        let mut plan =
            WrapperPlan::new("ide-user-data via --user-data-dir with ACP wrapper registrations");
        // Zed config is under ~/.config/zed; relocate via XDG_CONFIG_HOME style, plus IDE user-data
        plan.env_vars.push((
            "XDG_CONFIG_HOME".to_owned(),
            instance.config_root.to_string(),
        ));
        let user_data = Path::new(&instance.config_root.to_string()).join("zed-data");
        let extensions = Path::new(&instance.config_root.to_string()).join("extensions");
        plan.args.push(USER_DATA_DIR_FLAG.to_owned());
        plan.args.push(user_data.display().to_string());
        plan.args.push(EXTENSIONS_DIR_FLAG.to_owned());
        plan.args.push(extensions.display().to_string());
        // Record wrapper registration path for evidence
        let wrapper_marker = Path::new(&instance.config_root.to_string())
            .join("zed")
            .join("settings.json");
        plan.description = format!(
            " Wrapper sets XDG_CONFIG_HOME={} and execs `{} {} {}` with extensions {} (wrappers at {})",
            instance.config_root,
            EXECUTABLE,
            USER_DATA_DIR_FLAG,
            user_data.display(),
            extensions.display(),
            wrapper_marker.display()
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/zed/settings.json".to_owned(),
            "~/.config/zed/keymap.json".to_owned(),
            "%APPDATA%\\Zed\\settings.json".to_owned(),
            "settings.json:agent_servers".to_owned(),
            "settings.json:context_servers".to_owned(),
            "--user-data-dir".to_owned(),
            ".rules".to_owned(),
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
            Isolation::IdeUserData | Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("zed-acp requires isolation ide_user_data, got {other}"),
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
}

#[cfg(test)]
mod tests {
    use super::{
        DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, RESEARCH_DOC, USER_DATA_DIR_FLAG, ZedAcpAdapter,
    };
    use crate::adapter::{Adapter, DocumentKind, ProductStatus};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> ZedAcpAdapter {
        ZedAcpAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-zed-1").unwrap(),
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
            ZedAcpAdapter::parse_version_output("zed 0.192.0").as_deref(),
            Some("0.192.0")
        );
        assert_eq!(ZedAcpAdapter::parse_version_output(""), None);
    }

    #[test]
    fn surfaces_include_settings_and_wrappers() {
        let a = adapter();
        let s = a.config_surfaces();
        assert!(s.iter().any(|x| x.id == "settings.json"));
        assert!(s.iter().any(|x| x.id == "agent_servers"));
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
        let inst = sample_instance_with_root("/tmp/.zed-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(plan.args.contains(&USER_DATA_DIR_FLAG.to_owned()));
        let idx = plan
            .args
            .iter()
            .position(|x| x == USER_DATA_DIR_FLAG)
            .unwrap();
        #[expect(clippy::get_unwrap, reason = "test index vetted")]
        let path = plan.args.get(idx + 1).unwrap();
        assert!(path.contains(".zed-work"));
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains("agent_servers") || plan.description.contains("wrapper"));
    }

    #[test]
    fn wrapper_rejects_wrong_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.zed-work");
        inst.harness = HarnessId::new("cursor").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scan_candidates_cover_zed() {
        let a = adapter();
        let c = a.scan_candidates();
        assert!(c.iter().any(|s| s.contains("settings.json")));
        assert!(c.iter().any(|s| s.contains(USER_DATA_DIR_FLAG)));
        assert!(c.iter().any(|s| s.contains("agent_servers")));
    }

    #[test]
    fn validate_accepts_ide_user_data() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.zed-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.zed-work");
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
}
