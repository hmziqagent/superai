//! Abuse tests for the llmproxy ledger client (USG-08): the admin token
//! planted in env, token files, and response bodies never reaches the
//! wrapped token, client state, error text, or parsed rows. No live
//! network: responses are fixtures and requests hit a closed loopback port.

use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use jiff as _;
use rusqlite as _;
use serde as _;
use serde_json::json;
use superai_core as _;
use superai_usage::abuse::{
    API_KEY_SENTINEL, BEARER_SENTINEL, COOKIE_SENTINEL, LLMPROXY_ADMIN_SENTINEL,
    assert_no_sentinel_debug, assert_no_sentinel_display, assert_no_sentinel_json,
};
use superai_usage::llmproxy::{
    AdminToken, LedgerClient, RequestsQuery, SummaryGroupBy, UsageKeys, UsageRequests,
    UsageSummary, load_admin_token,
};
use superai_usage::poller::PollLimits;
use thiserror as _;
use ureq as _;

#[cfg(test)]
mod tests {
    use super::*;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "superai-abuse-llmproxy-{tag}-{}-{n}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
        dir
    }

    fn limits() -> PollLimits {
        PollLimits::new(Duration::from_secs(5), 1_048_576)
            .unwrap_or_else(|e| panic!("valid limits refused: {e:?}"))
    }

    #[test]
    fn admin_token_from_env_and_0600_file_stays_fully_redacted() {
        let from_env = load_admin_token(Some(LLMPROXY_ADMIN_SENTINEL), None)
            .unwrap_or_else(|e| panic!("env token: {e:?}"));
        assert_no_sentinel_debug(&from_env, "env-loaded admin token");
        assert_no_sentinel_display(&from_env, "env-loaded admin token display");
        assert_no_sentinel_json(&from_env, "env-loaded admin token json");
        assert_eq!(from_env, AdminToken::new(LLMPROXY_ADMIN_SENTINEL));

        let dir = temp_dir("token");
        let file = dir.join("admin-token");
        fs::write(&file, format!("{LLMPROXY_ADMIN_SENTINEL}\n"))
            .unwrap_or_else(|e| panic!("write: {e}"));
        set_mode_0600(&file);
        let from_file =
            load_admin_token(None, Some(&file)).unwrap_or_else(|e| panic!("file token: {e:?}"));
        assert_no_sentinel_debug(&from_file, "file-loaded admin token");
        assert_eq!(from_file, from_env);
    }

    #[cfg(unix)]
    fn set_mode_0600(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|e| panic!("chmod {}: {e}", path.display()));
    }

    #[cfg(not(unix))]
    fn set_mode_0600(_path: &std::path::Path) {}

    #[cfg(unix)]
    #[test]
    fn refused_token_files_error_without_leaking_their_bytes() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = temp_dir("refused");
        let wrong_mode = dir.join("wrong-mode");
        fs::write(&wrong_mode, LLMPROXY_ADMIN_SENTINEL).unwrap_or_else(|e| panic!("write: {e}"));
        fs::set_permissions(&wrong_mode, fs::Permissions::from_mode(0o644))
            .unwrap_or_else(|e| panic!("chmod: {e}"));

        let error = load_admin_token(None, Some(&wrong_mode)).unwrap_err();
        assert_no_sentinel_display(&error, "wrong-mode token file error");
        assert_no_sentinel_debug(&error, "wrong-mode token file error debug");

        let linked = dir.join("token-link");
        std::os::unix::fs::symlink(&wrong_mode, &linked).unwrap_or_else(|e| panic!("symlink: {e}"));
        let error = load_admin_token(None, Some(&linked)).unwrap_err();
        assert_no_sentinel_display(&error, "symlinked token file error");
        assert_no_sentinel_debug(&error, "symlinked token file error debug");

        let huge = dir.join("huge");
        fs::write(&huge, format!("{}\n", API_KEY_SENTINEL.repeat(200)))
            .unwrap_or_else(|e| panic!("write: {e}"));
        set_mode_0600(&huge);
        let error = load_admin_token(None, Some(&huge)).unwrap_err();
        assert_no_sentinel_display(&error, "oversized token file error");
        assert_no_sentinel_debug(&error, "oversized token file error debug");
    }

    #[test]
    fn client_holding_a_planted_token_never_leaks_it_through_state_or_errors() {
        let token = load_admin_token(Some(LLMPROXY_ADMIN_SENTINEL), None)
            .unwrap_or_else(|e| panic!("{e:?}"));
        // Grab a loopback port, then close it: requests fail fast without any
        // live network.
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("bind: {e}"));
            listener
                .local_addr()
                .unwrap_or_else(|e| panic!("addr: {e}"))
                .port()
        };
        let base = format!("http://127.0.0.1:{port}");
        let client = LedgerClient::new(&base, &token, limits()).unwrap_or_else(|e| panic!("{e:?}"));
        assert_no_sentinel_debug(&client, "ledger client debug");

        let summary = client.summary("24h", SummaryGroupBy::Provider);
        let error = summary.unwrap_err();
        assert_no_sentinel_display(&error, "summary transport error");
        assert_no_sentinel_debug(&error, "summary transport error debug");

        let requests = client.requests(&RequestsQuery::ALL);
        let error = requests.unwrap_err();
        assert_no_sentinel_display(&error, "requests transport error");
        assert_no_sentinel_debug(&error, "requests transport error debug");
    }

    #[test]
    fn planted_response_bodies_parse_clean_and_unparseable_costs_stay_none() {
        let summary: UsageSummary = serde_json::from_str(
            &json!({
                "window": "24h",
                "adminToken": LLMPROXY_ADMIN_SENTINEL,
                "rows": [{
                    "group": "anthropic",
                    "requests": 3,
                    "cost_usd": "3.5",
                    "apiKey": API_KEY_SENTINEL,
                    "sessionCookie": COOKIE_SENTINEL
                }]
            })
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("summary parse: {e:?}"));
        assert_no_sentinel_debug(&summary, "parsed usage summary");
        assert_no_sentinel_json(&summary, "parsed usage summary json");
        assert_eq!(
            summary.total_cost().map(|cost| cost.to_string()),
            Some("3.5".to_owned())
        );

        let requests: UsageRequests = serde_json::from_str(
            &json!({
                "rows": [{
                    "request_id": "r-1",
                    "provider": "anthropic",
                    "cost_usd": LLMPROXY_ADMIN_SENTINEL,
                    "bearer": BEARER_SENTINEL,
                    "body": "never in the contract"
                }]
            })
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("requests parse: {e:?}"));
        assert_no_sentinel_debug(&requests, "parsed usage requests");
        let row = requests
            .rows
            .first()
            .unwrap_or_else(|| panic!("row missing"));
        assert!(
            row.cost_usd.is_none(),
            "a sentinel-shaped cost must parse to None, never a value"
        );

        let keys: UsageKeys = serde_json::from_str(
            &json!({"keys": [{"caller": "team", "apiKey": API_KEY_SENTINEL}]}).to_string(),
        )
        .unwrap_or_else(|e| panic!("keys parse: {e:?}"));
        assert_no_sentinel_debug(&keys, "parsed usage keys");
        assert_no_sentinel_json(&keys, "parsed usage keys json");
    }
}
