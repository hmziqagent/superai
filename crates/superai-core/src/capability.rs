//! Capability model and catalog (plan 09 CAP-01).
//!
//! Typed known capability IDs give ergonomic Rust use; the catalog carries
//! each capability's stable meaning, aliases, and validation notes. Schema
//! validation rejects unknown capability IDs cleanly — unknown is never
//! silently treated as absent.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

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

impl std::fmt::Display for Support {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Native => "native",
            Self::Substituted => "substituted",
            Self::Absent => "absent",
        };
        f.write_str(s)
    }
}

/// A capability the interface layer may ask an instance about.
///
/// The interface asks "can this instance search the web", never "is this harness
/// Claude Code" — harness identity does not leak upward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Search the web during a turn.
    #[serde(alias = "web-search", alias = "websearch")]
    WebSearch,
    /// Accept images as input.
    #[serde(alias = "image-input")]
    Vision,
    /// Drive a screen, keyboard, and mouse.
    #[serde(alias = "computer-use")]
    ComputerUse,
    /// Connect MCP servers.
    #[serde(alias = "mcp-servers", alias = "mcp_servers")]
    Mcp,
}

/// Catalog entry for one capability (CAP-01: stable meaning, aliases,
/// deprecations, validation rules).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityCatalogEntry {
    /// The typed capability id.
    pub id: Capability,
    /// Stable meaning of the capability.
    pub meaning: &'static str,
    /// Aliases accepted in data files (schema validation resolves them).
    pub aliases: &'static [&'static str],
    /// Deprecation note, when the id or an alias is deprecated.
    pub deprecations: &'static [(&'static str, &'static str)],
    /// Validation rules that apply to claims about this capability.
    pub validation: &'static str,
}

/// The capability catalog: every capability core understands.
pub const CAPABILITY_CATALOG: &[CapabilityCatalogEntry] = &[
    CapabilityCatalogEntry {
        id: Capability::WebSearch,
        meaning: "Search the web during a turn and use the results in the answer",
        aliases: &["web-search", "websearch"],
        deprecations: &[],
        validation: "claims must name a transport (client tool or server-side tool)",
    },
    CapabilityCatalogEntry {
        id: Capability::Vision,
        meaning: "Accept images as model input",
        aliases: &["image-input", "images"],
        deprecations: &[],
        validation: "requires an image-carrying wire transport to the provider",
    },
    CapabilityCatalogEntry {
        id: Capability::ComputerUse,
        meaning: "Drive a screen, keyboard, and mouse",
        aliases: &["computer-use"],
        deprecations: &[],
        validation: "requires a computer-use-capable provider API and harness loop",
    },
    CapabilityCatalogEntry {
        id: Capability::Mcp,
        meaning: "Connect MCP servers and expose their tools",
        aliases: &["mcp-servers", "mcp_servers"],
        deprecations: &[],
        validation: "substituted claims must name the installed server or plugin",
    },
];

/// All known capabilities, for completeness checks.
pub const ALL_CAPABILITIES: &[Capability] = &[
    Capability::WebSearch,
    Capability::Vision,
    Capability::ComputerUse,
    Capability::Mcp,
];

/// Parse a capability id (or documented alias) into the typed capability.
///
/// Unknown ids are a typed validation error — never silently treated as
/// absent (CAP-01). Matching is case-insensitive on the `snake_case` id and
/// the catalog aliases.
pub fn parse_capability_id(input: &str) -> Result<Capability> {
    let normalized = input.trim().to_ascii_lowercase();
    for entry in CAPABILITY_CATALOG {
        let id_str = serde_plain_id(entry.id);
        if normalized == id_str {
            return Ok(entry.id);
        }
        if entry.aliases.iter().any(|alias| normalized == **alias) {
            return Ok(entry.id);
        }
    }
    Err(CoreError::Validation {
        field: "capability".to_owned(),
        reason: format!(
            "unknown capability id `{input}`; known ids: {}",
            CAPABILITY_CATALOG
                .iter()
                .map(|e| serde_plain_id(e.id))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

fn serde_plain_id(capability: Capability) -> String {
    capability.to_string()
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::WebSearch => "web_search",
            Self::Vision => "vision",
            Self::ComputerUse => "computer_use",
            Self::Mcp => "mcp",
        };
        f.write_str(s)
    }
}

/// Parse a support value from data (CAP-01: reject unknown cleanly).
pub fn parse_support(input: &str) -> Result<Support> {
    match input.trim().to_ascii_lowercase().as_str() {
        "native" => Ok(Support::Native),
        "substituted" => Ok(Support::Substituted),
        "absent" => Ok(Support::Absent),
        other => Err(CoreError::Validation {
            field: "support".to_owned(),
            reason: format!("unknown support value `{other}`; expected native|substituted|absent"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_covers_all_capabilities_with_meanings() {
        for cap in ALL_CAPABILITIES {
            let entry = CAPABILITY_CATALOG
                .iter()
                .find(|e| e.id == *cap)
                .unwrap_or_else(|| panic!("capability {cap:?} missing from catalog"));
            assert!(!entry.meaning.is_empty());
            assert!(!entry.validation.is_empty());
        }
        assert_eq!(CAPABILITY_CATALOG.len(), ALL_CAPABILITIES.len());
    }

    #[test]
    fn parse_capability_id_resolves_aliases_and_rejects_unknown() {
        assert_eq!(
            parse_capability_id("web_search").unwrap(),
            Capability::WebSearch
        );
        assert_eq!(
            parse_capability_id("web-search").unwrap(),
            Capability::WebSearch
        );
        assert_eq!(parse_capability_id("MCP_SERVERS").unwrap(), Capability::Mcp);
        assert_eq!(
            parse_capability_id("image-input").unwrap(),
            Capability::Vision
        );
        let err = parse_capability_id("telepathy").unwrap_err().to_string();
        assert!(err.contains("unknown capability id"), "got: {err}");
        assert!(err.contains("web_search"), "lists known ids: {err}");
    }

    #[test]
    fn parse_support_rejects_unknown() {
        assert_eq!(parse_support("native").unwrap(), Support::Native);
        assert_eq!(parse_support("SUBSTITUTED").unwrap(), Support::Substituted);
        assert_eq!(parse_support("absent").unwrap(), Support::Absent);
        parse_support("maybe").unwrap_err();
    }
}
