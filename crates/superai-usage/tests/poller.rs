//! Poller tests (USG-09): parse-level fixtures per provider, loopback
//! transport behavior under the `PRV-07` bounds, redaction, the `MiniMax` host
//! policy, the `ClinePass` session-file mode, and the structural
//! no-background-polling rule. Zero live upstream calls: every HTTP exchange
//! goes to a loopback listener serving canned bytes.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use superai_usage::poller::{
    PollError, PollLimits, PollSecret, ProviderUsageSnapshot, clinepass_providers_path,
    clinepass_session, parse_clinepass_limits, parse_deepseek_balance, parse_factory_limits,
    parse_kilo_credits, parse_minimax_remains, parse_mistral_credits, parse_openrouter_responses,
    parse_warp_graphql, parse_zed_billing, poll_clinepass, poll_warp, poll_zed,
    validate_minimax_endpoint_override,
};
use superai_usage::window::LaneReading;

use jiff as _;
use rusqlite as _;
use serde as _;
use superai_core as _;
use thiserror as _;
use ureq as _;

#[cfg(test)]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "explicit Ok/Err checks in tests"
    )]
    use super::*;

    const SENTINEL: &str = "sk-superai-test-sentinel-12345-fake";

    fn limits() -> PollLimits {
        PollLimits::new(Duration::from_secs(5), 1_048_576)
            .unwrap_or_else(|e| panic!("valid limits refused: {e:?}"))
    }

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/usage-providers")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
    }

    fn used_percent_of(reading: &LaneReading) -> f64 {
        reading
            .used_percent()
            .unwrap_or_else(|| panic!("lane {reading:?} reported no usage"))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn http_response(status_line: &str, headers: &[(&str, &str)], body: &str) -> String {
        let mut response = format!("HTTP/1.1 {status_line}\r\n");
        for (name, value) in headers {
            write!(response, "{name}: {value}\r\n")
                .unwrap_or_else(|e| panic!("render header: {e}"));
        }
        write!(response, "Content-Length: {}\r\n\r\n{body}", body.len())
            .unwrap_or_else(|e| panic!("render response: {e}"));
        response
    }

    fn ok_json(body: &str) -> String {
        http_response("200 OK", &[("Content-Type", "application/json")], body)
    }

    struct Served {
        endpoint: String,
        request: mpsc::Receiver<String>,
    }

    fn content_length_of(headers: &str) -> usize {
        headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0)
    }

    fn request_complete(received: &[u8]) -> bool {
        let text = String::from_utf8_lossy(received).into_owned();
        text.find("\r\n\r\n").is_some_and(|header_end| {
            received.len()
                >= header_end + 4 + content_length_of(text.get(..header_end).unwrap_or_default())
        })
    }

    /// Bind a loopback listener that serves exactly one exchange and reports
    /// the raw request bytes back to the test.
    fn serve_once(response: String) -> Served {
        let listener =
            TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("loopback bind: {e}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("loopback addr: {e}"));
        let (tx, rx) = mpsc::channel();
        let endpoint = format!("http://{addr}/api/v1/users/me/plan/usage-limits");
        std::thread::spawn(move || {
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) => panic!("loopback accept: {e}"),
            };
            let mut received = Vec::new();
            let mut buf = [0u8; 8192];
            // Read headers plus any Content-Length body, then answer.
            while !request_complete(&received) {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => received.extend_from_slice(buf.get(..n).unwrap_or_default()),
                }
            }
            tx.send(String::from_utf8_lossy(&received).into_owned())
                .unwrap_or_else(|e| panic!("report request: {e}"));
            stream
                .write_all(response.as_bytes())
                .unwrap_or_else(|e| panic!("loopback write: {e}"));
            stream
                .flush()
                .unwrap_or_else(|e| panic!("loopback flush: {e}"));
        });
        Served {
            endpoint,
            request: rx,
        }
    }

    /// Accept one connection and never answer; the poll must hit its timeout.
    fn serve_silence() -> String {
        let listener =
            TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("loopback bind: {e}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("loopback addr: {e}"));
        let endpoint = format!("http://{addr}/api/v1/users/me/plan/usage-limits");
        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            // Park while holding the accepted socket so it never answers; the
            // poller's global timeout is the only way out.
            let _held = stream;
            std::thread::park();
        });
        endpoint
    }

    #[test]
    fn clinepass_limits_map_their_three_windows_raw() {
        let snapshot = parse_clinepass_limits(&fixture("clinepass-limits.json"))
            .unwrap_or_else(|e| panic!("fixture rejected: {e}"));
        assert_eq!(snapshot.provider, "cline-pass");
        assert!(close(used_percent_of(&snapshot.primary), 42.5));
        assert!(close(used_percent_of(&snapshot.secondary), 17.25));
        assert!(close(used_percent_of(&snapshot.tertiary), 9.5));
        let LaneReading::Measured(monthly) = &snapshot.tertiary else {
            panic!("monthly lane missing");
        };
        assert_eq!(monthly.resets_at, None);
        assert_eq!(monthly.window_minutes, Some(30 * 24 * 60));
        let LaneReading::Measured(five_hour) = &snapshot.primary else {
            panic!("five-hour lane missing");
        };
        assert_eq!(five_hour.resets_at.as_deref(), Some("2026-10-13T18:00:00Z"));
    }

    #[test]
    fn clinepass_limits_keep_over_quota_raw_and_absent_windows_unavailable() {
        let body = r#"{"success": true, "data": {"limits": [
            {"type": "five_hour", "percentUsed": 137.5},
            {"type": "unknown_lanes_are_ignored", "percentUsed": 1.0}
        ]}}"#;
        let snapshot =
            parse_clinepass_limits(body).unwrap_or_else(|e| panic!("fixture rejected: {e}"));
        assert!(close(used_percent_of(&snapshot.primary), 137.5));
        assert_eq!(snapshot.secondary, LaneReading::Unavailable);
        assert_eq!(snapshot.tertiary, LaneReading::Unavailable);
        assert!(parse_clinepass_limits(r#"{"success": false}"#).is_err());
        assert!(parse_clinepass_limits(r#"{"data": {}}"#).is_err());
    }

    #[test]
    fn deepseek_balance_prefers_usd_and_splits_granted_from_paid() {
        let snapshot = parse_deepseek_balance(&fixture("deepseek-balance.json"))
            .unwrap_or_else(|e| panic!("fixture rejected: {e}"));
        let balance = snapshot.balance.expect("balance lane missing");
        assert_eq!(balance.currency, "USD");
        assert!(close(balance.available, 12.5));
        assert!(balance.granted.is_some_and(|granted| close(granted, 2.5)));
        assert!(balance.paid.is_some_and(|paid| close(paid, 10.0)));
        assert_eq!(snapshot.primary, LaneReading::Unavailable);
    }

    #[test]
    fn openrouter_key_and_credits_compose_window_and_balance() {
        let key = fixture("openrouter-key.json");
        let credits = fixture("openrouter-credits.json");
        let snapshot = parse_openrouter_responses(&key, Some(&credits))
            .unwrap_or_else(|e| panic!("fixture rejected: {e}"));
        assert!(close(used_percent_of(&snapshot.primary), 0.125));
        let balance = snapshot.balance.expect("balance lane missing");
        assert!(close(balance.available, 1.9));

        // Without the cap the same key reports spend, not a quota window.
        let uncapped = key.replace("\"limit\": 10.0", "\"limit\": null");
        let snapshot = parse_openrouter_responses(&uncapped, None)
            .unwrap_or_else(|e| panic!("uncapped key rejected: {e}"));
        assert_eq!(snapshot.primary, LaneReading::Unavailable);
        let cost = snapshot.cost.expect("cost lane missing");
        assert!(close(cost.used, 0.012));
        assert_eq!(cost.period, "api key this month");
    }

    #[test]
    fn zed_billing_maps_predictions_and_token_spend() {
        let snapshot = parse_zed_billing(&fixture("zed-billing.json"))
            .unwrap_or_else(|e| panic!("fixture rejected: {e}"));
        assert!(close(used_percent_of(&snapshot.primary), 24.0));
        let cost = snapshot.cost.expect("cost lane missing");
        assert!(close(cost.used, 12.34));
        assert!(cost.limit.is_some_and(|limit| close(limit, 50.0)));
        assert_eq!(cost.currency, "USD");

        let unlimited =
            r#"{"plan": "pro", "current_usage": {"edit_predictions": {"used": 3, "limit": null}}}"#;
        let snapshot =
            parse_zed_billing(unlimited).unwrap_or_else(|e| panic!("unlimited rejected: {e}"));
        assert!(close(used_percent_of(&snapshot.primary), 0.0));
        assert_eq!(
            snapshot
                .primary
                .window()
                .and_then(|w| w.reset_description.as_deref()),
            Some("Unlimited")
        );
    }

    #[test]
    fn warp_graphql_maps_credits_and_addon_grants() {
        let snapshot = parse_warp_graphql(&fixture("warp-graphql.json"))
            .unwrap_or_else(|e| panic!("fixture rejected: {e}"));
        assert!(close(used_percent_of(&snapshot.primary), 25.0));
        assert_eq!(
            snapshot
                .primary
                .window()
                .and_then(|w| w.resets_at.as_deref()),
            Some("2026-11-01T00:00:00Z")
        );
        assert!(close(used_percent_of(&snapshot.secondary), 40.0));
    }

    #[test]
    fn factory_limits_map_standard_core_and_extra_balance() {
        let snapshot = parse_factory_limits(&fixture("factory-limits.json"))
            .unwrap_or_else(|e| panic!("fixture rejected: {e}"));
        assert!(close(used_percent_of(&snapshot.primary), 15.5));
        assert!(close(used_percent_of(&snapshot.secondary), 42.0));
        assert!(close(used_percent_of(&snapshot.tertiary), 7.25));
        // The whole core pool renders once any of its windows carries data;
        // provider-reported zeros stay real zeros.
        assert_eq!(snapshot.extras.len(), 3);
        assert_eq!(snapshot.extras[0].id, "factory-core-fiveHour");
        assert!(close(used_percent_of(&snapshot.extras[0].reading), 3.0));
        assert!(close(used_percent_of(&snapshot.extras[1].reading), 0.0));
        let balance = snapshot.balance.expect("balance lane missing");
        assert!(close(balance.available, 15.0));
        let LaneReading::Measured(weekly) = &snapshot.secondary else {
            panic!("weekly lane missing");
        };
        assert_eq!(
            weekly.resets_at.as_deref(),
            Some("2025-10-19T02:40:00Z"),
            "epoch seconds windowEnd must render as ISO-8601"
        );
        let LaneReading::Measured(monthly) = &snapshot.tertiary else {
            panic!("monthly lane missing");
        };
        assert_eq!(
            monthly.reset_description.as_deref(),
            Some("resets in 3600s")
        );
    }

    #[test]
    fn kilo_credits_sum_micro_usd_blocks() {
        let snapshot = parse_kilo_credits(&fixture("kilo-credits.json"))
            .unwrap_or_else(|e| panic!("fixture rejected: {e}"));
        let LaneReading::Measured(window) = &snapshot.primary else {
            panic!("credits lane missing");
        };
        assert!(close(window.used_percent, 100.0 * 1.0 / 7.5));
        assert_eq!(
            window.reset_description.as_deref(),
            Some("1.00 / 7.50 credits")
        );
    }

    #[test]
    fn minimax_remains_map_first_model_and_named_extras() {
        let snapshot = parse_minimax_remains(&fixture("minimax-remains.json"))
            .unwrap_or_else(|e| panic!("fixture rejected: {e}"));
        assert!(close(used_percent_of(&snapshot.primary), 75.0));
        let LaneReading::Measured(window) = &snapshot.primary else {
            panic!("primary lane missing");
        };
        assert_eq!(window.window_minutes, Some(606));
        assert!(window.resets_at.is_some());
        assert_eq!(snapshot.extras.len(), 1);
        assert_eq!(snapshot.extras[0].id, "video");
        assert!(close(used_percent_of(&snapshot.extras[0].reading), 50.0));
        assert!(parse_minimax_remains(r#"{"data": {"model_remains": []}}"#).is_err());
    }

    #[test]
    fn mistral_credits_report_available_and_keep_lanes_provider_only() {
        let snapshot = parse_mistral_credits(&fixture("mistral-credits.json"))
            .unwrap_or_else(|e| panic!("fixture rejected: {e}"));
        let balance = snapshot.balance.expect("balance lane missing");
        assert!(close(balance.available, 13.75));
        assert_eq!(balance.currency, "EUR");
        assert_eq!(snapshot.primary, LaneReading::ProviderOnly);
        assert_eq!(snapshot.secondary, LaneReading::Unavailable);
    }

    #[test]
    fn every_parser_refuses_garbage() {
        let garbage = ["", "[]", "null", "{", "\"text\"", "{\"oops\": 1}"];
        for body in garbage {
            assert!(
                parse_clinepass_limits(body).is_err(),
                "clinepass took {body:?}"
            );
            assert!(
                parse_deepseek_balance(body).is_err(),
                "deepseek took {body:?}"
            );
            assert!(
                parse_openrouter_responses(body, None).is_err(),
                "openrouter took {body:?}"
            );
            assert!(parse_zed_billing(body).is_err(), "zed took {body:?}");
            assert!(parse_warp_graphql(body).is_err(), "warp took {body:?}");
            assert!(parse_kilo_credits(body).is_err(), "kilo took {body:?}");
            assert!(
                parse_minimax_remains(body).is_err(),
                "minimax took {body:?}"
            );
            assert!(
                parse_mistral_credits(body).is_err(),
                "mistral took {body:?}"
            );
        }
        // Factory degrades to an all-unavailable snapshot when `limits` is
        // absent, so its garbage bar is a malformed standard pool instead.
        for body in [
            "",
            "[]",
            "null",
            "{",
            "\"text\"",
            r#"{"limits": {"standard": {"fiveHour": {}}}}"#,
            r#"{"limits": {"standard": {"weekly": {"usedPercent": "NaN-ish"}}}}"#,
        ] {
            assert!(parse_factory_limits(body).is_err(), "factory took {body:?}");
        }
    }

    #[test]
    fn clinepass_poll_sends_bearer_and_renders_a_snapshot() {
        let served = serve_once(ok_json(&fixture("clinepass-limits.json")));
        let snapshot = poll_clinepass(&served.endpoint, &PollSecret::new(SENTINEL), &limits())
            .unwrap_or_else(|e| panic!("poll failed: {e}"));
        assert_eq!(snapshot.provider, "cline-pass");
        assert!(close(used_percent_of(&snapshot.primary), 42.5));
        let request = served
            .request
            .recv_timeout(Duration::from_secs(5))
            .expect("server saw no request");
        // ureq lowercases header names on the wire; the value must survive.
        assert!(
            request.contains(&format!("authorization: Bearer {SENTINEL}")),
            "bearer header missing: {request}"
        );
        assert!(request.starts_with("GET /api/v1/users/me/plan/usage-limits"));
        let rendered = serde_json::to_string(&snapshot)
            .unwrap_or_else(|e| panic!("snapshot not serializable: {e}"));
        assert!(rendered.contains("\"provider\":\"cline-pass\""));
        assert!(!rendered.contains(SENTINEL));
    }

    #[test]
    fn warp_poll_posts_graphql_with_bearer_and_warp_user_agent() {
        let served = serve_once(ok_json(&fixture("warp-graphql.json")));
        let endpoint = served.endpoint.replace(
            "/api/v1/users/me/plan/usage-limits",
            "/graphql/v2?op=GetRequestLimitInfo",
        );
        let snapshot = poll_warp(&endpoint, &PollSecret::new(SENTINEL), &limits())
            .unwrap_or_else(|e| panic!("poll failed: {e}"));
        assert_eq!(snapshot.provider, "warp");
        let request = served
            .request
            .recv_timeout(Duration::from_secs(5))
            .expect("server saw no request");
        assert!(request.starts_with("POST /graphql/v2?op=GetRequestLimitInfo"));
        assert!(request.contains("user-agent: Warp/1.0"));
        assert!(request.contains(&format!("authorization: Bearer {SENTINEL}")));
        assert!(request.contains("GetRequestLimitInfo"));
    }

    #[test]
    fn zed_poll_sends_the_cookie_header_value_verbatim() {
        let served = serve_once(ok_json(&fixture("zed-billing.json")));
        let cookie_header = format!("zed.session={SENTINEL}; other=1");
        let snapshot = poll_zed(
            &served.endpoint,
            &PollSecret::new(&cookie_header),
            &limits(),
        )
        .unwrap_or_else(|e| panic!("poll failed: {e}"));
        assert_eq!(snapshot.provider, "zed");
        let request = served
            .request
            .recv_timeout(Duration::from_secs(5))
            .expect("server saw no request");
        assert!(
            request.contains(&format!("cookie: {cookie_header}")),
            "the full Cookie header value must reach the wire verbatim: {request}"
        );
        let rendered = serde_json::to_string(&snapshot)
            .unwrap_or_else(|e| panic!("snapshot not serializable: {e}"));
        assert!(!rendered.contains(SENTINEL));
    }

    #[test]
    fn rejected_credentials_and_server_failures_are_typed_errors() {
        let served = serve_once(http_response("401 Unauthorized", &[], "{}"));
        let error = poll_clinepass(&served.endpoint, &PollSecret::new(SENTINEL), &limits())
            .expect_err("401 must fail");
        assert!(
            matches!(error, PollError::Auth { status: 401 }),
            "got {error:?}"
        );

        let served = serve_once(http_response("429 Too Many Requests", &[], "{}"));
        let error = poll_clinepass(&served.endpoint, &PollSecret::new(SENTINEL), &limits())
            .expect_err("429 must fail");
        assert!(matches!(error, PollError::RateLimited), "got {error:?}");

        let served = serve_once(http_response("503 Service Unavailable", &[], "{}"));
        let error = poll_clinepass(&served.endpoint, &PollSecret::new(SENTINEL), &limits())
            .expect_err("503 must fail");
        assert!(
            matches!(error, PollError::Server { status: 503 }),
            "got {error:?}"
        );
    }

    #[test]
    fn redirects_are_refused_not_followed() {
        let served = serve_once(http_response(
            "302 Found",
            &[("Location", "https://evil.example.com/steal")],
            "",
        ));
        let error = poll_clinepass(&served.endpoint, &PollSecret::new(SENTINEL), &limits())
            .expect_err("redirect must fail");
        assert!(matches!(error, PollError::Redirect { .. }), "got {error:?}");
        assert!(!format!("{error}").contains(SENTINEL));
    }

    #[test]
    fn oversized_responses_stop_at_the_cap() {
        let served = serve_once(ok_json(&"x".repeat(200)));
        let tight = PollLimits::new(Duration::from_secs(5), 64)
            .unwrap_or_else(|e| panic!("tight limits refused: {e:?}"));
        let error = poll_clinepass(&served.endpoint, &PollSecret::new(SENTINEL), &tight)
            .expect_err("oversized body must fail");
        assert!(
            matches!(error, PollError::Oversized { limit: 64 }),
            "got {error:?}"
        );
    }

    #[test]
    fn a_silent_endpoint_hits_the_timeout_bound() {
        let endpoint = serve_silence();
        let one_second = PollLimits::new(Duration::from_secs(1), 1024)
            .unwrap_or_else(|e| panic!("timeout floor refused: {e:?}"));
        let started = std::time::Instant::now();
        let error = poll_clinepass(&endpoint, &PollSecret::new(SENTINEL), &one_second)
            .expect_err("silence must time out");
        assert!(matches!(error, PollError::Timeout), "got {error:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "poll ran past its bound: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn non_loopback_http_and_broken_urls_fail_before_any_request() {
        for url in [
            "http://api.cline.bot/api/v1/users/me/plan/usage-limits",
            "ftp://api.cline.bot/api/v1/users/me/plan/usage-limits",
            "https://user:pw@api.cline.bot/api/v1/users/me/plan/usage-limits",
            "http://127.0.0.1.attacker.com/api/v1/users/me/plan/usage-limits",
        ] {
            let error = poll_clinepass(url, &PollSecret::new(SENTINEL), &limits())
                .expect_err("{url} must be refused");
            assert!(
                matches!(error, PollError::Invalid { .. }),
                "{url}: {error:?}"
            );
        }
    }

    #[test]
    fn an_empty_credential_is_refused_without_a_request() {
        let error = poll_clinepass(
            "https://api.cline.bot/api/v1/users/me/plan/usage-limits",
            &PollSecret::new("   "),
            &limits(),
        )
        .expect_err("empty credential must fail");
        assert!(
            matches!(error, PollError::MissingCredential),
            "got {error:?}"
        );
    }

    #[test]
    fn failure_renderings_never_carry_the_credential() {
        // The server answers 500 with a body echoing the Authorization header it
        // received; no error rendering may repeat body bytes.
        let listener =
            TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| panic!("loopback bind: {e}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("loopback addr: {e}"));
        let endpoint = format!("http://{addr}/api/v1/users/me/plan/usage-limits");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(buf.get(..n).unwrap_or_default()).into_owned();
            let leaked = request
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                .map(str::to_owned)
                .unwrap_or_default();
            let body = format!("{{\"echo\": \"{leaked}\"}}");
            let response = http_response("500 Internal Server Error", &[], &body);
            drop(stream.write_all(response.as_bytes()));
        });
        let error = poll_clinepass(&endpoint, &PollSecret::new(SENTINEL), &limits())
            .expect_err("500 must fail");
        for rendering in [format!("{error}"), format!("{error:?}")] {
            assert!(!rendering.contains(SENTINEL), "leaked in {rendering}");
        }
    }

    #[test]
    fn minimax_policy_accepts_clean_https_and_refuses_everything_sketchy() {
        assert_eq!(
            validate_minimax_endpoint_override("platform.minimaxi.com", false)
                .unwrap_or_else(|e| panic!("bare host refused: {e}")),
            "https://platform.minimaxi.com"
        );
        assert_eq!(
            validate_minimax_endpoint_override("https://proxy.example.com:8443", false)
                .unwrap_or_else(|e| panic!("https host refused: {e}")),
            "https://proxy.example.com:8443"
        );
        for raw in [
            "http://platform.minimaxi.com",
            "https://user@platform.minimaxi.com",
            "https://user:pw@platform.minimaxi.com",
            "https://evil.example.com%2f.minimaxi.com",
            "https://evil.example.com%40.minimaxi.com",
            "https://evil%23.minimaxi.com",
            "https://[not-ipv6",
            "https://host:notaport",
            "   ",
        ] {
            let error = validate_minimax_endpoint_override(raw, false)
                .err()
                .unwrap_or_else(|| panic!("{raw} accepted"));
            assert!(format!("{error}").contains("refused"), "{raw}: {error}");
        }
    }

    #[test]
    fn minimax_strict_mode_accepts_only_minimax_owned_hosts() {
        assert!(validate_minimax_endpoint_override("www.minimax.io", true).is_ok());
        assert!(validate_minimax_endpoint_override("https://api.minimaxi.com", true).is_ok());
        assert!(validate_minimax_endpoint_override("https://proxy.example.com", true).is_err());
        // A real punycode subdomain stays MiniMax-owned; a confusable or
        // delimiter-laden host spelling must never pass the ownership check.
        assert!(
            validate_minimax_endpoint_override("xn--vil-gna.minimax.io", true).is_ok(),
            "punycode subdomains of a MiniMax domain are MiniMax-owned"
        );
        for raw in [
            "evil].minimax.io",
            "еvil.minimax.io",
            "https://еvil.minimaxi.com",
            "[::1]",
        ] {
            let error = validate_minimax_endpoint_override(raw, true)
                .err()
                .unwrap_or_else(|| panic!("{raw} passed strict mode"));
            assert!(format!("{error}").contains("refused"), "{raw}: {error}");
        }
        // The same charset rule holds outside strict mode: no delimiters,
        // no non-ASCII, while ordinary proxy hosts stay accepted.
        for raw in ["evil].example.com", "еvil.example.com", "under_score.io"] {
            assert!(
                validate_minimax_endpoint_override(raw, false).is_err(),
                "{raw} accepted"
            );
        }
        assert!(validate_minimax_endpoint_override("my-proxy.example.com", false).is_ok());
    }

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("superai-poller-{tag}-{}-{n}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
        dir
    }

    #[test]
    fn clinepass_providers_path_honors_clines_own_overrides() {
        let home = Path::new("/home/tester");
        assert_eq!(
            clinepass_providers_path(None, None, None, home),
            PathBuf::from("/home/tester/.cline/data/settings/providers.json")
        );
        assert_eq!(
            clinepass_providers_path(Some("/custom/providers.json"), Some("/ignored"), None, home),
            PathBuf::from("/custom/providers.json")
        );
        assert_eq!(
            clinepass_providers_path(None, Some("/data-dir"), None, home),
            PathBuf::from("/data-dir/settings/providers.json")
        );
        assert_eq!(
            clinepass_providers_path(None, None, Some("~/.cline"), home),
            PathBuf::from("/home/tester/.cline/data/settings/providers.json")
        );
        assert_eq!(
            clinepass_providers_path(Some("~/session.json"), None, None, home),
            PathBuf::from("/home/tester/session.json")
        );
        assert_eq!(
            clinepass_providers_path(Some("  "), None, None, home),
            PathBuf::from("/home/tester/.cline/data/settings/providers.json")
        );
    }

    #[test]
    fn clinepass_session_follows_clines_credential_precedence() {
        let dir = temp_dir("clinepass-precedence");
        let path = dir.join("providers.json");
        std::fs::write(&path, fixture("clinepass-providers.json"))
            .unwrap_or_else(|e| panic!("write fixture: {e}"));

        let session = clinepass_session(&path, 1_048_576)
            .unwrap_or_else(|e| panic!("session read failed: {e}"))
            .expect("session missing");
        assert!(session.is_oauth());
        assert!(!format!("{session:?}").contains("raw-access-token"));

        // accessToken absent: settings.apiKey wins over auth.apiKey.
        std::fs::write(
            &path,
            r#"{"providers": {"cline": {"settings": {"apiKey": "sk-static", "auth": {"apiKey": "sk-auth"}}}}}"#,
        )
        .unwrap_or_else(|e| panic!("rewrite: {e}"));
        let session = clinepass_session(&path, 1_048_576)
            .unwrap_or_else(|e| panic!("session read failed: {e}"))
            .expect("session missing");
        assert!(!session.is_oauth());

        // Both keys absent: auth.apiKey is the last resort.
        std::fs::write(
            &path,
            r#"{"providers": {"cline": {"settings": {"auth": {"apiKey": "sk-auth"}}}}}"#,
        )
        .unwrap_or_else(|e| panic!("rewrite: {e}"));
        assert!(
            clinepass_session(&path, 1_048_576)
                .unwrap_or_else(|e| panic!("session read failed: {e}"))
                .is_some()
        );

        // No cline entry, an unrelated provider only, or garbage: no session.
        for body in [
            "{}",
            "not json",
            r#"{"providers": {"other-provider": {"settings": {"apiKey": "sk-unrelated"}}}}"#,
            r#"{"providers": {"cline": {"settings": {}}}}"#,
        ] {
            std::fs::write(&path, body).unwrap_or_else(|e| panic!("rewrite: {e}"));
            assert!(
                clinepass_session(&path, 1_048_576)
                    .unwrap_or_else(|e| panic!("session read failed: {e}"))
                    .is_none(),
                "found a session in {body}"
            );
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn clinepass_session_read_leaves_the_file_untouched() {
        let dir = temp_dir("clinepass-readonly");
        let path = dir.join("providers.json");
        let original = fixture("clinepass-providers.json");
        std::fs::write(&path, &original).unwrap_or_else(|e| panic!("write fixture: {e}"));
        let before: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("list dir: {e}"))
            .map(|entry| entry.expect("dir entry").file_name())
            .collect();
        drop(clinepass_session(&path, 1_048_576));
        let after = std::fs::read(&path).unwrap_or_else(|e| panic!("re-read: {e}"));
        assert_eq!(after, original.as_bytes(), "session read mutated the file");
        let later: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("list dir: {e}"))
            .map(|entry| entry.expect("dir entry").file_name())
            .collect();
        assert_eq!(before, later, "session read created or removed files");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn clinepass_session_caps_runaway_files() {
        let dir = temp_dir("clinepass-cap");
        let path = dir.join("providers.json");
        std::fs::write(&path, "x".repeat(300)).unwrap_or_else(|e| panic!("write: {e}"));
        let error = clinepass_session(&path, 64).expect_err("oversized file must fail");
        assert!(
            matches!(error, PollError::Oversized { limit: 64 }),
            "got {error:?}"
        );
        assert!(
            clinepass_session(&dir.join("missing.json"), 64)
                .unwrap_or_else(|e| panic!("missing file errored: {e}"))
                .is_none()
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_fifo_is_refused_without_opening_it() {
        let dir = temp_dir("clinepass-fifo");
        let path = dir.join("providers.json");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap_or_else(|e| panic!("spawn mkfifo: {e}"));
        assert!(status.success(), "mkfifo(1) must exist to run this test");
        // Opening a FIFO with no writer wedges forever; the stat-first
        // refusal must answer before any open.
        let error = clinepass_session(&path, 64).expect_err("a FIFO must be refused");
        assert!(matches!(error, PollError::Invalid { .. }), "got {error:?}");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn poller_source_carries_no_background_machinery() {
        let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/poller.rs"))
            .unwrap_or_else(|e| panic!("read poller source: {e}"));
        let banned = [
            "loop",
            "while",
            "spawn",
            "thread",
            "async",
            "await",
            "sleep",
            "tokio",
            "watcher",
            "daemon",
            "interval",
            "schedule",
            "background",
        ];
        for token in source.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            assert!(
                !banned.contains(&token),
                "poll path must stay one-shot; found `{token}`"
            );
        }
    }

    #[test]
    fn poll_functions_are_one_shot_by_signature() {
        // A watcher or recurring handle could not satisfy these fn-pointer
        // types: each poll returns an owned snapshot for one explicit call.
        let _: fn(&str, &PollSecret, &PollLimits) -> Result<ProviderUsageSnapshot, PollError> =
            poll_clinepass;
        let _: fn(&str, &PollSecret, &PollLimits) -> Result<ProviderUsageSnapshot, PollError> =
            poll_warp;
    }
}
