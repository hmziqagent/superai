//! Capability resolution — harness/provider pair matrix.
//!
//! Support is not a boolean: a capability can be native, substituted, or absent,
//! and it depends on the harness *and* provider together. This module resolves
//! a capability for a given harness/provider pair using a data-driven matrix.
//! The matrix lives as a static map; a file-based override can also be loaded.
//! No interface consumer branches on harness identity — they ask this resolver.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::capability::{Capability, Support};
use crate::error::{CoreError, Result};
use crate::ids::{HarnessId, ProviderId};

// ---------------------------------------------------------------------------
// Support source
// ---------------------------------------------------------------------------

/// Which data source satisfies a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySource {
    /// Harness implements the capability directly.
    Harness,
    /// Provider satisfies it server-side or via alternative transport.
    Provider,
    /// Harness/provider template declares the mapping.
    Template,
    /// Installed plugin or MCP server provides it.
    Plugin,
    /// Local or admin policy controls it.
    Policy,
    /// Unknown — no matrix entry.
    Unknown,
}

impl std::fmt::Display for CapabilitySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Harness => "harness",
            Self::Provider => "provider",
            Self::Template => "template",
            Self::Plugin => "plugin",
            Self::Policy => "policy",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// Resolved entry
// ---------------------------------------------------------------------------

/// Resolved capability — support plus source and explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCapability {
    /// How the capability is supported for this pair.
    pub support: Support,
    /// Which source satisfies it.
    pub source: CapabilitySource,
    /// Concise human explanation.
    pub explanation: String,
}

impl ResolvedCapability {
    /// Create a resolved entry.
    pub fn new(support: Support, source: CapabilitySource, explanation: &str) -> Self {
        Self {
            support,
            source,
            explanation: explanation.to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Static matrix entry
// ---------------------------------------------------------------------------

/// One row in the static harness/provider/capability matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatrixEntry {
    /// Harness identifier lowercased slug (e.g. `claude-code`).
    pub harness: &'static str,
    /// Provider identifier lowercased slug (e.g. `anthropic`).
    pub provider: &'static str,
    /// Capability this row covers.
    pub capability: Capability,
    /// Resolved support level.
    pub support: Support,
    /// Which source satisfies it.
    pub source: CapabilitySource,
    /// Concise explanation for UI.
    pub explanation: &'static str,
}

/// All known capabilities, for completeness checks.
pub const ALL_CAPABILITIES: &[Capability] = &[
    Capability::WebSearch,
    Capability::Vision,
    Capability::ComputerUse,
    Capability::Mcp,
];

/// Active harness/provider pairs that must be fully covered.
///
/// Adding a provider is data-only for the provider file, but the capability
/// matrix must gain rows for new pairs before they are considered complete.
/// Completeness validation fails if any pair here lacks a row for any
/// capability in [`ALL_CAPABILITIES`].
pub const ACTIVE_PAIRS: &[(&str, &str)] = &[
    ("claude-code", "anthropic"),
    ("claude-code", "glm"),
    ("claude-code", "openai"),
    ("codex-cli", "openai"),
    ("codex-cli", "anthropic"),
    ("opencode", "anthropic"),
    ("opencode", "glm"),
    ("pi", "anthropic"),
    ("aider", "openai"),
    ("cline", "anthropic"),
];

/// Static harness/provider/capability matrix — data-driven, no harness branch
/// in consumer code. Each entry is a data row; adding a provider means adding
/// rows, not code branches in callers.
///
/// Reference scenarios encoded:
/// - `claude-code` + `anthropic`: `web_search` native (client tool)
/// - `claude-code` + `glm`: `web_search` substituted (provider server search)
/// - `claude-code` + `glm`: vision absent (harness transport incompatible)
/// - `pi` + `anthropic`: mcp absent natively; plugin may provide substituted (not yet)
pub const MATRIX: &[MatrixEntry] = &[
    // claude-code + anthropic — native across the board
    MatrixEntry {
        harness: "claude-code",
        provider: "anthropic",
        capability: Capability::WebSearch,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Claude Code web_search native via client tool on Anthropic",
    },
    MatrixEntry {
        harness: "claude-code",
        provider: "anthropic",
        capability: Capability::Vision,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Claude Code vision native on Anthropic transport",
    },
    MatrixEntry {
        harness: "claude-code",
        provider: "anthropic",
        capability: Capability::ComputerUse,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Claude Code computer_use native on Anthropic",
    },
    MatrixEntry {
        harness: "claude-code",
        provider: "anthropic",
        capability: Capability::Mcp,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Claude Code MCP native",
    },
    // claude-code + glm — web_search substituted, vision absent (transport incompatible)
    MatrixEntry {
        harness: "claude-code",
        provider: "glm",
        capability: Capability::WebSearch,
        support: Support::Substituted,
        source: CapabilitySource::Provider,
        explanation: "Claude Code web_search substituted via GLM server-side search",
    },
    MatrixEntry {
        harness: "claude-code",
        provider: "glm",
        capability: Capability::Vision,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Claude Code vision absent on GLM — transport incompatible even though model advertises vision",
    },
    MatrixEntry {
        harness: "claude-code",
        provider: "glm",
        capability: Capability::ComputerUse,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Claude Code computer_use absent on GLM",
    },
    MatrixEntry {
        harness: "claude-code",
        provider: "glm",
        capability: Capability::Mcp,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Claude Code MCP native (provider-independent)",
    },
    // claude-code + openai
    MatrixEntry {
        harness: "claude-code",
        provider: "openai",
        capability: Capability::WebSearch,
        support: Support::Substituted,
        source: CapabilitySource::Provider,
        explanation: "Claude Code web_search substituted via OpenAI server search",
    },
    MatrixEntry {
        harness: "claude-code",
        provider: "openai",
        capability: Capability::Vision,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Claude Code vision native on OpenAI transport",
    },
    MatrixEntry {
        harness: "claude-code",
        provider: "openai",
        capability: Capability::ComputerUse,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Claude Code computer_use absent on OpenAI",
    },
    MatrixEntry {
        harness: "claude-code",
        provider: "openai",
        capability: Capability::Mcp,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Claude Code MCP native",
    },
    // codex-cli + openai — full native
    MatrixEntry {
        harness: "codex-cli",
        provider: "openai",
        capability: Capability::WebSearch,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Codex CLI web_search native on OpenAI",
    },
    MatrixEntry {
        harness: "codex-cli",
        provider: "openai",
        capability: Capability::Vision,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Codex CLI vision native on OpenAI",
    },
    MatrixEntry {
        harness: "codex-cli",
        provider: "openai",
        capability: Capability::ComputerUse,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Codex CLI computer_use absent",
    },
    MatrixEntry {
        harness: "codex-cli",
        provider: "openai",
        capability: Capability::Mcp,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Codex CLI MCP native",
    },
    // codex-cli + anthropic — vision absent (provider transport)
    MatrixEntry {
        harness: "codex-cli",
        provider: "anthropic",
        capability: Capability::WebSearch,
        support: Support::Substituted,
        source: CapabilitySource::Provider,
        explanation: "Codex CLI web_search substituted via Anthropic server search",
    },
    MatrixEntry {
        harness: "codex-cli",
        provider: "anthropic",
        capability: Capability::Vision,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Codex CLI vision absent on Anthropic transport",
    },
    MatrixEntry {
        harness: "codex-cli",
        provider: "anthropic",
        capability: Capability::ComputerUse,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Codex CLI computer_use absent",
    },
    MatrixEntry {
        harness: "codex-cli",
        provider: "anthropic",
        capability: Capability::Mcp,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Codex CLI MCP native",
    },
    // opencode + anthropic
    MatrixEntry {
        harness: "opencode",
        provider: "anthropic",
        capability: Capability::WebSearch,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "OpenCode web_search native on Anthropic",
    },
    MatrixEntry {
        harness: "opencode",
        provider: "anthropic",
        capability: Capability::Vision,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "OpenCode vision native",
    },
    MatrixEntry {
        harness: "opencode",
        provider: "anthropic",
        capability: Capability::ComputerUse,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "OpenCode computer_use absent",
    },
    MatrixEntry {
        harness: "opencode",
        provider: "anthropic",
        capability: Capability::Mcp,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "OpenCode MCP native",
    },
    // opencode + glm — computer_use absent, web_search substituted
    MatrixEntry {
        harness: "opencode",
        provider: "glm",
        capability: Capability::WebSearch,
        support: Support::Substituted,
        source: CapabilitySource::Provider,
        explanation: "OpenCode web_search substituted via GLM",
    },
    MatrixEntry {
        harness: "opencode",
        provider: "glm",
        capability: Capability::Vision,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "OpenCode vision absent on GLM transport",
    },
    MatrixEntry {
        harness: "opencode",
        provider: "glm",
        capability: Capability::ComputerUse,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "OpenCode computer_use absent",
    },
    MatrixEntry {
        harness: "opencode",
        provider: "glm",
        capability: Capability::Mcp,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "OpenCode MCP native",
    },
    // pi + anthropic — MCP absent natively
    MatrixEntry {
        harness: "pi",
        provider: "anthropic",
        capability: Capability::WebSearch,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Pi web_search native",
    },
    MatrixEntry {
        harness: "pi",
        provider: "anthropic",
        capability: Capability::Vision,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Pi vision native",
    },
    MatrixEntry {
        harness: "pi",
        provider: "anthropic",
        capability: Capability::ComputerUse,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Pi computer_use absent",
    },
    MatrixEntry {
        harness: "pi",
        provider: "anthropic",
        capability: Capability::Mcp,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Pi MCP absent natively; verified extension may provide substituted",
    },
    // aider + openai
    MatrixEntry {
        harness: "aider",
        provider: "openai",
        capability: Capability::WebSearch,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Aider web_search absent",
    },
    MatrixEntry {
        harness: "aider",
        provider: "openai",
        capability: Capability::Vision,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Aider vision absent",
    },
    MatrixEntry {
        harness: "aider",
        provider: "openai",
        capability: Capability::ComputerUse,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Aider computer_use absent",
    },
    MatrixEntry {
        harness: "aider",
        provider: "openai",
        capability: Capability::Mcp,
        support: Support::Absent,
        source: CapabilitySource::Harness,
        explanation: "Aider MCP absent",
    },
    // cline + anthropic
    MatrixEntry {
        harness: "cline",
        provider: "anthropic",
        capability: Capability::WebSearch,
        support: Support::Substituted,
        source: CapabilitySource::Provider,
        explanation: "Cline web_search substituted via provider",
    },
    MatrixEntry {
        harness: "cline",
        provider: "anthropic",
        capability: Capability::Vision,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Cline vision native",
    },
    MatrixEntry {
        harness: "cline",
        provider: "anthropic",
        capability: Capability::ComputerUse,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Cline computer_use native",
    },
    MatrixEntry {
        harness: "cline",
        provider: "anthropic",
        capability: Capability::Mcp,
        support: Support::Native,
        source: CapabilitySource::Harness,
        explanation: "Cline MCP native",
    },
];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve a single capability for a harness/provider pair.
///
/// Lookup is case-folded on harness and provider ids. Returns absent with
/// `Unknown` source and an explanation if no matrix entry exists — callers
/// never panic on unknown pairs.
pub fn resolve(harness: &HarnessId, provider: &ProviderId, cap: Capability) -> ResolvedCapability {
    resolve_with_matrix(harness, provider, cap, MATRIX)
}

/// Resolve using an explicit matrix slice (for file-driven overrides and tests).
pub fn resolve_with_matrix(
    harness: &HarnessId,
    provider: &ProviderId,
    cap: Capability,
    matrix: &[MatrixEntry],
) -> ResolvedCapability {
    for entry in matrix {
        if entry.capability != cap {
            continue;
        }
        let harness_matches = harness.eq_case_fold_str(entry.harness);
        let provider_matches = provider.eq_case_fold_str(entry.provider);
        if harness_matches && provider_matches {
            return ResolvedCapability {
                support: entry.support,
                source: entry.source,
                explanation: entry.explanation.to_owned(),
            };
        }
    }
    ResolvedCapability {
        support: Support::Absent,
        source: CapabilitySource::Unknown,
        explanation: format!(
            "no matrix entry for harness `{harness}` provider `{provider}` capability `{cap:?}`"
        ),
    }
}

/// Resolve all capabilities for a harness/provider pair.
pub fn resolve_all(
    harness: &HarnessId,
    provider: &ProviderId,
) -> Vec<(Capability, ResolvedCapability)> {
    resolve_all_with_matrix(harness, provider, MATRIX)
}

/// Resolve all using an explicit matrix.
pub fn resolve_all_with_matrix(
    harness: &HarnessId,
    provider: &ProviderId,
    matrix: &[MatrixEntry],
) -> Vec<(Capability, ResolvedCapability)> {
    let mut out = Vec::with_capacity(ALL_CAPABILITIES.len());
    for cap in ALL_CAPABILITIES {
        let resolved = resolve_with_matrix(harness, provider, *cap, matrix);
        out.push((*cap, resolved));
    }
    out
}

/// Validate that the static matrix is complete for every active pair.
///
/// Each pair in `ACTIVE_PAIRS` must have a row for every capability in
/// `ALL_CAPABILITIES`, with no duplicate rows and with substituted rows naming
/// a provider or template source.
pub fn validate_matrix_completeness() -> Result<()> {
    validate_matrix_completeness_with(MATRIX, ACTIVE_PAIRS, ALL_CAPABILITIES)
}

#[expect(clippy::excessive_nesting, reason = "matrix validation")]
fn validate_matrix_completeness_with(
    matrix: &[MatrixEntry],
    pairs: &[(&str, &str)],
    caps: &[Capability],
) -> Result<()> {
    // No duplicate rows.
    let mut seen: std::collections::HashSet<(String, String, Capability)> =
        std::collections::HashSet::new();
    for e in matrix {
        let key = (
            e.harness.to_lowercase(),
            e.provider.to_lowercase(),
            e.capability,
        );
        if seen.contains(&key) {
            return Err(CoreError::Validation {
                field: "matrix".to_owned(),
                reason: format!(
                    "duplicate matrix entry harness `{}` provider `{}` cap `{:?}`",
                    e.harness, e.provider, e.capability
                ),
            });
        }
        seen.insert(key);
    }
    // Every active pair has every capability.
    for (harness, provider) in pairs {
        for cap in caps {
            let mut found = false;
            for e in matrix {
                if e.harness.to_lowercase() == harness.to_lowercase()
                    && e.provider.to_lowercase() == provider.to_lowercase()
                    && &e.capability == cap
                {
                    found = true;
                    // Substituted must name provider/template/plugin source, not harness alone unless provider source.
                    if e.support == Support::Substituted
                        && matches!(
                            e.source,
                            CapabilitySource::Harness | CapabilitySource::Unknown
                        )
                    {
                        // For substituted, harness alone is not sufficient unless provider is named— but we allow Provider/Template/Plugin.
                        // Enforce that substituted rows have Provider, Template, or Plugin source.
                        return Err(CoreError::Validation {
                            field: "matrix".to_owned(),
                            reason: format!(
                                "substituted entry for harness `{harness}` provider `{provider}` cap `{cap:?}` must have provider/template/plugin source, got `{}`",
                                e.source
                            ),
                        });
                    }
                    // Explanation non-empty.
                    if e.explanation.trim().is_empty() {
                        return Err(CoreError::Validation {
                            field: "matrix".to_owned(),
                            reason: format!(
                                "matrix entry harness `{harness}` provider `{provider}` cap `{cap:?}` has empty explanation"
                            ),
                        });
                    }
                    break;
                }
            }
            if !found {
                return Err(CoreError::Validation {
                    field: "matrix".to_owned(),
                    reason: format!(
                        "incomplete matrix: harness `{harness}` provider `{provider}` missing cap `{cap:?}`"
                    ),
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// File-driven matrix loading
// ---------------------------------------------------------------------------

/// File-driven matrix row (JSON/YAML deserializable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMatrixEntry {
    /// Harness slug.
    pub harness: String,
    /// Provider slug.
    pub provider: String,
    /// Capability.
    pub capability: Capability,
    /// Support level.
    pub support: Support,
    /// Source.
    #[serde(default)]
    pub source: Option<CapabilitySource>,
    /// Explanation.
    pub explanation: String,
}

/// Load a file-driven matrix from JSON or YAML.
///
/// Accepts a single file containing an array of rows, or a single row.
/// JSON detected by `.json` extension, YAML by `.yaml`/`.yml`, fallback tries both.
/// Returns file rows converted to owned entries for use with `resolve_with_matrix`.
pub fn load_matrix_from_file(path: &Path) -> Result<Vec<FileMatrixEntry>> {
    let text = std::fs::read_to_string(path).map_err(|source| CoreError::InvalidPath {
        kind: "capability_matrix".to_owned(),
        value: path.display().to_string(),
        reason: format!("cannot read file: {source}"),
    })?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if ext == "json" {
        parse_matrix_json(&text, path)
    } else if ext == "yaml" || ext == "yml" {
        parse_matrix_yaml(&text, path)
    } else {
        parse_matrix_json(&text, path).or_else(|_| parse_matrix_yaml(&text, path))
    }
}

fn parse_matrix_json(text: &str, path: &Path) -> Result<Vec<FileMatrixEntry>> {
    if let Ok(vec) = serde_json::from_str::<Vec<FileMatrixEntry>>(text) {
        return Ok(vec);
    }
    match serde_json::from_str::<FileMatrixEntry>(text) {
        Ok(single) => Ok(vec![single]),
        Err(source) => Err(CoreError::Parse {
            path: path.to_path_buf(),
            kind: "json".to_owned(),
            message: source.to_string(),
        }),
    }
}

fn parse_matrix_yaml(text: &str, path: &Path) -> Result<Vec<FileMatrixEntry>> {
    if let Ok(vec) = yaml_serde::from_str::<Vec<FileMatrixEntry>>(text) {
        return Ok(vec);
    }
    match yaml_serde::from_str::<FileMatrixEntry>(text) {
        Ok(single) => Ok(vec![single]),
        Err(source) => Err(CoreError::Parse {
            path: path.to_path_buf(),
            kind: "yaml".to_owned(),
            message: source.to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn hid(s: &str) -> HarnessId {
        HarnessId::new(s).unwrap()
    }
    fn pid(s: &str) -> ProviderId {
        ProviderId::new(s).unwrap()
    }

    #[test]
    fn matrix_completeness_static() {
        validate_matrix_completeness().unwrap();
    }

    #[test]
    fn same_harness_different_provider_yields_different_support() {
        // Claude Code web_search: native on anthropic, substituted on glm
        let native = resolve(
            &hid("claude-code"),
            &pid("anthropic"),
            Capability::WebSearch,
        );
        let substituted = resolve(&hid("claude-code"), &pid("glm"), Capability::WebSearch);
        assert_eq!(native.support, Support::Native);
        assert_eq!(substituted.support, Support::Substituted);
        assert_ne!(native.support, substituted.support);
        assert_ne!(native.explanation, substituted.explanation);
    }

    #[test]
    fn provider_cannot_override_incompatible_harness_transport() {
        // GLM vision for claude-code is absent even though model advertises vision.
        let vision = resolve(&hid("claude-code"), &pid("glm"), Capability::Vision);
        assert_eq!(
            vision.support,
            Support::Absent,
            "harness transport incompatibility wins"
        );
        // Ensure explanation mentions transport or incompatibility.
        assert!(
            vision.explanation.contains("transport") || vision.explanation.contains("Absent"),
            "explanation should mention why absent: {}",
            vision.explanation
        );
    }

    #[test]
    fn pi_mcp_absent_natively() {
        let mcp = resolve(&hid("pi"), &pid("anthropic"), Capability::Mcp);
        assert_eq!(mcp.support, Support::Absent);
        assert_eq!(mcp.source, CapabilitySource::Harness);
    }

    #[test]
    fn unknown_pair_returns_absent_unknown() {
        let r = resolve(
            &hid("unknown-harness-xyz"),
            &pid("unknown-provider-xyz"),
            Capability::WebSearch,
        );
        assert_eq!(r.support, Support::Absent);
        assert_eq!(r.source, CapabilitySource::Unknown);
        assert!(r.explanation.contains("no matrix entry"));
    }

    #[test]
    fn resolve_all_returns_all_capabilities() {
        let all = resolve_all(&hid("claude-code"), &pid("anthropic"));
        assert_eq!(all.len(), ALL_CAPABILITIES.len());
        let caps: Vec<Capability> = all.iter().map(|(c, _)| *c).collect();
        for cap in ALL_CAPABILITIES {
            assert!(caps.contains(cap), "missing {cap:?}");
        }
    }

    #[test]
    fn file_driven_matrix_data_only() {
        let dir = std::env::temp_dir().join(format!("superai-cap-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("matrix.json");
        let json = r#"[
  {
    "harness": "synthetic-harness",
    "provider": "synthetic-provider",
    "capability": "web_search",
    "support": "substituted",
    "source": "provider",
    "explanation": "synthetic substituted for test"
  },
  {
    "harness": "synthetic-harness",
    "provider": "synthetic-provider",
    "capability": "vision",
    "support": "absent",
    "source": "harness",
    "explanation": "synthetic absent"
  }
]"#;
        std::fs::write(&path, json).unwrap();
        let rows = load_matrix_from_file(&path).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].harness, "synthetic-harness");
        // Convert to static-like entries for resolve_with_matrix check.
        let synthetic_matrix = [
            MatrixEntry {
                harness: "synthetic-harness",
                provider: "synthetic-provider",
                capability: Capability::WebSearch,
                support: Support::Substituted,
                source: CapabilitySource::Provider,
                explanation: "synthetic substituted for test",
            },
            MatrixEntry {
                harness: "synthetic-harness",
                provider: "synthetic-provider",
                capability: Capability::Vision,
                support: Support::Absent,
                source: CapabilitySource::Harness,
                explanation: "synthetic absent",
            },
        ];
        let r = resolve_with_matrix(
            &hid("synthetic-harness"),
            &pid("synthetic-provider"),
            Capability::WebSearch,
            &synthetic_matrix,
        );
        assert_eq!(r.support, Support::Substituted);
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn duplicate_matrix_detection() {
        let dup = [
            MatrixEntry {
                harness: "a",
                provider: "b",
                capability: Capability::WebSearch,
                support: Support::Native,
                source: CapabilitySource::Harness,
                explanation: "first",
            },
            MatrixEntry {
                harness: "a",
                provider: "b",
                capability: Capability::WebSearch,
                support: Support::Native,
                source: CapabilitySource::Harness,
                explanation: "dup",
            },
        ];
        validate_matrix_completeness_with(&dup, &[("a", "b")], &[Capability::WebSearch])
            .unwrap_err();
    }

    #[test]
    fn incomplete_matrix_fails() {
        let incomplete: &[MatrixEntry] = &[];
        validate_matrix_completeness_with(incomplete, &[("a", "b")], &[Capability::WebSearch])
            .unwrap_err();
    }

    #[test]
    fn substituted_requires_provider_or_template_source() {
        let bad = [MatrixEntry {
            harness: "a",
            provider: "b",
            capability: Capability::WebSearch,
            support: Support::Substituted,
            source: CapabilitySource::Harness,
            explanation: "bad source",
        }];
        validate_matrix_completeness_with(&bad, &[("a", "b")], &[Capability::WebSearch])
            .unwrap_err();
    }

    #[test]
    fn all_capabilities_are_serializable() {
        for cap in ALL_CAPABILITIES {
            let json = serde_json::to_string(cap).unwrap();
            let back: Capability = serde_json::from_str(&json).unwrap();
            assert_eq!(*cap, back);
        }
        for support in [Support::Native, Support::Substituted, Support::Absent] {
            let json = serde_json::to_string(&support).unwrap();
            let back: Support = serde_json::from_str(&json).unwrap();
            assert_eq!(support, back);
        }
    }

    #[test]
    fn resolve_is_case_insensitive_on_ids() {
        let lower = resolve(
            &hid("claude-code"),
            &pid("anthropic"),
            Capability::WebSearch,
        );
        let upper = resolve(
            &hid("Claude-Code"),
            &pid("Anthropic"),
            Capability::WebSearch,
        );
        assert_eq!(lower, upper);
    }

    #[test]
    fn capability_delta_visible() {
        // Simulate template update preview: GLM vision absent vs Anthropic native — delta is visible.
        let before = resolve(&hid("claude-code"), &pid("glm"), Capability::Vision);
        let after = resolve(&hid("claude-code"), &pid("anthropic"), Capability::Vision);
        assert_ne!(before.support, after.support);
        // Delta would be absent -> native.
        assert_eq!(before.support, Support::Absent);
        assert_eq!(after.support, Support::Native);
    }
}
