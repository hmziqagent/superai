//! opencode local v2 history database reader (USG-03), read-only.

use std::path::{Path, PathBuf};

/// Resolve the v2 history database per docs/harness-configs/opencode.md: an
/// `OPENCODE_DB` value wins, else `$XDG_DATA_HOME/opencode/` with a
/// channel-suffixed file name on non-default channels.
pub fn history_db_path(
    opencode_db_override: Option<&Path>,
    xdg_data_home: &Path,
    channel: Option<&str>,
) -> PathBuf {
    if let Some(path) = opencode_db_override {
        return path.to_path_buf();
    }
    let file_name = match channel {
        Some(channel) => format!("opencode-{channel}.db"),
        None => "opencode.db".to_owned(),
    };
    xdg_data_home.join("opencode").join(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_value_wins_over_every_derived_path() {
        let r#override = Path::new("/elsewhere/custom.db");
        assert_eq!(
            history_db_path(Some(r#override), Path::new("/data"), Some("beta")),
            PathBuf::from("/elsewhere/custom.db")
        );
    }

    #[test]
    fn default_channel_uses_the_plain_file_name() {
        assert_eq!(
            history_db_path(None, Path::new("/data"), None),
            PathBuf::from("/data/opencode/opencode.db")
        );
    }

    #[test]
    fn non_default_channel_suffixed_file_name() {
        assert_eq!(
            history_db_path(None, Path::new("/data"), Some("beta")),
            PathBuf::from("/data/opencode/opencode-beta.db")
        );
    }
}
