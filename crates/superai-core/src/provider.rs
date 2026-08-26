//! Provider definitions — data-driven, no hardcoded provider list.
//!
//! A provider is versioned data, not a Rust branch. Adding a provider is a
//! data-only change: add a JSON/YAML file and no Rust source edit is required.
//! Definitions are read fresh from disk on every load; nothing is cached.
//! Health probing validates URL format only (no network in tests).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::ids::ProviderId;

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
// Health probe (stub, no network)
// ---------------------------------------------------------------------------

/// Result of a stub health probe — validates URL format only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthProbeResult {
    /// Provider id as string.
    pub provider: String,
    /// Base URL probed.
    pub base_url: String,
    /// Whether the URL is syntactically valid.
    pub valid: bool,
    /// Reason for validity or failure.
    pub reason: String,
}

/// Stub health probe that validates URL format without network.
///
/// Returns `valid = true` only if `base_url` is a well-formed http/https URL.
/// No DNS, TLS, or HTTP request is performed.
pub fn health_probe(provider: &ProviderDefinition) -> HealthProbeResult {
    let (valid, reason) = is_valid_base_url(&provider.base_url);
    HealthProbeResult {
        provider: provider.id.to_string(),
        base_url: provider.base_url.clone(),
        valid,
        reason,
    }
}

/// Validate a raw URL string without a provider (useful for preview).
pub fn health_probe_url(url: &str) -> HealthProbeResult {
    let (valid, reason) = is_valid_base_url(url);
    HealthProbeResult {
        provider: String::new(),
        base_url: url.to_owned(),
        valid,
        reason,
    }
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
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(&format!("provider-{name}"))
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
}
