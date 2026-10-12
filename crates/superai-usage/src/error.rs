//! Usage-layer error taxonomy.

/// Errors for the registry and poller modules; cost and pricing carry
/// their own `CostError` in [`crate::cost`].
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    /// Two registry entries claimed the same provider id.
    #[error("duplicate usage provider entry `{id}`")]
    DuplicateProvider {
        /// Provider id that collided.
        id: String,
    },
    /// Poller bounds the PRV-07 discipline forbids.
    #[error("invalid poller limits: {reason}")]
    InvalidPollLimits {
        /// What was out of bounds.
        reason: String,
    },
}

/// Result alias for usage operations.
pub type Result<T> = std::result::Result<T, UsageError>;
