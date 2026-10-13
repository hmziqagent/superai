//! Abuse tests for the API-key pollers (USG-09): the `ClinePass` session
//! file, wrapped credentials, and provider response bodies carrying planted
//! secrets never reach snapshot output or poll errors. No live network:
//! requests hit a closed loopback port.

#[cfg(test)]
mod tests {
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
        ACCOUNT_ID_SENTINEL, API_KEY_SENTINEL, BEARER_SENTINEL, COOKIE_SENTINEL,
        OPENCODE_CREDENTIAL_SENTINEL, assert_no_sentinel_debug, assert_no_sentinel_display,
        assert_no_sentinel_json,
    };
    use superai_usage::poller::{
        PollLimits, PollSecret, clinepass_session, parse_clinepass_limits, parse_deepseek_balance,
        parse_openrouter_responses, poll_deepseek,
    };
    use thiserror as _;
    use ureq as _;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    const SESSION_CAP: usize = 64 * 1024;

    fn temp_dir(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "superai-abuse-pollers-{tag}-{}-{n}",
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
    fn clinepass_session_file_with_planted_secrets_stays_wrapped() {
        let dir = temp_dir("session");
        let oauth_file = dir.join("providers-oauth.json");
        fs::write(
            &oauth_file,
            json!({
                "providers": {
                    "opencode": {"settings": {"apiKey": OPENCODE_CREDENTIAL_SENTINEL}},
                    "cline": {"settings": {
                        "apiKey": API_KEY_SENTINEL,
                        "sessionCookie": COOKIE_SENTINEL,
                        "auth": {
                            "accessToken": BEARER_SENTINEL,
                            "refreshToken": ACCOUNT_ID_SENTINEL
                        }
                    }}
                }
            })
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("write: {e}"));

        let session = clinepass_session(&oauth_file, SESSION_CAP)
            .unwrap_or_else(|e| panic!("session: {e:?}"))
            .unwrap_or_else(|| panic!("the planted cline entry must yield a session"));
        assert!(
            session.is_oauth(),
            "accessToken must win Cline's precedence"
        );
        assert_no_sentinel_debug(&session, "clinepass session debug");
        assert_no_sentinel_display(session.secret(), "clinepass session secret display");

        let key_file = dir.join("providers-key.json");
        fs::write(
            &key_file,
            json!({
                "providers": {"cline": {"settings": {
                    "apiKey": API_KEY_SENTINEL,
                    "junk": COOKIE_SENTINEL
                }}}
            })
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("write: {e}"));
        let session = clinepass_session(&key_file, SESSION_CAP)
            .unwrap_or_else(|e| panic!("session: {e:?}"))
            .unwrap_or_else(|| panic!("the apiKey fallback must yield a session"));
        assert!(!session.is_oauth());
        assert_no_sentinel_debug(&session, "clinepass key-session debug");

        let malformed = dir.join("providers-bad.json");
        fs::write(&malformed, format!("not json {{ \"{API_KEY_SENTINEL}\""))
            .unwrap_or_else(|e| panic!("write: {e}"));
        let none = clinepass_session(&malformed, SESSION_CAP)
            .unwrap_or_else(|e| panic!("malformed file must not error: {e:?}"));
        assert!(none.is_none(), "a credential-less file yields no session");

        let oversized = dir.join("providers-huge.json");
        fs::write(&oversized, API_KEY_SENTINEL.repeat(200))
            .unwrap_or_else(|e| panic!("write: {e}"));
        let error = clinepass_session(&oversized, 64).unwrap_err();
        assert_no_sentinel_display(&error, "oversized session file error");
        assert_no_sentinel_debug(&error, "oversized session file error debug");
    }

    #[test]
    fn parse_bodies_with_planted_junk_fields_stay_out_of_snapshots() {
        let clinepass = parse_clinepass_limits(
        &json!({
            "success": true,
            "apiKey": API_KEY_SENTINEL,
            "data": {"limits": [
                {"type": "five_hour", "percentUsed": 42.5, "resetsAt": "2026-10-13T00:00:00Z", "sessionCookie": COOKIE_SENTINEL},
                {"type": "weird", "bearer": BEARER_SENTINEL}
            ]}
        })
        .to_string(),
    )
    .unwrap_or_else(|e| panic!("clinepass parse: {e:?}"));
        assert_no_sentinel_debug(&clinepass, "clinepass limits snapshot");
        assert_no_sentinel_json(&clinepass, "clinepass limits snapshot json");

        let deepseek = parse_deepseek_balance(
            &json!({
                "balance_infos": [{
                    "currency": "USD",
                    "total_balance": "110.50",
                    "granted_balance": "10.00",
                    "apiKey": API_KEY_SENTINEL
                }]
            })
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("deepseek parse: {e:?}"));
        assert_no_sentinel_debug(&deepseek, "deepseek balance snapshot");
        assert_no_sentinel_json(&deepseek, "deepseek balance snapshot json");

        let openrouter = parse_openrouter_responses(
        &json!({"data": {"usage": 12.5, "apiKey": API_KEY_SENTINEL}}).to_string(),
        Some(
            &json!({"data": {"total_credits": 100, "total_usage": 25.0, "sessionCookie": COOKIE_SENTINEL}}).to_string(),
        ),
    )
    .unwrap_or_else(|e| panic!("openrouter parse: {e:?}"));
        assert_no_sentinel_debug(&openrouter, "openrouter snapshot");
        assert_no_sentinel_json(&openrouter, "openrouter snapshot json");

        let error = parse_deepseek_balance(
            &json!({"balance_infos": [{"currency": "USD", "total_balance": API_KEY_SENTINEL}]})
                .to_string(),
        )
        .unwrap_err();
        assert_no_sentinel_display(&error, "deepseek unparseable amount error");
        assert_no_sentinel_debug(&error, "deepseek unparseable amount error debug");
    }

    #[test]
    fn poll_transport_failure_with_a_planted_key_never_leaks_it() {
        let secret = PollSecret::new(API_KEY_SENTINEL);
        assert_no_sentinel_debug(&secret, "wrapped poll secret debug");
        assert_no_sentinel_display(&secret, "wrapped poll secret display");
        assert_no_sentinel_json(&secret, "wrapped poll secret json");

        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("bind: {e}"));
            listener
                .local_addr()
                .unwrap_or_else(|e| panic!("addr: {e}"))
                .port()
        };
        let error = poll_deepseek(
            &format!("http://127.0.0.1:{port}/user/balance"),
            &secret,
            &limits(),
        )
        .unwrap_err();
        assert_no_sentinel_display(&error, "deepseek transport error");
        assert_no_sentinel_debug(&error, "deepseek transport error debug");
    }
}
