//! Qwen Code adapter — relocated-root via `QWEN_HOME`/`QWEN_CODE_*` with layered JSON/env/MCP.
//!
//! Research source: `docs/harness-configs/qwen-code.md` (last verified 2026-08-25).
//! Executable `qwen`, config root `~/.qwen` or `$QWEN_HOME` (also
//! `$QWEN_CODE_SYSTEM_DEFAULTS_PATH`/`$QWEN_CODE_SYSTEM_SETTINGS_PATH` for system layers),
//! primary writable surface `settings.json` (JSON), isolation `relocated-root`.

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

/// Harness identifier for Qwen Code.
pub const HARNESS_ID_STR: &str = "qwen-code";

/// Human display name.
pub const DISPLAY_NAME: &str = "Qwen Code";

/// Primary executable name.
pub const EXECUTABLE: &str = "qwen";

/// Primary environment variable that relocates the config root.
pub const CONFIG_ENV_VAR: &str = "QWEN_HOME";

/// System defaults path override.
pub const SYSTEM_DEFAULTS_ENV_VAR: &str = "QWEN_CODE_SYSTEM_DEFAULTS_PATH";

/// System settings path override.
pub const SYSTEM_SETTINGS_ENV_VAR: &str = "QWEN_CODE_SYSTEM_SETTINGS_PATH";

/// Runtime dir override.
pub const RUNTIME_DIR_ENV_VAR: &str = "QWEN_RUNTIME_DIR";

/// Default config root when relocation vars are unset.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.qwen";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/qwen-code.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current settings shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider/model/mcp mutation inside `settings.json`.
pub const OWNED_SELECTORS: &[&str] = &[
    "model.name",
    "modelProviders",
    "mcpServers",
    "security.auth.selectedType",
    "tools.approvalMode",
    "context.fileName",
    "permissions",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Qwen Code.
///
/// Isolation is `relocated-root` via `QWEN_HOME` (and `QWEN_RUNTIME_DIR` for
/// runtime separation). The wrapper sets `QWEN_HOME` to the instance
/// `config_root` and `QWEN_RUNTIME_DIR` to `<root>/runtime`.
#[derive(Debug, Clone)]
pub struct QwenCodeAdapter {
    id: HarnessId,
}

impl QwenCodeAdapter {
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

    /// Try to locate the `qwen` binary via `PATH`.
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

    /// Probe `qwen --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `qwen 0.1.0` into `0.1.0`.
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

    /// Resolve the default config root: `$QWEN_HOME` or `~/.qwen`.
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
        Some(PathBuf::from(home).join(".qwen"))
    }

    /// Check if default config root exists on disk.
    #[expect(dead_code, reason = "helper for future use")]
    #[expect(clippy::unused_self, reason = "adapter helper")]
    fn default_config_root_exists(&self) -> Option<PathBuf> {
        let root = Self::default_config_root()?;
        if root.exists() { Some(root) } else { None }
    }

    /// Build the settings.json path for a given config root.
    fn settings_path_for_root(root: &Path) -> PathBuf {
        root.join("settings.json")
    }

    /// Build detection evidence about config root and settings.
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
                    let settings = Self::settings_path_for_root(&root);
                    if settings.exists() {
                        evidence.push(format!("settings.json found at {}", settings.display()));
                        if let Ok(text) = std::fs::read_to_string(&settings)
                            && (text.contains("mcpServers") || text.contains("modelProviders"))
                        {
                            evidence.push(
                                "settings.json contains mcpServers/modelProviders".to_owned(),
                            );
                        }
                    } else {
                        evidence.push(format!("settings.json missing at {}", settings.display()));
                    }
                    let env_file = root.join(".env");
                    if env_file.exists() {
                        evidence.push(format!(".env present at {}", env_file.display()));
                    }
                    if root.join("QWEN.md").exists() {
                        evidence.push(format!("QWEN.md present at {}", root.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
        for var in [
            CONFIG_ENV_VAR,
            SYSTEM_DEFAULTS_ENV_VAR,
            SYSTEM_SETTINGS_ENV_VAR,
            RUNTIME_DIR_ENV_VAR,
        ] {
            if let Ok(dir) = std::env::var(var)
                && !dir.trim().is_empty()
            {
                evidence.push(format!("{var} set to {dir}"));
            } else {
                evidence.push(format!("{var} not set"));
            }
        }
        // Check project .qwen/settings.json
        let project_settings = Path::new(".qwen").join("settings.json");
        if project_settings.exists() {
            evidence.push(format!(
                "project settings found at {}",
                project_settings.display()
            ));
        }
        let project_qwen_md = Path::new("QWEN.md");
        if project_qwen_md.exists() {
            evidence.push(format!("QWEN.md found at {}", project_qwen_md.display()));
        }
    }
}

impl Default for QwenCodeAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "qwen-code is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for QwenCodeAdapter {
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
            notes.push(format!("detected qwen-code version {v}"));
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

        let user_resolver = PathResolver::new(
            Some("$QWEN_HOME/settings.json"),
            Some("$QWEN_HOME/settings.json"),
            Some("%QWEN_HOME%\\settings.json"),
            "~/.qwen/settings.json",
        );
        let mut user_settings = ConfigSurface::new(
            "settings.json",
            user_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        user_settings.precedence = 10;
        user_settings.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        user_settings.backup_required = true;
        user_settings.restart_behavior = RestartBehavior::Reload;
        surfaces.push(user_settings);

        let project_resolver = PathResolver::fallback_only(".qwen/settings.json (project)");
        let mut project_settings = ConfigSurface::new(
            "project.settings.json",
            project_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_settings.precedence = 20;
        project_settings.owned_selectors = vec!["mcpServers".to_owned(), "model".to_owned()];
        project_settings.backup_required = true;
        surfaces.push(project_settings);

        let defaults_resolver = PathResolver::new(
            Some("$QWEN_CODE_SYSTEM_DEFAULTS_PATH"),
            Some("$QWEN_CODE_SYSTEM_DEFAULTS_PATH"),
            Some("%QWEN_CODE_SYSTEM_DEFAULTS_PATH%"),
            "/etc/qwen-code/system-defaults.json",
        );
        let mut system_defaults = ConfigSurface::new(
            "system-defaults.json",
            defaults_resolver,
            DocumentKind::Json,
            ConfigScope::SystemManaged,
            SurfaceOwnership::HarnessManaged,
        );
        system_defaults.precedence = 1;
        system_defaults.backup_required = false;
        system_defaults.restart_behavior = RestartBehavior::None;
        surfaces.push(system_defaults);

        let system_resolver = PathResolver::new(
            Some("$QWEN_CODE_SYSTEM_SETTINGS_PATH"),
            Some("$QWEN_CODE_SYSTEM_SETTINGS_PATH"),
            Some("%QWEN_CODE_SYSTEM_SETTINGS_PATH%"),
            "/etc/qwen-code/settings.json",
        );
        let mut system_settings = ConfigSurface::new(
            "system.settings.json",
            system_resolver,
            DocumentKind::Json,
            ConfigScope::SystemManaged,
            SurfaceOwnership::HarnessManaged,
        );
        system_settings.precedence = 2;
        system_settings.backup_required = false;
        surfaces.push(system_settings);

        let env_resolver = PathResolver::new(
            Some("$QWEN_HOME/.env"),
            Some("$QWEN_HOME/.env"),
            Some("%QWEN_HOME%\\.env"),
            "~/.qwen/.env",
        );
        let mut env_surface = ConfigSurface::new(
            ".env",
            env_resolver,
            DocumentKind::Env,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        env_surface.precedence = 15;
        env_surface.backup_required = true;
        surfaces.push(env_surface);

        let mcp_resolver = PathResolver::new(
            Some("$QWEN_HOME/settings.json (mcpServers)"),
            Some("$QWEN_HOME/settings.json (mcpServers)"),
            Some("%QWEN_HOME%\\settings.json (mcpServers)"),
            "~/.qwen/settings.json (mcpServers)",
        );
        let mut mcp = ConfigSurface::new(
            "mcpServers",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp.precedence = 12;
        mcp.owned_selectors = vec!["mcpServers".to_owned()];
        mcp.backup_required = true;
        surfaces.push(mcp);

        let qwen_md_resolver = PathResolver::new(
            Some("$QWEN_HOME/QWEN.md"),
            Some("$QWEN_HOME/QWEN.md"),
            Some("%QWEN_HOME%\\QWEN.md"),
            "~/.qwen/QWEN.md",
        );
        let mut qwen_md = ConfigSurface::new(
            "QWEN.md",
            qwen_md_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        qwen_md.precedence = 8;
        qwen_md.backup_required = false;
        surfaces.push(qwen_md);

        let runtime_resolver = PathResolver::new(
            Some("$QWEN_RUNTIME_DIR"),
            Some("$QWEN_RUNTIME_DIR"),
            Some("%QWEN_RUNTIME_DIR%"),
            "~/.qwen/runtime",
        );
        let mut runtime = ConfigSurface::new(
            "runtime",
            runtime_resolver,
            DocumentKind::Opaque,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        runtime.precedence = 0;
        runtime.backup_required = false;
        surfaces.push(runtime);

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
            "sessions/*".to_owned(),
            "history/*".to_owned(),
            "conversations/*".to_owned(),
            "logs/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "tmp/*".to_owned(),
            "*.lock".to_owned(),
            "telemetry/*".to_owned(),
            "runtime/*".to_owned(),
            "state/*".to_owned(),
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
        let mut plan = WrapperPlan::new("relocated-root via QWEN_HOME with runtime isolation");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        // Separate runtime dir per instance to avoid session pollution.
        let runtime_dir = format!("{}/runtime", instance.config_root);
        plan.env_vars
            .push((RUNTIME_DIR_ENV_VAR.to_owned(), runtime_dir));
        plan.description = format!(
            " Wrapper sets {}={} {}={}/runtime and execs `{}`",
            CONFIG_ENV_VAR,
            instance.config_root,
            RUNTIME_DIR_ENV_VAR,
            instance.config_root,
            EXECUTABLE
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.qwen".to_owned(),
            "~/.qwen-work".to_owned(),
            "~/.qwen-glm".to_owned(),
            "$QWEN_HOME".to_owned(),
            "$QWEN_CODE_SYSTEM_DEFAULTS_PATH".to_owned(),
            "$QWEN_CODE_SYSTEM_SETTINGS_PATH".to_owned(),
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
            Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("qwen-code requires isolation relocated_root, got {other}"),
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

    use super::QwenCodeAdapter;
    use super::{
        CONFIG_ENV_VAR, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, OWNED_SELECTORS, RESEARCH_DOC,
        RUNTIME_DIR_ENV_VAR,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> QwenCodeAdapter {
        QwenCodeAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-qwen-1").unwrap(),
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
            ("qwen 0.1.0", Some("0.1.0")),
            ("0.2.0", Some("0.2.0")),
            ("v1.0.0", Some("1.0.0")),
            ("Version: 2.0.0", Some("2.0.0")),
            ("1.0.0-alpha", Some("1.0.0-alpha")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = QwenCodeAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_writable_settings() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let settings = surfaces
            .iter()
            .find(|s| s.id == "settings.json")
            .expect("settings.json surface must exist");
        assert_eq!(settings.kind, DocumentKind::Json);
        assert_eq!(settings.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(settings.scope, ConfigScope::User);
        assert!(settings.backup_required);
        for selector in ["model.name", "modelProviders", "mcpServers"] {
            assert!(
                settings.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in &settings.owned_selectors {
            assert!(!sel.is_empty());
        }
        for sel in OWNED_SELECTORS {
            assert!(settings.owned_selectors.contains(&(*sel).to_owned()));
        }

        let system = surfaces
            .iter()
            .find(|s| s.id == "system-defaults.json")
            .expect("system-defaults.json");
        assert_eq!(system.scope, ConfigScope::SystemManaged);

        let env = surfaces.iter().find(|s| s.id == ".env").expect(".env");
        assert_eq!(env.kind, DocumentKind::Env);
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
        let must_contain = ["sessions/*", "history/*", "logs/*", "cache/*", "*.lock"];
        for pat in must_contain {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"settings.json".to_owned()));
    }

    #[test]
    #[expect(clippy::excessive_nesting, reason = "test closure nesting is explicit")]
    fn plan_mirror_includes_settings_and_excludes_sessions() {
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
        assert!(!is_excluded("settings.json"));
        assert!(!is_excluded("QWEN.md"));
        assert!(is_excluded("sessions/abc.jsonl"));
        assert!(is_excluded("history/history.jsonl"));
        assert!(is_excluded("logs/qwen.log"));
        assert!(is_excluded("runtime/todo.json"));
    }

    #[test]
    fn plan_wrapper_sets_qwen_home_and_runtime() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.qwen-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == "/tmp/.qwen-work")
        );
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == RUNTIME_DIR_ENV_VAR && v.contains("/tmp/.qwen-work"))
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(CONFIG_ENV_VAR));
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my qwen work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == CONFIG_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, "/tmp/my qwen work");
        assert!(!env_val.contains('"'));
        assert!(env_val.contains(' '));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.qwen-work");
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
        assert!(candidates.iter().any(|c| c.contains(".qwen")));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.qwen-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.qwen-work");
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
        let mut inst = sample_instance_with_root("/tmp/.qwen-work");
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let settings = surfaces.iter().find(|s| s.id == "settings.json").unwrap();
        assert_eq!(settings.path_resolver.fallback, "~/.qwen/settings.json");
        let resolver = &settings.path_resolver;
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
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/qwen_code")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.json");
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.is_empty());
        let value = superai_config::json::load_value(&path).unwrap();
        assert_eq!(value, serde_json::Value::Object(serde_json::Map::default()));
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("settings.minimal.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(map.is_empty() || map.contains_key("model") || map.len() <= 2);
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(
            map.contains_key("model")
                || map.contains_key("modelProviders")
                || map.contains_key("mcpServers")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("qwen");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("settings.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("qwen3-coder-plus".to_owned()),
            );
            assert!(map.contains_key("foreignKey") || map.contains_key("unknownTopLevel"));
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert_eq!(
            after["model"],
            serde_json::Value::String("qwen3-coder-plus".to_owned())
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
        let path = fixture_path("settings.malformed.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::json::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn secret_redaction_placeholder() {
        use crate::error::RedactedString;
        let secret = RedactedString::new("sk-qwen-secret-789");
        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert!(!debug.contains("sk-qwen-secret-789"));
        assert!(!display.contains("sk-qwen-secret-789"));
        assert!(debug.contains("[REDACTED]"));
        assert!(display.contains("[REDACTED]"));
        let json = serde_json::to_string(&secret).unwrap();
        assert!(!json.contains("sk-qwen-secret-789"));
        assert!(json.contains("[REDACTED]"));
        assert_eq!(secret.expose_secret(), "sk-qwen-secret-789");
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
        let inst = sample_instance_with_root("/tmp/.qwen-work");
        a.validate_instance(&inst).unwrap();
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn wrapper_env_var_isolation_is_relocated_root() {
        let a = adapter();
        assert!(a.scan_candidates().len() >= 3);
        let inst = sample_instance_with_root("/home/user/.qwen-isolated");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(!plan.env_vars.is_empty());
        let (key, val) = &plan.env_vars[0];
        assert_eq!(key, CONFIG_ENV_VAR);
        assert_eq!(val, "/home/user/.qwen-isolated");
    }

    #[test]
    fn registry_no_harness_value_leak() {
        let inst = sample_instance_with_root("/tmp/.qwen-work");
        let json = serde_json::to_string(&inst).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let forbidden = [
            "model", "endpoint", "api_key", "skill", "plugin", "mcp", "baseUrl", "base_url",
        ];
        let text = json.to_lowercase();
        for field in forbidden {
            if let serde_json::Value::Object(map) = &v {
                assert!(
                    !map.contains_key(field),
                    "forbidden field `{field}` must not be emitted, json: {json}"
                );
            }
            assert!(
                !text.contains(&format!("\"{field}\"")),
                "forbidden field `{field}` appears in json: {json}"
            );
        }
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
