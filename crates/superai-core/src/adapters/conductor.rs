//! Conductor adapter — orchestrator, user/repo TOML, macOS worktrees/profiles, `Constrained`.
//!
//! Research source: `docs/harness-configs/orchestrators.md` (last verified 2026-08-25).
//! macOS desktop app `Conductor`, harnesses claude-code/codex/cursor/opencode,
//! configurable executables `claude_code_executable_path`/`codex_executable_path`,
//! providers `claude_provider` (anthropic/bedrock/vertex) with `bedrock_region`/
//! `vertex_project_id`, models `models.default`/`models.review` etc, TOML
//! `~/.conductor/settings.toml` user + `<repo>/.conductor/settings.toml` repo +
//! `settings.local.toml` secrets + `settings.managed.toml` org, scripts
//! `[scripts]` setup/run/archive + `run_mode`, env `[environment_variables]` with
//! `.local`/`.cloud` scopes, worktrees `~/conductor/workspaces/<name>/` with
//! `CONDUCTOR_*` env (PORT range, `ROOT_PATH`), `.worktreeinclude` file copy,
//! isolation `os_bound` (macOS worktrees), support `Constrained`.

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

/// Harness identifier for Conductor.
pub const HARNESS_ID_STR: &str = "conductor";

/// Human display name.
pub const DISPLAY_NAME: &str = "Conductor";

/// Primary executable name (desktop launcher + CLI helper).
pub const EXECUTABLE: &str = "conductor";

/// Alternative binary name (mac app helper).
pub const EXECUTABLE_ALT: &str = "conductor-cli";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/orchestrators.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Constrained note — macOS worktrees/profile scoped.
pub const CONSTRAINED_NOTE: &str = "user/repo TOML `~/.conductor/settings.toml` + `.conductor/settings.toml` + `.local`/`.managed` scopes, macOS worktrees `~/conductor/workspaces/` with CONDUCTOR_PORT..+9 per workspace, OS-bound: macOS desktop app, conductor build only, provider `claude_provider`/`codex_provider` + Bedrock/Vertex routing, profiles not fully isolated without containers";

/// Owned selectors for provider/executor/model mutation.
/// TOML top-level keys owned inside settings.toml.
pub const OWNED_SELECTORS: &[&str] = &[
    "claude_code_executable_path",
    "codex_executable_path",
    "claude_provider",
    "codex_provider",
    "bedrock_region",
    "vertex_project_id",
    "models.default",
    "models.review",
    "environment_variables",
    "environment_variables.local",
    "environment_variables.cloud",
    "scripts.setup",
    "scripts.run",
    "scripts.archive",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Conductor (`Constrained`, `os_bound`, macOS).
///
/// Isolation is `os_bound` via git worktrees plus macOS app bundle.
/// Constrained because provider/model is per-profile TOML only, true
/// multi-account isolation needs OS users/containers, and Cloud workspaces
/// are separate proprietary infrastructure.
#[derive(Debug, Clone)]
pub struct ConductorAdapter {
    id: HarnessId,
}

impl ConductorAdapter {
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

    /// Constrained note.
    pub fn constrained_note(&self) -> &str {
        CONSTRAINED_NOTE
    }

    /// Try to locate the `conductor` binary via `PATH`.
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

    /// Probe `conductor --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `conductor 1.2.3` into `1.2.3`.
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

    /// Resolve the default user settings path `~/.conductor/settings.toml`.
    fn default_user_settings() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".conductor").join("settings.toml"))
    }

    /// Build detection evidence about workspaces, TOMLs, and executors.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("constrained: {CONSTRAINED_NOTE}"));
        evidence.push(
            "platform macOS only for harnesses: claude-code, codex, cursor, opencode".to_owned(),
        );
        match Self::default_user_settings() {
            Some(path) => {
                if path.exists() {
                    evidence.push(format!("user settings.toml found at {}", path.display()));
                    if let Ok(text) = std::fs::read_to_string(&path)
                        && (text.contains("claude_provider") || text.contains("models"))
                    {
                        evidence
                            .push("user settings.toml contains claude_provider/models".to_owned());
                    }
                } else {
                    evidence.push(format!("user settings.toml missing at {}", path.display()));
                }
                let local = path.with_file_name("settings.local.toml");
                if local.exists() {
                    evidence.push(format!(
                        "settings.local.toml present at {}",
                        local.display()
                    ));
                }
                let managed = path.with_file_name("settings.managed.toml");
                if managed.exists() {
                    evidence.push(format!(
                        "settings.managed.toml present at {} (org overrides)",
                        managed.display()
                    ));
                }
            }
            None => evidence.push("could not resolve user settings path (no HOME)".to_owned()),
        }
        let project_settings = Path::new(".conductor").join("settings.toml");
        if project_settings.exists() {
            evidence.push(format!(
                "project .conductor/settings.toml found at {}",
                project_settings.display()
            ));
        } else {
            evidence.push(format!(
                "project .conductor/settings.toml missing at {}",
                project_settings.display()
            ));
        }
        let workspaces = {
            let home = std::env::var("HOME")
                .ok()
                .or_else(|| std::env::var("USERPROFILE").ok());
            home.map(|h| PathBuf::from(h).join("conductor").join("workspaces"))
        };
        if let Some(ws) = workspaces {
            if ws.exists() {
                evidence.push(format!("workspaces root exists at {}", ws.display()));
                if let Ok(entries) = std::fs::read_dir(&ws) {
                    let count = entries.count();
                    evidence.push(format!(
                        "workspaces root contains {count} workspace entries"
                    ));
                }
            } else {
                evidence.push(format!("workspaces root missing at {}", ws.display()));
            }
        }
        for var in [
            "CONDUCTOR_WORKSPACE_PATH",
            "CONDUCTOR_ROOT_PATH",
            "CONDUCTOR_PORT",
            "claude_code_executable_path",
        ] {
            if let Ok(val) = std::env::var(var)
                && !val.trim().is_empty()
            {
                evidence.push(format!("{var} set to {val}"));
            } else {
                evidence.push(format!("{var} not set"));
            }
        }
        // Executor paths
        evidence.push("executors: $claude_code_executable_path / $codex_executable_path overrides documented; harness binaries expected on PATH".to_owned());
        if Path::new(".worktreeinclude").exists() {
            evidence.push(".worktreeinclude present (untracked file copy)".to_owned());
        }
    }
}

impl Default for ConductorAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "conductor is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for ConductorAdapter {
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
            Platform::new(Os::Macos, Arch::Any),
            Platform::new(Os::Linux, Arch::Any),
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

        if let Some(path) = self.find_binary_in_path() {
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
        } else {
            evidence.push(format!("binary `{EXECUTABLE}` not found in PATH"));
            evidence.push(
                "Conductor is macOS desktop app (conductor.build), not a CLI; detection via settings.toml presence"
                    .to_owned(),
            );
        }

        self.collect_config_evidence(&mut evidence);

        let present = match (&binary_path, &version) {
            (Some(_), Some(_)) => InstallPresence::Present,
            (Some(_), None) => InstallPresence::UnknownVersion,
            (None, _) => InstallPresence::Absent,
        };

        let confidence = if present == InstallPresence::Absent {
            if evidence
                .iter()
                .any(|e| e.contains("user settings.toml found"))
            {
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
            notes.push(format!("detected conductor version {v}"));
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

        let user_resolver = PathResolver::new(
            Some("~/.conductor/settings.toml (user, TOML)"),
            Some("~/.conductor/settings.toml (user, macOS)"),
            Some("%USERPROFILE%\\.conductor\\settings.toml"),
            "~/.conductor/settings.toml (user TOML, claude_provider/models.environment_variables)",
        );
        let mut user = ConfigSurface::new(
            "settings.toml (user)",
            user_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        user.precedence = 10;
        user.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        user.backup_required = true;
        user.restart_behavior = RestartBehavior::Reload;
        surfaces.push(user);

        let managed_resolver = PathResolver::new(
            Some("~/.conductor/settings.managed.toml (org, overrides user+repo)"),
            Some("~/.conductor/settings.managed.toml (org, macOS)"),
            Some("%USERPROFILE%\\.conductor\\settings.managed.toml"),
            "~/.conductor/settings.managed.toml (org managed, overrides all)",
        );
        let mut managed = ConfigSurface::new(
            "settings.managed.toml (org)",
            managed_resolver,
            DocumentKind::Toml,
            ConfigScope::SystemManaged,
            SurfaceOwnership::HarnessManaged,
        );
        managed.precedence = 0;
        managed.backup_required = false;
        managed.restart_behavior = RestartBehavior::Reload;
        surfaces.push(managed);

        let local_resolver = PathResolver::new(
            Some("~/.conductor/settings.local.toml (secrets, not committed)"),
            Some("~/.conductor/settings.local.toml (macOS secrets)"),
            Some("%USERPROFILE%\\.conductor\\settings.local.toml"),
            "~/.conductor/settings.local.toml (secrets, never committed, .local scope)",
        );
        let mut local = ConfigSurface::new(
            "settings.local.toml (secrets)",
            local_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        local.precedence = 12;
        local.owned_selectors = vec![
            "environment_variables.local".to_owned(),
            "claude_code_executable_path".to_owned(),
        ];
        local.backup_required = false;
        local.restart_behavior = RestartBehavior::Reload;
        surfaces.push(local);

        let repo_resolver = PathResolver::fallback_only(
            "<repo>/.conductor/settings.toml (repo, scripts + environment_variables + file_include_globs)",
        );
        let mut repo = ConfigSurface::new(
            "settings.toml (repo)",
            repo_resolver,
            DocumentKind::Toml,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        repo.precedence = 14;
        repo.owned_selectors = vec![
            "scripts.setup".to_owned(),
            "scripts.run".to_owned(),
            "scripts.archive".to_owned(),
            "scripts.run_mode".to_owned(),
            "environment_variables".to_owned(),
            "file_include_globs".to_owned(),
        ];
        repo.backup_required = true;
        surfaces.push(repo);

        let worktrees_resolver = PathResolver::new(
            Some("~/conductor/workspaces/<name>/ (git worktree, branch per task, CONDUCTOR_* env)"),
            Some("~/conductor/workspaces/<name>/ (macOS worktree, conductor.build)"),
            Some("~/conductor/workspaces/<name>/ (worktree)"),
            "~/conductor/workspaces/<name>/ (worktree per task, CONDUCTOR_WORKSPACE_PATH/ROOT_PATH/PORT+9, .worktreeinclude)",
        );
        let mut worktrees = ConfigSurface::new(
            "worktrees (~/conductor/workspaces)",
            worktrees_resolver,
            DocumentKind::Opaque,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::HarnessManaged,
        );
        worktrees.precedence = 8;
        worktrees.backup_required = false;
        surfaces.push(worktrees);

        let env_resolver = PathResolver::fallback_only(
            "CONDUCTOR_* env (CONDUCTOR_WORKSPACE_PATH/ROOT_PATH/PORT/IS_LOCAL + cloud CONDUCTOR_API_URL/TOKEN/SESSION_ID)",
        );
        let mut env_surface = ConfigSurface::new(
            "env (CONDUCTOR_*)",
            env_resolver,
            DocumentKind::Env,
            ConfigScope::SessionInline,
            SurfaceOwnership::ExternalSecretStore,
        );
        env_surface.precedence = 20;
        env_surface.owned_selectors = vec![
            "CONDUCTOR_WORKSPACE_PATH".to_owned(),
            "CONDUCTOR_ROOT_PATH".to_owned(),
            "CONDUCTOR_PORT".to_owned(),
            "CONDUCTOR_IS_LOCAL".to_owned(),
            "environment_variables".to_owned(),
        ];
        env_surface.backup_required = false;
        env_surface.restart_behavior = RestartBehavior::None;
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
            "~/conductor/workspaces/*".to_owned(),
            ".conductor/workspaces/*".to_owned(),
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "tmp/*".to_owned(),
            "*.lock".to_owned(),
            "sessions/*".to_owned(),
            "DerivedData/*".to_owned(),
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
            "os_bound via macOS worktree + CONDUCTOR_* env + TOML scopes (constrained)",
        );
        // Worktree path is per-instance root; CONDUCTOR_WORKSPACE_PATH points there.
        plan.env_vars.push((
            "CONDUCTOR_WORKSPACE_PATH".to_owned(),
            instance.config_root.to_string(),
        ));
        plan.env_vars.push((
            "CONDUCTOR_ROOT_PATH".to_owned(),
            format!("{}/..", instance.config_root),
        ));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "name len < 1000, truncation intentional for deterministic port"
        )]
        let derived_port = 4000u16 + (instance.name.as_str().len() as u16 % 1000);
        plan.env_vars
            .push(("CONDUCTOR_PORT".to_owned(), derived_port.to_string()));
        plan.env_vars
            .push(("CONDUCTOR_IS_LOCAL".to_owned(), "1".to_owned()));
        plan.description = format!(
            " Wrapper sets CONDUCTOR_WORKSPACE_PATH={} CONDUCTOR_ROOT_PATH={}/.. CONDUCTOR_PORT={derived_port} CONDUCTOR_IS_LOCAL=1 (macOS worktree {} per task, PORT range+9, .worktreeinclude, {})",
            instance.config_root, instance.config_root, "~/conductor/workspaces", CONSTRAINED_NOTE
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.conductor/settings.toml".to_owned(),
            "~/.conductor/settings.local.toml".to_owned(),
            "~/.conductor/settings.managed.toml".to_owned(),
            ".conductor/settings.toml (repo)".to_owned(),
            "~/conductor/workspaces (worktrees root, macOS)".to_owned(),
            "$CONDUCTOR_WORKSPACE_PATH via CONDUCTOR_* (worktree)".to_owned(),
            ".worktreeinclude (file copy)".to_owned(),
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
            | Isolation::ProjectScope
            | Isolation::RelocatedRoot => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "conductor requires isolation os_bound (macOS worktrees) or project_scope, got {other} — {CONSTRAINED_NOTE}"
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
        CONSTRAINED_NOTE, ConductorAdapter, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR,
        OWNED_SELECTORS, RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> ConductorAdapter {
        ConductorAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-conductor-1").unwrap(),
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
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert_eq!(a.adapter_revision(), crate::adapter::ADAPTER_REVISION);
        assert!(a.constrained_note().contains("macOS"));
        assert!(CONSTRAINED_NOTE.contains("worktrees"));
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
            assert!(res.notes.iter().any(|n| n.contains("conductor")));
        } else {
            assert!(!res.compatible);
            assert!(res.schema_version.is_none());
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("conductor 1.2.3", Some("1.2.3")),
            ("1.0.0", Some("1.0.0")),
            ("v1.0.0", Some("1.0.0")),
            ("Version: 2.0.0", Some("2.0.0")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = ConductorAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_user_and_repo_and_worktrees() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 5);
        let user = surfaces
            .iter()
            .find(|s| s.id == "settings.toml (user)")
            .expect("user settings must exist");
        assert_eq!(user.kind, DocumentKind::Toml);
        assert_eq!(user.scope, ConfigScope::User);
        assert_eq!(user.ownership, SurfaceOwnership::UserEditable);
        for sel in OWNED_SELECTORS {
            assert!(user.owned_selectors.contains(&(*sel).to_owned()));
        }
        let repo = surfaces
            .iter()
            .find(|s| s.id == "settings.toml (repo)")
            .expect("repo settings must exist");
        assert_eq!(repo.scope, ConfigScope::ProjectWorkspace);
        let env = surfaces
            .iter()
            .find(|s| s.id == "env (CONDUCTOR_*)")
            .expect("env must exist");
        assert_eq!(env.kind, DocumentKind::Env);
        assert_eq!(env.scope, ConfigScope::SessionInline);
        let managed = surfaces
            .iter()
            .find(|s| s.id == "settings.managed.toml (org)")
            .expect("managed must exist");
        assert_eq!(managed.ownership, SurfaceOwnership::HarnessManaged);
        assert_eq!(managed.scope, ConfigScope::SystemManaged);
    }

    #[test]
    fn owned_selectors_are_stable() {
        assert!(OWNED_SELECTORS.len() >= 8);
        let set: HashSet<&str> = OWNED_SELECTORS.iter().copied().collect();
        assert_eq!(set.len(), OWNED_SELECTORS.len(), "selectors must be unique");
        for required in [
            "claude_code_executable_path",
            "claude_provider",
            "models.default",
            "scripts.setup",
        ] {
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
            "plan_wrapper",
            "validate_instance",
        ] {
            assert!(names.contains(required), "missing op {required}");
        }
    }

    #[test]
    fn plan_mirror_exclusions_cover_workspaces_and_logs() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        for pat in ["~/conductor/workspaces/*", "cache/*", "*.lock"] {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"settings.toml (user)".to_owned()));
    }

    #[test]
    fn plan_wrapper_sets_conductor_env() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.conductor-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == "CONDUCTOR_WORKSPACE_PATH" && v == "/tmp/.conductor-work")
        );
        assert!(plan.env_vars.iter().any(|(k, _)| k == "CONDUCTOR_PORT"));
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains("CONDUCTOR_WORKSPACE_PATH"));
        assert!(plan.description.contains("worktree"));
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my conductor work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == "CONDUCTOR_WORKSPACE_PATH")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, "/tmp/my conductor work");
        assert!(env_val.contains(' '));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.conductor-work");
        inst.harness = HarnessId::new("claude-code").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_settings_and_workspaces() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.contains("settings.toml")));
        assert!(candidates.iter().any(|c| c.contains("workspaces")));
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("CONDUCTOR_WORKSPACE_PATH") || c.contains(".worktreeinclude"))
        );
    }

    #[test]
    fn validate_instance_accepts_os_bound() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.conductor-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.conductor-work");
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
