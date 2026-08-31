//! Managed-span codec for `DocumentKind::TextFragment` (DOC-08).
//!
//! Non-structured supported files are edited only inside *managed spans*:
//! regions delimited by stable start/end sentinel lines such as
//!
//! ```text
//! # superai:begin:managed-name
//! …owned content…
//! # superai:end:managed-name
//! ```
//!
//! Rules enforced here (all fail closed):
//! - Every byte outside a managed span is preserved verbatim.
//! - Duplicate begin or end sentinels for one name are an error.
//! - Partial (unbalanced) sentinels — a begin without an end or vice versa —
//!   are an error.
//! - Nested or overlapping differently-named spans are an error.
//! - Removal removes exactly one complete owned span (both sentinels and the
//!   body between them, including the trailing newline).
//!
//! Shell quoting inside span bodies is *not* interpreted here. Executable
//! configs (crushrc-style surfaces) are not eligible for this codec; they
//! stay command-backed through their adapters.

use crate::error::{ConfigError, Result};

/// Fail-closed span validation error: the 1-based line of the offending
/// sentinel when known, plus a reason naming sentinels (never span content).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {reason}")]
pub struct SpanError {
    /// 1-based line number of the offending sentinel, when known.
    pub line: usize,
    /// What failed.
    pub reason: String,
}

impl SpanError {
    fn new(line: usize, reason: impl Into<String>) -> Self {
        Self {
            line: usize::max(line, 1),
            reason: reason.into(),
        }
    }

    /// Wrap into the typed config error for `path`.
    pub fn into_config_error(self, path: &std::path::Path) -> ConfigError {
        ConfigError::InvalidSpans {
            path: path.to_path_buf(),
            reason: format!("line {}: {}", self.line, self.reason),
        }
    }
}

/// A sentinel-delimited region of a text fragment.
///
/// `begin` is the byte offset of the start of the begin-sentinel line;
/// `end` is the byte offset just past the end-sentinel line including its
/// trailing newline when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanRange {
    /// Span name taken from the sentinels.
    pub name: String,
    /// Byte offset of the begin-sentinel line start.
    pub begin: usize,
    /// Byte offset past the end-sentinel line (newline included).
    pub end: usize,
}

/// Sentinel style: the comment prefix used to build sentinel lines.
///
/// The begin sentinel is `{prefix} superai:begin:{name}` and the end
/// sentinel is `{prefix} superai:end:{name}`, each on its own line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanCodec {
    comment_prefix: String,
}

impl Default for SpanCodec {
    fn default() -> Self {
        Self::new("#")
    }
}

impl SpanCodec {
    /// Create a codec whose sentinels start with `comment_prefix`
    /// (e.g. `"#"`, `"//"`, `"<!--"`).
    ///
    /// The prefix must be non-empty and contain no newline.
    pub fn new(comment_prefix: &str) -> Self {
        Self {
            comment_prefix: comment_prefix.to_owned(),
        }
    }

    /// The begin-sentinel line for `name`.
    pub fn begin_sentinel(&self, name: &str) -> String {
        format!("{} superai:begin:{}", self.comment_prefix, name)
    }

    /// The end-sentinel line for `name`.
    pub fn end_sentinel(&self, name: &str) -> String {
        format!("{} superai:end:{}", self.comment_prefix, name)
    }

    /// Validate a span name: non-empty, no whitespace, no `:`.
    fn validate_name(name: &str) -> std::result::Result<(), SpanError> {
        if name.is_empty() {
            return Err(SpanError::new(1, "span name must not be empty"));
        }
        if name.chars().any(|c| c.is_whitespace() || c == ':') {
            return Err(SpanError::new(
                1,
                format!("span name `{name}` must not contain whitespace or `:`"),
            ));
        }
        Ok(())
    }

    /// Parse and validate all managed spans in `text`.
    ///
    /// Fail closed on duplicate, partial, mis-ordered, or overlapping
    /// sentinels. Returned ranges are ordered by position.
    pub fn validate(&self, text: &str) -> std::result::Result<Vec<SpanRange>, SpanError> {
        let mut begins: Vec<(String, usize, usize)> = Vec::new(); // (name, line, offset)
        let mut ends: Vec<(String, usize, usize, usize)> = Vec::new(); // (name, line, offset, line_end)

        let begin_marker = format!("{}superai:begin:", prefix_with_space(self));
        let end_marker = format!("{}superai:end:", prefix_with_space(self));

        let mut offset = 0usize;
        for (idx, line) in text.split_inclusive('\n').enumerate() {
            let line_no = idx.saturating_add(1);
            // Sentinels must start at column 0 so span math stays exact;
            // trailing whitespace (and the line's `\n`/`\r\n`) is trimmed
            // from the name.
            if let Some(rest) = line.strip_prefix(&begin_marker) {
                let name = rest.trim_end();
                Self::validate_name(name).map_err(|e| SpanError::new(line_no, e.reason))?;
                begins.push((name.to_owned(), line_no, offset));
            } else if let Some(rest) = line.strip_prefix(&end_marker) {
                let name = rest.trim_end();
                Self::validate_name(name).map_err(|e| SpanError::new(line_no, e.reason))?;
                ends.push((
                    name.to_owned(),
                    line_no,
                    offset,
                    offset.saturating_add(line.len()),
                ));
            }
            offset = offset.saturating_add(line.len());
        }

        // Pair begins with ends, fail closed on every anomaly.
        let mut ranges = Vec::new();
        for (name, line_no, begin_offset) in &begins {
            let range = pair_single_span(name, *line_no, *begin_offset, &ends)?;
            ranges.push(range);
        }
        for (name, line_no, _, _) in &ends {
            let count = begins.iter().filter(|(n, _, _)| n == name).count();
            if count == 0 {
                return Err(SpanError::new(
                    *line_no,
                    format!("unbalanced span `{name}`: end sentinel has no begin sentinel"),
                ));
            }
            if count > 1 {
                return Err(SpanError::new(
                    *line_no,
                    format!("duplicate begin sentinel for span `{name}`"),
                ));
            }
        }

        ranges.sort_by_key(|r| r.begin);
        let mut prev_end = 0usize;
        for range in &ranges {
            if range.begin < prev_end {
                return Err(SpanError::new(
                    1,
                    format!(
                        "overlapping or nested spans are not supported (`{}` overlaps a previous span)",
                        range.name
                    ),
                ));
            }
            prev_end = range.end;
        }
        Ok(ranges)
    }

    /// Find the span named `name`, if present and well-formed.
    pub fn find_span(
        &self,
        text: &str,
        name: &str,
    ) -> std::result::Result<Option<SpanRange>, SpanError> {
        Self::validate_name(name)?;
        let ranges = self.validate(text)?;
        Ok(ranges.into_iter().find(|r| r.name == name))
    }

    /// The body between the sentinels of `name`, if the span exists.
    pub fn span_body(
        &self,
        text: &str,
        name: &str,
    ) -> std::result::Result<Option<String>, SpanError> {
        match self.find_span(text, name)? {
            None => Ok(None),
            Some(range) => {
                let body_start = range
                    .begin
                    .saturating_add(self.begin_sentinel(name).len())
                    .saturating_add(1); // newline after begin sentinel
                // Back up over the end sentinel and its line terminator
                // (LF, or the LF of a CRLF pair; a stray CR is trimmed).
                let body_end = range.end.saturating_sub(self.end_sentinel(name).len() + 1);
                let body = text
                    .get(body_start..body_end)
                    .map(str::to_owned)
                    .unwrap_or_default();
                Ok(Some(body.trim_end_matches(['\r', '\n']).to_owned()))
            }
        }
    }

    /// Insert a new span `name` with `body` at the end of `text`.
    ///
    /// Fails closed if the span already exists or if the result would carry
    /// duplicate/partial/nested sentinels (e.g. `body` itself contains
    /// sentinel lines). All bytes of `text` are preserved as a prefix; a
    /// separating newline is added when `text` does not end with one.
    pub fn insert_span(
        &self,
        text: &str,
        name: &str,
        body: &str,
    ) -> std::result::Result<String, SpanError> {
        Self::validate_name(name)?;
        if self.find_span(text, name)?.is_some() {
            return Err(SpanError::new(
                1,
                format!("duplicate span `{name}` already exists"),
            ));
        }
        let mut out = String::from(text);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&self.begin_sentinel(name));
        out.push('\n');
        out.push_str(body.trim_matches('\n'));
        out.push('\n');
        out.push_str(&self.end_sentinel(name));
        out.push('\n');
        // Fail closed if the body smuggled sentinels that break validation.
        drop(self.validate(&out)?);
        Ok(out)
    }

    /// Replace the body of span `name`; everything outside the span stays
    /// byte-identical. Fails closed when the span is absent or the new body
    /// breaks sentinel validation.
    pub fn replace_span(
        &self,
        text: &str,
        name: &str,
        body: &str,
    ) -> std::result::Result<String, SpanError> {
        Self::validate_name(name)?;
        let Some(range) = self.find_span(text, name)? else {
            return Err(SpanError::new(1, format!("span `{name}` not found")));
        };
        let mut rebuilt = String::with_capacity(text.len());
        let prefix = text.get(0..range.begin).unwrap_or_default();
        let suffix = text.get(range.end..).unwrap_or_default();
        rebuilt.push_str(prefix);
        rebuilt.push_str(&self.begin_sentinel(name));
        rebuilt.push('\n');
        rebuilt.push_str(body.trim_matches('\n'));
        rebuilt.push('\n');
        rebuilt.push_str(&self.end_sentinel(name));
        // Terminate the end-sentinel line (canonical form: the end line
        // always carries a trailing newline, matching [`Self::insert_span`]).
        // `suffix` starts at the next line; a leading newline inside it is a
        // blank line after the span and is preserved verbatim.
        rebuilt.push('\n');
        rebuilt.push_str(suffix);
        drop(self.validate(&rebuilt)?);
        Ok(rebuilt)
    }

    /// Remove the complete span `name` (sentinels, body, and the span's
    /// trailing newline). Bytes outside the span are preserved exactly.
    pub fn remove_span(&self, text: &str, name: &str) -> std::result::Result<String, SpanError> {
        Self::validate_name(name)?;
        let Some(range) = self.find_span(text, name)? else {
            return Err(SpanError::new(1, format!("span `{name}` not found")));
        };
        let prefix = text.get(0..range.begin).unwrap_or_default();
        let suffix = text.get(range.end..).unwrap_or_default();
        let mut out = String::with_capacity(text.len());
        out.push_str(prefix);
        out.push_str(suffix);
        drop(self.validate(&out)?);
        Ok(out)
    }

    /// The text with every complete managed span removed: the bytes that
    /// live outside all spans. Used to prove that an edit did not touch
    /// unmanaged content (DOC-08 commit gate).
    pub fn outside_span_bytes(&self, text: &str) -> std::result::Result<String, SpanError> {
        let ranges = self.validate(text)?;
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0usize;
        for range in &ranges {
            out.push_str(text.get(cursor..range.begin).unwrap_or_default());
            cursor = range.end;
        }
        out.push_str(text.get(cursor..).unwrap_or_default());
        Ok(out)
    }
}

/// `prefix` followed by one space, tolerating the marker's leading space so
/// `"#"` produces `"# superai:begin:"`.
fn prefix_with_space(codec: &SpanCodec) -> String {
    format!("{} ", codec.comment_prefix)
}

/// Pair one begin sentinel with its end, failing closed on duplicates,
/// missing ends, and mis-ordered sentinels.
fn pair_single_span(
    name: &str,
    line_no: usize,
    begin_offset: usize,
    ends: &[(String, usize, usize, usize)],
) -> std::result::Result<SpanRange, SpanError> {
    let matching: Vec<&(String, usize, usize, usize)> =
        ends.iter().filter(|(n, _, _, _)| n == name).collect();
    let one = match matching.as_slice() {
        [] => {
            return Err(SpanError::new(
                line_no,
                format!("unbalanced span `{name}`: begin sentinel has no end sentinel"),
            ));
        }
        [one] => one,
        _ => {
            return Err(SpanError::new(
                line_no,
                format!("duplicate end sentinel for span `{name}`"),
            ));
        }
    };
    if one.2 < begin_offset {
        return Err(SpanError::new(
            line_no,
            format!("unbalanced span `{name}`: end sentinel precedes begin"),
        ));
    }
    Ok(SpanRange {
        name: name.to_owned(),
        begin: begin_offset,
        end: one.3,
    })
}

/// Convenience: validate spans in `text` mapped to the typed config error.
pub fn ensure_valid_spans(text: &str, path: &std::path::Path) -> Result<Vec<SpanRange>> {
    SpanCodec::default()
        .validate(text)
        .map_err(|e| e.into_config_error(path))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_find_and_read_span_body() {
        let codec = SpanCodec::default();
        let base = "prelude line\n";
        let text = codec
            .insert_span(base, "managed", "owned body\nsecond line")
            .unwrap();
        assert_eq!(
            text,
            "prelude line\n# superai:begin:managed\nowned body\nsecond line\n# superai:end:managed\n"
        );
        let span = codec.find_span(&text, "managed").unwrap().unwrap();
        assert_eq!(span.name, "managed");
        assert_eq!(
            codec.span_body(&text, "managed").unwrap().unwrap(),
            "owned body\nsecond line"
        );
        assert!(codec.find_span(&text, "absent").unwrap().is_none());
    }

    #[test]
    fn insert_adds_separator_newline_when_missing() {
        let codec = SpanCodec::default();
        let text = codec
            .insert_span("no trailing newline", "x", "body")
            .unwrap();
        assert_eq!(
            text,
            "no trailing newline\n# superai:begin:x\nbody\n# superai:end:x\n"
        );
    }

    #[test]
    fn insert_into_empty_fragment() {
        let codec = SpanCodec::default();
        let text = codec.insert_span("", "x", "body").unwrap();
        assert_eq!(text, "# superai:begin:x\nbody\n# superai:end:x\n");
    }

    #[test]
    fn insert_existing_span_fails_closed() {
        let codec = SpanCodec::default();
        let text = codec.insert_span("", "x", "body").unwrap();
        let err = codec.insert_span(&text, "x", "other").unwrap_err();
        assert!(err.reason.contains("duplicate"), "{}", err.reason);
    }

    #[test]
    fn duplicate_begin_sentinels_fail_closed() {
        let text = "a\n# superai:begin:x\none\n# superai:begin:x\ntwo\n# superai:end:x\n";
        let err = SpanCodec::default().validate(text).unwrap_err();
        assert!(err.reason.contains("duplicate"), "{}", err.reason);
        assert!(err.reason.contains('x'));
    }

    #[test]
    fn duplicate_end_sentinels_fail_closed() {
        let text = "# superai:begin:x\none\n# superai:end:x\n# superai:end:x\n";
        let err = SpanCodec::default().validate(text).unwrap_err();
        assert!(err.reason.contains("duplicate"), "{}", err.reason);
    }

    #[test]
    fn unbalanced_begin_without_end_fails_closed() {
        let err = SpanCodec::default()
            .validate("# superai:begin:x\nbody\n")
            .unwrap_err();
        assert!(err.reason.contains("unbalanced"), "{}", err.reason);
        assert_eq!(err.line, 1);
    }

    #[test]
    fn orphan_end_sentinel_fails_closed() {
        let err = SpanCodec::default()
            .validate("body\n# superai:end:x\n")
            .unwrap_err();
        assert!(err.reason.contains("unbalanced"), "{}", err.reason);
    }

    #[test]
    fn end_before_begin_fails_closed() {
        let text = "# superai:end:x\n# superai:begin:x\n";
        let err = SpanCodec::default().validate(text).unwrap_err();
        assert!(err.reason.contains("unbalanced"), "{}", err.reason);
    }

    #[test]
    fn nested_and_overlapping_spans_fail_closed() {
        let text = "# superai:begin:outer\n# superai:begin:inner\nx\n# superai:end:inner\n# superai:end:outer\n";
        let err = SpanCodec::default().validate(text).unwrap_err();
        assert!(
            err.reason.contains("overlap") || err.reason.contains("nested"),
            "{}",
            err.reason
        );
    }

    #[test]
    fn sentinel_line_smuggled_into_body_fails_on_insert() {
        let codec = SpanCodec::default();
        let base = codec.insert_span("", "x", "body").unwrap();
        // A body containing another begin sentinel for the same name must be
        // rejected at insert time (duplicate).
        let err = codec
            .insert_span(&base, "x", "# superai:begin:x\nsmuggle")
            .unwrap_err();
        assert!(err.reason.contains("duplicate"), "{}", err.reason);
    }

    #[test]
    fn remove_span_preserves_every_outside_byte() {
        let codec = SpanCodec::default();
        let original = "before\n# superai:begin:x\nowned\n# superai:end:x\nafter\n";
        let removed = codec.remove_span(original, "x").unwrap();
        assert_eq!(removed, "before\nafter\n");
        // Round trip: removing an inserted span restores the base text.
        let base = "base text\n";
        let inserted = codec.insert_span(base, "x", "owned").unwrap();
        assert_eq!(codec.remove_span(&inserted, "x").unwrap(), base);
    }

    #[test]
    fn remove_absent_span_errors() {
        let err = SpanCodec::default()
            .remove_span("plain text\n", "x")
            .unwrap_err();
        assert!(err.reason.contains("not found"), "{}", err.reason);
    }

    #[test]
    fn replace_span_preserves_outside_bytes_exactly() {
        let codec = SpanCodec::default();
        let original = "header # keep\n# superai:begin:x\nold body\n# superai:end:x\ntail\n";
        let replaced = codec.replace_span(original, "x", "new body").unwrap();
        assert_eq!(
            replaced,
            "header # keep\n# superai:begin:x\nnew body\n# superai:end:x\ntail\n"
        );
        // Multiple spans: replacing one leaves the other untouched.
        let two = codec.insert_span(&replaced, "y", "y body").unwrap();
        let replaced_two = codec.replace_span(&two, "y", "changed").unwrap();
        assert!(replaced_two.contains("new body"));
        assert!(replaced_two.contains("changed"));
        assert!(replaced_two.contains("tail"));
    }

    #[test]
    fn outside_span_bytes_excludes_complete_spans_only() {
        let codec = SpanCodec::default();
        let text = "a\n# superai:begin:x\nsecretish\n# superai:end:x\nb\n";
        assert_eq!(codec.outside_span_bytes(text).unwrap(), "a\nb\n");
    }

    #[test]
    fn custom_comment_prefix_and_crlf_tolerant_names() {
        let codec = SpanCodec::new("//");
        let text = codec.insert_span("js\n", "cfg", "v=1").unwrap();
        assert!(text.contains("// superai:begin:cfg"));
        assert!(codec.find_span(&text, "cfg").unwrap().is_some());
        // CRLF file: sentinel lines end with \r\n, names still parse.
        let crlf = text.replace('\n', "\r\n");
        assert!(codec.find_span(&crlf, "cfg").unwrap().is_some());
    }

    #[test]
    fn invalid_span_names_rejected() {
        SpanCodec::default()
            .insert_span("", "has space", "b")
            .unwrap_err();
        SpanCodec::default().insert_span("", "", "b").unwrap_err();
        SpanCodec::default()
            .insert_span("", "a:b", "b")
            .unwrap_err();
    }

    #[test]
    fn ensure_valid_spans_maps_to_typed_config_error() {
        let err = ensure_valid_spans("# superai:begin:x\n", std::path::Path::new("/f/frag.txt"))
            .unwrap_err();
        match err {
            ConfigError::InvalidSpans { path, reason } => {
                assert_eq!(path, std::path::Path::new("/f/frag.txt"));
                assert!(reason.contains("unbalanced"), "{reason}");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }
}
