//! Format-neutral source document envelope and typed selectors.
//!
//! Implements DOC-01 (envelope) and DOC-02 (typed selectors / edit ops).
//! The envelope is deliberately codec-agnostic: it stores raw bytes,
//! detected encoding/BOM/newline style, a content digest, an inferred
//! [`DocumentKind`], and parse diagnostics with line/column spans.
//!
//! Rules enforced here:
//! - UTF-8 is the default encoding; adapters must opt into any other.
//! - Invalid encoding is reported as a [`Diagnostic`], not lossy replacement.
//! - Missing file and empty file are distinct (missing is an I/O error,
//!   empty is a present document with zero bytes).
//! - Root-shape expectations are an adapter/schema concern, not enforced here.
//!
//! Selectors and operations are typed (no ad-hoc dotted strings) so
//! codecs and adapters can reason about stability and redaction.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde_json::Value;

use crate::error::{ConfigError, Result};

// ---------------------------------------------------------------------------
// Encoding + newline
// ---------------------------------------------------------------------------

/// Text encoding detected for a source document.
///
/// Only UTF-8 is assumed by default. Other encodings require an explicit
/// adapter opt-in; this envelope records diagnostics instead of silently
/// replacing invalid bytes.
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

// ---------------------------------------------------------------------------
// DocumentKind
// ---------------------------------------------------------------------------

/// Kind of document inferred from its path and/or explicit caller intent.
///
/// Adapters may override the inference; this envelope only carries the kind
/// for codec dispatch. `Opaque` means no structured editing should be attempted.
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
    /// Unknown or executable config — treated as opaque/read-only.
    #[default]
    Opaque,
}

impl DocumentKind {
    /// Infer a kind from a file path's extension and file name.
    ///
    /// This is a heuristic for UI and codec dispatch; adapters decide the
    /// authoritative kind and the root-shape policy.
    pub fn from_path(path: &Path) -> Self {
        Self::infer_from_path(path)
    }

    /// Alias for [`Self::from_path`] to match the task's "envelope detection" naming.
    pub fn detect(path: &Path) -> Self {
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

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// A diagnostic with a line/column span and a message.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    /// One-based line number.
    pub line: usize,
    /// One-based column number.
    pub col: usize,
    /// Human-readable message.
    pub message: String,
}

impl Diagnostic {
    /// Create a new diagnostic.
    pub fn new(line: usize, col: usize, message: impl Into<String>) -> Self {
        Self {
            line: usize::max(line, 1),
            col: usize::max(col, 1),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

// ---------------------------------------------------------------------------
// Helpers: digest, newline, bom, encoding
// ---------------------------------------------------------------------------

/// Compute a hex digest of `bytes` using the standard library hasher.
///
/// The spec allows a simple hash of bytes via `std` (not a cryptographic
/// requirement). The output is a zero-padded 16-hex-digit string.
fn compute_digest(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
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

/// Detect BOM and UTF-8 validity, returning encoding, bom flag, and diagnostics.
///
/// UTF-8 is always reported; invalid sequences produce diagnostics instead of
/// replacement characters.
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

// ---------------------------------------------------------------------------
// SourceDocument envelope
// ---------------------------------------------------------------------------

/// Format-neutral source document envelope.
///
/// Carries raw bytes, detected metadata, an inferred kind, and diagnostics.
/// Missing files are not represented: [`SourceDocument::load`] returns an
/// I/O error for missing paths, keeping missing vs empty distinct.
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
    /// Create an envelope from already-read bytes and a path.
    ///
    /// Detection is pure: no I/O. The digest, encoding, BOM, newline style,
    /// kind, and diagnostics are derived from `bytes` and `path`.
    pub fn from_bytes(path: &Path, bytes: Vec<u8>) -> Self {
        let kind = DocumentKind::from_path(path);
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

    /// Create an envelope from bytes with an explicit kind.
    ///
    /// Useful when the caller knows the kind more precisely than the
    /// extension heuristic.
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

    /// Load a document fresh from disk.
    ///
    /// - Missing file → `Err(ConfigError::Io { kind: NotFound, .. })` (distinct from empty).
    /// - Empty file → `Ok` with zero `bytes` and no diagnostics.
    /// - Invalid UTF-8 → `Ok` with diagnostics; bytes are preserved, no replacement.
    ///
    /// Disk is the truth: no caching.
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

    /// Attempt to view the bytes as UTF-8, stripping a leading BOM if present.
    ///
    /// Returns `None` when the bytes are not valid UTF-8 (diagnostics will
    /// explain why). This never performs lossy replacement.
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

// ---------------------------------------------------------------------------
// Selector
// ---------------------------------------------------------------------------

/// Typed selector for a document edit, avoiding ad-hoc dotted strings.
///
/// Adapters declare which selector variants are stable for a given surface;
/// e.g. array `Index` is only allowed when the schema proves the index is
/// stable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Selector {
    /// Object/map key.
    Key(String),
    /// Array index — only when adapter proves the position is stable.
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
    /// Parse a selector from a string form.
    ///
    /// Supported forms (prefixes are case-insensitive):
    /// - `key:<name>` or `key:<dotted.path>` → [`Selector::Key`]
    /// - `index:<n>` → [`Selector::Index`]
    /// - `identity:<key>=<value>` → [`Selector::Identity`]
    /// - `table:<a.b.c>` or `toml:<a.b.c>` → [`Selector::TomlTable`]
    /// - `span:<name>` or `managed:<name>` → [`Selector::ManagedSpan`]
    ///
    /// Bare strings without a recognised prefix fall back to `Key`.
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

// ---------------------------------------------------------------------------
// Edit operations
// ---------------------------------------------------------------------------

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

/// Typed edit operation variants (DOC-02).
///
/// Each variant carries the minimal addressing/value payload; per-operation
/// policies like `owned_keys` or `duplicate_handling` are stored in the
/// wrapping [`Operation`] so callers declare ownership, conflict expectations,
/// and redaction explicitly.
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
    },
    /// Ensure a directory or list entry exists (e.g. skills, plugins).
    EnsureDirEntry {
        /// Parent selector.
        selector: Selector,
        /// Entry path or name.
        path: String,
    },
}

/// A fully specified operation with addressing, payload, and policies.
///
/// Each operation declares:
/// - `owned_keys` – which keys the adapter claims ownership of
/// - `expected_old` – expected previous value or `None` for absence (conflict detection)
/// - `duplicate_handling` – how to treat duplicates
/// - `create_parent` – whether missing parents should be created
/// - `redaction_policy` – what to redact in diffs/diagnostics
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    /// The typed edit to perform.
    pub kind: EditOperation,
    /// Keys owned by the adapter at the target (for merge/ownership checks).
    pub owned_keys: Vec<String>,
    /// Expected previous value, or `None` if the entry should be absent.
    pub expected_old: Option<Value>,
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

    /// Declare the expected old value (or absence).
    #[must_use]
    pub fn with_expected_old(mut self, expected: Option<Value>) -> Self {
        self.expected_old = expected;
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

// ---------------------------------------------------------------------------
// Tests — envelope detection + selector parsing
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn doc(path: &str, bytes: &[u8]) -> SourceDocument {
        SourceDocument::from_bytes(Path::new(path), bytes.to_vec())
    }

    // ---- DocumentKind detection ----

    #[test]
    fn kind_from_path_json() {
        assert_eq!(
            DocumentKind::from_path(Path::new("/tmp/settings.json")),
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
    fn kind_detect_alias_matches_from_path() {
        let p = Path::new("a.json");
        assert_eq!(DocumentKind::detect(p), DocumentKind::from_path(p));
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

    // ---- Envelope basics ----

    #[test]
    fn envelope_empty_file_has_lf_and_no_diagnostics() {
        let d = doc("/tmp/empty.json", b"");
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
        let d = doc("/tmp/with_bom.json", &bytes);
        assert!(d.bom);
        assert_eq!(d.text(), Some("{\"a\":1}"));
        assert!(d.diagnostics.is_empty());
    }

    #[test]
    fn envelope_detects_crlf() {
        let d = doc("/tmp/a.toml", b"a = 1\r\nb = 2\r\n");
        assert_eq!(d.newline_style, NewlineStyle::Crlf);
    }

    #[test]
    fn envelope_detects_lf() {
        let d = doc("/tmp/a.toml", b"a = 1\nb = 2\n");
        assert_eq!(d.newline_style, NewlineStyle::Lf);
    }

    #[test]
    fn envelope_invalid_utf8_is_diagnostic_not_replacement() {
        // 0xFF is never valid UTF-8.
        let bytes = vec![0xFF, 0xFE, b'{'];
        let d = doc("/tmp/bad.json", &bytes);
        assert!(!d.diagnostics.is_empty());
        assert!(d.text().is_none());
        // Bytes are preserved verbatim.
        assert_eq!(d.bytes, bytes);
        let diag = &d.diagnostics[0];
        assert_eq!(diag.line, 1);
        assert!(!diag.message.is_empty());
    }

    #[test]
    fn envelope_digest_is_stable_and_changes_with_bytes() {
        let a = doc("/tmp/a.json", b"{}");
        let b = doc("/tmp/a.json", b"{}");
        let c = doc("/tmp/a.json", b"{\"a\":1}");
        assert_eq!(a.digest, b.digest);
        assert_ne!(a.digest, c.digest);
        assert_eq!(a.digest.len(), 16);
    }

    #[test]
    fn envelope_load_distinguishes_missing_vs_empty() {
        let missing = PathBuf::from(format!(
            "/tmp/superai-doc-test-missing-{}",
            std::process::id()
        ));
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
        // StrictJson with an array root is allowed at envelope level; adapter decides.
        let d = doc("/tmp/a.json", b"[1,2,3]");
        assert_eq!(d.kind, DocumentKind::StrictJson);
        assert!(d.diagnostics.is_empty());
    }

    #[test]
    fn envelope_from_bytes_with_explicit_kind() {
        let d = SourceDocument::from_bytes_with_kind(
            Path::new("/tmp/unknown.bin"),
            b"hello".to_vec(),
            DocumentKind::Env,
        );
        assert_eq!(d.kind, DocumentKind::Env);
    }

    // ---- Selector parsing ----

    #[test]
    fn selector_parse_key() {
        assert_eq!(
            Selector::parse("key:foo").unwrap(),
            Selector::Key("foo".to_owned())
        );
        // Bare string falls back to Key.
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

    // ---- Operations carry required policies ----

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
        assert_eq!(op.expected_old, Some(Value::String("opus".to_owned())));
        assert_eq!(op.duplicate_handling, DuplicateHandling::Error);
        assert!(op.create_parent);
        assert_eq!(op.redaction_policy, RedactionPolicy::RedactValue);
        assert_eq!(op.selector(), &Selector::Key("model".to_owned()));
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
        });
        let _ = Operation::new(EditOperation::EnsureDirEntry {
            selector: Selector::Key("plugins".to_owned()),
            path: "/tmp/foo".to_owned(),
        });
    }
}
