//! Env files — `KEY=value`, `export KEY=value`, quoting, comments, duplicates.
//!
//! Crate research (DOC-07):
//! - `dotenvy` 0.15.7 exists on crates.io, MIT OR Apache-2.0, maintained fork of
//!   `dotenv`, verifies with `cargo search dotenvy` and `cargo info dotenvy`.
//!   It parses `KEY=value` and `export KEY=value`, supports single/double/unquoted
//!   values, handles comments and blank lines, but **does not preserve** comments,
//!   blank lines, export prefix, quoting style, spacing, or duplicate entries on
//!   write — it is a loader, not a lossless editor.
//! - `dotenv` 0.15.0 and `const-dotenvy` also exist but have the same limitation.
//! - For superai's requirement to preserve comments, blank lines, export prefix,
//!   quoting, spacing, and duplicate-key policy, a custom line-preserving parser
//!   is required (similar to `toml_edit` for TOML). No existing crate offers
//!   lossless round-tripping with `export` and duplicate preservation at the
//!   level required, so this module implements its own parser.
//!
//! Preservation contract (DOC-07):
//! - Comments (`# ...`), blank lines, `export` prefix, quoting style
//!   (`'`, `"`, or unquoted), spacing around `=`, and newline style (LF vs CRLF)
//!   are preserved where untouched.
//! - Duplicate keys are preserved; the effective value is the last occurrence.
//!   Edits update the **last** occurrence and never silently deduplicate.
//! - Duplicate policy is adapter-declared: this module implements
//!   "edit effective last value" and preserves earlier duplicates. Rejecting
//!   ambiguity or using a dedicated generated file is left to the adapter.
//! - CRLF inputs are preserved on edit (detected from raw bytes).
//! - Single/double quotes and escapes are handled (`\"`, `\\`, `\n`, `\r`, `\t` in
//!   double quotes; `\'`, `\\` in single quotes; `\#` in unquoted).

use std::collections::BTreeMap;
use std::path::Path;

use crate::backup::backup;
use crate::error::{ConfigError, Result};

/// Quoting style for a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quoting {
    /// No quotes.
    Unquoted,
    /// Single quotes `'...'`.
    Single,
    /// Double quotes `"..."`.
    Double,
}

/// A parsed line with original raw text and value range for in-place edits.
#[derive(Debug, Clone)]
struct ParsedLine {
    /// Original raw line without trailing `\r`/`\n`.
    raw: String,
    /// Kind of line.
    kind: LineKind,
}

#[derive(Debug, Clone)]
enum LineKind {
    /// Blank line (only whitespace).
    Blank,
    /// Comment line (first non-space char is `#`).
    Comment,
    /// Entry with key/value.
    Entry(EntryMeta),
}

#[derive(Debug, Clone)]
struct EntryMeta {
    /// Key name.
    key: String,
    /// Decoded value.
    value: String,
    /// Quoting style.
    quoting: Quoting,
    /// Whether line had `export` prefix.
    export: bool,
    /// Byte start of value token (including opening quote if any) in `raw`.
    value_start: usize,
    /// Byte end of value token (after closing quote if any) in `raw`.
    value_end: usize,
}

/// Detect newline style from raw bytes: CRLF if any `\r\n` occurs, else LF.
fn detect_newline(bytes: &[u8]) -> &'static str {
    let has_crlf = bytes
        .windows(2)
        .any(|w| w.first().copied() == Some(b'\r') && w.get(1).copied() == Some(b'\n'));
    if has_crlf { "\r\n" } else { "\n" }
}

/// Strip a leading UTF-8 BOM if present.
fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{FEFF}').unwrap_or(s)
}

/// Whether a value requires quoting when written unquoted.
fn requires_quotes(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    // Leading/trailing whitespace requires quotes.
    if value.chars().next().is_some_and(char::is_whitespace)
        || value.chars().last().is_some_and(char::is_whitespace)
    {
        return true;
    }
    // Characters that would be ambiguous unquoted.
    for c in value.chars() {
        if c == '#' || c == '"' || c == '\'' || c == '=' || c == '\n' || c == '\r' || c == '\t' {
            return true;
        }
        if c.is_whitespace() && c != ' ' {
            return true;
        }
    }
    // If value contains spaces, it needs quotes to preserve them.
    if value.contains(' ') {
        return true;
    }
    false
}

fn escape_double(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

fn escape_single(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            _ => out.push(c),
        }
    }
    out
}

fn format_double(value: &str) -> String {
    let mut s = String::with_capacity(value.len() + 2);
    s.push('"');
    s.push_str(&escape_double(value));
    s.push('"');
    s
}

fn format_single(value: &str) -> String {
    let mut s = String::with_capacity(value.len() + 2);
    s.push('\'');
    s.push_str(&escape_single(value));
    s.push('\'');
    s
}

fn format_value_for_entry(value: &str, original: Quoting) -> String {
    match original {
        Quoting::Double => format_double(value),
        Quoting::Single => {
            if value.contains('\'') {
                format_double(value)
            } else {
                format_single(value)
            }
        }
        Quoting::Unquoted => {
            if requires_quotes(value) {
                format_double(value)
            } else {
                value.to_owned()
            }
        }
    }
}

fn format_value_normalized(value: &str) -> String {
    if requires_quotes(value) {
        format_double(value)
    } else {
        value.to_owned()
    }
}

/// Decode a double-quoted inner value (without surrounding quotes).
#[expect(
    clippy::excessive_nesting,
    reason = "escape decoding needs nested match"
)]
fn decode_double(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(esc) = chars.next() {
                match esc {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '"' => out.push('"'),
                    '\'' => out.push('\''),
                    '\\' => out.push('\\'),
                    '$' => out.push('$'),
                    '`' => out.push('`'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Decode a single-quoted inner value.
#[expect(
    clippy::excessive_nesting,
    reason = "escape decoding needs nested match"
)]
fn decode_single(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(esc) = chars.next() {
                match esc {
                    '\'' => out.push('\''),
                    '\\' => out.push('\\'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse a single raw line (without trailing newline) into a `ParsedLine`.
///
/// Returns an error message if the line is neither blank, comment, nor a
/// valid entry. The byte offsets `value_start`/`value_end` are valid char
/// boundaries for slicing `raw`.
#[expect(
    clippy::too_many_lines,
    reason = "line parsing requires sequential validation steps"
)]
#[expect(clippy::excessive_nesting, reason = "env parsing needs nested checks")]
fn parse_line(raw: &str) -> std::result::Result<ParsedLine, String> {
    // Blank
    if raw.trim().is_empty() {
        return Ok(ParsedLine {
            raw: raw.to_owned(),
            kind: LineKind::Blank,
        });
    }

    // Find first non-whitespace byte index
    let first_non_ws = raw
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map_or(0, |(idx, _)| idx);

    // Comment line
    if let Some(slice) = raw.get(first_non_ws..)
        && slice.starts_with('#')
    {
        return Ok(ParsedLine {
            raw: raw.to_owned(),
            kind: LineKind::Comment,
        });
    }

    // Entry: find '='
    let eq_pos = raw.find('=');
    let Some(eq_idx) = eq_pos else {
        return Err(format!("invalid env line (no '='): {raw}"));
    };

    // Split left/right
    let left = raw.get(0..eq_idx).unwrap_or_default();
    let right = raw.get(eq_idx + 1..).unwrap_or_default();

    // Parse left for export and key
    let left_trimmed = left.trim();
    let mut export = false;
    let key: String;
    if left_trimmed.starts_with("export") {
        let after_export = left_trimmed.get(6..).unwrap_or_default();
        // "export" must be followed by whitespace or be the whole left part? But left is before '=', so after_export should be whitespace + key or empty? Actually export prefix is "export " then key.
        // left_trimmed is like "export FOO" or "export   FOO" or "FOO"
        if after_export.is_empty() {
            return Err(format!("invalid env line (export without key): {raw}"));
        }
        if after_export.chars().next().is_some_and(char::is_whitespace) {
            export = true;
            key = after_export.trim().to_owned();
            if key.is_empty() {
                return Err(format!("invalid env line (export without key): {raw}"));
            }
        } else {
            // "exportFOO" is not export prefix, treat as key
            key = left_trimmed.to_owned();
        }
    } else {
        key = left_trimmed.to_owned();
    }

    if key.is_empty() {
        return Err(format!("invalid env line (empty key): {raw}"));
    }
    // Validate key chars: allow alphanumeric, underscore, dot? Be permissive but reject spaces/#
    if key.contains(' ')
        || key.contains('\t')
        || key.contains('#')
        || key.contains('"')
        || key.contains('\'')
    {
        return Err(format!("invalid env key `{key}`"));
    }
    // Key must not be empty and should start with alphabetic or underscore? Be permissive.

    // Parse right for value and trailing comment
    // Find value_start: first non-whitespace in right
    let right_ws_len = right
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map_or(right.len(), |(idx, _)| idx);

    // value_start byte offset in raw = eq_idx + 1 + right_ws_len
    let value_start = eq_idx + 1 + right_ws_len;
    if value_start > raw.len() {
        // No value, empty
        let meta = EntryMeta {
            key,
            value: String::new(),
            quoting: Quoting::Unquoted,
            export,
            value_start,
            value_end: value_start,
        };
        return Ok(ParsedLine {
            raw: raw.to_owned(),
            kind: LineKind::Entry(meta),
        });
    }

    let value_part = raw.get(value_start..).unwrap_or_default();
    if value_part.is_empty() {
        let meta = EntryMeta {
            key,
            value: String::new(),
            quoting: Quoting::Unquoted,
            export,
            value_start,
            value_end: value_start,
        };
        return Ok(ParsedLine {
            raw: raw.to_owned(),
            kind: LineKind::Entry(meta),
        });
    }

    let first_char = value_part.chars().next().unwrap_or('\0');
    let (decoded, quoting, value_end) = if first_char == '"' {
        // Double quoted
        let mut end_idx: Option<usize> = None;
        let mut escaped = false;
        for (i, c) in value_part.char_indices().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if c == '\\' {
                escaped = true;
                continue;
            }
            if c == '"' {
                end_idx = Some(i);
                break;
            }
        }
        if let Some(end_offset) = end_idx {
            // end_offset is byte index of closing quote within value_part
            let inner = value_part.get(1..end_offset).unwrap_or_default();
            let decoded = decode_double(inner);
            // value_end in raw = value_start + end_offset + 1 (include closing quote)
            let closing_quote_len = '"'.len_utf8();
            let ve = value_start + end_offset + closing_quote_len;
            (decoded, Quoting::Double, ve)
        } else {
            return Err(format!("unterminated double quote in line: {raw}"));
        }
    } else if first_char == '\'' {
        // Single quoted
        let mut end_idx: Option<usize> = None;
        let mut escaped = false;
        for (i, c) in value_part.char_indices().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if c == '\\' {
                escaped = true;
                continue;
            }
            if c == '\'' {
                end_idx = Some(i);
                break;
            }
        }
        if let Some(end_offset) = end_idx {
            let inner = value_part.get(1..end_offset).unwrap_or_default();
            let decoded = decode_single(inner);
            let ve = value_start + end_offset + '\''.len_utf8();
            (decoded, Quoting::Single, ve)
        } else {
            return Err(format!("unterminated single quote in line: {raw}"));
        }
    } else {
        // Unquoted: value runs until '#' that is not escaped, or end (but # inside value if escaped as \#)
        // Scan value_part char_indices to find unescaped '#'
        let mut end_offset = value_part.len();
        let mut escaped = false;
        for (i, c) in value_part.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if c == '\\' {
                escaped = true;
                continue;
            }
            if c == '#' {
                end_offset = i;
                break;
            }
        }
        let raw_value = value_part.get(0..end_offset).unwrap_or_default();
        // Trim trailing whitespace from raw_value for decoded value (unquoted values are trimmed)
        let trimmed_end = raw_value.trim_end();
        // Need to handle escaped chars in unquoted: decode \# -> #, \\ -> \, etc? For simplicity handle \# and \\ and \= \"
        let mut decoded = String::with_capacity(trimmed_end.len());
        let mut chars = trimmed_end.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(n) = chars.next() {
                    match n {
                        '#' => decoded.push('#'),
                        '\\' => decoded.push('\\'),
                        '"' => decoded.push('"'),
                        '\'' => decoded.push('\''),
                        'n' => decoded.push('\n'),
                        'r' => decoded.push('\r'),
                        't' => decoded.push('\t'),
                        other => {
                            decoded.push('\\');
                            decoded.push(other);
                        }
                    }
                } else {
                    decoded.push('\\');
                }
            } else {
                decoded.push(c);
            }
        }
        // Value end is start + trimmed_end.len() (byte length of trimmed part) but need byte index for original raw_value's trimmed portion.
        // Compute byte length of trimmed raw_value: find where trimmed_end ends in raw_value
        let trimmed_byte_len = trimmed_end.len();
        let ve = value_start + trimmed_byte_len;
        (decoded, Quoting::Unquoted, ve)
    };

    let meta = EntryMeta {
        key,
        value: decoded,
        quoting,
        export,
        value_start,
        value_end,
    };

    Ok(ParsedLine {
        raw: raw.to_owned(),
        kind: LineKind::Entry(meta),
    })
}

/// Parse full text into lines and effective map.
///
/// Returns the lines in order and a map of effective (last) values.
#[expect(
    clippy::excessive_nesting,
    reason = "line splitting with comment handling"
)]
fn parse_env_text(
    text: &str,
) -> std::result::Result<(Vec<ParsedLine>, BTreeMap<String, String>), String> {
    let text = strip_bom(text);
    // Split on \n, but handle \r\n by trimming \r from each line's end in raw
    // We keep raw without \r or \n, but detect newline style separately.
    let mut lines = Vec::new();
    let mut map = BTreeMap::new();

    // Use split_inclusive to keep track, but simpler split on '\n'
    // Preserve CRLF handling: raw lines may end with '\r' if input was CRLF and we split on '\n'
    let mut start = 0usize;
    let bytes = text.as_bytes();
    while start <= text.len() {
        let Some(next_nl) = text.get(start..).and_then(|s| s.find('\n')) else {
            // Last segment
            let segment = text.get(start..).unwrap_or_default();
            // Remove trailing \r if present (part of CRLF, but last line may not have newline)
            let raw_line = if segment.ends_with('\r') {
                segment.get(0..segment.len() - 1).unwrap_or_default()
            } else {
                segment
            };
            // Only push if not the artificial empty after final newline? We need to handle trailing newline.
            // If text ends with newline, the last segment after split will be empty; we should not treat as extra blank line unless there was content.
            // Example: "a=1\n" split gives ["a=1", ""]; we want one line, not two.
            // So if start == text.len() (empty remainder after final newline), skip.
            if !(segment.is_empty() && start == text.len()) {
                // But if original text ended with newline, we already accounted for line before; the empty after should be ignored.
                // However if text is empty, we want zero lines.
                if !(raw_line.is_empty() && segment.is_empty() && text.ends_with('\n')) {
                    let parsed = parse_line(raw_line)?;
                    if let LineKind::Entry(ref meta) = parsed.kind {
                        map.insert(meta.key.clone(), meta.value.clone());
                    }
                    lines.push(parsed);
                }
            }
            break;
        };
        let nl_idx = start + next_nl;
        let segment = text.get(start..nl_idx).unwrap_or_default();
        let raw_line = if segment.ends_with('\r') {
            segment.get(0..segment.len() - 1).unwrap_or_default()
        } else {
            segment
        };
        let _ = bytes; // keep for lint
        let parsed = parse_line(raw_line)?;
        if let LineKind::Entry(ref meta) = parsed.kind {
            map.insert(meta.key.clone(), meta.value.clone());
        }
        lines.push(parsed);
        start = nl_idx + 1;
        // If text ends with newline, loop will handle final empty correctly via above logic
        if start == text.len() && text.ends_with('\n') {
            break;
        }
        if start > text.len() {
            break;
        }
    }

    Ok((lines, map))
}

// ---------------------------------------------------------------------------
// Public API — env files (DOC-07)
// ---------------------------------------------------------------------------

/// Read an env file fresh from disk. A missing file reads as an empty map.
///
/// Supports `KEY=value` and `export KEY=value`, single/double/unquoted values,
/// comments (`#`), blank lines, and duplicate keys (last wins). Each call
/// reads the file fresh (disk is the truth). Blank and comment lines are
/// validated but not included in the returned map.
pub fn load(path: &Path) -> Result<BTreeMap<String, String>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(ConfigError::io(path, e)),
    };
    let text = String::from_utf8(bytes).map_err(|e| ConfigError::Env {
        path: path.to_path_buf(),
        message: format!("invalid utf-8: {e}"),
    })?;
    if text.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let (_, map) = parse_env_text(&text).map_err(|message| ConfigError::Env {
        path: path.to_path_buf(),
        message,
    })?;
    Ok(map)
}

/// Back up, then write `vars` to `path`, creating parent directories as needed.
///
/// The file is written as normalized `KEY=value` lines with quoting only when
/// required (double quotes). No `export` prefix, comments, or blank lines are
/// emitted — they are preserved only via [`edit`] on existing files. The output
/// uses LF newlines and a trailing newline.
pub fn store(path: &Path, vars: &BTreeMap<String, String>) -> Result<()> {
    backup(path)?;

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    let mut text = String::new();
    for (k, v) in vars {
        let formatted = format_value_normalized(v);
        text.push_str(k);
        text.push('=');
        text.push_str(&formatted);
        text.push('\n');
    }

    crate::atomic::atomic_write(path, text.as_bytes())
}

/// Read fresh, apply `edit`, write back only if the effective map changed.
///
/// Preserves comments, blank lines, `export` prefix, quoting style, spacing,
/// and duplicate entries where untouched. For changed keys, the **last**
/// occurrence is updated in place (preserving its prefix/spacing/trailing
/// comment). New keys are appended. Removed keys have **all** occurrences
/// deleted. No silent deduplication occurs — earlier duplicates remain unless
/// explicitly removed. CRLF vs LF is preserved from the original file.
///
/// Nothing is cached between calls. For a no-op (effective map unchanged) no
/// write and no backup are performed, so the file's byte identity is preserved.
#[expect(
    clippy::too_many_lines,
    reason = "edit reconciles duplicates and preserves formatting"
)]
#[expect(clippy::excessive_nesting, reason = "reconciling edits needs nesting")]
pub fn edit<F>(path: &Path, edit_fn: F) -> Result<()>
where
    F: FnOnce(&mut BTreeMap<String, String>),
{
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut map = BTreeMap::new();
            edit_fn(&mut map);
            if map.is_empty() {
                return Ok(());
            }
            return store(path, &map);
        }
        Err(e) => return Err(ConfigError::io(path, e)),
    };

    let newline = detect_newline(&bytes);
    let text = String::from_utf8(bytes).map_err(|e| ConfigError::Env {
        path: path.to_path_buf(),
        message: format!("invalid utf-8: {e}"),
    })?;

    // Empty file case
    if text.trim().is_empty() {
        let mut map = BTreeMap::new();
        edit_fn(&mut map);
        if map.is_empty() {
            return Ok(());
        }
        return store(path, &map);
    }

    let (mut lines, mut map) = parse_env_text(&text).map_err(|message| ConfigError::Env {
        path: path.to_path_buf(),
        message,
    })?;

    let original_map = map.clone();
    edit_fn(&mut map);
    if map == original_map {
        return Ok(());
    }

    // Reconcile lines with new map
    // 1. Handle removed keys: delete all occurrences
    let mut removed_keys: Vec<String> = Vec::new();
    for k in original_map.keys() {
        if !map.contains_key(k) {
            removed_keys.push(k.clone());
        }
    }
    if !removed_keys.is_empty() {
        lines.retain(|line| match &line.kind {
            LineKind::Entry(meta) => !removed_keys.contains(&meta.key),
            _ => true,
        });
    }

    // 2. Handle changed or new keys
    for (key, new_value) in &map {
        let Some(old_value) = original_map.get(key) else {
            // New key: append
            let formatted = format_value_normalized(new_value);
            let raw = format!("{key}={formatted}");
            lines.push(ParsedLine {
                raw,
                kind: LineKind::Entry(EntryMeta {
                    key: key.clone(),
                    value: new_value.clone(),
                    quoting: if requires_quotes(new_value) {
                        Quoting::Double
                    } else {
                        Quoting::Unquoted
                    },
                    export: false,
                    value_start: key.len() + 1,
                    value_end: key.len() + 1 + formatted.len(),
                }),
            });
            continue;
        };
        if old_value == new_value {
            continue;
        }
        // Changed: update last occurrence
        let mut last_idx: Option<usize> = None;
        for (idx, line) in lines.iter().enumerate().rev() {
            if let LineKind::Entry(meta) = &line.kind
                && meta.key == *key
            {
                last_idx = Some(idx);
                break;
            }
        }
        if let Some(idx) = last_idx {
            let old_line = lines.get(idx).cloned().unwrap_or_else(|| ParsedLine {
                raw: String::new(),
                kind: LineKind::Blank,
            });
            if let LineKind::Entry(meta) = old_line.kind {
                let new_formatted = format_value_for_entry(new_value, meta.quoting);
                let prefix = old_line.raw.get(0..meta.value_start).unwrap_or_default();
                let suffix = old_line.raw.get(meta.value_end..).unwrap_or_default();
                let new_raw = format!("{prefix}{new_formatted}{suffix}");
                let new_start = prefix.len();
                let new_end = new_start + new_formatted.len();
                let new_meta = EntryMeta {
                    key: key.clone(),
                    value: new_value.clone(),
                    quoting: meta.quoting,
                    export: meta.export,
                    value_start: new_start,
                    value_end: new_end,
                };
                if let Some(slot) = lines.get_mut(idx) {
                    *slot = ParsedLine {
                        raw: new_raw,
                        kind: LineKind::Entry(new_meta),
                    };
                }
            }
        } else {
            // Should not happen (key existed before but no line), append
            let formatted = format_value_normalized(new_value);
            let raw = format!("{key}={formatted}");
            lines.push(ParsedLine {
                raw,
                kind: LineKind::Entry(EntryMeta {
                    key: key.clone(),
                    value: new_value.clone(),
                    quoting: if requires_quotes(new_value) {
                        Quoting::Double
                    } else {
                        Quoting::Unquoted
                    },
                    export: false,
                    value_start: key.len() + 1,
                    value_end: key.len() + 1 + formatted.len(),
                }),
            });
        }
    }

    // Serialize lines back
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&line.raw);
        if i + 1 < lines.len() {
            out.push_str(newline);
        } else {
            // Ensure trailing newline (like other codecs)
            out.push_str(newline);
        }
    }

    // If original file ended without newline, we still add one (normalized trailing newline)
    // This matches `store` behaviour and is acceptable.

    backup(path)?;

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    crate::atomic::atomic_write(path, out.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = crate::test_util::temp_dir_unique("config-env");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn parses_key_value_and_export() {
        let text = "FOO=bar\nexport BAZ=qux\n";
        let (_, map) = parse_env_text(text).unwrap();
        assert_eq!(map.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(map.get("BAZ").map(String::as_str), Some("qux"));
    }

    #[test]
    fn parses_single_double_unquoted() {
        let (_, map) = parse_env_text("A='single'\nB=\"double\"\nC=unquoted\n").unwrap();
        assert_eq!(map["A"], "single");
        assert_eq!(map["B"], "double");
        assert_eq!(map["C"], "unquoted");
    }

    #[test]
    fn preserves_comments_and_blank_lines_on_edit() {
        let path = scratch("preserve.env");
        let original = "# header comment\nFOO=bar\n\n# middle\nBAZ=qux\n";
        std::fs::write(&path, original).unwrap();
        edit(&path, |m| {
            m.insert("FOO".into(), "new".into());
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# header comment"));
        assert!(after.contains("# middle"));
        assert!(after.contains("FOO=new"));
        // Blank line preserved (two newlines in a row)
        assert!(after.contains("\n\n"));
        assert!(after.contains("BAZ=qux"));
    }

    #[test]
    fn handles_escaped_characters_in_double_quotes() {
        let (_, map) = parse_env_text("A=\"a \\\"quote\\\" and \\\\ backslash\"\n").unwrap();
        assert_eq!(map["A"], "a \"quote\" and \\ backslash");
        let (_, map2) = parse_env_text("B=\"line\\nbreak\"\n").unwrap();
        assert_eq!(map2["B"], "line\nbreak");
    }

    #[test]
    fn handles_comments_after_values_and_inside_quotes() {
        let text = "A=\"value # not comment\" # real comment\nB=unquoted # comment\nC='single # not comment' # comment\n";
        let (lines, map) = parse_env_text(text).unwrap();
        assert_eq!(map["A"], "value # not comment");
        assert_eq!(map["B"], "unquoted");
        assert_eq!(map["C"], "single # not comment");
        // Ensure trailing comment is preserved in raw
        let first = &lines[0];
        assert!(first.raw.contains("# real comment") || first.raw.contains("real comment"));
    }

    #[test]
    fn handles_duplicate_keys_last_wins_and_edit_updates_last() {
        let path = scratch("dup.env");
        std::fs::write(&path, "FOO=first\nFOO=second\n").unwrap();
        let map = load(&path).unwrap();
        assert_eq!(map["FOO"], "second");

        edit(&path, |m| {
            m.insert("FOO".into(), "third".into());
        })
        .unwrap();
        let after_text = std::fs::read_to_string(&path).unwrap();
        // Should preserve both lines but last updated
        let lines: Vec<&str> = after_text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("first"));
        assert!(lines[1].contains("third"));
        // Effective value is third
        let map2 = load(&path).unwrap();
        assert_eq!(map2["FOO"], "third");

        // Check no silent dedup: still two FOO lines
        let foo_count = after_text.matches("FOO=").count();
        assert_eq!(foo_count, 2);
    }

    #[test]
    fn never_silently_dedup_on_no_change() {
        let path = scratch("dedup.env");
        std::fs::write(&path, "FOO=first\nFOO=second\n").unwrap();
        edit(&path, |_| {}).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        let count = after.matches("FOO=").count();
        assert_eq!(count, 2, "no-op must not dedup");
    }

    #[test]
    fn handles_crlf() {
        let path = scratch("crlf.env");
        std::fs::write(&path, "FOO=bar\r\nBAZ=qux\r\n").unwrap();
        let map = load(&path).unwrap();
        assert_eq!(map["FOO"], "bar");
        assert_eq!(map["BAZ"], "qux");
        edit(&path, |m| {
            m.insert("FOO".into(), "new".into());
        })
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes.windows(2).any(|w| w == b"\r\n"),
            "should preserve CRLF"
        );
        let after = String::from_utf8(bytes).unwrap();
        assert!(after.contains("FOO=new"));
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let path = scratch("absent.env");
        drop(std::fs::remove_file(&path));
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn no_op_preserves_byte_identity() {
        let path = scratch("noop.env");
        let original = "FOO=bar\n# comment\nBAZ=qux\n";
        std::fs::write(&path, original).unwrap();
        let before = std::fs::read(&path).unwrap();
        edit(&path, |_| {}).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after);

        let backups: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("noop.env.bak."))
            .collect();
        assert!(backups.is_empty(), "no-op should not create backup");
    }

    #[test]
    fn store_writes_normalized() {
        let path = scratch("store.env");
        drop(std::fs::remove_file(&path));
        let mut map = BTreeMap::new();
        map.insert("A".into(), "hello world".into());
        map.insert("B".into(), "simple".into());
        store(&path, &map).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        // hello world requires quotes
        assert!(after.contains("A=\"hello world\"") || after.contains("A='hello world'"));
        assert!(after.contains("B=simple"));
    }

    #[test]
    fn preserves_export_prefix_and_quoting() {
        let path = scratch("export.env");
        std::fs::write(&path, "export FOO='bar baz'\n").unwrap();
        edit(&path, |m| {
            m.insert("FOO".into(), "new value".into());
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("export"));
        assert!(after.contains("FOO="));
        // Should preserve single vs double? Original single, new value has space, contains no single quote, so keep single or double both ok but export preserved
        assert!(after.contains("FOO=") && after.contains("new value"));
    }

    #[test]
    fn preserves_blank_lines_and_comments_order() {
        let text = "\n# comment\nFOO=1\n\nBAR=2\n";
        let (lines, _) = parse_env_text(text).unwrap();
        assert_eq!(lines.len(), 5);
        assert!(matches!(lines[0].kind, LineKind::Blank));
        assert!(matches!(lines[1].kind, LineKind::Comment));
        assert!(matches!(lines[2].kind, LineKind::Entry(_)));
    }

    #[test]
    fn writing_leaves_backup() {
        let path = scratch("backed.env");
        std::fs::write(&path, "FOO=old\n").unwrap();
        edit(&path, |m| {
            m.insert("FOO".into(), "new".into());
        })
        .unwrap();
        let backups: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("backed.env.bak.")
            })
            .collect();
        assert!(!backups.is_empty());
        let restored =
            std::fs::read_to_string(backups.first().expect("backup exists").path()).unwrap();
        assert!(restored.contains("old"));
        for b in backups {
            drop(std::fs::remove_file(b.path()));
        }
    }

    #[test]
    fn unquoted_value_trimming_and_comment() {
        let (_, map) = parse_env_text("A=hello   # comment\nB=  spaced  \n").unwrap();
        assert_eq!(map["A"], "hello");
        // B's value "spaced" trimmed
        assert_eq!(map["B"], "spaced");
    }
}
