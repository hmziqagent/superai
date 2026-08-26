use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{ConfigError, Result};

/// Copy `path` beside itself as `<name>.bak.<unix_millis>` before it is overwritten.
///
/// Returns `Ok(None)` when the file does not exist yet — a first write has nothing
/// to preserve. Every mutating helper in this crate calls this first.
pub fn backup(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());

    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".bak.{stamp}"));
    let target = path.with_file_name(name);

    std::fs::copy(path, &target).map_err(|e| ConfigError::io(path, e))?;
    Ok(Some(target))
}

/// Restore a backup produced by [`backup`] over `path`.
pub fn restore(backup_path: &Path, path: &Path) -> Result<()> {
    std::fs::copy(backup_path, path).map_err(|e| ConfigError::io(path, e))?;
    Ok(())
}
