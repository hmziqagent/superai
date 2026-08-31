//! Provider definitions — data-driven, no hardcoded provider list.
//!
//! A provider is versioned data, not a Rust branch. Adding a provider is a
//! data-only change: add a JSON/YAML file and no Rust source edit is required.
//! Definitions are read fresh from disk on every load; nothing is cached.
//! Health probing validates URL format, bounds timeout, redacts secrets,
//! classifies auth/rate-limit/TLS via the fake harness (no live network),
//! and strips auth on cross-host redirects. API keys are ephemeral and only
//! written to harness-declared sinks.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::adapter::{Adapter, DocumentKind, SurfaceOwnership};
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// `OpenAI` chat/completions.
    #[default]
    OpenAiChat,
    /// `OpenAI` responses.
    OpenAiResponses,
    /// Anthropic messages.
    Anthropic,
    /// Gemini.
    Gemini,
    /// Vendor-specific identifier preserved via `Other` (serde other).
    #[serde(other)]
    Other,
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
// ProviderDefinition
// ---------------------------------------------------------------------------

/// Provider definition as stored in a JSON/YAML data file.
///
/// No secret values are stored here. Adding a provider means adding a file,
/// not editing Rust. Fields not modelled survive via serde's ignore on write
/// but are not invented — unknown keys are ignored on read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl ProviderDefinition {
    #[expect(
        clippy::excessive_nesting,
        reason = "validation checks multiple levels"
    )]
    /// Validate the definition before use.
    ///
    /// Checks:
    /// - `base_url` non-empty and syntactically valid (no network)
    /// - unique model IDs and aliases
    /// - `default_model` exists and is active unless the provider is legacy/retired
    /// - positive model identifiers (non-empty)
    pub fn validate(&self) -> Result<()> {
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
            if !is_legacy && matches!(model.status, ModelStatus::Retired) {
                return Err(CoreError::Validation {
                    field: "defaults.default_model".to_owned(),
                    reason: format!(
                        "provider `{}` default_model `{default}` is retired but provider is not legacy",
                        self.id
                    ),
                });
            }
        }
        Ok(())
    }

    /// Normalized base URL for duplicate detection.
    ///
    /// Lowercases scheme/host and trims trailing slashes. Uses character-boundary safe truncation.
    pub fn normalized_base_url(&self) -> String {
        let mut url = self.base_url.trim().to_lowercase();
        while url.ends_with('/') && url.len() > 1 {
            // Safe: `ends_with('/')` guarantees last char is one byte '/'.
            url.pop();
        }
        url
    }
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

/// Validate a raw API key value before placement.
///
/// - Must be non-empty and not contain control chars or NUL.
/// - Optional prefix check only when the provider documents a prefix via
///   its auth style: `Bearer` expects not to look like an env var reference,
///   but no strict `sk-` enforcement to avoid false positives across vendors.
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
    // Allow ${ENV_VAR} references only for WrapperEnvRef sink path, not for literal writes.
    // We don't reject here; sink selection will enforce the right handling.
    Ok(())
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
    let existing: Option<serde_json::Value> = if dest.exists() {
        let bytes = std::fs::read(dest).map_err(|e| CoreError::InvalidPath {
            kind: "read".to_owned(),
            value: dest.display().to_string(),
            reason: format!("cannot read destination: {e}"),
        })?;
        if bytes.is_empty() {
            None
        } else {
            // Try parse as json; if fails, treat as error with validation kind.
            let v: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| CoreError::Parse {
                    path: dest.to_path_buf(),
                    kind: "json".to_owned(),
                    message: e.to_string(),
                })?;
            Some(v)
        }
    } else {
        None
    };
    let mut root = existing.unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
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
            root = serde_json::Value::Object(serde_json::Map::new());
        }
        let map = root.as_object_mut().expect("just set to object");
        let env_entry = map
            .entry(env_key.to_owned())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if !env_entry.is_object() {
            *env_entry = serde_json::Value::Object(serde_json::Map::new());
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
                *cur = serde_json::Value::Object(serde_json::Map::new());
            }
            let map = cur.as_object_mut().expect("object");
            let entry = map
                .entry((*part).to_owned())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            cur = entry;
        }
        (cur, leaf)
    } else {
        (&mut root, sel.to_owned())
    };
    if let Some(obj) = target_obj.as_object_mut() {
        obj.insert(leaf_key, serde_json::Value::String(secret.to_owned()));
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
    superai_config::atomic::atomic_write(dest, new_content.as_bytes())
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
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::TemplateRef;
    use crate::paths::AbsolutePath;
    use crate::state::{InstanceOrigin, Isolation, Ownership};
    use std::collections::BTreeMap;
    use std::time::Duration;

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
        let valid = ProviderDefinition {
            id: ProviderId::new("test-valid").unwrap(),
            display_name: "valid".to_owned(),
            base_url: "https://api.example.com".to_owned(),
            auth_style: AuthStyle::Bearer,
            protocol: Protocol::OpenAiChat,
            model_list: vec![],
            defaults: ProviderDefaults::default(),
            status: ProviderStatus::Active,
            documentation_url: None,
        };
        let ok = health_probe(&valid);
        assert!(ok.valid, "expected valid for https url: {}", ok.reason);

        let invalid = ProviderDefinition {
            id: ProviderId::new("test-invalid").unwrap(),
            display_name: "invalid".to_owned(),
            base_url: "file:///etc/passwd".to_owned(),
            auth_style: AuthStyle::Bearer,
            protocol: Protocol::OpenAiChat,
            model_list: vec![],
            defaults: ProviderDefaults::default(),
            status: ProviderStatus::Active,
            documentation_url: None,
        };
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
            id: ProviderId::new("local-prov").unwrap(),
            display_name: "local".to_owned(),
            base_url: "http://localhost:8080".to_owned(),
            auth_style: AuthStyle::None,
            protocol: Protocol::Other,
            model_list: vec![],
            defaults: ProviderDefaults::default(),
            status: ProviderStatus::Active,
            documentation_url: None,
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
        // Ensure no secret patterns in bundled file
        let raw = BUNDLED_PROVIDERS_JSON;
        assert!(
            !raw.contains("sk-"),
            "bundled json must not contain secrets"
        );
        let debug = format!("{bundled:?}");
        assert!(
            !debug.contains("sk-"),
            "bundled debug must not leak secrets"
        );
        let ser = serde_json::to_string(&bundled).unwrap();
        assert!(
            !ser.contains("sk-"),
            "bundled serialized must not contain secrets"
        );
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
        let prov = ProviderDefinition {
            id: ProviderId::new("test-health-bounded").unwrap(),
            display_name: "bounded".to_owned(),
            base_url: "https://api.example.com".to_owned(),
            auth_style: AuthStyle::Bearer,
            protocol: Protocol::OpenAiChat,
            model_list: vec![],
            defaults: ProviderDefaults::default(),
            status: ProviderStatus::Active,
            documentation_url: None,
        };
        // Valid config should be healthy
        let good = crate::health::HealthConfig::default();
        let res = crate::health::health_probe(&prov, &good);
        assert!(res.valid);
        assert_eq!(res.status, crate::failure::HealthStatus::Healthy);
        // Private host with bearer should fail when deny, succeed when allow
        let local = ProviderDefinition {
            id: ProviderId::new("local-bearer").unwrap(),
            display_name: "local".to_owned(),
            base_url: "http://localhost:8080".to_owned(),
            auth_style: AuthStyle::Bearer,
            protocol: Protocol::Other,
            model_list: vec![],
            defaults: ProviderDefaults::default(),
            status: ProviderStatus::Active,
            documentation_url: None,
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
            id: ProviderId::new("test-redact").unwrap(),
            display_name: "redact".to_owned(),
            base_url: "https://api.example.com?api_key=sk-superai-test-sentinel-12345-fake&foo=bar"
                .to_owned(),
            auth_style: AuthStyle::Bearer,
            protocol: Protocol::OpenAiChat,
            model_list: vec![],
            defaults: ProviderDefaults::default(),
            status: ProviderStatus::Active,
            documentation_url: None,
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
        let prov = ProviderDefinition {
            id: ProviderId::new("test-redirect").unwrap(),
            display_name: "redir".to_owned(),
            base_url: "https://api.example.com".to_owned(),
            auth_style: AuthStyle::Bearer,
            protocol: Protocol::OpenAiChat,
            model_list: vec![],
            defaults: ProviderDefaults::default(),
            status: ProviderStatus::Active,
            documentation_url: None,
        };
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
        let provider = ProviderDefinition {
            id: ProviderId::new("test-prov-key").unwrap(),
            display_name: "Test".to_owned(),
            base_url: "https://api.example.com".to_owned(),
            auth_style: AuthStyle::Bearer,
            protocol: Protocol::OpenAiChat,
            model_list: vec![],
            defaults: ProviderDefaults::default(),
            status: ProviderStatus::Active,
            documentation_url: None,
        };
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
        let prov = ProviderDefinition {
            id: ProviderId::new("test-key-val").unwrap(),
            display_name: "Test".to_owned(),
            base_url: "https://api.example.com".to_owned(),
            auth_style: AuthStyle::Bearer,
            protocol: Protocol::OpenAiChat,
            model_list: vec![],
            defaults: ProviderDefaults::default(),
            status: ProviderStatus::Active,
            documentation_url: None,
        };
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
            id: ProviderId::new("synthetic-redacted").unwrap(),
            display_name: "redacted".to_owned(),
            base_url: secret_url.to_owned(),
            auth_style: AuthStyle::Bearer,
            protocol: Protocol::OpenAiChat,
            model_list: vec![],
            defaults: ProviderDefaults::default(),
            status: ProviderStatus::Active,
            documentation_url: None,
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
}
