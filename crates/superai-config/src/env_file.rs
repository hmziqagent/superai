//! Env files: `KEY=value`, `export KEY=value`, quoting, comments, duplicates.
//!
//! No existing crate round-trips env files losslessly (dotenvy and friends
//! are loaders: they drop comments, blank lines, export prefixes, quoting
//! style, and duplicates on write), so this module implements its own
//! line-preserving parser (DOC-07). Comments, blank lines, `export`
//! prefixes, quoting style, spacing around `=`, and newline style survive
//! untouched; duplicate keys are preserved with the last occurrence
//! effective, and edits update the last occurrence without deduplicating.
//! Double quotes honor `\"` `\\` `\n` `\r` `\t`, single quotes `\'` `\\`,
//! unquoted `\#`; values stay literal (no `$VAR` expansion).

use std::collections::BTreeMap;
use std::path::Path;

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
        // Only "export<whitespace>KEY" carries the prefix; "exportFOO" is a key.
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
            key = left_trimmed.to_owned();
        }
    } else {
        key = left_trimmed.to_owned();
    }

    if key.is_empty() {
        return Err(format!("invalid env line (empty key): {raw}"));
    }
    if key.contains(' ')
        || key.contains('\t')
        || key.contains('#')
        || key.contains('"')
        || key.contains('\'')
    {
        return Err(format!("invalid env key `{key}`"));
    }

    let right_ws_len = right
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map_or(right.len(), |(idx, _)| idx);

    // value_start byte offset in raw = eq_idx + 1 + right_ws_len
    let value_start = eq_idx + 1 + right_ws_len;

    let value_part = raw.get(value_start..).unwrap_or_default();
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
        // Unquoted: value runs until an unescaped '#'.
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
        // Unquoted values are trailing-whitespace trimmed.
        let trimmed_end = raw_value.trim_end();
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
fn parse_env_text(
    text: &str,
) -> std::result::Result<(Vec<ParsedLine>, BTreeMap<String, String>), String> {
    let text = strip_bom(text);
    let mut lines = Vec::new();
    let mut map = BTreeMap::new();

    if !text.is_empty() {
        // A trailing newline ends the last line rather than starting an
        // empty one; interior blank lines stay.
        let body = text.strip_suffix('\n').unwrap_or(text);
        for segment in body.split('\n') {
            let raw_line = segment.strip_suffix('\r').unwrap_or(segment);
            let parsed = parse_line(raw_line)?;
            if let LineKind::Entry(ref meta) = parsed.kind {
                map.insert(meta.key.clone(), meta.value.clone());
            }
            lines.push(parsed);
        }
    }

    Ok((lines, map))
}

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
/// Written as normalized `KEY=value` lines, quoted only when required, with
/// LF newlines and a trailing newline. `export` prefixes, comments, and
/// blank lines survive only through [`edit`] on existing files.
pub fn store(path: &Path, vars: &BTreeMap<String, String>) -> Result<()> {
    let mut text = String::new();
    for (k, v) in vars {
        let formatted = format_value_normalized(v);
        text.push_str(k);
        text.push('=');
        text.push_str(&formatted);
        text.push('\n');
    }

    crate::transaction::commit_file(
        "env-store",
        path,
        text.as_bytes(),
        crate::document::DocumentKind::Env,
    )?;
    Ok(())
}

/// Read fresh, apply `edit`, write back only if the effective map changed.
///
/// Changed keys update their last occurrence in place; new keys are appended;
/// removed keys lose every occurrence. Comments, blank lines, `export`
/// prefixes, quoting, spacing, duplicates, and CRLF/LF are preserved where
/// untouched. A no-op edit performs no write and no backup.
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
    let removed_keys: Vec<String> = original_map
        .keys()
        .filter(|k| !map.contains_key(*k))
        .cloned()
        .collect();
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
            lines.push(new_entry_line(key, new_value));
            continue;
        };
        if old_value == new_value {
            continue;
        }
        // Changed: update the last occurrence in place, keeping its
        // export prefix, spacing, quoting style, and trailing comment.
        let Some(idx) = lines
            .iter()
            .rposition(|line| matches!(&line.kind, LineKind::Entry(m) if m.key == *key))
        else {
            continue;
        };
        let Some(line) = lines.get(idx) else {
            continue;
        };
        let LineKind::Entry(meta) = &line.kind else {
            continue;
        };
        let new_formatted = format_value_for_entry(new_value, meta.quoting);
        let prefix = line.raw.get(0..meta.value_start).unwrap_or_default();
        let suffix = line.raw.get(meta.value_end..).unwrap_or_default();
        let new_raw = format!("{prefix}{new_formatted}{suffix}");
        let new_start = prefix.len();
        let new_meta = EntryMeta {
            key: key.clone(),
            value: new_value.clone(),
            quoting: meta.quoting,
            export: meta.export,
            value_start: new_start,
            value_end: new_start + new_formatted.len(),
        };
        if let Some(slot) = lines.get_mut(idx) {
            *slot = ParsedLine {
                raw: new_raw,
                kind: LineKind::Entry(new_meta),
            };
        }
    }

    // Serialize lines back; every line keeps a terminator.
    let mut out = String::new();
    for line in &lines {
        out.push_str(&line.raw);
        out.push_str(newline);
    }

    crate::transaction::commit_file(
        "env-edit",
        path,
        out.as_bytes(),
        crate::document::DocumentKind::Env,
    )?;
    Ok(())
}

/// A normalized `KEY=value` line for a key the file did not carry before.
fn new_entry_line(key: &str, value: &str) -> ParsedLine {
    let formatted = format_value_normalized(value);
    let raw = format!("{key}={formatted}");
    ParsedLine {
        raw,
        kind: LineKind::Entry(EntryMeta {
            key: key.to_owned(),
            value: value.to_owned(),
            quoting: if requires_quotes(value) {
                Quoting::Double
            } else {
                Quoting::Unquoted
            },
            export: false,
            value_start: key.len() + 1,
            value_end: key.len() + 1 + formatted.len(),
        }),
    }
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
