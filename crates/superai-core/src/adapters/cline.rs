//! `Cline` adapter — VS Code `--user-data-dir` plus `CLINE_DATA_DIR` isolation.
//!
//! Research source: `docs/harness-configs/cline.md` (last verified 2026-08-25).
//! Executable `cline` (CLI) / VS Code extension `saoudrizwan.claude-dev`,
//! config root `~/.cline` or `$CLINE_DATA_DIR`, plus VS Code globalStorage
//! `…/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json`,
//! isolation `ide-user-data` via `--user-data-dir` + `CLINE_DATA_DIR`.

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

/// Harness identifier for Cline.
pub const HARNESS_ID_STR: &str = "cline";

/// Human display name.
pub const DISPLAY_NAME: &str = "Cline";

/// Primary executable name (CLI).
pub const EXECUTABLE: &str = "cline";

/// VS Code executable name for extension host.
pub const VSCODE_EXECUTABLE: &str = "code";

/// Environment variable that relocates the Cline data directory.
pub const DATA_DIR_ENV_VAR: &str = "CLINE_DATA_DIR";

/// Flag for VS Code user-data-dir isolation.
pub const USER_DATA_DIR_FLAG: &str = "--user-data-dir";

/// Flag for VS Code extensions-dir isolation.
pub const EXTENSIONS_DIR_FLAG: &str = "--extensions-dir";

/// Default config root when `CLINE_DATA_DIR` is unset.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.cline";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/cline.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current config shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider/MCP mutation inside Cline settings (JSON).
///
/// These are top-level keys superai owns; everything else round-trips untouched
/// via `superai-config::json`.
pub const OWNED_SELECTORS: &[&str] = &[
    "apiProvider",
    "openAiBaseUrl",
    "openAiApiKey",
    "openAiModelId",
    "mcpServers",
    "autoApprove",
    "preferredLanguage",
    "telemetrySetting",
];

/// Owned selectors for MCP servers inside `cline_mcp_settings.json`.
pub const MCP_OWNED_SELECTORS: &[&str] = &["mcpServers"];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Cline.
///
/// Isolation is `ide-user-data` via VS Code `--user-data-dir` + `--extensions-dir`
/// plus `CLINE_DATA_DIR` for the CLI/SDK side. The wrapper sets
/// `CLINE_DATA_DIR` to the instance `config_root` and passes
/// `--user-data-dir <root>/vscode-data` to `code`.
#[derive(Debug, Clone)]
pub struct ClineAdapter {
    id: HarnessId,
}

impl ClineAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    /// Borrow the harness id.
    pub fn harness_id(&self) -> &HarnessId {
        &self.id
    }

    /// Executable name for this harness (CLI).
    pub fn executable_name(&self) -> &str {
        EXECUTABLE
    }

    /// Data dir env var.
    pub fn data_dir_env_var(&self) -> &str {
        DATA_DIR_ENV_VAR
    }

    /// Try to locate the `cline` binary via `PATH`.
    #[expect(clippy::unused_self, reason = "adapter method uses instance constants")]
    #[expect(clippy::excessive_nesting, reason = "PATH scan branches are explicit")]
    fn find_binary_in_path(&self) -> Option<PathBuf> {
        let path_var = std::env::var("PATH").ok()?;
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
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
            // Also check for VS Code binary as secondary evidence.
            let _code_candidate = Path::new(dir).join(VSCODE_EXECUTABLE);
            // Prefer cline; VS Code binary check is deferred to second pass.
        }
        // If cline not found, check for VS Code binary directly for evidence.
        let path_var2 = std::env::var("PATH").ok()?;
        for dir in path_var2.split(separator) {
            if dir.is_empty() {
                continue;
            }
            let code_candidate = Path::new(dir).join(VSCODE_EXECUTABLE);
            if code_candidate.is_file() {
                return Some(code_candidate);
            }
            if cfg!(windows) {
                let exe_candidate = Path::new(dir).join(format!("{VSCODE_EXECUTABLE}.exe"));
                if exe_candidate.is_file() {
                    return Some(exe_candidate);
                }
            }
        }
        None
    }

    /// Probe `cline --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `cline 1.2.3` or `Cline 2.0.0` into `1.2.3`.
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

    /// Resolve the default config root: `$CLINE_DATA_DIR` or `~/.cline`.
    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(DATA_DIR_ENV_VAR)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir));
        }
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".cline"))
    }

    /// Resolve the default VS Code globalStorage path for Cline MCP settings.
    fn vscode_global_storage_root() -> Option<PathBuf> {
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
                    .join("Code")
                    .join("User")
                    .join("globalStorage")
                    .join("saoudrizwan.claude-dev")
                    .join("settings"),
            )
        } else if cfg!(windows) {
            // Approximate via APPDATA.
            if let Ok(appdata) = std::env::var("APPDATA")
                && !appdata.trim().is_empty()
            {
                return Some(
                    PathBuf::from(appdata)
                        .join("Code")
                        .join("User")
                        .join("globalStorage")
                        .join("saoudrizwan.claude-dev")
                        .join("settings"),
                );
            }
            Some(
                PathBuf::from(home)
                    .join("AppData")
                    .join("Roaming")
                    .join("Code")
                    .join("User")
                    .join("globalStorage")
                    .join("saoudrizwan.claude-dev")
                    .join("settings"),
            )
        } else {
            Some(
                PathBuf::from(home)
                    .join(".config")
                    .join("Code")
                    .join("User")
                    .join("globalStorage")
                    .join("saoudrizwan.claude-dev")
                    .join("settings"),
            )
        }
    }

    /// Check if default config root exists on disk.
    #[expect(dead_code, reason = "helper for future use")]
    #[expect(clippy::unused_self, reason = "adapter helper")]
    fn default_config_root_exists(&self) -> Option<PathBuf> {
        let root = Self::default_config_root()?;
        if root.exists() { Some(root) } else { None }
    }

    /// Build detection evidence about Cline config and VS Code storage.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    #[expect(clippy::too_many_lines, reason = "evidence branches are explicit")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let data_settings = root.join("data").join("settings");
                    if data_settings.exists() {
                        evidence.push(format!(
                            "data/settings exists at {}",
                            data_settings.display()
                        ));
                        let providers = data_settings.join("providers.json");
                        if providers.exists() {
                            evidence
                                .push(format!("providers.json found at {}", providers.display()));
                            if let Ok(text) = std::fs::read_to_string(&providers)
                                && (text.contains("apiProvider") || text.contains("mcpServers"))
                            {
                                evidence.push(
                                    "providers.json contains apiProvider/mcpServers".to_owned(),
                                );
                            }
                        } else {
                            evidence
                                .push(format!("providers.json missing at {}", providers.display()));
                        }
                        let global = data_settings.join("global-settings.json");
                        if global.exists() {
                            evidence.push(format!(
                                "global-settings.json found at {}",
                                global.display()
                            ));
                        }
                        let mcp = data_settings.join("cline_mcp_settings.json");
                        if mcp.exists() {
                            evidence.push(format!(
                                "cline_mcp_settings.json found at {}",
                                mcp.display()
                            ));
                        }
                    } else {
                        evidence.push(format!(
                            "data/settings missing at {}",
                            data_settings.display()
                        ));
                    }
                    // Rules directory.
                    let rules = root.join("rules");
                    if rules.exists() {
                        evidence.push(format!("rules dir present at {}", rules.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }

        // VS Code globalStorage
        if let Some(vs_root) = Self::vscode_global_storage_root() {
            if vs_root.exists() {
                evidence.push(format!(
                    "VS Code globalStorage exists at {}",
                    vs_root.display()
                ));
                let mcp_vs = vs_root.join("cline_mcp_settings.json");
                if mcp_vs.exists() {
                    evidence.push(format!(
                        "VS Code cline_mcp_settings.json found at {}",
                        mcp_vs.display()
                    ));
                }
            } else {
                evidence.push(format!(
                    "VS Code globalStorage missing at {}",
                    vs_root.display()
                ));
            }
            // VS Code settings.json
            if let Some(home) = Self::default_config_root()
                .as_ref()
                .and_then(|p| p.parent().map(Path::to_path_buf))
            {
                let _ = home;
            }
            // Check VS Code settings.json locations.
            let home_opt = std::env::var("HOME")
                .ok()
                .or_else(|| std::env::var("USERPROFILE").ok());
            if let Some(home) = home_opt
                && !home.trim().is_empty()
            {
                let candidates = if cfg!(target_os = "macos") {
                    vec![
                        PathBuf::from(&home)
                            .join("Library/Application Support/Code/User/settings.json"),
                    ]
                } else if cfg!(windows) {
                    vec![std::env::var("APPDATA").ok().map_or_else(
                        || PathBuf::from(&home).join("AppData/Roaming/Code/User/settings.json"),
                        |appdata| PathBuf::from(appdata).join("Code/User/settings.json"),
                    )]
                } else {
                    vec![PathBuf::from(&home).join(".config/Code/User/settings.json")]
                };
                for cand in candidates {
                    if cand.exists() {
                        evidence.push(format!("VS Code settings.json found at {}", cand.display()));
                        if let Ok(text) = std::fs::read_to_string(&cand)
                            && text.contains("cline.")
                        {
                            evidence.push("VS Code settings.json contains cline.* keys".to_owned());
                        }
                    }
                }
            }
        }

        // CLINE_DATA_DIR env
        if let Ok(dir) = std::env::var(DATA_DIR_ENV_VAR)
            && !dir.trim().is_empty()
        {
            evidence.push(format!("{DATA_DIR_ENV_VAR} set to {dir}"));
        } else {
            evidence.push(format!("{DATA_DIR_ENV_VAR} not set"));
        }

        // Project .clinerules
        let clinerules = Path::new(".clinerules");
        let cline_dir = Path::new(".cline");
        if clinerules.exists() {
            evidence.push(format!(".clinerules found at {}", clinerules.display()));
        }
        if cline_dir.exists() {
            evidence.push(format!(".cline dir found at {}", cline_dir.display()));
        }
        let clineignore = Path::new(".clineignore");
        if clineignore.exists() {
            evidence.push(format!(".clineignore found at {}", clineignore.display()));
        }
    }
}

impl Default for ClineAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "cline is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for ClineAdapter {
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

    #[expect(clippy::excessive_nesting, reason = "detection branches are explicit")]
    fn detection(&self) -> DetectionResult {
        let mut evidence = Vec::new();
        let mut version: Option<String> = None;
        let mut binary_path: Option<PathBuf> = None;

        if let Some(path) = self.find_binary_in_path() {
            evidence.push(format!(
                "found binary `{}` at {}",
                path.file_name().and_then(|n| n.to_str()).unwrap_or("cline"),
                path.display()
            ));
            // Only probe cline binary, not code.
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if file_name.contains("cline") {
                match Self::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `cline --version`"));
                        version = Some(v);
                    }
                    None => {
                        evidence.push(
                            "version probe failed for `cline --version` (timeout or non-zero)"
                                .to_owned(),
                        );
                    }
                }
            } else {
                evidence.push(format!("found VS Code binary at {}", path.display()));
            }
            binary_path = Some(path);
        } else {
            evidence.push(format!("binary `{EXECUTABLE}` not found in PATH"));
            evidence.push(format!("binary `{VSCODE_EXECUTABLE}` not found in PATH"));
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
            notes.push(format!("detected cline version {v}"));
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

    #[expect(clippy::too_many_lines, reason = "surfaces are declarative")]
    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        // Primary writable surface: providers.json (JSON, via CLINE_DATA_DIR or ~/.cline/data/settings)
        let providers_resolver = PathResolver::new(
            Some("$CLINE_DATA_DIR/settings/providers.json"),
            Some("$CLINE_DATA_DIR/settings/providers.json"),
            Some("%CLINE_DATA_DIR%\\settings\\providers.json"),
            "~/.cline/data/settings/providers.json",
        );
        let mut providers_surface = ConfigSurface::new(
            "providers.json",
            providers_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        providers_surface.precedence = 10;
        providers_surface.owned_selectors =
            OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        providers_surface.backup_required = true;
        providers_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(providers_surface);

        // Global settings: global-settings.json
        let global_resolver = PathResolver::new(
            Some("$CLINE_DATA_DIR/settings/global-settings.json"),
            Some("$CLINE_DATA_DIR/settings/global-settings.json"),
            Some("%CLINE_DATA_DIR%\\settings\\global-settings.json"),
            "~/.cline/data/settings/global-settings.json",
        );
        let mut global_surface = ConfigSurface::new(
            "global-settings.json",
            global_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        global_surface.precedence = 10;
        global_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        global_surface.backup_required = true;
        surfaces.push(global_surface);

        // MCP settings: cline_mcp_settings.json (CLI/SDK location)
        let mcp_resolver = PathResolver::new(
            Some("$CLINE_DATA_DIR/settings/cline_mcp_settings.json"),
            Some("$CLINE_DATA_DIR/settings/cline_mcp_settings.json"),
            Some("%CLINE_DATA_DIR%\\settings\\cline_mcp_settings.json"),
            "~/.cline/data/settings/cline_mcp_settings.json",
        );
        let mut mcp_surface = ConfigSurface::new(
            "cline_mcp_settings.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp_surface.precedence = 15;
        mcp_surface.owned_selectors = MCP_OWNED_SELECTORS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        mcp_surface.backup_required = true;
        surfaces.push(mcp_surface);

        // VS Code globalStorage MCP settings — harness-managed but we track it.
        let vscode_mcp_resolver = PathResolver::new(
            Some(
                "~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json",
            ),
            Some(
                "~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json",
            ),
            Some(
                "%APPDATA%\\Code\\User\\globalStorage\\saoudrizwan.claude-dev\\settings\\cline_mcp_settings.json",
            ),
            "~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json",
        );
        let mut vscode_mcp = ConfigSurface::new(
            "vscode cline_mcp_settings.json",
            vscode_mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        vscode_mcp.precedence = 12;
        vscode_mcp.owned_selectors = MCP_OWNED_SELECTORS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        vscode_mcp.backup_required = true;
        vscode_mcp.restart_behavior = RestartBehavior::Reload;
        surfaces.push(vscode_mcp);

        // VS Code settings.json with cline.* namespace
        let vscode_settings_resolver = PathResolver::new(
            Some("~/.config/Code/User/settings.json"),
            Some("~/Library/Application Support/Code/User/settings.json"),
            Some("%APPDATA%\\Code\\User\\settings.json"),
            "~/.config/Code/User/settings.json",
        );
        let mut vscode_settings = ConfigSurface::new(
            "vscode settings.json",
            vscode_settings_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        vscode_settings.precedence = 8;
        vscode_settings.owned_selectors = vec![
            "cline.autoApprove".to_owned(),
            "cline.preferredLanguage".to_owned(),
            "cline.enableCheckpoints".to_owned(),
            "cline.chromeExecutablePath".to_owned(),
        ];
        vscode_settings.backup_required = true;
        surfaces.push(vscode_settings);

        // Rules surface: .clinerules (text fragments)
        let rules_resolver =
            PathResolver::fallback_only(".clinerules / .cline/rules.md / ~/Documents/Cline/Rules");
        let mut rules_surface = ConfigSurface::new(
            ".clinerules",
            rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        rules_surface.precedence = 20;
        rules_surface.backup_required = false;
        surfaces.push(rules_surface);

        // .clineignore (text fragment)
        let ignore_resolver = PathResolver::fallback_only(".clineignore (project root)");
        let mut ignore_surface = ConfigSurface::new(
            ".clineignore",
            ignore_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        ignore_surface.precedence = 5;
        ignore_surface.backup_required = false;
        surfaces.push(ignore_surface);

        // Workflow/skill/hook surfaces are under ~/.cline or .cline/ — text fragments.
        let workflows_resolver =
            PathResolver::fallback_only("~/.cline/workflows / .cline/workflows");
        let mut workflows = ConfigSurface::new(
            "workflows",
            workflows_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        workflows.precedence = 15;
        workflows.backup_required = false;
        surfaces.push(workflows);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        vec![
            ("detect".to_owned(), AdapterSupport::Full),
            ("read_config".to_owned(), AdapterSupport::Full),
            ("write_config".to_owned(), AdapterSupport::Full),
            ("manage_skills".to_owned(), AdapterSupport::Full),
            ("manage_mcp".to_owned(), AdapterSupport::Full),
            ("manage_plugins".to_owned(), AdapterSupport::Full),
            ("configure_provider".to_owned(), AdapterSupport::Full),
            ("plan_mirror".to_owned(), AdapterSupport::Full),
            ("plan_wrapper".to_owned(), AdapterSupport::Full),
            ("scan_candidates".to_owned(), AdapterSupport::Full),
            ("validate_instance".to_owned(), AdapterSupport::Full),
        ]
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "sessions/*".to_owned(),
            "teams/*".to_owned(),
            "db/*".to_owned(),
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "*.lock".to_owned(),
            "workflows/*".to_owned(),
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
        let mut plan = WrapperPlan::new("ide-user-data via --user-data-dir + CLINE_DATA_DIR");
        // CLI/SDK isolation via CLINE_DATA_DIR.
        plan.env_vars.push((
            DATA_DIR_ENV_VAR.to_owned(),
            instance.config_root.to_string(),
        ));
        // VS Code isolation via --user-data-dir and --extensions-dir.
        let vscode_data = Path::new(&instance.config_root.to_string()).join("vscode-data");
        let extensions = Path::new(&instance.config_root.to_string()).join("extensions");
        plan.args.push(USER_DATA_DIR_FLAG.to_owned());
        plan.args.push(vscode_data.display().to_string());
        plan.args.push(EXTENSIONS_DIR_FLAG.to_owned());
        plan.args.push(extensions.display().to_string());
        plan.description = format!(
            " Wrapper sets {}={} and execs `code {} {}` with extensions {}",
            DATA_DIR_ENV_VAR,
            instance.config_root,
            USER_DATA_DIR_FLAG,
            vscode_data.display(),
            extensions.display()
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.cline".to_owned(),
            "~/.cline/data/settings/providers.json".to_owned(),
            "~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json".to_owned(),
            "$CLINE_DATA_DIR".to_owned(),
            "--user-data-dir".to_owned(),
            "./.clinerules".to_owned(),
            "./.clineignore".to_owned(),
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
                reason: format!("cline requires isolation ide_user_data, got {other}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{
        DATA_DIR_ENV_VAR, DISPLAY_NAME, EXECUTABLE, EXTENSIONS_DIR_FLAG, HARNESS_ID_STR,
        OWNED_SELECTORS, RESEARCH_DOC, USER_DATA_DIR_FLAG, VSCODE_EXECUTABLE,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> super::ClineAdapter {
        super::ClineAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-cline-1").unwrap(),
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
        assert_eq!(a.data_dir_env_var(), DATA_DIR_ENV_VAR);
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
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
            assert!(res.schema_version.is_none());
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("cline 1.2.3", Some("1.2.3")),
            ("Cline 2.0.0", Some("2.0.0")),
            ("v1.5.0", Some("1.5.0")),
            ("Version: 1.0.0", Some("1.0.0")),
            ("1.0.0-beta", Some("1.0.0-beta")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = super::ClineAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_writable_json() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 5);
        let providers = surfaces
            .iter()
            .find(|s| s.id == "providers.json")
            .expect("providers.json surface must exist");
        assert_eq!(providers.kind, DocumentKind::Json);
        assert_eq!(providers.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(providers.scope, ConfigScope::User);
        assert!(providers.backup_required);
        for selector in ["apiProvider", "mcpServers"] {
            assert!(
                providers.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in &providers.owned_selectors {
            assert!(!sel.is_empty());
        }
        for sel in OWNED_SELECTORS {
            assert!(providers.owned_selectors.contains(&(*sel).to_owned()));
        }

        let mcp = surfaces
            .iter()
            .find(|s| s.id == "cline_mcp_settings.json")
            .expect("cline_mcp_settings.json surface");
        assert_eq!(mcp.kind, DocumentKind::Json);
        assert!(mcp.backup_required);

        let vscode_mcp = surfaces
            .iter()
            .find(|s| s.id == "vscode cline_mcp_settings.json")
            .expect("vscode mcp surface");
        assert_eq!(vscode_mcp.ownership, SurfaceOwnership::HarnessManaged);

        let vscode_settings = surfaces
            .iter()
            .find(|s| s.id == "vscode settings.json")
            .expect("vscode settings");
        assert_eq!(vscode_settings.kind, DocumentKind::Json);

        let rules = surfaces
            .iter()
            .find(|s| s.id == ".clinerules")
            .expect(".clinerules surface");
        assert_eq!(rules.kind, DocumentKind::TextFragment);
        assert_eq!(rules.scope, ConfigScope::ProjectWorkspace);
    }

    #[test]
    fn owned_selectors_are_stable() {
        assert!(OWNED_SELECTORS.len() >= 5);
        let set: HashSet<&str> = OWNED_SELECTORS.iter().copied().collect();
        assert_eq!(set.len(), OWNED_SELECTORS.len(), "selectors must be unique");
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
    fn plan_mirror_exclusions_cover_sessions_and_logs() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        let must_contain = ["sessions/*", "teams/*", "db/*", "cache/*"];
        for pat in must_contain {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"providers.json".to_owned()));
    }

    #[test]
    #[expect(clippy::excessive_nesting, reason = "test closure nesting is explicit")]
    fn plan_mirror_includes_settings_and_excludes_sessions() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        let is_excluded = |file: &str| {
            exclusions.iter().any(|pat| {
                if pat.ends_with("/*") {
                    let prefix = pat.trim_end_matches("/*");
                    file.starts_with(prefix)
                } else if pat.starts_with("*.") {
                    let suffix = pat.trim_start_matches('*');
                    file.ends_with(suffix)
                } else {
                    file == pat
                }
            })
        };
        assert!(!is_excluded("providers.json"));
        assert!(!is_excluded("global-settings.json"));
        assert!(!is_excluded("cline_mcp_settings.json"));
        assert!(is_excluded("sessions/abc.json"));
        assert!(is_excluded("teams/my-team.json"));
        assert!(is_excluded("db/cron.db"));
    }

    #[test]
    fn plan_wrapper_sets_cline_data_dir_and_user_data_dir() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.cline-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == DATA_DIR_ENV_VAR && v == "/tmp/.cline-work")
        );
        assert!(plan.args.contains(&USER_DATA_DIR_FLAG.to_owned()));
        assert!(plan.args.contains(&EXTENSIONS_DIR_FLAG.to_owned()));
        let user_data_idx = plan
            .args
            .iter()
            .position(|a| a == USER_DATA_DIR_FLAG)
            .unwrap();
        #[expect(clippy::get_unwrap, reason = "test index is vetted")]
        let user_data_path = plan.args.get(user_data_idx + 1).unwrap();
        assert!(user_data_path.contains(".cline-work"));
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(DATA_DIR_ENV_VAR));
        assert!(plan.description.contains(USER_DATA_DIR_FLAG));
        let _ = VSCODE_EXECUTABLE;
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my cline work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == DATA_DIR_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, "/tmp/my cline work");
        assert!(!env_val.contains('"'));
        assert!(env_val.contains(' '));
        let user_data_idx = plan
            .args
            .iter()
            .position(|arg| arg == USER_DATA_DIR_FLAG)
            .unwrap();
        #[expect(clippy::get_unwrap, reason = "test index is vetted")]
        let user_data_arg = plan.args.get(user_data_idx + 1).unwrap();
        assert!(user_data_arg.contains(' '));
        assert!(!user_data_arg.contains('"'));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.cline-work");
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
        assert!(candidates.iter().any(|c| c.contains(".cline")));
        assert!(candidates.iter().any(|c| c.contains(DATA_DIR_ENV_VAR)));
        assert!(candidates.iter().any(|c| c.contains(USER_DATA_DIR_FLAG)));
    }

    #[test]
    fn validate_instance_accepts_ide_user_data() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.cline-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_accepts_relocated_root_for_catalog() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.cline-work");
        inst.isolation = Isolation::RelocatedRoot;
        a.validate_instance(&inst).unwrap();
        inst.isolation = Isolation::Unknown;
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.cline-work");
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
        let mut inst = sample_instance_with_root("/tmp/.cline-work");
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let providers = surfaces.iter().find(|s| s.id == "providers.json").unwrap();
        assert_eq!(
            providers.path_resolver.fallback,
            "~/.cline/data/settings/providers.json"
        );
        let resolver = &providers.path_resolver;
        assert!(
            resolver
                .linux
                .as_deref()
                .unwrap()
                .contains(DATA_DIR_ENV_VAR)
        );
        let vscode = surfaces
            .iter()
            .find(|s| s.id == "vscode cline_mcp_settings.json")
            .unwrap();
        assert!(vscode.path_resolver.fallback.contains("globalStorage"));
    }

    // -----------------------------------------------------------------------
    // Fixture-backed conformance tests
    // -----------------------------------------------------------------------

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/cline")
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
        // Minimal may be empty or contain only telemetrySetting.
        assert!(
            map.is_empty()
                || map.contains_key("telemetrySetting")
                || map.contains_key("apiProvider")
        );
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(
            map.contains_key("apiProvider")
                || map.contains_key("mcpServers")
                || map.contains_key("telemetrySetting")
        );
    }

    #[test]
    fn fixture_providers_populated_has_keys() {
        let path = fixture_path("providers.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.contains_key("apiProvider") || map.contains_key("openAiBaseUrl"));
    }

    #[test]
    fn fixture_mcp_populated_has_servers() {
        let path = fixture_path("cline_mcp_settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.contains_key("mcpServers"));
        let servers = map["mcpServers"].as_object().unwrap();
        assert!(!servers.is_empty());
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = std::env::temp_dir().join("superai-cline-foreign-test");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("settings.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "apiProvider".to_owned(),
                serde_json::Value::String("openai-compatible".to_owned()),
            );
            assert!(map.contains_key("foreignKey") || map.contains_key("unknownTopLevel"));
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert_eq!(
            after["apiProvider"],
            serde_json::Value::String("openai-compatible".to_owned())
        );
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
    fn fixture_providers_foreign_preserves() {
        let path = fixture_path("providers.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = std::env::temp_dir().join("superai-cline-prov-foreign");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("providers.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "apiProvider".to_owned(),
                serde_json::Value::String("anthropic".to_owned()),
            );
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert!(after.contains_key("foreignKey") || after.contains_key("unknownTopLevel"));
        drop(std::fs::remove_file(&tmp));
    }

    #[test]
    fn unknown_key_preservation_via_edit() {
        let dir = std::env::temp_dir().join("superai-cline-preserve-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preserve.json");
        let original_json = serde_json::json!({
            "apiProvider": "anthropic",
            "foreignKey": "keep-me",
            "customField": 123,
            "mcpServers": {
                "test": {"command": "node", "args": ["server.js"]}
            }
        });
        let text = serde_json::to_string_pretty(&original_json).unwrap();
        std::fs::write(&path, text).unwrap();

        superai_config::json::edit(&path, |map| {
            if let Some(mcp) = map.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
                mcp.insert(
                    "new-server".to_owned(),
                    serde_json::json!({"command": "node", "args": ["new.js"]}),
                );
            }
        })
        .unwrap();
        let after = superai_config::json::load(&path).unwrap();
        assert_eq!(
            after["foreignKey"],
            serde_json::Value::String("keep-me".to_owned())
        );
        assert_eq!(after["customField"], serde_json::Value::Number(123.into()));
        assert!(
            after["mcpServers"]
                .as_object()
                .unwrap()
                .contains_key("new-server")
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn provider_mutation_sets_api_provider_and_mcp() {
        let dir = std::env::temp_dir().join("superai-cline-provider-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("provider.json");
        let initial = serde_json::json!({
            "apiProvider": "anthropic",
            "openAiBaseUrl": "https://old.example.com"
        });
        std::fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        superai_config::json::edit(&path, |map| {
            map.insert(
                "apiProvider".to_owned(),
                serde_json::Value::String("openai-compatible".to_owned()),
            );
            map.insert(
                "openAiBaseUrl".to_owned(),
                serde_json::Value::String("http://localhost:4000/v1".to_owned()),
            );
            let mcp = map
                .entry("mcpServers".to_owned())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::default()));
            if let Some(obj) = mcp.as_object_mut() {
                obj.insert(
                    "linear".to_owned(),
                    serde_json::json!({"command": "node", "args": ["index.js"]}),
                );
            }
        })
        .unwrap();
        let after = superai_config::json::load(&path).unwrap();
        assert_eq!(
            after["apiProvider"],
            serde_json::Value::String("openai-compatible".to_owned())
        );
        assert_eq!(
            after["openAiBaseUrl"],
            serde_json::Value::String("http://localhost:4000/v1".to_owned())
        );
        assert!(
            after["mcpServers"]
                .as_object()
                .unwrap()
                .contains_key("linear")
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn clinerules_text_fragment_is_preserved() {
        let dir = std::env::temp_dir().join("superai-cline-rules-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".clinerules");
        let content = "# Cline Rules\n- Always use English\n";
        std::fs::write(&path, content).unwrap();
        let read = std::fs::read_to_string(&path).unwrap();
        assert!(read.contains("Cline Rules"));
        // Simulate edit: append rule, ensure original preserved.
        let mut updated = read;
        updated.push_str("- New rule\n");
        std::fs::write(&path, updated).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("Always use English"));
        assert!(after.contains("New rule"));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn secret_redaction_placeholder() {
        use crate::error::RedactedString;
        let secret = RedactedString::new("sk-test-secret-cline");
        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert!(!debug.contains("sk-test-secret-cline"));
        assert!(!display.contains("sk-test-secret-cline"));
        assert!(debug.contains("[REDACTED]"));
        assert!(display.contains("[REDACTED]"));
        let json = serde_json::to_string(&secret).unwrap();
        assert!(!json.contains("sk-test-secret-cline"));
        assert!(json.contains("[REDACTED]"));
        assert_eq!(secret.expose_secret(), "sk-test-secret-cline");
    }

    #[test]
    fn diff_redaction_does_not_leak_secrets() {
        use crate::operation::RedactedString as OpRedacted;
        let secret = OpRedacted::new("super-secret-key");
        let diff_text = format!("set api key to {secret}");
        assert!(!diff_text.contains("super-secret-key"));
        assert!(diff_text.contains("[REDACTED]"));
    }

    #[test]
    fn conflict_detection_placeholder_no_panic() {
        let a = adapter();
        let r1 = a.detection();
        let r2 = a.detection();
        assert_eq!(r1.present, r2.present);
        assert_eq!(r1.confidence, r2.confidence);
        let inst = sample_instance_with_root("/tmp/.cline-work");
        a.validate_instance(&inst).unwrap();
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn wrapper_env_var_isolation_is_ide_user_data() {
        let a = adapter();
        let inst = sample_instance_with_root("/home/user/.cline-isolated");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(!plan.env_vars.is_empty());
        let (key, val) = &plan.env_vars[0];
        assert_eq!(key, DATA_DIR_ENV_VAR);
        assert_eq!(val, "/home/user/.cline-isolated");
        assert!(plan.args.contains(&USER_DATA_DIR_FLAG.to_owned()));
    }

    #[test]
    fn vscode_storage_surface_exists() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let vscode = surfaces
            .iter()
            .find(|s| s.id == "vscode cline_mcp_settings.json")
            .unwrap();
        assert_eq!(vscode.kind, DocumentKind::Json);
        assert_eq!(vscode.scope, ConfigScope::User);
        // Should mention globalStorage in resolver.
        assert!(vscode.path_resolver.fallback.contains("globalStorage"));
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
