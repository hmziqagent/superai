//! Auggie adapter — `~/.augment/settings.json` with `.augment/rules`, account/workspace.
//!
//! Research source: `docs/harness-configs/auggie.md` (last verified 2026-08-25).
//! Executable `auggie`, config hierarchy `~/.augment/settings.json` (user) +
//! `<workspace>/.augment/settings.json` + `<workspace>/.augment/settings.local.json`
//! + `/etc/augment/settings.json` (managed), isolation `project-scope` with
//!   account/workspace constrained via `AUGMENT_SESSION_AUTH` and
//!   `--workspace-root` / `--augment-cache-dir`, constrained.

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

/// Harness identifier for Auggie.
pub const HARNESS_ID_STR: &str = "auggie";

/// Human display name.
pub const DISPLAY_NAME: &str = "Auggie";

/// Primary executable.
pub const EXECUTABLE: &str = "auggie";

/// Session auth env var for account isolation.
pub const SESSION_ENV_VAR: &str = "AUGMENT_SESSION_AUTH";

/// Cache dir flag for workspace isolation.
pub const CACHE_DIR_FLAG: &str = "--augment-cache-dir";

/// Workspace root flag.
pub const WORKSPACE_FLAG: &str = "--workspace-root";

/// Session json flag alternative.
pub const SESSION_JSON_FLAG: &str = "--augment-session-json";

/// Add workspace flag.
pub const ADD_WORKSPACE_FLAG: &str = "--add-workspace";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/auggie.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside settings.json.
pub const OWNED_SELECTORS: &[&str] = &[
    "mcpServers",
    "toolPermissions",
    "hooks",
    "enableToolSearch",
    "removedTools",
    "indexingAllowDirs",
    "shell",
    "startupScript",
];

/// MCP selectors.
pub const MCP_OWNED_SELECTORS: &[&str] = &["mcpServers"];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Auggie.
#[derive(Debug, Clone)]
pub struct AuggieAdapter {
    id: HarnessId,
}

impl AuggieAdapter {
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

    /// Session env var.
    pub fn session_env_var(&self) -> &str {
        SESSION_ENV_VAR
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

    fn default_config_root() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".augment"))
    }

    #[expect(clippy::excessive_nesting, reason = "evidence explicit")]
    #[expect(clippy::unused_self, reason = "adapter method")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let settings = root.join("settings.json");
                    if settings.exists() {
                        evidence.push(format!("settings.json found at {}", settings.display()));
                        if let Ok(text) = std::fs::read_to_string(&settings)
                            && text.contains("mcpServers")
                        {
                            evidence.push("settings.json contains mcpServers".to_owned());
                        }
                    } else {
                        evidence.push(format!("settings.json missing at {}", settings.display()));
                    }
                    let session = root.join("session.json");
                    if session.exists() {
                        evidence.push(format!("session.json present at {}", session.display()));
                    }
                    if root.join("rules").exists() {
                        evidence.push(format!(
                            "rules dir present at {}",
                            root.join("rules").display()
                        ));
                    }
                    if root.join("commands").exists() {
                        evidence.push(format!(
                            "commands dir present at {}",
                            root.join("commands").display()
                        ));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => evidence.push("could not resolve config root (no HOME)".to_owned()),
        }
        // workspace .augment
        if Path::new(".augment/settings.json").exists() {
            evidence.push(".augment/settings.json exists (workspace)".to_owned());
        }
        if Path::new(".augment/settings.local.json").exists() {
            evidence.push(".augment/settings.local.json exists".to_owned());
        }
        if Path::new(".augment/rules").exists() {
            evidence.push(".augment/rules present".to_owned());
        }
        if Path::new(".augment-guidelines").exists() {
            evidence.push(".augment-guidelines present".to_owned());
        }
        if Path::new("/etc/augment/settings.json").exists() {
            evidence.push("/etc/augment/settings.json present (managed)".to_owned());
        }
        if let Ok(val) = std::env::var(SESSION_ENV_VAR)
            && !val.trim().is_empty()
        {
            evidence.push(format!("{SESSION_ENV_VAR} is set (len {})", val.len()));
        } else {
            evidence.push(format!("{SESSION_ENV_VAR} not set"));
        }
        if std::env::var("AUGMENT_DISABLE_AUTO_UPDATE").is_ok() {
            evidence.push("AUGMENT_DISABLE_AUTO_UPDATE is set".to_owned());
        }
    }
}

impl Default for AuggieAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "auggie is static valid")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for AuggieAdapter {
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
                    EXECUTABLE,
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
            None => evidence.push(format!("binary `{EXECUTABLE}` not found in PATH")),
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
            evidence.iter().any(|e| e.contains("config root exists")),
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
            notes.push(format!("detected auggie version {v}"));
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

        let user_resolver = PathResolver::new(
            Some("~/.augment/settings.json"),
            Some("~/.augment/settings.json"),
            Some("%USERPROFILE%\\.augment\\settings.json"),
            "~/.augment/settings.json",
        );
        let mut user = ConfigSurface::new(
            "user settings.json",
            user_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        user.precedence = 10;
        user.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        user.backup_required = true;
        user.restart_behavior = RestartBehavior::Reload;
        surfaces.push(user);

        let workspace_resolver =
            PathResolver::fallback_only(".augment/settings.json (project, committed)");
        let mut workspace = ConfigSurface::new(
            "workspace settings.json",
            workspace_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        workspace.precedence = 12;
        workspace.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        workspace.backup_required = true;
        surfaces.push(workspace);

        let local_resolver =
            PathResolver::fallback_only(".augment/settings.local.json (personal, gitignored)");
        let mut local = ConfigSurface::new(
            "settings.local.json",
            local_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        local.precedence = 13;
        local.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        local.backup_required = true;
        surfaces.push(local);

        let managed_resolver = PathResolver::new(
            Some("/etc/augment/settings.json"),
            Some("/etc/augment/settings.json"),
            Some("C:\\ProgramData\\augment\\settings.json"),
            "/etc/augment/settings.json",
        );
        let mut managed = ConfigSurface::new(
            "managed settings.json",
            managed_resolver,
            DocumentKind::Json,
            ConfigScope::SystemManaged,
            SurfaceOwnership::HarnessManaged,
        );
        managed.precedence = 20;
        managed.backup_required = false;
        managed.restart_behavior = RestartBehavior::Reload;
        surfaces.push(managed);

        let session_resolver = PathResolver::new(
            Some("~/.augment/session.json"),
            Some("~/.augment/session.json"),
            Some("%USERPROFILE%\\.augment\\session.json"),
            "~/.augment/session.json",
        );
        let mut session = ConfigSurface::new(
            "session.json",
            session_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        session.precedence = 15;
        session.backup_required = false;
        session.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(session);

        let rules_resolver =
            PathResolver::fallback_only(".augment/rules/*.md / ~/.augment/rules/*.md");
        let mut rules = ConfigSurface::new(
            ".augment/rules",
            rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        rules.precedence = 14;
        rules.backup_required = false;
        surfaces.push(rules);

        let commands_resolver = PathResolver::fallback_only(".augment/commands/*.md");
        let mut commands = ConfigSurface::new(
            ".augment/commands",
            commands_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        commands.precedence = 7;
        commands.backup_required = false;
        surfaces.push(commands);

        let guidelines_resolver = PathResolver::fallback_only(
            ".augment-guidelines / CLAUDE.md / AGENTS.md (hierarchical)",
        );
        let mut guidelines = ConfigSurface::new(
            ".augment-guidelines",
            guidelines_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        guidelines.precedence = 9;
        guidelines.backup_required = false;
        surfaces.push(guidelines);

        let ignore_resolver = PathResolver::fallback_only(".augmentignore");
        let mut ignore = ConfigSurface::new(
            ".augmentignore",
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
            "session.json".to_owned(),
            "*.log".to_owned(),
            "logs/*".to_owned(),
            "cache/*".to_owned(),
            "tmp/*".to_owned(),
            ".tmp/*".to_owned(),
            "*.lock".to_owned(),
            "index/*".to_owned(),
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
            "account/workspace via AUGMENT_SESSION_AUTH + --workspace-root + --augment-cache-dir",
        );
        // Account isolation via AUGMENT_SESSION_AUTH pointing at per-instance session file
        let session_path = Path::new(&instance.config_root.to_string()).join("session.json");
        plan.env_vars.push((
            SESSION_ENV_VAR.to_owned(),
            session_path.display().to_string(),
        ));
        plan.env_vars
            .push(("AUGMENT_DISABLE_AUTO_UPDATE".to_owned(), "1".to_owned()));
        // Workspace and cache isolation
        let workspace = Path::new(&instance.config_root.to_string()).join("workspace");
        let cache = Path::new(&instance.config_root.to_string()).join("cache");
        plan.args.push(WORKSPACE_FLAG.to_owned());
        plan.args.push(workspace.display().to_string());
        plan.args.push(CACHE_DIR_FLAG.to_owned());
        plan.args.push(cache.display().to_string());
        plan.args.push(SESSION_JSON_FLAG.to_owned());
        plan.args.push(session_path.display().to_string());
        plan.description = format!(
            " Wrapper sets {}={} and execs `{} {} {}` {} {} and {} {} (account/workspace constrained)",
            SESSION_ENV_VAR,
            session_path.display(),
            EXECUTABLE,
            WORKSPACE_FLAG,
            workspace.display(),
            CACHE_DIR_FLAG,
            cache.display(),
            SESSION_JSON_FLAG,
            session_path.display()
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.augment/settings.json".to_owned(),
            "~/.augment/session.json".to_owned(),
            ".augment/settings.json".to_owned(),
            ".augment/settings.local.json".to_owned(),
            ".augment/rules".to_owned(),
            "$AUGMENT_SESSION_AUTH".to_owned(),
            "--workspace-root".to_owned(),
            "/etc/augment/settings.json".to_owned(),
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
                reason: format!("auggie requires isolation project_scope, got {other}"),
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
        AuggieAdapter, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, RESEARCH_DOC, SESSION_ENV_VAR,
        WORKSPACE_FLAG,
    };
    use crate::adapter::{Adapter, DocumentKind, ProductStatus};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> AuggieAdapter {
        AuggieAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-auggie-1").unwrap(),
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
        assert_eq!(a.session_env_var(), SESSION_ENV_VAR);
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
            AuggieAdapter::parse_version_output("auggie 0.5.0").as_deref(),
            Some("0.5.0")
        );
        assert_eq!(AuggieAdapter::parse_version_output(""), None);
    }

    #[test]
    fn surfaces_include_settings_and_rules() {
        let a = adapter();
        let s = a.config_surfaces();
        assert!(s.iter().any(|x| x.id == "user settings.json"));
        assert!(s.iter().any(|x| x.id == ".augment/rules"));
        assert!(s.iter().any(|x| x.kind == DocumentKind::Json));
        assert!(s.iter().any(|x| x.id == "session.json"));
    }

    #[test]
    fn operations_constrained() {
        let a = adapter();
        for (_, sup) in a.supported_operations() {
            assert_eq!(sup, AdapterSupport::Constrained);
        }
    }

    #[test]
    fn wrapper_has_workspace_and_session() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.augment-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(plan.env_vars.iter().any(|(k, _)| k == SESSION_ENV_VAR));
        assert!(plan.args.contains(&WORKSPACE_FLAG.to_owned()));
        assert!(plan.args.contains(&"--augment-cache-dir".to_owned()));
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(SESSION_ENV_VAR));
    }

    #[test]
    fn wrapper_rejects_wrong_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.augment-work");
        inst.harness = HarnessId::new("cursor").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scan_candidates_cover_augment() {
        let a = adapter();
        let c = a.scan_candidates();
        assert!(c.iter().any(|s| s.contains(".augment")));
        assert!(c.iter().any(|s| s.contains(SESSION_ENV_VAR)));
    }

    #[test]
    fn validate_accepts_project_scope() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.augment-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.augment-work");
        inst.isolation = Isolation::IdeUserData;
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
