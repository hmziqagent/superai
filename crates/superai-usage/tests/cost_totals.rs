//! Cost-engine total tests (USG-05): exact decimal sums, the
//! unavailable/zero/incomplete distinction, recorded-spend semantics.

// Lib deps that ride along as externs of this target without being
// referenced directly.

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use rusqlite as _;
    use serde as _;
    use superai_core as _;
    use superai_usage::cost::{
        CostEngine, CostError, CostPeriod, ModelRates, RateTable, RecordedUsage, ScanCoverage,
        Spend, UsageEvent, Usd,
    };
    use thiserror as _;
    use ureq as _;

    fn ts_utc(year: i16, month: i8, day: i8, hour: i8, minute: i8) -> Timestamp {
        jiff::civil::date(year, month, day)
            .at(hour, minute, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap_or_else(|e| panic!("utc datetime: {e}"))
            .timestamp()
    }

    fn event(
        timestamp: Timestamp,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> UsageEvent {
        UsageEvent {
            timestamp,
            provider: provider.to_owned(),
            model: model.to_owned(),
            input_tokens,
            output_tokens,
            cache_read_tokens: None,
            cache_write_tokens: None,
        }
    }

    fn bundled_engine() -> CostEngine {
        CostEngine::new(RateTable::bundled().unwrap_or_else(|e| panic!("bundled: {e}")))
    }

    fn all_history() -> CostPeriod {
        CostPeriod::AllHistory
    }

    fn total(usage: &RecordedUsage) -> superai_usage::cost::PeriodReport {
        let now = ts_utc(2026, 10, 13, 12, 0);
        bundled_engine()
            .total(&all_history(), now, "UTC", usage)
            .unwrap_or_else(|e| panic!("total failed: {e}"))
    }

    #[test]
    fn one_million_tokens_costs_exactly_the_per_million_rate() {
        let table = RateTable::bundled().unwrap();
        for (provider, model) in [
            ("anthropic", "claude-sonnet-4-5"),
            ("anthropic", "claude-haiku-4-5"),
            ("anthropic", "claude-opus-4-5"),
            ("openai", "gpt-5.1"),
            ("openai", "gpt-5-mini"),
        ] {
            let rates = table
                .lookup(provider, model)
                .unwrap_or_else(|| panic!("{provider}/{model} missing from bundled table"));
            let usage = RecordedUsage {
                events: vec![
                    event(ts_utc(2026, 10, 13, 1, 0), provider, model, 1_000_000, 0),
                    event(ts_utc(2026, 10, 13, 2, 0), provider, model, 0, 1_000_000),
                ],
                unscanned_records: 0,
            };
            let report = total(&usage);
            match &report.total.spend {
                Spend::Exact(sum) => {
                    let expected = rates
                        .input
                        .checked_add(rates.output)
                        .unwrap_or_else(|| panic!("test rate sum overflow"));
                    assert_eq!(*sum, expected, "{provider}/{model}");
                }
                other => panic!("{provider}/{model}: expected Exact, got {other:?}"),
            }
        }
    }

    #[test]
    fn sub_million_token_counts_stay_exact_decimals() {
        let usage = RecordedUsage {
            events: vec![event(
                ts_utc(2026, 10, 13, 1, 0),
                "anthropic",
                "claude-sonnet-4-5",
                1,
                0,
            )],
            unscanned_records: 0,
        };
        let report = total(&usage);
        match &report.total.spend {
            Spend::Exact(sum) => assert_eq!(sum.to_string(), "0.000003"),
            other => panic!("expected Exact, got {other:?}"),
        }
    }

    #[test]
    fn fractional_rates_sum_without_binary_drift() {
        let usage = RecordedUsage {
            events: vec![event(
                ts_utc(2026, 10, 13, 1, 0),
                "openai",
                "gpt-5-mini",
                100_000,
                100_000,
            )],
            unscanned_records: 0,
        };
        let report = total(&usage);
        match &report.total.spend {
            Spend::Exact(sum) => assert_eq!(
                sum.to_string(),
                "0.225",
                "0.25/mtok over 100k plus 2/mtok over 100k must be exactly 0.025 + 0.2"
            ),
            other => panic!("expected Exact, got {other:?}"),
        }
    }

    #[test]
    fn no_matching_events_is_zero_while_recorded_free_usage_is_exact_zero() {
        let empty = RecordedUsage::default();
        assert_eq!(total(&empty).total.spend, Spend::Zero);

        let free = RecordedUsage {
            events: vec![event(
                ts_utc(2026, 10, 13, 1, 0),
                "anthropic",
                "claude-sonnet-4-5",
                0,
                0,
            )],
            unscanned_records: 0,
        };
        assert_eq!(total(&free).total.spend, Spend::Exact(Usd::ZERO));
    }

    #[test]
    fn missing_pricing_yields_unavailable_with_the_model_named() {
        let usage = RecordedUsage {
            events: vec![
                event(
                    ts_utc(2026, 10, 13, 1, 0),
                    "anthropic",
                    "claude-sonnet-4-5",
                    1_000_000,
                    1_000_000,
                ),
                event(ts_utc(2026, 10, 13, 2, 0), "xai", "grok-unknown", 500, 500),
                event(ts_utc(2026, 10, 13, 3, 0), "xai", "grok-unknown", 5, 5),
                event(ts_utc(2026, 10, 13, 4, 0), "zai", "glm-future", 1, 1),
            ],
            unscanned_records: 0,
        };
        let report = total(&usage);
        match &report.total.spend {
            Spend::Unavailable { missing_pricing } => {
                assert_eq!(
                    missing_pricing,
                    &["xai/grok-unknown".to_owned(), "zai/glm-future".to_owned()]
                );
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
        let priced_row = report
            .models
            .iter()
            .find(|row| row.model == "claude-sonnet-4-5")
            .unwrap_or_else(|| panic!("priced row dropped: {:?}", report.models));
        assert_eq!(
            priced_row.cost.map(|c| c.to_string()),
            Some("18".to_owned())
        );
        let unpriced_row = report
            .models
            .iter()
            .find(|row| row.model == "grok-unknown")
            .unwrap_or_else(|| panic!("unpriced row dropped"));
        assert_eq!(unpriced_row.cost, None);
        assert_eq!(unpriced_row.events, 2);
    }

    #[test]
    fn consumed_cache_class_without_a_rate_also_names_the_model() {
        let mut table = RateTable::default();
        table.insert(
            "test",
            "no-cache-rate",
            ModelRates {
                input: Usd::parse_rate_per_mtok("1").unwrap(),
                output: Usd::parse_rate_per_mtok("2").unwrap(),
                cache_read: None,
                cache_write: None,
            },
        );
        let mut usage_event = event(
            ts_utc(2026, 10, 13, 1, 0),
            "test",
            "no-cache-rate",
            1_000,
            1_000,
        );
        usage_event.cache_read_tokens = Some(500);
        let usage = RecordedUsage {
            events: vec![usage_event],
            unscanned_records: 0,
        };
        let report = CostEngine::new(table)
            .total(&all_history(), ts_utc(2026, 10, 13, 12, 0), "UTC", &usage)
            .unwrap();
        match &report.total.spend {
            Spend::Unavailable { missing_pricing } => {
                assert_eq!(missing_pricing, &["test/no-cache-rate".to_owned()]);
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn cache_class_tokens_price_exactly_when_rates_exist() {
        let mut usage_event = event(
            ts_utc(2026, 10, 13, 1, 0),
            "anthropic",
            "claude-sonnet-4-5",
            1_000_000,
            1_000_000,
        );
        usage_event.cache_read_tokens = Some(1_000_000);
        usage_event.cache_write_tokens = Some(1_000_000);
        let usage = RecordedUsage {
            events: vec![usage_event],
            unscanned_records: 0,
        };
        let report = total(&usage);
        match &report.total.spend {
            Spend::Exact(sum) => assert_eq!(
                sum.to_string(),
                "22.05",
                "3 + 15 + 0.3 cache read + 3.75 cache write"
            ),
            other => panic!("expected Exact, got {other:?}"),
        }
    }

    #[test]
    fn unscanned_records_mark_the_total_incomplete_without_erasing_spend() {
        let usage = RecordedUsage {
            events: vec![event(
                ts_utc(2026, 10, 13, 1, 0),
                "anthropic",
                "claude-sonnet-4-5",
                1_000_000,
                0,
            )],
            unscanned_records: 7,
        };
        let report = total(&usage);
        assert_eq!(
            report.total.coverage,
            ScanCoverage::Incomplete {
                unscanned_records: 7
            }
        );
        assert_eq!(report.total.spend, Spend::Exact(Usd::parse("3").unwrap()));
    }

    #[test]
    fn events_outside_the_period_are_excluded() {
        let usage = RecordedUsage {
            events: vec![
                event(
                    ts_utc(2026, 10, 13, 11, 0),
                    "anthropic",
                    "claude-sonnet-4-5",
                    1_000_000,
                    0,
                ),
                event(
                    ts_utc(2026, 10, 11, 23, 0),
                    "anthropic",
                    "claude-sonnet-4-5",
                    1_000_000,
                    0,
                ),
            ],
            unscanned_records: 0,
        };
        let now = ts_utc(2026, 10, 13, 12, 0);
        let rolling = bundled_engine()
            .total(&CostPeriod::rolling(1).unwrap(), now, "UTC", &usage)
            .unwrap();
        assert_eq!(rolling.total.spend, Spend::Exact(Usd::parse("3").unwrap()));
        let history = bundled_engine()
            .total(&all_history(), now, "UTC", &usage)
            .unwrap();
        assert_eq!(history.total.spend, Spend::Exact(Usd::parse("6").unwrap()));
    }

    #[test]
    fn period_report_serializes_with_exact_decimal_strings() {
        let usage = RecordedUsage {
            events: vec![event(
                ts_utc(2026, 10, 13, 1, 0),
                "anthropic",
                "claude-sonnet-4-5",
                100_000,
                100_000,
            )],
            unscanned_records: 3,
        };
        let report = total(&usage);
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            json.contains("\"cost\":\"1.8\""),
            "costs must travel as exact strings, got: {json}"
        );
        assert!(json.contains("\"exact\":\"1.8\""), "got: {json}");
        assert!(
            json.contains("\"incomplete\":{\"unscanned_records\":3}"),
            "got: {json}"
        );
        let back: superai_usage::cost::PeriodReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
    }

    #[test]
    fn cost_arithmetic_overflow_is_an_error_not_a_wrong_number() {
        let mut table = RateTable::default();
        table.insert(
            "test",
            "huge",
            ModelRates {
                input: Usd::parse_rate_per_mtok("1e18").unwrap(),
                output: Usd::parse_rate_per_mtok("1e18").unwrap(),
                cache_read: None,
                cache_write: None,
            },
        );
        let usage = RecordedUsage {
            events: vec![event(
                ts_utc(2026, 10, 13, 1, 0),
                "test",
                "huge",
                u64::MAX,
                0,
            )],
            unscanned_records: 0,
        };
        let err = CostEngine::new(table)
            .total(&all_history(), ts_utc(2026, 10, 13, 12, 0), "UTC", &usage)
            .unwrap_err();
        assert!(matches!(err, CostError::CostOverflow), "{err:?}");
    }

    #[test]
    fn a_rate_that_cannot_convert_per_token_exactly_is_reported() {
        let mut table = RateTable::default();
        table.insert(
            "test",
            "hand-edited",
            ModelRates {
                input: Usd::from_picos(1),
                output: Usd::ZERO,
                cache_read: None,
                cache_write: None,
            },
        );
        let usage = RecordedUsage {
            events: vec![event(
                ts_utc(2026, 10, 13, 1, 0),
                "test",
                "hand-edited",
                1,
                0,
            )],
            unscanned_records: 0,
        };
        let err = CostEngine::new(table)
            .total(&all_history(), ts_utc(2026, 10, 13, 12, 0), "UTC", &usage)
            .unwrap_err();
        assert!(
            matches!(err, CostError::RateNotConvertible { ref provider, ref model } if provider == "test" && model == "hand-edited"),
            "{err:?}"
        );
    }

    #[test]
    fn model_rows_are_sorted_by_provider_then_model() {
        let usage = RecordedUsage {
            events: vec![
                event(ts_utc(2026, 10, 13, 1, 0), "openai", "gpt-5.1", 1, 1),
                event(
                    ts_utc(2026, 10, 13, 2, 0),
                    "anthropic",
                    "claude-opus-4-5",
                    1,
                    1,
                ),
                event(
                    ts_utc(2026, 10, 13, 3, 0),
                    "anthropic",
                    "claude-haiku-4-5",
                    1,
                    1,
                ),
            ],
            unscanned_records: 0,
        };
        let report = total(&usage);
        let keys: Vec<String> = report
            .models
            .iter()
            .map(|row| format!("{}/{}", row.provider, row.model))
            .collect();
        assert_eq!(
            keys,
            [
                "anthropic/claude-haiku-4-5",
                "anthropic/claude-opus-4-5",
                "openai/gpt-5.1"
            ]
        );
    }
}
