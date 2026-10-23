//! JSONC reads strip comments/trailing commas before the strict parse;
//! changing writes on files carrying that material are refused (DOC-05).

use std::borrow::Cow;
use std::path::Path;

use memchr::{memchr, memchr2_iter, memchr3_iter};
use serde_json::{Map, Value};

use crate::error::{ConfigError, Result};

#[cfg(test)]
fn strip_trailing_commas(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut run = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut idx = 0usize;

    while let Some(&ch) = bytes.get(idx) {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == b'\\' {
                escaped = true;
            } else if ch == b'"' {
                in_string = false;
            }
            idx += 1;
        } else if ch == b'"' {
            in_string = true;
            idx += 1;
        } else if ch == b',' {
            let mut look = idx + 1;
            while matches!(bytes.get(look), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                look += 1;
            }
            if matches!(bytes.get(look), Some(b'}' | b']')) {
                output.push_str(input.get(run..idx).unwrap_or_default());
                run = idx + 1;
            }
            idx += 1;
        } else {
            idx += 1;
        }
    }
    output.push_str(input.get(run..).unwrap_or_default());
    output
}

/// Strip JSONC extensions (comments + trailing commas) to produce strict JSON.
pub fn strip_jsonc(input: &str) -> String {
    strip_jsonc_cow(input).into_owned()
}

pub(crate) fn strip_jsonc_cow(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    match (memchr(b'/', bytes).is_some(), memchr(b',', bytes).is_some()) {
        (false, false) => Cow::Borrowed(text),
        (false, true) => strip_trailing_commas_cow(text),
        (true, _) => strip_fused_cow(text),
    }
}

fn span(text: &str, start: usize, end: usize) -> &str {
    text.get(start..end).unwrap_or_default()
}

fn escaped_at(bytes: &[u8], quote: usize) -> bool {
    let mut i = quote;
    let mut backslashes = 0u64;
    while i > 0 && bytes.get(i - 1) == Some(&b'\\') {
        backslashes += 1;
        i -= 1;
    }
    backslashes % 2 == 1
}

fn block_comment_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while let Some(rel) = memchr(b'*', bytes.get(i..).unwrap_or_default()) {
        let star = i + rel;
        if bytes.get(star + 1) == Some(&b'/') {
            return Some(star + 2);
        }
        i = star + 1;
    }
    None
}

fn comment_end_at(bytes: &[u8], slash: usize) -> usize {
    match bytes.get(slash + 1) {
        Some(b'/') => memchr(b'\n', bytes.get(slash + 2..).unwrap_or_default())
            .map_or(bytes.len(), |rel| slash + 2 + rel),
        _ => block_comment_end(bytes, slash + 2).unwrap_or(bytes.len()),
    }
}

fn comment_pair_at(bytes: &[u8], at: usize) -> bool {
    bytes.get(at) == Some(&b'/') && matches!(bytes.get(at + 1), Some(b'/' | b'*'))
}

fn flush_span(out: &mut String, text: &str, run: usize, end: usize) {
    if out.capacity() == 0 {
        out.reserve(text.len() - run);
    }
    out.push_str(span(text, run, end));
}

pub(crate) fn strip_jsonc_comments_cow(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    if memchr(b'/', bytes).is_none() {
        return Cow::Borrowed(text);
    }
    let mut out = String::new();
    let mut run = 0usize;
    let mut in_string = false;
    let mut skip_until = 0usize;
    for p in memchr2_iter(b'"', b'/', bytes) {
        if p < skip_until {
            continue;
        }
        if in_string {
            if bytes.get(p) == Some(&b'"') && !escaped_at(bytes, p) {
                in_string = false;
            }
            continue;
        }
        if bytes.get(p) == Some(&b'"') {
            in_string = true;
            continue;
        }
        if comment_pair_at(bytes, p) {
            flush_span(&mut out, text, run, p);
            let end = comment_end_at(bytes, p);
            skip_until = end;
            run = end;
        }
    }
    if out.is_empty() && run == 0 {
        return Cow::Borrowed(text);
    }
    flush_span(&mut out, text, run, bytes.len());
    Cow::Owned(out)
}

fn strip_fused_cow(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut run = 0usize;
    let mut in_string = false;
    let mut skip_until = 0usize;
    for p in memchr3_iter(b'"', b'/', b',', bytes) {
        if p < skip_until {
            continue;
        }
        if in_string {
            if bytes.get(p) == Some(&b'"') && !escaped_at(bytes, p) {
                in_string = false;
            }
            continue;
        }
        if bytes.get(p) == Some(&b'"') {
            in_string = true;
            continue;
        }
        if comment_pair_at(bytes, p) {
            flush_span(&mut out, text, run, p);
            let end = comment_end_at(bytes, p);
            skip_until = end;
            run = end;
            continue;
        }
        if bytes.get(p) != Some(&b',') {
            continue;
        }
        let mut look = p + 1;
        loop {
            match bytes.get(look) {
                Some(b' ' | b'\t' | b'\n' | b'\r') => look += 1,
                Some(b'/') if comment_pair_at(bytes, look) => {
                    look = comment_end_at(bytes, look);
                }
                _ => break,
            }
        }
        if matches!(bytes.get(look), Some(b'}' | b']')) {
            flush_span(&mut out, text, run, p);
            run = p + 1;
        }
    }
    if out.is_empty() && run == 0 {
        return Cow::Borrowed(text);
    }
    flush_span(&mut out, text, run, bytes.len());
    Cow::Owned(out)
}

fn strip_trailing_commas_cow(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut run = 0usize;
    let mut in_string = false;
    for p in memchr2_iter(b',', b'"', bytes) {
        if in_string {
            if bytes.get(p) == Some(&b'"') && !escaped_at(bytes, p) {
                in_string = false;
            }
            continue;
        }
        if bytes.get(p) == Some(&b'"') {
            in_string = true;
            continue;
        }
        let mut look = p + 1;
        while matches!(bytes.get(look), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            look += 1;
        }
        if matches!(bytes.get(look), Some(b'}' | b']')) {
            flush_span(&mut out, text, run, p);
            run = p + 1;
        }
    }
    if out.is_empty() && run == 0 {
        return Cow::Borrowed(text);
    }
    flush_span(&mut out, text, run, bytes.len());
    Cow::Owned(out)
}

/// Read fresh; comments and trailing commas accepted, duplicates rejected.
/// The root must be an object; use [`load_value`] for arbitrary roots.
pub fn load(path: &Path) -> Result<Map<String, Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(ConfigError::io(path, e)),
    };

    if text.trim().is_empty() {
        return Ok(Map::new());
    }

    let value = crate::json::parse_strict(&strip_jsonc_cow(&text), path)?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(ConfigError::NotAnObject {
            path: path.to_path_buf(),
        }),
    }
}

/// Read JSONC as `Value`, preserving an arbitrary root type (array, scalar, …).
pub fn load_value(path: &Path) -> Result<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Object(Map::new())),
        Err(e) => return Err(ConfigError::io(path, e)),
    };

    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }

    crate::json::parse_strict(&strip_jsonc_cow(&text), path)
}

fn ensure_lossless_write(path: &Path) -> Result<()> {
    match std::fs::read_to_string(path) {
        Ok(text) if strip_jsonc_cow(&text).as_ref() != text => {
            Err(ConfigError::lossy_write(path, "jsonc"))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ConfigError::io(path, e)),
    }
}

fn jsonc_text(path: &Path, value: impl serde::Serialize) -> Result<String> {
    let mut text = serde_json::to_string_pretty(&value).map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    text.push('\n');
    Ok(text)
}

fn commit_jsonc(path: &Path, text: &str) -> Result<()> {
    crate::transaction::commit_file(
        "jsonc-store",
        path,
        text.as_bytes(),
        crate::document::DocumentKind::JsonC,
    )?;
    Ok(())
}

/// Back up, then write normalized JSON; refused with `LossyWrite` when the
/// target carries comments or trailing commas (DOC-05).
pub fn store(path: &Path, config: &Map<String, Value>) -> Result<()> {
    ensure_lossless_write(path)?;
    let text = jsonc_text(path, config)?;
    commit_jsonc(path, &text)
}

/// [`store`] for any root; same lossless-write gate.
pub fn store_value(path: &Path, value: &Value) -> Result<()> {
    ensure_lossless_write(path)?;
    let text = jsonc_text(path, value)?;
    commit_jsonc(path, &text)
}

/// Read fresh, apply `edit`, write back only if changed; no-ops stay
/// byte-identical, changing edits pass the [`store`] gate.
pub fn edit<F>(path: &Path, edit: F) -> Result<()>
where
    F: FnOnce(&mut Map<String, Value>),
{
    let mut config = load(path)?;
    let original = config.clone();
    edit(&mut config);
    if config == original {
        return Ok(());
    }
    store(path, &config)
}

/// [`edit`] over [`Value`]; same gate.
pub fn edit_value<F>(path: &Path, edit: F) -> Result<()>
where
    F: FnOnce(&mut Value),
{
    let mut value = load_value(path)?;
    let original = value.clone();
    edit(&mut value);
    if value == original {
        return Ok(());
    }
    store_value(path, &value)
}

/// DOC-10 disclosure; files carrying JSONC material never reformat (their
/// writes are refused), extension-free files warn when not normalized.
pub fn formatting_change_warning(text: &str) -> Option<&'static str> {
    if text.trim().is_empty() || strip_jsonc_cow(text).as_ref() != text {
        return None;
    }
    crate::json::formatting_change_warning(text).map(|_| {
        "jsonc codec normalizes whitespace and indentation on changing writes for \
             extension-free files; surrounding formatting will change even where semantics do not"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (crate::test_util::TempDir, std::path::PathBuf) {
        let dir = crate::test_util::temp_dir_unique("config-jsonc");
        let path = dir.join(name);
        (dir, path)
    }

    fn scalar_strip_jsonc(text: &str) -> String {
        strip_trailing_commas(&crate::document::strip_jsonc_comments(text))
    }

    #[test]
    fn fast_strip_matches_scalar_on_fixed_corpus() {
        let cases = [
            "",
            "/",
            "//",
            "/*",
            "/**",
            "/**/",
            "/***/",
            "/*/",
            "{}",
            "{,}",
            "{\"a\":1,}",
            "[1,2,]",
            "[1,/*c*/2,]",
            "{\"u\":\"a\\\"/*b*/\",}",
            "{\"url\":\"http://x/y\",}",
            "a // c\r\nb\n",
            "// only comment, no newline",
            "/* unterminated {\"a\":1,}",
            "{\"k\":\"// not comment\", \"j\":/*c*/\"v\",}",
            "{\"é\":1/*中文*/,}",
            "{\"emoji\":\"\u{1F600}\",}",
            "{\"a\": [1, 2 /* x */, ] }",
            "{\"a\":1,,}",
            "[,]",
            "{\"a\":\"b\\\\\",}",
            "{\"a\":\"b\\\"\",}",
            "\r\n{\"a\":1,}\r\n",
            "{\"a\" : 1 , }",
            "\"\\uD83D\\uDE00\"",
            "{\"a\":\"\\u0061\",}",
        ];
        for case in cases {
            assert_eq!(strip_jsonc(case), scalar_strip_jsonc(case), "case {case:?}");
            assert_eq!(
                strip_jsonc_cow(case).as_ref(),
                scalar_strip_jsonc(case),
                "cow case {case:?}"
            );
            assert_eq!(
                strip_jsonc_comments_cow(case).as_ref(),
                crate::document::strip_jsonc_comments(case),
                "comments-only cow case {case:?}"
            );
        }
    }

    #[test]
    fn fast_strip_matches_scalar_on_generated_inputs() {
        fn xorshift(state: &mut u64) -> u64 {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            *state
        }
        let tokens = [
            "{",
            "}",
            "[",
            "]",
            ",",
            ":",
            "\"",
            "\\",
            "//c\n",
            "/*c*/",
            "/",
            "x",
            " ",
            "\r\n",
            "\"s\"",
            "1",
            "é",
            "\u{1F600}",
            "\"a\\\"b\"",
            "\"/*n*/\"",
            "//",
            "*/",
            "/*",
        ];
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..2000 {
            let count = usize::try_from(xorshift(&mut state) % 12).unwrap() + 1;
            let mut input = String::new();
            for _ in 0..count {
                let idx = usize::try_from(xorshift(&mut state) % tokens.len() as u64).unwrap();
                input.push_str(tokens[idx]);
            }
            assert_eq!(
                strip_jsonc_cow(&input).as_ref(),
                scalar_strip_jsonc(&input),
                "input {input:?}"
            );
            assert_eq!(
                strip_jsonc_comments_cow(&input).as_ref(),
                crate::document::strip_jsonc_comments(&input),
                "comments-only input {input:?}"
            );
        }
    }

    #[test]
    fn line_comment_swallows_the_cr_of_crlf() {
        let input = "{\"a\":1, // c\r\n\"b\":2}\n";
        assert_eq!(strip_jsonc(input), "{\"a\":1, \n\"b\":2}\n");
    }

    #[test]
    fn fast_strip_matches_scalar_on_pathological_inputs() {
        let mut escaped_quotes = String::from("\"");
        for _ in 0..2000 {
            escaped_quotes.push('\\');
            escaped_quotes.push('"');
        }
        escaped_quotes.push(',');
        let mut comma_comment_pairs = String::from("[");
        for i in 0..2000 {
            std::fmt::Write::write_fmt(&mut comma_comment_pairs, format_args!("{i}, /* c{i} "))
                .unwrap();
        }
        comma_comment_pairs.push_str("*/ ]");
        let unterminated = "1, /* never closed, ".repeat(1000);
        let star_storm = format!("/*{}*/ 1, ", "*".repeat(4000));
        let crlf_comments = "a: 1, // c\r\n".repeat(1000);
        let cases = [
            escaped_quotes,
            comma_comment_pairs,
            unterminated,
            star_storm,
            crlf_comments,
        ];
        for case in &cases {
            assert_eq!(
                strip_jsonc_cow(case).as_ref(),
                scalar_strip_jsonc(case),
                "pathological input of len {}",
                case.len()
            );
            assert_eq!(
                strip_jsonc_comments_cow(case).as_ref(),
                crate::document::strip_jsonc_comments(case),
                "pathological comments-only input of len {}",
                case.len()
            );
        }
    }

    #[test]
    fn bom_is_not_stripped_in_jsonc() {
        let (_dir, path) = scratch("bom.jsonc");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"{\"a\": 1}");
        std::fs::write(&path, bytes).unwrap();
        match load(&path) {
            Err(ConfigError::Json { .. }) => {}
            other => panic!("expected Json error for BOM-prefixed jsonc, got {other:?}"),
        }
    }

    #[test]
    fn staging_gate_refuses_bom_jsonc_bytes() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"{\"a\": 1}");
        let verdict = crate::document::validate_bytes_for_kind(
            &bytes,
            crate::document::DocumentKind::JsonC,
            Path::new("bom.jsonc"),
        );
        assert!(verdict.is_err(), "staging gate must refuse BOM'd jsonc");
    }

    #[test]
    fn staging_gate_strips_comments_but_not_trailing_commas() {
        let trailing = "{\"a\": 1,}".as_bytes();
        let editor_parse = serde_json::from_str::<Value>(&strip_jsonc("{\"a\": 1,}"));
        assert!(
            editor_parse.is_ok(),
            "editor chain must strip trailing commas"
        );
        let verdict = crate::document::validate_bytes_for_kind(
            trailing,
            crate::document::DocumentKind::JsonC,
            Path::new("trailing.jsonc"),
        );
        assert!(
            verdict.is_err(),
            "staging gate must refuse trailing commas it cannot strip"
        );

        let commented = "{\n  // c\n  \"a\": 1\n}".as_bytes();
        let commented_verdict = crate::document::validate_bytes_for_kind(
            commented,
            crate::document::DocumentKind::JsonC,
            Path::new("commented.jsonc"),
        );
        assert!(
            commented_verdict.is_ok(),
            "staging gate must strip comments it does strip"
        );
    }

    #[test]
    fn formatting_change_warning_edges() {
        assert_eq!(formatting_change_warning(""), None);
        assert_eq!(formatting_change_warning("{\n  \"a\": 1\n}\n"), None);
        assert!(formatting_change_warning("{\"a\":1}").is_some());
        assert_eq!(
            formatting_change_warning("// header\n{\"a\": 1}\n"),
            None,
            "comment-carrying files never reformat: their writes are refused"
        );
        assert_eq!(formatting_change_warning("{\"a\": 1,}\n"), None);
    }

    #[test]
    fn strips_line_comments() {
        let input = "{\n  \"a\": 1, // keep this\n  \"b\": 2 // trailing\n}\n";
        let out = strip_jsonc(input);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"], Value::Number(1.into()));
        assert_eq!(v["b"], Value::Number(2.into()));
    }

    #[test]
    fn strips_block_comments() {
        let input = r#"{"a": 1 /* comment */, "b": /* c */ 2}"#;
        let out = strip_jsonc(input);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"], Value::Number(1.into()));
        assert_eq!(v["b"], Value::Number(2.into()));
    }

    #[test]
    fn strips_trailing_commas_object_and_array() {
        let input = r#"{"a": 1, "b": 2,}"#;
        let out = strip_jsonc(input);
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap()["b"],
            Value::Number(2.into())
        );

        let input2 = "[1, 2, 3,]";
        let out2 = strip_jsonc(input2);
        let v: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 3);
    }

    #[test]
    fn preserves_comment_like_content_inside_strings() {
        let input = r#"{"a": "value // not a comment", "b": "value /* also not */"}"#;
        let out = strip_jsonc(input);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"], Value::String("value // not a comment".into()));
        assert_eq!(v["b"], Value::String("value /* also not */".into()));
    }

    #[test]
    fn preserves_commas_inside_strings() {
        let input = r#"{"a": "a, b, c", "b": 2,}"#;
        let out = strip_jsonc(input);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"], Value::String("a, b, c".into()));
    }

    #[test]
    fn load_accepts_jsonc_with_comments_and_trailing_commas() {
        let (_dir, path) = scratch("with_comments.jsonc");
        std::fs::write(
            &path,
            "{\n  // line comment\n  \"model\": \"opus\", /* block */\n  \"x\": 1,\n}\n",
        )
        .unwrap();
        let map = load(&path).unwrap();
        assert_eq!(map["model"], Value::String("opus".to_owned()));
        assert_eq!(map["x"], Value::Number(1.into()));
    }

    #[test]
    fn load_rejects_duplicate_keys_even_in_jsonc() {
        let (_dir, path) = scratch("dup.jsonc");
        std::fs::write(&path, r#"{"a": 1, "a": 2}"#).unwrap();
        let err = load(&path).unwrap_err();
        match err {
            ConfigError::Json { source, .. } => assert!(source.to_string().contains("duplicate")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_op_preserves_comments_byte_identity() {
        let (_dir, path) = scratch("noop.jsonc");
        let original = "{\n  // keep me\n  \"a\": 1, // comment\n  \"b\": 2,\n}\n";
        std::fs::write(&path, original).unwrap();
        let before = std::fs::read(&path).unwrap();
        edit(&path, |_| {}).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn changing_edit_refused_preserving_comments() {
        let (_dir, path) = scratch("change.jsonc");
        let original = "{\"a\":1, // c\n}";
        std::fs::write(&path, original).unwrap();
        let result = edit(&path, |m| {
            m.insert("b".into(), Value::Number(2.into()));
        });
        match result {
            Err(ConfigError::LossyWrite { format, .. }) => assert_eq!(format, "jsonc"),
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn changing_edit_value_refused_on_comments() {
        let (_dir, path) = scratch("value.jsonc");
        let original = "// header\n{\"a\": 1,}\n";
        std::fs::write(&path, original).unwrap();
        let result = edit_value(&path, |v| {
            if let Value::Object(m) = v {
                m.insert("b".into(), Value::Number(2.into()));
            }
        });
        assert!(matches!(result, Err(ConfigError::LossyWrite { .. })));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn store_refused_when_target_carries_jsonc_extensions() {
        let (_dir, path) = scratch("store.jsonc");
        let original = "{\n  // keep me\n  \"a\": 1,\n}\n";
        std::fs::write(&path, original).unwrap();
        let mut map = Map::new();
        map.insert("a".into(), Value::Number(2.into()));
        match store(&path, &map) {
            Err(ConfigError::LossyWrite { format, .. }) => assert_eq!(format, "jsonc"),
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let dir_entries = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(dir_entries, 1, "refused write must not create files");
    }

    #[test]
    fn store_allows_missing_target_and_extension_free_target() {
        // Creation: there is no lexical material to destroy.
        let (_dir, path) = scratch("new.jsonc");
        let mut map = Map::new();
        map.insert("a".into(), Value::Number(1.into()));
        store(&path, &map).unwrap();
        assert_eq!(load(&path).unwrap()["a"], Value::Number(1.into()));
        drop(std::fs::remove_file(&path));

        let (_dir, clean) = scratch("clean.json");
        std::fs::write(&clean, "{\"a\":1}").unwrap();
        map.insert("b".into(), Value::Number(2.into()));
        store(&clean, &map).unwrap();
        let loaded = load(&clean).unwrap();
        assert_eq!(loaded["a"], Value::Number(1.into()));
        assert_eq!(loaded["b"], Value::Number(2.into()));
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let (_dir, path) = scratch("absent.jsonc");
        drop(std::fs::remove_file(&path));
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn trailing_comma_with_comment_between() {
        let input = "{\n  \"a\": 1, // comment\n}\n";
        let out = strip_jsonc(input);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"], Value::Number(1.into()));
    }

    #[test]
    fn handles_opencode_kilo_amp_style_fixtures() {
        let input = r#"{
  // Provider config
  "provider": "glm", // glm endpoint
  "models": [
    "glm-4", // latest
    "glm-3",
  ],
  "settings": {
    /* nested */ "temperature": 0.7,
  },
}"#;
        let (_dir, path) = scratch("fixture.jsonc");
        std::fs::write(&path, input).unwrap();
        let map = load(&path).unwrap();
        assert_eq!(map["provider"], Value::String("glm".into()));
        let models = map["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
    }
}
