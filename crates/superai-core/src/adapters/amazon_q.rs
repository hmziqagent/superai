//! Amazon Q Developer CLI adapter — sunsetting, `MigrationOnly`.
//!
//! Research source: `docs/harness-configs/amazon-q-cli.md` (last verified 2026-08-25).
//! Executable `q`, config `~/.aws/amazonq/settings.json` plus `cli-agents/*.json`,
//! `rules/*.md`, `AmazonQ.md`, MCP inside agent JSON, isolation `project_scope`.
//! Product status `sunset`, successor `kiro` (Kiro CLI).

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

/// Harness identifier for Amazon Q Developer CLI.
pub const HARNESS_ID_STR: &str = "amazon-q-cli";

/// Human display name.
pub const DISPLAY_NAME: &str = "Amazon Q Developer CLI";

/// Primary executable name.
pub const EXECUTABLE: &str = "q";

/// Default config root fallback.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.aws/amazonq";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/amazon-q-cli.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Sunset announcement.
pub const SUNSET_NOTE: &str = "sunsetting 2026-05-15, EOS 2027-04-30";

/// Successor harness id.
pub const SUCCESSOR_ID: &str = "kiro";

/// Migration tip.
pub const MIGRATION_TIP: &str = "Amazon Q Developer CLI is sunsetting (no new signups 2026-05-15, EOS 2027-04-30); migrate to kiro (Kiro CLI) — export settings.json, cli-agents/*.json, rules/*.md, and mcpServers from agent JSON";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Amazon Q Developer CLI (`MigrationOnly`).
#[derive(Debug, Clone)]
pub struct AmazonQAdapter {
    id: HarnessId,
}

impl AmazonQAdapter {
    /// Create a new adapter instance.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    /// Borrow the harness id.
    pub fn harness_id(&self) -> &HarnessId {
        &self.id
    }

    /// Executable name.
    pub fn executable_name(&self) -> &str {
        EXECUTABLE
    }

    /// Migration tip.
    pub fn successor_tip(&self) -> &str {
        MIGRATION_TIP
    }

    /// Try to locate the `q` binary via `PATH`.
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

    /// Probe `q --version` with a timeout.
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

    /// Resolve default config root `~/.aws/amazonq`.
    fn default_config_root() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".aws").join("amazonq"))
    }

    /// Collect evidence.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!(
            "product sunset: {SUNSET_NOTE}, successor {SUCCESSOR_ID}"
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
                    let agents = root.join("cli-agents");
                    if agents.exists() {
                        evidence.push(format!("cli-agents dir present at {}", agents.display()));
                    }
                    let rules = root.join("rules");
                    if rules.exists() {
                        evidence.push(format!("rules dir present at {}", rules.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve home for config lookup".to_owned());
            }
        }
    }
}

impl Default for AmazonQAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "amazon-q-cli is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for AmazonQAdapter {
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
            notes.push(format!("detected amazon-q-cli version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            notes.push(format!("sunset {SUNSET_NOTE}"));
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
            Some("~/.aws/amazonq/settings.json"),
            Some("~/.aws/amazonq/settings.json"),
            Some("%USERPROFILE%\\.aws\\amazonq\\settings.json"),
            "~/.aws/amazonq/settings.json",
        );
        let mut settings_surface = ConfigSurface::new(
            "settings.json",
            settings_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        settings_surface.precedence = 10;
        settings_surface.owned_selectors = vec!["chat.defaultModel".to_owned()];
        settings_surface.backup_required = true;
        settings_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(settings_surface);

        let agents_resolver = PathResolver::new(
            Some("~/.aws/amazonq/cli-agents/*.json"),
            Some("~/.aws/amazonq/cli-agents/*.json"),
            Some("%USERPROFILE%\\.aws\\amazonq\\cli-agents\\*.json"),
            "~/.aws/amazonq/cli-agents/*.json",
        );
        let mut agents_surface = ConfigSurface::new(
            "cli-agents",
            agents_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        agents_surface.precedence = 12;
        agents_surface.owned_selectors = vec!["mcpServers".to_owned(), "model".to_owned()];
        agents_surface.backup_required = true;
        surfaces.push(agents_surface);

        let rules_resolver = PathResolver::new(
            Some("~/.aws/amazonq/rules/*.md"),
            Some("~/.aws/amazonq/rules/*.md"),
            Some("%USERPROFILE%\\.aws\\amazonq\\rules\\*.md"),
            "~/.aws/amazonq/rules/*.md",
        );
        let mut rules_surface = ConfigSurface::new(
            "rules",
            rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        rules_surface.precedence = 8;
        rules_surface.backup_required = false;
        surfaces.push(rules_surface);

        let project_resolver = PathResolver::fallback_only(".amazonq/rules/*.md (project)");
        let mut project_rules = ConfigSurface::new(
            "project-rules",
            project_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_rules.precedence = 15;
        project_rules.backup_required = false;
        surfaces.push(project_rules);

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
            "history/*".to_owned(),
            "sessions/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
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
            "~/.aws/amazonq/settings.json".to_owned(),
            "~/.aws/amazonq/cli-agents".to_owned(),
            "~/.aws/amazonq/rules".to_owned(),
            ".amazonq/rules (project)".to_owned(),
            "AmazonQ.md (project)".to_owned(),
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
            Isolation::ProjectScope | Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "amazon-q-cli (MigrationOnly) expects isolation project_scope, got {other} — {MIGRATION_TIP}"
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
        AmazonQAdapter, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, RESEARCH_DOC, SUCCESSOR_ID,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> AmazonQAdapter {
        AmazonQAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-amazon-q-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::ProjectScope,
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
        assert_eq!(a.product_status(), ProductStatus::Sunset);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
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
    fn detection_returns_evidence_with_migration_tip() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.contains("sunset") || e.contains("EOS"))
        );
        assert!(result.evidence.iter().any(|e| e.contains(SUCCESSOR_ID)));
        match result.present {
            InstallPresence::Absent
            | InstallPresence::Present
            | InstallPresence::UnknownVersion
            | InstallPresence::Broken => {
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
            ("q 1.12.0", Some("1.12.0")),
            ("1.0.0", Some("1.0.0")),
            ("v2.0.1", Some("2.0.1")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = AmazonQAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_settings_and_agents() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 3);
        let settings = surfaces
            .iter()
            .find(|s| s.id == "settings.json")
            .expect("settings.json must exist");
        assert_eq!(settings.kind, DocumentKind::Json);
        assert_eq!(settings.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(settings.scope, ConfigScope::User);
        assert!(settings.backup_required);

        let agents = surfaces
            .iter()
            .find(|s| s.id == "cli-agents")
            .expect("cli-agents must exist");
        assert_eq!(agents.kind, DocumentKind::Json);
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
    }

    #[test]
    fn plan_wrapper_is_blocked() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.aws-amazonq-work");
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::UnsupportedOperation { reason, .. } => {
                assert!(reason.contains(SUCCESSOR_ID) || reason.contains("MigrationOnly"));
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_amazonq_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("amazonq")));
    }

    #[test]
    fn validate_instance_accepts_project_scope() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.aws-amazonq-work");
        a.validate_instance(&inst).unwrap();
        let mut inst2 = sample_instance_with_root("/tmp/.aws-amazonq-work2");
        inst2.isolation = Isolation::RelocatedRoot;
        a.validate_instance(&inst2).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.aws-amazonq-work");
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
}
