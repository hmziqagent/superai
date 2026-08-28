//! iFlow CLI adapter — Gemini-fork, `MigrationOnly` shutdown 2026-04-17.
//!
//! Research source: `docs/harness-configs/iflow-cli.md` (last verified 2026-08-25).
//! Executable `iflow`, config `~/.iflow/settings.json` (user), `.iflow/settings.json`
//! (project), `/etc/iflow-cli/settings.json` or `$IFLOW_CLI_SYSTEM_SETTINGS_PATH`
//! (system) plus env `IFLOW_*` overrides, isolation `env_only` / system-file;
//! product status `sunset` (shutdown 2026-04-17), successor guidance at
//! `vibex.iflow.cn/t/topic/4819`; support `MigrationOnly` — detect/inspect/backup/export
//! with tip, no new defaults, no deletion.

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

/// Harness identifier for iFlow CLI.
pub const HARNESS_ID_STR: &str = "iflow-cli";

/// Human display name.
pub const DISPLAY_NAME: &str = "iFlow CLI";

/// Primary executable name.
pub const EXECUTABLE: &str = "iflow";

/// Alternative binary name (legacy).
pub const EXECUTABLE_ALT: &str = "iflow-cli";

/// Environment variable that relocates the system-tier settings file.
pub const SYSTEM_SETTINGS_ENV_VAR: &str = "IFLOW_CLI_SYSTEM_SETTINGS_PATH";

/// Default user config root fallback.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.iflow";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/iflow-cli.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current settings shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Shutdown date for iFlow CLI.
pub const SHUTDOWN_DATE: &str = "2026-04-17";

/// Successor harness id (Gemini lineage).
pub const SUCCESSOR_ID: &str = "gemini-cli";

/// Successor executable.
pub const SUCCESSOR_EXECUTABLE: &str = "gemini";

/// Migration tip shown for migration-only support.
pub const MIGRATION_TIP: &str = "iFlow CLI shutting down 2026-04-17 (UTC+8); migrate via gemini-cli (Gemini CLI lineage) — IFLOW_* env vars map to GEMINI_* (apiKey/baseUrl/modelName), system settings IFLOW_CLI_SYSTEM_SETTINGS_PATH -> GEMINI system path, auth selectedAuthType iflow/openai-compatible; guide https://vibex.iflow.cn/t/topic/4819";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for iFlow CLI (`MigrationOnly`).
///
/// Isolation is `env_only` (pure `IFLOW_*` env vars outrank all files) with
/// optional `IFLOW_CLI_SYSTEM_SETTINGS_PATH` for file-based isolation.
/// `MigrationOnly` means only detect/inspect/backup/export are supported.
#[derive(Debug, Clone)]
pub struct IflowAdapter {
    id: HarnessId,
}

impl IflowAdapter {
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

    /// System settings env var.
    pub fn system_settings_env_var(&self) -> &str {
        SYSTEM_SETTINGS_ENV_VAR
    }

    /// Successor tip.
    pub fn successor_tip(&self) -> &str {
        MIGRATION_TIP
    }

    /// Try to locate the `iflow` binary via `PATH`.
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

    /// Probe `iflow --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `iflow 0.9.0` into `0.9.0`.
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

    /// Resolve the default user config root: `~/.iflow`.
    fn default_config_root() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".iflow"))
    }

    /// Build detection evidence about shutdown, config roots, and env.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!(
            "product sunset shutdown {SHUTDOWN_DATE}, successor {SUCCESSOR_ID} ({SUCCESSOR_EXECUTABLE}) Gemini-fork lineage"
        ));
        evidence.push(MIGRATION_TIP.to_owned());
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("user config root exists at {}", root.display()));
                    let settings = root.join("settings.json");
                    if settings.exists() {
                        evidence.push(format!(
                            "user settings.json found at {}",
                            settings.display()
                        ));
                        if let Ok(text) = std::fs::read_to_string(&settings)
                            && (text.contains("selectedAuthType") || text.contains("apiKey"))
                        {
                            evidence.push(
                                "user settings.json contains selectedAuthType/apiKey".to_owned(),
                            );
                        }
                    } else {
                        evidence.push(format!(
                            "user settings.json missing at {}",
                            settings.display()
                        ));
                    }
                    let iflow_md = root.join("IFLOW.md");
                    if iflow_md.exists() {
                        evidence.push(format!("global IFLOW.md found at {}", iflow_md.display()));
                    }
                    let agents = root.join("agents");
                    if agents.exists() {
                        evidence.push(format!("agents dir present at {}", agents.display()));
                    }
                } else {
                    evidence.push(format!("user config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve user config root (no HOME)".to_owned());
            }
        }
        // Project tier
        let project_settings = Path::new(".iflow").join("settings.json");
        if project_settings.exists() {
            evidence.push(format!(
                "project settings found at {}",
                project_settings.display()
            ));
        }
        // System tier env var
        if let Ok(val) = std::env::var(SYSTEM_SETTINGS_ENV_VAR)
            && !val.trim().is_empty()
        {
            evidence.push(format!("{SYSTEM_SETTINGS_ENV_VAR} set to {val}"));
            let sys_path = Path::new(&val);
            if sys_path.exists() {
                evidence.push(format!(
                    "system settings file exists at {}",
                    sys_path.display()
                ));
            }
        } else {
            evidence.push(format!("{SYSTEM_SETTINGS_ENV_VAR} not set"));
        }
        // IFLOW_* env vars
        let mut env_count = 0;
        for (key, _) in std::env::vars() {
            if key.starts_with("IFLOW_") || key.starts_with("iflow_") {
                env_count += 1;
            }
        }
        if env_count > 0 {
            evidence.push(format!("found {env_count} IFLOW_* env vars"));
        }
        if Path::new("IFLOW.md").exists() {
            evidence.push("project IFLOW.md found in cwd".to_owned());
        }
    }
}

impl Default for IflowAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "iflow-cli is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for IflowAdapter {
    fn id(&self) -> HarnessId {
        self.id.clone()
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    fn product_status(&self) -> ProductStatus {
        ProductStatus::Sunset
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
            DetectionConfidence::High
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
            notes.push(format!("detected iflow-cli version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            notes.push(format!(
                "sunset shutdown {SHUTDOWN_DATE}, successor {SUCCESSOR_ID}"
            ));
            let mut res =
                VersionResolution::new(Some(v), Some(SCHEMA_VERSION_STR.to_owned()), true);
            res.notes = notes;
            res
        } else {
            let mut res = VersionResolution::unknown();
            res.notes = detection.evidence;
            res.notes.push(format!("migration tip: {MIGRATION_TIP}"));
            res
        }
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        // User settings.json — ~/.iflow/settings.json
        let user_resolver = PathResolver::new(
            Some("~/.iflow/settings.json"),
            Some("~/.iflow/settings.json"),
            Some("%USERPROFILE%\\.iflow\\settings.json"),
            "~/.iflow/settings.json",
        );
        let mut user_settings = ConfigSurface::new(
            "settings.json (user)",
            user_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        user_settings.precedence = 10;
        user_settings.owned_selectors = vec![
            "selectedAuthType".to_owned(),
            "apiKey".to_owned(),
            "baseUrl".to_owned(),
            "modelName".to_owned(),
            "searchApiKey".to_owned(),
            "mcpServers".to_owned(),
            "coreTools".to_owned(),
            "excludeTools".to_owned(),
        ];
        user_settings.backup_required = true;
        user_settings.restart_behavior = RestartBehavior::Reload;
        surfaces.push(user_settings);

        // Project settings — .iflow/settings.json
        let project_resolver = PathResolver::fallback_only(".iflow/settings.json (project)");
        let mut project_settings = ConfigSurface::new(
            "settings.json (project)",
            project_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_settings.precedence = 12;
        project_settings.owned_selectors = vec![
            "selectedAuthType".to_owned(),
            "apiKey".to_owned(),
            "baseUrl".to_owned(),
            "modelName".to_owned(),
            "mcpServers".to_owned(),
        ];
        project_settings.backup_required = true;
        project_settings.restart_behavior = RestartBehavior::Reload;
        surfaces.push(project_settings);

        // System settings — /etc/iflow-cli/settings.json or IFLOW_CLI_SYSTEM_SETTINGS_PATH
        let system_resolver = PathResolver::new(
            Some("$IFLOW_CLI_SYSTEM_SETTINGS_PATH or /etc/iflow-cli/settings.json"),
            Some(
                "$IFLOW_CLI_SYSTEM_SETTINGS_PATH or /Library/Application Support/iFlowCli/settings.json",
            ),
            Some("%IFLOW_CLI_SYSTEM_SETTINGS_PATH% or C:\\ProgramData\\iflow-cli\\settings.json"),
            "$IFLOW_CLI_SYSTEM_SETTINGS_PATH or /etc/iflow-cli/settings.json",
        );
        let mut system_settings = ConfigSurface::new(
            "settings.json (system)",
            system_resolver,
            DocumentKind::Json,
            ConfigScope::SystemManaged,
            SurfaceOwnership::UserEditable,
        );
        system_settings.precedence = 15;
        system_settings.owned_selectors = vec![
            "selectedAuthType".to_owned(),
            "apiKey".to_owned(),
            "baseUrl".to_owned(),
            "modelName".to_owned(),
        ];
        system_settings.backup_required = true;
        surfaces.push(system_settings);

        // IFLOW.md — project context file
        let iflow_md_resolver = PathResolver::fallback_only("IFLOW.md (project context)");
        let mut iflow_md = ConfigSurface::new(
            "IFLOW.md",
            iflow_md_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        iflow_md.precedence = 14;
        iflow_md.backup_required = false;
        surfaces.push(iflow_md);

        // Subagents — .iflow/agents/*.md / ~/.iflow/agents/*.md
        let agents_resolver = PathResolver::new(
            Some("~/.iflow/agents/*.md or .iflow/agents/*.md"),
            Some("~/.iflow/agents/*.md or .iflow/agents/*.md"),
            Some("%USERPROFILE%\\.iflow\\agents\\*.md"),
            "~/.iflow/agents/*.md or .iflow/agents/*.md",
        );
        let mut agents = ConfigSurface::new(
            "agents",
            agents_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        agents.precedence = 8;
        agents.backup_required = false;
        surfaces.push(agents);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        vec![
            ("detect".to_owned(), AdapterSupport::MigrationOnly),
            ("read_config".to_owned(), AdapterSupport::MigrationOnly),
            ("write_config".to_owned(), AdapterSupport::Unsupported),
            ("manage_skills".to_owned(), AdapterSupport::Unsupported),
            ("manage_mcp".to_owned(), AdapterSupport::Unsupported),
            ("manage_plugins".to_owned(), AdapterSupport::Unsupported),
            ("configure_provider".to_owned(), AdapterSupport::Unsupported),
            ("plan_mirror".to_owned(), AdapterSupport::MigrationOnly),
            ("plan_wrapper".to_owned(), AdapterSupport::Unsupported),
            ("scan_candidates".to_owned(), AdapterSupport::MigrationOnly),
            (
                "validate_instance".to_owned(),
                AdapterSupport::MigrationOnly,
            ),
            ("backup".to_owned(), AdapterSupport::MigrationOnly),
            ("export".to_owned(), AdapterSupport::MigrationOnly),
        ]
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "tmp/*".to_owned(),
            "agents/*".to_owned(),
            "mcp/*".to_owned(),
            "history/*".to_owned(),
            "sessions/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
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
        Err(CoreError::UnsupportedOperation {
            harness: self.id.to_string(),
            operation: "plan_wrapper".to_owned(),
            reason: format!("MigrationOnly: iFlow CLI shut down {SHUTDOWN_DATE}; {MIGRATION_TIP}"),
        })
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.iflow/settings.json".to_owned(),
            ".iflow/settings.json".to_owned(),
            "$IFLOW_CLI_SYSTEM_SETTINGS_PATH".to_owned(),
            "/etc/iflow-cli/settings.json".to_owned(),
            "~/.iflow/IFLOW.md".to_owned(),
            "IFLOW.md".to_owned(),
            "~/.iflow/agents".to_owned(),
            ".iflow/agents".to_owned(),
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
            Isolation::EnvOnly
            | Isolation::ProjectScope
            | Isolation::RelocatedRoot
            | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "iflow-cli expects isolation env_only or project_scope, got {other}"
                ),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{
        DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, IflowAdapter, MIGRATION_TIP, RESEARCH_DOC,
        SHUTDOWN_DATE, SUCCESSOR_ID,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> IflowAdapter {
        IflowAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-iflow-1").unwrap(),
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
        assert_eq!(a.system_settings_env_var(), super::SYSTEM_SETTINGS_ENV_VAR);
        assert_eq!(a.product_status(), ProductStatus::Sunset);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert_eq!(a.adapter_revision(), crate::adapter::ADAPTER_REVISION);
        assert!(a.successor_tip().contains(SHUTDOWN_DATE));
        assert!(a.successor_tip().contains(SUCCESSOR_ID));
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
    fn detection_returns_evidence_with_sunset() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.contains("shut") || e.contains(SHUTDOWN_DATE) || e.contains("sunset"))
        );
        assert!(result.evidence.iter().any(|e| e.contains(SUCCESSOR_ID)));
        assert_ne!(result.confidence.to_string(), "");
    }

    #[test]
    fn version_resolution_includes_tip() {
        let a = adapter();
        let res = a.version_resolution();
        assert!(!res.notes.is_empty());
        assert!(res.notes.iter().any(|n| n.contains(SUCCESSOR_ID)
            || n.contains("migration")
            || n.contains(SHUTDOWN_DATE)));
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("iflow 0.9.0", Some("0.9.0")),
            ("iflow 1.0.0-beta", Some("1.0.0-beta")),
            ("0.9.0", Some("0.9.0")),
            ("v1.2.3", Some("1.2.3")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = IflowAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_user_and_project() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let user = surfaces
            .iter()
            .find(|s| s.id == "settings.json (user)")
            .expect("user settings must exist");
        assert_eq!(user.kind, DocumentKind::Json);
        assert_eq!(user.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(user.scope, ConfigScope::User);
        assert!(user.backup_required);
        assert!(user.owned_selectors.iter().any(|s| s == "apiKey"));

        let project = surfaces
            .iter()
            .find(|s| s.id == "settings.json (project)")
            .expect("project settings must exist");
        assert_eq!(project.kind, DocumentKind::Json);
        assert_eq!(project.scope, ConfigScope::ProjectWorkspace);

        let system = surfaces
            .iter()
            .find(|s| s.id == "settings.json (system)")
            .expect("system settings must exist");
        assert_eq!(system.scope, ConfigScope::SystemManaged);

        let iflow_md = surfaces
            .iter()
            .find(|s| s.id == "IFLOW.md")
            .expect("IFLOW.md");
        assert_eq!(iflow_md.kind, DocumentKind::TextFragment);
    }

    #[test]
    fn supported_operations_are_migration_only() {
        let a = adapter();
        let ops = a.supported_operations();
        let map: std::collections::HashMap<String, AdapterSupport> = ops.into_iter().collect();
        assert_eq!(map.get("detect"), Some(&AdapterSupport::MigrationOnly));
        assert_eq!(map.get("read_config"), Some(&AdapterSupport::MigrationOnly));
        assert_eq!(map.get("write_config"), Some(&AdapterSupport::Unsupported));
        assert_eq!(map.get("plan_wrapper"), Some(&AdapterSupport::Unsupported));
        assert!(map.contains_key("backup"));
        assert!(map.contains_key("export"));
    }

    #[test]
    fn plan_wrapper_is_blocked_with_tip() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.iflow-work");
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::UnsupportedOperation { reason, .. } => {
                assert!(reason.contains(SUCCESSOR_ID) || reason.contains(SHUTDOWN_DATE));
                assert!(reason.contains(MIGRATION_TIP) || reason.contains("MigrationOnly"));
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.iflow-work");
        inst.harness = HarnessId::new("claude-code").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_iflow_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains(".iflow")));
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("IFLOW_CLI_SYSTEM_SETTINGS_PATH"))
        );
        assert!(candidates.iter().any(|c| c.contains("IFLOW.md")));
    }

    #[test]
    fn validate_instance_accepts_env_only() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.iflow-work");
        a.validate_instance(&inst).unwrap();
        let proj = {
            let mut p = sample_instance_with_root("/tmp/.iflow-work");
            p.isolation = Isolation::ProjectScope;
            p
        };
        a.validate_instance(&proj).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.iflow-work");
        inst.isolation = Isolation::FixedPathSingle;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn supported_skill_modes_is_empty() {
        let a = adapter();
        assert!(a.supported_skill_modes().is_empty());
    }

    #[test]
    fn plan_mirror_exclusions_cover_history() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(
            exclusions
                .iter()
                .any(|p| p.contains("history") || p.contains("sessions"))
        );
    }

    // -----------------------------------------------------------------------
    // Fixture-backed conformance tests
    // -----------------------------------------------------------------------

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/iflow_cli")
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
        // Minimal may be empty or contain selectedAuthType
        assert!(map.is_empty() || map.contains_key("selectedAuthType") || map.len() <= 3);
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(
            map.contains_key("selectedAuthType")
                || map.contains_key("apiKey")
                || map.contains_key("baseUrl")
                || map.contains_key("mcpServers")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("iflow");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("iflow.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "apiKey".to_owned(),
                serde_json::Value::String("sk-fake-test".to_owned()),
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
        let dir = crate::test_util::temp_dir_unique("iflow");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preserve.json");
        let original = serde_json::json!({
            "selectedAuthType": "iflow",
            "apiKey": "sk-fake-1",
            "foreignKey": "keep-me",
            "anotherForeign": {"nested": 123}
        });
        std::fs::write(&path, serde_json::to_string_pretty(&original).unwrap()).unwrap();
        superai_config::json::edit(&path, |map| {
            map.insert(
                "baseUrl".to_owned(),
                serde_json::Value::String("https://apis.iflow.cn/v1".to_owned()),
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
        assert_eq!(
            after["baseUrl"],
            serde_json::Value::String("https://apis.iflow.cn/v1".to_owned())
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
    fn migration_tip_contains_guide_url() {
        assert!(MIGRATION_TIP.contains("vibex.iflow.cn"));
        assert!(MIGRATION_TIP.contains(SHUTDOWN_DATE));
    }
}
