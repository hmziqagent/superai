//! Abuse tests for the opencode v2 history reader: credential tables,
//! secret-bearing JSON siblings, and malformed rows planted next to real
//! usage never reach parsed output, error text, or cost rows.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use jiff::Timestamp;
    use rusqlite::{Connection, params};

    use serde as _;
    use serde_json::json;
    use superai_core as _;
    use superai_usage::abuse::{
        ACCOUNT_ID_SENTINEL, API_KEY_SENTINEL, BEARER_SENTINEL, COOKIE_SENTINEL,
        LLMPROXY_ADMIN_SENTINEL, OPENCODE_CREDENTIAL_SENTINEL, assert_no_sentinel_debug,
        assert_no_sentinel_display, assert_no_sentinel_json,
    };
    use superai_usage::cost::{CostEngine, CostPeriod, RateTable};
    use superai_usage::reader::opencode::scan_history;
    use thiserror as _;
    use ureq as _;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    const BASE_MS: i64 = 1_769_989_200_000;

    fn temp_dir(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-abuse-opencode-{tag}-{}-{n}-{nanos}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
        dir
    }

    fn assistant_data(model: &str, input: i64, output: i64) -> String {
        json!({
            "model": {"providerID": "anthropic", "id": model},
            "tokens": {"input": input, "output": output}
        })
        .to_string()
    }

    fn planted_fixture(tag: &str) -> PathBuf {
        let path = temp_dir(tag).join("opencode.db");
        let conn = Connection::open(&path).unwrap_or_else(|e| panic!("fixture open: {e}"));
        conn.execute_batch(&format!(
        "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, fork_session_id TEXT, time_created INTEGER NOT NULL);
         CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);
         CREATE TABLE account (id TEXT PRIMARY KEY, access_token TEXT, refresh_token TEXT NOT NULL, time_updated INTEGER NOT NULL);
         CREATE TABLE control_account (email TEXT, url TEXT, access_token TEXT, refresh_token TEXT NOT NULL, time_updated INTEGER NOT NULL);
         CREATE TABLE credential (id TEXT PRIMARY KEY, value TEXT, time_updated INTEGER NOT NULL);
         CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL, time_updated INTEGER NOT NULL);
         INSERT INTO account VALUES ('acc-1', '{API_KEY_SENTINEL}', '{ACCOUNT_ID_SENTINEL}', 0);
         INSERT INTO control_account VALUES ('user@example.com', 'https://opencode.ai', '{LLMPROXY_ADMIN_SENTINEL}', '{OPENCODE_CREDENTIAL_SENTINEL}', 0);
         INSERT INTO credential VALUES ('cred-1', '{OPENCODE_CREDENTIAL_SENTINEL}', 0);
         INSERT INTO kv VALUES ('state', '{BEARER_SENTINEL}', 0);"
    ))
    .unwrap_or_else(|e| panic!("fixture schema: {e}"));
        conn.execute(
            "INSERT INTO session_v2 (id, fork_session_id, time_created) VALUES ('s1', NULL, ?1)",
            params![BASE_MS],
        )
        .unwrap_or_else(|e| panic!("session row: {e}"));
        let message = |id: &str, data: &str, at_ms: i64| {
            conn.execute(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, data)
             VALUES (?1, 's1', 'assistant', (SELECT COALESCE(MAX(seq), 0) + 1 FROM session_message), ?2, ?3)",
            params![id, at_ms, data],
        )
        .unwrap_or_else(|e| panic!("message row: {e}"));
        };
        message(
            "m1",
            &assistant_data("claude-sonnet-4-5", 100, 40),
            BASE_MS + 1_000,
        );
        message(
            "m2",
            &json!({
                "model": {"providerID": "anthropic", "id": "claude-haiku-4-5"},
                "tokens": {"input": 10, "output": 5},
                "apiKey": API_KEY_SENTINEL,
                "oauthToken": BEARER_SENTINEL,
                "sessionCookie": COOKIE_SENTINEL
            })
            .to_string(),
            BASE_MS + 2_000,
        );
        message(
            "m3",
            &format!("not json {{ \"cookie\": \"{COOKIE_SENTINEL}\""),
            BASE_MS + 3_000,
        );
        drop(conn);
        path
    }

    #[test]
    fn credential_rows_and_json_secrets_never_reach_output_or_cost_rows() {
        let path = planted_fixture("history");

        let usage = scan_history(&path).unwrap_or_else(|e| panic!("scan: {e}"));

        assert_eq!(
            usage.events.len(),
            2,
            "planted rows must not suppress usage"
        );
        assert!(
            usage.unscanned_records >= 1,
            "the malformed row is a counted gap"
        );
        assert_no_sentinel_debug(&usage, "opencode recorded usage");
        assert_no_sentinel_json(&usage, "opencode recorded usage json");
        let rates = RateTable::bundled().unwrap_or_else(|e| panic!("bundled rates: {e:?}"));
        let now = Timestamp::from_second(1_800_000_000).unwrap_or_else(|e| panic!("now: {e:?}"));
        let report = CostEngine::new(rates)
            .total(&CostPeriod::AllHistory, now, "UTC", &usage)
            .unwrap_or_else(|e| panic!("total: {e:?}"));
        assert_no_sentinel_debug(&report, "opencode cost report");
        assert_no_sentinel_json(&report, "opencode cost report json");
    }

    #[test]
    fn corrupt_database_with_sentinel_content_errors_without_leaking() {
        let dir = temp_dir("corrupt");
        let path = dir.join("history.db");
        fs::write(
            &path,
            format!("sqlite-nope api_key={API_KEY_SENTINEL} cookie={COOKIE_SENTINEL}"),
        )
        .unwrap_or_else(|e| panic!("write: {e}"));

        let error = scan_history(&path).unwrap_err();

        assert_no_sentinel_display(&error, "opencode corrupt-db error");
        assert_no_sentinel_debug(&error, "opencode corrupt-db error debug");
    }
}
