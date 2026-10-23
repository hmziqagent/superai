use std::fmt::Write as _;
use std::path::Path;

use toml_edit::DocumentMut;

use crate::error::{ConfigError, Result};
/// Read fresh; `toml_edit` keeps comments, order, whitespace, and layout,
/// so writes touch only what changed (DOC-04).
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

/// Back up, then write through the one mutation boundary; no typed struct
/// is ever serialized over the source document (DOC-04).
pub fn store(path: &Path, doc: &DocumentMut) -> Result<()> {
    let text = doc.to_string();
    crate::transaction::commit_file(
        "toml-store",
        path,
        text.as_bytes(),
        crate::document::DocumentKind::Toml,
    )?;
    Ok(())
}

/// Read fresh, apply `edit`, write back only if changed; no-ops keep the
/// original bytes, changing writes normalize CRLF to LF.
pub fn edit<F>(path: &Path, edit: F) -> Result<()>
where
    F: FnOnce(&mut DocumentMut),
{
    let mut doc = load(path)?;
    let before = doc.to_string();
    edit(&mut doc);
    let after = doc.to_string();
    if before == after {
        return Ok(());
    }
    store(path, &doc)
}

/// DOC-10 disclosure: changing writes normalize CRLF to LF and may not
/// round-trip decor byte-identically; `None` when layout already matches.
pub fn formatting_change_warning(text: &str) -> Option<&'static str> {
    if text.trim().is_empty() {
        return None;
    }
    if text.contains("\r\n") {
        return Some(
            "toml codec normalizes CRLF to LF on changing writes; \
             surrounding formatting will change even where semantics do not",
        );
    }
    match text.parse::<DocumentMut>() {
        Ok(doc) => {
            let mut rendered = RenderedEq {
                expected: text,
                pos: 0,
            };
            let matches = write!(rendered, "{doc}").is_ok() && rendered.pos == text.len();
            if matches {
                None
            } else {
                Some(
                    "toml codec cannot round-trip this document's decor byte-identically; \
                     surrounding formatting will change even where semantics do not",
                )
            }
        }
        Err(_) => None,
    }
}

struct RenderedEq<'a> {
    expected: &'a str,
    pos: usize,
}

impl std::fmt::Write for RenderedEq<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.pos + s.len();
        if self.expected.get(self.pos..end) != Some(s) {
            return Err(std::fmt::Error);
        }
        self.pos = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (crate::test_util::TempDir, std::path::PathBuf) {
        let dir = crate::test_util::temp_dir_unique("config-toml");
        let path = dir.join(name);
        (dir, path)
    }

    #[test]
    fn formatting_change_warning_edges() {
        assert_eq!(formatting_change_warning(""), None);
        assert_eq!(formatting_change_warning("not toml at all"), None);
        assert_eq!(formatting_change_warning("# c\na = 1\n"), None);
        assert_eq!(formatting_change_warning("a  =  1\n"), None);
        assert_eq!(formatting_change_warning("a = 1\n\n\n"), None);
        assert_eq!(formatting_change_warning("\ta = 1\n"), None);
        assert_eq!(formatting_change_warning("a = 1 \n"), None);
        assert!(formatting_change_warning("a = 1\r\n").is_some());
        assert!(formatting_change_warning("a = 1\r\nb = 2\n").is_some());
    }

    #[test]
    fn edit_preserves_comments_and_untouched_keys() {
        let (_dir, path) = scratch("comment.toml");
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
        let (_dir, path) = scratch("aot.toml");
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
        let (_dir, path) = scratch("inline.toml");
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
        let (_dir, path) = scratch("dotted.toml");
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
        let (_dir, path) = scratch("quoted.toml");
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
        let (_dir, path) = scratch("dup.toml");
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
        let (_dir, path) = scratch("invalid.toml");
        std::fs::write(&path, "a = [\n").unwrap();
        load(&path).unwrap_err();
    }

    #[test]
    fn handles_crlf_input() {
        let (_dir, path) = scratch("crlf.toml");
        std::fs::write(&path, "model = \"opus\"\r\nother = 1\r\n").unwrap();
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
    fn handles_missing_final_newline() {
        let (_dir, path) = scratch("no_nl.toml");
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
        let (_dir, path) = scratch("noop.toml");
        let original = "# keep me\nmodel = \"opus\"\n";
        std::fs::write(&path, original).unwrap();
        let before = std::fs::read(&path).unwrap();
        edit(&path, |_| {}).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let (_dir, path) = scratch("absent.toml");
        drop(std::fs::remove_file(&path));
        let doc = load(&path).unwrap();
        assert!(doc.is_empty());
    }

    #[test]
    fn table_decor_preserved() {
        let (_dir, path) = scratch("decor.toml");
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
