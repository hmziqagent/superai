//! Layer 2 — instances, templates, and capabilities.
//!
//! Exposes capabilities upward; harness identity stays below this line.

mod capability;
mod error;
/// Validated identifiers and names.
pub mod ids;
mod instance;
/// Operation preview and result contracts.
pub mod operation;
/// Validated path and executable reference types.
pub mod paths;
mod registry;
/// Lifecycle and ownership states.
pub mod state;

pub use capability::{Capability, Support};
pub use error::{CoreError, Result};
pub use instance::{Instance, TemplateRef};
pub use registry::{Registry, unmanaged_dirs};
