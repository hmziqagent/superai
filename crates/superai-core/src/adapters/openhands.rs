//! `OpenHands` adapter — V0 TOML vs V1 env/Docker persistence.
//!
//! Research source: `docs/harness-configs/openhands.md` (last verified 2026-08-25).
//! Executable `openhands`, V0 `config.toml` TOML (core/llm/agent/sandbox/security),
//! V1 `~/.openhands/agent_settings.json` JSON + env `LLM_*`/`OH_PERSISTENCE_DIR`/
//! Docker sandbox isolation `os_bound`, support `Constrained` version split requiring
//! both schemas, product status `active`.

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

/// Harness identifier for `OpenHands`.
pub const HARNESS_ID_STR: &str = "openhands";

/// Human display name.
pub const DISPLAY_NAME: &str = "OpenHands";

/// Primary executable name.
pub const EXECUTABLE: &str = "openhands";

/// Alternative binary name (legacy CLI).
pub const EXECUTABLE_ALT: &str = "openhands-cli";

/// Environment variable that relocates persistence root (V1).
pub const PERSISTENCE_ENV_VAR: &str = "OH_PERSISTENCE_DIR";

/// LLM env vars (V1, require --override-with-envs).
pub const LLM_MODEL_ENV_VAR: &str = "LLM_MODEL";
/// LLM API key env.
pub const LLM_API_KEY_ENV_VAR: &str = "LLM_API_KEY";
/// LLM base URL env.
pub const LLM_BASE_URL_ENV_VAR: &str = "LLM_BASE_URL";

/// Default persistence root fallback.
pub const DEFAULT_PERSISTENCE_FALLBACK: &str = "~/.openhands";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/openhands.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current config shape (V1+ V0 combined).
pub const SCHEMA_VERSION_STR: &str = "1";

/// V1 schema marker version split note.
pub const VERSION_SPLIT_NOTE: &str = "OpenHands V0 config.toml (TOML, ./config.toml or ~/.openhands/config.toml, sections [core]/[llm]/[agent]/[sandbox]) vs V1 agent_settings.json + env (OH_PERSISTENCE_DIR, LLM_MODEL/API_KEY/BASE_URL, SANDBOX_VOLUMES) with Docker isolation — both schemas must be owned for constrained writes";

/// Owned selectors for provider/model mutation — V0 TOML keys and V1 JSON paths.
///
/// V0: `llm.model`, `llm.api_key`, `llm.base_url`, `core.runtime`, `core.max_iterations`, `sandbox.*`
/// V1: `llm.model`, `llm.api_key`, `llm.base_url` inside `agent_settings.json`
pub const OWNED_SELECTORS: &[&str] = &[
    "llm.model",
    "llm.api_key",
    "llm.base_url",
    "llm.temperature",
    "llm.ollama_base_url",
    "core.runtime",
    "core.max_iterations",
    "core.default_agent",
    "sandbox.base_container_image",
    "sandbox.timeout",
    "sandbox.user_id",
    "sandbox.volumes",
    "agent.enable_browsing",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for `OpenHands` (`Constrained`, `os_bound`).
///
/// Isolation is `os_bound` via Docker (`runtime = docker`) plus V1
/// `OH_PERSISTENCE_DIR` relocation and V0 per-directory `config.toml`.
/// Constrained because version split requires owning both TOML and JSON
/// schemas, and Docker mounts plus GUI session state make full isolation
/// container-scoped.
#[derive(Debug, Clone)]
pub struct OpenHandsAdapter {
    id: HarnessId,
}

impl OpenHandsAdapter {
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

    /// Persistence env var.
    pub fn persistence_env_var(&self) -> &str {
        PERSISTENCE_ENV_VAR
    }

    /// Version split note.
    pub fn version_split_note(&self) -> &str {
        VERSION_SPLIT_NOTE
    }

    /// Try to locate the `openhands` binary via `PATH`.
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

    /// Probe `openhands --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `openhands 1.8.0` or `0.44.0` into `0.44.0`.
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

    /// Resolve the default persistence root: `$OH_PERSISTENCE_DIR` or `~/.openhands`.
    fn default_persistence_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(PERSISTENCE_ENV_VAR)
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
        Some(PathBuf::from(home).join(".openhands"))
    }

    /// Build detection evidence about V0/V1 surfaces and Docker persistence.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("constrained: {VERSION_SPLIT_NOTE}"));
        match Self::default_persistence_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("persistence root exists at {}", root.display()));
                    let v1_settings = root.join("agent_settings.json");
                    if v1_settings.exists() {
                        evidence.push(format!(
                            "V1 agent_settings.json found at {}",
                            v1_settings.display()
                        ));
                        if let Ok(text) = std::fs::read_to_string(&v1_settings)
                            && (text.contains("\"llm\"") || text.contains("model"))
                        {
                            evidence.push("agent_settings.json contains llm/model".to_owned());
                        }
                    } else {
                        evidence.push(format!(
                            "V1 agent_settings.json missing at {}",
                            v1_settings.display()
                        ));
                    }
                    let v0_global = root.join("config.toml");
                    if v0_global.exists() {
                        evidence.push(format!("V0 config.toml found at {}", v0_global.display()));
                        if let Ok(text) = std::fs::read_to_string(&v0_global)
                            && (text.contains("[llm]") || text.contains("[core]"))
                        {
                            evidence.push("V0 config.toml contains [llm]/[core]".to_owned());
                        }
                    } else {
                        evidence.push(format!("V0 config.toml missing at {}", v0_global.display()));
                    }
                    let cli_cfg = root.join("cli_config.json");
                    if cli_cfg.exists() {
                        evidence.push(format!("cli_config.json present at {}", cli_cfg.display()));
                    }
                    let mcp = root.join("mcp.json");
                    if mcp.exists() {
                        evidence.push(format!("mcp.json present at {}", mcp.display()));
                    }
                    let conversations = root.join("conversations");
                    if conversations.exists() {
                        evidence.push(format!(
                            "conversations dir present at {}",
                            conversations.display()
                        ));
                    }
                } else {
                    evidence.push(format!("persistence root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve persistence root (no HOME)".to_owned());
            }
        }
        // V0 per-directory config.toml
        let cwd_v0 = Path::new("config.toml");
        if cwd_v0.exists() {
            evidence.push(format!("cwd V0 config.toml found at {}", cwd_v0.display()));
        }
        let cwd_v0_local = Path::new("./config.toml");
        if cwd_v0_local.exists() && cwd_v0_local != cwd_v0 {
            evidence.push(format!(
                "cwd V0 config.toml found at {}",
                cwd_v0_local.display()
            ));
        }
        for var in [
            PERSISTENCE_ENV_VAR,
            LLM_MODEL_ENV_VAR,
            LLM_API_KEY_ENV_VAR,
            LLM_BASE_URL_ENV_VAR,
            "SANDBOX_VOLUMES",
            "SANDBOX_USER_ID",
            "RUNTIME",
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
        // Docker socket heuristic
        if Path::new("/var/run/docker.sock").exists() {
            evidence
                .push("docker socket present at /var/run/docker.sock (sandbox docker)".to_owned());
        }
    }
}

impl Default for OpenHandsAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "openhands is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for OpenHandsAdapter {
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
                .any(|e| e.contains("persistence root exists")),
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
            notes.push(format!("detected openhands version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            notes.push(format!("split: {VERSION_SPLIT_NOTE}"));
            // Heuristic: versions < 1.0 are V0 TOML era, >=1.0 V1 env/JSON
            if v.starts_with("0.") {
                notes.push("version 0.x suggests V0 config.toml era".to_owned());
            } else {
                notes.push(
                    "version 1.x+ suggests V1 agent_settings.json + OH_PERSISTENCE_DIR".to_owned(),
                );
            }
            let mut res =
                VersionResolution::new(Some(v), Some(SCHEMA_VERSION_STR.to_owned()), true);
            res.notes = notes;
            res
        } else {
            let mut res = VersionResolution::unknown();
            res.notes = detection.evidence;
            res.notes.push(format!("split: {VERSION_SPLIT_NOTE}"));
            res
        }
    }

    #[expect(clippy::too_many_lines, reason = "surfaces are declarative")]
    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let v0_resolver = PathResolver::new(
            Some("./config.toml then $OH_PERSISTENCE_DIR/config.toml"),
            Some("./config.toml then $OH_PERSISTENCE_DIR/config.toml"),
            Some("config.toml then %OH_PERSISTENCE_DIR%\\config.toml"),
            "./config.toml (cwd) then ~/.openhands/config.toml (V0)",
        );
        let mut v0_toml = ConfigSurface::new(
            "config.toml (V0)",
            v0_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        v0_toml.precedence = 8;
        v0_toml.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        v0_toml.backup_required = true;
        v0_toml.restart_behavior = RestartBehavior::Reload;
        surfaces.push(v0_toml);

        let cwd_v0_resolver = PathResolver::fallback_only("./config.toml (project V0)");
        let mut cwd_v0 = ConfigSurface::new(
            "config.toml (project)",
            cwd_v0_resolver,
            DocumentKind::Toml,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        cwd_v0.precedence = 10;
        cwd_v0.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        cwd_v0.backup_required = true;
        surfaces.push(cwd_v0);

        let v1_resolver = PathResolver::new(
            Some("$OH_PERSISTENCE_DIR/agent_settings.json"),
            Some("$OH_PERSISTENCE_DIR/agent_settings.json"),
            Some("%OH_PERSISTENCE_DIR%\\agent_settings.json"),
            "~/.openhands/agent_settings.json (V1)",
        );
        let mut v1_settings = ConfigSurface::new(
            "agent_settings.json (V1)",
            v1_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        v1_settings.precedence = 12;
        v1_settings.owned_selectors = vec![
            "llm.model".to_owned(),
            "llm.api_key".to_owned(),
            "llm.base_url".to_owned(),
            "llm.temperature".to_owned(),
            "sandbox.base_container_image".to_owned(),
        ];
        v1_settings.backup_required = true;
        v1_settings.restart_behavior = RestartBehavior::Reload;
        surfaces.push(v1_settings);

        let cli_resolver = PathResolver::new(
            Some("$OH_PERSISTENCE_DIR/cli_config.json"),
            Some("$OH_PERSISTENCE_DIR/cli_config.json"),
            Some("%OH_PERSISTENCE_DIR%\\cli_config.json"),
            "~/.openhands/cli_config.json",
        );
        let mut cli_cfg = ConfigSurface::new(
            "cli_config.json",
            cli_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        cli_cfg.precedence = 6;
        cli_cfg.backup_required = false;
        surfaces.push(cli_cfg);

        let mcp_resolver = PathResolver::new(
            Some("$OH_PERSISTENCE_DIR/mcp.json"),
            Some("$OH_PERSISTENCE_DIR/mcp.json"),
            Some("%OH_PERSISTENCE_DIR%\\mcp.json"),
            "~/.openhands/mcp.json",
        );
        let mut mcp = ConfigSurface::new(
            "mcp.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp.precedence = 11;
        mcp.owned_selectors = vec!["mcpServers".to_owned()];
        mcp.backup_required = true;
        surfaces.push(mcp);

        let env_resolver = PathResolver::new(
            Some("$LLM_MODEL/$LLM_API_KEY/$LLM_BASE_URL (--override-with-envs)"),
            Some("$LLM_MODEL/$LLM_API_KEY/$LLM_BASE_URL (--override-with-envs)"),
            Some("%LLM_MODEL%/%LLM_API_KEY%/%LLM_BASE_URL%"),
            "$LLM_MODEL/$LLM_API_KEY/$LLM_BASE_URL + $OH_PERSISTENCE_DIR + $SANDBOX_VOLUMES (env, session)",
        );
        let mut env_surface = ConfigSurface::new(
            "env (LLM_* + OH_*)",
            env_resolver,
            DocumentKind::Env,
            ConfigScope::SessionInline,
            SurfaceOwnership::ExternalSecretStore,
        );
        env_surface.precedence = 20;
        env_surface.owned_selectors = vec![
            "LLM_MODEL".to_owned(),
            "LLM_API_KEY".to_owned(),
            "LLM_BASE_URL".to_owned(),
            "OH_PERSISTENCE_DIR".to_owned(),
            "SANDBOX_VOLUMES".to_owned(),
            "SANDBOX_USER_ID".to_owned(),
            "RUNTIME".to_owned(),
        ];
        env_surface.backup_required = false;
        env_surface.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(env_surface);

        let conv_resolver = PathResolver::new(
            Some("$OH_PERSISTENCE_DIR/conversations/<id>/"),
            Some("$OH_PERSISTENCE_DIR/conversations/<id>/"),
            Some("%OH_PERSISTENCE_DIR%\\conversations\\<id>\\"),
            "~/.openhands/conversations/",
        );
        let mut conv = ConfigSurface::new(
            "conversations",
            conv_resolver,
            DocumentKind::Opaque,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        conv.precedence = 0;
        conv.backup_required = false;
        conv.restart_behavior = RestartBehavior::None;
        surfaces.push(conv);

        let microagents_resolver =
            PathResolver::fallback_only("microagents/ (.openhands/microagents/)");
        let mut microagents = ConfigSurface::new(
            "microagents",
            microagents_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        microagents.precedence = 14;
        microagents.backup_required = false;
        surfaces.push(microagents);

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
            "conversations/*".to_owned(),
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "workspace/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "tmp/*".to_owned(),
            "*.lock".to_owned(),
            "file_store/*".to_owned(),
            "trajectories/*".to_owned(),
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
        let mut plan = WrapperPlan::new("os_bound via OH_PERSISTENCE_DIR + LLM_* + Docker");
        plan.env_vars.push((
            PERSISTENCE_ENV_VAR.to_owned(),
            instance.config_root.to_string(),
        ));
        // V1 env overrides are session-inline and require --override-with-envs; we expose them as wrapper env hints.
        // The actual values are provider/template driven; wrapper sets persistence deterministically.
        plan.env_vars
            .push(("RUNTIME".to_owned(), "docker".to_owned()));
        let runtime_image = "ghcr.io/openhands/agent-server:1.26.0-python";
        plan.env_vars.push((
            "AGENT_SERVER_IMAGE_REPOSITORY".to_owned(),
            "ghcr.io/openhands/agent-server".to_owned(),
        ));
        plan.env_vars.push((
            "AGENT_SERVER_IMAGE_TAG".to_owned(),
            "1.26.0-python".to_owned(),
        ));
        plan.description = format!(
            " Wrapper sets {}={} RUNTIME=docker AGENT_SERVER_IMAGE_REPOSITORY/TAG={runtime_image} (V0 {} fallback to cwd config.toml, V1 --override-with-envs for LLM_*)",
            PERSISTENCE_ENV_VAR, instance.config_root, "config.toml"
        );
        // For headless wrappers, caller should add --override-with-envs and LLM_* exports;
        // docs note V0 per-directory config.toml is `cd` isolated, V1 is HOME/OH_PERSISTENCE_DIR isolated.
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.openhands/agent_settings.json".to_owned(),
            "~/.openhands/config.toml".to_owned(),
            "~/.openhands/cli_config.json".to_owned(),
            "~/.openhands/mcp.json".to_owned(),
            "./config.toml".to_owned(),
            "$OH_PERSISTENCE_DIR/agent_settings.json via OH_PERSISTENCE_DIR".to_owned(),
            "/var/run/docker.sock (docker runtime)".to_owned(),
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
            Isolation::OsBound
            | Isolation::Unknown
            | Isolation::EnvOnly
            | Isolation::RelocatedRoot => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "openhands requires isolation os_bound (Docker + OH_PERSISTENCE_DIR), got {other} — {VERSION_SPLIT_NOTE}"
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
        DEFAULT_PERSISTENCE_FALLBACK, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, OWNED_SELECTORS,
        OpenHandsAdapter, PERSISTENCE_ENV_VAR, RESEARCH_DOC, VERSION_SPLIT_NOTE,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> OpenHandsAdapter {
        OpenHandsAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-openhands-1").unwrap(),
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
        assert_eq!(a.persistence_env_var(), PERSISTENCE_ENV_VAR);
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert_eq!(a.adapter_revision(), crate::adapter::ADAPTER_REVISION);
        assert!(a.version_split_note().contains("V0"));
        assert!(a.version_split_note().contains("V1"));
        assert!(VERSION_SPLIT_NOTE.contains("Docker"));
        assert_eq!(DEFAULT_PERSISTENCE_FALLBACK, "~/.openhands");
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
    fn version_resolution_maps_detected_with_split() {
        let a = adapter();
        let res = a.version_resolution();
        if res.detected_version.is_some() {
            assert_eq!(
                res.schema_version.as_deref(),
                Some(super::SCHEMA_VERSION_STR)
            );
            assert!(res.compatible);
            assert!(
                res.notes
                    .iter()
                    .any(|n| n.contains("V0") || n.contains("V1"))
            );
        } else {
            assert!(!res.compatible);
            assert!(res.schema_version.is_none());
            assert!(res.notes.iter().any(|n| n.contains("split")));
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("openhands 1.8.0", Some("1.8.0")),
            ("openhands 0.44.0", Some("0.44.0")),
            ("0.44.0", Some("0.44.0")),
            ("v1.0.0", Some("1.0.0")),
            ("Version: 2.0.0", Some("2.0.0")),
            ("1.26.0-python", Some("1.26.0-python")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = OpenHandsAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_v0_and_v1() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 6);
        let v0 = surfaces
            .iter()
            .find(|s| s.id == "config.toml (V0)")
            .expect("V0 config.toml must exist");
        assert_eq!(v0.kind, DocumentKind::Toml);
        assert_eq!(v0.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(v0.scope, ConfigScope::User);
        assert!(v0.backup_required);
        for sel in OWNED_SELECTORS {
            assert!(v0.owned_selectors.contains(&(*sel).to_owned()));
        }
        let v1 = surfaces
            .iter()
            .find(|s| s.id == "agent_settings.json (V1)")
            .expect("V1 agent_settings.json must exist");
        assert_eq!(v1.kind, DocumentKind::Json);
        assert_eq!(v1.scope, ConfigScope::User);
        assert!(v1.owned_selectors.contains(&"llm.model".to_owned()));
        let env = surfaces
            .iter()
            .find(|s| s.id == "env (LLM_* + OH_*)")
            .expect("env surface must exist");
        assert_eq!(env.kind, DocumentKind::Env);
        assert_eq!(env.scope, ConfigScope::SessionInline);
        assert_eq!(env.ownership, SurfaceOwnership::ExternalSecretStore);
    }

    #[test]
    fn owned_selectors_are_stable_and_cover_v0_v1() {
        assert!(OWNED_SELECTORS.len() >= 8);
        let set: HashSet<&str> = OWNED_SELECTORS.iter().copied().collect();
        assert_eq!(set.len(), OWNED_SELECTORS.len(), "selectors must be unique");
        for required in ["llm.model", "llm.api_key", "llm.base_url", "core.runtime"] {
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
    fn plan_mirror_exclusions_cover_conversations_and_cache() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        for pat in ["conversations/*", "cache/*", "workspace/*", "*.lock"] {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"agent_settings.json (V1)".to_owned()));
    }

    #[test]
    fn plan_wrapper_sets_persistence_and_runtime() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.openhands-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == PERSISTENCE_ENV_VAR && v == "/tmp/.openhands-work")
        );
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == "RUNTIME" && v == "docker")
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(PERSISTENCE_ENV_VAR));
        assert!(plan.description.contains("V0"));
        assert!(plan.description.contains("V1"));
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my openhands work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == PERSISTENCE_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, "/tmp/my openhands work");
        assert!(env_val.contains(' '));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.openhands-work");
        inst.harness = HarnessId::new("claude-code").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_v0_v1_and_docker() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.contains("agent_settings.json")));
        assert!(candidates.iter().any(|c| c.contains("config.toml")));
        assert!(candidates.iter().any(|c| c.contains(PERSISTENCE_ENV_VAR)));
        assert!(candidates.iter().any(|c| c.contains("docker.sock")));
    }

    #[test]
    fn validate_instance_accepts_os_bound() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.openhands-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_accepts_env_only_for_cli() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.openhands-work");
        inst.isolation = Isolation::EnvOnly;
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.openhands-work");
        inst.isolation = Isolation::FixedPathSingle;
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
