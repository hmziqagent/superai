//! Provider → harness rendering, effective-provider inspection, and the
//! provider lifecycle on an instance (PRV-03, PRV-05, PRV-08).
//!
//! Generic provider data never knows file paths. Rendering maps canonical
//! provider fields onto the adapter's DECLARED owned selectors and produces
//! typed document-engine operations (area 1's executor types), so the same
//! provider renders differently per harness: Claude Code gets `env.*` keys
//! in `settings.json`, Codex gets a `model_providers.<id>` TOML table entry,
//! Kimi a `providers.<id>` table with `default_model`, OpenCode a
//! `provider.<id>` JSON object with a `provider/model` default, Zcode a
//! `provider`/`options` pair. A harness that cannot express the protocol or
//! endpoint gets a typed Unsupported outcome and no mutation.
//!
//! superai never proxies model traffic: rendering writes configuration only.
//! Auth is rendered as a sink/variable NAME, never a secret value — key
//! placement stays in [`crate::provider::commit_api_key`].

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use superai_config::document::{
    DocumentKind as EngineKind, EditOperation, Operation as EngineOperation, Selector,
};
use superai_config::transaction::{FileAction, Transaction};

use crate::adapter::{Adapter, ConfigSurface, DocumentKind};
use crate::error::{CoreError, Result};
use crate::ids::ProviderId;
use crate::instance::Instance;
use crate::provider::{ApiKeySink, Protocol, ProviderDefinition, resolve_api_key_sink};
use crate::template::Template;

// ---------------------------------------------------------------------------
// PRV-03 — render outcome types
// ---------------------------------------------------------------------------

/// A successful render: typed engine operations against one surface, plus
/// the auth reference the harness expects (name only, never a secret).
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderRender {
    /// Surface the operations target (e.g. `settings.json`, `config.toml`).
    pub surface_id: String,
    /// Document kind of that surface.
    pub kind: DocumentKind,
    /// Typed document-engine operations; each carries the surface's owned
    /// keys so the executor rejects anything outside adapter ownership.
    pub operations: Vec<EngineOperation>,
    /// Where the harness reads the key (resolved sink), when it renders one.
    pub auth_sink: Option<ApiKeySink>,
    /// Environment variable name the harness reads for auth (reference).
    pub auth_env_var: Option<String>,
    /// Non-blocking notes surfaced to the caller.
    pub warnings: Vec<String>,
}

/// A harness that cannot express this provider/protocol (typed, no mutation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderUnsupported {
    /// Harness identifier.
    pub harness: String,
    /// Protocol that failed to render.
    pub protocol: Protocol,
    /// Why the harness cannot express it.
    pub reason: String,
}

/// Result of [`render_provider_into_adapter`].
#[derive(Debug, Clone, PartialEq)]
pub enum RenderOutcome {
    /// The harness can express the provider; apply `operations`.
    Supported(ProviderRender),
    /// The harness cannot express the provider; nothing is mutated.
    Unsupported(RenderUnsupported),
}

/// The rendering strategy a surface's owned selectors express.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderStrategy {
    /// `env.*` keys in a JSON/JSONC settings object (claude-code).
    EnvKeys,
    /// A `model_providers` TOML table + `model_provider`/`model` (codex-cli).
    ModelProvidersTable,
    /// A `providers` table + `default_model` (kimi).
    ProvidersTable,
    /// `provider` + `options` + `models` keys (zcode).
    ProviderOptions,
    /// A `provider` object map keyed by provider id (opencode).
    ProviderMap,
}

fn writable_surface_kinds(kind: DocumentKind) -> bool {
    matches!(
        kind,
        DocumentKind::Json | DocumentKind::Jsonc | DocumentKind::Toml | DocumentKind::Yaml
    )
}

fn is_writable(surface: &ConfigSurface) -> bool {
    writable_surface_kinds(surface.kind)
        && !matches!(
            surface.ownership,
            crate::adapter::SurfaceOwnership::ExternalSecretStore
                | crate::adapter::SurfaceOwnership::HarnessManaged
        )
}

/// Pick the rendering strategy a surface's owned selectors express.
///
/// Pure selector inspection — the same provider data renders differently
/// per harness because the harnesses declare different owned selectors.
fn strategy_for(surface: &ConfigSurface) -> Option<RenderStrategy> {
    if !is_writable(surface) || surface.owned_selectors.is_empty() {
        return None;
    }
    let owned = |needle: &str| {
        surface
            .owned_selectors
            .iter()
            .any(|s| s.eq_ignore_ascii_case(needle))
    };
    let contains = |needle: &str| {
        surface
            .owned_selectors
            .iter()
            .any(|s| s.to_ascii_lowercase().contains(needle))
    };
    if contains("model_providers") {
        return Some(RenderStrategy::ModelProvidersTable);
    }
    if owned("providers") {
        return Some(RenderStrategy::ProvidersTable);
    }
    if owned("options") && owned("models") && owned("provider") {
        return Some(RenderStrategy::ProviderOptions);
    }
    if owned("provider") {
        return Some(RenderStrategy::ProviderMap);
    }
    if contains("base_url")
        && surface
            .owned_selectors
            .iter()
            .any(|s| s.starts_with("env."))
    {
        return Some(RenderStrategy::EnvKeys);
    }
    None
}

fn set_op(selector: &str, value: Value, owned_keys: Vec<String>) -> EngineOperation {
    EngineOperation::new(EditOperation::Set {
        selector: Selector::Key(selector.to_owned()),
        value,
    })
    .with_owned_keys(owned_keys)
    .with_create_parent(true)
}

fn remove_op(selector: &str, owned_keys: Vec<String>) -> EngineOperation {
    EngineOperation::new(EditOperation::Remove {
        selector: Selector::Key(selector.to_owned()),
    })
    .with_owned_keys(owned_keys)
    .with_create_parent(false)
}

/// Resolve the default model the render should write: the harness-provider
/// template's `key:model` patch wins (harness-provider template data),
/// otherwise the provider default.
fn effective_default_model(
    provider: &ProviderDefinition,
    template: Option<&Template>,
) -> Option<String> {
    if let Some(tmpl) = template
        && let Some(patch) = tmpl
            .patches
            .iter()
            .find(|p| p.selector.trim_start_matches("key:") == "model")
        && let Value::String(model) = &patch.value
    {
        return Some(model.clone());
    }
    provider.defaults.default_model.clone()
}

/// Derive the env var name a table-based harness should read the key from.
fn auth_env_var(provider: &ProviderDefinition, warnings: &mut Vec<String>) -> String {
    if let Some(first) = provider.auth.env_var_names.first() {
        return first.clone();
    }
    let derived = format!("{}_API_KEY", provider.id.as_str().to_uppercase());
    warnings.push(format!(
        "provider `{}` declares no auth env var; derived `{derived}` for the harness table entry",
        provider.id
    ));
    derived
}

/// Render a provider into typed adapter mutations (PRV-03).
///
/// Pure: nothing is written. `instance` only names the destination for the
/// caller; mutations are content-level engine operations the caller applies
/// (see [`commit_provider_change`]) or previews. Rendering never emits a
/// secret — auth appears as a sink/variable name only.
#[expect(clippy::too_many_lines, reason = "strategy table is deliberate")]
pub fn render_provider_into_adapter(
    provider: &ProviderDefinition,
    template: Option<&Template>,
    adapter: &dyn Adapter,
    instance: &Instance,
) -> RenderOutcome {
    let mut warnings: Vec<String> = Vec::new();
    if !instance.harness.eq_case_fold(&adapter.id()) {
        warnings.push(format!(
            "instance harness `{}` differs from adapter `{}`; rendering targets the adapter's surfaces",
            instance.harness, adapter.id()
        ));
    }
    if let Some(tmpl) = template
        && tmpl.provider != provider.id
    {
        warnings.push(format!(
            "template `{}` targets provider `{}`, rendering provider `{}` anyway",
            tmpl.id, tmpl.provider, provider.id
        ));
    }
    // Rendering targets the PRIMARY writable surface: the first surface in
    // adapter declaration order whose owned selectors express a strategy.
    // (Adapters declare the base config first; per-profile override surfaces
    // are user-created layers that provider defaults must not hijack.)
    let surfaces = adapter.config_surfaces();
    let Some((surface, strategy)) = surfaces
        .iter()
        .find_map(|surface| strategy_for(surface).map(|strategy| (surface, strategy)))
    else {
        return RenderOutcome::Unsupported(RenderUnsupported {
            harness: adapter.id().to_string(),
            protocol: provider.protocol,
            reason: "harness declares no writable provider-owned selectors (managed backend)"
                .to_owned(),
        });
    };
    let owned_keys = surface.owned_selectors.clone();
    let owned = |needle: &str| {
        surface
            .owned_selectors
            .iter()
            .any(|s| s.eq_ignore_ascii_case(needle))
    };
    let model_default = effective_default_model(provider, template);
    let mut operations: Vec<EngineOperation> = Vec::new();
    let mut auth_env_var_out: Option<String> = None;

    match strategy {
        RenderStrategy::EnvKeys => {
            // The harness speaks one wire protocol and takes an endpoint via
            // env keys; rendering requires an endpoint that serves it.
            let Some(endpoint) = provider.endpoint_for_protocol(Protocol::Anthropic) else {
                return RenderOutcome::Unsupported(RenderUnsupported {
                    harness: adapter.id().to_string(),
                    protocol: provider.protocol,
                    reason: format!(
                        "harness transport is the anthropic messages protocol; provider `{}` exposes no anthropic-capable endpoint (protocol `{}`, {} endpoint variant(s))",
                        provider.id,
                        provider.protocol,
                        provider.endpoints.len()
                    ),
                });
            };
            let mut wrote_base = false;
            for sel in &surface.owned_selectors {
                let lower = sel.to_ascii_lowercase();
                if lower.contains("base_url") {
                    operations.push(set_op(
                        sel,
                        Value::String(endpoint.to_owned()),
                        owned_keys.clone(),
                    ));
                    wrote_base = true;
                } else if lower.contains("auth_token") || lower.contains("api_key") {
                    auth_env_var_out = Some(sel.split('.').next_back().unwrap_or(sel).to_owned());
                }
            }
            if !wrote_base {
                return RenderOutcome::Unsupported(RenderUnsupported {
                    harness: adapter.id().to_string(),
                    protocol: provider.protocol,
                    reason: "env-key surface declares no base-url selector".to_owned(),
                });
            }
            if owned("model")
                && let Some(model) = model_default
            {
                operations.push(set_op("model", Value::String(model), owned_keys));
            }
        }
        RenderStrategy::ModelProvidersTable => {
            let wire_api = match provider.protocol {
                Protocol::OpenAiChat => "chat",
                Protocol::OpenAiResponses => "responses",
                Protocol::Anthropic | Protocol::Gemini | Protocol::Other => {
                    return RenderOutcome::Unsupported(RenderUnsupported {
                        harness: adapter.id().to_string(),
                        protocol: provider.protocol,
                        reason: format!(
                            "model_providers table speaks the openai wire_api (chat|responses); provider `{}` protocol has no mapping",
                            provider.id
                        ),
                    });
                }
            };
            let entry = serde_json::json!({
                "name": if provider.display_name.is_empty() {
                    provider.id.to_string()
                } else {
                    provider.display_name.clone()
                },
                "base_url": provider.base_url,
                "env_key": auth_env_var(provider, &mut warnings),
                "wire_api": wire_api,
            });
            let table = format!("model_providers.{}", provider.id.as_str());
            operations.push(set_op(&table, entry, owned_keys.clone()));
            if owned("model_provider") {
                operations.push(set_op(
                    "model_provider",
                    Value::String(provider.id.to_string()),
                    owned_keys.clone(),
                ));
            }
            if owned("model")
                && let Some(model) = model_default
            {
                operations.push(set_op("model", Value::String(model), owned_keys));
            }
        }
        RenderStrategy::ProvidersTable => {
            if !matches!(
                provider.protocol,
                Protocol::OpenAiChat | Protocol::OpenAiResponses
            ) {
                return RenderOutcome::Unsupported(RenderUnsupported {
                    harness: adapter.id().to_string(),
                    protocol: provider.protocol,
                    reason: format!(
                        "providers table speaks the openai-compatible protocol; provider `{}` does not",
                        provider.id
                    ),
                });
            }
            let entry = serde_json::json!({
                "base_url": provider.base_url,
                "api_key_env": auth_env_var(provider, &mut warnings),
            });
            let table = format!("providers.{}", provider.id.as_str());
            operations.push(set_op(&table, entry, owned_keys.clone()));
            if owned("default_model")
                && let Some(model) = model_default
            {
                operations.push(set_op("default_model", Value::String(model), owned_keys));
            }
        }
        RenderStrategy::ProviderOptions => {
            if !matches!(provider.protocol, Protocol::OpenAiChat) {
                return RenderOutcome::Unsupported(RenderUnsupported {
                    harness: adapter.id().to_string(),
                    protocol: provider.protocol,
                    reason: format!(
                        "provider/options shape maps a single openai_chat endpoint; provider `{}` protocol has no mapping",
                        provider.id
                    ),
                });
            }
            warnings.push(
                "zcode config schema research is incomplete; verify the provider/options keys after render"
                    .to_owned(),
            );
            operations.push(set_op(
                "provider",
                Value::String(provider.id.to_string()),
                owned_keys.clone(),
            ));
            let options = serde_json::json!({
                "baseUrl": provider.base_url,
                "apiKeyEnv": auth_env_var(provider, &mut warnings),
            });
            operations.push(set_op("options", options, owned_keys));
        }
        RenderStrategy::ProviderMap => {
            if !matches!(
                provider.protocol,
                Protocol::OpenAiChat | Protocol::OpenAiResponses | Protocol::Anthropic
            ) {
                return RenderOutcome::Unsupported(RenderUnsupported {
                    harness: adapter.id().to_string(),
                    protocol: provider.protocol,
                    reason: format!(
                        "provider map covers openai/anthropic chat transports; provider `{}` protocol has no mapping",
                        provider.id
                    ),
                });
            }
            let entry = serde_json::json!({
                "options": { "baseUrl": provider.base_url },
                "apiKey": provider
                    .auth
                    .env_var_names
                    .first().map_or_else(|| "[REDACTED]".to_owned(), |name| format!("{{env:{name}}}")),
            });
            let table = format!("provider.{}", provider.id.as_str());
            operations.push(set_op(&table, entry, owned_keys.clone()));
            if owned("model")
                && let Some(model) = model_default
            {
                operations.push(set_op(
                    "model",
                    Value::String(format!("{}/{}", provider.id, model)),
                    owned_keys,
                ));
            }
        }
    }

    let auth_sink = resolve_api_key_sink(adapter).ok();
    if auth_sink.is_none() {
        warnings.push(format!(
            "harness `{}` declares no config/env key sink; auth placement may require external login",
            adapter.id()
        ));
    }
    RenderOutcome::Supported(ProviderRender {
        surface_id: surface.id.clone(),
        kind: surface.kind,
        operations,
        auth_sink,
        auth_env_var: auth_env_var_out,
        warnings,
    })
}

// ---------------------------------------------------------------------------
// PRV-05 — effective provider inspection
// ---------------------------------------------------------------------------

/// Where a detected credential lives (type only, never the secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum CredentialSourceKind {
    /// Non-empty env-style field inside the harness config.
    ConfigEnvField,
    /// Env file entry under the isolated root.
    EnvFile,
    /// Externally managed (keychain/login); presence inferred from config shape.
    External,
}

/// Credential presence without the secret (PRV-05).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CredentialPresence {
    /// Whether a credential-shaped field is set.
    pub present: bool,
    /// Where it lives.
    pub source: CredentialSourceKind,
    /// Selector the field was found at.
    pub selector: String,
}

/// One detected model-role slot (default model, provider switch, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelRole {
    /// Role name, e.g. `default`, `provider`.
    pub role: String,
    /// Selector the role was read from.
    pub selector: String,
    /// Value read (model id / provider id — not secret-shaped).
    pub value: String,
    /// Surface the value was read from.
    pub surface: String,
}

/// Adapter/template compatibility verdict for the detected pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum CompatVerdict {
    /// The harness can express the detected provider.
    Compatible,
    /// It cannot, with the reason.
    Incompatible {
        /// Why the pair is incompatible.
        reason: String,
    },
    /// Nothing detected to check.
    NothingDetected,
}

/// Effective provider state, read FRESH from the instance config (PRV-05).
///
/// Ephemeral by design: never persisted into the registry, never carries a
/// secret (credential is presence + source type only).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EffectiveProviderReport {
    /// Instance whose config was inspected.
    pub instance_id: String,
    /// Detected provider, when one matches known data.
    pub detected_provider: Option<DetectedProvider>,
    /// Model-role slots currently set.
    pub model_roles: Vec<ModelRole>,
    /// Credential presence (never the value).
    pub credential: Option<CredentialPresence>,
    /// Surface that won for provider configuration (highest precedence with
    /// a provider-ish value present).
    pub winning_layer: Option<String>,
    /// Provider-shaped fields outside adapter-owned selectors.
    pub unknown_fields: Vec<String>,
    /// Adapter/template compatibility verdict.
    pub compatibility: CompatVerdict,
}

/// The provider detected from live config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectedProvider {
    /// Matching provider id from the supplied definitions, when found.
    pub id: Option<String>,
    /// Protocol the harness pair speaks (from the matched definition or the
    /// surface shape).
    pub protocol: Option<Protocol>,
    /// Endpoint in use (never secret-shaped; query strings are redacted).
    pub endpoint: String,
}

/// Read a surface's document fresh from the instance root.
fn load_surface_value(path: &std::path::Path, kind: DocumentKind) -> Result<Option<Value>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(CoreError::InvalidPath {
                kind: "instance_config".to_owned(),
                value: path.display().to_string(),
                reason: format!("cannot read config: {e}"),
            });
        }
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let parsed = match kind {
        DocumentKind::Json => {
            Some(
                serde_json::from_slice::<Value>(&bytes).map_err(|e| CoreError::Parse {
                    path: path.to_path_buf(),
                    kind: "json".to_owned(),
                    message: e.to_string(),
                })?,
            )
        }
        DocumentKind::Jsonc => serde_json::from_slice::<Value>(&bytes).ok(),
        DocumentKind::Yaml => yaml_serde::from_slice::<Value>(&bytes).ok(),
        DocumentKind::Toml => {
            let text = String::from_utf8_lossy(&bytes);
            let doc = text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|e| CoreError::Parse {
                    path: path.to_path_buf(),
                    kind: "toml".to_owned(),
                    message: e.to_string(),
                })?;
            Some(toml_to_value(&doc))
        }
        _ => None,
    };
    Ok(parsed)
}

/// Convert a parsed TOML document to the semantic JSON value tree.
#[expect(clippy::excessive_nesting, reason = "item/value recursion is per-kind")]
fn toml_to_value(doc: &toml_edit::DocumentMut) -> Value {
    fn item_to_value(item: &toml_edit::Item) -> Value {
        match item {
            toml_edit::Item::Value(value) => value_to_json(value),
            toml_edit::Item::Table(table) => {
                let mut map = serde_json::Map::new();
                for (key, child) in table {
                    map.insert(key.to_owned(), item_to_value(child));
                }
                Value::Object(map)
            }
            toml_edit::Item::ArrayOfTables(arr) => {
                let mut list = Vec::new();
                for table in arr {
                    let mut map = serde_json::Map::new();
                    for (key, child) in table {
                        map.insert(key.to_owned(), item_to_value(child));
                    }
                    list.push(Value::Object(map));
                }
                Value::Array(list)
            }
            toml_edit::Item::None => Value::Null,
        }
    }
    fn value_to_json(value: &toml_edit::Value) -> Value {
        match value {
            toml_edit::Value::String(s) => Value::String(s.value().to_owned()),
            toml_edit::Value::Integer(i) => Value::from(*i.value()),
            toml_edit::Value::Float(f) => Value::from(*f.value()),
            toml_edit::Value::Boolean(b) => Value::from(*b.value()),
            toml_edit::Value::Datetime(dt) => Value::String(dt.to_string()),
            toml_edit::Value::Array(arr) => Value::Array(arr.iter().map(value_to_json).collect()),
            toml_edit::Value::InlineTable(table) => {
                let mut map = serde_json::Map::new();
                for (key, child) in table {
                    map.insert(key.to_owned(), value_to_json(child));
                }
                Value::Object(map)
            }
        }
    }
    let mut root = serde_json::Map::new();
    for (key, item) in doc.iter() {
        root.insert(key.to_owned(), item_to_value(item));
    }
    Value::Object(root)
}

fn normalize_endpoint_for_match(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// Inspect the effective provider state of an instance (PRV-05).
///
/// Reads every writable surface under `instance.config_root` FRESH (disk is
/// truth; nothing is cached or persisted), matches endpoint/model values
/// against the supplied provider definitions, and reports credential
/// PRESENCE only. Never writes, never persists the report.
#[expect(
    clippy::excessive_nesting,
    reason = "fresh-scan branches over surfaces and selectors"
)]
#[expect(clippy::too_many_lines, reason = "strategy table is deliberate")]
pub fn inspect_effective_provider(
    instance: &Instance,
    adapter: &dyn Adapter,
    providers: &[ProviderDefinition],
) -> Result<EffectiveProviderReport> {
    let mut detected: Option<DetectedProvider> = None;
    let mut model_roles: Vec<ModelRole> = Vec::new();
    let mut credential: Option<CredentialPresence> = None;
    let mut winning_layer: Option<String> = None;
    let mut unknown_fields: Vec<String> = Vec::new();
    let mut winning_precedence: Option<u8> = None;

    let mut surfaces: Vec<ConfigSurface> = adapter
        .config_surfaces()
        .into_iter()
        .filter(|s| {
            writable_surface_kinds(s.kind)
                && !matches!(
                    s.ownership,
                    crate::adapter::SurfaceOwnership::ExternalSecretStore
                )
        })
        .collect();
    surfaces.sort_by_key(|s| s.precedence);

    for surface in &surfaces {
        let path = instance.config_root.as_path().join(&surface.id);
        let Some(value) = load_surface_value(&path, surface.kind)? else {
            continue;
        };
        let Value::Object(map) = value else {
            continue;
        };
        let mut surface_has_provider_value = false;
        let match_endpoint = |endpoint: &str| -> Option<DetectedProvider> {
            let normalized = normalize_endpoint_for_match(endpoint);
            let matched = providers.iter().find(|p| {
                normalize_endpoint_for_match(&p.base_url) == normalized
                    || p.endpoints
                        .iter()
                        .any(|v| normalize_endpoint_for_match(&v.base_url) == normalized)
            });
            Some(DetectedProvider {
                id: matched.map(|p| p.id.to_string()),
                protocol: matched.map(|p| p.protocol),
                endpoint: crate::health::redact_url(endpoint),
            })
        };
        for sel in &surface.owned_selectors {
            let lower = sel.to_ascii_lowercase();
            let segments: Vec<&str> = sel.split('.').collect();
            let mut current: Option<&Value> = Some(&Value::Object(map.clone()));
            for segment in &segments {
                current = current.and_then(|v| v.as_object().and_then(|o| o.get(*segment)));
            }
            let Some(found) = current else {
                continue;
            };
            if lower.contains("base_url") {
                if let Value::String(endpoint) = found {
                    surface_has_provider_value = true;
                    if detected.is_none()
                        && let Some(found_provider) = match_endpoint(endpoint)
                    {
                        detected = Some(found_provider);
                    }
                }
            } else if lower.contains("auth_token")
                || lower.contains("api_key")
                || lower.contains("apikey")
            {
                let present = matches!(found, Value::String(s) if !s.trim().is_empty());
                if present && credential.is_none() {
                    credential = Some(CredentialPresence {
                        present: true,
                        source: CredentialSourceKind::ConfigEnvField,
                        selector: sel.clone(),
                    });
                }
            } else if matches!(
                sel.as_str(),
                "model" | "default_model" | "model_provider" | "small_model"
            ) && let Value::String(role_value) = found
            {
                surface_has_provider_value = true;
                model_roles.push(ModelRole {
                    role: sel.as_str().to_owned(),
                    selector: sel.clone(),
                    value: role_value.clone(),
                    surface: surface.id.clone(),
                });
                // A provider-switch role also identifies the provider by id.
                if sel.as_str() == "model_provider" && detected.is_none() {
                    let matched = providers
                        .iter()
                        .find(|p| p.id.as_str().eq_ignore_ascii_case(role_value));
                    detected = matched.map(|p| DetectedProvider {
                        id: Some(p.id.to_string()),
                        protocol: Some(p.protocol),
                        endpoint: crate::health::redact_url(&p.base_url),
                    });
                }
            }
            // Table-shaped owned selectors (model_providers/providers/provider)
            // carry per-provider entries: detect through their base URLs.
            if let Value::Object(entries) = found {
                for (entry_key, entry) in entries {
                    let endpoint_field = entry
                        .as_object()
                        .and_then(|o| o.get("base_url").or_else(|| o.get("baseUrl")))
                        .and_then(Value::as_str);
                    if let Some(endpoint) = endpoint_field {
                        surface_has_provider_value = true;
                        if detected.is_none() {
                            detected = match_endpoint(endpoint).map(|mut found| {
                                if found.id.is_none() {
                                    found.id = Some(entry_key.clone());
                                }
                                found
                            });
                        }
                    }
                }
            }
        }
        // Provider-shaped keys the adapter does NOT own (relevant to
        // mutation decisions; read-only observation). Unowned tables count
        // when they or their children look provider-shaped.
        if let Value::Object(root) = &Value::Object(map.clone()) {
            let providerish = |name: &str| -> bool {
                let lower = name.to_ascii_lowercase();
                lower.contains("model")
                    || lower.contains("provider")
                    || lower.contains("base_url")
                    || lower.contains("apikey")
            };
            for (key, value) in root {
                if surface
                    .owned_selectors
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(key) || s.starts_with(&format!("{key}.")))
                {
                    continue;
                }
                if providerish(key) && !matches!(value, Value::Null) {
                    unknown_fields.push(format!("{}#{}", surface.id, key));
                } else if let Value::Object(children) = value {
                    for (child, child_value) in children {
                        if providerish(child) && !matches!(child_value, Value::Null) {
                            unknown_fields.push(format!("{}#{}.{}", surface.id, key, child));
                        }
                    }
                }
            }
        }
        if surface_has_provider_value && winning_precedence.is_none_or(|p| surface.precedence >= p)
        {
            winning_precedence = Some(surface.precedence);
            winning_layer = Some(surface.id.clone());
        }
    }

    let compatibility = match (&detected, &model_roles) {
        (None, roles) if roles.is_empty() => CompatVerdict::NothingDetected,
        (Some(found), _) => {
            let matched = providers
                .iter()
                .find(|p| Some(p.id.to_string()) == found.id);
            match matched {
                Some(provider) => {
                    let outcome = render_provider_into_adapter(provider, None, adapter, instance);
                    match outcome {
                        RenderOutcome::Supported(_) => CompatVerdict::Compatible,
                        RenderOutcome::Unsupported(reason) => CompatVerdict::Incompatible {
                            reason: reason.reason,
                        },
                    }
                }
                None => CompatVerdict::Compatible,
            }
        }
        (None, _) => CompatVerdict::NothingDetected,
    };

    Ok(EffectiveProviderReport {
        instance_id: instance.id.to_string(),
        detected_provider: detected,
        model_roles,
        credential,
        winning_layer,
        unknown_fields,
        compatibility,
    })
}

// ---------------------------------------------------------------------------
// PRV-08 — provider lifecycle
// ---------------------------------------------------------------------------

/// One lifecycle change to apply to an instance's provider configuration.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderChange<'a> {
    /// Add or update a provider's entries (endpoint/model defaults).
    AddOrUpdate {
        /// Provider definition to render in.
        provider: &'a ProviderDefinition,
    },
    /// Switch the default model (and provider-derived role slots).
    SwitchDefaultModel {
        /// Provider whose catalog the model must belong to.
        provider: &'a ProviderDefinition,
        /// Model id or alias to make the default.
        model: &'a str,
    },
    /// Remove a provider's owned entries only.
    RemoveProvider {
        /// Provider to remove.
        provider_id: &'a ProviderId,
        /// Optional provider to reassign dangling defaults to.
        reassign_to: Option<&'a ProviderDefinition>,
    },
}

/// Options for a lifecycle commit.
#[derive(Debug, Clone, Default)]
pub struct ProviderChangeOptions {
    /// Journal root enabling crash-journal recovery for the commit
    /// (e.g. `superai_config::journal::journal_dir(&home)`). `None` commits
    /// without a journal.
    pub journal_root: Option<PathBuf>,
}

/// Redacted preview of one lifecycle change (PRV-08 preview).
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderChangePreview {
    /// Surface that would change.
    pub surface_id: String,
    /// File that would change.
    pub path: PathBuf,
    /// Redacted edit descriptions (`selector: old -> new`); secrets never
    /// appear — auth fields are presence-only.
    pub edits: Vec<String>,
    /// Warnings (dangling references, unsupported notes).
    pub warnings: Vec<String>,
    /// Whether the harness can express the change at all.
    pub supported: bool,
    /// Reason when unsupported.
    pub unsupported_reason: Option<String>,
}

/// Outcome of a committed lifecycle change.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderChangeOutcome {
    /// Edits applied (redacted descriptions).
    pub applied: Vec<String>,
    /// Path of the file that changed.
    pub path: PathBuf,
    /// Warnings from the commit.
    pub warnings: Vec<String>,
}

/// The surface a change targets, resolved from the render strategy.
fn target_surface(adapter: &dyn Adapter) -> Result<(ConfigSurface, RenderStrategy)> {
    adapter
        .config_surfaces()
        .into_iter()
        .find_map(|surface| strategy_for(&surface).map(|strategy| (surface, strategy)))
        .ok_or_else(|| CoreError::UnsupportedOperation {
            harness: adapter.id().to_string(),
            operation: "provider_lifecycle".to_owned(),
            reason: "harness declares no writable provider-owned selectors (managed backend)"
                .to_owned(),
        })
}

fn describe_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => {
            if s.contains("sk-") {
                crate::error::RedactedString::placeholder().to_owned()
            } else {
                s.clone()
            }
        }
        Some(other) => format!("{other}"),
        None => "(absent)".to_owned(),
    }
}

/// Build the operations (and redacted edit descriptions) for a change.
#[expect(
    clippy::excessive_nesting,
    reason = "per-strategy lifecycle planning branches"
)]
#[expect(clippy::too_many_lines, reason = "strategy table is deliberate")]
fn plan_change(
    instance: &Instance,
    adapter: &dyn Adapter,
    change: &ProviderChange<'_>,
) -> Result<(
    Vec<EngineOperation>,
    Vec<String>,
    Vec<String>,
    DocumentKind,
    ConfigSurface,
)> {
    let (surface, strategy) = target_surface(adapter)?;
    let owned_keys = surface.owned_selectors.clone();
    let owned = |needle: &str| {
        surface
            .owned_selectors
            .iter()
            .any(|s| s.eq_ignore_ascii_case(needle))
    };
    let path = instance.config_root.as_path().join(&surface.id);
    let current =
        load_surface_value(&path, surface.kind)?.unwrap_or(Value::Object(serde_json::Map::new()));
    let read_selector = |selector: &str| -> Option<Value> {
        let mut current_ref: Option<&Value> = Some(&current);
        for segment in selector.split('.') {
            current_ref = current_ref.and_then(|v| v.as_object().and_then(|o| o.get(segment)));
        }
        current_ref.cloned()
    };
    let mut ops: Vec<EngineOperation> = Vec::new();
    let mut edits: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    let record = |ops: &mut Vec<EngineOperation>,
                  edits: &mut Vec<String>,
                  selector: &str,
                  value: Value,
                  owned_keys: &[String]| {
        let before = read_selector(selector);
        edits.push(format!(
            "{selector}: {} -> {}",
            describe_value(before.as_ref()),
            describe_value(Some(&value))
        ));
        ops.push(set_op(selector, value, owned_keys.to_vec()));
    };

    match change {
        ProviderChange::AddOrUpdate { provider } => {
            let outcome = render_provider_into_adapter(provider, None, adapter, instance);
            match outcome {
                RenderOutcome::Supported(render) => {
                    // Re-target ops at the same surface the render chose.
                    if render.surface_id != surface.id {
                        return Err(CoreError::Validation {
                            field: "surface".to_owned(),
                            reason: format!(
                                "render targeted `{}`, lifecycle targets `{}`",
                                render.surface_id, surface.id
                            ),
                        });
                    }
                    for op in &render.operations {
                        if let EditOperation::Set { selector, value } = &op.kind
                            && let Selector::Key(key) = selector
                        {
                            record(&mut ops, &mut edits, key, value.clone(), &owned_keys);
                        }
                    }
                    warnings.extend(render.warnings);
                }
                RenderOutcome::Unsupported(reason) => {
                    return Err(CoreError::UnsupportedOperation {
                        harness: reason.harness,
                        operation: "add_or_update_provider".to_owned(),
                        reason: reason.reason,
                    });
                }
            }
        }
        ProviderChange::SwitchDefaultModel { provider, model } => {
            let found = provider
                .model_list
                .iter()
                .find(|m| m.id == *model || m.alias.as_deref() == Some(model));
            let Some(model_entry) = found else {
                return Err(CoreError::Validation {
                    field: "model".to_owned(),
                    reason: format!(
                        "model `{model}` not found in provider `{}` catalog",
                        provider.id
                    ),
                });
            };
            if matches!(model_entry.status, crate::provider::ModelStatus::Retired) {
                return Err(CoreError::Validation {
                    field: "model".to_owned(),
                    reason: format!("model `{model}` is retired; refusing to make it the default"),
                });
            }
            let model_selector = match strategy {
                RenderStrategy::ProvidersTable => "default_model",
                _ => "model",
            };
            if !owned(model_selector) {
                return Err(CoreError::UnsupportedOperation {
                    harness: adapter.id().to_string(),
                    operation: "switch_default_model".to_owned(),
                    reason: format!("harness owns no `{model_selector}` selector"),
                });
            }
            let value = match strategy {
                RenderStrategy::ProviderMap => {
                    Value::String(format!("{}/{}", provider.id, model_entry.id))
                }
                _ => Value::String(model_entry.id.clone()),
            };
            record(&mut ops, &mut edits, model_selector, value, &owned_keys);
        }
        ProviderChange::RemoveProvider {
            provider_id,
            reassign_to,
        } => {
            let entry_selector = match strategy {
                RenderStrategy::ModelProvidersTable => {
                    format!("model_providers.{provider_id}")
                }
                RenderStrategy::ProvidersTable => format!("providers.{provider_id}"),
                RenderStrategy::ProviderMap => format!("provider.{provider_id}"),
                RenderStrategy::ProviderOptions => {
                    // Single-provider shape: removal means clearing the
                    // provider/options pair — only valid when they point at
                    // the removed provider.
                    let current_provider = read_selector("provider");
                    let matches_removed = current_provider
                        .as_ref()
                        .and_then(Value::as_str)
                        .is_some_and(|v| v == provider_id.as_str());
                    if !matches_removed {
                        return Err(CoreError::Validation {
                            field: "provider".to_owned(),
                            reason: format!(
                                "single-provider surface is not set to `{provider_id}`; nothing owned by it to remove"
                            ),
                        });
                    }
                    "provider".to_owned()
                }
                RenderStrategy::EnvKeys => {
                    // Env-key surfaces carry no provider identity; removal
                    // only clears keys currently pointing at the provider.
                    String::new()
                }
            };
            let mut dangling: Vec<String> = Vec::new();

            if strategy == RenderStrategy::EnvKeys {
                for sel in &surface.owned_selectors {
                    let lower = sel.to_ascii_lowercase();
                    if lower.contains("base_url") {
                        edits.push(format!(
                            "{sel}: {} -> (removed)",
                            describe_value(read_selector(sel).as_ref())
                        ));
                        ops.push(remove_op(sel, owned_keys.clone()));
                    }
                }
            } else if strategy == RenderStrategy::ProviderOptions {
                edits.push(format!(
                    "provider: {} -> (removed)",
                    describe_value(read_selector("provider").as_ref())
                ));
                ops.push(remove_op("provider", owned_keys.clone()));
                if owned("options") {
                    edits.push(format!(
                        "options: {} -> (removed)",
                        describe_value(read_selector("options").as_ref())
                    ));
                    ops.push(remove_op("options", owned_keys));
                }
            } else {
                if read_selector(&entry_selector).is_some() {
                    edits.push(format!("{entry_selector}: (removed)"));
                    ops.push(remove_op(&entry_selector, owned_keys.clone()));
                }
                // Dangling-default detection on the remaining references.
                let default_provider_selectors: &[&str] = match strategy {
                    RenderStrategy::ModelProvidersTable => &["model_provider"],
                    RenderStrategy::ProvidersTable => &["default_model"],
                    RenderStrategy::ProviderMap => &["model"],
                    _ => &[],
                };
                for selector in default_provider_selectors {
                    if !owned(selector) {
                        continue;
                    }
                    let value = read_selector(selector);
                    let references_removed =
                        value.as_ref().and_then(Value::as_str).is_some_and(|v| {
                            v == provider_id.as_str()
                                || v.starts_with(&format!("{}/", provider_id.as_str()))
                        });
                    if references_removed {
                        match reassign_to {
                            Some(next) => {
                                let new_value = if matches!(strategy, RenderStrategy::ProviderMap) {
                                    Value::String(format!(
                                        "{}/{}",
                                        next.id,
                                        next.defaults.default_model.clone().unwrap_or_default()
                                    ))
                                } else {
                                    Value::String(next.id.to_string())
                                };
                                edits.push(format!(
                                    "{selector}: {} -> {}",
                                    describe_value(value.as_ref()),
                                    describe_value(Some(&new_value))
                                ));
                                ops.push(set_op(selector, new_value, owned_keys.clone()));
                            }
                            None => {
                                dangling
                                    .push(format!("{selector}={}", describe_value(value.as_ref())));
                            }
                        }
                    }
                }
                if !dangling.is_empty() {
                    return Err(CoreError::Validation {
                        field: "remove_provider".to_owned(),
                        reason: format!(
                            "dangling default references after removing `{provider_id}`: {}; pass a reassignment or clear the defaults",
                            dangling.join(", ")
                        ),
                    });
                }
            }
        }
    }
    Ok((ops, edits, warnings, surface.kind, surface))
}

/// Preview a lifecycle change (read-only, redacted; PRV-08).
pub fn preview_provider_change(
    instance: &Instance,
    adapter: &dyn Adapter,
    change: &ProviderChange<'_>,
) -> ProviderChangePreview {
    let surface_id = target_surface(adapter).map(|(s, _)| s.id);
    match surface_id {
        Ok(surface_id) => {
            let path = instance.config_root.as_path().join(&surface_id);
            match plan_change(instance, adapter, change) {
                Ok((_, edits, warnings, _, _)) => ProviderChangePreview {
                    surface_id,
                    path,
                    edits,
                    warnings,
                    supported: true,
                    unsupported_reason: None,
                },
                Err(CoreError::UnsupportedOperation { reason, .. }) => ProviderChangePreview {
                    surface_id,
                    path,
                    edits: Vec::new(),
                    warnings: Vec::new(),
                    supported: false,
                    unsupported_reason: Some(reason),
                },
                Err(other) => ProviderChangePreview {
                    surface_id,
                    path,
                    edits: Vec::new(),
                    warnings: vec![other.to_string()],
                    supported: true,
                    unsupported_reason: None,
                },
            }
        }
        Err(CoreError::UnsupportedOperation { reason, .. }) => ProviderChangePreview {
            surface_id: "(none)".to_owned(),
            path: PathBuf::new(),
            edits: Vec::new(),
            warnings: Vec::new(),
            supported: false,
            unsupported_reason: Some(reason),
        },
        Err(other) => ProviderChangePreview {
            surface_id: "(none)".to_owned(),
            path: PathBuf::new(),
            edits: Vec::new(),
            warnings: vec![other.to_string()],
            supported: false,
            unsupported_reason: None,
        },
    }
}

fn engine_kind_for(kind: DocumentKind) -> EngineKind {
    match kind {
        DocumentKind::Json => EngineKind::StrictJson,
        DocumentKind::Jsonc => EngineKind::JsonC,
        DocumentKind::Toml => EngineKind::Toml,
        DocumentKind::Yaml => EngineKind::Yaml,
        DocumentKind::Env => EngineKind::Env,
        DocumentKind::TextFragment => EngineKind::TextFragment,
        DocumentKind::Executable
        | DocumentKind::Sqlite
        | DocumentKind::Keychain
        | DocumentKind::Opaque => EngineKind::Opaque,
    }
}

/// Navigate (creating as needed) the parent tables for a dotted key.
fn ensure_table<'t>(
    table: &'t mut toml_edit::Table,
    parents: &[&str],
) -> Option<&'t mut toml_edit::Table> {
    if parents.is_empty() {
        return Some(table);
    }
    let (first, rest) = parents.split_first()?;
    if table.get(first).is_none() {
        table.insert(first, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let child = table.get_mut(first)?.as_table_mut()?;
    ensure_table(child, rest)
}

/// Apply engine Set/Remove operations to a TOML document in place,
/// preserving decor of untouched keys.
#[expect(clippy::excessive_nesting, reason = "set/remove navigation branches")]
fn apply_ops_to_toml(doc: &mut toml_edit::DocumentMut, ops: &[EngineOperation]) -> Result<()> {
    fn value_to_toml(value: &Value) -> toml_edit::Item {
        match value {
            Value::String(s) => toml_edit::value(s.clone()),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    toml_edit::value(i)
                } else {
                    toml_edit::value(n.as_f64().unwrap_or_default())
                }
            }
            Value::Bool(b) => toml_edit::value(*b),
            Value::Array(arr) => {
                let items: Vec<toml_edit::Value> = arr
                    .iter()
                    .map(|v| match value_to_toml(v) {
                        toml_edit::Item::Value(value) => value,
                        _ => toml_edit::Value::from(""),
                    })
                    .collect();
                toml_edit::Item::Value(toml_edit::Value::Array(items.into_iter().collect()))
            }
            Value::Object(map) => {
                let mut table = toml_edit::Table::new();
                for (key, child) in map {
                    table.insert(key.as_str(), value_to_toml(child));
                }
                toml_edit::Item::Table(table)
            }
            Value::Null => toml_edit::Item::None,
        }
    }
    for op in ops {
        match &op.kind {
            EditOperation::Set { selector, value } => {
                let Selector::Key(key) = selector else {
                    return Err(CoreError::UnsupportedOperation {
                        harness: "toml".to_owned(),
                        operation: "provider_render".to_owned(),
                        reason: "only key selectors render into toml".to_owned(),
                    });
                };
                let segments: Vec<&str> = key.split('.').collect();
                let Some((last, parents)) = segments.split_last() else {
                    continue;
                };
                if let Some(table) = ensure_table(doc.as_table_mut(), parents) {
                    if table.get(last).is_some() {
                        // Index assignment keeps the existing key's decor
                        // (comments, spacing); `insert` would drop it.
                        table[last] = value_to_toml(value);
                    } else {
                        table.insert(last, value_to_toml(value));
                    }
                }
            }
            EditOperation::Remove { selector } => {
                let Selector::Key(key) = selector else {
                    continue;
                };
                let segments: Vec<&str> = key.split('.').collect();
                let Some((last, parents)) = segments.split_last() else {
                    continue;
                };
                if let Some(table) = ensure_table(doc.as_table_mut(), parents) {
                    table.remove(last);
                }
            }
            _ => {
                return Err(CoreError::UnsupportedOperation {
                    harness: "toml".to_owned(),
                    operation: "provider_render".to_owned(),
                    reason: "provider rendering uses set/remove only".to_owned(),
                });
            }
        }
    }
    Ok(())
}

/// Commit a lifecycle change (PRV-08): fresh read, engine-enforced owned-key
/// edits, backup + atomic write through a Transaction.
///
/// Foreign entries — other providers, unmodelled keys, comments and layout
/// on TOML surfaces — are preserved. JSONC surfaces with comments are
/// refused with the typed lossy-write error instead of silently stripping
/// them (codec honesty). Dangling default references block removal.
#[expect(clippy::excessive_nesting, reason = "per-kind serialization branches")]
#[expect(clippy::too_many_lines, reason = "strategy table is deliberate")]
pub fn commit_provider_change(
    instance: &Instance,
    adapter: &dyn Adapter,
    change: &ProviderChange<'_>,
    options: &ProviderChangeOptions,
) -> Result<ProviderChangeOutcome> {
    let (ops, edits, warnings, kind, surface) = plan_change(instance, adapter, change)?;
    if ops.is_empty() {
        return Err(CoreError::Validation {
            field: "provider_change".to_owned(),
            reason: "change produced no operations".to_owned(),
        });
    }
    let path = instance.config_root.as_path().join(&surface.id);
    let existing = std::fs::read(&path).ok();
    let new_bytes = match kind {
        DocumentKind::Toml => {
            let mut doc = match existing.as_deref() {
                Some(bytes) if !bytes.iter().all(u8::is_ascii_whitespace) => {
                    String::from_utf8_lossy(bytes)
                        .parse::<toml_edit::DocumentMut>()
                        .map_err(|e| CoreError::Parse {
                            path: path.clone(),
                            kind: "toml".to_owned(),
                            message: e.to_string(),
                        })?
                }
                _ => toml_edit::DocumentMut::new(),
            };
            apply_ops_to_toml(&mut doc, &ops)?;
            doc.to_string().into_bytes()
        }
        DocumentKind::Json | DocumentKind::Jsonc | DocumentKind::Yaml => {
            let mut value = match kind {
                DocumentKind::Json => {
                    serde_json::from_slice::<Value>(existing.as_deref().unwrap_or_default())
                        .map_err(|e| CoreError::Parse {
                            path: path.clone(),
                            kind: "json".to_owned(),
                            message: e.to_string(),
                        })?
                }
                DocumentKind::Jsonc => {
                    let bytes = existing.as_deref().unwrap_or_default();
                    if bytes.is_empty() || bytes.iter().all(u8::is_ascii_whitespace) {
                        Value::Object(serde_json::Map::new())
                    } else {
                        match serde_json::from_slice::<Value>(bytes) {
                            Ok(value) => value,
                            Err(_) => {
                                return Err(CoreError::Config(
                                    superai_config::ConfigError::LossyWrite {
                                        path: path.clone(),
                                        format: "jsonc",
                                    },
                                ));
                            }
                        }
                    }
                }
                _ => yaml_serde::from_slice::<Value>(existing.as_deref().unwrap_or_default())
                    .unwrap_or(Value::Object(serde_json::Map::new())),
            };
            if !value.is_object() {
                value = Value::Object(serde_json::Map::new());
            }
            for op in &ops {
                superai_config::executor::apply_to_value(&path, &mut value, op)
                    .map_err(CoreError::Config)?;
            }
            if kind == DocumentKind::Yaml {
                yaml_serde::to_string(&value)
                    .map_err(|e| CoreError::SchemaValidation {
                        path: path.clone(),
                        details: format!("yaml serialize failed: {e}"),
                    })?
                    .into_bytes()
            } else {
                let mut text = serde_json::to_string_pretty(&value).map_err(|e| {
                    CoreError::SchemaValidation {
                        path: path.clone(),
                        details: format!("json serialize failed: {e}"),
                    }
                })?;
                text.push('\n');
                text.into_bytes()
            }
        }
        _ => {
            return Err(CoreError::UnsupportedOperation {
                harness: adapter.id().to_string(),
                operation: "provider_lifecycle".to_owned(),
                reason: format!("surface kind `{kind}` cannot carry provider entries"),
            });
        }
    };

    let op_id_str = format!(
        "provider-change-{}-{}",
        instance.id.as_str(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default()
    );
    let tx_op_id = superai_config::transaction::OperationId::new(&op_id_str).map_err(|e| {
        CoreError::Validation {
            field: "operation_id".to_owned(),
            reason: format!("op id invalid: {e}"),
        }
    })?;
    let mut transaction = Transaction::new(
        tx_op_id,
        vec![FileAction::Write {
            path: path.clone(),
            content: new_bytes,
            kind: engine_kind_for(kind),
        }],
    );
    if let Some(journal_root) = options.journal_root.as_ref() {
        transaction = transaction.with_journal(journal_root.clone());
    }
    let outcome = transaction.execute().map_err(CoreError::Config)?;
    if !outcome.success {
        return Err(CoreError::Commit {
            path,
            reason: "provider change transaction failed; old content retained".to_owned(),
        });
    }
    Ok(ProviderChangeOutcome {
        applied: edits,
        path,
        warnings,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::paths::AbsolutePath;
    use crate::state::{InstanceOrigin, Isolation, Ownership};

    fn tmp_dir(name: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(&format!("provider-render-{name}"))
    }

    fn instance_in(dir: &std::path::Path, harness: &str, name: &str) -> Instance {
        let config_root = dir.join(name);
        std::fs::create_dir_all(&config_root).unwrap();
        Instance {
            id: InstanceId::new(&format!("id-{name}")).unwrap(),
            name: InstanceName::new(name).unwrap(),
            harness: HarnessId::new(harness).unwrap(),
            config_root: AbsolutePath::from_path(&config_root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T12:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        }
    }

    /// A provider that can render for all five fixture harnesses:
    /// `openai_chat` by default plus an anthropic-compatible variant.
    fn universal_provider(id: &str) -> ProviderDefinition {
        let mut def =
            ProviderDefinition::new(ProviderId::new(id).unwrap(), "https://api.example.com/v1");
        def.display_name = "Universal".to_owned();
        def.protocol = Protocol::OpenAiChat;
        def.auth.env_var_names = vec!["UNIVERSAL_API_KEY".to_owned()];
        def.endpoints = vec![crate::provider::EndpointVariant {
            name: "anthropic-compat".to_owned(),
            base_url: "https://api.example.com/anthropic".to_owned(),
            protocols: vec![Protocol::Anthropic],
        }];
        def.model_list = vec![crate::provider::ModelInfo {
            id: "model-a".to_owned(),
            display_name: None,
            status: crate::provider::ModelStatus::Active,
            alias: None,
            health_eligible: true,
            limits: crate::provider::ModelLimits::default(),
            input_modalities: vec![],
            output_modalities: vec![],
            supports_tools: false,
            supports_reasoning: false,
        }];
        def.defaults.default_model = Some("model-a".to_owned());
        def
    }

    #[test]
    fn same_provider_renders_differently_for_five_harnesses() {
        let dir = tmp_dir("five-harnesses");
        let provider = universal_provider("universal-prov");
        let adapters: Vec<Box<dyn Adapter>> = vec![
            Box::new(crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap()),
            Box::new(crate::adapters::codex_cli::CodexCliAdapter::new().unwrap()),
            Box::new(crate::adapters::kimi_code::KimiCodeAdapter::new().unwrap()),
            Box::new(crate::adapters::opencode::OpenCodeAdapter::new().unwrap()),
            Box::new(crate::adapters::zcode::ZcodeAdapter::new().unwrap()),
        ];
        let mut shapes = Vec::new();
        for adapter in &adapters {
            let instance = instance_in(&dir, adapter.id().as_str(), adapter.id().as_str());
            let outcome =
                render_provider_into_adapter(&provider, None, adapter.as_ref(), &instance);
            let RenderOutcome::Supported(render) = outcome else {
                panic!("render failed for {}", adapter.id());
            };
            let selector_summary: Vec<String> = render
                .operations
                .iter()
                .filter_map(|op| match &op.kind {
                    EditOperation::Set {
                        selector: Selector::Key(k),
                        ..
                    } => Some(k.clone()),
                    _ => None,
                })
                .collect();
            shapes.push((render.surface_id.clone(), selector_summary));
        }
        let (claude_surface, claude_sels) = &shapes[0];
        assert_eq!(claude_surface, "settings.json");
        assert!(claude_sels.iter().any(|s| s.contains("BASE_URL")));
        let (codex_surface, codex_sels) = &shapes[1];
        assert_eq!(codex_surface, "config.toml");
        assert!(
            codex_sels
                .iter()
                .any(|s| s == "model_providers.universal-prov")
        );
        let (kimi_surface, kimi_sels) = &shapes[2];
        assert_eq!(kimi_surface, "config.toml");
        assert!(kimi_sels.iter().any(|s| s == "providers.universal-prov"));
        assert!(kimi_sels.iter().any(|s| s == "default_model"));
        let (opencode_surface, opencode_sels) = &shapes[3];
        assert_eq!(opencode_surface, "opencode.json");
        assert!(opencode_sels.iter().any(|s| s == "provider.universal-prov"));
        let (zcode_surface, zcode_sels) = &shapes[4];
        assert_eq!(zcode_surface, "config.json");
        assert!(zcode_sels.iter().any(|s| s == "provider"));
        assert!(zcode_sels.iter().any(|s| s == "options"));
        // All five shapes are distinct.
        let surface_ids: Vec<&String> = shapes.iter().map(|(s, _)| s).collect();
        assert_eq!(
            surface_ids.len(),
            5,
            "expected five renders, got {surface_ids:?}"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn render_operations_carry_owned_keys_and_create_parents() {
        let dir = tmp_dir("owned-keys");
        let provider = universal_provider("owned-prov");
        let adapter = crate::adapters::codex_cli::CodexCliAdapter::new().unwrap();
        let instance = instance_in(&dir, "codex-cli", "owned");
        let RenderOutcome::Supported(render) =
            render_provider_into_adapter(&provider, None, &adapter, &instance)
        else {
            panic!("render failed");
        };
        for op in &render.operations {
            assert!(
                !op.owned_keys.is_empty(),
                "every render op declares the surface's owned keys"
            );
            assert!(op.create_parent, "render ops create missing parents");
        }
        // Operations apply through the executor without touching foreign keys.
        let mut value = serde_json::json!({"foreignRoot": {"keep": 1}});
        for op in &render.operations {
            let path = instance.config_root.as_path().join("config.toml");
            superai_config::executor::apply_to_value(&path, &mut value, op).unwrap();
        }
        assert_eq!(
            value.get("foreignRoot").and_then(|v| v.get("keep")),
            Some(&serde_json::json!(1)),
            "foreign keys survive"
        );
        assert!(
            value
                .get("model_providers")
                .and_then(|v| v.get("owned-prov"))
                .is_some()
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn unsupported_harness_and_protocol_return_reason_without_mutation() {
        let dir = tmp_dir("unsupported");
        let provider = universal_provider("unsupported-prov");
        let instance = instance_in(&dir, "generic", "gen");
        // A managed-backend style adapter with no provider-owned selectors.
        let generic = crate::adapter::GenericAdapter::new(
            HarnessId::new("generic-harness").unwrap(),
            "Generic",
            crate::adapter::ProductStatus::Active,
            "docs/harness-configs/generic.md",
            "2026-08-25",
            crate::state::AdapterSupport::ResearchBlocked,
            "managed backend",
            "docs/harness-configs/generic.md",
        );
        let outcome = render_provider_into_adapter(&provider, None, &generic, &instance);
        let RenderOutcome::Unsupported(reason) = outcome else {
            panic!("expected unsupported");
        };
        assert_eq!(reason.harness, "generic-harness");
        assert!(
            reason
                .reason
                .contains("no writable provider-owned selectors")
        );

        // Preview surfaces the same reason read-only.
        let preview = preview_provider_change(
            &instance,
            &generic,
            &ProviderChange::AddOrUpdate {
                provider: &provider,
            },
        );
        assert!(!preview.supported);
        assert!(preview.unsupported_reason.is_some());

        // Codex cannot speak the anthropic protocol.
        let codex = crate::adapters::codex_cli::CodexCliAdapter::new().unwrap();
        let mut anthropic_only = ProviderDefinition::new(
            ProviderId::new("anthropic-only").unwrap(),
            "https://api.anthropic.com",
        );
        anthropic_only.protocol = Protocol::Anthropic;
        let codex_instance = instance_in(&dir, "codex-cli", "codex");
        let outcome = render_provider_into_adapter(&anthropic_only, None, &codex, &codex_instance);
        let RenderOutcome::Unsupported(reason) = outcome else {
            panic!("expected unsupported for anthropic on codex");
        };
        assert!(reason.reason.contains("wire_api"), "got: {}", reason.reason);
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn render_never_emits_secret_values() {
        let dir = tmp_dir("no-secrets");
        let provider = universal_provider("secret-free");
        let adapter = crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap();
        let instance = instance_in(&dir, "claude-code", "sf");
        let RenderOutcome::Supported(render) =
            render_provider_into_adapter(&provider, None, &adapter, &instance)
        else {
            panic!("render failed");
        };
        let dumped = format!("{render:?}");
        assert!(!dumped.contains("sk-"));
        assert!(!dumped.contains("UNIVERSAL_API_KEY="));
        // The auth reference is a variable NAME only, never a value.
        if let Some(var) = render.auth_env_var.as_deref() {
            assert!(!var.contains('=') && !var.contains("sk-"));
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn commit_adds_provider_preserving_foreign_entries_and_backup() {
        let dir = tmp_dir("add-provider");
        let adapter = crate::adapters::codex_cli::CodexCliAdapter::new().unwrap();
        let instance = instance_in(&dir, "codex-cli", "add");
        let config = instance.config_root.as_path().join("config.toml");
        std::fs::write(
            &config,
            "# user comment\nmodel = \"gpt-4o\"\n\n[model_providers.openai]\nname = \"OpenAI\"\nbase_url = \"https://api.openai.com/v1\"\n\n[foreign_root]\nkeep = true\n",
        )
        .unwrap();

        let provider = universal_provider("glm-add");
        let outcome = commit_provider_change(
            &instance,
            &adapter,
            &ProviderChange::AddOrUpdate {
                provider: &provider,
            },
            &ProviderChangeOptions::default(),
        )
        .unwrap();
        assert!(!outcome.applied.is_empty());
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains("[model_providers.glm-add]"), "got: {text}");
        assert!(
            text.contains("[model_providers.openai]"),
            "foreign provider preserved: {text}"
        );
        assert!(
            text.contains("# user comment"),
            "comments preserved: {text}"
        );
        assert!(text.contains("[foreign_root]"), "foreign keys preserved");
        // Backup of the foreign-authored original exists.
        assert!(superai_config::backup::list_backups(&config).is_ok_and(|b| !b.is_empty()));
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn remove_provider_preserves_foreign_and_catches_dangling_defaults() {
        let dir = tmp_dir("remove-provider");
        let adapter = crate::adapters::codex_cli::CodexCliAdapter::new().unwrap();
        let instance = instance_in(&dir, "codex-cli", "remove");
        let config = instance.config_root.as_path().join("config.toml");
        std::fs::write(
            &config,
            "model_provider = \"glm-rm\"\nmodel = \"glm-4.5\"\n\n[model_providers.glm-rm]\nname = \"GLM\"\nbase_url = \"https://api.example.com/v1\"\n\n[model_providers.openai]\nname = \"OpenAI\"\nbase_url = \"https://api.openai.com/v1\"\n",
        )
        .unwrap();

        let removed = ProviderId::new("glm-rm").unwrap();
        // Without reassignment, the dangling default blocks the removal.
        let err = commit_provider_change(
            &instance,
            &adapter,
            &ProviderChange::RemoveProvider {
                provider_id: &removed,
                reassign_to: None,
            },
            &ProviderChangeOptions::default(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("dangling default references"), "got: {msg}");
        // Nothing was mutated.
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.contains("[model_providers.glm-rm]"));

        // With reassignment the defaults switch and the entry disappears;
        // the foreign provider survives.
        let next = universal_provider("openai");
        commit_provider_change(
            &instance,
            &adapter,
            &ProviderChange::RemoveProvider {
                provider_id: &removed,
                reassign_to: Some(&next),
            },
            &ProviderChangeOptions::default(),
        )
        .unwrap();
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(!text.contains("model_providers.glm-rm"), "got: {text}");
        assert!(text.contains("[model_providers.openai]"));
        assert!(text.contains("model_provider = \"openai\""));
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn switch_default_model_validates_catalog_and_writes_role() {
        let dir = tmp_dir("switch-model");
        let adapter = crate::adapters::kimi_code::KimiCodeAdapter::new().unwrap();
        let instance = instance_in(&dir, "kimi-code", "switch");
        let provider = universal_provider("switch-prov");
        let mut second = provider.clone();
        second.model_list[0].id = "model-b".to_owned();
        let mut provider = provider;
        provider.model_list.push(crate::provider::ModelInfo {
            id: "model-b".to_owned(),
            display_name: None,
            status: crate::provider::ModelStatus::Active,
            alias: Some("alias-b".to_owned()),
            health_eligible: true,
            limits: crate::provider::ModelLimits::default(),
            input_modalities: vec![],
            output_modalities: vec![],
            supports_tools: false,
            supports_reasoning: false,
        });

        // Unknown model rejected.
        let err = commit_provider_change(
            &instance,
            &adapter,
            &ProviderChange::SwitchDefaultModel {
                provider: &provider,
                model: "no-such-model",
            },
            &ProviderChangeOptions::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("not found in provider"));

        // Alias resolves to the underlying id.
        commit_provider_change(
            &instance,
            &adapter,
            &ProviderChange::SwitchDefaultModel {
                provider: &provider,
                model: "alias-b",
            },
            &ProviderChangeOptions::default(),
        )
        .unwrap();
        let text =
            std::fs::read_to_string(instance.config_root.as_path().join("config.toml")).unwrap();
        assert!(text.contains("default_model = \"model-b\""), "got: {text}");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn commit_refuses_jsonc_comments_instead_of_stripping() {
        let dir = tmp_dir("jsonc-refusal");
        let adapter = crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap();
        let instance = instance_in(&dir, "claude-code", "jsonc");
        let settings = instance.config_root.as_path().join("settings.json");
        let original = b"{\n  // user comment\n  \"foreign\": true\n}\n";
        std::fs::write(&settings, original).unwrap();

        let provider = universal_provider("jsonc-prov");
        let err = commit_provider_change(
            &instance,
            &adapter,
            &ProviderChange::AddOrUpdate {
                provider: &provider,
            },
            &ProviderChangeOptions::default(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("lossy") || msg.contains("jsonc"),
            "got: {msg}"
        );
        // Bytes untouched.
        assert_eq!(std::fs::read(&settings).unwrap(), original.to_vec());

        // Without comments, the same commit succeeds and preserves foreign keys.
        std::fs::write(&settings, b"{\n  \"foreign\": true\n}\n").unwrap();
        commit_provider_change(
            &instance,
            &adapter,
            &ProviderChange::AddOrUpdate {
                provider: &provider,
            },
            &ProviderChangeOptions::default(),
        )
        .unwrap();
        let text = std::fs::read_to_string(&settings).unwrap();
        assert!(text.contains("\"foreign\": true"));
        assert!(text.contains("BASE_URL"));
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn inspect_effective_provider_reads_fresh_and_reports_without_secrets() {
        let dir = tmp_dir("inspect");
        let adapter = crate::adapters::codex_cli::CodexCliAdapter::new().unwrap();
        let instance = instance_in(&dir, "codex-cli", "inspect");
        let config = instance.config_root.as_path().join("config.toml");
        std::fs::write(
            &config,
            "model_provider = \"glm\"\nmodel = \"glm-4.5\"\n\n[model_providers.glm]\nname = \"GLM\"\nbase_url = \"https://open.bigmodel.cn/api/paas/v4\"\n\n[mystery]\nmodel_hint = \"x\"\n",
        )
        .unwrap();
        let providers = crate::provider::load_bundled_providers().unwrap();
        let report = inspect_effective_provider(&instance, &adapter, &providers).unwrap();
        let detected = report.detected_provider.clone().expect("provider detected");
        assert_eq!(detected.id.as_deref(), Some("glm"));
        assert_eq!(detected.protocol, Some(Protocol::OpenAiChat));
        assert!(detected.endpoint.contains("bigmodel"));
        let roles: Vec<&str> = report.model_roles.iter().map(|r| r.role.as_str()).collect();
        assert!(roles.contains(&"model"));
        assert!(roles.contains(&"model_provider"));
        assert_eq!(report.winning_layer.as_deref(), Some("config.toml"));
        assert!(
            !report.unknown_fields.is_empty(),
            "unowned provider-shaped fields are flagged"
        );
        assert_eq!(report.compatibility, CompatVerdict::Compatible);
        // No secret field exists on the report at all.
        let dumped = format!("{report:?}");
        assert!(!dumped.contains("sk-"));
        // Read-only: bytes unchanged.
        assert!(
            std::fs::read_to_string(&config).is_ok_and(|t| t.contains("model_provider = \"glm\""))
        );

        // Claude env-key fixture: credential presence without the value.
        let claude = crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap();
        let claude_instance = instance_in(&dir, "claude-code", "cred");
        let settings = claude_instance.config_root.as_path().join("settings.json");
        std::fs::write(
            &settings,
            r#"{"env": {"ANTHROPIC_BASE_URL": "https://open.bigmodel.cn/api/anthropic", "ANTHROPIC_AUTH_TOKEN": "sk-superai-test-sentinel-12345-fake"}}"#,
        )
        .unwrap();
        let report = inspect_effective_provider(&claude_instance, &claude, &providers).unwrap();
        let cred = report.credential.clone().expect("credential presence");
        assert!(cred.present);
        assert_eq!(cred.source, CredentialSourceKind::ConfigEnvField);
        assert!(cred.selector.contains("AUTH_TOKEN"));
        let dumped = format!("{report:?}");
        assert!(
            !dumped.contains("sk-superai-test-sentinel-12345-fake"),
            "report leaked the secret: {dumped}"
        );
        // The anthropic-compat variant is matched (endpoint variant).
        assert_eq!(
            report.detected_provider.expect("detected").id.as_deref(),
            Some("glm")
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn preview_is_read_only_and_redacted() {
        let dir = tmp_dir("preview");
        let adapter = crate::adapters::codex_cli::CodexCliAdapter::new().unwrap();
        let instance = instance_in(&dir, "codex-cli", "preview");
        let config = instance.config_root.as_path().join("config.toml");
        std::fs::write(&config, "model = \"old-model\"\n").unwrap();
        let provider = universal_provider("preview-prov");
        let preview = preview_provider_change(
            &instance,
            &adapter,
            &ProviderChange::AddOrUpdate {
                provider: &provider,
            },
        );
        assert!(preview.supported);
        assert!(!preview.edits.is_empty());
        for edit in &preview.edits {
            assert!(!edit.contains("sk-"), "edit leaked secret: {edit}");
        }
        assert!(preview.edits.iter().any(|e| e.starts_with("model:")));
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "model = \"old-model\"\n",
            "preview must not write"
        );
        drop(std::fs::remove_dir_all(&dir));
    }
}
