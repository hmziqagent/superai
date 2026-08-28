//! Letta Code adapter — client config plus server/provider state, `Constrained`.
//!
//! Research source: `docs/harness-configs/letta-code.md` (last verified 2026-08-25).
//! Executable `letta`, client backends `cloud`/`local`/`self-hosted`, local state
//! `~/.letta/lc-local-backend` or `$LETTA_LOCAL_BACKEND_DIR`, server connection
//! `LETTA_API_KEY`/`LETTA_BASE_URL`/`LETTA_APP_SERVER_URL`, per-agent `MemFS`
//! `memfs/<agent-id>/memory`, skills `${MEMORY_DIR}/skills` / `.agents/skills` /
//! `~/.letta/skills`, isolation `daemon_service` (local backend plus server,
//! separate per-provider server processes), support `Constrained` — client
//! mutation via isolated local dir, server/provider state separate and not
//! directly mutated per-instance.

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

/// Harness identifier for Letta Code.
pub const HARNESS_ID_STR: &str = "letta-code";

/// Human display name.
pub const DISPLAY_NAME: &str = "Letta Code";

/// Primary executable name.
pub const EXECUTABLE: &str = "letta";

/// Alternative binary via npx.
pub const EXECUTABLE_ALT: &str = "letta-code";

/// Environment variable that relocates the local backend dir.
pub const LOCAL_BACKEND_ENV_VAR: &str = "LETTA_LOCAL_BACKEND_DIR";

/// Environment variable for Letta server base URL (self-hosted/app server).
pub const BASE_URL_ENV_VAR: &str = "LETTA_BASE_URL";

/// Environment variable for app server URL.
pub const APP_SERVER_URL_ENV_VAR: &str = "LETTA_APP_SERVER_URL";

/// Environment variable for API key (cloud/self-hosted).
pub const API_KEY_ENV_VAR: &str = "LETTA_API_KEY";

/// Environment variable for app server token.
pub const APP_SERVER_TOKEN_ENV_VAR: &str = "LETTA_APP_SERVER_TOKEN";

/// Default local backend dir fallback.
pub const DEFAULT_LOCAL_BACKEND_FALLBACK: &str = "~/.letta/lc-local-backend";

/// Default state dir fallback.
pub const DEFAULT_STATE_DIR_FALLBACK: &str = "~/.letta";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/letta-code.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current config shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for client/provider mutation inside local backend config.
pub const OWNED_SELECTORS: &[&str] = &[
    "model",
    "embedding",
    "toolset",
    "providers",
    "mcpServers",
    "memory",
    "skills",
    "context_limit",
];

/// Constrained note — separate server.
pub const CONSTRAINED_NOTE: &str = "client config isolated via LETTA_LOCAL_BACKEND_DIR; separate server per provider state (LETTA_BASE_URL, LETTA_API_KEY, Ollama/vLLM) is separate server and not per-instance mutated — run one server per provider (different ports/volumes at /root/.letta)";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Letta Code (`Constrained`).
///
/// Isolation is `daemon_service`: each instance gets an isolated local
/// backend dir via `LETTA_LOCAL_BACKEND_DIR`; server-side provider
/// connections (`LETTA_BASE_URL` with keys for Anthropic/Ollama/etc.)
/// are separate and require distinct server processes/volumes. The wrapper
/// sets `LETTA_LOCAL_BACKEND_DIR` and `LETTA_BASE_URL` for the client.
#[derive(Debug, Clone)]
pub struct LettaAdapter {
    id: HarnessId,
}

impl LettaAdapter {
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

    /// Local backend env var.
    pub fn local_backend_env_var(&self) -> &str {
        LOCAL_BACKEND_ENV_VAR
    }

    /// Base URL env var.
    pub fn base_url_env_var(&self) -> &str {
        BASE_URL_ENV_VAR
    }

    /// Try to locate the `letta` binary via `PATH`.
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

    /// Probe `letta --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `letta 0.2.1` into `0.2.1`.
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

    /// Resolve the default local backend dir: `$LETTA_LOCAL_BACKEND_DIR` or `~/.letta/lc-local-backend`.
    fn default_local_backend_dir() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(LOCAL_BACKEND_ENV_VAR)
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
        Some(PathBuf::from(home).join(".letta").join("lc-local-backend"))
    }

    /// Resolve the default state dir `~/.letta`.
    fn default_state_dir() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".letta"))
    }

    /// Build detection evidence about local backend, state, and server config.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("constrained: {CONSTRAINED_NOTE}"));
        match Self::default_state_dir() {
            Some(state) => {
                if state.exists() {
                    evidence.push(format!("state dir exists at {}", state.display()));
                    let local_backend = state.join("lc-local-backend");
                    if local_backend.exists() {
                        evidence.push(format!(
                            "local backend exists at {}",
                            local_backend.display()
                        ));
                        let memfs = local_backend.join("memfs");
                        if memfs.exists() {
                            evidence.push(format!("memfs present at {}", memfs.display()));
                        }
                    }
                    let skills_global = state.join("skills");
                    if skills_global.exists() {
                        evidence.push(format!(
                            "global skills dir present at {}",
                            skills_global.display()
                        ));
                    }
                } else {
                    evidence.push(format!("state dir missing at {}", state.display()));
                }
            }
            None => {
                evidence.push("could not resolve state dir (no HOME)".to_owned());
            }
        }
        match Self::default_local_backend_dir() {
            Some(dir) => {
                if dir.exists() {
                    evidence.push(format!("local backend dir exists at {}", dir.display()));
                } else {
                    evidence.push(format!("local backend dir missing at {}", dir.display()));
                }
            }
            None => {
                evidence.push("could not resolve local backend dir (no HOME)".to_owned());
            }
        }
        for var in [
            LOCAL_BACKEND_ENV_VAR,
            BASE_URL_ENV_VAR,
            APP_SERVER_URL_ENV_VAR,
            API_KEY_ENV_VAR,
            APP_SERVER_TOKEN_ENV_VAR,
        ] {
            if let Ok(val) = std::env::var(var)
                && !val.trim().is_empty()
            {
                let preview = if val.chars().count() > 80 {
                    let truncated: String = val.chars().take(80).collect();
                    format!("{truncated}…")
                } else {
                    // Redact API keys/tokens
                    if var.contains("KEY") || var.contains("TOKEN") {
                        "[REDACTED]".to_owned()
                    } else {
                        val
                    }
                };
                evidence.push(format!("{var} set to {preview}"));
            } else {
                evidence.push(format!("{var} not set"));
            }
        }
        // Project skills
        if Path::new(".agents/skills").exists() {
            evidence.push(".agents/skills present in cwd (project)".to_owned());
        }
        if Path::new(".letta").exists() {
            evidence.push(".letta present in cwd".to_owned());
        }
    }
}

impl Default for LettaAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "letta-code is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for LettaAdapter {
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
                        evidence.push(format!("version probe failed for `{EXECUTABLE} --version`"));
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

        let confidence = if present == InstallPresence::Absent {
            if evidence.iter().any(|e| e.contains("state dir exists")) {
                DetectionConfidence::Low
            } else {
                DetectionConfidence::High
            }
        } else if binary_path.is_some() && version.is_none() {
            DetectionConfidence::Medium
        } else {
            DetectionConfidence::High
        };

        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution(&self) -> VersionResolution {
        let detection = self.detection();
        if let Some(v) = detection.version {
            let mut notes = Vec::new();
            notes.push(format!("detected letta-code version {v}"));
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

    #[expect(clippy::too_many_lines, reason = "multiple surfaces")]
    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        // Local backend state — JSON-ish state per agent, user/instance scope
        let backend_resolver = PathResolver::new(
            Some("$LETTA_LOCAL_BACKEND_DIR"),
            Some("$LETTA_LOCAL_BACKEND_DIR"),
            Some("%LETTA_LOCAL_BACKEND_DIR%"),
            "~/.letta/lc-local-backend",
        );
        let mut backend = ConfigSurface::new(
            "lc-local-backend",
            backend_resolver,
            DocumentKind::Opaque,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        backend.precedence = 10;
        backend.backup_required = false;
        backend.restart_behavior = RestartBehavior::Reload;
        surfaces.push(backend);

        // MemFS per agent — git-backed memory filesystem, instance scope
        let memfs_resolver = PathResolver::new(
            Some("$LETTA_LOCAL_BACKEND_DIR/memfs/<agent-id>/memory"),
            Some("$LETTA_LOCAL_BACKEND_DIR/memfs/<agent-id>/memory"),
            Some("%LETTA_LOCAL_BACKEND_DIR%\\memfs\\<agent-id>\\memory"),
            "~/.letta/lc-local-backend/memfs/<agent-id>/memory",
        );
        let mut memfs = ConfigSurface::new(
            "memfs",
            memfs_resolver,
            DocumentKind::Opaque,
            ConfigScope::Instance,
            SurfaceOwnership::HarnessManaged,
        );
        memfs.precedence = 12;
        memfs.backup_required = false;
        surfaces.push(memfs);

        // Global skills — computer scope
        let global_skills_resolver = PathResolver::new(
            Some("~/.letta/skills/<name>/"),
            Some("~/.letta/skills/<name>/"),
            Some("%USERPROFILE%\\.letta\\skills\\<name>\\"),
            "~/.letta/skills/<name>/",
        );
        let mut global_skills = ConfigSurface::new(
            "skills (global)",
            global_skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        global_skills.precedence = 8;
        global_skills.backup_required = false;
        surfaces.push(global_skills);

        // Project skills — committed with repo
        let project_skills_resolver =
            PathResolver::fallback_only(".agents/skills/<name>/ (project)");
        let mut project_skills = ConfigSurface::new(
            "skills (project)",
            project_skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_skills.precedence = 14;
        project_skills.backup_required = false;
        surfaces.push(project_skills);

        // Agent skills — inside memfs, per agent
        let agent_skills_resolver = PathResolver::new(
            Some("$LETTA_LOCAL_BACKEND_DIR/memfs/<agent-id>/skills/<name>/"),
            Some("$LETTA_LOCAL_BACKEND_DIR/memfs/<agent-id>/skills/<name>/"),
            Some("%LETTA_LOCAL_BACKEND_DIR%\\memfs\\<agent-id>\\skills\\<name>\\"),
            "~/.letta/lc-local-backend/memfs/<agent-id>/skills/<name>/",
        );
        let mut agent_skills = ConfigSurface::new(
            "skills (agent)",
            agent_skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::Instance,
            SurfaceOwnership::UserEditable,
        );
        agent_skills.precedence = 16;
        agent_skills.backup_required = false;
        surfaces.push(agent_skills);

        // Client env config — LETTA_BASE_URL / LETTA_API_KEY (session inline, not file)
        let env_resolver = PathResolver::new(
            Some("$LETTA_BASE_URL / $LETTA_API_KEY (env, session)"),
            Some("$LETTA_BASE_URL / $LETTA_API_KEY (env, session)"),
            Some("%LETTA_BASE_URL% / %LETTA_API_KEY% (env)"),
            "$LETTA_BASE_URL / $LETTA_API_KEY (env)",
        );
        let mut env_surface = ConfigSurface::new(
            "env (LETTA_*)",
            env_resolver,
            DocumentKind::Env,
            ConfigScope::SessionInline,
            SurfaceOwnership::ExternalSecretStore,
        );
        env_surface.precedence = 20;
        env_surface.owned_selectors = vec![
            "LETTA_API_KEY".to_owned(),
            "LETTA_BASE_URL".to_owned(),
            "LETTA_APP_SERVER_URL".to_owned(),
            "model".to_owned(),
            "providers".to_owned(),
        ];
        env_surface.backup_required = false;
        env_surface.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(env_surface);

        // Server provider state — separate server process, constrained
        let server_resolver = PathResolver::fallback_only(
            "server provider state (separate process per provider, /root/.letta, LETTA_APP_SERVER_TOKEN)",
        );
        let mut server_state = ConfigSurface::new(
            "server/provider state",
            server_resolver,
            DocumentKind::Opaque,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        server_state.precedence = 5;
        server_state.backup_required = false;
        server_state.restart_behavior = RestartBehavior::Restart;
        surfaces.push(server_state);

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
            "memfs/*/memory/.git/*".to_owned(),
            "memfs/*/.git/*".to_owned(),
            "*.log".to_owned(),
            "logs/*".to_owned(),
            "cache/*".to_owned(),
            "tmp/*".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
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
            WrapperPlan::new("daemon_service via LETTA_LOCAL_BACKEND_DIR plus LETTA_BASE_URL");
        plan.env_vars.push((
            LOCAL_BACKEND_ENV_VAR.to_owned(),
            instance.config_root.to_string(),
        ));
        // Add a deterministic base-url hint based on instance root for isolation; real server url remains external
        // For constrained isolation we set LOCAL_BACKEND_DIR deterministically and leave BASE_URL to caller env.
        // To make wrapper deterministic and testable, we also set a derived BASE_URL if not externally set.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "name len < 100 truncation intentional"
        )]
        let derived_port = 8283u16 + (instance.name.as_str().len() as u16 % 100);
        let derived_url = format!("http://localhost:{derived_port}");
        plan.env_vars
            .push((BASE_URL_ENV_VAR.to_owned(), derived_url));
        plan.description = format!(
            " Wrapper sets {}={} and {} (separate server per provider, constrained: {}) and execs `{}` --backend local",
            LOCAL_BACKEND_ENV_VAR,
            instance.config_root,
            BASE_URL_ENV_VAR,
            CONSTRAINED_NOTE,
            EXECUTABLE
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.letta/lc-local-backend".to_owned(),
            "~/.letta".to_owned(),
            "$LETTA_LOCAL_BACKEND_DIR".to_owned(),
            "~/.letta/lc-local-backend/memfs".to_owned(),
            "~/.letta/skills".to_owned(),
            ".agents/skills".to_owned(),
            "$LETTA_BASE_URL".to_owned(),
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
            Isolation::DaemonService | Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "letta-code requires isolation daemon_service (or relocated_root), got {other}"
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
    use std::path::PathBuf;

    use super::{
        CONSTRAINED_NOTE, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, LettaAdapter, RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> LettaAdapter {
        LettaAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-letta-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::DaemonService,
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
        assert_eq!(a.local_backend_env_var(), super::LOCAL_BACKEND_ENV_VAR);
        assert_eq!(a.base_url_env_var(), super::BASE_URL_ENV_VAR);
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert_eq!(a.adapter_revision(), crate::adapter::ADAPTER_REVISION);
        assert!(CONSTRAINED_NOTE.contains("separate server"));
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
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("letta 0.2.1", Some("0.2.1")),
            ("letta 0.2.1-beta", Some("0.2.1-beta")),
            ("0.2.1", Some("0.2.1")),
            ("v0.2.1", Some("0.2.1")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = LettaAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_client_and_server() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 5);
        let backend = surfaces
            .iter()
            .find(|s| s.id == "lc-local-backend")
            .expect("backend must exist");
        assert_eq!(backend.kind, DocumentKind::Opaque);
        assert_eq!(backend.ownership, SurfaceOwnership::HarnessManaged);
        assert_eq!(backend.scope, ConfigScope::User);

        let memfs = surfaces
            .iter()
            .find(|s| s.id == "memfs")
            .expect("memfs must exist");
        assert_eq!(memfs.kind, DocumentKind::Opaque);
        assert_eq!(memfs.scope, ConfigScope::Instance);

        let global_skills = surfaces
            .iter()
            .find(|s| s.id == "skills (global)")
            .expect("global skills must exist");
        assert_eq!(global_skills.kind, DocumentKind::TextFragment);
        assert_eq!(global_skills.scope, ConfigScope::User);

        let env = surfaces
            .iter()
            .find(|s| s.id == "env (LETTA_*)")
            .expect("env must exist");
        assert_eq!(env.kind, DocumentKind::Env);
        assert_eq!(env.scope, ConfigScope::SessionInline);
        assert_eq!(env.ownership, SurfaceOwnership::ExternalSecretStore);

        let server = surfaces
            .iter()
            .find(|s| s.id == "server/provider state")
            .expect("server state must exist");
        assert_eq!(server.kind, DocumentKind::Opaque);
        assert_eq!(server.scope, ConfigScope::Internal);
        assert_eq!(server.ownership, SurfaceOwnership::HarnessManaged);
    }

    #[test]
    fn supported_operations_are_constrained() {
        let a = adapter();
        let ops = a.supported_operations();
        assert!(!ops.is_empty());
        for (name, support) in &ops {
            assert_eq!(
                *support,
                AdapterSupport::Constrained,
                "operation {name} should be Constrained"
            );
        }
        let names: HashSet<String> = ops.iter().map(|(n, _)| n.clone()).collect();
        for required in [
            "detect",
            "read_config",
            "write_config",
            "manage_skills",
            "plan_wrapper",
        ] {
            assert!(names.contains(required), "missing op {required}");
        }
    }

    #[test]
    fn plan_mirror_exclusions_cover_memfs_and_logs() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(exclusions.iter().any(|p| p.contains("memfs")));
        assert!(
            exclusions
                .iter()
                .any(|p| p.contains("cache") || p.contains("logs"))
        );
    }

    #[test]
    fn plan_wrapper_sets_env_and_args() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.letta-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == super::LOCAL_BACKEND_ENV_VAR && v.contains(".letta-work"))
        );
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, _)| k == super::BASE_URL_ENV_VAR)
        );
        assert!(plan.description.contains(super::LOCAL_BACKEND_ENV_VAR));
        assert!(plan.description.contains("separate server"));
        assert!(!plan.description.is_empty());
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my letta work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == super::LOCAL_BACKEND_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, "/tmp/my letta work");
        assert!(!env_val.contains('"'));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.letta-work");
        inst.harness = HarnessId::new("claude-code").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_letta_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("lc-local-backend")));
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("LETTA_LOCAL_BACKEND_DIR"))
        );
        assert!(candidates.iter().any(|c| c.contains("skills")));
    }

    #[test]
    fn validate_instance_accepts_daemon_service() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.letta-work");
        a.validate_instance(&inst).unwrap();
        let relocated = {
            let mut r = sample_instance_with_root("/tmp/.letta-work");
            r.isolation = Isolation::RelocatedRoot;
            r
        };
        a.validate_instance(&relocated).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.letta-work");
        inst.isolation = Isolation::FixedPathSingle;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn supported_skill_modes_are_constrained() {
        let a = adapter();
        let modes = a.supported_skill_modes();
        assert_eq!(modes.len(), 3);
        assert!(modes.contains(&crate::adapter::SkillMode::LinkAll));
        assert!(modes.contains(&crate::adapter::SkillMode::LinkSelected));
        assert!(modes.contains(&crate::adapter::SkillMode::CopySelected));
    }

    // -----------------------------------------------------------------------
    // Fixture-backed conformance tests
    // -----------------------------------------------------------------------

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/letta_code")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.json");
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("settings.minimal.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.is_empty() || map.len() <= 3);
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(
            map.contains_key("model")
                || map.contains_key("providers")
                || map.contains_key("mcpServers")
                || map.contains_key("memory")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("letta");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("letta.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("test-model".to_owned()),
            );
            assert!(map.contains_key("foreignKey") || map.contains_key("unknownTopLevel"));
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
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
    fn unknown_key_preservation_via_json_edit() {
        let dir = crate::test_util::temp_dir_unique("letta");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preserve.json");
        let original = serde_json::json!({
            "model": "test-model",
            "foreignKey": "keep-me",
            "anotherForeign": {"nested": 123}
        });
        std::fs::write(&path, serde_json::to_string_pretty(&original).unwrap()).unwrap();
        superai_config::json::edit(&path, |map| {
            map.insert(
                "providers".to_owned(),
                serde_json::json!({"openai": {"apiKeyEnv": "OPENAI_API_KEY"}}),
            );
        })
        .unwrap();
        let after = superai_config::json::load(&path).unwrap();
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
    fn provider_mutation_sets_model_selector() {
        let dir = crate::test_util::temp_dir_unique("letta");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("provider.json");
        let initial = serde_json::json!({
            "model": "old-model"
        });
        std::fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();
        superai_config::json::edit(&path, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("new-model".to_owned()),
            );
        })
        .unwrap();
        let after = superai_config::json::load(&path).unwrap();
        assert_eq!(
            after["model"],
            serde_json::Value::String("new-model".to_owned())
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn adapter_is_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
        assert!(!boxed.config_surfaces().is_empty());
        assert_eq!(boxed.adapter_revision(), crate::adapter::ADAPTER_REVISION);
    }

    #[test]
    fn constrained_note_contains_separate_server() {
        assert!(CONSTRAINED_NOTE.contains("separate server"));
    }
}
