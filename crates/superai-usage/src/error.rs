//! Usage-layer error taxonomy.

/// Everything that can go wrong in the usage layer.
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    /// Two registry entries claimed the same provider id.
    #[error("duplicate usage provider entry `{id}`")]
    DuplicateProvider {
        /// Provider id that collided.
        id: String,
    },
    /// Rolling cost window outside the 1..=365 day range (USG-05).
    #[error("rolling cost window of {days} days is outside 1..=365")]
    RollingWindowOutOfRange {
        /// Day count that was rejected.
        days: u16,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_provider_display_names_the_id() {
        let err = UsageError::DuplicateProvider {
            id: "anthropic".to_owned(),
        };
        assert_eq!(
            err.to_string(),
            "duplicate usage provider entry `anthropic`"
        );
    }
}
