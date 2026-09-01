use std::path::PathBuf;

/// Everything that can go wrong reading or writing a harness config file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read, written, or copied.
    #[error("io error on {path}: {source}")]
    Io {
        /// Path the operation was attempted on.
        path: PathBuf,
        /// Underlying OS error.
        source: std::io::Error,
    },

    /// The file exists but is not valid JSON.
    #[error("invalid json in {path}: {source}")]
    Json {
        /// Path of the offending file.
        path: PathBuf,
        /// Parser error.
        source: serde_json::Error,
    },

    /// The file exists but is not valid TOML.
    #[error("invalid toml in {path}: {source}")]
    Toml {
        /// Path of the offending file.
        path: PathBuf,
        /// Parser error.
        source: toml_edit::TomlError,
    },

    /// The file exists but is not valid YAML.
    #[error("invalid yaml in {path}: {source}")]
    Yaml {
        /// Path of the offending file.
        path: PathBuf,
        /// Parser error.
        source: yaml_serde::Error,
    },

    /// The file exists but is not a valid env file.
    #[error("invalid env file in {path}: {message}")]
    Env {
        /// Path of the offending file.
        path: PathBuf,
        /// Human-readable message.
        message: String,
    },

    /// A JSON config was expected to hold an object at its root.
    #[error("expected a json object at the root of {path}")]
    NotAnObject {
        /// Path of the offending file.
        path: PathBuf,
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

    /// Post-commit verification failed.
    #[error("verification failed for {path}: {reason}")]
    Verification {
        /// Path that verification was attempted for.
        path: PathBuf,
        /// Human-readable reason.
        reason: String,
    },

    /// Backup verification failed.
    #[error("backup verification failed for {path}: {reason}")]
    BackupVerification {
        /// Path that backup verification was attempted for.
        path: PathBuf,
        /// Human-readable reason.
        reason: String,
    },

    /// A changing write was refused because the codec cannot preserve the
    /// file's lexical content (comments, anchors, tags, scalar style,
    /// trailing commas). The format stays read-only for such writes until a
    /// lexically preserving codec exists (plans/01 DOC-05/DOC-06).
    #[error(
        "lossy write unsupported for {path}: {format} is read-only until a lexically preserving codec exists"
    )]
    LossyWrite {
        /// Path of the file the write was refused for.
        path: PathBuf,
        /// Human-readable format label (e.g. `"jsonc"`, `"yaml"`).
        format: &'static str,
    },

    /// The operation's selector addresses a key outside the operation's
    /// declared `owned_keys` (DOC-02). Nothing was written.
    #[error("selector {selector} is not within the owned keys declared for {path}")]
    NotOwned {
        /// Path of the document being edited.
        path: PathBuf,
        /// The rejected selector (redacted when the operation's policy says so).
        selector: String,
    },

    /// The value at the selector does not match the operation's `expected_old`
    /// expectation (DOC-02). Typed conflict: nothing was written.
    #[error("operation conflict at {selector} in {path}: expected {expected}, found {actual}")]
    OperationConflict {
        /// Path of the document being edited.
        path: PathBuf,
        /// Selector the conflict occurred at (redacted when policy says so).
        selector: String,
        /// Expected previous value (redacted rendering).
        expected: String,
        /// Actual current value (redacted rendering).
        actual: String,
    },

    /// The parent of the selector's path is missing and the operation did
    /// not allow creating it (DOC-02 `create_parent` policy). Nothing was
    /// written.
    #[error("missing parent for {selector} in {path} and create_parent is disabled")]
    ParentMissing {
        /// Path of the document being edited.
        path: PathBuf,
        /// Selector whose parent is missing (redacted when policy says so).
        selector: String,
    },

    /// The declared duplicate-handling mode rejects this edit (DOC-02
    /// `duplicate_handling`). Nothing was written.
    #[error("duplicate rejected at {selector} in {path}: {reason}")]
    DuplicateRejected {
        /// Path of the document being edited.
        path: PathBuf,
        /// Selector the duplicate was detected at.
        selector: String,
        /// Which duplicate situation was rejected.
        reason: String,
    },

    /// The operation cannot be applied to this document kind or shape
    /// (DOC-02). Typed refusal so callers fail closed instead of guessing.
    #[error("unsupported operation at {selector} on {path}: {reason}")]
    UnsupportedOperation {
        /// Path of the document being edited.
        path: PathBuf,
        /// Selector the operation addressed.
        selector: String,
        /// Why the operation is unsupported here.
        reason: String,
    },

    /// Managed-span sentinels are missing, duplicated, unbalanced, or
    /// nested (DOC-08). The fragment is treated as invalid and stays
    /// unwritten — fail closed rather than guess which span is owned.
    #[error("invalid managed spans in {path}: {reason}")]
    InvalidSpans {
        /// Path of the fragment.
        path: PathBuf,
        /// What failed (sentinel names and lines only, never span content).
        reason: String,
    },

    /// A text-fragment write was refused because bytes outside managed
    /// spans would change (DOC-08): whole-file rewrites stay opaque and
    /// read-only; only managed-span edits are accepted.
    #[error("unmanaged text fragment write refused for {path}: {reason}")]
    UnmanagedSpanWrite {
        /// Path of the fragment.
        path: PathBuf,
        /// Why the write was refused.
        reason: String,
    },

    /// Two planned paths resolve to the same inode on one device (MUT-02):
    /// a hard link alias. Committing both would silently mutate the same
    /// bytes twice and atomic replacement of one path breaks link sharing,
    /// so the plan is rejected.
    #[error("hard link conflict at {path}: also targets {alias} ({reason})")]
    HardlinkConflict {
        /// Path that collided.
        path: PathBuf,
        /// The other planned path sharing the inode.
        alias: PathBuf,
        /// Why the shared identity is rejected.
        reason: String,
    },

    /// An existing symlink does not point at the owned/expected target
    /// (MUT-02/MUT-06 `replace symlink only if it matches expected owned
    /// target`). Nothing was replaced.
    #[error("symlink target mismatch at {path}: expected {expected}, found {actual}")]
    SymlinkTargetMismatch {
        /// Link path that was refused replacement.
        path: PathBuf,
        /// Expected (owned) target rendering.
        expected: String,
        /// Actual target rendering observed on disk.
        actual: String,
    },

    /// A recursive copy cannot proceed with the requested policy for this
    /// entry (MUT-06): special file, broken/looping link under a
    /// content-following policy, or platform limitation.
    #[error("copy unsupported at {path}: {reason}")]
    UnsupportedCopy {
        /// Path the copy was refused for.
        path: PathBuf,
        /// Why the copy is unsupported.
        reason: String,
    },

    /// A mutation target is (or resolves through) a symlink whose referent
    /// lies OUTSIDE the adapter-allowed root set (MUT-02 default link
    /// policy: follow an existing symlink only after resolving the target
    /// within adapter-allowed roots). Nothing was mutated and the link
    /// itself was never replaced — following it would silently mutate a
    /// file the caller never declared authority over.
    #[error(
        "symlink follow refused at {path}: resolves to {resolved}, outside allowed roots {roots}"
    )]
    SymlinkFollowRefused {
        /// Symlink path whose follow was refused.
        path: PathBuf,
        /// Where the link resolves (or why it could not be resolved).
        resolved: String,
        /// Rendering of the declared allowed roots.
        roots: String,
    },
}

impl ConfigError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn concurrent_modification(
        path: impl Into<PathBuf>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self::ConcurrentModification {
            path: path.into(),
            expected: expected.into(),
            actual: actual.into(),
        }
    }

    pub(crate) fn verification(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::Verification {
            path: path.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn backup_verification(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::BackupVerification {
            path: path.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn lossy_write(path: impl Into<PathBuf>, format: &'static str) -> Self {
        Self::LossyWrite {
            path: path.into(),
            format,
        }
    }

    pub(crate) fn not_owned(path: impl Into<PathBuf>, selector: impl Into<String>) -> Self {
        Self::NotOwned {
            path: path.into(),
            selector: selector.into(),
        }
    }

    pub(crate) fn operation_conflict(
        path: impl Into<PathBuf>,
        selector: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self::OperationConflict {
            path: path.into(),
            selector: selector.into(),
            expected: expected.into(),
            actual: actual.into(),
        }
    }

    pub(crate) fn parent_missing(path: impl Into<PathBuf>, selector: impl Into<String>) -> Self {
        Self::ParentMissing {
            path: path.into(),
            selector: selector.into(),
        }
    }

    pub(crate) fn duplicate_rejected(
        path: impl Into<PathBuf>,
        selector: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::DuplicateRejected {
            path: path.into(),
            selector: selector.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn unsupported_operation(
        path: impl Into<PathBuf>,
        selector: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::UnsupportedOperation {
            path: path.into(),
            selector: selector.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn unmanaged_span_write(
        path: impl Into<PathBuf>,
        reason: impl Into<String>,
    ) -> Self {
        Self::UnmanagedSpanWrite {
            path: path.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn hardlink_conflict(
        path: impl Into<PathBuf>,
        alias: impl Into<PathBuf>,
        reason: impl Into<String>,
    ) -> Self {
        Self::HardlinkConflict {
            path: path.into(),
            alias: alias.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn symlink_target_mismatch(
        path: impl Into<PathBuf>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self::SymlinkTargetMismatch {
            path: path.into(),
            expected: expected.into(),
            actual: actual.into(),
        }
    }

    pub(crate) fn unsupported_copy(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::UnsupportedCopy {
            path: path.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn symlink_follow_refused(
        path: impl Into<PathBuf>,
        resolved: impl Into<String>,
        roots: impl Into<String>,
    ) -> Self {
        Self::SymlinkFollowRefused {
            path: path.into(),
            resolved: resolved.into(),
            roots: roots.into(),
        }
    }
}

/// Result alias for config operations.
pub type Result<T> = std::result::Result<T, ConfigError>;
