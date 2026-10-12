//! Cost periods and pricing metadata (USG-05): rolling windows,
//! month-to-date, all-history. Recorded spend is never a bill.

use serde::{Deserialize, Serialize};

use crate::error::{Result, UsageError};

/// One cost-reporting period. A period's total is spend superai recorded,
/// never an invoice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostPeriod {
    /// Last N days, 1..=365.
    RollingDays {
        /// Window length in days.
        days: u16,
    },
    /// Calendar month so far in a named time zone.
    MonthToDate {
        /// IANA zone name the month boundary is pinned to.
        time_zone: String,
    },
    /// Every recorded event.
    AllHistory,
}

impl CostPeriod {
    /// Rolling window; USG-05 bounds the day count to 1..=365.
    pub fn rolling(days: u16) -> Result<Self> {
        if days == 0 || days > 365 {
            return Err(UsageError::RollingWindowOutOfRange { days });
        }
        Ok(Self::RollingDays { days })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_window_rejects_zero_and_days_beyond_365() {
        for days in [0u16, 366, u16::MAX] {
            assert!(matches!(
                CostPeriod::rolling(days),
                Err(UsageError::RollingWindowOutOfRange { days: rejected })
                    if rejected == days
            ));
        }
    }

    #[test]
    fn rolling_window_accepts_both_documented_edges() {
        for days in [1u16, 365] {
            assert!(matches!(
                CostPeriod::rolling(days),
                Ok(CostPeriod::RollingDays { days: accepted }) if accepted == days
            ));
        }
    }
}
