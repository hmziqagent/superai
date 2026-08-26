use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A named, isolated setup of a harness: its own config dir, key, provider, and skills.
///
/// This record is superai's own data — the harness has never heard of it, so there is
/// nothing here to conflict with what the harness writes. Anything the harness owns
/// (model, base URL, …) is read fresh from its config file instead of mirrored here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    /// Name chosen by the user; also the wrapper command name unless overridden.
    pub name: String,

    /// Which harness this instance is an install of, e.g. `claude-code`.
    pub harness: String,

    /// Config directory the wrapper points the harness at.
    pub config_dir: PathBuf,

    /// Binary the wrapper execs, when it is not the one on `PATH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<PathBuf>,

    /// Template this instance was built from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<TemplateRef>,
}

/// The template an instance came from, and the version it was built at.
///
/// The template name alone cannot answer "is this instance behind?" — that needs
/// the version, tracked separately from the harness's own version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateRef {
    /// Template identifier, e.g. `claude-code-glm`.
    pub name: String,
    /// Template version the instance was built from.
    pub version: String,
}
