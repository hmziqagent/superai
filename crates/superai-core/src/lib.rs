//! Layer 2 — instances, templates, and capabilities.
//!
//! Exposes capabilities upward; harness identity stays below this line.

/// Harness adapter trait and supporting types.
pub mod adapter;
/// Concrete harness adapters.
pub mod adapters;
mod capability;
/// Capability resolver — harness/provider matrix.
pub mod capability_resolver;
/// Discovery, adoption, and drift reporting.
pub mod discovery;
mod error;
/// Registered harness catalog — the 48 planned product surfaces.
pub mod harness_catalog;
/// Validated identifiers and names.
pub mod ids;
mod instance;
/// Instance lifecycle orchestration.
pub mod lifecycle;
/// Operation preview and result contracts.
pub mod operation;
/// Validated path and executable reference types.
pub mod paths;
/// Provider definitions — data-driven.
pub mod provider;
/// Raw editor backend — harness-aware wrapper.
pub mod raw_editor;
mod registry;
/// Lifecycle and ownership states.
pub mod state;
/// Wrapper generation.
pub mod wrapper;

pub use capability::{Capability, Support};
pub use error::{CoreError, Result};
pub use instance::{Instance, TemplateRef, WrapperRef};
pub use registry::{Registry, SCHEMA_VERSION, unmanaged_dirs};
