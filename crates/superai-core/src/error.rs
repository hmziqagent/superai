/// Everything that can go wrong in the core layer.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// A config file operation failed.
    #[error(transparent)]
    Config(#[from] superai_config::ConfigError),

    /// The records file held something that is not a list of instances.
    #[error("malformed instance records: {0}")]
    Records(#[from] serde_json::Error),

    /// An instance with this name already exists.
    #[error("instance `{name}` already exists")]
    DuplicateInstance {
        /// The name that collided.
        name: String,
    },

    /// The user's home directory could not be determined.
    #[error("cannot determine the home directory")]
    NoHomeDir,

    /// An identifier or name failed validation.
    #[error("invalid {kind} `{value}`: {reason}")]
    InvalidIdentifier {
        /// Kind of identifier that failed validation.
        kind: String,
        /// Offending value.
        value: String,
        /// Human-readable reason.
        reason: String,
    },

    /// A path or executable reference failed validation.
    #[error("invalid {kind} `{value}`: {reason}")]
    InvalidPath {
        /// Kind of path that failed validation.
        kind: String,
        /// Offending value.
        value: String,
        /// Human-readable reason.
        reason: String,
    },
}

/// Result alias for core operations.
pub type Result<T> = std::result::Result<T, CoreError>;
