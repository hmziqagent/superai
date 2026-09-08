//! `WorkBuddy` / `CodeBuddy` CLI (cbc) adapter — relocated-root via
//! `CODEBUDDY_CONFIG_DIR` over the shared `~/.codebuddy` JSON tree.
//!
//! Research source: `docs/harness-configs/workbuddy.md` (verified 2026-09-08;
//! catalog freshness recorded as of 2026-09-01). `WorkBuddy` is Tencent's
//! desktop AI agent; the only documented programmatic surface is the shared
//! `CodeBuddy` CLI (`cbc`, npm `@tencent-ai/codebuddy-code`). Primary writable
//! surfaces: `models.json`, `settings.json`, `.mcp.json` under the config
//! root. The desktop app itself is GUI-only and is documented, never
//! mutated. Isolation is `relocated-root`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::adapter::{
    ADAPTER_REVISION, Adapter, AdapterCapabilityDecl, Arch, ConfigScope, ConfigSurface,
    DetectionConfidence, DetectionResult, DocumentKind, McpAdapterDecl, Os, PathResolver, Platform,
    ProductStatus, RestartBehavior, RootShape, SurfaceOwnership, SurfaceSchema, VersionResolution,
    WrapperPlan,
};
use crate::capability::{Capability, Support};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, InstallPresence, Isolation};
use superai_config::document::ValueType;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Harness identifier for `WorkBuddy` / `CodeBuddy` CLI.
pub const HARNESS_ID_STR: &str = "workbuddy";

/// Human display name.
pub const DISPLAY_NAME: &str = "WorkBuddy / CodeBuddy CLI (cbc)";

/// Primary executable name (the headless CLI binary).
pub const EXECUTABLE: &str = "cbc";

/// Alternate executable name shipped by the same npm package.
pub const ALT_EXECUTABLE: &str = "codebuddy";

/// npm package that installs both binaries (zero dependencies).
pub const NPM_PACKAGE: &str = "@tencent-ai/codebuddy-code";

/// Environment variable that relocates the whole config root.
pub const CONFIG_ENV_VAR: &str = "CODEBUDDY_CONFIG_DIR";

/// Default config root when `CODEBUDDY_CONFIG_DIR` is unset.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.codebuddy";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/workbuddy.md";

/// Catalog-recorded verification date (kept at the ledger's recheck date so
/// the freshness window stays honest; the research doc itself was verified
/// 2026-09-08).
pub const LAST_VERIFIED: &str = "2026-09-01";

/// Schema version for the current config shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// cbc version (major, minor, patch) where the autocompact window moved from
/// `models.json` `maxInputTokens` to the `CODEBUDDY_AUTO_COMPACT_WINDOW` env
/// var (workbuddy.md §4, workbuddy-bench presets).
pub const AUTO_COMPACT_WINDOW_MIN_VERSION: (u64, u64, u64) = (2, 103, 4);

/// Owned selectors for `models.json` mutation.
pub const MODELS_OWNED_SELECTORS: &[&str] = &["models", "availableModels"];

/// Owned selectors for `settings.json` mutation (verified-only keys; the full
/// schema is not published, so unmodelled keys are preserved verbatim).
pub const SETTINGS_OWNED_SELECTORS: &[&str] = &[
    "permissions",
    "alwaysThinkingEnabled",
    "autoCompactEnabled",
    "autoUpdates",
    "includeCoAuthoredBy",
    "promptSuggestionEnabled",
    "cleanupPeriodDays",
    "enableAllProjectMcpServers",
    "enabledMcpjsonServers",
    "apiKeyHelper",
    "env",
    "endpoint",
];

/// Environment variables the harness documents (references only; values are
/// never stored by superai).
pub const KNOWN_ENV_VARS: &[&str] = &[
    "CODEBUDDY_AUTH_TOKEN",
    "CODEBUDDY_API_KEY",
    "CODEBUDDY_BASE_URL",
    "CODEBUDDY_CONFIG_DIR",
    "CODEBUDDY_INTERNET_ENVIRONMENT",
    "CODEBUDDY_AUTO_COMPACT_WINDOW",
    "CODEBUDDY_AUTOCOMPACT_PCT_OVERRIDE",
    "CODEBUDDY_IS_SANDBOX",
    "CBC_BASE_URL",
    "CBC_API_KEY",
    "DISABLE_AUTOUPDATER",
    "MAX_MCP_OUTPUT_TOKENS",
];

/// Auth env vars in documented priority order (bearer token first, then the
/// individual API key; the settings `apiKeyHelper` sits between them).
pub const AUTH_ENV_VARS: &[&str] = &["CODEBUDDY_AUTH_TOKEN", "CODEBUDDY_API_KEY"];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for `WorkBuddy` / `CodeBuddy` CLI (cbc).
///
/// Isolation is `relocated-root` via `CODEBUDDY_CONFIG_DIR`: the wrapper
/// points the whole `~/.codebuddy` tree at the instance `config_root` and
/// execs `cbc`.
#[derive(Debug, Clone)]
pub struct WorkBuddyAdapter {
    id: HarnessId,
}

impl WorkBuddyAdapter {
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

    /// Config relocation env var.
    pub fn config_env_var(&self) -> &str {
        CONFIG_ENV_VAR
    }

    /// Try to locate `cbc` (then `codebuddy`) via `PATH`.
    #[expect(clippy::unused_self, reason = "adapter method uses instance constants")]
    #[expect(clippy::excessive_nesting, reason = "PATH scan branches are explicit")]
    fn find_binary_in_path(&self) -> Option<PathBuf> {
        let path_var = std::env::var("PATH").ok()?;
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            if dir.is_empty() {
                continue;
            }
            for executable in [EXECUTABLE, ALT_EXECUTABLE] {
                let candidate = Path::new(dir).join(executable);
                if candidate.is_file() {
                    return Some(candidate);
                }
                if cfg!(windows) {
                    let exe_candidate = Path::new(dir).join(format!("{executable}.exe"));
                    if exe_candidate.is_file() {
                        return Some(exe_candidate);
                    }
                }
            }
        }
        None
    }

    /// Run `binary` with `args` and a timeout, returning combined
    /// stdout/stderr.
    fn run_with_timeout(binary: &Path, args: &[&str]) -> Option<String> {
        let binary_owned = binary.to_path_buf();
        let args_owned: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut command = Command::new(&binary_owned);
            for arg in &args_owned {
                command.arg(arg);
            }
            let output = command
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
        if stdout.trim().is_empty() {
            Some(stderr.into_owned())
        } else if stderr.trim().is_empty() {
            Some(stdout.into_owned())
        } else {
            Some(format!("{stdout} {stderr}"))
        }
    }

    /// Probe the binary for a version string. The exact `cbc --version`
    /// output format is UNVERIFIED (workbuddy.md §7), so this is best-effort
    /// and detection falls back to npm package metadata.
    fn probe_binary_version(binary: &Path) -> Option<String> {
        Self::parse_version_output(&Self::run_with_timeout(binary, &["--version"])?)
    }

    /// Probe npm global metadata for the installed package version (the
    /// preferred version source per HAD-02 because the CLI flag format is
    /// unverified). The package is installed with `npm i -g`, so only the
    /// global tree can see it.
    fn probe_npm_version() -> Option<String> {
        let output = Self::run_with_timeout(Path::new("npm"), &["ls", "-g"])?;
        Self::harvest_npm_version(&output)
    }

    /// Harvest the first `@tencent-ai/codebuddy-code@<version>` mention from
    /// `npm ls -g` tree output.
    fn harvest_npm_version(output: &str) -> Option<String> {
        let needle = format!("{NPM_PACKAGE}@");
        let line = output.lines().find(|l| l.contains(&needle))?;
        let at = line.find(&needle)?;
        let rest = line.get(at + needle.len()..)?;
        let version: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
            .collect();
        if version.contains('.') {
            Some(version)
        } else {
            None
        }
    }

    /// Parse version output like `cbc 2.147.0` or
    /// `@tencent-ai/codebuddy-code 2.147.0` into `2.147.0`.
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

    /// Parse a `major.minor.patch` triple from a version string.
    fn parse_version_triple(version: &str) -> Option<(u64, u64, u64)> {
        let mut parts = Vec::with_capacity(3);
        for segment in version.split('.') {
            if parts.len() == 3 {
                return None;
            }
            let digits: String = segment.chars().take_while(char::is_ascii_digit).collect();
            if digits.is_empty() {
                return None;
            }
            parts.push(digits.parse::<u64>().ok()?);
        }
        match parts.as_slice() {
            [major, minor, patch] => Some((*major, *minor, *patch)),
            _ => None,
        }
    }

    /// Whether `version` is at or past the autocompact-window era boundary
    /// (cbc >= 2.103.4). Unparseable versions are conservatively `false`
    /// (the version gate blocks writes independently).
    fn is_auto_compact_window_era(version: &str) -> bool {
        match Self::parse_version_triple(version) {
            Some(triple) => triple >= AUTO_COMPACT_WINDOW_MIN_VERSION,
            None => false,
        }
    }

    /// Era conflict for `models.json` (HAD-05 step 5): cbc >= 2.103.4 moved
    /// the compaction window to the `CODEBUDDY_AUTO_COMPACT_WINDOW` env var;
    /// a string-valued `maxInputTokens` (the workbuddy-bench `${ENV}`
    /// preset carrier) belongs to the legacy < 2.103.4 era and must not be
    /// written against a current install.
    fn models_era_conflict(version: &str, content: &[u8]) -> Option<String> {
        if !Self::is_auto_compact_window_era(version) {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(content).ok()?;
        let entries = value.get("models")?.as_array()?;
        let boundary = format!(
            "{}.{}.{}",
            AUTO_COMPACT_WINDOW_MIN_VERSION.0,
            AUTO_COMPACT_WINDOW_MIN_VERSION.1,
            AUTO_COMPACT_WINDOW_MIN_VERSION.2
        );
        let legacy_carrier = entries.iter().any(|entry| {
            entry
                .get("maxInputTokens")
                .is_some_and(serde_json::Value::is_string)
        });
        legacy_carrier.then(|| {
            format!(
                "cbc {version} >= {boundary} carries the autocompact window via \
                 CODEBUDDY_AUTO_COMPACT_WINDOW; string maxInputTokens in models.json \
                 is the pre-{boundary} preset carrier"
            )
        })
    }

    /// Resolve the default config root: `$CODEBUDDY_CONFIG_DIR` or
    /// `~/.codebuddy`.
    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(CONFIG_ENV_VAR)
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
        Some(PathBuf::from(home).join(".codebuddy"))
    }

    /// Build detection evidence about the config root and its files.
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
                    for file in ["models.json", "settings.json", ".mcp.json"] {
                        let path = root.join(file);
                        if path.exists() {
                            evidence.push(format!("{file} found at {}", path.display()));
                        } else {
                            evidence.push(format!("{file} missing at {}", path.display()));
                        }
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
            evidence.push(format!("{CONFIG_ENV_VAR} not set, using ~/.codebuddy"));
        }
        if Path::new(".codebuddy").exists() {
            evidence.push(".codebuddy directory present in cwd".to_owned());
        }
        if Path::new(".mcp.json").exists() {
            evidence.push("project .mcp.json present in cwd".to_owned());
        }
    }
}

impl Default for WorkBuddyAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "workbuddy is a static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for WorkBuddyAdapter {
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
                let name = path.file_name().map_or_else(
                    || EXECUTABLE.to_owned(),
                    |n| n.to_string_lossy().into_owned(),
                );
                evidence.push(format!("found binary `{name}` at {}", path.display()));
                match Self::probe_binary_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `--version`"));
                        version = Some(v);
                    }
                    None => {
                        evidence.push(
                            "version probe failed for `--version` (format unverified; workbuddy.md §7)"
                                .to_owned(),
                        );
                    }
                }
                binary_path = Some(path);
            }
            None => {
                evidence.push(format!(
                    "binaries `{EXECUTABLE}`/`{ALT_EXECUTABLE}` not found in PATH"
                ));
            }
        }

        if version.is_none() {
            // HAD-02: npm metadata is the preferred version source because
            // the CLI flag format is unverified.
            match Self::probe_npm_version() {
                Some(v) => {
                    evidence.push(format!("version `{v}` via npm metadata for {NPM_PACKAGE}"));
                    version = Some(v);
                }
                None => {
                    evidence.push(format!(
                        "npm metadata for {NPM_PACKAGE} not probeable (npm absent or package not installed)"
                    ));
                }
            }
        }

        self.collect_config_evidence(&mut evidence);

        let present = match (&binary_path, &version) {
            (Some(_), Some(_)) => InstallPresence::Present,
            (Some(_), None) => InstallPresence::UnknownVersion,
            (None, _) => InstallPresence::Absent,
        };

        let confidence = if version.is_some() || present == InstallPresence::Absent {
            DetectionConfidence::High
        } else {
            DetectionConfidence::Medium
        };

        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution(&self) -> VersionResolution {
        let detection = self.detection();
        let Some(v) = detection.version else {
            let mut res = VersionResolution::unknown();
            res.notes = detection.evidence;
            return res;
        };
        let era = if Self::is_auto_compact_window_era(&v) {
            format!(
                "autocompact window via CODEBUDDY_AUTO_COMPACT_WINDOW (cbc >= {}.{}.{}); \
                 legacy string maxInputTokens refused in models.json",
                AUTO_COMPACT_WINDOW_MIN_VERSION.0,
                AUTO_COMPACT_WINDOW_MIN_VERSION.1,
                AUTO_COMPACT_WINDOW_MIN_VERSION.2
            )
        } else {
            "autocompact window via models.json maxInputTokens + \
             CODEBUDDY_AUTOCOMPACT_PCT_OVERRIDE (pre-2.103.4 era)"
                .to_owned()
        };
        let mut res =
            VersionResolution::new(Some(v.clone()), Some(SCHEMA_VERSION_STR.to_owned()), true);
        res.notes = vec![
            format!("detected {DISPLAY_NAME} version {v}"),
            format!("mapped to schema version {SCHEMA_VERSION_STR}"),
            era,
        ];
        res
    }

    #[expect(clippy::too_many_lines, reason = "surfaces are declarative")]
    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        // User-scope model catalog (strict JSON, UTF-8 no BOM).
        let models_resolver = PathResolver::new(
            Some("$CODEBUDDY_CONFIG_DIR/models.json"),
            Some("$CODEBUDDY_CONFIG_DIR/models.json"),
            Some("%CODEBUDDY_CONFIG_DIR%\\models.json"),
            "~/.codebuddy/models.json",
        );
        let mut models = ConfigSurface::new(
            "models.json",
            models_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        models.precedence = 10;
        models.owned_selectors = MODELS_OWNED_SELECTORS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        models.backup_required = true;
        models.restart_behavior = RestartBehavior::Restart;
        surfaces.push(models);

        // User-scope settings (strict JSON; schema only partially published —
        // unmodelled keys must be preserved verbatim on write-back).
        let settings_resolver = PathResolver::new(
            Some("$CODEBUDDY_CONFIG_DIR/settings.json"),
            Some("$CODEBUDDY_CONFIG_DIR/settings.json"),
            Some("%CODEBUDDY_CONFIG_DIR%\\settings.json"),
            "~/.codebuddy/settings.json",
        );
        let mut settings = ConfigSurface::new(
            "settings.json",
            settings_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        settings.precedence = 10;
        settings.owned_selectors = SETTINGS_OWNED_SELECTORS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        settings.backup_required = true;
        settings.restart_behavior = RestartBehavior::Restart;
        surfaces.push(settings);

        // User-scope MCP config. The document format is JSONC-tolerant
        // (comments + trailing commas allowed); canonical writes are strict
        // JSON, which is a valid JSONC subset — same treatment as
        // claude-code's `.mcp.json`.
        let mcp_resolver = PathResolver::new(
            Some("$CODEBUDDY_CONFIG_DIR/.mcp.json"),
            Some("$CODEBUDDY_CONFIG_DIR/.mcp.json"),
            Some("%CODEBUDDY_CONFIG_DIR%\\.mcp.json"),
            "~/.codebuddy/.mcp.json",
        );
        let mut mcp = ConfigSurface::new(
            ".mcp.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp.precedence = 10;
        mcp.owned_selectors = vec!["mcpServers".to_owned()];
        mcp.backup_required = true;
        mcp.restart_behavior = RestartBehavior::Reload;
        surfaces.push(mcp);

        // Project-scope settings (committed) and local settings (gitignored);
        // documented precedence CLI > local > project > user.
        for (id, name, precedence) in [
            ("project.settings.json", ".codebuddy/settings.json", 12u8),
            (
                "project.settings.local.json",
                ".codebuddy/settings.local.json",
                14u8,
            ),
        ] {
            let mut surface = ConfigSurface::new(
                id,
                PathResolver::fallback_only(name),
                DocumentKind::Json,
                ConfigScope::ProjectWorkspace,
                SurfaceOwnership::UserEditable,
            );
            surface.precedence = precedence;
            surface.backup_required = false;
            surfaces.push(surface);
        }

        // Project-scope model overrides (override the user level).
        let mut project_models = ConfigSurface::new(
            "project.models.json",
            PathResolver::fallback_only(".codebuddy/models.json"),
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_models.precedence = 12;
        project_models.backup_required = false;
        surfaces.push(project_models);

        // Project-scope MCP config (recommended `<project>/.mcp.json`).
        let mut project_mcp = ConfigSurface::new(
            "project.mcp.json",
            PathResolver::fallback_only(".mcp.json (project root)"),
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_mcp.precedence = 12;
        project_mcp.owned_selectors = vec!["mcpServers".to_owned()];
        project_mcp.backup_required = false;
        surfaces.push(project_mcp);

        // Deprecated MCP locations (first-existing wins per scope; kept
        // modelled so scans and mirrors see them, never preferred). The
        // `~/.codebuddy/*` pair is user scope; the bare project `mcp.json`
        // is project scope (workbuddy.md §1.1).
        for (id, name, precedence, scope) in [
            (
                "deprecated.user.mcp.json",
                "~/.codebuddy/mcp.json",
                8u8,
                ConfigScope::User,
            ),
            (
                "deprecated.user.codebuddy.json",
                "~/.codebuddy.json",
                7u8,
                ConfigScope::User,
            ),
            (
                "deprecated.project.mcp.json",
                "mcp.json (project root, deprecated)",
                9u8,
                ConfigScope::ProjectWorkspace,
            ),
        ] {
            let mut surface = ConfigSurface::new(
                id,
                PathResolver::fallback_only(name),
                DocumentKind::Json,
                scope,
                SurfaceOwnership::UserEditable,
            );
            surface.precedence = precedence;
            surface.owned_selectors = vec!["mcpServers".to_owned()];
            surface.backup_required = true;
            surfaces.push(surface);
        }

        // Environment surface: documented env vars (auth priority
        // CODEBUDDY_AUTH_TOKEN > settings apiKeyHelper > CODEBUDDY_API_KEY).
        let mut env = ConfigSurface::new(
            "env",
            PathResolver::fallback_only("process environment (CBC_/CODEBUDDY_ vars)"),
            DocumentKind::Env,
            ConfigScope::SessionInline,
            SurfaceOwnership::UserEditable,
        );
        env.precedence = 20;
        env.owned_selectors = KNOWN_ENV_VARS.iter().map(|s| (*s).to_owned()).collect();
        env.backup_required = false;
        surfaces.push(env);

        // Credentials live in the OS keychain/credential manager (written by
        // the interactive CLI): detectable, never writable.
        let mut keychain = ConfigSurface::new(
            "credentials",
            PathResolver::fallback_only("OS keychain / credential manager (cbc auth)"),
            DocumentKind::Keychain,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        keychain.precedence = 0;
        keychain.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(keychain);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        vec![
            ("detect".to_owned(), AdapterSupport::Constrained),
            ("read_config".to_owned(), AdapterSupport::Constrained),
            ("write_config".to_owned(), AdapterSupport::Constrained),
            ("manage_skills".to_owned(), AdapterSupport::Constrained),
            ("manage_mcp".to_owned(), AdapterSupport::Constrained),
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
            "tool-results/*".to_owned(),
            "logs/*".to_owned(),
            "cache/*".to_owned(),
            "tmp/*".to_owned(),
            ".tmp/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
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
        let mut plan = WrapperPlan::new("relocated-root via CODEBUDDY_CONFIG_DIR");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        // No updater races between concurrent instances (workbuddy.md §5).
        plan.env_vars
            .push(("DISABLE_AUTOUPDATER".to_owned(), "1".to_owned()));
        // Hermetic runs read only the user scope: project/local settings in
        // the checkout must not leak into an isolated instance.
        plan.args = vec!["--setting-sources".to_owned(), "user".to_owned()];
        plan.executable = Some(EXECUTABLE.to_owned());
        plan.state_paths = vec![
            format!("{CONFIG_ENV_VAR}={}", instance.config_root),
            "~/.codebuddy (relocated per instance)".to_owned(),
        ];
        plan.auth_prerequisites = AUTH_ENV_VARS
            .iter()
            .map(|name| crate::adapter::AuthPrerequisite::env_var(name))
            .collect();
        plan.isolation_guarantees = vec![
            "whole config tree (models.json, settings.json, .mcp.json, sessions) under CODEBUDDY_CONFIG_DIR"
                .to_owned(),
            "project/local settings ignored via --setting-sources user".to_owned(),
        ];
        plan.shared_state_warnings = vec![
            "OS keychain credentials are global across instances (detectable, never writable)"
                .to_owned(),
            "npm global install of @tencent-ai/codebuddy-code is shared".to_owned(),
            "Tencent subscription/account state is shared".to_owned(),
        ];
        plan.description = format!(
            "Wrapper sets {}={} and execs `{EXECUTABLE}`",
            CONFIG_ENV_VAR, instance.config_root
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.codebuddy".to_owned(),
            "$CODEBUDDY_CONFIG_DIR".to_owned(),
            "~/.codebuddy/mcp.json".to_owned(),
            "~/.codebuddy.json".to_owned(),
            ".codebuddy/settings.json".to_owned(),
            ".codebuddy/settings.local.json".to_owned(),
            ".codebuddy/models.json".to_owned(),
            ".mcp.json".to_owned(),
            "mcp.json".to_owned(),
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
            Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("workbuddy requires isolation relocated_root, got {other}"),
            }),
        }
    }

    fn surface_schema(&self, surface_id: &str) -> Option<SurfaceSchema> {
        // HAD-03 root shape + owned-key semantics per workbuddy.md §1.
        // Rules fire only when the key is present; unmodelled keys are
        // preserved untouched by design (the settings schema is only
        // partially published). Note: models.json token caps are documented
        // as numbers but Tencent's bench preset writes `${ENV}` strings, so
        // no type rule is declared for them — both eras are legal.
        match surface_id {
            "models.json" => Some(
                SurfaceSchema::new()
                    .with_root_shape(RootShape::Object)
                    .with_owned_key("models", ValueType::Array)
                    .with_owned_key("availableModels", ValueType::Array),
            ),
            "settings.json" => Some(
                SurfaceSchema::new()
                    .with_root_shape(RootShape::Object)
                    .with_owned_key("permissions", ValueType::Object)
                    .with_owned_key("env", ValueType::Object)
                    .with_owned_key("apiKeyHelper", ValueType::String)
                    .with_owned_key("endpoint", ValueType::String)
                    .with_owned_key("alwaysThinkingEnabled", ValueType::Boolean)
                    .with_owned_key("autoCompactEnabled", ValueType::Boolean)
                    .with_owned_key("autoUpdates", ValueType::Boolean)
                    .with_owned_key("includeCoAuthoredBy", ValueType::Boolean)
                    .with_owned_key("promptSuggestionEnabled", ValueType::Boolean)
                    .with_owned_key("enableAllProjectMcpServers", ValueType::Boolean)
                    .with_owned_key("enabledMcpjsonServers", ValueType::Array)
                    .with_owned_key("cleanupPeriodDays", ValueType::Number),
            ),
            ".mcp.json" => Some(
                SurfaceSchema::new()
                    .with_root_shape(RootShape::Object)
                    .with_owned_key("mcpServers", ValueType::Object),
            ),
            _ => None,
        }
    }

    fn era_conflict_reason(&self, surface_id: &str, content: &[u8]) -> Option<String> {
        if surface_id != "models.json" {
            return None;
        }
        let version = self.version_resolution().detected_version?;
        Self::models_era_conflict(&version, content)
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        vec![
            crate::adapter::SkillMode::LinkAll,
            crate::adapter::SkillMode::LinkSelected,
            crate::adapter::SkillMode::CopySelected,
        ]
    }

    fn capability_declarations(&self) -> Vec<AdapterCapabilityDecl> {
        vec![
            AdapterCapabilityDecl::new(
                Capability::Mcp,
                Support::Native,
                "MCP client: ~/.codebuddy/.mcp.json mcpServers plus `cbc mcp add`; tools surface as mcp__<server>__<tool>",
            ),
            AdapterCapabilityDecl::new(
                Capability::WebSearch,
                Support::Native,
                "WebSearch tool in the cbc tool set (named in the documented permissions deny list)",
            ),
            AdapterCapabilityDecl::new(
                Capability::ComputerUse,
                Support::Native,
                "ComputerUse tool named in the documented permissions deny list",
            ),
            AdapterCapabilityDecl::new(
                Capability::Vision,
                Support::Native,
                "image input routed to supportsImages models over the OpenAI-compatible chat transport",
            ),
        ]
    }

    /// EXT-08/09: MCP destination — WRITABLE `.mcp.json` under the config
    /// root (also manageable via `cbc mcp add/add-json/remove`); canonical
    /// writes are strict JSON, a valid JSONC subset.
    fn mcp_decl(&self) -> Option<McpAdapterDecl> {
        Some(McpAdapterDecl::new(
            ".mcp.json",
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::User,
            RestartBehavior::Reload,
        ))
    }

    /// EXT-06: explicit plugin-mechanism absence (corpus-grounded).
    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "no CLI plugin/skill loading path documented and no public skill.yml schema; desktop-app skills are UI-only (workbuddy.md §6/§7)",
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{
        CONFIG_ENV_VAR, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, KNOWN_ENV_VARS,
        MODELS_OWNED_SELECTORS, RESEARCH_DOC, SCHEMA_VERSION_STR, SETTINGS_OWNED_SELECTORS,
        WorkBuddyAdapter,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::capability::{Capability, Support};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> WorkBuddyAdapter {
        WorkBuddyAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-workbuddy-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-09-08T00:00:00Z".to_owned(),
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
        assert!(result.evidence.iter().any(|e| e.contains(CONFIG_ENV_VAR)));
        match result.present {
            InstallPresence::Absent => {
                assert!(result.version.is_none());
            }
            InstallPresence::Present | InstallPresence::UnknownVersion => {
                assert!(
                    result
                        .evidence
                        .iter()
                        .any(|e| e.contains("binary") || e.contains("npm"))
                );
            }
            InstallPresence::Broken => {}
        }
        assert_ne!(result.confidence.to_string(), "");
    }

    #[test]
    fn version_resolution_maps_detected() {
        let a = adapter();
        let res = a.version_resolution();
        if res.detected_version.is_some() {
            assert_eq!(res.schema_version.as_deref(), Some(SCHEMA_VERSION_STR));
            assert!(res.compatible);
        } else {
            assert!(!res.compatible);
            assert!(res.schema_version.is_none());
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = [
            ("cbc 2.147.0", Some("2.147.0")),
            ("@tencent-ai/codebuddy-code 2.147.0", Some("2.147.0")),
            ("v2.103.4", Some("2.103.4")),
            ("2.147.0-beta.1", Some("2.147.0-beta.1")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = WorkBuddyAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn version_triple_and_era_boundary() {
        assert_eq!(
            WorkBuddyAdapter::parse_version_triple("2.147.0"),
            Some((2, 147, 0))
        );
        assert_eq!(
            WorkBuddyAdapter::parse_version_triple("2.103.4"),
            Some((2, 103, 4))
        );
        assert_eq!(WorkBuddyAdapter::parse_version_triple("bogus"), None);
        assert!(!WorkBuddyAdapter::is_auto_compact_window_era("2.103.3"));
        assert!(WorkBuddyAdapter::is_auto_compact_window_era("2.103.4"));
        assert!(WorkBuddyAdapter::is_auto_compact_window_era("2.147.0"));
        assert!(!WorkBuddyAdapter::is_auto_compact_window_era("nonsense"));
    }

    #[test]
    fn config_surfaces_include_writable_primary() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 10);

        let models = surfaces
            .iter()
            .find(|s| s.id == "models.json")
            .expect("models.json surface must exist");
        assert_eq!(models.kind, DocumentKind::Json);
        assert_eq!(models.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(models.scope, ConfigScope::User);
        assert!(models.backup_required);
        assert!(models.owned_selectors.contains(&"models".to_owned()));
        assert!(
            models
                .owned_selectors
                .contains(&"availableModels".to_owned())
        );

        let settings = surfaces
            .iter()
            .find(|s| s.id == "settings.json")
            .expect("settings.json surface must exist");
        assert_eq!(settings.kind, DocumentKind::Json);
        assert!(settings.backup_required);
        assert!(settings.owned_selectors.contains(&"permissions".to_owned()));

        let mcp = surfaces
            .iter()
            .find(|s| s.id == ".mcp.json")
            .expect(".mcp.json surface must exist");
        assert_eq!(mcp.kind, DocumentKind::Json);
        assert_eq!(mcp.owned_selectors, vec!["mcpServers".to_owned()]);

        // Deprecated locations and the keychain stay modelled but secondary;
        // the bare project `mcp.json` is project scope, the `~/.codebuddy/*`
        // pair user scope (workbuddy.md §1.1).
        let deprecated_project = surfaces
            .iter()
            .find(|s| s.id == "deprecated.project.mcp.json")
            .expect("deprecated project mcp surface");
        assert_eq!(deprecated_project.scope, ConfigScope::ProjectWorkspace);
        let deprecated_user = surfaces
            .iter()
            .find(|s| s.id == "deprecated.user.mcp.json")
            .expect("deprecated user mcp surface");
        assert_eq!(deprecated_user.scope, ConfigScope::User);
        let keychain = surfaces
            .iter()
            .find(|s| s.id == "credentials")
            .expect("credentials surface");
        assert_eq!(keychain.kind, DocumentKind::Keychain);
        assert_eq!(keychain.ownership, SurfaceOwnership::ExternalSecretStore);

        // Settings precedence: local > project > user.
        let precedence = |id: &str| {
            surfaces
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.precedence)
                .unwrap()
        };
        assert!(precedence("project.settings.local.json") > precedence("project.settings.json"));
        assert!(precedence("project.settings.json") > precedence("settings.json"));
    }

    #[test]
    fn owned_selectors_are_stable() {
        for selectors in [MODELS_OWNED_SELECTORS, SETTINGS_OWNED_SELECTORS] {
            assert!(selectors.len() >= 2);
            let set: HashSet<&str> = selectors.iter().copied().collect();
            assert_eq!(set.len(), selectors.len(), "selectors must be unique");
            for sel in set {
                assert!(!sel.is_empty());
            }
        }
    }

    #[test]
    fn supported_operations_cover_constrained() {
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
    fn plan_mirror_exclusions_cover_sessions_and_locks() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        for pat in ["sessions/*", "tool-results/*", "logs/*", "*.lock"] {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"models.json".to_owned()));
        assert!(!exclusions.contains(&"settings.json".to_owned()));
    }

    #[test]
    fn plan_wrapper_sets_env_and_executable() {
        let tmp_root = crate::test_util::tmp_abs_str(".codebuddy-work");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == tmp_root.as_str())
        );
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == "DISABLE_AUTOUPDATER" && v == "1")
        );
        assert_eq!(plan.executable.as_deref(), Some(EXECUTABLE));
        assert!(plan.args.contains(&"--setting-sources".to_owned()));
        assert!(
            plan.auth_prerequisites
                .iter()
                .any(|p| p.reference == "CODEBUDDY_AUTH_TOKEN")
        );
        assert!(
            plan.auth_prerequisites
                .iter()
                .any(|p| p.reference == "CODEBUDDY_API_KEY")
        );
        assert!(!plan.shared_state_warnings.is_empty());
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(CONFIG_ENV_VAR));
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let tmp_root = crate::test_util::tmp_abs_str("my workbuddy work");
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
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".codebuddy-work"));
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
        assert!(candidates.len() >= 5);
        assert!(candidates.contains(&"~/.codebuddy".to_owned()));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
        assert!(candidates.contains(&"~/.codebuddy/mcp.json".to_owned()));
        assert!(candidates.contains(&".mcp.json".to_owned()));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".codebuddy-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".codebuddy-work"));
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
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".codebuddy-work"));
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let primary = surfaces.iter().find(|s| s.id == "models.json").unwrap();
        assert_eq!(primary.path_resolver.fallback, "~/.codebuddy/models.json");
        for hint in [
            primary.path_resolver.linux.as_deref(),
            primary.path_resolver.macos.as_deref(),
            primary.path_resolver.windows.as_deref(),
        ] {
            assert!(hint.unwrap().contains(CONFIG_ENV_VAR));
        }
    }

    #[test]
    fn surface_schema_declares_shapes() {
        let a = adapter();
        let models = a.surface_schema("models.json").expect("models schema");
        assert_eq!(models.root_shape, Some(crate::adapter::RootShape::Object));
        assert!(models.owned_key_rules.iter().any(|r| r.path == "models"));
        let settings = a.surface_schema("settings.json").expect("settings schema");
        assert!(
            settings
                .owned_key_rules
                .iter()
                .any(|r| r.path == "permissions")
        );
        assert!(
            settings
                .owned_key_rules
                .iter()
                .any(|r| r.path == "enabledMcpjsonServers")
        );
        let mcp = a.surface_schema(".mcp.json").expect("mcp schema");
        assert!(mcp.owned_key_rules.iter().any(|r| r.path == "mcpServers"));
        assert!(a.surface_schema("unknown-surface").is_none());
    }

    #[test]
    fn schema_rejects_non_object_models_root() {
        let a = adapter();
        let diags = crate::adapter::validate_surface_content(
            &a,
            "models.json",
            b"[1, 2, 3]",
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("[workbuddy/models.json]"))
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("root must be a object"))
        );
    }

    #[test]
    fn schema_rejects_wrong_typed_settings_key() {
        let a = adapter();
        let content = br#"{"permissions": "bypass"}"#;
        let diags = crate::adapter::validate_surface_content(
            &a,
            "settings.json",
            content,
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("owned key `permissions`"))
        );
        // Foreign keys stay untouched by design (partial schema).
        let foreign = br#"{"unknownSetting": "fine"}"#;
        let diags = crate::adapter::validate_surface_content(
            &a,
            "settings.json",
            foreign,
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(
            diags.is_empty(),
            "foreign keys must not diagnose: {diags:?}"
        );
    }

    #[test]
    fn era_conflict_flags_legacy_carrier_on_current_cbc() {
        let legacy = std::fs::read(fixture_path("models.boundary_legacy.json")).unwrap();
        // Current era (>= 2.103.4) + string maxInputTokens => conflict.
        let reason = WorkBuddyAdapter::models_era_conflict("2.147.0", &legacy);
        assert!(reason.as_deref().is_some_and(|r| r.contains("2.147.0")));
        // Boundary version itself still conflicts at exactly 2.103.4.
        assert!(WorkBuddyAdapter::models_era_conflict("2.103.4", &legacy).is_some());
        // Pre-boundary versions accept the legacy carrier.
        assert!(WorkBuddyAdapter::models_era_conflict("2.103.3", &legacy).is_none());
        // Current-era fixture (maxInputTokens omitted) is clean.
        let current = std::fs::read(fixture_path("models.boundary_current.json")).unwrap();
        assert!(WorkBuddyAdapter::models_era_conflict("2.147.0", &current).is_none());
        // Numeric caps are schema-legal in both eras.
        let numeric = br#"{"models": [{"id": "m", "maxInputTokens": 128000}]}"#;
        assert!(WorkBuddyAdapter::models_era_conflict("2.147.0", numeric).is_none());
        // Malformed content is syntax validation's problem, not era's.
        assert!(WorkBuddyAdapter::models_era_conflict("2.147.0", b"{ nope").is_none());
    }

    #[test]
    fn mcp_decl_is_writable_json_destination() {
        let a = adapter();
        let decl = a.mcp_decl().expect("workbuddy declares an MCP destination");
        assert_eq!(decl.dest_file, ".mcp.json");
        assert_eq!(decl.dest_key, "mcpServers");
        assert_eq!(decl.kind, DocumentKind::Json);
        assert!(
            decl.read_only.is_none(),
            "workbuddy MCP dest must be writable"
        );
    }

    #[test]
    fn plugin_absence_is_declared_with_reason() {
        let a = adapter();
        assert!(a.plugin_decl().is_none());
        let reason = a
            .plugin_absence_reason()
            .expect("plugin absence must be explicit");
        assert!(reason.contains("UI-only"));
    }

    #[test]
    fn skill_modes_match_constrained_catalog_state() {
        let a = adapter();
        let modes: HashSet<String> = a
            .supported_skill_modes()
            .iter()
            .map(ToString::to_string)
            .collect();
        for mode in ["link_all", "link_selected", "copy_selected"] {
            assert!(modes.contains(mode), "missing skill mode {mode}");
        }
    }

    #[test]
    fn capability_declarations_cover_catalog_natively_without_duplicates() {
        let a = adapter();
        let decls = a.capability_declarations();
        let mut seen: Vec<Capability> = Vec::new();
        for decl in &decls {
            assert_eq!(
                decl.support,
                Support::Native,
                "{} must declare native transport",
                decl.capability
            );
            assert!(
                !decl.explanation.is_empty(),
                "{} must carry corpus evidence",
                decl.capability
            );
            assert!(
                decl.version_req.is_none(),
                "{}: no version-gated claims without a verified format",
                decl.capability
            );
            assert!(
                !seen.contains(&decl.capability),
                "duplicate declaration for {}",
                decl.capability
            );
            seen.push(decl.capability);
        }
        // Every catalog capability is claimed exactly once, including the
        // deny-list-named tools (WebSearch, ComputerUse) and the
        // supportsImages-driven image path (Vision).
        for capability in [
            Capability::Mcp,
            Capability::WebSearch,
            Capability::ComputerUse,
            Capability::Vision,
        ] {
            assert!(
                seen.contains(&capability),
                "missing capability declaration for {capability}"
            );
        }
    }

    #[test]
    fn npm_version_harvest_reads_global_tree_output() {
        // Shape mirrors `npm ls -g` output for a global install.
        let global_tree =
            "/usr/lib\n├── @tencent-ai/codebuddy-code@2.147.0\n└── typescript@5.6.2\n";
        assert_eq!(
            WorkBuddyAdapter::harvest_npm_version(global_tree).as_deref(),
            Some("2.147.0")
        );
        // Tree without the package: no version claim.
        let without = "/usr/lib\n└── typescript@5.6.2\n";
        assert_eq!(WorkBuddyAdapter::harvest_npm_version(without), None);
        // Empty or errored output: no version claim.
        assert_eq!(WorkBuddyAdapter::harvest_npm_version(""), None);
        // Trailing tree glyphs after the version do not leak into it.
        let decorated = "└── @tencent-ai/codebuddy-code@2.147.4-beta.1\n";
        assert_eq!(
            WorkBuddyAdapter::harvest_npm_version(decorated).as_deref(),
            Some("2.147.4-beta.1")
        );
    }

    // -----------------------------------------------------------------------
    // Fixture-backed conformance tests (QAL-02 corpus)
    // -----------------------------------------------------------------------

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/workbuddy")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_corpus_covers_all_variants() {
        for name in [
            "models.minimal.json",
            "models.populated.json",
            "models.foreign.json",
            "models.malformed.json",
            "models.boundary_legacy.json",
            "models.boundary_current.json",
            "settings.minimal.json",
            "settings.populated.json",
            "settings.foreign.json",
            "settings.malformed.json",
            "mcp.minimal.json",
            "mcp.populated.json",
            "env.minimal.env",
            "env.populated.env",
            "wrapper.sh",
            "version.txt",
        ] {
            assert!(fixture_path(name).is_file(), "fixture missing: {name}");
        }
    }

    #[test]
    fn fixture_models_minimal_parses() {
        let map = superai_config::json::load(&fixture_path("models.minimal.json")).unwrap();
        assert!(map.contains_key("models"));
        assert!(map.contains_key("availableModels"));
        // Schema-clean: no diagnostics against the declared surface schema.
        let a = adapter();
        let content = std::fs::read(fixture_path("models.minimal.json")).unwrap();
        let diags = crate::adapter::validate_surface_content(
            &a,
            "models.json",
            &content,
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(
            diags.is_empty(),
            "minimal fixture must be schema-clean: {diags:?}"
        );
    }

    #[test]
    fn fixture_models_populated_parses_with_related_models() {
        let value =
            superai_config::json::load_value(&fixture_path("models.populated.json")).unwrap();
        let models = value.get("models").and_then(|m| m.as_array()).unwrap();
        assert!(models.len() >= 2);
        let deepseek = models.first().unwrap();
        assert_eq!(
            deepseek
                .get("maxInputTokens")
                .and_then(serde_json::Value::as_i64),
            Some(128_000)
        );
        assert!(deepseek.get("relatedModels").is_some());
        let available = value
            .get("availableModels")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(available.len() >= 2);
    }

    #[test]
    fn fixture_models_boundary_eras_parse_cleanly() {
        // Both era fixtures must parse and pass the declared schema: token
        // caps are deliberately untyped (number OR string both valid).
        let a = adapter();
        for name in [
            "models.boundary_legacy.json",
            "models.boundary_current.json",
        ] {
            let content = std::fs::read(fixture_path(name)).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&content).unwrap();
            assert!(value.get("models").is_some());
            let diags = crate::adapter::validate_surface_content(
                &a,
                "models.json",
                &content,
                superai_config::document::DocumentKind::StrictJson,
            );
            assert!(diags.is_empty(), "{name} must be schema-clean: {diags:?}");
        }
    }

    #[test]
    fn fixture_models_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("models.foreign.json");
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("workbuddy-foreign");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("models.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "availableModels".to_owned(),
                serde_json::Value::Array(vec![serde_json::Value::String("test-model".to_owned())]),
            );
            assert!(map.contains_key("unknownTopLevel"));
            assert!(map.contains_key("experimentalArray"));
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert!(after.contains_key("unknownTopLevel"));
        assert!(after.contains_key("experimentalArray"));
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn fixture_models_and_settings_malformed_rejected() {
        for name in ["models.malformed.json", "settings.malformed.json"] {
            let result = superai_config::json::load(&fixture_path(name));
            assert!(result.is_err(), "{name} must fail to parse");
        }
    }

    #[test]
    fn fixture_settings_minimal_and_populated_parse() {
        let minimal = superai_config::json::load(&fixture_path("settings.minimal.json")).unwrap();
        assert!(minimal.is_empty());
        let populated =
            superai_config::json::load(&fixture_path("settings.populated.json")).unwrap();
        let permissions = populated.get("permissions").unwrap();
        assert_eq!(
            permissions.get("defaultMode").and_then(|v| v.as_str()),
            Some("bypassPermissions")
        );
        let deny = permissions.get("deny").and_then(|v| v.as_array()).unwrap();
        assert!(deny.len() >= 4);
        // Schema-clean against the declared owned-key rules.
        let a = adapter();
        let content = std::fs::read(fixture_path("settings.populated.json")).unwrap();
        let diags = crate::adapter::validate_surface_content(
            &a,
            "settings.json",
            &content,
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(
            diags.is_empty(),
            "populated settings must be schema-clean: {diags:?}"
        );
    }

    #[test]
    fn fixture_settings_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("unknownSetting"));
        assert!(original.contains_key("experimentalFlag"));
        let dir = crate::test_util::temp_dir_unique("workbuddy-settings-foreign");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("settings.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "autoCompactEnabled".to_owned(),
                serde_json::Value::Bool(false),
            );
            assert!(map.contains_key("futureKey"));
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert_eq!(
            after
                .get("autoCompactEnabled")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert!(after.contains_key("unknownSetting"));
        assert!(after.contains_key("experimentalFlag"));
        assert!(after.contains_key("futureKey"));
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn fixture_mcp_parses_and_matches_declared_destination() {
        let a = adapter();
        let decl = a.mcp_decl().unwrap();
        let minimal = superai_config::json::load(&fixture_path("mcp.minimal.json")).unwrap();
        assert!(minimal.get("mcpServers").is_some());
        let populated_value =
            superai_config::json::load_value(&fixture_path("mcp.populated.json")).unwrap();
        let servers = populated_value
            .get(&decl.dest_key)
            .and_then(|v| v.as_object())
            .unwrap();
        assert!(servers.len() >= 2);
        let filesystem = servers.get("filesystem").unwrap();
        assert!(filesystem.get("command").is_some());
        let remote = servers.get("remote-tools").unwrap();
        assert_eq!(remote.get("type").and_then(|v| v.as_str()), Some("http"));
        assert!(remote.get("url").is_some());
    }

    #[test]
    fn fixture_env_files_load_documented_vars_with_fake_markers() {
        let minimal = superai_config::env_file::load(&fixture_path("env.minimal.env")).unwrap();
        assert_eq!(
            minimal.get("CODEBUDDY_API_KEY").map(String::as_str),
            Some("sk-test-fake-123")
        );
        let populated = superai_config::env_file::load(&fixture_path("env.populated.env")).unwrap();
        for name in [
            "CODEBUDDY_AUTH_TOKEN",
            "CBC_BASE_URL",
            "DISABLE_AUTOUPDATER",
        ] {
            assert!(
                populated.contains_key(name),
                "missing documented var {name}"
            );
        }
        // Every fixture key is either a documented var or rejected, and every
        // value carries an explicit fake/synthetic marker.
        for (key, value) in &populated {
            assert!(
                KNOWN_ENV_VARS.contains(&key.as_str()),
                "env fixture carries undocumented var {key}"
            );
            let fake = value.contains("fake")
                || value.contains("test")
                || value.contains("example")
                || matches!(value.as_str(), "1" | "public" | "20000");
            assert!(fake, "env value for {key} lacks a fake marker: {value}");
        }
    }

    #[test]
    fn fixture_wrapper_relocates_config_dir_without_real_secrets() {
        let text = std::fs::read_to_string(fixture_path("wrapper.sh")).unwrap();
        assert!(text.contains(CONFIG_ENV_VAR));
        assert!(text.contains(EXECUTABLE));
        assert!(text.contains("DISABLE_AUTOUPDATER"));
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("export CODEBUDDY_API_KEY=") {
                assert!(
                    value.contains("fake") || value.contains("test"),
                    "wrapper api key must keep its fake marker: {value}"
                );
            }
        }
    }

    #[test]
    fn fixture_version_txt_parses_via_npm_metadata_shape() {
        let text = std::fs::read_to_string(fixture_path("version.txt")).unwrap();
        let version = WorkBuddyAdapter::parse_version_output(&text);
        assert_eq!(version.as_deref(), Some("2.147.0"));
        assert!(WorkBuddyAdapter::is_auto_compact_window_era(
            &version.unwrap()
        ));
    }

    #[test]
    fn registry_no_harness_value_leak() {
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".codebuddy-work"));
        let json = serde_json::to_string(&inst).unwrap();
        for forbidden in ["api_key", "apiKey", "bearer", "secret"] {
            assert!(
                !json.contains(&format!("\"{forbidden}\"")),
                "forbidden field `{forbidden}` appears in json: {json}"
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
