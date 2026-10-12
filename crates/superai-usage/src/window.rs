//! Quota window model (USG-04), porting `CodexBar`'s `RateWindow`
//! (`UsageFetcher.swift:3-15`): raw values, explicit lane states.

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

impl RateWindow {
    /// Provider-reported `resets_at` wins; a reset computed from local
    /// logs fills in only when the provider reported none.
    pub fn resolved_resets_at<'a>(&'a self, computed: Option<&'a str>) -> Option<&'a str> {
        self.resets_at.as_deref().or(computed)
    }
}

/// One quota lane's reading; `CodexBar`'s `isSyntheticPlaceholder` bool
/// and `usageKnown` flag are variants here, so an unmeasured window can
/// never pose as a real zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneReading {
    /// The provider reported no lane of this kind; distinct from a
    /// reported zero.
    Unavailable,
    /// The lane exists, but only the provider's account endpoint or
    /// llmproxy can read it; local logs must not fabricate a fill.
    ProviderOnly,
    /// Placeholder standing in for a lane the provider did not report;
    /// its `used_percent` is not a measurement.
    Synthesized(RateWindow),
    /// Reset metadata arrived without usage; the window's percent field
    /// is not a measurement, only its reset fields carry meaning.
    Incomplete(RateWindow),
    /// A measured window; `used_percent == 0.0` is a real zero and
    /// over-quota values keep their raw magnitude.
    Measured(RateWindow),
}

impl LaneReading {
    /// The window payload, when one exists at all. Synthesized and
    /// incomplete windows stay visible for their reset context.
    pub fn window(&self) -> Option<&RateWindow> {
        match self {
            Self::Unavailable | Self::ProviderOnly => None,
            Self::Synthesized(window) | Self::Incomplete(window) | Self::Measured(window) => {
                Some(window)
            }
        }
    }

    /// Measured usage percent, unclamped. `Some(0.0)` is a real zero;
    /// every other state reports `None`, never a phantom zero.
    pub fn used_percent(&self) -> Option<f64> {
        match self {
            Self::Measured(window) => Some(window.used_percent),
            Self::Unavailable | Self::ProviderOnly | Self::Synthesized(_) | Self::Incomplete(_) => {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(used_percent: f64) -> RateWindow {
        RateWindow {
            used_percent,
            window_minutes: Some(300),
            resets_at: None,
            reset_description: None,
            next_regen_percent: None,
        }
    }

    fn window_with_reset(used_percent: f64, resets_at: &str) -> RateWindow {
        RateWindow {
            resets_at: Some(resets_at.to_owned()),
            ..window(used_percent)
        }
    }

    fn assert_percent(reading: &LaneReading, expected: f64) {
        let found = reading
            .used_percent()
            .unwrap_or_else(|| panic!("state {reading:?} reported no usage"));
        assert_eq!(
            found.to_bits(),
            expected.to_bits(),
            "usage percent was not preserved bit-for-bit"
        );
    }

    #[test]
    fn unavailable_zero_and_incomplete_stay_distinct_states() {
        let unavailable = LaneReading::Unavailable;
        let zero = LaneReading::Measured(window(0.0));
        let incomplete = LaneReading::Incomplete(window_with_reset(0.0, "2026-10-13T10:00:00Z"));

        assert!(unavailable.used_percent().is_none());
        assert!(unavailable.window().is_none());
        assert_percent(&zero, 0.0);
        assert!(zero.window().is_some());
        assert!(incomplete.used_percent().is_none());
        assert_eq!(
            incomplete.window().map(|w| w.resets_at.as_deref()),
            Some(Some("2026-10-13T10:00:00Z"))
        );

        assert_ne!(unavailable, zero);
        assert_ne!(zero, incomplete);
        assert_ne!(unavailable, incomplete);
    }

    #[test]
    fn synthesized_zero_is_not_a_measured_zero() {
        let phantom = LaneReading::Synthesized(window(0.0));
        let real = LaneReading::Measured(window(0.0));

        assert!(phantom.used_percent().is_none());
        assert!(phantom.window().is_some());
        assert_percent(&real, 0.0);
        assert_ne!(phantom, real);
    }

    #[test]
    fn over_quota_values_keep_their_raw_magnitude() {
        let reading = LaneReading::Measured(window(137.5));

        assert_percent(&reading, 137.5);
        assert!(reading.window().is_some());
    }

    #[test]
    fn provider_only_is_distinct_from_unavailable() {
        let provider_only = LaneReading::ProviderOnly;

        assert!(provider_only.used_percent().is_none());
        assert!(provider_only.window().is_none());
        assert_ne!(provider_only, LaneReading::Unavailable);
    }

    #[test]
    fn provider_reported_reset_is_preferred_over_a_computed_one() {
        let window = window_with_reset(40.0, "2026-10-13T10:00:00Z");

        assert_eq!(
            window.resolved_resets_at(Some("2026-10-13T09:00:00Z")),
            Some("2026-10-13T10:00:00Z")
        );
        assert_eq!(
            window.resolved_resets_at(None),
            Some("2026-10-13T10:00:00Z")
        );
    }

    #[test]
    fn computed_reset_fills_in_only_when_the_provider_reported_none() {
        let window = window(40.0);

        assert_eq!(
            window.resolved_resets_at(Some("2026-10-13T11:30:00Z")),
            Some("2026-10-13T11:30:00Z")
        );
        assert_eq!(window.resolved_resets_at(None), None);
    }
}
