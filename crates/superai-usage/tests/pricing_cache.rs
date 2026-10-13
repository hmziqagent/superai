//! Pricing-cache tests (USG-05): TTL, atomic writes, unknown-model
//! cooldown, plausibility gate, bounded fetch, cache-root isolation.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    // Lib deps that ride along as externs of this target without being
    // referenced directly.
    use jiff::Timestamp;
    use rusqlite as _;
    use serde as _;
    use serde_json as _;
    use superai_core as _;
    use thiserror as _;
    use ureq as _;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-usage-{tag}-{}-{n}-{nanos}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
        dir
    }

    fn ts_utc(year: i16, month: i8, day: i8, hour: i8, minute: i8) -> Timestamp {
        jiff::civil::date(year, month, day)
            .at(hour, minute, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap_or_else(|e| panic!("utc datetime: {e}"))
            .timestamp()
    }

    use superai_usage::cost::{
        CostError, Freshness, MAX_CACHE_FILE_BYTES, MAX_MODELS_DEV_BYTES, PricingCache, RateTable,
        fetch_models_dev_bytes,
    };

    const PLAUSIBLE_CATALOG: &str = r#"{
  "anthropic": { "models": { "claude-sonnet-4-5": { "cost": { "input": 3, "output": 15 } } } },
  "openai": { "models": { "gpt-5.1": { "cost": { "input": 1.25, "output": 10 } } } }
}"#;

    fn now() -> Timestamp {
        ts_utc(2026, 10, 13, 12, 0)
    }

    fn hours_before(base: Timestamp, hours: i64) -> Timestamp {
        base.checked_sub(jiff::Span::new().hours(hours))
            .unwrap_or_else(|e| panic!("time math: {e}"))
    }

    fn minutes_before(base: Timestamp, minutes: i64) -> Timestamp {
        base.checked_sub(jiff::Span::new().minutes(minutes))
            .unwrap_or_else(|e| panic!("time math: {e}"))
    }

    fn minutes_after(base: Timestamp, minutes: i64) -> Timestamp {
        base.checked_add(jiff::Span::new().minutes(minutes))
            .unwrap_or_else(|e| panic!("time math: {e}"))
    }

    fn sample_table() -> RateTable {
        RateTable::from_models_dev_json(PLAUSIBLE_CATALOG.as_bytes())
            .unwrap_or_else(|e| panic!("fixture catalog: {e}"))
    }

    fn file_url(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
            .map(|entry| {
                entry
                    .unwrap_or_else(|e| panic!("dir entry: {e}"))
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    fn tree_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            snapshot_dir(&dir, &mut out, &mut stack);
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn snapshot_dir(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>, stack: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir: {e}")) {
            let entry = entry.unwrap_or_else(|e| panic!("entry: {e}"));
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push((
                    path.clone(),
                    fs::read(&path).unwrap_or_else(|e| panic!("read: {e}")),
                ));
            }
        }
    }

    #[test]
    fn store_then_load_round_trips_fetched_at_and_rates() {
        let dir = temp_dir("cache-roundtrip");
        let cache = PricingCache::new(&dir);
        let fetched_at = hours_before(now(), 1);
        cache
            .store(&sample_table(), fetched_at)
            .unwrap_or_else(|e| panic!("store: {e}"));
        let loaded = cache
            .load()
            .unwrap_or_else(|| panic!("cache must load right after store"));
        assert_eq!(loaded.fetched_at, fetched_at);
        assert_eq!(loaded.rates, sample_table());
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn freshness_transitions_at_the_24_hour_ttl() {
        let dir = temp_dir("cache-ttl");
        let cache = PricingCache::new(&dir);
        assert_eq!(cache.freshness(now()), Freshness::Missing);
        cache
            .store(&sample_table(), hours_before(now(), 23))
            .unwrap();
        assert_eq!(
            cache.freshness(now()),
            Freshness::Fresh {
                fetched_at: hours_before(now(), 23)
            }
        );
        cache
            .store(&sample_table(), hours_before(now(), 25))
            .unwrap();
        assert_eq!(
            cache.freshness(now()),
            Freshness::Stale {
                fetched_at: hours_before(now(), 25)
            },
            "a stale cache stays loadable; only its freshness state changes"
        );
        assert!(cache.load().is_some());
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn corrupt_oversized_and_wrong_schema_caches_read_as_missing() {
        let dir = temp_dir("cache-corrupt");
        let cache = PricingCache::new(&dir);
        fs::create_dir_all(cache.path().parent().unwrap()).unwrap();

        fs::write(cache.path(), b"not json").unwrap();
        assert_eq!(cache.freshness(now()), Freshness::Missing);

        fs::write(
            cache.path(),
            r#"{"schema_version": 99, "fetched_at": "2026-10-13T00:00:00Z", "rates": {}}"#,
        )
        .unwrap();
        assert!(cache.load().is_none());

        let oversized = fs::File::create(cache.path()).unwrap();
        oversized.set_len(MAX_CACHE_FILE_BYTES as u64 + 1).unwrap();
        drop(oversized);
        assert!(cache.load().is_none());
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn store_leaves_no_temp_files_and_failed_refresh_keeps_the_old_cache() {
        let dir = temp_dir("cache-atomic");
        let cache = PricingCache::new(&dir);
        cache
            .store(&sample_table(), hours_before(now(), 2))
            .unwrap();
        let cache_dir = cache.path().parent().unwrap();
        assert_eq!(listing(cache_dir), vec!["models-dev.json".to_owned()]);

        let before = fs::read(cache.path()).unwrap();
        let bad_fixture = dir.join("implausible.json");
        fs::write(&bad_fixture, br#"{"anthropic": {"models": {}}}"#).unwrap();
        let err = cache.refresh(&file_url(&bad_fixture), now()).unwrap_err();
        assert!(
            matches!(err, CostError::ImplausibleCatalog { .. }),
            "{err:?}"
        );
        assert_eq!(fs::read(cache.path()).unwrap(), before);
        assert_eq!(
            listing(cache_dir),
            vec![
                "models-dev.json".to_owned(),
                "models-dev.last-attempt".to_owned()
            ],
            "a failed refresh leaves the cache plus its attempt stamp, never a temp file"
        );
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn unknown_model_cooldown_runs_from_the_effective_fetch_time() {
        let dir = temp_dir("cache-cooldown");
        let cache = PricingCache::new(&dir);
        assert!(
            cache.unknown_model_refresh_allowed(now()),
            "with no cache and no recorded attempt a refresh is always allowed"
        );
        cache
            .store(&sample_table(), minutes_before(now(), 5))
            .unwrap();
        assert!(
            !cache.unknown_model_refresh_allowed(now()),
            "5 minutes after the last fetch the cooldown must hold"
        );
        cache
            .store(&sample_table(), hours_before(now(), 15))
            .unwrap();
        assert!(
            cache.unknown_model_refresh_allowed(now()),
            "the cooldown is exactly 15 minutes"
        );
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn a_failed_attempt_paces_the_cooldown_even_with_no_cache() {
        let dir = temp_dir("cache-cooldown-attempt");
        let cache = PricingCache::new(&dir);
        let missing = dir.join("does-not-exist.json");
        let err = cache
            .refresh(&file_url(&missing), minutes_before(now(), 5))
            .unwrap_err();
        assert!(matches!(err, CostError::Fetch { .. }), "{err:?}");
        assert!(cache.load().is_none());
        assert!(
            !cache.unknown_model_refresh_allowed(now()),
            "5 minutes after a failed attempt the cooldown must hold with no cache at all"
        );
        assert!(
            cache.unknown_model_refresh_allowed(minutes_after(now(), 11)),
            "the cooldown from the attempt is the same 15 minutes"
        );
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn a_newer_failed_attempt_re_arms_the_cooldown_over_an_older_cache() {
        let dir = temp_dir("cache-cooldown-rearm");
        let cache = PricingCache::new(&dir);
        cache
            .store(&sample_table(), hours_before(now(), 1))
            .unwrap();
        assert!(
            cache.unknown_model_refresh_allowed(now()),
            "an hour-old cache no longer paces refreshes"
        );
        let missing = dir.join("nope.json");
        cache
            .refresh(&file_url(&missing), minutes_before(now(), 2))
            .unwrap_err();
        assert!(
            !cache.unknown_model_refresh_allowed(now()),
            "the later attempt, not the older cache, must anchor the cooldown"
        );
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn refresh_from_a_file_url_stores_a_fresh_loadable_cache() {
        let dir = temp_dir("cache-refresh");
        let cache = PricingCache::new(&dir);
        let fixture = dir.join("api.json");
        fs::write(&fixture, PLAUSIBLE_CATALOG.as_bytes()).unwrap();
        let cached = cache
            .refresh(&file_url(&fixture), now())
            .unwrap_or_else(|e| panic!("refresh: {e}"));
        assert_eq!(cached.fetched_at, now());
        assert_eq!(
            cache.freshness(now()),
            Freshness::Fresh { fetched_at: now() }
        );
        let loaded = cache.load().unwrap();
        assert!(
            loaded
                .rates
                .lookup("anthropic", "claude-sonnet-4-5")
                .is_some()
        );
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn a_missing_fixture_file_fails_the_refresh_without_touching_the_cache() {
        let dir = temp_dir("cache-missing-fetch");
        let cache = PricingCache::new(&dir);
        cache
            .store(&sample_table(), hours_before(now(), 1))
            .unwrap();
        let before = fs::read(cache.path()).unwrap();
        let missing = dir.join("does-not-exist.json");
        let err = cache.refresh(&file_url(&missing), now()).unwrap_err();
        assert!(matches!(err, CostError::Fetch { .. }), "{err:?}");
        assert_eq!(fs::read(cache.path()).unwrap(), before);
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn fetch_rejects_http_private_hosts_and_traversal() {
        for url in [
            "http://models.dev/api.json",
            "https://127.0.0.1/api.json",
            "https://models.dev/../api.json",
            "https://10.0.0.5/api.json",
            "ftp://models.dev/api.json",
        ] {
            assert!(
                fetch_models_dev_bytes(url).is_err(),
                "{url} must be refused before any request is made"
            );
        }
    }

    #[test]
    fn fetch_enforces_the_size_cap_on_local_files() {
        let dir = temp_dir("cache-fetch-cap");
        let big = dir.join("big.json");
        let mut payload = Vec::new();
        payload.push(b'[');
        payload.resize(MAX_MODELS_DEV_BYTES + 1, b'1');
        payload.push(b']');
        fs::write(&big, &payload).unwrap();
        let err = fetch_models_dev_bytes(&file_url(&big)).unwrap_err();
        assert!(matches!(err, CostError::FetchSizeLimit { .. }), "{err:?}");
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn a_local_file_at_exactly_the_cap_still_loads() {
        let dir = temp_dir("cache-fetch-cap-edge");
        let edge = dir.join("edge.json");
        let mut payload = Vec::new();
        payload.push(b'[');
        payload.resize(MAX_MODELS_DEV_BYTES - 1, b'1');
        payload.push(b']');
        fs::write(&edge, &payload).unwrap();
        let bytes = fetch_models_dev_bytes(&file_url(&edge))
            .unwrap_or_else(|e| panic!("edge-size fetch: {e}"));
        assert_eq!(bytes.len(), MAX_MODELS_DEV_BYTES);
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn the_default_cache_root_is_superai_owned_never_a_harness_dir() {
        let xdg = Path::new("/tmp/xdg-cache-home-fixture");
        let home = Path::new("/home fixture/user");
        assert_eq!(
            PricingCache::default_root(Some(xdg), home).unwrap(),
            xdg.join("superai")
        );
        assert_eq!(
            PricingCache::default_root(None, home).unwrap(),
            home.join(".cache").join("superai")
        );
        for ignored_xdg in [Path::new("relative/xdg"), Path::new("")] {
            assert_eq!(
                PricingCache::default_root(Some(ignored_xdg), home).unwrap(),
                home.join(".cache").join("superai"),
                "a relative or empty XDG_CACHE_HOME must be ignored per the XDG rule"
            );
        }
        let cache = PricingCache::new(&PricingCache::default_root(Some(xdg), home).unwrap());
        let path = cache.path();
        assert!(path.starts_with(xdg.join("superai")));
        assert_eq!(
            path,
            xdg.join("superai")
                .join("model-pricing")
                .join("models-dev.json")
        );
        assert!(
            !path
                .components()
                .any(|c| c == std::path::Component::ParentDir),
            "the cache path must never climb out of its root: {}",
            path.display()
        );
    }

    #[test]
    fn a_cache_root_never_resolves_relative_to_the_working_directory() {
        for home in [Path::new(""), Path::new("relative-home")] {
            let err = PricingCache::default_root(None, home).unwrap_err();
            assert!(
                matches!(err, CostError::InvalidCacheRoot { .. }),
                "home `{:?}`: {err:?}",
                home.display()
            );
        }
        let err =
            PricingCache::default_root(Some(Path::new("relative/xdg")), Path::new("")).unwrap_err();
        assert!(matches!(err, CostError::InvalidCacheRoot { .. }), "{err:?}");
    }

    #[test]
    fn refresh_flows_never_write_into_a_harness_config_tree() {
        let harness_home = temp_dir("cache-harness-home");
        let codex_sessions = harness_home.join(".codex").join("sessions");
        let claude_projects = harness_home.join(".config").join("claude").join("projects");
        fs::create_dir_all(&codex_sessions).unwrap();
        fs::create_dir_all(&claude_projects).unwrap();
        fs::write(codex_sessions.join("rollout.jsonl"), b"{\"usage\":{}}\n").unwrap();
        fs::write(
            claude_projects.join("transcript.jsonl"),
            b"{\"message\":{}}\n",
        )
        .unwrap();
        let before = tree_snapshot(&harness_home);

        let cache_root = temp_dir("cache-refresh-root");
        let cache = PricingCache::new(&cache_root);
        let fixture = cache_root.join("api.json");
        fs::write(&fixture, PLAUSIBLE_CATALOG.as_bytes()).unwrap();
        cache
            .refresh(&file_url(&fixture), now())
            .unwrap_or_else(|e| panic!("refresh: {e}"));
        cache.store(&sample_table(), now()).unwrap();
        assert_eq!(
            tree_snapshot(&harness_home),
            before,
            "a pricing refresh must not write a single byte into harness dirs"
        );
        assert!(cache.load().is_some());
        drop(fs::remove_dir_all(&harness_home));
        drop(fs::remove_dir_all(&cache_root));
    }
}
