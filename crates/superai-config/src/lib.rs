//! Layer 1 — harness config files.
//!
//! Every operation reads the file fresh, backs it up, and writes back preserving
//! keys superai does not model. Nothing is cached: the harness, an editor, or a
//! synced folder can change these files between two calls.

mod backup;
pub mod document;
mod error;
/// Strict JSON configs, key order preserved.
pub mod json;
/// JSONC configs (comments + trailing commas), normalized on write.
pub mod jsonc;
/// TOML configs, comments and formatting preserved.
pub mod toml_file;

pub use backup::{backup, restore};
pub use error::{ConfigError, Result};
