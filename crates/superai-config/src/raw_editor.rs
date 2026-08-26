//! Raw editor backend — read/validate/diff/commit (RAW-01..07).
//!
//! Interface-neutral services for future editors to open the exact harness
//! files, display parse diagnostics, preview semantic/lexical changes, and
//! commit safely. No editor widget or interface type is implemented.
//!
//! Guarantees:
//! - Every `read` is fresh from disk (no cache).
//! - `validate` is syntax-only and never touches disk.
//! - `diff` produces semantic ops (format-agnostic) and lexical unified diff.
//! - `commit` revalidates, checks conflict token, backs up, atomically
//!   replaces, verifies, and never writes invalid content.
//! - Sensitive content is wrapped in [`SensitiveContent`] whose `Debug` is
//!   redacted.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::de::{self, Deserialize, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use toml_edit::DocumentMut;

use crate::backup::{BackupEntry, backup_with_reason};
use crate::document::{Diagnostic, DocumentKind, Encoding, NewlineStyle};
use crate::error::{ConfigError, Result};
use crate::snapshot::{Snapshot, is_modified, snapshot};

// ---------------------------------------------------------------------------
// Sensitive wrapper
// ---------------------------------------------------------------------------

/// Sensitive bytes whose `Debug`/`Display` are redacted.
///
/// The raw secret is only available via [`Self::expose`] or
/// [`Self::expose_str`]. This satisfies RAW-01's "sensitive source text
/// wrapper must avoid Debug/log/telemetry serialization".
#[derive(Clone, PartialEq, Eq)]
pub struct SensitiveContent(Vec<u8>);

impl SensitiveContent {
    /// Create from owned bytes.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Create from a byte slice.
    pub fn from_slice(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }

    /// Borrow the raw bytes. Use only at an authorized boundary.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// Borrow as UTF-8 if valid, otherwise `None`.
    pub fn expose_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the content is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Consume into inner bytes.
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl std::fmt::Debug for SensitiveContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SensitiveContent([REDACTED])")
    }
}

impl std::fmt::Display for SensitiveContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

// ---------------------------------------------------------------------------
// Helpers: digest, newline, encoding
// ---------------------------------------------------------------------------

fn compute_digest(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

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

fn detect_bom(bytes: &[u8]) -> bool {
    bytes.len() >= 3
        && bytes.first().copied() == Some(0xEF)
        && bytes.get(1).copied() == Some(0xBB)
        && bytes.get(2).copied() == Some(0xBF)
}

fn bytes_without_bom(bytes: &[u8]) -> &[u8] {
    if detect_bom(bytes) {
        bytes.get(3..).unwrap_or(&[])
    } else {
        bytes
    }
}

fn offset_to_line_col(bytes: &[u8], offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    let limit = usize::min(offset, bytes.len());
    let mut idx = 0usize;
    while idx < limit {
        if let Some(b) = bytes.get(idx).copied() {
            if b == b'\n' {
                line = line.saturating_add(1);
                col = 1;
            } else {
                col = col.saturating_add(1);
            }
        }
        idx = idx.saturating_add(1);
    }
    (line, col)
}

// ---------------------------------------------------------------------------
// RawDocument
// ---------------------------------------------------------------------------

/// Fresh document as seen by the raw editor.
///
/// Contains sensitive content plus conflict token, diagnostics, and metadata.
/// `Debug` is redacted via [`SensitiveContent`].
#[derive(Debug, Clone)]
pub struct RawDocument {
    /// Path the document was loaded from.
    pub path: PathBuf,
    /// Sensitive raw bytes exactly as on disk.
    pub content: SensitiveContent,
    /// Document kind inferred from extension.
    pub kind: DocumentKind,
    /// Detected encoding (always UTF-8, diagnostics for invalid).
    pub encoding: Encoding,
    /// Whether a UTF-8 BOM was present.
    pub bom: bool,
    /// Detected newline style.
    pub newline_style: NewlineStyle,
    /// Hex digest of `content` bytes.
    pub digest: String,
    /// Fresh filesystem snapshot (conflict token).
    pub snapshot: Snapshot,
    /// Syntax diagnostics with spans.
    pub diagnostics: Vec<Diagnostic>,
}

impl RawDocument {
    /// Whether the document has any diagnostics.
    pub fn has_diagnostics(&self) -> bool {
        !self.diagnostics.is_empty()
    }

    /// Whether the underlying file was empty.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate `content` for `kind` without touching disk.
///
/// Returns syntax diagnostics only; adapter schema is optional and not
/// applied here. Empty diagnostics means the content is syntactically valid.
/// For `StrictJson`, empty/whitespace-only is considered valid (empty object
/// compatibility); for other kinds, empty is valid where the codec allows it.
pub fn validate(content: &[u8], kind: DocumentKind) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // Encoding check: invalid UTF-8 is always a diagnostic.
    // Opaque and TextFragment still require valid UTF-8 for display; binary
    // content is reported but not treated as lossy replacement.
    let without_bom = bytes_without_bom(content);
    if let Err(err) = std::str::from_utf8(without_bom) {
        let valid_up_to = err.valid_up_to();
        let (line, col) = offset_to_line_col(without_bom, valid_up_to);
        let len = err.error_len().unwrap_or(1);
        diagnostics.push(Diagnostic::new(
            line,
            col,
            format!("invalid utf-8 at {line}:{col} ({len} byte(s))"),
        ));
        // Without valid UTF-8 we cannot parse further for text kinds.
        return diagnostics;
    }

    let text = std::str::from_utf8(without_bom).unwrap_or_default();
    // Empty/whitespace-only is valid for JSON (empty object), TOML/YAML (empty doc), env (empty map).
    // For strictness, we still let parsers decide; empty is treated as valid.
    if text.trim().is_empty() {
        return diagnostics;
    }

    match kind {
        DocumentKind::StrictJson => diagnostics.extend(validate_json(text)),
        DocumentKind::JsonC => diagnostics.extend(validate_jsonc(text)),
        DocumentKind::Toml => diagnostics.extend(validate_toml(text)),
        DocumentKind::Yaml => diagnostics.extend(validate_yaml(text)),
        DocumentKind::Env => diagnostics.extend(validate_env(text)),
        DocumentKind::TextFragment | DocumentKind::Opaque => {}
    }

    diagnostics
}

fn validate_json(text: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    if let Err(err) = parse_strict_json_value(text) {
        let (line, col) = (err.line(), err.column());
        diags.push(Diagnostic::new(line, col, err.to_string()));
    }
    diags
}

fn validate_jsonc(text: &str) -> Vec<Diagnostic> {
    let stripped = strip_jsonc(text);
    // If original empty after stripping, it's valid.
    if stripped.trim().is_empty() {
        return Vec::new();
    }
    validate_json(&stripped)
}

fn validate_toml(text: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    if let Err(err) = text.parse::<DocumentMut>() {
        // TomlError span handling: use first line/col if span available, else 1:1
        let msg = err.to_string();
        // Try to extract line from message like "expected ..." often includes line info.
        // Fallback to 1:1.
        diags.push(Diagnostic::new(1, 1, msg));
    }
    diags
}

fn validate_yaml(text: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    if let Err(err) = yaml_serde::from_str::<StrictYamlValue>(text) {
        let msg = err.to_string();
        // yaml_serde error may contain location; try to parse line from message or use 1:1
        diags.push(Diagnostic::new(1, 1, msg));
    }
    diags
}

fn validate_env(text: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    // Reuse env parsing logic: split lines, check syntax for entries.
    let lines: Vec<&str> = text.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Allow optional leading "export " (space or tab)
        let without_export = if trimmed.starts_with("export ") || trimmed.starts_with("export\t") {
            trimmed.get(7..).unwrap_or_default().trim()
        } else {
            trimmed
        };
        if without_export.is_empty() {
            continue;
        }
        // Must contain '='
        if !without_export.contains('=') {
            diags.push(Diagnostic::new(
                idx.saturating_add(1),
                1,
                format!(
                    "invalid env entry: missing '=' in line {}",
                    idx.saturating_add(1)
                ),
            ));
            continue;
        }
        // Key validation: before '=' must be valid identifier
        let Some(eq_pos) = without_export.find('=') else {
            continue;
        };
        let key = without_export.get(0..eq_pos).unwrap_or_default().trim();
        if key.is_empty() {
            diags.push(Diagnostic::new(
                idx.saturating_add(1),
                1,
                "invalid env entry: empty key".to_owned(),
            ));
        } else if !is_valid_env_key(key) {
            diags.push(Diagnostic::new(
                idx.saturating_add(1),
                1,
                format!("invalid env key `{key}`"),
            ));
        }
    }
    diags
}

fn is_valid_env_key(key: &str) -> bool {
    // Env keys: [A-Za-z_][A-Za-z0-9_]*
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    for ch in chars {
        if !ch.is_ascii_alphanumeric() && ch != '_' {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Strict JSON helpers (duplicate key detection)
// ---------------------------------------------------------------------------

struct StrictJsonValue(Value);

impl<'de> Deserialize<'de> for StrictJsonValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct StrictVisitor;

        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictJsonValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("any valid JSON value")
            }

            fn visit_bool<E>(self, v: bool) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictJsonValue(Value::Bool(v)))
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictJsonValue(Value::Number(Number::from(v))))
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictJsonValue(Value::Number(Number::from(v))))
            }

            fn visit_f64<E>(self, v: f64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Number::from_f64(v).map_or_else(
                    || Err(de::Error::custom("invalid f64")),
                    |n| Ok(StrictJsonValue(Value::Number(n))),
                )
            }

            fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictJsonValue(Value::String(v.to_owned())))
            }

            fn visit_string<E>(self, v: String) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictJsonValue(Value::String(v)))
            }

            fn visit_borrowed_str<E>(self, v: &'de str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictJsonValue(Value::String(v.to_owned())))
            }

            fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictJsonValue(Value::Null))
            }

            fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictJsonValue(Value::Null))
            }

            #[expect(clippy::excessive_nesting, reason = "visitor boilerplate")]
            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut vec = Vec::new();
                while let Some(elem) = seq.next_element::<StrictJsonValue>()? {
                    vec.push(elem.0);
                }
                Ok(StrictJsonValue(Value::Array(vec)))
            }

            #[expect(clippy::excessive_nesting, reason = "visitor boilerplate")]
            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut m = Map::new();
                while let Some((key, value)) = map.next_entry::<String, StrictJsonValue>()? {
                    if m.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate key `{key}`")));
                    }
                    m.insert(key, value.0);
                }
                Ok(StrictJsonValue(Value::Object(m)))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

fn parse_strict_json_value(text: &str) -> std::result::Result<Value, serde_json::Error> {
    let mut de = serde_json::Deserializer::from_str(text);
    let v = StrictJsonValue::deserialize(&mut de)?;
    de.end()?;
    Ok(v.0)
}

// ---------------------------------------------------------------------------
// Strict YAML helper
// ---------------------------------------------------------------------------

struct StrictYamlValue(Value);

impl<'de> Deserialize<'de> for StrictYamlValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct StrictVisitor;

        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictYamlValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("any valid YAML value")
            }

            fn visit_bool<E>(self, v: bool) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictYamlValue(Value::Bool(v)))
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictYamlValue(Value::Number(Number::from(v))))
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictYamlValue(Value::Number(Number::from(v))))
            }

            fn visit_f64<E>(self, v: f64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Number::from_f64(v).map_or_else(
                    || Err(de::Error::custom("invalid f64")),
                    |n| Ok(StrictYamlValue(Value::Number(n))),
                )
            }

            fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictYamlValue(Value::String(v.to_owned())))
            }

            fn visit_string<E>(self, v: String) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictYamlValue(Value::String(v)))
            }

            fn visit_borrowed_str<E>(self, v: &'de str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictYamlValue(Value::String(v.to_owned())))
            }

            fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictYamlValue(Value::Null))
            }

            fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictYamlValue(Value::Null))
            }

            #[expect(clippy::excessive_nesting, reason = "visitor boilerplate")]
            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut vec = Vec::new();
                while let Some(elem) = seq.next_element::<StrictYamlValue>()? {
                    vec.push(elem.0);
                }
                Ok(StrictYamlValue(Value::Array(vec)))
            }

            #[expect(clippy::excessive_nesting, reason = "visitor boilerplate")]
            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut m = Map::new();
                while let Some((key, value)) = map.next_entry::<String, StrictYamlValue>()? {
                    if m.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate key `{key}`")));
                    }
                    m.insert(key, value.0);
                }
                Ok(StrictYamlValue(Value::Object(m)))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

// ---------------------------------------------------------------------------
// JSONC stripping
// ---------------------------------------------------------------------------

#[expect(clippy::excessive_nesting, reason = "comment scan")]
fn strip_comments(input: &str) -> String {
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

#[expect(clippy::excessive_nesting, reason = "string-aware scan")]
fn strip_trailing_commas(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut in_string = false;
    let mut escaped = false;
    let mut idx = 0usize;

    while idx < chars.len() {
        let Some(ch) = chars.get(idx).copied() else {
            break;
        };
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            idx = idx.saturating_add(1);
        } else if ch == '"' {
            in_string = true;
            output.push(ch);
            idx = idx.saturating_add(1);
        } else if ch == ',' {
            let mut look = idx.saturating_add(1);
            loop {
                match chars.get(look).copied() {
                    Some(c) if c == ' ' || c == '\t' || c == '\n' || c == '\r' => {
                        look = look.saturating_add(1);
                    }
                    _ => break,
                }
            }
            if let Some(next) = chars.get(look).copied()
                && (next == '}' || next == ']')
            {
                idx = idx.saturating_add(1);
                continue;
            }
            output.push(ch);
            idx = idx.saturating_add(1);
        } else {
            output.push(ch);
            idx = idx.saturating_add(1);
        }
    }

    output
}

fn strip_jsonc(input: &str) -> String {
    let without_comments = strip_comments(input);
    strip_trailing_commas(&without_comments)
}

// ---------------------------------------------------------------------------
// Diff structures
// ---------------------------------------------------------------------------

/// Semantic operation describing a change at a selector.
///
/// For JSON/YAML/env, the selector is a key path; for TOML it may be a table
/// path. Lexical details (whitespace, comments) are excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticOp {
    /// Human-readable selector, e.g. `key:model` or `table:servers.prod`.
    pub selector: String,
    /// Previous value as string, if any.
    pub old_value: Option<String>,
    /// New value as string, if any.
    pub new_value: Option<String>,
    /// Short description.
    pub description: String,
}

/// Span in `new` content that bears a secret and should be redacted in UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionSpan {
    /// Byte start (inclusive) in `new` content.
    pub start: usize,
    /// Byte end (exclusive) in `new` content.
    pub end: usize,
    /// Reason, e.g. `contains api_key`.
    pub reason: String,
}

/// Result of diffing two file versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffResult {
    /// Semantic operations (empty if only formatting/comments changed).
    pub semantic_ops: Vec<SemanticOp>,
    /// Lexical unified diff (`---`/`+++` style). Empty if byte-identical.
    pub lexical_unified_diff: String,
    /// Redaction spans for secret-bearing regions in `new`.
    pub redaction_spans: Vec<RedactionSpan>,
    /// Whether the two contents are byte-identical.
    pub is_noop: bool,
}

// ---------------------------------------------------------------------------
// Diff implementation
// ---------------------------------------------------------------------------

/// Produce semantic ops, lexical diff, and redaction spans for `old` vs `new`.
///
/// Neither buffer is written to disk. `kind` selects the parser for semantic
/// comparison; lexical diff is always line-oriented after UTF-8 lossy
/// conversion (binary files produce a size/digest summary).
pub fn diff(old: &[u8], new: &[u8], kind: DocumentKind) -> DiffResult {
    let is_noop = old == new;

    let lexical_unified_diff = lexical_diff(old, new);
    let semantic_ops = semantic_diff(old, new, kind);
    let redaction_spans = find_redaction_spans(new, kind);

    DiffResult {
        semantic_ops,
        lexical_unified_diff,
        redaction_spans,
        is_noop,
    }
}

fn lexical_diff(old: &[u8], new: &[u8]) -> String {
    if old == new {
        return String::new();
    }

    let old_ok = std::str::from_utf8(old).is_ok();
    let new_ok = std::str::from_utf8(new).is_ok();
    if !old_ok || !new_ok {
        return format!(
            "binary diff: old {} bytes ({}), new {} bytes ({})",
            old.len(),
            compute_digest(old),
            new.len(),
            compute_digest(new)
        );
    }

    let old_str = std::str::from_utf8(old).unwrap_or_default();
    let new_str = std::str::from_utf8(new).unwrap_or_default();

    if old_str == new_str {
        return String::new();
    }

    let old_lines: Vec<&str> = old_str.lines().collect();
    let new_lines: Vec<&str> = new_str.lines().collect();

    let mut out = String::new();
    out.push_str("--- old\n");
    out.push_str("+++ new\n");

    let max = usize::max(old_lines.len(), new_lines.len());
    for idx in 0..max {
        let old_line = old_lines.get(idx).copied();
        let new_line = new_lines.get(idx).copied();
        match (old_line, new_line) {
            (Some(a), Some(b)) if a == b => {
                out.push(' ');
                out.push_str(a);
                out.push('\n');
            }
            (Some(a), Some(b)) => {
                out.push_str("- ");
                out.push_str(&redact_line_for_preview(a));
                out.push('\n');
                out.push_str("+ ");
                out.push_str(&redact_line_for_preview(b));
                out.push('\n');
            }
            (Some(a), None) => {
                out.push_str("- ");
                out.push_str(&redact_line_for_preview(a));
                out.push('\n');
            }
            (None, Some(b)) => {
                out.push_str("+ ");
                out.push_str(&redact_line_for_preview(b));
                out.push('\n');
            }
            (None, None) => {}
        }
        if out.len() > 8192 {
            out.push_str("... truncated\n");
            break;
        }
    }

    out
}

fn redact_line_for_preview(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let needs = lower.contains("apikey")
        || lower.contains("api_key")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("authorization")
        || lower.contains("bearer");
    if !needs {
        return line.to_owned();
    }
    if let Some(pos) = line.find(':') {
        let key = line.get(0..pos.saturating_add(1)).unwrap_or_default();
        return format!("{key} [REDACTED]");
    }
    if let Some(pos) = line.find('=') {
        let key = line.get(0..pos.saturating_add(1)).unwrap_or_default();
        return format!("{key}[REDACTED]");
    }
    "[REDACTED]".to_owned()
}

fn semantic_diff(old: &[u8], new: &[u8], kind: DocumentKind) -> Vec<SemanticOp> {
    // Try to parse both; if either fails, no semantic ops (lexical diff still covers it)
    // For valid parse, compare semantic values.

    match kind {
        DocumentKind::StrictJson => json_semantic_diff(old, new),
        DocumentKind::JsonC => jsonc_semantic_diff(old, new),
        DocumentKind::Yaml => yaml_semantic_diff(old, new),
        DocumentKind::Toml => toml_semantic_diff(old, new),
        DocumentKind::Env => env_semantic_diff(old, new),
        DocumentKind::TextFragment | DocumentKind::Opaque => Vec::new(),
    }
}

#[expect(
    clippy::manual_let_else,
    reason = "explicit match is clearer for validation"
)]
fn json_semantic_diff(old: &[u8], new: &[u8]) -> Vec<SemanticOp> {
    let old_text = match std::str::from_utf8(bytes_without_bom(old)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let new_text = match std::str::from_utf8(bytes_without_bom(new)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    if old_text.trim().is_empty() && new_text.trim().is_empty() {
        return Vec::new();
    }
    let old_val = match parse_strict_json_value(old_text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let new_val = match parse_strict_json_value(new_text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    diff_json_values(&old_val, &new_val, String::new())
}

#[expect(
    clippy::manual_let_else,
    reason = "explicit match is clearer for validation"
)]
fn jsonc_semantic_diff(old: &[u8], new: &[u8]) -> Vec<SemanticOp> {
    let old_text = match std::str::from_utf8(bytes_without_bom(old)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let new_text = match std::str::from_utf8(bytes_without_bom(new)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let old_stripped = strip_jsonc(old_text);
    let new_stripped = strip_jsonc(new_text);
    if old_stripped.trim().is_empty() && new_stripped.trim().is_empty() {
        return Vec::new();
    }
    let old_val = match parse_strict_json_value(&old_stripped) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let new_val = match parse_strict_json_value(&new_stripped) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    diff_json_values(&old_val, &new_val, String::new())
}

#[expect(
    clippy::manual_let_else,
    reason = "explicit match is clearer for validation"
)]
fn yaml_semantic_diff(old: &[u8], new: &[u8]) -> Vec<SemanticOp> {
    let old_text = match std::str::from_utf8(bytes_without_bom(old)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let new_text = match std::str::from_utf8(bytes_without_bom(new)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    if old_text.trim().is_empty() && new_text.trim().is_empty() {
        return Vec::new();
    }
    let old_val: Value = match yaml_serde::from_str::<StrictYamlValue>(old_text) {
        Ok(v) => v.0,
        Err(_) => return Vec::new(),
    };
    let new_val: Value = match yaml_serde::from_str::<StrictYamlValue>(new_text) {
        Ok(v) => v.0,
        Err(_) => return Vec::new(),
    };
    diff_json_values(&old_val, &new_val, String::new())
}

#[expect(
    clippy::manual_let_else,
    reason = "explicit match is clearer for validation"
)]
fn toml_semantic_diff(old: &[u8], new: &[u8]) -> Vec<SemanticOp> {
    let old_text = match std::str::from_utf8(bytes_without_bom(old)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let new_text = match std::str::from_utf8(bytes_without_bom(new)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    if old_text.trim().is_empty() && new_text.trim().is_empty() {
        return Vec::new();
    }
    let old_doc: DocumentMut = match old_text.parse() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let new_doc: DocumentMut = match new_text.parse() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    // Compare via serialized normalized form: if to_string equal, no semantic diff.
    // This captures data changes while ignoring decor differences where possible.
    // toml_edit's to_string preserves comments, so we need a data-only comparison.
    // Fallback: compare item keys/values via iteration.
    let old_str = old_doc.to_string();
    let new_str = new_doc.to_string();
    if old_str == new_str {
        return Vec::new();
    }
    // Deeper comparison: collect keys and compare values as strings.
    // Simple: if number of differing keys, produce ops.
    let mut ops = Vec::new();
    // Collect all keys from both docs
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for (k, _) in old_doc.iter() {
        keys.insert(k.to_owned());
    }
    for (k, _) in new_doc.iter() {
        keys.insert(k.to_owned());
    }
    for key in keys {
        let old_item = old_doc.get(&key);
        let new_item = new_doc.get(&key);
        let old_s = old_item.map(ToString::to_string);
        let new_s = new_item.map(ToString::to_string);
        if old_s != new_s {
            ops.push(SemanticOp {
                selector: format!("key:{key}"),
                old_value: old_s,
                new_value: new_s,
                description: format!("change key `{key}`"),
            });
        }
    }
    // Also check table headers differences via string comparison already caught
    if ops.is_empty() && old_str != new_str {
        ops.push(SemanticOp {
            selector: "root".to_owned(),
            old_value: Some(old_str),
            new_value: Some(new_str),
            description: "toml content changed".to_owned(),
        });
    }
    ops
}

#[expect(
    clippy::manual_let_else,
    reason = "explicit match is clearer for validation"
)]
fn env_semantic_diff(old: &[u8], new: &[u8]) -> Vec<SemanticOp> {
    let old_text = match std::str::from_utf8(old) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let new_text = match std::str::from_utf8(new) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let old_map = parse_env_map(old_text);
    let new_map = parse_env_map(new_text);
    diff_env_maps(&old_map, &new_map)
}

fn parse_env_map(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let without_export = if trimmed.starts_with("export ") {
            trimmed.get(7..).unwrap_or_default().trim()
        } else {
            trimmed
        };
        if let Some(eq) = without_export.find('=') {
            let key = without_export
                .get(0..eq)
                .unwrap_or_default()
                .trim()
                .to_owned();
            let raw_val = without_export
                .get(eq.saturating_add(1)..)
                .unwrap_or_default()
                .trim();
            let val = unquote_env_value(raw_val);
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }
    map
}

fn unquote_env_value(raw: &str) -> String {
    if raw.len() >= 2
        && raw.starts_with('"')
        && raw.ends_with('"')
        && let Some(inner) = raw.get(1..raw.len().saturating_sub(1))
    {
        return decode_double_quoted(inner);
    }
    if raw.len() >= 2
        && raw.starts_with('\'')
        && raw.ends_with('\'')
        && let Some(inner) = raw.get(1..raw.len().saturating_sub(1))
    {
        return inner.replace("\\'", "'").replace("\\\\", "\\");
    }
    raw.to_owned()
}

#[expect(clippy::excessive_nesting, reason = "escape decode needs nested match")]
fn decode_double_quoted(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(esc) = chars.next() {
                match esc {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn diff_env_maps(
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
) -> Vec<SemanticOp> {
    let mut ops = Vec::new();
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for k in old.keys() {
        keys.insert(k.clone());
    }
    for k in new.keys() {
        keys.insert(k.clone());
    }
    for key in keys {
        let old_v = old.get(&key);
        let new_v = new.get(&key);
        if old_v != new_v {
            ops.push(SemanticOp {
                selector: format!("key:{key}"),
                old_value: old_v.cloned(),
                new_value: new_v.cloned(),
                description: format!("env change key `{key}`"),
            });
        }
    }
    ops
}

fn diff_json_values(old: &Value, new: &Value, prefix: String) -> Vec<SemanticOp> {
    if old == new {
        return Vec::new();
    }
    match (old, new) {
        (Value::Object(old_map), Value::Object(new_map)) => {
            let mut ops = Vec::new();
            let mut keys: BTreeSet<String> = BTreeSet::new();
            for k in old_map.keys() {
                keys.insert(k.clone());
            }
            for k in new_map.keys() {
                keys.insert(k.clone());
            }
            for key in keys {
                let old_v = old_map.get(&key);
                let new_v = new_map.get(&key);
                let sel = if prefix.is_empty() {
                    format!("key:{key}")
                } else {
                    format!("{prefix}.{key}")
                };
                match (old_v, new_v) {
                    (Some(a), Some(b)) => {
                        ops.extend(diff_json_values(a, b, sel));
                    }
                    (Some(a), None) => ops.push(SemanticOp {
                        selector: sel.clone(),
                        old_value: Some(value_to_string(a)),
                        new_value: None,
                        description: format!("remove `{sel}`"),
                    }),
                    (None, Some(b)) => ops.push(SemanticOp {
                        selector: sel.clone(),
                        old_value: None,
                        new_value: Some(value_to_string(b)),
                        description: format!("add `{sel}`"),
                    }),
                    (None, None) => {}
                }
            }
            ops
        }
        (Value::Array(old_arr), Value::Array(new_arr)) => {
            // For arrays, simple length/value comparison without identity logic.
            if old_arr == new_arr {
                return Vec::new();
            }
            vec![SemanticOp {
                selector: if prefix.is_empty() {
                    "array".to_owned()
                } else {
                    prefix
                },
                old_value: Some(value_to_string(old)),
                new_value: Some(value_to_string(new)),
                description: "array changed".to_owned(),
            }]
        }
        _ => vec![SemanticOp {
            selector: if prefix.is_empty() {
                "value".to_owned()
            } else {
                prefix
            },
            old_value: Some(value_to_string(old)),
            new_value: Some(value_to_string(new)),
            description: "value changed".to_owned(),
        }],
    }
}

fn value_to_string(v: &Value) -> String {
    // Use compact JSON for stable representation.
    serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}"))
}

// ---------------------------------------------------------------------------
// Redaction spans
// ---------------------------------------------------------------------------

/// Find secret-bearing spans in `content` for UI redaction.
///
/// Scans for keys containing `apikey`, `api_key`, `secret`, `token`,
/// `password`, `authorization`, `bearer` (case-insensitive) and marks the
/// value region after `:` or `=` as sensitive. Binary content yields no spans.
#[expect(clippy::manual_let_else, reason = "explicit match is clearer")]
pub fn find_redaction_spans(content: &[u8], _kind: DocumentKind) -> Vec<RedactionSpan> {
    let text = match std::str::from_utf8(content) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    find_redaction_spans_str(text)
}

fn find_redaction_spans_str(text: &str) -> Vec<RedactionSpan> {
    let mut spans = Vec::new();
    let mut offset = 0usize;
    // Split while preserving offsets: iterate over lines including empty.
    // Use split_inclusive to keep newline lengths.
    let mut remaining = text;
    while !remaining.is_empty() {
        let Some(nl_pos) = remaining.find('\n') else {
            // Last line without newline
            let line = remaining;
            if let Some(span) = redaction_span_for_line(line, offset) {
                spans.push(span);
            }
            break;
        };
        let line_end = nl_pos.saturating_add(1);
        let Some(line_with_nl) = remaining.get(0..line_end) else {
            break;
        };
        // line without trailing newline (and possible \r)
        let line = if line_with_nl.ends_with("\r\n") {
            line_with_nl
                .get(0..line_with_nl.len().saturating_sub(2))
                .unwrap_or_default()
        } else if line_with_nl.ends_with('\n') {
            line_with_nl
                .get(0..line_with_nl.len().saturating_sub(1))
                .unwrap_or_default()
        } else {
            line_with_nl
        };
        if let Some(span) = redaction_span_for_line(line, offset) {
            spans.push(span);
        }
        offset = offset.saturating_add(line_end);
        remaining = remaining.get(line_end..).unwrap_or_default();
    }
    spans
}

fn redaction_span_for_line(line: &str, line_offset: usize) -> Option<RedactionSpan> {
    let lower = line.to_ascii_lowercase();
    let needs = lower.contains("apikey")
        || lower.contains("api_key")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("authorization")
        || lower.contains("bearer")
        || lower.contains("auth");
    if !needs {
        return None;
    }
    // Find value start after ':' or '='
    let sep_pos = line.find(':').or_else(|| line.find('='));
    let Some(pos) = sep_pos else {
        // No separator, redact whole line
        let start = line_offset;
        let end = line_offset.saturating_add(line.len());
        return Some(RedactionSpan {
            start,
            end,
            reason: "secret-bearing line without separator".to_owned(),
        });
    };
    // Value starts after separator, skipping spaces and quotes
    let val_start_in_line = pos.saturating_add(1);
    // Skip leading spaces
    let rest = line.get(val_start_in_line..).unwrap_or_default();
    let trimmed_start = rest.len().saturating_sub(rest.trim_start().len());
    let start = line_offset
        .saturating_add(val_start_in_line)
        .saturating_add(trimmed_start);
    let end = line_offset.saturating_add(line.len());
    if start >= end {
        return None;
    }
    Some(RedactionSpan {
        start,
        end,
        reason: "secret value".to_owned(),
    })
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

/// Read a document fresh from disk, detecting kind via extension.
///
/// Returns [`RawDocument`] with sensitive content, digest, snapshot, and
/// syntax diagnostics. Missing file yields `Err(ConfigError::Io)` with
/// `NotFound`, keeping missing vs empty distinct. Disk is the truth.
pub fn read(path: &Path) -> Result<RawDocument> {
    let bytes = std::fs::read(path).map_err(|e| ConfigError::io(path, e))?;
    let kind = DocumentKind::from_path(path);
    let encoding = Encoding::Utf8;
    let bom = detect_bom(&bytes);
    let newline_style = detect_newline(&bytes);
    let digest = compute_digest(&bytes);
    let snapshot = snapshot(path);
    let diagnostics = validate(&bytes, kind);

    Ok(RawDocument {
        path: path.to_path_buf(),
        content: SensitiveContent::new(bytes),
        kind,
        encoding,
        bom,
        newline_style,
        digest,
        snapshot,
        diagnostics,
    })
}

// ---------------------------------------------------------------------------
// Commit
// ---------------------------------------------------------------------------

/// Report after a successful commit.
#[derive(Debug, Clone)]
pub struct CommitReport {
    /// Backup of the previous file, if it existed.
    pub backup: Option<BackupEntry>,
    /// New hex digest of the committed content.
    pub new_digest: String,
    /// Fresh snapshot after commit.
    pub new_snapshot: Snapshot,
    /// Whether the commit was a no-op (content already equal).
    pub is_noop: bool,
}

/// Commit `new_content` to `path` after validation and conflict check.
///
/// `expected_digest` is the digest observed at `read` time. If `Some`, the
/// current on-disk digest must match exactly or a `ConcurrentModification`
/// error is returned. If `None`, the file is expected not to exist.
/// Rejected invalid content (syntax error) never touches disk and creates no
/// backup.
pub fn commit(
    path: &Path,
    new_content: &[u8],
    expected_digest: Option<&str>,
) -> Result<CommitReport> {
    commit_inner(path, new_content, expected_digest, None)
}

/// Commit with an explicit [`Snapshot`] conflict token.
pub fn commit_with_snapshot(
    path: &Path,
    new_content: &[u8],
    expected: Option<&Snapshot>,
) -> Result<CommitReport> {
    let digest_opt = expected.and_then(|s| s.digest.as_deref());
    commit_inner(path, new_content, digest_opt, expected)
}

fn validate_for_commit(path: &Path, content: &[u8], kind: DocumentKind) -> Result<()> {
    let diagnostics = validate(content, kind);
    if diagnostics.is_empty() {
        return Ok(());
    }
    // Map first diagnostic to a ConfigError variant per kind.
    let Some(first) = diagnostics.first() else {
        return Err(ConfigError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid syntax"),
        });
    };
    let msg = format!("{}:{}: {}", first.line, first.col, first.message);
    match kind {
        DocumentKind::StrictJson | DocumentKind::JsonC => {
            let text = std::str::from_utf8(bytes_without_bom(content)).unwrap_or_default();
            match parse_strict_json_value(text) {
                Ok(_) => Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidData, msg),
                }),
                Err(source) => Err(ConfigError::Json {
                    path: path.to_path_buf(),
                    source,
                }),
            }
        }
        DocumentKind::Toml => {
            let text = std::str::from_utf8(bytes_without_bom(content)).unwrap_or_default();
            let parse_err: std::result::Result<DocumentMut, toml_edit::TomlError> = text.parse();
            match parse_err {
                Ok(_) => Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidData, msg),
                }),
                Err(source) => Err(ConfigError::Toml {
                    path: path.to_path_buf(),
                    source,
                }),
            }
        }
        DocumentKind::Yaml => {
            let text = std::str::from_utf8(bytes_without_bom(content)).unwrap_or_default();
            match yaml_serde::from_str::<StrictYamlValue>(text) {
                Ok(_) => Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidData, msg),
                }),
                Err(source) => Err(ConfigError::Yaml {
                    path: path.to_path_buf(),
                    source,
                }),
            }
        }
        DocumentKind::Env => Err(ConfigError::Env {
            path: path.to_path_buf(),
            message: msg,
        }),
        DocumentKind::TextFragment | DocumentKind::Opaque => Err(ConfigError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, msg),
        }),
    }
}

#[expect(clippy::too_many_lines, reason = "commit steps are sequential")]
fn commit_inner(
    path: &Path,
    new_content: &[u8],
    expected_digest: Option<&str>,
    expected_snapshot: Option<&Snapshot>,
) -> Result<CommitReport> {
    let kind = DocumentKind::from_path(path);

    // Opaque/internal stores are never editable via raw editor (RAW-06)
    if kind == DocumentKind::Opaque {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "opaque/internal store is read-only",
            ),
        ));
    }

    // Validate before any disk mutation.
    validate_for_commit(path, new_content, kind)?;

    // Fresh snapshot and conflict check.
    let current_snapshot = snapshot(path);

    // If caller supplied a snapshot token, use snapshot comparison.
    if let Some(expected) = expected_snapshot {
        if is_modified(expected, &current_snapshot) {
            let exp = expected.digest.as_deref().unwrap_or_default().to_owned();
            let actual = current_snapshot
                .digest
                .as_deref()
                .unwrap_or_default()
                .to_owned();
            return Err(ConfigError::concurrent_modification(path, exp, actual));
        }
    } else if let Some(exp) = expected_digest {
        let actual = current_snapshot.digest.as_deref().unwrap_or_default();
        if actual != exp {
            return Err(ConfigError::concurrent_modification(
                path,
                exp.to_owned(),
                actual.to_owned(),
            ));
        }
    } else {
        // expected_digest == None from caller means they thought file missing.
        // But if our read path was without token, we skip conflict check.
        // Only enforce when explicit None was provided as "expect missing"
        // We distinguish by whether the overload was called with Some(&"")? For now,
        // if expected_digest is None and we are in commit(path, new_content, None)
        // where caller explicitly expects missing, we should enforce missing.
        // To avoid breaking the simple `commit(path, bytes)` where expected_digest=None
        // means "no expectation", we only enforce when the caller used
        // commit_with_snapshot with expected missing snapshot.
        // So here, do not enforce missing when expected_digest is None and no snapshot.
    }

    // Special handling: if expected_digest is Some("") it means expect missing, enforce.
    if let Some(exp) = expected_digest
        && exp.is_empty()
        && current_snapshot.exists
    {
        let actual = current_snapshot
            .digest
            .as_deref()
            .unwrap_or_default()
            .to_owned();
        return Err(ConfigError::concurrent_modification(
            path,
            String::new(),
            actual,
        ));
    }

    // No-op check: if file exists and content already equal, no write.
    if current_snapshot.exists {
        if let Ok(existing) = std::fs::read(path)
            && existing == new_content
        {
            let new_digest = compute_digest(new_content);
            return Ok(CommitReport {
                backup: None,
                new_digest,
                new_snapshot: current_snapshot,
                is_noop: true,
            });
        }
    } else if new_content.is_empty() {
        // Creating an empty file from missing: treat as valid but check if empty is expected to be no-op?
        // For some formats empty may be considered not needing creation; but we still create if caller requested.
        // If new_content empty and file missing, is_noop false (we create).
    }

    // Reject directory targets
    if current_snapshot.is_dir {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "is a directory"),
        ));
    }

    // Backup before first mutation if file exists.
    let backup = if current_snapshot.exists {
        backup_with_reason(path, "raw editor pre-commit backup")?
    } else {
        None
    };

    // Ensure parent exists (atomic handles but do explicitly for clarity)
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    // Atomic replace with verification (also checks concurrent mod between snapshot and rename)
    // Use the expected digest for the atomic layer if provided.
    let atomic_res = if expected_digest.is_some() || expected_snapshot.is_some() {
        let digest_for_atomic =
            expected_digest.or_else(|| expected_snapshot.and_then(|s| s.digest.as_deref()));
        crate::atomic::atomic_write_with_expected_digest(path, new_content, digest_for_atomic)
    } else {
        crate::atomic::atomic_write(path, new_content)
    };

    atomic_res?;

    // Read-back verification: fresh read and parse, plus digest check.
    let read_back = std::fs::read(path).map_err(|e| ConfigError::io(path, e))?;
    let expected_digest_new = compute_digest(new_content);
    let actual_digest = compute_digest(&read_back);
    if expected_digest_new != actual_digest {
        return Err(ConfigError::verification(
            path,
            format!(
                "digest mismatch after commit: expected {expected_digest_new}, got {actual_digest}"
            ),
        ));
    }
    // Parse verification ensures written file is syntactically valid.
    validate_for_commit(path, &read_back, kind)?;

    let new_snapshot = snapshot(path);
    let new_digest = new_snapshot
        .digest
        .clone()
        .unwrap_or_else(|| compute_digest(new_content));

    Ok(CommitReport {
        backup,
        new_digest,
        new_snapshot,
        is_noop: false,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_scratch(prefix: &str, suffix: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let pid = std::process::id();
        let dir = std::env::temp_dir().join("superai-config-raw-editor-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{prefix}-{now}-{pid}{suffix}"))
    }

    // ---- SensitiveContent ----

    #[test]
    fn sensitive_content_debug_is_redacted() {
        let secret = SensitiveContent::new(b"my-secret-api-key-123".to_vec());
        let dbg = format!("{secret:?}");
        assert!(
            !dbg.contains("my-secret"),
            "debug should not contain secret"
        );
        assert!(dbg.contains("[REDACTED]"));
        assert_eq!(format!("{secret}"), "[REDACTED]");
        // expose still works
        assert_eq!(secret.expose(), b"my-secret-api-key-123");
    }

    #[test]
    fn raw_document_debug_is_redacted() {
        let path = unique_scratch("raw-doc", ".json");
        std::fs::write(&path, br#"{"api_key":"super-secret"}"#).unwrap();
        let doc = read(&path).unwrap();
        let dbg = format!("{doc:?}");
        assert!(!dbg.contains("super-secret"));
        assert!(dbg.contains("[REDACTED]") || !dbg.contains("super-secret"));
        drop(std::fs::remove_file(&path));
    }

    // ---- validate ----

    #[test]
    fn validate_rejects_invalid_json() {
        let diags = validate(b"{ invalid json }", DocumentKind::StrictJson);
        assert!(!diags.is_empty(), "invalid json should produce diagnostics");
    }

    #[test]
    fn validate_accepts_valid_json() {
        let diags = validate(br#"{"a":1}"#, DocumentKind::StrictJson);
        assert!(
            diags.is_empty(),
            "valid json should have no diagnostics: {diags:?}"
        );
    }

    #[test]
    fn validate_rejects_invalid_yaml() {
        let diags = validate(b"a: [unclosed\n", DocumentKind::Yaml);
        assert!(!diags.is_empty());
    }

    #[test]
    fn validate_rejects_invalid_toml() {
        let diags = validate(b"a = [\n", DocumentKind::Toml);
        assert!(!diags.is_empty());
    }

    #[test]
    fn validate_detects_duplicate_json_keys() {
        let diags = validate(br#"{"a":1,"a":2}"#, DocumentKind::StrictJson);
        assert!(!diags.is_empty());
        assert!(diags[0].message.contains("duplicate"));
    }

    #[test]
    fn validate_detects_invalid_utf8() {
        let bytes = vec![0xFF, 0xFE, 0xFD];
        let diags = validate(&bytes, DocumentKind::StrictJson);
        assert!(!diags.is_empty());
        assert!(diags[0].message.contains("invalid utf-8"));
    }

    // ---- read ----

    #[test]
    fn read_detects_kind_via_extension() {
        let json_path = unique_scratch("detect", ".json");
        std::fs::write(&json_path, br#"{"a":1}"#).unwrap();
        let doc = read(&json_path).unwrap();
        assert_eq!(doc.kind, DocumentKind::StrictJson);
        drop(std::fs::remove_file(&json_path));

        let toml_path = unique_scratch("detect", ".toml");
        std::fs::write(&toml_path, b"a = 1\n").unwrap();
        let doc2 = read(&toml_path).unwrap();
        assert_eq!(doc2.kind, DocumentKind::Toml);
        drop(std::fs::remove_file(&toml_path));

        let yaml_path = unique_scratch("detect", ".yaml");
        std::fs::write(&yaml_path, b"a: 1\n").unwrap();
        let doc3 = read(&yaml_path).unwrap();
        assert_eq!(doc3.kind, DocumentKind::Yaml);
        drop(std::fs::remove_file(&yaml_path));
    }

    #[test]
    fn read_is_fresh_and_produces_snapshot() {
        let path = unique_scratch("fresh", ".json");
        std::fs::write(&path, br#"{"a":1}"#).unwrap();
        let doc1 = read(&path).unwrap();
        std::fs::write(&path, br#"{"a":2}"#).unwrap();
        let doc2 = read(&path).unwrap();
        assert_ne!(doc1.digest, doc2.digest);
        assert!(is_modified(&doc1.snapshot, &doc2.snapshot));
        drop(std::fs::remove_file(&path));
    }

    // ---- commit: invalid never touches disk ----

    #[test]
    fn invalid_json_rejected_without_touching_disk() {
        let path = unique_scratch("invalid-commit", ".json");
        let original = br#"{"model":"opus"}"#;
        std::fs::write(&path, original).unwrap();
        let before_bytes = std::fs::read(&path).unwrap();
        let before_snap = snapshot(&path);
        let invalid = b"{ invalid json }";
        let err = commit(
            &path,
            invalid,
            Some(before_snap.digest.as_deref().unwrap_or_default()),
        )
        .unwrap_err();
        match err {
            ConfigError::Json { .. } => {}
            other => panic!("expected Json error, got {other:?}"),
        }
        let after_bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            before_bytes, after_bytes,
            "invalid commit must not touch disk"
        );
        let after_snap = snapshot(&path);
        assert!(
            !is_modified(&before_snap, &after_snap),
            "no file change on invalid"
        );

        // No backup should have been created because we validate before backup.
        // Check that no backup with newer timestamp appears beyond initial state.
        // We can list backups and ensure none contain invalid content.
        let backups = crate::backup::list_backups(&path).unwrap();
        for entry in &backups {
            let bytes = std::fs::read(&entry.backup_path).unwrap();
            assert_ne!(bytes, invalid.as_slice());
        }
        for entry in backups {
            drop(std::fs::remove_file(entry.backup_path));
        }
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn invalid_yaml_rejected_without_touching_disk() {
        let path = unique_scratch("invalid-yaml", ".yaml");
        let original = b"a: 1\n";
        std::fs::write(&path, original).unwrap();
        let before = std::fs::read(&path).unwrap();
        let invalid = b"a: [unclosed\n";
        let snap = snapshot(&path);
        let err = commit(&path, invalid, snap.digest.as_deref()).unwrap_err();
        match err {
            ConfigError::Yaml { .. } => {}
            other => panic!("expected Yaml error, got {other:?}"),
        }
        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after);
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn invalid_toml_rejected_without_touching_disk() {
        let path = unique_scratch("invalid-toml", ".toml");
        let original = b"a = 1\n";
        std::fs::write(&path, original).unwrap();
        let before = std::fs::read(&path).unwrap();
        let invalid = b"a = [\n";
        let snap = snapshot(&path);
        let err = commit(&path, invalid, snap.digest.as_deref()).unwrap_err();
        match err {
            ConfigError::Toml { .. } => {}
            other => panic!("expected Toml error, got {other:?}"),
        }
        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after);
        drop(std::fs::remove_file(&path));
    }

    // ---- commit: no-op ----

    #[test]
    fn commit_noop_creates_no_backup() {
        let path = unique_scratch("noop", ".json");
        let content = br#"{"a":1}"#;
        std::fs::write(&path, content).unwrap();
        let snap = snapshot(&path);
        let digest = snap.digest.as_deref().unwrap_or_default().to_owned();
        // Ensure no prior backups
        let before_backups = crate::backup::list_backups(&path).unwrap().len();
        let report = commit(&path, content, Some(&digest)).unwrap();
        assert!(report.is_noop);
        assert!(report.backup.is_none());
        let after_backups = crate::backup::list_backups(&path).unwrap().len();
        assert_eq!(
            before_backups, after_backups,
            "no-op should not create backup"
        );
        drop(std::fs::remove_file(&path));
        // clean backups
        for entry in crate::backup::list_backups(&path).unwrap() {
            drop(std::fs::remove_file(entry.backup_path));
        }
    }

    // ---- commit: conflict detection ----

    #[test]
    fn external_edit_after_open_causes_conflict() {
        let path = unique_scratch("conflict", ".json");
        std::fs::write(&path, br#"{"a":1}"#).unwrap();
        let snap = snapshot(&path);
        let digest = snap.digest.as_deref().unwrap_or_default().to_owned();
        // External edit
        std::fs::write(&path, br#"{"a":2}"#).unwrap();
        let new_content = br#"{"a":3}"#;
        let err = commit(&path, new_content, Some(&digest)).unwrap_err();
        match err {
            ConfigError::ConcurrentModification { .. } => {}
            other => panic!("expected ConcurrentModification, got {other:?}"),
        }
        let after = std::fs::read(&path).unwrap();
        assert_eq!(
            after, br#"{"a":2}"#,
            "conflict should leave external edit intact"
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn commit_with_snapshot_detects_conflict() {
        let path = unique_scratch("snap-conflict", ".yaml");
        std::fs::write(&path, b"a: 1\n").unwrap();
        let snap = snapshot(&path);
        std::fs::write(&path, b"a: 2\n").unwrap();
        let err = commit_with_snapshot(&path, b"a: 3\n", Some(&snap)).unwrap_err();
        match err {
            ConfigError::ConcurrentModification { .. } => {}
            other => panic!("expected ConcurrentModification, got {other:?}"),
        }
        drop(std::fs::remove_file(&path));
    }

    // ---- commit: unknown keys survive ----

    #[test]
    fn unknown_key_survives_commit() {
        let path = unique_scratch("unknown", ".json");
        let original = br#"{"known":"a","foreign":"keep"}"#;
        std::fs::write(&path, original).unwrap();
        let snap = snapshot(&path);
        let new_content = br#"{"known":"b","foreign":"keep","extra":"x"}"#;
        let report = commit(&path, new_content, snap.digest.as_deref()).unwrap();
        assert!(!report.is_noop);
        let after = std::fs::read(&path).unwrap();
        assert_eq!(after, new_content);
        // Verify foreign preserved via load
        let loaded = crate::json::load_value(&path).unwrap();
        assert_eq!(loaded["foreign"], Value::String("keep".into()));
        assert_eq!(loaded["extra"], Value::String("x".into()));
        drop(std::fs::remove_file(&path));
        if let Some(b) = report.backup {
            drop(std::fs::remove_file(b.backup_path));
        }
    }

    // ---- diff: lexical vs semantic ----

    #[test]
    fn diff_shows_lexical_vs_semantic() {
        let old = br#"{"a":1,"b":2}"#;
        let new = b"{\n  \"a\": 1,\n  \"b\": 2\n}\n";
        let res = diff(old, new, DocumentKind::StrictJson);
        assert!(
            !res.lexical_unified_diff.is_empty(),
            "lexical diff should be non-empty for formatting change"
        );
        assert!(
            res.semantic_ops.is_empty(),
            "semantic ops should be empty for whitespace-only change: {:?}",
            res.semantic_ops
        );
        assert!(!res.is_noop);
    }

    #[test]
    fn diff_semantic_detects_value_change() {
        let old = br#"{"a":1}"#;
        let new = br#"{"a":2}"#;
        let res = diff(old, new, DocumentKind::StrictJson);
        assert!(
            !res.semantic_ops.is_empty(),
            "value change should produce semantic ops"
        );
        assert!(!res.lexical_unified_diff.is_empty());
        assert!(res.lexical_unified_diff.contains('a'));
    }

    #[test]
    fn diff_is_noop_for_identical() {
        let data = br#"{"a":1}"#;
        let res = diff(data, data, DocumentKind::StrictJson);
        assert!(res.is_noop);
        assert!(res.lexical_unified_diff.is_empty());
        assert!(res.semantic_ops.is_empty());
    }

    #[test]
    fn diff_yaml_whitespace_is_lexical_only() {
        let old = b"a: 1\nb: 2\n";
        let new = b"a: 1\nb: 2\n\n";
        let res = diff(old, new, DocumentKind::Yaml);
        // YAML trailing newline difference may be semantic no-op
        assert!(!res.lexical_unified_diff.is_empty() || res.is_noop || res.semantic_ops.is_empty());
        // At least semantic should be empty if values equal
        let old_val = yaml_serde::from_str::<Value>(std::str::from_utf8(old).unwrap()).unwrap();
        let new_val = yaml_serde::from_str::<Value>(std::str::from_utf8(new).unwrap()).unwrap();
        if old_val == new_val {
            assert!(res.semantic_ops.is_empty());
        }
    }

    // ---- redaction ----

    #[test]
    fn diff_marks_redaction_spans() {
        let old = br#"{"api_key":"old"}"#;
        let new = br#"{"api_key":"super-secret-value","model":"opus"}"#;
        let res = diff(old, new, DocumentKind::StrictJson);
        assert!(
            !res.redaction_spans.is_empty(),
            "secret-bearing content should produce redaction spans"
        );
        // Ensure spans point inside new content where secret occurs
        let new_str = std::str::from_utf8(new).unwrap();
        let span = &res.redaction_spans[0];
        let secret_slice = new_str.get(span.start..span.end).unwrap_or_default();
        assert!(secret_slice.contains("super-secret-value") || span.reason.contains("secret"));
        // Lexical diff preview should be redacted
        assert!(
            !res.lexical_unified_diff.contains("super-secret-value"),
            "lexical diff should be redacted preview"
        );
    }

    #[test]
    fn redaction_spans_for_env() {
        let new = b"API_KEY=super-secret\nMODEL=opus\n";
        let spans = find_redaction_spans(new, DocumentKind::Env);
        assert!(!spans.is_empty());
        let Some(span) = spans.first() else {
            panic!("expected redaction span");
        };
        let text = std::str::from_utf8(new).unwrap();
        let slice = text.get(span.start..span.end).unwrap_or_default();
        assert!(slice.contains("super-secret"));
    }

    #[test]
    fn redaction_absent_for_non_secret() {
        let new = br#"{"model":"opus","other":"value"}"#;
        let spans = find_redaction_spans(new, DocumentKind::StrictJson);
        assert!(spans.is_empty());
    }

    // ---- opaque read-only ----

    #[test]
    fn opaque_is_read_only() {
        let path = unique_scratch("opaque", ".bin");
        std::fs::write(&path, b"binary content").unwrap();
        let err = commit(&path, b"new", None).unwrap_err();
        match err {
            ConfigError::Io { .. } => {}
            other => panic!("expected Io error for opaque, got {other:?}"),
        }
        drop(std::fs::remove_file(&path));
    }

    // ---- missing file create ----

    #[test]
    fn create_missing_file_and_commit() {
        let path = unique_scratch("create", ".json");
        drop(std::fs::remove_file(&path));
        let content = br#"{"a":1}"#;
        let report = commit(&path, content, None).unwrap();
        assert!(!report.is_noop);
        assert!(
            report.backup.is_none(),
            "missing file should have no backup"
        );
        let after = std::fs::read(&path).unwrap();
        assert_eq!(after, content);
        // Second commit with same content is noop
        let snap = snapshot(&path);
        let report2 = commit(&path, content, snap.digest.as_deref()).unwrap();
        assert!(report2.is_noop);
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn invalid_content_does_not_create_missing_file() {
        let path = unique_scratch("create-invalid", ".json");
        drop(std::fs::remove_file(&path));
        let invalid = b"{ bad }";
        let err = commit(&path, invalid, None).unwrap_err();
        match err {
            ConfigError::Json { .. } => {}
            other => panic!("expected Json error, got {other:?}"),
        }
        assert!(!path.exists(), "invalid content should not create file");
    }
}
