//! Roo Code adapter — archived 2026-05, `MigrationOnly`, successor Kilo.
//!
//! Research source: `docs/harness-configs/roo-code.md` (last verified 2026-08-25).
//! VS Code extension `RooVeterinaryInc.roo-cline` with YAML `custom_modes.yaml`,
//! JSON `mcp_settings.json`, project `.roomodes` / `.roo/mcp.json`, isolation
//! `ide_user_data` via `--user-data-dir`, product status `archived`, successor
//! `kilo-code`.

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

/// Harness identifier for Roo Code.
pub const HARNESS_ID_STR: &str = "roo-code";

/// Human display name.
pub const DISPLAY_NAME: &str = "Roo Code";

/// Primary executable (VS Code).
pub const EXECUTABLE: &str = "code";

/// Extension identifier.
pub const EXTENSION_ID: &str = "RooVeterinaryInc.roo-cline";

/// Successor extension identifier.
pub const SUCCESSOR_EXTENSION: &str = "kilocode.Kilo-Code";

/// Successor harness id.
pub const SUCCESSOR_ID: &str = "kilo-code";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/roo-code.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Archive date.
pub const ARCHIVE_DATE: &str = "2026-05";

/// Migration tip.
pub const MIGRATION_TIP: &str = "Roo Code archived 2026-05; successor kilo-code (Kilo Code kilocode.Kilo-Code) via Migration Wizard or `code --install-extension kilocode.Kilo-Code` — map .roomodes/.roo/rules/custom_modes.yaml/mcp_settings.json -> .kilocode, .roo/mcp.json -> .kilocode/mcp.json, .roorules -> AGENTS.md";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Roo Code (`MigrationOnly`).
#[derive(Debug, Clone)]
pub struct RooCodeAdapter {
    id: HarnessId,
}

impl RooCodeAdapter {
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

    /// Migration tip.
    pub fn successor_tip(&self) -> &str {
        MIGRATION_TIP
    }

    /// Try to locate `code` binary via PATH.
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

    /// Probe `code --version` with timeout.
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

    /// Resolve default config storage dir (approx).
    fn default_storage_root() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        let base = PathBuf::from(home);
        // Linux: ~/.config/Code/User/globalStorage
        let candidates = [
            base.join(".config")
                .join("Code")
                .join("User")
                .join("globalStorage")
                .join("rooveterinaryinc.roo-cline"),
            base.join(".vscode").join("user-data"),
            base.join("Library")
                .join("Application Support")
                .join("Code")
                .join("User")
                .join("globalStorage")
                .join("rooveterinaryinc.roo-cline"),
        ];
        for c in candidates {
            if c.exists() {
                return Some(c);
            }
        }
        // Fallback to first candidate even if missing for evidence.
        Some(
            base.join(".config")
                .join("Code")
                .join("User")
                .join("globalStorage")
                .join("rooveterinaryinc.roo-cline"),
        )
    }

    /// Collect evidence.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!(
            "product archived {ARCHIVE_DATE}, successor {SUCCESSOR_ID} ({SUCCESSOR_EXTENSION})"
        ));
        evidence.push(MIGRATION_TIP.to_owned());
        evidence.push(format!("extension id {EXTENSION_ID}"));
        match Self::default_storage_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("storage dir exists at {}", root.display()));
                    let mcp = root.join("mcp_settings.json");
                    if mcp.exists() {
                        evidence.push(format!("mcp_settings.json found at {}", mcp.display()));
                    }
                    let modes = root.join("custom_modes.yaml");
                    if modes.exists() {
                        evidence.push(format!("custom_modes.yaml found at {}", modes.display()));
                    }
                } else {
                    evidence.push(format!("storage dir missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve home for storage lookup".to_owned());
            }
        }
        // Project-level evidence
        let proj_mcp = Path::new(".roo/mcp.json");
        if proj_mcp.exists() {
            evidence.push(format!("project mcp found at {}", proj_mcp.display()));
        }
        let roomodes = Path::new(".roomodes");
        if roomodes.exists() {
            evidence.push(format!(".roomodes found at {}", roomodes.display()));
        }
    }
}

impl Default for RooCodeAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "roo-code is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for RooCodeAdapter {
    fn id(&self) -> HarnessId {
        self.id.clone()
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    fn product_status(&self) -> ProductStatus {
        ProductStatus::Archived
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
            // Even without binary, storage existence gives low confidence.
            if evidence.iter().any(|e| e.contains("storage dir exists")) {
                DetectionConfidence::Low
            } else {
                DetectionConfidence::High
            }
        } else {
            DetectionConfidence::Medium
        };

        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution(&self) -> VersionResolution {
        let detection = self.detection();
        if let Some(v) = detection.version {
            let mut notes = Vec::new();
            notes.push(format!("detected roo-code via code version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            notes.push(format!("archived {ARCHIVE_DATE}, successor {SUCCESSOR_ID}"));
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

        let mcp_resolver = PathResolver::new(
            Some("~/.config/Code/User/globalStorage/rooveterinaryinc.roo-cline/mcp_settings.json"),
            Some(
                "~/Library/Application Support/Code/User/globalStorage/rooveterinaryinc.roo-cline/mcp_settings.json",
            ),
            Some(
                "%APPDATA%\\Code\\User\\globalStorage\\rooveterinaryinc.roo-cline\\mcp_settings.json",
            ),
            "VS Code globalStorage/rooveterinaryinc.roo-cline/mcp_settings.json",
        );
        let mut mcp_surface = ConfigSurface::new(
            "mcp_settings.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp_surface.precedence = 10;
        mcp_surface.owned_selectors = vec!["mcpServers".to_owned()];
        mcp_surface.backup_required = true;
        mcp_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(mcp_surface);

        let modes_resolver = PathResolver::new(
            Some("~/.config/Code/User/globalStorage/rooveterinaryinc.roo-cline/custom_modes.yaml"),
            Some(
                "~/Library/Application Support/Code/User/globalStorage/rooveterinaryinc.roo-cline/custom_modes.yaml",
            ),
            Some(
                "%APPDATA%\\Code\\User\\globalStorage\\rooveterinaryinc.roo-cline\\custom_modes.yaml",
            ),
            "VS Code globalStorage/rooveterinaryinc.roo-cline/custom_modes.yaml",
        );
        let mut modes_surface = ConfigSurface::new(
            "custom_modes.yaml",
            modes_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        modes_surface.precedence = 12;
        modes_surface.backup_required = true;
        surfaces.push(modes_surface);

        let project_mcp_resolver = PathResolver::fallback_only(".roo/mcp.json (project)");
        let mut project_mcp = ConfigSurface::new(
            ".roo/mcp.json",
            project_mcp_resolver,
            DocumentKind::Json,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_mcp.precedence = 15;
        project_mcp.owned_selectors = vec!["mcpServers".to_owned()];
        project_mcp.backup_required = true;
        surfaces.push(project_mcp);

        let roomodes_resolver = PathResolver::fallback_only(".roomodes (project YAML)");
        let mut roomodes_surface = ConfigSurface::new(
            ".roomodes",
            roomodes_resolver,
            DocumentKind::Yaml,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        roomodes_surface.precedence = 14;
        roomodes_surface.backup_required = true;
        surfaces.push(roomodes_surface);

        let vscode_settings_resolver = PathResolver::new(
            Some("~/.config/Code/User/settings.json (roo-cline.*)"),
            Some("~/Library/Application Support/Code/User/settings.json (roo-cline.*)"),
            Some("%APPDATA%\\Code\\User\\settings.json (roo-cline.*)"),
            "VS Code settings.json roo-cline.* keys",
        );
        let mut vscode_surface = ConfigSurface::new(
            "vscode-settings",
            vscode_settings_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        vscode_surface.precedence = 8;
        vscode_surface.owned_selectors = vec!["roo-cline.allowedCommands".to_owned()];
        vscode_surface.backup_required = true;
        surfaces.push(vscode_surface);

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
            "checkpoints/*".to_owned(),
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
                "MigrationOnly: {MIGRATION_TIP} — use VS Code --user-data-dir for isolation; no new defaults"
            ),
        })
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/Code/User/globalStorage/rooveterinaryinc.roo-cline/mcp_settings.json"
                .to_owned(),
            "~/.config/Code/User/globalStorage/rooveterinaryinc.roo-cline/custom_modes.yaml"
                .to_owned(),
            ".roo/mcp.json (project)".to_owned(),
            ".roomodes (project)".to_owned(),
            "VS Code settings.json roo-cline.*".to_owned(),
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
            Isolation::IdeUserData | Isolation::Unknown | Isolation::RelocatedRoot => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "roo-code (MigrationOnly) expects isolation ide_user_data, got {other} — {MIGRATION_TIP}"
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
        DISPLAY_NAME, EXECUTABLE, EXTENSION_ID, HARNESS_ID_STR, RESEARCH_DOC, RooCodeAdapter,
        SUCCESSOR_ID,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> RooCodeAdapter {
        RooCodeAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-roo-1").unwrap(),
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
        assert_eq!(a.product_status(), ProductStatus::Archived);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert!(a.successor_tip().contains(SUCCESSOR_ID));
        assert_eq!(EXTENSION_ID, "RooVeterinaryInc.roo-cline");
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
        assert!(result.evidence.iter().any(|e| e.contains("archived")));
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
            ("1.84.0", Some("1.84.0")),
            ("v1.2.3", Some("1.2.3")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = RooCodeAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_mcp_and_modes() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let mcp = surfaces
            .iter()
            .find(|s| s.id == "mcp_settings.json")
            .expect("mcp_settings.json must exist");
        assert_eq!(mcp.kind, DocumentKind::Json);
        assert_eq!(mcp.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(mcp.scope, ConfigScope::User);
        let modes = surfaces
            .iter()
            .find(|s| s.id == "custom_modes.yaml")
            .expect("custom_modes.yaml must exist");
        assert_eq!(modes.kind, DocumentKind::Yaml);
        let proj = surfaces
            .iter()
            .find(|s| s.id == ".roo/mcp.json")
            .expect(".roo/mcp.json must exist");
        assert_eq!(proj.kind, DocumentKind::Json);
        assert_eq!(proj.scope, ConfigScope::ProjectWorkspace);
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
        let inst = sample_instance_with_root("/tmp/.vscode-roo-work");
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::UnsupportedOperation { reason, .. } => {
                assert!(reason.contains(SUCCESSOR_ID) || reason.contains("MigrationOnly"));
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_roo_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("mcp_settings")));
        assert!(candidates.iter().any(|c| c.contains("custom_modes")));
    }

    #[test]
    fn validate_instance_accepts_ide_user_data() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.vscode-roo-work");
        a.validate_instance(&inst).unwrap();
        let mut inst2 = sample_instance_with_root("/tmp/.vscode-roo-work2");
        inst2.isolation = Isolation::RelocatedRoot;
        a.validate_instance(&inst2).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.vscode-roo-work");
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
