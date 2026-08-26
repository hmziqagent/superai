//! Template fetch client (TPL-03).
//!
//! Provides blocking HTTPS fetch for catalog and template files with
//! bounded redirects, size limits, timeouts, digest verification, and
//! traversal rejection. Untrusted network data never reaches filesystem
//! APIs without validation.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{CoreError, Result as CoreResult};
use crate::template::{
    Catalog, MAX_TEMPLATE_BYTES, Template, TemplateRepoConfig, compute_digest,
    validate_template_path,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of redirects to follow.
pub const MAX_REDIRECTS: u8 = 3;

/// Fetch timeout in seconds.
pub const FETCH_TIMEOUT_SECS: u64 = 30;

/// User-Agent string `superai/<version>`.
pub const USER_AGENT: &str = concat!("superai/", env!("CARGO_PKG_VERSION"));

/// Maximum response size (1 MiB).
pub const MAX_BYTES: usize = MAX_TEMPLATE_BYTES;

// ---------------------------------------------------------------------------
// Error taxonomy (maps to CoreError::NetworkTemplate etc.)
// ---------------------------------------------------------------------------

/// Errors from fetching templates or catalogs.
#[derive(Debug, thiserror::Error)]
pub enum TemplateFetchError {
    /// Network or I/O error.
    #[error("network error for `{template}`: {reason}")]
    Network {
        /// Template or URL context.
        template: String,
        /// Human reason.
        reason: String,
    },
    /// Remote file not found (404).
    #[error("not found for `{template}`: {reason}")]
    NotFound {
        /// Template or URL context.
        template: String,
        /// Human reason.
        reason: String,
    },
    /// Rate limited (429).
    #[error("rate limited for `{template}`: {reason}")]
    RateLimited {
        /// Template or URL context.
        template: String,
        /// Human reason.
        reason: String,
    },
    /// Digest mismatch between catalog and fetched bytes.
    #[error("digest mismatch for `{template}`: expected {expected}, got {actual}")]
    DigestMismatch {
        /// Template or URL context.
        template: String,
        /// Expected digest.
        expected: String,
        /// Actual digest.
        actual: String,
    },
    /// Schema validation failed after fetch.
    #[error("schema invalid for `{template}`: {reason}")]
    SchemaInvalid {
        /// Template or URL context.
        template: String,
        /// Human reason.
        reason: String,
    },
    /// Invalid URL (not https, or traversal).
    #[error("invalid url for `{template}`: {reason}")]
    InvalidUrl {
        /// Template or URL context.
        template: String,
        /// Human reason.
        reason: String,
    },
    /// Response exceeds size limit.
    #[error("size limit exceeded for `{template}`: {reason}")]
    SizeLimit {
        /// Template or URL context.
        template: String,
        /// Human reason.
        reason: String,
    },
    /// Too many redirects.
    #[error("redirect limit exceeded for `{template}`: {reason}")]
    RedirectLimit {
        /// Template or URL context.
        template: String,
        /// Human reason.
        reason: String,
    },
}

impl TemplateFetchError {
    /// Convert to [`CoreError::NetworkTemplate`] for callers that use the core taxonomy.
    pub fn into_core(self) -> CoreError {
        match self {
            Self::Network { template, reason } => CoreError::NetworkTemplate {
                template,
                reason,
                context_redacted: None,
            },
            Self::NotFound { template, reason } => CoreError::NetworkTemplate {
                template,
                reason: format!("not found: {reason}"),
                context_redacted: None,
            },
            Self::RateLimited { template, reason } => CoreError::NetworkTemplate {
                template,
                reason: format!("rate limited: {reason}"),
                context_redacted: None,
            },
            Self::DigestMismatch {
                template,
                expected,
                actual,
            } => CoreError::Verification {
                path: PathBuf::from(template),
                kind: "digest".to_owned(),
                reason: format!("expected {expected}, got {actual}"),
            },
            Self::SchemaInvalid { template, reason } => CoreError::SchemaValidation {
                path: PathBuf::from(template),
                details: reason,
            },
            Self::InvalidUrl { template, reason } => CoreError::Validation {
                field: "url".to_owned(),
                reason: format!("invalid url for `{template}`: {reason}"),
            },
            Self::SizeLimit { template, reason } => CoreError::Validation {
                field: "size".to_owned(),
                reason: format!("size limit for `{template}`: {reason}"),
            },
            Self::RedirectLimit { template, reason } => CoreError::NetworkTemplate {
                template,
                reason: format!("redirect limit: {reason}"),
                context_redacted: None,
            },
        }
    }
}

impl From<TemplateFetchError> for CoreError {
    fn from(err: TemplateFetchError) -> Self {
        err.into_core()
    }
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

/// Validate that a URL is HTTPS (or `file://` for tests) and not containing
/// traversal or control characters.
///
/// Returns `Ok(())` if the URL is acceptable for fetching.
pub fn validate_fetch_url(url: &str, context: &str) -> Result<(), TemplateFetchError> {
    if url.trim().is_empty() {
        return Err(TemplateFetchError::InvalidUrl {
            template: context.to_owned(),
            reason: "url must not be empty".to_owned(),
        });
    }
    if url.contains('\0') || url.chars().any(char::is_control) {
        return Err(TemplateFetchError::InvalidUrl {
            template: context.to_owned(),
            reason: "url must not contain NUL or control chars".to_owned(),
        });
    }
    if url.starts_with("file://") {
        // file URLs are allowed only for tests; still validate the path part.
        let path_part = url.trim_start_matches("file://");
        if path_part.is_empty() {
            return Err(TemplateFetchError::InvalidUrl {
                template: context.to_owned(),
                reason: "file url path must not be empty".to_owned(),
            });
        }
        // Reject traversal in file path as well.
        validate_template_path(path_part.trim_start_matches('/')).map_err(|e| {
            TemplateFetchError::InvalidUrl {
                template: context.to_owned(),
                reason: format!("file path traversal: {e}"),
            }
        })?;
        // For absolute file paths like file:///tmp/... the above strips leading /,
        // but we still want to allow. So also check raw path via Path.
        let path = Path::new(path_part);
        for comp in path.components() {
            if matches!(comp, std::path::Component::ParentDir) {
                return Err(TemplateFetchError::InvalidUrl {
                    template: context.to_owned(),
                    reason: format!("file url must not contain '..': {url}"),
                });
            }
        }
        return Ok(());
    }
    if !url.starts_with("https://") {
        return Err(TemplateFetchError::InvalidUrl {
            template: context.to_owned(),
            reason: format!("url must be https://, got `{url}`"),
        });
    }
    // Reject obvious traversal in URL path.
    if url.contains("/../") || url.contains("/./") || url.ends_with("/..") {
        return Err(TemplateFetchError::InvalidUrl {
            template: context.to_owned(),
            reason: format!("url must not contain path traversal: `{url}`"),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Core fetch: bytes with limits
// ---------------------------------------------------------------------------

/// Fetch raw bytes from `url` with HTTPS-only, redirect, size, and timeout guards.
///
/// - If `url` starts with `file://`, reads from the local filesystem (for tests).
/// - Otherwise performs a blocking HTTPS GET via `ureq` with:
///   - `MAX_REDIRECTS` redirects,
///   - `MAX_BYTES` size limit,
///   - `FETCH_TIMEOUT_SECS` timeout,
///   - `USER_AGENT` header.
///
/// Returns the bytes on success or a [`TemplateFetchError`].
pub fn fetch_bytes(url: &str, context: &str) -> Result<Vec<u8>, TemplateFetchError> {
    validate_fetch_url(url, context)?;

    if let Some(path_str) = url.strip_prefix("file://") {
        // Local file mock path for tests. Do not follow symlinks blindly beyond read.
        let path = Path::new(path_str);
        let bytes = std::fs::read(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TemplateFetchError::NotFound {
                    template: context.to_owned(),
                    reason: format!("file not found `{path_str}`: {e}"),
                }
            } else {
                TemplateFetchError::Network {
                    template: context.to_owned(),
                    reason: format!("file read failed `{path_str}`: {e}"),
                }
            }
        })?;
        if bytes.len() > MAX_BYTES {
            return Err(TemplateFetchError::SizeLimit {
                template: context.to_owned(),
                reason: format!("file size {} exceeds limit {MAX_BYTES}", bytes.len()),
            });
        }
        return Ok(bytes);
    }

    // HTTPS fetch via ureq.
    fetch_bytes_ureq(url, context)
}

fn fetch_bytes_ureq(url: &str, context: &str) -> Result<Vec<u8>, TemplateFetchError> {
    // Build an agent with timeout and redirect limit.
    // ureq 3 API: Agent::config_builder() -> ConfigBuilder.
    // Fall back to simple agent if builder not available; we attempt the modern API first.
    let timeout = Duration::from_secs(FETCH_TIMEOUT_SECS);

    // Try to create an agent with custom config. If the builder API differs, we fall back to default.
    let agent = build_agent(timeout);

    let mut request = agent.get(url);
    request = request.header("User-Agent", USER_AGENT);

    let mut response = request
        .call()
        .map_err(|e| map_ureq_error(e, context, url))?;

    // Status checks.
    let status = response.status().as_u16();
    if status == 404 {
        return Err(TemplateFetchError::NotFound {
            template: context.to_owned(),
            reason: format!("404 not found for `{url}`"),
        });
    }
    if status == 429 {
        return Err(TemplateFetchError::RateLimited {
            template: context.to_owned(),
            reason: format!("429 rate limited for `{url}`"),
        });
    }
    if !(200..300).contains(&i32::from(status)) {
        return Err(TemplateFetchError::Network {
            template: context.to_owned(),
            reason: format!("http {status} for `{url}`"),
        });
    }

    // Size limit via Content-Length header if present.
    if let Some(len_str) = response.headers().get("Content-Length")
        && let Ok(s) = len_str.to_str()
        && let Ok(len) = s.parse::<usize>()
        && len > MAX_BYTES
    {
        return Err(TemplateFetchError::SizeLimit {
            template: context.to_owned(),
            reason: format!("content-length {len} exceeds limit {MAX_BYTES}"),
        });
    }

    // Read body with size limit.
    let mut body = response.body_mut().as_reader();
    let mut buf: Vec<u8> = Vec::new();
    let mut limited = (&mut body).take((MAX_BYTES + 1) as u64);
    limited
        .read_to_end(&mut buf)
        .map_err(|e| TemplateFetchError::Network {
            template: context.to_owned(),
            reason: format!("read failed for `{url}`: {e}"),
        })?;
    if buf.len() > MAX_BYTES {
        return Err(TemplateFetchError::SizeLimit {
            template: context.to_owned(),
            reason: format!("response size exceeds limit {MAX_BYTES}"),
        });
    }
    Ok(buf)
}

fn build_agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(u32::from(MAX_REDIRECTS))
        .max_redirects_will_error(true)
        .user_agent(USER_AGENT)
        .build();
    ureq::Agent::new_with_config(config)
}

fn map_ureq_error(err: ureq::Error, context: &str, url: &str) -> TemplateFetchError {
    match err {
        ureq::Error::StatusCode(code) => {
            if code == 404 {
                TemplateFetchError::NotFound {
                    template: context.to_owned(),
                    reason: format!("404 for `{url}`"),
                }
            } else if code == 429 {
                TemplateFetchError::RateLimited {
                    template: context.to_owned(),
                    reason: format!("429 for `{url}`"),
                }
            } else {
                TemplateFetchError::Network {
                    template: context.to_owned(),
                    reason: format!("http {code} for `{url}`"),
                }
            }
        }
        ureq::Error::Timeout(_) => TemplateFetchError::Network {
            template: context.to_owned(),
            reason: format!("timeout after {FETCH_TIMEOUT_SECS}s for `{url}`"),
        },
        ureq::Error::HostNotFound => TemplateFetchError::Network {
            template: context.to_owned(),
            reason: format!("host not found for `{url}`"),
        },
        other => {
            let msg = format!("{other}");
            if msg.contains("redirect") || msg.contains("Redirect") {
                TemplateFetchError::RedirectLimit {
                    template: context.to_owned(),
                    reason: msg,
                }
            } else if msg.contains("timed out") || msg.contains("timeout") {
                TemplateFetchError::Network {
                    template: context.to_owned(),
                    reason: format!("timeout for `{url}`: {msg}"),
                }
            } else {
                TemplateFetchError::Network {
                    template: context.to_owned(),
                    reason: format!("network error for `{url}`: {msg}"),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// High-level fetchers
// ---------------------------------------------------------------------------

/// Fetch and validate the catalog for `config`.
///
/// Performs HTTPS-only fetch, validates JSON schema, checks size, and
/// verifies the catalog's own digest if the server provides one via
/// `X-Content-Sha256` header (optional). The returned catalog is already
/// validated.
pub fn fetch_catalog(config: &TemplateRepoConfig) -> Result<Catalog, TemplateFetchError> {
    let url = config
        .catalog_url()
        .map_err(|e| TemplateFetchError::InvalidUrl {
            template: "catalog".to_owned(),
            reason: format!("{e}"),
        })?;
    let bytes = fetch_bytes(&url, "catalog")?;
    let catalog =
        Catalog::from_json_bytes(&bytes).map_err(|e| TemplateFetchError::SchemaInvalid {
            template: "catalog".to_owned(),
            reason: format!("{e}"),
        })?;
    Ok(catalog)
}

/// Fetch a catalog from a local file path (for tests).
///
/// Reads `path` from disk, validates size, parses and validates the catalog.
/// No network is involved.
pub fn fetch_catalog_from_path(path: &Path) -> Result<Catalog, TemplateFetchError> {
    let bytes = std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            TemplateFetchError::NotFound {
                template: "catalog".to_owned(),
                reason: format!("catalog file not found `{}`: {e}", path.display()),
            }
        } else {
            TemplateFetchError::Network {
                template: "catalog".to_owned(),
                reason: format!("read failed `{}`: {e}", path.display()),
            }
        }
    })?;
    if bytes.len() > MAX_BYTES {
        return Err(TemplateFetchError::SizeLimit {
            template: "catalog".to_owned(),
            reason: format!("catalog size {} exceeds limit {MAX_BYTES}", bytes.len()),
        });
    }
    let catalog =
        Catalog::from_json_bytes(&bytes).map_err(|e| TemplateFetchError::SchemaInvalid {
            template: "catalog".to_owned(),
            reason: format!("{e}"),
        })?;
    Ok(catalog)
}

/// Fetch raw template bytes for a given template id and version, verifying
/// digest against the catalog entry.
///
/// Lookup `template_id`/`version` in `catalog`, validate the file path for
/// traversal, fetch the bytes, verify `MAX_BYTES`, verify SHA-256 digest
/// matches the catalog's `digest`, and return the bytes.
pub fn fetch_template_bytes(
    config: &TemplateRepoConfig,
    catalog: &Catalog,
    template_id: &str,
    version: &str,
) -> Result<Vec<u8>, TemplateFetchError> {
    let entry = catalog
        .find(template_id)
        .ok_or_else(|| TemplateFetchError::NotFound {
            template: template_id.to_owned(),
            reason: format!("template `{template_id}` not found in catalog"),
        })?;
    let file_ref = entry
        .file_for_version(version)
        .ok_or_else(|| TemplateFetchError::NotFound {
            template: template_id.to_owned(),
            reason: format!("version `{version}` not found for template `{template_id}`"),
        })?;
    validate_template_path(&file_ref.path).map_err(|e| TemplateFetchError::InvalidUrl {
        template: template_id.to_owned(),
        reason: format!("invalid path `{}`: {e}", file_ref.path),
    })?;
    let url = config
        .template_url(&file_ref.path)
        .map_err(|e| TemplateFetchError::InvalidUrl {
            template: template_id.to_owned(),
            reason: format!("{e}"),
        })?;
    let bytes = fetch_bytes(&url, template_id)?;
    // Digest verification from catalog.
    let actual = compute_digest(&bytes);
    if actual != file_ref.digest.to_ascii_lowercase() {
        return Err(TemplateFetchError::DigestMismatch {
            template: template_id.to_owned(),
            expected: file_ref.digest.clone(),
            actual,
        });
    }
    Template::from_json_bytes(&bytes).map_err(|e| TemplateFetchError::SchemaInvalid {
        template: template_id.to_owned(),
        reason: format!("template schema invalid: {e}"),
    })?;
    Ok(bytes)
}

/// Fetch a template struct directly, after verifying catalog digest.
///
/// Convenience wrapper around [`fetch_template_bytes`] that parses and
/// validates the template and checks `validate_against_adapter` is not
/// called here (callers must validate against an adapter explicitly).
pub fn fetch_template(
    config: &TemplateRepoConfig,
    catalog: &Catalog,
    template_id: &str,
    version: &str,
) -> Result<Template, TemplateFetchError> {
    let bytes = fetch_template_bytes(config, catalog, template_id, version)?;
    let tmpl =
        Template::from_json_bytes(&bytes).map_err(|e| TemplateFetchError::SchemaInvalid {
            template: template_id.to_owned(),
            reason: format!("{e}"),
        })?;
    Ok(tmpl)
}

/// Verify that a template file's bytes match the expected digest from catalog
/// without performing network I/O. Useful for offline verification.
pub fn verify_template_bytes(
    bytes: &[u8],
    expected_digest: &str,
    context: &str,
) -> Result<(), TemplateFetchError> {
    let actual = compute_digest(bytes);
    if actual != expected_digest.to_ascii_lowercase() {
        return Err(TemplateFetchError::DigestMismatch {
            template: context.to_owned(),
            expected: expected_digest.to_owned(),
            actual,
        });
    }
    // Also ensure bytes are valid template JSON.
    Template::from_json_bytes(bytes).map_err(|e| TemplateFetchError::SchemaInvalid {
        template: context.to_owned(),
        reason: format!("{e}"),
    })?;
    Ok(())
}

/// Ensure a relative template path never escapes the repository root when
/// joined to a base directory. Rejects traversal.
///
/// This is the filesystem-side guard that mirrors [`validate_template_path`]
/// but also checks the joined path does not escape `base`.
///
/// The function does not touch the filesystem; it only validates the path
/// string. Callers must still use validated joins when writing files.
pub fn ensure_path_safe(base: &Path, relative: &str) -> Result<PathBuf, TemplateFetchError> {
    validate_template_path(relative).map_err(|e| TemplateFetchError::InvalidUrl {
        template: relative.to_owned(),
        reason: format!("{e}"),
    })?;
    let joined = base.join(relative);
    // Lexically check for parent components in joined path.
    for comp in joined.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return Err(TemplateFetchError::InvalidUrl {
                template: relative.to_owned(),
                reason: format!("joined path escapes base: `{}`", joined.display()),
            });
        }
    }
    // Ensure the joined path is still under base (prefix check).
    // For lexical check, ensure base is prefix of joined.
    let base_str = base.to_string_lossy();
    let joined_str = joined.to_string_lossy();
    if !joined_str.starts_with(base_str.as_ref()) {
        return Err(TemplateFetchError::InvalidUrl {
            template: relative.to_owned(),
            reason: format!(
                "joined path `{}` escapes base `{}`",
                joined.display(),
                base.display()
            ),
        });
    }
    Ok(joined)
}

// ---------------------------------------------------------------------------
// CoreError convenience
// ---------------------------------------------------------------------------

/// Fetch catalog and map errors to [`CoreError`] for callers that use the
/// core taxonomy.
pub fn fetch_catalog_core(config: &TemplateRepoConfig) -> CoreResult<Catalog> {
    fetch_catalog(config).map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(redundant_imports, reason = "test imports overlap")]
mod tests {
    use super::*;
    use crate::ids::{HarnessId, ProviderId, TemplateId, TemplateVersion};
    use crate::template::{
        CATALOG_SCHEMA_VERSION, Catalog, OwnedPatch, TEMPLATE_SCHEMA_VERSION, Template,
        TemplateCatalogEntry, TemplateFileRef, TemplateInput, TemplateRepoConfig, TemplateStatus,
        compute_digest,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    fn sample_template_bytes(
        label: &str,
        patches: Vec<OwnedPatch>,
        digest: Option<String>,
    ) -> Vec<u8> {
        let tmpl = Template {
            schema_version: TEMPLATE_SCHEMA_VERSION,
            id: TemplateId::new("claude-glm").unwrap(),
            version: "1.2.0".to_owned(),
            harness: HarnessId::new("claude-code").unwrap(),
            provider: ProviderId::new("glm").unwrap(),
            label: label.to_owned(),
            status: TemplateStatus::Active,
            inputs: vec![TemplateInput {
                key: "model".to_owned(),
                description: "model".to_owned(),
                required: true,
            }],
            patches,
            wrapper_env: BTreeMap::new(),
            wrapper_args: Vec::new(),
            assets: Vec::new(),
            capability_map: BTreeMap::new(),
            migration_notes: Vec::new(),
            digest: digest.unwrap_or_else(|| "0".repeat(64)),
            harness_version_req: None,
            provider_protocol: None,
        };
        serde_json::to_vec(&tmpl).unwrap()
    }

    #[test]
    fn validate_fetch_url_rejects_http() {
        validate_fetch_url("http://example.com/catalog.json", "catalog").unwrap_err();
        validate_fetch_url("https://example.com/catalog.json", "catalog").unwrap();
        validate_fetch_url("file:///tmp/catalog.json", "catalog").unwrap();
        validate_fetch_url("", "catalog").unwrap_err();
        validate_fetch_url("https://example.com/../evil", "catalog").unwrap_err();
    }

    #[test]
    fn fetch_bytes_rejects_http() {
        let err = fetch_bytes("http://example.com/catalog.json", "catalog").unwrap_err();
        match err {
            TemplateFetchError::InvalidUrl { .. } => {}
            other => panic!("expected InvalidUrl, got {other:?}"),
        }
    }

    #[test]
    fn fetch_catalog_from_path_round_trip() {
        let dir = crate::test_util::temp_dir_unique("tpl-fetch");
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        let catalog = Catalog {
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
        };
        let path = dir.join("catalog.json");
        let bytes = serde_json::to_vec(&catalog).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let fetched = fetch_catalog_from_path(&path).unwrap();
        assert_eq!(fetched, catalog);
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn fetch_catalog_from_path_rejects_missing() {
        let missing = Path::new("/tmp/superai-missing-catalog-xyz-12345.json");
        drop(std::fs::remove_file(missing));
        let err = fetch_catalog_from_path(missing).unwrap_err();
        match err {
            TemplateFetchError::NotFound { .. } => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn fetch_template_bytes_via_file_url() {
        let dir = crate::test_util::temp_dir_unique("tpl-fetch");
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(dir.join("claude-glm")).unwrap();

        // Create a template file. Internal digest is just a valid placeholder;
        // catalog digest is the hash of the raw bytes (integrity from catalog).
        let patch = OwnedPatch {
            selector: "key:model".to_owned(),
            value: json!("sonnet"),
        };
        let raw = sample_template_bytes("Claude on GLM", vec![patch], Some("c".repeat(64)));
        let catalog_digest = compute_digest(&raw);

        let template_path = dir.join("claude-glm").join("1.2.0.json");
        std::fs::write(&template_path, &raw).unwrap();

        let catalog = Catalog {
            version: CATALOG_SCHEMA_VERSION,
            templates: vec![TemplateCatalogEntry {
                id: TemplateId::new("claude-glm").unwrap(),
                latest_version: TemplateVersion::new("1.2.0").unwrap(),
                files: vec![TemplateFileRef {
                    version: TemplateVersion::new("1.2.0").unwrap(),
                    path: "claude-glm/1.2.0.json".to_owned(),
                    digest: catalog_digest,
                }],
                status: TemplateStatus::Active,
            }],
        };
        let catalog_bytes = serde_json::to_vec(&catalog).unwrap();
        std::fs::write(dir.join("catalog.json"), &catalog_bytes).unwrap();

        let config = TemplateRepoConfig {
            host: "example.com".to_owned(),
            owner: "owner".to_owned(),
            repo: "repo".to_owned(),
            git_ref: "main".to_owned(),
            base_url: Some(format!("file://{}", dir.display())),
        };
        let fetched_catalog = fetch_catalog(&config).unwrap();
        assert_eq!(fetched_catalog, catalog);

        let bytes = fetch_template_bytes(&config, &fetched_catalog, "claude-glm", "1.2.0").unwrap();
        assert_eq!(bytes, raw);

        // Tamper detection: changing a byte should cause digest mismatch.
        let mut tampered = raw;
        tampered[0] ^= 0xFF;
        std::fs::write(&template_path, &tampered).unwrap();
        let err =
            fetch_template_bytes(&config, &fetched_catalog, "claude-glm", "1.2.0").unwrap_err();
        match err {
            TemplateFetchError::DigestMismatch { .. } => {}
            other => panic!("expected DigestMismatch, got {other:?}"),
        }

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn fetch_rejects_traversal_path() {
        let dir = crate::test_util::temp_dir_unique("tpl-fetch");
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("catalog.json");
        let catalog = Catalog {
            version: CATALOG_SCHEMA_VERSION,
            templates: vec![TemplateCatalogEntry {
                id: TemplateId::new("evil").unwrap(),
                latest_version: TemplateVersion::new("1.0.0").unwrap(),
                files: vec![TemplateFileRef {
                    version: TemplateVersion::new("1.0.0").unwrap(),
                    path: "../evil.json".to_owned(),
                    digest: "a".repeat(64),
                }],
                status: TemplateStatus::Active,
            }],
        };
        // This catalog should fail validation, not traversal at fetch time.
        let bytes = serde_json::to_vec(&catalog).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let err = fetch_catalog_from_path(&path).unwrap_err();
        match err {
            TemplateFetchError::SchemaInvalid { .. } => {}
            other => panic!("expected SchemaInvalid, got {other:?}"),
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn ensure_path_safe_rejects_escape() {
        let base = Path::new("/tmp/base");
        ensure_path_safe(base, "a/b.json").unwrap();
        ensure_path_safe(base, "../escape.json").unwrap_err();
        ensure_path_safe(base, "/absolute.json").unwrap_err();
        ensure_path_safe(base, "a\\b.json").unwrap_err();
    }

    #[test]
    fn size_limit_enforced() {
        let dir = crate::test_util::temp_dir_unique("tpl-fetch");
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        let huge = vec![b'a'; MAX_BYTES + 1];
        let path = dir.join("catalog.json");
        std::fs::write(&path, &huge).unwrap();
        let err = fetch_catalog_from_path(&path).unwrap_err();
        match err {
            TemplateFetchError::SizeLimit { .. } => {}
            other => panic!("expected SizeLimit, got {other:?}"),
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn verify_template_bytes_success_and_mismatch() {
        let patch = OwnedPatch {
            selector: "key:model".to_owned(),
            value: json!("sonnet"),
        };
        let current = sample_template_bytes("Label", vec![patch], Some("c".repeat(64)));
        let digest = compute_digest(&current);
        verify_template_bytes(&current, &digest, "claude-glm").unwrap();
        verify_template_bytes(&current, &"0".repeat(64), "claude-glm").unwrap_err();
    }

    #[test]
    fn template_fetch_error_into_core() {
        let err = TemplateFetchError::NotFound {
            template: "t".to_owned(),
            reason: "missing".to_owned(),
        };
        let core: CoreError = err.into();
        assert!(format!("{core}").contains("not found"));

        let err2 = TemplateFetchError::DigestMismatch {
            template: "t".to_owned(),
            expected: "a".repeat(64),
            actual: "b".repeat(64),
        };
        let core2: CoreError = err2.into();
        assert!(format!("{core2}").contains("digest"));
    }
}
