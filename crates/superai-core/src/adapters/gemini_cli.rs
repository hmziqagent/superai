//! Gemini CLI adapter — relocated-root via `GEMINI_CLI_HOME`, retired 2026-06-18.
//!
//! Research source: `docs/harness-configs/gemini-cli.md` (last verified 2026-08-25).
//! Executable `gemini`, config root `~/.gemini` or `$GEMINI_CLI_HOME`,
//! primary writable surface `settings.json` (JSON), isolation `relocated-root`.
//! Product status `retired`, successor `antigravity-cli` (`agy`).
//! Support `MigrationOnly`: detect/inspect/backup/export with tip, no new defaults, no deletion.

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

/// Harness identifier for Gemini CLI.
pub const HARNESS_ID_STR: &str = "gemini-cli";

/// Human display name.
pub const DISPLAY_NAME: &str = "Gemini CLI";

/// Primary executable name.
pub const EXECUTABLE: &str = "gemini";

/// Environment variable that relocates the config root.
pub const CONFIG_ENV_VAR: &str = "GEMINI_CLI_HOME";

/// Default config root when `GEMINI_CLI_HOME` is unset.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.gemini";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/gemini-cli.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current settings shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Retirement date for consumer tiers.
pub const RETIREMENT_DATE: &str = "2026-06-18";

/// Successor harness id.
pub const SUCCESSOR_ID: &str = "antigravity-cli";

/// Successor executable.
pub const SUCCESSOR_EXECUTABLE: &str = "agy";

/// Tip shown for migration.
pub const MIGRATION_TIP: &str = "Gemini CLI consumer tiers retired 2026-06-18; migrate to Antigravity CLI (agy) via `agy plugin import gemini` — skills .gemini/skills/ -> .gemini/antigravity-cli/skills/, mcpServers url/httpUrl -> serverUrl in mcp_config.json";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Gemini CLI (`MigrationOnly`).
///
/// Isolation is `relocated-root` via `GEMINI_CLI_HOME`. `MigrationOnly` means
/// only detect/inspect/backup/export are supported; new instance creation and
/// deletion are not provided. Every mutating attempt returns a tip to the
/// successor `antigravity-cli`.
#[derive(Debug, Clone)]
pub struct GeminiCliAdapter {
    id: HarnessId,
}

impl GeminiCliAdapter {
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

    /// Successor tip.
    pub fn successor_tip(&self) -> &str {
        MIGRATION_TIP
    }

    /// Try to locate the `gemini` binary via `PATH`.
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

    /// Probe `gemini --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `0.84.0` or `gemini 2.1.0` into version.
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

    /// Resolve the default config root: `$GEMINI_CLI_HOME` or `~/.gemini`.
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
        Some(PathBuf::from(home).join(".gemini"))
    }

    /// Build detection evidence about config root and settings.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!(
            "product retired {RETIREMENT_DATE}, successor {SUCCESSOR_ID} ({SUCCESSOR_EXECUTABLE})"
        ));
        evidence.push(MIGRATION_TIP.to_owned());
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let settings = root.join("settings.json");
                    if settings.exists() {
                        evidence.push(format!("settings.json found at {}", settings.display()));
                    } else {
                        evidence.push(format!("settings.json missing at {}", settings.display()));
                    }
                    let extensions = root.join("extensions");
                    if extensions.exists() {
                        evidence.push(format!(
                            "extensions dir present at {}",
                            extensions.display()
                        ));
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

impl Default for GeminiCliAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "gemini-cli is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for GeminiCliAdapter {
    fn id(&self) -> HarnessId {
        self.id.clone()
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    fn product_status(&self) -> ProductStatus {
        ProductStatus::Retired
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

        let confidence = if present == InstallPresence::Absent {
            DetectionConfidence::High
        } else if binary_path.is_some() && version.is_none()
            || evidence.iter().any(|e| e.contains("config root exists"))
        {
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
            notes.push(format!("detected gemini-cli version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            notes.push(format!(
                "retired {RETIREMENT_DATE}, successor {SUCCESSOR_ID}"
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

        let settings_resolver = PathResolver::new(
            Some("$GEMINI_CLI_HOME/settings.json"),
            Some("$GEMINI_CLI_HOME/settings.json"),
            Some("%GEMINI_CLI_HOME%\\settings.json"),
            "~/.gemini/settings.json",
        );
        let mut settings_surface = ConfigSurface::new(
            "settings.json",
            settings_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        settings_surface.precedence = 10;
        settings_surface.owned_selectors = vec!["model".to_owned(), "general".to_owned()];
        settings_surface.backup_required = true;
        settings_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(settings_surface);

        let trusted_resolver = PathResolver::new(
            Some("$GEMINI_CLI_HOME/trustedFolders.json"),
            Some("$GEMINI_CLI_HOME/trustedFolders.json"),
            Some("%GEMINI_CLI_HOME%\\trustedFolders.json"),
            "~/.gemini/trustedFolders.json",
        );
        let mut trusted_surface = ConfigSurface::new(
            "trustedFolders.json",
            trusted_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        trusted_surface.precedence = 5;
        trusted_surface.backup_required = true;
        surfaces.push(trusted_surface);

        let extensions_resolver = PathResolver::new(
            Some("$GEMINI_CLI_HOME/extensions/<name>/extension.toml"),
            Some("$GEMINI_CLI_HOME/extensions/<name>/extension.toml"),
            Some("%GEMINI_CLI_HOME%\\extensions\\<name>\\extension.toml"),
            "~/.gemini/extensions/<name>/extension.toml",
        );
        let mut extensions_surface = ConfigSurface::new(
            "extensions",
            extensions_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        extensions_surface.precedence = 8;
        extensions_surface.backup_required = true;
        surfaces.push(extensions_surface);

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
            "history.jsonl".to_owned(),
            "debug/*".to_owned(),
            ".credentials.json".to_owned(),
            "oauth/*".to_owned(),
            "sessions/*".to_owned(),
            "telemetry/*".to_owned(),
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
            reason: format!(
                "MigrationOnly: {MIGRATION_TIP} — no new instances; export/backup only"
            ),
        })
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.gemini/settings.json".to_owned(),
            "~/.gemini/trustedFolders.json".to_owned(),
            "~/.gemini/extensions".to_owned(),
            "$GEMINI_CLI_HOME/settings.json via GEMINI_CLI_HOME".to_owned(),
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
                reason: format!(
                    "gemini-cli (MigrationOnly) expects isolation relocated_root, got {other} — {MIGRATION_TIP}"
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

    use super::{
        DISPLAY_NAME, EXECUTABLE, GeminiCliAdapter, HARNESS_ID_STR, MIGRATION_TIP, RESEARCH_DOC,
        SUCCESSOR_ID,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> GeminiCliAdapter {
        GeminiCliAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-gemini-1").unwrap(),
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
        assert_eq!(a.product_status(), ProductStatus::Retired);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert_eq!(a.adapter_revision(), crate::adapter::ADAPTER_REVISION);
        assert!(a.successor_tip().contains(SUCCESSOR_ID));
        assert!(a.successor_tip().contains("agy"));
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
    fn detection_returns_evidence_with_migration_tip() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        assert!(result.evidence.iter().any(|e| e.contains("retired")));
        assert!(result.evidence.iter().any(|e| e.contains(SUCCESSOR_ID)));
        match result.present {
            InstallPresence::Absent => {
                assert!(result.version.is_none());
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
    }

    #[test]
    fn version_resolution_includes_tip() {
        let a = adapter();
        let res = a.version_resolution();
        assert!(!res.notes.is_empty());
        assert!(
            res.notes
                .iter()
                .any(|n| n.contains(SUCCESSOR_ID) || n.contains("migration"))
        );
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("gemini 2.1.0", Some("2.1.0")),
            ("0.9.0", Some("0.9.0")),
            ("v1.2.3", Some("1.2.3")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = GeminiCliAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_settings() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(!surfaces.is_empty());
        let settings = surfaces
            .iter()
            .find(|s| s.id == "settings.json")
            .expect("settings.json surface must exist");
        assert_eq!(settings.kind, DocumentKind::Json);
        assert_eq!(settings.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(settings.scope, ConfigScope::User);
        assert!(settings.backup_required);
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
    }

    #[test]
    fn plan_wrapper_is_blocked_with_tip() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.gemini-work");
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::UnsupportedOperation { reason, .. } => {
                assert!(reason.contains(SUCCESSOR_ID));
                assert!(reason.contains(MIGRATION_TIP) || reason.contains("MigrationOnly"));
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.gemini-work");
        inst.harness = HarnessId::new("codex-cli").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_gemini_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("settings.json")));
        assert!(candidates.iter().any(|c| c.contains("GEMINI_CLI_HOME")));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.gemini-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.gemini-work");
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
        assert!(exclusions.iter().any(|p| p.contains("history")));
    }
}
