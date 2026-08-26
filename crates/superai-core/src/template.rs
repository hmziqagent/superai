//! Template schema and catalog for remote distribution.
//!
//! Implements TPL-01 (catalog layout) and TPL-02 (template schema, validation,
//! semver, digest, traversal, secret/shell checks, adapter selector validation).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as ShaDigest, Sha256};

use crate::adapter::Adapter;
use crate::error::{CoreError, RedactedString, Result};
use crate::ids::{HarnessId, ProviderId, TemplateId, TemplateVersion};
use crate::instance::Instance;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Current template schema version.
pub const TEMPLATE_SCHEMA_VERSION: u32 = 1;

/// Current catalog schema version.
pub const CATALOG_SCHEMA_VERSION: u32 = 1;

/// Example template repository `owner/repo` used in docs and tests.
///
/// Domain logic must not hard-code a single repository; this constant is only
/// for examples and for [`TemplateRepoConfig::example`].
pub const EXAMPLE_REPO: &str = "freeoxide/superai-templates";

/// Maximum allowed template or catalog file size (1 MiB).
pub const MAX_TEMPLATE_BYTES: usize = 1_048_576;

// ---------------------------------------------------------------------------
// Helpers: digest, path validation, semver
// ---------------------------------------------------------------------------

/// Compute SHA-256 hex digest of `bytes`.
pub fn compute_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Verify that `bytes` match `expected` digest (hex, case-insensitive).
///
/// `expected` must be 64 hex characters (SHA-256). Returns `Ok(())` on match,
/// or `Err` with `DigestMismatch` context.
pub fn verify_digest(bytes: &[u8], expected: &str) -> Result<()> {
    let normalized = expected.trim().to_ascii_lowercase();
    if normalized.len() != 64 || !normalized.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CoreError::Validation {
            field: "digest".to_owned(),
            reason: format!(
                "digest must be 64 hex chars (sha256), got `{expected}` (len {})",
                normalized.len()
            ),
        });
    }
    let actual = compute_digest(bytes);
    if actual != normalized {
        return Err(CoreError::Verification {
            path: Path::new(expected).to_path_buf(),
            kind: "digest".to_owned(),
            reason: format!("digest mismatch: expected {normalized}, got {actual}"),
        });
    }
    Ok(())
}

/// Validate that a template content `path` is safe to join to a base URL or
/// filesystem without traversal.
///
/// Rejects empty, absolute, `..`, control chars, `:` , `\`, and NUL.
pub fn validate_template_path(path: &str) -> Result<()> {
    if path.is_empty() {
        return Err(CoreError::Validation {
            field: "path".to_owned(),
            reason: "template path must not be empty".to_owned(),
        });
    }
    if path.contains('\0') || path.chars().any(char::is_control) {
        return Err(CoreError::Validation {
            field: "path".to_owned(),
            reason: format!("template path must not contain NUL or control chars: `{path}`"),
        });
    }
    if path.contains(':') || path.contains('\\') {
        return Err(CoreError::Validation {
            field: "path".to_owned(),
            reason: format!("template path must not contain ':' or '\\': `{path}`"),
        });
    }
    if path.starts_with('/') {
        return Err(CoreError::Validation {
            field: "path".to_owned(),
            reason: format!("template path must be relative, got `{path}`"),
        });
    }
    let p = Path::new(path);
    for comp in p.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(CoreError::Validation {
                field: "path".to_owned(),
                reason: format!("template path must not contain '..': `{path}`"),
            });
        }
        if matches!(comp, Component::Prefix(_)) || matches!(comp, Component::RootDir) {
            return Err(CoreError::Validation {
                field: "path".to_owned(),
                reason: format!("template path must not be absolute: `{path}`"),
            });
        }
    }
    if path.split('/').any(str::is_empty) {
        return Err(CoreError::Validation {
            field: "path".to_owned(),
            reason: format!("template path must not contain empty segments or '//': `{path}`"),
        });
    }
    Ok(())
}

/// Parse a semver string, returning the parsed version or a validation error.
pub fn parse_semver(version: &str) -> Result<semver::Version> {
    semver::Version::parse(version).map_err(|e| CoreError::Validation {
        field: "version".to_owned(),
        reason: format!("invalid semver `{version}`: {e}"),
    })
}

/// Compare two semver strings.
///
/// Returns `Ok(ordering)` where `ordering` is `candidate` compared to `current`.
/// `Ok(true)` helper `is_newer_version` returns whether `candidate` is newer.
pub fn compare_semver(current: &str, candidate: &str) -> Result<std::cmp::Ordering> {
    let cur = parse_semver(current)?;
    let cand = parse_semver(candidate)?;
    Ok(cand.cmp(&cur))
}

/// Return `true` if `candidate` is strictly newer than `current`.
pub fn is_newer_version(current: &str, candidate: &str) -> Result<bool> {
    Ok(compare_semver(current, candidate)? == std::cmp::Ordering::Greater)
}

// ---------------------------------------------------------------------------
// Forbidden payload detection
// ---------------------------------------------------------------------------

/// Heuristic patterns that indicate an embedded secret.
const SECRET_PATTERNS: &[&str] = &[
    "api_key", "apikey", "api-key", "secret", "password", "passwd", "token", "bearer", "sk-",
    "ak_", "aws_",
];

/// Heuristic shell meta-characters or commands that must not appear in
/// patch values. Templates must not contain executable shell code.
const SHELL_PATTERNS: &[&str] = &[
    "`", "$(", "${", "&&", "||", ";", "|", ">", "sh -c", "bash -c", "exec ", "eval ", "curl ",
    "wget ",
];

/// Check whether a JSON value contains forbidden secret/shell/binary content.
///
/// Returns `Ok(())` if clean, or `Err` describing the first violation.
#[expect(
    clippy::excessive_nesting,
    reason = "forbidden payload checks are branched"
)]
pub fn check_value_forbidden(value: &Value) -> Result<()> {
    match value {
        Value::String(s) => {
            let lower = s.to_ascii_lowercase();
            for pat in SECRET_PATTERNS {
                if lower.contains(pat) {
                    return Err(CoreError::Validation {
                        field: "patches.value".to_owned(),
                        reason: format!(
                            "patch value contains forbidden secret pattern `{pat}`: value contains `{pat}`"
                        ),
                    });
                }
            }
            for pat in SHELL_PATTERNS {
                let pat_lower = pat.to_ascii_lowercase();
                if lower.contains(&pat_lower) || s.contains(pat) {
                    return Err(CoreError::Validation {
                        field: "patches.value".to_owned(),
                        reason: format!("patch value contains forbidden shell pattern `{pat}`"),
                    });
                }
            }
            if s.contains('\0') {
                return Err(CoreError::Validation {
                    field: "patches.value".to_owned(),
                    reason: "patch value must not contain NUL".to_owned(),
                });
            }
            if s.len() > 16_384 {
                return Err(CoreError::Validation {
                    field: "patches.value".to_owned(),
                    reason: "patch value exceeds 16 KiB limit".to_owned(),
                });
            }
            // Binary payload heuristic: large base64-looking blob without spaces.
            if s.len() > 1024
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
            {
                return Err(CoreError::Validation {
                    field: "patches.value".to_owned(),
                    reason: "patch value looks like an embedded binary/base64 blob".to_owned(),
                });
            }
            Ok(())
        }
        Value::Array(arr) => {
            for item in arr {
                check_value_forbidden(item)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for (k, v) in map {
                // Key itself must not look like a secret/shell pattern.
                let klower = k.to_ascii_lowercase();
                for pat in SHELL_PATTERNS {
                    if klower.contains(&pat.to_ascii_lowercase()) || k.contains(pat) {
                        return Err(CoreError::Validation {
                            field: "patches.value".to_owned(),
                            reason: format!(
                                "patch object key contains forbidden shell pattern `{pat}`"
                            ),
                        });
                    }
                }
                // For object values, recurse.
                check_value_forbidden(v)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// TemplateStatus
// ---------------------------------------------------------------------------

/// Lifecycle status of a template catalog entry or template file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateStatus {
    /// Actively maintained and recommended.
    Active,
    /// Preview or beta, may still change.
    Preview,
    /// Deprecated but still available; points to a replacement.
    Deprecated,
    /// Yanked for unsafe or broken content; must not be used for new instances.
    Yanked,
}

impl std::fmt::Display for TemplateStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Active => "active",
            Self::Preview => "preview",
            Self::Deprecated => "deprecated",
            Self::Yanked => "yanked",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// TemplateFileRef + TemplateCatalogEntry + Catalog
// ---------------------------------------------------------------------------

/// One immutable version file for a template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateFileRef {
    /// Template version for this file.
    pub version: TemplateVersion,
    /// Relative path to the file inside the repository (e.g. `claude-glm/1.2.0.json`).
    pub path: String,
    /// SHA-256 hex digest of the file content.
    pub digest: String,
}

impl TemplateFileRef {
    /// Validate path traversal and digest format.
    pub fn validate(&self) -> Result<()> {
        // TemplateVersion validated via newtype construction.
        validate_template_path(&self.path)?;
        let d = self.digest.trim();
        if d.len() != 64 || !d.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CoreError::Validation {
                field: "digest".to_owned(),
                reason: format!(
                    "file digest must be 64 hex chars, got `{}` (len {})",
                    self.digest,
                    d.len()
                ),
            });
        }
        // Ensure digest is lowercased?
        if d.chars().any(|c| c.is_ascii_uppercase()) {
            return Err(CoreError::Validation {
                field: "digest".to_owned(),
                reason: "digest must be lowercase hex".to_owned(),
            });
        }
        Ok(())
    }
}

/// One template's entry in the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateCatalogEntry {
    /// Template identifier.
    pub id: TemplateId,
    /// Latest version available for this template.
    pub latest_version: TemplateVersion,
    /// All available version files (immutable history).
    pub files: Vec<TemplateFileRef>,
    /// Lifecycle status of the template overall.
    pub status: TemplateStatus,
}

impl TemplateCatalogEntry {
    /// Validate the entry: latest version must exist in files, no duplicates, etc.
    pub fn validate(&self) -> Result<()> {
        if self.files.is_empty() {
            return Err(CoreError::Validation {
                field: "files".to_owned(),
                reason: format!("template `{}` must have at least one file", self.id),
            });
        }
        parse_semver(self.latest_version.as_str())?;
        let mut seen_versions: HashSet<String> = HashSet::new();
        let mut seen_paths: HashSet<String> = HashSet::new();
        let mut found_latest = false;
        for f in &self.files {
            f.validate()?;
            parse_semver(f.version.as_str())?;
            let ver_norm = f.version.as_str().to_owned();
            if !seen_versions.insert(ver_norm.clone()) {
                return Err(CoreError::Validation {
                    field: "files.version".to_owned(),
                    reason: format!("duplicate version `{ver_norm}` in template `{}`", self.id),
                });
            }
            if !seen_paths.insert(f.path.clone()) {
                return Err(CoreError::Validation {
                    field: "files.path".to_owned(),
                    reason: format!("duplicate path `{}` in template `{}`", f.path, self.id),
                });
            }
            if f.version == self.latest_version {
                found_latest = true;
            }
        }
        if !found_latest {
            return Err(CoreError::Validation {
                field: "latest_version".to_owned(),
                reason: format!(
                    "latest_version `{}` not found in files for template `{}`",
                    self.latest_version, self.id
                ),
            });
        }
        // Ensure latest_version is indeed the maximum semver among files.
        let first_version = self.files.first().map_or("0.0.0", |f| f.version.as_str());
        let mut max_ver = parse_semver(first_version)?;
        for f in &self.files {
            let v = parse_semver(f.version.as_str())?;
            if v > max_ver {
                max_ver = v;
            }
        }
        let latest_parsed = parse_semver(self.latest_version.as_str())?;
        if latest_parsed != max_ver {
            return Err(CoreError::Validation {
                field: "latest_version".to_owned(),
                reason: format!(
                    "latest_version `{}` is not the maximum semver (max is `{max_ver}`) for `{}`",
                    self.latest_version, self.id
                ),
            });
        }
        Ok(())
    }

    /// Find the file ref for a given version string, if any.
    pub fn file_for_version(&self, version: &str) -> Option<&TemplateFileRef> {
        self.files.iter().find(|f| f.version.as_str() == version)
    }
}

/// Catalog listing all templates, their latest versions, and digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    /// Catalog schema version.
    #[serde(default = "default_catalog_version")]
    pub version: u32,
    /// All template entries.
    pub templates: Vec<TemplateCatalogEntry>,
}

fn default_catalog_version() -> u32 {
    CATALOG_SCHEMA_VERSION
}

impl Catalog {
    /// Validate the entire catalog.
    pub fn validate(&self) -> Result<()> {
        if self.version != CATALOG_SCHEMA_VERSION {
            return Err(CoreError::Validation {
                field: "version".to_owned(),
                reason: format!(
                    "catalog version must be {}, got {}",
                    CATALOG_SCHEMA_VERSION, self.version
                ),
            });
        }
        let mut seen_ids: HashSet<String> = HashSet::new();
        for entry in &self.templates {
            let id_str = entry.id.as_str().to_owned();
            if !seen_ids.insert(id_str.clone()) {
                return Err(CoreError::Validation {
                    field: "templates.id".to_owned(),
                    reason: format!("duplicate template id `{id_str}`"),
                });
            }
            entry.validate()?;
        }
        Ok(())
    }

    /// Find an entry by template id string.
    pub fn find(&self, id: &str) -> Option<&TemplateCatalogEntry> {
        self.templates.iter().find(|e| e.id.as_str() == id)
    }

    /// Parse and validate from JSON bytes, checking size limit.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_TEMPLATE_BYTES {
            return Err(CoreError::Validation {
                field: "catalog".to_owned(),
                reason: format!(
                    "catalog exceeds size limit {} bytes (got {})",
                    MAX_TEMPLATE_BYTES,
                    bytes.len()
                ),
            });
        }
        let catalog: Self =
            serde_json::from_slice(bytes).map_err(|e| CoreError::SchemaValidation {
                path: Path::new("catalog.json").to_path_buf(),
                details: format!("catalog json invalid: {e}"),
            })?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// Serialize to JSON bytes (canonical).
    pub fn to_json_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| CoreError::Validation {
            field: "catalog".to_owned(),
            reason: format!("catalog serialize failed: {e}"),
        })
    }
}

// ---------------------------------------------------------------------------
// Template config (host/owner/repo/ref/base_url)
// ---------------------------------------------------------------------------

/// Configuration for locating the remote template repository.
///
/// No field is hard-coded to a single owner or host; the example
/// `freeoxide/superai-templates` is only the default for
/// [`Self::example`]. All callers must supply a config or explicitly use
/// `example()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateRepoConfig {
    /// Git host, e.g. `raw.githubusercontent.com` or `github.example.com`.
    pub host: String,
    /// Repository owner or organization.
    pub owner: String,
    /// Repository name.
    pub repo: String,
    /// Git ref, tag, branch, or commit SHA that pins the channel.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// Optional fully-qualified base URL that overrides `https://{host}/{owner}/{repo}/{ref}`.
    ///
    /// When `Some`, it is used verbatim as the prefix for `catalog_url()` and
    /// `template_url()`. This is intended for tests (`file://` or local
    /// `http://`) and for self-hosted mirrors. When `None`, the URL is built
    /// from `host`/`owner`/`repo`/`git_ref`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

impl TemplateRepoConfig {
    /// Create a new config from individual fields.
    pub fn new(host: &str, owner: &str, repo: &str, git_ref: &str) -> Result<Self> {
        let cfg = Self {
            host: host.to_owned(),
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            git_ref: git_ref.to_owned(),
            base_url: None,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    /// Example config pointing at `freeoxide/superai-templates` pinned to `main`.
    pub fn example() -> Self {
        Self {
            host: "raw.githubusercontent.com".to_owned(),
            owner: "freeoxide".to_owned(),
            repo: "superai-templates".to_owned(),
            git_ref: "main".to_owned(),
            base_url: None,
        }
    }

    /// Validate host/owner/repo/ref for traversal and emptiness.
    pub fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("host", self.host.as_str()),
            ("owner", self.owner.as_str()),
            ("repo", self.repo.as_str()),
            ("ref", self.git_ref.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(CoreError::Validation {
                    field: field.to_owned(),
                    reason: format!("{field} must not be empty"),
                });
            }
            if value.contains('\0') || value.chars().any(char::is_control) {
                return Err(CoreError::Validation {
                    field: field.to_owned(),
                    reason: format!("{field} must not contain NUL or control chars"),
                });
            }
            if value.contains('/') || value.contains('\\') || value.contains(':') {
                // host may contain '.' and '-', owner/repo may contain '-' '_' '.'
                // but must not contain path separators. For host, allow '.' and '-'.
                // Check specifically for slashes which would allow traversal; colon is
                // also rejected to prevent scheme injection.
                return Err(CoreError::Validation {
                    field: field.to_owned(),
                    reason: format!("{field} must not contain '/', '\\', or ':'"),
                });
            }
            if value == "." || value == ".." {
                return Err(CoreError::Validation {
                    field: field.to_owned(),
                    reason: format!("{field} must not be '.' or '..'"),
                });
            }
        }
        if let Some(base) = self.base_url.as_deref() {
            if base.trim().is_empty() {
                return Err(CoreError::Validation {
                    field: "base_url".to_owned(),
                    reason: "base_url must not be empty if provided".to_owned(),
                });
            }
            // base_url may be file:// for tests or https:// for real. Validate not
            // containing NUL/control and that it parses as having a scheme.
            if base.contains('\0') {
                return Err(CoreError::Validation {
                    field: "base_url".to_owned(),
                    reason: "base_url must not contain NUL".to_owned(),
                });
            }
        }
        Ok(())
    }

    /// Build the catalog URL for this config.
    ///
    /// If `base_url` is `Some`, returns `{base_url}/catalog.json` (ensuring one
    /// slash). Otherwise builds `https://{host}/{owner}/{repo}/raw/{ref}/catalog.json`
    /// for raw.githubusercontent.com style, or `https://{host}/{owner}/{repo}/{ref}/catalog.json`
    /// generically. For `raw.githubusercontent.com` we emit the canonical raw path.
    pub fn catalog_url(&self) -> Result<String> {
        self.validate()?;
        if let Some(base) = self.base_url.as_deref() {
            let trimmed = base.trim_end_matches('/');
            if trimmed.starts_with("file://") {
                // For file URLs, validation is filesystem path safety; still return as-is.
                return Ok(format!("{trimmed}/catalog.json"));
            }
            if !trimmed.starts_with("https://") && !trimmed.starts_with("http://") {
                return Err(CoreError::Validation {
                    field: "base_url".to_owned(),
                    reason: format!(
                        "base_url must start with https:// or file://, got `{trimmed}`"
                    ),
                });
            }
            return Ok(format!("{trimmed}/catalog.json"));
        }
        // Canonical construction. For raw.githubusercontent.com the raw path is
        // /{owner}/{repo}/{ref}/catalog.json
        Ok(format!(
            "https://{}/{}/{}/{}/catalog.json",
            self.host, self.owner, self.repo, self.git_ref
        ))
    }

    /// Build a template file URL for a given relative path (validated for traversal).
    pub fn template_url(&self, relative_path: &str) -> Result<String> {
        self.validate()?;
        validate_template_path(relative_path)?;
        if let Some(base) = self.base_url.as_deref() {
            let trimmed = base.trim_end_matches('/');
            if trimmed.starts_with("file://") {
                return Ok(format!("{trimmed}/{relative_path}"));
            }
            if !trimmed.starts_with("https://") {
                return Err(CoreError::Validation {
                    field: "base_url".to_owned(),
                    reason: "base_url must be https:// when fetching remote templates".to_owned(),
                });
            }
            return Ok(format!("{trimmed}/{relative_path}"));
        }
        Ok(format!(
            "https://{}/{}/{}/{}/{}",
            self.host, self.owner, self.repo, self.git_ref, relative_path
        ))
    }
}

// ---------------------------------------------------------------------------
// Template schema (TPL-02)
// ---------------------------------------------------------------------------

/// One required or optional input the user must supply when instantiating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateInput {
    /// Input key, e.g. `api_key`, `region`, `model`.
    pub key: String,
    /// Human description.
    #[serde(default)]
    pub description: String,
    /// Whether the input is required.
    #[serde(default)]
    pub required: bool,
}

impl TemplateInput {
    /// Validate the input entry.
    pub fn validate(&self) -> Result<()> {
        if self.key.trim().is_empty() {
            return Err(CoreError::Validation {
                field: "inputs.key".to_owned(),
                reason: "input key must not be empty".to_owned(),
            });
        }
        if self.key.contains('/') || self.key.contains('\\') || self.key.contains(':') {
            return Err(CoreError::Validation {
                field: "inputs.key".to_owned(),
                reason: format!(
                    "input key must not contain '/', '\\', or ':': `{}`",
                    self.key
                ),
            });
        }
        if self.key.contains('\0') || self.key.chars().any(char::is_control) {
            return Err(CoreError::Validation {
                field: "inputs.key".to_owned(),
                reason: "input key must not contain NUL or control chars".to_owned(),
            });
        }
        Ok(())
    }
}

/// One owned config patch that a template applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedPatch {
    /// Typed selector string (e.g. `key:model`, `key:env.ANTHROPIC_BASE_URL`).
    pub selector: String,
    /// Value to set at the selector.
    pub value: Value,
}

impl OwnedPatch {
    /// Validate selector syntax and forbid secret/shell/binary payloads.
    pub fn validate(&self) -> Result<()> {
        if self.selector.trim().is_empty() {
            return Err(CoreError::Validation {
                field: "patches.selector".to_owned(),
                reason: "selector must not be empty".to_owned(),
            });
        }
        if self.selector.contains("..") {
            return Err(CoreError::Validation {
                field: "patches.selector".to_owned(),
                reason: format!("selector must not contain '..': `{}`", self.selector),
            });
        }
        // Parse via superai-config's typed Selector to ensure syntax is valid.
        superai_config::document::Selector::parse(&self.selector).map_err(|e| {
            CoreError::Validation {
                field: "patches.selector".to_owned(),
                reason: format!("invalid selector `{}`: {e}", self.selector),
            }
        })?;
        check_value_forbidden(&self.value)?;
        Ok(())
    }
}

/// Versioned preset that turns `harness + provider` into a working instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    /// Schema version for this file.
    pub schema_version: u32,
    /// Template identifier.
    pub id: TemplateId,
    /// Semver version string, e.g. `1.2.0`.
    pub version: String,
    /// Harness this template targets.
    pub harness: HarnessId,
    /// Provider this template configures.
    pub provider: ProviderId,
    /// Human label, e.g. `Claude Code on GLM`.
    pub label: String,
    /// Lifecycle status.
    pub status: TemplateStatus,
    /// Required user inputs.
    #[serde(default)]
    pub inputs: Vec<TemplateInput>,
    /// Ordered owned config patches.
    #[serde(default)]
    pub patches: Vec<OwnedPatch>,
    /// Environment variable additions for the wrapper.
    #[serde(default)]
    pub wrapper_env: BTreeMap<String, String>,
    /// Extra wrapper arguments.
    #[serde(default)]
    pub wrapper_args: Vec<String>,
    /// Asset requirements (relative paths).
    #[serde(default)]
    pub assets: Vec<String>,
    /// Capability map (capability id -> support string).
    #[serde(default)]
    pub capability_map: BTreeMap<String, String>,
    /// Migration notes / warnings.
    #[serde(default)]
    pub migration_notes: Vec<String>,
    /// Content digest (SHA-256 hex) of the template file (excluding this field if needed).
    pub digest: String,
    /// Optional harness version requirement (semver req), e.g. `>=1.0.0, <2.0.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_version_req: Option<String>,
    /// Optional provider protocol name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_protocol: Option<String>,
}

impl Template {
    /// Validate the template: schema, semver, selectors, forbidden payloads, digest, etc.
    #[expect(clippy::too_many_lines, reason = "exhaustive template validation")]
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != TEMPLATE_SCHEMA_VERSION {
            return Err(CoreError::Validation {
                field: "schema_version".to_owned(),
                reason: format!(
                    "schema_version must be {}, got {}",
                    TEMPLATE_SCHEMA_VERSION, self.schema_version
                ),
            });
        }
        if self.label.trim().is_empty() {
            return Err(CoreError::Validation {
                field: "label".to_owned(),
                reason: "label must not be empty".to_owned(),
            });
        }
        parse_semver(&self.version)?;
        // digest must be 64 hex
        let d = self.digest.trim();
        if d.len() != 64 || !d.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CoreError::Validation {
                field: "digest".to_owned(),
                reason: format!(
                    "digest must be 64 hex chars (sha256), got `{}` (len {})",
                    self.digest,
                    d.len()
                ),
            });
        }
        if d.chars().any(|c| c.is_ascii_uppercase()) {
            return Err(CoreError::Validation {
                field: "digest".to_owned(),
                reason: "digest must be lowercase hex".to_owned(),
            });
        }
        if self.label.chars().any(char::is_control) || self.label.contains('\0') {
            return Err(CoreError::Validation {
                field: "label".to_owned(),
                reason: "label must not contain control chars".to_owned(),
            });
        }
        let mut seen_keys: HashSet<String> = HashSet::new();
        for input in &self.inputs {
            input.validate()?;
            let norm = input.key.to_ascii_lowercase();
            if !seen_keys.insert(norm.clone()) {
                return Err(CoreError::Validation {
                    field: "inputs.key".to_owned(),
                    reason: format!("duplicate input key `{}`", input.key),
                });
            }
        }
        let mut seen_selectors: HashSet<String> = HashSet::new();
        for patch in &self.patches {
            patch.validate()?;
            if !seen_selectors.insert(patch.selector.clone()) {
                return Err(CoreError::Validation {
                    field: "patches.selector".to_owned(),
                    reason: format!("duplicate selector `{}`", patch.selector),
                });
            }
        }
        for (k, v) in &self.wrapper_env {
            if k.trim().is_empty() {
                return Err(CoreError::Validation {
                    field: "wrapper_env".to_owned(),
                    reason: "wrapper_env key must not be empty".to_owned(),
                });
            }
            if k.contains('\0') || k.chars().any(char::is_control) {
                return Err(CoreError::Validation {
                    field: "wrapper_env".to_owned(),
                    reason: format!("wrapper_env key must not contain control/NUL: `{k}`"),
                });
            }
            if v.contains('\0') {
                return Err(CoreError::Validation {
                    field: "wrapper_env".to_owned(),
                    reason: "wrapper_env value must not contain NUL".to_owned(),
                });
            }
            // Forbid secrets/shell in env values as well.
            check_value_forbidden(&Value::String(v.clone()))?;
        }
        for arg in &self.wrapper_args {
            if arg.contains('\0') {
                return Err(CoreError::Validation {
                    field: "wrapper_args".to_owned(),
                    reason: "wrapper arg must not contain NUL".to_owned(),
                });
            }
            // Shell patterns in args are forbidden (no `sh -c` etc).
            check_value_forbidden(&Value::String(arg.clone()))?;
        }
        for asset in &self.assets {
            validate_template_path(asset)?;
        }
        for note in &self.migration_notes {
            if note.contains('\0') {
                return Err(CoreError::Validation {
                    field: "migration_notes".to_owned(),
                    reason: "migration note must not contain NUL".to_owned(),
                });
            }
        }
        if let Some(req) = self.harness_version_req.as_deref() {
            if req.trim().is_empty() {
                return Err(CoreError::Validation {
                    field: "harness_version_req".to_owned(),
                    reason: "harness_version_req must not be empty if present".to_owned(),
                });
            }
            // Validate as semver req syntax.
            semver::VersionReq::parse(req).map_err(|e| CoreError::Validation {
                field: "harness_version_req".to_owned(),
                reason: format!("invalid semver req `{req}`: {e}"),
            })?;
        }
        Ok(())
    }

    /// Parse from JSON bytes, validate schema and size limit.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_TEMPLATE_BYTES {
            return Err(CoreError::Validation {
                field: "template".to_owned(),
                reason: format!(
                    "template exceeds size limit {} bytes (got {})",
                    MAX_TEMPLATE_BYTES,
                    bytes.len()
                ),
            });
        }
        let tmpl: Self =
            serde_json::from_slice(bytes).map_err(|e| CoreError::SchemaValidation {
                path: Path::new("template.json").to_path_buf(),
                details: format!("template json invalid: {e}"),
            })?;
        tmpl.validate()?;
        Ok(tmpl)
    }

    /// Verify that the template's digest matches the hash of `bytes`.
    ///
    /// The digest is computed over the raw file bytes. Callers that have the
    /// original bytes should use this to ensure the file was not tampered with.
    pub fn verify_bytes_digest(&self, bytes: &[u8]) -> Result<()> {
        verify_digest(bytes, &self.digest)
    }

    /// Validate that every patch selector is owned by the given adapter.
    ///
    /// Collects `owned_selectors` from `adapter.config_surfaces()` and checks
    /// that each `patches[].selector` appears there, or is a prefix of an
    /// owned path, or matches a `supported_operations` key if the adapter
    /// exposes operation names that correspond to selectors. The primary check
    /// is against `config_surfaces().owned_selectors`.
    #[expect(
        clippy::excessive_nesting,
        reason = "adapter selector matching branches are explicit"
    )]
    pub fn validate_against_adapter(&self, adapter: &dyn Adapter) -> Result<()> {
        let surfaces = adapter.config_surfaces();
        let mut owned: HashSet<String> = HashSet::new();
        for surface in &surfaces {
            for sel in &surface.owned_selectors {
                owned.insert(sel.clone());
                // Also insert the typed-string form if it parses as a selector,
                // so both `model` and `key:model` are recognised.
                if let Ok(parsed) = superai_config::document::Selector::parse(sel) {
                    owned.insert(parsed.to_typed_string());
                }
            }
        }
        // Also include operation names as owned keys for adapters that declare
        // ownership via supported_operations (per spec: call supported_operations
        // to verify owned keys).
        for (op, _support) in adapter.supported_operations() {
            owned.insert(op);
        }

        for patch in &self.patches {
            let selector = patch.selector.trim();
            // Direct match?
            if owned.contains(selector) {
                continue;
            }
            // Try parsed canonical form.
            let canonical = match superai_config::document::Selector::parse(selector) {
                Ok(s) => s.to_typed_string(),
                Err(_) => selector.to_owned(),
            };
            if owned.contains(&canonical) {
                continue;
            }
            // For Key selectors, check if the raw key after `key:` prefix matches.
            // e.g. patch `key:model` should match owned `model`.
            let mut matched = false;
            if let Ok(superai_config::document::Selector::Key(k)) =
                superai_config::document::Selector::parse(selector)
            {
                if owned.contains(&k) {
                    matched = true;
                }
                for o in &owned {
                    if k == *o || k.starts_with(&format!("{o}.")) || o.starts_with(&format!("{k}."))
                    {
                        matched = true;
                        break;
                    }
                    if let Ok(superai_config::document::Selector::Key(ok)) =
                        superai_config::document::Selector::parse(o)
                        && ok == k
                    {
                        matched = true;
                        break;
                    }
                }
            }
            if matched {
                continue;
            }
            return Err(CoreError::Validation {
                field: "patches.selector".to_owned(),
                reason: format!(
                    "selector `{}` is not owned by adapter `{}`; owned: {}",
                    selector,
                    adapter.id(),
                    {
                        let mut v: Vec<&String> = owned.iter().collect();
                        v.sort();
                        let joined = v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
                        if joined.is_empty() {
                            "(none)".to_owned()
                        } else {
                            joined
                        }
                    }
                ),
            });
        }
        Ok(())
    }

    /// Compute the digest that should be stored for `bytes`.
    pub fn expected_digest_for(bytes: &[u8]) -> String {
        compute_digest(bytes)
    }
}

// ---------------------------------------------------------------------------
// Update status and version discovery (TPL-04)
// ---------------------------------------------------------------------------

/// Result of checking whether an instance's template is up to date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Instance is on the latest compatible version.
    UpToDate,
    /// A newer compatible version is available.
    UpdateAvailable {
        /// Latest version available in the catalog.
        latest: TemplateVersion,
    },
    /// The template (or its current version) has been yanked.
    Yanked,
    /// The instance's template id or current version is not found in the catalog.
    CurrentMissing,
    /// A newer version exists but is not compatible with the instance's harness.
    Incompatible {
        /// Human-readable reason.
        reason: String,
    },
    /// The catalog could not be fetched (offline, network error, etc.).
    Offline,
}

impl UpdateStatus {
    /// True if an update is available.
    pub fn is_update_available(&self) -> bool {
        matches!(self, Self::UpdateAvailable { .. })
    }

    /// True if the instance is up to date.
    pub fn is_up_to_date(&self) -> bool {
        matches!(self, Self::UpToDate)
    }
}

/// Pure catalog-based update check (no network).
///
/// Validates the instance's template reference against the provided catalog
/// using semver ordering and yanked status. Harness/adapter compatibility
/// beyond semver ordering is not checked here; use [`check_update`] for the
/// network-aware variant that also verifies `harness_version_req` and harness
/// identity by fetching the candidate template.
pub fn check_update_with_catalog(instance: &Instance, catalog: &Catalog) -> UpdateStatus {
    // Instance must have a template reference to be considered.
    let Some(template_ref) = instance.template.as_ref() else {
        return UpdateStatus::CurrentMissing;
    };

    let Some(entry) = catalog.find(template_ref.name.as_str()) else {
        return UpdateStatus::CurrentMissing;
    };

    // Yanked entries are always reported as yanked.
    if entry.status == TemplateStatus::Yanked {
        return UpdateStatus::Yanked;
    }

    // Current version must exist in the catalog's file list.
    let current_version_str = template_ref.version.as_str();
    let current_file = entry.file_for_version(current_version_str);
    if current_file.is_none() {
        return UpdateStatus::CurrentMissing;
    }

    // If the current template's own file is yanked, the entry status already
    // reflects that, but also check the specific template file's status if we
    // had it; catalog entry status is authoritative for now.
    // Validate semver for current and latest.
    let current_ver = match parse_semver(current_version_str) {
        Ok(v) => v,
        Err(e) => {
            return UpdateStatus::Incompatible {
                reason: format!("current version `{current_version_str}` invalid: {e}"),
            };
        }
    };
    let latest_str = entry.latest_version.as_str();
    let latest_ver = match parse_semver(latest_str) {
        Ok(v) => v,
        Err(e) => {
            return UpdateStatus::Incompatible {
                reason: format!("latest version `{latest_str}` invalid: {e}"),
            };
        }
    };

    // Compare semver: candidate > current => update available, == => up to date,
    // < => local is ahead (treat as up to date).
    if latest_ver == current_ver {
        return UpdateStatus::UpToDate;
    }
    if latest_ver < current_ver {
        return UpdateStatus::UpToDate;
    }

    // latest > current: potential update. Check if the current version itself
    // is yanked (entry already checked) else report available.
    // For pure catalog check we cannot verify harness_version_req without
    // fetching the template file, so we report UpdateAvailable here; the
    // network-aware `check_update` will refine to Incompatible when needed.
    match TemplateVersion::new(latest_str) {
        Ok(latest) => UpdateStatus::UpdateAvailable { latest },
        Err(_) => UpdateStatus::Incompatible {
            reason: format!("latest version `{latest_str}` invalid identifier"),
        },
    }
}

/// Check whether a template's harness and `harness_version_req` are compatible
/// with the given instance.
///
/// Returns `Ok(())` if compatible, or `Err(reason)` if incompatible.
fn check_template_compatible(
    instance: &Instance,
    template: &Template,
) -> std::result::Result<(), String> {
    if template.harness != instance.harness {
        return Err(format!(
            "harness mismatch: instance `{}` requires `{}`, template targets `{}`",
            instance.name, instance.harness, template.harness
        ));
    }
    if let Some(req_str) = template.harness_version_req.as_deref() {
        let req = semver::VersionReq::parse(req_str)
            .map_err(|e| format!("invalid harness_version_req `{req_str}`: {e}"))?;
        // Use instance.adapter_revision as a proxy for the harness version.
        let harness_ver_str = instance.adapter_revision.as_str();
        // Some revisions may not be semver (e.g., "0.1.0" is semver, but if not, skip check).
        if let Ok(harness_ver) = semver::Version::parse(harness_ver_str) {
            if !req.matches(&harness_ver) {
                return Err(format!(
                    "harness version `{harness_ver_str}` does not satisfy `{req_str}`"
                ));
            }
        } else {
            // If harness version is not semver, we cannot enforce the requirement;
            // treat as compatible rather than blocking.
        }
    }
    Ok(())
}

/// Network-aware update check that fetches the catalog fresh.
///
/// Must be called with a `repo` that points at the remote template repository
/// (usually `file://` for tests). Fetches the catalog via
/// [`crate::template_fetch::fetch_catalog`]; on network failure returns
/// [`UpdateStatus::Offline`]. Otherwise uses the fresh catalog to determine
/// status, additionally verifying harness compatibility by fetching the latest
/// template file when an update appears available.
pub fn check_update(
    instance: &Instance,
    catalog: &Catalog,
    repo: &TemplateRepoConfig,
) -> UpdateStatus {
    // Validate repo early; invalid repo is treated as offline to avoid panics.
    if repo.validate().is_err() {
        return UpdateStatus::Offline;
    }
    // The provided catalog is validated opportunistically; errors are ignored
    // because fresh fetch is authoritative. Avoid `let _ =` on must_use.
    drop(catalog.validate().err());

    let Ok(fresh) = crate::template_fetch::fetch_catalog(repo) else {
        return UpdateStatus::Offline;
    };

    // First, run the pure catalog check.
    let pure_status = check_update_with_catalog(instance, &fresh);
    let latest = match &pure_status {
        UpdateStatus::UpdateAvailable { latest } => latest.clone(),
        other => return other.clone(),
    };

    // For UpdateAvailable, verify harness compatibility by fetching the latest template.
    let template_id = match instance.template.as_ref() {
        Some(t) => t.name.as_str(),
        None => return UpdateStatus::CurrentMissing,
    };
    let latest_str = latest.as_str();
    match crate::template_fetch::fetch_template(repo, &fresh, template_id, latest_str) {
        Ok(template) => {
            if template.status == TemplateStatus::Yanked {
                return UpdateStatus::Yanked;
            }
            match check_template_compatible(instance, &template) {
                Ok(()) => UpdateStatus::UpdateAvailable { latest },
                Err(reason) => UpdateStatus::Incompatible { reason },
            }
        }
        Err(e) => {
            // Map fetch errors: offline vs missing.
            let msg = format!("{e}");
            // If the template file is not found, treat as CurrentMissing rather than Offline.
            if msg.contains("not found") || msg.contains("NotFound") {
                UpdateStatus::CurrentMissing
            } else if msg.contains("digest") || msg.contains("DigestMismatch") {
                UpdateStatus::Incompatible {
                    reason: format!("latest template digest mismatch: {msg}"),
                }
            } else {
                // For network/rate-limit etc., treat as Offline to be safe, but only if the catalog itself was fresh.
                // Since we already fetched the catalog, a template fetch failure is more likely a catalog inconsistency.
                UpdateStatus::Incompatible {
                    reason: format!("cannot fetch latest template `{latest_str}`: {msg}"),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Template diff (TPL-05)
// ---------------------------------------------------------------------------

fn is_secret_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    for pat in SECRET_PATTERNS {
        if lower.contains(pat) {
            return true;
        }
    }
    false
}

fn redact_string(value: &str) -> String {
    if is_secret_like(value) {
        RedactedString::placeholder().to_owned()
    } else {
        value.to_owned()
    }
}

fn redact_value(value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(redact_string(s)),
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(redact_value(item));
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            for (k, v) in map {
                new_map.insert(k.clone(), redact_value(v));
            }
            Value::Object(new_map)
        }
        other => other.clone(),
    }
}

/// Diff of provider-related fields.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderDiff {
    /// Provider id changed `Some((old, new))`.
    pub provider_changed: Option<(String, String)>,
    /// Provider protocol changed `Some((old, new))`.
    pub protocol_changed: Option<(Option<String>, Option<String>)>,
    /// `true` when any provider field changed.
    pub has_change: bool,
}

/// Diff of model-related patches (selectors containing "model").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelChanges {
    /// Selectors that were added (model-related).
    pub added: Vec<(String, Value)>,
    /// Selectors that were removed (model-related).
    pub removed: Vec<(String, Value)>,
    /// Selectors whose value changed: `(selector, old, new)`.
    pub changed: Vec<(String, Value, Value)>,
    /// Convenience: default model value changed, if any.
    pub default_changed: Option<(Value, Value)>,
}

/// Diff of capability map entries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CapabilityChanges {
    /// Added capabilities: `(key, new_value)`.
    pub added: Vec<(String, String)>,
    /// Removed capabilities: `(key, old_value)`.
    pub removed: Vec<(String, String)>,
    /// Changed capabilities: `(key, old_value, new_value)`.
    pub changed: Vec<(String, String, String)>,
}

/// Diff of wrapper env (`wrapper_env`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EnvChanges {
    /// Added env entries.
    pub added: Vec<(String, String)>,
    /// Removed env entries.
    pub removed: Vec<(String, String)>,
    /// Changed env entries: `(key, old, new)`.
    pub changed: Vec<(String, String, String)>,
}

/// Diff of wrapper args.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArgsChanges {
    /// Args present in new but not old.
    pub added: Vec<String>,
    /// Args present in old but not new.
    pub removed: Vec<String>,
}

/// Diff of asset lists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssetChanges {
    /// Added asset paths.
    pub added: Vec<String>,
    /// Removed asset paths.
    pub removed: Vec<String>,
}

/// Diff of selector patches (all patches).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SelectorChanges {
    /// Added selectors.
    pub added: Vec<(String, Value)>,
    /// Removed selectors.
    pub removed: Vec<(String, Value)>,
    /// Changed selectors: `(selector, old, new)`.
    pub changed: Vec<(String, Value, Value)>,
}

/// Diff of input definitions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputChanges {
    /// Added input keys.
    pub added: Vec<String>,
    /// Removed input keys.
    pub removed: Vec<String>,
}

/// Semantic diff between two template versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateDiff {
    /// Template identifier (should be equal for old and new).
    pub template_id: String,
    /// Version of `old`.
    pub from_version: String,
    /// Version of `new`.
    pub to_version: String,
    /// Harness changed, if any.
    pub harness_changed: Option<(String, String)>,
    /// Provider-related diff.
    pub provider_diff: ProviderDiff,
    /// Context changes: harness version requirement.
    pub harness_version_req_changed: Option<(Option<String>, Option<String>)>,
    /// Status changed.
    pub status_changed: Option<(TemplateStatus, TemplateStatus)>,
    /// Label changed.
    pub label_changed: Option<(String, String)>,
    /// Model-related changes.
    pub model_changes: ModelChanges,
    /// Capability map changes.
    pub capability_changes: CapabilityChanges,
    /// Wrapper env changes.
    pub wrapper_env_changes: EnvChanges,
    /// Wrapper args changes.
    pub wrapper_args_changes: ArgsChanges,
    /// Asset changes.
    pub asset_changes: AssetChanges,
    /// All selector/patch changes.
    pub selector_changes: SelectorChanges,
    /// Input definition changes.
    pub input_changes: InputChanges,
    /// Migration notes that are newly added in `new` (redacted for display).
    pub migration_notes_added: Vec<String>,
    /// True when any semantic change exists.
    pub has_changes: bool,
}

impl TemplateDiff {
    /// True when the two templates are semantically identical (no changes).
    pub fn is_empty(&self) -> bool {
        !self.has_changes
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "diff assembles multiple semantic categories"
)]
/// Compute a semantic diff between two template versions.
///
/// Redacts secret-like values via [`RedactedString`] and reports provider,
/// model, capability, wrapper, asset and selector changes.
pub fn diff_templates(old: &Template, new: &Template) -> TemplateDiff {
    let template_id = old.id.to_string();
    let from_version = old.version.clone();
    let to_version = new.version.clone();

    // --- provider / harness / status / label / version req ---
    let harness_changed = if old.harness == new.harness {
        None
    } else {
        Some((old.harness.to_string(), new.harness.to_string()))
    };

    let provider_changed = if old.provider == new.provider {
        None
    } else {
        Some((old.provider.to_string(), new.provider.to_string()))
    };
    let protocol_changed = if old.provider_protocol == new.provider_protocol {
        None
    } else {
        Some((old.provider_protocol.clone(), new.provider_protocol.clone()))
    };
    let provider_diff = ProviderDiff {
        has_change: provider_changed.is_some() || protocol_changed.is_some(),
        provider_changed,
        protocol_changed,
    };

    let harness_version_req_changed = if old.harness_version_req == new.harness_version_req {
        None
    } else {
        Some((
            old.harness_version_req.clone(),
            new.harness_version_req.clone(),
        ))
    };

    let status_changed = if old.status == new.status {
        None
    } else {
        Some((old.status, new.status))
    };

    let label_changed = if old.label == new.label {
        None
    } else {
        Some((redact_string(&old.label), redact_string(&new.label)))
    };

    // --- capability map ---
    let mut cap_added = Vec::new();
    let mut cap_removed = Vec::new();
    let mut cap_changed = Vec::new();
    for (k, old_v) in &old.capability_map {
        match new.capability_map.get(k) {
            None => cap_removed.push((k.clone(), redact_string(old_v))),
            Some(new_v) => {
                if old_v != new_v {
                    cap_changed.push((k.clone(), redact_string(old_v), redact_string(new_v)));
                }
            }
        }
    }
    for (k, new_v) in &new.capability_map {
        if !old.capability_map.contains_key(k) {
            cap_added.push((k.clone(), redact_string(new_v)));
        }
    }
    let capability_changes = CapabilityChanges {
        added: cap_added,
        removed: cap_removed,
        changed: cap_changed,
    };

    // --- wrapper env ---
    let mut env_added = Vec::new();
    let mut env_removed = Vec::new();
    let mut env_changed = Vec::new();
    for (k, old_v) in &old.wrapper_env {
        match new.wrapper_env.get(k) {
            None => env_removed.push((k.clone(), redact_string(old_v))),
            Some(new_v) => {
                if old_v != new_v {
                    env_changed.push((k.clone(), redact_string(old_v), redact_string(new_v)));
                }
            }
        }
    }
    for (k, new_v) in &new.wrapper_env {
        if !old.wrapper_env.contains_key(k) {
            env_added.push((k.clone(), redact_string(new_v)));
        }
    }
    let wrapper_env_changes = EnvChanges {
        added: env_added,
        removed: env_removed,
        changed: env_changed,
    };

    // --- wrapper args (set diff) ---
    let old_args_set: BTreeSet<&String> = old.wrapper_args.iter().collect();
    let new_args_set: BTreeSet<&String> = new.wrapper_args.iter().collect();
    let mut args_added = Vec::new();
    let mut args_removed = Vec::new();
    for arg in &old.wrapper_args {
        if !new_args_set.contains(arg) {
            args_removed.push(redact_string(arg));
        }
    }
    for arg in &new.wrapper_args {
        if !old_args_set.contains(arg) {
            args_added.push(redact_string(arg));
        }
    }
    let wrapper_args_changes = ArgsChanges {
        added: args_added,
        removed: args_removed,
    };

    // --- assets (set diff) ---
    let old_assets_set: BTreeSet<&String> = old.assets.iter().collect();
    let new_assets_set: BTreeSet<&String> = new.assets.iter().collect();
    let mut asset_added = Vec::new();
    let mut asset_removed = Vec::new();
    for a in &old.assets {
        if !new_assets_set.contains(a) {
            asset_removed.push(a.clone());
        }
    }
    for a in &new.assets {
        if !old_assets_set.contains(a) {
            asset_added.push(a.clone());
        }
    }
    let asset_changes = AssetChanges {
        added: asset_added,
        removed: asset_removed,
    };

    // --- selector / patch diff (all patches) ---
    let mut old_map: BTreeMap<String, Value> = BTreeMap::new();
    for p in &old.patches {
        old_map.insert(p.selector.clone(), redact_value(&p.value));
    }
    let mut new_map: BTreeMap<String, Value> = BTreeMap::new();
    for p in &new.patches {
        new_map.insert(p.selector.clone(), redact_value(&p.value));
    }
    let mut sel_added = Vec::new();
    let mut sel_removed = Vec::new();
    let mut sel_changed = Vec::new();
    for (sel, old_val) in &old_map {
        match new_map.get(sel) {
            None => sel_removed.push((sel.clone(), old_val.clone())),
            Some(new_val) => {
                if old_val != new_val {
                    sel_changed.push((sel.clone(), old_val.clone(), new_val.clone()));
                }
            }
        }
    }
    for (sel, new_val) in &new_map {
        if !old_map.contains_key(sel) {
            sel_added.push((sel.clone(), new_val.clone()));
        }
    }
    let selector_changes = SelectorChanges {
        added: sel_added.clone(),
        removed: sel_removed.clone(),
        changed: sel_changed.clone(),
    };

    // --- model changes: filter selector changes where selector contains "model" ---
    let mut model_added = Vec::new();
    let mut model_removed = Vec::new();
    let mut model_changed_entries = Vec::new();
    for (sel, val) in &sel_added {
        if sel.to_ascii_lowercase().contains("model") {
            model_added.push((sel.clone(), val.clone()));
        }
    }
    for (sel, val) in &sel_removed {
        if sel.to_ascii_lowercase().contains("model") {
            model_removed.push((sel.clone(), val.clone()));
        }
    }
    for (sel, old_v, new_v) in &sel_changed {
        if sel.to_ascii_lowercase().contains("model") {
            model_changed_entries.push((sel.clone(), old_v.clone(), new_v.clone()));
        }
    }
    // Also consider provider-level model default change if a single model selector changed
    // value, expose as default_changed for convenience.
    let default_changed = if model_changed_entries.len() == 1 {
        model_changed_entries
            .first()
            .map(|(_, old, new)| (old.clone(), new.clone()))
    } else {
        None
    };
    let model_changes = ModelChanges {
        added: model_added,
        removed: model_removed,
        changed: model_changed_entries,
        default_changed,
    };

    // --- inputs ---
    let old_inputs: BTreeSet<&String> = old.inputs.iter().map(|i| &i.key).collect();
    let new_inputs: BTreeSet<&String> = new.inputs.iter().map(|i| &i.key).collect();
    let mut inputs_added = Vec::new();
    let mut inputs_removed = Vec::new();
    for inp in &old.inputs {
        if !new_inputs.contains(&inp.key) {
            inputs_added.push(inp.key.clone());
            // Actually this is removed; fix logic below
        }
    }
    // Correct the above: we pushed to wrong vec; recompute cleanly.
    inputs_added.clear();
    inputs_removed.clear();
    for inp in &old.inputs {
        if !new_inputs.contains(&inp.key) {
            inputs_removed.push(inp.key.clone());
        }
    }
    for inp in &new.inputs {
        if !old_inputs.contains(&inp.key) {
            inputs_added.push(inp.key.clone());
        }
    }
    let input_changes = InputChanges {
        added: inputs_added,
        removed: inputs_removed,
    };

    // --- migration notes ---
    let mut migration_notes_added = Vec::new();
    for note in &new.migration_notes {
        if !old.migration_notes.contains(note) {
            migration_notes_added.push(redact_string(note));
        }
    }

    let has_changes = harness_changed.is_some()
        || provider_diff.has_change
        || harness_version_req_changed.is_some()
        || status_changed.is_some()
        || label_changed.is_some()
        || !capability_changes.added.is_empty()
        || !capability_changes.removed.is_empty()
        || !capability_changes.changed.is_empty()
        || !wrapper_env_changes.added.is_empty()
        || !wrapper_env_changes.removed.is_empty()
        || !wrapper_env_changes.changed.is_empty()
        || !wrapper_args_changes.added.is_empty()
        || !wrapper_args_changes.removed.is_empty()
        || !asset_changes.added.is_empty()
        || !asset_changes.removed.is_empty()
        || !selector_changes.added.is_empty()
        || !selector_changes.removed.is_empty()
        || !selector_changes.changed.is_empty()
        || !input_changes.added.is_empty()
        || !input_changes.removed.is_empty()
        || !migration_notes_added.is_empty();

    TemplateDiff {
        template_id,
        from_version,
        to_version,
        harness_changed,
        provider_diff,
        harness_version_req_changed,
        status_changed,
        label_changed,
        model_changes,
        capability_changes,
        wrapper_env_changes,
        wrapper_args_changes,
        asset_changes,
        selector_changes,
        input_changes,
        migration_notes_added,
        has_changes,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(redundant_imports, reason = "test imports overlap via super")]
mod tests {
    use super::*;
    use crate::ids::{HarnessId, ProviderId, TemplateId, TemplateVersion};
    use crate::instance::Instance;
    use serde_json::json;

    fn minimal_catalog() -> Catalog {
        Catalog {
            version: CATALOG_SCHEMA_VERSION,
            templates: vec![TemplateCatalogEntry {
                id: TemplateId::new("claude-glm").unwrap(),
                latest_version: TemplateVersion::new("1.2.0").unwrap(),
                files: vec![
                    TemplateFileRef {
                        version: TemplateVersion::new("1.1.0").unwrap(),
                        path: "claude-glm/1.1.0.json".to_owned(),
                        digest: "a".repeat(64),
                    },
                    TemplateFileRef {
                        version: TemplateVersion::new("1.2.0").unwrap(),
                        path: "claude-glm/1.2.0.json".to_owned(),
                        digest: "b".repeat(64),
                    },
                ],
                status: TemplateStatus::Active,
            }],
        }
    }

    fn minimal_template() -> Template {
        Template {
            schema_version: TEMPLATE_SCHEMA_VERSION,
            id: TemplateId::new("claude-glm").unwrap(),
            version: "1.2.0".to_owned(),
            harness: HarnessId::new("claude-code").unwrap(),
            provider: ProviderId::new("glm").unwrap(),
            label: "Claude Code on GLM".to_owned(),
            status: TemplateStatus::Active,
            inputs: vec![TemplateInput {
                key: "model".to_owned(),
                description: "Model to use".to_owned(),
                required: true,
            }],
            patches: vec![OwnedPatch {
                selector: "key:model".to_owned(),
                value: json!("sonnet"),
            }],
            wrapper_env: BTreeMap::new(),
            wrapper_args: Vec::new(),
            assets: Vec::new(),
            capability_map: BTreeMap::new(),
            migration_notes: Vec::new(),
            digest: "c".repeat(64),
            harness_version_req: None,
            provider_protocol: None,
        }
    }

    #[test]
    fn catalog_round_trip() {
        let catalog = minimal_catalog();
        catalog.validate().unwrap();
        let bytes = catalog.to_json_bytes().unwrap();
        let back = Catalog::from_json_bytes(&bytes).unwrap();
        assert_eq!(catalog, back);
    }

    #[test]
    fn catalog_rejects_traversal_path() {
        let mut catalog = minimal_catalog();
        catalog.templates[0].files[0].path = "../evil.json".to_owned();
        let err = catalog.validate().unwrap_err();
        assert!(format!("{err}").contains(".."));
    }

    #[test]
    fn catalog_rejects_duplicate_id() {
        let mut catalog = minimal_catalog();
        let dup = catalog.templates[0].clone();
        catalog.templates.push(dup);
        catalog.validate().unwrap_err();
    }

    #[test]
    fn catalog_rejects_missing_latest() {
        let mut catalog = minimal_catalog();
        catalog.templates[0].latest_version = TemplateVersion::new("9.9.9").unwrap();
        catalog.validate().unwrap_err();
    }

    #[test]
    fn catalog_rejects_not_max_latest() {
        let mut catalog = minimal_catalog();
        // latest should be 1.2.0 (max), set to 1.1.0 should fail
        catalog.templates[0].latest_version = TemplateVersion::new("1.1.0").unwrap();
        catalog.validate().unwrap_err();
    }

    #[test]
    fn catalog_rejects_absolute_path() {
        let mut catalog = minimal_catalog();
        catalog.templates[0].files[0].path = "/etc/passwd".to_owned();
        catalog.validate().unwrap_err();
    }

    #[test]
    fn validate_template_path_rejects_traversal() {
        validate_template_path("a/../b").unwrap_err();
        validate_template_path("../evil").unwrap_err();
        validate_template_path("/absolute").unwrap_err();
        validate_template_path("a//b").unwrap_err();
        validate_template_path("a\\b").unwrap_err();
        validate_template_path("a:b").unwrap_err();
        validate_template_path("").unwrap_err();
    }

    #[test]
    fn validate_template_path_accepts_valid() {
        validate_template_path("claude-glm/1.2.0.json").unwrap();
        validate_template_path("a/b/c.json").unwrap();
        validate_template_path("file.json").unwrap();
    }

    #[test]
    fn template_validates_minimal() {
        let tmpl = minimal_template();
        tmpl.validate().unwrap();
    }

    #[test]
    fn template_rejects_invalid_semver() {
        let mut tmpl = minimal_template();
        tmpl.version = "not-semver".to_owned();
        tmpl.validate().unwrap_err();
    }

    #[test]
    fn template_rejects_invalid_selector() {
        let mut tmpl = minimal_template();
        tmpl.patches[0].selector = "identity:missing_equals".to_owned();
        tmpl.validate().unwrap_err();
    }

    #[test]
    fn template_rejects_secret_value() {
        let mut tmpl = minimal_template();
        tmpl.patches[0].value = json!("my_api_key=sk-123");
        tmpl.validate().unwrap_err();
    }

    #[test]
    fn template_rejects_shell_value() {
        let mut tmpl = minimal_template();
        tmpl.patches[0].value = json!("`rm -rf /`");
        tmpl.validate().unwrap_err();
        let mut tmpl2 = minimal_template();
        tmpl2.patches[0].value = json!("$(rm -rf /)");
        tmpl2.validate().unwrap_err();
        let mut tmpl3 = minimal_template();
        tmpl3.patches[0].value = json!("a && b");
        tmpl3.validate().unwrap_err();
        let mut tmpl4 = minimal_template();
        tmpl4.patches[0].value = json!("sh -c 'evil'");
        tmpl4.validate().unwrap_err();
    }

    #[test]
    fn template_rejects_binary_blob() {
        let mut tmpl = minimal_template();
        let blob = "A".repeat(2048);
        tmpl.patches[0].value = json!(blob);
        tmpl.validate().unwrap_err();
    }

    #[test]
    fn semver_comparison() {
        assert!(is_newer_version("1.0.0", "1.0.1").unwrap());
        assert!(!is_newer_version("1.0.1", "1.0.0").unwrap());
        assert!(!is_newer_version("1.0.0", "1.0.0").unwrap());
        assert!(is_newer_version("1.0.0-alpha", "1.0.0").unwrap());
        assert_eq!(
            compare_semver("1.0.0", "2.0.0").unwrap(),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_semver("2.0.0", "1.0.0").unwrap(),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_semver("1.0.0", "1.0.0").unwrap(),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn semver_rejects_invalid() {
        parse_semver("not-a-version").unwrap_err();
        compare_semver("1.0.0", "bad").unwrap_err();
        is_newer_version("bad", "1.0.0").unwrap_err();
    }

    #[test]
    fn digest_verification_success_and_failure() {
        let bytes = b"hello world";
        let digest = compute_digest(bytes);
        assert_eq!(digest.len(), 64);
        verify_digest(bytes, &digest).unwrap();
        let bad = "0".repeat(64);
        verify_digest(bytes, &bad).unwrap_err();
        verify_digest(bytes, "short").unwrap_err();
        verify_digest(bytes, &"Z".repeat(64)).unwrap_err();
    }

    #[test]
    fn digest_is_case_insensitive() {
        let bytes = b"test digest";
        let lower = compute_digest(bytes);
        let upper = lower.to_ascii_uppercase();
        verify_digest(bytes, &upper).unwrap();
    }

    #[test]
    fn template_digest_must_be_lowercase() {
        let mut tmpl = minimal_template();
        tmpl.digest = "A".repeat(64);
        tmpl.validate().unwrap_err();
    }

    #[test]
    fn repo_config_example_and_validation() {
        let cfg = TemplateRepoConfig::example();
        assert_eq!(cfg.owner, "freeoxide");
        assert_eq!(cfg.repo, "superai-templates");
        cfg.validate().unwrap();
        let url = cfg.catalog_url().unwrap();
        assert!(url.starts_with("https://"));
        assert!(url.ends_with("/catalog.json"));
    }

    #[test]
    fn repo_config_rejects_traversal_fields() {
        TemplateRepoConfig::new("evil/host", "owner", "repo", "main").unwrap_err();
        TemplateRepoConfig::new("host", "owner/repo", "repo", "main").unwrap_err();
        TemplateRepoConfig::new("host", "owner", "repo", "../main").unwrap_err();
        TemplateRepoConfig::new("", "owner", "repo", "main").unwrap_err();
    }

    #[test]
    fn repo_config_base_url_file_scheme() {
        let mut cfg = TemplateRepoConfig::example();
        cfg.base_url = Some("file:///tmp/templates".to_owned());
        let url = cfg.catalog_url().unwrap();
        assert_eq!(url, "file:///tmp/templates/catalog.json");
        let turl = cfg.template_url("claude-glm/1.2.0.json").unwrap();
        assert_eq!(turl, "file:///tmp/templates/claude-glm/1.2.0.json");
    }

    #[test]
    fn repo_config_template_url_rejects_traversal() {
        let cfg = TemplateRepoConfig::example();
        cfg.template_url("../evil.json").unwrap_err();
        cfg.template_url("/etc/passwd").unwrap_err();
        cfg.template_url("a\\b").unwrap_err();
    }

    #[test]
    fn repo_config_template_url_https_only() {
        let mut cfg = TemplateRepoConfig::example();
        cfg.base_url = Some("http://insecure.example.com/templates".to_owned());
        cfg.template_url("a.json").unwrap_err();
    }

    #[test]
    fn template_validate_against_adapter_success() {
        let tmpl = minimal_template();
        let adapter = crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap();
        // minimal template uses selector key:model which is owned by Claude adapter
        tmpl.validate_against_adapter(&adapter).unwrap();
    }

    #[test]
    fn template_validate_against_adapter_rejects_unowned() {
        let mut tmpl = minimal_template();
        tmpl.patches[0].selector = "key:foreign_unowned_field_xyz".to_owned();
        tmpl.validate().unwrap();
        let adapter = crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap();
        tmpl.validate_against_adapter(&adapter).unwrap_err();
    }

    #[test]
    fn template_from_json_bytes_validates_size_and_schema() {
        let tmpl = minimal_template();
        let bytes = serde_json::to_vec(&tmpl).unwrap();
        let parsed = Template::from_json_bytes(&bytes).unwrap();
        assert_eq!(parsed.id, tmpl.id);
        let bad_json = b"{ invalid json }";
        Template::from_json_bytes(bad_json).unwrap_err();
        let huge = vec![b'a'; MAX_TEMPLATE_BYTES + 1];
        Template::from_json_bytes(&huge).unwrap_err();
    }

    #[test]
    fn template_wrapper_env_secret_rejected() {
        let mut tmpl = minimal_template();
        tmpl.wrapper_env
            .insert("API_KEY".to_owned(), "sk-123".to_owned());
        // The key itself is not checked for secret, but value is.
        // Actually wrapper_env values are checked via check_value_forbidden which
        // will reject secret pattern.
        tmpl.validate().unwrap_err();
    }

    #[test]
    fn catalog_from_json_bytes_validates() {
        let catalog = minimal_catalog();
        let bytes = serde_json::to_vec(&catalog).unwrap();
        Catalog::from_json_bytes(&bytes).unwrap();
        let bad = b"not json";
        Catalog::from_json_bytes(bad).unwrap_err();
        let huge = vec![b'a'; MAX_TEMPLATE_BYTES + 1];
        Catalog::from_json_bytes(&huge).unwrap_err();
    }

    #[test]
    fn template_harness_version_req_validates() {
        let mut tmpl = minimal_template();
        tmpl.harness_version_req = Some(">=1.0.0, <2.0.0".to_owned());
        tmpl.validate().unwrap();
        tmpl.harness_version_req = Some("not a req".to_owned());
        tmpl.validate().unwrap_err();
    }

    #[test]
    fn file_ref_rejects_uppercase_digest() {
        let mut catalog = minimal_catalog();
        catalog.templates[0].files[0].digest = "A".repeat(64);
        catalog.validate().unwrap_err();
    }

    // -----------------------------------------------------------------------
    // TPL-04 tests: version discovery
    // -----------------------------------------------------------------------

    fn sample_instance_with_template(
        harness: &str,
        template_name: &str,
        version: &str,
        adapter_revision: &str,
    ) -> Instance {
        use crate::ids::{InstanceId, InstanceName};
        use crate::instance::{Instance, TemplateRef};
        use crate::paths::AbsolutePath;
        use crate::state::{InstanceOrigin, Isolation, Ownership};
        Instance {
            id: InstanceId::new("test-instance-001").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(harness).unwrap(),
            config_root: AbsolutePath::new("/tmp/.claude-work").unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: TemplateId::new(template_name).unwrap(),
                version: TemplateVersion::new(version).unwrap(),
            }),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: adapter_revision.to_owned(),
        }
    }

    #[test]
    fn check_update_up_to_date() {
        let catalog = minimal_catalog();
        let instance = sample_instance_with_template("claude-code", "claude-glm", "1.2.0", "0.1.0");
        let status = check_update_with_catalog(&instance, &catalog);
        assert_eq!(status, UpdateStatus::UpToDate);
    }

    #[test]
    fn check_update_update_available() {
        let catalog = minimal_catalog();
        let instance = sample_instance_with_template("claude-code", "claude-glm", "1.1.0", "0.1.0");
        let status = check_update_with_catalog(&instance, &catalog);
        match status {
            UpdateStatus::UpdateAvailable { latest } => assert_eq!(latest.as_str(), "1.2.0"),
            other => panic!("expected UpdateAvailable, got {other:?}"),
        }
    }

    #[test]
    fn check_update_semver_ordering() {
        // 1.0.0 < 1.0.1 < 1.10.0, and 1.0.0-alpha < 1.0.0
        let mut catalog = minimal_catalog();
        catalog.templates[0].latest_version = TemplateVersion::new("1.10.0").unwrap();
        catalog.templates[0].files.push(TemplateFileRef {
            version: TemplateVersion::new("1.10.0").unwrap(),
            path: "claude-glm/1.10.0.json".to_owned(),
            digest: "c".repeat(64),
        });
        let instance = sample_instance_with_template("claude-code", "claude-glm", "1.2.0", "0.1.0");
        let status = check_update_with_catalog(&instance, &catalog);
        match status {
            UpdateStatus::UpdateAvailable { latest } => assert_eq!(latest.as_str(), "1.10.0"),
            other => panic!("expected UpdateAvailable for semver ordering, got {other:?}"),
        }
        // Pre-release ordering: 1.0.0-alpha is less than 1.0.0
        assert!(is_newer_version("1.0.0-alpha", "1.0.0").unwrap());
        assert!(!is_newer_version("1.0.0", "1.0.0-alpha").unwrap());
        assert!(is_newer_version("1.0.0", "1.0.1").unwrap());
        assert!(!is_newer_version("1.2.0", "1.2.0").unwrap());
    }

    #[test]
    fn check_update_yanked_handling() {
        let mut catalog = minimal_catalog();
        catalog.templates[0].status = TemplateStatus::Yanked;
        let instance = sample_instance_with_template("claude-code", "claude-glm", "1.2.0", "0.1.0");
        let status = check_update_with_catalog(&instance, &catalog);
        assert_eq!(status, UpdateStatus::Yanked);
        let instance_old =
            sample_instance_with_template("claude-code", "claude-glm", "1.1.0", "0.1.0");
        let status2 = check_update_with_catalog(&instance_old, &catalog);
        assert_eq!(status2, UpdateStatus::Yanked);
    }

    #[test]
    fn check_update_current_missing() {
        let catalog = minimal_catalog();
        let instance_missing_template =
            sample_instance_with_template("claude-code", "claude-glm", "9.9.9", "0.1.0");
        let status = check_update_with_catalog(&instance_missing_template, &catalog);
        assert_eq!(status, UpdateStatus::CurrentMissing);
        let instance_unknown_id =
            sample_instance_with_template("claude-code", "unknown-template", "1.0.0", "0.1.0");
        let status2 = check_update_with_catalog(&instance_unknown_id, &catalog);
        assert_eq!(status2, UpdateStatus::CurrentMissing);
        // No template at all
        let mut instance_no_template =
            sample_instance_with_template("claude-code", "claude-glm", "1.2.0", "0.1.0");
        instance_no_template.template = None;
        let status3 = check_update_with_catalog(&instance_no_template, &catalog);
        assert_eq!(status3, UpdateStatus::CurrentMissing);
    }

    #[test]
    fn check_update_offline_via_repo() {
        // Use a file:// repo that points to a non-existent directory to trigger Offline
        let catalog = minimal_catalog();
        let instance = sample_instance_with_template("claude-code", "claude-glm", "1.1.0", "0.1.0");
        let mut repo = TemplateRepoConfig::example();
        repo.base_url = Some("file:///tmp/superai-offline-missing-xyz-12345".to_owned());
        let status = check_update(&instance, &catalog, &repo);
        assert_eq!(status, UpdateStatus::Offline);
    }

    #[test]
    fn check_update_incompatible_via_network_fetch() {
        // Create a temp repo with catalog and two templates where latest has incompatible harness_version_req
        let dir = crate::test_util::temp_dir_unique("tpl");
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("claude-glm")).unwrap();

        let instance = sample_instance_with_template("claude-code", "claude-glm", "1.1.0", "0.1.0");

        // Create old template file (1.1.0) with no harness_version_req
        let old_bytes = {
            let mut tmpl = minimal_template();
            tmpl.version = "1.1.0".to_owned();
            tmpl.harness_version_req = None;
            // Set digest to valid lowercase hex placeholder
            tmpl.digest = "d".repeat(64);
            // Write actual bytes and compute digest for catalog? We use placeholder digest but we need catalog digest to match file hash
            // Instead, compute digest from bytes after serializing with placeholder digest, then update template digest to match?
            // Simpler: create template with digest = compute_digest(bytes_without_digest_check)
            // But Template::from_json_bytes will validate digest format only, not that it matches file hash via verify_digest separately.
            // The fetch_template verifies digest of file bytes against catalog digest, not template.digest field.
            // So we can keep template.digest as "d".repeat(64) and catalog digest as compute_digest(raw)
            serde_json::to_vec(&tmpl).unwrap()
        };
        let old_catalog_digest = compute_digest(&old_bytes);
        std::fs::write(dir.join("claude-glm/1.1.0.json"), &old_bytes).unwrap();

        // Create new template file (1.2.0) with harness_version_req that is incompatible with instance adapter_revision 0.1.0
        let new_bytes = {
            let mut tmpl = minimal_template();
            tmpl.version = "1.2.0".to_owned();
            tmpl.harness_version_req = Some(">=9.0.0".to_owned());
            tmpl.digest = "e".repeat(64);
            serde_json::to_vec(&tmpl).unwrap()
        };
        let new_catalog_digest = compute_digest(&new_bytes);
        std::fs::write(dir.join("claude-glm/1.2.0.json"), &new_bytes).unwrap();

        // Catalog with both versions, latest 1.2.0
        let catalog = Catalog {
            version: CATALOG_SCHEMA_VERSION,
            templates: vec![TemplateCatalogEntry {
                id: TemplateId::new("claude-glm").unwrap(),
                latest_version: TemplateVersion::new("1.2.0").unwrap(),
                files: vec![
                    TemplateFileRef {
                        version: TemplateVersion::new("1.1.0").unwrap(),
                        path: "claude-glm/1.1.0.json".to_owned(),
                        digest: old_catalog_digest,
                    },
                    TemplateFileRef {
                        version: TemplateVersion::new("1.2.0").unwrap(),
                        path: "claude-glm/1.2.0.json".to_owned(),
                        digest: new_catalog_digest,
                    },
                ],
                status: TemplateStatus::Active,
            }],
        };
        let catalog_bytes = serde_json::to_vec(&catalog).unwrap();
        std::fs::write(dir.join("catalog.json"), &catalog_bytes).unwrap();

        let repo = TemplateRepoConfig {
            host: "example.com".to_owned(),
            owner: "owner".to_owned(),
            repo: "repo".to_owned(),
            git_ref: "main".to_owned(),
            base_url: Some(format!("file://{}", dir.display())),
        };

        // check_update should detect incompatible due to harness_version_req
        let status = check_update(&instance, &catalog, &repo);
        match status {
            UpdateStatus::Incompatible { reason } => assert!(
                reason.contains("9.0.0") || reason.contains("harness version"),
                "unexpected reason: {reason}"
            ),
            other => panic!("expected Incompatible, got {other:?}"),
        }

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn check_update_via_file_repo_update_available() {
        // Similar setup but compatible version should be UpdateAvailable
        let dir = crate::test_util::temp_dir_unique("tpl");
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(dir.join("claude-glm")).unwrap();

        let instance = sample_instance_with_template("claude-code", "claude-glm", "1.1.0", "1.5.0");

        let old_bytes = {
            let mut tmpl = minimal_template();
            tmpl.version = "1.1.0".to_owned();
            tmpl.harness_version_req = Some(">=1.0.0".to_owned());
            tmpl.digest = "f".repeat(64);
            serde_json::to_vec(&tmpl).unwrap()
        };
        let old_digest = compute_digest(&old_bytes);
        std::fs::write(dir.join("claude-glm/1.1.0.json"), &old_bytes).unwrap();
        let new_bytes = {
            let mut tmpl = minimal_template();
            tmpl.version = "1.2.0".to_owned();
            tmpl.harness_version_req = Some(">=1.0.0".to_owned());
            tmpl.digest = "a".repeat(64);
            serde_json::to_vec(&tmpl).unwrap()
        };
        let new_digest = compute_digest(&new_bytes);
        std::fs::write(dir.join("claude-glm/1.2.0.json"), &new_bytes).unwrap();

        let catalog = Catalog {
            version: CATALOG_SCHEMA_VERSION,
            templates: vec![TemplateCatalogEntry {
                id: TemplateId::new("claude-glm").unwrap(),
                latest_version: TemplateVersion::new("1.2.0").unwrap(),
                files: vec![
                    TemplateFileRef {
                        version: TemplateVersion::new("1.1.0").unwrap(),
                        path: "claude-glm/1.1.0.json".to_owned(),
                        digest: old_digest,
                    },
                    TemplateFileRef {
                        version: TemplateVersion::new("1.2.0").unwrap(),
                        path: "claude-glm/1.2.0.json".to_owned(),
                        digest: new_digest,
                    },
                ],
                status: TemplateStatus::Active,
            }],
        };
        std::fs::write(
            dir.join("catalog.json"),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();

        let repo = TemplateRepoConfig {
            host: "example.com".to_owned(),
            owner: "owner".to_owned(),
            repo: "repo".to_owned(),
            git_ref: "main".to_owned(),
            base_url: Some(format!("file://{}", dir.display())),
        };

        let status = check_update(&instance, &catalog, &repo);
        match status {
            UpdateStatus::UpdateAvailable { latest } => assert_eq!(latest.as_str(), "1.2.0"),
            other => panic!("expected UpdateAvailable, got {other:?}"),
        }

        drop(std::fs::remove_dir_all(&dir));
    }

    // -----------------------------------------------------------------------
    // TPL-05 tests: diff
    // -----------------------------------------------------------------------

    #[test]
    #[expect(clippy::too_many_lines, reason = "diff test covers many categories")]
    fn diff_contains_expected_fields() {
        let mut old = minimal_template();
        old.version = "1.1.0".to_owned();
        old.provider = ProviderId::new("glm").unwrap();
        old.patches = vec![
            OwnedPatch {
                selector: "key:model".to_owned(),
                value: json!("glm-4"),
            },
            OwnedPatch {
                selector: "key:temperature".to_owned(),
                value: json!(0.7),
            },
        ];
        old.capability_map
            .insert("web_search".to_owned(), "native".to_owned());
        old.wrapper_env.insert("FOO".to_owned(), "bar".to_owned());
        old.wrapper_args = vec!["--foo".to_owned()];
        old.assets = vec!["asset/a.json".to_owned()];
        old.digest = "a".repeat(64);

        let mut new = old.clone();
        new.version = "1.2.0".to_owned();
        new.provider = ProviderId::new("minimax").unwrap();
        new.provider_protocol = Some("openai".to_owned());
        new.patches = vec![
            OwnedPatch {
                selector: "key:model".to_owned(),
                value: json!("glm-4.5"),
            },
            OwnedPatch {
                selector: "key:max_tokens".to_owned(),
                value: json!(8192),
            },
        ];
        new.capability_map.clear();
        new.capability_map
            .insert("web_search".to_owned(), "substituted".to_owned());
        new.capability_map
            .insert("vision".to_owned(), "native".to_owned());
        new.wrapper_env.clear();
        new.wrapper_env.insert("FOO".to_owned(), "baz".to_owned());
        new.wrapper_env.insert("BAR".to_owned(), "qux".to_owned());
        new.wrapper_args = vec!["--foo".to_owned(), "--bar".to_owned()];
        new.assets = vec!["asset/a.json".to_owned(), "asset/b.json".to_owned()];
        new.digest = "b".repeat(64);

        let diff = diff_templates(&old, &new);
        assert!(diff.has_changes);
        assert!(!diff.is_empty());
        assert_eq!(diff.from_version, "1.1.0");
        assert_eq!(diff.to_version, "1.2.0");
        // provider changes
        assert!(diff.provider_diff.has_change);
        assert_eq!(
            diff.provider_diff.provider_changed,
            Some(("glm".to_owned(), "minimax".to_owned()))
        );
        // provider protocol changed
        assert_eq!(
            diff.provider_diff.protocol_changed,
            Some((None, Some("openai".to_owned())))
        );
        // model changes: old model glm-4 -> glm-4.5 and added/removed
        assert!(!diff.model_changes.changed.is_empty());
        assert!(
            diff.model_changes
                .changed
                .iter()
                .any(|(sel, old_v, new_v)| sel == "key:model"
                    && old_v == &json!("glm-4")
                    && new_v == &json!("glm-4.5"))
        );
        // model default_changed should be set
        assert!(diff.model_changes.default_changed.is_some());
        // capability changes: web_search native -> substituted, vision added
        assert!(
            diff.capability_changes
                .changed
                .iter()
                .any(|(k, old, new)| k == "web_search" && old == "native" && new == "substituted")
        );
        assert!(
            diff.capability_changes
                .added
                .iter()
                .any(|(k, v)| k == "vision" && v == "native")
        );
        // wrapper env changes: FOO bar->baz, BAR added
        assert!(
            diff.wrapper_env_changes
                .changed
                .iter()
                .any(|(k, old, new)| k == "FOO" && old == "bar" && new == "baz")
        );
        assert!(
            diff.wrapper_env_changes
                .added
                .iter()
                .any(|(k, v)| k == "BAR" && v == "qux")
        );
        // wrapper args: BAR added? Actually --bar added
        assert!(
            diff.wrapper_args_changes
                .added
                .contains(&"--bar".to_owned())
        );
        // asset changes: b added
        assert!(
            diff.asset_changes
                .added
                .contains(&"asset/b.json".to_owned())
        );
        // selector changes: temperature removed, max_tokens added, model changed
        assert!(
            diff.selector_changes
                .removed
                .iter()
                .any(|(sel, _)| sel == "key:temperature")
        );
        assert!(
            diff.selector_changes
                .added
                .iter()
                .any(|(sel, _)| sel == "key:max_tokens")
        );
        assert!(
            diff.selector_changes
                .changed
                .iter()
                .any(|(sel, _, _)| sel == "key:model")
        );
    }

    #[test]
    fn diff_no_changes_is_empty() {
        let tmpl = minimal_template();
        let diff = diff_templates(&tmpl, &tmpl);
        assert!(!diff.has_changes);
        assert!(diff.is_empty());
        assert_eq!(diff.from_version, diff.to_version);
    }

    #[test]
    fn diff_redacts_secret_placeholders_consistently() {
        // Create two templates that differ in a value that looks like a secret placeholder
        // Use a value that contains a secret pattern but is still allowed via direct construction
        // (bypassing validate). diff should redact it to [REDACTED].
        let mut old = minimal_template();
        old.patches = vec![OwnedPatch {
            selector: "key:api_url".to_owned(),
            value: json!("https://api.example.com"),
        }];
        let mut new = old.clone();
        new.patches = vec![OwnedPatch {
            selector: "key:api_url".to_owned(),
            value: json!("https://api.example.com/v2"),
        }];
        // Add a wrapper env that contains a secret-like placeholder
        old.wrapper_env
            .insert("MY_TOKEN".to_owned(), "secret-value-sk-123".to_owned());
        new.wrapper_env
            .insert("MY_TOKEN".to_owned(), "secret-value-sk-456".to_owned());
        // Add capability with secret-like? Not needed.
        let diff = diff_templates(&old, &new);
        // Check that secret values are redacted in diff
        let redacted = RedactedString::placeholder();
        // wrapper env changed entry should be redacted
        for (key, old_v, new_v) in &diff.wrapper_env_changes.changed {
            if key == "MY_TOKEN" {
                assert_eq!(old_v, redacted);
                assert_eq!(new_v, redacted);
            }
        }
        // Also ensure no raw secret appears in debug output of diff
        let debug = format!("{diff:?}");
        assert!(
            !debug.contains("sk-123") && !debug.contains("sk-456"),
            "diff debug must not leak secret: {debug}"
        );
        // Also check that selector values containing secret pattern are redacted
        // Create a diff where patch value is secret-like
        let mut old2 = minimal_template();
        old2.patches = vec![OwnedPatch {
            selector: "key:model".to_owned(),
            value: json!("my_api_key_sk-abc"),
        }];
        let mut new2 = old2.clone();
        new2.patches = vec![OwnedPatch {
            selector: "key:model".to_owned(),
            value: json!("my_api_key_sk-def"),
        }];
        let diff2 = diff_templates(&old2, &new2);
        for (sel, old_v, new_v) in &diff2.selector_changes.changed {
            if sel == "key:model" {
                assert_eq!(old_v, &json!(redacted));
                assert_eq!(new_v, &json!(redacted));
            }
        }
        let debug2 = format!("{diff2:?}");
        assert!(!debug2.contains("sk-abc") && !debug2.contains("sk-def"));
    }

    #[test]
    fn diff_harness_and_status_and_label() {
        let mut old = minimal_template();
        old.harness = HarnessId::new("claude-code").unwrap();
        old.status = TemplateStatus::Active;
        old.label = "Claude Code on GLM".to_owned();
        old.harness_version_req = Some(">=1.0.0".to_owned());
        let mut new = old.clone();
        new.harness = HarnessId::new("opencode").unwrap();
        new.status = TemplateStatus::Deprecated;
        new.label = "Opencode on GLM".to_owned();
        new.harness_version_req = Some(">=2.0.0".to_owned());
        let diff = diff_templates(&old, &new);
        assert_eq!(
            diff.harness_changed,
            Some(("claude-code".to_owned(), "opencode".to_owned()))
        );
        assert_eq!(
            diff.status_changed,
            Some((TemplateStatus::Active, TemplateStatus::Deprecated))
        );
        assert!(diff.label_changed.is_some());
        assert_eq!(
            diff.harness_version_req_changed,
            Some((Some(">=1.0.0".to_owned()), Some(">=2.0.0".to_owned())))
        );
    }
}
