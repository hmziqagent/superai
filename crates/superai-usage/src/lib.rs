//! Layer 3: usage truth. Local artifact readers, window/cost engines, the
//! llmproxy ledger client, and pollers. Reads telemetry only: never proxies
//! traffic, never writes harness config.

/// Usage-layer secret and path abuse verification.
pub mod abuse;
/// Cost periods and pricing metadata (USG-05).
pub mod cost;
/// llmproxy `/usage` ledger client (USG-08, subplan 15).
pub mod llmproxy;
/// API-key usage pollers (USG-09, subplan 15).
pub mod poller;
/// Local usage-artifact readers (USG-03).
pub mod reader;
/// Usage provider registry (USG-07).
pub mod registry;
/// Quota window model (USG-04).
pub mod window;

mod error;

pub use error::{Result, UsageError};
