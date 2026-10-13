//! models.dev catalog parsing and merge-semantics tests (USG-05).

// Lib deps that ride along as externs of this target without being
// referenced directly.

#[cfg(test)]
mod tests {
    use jiff as _;
    use rusqlite as _;
    use serde as _;
    use serde_json as _;
    use superai_core as _;
    use thiserror as _;
    use ureq as _;

    use superai_usage::cost::{CostError, ModelRates, RateTable, Usd};

    const FLAT_CATALOG: &str = r#"{
  "anthropic": {
    "id": "anthropic",
    "name": "Anthropic",
    "models": {
      "claude-sonnet-4-5": {
        "name": "Claude Sonnet 4.5",
        "cost": { "input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75 }
      },
      "claude-tiny": { "name": "announced but unpriced" },
      "claude-garbled": { "cost": { "input": "oops", "output": 2 } },
      "claude-odd-precision": { "cost": { "input": 0.1234567, "output": 1 } }
    }
  },
  "OpenAI ": {
    "id": "OpenAI ",
    "models": {
      "gpt-5.1": { "cost": { "input": 1.25, "output": 10, "cache_read": 0.125 } },
      "gpt-reseller": { "cost": { "input": 4.2929, "output": 21.4646 } }
    }
  },
  "empty-provider": { "models": {} }
}"#;

    #[test]
    fn parses_raw_json_numbers_as_exact_decimals() {
        let table = RateTable::from_models_dev_json(FLAT_CATALOG.as_bytes())
            .unwrap_or_else(|e| panic!("parse failed: {e}"));
        let sonnet = table
            .lookup("anthropic", "claude-sonnet-4-5")
            .unwrap_or_else(|| panic!("sonnet missing"));
        assert_eq!(sonnet.input.to_string(), "3");
        assert_eq!(sonnet.output.to_string(), "15");
        assert_eq!(sonnet.cache_read.map(|r| r.to_string()), Some("0.3".into()));
        assert_eq!(
            sonnet.cache_write.map(|r| r.to_string()),
            Some("3.75".into())
        );
        let reseller = table.lookup("openai", "gpt-reseller").unwrap();
        assert_eq!(reseller.input.to_string(), "4.2929");
        assert_eq!(reseller.output.to_string(), "21.4646");
    }

    #[test]
    fn unpriceable_and_over_precise_models_are_skipped_not_fatal() {
        let table = RateTable::from_models_dev_json(FLAT_CATALOG.as_bytes()).unwrap();
        assert!(table.lookup("anthropic", "claude-tiny").is_none());
        assert!(table.lookup("anthropic", "claude-garbled").is_none());
        assert!(table.lookup("anthropic", "claude-odd-precision").is_none());
        assert!(
            table.lookup("anthropic", "claude-sonnet-4-5").is_some(),
            "the good models in the same provider must survive the skips"
        );
    }

    #[test]
    fn provider_ids_are_normalized_for_lookup() {
        let table = RateTable::from_models_dev_json(FLAT_CATALOG.as_bytes()).unwrap();
        assert!(
            table.lookup("openai", "gpt-5.1").is_some(),
            "the catalog's `OpenAI ` id must be reachable as `openai`"
        );
        assert!(table.lookup("OpenAI", "gpt-5.1").is_none());
    }

    #[test]
    fn a_providers_wrapper_key_parses_too() {
        let wrapped = format!("{{\"providers\": {FLAT_CATALOG}}}");
        let table = RateTable::from_models_dev_json(wrapped.as_bytes()).unwrap();
        assert!(table.lookup("anthropic", "claude-sonnet-4-5").is_some());
    }

    #[test]
    fn garbage_json_is_an_error() {
        for bytes in [&b"not json at all"[..], &b""[..], &b"[1,2,3]"[..]] {
            assert!(matches!(
                RateTable::from_models_dev_json(bytes),
                Err(CostError::CatalogMalformed { .. })
            ));
        }
    }

    #[test]
    fn merge_additive_lets_the_remote_catalog_win_and_bundled_fill_gaps() {
        let mut remote = RateTable::default();
        remote.insert(
            "anthropic",
            "claude-sonnet-4-5",
            ModelRates {
                input: Usd::parse_rate_per_mtok("9.99").unwrap(),
                output: Usd::parse_rate_per_mtok("99").unwrap(),
                cache_read: None,
                cache_write: None,
            },
        );
        remote.insert(
            "moonshot",
            "kimi-k3",
            ModelRates {
                input: Usd::parse_rate_per_mtok("0.6").unwrap(),
                output: Usd::parse_rate_per_mtok("2.5").unwrap(),
                cache_read: None,
                cache_write: None,
            },
        );
        let merged = RateTable::bundled().unwrap().merge_additive(&remote);
        assert_eq!(
            merged
                .lookup("anthropic", "claude-sonnet-4-5")
                .unwrap()
                .input
                .to_string(),
            "9.99",
            "the fetched entry must override the bundled fallback"
        );
        assert!(
            merged.lookup("anthropic", "claude-opus-4-5").is_some(),
            "bundled-only models must survive the merge"
        );
        assert!(merged.lookup("moonshot", "kimi-k3").is_some());
        assert!(
            merged.lookup("anthropic", "claude-not-real").is_none(),
            "unknown models stay unknown"
        );
    }

    fn bundled_rows() -> Vec<(String, String, ModelRates)> {
        let table = RateTable::bundled().unwrap_or_else(|e| panic!("bundled table invalid: {e}"));
        let json =
            serde_json::to_value(&table).unwrap_or_else(|e| panic!("serialize bundled table: {e}"));
        let providers = json
            .get("providers")
            .and_then(|value| value.as_object())
            .unwrap_or_else(|| panic!("bundled table must serialize as a providers map"));
        let mut rows = Vec::new();
        for (provider, models) in providers {
            for (model, raw) in models
                .as_object()
                .unwrap_or_else(|| panic!("provider `{provider}` must serialize as a model map"))
            {
                let rates = serde_json::from_value::<ModelRates>(raw.clone())
                    .unwrap_or_else(|e| panic!("bundled `{provider}/{model}`: {e}"));
                rows.push((provider.clone(), model.clone(), rates));
            }
        }
        rows
    }

    #[test]
    fn bundled_rates_round_trip_and_writes_outprice_reads() {
        let rows = bundled_rows();
        assert!(
            !rows.is_empty(),
            "the bundled table must carry priced models"
        );
        for (provider, model, rates) in &rows {
            for (label, rate) in [
                ("input", rates.input),
                ("output", rates.output),
                ("cache_read", rates.cache_read.unwrap_or_default()),
                ("cache_write", rates.cache_write.unwrap_or_default()),
            ] {
                let text = rate.to_string();
                assert_eq!(
                    Usd::parse(&text).ok(),
                    Some(rate),
                    "{provider}/{model}: `{label}` must re-parse its own display form"
                );
            }
            if let (Some(read), Some(write)) = (rates.cache_read, rates.cache_write) {
                assert!(
                    write.as_picos() > read.as_picos(),
                    "{provider}/{model}: cache writes must outprice cache reads"
                );
            }
        }
    }

    #[test]
    fn merge_keeps_the_remote_override_and_every_other_bundled_row() {
        let table = RateTable::bundled().unwrap();
        assert_eq!(
            table.merge_additive(&RateTable::default()),
            table,
            "an empty remote catalog must not disturb any bundled row"
        );
        let mut remote = RateTable::default();
        remote.insert(
            "anthropic",
            "claude-sonnet-4-5",
            ModelRates {
                input: Usd::parse_rate_per_mtok("9.99").unwrap(),
                output: Usd::parse_rate_per_mtok("99").unwrap(),
                cache_read: None,
                cache_write: None,
            },
        );
        let merged = table.merge_additive(&remote);
        assert_eq!(
            merged.lookup("anthropic", "claude-sonnet-4-5"),
            remote.lookup("anthropic", "claude-sonnet-4-5"),
            "the remote entry must win over the bundled row"
        );
        for (provider, model, rates) in bundled_rows() {
            if provider == "anthropic" && model == "claude-sonnet-4-5" {
                continue;
            }
            assert_eq!(
                merged.lookup(&provider, &model),
                Some(&rates),
                "bundled `{provider}/{model}` must survive the merge unchanged"
            );
        }
    }
}
