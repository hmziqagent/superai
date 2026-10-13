//! Punted-items ledger: known leaks and unverified wire schemas, recorded as
//! `#[ignore]`d repros so the loop can read and act on them. Each test
//! asserts the clean state its owning slice owes; run the repro with
//! `cargo test -p superai-usage --test abuse_punted_leaks -- --ignored`
//! and remove the `#[ignore]` once the owner lands the fix.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use jiff as _;
    use rusqlite as _;
    use serde as _;
    use serde_json::json;
    use superai_core as _;
    use superai_usage::abuse::{
        API_KEY_SENTINEL, assert_no_sentinel_debug, assert_no_sentinel_display,
    };
    use superai_usage::reader::opencode::scan_history;
    use superai_usage::registry::UsageRegistry;
    use thiserror as _;
    use ureq as _;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "superai-abuse-punted-{tag}-{}-{n}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
        dir
    }

    #[ignore = "PUNTED, owning slice: the registry (USG-07, src/registry.rs). RegistryError::Parse forwards serde_json's message, which echoes the offending artifact VALUE; bound or redact the reason the way bound_reason does in llmproxy.rs"]
    #[test]
    fn registry_parse_errors_must_not_echo_artifact_values() {
        let dir = temp_dir("registry-echo");
        fs::write(
            dir.join("bad.json"),
            json!({
                "id": "echo-probe",
                "label": "Echo Probe",
                "locators": [],
                "auth_material": [],
                "window_strategy": "cost_only",
                "pricing": "not_priced",
                "schema_version": API_KEY_SENTINEL
            })
            .to_string(),
        )
        .unwrap_or_else(|e| panic!("write: {e}"));

        let error = UsageRegistry::load_dir(&dir).unwrap_err();
        assert_no_sentinel_display(&error, "registry parse error display");
        assert_no_sentinel_debug(&error, "registry parse error debug");
    }

    #[ignore = "PUNTED, owning slices: opencode reader (src/reader/opencode.rs), registry (src/registry.rs), llmproxy (src/llmproxy.rs), pollers (src/poller.rs). Their error Displays embed caller-supplied PATHS (rusqlite's open error carries the db path; RegistryError::TokenFile-style variants echo path.display()). Path echo is the crate's standing convention (CostError::CacheIo), so owners must decide on redaction"]
    #[test]
    fn error_displays_must_not_echo_caller_supplied_paths() {
        let missing = temp_dir("path-echo").join(format!("missing-{API_KEY_SENTINEL}.db"));

        let error = scan_history(&missing).unwrap_err();
        assert_no_sentinel_display(&error, "opencode open error display");
        assert_no_sentinel_debug(&error, "opencode open error debug");
    }

    #[ignore = "PUNTED, owning slice: the llmproxy client (USG-08, src/llmproxy.rs). PRX-03 is not deployed, so the /usage shapes in tests/fixtures/llmproxy/ are assumptions from the module doc, never checked against live endpoints. When PRX-03 ships: verify every fixture against the deployed endpoints, record one verified-date line per fixture in tests/fixtures/llmproxy/VERIFICATION.md, then drop this ignore"]
    #[test]
    fn llmproxy_fixture_shapes_must_carry_a_verification_record() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/llmproxy");
        let record = fs::read_to_string(dir.join("VERIFICATION.md")).unwrap_or_else(|e| {
            panic!("no verification record for the assumed /usage shapes: {e}")
        });
        let fixtures = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read fixture dir: {e}"))
            .map(|entry| {
                entry
                    .unwrap_or_else(|e| panic!("read fixture dir entry: {e}"))
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| Path::new(name).extension().is_some_and(|ext| ext == "json"))
            .collect::<Vec<_>>();
        assert!(
            !fixtures.is_empty(),
            "fixture dir emptied; this ledger entry is stale"
        );
        for fixture in fixtures {
            let entry = record
                .lines()
                .find(|line| line.contains(fixture.as_str()))
                .unwrap_or_else(|| panic!("no verification entry for {fixture}"));
            assert!(
                entry.contains("verified"),
                "entry for {fixture} carries no verification date: {entry}"
            );
        }
    }
}
