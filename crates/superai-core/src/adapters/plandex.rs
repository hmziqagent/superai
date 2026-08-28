//! Plandex adapter — env and server/model-pack config, `Constrained` provider/server scoped.
//!
//! Research source: `docs/harness-configs/plandex.md` (last verified 2026-08-25).
//! Executable `plandex`, env-driven providers
//! (`OPENROUTER_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`,
//!  `PLANDEX_API_HOST`/`PLANDEX_ENV` plus `PLANDEX_BASE_DIR`/`DATABASE_URL` for
//!  self-host), custom models JSON via `plandex models custom`
//!  (`https://plandex.ai/schemas/models-input.schema.json`, `providers`/`models`/
//!  `modelPacks`), per-plan roles (`planner`/`coder`/… with temperature/strongModel
//!  fallbacks), provider precedence + `OpenRouter` failover, isolation `env_only`,
//!  support `Constrained` (provider/server scoped), product `active`.

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

/// Harness identifier for Plandex.
pub const HARNESS_ID_STR: &str = "plandex";

/// Human display name.
pub const DISPLAY_NAME: &str = "Plandex";

/// Primary executable name.
pub const EXECUTABLE: &str = "plandex";

/// Env var for API host override.
pub const API_HOST_ENV_VAR: &str = "PLANDEX_API_HOST";

/// Env var for env selection.
pub const ENV_ENV_VAR: &str = "PLANDEX_ENV";

/// Provider env vars (CLI direct).
pub const OPENROUTER_API_KEY_ENV_VAR: &str = "OPENROUTER_API_KEY";
/// `OpenAI` direct.
pub const OPENAI_API_KEY_ENV_VAR: &str = "OPENAI_API_KEY";
/// Anthropic direct.
pub const ANTHROPIC_API_KEY_ENV_VAR: &str = "ANTHROPIC_API_KEY";
/// Gemini direct.
pub const GEMINI_API_KEY_ENV_VAR: &str = "GEMINI_API_KEY";

/// Server base dir (self-host).
pub const SERVER_BASE_DIR_ENV_VAR: &str = "PLANDEX_BASE_DIR";

/// Database URL (self-host).
pub const DATABASE_URL_ENV_VAR: &str = "DATABASE_URL";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/plandex.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current config shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Constrained note — provider/server scoped.
pub const CONSTRAINED_NOTE: &str = "env + server/model-pack, provider/server scoped: all-provider state is env-driven (per-instance env switching trivial), custom models JSON per-user-file (provider baseUrl/apiKeyEnvVar/skipAuth), self-hosted servers per-deploy (PLANDEX_API_HOST points CLI at any server), per-plan roles planner/coder/architect/… with modelPacks, direct-provider precedence + OpenRouter failover, self-hosted = everything / Cloud+BYO = custom models on built-in / Cloud integrated = packs of built-in only";

/// Owned selectors for provider/model-pack mutation.
/// Covers custom JSON top-level `providers`/`models`/`modelPacks` plus role keys
/// and provider env var names that superai owns.
pub const OWNED_SELECTORS: &[&str] = &[
    "providers",
    "models",
    "modelPacks",
    "planner",
    "coder",
    "architect",
    "summarizer",
    "builder",
    "wholeFileBuilder",
    "OPENROUTER_API_KEY",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "PLANDEX_API_HOST",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Plandex (`Constrained`, `env_only`).
///
/// Isolation is `env_only` (provider keys + `PLANDEX_API_HOST` per wrapper).
/// Custom models JSON is per-user-file via `plandex models custom`; self-hosted
/// servers are per-deploy with `PLANDEX_BASE_DIR`/`DATABASE_URL`/`PORT`.
#[derive(Debug, Clone)]
pub struct PlandexAdapter {
    id: HarnessId,
}

impl PlandexAdapter {
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

    /// API host env var.
    pub fn api_host_env_var(&self) -> &str {
        API_HOST_ENV_VAR
    }

    /// Constrained note.
    pub fn constrained_note(&self) -> &str {
        CONSTRAINED_NOTE
    }

    /// Try to locate the `plandex` binary via `PATH`.
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

    /// Probe `plandex --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `plandex v2.1.0` into `2.1.0`.
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

    /// Resolve the custom models JSON path: heuristic `~/.config/plandex/models.json` or `~/.plandex/models.json`.
    fn custom_models_path() -> Option<PathBuf> {
        if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"))
            && !home.trim().is_empty()
        {
            let candidate = PathBuf::from(&home)
                .join(".config")
                .join("plandex")
                .join("models.json");
            if candidate.exists() {
                return Some(candidate);
            }
            let alt = PathBuf::from(&home).join(".plandex").join("models.json");
            if alt.exists() {
                return Some(alt);
            }
            // Default to xdg path even if missing for evidence.
            return Some(candidate);
        }
        None
    }

    /// Build detection evidence about env, custom models, and server config.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("constrained: {CONSTRAINED_NOTE}"));
        // Provider envs
        for var in [
            OPENROUTER_API_KEY_ENV_VAR,
            OPENAI_API_KEY_ENV_VAR,
            ANTHROPIC_API_KEY_ENV_VAR,
            GEMINI_API_KEY_ENV_VAR,
            API_HOST_ENV_VAR,
            ENV_ENV_VAR,
            SERVER_BASE_DIR_ENV_VAR,
            DATABASE_URL_ENV_VAR,
            "GOENV",
            "PORT",
            "OLLAMA_BASE_URL",
        ] {
            if let Ok(val) = std::env::var(var)
                && !val.trim().is_empty()
            {
                let preview =
                    if var.contains("KEY") || var.contains("TOKEN") || var.contains("DATABASE_URL")
                    {
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
        match Self::custom_models_path() {
            Some(path) => {
                if path.exists() {
                    evidence.push(format!("custom models JSON found at {}", path.display()));
                    if let Ok(text) = std::fs::read_to_string(&path)
                        && (text.contains("\"providers\"") || text.contains("modelPacks"))
                    {
                        evidence
                            .push("custom models JSON contains providers/modelPacks".to_owned());
                    }
                    if let Ok(text) = std::fs::read_to_string(&path)
                        && text.contains("$schema")
                    {
                        evidence.push("custom models JSON contains $schema".to_owned());
                    }
                } else {
                    evidence.push(format!("custom models JSON missing at {}", path.display()));
                }
            }
            None => evidence.push("could not resolve custom models path (no HOME)".to_owned()),
        }
        // Provider precedence hint
        evidence.push("direct-provider keys take precedence over OpenRouter; OpenRouter failover when both set".to_owned());
        evidence.push("custom models: self-hosted=everything, Cloud+BYO=custom models on built-in, Cloud integrated=packs of built-in only".to_owned());
    }
}

impl Default for PlandexAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "plandex is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for PlandexAdapter {
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
                .any(|e| e.contains("custom models JSON found")),
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
            notes.push(format!("detected plandex version {v}"));
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

    #[expect(clippy::too_many_lines, reason = "surfaces are declarative")]
    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let env_resolver = PathResolver::new(
            Some(
                "$PLANDEX_API_HOST/$OPENROUTER_API_KEY/$OPENAI_API_KEY/$ANTHROPIC_API_KEY (env, session)",
            ),
            Some(
                "$PLANDEX_API_HOST/$OPENROUTER_API_KEY/$OPENAI_API_KEY/$ANTHROPIC_API_KEY (env, session)",
            ),
            Some("%PLANDEX_API_HOST%/%OPENROUTER_API_KEY%/%OPENAI_API_KEY%/%ANTHROPIC_API_KEY%"),
            "$PLANDEX_API_HOST + $PLANDEX_ENV + provider keys (env, session)",
        );
        let mut env_surface = ConfigSurface::new(
            "env (PLANDEX_* + provider keys)",
            env_resolver,
            DocumentKind::Env,
            ConfigScope::SessionInline,
            SurfaceOwnership::ExternalSecretStore,
        );
        env_surface.precedence = 20;
        env_surface.owned_selectors = vec![
            "PLANDEX_API_HOST".to_owned(),
            "PLANDEX_ENV".to_owned(),
            "OPENROUTER_API_KEY".to_owned(),
            "OPENAI_API_KEY".to_owned(),
            "ANTHROPIC_API_KEY".to_owned(),
            "GEMINI_API_KEY".to_owned(),
            "AZURE_OPENAI_API_KEY".to_owned(),
        ];
        env_surface.backup_required = false;
        env_surface.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(env_surface);

        let models_resolver = PathResolver::new(
            Some("~/.config/plandex/models.json (custom models, via `plandex models custom`)"),
            Some("~/.config/plandex/models.json (via `plandex models custom`)"),
            Some("%USERPROFILE%\\.config\\plandex\\models.json"),
            "~/.config/plandex/models.json (`plandex models custom`, providers/models/modelPacks)",
        );
        let mut models_surface = ConfigSurface::new(
            "custom-models.json",
            models_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        models_surface.precedence = 12;
        models_surface.owned_selectors = vec![
            "providers".to_owned(),
            "models".to_owned(),
            "modelPacks".to_owned(),
            "$schema".to_owned(),
        ];
        models_surface.backup_required = true;
        models_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(models_surface);

        let alt_models_resolver =
            PathResolver::fallback_only("~/.plandex/models.json (alt custom path)");
        let mut alt_models = ConfigSurface::new(
            "custom-models.json (alt)",
            alt_models_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        alt_models.precedence = 11;
        alt_models.owned_selectors = vec![
            "providers".to_owned(),
            "models".to_owned(),
            "modelPacks".to_owned(),
        ];
        alt_models.backup_required = true;
        surfaces.push(alt_models);

        let server_resolver = PathResolver::new(
            Some("$PLANDEX_BASE_DIR + $DATABASE_URL + $PORT + $GOENV (server, self-host)"),
            Some("$PLANDEX_BASE_DIR + $DATABASE_URL + $PORT + $GOENV (server, self-host)"),
            Some("%PLANDEX_BASE_DIR% + %DATABASE_URL% (server)"),
            "$PLANDEX_BASE_DIR + $DATABASE_URL + $PORT + $OLLAMA_BASE_URL (server env, self-host)",
        );
        let mut server_surface = ConfigSurface::new(
            "server env (self-host)",
            server_resolver,
            DocumentKind::Env,
            ConfigScope::SystemManaged,
            SurfaceOwnership::HarnessManaged,
        );
        server_surface.precedence = 5;
        server_surface.owned_selectors = vec![
            "PLANDEX_BASE_DIR".to_owned(),
            "DATABASE_URL".to_owned(),
            "PORT".to_owned(),
            "GOENV".to_owned(),
            "OLLAMA_BASE_URL".to_owned(),
        ];
        server_surface.backup_required = false;
        server_surface.restart_behavior = RestartBehavior::Restart;
        surfaces.push(server_surface);

        let roles_resolver = PathResolver::fallback_only(
            "per-plan roles JSON (planner/coder/architect/summarizer/builder via `plandex set-model --json`)",
        );
        let mut roles_surface = ConfigSurface::new(
            "per-plan roles (set-model)",
            roles_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        roles_surface.precedence = 15;
        roles_surface.owned_selectors = vec![
            "planner".to_owned(),
            "coder".to_owned(),
            "architect".to_owned(),
            "summarizer".to_owned(),
            "builder".to_owned(),
            "wholeFileBuilder".to_owned(),
        ];
        roles_surface.backup_required = true;
        surfaces.push(roles_surface);

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
            "plans/*/.plandex/*".to_owned(),
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "tmp/*".to_owned(),
            "*.lock".to_owned(),
            ".plandex/*".to_owned(),
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
            WrapperPlan::new("env_only via PLANDEX_API_HOST + provider keys, server per-deploy");
        // Per-instance API host — points CLI at isolated server (self-host per-deploy) or cloud default.
        // Use a derived localhost URL based on instance name hash for test determinism, or leave to template provider.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "name len < 100, truncation intentional for deterministic port"
        )]
        let derived_port = 8099u16 + (instance.name.as_str().len() as u16 % 100);
        let derived_host = format!("http://localhost:{derived_port}");
        plan.env_vars
            .push((API_HOST_ENV_VAR.to_owned(), derived_host));
        plan.env_vars
            .push((ENV_ENV_VAR.to_owned(), "production".to_owned()));
        // Provider keys are template/secrets driven; wrapper sets host + env marker.
        plan.description = format!(
            " Wrapper sets {API_HOST_ENV_VAR}=http://localhost:{derived_port} {ENV_ENV_VAR}=production (provider keys via template, custom models JSON per-user-file, server PLANDEX_BASE_DIR/DATABASE_URL per-deploy, {CONSTRAINED_NOTE})"
        );
        // Also expose instance isolation hint via custom models path env if needed
        plan.env_vars.push((
            "PLANDEX_MODELS_FILE".to_owned(),
            format!("{}/models.json", instance.config_root),
        ));
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/plandex/models.json".to_owned(),
            "~/.plandex/models.json".to_owned(),
            "$PLANDEX_API_HOST via PLANDEX_API_HOST".to_owned(),
            "$PLANDEX_BASE_DIR via PLANDEX_BASE_DIR (server)".to_owned(),
            "$DATABASE_URL via DATABASE_URL (server)".to_owned(),
            "./.plandex (project)".to_owned(),
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
            Isolation::EnvOnly | Isolation::Unknown | Isolation::RelocatedRoot => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "plandex requires isolation env_only (provider/server scoped), got {other} — {CONSTRAINED_NOTE}"
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
        API_HOST_ENV_VAR, CONSTRAINED_NOTE, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR,
        OWNED_SELECTORS, PlandexAdapter, RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> PlandexAdapter {
        PlandexAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-plandex-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::EnvOnly,
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
        assert_eq!(a.api_host_env_var(), API_HOST_ENV_VAR);
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert_eq!(a.adapter_revision(), crate::adapter::ADAPTER_REVISION);
        assert!(a.constrained_note().contains("env"));
        assert!(CONSTRAINED_NOTE.contains("PLANDEX_API_HOST"));
        assert!(CONSTRAINED_NOTE.contains("provider"));
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
            assert!(res.notes.iter().any(|n| n.contains("plandex")));
        } else {
            assert!(!res.compatible);
            assert!(res.schema_version.is_none());
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("plandex v2.1.0", Some("2.1.0")),
            ("plandex 2.0.1", Some("2.0.1")),
            ("v1.0.0", Some("1.0.0")),
            ("Version: 2.0.0", Some("2.0.0")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = PlandexAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_env_and_custom_models() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let env = surfaces
            .iter()
            .find(|s| s.id == "env (PLANDEX_* + provider keys)")
            .expect("env surface must exist");
        assert_eq!(env.kind, DocumentKind::Env);
        assert_eq!(env.scope, ConfigScope::SessionInline);
        assert_eq!(env.ownership, SurfaceOwnership::ExternalSecretStore);
        assert!(env.owned_selectors.contains(&"PLANDEX_API_HOST".to_owned()));
        let models = surfaces
            .iter()
            .find(|s| s.id == "custom-models.json")
            .expect("custom-models.json must exist");
        assert_eq!(models.kind, DocumentKind::Json);
        assert_eq!(models.scope, ConfigScope::User);
        assert!(models.owned_selectors.contains(&"providers".to_owned()));
        assert!(models.owned_selectors.contains(&"modelPacks".to_owned()));
        for sel in OWNED_SELECTORS {
            assert!(
                models.owned_selectors.contains(&(*sel).to_owned())
                    || env.owned_selectors.contains(&(*sel).to_owned())
                    || surfaces
                        .iter()
                        .any(|s| s.owned_selectors.contains(&(*sel).to_owned()))
            );
        }
        let server = surfaces
            .iter()
            .find(|s| s.id == "server env (self-host)")
            .expect("server env must exist");
        assert_eq!(server.scope, ConfigScope::SystemManaged);
    }

    #[test]
    fn owned_selectors_are_stable() {
        assert!(OWNED_SELECTORS.len() >= 8);
        let set: HashSet<&str> = OWNED_SELECTORS.iter().copied().collect();
        assert_eq!(set.len(), OWNED_SELECTORS.len(), "selectors must be unique");
        for required in ["providers", "models", "modelPacks", "PLANDEX_API_HOST"] {
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
            "configure_provider",
            "plan_wrapper",
        ] {
            assert!(names.contains(required), "missing op {required}");
        }
    }

    #[test]
    fn plan_mirror_exclusions_cover_plandex_dirs() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        for pat in ["cache/*", "*.lock", ".plandex/*"] {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"custom-models.json".to_owned()));
    }

    #[test]
    fn plan_wrapper_sets_api_host_and_models_file() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.plandex-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == API_HOST_ENV_VAR && v.contains("localhost"))
        );
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == "PLANDEX_MODELS_FILE" && v.contains(".plandex-work"))
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(API_HOST_ENV_VAR));
        assert!(plan.description.contains("provider"));
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my plandex work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let models_file = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == "PLANDEX_MODELS_FILE")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(models_file, "/tmp/my plandex work/models.json");
        assert!(models_file.contains(' '));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.plandex-work");
        inst.harness = HarnessId::new("claude-code").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_custom_models_and_host() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.contains("models.json")));
        assert!(candidates.iter().any(|c| c.contains(API_HOST_ENV_VAR)));
        assert!(candidates.iter().any(|c| c.contains("DATABASE_URL")));
    }

    #[test]
    fn validate_instance_accepts_env_only() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.plandex-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.plandex-work");
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
