//! `OpenCode` adapter — JSONC layered config with relocated-root and inline overrides.
//!
//! Research source: `docs/harness-configs/opencode.md` (last verified 2026-08-25).
//! Executable `opencode`, config root `~/.config/opencode` or `$XDG_CONFIG_HOME/opencode`,
//! primary writable surface `opencode.json` / `opencode.jsonc` (JSONC, layered),
//! isolation `relocated-root` via `XDG_CONFIG_HOME` plus `OPENCODE_CONFIG` / `OPENCODE_CONFIG_CONTENT`.

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

/// Harness identifier for `OpenCode`.
pub const HARNESS_ID_STR: &str = "opencode";

/// Human display name.
pub const DISPLAY_NAME: &str = "OpenCode";

/// Primary executable name.
pub const EXECUTABLE: &str = "opencode";

/// Environment variable that relocates the global config root (XDG base).
pub const CONFIG_ENV_VAR: &str = "XDG_CONFIG_HOME";

/// Environment variable for an extra config file layered between global and project.
pub const CUSTOM_CONFIG_ENV_VAR: &str = "OPENCODE_CONFIG";

/// Environment variable for inline JSON overrides (highest precedence).
pub const INLINE_CONFIG_ENV_VAR: &str = "OPENCODE_CONFIG_CONTENT";

/// Environment variable for extra directory scanned like `.opencode/`.
pub const CONFIG_DIR_ENV_VAR: &str = "OPENCODE_CONFIG_DIR";

/// Environment variable for TUI config override.
pub const TUI_CONFIG_ENV_VAR: &str = "OPENCODE_TUI_CONFIG";

/// Default config root when `XDG_CONFIG_HOME` is unset.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.config/opencode";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/opencode.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current config shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider/model mutation inside `opencode.json` (JSONC).
///
/// These are top-level keys superai owns; everything else round-trips untouched
/// via `superai-config::jsonc`.
pub const OWNED_SELECTORS: &[&str] = &[
    "model",
    "small_model",
    "provider",
    "mcp",
    "permission",
    "share",
    "autoupdate",
    "disabled_providers",
    "enabled_providers",
    "agent",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for `OpenCode`.
///
/// Isolation is `relocated-root` via `XDG_CONFIG_HOME` (full) plus
/// `OPENCODE_CONFIG` (file) and `OPENCODE_CONFIG_CONTENT` (inline) for
/// lighter layering. The wrapper sets `XDG_CONFIG_HOME` to the instance
/// `config_root` and `OPENCODE_CONFIG` to `<root>/opencode.json`.
#[derive(Debug, Clone)]
pub struct OpenCodeAdapter {
    id: HarnessId,
}

impl OpenCodeAdapter {
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

    /// Config relocation env var (`XDG_CONFIG_HOME`).
    pub fn config_env_var(&self) -> &str {
        CONFIG_ENV_VAR
    }

    /// Custom config env var.
    pub fn custom_config_env_var(&self) -> &str {
        CUSTOM_CONFIG_ENV_VAR
    }

    /// Inline config env var.
    pub fn inline_config_env_var(&self) -> &str {
        INLINE_CONFIG_ENV_VAR
    }

    /// Try to locate the `opencode` binary via `PATH`.
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
        }
        None
    }

    /// Probe `opencode --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `opencode 0.12.0` or `opencode-ai 1.0.0` into `1.0.0`.
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

    /// Resolve the default global config root: `$XDG_CONFIG_HOME/opencode` or `~/.config/opencode`.
    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(CONFIG_ENV_VAR)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir).join("opencode"));
        }
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        // Also respect XDG_CONFIG_HOME if set but not via CONFIG_ENV_VAR? CONFIG_ENV_VAR is XDG_CONFIG_HOME.
        // Fallback to HOME/.config/opencode.
        Some(PathBuf::from(home).join(".config").join("opencode"))
    }

    /// Check if default config root exists on disk.
    #[expect(clippy::unused_self, reason = "adapter helper")]
    #[expect(dead_code, reason = "helper for future use")]
    fn default_config_root_exists(&self) -> Option<PathBuf> {
        let root = Self::default_config_root()?;
        if root.exists() { Some(root) } else { None }
    }

    /// Build the opencode.json path for a given config root.
    fn config_path_for_root(root: &Path) -> PathBuf {
        root.join("opencode.json")
    }

    /// Build the tui.json path for a given config root.
    fn tui_path_for_root(root: &Path) -> PathBuf {
        root.join("tui.json")
    }

    /// Build detection evidence about `OpenCode` config layers.
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
                    let cfg = Self::config_path_for_root(&root);
                    let cfg_jc = root.join("opencode.jsonc");
                    if cfg.exists() {
                        evidence.push(format!("opencode.json found at {}", cfg.display()));
                        if let Ok(text) = std::fs::read_to_string(&cfg)
                            && (text.contains("\"$schema\"") || text.contains("$schema"))
                        {
                            evidence.push("opencode.json contains $schema marker".to_owned());
                        }
                    } else if cfg_jc.exists() {
                        evidence.push(format!("opencode.jsonc found at {}", cfg_jc.display()));
                    } else {
                        evidence.push(format!("opencode.json missing at {}", cfg.display()));
                    }
                    let tui = Self::tui_path_for_root(&root);
                    let tui_jc = root.join("tui.jsonc");
                    if tui.exists() || tui_jc.exists() {
                        evidence.push(format!("tui config present at {}", root.display()));
                    }
                    let auth = dirs_auth_path();
                    if let Some(auth_path) = auth
                        && auth_path.exists()
                    {
                        evidence.push(format!("auth.json present at {}", auth_path.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
        // Custom config env layer.
        if let Ok(custom) = std::env::var(CUSTOM_CONFIG_ENV_VAR)
            && !custom.trim().is_empty()
        {
            let p = Path::new(&custom);
            if p.exists() {
                evidence.push(format!(
                    "custom config {CUSTOM_CONFIG_ENV_VAR} exists at {}",
                    p.display()
                ));
            } else {
                evidence.push(format!(
                    "custom config {CUSTOM_CONFIG_ENV_VAR} set to {} (missing)",
                    p.display()
                ));
            }
        } else {
            evidence.push(format!("{CUSTOM_CONFIG_ENV_VAR} not set"));
        }
        if let Ok(inline) = std::env::var(INLINE_CONFIG_ENV_VAR)
            && !inline.trim().is_empty()
        {
            evidence.push(format!(
                "inline config {INLINE_CONFIG_ENV_VAR} is set (len {})",
                inline.len()
            ));
        }
        // Project opencode.json heuristic.
        let proj = Path::new("opencode.json");
        let proj_jc = Path::new("opencode.jsonc");
        if proj.exists() {
            evidence.push(format!("project opencode.json found at {}", proj.display()));
        } else if proj_jc.exists() {
            evidence.push(format!(
                "project opencode.jsonc found at {}",
                proj_jc.display()
            ));
        }
        // Extra config dir.
        if let Ok(extra) = std::env::var(CONFIG_DIR_ENV_VAR)
            && !extra.trim().is_empty()
        {
            evidence.push(format!("extra config dir {CONFIG_DIR_ENV_VAR}={extra}"));
        }
    }
}

/// Resolve auth.json path: `~/.local/share/opencode/auth.json` or `$XDG_DATA_HOME/opencode/auth.json`.
fn dirs_auth_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.trim().is_empty()
    {
        return Some(PathBuf::from(xdg).join("opencode").join("auth.json"));
    }
    let home = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())?;
    if home.trim().is_empty() {
        return None;
    }
    Some(
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("opencode")
            .join("auth.json"),
    )
}

impl Default for OpenCodeAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "opencode is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for OpenCodeAdapter {
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
            notes.push(format!("detected opencode version {v}"));
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

        // Primary writable surface: global opencode.json / opencode.jsonc (JSONC, layered).
        let opencode_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/opencode/opencode.json"),
            Some("$XDG_CONFIG_HOME/opencode/opencode.json"),
            Some("%XDG_CONFIG_HOME%\\opencode\\opencode.json"),
            "~/.config/opencode/opencode.json",
        );
        let mut opencode_surface = ConfigSurface::new(
            "opencode.json",
            opencode_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        opencode_surface.precedence = 10;
        opencode_surface.owned_selectors =
            OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        opencode_surface.backup_required = true;
        opencode_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(opencode_surface);

        // JSONC variant — same shape but explicit .jsonc extension for resolvers.
        let opencode_jc_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/opencode/opencode.jsonc"),
            Some("$XDG_CONFIG_HOME/opencode/opencode.jsonc"),
            Some("%XDG_CONFIG_HOME%\\opencode\\opencode.jsonc"),
            "~/.config/opencode/opencode.jsonc",
        );
        let mut opencode_jc_surface = ConfigSurface::new(
            "opencode.jsonc",
            opencode_jc_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        opencode_jc_surface.precedence = 10;
        opencode_jc_surface.owned_selectors =
            OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        opencode_jc_surface.backup_required = true;
        opencode_jc_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(opencode_jc_surface);

        // TUI config: tui.json / tui.jsonc
        let tui_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/opencode/tui.json"),
            Some("$XDG_CONFIG_HOME/opencode/tui.json"),
            Some("%XDG_CONFIG_HOME%\\opencode\\tui.json"),
            "~/.config/opencode/tui.json",
        );
        let mut tui_surface = ConfigSurface::new(
            "tui.json",
            tui_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        tui_surface.precedence = 5;
        tui_surface.backup_required = true;
        surfaces.push(tui_surface);

        // Auth surface: ~/.local/share/opencode/auth.json — external secret store.
        let auth_resolver = PathResolver::new(
            Some("$XDG_DATA_HOME/opencode/auth.json"),
            Some("$XDG_DATA_HOME/opencode/auth.json"),
            Some("%APPDATA%\\opencode\\auth.json"),
            "~/.local/share/opencode/auth.json",
        );
        let mut auth = ConfigSurface::new(
            "auth.json",
            auth_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        auth.precedence = 0;
        auth.backup_required = false;
        auth.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(auth);

        // Project-local opencode.json — workspace scope, layered after global.
        let project_resolver = PathResolver::fallback_only(
            "opencode.json in cwd or nearest git root (also opencode.jsonc)",
        );
        let mut project_surface = ConfigSurface::new(
            "project opencode.json",
            project_resolver,
            DocumentKind::Jsonc,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_surface.precedence = 20;
        project_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        project_surface.backup_required = true;
        surfaces.push(project_surface);

        // Custom config file via OPENCODE_CONFIG (explicit-config).
        let custom_resolver = PathResolver::new(
            Some("$OPENCODE_CONFIG"),
            Some("$OPENCODE_CONFIG"),
            Some("%OPENCODE_CONFIG%"),
            "$OPENCODE_CONFIG (custom path, layered between global and project)",
        );
        let mut custom_surface = ConfigSurface::new(
            "OPENCODE_CONFIG",
            custom_resolver,
            DocumentKind::Jsonc,
            ConfigScope::SessionInline,
            SurfaceOwnership::UserEditable,
        );
        custom_surface.precedence = 15;
        custom_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        custom_surface.backup_required = true;
        surfaces.push(custom_surface);

        // Inline JSON via OPENCODE_CONFIG_CONTENT (session inline, highest).
        let inline_resolver = PathResolver::fallback_only("$OPENCODE_CONFIG_CONTENT (inline JSON)");
        let mut inline_surface = ConfigSurface::new(
            "OPENCODE_CONFIG_CONTENT",
            inline_resolver,
            DocumentKind::Jsonc,
            ConfigScope::SessionInline,
            SurfaceOwnership::UserEditable,
        );
        inline_surface.precedence = 40;
        inline_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        inline_surface.backup_required = false;
        surfaces.push(inline_surface);

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
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "*.lock".to_owned(),
            "node_modules/*".to_owned(),
            ".git/*".to_owned(),
            "tmp/*".to_owned(),
            "auth.json".to_owned(),
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
        let mut plan = WrapperPlan::new("relocated-root via XDG_CONFIG_HOME + OPENCODE_CONFIG");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        let cfg_path = Path::new(&instance.config_root.to_string()).join("opencode.json");
        plan.env_vars.push((
            CUSTOM_CONFIG_ENV_VAR.to_owned(),
            cfg_path.display().to_string(),
        ));
        plan.description = format!(
            " Wrapper sets {}={} and {}={} and execs `{}`",
            CONFIG_ENV_VAR,
            instance.config_root,
            CUSTOM_CONFIG_ENV_VAR,
            cfg_path.display(),
            EXECUTABLE
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/opencode".to_owned(),
            "~/.config/opencode/opencode.json".to_owned(),
            "~/.config/opencode/opencode.jsonc".to_owned(),
            "$XDG_CONFIG_HOME/opencode".to_owned(),
            "$OPENCODE_CONFIG".to_owned(),
            "$OPENCODE_CONFIG_CONTENT".to_owned(),
            "./opencode.json".to_owned(),
            "./opencode.jsonc".to_owned(),
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
            Isolation::RelocatedRoot
            | Isolation::ExplicitConfig
            | Isolation::EnvOnly
            | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("opencode requires isolation relocated_root, got {other}"),
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
    use std::path::PathBuf;

    use super::{
        CONFIG_ENV_VAR, CUSTOM_CONFIG_ENV_VAR, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR,
        INLINE_CONFIG_ENV_VAR, OWNED_SELECTORS, RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> super::OpenCodeAdapter {
        super::OpenCodeAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-opencode-1").unwrap(),
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
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(a.display_name(), DISPLAY_NAME);
        assert_eq!(a.executable_name(), EXECUTABLE);
        assert_eq!(a.config_env_var(), CONFIG_ENV_VAR);
        assert_eq!(a.custom_config_env_var(), CUSTOM_CONFIG_ENV_VAR);
        assert_eq!(a.inline_config_env_var(), INLINE_CONFIG_ENV_VAR);
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
            ("opencode 0.12.0", Some("0.12.0")),
            ("opencode-ai 1.0.0", Some("1.0.0")),
            ("v0.15.3", Some("0.15.3")),
            ("Version: 1.2.3", Some("1.2.3")),
            ("0.12.0-alpha", Some("0.12.0-alpha")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = super::OpenCodeAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_writable_jsonc() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 5);
        let cfg = surfaces
            .iter()
            .find(|s| s.id == "opencode.json")
            .expect("opencode.json surface must exist");
        assert_eq!(cfg.kind, DocumentKind::Jsonc);
        assert_eq!(cfg.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(cfg.scope, ConfigScope::User);
        assert!(cfg.backup_required);
        for selector in ["model", "provider", "mcp"] {
            assert!(
                cfg.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in &cfg.owned_selectors {
            assert!(!sel.is_empty());
        }
        for sel in OWNED_SELECTORS {
            assert!(cfg.owned_selectors.contains(&(*sel).to_owned()));
        }

        let cfg_jc = surfaces
            .iter()
            .find(|s| s.id == "opencode.jsonc")
            .expect("opencode.jsonc surface");
        assert_eq!(cfg_jc.kind, DocumentKind::Jsonc);

        let auth = surfaces
            .iter()
            .find(|s| s.id == "auth.json")
            .expect("auth surface");
        assert_eq!(auth.ownership, SurfaceOwnership::ExternalSecretStore);
        assert!(!auth.backup_required);

        let project = surfaces
            .iter()
            .find(|s| s.id == "project opencode.json")
            .expect("project surface");
        assert_eq!(project.scope, ConfigScope::ProjectWorkspace);
        assert_eq!(project.kind, DocumentKind::Jsonc);

        let inline = surfaces
            .iter()
            .find(|s| s.id == "OPENCODE_CONFIG_CONTENT")
            .expect("inline surface");
        assert_eq!(inline.scope, ConfigScope::SessionInline);
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
    fn plan_mirror_exclusions_cover_cache_and_locks() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        let must_contain = ["cache/*", "logs/*", "*.log", "node_modules/*"];
        for pat in must_contain {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"opencode.json".to_owned()));
    }

    #[test]
    #[expect(clippy::excessive_nesting, reason = "test closure nesting is explicit")]
    fn plan_mirror_includes_config_and_excludes_cache() {
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
        assert!(!is_excluded("opencode.json"));
        assert!(!is_excluded("opencode.jsonc"));
        assert!(!is_excluded("tui.json"));
        assert!(is_excluded("cache/data.bin"));
        assert!(is_excluded("logs/opencode.log"));
        assert!(is_excluded("node_modules/foo/index.js"));
    }

    #[test]
    fn plan_wrapper_sets_xdg_and_custom_config() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.opencode-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == "/tmp/.opencode-work")
        );
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CUSTOM_CONFIG_ENV_VAR && v.contains(".opencode-work"))
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(CONFIG_ENV_VAR));
        assert!(plan.description.contains(CUSTOM_CONFIG_ENV_VAR));
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my opencode work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == CONFIG_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, "/tmp/my opencode work");
        assert!(!env_val.contains('"'));
        assert!(env_val.contains(' '));
        let custom_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == CUSTOM_CONFIG_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert!(custom_val.contains(' '));
        assert!(!custom_val.contains('"'));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.opencode-work");
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
        assert!(candidates.iter().any(|c| c.contains("opencode")));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
        assert!(candidates.iter().any(|c| c.contains(CUSTOM_CONFIG_ENV_VAR)));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.opencode-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.opencode-work");
        inst.isolation = Isolation::FixedPathSingle;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn validate_instance_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.opencode-work");
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let cfg = surfaces.iter().find(|s| s.id == "opencode.json").unwrap();
        assert_eq!(
            cfg.path_resolver.fallback,
            "~/.config/opencode/opencode.json"
        );
        let resolver = &cfg.path_resolver;
        assert!(resolver.linux.as_deref().unwrap().contains(CONFIG_ENV_VAR));
    }

    // -----------------------------------------------------------------------
    // Fixture-backed conformance tests
    // -----------------------------------------------------------------------

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/opencode")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.json");
        let map = superai_config::jsonc::load(&path).unwrap();
        assert!(map.is_empty());
        let value = superai_config::jsonc::load_value(&path).unwrap();
        assert_eq!(value, serde_json::Value::Object(serde_json::Map::default()));
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("opencode.minimal.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::jsonc::load(&path).unwrap();
        // Minimal may be empty or contain only $schema.
        assert!(map.is_empty() || map.contains_key("$schema"));
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("opencode.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::jsonc::load(&path).unwrap();
        assert!(
            map.contains_key("model") || map.contains_key("provider") || map.contains_key("mcp")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("opencode.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::jsonc::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("opencode.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::jsonc::edit(&tmp, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("anthropic/claude-sonnet-4-5".to_owned()),
            );
            assert!(map.contains_key("foreignKey") || map.contains_key("unknownTopLevel"));
        })
        .unwrap();
        let after = superai_config::jsonc::load(&tmp).unwrap();
        assert_eq!(
            after["model"],
            serde_json::Value::String("anthropic/claude-sonnet-4-5".to_owned())
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
        let path = fixture_path("opencode.malformed.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::jsonc::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn unknown_key_preservation_via_preserve_order() {
        let dir = crate::test_util::temp_dir_unique("opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preserve.json");
        let original_json = serde_json::json!({
            "model": "anthropic/claude-sonnet-4-5",
            "foreignKey": "keep-me",
            "provider": {
                "anthropic": {
                    "options": {"baseURL": "https://example.com"}
                }
            },
            "anotherForeign": {"nested": 123}
        });
        let text = serde_json::to_string_pretty(&original_json).unwrap();
        std::fs::write(&path, text).unwrap();

        superai_config::jsonc::edit(&path, |map| {
            if let Some(provider) = map.get_mut("provider").and_then(|v| v.as_object_mut())
                && let Some(anth) = provider
                    .get_mut("anthropic")
                    .and_then(|v| v.as_object_mut())
                && let Some(opts) = anth.get_mut("options").and_then(|v| v.as_object_mut())
            {
                opts.insert(
                    "baseURL".to_owned(),
                    serde_json::Value::String("https://new.example.com".to_owned()),
                );
            }
        })
        .unwrap();
        let after = superai_config::jsonc::load(&path).unwrap();
        assert_eq!(
            after["foreignKey"],
            serde_json::Value::String("keep-me".to_owned())
        );
        assert_eq!(
            after["anotherForeign"]["nested"],
            serde_json::Value::Number(123.into())
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn provider_mutation_sets_model_and_provider() {
        let dir = crate::test_util::temp_dir_unique("opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("provider.json");
        let initial = serde_json::json!({
            "model": "anthropic/claude-sonnet-4-5",
            "provider": {
                "anthropic": {
                    "options": {"apiKey": "{env:ANTHROPIC_API_KEY}"}
                }
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        superai_config::jsonc::edit(&path, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("openrouter/anthropic/claude-sonnet-4".to_owned()),
            );
            let provider = map
                .entry("provider".to_owned())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::default()));
            if let Some(obj) = provider.as_object_mut() {
                obj.insert(
                    "openrouter".to_owned(),
                    serde_json::json!({
                        "npm": "@ai-sdk/openai-compatible",
                        "options": {"baseURL": "https://openrouter.ai/api/v1"}
                    }),
                );
            }
        })
        .unwrap();
        let after = superai_config::jsonc::load(&path).unwrap();
        assert_eq!(
            after["model"],
            serde_json::Value::String("openrouter/anthropic/claude-sonnet-4".to_owned())
        );
        assert!(
            after["provider"]
                .as_object()
                .unwrap()
                .contains_key("openrouter")
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn secret_redaction_placeholder() {
        use crate::error::RedactedString;
        let secret = RedactedString::new("sk-test-secret-opencode");
        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert!(!debug.contains("sk-test-secret-opencode"));
        assert!(!display.contains("sk-test-secret-opencode"));
        assert!(debug.contains("[REDACTED]"));
        assert!(display.contains("[REDACTED]"));
        let json = serde_json::to_string(&secret).unwrap();
        assert!(!json.contains("sk-test-secret-opencode"));
        assert!(json.contains("[REDACTED]"));
        assert_eq!(secret.expose_secret(), "sk-test-secret-opencode");
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
        let inst = sample_instance_with_root("/tmp/.opencode-work");
        a.validate_instance(&inst).unwrap();
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn wrapper_env_var_isolation_is_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root("/home/user/.opencode-isolated");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(!plan.env_vars.is_empty());
        let (key, val) = &plan.env_vars[0];
        assert_eq!(key, CONFIG_ENV_VAR);
        assert_eq!(val, "/home/user/.opencode-isolated");
    }

    #[test]
    fn jsonc_stripping_allows_comments_and_trailing_commas() {
        let dir = crate::test_util::temp_dir_unique("opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.jsonc");
        let content = r#"
        {
            // Model selection
            "model": "anthropic/claude-sonnet-4-5", // trailing comma,
            /* provider block */
            "provider": {
                "anthropic": {
                    "options": {
                        "baseURL": "https://example.com", // trailing comma,
                    },
                },
            },
        }
        "#;
        std::fs::write(&path, content).unwrap();
        let map = superai_config::jsonc::load(&path).unwrap();
        assert_eq!(
            map["model"],
            serde_json::Value::String("anthropic/claude-sonnet-4-5".to_owned())
        );
        assert_eq!(
            map["provider"]["anthropic"]["options"]["baseURL"],
            serde_json::Value::String("https://example.com".to_owned())
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn inline_config_surface_is_session_inline() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let inline = surfaces
            .iter()
            .find(|s| s.id == "OPENCODE_CONFIG_CONTENT")
            .unwrap();
        assert_eq!(inline.scope, ConfigScope::SessionInline);
        assert_eq!(inline.kind, DocumentKind::Jsonc);
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
