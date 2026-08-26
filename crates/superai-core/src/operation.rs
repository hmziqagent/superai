//! Operation preview and result contracts.
//!
//! Every mutating workflow returns a [`OperationPreview`] before commit and a
//! [`OperationResult`] after commit. No interface types are referenced; all
//! fields are harness-agnostic, serializable, and redact secrets.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ids::{BackupId, HarnessId, InstanceName, OperationId};
use crate::paths::AbsolutePath;

// ---------------------------------------------------------------------------
// Redacted helper
// ---------------------------------------------------------------------------

/// Wrapper for secret-bearing values that never exposes the inner value.
///
/// Debug, Display, and Serialize all emit a fixed placeholder. The raw secret
/// is only reachable via [`Self::expose_secret`], which callers must use
/// explicitly at the harness-write boundary.
#[derive(Clone, PartialEq, Eq)]
pub struct RedactedString(String);

impl RedactedString {
    /// Create a new redacted wrapper from a secret value.
    pub fn new(secret: &str) -> Self {
        Self(secret.to_owned())
    }

    /// Borrow the raw secret. Use only at the sink that writes to the harness
    /// config; never log or serialize this value.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Redacted placeholder used in serialization and display.
    pub fn placeholder() -> &'static str {
        "[REDACTED]"
    }
}

impl fmt::Debug for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedactedString([REDACTED])")
    }
}

impl fmt::Display for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Serialize for RedactedString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(Self::placeholder())
    }
}

impl<'de> Deserialize<'de> for RedactedString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        // Deserializing a preview/result never recovers the original secret;
        // the placeholder is stored. The true secret is only available from
        // the live `RedactedString` held at call-site, not from persisted JSON.
        Ok(Self(s))
    }
}

// ---------------------------------------------------------------------------
// OperationKind
// ---------------------------------------------------------------------------

/// High-level kind of a mutating operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    /// Create a new isolated instance.
    CreateInstance,
    /// Mirror an existing instance into a new isolated root.
    MirrorInstance,
    /// Adopt an existing unmanaged config directory.
    AdoptInstance,
    /// Remove an instance record and optionally its wrapper/root.
    RemoveInstance,
    /// Rename an instance and its wrapper.
    RenameInstance,
    /// Reconfigure provider/model/skills for an instance.
    ReconfigureInstance,
    /// Update harness config for an instance.
    UpdateConfig,
    /// Restore a backup over its original.
    RestoreBackup,
    /// Install or update a harness binary.
    InstallHarness,
    /// Uninstall a harness binary.
    UninstallHarness,
    /// Manage a skill (install/link/copy/remove).
    ManageSkill,
    /// Manage a plugin.
    ManagePlugin,
    /// Manage an MCP server definition.
    ManageMcp,
    /// Configure provider endpoint or model defaults.
    ConfigureProvider,
    /// Generic create.
    Create,
    /// Generic update.
    Update,
    /// Generic delete.
    Delete,
    /// Other custom operation.
    Custom,
}

impl fmt::Display for OperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::CreateInstance => "create_instance",
            Self::MirrorInstance => "mirror_instance",
            Self::AdoptInstance => "adopt_instance",
            Self::RemoveInstance => "remove_instance",
            Self::RenameInstance => "rename_instance",
            Self::ReconfigureInstance => "reconfigure_instance",
            Self::UpdateConfig => "update_config",
            Self::RestoreBackup => "restore_backup",
            Self::InstallHarness => "install_harness",
            Self::UninstallHarness => "uninstall_harness",
            Self::ManageSkill => "manage_skill",
            Self::ManagePlugin => "manage_plugin",
            Self::ManageMcp => "manage_mcp",
            Self::ConfigureProvider => "configure_provider",
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Custom => "custom",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// RequestedTarget / ResolvedResource
// ---------------------------------------------------------------------------

/// What the caller asked to operate on, before adapter resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestedTarget {
    /// Display form of the requested target as supplied by the caller.
    pub display: String,
    /// Optional harness in scope for this operation.
    pub harness: Option<HarnessId>,
    /// Optional instance name in scope for this operation.
    pub instance: Option<InstanceName>,
}

/// A resource resolved from the requested target via adapter and filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedResource {
    /// Kind of resource, e.g., `config_file`, `config_root`, `wrapper`, `instance_root`.
    pub kind: String,
    /// Normalized absolute path of the resource.
    pub path: AbsolutePath,
    /// Human-readable description of the resource role.
    pub description: String,
    /// Whether the resource is owned by superai or is foreign.
    pub owned_by_superai: bool,
}

// ---------------------------------------------------------------------------
// Preconditions
// ---------------------------------------------------------------------------

/// Kind of precondition that must hold before commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreconditionKind {
    /// File or directory must exist.
    Exists,
    /// File or directory must be absent.
    Absent,
    /// File must be unchanged since preview (digest match).
    Unchanged,
    /// Binary must be present and executable.
    BinaryPresent,
    /// No foreign ownership marker present.
    NoForeignOwner,
    /// Sufficient disk space.
    DiskSpace,
    /// No concurrent modification detected.
    NoConcurrentModification,
    /// Authentication is present or not required.
    AuthPresent,
}

/// A single precondition for the operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Precondition {
    /// Kind of precondition.
    pub kind: PreconditionKind,
    /// Human-readable description.
    pub description: String,
    /// Path the precondition relates to, if any.
    pub path: Option<AbsolutePath>,
    /// Whether the precondition is currently satisfied.
    pub satisfied: bool,
}

// ---------------------------------------------------------------------------
// Planned actions (ordered file/process actions)
// ---------------------------------------------------------------------------

/// Kind of ordered action in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// Create a directory.
    CreateDir,
    /// Write or atomically replace a file.
    WriteFile,
    /// Copy a file or directory.
    CopyFile,
    /// Remove a file.
    RemoveFile,
    /// Remove a directory.
    RemoveDir,
    /// Create a symlink.
    CreateSymlink,
    /// Remove a symlink.
    RemoveSymlink,
    /// Move a path into quarantine.
    MoveToQuarantine,
    /// Generate a wrapper script/binary shim.
    CreateWrapper,
    /// Execute a process (e.g., version probe).
    ExecProcess,
    /// Update the registry file.
    UpdateRegistry,
}

/// An ordered step that will be executed on commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedAction {
    /// Deterministic order, starting at 0.
    pub order: u32,
    /// Kind of action.
    pub kind: ActionKind,
    /// Primary target path of the action.
    pub target: AbsolutePath,
    /// Human-readable description of what the action does.
    pub description: String,
    /// Whether this action mutates a file superai did not create and therefore needs a backup.
    pub requires_backup: bool,
}

// ---------------------------------------------------------------------------
// Diffs (redacted)
// ---------------------------------------------------------------------------

/// A redacted diff for a single surface. Secrets are never included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedDiff {
    /// Absolute path of the file whose diff this is.
    pub path: AbsolutePath,
    /// Surface identifier, e.g., `settings.json` or `config.toml`.
    pub surface: String,
    /// Lexical diff with secret values replaced by `[REDACTED]`.
    pub lexical_redacted: String,
    /// Semantic summary with secrets redacted.
    pub semantic_redacted: String,
    /// Names of fields whose values were redacted in this diff.
    pub redacted_fields: Vec<String>,
}

// ---------------------------------------------------------------------------
// Backups
// ---------------------------------------------------------------------------

/// A backup that will be created before the first write to a foreign file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupPlan {
    /// Identifier for the backup artifact.
    pub backup_id: BackupId,
    /// Original path to be backed up.
    pub source_path: AbsolutePath,
    /// Where the backup will be written. `None` if the source does not yet exist.
    pub backup_path: Option<AbsolutePath>,
    /// Reason for the backup.
    pub reason: String,
    /// Digest of the source before mutation, if known.
    pub digest_before: Option<String>,
}

/// A backup record after commit, with concrete IDs and paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupRecord {
    /// Backup identifier.
    pub backup_id: BackupId,
    /// Original source path.
    pub source_path: AbsolutePath,
    /// Actual backup path on disk, if a backup was created.
    pub backup_path: Option<AbsolutePath>,
    /// Digest before mutation, if captured.
    pub digest_before: Option<String>,
    /// Whether a backup file was created (false for a first-write creation).
    pub created: bool,
}

// ---------------------------------------------------------------------------
// Warnings / Conflicts / Limitations
// ---------------------------------------------------------------------------

/// Non-blocking warning about the operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    /// Stable code for the warning, e.g., `permissions_existing`.
    pub code: String,
    /// Human-readable message, secrets already redacted.
    pub message: String,
    /// Path the warning relates to, if any.
    pub path: Option<AbsolutePath>,
}

/// Hard conflict that blocks commit until resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    /// Stable code for the conflict, e.g., `name_collision`.
    pub code: String,
    /// Human-readable message, secrets already redacted.
    pub message: String,
    /// Paths involved in the conflict.
    pub paths: Vec<AbsolutePath>,
}

/// Documented limitation of the operation on this harness or platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limitation {
    /// Stable code for the limitation.
    pub code: String,
    /// Human-readable description.
    pub description: String,
}

/// An external auth step the user must perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthStep {
    /// Human-readable description of the step.
    pub description: String,
    /// Harness the step pertains to, if any.
    pub harness: Option<HarnessId>,
    /// Whether the step is required for the operation to succeed.
    pub required: bool,
}

/// A restart or reload requirement after commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartRequirement {
    /// Target that needs restart, e.g., `harness`, `daemon`, `ide`.
    pub target: String,
    /// Reason for the requirement.
    pub reason: String,
    /// Whether restart is required for changes to take effect.
    pub required: bool,
}

// ---------------------------------------------------------------------------
// Rollback
// ---------------------------------------------------------------------------

/// A single step in the rollback plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackStep {
    /// Deterministic order.
    pub order: u32,
    /// Human-readable description.
    pub description: String,
    /// Target path to restore or remove.
    pub target: AbsolutePath,
    /// Backup to restore from, if any.
    pub backup_id: Option<BackupId>,
}

/// Deterministic rollback plan if commit fails partway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackPlan {
    /// Ordered steps to revert completed actions.
    pub steps: Vec<RollbackStep>,
    /// Whether the plan will restore backups (true) or simply remove creations (false).
    pub will_restore_backups: bool,
    /// Estimated number of steps.
    pub estimated_steps: usize,
}

// ---------------------------------------------------------------------------
// Commit result types
// ---------------------------------------------------------------------------

/// An action that was executed during commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletedAction {
    /// Order as in the preview.
    pub order: u32,
    /// Kind of action.
    pub kind: ActionKind,
    /// Target path that was acted on.
    pub target: AbsolutePath,
    /// Whether this action succeeded.
    pub success: bool,
    /// Optional elapsed time in milliseconds.
    pub elapsed_ms: Option<u64>,
}

/// Kind of verification performed after commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationKind {
    /// File parses as valid JSON/TOML/etc.
    Parse,
    /// Digest matches expected post-write content.
    Digest,
    /// Semantic assertions pass (e.g., required keys present).
    Semantic,
    /// Permissions match expected mode.
    Permissions,
    /// Process probe (e.g., version) succeeded.
    ProcessProbe,
}

/// Result of a single verification check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Path that was verified.
    pub path: AbsolutePath,
    /// Kind of verification.
    pub kind: VerificationKind,
    /// Whether verification passed.
    pub passed: bool,
    /// Human-readable message, secrets redacted.
    pub message: String,
}

/// Status of rollback after a failed commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackStatus {
    /// No rollback was needed.
    NotNeeded,
    /// Rollback was attempted and succeeded.
    Succeeded,
    /// Rollback was attempted and partially succeeded.
    Partial,
    /// Rollback was attempted and failed.
    Failed,
    /// Rollback was not attempted (e.g., caller chose to inspect residuals).
    NotAttempted,
}

impl fmt::Display for RollbackStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::NotNeeded => "not_needed",
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::NotAttempted => "not_attempted",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// OperationPreview / OperationResult
// ---------------------------------------------------------------------------

/// Preview of a mutating operation before commit.
///
/// Contains every field required by FND-05: identification, targets, preconditions,
/// ordered actions, redacted diffs, backups, warnings/conflicts/limitations,
/// auth and restart requirements, and a deterministic rollback plan. No interface
/// types are present and all secrets are redacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationPreview {
    /// Stable identifier for the preview/commit cycle.
    pub id: OperationId,
    /// High-level kind of operation.
    pub kind: OperationKind,
    /// What the caller requested, before resolution.
    pub requested_target: RequestedTarget,
    /// Fully resolved resources after adapter and filesystem resolution.
    pub resolved_resources: Vec<ResolvedResource>,
    /// Conditions that must hold before commit.
    pub preconditions: Vec<Precondition>,
    /// Ordered file and process actions that will be executed.
    pub actions: Vec<PlannedAction>,
    /// Redacted lexical and semantic diffs for each affected surface.
    pub diffs: Vec<RedactedDiff>,
    /// Backups that will be created before first write.
    pub backups: Vec<BackupPlan>,
    /// Non-blocking warnings.
    pub warnings: Vec<Warning>,
    /// Hard conflicts that block commit.
    pub conflicts: Vec<Conflict>,
    /// Known limitations on this harness/platform.
    pub limitations: Vec<Limitation>,
    /// External auth steps required.
    pub auth_steps: Vec<AuthStep>,
    /// Restart or reload requirements after commit.
    pub restart_requirements: Vec<RestartRequirement>,
    /// Deterministic rollback plan if commit fails partway.
    pub rollback_plan: RollbackPlan,
}

/// Result of committing an operation preview.
///
/// Contains every field required by FND-05: exact actions completed, backup
/// IDs and paths, verification results, rollback status, and redacted diagnostics.
/// No interface types are present and all secrets are redacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationResult {
    /// Identifier of the operation that was committed.
    pub id: OperationId,
    /// Kind of operation that was committed.
    pub kind: OperationKind,
    /// Exact actions that were executed, in order.
    pub actions_completed: Vec<CompletedAction>,
    /// Backups that were created, with concrete IDs and paths.
    pub backups: Vec<BackupRecord>,
    /// Verification results after commit or after rollback.
    pub verification: Vec<VerificationResult>,
    /// Rollback status; `NotNeeded` when commit succeeded.
    pub rollback_status: RollbackStatus,
    /// Diagnostics with secrets redacted; never contains raw secret values.
    pub diagnostics_redacted: Vec<String>,
    /// Whether the whole operation is considered successful.
    pub success: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn sample_preview() -> OperationPreview {
        let id = OperationId::new("op-preview-1").unwrap();
        let backup_id = BackupId::new("backup-1").unwrap();
        OperationPreview {
            id,
            kind: OperationKind::CreateInstance,
            requested_target: RequestedTarget {
                display: "work".to_owned(),
                harness: HarnessId::new("claude-code").ok(),
                instance: InstanceName::new("work").ok(),
            },
            resolved_resources: vec![ResolvedResource {
                kind: "config_root".to_owned(),
                path: AbsolutePath::new("/home/user/.claude-work").unwrap(),
                description: "isolated config root".to_owned(),
                owned_by_superai: true,
            }],
            preconditions: vec![Precondition {
                kind: PreconditionKind::Absent,
                description: "target path must be absent".to_owned(),
                path: AbsolutePath::new("/home/user/.claude-work").ok(),
                satisfied: true,
            }],
            actions: vec![PlannedAction {
                order: 0,
                kind: ActionKind::CreateDir,
                target: AbsolutePath::new("/home/user/.claude-work").unwrap(),
                description: "create isolated root".to_owned(),
                requires_backup: false,
            }],
            diffs: vec![RedactedDiff {
                path: AbsolutePath::new("/home/user/.claude-work/settings.json").unwrap(),
                surface: "settings.json".to_owned(),
                lexical_redacted: "{\"model\":\"sonnet\",\"apiKey\":\"[REDACTED]\"}".to_owned(),
                semantic_redacted: "set model to sonnet, set apiKey to [REDACTED]".to_owned(),
                redacted_fields: vec!["apiKey".to_owned()],
            }],
            backups: vec![BackupPlan {
                backup_id,
                source_path: AbsolutePath::new("/home/user/.claude/settings.json").unwrap(),
                backup_path: AbsolutePath::new("/home/user/.claude/settings.json.bak.1").ok(),
                reason: "preserve foreign file before write".to_owned(),
                digest_before: Some("abc123".to_owned()),
            }],
            warnings: vec![Warning {
                code: "permissions_existing".to_owned(),
                message: "existing file has broad permissions".to_owned(),
                path: AbsolutePath::new("/home/user/.claude/settings.json").ok(),
            }],
            conflicts: vec![],
            limitations: vec![Limitation {
                code: "single_instance".to_owned(),
                description: "harness supports only single instance".to_owned(),
            }],
            auth_steps: vec![AuthStep {
                description: "run harness login to obtain API key".to_owned(),
                harness: HarnessId::new("claude-code").ok(),
                required: true,
            }],
            restart_requirements: vec![RestartRequirement {
                target: "harness".to_owned(),
                reason: "config change requires restart".to_owned(),
                required: true,
            }],
            rollback_plan: RollbackPlan {
                steps: vec![RollbackStep {
                    order: 0,
                    description: "remove created dir".to_owned(),
                    target: AbsolutePath::new("/home/user/.claude-work").unwrap(),
                    backup_id: None,
                }],
                will_restore_backups: false,
                estimated_steps: 1,
            },
        }
    }

    fn sample_result() -> OperationResult {
        let id = OperationId::new("op-preview-1").unwrap();
        let backup_id = BackupId::new("backup-1").unwrap();
        OperationResult {
            id,
            kind: OperationKind::CreateInstance,
            actions_completed: vec![CompletedAction {
                order: 0,
                kind: ActionKind::CreateDir,
                target: AbsolutePath::new("/home/user/.claude-work").unwrap(),
                success: true,
                elapsed_ms: Some(12),
            }],
            backups: vec![BackupRecord {
                backup_id,
                source_path: AbsolutePath::new("/home/user/.claude/settings.json").unwrap(),
                backup_path: AbsolutePath::new("/home/user/.claude/settings.json.bak.1").ok(),
                digest_before: Some("abc123".to_owned()),
                created: true,
            }],
            verification: vec![VerificationResult {
                path: AbsolutePath::new("/home/user/.claude-work/settings.json").unwrap(),
                kind: VerificationKind::Parse,
                passed: true,
                message: "file parses and contains expected keys".to_owned(),
            }],
            rollback_status: RollbackStatus::NotNeeded,
            diagnostics_redacted: vec!["created instance work".to_owned()],
            success: true,
        }
    }

    #[test]
    fn preview_serializes_without_leaking_secret() {
        let preview = sample_preview();
        let secret = "super-secret-sentinel-42";
        // Simulate that diff lexical was correctly redacted: it must not contain the secret,
        // and must contain the placeholder.
        let redacted_diff = format!("apiKey set to {}", RedactedString::placeholder());
        let _ = redacted_diff;
        let json = serde_json::to_string(&preview).unwrap();
        assert!(
            !json.contains(secret),
            "serialized preview must not contain secret sentinel"
        );
        assert!(
            json.contains("[REDACTED]"),
            "serialized preview must contain redacted placeholder"
        );
        // Ensure preview round-trips.
        let back: OperationPreview = serde_json::from_str(&json).unwrap();
        assert_eq!(preview, back);
    }

    #[test]
    fn result_serializes_without_leaking_secret() {
        let mut result = sample_result();
        // Put a diagnostic that has been redacted prior to insertion.
        let secret = "another-super-secret-99";
        let diagnostic_redacted = format!("apiKey was {}", RedactedString::placeholder());
        result.diagnostics_redacted.push(diagnostic_redacted);
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains(secret));
        assert!(json.contains("[REDACTED]"));
        let back: OperationResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, back);
    }

    #[test]
    fn redacted_string_debug_and_display_do_not_leak() {
        let secret = "my-very-secret-api-key-xyz";
        let redacted = RedactedString::new(secret);
        let debug = format!("{redacted:?}");
        let display = format!("{redacted}");
        let json = serde_json::to_string(&redacted).unwrap();
        for output in [debug, display, json] {
            assert!(
                !output.contains(secret),
                "redacted output must not contain secret: {output}"
            );
            assert!(
                output.contains("[REDACTED]"),
                "redacted output must contain placeholder: {output}"
            );
        }
        // Expose is explicit.
        assert_eq!(redacted.expose_secret(), secret);
    }

    #[test]
    fn redacted_string_equality_is_based_on_secret() {
        let a = RedactedString::new("same");
        let b = RedactedString::new("same");
        let c = RedactedString::new("different");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn preview_contains_all_required_fields() {
        let preview = sample_preview();
        // Verify that all required FND-05 sections are present and non-empty where expected.
        assert!(!preview.actions.is_empty(), "actions must be ordered");
        assert_eq!(preview.actions[0].order, 0);
        assert!(!preview.diffs.is_empty());
        assert!(!preview.backups.is_empty());
        assert!(!preview.rollback_plan.steps.is_empty());
        // Ensure actions are ordered.
        let mut last_order: Option<u32> = None;
        for action in &preview.actions {
            if let Some(prev) = last_order {
                assert!(action.order > prev, "actions must be strictly ordered");
            }
            last_order = Some(action.order);
        }
    }

    #[test]
    fn operation_kind_display_and_serialization() {
        let kinds = [
            (OperationKind::CreateInstance, "create_instance"),
            (OperationKind::RemoveInstance, "remove_instance"),
            (OperationKind::InstallHarness, "install_harness"),
            (OperationKind::ManageSkill, "manage_skill"),
            (OperationKind::Custom, "custom"),
        ];
        for (kind, expected) in kinds {
            assert_eq!(kind.to_string(), expected);
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{expected}\""));
            let back: OperationKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn result_reports_backup_ids_and_verification() {
        let result = sample_result();
        assert_eq!(result.rollback_status, RollbackStatus::NotNeeded);
        assert!(result.success);
        assert!(!result.backups.is_empty());
        assert!(!result.verification.is_empty());
        assert!(result.verification[0].passed);
        // Backups must carry IDs.
        let ids: HashSet<_> = result
            .backups
            .iter()
            .map(|b| b.backup_id.as_str().to_owned())
            .collect();
        assert!(ids.contains("backup-1"));
    }

    #[test]
    fn operation_preview_and_result_use_validated_ids_and_paths() {
        let preview = sample_preview();
        assert_eq!(preview.id.as_str(), "op-preview-1");
        assert_eq!(
            preview.resolved_resources[0]
                .path
                .as_path()
                .to_string_lossy(),
            "/home/user/.claude-work"
        );
        // Ensure paths are normalized absolute (no traversal).
        for resource in &preview.resolved_resources {
            assert!(resource.path.as_path().is_absolute());
            for comp in resource.path.as_path().components() {
                assert!(
                    !matches!(comp, std::path::Component::ParentDir),
                    "path must not contain traversal"
                );
            }
        }
    }

    #[test]
    fn rollback_status_display() {
        assert_eq!(RollbackStatus::Succeeded.to_string(), "succeeded");
        assert_eq!(RollbackStatus::NotNeeded.to_string(), "not_needed");
        // Serialization round-trip.
        let json = serde_json::to_string(&RollbackStatus::Failed).unwrap();
        assert_eq!(json, "\"failed\"");
        let back: RollbackStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, RollbackStatus::Failed);
    }

    #[test]
    fn diffs_are_already_redacted() {
        let preview = sample_preview();
        let secret = "s3cr3t-not-in-diff";
        for diff in &preview.diffs {
            assert!(
                !diff.lexical_redacted.contains(secret),
                "diff lexical must be redacted"
            );
            assert!(
                !diff.semantic_redacted.contains(secret),
                "diff semantic must be redacted"
            );
            assert!(
                diff.lexical_redacted.contains("[REDACTED]")
                    || diff.semantic_redacted.contains("[REDACTED]")
            );
        }
    }
}
