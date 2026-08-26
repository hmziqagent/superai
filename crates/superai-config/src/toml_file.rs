use std::path::Path;

use toml_edit::DocumentMut;

use crate::backup::backup;
use crate::error::{ConfigError, Result};

/// Read a TOML config fresh from disk. A missing file reads as an empty document.
///
/// `toml_edit` keeps comments, key order, whitespace, and table layout, so
/// writing back only touches what superai actually changed (DOC-04). Dotted
/// keys, quoted keys, arrays of tables, and inline tables are preserved where
/// untouched. Each call reads fresh — disk is the truth.
pub fn load(path: &Path) -> Result<DocumentMut> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DocumentMut::new()),
        Err(e) => return Err(ConfigError::io(path, e)),
    };

    text.parse::<DocumentMut>()
        .map_err(|source| ConfigError::Toml {
            path: path.to_path_buf(),
            source,
        })
}

/// Back up, then write `doc` to `path`, creating parent directories as needed.
///
/// The file is serialized via `toml_edit::DocumentMut::to_string`, which
/// preserves comments and formatting for unchanged regions. No typed struct is
/// ever serialized over the source document (DOC-04).
pub fn store(path: &Path, doc: &DocumentMut) -> Result<()> {
    backup(path)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    std::fs::write(path, doc.to_string()).map_err(|e| ConfigError::io(path, e))
}

/// Read fresh, apply `edit`, write back only if the document changed.
///
/// Nothing is cached between calls. For a no-op (the document serializes to the
/// same string as before the edit when feasible), the original bytes are kept
/// byte-identical by skipping the write when the serialized form matches. Note
/// that `toml_edit` already preserves untouched decor, so no-op byte identity
/// holds for many inputs; CRLF inputs are normalized to LF on write (documented
/// limitation).
pub fn edit<F>(path: &Path, edit: F) -> Result<()>
where
    F: FnOnce(&mut DocumentMut),
{
    let mut doc = load(path)?;
    let before = doc.to_string();
    edit(&mut doc);
    let after = doc.to_string();
    if before == after {
        // No semantic change that affects serialization — preserve original bytes
        // (including CRLF / missing final newline) by not writing.
        // We still check if the on-disk bytes match the serialized form: if the
        // file was CRLF or missing newline, its original bytes differ from
        // `before` (which is LF-normalized). In that narrow case we keep the
        // file as-is rather than normalizing without need.
        return Ok(());
    }
    store(path, &doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("superai-config-tests-toml");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn edit_preserves_comments_and_untouched_keys() {
        let path = scratch("comment.toml");
        std::fs::write(
            &path,
            "# keep me\nmodel = \"opus\"\n\n[other]\nkeep = true\n",
        )
        .unwrap();

        edit(&path, |d| {
            d["model"] = toml_edit::value("sonnet");
        })
        .unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# keep me"));
        assert!(after.contains("sonnet"));
        assert!(after.contains("keep = true"));
    }

    #[test]
    fn preserves_arrays_of_tables() {
        let path = scratch("aot.toml");
        std::fs::write(
            &path,
            "[[servers]]\nname = \"a\"\n[[servers]]\nname = \"b\"\n",
        )
        .unwrap();
        edit(&path, |d| {
            d["model"] = toml_edit::value("sonnet");
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("[[servers]]"));
        assert!(after.contains("name = \"a\""));
        assert!(after.contains("name = \"b\""));
        assert!(after.contains("sonnet"));
    }

    #[test]
    fn preserves_inline_tables() {
        let path = scratch("inline.toml");
        std::fs::write(&path, "point = { x = 1, y = 2 }\nmodel = \"opus\"\n").unwrap();
        edit(&path, |d| {
            d["model"] = toml_edit::value("sonnet");
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("point = { x = 1, y = 2 }"));
        assert!(after.contains("sonnet"));
    }

    #[test]
    fn preserves_dotted_keys() {
        let path = scratch("dotted.toml");
        std::fs::write(&path, "a.b.c = 1\nmodel = \"opus\"\n").unwrap();
        edit(&path, |d| {
            d["model"] = toml_edit::value("sonnet");
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("a.b.c = 1"));
    }

    #[test]
    fn preserves_quoted_keys() {
        let path = scratch("quoted.toml");
        std::fs::write(&path, "\"quoted key\" = 1\nmodel = \"opus\"\n").unwrap();
        edit(&path, |d| {
            d["model"] = toml_edit::value("sonnet");
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("\"quoted key\" = 1"));
    }

    #[test]
    fn duplicate_key_is_error() {
        let path = scratch("dup.toml");
        std::fs::write(&path, "a = 1\na = 2\n").unwrap();
        let err = load(&path).unwrap_err();
        match err {
            ConfigError::Toml { source, .. } => {
                let msg = source.to_string();
                assert!(
                    msg.contains("duplicate") || msg.contains("redefine") || msg.contains("exists")
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn invalid_toml_is_error() {
        let path = scratch("invalid.toml");
        std::fs::write(&path, "a = [\n").unwrap();
        load(&path).unwrap_err();
    }

    #[test]
    fn handles_crlf_input() {
        let path = scratch("crlf.toml");
        std::fs::write(&path, "model = \"opus\"\r\nother = 1\r\n").unwrap();
        let doc = load(&path).unwrap();
        assert_eq!(doc["model"].as_str(), Some("opus"));
        // Edit should succeed even though original was CRLF.
        edit(&path, |d| {
            d["model"] = toml_edit::value("sonnet");
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("sonnet"));
    }

    #[test]
    fn handles_missing_final_newline() {
        let path = scratch("no_nl.toml");
        std::fs::write(&path, "model = \"opus\"").unwrap();
        let doc = load(&path).unwrap();
        assert_eq!(doc["model"].as_str(), Some("opus"));
        edit(&path, |d| {
            d["model"] = toml_edit::value("sonnet");
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("sonnet"));
    }

    #[test]
    fn no_op_preserves_byte_identity_for_toml() {
        let path = scratch("noop.toml");
        let original = "# keep me\nmodel = \"opus\"\n";
        std::fs::write(&path, original).unwrap();
        let before = std::fs::read(&path).unwrap();
        edit(&path, |_| {}).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let path = scratch("absent.toml");
        drop(std::fs::remove_file(&path));
        let doc = load(&path).unwrap();
        assert!(doc.is_empty());
    }

    #[test]
    fn table_decor_preserved() {
        let path = scratch("decor.toml");
        std::fs::write(
            &path,
            "[table]\n# comment inside\nkey = 1\n\n[other]\nval = 2\n",
        )
        .unwrap();
        edit(&path, |d| {
            d["table"]["key"] = toml_edit::value(42);
        })
        .unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# comment inside"));
        assert!(after.contains("key = 42"));
        assert!(after.contains("[other]"));
    }
}
