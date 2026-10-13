//! Usage provider registry (USG-07): data-only entries keyed by provider id,
//! seeded from subplan 14's overlap table and extendable through JSON data
//! files so adding a standard provider needs no Rust edit.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use superai_core::ids::ProviderId;

/// Current schema version of a usage-provider entry.
pub const USAGE_PROVIDER_SCHEMA_VERSION: u32 = 1;

/// Error-source label for the embedded bundled seed.
const BUNDLED_SOURCE: &str = "assets/usage-providers.json";

/// Raw JSON for the bundled usage providers, checked in at
/// `crates/superai-usage/assets/usage-providers.json` and embedded via
/// `include_str!`. Adding a standard provider is editing that data file,
/// not Rust code.
const BUNDLED_USAGE_PROVIDERS_JSON: &str = include_str!("../assets/usage-providers.json");

/// Everything registry construction, loading, and validation can fail with;
/// cost keeps its own `CostError` in [`crate::cost`].
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// Two entries claimed the same provider id.
    #[error("duplicate usage provider entry `{id}`")]
    Duplicate {
        /// Provider id that collided.
        id: String,
    },
    /// An entry field failed validation.
    #[error("invalid usage provider entry `{id}` {field}: {reason}")]
    Invalid {
        /// Entry that failed.
        id: String,
        /// Field that failed.
        field: &'static str,
        /// What was wrong.
        reason: String,
    },
    /// A data file could not be read.
    #[error("read {path}: {source}")]
    Read {
        /// File being read.
        path: String,
        /// Underlying io failure.
        source: std::io::Error,
    },
    /// A data source could not be parsed as entry data.
    #[error("parse {path}: {reason}")]
    Parse {
        /// File (or embedded source label) being parsed.
        path: String,
        /// What did not parse.
        reason: String,
    },
}

/// Where one provider's usage truth lives: a documented endpoint or a local
/// artifact path template (`~`- and env-relative). Locators name places,
/// never credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum UsageLocator {
    /// Usage endpoint; https, except plain http on a loopback host for
    /// local proxies such as llmproxy.
    Endpoint {
        /// Documented endpoint URL.
        url: String,
    },
    /// Local artifact location, e.g. `~/.codex/sessions`.
    ArtifactPath {
        /// Path template under the user's home.
        template: String,
    },
}

/// The kind of auth material a usage source needs: names and file locations
/// only, never secret values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AuthMaterialKind {
    /// Environment variable carrying the credential, e.g. `CLINE_API_KEY`.
    EnvVar {
        /// Variable name.
        name: String,
    },
    /// Env-style file under the user's home, e.g. `~/.factory/.env`.
    EnvFile {
        /// Path template.
        template: String,
    },
    /// Token-bearing file, e.g. `~/.codex/auth.json`.
    TokenFile {
        /// Path template.
        template: String,
    },
    /// Provider OAuth token whose storage superai does not own.
    #[serde(rename = "oauth_token")]
    OAuthToken,
    /// Browser cookies or localStorage; reading them is out of scope for
    /// this subplan, so sources needing them stay deferred.
    BrowserSession,
}

/// How a provider's quota windows are obtained; readers report the matching
/// [`crate::window::LaneReading`] state, never a fabricated fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowStrategy {
    /// Windows come from a documented usage endpoint plus auth material.
    Endpoint,
    /// Windows are computed from local artifacts under semantics the
    /// provider documents.
    LocalArtifacts,
    /// The lane exists but only the provider's account surface or llmproxy
    /// reads it; report [`crate::window::LaneReading::ProviderOnly`].
    ProviderOnly,
    /// No quota window exists; usage truth is cost or balance only.
    CostOnly,
}

/// Pricing context that applies to a provider's recorded usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PricingReference {
    /// Plan- or credit-denominated usage; recorded spend is not a bill.
    Subscription,
    /// Per-token estimates from the models.dev catalog (USG-05).
    ModelsDev,
    /// The source records no billable amounts.
    NotPriced,
    /// Pricing context not yet verified against documentation.
    Unverified,
}

/// One provider's usage-truth declaration. Data only, never secret values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageProviderEntry {
    /// Provider this entry describes.
    pub id: ProviderId,
    /// Human-readable provider label.
    #[serde(default)]
    pub label: String,
    /// Documented places the usage truth lives.
    #[serde(default)]
    pub locators: Vec<UsageLocator>,
    /// Auth material kinds reading may need; empty means none.
    #[serde(default)]
    pub auth_material: Vec<AuthMaterialKind>,
    /// How quota windows for this provider are obtained.
    pub window_strategy: WindowStrategy,
    /// Pricing context for this provider's recorded usage.
    pub pricing: PricingReference,
    /// Link to the documentation that verified this entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    /// Schema version of this entry.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Date the entry was last verified, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
}

fn default_schema_version() -> u32 {
    USAGE_PROVIDER_SCHEMA_VERSION
}

impl UsageProviderEntry {
    /// Validate the entry before use: schema version, date shape, endpoint
    /// safety, control-character discipline. No network.
    pub fn validate(&self) -> Result<(), RegistryError> {
        let id = self.id.as_str();
        let invalid = |field: &'static str, reason: String| RegistryError::Invalid {
            id: id.to_owned(),
            field,
            reason,
        };
        if self.schema_version != USAGE_PROVIDER_SCHEMA_VERSION {
            return Err(invalid(
                "schema_version",
                format!(
                    "must be {USAGE_PROVIDER_SCHEMA_VERSION}, got {}",
                    self.schema_version
                ),
            ));
        }
        if self.label.trim().is_empty() {
            return Err(invalid("label", "must not be empty".to_owned()));
        }
        if let Some(url) = self.documentation_url.as_deref()
            && (url.trim().is_empty() || url.chars().any(char::is_control))
        {
            return Err(invalid(
                "documentation_url",
                "must be a non-empty control-free link".to_owned(),
            ));
        }
        if let Some(date) = self.verified_at.as_deref()
            && !is_iso_date(date)
        {
            return Err(invalid(
                "verified_at",
                format!("must be YYYY-MM-DD, got `{date}`"),
            ));
        }
        for locator in &self.locators {
            let (field, reason) = match locator {
                UsageLocator::Endpoint { url } => ("locators.url", endpoint_url_reason(url)),
                UsageLocator::ArtifactPath { template } => {
                    ("locators.template", path_template_reason(template))
                }
            };
            if let Some(reason) = reason {
                return Err(invalid(field, reason));
            }
        }
        for material in &self.auth_material {
            let (field, reason) = match material {
                AuthMaterialKind::EnvVar { name } => ("auth_material.name", env_name_reason(name)),
                AuthMaterialKind::EnvFile { template }
                | AuthMaterialKind::TokenFile { template } => {
                    ("auth_material.template", path_template_reason(template))
                }
                AuthMaterialKind::OAuthToken | AuthMaterialKind::BrowserSession => continue,
            };
            if let Some(reason) = reason {
                return Err(invalid(field, reason));
            }
        }
        Ok(())
    }
}

/// Registry of usage provider entries, keyed by provider id.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageRegistry {
    entries: BTreeMap<ProviderId, UsageProviderEntry>,
}

impl UsageRegistry {
    /// Build from entries; duplicate provider ids are rejected.
    pub fn from_entries(
        entries: impl IntoIterator<Item = UsageProviderEntry>,
    ) -> Result<Self, RegistryError> {
        let mut map = BTreeMap::new();
        for entry in entries {
            if map.contains_key(&entry.id) {
                return Err(RegistryError::Duplicate {
                    id: entry.id.as_str().to_owned(),
                });
            }
            map.insert(entry.id.clone(), entry);
        }
        Ok(Self { entries: map })
    }

    /// The bundled entries: one per provider row of subplan 14's overlap
    /// table, parsed and validated from the embedded JSON through the same
    /// path as data files.
    pub fn bundled() -> Result<Self, RegistryError> {
        Self::from_entries(parse_entries(BUNDLED_SOURCE, BUNDLED_USAGE_PROVIDERS_JSON)?)
    }

    /// Load a directory of `*.json` data files fresh from disk; each file
    /// holds one entry or an array of entries. Adding a standard provider
    /// is adding a file here, not a Rust edit.
    pub fn load_dir(dir: &Path) -> Result<Self, RegistryError> {
        let listing = std::fs::read_dir(dir).map_err(|source| RegistryError::Read {
            path: dir.display().to_string(),
            source,
        })?;
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        for item in listing {
            let entry = item.map_err(|source| RegistryError::Read {
                path: dir.display().to_string(),
                source,
            })?;
            // Regular data files only: opening a FIFO or device node named
            // *.json blocks forever, and a followed symlink is not trusted
            // (same discipline as the reader scan in `crate::reader`).
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_file() {
                continue;
            }
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            {
                paths.push(path);
            }
        }
        paths.sort();
        let mut entries = Vec::new();
        for path in paths {
            let text = std::fs::read_to_string(&path).map_err(|source| RegistryError::Read {
                path: path.display().to_string(),
                source,
            })?;
            entries.extend(parse_entries(&path.display().to_string(), &text)?);
        }
        Self::from_entries(entries)
    }

    /// Validate every entry.
    pub fn validate(&self) -> Result<(), RegistryError> {
        for entry in self.entries.values() {
            entry.validate()?;
        }
        Ok(())
    }

    /// Look up an entry by provider id.
    pub fn get(&self, id: &ProviderId) -> Option<&UsageProviderEntry> {
        self.entries.get(id)
    }

    /// Iterate entries ordered by provider id.
    pub fn entries(&self) -> impl Iterator<Item = &UsageProviderEntry> {
        self.entries.values()
    }

    /// Number of registered entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Parse and validate one JSON document (single entry or array); the
/// bundled seed and every loaded data file flow through here.
fn parse_entries(source: &str, text: &str) -> Result<Vec<UsageProviderEntry>, RegistryError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| RegistryError::Parse {
            path: source.to_owned(),
            reason: e.to_string(),
        })?;
    let raw = match value {
        serde_json::Value::Array(items) => items,
        single => vec![single],
    };
    let mut entries = Vec::new();
    for item in raw {
        let entry: UsageProviderEntry =
            serde_json::from_value(item).map_err(|e| RegistryError::Parse {
                path: source.to_owned(),
                reason: e.to_string(),
            })?;
        entry.validate()?;
        entries.push(entry);
    }
    Ok(entries)
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

/// Exact loopback match only: `127.0.0.1.attacker.com` is a DNS name an
/// attacker controls, not a loopback address.
fn is_loopback_host(host: &str) -> bool {
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

/// Host part of a URL authority with brackets and port stripped:
/// `[::1]:8080` -> `::1`, `127.0.0.1:99` -> `127.0.0.1`.
fn strip_port(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default();
    }
    authority.split(':').next().unwrap_or_default()
}

fn endpoint_url_reason(url: &str) -> Option<String> {
    let (rest, https) = if let Some(rest) = url.strip_prefix("https://") {
        (rest, true)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (rest, false)
    } else {
        return Some("must be an https URL".to_owned());
    };
    if url
        .chars()
        .any(|c| char::is_control(c) || c.is_whitespace())
    {
        return Some("must not contain control or whitespace characters".to_owned());
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest.get(..authority_end).unwrap_or_default();
    if authority.contains('@') {
        return Some("must not carry userinfo".to_owned());
    }
    let host = strip_port(authority);
    if host.is_empty() {
        return Some("must name a host".to_owned());
    }
    if !https && !is_loopback_host(host) {
        return Some("plain http is allowed only on a loopback host".to_owned());
    }
    None
}

fn path_template_reason(template: &str) -> Option<String> {
    (template.trim().is_empty() || template.chars().any(char::is_control))
        .then(|| "must be a non-empty control-free path template".to_owned())
}

fn env_name_reason(name: &str) -> Option<String> {
    (name.trim().is_empty()
        || name
            .chars()
            .any(|c| char::is_control(c) || c.is_whitespace()))
    .then(|| "env var names carry no spaces or control characters".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, label: &str) -> UsageProviderEntry {
        let id = ProviderId::new(id).unwrap_or_else(|e| panic!("invalid id `{id}`: {e:?}"));
        UsageProviderEntry {
            id,
            label: label.to_owned(),
            locators: Vec::new(),
            auth_material: Vec::new(),
            window_strategy: WindowStrategy::ProviderOnly,
            pricing: PricingReference::Unverified,
            documentation_url: None,
            schema_version: USAGE_PROVIDER_SCHEMA_VERSION,
            verified_at: None,
        }
    }

    fn provider_id(id: &str) -> ProviderId {
        ProviderId::new(id).unwrap_or_else(|e| panic!("invalid id `{id}`: {e:?}"))
    }

    #[test]
    fn lookup_returns_the_entry_for_its_own_id_only() {
        let registry = UsageRegistry::from_entries([entry("anthropic", "Anthropic")])
            .unwrap_or_else(|e| panic!("valid entries rejected: {e:?}"));
        let found = registry.get(&provider_id("anthropic"));
        assert_eq!(found.map(|e| e.label.as_str()), Some("Anthropic"));
        assert!(registry.get(&provider_id("openrouter")).is_none());
    }

    #[test]
    fn duplicate_provider_ids_are_rejected() {
        let result = UsageRegistry::from_entries([
            entry("zed-cloud", "Zed"),
            entry("zed-cloud", "Zed duplicate"),
        ]);
        assert_eq!(
            result.unwrap_err().to_string(),
            "duplicate usage provider entry `zed-cloud`"
        );
    }

    #[test]
    fn entries_iterate_in_provider_id_order() {
        let registry = UsageRegistry::from_entries([
            entry("zed-cloud", "Zed"),
            entry("anthropic", "Anthropic"),
            entry("openrouter", "OpenRouter"),
        ])
        .unwrap_or_else(|e| panic!("valid entries rejected: {e:?}"));
        let ids: Vec<&str> = registry.entries().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["anthropic", "openrouter", "zed-cloud"]);
        assert_eq!(registry.len(), 3);
        assert!(!registry.is_empty());
    }

    #[test]
    fn empty_registry_is_constructible_and_queries_none() {
        let registry =
            UsageRegistry::from_entries([]).unwrap_or_else(|e| panic!("empty rejected: {e:?}"));
        assert!(registry.is_empty());
        assert!(registry.get(&provider_id("anthropic")).is_none());
        assert_eq!(registry.entries().count(), 0);
    }

    #[test]
    fn validation_rejects_a_wrong_schema_version() {
        let mut row = entry("old-schema", "Old");
        row.schema_version = USAGE_PROVIDER_SCHEMA_VERSION + 1;
        let msg = row.validate().unwrap_err().to_string();
        assert!(msg.contains("schema_version"), "got: {msg}");
    }

    #[test]
    fn validation_rejects_a_misshapen_verification_date() {
        let mut row = entry("bad-date", "Bad Date");
        row.verified_at = Some("2026/08/31".to_owned());
        let msg = row.validate().unwrap_err().to_string();
        assert!(msg.contains("verified_at"), "got: {msg}");
        row.verified_at = Some("2026-08-31".to_owned());
        row.validate()
            .unwrap_or_else(|e| panic!("ISO date rejected: {e:?}"));
    }

    #[test]
    fn validation_rejects_plain_http_off_loopback_but_allows_a_local_proxy() {
        let mut row = entry("public-http", "Public Http");
        row.locators = vec![UsageLocator::Endpoint {
            url: "http://usage.example.com/v1/usage".to_owned(),
        }];
        let msg = row.validate().unwrap_err().to_string();
        assert!(msg.contains("loopback"), "got: {msg}");

        row.locators = vec![UsageLocator::Endpoint {
            url: "http://127.0.0.1:8080/usage/summary".to_owned(),
        }];
        row.validate()
            .unwrap_or_else(|e| panic!("loopback proxy endpoint rejected: {e:?}"));
    }

    #[test]
    fn loopback_matching_is_exact_not_a_prefix() {
        let mut row = entry("dns-spoof", "Dns Spoof");
        row.locators = vec![UsageLocator::Endpoint {
            url: "http://127.0.0.1.attacker.com/v1/usage".to_owned(),
        }];
        let msg = row.validate().unwrap_err().to_string();
        assert!(msg.contains("loopback"), "got: {msg}");
    }

    #[test]
    fn ipv6_bracketed_loopback_and_query_emails_are_valid_endpoint_data() {
        let mut row = entry("ipv6-loopback", "Ipv6 Loopback");
        row.locators = vec![UsageLocator::Endpoint {
            url: "http://[::1]:8080/usage/summary".to_owned(),
        }];
        row.validate()
            .unwrap_or_else(|e| panic!("bracketed IPv6 loopback rejected: {e:?}"));

        row.locators = vec![UsageLocator::Endpoint {
            url: "https://billing.example.com/v1/usage?email=a@b.com".to_owned(),
        }];
        row.validate()
            .unwrap_or_else(|e| panic!("`@` inside a query string rejected: {e:?}"));
    }

    #[test]
    fn validation_rejects_userinfo_control_characters_and_missing_hosts() {
        for url in [
            "https://user:secret@example.com/usage",
            "https://example.com/usa\u{7}ge",
            "https:///usage",
            "ftp://example.com/usage",
        ] {
            let mut row = entry("bad-url", "Bad Url");
            row.locators = vec![UsageLocator::Endpoint {
                url: url.to_owned(),
            }];
            assert!(row.validate().is_err(), "`{url}` passed validation");
        }
    }

    #[test]
    fn validation_rejects_auth_names_that_cannot_be_looked_up() {
        let mut row = entry("bad-env", "Bad Env");
        row.auth_material = vec![AuthMaterialKind::EnvVar {
            name: "NOT A NAME".to_owned(),
        }];
        assert!(row.validate().is_err());
        row.auth_material = vec![AuthMaterialKind::EnvVar {
            name: "DEEPSEEK_API_KEY".to_owned(),
        }];
        row.validate()
            .unwrap_or_else(|e| panic!("clean env name rejected: {e:?}"));
    }
}
