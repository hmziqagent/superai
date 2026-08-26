use std::path::Path;

use toml_edit::DocumentMut;

use crate::backup::backup;
use crate::error::{ConfigError, Result};

/// Read a TOML config fresh from disk. A missing file reads as an empty document.
///
/// `toml_edit` keeps comments, key order, and formatting, so writing back only
/// touches what superai actually changed.
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
pub fn store(path: &Path, doc: &DocumentMut) -> Result<()> {
    backup(path)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    std::fs::write(path, doc.to_string()).map_err(|e| ConfigError::io(path, e))
}

/// Read fresh, apply `edit`, write back. The only supported way to mutate a config.
pub fn edit<F>(path: &Path, edit: F) -> Result<()>
where
    F: FnOnce(&mut DocumentMut),
{
    let mut doc = load(path)?;
    edit(&mut doc);
    store(path, &doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_preserves_comments_and_untouched_keys() {
        let path = std::env::temp_dir().join("superai-config-tests-toml/config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
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
}
