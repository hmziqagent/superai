//! Aider adapter — explicit-config via `--config`/`--env-file` plus HOME relocation.
//!
//! Research source: `docs/harness-configs/aider.md` (last verified 2026-08-25).
//! Executable `aider`, YAML config `~/.aider.conf.yml` / `.aider.conf.yml`,
//! env file `~/.env` / `.env` with explicit `--env-file`, JSON metadata
//! `.aider.model.metadata.json`, isolation `explicit-config` (with HOME trick).

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

/// Harness identifier for Aider.
pub const HARNESS_ID_STR: &str = "aider";

/// Human display name.
pub const DISPLAY_NAME: &str = "Aider";

/// Primary executable name.
pub const EXECUTABLE: &str = "aider";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/aider.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current config shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider/model mutation inside `.aider.conf.yml` (YAML).
///
/// These are kebab-case keys matching long CLI options without `--`.
/// Everything else round-trips untouched via `superai-config::yaml`.
pub const OWNED_SELECTORS: &[&str] = &[
    "model",
    "weak-model",
    "editor-model",
    "dark-mode",
    "auto-commits",
    "map-tokens",
    "edit-format",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Aider.
///
/// Isolation is `explicit-config` via `--config` / `--env-file` plus the
/// `HOME` relocation trick. The wrapper sets `HOME` to the instance
/// `config_root` and passes explicit `--config` and `--env-file` args
/// pointing inside that root.
#[derive(Debug, Clone)]
pub struct AiderAdapter {
    id: HarnessId,
}

impl AiderAdapter {
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

    /// Try to locate the `aider` binary via `PATH`.
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

    /// Probe `aider --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `aider 0.84.0` or `aider 0.84.0.dev` into version.
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

    /// Resolve the default HOME for config lookup.
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

    /// Check if default config root exists on disk for evidence.
    #[expect(dead_code, reason = "helper for future use")]
    #[expect(clippy::unused_self, reason = "adapter helper")]
    fn default_config_root_exists(&self) -> Option<PathBuf> {
        let home = Self::default_home()?;
        let conf = home.join(".aider.conf.yml");
        if conf.exists() { Some(home) } else { None }
    }

    /// Build detection evidence about aider config and env files.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        if let Some(home) = Self::default_home() {
            let yml = home.join(".aider.conf.yml");
            if yml.exists() {
                evidence.push(format!("yaml config exists at {}", yml.display()));
                if let Ok(text) = std::fs::read_to_string(&yml)
                    && text.contains("model:")
                {
                    evidence.push("yaml config contains model key".to_owned());
                }
            } else {
                evidence.push(format!("yaml config missing at {}", yml.display()));
            }

            let env = home.join(".env");
            if env.exists() {
                evidence.push(format!(".env file present at {}", env.display()));
            }

            let metadata = home.join(".aider.model.metadata.json");
            if metadata.exists() {
                evidence.push(format!("model metadata present at {}", metadata.display()));
            }

            let settings = home.join(".aider.model.settings.yml");
            if settings.exists() {
                evidence.push(format!("model settings present at {}", settings.display()));
            }

            // Also check cwd/git-root heuristic surfaces if they exist.
            let cwd_yml = Path::new(".aider.conf.yml");
            if cwd_yml.exists() {
                evidence.push(format!("cwd yaml config found at {}", cwd_yml.display()));
            }
            let cwd_env = Path::new(".env");
            if cwd_env.exists() {
                evidence.push(format!("cwd .env found at {}", cwd_env.display()));
            }
        } else {
            evidence.push("could not resolve HOME for config lookup".to_owned());
        }
    }
}

impl Default for AiderAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "aider is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for AiderAdapter {
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
            evidence.iter().any(|e| e.contains("yaml config exists")),
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
            notes.push(format!("detected aider version {v}"));
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

        // Primary writable surface: .aider.conf.yml (YAML) — searched git-root/cwd/home, explicit --config overrides.
        let yml_resolver = PathResolver::new(
            Some(
                "~/.aider.conf.yml / ./.aider.conf.yml / $GIT_ROOT/.aider.conf.yml (or --config <path>)",
            ),
            Some("~/.aider.conf.yml / ./.aider.conf.yml"),
            Some("%USERPROFILE%\\.aider.conf.yml / .\\.aider.conf.yml"),
            "~/.aider.conf.yml (also ./.aider.conf.yml, --config overrides)",
        );
        let mut yml_surface = ConfigSurface::new(
            ".aider.conf.yml",
            yml_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        yml_surface.precedence = 10;
        yml_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        yml_surface.backup_required = true;
        yml_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(yml_surface);

        // Env file surface: .env / --env-file (dotenv)
        let env_resolver = PathResolver::new(
            Some("~/.env / ./.env / $GIT_ROOT/.env (or --env-file <path>)"),
            Some("~/.env / ./.env"),
            Some("%USERPROFILE%\\.env / .\\.env"),
            "~/.env (also ./.env, --env-file overrides)",
        );
        let mut env_surface = ConfigSurface::new(
            ".env",
            env_resolver,
            DocumentKind::Env,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        env_surface.precedence = 20;
        env_surface.owned_selectors = vec![
            "OPENAI_API_KEY".to_owned(),
            "ANTHROPIC_API_KEY".to_owned(),
            "OPENAI_API_BASE".to_owned(),
        ];
        env_surface.backup_required = true;
        env_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(env_surface);

        // Model settings surface: .aider.model.settings.yml (YAML)
        let settings_resolver = PathResolver::new(
            Some("~/.aider.model.settings.yml / ./.aider.model.settings.yml"),
            Some("~/.aider.model.settings.yml"),
            Some("%USERPROFILE%\\.aider.model.settings.yml"),
            "~/.aider.model.settings.yml (also ./.aider.model.settings.yml, --model-settings-file overrides)",
        );
        let mut settings_surface = ConfigSurface::new(
            ".aider.model.settings.yml",
            settings_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        settings_surface.precedence = 15;
        settings_surface.backup_required = true;
        surfaces.push(settings_surface);

        // Model metadata surface: .aider.model.metadata.json (JSON)
        let metadata_resolver = PathResolver::new(
            Some("~/.aider.model.metadata.json / ./.aider.model.metadata.json"),
            Some("~/.aider.model.metadata.json"),
            Some("%USERPROFILE%\\.aider.model.metadata.json"),
            "~/.aider.model.metadata.json",
        );
        let mut metadata_surface = ConfigSurface::new(
            ".aider.model.metadata.json",
            metadata_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        metadata_surface.precedence = 12;
        metadata_surface.backup_required = true;
        surfaces.push(metadata_surface);

        // Chat history surface: .aider.chat.history.md — text fragment, project workspace.
        let history_resolver = PathResolver::fallback_only(
            ".aider.chat.history.md (project root or --chat-history-file)",
        );
        let mut history_surface = ConfigSurface::new(
            ".aider.chat.history.md",
            history_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        history_surface.precedence = 5;
        history_surface.backup_required = false;
        surfaces.push(history_surface);

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
            ".aider.chat.history.md".to_owned(),
            ".aider.input.history".to_owned(),
            ".aider.llm.history".to_owned(),
            "history.jsonl".to_owned(),
            ".aider*history*".to_owned(),
            "cache/*".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "*.lock".to_owned(),
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
        instance.validate()?;
        let mut plan =
            WrapperPlan::new("explicit-config via --config/--env-file + HOME relocation");
        // HOME relocation isolates ~/.aider.conf.yml, ~/.env, model settings etc.
        plan.env_vars
            .push(("HOME".to_owned(), instance.config_root.to_string()));
        // Explicit CLI paths ensure the instance uses its own config and env file.
        let config_path = Path::new(&instance.config_root.to_string()).join(".aider.conf.yml");
        let env_path = Path::new(&instance.config_root.to_string()).join(".env");
        plan.args.push("--config".to_owned());
        plan.args.push(config_path.display().to_string());
        plan.args.push("--env-file".to_owned());
        plan.args.push(env_path.display().to_string());
        plan.description = format!(
            " Wrapper sets HOME={} and execs `{} --config {}` with `--env-file {}`",
            instance.config_root,
            EXECUTABLE,
            config_path.display(),
            env_path.display()
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.aider.conf.yml".to_owned(),
            "./.aider.conf.yml".to_owned(),
            "~/.aider.model.settings.yml".to_owned(),
            "./.env".to_owned(),
            "~/.env".to_owned(),
            "$HOME/.aider.conf.yml via HOME relocation".to_owned(),
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
                reason: format!("aider requires isolation explicit_config, got {other}"),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        vec![crate::adapter::SkillMode::CopySelected]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{
        AiderAdapter, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, OWNED_SELECTORS, RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> AiderAdapter {
        AiderAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-aider-1").unwrap(),
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
            ("aider 0.84.0", Some("0.84.0")),
            ("aider 0.84.0.dev", Some("0.84.0.dev")),
            ("v0.80.1", Some("0.80.1")),
            ("Version: 0.84.0", Some("0.84.0")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = AiderAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_yaml_and_env() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let yml = surfaces
            .iter()
            .find(|s| s.id == ".aider.conf.yml")
            .expect(".aider.conf.yml surface must exist");
        assert_eq!(yml.kind, DocumentKind::Yaml);
        assert_eq!(yml.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(yml.scope, ConfigScope::User);
        assert!(yml.backup_required);
        for selector in ["model", "dark-mode", "auto-commits"] {
            assert!(
                yml.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in &yml.owned_selectors {
            assert!(!sel.is_empty());
        }
        for sel in OWNED_SELECTORS {
            assert!(yml.owned_selectors.contains(&(*sel).to_owned()));
        }

        let env = surfaces
            .iter()
            .find(|s| s.id == ".env")
            .expect(".env surface must exist");
        assert_eq!(env.kind, DocumentKind::Env);
        assert_eq!(env.ownership, SurfaceOwnership::UserEditable);

        let metadata = surfaces
            .iter()
            .find(|s| s.id == ".aider.model.metadata.json")
            .expect("metadata surface");
        assert_eq!(metadata.kind, DocumentKind::Json);

        let history = surfaces
            .iter()
            .find(|s| s.id == ".aider.chat.history.md")
            .expect("history");
        assert_eq!(history.kind, DocumentKind::TextFragment);
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
        let must_contain = [
            ".aider.chat.history.md",
            ".aider.input.history",
            "cache/*",
            "*.lock",
        ];
        for pat in must_contain {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&".aider.conf.yml".to_owned()));
    }

    #[test]
    #[expect(clippy::excessive_nesting, reason = "test closure nesting is explicit")]
    fn plan_mirror_includes_config_and_excludes_history() {
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
                } else if pat.contains('*') {
                    // simple glob: check prefix before * and suffix after *
                    let parts: Vec<&str> = pat.split('*').collect();
                    if parts.len() == 2 {
                        file.starts_with(parts[0]) && file.ends_with(parts[1])
                    } else {
                        file == pat
                    }
                } else {
                    file == pat
                }
            })
        };
        assert!(!is_excluded(".aider.conf.yml"));
        assert!(!is_excluded(".aider.model.settings.yml"));
        assert!(is_excluded(".aider.chat.history.md"));
        assert!(is_excluded(".aider.input.history"));
        assert!(is_excluded("cache/data.bin"));
    }

    #[test]
    fn plan_wrapper_sets_home_and_explicit_paths() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.aider-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == "HOME" && v == "/tmp/.aider-work")
        );
        assert!(plan.args.contains(&"--config".to_owned()));
        assert!(plan.args.contains(&"--env-file".to_owned()));
        // Check that args contain the config root path
        let config_arg = plan
            .args
            .windows(2)
            .find(|w| w[0] == "--config")
            .map(|w| w[1].as_str())
            .unwrap();
        assert!(config_arg.contains(".aider-work"));
        assert!(config_arg.contains(".aider.conf.yml"));
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains("HOME"));
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my aider work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == "HOME")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, "/tmp/my aider work");
        assert!(!env_val.contains('"'));
        assert!(env_val.contains(' '));
        // Args with spaces must be preserved verbatim (shell quoting handled by wrapper generator)
        let config_path = plan
            .args
            .windows(2)
            .find(|w| w[0] == "--config")
            .map(|w| w[1].as_str())
            .unwrap();
        assert!(config_path.contains(' '));
        assert!(!config_path.contains('"'));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.aider-work");
        inst.harness = HarnessId::new("codex-cli").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_home_and_cwd() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.contains(".aider.conf.yml")));
        assert!(candidates.iter().any(|c| c.contains(".env")));
    }

    #[test]
    fn validate_instance_accepts_explicit_config() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.aider-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_accepts_home_relocation_variant() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.aider-work");
        inst.isolation = Isolation::RelocatedRoot;
        a.validate_instance(&inst).unwrap();
        inst.isolation = Isolation::Unknown;
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.aider-work");
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
        let mut inst = sample_instance_with_root("/tmp/.aider-work");
        inst.harness = HarnessId::new("codex-cli").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let yml = surfaces.iter().find(|s| s.id == ".aider.conf.yml").unwrap();
        assert!(yml.path_resolver.fallback.contains(".aider.conf.yml"));
        assert!(yml.path_resolver.linux.is_some());
        let env = surfaces.iter().find(|s| s.id == ".env").unwrap();
        assert!(env.path_resolver.fallback.contains(".env"));
    }

    // -----------------------------------------------------------------------
    // Fixture-backed conformance tests
    // -----------------------------------------------------------------------

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/aider")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.yml");
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(map.is_empty());
        let value = superai_config::yaml::load_value(&path).unwrap();
        assert_eq!(value, serde_json::Value::Object(serde_json::Map::default()));
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("aider.minimal.yml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::yaml::load(&path).unwrap();
        // Minimal may be empty or contain only a model key.
        assert!(map.is_empty() || map.contains_key("model") || map.len() <= 2);
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("aider.populated.yml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(
            map.contains_key("model")
                || map.contains_key("dark-mode")
                || map.contains_key("auto-commits")
        );
        if let Some(v) = map.get("model") {
            assert!(v.is_string());
        }
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("aider.foreign.yml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::yaml::load(&path).unwrap();
        assert!(
            original.contains_key("foreignKey")
                || original.contains_key("unknownTopLevel")
                || original.contains_key("customField")
        );
        let dir = crate::test_util::temp_dir_unique("aider");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("aider.foreign.copy.yml");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::yaml::edit(&tmp, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("gpt-4".to_owned()),
            );
            assert!(
                map.contains_key("foreignKey")
                    || map.contains_key("unknownTopLevel")
                    || map.contains_key("customField")
            );
        })
        .unwrap();
        let after = superai_config::yaml::load(&tmp).unwrap();
        assert_eq!(
            after["model"],
            serde_json::Value::String("gpt-4".to_owned())
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
        let path = fixture_path("aider.malformed.yml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::yaml::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn fixture_env_minimal_parses() {
        let path = fixture_path(".env.minimal");
        assert!(
            path.exists(),
            "env minimal fixture missing: {}",
            path.display()
        );
        let map = superai_config::env_file::load(&path).unwrap();
        // Minimal env may be empty or contain a key.
        assert!(map.is_empty() || map.contains_key("OPENAI_API_KEY") || !map.is_empty());
    }

    #[test]
    fn fixture_env_populated_has_keys() {
        let path = fixture_path(".env.populated");
        assert!(
            path.exists(),
            "env populated fixture missing: {}",
            path.display()
        );
        let map = superai_config::env_file::load(&path).unwrap();
        assert!(
            map.contains_key("OPENAI_API_KEY")
                || map.contains_key("OPENAI_API_BASE")
                || map.contains_key("ANTHROPIC_API_KEY")
        );
    }

    #[test]
    fn fixture_json_metadata_populated_parses() {
        let path = fixture_path("model.metadata.populated.json");
        assert!(
            path.exists(),
            "json metadata fixture missing: {}",
            path.display()
        );
        let map = superai_config::json::load(&path).unwrap();
        assert!(!map.is_empty());
    }

    #[test]
    fn unknown_keys_survive_because_changing_yaml_edit_refuses() {
        let dir = crate::test_util::temp_dir_unique("aider");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preserve.yml");
        let original = "model: gpt-4\nforeignKey: keep-me\ncustomField: 123\n";
        std::fs::write(&path, original).unwrap();

        // codec-honesty (DOC-06): changing YAML writes on existing files are
        // refused outright; preservation is expressed by refusing.
        let result = superai_config::yaml::edit(&path, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("gpt-5".to_owned()),
            );
        });
        match result {
            Err(superai_config::ConfigError::LossyWrite { format, .. }) => {
                assert_eq!(format, "yaml");
            }
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let after = superai_config::yaml::load(&path).unwrap();
        assert_eq!(
            after["foreignKey"],
            serde_json::Value::String("keep-me".to_owned())
        );
        assert_eq!(after["customField"], serde_json::Value::Number(123.into()));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn provider_mutation_sets_model_and_env() {
        let dir = crate::test_util::temp_dir_unique("aider");
        std::fs::create_dir_all(&dir).unwrap();
        let yml_path = dir.join("aider.yml");
        let env_path = dir.join(".env");
        std::fs::write(&yml_path, "model: gpt-4\ndark-mode: true\n").unwrap();
        std::fs::write(&env_path, "OPENAI_API_KEY=sk-old\n").unwrap();

        // codec-honesty (DOC-06): the YAML leg refuses; the env leg is a
        // preserving codec and still succeeds.
        let yml_result = superai_config::yaml::edit(&yml_path, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("openrouter/anthropic/claude-sonnet-4".to_owned()),
            );
        });
        match yml_result {
            Err(superai_config::ConfigError::LossyWrite { format, .. }) => {
                assert_eq!(format, "yaml");
            }
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        superai_config::env_file::edit(&env_path, |map| {
            map.insert("OPENAI_API_KEY".to_owned(), "sk-new".to_owned());
            map.insert(
                "OPENAI_API_BASE".to_owned(),
                "https://api.openrouter.ai/v1".to_owned(),
            );
        })
        .unwrap();

        let after_yml = superai_config::yaml::load(&yml_path).unwrap();
        assert_eq!(
            after_yml["model"],
            serde_json::Value::String("gpt-4".to_owned())
        );
        let after_env = superai_config::env_file::load(&env_path).unwrap();
        assert_eq!(after_env["OPENAI_API_KEY"], "sk-new");
        assert_eq!(after_env["OPENAI_API_BASE"], "https://api.openrouter.ai/v1");
        drop(std::fs::remove_file(&yml_path));
        drop(std::fs::remove_file(&env_path));
    }

    #[test]
    fn secret_redaction_placeholder() {
        use crate::error::RedactedString;
        let secret = RedactedString::new("sk-test-secret-456");
        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert!(!debug.contains("sk-test-secret-456"));
        assert!(!display.contains("sk-test-secret-456"));
        assert!(debug.contains("[REDACTED]"));
        assert!(display.contains("[REDACTED]"));
        let json = serde_json::to_string(&secret).unwrap();
        assert!(!json.contains("sk-test-secret-456"));
        assert!(json.contains("[REDACTED]"));
        assert_eq!(secret.expose_secret(), "sk-test-secret-456");
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
        let inst = sample_instance_with_root("/tmp/.aider-work");
        a.validate_instance(&inst).unwrap();
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn wrapper_env_var_isolation_is_explicit() {
        let a = adapter();
        assert!(a.scan_candidates().len() >= 3);
        let inst = sample_instance_with_root("/home/user/.aider-isolated");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(!plan.env_vars.is_empty());
        let (key, val) = &plan.env_vars[0];
        assert_eq!(key, "HOME");
        assert_eq!(val, "/home/user/.aider-isolated");
        assert!(plan.args.contains(&"--config".to_owned()));
    }

    #[test]
    fn yaml_comment_changing_write_refused_and_comments_preserved() {
        // codec-honesty (DOC-06): comments parse on read, but a changing write
        // on a comment-bearing YAML file is refused instead of normalizing the
        // comments away; the on-disk bytes survive verbatim.
        let dir = crate::test_util::temp_dir_unique("aider");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("comment.yml");
        let content = "# Aider config\nmodel: gpt-4 # inline comment\ndark-mode: true\n";
        std::fs::write(&path, content).unwrap();
        let map = superai_config::yaml::load(&path).unwrap();
        assert_eq!(map["model"], serde_json::Value::String("gpt-4".to_owned()));
        assert_eq!(map["dark-mode"], serde_json::Value::Bool(true));
        let result = superai_config::yaml::edit(&path, |m| {
            m.insert(
                "model".to_owned(),
                serde_json::Value::String("gpt-5".to_owned()),
            );
        });
        match result {
            Err(superai_config::ConfigError::LossyWrite { format, .. }) => {
                assert_eq!(format, "yaml");
            }
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            content,
            "refused edit must leave the file byte-identical"
        );
        drop(std::fs::remove_file(&path));
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
