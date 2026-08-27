//! `OpenClaw` adapter — `OPENCLAW_HOME` daemon, `ResearchBlocked`.
//!
//! Research source: `docs/harness-configs/openclaw.md` (last verified 2026-08-25).
//! Executable `openclaw`, config `~/.openclaw/openclaw.json` (JSON5), env file
//! `~/.openclaw/.env`, default state dir `~/.openclaw`, relocation via
//! `OPENCLAW_HOME` / `OPENCLAW_STATE_DIR` / `OPENCLAW_CONFIG_PATH`, isolation
//! `daemon_service` (long-running Node service, ports/gateway), support
//! `ResearchBlocked` until gateway and full schema gaps close.

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
use crate::state::{AdapterSupport, InstallPresence};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Harness identifier for `OpenClaw`.
pub const HARNESS_ID_STR: &str = "openclaw";

/// Human display name.
pub const DISPLAY_NAME: &str = "OpenClaw";

/// Primary executable name.
pub const EXECUTABLE: &str = "openclaw";

/// Env var that overrides home.
pub const HOME_ENV_VAR: &str = "OPENCLAW_HOME";

/// Env var that overrides state dir.
pub const STATE_DIR_ENV_VAR: &str = "OPENCLAW_STATE_DIR";

/// Env var that overrides config path.
pub const CONFIG_PATH_ENV_VAR: &str = "OPENCLAW_CONFIG_PATH";

/// Default config path fallback.
pub const DEFAULT_CONFIG_PATH_FALLBACK: &str = "~/.openclaw/openclaw.json";

/// Default state dir fallback.
pub const DEFAULT_STATE_DIR_FALLBACK: &str = "~/.openclaw";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/openclaw.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Research-blocked reason.
pub const BLOCKED_REASON: &str = "daemon state, gateway/schema incomplete — ports, gateway security, multi-agent, plugin/skill paths unverified; long-running service not per-invocation CLI";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for `OpenClaw` (`ResearchBlocked`).
#[derive(Debug, Clone)]
pub struct OpenClawAdapter {
    id: HarnessId,
}

impl OpenClawAdapter {
    /// Create a new adapter.
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

    /// Blocked reason.
    pub fn blocked_reason(&self) -> &str {
        BLOCKED_REASON
    }

    /// Try to locate `openclaw` binary via PATH.
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

    /// Probe `openclaw --version` with timeout.
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

    /// Parse version output.
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

    /// Resolve default state dir.
    fn default_state_dir() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(STATE_DIR_ENV_VAR)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir));
        }
        if let Ok(dir) = std::env::var(HOME_ENV_VAR)
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
        Some(PathBuf::from(home).join(".openclaw"))
    }

    /// Collect evidence.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("research blocked: {BLOCKED_REASON}"));
        evidence.push(format!(
            "relocation via {HOME_ENV_VAR}/{STATE_DIR_ENV_VAR}/{CONFIG_PATH_ENV_VAR} (precedence explicit > HOME)"
        ));
        match Self::default_state_dir() {
            Some(dir) => {
                if dir.exists() {
                    evidence.push(format!("state dir exists at {}", dir.display()));
                    let config = dir.join("openclaw.json");
                    if config.exists() {
                        evidence.push(format!("openclaw.json found at {}", config.display()));
                    } else {
                        evidence.push(format!("openclaw.json missing at {}", config.display()));
                    }
                    let env = dir.join(".env");
                    if env.exists() {
                        evidence.push(format!(".env found at {}", env.display()));
                    }
                } else {
                    evidence.push(format!("state dir missing at {}", dir.display()));
                }
            }
            None => {
                evidence.push("could not resolve state dir (no HOME)".to_owned());
            }
        }
        if let Ok(cfg_path) = std::env::var(CONFIG_PATH_ENV_VAR)
            && !cfg_path.trim().is_empty()
        {
            evidence.push(format!("{CONFIG_PATH_ENV_VAR} override set to {cfg_path}"));
        }
    }
}

impl Default for OpenClawAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "openclaw is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for OpenClawAdapter {
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
            notes.push(format!("detected openclaw version {v}"));
            notes.push(format!("research blocked — {BLOCKED_REASON}"));
            let mut res = VersionResolution::new(Some(v), None, false);
            res.notes = notes;
            res
        } else {
            let mut res = VersionResolution::unknown();
            res.notes = detection.evidence;
            res.notes
                .push(format!("research blocked: {BLOCKED_REASON}"));
            res
        }
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let config_resolver = PathResolver::new(
            Some(
                "~/.openclaw/openclaw.json ($OPENCLAW_STATE_DIR/openclaw.json or $OPENCLAW_CONFIG_PATH)",
            ),
            Some("~/.openclaw/openclaw.json"),
            Some("%USERPROFILE%\\.openclaw\\openclaw.json"),
            "~/.openclaw/openclaw.json",
        );
        let mut config_surface = ConfigSurface::new(
            "openclaw.json",
            config_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        config_surface.precedence = 10;
        config_surface.owned_selectors = vec![
            "agents.defaults.model".to_owned(),
            "models.providers".to_owned(),
        ];
        config_surface.backup_required = true;
        // JSON5 allows comments; must be format-preserving, but we model as Json for now.
        config_surface.restart_behavior = RestartBehavior::Restart;
        surfaces.push(config_surface);

        let env_resolver = PathResolver::new(
            Some("~/.openclaw/.env ($OPENCLAW_STATE_DIR/.env)"),
            Some("~/.openclaw/.env"),
            Some("%USERPROFILE%\\.openclaw\\.env"),
            "~/.openclaw/.env",
        );
        let mut env_surface = ConfigSurface::new(
            ".env",
            env_resolver,
            DocumentKind::Env,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        env_surface.precedence = 8;
        env_surface.backup_required = true;
        surfaces.push(env_surface);

        let state_resolver = PathResolver::new(
            Some("~/.openclaw (daemon state, not writable)"),
            Some("~/.openclaw (daemon state)"),
            Some("%USERPROFILE%\\.openclaw (daemon state)"),
            "~/.openclaw (daemon state, gateway)",
        );
        let mut state_surface = ConfigSurface::new(
            "daemon-state",
            state_resolver,
            DocumentKind::Opaque,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        state_surface.precedence = 0;
        state_surface.backup_required = false;
        state_surface.restart_behavior = RestartBehavior::Restart;
        surfaces.push(state_surface);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        vec![
            ("detect".to_owned(), AdapterSupport::ResearchBlocked),
            ("read_config".to_owned(), AdapterSupport::ResearchBlocked),
            ("write_config".to_owned(), AdapterSupport::ResearchBlocked),
            ("manage_skills".to_owned(), AdapterSupport::ResearchBlocked),
            ("manage_mcp".to_owned(), AdapterSupport::ResearchBlocked),
            ("manage_plugins".to_owned(), AdapterSupport::ResearchBlocked),
            (
                "configure_provider".to_owned(),
                AdapterSupport::ResearchBlocked,
            ),
            ("plan_mirror".to_owned(), AdapterSupport::ResearchBlocked),
            ("plan_wrapper".to_owned(), AdapterSupport::ResearchBlocked),
            (
                "scan_candidates".to_owned(),
                AdapterSupport::ResearchBlocked,
            ),
            (
                "validate_instance".to_owned(),
                AdapterSupport::ResearchBlocked,
            ),
        ]
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "sessions/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
            "daemon/*".to_owned(),
            "gateway/*".to_owned(),
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
        Err(CoreError::ResearchBlocked {
            harness: self.id.to_string(),
            surface: "wrapper".to_owned(),
            reason: format!(
                "ResearchBlocked: {BLOCKED_REASON} — two instances means two daemons, ports/gateway not verified"
            ),
        })
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.openclaw/openclaw.json".to_owned(),
            "~/.openclaw/.env".to_owned(),
            "~/.openclaw (state dir)".to_owned(),
            "$OPENCLAW_STATE_DIR/openclaw.json via OPENCLAW_STATE_DIR".to_owned(),
            "$OPENCLAW_HOME/openclaw.json via OPENCLAW_HOME".to_owned(),
            "$OPENCLAW_CONFIG_PATH via OPENCLAW_CONFIG_PATH".to_owned(),
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
        Err(CoreError::ResearchBlocked {
            harness: self.id.to_string(),
            surface: "validate_instance".to_owned(),
            reason: format!(
                "ResearchBlocked: {BLOCKED_REASON} — validate blocked until gateway/schema complete"
            ),
        })
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        BLOCKED_REASON, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, OpenClawAdapter, RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> OpenClawAdapter {
        OpenClawAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-openclaw-1").unwrap(),
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
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert!(a.blocked_reason().contains("gateway"));
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
    fn detection_returns_evidence_with_blocked_reason() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.contains("research blocked"))
        );
        assert!(result.evidence.iter().any(|e| e.contains("OPENCLAW")));
    }

    #[test]
    fn version_resolution_is_not_compatible() {
        let a = adapter();
        let res = a.version_resolution();
        assert!(!res.compatible);
        assert!(res.schema_version.is_none());
        assert!(!res.notes.is_empty());
        assert!(
            res.notes
                .iter()
                .any(|n| n.contains("research blocked") || n.contains("gateway"))
        );
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("openclaw 1.2.3", Some("1.2.3")),
            ("1.0.0", Some("1.0.0")),
            ("v2.0.0", Some("2.0.0")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = OpenClawAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_exist() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(!surfaces.is_empty());
        let cfg = surfaces
            .iter()
            .find(|s| s.id == "openclaw.json")
            .expect("openclaw.json must exist");
        assert_eq!(cfg.kind, DocumentKind::Json);
        assert_eq!(cfg.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(cfg.scope, ConfigScope::User);
        assert!(cfg.backup_required);
    }

    #[test]
    fn supported_operations_are_research_blocked() {
        let a = adapter();
        let ops = a.supported_operations();
        for (_, support) in ops {
            assert_eq!(support, AdapterSupport::ResearchBlocked);
        }
        assert!(!BLOCKED_REASON.is_empty());
    }

    #[test]
    fn plan_wrapper_is_research_blocked() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.openclaw-work");
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::ResearchBlocked { reason, .. } => {
                assert!(
                    reason.contains("daemon")
                        || reason.contains("gateway")
                        || reason.contains("ResearchBlocked")
                );
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn validate_instance_is_research_blocked() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.openclaw-work");
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::ResearchBlocked { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_openclaw_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("openclaw.json")));
        assert!(candidates.iter().any(|c| c.contains("OPENCLAW")));
    }

    #[test]
    fn supported_skill_modes_is_empty() {
        let a = adapter();
        assert!(a.supported_skill_modes().is_empty());
    }
}
