//! `DeepSeek` Harness adapter — `dsh`, `DSH_HOME`, relocated-root, `ResearchBlocked` dev preview.
//!
//! Research source: `docs/harness-configs/deepseek-harness.md` (last verified 2026-08-25).
//! Executable `dsh` (`npx @deepseek-ai/dsh`), config root `~/.dsh` or `$DSH_HOME`,
//! provider catalog via `@earendil-works/pi-ai` (`providers` keyed by route, `apiKeyEnv`
//! as env-var name, `compat` wire-quirk catalog), plugin config incomplete,
//! isolation `relocated-root` via `DSH_HOME`, product status `preview` (developer
//! preview 2026-08-13, compatibility-breaking changes warned), support `ResearchBlocked`.

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

/// Harness identifier for `DeepSeek` Harness.
pub const HARNESS_ID_STR: &str = "deepseek-harness";

/// Human display name.
pub const DISPLAY_NAME: &str = "DeepSeek Harness";

/// Primary executable name (`dsh` CLI).
pub const EXECUTABLE: &str = "dsh";

/// Alternative executable via npm alias.
pub const EXECUTABLE_ALT: &str = "deepseek";

/// Environment variable that relocates the harness home.
pub const CONFIG_ENV_VAR: &str = "DSH_HOME";

/// Default config root fallback.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.dsh";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/deepseek-harness.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version (provider catalog surface).
pub const SCHEMA_VERSION_STR: &str = "1";

/// Research-blocked reason — provider catalog + plugin incomplete, dev preview.
pub const BLOCKED_REASON: &str = "DeepSeek Harness developer preview (2026-08-13): provider catalog pi-ai compat switches, plugin contracts, profile boot, AGENTS.md/skills, sandbox runner unverified — relocated-root via DSH_HOME verified but writes blocked";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for `DeepSeek` Harness (`ResearchBlocked`).
///
/// Isolation is `relocated-root` via `DSH_HOME`. Detection via `dsh --version`
/// and harness home existence; provider `compat` and plugin system are not
/// yet stable for mutation. The wrapper would set `DSH_HOME` to the instance
/// `config_root` and exec `dsh`, but is blocked until research gaps close.
#[derive(Debug, Clone)]
pub struct DeepSeekAdapter {
    id: HarnessId,
}

impl DeepSeekAdapter {
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

    /// Try to locate the `dsh` binary via `PATH`.
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

    /// Probe `dsh --version` with a timeout, returning the parsed version string if successful.
    fn probe_version(binary: &Path) -> Option<String> {
        let binary_owned = binary.to_path_buf();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            // dsh may also support `dsh --version` or `dsh cli --version`; try plain --version
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

    /// Parse version output like `0.1.1-rc.2` or `dsh 0.1.1-rc.2` into `0.1.1-rc.2`.
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

    /// Resolve the default harness home: `$DSH_HOME` or `~/.dsh`.
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
        Some(PathBuf::from(home).join(".dsh"))
    }

    /// Check if default harness home exists on disk.
    #[expect(dead_code, reason = "helper for future use")]
    #[expect(clippy::unused_self, reason = "adapter helper")]
    fn default_config_root_exists(&self) -> Option<PathBuf> {
        let root = Self::default_config_root()?;
        if root.exists() { Some(root) } else { None }
    }

    /// Build detection evidence about harness home and provider config.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("research blocked: {BLOCKED_REASON}"));
        evidence.push(
            "gaps: full plugin system/contracts, AGENTS.md handling, profile boot, sandbox runner (runnerCommand/bwrap), full env-var list, config-catalog 3352 lines beyond provider/compat"
                .to_owned(),
        );
        evidence.push(format!(
            "isolation relocated-root via {CONFIG_ENV_VAR} (harness home), default ~/.dsh, exposed to child processes"
        ));
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("harness home exists at {}", root.display()));
                    let agents_md = root.join("AGENTS.md");
                    if agents_md.exists() {
                        evidence.push(format!("AGENTS.md found at {}", agents_md.display()));
                    } else {
                        evidence.push(format!("AGENTS.md missing at {}", agents_md.display()));
                    }
                    // Provider catalog config — may be plugin-scoped, unverified exact filename
                    let config_candidates =
                        ["config.json", "config.yaml", "dsh.json", "settings.json"];
                    let mut found_config = false;
                    for name in config_candidates {
                        let p = root.join(name);
                        if p.exists() {
                            evidence.push(format!("config candidate found at {}", p.display()));
                            found_config = true;
                            if let Ok(text) = std::fs::read_to_string(&p)
                                && (text.contains("providers") || text.contains("apiKeyEnv"))
                            {
                                evidence.push("config contains providers/apiKeyEnv".to_owned());
                            }
                        }
                    }
                    if !found_config {
                        evidence.push("no known provider config filename found in harness home (plugin incomplete)".to_owned());
                    }
                    let plugins = root.join("plugins");
                    if plugins.exists() {
                        evidence.push(format!("plugins dir present at {}", plugins.display()));
                    }
                } else {
                    evidence.push(format!("harness home missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve harness home (no HOME)".to_owned());
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
            evidence.push(format!("{CONFIG_ENV_VAR} not set, using ~/.dsh"));
        }
        // Project AGENTS.md
        if Path::new("AGENTS.md").exists() {
            evidence.push("project AGENTS.md found in cwd".to_owned());
        }
    }
}

impl Default for DeepSeekAdapter {
    fn default() -> Self {
        #[expect(
            clippy::unwrap_used,
            reason = "deepseek-harness is static valid HarnessId"
        )]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for DeepSeekAdapter {
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
            if evidence.iter().any(|e| e.contains("harness home exists")) {
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
            notes.push(format!("detected deepseek-harness version {v}"));
            notes.push(format!("research blocked — {BLOCKED_REASON}"));
            notes.push("dev preview 0.1.1-rc.2, compatibility-breaking changes warned".to_owned());
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

        // Harness home config — providers catalog (JSON), user scope
        let home_resolver = PathResolver::new(
            Some("$DSH_HOME/config.json"),
            Some("$DSH_HOME/config.json"),
            Some("%DSH_HOME%\\config.json"),
            "~/.dsh/config.json",
        );
        let mut home_config = ConfigSurface::new(
            "config.json",
            home_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        home_config.precedence = 10;
        home_config.owned_selectors = vec![
            "providers".to_owned(),
            "providers.*.apiKeyEnv".to_owned(),
            "providers.*.baseURL".to_owned(),
            "providers.*.api".to_owned(),
            "providers.*.models".to_owned(),
            "providers.*.compat".to_owned(),
        ];
        home_config.backup_required = true;
        home_config.restart_behavior = RestartBehavior::Reload;
        surfaces.push(home_config);

        // Global AGENTS.md — text fragment, user scope
        let agents_resolver = PathResolver::new(
            Some("$DSH_HOME/AGENTS.md"),
            Some("$DSH_HOME/AGENTS.md"),
            Some("%DSH_HOME%\\AGENTS.md"),
            "~/.dsh/AGENTS.md",
        );
        let mut agents_global = ConfigSurface::new(
            "AGENTS.md (global)",
            agents_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        agents_global.precedence = 12;
        agents_global.backup_required = false;
        surfaces.push(agents_global);

        // Project AGENTS.md — project/workspace scope
        let project_agents = PathResolver::fallback_only("AGENTS.md (project, cwd)");
        let mut project_agents_surface = ConfigSurface::new(
            "AGENTS.md (project)",
            project_agents,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_agents_surface.precedence = 15;
        project_agents_surface.backup_required = false;
        surfaces.push(project_agents_surface);

        // Plugins — directory bundle, user scope (incomplete)
        let plugins_resolver = PathResolver::new(
            Some("$DSH_HOME/plugins/<name>/"),
            Some("$DSH_HOME/plugins/<name>/"),
            Some("%DSH_HOME%\\plugins\\<name>\\"),
            "~/.dsh/plugins/<name>/",
        );
        let mut plugins = ConfigSurface::new(
            "plugins",
            plugins_resolver,
            DocumentKind::Opaque,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        plugins.precedence = 5;
        plugins.backup_required = false;
        surfaces.push(plugins);

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
            "plugins/*".to_owned(),
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "history/*".to_owned(),
            "*.db".to_owned(),
            "*.log".to_owned(),
            "tmp/*".to_owned(),
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
                "ResearchBlocked: {BLOCKED_REASON} — wrapper via {CONFIG_ENV_VAR} not yet verified for concurrent plugin/profile isolation"
            ),
        })
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.dsh".to_owned(),
            "~/.dsh/config.json".to_owned(),
            "~/.dsh/AGENTS.md".to_owned(),
            "$DSH_HOME".to_owned(),
            "$DSH_HOME/config.json".to_owned(),
            "AGENTS.md (project)".to_owned(),
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
                "ResearchBlocked: {BLOCKED_REASON} — validate blocked until provider/plugin schema stabilized"
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
        BLOCKED_REASON, DISPLAY_NAME, DeepSeekAdapter, EXECUTABLE, HARNESS_ID_STR, RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> DeepSeekAdapter {
        DeepSeekAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-deepseek-1").unwrap(),
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
        assert_eq!(a.config_env_var(), super::CONFIG_ENV_VAR);
        assert_eq!(a.product_status(), ProductStatus::Preview);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert_eq!(a.adapter_revision(), crate::adapter::ADAPTER_REVISION);
        assert!(a.blocked_reason().contains("DSH_HOME") || a.blocked_reason().contains("preview"));
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
        assert!(result.evidence.iter().any(|e| e.contains("DSH_HOME")));
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
                .any(|n| n.contains("research blocked") || n.contains("preview"))
        );
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("dsh 0.1.1-rc.2", Some("0.1.1-rc.2")),
            ("0.1.1-rc.2", Some("0.1.1-rc.2")),
            ("v0.1.1", Some("0.1.1")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = DeepSeekAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_relocated_root() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 3);
        let config = surfaces
            .iter()
            .find(|s| s.id == "config.json")
            .expect("config.json must exist");
        assert_eq!(config.kind, DocumentKind::Json);
        assert_eq!(config.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(config.scope, ConfigScope::User);
        assert!(config.backup_required);
        assert!(
            config
                .owned_selectors
                .iter()
                .any(|s| s.contains("providers"))
        );

        let agents_global = surfaces
            .iter()
            .find(|s| s.id == "AGENTS.md (global)")
            .expect("global AGENTS.md must exist");
        assert_eq!(agents_global.kind, DocumentKind::TextFragment);
        assert_eq!(agents_global.scope, ConfigScope::User);

        let plugins = surfaces
            .iter()
            .find(|s| s.id == "plugins")
            .expect("plugins");
        assert_eq!(plugins.kind, DocumentKind::Opaque);
        assert_eq!(plugins.ownership, SurfaceOwnership::HarnessManaged);
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
    }

    #[test]
    fn plan_wrapper_is_research_blocked() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.dsh-work");
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::ResearchBlocked { reason, .. } => {
                assert!(reason.contains("DSH_HOME") || reason.contains("ResearchBlocked"));
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn validate_instance_is_research_blocked() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.dsh-work");
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::ResearchBlocked { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_dsh_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains(".dsh")));
        assert!(candidates.iter().any(|c| c.contains("DSH_HOME")));
        assert!(candidates.iter().any(|c| c.contains("AGENTS.md")));
    }

    #[test]
    fn supported_skill_modes_is_empty() {
        let a = adapter();
        assert!(a.supported_skill_modes().is_empty());
    }

    #[test]
    fn plan_mirror_exclusions_cover_plugins_and_cache() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(exclusions.iter().any(|p| p.contains("plugins")));
        assert!(exclusions.iter().any(|p| p.contains("cache")));
    }

    // -----------------------------------------------------------------------
    // Fixture-backed conformance tests
    // -----------------------------------------------------------------------

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/deepseek_harness")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("settings.minimal.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        // Minimal may be empty or contain providers routing stub
        assert!(map.is_empty() || map.contains_key("providers") || map.len() <= 2);
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(
            map.contains_key("providers")
                || map.contains_key("compat")
                || map.contains_key("models")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("deepseek");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("deepseek.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::json::edit(&tmp, |map| {
            map.insert(
                "providers".to_owned(),
                serde_json::json!({"openai": {"apiKeyEnv": "OPENAI_API_KEY"}}),
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
        let dir = crate::test_util::temp_dir_unique("deepseek");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preserve.json");
        let original = serde_json::json!({
            "providers": {"anthropic": {"apiKeyEnv": "ANTHROPIC_API_KEY"}},
            "foreignKey": "keep-me",
            "anotherForeign": {"nested": 123}
        });
        std::fs::write(&path, serde_json::to_string_pretty(&original).unwrap()).unwrap();
        superai_config::json::edit(&path, |map| {
            map.insert(
                "compat".to_owned(),
                serde_json::json!({"supportsStore": true}),
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
    fn research_blocked_reason_contains_preview() {
        assert!(BLOCKED_REASON.contains("preview") || BLOCKED_REASON.contains("DSH_HOME"));
    }
}
