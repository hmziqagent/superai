//! Claude Desktop adapter: platform-gated default root, no relocation.
//! Research source: `docs/harness-configs/claude-desktop.md` (verified 2026-09-18).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use superai_config::document::ValueType;

use serde_json::Value;

use crate::adapter::{
    ADAPTER_REVISION, Adapter, Arch, ConfigScope, ConfigSurface, DetectionConfidence,
    DetectionResult, DocumentKind, Os, PathResolver, Platform, ProductStatus, RestartBehavior,
    RootShape, SurfaceOwnership, SurfaceSchema, VersionResolution, WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, InstallPresence, Isolation};

/// Harness identifier for Claude Desktop.
pub const HARNESS_ID_STR: &str = "claude-desktop";

/// Human display name.
pub const DISPLAY_NAME: &str = "Claude Desktop";

/// Linux .deb package/binary name; the GUI binary name is otherwise unverified.
pub const EXECUTABLE: &str = "claude-desktop";

/// Consumer config file name inside the per-OS app-support root.
pub const CONFIG_FILE: &str = "claude_desktop_config.json";

/// macOS config root (official, modelcontextprotocol.io quickstart).
pub const MACOS_CONFIG_ROOT: &str = "~/Library/Application Support/Claude";

/// Windows config root; MSIX installs may read the packaged Packages path instead (github #26073).
pub const WINDOWS_CONFIG_ROOT: &str = "%APPDATA%\\Claude";

/// Linux config root, community-corroborated; not yet in official consumer docs.
pub const LINUX_CONFIG_ROOT: &str = "~/.config/Claude";

/// Personal skills loaded by desktop/Cowork sessions; SHARED with the claude-code harness.
pub const PERSONAL_SKILLS_PATH: &str = "~/.claude/skills";

/// Verified-absent relocation: no env var, no portable mode, no `--config-dir`.
pub const NO_RELOCATION_NOTE: &str = "no config-relocation mechanism (verified-absent): \
no env var, no portable mode, no --config-dir; hardcoded app-support paths";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/claude-desktop.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside `claude_desktop_config.json`.
pub const OWNED_SELECTORS: &[&str] = &["mcpServers"];

/// MCP container selector.
pub const MCP_OWNED_SELECTORS: &[&str] = &["mcpServers"];

/// Linux local 3P config root; coexists by design with the 1P `~/.config/Claude`.
pub const THIRD_PARTY_LINUX_ROOT: &str = "~/.config/Claude-3p";

/// macOS local 3P config root.
pub const THIRD_PARTY_MACOS_ROOT: &str = "~/Library/Application Support/Claude-3p";

/// Windows local 3P config root.
pub const THIRD_PARTY_WINDOWS_ROOT: &str = "%LOCALAPPDATA%\\Claude-3p";

/// Inference-config directory inside the 3P root (official: `configLibrary/`).
pub const THIRD_PARTY_LIBRARY_DIR: &str = "configLibrary";

/// Library file name; undocumented, so it stays a parameter of [`commit_third_party_inference`].
pub const THIRD_PARTY_LIBRARY_FILE: &str = "inference.json";

/// Gateway-group + connection keys superai owns on the 3P surface (official names).
pub const THIRD_PARTY_OWNED_SELECTORS: &[&str] = &[
    "inferenceGatewayBaseUrl",
    "inferenceGatewayApiKey",
    "inferenceGatewayAuthScheme",
    "inferenceProvider",
    "inferenceModels",
];

/// Auth scheme the 3P gateway expects (official enum: `bearer` default | `x-api-key`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThirdPartyAuthScheme {
    /// `Authorization: Bearer <key>` (the documented default).
    Bearer,
    /// `x-api-key: <key>`.
    XApiKey,
}

impl ThirdPartyAuthScheme {
    /// The wire value the app config expects.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bearer => "bearer",
            Self::XApiKey => "x-api-key",
        }
    }
}

/// Third-party inference gateway keys; the gateway must speak the Anthropic Messages API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThirdPartyInference<'a> {
    /// Full URL of the inference gateway endpoint (`inferenceGatewayBaseUrl`).
    pub gateway_base_url: &'a str,
    /// Gateway key, written into the 3P config (0600, backed up); never reaches a manifest or log.
    pub api_key: &'a crate::error::RedactedString,
    /// Auth scheme (`inferenceGatewayAuthScheme`).
    pub auth_scheme: ThirdPartyAuthScheme,
    /// `inferenceProvider` value; the gateway flow uses `gateway`.
    pub provider: &'a str,
    /// `inferenceModels` names, first = default model.
    pub models: Vec<String>,
}

impl<'a> ThirdPartyInference<'a> {
    /// A gateway-flavored configuration with the documented defaults.
    #[must_use]
    pub fn gateway(
        base_url: &'a str,
        api_key: &'a crate::error::RedactedString,
        auth_scheme: ThirdPartyAuthScheme,
    ) -> Self {
        Self {
            gateway_base_url: base_url,
            api_key,
            auth_scheme,
            provider: "gateway",
            models: Vec::new(),
        }
    }

    /// Pin the model list (first entry is the default).
    #[must_use]
    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.models = models;
        self
    }
}

/// Standalone surface with its own fixed `Claude-3p` root, reached via [`crate::profile`].
pub fn third_party_inference_surface() -> ConfigSurface {
    let file = format!("{THIRD_PARTY_LIBRARY_DIR}/{THIRD_PARTY_LIBRARY_FILE}");
    let resolver = PathResolver::new(
        Some(&format!("{THIRD_PARTY_LINUX_ROOT}/{file}")),
        Some(&format!("{THIRD_PARTY_MACOS_ROOT}/{file}")),
        Some(&format!(
            "{THIRD_PARTY_WINDOWS_ROOT}\\{THIRD_PARTY_LIBRARY_DIR}\\{THIRD_PARTY_LIBRARY_FILE}"
        )),
        &format!("{THIRD_PARTY_MACOS_ROOT}/{file}"),
    );
    let mut surface = ConfigSurface::new(
        &file,
        resolver,
        DocumentKind::Json,
        ConfigScope::User,
        SurfaceOwnership::UserEditable,
    );
    surface.precedence = 10;
    surface.owned_selectors = THIRD_PARTY_OWNED_SELECTORS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    surface.backup_required = true;
    surface.restart_behavior = RestartBehavior::Restart;
    surface
}

/// Write the 3P inference config under `config_root` (never the real home):
/// fresh read, backup, atomic write; foreign keys survive; 0600 for the key.
pub fn commit_third_party_inference(
    config_root: &Path,
    library_file: &str,
    config: &ThirdPartyInference<'_>,
) -> Result<PathBuf, CoreError> {
    let path = config_root
        .join(THIRD_PARTY_LIBRARY_DIR)
        .join(validated_library_file(library_file)?);
    let key = config.api_key.expose_secret().to_owned();
    superai_config::json::edit(&path, |map| {
        map.insert(
            "inferenceGatewayBaseUrl".to_owned(),
            Value::String(config.gateway_base_url.to_owned()),
        );
        map.insert("inferenceGatewayApiKey".to_owned(), Value::String(key));
        map.insert(
            "inferenceGatewayAuthScheme".to_owned(),
            Value::String(config.auth_scheme.as_str().to_owned()),
        );
        map.insert(
            "inferenceProvider".to_owned(),
            Value::String(config.provider.to_owned()),
        );
        let models: Vec<Value> = config
            .models
            .iter()
            .map(|name| serde_json::json!({ "name": name }))
            .collect();
        map.insert("inferenceModels".to_owned(), Value::Array(models));
    })
    .map_err(CoreError::Config)?;
    harden_file_permissions(&path)?;
    Ok(path)
}

/// A plain file name: rejecting separators, drive colons, parent components,
/// and DOS device basenames keeps the write inside `configLibrary`.
fn validated_library_file(library_file: &str) -> Result<&str, CoreError> {
    if is_windows_device_basename(library_file) {
        return Err(CoreError::InvalidPath {
            kind: "library_file".to_owned(),
            value: library_file.to_owned(),
            reason: "reserved as a Windows device name".to_owned(),
        });
    }
    let is_plain = !library_file.is_empty()
        && !library_file.contains(['/', '\\', ':'])
        && library_file != "."
        && library_file != ".."
        && Path::new(library_file)
            .file_name()
            .is_some_and(|n| n == library_file);
    if is_plain {
        Ok(library_file)
    } else {
        Err(CoreError::InvalidPath {
            kind: "library_file".to_owned(),
            value: library_file.to_owned(),
            reason: "must be a plain file name inside configLibrary".to_owned(),
        })
    }
}

/// DOS device basenames (`CON`, `COM1`, ...) resolve to devices on Windows
/// in any case or extension; the key is the stem before the first dot.
fn is_windows_device_basename(name: &str) -> bool {
    let stem = name.split_once('.').map_or(name, |(s, _)| s);
    let key = stem.trim_end_matches(['.', ' ', '\t']).to_ascii_lowercase();
    let numbered = |prefix: &str| {
        key.strip_prefix(prefix).is_some_and(|digits| {
            digits.len() == 1
                && digits
                    .as_bytes()
                    .first()
                    .is_some_and(|&b| b.is_ascii_digit() && b != b'0')
        })
    };
    matches!(key.as_str(), "aux" | "con" | "nul" | "prn") || numbered("com") || numbered("lpt")
}

/// Unix: 0600 because the file carries the gateway key; Windows already
/// creates it user-only. Same treatment as the provider key sink.
#[cfg(unix)]
fn harden_file_permissions(path: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::PermissionsExt as _;
    let perm = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perm).map_err(|e| CoreError::InvalidPath {
        kind: "permissions".to_owned(),
        value: path.display().to_string(),
        reason: format!("cannot set 0o600: {e}"),
    })
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "no-op off unix; callers keep the Result contract"
)]
fn harden_file_permissions(_path: &Path) -> Result<(), CoreError> {
    Ok(())
}

/// Concrete adapter for Claude Desktop.
#[derive(Debug, Clone)]
pub struct ClaudeDesktopAdapter {
    id: HarnessId,
}

impl ClaudeDesktopAdapter {
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

    fn find_binary_in_path() -> Option<PathBuf> {
        super::find_in_path(&[EXECUTABLE])
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
        } else {
            stdout.into_owned()
        };
        harvest_version_token(&combined)
    }

    /// Per-OS default root; the app has no relocation env var, so this is the only root.
    fn default_config_root() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        if cfg!(target_os = "macos") {
            Some(
                PathBuf::from(&home)
                    .join("Library")
                    .join("Application Support")
                    .join("Claude"),
            )
        } else if cfg!(windows) {
            if let Ok(appdata) = std::env::var("APPDATA")
                && !appdata.trim().is_empty()
            {
                return Some(PathBuf::from(appdata).join("Claude"));
            }
            Some(
                PathBuf::from(&home)
                    .join("AppData")
                    .join("Roaming")
                    .join("Claude"),
            )
        } else {
            Some(PathBuf::from(&home).join(".config").join("Claude"))
        }
    }

    fn personal_skills_root() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".claude").join("skills"))
    }

    #[expect(clippy::excessive_nesting, reason = "evidence branches explicit")]
    fn collect_config_evidence(evidence: &mut Vec<String>) {
        match Self::default_config_root() {
            Some(root) => {
                let config = root.join(CONFIG_FILE);
                if config.exists() {
                    evidence.push(format!("config exists at {}", config.display()));
                    if let Ok(text) = std::fs::read_to_string(&config)
                        && text.contains("\"mcpServers\"")
                    {
                        evidence.push("config contains mcpServers".to_owned());
                    }
                } else if root.exists() {
                    evidence.push(format!(
                        "app-support root exists without {} at {}",
                        CONFIG_FILE,
                        root.display()
                    ));
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
                let logs = root.join("logs");
                if logs.exists() {
                    evidence.push(format!("logs dir exists at {}", logs.display()));
                }
            }
            None => evidence.push("could not resolve home for config root".to_owned()),
        }
        if let Some(skills) = Self::personal_skills_root()
            && skills.exists()
        {
            evidence.push(format!(
                "personal skills shared with claude-code at {}",
                skills.display()
            ));
        }
        evidence.push(NO_RELOCATION_NOTE.to_owned());
    }
}

fn harvest_version_token(output: &str) -> Option<String> {
    for token in output.split_whitespace() {
        let candidate = token.strip_prefix('v').unwrap_or(token);
        let has_dot = candidate.contains('.');
        let starts_digit = candidate.chars().next().is_some_and(|c| c.is_ascii_digit());
        let version_like = candidate
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if has_dot && starts_digit && version_like {
            return Some(candidate.to_owned());
        }
    }
    None
}

impl Default for ClaudeDesktopAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "claude-desktop is static valid")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for ClaudeDesktopAdapter {
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
        match Self::find_binary_in_path() {
            Some(path) => {
                evidence.push(format!(
                    "found binary `{EXECUTABLE}` at {} (GUI binary name otherwise unverified)",
                    path.display()
                ));
                match Self::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `--version`"));
                        version = Some(v);
                    }
                    None => evidence.push(
                        "version probe failed (no documented --version for the GUI binary)"
                            .to_owned(),
                    ),
                }
                binary_path = Some(path);
            }
            None => {
                evidence.push(format!("binary `{EXECUTABLE}` not found in PATH"));
            }
        }
        Self::collect_config_evidence(&mut evidence);
        // GUI apps often expose no binary on PATH; config-root evidence
        // alone counts as a low-confidence present.
        let config_seen = evidence.iter().any(|e| e.contains("config exists"));
        let present = match (&binary_path, &version) {
            (Some(_), Some(_)) => InstallPresence::Present,
            (Some(_), None) => InstallPresence::UnknownVersion,
            (None, _) if config_seen => InstallPresence::Present,
            (None, _) => InstallPresence::Absent,
        };
        let confidence = match (&binary_path, config_seen) {
            (Some(_), false) => DetectionConfidence::Medium,
            (None, true) => DetectionConfidence::Low,
            (Some(_), true) | (None, false) => DetectionConfidence::High,
        };
        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution(&self) -> VersionResolution {
        let detection = self.detection();
        if let Some(v) = detection.version {
            let notes = vec![
                format!("detected claude-desktop version {v} (date-stamped build scheme)"),
                format!("mapped to schema version {SCHEMA_VERSION_STR}"),
            ];
            let mut res =
                VersionResolution::new(Some(v), Some(SCHEMA_VERSION_STR.to_owned()), true);
            res.notes = notes;
            res
        } else if detection.present == InstallPresence::Present {
            let mut res = VersionResolution::new(None, Some(SCHEMA_VERSION_STR.to_owned()), true);
            res.notes = detection.evidence;
            res.notes.push(format!(
                "config-present detection; schema {SCHEMA_VERSION_STR} is the documented mcpServers shape"
            ));
            res
        } else {
            let mut res = VersionResolution::unknown();
            res.notes = detection.evidence;
            res
        }
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let config_resolver = PathResolver::new(
            Some(&format!("{LINUX_CONFIG_ROOT}/{CONFIG_FILE}")),
            Some(&format!("{MACOS_CONFIG_ROOT}/{CONFIG_FILE}")),
            Some(&format!("{WINDOWS_CONFIG_ROOT}\\{CONFIG_FILE}")),
            &format!("{MACOS_CONFIG_ROOT}/{CONFIG_FILE}"),
        );
        let mut config = ConfigSurface::new(
            CONFIG_FILE,
            config_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        config.precedence = 10;
        config.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        config.backup_required = true;
        config.restart_behavior = RestartBehavior::Restart;
        surfaces.push(config);

        let logs_resolver = PathResolver::new(
            Some("~/.config/Claude/logs (mcp.log, mcp-server-<name>.log)"),
            Some("~/Library/Logs/Claude (mcp.log, mcp-server-<name>.log)"),
            Some("%APPDATA%\\Claude\\logs"),
            "~/Library/Logs/Claude",
        );
        let mut logs = ConfigSurface::new(
            "logs",
            logs_resolver,
            DocumentKind::TextFragment,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        logs.precedence = 0;
        logs.backup_required = false;
        logs.restart_behavior = RestartBehavior::None;
        surfaces.push(logs);

        let skills_resolver = PathResolver::fallback_only(&format!(
            "{PERSONAL_SKILLS_PATH}/<name>/ (SHARED with claude-code)"
        ));
        let mut skills = ConfigSurface::new(
            "personal skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills.precedence = 5;
        skills.backup_required = false;
        skills.restart_behavior = RestartBehavior::Reload;
        surfaces.push(skills);

        let managed_resolver = PathResolver::new(
            Some("/etc/claude-desktop/managed-settings.json"),
            Some("/Library/Managed Preferences/<user>/com.anthropic.claudefordesktop.plist"),
            Some("HKLM\\SOFTWARE\\Policies\\Claude"),
            "3P/enterprise managed settings (out of alias scope)",
        );
        let mut managed = ConfigSurface::new(
            "managed-settings (3P/enterprise)",
            managed_resolver,
            DocumentKind::Json,
            ConfigScope::SystemManaged,
            SurfaceOwnership::HarnessManaged,
        );
        managed.precedence = 1;
        managed.backup_required = false;
        managed.restart_behavior = RestartBehavior::None;
        surfaces.push(managed);

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
            "*.log".to_owned(),
            "cache/*".to_owned(),
            "tmp/*".to_owned(),
            "Cache".to_owned(),
            "Code Cache".to_owned(),
        ]
    }

    /// The app reads one hardcoded per-OS path; no env var points elsewhere.
    /// The alias core refuses aliasing on exactly this empty env set.
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
            WrapperPlan::new("fixed default root: no relocation mechanism (verified-absent)");
        plan.description = format!(
            " claude-desktop reads hardcoded per-OS app-support paths \
             (macOS {MACOS_CONFIG_ROOT}, Windows {WINDOWS_CONFIG_ROOT}, Linux \
             {LINUX_CONFIG_ROOT}); {NO_RELOCATION_NOTE}; config_root {} is \
             informative only: no env vars are set and aliasing is refused \
             upstream (claude-desktop.md sections 4-5)",
            instance.config_root
        );
        plan.shared_state_warnings = vec![
            "extension (.mcpb) secrets live in the OS keychain, shared across every profile"
                .to_owned(),
            "personal skills at ~/.claude/skills are shared with the claude-code harness"
                .to_owned(),
            "on Windows the app's Claude Code sessions run inside a WSL2 distro (per-distro state)"
                .to_owned(),
        ];
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            format!("{MACOS_CONFIG_ROOT}/{CONFIG_FILE}"),
            format!("{WINDOWS_CONFIG_ROOT}\\{CONFIG_FILE}"),
            format!("{LINUX_CONFIG_ROOT}/{CONFIG_FILE}"),
            format!("{PERSONAL_SKILLS_PATH} (shared with claude-code)"),
            "/etc/claude-desktop/managed-settings.json (3P/enterprise)".to_owned(),
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
            Isolation::FixedPathSingle | Isolation::RelocatedRoot | Isolation::Unknown => {
                crate::adapter::validate_instance_surfaces(self, instance.config_root.as_path())
            }
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "claude-desktop has no relocation mechanism; isolation is \
                     fixed_path_single at the default root, got {other}"
                ),
            }),
        }
    }

    fn surface_schema(&self, surface_id: &str) -> Option<SurfaceSchema> {
        // Only the documented `mcpServers` shape is modeled; the full
        // consumer schema (beyond mcpServers) is unpublished.
        match surface_id {
            CONFIG_FILE => Some(
                SurfaceSchema::new()
                    .with_root_shape(RootShape::Object)
                    .with_owned_key("mcpServers", ValueType::Object),
            ),
            _ => None,
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        vec![
            crate::adapter::SkillMode::LinkAll,
            crate::adapter::SkillMode::LinkSelected,
            crate::adapter::SkillMode::CopySelected,
        ]
    }

    /// Remote connectors are UI-managed and intentionally not modeled here.
    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            CONFIG_FILE,
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::User,
            RestartBehavior::Restart,
        ))
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "desktop extensions (.mcpb) exist but the per-OS install directory is not \
             published (claude-desktop.md section 3); no honest staging target",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONFIG_FILE, ClaudeDesktopAdapter, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR,
        LINUX_CONFIG_ROOT, MACOS_CONFIG_ROOT, NO_RELOCATION_NOTE, OWNED_SELECTORS, RESEARCH_DOC,
        WINDOWS_CONFIG_ROOT,
    };
    use crate::adapter::{
        Adapter, ConfigScope, DocumentKind, ProductStatus, RestartBehavior, SurfaceOwnership,
    };
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> ClaudeDesktopAdapter {
        ClaudeDesktopAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-claude-desktop-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::FixedPathSingle,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-09-18T00:00:00Z".to_owned(),
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
        assert_eq!(a.last_verified_date(), "2026-09-18");
    }

    #[test]
    fn detection_returns_evidence_and_honesty_lines() {
        let a = adapter();
        let r = a.detection();
        assert!(!r.evidence.is_empty());
        // The verified-absent relocation note is part of the evidence so
        // callers never guess a CLAUDE_DESKTOP_CONFIG_DIR.
        assert!(
            r.evidence
                .iter()
                .any(|e| e.contains("no config-relocation")),
            "evidence must carry the no-relocation note: {:?}",
            r.evidence
        );
        match r.present {
            InstallPresence::Absent => assert!(r.version.is_none()),
            InstallPresence::Present | InstallPresence::UnknownVersion => {
                assert!(!r.evidence.is_empty());
            }
            InstallPresence::Broken => {}
        }
    }

    #[test]
    fn version_tokens_harvest_date_stamped_builds() {
        assert_eq!(
            super::harvest_version_token("Claude Desktop v1.2581.0").as_deref(),
            Some("1.2581.0")
        );
        assert_eq!(
            super::harvest_version_token("1.46388.1").as_deref(),
            Some("1.46388.1")
        );
        assert_eq!(super::harvest_version_token("not a version"), None);
        assert_eq!(super::harvest_version_token(""), None);
    }

    #[test]
    fn config_surfaces_are_platform_gated_without_env_relocation() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let config = surfaces
            .iter()
            .find(|s| s.id == CONFIG_FILE)
            .expect("claude_desktop_config.json surface");
        assert_eq!(config.kind, DocumentKind::Json);
        assert_eq!(config.scope, ConfigScope::User);
        assert_eq!(config.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(config.restart_behavior, RestartBehavior::Restart);
        assert!(config.backup_required);
        for sel in OWNED_SELECTORS {
            assert!(config.owned_selectors.contains(&(*sel).to_owned()));
        }
        // Platform-gated resolvers name the three documented roots and never
        // invent an env var.
        let hints = [
            config.path_resolver.linux.as_deref(),
            config.path_resolver.macos.as_deref(),
            config.path_resolver.windows.as_deref(),
            Some(config.path_resolver.fallback.as_str()),
        ];
        for hint in hints.into_iter().flatten() {
            assert!(
                !hint.contains("CLAUDE_DESKTOP"),
                "no fabricated env: {hint}"
            );
        }
        assert_eq!(
            config.path_resolver.linux.as_deref(),
            Some(format!("{LINUX_CONFIG_ROOT}/{CONFIG_FILE}").as_str())
        );
        assert_eq!(
            config.path_resolver.macos.as_deref(),
            Some(format!("{MACOS_CONFIG_ROOT}/{CONFIG_FILE}").as_str())
        );
        assert_eq!(
            config.path_resolver.windows.as_deref(),
            Some(format!("{WINDOWS_CONFIG_ROOT}\\{CONFIG_FILE}").as_str())
        );
        assert!(surfaces.iter().any(|s| s.id == "personal skills"));
    }

    #[test]
    fn mcp_decl_is_writable_json_with_restart() {
        let a = adapter();
        let decl = a
            .mcp_decl()
            .expect("claude-desktop models a writable MCP dest");
        assert_eq!(decl.dest_file, CONFIG_FILE);
        assert_eq!(decl.dest_key, "mcpServers");
        assert_eq!(decl.kind, DocumentKind::Json);
        assert!(decl.read_only.is_none(), "consumer config is writable");
        assert_eq!(decl.restart, RestartBehavior::Restart);
        assert!(a.plugin_absence_reason().is_some_and(|r| !r.is_empty()));
        assert!(a.mcp_absence_reason().is_none());
    }

    #[test]
    fn plugin_absence_cites_unpublished_install_dir() {
        let a = adapter();
        let reason = a.plugin_absence_reason().unwrap();
        assert!(
            reason.contains("install directory is not published"),
            "absence must cite the .mcpb gap: {reason}"
        );
    }

    #[test]
    fn supported_operations_constrained() {
        let a = adapter();
        let ops = a.supported_operations();
        assert!(!ops.is_empty());
        for (_, support) in ops {
            assert_eq!(support, AdapterSupport::Constrained);
        }
    }

    /// Alias-contract pin: no relocation env vars exist to declare, so
    /// `alias::create_alias` refuses; the honest outcome, not a missing feature.
    #[test]
    fn plan_wrapper_declares_no_relocation_env_vars() {
        let a = adapter();
        let inst =
            sample_instance_with_root(&crate::test_util::tmp_abs_str(".claude-desktop-work"));
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars.is_empty(),
            "no relocation env vars may be fabricated: {:?}",
            plan.env_vars
        );
        assert!(!plan.description.is_empty());
        assert!(
            plan.description
                .contains(NO_RELOCATION_NOTE.split(':').next().unwrap_or("")),
            "description must state the verified absence: {}",
            plan.description
        );
        assert!(
            !plan.shared_state_warnings.is_empty(),
            "keychain/skills/WSL shared state must be warned about"
        );
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".cd-work"));
        inst.harness = HarnessId::new("claude-code").unwrap();
        match a.plan_wrapper(&inst).unwrap_err() {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("expected harness validation, got {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_cover_all_three_platforms() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("Library/Application Support"))
        );
        assert!(candidates.iter().any(|c| c.contains("%APPDATA%")));
        assert!(candidates.iter().any(|c| c.contains(".config/Claude")));
    }

    #[test]
    fn validate_instance_accepts_fixed_path_and_relocated() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".cd-work"));
        a.validate_instance(&inst).unwrap();
        inst.isolation = Isolation::RelocatedRoot;
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".cd-work"));
        inst.isolation = Isolation::EnvOnly;
        match a.validate_instance(&inst).unwrap_err() {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("expected isolation validation, got {other:?}"),
        }
    }

    #[test]
    fn adapter_is_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
        assert!(!boxed.config_surfaces().is_empty());
    }

    #[test]
    fn fixture_corpus_validity_and_secret_free() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/claude_desktop");
        let report = crate::verification::fixture_report(&path);
        assert!(report.validity_pass, "claude_desktop corpus validity");
        assert!(report.secret_free_pass, "claude_desktop corpus secret-free");
        let populated = path.join("claude_desktop_config.populated.json");
        assert!(
            populated.exists(),
            "fixture missing: {}",
            populated.display()
        );
        let value = superai_config::json::load(&populated).unwrap();
        let servers = value.get("mcpServers").unwrap();
        assert!(servers.as_object().is_some_and(|m| !m.is_empty()));
    }

    /// Corpus satisfies the declared root shape + owned-key rule; a non-object root is rejected.
    #[test]
    fn surface_schema_accepts_corpus_and_rejects_bad_root() {
        let a = adapter();
        let corpus = b"{\"mcpServers\": {\"x\": {\"command\": \"c\"}}}";
        let diags = crate::adapter::validate_surface_content(
            &a,
            CONFIG_FILE,
            corpus,
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(diags.is_empty(), "corpus shape must validate: {diags:?}");
        let bad_root = b"[1, 2, 3]";
        let diags = crate::adapter::validate_surface_content(
            &a,
            CONFIG_FILE,
            bad_root,
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("root must be") && d.message.contains("object")),
            "non-object root must be rejected: {diags:?}"
        );
    }

    #[test]
    fn third_party_surface_declares_official_keys_and_roots() {
        let surface = super::third_party_inference_surface();
        assert_eq!(surface.kind, DocumentKind::Json);
        assert_eq!(surface.scope, ConfigScope::User);
        assert_eq!(surface.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(surface.restart_behavior, RestartBehavior::Restart);
        assert!(surface.backup_required);
        for key in super::THIRD_PARTY_OWNED_SELECTORS {
            assert!(
                surface.owned_selectors.contains(&(*key).to_owned()),
                "owned selector {key} missing"
            );
        }
        // The three officially documented local roots (Claude-3p siblings of
        // the 1P roots) and the configLibrary dir; never the 1P root.
        let hints = [
            surface.path_resolver.linux.as_deref(),
            surface.path_resolver.macos.as_deref(),
            surface.path_resolver.windows.as_deref(),
        ];
        for hint in hints.into_iter().flatten() {
            assert!(hint.contains("Claude-3p"), "3P root missing: {hint}");
            assert!(
                hint.contains(super::THIRD_PARTY_LIBRARY_DIR),
                "configLibrary missing: {hint}"
            );
            assert!(!hint.contains("Claude-3p-3p"), "{hint}");
        }
        // The declaration is NOT part of config_surfaces() (pinned 1P
        // partition): it is reached explicitly with parameterized roots.
        let a = adapter();
        assert!(
            !a.config_surfaces().iter().any(|s| s.id == surface.id),
            "the 3P surface is a separate fixed root, not under config_root"
        );
    }

    #[test]
    fn third_party_write_preserves_foreign_keys_and_hardens() {
        let root = crate::test_util::temp_dir_unique("claude-3p-write");
        let library = root.join(super::THIRD_PARTY_LIBRARY_DIR);
        std::fs::create_dir_all(&library).unwrap();
        let file = library.join(super::THIRD_PARTY_LIBRARY_FILE);
        std::fs::write(
            &file,
            serde_json::to_string_pretty(&serde_json::json!({
                "inferenceStreamIdleTimeoutSec": 600,
                "coworkEgressAllowedHosts": ["example.com"],
            }))
            .unwrap(),
        )
        .unwrap();

        let key = crate::error::RedactedString::new("dummy-gateway-token");
        let config = super::ThirdPartyInference::gateway(
            "http://127.0.0.1:8787",
            &key,
            super::ThirdPartyAuthScheme::Bearer,
        )
        .with_models(vec!["gateway-default".to_owned()]);
        let written =
            super::commit_third_party_inference(&root, super::THIRD_PARTY_LIBRARY_FILE, &config)
                .unwrap();
        assert_eq!(written, file);

        let value = superai_config::json::load(&file).unwrap();
        assert_eq!(
            value.get("inferenceGatewayBaseUrl"),
            Some(&serde_json::json!("http://127.0.0.1:8787"))
        );
        assert_eq!(
            value.get("inferenceGatewayApiKey"),
            Some(&serde_json::json!("dummy-gateway-token"))
        );
        assert_eq!(
            value.get("inferenceGatewayAuthScheme"),
            Some(&serde_json::json!("bearer"))
        );
        assert_eq!(
            value.get("inferenceProvider"),
            Some(&serde_json::json!("gateway"))
        );
        assert_eq!(
            value.get("inferenceModels"),
            Some(&serde_json::json!([{"name": "gateway-default"}]))
        );
        assert_eq!(
            value.get("inferenceStreamIdleTimeoutSec"),
            Some(&serde_json::json!(600))
        );
        assert_eq!(
            value.get("coworkEgressAllowedHosts"),
            Some(&serde_json::json!(["example.com"]))
        );
        assert!(superai_config::backup::list_backups(&file).is_ok_and(|b| !b.is_empty()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "gateway key file must be 0600");
        }

        let xapikey = super::ThirdPartyInference::gateway(
            "http://127.0.0.1:8787",
            &key,
            super::ThirdPartyAuthScheme::XApiKey,
        );
        super::commit_third_party_inference(&root, super::THIRD_PARTY_LIBRARY_FILE, &xapikey)
            .unwrap();
        let value = superai_config::json::load(&file).unwrap();
        assert_eq!(
            value.get("inferenceGatewayAuthScheme"),
            Some(&serde_json::json!("x-api-key"))
        );
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn third_party_library_file_traversal_is_refused() {
        let root = crate::test_util::temp_dir_unique("claude-3p-traversal");
        std::fs::create_dir_all(&root).unwrap();
        let key = crate::error::RedactedString::new("dummy-gateway-token");
        let config = super::ThirdPartyInference::gateway(
            "http://127.0.0.1:8787",
            &key,
            super::ThirdPartyAuthScheme::Bearer,
        );
        for escape in ["../escape.json", "a/b.json", "..", "", "C:con"] {
            let err = super::commit_third_party_inference(&root, escape, &config).unwrap_err();
            assert!(
                matches!(err, CoreError::InvalidPath { .. }),
                "{escape}: {err:?}"
            );
        }
        assert!(!root.join("escape.json").exists());
        assert!(
            superai_config::json::load(&root.join("configLibrary").join("inference.json"))
                .is_ok_and(|m| m.is_empty())
        );
        drop(std::fs::remove_dir_all(&root));
    }

    /// DOS device basenames resolve to devices on Windows with any
    /// extension and any case; nearby ordinary names stay legal.
    #[test]
    fn windows_device_basenames_are_matched_case_folded_with_extensions() {
        for name in [
            "con",
            "CON",
            "Con",
            "cOn.json",
            "aux.cfg",
            "nul",
            "nul.tar.gz",
            "prn.",
            "com3 .txt",
            "com1",
            "COM9.data",
            "lpt1",
            "LPT9",
        ] {
            assert!(super::is_windows_device_basename(name), "{name}");
        }
        for name in [
            "config.json",
            "console.log",
            "com",
            "com0",
            "com10",
            "lpt0",
            "lptx",
            "nully",
            "constants.json",
            "auxiliary",
        ] {
            assert!(!super::is_windows_device_basename(name), "{name}");
        }
    }

    /// Writing a 3P library file named after a Windows device is refused
    /// before any file is created.
    #[test]
    fn third_party_library_file_windows_device_names_are_refused() {
        let root = crate::test_util::temp_dir_unique("claude-3p-devices");
        std::fs::create_dir_all(&root).unwrap();
        let key = crate::error::RedactedString::new("dummy-gateway-token");
        let config = super::ThirdPartyInference::gateway(
            "http://127.0.0.1:8787",
            &key,
            super::ThirdPartyAuthScheme::Bearer,
        );
        for name in ["con", "CON.json", "aux", "com1.cfg", "lpt9"] {
            let err = super::commit_third_party_inference(&root, name, &config).unwrap_err();
            assert!(
                matches!(err, CoreError::InvalidPath { .. }),
                "{name}: {err:?}"
            );
            assert!(
                !root.join("configLibrary").join(name).exists(),
                "{name} must not be written"
            );
        }
        drop(std::fs::remove_dir_all(&root));
    }

    /// Run-5 area-A reachability: the 3P keys reach the fixed path through a
    /// symlink-swap profile, all on fake roots.
    #[test]
    fn third_party_keys_reach_the_fixed_path_via_a_profile_swap() {
        use crate::profile;
        let base = crate::test_util::temp_dir_unique("claude-3p-profile-base");
        let record = profile::create_profile(
            &base,
            &profile::ProfileSpec::new(
                HarnessId::new(HARNESS_ID_STR).unwrap(),
                InstanceName::new("gatewayed").unwrap(),
            ),
        )
        .unwrap();
        let key = crate::error::RedactedString::new("dummy-gateway-token");
        let config = super::ThirdPartyInference::gateway(
            "http://127.0.0.1:8787",
            &key,
            super::ThirdPartyAuthScheme::Bearer,
        )
        .with_models(vec!["gateway-default".to_owned()]);
        super::commit_third_party_inference(
            record.root.as_path(),
            super::THIRD_PARTY_LIBRARY_FILE,
            &config,
        )
        .unwrap();

        // Swap the profile in at a FAKE Claude-3p path (never the real home)
        // and read the keys back through it.
        let fake_fixed = crate::test_util::temp_dir_unique("claude-3p-fake-root");
        let fixed_path = fake_fixed.join(".config").join("Claude-3p");
        profile::activate_profile(
            &base,
            &HarnessId::new(HARNESS_ID_STR).unwrap(),
            "gatewayed",
            &fixed_path,
        )
        .unwrap();
        let through_path = fixed_path
            .join(super::THIRD_PARTY_LIBRARY_DIR)
            .join(super::THIRD_PARTY_LIBRARY_FILE);
        let value = superai_config::json::load(&through_path).unwrap();
        assert_eq!(
            value.get("inferenceGatewayBaseUrl"),
            Some(&serde_json::json!("http://127.0.0.1:8787"))
        );
        assert_eq!(
            value.get("inferenceGatewayApiKey"),
            Some(&serde_json::json!("dummy-gateway-token"))
        );

        // Deactivate removes the swap; the fixed path is gone again (there
        // was no pre-existing content to restore).
        profile::deactivate_profile(&base, &HarnessId::new(HARNESS_ID_STR).unwrap(), &fixed_path)
            .unwrap();
        assert!(!fixed_path.exists(), "swap removed");
        drop(std::fs::remove_dir_all(&base));
        drop(std::fs::remove_dir_all(&fake_fixed));
    }
}
