//! Cost-period boundary tests (USG-05): DST day bucketing, leap
//! February, rolling 1..=365 bounds, pinned month-to-date zones.

// Lib deps that ride along as externs of this target without being
// referenced directly.

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;
    use jiff::{Timestamp, Zoned};
    use serde as _;
    use serde_json as _;
    use superai_core as _;
    use superai_usage::cost::{CostError, CostPeriod};
    use thiserror as _;

    fn ts_utc(year: i16, month: i8, day: i8, hour: i8, minute: i8) -> Timestamp {
        jiff::civil::date(year, month, day)
            .at(hour, minute, 0, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap_or_else(|e| panic!("utc datetime: {e}"))
            .timestamp()
    }

    fn ts_zone(zone: &str, year: i16, month: i8, day: i8, hour: i8, minute: i8) -> Timestamp {
        jiff::civil::date(year, month, day)
            .at(hour, minute, 0, 0)
            .in_tz(zone)
            .unwrap_or_else(|e| panic!("{zone} datetime: {e}"))
            .timestamp()
    }

    fn zoned_utc(year: i16, month: i8, day: i8, hour: i8, minute: i8) -> Zoned {
        jiff::civil::date(year, month, day)
            .at(hour, minute, 0, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap_or_else(|e| panic!("utc datetime: {e}"))
    }

    fn start_in_zone(period: &CostPeriod, now: Timestamp, bucket_zone: &str) -> Zoned {
        let range = period
            .range(now, bucket_zone)
            .unwrap_or_else(|e| panic!("range failed: {e}"));
        let start = range
            .start
            .unwrap_or_else(|| panic!("period {period:?} has no start"));
        let zone_name = match period {
            CostPeriod::MonthToDate { time_zone } => time_zone.as_str(),
            _ => bucket_zone,
        };
        start.to_zoned(TimeZone::get(zone_name).unwrap_or_else(|e| panic!("zone: {e}")))
    }

    #[test]
    fn rolling_window_anchors_to_wall_clock_midnight_across_spring_forward() {
        let now = ts_zone("America/New_York", 2026, 3, 10, 12, 0);
        let start = start_in_zone(&CostPeriod::rolling(7).unwrap(), now, "America/New_York");
        assert_eq!(start.datetime().to_string(), "2026-03-04T00:00:00");
        assert_eq!(start.offset().seconds(), -5 * 3600);
        let naive = now
            .checked_sub(jiff::Span::new().hours(6 * 24))
            .unwrap()
            .to_zoned(TimeZone::get("America/New_York").unwrap());
        assert_ne!(
            naive.datetime().to_string(),
            "2026-03-04T00:00:00",
            "a fixed 24h-per-day subtraction misses the 23-hour day"
        );
    }

    #[test]
    fn rolling_one_day_on_the_23h_day_itself_starts_at_that_midnight() {
        let now = ts_zone("America/New_York", 2026, 3, 8, 12, 0);
        let start = start_in_zone(&CostPeriod::rolling(1).unwrap(), now, "America/New_York");
        assert_eq!(start.datetime().to_string(), "2026-03-08T00:00:00");
        assert_eq!(start.offset().seconds(), -5 * 3600);
    }

    #[test]
    fn rolling_window_over_the_25h_day_keeps_whole_reporting_days() {
        let now = ts_zone("America/New_York", 2026, 11, 2, 12, 0);
        let start = start_in_zone(&CostPeriod::rolling(2).unwrap(), now, "America/New_York");
        assert_eq!(start.datetime().to_string(), "2026-11-01T00:00:00");
        assert_eq!(start.offset().seconds(), -4 * 3600);
    }

    #[test]
    fn month_to_date_is_pinned_to_its_configured_zone_not_the_bucket_zone() {
        let now = ts_zone("America/New_York", 2026, 11, 3, 9, 30);
        let period = CostPeriod::MonthToDate {
            time_zone: "America/New_York".to_owned(),
        };
        let start = start_in_zone(&period, now, "UTC");
        assert_eq!(start.datetime().to_string(), "2026-11-01T00:00:00");
        assert_eq!(
            start.offset().seconds(),
            -4 * 3600,
            "November starts on EDT; a UTC-pinned midnight would be offset 0"
        );
    }

    #[test]
    fn month_to_date_covers_the_fall_back_repeated_hour() {
        let period = CostPeriod::MonthToDate {
            time_zone: "America/New_York".to_owned(),
        };
        let now = ts_zone("America/New_York", 2026, 11, 3, 9, 30);
        let range = period.range(now, "UTC").unwrap();
        let first_130_edt = ts_zone("America/New_York", 2026, 11, 1, 1, 30);
        let span = jiff::Span::new().hours(1);
        let repeated_130_est = first_130_edt
            .checked_add(span)
            .unwrap()
            .to_zoned(TimeZone::get("America/New_York").unwrap());
        assert_eq!(
            repeated_130_est.datetime().to_string(),
            "2026-11-01T01:30:00"
        );
        assert_eq!(repeated_130_est.offset().seconds(), -5 * 3600);
        assert!(range.contains(first_130_edt));
        assert!(range.contains(repeated_130_est.timestamp()));
    }

    #[test]
    fn month_to_date_covers_a_leap_february() {
        let period = CostPeriod::MonthToDate {
            time_zone: "UTC".to_owned(),
        };
        let now = ts_utc(2028, 2, 29, 18, 0);
        let range = period.range(now, "UTC").unwrap();
        let start = range.start.unwrap().to_zoned(TimeZone::UTC);
        assert_eq!(start.datetime().to_string(), "2028-02-01T00:00:00");
        assert!(range.contains(ts_utc(2028, 2, 29, 0, 0)));
        assert!(!range.contains(ts_utc(2028, 1, 31, 23, 59)));
    }

    #[test]
    fn month_to_date_in_a_non_leap_february_stops_at_the_28th() {
        let period = CostPeriod::MonthToDate {
            time_zone: "UTC".to_owned(),
        };
        let now = ts_utc(2027, 2, 28, 23, 0);
        let range = period.range(now, "UTC").unwrap();
        assert!(range.contains(ts_utc(2027, 2, 28, 0, 0)));
        assert!(!range.contains(ts_utc(2027, 1, 31, 23, 59)));
    }

    #[test]
    fn month_to_date_rolls_over_a_year_boundary() {
        let period = CostPeriod::MonthToDate {
            time_zone: "UTC".to_owned(),
        };
        let now = ts_utc(2027, 1, 1, 0, 30);
        let range = period.range(now, "UTC").unwrap();
        let start = range.start.unwrap().to_zoned(TimeZone::UTC);
        assert_eq!(start.datetime().to_string(), "2027-01-01T00:00:00");
        assert!(!range.contains(ts_utc(2026, 12, 31, 23, 59)));
    }

    #[test]
    fn rolling_bounds_are_enforced_when_deserialize_bypasses_the_constructor() {
        for days in [0u16, 366] {
            let period = CostPeriod::RollingDays { days };
            let err = period
                .range(zoned_utc(2026, 10, 13, 12, 0).timestamp(), "UTC")
                .unwrap_err();
            assert!(
                matches!(err, CostError::RollingWindowOutOfRange { days: rejected } if rejected == days),
                "days {days}: {err:?}"
            );
        }
        for days in [1u16, 365] {
            let period = CostPeriod::RollingDays { days };
            period
                .range(ts_utc(2026, 10, 13, 12, 0), "UTC")
                .unwrap_or_else(|e| panic!("days {days} must resolve: {e}"));
        }
    }

    #[test]
    fn unknown_zone_names_are_rejected_for_both_zone_roles() {
        let now = ts_utc(2026, 10, 13, 12, 0);
        let rolling_err = CostPeriod::rolling(7)
            .unwrap()
            .range(now, "Not/AZone")
            .unwrap_err();
        assert!(matches!(rolling_err, CostError::UnknownZone { .. }));
        let mtd_err = CostPeriod::MonthToDate {
            time_zone: "Mars/Olympus".to_owned(),
        }
        .range(now, "UTC")
        .unwrap_err();
        assert!(matches!(mtd_err, CostError::UnknownZone { .. }));
    }

    #[test]
    fn all_history_has_no_start_and_ends_at_now() {
        let now = ts_utc(2026, 10, 13, 12, 0);
        let range = CostPeriod::AllHistory.range(now, "UTC").unwrap();
        assert_eq!(range.start, None);
        assert!(range.contains(ts_utc(1999, 1, 1, 0, 0)));
        assert!(!range.contains(ts_utc(2026, 10, 13, 12, 1)));
    }
}
