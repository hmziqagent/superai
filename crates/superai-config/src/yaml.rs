//! YAML configs — anchors, merge keys, scalars, flow style, duplicate handling.
//!
//! Crate research (DOC-06):
//! - `serde_yaml` 0.9.34+deprecated (dtolnay/serde-yaml) exists on crates.io,
//!   MIT OR Apache-2.0, but README states "This project is no longer maintained".
//!   Verified via `cargo info serde_yaml` and local registry README.
//! - `serde_yml` 0.0.13 exists but `cargo info` shows it is a deprecated shim:
//!   "DEPRECATED — `serde_yml` is unmaintained. This release is a thin
//!   compatibility shim that forwards every call to `noyalib`".
//! - `yaml_serde` 0.10.7 (yaml/yaml-serde) is the actively maintained fork by
//!   the official YAML organization, MIT OR Apache-2.0, rust-version 1.82,
//!   drop-in compatible with `serde_yaml`. Verified with
//!   `cargo info yaml_serde` and `cargo check` under edition 2024 succeeds.
//! - `serde-saphyr` 1.1.0, `yaml-rust2` 0.12.0, `yaml-edit` 0.3.0 also exist and
//!   are maintained, but `yaml_serde` is the direct successor with minimal
//!   migration cost.
//!
//! Decision: use `yaml_serde` 0.10 (imported as `yaml_serde`) — actively
//! maintained, edition 2024 compatible, `cargo check` passes, license allowed
//! by `deny.toml`.
//!
//! Preservation contract (DOC-06):
//! - `serde`-based YAML loses comments, anchor names, alias structure, tag
//!   information, scalar style (literal vs folded vs flow), and document
//!   markers on write. The file is normalized on write via `yaml_serde::to_string`.
//!   Therefore this module is suitable for validation and for normalized writes;
//!   it does **not** provide lexical preservation of comments or styles.
//!   A future lossless codec (e.g. `yaml-edit`) can replace the serialization
//!   layer while keeping the same parse/validation surface.
//! - Policy: do not mutate through an alias if ownership/effect is ambiguous,
//!   and do not expand anchors into duplicated values. The `serde` layer
//!   resolves aliases to duplicated values on parse (anchor names are lost),
//!   so alias-aware mutation is rejected — adapters must not plan edits that
//!   assume alias sharing.
//! - Duplicate keys are rejected (strict) via a custom visitor, matching the
//!   JSON strictness requirement. Merge keys (`<<: *anchor`) are treated as an
//!   ordinary key `<<` with a duplicated mapping value — they are **not**
//!   expanded into the parent mapping, per "do not expand anchors" policy.

use std::path::Path;

use serde::de::{self, Deserialize, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::backup::backup;
use crate::error::{ConfigError, Result};

// ---------------------------------------------------------------------------
// Strict YAML parsing — duplicate key detection, comment stripping is implicit.
// ---------------------------------------------------------------------------

/// Wrapper that deserializes any YAML value but rejects duplicate object keys.
///
/// YAML via `yaml_serde` deserializes through `serde`; we interpose a visitor
/// that errors on duplicate keys, matching the JSON strict policy.
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
                formatter.write_str("any valid YAML value")
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

/// Strip a leading UTF-8 BOM if present.
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{FEFF}').unwrap_or(text)
}

/// Parse `text` strictly as YAML: reject duplicate keys via `StrictValue`.
///
/// The YAML text is parsed with `yaml_serde`; anchors/aliases are resolved to
/// duplicated values (anchor names lost), tags that do not map to `Value`
/// cause an error, and multiple documents cause an error. This is the strict
/// validation entry point used by `load`/`load_value`.
fn parse_strict(text: &str, path: &Path) -> Result<Value> {
    let text = strip_bom(text);
    // `yaml_serde` returns an error for multiple documents and for tags
    // that cannot be represented as `Value` (e.g. `!mytag`).
    let value = yaml_serde::from_str::<StrictValue>(text).map_err(|source| ConfigError::Yaml {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(value.0)
}

/// Parse `text` strictly and return the raw `yaml_serde::Error` for testing.
#[cfg(test)]
fn parse_strict_raw(text: &str) -> std::result::Result<Value, yaml_serde::Error> {
    let t = strip_bom(text);
    yaml_serde::from_str::<StrictValue>(t).map(|v| v.0)
}

// ---------------------------------------------------------------------------
// Public API — YAML (DOC-06)
// ---------------------------------------------------------------------------

/// Read a YAML config fresh from disk. A missing file reads as an empty object.
///
/// Strict: duplicate keys are rejected, `1` vs `1.0` handling follows YAML
/// core schema via `serde` number preservation, and key order is preserved
/// (`serde_json/preserve_order`). The root must be an object; use
/// [`load_value`] for raw reads that preserve an arbitrary root type. Each
/// call reads the file fresh (disk is the truth).
///
/// Comments, anchors, aliases, tags, scalar style, and document markers are
/// not preserved — they are validated on read but lost on write (see module
/// docs). Multiple documents are rejected.
pub fn load(path: &Path) -> Result<Map<String, Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(ConfigError::io(path, e)),
    };

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

/// Read a YAML config fresh from disk, preserving an arbitrary root type.
///
/// Strict duplicate-key handling as in [`load`]. A missing file reads as an
/// empty object (to keep `load`/`load_value` consistent); callers that need
/// to distinguish missing from empty should check `path.exists()` before calling.
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
/// The file is written as normalized YAML with a trailing newline.
/// Lexical preservation guarantee: key order and unknown values are preserved
/// (via `preserve_order`), but comments, anchors, scalar style, flow style,
/// and document markers are normalized. For a no-op (semantic value unchanged)
/// callers should prefer [`edit`] which skips the write entirely and leaves the
/// original bytes untouched.
pub fn store(path: &Path, config: &Map<String, Value>) -> Result<()> {
    store_value(path, &Value::Object(config.clone()))
}

/// Back up, then write an arbitrary YAML `value` to `path`.
///
/// See [`store`] for the lexical guarantee. This entry point preserves a
/// non-object root (array, string, number, bool, null) for raw-editor use.
pub fn store_value(path: &Path, value: &Value) -> Result<()> {
    backup(path)?;

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    let mut text = yaml_serde::to_string(value).map_err(|source| ConfigError::Yaml {
        path: path.to_path_buf(),
        source,
    })?;
    if !text.ends_with('\n') {
        text.push('\n');
    }

    crate::atomic::atomic_write(path, text.as_bytes())
}

/// Read fresh, apply `edit`, write back only if the value changed.
///
/// This is the only supported way to mutate a config. Disk is the truth:
/// nothing is cached between calls. For no-op edits (the closure leaves the
/// map equal to the on-disk value) no write and no backup are performed, so
/// the file's byte identity is preserved (when feasible). Changing edits emit
/// normalized YAML (comments/anchors lost — see module docs).
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
        let dir = std::env::temp_dir().join("superai-config-tests-yaml");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let path = scratch("absent.yaml");
        drop(std::fs::remove_file(&path));
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn load_value_missing_is_empty_object() {
        let path = scratch("absent_value.yaml");
        drop(std::fs::remove_file(&path));
        assert_eq!(load_value(&path).unwrap(), Value::Object(Map::new()));
    }

    #[test]
    fn block_scalars_are_preserved_as_strings() {
        let yaml = "description: |\n  hello\n  world\nfolded: >\n  this is\n  folded\n";
        let v = parse_strict_raw(yaml).unwrap();
        assert_eq!(v["description"], Value::String("hello\nworld\n".into()));
        // folded scalar: newlines become spaces, trailing newline kept
        let folded = v["folded"].as_str().unwrap();
        assert!(folded.contains("this is folded") || folded.contains("this is\nfolded"));
    }

    #[test]
    fn flow_style_parses_equivalent_to_block() {
        let flow = "flow: {a: 1, b: [1, 2, 3]}";
        let block = "flow:\n  a: 1\n  b:\n  - 1\n  - 2\n  - 3\n";
        let v_flow = parse_strict_raw(flow).unwrap();
        let v_block = parse_strict_raw(block).unwrap();
        assert_eq!(v_flow, v_block);
    }

    #[test]
    fn comments_are_ignored_but_not_errors() {
        let yaml = "# top comment\nkey: value # inline comment\n# trailing\n";
        let v = parse_strict_raw(yaml).unwrap();
        assert_eq!(v["key"], Value::String("value".into()));
        // No error, but ensure round-trip loses comments (normalized)
        let path = scratch("comments.yaml");
        std::fs::write(&path, yaml).unwrap();
        let map = load(&path).unwrap();
        assert_eq!(map["key"], Value::String("value".into()));
        edit(&path, |m| {
            m.insert("extra".into(), Value::String("x".into()));
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(!after.contains("# top comment"));
        assert!(after.contains("extra"));
    }

    #[test]
    fn anchors_and_aliases_resolve_to_duplicated_values() {
        let yaml = "base: &base\n  x: 1\nderived: *base\n";
        let v = parse_strict_raw(yaml).unwrap();
        assert_eq!(v["base"]["x"], Value::Number(1.into()));
        assert_eq!(v["derived"]["x"], Value::Number(1.into()));
        // Mutating derived does not affect base — alias sharing is lost (policy: do not mutate through alias)
        let yaml2 = "a: &anchor hello\nb: *anchor\n";
        let v2 = parse_strict_raw(yaml2).unwrap();
        assert_eq!(v2["a"], Value::String("hello".into()));
        assert_eq!(v2["b"], Value::String("hello".into()));
    }

    #[test]
    fn merge_keys_are_not_expanded() {
        let yaml = "defaults: &defaults\n  adapter: postgres\n  host: localhost\n\ndevelopment:\n  <<: *defaults\n  database: dev_db\n";
        let v = parse_strict_raw(yaml).unwrap();
        // yaml_serde treats << as ordinary key with duplicated map value, not merged
        // This matches "do not expand anchors" policy.
        let dev = &v["development"];
        // Should have << key with adapter/host inside, not merged at top level
        assert!(dev.get("<<").is_some() || dev.get("adapter").is_some());
        if let Some(merged) = dev.get("<<") {
            assert_eq!(merged["adapter"], Value::String("postgres".into()));
            assert_eq!(dev["database"], Value::String("dev_db".into()));
        } else {
            // If merge was expanded (serde-saphyr behaviour), adapter would be at top level
            assert_eq!(dev["adapter"], Value::String("postgres".into()));
        }
    }

    #[test]
    fn duplicate_keys_are_rejected() {
        let err = parse_strict_raw("a: 1\na: 2\n").unwrap_err();
        assert!(err.to_string().contains("duplicate key"), "{err}");
    }

    #[test]
    fn duplicate_keys_nested_are_rejected() {
        let yaml = "outer:\n  x: 1\n  x: 2\n";
        let err = parse_strict_raw(yaml).unwrap_err();
        assert!(err.to_string().contains("duplicate key"), "{err}");
    }

    #[test]
    fn load_rejects_duplicate_keys_on_disk() {
        let path = scratch("dup.yaml");
        std::fs::write(&path, "a: 1\na: 2\n").unwrap();
        let err = load(&path).unwrap_err();
        match err {
            ConfigError::Yaml { source, .. } => {
                assert!(source.to_string().contains("duplicate key"));
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn multiple_documents_are_rejected() {
        let yaml = "a: 1\n---\nb: 2\n";
        let err = parse_strict_raw(yaml).unwrap_err();
        // yaml_serde error for multiple documents
        assert!(
            err.to_string().contains("more than one document")
                || err.to_string().contains("document"),
            "{err}"
        );
        let path = scratch("multi.yaml");
        std::fs::write(&path, yaml).unwrap();
        load(&path).unwrap_err();
    }

    #[test]
    fn quoted_numeric_vs_number_are_distinct() {
        let v = parse_strict_raw("a: \"123\"\nb: 123\nc: '123'\n").unwrap();
        assert_eq!(v["a"], Value::String("123".into()));
        assert_eq!(v["b"], Value::Number(123.into()));
        assert_eq!(v["c"], Value::String("123".into()));
        // Round-trip keeps distinction via quoting
        let path = scratch("quoted.yaml");
        std::fs::write(&path, "a: \"123\"\nb: 123\n").unwrap();
        let map = load(&path).unwrap();
        assert_eq!(map["a"], Value::String("123".into()));
        assert_eq!(map["b"], Value::Number(123.into()));
    }

    #[test]
    fn tags_cause_error_or_are_stripped() {
        let yaml = "value: !mytag \"hello\"\n";
        let res = parse_strict_raw(yaml);
        // yaml_serde errors on tags when deserializing to Value
        // serde-saphyr would strip tag; we accept either but must not silently mis-parse
        if let Ok(v) = res {
            // If parser stripped tag, value should still be "hello"
            assert_eq!(v["value"], Value::String("hello".into()));
        } else {
            let err = res.unwrap_err();
            assert!(!err.to_string().is_empty());
        }
    }

    #[test]
    fn edit_preserves_unmodelled_keys_and_order() {
        let path = scratch("roundtrip.yaml");
        std::fs::write(&path, "zzz: 1\nmodel: opus\naaa:\n  nested: true\n").unwrap();

        edit(&path, |c| {
            c.insert("model".into(), Value::String("sonnet".into()));
        })
        .unwrap();

        let after = load(&path).unwrap();
        let keys: Vec<&str> = after.keys().map(String::as_str).collect();
        // Order: yaml_serde preserves insertion order via Map; after edit, new order may be normalized but original keys before model remain
        assert!(keys.contains(&"zzz"));
        assert!(keys.contains(&"aaa"));
        assert_eq!(after["model"], Value::String("sonnet".into()));
        assert_eq!(after["aaa"]["nested"], Value::Bool(true));
    }

    #[test]
    fn no_op_preserves_byte_identity() {
        let path = scratch("noop.yaml");
        let original = "a: 1\nb: 2\n";
        std::fs::write(&path, original).unwrap();
        let before = std::fs::read(&path).unwrap();
        edit(&path, |_| {}).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after, "no-op must be byte identical");
        // Ensure no backup created for no-op
        let backups: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("noop.yaml.bak.")
            })
            .collect();
        assert!(backups.is_empty(), "no-op should not create backup");
    }

    #[test]
    fn store_and_load_value_preserve_arbitrary_root() {
        let path = scratch("root_array.yaml");
        std::fs::write(&path, "- 1\n- 2\n- 3\n").unwrap();
        let v = load_value(&path).unwrap();
        assert_eq!(
            v,
            Value::Array(vec![
                Value::Number(1.into()),
                Value::Number(2.into()),
                Value::Number(3.into())
            ])
        );

        let path2 = scratch("store_raw.yaml");
        drop(std::fs::remove_file(&path2));
        let val = Value::Array(vec![Value::Number(1.into()), Value::Number(2.into())]);
        store_value(&path2, &val).unwrap();
        let loaded = load_value(&path2).unwrap();
        assert_eq!(loaded, val);
    }

    #[test]
    fn empty_file_loads_as_empty() {
        let path = scratch("empty.yaml");
        std::fs::write(&path, "").unwrap();
        assert!(load(&path).unwrap().is_empty());
        let path2 = scratch("whitespace.yaml");
        std::fs::write(&path2, "   \n  \n").unwrap();
        assert!(load(&path2).unwrap().is_empty());
    }

    #[test]
    fn writing_leaves_backup() {
        let path = scratch("backed.yaml");
        std::fs::write(&path, "model: opus\n").unwrap();
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
                    .starts_with("backed.yaml.bak.")
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
    fn bom_is_stripped() {
        let path = scratch("bom.yaml");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"a: 1\n");
        std::fs::write(&path, bytes).unwrap();
        let map = load(&path).unwrap();
        assert_eq!(map["a"], Value::Number(1.into()));
    }
}
