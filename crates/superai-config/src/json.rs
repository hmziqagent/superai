use std::path::Path;

use serde_json::{Map, Value};

use crate::backup::backup;
use crate::error::{ConfigError, Result};

/// Read a JSON config fresh from disk. A missing file reads as an empty object.
///
/// Key order is preserved (`serde_json/preserve_order`), so a round-trip through
/// [`store`] leaves keys superai does not model exactly where the harness put them.
pub fn load(path: &Path) -> Result<Map<String, Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(ConfigError::io(path, e)),
    };

    let value: Value = serde_json::from_str(&text).map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;

    match value {
        Value::Object(map) => Ok(map),
        _ => Err(ConfigError::NotAnObject {
            path: path.to_path_buf(),
        }),
    }
}

/// Back up, then write `config` to `path`, creating parent directories as needed.
pub fn store(path: &Path, config: &Map<String, Value>) -> Result<()> {
    backup(path)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    let mut text = serde_json::to_string_pretty(config).map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    text.push('\n');

    std::fs::write(path, text).map_err(|e| ConfigError::io(path, e))
}

/// Read fresh, apply `edit`, write back. The only supported way to mutate a config.
///
/// Nothing is cached between calls: disk is the source of truth, because the harness,
/// an editor, or another machine may have touched the file since the last read.
pub fn edit<F>(path: &Path, edit: F) -> Result<()>
where
    F: FnOnce(&mut Map<String, Value>),
{
    let mut config = load(path)?;
    edit(&mut config);
    store(path, &config)
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
            std::fs::remove_file(b.path()).unwrap();
        }
    }
}
