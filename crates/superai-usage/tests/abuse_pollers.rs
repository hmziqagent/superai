//! Abuse tests for the API-key pollers (USG-09): every poller's session
//! file, wrapped credential, response body, and redirect refusal carries
//! planted secrets that never reach snapshot output or poll errors. No live
//! network: requests hit loopback ports only.

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
        PollError, PollLimits, PollSecret, clinepass_session, parse_clinepass_limits,
        parse_deepseek_balance, parse_factory_limits, parse_kilo_credits, parse_minimax_remains,
        parse_mistral_credits, parse_openrouter_responses, parse_warp_graphql, parse_zed_billing,
        poll_deepseek, poll_factory, poll_mistral, poll_zed,
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

    #[test]
    fn six_more_parsers_with_planted_junk_stay_out_of_snapshots() {
        let zed = parse_zed_billing(
            &json!({
                "plan": "Free",
                "sessionCookie": COOKIE_SENTINEL,
                "current_usage": {
                    "edit_predictions": {"used": 3, "limit": 10, "junk": BEARER_SENTINEL},
                },
                "token_spend": {
                    "spend_in_cents": 120,
                    "limit_in_cents": 1000,
                    "apiKey": API_KEY_SENTINEL
                }
            })
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("zed parse: {e:?}"));
        assert_no_sentinel_debug(&zed, "zed billing snapshot");
        assert_no_sentinel_json(&zed, "zed billing snapshot json");

        let warp = parse_warp_graphql(
            &json!({"data": {"user": {"user": {
                "sessionCookie": COOKIE_SENTINEL,
                "requestLimitInfo": {
                    "isUnlimited": false,
                    "requestLimit": 100,
                    "requestsUsedSinceLastRefresh": 25,
                    "nextRefreshTime": "2026-10-14T00:00:00Z",
                    "junk": API_KEY_SENTINEL
                },
                "bonusGrants": [{
                    "requestCreditsGranted": 10,
                    "requestCreditsRemaining": 4,
                    "note": BEARER_SENTINEL
                }]
            }}}})
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("warp parse: {e:?}"));
        assert_no_sentinel_debug(&warp, "warp graphql snapshot");
        assert_no_sentinel_json(&warp, "warp graphql snapshot json");

        let factory = parse_factory_limits(
            &json!({
                "extraUsageBalanceCents": 5000,
                "apiKey": API_KEY_SENTINEL,
                "limits": {
                    "standard": {
                        "fiveHour": {"usedPercent": 40.0, "windowEnd": "2026-10-13T12:00:00Z", "secondsRemaining": 600},
                        "weekly": {"usedPercent": 10.0},
                        "monthly": {"usedPercent": 5.0}
                    },
                    "junk": COOKIE_SENTINEL
                }
            })
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("factory parse: {e:?}"));
        assert_no_sentinel_debug(&factory, "factory limits snapshot");
        assert_no_sentinel_json(&factory, "factory limits snapshot json");
    }

    #[test]
    fn kilo_minimax_and_mistral_parsers_with_planted_junk_stay_out_of_snapshots() {
        let kilo = parse_kilo_credits(
            &json!([{"result": {"data": {"json": {"creditBlocks": [
                {"amount_mUsd": 2_500_000, "balance_mUsd": 1_000_000, "apiKey": API_KEY_SENTINEL},
                {"amount_mUsd": 500_000, "balance_mUsd": 250_000, "junk": COOKIE_SENTINEL}
            ]}}}}])
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("kilo parse: {e:?}"));
        assert_no_sentinel_debug(&kilo, "kilo credits snapshot");
        assert_no_sentinel_json(&kilo, "kilo credits snapshot json");

        let minimax = parse_minimax_remains(
            &json!({"data": {
                "baseResp": {"statusCode": 0, "statusMsg": API_KEY_SENTINEL},
                "model_remains": [
                    {
                        "model_name": "MiniMax-M2",
                        "current_interval_remaining_percent": 60.0,
                        "current_interval_total_count": 100,
                        "current_interval_usage_count": 40,
                        "junk": COOKIE_SENTINEL
                    },
                    {
                        "model_name": "MiniMax-M2.1",
                        "current_interval_remaining_percent": 80.0,
                        "sessionCookie": BEARER_SENTINEL
                    }
                ]
            }})
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("minimax parse: {e:?}"));
        assert_no_sentinel_debug(&minimax, "minimax remains snapshot");
        assert_no_sentinel_json(&minimax, "minimax remains snapshot json");

        let mistral = parse_mistral_credits(
            &json!({
                "wallet_amount": 10.0,
                "credit_notes_amount": 2.0,
                "ongoing_usage_balance": 1.0,
                "currency": "eur",
                "apiKey": API_KEY_SENTINEL,
                "sessionCookie": COOKIE_SENTINEL
            })
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("mistral parse: {e:?}"));
        assert_no_sentinel_debug(&mistral, "mistral credits snapshot");
        assert_no_sentinel_json(&mistral, "mistral credits snapshot json");
    }

    #[test]
    fn cookie_and_bearer_poll_transports_with_planted_secrets_never_leak_them() {
        let cookie = PollSecret::new(COOKIE_SENTINEL);
        assert_no_sentinel_debug(&cookie, "wrapped cookie secret debug");
        assert_no_sentinel_display(&cookie, "wrapped cookie secret display");

        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("bind: {e}"));
            listener
                .local_addr()
                .unwrap_or_else(|e| panic!("addr: {e}"))
                .port()
        };
        let closed = format!("http://127.0.0.1:{port}");
        for (label, error) in [
            (
                "zed cookie transport",
                poll_zed(&format!("{closed}/usage"), &cookie, &limits()).unwrap_err(),
            ),
            (
                "mistral cookie transport",
                poll_mistral(&format!("{closed}/api/billing/credits"), &cookie, &limits())
                    .unwrap_err(),
            ),
            (
                "factory bearer transport",
                poll_factory(
                    &format!("{closed}/api/billing/limits"),
                    &PollSecret::new(API_KEY_SENTINEL),
                    &limits(),
                )
                .unwrap_err(),
            ),
        ] {
            assert_no_sentinel_display(&error, label);
            assert_no_sentinel_debug(&error, label);
        }
    }

    #[test]
    fn redirect_refusal_redacts_a_planted_query_secret() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("bind: {e}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("addr: {e}"));
        std::thread::spawn(move || {
            // Serve every connection until the test binary exits: a client
            // reconnect must meet the same 302, not a dead listener.
            while let Ok((mut stream, _)) = listener.accept() {
                if std::io::Write::write_all(
                    &mut stream,
                    b"HTTP/1.1 302 Found\r\nLocation: /elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .is_err()
                {
                    continue;
                }
            }
        });
        let url = format!("http://{addr}/usage?api_key={COOKIE_SENTINEL}");
        let error = poll_zed(&url, &PollSecret::new(COOKIE_SENTINEL), &limits()).unwrap_err();
        assert!(
            matches!(error, PollError::Redirect { .. }),
            "the 302 must surface as a refused redirect: {error:?}"
        );
        assert_no_sentinel_display(&error, "redirect refusal error");
        assert_no_sentinel_debug(&error, "redirect refusal error debug");
    }
}
