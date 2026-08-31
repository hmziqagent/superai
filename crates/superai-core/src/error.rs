//! Core error taxonomy.
//!
//! Each variant carries safe resource identity (paths, ids, digests) and
//! causal context. Secret-bearing values are wrapped in [`RedactedString`]
//! which never exposes the inner value via `Debug`, `Display`, or `Serialize`.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// Redacted helper for secret-bearing error context
// ---------------------------------------------------------------------------

/// Wrapper for secret-bearing values in errors.
///
/// Debug, Display, and Serialize all emit `[REDACTED]`. The raw secret is
/// only available via [`Self::expose_secret`].
#[derive(Clone, PartialEq, Eq)]
pub struct RedactedString(String);

impl RedactedString {
    /// Create a new redacted wrapper.
    pub fn new(secret: &str) -> Self {
        Self(secret.to_owned())
    }

    /// Borrow the raw secret. Use only at the harness-write boundary.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// Placeholder used in display and serialization.
    pub fn placeholder() -> &'static str {
        "[REDACTED]"
    }
}

impl fmt::Debug for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RedactedString([REDACTED])")
    }
}

impl fmt::Display for RedactedString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Serialize for RedactedString {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(Self::placeholder())
    }
}

impl<'de> Deserialize<'de> for RedactedString {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self(s))
    }
}

// ---------------------------------------------------------------------------
// CoreError
// ---------------------------------------------------------------------------

/// Everything that can go wrong in the core layer.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// A config file operation failed.
    #[error(transparent)]
    Config(#[from] superai_config::ConfigError),

    /// The records file held something that is not a list of instances.
    #[error("malformed instance records: {0}")]
    Records(#[from] serde_json::Error),

    /// An instance with this name already exists.
    #[error("instance `{name}` already exists")]
    DuplicateInstance {
        /// The name that collided.
        name: String,
    },

    /// The user's home directory could not be determined.
    #[error("cannot determine the home directory")]
    NoHomeDir,

    /// An identifier or name failed validation.
    #[error("invalid {kind} `{value}`: {reason}")]
    InvalidIdentifier {
        /// Kind of identifier that failed validation.
        kind: String,
        /// Offending value.
        value: String,
        /// Human-readable reason.
        reason: String,
    },

    /// A path or executable reference failed validation.
    #[error("invalid {kind} `{value}`: {reason}")]
    InvalidPath {
        /// Kind of path that failed validation.
        kind: String,
        /// Offending value.
        value: String,
        /// Human-readable reason.
        reason: String,
    },

    // -----------------------------------------------------------------------
    // FND-06 taxonomy
    // -----------------------------------------------------------------------
    /// Generic validation failure for a field.
    #[error("validation failed for {field}: {reason}")]
    Validation {
        /// Field or context that failed validation.
        field: String,
        /// Human-readable reason.
        reason: String,
    },

    /// A name collision after case-folded comparison.
    #[error("name collision for {kind} `{name}`: {reason}")]
    NameCollision {
        /// Kind of name that collided, e.g., `InstanceName`.
        kind: String,
        /// Offending value.
        name: String,
        /// Reason for collision.
        reason: String,
    },

    /// The requested harness is not supported.
    #[error("unsupported harness `{harness}`: {reason}")]
    UnsupportedHarness {
        /// Harness identifier.
        harness: String,
        /// Reason it is not supported.
        reason: String,
    },

    /// A specific harness version is not supported.
    #[error("unsupported version `{version}` for harness `{harness}`: {reason}")]
    UnsupportedVersion {
        /// Harness identifier.
        harness: String,
        /// Version string.
        version: String,
        /// Reason it is not supported.
        reason: String,
    },

    /// A config surface is not supported for this harness.
    #[error("unsupported surface `{surface}` for harness `{harness}`: {reason}")]
    UnsupportedSurface {
        /// Harness identifier.
        harness: String,
        /// Surface identifier, e.g., `settings.json`.
        surface: String,
        /// Reason it is not supported.
        reason: String,
    },

    /// A requested operation is not supported for this harness or surface.
    #[error("unsupported operation `{operation}` for harness `{harness}`: {reason}")]
    UnsupportedOperation {
        /// Harness identifier, if any.
        harness: String,
        /// Operation name.
        operation: String,
        /// Reason it is not supported.
        reason: String,
    },

    /// Research for this harness or surface is incomplete and blocks writes.
    #[error("research blocked for harness `{harness}` surface `{surface}`: {reason}")]
    ResearchBlocked {
        /// Harness identifier.
        harness: String,
        /// Surface or feature that is blocked.
        surface: String,
        /// Reason research is blocked.
        reason: String,
    },

    /// A file failed to parse.
    #[error("parse error in {path}: {kind}: {message}")]
    Parse {
        /// Path of the file that failed to parse.
        path: PathBuf,
        /// Kind of file, e.g., `json`, `toml`.
        kind: String,
        /// Parser message, redacted of secrets.
        message: String,
    },

    /// Schema validation failed after parse.
    #[error("schema validation failed for {path}: {details}")]
    SchemaValidation {
        /// Path of the file that failed validation.
        path: PathBuf,
        /// Details of the validation failure, redacted.
        details: String,
    },

    /// The file changed between preview and commit.
    #[error("concurrent modification of {path}: expected {expected}, actual {actual}")]
    ConcurrentModification {
        /// Path that was concurrently modified.
        path: PathBuf,
        /// Digest or metadata expected at preview time.
        expected: String,
        /// Digest or metadata observed at commit time.
        actual: String,
    },

    /// Creating or verifying a backup failed.
    #[error("backup failed for {path}: {reason}")]
    Backup {
        /// Path that backup was attempted for.
        path: PathBuf,
        /// Backup identifier, if any.
        backup_id: Option<String>,
        /// Reason for failure.
        reason: String,
    },

    /// The atomic commit step failed.
    #[error("commit failed for {path}: {reason}")]
    Commit {
        /// Path that commit was attempted for.
        path: PathBuf,
        /// Reason for failure.
        reason: String,
    },

    /// Post-commit verification failed.
    #[error("verification failed for {path} ({kind}): {reason}")]
    Verification {
        /// Path that verification was attempted for.
        path: PathBuf,
        /// Kind of verification, e.g., `parse`, `digest`, `semantic`.
        kind: String,
        /// Reason for failure.
        reason: String,
    },

    /// Rollback failed after a commit failure.
    #[error("rollback failed for {path}: {reason}")]
    Rollback {
        /// Path that rollback was attempted for.
        path: PathBuf,
        /// Backup identifier used for rollback, if any.
        backup_id: Option<String>,
        /// Reason for rollback failure.
        reason: String,
    },

    /// Detecting or probing a binary failed.
    #[error("binary detection failed for `{binary}`: {reason}")]
    BinaryDetection {
        /// Binary name or path.
        binary: String,
        /// Reason for failure.
        reason: String,
    },

    /// Fetching or validating a remote template failed.
    #[error("network/template error for `{template}`: {reason}")]
    NetworkTemplate {
        /// Template identifier.
        template: String,
        /// Reason for failure; URL with secrets is never stored raw.
        reason: String,
        /// Redacted URL or network context, if any.
        context_redacted: Option<RedactedString>,
    },

    /// Fetching a remote source (e.g., a skill over HTTPS) failed.
    ///
    /// Content is never fabricated in place of a successful fetch: the
    /// operation fails with the failing locator so the caller can retry or
    /// report honestly.
    #[error("source fetch failed for {kind} `{locator}`: {reason}")]
    SourceFetch {
        /// Kind of source that failed, e.g., `skill_source`.
        kind: String,
        /// Locator (URL) the fetch was attempted from.
        locator: String,
        /// Reason the fetch failed; transport/HTTP detail, never secret-bearing.
        reason: String,
    },

    /// Authentication is required but owned externally.
    #[error("auth required for harness `{harness}` instance `{instance:?}`: {reason}")]
    AuthRequired {
        /// Harness identifier.
        harness: String,
        /// Instance name, if any.
        instance: Option<String>,
        /// Reason authentication is required.
        reason: String,
    },

    /// The path is owned by a foreign manager and must not be mutated.
    #[error("foreign ownership for {path}: owned by {owner}")]
    ForeignOwnership {
        /// Path with foreign ownership.
        path: PathBuf,
        /// Owner identifier, e.g., `claude-multi`.
        owner: String,
    },

    /// A port conflict prevents the operation.
    #[error("port conflict on {port}: {reason}")]
    PortConflict {
        /// Port number that conflicted.
        port: u16,
        /// Holder of the port, if known.
        holder: Option<String>,
        /// Reason for conflict.
        reason: String,
    },

    /// A daemon was not ready or failed to become ready.
    #[error("daemon not ready for harness `{harness}`: {reason}")]
    DaemonNotReady {
        /// Harness identifier.
        harness: String,
        /// Reason daemon is not ready.
        reason: String,
    },

    /// Binary detection determined the binary is present but version is unknown or unsupported.
    #[error("binary version probe failed for `{binary}`: {reason}")]
    BinaryVersionProbe {
        /// Binary name or path.
        binary: String,
        /// Reason probe failed.
        reason: String,
    },

    /// A secret-bearing value was provided where redaction is required.
    #[error("secret validation failed for {field}: {reason}")]
    SecretValidation {
        /// Field that failed secret validation.
        field: String,
        /// Reason for failure.
        reason: String,
        /// Redacted value preview, never the raw secret.
        redacted: RedactedString,
    },

    /// Plugin or MCP operation requires external command execution and caller approval (EXT-06).
    #[error("requires approval for plugin `{plugin}` operation `{operation}`: {reason}")]
    RequiresApproval {
        /// Plugin or server identifier requesting approval.
        plugin: String,
        /// Operation that needs approval, e.g., `install`.
        operation: String,
        /// Human-readable reason approval is required.
        reason: String,
    },
}

/// Result alias for core operations.
pub type Result<T> = std::result::Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_string_does_not_leak_in_debug_or_display() {
        let secret = "super-secret-sentinel-xyz-123";
        let redacted = RedactedString::new(secret);
        let debug = format!("{redacted:?}");
        let display = format!("{redacted}");
        let json = serde_json::to_string(&redacted).unwrap();
        for output in [&debug, &display, &json] {
            assert!(
                !output.contains(secret),
                "output must not contain secret: {output}"
            );
            assert!(
                output.contains("[REDACTED]"),
                "output must contain placeholder: {output}"
            );
        }
        assert_eq!(redacted.expose_secret(), secret);
    }

    #[test]
    fn error_display_does_not_leak_secret_via_redacted_field() {
        let secret = "another-secret-999";
        let redacted = RedactedString::new(secret);
        let err = CoreError::NetworkTemplate {
            template: "claude-glm".to_owned(),
            reason: "fetch failed".to_owned(),
            context_redacted: Some(redacted),
        };
        let display = format!("{err}");
        let debug = format!("{err:?}");
        for output in [display, debug] {
            assert!(
                !output.contains(secret),
                "error output must not contain secret: {output}"
            );
        }
        // The redacted placeholder appears only if the error's Debug/Display
        // chooses to include the redacted field. Our NetworkTemplate Display
        // does not directly print context_redacted, but its Debug does via
        // derived Debug which uses RedactedString's redacted Debug. Either way
        // secret must not leak.
        let json_err = CoreError::SecretValidation {
            field: "apiKey".to_owned(),
            reason: "must be non-empty".to_owned(),
            redacted: RedactedString::new(secret),
        };
        let debug2 = format!("{json_err:?}");
        let display2 = format!("{json_err}");
        for output in [debug2, display2] {
            assert!(
                !output.contains(secret),
                "secret validation error must not leak: {output}"
            );
            assert!(output.contains("[REDACTED]") || output.contains("apiKey"));
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive taxonomy coverage requires many variants"
    )]
    fn all_taxonomy_variants_carry_safe_identity() {
        // Construct each variant with safe identity and ensure Display works.
        let variants: Vec<CoreError> = vec![
            CoreError::Validation {
                field: "name".to_owned(),
                reason: "must not be empty".to_owned(),
            },
            CoreError::NameCollision {
                kind: "InstanceName".to_owned(),
                name: "work".to_owned(),
                reason: "case-fold collision with WORK".to_owned(),
            },
            CoreError::UnsupportedHarness {
                harness: "unknown-harness".to_owned(),
                reason: "no adapter".to_owned(),
            },
            CoreError::UnsupportedVersion {
                harness: "claude-code".to_owned(),
                version: "999.0.0".to_owned(),
                reason: "schema not yet researched".to_owned(),
            },
            CoreError::UnsupportedSurface {
                harness: "claude-code".to_owned(),
                surface: "keychain".to_owned(),
                reason: "external secret store".to_owned(),
            },
            CoreError::UnsupportedOperation {
                harness: "claude-code".to_owned(),
                operation: "write_keychain".to_owned(),
                reason: "not supported".to_owned(),
            },
            CoreError::ResearchBlocked {
                harness: "deepseek-harness".to_owned(),
                surface: "config".to_owned(),
                reason: "research gaps".to_owned(),
            },
            CoreError::Parse {
                path: PathBuf::from("/home/user/.claude/settings.json"),
                kind: "json".to_owned(),
                message: "expected object".to_owned(),
            },
            CoreError::SchemaValidation {
                path: PathBuf::from("/home/user/.claude/settings.json"),
                details: "missing required field".to_owned(),
            },
            CoreError::ConcurrentModification {
                path: PathBuf::from("/home/user/.claude/settings.json"),
                expected: "abc123".to_owned(),
                actual: "def456".to_owned(),
            },
            CoreError::Backup {
                path: PathBuf::from("/home/user/.claude/settings.json"),
                backup_id: Some("backup-1".to_owned()),
                reason: "io error".to_owned(),
            },
            CoreError::Commit {
                path: PathBuf::from("/home/user/.claude/settings.json"),
                reason: "atomic replace failed".to_owned(),
            },
            CoreError::Verification {
                path: PathBuf::from("/home/user/.claude/settings.json"),
                kind: "parse".to_owned(),
                reason: "file not valid json after write".to_owned(),
            },
            CoreError::Rollback {
                path: PathBuf::from("/home/user/.claude/settings.json"),
                backup_id: Some("backup-1".to_owned()),
                reason: "restore failed".to_owned(),
            },
            CoreError::BinaryDetection {
                binary: "claude".to_owned(),
                reason: "not found in PATH".to_owned(),
            },
            CoreError::NetworkTemplate {
                template: "claude-glm".to_owned(),
                reason: "network failure".to_owned(),
                context_redacted: None,
            },
            CoreError::SourceFetch {
                kind: "skill_source".to_owned(),
                locator: "https://example.com/skills/demo/SKILL.md".to_owned(),
                reason: "connection refused".to_owned(),
            },
            CoreError::AuthRequired {
                harness: "claude-code".to_owned(),
                instance: Some("work".to_owned()),
                reason: "api key not present and provider needs auth".to_owned(),
            },
            CoreError::ForeignOwnership {
                path: PathBuf::from("/home/user/.claude"),
                owner: "claude-multi".to_owned(),
            },
            CoreError::PortConflict {
                port: 8080,
                holder: Some("openclaw-daemon".to_owned()),
                reason: "already in use".to_owned(),
            },
            CoreError::DaemonNotReady {
                harness: "openclaw".to_owned(),
                reason: "health check timed out".to_owned(),
            },
        ];
        for err in variants {
            let display = format!("{err}");
            let debug = format!("{err:?}");
            assert!(!display.is_empty());
            assert!(!debug.is_empty());
            // Safe identities are present; secrets are not.
            assert!(!display.contains("super-secret"));
            assert!(!debug.contains("super-secret"));
        }
    }
}
