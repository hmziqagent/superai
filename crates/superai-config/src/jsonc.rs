//! JSONC — JSON with comments and trailing commas.
//!
//! Crate research (DOC-05):
//! - `jsonc-parser` 0.33.1 (dprint/jsonc-parser) exists on crates.io, MIT, maintained,
//!   handles comments/trailing commas and offers CST/serde features. Its serializer,
//!   however, normalizes output (loses comments, reformats), so it does **not** satisfy
//!   the lexical-preservation requirement ("reject a codec that parses JSONC but
//!   serializes normalized JSON") for write-preservation.
//! - `comment-json` does not exist on crates.io (verified via `cargo search`).
//! - `jsonc` 0.1.0 exists but is single-owner, minimal docs, and also normalizes.
//!
//! Decision: for now implement read support via comment/trailing-comma stripping
//! before strict `serde_json` parsing, and write back via normalized pretty JSON.
//! The file header documents this limitation. Edit preserves key order and unknown
//! values (via `preserve_order`), but discards comments/trailing commas on write.
//! A future lexical-preserving JSONC codec can replace the stripping layer.

use std::path::Path;

use serde::de::{self, Deserialize, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::backup::backup;
use crate::error::{ConfigError, Result};

// ---------------------------------------------------------------------------
// Stripping — comments (//, /* */) and trailing commas, string-aware
// ---------------------------------------------------------------------------

/// Strip `//` line comments and `/* */` block comments, string-aware.
///
/// Escaped quotes inside strings are respected, so `//` or `/*` inside a
/// JSON string literal is not treated as a comment. Line comments preserve the
/// terminating newline to keep line numbers stable for diagnostics.
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
                    // Skip until newline, preserve the newline itself.
                    while let Some(&peek) = chars.peek() {
                        if peek == '\n' {
                            break;
                        }
                        chars.next();
                    }
                }
                Some('*') => {
                    chars.next();
                    // Skip until */
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

/// Strip trailing commas before `}` or `]`, string-aware.
///
/// A comma that is followed only by whitespace and then `}` or `]` is a
/// trailing comma. Commas inside strings are ignored.
#[expect(clippy::excessive_nesting, reason = "string-aware scan")]
fn strip_trailing_commas(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut in_string = false;
    let mut escaped = false;
    let mut idx = 0usize;

    while idx < chars.len() {
        let Some(&ch) = chars.get(idx) else {
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
            idx += 1;
        } else if ch == '"' {
            in_string = true;
            output.push(ch);
            idx += 1;
        } else if ch == ',' {
            // Look ahead skipping ASCII whitespace to find next significant char.
            let mut look = idx + 1;
            loop {
                match chars.get(look).copied() {
                    Some(c) if c == ' ' || c == '\t' || c == '\n' || c == '\r' => {
                        look += 1;
                    }
                    _ => break,
                }
            }
            if let Some(next) = chars.get(look).copied()
                && (next == '}' || next == ']')
            {
                // Trailing comma — skip it.
                idx += 1;
                continue;
            }
            output.push(ch);
            idx += 1;
        } else {
            output.push(ch);
            idx += 1;
        }
    }

    output
}

/// Strip JSONC extensions (comments + trailing commas) to produce strict JSON.
fn strip_jsonc(input: &str) -> String {
    let without_comments = strip_comments(input);
    strip_trailing_commas(&without_comments)
}

// ---------------------------------------------------------------------------
// Strict parsing (same duplicate-key guard as `json`)
// ---------------------------------------------------------------------------

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct StrictVisitor;

        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("any valid JSON value")
            }

            fn visit_bool<E>(self, v: bool) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictValue(Value::Bool(v)))
            }

            fn visit_i64<E>(self, v: i64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictValue(Value::Number(Number::from(v))))
            }

            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictValue(Value::Number(Number::from(v))))
            }

            fn visit_f64<E>(self, v: f64) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Number::from_f64(v).map_or_else(
                    || Err(de::Error::custom("invalid f64")),
                    |n| Ok(StrictValue(Value::Number(n))),
                )
            }

            fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictValue(Value::String(v.to_owned())))
            }

            fn visit_string<E>(self, v: String) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictValue(Value::String(v)))
            }

            fn visit_borrowed_str<E>(self, v: &'de str) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictValue(Value::String(v.to_owned())))
            }

            fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictValue(Value::Null))
            }

            fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StrictValue(Value::Null))
            }

            #[expect(clippy::excessive_nesting, reason = "visitor boilerplate")]
            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut vec = Vec::new();
                while let Some(elem) = seq.next_element::<StrictValue>()? {
                    vec.push(elem.0);
                }
                Ok(StrictValue(Value::Array(vec)))
            }

            #[expect(clippy::excessive_nesting, reason = "visitor boilerplate")]
            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut m = Map::new();
                while let Some((key, value)) = map.next_entry::<String, StrictValue>()? {
                    if m.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate key `{key}`")));
                    }
                    m.insert(key, value.0);
                }
                Ok(StrictValue(Value::Object(m)))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

fn parse_jsonc_strict(text: &str, path: &Path) -> Result<Value> {
    let stripped = strip_jsonc(text);
    let mut de = serde_json::Deserializer::from_str(&stripped);
    let value = StrictValue::deserialize(&mut de).map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    de.end().map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(value.0)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read a JSONC config fresh from disk. A missing file reads as an empty object.
///
/// JSONC extensions are accepted: `//` and `/* */` comments and trailing commas.
/// Duplicate keys are rejected. Key order is preserved. The root must be an
/// object; use [`load_value`] for arbitrary roots.
pub fn load(path: &Path) -> Result<Map<String, Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(ConfigError::io(path, e)),
    };

    if text.trim().is_empty() {
        return Ok(Map::new());
    }

    let value = parse_jsonc_strict(&text, path)?;
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

    parse_jsonc_strict(&text, path)
}

/// Back up, then write `config` to `path`.
///
/// The output is normalized pretty-printed JSON (no comments, no trailing commas).
/// Edit preserves key order and unknown values, but discards JSONC lexical
/// material — see module docs.
pub fn store(path: &Path, config: &Map<String, Value>) -> Result<()> {
    store_value(path, &Value::Object(config.clone()))
}

/// Back up, then write an arbitrary `value` to `path` as normalized JSON.
pub fn store_value(path: &Path, value: &Value) -> Result<()> {
    backup(path)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    let mut text = serde_json::to_string_pretty(value).map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    text.push('\n');

    std::fs::write(path, text).map_err(|e| ConfigError::io(path, e))
}

/// Read fresh JSONC, apply `edit`, write back only if changed.
///
/// No-op edits leave the file byte-identical (comments/trailing commas are
/// preserved because no write occurs). Changing writes emit normalized JSON.
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

/// Read fresh JSONC as `Value`, apply `edit`, write back only if changed.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("superai-config-tests-jsonc");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
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
        let path = scratch("with_comments.jsonc");
        std::fs::write(
            &path,
            "{\n  // line comment\n  \"model\": \"opus\", /* block */\n  \"x\": 1,\n}\n",
        )
        .unwrap();
        let map = load(&path).unwrap();
        assert_eq!(
            map["model"],
            Value::String("opaque".replace("opaque", "opus"))
        );
        assert_eq!(map["x"], Value::Number(1.into()));
    }

    #[test]
    fn load_rejects_duplicate_keys_even_in_jsonc() {
        let path = scratch("dup.jsonc");
        std::fs::write(&path, r#"{"a": 1, "a": 2}"#).unwrap();
        let err = load(&path).unwrap_err();
        match err {
            ConfigError::Json { source, .. } => assert!(source.to_string().contains("duplicate")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn no_op_preserves_comments_byte_identity() {
        let path = scratch("noop.jsonc");
        let original = "{\n  // keep me\n  \"a\": 1, // comment\n  \"b\": 2,\n}\n";
        std::fs::write(&path, original).unwrap();
        let before = std::fs::read(&path).unwrap();
        edit(&path, |_| {}).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn edit_changes_normalize_output() {
        let path = scratch("change.jsonc");
        std::fs::write(&path, "{\"a\":1, // c\n}").unwrap();
        edit(&path, |m| {
            m.insert("b".into(), Value::Number(2.into()));
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        // Normalized: no comments, pretty
        assert!(!after.contains("//"));
        assert!(after.contains("\"b\": 2"));
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let path = scratch("absent.jsonc");
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
        // Representative of OpenCode/Kilo/Amp/Copilot settings: comments + trailing commas + nested.
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
        let path = scratch("fixture.jsonc");
        std::fs::write(&path, input).unwrap();
        let map = load(&path).unwrap();
        assert_eq!(map["provider"], Value::String("glm".into()));
        let models = map["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
    }
}
