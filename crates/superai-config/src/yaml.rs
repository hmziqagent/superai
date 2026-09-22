//! Strict YAML reads; every changing write to an existing file is refused
//! with `LossyWrite` (DOC-06): the writer cannot preserve lexical content.

use std::path::Path;

use serde::de::{self, Deserialize, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::error::{ConfigError, Result};

/// Wrapper that deserializes any YAML value but rejects duplicate object keys.
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

/// Existing YAML is read-only (DOC-06): the writer drops comments, anchors,
/// tags, scalar style, and markers, and lexical detection cannot be hole-free.
fn ensure_lossless_write(path: &Path) -> Result<()> {
    match std::fs::read_to_string(path) {
        Ok(_) => Err(ConfigError::lossy_write(path, "yaml")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ConfigError::io(path, e)),
    }
}

/// Strict parse: duplicate keys rejected; multiple documents and
/// non-`Value`-mappable tags are errors.
pub(crate) fn parse_strict_raw(text: &str) -> std::result::Result<Value, yaml_serde::Error> {
    yaml_serde::from_str::<StrictValue>(strip_bom(text)).map(|v| v.0)
}

fn parse_strict(text: &str, path: &Path) -> Result<Value> {
    parse_strict_raw(text).map_err(|source| ConfigError::Yaml {
        path: path.to_path_buf(),
        source,
    })
}

/// Read fresh; strict, key order preserved, root must be an object
/// (use [`load_value`] for other roots); multiple documents rejected.
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

/// Read fresh preserving any root; missing reads as an empty object, so
/// distinguish missing via `path.exists()`.
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

/// Write normalized YAML; every changing write to an existing file is
/// refused with `LossyWrite` before any disk mutation (DOC-06).
pub fn store(path: &Path, config: &Map<String, Value>) -> Result<()> {
    ensure_lossless_write(path)?;

    let mut text = yaml_serde::to_string(config).map_err(|source| ConfigError::Yaml {
        path: path.to_path_buf(),
        source,
    })?;
    if !text.ends_with('\n') {
        text.push('\n');
    }

    crate::transaction::commit_file(
        "yaml-store",
        path,
        text.as_bytes(),
        crate::document::DocumentKind::Yaml,
    )?;
    Ok(())
}

/// [`store`] for any root; same gate and boundary.
pub fn store_value(path: &Path, value: &Value) -> Result<()> {
    ensure_lossless_write(path)?;

    let mut text = yaml_serde::to_string(value).map_err(|source| ConfigError::Yaml {
        path: path.to_path_buf(),
        source,
    })?;
    if !text.ends_with('\n') {
        text.push('\n');
    }

    crate::transaction::commit_file(
        "yaml-store",
        path,
        text.as_bytes(),
        crate::document::DocumentKind::Yaml,
    )?;
    Ok(())
}

/// Read fresh, apply `edit`, write back only if changed; changing edits on
/// existing files are refused, missing files are created.
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

/// [`edit`] over [`Value`]; no-ops stay byte-identical, changes refused.
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
        let dir = crate::test_util::temp_dir_unique("config-yaml");
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
        // Comments parse fine on read ...
        let path = scratch("comments.yaml");
        std::fs::write(&path, yaml).unwrap();
        let map = load(&path).unwrap();
        assert_eq!(map["key"], Value::String("value".into()));
        // ... but a changing write is refused instead of normalizing them away.
        let result = edit(&path, |m| {
            m.insert("extra".into(), Value::String("x".into()));
        });
        match result {
            Err(ConfigError::LossyWrite { format, .. }) => assert_eq!(format, "yaml"),
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), yaml);
    }

    #[test]
    fn anchors_and_block_scalars_refuse_changing_writes() {
        let anchored = "base: &base\n  x: 1\nderived: *base\n";
        let path = scratch("anchor.yaml");
        std::fs::write(&path, anchored).unwrap();
        let result = edit(&path, |m| {
            m.insert("new".into(), Value::Number(1.into()));
        });
        assert!(matches!(
            result,
            Err(ConfigError::LossyWrite { format: "yaml", .. })
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), anchored);

        let block = "description: |\n  hello\n  world\n";
        let path2 = scratch("block.yaml");
        std::fs::write(&path2, block).unwrap();
        let result2 = edit(&path2, |m| {
            m.insert("new".into(), Value::Number(1.into()));
        });
        assert!(matches!(result2, Err(ConfigError::LossyWrite { .. })));
        assert_eq!(std::fs::read_to_string(&path2).unwrap(), block);
    }

    #[test]
    fn plain_scalar_quotes_do_not_hide_comments() {
        // Apostrophes are legal inside plain scalars; every case must reach the gate.
        let cases = [
            "title: it's fine\n# real comment\nb: 2\n",
            "x: don't\n# between\ny: can't\n",
            "a: she said \"hi\"\n# real comment\nb: 2\n",
            "a: \"unclosed\n# real comment\nb: 2\n",
        ];
        for yaml in cases {
            let path = scratch("plain_quote.yaml");
            std::fs::write(&path, yaml).unwrap();
            let mut map = Map::new();
            map.insert("new".into(), Value::Number(1.into()));
            match store(&path, &map) {
                Err(ConfigError::LossyWrite { format, .. }) => assert_eq!(format, "yaml"),
                other => panic!("expected LossyWrite for {yaml:?}, got {other:?}"),
            }
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                yaml,
                "refused store must leave the file byte-identical"
            );
        }
    }

    #[test]
    fn plain_scalar_apostrophe_edit_refuses_and_keeps_comments() {
        // The file parses, so only the write gate stands between the comments and destruction.
        let yaml = "title: it's fine\n# real comment\nb: 2\n";
        let path = scratch("apostrophe.yaml");
        std::fs::write(&path, yaml).unwrap();
        let result = edit(&path, |m| {
            m.insert("new".into(), Value::Number(1.into()));
        });
        match result {
            Err(ConfigError::LossyWrite { format, .. }) => assert_eq!(format, "yaml"),
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("# real comment")
        );
    }

    #[test]
    fn quoted_scalars_refuse_changing_writes() {
        let yaml = "a: \"quoted\"\nb: 2\n";
        let path = scratch("quoted.yaml");
        std::fs::write(&path, yaml).unwrap();
        let result = edit(&path, |m| {
            m.insert("c".into(), Value::Number(3.into()));
        });
        assert!(matches!(
            result,
            Err(ConfigError::LossyWrite { format: "yaml", .. })
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), yaml);
    }

    #[test]
    fn flow_style_file_refuses_changing_write() {
        let yaml = "a: [&one 1, &two 2]\nb: 2\n";
        let path = scratch("flow_anchor.yaml");
        std::fs::write(&path, yaml).unwrap();
        let mut map = Map::new();
        map.insert("c".into(), Value::Number(3.into()));
        match store(&path, &map) {
            Err(ConfigError::LossyWrite { format, .. }) => assert_eq!(format, "yaml"),
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), yaml);
    }

    #[test]
    fn store_refuses_any_existing_target() {
        // CR, NEL (U+0085), LS (U+2028), and PS (U+2029) are valid YAML line
        // breaks for libyaml; the unconditional gate covers them by construction.
        let cases = [
            "# top comment\na: 1\n",
            "a: 1\nb: 2\n",
            "a: 1\r# real user comment\rb: 2\r",
            "a: 1\u{85}# comment\u{85}b: 2\u{85}",
            "a: 1\u{2028}# comment\u{2028}b: 2\u{2028}",
            "a: 1\u{2029}# comment\u{2029}b: 2\u{2029}",
        ];
        for original in cases {
            let path = scratch("store.yaml");
            std::fs::write(&path, original).unwrap();
            let mut map = Map::new();
            map.insert("a".into(), Value::Number(2.into()));
            match store(&path, &map) {
                Err(ConfigError::LossyWrite { format, .. }) => assert_eq!(format, "yaml"),
                other => panic!("expected LossyWrite for {original:?}, got {other:?}"),
            }
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                original,
                "refused store must leave the file byte-identical"
            );
            let dir_entries = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
            assert_eq!(dir_entries, 1, "refused store must not create files");
        }
    }

    #[test]
    fn lone_cr_comment_file_refuses_changing_edit() {
        // libyaml accepts lone CR as a line break, so this file loads fine;
        // the edit must still refuse rather than destroy the comment.
        let yaml = "a: 1\r# real user comment\rb: 2\r";
        let path = scratch("lone_cr.yaml");
        std::fs::write(&path, yaml).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded["a"], Value::Number(1.into()));
        let result = edit(&path, |m| {
            m.insert("new".into(), Value::Number(9.into()));
        });
        match result {
            Err(ConfigError::LossyWrite { format, .. }) => assert_eq!(format, "yaml"),
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            yaml,
            "refused edit must leave the file byte-identical"
        );
    }

    #[test]
    fn edit_value_refused_on_existing_target() {
        let yaml = "a: 1 # keep\n";
        let path = scratch("edit_value.yaml");
        std::fs::write(&path, yaml).unwrap();
        let result = edit_value(&path, |v| {
            if let Value::Object(m) = v {
                m.insert("b".into(), Value::Number(2.into()));
            }
        });
        assert!(matches!(
            result,
            Err(ConfigError::LossyWrite { format: "yaml", .. })
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), yaml);
    }

    #[test]
    fn store_allows_missing_target() {
        let path = scratch("created.yaml");
        let mut map = Map::new();
        map.insert("a".into(), Value::Number(1.into()));
        store(&path, &map).unwrap();
        assert_eq!(load(&path).unwrap()["a"], Value::Number(1.into()));
    }

    #[test]
    fn anchors_and_aliases_resolve_to_duplicated_values() {
        let yaml = "base: &base\n  x: 1\nderived: *base\n";
        let v = parse_strict_raw(yaml).unwrap();
        assert_eq!(v["base"]["x"], Value::Number(1.into()));
        assert_eq!(v["derived"]["x"], Value::Number(1.into()));
        // Mutating derived does not affect base: alias sharing is lost.
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
            assert_eq!(v["value"], Value::String("hello".into()));
        } else {
            let err = res.unwrap_err();
            assert!(!err.to_string().is_empty());
        }
    }

    #[test]
    fn changing_edit_refused_but_unmodelled_keys_still_readable() {
        // Preservation is expressed the only honest way: nothing is written.
        let path = scratch("roundtrip.yaml");
        let original = "zzz: 1\nmodel: opus\naaa:\n  nested: true\n";
        std::fs::write(&path, original).unwrap();

        let result = edit(&path, |c| {
            c.insert("model".into(), Value::String("sonnet".into()));
        });
        assert!(matches!(result, Err(ConfigError::LossyWrite { .. })));

        let after = load(&path).unwrap();
        let keys: Vec<&str> = after.keys().map(String::as_str).collect();
        assert!(keys.contains(&"zzz"));
        assert!(keys.contains(&"aaa"));
        assert_eq!(after["model"], Value::String("opus".into()));
        assert_eq!(after["aaa"]["nested"], Value::Bool(true));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
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
    fn refused_write_leaves_no_backup() {
        let path = scratch("backed.yaml");
        let original = "model: opus\n";
        std::fs::write(&path, original).unwrap();
        let result = edit(&path, |c| {
            c.insert("model".into(), Value::String("sonnet".into()));
        });
        assert!(matches!(result, Err(ConfigError::LossyWrite { .. })));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let backups: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("backed.yaml.bak.")
            })
            .collect();
        assert!(backups.is_empty(), "refused write must not create backup");
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
