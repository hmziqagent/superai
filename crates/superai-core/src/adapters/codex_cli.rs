//! Codex CLI adapter — relocated-root via `CODEX_HOME` with profile isolation.
//!
//! Research source: `docs/harness-configs/codex-cli.md` (last verified 2026-08-25).
//! Executable `codex`, config root `~/.codex` or `$CODEX_HOME`, primary
//! writable surface `config.toml` (TOML), isolation `relocated-root` plus
//! profile files `$CODEX_HOME/<name>.config.toml` (>=0.134).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use toml_edit as _;

use superai_config::document::ValueType;

use crate::adapter::{
    ADAPTER_REVISION, Adapter, Arch, ConfigScope, ConfigSurface, DetectionConfidence,
    DetectionResult, DocumentKind, Os, PathResolver, Platform, ProductStatus, RestartBehavior,
    RootShape, SurfaceOwnership, SurfaceSchema, VersionResolution, WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, InstallPresence, Isolation};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Harness identifier for Codex CLI.
pub const HARNESS_ID_STR: &str = "codex-cli";

/// Human display name.
pub const DISPLAY_NAME: &str = "Codex CLI";

/// Primary executable name.
pub const EXECUTABLE: &str = "codex";

/// Environment variable that relocates the config root.
pub const CONFIG_ENV_VAR: &str = "CODEX_HOME";

/// Default config root when `CODEX_HOME` is unset.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.codex";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/codex-cli.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current config shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider/model mutation — instance-specific fields.
///
/// These are the selectors superai owns inside `config.toml`. Everything
/// else round-trips untouched via `superai-config::toml_file` (`toml_edit`).
pub const OWNED_SELECTORS: &[&str] = &[
    "model",
    "model_provider",
    "model_providers",
    "openai_base_url",
    "model_reasoning_effort",
    "model_verbosity",
    "model_context_window",
    "approval_policy",
    "sandbox_mode",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Codex CLI.
///
/// Isolation is `relocated-root` via `CODEX_HOME`. The wrapper sets
/// `CODEX_HOME` to the instance `config_root` and execs `codex`. Profile
/// isolation (>=0.134) is via `$CODEX_HOME/<name>.config.toml` selected
/// with `codex --profile <name>`, shared under the same relocated root.
#[derive(Debug, Clone)]
pub struct CodexCliAdapter {
    id: HarnessId,
}

impl CodexCliAdapter {
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

    /// Try to locate the `codex` binary via `PATH`.
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

    /// Probe `codex --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `codex-cli 0.134.0` or `0.135.1` into `0.135.1`.
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

    /// Resolve the default config root: `$CODEX_HOME` or `~/.codex`.
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
        Some(PathBuf::from(home).join(".codex"))
    }

    /// Check if default config root exists on disk.
    #[expect(dead_code, reason = "helper for future use")]
    #[expect(clippy::unused_self, reason = "adapter helper")]
    fn default_config_root_exists(&self) -> Option<PathBuf> {
        let root = Self::default_config_root()?;
        if root.exists() { Some(root) } else { None }
    }

    /// Build the config.toml path for a given config root.
    fn config_path_for_root(root: &Path) -> PathBuf {
        root.join("config.toml")
    }

    /// Build a profile config path for a given root and profile name.
    #[expect(dead_code, reason = "helper for future use")]
    fn profile_path_for_root(root: &Path, profile: &str) -> PathBuf {
        root.join(format!("{profile}.config.toml"))
    }

    /// Build detection evidence about config root and TOML config.
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
                    let config = Self::config_path_for_root(&root);
                    if config.exists() {
                        evidence.push(format!("config.toml found at {}", config.display()));
                        if let Ok(text) = std::fs::read_to_string(&config) {
                            if text.contains("model =") || text.contains("model_provider") {
                                evidence.push("config.toml contains model keys".to_owned());
                            }
                            if text.contains("[model_providers") {
                                evidence.push("config.toml contains model_providers".to_owned());
                            }
                            if text.contains("[mcp_servers") {
                                evidence.push("config.toml contains mcp_servers".to_owned());
                            }
                        }
                    } else {
                        evidence.push(format!("config.toml missing at {}", config.display()));
                    }
                    let auth = root.join("auth.json");
                    if auth.exists() {
                        evidence.push(format!("auth.json present at {}", auth.display()));
                    }
                    // Check for profile files (Codex >=0.134)
                    if let Ok(entries) = std::fs::read_dir(&root) {
                        let mut profile_count = 0;
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                                && name.ends_with(".config.toml")
                                && name != "config.toml"
                            {
                                profile_count += 1;
                            }
                        }
                        if profile_count > 0 {
                            evidence.push(format!(
                                "found {profile_count} profile config(s) in {}",
                                root.display()
                            ));
                        }
                    }
                    let skills = root.join("skills");
                    if skills.exists() {
                        evidence.push(format!("skills dir present at {}", skills.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
    }
}

impl Default for CodexCliAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "codex-cli is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for CodexCliAdapter {
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
            notes.push(format!("detected codex version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            // Detect profile era: >=0.134 uses separate profile files.
            let era_note = if is_profile_era(&v) {
                "profile era >=0.134: separate $CODEX_HOME/<name>.config.toml"
            } else {
                "legacy era <0.134: inline [profiles.*] in config.toml"
            };
            notes.push(era_note.to_owned());
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

        // Primary writable surface: config.toml under CODEX_HOME or ~/.codex.
        let config_resolver = PathResolver::new(
            Some("$CODEX_HOME/config.toml"),
            Some("$CODEX_HOME/config.toml"),
            Some("%CODEX_HOME%\\config.toml"),
            "~/.codex/config.toml",
        );
        let mut config_surface = ConfigSurface::new(
            "config.toml",
            config_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        config_surface.precedence = 10;
        config_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        config_surface.backup_required = true;
        config_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(config_surface);

        // Profile surface: $CODEX_HOME/<name>.config.toml (>=0.134)
        let profile_resolver = PathResolver::new(
            Some("$CODEX_HOME/<name>.config.toml"),
            Some("$CODEX_HOME/<name>.config.toml"),
            Some("%CODEX_HOME%\\<name>.config.toml"),
            "~/.codex/<name>.config.toml",
        );
        let mut profile_surface = ConfigSurface::new(
            "profile.config.toml",
            profile_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        profile_surface.precedence = 20;
        profile_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        profile_surface.backup_required = true;
        profile_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(profile_surface);

        // Auth surface: auth.json — external secret store, not writable for provider.
        let auth_resolver = PathResolver::new(
            Some("$CODEX_HOME/auth.json"),
            Some("$CODEX_HOME/auth.json"),
            Some("%CODEX_HOME%\\auth.json"),
            "~/.codex/auth.json",
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

        // Skills surface: SKILL.md files — text fragments.
        let skills_resolver = PathResolver::new(
            Some("$CODEX_HOME/skills/<name>/SKILL.md"),
            Some("$CODEX_HOME/skills/<name>/SKILL.md"),
            Some("%CODEX_HOME%\\skills\\<name>\\SKILL.md"),
            "~/.codex/skills/<name>/SKILL.md",
        );
        let mut skills = ConfigSurface::new(
            "skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills.precedence = 15;
        skills.backup_required = false;
        surfaces.push(skills);

        // MCP is inside config.toml under [mcp_servers.*], but also track as logical surface.
        // We do not add a separate file surface for MCP since it shares config.toml.

        // Project-local config: .codex/config.toml (trusted projects only, read-only for provider keys)
        let project_resolver =
            PathResolver::fallback_only(".codex/config.toml (project root, trusted only)");
        let mut project_surface = ConfigSurface::new(
            "project.config.toml",
            project_resolver,
            DocumentKind::Toml,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_surface.precedence = 30;
        project_surface.backup_required = false;
        project_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(project_surface);

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
            "history.jsonl".to_owned(),
            "log/*".to_owned(),
            "sessions/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "auth.json".to_owned(),
            "*.lock".to_owned(),
            "logs/*".to_owned(),
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
        let mut plan = WrapperPlan::new("relocated-root via CODEX_HOME");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        plan.description = format!(
            " Wrapper sets {}={} and execs `{}`",
            CONFIG_ENV_VAR, instance.config_root, EXECUTABLE
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.codex".to_owned(),
            "~/.codex-work".to_owned(),
            "$CODEX_HOME".to_owned(),
            "~/.codex favor via CODEX_HOME relocation".to_owned(),
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
            Isolation::RelocatedRoot | Isolation::Unknown => {
                // HAD-03: surface content present under the instance root must
                // satisfy the declared root shape / owned-key rules.
                crate::adapter::validate_instance_surfaces(self, instance.config_root.as_path())
            }
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("codex-cli requires isolation relocated_root, got {other}"),
            }),
        }
    }

    fn surface_schema(&self, surface_id: &str) -> Option<SurfaceSchema> {
        // HAD-03: `config.toml` and the >=0.134 per-profile files share the
        // same top-level shape (docs/harness-configs/codex-cli.md §profiles:
        // "Use top-level keys in the profile file; do NOT nest under
        // [profiles.<name>]").
        match surface_id {
            "config.toml" | "profile.config.toml" => Some(
                SurfaceSchema::new()
                    .with_root_shape(RootShape::Table)
                    .with_owned_key("model", ValueType::String)
                    .with_owned_key("model_provider", ValueType::String)
                    .with_owned_key("openai_base_url", ValueType::String)
                    .with_owned_key("model_reasoning_effort", ValueType::String)
                    .with_owned_key("model_verbosity", ValueType::String)
                    .with_owned_key("approval_policy", ValueType::String)
                    .with_owned_key("sandbox_mode", ValueType::String)
                    .with_owned_key("model_providers", ValueType::Object)
                    .with_owned_key("mcp_servers", ValueType::Object)
                    // Legacy top-level profile selector: no longer supported
                    // in the >=0.134 profile-file era (research doc §profiles).
                    .with_deprecated(
                        "profile",
                        Some("per-profile $CODEX_HOME/<name>.config.toml via --profile".to_owned()),
                    ),
            ),
            _ => None,
        }
    }

    fn era_conflict_reason(&self, surface_id: &str, content: &[u8]) -> Option<String> {
        if surface_id != "config.toml" {
            return None;
        }
        let resolution = self.version_resolution();
        let Some(version) = resolution.detected_version.as_deref() else {
            // Unknown era already blocks via the version gate.
            return None;
        };
        profile_era_conflict(version, content)
    }

    fn capability_declarations(&self) -> Vec<crate::adapter::AdapterCapabilityDecl> {
        vec![
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::WebSearch,
                crate::capability::Support::Native,
                "web search tool on the responses/chat APIs",
            ),
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::Vision,
                crate::capability::Support::Native,
                "image input on the responses transport",
            ),
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::ComputerUse,
                crate::capability::Support::Absent,
                "no computer-use loop in the CLI harness",
            ),
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::Mcp,
                crate::capability::Support::Native,
                "mcp_servers table in config.toml",
            ),
        ]
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        vec![
            crate::adapter::SkillMode::LinkSelected,
            crate::adapter::SkillMode::CopySelected,
        ]
    }

    /// EXT-08/09: MCP destination (codex-cli.md MCP servers: `[mcp_servers.<name>]` TOML tables)
    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            "config.toml",
            "mcp_servers",
            DocumentKind::Toml,
            ConfigScope::User,
            RestartBehavior::Restart,
        ))
    }

    /// EXT-06: explicit plugin-mechanism absence (corpus-grounded).
    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some("codex documents no plugin mechanism (codex-cli.md)")
    }
}

/// Config era of codex `config.toml` content (HAD-05).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigEra {
    /// >=0.134: profiles are separate `$CODEX_HOME/<name>.config.toml` files.
    ProfileFile,
    /// <0.134: inline `[profiles.*]` tables inside `config.toml`.
    InlineProfiles,
    /// Content carries no era marker.
    Unknown,
}

/// Classify the config era of raw `config.toml` content.
///
/// The documented marker is the legacy inline `[profiles.<name>]` table
/// (research doc §profiles: legacy (<0.134) uses inline tables; >=0.134 uses
/// separate files, so current-era configs never contain `[profiles.`).
pub fn config_era(content: &[u8]) -> ConfigEra {
    let text = String::from_utf8_lossy(content);
    if text.contains("[profiles.") {
        ConfigEra::InlineProfiles
    } else {
        ConfigEra::Unknown
    }
}

/// Pure era-conflict check for `config.toml` content against a detected
/// version (HAD-05 step 5).
///
/// The documented era marker is the legacy-only inline `[profiles.*]` table;
/// current-era configs carry no marker, so the detectable conflict is a
/// profile-file-era version (>=0.134) paired with legacy inline content.
pub fn profile_era_conflict(version: &str, content: &[u8]) -> Option<String> {
    if is_profile_era(version) && config_era(content) == ConfigEra::InlineProfiles {
        return Some(format!(
            "detected codex {version} uses the >=0.134 profile-file era but the config carries legacy inline [profiles.*] tables; migrate before writing"
        ));
    }
    None
}

/// Determine if version is in profile era (>=0.134.0).
fn is_profile_era(version: &str) -> bool {
    // Parse leading numeric semver. If parsing fails, assume legacy to be safe.
    let mut parts = version.split('.');
    let major = parts
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let minor = parts
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let patch_str = parts.next().unwrap_or("0");
    // Strip suffix like -alpha, +build.
    let _patch_clean = patch_str.split(['-', '+']).next().unwrap_or("0");
    if major > 0 {
        return true;
    }
    if minor > 134 {
        return true;
    }
    if minor == 134 {
        return true;
    }
    if minor == 0 && major == 0 {
        // Handle case like "0.134.0" correctly.
        // The above already handles, but keep fallback.
        return minor >= 134;
    }
    false
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{
        CONFIG_ENV_VAR, CodexCliAdapter, ConfigEra, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR,
        LAST_VERIFIED, OWNED_SELECTORS, RESEARCH_DOC, SCHEMA_VERSION_STR, config_era,
        is_profile_era, profile_era_conflict,
    };
    use toml_edit as _;

    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> CodexCliAdapter {
        CodexCliAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-codex-1").unwrap(),
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
                assert!(!result.evidence.is_empty());
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
        let cases = vec![
            ("codex-cli 0.134.0", Some("0.134.0")),
            ("codex 0.135.1", Some("0.135.1")),
            ("v0.134.0", Some("0.134.0")),
            ("Version: 0.136.0", Some("0.136.0")),
            ("0.134.0-alpha", Some("0.134.0-alpha")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = CodexCliAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn profile_era_detection() {
        assert!(is_profile_era("0.134.0"));
        assert!(is_profile_era("0.135.0"));
        assert!(is_profile_era("1.0.0"));
        assert!(!is_profile_era("0.133.9"));
        assert!(!is_profile_era("0.100.0"));
        assert!(is_profile_era("0.134.0-alpha"));
    }

    #[test]
    fn config_surfaces_include_writable_toml() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let config = surfaces
            .iter()
            .find(|s| s.id == "config.toml")
            .expect("config.toml surface must exist");
        assert_eq!(config.kind, DocumentKind::Toml);
        assert_eq!(config.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(config.scope, ConfigScope::User);
        assert!(config.backup_required);
        for selector in ["model", "model_provider", "openai_base_url"] {
            assert!(
                config.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in &config.owned_selectors {
            assert!(!sel.is_empty());
        }
        for sel in OWNED_SELECTORS {
            assert!(config.owned_selectors.contains(&(*sel).to_owned()));
        }

        let auth = surfaces
            .iter()
            .find(|s| s.id == "auth.json")
            .expect("auth.json surface must exist");
        assert_eq!(auth.ownership, SurfaceOwnership::ExternalSecretStore);
        assert!(!auth.backup_required);

        let skills = surfaces.iter().find(|s| s.id == "skills").expect("skills");
        assert_eq!(skills.kind, DocumentKind::TextFragment);

        let profile = surfaces
            .iter()
            .find(|s| s.id == "profile.config.toml")
            .expect("profile surface");
        assert_eq!(profile.kind, DocumentKind::Toml);
        assert!(profile.backup_required);
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
    fn plan_mirror_exclusions_cover_history_and_locks() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        let must_contain = ["history.jsonl", "log/*", "sessions/*", "cache/*", "*.log"];
        for pat in must_contain {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"config.toml".to_owned()));
    }

    #[test]
    #[expect(clippy::excessive_nesting, reason = "test closure nesting is explicit")]
    fn plan_mirror_includes_config_and_excludes_sessions() {
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
        assert!(!is_excluded("config.toml"));
        assert!(!is_excluded("skills/my-skill/SKILL.md"));
        assert!(is_excluded("sessions/abc.jsonl"));
        assert!(is_excluded("history.jsonl"));
        assert!(is_excluded("log/tui.log"));
    }

    #[test]
    fn plan_wrapper_sets_codex_home() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.codex-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == "/tmp/.codex-work")
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(CONFIG_ENV_VAR));
        assert!(plan.args.is_empty());
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my codex work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == CONFIG_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, "/tmp/my codex work");
        assert!(!env_val.contains('"'));
        assert!(env_val.contains(' '));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.codex-work");
        inst.harness = HarnessId::new("claude-code").unwrap();
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
        assert!(candidates.iter().any(|c| c.contains(".codex")));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.codex-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.codex-work");
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
        let mut inst = sample_instance_with_root("/tmp/.codex-work");
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let config = surfaces.iter().find(|s| s.id == "config.toml").unwrap();
        assert_eq!(config.path_resolver.fallback, "~/.codex/config.toml");
        let resolver = &config.path_resolver;
        assert!(resolver.linux.as_deref().unwrap().contains(CONFIG_ENV_VAR));
        assert!(resolver.macos.as_deref().unwrap().contains(CONFIG_ENV_VAR));
        assert!(
            resolver
                .windows
                .as_deref()
                .unwrap()
                .contains(CONFIG_ENV_VAR)
        );
    }

    // -----------------------------------------------------------------------
    // Fixture-backed conformance tests
    // -----------------------------------------------------------------------

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/codex_cli")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.toml");
        let doc = superai_config::toml_file::load(&path).unwrap();
        assert!(doc.is_empty());
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("config.minimal.toml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let doc = superai_config::toml_file::load(&path).unwrap();
        // Minimal may be empty or contain only a comment.
        assert!(doc.is_empty() || doc.to_string().contains("model") || doc.to_string().is_empty());
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("config.populated.toml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let doc = superai_config::toml_file::load(&path).unwrap();
        let text = doc.to_string();
        assert!(
            text.contains("model")
                || text.contains("model_provider")
                || text.contains("model_providers"),
            "populated must contain model keys: {text}"
        );
        if text.contains("[model_providers") {
            assert!(text.contains("base_url") || text.contains("env_key"));
        }
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("config.foreign.toml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original_text = std::fs::read_to_string(&path).unwrap();
        assert!(
            original_text.contains("foreignKey") || original_text.contains("customField"),
            "foreign fixture must contain custom keys"
        );
        let dir = crate::test_util::temp_dir_unique("codex");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("config.foreign.copy.toml");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::toml_file::edit(&tmp, |doc| {
            doc["model"] = toml_edit::value("gpt-5.5");
            assert!(
                doc.to_string().contains("foreignKey") || doc.to_string().contains("customField"),
                "foreign keys must survive in loaded doc"
            );
        })
        .unwrap();
        let after = std::fs::read_to_string(&tmp).unwrap();
        assert!(after.contains("gpt-5.5"));
        let foreign_preserved = after.contains("foreignKey")
            || after.contains("customField")
            || after.contains("unknownTopLevel");
        assert!(
            foreign_preserved,
            "foreign keys must be preserved, got {after:?}"
        );
        drop(std::fs::remove_file(&tmp));
    }

    #[test]
    fn fixture_malformed_fails_to_parse() {
        let path = fixture_path("config.malformed.toml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::toml_file::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn unknown_key_preservation_via_toml_edit() {
        let dir = crate::test_util::temp_dir_unique("codex");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preserve.toml");
        let original =
            "# keep me\nmodel = \"gpt-4\"\ncustomField = \"keep\"\n[foreignSection]\nkeep = true\n";
        std::fs::write(&path, original).unwrap();

        superai_config::toml_file::edit(&path, |doc| {
            doc["model"] = toml_edit::value("gpt-5.5");
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# keep me") || after.contains("keep me"));
        assert!(after.contains("customField"));
        assert!(after.contains("foreignSection"));
        assert!(after.contains("gpt-5.5"));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn provider_mutation_sets_model_and_provider() {
        let dir = crate::test_util::temp_dir_unique("codex");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("provider.toml");
        let initial = "model = \"gpt-4\"\nmodel_provider = \"openai\"\n";
        std::fs::write(&path, initial).unwrap();

        superai_config::toml_file::edit(&path, |doc| {
            doc["model"] = toml_edit::value("gpt-5.5");
            doc["model_provider"] = toml_edit::value("openrouter");
        })
        .unwrap();
        let after = superai_config::toml_file::load(&path).unwrap();
        assert_eq!(after["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(after["model_provider"].as_str(), Some("openrouter"));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn provider_removal_clears_model() {
        let dir = crate::test_util::temp_dir_unique("codex");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("remove.toml");
        let initial = "model = \"gpt-4\"\nmodel_provider = \"openai\"\nopenai_base_url = \"https://old.example.com\"\n";
        std::fs::write(&path, initial).unwrap();

        superai_config::toml_file::edit(&path, |doc| {
            doc["model"] = toml_edit::Item::None;
            doc["model_provider"] = toml_edit::Item::None;
        })
        .unwrap();
        let after = superai_config::toml_file::load(&path).unwrap();
        // model should be removed or empty
        assert!(after.get("model").is_none() || after["model"].as_str().is_none());
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn secret_redaction_placeholder() {
        use crate::error::RedactedString;
        let secret = RedactedString::new("sk-ant-secret-123");
        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert!(!debug.contains("sk-ant-secret-123"));
        assert!(!display.contains("sk-ant-secret-123"));
        assert!(debug.contains("[REDACTED]"));
        assert!(display.contains("[REDACTED]"));
        let json = serde_json::to_string(&secret).unwrap();
        assert!(!json.contains("sk-ant-secret-123"));
        assert!(json.contains("[REDACTED]"));
        assert_eq!(secret.expose_secret(), "sk-ant-secret-123");
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
        let inst = sample_instance_with_root("/tmp/.codex-work");
        a.validate_instance(&inst).unwrap();
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn wrapper_env_var_isolation_is_relocated_root() {
        let a = adapter();
        assert!(a.scan_candidates().len() >= 3);
        let inst = sample_instance_with_root("/home/user/.codex-isolated");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(!plan.env_vars.is_empty());
        let (key, val) = &plan.env_vars[0];
        assert_eq!(key, CONFIG_ENV_VAR);
        assert_eq!(val, "/home/user/.codex-isolated");
    }

    #[test]
    fn toml_comment_preservation() {
        let dir = crate::test_util::temp_dir_unique("codex");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("comment.toml");
        let content = "# Model selection\nmodel = \"gpt-4\" # inline comment\n# provider block\nmodel_provider = \"openai\"\n";
        std::fs::write(&path, content).unwrap();
        superai_config::toml_file::edit(&path, |doc| {
            doc["model"] = toml_edit::value("gpt-5.5");
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# Model selection") || after.contains("Model selection"));
        assert!(after.contains("gpt-5.5"));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn adapter_is_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
        assert!(!boxed.config_surfaces().is_empty());
        assert!(!boxed.plan_mirror_exclusions().is_empty());
    }

    // -------------------------------------------------------------------
    // HAD-03 surface schema (table root + owned-key semantics)
    // -------------------------------------------------------------------

    #[test]
    fn surface_schema_declares_table_root_and_typed_keys() {
        let a = adapter();
        let schema = a.surface_schema("config.toml").expect("config schema");
        assert_eq!(schema.root_shape, Some(crate::adapter::RootShape::Table));
        for path in ["model", "model_provider", "model_providers", "mcp_servers"] {
            assert!(
                schema.owned_key_rules.iter().any(|r| r.path == path),
                "missing rule for {path}"
            );
        }
        // Profile files share the same top-level schema.
        assert!(a.surface_schema("profile.config.toml").is_some());
        assert!(a.surface_schema("unknown").is_none());
    }

    #[test]
    fn schema_rejects_wrongly_typed_owned_key() {
        let diags = crate::adapter::validate_surface_content(
            &adapter(),
            "config.toml",
            b"model = { nested = true }\n",
            superai_config::document::DocumentKind::Toml,
        );
        assert!(
            diags.iter().any(|d| d
                .message
                .contains("owned key `model` must hold a value of type string")
                && d.message.starts_with("[codex-cli/config.toml]")),
            "diags: {diags:?}"
        );
    }

    #[test]
    fn schema_flags_deprecated_profile_selector() {
        let diags = crate::adapter::validate_surface_content(
            &adapter(),
            "config.toml",
            b"profile = \"work\"\n",
            superai_config::document::DocumentKind::Toml,
        );
        let dep = diags
            .iter()
            .find(|d| d.severity == superai_config::document::DiagnosticSeverity::Deprecation)
            .expect("deprecation diagnostic for legacy profile selector");
        assert!(dep.message.contains("`profile` is deprecated"));
        assert!(dep.message.contains("--profile"));
    }

    #[test]
    fn validate_instance_rejects_schema_invalid_config_under_root() {
        let a = adapter();
        let dir = crate::test_util::temp_dir_unique("codex-schema");
        std::fs::create_dir_all(&dir).unwrap();
        let inst = sample_instance_with_root(dir.to_str().unwrap());
        // Missing config: fresh instance validates.
        a.validate_instance(&inst).unwrap();
        std::fs::write(dir.join("config.toml"), b"model = \"gpt-5\"\n").unwrap();
        a.validate_instance(&inst).unwrap();
        std::fs::write(dir.join("config.toml"), b"model = 123\n").unwrap();
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::SchemaValidation { details, .. } => {
                assert!(details.contains("[codex-cli/config.toml]"), "{details}");
            }
            other => panic!("expected SchemaValidation, got {other:?}"),
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    // -------------------------------------------------------------------
    // HAD-05/HAD-06 version-boundary fixtures (profile era vs pre-profile)
    // -------------------------------------------------------------------

    #[test]
    fn boundary_fixtures_split_profile_eras_per_documented_boundary() {
        let legacy = fixture_path("config.boundary_legacy.toml");
        let current = fixture_path("config.boundary_current.toml");
        assert!(legacy.exists(), "missing {}", legacy.display());
        assert!(current.exists(), "missing {}", current.display());

        let legacy_bytes = std::fs::read(&legacy).unwrap();
        let current_bytes = std::fs::read(&current).unwrap();

        // Era markers per docs/harness-configs/codex-cli.md §profiles.
        assert_eq!(config_era(&legacy_bytes), ConfigEra::InlineProfiles);
        assert_eq!(config_era(&current_bytes), ConfigEra::Unknown);
        // The documented version boundary classifies the paired versions.
        assert!(!is_profile_era("0.133.9"));
        assert!(is_profile_era("0.134.0"));

        // Both eras parse and satisfy the declared schema on read.
        for content in [&legacy_bytes, &current_bytes] {
            let diags = crate::adapter::validate_surface_content(
                &adapter(),
                "config.toml",
                content,
                superai_config::document::DocumentKind::Toml,
            );
            assert!(
                diags
                    .iter()
                    .all(|d| d.severity != superai_config::document::DiagnosticSeverity::Error),
                "boundary fixture must satisfy the declared schema: {diags:?}"
            );
        }

        // The version.txt fixture records a >=0.134 detection: the current
        // fixture is in-range, the legacy fixture would be era-conflicting.
        let version_text = std::fs::read_to_string(fixture_path("version.txt"))
            .unwrap()
            .trim()
            .to_owned();
        let parsed = CodexCliAdapter::parse_version_output(&version_text);
        assert_eq!(parsed.as_deref(), Some("0.134.0"));
        assert!(is_profile_era(parsed.as_deref().unwrap_or("")));
    }

    #[test]
    fn era_classifier_markers_and_negatives() {
        assert_eq!(config_era(b"model = \"gpt-5\"\n"), ConfigEra::Unknown);
        assert_eq!(
            config_era(b"[profiles.o3]\nmodel = \"o3\"\n"),
            ConfigEra::InlineProfiles
        );
        // A comment mentioning the marker is still a marker-bearing file;
        // the classifier is lexical by design and errs toward legacy.
        assert_eq!(
            config_era(b"# see [profiles.*] docs\n"),
            ConfigEra::InlineProfiles
        );
    }

    /// Era-conflict refusal through the shared raw-editor boundary: a
    /// profile-era adapter refuses to write legacy inline-profile content,
    /// leaving the file untouched (HAD-05 step 5).
    #[test]
    fn commit_refuses_era_conflicting_content() {
        #[derive(Debug)]
        struct ProfileEraCodex;

        impl Adapter for ProfileEraCodex {
            fn id(&self) -> HarnessId {
                HarnessId::new("codex-cli").unwrap()
            }
            #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
            fn display_name(&self) -> &str {
                "Codex CLI"
            }
            fn product_status(&self) -> ProductStatus {
                ProductStatus::Active
            }
            fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
                Vec::new()
            }
            #[expect(clippy::unnecessary_literal_bound, reason = "trait requires &str")]
            fn adapter_revision(&self) -> &str {
                "0.1.0"
            }
            fn research_doc_link(&self) -> &str {
                RESEARCH_DOC
            }
            fn last_verified_date(&self) -> &str {
                LAST_VERIFIED
            }
            fn detection(&self) -> crate::adapter::DetectionResult {
                crate::adapter::DetectionResult::absent(vec!["test".to_owned()])
            }
            fn version_resolution(&self) -> crate::adapter::VersionResolution {
                crate::adapter::VersionResolution::new(
                    Some("0.134.0".to_owned()),
                    Some(SCHEMA_VERSION_STR.to_owned()),
                    true,
                )
            }
            fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
                vec![crate::adapter::ConfigSurface::new(
                    "config.toml",
                    crate::adapter::PathResolver::fallback_only("~/.codex/config.toml"),
                    DocumentKind::Toml,
                    ConfigScope::User,
                    SurfaceOwnership::UserEditable,
                )]
            }
            fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
                Vec::new()
            }
            fn plan_mirror_exclusions(&self) -> Vec<String> {
                Vec::new()
            }
            fn plan_wrapper(
                &self,
                _instance: &Instance,
            ) -> Result<crate::adapter::WrapperPlan, CoreError> {
                Ok(crate::adapter::WrapperPlan::new("test"))
            }
            fn scan_candidates(&self) -> Vec<String> {
                Vec::new()
            }
            fn validate_instance(&self, _instance: &Instance) -> Result<(), CoreError> {
                Ok(())
            }
            fn era_conflict_reason(&self, surface_id: &str, content: &[u8]) -> Option<String> {
                (surface_id == "config.toml")
                    .then(|| profile_era_conflict("0.134.0", content))
                    .flatten()
            }
        }

        let dir = crate::test_util::temp_dir_unique("codex-era");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let original: &[u8] = b"model = \"gpt-5\"\n";
        std::fs::write(&path, original).unwrap();
        let adapter = ProfileEraCodex;
        let legacy: Vec<u8> = std::fs::read(fixture_path("config.boundary_legacy.toml")).unwrap();
        let err =
            crate::raw_editor::commit_for_adapter(&path, &legacy, None, &adapter).unwrap_err();
        match err {
            CoreError::UnsupportedVersion { reason, .. } => {
                assert!(reason.contains("[profiles.*]"), "{reason}");
                assert!(reason.contains("0.134"), "{reason}");
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
        // Consistent-era content commits through the same boundary.
        let current: Vec<u8> = std::fs::read(fixture_path("config.boundary_current.toml")).unwrap();
        crate::raw_editor::commit_for_adapter(&path, &current, None, &adapter).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), current);
        drop(std::fs::remove_dir_all(&dir));
    }
}
