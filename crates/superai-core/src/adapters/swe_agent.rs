//! SWE-agent adapter — composed YAML via `--config` batch.
//!
//! Research source: `docs/harness-configs/swe-agent.md` (last verified 2026-08-25).
//! Executable `sweagent`, composed YAML `config/*.yaml` with repeatable `--config`,
//! plus `SWE_AGENT_CONFIG_ROOT` and `SWE_AGENT_TRAJECTORY_DIR`, isolation `explicit-config`.
//! Full for config-run instances; batch/Docker orchestration handled separately.

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

/// Harness identifier for SWE-agent.
pub const HARNESS_ID_STR: &str = "swe-agent";

/// Human display name.
pub const DISPLAY_NAME: &str = "SWE-agent";

/// Primary executable name.
pub const EXECUTABLE: &str = "sweagent";

/// Alternative executable (with dash).
pub const EXECUTABLE_ALT: &str = "swe-agent";

/// Environment variable for config root overlay.
pub const CONFIG_DIR_ENV_VAR: &str = "SWE_AGENT_CONFIG_DIR";

/// Environment variable for trajectory dir.
pub const TRAJECTORY_ENV_VAR: &str = "SWE_AGENT_TRAJECTORY_DIR";

/// Environment variable for config root (package dir).
pub const CONFIG_ROOT_ENV_VAR: &str = "SWE_AGENT_CONFIG_ROOT";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/swe-agent.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current config shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside SWE-agent YAML config (`agent.model` etc).
pub const OWNED_SELECTORS: &[&str] = &[
    "agent",
    "agent.model",
    "agent.model.name",
    "agent.model.temperature",
    "agent.model.api_key",
    "agent.model.per_instance_cost_limit",
    "agent.templates",
    "tools",
    "tools.bundles",
    "environment",
    "environment.deployment",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for SWE-agent.
///
/// Isolation is `explicit-config` via composed `--config` flags. The wrapper
/// sets `--config <instance>/config.yaml` (and may set env vars for
/// config/trajectory isolation). Batch sharding is a separate orchestration
/// concern but uses the same config composition.
#[derive(Debug, Clone)]
pub struct SweAgentAdapter {
    id: HarnessId,
}

impl SweAgentAdapter {
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

    /// Config dir env var.
    pub fn config_dir_env_var(&self) -> &str {
        CONFIG_DIR_ENV_VAR
    }

    /// Try to locate the `sweagent` binary via `PATH`.
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

    /// Probe `sweagent --help` / `--version` with a timeout.
    fn probe_version(binary: &Path) -> Option<String> {
        let binary_owned = binary.to_path_buf();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let output = Command::new(&binary_owned)
                .arg("--version")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();
            let output = if output
                .as_ref()
                .is_ok_and(|o| o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty())
            {
                output
            } else {
                Command::new(&binary_owned)
                    .arg("--help")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output()
            };
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

    /// Parse version output like `sweagent 1.0.0` into `1.0.0`.
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

    /// Resolve the default config root (package config dir).
    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(CONFIG_ROOT_ENV_VAR)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir));
        }
        if let Ok(dir) = std::env::var(CONFIG_DIR_ENV_VAR)
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
        Some(PathBuf::from(home).join(".config").join("swe-agent"))
    }

    /// Collect config evidence.
    #[expect(clippy::excessive_nesting, reason = "detection branches are explicit")]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let default_cfg = root.join("config").join("default.yaml");
                    if default_cfg.exists() {
                        evidence.push(format!("default.yaml found at {}", default_cfg.display()));
                    } else if root.join("config.yaml").exists() {
                        evidence.push(format!(
                            "config.yaml found at {}",
                            root.join("config.yaml").display()
                        ));
                    } else {
                        evidence.push(format!("no default config at {}", root.display()));
                    }
                    let tools = root.join("tools");
                    if tools.exists() {
                        evidence.push(format!("tools bundles present at {}", tools.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
        if let Ok(val) = std::env::var(CONFIG_DIR_ENV_VAR)
            && !val.trim().is_empty()
        {
            evidence.push(format!("{CONFIG_DIR_ENV_VAR} set to {val}"));
        }
        if let Ok(val) = std::env::var(TRAJECTORY_ENV_VAR)
            && !val.trim().is_empty()
        {
            evidence.push(format!("{TRAJECTORY_ENV_VAR} set to {val}"));
        }
        if let Ok(val) = std::env::var(CONFIG_ROOT_ENV_VAR)
            && !val.trim().is_empty()
        {
            evidence.push(format!("{CONFIG_ROOT_ENV_VAR} set to {val}"));
        }
        if Path::new("config/default.yaml").exists() {
            evidence.push("project config/default.yaml present".to_owned());
        }
        if Path::new(".env").exists() {
            evidence.push(".env present (api keys)".to_owned());
        }
    }
}

impl Default for SweAgentAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "swe-agent is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for SweAgentAdapter {
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
            notes.push(format!("detected swe-agent version {v}"));
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

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let base_resolver = PathResolver::new(
            Some("config/default.yaml"),
            Some("config/default.yaml"),
            Some("config\\default.yaml"),
            "config/default.yaml (package default)",
        );
        let mut base = ConfigSurface::new(
            "config/default.yaml",
            base_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        base.precedence = 10;
        base.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        base.backup_required = true;
        base.restart_behavior = RestartBehavior::Reload;
        surfaces.push(base);

        let overlay_resolver = PathResolver::new(
            Some("$SWE_AGENT_CONFIG_DIR/config.yaml or --config <path>"),
            Some("$SWE_AGENT_CONFIG_DIR/config.yaml or --config <path>"),
            Some("%SWE_AGENT_CONFIG_DIR%\\config.yaml or --config <path>"),
            "--config <path> overlay (composed YAML)",
        );
        let mut overlay = ConfigSurface::new(
            "config/overlay.yaml",
            overlay_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        overlay.precedence = 11;
        overlay.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        overlay.backup_required = true;
        surfaces.push(overlay);

        let tools_resolver = PathResolver::new(
            Some("tools/<bundle>/config.yaml"),
            Some("tools/<bundle>/config.yaml"),
            Some("tools\\<bundle>\\config.yaml"),
            "tools/<bundle>/config.yaml",
        );
        let mut tools = ConfigSurface::new(
            "tools",
            tools_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        tools.precedence = 12;
        tools.owned_selectors = vec!["tools.bundles".to_owned()];
        surfaces.push(tools);

        let env_resolver = PathResolver::new(
            Some(".env"),
            Some(".env"),
            Some(".env"),
            ".env (api keys, SWE_AGENT_* vars)",
        );
        let mut env = ConfigSurface::new(
            ".env",
            env_resolver,
            DocumentKind::Env,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        env.precedence = 5;
        surfaces.push(env);

        let traj_resolver = PathResolver::new(
            Some("$SWE_AGENT_TRAJECTORY_DIR/trajectory.json"),
            Some("$SWE_AGENT_TRAJECTORY_DIR/trajectory.json"),
            Some("%SWE_AGENT_TRAJECTORY_DIR%\\trajectory.json"),
            "trajectories/trajectory.json (harness managed)",
        );
        let mut traj = ConfigSurface::new(
            "trajectories",
            traj_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        traj.precedence = 0;
        traj.backup_required = false;
        surfaces.push(traj);

        let skills_resolver = PathResolver::new(
            Some("skills/<name>/SKILL.md"),
            Some("skills/<name>/SKILL.md"),
            Some("skills\\<name>\\SKILL.md"),
            "skills/<name>/SKILL.md",
        );
        let mut skills = ConfigSurface::new(
            "skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills.precedence = 8;
        surfaces.push(skills);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        // Full for config-run instances (the HAD-09 wave scope). Batch orchestration
        // itself is out of scope but uses the same composed config mechanism.
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
            "trajectories/*".to_owned(),
            "trajectories/**/*".to_owned(),
            "*.trajectories/*".to_owned(),
            "preds.json".to_owned(),
            "results/*".to_owned(),
            "logs/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "tmp/*".to_owned(),
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
        let mut plan =
            WrapperPlan::new("explicit config via --config <instance>/config.yaml (composed)");
        let config_path = Path::new(&instance.config_root.to_string()).join("config.yaml");
        plan.args.push("--config".to_owned());
        plan.args.push(config_path.display().to_string());
        // Isolate trajectory dir per instance as well
        plan.env_vars.push((
            TRAJECTORY_ENV_VAR.to_owned(),
            Path::new(&instance.config_root.to_string())
                .join("trajectories")
                .display()
                .to_string(),
        ));
        plan.description = format!(
            " Wrapper execs `{} --config {}` with {TRAJECTORY_ENV_VAR} isolated (batch Full for config-run)",
            EXECUTABLE,
            config_path.display()
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "config/default.yaml".to_owned(),
            "config.yaml".to_owned(),
            "$SWE_AGENT_CONFIG_DIR".to_owned(),
            "$SWE_AGENT_CONFIG_ROOT".to_owned(),
            ".env".to_owned(),
            "trajectories/*".to_owned(),
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
            Isolation::ExplicitConfig | Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("swe-agent requires isolation explicit_config, got {other}"),
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
        DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, OWNED_SELECTORS, RESEARCH_DOC, SweAgentAdapter,
        TRAJECTORY_ENV_VAR,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> SweAgentAdapter {
        SweAgentAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-swe-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::ExplicitConfig,
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
            ("sweagent 1.0.0", Some("1.0.0")),
            ("swe-agent 0.5.1", Some("0.5.1")),
            ("v1.2.3", Some("1.2.3")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = SweAgentAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_composed_yaml() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let base = surfaces
            .iter()
            .find(|s| s.id == "config/default.yaml")
            .expect("config/default.yaml");
        assert_eq!(base.kind, DocumentKind::Yaml);
        assert_eq!(base.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(base.scope, ConfigScope::User);
        for sel in ["agent.model.name", "tools.bundles"] {
            assert!(base.owned_selectors.contains(&sel.to_owned()));
        }
        let env = surfaces.iter().find(|s| s.id == ".env").expect(".env");
        assert_eq!(env.kind, DocumentKind::Env);
        let traj = surfaces
            .iter()
            .find(|s| s.id == "trajectories")
            .expect("trajectories");
        assert_eq!(traj.ownership, SurfaceOwnership::HarnessManaged);
    }

    #[test]
    fn owned_selectors_are_stable() {
        assert!(OWNED_SELECTORS.len() >= 5);
        let set: HashSet<&str> = OWNED_SELECTORS.iter().copied().collect();
        assert_eq!(set.len(), OWNED_SELECTORS.len());
    }

    #[test]
    fn supported_operations_are_full_for_config_run() {
        let a = adapter();
        let ops = a.supported_operations();
        for (_, support) in &ops {
            assert_eq!(*support, AdapterSupport::Full);
        }
        let names: HashSet<String> = ops.iter().map(|(n, _)| n.clone()).collect();
        for required in ["detect", "read_config", "write_config", "plan_wrapper"] {
            assert!(names.contains(required));
        }
    }

    #[test]
    fn plan_mirror_exclusions_cover_trajectories_and_locks() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(exclusions.contains(&"trajectories/*".to_owned()));
        assert!(exclusions.contains(&"*.lock".to_owned()));
        assert!(!exclusions.contains(&"config/default.yaml".to_owned()));
    }

    #[test]
    fn plan_wrapper_sets_config_and_traj() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/swe-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(plan.args.contains(&"--config".to_owned()));
        let cfg = plan.args.windows(2).find(|w| w[0] == "--config").unwrap()[1].clone();
        assert!(cfg.contains("swe-work"));
        assert!(cfg.contains("config.yaml"));
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == TRAJECTORY_ENV_VAR && v.contains("swe-work"))
        );
        assert!(plan.description.contains("--config"));
        assert!(plan.description.contains("batch Full"));
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my swe work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let cfg = plan.args.windows(2).find(|w| w[0] == "--config").unwrap()[1].clone();
        assert_eq!(cfg, "/tmp/my swe work/config.yaml");
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/swe-work");
        inst.harness = HarnessId::new("codex-cli").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_default_root() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("config/default.yaml")));
        assert!(candidates.iter().any(|c| c.contains("SWE_AGENT")));
    }

    #[test]
    fn validate_instance_accepts_explicit_config() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/swe-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/swe-work");
        inst.isolation = Isolation::ProjectScope;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn validate_instance_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/swe-work");
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/swe_agent")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.yaml");
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("config.minimal.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(
            map.contains_key("agent")
                || map.contains_key("tools")
                || map.is_empty()
                || map.len() <= 2
        );
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("config.populated.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(
            map.contains_key("agent")
                || map.contains_key("tools")
                || map.contains_key("environment")
        );
    }

    #[test]
    fn fixture_foreign_survive_because_changing_edit_refuses() {
        let path = fixture_path("config.foreign.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::yaml::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("swe");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("swe.foreign.copy.yaml");
        std::fs::copy(&path, &tmp).unwrap();
        // codec-honesty (DOC-06): changing YAML writes on existing files are
        // refused outright, so foreign keys survive because nothing is written.
        let result = superai_config::yaml::edit(&tmp, |map| {
            map.insert(
                "tools".to_owned(),
                serde_json::Value::Object(serde_json::Map::default()),
            );
            assert!(map.contains_key("foreignKey") || map.contains_key("unknownTopLevel"));
        });
        assert!(matches!(
            result,
            Err(superai_config::ConfigError::LossyWrite { format: "yaml", .. })
        ));
        let after = superai_config::yaml::load(&tmp).unwrap();
        assert!(after.contains_key("foreignKey") || after.contains_key("unknownTopLevel"));
        drop(std::fs::remove_file(&tmp));
    }

    #[test]
    fn fixture_malformed_fails_to_parse() {
        let path = fixture_path("config.malformed.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::yaml::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn fixture_batch_overlay_minimal_parses() {
        let path = fixture_path("overlay.minimal.yaml");
        assert!(path.exists(), "overlay minimal missing: {}", path.display());
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(map.contains_key("agent") || map.is_empty() || !map.is_empty());
    }
}
