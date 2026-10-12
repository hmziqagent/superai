//! Quota window model (USG-04); field shape ported from CodexBar's
//! `RateWindow` (`UsageFetcher.swift:3-15`).

use serde::{Deserialize, Serialize};

/// One provider-reported quota window. Raw values are kept exactly as
/// reported, over-quota included; clamping is a display concern, never
/// done here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateWindow {
    /// Usage as a percentage of the window, unclamped.
    pub used_percent: f64,
    /// Window length in minutes, when the provider reports it.
    pub window_minutes: Option<u32>,
    /// Provider-reported reset time, when available.
    pub resets_at: Option<String>,
    /// Provider-supplied reset description, when available.
    pub reset_description: Option<String>,
    /// Percent restored on the next regeneration tick, for
    /// rolling-recovery providers.
    pub next_regen_percent: Option<f64>,
}
