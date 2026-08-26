use std::path::PathBuf;

/// Everything that can go wrong reading or writing a harness config file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read, written, or copied.
    #[error("io error on {path}: {source}")]
    Io {
        /// Path the operation was attempted on.
        path: PathBuf,
        /// Underlying OS error.
        source: std::io::Error,
    },

    /// The file exists but is not valid JSON.
    #[error("invalid json in {path}: {source}")]
    Json {
        /// Path of the offending file.
        path: PathBuf,
        /// Parser error.
        source: serde_json::Error,
    },

    /// The file exists but is not valid TOML.
    #[error("invalid toml in {path}: {source}")]
    Toml {
        /// Path of the offending file.
        path: PathBuf,
        /// Parser error.
        source: toml_edit::TomlError,
    },

    /// The file exists but is not valid YAML.
    #[error("invalid yaml in {path}: {source}")]
    Yaml {
        /// Path of the offending file.
        path: PathBuf,
        /// Parser error.
        source: yaml_serde::Error,
    },

    /// The file exists but is not a valid env file.
    #[error("invalid env file in {path}: {message}")]
    Env {
        /// Path of the offending file.
        path: PathBuf,
        /// Human-readable message.
        message: String,
    },

    /// A JSON config was expected to hold an object at its root.
    #[error("expected a json object at the root of {path}")]
    NotAnObject {
        /// Path of the offending file.
        path: PathBuf,
    },

    /// The file changed between preview and commit.
    #[error("concurrent modification of {path}: expected {expected}, actual {actual}")]
    ConcurrentModification {
        /// Path that was concurrently modified.
        path: PathBuf,
        /// Digest or metadata expected at preview time.
        expected: String,
        /// Digest or metadata observed at commit time.
        actual: String,
    },

    /// Post-commit verification failed.
    #[error("verification failed for {path}: {reason}")]
    Verification {
        /// Path that verification was attempted for.
        path: PathBuf,
        /// Human-readable reason.
        reason: String,
    },

    /// Backup verification failed.
    #[error("backup verification failed for {path}: {reason}")]
    BackupVerification {
        /// Path that backup verification was attempted for.
        path: PathBuf,
        /// Human-readable reason.
        reason: String,
    },
}

impl ConfigError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn concurrent_modification(
        path: impl Into<PathBuf>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self::ConcurrentModification {
            path: path.into(),
            expected: expected.into(),
            actual: actual.into(),
        }
    }

    pub(crate) fn verification(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::Verification {
            path: path.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn backup_verification(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::BackupVerification {
            path: path.into(),
            reason: reason.into(),
        }
    }
}

/// Result alias for config operations.
pub type Result<T> = std::result::Result<T, ConfigError>;
