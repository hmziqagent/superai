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
}

impl ConfigError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Result alias for config operations.
pub type Result<T> = std::result::Result<T, ConfigError>;
