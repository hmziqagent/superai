//! Local usage-artifact readers (USG-03): transcripts, rollouts, local
//! databases. Reading never writes; no credentials leave their file.

/// JSONL transcript and rollout scanning (claude-code, codex-cli, pi).
pub mod jsonl;
/// Read-only scan of opencode's local v2 history database.
pub mod opencode;
