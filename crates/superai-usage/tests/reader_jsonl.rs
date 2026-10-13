//! JSONL reader tests (USG-03): fixture corpora with malformed lines, era
//! drift, duplicate session ids, and property scans that never panic and
//! always account for every line.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use jiff::Timestamp;
    use rusqlite as _;
    use serde as _;
    use serde_json::json;
    use superai_core as _;
    use superai_usage::abuse::contains_any_sentinel;
    use superai_usage::cost::{CostEngine, CostPeriod, RateTable, RecordedUsage, Spend, Usd};
    use superai_usage::reader::ScanOutcome;
    use superai_usage::reader::jsonl::{
        PiSelectors, pi_session_roots, scan_claude_transcripts, scan_codex_rollouts,
        scan_pi_sessions,
    };
    use thiserror as _;
    use ureq as _;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-reader-jsonl-{tag}-{}-{n}-{nanos}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
        dir
    }

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    fn ts(text: &str) -> Timestamp {
        text.parse::<Timestamp>()
            .unwrap_or_else(|e| panic!("timestamp `{text}`: {e}"))
    }

    fn event_summary(outcome: &ScanOutcome) -> Vec<(&str, &str, u64, u64, u64, u64)> {
        outcome
            .events
            .iter()
            .map(|event| {
                (
                    event.provider.as_str(),
                    event.model.as_str(),
                    event.input_tokens,
                    event.output_tokens,
                    event.cache_read_tokens.unwrap_or(0),
                    event.cache_write_tokens.unwrap_or(0),
                )
            })
            .collect()
    }

    #[test]
    fn claude_corpus_yields_events_and_counts_every_unreadable_line() {
        let outcome = scan_claude_transcripts(&[fixtures().join("claude/projects")]);
        assert_eq!(outcome.counts.parsed, 9);
        assert_eq!(outcome.counts.skipped, 7);
        assert_eq!(outcome.unreadable_artifacts, 0);
        assert_eq!(
            event_summary(&outcome),
            vec![
                ("anthropic", "claude-sonnet-4-5", 100, 200, 50, 10),
                ("anthropic", "claude-sonnet-4-5", 7, 3, 0, 0),
                ("anthropic", "claude-sonnet-4-5", 33, 0, 0, 0),
                ("anthropic", "claude-sonnet-4-5", 1, 1, 0, 0),
                ("anthropic", "claude-haiku-4-5", 10, 20, 0, 0),
            ]
        );
        assert_eq!(outcome.events[0].timestamp, ts("2026-10-13T09:00:05.123Z"));
        let recorded = outcome.into_recorded_usage();
        assert_eq!(recorded.unscanned_records, 7);
        assert_eq!(recorded.events.len(), 5);
    }

    #[test]
    fn claude_streaming_chunks_and_copied_transcripts_count_once() {
        let outcome = scan_claude_transcripts(&[fixtures().join("claude/projects")]);
        let msg_01: Vec<_> = outcome
            .events
            .iter()
            .filter(|event| event.input_tokens == 100 && event.output_tokens == 200)
            .collect();
        assert_eq!(
            msg_01.len(),
            1,
            "the copied transcript and the smaller chunk must not add rows"
        );
    }

    fn claude_estimate(input: u64) -> String {
        json!({
            "type": "assistant",
            "timestamp": "2026-10-13T09:30:00Z",
            "sessionId": "sess-merge",
            "requestId": "req-merge",
            "message": {
                "id": "msg-merge",
                "model": "claude-sonnet-4-5",
                "stop_reason": null,
                "usage": {"input_tokens": input, "output_tokens": 0},
            },
        })
        .to_string()
    }

    fn claude_complete(input: u64, output: u64) -> String {
        json!({
            "type": "assistant",
            "timestamp": "2026-10-13T09:30:01Z",
            "sessionId": "sess-merge",
            "requestId": "req-merge",
            "message": {
                "id": "msg-merge",
                "model": "claude-sonnet-4-5",
                "stop_reason": "end_turn",
                "usage": {"input_tokens": input, "output_tokens": output},
            },
        })
        .to_string()
    }

    #[test]
    fn a_complete_row_displaces_a_larger_estimate_under_the_same_key() {
        let corpus = write_corpus(
            "claude-complete-wins",
            &[
                claude_estimate(500).into_bytes(),
                claude_complete(10, 5).into_bytes(),
            ],
        );
        let root = corpus.parent().unwrap_or_else(|| Path::new("."));
        let outcome = scan_claude_transcripts(&[root.to_path_buf()]);
        assert_eq!(
            event_summary(&outcome),
            vec![("anthropic", "claude-sonnet-4-5", 10, 5, 0, 0)],
            "completeness must outrank the estimate's larger total"
        );
        assert_eq!(
            outcome.counts.skipped, 1,
            "the estimate stays a flagged gap"
        );
    }

    #[test]
    fn a_larger_estimate_never_displaces_a_complete_row() {
        let corpus = write_corpus(
            "claude-estimate-loses",
            &[
                claude_complete(10, 5).into_bytes(),
                claude_estimate(500).into_bytes(),
            ],
        );
        let root = corpus.parent().unwrap_or_else(|| Path::new("."));
        let outcome = scan_claude_transcripts(&[root.to_path_buf()]);
        assert_eq!(
            event_summary(&outcome),
            vec![("anthropic", "claude-sonnet-4-5", 10, 5, 0, 0)]
        );
        assert_eq!(outcome.counts.skipped, 1);
    }

    #[test]
    fn claude_events_price_exactly_through_the_cost_engine() {
        let outcome = scan_claude_transcripts(&[fixtures().join("claude/projects")]);
        let engine = CostEngine::new(
            RateTable::bundled().unwrap_or_else(|e| panic!("bundled rates: {e:?}")),
        );
        let now = ts("2026-10-13T12:00:00Z");
        let report = engine
            .total(
                &CostPeriod::AllHistory,
                now,
                "UTC",
                &outcome.into_recorded_usage(),
            )
            .unwrap_or_else(|e| panic!("total: {e:?}"));
        let expected = Usd::parse("0.0036455").unwrap_or_else(|e| panic!("expected sum: {e:?}"));
        assert_eq!(report.total.spend, Spend::Exact(expected));
    }

    #[test]
    fn codex_rollouts_emit_cumulative_deltas_and_count_new_era_records() {
        let outcome = scan_codex_rollouts(&[
            fixtures().join("codex/sessions"),
            fixtures().join("codex/archived_sessions"),
        ]);
        assert_eq!(outcome.counts.parsed, 12);
        assert_eq!(outcome.counts.skipped, 4);
        assert_eq!(outcome.unreadable_artifacts, 0);
        assert_eq!(
            event_summary(&outcome),
            vec![
                ("openai", "gpt-5-mini", 6, 4, 2, 0),
                ("openai", "gpt-5.1", 60, 50, 40, 0),
                ("openai", "gpt-5.1", 50, 20, 0, 0),
                ("openai", "gpt-5.1", 5, 5, 5, 0),
                ("openai", "gpt-5.1", 10, 8, 2, 0),
            ]
        );
        assert_eq!(outcome.events[1].timestamp, ts("2026-10-13T08:00:10Z"));
    }

    #[test]
    fn codex_events_price_exactly_through_the_cost_engine() {
        let outcome = scan_codex_rollouts(&[
            fixtures().join("codex/sessions"),
            fixtures().join("codex/archived_sessions"),
        ]);
        let engine = CostEngine::new(
            RateTable::bundled().unwrap_or_else(|e| panic!("bundled rates: {e:?}")),
        );
        let report = engine
            .total(
                &CostPeriod::AllHistory,
                ts("2026-10-13T12:00:00Z"),
                "UTC",
                &outcome.into_recorded_usage(),
            )
            .unwrap_or_else(|e| panic!("total: {e:?}"));
        let expected = Usd::parse("0.001001675").unwrap_or_else(|e| panic!("expected sum: {e:?}"));
        assert_eq!(report.total.spend, Spend::Exact(expected));
    }

    #[test]
    fn codex_ledger_records_count_once_against_their_token_count_mirrors() {
        let corpus = write_corpus(
            "codex-ledger",
            &[
                json!({"timestamp":"2026-10-13T08:00:00Z","type":"turn_context","payload":{"model":"gpt-5.1"}})
                    .to_string()
                    .into_bytes(),
                json!({
                    "timestamp": "2026-10-13T08:00:10Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 40, "output_tokens": 50}},
                    },
                })
                .to_string()
                .into_bytes(),
                json!({
                    "timestamp": "2026-10-13T08:00:11Z",
                    "type": "token_usage_record",
                    "payload": {
                        "thread_id": "th-1",
                        "response_id": "resp-1",
                        "usage": {"input_tokens": 100, "cached_input_tokens": 40, "output_tokens": 50},
                        "thread_token_usage": {"input_tokens": 100, "cached_input_tokens": 40, "output_tokens": 50},
                    },
                })
                .to_string()
                .into_bytes(),
                json!({
                    "timestamp": "2026-10-13T08:00:12Z",
                    "type": "token_usage_record",
                    "payload": {
                        "thread_id": "th-1",
                        "response_id": "resp-2",
                        "usage": {"input_tokens": 50, "cached_input_tokens": 5, "output_tokens": 20},
                        "thread_token_usage": {"input_tokens": 150, "cached_input_tokens": 45, "output_tokens": 70},
                    },
                })
                .to_string()
                .into_bytes(),
                json!({"timestamp":"2026-10-13T08:00:13Z","type":"token_usage_record","payload":{"input_tokens":9,"output_tokens":9}})
                    .to_string()
                    .into_bytes(),
            ],
        );
        let root = corpus.parent().unwrap_or_else(|| Path::new("."));
        let outcome = scan_codex_rollouts(&[root.to_path_buf()]);
        assert_eq!(
            event_summary(&outcome),
            vec![
                ("openai", "gpt-5.1", 60, 50, 40, 0),
                ("openai", "gpt-5.1", 45, 20, 5, 0),
            ],
            "the mirror must add nothing; only the advancing record emits"
        );
        assert_eq!(outcome.counts.parsed, 4);
        assert_eq!(
            outcome.counts.skipped, 1,
            "a record without thread_token_usage is a flagged gap"
        );
    }

    fn default_pi_roots() -> Vec<PathBuf> {
        let selectors = PiSelectors {
            xdg_data_home: Some(&fixtures().join("pi/xdg")),
            ..PiSelectors::default()
        };
        pi_session_roots(&selectors, &fixtures().join("pi/home")).roots
    }

    #[test]
    fn pi_sessions_estimate_costs_from_recorded_usage_only() {
        let outcome = scan_pi_sessions(&default_pi_roots());
        assert_eq!(outcome.counts.parsed, 14);
        assert_eq!(outcome.counts.skipped, 3);
        assert_eq!(outcome.unreadable_artifacts, 0);
        assert_eq!(
            event_summary(&outcome),
            vec![
                ("openai", "gpt-5-mini", 14, 6, 3, 0),
                ("anthropic", "claude-opus-4-5", 3, 5, 1, 0),
                ("anthropic", "claude-sonnet-4-5", 20, 40, 5, 2),
                ("anthropic", "claude-sonnet-4-5", 10, 6, 4, 1),
            ]
        );
        assert_eq!(
            outcome.events[3].timestamp,
            Timestamp::from_second(1_760_000_000)
                .unwrap_or_else(|e| panic!("epoch seconds: {e:?}"))
        );
    }

    #[test]
    fn pi_named_profile_limits_the_scan_to_that_profile() {
        let selectors = PiSelectors {
            omp_profile: Some("work"),
            ..PiSelectors::default()
        };
        let roots = pi_session_roots(&selectors, &fixtures().join("pi/home")).roots;
        let outcome = scan_pi_sessions(&roots);
        assert_eq!(
            event_summary(&outcome),
            vec![
                ("anthropic", "claude-opus-4-5", 3, 5, 1, 0),
                ("anthropic", "claude-sonnet-4-5", 20, 40, 5, 2),
                ("anthropic", "claude-sonnet-4-5", 10, 6, 4, 1),
            ]
        );
    }

    #[test]
    fn pi_events_price_exactly_through_the_cost_engine() {
        let outcome = scan_pi_sessions(&default_pi_roots());
        let engine = CostEngine::new(
            RateTable::bundled().unwrap_or_else(|e| panic!("bundled rates: {e:?}")),
        );
        let report = engine
            .total(
                &CostPeriod::AllHistory,
                ts("2026-10-13T12:00:00Z"),
                "UTC",
                &outcome.into_recorded_usage(),
            )
            .unwrap_or_else(|e| panic!("total: {e:?}"));
        let expected = Usd::parse("0.000950025").unwrap_or_else(|e| panic!("expected sum: {e:?}"));
        assert_eq!(report.total.spend, Spend::Exact(expected));
    }

    #[test]
    fn scanning_never_writes_a_byte_into_the_artifact_tree() {
        let fixture_root = fixtures();
        let mut before = BTreeMap::new();
        collect_tree(&fixture_root, &mut before);
        scan_claude_transcripts(&[fixtures().join("claude/projects")]);
        scan_codex_rollouts(&[
            fixtures().join("codex/sessions"),
            fixtures().join("codex/archived_sessions"),
        ]);
        scan_pi_sessions(&default_pi_roots());
        let mut after = BTreeMap::new();
        collect_tree(&fixture_root, &mut after);
        assert_eq!(before, after, "a reader modified the fixture tree");
    }

    fn collect_tree(root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(root)
            .unwrap_or_else(|e| panic!("read dir {}: {e}", root.display()))
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                collect_tree(&path, out);
            } else {
                let bytes =
                    fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
                out.insert(path, bytes);
            }
        }
    }

    #[test]
    fn planted_credentials_never_reach_scan_output() {
        let outcome = scan_claude_transcripts(&[fixtures().join("claude/projects")]);
        let recorded: RecordedUsage = outcome.into_recorded_usage();
        let debug = format!("{recorded:?}");
        let serialized = serde_json::to_string(&recorded)
            .unwrap_or_else(|e| panic!("serialize recorded usage: {e}"));
        assert!(!contains_any_sentinel(debug.as_bytes()));
        assert!(!contains_any_sentinel(serialized.as_bytes()));
    }

    struct XorShift64(u64);

    impl XorShift64 {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }
    }

    fn write_corpus(tag: &str, lines: &[Vec<u8>]) -> PathBuf {
        let dir = temp_dir(tag);
        let path = dir.join("corpus.jsonl");
        let mut bytes = Vec::new();
        for line in lines {
            bytes.extend_from_slice(line);
            bytes.push(b'\n');
        }
        fs::write(&path, bytes).unwrap_or_else(|e| panic!("corpus: {e}"));
        path
    }

    fn random_bytes(rng: &mut XorShift64, len: usize) -> Vec<u8> {
        (0..len).map(|_| (rng.next() & 0xff) as u8).collect()
    }

    fn claude_generated_line(rng: &mut XorShift64) -> Vec<u8> {
        match rng.below(10) {
            0 | 1 => json!({
                "type": "assistant",
                "timestamp": "2026-10-13T09:00:00Z",
                "sessionId": "s-gen",
                "requestId": format!("r-{}", rng.below(1000)),
                "message": {
                    "id": format!("m-{}", rng.below(1000)),
                    "model": "claude-sonnet-4-5",
                    "stop_reason": "end_turn",
                    "usage": {
                        "input_tokens": rng.below(5000),
                        "cache_read_input_tokens": rng.below(500),
                        "cache_creation_input_tokens": rng.below(500),
                        "output_tokens": rng.below(5000),
                    },
                },
            })
            .to_string()
            .into_bytes(),
            2 => json!({"type": "user", "message": {"role": "user", "content": "hi"}})
                .to_string()
                .into_bytes(),
            3 => random_bytes(rng, 40),
            4 => b"{\"foo\":1}".to_vec(),
            5 => json!({"type": "assistant", "message": {"model": "claude-sonnet-4-5"}})
                .to_string()
                .into_bytes(),
            6 => format!("{{\"type\":\"assistant\",\"x\":\"{}\"", "[".repeat(300)).into_bytes(),
            7 => json!({"type": format!("unknown_{}", rng.below(100)), "n": rng.next()})
                .to_string()
                .into_bytes(),
            8 => json!({
                "type": "assistant",
                "timestamp": "2026-10-13T09:00:00Z",
                "message": {
                    "model": "claude-haiku-4-5",
                    "stop_reason": "end_turn",
                    "usage": {"input_tokens": rng.below(100), "output_tokens": rng.below(100)},
                },
            })
            .to_string()
            .into_bytes(),
            _ => vec![0xff, 0xfe, b'{', b'"'],
        }
    }

    fn codex_generated_line(rng: &mut XorShift64) -> Vec<u8> {
        match rng.below(9) {
            0 => json!({
                "timestamp": "2026-10-13T08:00:00Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5.1",
                        "total_token_usage": {
                            "input_tokens": rng.below(9000),
                            "cached_input_tokens": rng.below(900),
                            "output_tokens": rng.below(9000),
                        },
                    },
                },
            })
            .to_string()
            .into_bytes(),
            1 => json!({"timestamp": "2026-10-13T08:00:00Z", "type": "session_meta", "payload": {"id": "x"}})
                .to_string()
                .into_bytes(),
            2 => random_bytes(rng, 30),
            3 => json!({"timestamp": "2026-10-13T08:00:00Z", "type": "token_usage_record", "payload": {"input_tokens": 1}})
                .to_string()
                .into_bytes(),
            4 => json!({"timestamp": "2026-10-13T08:00:00Z", "type": "event_msg", "payload": {"type": "token_count"}})
                .to_string()
                .into_bytes(),
            5 => json!({"type": "event_msg", "payload": {"type": "agent_reasoning"}})
                .to_string()
                .into_bytes(),
            6 => vec![b'{', b'"', 0xc3, 0x28],
            7 => json!({
                "timestamp": "2026-10-13T08:00:00Z",
                "type": "token_usage_record",
                "payload": {
                    "thread_id": "th-gen",
                    "response_id": format!("r-{}", rng.below(50)),
                    "model": "gpt-5.1",
                    "usage": {"input_tokens": rng.below(900), "cached_input_tokens": rng.below(90), "output_tokens": rng.below(900)},
                    "thread_token_usage": {"input_tokens": rng.below(9000), "cached_input_tokens": rng.below(900), "output_tokens": rng.below(9000)},
                },
            })
            .to_string()
            .into_bytes(),
            _ => json!({"timestamp": "not-a-time", "type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 5, "output_tokens": 5}}}})
                .to_string()
                .into_bytes(),
        }
    }

    fn pi_generated_line(rng: &mut XorShift64) -> Vec<u8> {
        match rng.below(8) {
            0 => json!({
                "type": "session",
                "id": format!("pi-{}", rng.below(4)),
            })
            .to_string()
            .into_bytes(),
            1 => json!({"type": "model_change", "provider": "anthropic", "modelId": "claude-sonnet-4-5"})
                .to_string()
                .into_bytes(),
            2 => json!({
                "type": "message",
                "id": format!("e-{}", rng.below(2000)),
                "message": {
                    "role": "assistant",
                    "timestamp": 1_760_000_000 + rng.below(1000),
                    "usage": {
                        "input": rng.below(4000),
                        "cacheRead": rng.below(400),
                        "cacheWrite": rng.below(400),
                        "output": rng.below(4000),
                        "cacheWrite1h": 0,
                    },
                },
            })
            .to_string()
            .into_bytes(),
            3 => json!({"type": "message", "message": {"role": "user", "content": "hi"}})
                .to_string()
                .into_bytes(),
            4 => random_bytes(rng, 25),
            5 => json!({"type": "message", "id": "x", "message": {"role": "assistant", "timestamp": 5, "usage": {}}})
                .to_string()
                .into_bytes(),
            6 => json!({"type": "model_change", "provider": "google-gemini", "modelId": "gemini"})
                .to_string()
                .into_bytes(),
            _ => vec![0xed, 0xa0, 0x80, b'}'],
        }
    }

    fn assert_property_holds(
        tag: &str,
        seed: u64,
        generator: fn(&mut XorShift64) -> Vec<u8>,
        scan: fn(&[PathBuf]) -> ScanOutcome,
    ) {
        let mut rng = XorShift64(seed | 1);
        let lines: Vec<Vec<u8>> = (0..400).map(|_| generator(&mut rng)).collect();
        let path = write_corpus(tag, &lines);
        let corpus = fs::read(&path).unwrap_or_else(|e| panic!("read corpus: {e}"));
        let physical = corpus
            .split(|byte| *byte == b'\n')
            .filter(|line| {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                !line.is_empty()
            })
            .count() as u64;
        let root_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let outcome = scan(&[root_dir.to_path_buf()]);
        assert_eq!(
            outcome.counts.parsed + outcome.counts.skipped,
            physical,
            "every generated line must be accounted for (seed {seed})"
        );
        assert_eq!(outcome.unreadable_artifacts, 0);
        let again = scan(&[root_dir.to_path_buf()]);
        assert_eq!(outcome, again, "scans must be deterministic (seed {seed})");
    }

    #[test]
    fn generated_claude_corpora_never_panic_and_account_for_every_line() {
        for seed in [7u64, 42, 991, 0xbeef] {
            assert_property_holds(
                "claude-prop",
                seed,
                claude_generated_line,
                scan_claude_transcripts,
            );
        }
    }

    #[test]
    fn generated_codex_corpora_never_panic_and_account_for_every_line() {
        for seed in [3u64, 88, 1234, 0xfeed] {
            assert_property_holds(
                "codex-prop",
                seed,
                codex_generated_line,
                scan_codex_rollouts,
            );
        }
    }

    #[test]
    fn generated_pi_corpora_never_panic_and_account_for_every_line() {
        for seed in [5u64, 61, 777, 0xcafe] {
            assert_property_holds("pi-prop", seed, pi_generated_line, scan_pi_sessions);
        }
    }

    #[test]
    fn unreadable_token_counters_skip_the_record_instead_of_zeroing_it() {
        let dir = temp_dir("strict-counters");
        let claude_dir = dir.join("claude");
        fs::create_dir_all(&claude_dir).unwrap_or_else(|e| panic!("claude dir: {e}"));
        fs::write(
            claude_dir.join("a.jsonl"),
            br#"{"type":"assistant","timestamp":"2026-10-13T09:00:00Z","message":{"id":"m1","model":"claude-sonnet-4-5","usage":{"input_tokens":"100","output_tokens":"50"}}}"#,
        )
        .unwrap_or_else(|e| panic!("claude corpus: {e}"));
        let claude = scan_claude_transcripts(&[claude_dir]);
        assert_eq!(claude.counts.parsed, 0);
        assert_eq!(claude.counts.skipped, 1);
        assert_eq!(claude.events.len(), 0);

        let codex_dir = dir.join("codex");
        fs::create_dir_all(&codex_dir).unwrap_or_else(|e| panic!("codex dir: {e}"));
        fs::write(
            codex_dir.join("r.jsonl"),
            br#"{"timestamp":"2026-10-13T08:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5.1","total_token_usage":{"input_tokens":100.5,"cached_input_tokens":40,"output_tokens":10}}}}"#,
        )
        .unwrap_or_else(|e| panic!("codex corpus: {e}"));
        let codex = scan_codex_rollouts(&[codex_dir]);
        assert_eq!(codex.counts.parsed, 0);
        assert_eq!(codex.counts.skipped, 1);
        assert_eq!(
            codex.events.len(),
            0,
            "an unreadable counter must not yield a zero-token event"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_fifo_is_ignored_instead_of_wedging_the_scan() {
        let dir = temp_dir("fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(dir.join("wedge.jsonl"))
            .status()
            .unwrap_or_else(|e| panic!("spawn mkfifo: {e}"));
        assert!(status.success(), "mkfifo(1) must exist to run this test");
        let real = dir.join("real.jsonl");
        fs::write(
            &real,
            br#"{"type":"assistant","timestamp":"2026-10-13T09:00:00Z","message":{"model":"claude-sonnet-4-5","usage":{"input_tokens":3,"output_tokens":4}}}"#,
        )
        .unwrap_or_else(|e| panic!("write real: {e}"));
        let outcome = scan_claude_transcripts(std::slice::from_ref(&dir));
        assert_eq!(outcome.counts.parsed, 1);
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(outcome.unreadable_artifacts, 0);
    }

    #[test]
    fn line_caps_differ_per_surface() {
        let pad = "a".repeat(600 * 1024);
        let big_line = json!({"type": "session", "id": "big", "pad": pad}).to_string();
        let dir = write_corpus("caps", &[big_line.into_bytes()])
            .parent()
            .unwrap()
            .to_path_buf();
        let claude = scan_claude_transcripts(std::slice::from_ref(&dir));
        assert_eq!(claude.counts.skipped, 1, "claude caps at 512 KiB");
        let pi = scan_pi_sessions(std::slice::from_ref(&dir));
        assert_eq!(pi.counts.skipped, 0, "pi accepts records up to 16 MiB");
        assert_eq!(pi.counts.parsed, 1);
    }

    #[test]
    fn an_oversized_record_is_counted_and_the_scan_continues() {
        let mut lines: Vec<Vec<u8>> = Vec::new();
        lines.push(
            json!({
                "type": "assistant",
                "timestamp": "2026-10-13T09:00:00Z",
                "message": {"model": "claude-sonnet-4-5", "usage": {"input_tokens": 3, "output_tokens": 4}},
            })
            .to_string()
            .into_bytes(),
        );
        lines.push(format!("{{\"pad\":\"{}\"}}", "a".repeat(600 * 1024)).into_bytes());
        lines.push(
            json!({"type": "user", "message": {"role": "user"}})
                .to_string()
                .into_bytes(),
        );
        let dir = write_corpus("oversized", &lines)
            .parent()
            .unwrap()
            .to_path_buf();
        let outcome = scan_claude_transcripts(&[dir]);
        assert_eq!(outcome.counts.skipped, 1);
        assert_eq!(outcome.counts.parsed, 2);
        assert_eq!(outcome.events.len(), 1);
    }
}
