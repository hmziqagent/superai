//! `superai usage` integration tests (USG-06): the exit gate runs the real
//! binary against fixture HOMEs, one store per reader family, and checks
//! tokens, exact costs, truth markers, --json, and typed errors.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    #[cfg(unix)]
    use std::process::Stdio;
    use std::process::{Command, Output};
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::{Connection, params};
    use superai_usage::abuse::SENTINELS;

    // unused_crate_dependencies demands an ack per unreferenced extern.
    use serde as _;
    use superai_core as _;
    use thiserror as _;

    const CLAUDE_LINE: &str = r#"{"type":"assistant","timestamp":"2026-01-15T09:00:05Z","sessionId":"s1","requestId":"r1","message":{"id":"m1","model":"claude-sonnet-4-5","stop_reason":"end_turn","usage":{"input_tokens":100,"cache_read_input_tokens":50,"cache_creation_input_tokens":10,"output_tokens":200}}}"#;
    fn sentinel_at(index: usize) -> &'static str {
        SENTINELS
            .get(index)
            .unwrap_or_else(|| panic!("abuse SENTINELS has no entry {index}"))
    }

    fn claude_sentinel_line(sentinel: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-01-15T09:00:06Z","sessionId":"s1","message":{{"id":"m2","model":"claude-sonnet-4-5","stop_reason":"end_turn","usage":{{"input_tokens":1,"output_tokens":1}},"apiKey":"{sentinel}"}}}}"#
        )
    }
    const CODEX_LINE: &str = r#"{"timestamp":"2026-01-15T08:00:10.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5.1","last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":50},"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":50}}}}"#;
    fn codex_sentinel_line(sentinel: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-01-15T08:00:11.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"model":"gpt-5.1","total_token_usage":{{"input_tokens":100,"cached_input_tokens":40,"output_tokens":50}},"api_key":"{sentinel}"}}}}}}"#
        )
    }
    const OPENCODE_USAGE: &str = r#"{"model":{"providerID":"anthropic","id":"claude-sonnet-4-5"},"tokens":{"input":120,"output":80,"reasoning":40,"cache":{"read":500,"write":10}}}"#;
    fn pi_lines(sentinel: &str) -> [String; 4] {
        [
            r#"{"type":"session","id":"pi-s1"}"#.to_owned(),
            r#"{"type":"model_change","provider":"anthropic","modelId":"claude-sonnet-4-5"}"#.to_owned(),
            r#"{"type":"message","id":"e1","timestamp":"2026-01-15T09:30:00Z","message":{"role":"assistant","model":"claude-sonnet-4-5","timestamp":"2026-01-15T09:30:00Z","usage":{"input":20,"cacheRead":5,"cacheWrite":2,"output":40}}}"#.to_owned(),
            format!(
                r#"{{"type":"message","id":"e9","timestamp":"2026-01-15T09:31:00Z","message":{{"role":"assistant","model":"claude-sonnet-4-5","timestamp":"2026-01-15T09:31:00Z","usage":{{"input":1,"output":1}},"account":"{sentinel}"}}}}"#
            ),
        ]
    }

    fn omp_lines(sentinel: &str) -> [String; 3] {
        [
            r#"{"type":"session","id":"omp-s1"}"#.to_owned(),
            r#"{"type":"model_change","provider":"openai-codex","modelId":"gpt-5-mini"}"#
                .to_owned(),
            format!(
                r#"{{"type":"message","id":"o1","message":{{"role":"assistant","timestamp":1760000100,"usage":{{"input":12,"cacheRead":3,"output":6}},"cookie":"{sentinel}"}}}}"#
            ),
        ]
    }

    const OPENCODE_MS: i64 = 1_768_467_600_000;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_home(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-cli-usage-{tag}-{}-{n}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp home {}: {e}", dir.display()));
        dir
    }

    fn write_lines(path: &Path, lines: &[&str]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("mkdir {}: {e}", parent.display()));
        }
        let mut text = lines.join("\n");
        text.push('\n');
        fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    }

    fn full_usage_home(tag: &str) -> PathBuf {
        let home = temp_home(tag);
        let claude = claude_sentinel_line(sentinel_at(0));
        write_lines(
            &home.join(".claude/projects/proj/session.jsonl"),
            &[CLAUDE_LINE, claude.as_str()],
        );
        let codex = codex_sentinel_line(sentinel_at(1));
        write_lines(
            &home.join(".codex/sessions/2026/10/13/rollout.jsonl"),
            &[CODEX_LINE, codex.as_str()],
        );
        let pi = pi_lines(sentinel_at(2));
        let pi_refs: Vec<&str> = pi.iter().map(String::as_str).collect();
        write_lines(&home.join(".pi/agent/sessions/alpha.jsonl"), &pi_refs);
        let omp = omp_lines(sentinel_at(3));
        let omp_refs: Vec<&str> = omp.iter().map(String::as_str).collect();
        write_lines(&home.join(".omp/agent/sessions/omp.jsonl"), &omp_refs);
        opencode_db(&home);
        home
    }

    fn opencode_db(home: &Path) {
        let path = home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db");
        fs::create_dir_all(path.parent().unwrap_or_else(|| panic!("db parent")))
            .unwrap_or_else(|e| panic!("mkdir: {e}"));
        let conn = Connection::open(&path).unwrap_or_else(|e| panic!("db open: {e}"));
        conn.execute_batch(
            "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, fork_session_id TEXT, time_created INTEGER NOT NULL);
             CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);
             CREATE TABLE account (id TEXT PRIMARY KEY, access_token TEXT, refresh_token TEXT NOT NULL, time_updated INTEGER NOT NULL);",
        )
        .unwrap_or_else(|e| panic!("db schema: {e}"));
        // Every abuse sentinel lives in the credential table, so each one
        // the module plants is also one the output test asserts against.
        for (index, sentinel) in SENTINELS.iter().enumerate() {
            conn.execute(
                "INSERT INTO account VALUES (?1, ?2, ?2, 0)",
                params![format!("acc-{index}"), sentinel],
            )
            .unwrap_or_else(|e| panic!("account row {index}: {e}"));
        }
        conn.execute(
            "INSERT INTO session_v2 (id, fork_session_id, time_created) VALUES ('s1', NULL, ?1)",
            params![OPENCODE_MS],
        )
        .unwrap_or_else(|e| panic!("session row: {e}"));
        conn.execute(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES ('m1', 's1', 'assistant', 1, ?1, ?2)",
            params![OPENCODE_MS, OPENCODE_USAGE],
        )
        .unwrap_or_else(|e| panic!("message row: {e}"));
        drop(conn);
    }

    fn run(home: &Path, args: &[&str]) -> Output {
        run_with(home, args, &[])
    }

    fn run_with(home: &Path, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_superai"));
        command
            .args(args)
            .env_clear()
            .env("HOME", home)
            .env("USERPROFILE", home);
        for (name, value) in extra_env {
            command.env(name, value);
        }
        command
            .output()
            .unwrap_or_else(|e| panic!("spawn superai: {e}"))
    }

    fn stdout(output: &Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn stderr(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    struct Expected {
        harness: &'static str,
        source: &'static str,
        tokens: &'static str,
        cost: &'static str,
    }

    impl Expected {
        fn usd(&self) -> &str {
            self.cost.trim_start_matches('$')
        }
    }

    fn per_harness_expectations() -> Vec<Expected> {
        vec![
            Expected {
                harness: "claude-code",
                source: "claude-code",
                tokens: "in 101 out 201 cache-read 50 cache-write 10 events 2",
                cost: "$0.0033705",
            },
            Expected {
                harness: "codex-cli",
                source: "codex",
                tokens: "in 60 out 50 cache-read 40 cache-write 0 events 1",
                cost: "$0.00058",
            },
            Expected {
                harness: "oh-my-codex",
                source: "codex",
                tokens: "in 60 out 50 cache-read 40 cache-write 0 events 1",
                cost: "$0.00058",
            },
            Expected {
                harness: "opencode",
                source: "opencode",
                tokens: "in 120 out 120 cache-read 500 cache-write 10 events 1",
                cost: "$0.0023475",
            },
            Expected {
                harness: "oh-my-opencode",
                source: "opencode",
                tokens: "in 120 out 120 cache-read 500 cache-write 10 events 1",
                cost: "$0.0023475",
            },
            Expected {
                harness: "pi",
                source: "pi",
                tokens: "in 33 out 47 cache-read 8 cache-write 2 events 3",
                cost: "$0.000702075",
            },
            Expected {
                harness: "omp",
                source: "pi",
                tokens: "in 33 out 47 cache-read 8 cache-write 2 events 3",
                cost: "$0.000702075",
            },
        ]
    }

    #[test]
    fn show_reports_every_served_harness_from_local_artifacts() {
        let home = full_usage_home("show-all");
        for expected in per_harness_expectations() {
            let output = run(&home, &["usage", "show", "--harness", expected.harness]);
            assert!(
                output.status.success(),
                "show {} failed: {}",
                expected.harness,
                stderr(&output)
            );
            let text = stdout(&output);
            assert!(
                text.contains(expected.source),
                "source missing for {}: {text}",
                expected.harness
            );
            assert!(
                text.contains("local artifacts"),
                "truth marker missing: {text}"
            );
            assert!(
                text.contains(expected.tokens),
                "tokens wrong for {}: {text}",
                expected.harness
            );
            assert!(
                text.contains(expected.cost),
                "cost wrong for {}: {text}",
                expected.harness
            );
        }
    }

    #[test]
    fn cost_all_matches_exact_engine_totals_per_harness() {
        let home = full_usage_home("cost-all");
        for expected in per_harness_expectations() {
            let output = run(
                &home,
                &[
                    "usage",
                    "cost",
                    "--harness",
                    expected.harness,
                    "--period",
                    "all",
                    "--json",
                ],
            );
            assert!(
                output.status.success(),
                "cost {} failed: {}",
                expected.harness,
                stderr(&output)
            );
            let report: serde_json::Value =
                serde_json::from_str(&stdout(&output)).unwrap_or_else(|e| panic!("json: {e}"));
            let source = &report["sources"][0];
            assert_eq!(
                source["source"], expected.source,
                "wrong source for {}",
                expected.harness
            );
            assert_eq!(source["truth"], "local-artifacts");
            assert_eq!(source["total"]["state"], "exact");
            assert_eq!(source["total"]["usd"], expected.usd());
            assert_eq!(source["coverage"]["state"], "complete");
        }
    }

    #[test]
    fn show_json_marks_truth_tokens_window_and_coverage() {
        let home = full_usage_home("show-json");
        let output = run(
            &home,
            &["usage", "show", "--harness", "claude-code", "--json"],
        );
        assert!(output.status.success(), "{}", stderr(&output));
        let report: serde_json::Value =
            serde_json::from_str(&stdout(&output)).unwrap_or_else(|e| panic!("json: {e}"));
        let source = &report["sources"][0];
        assert_eq!(report["window"], "session");
        assert_eq!(source["harnesses"], serde_json::json!(["claude-code"]));
        assert_eq!(source["truth"], "local-artifacts");
        assert_eq!(source["totals_scope"], "all-history");
        assert_eq!(source["window"]["lane"], "provider-only");
        assert_eq!(source["window"]["resets_at"], serde_json::Value::Null);
        assert_eq!(source["tokens"]["input"], 101);
        assert_eq!(source["tokens"]["output"], 201);
        assert_eq!(source["tokens"]["cache_read"], 50);
        assert_eq!(source["tokens"]["cache_write"], 10);
        assert_eq!(source["tokens"]["events"], 2);
        assert_eq!(source["cost"]["state"], "exact");
        assert_eq!(source["cost"]["usd"], "0.0033705");
        assert_eq!(source["coverage"]["state"], "complete");
    }

    #[test]
    fn month_window_reports_next_month_start() {
        let home = full_usage_home("month-window");
        let output = run(
            &home,
            &[
                "usage",
                "show",
                "--harness",
                "claude-code",
                "--window",
                "month",
                "--json",
            ],
        );
        assert!(output.status.success(), "{}", stderr(&output));
        let report: serde_json::Value =
            serde_json::from_str(&stdout(&output)).unwrap_or_else(|e| panic!("json: {e}"));
        let resets_at = report["sources"][0]["window"]["resets_at"]
            .as_str()
            .unwrap_or_else(|| panic!("resets_at not a string: {}", report["sources"][0]));
        assert!(
            resets_at.contains("-01T00:00:00"),
            "reset is not next month's midnight: {resets_at}"
        );
    }

    #[test]
    fn cost_period_flag_and_default_echo() {
        let home = full_usage_home("period-echo");
        let defaulted = run(&home, &["usage", "cost", "--harness", "pi", "--json"]);
        assert!(defaulted.status.success(), "{}", stderr(&defaulted));
        let report: serde_json::Value =
            serde_json::from_str(&stdout(&defaulted)).unwrap_or_else(|e| panic!("json: {e}"));
        assert_eq!(report["period"]["kind"], "month-to-date");

        let rolled = run(&home, &["usage", "cost", "--harness", "pi", "--days", "7"]);
        assert!(rolled.status.success(), "{}", stderr(&rolled));
        assert!(
            stdout(&rolled).contains("last 7 days"),
            "{}",
            stdout(&rolled)
        );

        let everything = run(
            &home,
            &["usage", "cost", "--harness", "pi", "--period", "all"],
        );
        assert!(everything.status.success(), "{}", stderr(&everything));
        assert!(
            stdout(&everything).contains("all history"),
            "{}",
            stdout(&everything)
        );
    }

    #[test]
    fn cost_and_show_reject_bad_arguments() {
        let home = full_usage_home("bad-args");
        let cases: &[(&[&str], &str)] = &[
            (&["usage", "cost", "--days", "0"], "outside 1..=365"),
            (&["usage", "cost", "--days", "366"], "outside 1..=365"),
            (&["usage", "cost", "--days", "abc"], "whole number"),
            (
                &["usage", "cost", "--days", "1", "--period", "all"],
                "mutually exclusive",
            ),
            (
                &["usage", "cost", "--period", "fortnight"],
                "month-to-date or all",
            ),
            (
                &["usage", "show", "--window", "fortnight"],
                "session, week, or month",
            ),
            (&["usage", "show", "--bogus"], "unknown flag"),
            (
                &["usage", "show", "--window", "week", "--window", "month"],
                "given twice",
            ),
            (&["usage"], "needs a subcommand"),
            (&["usage", "frobnicate"], "unknown usage subcommand"),
            (&["frobnicate"], "unknown command"),
            (&["usage", "show", "--harness"], "needs a value"),
        ];
        for (args, expected) in cases {
            let output = run(&home, args);
            assert!(
                !output.status.success(),
                "`{args:?}` unexpectedly succeeded"
            );
            assert!(
                stderr(&output).contains(expected),
                "`{args:?}` stderr missing `{expected}`: {}",
                stderr(&output)
            );
        }
    }

    #[test]
    fn unknown_harness_lists_every_served_id() {
        let home = full_usage_home("unknown-harness");
        let output = run(&home, &["usage", "show", "--harness", "nope"]);
        assert!(!output.status.success());
        let text = stderr(&output);
        for id in [
            "claude-code",
            "codex-cli",
            "oh-my-codex",
            "opencode",
            "oh-my-opencode",
            "pi",
            "omp",
        ] {
            assert!(text.contains(id), "served id `{id}` missing from: {text}");
        }
    }

    #[test]
    fn missing_home_is_a_typed_error() {
        let output = Command::new(env!("CARGO_BIN_EXE_superai"))
            .args(["usage", "show"])
            .env_clear()
            .output()
            .unwrap_or_else(|e| panic!("spawn: {e}"));
        assert!(!output.status.success());
        assert!(stderr(&output).contains("HOME"), "{}", stderr(&output));
    }

    #[test]
    fn planted_sentinels_never_reach_any_output() {
        let home = full_usage_home("sentinels");
        for args in [
            vec!["usage", "show"],
            vec!["usage", "show", "--json"],
            vec!["usage", "cost", "--period", "all"],
            vec!["usage", "cost", "--period", "all", "--json"],
        ] {
            let output = run(&home, &args);
            assert!(
                output.status.success(),
                "`{args:?}` failed: {}",
                stderr(&output)
            );
            let mut all = stdout(&output);
            all.push_str(&stderr(&output));
            for sentinel in SENTINELS {
                assert!(
                    !all.contains(sentinel),
                    "sentinel `{sentinel}` leaked via `{args:?}`: {all}"
                );
            }
        }
    }

    #[test]
    fn pi_selector_refusal_is_a_note_not_a_failure() {
        let home = full_usage_home("pi-refusal");
        let output = run_with(
            &home,
            &["usage", "show", "--harness", "pi", "--json"],
            &[("PI_CONFIG_DIR", "../outside")],
        );
        assert!(output.status.success(), "{}", stderr(&output));
        let report: serde_json::Value =
            serde_json::from_str(&stdout(&output)).unwrap_or_else(|e| panic!("json: {e}"));
        let source = &report["sources"][0];
        assert!(
            source["notes"][0]
                .as_str()
                .is_some_and(|note| note.contains("PI_CONFIG_DIR"))
        );
        assert_eq!(source["tokens"]["events"], 0);
        assert_eq!(source["cost"]["state"], "zero");
    }

    #[test]
    fn opencode_database_missing_is_a_note_not_a_failure() {
        let home = temp_home("oc-missing");
        let output = run_with(
            &home,
            &["usage", "show", "--harness", "opencode"],
            &[("OPENCODE_DB", "absent.db")],
        );
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(
            stdout(&output).contains("could not be opened"),
            "{}",
            stdout(&output)
        );
    }

    #[test]
    fn unpriced_model_makes_the_cost_unavailable_not_wrong() {
        let home = temp_home("unpriced");
        write_lines(
            &home.join(".claude/projects/proj/session.jsonl"),
            &[
                r#"{"type":"assistant","timestamp":"2026-01-15T09:00:05Z","sessionId":"s1","message":{"id":"m1","model":"claude-dr-4-saas","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":10}}}"#,
            ],
        );
        let output = run(
            &home,
            &["usage", "show", "--harness", "claude-code", "--json"],
        );
        assert!(output.status.success(), "{}", stderr(&output));
        let report: serde_json::Value =
            serde_json::from_str(&stdout(&output)).unwrap_or_else(|e| panic!("json: {e}"));
        let cost = &report["sources"][0]["cost"];
        assert_eq!(cost["state"], "unavailable");
        assert_eq!(cost["missing_pricing"][0], "anthropic/claude-dr-4-saas");
    }

    #[test]
    fn unreadable_lines_make_coverage_a_floor() {
        let home = temp_home("floor");
        write_lines(
            &home.join(".claude/projects/proj/session.jsonl"),
            &[CLAUDE_LINE, "this line is {broken"],
        );
        let output = run(
            &home,
            &["usage", "show", "--harness", "claude-code", "--json"],
        );
        assert!(output.status.success(), "{}", stderr(&output));
        let report: serde_json::Value =
            serde_json::from_str(&stdout(&output)).unwrap_or_else(|e| panic!("json: {e}"));
        let coverage = &report["sources"][0]["coverage"];
        assert_eq!(coverage["state"], "incomplete");
        assert_eq!(coverage["unscanned_records"], 1);
    }

    #[test]
    fn bare_invocation_still_lists_instances() {
        let home = temp_home("bare");
        let output = run(&home, &[]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(
            stdout(&output).contains("no instances recorded"),
            "{}",
            stdout(&output)
        );
    }

    #[test]
    fn recovery_notices_stay_off_stdout_so_json_stays_pure() {
        let home = full_usage_home("journal-preamble");
        let journal_dir = home.join(".superai").join("journal");
        fs::create_dir_all(&journal_dir).unwrap_or_else(|e| panic!("journal dir: {e}"));
        let stray = journal_dir.join("stray.journal.json");
        for args in [
            vec!["usage", "show", "--harness", "claude-code", "--json"],
            vec![
                "usage",
                "cost",
                "--harness",
                "claude-code",
                "--period",
                "all",
                "--json",
            ],
        ] {
            // Recovery deletes an empty stray as "nothing to recover", so
            // each invocation plants its own.
            fs::write(&stray, b"").unwrap_or_else(|e| panic!("journal file: {e}"));
            let output = run(&home, &args);
            assert!(
                output.status.success(),
                "`{args:?}` failed: {}",
                stderr(&output)
            );
            let report: serde_json::Value = serde_json::from_str(&stdout(&output))
                .unwrap_or_else(|e| panic!("stdout was not pure json for `{args:?}`: {e}"));
            assert_eq!(report["sources"][0]["source"], "claude-code");
            assert!(
                stderr(&output).contains("recovered:"),
                "recovery notice missing from stderr: {}",
                stderr(&output)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_closed_stdout_pipe_fails_cleanly_instead_of_panicking() {
        use std::os::unix::net::UnixStream;
        let home = full_usage_home("broken-pipe");
        // The read end dies before spawn, so the child's first write meets a
        // closed peer instead of racing the pipe buffer.
        let (write_end, read_end) =
            UnixStream::pair().unwrap_or_else(|e| panic!("socketpair: {e}"));
        drop(read_end);
        let write_end: std::os::fd::OwnedFd = write_end.into();
        let mut command = Command::new(env!("CARGO_BIN_EXE_superai"));
        command
            .args(["usage", "show"])
            .env_clear()
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .stdout(Stdio::from(write_end))
            .stderr(Stdio::piped());
        let output = command
            .output()
            .unwrap_or_else(|e| panic!("run superai: {e}"));

        assert_ne!(
            output.status.code(),
            Some(101),
            "a broken pipe must not panic"
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains("failed printing to stdout"),
            "unexpected stderr: {}",
            stderr(&output)
        );
    }

    #[test]
    fn a_corrupt_opencode_database_is_a_note_not_a_fatal_report() {
        let home = full_usage_home("oc-corrupt");
        let db = home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db");
        fs::remove_file(&db).unwrap_or_else(|e| panic!("remove db: {e}"));
        let conn = Connection::open(&db).unwrap_or_else(|e| panic!("db open: {e}"));
        conn.execute_batch("CREATE TABLE unrelated (x TEXT);")
            .unwrap_or_else(|e| panic!("wrong schema: {e}"));
        drop(conn);

        let output = run(&home, &["usage", "show"]);
        assert!(
            output.status.success(),
            "a drifted database must not fail the report: {}",
            stderr(&output)
        );
        let text = stdout(&output);
        assert!(text.contains("could not be scanned"), "{text}");
        assert!(
            text.contains("claude-code"),
            "other families must still flow: {text}"
        );
    }

    #[test]
    fn window_scoping_separates_recent_events_from_old_ones() {
        let home = full_usage_home("window-scoping");
        let corpus = home
            .join(".claude")
            .join("projects")
            .join("proj")
            .join("session.jsonl");
        let fresh_at = jiff::Timestamp::now()
            .checked_sub(jiff::Span::new().hours(1))
            .unwrap_or_else(|e| panic!("clock: {e}"));
        let fresh = format!(
            r#"{{"type":"assistant","timestamp":"{fresh_at}","sessionId":"s1","requestId":"r2","message":{{"id":"m9","model":"claude-sonnet-4-5","stop_reason":"end_turn","usage":{{"input_tokens":10,"output_tokens":10}}}}}}"#
        );
        let mut text = fs::read_to_string(&corpus)
            .unwrap_or_else(|e| panic!("corpus {}: {e}", corpus.display()));
        text.push_str(&fresh);
        text.push('\n');
        fs::write(&corpus, text).unwrap_or_else(|e| panic!("corpus: {e}"));

        for (window, scope, events) in [
            ("session", "all-history", 3),
            ("week", "last-7-days", 1),
            ("month", "month-to-date", 1),
        ] {
            let output = run(
                &home,
                &[
                    "usage",
                    "show",
                    "--harness",
                    "claude-code",
                    "--window",
                    window,
                    "--json",
                ],
            );
            assert!(
                output.status.success(),
                "window {window} failed: {}",
                stderr(&output)
            );
            let report: serde_json::Value =
                serde_json::from_str(&stdout(&output)).unwrap_or_else(|e| panic!("json: {e}"));
            let source = &report["sources"][0];
            assert_eq!(source["totals_scope"], scope, "window {window}");
            assert_eq!(source["tokens"]["events"], events, "window {window}");
            assert_eq!(
                source["tokens"]["input"],
                if events == 3 { 111 } else { 10 }
            );
        }

        let month = run(
            &home,
            &[
                "usage",
                "show",
                "--harness",
                "claude-code",
                "--window",
                "month",
                "--json",
            ],
        );
        let report: serde_json::Value =
            serde_json::from_str(&stdout(&month)).unwrap_or_else(|e| panic!("json: {e}"));
        assert!(
            report["sources"][0]["window"]["resets_at"].is_string(),
            "month reset must survive scoping"
        );
    }

    #[test]
    fn every_sentinel_is_planted_in_the_fixture_home_before_output_is_asserted() {
        let home = full_usage_home("plant-audit");
        let paths = [
            home.join(".claude")
                .join("projects")
                .join("proj")
                .join("session.jsonl"),
            home.join(".codex")
                .join("sessions")
                .join("2026")
                .join("10")
                .join("13")
                .join("rollout.jsonl"),
            home.join(".pi")
                .join("agent")
                .join("sessions")
                .join("alpha.jsonl"),
            home.join(".omp")
                .join("agent")
                .join("sessions")
                .join("omp.jsonl"),
            home.join(".local")
                .join("share")
                .join("opencode")
                .join("opencode.db"),
        ];
        let mut corpus: Vec<u8> = Vec::new();
        for path in paths {
            let bytes =
                fs::read(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
            corpus.extend_from_slice(&bytes);
        }
        let text = String::from_utf8_lossy(&corpus).into_owned();
        for sentinel in SENTINELS {
            assert!(
                text.contains(sentinel),
                "sentinel `{sentinel}` was never planted; the leak assertions would pass vacuously"
            );
        }
    }

    /// One-shot loopback HTTP server standing in for the VPS ledger's
    /// admin listener; serves one canned body, then closes.
    fn ledger_server(body: &'static str, connections: usize) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap_or_else(|e| panic!("bind: {e}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("addr: {e}"));
        std::thread::spawn(move || {
            for _ in 0..connections {
                serve_ledger_page(&listener, body);
            }
        });
        format!("http://{addr}")
    }

    fn serve_ledger_page(listener: &TcpListener, body: &str) {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut head = Vec::new();
        let mut chunk = [0u8; 1024];
        while !(head.windows(4).any(|w| w == b"\r\n\r\n") || head.len() > 16_384) {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&chunk[..n]),
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        if stream
            .write_all(response.as_bytes())
            .and_then(|()| stream.flush())
            .is_err()
        {
            // The client vanished mid-page; the test fails on its own.
        }
    }

    #[test]
    fn the_vps_ledger_feeds_usage_rows_when_configured() {
        let home = full_usage_home("ledger-fed");
        let base = ledger_server(
            r#"{"rows":[
                {"received_at":"2026-01-15T09:00:00Z","provider":"anthropic","model":"claude-sonnet-4-5","input_tokens":100,"output_tokens":50,"cache_read_tokens":20,"cache_write_tokens":5,"cost_usd":"0.001"},
                {"received_at":"2026-01-15T09:05:00Z","provider":"openai","model":"gpt-5.1","input_tokens":10,"output_tokens":5,"cost_usd":"0.0005"},
                {"received_at":"2026-01-15T09:06:00Z"}
            ]}"#,
            2,
        );

        let show = run_with(
            &home,
            &["usage", "show", "--json"],
            &[
                ("SUPERAI_LLMPROXY_URL", base.as_str()),
                ("SUPERAI_LLMPROXY_ADMIN_TOKEN", "test-admin-token"),
            ],
        );
        assert!(show.status.success(), "{}", stderr(&show));
        let report: serde_json::Value =
            serde_json::from_str(&stdout(&show)).unwrap_or_else(|e| panic!("json: {e}"));
        let ledger = report["sources"]
            .as_array()
            .unwrap_or_else(|| panic!("sources array: {report}"))
            .iter()
            .find(|source| source["source"] == "vps-ledger")
            .unwrap_or_else(|| panic!("no ledger row: {report}"))
            .clone();
        assert_eq!(ledger["truth"], "llmproxy");
        assert_eq!(ledger["harnesses"], serde_json::json!([]));
        assert_eq!(ledger["window"]["lane"], "unavailable");
        assert_eq!(ledger["totals_scope"], "all-history");
        assert_eq!(ledger["tokens"]["input"], 110);
        assert_eq!(ledger["tokens"]["output"], 55);
        assert_eq!(ledger["tokens"]["cache_read"], 20);
        assert_eq!(ledger["tokens"]["cache_write"], 5);
        assert_eq!(ledger["tokens"]["events"], 2);
        assert_eq!(ledger["coverage"]["state"], "incomplete");
        assert_eq!(ledger["coverage"]["unscanned_records"], 1);
        // The unattributed row poisons the aggregate, never silently drops.
        assert_eq!(ledger["cost"]["state"], "unavailable");
        assert_eq!(
            ledger["cost"]["missing_pricing"][0],
            "ledger/unattributed-row"
        );
        assert!(
            stdout(&show).contains("claude-code"),
            "local families must still flow: {}",
            stdout(&show)
        );

        let cost = run_with(
            &home,
            &["usage", "cost", "--period", "all", "--json"],
            &[
                ("SUPERAI_LLMPROXY_URL", base.as_str()),
                ("SUPERAI_LLMPROXY_ADMIN_TOKEN", "test-admin-token"),
            ],
        );
        assert!(cost.status.success(), "{}", stderr(&cost));
        let report: serde_json::Value =
            serde_json::from_str(&stdout(&cost)).unwrap_or_else(|e| panic!("json: {e}"));
        let ledger = report["sources"]
            .as_array()
            .unwrap_or_else(|| panic!("sources array: {report}"))
            .iter()
            .find(|source| source["source"] == "vps-ledger")
            .unwrap_or_else(|| panic!("no ledger row: {report}"))
            .clone();
        assert_eq!(ledger["total"]["state"], "unavailable");
        assert_eq!(
            ledger["total"]["missing_pricing"][0],
            "ledger/unattributed-row"
        );
        let models = ledger["models"]
            .as_array()
            .unwrap_or_else(|| panic!("models"));
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["provider"], "anthropic");
        assert_eq!(models[0]["cost"], "0.001");
        assert_eq!(models[1]["provider"], "openai");
        assert_eq!(models[1]["cost"], "0.0005");
    }

    #[test]
    fn without_a_ledger_url_the_default_report_carries_an_unconfigured_note() {
        let home = full_usage_home("ledger-off");
        let output = run(&home, &["usage", "show"]);
        assert!(output.status.success(), "{}", stderr(&output));
        let text = stdout(&output);
        assert!(text.contains("vps-ledger"), "{text}");
        assert!(text.contains("SUPERAI_LLMPROXY_URL"), "{text}");
        assert!(text.contains("llmproxy ledger"), "{text}");
    }
}
