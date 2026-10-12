//! API-key usage pollers (USG-09, subplan 15) under the PRV-07 probe
//! discipline: bounded, redacted, no background polling.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use superai_core::health::{MAX_RESPONSE_BYTES, validate_timeout};

use crate::error::{Result, UsageError};

/// Bounds for one explicit poller invocation, held to the window the health
/// probe already enforces (subplan 07's shipped PRV-07 discipline).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollLimits {
    /// Whole-request timeout.
    pub timeout: Duration,
    /// Response bytes the poller stops at.
    pub max_response_bytes: usize,
}

impl PollLimits {
    /// Accept only bounds the health probe would accept: timeout within
    /// 1..=30s, response cap within 1..=1 MiB.
    pub fn new(timeout: Duration, max_response_bytes: usize) -> Result<Self> {
        validate_timeout(timeout).map_err(|e| UsageError::InvalidPollLimits {
            reason: e.to_string(),
        })?;
        if max_response_bytes == 0 || max_response_bytes > MAX_RESPONSE_BYTES {
            return Err(UsageError::InvalidPollLimits {
                reason: format!(
                    "response cap {max_response_bytes} outside 1..={MAX_RESPONSE_BYTES}"
                ),
            });
        }
        Ok(Self {
            timeout,
            max_response_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_outside_the_probe_window_are_rejected() {
        let cases = [
            (Duration::ZERO, 1024),
            (Duration::from_millis(500), 1024),
            (Duration::from_secs(31), 1024),
            (Duration::from_secs(5), 0),
            (Duration::from_secs(5), MAX_RESPONSE_BYTES + 1),
        ];
        for (timeout, cap) in cases {
            let result = PollLimits::new(timeout, cap);
            assert!(
                matches!(result, Err(UsageError::InvalidPollLimits { .. })),
                "accepted timeout {timeout:?} cap {cap}"
            );
        }
    }

    #[test]
    fn probe_window_edges_are_accepted_as_given() {
        let limits = PollLimits::new(Duration::from_secs(1), 1)
            .unwrap_or_else(|e| panic!("probe floor rejected: {e:?}"));
        assert_eq!(limits.timeout, Duration::from_secs(1));
        assert_eq!(limits.max_response_bytes, 1);
    }
}
