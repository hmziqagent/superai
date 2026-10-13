//! Usage-layer error taxonomy.

/// Errors for the poller module; registry and cost carry their own
/// `RegistryError` and `CostError`.
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    /// Poller bounds the PRV-07 discipline forbids.
    #[error("invalid poller limits: {reason}")]
    InvalidPollLimits {
        /// What was out of bounds.
        reason: String,
    },
}

/// Result alias for usage operations.
pub type Result<T> = std::result::Result<T, UsageError>;
