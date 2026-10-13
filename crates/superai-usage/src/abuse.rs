//! Usage-layer secret and path abuse verification: no token, key, cookie,
//! or account identity reaches a record, log, or output.

/// API-key-shaped plant (`sk-` prefix).
pub const API_KEY_SENTINEL: &str = "sk-usage-abuse-key-3f9d21-fake";
/// Bearer-token-shaped plant.
pub const BEARER_SENTINEL: &str = "usage-abuse-bearer-8c4a07-fake";
/// Cookie-value-shaped plant.
pub const COOKIE_SENTINEL: &str = "usage-abuse-cookie-51b6ee-fake";
/// Account-identity-shaped plant.
pub const ACCOUNT_ID_SENTINEL: &str = "usage-abuse-account-9943cc-fake";
/// llmproxy `/usage` admin-token plant (USG-08, subplan 15).
pub const LLMPROXY_ADMIN_SENTINEL: &str = "usage-abuse-admin-token-e2d740-fake";
/// opencode v2 credential-store plant (USG-03).
pub const OPENCODE_CREDENTIAL_SENTINEL: &str = "usage-abuse-opencode-cred-6a1f85-fake";

/// Every planted secret, one per kind a usage path can touch. Plant these
/// in fixtures; parsed output, error text, and serialized rows must stay
/// free of all of them.
pub const SENTINELS: &[&str] = &[
    API_KEY_SENTINEL,
    BEARER_SENTINEL,
    COOKIE_SENTINEL,
    ACCOUNT_ID_SENTINEL,
    LLMPROXY_ADMIN_SENTINEL,
    OPENCODE_CREDENTIAL_SENTINEL,
];

/// Whether bytes hold any planted sentinel verbatim.
pub fn contains_any_sentinel(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    SENTINELS.iter().any(|sentinel| text.contains(sentinel))
}

/// Assert no planted sentinel is present; `ctx` names the surface, never
/// the payload.
pub fn assert_no_sentinel_bytes(bytes: &[u8], ctx: &str) {
    assert!(!contains_any_sentinel(bytes), "sentinel leaked in {ctx}");
}

/// [`assert_no_sentinel_bytes`] over a `Debug` rendering.
pub fn assert_no_sentinel_debug<T: std::fmt::Debug>(value: &T, ctx: &str) {
    assert_no_sentinel_bytes(format!("{value:?}").as_bytes(), ctx);
}

/// [`assert_no_sentinel_bytes`] over a `Display` rendering.
pub fn assert_no_sentinel_display<T: std::fmt::Display>(value: &T, ctx: &str) {
    assert_no_sentinel_bytes(format!("{value}").as_bytes(), ctx);
}

/// [`assert_no_sentinel_bytes`] over the JSON serialization; a value that
/// fails to serialize produces no JSON output to leak.
pub fn assert_no_sentinel_json<T: serde::Serialize>(value: &T, ctx: &str) {
    if let Ok(json) = serde_json::to_string(value) {
        assert_no_sentinel_bytes(json.as_bytes(), ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_planted_sentinel_is_detected_in_artifact_shaped_lines() {
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
