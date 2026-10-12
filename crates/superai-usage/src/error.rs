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
