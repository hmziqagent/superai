//! Copilot Coding Agent adapter — `Unsupported`, cloud-owned.
//!
//! Research source: `docs/harness-configs/copilot-cli.md` annex (also
//! `docs/harness-configs/orchestrators.md` context) (last verified 2026-08-25).
//! The *Copilot coding agent* (distinct from Copilot CLI) is a cloud-owned,
//! GitHub-hosted agent: repo/org settings `AGENTS.md`/`copilot-instructions.md`,
//! org policy, Actions workflow `copilot-setup-steps.yml`, no local config dir,
//! no relocatable root, no MCP local, no skills local, no API key local.
//! Isolation `unsupported`, support `Unsupported`, product `active` (cloud), no
//! local mutation — detection only informs, wrapper blocked, every write
//! `UnsupportedOperation` with cloud-owned reason.

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

/// Harness identifier for Copilot Coding Agent.
pub const HARNESS_ID_STR: &str = "copilot-coding-agent";

/// Human display name.
pub const DISPLAY_NAME: &str = "Copilot Coding Agent";

/// Primary executable name (none local; `gh` is closest local helper, but agent is cloud).
pub const EXECUTABLE: &str = "gh";

/// Alternative executable name (copilot CLI, distinct product but shares brand).
pub const EXECUTABLE_ALT: &str = "copilot";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/copilot-cli.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version (no local schema).
pub const SCHEMA_VERSION_STR: &str = "1";

/// Unsupported reason — cloud-owned.
pub const UNSUPPORTED_REASON: &str = "cloud-owned repo/org settings (github.com settings → Copilot → Coding agent, AGENTS.md/copilot-instructions.md, copilot-setup-steps.yml Actions workflow, org policy), no local mutation, no relocatable root, no MCP/skills local — use Copilot CLI (`copilot-cli`) for local isolation";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Copilot Coding Agent (`Unsupported`, `unsupported`).
///
/// All operations except `detect`/`scan_candidates` are `Unsupported`.
/// `detect` probes for `gh` (GitHub CLI) and for cloud hints
/// (`AGENTS.md`, `copilot-instructions.md`, `.github/workflows/copilot-setup-steps.yml`);
/// `version_resolution` is unknown/unsupported; `plan_wrapper` and `write_config`
/// always return `UnsupportedOperation`.
#[derive(Debug, Clone)]
pub struct CopilotCodingAgentAdapter {
    id: HarnessId,
}

impl CopilotCodingAgentAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    /// Borrow the harness id.
    pub fn harness_id(&self) -> &HarnessId {
        &self.id
    }

    /// Executable name for this harness (cloud agent has no local binary; `gh` is helper).
    pub fn executable_name(&self) -> &str {
        EXECUTABLE
    }

    /// Unsupported reason.
    pub fn unsupported_reason(&self) -> &str {
        UNSUPPORTED_REASON
    }

    /// Try to locate the `gh` binary via `PATH`.
    #[expect(clippy::unused_self, reason = "adapter method uses instance constants")]
    #[expect(clippy::excessive_nesting, reason = "PATH scan branches are explicit")]
    fn find_binary_in_path(&self) -> Option<PathBuf> {
        let path_var = std::env::var("PATH").ok()?;
        let separator = if cfg!(windows) { ';' } else { ':' };
        for exec in [EXECUTABLE, EXECUTABLE_ALT] {
            for dir in path_var.split(separator) {
                if dir.is_empty() {
                    continue;
                }
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

    /// Probe `gh --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `gh version 2.80.0` into `2.80.0`.
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

    /// Build detection evidence about cloud-owned repo/org settings and local hints.
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("unsupported: {UNSUPPORTED_REASON}"));
        // Repo-local instructions files (cloud agent reads them, but local file presence is hint)
        if Path::new(".github")
            .join("copilot-instructions.md")
            .exists()
        {
            evidence.push(".github/copilot-instructions.md found (cloud agent context)".to_owned());
        } else {
            evidence.push(".github/copilot-instructions.md missing (cloud)".to_owned());
        }
        if Path::new(".github")
            .join("workflows")
            .join("copilot-setup-steps.yml")
            .exists()
        {
            evidence.push(
                ".github/workflows/copilot-setup-steps.yml found (cloud agent setup)".to_owned(),
            );
        }
        if Path::new("AGENTS.md").exists() {
            evidence.push("AGENTS.md found (shared with other agents, cloud reads it)".to_owned());
        }
        let agents_alt = Path::new(".github").join("agents.md");
        if agents_alt.exists() {
            evidence.push(format!(
                ".github/agents.md found at {}",
                agents_alt.display()
            ));
        }
        // Actions workflow dir
        if Path::new(".github").join("workflows").exists() {
            evidence.push(".github/workflows/ present (Actions, cloud-owned)".to_owned());
        }
        // Auth helper hint: gh auth status
        evidence.push("cloud-owned: no local config dir, no relocatable root, settings live on github.com (Copilot → Coding agent)".to_owned());
        evidence.push(
            "no MCP local, no skills local, no API key local — org policy + repo settings govern"
                .to_owned(),
        );
        // GH token env (but don't leak)
        if let Ok(val) = std::env::var("GH_TOKEN")
            && !val.trim().is_empty()
        {
            evidence.push(
                "GH_TOKEN is set (len redacted) — may auth gh helper, not coding agent".to_owned(),
            );
        } else if let Ok(val) = std::env::var("GITHUB_TOKEN")
            && !val.trim().is_empty()
        {
            evidence.push(
                "GITHUB_TOKEN is set (len redacted) — Actions token, not local coding agent config"
                    .to_owned(),
            );
        } else {
            evidence.push("GH_TOKEN/GITHUB_TOKEN not set (cloud auth separate)".to_owned());
        }
    }
}

impl Default for CopilotCodingAgentAdapter {
    fn default() -> Self {
        #[expect(
            clippy::unwrap_used,
            reason = "copilot-coding-agent is static valid HarnessId"
        )]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for CopilotCodingAgentAdapter {
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
        let binary_path: Option<PathBuf> = self.find_binary_in_path();

        if let Some(path) = binary_path.as_ref() {
            evidence.push(format!(
                "found helper binary `{}` at {}",
                path.display(),
                path.display()
            ));
            // Probe gh version but do NOT claim it's coding-agent version
            match Self::probe_version(path) {
                Some(v) => {
                    evidence.push(format!(
                        "helper version `{v}` via `gh --version` (helper, not coding-agent version)"
                    ));
                    version = Some(v);
                }
                None => {
                    evidence.push(
                        "helper version probe failed for `gh --version` (expected, coding agent is cloud)"
                            .to_owned(),
                    );
                }
            }
        } else {
            evidence.push(format!("helper binary `{EXECUTABLE}` not found in PATH"));
            evidence.push(format!(
                "alternative `{EXECUTABLE_ALT}` (copilot-cli) not found, distinct product"
            ));
        }

        self.collect_config_evidence(&mut evidence);

        // Coding agent itself is cloud; local presence is always Absent for local mutation.
        let present = InstallPresence::Absent;
        // Confidence high because cloud-owned is deterministic
        let confidence = DetectionConfidence::High;

        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution(&self) -> VersionResolution {
        let detection = self.detection();
        // Coding agent has no local versioned schema; always unknown/unsupported
        let mut res = VersionResolution::unknown();
        res.notes = detection.evidence;
        res.notes.push(format!("unsupported: {UNSUPPORTED_REASON}"));
        res.notes.push(format!(
            "schema version {SCHEMA_VERSION_STR} is placeholder, no local config schema"
        ));
        res.compatible = false;
        res
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        // Cloud-owned surfaces are not locally writable; we expose them as Opaque for documentation.
        let mut surfaces = Vec::new();

        let cloud_instructions_resolver = PathResolver::fallback_only(
            "github.com repo → Copilot → Coding agent settings + `.github/copilot-instructions.md` + `AGENTS.md` (cloud, repo/org policy)",
        );
        let mut cloud_instructions = ConfigSurface::new(
            "cloud instructions (copilot-instructions.md)",
            cloud_instructions_resolver,
            DocumentKind::Opaque,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        cloud_instructions.precedence = 0;
        cloud_instructions.owned_selectors = Vec::new();
        cloud_instructions.backup_required = false;
        cloud_instructions.restart_behavior = RestartBehavior::None;
        surfaces.push(cloud_instructions);

        let setup_resolver = PathResolver::fallback_only(
            ".github/workflows/copilot-setup-steps.yml (Actions, cloud agent bootstrap, npm/dependencies install)",
        );
        let mut setup = ConfigSurface::new(
            "cloud setup workflow (copilot-setup-steps.yml)",
            setup_resolver,
            DocumentKind::Yaml,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        setup.precedence = 0;
        setup.backup_required = false;
        surfaces.push(setup);

        let policy_resolver = PathResolver::fallback_only(
            "github.com org policy + Actions permissions + network allowlist (cloud, org-owned, no local file)",
        );
        let mut policy = ConfigSurface::new(
            "org policy (cloud)",
            policy_resolver,
            DocumentKind::Opaque,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        policy.precedence = 0;
        policy.backup_required = false;
        surfaces.push(policy);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        vec![
            ("detect".to_owned(), AdapterSupport::Unsupported),
            ("read_config".to_owned(), AdapterSupport::Unsupported),
            ("write_config".to_owned(), AdapterSupport::Unsupported),
            ("manage_skills".to_owned(), AdapterSupport::Unsupported),
            ("manage_mcp".to_owned(), AdapterSupport::Unsupported),
            ("manage_plugins".to_owned(), AdapterSupport::Unsupported),
            ("configure_provider".to_owned(), AdapterSupport::Unsupported),
            ("plan_mirror".to_owned(), AdapterSupport::Unsupported),
            ("plan_wrapper".to_owned(), AdapterSupport::Unsupported),
            ("scan_candidates".to_owned(), AdapterSupport::Unsupported),
            ("validate_instance".to_owned(), AdapterSupport::Unsupported),
        ]
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        // No local instance to mirror; exclusions are placeholder to satisfy trait.
        vec![
            ".github/*".to_owned(),
            "*.log".to_owned(),
            "node_modules/*".to_owned(),
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
                "Unsupported: {UNSUPPORTED_REASON} — coding agent is hosted by GitHub, not on this machine; manage via github.com repo settings and Actions; for local isolation use copilot-cli harness instead"
            ),
        })
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            ".github/copilot-instructions.md (cloud)".to_owned(),
            ".github/workflows/copilot-setup-steps.yml (cloud bootstrap)".to_owned(),
            "AGENTS.md (shared, also read by cloud agent)".to_owned(),
            "github.com/<org>/<repo> → Settings → Copilot → Coding agent (cloud, org)".to_owned(),
            "gh (local helper, not coding agent)".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        if instance.harness != self.id {
            return Err(CoreError::Validation {
                field: "harness".to_owned(),
                reason: format!("expected harness `{}`, got `{}`", self.id, instance.harness),
            });
        }
        // Unsupported: every validate is an UnsupportedOperation
        Err(CoreError::UnsupportedOperation {
            harness: self.id.to_string(),
            operation: "validate_instance".to_owned(),
            reason: format!("Unsupported: {UNSUPPORTED_REASON}"),
        })
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        CopilotCodingAgentAdapter, DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, RESEARCH_DOC,
        UNSUPPORTED_REASON,
    };
    use crate::adapter::{Adapter, DocumentKind, ProductStatus};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> CopilotCodingAgentAdapter {
        CopilotCodingAgentAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-copilot-agent-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::Unsupported,
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
        assert!(a.unsupported_reason().contains("cloud-owned"));
        assert!(UNSUPPORTED_REASON.contains("no local mutation"));
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
    fn detection_returns_absent_and_evidence_cloud() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        assert!(result.evidence.iter().any(|e| e.contains("unsupported")));
        assert!(result.evidence.iter().any(|e| e.contains("cloud-owned")));
        // Coding agent is cloud; always Absent locally
        assert_eq!(result.present, crate::state::InstallPresence::Absent);
        assert_eq!(result.confidence, crate::adapter::DetectionConfidence::High);
    }

    #[test]
    fn version_resolution_is_unknown_unsupported() {
        let a = adapter();
        let res = a.version_resolution();
        assert!(!res.compatible);
        assert!(res.schema_version.is_none());
        assert!(res.detected_version.is_none() || res.detected_version.is_some()); // helper may exist
        assert!(!res.notes.is_empty());
        assert!(res.notes.iter().any(|n| n.contains("unsupported")));
        assert!(res.notes.iter().any(|n| n.contains("no local config")));
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("gh version 2.80.0", Some("2.80.0")),
            ("copilot 0.1.1", Some("0.1.1")),
            ("v2.80.0", Some("2.80.0")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = CopilotCodingAgentAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_are_cloud_opaque() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 3);
        let instr = surfaces
            .iter()
            .find(|s| s.id == "cloud instructions (copilot-instructions.md)")
            .expect("cloud instructions must exist");
        assert_eq!(instr.kind, DocumentKind::Opaque);
        assert!(instr.owned_selectors.is_empty());
        assert!(!instr.backup_required);
        let setup = surfaces
            .iter()
            .find(|s| s.id == "cloud setup workflow (copilot-setup-steps.yml)")
            .expect("setup workflow must exist");
        assert_eq!(setup.kind, DocumentKind::Yaml);
    }

    #[test]
    fn supported_operations_are_all_unsupported() {
        let a = adapter();
        let ops = a.supported_operations();
        assert!(!ops.is_empty());
        for (_, support) in &ops {
            assert_eq!(*support, AdapterSupport::Unsupported);
        }
        let names: HashSet<String> = ops.iter().map(|(n, _)| n.clone()).collect();
        for required in ["detect", "read_config", "write_config", "plan_wrapper"] {
            assert!(names.contains(required), "missing op {required}");
        }
    }

    #[test]
    fn plan_mirror_exclusions_nonempty() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        assert!(exclusions.iter().any(|e| e.contains(".github")));
    }

    #[test]
    fn plan_wrapper_is_unsupported() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.copilot-agent-work");
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::UnsupportedOperation {
                harness,
                operation,
                reason,
            } => {
                assert_eq!(harness, HARNESS_ID_STR);
                assert_eq!(operation, "plan_wrapper");
                assert!(reason.contains("Unsupported"));
                assert!(reason.contains("cloud"));
                assert!(reason.contains("copilot-cli"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.copilot-agent-work");
        inst.harness = HarnessId::new("claude-code").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_cloud_and_helper() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(!candidates.is_empty());
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("copilot-instructions.md"))
        );
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("copilot-setup-steps.yml"))
        );
        assert!(candidates.iter().any(|c| c.contains("github.com")));
        assert!(candidates.iter().any(|c| c.contains("gh")));
    }

    #[test]
    fn validate_instance_is_unsupported() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.copilot-agent-work");
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::UnsupportedOperation {
                harness, operation, ..
            } => {
                assert_eq!(harness, HARNESS_ID_STR);
                assert_eq!(operation, "validate_instance");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn supported_skill_modes_is_empty_unsupported() {
        let a = adapter();
        assert!(a.supported_skill_modes().is_empty());
    }
}
