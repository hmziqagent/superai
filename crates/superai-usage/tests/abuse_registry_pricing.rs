//! Abuse tests for the usage registry and pricing paths: secret bytes in
//! data-file fields a provider never asked for, and in fetched catalog
//! entries, never reach registry output, the pricing cache, or cost rows.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use jiff::Timestamp;
    use rusqlite as _;
    use serde as _;
    use serde_json::json;
    use superai_core as _;
    use superai_usage::abuse::{
        API_KEY_SENTINEL, BEARER_SENTINEL, COOKIE_SENTINEL, LLMPROXY_ADMIN_SENTINEL,
        assert_no_sentinel_bytes, assert_no_sentinel_debug, assert_no_sentinel_json,
    };
    use superai_usage::cost::{
        CostEngine, CostPeriod, PricingCache, RecordedUsage, Spend, UsageEvent,
    };
    use superai_usage::registry::UsageRegistry;
    use thiserror as _;
    use ureq as _;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-abuse-registry-{tag}-{}-{n}-{nanos}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
        dir
    }

    #[test]
    fn registry_data_files_with_secret_junk_fields_stay_out_of_output() {
        let dir = temp_dir("data");
        fs::write(
            dir.join("provider.json"),
            json!({
                "id": "abuse-probe",
                "label": "Abuse Probe",
                "locators": [],
                // Declares WHERE the llmproxy admin token lives, never its value;
                // the value itself is what USG-08 must plant and scan for.
                "auth_material": [{"kind": "token_file", "template": "~/.llmproxy/admin-token"}],
                "window_strategy": "cost_only",
                "pricing": "not_priced",
                "schema_version": 1,
                "apiKey": API_KEY_SENTINEL,
                "sessionCookie": COOKIE_SENTINEL,
                "adminToken": LLMPROXY_ADMIN_SENTINEL,
                "bearer": BEARER_SENTINEL
            })
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("write: {e}"));

        let registry = UsageRegistry::load_dir(&dir).unwrap_or_else(|e| panic!("load_dir: {e:?}"));

        assert_eq!(registry.len(), 1);
        assert_no_sentinel_debug(&registry, "usage registry");
        let entries: Vec<_> = registry.entries().collect();
        assert_no_sentinel_json(&entries, "usage registry json");
        for entry in registry.entries() {
            assert_no_sentinel_debug(entry, "registry entry debug");
        }
    }

    #[test]
    fn fetched_catalog_with_secret_entries_never_reaches_table_or_cache() {
        let dir = temp_dir("pricing");
        let catalog_path = dir.join("api.json");
        fs::write(
            &catalog_path,
            json!({
                "anthropic": {
                    "id": "anthropic",
                    "models": {"claude-sonnet-4-5": {"cost": {"input": 3, "output": 15}}}
                },
                "openai": {
                    "id": "openai",
                    "models": {"gpt-5.1": {"cost": {"input": 1.25, "output": 10}}}
                },
                "abuse-junk": {
                    "id": "abuse-junk",
                    "apiKey": API_KEY_SENTINEL,
                    "models": {
                        "m1": {
                            "cost": {
                                "input": API_KEY_SENTINEL,
                                "output": "2",
                                "sessionCookie": COOKIE_SENTINEL
                            }
                        }
                    }
                }
            })
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("write: {e}"));

        let url = format!("file://{}", catalog_path.display());
        let now = Timestamp::from_second(1_800_000_000).unwrap_or_else(|e| panic!("now: {e:?}"));
        let cache = PricingCache::new(&dir);
        let cached = cache
            .refresh(&url, now)
            .unwrap_or_else(|e| panic!("refresh: {e:?}"));

        assert_no_sentinel_debug(&cached.rates, "fetched rate table");
        let cache_bytes = fs::read(cache.path()).unwrap_or_else(|e| panic!("read cache: {e}"));
        assert_no_sentinel_bytes(&cache_bytes, "pricing cache file");
        let fresh = cache.load().unwrap_or_else(|| panic!("reload failed"));
        assert_no_sentinel_debug(&fresh, "reloaded cache");

        let event = UsageEvent {
            timestamp: Timestamp::from_second(1_799_000_000)
                .unwrap_or_else(|e| panic!("ts: {e:?}")),
            provider: "anthropic".to_owned(),
            model: "claude-sonnet-4-5".to_owned(),
            input_tokens: 100,
            output_tokens: 40,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        let report = CostEngine::new(cached.rates)
            .total(
                &CostPeriod::AllHistory,
                now,
                "UTC",
                &RecordedUsage {
                    events: vec![event],
                    unscanned_records: 0,
                },
            )
            .unwrap_or_else(|e| panic!("total: {e:?}"));
        assert!(
            matches!(report.total.spend, Spend::Exact(_)),
            "the junk provider must not poison real pricing"
        );
        assert_no_sentinel_debug(&report, "pricing abuse report");
        assert_no_sentinel_json(&report, "pricing abuse report json");
    }
}
