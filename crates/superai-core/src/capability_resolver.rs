//! Capability resolution — fresh, source-fed, precedence-explicit (plan 09).
//!
//! Support is not a boolean: a capability can be native, substituted, or
//! absent, and it depends on the harness *and_ provider together.
//!
//! Resolution consults CURRENT data (CAP-03/CAP-05), in this precedence:
//!
//! 1. **Adapter declaration** — the harness transport constraint. An `absent`
//!    transport claim is final: provider, template, plugin, and policy data
//!    cannot override an incompatible harness transport.
//! 2. **Provider data** — `ProviderDefinition::capabilities` (server-side or
//!    modal capabilities). A provider `substituted`/`absent` claim replaces
//!    the transport default; a provider `native` claim alongside a `native`
//!    transport keeps the harness as the satisfying source.
//! 3. **Template capability map** — pair-specific overrides from the applied
//!    template.
//! 4. **Installed plugin/MCP state** — may raise `absent` to `substituted`
//!    only where the adapter can verify it (an MCP declaration plus
//!    installed servers read fresh from the instance config).
//! 5. **Policy** — may disable (downgrade) support, never upgrade it.
//!
//! Nothing is persisted: every query resolves fresh from its inputs (CAP-05),
//! and results are returned keyed by [`crate::ids::InstanceId`] for the
//! public instance queries — harness identity stays internal diagnostic
//! metadata.
//!
//! The legacy `MATRIX` const and its completeness validator are retained as
//! reference data for the invariant tests only; resolution NEVER consults
//! compile-time data.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::adapter::{Adapter, AdapterCapabilityDecl};
use crate::capability::{Capability, Support};
use crate::error::{CoreError, Result};
use crate::ids::{HarnessId, InstanceId, ProviderId};
use crate::instance::Instance;
use crate::provider::ProviderDefinition;

pub use crate::capability::ALL_CAPABILITIES;
// Single public path for the catalog vocabulary (capability.rs is private).
pub use crate::capability::{
    CAPABILITY_CATALOG, CapabilityCatalogEntry, parse_capability_id, parse_support,
};

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
    /// Unknown — no source resolved the capability.
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
// Resolved entry (CAP-02: source, explanation, evidence, limitations)
// ---------------------------------------------------------------------------

/// Resolved capability — support plus source, explanation, evidence, and
/// limitations (CAP-02).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCapability {
    /// How the capability is supported for this pair.
    pub support: Support,
    /// Which source satisfies it.
    pub source: CapabilitySource,
    /// Concise human explanation.
    pub explanation: String,
    /// Evidence backing the claim (adapter/provider/template data or the
    /// installed extension that was verified).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// Version range the claim is valid for, when the source declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_range: Option<String>,
    /// Known limitations of this support (CAP-02), including the
    /// absent-claim upgrade hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limitations: Option<String>,
}

impl ResolvedCapability {
    /// Create a resolved entry.
    pub fn new(support: Support, source: CapabilitySource, explanation: &str) -> Self {
        Self {
            support,
            source,
            explanation: explanation.to_owned(),
            evidence: None,
            version_range: None,
            limitations: None,
        }
    }

    /// Attach evidence (builder).
    #[must_use]
    pub fn with_evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence = Some(evidence.into());
        self
    }

    /// Attach a version range (builder).
    #[must_use]
    pub fn with_version_range(mut self, range: impl Into<String>) -> Self {
        self.version_range = Some(range.into());
        self
    }

    /// Attach limitations (builder).
    #[must_use]
    pub fn with_limitations(mut self, limitations: impl Into<String>) -> Self {
        self.limitations = Some(limitations.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Resolution inputs (CAP-03 sources)
// ---------------------------------------------------------------------------

/// Installed extension state for one instance (CAP-03 source 4).
///
/// Supplied by the caller or built fresh from the instance config; only
/// entries the adapter can verify influence resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionState {
    /// MCP server ids installed on the instance.
    pub installed_mcp_servers: Vec<String>,
    /// Plugin ids enabled on the instance.
    pub enabled_plugins: Vec<String>,
}

impl ExtensionState {
    /// Read the installed MCP server ids fresh from the instance's declared
    /// MCP destination (read-only; never writes, never parses secret stores).
    ///
    /// Returns the empty state when the adapter has no MCP declaration or
    /// the destination file does not exist.
    pub fn from_instance(adapter: &dyn Adapter, instance: &Instance) -> Self {
        let mut state = Self::default();
        let Some(decl) = adapter.mcp_decl() else {
            return state;
        };
        let path = instance.config_root.as_path().join(&decl.dest_file);
        let Ok(bytes) = std::fs::read(&path) else {
            return state;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return state;
        };
        if let Some(servers) = value.get(&decl.dest_key).and_then(|v| v.as_object()) {
            state.installed_mcp_servers = servers.keys().cloned().collect();
            state.installed_mcp_servers.sort();
        }
        state
    }
}

/// All resolution inputs for one resolution call (CAP-03).
///
/// Everything here is CURRENT data supplied per call — the resolver holds no
/// caches and persists nothing (CAP-05).
#[derive(Debug)]
pub struct CapabilitySources<'a> {
    /// Harness transport declarations (source 1).
    pub adapter_decls: Vec<AdapterCapabilityDecl>,
    /// Whether the adapter declares an MCP destination (gates source 4).
    pub adapter_verifies_mcp: bool,
    /// Provider capability data (source 2).
    pub provider: Option<&'a ProviderDefinition>,
    /// Template capability-map overrides (source 3).
    pub template_map: Option<&'a BTreeMap<Capability, Support>>,
    /// Installed extension state (source 4).
    pub extensions: ExtensionState,
    /// Policy rows (source 5); downgrades only.
    pub policy: Vec<FileMatrixEntry>,
    /// Installed harness version, when known (CAP-04 native-vs-version).
    pub harness_version: Option<String>,
    /// Harness label for diagnostics (internal metadata).
    pub harness_label: String,
}

impl<'a> CapabilitySources<'a> {
    /// Gather the sources available from a live adapter.
    #[must_use]
    pub fn for_adapter(
        adapter: &dyn Adapter,
        provider: Option<&'a ProviderDefinition>,
        template_map: Option<&'a BTreeMap<Capability, Support>>,
    ) -> Self {
        Self {
            adapter_decls: adapter.capability_declarations(),
            adapter_verifies_mcp: adapter.mcp_decl().is_some(),
            provider,
            template_map,
            extensions: ExtensionState::default(),
            policy: Vec::new(),
            harness_version: None,
            harness_label: adapter.id().to_string(),
        }
    }

    /// Attach installed extension state (builder).
    #[must_use]
    pub fn with_extensions(mut self, extensions: ExtensionState) -> Self {
        self.extensions = extensions;
        self
    }

    /// Attach policy rows (builder).
    #[must_use]
    pub fn with_policy(mut self, policy: Vec<FileMatrixEntry>) -> Self {
        self.policy = policy;
        self
    }

    /// Attach the installed harness version (builder).
    #[must_use]
    pub fn with_harness_version(mut self, version: impl Into<String>) -> Self {
        self.harness_version = Some(version.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Source-driven resolution (CAP-03/CAP-05)
// ---------------------------------------------------------------------------

fn support_rank(support: Support) -> u8 {
    match support {
        Support::Native => 2,
        Support::Substituted => 1,
        Support::Absent => 0,
    }
}

/// Resolve one capability from explicit sources.
#[expect(
    clippy::too_many_lines,
    reason = "precedence chain covers five sources"
)]
pub fn resolve_with_sources(
    harness: &HarnessId,
    provider_id: &ProviderId,
    cap: Capability,
    sources: &CapabilitySources<'_>,
) -> ResolvedCapability {
    // Source 1 — the harness transport constraint.
    let Some(decl) = sources.adapter_decls.iter().find(|d| d.capability == cap) else {
        // The adapter has not modeled this capability's transport. A
        // template declaration still resolves the pair (the template author
        // takes responsibility); otherwise the capability is honestly
        // Unknown — never silently absent.
        if let Some(map) = sources.template_map
            && let Some(support) = map.get(&cap)
        {
            return ResolvedCapability::new(
                *support,
                CapabilitySource::Template,
                &format!(
                    "template declares {support} for {cap}; adapter `{}` has not modeled the transport",
                    sources.harness_label
                ),
            )
            .with_evidence("applied template capability map");
        }
        return ResolvedCapability {
            support: Support::Absent,
            source: CapabilitySource::Unknown,
            explanation: format!(
                "no matrix entry for harness `{harness}` provider `{provider_id}` capability `{cap}`: adapter declares no capability transport"
            ),
            evidence: None,
            version_range: None,
            limitations: None,
        };
    };
    let version_blocked = decl
        .version_req
        .as_deref()
        .and_then(|req| semver::VersionReq::parse(req).ok())
        .zip(
            sources
                .harness_version
                .as_deref()
                .and_then(|v| semver::Version::parse(v).ok()),
        )
        .is_some_and(|(req, installed)| !req.matches(&installed));
    let transport_absent = decl.support == Support::Absent || version_blocked;
    let mut support = if transport_absent {
        Support::Absent
    } else {
        decl.support
    };
    let mut source = CapabilitySource::Harness;
    let mut explanation = if version_blocked {
        format!(
            "native claim requires harness {}, installed `{}` does not match",
            decl.version_req.as_deref().unwrap_or("unknown"),
            sources.harness_version.as_deref().unwrap_or("unknown")
        )
    } else {
        decl.explanation.clone()
    };
    let mut evidence = format!("adapter `{}` transport declaration", sources.harness_label);
    let mut version_range = decl.version_req.clone();

    // Source 2 — provider capability data. An incompatible (absent or
    // version-blocked) transport is FINAL here: provider data cannot
    // override it.
    if !transport_absent
        && let Some(provider) = sources.provider
        && let Some(provider_decl) = provider.capability_decl(cap)
    {
        match provider_decl.support {
            Support::Native => {
                if support != Support::Native {
                    support = Support::Native;
                    source = CapabilitySource::Provider;
                    explanation.clone_from(&provider_decl.explanation);
                }
                // Native transport + native provider: the harness remains
                // the satisfying source.
            }
            Support::Substituted | Support::Absent => {
                support = provider_decl.support;
                source = CapabilitySource::Provider;
                explanation.clone_from(&provider_decl.explanation);
            }
        }
        evidence = format!("provider `{}` capability data", provider.id);
        if let Some(l) = provider_decl.limitations.as_deref() {
            explanation = format!("{explanation}; {l}");
        }
    }

    // Source 3 — template overrides (also blocked by an incompatible
    // transport, same precedence rule).
    if !transport_absent
        && let Some(map) = sources.template_map
        && let Some(override_support) = map.get(&cap)
    {
        support = *override_support;
        source = CapabilitySource::Template;
        explanation = format!("template override declares {support} for {cap}");
        "applied template capability map".clone_into(&mut evidence);
        version_range = None;
    }

    // Source 4 — verifiable extension state raises absent to substituted.
    if support == Support::Absent
        && cap == Capability::Mcp
        && sources.adapter_verifies_mcp
        && !sources.extensions.installed_mcp_servers.is_empty()
    {
        support = Support::Substituted;
        source = CapabilitySource::Plugin;
        explanation = format!(
            "harness has no native MCP support, but installed servers provide it: {}",
            sources.extensions.installed_mcp_servers.join(", ")
        );
        "instance MCP config read fresh at query time".clone_into(&mut evidence);
    }

    // Source 5 — policy may only disable (downgrade), never upgrade.
    for row in &sources.policy {
        if !row.harness.eq_ignore_ascii_case(&sources.harness_label)
            || !row.provider.eq_ignore_ascii_case(provider_id.as_str())
            || row.capability != cap
        {
            continue;
        }
        if support_rank(row.support) < support_rank(support) {
            support = row.support;
            source = CapabilitySource::Policy;
            explanation.clone_from(&row.explanation);
            "local/admin policy override".clone_into(&mut evidence);
            version_range = None;
        }
    }

    let mut resolved =
        ResolvedCapability::new(support, source, &explanation).with_evidence(evidence);
    if let Some(range) = version_range {
        resolved = resolved.with_version_range(range);
    }
    if support == Support::Absent {
        resolved = resolved.with_limitations(
            "upgrade hint: a newer harness, a different provider/endpoint, or a verified extension may provide this capability",
        );
    }
    resolved
}

/// Resolve every catalog capability from explicit sources.
pub fn resolve_all_with_sources(
    harness: &HarnessId,
    provider_id: &ProviderId,
    sources: &CapabilitySources<'_>,
) -> Vec<(Capability, ResolvedCapability)> {
    ALL_CAPABILITIES
        .iter()
        .map(|cap| {
            (
                *cap,
                resolve_with_sources(harness, provider_id, *cap, sources),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Default-source resolution (fresh adapter + bundled provider data)
// ---------------------------------------------------------------------------

/// Gather the default sources for a pair: a fresh adapter from the harness
/// catalog and the bundled provider data. No template/plugin/policy input.
fn gather_default_sources<'a>(
    harness: &HarnessId,
    provider_id: &ProviderId,
    providers: &'a [ProviderDefinition],
) -> CapabilitySources<'a> {
    let adapter = crate::harness_catalog::all_adapters()
        .into_iter()
        .find(|a| a.id().eq_case_fold(harness));
    let provider = providers.iter().find(|p| p.id.eq_case_fold(provider_id));
    match adapter {
        Some(adapter) => CapabilitySources::for_adapter(&*adapter, provider, None),
        None => CapabilitySources {
            adapter_decls: Vec::new(),
            adapter_verifies_mcp: false,
            provider,
            template_map: None,
            extensions: ExtensionState::default(),
            policy: Vec::new(),
            harness_version: None,
            harness_label: harness.to_string(),
        },
    }
}

/// Resolve a single capability for a harness/provider pair.
///
/// Default sources: fresh adapter inspection (harness catalog) plus the
/// bundled provider data — resolution never consults compile-time tables.
/// Callers with template/plugin/policy context use
/// [`resolve_with_sources`] / [`resolve_all_with_sources`].
pub fn resolve(harness: &HarnessId, provider: &ProviderId, cap: Capability) -> ResolvedCapability {
    let bundled = crate::provider::load_bundled_providers().unwrap_or_default();
    let sources = gather_default_sources(harness, provider, &bundled);
    resolve_with_sources(harness, provider, cap, &sources)
}

/// Resolve all capabilities for a harness/provider pair (default sources).
pub fn resolve_all(
    harness: &HarnessId,
    provider: &ProviderId,
) -> Vec<(Capability, ResolvedCapability)> {
    let bundled = crate::provider::load_bundled_providers().unwrap_or_default();
    let sources = gather_default_sources(harness, provider, &bundled);
    resolve_all_with_sources(harness, provider, &sources)
}

// ---------------------------------------------------------------------------
// Instance-keyed public queries (CAP-05)
// ---------------------------------------------------------------------------

/// Inputs for the instance-level capability queries (CAP-05).
///
/// Consumers never branch on harness identity: the query takes instances and
/// resolution inputs, and harness identity stays internal diagnostic
/// metadata on the results.
#[derive(Debug)]
pub struct InstanceCapabilitySources<'a> {
    /// Known provider definitions (bundled + caller-supplied).
    pub providers: &'a [ProviderDefinition],
    /// Per-instance template capability overrides, keyed by instance id.
    pub template_overrides: &'a BTreeMap<InstanceId, BTreeMap<Capability, Support>>,
    /// Per-instance extension state, keyed by instance id.
    pub extensions: &'a BTreeMap<InstanceId, ExtensionState>,
    /// Policy rows (downgrades only).
    pub policy: Vec<FileMatrixEntry>,
}

static EMPTY_PROVIDERS: Vec<ProviderDefinition> = Vec::new();
static EMPTY_OVERRIDES: BTreeMap<InstanceId, BTreeMap<Capability, Support>> = BTreeMap::new();
static EMPTY_EXTENSIONS: BTreeMap<InstanceId, ExtensionState> = BTreeMap::new();

impl Default for InstanceCapabilitySources<'_> {
    fn default() -> Self {
        Self {
            providers: &EMPTY_PROVIDERS,
            template_overrides: &EMPTY_OVERRIDES,
            extensions: &EMPTY_EXTENSIONS,
            policy: Vec::new(),
        }
    }
}

/// Resolve all capabilities for one instance (fresh; nothing persisted).
///
/// The result carries the instance id; harness identity remains internal
/// diagnostic metadata inside each resolution's evidence.
pub fn resolve_for_instance(
    instance: &Instance,
    sources: &InstanceCapabilitySources<'_>,
) -> Vec<(Capability, ResolvedCapability)> {
    let providers = if sources.providers.is_empty() {
        crate::provider::load_bundled_providers().unwrap_or_default()
    } else {
        Vec::new()
    };
    let effective: &[ProviderDefinition] = if sources.providers.is_empty() {
        &providers
    } else {
        sources.providers
    };
    let empty_map = BTreeMap::new();
    let empty_ext = ExtensionState::default();
    let template_map = sources
        .template_overrides
        .get(&instance.id)
        .unwrap_or(&empty_map);
    let extensions = sources.extensions.get(&instance.id).unwrap_or(&empty_ext);
    let adapter = crate::harness_catalog::all_adapters()
        .into_iter()
        .find(|a| a.id().eq_case_fold(&instance.harness));
    // Without any provider data there is nothing to resolve against:
    // return the honest empty result rather than inventing a provider.
    let Some(provider) = effective.first() else {
        return Vec::new();
    };
    let cap_sources = CapabilitySources {
        adapter_decls: adapter
            .as_deref()
            .map(Adapter::capability_declarations)
            .unwrap_or_default(),
        adapter_verifies_mcp: adapter.as_deref().is_some_and(|a| a.mcp_decl().is_some()),
        provider: Some(provider),
        template_map: Some(template_map),
        extensions: extensions.clone(),
        policy: sources.policy.clone(),
        harness_version: None,
        harness_label: instance.harness.to_string(),
    };
    resolve_all_with_sources(&instance.harness, &provider.id, &cap_sources)
}

/// Filter instances by capability (CAP-05 public query).
///
/// Returns `(InstanceId, ResolvedCapability)` for every instance that
/// resolves to `capability`; when `support` is given, only instances whose
/// resolved support equals it are returned. No harness switch, no registry
/// writes, no persistence.
pub fn filter_instances_by_capability(
    instances: &[Instance],
    capability: Capability,
    support: Option<Support>,
    sources: &InstanceCapabilitySources<'_>,
) -> Vec<(InstanceId, ResolvedCapability)> {
    let mut out = Vec::new();
    for instance in instances {
        let resolved = resolve_for_instance(instance, sources)
            .into_iter()
            .find(|(cap, _)| *cap == capability)
            .map(|(_, resolved)| resolved);
        if let Some(resolved) = resolved
            && support.is_none_or(|wanted| resolved.support == wanted)
        {
            out.push((instance.id.clone(), resolved));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Completeness validation (CAP-04)
// ---------------------------------------------------------------------------

/// Validate that the given sources resolve EVERY catalog capability for the
/// pair, with no duplicate/conflicting rules (CAP-04).
///
/// Called from template validation: an incomplete matrix blocks template
/// publication/use instead of defaulting to absent.
pub fn validate_resolution_completeness(
    harness: &HarnessId,
    provider_id: &ProviderId,
    sources: &CapabilitySources<'_>,
) -> Result<Vec<(Capability, ResolvedCapability)>> {
    let mut seen_decls: Vec<Capability> = Vec::new();
    for decl in &sources.adapter_decls {
        if seen_decls.contains(&decl.capability) {
            return Err(CoreError::Validation {
                field: "capability_matrix".to_owned(),
                reason: format!(
                    "duplicate adapter capability declaration for `{}` on harness `{harness}`",
                    decl.capability
                ),
            });
        }
        seen_decls.push(decl.capability);
    }
    let resolved = resolve_all_with_sources(harness, provider_id, sources);
    let mut missing = Vec::new();
    for (cap, res) in &resolved {
        if res.source == CapabilitySource::Unknown {
            missing.push(cap.to_string());
        }
    }
    if !missing.is_empty() {
        return Err(CoreError::Validation {
            field: "capability_matrix".to_owned(),
            reason: format!(
                "incomplete capability coverage for harness `{harness}` provider `{provider_id}`: {} do not resolve from adapter/provider/template data",
                missing.join(", ")
            ),
        });
    }
    Ok(resolved)
}

// ---------------------------------------------------------------------------
// Legacy static matrix — reference data for invariant tests ONLY
// ---------------------------------------------------------------------------

/// One row in the static harness/provider/capability matrix.
///
/// The static matrix is RETAINED AS REFERENCE DATA for the completeness
/// invariant tests; the live resolution path ([`resolve`],
/// [`resolve_with_sources`]) never consults it.
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

/// Static harness/provider/capability matrix — REFERENCE DATA ONLY.
///
/// Retained for the invariant tests (`validate_matrix_completeness`) and as
/// the documented expectation table for the reference scenarios. The
/// resolution path feeds on adapter/provider/template/plugin/policy data;
/// it never reads this const.
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
        source: CapabilitySource::Provider,
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
        source: CapabilitySource::Provider,
        explanation: "Claude Code computer_use absent on OpenAI (no computer-use API)",
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
        support: Support::Substituted,
        source: CapabilitySource::Provider,
        explanation: "Codex CLI web_search substituted via OpenAI server-side search",
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
        source: CapabilitySource::Provider,
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
        support: Support::Substituted,
        source: CapabilitySource::Provider,
        explanation: "Codex CLI vision substituted via Anthropic OpenAI-compatible endpoint",
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
        source: CapabilitySource::Provider,
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

/// Resolve using an explicit matrix slice (legacy path for tests and the
/// invariant fixtures; the live resolver is [`resolve_with_sources`]).
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
                evidence: None,
                version_range: None,
                limitations: None,
            };
        }
    }
    ResolvedCapability {
        support: Support::Absent,
        source: CapabilitySource::Unknown,
        explanation: format!(
            "no matrix entry for harness `{harness}` provider `{provider}` capability `{cap:?}`"
        ),
        evidence: None,
        version_range: None,
        limitations: None,
    }
}

/// Resolve all using an explicit matrix (legacy path).
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
// File-driven policy rows (CAP-03 source 5)
// ---------------------------------------------------------------------------

/// File-driven matrix row (JSON/YAML deserializable). Doubles as the policy
/// override input for [`CapabilitySources`].
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

/// Load policy rows from JSON or YAML (CAP-03 source 5: local/admin policy).
///
/// Accepts a single file containing an array of rows, or a single row.
/// JSON detected by `.json` extension, YAML by `.yaml`/`.yml`, fallback tries both.
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
            path: Path::to_path_buf(path),
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
        assert_eq!(native.source, CapabilitySource::Harness);
        assert_eq!(substituted.support, Support::Substituted);
        assert_eq!(substituted.source, CapabilitySource::Provider);
        assert_ne!(native.support, substituted.support);
        assert_ne!(native.explanation, substituted.explanation);
    }

    #[test]
    fn provider_cannot_override_incompatible_harness_transport() {
        // GLM vision for claude-code is absent even though a vision model
        // exists in the catalog; and synthetic: a provider native claim
        // cannot pass an absent transport declaration.
        let vision = resolve(&hid("claude-code"), &pid("glm"), Capability::Vision);
        assert_eq!(
            vision.support,
            Support::Absent,
            "harness transport incompatibility wins"
        );
        assert!(vision.limitations.is_some());

        // Synthetic adapter with an absent transport + provider claiming native.
        let adapter = NoTransportAdapter;
        let mut provider = ProviderDefinition::new(pid("any-prov"), "https://api.example.com");
        provider.capabilities = vec![crate::provider::ProviderCapabilityDecl {
            capability: Capability::Vision,
            support: Support::Native,
            explanation: "provider claims vision".to_owned(),
            limitations: None,
        }];
        let sources = CapabilitySources::for_adapter(&adapter, Some(&provider), None);
        let resolved = resolve_with_sources(
            &hid("no-transport"),
            &pid("any-prov"),
            Capability::Vision,
            &sources,
        );
        assert_eq!(resolved.support, Support::Absent);
        assert_eq!(resolved.source, CapabilitySource::Harness);
    }

    #[test]
    fn pi_mcp_absent_natively_but_extension_substitutes() {
        let mcp = resolve(&hid("pi"), &pid("anthropic"), Capability::Mcp);
        assert_eq!(mcp.support, Support::Absent);
        assert_eq!(mcp.source, CapabilitySource::Harness);

        // With a verified MCP declaration + installed servers, absent rises
        // to substituted via the plugin source.
        let adapter = McpDeclAdapter;
        let sources =
            CapabilitySources::for_adapter(&adapter, None, None).with_extensions(ExtensionState {
                installed_mcp_servers: vec!["context7".to_owned()],
                enabled_plugins: vec![],
            });
        let resolved = resolve_with_sources(
            &hid("mcp-decl"),
            &pid("anthropic"),
            Capability::Mcp,
            &sources,
        );
        assert_eq!(resolved.support, Support::Substituted);
        assert_eq!(resolved.source, CapabilitySource::Plugin);
        assert!(resolved.explanation.contains("context7"));
    }

    #[test]
    fn policy_disables_native_but_never_upgrades() {
        let adapter = NativeAdapter;
        let provider = ProviderDefinition::new(pid("prov"), "https://api.example.com");
        let policy = vec![FileMatrixEntry {
            harness: "native-h".to_owned(),
            provider: "prov".to_owned(),
            capability: Capability::WebSearch,
            support: Support::Absent,
            source: Some(CapabilitySource::Policy),
            explanation: "disabled by admin policy".to_owned(),
        }];
        let sources =
            CapabilitySources::for_adapter(&adapter, Some(&provider), None).with_policy(policy);
        let disabled = resolve_with_sources(
            &hid("native-h"),
            &pid("prov"),
            Capability::WebSearch,
            &sources,
        );
        assert_eq!(disabled.support, Support::Absent);
        assert_eq!(disabled.source, CapabilitySource::Policy);

        // An upgrading policy row (absent -> native) is ignored.
        let upgrade = vec![FileMatrixEntry {
            harness: "no-transport".to_owned(),
            provider: "prov".to_owned(),
            capability: Capability::WebSearch,
            support: Support::Native,
            source: Some(CapabilitySource::Policy),
            explanation: "policy cannot grant transport".to_owned(),
        }];
        let no_transport = NoTransportAdapter;
        let sources = CapabilitySources::for_adapter(&no_transport, Some(&provider), None)
            .with_policy(upgrade);
        let resolved = resolve_with_sources(
            &hid("no-transport"),
            &pid("prov"),
            Capability::WebSearch,
            &sources,
        );
        assert_eq!(resolved.support, Support::Absent);
        assert_eq!(resolved.source, CapabilitySource::Harness);
    }

    #[test]
    fn native_claim_respects_harness_version_requirement() {
        let adapter = NativeAdapter;
        let provider = ProviderDefinition::new(pid("prov"), "https://api.example.com");
        let sources = CapabilitySources::for_adapter(&adapter, Some(&provider), None)
            .with_harness_version("0.1.0");
        let resolved =
            resolve_with_sources(&hid("native-h"), &pid("prov"), Capability::Mcp, &sources);
        // NativeAdapter requires >=2.0.0 for MCP; 0.1.0 does not match.
        assert_eq!(resolved.support, Support::Absent);
        assert!(
            resolved.explanation.contains("requires harness >=2.0.0"),
            "got: {}",
            resolved.explanation
        );
        assert_eq!(resolved.version_range.as_deref(), Some(">=2.0.0"));
    }

    #[test]
    fn template_override_wins_over_provider() {
        let adapter = NativeAdapter;
        let mut provider = ProviderDefinition::new(pid("prov"), "https://api.example.com");
        provider.capabilities = vec![crate::provider::ProviderCapabilityDecl {
            capability: Capability::WebSearch,
            support: Support::Substituted,
            explanation: "provider-side search".to_owned(),
            limitations: None,
        }];
        let mut map = BTreeMap::new();
        map.insert(Capability::WebSearch, Support::Native);
        let sources = CapabilitySources::for_adapter(&adapter, Some(&provider), Some(&map));
        let resolved = resolve_with_sources(
            &hid("native-h"),
            &pid("prov"),
            Capability::WebSearch,
            &sources,
        );
        assert_eq!(resolved.support, Support::Native);
        assert_eq!(resolved.source, CapabilitySource::Template);
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
    fn resolved_entries_carry_evidence_and_limitations() {
        let all = resolve_all(&hid("claude-code"), &pid("anthropic"));
        for (_, res) in &all {
            assert!(
                res.evidence.is_some(),
                "native/substituted results carry evidence: {res:?}"
            );
        }
        let glm_vision = resolve(&hid("claude-code"), &pid("glm"), Capability::Vision);
        assert!(glm_vision.limitations.is_some());
        assert!(glm_vision.explanation.contains("vision"));
    }

    #[test]
    fn completeness_validation_blocks_incomplete_coverage() {
        // claude-code declares all four capabilities, so the pair resolves.
        let bundled = crate::provider::load_bundled_providers().unwrap();
        let anthropic = bundled
            .iter()
            .find(|p| p.id.as_str() == "anthropic")
            .unwrap();
        let sources = gather_default_sources(&hid("claude-code"), &anthropic.id, &bundled);
        validate_resolution_completeness(&hid("claude-code"), &anthropic.id, &sources).unwrap();

        // An adapter declaring only one capability cannot cover the catalog.
        let partial = CapabilitySources {
            adapter_decls: vec![AdapterCapabilityDecl::new(
                Capability::WebSearch,
                Support::Native,
                "only one",
            )],
            adapter_verifies_mcp: false,
            provider: Some(anthropic),
            template_map: None,
            extensions: ExtensionState::default(),
            policy: Vec::new(),
            harness_version: None,
            harness_label: "partial-h".to_owned(),
        };
        let err = validate_resolution_completeness(&hid("partial-h"), &anthropic.id, &partial)
            .unwrap_err()
            .to_string();
        assert!(err.contains("do not resolve"), "got: {err}");
    }

    #[test]
    fn duplicate_adapter_declarations_rejected() {
        let sources = CapabilitySources {
            adapter_decls: vec![
                AdapterCapabilityDecl::new(Capability::WebSearch, Support::Native, "a"),
                AdapterCapabilityDecl::new(Capability::WebSearch, Support::Native, "b"),
            ],
            adapter_verifies_mcp: false,
            provider: None,
            template_map: None,
            extensions: ExtensionState::default(),
            policy: Vec::new(),
            harness_version: None,
            harness_label: "dup-h".to_owned(),
        };
        let err = validate_resolution_completeness(&hid("dup-h"), &pid("p"), &sources)
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate"), "got: {err}");
    }

    #[test]
    fn file_driven_policy_rows_load() {
        let dir = crate::test_util::temp_dir_unique("cap-policy");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("policy.json");
        std::fs::write(
            &path,
            r#"[{
                "harness": "claude-code",
                "provider": "anthropic",
                "capability": "web_search",
                "support": "absent",
                "explanation": "disabled by policy"
            }]"#,
        )
        .unwrap();
        let rows = load_matrix_from_file(&path).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].capability, Capability::WebSearch);
        // Unknown capability in a policy file fails typed, never silently absent.
        std::fs::write(
            &path,
            r#"[{
                "harness": "claude-code",
                "provider": "anthropic",
                "capability": "telepathy",
                "support": "absent",
                "explanation": "typo"
            }]"#,
        )
        .unwrap();
        load_matrix_from_file(&path).unwrap_err();
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn instance_query_filters_without_harness_identity() {
        let make_instance = |harness: &str, name: &str| Instance {
            id: InstanceId::new(&format!("id-{name}")).unwrap(),
            name: crate::ids::InstanceName::new(name).unwrap(),
            harness: hid(harness),
            config_root: crate::paths::AbsolutePath::from_path(
                &std::env::temp_dir().join(format!("superai-cap-{name}")),
            )
            .unwrap(),
            binary: None,
            wrapper: None,
            isolation: crate::state::Isolation::RelocatedRoot,
            origin: crate::state::InstanceOrigin::Created,
            ownership: crate::state::Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        };
        let claude = make_instance("claude-code", "claude-inst");
        let aider = make_instance("aider", "aider-inst");
        let instances = vec![claude, aider];
        let sources = InstanceCapabilitySources::default();
        let with_mcp = filter_instances_by_capability(
            &instances,
            Capability::Mcp,
            Some(Support::Native),
            &sources,
        );
        assert_eq!(with_mcp.len(), 1);
        assert_eq!(
            with_mcp[0].0.as_str(),
            "id-claude-inst",
            "harness identity never appears in the query API"
        );
        let none = filter_instances_by_capability(
            &instances,
            Capability::Mcp,
            Some(Support::Substituted),
            &sources,
        );
        assert!(none.is_empty());
    }

    #[test]
    fn extension_state_reads_mcp_destination_fresh() {
        let dir = crate::test_util::temp_dir_unique("cap-ext");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mcp.json"),
            r#"{"mcpServers": {"context7": {"command": "npx"}}, "other": 1}"#,
        )
        .unwrap();
        let instance = Instance {
            id: InstanceId::new("id-ext").unwrap(),
            name: crate::ids::InstanceName::new("ext").unwrap(),
            harness: hid("mcp-decl"),
            config_root: crate::paths::AbsolutePath::from_path(&dir).unwrap(),
            binary: None,
            wrapper: None,
            isolation: crate::state::Isolation::RelocatedRoot,
            origin: crate::state::InstanceOrigin::Created,
            ownership: crate::state::Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        };
        let state = ExtensionState::from_instance(&McpDeclAdapter, &instance);
        assert_eq!(state.installed_mcp_servers, vec!["context7".to_owned()]);
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn parse_capability_id_available_through_resolver() {
        assert_eq!(
            parse_capability_id("computer-use").unwrap(),
            Capability::ComputerUse
        );
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
        // Template update preview: GLM vision absent vs Anthropic native — delta is visible.
        let before = resolve(&hid("claude-code"), &pid("glm"), Capability::Vision);
        let after = resolve(&hid("claude-code"), &pid("anthropic"), Capability::Vision);
        assert_ne!(before.support, after.support);
        assert_eq!(before.support, Support::Absent);
        assert_eq!(after.support, Support::Native);
    }

    #[test]
    fn matrix_has_native_substituted_absent_with_explanations() {
        validate_matrix_completeness().unwrap();
        let mut seen_native = false;
        let mut seen_substituted = false;
        let mut seen_absent = false;
        for entry in MATRIX {
            assert!(
                !entry.explanation.trim().is_empty(),
                "matrix entry harness `{}` provider `{}` cap `{:?}` has empty explanation",
                entry.harness,
                entry.provider,
                entry.capability
            );
            match entry.support {
                Support::Native => seen_native = true,
                Support::Substituted => seen_substituted = true,
                Support::Absent => seen_absent = true,
            }
        }
        assert!(seen_native, "matrix must contain at least one native");
        assert!(
            seen_substituted,
            "matrix must contain at least one substituted"
        );
        assert!(seen_absent, "matrix must contain at least one absent");
        for (harness, provider) in ACTIVE_PAIRS {
            let hid_v = HarnessId::new(harness).unwrap();
            let pid_v = ProviderId::new(provider).unwrap();
            let all = resolve_all(&hid_v, &pid_v);
            assert_eq!(
                all.len(),
                ALL_CAPABILITIES.len(),
                "pair {harness}/{provider} missing capabilities"
            );
            for (cap, resolved) in all {
                assert!(
                    !resolved.explanation.trim().is_empty(),
                    "pair {harness}/{provider} cap {cap:?} has empty explanation"
                );
            }
        }
    }

    #[test]
    fn add_provider_without_code_change_and_matrix_completeness() {
        // Provider side: adding a provider via file requires no Rust edit.
        let dir = crate::test_util::temp_dir_unique("matrix-provider-polish");
        std::fs::create_dir_all(&dir).unwrap();
        let provider_json = r#"{
  "id": "synthetic-matrix-provider",
  "display_name": "Synthetic",
  "base_url": "https://synthetic.example.com",
  "auth_style": "bearer",
  "protocol": "openai_chat",
  "model_list": [{"id": "m1", "status": "active"}],
  "defaults": {"default_model": "m1"},
  "status": "active"
}"#;
        let path = dir.join("synthetic-matrix-provider.json");
        std::fs::write(&path, provider_json).unwrap();
        let loaded = crate::provider::load_provider_defs(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id.as_str(), "synthetic-matrix-provider");

        // The synthetic pair has no adapter declaration -> Unknown (honest),
        // and completeness validation blocks it rather than defaulting absent.
        let synthetic_h = HarnessId::new("synthetic-harness").unwrap();
        let synthetic_p = loaded[0].id.clone();
        let unknown = resolve(&synthetic_h, &synthetic_p, Capability::WebSearch);
        assert_eq!(unknown.support, Support::Absent);
        assert_eq!(unknown.source, CapabilitySource::Unknown);

        let sources = gather_default_sources(&synthetic_h, &synthetic_p, &loaded);
        let err = validate_resolution_completeness(&synthetic_h, &synthetic_p, &sources)
            .unwrap_err()
            .to_string();
        assert!(err.contains("do not resolve"), "got: {err}");

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn legacy_matrix_path_still_resolves_explicit_rows() {
        let synthetic_matrix = [MatrixEntry {
            harness: "synthetic-harness",
            provider: "synthetic-matrix-provider",
            capability: Capability::WebSearch,
            support: Support::Substituted,
            source: CapabilitySource::Provider,
            explanation: "synthetic substituted for test",
        }];
        let r = resolve_with_matrix(
            &hid("synthetic-harness"),
            &pid("synthetic-matrix-provider"),
            Capability::WebSearch,
            &synthetic_matrix,
        );
        assert_eq!(r.support, Support::Substituted);
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
    fn file_driven_matrix_data_only() {
        let dir = crate::test_util::temp_dir_unique("cap");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("matrix.json");
        std::fs::write(&path, "[]").unwrap();
        let rows = load_matrix_from_file(&path).unwrap();
        assert!(rows.is_empty());
        drop(std::fs::remove_dir_all(&dir));
    }

    // --- local adapters for the resolution tests ---

    /// Declares NO capability transport at all.
    #[derive(Debug)]
    struct NoTransportAdapter;

    impl Adapter for NoTransportAdapter {
        fn id(&self) -> HarnessId {
            hid("no-transport")
        }
        fn display_name(&self) -> &'static str {
            "No Transport"
        }
        fn product_status(&self) -> crate::adapter::ProductStatus {
            crate::adapter::ProductStatus::Active
        }
        fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
            Vec::new()
        }
        fn adapter_revision(&self) -> &'static str {
            "0.1.0"
        }
        fn research_doc_link(&self) -> &'static str {
            "docs/harness-configs/no-transport.md"
        }
        fn last_verified_date(&self) -> &'static str {
            "2026-08-25"
        }
        fn detection(&self) -> crate::adapter::DetectionResult {
            crate::adapter::DetectionResult::absent(vec!["test".to_owned()])
        }
        fn version_resolution(&self) -> crate::adapter::VersionResolution {
            crate::adapter::VersionResolution::unknown()
        }
        fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
            Vec::new()
        }
        fn supported_operations(&self) -> Vec<(String, crate::state::AdapterSupport)> {
            Vec::new()
        }
        fn plan_mirror_exclusions(&self) -> Vec<String> {
            Vec::new()
        }
        fn plan_wrapper(&self, _instance: &Instance) -> Result<crate::adapter::WrapperPlan> {
            Ok(crate::adapter::WrapperPlan::new("test"))
        }
        fn scan_candidates(&self) -> Vec<String> {
            Vec::new()
        }
        fn validate_instance(&self, _instance: &Instance) -> Result<()> {
            Ok(())
        }
        fn capability_declarations(&self) -> Vec<AdapterCapabilityDecl> {
            vec![
                AdapterCapabilityDecl::new(
                    Capability::WebSearch,
                    Support::Absent,
                    "no web-search transport in this harness",
                ),
                AdapterCapabilityDecl::new(
                    Capability::Vision,
                    Support::Absent,
                    "no image transport in this harness",
                ),
                AdapterCapabilityDecl::new(
                    Capability::ComputerUse,
                    Support::Absent,
                    "no computer-use transport in this harness",
                ),
                AdapterCapabilityDecl::new(
                    Capability::Mcp,
                    Support::Absent,
                    "no MCP transport in this harness",
                ),
            ]
        }
    }

    /// Native transport for everything; MCP gated on harness >=2.0.0.
    #[derive(Debug)]
    struct NativeAdapter;

    impl Adapter for NativeAdapter {
        fn id(&self) -> HarnessId {
            hid("native-h")
        }
        fn display_name(&self) -> &'static str {
            "Native"
        }
        fn product_status(&self) -> crate::adapter::ProductStatus {
            crate::adapter::ProductStatus::Active
        }
        fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
            Vec::new()
        }
        fn adapter_revision(&self) -> &'static str {
            "0.1.0"
        }
        fn research_doc_link(&self) -> &'static str {
            "docs/harness-configs/native.md"
        }
        fn last_verified_date(&self) -> &'static str {
            "2026-08-25"
        }
        fn detection(&self) -> crate::adapter::DetectionResult {
            crate::adapter::DetectionResult::absent(vec!["test".to_owned()])
        }
        fn version_resolution(&self) -> crate::adapter::VersionResolution {
            crate::adapter::VersionResolution::unknown()
        }
        fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
            Vec::new()
        }
        fn supported_operations(&self) -> Vec<(String, crate::state::AdapterSupport)> {
            Vec::new()
        }
        fn plan_mirror_exclusions(&self) -> Vec<String> {
            Vec::new()
        }
        fn plan_wrapper(&self, _instance: &Instance) -> Result<crate::adapter::WrapperPlan> {
            Ok(crate::adapter::WrapperPlan::new("test"))
        }
        fn scan_candidates(&self) -> Vec<String> {
            Vec::new()
        }
        fn validate_instance(&self, _instance: &Instance) -> Result<()> {
            Ok(())
        }
        fn capability_declarations(&self) -> Vec<AdapterCapabilityDecl> {
            vec![
                AdapterCapabilityDecl::new(
                    Capability::WebSearch,
                    Support::Native,
                    "client-side web search tool",
                ),
                AdapterCapabilityDecl::new(Capability::Vision, Support::Native, "image input"),
                AdapterCapabilityDecl::new(Capability::Mcp, Support::Native, "MCP transport")
                    .with_version_req(">=2.0.0"),
                AdapterCapabilityDecl::new(
                    Capability::ComputerUse,
                    Support::Native,
                    "computer use loop",
                ),
            ]
        }
    }

    /// No native MCP support, but declares an MCP destination it can verify.
    #[derive(Debug)]
    struct McpDeclAdapter;

    impl Adapter for McpDeclAdapter {
        fn id(&self) -> HarnessId {
            hid("mcp-decl")
        }
        fn display_name(&self) -> &'static str {
            "MCP Decl"
        }
        fn product_status(&self) -> crate::adapter::ProductStatus {
            crate::adapter::ProductStatus::Active
        }
        fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
            Vec::new()
        }
        fn adapter_revision(&self) -> &'static str {
            "0.1.0"
        }
        fn research_doc_link(&self) -> &'static str {
            "docs/harness-configs/mcp-decl.md"
        }
        fn last_verified_date(&self) -> &'static str {
            "2026-08-25"
        }
        fn detection(&self) -> crate::adapter::DetectionResult {
            crate::adapter::DetectionResult::absent(vec!["test".to_owned()])
        }
        fn version_resolution(&self) -> crate::adapter::VersionResolution {
            crate::adapter::VersionResolution::unknown()
        }
        fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
            Vec::new()
        }
        fn supported_operations(&self) -> Vec<(String, crate::state::AdapterSupport)> {
            Vec::new()
        }
        fn plan_mirror_exclusions(&self) -> Vec<String> {
            Vec::new()
        }
        fn plan_wrapper(&self, _instance: &Instance) -> Result<crate::adapter::WrapperPlan> {
            Ok(crate::adapter::WrapperPlan::new("test"))
        }
        fn scan_candidates(&self) -> Vec<String> {
            Vec::new()
        }
        fn validate_instance(&self, _instance: &Instance) -> Result<()> {
            Ok(())
        }
        fn capability_declarations(&self) -> Vec<AdapterCapabilityDecl> {
            vec![
                AdapterCapabilityDecl::new(Capability::WebSearch, Support::Native, "search tool"),
                AdapterCapabilityDecl::new(Capability::Vision, Support::Native, "image input"),
                AdapterCapabilityDecl::new(
                    Capability::ComputerUse,
                    Support::Absent,
                    "no computer use loop",
                ),
                AdapterCapabilityDecl::new(Capability::Mcp, Support::Absent, "no native MCP"),
            ]
        }
        fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
            Some(crate::adapter::McpAdapterDecl::new(
                "mcp.json",
                "mcpServers",
                crate::adapter::DocumentKind::Json,
                crate::adapter::ConfigScope::User,
                crate::adapter::RestartBehavior::Reload,
            ))
        }
    }
}
