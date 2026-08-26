//! Claude Code adapter — relocated-root via `CLAUDE_CONFIG_DIR`.
//!
//! Research source: `docs/harness-configs/claude-code.md` (last verified 2026-08-25).
//! Executable `claude`, config root `~/.claude` or `$CLAUDE_CONFIG_DIR`, primary
//! writable surface `settings.json` (JSON/JSONC), isolation `relocated-root`.

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

/// Harness identifier for Claude Code.
pub const HARNESS_ID_STR: &str = "claude-code";

/// Human display name.
pub const DISPLAY_NAME: &str = "Claude Code";

/// Primary executable name.
pub const EXECUTABLE: &str = "claude";

/// Environment variable that relocates the config root.
pub const CONFIG_ENV_VAR: &str = "CLAUDE_CONFIG_DIR";

/// Default config root when `CLAUDE_CONFIG_DIR` is unset.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.claude";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/claude-code.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version for current settings shape.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors for provider/model mutation — instance-specific fields.
///
/// These are the selectors superai owns inside `settings.json`. Everything
/// else round-trips untouched via `superai-config::json`.
pub const OWNED_SELECTORS: &[&str] = &[
    "model",
    "env.ANTHROPIC_BASE_URL",
    "env.ANTHROPIC_AUTH_TOKEN",
    "env.ANTHROPIC_API_KEY",
    "env.ANTHROPIC_MODEL",
    "env.ANTHROPIC_DEFAULT_MODEL",
    "apiKeyHelper",
    "env.CLAUDE_CODE_USE_BEDROCK",
    "env.CLAUDE_CODE_USE_VERTEX",
    "env.CLAUDE_CODE_USE_FOUNDRY",
];

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Claude Code.
///
/// Isolation is `relocated-root` via `CLAUDE_CONFIG_DIR`. The wrapper sets
/// `CLAUDE_CONFIG_DIR` to the instance `config_root` and execs `claude`.
#[derive(Debug, Clone)]
pub struct ClaudeCodeAdapter {
    id: HarnessId,
}

impl ClaudeCodeAdapter {
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

    /// Try to locate the `claude` binary via `PATH`.
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

    /// Probe `claude --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `2.0.12 (Claude Code)` or `claude 1.5.3` into `1.5.3`.
    #[expect(
        clippy::excessive_nesting,
        reason = "version parsing branches are explicit"
    )]
    fn parse_version_output(output: &str) -> Option<String> {
        let trimmed = output.trim();
        if trimmed.is_empty() {
            return None;
        }
        // Split by whitespace and look for token containing '.' and starting with digit or 'v'.
        for token in trimmed.split_whitespace() {
            let mut candidate = token;
            // Strip leading 'v' or 'V'.
            if let Some(stripped) = candidate.strip_prefix('v') {
                candidate = stripped;
            } else if let Some(stripped) = candidate.strip_prefix('V') {
                candidate = stripped;
            }
            // Remove trailing punctuation like ',' or ')'.
            let cleaned = candidate.trim_matches(|c: char| c == ',' || c == ')' || c == '(');
            if cleaned.is_empty() {
                continue;
            }
            let has_dot = cleaned.contains('.');
            let starts_digit = cleaned.chars().next().is_some_and(|c| c.is_ascii_digit());
            if has_dot && starts_digit {
                // Accept semver-like strings: allow alphanumeric suffix after dash/plus.
                // Trim trailing junk but keep suffix like "-alpha", "-rc1", "+build".
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

    /// Resolve the default config root: `$CLAUDE_CONFIG_DIR` or `~/.claude`.
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
        Some(PathBuf::from(home).join(".claude"))
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
                        // Check for schema marker.
                        if let Ok(text) = std::fs::read_to_string(&settings)
                            && (text.contains("\"$schema\"") || text.contains("$schema"))
                        {
                            evidence.push("settings.json contains $schema marker".to_owned());
                        }
                    } else {
                        evidence.push(format!("settings.json missing at {}", settings.display()));
                    }
                    let creds = root.join(".credentials.json");
                    if creds.exists() {
                        evidence.push(format!("credentials file present at {}", creds.display()));
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

impl Default for ClaudeCodeAdapter {
    fn default() -> Self {
        // Static id is known valid; use expect with reason for the lint.
        #[expect(clippy::unwrap_used, reason = "claude-code is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for ClaudeCodeAdapter {
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
                // Try version probe.
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

        // If absent, confidence is high (we looked and found nothing).
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
            notes.push(format!("detected claude version {v}"));
            // For now, any detected version maps to schema 1 and is compatible.
            // Future versions may branch.
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            let mut res =
                VersionResolution::new(Some(v), Some(SCHEMA_VERSION_STR.to_owned()), true);
            res.notes = notes;
            res
        } else {
            let mut res = VersionResolution::unknown();
            // Preserve evidence from detection for debugging.
            res.notes = detection.evidence;
            res
        }
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        // Primary writable surface: settings.json under CLAUDE_CONFIG_DIR or ~/.claude.
        let settings_resolver = PathResolver::new(
            Some("$CLAUDE_CONFIG_DIR/settings.json"),
            Some("$CLAUDE_CONFIG_DIR/settings.json"),
            Some("%CLAUDE_CONFIG_DIR%\\settings.json"),
            "~/.claude/settings.json",
        );
        let mut settings_surface = ConfigSurface::new(
            "settings.json",
            settings_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        settings_surface.precedence = 10;
        settings_surface.owned_selectors =
            OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        settings_surface.backup_required = true;
        settings_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(settings_surface);

        // Secondary surface: .claude.json (global config, MCP servers, OAuth state) — harness-managed, not directly writable for provider.
        let claude_json_resolver = PathResolver::new(
            Some("$CLAUDE_CONFIG_DIR/.claude.json"),
            Some("$CLAUDE_CONFIG_DIR/.claude.json"),
            Some("%CLAUDE_CONFIG_DIR%\\.claude.json"),
            "~/.claude.json",
        );
        let mut claude_json = ConfigSurface::new(
            ".claude.json",
            claude_json_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        claude_json.precedence = 5;
        claude_json.backup_required = true;
        claude_json.restart_behavior = RestartBehavior::Reload;
        surfaces.push(claude_json);

        // Project MCP surface: .mcp.json at project root — writable for MCP management.
        let mcp_resolver = PathResolver::fallback_only(".mcp.json (project root)");
        let mut mcp_surface = ConfigSurface::new(
            ".mcp.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        mcp_surface.precedence = 20;
        mcp_surface.owned_selectors = vec!["mcpServers".to_owned()];
        mcp_surface.backup_required = true;
        surfaces.push(mcp_surface);

        // Credentials surface: .credentials.json — external secret store, not writable.
        let creds_resolver = PathResolver::new(
            Some("$CLAUDE_CONFIG_DIR/.credentials.json"),
            Some("$CLAUDE_CONFIG_DIR/.credentials.json"),
            Some("%CLAUDE_CONFIG_DIR%\\.credentials.json"),
            "~/.claude/.credentials.json",
        );
        let mut creds = ConfigSurface::new(
            ".credentials.json",
            creds_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        creds.precedence = 0;
        creds.backup_required = false;
        creds.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(creds);

        // Skills surface: SKILL.md files — text fragments.
        let skills_resolver = PathResolver::new(
            Some("$CLAUDE_CONFIG_DIR/skills/<name>/SKILL.md"),
            Some("$CLAUDE_CONFIG_DIR/skills/<name>/SKILL.md"),
            Some("%CLAUDE_CONFIG_DIR%\\skills\\<name>\\SKILL.md"),
            "~/.claude/skills/<name>/SKILL.md",
        );
        let mut skills = ConfigSurface::new(
            "skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills.precedence = 15;
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
            "projects/*".to_owned(),
            "history.jsonl".to_owned(),
            "debug/*".to_owned(),
            "todos/*".to_owned(),
            "file-history/*".to_owned(),
            "shell-snapshots/*".to_owned(),
            "statsig/*".to_owned(),
            ".credentials.json".to_owned(),
            "cache/*".to_owned(),
            "*.lock".to_owned(),
            "logs/*".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
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
        let mut plan = WrapperPlan::new("relocated-root via CLAUDE_CONFIG_DIR");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        // No extra args; the binary is EXECUTABLE and will be resolved via PATH
        // or instance.binary if set. The wrapper execs `claude` with the env set.
        plan.description = format!(
            " Wrapper sets {}={} and execs `{}`",
            CONFIG_ENV_VAR, instance.config_root, EXECUTABLE
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.claude".to_owned(),
            "~/.claude-work".to_owned(),
            "~/.claude-glm".to_owned(),
            "$CLAUDE_CONFIG_DIR".to_owned(),
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
        // Enforce relocated-root isolation for Claude Code; allow Unknown for legacy adoption.
        match instance.isolation {
            Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("claude-code requires isolation relocated_root, got {other}"),
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
        CONFIG_ENV_VAR, ClaudeCodeAdapter, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR,
        OWNED_SELECTORS, RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> ClaudeCodeAdapter {
        ClaudeCodeAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-claude-1").unwrap(),
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
        // Detection must always return evidence and a confidence.
        assert!(!result.evidence.is_empty());
        // Present can be Absent on CI where claude is not installed; just check it's coherent.
        match result.present {
            InstallPresence::Absent => {
                assert!(result.version.is_none());
                assert!(!result.evidence.is_empty());
            }
            InstallPresence::Present => {
                assert!(result.version.is_some());
            }
            InstallPresence::UnknownVersion => {
                // binary found but version unknown
                assert!(result.evidence.iter().any(|e| e.contains("found binary")));
            }
            InstallPresence::Broken => {
                assert!(!result.evidence.is_empty());
            }
        }
        // Confidence must be set.
        assert_ne!(result.confidence.to_string(), "");
    }

    #[test]
    fn version_resolution_maps_detected() {
        let a = adapter();
        let res = a.version_resolution();
        // If detection found a version, schema is Some("1") and compatible true.
        // If not, it's unknown and not compatible.
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
            ("2.0.12 (Claude Code)", Some("2.0.12")),
            ("claude 1.5.3", Some("1.5.3")),
            ("v2.1.13", Some("2.1.13")),
            ("Version: 2.1.0", Some("2.1.0")),
            ("2.0.3-alpha", Some("2.0.3-alpha")),
            ("", None),
            ("not a version", None),
            ("claude-code 2.1.13", Some("2.1.13")),
        ];
        for (input, expected) in cases {
            let got = ClaudeCodeAdapter::parse_version_output(input);
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
        assert_eq!(settings.kind, DocumentKind::Jsonc);
        assert_eq!(settings.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(settings.scope, ConfigScope::User);
        assert!(settings.backup_required);
        // Owned selectors must include provider/model fields.
        for selector in ["model", "env.ANTHROPIC_BASE_URL", "apiKeyHelper"] {
            assert!(
                settings.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        // All owned selectors are non-empty.
        for sel in &settings.owned_selectors {
            assert!(!sel.is_empty());
        }
        // Verify owned selectors constant matches surface.
        for sel in OWNED_SELECTORS {
            assert!(settings.owned_selectors.contains(&(*sel).to_owned()));
        }

        // Credentials surface must be external secret store and not require backup.
        let creds = surfaces
            .iter()
            .find(|s| s.id == ".credentials.json")
            .expect("credentials surface must exist");
        assert_eq!(creds.ownership, SurfaceOwnership::ExternalSecretStore);
        assert!(!creds.backup_required);

        // Skills surface must be text fragment.
        let skills = surfaces.iter().find(|s| s.id == "skills").expect("skills");
        assert_eq!(skills.kind, DocumentKind::TextFragment);
    }

    #[test]
    fn owned_selectors_are_stable() {
        // Ensure we have at least 5 selectors and they are distinct.
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
        // Must exclude sessions/history/logs/caches/locks.
        let must_contain = [
            "projects/*",
            "history.jsonl",
            "debug/*",
            "cache/*",
            "*.lock",
            "logs/*",
        ];
        for pat in must_contain {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        // Ensure no writable settings are excluded.
        assert!(!exclusions.contains(&"settings.json".to_owned()));
    }

    #[test]
    #[expect(clippy::excessive_nesting, reason = "test closure nesting is explicit")]
    fn plan_mirror_includes_settings_and_excludes_sessions() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        // Simulate a file list: included are settings.json, skills, mcp; excluded are projects etc.
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
        assert!(!is_excluded("skills/my-skill/SKILL.md"));
        assert!(is_excluded("projects/myproj/session.jsonl"));
        assert!(is_excluded("history.jsonl"));
        assert!(is_excluded("debug/abc.txt"));
    }

    #[test]
    fn plan_wrapper_sets_claude_config_dir() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.claude-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == "/tmp/.claude-work")
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(CONFIG_ENV_VAR));
        assert!(plan.args.is_empty());
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let a = adapter();
        // Path with spaces must be preserved verbatim in env var value.
        let inst = sample_instance_with_root("/tmp/my claude work");
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == CONFIG_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, "/tmp/my claude work");
        // Shell quoting is handled by the wrapper generator, not the plan;
        // the plan must not pre-quote or escape.
        assert!(!env_val.contains('"'));
        assert!(env_val.contains(' '));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.claude-work");
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
        assert!(candidates.iter().any(|c| c.contains(".claude")));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.claude-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.claude-work");
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
        let mut inst = sample_instance_with_root("/tmp/.claude-work");
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let settings = surfaces.iter().find(|s| s.id == "settings.json").unwrap();
        assert_eq!(settings.path_resolver.fallback, "~/.claude/settings.json");
        // Linux/macos/windows hints should contain CLAUDE_CONFIG_DIR
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
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/claude_code")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent-settings.json");
        // Ensure missing file is treated as empty object.
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
        // Minimal may be empty or contain only $schema.
        // Should not error.
        assert!(map.is_empty() || map.contains_key("$schema"));
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("settings.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::json::load(&path).unwrap();
        assert!(
            map.contains_key("env") || map.contains_key("model") || map.contains_key("permissions")
        );
        // If env exists, check that ANTHROPIC keys are plausible.
        if let Some(env) = map.get("env")
            && let Some(obj) = env.as_object()
        {
            for key in obj.keys() {
                assert!(!key.is_empty());
            }
        }
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("settings.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        // Load original.
        let original = superai_config::json::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        // Simulate provider mutation: create a temp copy, edit owned selector, verify foreign keys survive.
        let dir = std::env::temp_dir().join("superai-claude-foreign-test");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("settings.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        // Edit via superai-config json edit (preserves unknown keys).
        superai_config::json::edit(&tmp, |map| {
            // Mutate an owned selector.
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("sonnet".to_owned()),
            );
            // Ensure foreign keys are still there after edit closure runs on the loaded map.
            // The edit closure receives the map that already contains foreign keys.
            assert!(map.contains_key("foreignKey") || map.contains_key("unknownTopLevel"));
        })
        .unwrap();
        let after = superai_config::json::load(&tmp).unwrap();
        assert!(after.contains_key("model"));
        assert_eq!(
            after["model"],
            serde_json::Value::String("sonnet".to_owned())
        );
        // Foreign keys must still be present.
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
    fn unknown_key_preservation_via_preserve_order() {
        let dir = std::env::temp_dir().join("superai-claude-preserve-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preserve.json");
        let original_json = serde_json::json!({
            "model": "opus",
            "foreignKey": "keep-me",
            "env": {
                "ANTHROPIC_BASE_URL": "https://example.com",
                "CUSTOM_FOO": "bar"
            },
            "anotherForeign": {"nested": 123}
        });
        let text = serde_json::to_string_pretty(&original_json).unwrap();
        std::fs::write(&path, text).unwrap();

        // Edit only the owned selector; foreign keys must survive.
        superai_config::json::edit(&path, |map| {
            if let Some(env) = map.get_mut("env").and_then(|v| v.as_object_mut()) {
                env.insert(
                    "ANTHROPIC_BASE_URL".to_owned(),
                    serde_json::Value::String("https://new.example.com".to_owned()),
                );
            }
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
        assert_eq!(
            after["env"]["CUSTOM_FOO"],
            serde_json::Value::String("bar".to_owned())
        );
        assert_eq!(
            after["env"]["ANTHROPIC_BASE_URL"],
            serde_json::Value::String("https://new.example.com".to_owned())
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn provider_mutation_sets_base_url_and_model() {
        let dir = std::env::temp_dir().join("superai-claude-provider-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("provider.json");
        let initial = serde_json::json!({
            "model": "sonnet",
            "env": {
                "ANTHROPIC_BASE_URL": "https://old.example.com"
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        // Simulate template applying new provider.
        superai_config::json::edit(&path, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("opus".to_owned()),
            );
            let env = map
                .entry("env".to_owned())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::default()));
            if let Some(obj) = env.as_object_mut() {
                obj.insert(
                    "ANTHROPIC_BASE_URL".to_owned(),
                    serde_json::Value::String("https://new.example.com".to_owned()),
                );
                obj.insert(
                    "ANTHROPIC_AUTH_TOKEN".to_owned(),
                    serde_json::Value::String("sk-test-123".to_owned()),
                );
            }
        })
        .unwrap();
        let after = superai_config::json::load(&path).unwrap();
        assert_eq!(after["model"], serde_json::Value::String("opus".to_owned()));
        assert_eq!(
            after["env"]["ANTHROPIC_BASE_URL"],
            serde_json::Value::String("https://new.example.com".to_owned())
        );
        // Verify old value is gone, new value present.
        assert_ne!(
            after["env"]["ANTHROPIC_BASE_URL"],
            serde_json::Value::String("https://old.example.com".to_owned())
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn provider_removal_clears_auth() {
        let dir = std::env::temp_dir().join("superai-claude-provider-remove");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("remove.json");
        let initial = serde_json::json!({
            "env": {
                "ANTHROPIC_API_KEY": "sk-old",
                "ANTHROPIC_BASE_URL": "https://old.example.com"
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        // Remove auth keys.
        superai_config::json::edit(&path, |map| {
            if let Some(env) = map.get_mut("env").and_then(|v| v.as_object_mut()) {
                env.remove("ANTHROPIC_API_KEY");
                env.remove("ANTHROPIC_BASE_URL");
            }
        })
        .unwrap();
        let after = superai_config::json::load(&path).unwrap();
        let env = after.get("env").and_then(|v| v.as_object());
        if let Some(obj) = env {
            assert!(!obj.contains_key("ANTHROPIC_API_KEY"));
            assert!(!obj.contains_key("ANTHROPIC_BASE_URL"));
        }
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn secret_redaction_placeholder() {
        use crate::error::RedactedString;
        let secret = RedactedString::new("sk-ant-secret-123");
        // Debug and Display must not contain the secret.
        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert!(!debug.contains("sk-ant-secret-123"));
        assert!(!display.contains("sk-ant-secret-123"));
        assert!(debug.contains("[REDACTED]"));
        assert!(display.contains("[REDACTED]"));
        // Serialization must also redact.
        let json = serde_json::to_string(&secret).unwrap();
        assert!(!json.contains("sk-ant-secret-123"));
        assert!(json.contains("[REDACTED]"));
        // Expose is explicit.
        assert_eq!(secret.expose_secret(), "sk-ant-secret-123");
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
        // Placeholder: ensure detection and validation can be called repeatedly without side effects.
        let a = adapter();
        let r1 = a.detection();
        let r2 = a.detection();
        // Evidence may vary (e.g., timing), but structure should be consistent.
        assert_eq!(r1.present, r2.present);
        assert_eq!(r1.confidence, r2.confidence);
        // Validate instance twice.
        let inst = sample_instance_with_root("/tmp/.claude-work");
        a.validate_instance(&inst).unwrap();
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn wrapper_env_var_isolation_is_relocated_root() {
        let a = adapter();
        assert_eq!(a.scan_candidates().len(), 4);
        // Simulate wrapper generation: env var must be CLAUDE_CONFIG_DIR.
        let inst = sample_instance_with_root("/home/user/.claude-isolated");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(!plan.env_vars.is_empty());
        let (key, val) = &plan.env_vars[0];
        assert_eq!(key, CONFIG_ENV_VAR);
        assert_eq!(val, "/home/user/.claude-isolated");
    }

    #[test]
    fn jsonc_stripping_allows_comments() {
        // Claude Code settings are JSONC-tolerant; ensure jsonc loader handles comments.
        let dir = std::env::temp_dir().join("superai-claude-jsonc-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.jsonc");
        let content = r#"
        {
            // Model selection
            "model": "sonnet", // default model
            /* provider block */
            "env": {
                "ANTHROPIC_BASE_URL": "https://example.com", // trailing comma,
            },
        }
        "#;
        std::fs::write(&path, content).unwrap();
        let map = superai_config::jsonc::load(&path).unwrap();
        assert_eq!(map["model"], serde_json::Value::String("sonnet".to_owned()));
        assert_eq!(
            map["env"]["ANTHROPIC_BASE_URL"],
            serde_json::Value::String("https://example.com".to_owned())
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
