//! Harness adapter trait and supporting types.
//!
//! The adapter is the narrow seam between superai's domain and a single
//! harness's on-disk layout. Every method is synchronous and object-safe so
//! adapters can be stored as `Box<dyn Adapter>`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, InstallPresence};

// ---------------------------------------------------------------------------
// Adapter revision
// ---------------------------------------------------------------------------

/// Revision of the adapter implementation, equal to the crate version.
pub const ADAPTER_REVISION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Product status
// ---------------------------------------------------------------------------

/// Lifecycle status of the upstream product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductStatus {
    /// Actively maintained and recommended for new use.
    Active,
    /// Public preview or beta, API may still change.
    Preview,
    /// Early-access program, schema drift expected.
    Eap,
    /// Retired but still documented; no new features.
    Retired,
    /// Archived and read-only upstream.
    Archived,
    /// Sunset announced, removal date known.
    Sunset,
    /// Acquired or rebranded, successor exists.
    Acquired,
    /// Status could not be determined from research docs.
    Unknown,
}

impl fmt::Display for ProductStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Active => "active",
            Self::Preview => "preview",
            Self::Eap => "eap",
            Self::Retired => "retired",
            Self::Archived => "archived",
            Self::Sunset => "sunset",
            Self::Acquired => "acquired",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// Platform
// ---------------------------------------------------------------------------

/// Operating system for adapter support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    /// Linux and WSL.
    Linux,
    /// macOS.
    Macos,
    /// Windows.
    Windows,
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
        };
        f.write_str(s)
    }
}

/// CPU architecture for adapter support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    /// `x86_64` / `amd64`.
    X86_64,
    /// `aarch64` / `arm64`.
    Aarch64,
    /// Any architecture (portable shell wrapper).
    Any,
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::Any => "any",
        };
        f.write_str(s)
    }
}

/// Supported OS/arch pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Platform {
    /// Operating system.
    pub os: Os,
    /// CPU architecture.
    pub arch: Arch,
}

impl Platform {
    /// Create a platform pair.
    pub fn new(os: Os, arch: Arch) -> Self {
        Self { os, arch }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.os, self.arch)
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Confidence of a detection probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionConfidence {
    /// Multiple consistent evidence signals.
    High,
    /// Single solid signal.
    Medium,
    /// Indirect or heuristic signal.
    Low,
    /// No evidence.
    None,
}

impl fmt::Display for DetectionConfidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::None => "none",
        };
        f.write_str(s)
    }
}

/// Result of probing whether a harness is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectionResult {
    /// Install presence as observed.
    pub present: InstallPresence,
    /// Detected version string, if probe succeeded.
    pub version: Option<String>,
    /// Evidence lines that led to the result.
    pub evidence: Vec<String>,
    /// Confidence in the result.
    pub confidence: DetectionConfidence,
}

impl DetectionResult {
    /// Create a new detection result.
    pub fn new(
        present: InstallPresence,
        version: Option<String>,
        evidence: Vec<String>,
        confidence: DetectionConfidence,
    ) -> Self {
        Self {
            present,
            version,
            evidence,
            confidence,
        }
    }

    /// Convenience for absent installs.
    pub fn absent(evidence: Vec<String>) -> Self {
        Self {
            present: InstallPresence::Absent,
            version: None,
            evidence,
            confidence: DetectionConfidence::High,
        }
    }
}

// ---------------------------------------------------------------------------
// Config surface types
// ---------------------------------------------------------------------------

/// Kind of on-disk document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    /// Strict JSON.
    Json,
    /// JSON with comments and trailing commas.
    Jsonc,
    /// TOML.
    Toml,
    /// YAML.
    Yaml,
    /// Dot-env style environment file.
    Env,
    /// Line-oriented text fragment or include.
    TextFragment,
    /// Executable script (e.g. `crushrc`).
    Executable,
    /// `SQLite` database (auto-managed, not writable).
    Sqlite,
    /// System keychain or credential store (not writable).
    Keychain,
    /// Opaque internal state, not directly editable.
    Opaque,
}

impl fmt::Display for DocumentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Json => "json",
            Self::Jsonc => "jsonc",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
            Self::Env => "env",
            Self::TextFragment => "text_fragment",
            Self::Executable => "executable",
            Self::Sqlite => "sqlite",
            Self::Keychain => "keychain",
            Self::Opaque => "opaque",
        };
        f.write_str(s)
    }
}

/// Scope where a surface is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigScope {
    /// Machine-wide managed or system location.
    SystemManaged,
    /// Per-user global config.
    User,
    /// Per-instance isolated root.
    Instance,
    /// Project or workspace local.
    ProjectWorkspace,
    /// Session, inline, or command-line flag.
    SessionInline,
    /// Internal harness state, not user-facing.
    Internal,
}

impl fmt::Display for ConfigScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::SystemManaged => "system_managed",
            Self::User => "user",
            Self::Instance => "instance",
            Self::ProjectWorkspace => "project_workspace",
            Self::SessionInline => "session_inline",
            Self::Internal => "internal",
        };
        f.write_str(s)
    }
}

/// Who owns a surface for mutation purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceOwnership {
    /// User-editable config file.
    UserEditable,
    /// Harness-managed, should not be hand-edited.
    HarnessManaged,
    /// External secret store (keychain, SSO cache).
    ExternalSecretStore,
    /// Created and owned by superai itself.
    SuperaiCreated,
}

impl fmt::Display for SurfaceOwnership {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::UserEditable => "user_editable",
            Self::HarnessManaged => "harness_managed",
            Self::ExternalSecretStore => "external_secret_store",
            Self::SuperaiCreated => "superai_created",
        };
        f.write_str(s)
    }
}

/// What happens after a surface is mutated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartBehavior {
    /// No restart needed.
    None,
    /// Reload or re-read on next launch.
    Reload,
    /// Full restart required.
    Restart,
    /// Re-login or re-auth required.
    ReLogin,
}

impl fmt::Display for RestartBehavior {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::None => "none",
            Self::Reload => "reload",
            Self::Restart => "restart",
            Self::ReLogin => "re_login",
        };
        f.write_str(s)
    }
}

/// How to resolve a path for a given OS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathResolver {
    /// Linux hint, if any.
    pub linux: Option<String>,
    /// macOS hint, if any.
    pub macos: Option<String>,
    /// Windows hint, if any.
    pub windows: Option<String>,
    /// Fallback or generic description.
    pub fallback: String,
}

impl PathResolver {
    /// Create a resolver from optional per-OS hints.
    pub fn new(
        linux: Option<&str>,
        macos: Option<&str>,
        windows: Option<&str>,
        fallback: &str,
    ) -> Self {
        Self {
            linux: linux.map(ToOwned::to_owned),
            macos: macos.map(ToOwned::to_owned),
            windows: windows.map(ToOwned::to_owned),
            fallback: fallback.to_owned(),
        }
    }

    /// Constant fallback-only resolver.
    pub fn fallback_only(fallback: &str) -> Self {
        Self {
            linux: None,
            macos: None,
            windows: None,
            fallback: fallback.to_owned(),
        }
    }
}

/// A single config surface the adapter knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSurface {
    /// Stable identifier for the surface, e.g. `settings.json`.
    pub id: String,
    /// How to locate the surface on each OS.
    pub path_resolver: PathResolver,
    /// On-disk document kind.
    pub kind: DocumentKind,
    /// Scope where the surface is read.
    pub scope: ConfigScope,
    /// Ownership classification.
    pub ownership: SurfaceOwnership,
    /// Precedence rank, lower is overridden by higher.
    pub precedence: u8,
    /// Selectors (JSON pointer, TOML key, etc.) that superai owns.
    pub owned_selectors: Vec<String>,
    /// Whether a backup is required before writing.
    pub backup_required: bool,
    /// Restart or reload behavior after mutation.
    pub restart_behavior: RestartBehavior,
}

impl ConfigSurface {
    /// Create a surface with the most common defaults.
    pub fn new(
        id: &str,
        path_resolver: PathResolver,
        kind: DocumentKind,
        scope: ConfigScope,
        ownership: SurfaceOwnership,
    ) -> Self {
        Self {
            id: id.to_owned(),
            path_resolver,
            kind,
            scope,
            ownership,
            precedence: 0,
            owned_selectors: Vec::new(),
            backup_required: true,
            restart_behavior: RestartBehavior::None,
        }
    }
}

// ---------------------------------------------------------------------------
// Version resolution
// ---------------------------------------------------------------------------

/// How the adapter maps a detected harness version to a schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionResolution {
    /// Raw version string as detected, if any.
    pub detected_version: Option<String>,
    /// Adapter schema version that matches the detected version, if known.
    pub schema_version: Option<String>,
    /// Whether the detected version is compatible for writes.
    pub compatible: bool,
    /// Evidence or notes about the resolution.
    pub notes: Vec<String>,
}

impl VersionResolution {
    /// Create a resolution result.
    pub fn new(
        detected_version: Option<String>,
        schema_version: Option<String>,
        compatible: bool,
    ) -> Self {
        Self {
            detected_version,
            schema_version,
            compatible,
            notes: Vec::new(),
        }
    }

    /// Unknown version, not compatible for writes.
    pub fn unknown() -> Self {
        Self {
            detected_version: None,
            schema_version: None,
            compatible: false,
            notes: vec!["version unknown, writes blocked".to_owned()],
        }
    }
}

// ---------------------------------------------------------------------------
// Wrapper plan
// ---------------------------------------------------------------------------

/// Plan for invoking an isolated instance via a wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrapperPlan {
    /// Environment variables to set before exec.
    pub env_vars: Vec<(String, String)>,
    /// Extra arguments to prepend to the harness binary.
    pub args: Vec<String>,
    /// Human-readable description of the isolation mechanism.
    pub description: String,
}

impl WrapperPlan {
    /// Create a new wrapper plan.
    pub fn new(description: &str) -> Self {
        Self {
            env_vars: Vec::new(),
            args: Vec::new(),
            description: description.to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Skill destination modes
// ---------------------------------------------------------------------------

/// How a skill reaches an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillMode {
    /// Destination points to registry root (symlink/shallow).
    LinkAll,
    /// Destination contains links to chosen skill directories.
    LinkSelected,
    /// Destination owns copies of chosen skill directories.
    CopySelected,
}

impl fmt::Display for SkillMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::LinkAll => "link_all",
            Self::LinkSelected => "link_selected",
            Self::CopySelected => "copy_selected",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// MCP transport and adapter declarations (EXT-06..08)
// ---------------------------------------------------------------------------

/// Transport for an MCP server (EXT-08).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    /// Standard I/O (command + args).
    Stdio,
    /// Server-sent events (URL).
    Sse,
    /// Plain HTTP (URL).
    Http,
    /// WebSocket (URL).
    WebSocket,
    /// Streamable HTTP (URL).
    StreamableHttp,
}

impl fmt::Display for McpTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Stdio => "stdio",
            Self::Sse => "sse",
            Self::Http => "http",
            Self::WebSocket => "websocket",
            Self::StreamableHttp => "streamable_http",
        };
        f.write_str(s)
    }
}

/// Where an adapter expects MCP servers to be persisted (EXT-08/09).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAdapterDecl {
    /// File name or path hint relative to instance root (e.g. `settings.json`).
    pub dest_file: String,
    /// Key inside the file that holds the server map (e.g. `mcpServers`).
    pub dest_key: String,
    /// Document kind of the destination file.
    pub kind: DocumentKind,
    /// Scope of the destination file.
    pub scope: ConfigScope,
    /// Restart required after mutation.
    pub restart: RestartBehavior,
}

impl McpAdapterDecl {
    /// Create a new MCP adapter declaration.
    pub fn new(
        dest_file: &str,
        dest_key: &str,
        kind: DocumentKind,
        scope: ConfigScope,
        restart: RestartBehavior,
    ) -> Self {
        Self {
            dest_file: dest_file.to_owned(),
            dest_key: dest_key.to_owned(),
            kind,
            scope,
            restart,
        }
    }
}

/// Plugin kind is adapter-specific (EXT-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    /// Directory or bundle on the filesystem.
    DirectoryBundle,
    /// Single config entry inside a harness config file.
    ConfigEntry,
    /// NPM / package reference requiring external install.
    NpmRef,
    /// Marketplace install record requiring external command.
    MarketplaceRecord,
    /// Extension script (e.g. `.js` loaded by harness).
    ExtensionScript,
}

impl fmt::Display for PluginKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::DirectoryBundle => "directory_bundle",
            Self::ConfigEntry => "config_entry",
            Self::NpmRef => "npm_ref",
            Self::MarketplaceRecord => "marketplace_record",
            Self::ExtensionScript => "extension_script",
        };
        f.write_str(s)
    }
}

/// Adapter declaration for plugin persistence (EXT-06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAdapterDecl {
    /// Source hint (e.g. registry path or marketplace identifier).
    pub source_hint: String,
    /// Destination file hint relative to instance root.
    pub dest_file: String,
    /// Optional key inside destination file for config-entry plugins.
    pub dest_key: Option<String>,
    /// Plugin kind this declaration handles.
    pub kind: PluginKind,
    /// Whether install requires executing a harness/package command.
    pub requires_execution: bool,
    /// Enable semantics (human description).
    pub enable_semantics: String,
    /// Disable semantics (human description).
    pub disable_semantics: String,
    /// Remove semantics (human description).
    pub remove_semantics: String,
    /// Dependency effect (human description).
    pub dependency_effect: String,
    /// Restart requirement after mutation.
    pub restart: RestartBehavior,
}

impl PluginAdapterDecl {
    /// Create a file/config safe plugin declaration (no execution).
    pub fn file_config(
        dest_file: &str,
        dest_key: Option<&str>,
        kind: PluginKind,
        restart: RestartBehavior,
    ) -> Self {
        Self {
            source_hint: "local".to_owned(),
            dest_file: dest_file.to_owned(),
            dest_key: dest_key.map(ToOwned::to_owned),
            kind,
            requires_execution: false,
            enable_semantics: "add entry / create link".to_owned(),
            disable_semantics: "remove entry / remove link (reversible)".to_owned(),
            remove_semantics: "remove owned entry only".to_owned(),
            dependency_effect: "none (file/config only)".to_owned(),
            restart,
        }
    }

    /// Create a declaration that requires external command execution (needs approval).
    pub fn requires_execution(
        source_hint: &str,
        dest_file: &str,
        kind: PluginKind,
        restart: RestartBehavior,
    ) -> Self {
        Self {
            source_hint: source_hint.to_owned(),
            dest_file: dest_file.to_owned(),
            dest_key: None,
            kind,
            requires_execution: true,
            enable_semantics: "execute harness command to enable".to_owned(),
            disable_semantics: "execute harness command to disable".to_owned(),
            remove_semantics:
                "execute harness command to remove; shared deps retained until no consumer"
                    .to_owned(),
            dependency_effect: "shared package dependency".to_owned(),
            restart,
        }
    }
}

// ---------------------------------------------------------------------------
// Adapter trait
// ---------------------------------------------------------------------------

/// Harness adapter: read-only probes plus plans for mutation.
///
/// Every implementor is object-safe, `Send + Sync`, and has no async methods.
/// Plans describe what would be written; the caller commits via the safe
/// mutation layer.
pub trait Adapter: Send + Sync + fmt::Debug {
    /// Stable harness identifier.
    fn id(&self) -> HarnessId;

    /// Human display name.
    fn display_name(&self) -> &str;

    /// Product lifecycle status.
    fn product_status(&self) -> ProductStatus;

    /// Platforms this adapter claims to support.
    fn supported_platforms(&self) -> Vec<Platform>;

    /// Adapter implementation revision (crate version).
    fn adapter_revision(&self) -> &str;

    /// Link to the research document that justifies the adapter.
    fn research_doc_link(&self) -> &str;

    /// Date the research was last verified, `YYYY-MM-DD`.
    fn last_verified_date(&self) -> &str;

    /// Detect whether the harness is installed and which version.
    fn detection(&self) -> DetectionResult;

    /// Map the detected harness version to a config schema.
    fn version_resolution(&self) -> VersionResolution;

    /// All config surfaces for this harness.
    fn config_surfaces(&self) -> Vec<ConfigSurface>;

    /// Which operations are supported, as `(operation, support)` pairs.
    fn supported_operations(&self) -> Vec<(String, AdapterSupport)>;

    /// File patterns to exclude when mirroring an instance root.
    fn plan_mirror_exclusions(&self) -> Vec<String>;

    /// Plan how to invoke the instance via a wrapper.
    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError>;

    /// Candidate absolute paths that might be unmanaged roots for this harness.
    fn scan_candidates(&self) -> Vec<String>;

    /// Validate that an instance record is coherent for this harness.
    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError>;

    /// Which skill destination modes this harness supports.
    fn supported_skill_modes(&self) -> Vec<SkillMode> {
        Vec::new()
    }

    /// MCP adapter declaration, if the harness supports MCP servers (EXT-08).
    fn mcp_decl(&self) -> Option<McpAdapterDecl> {
        None
    }

    /// Plugin adapter declaration, if the harness supports plugins (EXT-06).
    fn plugin_decl(&self) -> Option<PluginAdapterDecl> {
        None
    }
}

// ---------------------------------------------------------------------------
// Generic adapter backed by catalog data
// ---------------------------------------------------------------------------

/// Generic adapter constructed from catalog ledger data.
///
/// This is the runtime representation of a provisional ledger row. Real
/// harness-specific adapters will replace the placeholder behaviours for
/// detection, surfaces, and wrapper planning.
#[derive(Debug, Clone)]
pub struct GenericAdapter {
    id: HarnessId,
    display_name: String,
    product_status: ProductStatus,
    supported_platforms: Vec<Platform>,
    adapter_revision: String,
    research_doc_link: String,
    last_verified_date: String,
    support: AdapterSupport,
    reason: String,
    source: String,
}

impl GenericAdapter {
    /// Create a generic adapter from catalog row fields.
    ///
    /// All string parameters are `&str` to satisfy the `&str` over `String`
    /// guideline; they are cloned into owned storage.
    #[expect(
        clippy::too_many_arguments,
        reason = "catalog row maps to adapter fields"
    )]
    pub fn new(
        id: HarnessId,
        display_name: &str,
        product_status: ProductStatus,
        research_doc_link: &str,
        last_verified_date: &str,
        support: AdapterSupport,
        reason: &str,
        source: &str,
    ) -> Self {
        let platforms = vec![
            Platform::new(Os::Linux, Arch::Any),
            Platform::new(Os::Macos, Arch::Any),
            Platform::new(Os::Windows, Arch::Any),
        ];
        Self {
            id,
            display_name: display_name.to_owned(),
            product_status,
            supported_platforms: platforms,
            adapter_revision: ADAPTER_REVISION.to_owned(),
            research_doc_link: research_doc_link.to_owned(),
            last_verified_date: last_verified_date.to_owned(),
            support,
            reason: reason.to_owned(),
            source: source.to_owned(),
        }
    }

    /// Overall ledger support for this adapter.
    pub fn ledger_support(&self) -> AdapterSupport {
        self.support
    }

    /// Ledger reason.
    pub fn ledger_reason(&self) -> &str {
        &self.reason
    }

    /// Ledger source document.
    pub fn ledger_source(&self) -> &str {
        &self.source
    }
}

impl Adapter for GenericAdapter {
    fn id(&self) -> HarnessId {
        self.id.clone()
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn product_status(&self) -> ProductStatus {
        self.product_status
    }

    fn supported_platforms(&self) -> Vec<Platform> {
        self.supported_platforms.clone()
    }

    fn adapter_revision(&self) -> &str {
        &self.adapter_revision
    }

    fn research_doc_link(&self) -> &str {
        &self.research_doc_link
    }

    fn last_verified_date(&self) -> &str {
        &self.last_verified_date
    }

    fn detection(&self) -> DetectionResult {
        // Generic adapters do not probe the filesystem; they report unknown
        // version so callers know to treat the install as absent.
        DetectionResult {
            present: InstallPresence::Absent,
            version: None,
            evidence: vec![format!("generic adapter for {}", self.display_name)],
            confidence: DetectionConfidence::Low,
        }
    }

    fn version_resolution(&self) -> VersionResolution {
        VersionResolution::unknown()
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        // Placeholder single surface; real adapters enumerate every surface.
        let resolver = PathResolver::fallback_only(&self.source);
        let surface = ConfigSurface {
            id: "generic".to_owned(),
            path_resolver: resolver,
            kind: DocumentKind::Json,
            scope: ConfigScope::User,
            ownership: SurfaceOwnership::UserEditable,
            precedence: 0,
            owned_selectors: Vec::new(),
            backup_required: true,
            restart_behavior: RestartBehavior::None,
        };
        vec![surface]
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        // Expose the ledger support as the overall capability map.
        vec![
            ("detect".to_owned(), self.support),
            ("read_config".to_owned(), self.support),
            ("write_config".to_owned(), self.support),
            ("manage_skills".to_owned(), self.support),
            ("manage_mcp".to_owned(), self.support),
        ]
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        // Generic exclusions that are safe for most harnesses.
        vec![
            "history.jsonl".to_owned(),
            "debug/*".to_owned(),
            ".credentials.json".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        if instance.harness != self.id {
            return Err(CoreError::Validation {
                field: "harness".to_owned(),
                reason: format!(
                    "instance harness `{}` does not match adapter `{}`",
                    instance.harness, self.id
                ),
            });
        }
        let mut plan = WrapperPlan::new(&format!("generic wrapper for {}", self.display_name));
        // Provide a relocated-root style hint; real adapters use the correct env var.
        plan.env_vars.push((
            format!("{}_CONFIG_DIR", self.id.as_str().to_uppercase()),
            instance.config_root.to_string(),
        ));
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        // Generic scan hints based on home-relative paths.
        vec![
            format!("~/.{}", self.id.as_str()),
            format!("~/.{}-work", self.id.as_str()),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        if instance.harness != self.id {
            return Err(CoreError::Validation {
                field: "harness".to_owned(),
                reason: format!("expected harness `{}`, got `{}`", self.id, instance.harness),
            });
        }
        instance.validate()?;
        Ok(())
    }

    fn supported_skill_modes(&self) -> Vec<SkillMode> {
        match self.support {
            AdapterSupport::Full | AdapterSupport::Constrained => {
                vec![
                    SkillMode::LinkAll,
                    SkillMode::LinkSelected,
                    SkillMode::CopySelected,
                ]
            }
            AdapterSupport::SingleInstance => vec![SkillMode::CopySelected],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::adapter::{
        ADAPTER_REVISION, Adapter, ConfigScope, ConfigSurface, DetectionConfidence,
        DetectionResult, DocumentKind, GenericAdapter, PathResolver, ProductStatus,
        RestartBehavior, SurfaceOwnership,
    };
    use crate::error::CoreError;
    use crate::ids::HarnessId;
    use crate::state::{AdapterSupport, InstallPresence};

    fn sample_adapter() -> GenericAdapter {
        let id = HarnessId::new("claude-code").unwrap();
        GenericAdapter::new(
            id,
            "Claude Code",
            ProductStatus::Active,
            "docs/harness-configs/claude-code.md",
            "2026-08-25",
            AdapterSupport::Full,
            "relocated-root isolation verified",
            "docs/harness-configs/claude-code.md",
        )
    }

    #[test]
    fn generic_adapter_is_object_safe() {
        let adapter: Box<dyn Adapter> = Box::new(sample_adapter());
        assert_eq!(adapter.id().as_str(), "claude-code");
        assert_eq!(adapter.display_name(), "Claude Code");
        assert_eq!(adapter.product_status(), ProductStatus::Active);
        assert!(!adapter.supported_platforms().is_empty());
        assert_eq!(adapter.adapter_revision(), ADAPTER_REVISION);
        assert!(!adapter.research_doc_link().is_empty());
        assert!(!adapter.last_verified_date().is_empty());
        let detection = adapter.detection();
        assert_eq!(detection.present, InstallPresence::Absent);
        let version = adapter.version_resolution();
        assert!(!version.compatible);
        let surfaces = adapter.config_surfaces();
        assert!(!surfaces.is_empty());
        let ops = adapter.supported_operations();
        assert!(!ops.is_empty());
        for (name, support) in ops {
            assert!(!name.is_empty());
            assert_eq!(support, AdapterSupport::Full);
        }
        assert!(!adapter.plan_mirror_exclusions().is_empty());
        assert!(!adapter.scan_candidates().is_empty());
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        use crate::ids::{InstanceId, InstanceName};
        use crate::instance::Instance;
        use crate::paths::AbsolutePath;
        use crate::state::{InstanceOrigin, Isolation, Ownership};
        let adapter = sample_adapter();
        let inst = Instance {
            id: InstanceId::new("test-id-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new("codex-cli").unwrap(),
            config_root: AbsolutePath::new("/tmp/.codex-work").unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: ADAPTER_REVISION.to_owned(),
        };
        let err = adapter.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("expected validation, got {other:?}"),
        }
    }

    #[test]
    fn generic_adapter_support_is_preserved() {
        let id = HarnessId::new("deepseek-harness").unwrap();
        let adapter = GenericAdapter::new(
            id,
            "DeepSeek Harness",
            ProductStatus::Preview,
            "docs/harness-configs/deepseek-harness.md",
            "2026-08-25",
            AdapterSupport::ResearchBlocked,
            "developer preview, schema incomplete",
            "docs/harness-configs/deepseek-harness.md",
        );
        assert_eq!(adapter.ledger_support(), AdapterSupport::ResearchBlocked);
        assert_eq!(adapter.product_status(), ProductStatus::Preview);
        let boxed: Box<dyn Adapter> = Box::new(adapter);
        let ops = boxed.supported_operations();
        for (_, support) in ops {
            assert_eq!(support, AdapterSupport::ResearchBlocked);
        }
    }

    #[test]
    fn product_status_display_and_roundtrip() {
        let cases = [
            (ProductStatus::Active, "active"),
            (ProductStatus::Preview, "preview"),
            (ProductStatus::Eap, "eap"),
            (ProductStatus::Retired, "retired"),
            (ProductStatus::Archived, "archived"),
            (ProductStatus::Sunset, "sunset"),
            (ProductStatus::Acquired, "acquired"),
            (ProductStatus::Unknown, "unknown"),
        ];
        for (status, expected) in cases {
            assert_eq!(status.to_string(), expected);
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json, format!("\"{expected}\""));
            let back: ProductStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn detection_absent_helper() {
        let result = DetectionResult::absent(vec!["no binary in PATH".to_owned()]);
        assert_eq!(result.present, InstallPresence::Absent);
        assert!(result.version.is_none());
        assert_eq!(result.confidence, DetectionConfidence::High);
    }

    #[test]
    fn document_kind_display() {
        assert_eq!(DocumentKind::Json.to_string(), "json");
        assert_eq!(DocumentKind::Executable.to_string(), "executable");
        let json = serde_json::to_string(&DocumentKind::Sqlite).unwrap();
        assert_eq!(json, "\"sqlite\"");
    }

    #[test]
    fn path_resolver_fallback() {
        let resolver = PathResolver::fallback_only("~/fallback");
        assert!(resolver.linux.is_none());
        assert_eq!(resolver.fallback, "~/fallback");
        let full = PathResolver::new(
            Some("~/.config/a"),
            Some("~/Library/a"),
            Some("%APPDATA%\\a"),
            "~/fallback",
        );
        assert_eq!(full.linux.as_deref(), Some("~/.config/a"));
        assert_eq!(full.macos.as_deref(), Some("~/Library/a"));
    }

    #[test]
    fn config_surface_new_defaults() {
        let resolver = PathResolver::fallback_only("~/.claude/settings.json");
        let surface = ConfigSurface::new(
            "settings.json",
            resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        assert_eq!(surface.id, "settings.json");
        assert!(surface.backup_required);
        assert_eq!(surface.restart_behavior, RestartBehavior::None);
    }

    #[test]
    fn wrapper_plan_env_isolation() {
        use crate::ids::{InstanceId, InstanceName};
        use crate::instance::Instance;
        use crate::paths::AbsolutePath;
        use crate::state::{InstanceOrigin, Isolation, Ownership};
        let adapter = sample_adapter();
        let inst = Instance {
            id: InstanceId::new("test-id-2").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::new("/tmp/.claude-work").unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: ADAPTER_REVISION.to_owned(),
        };
        let plan = adapter.plan_wrapper(&inst).unwrap();
        assert!(!plan.description.is_empty());
        assert!(!plan.env_vars.is_empty());
        // env var should contain the config root string
        let found = plan
            .env_vars
            .iter()
            .any(|(_, v)| v.contains(".claude-work"));
        assert!(found, "wrapper env must reference instance root");
    }
}
