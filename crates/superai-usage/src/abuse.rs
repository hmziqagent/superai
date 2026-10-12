//! Usage-layer secret and path abuse verification: no token, key, cookie,
//! or account identity reaches a record, log, or output.

/// One sentinel per secret kind the usage layer touches (subplan 14's exit
/// gate). Plant these in fixtures; reader and poller output must scan clean.
pub const SENTINELS: &[&str] = &[
    "usage-test-token-4f2a9c-fake",
    "usage-test-key-7b31d0-fake",
    "usage-test-cookie-58e0ab-fake",
    "usage-test-account-93c6f1-fake",
];

/// Whether bytes hold any usage sentinel verbatim.
pub fn contains_any_sentinel(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    SENTINELS.iter().any(|sentinel| text.contains(sentinel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planted_sentinels_are_detected_in_artifact_shaped_lines() {
        for sentinel in SENTINELS {
            let line = format!("{{\"model\":\"m\",\"auth\":\"{sentinel}\"}}\n");
            assert!(
                contains_any_sentinel(line.as_bytes()),
                "missed sentinel {sentinel}"
            );
        }
    }

    #[test]
    fn clean_artifact_lines_scan_clean() {
        let text = "{\"cost_usd\":0.42,\"input_tokens\":120}\n";
        assert!(!contains_any_sentinel(text.as_bytes()));
    }
}
