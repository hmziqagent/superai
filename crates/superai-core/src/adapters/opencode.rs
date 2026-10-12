//! `OpenCode` adapter: layered JSONC `opencode.json`/`.jsonc`, relocated via
//! `XDG_CONFIG_HOME` plus `OPENCODE_CONFIG`/`OPENCODE_CONFIG_CONTENT` overrides.
//! Two live lines share the `opencode` binary name: v1 (`opencode-ai` 1.x)
//! and v2 (`@opencode/cli` 2.x), so the surfaces below model both eras.

use std::path::{Path, PathBuf};

use crate::adapter::{
    ADAPTER_REVISION, Adapter, ConfigScope, ConfigSurface, DetectionResult, DocumentKind,
    PathResolver, Platform, ProductStatus, RestartBehavior, RootShape, SurfaceOwnership,
    SurfaceSchema, UsageReaderKind, UsageSurface, VersionResolution, WrapperPlan,
};
use superai_config::document::ValueType;

use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, Isolation};

/// Catalog id superai registers this harness under.
pub const HARNESS_ID_STR: &str = "opencode";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "OpenCode";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "opencode";

/// Environment variable that relocates the global config root (XDG base).
pub const CONFIG_ENV_VAR: &str = "XDG_CONFIG_HOME";

/// Environment variable for an extra config file layered between global and project.
pub const CUSTOM_CONFIG_ENV_VAR: &str = "OPENCODE_CONFIG";

/// Environment variable for inline JSON overrides (highest precedence).
pub const INLINE_CONFIG_ENV_VAR: &str = "OPENCODE_CONFIG_CONTENT";

/// Environment variable for extra directory scanned like `.opencode/`; in
/// current v1 and v2 releases it also relocates the global config root.
pub const CONFIG_DIR_ENV_VAR: &str = "OPENCODE_CONFIG_DIR";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/opencode.md";

/// Date the research doc was last checked against the harness; the catalog
/// freshness gate caps how far this may advance past its ledger epoch.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned top-level keys inside `opencode.json`; everything else round-trips
/// untouched via `superai-config::jsonc`. Writes use the v1 spellings, which
/// v2 binaries normalize up, so the v2-only keys stay unowned and preserved.
pub const OWNED_SELECTORS: &[&str] = &[
    "model",
    "small_model",
    "provider",
    "mcp",
    "permission",
    "share",
    "autoupdate",
    "disabled_providers",
    "enabled_providers",
    "agent",
];

/// v2-only keys a v1-line binary cannot honor: upstream `v2-compat.ts` throws
/// on `permissions` and drops the rest with diagnostics.
pub const V2_ONLY_SELECTORS: &[&str] = &[
    "permissions",
    "providers",
    "plugins",
    "websearch",
    "warming",
];

/// Concrete adapter for `OpenCode`: `relocated-root` via `XDG_CONFIG_HOME`
/// plus `OPENCODE_CONFIG` (file) and `OPENCODE_CONFIG_CONTENT` (inline).
#[derive(Debug, Clone)]
pub struct OpenCodeAdapter {
    id: HarnessId,
}

impl OpenCodeAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(CONFIG_ENV_VAR)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir).join("opencode"));
        }
        let home = super::home_dir()?;
        Some(home.join(".config").join("opencode"))
    }

    fn config_path_for_root(root: &Path) -> PathBuf {
        root.join("opencode.json")
    }

    fn tui_path_for_root(root: &Path) -> PathBuf {
        root.join("tui.json")
    }

    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let cfg = Self::config_path_for_root(&root);
                    let cfg_jc = root.join("opencode.jsonc");
                    if cfg.exists() {
                        evidence.push(format!("opencode.json found at {}", cfg.display()));
                        match std::fs::read_to_string(&cfg) {
                            Ok(text)
                                if (text.contains("\"$schema\"") || text.contains("$schema")) =>
                            {
                                evidence.push("opencode.json contains $schema marker".to_owned());
                            }
                            Ok(_) => {}
                            Err(err) => evidence
                                .push(format!("config unreadable at {}: {err}", cfg.display())),
                        }
                    } else if cfg_jc.exists() {
                        evidence.push(format!("opencode.jsonc found at {}", cfg_jc.display()));
                    } else {
                        evidence.push(format!("opencode.json missing at {}", cfg.display()));
                    }
                    let tui = Self::tui_path_for_root(&root);
                    let tui_jc = root.join("tui.jsonc");
                    if tui.exists() || tui_jc.exists() {
                        evidence.push(format!("tui config present at {}", root.display()));
                    }
                    let cli_jc = root.join("cli.json").exists() || root.join("cli.jsonc").exists();
                    if cli_jc {
                        evidence.push(format!("cli.json (v2 line) present at {}", root.display()));
                    }
                    let auth = dirs_auth_path();
                    if let Some(auth_path) = auth
                        && auth_path.exists()
                    {
                        evidence.push(format!("auth.json present at {}", auth_path.display()));
                    }
                    if let Some(db) = dirs_db_path()
                        && db.exists()
                    {
                        evidence.push(format!("opencode.db (v2 line) present at {}", db.display()));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
        if let Ok(custom) = std::env::var(CUSTOM_CONFIG_ENV_VAR)
            && !custom.trim().is_empty()
        {
            let p = Path::new(&custom);
            if p.exists() {
                evidence.push(format!(
                    "custom config {CUSTOM_CONFIG_ENV_VAR} exists at {}",
                    p.display()
                ));
            } else {
                evidence.push(format!(
                    "custom config {CUSTOM_CONFIG_ENV_VAR} set to {} (missing)",
                    p.display()
                ));
            }
        } else {
            evidence.push(format!("{CUSTOM_CONFIG_ENV_VAR} not set"));
        }
        if let Ok(inline) = std::env::var(INLINE_CONFIG_ENV_VAR)
            && !inline.trim().is_empty()
        {
            evidence.push(format!(
                "inline config {INLINE_CONFIG_ENV_VAR} is set (len {})",
                inline.len()
            ));
        }
        let proj = Path::new("opencode.json");
        let proj_jc = Path::new("opencode.jsonc");
        if proj.exists() {
            evidence.push(format!("project opencode.json found at {}", proj.display()));
        } else if proj_jc.exists() {
            evidence.push(format!(
                "project opencode.jsonc found at {}",
                proj_jc.display()
            ));
        }
        if let Ok(extra) = std::env::var(CONFIG_DIR_ENV_VAR)
            && !extra.trim().is_empty()
        {
            evidence.push(format!("extra config dir {CONFIG_DIR_ENV_VAR}={extra}"));
        }
    }
}

fn dirs_auth_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.trim().is_empty()
    {
        return Some(PathBuf::from(xdg).join("opencode").join("auth.json"));
    }
    let home = super::home_dir()?;
    Some(
        home.join(".local")
            .join("share")
            .join("opencode")
            .join("auth.json"),
    )
}

fn dirs_db_path() -> Option<PathBuf> {
    dirs_auth_path()?
        .parent()
        .map(|dir| dir.join("opencode.db"))
}

/// v1 line: 0.x and 1.x binaries. 2.x is the v2 line; anything unparseable
/// is treated as unknown and never conflicts.
fn is_v1_line(version: &str) -> bool {
    let major = version
        .split('.')
        .next()
        .and_then(|s| s.parse::<u64>().ok());
    matches!(major, Some(0 | 1))
}

fn present_v2_keys(content: &[u8]) -> Vec<&'static str> {
    let Ok(text) = std::str::from_utf8(content) else {
        return Vec::new();
    };
    let Ok(value) =
        serde_json::from_str::<serde_json::Value>(&superai_config::jsonc::strip_jsonc(text))
    else {
        return Vec::new();
    };
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };
    V2_ONLY_SELECTORS
        .iter()
        .filter(|key| obj.contains_key(**key))
        .copied()
        .collect()
}

/// Refuse v2-only keys under a detected v1-line binary: v1 hard-errors on
/// `permissions` and silently drops the others, while v2 normalizes v1 keys,
/// so only this direction breaks the harness.
pub fn v2_key_era_conflict(version: &str, content: &[u8]) -> Option<String> {
    if !is_v1_line(version) {
        return None;
    }
    let present = present_v2_keys(content);
    if present.is_empty() {
        return None;
    }
    Some(format!(
        "detected opencode {version} is the v1 line but the config carries v2-only keys ({}); v1 refuses `permissions` and ignores the rest while v2 accepts v1 keys, so migrate the file before writing",
        present.join(", ")
    ))
}

impl Default for OpenCodeAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for OpenCodeAdapter {
    fn id(&self) -> HarnessId {
        self.id.clone()
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    fn product_status(&self) -> ProductStatus {
        ProductStatus::Active
    }

    fn supported_platforms(&self) -> Vec<Platform> {
        super::desktop_platforms()
    }

    fn adapter_revision(&self) -> &str {
        ADAPTER_REVISION
    }

    fn research_doc_link(&self) -> &str {
        RESEARCH_DOC
    }

    fn last_verified_date(&self) -> &str {
        LAST_VERIFIED
    }

    fn detection(&self) -> DetectionResult {
        let mut evidence = Vec::new();
        let mut version: Option<String> = None;
        let mut binary_path: Option<PathBuf> = None;

        match super::find_in_path(&[EXECUTABLE]) {
            Some(path) => {
                evidence.push(format!(
                    "found binary `{}` at {}",
                    EXECUTABLE,
                    path.display()
                ));
                match super::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `{EXECUTABLE} --version`"));
                        version = Some(v);
                    }
                    None => {
                        evidence.push(format!(
                            "version probe failed for `{EXECUTABLE} --version` (timeout or non-zero)"
                        ));
                    }
                }
                binary_path = Some(path);
            }
            None => {
                evidence.push(format!("binary `{EXECUTABLE}` not found in PATH"));
            }
        }

        self.collect_config_evidence(&mut evidence);

        let present = super::install_presence(binary_path.is_some(), version.is_some());

        let confidence =
            super::detection_confidence(binary_path.is_some(), version.is_some(), false);
        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution_from(&self, detection: &DetectionResult) -> VersionResolution {
        super::resolution_from_detection(detection, "opencode", SCHEMA_VERSION_STR)
    }

    #[expect(clippy::too_many_lines, reason = "surfaces are declarative")]
    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let opencode_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/opencode/opencode.json"),
            Some("$XDG_CONFIG_HOME/opencode/opencode.json"),
            Some("%XDG_CONFIG_HOME%\\opencode\\opencode.json"),
            "~/.config/opencode/opencode.json",
        );
        let mut opencode_surface = ConfigSurface::new(
            "opencode.json",
            opencode_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        opencode_surface.precedence = 10;
        opencode_surface.owned_selectors =
            OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        opencode_surface.backup_required = true;
        opencode_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(opencode_surface);

        let opencode_jc_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/opencode/opencode.jsonc"),
            Some("$XDG_CONFIG_HOME/opencode/opencode.jsonc"),
            Some("%XDG_CONFIG_HOME%\\opencode\\opencode.jsonc"),
            "~/.config/opencode/opencode.jsonc",
        );
        let mut opencode_jc_surface = ConfigSurface::new(
            "opencode.jsonc",
            opencode_jc_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        opencode_jc_surface.precedence = 10;
        opencode_jc_surface.owned_selectors =
            OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        opencode_jc_surface.backup_required = true;
        opencode_jc_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(opencode_jc_surface);

        let tui_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/opencode/tui.json"),
            Some("$XDG_CONFIG_HOME/opencode/tui.json"),
            Some("%XDG_CONFIG_HOME%\\opencode\\tui.json"),
            "~/.config/opencode/tui.json",
        );
        let mut tui_surface = ConfigSurface::new(
            "tui.json",
            tui_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        tui_surface.precedence = 5;
        tui_surface.backup_required = true;
        surfaces.push(tui_surface);

        let cli_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/opencode/cli.json"),
            Some("$XDG_CONFIG_HOME/opencode/cli.json"),
            Some("%XDG_CONFIG_HOME%\\opencode\\cli.json"),
            "~/.config/opencode/cli.json",
        );
        let mut cli_surface = ConfigSurface::new(
            "cli.json",
            cli_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        cli_surface.precedence = 5;
        cli_surface.backup_required = true;
        cli_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(cli_surface);

        let db_resolver = PathResolver::new(
            Some("$XDG_DATA_HOME/opencode/opencode.db"),
            Some("$XDG_DATA_HOME/opencode/opencode.db"),
            Some("%APPDATA%\\opencode\\opencode.db"),
            "~/.local/share/opencode/opencode.db",
        );
        let mut db = ConfigSurface::new(
            "opencode.db",
            db_resolver,
            DocumentKind::Sqlite,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        db.precedence = 0;
        db.backup_required = false;
        db.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(db);

        let auth_resolver = PathResolver::new(
            Some("$XDG_DATA_HOME/opencode/auth.json"),
            Some("$XDG_DATA_HOME/opencode/auth.json"),
            Some("%APPDATA%\\opencode\\auth.json"),
            "~/.local/share/opencode/auth.json",
        );
        let mut auth = ConfigSurface::new(
            "auth.json",
            auth_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        auth.precedence = 0;
        auth.backup_required = false;
        auth.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(auth);

        let project_resolver = PathResolver::fallback_only(
            "opencode.json in cwd or nearest git root (also opencode.jsonc)",
        );
        let mut project_surface = ConfigSurface::new(
            "project opencode.json",
            project_resolver,
            DocumentKind::Jsonc,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project_surface.precedence = 20;
        project_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        project_surface.backup_required = true;
        surfaces.push(project_surface);

        let custom_resolver = PathResolver::new(
            Some("$OPENCODE_CONFIG"),
            Some("$OPENCODE_CONFIG"),
            Some("%OPENCODE_CONFIG%"),
            "$OPENCODE_CONFIG (custom path, layered between global and project)",
        );
        let mut custom_surface = ConfigSurface::new(
            "OPENCODE_CONFIG",
            custom_resolver,
            DocumentKind::Jsonc,
            ConfigScope::SessionInline,
            SurfaceOwnership::UserEditable,
        );
        custom_surface.precedence = 15;
        custom_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        custom_surface.backup_required = true;
        surfaces.push(custom_surface);

        let inline_resolver = PathResolver::fallback_only("$OPENCODE_CONFIG_CONTENT (inline JSON)");
        let mut inline_surface = ConfigSurface::new(
            "OPENCODE_CONFIG_CONTENT",
            inline_resolver,
            DocumentKind::Jsonc,
            ConfigScope::SessionInline,
            SurfaceOwnership::UserEditable,
        );
        inline_surface.precedence = 40;
        inline_surface.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        inline_surface.backup_required = false;
        surfaces.push(inline_surface);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        super::all_operations_full()
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "cache/*".to_owned(),
            "logs/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            ".tmp/*".to_owned(),
            "*.lock".to_owned(),
            "node_modules/*".to_owned(),
            ".git/*".to_owned(),
            "tmp/*".to_owned(),
            "auth.json".to_owned(),
            "*.db".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let mut plan = WrapperPlan::new("relocated-root via XDG_CONFIG_HOME + OPENCODE_CONFIG");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        let cfg_path = instance.config_root.as_path().join("opencode.json");
        plan.env_vars.push((
            CUSTOM_CONFIG_ENV_VAR.to_owned(),
            cfg_path.display().to_string(),
        ));
        // Current v1 and v2 releases let this var override the config root,
        // so an inherited value would bypass the XDG relocation entirely.
        plan.env_unset.push(CONFIG_DIR_ENV_VAR.to_owned());
        plan.description = format!(
            " Wrapper sets {}={} and {}={} and execs `{}`",
            CONFIG_ENV_VAR,
            instance.config_root,
            CUSTOM_CONFIG_ENV_VAR,
            cfg_path.display(),
            EXECUTABLE
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/opencode".to_owned(),
            "~/.config/opencode/opencode.json".to_owned(),
            "~/.config/opencode/opencode.jsonc".to_owned(),
            "~/.config/opencode/cli.json".to_owned(),
            "$XDG_CONFIG_HOME/opencode".to_owned(),
            "$OPENCODE_CONFIG".to_owned(),
            "$OPENCODE_CONFIG_CONTENT".to_owned(),
            "./opencode.json".to_owned(),
            "./opencode.jsonc".to_owned(),
            "./.opencode/opencode.json".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::RelocatedRoot
            | Isolation::ExplicitConfig
            | Isolation::EnvOnly
            | Isolation::Unknown => {
                crate::adapter::validate_instance_surfaces(self, instance.config_root.as_path())
            }
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("opencode requires isolation relocated_root, got {other}"),
            }),
        }
    }

    fn surface_schema(&self, surface_id: &str) -> Option<SurfaceSchema> {
        // `model` is deliberately untyped: the docs show a string but
        // model-object forms exist too (opencode.md §1).
        match surface_id {
            "opencode.json" | "opencode.jsonc" | "project opencode.json" => Some(
                SurfaceSchema::new()
                    .with_root_shape(RootShape::Object)
                    .with_owned_key("provider", ValueType::Object)
                    .with_owned_key("providers", ValueType::Object)
                    .with_owned_key("mcp", ValueType::Object)
                    .with_owned_key("agent", ValueType::Object)
                    .with_owned_key("agents", ValueType::Object)
                    .with_owned_key("command", ValueType::Object)
                    .with_owned_key("commands", ValueType::Object)
                    .with_owned_key("permissions", ValueType::Array)
                    .with_owned_key("update", ValueType::String)
                    .with_owned_key("small_model", ValueType::String),
            ),
            "tui.json" | "cli.json" => {
                Some(SurfaceSchema::new().with_root_shape(RootShape::Object))
            }
            _ => None,
        }
    }

    fn era_conflict_reason(&self, surface_id: &str, content: &[u8]) -> Option<String> {
        if !matches!(
            surface_id,
            "opencode.json" | "opencode.jsonc" | "project opencode.json"
        ) {
            return None;
        }
        let version = self.version_resolution().detected_version?;
        v2_key_era_conflict(&version, content)
    }

    fn capability_declarations(&self) -> Vec<crate::adapter::AdapterCapabilityDecl> {
        vec![
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::WebSearch,
                crate::capability::Support::Native,
                "web search tool against the configured provider",
            ),
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::Vision,
                crate::capability::Support::Native,
                "image input on the chat transports",
            ),
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::ComputerUse,
                crate::capability::Support::Absent,
                "no computer-use loop in the opencode harness",
            ),
            crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::Mcp,
                crate::capability::Support::Native,
                "mcp table in opencode.json",
            ),
        ]
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        super::skill_modes_link_first()
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(crate::adapter::McpAdapterDecl::new(
            "opencode.json",
            "mcp",
            DocumentKind::Jsonc,
            ConfigScope::User,
            RestartBehavior::Reload,
        ).with_read_only("jsonc writes refuse (LossyWrite) until a preserving codec exists; inspect/diff only; v2 nests servers under mcp.servers while v1 keys them under mcp directly"))
    }

    /// npm `plugin` list entries plus files in `.opencode/plugins/` or `~/.config/opencode/plugins/`.
    fn plugin_decl(&self) -> Option<crate::adapter::PluginAdapterDecl> {
        Some(crate::adapter::PluginAdapterDecl::directory_bundle(
            "plugins",
            None,
            RestartBehavior::Reload,
        ))
    }
    fn usage_surface(&self) -> Option<UsageSurface> {
        Some(
            UsageSurface::new(UsageReaderKind::HistorySqlite, Some("opencode.ai"))
                .with_artifact(
                    "history-db",
                    PathResolver::new(
                        Some("$OPENCODE_DB, else $XDG_DATA_HOME/opencode/opencode.db"),
                        Some("$OPENCODE_DB, else $XDG_DATA_HOME/opencode/opencode.db"),
                        None,
                        "~/.local/share/opencode/opencode.db (channel-suffixed on non-default channels)",
                    ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{
        CONFIG_DIR_ENV_VAR, CONFIG_ENV_VAR, CUSTOM_CONFIG_ENV_VAR, HARNESS_ID_STR, OWNED_SELECTORS,
        V2_ONLY_SELECTORS,
    };
    use crate::adapter::{
        Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership, WrapperPlan,
    };
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> super::OpenCodeAdapter {
        super::OpenCodeAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-opencode-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
        }
    }

    #[test]
    fn adapter_identity() {
        let a = adapter();
        let entry = crate::harness_catalog::find_by_id(HARNESS_ID_STR).unwrap();
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(a.product_status(), entry.product_status);
    }

    #[test]
    fn supported_platforms_covers_all() {
        let a = adapter();
        let platforms = a.supported_platforms();
        assert!(platforms.len() >= 3);
        let os_set: HashSet<String> = platforms.iter().map(|p| p.os.to_string()).collect();
        assert!(os_set.contains("linux"));
        assert!(os_set.contains("macos"));
        assert!(os_set.contains("windows"));
    }

    #[test]
    fn detection_returns_evidence_and_confidence() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        match result.present {
            InstallPresence::Absent => {
                assert!(result.version.is_none());
            }
            InstallPresence::Present => {
                assert!(result.version.is_some());
            }
            InstallPresence::UnknownVersion => {
                assert!(result.evidence.iter().any(|e| e.contains("found binary")));
            }
            InstallPresence::Broken => {
                assert!(!result.evidence.is_empty());
            }
        }
    }

    #[test]
    fn version_resolution_maps_detected() {
        let a = adapter();
        let res = a.version_resolution();
        if res.detected_version.is_some() {
            assert_eq!(
                res.schema_version.as_deref(),
                Some(super::SCHEMA_VERSION_STR)
            );
            assert!(res.compatible);
        } else {
            assert!(!res.compatible);
            assert!(res.schema_version.is_none());
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn v2_key_era_conflict_refuses_only_v1_line_with_v2_keys() {
        let v2_content = br#"{"providers": {"acme": {}}, "permissions": []}"#;
        let conflict = super::v2_key_era_conflict("1.19.0", v2_content).unwrap();
        assert!(conflict.contains("permissions"), "{conflict}");
        assert!(conflict.contains("providers"), "{conflict}");

        assert!(super::v2_key_era_conflict("2.0.27", v2_content).is_none());
        assert!(super::v2_key_era_conflict("1.19.0", br#"{"provider": {}}"#).is_none());
        assert!(super::v2_key_era_conflict("0.12.0", br#"{"plugins": ["x"]}"#).is_some());
        assert!(super::v2_key_era_conflict("garbage", v2_content).is_none());
        assert!(super::v2_key_era_conflict("1.19.0", b"[1]").is_none());
        assert!(super::v2_key_era_conflict("1.19.0", b"not json").is_none());
    }

    #[test]
    fn v2_key_era_conflict_reads_jsonc_comments_and_trailing_commas() {
        let jsonc = b"{ // providers block\n \"providers\": {}, }";
        assert!(super::v2_key_era_conflict("1.19.0", jsonc).is_some());
    }

    #[test]
    fn v2_only_selectors_disjoint_from_v1_only_era_keys() {
        let v1_only = [
            "provider",
            "agent",
            "command",
            "plugin",
            "permission",
            "autoupdate",
        ];
        for key in V2_ONLY_SELECTORS {
            assert!(!v1_only.contains(key));
        }
    }

    #[derive(Debug)]
    struct V1EraOpenCode;

    impl Adapter for V1EraOpenCode {
        fn id(&self) -> HarnessId {
            HarnessId::from_validated_const(HARNESS_ID_STR)
        }
        fn display_name(&self) -> &str {
            super::DISPLAY_NAME
        }
        fn product_status(&self) -> ProductStatus {
            ProductStatus::Active
        }
        fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
            Vec::new()
        }
        fn adapter_revision(&self) -> &str {
            crate::adapter::ADAPTER_REVISION
        }
        fn research_doc_link(&self) -> &str {
            super::RESEARCH_DOC
        }
        fn last_verified_date(&self) -> &str {
            super::LAST_VERIFIED
        }
        fn detection(&self) -> crate::adapter::DetectionResult {
            crate::adapter::DetectionResult::absent(vec!["test".to_owned()])
        }
        fn version_resolution(&self) -> crate::adapter::VersionResolution {
            crate::adapter::VersionResolution::new(
                Some("1.19.0".to_owned()),
                Some(super::SCHEMA_VERSION_STR.to_owned()),
                true,
            )
        }
        fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
            vec![crate::adapter::ConfigSurface::new(
                "opencode.json",
                crate::adapter::PathResolver::fallback_only("~/.config/opencode/opencode.json"),
                DocumentKind::Jsonc,
                ConfigScope::User,
                SurfaceOwnership::UserEditable,
            )]
        }
        fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
            Vec::new()
        }
        fn plan_mirror_exclusions(&self) -> Vec<String> {
            Vec::new()
        }
        fn plan_wrapper(&self, _instance: &Instance) -> Result<WrapperPlan, CoreError> {
            Ok(WrapperPlan::new("test"))
        }
        fn scan_candidates(&self) -> Vec<String> {
            Vec::new()
        }
        fn validate_instance(&self, _instance: &Instance) -> Result<(), CoreError> {
            Ok(())
        }
        fn era_conflict_reason(&self, surface_id: &str, content: &[u8]) -> Option<String> {
            (surface_id == "opencode.json")
                .then(|| super::v2_key_era_conflict("1.19.0", content))
                .flatten()
        }
    }

    #[test]
    fn raw_editor_refuses_v2_keys_under_v1_line_and_keeps_file() {
        let dir = crate::test_util::temp_dir_unique("opencode-era");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.json");
        let original: &[u8] = br#"{"model": "anthropic/claude-sonnet-4-5"}"#;
        std::fs::write(&path, original).unwrap();
        let adapter = V1EraOpenCode;
        let v2_bytes = std::fs::read(fixture_path("opencode.v2.json")).unwrap();
        let err =
            crate::raw_editor::commit_for_adapter(&path, &v2_bytes, None, &adapter).unwrap_err();
        match err {
            CoreError::UnsupportedVersion { reason, .. } => {
                assert!(reason.contains("v1 line"), "{reason}");
                assert!(reason.contains("permissions"), "{reason}");
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let v1_bytes = std::fs::read(fixture_path("opencode.populated.json")).unwrap();
        crate::raw_editor::commit_for_adapter(&path, &v1_bytes, None, &adapter).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), v1_bytes);
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn config_surfaces_include_writable_jsonc() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 5);
        let cfg = surfaces
            .iter()
            .find(|s| s.id == "opencode.json")
            .expect("opencode.json surface must exist");
        assert_eq!(cfg.kind, DocumentKind::Jsonc);
        assert_eq!(cfg.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(cfg.scope, ConfigScope::User);
        assert!(cfg.backup_required);
        for selector in ["model", "provider", "mcp"] {
            assert!(
                cfg.owned_selectors.contains(&selector.to_owned()),
                "owned_selectors must contain {selector}"
            );
        }
        for sel in &cfg.owned_selectors {
            assert!(!sel.is_empty());
        }
        for sel in OWNED_SELECTORS {
            assert!(cfg.owned_selectors.contains(&(*sel).to_owned()));
        }

        let cfg_jc = surfaces
            .iter()
            .find(|s| s.id == "opencode.jsonc")
            .expect("opencode.jsonc surface");
        assert_eq!(cfg_jc.kind, DocumentKind::Jsonc);

        let auth = surfaces
            .iter()
            .find(|s| s.id == "auth.json")
            .expect("auth surface");
        assert_eq!(auth.ownership, SurfaceOwnership::ExternalSecretStore);
        assert!(!auth.backup_required);

        let cli = surfaces
            .iter()
            .find(|s| s.id == "cli.json")
            .expect("cli.json surface");
        assert_eq!(cli.kind, DocumentKind::Jsonc);
        assert_eq!(cli.scope, ConfigScope::User);
        assert_eq!(cli.ownership, SurfaceOwnership::UserEditable);
        assert!(cli.backup_required);

        let db = surfaces
            .iter()
            .find(|s| s.id == "opencode.db")
            .expect("opencode.db surface");
        assert_eq!(db.kind, DocumentKind::Sqlite);
        assert_eq!(db.ownership, SurfaceOwnership::ExternalSecretStore);
        assert!(!db.backup_required);

        let project = surfaces
            .iter()
            .find(|s| s.id == "project opencode.json")
            .expect("project surface");
        assert_eq!(project.scope, ConfigScope::ProjectWorkspace);
        assert_eq!(project.kind, DocumentKind::Jsonc);

        let inline = surfaces
            .iter()
            .find(|s| s.id == "OPENCODE_CONFIG_CONTENT")
            .expect("inline surface");
        assert_eq!(inline.scope, ConfigScope::SessionInline);
    }

    #[test]
    fn supported_operations_cover_full() {
        let a = adapter();
        let ops = a.supported_operations();
        assert!(!ops.is_empty());
        for (_, support) in &ops {
            assert_eq!(*support, AdapterSupport::Full);
        }
        let names: HashSet<String> = ops.iter().map(|(n, _)| n.clone()).collect();
        for required in [
            "detect",
            "read_config",
            "write_config",
            "manage_mcp",
            "plan_wrapper",
        ] {
            assert!(names.contains(required), "missing op {required}");
        }
    }

    #[test]
    fn plan_mirror_exclusions_cover_cache_and_locks() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        assert!(!exclusions.is_empty());
        let must_contain = ["cache/*", "logs/*", "*.log", "node_modules/*"];
        for pat in must_contain {
            assert!(
                exclusions.contains(&pat.to_owned()),
                "exclusions must contain {pat}"
            );
        }
        assert!(!exclusions.contains(&"opencode.json".to_owned()));
    }

    #[test]
    fn plan_mirror_includes_config_and_excludes_cache() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        let is_excluded = |file: &str| crate::adapters::exclusion_matches(&exclusions, file);
        assert!(!is_excluded("opencode.json"));
        assert!(!is_excluded("opencode.jsonc"));
        assert!(!is_excluded("tui.json"));
        assert!(!is_excluded("cli.json"));
        assert!(is_excluded("cache/data.bin"));
        assert!(is_excluded("logs/opencode.log"));
        assert!(is_excluded("node_modules/foo/index.js"));
        assert!(is_excluded("opencode.db"));
        assert!(is_excluded("opencode-beta.db"));
    }

    #[test]
    fn plan_wrapper_sets_xdg_and_custom_config() {
        let tmp_root = crate::test_util::tmp_abs_str(".opencode-work");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CONFIG_ENV_VAR && v == tmp_root.as_str())
        );
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == CUSTOM_CONFIG_ENV_VAR && v.contains(".opencode-work"))
        );
        assert!(!plan.description.is_empty());
        assert!(plan.description.contains(CONFIG_ENV_VAR));
        assert!(plan.description.contains(CUSTOM_CONFIG_ENV_VAR));
        assert!(
            plan.env_unset.contains(&CONFIG_DIR_ENV_VAR.to_owned()),
            "wrapper must clear {CONFIG_DIR_ENV_VAR} so it cannot bypass the relocation"
        );
    }

    #[test]
    fn plan_wrapper_quoting_with_spaces() {
        let tmp_root = crate::test_util::tmp_abs_str("my opencode work");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        let env_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == CONFIG_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(env_val, tmp_root.as_str());
        assert!(!env_val.contains('"'));
        assert!(env_val.contains(' '));
        let custom_val = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == CUSTOM_CONFIG_ENV_VAR)
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert!(custom_val.contains(' '));
        assert!(!custom_val.contains('"'));
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".opencode-work"));
        inst.harness = HarnessId::new("codex-cli").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_default_root() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| c.contains("opencode")));
        assert!(candidates.iter().any(|c| c.contains(CONFIG_ENV_VAR)));
        assert!(candidates.iter().any(|c| c.contains(CUSTOM_CONFIG_ENV_VAR)));
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("./.opencode/opencode.json"))
        );
        assert!(candidates.iter().any(|c| c.contains("cli.json")));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".opencode-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".opencode-work"));
        inst.isolation = Isolation::FixedPathSingle;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn validate_instance_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".opencode-work"));
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    #[test]
    fn path_resolution_resolver_fallbacks() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let cfg = surfaces.iter().find(|s| s.id == "opencode.json").unwrap();
        assert_eq!(
            cfg.path_resolver.fallback,
            "~/.config/opencode/opencode.json"
        );
        let resolver = &cfg.path_resolver;
        assert!(resolver.linux.as_deref().unwrap().contains(CONFIG_ENV_VAR));
    }

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/opencode")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.json");
        let map = superai_config::jsonc::load(&path).unwrap();
        assert!(map.is_empty());
        let value = superai_config::jsonc::load_value(&path).unwrap();
        assert_eq!(value, serde_json::Value::Object(serde_json::Map::default()));
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("opencode.minimal.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::jsonc::load(&path).unwrap();
        assert!(map.is_empty() || map.contains_key("$schema"));
    }

    #[test]
    fn fixture_populated_parses_and_has_expected_keys() {
        let path = fixture_path("opencode.populated.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::jsonc::load(&path).unwrap();
        assert!(
            map.contains_key("model") || map.contains_key("provider") || map.contains_key("mcp")
        );
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("opencode.foreign.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::jsonc::load(&path).unwrap();
        assert!(original.contains_key("foreignKey") || original.contains_key("unknownTopLevel"));
        let dir = crate::test_util::temp_dir_unique("opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("opencode.foreign.copy.json");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::jsonc::edit(&tmp, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("anthropic/claude-sonnet-4-5".to_owned()),
            );
            assert!(map.contains_key("foreignKey") || map.contains_key("unknownTopLevel"));
        })
        .unwrap();
        let after = superai_config::jsonc::load(&tmp).unwrap();
        assert_eq!(
            after["model"],
            serde_json::Value::String("anthropic/claude-sonnet-4-5".to_owned())
        );
        let foreign_preserved = after.contains_key("foreignKey")
            || after.contains_key("unknownTopLevel")
            || after.contains_key("customField");
        assert!(
            foreign_preserved,
            "foreign keys must be preserved, got {after:?}"
        );
        drop(std::fs::remove_file(&tmp));
    }

    #[test]
    fn fixture_v2_parses_and_keeps_era_keys_side_by_side() {
        let path = fixture_path("opencode.v2.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::jsonc::load(&path).unwrap();
        assert!(map.contains_key("providers"));
        assert!(map.contains_key("permissions"));
        assert!(map["mcp"]["servers"]["local-server"]["command"].is_array());
        assert!(map["permissions"].is_array());

        let jc = fixture_path("opencode.v2.jsonc");
        let jc_map = superai_config::jsonc::load(&jc).unwrap();
        assert!(jc_map.contains_key("providers"));
        assert!(jc_map.contains_key("update"));

        let cli = fixture_path("cli.json");
        let cli_map = superai_config::jsonc::load(&cli).unwrap();
        assert!(cli_map.contains_key("tabs") && cli_map.contains_key("theme"));
    }

    #[test]
    fn fixture_v2_conflicts_under_v1_line_not_v2() {
        let v2_bytes = std::fs::read(fixture_path("opencode.v2.json")).unwrap();
        assert!(super::v2_key_era_conflict("1.19.0", &v2_bytes).is_some());
        assert!(super::v2_key_era_conflict("2.0.27", &v2_bytes).is_none());
    }

    #[test]
    fn fixture_malformed_fails_to_parse() {
        let path = fixture_path("opencode.malformed.json");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::jsonc::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn unknown_key_preservation_via_preserve_order() {
        let dir = crate::test_util::temp_dir_unique("opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("preserve.json");
        let original_json = serde_json::json!({
            "model": "anthropic/claude-sonnet-4-5",
            "foreignKey": "keep-me",
            "provider": {
                "anthropic": {
                    "options": {"baseURL": "https://example.com"}
                }
            },
            "anotherForeign": {"nested": 123}
        });
        let text = serde_json::to_string_pretty(&original_json).unwrap();
        std::fs::write(&path, text).unwrap();

        superai_config::jsonc::edit(&path, |map| {
            if let Some(provider) = map.get_mut("provider").and_then(|v| v.as_object_mut())
                && let Some(anth) = provider
                    .get_mut("anthropic")
                    .and_then(|v| v.as_object_mut())
                && let Some(opts) = anth.get_mut("options").and_then(|v| v.as_object_mut())
            {
                opts.insert(
                    "baseURL".to_owned(),
                    serde_json::Value::String("https://new.example.com".to_owned()),
                );
            }
        })
        .unwrap();
        let after = superai_config::jsonc::load(&path).unwrap();
        assert_eq!(
            after["foreignKey"],
            serde_json::Value::String("keep-me".to_owned())
        );
        assert_eq!(
            after["anotherForeign"]["nested"],
            serde_json::Value::Number(123.into())
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn provider_mutation_sets_model_and_provider() {
        let dir = crate::test_util::temp_dir_unique("opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("provider.json");
        let initial = serde_json::json!({
            "model": "anthropic/claude-sonnet-4-5",
            "provider": {
                "anthropic": {
                    "options": {"apiKey": "{env:ANTHROPIC_API_KEY}"}
                }
            }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        superai_config::jsonc::edit(&path, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("openrouter/anthropic/claude-sonnet-4".to_owned()),
            );
            let provider = map
                .entry("provider".to_owned())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::default()));
            if let Some(obj) = provider.as_object_mut() {
                obj.insert(
                    "openrouter".to_owned(),
                    serde_json::json!({
                        "npm": "@ai-sdk/openai-compatible",
                        "options": {"baseURL": "https://openrouter.ai/api/v1"}
                    }),
                );
            }
        })
        .unwrap();
        let after = superai_config::jsonc::load(&path).unwrap();
        assert_eq!(
            after["model"],
            serde_json::Value::String("openrouter/anthropic/claude-sonnet-4".to_owned())
        );
        assert!(
            after["provider"]
                .as_object()
                .unwrap()
                .contains_key("openrouter")
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn wrapper_env_var_isolation_is_relocated_root() {
        let tmp_root = crate::test_util::tmp_abs_str("user/.opencode-isolated");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(!plan.env_vars.is_empty());
        let (key, val) = &plan.env_vars[0];
        assert_eq!(key, CONFIG_ENV_VAR);
        assert_eq!(val, tmp_root.as_str());
    }

    #[test]
    fn jsonc_stripping_allows_comments_and_trailing_commas() {
        let dir = crate::test_util::temp_dir_unique("opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.jsonc");
        let content = r#"
        {
            // Model selection
            "model": "anthropic/claude-sonnet-4-5", // trailing comma,
            /* provider block */
            "provider": {
                "anthropic": {
                    "options": {
                        "baseURL": "https://example.com", // trailing comma,
                    },
                },
            },
        }
        "#;
        std::fs::write(&path, content).unwrap();
        let map = superai_config::jsonc::load(&path).unwrap();
        assert_eq!(
            map["model"],
            serde_json::Value::String("anthropic/claude-sonnet-4-5".to_owned())
        );
        assert_eq!(
            map["provider"]["anthropic"]["options"]["baseURL"],
            serde_json::Value::String("https://example.com".to_owned())
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn inline_config_surface_is_session_inline() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let inline = surfaces
            .iter()
            .find(|s| s.id == "OPENCODE_CONFIG_CONTENT")
            .unwrap();
        assert_eq!(inline.scope, ConfigScope::SessionInline);
        assert_eq!(inline.kind, DocumentKind::Jsonc);
    }

    #[test]
    fn adapter_is_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
        assert!(!boxed.config_surfaces().is_empty());
        assert!(!boxed.plan_mirror_exclusions().is_empty());
    }

    #[test]
    fn surface_schema_declares_object_root_and_typed_maps() {
        let a = adapter();
        let schema = a
            .surface_schema("opencode.json")
            .expect("opencode.json schema");
        assert_eq!(schema.root_shape, Some(crate::adapter::RootShape::Object));
        for path in [
            "provider",
            "providers",
            "mcp",
            "agent",
            "agents",
            "permissions",
        ] {
            assert!(
                schema.owned_key_rules.iter().any(|r| r.path == path),
                "missing rule for {path}"
            );
        }
        assert!(a.surface_schema("opencode.jsonc").is_some());
        assert!(a.surface_schema("tui.json").is_some());
        assert!(a.surface_schema("cli.json").is_some());
        assert!(a.surface_schema("unknown").is_none());
    }

    #[test]
    fn schema_types_v2_keys_according_to_upstream_shapes() {
        let a = adapter();
        let diags = crate::adapter::validate_surface_content(
            &a,
            "opencode.json",
            br#"{"providers": ["acme"], "permissions": "ask", "update": true}"#,
            superai_config::document::DocumentKind::StrictJson,
        );
        assert_eq!(
            diags
                .iter()
                .filter(|d| d.severity == superai_config::document::DiagnosticSeverity::Error)
                .count(),
            3,
            "diags: {diags:?}"
        );
        let ok = crate::adapter::validate_surface_content(
            &a,
            "opencode.json",
            br#"{"providers": {"acme": {}}, "permissions": [{"action": "bash", "resource": "*", "effect": "ask"}], "update": "notify"}"#,
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(
            ok.iter()
                .all(|d| d.severity != superai_config::document::DiagnosticSeverity::Error),
            "diags: {ok:?}"
        );
    }

    #[test]
    fn schema_rejects_non_object_root_and_wrongly_typed_map() {
        let diags = crate::adapter::validate_surface_content(
            &adapter(),
            "opencode.json",
            b"[{\"a\": 1}]",
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("root must be of kind object")),
            "diags: {diags:?}"
        );
        let typed = crate::adapter::validate_surface_content(
            &adapter(),
            "opencode.json",
            br#"{"provider": ["acme"]}"#,
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(
            typed.iter().any(|d| d
                .message
                .contains("`provider` must hold a value of type object")),
            "diags: {typed:?}"
        );
    }

    #[test]
    fn validate_instance_rejects_schema_invalid_config_under_root() {
        let a = adapter();
        let dir = crate::test_util::temp_dir_unique("opencode-schema");
        std::fs::create_dir_all(&dir).unwrap();
        let inst = sample_instance_with_root(dir.to_str().unwrap());
        a.validate_instance(&inst).unwrap();
        std::fs::write(dir.join("opencode.json"), br#"{"theme": "dark"}"#).unwrap();
        a.validate_instance(&inst).unwrap();
        std::fs::write(dir.join("opencode.json"), br#"{"mcp": "not-a-map"}"#).unwrap();
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::SchemaValidation { details, .. } => {
                assert!(details.contains("[opencode/opencode.json]"), "{details}");
            }
            other => panic!("expected SchemaValidation, got {other:?}"),
        }
        drop(std::fs::remove_dir_all(&dir));
    }
}
