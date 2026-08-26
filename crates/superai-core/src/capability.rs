use serde::{Deserialize, Serialize};

/// How an instance satisfies a capability.
///
/// Support is not a boolean: it depends on the harness and the provider together.
/// Claude Code's web search is a client-side tool on Anthropic ([`Support::Native`]),
/// while the same harness on GLM gets search server-side ([`Support::Substituted`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    /// The harness implements it directly against this provider.
    Native,
    /// Not available as the harness implements it, but the provider covers it another way.
    Substituted,
    /// Unavailable on this harness/provider pair.
    Absent,
}

/// A capability the interface layer may ask an instance about.
///
/// The interface asks "can this instance search the web", never "is this harness
/// Claude Code" — harness identity does not leak upward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Search the web during a turn.
    WebSearch,
    /// Accept images as input.
    Vision,
    /// Drive a screen, keyboard, and mouse.
    ComputerUse,
    /// Connect MCP servers.
    Mcp,
}
