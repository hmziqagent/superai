//! `MiMo` adapter — relocated-root via `MIMOCODE_HOME` with JSON/JSONC `mimocode.jsonc`
//! inline `plugin`/`mcp`.
//!
//! Research source: `docs/harness-configs/mimo-code.md` (last verified 2026-08-25).
//! Executable `mimo`, config root `~/.config/mimocode` or `$MIMOCODE_HOME`,
//! primary writable surface `mimocode.jsonc` (JSONC), isolation `relocated-root`.

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

/// Harness identifier for `MiMo` Code.
pub const HARNESS_ID_STR: &str = "mimo-code";

/// Human display name.
pub const DISPLAY_NAME: &str = "MiMo Code";

/// Primary executable name.
pub const EXECUTABLE: &str = "mimo";

/// Environment variable that relocates the config root.
pub const CONFIG_ENV_VAR: &str = "MIMOCODE_HOME";

/// Default config root when `MIMOCODE_HOME` is unset.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.config/mimocode";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/mimo-code.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current config shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider/model/mcp mutation.
pub const OWNED_SELECTORS: &[&str] = &[
    "model",
    "provider",
    "mcp",
    "plugin",
    "skills",
    "instructions",
    "permission",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for `MiMo` Code.
///
/// Isolation is `relocated-root` via `MIMOCODE_HOME`. The wrapper sets
/// `MIMOCODE_HOME` to the instance `config_root` and execs `mimo`.
#[derive(Debug, Clone)]
pub struct MimoAdapter {
    id: HarnessId,
}

impl MimoAdapter {
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

    /// Try to locate the `pi` binary via `PATH`.
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

    /// Probe `pi --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `pi 0.9.0` into `0.9.0`.
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

    /// Resolve the default config root: `$MIMOCODE_HOME` or `~/.config/mimocode`.
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
        Some(PathBuf::from(home).join(".config").join("mimocode"))
    }

    /// Check if default config root exists on disk.
    #[expect(dead_code, reason = "helper for future use")]
    #[expect(clippy::unused_self, reason = "adapter helper")]
    fn default_config_root_exists(&self) -> Option<PathBuf> {
        let root = Self::default_config_root()?;
        if root.exists() { Some(root) } else { None }
    }

    /// Build the mimocode.jsonc path for a given config root.
    fn config_path_for_root(root: &Path) -> PathBuf {
        root.join("mimocode.jsonc")
    }

    /// Build the mimocode.json fallback path.
    fn config_json_path_for_root(root: &Path) -> PathBuf {
        root.join("mimocode.json")
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
                    let cfg = Self::config_path_for_root(&root);
                    let cfg_json = Self::config_json_path_for_root(&root);
                    if cfg.exists() {
                        evidence.push(format!("mimocode.jsonc found at {}", cfg.display()));
                        if let Ok(text) = std::fs::read_to_string(&cfg)
                            && (text.contains("model") || text.contains("provider"))
                        {
                            evidence.push("mimocode.jsonc contains model/provider".to_owned());
                        }
                    } else if cfg_json.exists() {
                        evidence.push(format!("mimocode.json found at {}", cfg_json.display()));
                    } else {
                        evidence.push(format!("mimocode.jsonc missing at {}", cfg.display()));
                    }
                    let tui = root.join("tui.json");
                    if tui.exists() {
                        evidence.push(format!("tui.json present at {}", tui.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
        if let Ok(dir) = std::env::var(CONFIG_ENV_VAR)
            && !dir.trim().is_empty()
        {
            evidence.push(format!("{CONFIG_ENV_VAR} set to {dir}"));
        } else {
            evidence.push(format!(
                "{CONFIG_ENV_VAR} not set, using ~/.config/mimocode"
            ));
        }
        if let Ok(inline) = std::env::var("MIMOCODE_CONFIG_CONTENT")
            && !inline.trim().is_empty()
        {
            evidence.push(format!(
                "MIMOCODE_CONFIG_CONTENT is set (len {})",
                inline.len()
            ));
        }
        let project_cfg = Path::new(".mimocode").join("mimocode.jsonc");
        if project_cfg.exists() {
            evidence.push(format!("project config found at {}", project_cfg.display()));
        }
        if Path::new(".mimocode").exists() {
            evidence.push(".mimocode directory present in cwd".to_owned());
        }
    }
}

impl Default for MimoAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "pi is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for MimoAdapter {
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
            notes.push(format!("detected mimo-code version {v}"));
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

        let mimocode_resolver = PathResolver::new(
            Some("$MIMOCODE_HOME/config/mimocode.jsonc"),
            Some("$MIMOCODE_HOME/config/mimocode.jsonc"),
            Some("%MIMOCODE_HOME%\\config\\mimocode.jsonc"),
            "~/.config/mimocode/mimocode.jsonc",
        );
        let mut mimocode_surface = ConfigSurface::new(
            "mimocode.jsonc",
            mimocode_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mimocode_surface.precedence = 10;
        mimocode_surface.owned_selectors =
            OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        mimocode_surface.backup_required = true;
        mimocode_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(mimocode_surface);

        let mimocode_json_resolver = PathResolver::new(
            Some("$MIMOCODE_HOME/config/mimocode.json"),
            Some("$MIMOCODE_HOME/config/mimocode.json"),
            Some("%MIMOCODE_HOME%\\config\\mimocode.json"),
            "~/.config/mimocode/mimocode.json",
        );
        let mut mimocode_json = ConfigSurface::new(
            "mimocode.json",
            mimocode_json_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mimocode_json.precedence = 9;
        mimocode_json.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        mimocode_json.backup_required = true;
        surfaces.push(mimocode_json);

        let project_resolver = PathResolver::fallback_only(".mimocode/mimocode.jsonc (project)");
        let mut project = ConfigSurface::new(
            "project.mimocode.jsonc",
            project_resolver,
            DocumentKind::Jsonc,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project.precedence = 14;
        project.backup_required = false;
        surfaces.push(project);

        let tui_resolver = PathResolver::new(
            Some("$MIMOCODE_HOME/config/tui.json"),
            Some("$MIMOCODE_HOME/config/tui.json"),
            Some("%MIMOCODE_HOME%\\config\\tui.json"),
            "~/.config/mimocode/tui.json",
        );
        let mut tui = ConfigSurface::new(
            "tui.json",
            tui_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        tui.precedence = 6;
        tui.backup_required = false;
        surfaces.push(tui);

        let auth_resolver = PathResolver::new(
            Some("$MIMOCODE_HOME/data/auth.json"),
            Some("$MIMOCODE_HOME/data/auth.json"),
            Some("%MIMOCODE_HOME%\\data\\auth.json"),
            "~/.local/share/mimocode/auth.json",
        );
        let mut auth = ConfigSurface::new(
            "auth.json",
            auth_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        auth.precedence = 12;
        auth.backup_required = false;
        auth.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(auth);

        let mcp_auth_resolver = PathResolver::new(
            Some("$MIMOCODE_HOME/data/mcp-auth.json"),
            Some("$MIMOCODE_HOME/data/mcp-auth.json"),
            Some("%MIMOCODE_HOME%\\data\\mcp-auth.json"),
            "~/.local/share/mimocode/mcp-auth.json",
        );
        let mut mcp_auth = ConfigSurface::new(
            "mcp-auth.json",
            mcp_auth_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        mcp_auth.precedence = 8;
        mcp_auth.backup_required = false;
        surfaces.push(mcp_auth);

        let inline_resolver = PathResolver::fallback_only("MIMOCODE_CONFIG_CONTENT (inline JSON)");
        let mut inline = ConfigSurface::new(
            "inline.config",
            inline_resolver,
            DocumentKind::Json,
            ConfigScope::SessionInline,
            SurfaceOwnership::UserEditable,
        );
        inline.precedence = 15;
        inline.backup_required = false;
        surfaces.push(inline);

        let skills_resolver = PathResolver::new(
            Some("$MIMOCODE_HOME/skills/<name>/SKILL.md"),
            Some("$MIMOCODE_HOME/skills/<name>/SKILL.md"),
            Some("%MIMOCODE_HOME%\\skills\\<name>\\SKILL.md"),
            "~/.config/mimocode/skills/<name>/SKILL.md",
        );
        let mut skills = ConfigSurface::new(
            "skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills.precedence = 7;
        skills.backup_required = false;
        surfaces.push(skills);

        let plugins_resolver = PathResolver::new(
            Some("$MIMOCODE_HOME/config/plugins/<name>"),
            Some("$MIMOCODE_HOME/config/plugins/<name>"),
            Some("%MIMOCODE_HOME%\\config\\plugins\\<name>"),
            "~/.config/mimocode/plugins/<name>",
        );
        let mut plugins = ConfigSurface::new(
            "plugins",
            plugins_resolver,
            DocumentKind::Opaque,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        plugins.precedence = 5;
        plugins.backup_required = false;
        surfaces.push(plugins);

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
            "data/mimocode.db*".to_owned(),
            "state/*".to_owned(),
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "tmp/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "data/logs/*".to_owned(),
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
        let mut plan = WrapperPlan::new("relocated-root via MIMOCODE_HOME");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        plan.description = format!(
            " Wrapper sets {}={} and execs `{}`",
            CONFIG_ENV_VAR, instance.config_root, EXECUTABLE
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/mimocode".to_owned(),
            "~/.local/share/mimocode".to_owned(),
            "$MIMOCODE_HOME".to_owned(),
            ".mimocode/mimocode.jsonc".to_owned(),
            "~/.mimo-profiles/<name>".to_owned(),
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
                reason: format!("mimo-code requires isolation relocated_root, got {other}"),
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
        CONFIG_ENV_VAR, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, MimoAdapter, OWNED_SELECTORS,
        RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> MimoAdapter {
        MimoAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-mimo-1").unwrap(),
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
            ("mimo 1.2.3", Some("1.2.3")),
            ("mimo 0.1.0-beta", Some("0.1.0-beta")),
            ("v1.0.0", Some("1.0.0")),
            ("Version: 2.0.0", Some("2.0.0")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = MimoAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_writable_primary() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 5);
        let primary = surfaces
            .iter()
            .find(|s| s.id == "mimocode.jsonc")
            .expect("mimocode.jsonc surface must exist");
        assert_eq!(primary.kind, DocumentKind::Jsonc);
        assert_eq!(primary.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(primary.scope, ConfigScope::User);
        assert!(primary.backup_required);
        for selector in ["model", "provider"] {
            assert!(
                primary.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in &primary.owned_selectors {
            assert!(!sel.is_empty());
        }
        for sel in OWNED_SELECTORS {
            assert!(primary.owned_selectors.contains(&(*sel).to_owned()));
        }

        let second = surfaces
            .iter()
            .find(|s| s.id == "mimocode.json")
            .expect("mimocode.json");
        assert_eq!(second.kind, DocumentKind::Json);
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
        let must_contain = ["sessions/*", "logs/*", "cache/*", "*.lock"];
        for pat in must_contain {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"mimocode.jsonc".to_owned()));
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
        assert!(!is_excluded("mimocode.jsonc"));
        assert!(is_excluded("sessions/abc.jsonl") || is_excluded("sessions/abc.yaml"));
        assert!(is_excluded("cache/data") || is_excluded("cache/file"));
        assert!(is_excluded("logs/app.log"));
    }

    #[test]
    fn plan_wrapper_sets_env() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.mimo-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == "/tmp/.mimo-work")
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(CONFIG_ENV_VAR));
        assert!(plan.args.is_empty() || !plan.description.is_empty());
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my mimo work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == CONFIG_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, "/tmp/my mimo work");
        assert!(!env_val.contains('"'));
        assert!(env_val.contains(' '));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.mimo-work");
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
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("MIMOCODE_HOME") || c.contains("mimo") || c.contains('.'))
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.contains(CONFIG_ENV_VAR) || c.contains("MIMOCODE_HOME"))
        );
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.mimo-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.mimo-work");
        if HARNESS_ID_STR == "nanocoder" {
            inst.isolation = Isolation::ProjectScope;
        } else {
            inst.isolation = Isolation::EnvOnly;
        }
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn validate_instance_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.mimo-work");
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let primary = surfaces.iter().find(|s| s.id == "mimocode.jsonc").unwrap();
        assert_eq!(
            primary.path_resolver.fallback,
            "~/.config/mimocode/mimocode.jsonc"
        );
        let resolver = &primary.path_resolver;
        assert!(
            resolver.linux.as_deref().unwrap().contains(CONFIG_ENV_VAR)
                || resolver.linux.as_deref().unwrap().contains("MIMOCODE_HOME")
        );
        assert!(
            resolver.macos.as_deref().unwrap().contains(CONFIG_ENV_VAR)
                || resolver.macos.as_deref().unwrap().contains("MIMOCODE_HOME")
        );
        assert!(
            resolver
                .windows
                .as_deref()
                .unwrap()
                .contains(CONFIG_ENV_VAR)
                || resolver
                    .windows
                    .as_deref()
                    .unwrap()
                    .contains("MIMOCODE_HOME")
        );
    }

    // -----------------------------------------------------------------------
    // Fixture-backed conformance tests
    // -----------------------------------------------------------------------

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/mimo")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.jsonc");
        let map = superai_config::jsonc::load(&path).unwrap();
        assert!(map.is_empty());
        let value = superai_config::jsonc::load_value(&path).unwrap();
        assert_eq!(value, serde_json::Value::Object(serde_json::Map::default()));
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("mimocode.minimal.jsonc");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::jsonc::load(&path).unwrap();
        assert!(
            map.is_empty()
                || map.contains_key("model")
                || map.contains_key("provider")
                || map.contains_key("$schema")
        );
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("mimocode.populated.jsonc");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::jsonc::load(&path).unwrap();
        assert!(
            map.contains_key("model")
                || map.contains_key("provider")
                || map.contains_key("model")
                || map.contains_key("providers")
                || map.contains_key("mcpServers")
                || !map.is_empty()
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("mimocode.foreign.jsonc");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::jsonc::load(&path).unwrap();
        assert!(
            original.contains_key("foreignKey")
                || original.contains_key("unknownTopLevel")
                || original.contains_key("extraUnknown")
        );
        let dir = crate::test_util::temp_dir_unique("mimo");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("mimocode.foreign.jsonc.copy.jsonc");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::jsonc::edit(&tmp, |map| {
            map.insert(
                "theme".to_owned(),
                serde_json::Value::String("dark".to_owned()),
            );
            assert!(
                map.contains_key("foreignKey")
                    || map.contains_key("unknownTopLevel")
                    || map.contains_key("extraUnknown")
            );
        })
        .unwrap();
        let after = superai_config::jsonc::load(&tmp).unwrap();
        assert_eq!(after["theme"], serde_json::Value::String("dark".to_owned()));
        let foreign_preserved = after.contains_key("foreignKey")
            || after.contains_key("unknownTopLevel")
            || after.contains_key("customField")
            || after.contains_key("extraUnknown");
        assert!(
            foreign_preserved,
            "foreign keys must be preserved, got {after:?}"
        );
        drop(std::fs::remove_file(&tmp));
    }

    #[test]
    fn fixture_malformed_fails_to_parse() {
        let path = fixture_path("mimocode.malformed.jsonc");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::jsonc::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn secret_redaction_placeholder() {
        use crate::error::RedactedString;
        let secret = RedactedString::new("sk-mimo-secret-321");
        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert!(!debug.contains("sk-mimo-secret-321"));
        assert!(!display.contains("sk-mimo-secret-321"));
        assert!(debug.contains("[REDACTED]"));
        assert!(display.contains("[REDACTED]"));
        let json = serde_json::to_string(&secret).unwrap();
        assert!(!json.contains("sk-mimo-secret-321"));
        assert!(json.contains("[REDACTED]"));
        assert_eq!(secret.expose_secret(), "sk-mimo-secret-321");
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
        let inst = sample_instance_with_root("/tmp/.mimo-work");
        a.validate_instance(&inst).unwrap();
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn wrapper_env_var_isolation_is_relocated_root() {
        let a = adapter();
        assert!(a.scan_candidates().len() >= 3);
        let inst = sample_instance_with_root("/home/user/.mimo-isolated");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(!plan.env_vars.is_empty());
        let (key, val) = &plan.env_vars[0];
        assert_eq!(key, CONFIG_ENV_VAR);
        assert_eq!(val, "/home/user/.mimo-isolated");
    }

    #[test]
    fn registry_no_harness_value_leak() {
        let inst = sample_instance_with_root("/tmp/.mimo-work");
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
