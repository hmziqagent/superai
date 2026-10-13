//! opencode v2 history reader tests (USG-03): event mapping, malformed-row
//! counts, credential isolation, fork dedup, read-only open.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use jiff::Timestamp;
    use rusqlite::{Connection, params};

    // Lib deps that ride along as externs of this target without being
    // referenced directly.
    use serde as _;
    use superai_core as _;
    use thiserror as _;

    use superai_usage::cost::UsageEvent;
    use superai_usage::reader::opencode::{OpenCodeError, scan_history};

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    const CANARY: &str = "CANARY-sk-oc-leak-0001";
    const BASE_MS: i64 = 1_769_989_200_000;
    const FULL_USAGE: &str = r#"{"model":{"providerID":"anthropic","id":"claude-sonnet-4-5"},"tokens":{"input":120,"output":80,"reasoning":40,"cache":{"read":500,"write":10}}}"#;
    const NO_CACHE_USAGE: &str =
        r#"{"model":{"providerID":"openai","id":"gpt-5.1"},"tokens":{"input":10,"output":5}}"#;
    const MODEL_ONLY: &str = r#"{"model":{"providerID":"custom-gw","id":"some-internal-model"}}"#;
    const ZERO_USAGE: &str = r#"{"model":{"providerID":"custom-gw","id":"some-internal-model"},"tokens":{"input":0,"output":0,"reasoning":0}}"#;

    fn temp_dir(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-usage-opencode-{tag}-{}-{n}-{nanos}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
        dir
    }

    fn ms(epoch_ms: i64) -> Timestamp {
        Timestamp::from_millisecond(epoch_ms).unwrap_or_else(|e| panic!("epoch ms: {e}"))
    }

    fn full_usage_event(at_ms: i64) -> UsageEvent {
        UsageEvent {
            timestamp: ms(at_ms),
            provider: "anthropic".to_owned(),
            model: "claude-sonnet-4-5".to_owned(),
            input_tokens: 120,
            output_tokens: 120,
            cache_read_tokens: Some(500),
            cache_write_tokens: Some(10),
        }
    }

    struct Fixture {
        path: PathBuf,
        conn: Connection,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let path = temp_dir(tag).join("opencode.db");
            let conn = Connection::open(&path).unwrap_or_else(|e| panic!("fixture open: {e}"));
            conn.execute_batch(&format!(
                "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, fork_session_id TEXT, time_created INTEGER NOT NULL);
                 CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);
                 CREATE TABLE account (id TEXT PRIMARY KEY, access_token TEXT, refresh_token TEXT NOT NULL, time_updated INTEGER NOT NULL);
                 CREATE TABLE control_account (email TEXT, url TEXT, access_token TEXT, refresh_token TEXT NOT NULL, time_updated INTEGER NOT NULL);
                 CREATE TABLE account_state (id INTEGER PRIMARY KEY, active_account_id TEXT);
                 CREATE TABLE credential (id TEXT PRIMARY KEY, value TEXT, time_updated INTEGER NOT NULL);
                 CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL, time_updated INTEGER NOT NULL);
                 INSERT INTO account VALUES ('acc-1', '{CANARY}', '{CANARY}', 0);
                 INSERT INTO control_account VALUES ('user@example.com', 'https://opencode.ai', '{CANARY}', '{CANARY}', 0);
                 INSERT INTO account_state VALUES (1, '{CANARY}');
                 INSERT INTO credential VALUES ('cred-1', '{CANARY}', 0);
                 INSERT INTO kv VALUES ('state', '{CANARY}', 0);"
            ))
            .unwrap_or_else(|e| panic!("fixture schema: {e}"));
            Self { path, conn }
        }

        fn session(&self, id: &str, fork: Option<&str>, created_ms: i64) {
            self.conn
                .execute(
                    "INSERT INTO session_v2 (id, fork_session_id, time_created) VALUES (?1, ?2, ?3)",
                    params![id, fork, created_ms],
                )
                .unwrap_or_else(|e| panic!("session row: {e}"));
        }

        fn message(&self, id: &str, session_id: &str, kind: &str, created_ms: i64, data: &str) {
            self.conn
                .execute(
                    "INSERT INTO session_message (id, session_id, type, seq, time_created, data)
                     VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(seq), 0) + 1 FROM session_message WHERE session_id = ?2), ?4, ?5)",
                    params![id, session_id, kind, created_ms, data],
                )
                .unwrap_or_else(|e| panic!("message row: {e}"));
        }

        fn done(self) -> PathBuf {
            let path = self.path;
            drop(self.conn);
            path
        }
    }

    #[test]
    fn assistant_rows_map_to_events_with_reasoning_folded_into_output() {
        let fixture = Fixture::new("happy");
        fixture.session("s1", None, BASE_MS);
        fixture.message("m1", "s1", "assistant", BASE_MS + 1_000, FULL_USAGE);
        fixture.message("m2", "s1", "user", BASE_MS + 2_000, FULL_USAGE);
        fixture.message("m3", "s1", "assistant", BASE_MS + 3_000, NO_CACHE_USAGE);
        let path = fixture.done();

        let usage = scan_history(&path).unwrap_or_else(|e| panic!("scan: {e}"));

        assert_eq!(usage.unscanned_records, 0);
        assert_eq!(usage.events.len(), 2);
        assert!(usage.events.contains(&full_usage_event(BASE_MS + 1_000)));
        assert!(usage.events.contains(&UsageEvent {
            timestamp: ms(BASE_MS + 3_000),
            provider: "openai".to_owned(),
            model: "gpt-5.1".to_owned(),
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: None,
            cache_write_tokens: None,
        }));
    }

    #[test]
    fn model_only_and_zero_token_rows_are_dropped_without_a_gap() {
        let fixture = Fixture::new("zero");
        fixture.session("s1", None, BASE_MS);
        fixture.message("m1", "s1", "assistant", BASE_MS, MODEL_ONLY);
        fixture.message("m2", "s1", "assistant", BASE_MS + 1_000, ZERO_USAGE);
        fixture.message("m3", "s1", "assistant", BASE_MS + 2_000, FULL_USAGE);
        let path = fixture.done();

        let usage = scan_history(&path).unwrap_or_else(|e| panic!("scan: {e}"));

        assert_eq!(usage.events, vec![full_usage_event(BASE_MS + 2_000)]);
        assert_eq!(
            usage.unscanned_records, 0,
            "a zero row is understood, not an unreadable record"
        );
    }

    #[test]
    fn malformed_rows_are_counted_and_never_fatal() {
        let fixture = Fixture::new("malformed");
        fixture.session("s1", None, BASE_MS);
        fixture.message("b1", "s1", "assistant", BASE_MS, "not json {");
        fixture.message(
            "b2",
            "s1",
            "assistant",
            BASE_MS,
            r#"{"tokens":{"input":9,"output":9}}"#,
        );
        fixture.message(
            "b3",
            "s1",
            "assistant",
            BASE_MS,
            r#"{"model":{"providerID":"x","id":"y"},"tokens":{"input":"120"}}"#,
        );
        fixture.message(
            "b4",
            "s1",
            "assistant",
            BASE_MS,
            r#"{"model":{"providerID":"x","id":"y"},"tokens":{"input":-5}}"#,
        );
        fixture.message("b5", "s1", "assistant", BASE_MS, "\"just a scalar\"");
        fixture.message("b6", "s1", "assistant", i64::MAX, FULL_USAGE);
        fixture.message("ok1", "s1", "assistant", BASE_MS + 1_000, FULL_USAGE);
        let path = fixture.done();

        let usage = scan_history(&path).unwrap_or_else(|e| panic!("scan: {e}"));

        assert_eq!(usage.events, vec![full_usage_event(BASE_MS + 1_000)]);
        assert_eq!(usage.unscanned_records, 6);
    }

    #[test]
    fn credential_table_bytes_never_reach_parsed_output() {
        let fixture = Fixture::new("canary");
        fixture.session("s1", None, BASE_MS);
        fixture.message("m1", "s1", "assistant", BASE_MS, FULL_USAGE);
        let path = fixture.done();

        let usage = scan_history(&path).unwrap_or_else(|e| panic!("scan: {e}"));
        let json = serde_json::to_string(&usage).unwrap_or_else(|e| panic!("serialize: {e}"));

        assert!(!json.contains(CANARY));
        assert_eq!(usage.events, vec![full_usage_event(BASE_MS)]);
    }

    #[test]
    fn forked_sessions_count_ancestor_messages_once() {
        let fixture = Fixture::new("fork");
        fixture.session("s1", None, BASE_MS);
        fixture.session("s2", Some("s1"), BASE_MS + 100_000);
        fixture.message("m0", "s1", "assistant", BASE_MS + 500, FULL_USAGE);
        fixture.message("m1", "s2", "assistant", BASE_MS + 500, FULL_USAGE);
        fixture.message("m2", "s2", "assistant", BASE_MS + 150_000, NO_CACHE_USAGE);
        let path = fixture.done();

        let usage = scan_history(&path).unwrap_or_else(|e| panic!("scan: {e}"));

        let ancestor = full_usage_event(BASE_MS + 500);
        assert_eq!(
            usage.events.iter().filter(|e| **e == ancestor).count(),
            1,
            "forked copy of an ancestor message must not double-count"
        );
        assert_eq!(usage.events.len(), 2);
    }

    #[test]
    fn scan_never_writes_sidecar_files() {
        let fixture = Fixture::new("sidecar");
        fixture.session("s1", None, BASE_MS);
        fixture.message("m1", "s1", "assistant", BASE_MS, FULL_USAGE);
        let path = fixture.done();

        let usage = scan_history(&path).unwrap_or_else(|e| panic!("scan: {e}"));

        assert_eq!(usage.events.len(), 1);
        let dir = path
            .parent()
            .unwrap_or_else(|| panic!("fixture path has no parent"))
            .to_path_buf();
        let entries = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
            .count();
        assert_eq!(entries, 1, "scan must leave no -wal/-shm/-journal sidecars");
    }

    #[test]
    fn missing_database_is_an_error_and_is_never_created() {
        let path = temp_dir("missing").join("opencode.db");

        let result = scan_history(&path);

        assert!(matches!(result, Err(OpenCodeError::Open(_))));
        assert!(!path.exists());
    }
}
