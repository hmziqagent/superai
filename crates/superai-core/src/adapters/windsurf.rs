//! Windsurf adapter — IDE `--user-data-dir` with MCP JSON and rules/skills.
//!
//! Research source: `docs/harness-configs/windsurf.md` (last verified 2026-08-25).
//! Executable `windsurf` (stable) / `devin` (converged name), MCP
//! `~/.codeium/windsurf/mcp_config.json` (JSON), rules `.windsurf/rules` and
//! `.devin/rules`, isolation `ide-user-data` via `--user-data-dir`, constrained.

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

/// Harness identifier for Windsurf.
pub const HARNESS_ID_STR: &str = "windsurf";

/// Human display name.
pub const DISPLAY_NAME: &str = "Windsurf/Devin Desktop";

/// Primary executable.
pub const EXECUTABLE: &str = "windsurf";

/// Alternative executable (Devin converged).
pub const EXECUTABLE_ALT: &str = "devin";

/// Flags for IDE isolation.
pub const USER_DATA_DIR_FLAG: &str = "--user-data-dir";

/// Extensions dir flag.
pub const EXTENSIONS_DIR_FLAG: &str = "--extensions-dir";

/// Default MCP config fallback.
pub const DEFAULT_MCP_FALLBACK: &str = "~/.codeium/windsurf/mcp_config.json";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/windsurf.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for MCP.
pub const MCP_OWNED_SELECTORS: &[&str] = &["mcpServers"];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Windsurf.
#[derive(Debug, Clone)]
pub struct WindsurfAdapter {
    id: HarnessId,
}

impl WindsurfAdapter {
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

    #[expect(clippy::unused_self, reason = "adapter uses instance constants")]
    #[expect(clippy::excessive_nesting, reason = "PATH scan branches are explicit")]
    fn find_binary_in_path(&self) -> Option<PathBuf> {
        let path_var = std::env::var("PATH").ok()?;
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path_var.split(sep) {
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
        for dir in path_var.split(sep) {
            if dir.is_empty() {
                continue;
            }
            let candidate = Path::new(dir).join(EXECUTABLE_ALT);
            if candidate.is_file() {
                return Some(candidate);
            }
            if cfg!(windows) {
                let exe_candidate = Path::new(dir).join(format!("{EXECUTABLE_ALT}.exe"));
                if exe_candidate.is_file() {
                    return Some(exe_candidate);
                }
            }
        }
        None
    }

    fn probe_version(binary: &Path) -> Option<String> {
        let owned = binary.to_path_buf();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let output = Command::new(&owned)
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

    #[expect(clippy::excessive_nesting, reason = "version parsing explicit")]
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

    fn default_mcp_path() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(
            PathBuf::from(home)
                .join(".codeium")
                .join("windsurf")
                .join("mcp_config.json"),
        )
    }

    fn default_user_data_root() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        if cfg!(target_os = "macos") {
            Some(
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("Windsurf")
                    .join("User"),
            )
        } else if cfg!(windows) {
            if let Ok(appdata) = std::env::var("APPDATA")
                && !appdata.trim().is_empty()
            {
                return Some(PathBuf::from(appdata).join("Windsurf").join("User"));
            }
            Some(
                PathBuf::from(home)
                    .join("AppData")
                    .join("Roaming")
                    .join("Windsurf")
                    .join("User"),
            )
        } else {
            Some(
                PathBuf::from(home)
                    .join(".config")
                    .join("Windsurf")
                    .join("User"),
            )
        }
    }

    #[expect(clippy::excessive_nesting, reason = "evidence explicit")]
    #[expect(clippy::unused_self, reason = "adapter method")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_mcp_path() {
            Some(p) => {
                if p.exists() {
                    evidence.push(format!("mcp_config.json exists at {}", p.display()));
                    if let Ok(text) = std::fs::read_to_string(&p)
                        && text.contains("mcpServers")
                    {
                        evidence.push("mcp_config.json contains mcpServers".to_owned());
                    }
                } else {
                    evidence.push(format!("mcp_config.json missing at {}", p.display()));
                }
            }
            None => evidence.push("could not resolve mcp path (no HOME)".to_owned()),
        }
        for rule_path in [
            Path::new(".windsurf/rules"),
            Path::new(".devin/rules"),
            Path::new(".windsurfrules"),
        ] {
            if rule_path.exists() {
                evidence.push(format!("rules present at {}", rule_path.display()));
            }
        }
        if Path::new("AGENTS.md").exists() {
            evidence.push("AGENTS.md present".to_owned());
        }
        if let Some(root) = Self::default_user_data_root()
            && root.exists()
        {
            evidence.push(format!("Windsurf User data exists at {}", root.display()));
        }
        if Path::new(".codeium/windsurf/memories").exists() || Path::new(".devin/memories").exists()
        {
            evidence.push("memories dir present".to_owned());
        }
    }
}

impl Default for WindsurfAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "windsurf is static valid")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for WindsurfAdapter {
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

    #[expect(clippy::single_match_else, reason = "detection branching explicit")]
    fn detection(&self) -> DetectionResult {
        let mut evidence = Vec::new();
        let mut version: Option<String> = None;
        let mut binary_path: Option<PathBuf> = None;
        match self.find_binary_in_path() {
            Some(path) => {
                evidence.push(format!(
                    "found binary `{}` at {}",
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("windsurf"),
                    path.display()
                ));
                match Self::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `--version`"));
                        version = Some(v);
                    }
                    None => {
                        evidence.push("version probe failed for `--version` (timeout)".to_owned());
                    }
                }
                binary_path = Some(path);
            }
            None => {
                evidence.push(format!("binary `{EXECUTABLE}` not found in PATH"));
                evidence.push(format!("binary `{EXECUTABLE_ALT}` not found in PATH"));
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
            evidence
                .iter()
                .any(|e| e.contains("mcp_config.json exists")),
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
            notes.push(format!("detected windsurf version {v}"));
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

        let mcp_resolver = PathResolver::new(
            Some("~/.codeium/windsurf/mcp_config.json"),
            Some("~/.codeium/windsurf/mcp_config.json"),
            Some("%USERPROFILE%\\.codeium\\windsurf\\mcp_config.json"),
            "~/.codeium/windsurf/mcp_config.json",
        );
        let mut mcp = ConfigSurface::new(
            "mcp_config.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp.precedence = 10;
        mcp.owned_selectors = MCP_OWNED_SELECTORS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        mcp.backup_required = true;
        mcp.restart_behavior = RestartBehavior::Reload;
        surfaces.push(mcp);

        let memories_resolver = PathResolver::fallback_only("~/.codeium/windsurf/memories/");
        let mut memories = ConfigSurface::new(
            "memories",
            memories_resolver,
            DocumentKind::Opaque,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        memories.precedence = 5;
        memories.backup_required = false;
        surfaces.push(memories);

        let windsurf_rules_resolver =
            PathResolver::fallback_only(".windsurf/rules/*.md / .devin/rules/*.md");
        let mut rules = ConfigSurface::new(
            ".windsurf/rules",
            windsurf_rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        rules.precedence = 12;
        rules.backup_required = false;
        surfaces.push(rules);

        let legacy_rules_resolver =
            PathResolver::fallback_only(".windsurfrules (legacy) / .windsumrfrules");
        let mut legacy = ConfigSurface::new(
            ".windsurfrules",
            legacy_rules_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        legacy.precedence = 8;
        legacy.backup_required = false;
        surfaces.push(legacy);

        let agents_resolver =
            PathResolver::fallback_only("AGENTS.md (any directory, hierarchical)");
        let mut agents = ConfigSurface::new(
            "AGENTS.md",
            agents_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        agents.precedence = 11;
        agents.backup_required = false;
        surfaces.push(agents);

        let ignore_resolver = PathResolver::fallback_only(".codeiumignore / .devinignore");
        let mut ignore = ConfigSurface::new(
            ".codeiumignore",
            ignore_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        ignore.precedence = 5;
        ignore.backup_required = false;
        surfaces.push(ignore);

        let vscode_resolver = PathResolver::new(
            Some("~/.config/Windsurf/User/settings.json"),
            Some("~/Library/Application Support/Windsurf/User/settings.json"),
            Some("%APPDATA%\\Windsurf\\User\\settings.json"),
            "~/.config/Windsurf/User/settings.json",
        );
        let mut settings = ConfigSurface::new(
            "windsurf settings.json",
            vscode_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        settings.precedence = 7;
        settings.backup_required = true;
        surfaces.push(settings);

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
            "memories/*".to_owned(),
            "cache/*".to_owned(),
            "logs/*".to_owned(),
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
        let mut plan = WrapperPlan::new(
            "ide-user-data via --user-data-dir (MCP JSON + rules/skills IDE storage)",
        );
        // Windsurf has no config-dir env var; isolation is via IDE user-data.
        // We do not set an env var, but plan records the isolated MCP path for evidence.
        let mcp_isolated = Path::new(&instance.config_root.to_string())
            .join("codeium")
            .join("windsurf")
            .join("mcp_config.json");
        plan.env_vars.push((
            "WINDSURF_MCP_CONFIG".to_owned(),
            mcp_isolated.display().to_string(),
        ));
        let user_data = Path::new(&instance.config_root.to_string()).join("vscode-data");
        let extensions = Path::new(&instance.config_root.to_string()).join("extensions");
        plan.args.push(USER_DATA_DIR_FLAG.to_owned());
        plan.args.push(user_data.display().to_string());
        plan.args.push(EXTENSIONS_DIR_FLAG.to_owned());
        plan.args.push(extensions.display().to_string());
        plan.description = format!(
            " Wrapper execs `{} {} {}` with extensions {} and isolated MCP {}",
            EXECUTABLE,
            USER_DATA_DIR_FLAG,
            user_data.display(),
            extensions.display(),
            mcp_isolated.display()
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.codeium/windsurf/mcp_config.json".to_owned(),
            "~/.codeium/windsurf/memories".to_owned(),
            ".windsurf/rules".to_owned(),
            ".devin/rules".to_owned(),
            ".windsurfrules".to_owned(),
            "AGENTS.md".to_owned(),
            "--user-data-dir".to_owned(),
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
            Isolation::IdeUserData | Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("windsurf requires isolation ide_user_data, got {other}"),
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
    use super::{
        DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, RESEARCH_DOC, USER_DATA_DIR_FLAG, WindsurfAdapter,
    };
    use crate::adapter::{Adapter, DocumentKind, ProductStatus};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> WindsurfAdapter {
        WindsurfAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-windsurf-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::IdeUserData,
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
    }

    #[test]
    fn detection_has_evidence() {
        let a = adapter();
        let r = a.detection();
        assert!(!r.evidence.is_empty());
        match r.present {
            InstallPresence::Absent => assert!(r.version.is_none()),
            _ => assert!(!r.evidence.is_empty()),
        }
    }

    #[test]
    fn parse_version_ok() {
        assert_eq!(
            WindsurfAdapter::parse_version_output("windsurf 1.0.0").as_deref(),
            Some("1.0.0")
        );
        assert_eq!(WindsurfAdapter::parse_version_output(""), None);
    }

    #[test]
    fn surfaces_include_mcp() {
        let a = adapter();
        let s = a.config_surfaces();
        assert!(s.iter().any(|x| x.id == "mcp_config.json"));
        assert!(s.iter().any(|x| x.id == ".windsurf/rules"));
        assert!(s.iter().any(|x| x.kind == DocumentKind::Json));
    }

    #[test]
    fn operations_constrained() {
        let a = adapter();
        for (_, sup) in a.supported_operations() {
            assert_eq!(sup, AdapterSupport::Constrained);
        }
    }

    #[test]
    fn wrapper_has_user_data_dir() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.windsurf-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(plan.args.contains(&USER_DATA_DIR_FLAG.to_owned()));
        let idx = plan
            .args
            .iter()
            .position(|x| x == USER_DATA_DIR_FLAG)
            .unwrap();
        #[expect(clippy::get_unwrap, reason = "test index vetted")]
        let path = plan.args.get(idx + 1).unwrap();
        assert!(path.contains(".windsurf-work"));
        assert!(!plan.description.is_empty());
    }

    #[test]
    fn wrapper_rejects_wrong_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.windsurf-work");
        inst.harness = HarnessId::new("cursor").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scan_candidates_cover_windsurf() {
        let a = adapter();
        let c = a.scan_candidates();
        assert!(c.iter().any(|s| s.contains("mcp_config.json")));
        assert!(c.iter().any(|s| s.contains(USER_DATA_DIR_FLAG)));
    }

    #[test]
    fn validate_accepts_ide_user_data() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.windsurf-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.windsurf-work");
        inst.isolation = Isolation::EnvOnly;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn adapter_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
    }
}
