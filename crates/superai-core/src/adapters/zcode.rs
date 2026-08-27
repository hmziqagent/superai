//! `ZCode` adapter — fixed path `~/.zcode/v2/config.json`, `SingleInstance`.
//!
//! Research source: `docs/harness-configs/zcode.md` (last verified 2026-08-25).
//! Proprietary Electron app, config fixed at `~/.zcode/v2/config.json` (JSON,
//! versioned path `v2`), no documented relocation env var, isolation
//! `fixed_path_single` (single instance, GUI), product status `active`,
//! support `SingleInstance` read/single.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::adapter::{
    ADAPTER_REVISION, Adapter, Arch, ConfigScope, ConfigSurface, DetectionConfidence,
    DetectionResult, DocumentKind, Os, PathResolver, Platform, ProductStatus, RestartBehavior,
    SkillMode, SurfaceOwnership, VersionResolution, WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, InstallPresence, Isolation};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Harness identifier for `ZCode`.
pub const HARNESS_ID_STR: &str = "zcode";

/// Human display name.
pub const DISPLAY_NAME: &str = "ZCode";

/// Primary executable name (Electron app may not have CLI, but use zcode).
pub const EXECUTABLE: &str = "zcode";

/// Fixed config path (versioned).
pub const FIXED_CONFIG_PATH: &str = "~/.zcode/v2/config.json";

/// Fixed config root (parent of versioned file).
pub const FIXED_CONFIG_ROOT: &str = "~/.zcode/v2";

/// Application bundle ID hint (macOS).
pub const BUNDLE_ID: &str = "ai.zcode.app";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/zcode.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for `ZCode` (`SingleInstance`).
///
/// Config is at fixed `~/.zcode/v2/config.json`. No isolation env var; only
/// one instance can exist. Reads are single-instance; writes must mutate the
/// fixed path in place (backed up before every write). Wrappers cannot create
/// isolated copies.
#[derive(Debug, Clone)]
pub struct ZcodeAdapter {
    id: HarnessId,
}

impl ZcodeAdapter {
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

    /// Fixed path.
    pub fn fixed_path(&self) -> &str {
        FIXED_CONFIG_PATH
    }

    /// Try to locate `zcode` binary via PATH.
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

    /// Probe `zcode --version` with timeout.
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

    /// Resolve fixed config path `~/.zcode/v2/config.json`.
    fn fixed_config_path_buf() -> Option<PathBuf> {
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(
            PathBuf::from(home)
                .join(".zcode")
                .join("v2")
                .join("config.json"),
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
            "fixed path single instance at {FIXED_CONFIG_PATH} (versioned path v2, no relocation env var)"
        ));
        match Self::fixed_config_path_buf() {
            Some(path) => {
                if path.exists() {
                    evidence.push(format!("config exists at {}", path.display()));
                    if let Ok(text) = std::fs::read_to_string(&path)
                        && (text.contains("\"provider\"") || text.contains("\"options\""))
                    {
                        evidence.push("config contains provider/options marker".to_owned());
                    }
                } else {
                    evidence.push(format!("config missing at {}", path.display()));
                    // Check parent dir exists
                    if let Some(parent) = path.parent()
                        && parent.exists()
                    {
                        evidence.push(format!("parent dir exists at {}", parent.display()));
                    }
                }
            }
            None => {
                evidence.push("could not resolve home for fixed path".to_owned());
            }
        }
        // Check bundle hint on macOS
        if cfg!(target_os = "macos") {
            evidence.push(format!("bundle id hint {BUNDLE_ID}"));
        }
    }
}

impl Default for ZcodeAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "zcode is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for ZcodeAdapter {
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
            (None, _) => {
                // For fixed-path GUI, config existence alone counts as low confidence present
                if evidence.iter().any(|e| e.contains("config exists")) {
                    InstallPresence::Present
                } else {
                    InstallPresence::Absent
                }
            }
        };

        // Version for GUI app may be unknown; treat config existence as low confidence.
        let confidence = match (
            &binary_path,
            evidence.iter().any(|e| e.contains("config exists")),
        ) {
            (Some(_), false) => DetectionConfidence::Medium,
            (None, true) => DetectionConfidence::Low,
            (Some(_), true) | (None, false) => DetectionConfidence::High,
        };

        // If absent (no binary and no config), confidence high.
        let version_for_result = version;

        DetectionResult::new(present, version_for_result, evidence, confidence)
    }

    fn version_resolution(&self) -> VersionResolution {
        let detection = self.detection();
        if let Some(v) = detection.version.clone() {
            let mut notes = Vec::new();
            notes.push(format!("detected zcode version {v}"));
            notes.push(format!(
                "mapped to schema version {SCHEMA_VERSION_STR} (fixed path v2)"
            ));
            let mut res =
                VersionResolution::new(Some(v), Some(SCHEMA_VERSION_STR.to_owned()), true);
            res.notes = notes;
            res
        } else if detection.present == InstallPresence::Present {
            // Config exists but version unknown — still compatible via fixed-path schema
            let mut res = VersionResolution::new(None, Some(SCHEMA_VERSION_STR.to_owned()), true);
            res.notes = detection.evidence;
            res.notes.push(format!(
                "fixed path {FIXED_CONFIG_PATH} schema {SCHEMA_VERSION_STR}"
            ));
            res
        } else {
            let mut res = VersionResolution::unknown();
            res.notes = detection.evidence;
            res
        }
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let config_resolver = PathResolver::new(
            Some("~/.zcode/v2/config.json"),
            Some("~/.zcode/v2/config.json"),
            Some("%USERPROFILE%\\.zcode\\v2\\config.json"),
            "~/.zcode/v2/config.json",
        );
        let mut config_surface = ConfigSurface::new(
            "config.json",
            config_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        config_surface.precedence = 10;
        config_surface.owned_selectors = vec![
            "provider".to_owned(),
            "options".to_owned(),
            "models".to_owned(),
        ];
        config_surface.backup_required = true;
        config_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(config_surface);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        vec![
            ("detect".to_owned(), AdapterSupport::SingleInstance),
            ("read_config".to_owned(), AdapterSupport::SingleInstance),
            ("write_config".to_owned(), AdapterSupport::SingleInstance),
            ("manage_skills".to_owned(), AdapterSupport::SingleInstance),
            ("manage_mcp".to_owned(), AdapterSupport::SingleInstance),
            ("manage_plugins".to_owned(), AdapterSupport::SingleInstance),
            (
                "configure_provider".to_owned(),
                AdapterSupport::SingleInstance,
            ),
            ("plan_mirror".to_owned(), AdapterSupport::SingleInstance),
            ("plan_wrapper".to_owned(), AdapterSupport::SingleInstance),
            ("scan_candidates".to_owned(), AdapterSupport::SingleInstance),
            (
                "validate_instance".to_owned(),
                AdapterSupport::SingleInstance,
            ),
        ]
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "history/*".to_owned(),
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
        instance.validate()?;
        // Fixed path: no relocation; wrapper is identity (single instance).
        let mut plan = WrapperPlan::new(
            "fixed path single instance — no isolation, writes to ~/.zcode/v2/config.json in place",
        );
        // No env vars; the harness always reads the fixed path.
        plan.description = format!(
            " single instance at {FIXED_CONFIG_PATH}; wrapper is no-op (config_root {} is informative, not used for isolation)",
            instance.config_root
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![FIXED_CONFIG_PATH.to_owned()]
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
            Isolation::FixedPathSingle | Isolation::Unknown | Isolation::RelocatedRoot => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "zcode requires isolation fixed_path_single (fixed path {FIXED_CONFIG_PATH}), got {other}"
                ),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<SkillMode> {
        vec![SkillMode::CopySelected]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        DISPLAY_NAME, EXECUTABLE, FIXED_CONFIG_PATH, HARNESS_ID_STR, RESEARCH_DOC, ZcodeAdapter,
    };
    use crate::adapter::{
        Adapter, ConfigScope, DocumentKind, ProductStatus, SkillMode, SurfaceOwnership,
    };
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> ZcodeAdapter {
        ZcodeAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-zcode-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::FixedPathSingle,
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
        assert_eq!(a.fixed_path(), FIXED_CONFIG_PATH);
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
    fn detection_returns_evidence_with_fixed_path() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        assert!(result.evidence.iter().any(|e| e.contains("fixed path")));
        assert!(result.evidence.iter().any(|e| e.contains(".zcode")));
    }

    #[test]
    fn version_resolution_handles_fixed_path() {
        let a = adapter();
        let res = a.version_resolution();
        assert!(!res.notes.is_empty());
        // Should have schema version if present or unknown otherwise, but notes non-empty.
        if res.detected_version.is_some() {
            assert_eq!(
                res.schema_version.as_deref(),
                Some(super::SCHEMA_VERSION_STR)
            );
            assert!(res.compatible);
        }
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("zcode 0.9.0", Some("0.9.0")),
            ("1.0.0", Some("1.0.0")),
            ("v1.2.3", Some("1.2.3")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = ZcodeAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_fixed_config() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert_eq!(surfaces.len(), 1);
        let cfg = surfaces
            .iter()
            .find(|s| s.id == "config.json")
            .expect("config.json must exist");
        assert_eq!(cfg.kind, DocumentKind::Json);
        assert_eq!(cfg.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(cfg.scope, ConfigScope::User);
        assert!(cfg.backup_required);
        assert_eq!(cfg.path_resolver.fallback, FIXED_CONFIG_PATH);
    }

    #[test]
    fn supported_operations_are_single_instance() {
        let a = adapter();
        let ops = a.supported_operations();
        for (_, support) in ops {
            assert_eq!(support, AdapterSupport::SingleInstance);
        }
    }

    #[test]
    fn plan_wrapper_succeeds_with_no_env() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.zcode-work");
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(plan.env_vars.is_empty());
        assert!(plan.description.contains("single instance"));
        assert!(plan.description.contains(FIXED_CONFIG_PATH));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.zcode-work");
        inst.harness = HarnessId::new("codex-cli").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            crate::error::CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_is_fixed_path_only() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], FIXED_CONFIG_PATH);
    }

    #[test]
    fn validate_instance_accepts_fixed_path() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.zcode-work");
        a.validate_instance(&inst).unwrap();
        let mut inst2 = sample_instance_with_root("/tmp/.zcode-work2");
        inst2.isolation = Isolation::RelocatedRoot;
        a.validate_instance(&inst2).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.zcode-work");
        inst.isolation = Isolation::EnvOnly;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            crate::error::CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn supported_skill_modes_is_copy_selected_only() {
        let a = adapter();
        let modes = a.supported_skill_modes();
        assert_eq!(modes, vec![SkillMode::CopySelected]);
    }
}
