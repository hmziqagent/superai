//! llmproxy ledger client tests (USG-08): endpoint parsing against the
//! fixture-recorded shapes, token-file discipline, auth-header redaction
//! across every error path, the divergence-line merge with the local cost
//! engine, and degradation when the proxy is unreachable. The mock is a
//! `std::net::TcpListener` on 127.0.0.1; no network beyond loopback.

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use std::str::FromStr;

    use jiff::Timestamp;
    use rusqlite as _;
    use serde as _;
    use serde_json as _;
    use superai_core as _;
    use thiserror as _;
    use ureq as _;

    use superai_usage::abuse::contains_any_sentinel;
    use superai_usage::cost::{
        CostEngine, CostPeriod, RateTable, RecordedUsage, Spend, UsageEvent, Usd,
    };
    use superai_usage::llmproxy::{
        AdminToken, LedgerClient, LedgerCost, LedgerCrossCheck, LedgerError, RequestsQuery,
        SummaryGroupBy, load_admin_token,
    };
    use superai_usage::poller::PollLimits;

    // abuse.rs sentinel: planted as the admin token everywhere, so
    // assert_clean fails if any path echoes it.
    const TOKEN: &str = superai_usage::abuse::LLMPROXY_ADMIN_SENTINEL;

    enum Reply {
        Respond { status: &'static str, body: String },
        Redirect { location: String },
        Stall,
        Reset,
    }

    #[derive(Clone)]
    struct Captured {
        target: String,
        authorization: Option<String>,
    }

    struct MockLedger {
        addr: SocketAddr,
        requests: Arc<Mutex<Vec<Captured>>>,
    }

    impl MockLedger {
        fn start(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .unwrap_or_else(|e| panic!("bind loopback: {e:?}"));
            let addr = listener
                .local_addr()
                .unwrap_or_else(|e| panic!("local addr: {e:?}"));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = Arc::clone(&requests);
            let queue: VecDeque<Reply> = replies.into();
            std::thread::spawn(move || {
                accept_loop(listener, &captured, queue);
            });
            Self { addr, requests }
        }

        fn base_url(&self) -> String {
            format!("http://{}", self.addr)
        }

        fn sent(&self) -> Vec<Captured> {
            self.requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "the listener moves into the server thread and dies with it"
    )]
    fn accept_loop(
        listener: TcpListener,
        captured: &Mutex<Vec<Captured>>,
        mut queue: VecDeque<Reply>,
    ) {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let Some(reply) = queue.pop_front() else {
                break;
            };
            serve(&mut stream, captured, reply);
        }
    }

    fn serve(stream: &mut TcpStream, captured: &Mutex<Vec<Captured>>, reply: Reply) {
        drop(stream.set_read_timeout(Some(Duration::from_secs(5))));
        let raw = read_head(stream);
        let target = raw
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or_default()
            .to_owned();
        let authorization = raw.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().to_owned())
        });
        captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Captured {
                target,
                authorization,
            });
        match reply {
            Reply::Respond { status, body } => {
                write_plain(stream, status, &[], &body);
            }
            Reply::Redirect { location } => {
                write_plain(stream, "302 Found", &[("Location", &location)], "");
            }
            Reply::Stall => {
                std::thread::sleep(Duration::from_secs(3));
                write_plain(stream, "500 Internal Server Error", &[], "late");
            }
            Reply::Reset => drop(stream.shutdown(Shutdown::Both)),
        }
    }

    fn read_head(stream: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 2048];
        while !(buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 65536) {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    fn write_plain(
        stream: &mut TcpStream,
        status: &'static str,
        headers: &[(&str, &str)],
        body: &str,
    ) {
        let mut head = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        drop(stream.write_all(head.as_bytes()));
        drop(stream.write_all(body.as_bytes()));
        drop(stream.flush());
    }

    fn fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/llmproxy")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
    }

    fn limits() -> PollLimits {
        PollLimits::new(Duration::from_secs(5), 64 * 1024)
            .unwrap_or_else(|e| panic!("valid limits rejected: {e:?}"))
    }

    fn make_client(base: &str) -> LedgerClient {
        LedgerClient::new(base, &AdminToken::new(TOKEN), limits())
            .unwrap_or_else(|e| panic!("valid client rejected: {e:?}"))
    }

    fn assert_clean(label: &str, text: &str) {
        assert!(
            !contains_any_sentinel(text.as_bytes()),
            "{label} leaked the admin token: {text}"
        );
    }

    #[test]
    fn summary_fixture_parses_and_carries_the_bearer_header() {
        let mock = MockLedger::start(vec![Reply::Respond {
            status: "200 OK",
            body: fixture("summary-provider.json"),
        }]);
        let client = make_client(&mock.base_url());

        let summary = client
            .summary("24h", SummaryGroupBy::Provider)
            .unwrap_or_else(|e| panic!("fixture summary failed: {e:?}"));

        assert_eq!(summary.window.as_deref(), Some("24h"));
        assert_eq!(summary.rows.len(), 2);
        let anthropic = summary
            .rows
            .first()
            .unwrap_or_else(|| panic!("no summary rows"));
        assert_eq!(anthropic.group.as_deref(), Some("anthropic"));
        assert_eq!(anthropic.requests, Some(12));
        assert_eq!(anthropic.errors, Some(1));
        assert_eq!(anthropic.input_tokens, Some(38_211));
        assert_eq!(anthropic.output_tokens, Some(9_120));
        assert_eq!(anthropic.cache_read_tokens, Some(5_000));
        assert_eq!(anthropic.cache_write_tokens, Some(1_000));
        assert_eq!(anthropic.cost_usd.map(Usd::as_picos), Some(3_645_500_000));
        assert_eq!(summary.total_cost().map(Usd::as_picos), Some(4_611_175_000));

        let sent = mock.sent();
        let request = sent.first().unwrap_or_else(|| panic!("no request seen"));
        assert!(
            request.target.starts_with("/usage/summary?"),
            "target was {}",
            request.target
        );
        assert!(request.target.contains("window=24h"));
        assert!(request.target.contains("group_by=provider"));
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert_clean("client debug", &format!("{client:?}"));
        assert_clean("summary debug", &format!("{summary:?}"));
    }

    #[test]
    fn every_group_by_wire_value_reaches_the_query() {
        let mock = MockLedger::start(vec![
            Reply::Respond {
                status: "200 OK",
                body: "{\"rows\":[]}".to_owned(),
            },
            Reply::Respond {
                status: "200 OK",
                body: "{\"rows\":[]}".to_owned(),
            },
        ]);
        let client = make_client(&mock.base_url());
        for group_by in [SummaryGroupBy::Model, SummaryGroupBy::Caller] {
            client
                .summary("1h", group_by)
                .unwrap_or_else(|e| panic!("summary for {group_by:?} failed: {e:?}"));
        }
        let sent = mock.sent();
        let targets: Vec<&str> = sent.iter().map(|c| c.target.as_str()).collect();
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|t| t.contains("group_by=model")));
        assert!(targets.iter().any(|t| t.contains("group_by=caller")));
    }

    #[test]
    fn requests_fixture_parses_and_percent_encodes_its_filters() {
        let mock = MockLedger::start(vec![Reply::Respond {
            status: "200 OK",
            body: fixture("requests-window.json"),
        }]);
        let client = make_client(&mock.base_url());
        let query = RequestsQuery {
            since: Some(
                Timestamp::from_str("2026-10-13T00:00:00Z")
                    .unwrap_or_else(|e| panic!("since: {e:?}")),
            ),
            until: Some(
                Timestamp::from_str("2026-10-13T12:00:00Z")
                    .unwrap_or_else(|e| panic!("until: {e:?}")),
            ),
            provider: Some("amazon bedrock"),
            limit: Some(50),
        };

        let requests = client
            .requests(&query)
            .unwrap_or_else(|e| panic!("fixture requests failed: {e:?}"));

        assert_eq!(requests.rows.len(), 2);
        let first = requests.rows.first().unwrap_or_else(|| panic!("no rows"));
        assert_eq!(first.request_id.as_deref(), Some("req_018f3a2b"));
        assert!(first.received_at.is_some());
        assert!(first.completed_at.is_some());
        assert_eq!(first.provider.as_deref(), Some("anthropic"));
        assert_eq!(first.model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(first.caller.as_deref(), Some("superai"));
        assert_eq!(first.streaming, Some(true));
        assert_eq!(first.http_status, Some(200));
        assert_eq!(first.cost_usd.map(Usd::as_picos), Some(3_645_500_000));
        let second = requests
            .rows
            .get(1)
            .unwrap_or_else(|| panic!("no second row"));
        assert_eq!(
            second.received_at.map(Timestamp::as_millisecond),
            Some(1_760_342_397_000)
        );
        assert_eq!(second.completed_at, None);
        assert_eq!(second.cost_usd.map(Usd::as_picos), Some(965_675_000));
        assert_eq!(second.error_kind.as_deref(), Some("upstream_timeout"));

        let sent = mock.sent();
        let target = &sent
            .first()
            .unwrap_or_else(|| panic!("no request seen"))
            .target;
        assert!(target.starts_with("/usage/requests?"));
        assert!(target.contains("since=2026-10-13"));
        assert!(target.contains("until=2026-10-13"));
        assert!(
            target.contains("provider=amazon%20bedrock"),
            "provider filter not encoded: {target}"
        );
        assert!(target.contains("limit=50"));
    }

    #[test]
    fn keys_fixture_parses() {
        let mock = MockLedger::start(vec![Reply::Respond {
            status: "200 OK",
            body: fixture("keys.json"),
        }]);
        let client = make_client(&mock.base_url());

        let keys = client
            .keys()
            .unwrap_or_else(|e| panic!("fixture keys failed: {e:?}"));

        assert_eq!(keys.keys.len(), 2);
        let first = keys.keys.first().unwrap_or_else(|| panic!("no keys"));
        assert_eq!(first.caller.as_deref(), Some("superai"));
        assert!(first.last_seen.is_some());
        let second = keys.keys.get(1).unwrap_or_else(|| panic!("no second key"));
        assert_eq!(second.caller.as_deref(), Some("anonymous"));
        assert_eq!(second.last_seen, None);
        let sent = mock.sent();
        assert_eq!(
            sent.first().unwrap_or_else(|| panic!("no request")).target,
            "/usage/keys"
        );
    }

    #[test]
    fn drifted_bodies_yield_typed_unavailables_or_a_loud_parse_error() {
        let mock = MockLedger::start(vec![
            Reply::Respond {
                status: "200 OK",
                body: "{\"rows\":[{\"group\":\"anthropic\",\"note\":\"unknown field\"},{\"group\":null}]}"
                    .to_owned(),
            },
            Reply::Respond {
                status: "200 OK",
                body: "{\"rows\":[]}".to_owned(),
            },
            Reply::Respond {
                status: "200 OK",
                body: "{\"window\":\"24h\"}".to_owned(),
            },
            Reply::Respond {
                status: "200 OK",
                body: format!("{{\"rows\":[{{\"requests\":\"{TOKEN}\"}}]}}"),
            },
            Reply::Respond {
                status: "200 OK",
                body: "not json".to_owned(),
            },
        ]);
        let client = make_client(&mock.base_url());

        let drifted = client
            .summary("24h", SummaryGroupBy::Provider)
            .unwrap_or_else(|e| panic!("drifted rows failed: {e:?}"));
        assert_eq!(drifted.rows.len(), 2);
        let sparse = drifted
            .rows
            .first()
            .unwrap_or_else(|| panic!("no drifted row"));
        assert_eq!(sparse.group.as_deref(), Some("anthropic"));
        assert_eq!(sparse.requests, None);
        assert_eq!(sparse.errors, None);
        assert_eq!(sparse.cost_usd, None);
        let anonymous = drifted
            .rows
            .get(1)
            .unwrap_or_else(|| panic!("no second drifted row"));
        assert_eq!(anonymous.group, None);
        assert_eq!(drifted.total_cost(), None);

        let empty = client
            .summary("24h", SummaryGroupBy::Provider)
            .unwrap_or_else(|e| panic!("empty rows failed: {e:?}"));
        assert_eq!(empty.rows.len(), 0);
        assert_eq!(empty.total_cost(), Some(Usd::ZERO));

        let missing_rows = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("a summary without rows should not parse"));
        assert!(
            matches!(missing_rows, LedgerError::Parse { .. }),
            "{missing_rows:?}"
        );

        let wrong_typed = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("a sentinel-typed count should not parse"));
        assert!(
            matches!(wrong_typed, LedgerError::Parse { .. }),
            "{wrong_typed:?}"
        );
        let reason = wrong_typed.to_string();
        assert_clean("parse error", &reason);
        assert_clean("parse error debug", &format!("{wrong_typed:?}"));
        assert!(
            reason.contains("line"),
            "the reason should still locate the drift: {reason}"
        );

        let garbage = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("garbage should not parse"));
        assert!(matches!(garbage, LedgerError::Parse { .. }), "{garbage:?}");
    }

    #[test]
    fn every_error_path_stays_typed_and_redacted() {
        let scenarios: Vec<(&str, Reply, fn(&LedgerError) -> bool)> = vec![
            (
                "server error",
                Reply::Respond {
                    status: "500 Internal Server Error",
                    body: "boom".to_owned(),
                },
                |e| matches!(e, LedgerError::Status { code: 500 }),
            ),
            (
                "wrong token",
                Reply::Respond {
                    status: "401 Unauthorized",
                    body: "{\"error\":\"denied\"}".to_owned(),
                },
                |e| matches!(e, LedgerError::Status { code: 401 }),
            ),
            ("reset connection", Reply::Reset, |e| {
                matches!(e, LedgerError::Unreachable { .. })
            }),
        ];
        for (label, reply, expected) in scenarios {
            let mock = MockLedger::start(vec![reply]);
            let client = make_client(&mock.base_url());
            let error = client
                .summary("24h", SummaryGroupBy::Provider)
                .err()
                .unwrap_or_else(|| panic!("{label}: expected an error"));
            assert!(expected(&error), "{label}: wrong error {error:?}");
            assert_clean(label, &error.to_string());
            assert_clean(label, &format!("{error:?}"));
            let sent = mock.sent();
            assert_eq!(
                sent.first()
                    .unwrap_or_else(|| panic!("{label}: no request"))
                    .authorization
                    .as_deref(),
                Some(format!("Bearer {TOKEN}").as_str()),
                "{label}: auth was never sent, so redaction proved nothing"
            );
        }
    }

    #[test]
    fn timeout_and_refused_connections_degrade_gracefully() {
        let stalling = MockLedger::start(vec![Reply::Stall]);
        let quick = PollLimits::new(Duration::from_secs(1), 64 * 1024)
            .unwrap_or_else(|e| panic!("valid limits rejected: {e:?}"));
        let client = LedgerClient::new(&stalling.base_url(), &AdminToken::new(TOKEN), quick)
            .unwrap_or_else(|e| panic!("valid client rejected: {e:?}"));
        let error = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("stalled ledger should time out"));
        assert!(
            matches!(error, LedgerError::Unreachable { .. }),
            "{error:?}"
        );
        assert_clean("timeout", &error.to_string());

        let error = client
            .requests(&RequestsQuery::ALL)
            .err()
            .unwrap_or_else(|| panic!("stalled ledger should time out"));
        assert!(
            matches!(error, LedgerError::Unreachable { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_dead_port_is_unreachable_and_never_leaks_the_token() {
        let port = {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .unwrap_or_else(|e| panic!("bind loopback: {e:?}"));
            listener
                .local_addr()
                .unwrap_or_else(|e| panic!("addr: {e:?}"))
                .port()
        };
        let client = LedgerClient::new(
            &format!("http://127.0.0.1:{port}"),
            &AdminToken::new(TOKEN),
            limits(),
        )
        .unwrap_or_else(|e| panic!("valid client rejected: {e:?}"));
        let error = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("dead port should not answer"));
        assert!(
            matches!(error, LedgerError::Unreachable { .. }),
            "{error:?}"
        );
        assert_clean("refused", &error.to_string());
    }

    #[test]
    fn oversize_responses_are_refused_by_the_byte_cap() {
        let oversized = format!("{}{TOKEN}", "x".repeat(4096));
        let mock = MockLedger::start(vec![Reply::Respond {
            status: "200 OK",
            body: oversized,
        }]);
        let tight = PollLimits::new(Duration::from_secs(5), 128)
            .unwrap_or_else(|e| panic!("valid limits rejected: {e:?}"));
        let client = LedgerClient::new(&mock.base_url(), &AdminToken::new(TOKEN), tight)
            .unwrap_or_else(|e| panic!("valid client rejected: {e:?}"));

        let error = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("oversized body should not parse"));
        assert!(
            matches!(error, LedgerError::TooLarge { max_bytes: 128 }),
            "{error:?}"
        );
        assert_clean("byte-cap error", &error.to_string());
        assert_clean("byte-cap error debug", &format!("{error:?}"));
    }

    #[test]
    fn redirect_bytes_never_reach_error_text() {
        let exfil = MockLedger::start(vec![Reply::Redirect {
            location: format!("https://collector.example/leak?tok={TOKEN}"),
        }]);
        let client = make_client(&exfil.base_url());
        let error = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("a Location that smuggles the token must be refused"));
        assert!(
            matches!(error, LedgerError::InvalidBase { .. }),
            "{error:?}"
        );
        assert_clean("location-smuggling redirect", &error.to_string());
        assert_clean("location-smuggling redirect debug", &format!("{error:?}"));
        assert_eq!(exfil.sent().len(), 1);

        let same_origin = MockLedger::start(vec![
            Reply::Redirect {
                location: format!("/usage/summary?echo={TOKEN}"),
            },
            Reply::Reset,
        ]);
        let client = make_client(&same_origin.base_url());
        let error = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("the smuggled same-origin hop should fail on reset"));
        assert!(
            matches!(error, LedgerError::Unreachable { .. }),
            "{error:?}"
        );
        assert_clean("redirect-derived url", &error.to_string());
        assert_clean("redirect-derived url debug", &format!("{error:?}"));

        let huge = MockLedger::start(vec![Reply::Redirect {
            location: format!("/{}", "a".repeat(8192)),
        }]);
        let client = make_client(&huge.base_url());
        let error = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("the oversized same-origin hop should fail to answer"));
        assert!(
            matches!(error, LedgerError::Unreachable { .. }),
            "{error:?}"
        );
        let rendered = error.to_string();
        assert_clean("oversized location", &rendered);
        assert!(
            rendered.chars().count() < 300,
            "error text must stay bounded: {} chars",
            rendered.chars().count()
        );
    }

    #[test]
    fn redirects_stay_on_the_ledger_origin_or_are_refused() {
        let mock = MockLedger::start(vec![
            Reply::Redirect {
                location: "/usage/summary?retry=1".to_owned(),
            },
            Reply::Respond {
                status: "200 OK",
                body: fixture("summary-provider.json"),
            },
        ]);
        let client = make_client(&mock.base_url());
        let summary = client
            .summary("24h", SummaryGroupBy::Provider)
            .unwrap_or_else(|e| panic!("same-origin redirect failed: {e:?}"));
        assert_eq!(summary.rows.len(), 2);
        let sent = mock.sent();
        assert_eq!(sent.len(), 2);
        for request in &sent {
            assert_eq!(
                request.authorization.as_deref(),
                Some(format!("Bearer {TOKEN}").as_str()),
                "same-origin redirect must keep the admin auth"
            );
            assert!(request.target.starts_with("/usage/summary"));
        }

        let off_origin = MockLedger::start(vec![Reply::Respond {
            status: "200 OK",
            body: "{\"rows\":[]}".to_owned(),
        }]);
        let malicious = MockLedger::start(vec![Reply::Redirect {
            location: format!("{}/usage/summary", off_origin.base_url()),
        }]);
        let client = make_client(&malicious.base_url());
        let error = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("cross-origin redirect must be refused"));
        assert!(
            matches!(error, LedgerError::InvalidBase { .. }),
            "{error:?}"
        );
        assert!(
            error.to_string().contains("origin"),
            "reason should name the origin rule: {error}"
        );
        assert_eq!(
            off_origin.sent().len(),
            0,
            "the token must never reach the off-origin host"
        );
        assert_eq!(malicious.sent().len(), 1);
        assert_clean("cross-origin redirect", &error.to_string());

        let collector = MockLedger::start(vec![Reply::Redirect {
            location: "https://collector.example/usage/summary".to_owned(),
        }]);
        let client = make_client(&collector.base_url());
        let error = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("a shape-valid foreign https target must still be refused"));
        assert!(
            matches!(error, LedgerError::InvalidBase { .. }),
            "{error:?}"
        );
        assert!(
            error.to_string().contains("origin"),
            "reason should name the origin rule: {error}"
        );
        assert_eq!(collector.sent().len(), 1);

        let refused = MockLedger::start(vec![Reply::Redirect {
            location: "http://user:secret@127.0.0.1:9/usage/summary".to_owned(),
        }]);
        let client = make_client(&refused.base_url());
        let error = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("userinfo-bearing redirect should be refused"));
        assert!(
            matches!(error, LedgerError::InvalidBase { .. }),
            "{error:?}"
        );
        assert_eq!(refused.sent().len(), 1);
        assert_clean("redirect", &error.to_string());

        let relative = MockLedger::start(vec![Reply::Redirect {
            location: "usage/summary".to_owned(),
        }]);
        let client = make_client(&relative.base_url());
        assert!(matches!(
            client.summary("24h", SummaryGroupBy::Provider),
            Err(LedgerError::InvalidBase { .. })
        ));
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-usage-llmproxy-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir: {e}"));
        dir
    }

    #[test]
    fn token_files_outside_the_0600_discipline_are_refused() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("token");
        let token_path = dir.join("admin-token");

        std::fs::write(&token_path, format!("{TOKEN}\n"))
            .unwrap_or_else(|e| panic!("write token: {e}"));
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o644))
            .unwrap_or_else(|e| panic!("chmod 644: {e}"));
        let error = load_admin_token(None, Some(&token_path))
            .err()
            .unwrap_or_else(|| panic!("world-readable token file accepted"));
        assert!(matches!(error, LedgerError::TokenFile { .. }), "{error:?}");
        assert!(error.to_string().contains("0600"), "{error}");
        assert_clean("token-file error", &error.to_string());
        assert_clean("token-file debug", &format!("{error:?}"));

        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|e| panic!("chmod 600: {e}"));
        let token = load_admin_token(None, Some(&token_path))
            .unwrap_or_else(|e| panic!("0600 token file refused: {e:?}"));
        assert_eq!(token, AdminToken::new(TOKEN));
        assert_clean("token debug", &format!("{token:?}"));
        assert_eq!(token.to_string(), "[REDACTED]");

        let empty = dir.join("empty");
        std::fs::write(&empty, "").unwrap_or_else(|e| panic!("write empty: {e}"));
        std::fs::set_permissions(&empty, std::fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|e| panic!("chmod 600: {e}"));
        assert!(matches!(
            load_admin_token(None, Some(&empty)),
            Err(LedgerError::TokenFile { .. })
        ));

        let oversized = dir.join("oversized");
        std::fs::write(&oversized, "x".repeat(5000)).unwrap_or_else(|e| panic!("write big: {e}"));
        std::fs::set_permissions(&oversized, std::fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|e| panic!("chmod 600: {e}"));
        assert!(matches!(
            load_admin_token(None, Some(&oversized)),
            Err(LedgerError::TokenFile { .. })
        ));

        assert!(matches!(
            load_admin_token(None, Some(&dir)),
            Err(LedgerError::TokenFile { .. })
        ));

        let linked = dir.join("linked");
        std::os::unix::fs::symlink(&token_path, &linked).unwrap_or_else(|e| panic!("symlink: {e}"));
        assert!(matches!(
            load_admin_token(None, Some(&linked)),
            Err(LedgerError::TokenFile { .. })
        ));

        let missing = dir.join("missing");
        assert!(matches!(
            load_admin_token(None, Some(&missing)),
            Err(LedgerError::TokenFile { .. })
        ));

        assert!(matches!(
            load_admin_token(None, None),
            Err(LedgerError::MissingToken)
        ));
        let blanks_env = load_admin_token(Some("  "), Some(&token_path))
            .unwrap_or_else(|e| panic!("blank env fell through to a refused file: {e:?}"));
        assert_eq!(blanks_env, AdminToken::new(TOKEN));
        let from_env = load_admin_token(Some(" env-secret "), Some(&token_path))
            .unwrap_or_else(|e| panic!("env token refused: {e:?}"));
        assert_eq!(from_env, AdminToken::new("env-secret"));

        drop(std::fs::remove_dir_all(&dir));
    }

    fn local_spend_usd(usage: &RecordedUsage) -> Option<Usd> {
        let engine = CostEngine::new(RateTable::bundled().unwrap_or_else(|e| panic!("{e:?}")));
        let now =
            Timestamp::from_str("2026-10-13T12:00:00Z").unwrap_or_else(|e| panic!("now: {e:?}"));
        let report = engine
            .total(&CostPeriod::AllHistory, now, "UTC", usage)
            .unwrap_or_else(|e| panic!("local total failed: {e:?}"));
        match report.total.spend {
            Spend::Exact(amount) => Some(amount),
            Spend::Zero => Some(Usd::ZERO),
            Spend::Unavailable { .. } => None,
        }
    }

    fn routed_local_usage() -> RecordedUsage {
        RecordedUsage {
            events: vec![UsageEvent {
                timestamp: Timestamp::from_str("2026-10-13T08:00:00Z")
                    .unwrap_or_else(|e| panic!("event ts: {e:?}")),
                provider: "anthropic".to_owned(),
                model: "claude-sonnet-4-5".to_owned(),
                input_tokens: 1_000_000,
                output_tokens: 0,
                cache_read_tokens: None,
                cache_write_tokens: None,
                cache_write_1h_tokens: None,
            }],
            unscanned_records: 0,
        }
    }

    #[test]
    fn ledger_numbers_win_and_local_stays_a_cross_check() {
        let local = local_spend_usd(&routed_local_usage());
        let local = local.unwrap_or_else(|| panic!("bundled rates should price the local scan"));

        let mock = MockLedger::start(vec![Reply::Respond {
            status: "200 OK",
            body: fixture("summary-provider.json"),
        }]);
        let client = make_client(&mock.base_url());
        let ledger = client
            .summary("24h", SummaryGroupBy::Provider)
            .unwrap_or_else(|e| panic!("summary failed: {e:?}"))
            .total_cost();

        let check = LedgerCrossCheck::new(LedgerCost::Reported(ledger), Some(local));
        let ledger_value = ledger.unwrap_or_else(|| panic!("fixture carries costs"));
        assert_ne!(ledger_value, local);
        assert_eq!(check.winning_cost(), Some(ledger_value));
        assert!(check.diverged());
        let expected = (ledger_value.as_picos() - local.as_picos()).abs();
        assert_eq!(check.divergence().map(Usd::as_picos), Some(expected));

        let body = format!("{{\"rows\":[{{\"group\":\"anthropic\",\"cost_usd\":\"{local}\"}}]}}");
        let agreeing = MockLedger::start(vec![Reply::Respond {
            status: "200 OK",
            body,
        }]);
        let client = make_client(&agreeing.base_url());
        let same = client
            .summary("24h", SummaryGroupBy::Provider)
            .unwrap_or_else(|e| panic!("summary failed: {e:?}"))
            .total_cost();
        let check = LedgerCrossCheck::new(LedgerCost::Reported(same), Some(local));
        assert_eq!(check.winning_cost(), Some(local));
        assert!(!check.diverged());

        let unpriced = MockLedger::start(vec![Reply::Respond {
            status: "200 OK",
            body: "{\"rows\":[{\"group\":\"anthropic\"}]}".to_owned(),
        }]);
        let client = make_client(&unpriced.base_url());
        let drifted = client
            .summary("24h", SummaryGroupBy::Provider)
            .unwrap_or_else(|e| panic!("summary failed: {e:?}"))
            .total_cost();
        let check = LedgerCrossCheck::new(LedgerCost::Reported(drifted), Some(local));
        assert_eq!(check.winning_cost(), None);
        assert!(!check.diverged());
    }

    #[test]
    fn unreachable_proxy_leaves_local_numbers_standing_alone() {
        let local = local_spend_usd(&routed_local_usage());
        let port = {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .unwrap_or_else(|e| panic!("bind loopback: {e:?}"));
            listener
                .local_addr()
                .unwrap_or_else(|e| panic!("addr: {e:?}"))
                .port()
        };
        let client = LedgerClient::new(
            &format!("http://127.0.0.1:{port}"),
            &AdminToken::new(TOKEN),
            limits(),
        )
        .unwrap_or_else(|e| panic!("valid client rejected: {e:?}"));

        let error = client
            .summary("24h", SummaryGroupBy::Provider)
            .err()
            .unwrap_or_else(|| panic!("dead port should not answer"));
        assert_clean("degraded reason", &error.to_string());

        let check = LedgerCrossCheck::new(
            LedgerCost::Unreachable {
                reason: error.to_string(),
            },
            local,
        );
        assert_eq!(check.winning_cost(), None);
        assert_eq!(check.divergence(), None);
        assert!(!check.diverged());
        assert_eq!(check.local, local);
    }
}
