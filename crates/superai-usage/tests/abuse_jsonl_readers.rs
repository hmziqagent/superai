//! Abuse tests for the JSONL readers (claude-code, codex-cli, pi): planted
//! secrets in ignored fields, malformed lines, oversized records, and
//! sentinel-named files never reach parsed output, error text, or cost rows.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use jiff::Timestamp;
    use rusqlite as _;
    use serde as _;
    use serde_json::json;
    use superai_core as _;
    use superai_usage::abuse::{
        ACCOUNT_ID_SENTINEL, API_KEY_SENTINEL, BEARER_SENTINEL, COOKIE_SENTINEL,
        OPENCODE_CREDENTIAL_SENTINEL, assert_no_sentinel_debug, assert_no_sentinel_json,
    };
    use superai_usage::cost::{
        CostEngine, CostPeriod, PeriodReport, RateTable, ScanCoverage, Spend,
    };
    use superai_usage::reader::ScanOutcome;
    use superai_usage::reader::jsonl::{
        PiSelectors, claude_projects_roots, codex_sessions_roots, pi_session_roots,
        scan_claude_transcripts, scan_codex_rollouts, scan_pi_sessions,
    };
    use thiserror as _;
    use ureq as _;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_home(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-abuse-jsonl-{tag}-{}-{n}-{nanos}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
        dir
    }

    fn write_lines(path: &Path, lines: &[String]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("mkdir {}: {e}", parent.display()));
        }
        let mut body = lines.join("\n");
        if !lines.is_empty() {
            body.push('\n');
        }
        fs::write(path, body).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }

    fn claude_assistant(model: &str, input: u64, output: u64) -> String {
        json!({
            "type": "assistant",
            "timestamp": "2026-10-12T10:00:00Z",
            "message": {
                "id": format!("msg-{model}-{input}"),
                "model": model,
                "usage": {"input_tokens": input, "output_tokens": output}
            }
        })
        .to_string()
    }

    /// The engine-composition path a `superai usage` row travels; every shape
    /// it can produce must scan clean.
    fn clean_report(outcome: &ScanOutcome, ctx: &str) -> PeriodReport {
        assert_no_sentinel_debug(outcome, ctx);
        let recorded = outcome.clone().into_recorded_usage();
        assert_no_sentinel_debug(&recorded, ctx);
        assert_no_sentinel_json(&recorded, ctx);
        let rates = RateTable::bundled().unwrap_or_else(|e| panic!("bundled rates: {e:?}"));
        let now = Timestamp::from_second(1_800_000_000).unwrap_or_else(|e| panic!("now: {e:?}"));
        let report = CostEngine::new(rates)
            .total(&CostPeriod::AllHistory, now, "UTC", &recorded)
            .unwrap_or_else(|e| panic!("total: {e:?}"));
        assert_no_sentinel_debug(&report, ctx);
        assert_no_sentinel_json(&report, ctx);
        report
    }

    #[test]
    fn claude_secrets_in_ignored_fields_and_junk_lines_stay_in_the_file() {
        let home = temp_home("claude");
        let transcript = home
            .join(".config")
            .join("claude")
            .join("projects")
            .join("proj-a")
            .join("session.jsonl");
        write_lines(
            &transcript,
            &[
                claude_assistant("claude-sonnet-4-5", 100, 40),
                json!({
                    "type": "assistant",
                    "timestamp": "2026-10-12T10:01:00Z",
                    "cookie": COOKIE_SENTINEL,
                    "metadata": {"accountId": ACCOUNT_ID_SENTINEL},
                    "message": {
                        "id": "msg-secret",
                        "model": "claude-sonnet-4-5",
                        "api_key": API_KEY_SENTINEL,
                        "usage": {"input_tokens": 10, "output_tokens": 5}
                    }
                })
                .to_string(),
                json!({"type": "user", "auth": BEARER_SENTINEL, "text": "hi"}).to_string(),
                BEARER_SENTINEL.to_owned(),
                format!("\"{}\"", OPENCODE_CREDENTIAL_SENTINEL.repeat(60_000)),
            ],
        );
        let named = home
            .join(".config")
            .join("claude")
            .join("projects")
            .join("proj-b")
            .join(format!("{API_KEY_SENTINEL}.jsonl"));
        write_lines(&named, &[claude_assistant("claude-haiku-4-5", 7, 3)]);

        let roots = claude_projects_roots(None, None, &home);
        let outcome = scan_claude_transcripts(&roots);

        assert_eq!(
            outcome.events.len(),
            3,
            "planted junk must not suppress usage"
        );
        assert!(
            outcome.counts.skipped >= 2,
            "the bare-token line and the oversized line must be skips"
        );
        let report = clean_report(&outcome, "claude abuse corpus");
        assert!(
            matches!(report.total.spend, Spend::Exact(_)),
            "priced rows must still total"
        );
        assert!(
            matches!(
                report.total.coverage,
                ScanCoverage::Incomplete { unscanned_records: gap } if gap >= 2
            ),
            "secret-bearing junk must surface as an honest gap, never a silent zero"
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_unreadable_sentinel_named_file_counts_without_leaking() {
        use std::os::unix::fs::PermissionsExt as _;

        let home = temp_home("claude-unreadable");
        let dir = home
            .join(".config")
            .join("claude")
            .join("projects")
            .join("p");
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let locked = dir.join(format!("{BEARER_SENTINEL}.jsonl"));
        fs::write(&locked, format!("{{\"api_key\":\"{API_KEY_SENTINEL}\"}}\n"))
            .unwrap_or_else(|e| panic!("write: {e}"));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))
            .unwrap_or_else(|e| panic!("chmod: {e}"));

        let roots = claude_projects_roots(None, None, &home);
        let outcome = scan_claude_transcripts(&roots);

        assert_eq!(outcome.unreadable_artifacts, 1);
        clean_report(&outcome, "claude unreadable file");
        drop(fs::remove_dir_all(&home));
    }

    #[test]
    fn codex_secrets_in_payload_fields_and_era_records_stay_in_the_file() {
        let home = temp_home("codex");
        let rollout = home
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("10")
            .join("12")
            .join("rollout.jsonl");
        write_lines(
        &rollout,
        &[
            json!({"type":"turn_context","payload":{"model":"gpt-5.1"}}).to_string(),
            json!({
                "type": "event_msg",
                "timestamp": "2026-10-12T11:00:00Z",
                "authorization": BEARER_SENTINEL,
                "payload": {
                    "type": "token_count",
                    "api_key": API_KEY_SENTINEL,
                    "info": {
                        "model": "gpt-5.1",
                        "total_token_usage": {"input_tokens": 200, "cached_input_tokens": 50, "output_tokens": 30}
                    }
                }
            })
            .to_string(),
            json!({
                "type": "token_usage_record",
                "payload": {"session_api_key": API_KEY_SENTINEL, "cookie": COOKIE_SENTINEL}
            })
            .to_string(),
            COOKIE_SENTINEL.to_owned(),
        ],
    );
        let archived = home
            .join(".codex")
            .join("archived_sessions")
            .join("old.jsonl");
        write_lines(
        &archived,
        &[
            json!({"type":"turn_context","payload":{"model":"gpt-5.1","account": ACCOUNT_ID_SENTINEL}}).to_string(),
            json!({
                "type": "event_msg",
                "timestamp": "2026-10-11T09:00:00Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5.1",
                        "last_token_usage": {"input_tokens": 60, "output_tokens": 20}
                    }
                }
            })
            .to_string(),
        ],
    );

        let roots = codex_sessions_roots(None, &home);
        let outcome = scan_codex_rollouts(&roots);

        assert_eq!(
            outcome.events.len(),
            2,
            "era records and junk must not suppress usage"
        );
        assert!(
            outcome.counts.skipped >= 2,
            "the era record and the bare-cookie line must be skips"
        );
        clean_report(&outcome, "codex abuse corpus");
    }

    #[test]
    fn pi_unmapped_provider_with_secrets_is_skipped_not_borrowed() {
        let home = temp_home("pi");
        let sessions = home.join("pi-sessions");
        write_lines(
            &sessions.join("s.jsonl"),
            &[
                json!({"type":"session","id":"s-abuse","apiKey": API_KEY_SENTINEL}).to_string(),
                json!({"type":"model_change","provider":"anthropic","modelId":"claude-haiku-4-5"})
                    .to_string(),
                json!({
                    "type": "message",
                    "bearer": BEARER_SENTINEL,
                    "message": {
                        "role": "assistant",
                        "model": "claude-haiku-4-5",
                        "cookie": COOKIE_SENTINEL,
                        "usage": {"input": 30, "output": 10},
                        "timestamp": 1_769_989_200
                    }
                })
                .to_string(),
                json!({
                    "type": "message",
                    "message": {
                        "role": "assistant",
                        "provider": API_KEY_SENTINEL,
                        "model": "claude-haiku-4-5",
                        "usage": {"input": 999, "output": 999},
                        "timestamp": 1_769_989_201
                    }
                })
                .to_string(),
                OPENCODE_CREDENTIAL_SENTINEL.to_owned(),
            ],
        );
        let session_dir = sessions.to_string_lossy().into_owned();
        let selectors = PiSelectors {
            session_dir: Some(session_dir.as_str()),
            ..PiSelectors::default()
        };
        let roots = pi_session_roots(&selectors, &home);
        assert!(roots.refusal.is_none());
        assert_no_sentinel_debug(&roots, "pi session roots");
        let outcome = scan_pi_sessions(&roots.roots);

        assert_eq!(
            outcome.events.len(),
            1,
            "the unmapped provider's tokens must not be borrowed"
        );
        assert!(
            outcome.counts.skipped >= 2,
            "the unmapped-provider row and the bare-credential line must be skips"
        );
        clean_report(&outcome, "pi abuse corpus");
    }
}
