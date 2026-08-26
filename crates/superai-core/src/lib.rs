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
/// Verification harness for plan 13 gates: fixtures, secret-free checks, platform gates.
pub mod verification;
/// Wrapper generation.
pub mod wrapper;

pub use capability::{Capability, Support};
pub use error::{CoreError, Result};
pub use instance::{Instance, TemplateRef, WrapperRef};
pub use registry::{Registry, SCHEMA_VERSION, unmanaged_dirs};

/// Duct-backed process execution wrapper (PKG-01, PKG-05).
pub mod process;

/// Install execution, verification receipt, update and uninstall (PKG-05..08).
pub mod install_execute;

/// Installation catalog — data-driven harness package registry (PKG-02).
pub mod install_catalog;

/// Install detection — collects all harness matches (PKG-03).
pub mod detect;

/// Install planning — validates and previews harness installs (PKG-04).
pub mod install_plan;

/// Template catalog, schema, and repo config (TPL-01, TPL-02).
pub mod template;
/// Template fetch client with HTTPS, digest, and traversal guards (TPL-03).
pub mod template_fetch;
/// Three-way template update and transactional apply (TPL-06, TPL-07).
pub mod template_update;

/// Skill registry, acquisition, destination modes, enable/disable and drift (EXT-01..05).
pub mod skills;

/// MCP canonical definition and lifecycle (EXT-08..10).
pub mod mcp;

/// Plugin abstraction and lifecycle (EXT-06/07).
pub mod plugin;
