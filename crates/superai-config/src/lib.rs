//! Layer 1 — harness config files.
//!
//! Every operation reads the file fresh, backs it up, and writes back preserving
//! keys superai does not model. Nothing is cached: the harness, an editor, or a
//! synced folder can change these files between two calls.

/// QAL-10/11 secret and path abuse verification.
pub mod abuse;
/// Atomic commit utilities.
pub mod atomic;
/// Backup catalog and verification.
pub mod backup;
/// Document envelope and selectors.
pub mod document;
/// Env file configs, comments and duplicate handling preserved.
pub mod env_file;
mod error;
/// Strict JSON configs, key order preserved.
pub mod json;
/// JSONC configs (comments + trailing commas), normalized on write.
pub mod jsonc;
/// Recoverable quarantine for directory removal.
pub mod quarantine;
/// Raw editor backend — read/validate/diff/commit.
pub mod raw_editor;
/// Filesystem snapshot and conflict token.
pub mod snapshot;
/// TOML configs, comments and formatting preserved.
pub mod toml_file;
/// Multi-file compensated transaction.
pub mod transaction;
/// YAML configs, validation and normalized write.
pub mod yaml;

#[cfg(test)]
mod test_util;

#[cfg(test)]
mod property_tests;

#[cfg(test)]
mod fuzz;

pub use error::{ConfigError, Result};
