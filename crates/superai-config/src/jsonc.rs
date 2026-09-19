//! JSONC: JSON with comments and trailing commas.
//!
//! Read support is comment/trailing-comma stripping before the strict
//! `serde_json` parse (duplicate keys still rejected). Per DOC-05 a codec
//! that cannot preserve lexical content must not perform changing writes, so
//! a write is refused with
//! [`ConfigError::LossyWrite`](crate::error::ConfigError::LossyWrite) unless
//! it is provably lossless: the target is missing (creation) or its bytes
//! carry no JSONC extensions (`strip_jsonc(bytes) == bytes`). No-op edits
//! never write and keep byte identity.

use std::path::Path;

use serde_json::{Map, Value};

use crate::error::{ConfigError, Result};

/// Strip trailing commas before `}` or `]`, string-aware.
///
/// A comma followed only by whitespace and then `}` or `]` is a trailing
/// comma. Commas inside strings are ignored. Byte-oriented scan: JSON
/// structure characters are ASCII, and UTF-8 continuation bytes never alias
/// them, so this allocates nothing beyond the output.
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
pub(crate) fn strip_jsonc(input: &str) -> String {
    strip_trailing_commas(&crate::document::strip_jsonc_comments(input))
}

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

    let value = crate::json::parse_strict(&strip_jsonc(&text), path)?;
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

    crate::json::parse_strict(&strip_jsonc(&text), path)
}

/// Refuse writes that would destroy JSONC lexical material already on disk.
///
/// A file whose bytes equal their stripped form carries no comments and no
/// trailing commas, so normalized output preserves its entire lexical content.
/// Missing files are writable (nothing to destroy); files that cannot be read
/// as UTF-8 are refused because preservation cannot be proven.
fn ensure_lossless_write(path: &Path) -> Result<()> {
    match std::fs::read_to_string(path) {
        Ok(text) if strip_jsonc(&text) != text => Err(ConfigError::lossy_write(path, "jsonc")),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ConfigError::io(path, e)),
    }
}

/// Back up, then write `config` to `path`.
///
/// Changing writes are refused with [`ConfigError::LossyWrite`] when `path`
/// already exists and carries JSONC lexical material (comments or trailing
/// commas): normalized pretty JSON cannot preserve it (DOC-05). Missing files
/// are created, and extension-free files are rewritten losslessly. Key order
/// and unknown values are preserved.
pub fn store(path: &Path, config: &Map<String, Value>) -> Result<()> {
    ensure_lossless_write(path)?;

    let mut text = serde_json::to_string_pretty(config).map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    text.push('\n');

    crate::transaction::commit_file(
        "jsonc-store",
        path,
        text.as_bytes(),
        crate::document::DocumentKind::JsonC,
    )?;
    Ok(())
}

/// Back up, then write an arbitrary `value` to `path` as normalized JSON.
///
/// Same gate as [`store`]; this entry point preserves a non-object root for
/// raw-editor use.
pub fn store_value(path: &Path, value: &Value) -> Result<()> {
    ensure_lossless_write(path)?;

    let mut text = serde_json::to_string_pretty(value).map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    text.push('\n');

    crate::transaction::commit_file(
        "jsonc-store",
        path,
        text.as_bytes(),
        crate::document::DocumentKind::JsonC,
    )?;
    Ok(())
}

/// Read fresh JSONC, apply `edit`, write back only if changed.
///
/// No-op edits leave the file byte-identical (no write occurs). Changing
/// edits follow the [`store`] lossless-write gate.
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
///
/// See [`edit`] for the lossless-write gate.
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

/// DOC-10: disclosure when a changing write must reformat surrounding layout.
///
/// Files carrying JSONC material have changing writes refused, so they never
/// reformat. Extension-free files are written as normalized pretty JSON; the
/// warning fires when such a file is not already in that form.
pub fn formatting_change_warning(text: &str) -> Option<&'static str> {
    if text.trim().is_empty() || strip_jsonc(text) != text {
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

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = crate::test_util::temp_dir_unique("config-jsonc");
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
    fn changing_edit_refused_preserving_comments() {
        let path = scratch("change.jsonc");
        let original = "{\"a\":1, // c\n}";
        std::fs::write(&path, original).unwrap();
        let result = edit(&path, |m| {
            m.insert("b".into(), Value::Number(2.into()));
        });
        match result {
            Err(ConfigError::LossyWrite { format, .. }) => assert_eq!(format, "jsonc"),
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        // Refusal must not touch the file: comment and trailing comma survive.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn changing_edit_value_refused_on_comments() {
        let path = scratch("value.jsonc");
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
        let path = scratch("store.jsonc");
        let original = "{\n  // keep me\n  \"a\": 1,\n}\n";
        std::fs::write(&path, original).unwrap();
        let mut map = Map::new();
        map.insert("a".into(), Value::Number(2.into()));
        match store(&path, &map) {
            Err(ConfigError::LossyWrite { format, .. }) => assert_eq!(format, "jsonc"),
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        // A refused write must not leave a backup either: no disk mutation.
        let dir_entries = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(dir_entries, 1, "refused write must not create files");
    }

    #[test]
    fn store_allows_missing_target_and_extension_free_target() {
        // Creation: there is no lexical material to destroy.
        let path = scratch("new.jsonc");
        let mut map = Map::new();
        map.insert("a".into(), Value::Number(1.into()));
        store(&path, &map).unwrap();
        assert_eq!(load(&path).unwrap()["a"], Value::Number(1.into()));
        drop(std::fs::remove_file(&path));

        // A file whose bytes contain no JSONC extensions is rewritten
        // losslessly: normalized output preserves every lexical feature it has.
        let clean = scratch("clean.json");
        std::fs::write(&clean, "{\"a\":1}").unwrap();
        map.insert("b".into(), Value::Number(2.into()));
        store(&clean, &map).unwrap();
        let loaded = load(&clean).unwrap();
        assert_eq!(loaded["a"], Value::Number(1.into()));
        assert_eq!(loaded["b"], Value::Number(2.into()));
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
