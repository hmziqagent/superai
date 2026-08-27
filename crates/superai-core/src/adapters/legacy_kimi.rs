//! Legacy Kimi CLI adapter — TOML/MCP legacy root, `MigrationOnly`.
//!
//! Research source: `docs/harness-configs/kimi-cli.md` (last verified 2026-08-25).
//! Executable `kimi`, legacy config root `~/.kimi` with `config.toml` TOML and
//! `mcp.json`, isolation `relocated-root` (legacy), product status `retired`,
//! successor `kimi-code-cli` (`kimi` Node).

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

/// Harness identifier for Legacy Kimi CLI (canonical ledger id).
pub const HARNESS_ID_STR: &str = "legacy-kimi-cli";

/// Alias for task label `legacy-kimi`.
pub const HARNESS_ID_ALIAS: &str = "legacy-kimi";

/// Human display name.
pub const DISPLAY_NAME: &str = "Legacy Kimi CLI";

/// Primary executable name.
pub const EXECUTABLE: &str = "kimi";

/// Legacy config root fallback.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.kimi";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/kimi-cli.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Successor harness id.
pub const SUCCESSOR_ID: &str = "kimi-code-cli";

/// Successor executable (same name, different impl).
pub const SUCCESSOR_EXECUTABLE: &str = "kimi";

/// Migration tip.
pub const MIGRATION_TIP: &str = "Legacy Kimi CLI (Python, ~/.kimi/config.toml) is wound down; migrate via `kimi migrate` to kimi-code-cli (Kimi Code CLI ~/.kimi-code) — carries config.toml, MCP servers, history; OAuth and MCP authorizations not migrated";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Legacy Kimi CLI (`MigrationOnly`).
#[derive(Debug, Clone)]
pub struct LegacyKimiAdapter {
    id: HarnessId,
}

impl LegacyKimiAdapter {
    /// Create a new adapter.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    /// Create with alias id.
    pub fn from_alias() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_ALIAS)?;
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

    /// Migration tip.
    pub fn successor_tip(&self) -> &str {
        MIGRATION_TIP
    }

    /// Try to locate `kimi` binary via PATH.
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

    /// Probe `kimi --version` with timeout.
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

    /// Resolve default legacy root `~/.kimi`.
    fn default_config_root() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".kimi"))
    }

    /// Collect evidence.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!(
            "legacy product wound down, successor {SUCCESSOR_ID}"
        ));
        evidence.push(MIGRATION_TIP.to_owned());
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("legacy config root exists at {}", root.display()));
                    let config = root.join("config.toml");
                    if config.exists() {
                        evidence.push(format!("config.toml found at {}", config.display()));
                    } else {
                        evidence.push(format!("config.toml missing at {}", config.display()));
                    }
                    let mcp = root.join("mcp.json");
                    if mcp.exists() {
                        evidence.push(format!("mcp.json found at {}", mcp.display()));
                    }
                } else {
                    evidence.push(format!("legacy config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve home for legacy root".to_owned());
            }
        }
    }
}

impl Default for LegacyKimiAdapter {
    fn default() -> Self {
        #[expect(
            clippy::unwrap_used,
            reason = "legacy-kimi-cli is static valid HarnessId"
        )]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for LegacyKimiAdapter {
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
            notes.push(format!("detected legacy-kimi version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            notes.push(format!("successor {SUCCESSOR_ID}"));
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

        let config_resolver = PathResolver::new(
            Some("~/.kimi/config.toml"),
            Some("~/.kimi/config.toml"),
            Some("%USERPROFILE%\\.kimi\\config.toml"),
            "~/.kimi/config.toml",
        );
        let mut config_surface = ConfigSurface::new(
            "config.toml",
            config_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        config_surface.precedence = 10;
        config_surface.owned_selectors = vec![
            "default_model".to_owned(),
            "providers".to_owned(),
            "models".to_owned(),
        ];
        config_surface.backup_required = true;
        config_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(config_surface);

        let mcp_resolver = PathResolver::new(
            Some("~/.kimi/mcp.json"),
            Some("~/.kimi/mcp.json"),
            Some("%USERPROFILE%\\.kimi\\mcp.json"),
            "~/.kimi/mcp.json",
        );
        let mut mcp_surface = ConfigSurface::new(
            "mcp.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp_surface.precedence = 12;
        mcp_surface.owned_selectors = vec!["mcpServers".to_owned()];
        mcp_surface.backup_required = true;
        surfaces.push(mcp_surface);

        let tui_resolver = PathResolver::new(
            Some("~/.kimi/tui.toml"),
            Some("~/.kimi/tui.toml"),
            Some("%USERPROFILE%\\.kimi\\tui.toml"),
            "~/.kimi/tui.toml",
        );
        let mut tui_surface = ConfigSurface::new(
            "tui.toml",
            tui_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        tui_surface.precedence = 5;
        tui_surface.backup_required = true;
        surfaces.push(tui_surface);

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
            "~/.kimi/config.toml".to_owned(),
            "~/.kimi/mcp.json".to_owned(),
            "~/.kimi/tui.toml".to_owned(),
            "~/.kimi (legacy root)".to_owned(),
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
                    "legacy-kimi (MigrationOnly) expects isolation relocated_root, got {other} — {MIGRATION_TIP}"
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
        DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, LegacyKimiAdapter, RESEARCH_DOC, SUCCESSOR_ID,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> LegacyKimiAdapter {
        LegacyKimiAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-legacy-kimi-1").unwrap(),
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
        assert!(a.successor_tip().contains(SUCCESSOR_ID));
    }

    #[test]
    fn alias_is_valid() {
        let alias = HarnessId::new(super::HARNESS_ID_ALIAS).unwrap();
        assert_eq!(alias.as_str(), "legacy-kimi");
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
                .any(|e| e.contains("legacy") || e.contains("wound"))
        );
        assert!(result.evidence.iter().any(|e| e.contains(SUCCESSOR_ID)));
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
            ("kimi 0.21.0", Some("0.21.0")),
            ("0.10.0", Some("0.10.0")),
            ("v0.32.0", Some("0.32.0")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = LegacyKimiAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_toml_and_mcp() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 2);
        let config = surfaces
            .iter()
            .find(|s| s.id == "config.toml")
            .expect("config.toml must exist");
        assert_eq!(config.kind, DocumentKind::Toml);
        assert_eq!(config.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(config.scope, ConfigScope::User);
        assert!(config.backup_required);
        let mcp = surfaces
            .iter()
            .find(|s| s.id == "mcp.json")
            .expect("mcp.json must exist");
        assert_eq!(mcp.kind, DocumentKind::Json);
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
        let inst = sample_instance_with_root("/tmp/.kimi-work");
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::UnsupportedOperation { reason, .. } => {
                assert!(reason.contains(SUCCESSOR_ID) || reason.contains("MigrationOnly"));
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_legacy_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains(".kimi")));
        assert!(candidates.iter().any(|c| c.contains("config.toml")));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.kimi-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.kimi-work");
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
