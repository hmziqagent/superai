//! Warp adapter — CLI TOML MCP JSON workflows YAML, Linux XDG/profile Constrained.
//!
//! Research source: `docs/harness-configs/warp.md` (last verified 2026-08-25).
//! Executable `warp`, Agent CLI `~/.warp_cli/settings.toml` TOML (macOS) and
//! `~/.config/warp-terminal/cli/settings.toml` XDG on Linux (`$XDG_CONFIG_HOME`),
//! global `~/.warp/.mcp.json` JSON plus CLI `~/.warp_cli/.mcp.json`, workflows
//! `~/.warp/workflows/*.yaml` YAML (local) and `${XDG_DATA_HOME:-~/.local/share}/warp-terminal/workflows/`, custom routers
//! `~/.warp/custom_model_routers/`, themes, `WARP_API_KEY`/`WARP_TUI_DISABLE_AUTOUPDATE`
//! env, platform-mediated inference (no arbitrary `base_url`), isolation `os_bound`,
//! support `Constrained` (Linux XDG/profile constrained).

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

/// Harness identifier for Warp.
pub const HARNESS_ID_STR: &str = "warp";

/// Human display name.
pub const DISPLAY_NAME: &str = "Warp Agent CLI/app";

/// Primary executable name.
pub const EXECUTABLE: &str = "warp";

/// Alternative executable name (agent CLI alias).
pub const EXECUTABLE_ALT: &str = "warp-agent-cli";

/// Environment variable for headless API key.
pub const API_KEY_ENV_VAR: &str = "WARP_API_KEY";

/// Environment variable for XDG config relocation (Linux CLI).
pub const XDG_CONFIG_HOME_ENV_VAR: &str = "XDG_CONFIG_HOME";

/// Environment variable for XDG data relocation (workflows Linux).
pub const XDG_DATA_HOME_ENV_VAR: &str = "XDG_DATA_HOME";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/warp.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Constrained note — Linux XDG/profile constrained.
pub const CONSTRAINED_NOTE: &str = "CLI TOML `~/.warp_cli/settings.toml` (macOS) vs `~/.config/warp-terminal/cli/settings.toml` (Linux XDG `$XDG_CONFIG_HOME`) plus MCP JSON `~/.warp/.mcp.json` vs `~/.warp_cli/.mcp.json`, workflows YAML `~/.warp/workflows/` vs XDG_DATA_HOME, Linux XDG/profile constrained: no documented WARP_HOME, only CLI XDG isolation documented, app settings/GUI not relocatable, inference platform-mediated (no arbitrary base_url, BYOK billing-only)";

/// Owned selectors for provider/model/mcp mutation inside CLI TOML and MCP JSON.
/// CLI settings are dotted-section TOML; we own appearance, general, agents, and mcpServers.
pub const OWNED_SELECTORS: &[&str] = &[
    "appearance.theme",
    "general.autoupdate_enabled",
    "agents.default_model",
    "agents.profiles",
    "mcpServers",
    "mcp.mcpServers",
    "custom_model_routers",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Warp (`Constrained`, `os_bound`).
///
/// Isolation is `os_bound`: Linux CLI honors `$XDG_CONFIG_HOME` for
/// `~/.config/warp-terminal/cli/settings.toml` and `$XDG_DATA_HOME` for
/// workflows; macOS `~/.warp_cli/` not XDG-relocatable, and app/GUI
/// settings/Drive are not portable. `WARP_API_KEY` provides per-instance
/// credential isolation; no `WARP_HOME` exists.
#[derive(Debug, Clone)]
pub struct WarpAdapter {
    id: HarnessId,
}

impl WarpAdapter {
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

    /// API key env var.
    pub fn api_key_env_var(&self) -> &str {
        API_KEY_ENV_VAR
    }

    /// Constrained note.
    pub fn constrained_note(&self) -> &str {
        CONSTRAINED_NOTE
    }

    /// Try to locate the `warp` binary via `PATH`.
    #[expect(clippy::unused_self, reason = "adapter method uses instance constants")]
    #[expect(clippy::excessive_nesting, reason = "PATH scan branches are explicit")]
    fn find_binary_in_path(&self) -> Option<PathBuf> {
        let path_var = std::env::var("PATH").ok()?;
        let separator = if cfg!(windows) { ';' } else { ':' };
        for exec in [EXECUTABLE, EXECUTABLE_ALT] {
            for dir in path_var.split(separator) {
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

    /// Probe `warp --version` with a timeout, returning the parsed version string if successful.
    fn probe_version(binary: &Path) -> Option<String> {
        let binary_owned = binary.to_path_buf();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let output = Command::new(&binary_owned)
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

    /// Parse version output like `warp 1.2.3` into `1.2.3`.
    #[expect(
        clippy::excessive_nesting,
        reason = "version parsing branches are explicit"
    )]
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

    /// Resolve the CLI settings path candidate (macOS `~/.warp_cli/settings.toml` or Linux XDG).
    fn cli_settings_path() -> Option<PathBuf> {
        if let Ok(xdg) = std::env::var(XDG_CONFIG_HOME_ENV_VAR)
            && !xdg.trim().is_empty()
        {
            return Some(
                PathBuf::from(xdg)
                    .join("warp-terminal")
                    .join("cli")
                    .join("settings.toml"),
            );
        }
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        if cfg!(target_os = "linux") {
            Some(
                PathBuf::from(home)
                    .join(".config")
                    .join("warp-terminal")
                    .join("cli")
                    .join("settings.toml"),
            )
        } else if cfg!(windows) {
            // Windows: %LOCALAPPDATA%\warp\Warp\config\cli\settings.toml
            if let Ok(local) = std::env::var("LOCALAPPDATA")
                && !local.trim().is_empty()
            {
                return Some(
                    PathBuf::from(local)
                        .join("warp")
                        .join("Warp")
                        .join("config")
                        .join("cli")
                        .join("settings.toml"),
                );
            }
            Some(
                PathBuf::from(home)
                    .join("AppData")
                    .join("Local")
                    .join("warp")
                    .join("Warp")
                    .join("config")
                    .join("cli")
                    .join("settings.toml"),
            )
        } else {
            Some(PathBuf::from(home).join(".warp_cli").join("settings.toml"))
        }
    }

    /// Build detection evidence about CLI config, MCP, workflows, and env.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    #[expect(
        clippy::too_many_lines,
        reason = "evidence branches are explicit for warp surfaces"
    )]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("constrained: {CONSTRAINED_NOTE}"));
        match Self::cli_settings_path() {
            Some(path) => {
                if path.exists() {
                    evidence.push(format!("CLI settings.toml found at {}", path.display()));
                    if let Ok(text) = std::fs::read_to_string(&path)
                        && (text.contains("[appearance]") || text.contains("theme"))
                    {
                        evidence.push("CLI settings.toml contains appearance/theme".to_owned());
                    }
                } else {
                    evidence.push(format!("CLI settings.toml missing at {}", path.display()));
                }
            }
            None => evidence.push("could not resolve CLI settings path (no HOME)".to_owned()),
        }
        // Global MCP
        let home_opt = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok());
        if let Some(home) = home_opt
            && !home.trim().is_empty()
        {
            let global_mcp = PathBuf::from(&home).join(".warp").join(".mcp.json");
            if global_mcp.exists() {
                evidence.push(format!(
                    "global .mcp.json found at {}",
                    global_mcp.display()
                ));
                if let Ok(text) = std::fs::read_to_string(&global_mcp)
                    && text.contains("mcpServers")
                {
                    evidence.push("global .mcp.json contains mcpServers".to_owned());
                }
            } else {
                evidence.push(format!(
                    "global .mcp.json missing at {}",
                    global_mcp.display()
                ));
            }
            let cli_mcp = if cfg!(target_os = "linux") {
                Self::cli_settings_path().and_then(|p| p.parent().map(|d| d.join(".mcp.json")))
            } else {
                Some(PathBuf::from(&home).join(".warp_cli").join(".mcp.json"))
            };
            if let Some(p) = cli_mcp {
                if p.exists() {
                    evidence.push(format!("CLI .mcp.json found at {}", p.display()));
                } else {
                    evidence.push(format!("CLI .mcp.json missing at {}", p.display()));
                }
            }
            // Workflows
            let workflows = PathBuf::from(&home).join(".warp").join("workflows");
            if workflows.exists() {
                evidence.push(format!("workflows dir present at {}", workflows.display()));
            }
            // XDG workflows Linux
            let xdg_data = std::env::var(XDG_DATA_HOME_ENV_VAR).ok();
            if let Some(xdg) = xdg_data
                && !xdg.trim().is_empty()
            {
                let xdg_workflows = PathBuf::from(xdg).join("warp-terminal").join("workflows");
                if xdg_workflows.exists() {
                    evidence.push(format!(
                        "XDG workflows dir present at {}",
                        xdg_workflows.display()
                    ));
                }
            } else if cfg!(target_os = "linux") {
                let xdg_default = PathBuf::from(&home)
                    .join(".local")
                    .join("share")
                    .join("warp-terminal")
                    .join("workflows");
                if xdg_default.exists() {
                    evidence.push(format!(
                        "XDG workflows dir present at {}",
                        xdg_default.display()
                    ));
                }
            }
            let custom_routers = PathBuf::from(&home)
                .join(".warp")
                .join("custom_model_routers");
            if custom_routers.exists() {
                evidence.push(format!(
                    "custom_model_routers present at {}",
                    custom_routers.display()
                ));
            }
            // Project AGENTS.md / WARP.md
            if Path::new("AGENTS.md").exists() {
                evidence.push("project AGENTS.md found in cwd".to_owned());
            }
            if Path::new("WARP.md").exists() {
                evidence.push("project WARP.md found in cwd".to_owned());
            }
        }
        for var in [
            API_KEY_ENV_VAR,
            XDG_CONFIG_HOME_ENV_VAR,
            XDG_DATA_HOME_ENV_VAR,
            "WARP_TUI_DISABLE_AUTOUPDATE",
        ] {
            if let Ok(val) = std::env::var(var)
                && !val.trim().is_empty()
            {
                let preview = if var.contains("KEY") || var.contains("TOKEN") {
                    "[REDACTED]".to_owned()
                } else if val.chars().count() > 80 {
                    let truncated: String = val.chars().take(80).collect();
                    format!("{truncated}…")
                } else {
                    val
                };
                evidence.push(format!("{var} set to {preview}"));
            } else {
                evidence.push(format!("{var} not set"));
            }
        }
        evidence.push("app settings and CLI settings are two separate stores, only CLI XDG relocatable on Linux".to_owned());
        evidence.push(
            "no WARP_HOME; inference via Warp platform, BYOK billing-only, no arbitrary base_url"
                .to_owned(),
        );
    }
}

impl Default for WarpAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "warp is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for WarpAdapter {
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

        let confidence = match (
            &binary_path,
            &version,
            evidence
                .iter()
                .any(|e| e.contains("CLI settings.toml found")),
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
            notes.push(format!("detected warp version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            notes.push(format!("constrained: {CONSTRAINED_NOTE}"));
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

        let cli_mac_resolver = PathResolver::new(
            Some("~/.warp_cli/settings.toml (macOS, fixed)"),
            Some("~/.warp_cli/settings.toml (macOS, not XDG)"),
            Some("%LOCALAPPDATA%\\warp\\Warp\\config\\cli\\settings.toml (Windows)"),
            "~/.warp_cli/settings.toml (macOS) — not XDG relocatable, CLI-only",
        );
        let mut cli_mac = ConfigSurface::new(
            "settings.toml (CLI, macOS)",
            cli_mac_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        cli_mac.precedence = 10;
        cli_mac.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        cli_mac.backup_required = true;
        cli_mac.restart_behavior = RestartBehavior::Reload;
        surfaces.push(cli_mac);

        let cli_linux_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/warp-terminal/cli/settings.toml (Linux XDG, relocatable)"),
            Some("~/.warp_cli/settings.toml (macOS, fixed)"),
            Some("%LOCALAPPDATA%\\warp\\Warp\\config\\cli\\settings.toml"),
            "~/.config/warp-terminal/cli/settings.toml (Linux, respects $XDG_CONFIG_HOME)",
        );
        let mut cli_linux = ConfigSurface::new(
            "settings.toml (CLI, Linux XDG)",
            cli_linux_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        cli_linux.precedence = 11;
        cli_linux.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        cli_linux.backup_required = true;
        cli_linux.restart_behavior = RestartBehavior::Reload;
        surfaces.push(cli_linux);

        let mcp_global_resolver = PathResolver::new(
            Some("~/.warp/.mcp.json (global file-based MCP, cross-OS)"),
            Some("~/.warp/.mcp.json (global MCP, macOS)"),
            Some("~/.warp/.mcp.json (global MCP, Windows)"),
            "~/.warp/.mcp.json (global MCP, auto-spawn)",
        );
        let mut mcp_global = ConfigSurface::new(
            ".mcp.json (global)",
            mcp_global_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp_global.precedence = 12;
        mcp_global.owned_selectors = vec!["mcpServers".to_owned()];
        mcp_global.backup_required = true;
        surfaces.push(mcp_global);

        let mcp_cli_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/warp-terminal/cli/.mcp.json (Linux XDG, CLI MCP)"),
            Some("~/.warp_cli/.mcp.json (macOS CLI MCP, separate from app)"),
            Some("%LOCALAPPDATA%\\warp\\Warp\\config\\cli\\.mcp.json"),
            "~/.warp_cli/.mcp.json (CLI) / $XDG_CONFIG_HOME/warp-terminal/cli/.mcp.json (Linux)",
        );
        let mut mcp_cli = ConfigSurface::new(
            ".mcp.json (CLI)",
            mcp_cli_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp_cli.precedence = 13;
        mcp_cli.owned_selectors = vec!["mcpServers".to_owned()];
        mcp_cli.backup_required = true;
        surfaces.push(mcp_cli);

        let workflows_local_resolver = PathResolver::new(
            Some("${XDG_DATA_HOME:-~/.local/share}/warp-terminal/workflows/*.yaml (Linux)"),
            Some("~/.warp/workflows/*.yaml (macOS local workflows)"),
            Some("%APPDATA%\\warp\\Warp\\data\\workflows\\*.yaml (Windows)"),
            "~/.warp/workflows/*.yaml (local, portable) — also {{repo}}/.warp/workflows/ (repo-scoped)",
        );
        let mut workflows = ConfigSurface::new(
            "workflows (local)",
            workflows_local_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        workflows.precedence = 9;
        workflows.backup_required = false;
        workflows.restart_behavior = RestartBehavior::Reload;
        surfaces.push(workflows);

        let workflows_project_resolver =
            PathResolver::fallback_only("{{repo}}/.warp/workflows/*.yaml (project, committed)");
        let mut workflows_project = ConfigSurface::new(
            "workflows (project)",
            workflows_project_resolver,
            DocumentKind::Yaml,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        workflows_project.precedence = 14;
        workflows_project.backup_required = false;
        surfaces.push(workflows_project);

        let routers_resolver =
            PathResolver::fallback_only("~/.warp/custom_model_routers/*.yaml (custom routers)");
        let mut routers = ConfigSurface::new(
            "custom_model_routers",
            routers_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        routers.precedence = 7;
        routers.backup_required = false;
        surfaces.push(routers);

        let themes_resolver =
            PathResolver::fallback_only("~/.warp/themes/* (custom themes, AGPL client)");
        let mut themes = ConfigSurface::new(
            "themes",
            themes_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        themes.precedence = 4;
        themes.backup_required = false;
        surfaces.push(themes);

        let rules_resolver = PathResolver::fallback_only(
            "AGENTS.md/WARP.md (project rules, ALL CAPS) — Global Rules are cloud-synced, not files",
        );
        let mut rules = ConfigSurface::new(
            "AGENTS.md/WARP.md (project rules)",
            rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        rules.precedence = 15;
        rules.backup_required = false;
        surfaces.push(rules);

        let env_resolver = PathResolver::new(
            Some(
                "$WARP_API_KEY (headless, per-instance credential) + $WARP_TUI_DISABLE_AUTOUPDATE",
            ),
            Some("$WARP_API_KEY (headless CI)"),
            Some("%WARP_API_KEY%"),
            "$WARP_API_KEY (headless auth) + $XDG_CONFIG_HOME/$XDG_DATA_HOME (Linux isolation) + CLI --set-provider-api-key flags",
        );
        let mut env_surface = ConfigSurface::new(
            "env (WARP_* + XDG_*)",
            env_resolver,
            DocumentKind::Env,
            ConfigScope::SessionInline,
            SurfaceOwnership::ExternalSecretStore,
        );
        env_surface.precedence = 20;
        env_surface.owned_selectors = vec![
            "WARP_API_KEY".to_owned(),
            "XDG_CONFIG_HOME".to_owned(),
            "XDG_DATA_HOME".to_owned(),
            "WARP_TUI_DISABLE_AUTOUPDATE".to_owned(),
        ];
        env_surface.backup_required = false;
        env_surface.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(env_surface);

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
            "logs/*".to_owned(),
            "cache/*".to_owned(),
            "telemetry/*".to_owned(),
            "sessions/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "tmp/*".to_owned(),
            "*.lock".to_owned(),
            ".mcp-auth/*".to_owned(),
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
            "os_bound via XDG_CONFIG_HOME/XDG_DATA_HOME + WARP_API_KEY (Linux CLI constrained, app/GUI not relocatable)",
        );
        // Linux CLI isolation via XDG_CONFIG_HOME — moves settings.toml and CLI .mcp.json
        let xdg_config = format!("{}/config", instance.config_root);
        let xdg_data = format!("{}/data", instance.config_root);
        plan.env_vars
            .push((XDG_CONFIG_HOME_ENV_VAR.to_owned(), xdg_config.clone()));
        plan.env_vars
            .push((XDG_DATA_HOME_ENV_VAR.to_owned(), xdg_data.clone()));
        // Per-instance credential
        plan.env_vars.push((
            API_KEY_ENV_VAR.to_owned(),
            format!("{}/api_key", instance.config_root),
        ));
        plan.description = format!(
            " Wrapper sets {XDG_CONFIG_HOME_ENV_VAR}={xdg_config} {XDG_DATA_HOME_ENV_VAR}={xdg_data} {API_KEY_ENV_VAR}=<per-instance> and execs `{EXECUTABLE}` — Linux CLI XDG relocatable per docs ({CONSTRAINED_NOTE}); macOS ~/Library paths and app settings/Drive not relocatable, project AGENTS.md/WARP.md via cwd"
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.warp_cli/settings.toml".to_owned(),
            "~/.config/warp-terminal/cli/settings.toml".to_owned(),
            "~/.warp/.mcp.json".to_owned(),
            "~/.warp_cli/.mcp.json".to_owned(),
            "~/.warp/workflows".to_owned(),
            "${XDG_DATA_HOME}/warp-terminal/workflows (Linux)".to_owned(),
            "~/.warp/custom_model_routers".to_owned(),
            "$XDG_CONFIG_HOME/warp-terminal/cli/.mcp.json (Linux XDG MCP)".to_owned(),
            "$WARP_API_KEY via WARP_API_KEY".to_owned(),
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
            Isolation::OsBound | Isolation::Unknown | Isolation::RelocatedRoot => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "warp requires isolation os_bound (Linux XDG/profile constrained), got {other} — {CONSTRAINED_NOTE}"
                ),
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
    use std::collections::HashSet;

    use super::{
        API_KEY_ENV_VAR, CONSTRAINED_NOTE, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR,
        OWNED_SELECTORS, RESEARCH_DOC, WarpAdapter, XDG_CONFIG_HOME_ENV_VAR, XDG_DATA_HOME_ENV_VAR,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> WarpAdapter {
        WarpAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-warp-1").unwrap(),
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
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(a.display_name(), DISPLAY_NAME);
        assert_eq!(a.executable_name(), EXECUTABLE);
        assert_eq!(a.api_key_env_var(), API_KEY_ENV_VAR);
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert_eq!(a.adapter_revision(), crate::adapter::ADAPTER_REVISION);
        assert!(a.constrained_note().contains("XDG"));
        assert!(CONSTRAINED_NOTE.contains("Linux"));
        assert!(CONSTRAINED_NOTE.contains("no arbitrary base_url"));
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
        assert!(result.evidence.iter().any(|e| e.contains("constrained")));
        match result.present {
            InstallPresence::Absent => assert!(result.version.is_none()),
            InstallPresence::Present => assert!(result.version.is_some()),
            InstallPresence::UnknownVersion => {
                assert!(result.evidence.iter().any(|e| e.contains("found binary")));
            }
            InstallPresence::Broken => assert!(!result.evidence.is_empty()),
        }
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
            assert!(res.notes.iter().any(|n| n.contains("warp")));
        } else {
            assert!(!res.compatible);
            assert!(res.schema_version.is_none());
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("warp 1.2.3", Some("1.2.3")),
            ("1.0.0", Some("1.0.0")),
            ("v1.0.0", Some("1.0.0")),
            ("Version: 2.0.0", Some("2.0.0")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = WarpAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_cli_toml_and_mcp_and_workflows() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 8);
        let cli_macos = surfaces
            .iter()
            .find(|s| s.id == "settings.toml (CLI, macOS)")
            .expect("CLI macOS settings must exist");
        assert_eq!(cli_macos.kind, DocumentKind::Toml);
        assert_eq!(cli_macos.scope, ConfigScope::User);
        for sel in OWNED_SELECTORS {
            assert!(cli_macos.owned_selectors.contains(&(*sel).to_owned()));
        }
        let cli_linux = surfaces
            .iter()
            .find(|s| s.id == "settings.toml (CLI, Linux XDG)")
            .expect("Linux XDG settings must exist");
        assert_eq!(cli_linux.kind, DocumentKind::Toml);
        assert!(cli_linux.path_resolver.fallback.contains("XDG_CONFIG_HOME"));
        let mcp_global = surfaces
            .iter()
            .find(|s| s.id == ".mcp.json (global)")
            .expect("global mcp must exist");
        assert_eq!(mcp_global.kind, DocumentKind::Json);
        let workflows = surfaces
            .iter()
            .find(|s| s.id == "workflows (local)")
            .expect("workflows must exist");
        assert_eq!(workflows.kind, DocumentKind::Yaml);
        let env = surfaces
            .iter()
            .find(|s| s.id == "env (WARP_* + XDG_*)")
            .expect("env must exist");
        assert_eq!(env.kind, DocumentKind::Env);
        assert_eq!(env.ownership, SurfaceOwnership::ExternalSecretStore);
        assert!(env.owned_selectors.contains(&"WARP_API_KEY".to_owned()));
    }

    #[test]
    fn owned_selectors_are_stable() {
        assert!(OWNED_SELECTORS.len() >= 5);
        let set: HashSet<&str> = OWNED_SELECTORS.iter().copied().collect();
        assert_eq!(set.len(), OWNED_SELECTORS.len(), "selectors must be unique");
        for required in ["appearance.theme", "mcpServers"] {
            assert!(set.contains(required), "missing {required}");
        }
    }

    #[test]
    fn supported_operations_are_constrained() {
        let a = adapter();
        let ops = a.supported_operations();
        assert!(!ops.is_empty());
        for (_, support) in &ops {
            assert_eq!(*support, AdapterSupport::Constrained);
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
    fn plan_mirror_exclusions_cover_logs_and_mcp_auth() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        for pat in ["logs/*", "cache/*", "*.lock", ".mcp-auth/*"] {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"settings.toml (CLI, Linux XDG)".to_owned()));
    }

    #[test]
    fn plan_wrapper_sets_xdg_and_api_key() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.warp-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == XDG_CONFIG_HOME_ENV_VAR && v == "/tmp/.warp-work/config")
        );
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == XDG_DATA_HOME_ENV_VAR && v == "/tmp/.warp-work/data")
        );
        assert!(plan.env_vars.iter().any(|(k, _)| k == API_KEY_ENV_VAR));
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(XDG_CONFIG_HOME_ENV_VAR));
        assert!(plan.description.contains("Linux CLI XDG"));
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my warp work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let xdg_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == XDG_CONFIG_HOME_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(xdg_val, "/tmp/my warp work/config");
        assert!(xdg_val.contains(' '));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.warp-work");
        inst.harness = HarnessId::new("claude-code").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_toml_and_mcp_and_workflows() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.contains("settings.toml")));
        assert!(candidates.iter().any(|c| c.contains(".mcp.json")));
        assert!(candidates.iter().any(|c| c.contains("workflows")));
        assert!(candidates.iter().any(|c| c.contains("XDG_CONFIG_HOME")));
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("XDG_DATA_HOME") || c.contains("custom_model_routers"))
        );
    }

    #[test]
    fn validate_instance_accepts_os_bound() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.warp-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.warp-work");
        inst.isolation = Isolation::EnvOnly;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn supported_skill_modes_matches_catalog_constrained() {
        let a = adapter();
        let modes = a.supported_skill_modes();
        assert_eq!(modes.len(), 3);
        let s: HashSet<String> = modes.iter().map(ToString::to_string).collect();
        assert!(s.contains("link_all"));
        assert!(s.contains("link_selected"));
        assert!(s.contains("copy_selected"));
    }
}
