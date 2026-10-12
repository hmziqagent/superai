//! JSONL transcript and rollout readers (USG-03: claude-code, codex-cli, pi).

use serde::{Deserialize, Serialize};

/// Defensive-scan tally for one JSONL artifact (USG-03): unknown line
/// shapes are skipped and counted, never fatal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScanCounts {
    /// Lines parsed into records.
    pub parsed: u64,
    /// Lines skipped as unknown or malformed.
    pub skipped: u64,
}
