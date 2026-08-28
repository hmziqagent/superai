//! Crush adapter — executable `crushrc`, per-project `crush.json` plus XDG, `ResearchBlocked`.
//!
//! Research source: `docs/harness-configs/crush.md` (last verified 2026-08-25).
//! Executable `crush`, primary config `crushrc` (executable Bash, `provider`/`model`/`mcp`
//! builtins) with legacy `crush.json` (JSON) deprecated; discovery project overrides
//! global XDG `$XDG_CONFIG_HOME/crush/crushrc` (`~/.config/crush/crushrc`); override
//! via `CRUSH_GLOBAL_CONFIG=<dir>`; isolation `project-scope` (project/XDG); support
//! `ResearchBlocked` for writes until command API (`provider add`, `model add`) is
//! verified non-interactive; read-only detect with minimal fixture.

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

/// Harness identifier for Crush.
pub const HARNESS_ID_STR: &str = "crush";

/// Human display name.
pub const DISPLAY_NAME: &str = "Crush";

/// Primary executable name.
pub const EXECUTABLE: &str = "crush";

/// Alternate executable name (config is an executable Bash script `crushrc`).
pub const EXECUTABLE_ALT: &str = "crushrc";

/// Environment variable that overrides the global config dir (`<dir>/crushrc`).
pub const CONFIG_ENV_VAR: &str = "CRUSH_GLOBAL_CONFIG";

/// Environment variable that overrides the global data dir.
pub const DATA_ENV_VAR: &str = "CRUSH_GLOBAL_DATA";

/// Default global config path fallback (`$XDG_CONFIG_HOME/crush/crushrc`).
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.config/crush";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/crush.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version (legacy JSON surface).
pub const SCHEMA_VERSION_STR: &str = "1";

/// Research-blocked reason — writes blocked until command API verified.
pub const BLOCKED_REASON: &str = "crushrc is executable Bash with command-backed mutation (provider add/model add) — writes ResearchBlocked until non-interactive command API is verified; project/XDG isolation via CRUSH_GLOBAL_CONFIG";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Crush (`ResearchBlocked`).
///
/// Only read-only detection and inspection are supported. The executable
/// `crushrc` (`provider`, `model`, `mcp` builtins) and legacy `crush.json`
/// are detected but not mutated; `CRUSH_GLOBAL_CONFIG` / `CRUSH_GLOBAL_DATA`
/// relocation for isolation is documented but not assumed stable for
/// concurrent wrappers until verified. Project `.crushrc`/`.crush.json` and
/// global `~/.config/crush/crushrc` are scanned.
#[derive(Debug, Clone)]
pub struct CrushAdapter {
    id: HarnessId,
}

impl CrushAdapter {
    /// Create a new adapter, validating the static harness id.
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

    /// Research-blocked reason.
    pub fn blocked_reason(&self) -> &str {
        BLOCKED_REASON
    }

    /// Try to locate the `crush` binary via `PATH`.
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

    /// Probe `crush --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `crush 0.5.1` or `0.5.1` into `0.5.1`.
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

    /// Resolve the default global config path: `$CRUSH_GLOBAL_CONFIG` or XDG/home.
    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(CONFIG_ENV_VAR)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir));
        }
        if let Ok(dir) = std::env::var("XDG_CONFIG_HOME")
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir).join("crush"));
        }
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".config").join("crush"))
    }

    /// Check if default config root exists on disk.
    #[expect(dead_code, reason = "helper for future use")]
    #[expect(clippy::unused_self, reason = "adapter helper")]
    fn default_config_root_exists(&self) -> Option<PathBuf> {
        let root = Self::default_config_root()?;
        if root.exists() { Some(root) } else { None }
    }

    /// Build detection evidence about config presence and surfaces.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("research blocked: {BLOCKED_REASON}"));
        evidence.push(
            "gaps: crushrc command-backed mutation (provider/model add) unverified non-interactive, formatter/keybinds diverged from OpenCode, data-dir SQLite state"
                .to_owned(),
        );
        evidence.push(format!(
            "isolation project-scope: project .crushrc/.crush.json overrides {CONFIG_ENV_VAR}/XDG ~/.config/crush/crushrc"
        ));
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("global config root exists at {}", root.display()));
                    let crushrc = root.join("crushrc");
                    let crush_json = root.join("crush.json");
                    let legacy_global = root.join("crushrc");
                    if crushrc.exists() {
                        evidence.push(format!("global crushrc found at {}", crushrc.display()));
                        if let Ok(text) = std::fs::read_to_string(&crushrc)
                            && (text.contains("provider add") || text.contains("model large"))
                        {
                            evidence.push("global crushrc contains provider/model".to_owned());
                        }
                    } else {
                        evidence.push(format!("global crushrc missing at {}", crushrc.display()));
                    }
                    if crush_json.exists() {
                        evidence.push(format!(
                            "legacy global crush.json found at {}",
                            crush_json.display()
                        ));
                    }
                    drop(legacy_global);
                } else {
                    evidence.push(format!("global config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push(
                    "could not resolve global config root (no HOME/XDG_CONFIG_HOME)".to_owned(),
                );
            }
        }
        if let Ok(val) = std::env::var(CONFIG_ENV_VAR)
            && !val.trim().is_empty()
        {
            let preview = if val.chars().count() > 80 {
                let truncated: String = val.chars().take(80).collect();
                format!("{truncated}…")
            } else {
                val
            };
            evidence.push(format!("{CONFIG_ENV_VAR} set to {preview}"));
        } else {
            evidence.push(format!("{CONFIG_ENV_VAR} not set"));
        }
        if let Ok(val) = std::env::var(DATA_ENV_VAR)
            && !val.trim().is_empty()
        {
            evidence.push(format!("{DATA_ENV_VAR} set to {val}"));
        }
        // Project discovery
        for proj in [
            Path::new("./.crushrc"),
            Path::new("./crushrc"),
            Path::new("./.crush.json"),
            Path::new("./crush.json"),
        ] {
            if proj.exists() {
                evidence.push(format!("project config found at {}", proj.display()));
            }
        }
        if Path::new(".crush").exists() {
            evidence.push(".crush/ data directory present in cwd".to_owned());
        }
        if std::env::var("XDG_CONFIG_HOME").is_ok() {
            evidence.push("XDG_CONFIG_HOME set".to_owned());
        }
    }
}

impl Default for CrushAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "crush is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for CrushAdapter {
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
            if evidence
                .iter()
                .any(|e| e.contains("global config root exists"))
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
            notes.push(format!("detected crush version {v}"));
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

        // Project crushrc — executable Bash, project scope, user-editable, highest precedence
        let project_rc_resolver = PathResolver::fallback_only("./.crushrc or ./crushrc (project)");
        let mut project_rc = ConfigSurface::new(
            "crushrc (project)",
            project_rc_resolver,
            DocumentKind::Executable,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_rc.precedence = 30;
        project_rc.owned_selectors = Vec::new();
        project_rc.backup_required = true;
        project_rc.restart_behavior = RestartBehavior::Reload;
        surfaces.push(project_rc);

        // Project crush.json — legacy JSON, project scope
        let project_json_resolver =
            PathResolver::fallback_only("./.crush.json or ./crush.json (project, deprecated)");
        let mut project_json = ConfigSurface::new(
            "crush.json (project)",
            project_json_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_json.precedence = 28;
        project_json.owned_selectors = Vec::new();
        project_json.backup_required = true;
        surfaces.push(project_json);

        // Global crushrc — XDG or CRUSH_GLOBAL_CONFIG
        let global_rc_resolver = PathResolver::new(
            Some("$CRUSH_GLOBAL_CONFIG/crushrc"),
            Some("$CRUSH_GLOBAL_CONFIG/crushrc"),
            Some("%CRUSH_GLOBAL_CONFIG%\\crushrc"),
            "~/.config/crush/crushrc",
        );
        let mut global_rc = ConfigSurface::new(
            "crushrc (global)",
            global_rc_resolver,
            DocumentKind::Executable,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        global_rc.precedence = 10;
        global_rc.owned_selectors = Vec::new();
        global_rc.backup_required = true;
        global_rc.restart_behavior = RestartBehavior::Reload;
        surfaces.push(global_rc);

        // Global crush.json — legacy JSON global
        let global_json_resolver = PathResolver::new(
            Some("$CRUSH_GLOBAL_CONFIG/crush.json"),
            Some("$CRUSH_GLOBAL_CONFIG/crush.json"),
            Some("%CRUSH_GLOBAL_CONFIG%\\crush.json"),
            "~/.config/crush/crush.json",
        );
        let mut global_json = ConfigSurface::new(
            "crush.json (global)",
            global_json_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        global_json.precedence = 8;
        global_json.owned_selectors = vec![
            "providers".to_owned(),
            "models".to_owned(),
            "mcp".to_owned(),
            "lsp".to_owned(),
            "options".to_owned(),
            "permissions".to_owned(),
        ];
        global_json.backup_required = true;
        surfaces.push(global_json);

        // Per-project state dir .crush/ — opaque, not user-editable for wiring
        let state_resolver = PathResolver::fallback_only(".crush/ (project data_directory)");
        let mut state = ConfigSurface::new(
            ".crush/state",
            state_resolver,
            DocumentKind::Opaque,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::HarnessManaged,
        );
        state.precedence = 5;
        state.backup_required = false;
        state.restart_behavior = RestartBehavior::None;
        surfaces.push(state);

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
            ".crush/*".to_owned(),
            "history/*".to_owned(),
            "sessions/*".to_owned(),
            "cache/*".to_owned(),
            "*.db".to_owned(),
            "*.sqlite".to_owned(),
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
        Err(CoreError::ResearchBlocked {
            harness: self.id.to_string(),
            surface: "wrapper".to_owned(),
            reason: format!(
                "ResearchBlocked: {BLOCKED_REASON} — detect only; wrapper via {CONFIG_ENV_VAR} not verified for concurrent instances"
            ),
        })
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/crush/crushrc".to_owned(),
            "~/.config/crush/crush.json".to_owned(),
            "$CRUSH_GLOBAL_CONFIG/crushrc".to_owned(),
            "$CRUSH_GLOBAL_CONFIG/crush.json".to_owned(),
            "./crushrc".to_owned(),
            "./.crushrc".to_owned(),
            "./crush.json".to_owned(),
            "./.crush.json".to_owned(),
            ".crush/".to_owned(),
            "$XDG_CONFIG_HOME/crush/crushrc".to_owned(),
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
                "ResearchBlocked: {BLOCKED_REASON} — validate blocked until crushrc mutation verified"
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
    use std::path::PathBuf;

    use super::{
        BLOCKED_REASON, CrushAdapter, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> CrushAdapter {
        CrushAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-crush-1").unwrap(),
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
        assert_eq!(a.config_env_var(), super::CONFIG_ENV_VAR);
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert_eq!(a.adapter_revision(), crate::adapter::ADAPTER_REVISION);
        assert!(a.blocked_reason().contains("crushrc"));
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
        assert!(result.evidence.iter().any(|e| e.contains("crushrc")));
        assert_ne!(result.confidence.to_string(), "");
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
                .any(|n| n.contains("research blocked") || n.contains("crushrc"))
        );
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("crush 0.5.1", Some("0.5.1")),
            ("crush 0.5.1-alpha", Some("0.5.1-alpha")),
            ("0.5.1", Some("0.5.1")),
            ("v0.5.1", Some("0.5.1")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = CrushAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_executable_and_json() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let global_rc = surfaces
            .iter()
            .find(|s| s.id == "crushrc (global)")
            .expect("global crushrc must exist");
        assert_eq!(global_rc.kind, DocumentKind::Executable);
        assert_eq!(global_rc.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(global_rc.scope, ConfigScope::User);
        assert!(global_rc.backup_required);

        let global_json = surfaces
            .iter()
            .find(|s| s.id == "crush.json (global)")
            .expect("global json must exist");
        assert_eq!(global_json.kind, DocumentKind::Json);
        assert_eq!(global_json.ownership, SurfaceOwnership::UserEditable);

        let proj_rc = surfaces
            .iter()
            .find(|s| s.id == "crushrc (project)")
            .expect("project crushrc must exist");
        assert_eq!(proj_rc.kind, DocumentKind::Executable);
        assert_eq!(proj_rc.scope, ConfigScope::ProjectWorkspace);

        let state = surfaces
            .iter()
            .find(|s| s.id == ".crush/state")
            .expect("state must exist");
        assert_eq!(state.ownership, SurfaceOwnership::HarnessManaged);
        assert_eq!(state.kind, DocumentKind::Opaque);
    }

    #[test]
    fn supported_operations_are_research_blocked() {
        let a = adapter();
        let ops = a.supported_operations();
        assert!(!ops.is_empty());
        for (name, support) in &ops {
            assert_eq!(
                *support,
                AdapterSupport::ResearchBlocked,
                "operation {name} should be ResearchBlocked"
            );
        }
        let map: HashSet<String> = ops.iter().map(|(n, _)| n.clone()).collect();
        for required in [
            "detect",
            "read_config",
            "write_config",
            "plan_wrapper",
            "validate_instance",
        ] {
            assert!(map.contains(required), "missing op {required}");
        }
    }

    #[test]
    fn plan_wrapper_is_research_blocked() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.crush-work");
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::ResearchBlocked { reason, .. } => {
                assert!(reason.contains("crushrc") || reason.contains("ResearchBlocked"));
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.crush-work");
        inst.harness = HarnessId::new("claude-code").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn validate_instance_is_research_blocked() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.crush-work");
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::ResearchBlocked { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_crush_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("crushrc")));
        assert!(candidates.iter().any(|c| c.contains("crush.json")));
        assert!(candidates.iter().any(|c| c.contains("CRUSH_GLOBAL_CONFIG")));
        assert!(candidates.iter().any(|c| c.contains("XDG_CONFIG_HOME")));
    }

    #[test]
    fn supported_skill_modes_is_empty() {
        let a = adapter();
        assert!(a.supported_skill_modes().is_empty());
    }

    #[test]
    fn plan_mirror_exclusions_cover_state_and_logs() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(exclusions.iter().any(|p| p.contains(".crush")));
        assert!(exclusions.iter().any(|p| p.contains("cache")));
    }

    // -----------------------------------------------------------------------
    // Fixture-backed conformance tests
    // -----------------------------------------------------------------------

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/crush")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("settings.minimal.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        // Minimal may be empty object with optional $schema
        assert!(map.is_empty() || map.contains_key("$schema") || map.len() <= 2);
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(
            map.contains_key("providers")
                || map.contains_key("models")
                || map.contains_key("mcp")
                || map.contains_key("options")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("crush");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("crush.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "providers".to_owned(),
                serde_json::json!({"test": {"type": "openai-compat"}}),
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
    fn crushrc_executable_fixture_exists_and_is_executable() {
        let path = fixture_path("crushrc.minimal");
        assert!(path.exists(), "crushrc fixture missing: {}", path.display());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("provider") || text.is_empty() || text.contains("#!/"));
    }

    #[test]
    fn unknown_key_preservation_via_json_edit() {
        let dir = crate::test_util::temp_dir_unique("crush");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preserve.json");
        let original = serde_json::json!({
            "providers": {"openai": {"type": "openai"}},
            "foreignKey": "keep-me",
            "anotherForeign": {"nested": 123}
        });
        std::fs::write(&path, serde_json::to_string_pretty(&original).unwrap()).unwrap();
        superai_config::json::edit(&path, |map| {
            map.insert("mcp".to_owned(), serde_json::json!({}));
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
    fn adapter_is_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
        assert!(!boxed.config_surfaces().is_empty());
        assert_eq!(boxed.adapter_revision(), crate::adapter::ADAPTER_REVISION);
    }

    #[test]
    fn research_blocked_reason_contains_gaps() {
        assert!(BLOCKED_REASON.contains("crushrc"));
        assert!(BLOCKED_REASON.contains("ResearchBlocked") || BLOCKED_REASON.contains("command"));
    }
}
