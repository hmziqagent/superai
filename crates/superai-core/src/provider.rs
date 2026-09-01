//! Provider definitions — data-driven, no hardcoded provider list.
//!
//! A provider is versioned data, not a Rust branch. Adding a provider is a
//! data-only change: add a JSON/YAML file and no Rust source edit is required.
//! Definitions are read fresh from disk on every load; nothing is cached.
//! Health probing validates URL format, bounds timeout, redacts secrets,
//! classifies auth/rate-limit/TLS via the fake harness (no live network),
//! and strips auth on cross-host redirects. API keys are ephemeral and only
//! written to harness-declared sinks.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::adapter::{Adapter, DocumentKind, SurfaceOwnership};
use crate::capability::{Capability, Support};
use crate::error::{CoreError, RedactedString, Result};
use crate::ids::ProviderId;
use crate::instance::Instance;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// How the harness authenticates to the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthStyle {
    /// Bearer token `Authorization: Bearer <key>`.
    #[default]
    Bearer,
    /// `x-api-key` or provider-specific header.
    ApiKeyHeader,
    /// Anthropic-style `x-api-key` header.
    XApiKey,
    /// API key in query parameter.
    QueryParam,
    /// No authentication (local provider).
    None,
    /// Unrecognised style preserved as string (round-trips verbatim).
    #[serde(other)]
    Unknown,
}

/// Wire protocol / API variant the provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// `OpenAI` chat/completions.
    #[default]
    #[serde(alias = "openai_chat", alias = "openai-compat")]
    OpenAiChat,
    /// `OpenAI` responses.
    #[serde(alias = "openai_responses")]
    OpenAiResponses,
    /// Anthropic messages.
    Anthropic,
    /// Gemini.
    Gemini,
    /// Vendor-specific identifier preserved via `Other` (serde other).
    #[serde(other)]
    Other,
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::OpenAiChat => "openai_chat",
            Self::OpenAiResponses => "openai_responses",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Other => "other",
        };
        f.write_str(s)
    }
}

/// Lifecycle status of a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    /// Generally available.
    #[default]
    Active,
    /// Preview / beta.
    Preview,
    /// Deprecated but still available.
    Deprecated,
    /// Retired — must not be used as default.
    Retired,
}

impl std::fmt::Display for ModelStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Active => "active",
            Self::Preview => "preview",
            Self::Deprecated => "deprecated",
            Self::Retired => "retired",
        };
        f.write_str(s)
    }
}

/// Lifecycle status of a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    /// Active and recommended.
    #[default]
    Active,
    /// Preview / beta.
    Preview,
    /// Deprecated with replacement.
    Deprecated,
    /// Retired / archived.
    Retired,
}

// ---------------------------------------------------------------------------
// Modalities and model limits (PRV-02)
// ---------------------------------------------------------------------------

/// Input/output modality of a model (PRV-02).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    /// Plain text in/out.
    Text,
    /// Images in/out.
    Image,
    /// Audio in/out.
    Audio,
    /// Video in/out.
    Video,
}

impl std::fmt::Display for Modality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
        };
        f.write_str(s)
    }
}

/// Token limits of a model (PRV-02: context/input/output limits).
///
/// All fields are optional — providers document different subsets. Validation
/// requires every present limit to be positive and consistent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelLimits {
    /// Total context window in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// Maximum input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Maximum output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

// ---------------------------------------------------------------------------
// Model and defaults
// ---------------------------------------------------------------------------

/// One model entry in the provider catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Stable provider-local model identifier, e.g. `glm-4.5`.
    pub id: String,
    /// Human display name.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Lifecycle status.
    #[serde(default)]
    pub status: ModelStatus,
    /// Optional harness alias (e.g. Codex alias for the same model).
    #[serde(default)]
    pub alias: Option<String>,
    /// Whether this model may be used for health probing.
    #[serde(default = "default_true")]
    pub health_eligible: bool,
    /// Context/input/output token limits (PRV-02).
    #[serde(default)]
    pub limits: ModelLimits,
    /// Input modalities (empty = unspecified).
    #[serde(default)]
    pub input_modalities: Vec<Modality>,
    /// Output modalities (empty = unspecified).
    #[serde(default)]
    pub output_modalities: Vec<Modality>,
    /// Whether the model accepts tool/function calls.
    #[serde(default)]
    pub supports_tools: bool,
    /// Whether the model exposes a reasoning mode.
    #[serde(default)]
    pub supports_reasoning: bool,
}

fn default_true() -> bool {
    true
}

/// Defaults for a provider — which model to use when the harness needs one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderDefaults {
    /// Default model identifier.
    #[serde(default)]
    pub default_model: Option<String>,
}

// ---------------------------------------------------------------------------
// Endpoint variants, auth inputs, probes, capabilities (PRV-01/PRV-06)
// ---------------------------------------------------------------------------

/// Base-endpoint variant keyed by region or plan (PRV-01).
///
/// When a variant is requested but absent, resolution falls back to the
/// provider's default `base_url`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointVariant {
    /// Variant key, e.g. `us`, `eu`, `free-tier`, `anthropic-compat`.
    pub name: String,
    /// Base URL for this variant.
    pub base_url: String,
    /// Protocols this variant serves; empty means the provider default
    /// protocol applies.
    #[serde(default)]
    pub protocols: Vec<Protocol>,
}

/// Auth input declarations (PRV-01: env variable names + placeholder policy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AuthInputs {
    /// Environment variable names the harness may read the key from, in
    /// preference order (names only — never values).
    #[serde(default)]
    pub env_var_names: Vec<String>,
    /// Config field name the harness uses for the key, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_field_name: Option<String>,
    /// Documented placeholder shown in previews, e.g. `sk-...`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_placeholder: Option<String>,
    /// Documented key prefix policy (PRV-04): when present, a provided key
    /// must start with this prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_prefix: Option<String>,
}

/// Health probe definition carried in provider data (PRV-06).
///
/// Pure data: kind, URL derivation from the base endpoint, method, header and
/// body templates (with placeholders, never secrets), auth reference, bounds,
/// accepted-status/body predicates, TLS/private-network policy, and a
/// rate/cost warning. Execution lives in [`crate::health`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeDefinition {
    /// Stable probe identifier, unique within the provider.
    pub id: String,
    /// Which endpoint the probe hits.
    pub kind: crate::health::HealthProbeKind,
    /// Path appended to the (possibly variant) base URL, e.g. `/v1/models`.
    /// Must start with `/`; required for HTTP-ish kinds.
    #[serde(default)]
    pub path_suffix: String,
    /// HTTP method; `None` means `GET`. Only `GET`/`HEAD`/`POST` allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Header templates. Values may reference `${ENV:VAR}` placeholders;
    /// never literal secrets.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Optional body template (same placeholder rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_template: Option<String>,
    /// Whether the probe must carry provider auth (references the provider's
    /// auth style; the secret itself is supplied per operation, never stored).
    #[serde(default)]
    pub uses_auth: bool,
    /// Timeout in milliseconds; `None` uses the executor default. Bounded
    /// 1000..=30000 by validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Response size cap; `None` uses the executor default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_response_bytes: Option<usize>,
    /// Accepted HTTP status codes (the "accepted status predicate").
    pub accepted_status: Vec<u16>,
    /// Optional accepted-body predicate: a substring that must appear in a
    /// successful response body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_contains: Option<String>,
    /// Whether this probe is allowed against private/loopback endpoints
    /// (local provider intent).
    #[serde(default)]
    pub allow_private_network: bool,
    /// Rate/cost warning shown before the probe runs (e.g. minimal
    /// authenticated requests may bill).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_cost_warning: Option<String>,
}

/// Server-side/modal capability contribution of a provider (plan 09 CAP-03
/// source 2). The provider declares what it can satisfy; the harness
/// transport constraint still gates the final resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilityDecl {
    /// Capability this declaration concerns.
    pub capability: Capability,
    /// Support the provider can provide for it.
    pub support: Support,
    /// Concise explanation (evidence).
    pub explanation: String,
    /// Known limitations, when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limitations: Option<String>,
}

// ---------------------------------------------------------------------------
// ProviderDefinition
// ---------------------------------------------------------------------------

/// Current provider definition schema version (PRV-01: versioned data).
pub const PROVIDER_SCHEMA_VERSION: u32 = 1;

fn default_provider_schema_version() -> u32 {
    PROVIDER_SCHEMA_VERSION
}

/// Provider definition as stored in a JSON/YAML data file.
///
/// No secret values are stored here. Adding a provider means adding a file,
/// not editing Rust. Fields not modelled survive via serde's ignore on write
/// but are not invented — unknown keys are ignored on read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderDefinition {
    /// Stable provider identifier.
    pub id: ProviderId,
    /// Human display name.
    #[serde(default)]
    pub display_name: String,
    /// Base URL for the provider API, e.g. `https://api.anthropic.com`.
    pub base_url: String,
    /// Authentication style.
    #[serde(default)]
    pub auth_style: AuthStyle,
    /// Protocol spoken by the provider.
    #[serde(default)]
    pub protocol: Protocol,
    /// Model catalog.
    #[serde(default, alias = "model_list", alias = "models")]
    pub model_list: Vec<ModelInfo>,
    /// Defaults (default model etc.).
    #[serde(default)]
    pub defaults: ProviderDefaults,
    /// Lifecycle status.
    #[serde(default)]
    pub status: ProviderStatus,
    /// Optional documentation URL.
    #[serde(default)]
    pub documentation_url: Option<String>,
    /// Schema version of this definition (PRV-01).
    #[serde(default = "default_provider_schema_version")]
    pub schema_version: u32,
    /// Base-endpoint variants by region/plan (PRV-01).
    #[serde(default)]
    pub endpoints: Vec<EndpointVariant>,
    /// Required request headers (name -> template value).
    #[serde(default)]
    pub required_headers: BTreeMap<String, String>,
    /// Optional request headers (name -> template value).
    #[serde(default)]
    pub optional_headers: BTreeMap<String, String>,
    /// Request parameters applied to every request (never secrets).
    #[serde(default)]
    pub request_params: BTreeMap<String, Value>,
    /// Auth inputs: env var names + placeholder/prefix policy (PRV-01/04).
    #[serde(default)]
    pub auth: AuthInputs,
    /// References to capability contributions documented elsewhere
    /// (e.g. `docs/capabilities.md#glm-web-search`).
    #[serde(default)]
    pub capability_refs: Vec<String>,
    /// Structured capability contributions (plan 09 source 2).
    #[serde(default)]
    pub capabilities: Vec<ProviderCapabilityDecl>,
    /// Health probe definitions (PRV-06).
    #[serde(default)]
    pub health_probes: Vec<ProbeDefinition>,
    /// Date the definition was last verified against vendor docs,
    /// `YYYY-MM-DD` (PRV-01).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
}

impl ProviderDefinition {
    /// Construct a minimal valid definition (all optional fields defaulted).
    ///
    /// Useful for tests and as the base of struct-update expressions; the
    /// result still needs `model_list`/`defaults` populated to be meaningful.
    pub fn new(id: ProviderId, base_url: impl Into<String>) -> Self {
        Self {
            id,
            display_name: String::new(),
            base_url: base_url.into(),
            auth_style: AuthStyle::default(),
            protocol: Protocol::default(),
            model_list: Vec::new(),
            defaults: ProviderDefaults::default(),
            status: ProviderStatus::default(),
            documentation_url: None,
            schema_version: PROVIDER_SCHEMA_VERSION,
            endpoints: Vec::new(),
            required_headers: BTreeMap::new(),
            optional_headers: BTreeMap::new(),
            request_params: BTreeMap::new(),
            auth: AuthInputs::default(),
            capability_refs: Vec::new(),
            capabilities: Vec::new(),
            health_probes: Vec::new(),
            verified_at: None,
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "validation covers the full PRV-01/02 field set"
    )]
    #[expect(clippy::excessive_nesting, reason = "field-set validation branches")]
    /// Validate the definition before use.
    ///
    /// Checks (PRV-01/02):
    /// - `schema_version` equals [`PROVIDER_SCHEMA_VERSION`]
    /// - `base_url` and every endpoint variant non-empty and syntactically
    ///   valid (no network), no duplicate normalized endpoint
    /// - unique endpoint variant names
    /// - unique model IDs and aliases
    /// - `default_model` exists and is active unless the provider is legacy
    /// - positive, consistent model limits; consistent modality/capability
    ///   combinations
    /// - header/param names carry no control characters and no secrets
    /// - auth env var names are valid identifiers
    /// - probe definitions: unique ids, valid method/path/bounds/predicates,
    ///   auth reference only with an auth style
    /// - `verified_at`, when present, is `YYYY-MM-DD`
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != PROVIDER_SCHEMA_VERSION {
            return Err(CoreError::Validation {
                field: "schema_version".to_owned(),
                reason: format!(
                    "provider `{}` schema_version must be {}, got {}",
                    self.id, PROVIDER_SCHEMA_VERSION, self.schema_version
                ),
            });
        }
        if self.base_url.trim().is_empty() {
            return Err(CoreError::Validation {
                field: "base_url".to_owned(),
                reason: format!("provider `{}` base_url must not be empty", self.id),
            });
        }
        let (valid, reason) = is_valid_base_url(&self.base_url);
        if !valid {
            return Err(CoreError::Validation {
                field: "base_url".to_owned(),
                reason: format!("provider `{}` base_url invalid: {reason}", self.id),
            });
        }
        if self.base_url.chars().any(char::is_control) {
            return Err(CoreError::Validation {
                field: "base_url".to_owned(),
                reason: format!(
                    "provider `{}` base_url must not contain control characters",
                    self.id
                ),
            });
        }
        if let Some(verified) = self.verified_at.as_deref()
            && !is_iso_date(verified)
        {
            return Err(CoreError::Validation {
                field: "verified_at".to_owned(),
                reason: format!(
                    "provider `{}` verified_at must be YYYY-MM-DD, got `{verified}`",
                    self.id
                ),
            });
        }
        self.validate_endpoints()?;
        self.validate_headers_and_params()?;
        self.validate_auth_inputs()?;
        self.validate_capabilities()?;
        self.validate_probes()?;
        // Model IDs non-empty and unique.
        let mut seen_ids: HashSet<String> = HashSet::new();
        let mut seen_aliases: HashSet<String> = HashSet::new();
        for model in &self.model_list {
            if model.id.trim().is_empty() {
                return Err(CoreError::Validation {
                    field: "model_list.id".to_owned(),
                    reason: format!("provider `{}` has empty model id", self.id),
                });
            }
            if model.id.chars().any(char::is_control) {
                return Err(CoreError::Validation {
                    field: "model_list.id".to_owned(),
                    reason: format!(
                        "provider `{}` model id contains control characters",
                        self.id
                    ),
                });
            }
            self.validate_model_limits(model)?;
            self.validate_model_modalities(model)?;
            let normalized = model.id.to_lowercase();
            if seen_ids.contains(&normalized) {
                return Err(CoreError::Validation {
                    field: "model_list.id".to_owned(),
                    reason: format!("provider `{}` duplicate model id `{}`", self.id, model.id),
                });
            }
            seen_ids.insert(normalized);
            if let Some(alias) = model.alias.as_deref() {
                if alias.trim().is_empty() {
                    // Empty alias is treated as absent; skip uniqueness check.
                    continue;
                }
                let alias_norm = alias.to_lowercase();
                if seen_aliases.contains(&alias_norm) || seen_ids.contains(&alias_norm) {
                    return Err(CoreError::Validation {
                        field: "model_list.alias".to_owned(),
                        reason: format!("provider `{}` duplicate model alias `{alias}`", self.id),
                    });
                }
                seen_aliases.insert(alias_norm);
            }
        }
        // Default model must exist and be active unless explicitly legacy/retired provider.
        if let Some(default) = self.defaults.default_model.as_deref() {
            if default.trim().is_empty() {
                return Err(CoreError::Validation {
                    field: "defaults.default_model".to_owned(),
                    reason: format!("provider `{}` default_model must not be empty", self.id),
                });
            }
            let mut found: Option<&ModelInfo> = None;
            for model in &self.model_list {
                if model.id == default || model.alias.as_deref() == Some(default) {
                    found = Some(model);
                    break;
                }
            }
            let Some(model) = found else {
                return Err(CoreError::Validation {
                    field: "defaults.default_model".to_owned(),
                    reason: format!(
                        "provider `{}` default_model `{default}` not found in model_list",
                        self.id
                    ),
                });
            };
            let is_legacy = matches!(
                self.status,
                ProviderStatus::Deprecated | ProviderStatus::Retired
            );
            if !is_legacy
                && matches!(
                    model.status,
                    ModelStatus::Retired | ModelStatus::Deprecated | ModelStatus::Preview
                )
            {
                return Err(CoreError::Validation {
                    field: "defaults.default_model".to_owned(),
                    reason: format!(
                        "provider `{}` default_model `{default}` is {} but provider is not legacy (deprecated/retired)",
                        self.id, model.status
                    ),
                });
            }
        }
        Ok(())
    }

    fn validate_endpoints(&self) -> Result<()> {
        let mut seen_names: HashSet<String> = HashSet::new();
        let mut seen_urls: HashSet<String> = HashSet::new();
        seen_urls.insert(self.normalized_base_url());
        for variant in &self.endpoints {
            if variant.name.trim().is_empty() {
                return Err(CoreError::Validation {
                    field: "endpoints.name".to_owned(),
                    reason: format!("provider `{}` endpoint variant has empty name", self.id),
                });
            }
            let name_norm = variant.name.to_lowercase();
            if !seen_names.insert(name_norm) {
                return Err(CoreError::Validation {
                    field: "endpoints.name".to_owned(),
                    reason: format!(
                        "provider `{}` duplicate endpoint variant `{}`",
                        self.id, variant.name
                    ),
                });
            }
            let (valid, reason) = is_valid_base_url(&variant.base_url);
            if !valid || variant.base_url.chars().any(char::is_control) {
                return Err(CoreError::Validation {
                    field: "endpoints.base_url".to_owned(),
                    reason: format!(
                        "provider `{}` endpoint `{}` base_url invalid: {reason}",
                        self.id, variant.name
                    ),
                });
            }
            let normalized = normalize_endpoint(&variant.base_url);
            if !seen_urls.insert(normalized) {
                return Err(CoreError::Validation {
                    field: "endpoints.base_url".to_owned(),
                    reason: format!(
                        "provider `{}` duplicate normalized endpoint for variant `{}`",
                        self.id, variant.name
                    ),
                });
            }
        }
        Ok(())
    }

    fn validate_headers_and_params(&self) -> Result<()> {
        let mut optional = HashSet::new();
        for (name, value) in &self.optional_headers {
            optional.insert(name.to_ascii_lowercase());
            validate_header_entry(&self.id, name, value)?;
        }
        for (name, value) in &self.required_headers {
            validate_header_entry(&self.id, name, value)?;
            if optional.contains(&name.to_ascii_lowercase()) {
                return Err(CoreError::Validation {
                    field: "required_headers".to_owned(),
                    reason: format!(
                        "provider `{}` header `{name}` declared both required and optional",
                        self.id
                    ),
                });
            }
        }
        for (name, value) in &self.request_params {
            if name.trim().is_empty() || name.chars().any(char::is_control) {
                return Err(CoreError::Validation {
                    field: "request_params".to_owned(),
                    reason: format!("provider `{}` request param name `{name}` invalid", self.id),
                });
            }
            let lower = name.to_ascii_lowercase();
            let secret_word = lower.split(['_', '-', '.']).any(|word| {
                matches!(
                    word,
                    "key" | "secret" | "token" | "password" | "auth" | "apikey"
                )
            });
            if secret_word {
                return Err(CoreError::Validation {
                    field: "request_params".to_owned(),
                    reason: format!(
                        "provider `{}` request param `{name}` looks secret-shaped; provider data must not carry secrets",
                        self.id
                    ),
                });
            }
            let value_text = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if value_text.chars().any(char::is_control) {
                return Err(CoreError::Validation {
                    field: "request_params".to_owned(),
                    reason: format!(
                        "provider `{}` request param `{name}` value contains control characters",
                        self.id
                    ),
                });
            }
        }
        Ok(())
    }

    fn validate_auth_inputs(&self) -> Result<()> {
        for name in &self.auth.env_var_names {
            if name.is_empty()
                || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                || name.chars().next().is_some_and(|c| c.is_ascii_digit())
            {
                return Err(CoreError::Validation {
                    field: "auth.env_var_names".to_owned(),
                    reason: format!(
                        "provider `{}` auth env var name `{name}` must be a valid identifier",
                        self.id
                    ),
                });
            }
        }
        if let Some(placeholder) = self.auth.key_placeholder.as_deref()
            && (placeholder.chars().any(char::is_control) || placeholder.is_empty())
        {
            return Err(CoreError::Validation {
                field: "auth.key_placeholder".to_owned(),
                reason: format!(
                    "provider `{}` key_placeholder must be non-empty without control characters",
                    self.id
                ),
            });
        }
        if let Some(prefix) = self.auth.key_prefix.as_deref()
            && (prefix.is_empty() || prefix.chars().any(char::is_control) || prefix.contains(' '))
        {
            return Err(CoreError::Validation {
                field: "auth.key_prefix".to_owned(),
                reason: format!(
                    "provider `{}` key_prefix must be a non-empty token without spaces",
                    self.id
                ),
            });
        }
        Ok(())
    }

    fn validate_capabilities(&self) -> Result<()> {
        let mut seen: HashSet<Capability> = HashSet::new();
        for decl in &self.capabilities {
            if !seen.insert(decl.capability) {
                return Err(CoreError::Validation {
                    field: "capabilities".to_owned(),
                    reason: format!(
                        "provider `{}` duplicate capability declaration {:?}",
                        self.id, decl.capability
                    ),
                });
            }
            if decl.explanation.trim().is_empty() {
                return Err(CoreError::Validation {
                    field: "capabilities".to_owned(),
                    reason: format!(
                        "provider `{}` capability {:?} has empty explanation",
                        self.id, decl.capability
                    ),
                });
            }
        }
        Ok(())
    }

    #[expect(clippy::too_many_lines, reason = "full PRV-06 probe field set")]
    #[expect(clippy::excessive_nesting, reason = "probe validation branches")]
    fn validate_probes(&self) -> Result<()> {
        let mut seen_ids: HashSet<String> = HashSet::new();
        for probe in &self.health_probes {
            if probe.id.trim().is_empty() || probe.id.chars().any(char::is_control) {
                return Err(CoreError::Validation {
                    field: "health_probes.id".to_owned(),
                    reason: format!("provider `{}` probe id `{}` invalid", self.id, probe.id),
                });
            }
            if !seen_ids.insert(probe.id.to_lowercase()) {
                return Err(CoreError::Validation {
                    field: "health_probes.id".to_owned(),
                    reason: format!("provider `{}` duplicate probe id `{}`", self.id, probe.id),
                });
            }
            let http_kind = matches!(
                probe.kind,
                crate::health::HealthProbeKind::HttpStatus
                    | crate::health::HealthProbeKind::ModelList
                    | crate::health::HealthProbeKind::MinimalAuth
            );
            if http_kind {
                if !probe.path_suffix.starts_with('/') {
                    return Err(CoreError::Validation {
                        field: "health_probes.path_suffix".to_owned(),
                        reason: format!(
                            "provider `{}` probe `{}` path_suffix must start with '/'",
                            self.id, probe.id
                        ),
                    });
                }
                if probe.path_suffix.chars().any(char::is_control)
                    || probe.path_suffix.contains(' ')
                    || probe.path_suffix.contains('?')
                {
                    return Err(CoreError::Validation {
                        field: "health_probes.path_suffix".to_owned(),
                        reason: format!(
                            "provider `{}` probe `{}` path_suffix must not contain spaces, '?' or control characters",
                            self.id, probe.id
                        ),
                    });
                }
            }
            if let Some(method) = probe.method.as_deref() {
                let upper = method.to_ascii_uppercase();
                if !matches!(upper.as_str(), "GET" | "HEAD" | "POST") {
                    return Err(CoreError::Validation {
                        field: "health_probes.method".to_owned(),
                        reason: format!(
                            "provider `{}` probe `{}` method must be GET/HEAD/POST, got `{method}`",
                            self.id, probe.id
                        ),
                    });
                }
            }
            if probe.uses_auth && matches!(self.auth_style, AuthStyle::None) {
                return Err(CoreError::Validation {
                    field: "health_probes.uses_auth".to_owned(),
                    reason: format!(
                        "provider `{}` probe `{}` references auth but provider auth_style is none",
                        self.id, probe.id
                    ),
                });
            }
            if let Some(timeout_ms) = probe.timeout_ms
                && let Err(e) =
                    crate::health::validate_timeout(std::time::Duration::from_millis(timeout_ms))
            {
                return Err(CoreError::Validation {
                    field: "health_probes.timeout_ms".to_owned(),
                    reason: format!("provider `{}` probe `{}`: {e}", self.id, probe.id),
                });
            }
            if let Some(max_bytes) = probe.max_response_bytes
                && (max_bytes == 0 || max_bytes > 10 * crate::health::MAX_RESPONSE_BYTES)
            {
                return Err(CoreError::Validation {
                    field: "health_probes.max_response_bytes".to_owned(),
                    reason: format!(
                        "provider `{}` probe `{}` max_response_bytes must be 1..={} bytes",
                        self.id,
                        probe.id,
                        10 * crate::health::MAX_RESPONSE_BYTES
                    ),
                });
            }
            if probe.accepted_status.is_empty()
                || probe
                    .accepted_status
                    .iter()
                    .any(|code| !(200..=599).contains(code))
            {
                return Err(CoreError::Validation {
                    field: "health_probes.accepted_status".to_owned(),
                    reason: format!(
                        "provider `{}` probe `{}` accepted_status must list codes in 200..=599",
                        self.id, probe.id
                    ),
                });
            }
            for (name, value) in &probe.headers {
                validate_header_entry(&self.id, name, value)?;
            }
            if let Some(body) = probe.body_template.as_deref()
                && (body.chars().any(char::is_control) || body.contains('\0'))
            {
                return Err(CoreError::Validation {
                    field: "health_probes.body_template".to_owned(),
                    reason: format!(
                        "provider `{}` probe `{}` body_template contains control characters",
                        self.id, probe.id
                    ),
                });
            }
            if let Some(needle) = probe.body_contains.as_deref()
                && (needle.is_empty() || needle.chars().any(char::is_control))
            {
                return Err(CoreError::Validation {
                    field: "health_probes.body_contains".to_owned(),
                    reason: format!(
                        "provider `{}` probe `{}` body_contains must be non-empty without control characters",
                        self.id, probe.id
                    ),
                });
            }
        }
        Ok(())
    }

    fn validate_model_limits(&self, model: &ModelInfo) -> Result<()> {
        let limits = &model.limits;
        for (name, value) in [
            ("context_tokens", limits.context_tokens),
            ("input_tokens", limits.input_tokens),
            ("output_tokens", limits.output_tokens),
        ] {
            if let Some(v) = value
                && v == 0
            {
                return Err(CoreError::Validation {
                    field: "model_list.limits".to_owned(),
                    reason: format!(
                        "provider `{}` model `{}` limit {name} must be positive",
                        self.id, model.id
                    ),
                });
            }
        }
        if let (Some(context), Some(input), Some(output)) = (
            limits.context_tokens,
            limits.input_tokens,
            limits.output_tokens,
        ) && input.saturating_add(output) > context
        {
            return Err(CoreError::Validation {
                field: "model_list.limits".to_owned(),
                reason: format!(
                    "provider `{}` model `{}` input+output tokens exceed the context window",
                    self.id, model.id
                ),
            });
        }
        Ok(())
    }

    fn validate_model_modalities(&self, model: &ModelInfo) -> Result<()> {
        if model.supports_tools && !model.input_modalities.is_empty() {
            let has_text = model
                .input_modalities
                .iter()
                .any(|m| matches!(m, Modality::Text));
            if !has_text {
                return Err(CoreError::Validation {
                    field: "model_list.input_modalities".to_owned(),
                    reason: format!(
                        "provider `{}` model `{}` declares tool support but no text input modality — tools ride on text turns",
                        self.id, model.id
                    ),
                });
            }
        }
        if model.supports_reasoning && model.limits.output_tokens == Some(0) {
            return Err(CoreError::Validation {
                field: "model_list.limits".to_owned(),
                reason: format!(
                    "provider `{}` model `{}` declares reasoning with zero output budget",
                    self.id, model.id
                ),
            });
        }
        Ok(())
    }

    /// Resolve the base URL to use (PRV-01: variant with fallback).
    ///
    /// A named variant matches case-insensitively; when absent or `None` is
    /// requested, the default `base_url` is returned.
    pub fn endpoint_for(&self, variant: Option<&str>) -> &str {
        let Some(wanted) = variant else {
            return &self.base_url;
        };
        for candidate in &self.endpoints {
            if candidate.name.eq_ignore_ascii_case(wanted) {
                return &candidate.base_url;
            }
        }
        &self.base_url
    }

    /// Resolve an endpoint that can serve `protocol` (PRV-03 protocol
    /// selection): a variant advertising the protocol wins; otherwise the
    /// default endpoint when the provider's own protocol matches.
    pub fn endpoint_for_protocol(&self, protocol: Protocol) -> Option<&str> {
        for candidate in &self.endpoints {
            if candidate
                .protocols
                .iter()
                .any(|p| *p == protocol && *p != Protocol::Other)
            {
                return Some(&candidate.base_url);
            }
        }
        if self.protocol == protocol {
            Some(&self.base_url)
        } else {
            None
        }
    }

    /// Capability declaration for `capability`, if the provider makes one.
    pub fn capability_decl(&self, capability: Capability) -> Option<&ProviderCapabilityDecl> {
        self.capabilities
            .iter()
            .find(|d| d.capability == capability)
    }

    /// Normalized base URL for duplicate detection.
    ///
    /// Lowercases scheme/host and trims trailing slashes. Uses character-boundary safe truncation.
    pub fn normalized_base_url(&self) -> String {
        normalize_endpoint(&self.base_url)
    }
}

fn normalize_endpoint(url: &str) -> String {
    let mut url = url.trim().to_lowercase();
    while url.ends_with('/') && url.len() > 1 {
        // Safe: `ends_with('/')` guarantees last char is one byte '/'.
        url.pop();
    }
    url
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 {
        return false;
    }
    let dash_at = |i: usize| bytes.get(i).is_some_and(|b| *b == b'-');
    let digits_at = |(from, to): (usize, usize)| {
        bytes
            .get(from..to)
            .is_some_and(|slice| slice.iter().all(u8::is_ascii_digit))
    };
    dash_at(4) && dash_at(7) && digits_at((0, 4)) && digits_at((5, 7)) && digits_at((8, 10))
}

fn validate_header_entry(provider: &ProviderId, name: &str, value: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().any(char::is_control) || name.contains(' ') {
        return Err(CoreError::Validation {
            field: "headers".to_owned(),
            reason: format!("provider `{provider}` header name `{name}` invalid"),
        });
    }
    if value.chars().any(char::is_control) || value.contains('\0') {
        return Err(CoreError::Validation {
            field: "headers".to_owned(),
            reason: format!(
                "provider `{provider}` header `{name}` value contains control characters"
            ),
        });
    }
    Ok(())
}

/// Whether `text` contains a secret-shaped value: `sk-` (or `sk-live-`)
/// followed by a run of at least 16 token characters. Documentation
/// placeholders like `sk-...` do not match.
#[cfg(test)]
pub(crate) fn contains_secret_shaped_value(text: &str) -> bool {
    fn is_token_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
    }
    for marker in ["sk-", "sk-live-"] {
        let mut search_from = 0;
        while let Some(found) = text.get(search_from..).and_then(|rest| rest.find(marker)) {
            let after_start = search_from + found + marker.len();
            let run = text.get(after_start..).map_or(0, |rest| {
                rest.chars().take_while(|c| is_token_char(*c)).count()
            });
            if run >= 16 {
                return true;
            }
            search_from = after_start;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// URL validation
// ---------------------------------------------------------------------------

fn is_valid_base_url(url: &str) -> (bool, String) {
    if url.trim().is_empty() {
        return (false, "must not be empty".to_owned());
    }
    if url.chars().any(char::is_control) {
        return (false, "must not contain control characters".to_owned());
    }
    if url.contains(' ') {
        return (false, "must not contain spaces".to_owned());
    }
    let scheme_rest = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        rest
    } else {
        return (false, "must start with https:// or http://".to_owned());
    };
    if scheme_rest.is_empty() {
        return (false, "missing host".to_owned());
    }
    // Host is up to first '/' or ':' or end.
    let host_end = scheme_rest.find('/').unwrap_or(scheme_rest.len());
    let host_with_port = scheme_rest.get(0..host_end).unwrap_or_default();
    let host = host_with_port.split(':').next().unwrap_or_default();
    if host.is_empty() {
        return (false, "missing host".to_owned());
    }
    // Allow localhost and loopback, otherwise require dot.
    let is_local = host == "localhost" || host == "127.0.0.1" || host == "::1";
    if !is_local && !host.contains('.') {
        return (false, "host must contain '.' or be localhost".to_owned());
    }
    // Block file:// already handled by scheme check; explicitly reject others.
    if url.starts_with("file://") {
        return (false, "file scheme not allowed".to_owned());
    }
    (true, "ok".to_owned())
}

// ---------------------------------------------------------------------------
// Bundled providers — data-driven JSON under assets/providers.json
// ---------------------------------------------------------------------------

/// Raw JSON for bundled providers (`GLM`, `MiniMax`, `Anthropic`) as checked in
/// `crates/superai-core/assets/providers.json`.
///
/// The file is the source of truth; no provider is hardcoded in Rust. Adding
/// a provider is a data-only change: add an entry to the JSON and no Rust
/// edit is required. The value is embedded via `include_str!` so tests and
/// runtime both read the same data without filesystem assumptions.
pub const BUNDLED_PROVIDERS_JSON: &str = include_str!("../assets/providers.json");

/// Load providers from the bundled `assets/providers.json`.
///
/// Parses the embedded JSON, validates each definition, and rejects
/// duplicates. No secret is contained or leaked.
pub fn load_bundled_providers() -> Result<Vec<ProviderDefinition>> {
    let providers: Vec<ProviderDefinition> =
        serde_json::from_str(BUNDLED_PROVIDERS_JSON).map_err(|source| CoreError::Parse {
            path: PathBuf::from("assets/providers.json"),
            kind: "json".to_owned(),
            message: source.to_string(),
        })?;
    for p in &providers {
        p.validate()?;
    }
    validate_no_duplicates(&providers)?;
    Ok(providers)
}

/// Load bundled providers plus any additional definitions from `extra_path`.
///
/// `extra_path` may be a file or directory. Bundled providers and extra
/// providers are merged; duplicates across the two sets are rejected. This
/// proves that adding a dummy provider via a file requires no code change.
pub fn load_bundled_plus_extra(extra_path: &Path) -> Result<Vec<ProviderDefinition>> {
    let mut bundled = load_bundled_providers()?;
    let extra = load_provider_defs(extra_path)?;
    bundled.extend(extra);
    validate_no_duplicates(&bundled)?;
    for p in &bundled {
        p.validate()?;
    }
    Ok(bundled)
}

// ---------------------------------------------------------------------------
// Health probe — delegated to crate::health (bounded, redacted, classified)
// ---------------------------------------------------------------------------

/// Result of a health probe — validates URL format, timeout, and classification.
///
/// `base_url` is redacted if it contained query secrets (e.g. `api_key=...`).
/// `reason` never contains raw secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthProbeResult {
    /// Provider id as string.
    pub provider: String,
    /// Base URL probed (redacted).
    pub base_url: String,
    /// Whether the URL is syntactically valid and healthy.
    pub valid: bool,
    /// Reason for validity or failure (redacted).
    pub reason: String,
}

/// Health probe that validates URL format, timeout bounds, private policy,
/// and redacts secrets without network.
///
/// Delegates to `crate::health` for bounded timeout and classification.
/// No DNS, TLS, or HTTP request is performed in the default path; mock
/// network variants are available via `crate::health::health_probe_with_mock`.
pub fn health_probe(provider: &ProviderDefinition) -> HealthProbeResult {
    let cfg = crate::health::HealthConfig::default();
    let res = crate::health::health_probe(provider, &cfg);
    HealthProbeResult {
        provider: res.provider,
        base_url: res.base_url_redacted,
        valid: res.valid,
        reason: res.reason,
    }
}

/// Validate a raw URL string without a provider (useful for preview).
///
/// Uses the same bounded, redacted validation as the provider probe.
pub fn health_probe_url(url: &str) -> HealthProbeResult {
    let cfg = crate::health::HealthConfig::default();
    let res = crate::health::health_probe_url(url, &cfg);
    HealthProbeResult {
        provider: res.provider,
        base_url: res.base_url_redacted,
        valid: res.valid,
        reason: res.reason,
    }
}

/// Health probe with explicit config (timeout, private policy, size caps).
pub fn health_probe_with_config(
    provider: &ProviderDefinition,
    config: &crate::health::HealthConfig,
) -> HealthProbeResult {
    let res = crate::health::health_probe(provider, config);
    HealthProbeResult {
        provider: res.provider,
        base_url: res.base_url_redacted,
        valid: res.valid,
        reason: res.reason,
    }
}

// ---------------------------------------------------------------------------
// API-key placement — ephemeral, sink-restricted, redacted
// ---------------------------------------------------------------------------

/// Kind of sink where an ephemeral API key may be written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeySinkKind {
    /// Harness config field (JSON/TOML/YAML) under the instance isolated root.
    ConfigField,
    /// Harness-supported env file under the isolated root (e.g. `.env`).
    EnvFile,
    /// Wrapper reference to an externally set env var (no secret in wrapper).
    WrapperEnvRef,
    /// Harness-supported helper/command reference.
    Helper,
}

impl std::fmt::Display for ApiKeySinkKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::ConfigField => "config_field",
            Self::EnvFile => "env_file",
            Self::WrapperEnvRef => "wrapper_env_ref",
            Self::Helper => "helper",
        };
        f.write_str(s)
    }
}

/// Resolved sink for an API key write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeySink {
    /// Kind of sink.
    pub kind: ApiKeySinkKind,
    /// Config surface id (e.g. `settings.json` or `.env`).
    pub surface_id: String,
    /// Selector inside the surface (e.g. `env.ANTHROPIC_API_KEY`).
    pub selector: String,
    /// Human description of the destination.
    pub description: String,
}

/// Preview of where an ephemeral API key would be written.
///
/// Contains no secret — only the destination and auth style, with a
/// `[REDACTED]` placeholder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeyPreview {
    /// Sink that would receive the key.
    pub sink: ApiKeySink,
    /// Auth style used for the provider.
    pub auth_style: AuthStyle,
    /// Destination path (absolute) where the secret would be written.
    pub destination: String,
    /// Redacted placeholder (never the raw key).
    pub redacted: String,
}

/// Validate a raw API key value before placement (PRV-04).
///
/// - Must be non-empty and not contain control chars or NUL.
/// - Must fit the 4 KiB cap.
/// - Prefix check ONLY when the provider documents one via
///   `auth.key_prefix` (per-provider documented prefix policy); the value
///   itself is never echoed into the error.
pub fn validate_api_key_value(provider: &ProviderDefinition, key: &str) -> Result<()> {
    if key.trim().is_empty() {
        return Err(CoreError::Validation {
            field: "api_key".to_owned(),
            reason: format!("api key for provider `{}` must not be empty", provider.id),
        });
    }
    if key.chars().any(char::is_control) {
        return Err(CoreError::Validation {
            field: "api_key".to_owned(),
            reason: "api key must not contain control characters".to_owned(),
        });
    }
    if key.contains('\0') {
        return Err(CoreError::Validation {
            field: "api_key".to_owned(),
            reason: "api key must not contain NUL".to_owned(),
        });
    }
    if key.len() > 4096 {
        return Err(CoreError::Validation {
            field: "api_key".to_owned(),
            reason: "api key exceeds 4 KiB limit".to_owned(),
        });
    }
    if let Some(prefix) = provider.auth.key_prefix.as_deref()
        && !key.starts_with(prefix)
    {
        return Err(CoreError::SecretValidation {
            field: "api_key".to_owned(),
            reason: format!(
                "key for provider `{}` does not match the documented prefix `{prefix}`",
                provider.id
            ),
            redacted: RedactedString::new(key),
        });
    }
    // Allow ${ENV_VAR} references only for WrapperEnvRef sink path, not for literal writes.
    // We don't reject here; sink selection will enforce the right handling.
    Ok(())
}

/// OAuth/subscription/keychain login requirement (PRV-04).
///
/// When a harness's only credential surfaces are external secret stores
/// (keychain/SSO), an API key cannot be placed by superai: the user must run
/// the harness's own login command. Returns the typed
/// [`CoreError::ExternalAuthRequired`] with harness instructions; superai
/// never proxies or performs the login flow.
pub fn external_auth_requirement(adapter: &dyn Adapter) -> Option<CoreError> {
    let surfaces = adapter.config_surfaces();
    let has_external_store = surfaces.iter().any(|surface| {
        surface.ownership == SurfaceOwnership::ExternalSecretStore
            || matches!(surface.kind, DocumentKind::Keychain)
    });
    let has_writable = surfaces.iter().any(|surface| {
        matches!(
            surface.ownership,
            SurfaceOwnership::UserEditable | SurfaceOwnership::SuperaiCreated
        ) && !matches!(
            surface.kind,
            DocumentKind::Sqlite | DocumentKind::Keychain | DocumentKind::Opaque
        )
    });
    if has_external_store && !has_writable {
        return Some(CoreError::ExternalAuthRequired {
            harness: adapter.id().to_string(),
            instructions: format!(
                "run the harness's own login command (e.g. `{binary} login`) in the instance environment; credentials live in the harness keychain, which superai does not touch",
                binary = adapter.id().as_str()
            ),
        });
    }
    None
}

/// Resolve the harness-supported sink for `adapter`.
///
/// Inspects `adapter.config_surfaces()` and picks the first suitable
/// `UserEditable` / `Instance` sink whose owned selectors indicate an API key
/// field. For JSON surfaces the selector is that owned selector; for Env
/// surfaces it is the env var name derived from the selector.
///
/// Never selects registry, logs, or `ExternalSecretStore` / `Sqlite` / `Keychain`
/// surfaces.
pub fn resolve_api_key_sink(adapter: &dyn Adapter) -> Result<ApiKeySink> {
    let surfaces = adapter.config_surfaces();
    // First, prefer any UserEditable JSON/Jsonc/Toml/Yaml/Toml surface with an api-key-like owned selector.
    for surface in &surfaces {
        if surface.ownership == SurfaceOwnership::ExternalSecretStore {
            continue;
        }
        if matches!(
            surface.kind,
            DocumentKind::Sqlite
                | DocumentKind::Keychain
                | DocumentKind::Opaque
                | DocumentKind::Executable
        ) {
            continue;
        }
        // Only consider surfaces that are writable (UserEditable or SuperaiCreated)
        let is_writable = matches!(
            surface.ownership,
            SurfaceOwnership::UserEditable | SurfaceOwnership::SuperaiCreated
        );
        if !is_writable {
            continue;
        }
        // Check owned selectors for api-key-like patterns.
        for sel in &surface.owned_selectors {
            let lower = sel.to_ascii_lowercase();
            if lower.contains("api_key")
                || lower.contains("apikey")
                || lower.contains("api-key")
                || lower.contains("auth_token")
                || lower.contains("anthropic_api_key")
                || lower.contains("anthropic_auth_token")
                || lower.contains("apikeyhelper")
            {
                return Ok(ApiKeySink {
                    kind: ApiKeySinkKind::ConfigField,
                    surface_id: surface.id.clone(),
                    selector: sel.clone(),
                    description: format!(
                        "harness config field `{}` in surface `{}` (instance root)",
                        sel, surface.id
                    ),
                });
            }
        }
    }
    // Second, Env file under isolated root.
    for surface in &surfaces {
        if surface.kind == DocumentKind::Env
            && matches!(
                surface.ownership,
                SurfaceOwnership::UserEditable | SurfaceOwnership::SuperaiCreated
            )
        {
            // Prefer Instance-scoped env files.
            if surface.scope == crate::adapter::ConfigScope::Instance
                || surface.scope == crate::adapter::ConfigScope::User
            {
                // Derive selector: first owned selector if any, else conventional var
                let selector = surface
                    .owned_selectors
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "ANTHROPIC_API_KEY".to_owned());
                return Ok(ApiKeySink {
                    kind: ApiKeySinkKind::EnvFile,
                    surface_id: surface.id.clone(),
                    selector,
                    description: format!("env file `{}` under isolated root", surface.id),
                });
            }
        }
    }
    // Third, wrapper env ref is allowed only if the harness explicitly declares
    // wrapper/env file as credential storage via its surfaces. We do NOT invent
    // a generic wrapper literal sink. If no config/env sink exists, return
    // Unsupported with a reason.
    Err(CoreError::UnsupportedOperation {
        harness: adapter.id().to_string(),
        operation: "place_api_key".to_owned(),
        reason: "harness declares no writable config or env sink for api key".to_owned(),
    })
}

/// Preview where an ephemeral key would be written (redacted).
///
/// Validates the key, resolves the harness-declared sink, and returns a
/// description with `[REDACTED]` placeholder and auth style. The raw key
/// never appears in the returned value or in logs.
pub fn preview_api_key_placement(
    key: &RedactedString,
    provider: &ProviderDefinition,
    adapter: &dyn Adapter,
    instance: &Instance,
) -> Result<ApiKeyPreview> {
    let raw = key.expose_secret();
    validate_api_key_value(provider, raw)?;
    let sink = resolve_api_key_sink(adapter)?;
    // Destination: instance isolated root + surface id (best-effort)
    let dest = instance
        .config_root
        .as_path()
        .join(&sink.surface_id)
        .display()
        .to_string();
    Ok(ApiKeyPreview {
        sink,
        auth_style: provider.auth_style.clone(),
        destination: dest,
        redacted: RedactedString::placeholder().to_owned(),
    })
}

/// Write an ephemeral API key only to the harness-supported sink.
///
/// Validates the key, resolves the sink, backs up the destination if it
/// exists, writes through an atomic transaction preserving unmodelled keys,
/// sets restrictive permissions (0o600 on unix), and drops the raw key
/// after. The key is never written to instance/registry/provider/template
/// records, wrapper literals, logs, or journal.
pub fn commit_api_key(
    key: &RedactedString,
    provider: &ProviderDefinition,
    adapter: &dyn Adapter,
    instance: &Instance,
) -> Result<ApiKeyPreview> {
    let raw = key.expose_secret();
    validate_api_key_value(provider, raw)?;
    let sink = resolve_api_key_sink(adapter)?;
    let dest_path = instance.config_root.as_path().join(&sink.surface_id);
    let preview = ApiKeyPreview {
        sink: sink.clone(),
        auth_style: provider.auth_style.clone(),
        destination: dest_path.display().to_string(),
        redacted: RedactedString::placeholder().to_owned(),
    };
    // Ensure instance root exists.
    std::fs::create_dir_all(instance.config_root.as_path()).map_err(|e| {
        CoreError::InvalidPath {
            kind: "config_root".to_owned(),
            value: instance.config_root.to_string(),
            reason: format!("cannot create config root: {e}"),
        }
    })?;
    match sink.kind {
        ApiKeySinkKind::ConfigField => {
            write_config_field(&dest_path, &sink.selector, raw, adapter)?;
        }
        ApiKeySinkKind::EnvFile => {
            write_env_file(&dest_path, &sink.selector, raw)?;
        }
        ApiKeySinkKind::WrapperEnvRef | ApiKeySinkKind::Helper => {
            return Err(CoreError::UnsupportedOperation {
                harness: adapter.id().to_string(),
                operation: "place_api_key".to_owned(),
                reason:
                    "wrapper/helper sink requires caller to set external env var, not literal write"
                        .to_owned(),
            });
        }
    }
    // Harden permissions (unix 0o600). Do not log raw key.
    harden_permissions(&dest_path)?;
    // Drop raw: the RedactedString will be dropped by caller; we ensure no copy remains in preview.
    // Explicitly zeroing is not needed here as we never cloned raw into a long-lived structure.
    Ok(preview)
}

/// Unix: tighten the sink file to owner-only (0o600). Windows has no mode
/// bits; the atomic write already creates the file with user-only defaults.
#[cfg(unix)]
fn harden_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let perm = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perm).map_err(|e| CoreError::InvalidPath {
        kind: "permissions".to_owned(),
        value: path.display().to_string(),
        reason: format!("cannot set 0o600: {e}"),
    })
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "no-op off unix; callers keep the Result contract"
)]
fn harden_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn write_config_field(
    dest: &Path,
    selector: &str,
    secret: &str,
    adapter: &dyn Adapter,
) -> Result<()> {
    // Read existing json if present, else start empty object. Preserve unmodelled keys.
    let existing: Option<Value> = if dest.exists() {
        let bytes = std::fs::read(dest).map_err(|e| CoreError::InvalidPath {
            kind: "read".to_owned(),
            value: dest.display().to_string(),
            reason: format!("cannot read destination: {e}"),
        })?;
        if bytes.is_empty() {
            None
        } else {
            // Try parse as json; if fails, treat as error with validation kind.
            let v: Value = serde_json::from_slice(&bytes).map_err(|e| CoreError::Parse {
                path: dest.to_path_buf(),
                kind: "json".to_owned(),
                message: e.to_string(),
            })?;
            Some(v)
        }
    } else {
        None
    };
    let mut root = existing.unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    // Backup before write if file existed.
    if dest.exists() {
        let _ = superai_config::backup::backup(dest).map_err(CoreError::Config)?;
    }
    // Apply selector: supports "model", "env.FOO", "env.ANTHROPIC_API_KEY" etc.
    // Selector may be prefixed with "key:" or "env." already; strip "key:" if present.
    let sel = selector
        .strip_prefix("key:")
        .unwrap_or(selector)
        .strip_prefix("env.")
        .unwrap_or(selector);
    // Heuristic: if selector still contains "env." handle nested env object.
    let (target_obj, leaf_key) = if selector.contains("env.") || selector.starts_with("env.") {
        // Ensure "env" object exists.
        let env_key = "env";
        if !root.is_object() {
            root = Value::Object(serde_json::Map::new());
        }
        let map = root.as_object_mut().expect("just set to object");
        let env_entry = map
            .entry(env_key.to_owned())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if !env_entry.is_object() {
            *env_entry = Value::Object(serde_json::Map::new());
        }
        // Extract leaf after last '.'
        let leaf = sel.split('.').next_back().unwrap_or(sel);
        // For selectors like "env.ANTHROPIC_API_KEY", sel already stripped, leaf is correct.
        // If original was "env.ANTHROPIC_API_KEY", sel = "ANTHROPIC_API_KEY", ok.
        (env_entry, leaf.to_owned())
    } else if selector.contains('.') {
        // Generic dot nesting: create nested objects.
        let parts: Vec<&str> = selector.split('.').collect();
        let leaf = parts.last().copied().unwrap_or(selector).to_owned();
        // Walk/create path except leaf.
        let mut cur = &mut root;
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            if !cur.is_object() {
                *cur = Value::Object(serde_json::Map::new());
            }
            let map = cur.as_object_mut().expect("object");
            let entry = map
                .entry((*part).to_owned())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            cur = entry;
        }
        (cur, leaf)
    } else {
        (&mut root, sel.to_owned())
    };
    if let Some(obj) = target_obj.as_object_mut() {
        obj.insert(leaf_key, Value::String(secret.to_owned()));
    } else {
        return Err(CoreError::Validation {
            field: "selector".to_owned(),
            reason: format!("selector `{selector}` target is not an object"),
        });
    }
    let new_bytes = serde_json::to_vec_pretty(&root).map_err(|e| CoreError::InvalidPath {
        kind: "serialize".to_owned(),
        value: dest.display().to_string(),
        reason: format!("cannot serialize json: {e}"),
    })?;
    // Write via atomic transaction (which also backs up, but we already did). Use raw_editor commit_for_adapter to enforce surface policy.
    crate::raw_editor::commit_for_adapter(dest, &new_bytes, None, adapter)?;
    Ok(())
}

fn write_env_file(dest: &Path, var: &str, secret: &str) -> Result<()> {
    if var.trim().is_empty() || var.chars().any(char::is_control) || var.contains('=') {
        return Err(CoreError::Validation {
            field: "env_var".to_owned(),
            reason: format!("invalid env var name `{var}`"),
        });
    }
    if dest.exists() {
        let _ = superai_config::backup::backup(dest).map_err(CoreError::Config)?;
    }
    let mut content = if dest.exists() {
        std::fs::read_to_string(dest).map_err(|e| CoreError::InvalidPath {
            kind: "read".to_owned(),
            value: dest.display().to_string(),
            reason: format!("cannot read env file: {e}"),
        })?
    } else {
        String::new()
    };
    // Preserve other lines, replace or append var.
    let mut lines: Vec<String> = content.lines().map(ToOwned::to_owned).collect();
    let mut found = false;
    for line in &mut lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if let Some(eq) = trimmed.find('=') {
            let key = trimmed.get(0..eq).map_or("", |s| s.trim());
            if key == var {
                *line = format!("{var}={secret}");
                found = true;
                break;
            }
        }
    }
    if !found {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
            lines = content.lines().map(ToOwned::to_owned).collect();
        }
        lines.push(format!("{var}={secret}"));
    }
    let new_content = lines.join("\n") + "\n";
    // Ensure parent exists.
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::InvalidPath {
            kind: "env_parent".to_owned(),
            value: parent.display().to_string(),
            reason: format!("cannot create parent: {e}"),
        })?;
    }
    // Plan-02 fold: provider env writes go through the config crate's ONE
    // mutation boundary (snapshot → backup → §4.2 recheck → atomic replace →
    // verify); the boundary's env staged-validation skips comment/blank lines
    // so preserved lexical material is never refused.
    superai_config::transaction::commit_file(
        "provider-env",
        dest,
        new_content.as_bytes(),
        superai_config::document::DocumentKind::Env,
    )
    .map_err(CoreError::Config)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Load provider definitions from a file or directory.
///
/// - If `path` is a file, parse it as JSON or YAML (by extension, fallback to try both).
/// - If `path` is a directory, read every `*.json`, `*.yaml`, `*.yml` file inside non-recursively
///   and merge results. Duplicate ids or normalized base URLs are rejected.
/// - Every loaded definition is validated.
pub fn load_provider_defs(path: &Path) -> Result<Vec<ProviderDefinition>> {
    if !path.exists() {
        return Err(CoreError::InvalidPath {
            kind: "provider_defs".to_owned(),
            value: path.display().to_string(),
            reason: "path does not exist".to_owned(),
        });
    }
    if path.is_dir() {
        load_from_dir(path)
    } else {
        load_from_file(path)
    }
}

fn load_from_dir(dir: &Path) -> Result<Vec<ProviderDefinition>> {
    let entries = std::fs::read_dir(dir).map_err(|source| CoreError::InvalidPath {
        kind: "provider_defs".to_owned(),
        value: dir.display().to_string(),
        reason: format!("cannot read directory: {source}"),
    })?;
    let mut all: Vec<ProviderDefinition> = Vec::new();
    for entry_res in entries {
        let entry = entry_res.map_err(|source| CoreError::InvalidPath {
            kind: "provider_defs".to_owned(),
            value: dir.display().to_string(),
            reason: format!("cannot read dir entry: {source}"),
        })?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_lowercase();
        let is_provider_file = ext == "json" || ext == "yaml" || ext == "yml";
        if !is_provider_file {
            continue;
        }
        let mut providers = load_from_file(&p)?;
        all.append(&mut providers);
    }
    validate_no_duplicates(&all)?;
    Ok(all)
}

fn load_from_file(path: &Path) -> Result<Vec<ProviderDefinition>> {
    let text = std::fs::read_to_string(path).map_err(|source| CoreError::InvalidPath {
        kind: "provider_defs".to_owned(),
        value: path.display().to_string(),
        reason: format!("cannot read file: {source}"),
    })?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let providers = if ext == "json" {
        parse_json_providers(&text, path)?
    } else if ext == "yaml" || ext == "yml" {
        parse_yaml_providers(&text, path)?
    } else {
        // Fallback: try JSON then YAML.
        parse_json_providers(&text, path).or_else(|_| parse_yaml_providers(&text, path))?
    };
    for p in &providers {
        p.validate()?;
    }
    validate_no_duplicates(&providers)?;
    Ok(providers)
}

fn parse_json_providers(text: &str, path: &Path) -> Result<Vec<ProviderDefinition>> {
    // Try vec first.
    if let Ok(vec) = serde_json::from_str::<Vec<ProviderDefinition>>(text) {
        return Ok(vec);
    }
    // Try single.
    match serde_json::from_str::<ProviderDefinition>(text) {
        Ok(single) => Ok(vec![single]),
        Err(source) => Err(CoreError::Parse {
            path: path.to_path_buf(),
            kind: "json".to_owned(),
            message: source.to_string(),
        }),
    }
}

fn parse_yaml_providers(text: &str, path: &Path) -> Result<Vec<ProviderDefinition>> {
    if let Ok(vec) = yaml_serde::from_str::<Vec<ProviderDefinition>>(text) {
        return Ok(vec);
    }
    match yaml_serde::from_str::<ProviderDefinition>(text) {
        Ok(single) => Ok(vec![single]),
        Err(source) => Err(CoreError::Parse {
            path: PathBuf::from(path),
            kind: "yaml".to_owned(),
            message: source.to_string(),
        }),
    }
}

fn validate_no_duplicates(providers: &[ProviderDefinition]) -> Result<()> {
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut seen_urls: HashMap<String, String> = HashMap::new();
    for p in providers {
        let id_norm = p.id.normalized();
        if seen_ids.contains(&id_norm) {
            return Err(CoreError::Validation {
                field: "id".to_owned(),
                reason: format!("duplicate provider id `{}`", p.id),
            });
        }
        seen_ids.insert(id_norm);
        let norm_url = p.normalized_base_url();
        if let Some(existing) = seen_urls.get(&norm_url) {
            return Err(CoreError::Validation {
                field: "base_url".to_owned(),
                reason: format!(
                    "duplicate normalized base_url `{}` for providers `{existing}` and `{}`",
                    p.base_url, p.id
                ),
            });
        }
        seen_urls.insert(norm_url, p.id.to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![expect(clippy::assertions_on_result_states, reason = "explicit Ok/Err checks")]
    use super::*;
    use crate::adapter::{
        ConfigScope, ConfigSurface, DetectionResult, PathResolver, Platform, ProductStatus,
        VersionResolution, WrapperPlan,
    };
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::TemplateRef;
    use crate::paths::AbsolutePath;
    use crate::state::AdapterSupport;
    use crate::state::{InstanceOrigin, Isolation, Ownership};
    use std::time::Duration;

    fn def(id: &str, base_url: &str) -> ProviderDefinition {
        ProviderDefinition::new(ProviderId::new(id).unwrap(), base_url)
    }

    fn tmp_dir(name: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(&format!("provider-{name}"))
    }

    /// Write a fake `claude` binary that answers `--version` with a parseable
    /// version. Keeps the adapter's version gate hermetic: no real `claude`
    /// install is probed on the host.
    fn write_fake_claude(dir: &Path) -> PathBuf {
        #[cfg(unix)]
        {
            let path = dir.join("claude");
            std::fs::write(
                &path,
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"1.2.3 (Claude Code)\"; exit 0; fi\necho \"Usage: claude\"\n",
            )
            .unwrap();
            {
                use std::os::unix::fs::PermissionsExt as _;
                let mut perms = std::fs::metadata(&path).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&path, perms).unwrap();
            }
            path
        }
        #[cfg(not(unix))]
        {
            // Windows cannot exec a `#!/bin/sh` script; a batch stub answers
            // the same probe through cmd.exe.
            let path = dir.join("claude.bat");
            std::fs::write(
                &path,
                "@echo off\r\nif \"%1\"==\"--version\" (echo 1.2.3) else (echo Usage: claude)\r\n",
            )
            .unwrap();
            path
        }
    }

    fn single_provider_json(id: &str, base_url: &str) -> String {
        format!(
            r#"{{
  "id": "{id}",
  "display_name": "{id} display",
  "base_url": "{base_url}",
  "auth_style": "bearer",
  "protocol": "openai_chat",
  "model_list": [
    {{"id": "model-a", "status": "active"}},
    {{"id": "model-b", "status": "active"}}
  ],
  "defaults": {{"default_model": "model-a"}},
  "status": "active"
}}"#
        )
    }

    fn sample_instance(dir: &Path, name: &str) -> Instance {
        let config_root = dir.join(name);
        std::fs::create_dir_all(&config_root).unwrap();
        Instance {
            id: InstanceId::new(&format!("id-{name}-{}", std::process::id())).unwrap(),
            name: InstanceName::new(name).unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::from_path(&config_root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: crate::ids::TemplateId::new("claude-glm").unwrap(),
                version: crate::ids::TemplateVersion::new("1.2.0").unwrap(),
            }),
            created_at: "2026-08-26T12:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        }
    }

    #[test]
    fn load_single_json_file() {
        let dir = tmp_dir("single");
        let path = dir.join("provider.json");
        let json = single_provider_json("synthetic-provider-xyz", "https://api.example.com");
        std::fs::write(&path, json).unwrap();
        let out = load_provider_defs(&path).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id.as_str(), "synthetic-provider-xyz");
        assert_eq!(out[0].base_url, "https://api.example.com");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn data_only_adding_provider_requires_no_code_change() {
        let dir = tmp_dir("data-only");
        // Two providers initially.
        for (id, url) in [
            ("prov-a", "https://a.example.com"),
            ("prov-b", "https://b.example.com"),
        ] {
            let path = dir.join(format!("{id}.json"));
            std::fs::write(&path, single_provider_json(id, url)).unwrap();
        }
        let first = load_provider_defs(&dir).unwrap();
        assert_eq!(first.len(), 2);

        // Add a synthetic third provider — no Rust edit.
        let new_json = single_provider_json("synthetic-new-99", "https://new.example.com");
        std::fs::write(dir.join("synthetic-new-99.json"), new_json).unwrap();
        let second = load_provider_defs(&dir).unwrap();
        assert_eq!(second.len(), 3);
        assert!(second.iter().any(|p| p.id.as_str() == "synthetic-new-99"));
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn load_yaml_file() {
        let dir = tmp_dir("yaml");
        let path = dir.join("provider.yaml");
        let yaml = r"
id: yaml-provider
display_name: YAML Provider
base_url: https://yaml.example.com
auth_style: bearer
protocol: anthropic
model_list:
  - id: model-x
    status: active
defaults:
  default_model: model-x
status: active
";
        std::fs::write(&path, yaml).unwrap();
        let out = load_provider_defs(&path).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id.as_str(), "yaml-provider");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn load_array_json() {
        let dir = tmp_dir("array");
        let path = dir.join("providers.json");
        let json = format!(
            "[{}, {}]",
            single_provider_json("arr-a", "https://arr-a.example.com"),
            single_provider_json("arr-b", "https://arr-b.example.com")
        );
        std::fs::write(&path, json).unwrap();
        let out = load_provider_defs(&path).unwrap();
        assert_eq!(out.len(), 2);
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn validation_rejects_duplicate_model_ids() {
        let json = r#"{
  "id": "dup-model-prov",
  "base_url": "https://dup.example.com",
  "auth_style": "bearer",
  "model_list": [
    {"id": "dup", "status": "active"},
    {"id": "dup", "status": "active"}
  ],
  "defaults": {"default_model": "dup"}
}"#;
        let def: ProviderDefinition = serde_json::from_str(json).unwrap();
        def.validate().unwrap_err();
    }

    #[test]
    fn validation_rejects_missing_default() {
        let json = r#"{
  "id": "missing-default",
  "base_url": "https://missing.example.com",
  "auth_style": "bearer",
  "model_list": [
    {"id": "a", "status": "active"}
  ],
  "defaults": {"default_model": "not-there"}
}"#;
        let def: ProviderDefinition = serde_json::from_str(json).unwrap();
        def.validate().unwrap_err();
    }

    #[test]
    fn validation_rejects_retired_default_for_active_provider() {
        let json = r#"{
  "id": "retired-default",
  "base_url": "https://retired.example.com",
  "auth_style": "bearer",
  "status": "active",
  "model_list": [
    {"id": "old", "status": "retired"}
  ],
  "defaults": {"default_model": "old"}
}"#;
        let def: ProviderDefinition = serde_json::from_str(json).unwrap();
        def.validate().unwrap_err();
    }

    #[test]
    fn duplicate_normalized_url_rejected() {
        let dir = tmp_dir("dup-url");
        std::fs::write(
            dir.join("a.json"),
            single_provider_json("dup-url-a", "https://dup.example.com/"),
        )
        .unwrap();
        std::fs::write(
            dir.join("b.json"),
            single_provider_json("dup-url-b", "https://dup.example.com"),
        )
        .unwrap();
        load_provider_defs(&dir).unwrap_err();
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn duplicate_id_case_fold_rejected() {
        let dir = tmp_dir("dup-id");
        std::fs::write(
            dir.join("a.json"),
            single_provider_json("DupID", "https://a.example.com"),
        )
        .unwrap();
        std::fs::write(
            dir.join("b.json"),
            single_provider_json("dupid", "https://b.example.com"),
        )
        .unwrap();
        load_provider_defs(&dir).unwrap_err();
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn health_probe_valid_and_invalid() {
        let valid = def("test-valid", "https://api.example.com");
        let ok = health_probe(&valid);
        assert!(ok.valid, "expected valid for https url: {}", ok.reason);

        let invalid = def("test-invalid", "file:///etc/passwd");
        let bad = health_probe(&invalid);
        assert!(!bad.valid);
        assert!(!bad.reason.is_empty());

        let url_only = health_probe_url("https://api.example.com/v1");
        assert!(url_only.valid);
        let url_bad = health_probe_url("ftp://example.com");
        assert!(!url_bad.valid);
    }

    #[test]
    fn health_probe_allows_localhost() {
        let local = ProviderDefinition {
            auth_style: AuthStyle::None,
            protocol: Protocol::Other,
            ..def("local-prov", "http://localhost:8080")
        };
        let r = health_probe(&local);
        assert!(r.valid, "localhost should be valid: {}", r.reason);
    }

    #[test]
    fn health_probe_rejects_spaces_and_controls() {
        assert!(!health_probe_url("https://api.example .com").valid);
        assert!(!health_probe_url("https://api.example.com\n").valid);
        assert!(!health_probe_url("not-a-url").valid);
        assert!(!health_probe_url("").valid);
    }

    #[test]
    fn parse_preserves_no_secret() {
        // Ensure secret never appears in debug/serialize of provider (no secret field exists).
        let json = single_provider_json("no-secret", "https://api.example.com");
        let def: ProviderDefinition = serde_json::from_str(&json).unwrap();
        let debug = format!("{def:?}");
        assert!(!debug.contains("sk-"));
        let ser = serde_json::to_string(&def).unwrap();
        assert!(!ser.contains("sk-"));
    }

    #[test]
    fn load_nonexistent_path_errors() {
        let p = PathBuf::from("/tmp/superai-nonexistent-xyz-9999/nope.json");
        load_provider_defs(&p).unwrap_err();
    }

    #[test]
    fn no_hardcoded_provider_list() {
        // Loading from empty dir yields empty vec — no built-in providers injected.
        let dir = tmp_dir("empty");
        let out = load_provider_defs(&dir).unwrap();
        assert!(
            out.is_empty(),
            "empty dir must yield empty, not hardcoded list"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn round_trip_serialization() {
        let json = single_provider_json("round-trip", "https://rt.example.com");
        let def: ProviderDefinition = serde_json::from_str(&json).unwrap();
        let back_json = serde_json::to_string(&def).unwrap();
        let back: ProviderDefinition = serde_json::from_str(&back_json).unwrap();
        assert_eq!(def, back);
        let yaml = yaml_serde::to_string(&def).unwrap();
        let from_yaml: ProviderDefinition = yaml_serde::from_str(&yaml).unwrap();
        assert_eq!(def.id, from_yaml.id);
        assert_eq!(def.base_url, from_yaml.base_url);
    }

    #[test]
    fn dir_ignores_non_provider_files() {
        let dir = tmp_dir("ignore-non");
        std::fs::write(dir.join("readme.md"), "# hello").unwrap();
        std::fs::write(
            dir.join("good.json"),
            single_provider_json("good-one", "https://good.example.com"),
        )
        .unwrap();
        let out = load_provider_defs(&dir).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id.as_str(), "good-one");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn missing_file_has_valid_error() {
        let dir = tmp_dir("bad-json");
        let path = dir.join("bad.json");
        std::fs::write(&path, "{ not valid json").unwrap();
        load_provider_defs(&path).unwrap_err();
        drop(std::fs::remove_dir_all(&dir));
    }

    // -----------------------------------------------------------------------
    // Bundled providers data-driven tests
    // -----------------------------------------------------------------------

    #[test]
    fn bundled_providers_load_examples() {
        let bundled = load_bundled_providers().unwrap();
        assert!(
            bundled.len() >= 3,
            "expected at least 3 bundled providers, got {}",
            bundled.len()
        );
        let ids: Vec<String> = bundled.iter().map(|p| p.id.as_str().to_owned()).collect();
        for expected in ["anthropic", "glm", "minimax"] {
            assert!(
                ids.iter().any(|id| id == expected),
                "bundled missing {expected}: {ids:?}"
            );
        }
        for p in &bundled {
            assert!(
                p.validate().is_ok(),
                "bundled provider {} failed validation",
                p.id
            );
        }
        // Ensure no secret VALUES in the bundled file. The placeholder/prefix
        // policy strings legitimately contain `sk-...` shapes; a secret value
        // is `sk-` followed by a long token run.
        let secret_value = |text: &str| contains_secret_shaped_value(text);
        let raw = BUNDLED_PROVIDERS_JSON;
        assert!(
            !secret_value(raw),
            "bundled json must not contain secret values"
        );
        let debug = format!("{bundled:?}");
        assert!(!secret_value(&debug), "bundled debug must not leak secrets");
        let ser = serde_json::to_string(&bundled).unwrap();
        assert!(
            !secret_value(&ser),
            "bundled serialized must not contain secret values"
        );
        // Placeholder policy survives the round trip (it is data, not a secret).
        let anthropic = bundled
            .iter()
            .find(|p| p.id.as_str() == "anthropic")
            .expect("anthropic bundled");
        assert_eq!(anthropic.auth.key_prefix.as_deref(), Some("sk-ant-"));
    }

    #[test]
    fn bundled_plus_dummy_via_file_no_code_change() {
        let bundled = load_bundled_providers().unwrap();
        let base_len = bundled.len();
        let dir = tmp_dir("bundled-plus-dummy");
        let dummy_json = single_provider_json("dummy-provider-999", "https://dummy.example.com");
        std::fs::write(dir.join("dummy.json"), &dummy_json).unwrap();
        let merged = load_bundled_plus_extra(&dir).unwrap();
        assert_eq!(merged.len(), base_len + 1);
        assert!(merged.iter().any(|p| p.id.as_str() == "dummy-provider-999"));
        // Duplicate across bundled and extra should be rejected (same id)
        let dup_path = dir.join("dup-dummy.json");
        let dup_json = single_provider_json("anthropic", "https://dup.example.com");
        std::fs::write(&dup_path, dup_json).unwrap();
        let err = load_bundled_plus_extra(&dir).unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.to_ascii_lowercase().contains("duplicate"),
            "expected duplicate error, got {msg}"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    // -----------------------------------------------------------------------
    // Health probe enhanced — bounded, redacted, classified via fake harness
    // -----------------------------------------------------------------------

    #[test]
    fn health_bounded_timeout_and_private_policy() {
        let prov = def("test-health-bounded", "https://api.example.com");
        // Valid config should be healthy
        let good = crate::health::HealthConfig::default();
        let res = crate::health::health_probe(&prov, &good);
        assert!(res.valid);
        assert_eq!(res.status, crate::failure::HealthStatus::Healthy);
        // Private host with bearer should fail when deny, succeed when allow
        let local = ProviderDefinition {
            protocol: Protocol::Other,
            ..def("local-bearer", "http://localhost:8080")
        };
        let deny = crate::health::HealthConfig {
            allow_private_network: false,
            ..crate::health::HealthConfig::default()
        };
        let allow = crate::health::HealthConfig {
            allow_private_network: true,
            ..crate::health::HealthConfig::default()
        };
        let r_deny = crate::health::health_probe(&local, &deny);
        assert!(
            !r_deny.valid,
            "private should be rejected: {}",
            r_deny.reason
        );
        let r_allow = crate::health::health_probe(&local, &allow);
        assert!(
            r_allow.valid,
            "private allowed should be valid: {}",
            r_allow.reason
        );
        // Timeout bounded
        assert!(crate::health::validate_timeout(Duration::from_millis(500)).is_err());
        assert!(crate::health::validate_timeout(Duration::from_secs(5)).is_ok());
        assert!(crate::health::validate_timeout(Duration::from_secs(31)).is_err());
    }

    #[test]
    fn health_redacts_secrets_and_classifies() {
        let prov = ProviderDefinition {
            base_url: "https://api.example.com?api_key=sk-superai-test-sentinel-12345-fake&foo=bar"
                .to_owned(),
            ..def("test-redact", "https://api.example.com")
        };
        let cfg = crate::health::HealthConfig::default();
        let res = crate::health::health_probe(&prov, &cfg);
        assert!(
            !res.base_url_redacted
                .contains("sk-superai-test-sentinel-12345-fake")
        );
        assert!(res.base_url_redacted.contains("[REDACTED]"));
        assert!(!res.reason.contains("sk-superai-test-sentinel-12345-fake"));

        // Mock harness classification
        let sentinel = "sk-superai-test-sentinel-12345-fake";
        let body_with_sentinel = format!("rate limit {sentinel}");
        let mock_res =
            crate::health::health_probe_with_mock(&prov, &cfg, 429, &body_with_sentinel, None);
        assert_eq!(mock_res.status, crate::failure::HealthStatus::RateLimited);
        assert!(
            !mock_res.reason.contains(sentinel),
            "mock reason leaked: {}",
            mock_res.reason
        );

        // TLS, auth, etc.
        let tls =
            crate::health::health_probe_with_mock(&prov, &cfg, 200, "tls certificate error", None);
        assert_eq!(tls.status, crate::failure::HealthStatus::TlsError);
        let auth = crate::health::health_probe_with_mock(&prov, &cfg, 401, "unauthorized", None);
        assert_eq!(auth.status, crate::failure::HealthStatus::AuthError);
        // Oversized
        let big = "x".repeat(cfg.max_bytes + 1);
        let over = crate::health::health_probe_with_mock(&prov, &cfg, 200, &big, None);
        assert_eq!(over.status, crate::failure::HealthStatus::Oversized);
    }

    #[test]
    fn health_redirect_strips_auth_cross_host() {
        let prov = def("test-redirect", "https://api.example.com");
        let cfg = crate::health::HealthConfig::default();
        let cross = crate::health::health_probe_with_mock(
            &prov,
            &cfg,
            302,
            "redirect",
            Some("https://evil.example.com/other"),
        );
        assert!(cross.stripped_auth_on_redirect);
        let same = crate::health::health_probe_with_mock(
            &prov,
            &cfg,
            302,
            "redirect",
            Some("https://api.example.com/other"),
        );
        assert!(!same.stripped_auth_on_redirect);
        assert!(crate::failure::should_strip_auth_for_redirect(
            "https://a.com/x",
            "https://b.com/y"
        ));
        assert!(!crate::failure::should_strip_auth_for_redirect(
            "https://a.com/x",
            "https://a.com/y"
        ));
    }

    // -----------------------------------------------------------------------
    // API-key placement — ephemeral, sink-restricted, redacted
    // -----------------------------------------------------------------------

    #[test]
    fn api_key_placement_only_to_declared_sink_and_redacted() {
        let dir = tmp_dir("api-key-sink");
        let inst = sample_instance(&dir, "work-sink");
        // Hermetic version gate: `commit_api_key` enforces the adapter's
        // version resolution, which otherwise probes whatever `claude` happens
        // to be on PATH (none on CI). Pin a fake binary that answers
        // `--version` so the gate sees a compatible harness deterministically.
        let bin_dir = dir.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let fake_claude = write_fake_claude(&bin_dir);
        let adapter =
            crate::adapters::claude_code::ClaudeCodeAdapter::with_configured_binary(fake_claude)
                .unwrap();
        let provider = def("test-prov-key", "https://api.example.com");
        let sentinel = crate::abuse::SENTINEL;
        let key = RedactedString::new(sentinel);

        // Resolve sink: must be config field for Claude Code
        let sink = resolve_api_key_sink(&adapter).unwrap();
        assert_eq!(sink.kind, ApiKeySinkKind::ConfigField);
        assert!(
            sink.surface_id.contains("settings.json"),
            "expected settings.json sink, got {}",
            sink.surface_id
        );
        assert!(!sink.selector.is_empty());

        // Preview must be redacted, never contain sentinel
        let preview = preview_api_key_placement(&key, &provider, &adapter, &inst).unwrap();
        let preview_json = serde_json::to_string(&preview).unwrap();
        let preview_dbg = format!("{preview:?}");
        for out in [&preview_json, &preview_dbg] {
            assert!(!out.contains(sentinel), "preview leaked sentinel: {out}");
            assert!(
                out.contains("[REDACTED]")
                    || out.contains("config_field")
                    || out.contains("settings.json"),
                "preview should contain redacted placeholder"
            );
        }
        assert_eq!(preview.redacted, "[REDACTED]");
        assert!(!preview.destination.contains(sentinel));

        // Commit must write only to sink, not to registry, and must be redacted in preview/result
        let commit_preview = commit_api_key(&key, &provider, &adapter, &inst).unwrap();
        let commit_json = serde_json::to_string(&commit_preview).unwrap();
        assert!(!commit_json.contains(sentinel));
        assert!(!format!("{commit_preview:?}").contains(sentinel));

        // Destination file must contain secret (allowed) but preview/result never does
        let dest_path = inst.config_root.as_path().join(&sink.surface_id);
        assert!(
            dest_path.exists(),
            "sink file should exist at {}",
            dest_path.display()
        );
        let dest_bytes = std::fs::read(&dest_path).unwrap();
        assert!(
            String::from_utf8_lossy(&dest_bytes).contains(sentinel),
            "dest should contain sentinel (allowed sink)"
        );

        // But registry must not contain sentinel
        let reg_path = dir.join("registry.json");
        let mut reg = crate::registry::Registry::default();
        reg.insert(inst).unwrap();
        reg.store(&reg_path).unwrap();
        let reg_bytes = std::fs::read(&reg_path).unwrap();
        assert!(
            !String::from_utf8_lossy(&reg_bytes).contains(sentinel),
            "registry leaked sentinel"
        );
        assert!(!format!("{reg:?}").contains(sentinel));

        // Backup exists and backup catalog does not leak (catalog debug)
        let backups = superai_config::backup::list_backups(&dest_path).unwrap();
        assert!(!format!("{backups:?}").contains(sentinel));

        // Ensure permissions are restrictive on unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let perm = std::fs::metadata(&dest_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(perm, 0o600, "dest permissions should be 600, got {perm:o}");
        }

        // ApiKey debug must be redacted
        assert!(!format!("{key:?}").contains(sentinel));
        assert_eq!(format!("{key}"), "[REDACTED]");

        // Check that writing literal to wrapper is not allowed as sink
        // (resolve would not return wrapper literal; committing via that kind should error)
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn api_key_never_to_registry_or_logs_via_template_validation() {
        let patch = crate::template::OwnedPatch {
            selector: "key:api_key".to_owned(),
            value: serde_json::json!(crate::abuse::SENTINEL),
        };
        let err = patch.validate().unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            !msg.contains(crate::abuse::SENTINEL),
            "template error leaked sentinel"
        );
    }

    #[test]
    fn api_key_validation_rejects_empty_and_controls() {
        let prov = def("test-key-val", "https://api.example.com");
        assert!(validate_api_key_value(&prov, "").is_err());
        assert!(validate_api_key_value(&prov, "   ").is_err());
        assert!(validate_api_key_value(&prov, "valid-key-123").is_ok());
        assert!(validate_api_key_value(&prov, "key\nwith\nnewline").is_err());
    }

    #[test]
    fn health_probe_redacted_url_preserves_non_secret() {
        let url = "https://api.example.com/v1/models?api_key=sk-superai-test-sentinel-12345-fake&model=foo";
        let redacted = crate::health::redact_url(url);
        assert!(!redacted.contains("sk-superai-test-sentinel-12345-fake"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(redacted.contains("model=foo"));
        let headers = {
            let mut m = BTreeMap::new();
            m.insert(
                "Authorization".to_owned(),
                "Bearer sk-superai-test-sentinel-12345-fake".to_owned(),
            );
            m.insert("Content-Type".to_owned(), "application/json".to_owned());
            m
        };
        let redacted_h = crate::health::redact_headers(&headers);
        assert_eq!(
            redacted_h.get("Authorization").map(String::as_str),
            Some("[REDACTED]")
        );
        assert_eq!(
            redacted_h.get("Content-Type").map(String::as_str),
            Some("application/json")
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "comprehensive health polish covers data-driven, bounded, redacted, classify, redirect in one test"
    )]
    fn health_polish_comprehensive_data_driven_bounded_redacted_classified_and_redirect_stripping()
    {
        // Data-driven: synthetic provider loaded from file, no Rust edit.
        let dir = tmp_dir("health-polish");
        let json = single_provider_json("synthetic-health-polish", "https://api.example.com");
        let path = dir.join("synthetic-health-polish.json");
        std::fs::write(&path, &json).unwrap();
        let loaded = load_provider_defs(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        let prov = &loaded[0];
        assert_eq!(prov.id.as_str(), "synthetic-health-polish");

        // Bounded timeout: valid bounds succeed, out-of-bounds fail.
        assert!(
            crate::health::HealthConfig::new(
                crate::health::HealthProbeKind::HttpStatus,
                Duration::from_secs(5),
                1024,
                false
            )
            .is_ok()
        );
        assert!(
            crate::health::HealthConfig::new(
                crate::health::HealthProbeKind::HttpStatus,
                Duration::from_millis(500),
                1024,
                false
            )
            .is_err()
        );
        assert!(
            crate::health::HealthConfig::new(
                crate::health::HealthProbeKind::HttpStatus,
                Duration::from_secs(31),
                1024,
                false
            )
            .is_err()
        );
        let cfg = crate::health::HealthConfig::default();
        let res = crate::health::health_probe(prov, &cfg);
        assert!(
            res.valid,
            "synthetic provider should be valid: {}",
            res.reason
        );
        assert_eq!(res.timeout_ms, 5000);

        // Redacted: query secret never appears in result.
        let secret_url =
            "https://api.example.com?api_key=sk-superai-test-sentinel-12345-fake&model=foo";
        let secret_prov = ProviderDefinition {
            base_url: secret_url.to_owned(),
            ..def("synthetic-redacted", "https://api.example.com")
        };
        let redacted_res = crate::health::health_probe(&secret_prov, &cfg);
        assert!(
            !redacted_res
                .base_url_redacted
                .contains("sk-superai-test-sentinel-12345-fake")
        );
        assert!(redacted_res.base_url_redacted.contains("[REDACTED]"));
        assert!(redacted_res.base_url_redacted.contains("model=foo"));

        // Classify auth / rate-limit / TLS via mock harness.
        let ok = crate::health::health_probe_with_mock(prov, &cfg, 200, "all good", None);
        assert_eq!(ok.status, crate::failure::HealthStatus::Healthy);
        assert!(ok.valid);
        let rate =
            crate::health::health_probe_with_mock(prov, &cfg, 429, "rate limit exceeded", None);
        assert_eq!(rate.status, crate::failure::HealthStatus::RateLimited);
        assert!(!rate.valid);
        let auth = crate::health::health_probe_with_mock(prov, &cfg, 401, "unauthorized", None);
        assert_eq!(auth.status, crate::failure::HealthStatus::AuthError);
        assert!(!auth.valid);
        let tls = crate::health::health_probe_with_mock(
            prov,
            &cfg,
            200,
            "tls certificate verify failed",
            None,
        );
        assert_eq!(tls.status, crate::failure::HealthStatus::TlsError);
        assert!(!tls.valid);

        // Cross-host redirect stripping.
        let cross = crate::health::health_probe_with_mock(
            prov,
            &cfg,
            302,
            "redirect",
            Some("https://evil.example.com/other"),
        );
        assert!(cross.stripped_auth_on_redirect);
        let same = crate::health::health_probe_with_mock(
            prov,
            &cfg,
            302,
            "redirect",
            Some("https://api.example.com/other"),
        );
        assert!(!same.stripped_auth_on_redirect);
        assert!(crate::failure::should_strip_auth_for_redirect(
            "https://a.example.com/x",
            "https://b.example.com/y"
        ));
        assert!(!crate::failure::should_strip_auth_for_redirect(
            "https://a.example.com/x",
            "https://a.example.com/y"
        ));

        // Sentinel never leaks in reason.
        let sentinel = crate::abuse::SENTINEL;
        let body_with_sentinel = format!("rate limit {sentinel}");
        let leaked =
            crate::health::health_probe_with_mock(prov, &cfg, 429, &body_with_sentinel, None);
        assert!(!leaked.reason.contains(sentinel));

        drop(std::fs::remove_dir_all(&dir));
    }

    // -----------------------------------------------------------------------
    // PRV-01/02 field completeness + validation
    // -----------------------------------------------------------------------

    #[test]
    fn schema_version_mismatch_rejected() {
        let mut def = ProviderDefinition::new(
            ProviderId::new("schema-mismatch").unwrap(),
            "https://api.example.com",
        );
        def.schema_version = 2;
        let err = def.validate().unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("schema_version"), "got: {msg}");
    }

    #[test]
    fn endpoint_variants_validate_and_fall_back() {
        let mut def = ProviderDefinition::new(
            ProviderId::new("endpoint-prov").unwrap(),
            "https://api.example.com/v1",
        );
        def.endpoints = vec![EndpointVariant {
            name: "eu".to_owned(),
            base_url: "https://eu.api.example.com/v1".to_owned(),
            protocols: vec![Protocol::OpenAiChat],
        }];
        def.validate().unwrap();
        // Named variant resolves; unknown variant falls back to the default.
        assert_eq!(
            def.endpoint_for(Some("EU")),
            "https://eu.api.example.com/v1"
        );
        assert_eq!(
            def.endpoint_for(Some("missing")),
            "https://api.example.com/v1"
        );
        assert_eq!(def.endpoint_for(None), "https://api.example.com/v1");
        // Protocol selection picks the variant advertising the protocol.
        assert_eq!(
            def.endpoint_for_protocol(Protocol::OpenAiChat),
            Some("https://eu.api.example.com/v1")
        );
        assert_eq!(def.endpoint_for_protocol(Protocol::Anthropic), None);
        def.protocol = Protocol::Anthropic;
        assert_eq!(
            def.endpoint_for_protocol(Protocol::Anthropic),
            Some("https://api.example.com/v1")
        );

        // Duplicate variant names rejected.
        def.endpoints.push(EndpointVariant {
            name: "EU".to_owned(),
            base_url: "https://eu2.api.example.com".to_owned(),
            protocols: vec![],
        });
        assert!(
            def.validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate endpoint")
        );

        // Duplicate normalized URL vs the default endpoint rejected.
        def.endpoints = vec![EndpointVariant {
            name: "same".to_owned(),
            base_url: "https://api.example.com/v1/".to_owned(),
            protocols: vec![],
        }];
        assert!(
            def.validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate normalized")
        );

        // Invalid variant URL rejected.
        def.endpoints = vec![EndpointVariant {
            name: "bad".to_owned(),
            base_url: "ftp://nope".to_owned(),
            protocols: vec![],
        }];
        assert!(def.validate().is_err());
    }

    #[test]
    fn header_and_param_validation() {
        let mut def = ProviderDefinition::new(
            ProviderId::new("headers-prov").unwrap(),
            "https://api.example.com",
        );
        def.required_headers
            .insert("x-version".to_owned(), "1".to_owned());
        def.optional_headers
            .insert("X-Version".to_owned(), "2".to_owned());
        let err = def.validate().unwrap_err().to_string();
        assert!(err.contains("both required and optional"), "got: {err}");

        def.optional_headers.clear();
        def.optional_headers
            .insert("bad name".to_owned(), "v".to_owned());
        assert!(def.validate().is_err());

        def.optional_headers.clear();
        def.request_params
            .insert("api_key".to_owned(), Value::String("x".to_owned()));
        let err = def.validate().unwrap_err().to_string();
        assert!(err.contains("secret-shaped"), "got: {err}");

        def.request_params.clear();
        def.request_params
            .insert("max_tokens".to_owned(), Value::from(1024u32));
        def.validate().unwrap();
    }

    #[test]
    fn auth_inputs_validation_and_prefix_policy() {
        let mut def = ProviderDefinition::new(
            ProviderId::new("auth-prov").unwrap(),
            "https://api.example.com",
        );
        def.auth.env_var_names = vec!["9BAD".to_owned()];
        assert!(def.validate().unwrap_err().to_string().contains("env var"));

        def.auth.env_var_names = vec!["GOOD_NAME_1".to_owned()];
        def.auth.key_prefix = Some("pk-".to_owned());
        def.validate().unwrap();
        // Prefix policy enforced at key validation; the key never echoes.
        assert!(validate_api_key_value(&def, "sk-wrongprefix").is_err());
        let err = validate_api_key_value(&def, "sk-wrongprefix-long-value").unwrap_err();
        let msg = format!("{err} {err:?}");
        assert!(!msg.contains("sk-wrongprefix"), "error leaked key: {msg}");
        assert!(validate_api_key_value(&def, "pk-correct").is_ok());
        // No documented prefix -> no prefix enforcement.
        def.auth.key_prefix = None;
        assert!(validate_api_key_value(&def, "anything-goes").is_ok());
    }

    #[test]
    fn probe_definitions_validate() {
        let mut def = ProviderDefinition::new(
            ProviderId::new("probe-prov").unwrap(),
            "https://api.example.com",
        );
        def.auth_style = AuthStyle::Bearer;
        def.health_probes = vec![ProbeDefinition {
            id: "models".to_owned(),
            kind: crate::health::HealthProbeKind::ModelList,
            path_suffix: "/v1/models".to_owned(),
            method: Some("GET".to_owned()),
            headers: BTreeMap::new(),
            body_template: None,
            uses_auth: true,
            timeout_ms: Some(5000),
            max_response_bytes: Some(2048),
            accepted_status: vec![200],
            body_contains: Some("data".to_owned()),
            allow_private_network: false,
            rate_cost_warning: Some("counted against limits".to_owned()),
        }];
        def.validate().unwrap();

        // Duplicate ids.
        def.health_probes.push(def.health_probes[0].clone());
        assert!(
            def.validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate probe")
        );
        def.health_probes.pop();

        // Bad method.
        def.health_probes[0].method = Some("DELETE".to_owned());
        assert!(
            def.validate()
                .unwrap_err()
                .to_string()
                .contains("GET/HEAD/POST")
        );
        def.health_probes[0].method = None;

        // Path suffix without leading slash.
        def.health_probes[0].path_suffix = "v1/models".to_owned();
        assert!(
            def.validate()
                .unwrap_err()
                .to_string()
                .contains("path_suffix")
        );
        def.health_probes[0].path_suffix = "/v1/models?x=1".to_owned();
        assert!(
            def.validate()
                .unwrap_err()
                .to_string()
                .contains("path_suffix")
        );
        def.health_probes[0].path_suffix = "/v1/models".to_owned();

        // Timeout out of bounds.
        def.health_probes[0].timeout_ms = Some(500);
        assert!(def.validate().unwrap_err().to_string().contains("timeout"));
        def.health_probes[0].timeout_ms = Some(30_001);
        assert!(def.validate().unwrap_err().to_string().contains("timeout"));
        def.health_probes[0].timeout_ms = Some(5000);

        // Empty accepted status.
        def.health_probes[0].accepted_status = vec![];
        assert!(
            def.validate()
                .unwrap_err()
                .to_string()
                .contains("accepted_status")
        );
        def.health_probes[0].accepted_status = vec![200, 777];
        assert!(def.validate().is_err());
        def.health_probes[0].accepted_status = vec![200];

        // Auth reference without an auth style.
        def.auth_style = AuthStyle::None;
        assert!(
            def.validate()
                .unwrap_err()
                .to_string()
                .contains("references auth")
        );
    }

    #[test]
    fn model_limits_and_modalities_validate() {
        let mut def = ProviderDefinition::new(
            ProviderId::new("limits-prov").unwrap(),
            "https://api.example.com",
        );
        def.model_list = vec![ModelInfo {
            id: "m".to_owned(),
            display_name: None,
            status: ModelStatus::Active,
            alias: None,
            health_eligible: true,
            limits: ModelLimits {
                context_tokens: Some(1000),
                input_tokens: Some(600),
                output_tokens: Some(600),
            },
            input_modalities: vec![],
            output_modalities: vec![],
            supports_tools: false,
            supports_reasoning: false,
        }];
        let err = def.validate().unwrap_err().to_string();
        assert!(err.contains("input+output"), "got: {err}");

        def.model_list[0].limits.output_tokens = Some(400);
        def.model_list[0].limits.context_tokens = Some(0);
        assert!(
            def.validate()
                .unwrap_err()
                .to_string()
                .contains("must be positive")
        );

        def.model_list[0].limits.context_tokens = Some(1000);
        def.model_list[0].supports_tools = true;
        def.model_list[0].input_modalities = vec![Modality::Image];
        let err = def.validate().unwrap_err().to_string();
        assert!(err.contains("no text input modality"), "got: {err}");

        def.model_list[0].input_modalities = vec![Modality::Text, Modality::Image];
        def.validate().unwrap();
    }

    #[test]
    fn default_model_must_be_active_for_active_provider() {
        let mut def = ProviderDefinition::new(
            ProviderId::new("default-status").unwrap(),
            "https://api.example.com",
        );
        def.model_list = vec![ModelInfo {
            id: "preview-m".to_owned(),
            display_name: None,
            status: ModelStatus::Preview,
            alias: None,
            health_eligible: false,
            limits: ModelLimits::default(),
            input_modalities: vec![],
            output_modalities: vec![],
            supports_tools: false,
            supports_reasoning: false,
        }];
        def.defaults.default_model = Some("preview-m".to_owned());
        for status in [ModelStatus::Preview, ModelStatus::Deprecated] {
            def.model_list[0].status = status;
            let err = def.validate().unwrap_err().to_string();
            assert!(err.contains("but provider is not legacy"), "got: {err}");
        }
        // A legacy (deprecated) provider may keep a legacy default.
        def.status = ProviderStatus::Deprecated;
        def.model_list[0].status = ModelStatus::Deprecated;
        def.validate().unwrap();
    }

    #[test]
    fn verified_at_format_enforced() {
        let mut def = ProviderDefinition::new(
            ProviderId::new("verified-prov").unwrap(),
            "https://api.example.com",
        );
        def.verified_at = Some("2026/08/31".to_owned());
        assert!(
            def.validate()
                .unwrap_err()
                .to_string()
                .contains("YYYY-MM-DD")
        );
        def.verified_at = Some("2026-08-31".to_owned());
        def.validate().unwrap();
    }

    #[test]
    fn bundled_providers_carry_complete_operational_data() {
        let bundled = load_bundled_providers().unwrap();
        for p in &bundled {
            assert_eq!(p.schema_version, PROVIDER_SCHEMA_VERSION);
            assert!(
                p.verified_at.is_some(),
                "provider {} must record a verification date",
                p.id
            );
            assert!(
                !p.auth.env_var_names.is_empty(),
                "provider {} must declare auth env var names",
                p.id
            );
        }
        let glm = bundled
            .iter()
            .find(|p| p.id.as_str() == "glm")
            .expect("glm bundled");
        assert!(
            glm.endpoint_for_protocol(Protocol::Anthropic).is_some(),
            "glm must expose an anthropic-compatible endpoint variant"
        );
        assert!(
            !glm.health_probes.is_empty(),
            "glm must carry at least one probe definition"
        );
        let anthropic = bundled
            .iter()
            .find(|p| p.id.as_str() == "anthropic")
            .expect("anthropic bundled");
        let sonnet = anthropic
            .model_list
            .iter()
            .find(|m| m.id == "claude-sonnet-4-20250514")
            .expect("sonnet 4 in catalog");
        assert!(sonnet.limits.context_tokens.is_some_and(|v| v > 0));
        assert!(sonnet.supports_tools);
        assert!(sonnet.input_modalities.contains(&Modality::Image));
        assert!(!anthropic.capabilities.is_empty());
    }

    #[test]
    fn external_auth_required_for_keychain_only_harness() {
        #[derive(Debug)]
        struct KeychainOnlyAdapter;

        impl Adapter for KeychainOnlyAdapter {
            fn id(&self) -> HarnessId {
                HarnessId::new("keychain-harness").unwrap()
            }
            fn display_name(&self) -> &'static str {
                "Keychain Harness"
            }
            fn product_status(&self) -> ProductStatus {
                ProductStatus::Active
            }
            fn supported_platforms(&self) -> Vec<Platform> {
                Vec::new()
            }
            fn adapter_revision(&self) -> &'static str {
                "0.1.0"
            }
            fn research_doc_link(&self) -> &'static str {
                "docs/harness-configs/keychain-harness.md"
            }
            fn last_verified_date(&self) -> &'static str {
                "2026-08-25"
            }
            fn detection(&self) -> DetectionResult {
                DetectionResult::absent(vec!["test".to_owned()])
            }
            fn version_resolution(&self) -> VersionResolution {
                VersionResolution::unknown()
            }
            fn config_surfaces(&self) -> Vec<ConfigSurface> {
                vec![ConfigSurface::new(
                    "keychain",
                    PathResolver::fallback_only("keychain"),
                    DocumentKind::Keychain,
                    ConfigScope::User,
                    SurfaceOwnership::ExternalSecretStore,
                )]
            }
            fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
                Vec::new()
            }
            fn plan_mirror_exclusions(&self) -> Vec<String> {
                Vec::new()
            }
            fn plan_wrapper(&self, _instance: &Instance) -> Result<WrapperPlan> {
                Ok(WrapperPlan::new("test"))
            }
            fn scan_candidates(&self) -> Vec<String> {
                Vec::new()
            }
            fn validate_instance(&self, _instance: &Instance) -> Result<()> {
                Ok(())
            }
        }

        let err = external_auth_requirement(&KeychainOnlyAdapter)
            .expect("keychain-only harness must require external auth");
        match err {
            CoreError::ExternalAuthRequired {
                harness,
                instructions,
            } => {
                assert_eq!(harness, "keychain-harness");
                assert!(instructions.contains("login"), "got: {instructions}");
            }
            other => panic!("expected ExternalAuthRequired, got {other:?}"),
        }
    }
}
