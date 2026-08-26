use std::path::Path;

use serde::de::{self, Deserialize, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::backup::backup;
use crate::error::{ConfigError, Result};

// ---------------------------------------------------------------------------
// Strict JSON parsing — duplicate key detection + float/int preservation
// ---------------------------------------------------------------------------

/// Wrapper that deserializes any JSON value but rejects duplicate object keys.
///
/// `serde_json::Map` with `preserve_order` keeps the last value for a duplicate,
/// so we need a custom visitor that errors on duplicates. This wrapper also
/// preserves the numeric type (i64 / u64 / f64) via `Number` so `1` and `1.0`
/// remain distinct (DOC-03: avoid float coercion).
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

/// Parse `text` strictly: reject duplicate keys, preserve number types, reject
/// trailing content after the top-level value.
///
/// Used by both `json` and `jsonc` (after stripping). Errors are mapped to
/// `ConfigError::Json` by the caller.
fn parse_strict(text: &str, path: &Path) -> Result<Value> {
    let mut de = serde_json::Deserializer::from_str(text);
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

/// Parse `text` strictly and return the raw `serde_json::Error` for testing.
#[cfg(test)]
fn parse_strict_raw(text: &str) -> std::result::Result<Value, serde_json::Error> {
    let mut de = serde_json::Deserializer::from_str(text);
    let v = StrictValue::deserialize(&mut de)?;
    de.end()?;
    Ok(v.0)
}

// ---------------------------------------------------------------------------
// Public API — strict JSON (DOC-03)
// ---------------------------------------------------------------------------

/// Read a JSON config fresh from disk. A missing file reads as an empty object.
///
/// Strict: duplicate keys are rejected, `1` vs `1.0` are kept distinct,
/// and key order is preserved (`serde_json/preserve_order`). The root must be
/// an object; use [`load_value`] for raw reads that preserve an arbitrary root
/// type. Each call reads the file fresh (disk is the truth).
pub fn load(path: &Path) -> Result<Map<String, Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(ConfigError::io(path, e)),
    };

    // Empty or whitespace-only file is treated as empty object for compatibility.
    if text.trim().is_empty() {
        return Ok(Map::new());
    }

    let value = parse_strict(&text, path)?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(ConfigError::NotAnObject {
            path: path.to_path_buf(),
        }),
    }
}

/// Read a JSON config fresh from disk, preserving an arbitrary root type.
///
/// Strict duplicate-key handling as in [`load`]. A missing file reads as an
/// empty object (to keep `load`/`load_value` consistent); callers that need
/// to distinguish missing from empty should check `path.exists()` before
/// calling.
pub fn load_value(path: &Path) -> Result<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Object(Map::new())),
        Err(e) => return Err(ConfigError::io(path, e)),
    };

    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }

    parse_strict(&text, path)
}

/// Back up, then write `config` to `path`, creating parent directories as needed.
///
/// The file is written as pretty-printed JSON with a trailing newline.
/// Lexical preservation guarantee: key order and unknown values are preserved
/// (via `preserve_order`), but whitespace and indentation are normalized. For
/// a no-op (semantic value unchanged) callers should prefer [`edit`] which
/// skips the write entirely and leaves the original bytes untouched (DOC-03).
pub fn store(path: &Path, config: &Map<String, Value>) -> Result<()> {
    store_value(path, &Value::Object(config.clone()))
}

/// Back up, then write an arbitrary JSON `value` to `path`.
///
/// See [`store`] for the lexical guarantee. This entry point preserves a
/// non-object root (array, string, number, bool, null) for raw-editor use.
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

/// Read fresh, apply `edit`, write back only if the value changed.
///
/// This is the only supported way to mutate a config. Disk is the truth:
/// nothing is cached between calls. For no-op edits (the closure leaves the
/// map equal to the on-disk value) no write and no backup are performed, so
/// the file's byte identity is preserved (DOC-03).
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

/// Read fresh as [`Value`], apply `edit`, write back only if changed.
///
/// Preserves an arbitrary root type. Duplicate keys in the original file are
/// still rejected on load. No-op edits leave the file byte-identical.
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
        let dir = std::env::temp_dir().join("superai-config-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let path = scratch("absent.json");
        drop(std::fs::remove_file(&path));
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn load_value_missing_is_empty_object() {
        let path = scratch("absent_value.json");
        drop(std::fs::remove_file(&path));
        assert_eq!(load_value(&path).unwrap(), Value::Object(Map::new()));
    }

    #[test]
    fn edit_preserves_unmodelled_keys_and_their_order() {
        let path = scratch("roundtrip.json");
        std::fs::write(&path, r#"{"zzz":1,"model":"opus","aaa":{"nested":true}}"#).unwrap();

        edit(&path, |c| {
            c.insert("model".into(), Value::String("sonnet".into()));
        })
        .unwrap();

        let after = load(&path).unwrap();
        let keys: Vec<&str> = after.keys().map(String::as_str).collect();
        assert_eq!(keys, ["zzz", "model", "aaa"]);
        assert_eq!(after["model"], Value::String("sonnet".into()));
        assert_eq!(after["aaa"]["nested"], Value::Bool(true));
    }

    #[test]
    fn writing_leaves_a_backup_of_the_previous_contents() {
        let path = scratch("backed-up.json");
        std::fs::write(&path, r#"{"model":"opus"}"#).unwrap();

        edit(&path, |c| {
            c.insert("model".into(), Value::String("sonnet".into()));
        })
        .unwrap();

        let backups: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("backed-up.json.bak.")
            })
            .collect();
        assert!(!backups.is_empty(), "no backup written");

        let restored = std::fs::read_to_string(backups[0].path()).unwrap();
        assert!(restored.contains("opus"));
        for b in backups {
            drop(std::fs::remove_file(b.path()));
        }
    }

    #[test]
    fn strict_rejects_duplicate_keys_top_level() {
        let err = parse_strict_raw(r#"{"a":1,"a":2}"#).unwrap_err();
        assert!(err.to_string().contains("duplicate key"), "{err}");
    }

    #[test]
    fn strict_rejects_duplicate_keys_nested() {
        let err = parse_strict_raw(r#"{"outer":{"x":1,"x":2}}"#).unwrap_err();
        assert!(err.to_string().contains("duplicate key"), "{err}");
    }

    #[test]
    fn strict_rejects_duplicate_keys_in_array_objects() {
        let err = parse_strict_raw(r#"[{"a":1,"a":2}]"#).unwrap_err();
        assert!(err.to_string().contains("duplicate key"), "{err}");
    }

    #[test]
    fn duplicate_keys_with_unicode_escape_are_detected() {
        // \u0061 decodes to 'a', so {"\u0061":1,"a":2} is a duplicate.
        let err = parse_strict_raw("{\"\\u0061\":1,\"a\":2}").unwrap_err();
        assert!(err.to_string().contains("duplicate key"), "{err}");
    }

    #[test]
    fn load_rejects_duplicate_keys_on_disk() {
        let path = scratch("dup.json");
        std::fs::write(&path, r#"{"a":1,"a":2}"#).unwrap();
        let err = load(&path).unwrap_err();
        match err {
            ConfigError::Json { source, .. } => {
                assert!(source.to_string().contains("duplicate key"));
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn strict_allows_valid_json() {
        let v = parse_strict_raw(r#"{"a":1,"b":[1,2,3]}"#).unwrap();
        assert_eq!(v["a"], Value::Number(Number::from(1)));
    }

    #[test]
    fn float_and_int_are_distinct() {
        let int_val = parse_strict_raw(r#"{"n":1}"#).unwrap();
        let float_val = parse_strict_raw(r#"{"n":1.0}"#).unwrap();
        let int_n = int_val["n"].as_number().unwrap();
        let float_n = float_val["n"].as_number().unwrap();
        assert!(int_n.is_i64());
        assert!(float_n.is_f64());
        assert_ne!(int_n, float_n);
        // Ensure pretty output keeps distinction: 1 vs 1.0
        let int_str = serde_json::to_string(int_val.get("n").unwrap()).unwrap();
        let float_str = serde_json::to_string(float_val.get("n").unwrap()).unwrap();
        assert_eq!(int_str, "1");
        assert_eq!(float_str, "1.0");
    }

    #[test]
    fn deterministic_insertion_appends() {
        let path = scratch("insertion.json");
        std::fs::write(&path, r#"{"a":1,"b":2}"#).unwrap();
        edit(&path, |c| {
            c.insert("c".into(), Value::Number(Number::from(3)));
            c.insert("d".into(), Value::Number(Number::from(4)));
        })
        .unwrap();
        let after = load(&path).unwrap();
        let keys: Vec<&str> = after.keys().map(String::as_str).collect();
        assert_eq!(keys, ["a", "b", "c", "d"]);
    }

    #[test]
    fn load_value_preserves_arbitrary_root_types() {
        let path = scratch("root_array.json");
        std::fs::write(&path, "[1,2,3]").unwrap();
        let v = load_value(&path).unwrap();
        assert_eq!(
            v,
            Value::Array(vec![
                Value::Number(1.into()),
                Value::Number(2.into()),
                Value::Number(3.into())
            ])
        );

        let path2 = scratch("root_string.json");
        std::fs::write(&path2, r#""hello""#).unwrap();
        assert_eq!(load_value(&path2).unwrap(), Value::String("hello".into()));

        let path3 = scratch("root_number.json");
        std::fs::write(&path3, "42").unwrap();
        assert_eq!(load_value(&path3).unwrap(), Value::Number(Number::from(42)));

        let path4 = scratch("root_bool.json");
        std::fs::write(&path4, "true").unwrap();
        assert_eq!(load_value(&path4).unwrap(), Value::Bool(true));
    }

    #[test]
    fn load_rejects_non_object_root() {
        let path = scratch("non_object.json");
        std::fs::write(&path, "[1,2,3]").unwrap();
        let err = load(&path).unwrap_err();
        match err {
            ConfigError::NotAnObject { .. } => {}
            other => panic!("expected NotAnObject, got {other:?}"),
        }
    }

    #[test]
    fn no_op_preserves_byte_identity() {
        let path = scratch("noop.json");
        let original = "{\n  \"a\": 1,\n  \"b\": 2\n}\n";
        std::fs::write(&path, original).unwrap();
        let before = std::fs::read(&path).unwrap();

        // Edit with no semantic change must leave bytes identical and create no backup.
        edit(&path, |_| {}).unwrap();

        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after, "no-op must be byte identical");

        // Ensure no backup was created for no-op.
        let backups: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("noop.json.bak.")
            })
            .collect();
        assert!(backups.is_empty(), "no-op should not create backup");
        for b in backups {
            drop(std::fs::remove_file(b.path()));
        }
    }

    #[test]
    fn no_op_with_minified_input_is_preserved() {
        let path = scratch("noop_min.json");
        let original = r#"{"zz":1,"aa":2}"#;
        std::fs::write(&path, original).unwrap();
        let before = std::fs::read(&path).unwrap();
        edit(&path, |_| {}).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn comments_are_rejected_in_strict_json() {
        let err = parse_strict_raw(
            r#"{"a":1 // comment
}"#,
        )
        .unwrap_err();
        // serde_json error for comment
        assert!(!err.to_string().contains("duplicate"));
    }

    #[test]
    fn trailing_commas_are_rejected_in_strict_json() {
        let err = parse_strict_raw(r#"{"a":1,}"#).unwrap_err();
        assert!(err.to_string().contains("trailing comma") || err.to_string().contains("expected"));
    }

    #[test]
    fn store_value_preserves_root_type() {
        let path = scratch("store_raw.json");
        drop(std::fs::remove_file(&path));
        let val = Value::Array(vec![Value::Number(1.into()), Value::Number(2.into())]);
        store_value(&path, &val).unwrap();
        let loaded = load_value(&path).unwrap();
        assert_eq!(loaded, val);
    }
}
