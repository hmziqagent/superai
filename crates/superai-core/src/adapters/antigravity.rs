//! Antigravity CLI adapter — `HOME` workaround, `ResearchBlocked`.
//!
//! Research source: `docs/harness-configs/antigravity-cli.md` (last verified 2026-08-25).
//! Executable `agy`, config `~/.gemini/antigravity-cli/settings.json` (JSON) plus
//! `~/.gemini/config/mcp_config.json` and `~/.gemini/antigravity-cli/skills/`,
//! isolation `os_bound` via `HOME` workaround (no documented config-dir env var),
//! product status `preview`, support `ResearchBlocked` until full path/auth/MCP
//! and concurrency gaps are closed. Detect only.

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

/// Harness identifier for Antigravity CLI.
pub const HARNESS_ID_STR: &str = "antigravity-cli";

/// Human display name.
pub const DISPLAY_NAME: &str = "Antigravity CLI";

/// Primary executable name.
pub const EXECUTABLE: &str = "agy";

/// Alternative binary name (installer).
pub const EXECUTABLE_ALT: &str = "antigravity";

/// Default config root fallback (under ~/.gemini).
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.gemini/antigravity-cli";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/antigravity-cli.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// HOME workaround note.
pub const HOME_WORKAROUND_NOTE: &str = "no documented AGY_HOME/CONFIG_DIR; HOME relocation is undocumented workaround — verify before relying, especially on macOS keychain";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Antigravity CLI (`ResearchBlocked`).
///
/// Only detection with evidence is supported. Writes are blocked until
/// research gaps close: full settings schema, auth/keyring behavior per OS,
/// plugin/skill/MCP paths, sandbox flags, and HOME concurrency.
#[derive(Debug, Clone)]
pub struct AntigravityAdapter {
    id: HarnessId,
}

impl AntigravityAdapter {
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

    /// Research-blocked reason.
    pub fn blocked_reason(&self) -> &str {
        HOME_WORKAROUND_NOTE
    }

    /// Try to locate `agy` binary via PATH, fallback to `antigravity`.
    #[expect(clippy::unused_self, reason = "adapter method uses instance constants")]
    #[expect(clippy::excessive_nesting, reason = "PATH scan branches are explicit")]
    fn find_binary_in_path(&self) -> Option<PathBuf> {
        let path_var = std::env::var("PATH").ok()?;
        let separator = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(separator) {
            if dir.is_empty() {
                continue;
            }
            for exec in [EXECUTABLE, EXECUTABLE_ALT] {
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

    /// Probe `agy --version` with timeout.
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

    /// Resolve default HOME.
    fn default_home() -> Option<PathBuf> {
        if let Ok(home) = std::env::var("HOME")
            && !home.trim().is_empty()
        {
            return Some(PathBuf::from(home));
        }
        if let Ok(home) = std::env::var("USERPROFILE")
            && !home.trim().is_empty()
        {
            return Some(PathBuf::from(home));
        }
        None
    }

    /// Collect evidence including research-blocked explanation.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("research blocked: {HOME_WORKAROUND_NOTE}"));
        evidence.push("gaps: settings schema, auth/keyring per OS, plugin/skill/MCP paths, sandbox, HOME concurrency".to_owned());
        evidence
            .push("successor of gemini-cli; migration via `agy plugin import gemini`".to_owned());
        match Self::default_home() {
            Some(home) => {
                let root = home.join(".gemini").join("antigravity-cli");
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let settings = root.join("settings.json");
                    if settings.exists() {
                        evidence.push(format!("settings.json found at {}", settings.display()));
                    } else {
                        evidence.push(format!("settings.json missing at {}", settings.display()));
                    }
                    let skills = root.join("skills");
                    if skills.exists() {
                        evidence.push(format!("skills dir present at {}", skills.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
                let global_mcp = home.join(".gemini").join("config").join("mcp_config.json");
                if global_mcp.exists() {
                    evidence.push(format!(
                        "global mcp_config.json found at {}",
                        global_mcp.display()
                    ));
                }
            }
            None => {
                evidence.push("could not resolve home for antigravity lookup".to_owned());
            }
        }
    }
}

impl Default for AntigravityAdapter {
    fn default() -> Self {
        #[expect(
            clippy::unwrap_used,
            reason = "antigravity-cli is static valid HarnessId"
        )]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for AntigravityAdapter {
    fn id(&self) -> HarnessId {
        self.id.clone()
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    fn product_status(&self) -> ProductStatus {
        ProductStatus::Preview
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
            if evidence.iter().any(|e| e.contains("config root exists")) {
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
            notes.push(format!("detected antigravity-cli version {v}"));
            notes.push(format!("research blocked — {HOME_WORKAROUND_NOTE}"));
            let mut res = VersionResolution::new(Some(v), None, false);
            res.notes = notes;
            res
        } else {
            let mut res = VersionResolution::unknown();
            res.notes = detection.evidence;
            res.notes
                .push(format!("research blocked: {HOME_WORKAROUND_NOTE}"));
            res
        }
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let settings_resolver = PathResolver::new(
            Some("~/.gemini/antigravity-cli/settings.json"),
            Some("~/.gemini/antigravity-cli/settings.json"),
            Some("%USERPROFILE%\\.gemini\\antigravity-cli\\settings.json"),
            "~/.gemini/antigravity-cli/settings.json",
        );
        let mut settings_surface = ConfigSurface::new(
            "settings.json",
            settings_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        settings_surface.precedence = 10;
        settings_surface.owned_selectors = Vec::new();
        settings_surface.backup_required = true;
        settings_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(settings_surface);

        let mcp_resolver = PathResolver::new(
            Some("~/.gemini/config/mcp_config.json"),
            Some("~/.gemini/config/mcp_config.json"),
            Some("%USERPROFILE%\\.gemini\\config\\mcp_config.json"),
            "~/.gemini/config/mcp_config.json",
        );
        let mut mcp_surface = ConfigSurface::new(
            "mcp_config.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp_surface.precedence = 12;
        mcp_surface.owned_selectors = vec!["mcpServers".to_owned()];
        mcp_surface.backup_required = true;
        surfaces.push(mcp_surface);

        let skills_resolver = PathResolver::new(
            Some("~/.gemini/antigravity-cli/skills/<name>.md"),
            Some("~/.gemini/antigravity-cli/skills/<name>.md"),
            Some("%USERPROFILE%\\.gemini\\antigravity-cli\\skills\\<name>.md"),
            "~/.gemini/antigravity-cli/skills/<name>.md",
        );
        let mut skills_surface = ConfigSurface::new(
            "skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills_surface.precedence = 8;
        skills_surface.backup_required = false;
        surfaces.push(skills_surface);

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
            "history/*".to_owned(),
            "sessions/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
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
                "ResearchBlocked: {HOME_WORKAROUND_NOTE} — detect only; HOME workaround not verified for concurrent wrappers"
            ),
        })
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.gemini/antigravity-cli/settings.json".to_owned(),
            "~/.gemini/antigravity-cli/skills".to_owned(),
            "~/.gemini/config/mcp_config.json".to_owned(),
            ".agents/skills (project)".to_owned(),
            ".agents/mcp_config.json (project)".to_owned(),
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
                "ResearchBlocked: {HOME_WORKAROUND_NOTE} — validate blocked until HOME isolation verified"
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

    use super::{AntigravityAdapter, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, RESEARCH_DOC};
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> AntigravityAdapter {
        AntigravityAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-antigravity-1").unwrap(),
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
        assert_eq!(a.product_status(), ProductStatus::Preview);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert!(a.blocked_reason().contains("HOME"));
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
        assert!(result.evidence.iter().any(|e| e.contains("HOME")));
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
                .any(|n| n.contains("research blocked") || n.contains("HOME"))
        );
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("agy 1.1.17", Some("1.1.17")),
            ("1.0.7", Some("1.0.7")),
            ("v1.2.0", Some("1.2.0")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = AntigravityAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_exist() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(!surfaces.is_empty());
        let settings = surfaces
            .iter()
            .find(|s| s.id == "settings.json")
            .expect("settings.json must exist");
        assert_eq!(settings.kind, DocumentKind::Json);
        assert_eq!(settings.ownership, SurfaceOwnership::HarnessManaged);
        assert_eq!(settings.scope, ConfigScope::User);
    }

    #[test]
    fn supported_operations_are_research_blocked() {
        let a = adapter();
        let ops = a.supported_operations();
        for (_, support) in ops {
            assert_eq!(support, AdapterSupport::ResearchBlocked);
        }
    }

    #[test]
    fn plan_wrapper_is_research_blocked() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.gemini-antigravity-work");
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::ResearchBlocked { reason, .. } => {
                assert!(reason.contains("HOME") || reason.contains("ResearchBlocked"));
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn validate_instance_is_research_blocked() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.gemini-antigravity-work");
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::ResearchBlocked { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_antigravity_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("antigravity-cli")));
        assert!(candidates.iter().any(|c| c.contains("mcp_config")));
    }

    #[test]
    fn supported_skill_modes_is_empty() {
        let a = adapter();
        assert!(a.supported_skill_modes().is_empty());
    }
}
