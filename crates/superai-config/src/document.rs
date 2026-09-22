//! Format-neutral source envelope and typed selectors (DOC-01/02/09);
//! invalid bytes become diagnostics, never lossy replacements.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde_json::Value;

use crate::atomic::compute_digest;
use crate::error::{ConfigError, Result};

/// Detected encoding; UTF-8 only, adapters opt into anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Encoding {
    /// UTF-8 (with or without BOM).
    #[default]
    Utf8,
}

/// Newline style detected in the raw bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NewlineStyle {
    /// Unix-style `\n` (also used for empty files).
    #[default]
    Lf,
    /// Windows-style `\r\n`.
    Crlf,
}

/// Document kind from the path or caller; `Opaque` means read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DocumentKind {
    /// Strict JSON (no comments, no trailing commas).
    StrictJson,
    /// JSON with comments (JSONC).
    JsonC,
    /// TOML document.
    Toml,
    /// YAML document.
    Yaml,
    /// Dot-env style file.
    Env,
    /// Line-oriented or templated fragment with managed spans.
    TextFragment,
    /// Unknown or executable config, treated as opaque/read-only.
    #[default]
    Opaque,
}

impl DocumentKind {
    /// Infer a kind from the file name; a heuristic, adapters stay authoritative.
    pub fn from_path(path: &Path) -> Self {
        Self::infer_from_path(path)
    }

    fn infer_from_path(path: &Path) -> Self {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let lower = name.to_ascii_lowercase();
            if lower == ".env" || lower.starts_with(".env.") || lower == "env" {
                return Self::Env;
            }
        }

        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return Self::Opaque;
        };
        match ext.to_ascii_lowercase().as_str() {
            "json" => Self::StrictJson,
            "jsonc" => Self::JsonC,
            "toml" => Self::Toml,
            "yaml" | "yml" => Self::Yaml,
            "env" => Self::Env,
            "txt" | "md" | "sh" | "bash" | "zsh" | "rc" | "conf" | "fragment" => Self::TextFragment,
            _ => Self::Opaque,
        }
    }

    /// Human-readable label for the kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StrictJson => "strict_json",
            Self::JsonC => "jsonc",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
            Self::Env => "env",
            Self::TextFragment => "text_fragment",
            Self::Opaque => "opaque",
        }
    }
}

impl std::fmt::Display for DocumentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DocumentKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "strict_json" | "strictjson" | "json" => Ok(Self::StrictJson),
            "jsonc" => Ok(Self::JsonC),
            "toml" => Ok(Self::Toml),
            "yaml" | "yml" => Ok(Self::Yaml),
            "env" => Ok(Self::Env),
            "text_fragment" | "textfragment" | "fragment" | "text" => Ok(Self::TextFragment),
            "opaque" => Ok(Self::Opaque),
            other => Err(format!("unknown document kind: {other}")),
        }
    }
}

/// Severity; syntax problems are `Error`, validators may downgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DiagnosticSeverity {
    /// Blocking problem (the default for syntax diagnostics).
    #[default]
    Error,
    /// Non-blocking warning.
    Warning,
    /// Use of a deprecated key or feature (DOC-09 deprecation diagnostics).
    Deprecation,
    /// Informational hint.
    Hint,
}

impl DiagnosticSeverity {
    /// Stable wire label.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Deprecation => "deprecation",
            Self::Hint => "hint",
        }
    }
}

/// A diagnostic with a line/column span, a severity, and a message.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    /// One-based line number.
    pub line: usize,
    /// One-based column number.
    pub col: usize,
    /// Severity (DOC-09); syntax diagnostics default to `Error`.
    pub severity: DiagnosticSeverity,
    /// Human-readable message.
    pub message: String,
}

impl Diagnostic {
    /// Create a new error-severity diagnostic.
    pub fn new(line: usize, col: usize, message: impl Into<String>) -> Self {
        Self {
            line: usize::max(line, 1),
            col: usize::max(col, 1),
            severity: DiagnosticSeverity::Error,
            message: message.into(),
        }
    }

    /// Create a warning-severity diagnostic (DOC-09).
    pub fn warning(line: usize, col: usize, message: impl Into<String>) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            ..Self::new(line, col, message)
        }
    }

    /// Deprecation diagnostic for an owned key (DOC-09), naming the
    /// replacement when one exists.
    pub fn deprecation(
        line: usize,
        col: usize,
        deprecated: &str,
        replacement: Option<&str>,
    ) -> Self {
        let message = match replacement {
            Some(replacement) => {
                format!("owned key `{deprecated}` is deprecated; use `{replacement}` instead")
            }
            None => format!("owned key `{deprecated}` is deprecated"),
        };
        Self {
            severity: DiagnosticSeverity::Deprecation,
            ..Self::new(line, col, message)
        }
    }

    /// Create a hint-severity diagnostic (DOC-09).
    pub fn hint(line: usize, col: usize, message: impl Into<String>) -> Self {
        Self {
            severity: DiagnosticSeverity::Hint,
            ..Self::new(line, col, message)
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

/// Detect newline style: presence of `\r\n` means `Crlf`, otherwise `Lf`.
fn detect_newline(bytes: &[u8]) -> NewlineStyle {
    let has_crlf = bytes
        .windows(2)
        .any(|w| w.first().copied() == Some(b'\r') && w.get(1).copied() == Some(b'\n'));
    if has_crlf {
        NewlineStyle::Crlf
    } else {
        NewlineStyle::Lf
    }
}

/// Detect BOM and UTF-8 validity; invalid bytes become diagnostics, never replacements.
fn detect_encoding_and_diagnostics(bytes: &[u8]) -> (Encoding, bool, Vec<Diagnostic>) {
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let without_bom = if bom {
        bytes.get(3..).unwrap_or(&[])
    } else {
        bytes
    };

    let mut diagnostics = Vec::new();
    if let Err(err) = std::str::from_utf8(without_bom) {
        let valid_up_to = err.valid_up_to();
        let (line, col) = offset_to_line_col(without_bom, valid_up_to);
        let len = err.error_len().unwrap_or(1);
        diagnostics.push(Diagnostic::new(
            line,
            col,
            format!("invalid utf-8 at {line}:{col} ({len} byte(s))"),
        ));
    }

    (Encoding::Utf8, bom, diagnostics)
}

fn offset_to_line_col(bytes: &[u8], offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    let limit = usize::min(offset, bytes.len());
    for (idx, b) in bytes.iter().enumerate() {
        if idx >= limit {
            break;
        }
        if *b == b'\n' {
            line = line.saturating_add(1);
            col = 1;
        } else {
            col = col.saturating_add(1);
        }
    }
    (line, col)
}

/// Format-neutral document envelope. Missing files are not represented:
/// [`SourceDocument::load`] errors, keeping missing vs empty distinct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDocument {
    /// Original path the document was loaded from.
    pub path: PathBuf,
    /// Raw source bytes exactly as read from disk.
    pub bytes: Vec<u8>,
    /// Detected encoding (UTF-8 default).
    pub encoding: Encoding,
    /// Whether a UTF-8 BOM was present.
    pub bom: bool,
    /// Detected newline style.
    pub newline_style: NewlineStyle,
    /// Hex digest of `bytes` (stable for the loaded snapshot).
    pub digest: String,
    /// Inferred document kind.
    pub kind: DocumentKind,
    /// Parse/encoding diagnostics with spans.
    pub diagnostics: Vec<Diagnostic>,
}

impl SourceDocument {
    /// Envelope from already-read bytes; detection is pure, no I/O.
    pub fn from_bytes(path: &Path, bytes: Vec<u8>) -> Self {
        Self::from_bytes_with_kind(path, bytes, DocumentKind::from_path(path))
    }

    /// Envelope with an explicit kind for callers that know better than the heuristic.
    pub fn from_bytes_with_kind(path: &Path, bytes: Vec<u8>, kind: DocumentKind) -> Self {
        let newline_style = detect_newline(&bytes);
        let (encoding, bom, diagnostics) = detect_encoding_and_diagnostics(&bytes);
        let digest = compute_digest(&bytes);
        Self {
            path: path.to_path_buf(),
            bytes,
            encoding,
            bom,
            newline_style,
            digest,
            kind,
            diagnostics,
        }
    }

    /// Load fresh: missing is `Io`/`NotFound`, empty is zero bytes, invalid
    /// UTF-8 keeps the bytes with a diagnostic. No caching.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| ConfigError::io(path, e))?;
        Ok(Self::from_bytes(path, bytes))
    }

    /// Whether the document is empty (zero bytes, regardless of kind).
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Whether the document has any diagnostics.
    pub fn has_diagnostics(&self) -> bool {
        !self.diagnostics.is_empty()
    }

    /// UTF-8 view with BOM stripped; `None` when invalid, never lossy.
    pub fn text(&self) -> Option<&str> {
        let slice: &[u8] = if self.bom {
            self.bytes.get(3..).unwrap_or(&[])
        } else {
            &self.bytes
        };
        std::str::from_utf8(slice).ok()
    }

    /// Raw text including BOM bytes if present, when valid UTF-8.
    pub fn text_with_bom(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    /// Recompute the digest and check it matches the stored one.
    pub fn verify_digest(&self) -> bool {
        compute_digest(&self.bytes) == self.digest
    }
}

/// Typed edit selector; adapters declare which variants are stable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Selector {
    /// Object/map key.
    Key(String),
    /// Array index, only with proof the position is stable.
    Index(usize),
    /// Identity-selected array item (e.g. model id or server name).
    Identity {
        /// Key field that identifies the item (e.g. `"name"` or `"id"`).
        key: String,
        /// Expected value of the identity key.
        value: String,
    },
    /// TOML table path, e.g. `["servers", "production"]`.
    TomlTable(Vec<String>),
    /// Managed text span delimited by stable sentinels.
    ManagedSpan(String),
}

impl Selector {
    /// Parse `key:`, `index:`, `identity:`, `table:`/`toml:`, `span:`/`managed:`
    /// prefixes (case-insensitive); bare strings fall back to `Key`.
    pub fn parse(input: &str) -> std::result::Result<Self, String> {
        Self::from_str(input)
    }

    /// Return a stable string representation for round-tripping.
    pub fn to_typed_string(&self) -> String {
        match self {
            Self::Key(k) => format!("key:{k}"),
            Self::Index(i) => format!("index:{i}"),
            Self::Identity { key, value } => format!("identity:{key}={value}"),
            Self::TomlTable(parts) => format!("table:{}", parts.join(".")),
            Self::ManagedSpan(name) => format!("span:{name}"),
        }
    }
}

fn parse_key_selector(rest: &str) -> std::result::Result<Selector, String> {
    if rest.is_empty() {
        return Err("key selector requires a name".to_owned());
    }
    Ok(Selector::Key(rest.to_owned()))
}

fn parse_index_selector(rest: &str) -> std::result::Result<Selector, String> {
    let n: usize = rest
        .parse()
        .map_err(|_err| format!("invalid index selector: {rest}"))?;
    Ok(Selector::Index(n))
}

fn parse_identity_selector(rest: &str) -> std::result::Result<Selector, String> {
    let Some(eq) = rest.find('=') else {
        return Err("identity selector requires key=value".to_owned());
    };
    let key = rest.get(0..eq).unwrap_or_default();
    let value = rest.get(eq + 1..).unwrap_or_default();
    if key.is_empty() || value.is_empty() {
        return Err("identity selector requires non-empty key and value".to_owned());
    }
    Ok(Selector::Identity {
        key: key.to_owned(),
        value: value.to_owned(),
    })
}

fn parse_table_selector(rest: &str) -> std::result::Result<Selector, String> {
    if rest.is_empty() {
        return Err("table selector requires a path".to_owned());
    }
    let parts: Vec<String> = rest
        .split('.')
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("table selector requires at least one segment".to_owned());
    }
    Ok(Selector::TomlTable(parts))
}

fn parse_span_selector(rest: &str) -> std::result::Result<Selector, String> {
    if rest.is_empty() {
        return Err("span selector requires a name".to_owned());
    }
    Ok(Selector::ManagedSpan(rest.to_owned()))
}

impl FromStr for Selector {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("selector must not be empty".to_owned());
        }

        let Some(colon) = trimmed.find(':') else {
            return Ok(Self::Key(trimmed.to_owned()));
        };

        let prefix = trimmed
            .get(0..colon)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let rest = trimmed.get(colon + 1..).unwrap_or_default();
        match prefix.as_str() {
            "key" => parse_key_selector(rest),
            "index" => parse_index_selector(rest),
            "identity" => parse_identity_selector(rest),
            "table" | "toml" | "tomltable" => parse_table_selector(rest),
            "span" | "managed" | "managedspan" => parse_span_selector(rest),
            _ => Ok(Self::Key(trimmed.to_owned())),
        }
    }
}

impl std::fmt::Display for Selector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_typed_string())
    }
}

/// How to handle duplicate keys or values during an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DuplicateHandling {
    /// Overwrite the existing entry.
    #[default]
    Overwrite,
    /// Keep the first occurrence and ignore later duplicates.
    KeepFirst,
    /// Fail if a duplicate would be created.
    Error,
    /// Append a new entry without deduplicating.
    Append,
}

/// Redaction policy for diffs and diagnostics that may contain secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RedactionPolicy {
    /// No redaction.
    #[default]
    None,
    /// Redact the value being set.
    RedactValue,
    /// Redact the key/selector.
    RedactKey,
    /// Redact both.
    Full,
}

/// Typed edit variants (DOC-02); ownership/conflict/duplicate/redaction
/// policies live in the wrapping [`Operation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOperation {
    /// Set a value at a selector.
    Set {
        /// Target selector.
        selector: Selector,
        /// New value.
        value: Value,
    },
    /// Insert a new map/table entry at a policy-defined position.
    InsertEntry {
        /// Parent selector.
        selector: Selector,
        /// Key to insert.
        key: String,
        /// Value to insert.
        value: Value,
    },
    /// Remove a key/entry at a selector.
    Remove {
        /// Target selector.
        selector: Selector,
    },
    /// Merge owned fields into an object while retaining foreign fields.
    Merge {
        /// Target selector (usually an object).
        selector: Selector,
        /// Object to merge.
        value: Value,
    },
    /// Enable or disable an entry without deleting its definition.
    EnableDisable {
        /// Target selector.
        selector: Selector,
        /// Desired enabled state.
        enabled: bool,
    },
    /// Append or remove an identity-keyed item in an array.
    AppendIdentityItem {
        /// Array selector.
        selector: Selector,
        /// Item to append.
        value: Value,
        /// Field inside `value` carrying the item's identity; duplicate
        /// handling compares it.
        identity_key: String,
    },
    /// Ensure a directory or list entry exists (e.g. skills, plugins).
    EnsureDirEntry {
        /// Parent selector.
        selector: Selector,
        /// Entry path or name.
        path: String,
    },
}

/// A fully specified operation: addressing, payload, and policies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    /// The typed edit to perform.
    pub kind: EditOperation,
    /// Adapter-owned keys at the target. Empty owns nothing and rejects
    /// every selector (DOC-02): ownership is declared, never assumed.
    pub owned_keys: Vec<String>,
    /// Expected previous state: `None` accepts anything, `Some(None)`
    /// requires absence, `Some(Some(v))` equality; mismatch writes nothing.
    pub expected_old: Option<Option<Value>>,
    /// Duplicate-key policy.
    pub duplicate_handling: DuplicateHandling,
    /// Whether to create parent tables/objects if they are missing.
    pub create_parent: bool,
    /// Redaction policy for secrets in diffs/diagnostics.
    pub redaction_policy: RedactionPolicy,
}

impl Operation {
    /// Create a new operation with required kind and default policies.
    pub fn new(kind: EditOperation) -> Self {
        Self {
            kind,
            owned_keys: Vec::new(),
            expected_old: None,
            duplicate_handling: DuplicateHandling::default(),
            create_parent: false,
            redaction_policy: RedactionPolicy::default(),
        }
    }

    /// Declare which keys are owned.
    #[must_use]
    pub fn with_owned_keys(mut self, keys: Vec<String>) -> Self {
        self.owned_keys = keys;
        self
    }

    /// Declare the expected previous state: `Some(v)` equality, `None`
    /// absence; not calling it disables conflict detection.
    #[must_use]
    pub fn with_expected_old(mut self, expected: Option<Value>) -> Self {
        self.expected_old = Some(expected);
        self
    }

    /// Set duplicate handling.
    #[must_use]
    pub fn with_duplicate_handling(mut self, handling: DuplicateHandling) -> Self {
        self.duplicate_handling = handling;
        self
    }

    /// Enable or disable parent creation.
    #[must_use]
    pub fn with_create_parent(mut self, create: bool) -> Self {
        self.create_parent = create;
        self
    }

    /// Set the redaction policy.
    #[must_use]
    pub fn with_redaction_policy(mut self, policy: RedactionPolicy) -> Self {
        self.redaction_policy = policy;
        self
    }

    /// Borrow the selector for this operation, if any.
    pub fn selector(&self) -> &Selector {
        match &self.kind {
            EditOperation::Set { selector, .. }
            | EditOperation::InsertEntry { selector, .. }
            | EditOperation::Remove { selector }
            | EditOperation::Merge { selector, .. }
            | EditOperation::EnableDisable { selector, .. }
            | EditOperation::AppendIdentityItem { selector, .. }
            | EditOperation::EnsureDirEntry { selector, .. } => selector,
        }
    }
}

/// Adapter-supplied pure validator returning diagnostics (empty = valid).
pub type SemanticValidator =
    std::sync::Arc<dyn Fn(&Value, DocumentKind) -> Vec<Diagnostic> + Send + Sync>;

/// An owned key the adapter has deprecated (DOC-09 deprecation diagnostics).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeprecatedKey {
    /// Dotted key path (or typed selector string) that is deprecated.
    pub key: String,
    /// Replacement pointer (e.g. `"key:newModel"`), if one exists.
    pub replacement: Option<String>,
}

impl DeprecatedKey {
    /// Create a deprecation marker for `key` with an optional replacement.
    pub fn new(key: impl Into<String>, replacement: Option<String>) -> Self {
        Self {
            key: key.into(),
            replacement,
        }
    }
}

/// Validator hook plus deprecated keys for `validate_with_schema`.
#[derive(Default, Clone)]
pub struct SemanticSchema {
    /// Validator over the parsed semantic value, if the adapter supplies one.
    pub validator: Option<SemanticValidator>,
    /// Owned keys the adapter has deprecated.
    pub deprecated_keys: Vec<DeprecatedKey>,
}

impl std::fmt::Debug for SemanticSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticSchema")
            .field("validator", &self.validator.as_ref().map_or(0, |_| 1))
            .field("deprecated_keys", &self.deprecated_keys)
            .finish()
    }
}

impl SemanticSchema {
    /// Attach a semantic validator.
    #[must_use]
    pub fn with_validator(mut self, validator: SemanticValidator) -> Self {
        self.validator = Some(validator);
        self
    }

    /// Declare deprecated owned keys.
    #[must_use]
    pub fn with_deprecated(mut self, keys: Vec<DeprecatedKey>) -> Self {
        self.deprecated_keys = keys;
        self
    }
}

/// Expected value type at a key path (DOC-09 path/value type checks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueType {
    /// A JSON-family object (`{ … }`).
    Object,
    /// An array.
    Array,
    /// A string.
    String,
    /// A number (integer or float; `1` and `1.0` are both numbers).
    Number,
    /// A boolean.
    Boolean,
    /// The null value.
    Null,
}

impl ValueType {
    /// Name shown in diagnostics.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Array => "array",
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Null => "null",
        }
    }

    /// The type `value` currently holds.
    pub fn of(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(_) => Self::Boolean,
            Value::Number(_) => Self::Number,
            Value::String(_) => Self::String,
            Value::Array(_) => Self::Array,
            Value::Object(_) => Self::Object,
        }
    }
}

/// Check the dotted `path` exists and holds `expected`. The error names
/// the failing segment, never a value, so it is safe to surface.
pub fn check_path_type(
    value: &Value,
    path: &str,
    expected: ValueType,
) -> std::result::Result<(), String> {
    let mut current = value;
    let segments: Vec<&str> = path.split('.').map(str::trim).collect();
    for (idx, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            return Err(format!("path `{path}` has an empty segment"));
        }
        let Value::Object(map) = current else {
            return Err(format!(
                "path `{path}`: segment {} of `{}` is not an object",
                idx.saturating_add(1),
                segments.first().unwrap_or(&"")
            ));
        };
        match map.get(*segment) {
            Some(next) => current = next,
            None => {
                return Err(format!("path `{path}`: segment `{segment}` is missing"));
            }
        }
    }
    let actual = ValueType::of(current);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "path `{path}` holds a {}, expected a {}",
            actual.as_str(),
            expected.as_str()
        ))
    }
}

/// Deprecation diagnostics for deprecated keys present in `value`;
/// positions are approximate (1:1): the value tree carries no spans.
pub fn deprecation_diagnostics(value: &Value, deprecated: &[DeprecatedKey]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for entry in deprecated {
        if resolve_dotted(value, &entry.key).is_some() {
            diagnostics.push(Diagnostic::deprecation(
                1,
                1,
                &entry.key,
                entry.replacement.as_deref(),
            ));
        }
    }
    diagnostics
}

/// Resolve a dotted key path in `value`, walking objects only.
pub(crate) fn resolve_dotted<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.').map(str::trim) {
        if segment.is_empty() {
            return None;
        }
        let Value::Object(map) = current else {
            return None;
        };
        current = map.get(segment)?;
    }
    Some(current)
}

/// Parse-check `bytes` as `kind` (staging and restore verification);
/// env lines must be blank, comments, or `KEY=VALUE`.
pub(crate) fn validate_bytes_for_kind(
    content: &[u8],
    kind: DocumentKind,
    path: &Path,
) -> Result<()> {
    match kind {
        DocumentKind::StrictJson => {
            std::str::from_utf8(content).map_err(|_err| {
                ConfigError::io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8 in json"),
                )
            })?;
            serde_json::from_slice::<Value>(content).map_err(|source| ConfigError::Json {
                path: path.to_path_buf(),
                source,
            })?;
            Ok(())
        }
        DocumentKind::JsonC => {
            let text = std::str::from_utf8(content).map_err(|_err| {
                ConfigError::io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8 in jsonc"),
                )
            })?;
            let stripped = strip_jsonc_comments(text);
            serde_json::from_str::<Value>(&stripped).map_err(|source| ConfigError::Json {
                path: path.to_path_buf(),
                source,
            })?;
            Ok(())
        }
        DocumentKind::Toml => {
            let text = std::str::from_utf8(content).map_err(|_err| {
                ConfigError::io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8 in toml"),
                )
            })?;
            let _ = text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|source| ConfigError::Toml {
                    path: path.to_path_buf(),
                    source,
                })?;
            Ok(())
        }
        DocumentKind::Yaml => {
            let text = std::str::from_utf8(content).map_err(|_err| {
                ConfigError::io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8 in yaml"),
                )
            })?;
            yaml_serde::from_str::<Value>(text).map_err(|source| ConfigError::Yaml {
                path: path.to_path_buf(),
                source,
            })?;
            Ok(())
        }
        DocumentKind::Env => {
            // Validate env: each non-blank, non-comment line must contain '='
            let text = std::str::from_utf8(content).map_err(|_err| ConfigError::Env {
                path: path.to_path_buf(),
                message: "invalid utf8 in env file".to_owned(),
            })?;
            for (idx, line) in text.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let without_export = if let Some(rest) = trimmed.strip_prefix("export ") {
                    rest.trim()
                } else {
                    trimmed
                };
                if !without_export.contains('=') {
                    return Err(ConfigError::Env {
                        path: path.to_path_buf(),
                        message: format!("line {} missing '='", idx + 1),
                    });
                }
                if without_export.starts_with('=') {
                    return Err(ConfigError::Env {
                        path: path.to_path_buf(),
                        message: format!("line {} has empty key", idx + 1),
                    });
                }
            }
            Ok(())
        }
        DocumentKind::TextFragment | DocumentKind::Opaque => Ok(()),
    }
}

/// Strip `//` and `/* */` comments outside strings; strings survive verbatim.
#[expect(
    clippy::excessive_nesting,
    reason = "comment stripping state machine requires nesting"
)]
pub(crate) fn strip_jsonc_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
            output.push(ch);
        } else if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    while let Some(&peek) = chars.peek() {
                        if peek == '\n' {
                            break;
                        }
                        chars.next();
                    }
                }
                Some('*') => {
                    chars.next();
                    loop {
                        match chars.next() {
                            Some('*') => {
                                if chars.peek().copied() == Some('/') {
                                    chars.next();
                                    break;
                                }
                            }
                            Some(_) => {}
                            None => break,
                        }
                    }
                }
                _ => output.push(ch),
            }
        } else {
            output.push(ch);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn doc(path: &str, bytes: &[u8]) -> SourceDocument {
        SourceDocument::from_bytes(Path::new(path), bytes.to_vec())
    }

    /// Absolute on every platform; `"/tmp/..."` is drive-relative on Windows.
    fn tmp_path(name: &str) -> String {
        std::env::temp_dir()
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn kind_from_path_json() {
        assert_eq!(
            DocumentKind::from_path(&std::env::temp_dir().join("settings.json")),
            DocumentKind::StrictJson
        );
    }

    #[test]
    fn kind_from_path_jsonc() {
        assert_eq!(
            DocumentKind::from_path(Path::new("config.jsonc")),
            DocumentKind::JsonC
        );
    }

    #[test]
    fn kind_from_path_toml() {
        assert_eq!(
            DocumentKind::from_path(Path::new("Cargo.toml")),
            DocumentKind::Toml
        );
    }

    #[test]
    fn kind_from_path_yaml_variants() {
        assert_eq!(
            DocumentKind::from_path(Path::new("a.yaml")),
            DocumentKind::Yaml
        );
        assert_eq!(
            DocumentKind::from_path(Path::new("b.yml")),
            DocumentKind::Yaml
        );
    }

    #[test]
    fn kind_from_path_env() {
        assert_eq!(
            DocumentKind::from_path(Path::new(".env")),
            DocumentKind::Env
        );
        assert_eq!(
            DocumentKind::from_path(Path::new(".env.local")),
            DocumentKind::Env
        );
        assert_eq!(
            DocumentKind::from_path(Path::new("/home/user/project/.env")),
            DocumentKind::Env
        );
        assert_eq!(
            DocumentKind::from_path(Path::new("secrets.env")),
            DocumentKind::Env
        );
    }

    #[test]
    fn kind_from_path_text_fragment() {
        assert_eq!(
            DocumentKind::from_path(Path::new("notes.txt")),
            DocumentKind::TextFragment
        );
    }

    #[test]
    fn kind_from_path_opaque_for_unknown() {
        assert_eq!(
            DocumentKind::from_path(Path::new("binary.bin")),
            DocumentKind::Opaque
        );
        assert_eq!(
            DocumentKind::from_path(Path::new("noext")),
            DocumentKind::Opaque
        );
    }

    #[test]
    fn kind_display_and_from_str_round_trip() {
        for kind in [
            DocumentKind::StrictJson,
            DocumentKind::JsonC,
            DocumentKind::Toml,
            DocumentKind::Yaml,
            DocumentKind::Env,
            DocumentKind::TextFragment,
            DocumentKind::Opaque,
        ] {
            let s = kind.as_str();
            let parsed: DocumentKind = s.parse().unwrap();
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn envelope_empty_file_has_lf_and_no_diagnostics() {
        let d = doc(&tmp_path("empty.json"), b"");
        assert!(d.is_empty());
        assert_eq!(d.newline_style, NewlineStyle::Lf);
        assert!(!d.bom);
        assert_eq!(d.encoding, Encoding::Utf8);
        assert!(d.diagnostics.is_empty());
        assert!(d.verify_digest());
        assert_eq!(d.kind, DocumentKind::StrictJson);
    }

    #[test]
    fn envelope_detects_bom() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"{\"a\":1}");
        let d = doc(&tmp_path("with_bom.json"), &bytes);
        assert!(d.bom);
        assert_eq!(d.text(), Some("{\"a\":1}"));
        assert!(d.diagnostics.is_empty());
    }

    #[test]
    fn envelope_detects_crlf() {
        let d = doc(&tmp_path("a.toml"), b"a = 1\r\nb = 2\r\n");
        assert_eq!(d.newline_style, NewlineStyle::Crlf);
    }

    #[test]
    fn envelope_detects_lf() {
        let d = doc(&tmp_path("a.toml"), b"a = 1\nb = 2\n");
        assert_eq!(d.newline_style, NewlineStyle::Lf);
    }

    #[test]
    fn envelope_invalid_utf8_is_diagnostic_not_replacement() {
        let bytes = vec![0xFF, 0xFE, b'{'];
        let d = doc(&tmp_path("bad.json"), &bytes);
        assert!(!d.diagnostics.is_empty());
        assert!(d.text().is_none());
        assert_eq!(d.bytes, bytes);
        let diag = &d.diagnostics[0];
        assert_eq!(diag.line, 1);
        assert!(!diag.message.is_empty());
    }

    #[test]
    fn envelope_digest_is_stable_and_changes_with_bytes() {
        let same_path = tmp_path("a.json");
        let a = doc(&same_path, b"{}");
        let b = doc(&same_path, b"{}");
        let c = doc(&same_path, b"{\"a\":1}");
        assert_eq!(a.digest, b.digest);
        assert_ne!(a.digest, c.digest);
        assert_eq!(a.digest.len(), 16);
    }

    #[test]
    fn envelope_load_distinguishes_missing_vs_empty() {
        let missing =
            std::env::temp_dir().join(format!("superai-doc-test-missing-{}", std::process::id()));
        drop(std::fs::remove_file(&missing));
        let err = SourceDocument::load(&missing).unwrap_err();
        match err {
            ConfigError::Io { path, .. } => assert_eq!(path, missing),
            other => panic!("unexpected error: {other:?}"),
        }

        let empty_path = crate::test_util::temp_dir_unique("config-doc").join("empty.json");
        std::fs::write(&empty_path, b"").unwrap();
        let doc = SourceDocument::load(&empty_path).unwrap();
        assert!(doc.is_empty());
        assert!(doc.diagnostics.is_empty());
        drop(std::fs::remove_file(&empty_path));
    }

    #[test]
    fn envelope_root_shape_is_not_validated_by_envelope() {
        let d = doc(&tmp_path("a.json"), b"[1,2,3]");
        assert_eq!(d.kind, DocumentKind::StrictJson);
        assert!(d.diagnostics.is_empty());
    }

    #[test]
    fn envelope_from_bytes_with_explicit_kind() {
        let d = SourceDocument::from_bytes_with_kind(
            &std::env::temp_dir().join("unknown.bin"),
            b"hello".to_vec(),
            DocumentKind::Env,
        );
        assert_eq!(d.kind, DocumentKind::Env);
    }

    #[test]
    fn selector_parse_key() {
        assert_eq!(
            Selector::parse("key:foo").unwrap(),
            Selector::Key("foo".to_owned())
        );
        assert_eq!(
            Selector::parse("foo").unwrap(),
            Selector::Key("foo".to_owned())
        );
    }

    #[test]
    fn selector_parse_index() {
        assert_eq!(Selector::parse("index:0").unwrap(), Selector::Index(0));
        assert_eq!(Selector::parse("index:42").unwrap(), Selector::Index(42));
        Selector::parse("index:abc").unwrap_err();
    }

    #[test]
    fn selector_parse_identity() {
        assert_eq!(
            Selector::parse("identity:name=my-server").unwrap(),
            Selector::Identity {
                key: "name".to_owned(),
                value: "my-server".to_owned()
            }
        );
        Selector::parse("identity:novalue").unwrap_err();
    }

    #[test]
    fn selector_parse_toml_table() {
        assert_eq!(
            Selector::parse("table:servers.production").unwrap(),
            Selector::TomlTable(vec!["servers".to_owned(), "production".to_owned()])
        );
        assert_eq!(
            Selector::parse("toml:a.b.c").unwrap(),
            Selector::TomlTable(vec!["a".to_owned(), "b".to_owned(), "c".to_owned()])
        );
    }

    #[test]
    fn selector_parse_managed_span() {
        assert_eq!(
            Selector::parse("span:my-span").unwrap(),
            Selector::ManagedSpan("my-span".to_owned())
        );
        assert_eq!(
            Selector::parse("managed:my-span").unwrap(),
            Selector::ManagedSpan("my-span".to_owned())
        );
    }

    #[test]
    fn selector_round_trip_via_display() {
        let cases = [
            Selector::Key("foo".to_owned()),
            Selector::Index(7),
            Selector::Identity {
                key: "id".to_owned(),
                value: "abc".to_owned(),
            },
            Selector::TomlTable(vec!["a".to_owned(), "b".to_owned()]),
            Selector::ManagedSpan("superai".to_owned()),
        ];
        for s in cases {
            let serialized = s.to_string();
            let parsed = Selector::parse(&serialized).unwrap();
            assert_eq!(parsed, s);
        }
    }

    #[test]
    fn selector_rejects_empty() {
        Selector::parse("").unwrap_err();
        Selector::parse("   ").unwrap_err();
    }

    #[test]
    fn operation_carries_owned_keys_and_policies() {
        let op = Operation::new(EditOperation::Set {
            selector: Selector::Key("model".to_owned()),
            value: Value::String("sonnet".to_owned()),
        })
        .with_owned_keys(vec!["model".to_owned()])
        .with_expected_old(Some(Value::String("opus".to_owned())))
        .with_duplicate_handling(DuplicateHandling::Error)
        .with_create_parent(true)
        .with_redaction_policy(RedactionPolicy::RedactValue);

        assert_eq!(op.owned_keys, vec!["model"]);
        assert_eq!(
            op.expected_old,
            Some(Some(Value::String("opus".to_owned())))
        );
        assert_eq!(op.duplicate_handling, DuplicateHandling::Error);
        assert!(op.create_parent);
        assert_eq!(op.redaction_policy, RedactionPolicy::RedactValue);
        assert_eq!(op.selector(), &Selector::Key("model".to_owned()));
    }

    #[test]
    fn operation_expected_old_absent_vs_no_expectation() {
        let no_expectation = Operation::new(EditOperation::Remove {
            selector: Selector::Key("a".to_owned()),
        });
        assert_eq!(no_expectation.expected_old, None);

        let expect_absent = no_expectation.clone().with_expected_old(None);
        assert_eq!(expect_absent.expected_old, Some(None));

        let expect_value = no_expectation.with_expected_old(Some(Value::Bool(true)));
        assert_eq!(expect_value.expected_old, Some(Some(Value::Bool(true))));
    }

    #[test]
    fn operation_insert_entry() {
        let op = Operation::new(EditOperation::InsertEntry {
            selector: Selector::Key("mcpServers".to_owned()),
            key: "my-server".to_owned(),
            value: Value::String("http://example".to_owned()),
        });
        match op.kind {
            EditOperation::InsertEntry { ref key, .. } => assert_eq!(key, "my-server"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn operation_all_variants_constructible() {
        let _ = Operation::new(EditOperation::Remove {
            selector: Selector::Key("old".to_owned()),
        });
        let mut map = Map::new();
        map.insert("x".to_owned(), Value::Bool(true));
        let _ = Operation::new(EditOperation::Merge {
            selector: Selector::Key("obj".to_owned()),
            value: Value::Object(map),
        });
        let _ = Operation::new(EditOperation::EnableDisable {
            selector: Selector::Key("feature".to_owned()),
            enabled: false,
        });
        let _ = Operation::new(EditOperation::AppendIdentityItem {
            selector: Selector::Key("servers".to_owned()),
            value: Value::String("x".to_owned()),
            identity_key: "name".to_owned(),
        });
        let _ = Operation::new(EditOperation::EnsureDirEntry {
            selector: Selector::Key("plugins".to_owned()),
            path: tmp_path("foo"),
        });
    }

    #[test]
    fn diagnostic_severity_defaults_to_error_and_has_constructors() {
        let error = Diagnostic::new(1, 1, "syntax");
        assert_eq!(error.severity, DiagnosticSeverity::Error);

        let warning = Diagnostic::warning(2, 3, "careful");
        assert_eq!(warning.severity, DiagnosticSeverity::Warning);
        assert_eq!((warning.line, warning.col), (2, 3));

        let hint = Diagnostic::hint(4, 1, "fyi");
        assert_eq!(hint.severity, DiagnosticSeverity::Hint);

        let severities = [
            DiagnosticSeverity::Error,
            DiagnosticSeverity::Warning,
            DiagnosticSeverity::Deprecation,
            DiagnosticSeverity::Hint,
        ];
        let labels: Vec<&str> = severities.iter().map(DiagnosticSeverity::as_str).collect();
        assert_eq!(labels, ["error", "warning", "deprecation", "hint"]);
    }

    #[test]
    fn deprecation_diagnostic_names_key_and_replacement() {
        let with_replacement = Diagnostic::deprecation(1, 1, "key:oldModel", Some("key:model"));
        assert_eq!(with_replacement.severity, DiagnosticSeverity::Deprecation);
        assert!(with_replacement.message.contains("key:oldModel"));
        assert!(with_replacement.message.contains("key:model"));

        let without = Diagnostic::deprecation(1, 1, "legacyKey", None);
        assert!(without.message.contains("legacyKey"));
        assert!(!without.message.contains("use `"));
    }

    #[test]
    fn check_path_type_passes_and_fails_on_type_mismatch() {
        let value: Value = serde_json::from_str(r#"{"model":{"name":"opus"},"list":[1]}"#).unwrap();
        check_path_type(&value, "model", ValueType::Object).unwrap();
        check_path_type(&value, "model.name", ValueType::String).unwrap();
        check_path_type(&value, "list", ValueType::Array).unwrap();

        let mismatch = check_path_type(&value, "model.name", ValueType::Number).unwrap_err();
        assert!(mismatch.contains("model.name"));
        assert!(mismatch.contains("string"));

        let missing = check_path_type(&value, "model.absent", ValueType::String).unwrap_err();
        assert!(missing.contains("absent"));

        let not_object =
            check_path_type(&value, "model.name.deeper", ValueType::String).unwrap_err();
        assert!(not_object.contains("not an object"));
    }

    #[test]
    fn deprecation_diagnostics_fire_only_for_present_keys() {
        let value: Value = serde_json::from_str(r#"{"oldModel":"x","other":1}"#).unwrap();
        let deprecated = vec![
            DeprecatedKey::new("oldModel", Some("model".to_owned())),
            DeprecatedKey::new("notPresent", None),
        ];
        let diagnostics = deprecation_diagnostics(&value, &deprecated);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Deprecation);
        assert!(diagnostics[0].message.contains("oldModel"));
        assert!(diagnostics[0].message.contains("model"));
    }
}
