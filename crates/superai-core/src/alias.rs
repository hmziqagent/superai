//! Multi-instance aliases: isolated launch configs under a caller-chosen base;
//! the on-disk manifest is the only registry, read fresh every operation.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use superai_config::transaction::{FileAction, Transaction};

use crate::adapter::{Adapter, McpAdapterDecl, PluginAdapterDecl, PluginKind, WrapperPlan};
use crate::error::{CoreError, RedactedString, Result};
use crate::ids::{HarnessId, InstanceId, InstanceName, ProviderId};
use crate::instance::{Instance, WrapperRef};
use crate::mcp::{self, McpServerDef};
use crate::paths::{AbsolutePath, ExecutableRef, WrapperPath};
use crate::plugin::{self, PluginSource};
use crate::provider::{AuthStyle, Protocol, ProviderDefinition};
use crate::provider_render::{ProviderChange, ProviderChangeOptions, commit_provider_change};
use crate::state::{InstanceOrigin, Isolation, Ownership};
use crate::wrapper as wrapper_helper;

/// Marker file inside every alias root proving superai ownership. Line 1 is
/// the harness id, line 2 the alias name (exact case).
pub const ALIAS_MARKER_FILE: &str = ".superai-alias";

/// Manifest file under the alias base directory listing every alias.
pub const ALIAS_MANIFEST_FILE: &str = "aliases.json";

/// Directory under an alias root holding superai's own per-alias state (the
/// plugin registry). Dot-prefixed so harnesses ignore it.
const ALIAS_STATE_DIR: &str = ".superai";

const MANIFEST_SCHEMA_KEY: &str = "schema_version";
const MANIFEST_ALIASES_KEY: &str = "aliases";
/// Current alias manifest schema version.
pub const ALIAS_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Per-alias provider reference file (run-5): names, URL, and model pinning
/// only, never a secret; the token rides the launch env.
pub const ALIAS_PROVIDER_REF_FILE: &str = ".superai/provider.json";

/// Current provider-reference schema version.
pub const ALIAS_PROVIDER_REF_SCHEMA_VERSION: u32 = 1;

/// Harnesses whose real binaries demonstrably relocate their config under a
/// virtualized `HOME` (live evidence); every other harness keeps the guard.
pub const HOME_VIRT_HARNESSES: &[&str] = &["claude-desktop", "chatgpt-desktop"];

/// HOME-virt MCP destination overrides: the seeded file must land where the
/// HOME-relocated binary actually reads it (relative to the alias root).
const HOME_VIRT_MCP_DESTS: &[(&str, &str)] = &[(
    "claude-desktop",
    // XDG_CONFIG_HOME=<root>/.config + Electron appData appname "Claude".
    ".config/Claude/claude_desktop_config.json",
)];

/// Env vars the claude-code family reads for an env-carried provider
/// (code.claude.com/docs/en/env-vars, 2026-09-19).
const CLAUDE_CODE_BASE_URL_VAR: &str = "ANTHROPIC_BASE_URL";
const CLAUDE_CODE_AUTH_TOKEN_VAR: &str = "ANTHROPIC_AUTH_TOKEN";
const CLAUDE_CODE_API_KEY_VAR: &str = "ANTHROPIC_API_KEY";
const CLAUDE_CODE_MODEL_VAR: &str = "ANTHROPIC_MODEL";
const CLAUDE_CODE_HAIKU_VAR: &str = "ANTHROPIC_DEFAULT_HAIKU_MODEL";

/// Deterministic instance id for an alias: stable across recreation of the
/// same (harness, name, root) triple.
fn derive_alias_instance_id(
    harness: &HarnessId,
    name: &InstanceName,
    root: &AbsolutePath,
) -> Result<InstanceId> {
    let mut hasher = DefaultHasher::new();
    harness.as_str().hash(&mut hasher);
    name.as_str().hash(&mut hasher);
    root.to_string().hash(&mut hasher);
    let candidate = format!("alias-{:016x}", hasher.finish());
    InstanceId::new(&candidate).map_err(|e| CoreError::Validation {
        field: "id".to_owned(),
        reason: format!("generated alias id invalid: {e}"),
    })
}

fn transaction_operation_id(prefix: &str) -> Result<superai_config::transaction::OperationId> {
    let candidate = crate::registry::unique_operation_string(prefix);
    superai_config::transaction::OperationId::new(&candidate).map_err(|e| CoreError::Validation {
        field: "operation_id".to_owned(),
        reason: format!("generated operation id invalid: {e}"),
    })
}

/// How a harness carries a third-party provider override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderCarriage {
    /// The binary reads gateway endpoint/model/auth from its environment
    /// (claude-code: `ANTHROPIC_*`); nothing is written to the config.
    EnvCarried,
    /// The binary reads a provider table from its config file (codex family:
    /// `$CODEX_HOME/config.toml`). Only the `env_key` NAME is written; the token rides the launch env.
    ConfigCarried,
}

fn provider_carriage(harness: &HarnessId) -> Result<ProviderCarriage> {
    match harness.as_str() {
        "claude-code" => Ok(ProviderCarriage::EnvCarried),
        "codex-cli" | "chatgpt-desktop" => Ok(ProviderCarriage::ConfigCarried),
        other => Err(CoreError::UnsupportedOperation {
            harness: other.to_owned(),
            operation: "attach provider profile".to_owned(),
            reason: format!(
                "third-party provider overrides are modeled for claude-code \
                 (env-carried) and the codex family (config-carried); harness \
                 `{other}` declares no provider surface for aliases"
            ),
        }),
    }
}

/// Whether `name` is a syntactically valid environment variable name
/// (identifier, never PATH).
fn valid_env_var_name(name: &str) -> bool {
    !name.is_empty()
        && name != "PATH"
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A third-party inference provider attached to an alias at creation. The
/// manifest and state files carry names/URLs/model ids only, never a token value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProfile {
    /// Provider id; becomes the `model_providers.<id>` table key for the
    /// config-carried family.
    pub provider_id: ProviderId,
    /// Gateway base URL. The harness appends its protocol path (claude-code
    /// and the desktop 3P gateway append `/v1/messages`).
    pub base_url: String,
    /// Wire protocol the gateway serves.
    pub protocol: Protocol,
    /// Auth style the gateway expects.
    pub auth_style: AuthStyle,
    /// Env var NAME the token rides at launch (codex: the `env_key` written
    /// into the provider table; claude-code: `ANTHROPIC_AUTH_TOKEN`).
    pub auth_env_var: String,
    /// Model pinning (`ANTHROPIC_MODEL` / codex `model`).
    pub model: Option<String>,
    /// Haiku-class model pinning for background tasks
    /// (`ANTHROPIC_DEFAULT_HAIKU_MODEL`).
    pub small_model: Option<String>,
}

impl ProviderProfile {
    /// Minimal profile; protocol defaults to `anthropic` (bearer auth).
    pub fn new(provider_id: ProviderId, base_url: impl Into<String>) -> Self {
        Self {
            provider_id,
            base_url: base_url.into(),
            protocol: Protocol::Anthropic,
            auth_style: AuthStyle::Bearer,
            auth_env_var: CLAUDE_CODE_AUTH_TOKEN_VAR.to_owned(),
            model: None,
            small_model: None,
        }
    }

    /// Set the wire protocol.
    #[must_use]
    pub fn with_protocol(mut self, protocol: Protocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Set the auth style.
    #[must_use]
    pub fn with_auth_style(mut self, auth_style: AuthStyle) -> Self {
        self.auth_style = auth_style;
        self
    }

    /// Set the env var name the token rides at launch.
    #[must_use]
    pub fn with_auth_env_var(mut self, name: impl Into<String>) -> Self {
        self.auth_env_var = name.into();
        self
    }

    /// Pin the default model.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Pin the haiku-class (background) model.
    #[must_use]
    pub fn with_small_model(mut self, model: impl Into<String>) -> Self {
        self.small_model = Some(model.into());
        self
    }

    /// Validate the profile against the harness's modeled provider surface
    /// (researcher-exact semantics; refusals cite them).
    fn validate_for(&self, harness: &HarnessId, home_virt: bool) -> Result<()> {
        if !valid_env_var_name(&self.auth_env_var) {
            return Err(CoreError::Validation {
                field: "provider.auth_env_var".to_owned(),
                reason: format!(
                    "`{}` is not a valid env var name (and PATH is never set)",
                    self.auth_env_var
                ),
            });
        }
        match provider_carriage(harness)? {
            ProviderCarriage::EnvCarried => {
                if self.protocol != Protocol::Anthropic {
                    return Err(CoreError::Validation {
                        field: "provider.protocol".to_owned(),
                        reason: format!(
                            "claude-code speaks the anthropic messages protocol; \
                             protocol `{}` has no env mapping",
                            self.protocol
                        ),
                    });
                }
                let expected = match &self.auth_style {
                    AuthStyle::Bearer => CLAUDE_CODE_AUTH_TOKEN_VAR,
                    AuthStyle::XApiKey | AuthStyle::ApiKeyHeader => CLAUDE_CODE_API_KEY_VAR,
                    other => {
                        return Err(CoreError::Validation {
                            field: "provider.auth_style".to_owned(),
                            reason: format!(
                                "claude-code env auth is bearer \
                                 (ANTHROPIC_AUTH_TOKEN) or x-api-key \
                                 (ANTHROPIC_API_KEY); auth style `{other:?}` has no mapping"
                            ),
                        });
                    }
                };
                if self.auth_env_var != expected {
                    return Err(CoreError::Validation {
                        field: "provider.auth_env_var".to_owned(),
                        reason: format!(
                            "auth style `{:?}` requires env var `{expected}` \
                             (got `{}`); AUTH_TOKEN sends `Authorization: Bearer`, \
                             API_KEY sends `X-Api-Key`",
                            self.auth_style, self.auth_env_var
                        ),
                    });
                }
            }
            ProviderCarriage::ConfigCarried => {
                if self.protocol != Protocol::OpenAiResponses {
                    return Err(CoreError::Validation {
                        field: "provider.protocol".to_owned(),
                        reason: format!(
                            "codex `model_providers.wire_api` supports `responses` only \
                             (the `chat` value was removed from current codex); \
                             protocol `{}` cannot be seeded",
                            self.protocol
                        ),
                    });
                }
                if harness.as_str() == "chatgpt-desktop" && !home_virt {
                    return Err(CoreError::Validation {
                        field: "provider".to_owned(),
                        reason: "chatgpt-desktop reads the shared ~/.codex store; a \
                                 provider can only be seeded into a HOME-virtualized \
                                 alias (whose HOME override isolates ~/.codex), via \
                                 the codex redirect"
                            .to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    /// The `ProviderDefinition` rendered from this profile (auth as a NAME
    /// only; the definition never sees a secret).
    fn to_definition(&self) -> ProviderDefinition {
        let mut def = ProviderDefinition::new(self.provider_id.clone(), self.base_url.clone());
        def.display_name = self.provider_id.to_string();
        def.protocol = self.protocol;
        def.auth_style = self.auth_style.clone();
        def.auth.env_var_names = vec![self.auth_env_var.clone()];
        def.defaults.default_model.clone_from(&self.model);
        def
    }
}

/// The persisted provider reference under the alias root: names, URL, and
/// model pinning only, never a secret value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProviderRef {
    schema_version: u32,
    provider_id: String,
    base_url: String,
    protocol: Protocol,
    auth_style: AuthStyle,
    auth_env_var: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    small_model: Option<String>,
}

fn provider_ref_path(root: &AbsolutePath) -> Result<AbsolutePath> {
    root.join(ALIAS_PROVIDER_REF_FILE)
}

/// Load the per-alias provider reference, fresh from disk (`Ok(None)` when
/// absent); malformed state is a typed schema error, never silently ignored.
fn load_provider_ref(root: &AbsolutePath) -> Result<Option<ProviderRef>> {
    let path = provider_ref_path(root)?;
    match std::fs::read(path.as_path()) {
        Ok(bytes) => {
            let reference: ProviderRef =
                serde_json::from_slice(&bytes).map_err(|e| CoreError::SchemaValidation {
                    path: path.as_path().to_path_buf(),
                    details: format!("malformed alias provider reference: {e}"),
                })?;
            if reference.schema_version != ALIAS_PROVIDER_REF_SCHEMA_VERSION {
                return Err(CoreError::SchemaValidation {
                    path: path.as_path().to_path_buf(),
                    details: format!(
                        "unsupported provider reference schema_version {}: expected \
                         {ALIAS_PROVIDER_REF_SCHEMA_VERSION}",
                        reference.schema_version
                    ),
                });
            }
            Ok(Some(reference))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CoreError::Config(superai_config::ConfigError::Io {
            path: path.as_path().to_path_buf(),
            source: e,
        })),
    }
}

/// Persist the provider reference through the config crate's mutation
/// boundary (backup + atomic replace; the file is superai-created).
fn store_provider_ref(root: &AbsolutePath, profile: &ProviderProfile) -> Result<()> {
    let reference = ProviderRef {
        schema_version: ALIAS_PROVIDER_REF_SCHEMA_VERSION,
        provider_id: profile.provider_id.to_string(),
        base_url: profile.base_url.clone(),
        protocol: profile.protocol,
        auth_style: profile.auth_style.clone(),
        auth_env_var: profile.auth_env_var.clone(),
        model: profile.model.clone(),
        small_model: profile.small_model.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&reference).map_err(|e| CoreError::Validation {
        field: "provider_ref".to_owned(),
        reason: format!("cannot serialize alias provider reference: {e}"),
    })?;
    let path = provider_ref_path(root)?;
    superai_config::transaction::commit_file(
        "alias-provider-ref",
        path.as_path(),
        &bytes,
        superai_config::document::DocumentKind::StrictJson,
    )
    .map_err(CoreError::Config)?;
    Ok(())
}

/// Request to create an alias: harness, name, and the optional sets seeded
/// into the fresh alias root at creation time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasSpec {
    /// Harness the alias launches.
    pub harness: HarnessId,
    /// User-chosen alias label (also the root's last path segment).
    pub name: InstanceName,
    /// MCP servers seeded into the adapter-declared destination. The set
    /// lives in the harness's own config file under the root, not the manifest.
    pub mcp_servers: Vec<McpServerDef>,
    /// Plugins staged through the adapter-declared plugin mechanism, where it
    /// is a file-staged `directory_bundle` (no external execution).
    pub plugins: Vec<PluginSource>,
    /// Binary the alias launches when the adapter's plan names no executable.
    /// Resolution precedence: plan executable, then this pin, then PATH default.
    pub binary: Option<ExecutableRef>,
    /// Third-party provider override (run-5): env-carried for claude-code,
    /// config-carried for the codex family. Never carries a secret value.
    pub provider: Option<ProviderProfile>,
    /// HOME-virtualized instance mode: launch exports `HOME=<alias-root>` and
    /// `XDG_CONFIG_HOME=<alias-root>/.config`; restricted to [`HOME_VIRT_HARNESSES`].
    pub home_virt: bool,
}

impl AliasSpec {
    /// Create a minimal spec (no MCP servers, no plugins, default binary).
    pub fn new(harness: HarnessId, name: InstanceName) -> Self {
        Self {
            harness,
            name,
            mcp_servers: Vec::new(),
            plugins: Vec::new(),
            binary: None,
            provider: None,
            home_virt: false,
        }
    }

    /// Set the MCP server set to seed.
    #[must_use]
    pub fn with_mcp_servers(mut self, servers: Vec<McpServerDef>) -> Self {
        self.mcp_servers = servers;
        self
    }

    /// Set the plugin set to stage.
    #[must_use]
    pub fn with_plugins(mut self, plugins: Vec<PluginSource>) -> Self {
        self.plugins = plugins;
        self
    }

    /// Pin the binary the alias launches.
    #[must_use]
    pub fn with_binary(mut self, binary: ExecutableRef) -> Self {
        self.binary = Some(binary);
        self
    }

    /// Attach a third-party provider override.
    #[must_use]
    pub fn with_provider(mut self, provider: ProviderProfile) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Request a HOME-virtualized instance (desktop alternative 1).
    #[must_use]
    pub fn with_home_virt(mut self) -> Self {
        self.home_virt = true;
        self
    }

    /// The alias's isolation root derived from `base_dir` (see [`alias_root`]).
    pub fn root(&self, base_dir: &Path) -> Result<AbsolutePath> {
        alias_root(base_dir, &self.harness, &self.name)
    }
}

/// Derive the alias config root: `<base>/<harness>/<alias-name>`. Validated
/// identifiers keep every pair's root disjoint and nested under the base.
pub fn alias_root(
    base_dir: &Path,
    harness: &HarnessId,
    name: &InstanceName,
) -> Result<AbsolutePath> {
    AbsolutePath::from_path(base_dir)?
        .join(harness.as_str())?
        .join(name.as_str())
}

/// A recorded alias in the on-disk manifest. Forbidden fields (never
/// serialized): model, endpoint, api keys, skill/mcp/plugin lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliasRecord {
    /// Harness this alias launches.
    pub harness: HarnessId,
    /// Alias label.
    pub name: InstanceName,
    /// The alias's relocated config root.
    pub root: AbsolutePath,
    /// Pinned binary, when the alias does not use the `PATH` default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<ExecutableRef>,
    /// Generated wrapper, when one was written at creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapper: Option<WrapperRef>,
    /// HOME-virtualized instance mode (run-5): the record still carries no
    /// model/provider/secret data; the provider reference lives under the root.
    #[serde(default)]
    pub home_virt: bool,
    /// When the alias was created (ISO8601 UTC).
    pub created_at: String,
    /// Version of the adapter that created the record.
    pub adapter_revision: String,
}

impl AliasRecord {
    /// Rebuild the adapter-facing [`Instance`] for this record. The id is
    /// re-derived deterministically, so planning is stable across processes.
    fn to_instance(&self) -> Result<Instance> {
        let id = derive_alias_instance_id(&self.harness, &self.name, &self.root)?;
        Ok(Instance {
            id,
            name: self.name.clone(),
            harness: self.harness.clone(),
            config_root: self.root.clone(),
            binary: self.binary.clone(),
            wrapper: self.wrapper.clone(),
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: self.created_at.clone(),
            adapter_revision: self.adapter_revision.clone(),
        })
    }
}

fn manifest_path(base_dir: &Path) -> PathBuf {
    base_dir.join(ALIAS_MANIFEST_FILE)
}

fn load_manifest(base_dir: &Path) -> Result<Vec<AliasRecord>> {
    let path = manifest_path(base_dir);
    let map = superai_config::json::load(&path).map_err(CoreError::Config)?;
    if let Some(version) = map.get(MANIFEST_SCHEMA_KEY) {
        let parsed = version.as_u64().and_then(|v| u32::try_from(v).ok());
        if parsed != Some(ALIAS_MANIFEST_SCHEMA_VERSION) {
            return Err(CoreError::SchemaValidation {
                path,
                details: format!(
                    "unsupported {MANIFEST_SCHEMA_KEY} {version}: expected \
                     {ALIAS_MANIFEST_SCHEMA_VERSION}"
                ),
            });
        }
    }
    let aliases = map
        .get(MANIFEST_ALIASES_KEY)
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let records: Vec<AliasRecord> = serde_json::from_value(aliases).map_err(CoreError::Records)?;
    Ok(records)
}

fn manifest_entry_matches(entry: &Value, harness: &HarnessId, name: &str) -> bool {
    let entry_harness = entry.get("harness").and_then(Value::as_str);
    let entry_name = entry.get("name").and_then(Value::as_str);
    entry_harness == Some(harness.as_str())
        && entry_name.is_some_and(|n| n.eq_ignore_ascii_case(name))
}

/// Insert a record into the manifest through the config mutation boundary
/// (foreign keys preserved; existing content backed up before replace).
fn insert_manifest_record(base_dir: &Path, record: &AliasRecord) -> Result<()> {
    let path = manifest_path(base_dir);
    let encoded = serde_json::to_value(record).map_err(CoreError::Records)?;
    superai_config::json::edit(&path, |map| {
        if !map.contains_key(MANIFEST_SCHEMA_KEY) {
            map.insert(
                MANIFEST_SCHEMA_KEY.to_owned(),
                Value::Number(serde_json::Number::from(ALIAS_MANIFEST_SCHEMA_VERSION)),
            );
        }
        let mut aliases: Vec<Value> = map
            .get(MANIFEST_ALIASES_KEY)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        aliases.push(encoded.clone());
        map.insert(MANIFEST_ALIASES_KEY.to_owned(), Value::Array(aliases));
    })
    .map_err(CoreError::Config)?;
    Ok(())
}

/// Remove an alias's entry from the manifest, leaving foreign keys and other
/// records untouched.
fn remove_manifest_record(base_dir: &Path, harness: &HarnessId, name: &str) -> Result<()> {
    let path = manifest_path(base_dir);
    superai_config::json::edit(&path, |map| {
        let kept: Vec<Value> = map
            .get(MANIFEST_ALIASES_KEY)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| !manifest_entry_matches(entry, harness, name))
            .collect();
        map.insert(MANIFEST_ALIASES_KEY.to_owned(), Value::Array(kept));
    })
    .map_err(CoreError::Config)?;
    Ok(())
}

/// The adapter's MCP declaration, refusing absence and read-only dests.
fn writable_mcp_decl(harness: &HarnessId, adapter: &dyn Adapter) -> Result<McpAdapterDecl> {
    let Some(decl) = adapter.mcp_decl() else {
        let reason = adapter
            .mcp_absence_reason()
            .unwrap_or("no MCP destination modeled for this harness");
        return Err(CoreError::UnsupportedOperation {
            harness: harness.to_string(),
            operation: "seed alias mcp set".to_owned(),
            reason: reason.to_owned(),
        });
    };
    if let Some(reason) = &decl.read_only {
        return Err(CoreError::UnsupportedOperation {
            harness: harness.to_string(),
            operation: "seed alias mcp set".to_owned(),
            reason: reason.clone(),
        });
    }
    Ok(decl)
}

/// The adapter's plugin declaration, refusing absent and execution-requiring
/// mechanisms (alias creation stages files; it never runs harness commands).
fn stageable_plugin_decl(harness: &HarnessId, adapter: &dyn Adapter) -> Result<PluginAdapterDecl> {
    let Some(decl) = adapter.plugin_decl() else {
        let reason = adapter
            .plugin_absence_reason()
            .unwrap_or("no plugin mechanism modeled for this harness");
        return Err(CoreError::UnsupportedOperation {
            harness: harness.to_string(),
            operation: "stage alias plugin set".to_owned(),
            reason: reason.to_owned(),
        });
    };
    if decl.kind != PluginKind::DirectoryBundle || decl.requires_execution {
        return Err(CoreError::UnsupportedOperation {
            harness: harness.to_string(),
            operation: "stage alias plugin set".to_owned(),
            reason: format!(
                "alias create stages directory bundles only; plugin kind `{}` requires \
                 external execution (EXT-06 approval path)",
                decl.kind
            ),
        });
    }
    Ok(decl)
}

/// Build the HOME-virtualization launch plan: `HOME=<alias-root>` plus
/// `XDG_CONFIG_HOME=<alias-root>/.config`; never sets `PATH`.
fn home_virt_wrapper_plan(root: &AbsolutePath) -> Result<WrapperPlan> {
    let config_root = root.join(".config")?;
    let mut plan = WrapperPlan::new(
        "HOME-virtualized instance (Electron appData and the codex ~/.codex default resolve under HOME)",
    );
    plan.env_vars.push(("HOME".to_owned(), root.to_string()));
    plan.env_vars
        .push(("XDG_CONFIG_HOME".to_owned(), config_root.to_string()));
    plan.state_paths = vec![
        format!("HOME={root}"),
        format!("XDG_CONFIG_HOME={config_root}"),
    ];
    plan.isolation_guarantees = vec![
        "app userData relocates to <root>/.config (Electron appData)".to_owned(),
        "the codex store relocates to <root>/.codex (CODEX_HOME default is HOME-relative)"
            .to_owned(),
    ];
    plan.shared_state_warnings = vec![
        "OS keychain credentials stay shared across every HOME-virtualized instance".to_owned(),
    ];
    Ok(plan)
}

/// Plan the alias launch through the adapter's own `plan_wrapper`, refusing
/// plans without relocation vars or that override `PATH`.
fn wrapper_plan_for_alias(
    adapter: &dyn Adapter,
    instance: &Instance,
    home_virt: bool,
) -> Result<WrapperPlan> {
    if adapter.id() != instance.harness {
        return Err(CoreError::UnsupportedHarness {
            harness: instance.harness.to_string(),
            reason: format!(
                "adapter `{}` cannot plan aliases for harness `{}`",
                adapter.id(),
                instance.harness
            ),
        });
    }
    let plan = if home_virt {
        if !HOME_VIRT_HARNESSES.contains(&instance.harness.as_str()) {
            return Err(CoreError::UnsupportedOperation {
                harness: instance.harness.to_string(),
                operation: "home_virt".to_owned(),
                reason: format!(
                    "HOME-virtualization is modeled only for {} (binaries demonstrably \
                     honoring HOME, run-4 live evidence); the relocation guard is not \
                     weakened for `{}`",
                    HOME_VIRT_HARNESSES.join(", "),
                    instance.harness
                ),
            });
        }
        home_virt_wrapper_plan(&instance.config_root)?
    } else {
        adapter.plan_wrapper(instance)?
    };
    if plan.env_vars.is_empty() {
        return Err(CoreError::Validation {
            field: "wrapper_plan.env_vars".to_owned(),
            reason: format!(
                "adapter `{}` declares no relocation env vars; an alias needs a relocated \
                 config root",
                instance.harness
            ),
        });
    }
    if plan.env_vars.iter().any(|(key, _)| key == "PATH") {
        return Err(CoreError::Validation {
            field: "wrapper_plan.env_vars".to_owned(),
            reason: "alias launch env must not override PATH".to_owned(),
        });
    }
    Ok(plan)
}

/// Create an alias: fresh root, seeded MCP/plugin sets, optional wrapper,
/// manifest record committed last; a seeding failure quarantines the root.
pub fn create_alias(
    base_dir: &Path,
    spec: &AliasSpec,
    adapter: &dyn Adapter,
    wrapper: Option<&WrapperPath>,
) -> Result<AliasRecord> {
    let root = alias_root(base_dir, &spec.harness, &spec.name)?;
    let existing = load_manifest(base_dir)?;
    for record in &existing {
        if record.harness == spec.harness && record.name.eq_case_fold(&spec.name) {
            return Err(CoreError::NameCollision {
                kind: "AliasName".to_owned(),
                name: spec.name.to_string(),
                reason: format!("case-fold collision with existing alias `{}`", record.name),
            });
        }
    }
    if root.as_path().exists() {
        return Err(CoreError::ForeignOwnership {
            path: root.as_path().to_path_buf(),
            owner: "alias root already exists on disk".to_owned(),
        });
    }
    if let Some(provider) = &spec.provider {
        provider.validate_for(&spec.harness, spec.home_virt)?;
    }
    let instance = AliasRecord {
        harness: spec.harness.clone(),
        name: spec.name.clone(),
        root: root.clone(),
        binary: spec.binary.clone(),
        wrapper: None,
        home_virt: spec.home_virt,
        created_at: crate::registry::now_iso8601(),
        adapter_revision: adapter.adapter_revision().to_owned(),
    }
    .to_instance()?;
    let plan = wrapper_plan_for_alias(adapter, &instance, spec.home_virt)?;

    let started_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    create_alias_root(&root, &spec.harness, &spec.name)?;
    // Wrapper generation runs LAST, so a seed failure leaves nothing at the
    // wrapper path and its rollback must not run at all.
    if let Err(e) = seed_mcp_set(
        &root,
        &spec.harness,
        adapter,
        &spec.mcp_servers,
        spec.home_virt,
    )
    .and_then(|()| seed_plugin_set(&root, &spec.harness, adapter, &spec.plugins))
    .and_then(|()| seed_provider_profile(&root, spec))
    {
        return Err(rollback_partial_alias(
            base_dir,
            root.as_path(),
            None,
            started_millis,
            &[],
            e,
        ));
    }
    let (wrapper_content, _) = crate::wrapper::generate_shell_wrapper(&instance, &plan)?;
    let wrapper_ref = match generate_alias_wrapper(&instance, &plan, wrapper) {
        Ok(wrapper_ref) => wrapper_ref,
        Err(e) => {
            return Err(rollback_partial_alias(
                base_dir,
                root.as_path(),
                wrapper,
                started_millis,
                wrapper_content.as_bytes(),
                e,
            ));
        }
    };

    let record = AliasRecord {
        harness: spec.harness.clone(),
        name: spec.name.clone(),
        root: root.clone(),
        binary: spec.binary.clone(),
        wrapper: wrapper_ref,
        home_virt: spec.home_virt,
        created_at: instance.created_at,
        adapter_revision: adapter.adapter_revision().to_owned(),
    };
    match insert_manifest_record(base_dir, &record) {
        Ok(()) => Ok(record),
        Err(e) => Err(rollback_partial_alias(
            base_dir,
            root.as_path(),
            wrapper,
            started_millis,
            wrapper_content.as_bytes(),
            e,
        )),
    }
}

/// Undo a partially created alias: root to quarantine, this run's wrapper
/// restored; the original error survives a clean rollback.
fn rollback_partial_alias(
    base_dir: &Path,
    root: &Path,
    wrapper: Option<&WrapperPath>,
    started_millis: u128,
    wrapper_content: &[u8],
    failure: CoreError,
) -> CoreError {
    let mut notes = Vec::new();
    let mut clean = true;
    match quarantine_alias_root(base_dir, root) {
        Ok(path) => notes.push(format!("alias root quarantined at {}", path.display())),
        Err(e) => {
            clean = false;
            notes.push(format!(
                "alias root {} could NOT be quarantined: {e}",
                root.display()
            ));
        }
    }
    if let Some(wrapper) = wrapper {
        match rollback_wrapper_file(wrapper.as_path(), wrapper_content, started_millis) {
            Ok(note) => notes.push(note),
            Err(note) => {
                clean = false;
                notes.push(note);
            }
        }
    }
    if clean {
        failure
    } else {
        CoreError::Commit {
            path: root.to_path_buf(),
            reason: format!(
                "alias creation failed ({failure}); rollback left state: {}",
                notes.join("; ")
            ),
        }
    }
}

/// Return the wrapper path to its pre-create state. Only bytes provably
/// this run's wrapper are touched; foreign launchers are left alone.
fn rollback_wrapper_file(
    path: &Path,
    wrapper_content: &[u8],
    started_millis: u128,
) -> std::result::Result<String, String> {
    match std::fs::read(path) {
        Ok(current) if current == wrapper_content => {}
        Ok(_) => {
            return Ok(format!(
                "wrapper {} is not this run's generation, left untouched",
                path.display()
            ));
        }
        Err(_) => return Ok(format!("wrapper {} already absent", path.display())),
    }
    // A backup from this run is the pre-create state write_wrapper saved.
    let ours = superai_config::backup::list_backups(path)
        .ok()
        .and_then(|mut all| {
            all.retain(|b| b.timestamp_millis >= started_millis);
            all.pop()
        });
    match ours {
        Some(entry) => superai_config::backup::restore(&entry.backup_path, path)
            .map(|()| {
                format!(
                    "wrapper {} restored to its pre-create bytes from {}",
                    path.display(),
                    entry.backup_path.display()
                )
            })
            .map_err(|e| {
                format!(
                    "wrapper {} is this run's generation and could NOT be restored from {}: {e}",
                    path.display(),
                    entry.backup_path.display()
                )
            }),
        None => std::fs::remove_file(path)
            .map(|()| format!("wrapper {} removed (nothing pre-existed)", path.display()))
            .map_err(|e| {
                format!(
                    "wrapper {} is this run's generation and could NOT be removed: {e}",
                    path.display()
                )
            }),
    }
}

/// Create the alias root directory plus the ownership marker via a
/// compensated transaction (no raw `mkdir`/file writes).
fn create_alias_root(root: &AbsolutePath, harness: &HarnessId, name: &InstanceName) -> Result<()> {
    let marker = format!("{}\n{}\n", harness.as_str(), name.as_str());
    let steps = vec![
        FileAction::CreateDir {
            path: root.as_path().to_path_buf(),
        },
        FileAction::Write {
            path: root.as_path().join(ALIAS_MARKER_FILE),
            content: marker.into_bytes(),
            kind: superai_config::document::DocumentKind::TextFragment,
        },
    ];
    let operation = transaction_operation_id("alias-create")?;
    let mut transaction = Transaction::new(operation, steps);
    let outcome = transaction.execute().map_err(CoreError::Config)?;
    if !outcome.success {
        return Err(CoreError::Commit {
            path: root.as_path().to_path_buf(),
            reason: outcome.diagnostics_redacted.join("; "),
        });
    }
    Ok(())
}

/// The MCP destination relative to the alias root: under HOME-virt the file
/// must land where the HOME-relocated binary reads it.
fn alias_mcp_dest(harness: &HarnessId, decl_dest: &str, home_virt: bool) -> String {
    if home_virt
        && let Some((_, overridden)) = HOME_VIRT_MCP_DESTS
            .iter()
            .find(|(id, _)| *id == harness.as_str())
    {
        return (*overridden).to_owned();
    }
    decl_dest.to_owned()
}

/// Seed the MCP set through the existing `mcp` write path at the
/// adapter-declared destination under the alias root.
fn seed_mcp_set(
    root: &AbsolutePath,
    harness: &HarnessId,
    adapter: &dyn Adapter,
    servers: &[McpServerDef],
    home_virt: bool,
) -> Result<()> {
    if servers.is_empty() {
        return Ok(());
    }
    let decl = writable_mcp_decl(harness, adapter)?;
    let dest = root.join(&alias_mcp_dest(harness, &decl.dest_file, home_virt))?;
    for server in servers {
        mcp::install_mcp_server(dest.as_path(), &decl, server)?;
    }
    Ok(())
}

/// Stage the plugin set through the existing `plugin` machinery, with the
/// plugin registry colocated under the alias root.
fn seed_plugin_set(
    root: &AbsolutePath,
    harness: &HarnessId,
    adapter: &dyn Adapter,
    sources: &[PluginSource],
) -> Result<()> {
    if sources.is_empty() {
        return Ok(());
    }
    let decl = stageable_plugin_decl(harness, adapter)?;
    let registry_root = root.join(&format!("{ALIAS_STATE_DIR}/plugins"))?;
    let mut registry = plugin::PluginRegistry::load(registry_root.as_path())?;
    for source in sources {
        plugin::install_directory_bundle(&mut registry, source, &decl, root.as_path())?;
    }
    Ok(())
}

/// Seed the alias's third-party provider: env-carried composes at launch,
/// config-carried seeds the codex `CODEX_HOME` table; the reference carries names only.
fn seed_provider_profile(root: &AbsolutePath, spec: &AliasSpec) -> Result<()> {
    let Some(profile) = &spec.provider else {
        return Ok(());
    };
    profile.validate_for(&spec.harness, spec.home_virt)?;
    match provider_carriage(&spec.harness)? {
        ProviderCarriage::EnvCarried => {}
        ProviderCarriage::ConfigCarried => {
            let codex_home = if spec.harness.as_str() == "codex-cli" {
                // The codex-cli alias root IS the CODEX_HOME (plan env).
                root.clone()
            } else {
                // chatgpt-desktop HOME-virt: CODEX_HOME default resolves
                // under the virtualized HOME.
                root.join(".codex")?
            };
            let codex_harness = HarnessId::new("codex-cli").map_err(|e| CoreError::Validation {
                field: "harness".to_owned(),
                reason: format!("cannot build codex redirect harness id: {e}"),
            })?;
            let instance = Instance {
                id: derive_alias_instance_id(&codex_harness, &spec.name, &codex_home)?,
                name: spec.name.clone(),
                harness: codex_harness,
                config_root: codex_home,
                binary: None,
                wrapper: None,
                isolation: Isolation::RelocatedRoot,
                origin: InstanceOrigin::Created,
                ownership: Ownership::SuperaiCreated,
                template: None,
                created_at: crate::registry::now_iso8601(),
                adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
            };
            let codex_adapter = crate::adapters::codex_cli::CodexCliAdapter::new()?;
            let definition = profile.to_definition();
            commit_provider_change(
                &instance,
                &codex_adapter,
                &ProviderChange::AddOrUpdate {
                    provider: &definition,
                },
                &ProviderChangeOptions::default(),
            )?;
        }
    }
    store_provider_ref(root, profile)
}

/// Compose the provider's launch env from the persisted reference and the
/// caller's secrets; a missing secret warns (both families validate lazily).
fn compose_provider_env(
    harness: &HarnessId,
    root: &AbsolutePath,
    provider_secrets: &[(&str, &str)],
    warnings: &mut Vec<String>,
) -> Result<(Vec<(String, String)>, Vec<(String, RedactedString)>)> {
    let Some(reference) = load_provider_ref(root)? else {
        return Ok((Vec::new(), Vec::new()));
    };
    let secret = provider_secrets
        .iter()
        .find(|(name, _)| *name == reference.auth_env_var)
        .map(|(_, value)| (*value).to_owned());
    let secret_entries = if let Some(value) = secret {
        vec![(reference.auth_env_var.clone(), RedactedString::new(&value))]
    } else {
        warnings.push(format!(
            "provider `{}` auth env var `{}` not supplied at composition; the \
             harness validates it lazily at first use",
            reference.provider_id, reference.auth_env_var
        ));
        Vec::new()
    };
    let overlay = match provider_carriage(harness)? {
        ProviderCarriage::EnvCarried => {
            let mut vars = vec![(
                CLAUDE_CODE_BASE_URL_VAR.to_owned(),
                reference.base_url.clone(),
            )];
            if let Some(model) = &reference.model {
                vars.push((CLAUDE_CODE_MODEL_VAR.to_owned(), model.clone()));
            }
            if let Some(small) = &reference.small_model {
                vars.push((CLAUDE_CODE_HAIKU_VAR.to_owned(), small.clone()));
            }
            vars
        }
        ProviderCarriage::ConfigCarried => Vec::new(),
    };
    Ok((overlay, secret_entries))
}

/// Overlay env entries onto a base set: overlay values WIN on key conflict
/// (explicit provider-vs-plan precedence), new keys are appended in order.
fn overlay_env(
    mut env: Vec<(String, String)>,
    overlay: Vec<(String, String)>,
) -> Vec<(String, String)> {
    for (key, value) in overlay {
        match env.iter_mut().find(|(existing, _)| *existing == key) {
            Some(slot) => slot.1 = value,
            None => env.push((key, value)),
        }
    }
    env
}

/// Guard the composed environment: PATH is never set.
fn refuse_path_override(env: &[(String, String)]) -> Result<()> {
    if env.iter().any(|(key, _)| key == "PATH") {
        return Err(CoreError::Validation {
            field: "alias_env".to_owned(),
            reason: "alias launch env must not override PATH".to_owned(),
        });
    }
    Ok(())
}

/// Generate and write the alias wrapper through the existing generator and
/// write path (foreign-ownership refusal included).
fn generate_alias_wrapper(
    instance: &Instance,
    plan: &WrapperPlan,
    wrapper: Option<&WrapperPath>,
) -> Result<Option<WrapperRef>> {
    let Some(path) = wrapper else {
        return Ok(None);
    };
    let (content, digest) = wrapper_helper::generate_shell_wrapper(instance, plan)?;
    wrapper_helper::write_wrapper(path, &content)?;
    Ok(Some(WrapperRef {
        path: path.clone(),
        command_name: instance.name.clone(),
        generator_version: wrapper_helper::GENERATOR_VERSION.to_owned(),
        content_digest: digest,
    }))
}

/// Quarantine a half-created alias root. Recovery state stays under the
/// alias base (`<base>/.superai/quarantine/...`), never the user's home.
fn quarantine_alias_root(base_dir: &Path, root: &Path) -> std::result::Result<PathBuf, CoreError> {
    let op = crate::registry::unique_operation_string("alias-failure");
    superai_config::quarantine::move_to_quarantine_under(base_dir, root, &op)
        .map(|entry| entry.quarantine_path)
        .map_err(CoreError::Config)
}

/// List every recorded alias, read fresh from the on-disk manifest.
pub fn list_aliases(base_dir: &Path) -> Result<Vec<AliasRecord>> {
    load_manifest(base_dir)
}

/// Look up one alias by harness and name (case-folded), read fresh.
pub fn get_alias(base_dir: &Path, harness: &HarnessId, name: &str) -> Result<AliasRecord> {
    let records = load_manifest(base_dir)?;
    records
        .into_iter()
        .find(|record| record.harness == *harness && record.name.eq_case_fold_str(name))
        .ok_or_else(|| CoreError::Validation {
            field: "alias".to_owned(),
            reason: format!("alias `{harness}@{name}` not found"),
        })
}

/// The composed launch environment for an alias: relocation vars at the
/// root plus provider overlay. `PATH` is never part of it; values are raw, do not log.
pub fn alias_env(
    base_dir: &Path,
    harness: &HarnessId,
    name: &str,
    adapter: &dyn Adapter,
    provider_secrets: &[(&str, &str)],
) -> Result<Vec<(String, String)>> {
    let record = get_alias(base_dir, harness, name)?;
    let instance = record.to_instance()?;
    let plan = wrapper_plan_for_alias(adapter, &instance, record.home_virt)?;
    // Warnings are surfaced by `launch_composition`; this export list is
    // complete on its own (a missing token simply exports nothing).
    let (overlay, secrets) =
        compose_provider_env(harness, &record.root, provider_secrets, &mut Vec::new())?;
    let mut env = overlay_env(plan.env_vars, overlay);
    for (name, secret) in secrets {
        env.push((name, secret.expose_secret().to_owned()));
    }
    refuse_path_override(&env)?;
    Ok(env)
}

/// A runnable command for an alias: direct `argv` + env for in-process
/// exec, or the deterministic `sh` script text for an on-disk launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchComposition {
    /// argv to exec: resolved binary, the plan's fixed args, then
    /// `extra_args`.
    pub argv: Vec<String>,
    /// Environment variables to set (relocation vars plus non-secret
    /// provider overlay). `PATH` is never here.
    pub env: Vec<(String, String)>,
    /// Provider auth entries (token under the recorded env var name); the
    /// values are redacted in Debug, apply them to the child like `env`.
    pub secret_env: Vec<(String, RedactedString)>,
    /// Environment variables to unset before exec (WRP-02 leak guard).
    pub env_unset: Vec<String>,
    /// Fixed working directory, when the adapter declares one.
    pub working_dir: Option<String>,
    /// Non-blocking composition notes (e.g. a provider token not supplied,
    /// the harness validates it lazily at first use).
    pub warnings: Vec<String>,
    /// Deterministic `#!/bin/sh` launcher script carrying the PLAN's env only:
    /// launcher files stay secret-free; provider vars ride direct exec.
    pub script: String,
}

/// Compose how an alias launches: executable/args/env resolved against the
/// root (plan, then pinned binary, then PATH default) plus the provider overlay.
pub fn launch_composition(
    base_dir: &Path,
    harness: &HarnessId,
    name: &str,
    adapter: &dyn Adapter,
    extra_args: &[String],
    provider_secrets: &[(&str, &str)],
) -> Result<LaunchComposition> {
    let record = get_alias(base_dir, harness, name)?;
    let instance = record.to_instance()?;
    let plan = wrapper_plan_for_alias(adapter, &instance, record.home_virt)?;
    let executable = plan
        .executable
        .clone()
        .or_else(|| instance.binary.as_ref().map(ToString::to_string))
        .unwrap_or_else(|| wrapper_helper::executable_for_harness(&instance.harness));
    let mut argv = Vec::with_capacity(1 + plan.args.len() + extra_args.len());
    argv.push(executable);
    argv.extend(plan.args.iter().cloned());
    argv.extend(extra_args.iter().cloned());
    let mut warnings = Vec::new();
    let (overlay, secret_env) =
        compose_provider_env(harness, &record.root, provider_secrets, &mut warnings)?;
    let env = overlay_env(plan.env_vars.clone(), overlay);
    refuse_path_override(&env)?;
    let (script, _digest) = wrapper_helper::generate_shell_wrapper(&instance, &plan)?;
    Ok(LaunchComposition {
        argv,
        env,
        secret_env,
        env_unset: plan.env_unset,
        working_dir: plan.working_dir,
        warnings,
        script,
    })
}

/// Whether `path` is `base` itself or nested under it (component-wise).
fn path_is_under(path: &Path, base: &Path) -> bool {
    path.starts_with(base)
}

/// Verify the alias marker names this harness and alias (case-folded on the
/// name). A missing or mismatched marker means the root must not be touched.
fn verify_alias_marker(root: &Path, harness: &HarnessId, name: &str) -> Result<()> {
    let marker_path = root.join(ALIAS_MARKER_FILE);
    let text = std::fs::read_to_string(&marker_path).map_err(|e| CoreError::ForeignOwnership {
        path: marker_path.clone(),
        owner: format!("alias marker unreadable ({e}); not a managed alias root"),
    })?;
    let mut lines = text.lines();
    let marker_harness = lines.next();
    let marker_name = lines.next();
    match (marker_harness, marker_name) {
        (Some(h), Some(n)) if h == harness.as_str() && n.eq_ignore_ascii_case(name) => Ok(()),
        _ => Err(CoreError::ForeignOwnership {
            path: marker_path,
            owner: "alias marker does not name this alias".to_owned(),
        }),
    }
}

/// Remove an alias: superai-owned wrapper, root to quarantine (never a
/// blind delete), manifest entry; refuses outside-base roots or missing marker.
pub fn remove_alias(base_dir: &Path, harness: &HarnessId, name: &str) -> Result<AliasRecord> {
    let base = AbsolutePath::from_path(base_dir)?;
    let record = get_alias(base_dir, harness, name)?;
    if !path_is_under(record.root.as_path(), base.as_path()) {
        return Err(CoreError::ForeignOwnership {
            path: record.root.as_path().to_path_buf(),
            owner: "alias root outside the superai alias base".to_owned(),
        });
    }
    verify_alias_marker(record.root.as_path(), harness, name)?;
    if let Some(wrapper) = &record.wrapper {
        // Verified rename-shuffle removal: a swap between verify and delete
        // is refused instead of deleting foreign bytes.
        wrapper_helper::remove_owned_wrapper_verified(
            wrapper.path.as_path(),
            Some(&wrapper.content_digest),
        )?;
    }
    if record.root.as_path().exists() {
        // Recovery state stays under the alias base, never the user's home.
        let op = crate::registry::unique_operation_string("alias-remove");
        superai_config::quarantine::move_to_quarantine_under(
            base.as_path(),
            record.root.as_path(),
            &op,
        )
        .map_err(CoreError::Config)?;
    }
    remove_manifest_record(base_dir, harness, name)?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::McpServerId;
    use crate::ids::PluginId;

    fn base(tag: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(&format!("alias-{tag}"))
    }

    fn adapter(harness: &str) -> Box<dyn Adapter> {
        crate::harness_catalog::concrete_adapter_for(harness)
            .unwrap_or_else(|| panic!("no concrete adapter for {harness}"))
    }

    fn harness(harness: &str) -> HarnessId {
        HarnessId::new(harness).unwrap()
    }

    fn name(name: &str) -> InstanceName {
        InstanceName::new(name).unwrap()
    }

    fn server(id: &str) -> McpServerDef {
        McpServerDef::stdio(
            McpServerId::new(id).unwrap(),
            "node",
            vec!["s.js".to_owned()],
        )
        .unwrap()
    }

    #[test]
    fn create_yields_disjoint_roots_per_harness_and_name() {
        let base = base("disjoint");
        let workbuddy = adapter("workbuddy");
        let qwen = adapter("qwen-code");

        let a = create_alias(
            &base,
            &AliasSpec::new(harness("workbuddy"), name("team-a"))
                .with_mcp_servers(vec![server("alpha")]),
            workbuddy.as_ref(),
            None,
        )
        .unwrap();
        let b = create_alias(
            &base,
            &AliasSpec::new(harness("workbuddy"), name("team-b"))
                .with_mcp_servers(vec![server("beta")]),
            workbuddy.as_ref(),
            None,
        )
        .unwrap();
        let c = create_alias(
            &base,
            &AliasSpec::new(harness("qwen-code"), name("team-a")),
            qwen.as_ref(),
            None,
        )
        .unwrap();

        assert_ne!(
            a.root, b.root,
            "same harness, different names must be disjoint"
        );
        assert_ne!(
            a.root, c.root,
            "same name, different harnesses must be disjoint"
        );
        assert!(
            a.root.as_path().starts_with(&base),
            "roots live under the base"
        );
        assert!(a.root.as_path().is_dir());
        assert_eq!(list_aliases(&base).unwrap().len(), 3);
    }

    #[test]
    fn mcp_set_lands_in_adapter_declared_dest_per_adapter() {
        for (harness_id, expected_dest) in [
            ("workbuddy", ".mcp.json"),
            ("qwen-code", "settings.json"),
            ("grok-build", "config.toml"),
            ("factory-droid", ".factory/mcp.json"),
        ] {
            let base = base(&format!("mcp-{harness_id}"));
            let adapter = adapter(harness_id);
            let spec = AliasSpec::new(harness(harness_id), name("seeded"))
                .with_mcp_servers(vec![server("alpha"), server("beta")]);
            let record = create_alias(&base, &spec, adapter.as_ref(), None).unwrap();

            let decl = adapter.mcp_decl().unwrap();
            assert_eq!(decl.dest_file, expected_dest, "{harness_id} dest file");
            let dest = record.root.join(&decl.dest_file).unwrap();
            assert!(dest.is_file(), "{harness_id} seeded dest missing at {dest}");
            let effective = mcp::inspect_servers(dest.as_path(), &decl).unwrap();
            assert_eq!(
                effective.servers.len(),
                2,
                "{harness_id}: both seeded servers must round-trip"
            );
            let alpha = McpServerId::new("alpha").unwrap();
            let seeded_alpha = effective
                .servers
                .get(&alpha)
                .unwrap_or_else(|| panic!("{harness_id}: seeded server alpha missing"));
            assert_eq!(seeded_alpha.command.as_deref(), Some("node"));
        }
    }

    #[test]
    fn different_aliases_carry_different_mcp_sets() {
        let base = base("sets");
        let workbuddy = adapter("workbuddy");
        let decl = workbuddy.mcp_decl().unwrap();
        let a = create_alias(
            &base,
            &AliasSpec::new(harness("workbuddy"), name("a"))
                .with_mcp_servers(vec![server("only-a")]),
            workbuddy.as_ref(),
            None,
        )
        .unwrap();
        create_alias(
            &base,
            &AliasSpec::new(harness("workbuddy"), name("b"))
                .with_mcp_servers(vec![server("only-b")]),
            workbuddy.as_ref(),
            None,
        )
        .unwrap();

        let set_a =
            mcp::inspect_servers(a.root.join(&decl.dest_file).unwrap().as_path(), &decl).unwrap();
        assert!(set_a.contains_key(&McpServerId::new("only-a").unwrap()));
        assert!(
            !set_a.contains_key(&McpServerId::new("only-b").unwrap()),
            "alias A must not see alias B's servers"
        );
    }

    #[test]
    fn read_only_mcp_dest_refuses_seed_and_leaves_no_trace() {
        let base = base("readonly");
        let amp = adapter("amp");
        let refusal = amp.mcp_decl().unwrap().read_only.unwrap();
        let spec = AliasSpec::new(harness("amp"), name("ro")).with_mcp_servers(vec![server("x")]);

        let err = create_alias(&base, &spec, amp.as_ref(), None).unwrap_err();
        match &err {
            CoreError::UnsupportedOperation { reason, .. } => assert_eq!(reason, &refusal),
            other => panic!("expected read-only refusal, got {other:?}"),
        }
        assert!(
            list_aliases(&base).unwrap().is_empty(),
            "refused alias must not be recorded"
        );
        assert!(
            !base.join("amp").join("ro").exists(),
            "refused alias root must be cleaned up"
        );
    }

    #[test]
    fn plugin_seeds_through_declared_directory_bundle() {
        let base = base("plugin");
        let bundle = base.join("bundle-src");
        std::fs::create_dir_all(bundle.join("skills")).unwrap();
        std::fs::write(bundle.join("skills").join("s.md"), b"# skill\n").unwrap();
        let grok = adapter("grok-build");
        let source = PluginSource {
            id: PluginId::new("my-plugin").unwrap(),
            kind: PluginKind::DirectoryBundle,
            locator: bundle.display().to_string(),
            version: None,
            digest: None,
        };
        let record = create_alias(
            &base,
            &AliasSpec::new(harness("grok-build"), name("plugged")).with_plugins(vec![source]),
            grok.as_ref(),
            None,
        )
        .unwrap();

        assert!(
            record
                .root
                .join("plugins/my-plugin/skills/s.md")
                .unwrap()
                .is_file(),
            "bundle must stage into the adapter-declared plugins dir"
        );
        assert!(
            record
                .root
                .join(".superai/plugins/registry.json")
                .unwrap()
                .is_file(),
            "per-alias plugin registry must live under the alias root"
        );
    }

    #[test]
    fn execution_requiring_plugin_mechanism_refuses() {
        let base = base("plugin-refuse");
        let kimi = adapter("kimi-code-cli");
        let source = PluginSource {
            id: PluginId::new("market").unwrap(),
            kind: PluginKind::MarketplaceRecord,
            locator: "marketplace://example".to_owned(),
            version: None,
            digest: None,
        };
        let spec = AliasSpec::new(harness("kimi-code-cli"), name("mk")).with_plugins(vec![source]);
        let err = create_alias(&base, &spec, kimi.as_ref(), None).unwrap_err();
        match err {
            CoreError::UnsupportedOperation { reason, .. } => {
                assert!(reason.contains("external execution"), "got {reason}");
            }
            other => panic!("expected honest plugin refusal, got {other:?}"),
        }
        assert!(!base.join("kimi-code-cli").join("mk").exists());
        // The half-created root is recoverable UNDER THE ALIAS BASE, not in
        // the user's home (run-4 round-3 finding 2).
        let qbase = base.join(".superai").join("quarantine");
        let recovered: Vec<std::ffi::OsString> = std::fs::read_dir(&qbase)
            .map(|rd| {
                rd.filter_map(std::result::Result::ok)
                    .filter(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with("alias-failure-")
                    })
                    .map(|e| e.file_name())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            recovered.len(),
            1,
            "exactly one alias-failure quarantine op under the base"
        );
        assert!(qbase.join(&recovered[0]).join("mk").is_dir());
    }

    #[test]
    fn name_collision_and_foreign_root_are_refused() {
        let base = base("collide");
        let workbuddy = adapter("workbuddy");
        let spec = AliasSpec::new(harness("workbuddy"), name("team"));
        create_alias(&base, &spec, workbuddy.as_ref(), None).unwrap();

        let folded = AliasSpec::new(harness("workbuddy"), name("TEAM"));
        match create_alias(&base, &folded, workbuddy.as_ref(), None).unwrap_err() {
            CoreError::NameCollision { kind, .. } => assert_eq!(kind, "AliasName"),
            other => panic!("expected NameCollision, got {other:?}"),
        }
        // Pre-existing directory at the root: foreign, never overwritten.
        std::fs::create_dir_all(base.join("workbuddy").join("fresh")).unwrap();
        let occupied = AliasSpec::new(harness("workbuddy"), name("fresh"));
        match create_alias(&base, &occupied, workbuddy.as_ref(), None).unwrap_err() {
            CoreError::ForeignOwnership { .. } => {}
            other => panic!("expected ForeignOwnership, got {other:?}"),
        }
    }

    #[test]
    fn mismatched_adapter_harness_is_refused() {
        let base = base("mismatch");
        let workbuddy = adapter("workbuddy");
        let spec = AliasSpec::new(harness("no-such-harness"), name("ghost"));
        match create_alias(&base, &spec, workbuddy.as_ref(), None).unwrap_err() {
            CoreError::UnsupportedHarness { harness, .. } => assert_eq!(harness, "no-such-harness"),
            other => panic!("expected UnsupportedHarness, got {other:?}"),
        }
    }

    #[test]
    fn remove_cleans_wrapper_root_and_manifest() {
        let base = base("remove");
        let wrapper_dir = base.join("bin");
        std::fs::create_dir_all(&wrapper_dir).unwrap();
        let wrapper_path =
            WrapperPath::new(&wrapper_dir.join("team").display().to_string()).unwrap();
        let workbuddy = adapter("workbuddy");
        let record = create_alias(
            &base,
            &AliasSpec::new(harness("workbuddy"), name("team")),
            workbuddy.as_ref(),
            Some(&wrapper_path),
        )
        .unwrap();

        assert!(wrapper_path.as_path().is_file());
        let root = record.root;
        let removed = remove_alias(&base, &harness("workbuddy"), "team").unwrap();
        assert_eq!(removed.name.as_str(), "team");
        assert!(!root.as_path().exists(), "root must be gone (quarantined)");
        assert!(!wrapper_path.as_path().exists(), "wrapper must be removed");
        assert!(list_aliases(&base).unwrap().is_empty());

        match remove_alias(&base, &harness("workbuddy"), "team") {
            Err(CoreError::Validation { field, .. }) => assert_eq!(field, "alias"),
            other => panic!("expected not-found validation, got {other:?}"),
        }
    }

    /// Alias quarantine ops recorded under the REAL home (only alias.rs
    /// mints `alias-*` operation ids; none target the home base).
    fn home_quarantine_alias_ops() -> Vec<String> {
        let Some(home) = std::env::var_os("HOME") else {
            return Vec::new();
        };
        let dir = PathBuf::from(home).join(".superai").join("quarantine");
        std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(std::result::Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.starts_with("alias-"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Quarantine for alias removal lives under the alias base
    /// (`<base>/.superai/quarantine/...`), never the user's real home.
    #[test]
    fn remove_quarantines_under_the_alias_base_not_the_user_home() {
        let base = base("quar-under");
        let workbuddy = adapter("workbuddy");
        let record = create_alias(
            &base,
            &AliasSpec::new(harness("workbuddy"), name("gone"))
                .with_mcp_servers(vec![server("only-a")]),
            workbuddy.as_ref(),
            None,
        )
        .unwrap();
        let root = record.root;
        let home_ops_before = home_quarantine_alias_ops();

        remove_alias(&base, &harness("workbuddy"), "gone").unwrap();

        assert!(!root.as_path().exists(), "root must be gone");
        let qbase = base.join(".superai").join("quarantine");
        let ops: Vec<String> = std::fs::read_dir(&qbase)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("alias-remove-"))
            .collect();
        assert_eq!(ops.len(), 1, "one alias-remove op under the base");
        let moved = qbase.join(&ops[0]).join("gone");
        assert!(
            moved.join(ALIAS_MARKER_FILE).is_file(),
            "the moved root stays recoverable under the alias base"
        );
        assert!(
            moved.join(".mcp.json").is_file(),
            "seeded content moved with the root"
        );
        assert_eq!(
            home_quarantine_alias_ops(),
            home_ops_before,
            "no alias quarantine entry may appear under the real home"
        );
    }

    #[test]
    fn wrapper_rollback_returns_only_this_runs_bytes_to_pre_create_state() {
        let dir = crate::test_util::temp_dir_unique("alias-wrapper-rollback");
        let path = dir.join("cli");
        let generated = b"#!/bin/sh\n# superai wrapper generated\nexec tool\n";
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());

        // Foreign bytes at the path: left untouched.
        std::fs::write(&path, b"foreign launcher").unwrap();
        let note = rollback_wrapper_file(&path, generated, started).unwrap();
        assert!(note.contains("left untouched"), "{note}");
        assert_eq!(std::fs::read(&path).unwrap(), b"foreign launcher".to_vec());

        // This run's wrapper with nothing pre-existing: removed.
        std::fs::write(&path, generated).unwrap();
        let note = rollback_wrapper_file(&path, generated, started).unwrap();
        assert!(note.contains("removed"), "{note}");
        assert!(!path.exists(), "a wrapper nobody owned must be cleaned up");

        // This run's wrapper over a backed-up pre-state: the pre-create
        // bytes come back.
        std::fs::write(&path, b"pre-create launcher").unwrap();
        superai_config::backup::backup(&path).unwrap();
        std::fs::write(&path, generated).unwrap();
        let note = rollback_wrapper_file(&path, generated, started).unwrap();
        assert!(note.contains("restored"), "{note}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"pre-create launcher".to_vec()
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    /// A wrapper-stage failure quarantines the root and leaves nothing at
    /// the wrapper path; the error is the original one (rollback was clean).
    #[test]
    #[cfg(unix)]
    fn wrapper_write_failure_rolls_the_alias_back_cleanly() {
        use std::os::unix::fs::PermissionsExt;
        let base = base("rollback-wrapper");
        let workbuddy = adapter("workbuddy");
        let wrapper_dir = base.join("bin");
        std::fs::create_dir_all(&wrapper_dir).unwrap();
        std::fs::set_permissions(&wrapper_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let wrapper = WrapperPath::from_path(&wrapper_dir.join("cli")).unwrap();

        let err = create_alias(
            &base,
            &AliasSpec::new(harness("workbuddy"), name("seedfail")),
            workbuddy.as_ref(),
            Some(&wrapper),
        )
        .unwrap_err();
        assert!(
            format!("{err}").to_lowercase().contains("permission"),
            "expected the underlying write failure, got: {err}"
        );
        assert!(
            !wrapper.as_path().exists(),
            "nothing may linger at the wrapper path"
        );
        let root = base.join("workbuddy").join("seedfail");
        assert!(
            !root.exists(),
            "the partially seeded root must be quarantined"
        );
        assert!(
            base.join(".superai").join("quarantine").exists(),
            "recovery state must exist under the alias base"
        );
        assert_eq!(list_aliases(&base).unwrap().len(), 0);
        std::fs::set_permissions(&wrapper_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        drop(std::fs::remove_dir_all(&base));
    }

    /// A manifest-insert failure after a full seed must not vanish silently:
    /// the wrapper is removed and the error records where the root sits.
    #[test]
    #[cfg(unix)]
    fn manifest_insert_failure_records_leftover_state() {
        use std::os::unix::fs::PermissionsExt;
        let base = base("rollback-manifest");
        // The harness dir is pre-created so root creation survives the base
        // becoming read-only; only the manifest write must fail.
        std::fs::create_dir_all(base.join("workbuddy")).unwrap();
        let wrapper_dir = base.join("bin");
        std::fs::create_dir_all(&wrapper_dir).unwrap();
        let wrapper = WrapperPath::from_path(&wrapper_dir.join("cli")).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o555)).unwrap();

        let err = create_alias(
            &base,
            &AliasSpec::new(harness("workbuddy"), name("stuck")),
            adapter("workbuddy").as_ref(),
            Some(&wrapper),
        )
        .unwrap_err();
        let text = format!("{err}");
        assert!(
            text.contains("could NOT be quarantined"),
            "the rollback state must be recorded in the error: {text}"
        );
        assert!(
            text.contains("workbuddy") && text.contains("stuck"),
            "the leftover root must be named: {text}"
        );
        assert!(
            !wrapper.as_path().exists(),
            "this run's wrapper must be removed even when the root cannot move"
        );
        assert!(
            base.join("workbuddy").join("stuck").exists(),
            "the root itself stays put (read-only base)"
        );
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
        drop(std::fs::remove_dir_all(&base));
    }

    #[test]
    fn remove_refuses_roots_outside_base_or_without_marker() {
        let base = base("refuse-remove");
        let outside = crate::test_util::temp_dir_unique("alias-outside");
        std::fs::create_dir_all(&outside).unwrap();
        let harness_id = harness("workbuddy");
        let manifest = serde_json::json!({
            "schema_version": 1,
            "aliases": [
                {"harness": "workbuddy", "name": "outside",
                 "root": outside.display().to_string(),
                 "created_at": "2026-09-18T00:00:00Z", "adapter_revision": "0.1.0"},
                {"harness": "workbuddy", "name": "nomarker",
                 "root": base.join("workbuddy").join("nomarker").display().to_string(),
                 "created_at": "2026-09-18T00:00:00Z", "adapter_revision": "0.1.0"}
            ]
        });
        std::fs::create_dir_all(base.join("workbuddy").join("nomarker")).unwrap();
        std::fs::write(
            base.join(ALIAS_MANIFEST_FILE),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        match remove_alias(&base, &harness_id, "outside").unwrap_err() {
            CoreError::ForeignOwnership { .. } => {}
            other => panic!("expected managed-root refusal, got {other:?}"),
        }
        match remove_alias(&base, &harness_id, "nomarker").unwrap_err() {
            CoreError::ForeignOwnership { .. } => {}
            other => panic!("expected marker refusal, got {other:?}"),
        }
        assert!(outside.exists(), "outside dir must be untouched");
        assert!(
            base.join("workbuddy").join("nomarker").exists(),
            "unmarked dir must be untouched"
        );
        assert_eq!(list_aliases(&base).unwrap().len(), 2, "manifest untouched");
    }

    #[test]
    fn manifest_round_trips_and_preserves_foreign_keys() {
        let base = base("manifest");
        let workbuddy = adapter("workbuddy");
        create_alias(
            &base,
            &AliasSpec::new(harness("workbuddy"), name("one")),
            workbuddy.as_ref(),
            None,
        )
        .unwrap();
        superai_config::json::edit(&base.join(ALIAS_MANIFEST_FILE), |map| {
            map.insert(
                "foreign_note".to_owned(),
                Value::String("keep-me".to_owned()),
            );
        })
        .unwrap();

        create_alias(
            &base,
            &AliasSpec::new(harness("workbuddy"), name("two")),
            workbuddy.as_ref(),
            None,
        )
        .unwrap();
        let raw = superai_config::json::load(&base.join(ALIAS_MANIFEST_FILE)).unwrap();
        assert_eq!(
            raw.get("foreign_note"),
            Some(&Value::String("keep-me".to_owned())),
            "foreign manifest keys survive later creates"
        );
        assert_eq!(
            raw.get("schema_version").and_then(Value::as_u64),
            Some(u64::from(ALIAS_MANIFEST_SCHEMA_VERSION))
        );
        let listed = list_aliases(&base).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|r| r.name.as_str() == "two"));

        remove_alias(&base, &harness("workbuddy"), "one").unwrap();
        let after = list_aliases(&base).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after.first().unwrap().name.as_str(), "two");
        let raw_after = superai_config::json::load(&base.join(ALIAS_MANIFEST_FILE)).unwrap();
        assert_eq!(
            raw_after.get("foreign_note"),
            Some(&Value::String("keep-me".to_owned()))
        );
    }

    #[test]
    fn alias_env_composes_relocation_vars_and_never_path() {
        let cases = [
            ("workbuddy", "CODEBUDDY_CONFIG_DIR"),
            ("qwen-code", "QWEN_HOME"),
            ("grok-build", "GROK_HOME"),
        ];
        for (harness_id, env_var) in cases {
            let base = base(&format!("env-{harness_id}"));
            let adapter = adapter(harness_id);
            let record = create_alias(
                &base,
                &AliasSpec::new(harness(harness_id), name("envy")),
                adapter.as_ref(),
                None,
            )
            .unwrap();

            let env =
                alias_env(&base, &harness(harness_id), "envy", adapter.as_ref(), &[]).unwrap();
            assert!(
                env.iter()
                    .any(|(k, v)| k == env_var && v == &record.root.to_string()),
                "{harness_id}: {env_var} must point at the alias root, got {env:?}"
            );
            assert!(
                !env.iter().any(|(k, _)| k == "PATH"),
                "{harness_id}: alias env must never override PATH"
            );
        }
    }

    #[test]
    fn launch_composition_targets_the_pinned_binary_and_relocates() {
        // qwen-code's plan names no executable, so the pinned binary wins
        // (same precedence as the wrapper generator: plan -> pin -> default).
        let base = base("launch");
        let pinned =
            ExecutableRef::new(&crate::test_util::tmp_abs_str("arena-bin/qwen@x")).unwrap();
        let qwen = adapter("qwen-code");
        let record = create_alias(
            &base,
            &AliasSpec::new(harness("qwen-code"), name("launch")).with_binary(pinned.clone()),
            qwen.as_ref(),
            None,
        )
        .unwrap();

        let composition = launch_composition(
            &base,
            &harness("qwen-code"),
            "launch",
            qwen.as_ref(),
            &["--extra".to_owned()],
            &[],
        )
        .unwrap();

        let pinned_display = pinned.to_string();
        assert_eq!(
            composition.argv.first().map(String::as_str),
            Some(pinned_display.as_str())
        );
        assert_eq!(composition.argv.last().unwrap(), "--extra");
        assert!(
            composition.script.contains("export QWEN_HOME="),
            "script must relocate: {}",
            composition.script
        );
        assert!(
            composition.script.contains(&format!("exec '{pinned}'")),
            "script must exec the pinned binary"
        );
        assert!(
            composition.script.contains("\"$@\""),
            "script forwards caller args"
        );
        assert!(
            composition
                .env
                .iter()
                .any(|(k, v)| k == "QWEN_HOME" && v == &record.root.to_string())
        );
    }

    #[test]
    fn launch_honors_the_plan_executable_over_the_pin() {
        // workbuddy's plan pins `cbc` itself; the plan wins over the record's
        // binary pin and over the harness-name fallback (wrapper contract).
        let base = base("launch-plan-exe");
        let workbuddy = adapter("workbuddy");
        let pinned = ExecutableRef::new(&crate::test_util::tmp_abs_str("arena-bin/cbc@x")).unwrap();
        create_alias(
            &base,
            &AliasSpec::new(harness("workbuddy"), name("plain")).with_binary(pinned),
            workbuddy.as_ref(),
            None,
        )
        .unwrap();
        let composition = launch_composition(
            &base,
            &harness("workbuddy"),
            "plain",
            workbuddy.as_ref(),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(composition.argv.first().unwrap(), "cbc");
        assert!(composition.argv.contains(&"--setting-sources".to_owned()));
        assert!(
            composition.script.contains("export CODEBUDDY_CONFIG_DIR="),
            "script must relocate: {}",
            composition.script
        );
    }

    /// Run-5 desktop harnesses have no relocation mechanism, so creation is
    /// refused up front, before any root, marker, or manifest write.
    #[test]
    fn desktop_harnesses_refuse_alias_creation_at_the_relocation_guard() {
        for harness_id in ["claude-desktop", "chatgpt-desktop"] {
            let base = base(&format!("desktop-{harness_id}"));
            let desktop = adapter(harness_id);
            let spec = AliasSpec::new(harness(harness_id), name("work"))
                .with_mcp_servers(vec![server("echo-test")]);

            let err = create_alias(&base, &spec, desktop.as_ref(), None).unwrap_err();
            match &err {
                CoreError::Validation { field, reason } => {
                    assert_eq!(field, "wrapper_plan.env_vars", "{harness_id}");
                    assert!(
                        reason.contains("no relocation env vars"),
                        "{harness_id}: refusal must name the missing relocation: {reason}"
                    );
                }
                other => panic!("{harness_id}: expected relocation refusal, got {other:?}"),
            }
            assert!(
                !base.join(harness_id).join("work").exists(),
                "{harness_id}: refused alias must leave no root"
            );
            assert!(
                list_aliases(&base).unwrap().is_empty(),
                "{harness_id}: refused alias must not be recorded"
            );
        }
    }

    /// The chatgpt-desktop redirect: its store belongs to codex-cli, so the
    /// MCP surface is declared absent and seeding refuses with that reason.
    #[test]
    fn chatgpt_desktop_alias_story_redirects_to_codex_cli() {
        let desktop = adapter("chatgpt-desktop");
        let absence = desktop
            .mcp_absence_reason()
            .unwrap_or_else(|| panic!("chatgpt-desktop must declare MCP absence"));
        let lowered = absence.to_ascii_lowercase();
        assert!(
            lowered.contains("remote mcp servers"),
            "absence must cite the remote-only position: {absence}"
        );
        assert!(
            absence.contains("codex-cli"),
            "absence must name the owning harness: {absence}"
        );

        let instance = AliasRecord {
            harness: harness("chatgpt-desktop"),
            name: name("work"),
            root: AbsolutePath::new(&crate::test_util::tmp_abs_str(".cgd-alias")).unwrap(),
            binary: None,
            wrapper: None,
            home_virt: false,
            created_at: "2026-09-18T00:00:00Z".to_owned(),
            adapter_revision: desktop.adapter_revision().to_owned(),
        }
        .to_instance()
        .unwrap();
        let plan = desktop.plan_wrapper(&instance).unwrap();
        assert!(
            plan.env_vars.is_empty(),
            "no GUI-level relocation may be fabricated: {:?}",
            plan.env_vars
        );
        assert!(
            plan.description.contains("codex-cli"),
            "plan must redirect aliasing to codex-cli: {}",
            plan.description
        );
        assert!(
            plan.shared_state_warnings
                .iter()
                .any(|w| w.contains("shares ~/.codex with codex-cli")),
            "shared-state warning must name the shared store: {:?}",
            plan.shared_state_warnings
        );

        // Direct seeding through the absent surface refuses with the reason.
        let decl = writable_mcp_decl(&harness("chatgpt-desktop"), desktop.as_ref());
        match decl {
            Err(CoreError::UnsupportedOperation { reason, .. }) => assert_eq!(reason, absence),
            other => panic!("expected MCP-absence refusal, got {other:?}"),
        }
    }

    // Run-5: third-party provider profiles + HOME-virtualized instances

    fn provider_anthropic() -> ProviderProfile {
        ProviderProfile::new(
            ProviderId::new("mock-anthropic").unwrap(),
            "http://127.0.0.1:8787",
        )
        .with_model("gateway-default")
        .with_small_model("gateway-haiku")
    }

    fn provider_openai_responses() -> ProviderProfile {
        ProviderProfile::new(
            ProviderId::new("mock-codex").unwrap(),
            "http://127.0.0.1:8788/v1",
        )
        .with_protocol(Protocol::OpenAiResponses)
        .with_auth_env_var("MOCK_CODEX_KEY")
        .with_model("gateway-codex-model")
    }

    const DUMMY_TOKEN: &str = "sk-superai-mock-dummy-token-12345";

    /// Env-carried family (claude-code): the alias composes `ANTHROPIC_*` at
    /// launch; the token arrives from the CALLER, never from disk.
    #[test]
    fn env_carried_provider_composes_anthropic_vars_from_caller_secrets() {
        let base = base("provider-env");
        let claude = adapter("claude-code");
        let record = create_alias(
            &base,
            &AliasSpec::new(harness("claude-code"), name("gatewayed"))
                .with_provider(provider_anthropic()),
            claude.as_ref(),
            None,
        )
        .unwrap();

        // Nothing provider-ish was written into the harness config: the
        // settings env-block equivalent is NOT used; env is the carrier.
        assert!(
            !record.root.join("settings.json").unwrap().exists(),
            "env-carried providers write no harness config"
        );

        let secrets = [("ANTHROPIC_AUTH_TOKEN", DUMMY_TOKEN)];
        let env = alias_env(
            &base,
            &harness("claude-code"),
            "gatewayed",
            claude.as_ref(),
            &secrets,
        )
        .unwrap();
        let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        // The researcher's exact semantics (code.claude.com env-vars).
        assert_eq!(
            get("ANTHROPIC_BASE_URL").as_deref(),
            Some("http://127.0.0.1:8787"),
            "gateway endpoint var: {env:?}"
        );
        assert_eq!(
            get("ANTHROPIC_AUTH_TOKEN").as_deref(),
            Some(DUMMY_TOKEN),
            "bearer token rides the composed env"
        );
        assert_eq!(get("ANTHROPIC_MODEL").as_deref(), Some("gateway-default"));
        assert_eq!(
            get("ANTHROPIC_DEFAULT_HAIKU_MODEL").as_deref(),
            Some("gateway-haiku")
        );
        // The plan's relocation var coexists (no conflict for claude-code,
        // and PATH is still never part of the composition).
        assert!(get("CLAUDE_CONFIG_DIR").is_some());
        assert!(env.iter().all(|(k, _)| k != "PATH"));

        // Without the caller secret: the auth var is simply absent and the
        // launch composition WARNS (both families validate auth lazily).
        let composition = launch_composition(
            &base,
            &harness("claude-code"),
            "gatewayed",
            claude.as_ref(),
            &[],
            &[],
        )
        .unwrap();
        assert!(composition.secret_env.is_empty());
        assert!(
            composition
                .warnings
                .iter()
                .any(|w| w.contains("validates it lazily")),
            "missing-token warning must surface: {:?}",
            composition.warnings
        );
        // The deterministic script stays plan-only (secret-free launcher
        // files); provider vars ride the direct-exec channel.
        assert!(!composition.script.contains("ANTHROPIC_BASE_URL"));

        // With the secret, the redacted channel carries it without ever
        // exposing it through Debug.
        let composition = launch_composition(
            &base,
            &harness("claude-code"),
            "gatewayed",
            claude.as_ref(),
            &[],
            &[("ANTHROPIC_AUTH_TOKEN", DUMMY_TOKEN)],
        )
        .unwrap();
        assert_eq!(composition.secret_env.len(), 1);
        let dumped = format!("{composition:?}");
        assert!(
            !dumped.contains(DUMMY_TOKEN),
            "LaunchComposition Debug must redact secrets: {dumped}"
        );
    }

    /// Secret hygiene across both families: the dummy token appears nowhere
    /// in the manifest, the provider reference, or the seeded codex config.
    #[test]
    fn provider_secrets_never_reach_the_manifest_state_or_seeded_config() {
        let base = base("provider-secrets");
        let claude = adapter("claude-code");
        create_alias(
            &base,
            &AliasSpec::new(harness("claude-code"), name("envy"))
                .with_provider(provider_anthropic()),
            claude.as_ref(),
            None,
        )
        .unwrap();
        let codex = adapter("codex-cli");
        let codex_record = create_alias(
            &base,
            &AliasSpec::new(harness("codex-cli"), name("seeded"))
                .with_provider(provider_openai_responses()),
            codex.as_ref(),
            None,
        )
        .unwrap();

        let manifest_text = std::fs::read_to_string(base.join(ALIAS_MANIFEST_FILE)).unwrap();
        assert!(!manifest_text.contains(DUMMY_TOKEN));
        let reference_text = std::fs::read_to_string(
            codex_record
                .root
                .join(ALIAS_PROVIDER_REF_FILE)
                .unwrap()
                .as_path(),
        )
        .unwrap();
        assert!(
            !reference_text.contains(DUMMY_TOKEN),
            "provider reference must carry names only: {reference_text}"
        );
        assert!(reference_text.contains("MOCK_CODEX_KEY"));
        let codex_config =
            std::fs::read_to_string(codex_record.root.join("config.toml").unwrap()).unwrap();
        assert!(
            !codex_config.contains(DUMMY_TOKEN),
            "seeded config carries the env_key NAME, never the value: {codex_config}"
        );
        assert!(codex_config.contains("MOCK_CODEX_KEY"));
    }

    /// Config-carried family (codex-cli): the alias `CODEX_HOME` config.toml
    /// gains the provider table, read back fresh through the loader.
    #[test]
    fn codex_provider_seeds_model_providers_table_round_trip() {
        let base = base("provider-codex");
        let codex = adapter("codex-cli");
        let record = create_alias(
            &base,
            &AliasSpec::new(harness("codex-cli"), name("seeded"))
                .with_provider(provider_openai_responses()),
            codex.as_ref(),
            None,
        )
        .unwrap();

        let config = record.root.join("config.toml").unwrap();
        let doc = superai_config::toml_file::load(config.as_path()).unwrap();
        let get = |key: &str| doc.get(key).and_then(|item| item.as_str());
        assert_eq!(get("model_provider"), Some("mock-codex"));
        assert_eq!(get("model"), Some("gateway-codex-model"));
        let table = doc
            .get("model_providers")
            .and_then(|item| item.as_table())
            .unwrap_or_else(|| panic!("model_providers table missing: {doc}"));
        let entry = table
            .get("mock-codex")
            .and_then(|item| item.as_table())
            .unwrap_or_else(|| panic!("model_providers.mock-codex missing: {doc}"));
        let field = |key: &str| {
            entry
                .get(key)
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        };
        assert_eq!(
            field("base_url").as_deref(),
            Some("http://127.0.0.1:8788/v1")
        );
        assert_eq!(field("env_key").as_deref(), Some("MOCK_CODEX_KEY"));
        // wire_api "responses", "chat" was REMOVED from current codex.
        assert_eq!(field("wire_api").as_deref(), Some("responses"));
        assert_eq!(field("name").as_deref(), Some("mock-codex"));

        // Launch composition carries ONLY the env_key (config-carried
        // endpoint stays in the config), supplied by the caller.
        let env = alias_env(
            &base,
            &harness("codex-cli"),
            "seeded",
            codex.as_ref(),
            &[("MOCK_CODEX_KEY", DUMMY_TOKEN)],
        )
        .unwrap();
        assert!(
            env.iter()
                .any(|(k, v)| k == "MOCK_CODEX_KEY" && v == DUMMY_TOKEN)
        );
        assert!(
            !env.iter().any(|(k, _)| k == "ANTHROPIC_BASE_URL"),
            "config-carried providers export no endpoint var: {env:?}"
        );
        assert!(
            env.iter()
                .any(|(k, v)| k == "CODEX_HOME" && v == &record.root.to_string())
        );
    }

    /// The codex redirect (chatgpt-desktop): a HOME-virt alias seeds the
    /// provider table at `<root>/.codex`, the `CODEX_HOME` under virtual HOME.
    #[test]
    fn chatgpt_desktop_home_virt_alias_seeds_the_codex_redirect() {
        let base = base("provider-cgd");
        let desktop = adapter("chatgpt-desktop");
        let record = create_alias(
            &base,
            &AliasSpec::new(harness("chatgpt-desktop"), name("work"))
                .with_home_virt()
                .with_provider(provider_openai_responses()),
            desktop.as_ref(),
            None,
        )
        .unwrap();

        let codex_config = record.root.join(".codex/config.toml").unwrap();
        let doc = superai_config::toml_file::load(codex_config.as_path()).unwrap();
        assert_eq!(
            doc.get("model_provider").and_then(|item| item.as_str()),
            Some("mock-codex")
        );
        let env = alias_env(
            &base,
            &harness("chatgpt-desktop"),
            "work",
            desktop.as_ref(),
            &[("MOCK_CODEX_KEY", DUMMY_TOKEN)],
        )
        .unwrap();
        assert!(
            env.iter()
                .any(|(k, v)| k == "HOME" && v == &record.root.to_string()),
            "HOME-virt env: {env:?}"
        );
        assert!(env.iter().any(|(k, v)| k == "XDG_CONFIG_HOME"
            && v == &record.root.join(".config").unwrap().to_string()));
        assert!(
            env.iter()
                .any(|(k, v)| k == "MOCK_CODEX_KEY" && v == DUMMY_TOKEN)
        );
    }

    /// HOME-virt desktop alias (claude-desktop): HOME is the relocation var
    /// by construction; MCP lands where the HOME-relocated binary reads it.
    #[test]
    fn claude_desktop_home_virt_alias_seeds_mcp_under_xdg_config() {
        let base = base("homevirt-claude");
        let desktop = adapter("claude-desktop");
        let record = create_alias(
            &base,
            &AliasSpec::new(harness("claude-desktop"), name("work"))
                .with_home_virt()
                .with_mcp_servers(vec![server("echo-test")]),
            desktop.as_ref(),
            None,
        )
        .unwrap();

        let decl = desktop.mcp_decl().unwrap();
        let dest = record
            .root
            .join(".config/Claude/claude_desktop_config.json")
            .unwrap();
        assert!(
            dest.is_file(),
            "HOME-virt MCP dest must be XDG-shaped: {dest}"
        );
        let effective = mcp::inspect_servers(dest.as_path(), &decl).unwrap();
        assert_eq!(effective.servers.len(), 1);
        assert!(effective.contains_key(&McpServerId::new("echo-test").unwrap()));
        // The flat dest the 1P decl names is NOT written (the binary would
        // never read it under HOME relocation, factory-droid lesson).
        assert!(
            !record.root.join(&decl.dest_file).unwrap().exists(),
            "flat dest must not be seeded under HOME-virt"
        );

        let env = alias_env(
            &base,
            &harness("claude-desktop"),
            "work",
            desktop.as_ref(),
            &[],
        )
        .unwrap();
        assert!(
            env.iter()
                .any(|(k, v)| k == "HOME" && v == &record.root.to_string()),
            "HOME must relocate to the alias root: {env:?}"
        );
        assert!(env.iter().any(|(k, v)| k == "XDG_CONFIG_HOME"
            && v == &record.root.join(".config").unwrap().to_string()));
        assert!(env.iter().all(|(k, _)| k != "PATH"));
    }

    /// The guard is NOT weakened: HOME-virt is refused without HOME
    /// evidence; the desktops still refuse at the relocation guard.
    #[test]
    fn home_virt_is_refused_outside_the_modeled_desktop_set() {
        let base = base("homevirt-refuse");
        let workbuddy = adapter("workbuddy");
        let spec = AliasSpec::new(harness("workbuddy"), name("nope")).with_home_virt();
        match create_alias(&base, &spec, workbuddy.as_ref(), None).unwrap_err() {
            CoreError::UnsupportedOperation {
                operation, reason, ..
            } => {
                assert_eq!(operation, "home_virt");
                assert!(reason.contains("not weakened"), "{reason}");
            }
            other => panic!("expected UnsupportedOperation, got {other:?}"),
        }
        assert!(!base.join("workbuddy").join("nope").exists());
        assert!(list_aliases(&base).unwrap().is_empty());
    }

    /// Researcher-exact protocol/auth semantics are REFUSED, not coerced:
    /// anthropic-only env for claude-code, responses-only `wire_api` for codex.
    #[test]
    fn provider_protocol_auth_and_harness_mismatches_are_refused() {
        let base = base("provider-refuse");
        let claude = adapter("claude-code");
        let codex = adapter("codex-cli");
        let qwen = adapter("qwen-code");

        let wrong_protocol =
            ProviderProfile::new(ProviderId::new("mock").unwrap(), "http://127.0.0.1:8787")
                .with_protocol(Protocol::OpenAiResponses);
        let err = create_alias(
            &base,
            &AliasSpec::new(harness("claude-code"), name("a")).with_provider(wrong_protocol),
            claude.as_ref(),
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("anthropic messages protocol"),
            "{err}"
        );

        let wrong_auth_var =
            ProviderProfile::new(ProviderId::new("mock").unwrap(), "http://127.0.0.1:8787")
                .with_auth_env_var("ANTHROPIC_API_KEY");
        let err = create_alias(
            &base,
            &AliasSpec::new(harness("claude-code"), name("b")).with_provider(wrong_auth_var),
            claude.as_ref(),
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("requires env var `ANTHROPIC_AUTH_TOKEN`"),
            "{err}"
        );

        let wrong_codex_protocol =
            ProviderProfile::new(ProviderId::new("mock").unwrap(), "http://127.0.0.1:8787");
        let err = create_alias(
            &base,
            &AliasSpec::new(harness("codex-cli"), name("c")).with_provider(wrong_codex_protocol),
            codex.as_ref(),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("`responses` only"), "{err}");

        let err = create_alias(
            &base,
            &AliasSpec::new(harness("qwen-code"), name("d")).with_provider(provider_anthropic()),
            qwen.as_ref(),
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("no provider surface for aliases"),
            "{err}"
        );

        // Every refusal left no root and no record.
        for (harness_id, alias_name) in [
            ("claude-code", "a"),
            ("claude-code", "b"),
            ("codex-cli", "c"),
            ("qwen-code", "d"),
        ] {
            assert!(
                !base.join(harness_id).join(alias_name).exists(),
                "{harness_id}/{alias_name} must leave no root"
            );
        }
        assert!(list_aliases(&base).unwrap().is_empty());
    }

    /// Explicit provider-vs-plan precedence: the overlay WINS on key
    /// conflict, appends otherwise, and never introduces PATH.
    #[test]
    fn overlay_env_provider_wins_on_conflict_and_appends_new_keys() {
        let base_env = vec![
            ("CLAUDE_CONFIG_DIR".to_owned(), "/plan/root".to_owned()),
            (
                "ANTHROPIC_BASE_URL".to_owned(),
                "http://plan-override".to_owned(),
            ),
        ];
        let overlay = vec![
            (
                "ANTHROPIC_BASE_URL".to_owned(),
                "http://provider-wins".to_owned(),
            ),
            ("ANTHROPIC_MODEL".to_owned(), "m1".to_owned()),
        ];
        let merged = overlay_env(base_env, overlay);
        let get = |key: &str| {
            merged
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(
            get("ANTHROPIC_BASE_URL").as_deref(),
            Some("http://provider-wins")
        );
        assert_eq!(get("CLAUDE_CONFIG_DIR").as_deref(), Some("/plan/root"));
        assert_eq!(get("ANTHROPIC_MODEL").as_deref(), Some("m1"));
    }
}
