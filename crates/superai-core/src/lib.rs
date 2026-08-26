//! Layer 2 — instances, templates, and capabilities.
//!
//! Exposes capabilities upward; harness identity stays below this line.

mod capability;
mod error;
/// Validated identifiers and names.
pub mod ids;
mod instance;
mod registry;

pub use capability::{Capability, Support};
pub use error::{CoreError, Result};
pub use instance::{Instance, TemplateRef};
pub use registry::{Registry, unmanaged_dirs};
