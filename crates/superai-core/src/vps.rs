//! VPS deploy and status (subplan 16): a deploy plan previewed against the
//! remote tree, committed stepwise with backups and rollback, plus read-only
//! status. Remote mutation goes through the [`remote::RemoteHost`] port; the
//! fixture backend proves the flow in-tree, the toride backend refuses typed
//! until toride-ssh/toride-service 0.2 publish (toride plan VPS-01).

mod execute;
mod plan;
mod point;
mod remote;
mod render;
mod status;

pub use execute::{
    BackupRecord, DeployJournal, JournalEntry, RollbackInfo, UpgradeRequest, execute_deploy,
    execute_upgrade, plan_upgrade,
};
pub use plan::{
    CheckState, DeployPlan, DeployRequest, DeployStep, Existing, IngressMode, PlannedStep,
    PreflightCheck, PreflightReport, ProviderToml, StepPreview, TunnelKind, plan_deploy, preflight,
    validate_host,
};
pub use point::{PointOutcome, point_instance_at, point_recorded_instance};
pub use remote::{
    ExecOutput, Fault, FixtureHost, FixtureRemote, HostKeyPolicy, RemoteError, RemoteHost,
    TorideRemote,
};
pub use status::{CertView, ServiceView, VpsStatusReport, remote_status};

/// Errors for the VPS surface; never carries a secret or remote file bytes.
#[derive(Debug, thiserror::Error)]
pub enum VpsError {
    /// A remote verb failed; the backend's own typed error, wrapped.
    #[error("{0}")]
    Remote(#[from] RemoteError),
    /// Request or argument validation refused.
    #[error("vps {field}: {reason}")]
    Validation {
        /// Field that failed validation.
        field: String,
        /// Why it was refused.
        reason: String,
    },
    /// A local file the plan needs is missing or unreadable.
    #[error("local artifact {path}: {reason}")]
    LocalArtifact {
        /// Path of the local file.
        path: std::path::PathBuf,
        /// What failed.
        reason: String,
    },
    /// The remote host is not in a state this operation allows.
    #[error("remote state {path}: {reason}")]
    RemoteState {
        /// Remote path or service the state check is about.
        path: String,
        /// Why the state blocks the operation.
        reason: String,
    },
    /// A superai-core operation the vps flow composes failed.
    #[error("{0}")]
    Core(#[from] crate::error::CoreError),
}

pub(crate) fn validation(field: &str, reason: String) -> VpsError {
    VpsError::Validation {
        field: field.to_owned(),
        reason,
    }
}
