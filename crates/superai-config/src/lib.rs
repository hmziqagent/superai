//! Layer 1 — harness config files.
//!
//! Every operation reads the file fresh, backs it up, and writes back preserving
//! keys superai does not model. Nothing is cached: the harness, an editor, or a
//! synced folder can change these files between two calls.

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
/// Filesystem snapshot and conflict token.
pub mod snapshot;
/// TOML configs, comments and formatting preserved.
pub mod toml_file;
/// YAML configs, validation and normalized write.
pub mod yaml;

pub use atomic::{atomic_write, atomic_write_with_expected_digest, atomic_write_with_snapshot};
pub use backup::{
    BackupEntry, BackupId, backup, backup_with_operation, backup_with_reason, list_backups,
    restore, restore_entry, verify_backup,
};
pub use error::{ConfigError, Result};
pub use snapshot::{Snapshot, is_modified, is_symlink_loop, snapshot};
