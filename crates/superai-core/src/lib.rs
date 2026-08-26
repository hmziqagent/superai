//! Layer 2 — instances, templates, and capabilities.
//!
//! Exposes capabilities upward; harness identity stays below this line.

/// Harness adapter trait and supporting types.
pub mod adapter;
mod capability;
mod error;
/// Registered harness catalog — the 48 planned product surfaces.
pub mod harness_catalog;
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
pub use instance::{Instance, TemplateRef, WrapperRef};
pub use registry::{Registry, SCHEMA_VERSION, unmanaged_dirs};
