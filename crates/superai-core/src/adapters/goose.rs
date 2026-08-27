//! Goose adapter — relocated-root via `GOOSE_PATH_ROOT` with YAML config/recipes.
//!
//! Research source: `docs/harness-configs/goose.md` (last verified 2026-08-25).
//! Executable `goose`, config root `~/.config/goose` or `$GOOSE_PATH_ROOT`,
//! primary writable surface `config.yaml` (YAML), isolation `relocated-root`.

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

/// Harness identifier for Goose.
pub const HARNESS_ID_STR: &str = "goose";

/// Human display name.
pub const DISPLAY_NAME: &str = "Goose";

/// Primary executable name.
pub const EXECUTABLE: &str = "goose";

/// Environment variable that relocates the config root.
pub const CONFIG_ENV_VAR: &str = "GOOSE_PATH_ROOT";

/// Default config root when `GOOSE_PATH_ROOT` is unset.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.config/goose";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/goose.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current config shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider/model mutation inside `config.yaml`.
pub const OWNED_SELECTORS: &[&str] = &[
    "GOOSE_PROVIDER",
    "GOOSE_MODEL",
    "GOOSE_MODE",
    "GOOSE_TEMPERATURE",
    "GOOSE_MAX_TOKENS",
    "extensions",
    "enabled_extensions",
    "GOOSE_LEAD_PROVIDER",
    "GOOSE_LEAD_MODEL",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Goose.
///
/// Isolation is `relocated-root` via `GOOSE_PATH_ROOT`. The wrapper sets
/// `GOOSE_PATH_ROOT` to the instance `config_root` and execs `goose`.
#[derive(Debug, Clone)]
pub struct GooseAdapter {
    id: HarnessId,
}

impl GooseAdapter {
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

    /// Try to locate the `goose` binary via `PATH`.
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

    /// Probe `goose --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `goose 1.2.3` into `1.2.3`.
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

    /// Resolve the default config root: `$GOOSE_PATH_ROOT` or `~/.config/goose`.
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
        Some(PathBuf::from(home).join(".config").join("goose"))
    }

    /// Check if default config root exists on disk.
    #[expect(dead_code, reason = "helper for future use")]
    #[expect(clippy::unused_self, reason = "adapter helper")]
    fn default_config_root_exists(&self) -> Option<PathBuf> {
        let root = Self::default_config_root()?;
        if root.exists() { Some(root) } else { None }
    }

    /// Build the config.yaml path for a given config root.
    fn config_path_for_root(root: &Path) -> PathBuf {
        root.join("config.yaml")
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
                    if cfg.exists() {
                        evidence.push(format!("config.yaml found at {}", cfg.display()));
                        if let Ok(text) = std::fs::read_to_string(&cfg)
                            && (text.contains("GOOSE_PROVIDER") || text.contains("extensions"))
                        {
                            evidence
                                .push("config.yaml contains GOOSE_PROVIDER/extensions".to_owned());
                        }
                    } else {
                        evidence.push(format!("config.yaml missing at {}", cfg.display()));
                    }
                    let secrets = root.join("secrets.yaml");
                    if secrets.exists() {
                        evidence.push(format!("secrets.yaml present at {}", secrets.display()));
                    }
                    let recipes = root.join("recipes");
                    if recipes.exists() {
                        evidence.push(format!("recipes dir present at {}", recipes.display()));
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
            evidence.push(format!("{CONFIG_ENV_VAR} not set, using ~/.config/goose"));
        }
    }
}

impl Default for GooseAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "goose is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for GooseAdapter {
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
            notes.push(format!("detected goose version {v}"));
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

        let config_resolver = PathResolver::new(
            Some("$GOOSE_PATH_ROOT/config.yaml"),
            Some("$GOOSE_PATH_ROOT/config.yaml"),
            Some("%GOOSE_PATH_ROOT%\\config.yaml"),
            "~/.config/goose/config.yaml",
        );
        let mut config_surface = ConfigSurface::new(
            "config.yaml",
            config_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        config_surface.precedence = 10;
        config_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        config_surface.backup_required = true;
        config_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(config_surface);

        let secrets_resolver = PathResolver::new(
            Some("$GOOSE_PATH_ROOT/secrets.yaml"),
            Some("$GOOSE_PATH_ROOT/secrets.yaml"),
            Some("%GOOSE_PATH_ROOT%\\secrets.yaml"),
            "~/.config/goose/secrets.yaml",
        );
        let mut secrets = ConfigSurface::new(
            "secrets.yaml",
            secrets_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        secrets.precedence = 5;
        secrets.backup_required = false;
        secrets.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(secrets);

        let recipe_resolver = PathResolver::new(
            Some("$GOOSE_PATH_ROOT/recipes/<name>.yaml"),
            Some("$GOOSE_PATH_ROOT/recipes/<name>.yaml"),
            Some("%GOOSE_PATH_ROOT%\\recipes\\<name>.yaml"),
            "~/.config/goose/recipes/<name>.yaml",
        );
        let mut recipe = ConfigSurface::new(
            "recipes",
            recipe_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        recipe.precedence = 15;
        recipe.backup_required = false;
        surfaces.push(recipe);

        let ext_resolver = PathResolver::new(
            Some("$GOOSE_PATH_ROOT/config.yaml (extensions key)"),
            Some("$GOOSE_PATH_ROOT/config.yaml (extensions key)"),
            Some("%GOOSE_PATH_ROOT%\\config.yaml (extensions key)"),
            "~/.config/goose/config.yaml (extensions key)",
        );
        let mut extensions = ConfigSurface::new(
            "extensions",
            ext_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        extensions.precedence = 12;
        extensions.owned_selectors = vec!["extensions".to_owned(), "enabled_extensions".to_owned()];
        extensions.backup_required = true;
        surfaces.push(extensions);

        let session_resolver = PathResolver::new(
            Some("$GOOSE_PATH_ROOT/sessions/<id>.jsonl"),
            Some("$GOOSE_PATH_ROOT/sessions/<id>.jsonl"),
            Some("%GOOSE_PATH_ROOT%\\sessions\\<id>.jsonl"),
            "~/.config/goose/sessions/<id>.jsonl",
        );
        let mut sessions = ConfigSurface::new(
            "sessions",
            session_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        sessions.precedence = 0;
        sessions.backup_required = false;
        sessions.restart_behavior = RestartBehavior::None;
        surfaces.push(sessions);

        let skills_resolver = PathResolver::new(
            Some("$GOOSE_PATH_ROOT/skills/<name>/SKILL.md"),
            Some("$GOOSE_PATH_ROOT/skills/<name>/SKILL.md"),
            Some("%GOOSE_PATH_ROOT%\\skills\\<name>\\SKILL.md"),
            "~/.config/goose/skills/<name>/SKILL.md",
        );
        let mut skills = ConfigSurface::new(
            "skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills.precedence = 8;
        skills.backup_required = false;
        surfaces.push(skills);

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
            "session-store.db".to_owned(),
            "logs/*".to_owned(),
            "history/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "tmp/*".to_owned(),
            "*.lock".to_owned(),
            "data/*".to_owned(),
            "state/*".to_owned(),
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
        instance.validate()?;
        let mut plan = WrapperPlan::new("relocated-root via GOOSE_PATH_ROOT");
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
            "~/.config/goose".to_owned(),
            "~/.config/goose-work".to_owned(),
            "~/.goose".to_owned(),
            "$GOOSE_PATH_ROOT".to_owned(),
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
                reason: format!("goose requires isolation relocated_root, got {other}"),
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
        CONFIG_ENV_VAR, DISPLAY_NAME, EXECUTABLE, GooseAdapter, HARNESS_ID_STR, OWNED_SELECTORS,
        RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> GooseAdapter {
        GooseAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-goose-1").unwrap(),
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
            ("goose 1.2.3", Some("1.2.3")),
            ("goose version 0.5.0", Some("0.5.0")),
            ("v1.0.0", Some("1.0.0")),
            ("Version: 2.0.0", Some("2.0.0")),
            ("1.0.0-alpha", Some("1.0.0-alpha")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = GooseAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_writable_yaml() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let config = surfaces
            .iter()
            .find(|s| s.id == "config.yaml")
            .expect("config.yaml surface must exist");
        assert_eq!(config.kind, DocumentKind::Yaml);
        assert_eq!(config.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(config.scope, ConfigScope::User);
        assert!(config.backup_required);
        for selector in ["GOOSE_PROVIDER", "GOOSE_MODEL", "extensions"] {
            assert!(
                config.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in &config.owned_selectors {
            assert!(!sel.is_empty());
        }
        for sel in OWNED_SELECTORS {
            assert!(config.owned_selectors.contains(&(*sel).to_owned()));
        }

        let secrets = surfaces
            .iter()
            .find(|s| s.id == "secrets.yaml")
            .expect("secrets.yaml surface must exist");
        assert_eq!(secrets.ownership, SurfaceOwnership::ExternalSecretStore);
        assert!(!secrets.backup_required);
        assert_eq!(secrets.kind, DocumentKind::Yaml);
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
        assert!(!exclusions.contains(&"config.yaml".to_owned()));
    }

    #[test]
    #[expect(clippy::excessive_nesting, reason = "test closure nesting is explicit")]
    fn plan_mirror_includes_config_and_excludes_sessions() {
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
        assert!(!is_excluded("config.yaml"));
        assert!(!is_excluded("recipes/test.yaml"));
        assert!(is_excluded("sessions/abc.jsonl"));
        assert!(is_excluded("history/history.jsonl"));
        assert!(is_excluded("logs/goose.log"));
    }

    #[test]
    fn plan_wrapper_sets_goose_path_root() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.goose-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == "/tmp/.goose-work")
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(CONFIG_ENV_VAR));
        assert!(plan.args.is_empty());
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/my goose work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == CONFIG_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, "/tmp/my goose work");
        assert!(!env_val.contains('"'));
        assert!(env_val.contains(' '));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.goose-work");
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
        assert!(candidates.iter().any(|c| c.contains("goose")));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.goose-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.goose-work");
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
        let mut inst = sample_instance_with_root("/tmp/.goose-work");
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let config = surfaces.iter().find(|s| s.id == "config.yaml").unwrap();
        assert_eq!(config.path_resolver.fallback, "~/.config/goose/config.yaml");
        let resolver = &config.path_resolver;
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
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/goose")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.yaml");
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(map.is_empty());
        let value = superai_config::yaml::load_value(&path).unwrap();
        assert_eq!(value, serde_json::Value::Object(serde_json::Map::default()));
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("config.minimal.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(map.is_empty() || map.contains_key("GOOSE_PROVIDER") || map.len() <= 2);
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("config.populated.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::yaml::load(&path).unwrap();
        assert!(
            map.contains_key("GOOSE_PROVIDER")
                || map.contains_key("GOOSE_MODEL")
                || map.contains_key("extensions")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("config.foreign.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::yaml::load(&path).unwrap();
        assert!(
            original.contains_key("foreignKey")
                || original.contains_key("unknownTopLevel")
                || original.contains_key("customField")
        );
        let dir = crate::test_util::temp_dir_unique("goose");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("config.foreign.copy.yaml");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::yaml::edit(&tmp, |map| {
            map.insert(
                "GOOSE_PROVIDER".to_owned(),
                serde_json::Value::String("openai".to_owned()),
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
            after["GOOSE_PROVIDER"],
            serde_json::Value::String("openai".to_owned())
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
        let path = fixture_path("config.malformed.yaml");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::yaml::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn unknown_key_preservation_via_yaml_edit() {
        let dir = crate::test_util::temp_dir_unique("goose");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preserve.yaml");
        let original = "GOOSE_PROVIDER: openai\nforeignKey: keep-me\ncustomField: 123\n";
        std::fs::write(&path, original).unwrap();

        superai_config::yaml::edit(&path, |map| {
            map.insert(
                "GOOSE_PROVIDER".to_owned(),
                serde_json::Value::String("anthropic".to_owned()),
            );
        })
        .unwrap();
        let after = superai_config::yaml::load(&path).unwrap();
        assert_eq!(
            after["foreignKey"],
            serde_json::Value::String("keep-me".to_owned())
        );
        assert_eq!(after["customField"], serde_json::Value::Number(123.into()));
        assert_eq!(
            after["GOOSE_PROVIDER"],
            serde_json::Value::String("anthropic".to_owned())
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn secret_redaction_placeholder() {
        use crate::error::RedactedString;
        let secret = RedactedString::new("sk-goose-secret-456");
        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert!(!debug.contains("sk-goose-secret-456"));
        assert!(!display.contains("sk-goose-secret-456"));
        assert!(debug.contains("[REDACTED]"));
        assert!(display.contains("[REDACTED]"));
        let json = serde_json::to_string(&secret).unwrap();
        assert!(!json.contains("sk-goose-secret-456"));
        assert!(json.contains("[REDACTED]"));
        assert_eq!(secret.expose_secret(), "sk-goose-secret-456");
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
        let inst = sample_instance_with_root("/tmp/.goose-work");
        a.validate_instance(&inst).unwrap();
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn wrapper_env_var_isolation_is_relocated_root() {
        let a = adapter();
        assert!(a.scan_candidates().len() >= 3);
        let inst = sample_instance_with_root("/home/user/.goose-isolated");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(!plan.env_vars.is_empty());
        let (key, val) = &plan.env_vars[0];
        assert_eq!(key, CONFIG_ENV_VAR);
        assert_eq!(val, "/home/user/.goose-isolated");
    }

    #[test]
    fn registry_no_harness_value_leak() {
        let inst = sample_instance_with_root("/tmp/.goose-work");
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
