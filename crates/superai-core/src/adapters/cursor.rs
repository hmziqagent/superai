//! Cursor adapter — `CURSOR_CONFIG_DIR` plus IDE `--user-data-dir` isolation.
//!
//! Research source: `docs/harness-configs/cursor.md` (last verified 2026-08-25).
//! Executables `cursor` (IDE) and `agent`/`cursor-agent` (CLI), CLI config
//! `~/.cursor/cli-config.json` via `$CURSOR_CONFIG_DIR`, IDE user-data via
//! `--user-data-dir` + `--extensions-dir`, isolation `ide-user-data`.

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

/// Harness identifier for Cursor.
pub const HARNESS_ID_STR: &str = "cursor";

/// Human display name.
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

/// Default CLI config fallback.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.cursor";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/cursor.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
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

/// Owned selectors for MCP.
pub const MCP_OWNED_SELECTORS: &[&str] = &["mcpServers"];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

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

    /// Borrow harness id.
    pub fn harness_id(&self) -> &HarnessId {
        &self.id
    }

    /// Executable name.
    pub fn executable_name(&self) -> &str {
        EXECUTABLE
    }

    /// Config env var.
    pub fn config_env_var(&self) -> &str {
        CONFIG_ENV_VAR
    }

    /// Try locate binary via PATH, checking `cursor`, `agent`, `cursor-agent`.
    #[expect(clippy::unused_self, reason = "uses instance constants")]
    #[expect(clippy::excessive_nesting, reason = "PATH scan branches are explicit")]
    fn find_binary_in_path(&self) -> Option<PathBuf> {
        let path_var = std::env::var("PATH").ok()?;
        let sep = if cfg!(windows) { ';' } else { ':' };
        for exec in [EXECUTABLE, EXECUTABLE_CLI, EXECUTABLE_LEGACY] {
            for dir in path_var.split(sep) {
                if dir.is_empty() {
                    continue;
                }
                let candidate = Path::new(dir).join(exec);
                if candidate.is_file() {
                    return Some(candidate);
                }
                if cfg!(windows) {
                    let exe_candidate = Path::new(dir).join(format!("{exec}.exe"));
                    if exe_candidate.is_file() {
                        return Some(exe_candidate);
                    }
                }
            }
        }
        None
    }

    /// Probe `--version`.
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

    /// Parse version token.
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

    /// Resolve default CLI config root.
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
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".cursor"))
    }

    /// IDE user-data root default.
    fn default_user_data_root() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        if cfg!(target_os = "macos") {
            Some(
                PathBuf::from(home)
                    .join("Library")
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
                PathBuf::from(home)
                    .join("AppData")
                    .join("Roaming")
                    .join("Cursor")
                    .join("User"),
            )
        } else {
            Some(
                PathBuf::from(home)
                    .join(".config")
                    .join("Cursor")
                    .join("User"),
            )
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
                        if let Ok(text) = std::fs::read_to_string(&cli_config)
                            && text.contains("permissions")
                        {
                            evidence.push("cli-config.json contains permissions".to_owned());
                        }
                    } else if alt.exists() {
                        evidence.push(format!("cli.json found at {}", alt.display()));
                    } else {
                        evidence.push(format!(
                            "cli-config.json missing at {}",
                            cli_config.display()
                        ));
                    }
                    let mcp = root.join("mcp.json");
                    if mcp.exists() {
                        evidence.push(format!("mcp.json found at {}", mcp.display()));
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
        #[expect(clippy::unwrap_used, reason = "cursor is static valid")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
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

    #[expect(clippy::single_match_else, reason = "detection branching explicit")]
    fn detection(&self) -> DetectionResult {
        let mut evidence = Vec::new();
        let mut version: Option<String> = None;
        let mut binary_path: Option<PathBuf> = None;
        match self.find_binary_in_path() {
            Some(path) => {
                evidence.push(format!(
                    "found binary `{}` at {}",
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("cursor"),
                    path.display()
                ));
                match Self::probe_version(&path) {
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
            notes.push(format!("detected cursor version {v}"));
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

        let mcp_resolver = PathResolver::new(
            Some("$CURSOR_CONFIG_DIR/mcp.json"),
            Some("$CURSOR_CONFIG_DIR/mcp.json"),
            Some("%CURSOR_CONFIG_DIR%\\mcp.json"),
            "~/.cursor/mcp.json",
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
        let mut plan = WrapperPlan::new("ide-user-data via --user-data-dir + CURSOR_CONFIG_DIR");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        let user_data = Path::new(&instance.config_root.to_string()).join("vscode-data");
        let extensions = Path::new(&instance.config_root.to_string()).join("extensions");
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
            "$CURSOR_CONFIG_DIR/mcp.json".to_owned(),
            ".cursor/cli.json".to_owned(),
            ".cursor/mcp.json".to_owned(),
            "~/.config/Cursor/User/settings.json".to_owned(),
            "--user-data-dir".to_owned(),
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
            | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("cursor requires isolation ide_user_data, got {other}"),
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
        CONFIG_ENV_VAR, CursorAdapter, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, OWNED_SELECTORS,
        RESEARCH_DOC, USER_DATA_DIR_FLAG,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
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
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(a.display_name(), DISPLAY_NAME);
        assert_eq!(a.executable_name(), EXECUTABLE);
        assert_eq!(a.config_env_var(), CONFIG_ENV_VAR);
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
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
            CursorAdapter::parse_version_output("cursor 1.2.3").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            CursorAdapter::parse_version_output("agent 0.5.0").as_deref(),
            Some("0.5.0")
        );
        assert_eq!(CursorAdapter::parse_version_output(""), None);
        assert_eq!(CursorAdapter::parse_version_output("not a version"), None);
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

    #[test]
    fn supported_operations_constrained() {
        let a = adapter();
        for (_, support) in a.supported_operations() {
            assert_eq!(support, AdapterSupport::Constrained);
        }
    }

    #[test]
    fn plan_wrapper_sets_env_and_user_data_dir() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.cursor-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == "/tmp/.cursor-work")
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
        let mut inst = sample_instance_with_root("/tmp/.cursor-work");
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
        let inst = sample_instance_with_root("/tmp/.cursor-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.cursor-work");
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
}
