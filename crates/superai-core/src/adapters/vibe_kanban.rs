//! Vibe Kanban adapter — orchestrator profiles/env/MCP/worktrees, `MigrationOnly`.
//!
//! Research source: `docs/harness-configs/orchestrators.md` (last verified 2026-08-25).
//! Executable `vibe-kanban` (`npx vibe-kanban`), Tauri desktop or headless, ten
//! harnesses (claude-code/codex/copilot-cli/gemini-cli/amp/cursor/opencode/droid/ccr/qwen-code),
//! per-agent reusable profiles (plan/model/sandbox), env injection overriding shell,
//! MCP `{"mcpServers":…}` written into each harness's own global config, worktrees
//! under `.vibe-kanban-workspaces/` (configurable) with branch `vk/*`, setup/run/
//! cleanup scripts per repo, sunsetting → community-maintained Apache-2.0 (v0.1.44),
//! isolation `project_scope`, support `MigrationOnly`, product `sunset`.

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

/// Harness identifier for Vibe Kanban.
pub const HARNESS_ID_STR: &str = "vibe-kanban";

/// Human display name.
pub const DISPLAY_NAME: &str = "Vibe Kanban";

/// Primary executable name.
pub const EXECUTABLE: &str = "vibe-kanban";

/// Alternative binary name (via npx).
pub const EXECUTABLE_ALT: &str = "vk";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/orchestrators.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Migration tip — sunsetting → community maintained.
pub const MIGRATION_TIP: &str = "Vibe Kanban sunsetting as company product, continuing as community-maintained OSS (Apache-2.0, v0.1.44, github.com/BloopAI/vibe-kanban): orchestrator profiles/env/MCP/worktrees — export agent profiles (env ANTHROPIC_BASE_URL/ANTHROPIC_AUTH_TOKEN overrides), MCP JSON written into harness global configs, .vibe-kanban-workspaces/ worktrees, migrate to conductor/sculptor or direct harness usage";

/// Community maintained flag.
pub const COMMUNITY_MAINTAINED: &str = "community-maintained OSS (Apache-2.0)";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for Vibe Kanban (`MigrationOnly`, `project_scope`).
///
/// Vibe Kanban is not an agent but an orchestrator GUI that spawns ten
/// harnesses in git worktrees. `MigrationOnly` means only detect/inspect/
/// backup/export are supported; no new instances, no wrapper.
#[derive(Debug, Clone)]
pub struct VibeKanbanAdapter {
    id: HarnessId,
}

impl VibeKanbanAdapter {
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

    /// Migration tip.
    pub fn migration_tip(&self) -> &str {
        MIGRATION_TIP
    }

    /// Try to locate the `vibe-kanban` binary via `PATH`.
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

    /// Probe `vibe-kanban --version` with a timeout, returning the parsed version string if successful.
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

    /// Parse version output like `vibe-kanban 0.1.44` into `0.1.44`.
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

    /// Resolve workspaces dir heuristic (repo-local `.vibe-kanban-workspaces` or `~/vibe-kanban-workspaces`).
    fn workspaces_dir() -> Option<PathBuf> {
        let cwd_ws = Path::new(".vibe-kanban-workspaces");
        if cwd_ws.exists() {
            return Some(cwd_ws.to_path_buf());
        }
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".vibe-kanban-workspaces"))
    }

    /// Build detection evidence about binary, worktrees, profiles, and MCP.
    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("sunset → {COMMUNITY_MAINTAINED}, MigrationOnly"));
        evidence.push(MIGRATION_TIP.to_owned());
        match Self::workspaces_dir() {
            Some(dir) => {
                if dir.exists() {
                    evidence.push(format!("workspaces dir exists at {}", dir.display()));
                    // Count branches hint
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        let count = entries.count();
                        evidence.push(format!("workspaces dir contains {count} entries"));
                    }
                } else {
                    evidence.push(format!("workspaces dir missing at {}", dir.display()));
                }
            }
            None => evidence.push("could not resolve workspaces dir (no HOME/cwd)".to_owned()),
        }
        if Path::new(".vibe-kanban").exists() || Path::new(".vibe-kanban-workspaces").exists() {
            evidence.push("repo-local .vibe-kanban* present".to_owned());
        }
        // Agent profiles hint — not a file but documented in orchestrator
        evidence.push("agent profiles: claude-code/codex/gemini-cli etc with env ANTHROPIC_BASE_URL/AUTH_TOKEN, mcpServers written into harness global configs".to_owned());
        // Server env vars
        for var in ["PORT", "HOST", "MCP_HOST", "MCP_PORT", "VK_ALLOWED_ORIGINS"] {
            if let Ok(val) = std::env::var(var)
                && !val.trim().is_empty()
            {
                evidence.push(format!("{var} set to {val}"));
            } else {
                evidence.push(format!("{var} not set"));
            }
        }
        evidence.push("ten harnesses: claude-code, codex, copilot-cli, gemini-cli, amp, cursor, opencode, droid, ccr, qwen-code — all must be pre-installed on PATH".to_owned());
    }
}

impl Default for VibeKanbanAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "vibe-kanban is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for VibeKanbanAdapter {
    fn id(&self) -> HarnessId {
        self.id.clone()
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    fn product_status(&self) -> ProductStatus {
        ProductStatus::Sunset
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

        if let Some(path) = self.find_binary_in_path() {
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
        } else {
            evidence.push(format!("binary `{EXECUTABLE}` not found in PATH"));
            evidence.push("try `npx vibe-kanban --version` for npx entrypoint".to_owned());
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
            notes.push(format!("detected vibe-kanban version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            notes.push(format!("sunset → {COMMUNITY_MAINTAINED}"));
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

        let workspaces_resolver = PathResolver::new(
            Some(".vibe-kanban-workspaces/<vk-*>/ (git worktree per workspace, configurable)"),
            Some(".vibe-kanban-workspaces/<vk-*>/ (git worktree)"),
            Some(".vibe-kanban-workspaces\\<vk-*>\\ (worktree)"),
            ".vibe-kanban-workspaces/ (configurable via Settings → General → Workspace Directory, branch vk/*)",
        );
        let mut workspaces = ConfigSurface::new(
            "worktrees (.vibe-kanban-workspaces)",
            workspaces_resolver,
            DocumentKind::Opaque,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::HarnessManaged,
        );
        workspaces.precedence = 12;
        workspaces.backup_required = false;
        workspaces.restart_behavior = RestartBehavior::None;
        surfaces.push(workspaces);

        let profiles_resolver = PathResolver::fallback_only(
            "agent profiles JSON (per-agent reusable: plan/sandbox/model/provider, env ANTHROPIC_BASE_URL overrides)",
        );
        let mut profiles = ConfigSurface::new(
            "agent profiles",
            profiles_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        profiles.precedence = 10;
        profiles.owned_selectors = vec![
            "agent".to_owned(),
            "environmentVariables".to_owned(),
            "model".to_owned(),
            "sandbox".to_owned(),
        ];
        profiles.backup_required = true;
        surfaces.push(profiles);

        let mcp_resolver = PathResolver::fallback_only(
            "MCP `{\"mcpServers\":{…}}` per-agent, written into harness global config (persists outside VK)",
        );
        let mut mcp = ConfigSurface::new(
            "mcpServers (per-agent, harness-global)",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::HarnessManaged,
        );
        mcp.precedence = 11;
        mcp.owned_selectors = vec!["mcpServers".to_owned()];
        mcp.backup_required = true;
        surfaces.push(mcp);

        let project_resolver = PathResolver::fallback_only(
            ".vibe-kanban / project settings (dev-server/setup/cleanup scripts, parallel setup toggle)",
        );
        let mut project = ConfigSurface::new(
            "project settings",
            project_resolver,
            DocumentKind::TextFragment,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project.precedence = 14;
        project.backup_required = false;
        surfaces.push(project);

        let remote_resolver = PathResolver::fallback_only(
            "remote-access via cloud.vibekanban.com pairing, env VIBEKANBAN_REMOTE_JWT_SECRET / VK_ALLOWED_ORIGINS",
        );
        let mut remote = ConfigSurface::new(
            "remote access (cloud pairing)",
            remote_resolver,
            DocumentKind::Opaque,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        remote.precedence = 0;
        remote.backup_required = false;
        surfaces.push(remote);

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
            ".vibe-kanban-workspaces/*".to_owned(),
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "tmp/*".to_owned(),
            "*.lock".to_owned(),
            "sessions/*".to_owned(),
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
            ".vibe-kanban-workspaces".to_owned(),
            ".vibe-kanban-workspaces/vk-* (worktree branch)".to_owned(),
            ".vibe-kanban (project)".to_owned(),
            "~/.vibe-kanban-workspaces (global workspaces)".to_owned(),
            "$VK_ALLOWED_ORIGINS / $VIBEKANBAN_REMOTE_JWT_SECRET (remote)".to_owned(),
            "npx vibe-kanban entrypoint".to_owned(),
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
            Isolation::ProjectScope | Isolation::Unknown | Isolation::RelocatedRoot => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "vibe-kanban (MigrationOnly) expects isolation project_scope (git worktree), got {other} — {MIGRATION_TIP}"
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
        DISPLAY_NAME, EXECUTABLE, HARNESS_ID_STR, MIGRATION_TIP, RESEARCH_DOC, VibeKanbanAdapter,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> VibeKanbanAdapter {
        VibeKanbanAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-vibe-kanban-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::ProjectScope,
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
        assert_eq!(a.product_status(), ProductStatus::Sunset);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert_eq!(a.adapter_revision(), crate::adapter::ADAPTER_REVISION);
        assert!(a.migration_tip().contains("community"));
        assert!(MIGRATION_TIP.contains("worktrees"));
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
        assert!(result.evidence.iter().any(|e| e.contains("sunset")));
        assert!(
            result
                .evidence
                .iter()
                .any(|e| e.contains("MigrationOnly") || e.contains("community"))
        );
        match result.present {
            InstallPresence::Absent => assert!(result.version.is_none()),
            InstallPresence::Present => assert!(result.version.is_some()),
            InstallPresence::UnknownVersion => {
                assert!(result.evidence.iter().any(|e| e.contains("found binary")));
            }
            InstallPresence::Broken => assert!(!result.evidence.is_empty()),
        }
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
            assert!(res.notes.iter().any(|n| n.contains("vibe-kanban")));
        } else {
            assert!(!res.compatible);
            assert!(res.schema_version.is_none());
            assert!(
                res.notes
                    .iter()
                    .any(|n| n.contains("MigrationOnly") || n.contains("migration tip"))
            );
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("vibe-kanban 0.1.44", Some("0.1.44")),
            ("0.1.44", Some("0.1.44")),
            ("v1.0.0", Some("1.0.0")),
            ("Version: 0.1.44", Some("0.1.44")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = VibeKanbanAdapter::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    #[test]
    fn config_surfaces_include_worktrees_and_profiles() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let worktrees = surfaces
            .iter()
            .find(|s| s.id == "worktrees (.vibe-kanban-workspaces)")
            .expect("worktrees must exist");
        assert_eq!(worktrees.kind, DocumentKind::Opaque);
        assert_eq!(worktrees.scope, ConfigScope::ProjectWorkspace);
        let profiles = surfaces
            .iter()
            .find(|s| s.id == "agent profiles")
            .expect("agent profiles must exist");
        assert_eq!(profiles.kind, DocumentKind::Json);
        let mcp = surfaces
            .iter()
            .find(|s| s.id == "mcpServers (per-agent, harness-global)")
            .expect("mcp must exist");
        assert!(mcp.owned_selectors.contains(&"mcpServers".to_owned()));
    }

    #[test]
    fn supported_operations_are_migration_only() {
        let a = adapter();
        let ops = a.supported_operations();
        assert!(!ops.is_empty());
        let map: std::collections::HashMap<String, AdapterSupport> = ops.into_iter().collect();
        assert_eq!(map.get("detect"), Some(&AdapterSupport::MigrationOnly));
        assert_eq!(map.get("plan_wrapper"), Some(&AdapterSupport::Unsupported));
        assert_eq!(map.get("write_config"), Some(&AdapterSupport::Unsupported));
        assert_eq!(map.get("backup"), Some(&AdapterSupport::MigrationOnly));
    }

    #[test]
    fn plan_mirror_exclusions_cover_workspaces_and_logs() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        for pat in [".vibe-kanban-workspaces/*", "cache/*", "*.lock"] {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"agent profiles".to_owned()));
    }

    #[test]
    fn plan_wrapper_is_blocked_migration_only() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.vibe-kanban-work");
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::UnsupportedOperation {
                harness,
                operation,
                reason,
            } => {
                assert_eq!(harness, HARNESS_ID_STR);
                assert_eq!(operation, "plan_wrapper");
                assert!(reason.contains("MigrationOnly"));
                assert!(reason.contains("community"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness_but_blocked_first() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.vibe-kanban-work");
        inst.harness = HarnessId::new("claude-code").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_workspaces_and_npx() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(!candidates.is_empty());
        assert!(
            candidates
                .iter()
                .any(|c| c.contains(".vibe-kanban-workspaces"))
        );
        assert!(candidates.iter().any(|c| c.contains("npx")));
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("VK_ALLOWED_ORIGINS") || c.contains("VIBEKANBAN"))
        );
    }

    #[test]
    fn validate_instance_accepts_project_scope() {
        let a = adapter();
        let inst = sample_instance_with_root("/tmp/.vibe-kanban-work");
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root("/tmp/.vibe-kanban-work");
        inst.isolation = Isolation::OsBound;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn supported_skill_modes_is_empty_migration_only() {
        let a = adapter();
        assert!(a.supported_skill_modes().is_empty());
    }
}
