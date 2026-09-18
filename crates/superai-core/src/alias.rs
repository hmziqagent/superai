//! Multi-instance alias core (run-4 routing area a).
//!
//! An alias is a named, isolated launch configuration of a harness: a FRESH
//! relocated config root under a superai-owned base directory, seeded at
//! creation with a chosen MCP server set (through the existing [`crate::mcp`]
//! write path — never raw file writes) and, where the adapter declares a
//! file-staged plugin mechanism, a plugin set (through [`crate::plugin`]
//! staging). Launch composition is derived from the adapter's own
//! [`crate::adapter::Adapter::plan_wrapper`] — never the generic
//! `env_var_for_harness` fallback, which is wrong for several harnesses.
//!
//! Layout under the caller-chosen base directory:
//!
//! ```text
//! <base>/aliases.json                                  alias manifest
//! <base>/<harness>/<alias-name>/                       alias config root
//! <base>/<harness>/<alias-name>/.superai-alias         ownership marker
//! <base>/<harness>/<alias-name>/.superai/plugins/      per-alias plugin registry
//! <base>/.superai/quarantine/<operation_id>/           quarantined alias roots
//! ```
//!
//! The on-disk manifest is the only registry: it is read fresh on every
//! operation (disk is the truth; nothing is cached in memory) and written
//! through the config crate's mutation boundary, which backs up foreign
//! content and replaces atomically. Alias records carry no model/mcp/plugin
//! data — the alias's effective MCP set lives in the harness's own config
//! files under the alias root, read fresh via [`crate::mcp::inspect_servers`].

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use superai_config::transaction::{FileAction, Transaction};

use crate::adapter::{Adapter, McpAdapterDecl, PluginAdapterDecl, PluginKind, WrapperPlan};
use crate::error::{CoreError, Result};
use crate::ids::{HarnessId, InstanceId, InstanceName};
use crate::instance::{Instance, WrapperRef};
use crate::mcp::{self, McpServerDef};
use crate::paths::{AbsolutePath, ExecutableRef, WrapperPath};
use crate::plugin::{self, PluginSource};
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

// ---------------------------------------------------------------------------
// helpers: time, ids
// ---------------------------------------------------------------------------

fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86400).unwrap_or(0);
    let secs_of_day = secs % 86400;
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// Days since 1970-01-01 to y/m/d (Howard Hinnant's civil-from-days).
fn days_to_ymd(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

/// Deterministic instance id for an alias: stable across recreation of the
/// same (harness, name, root) triple, so the manifest record and any
/// regenerated wrapper stay consistent.
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

fn unique_operation_string(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hasher = DefaultHasher::new();
    millis.hash(&mut hasher);
    count.hash(&mut hasher);
    let suffix = hasher.finish() & 0xffff;
    format!("{prefix}-{millis:013}-{suffix:04x}-{count:04x}")
}

fn transaction_operation_id(prefix: &str) -> Result<superai_config::transaction::OperationId> {
    let candidate = unique_operation_string(prefix);
    superai_config::transaction::OperationId::new(&candidate).map_err(|e| CoreError::Validation {
        field: "operation_id".to_owned(),
        reason: format!("generated operation id invalid: {e}"),
    })
}

// ---------------------------------------------------------------------------
// AliasSpec
// ---------------------------------------------------------------------------

/// Request to create an alias: harness, name, and the optional sets seeded
/// into the fresh alias root at creation time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasSpec {
    /// Harness the alias launches.
    pub harness: HarnessId,
    /// User-chosen alias label (also the root's last path segment).
    pub name: InstanceName,
    /// MCP servers seeded into the adapter-declared destination. The set
    /// lives in the harness's own config file under the alias root, never in
    /// the manifest record.
    pub mcp_servers: Vec<McpServerDef>,
    /// Plugins staged through the adapter-declared plugin mechanism, where it
    /// is a file-staged `directory_bundle` (no external execution).
    pub plugins: Vec<PluginSource>,
    /// Binary the alias launches when the adapter's plan does not name its
    /// own executable (e.g. a pinned arena install). Resolution precedence
    /// matches the wrapper generator: plan executable → this pin → the
    /// harness default on `PATH`.
    pub binary: Option<ExecutableRef>,
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

    /// The alias's isolation root derived from `base_dir` (see [`alias_root`]).
    pub fn root(&self, base_dir: &Path) -> Result<AbsolutePath> {
        alias_root(base_dir, &self.harness, &self.name)
    }
}

/// Derive the alias config root: `<base>/<harness>/<alias-name>`.
///
/// Harness ids and alias names are validated identifiers (no separators, no
/// `..`), so every (harness, name) pair addresses a root that is disjoint
/// from every other pair's root and strictly nested under the base.
pub fn alias_root(
    base_dir: &Path,
    harness: &HarnessId,
    name: &InstanceName,
) -> Result<AbsolutePath> {
    AbsolutePath::from_path(base_dir)?
        .join(harness.as_str())?
        .join(name.as_str())
}

// ---------------------------------------------------------------------------
// AliasRecord
// ---------------------------------------------------------------------------

/// A recorded alias in the on-disk manifest.
///
/// Forbidden fields (never serialized): `model`, `endpoint`, api keys,
/// skill/mcp/plugin lists — the alias's effective MCP/plugin state lives in
/// the harness's own files under `root`, read fresh.
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

// ---------------------------------------------------------------------------
// Manifest (on-disk registry; read fresh every call)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Adapter-declared surfaces (honest refusal paths)
// ---------------------------------------------------------------------------

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

/// Plan the alias launch through the adapter's own `plan_wrapper`, refusing
/// plans that would not isolate (no relocation vars) or that would override
/// `PATH` (alias resolution must not change command lookup).
fn wrapper_plan_for_alias(adapter: &dyn Adapter, instance: &Instance) -> Result<WrapperPlan> {
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
    let plan = adapter.plan_wrapper(instance)?;
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

// ---------------------------------------------------------------------------
// Creation
// ---------------------------------------------------------------------------

/// Create an alias: fresh relocated root, seeded MCP/plugin sets, optional
/// generated wrapper, manifest record committed last.
///
/// Ordering follows the lifecycle create discipline: the root and marker are
/// created through a compensated transaction, the MCP set is written through
/// [`mcp::install_mcp_server`] at the adapter-declared destination, plugins
/// stage through [`plugin::install_directory_bundle`], the wrapper (if any)
/// goes through [`crate::wrapper::write_wrapper`] with its foreign-ownership
/// refusal, and the manifest record is inserted only after everything else
/// succeeded. Any seeding failure quarantines the fresh root before the
/// error propagates, so a half-created alias never lingers.
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
    let instance = AliasRecord {
        harness: spec.harness.clone(),
        name: spec.name.clone(),
        root: root.clone(),
        binary: spec.binary.clone(),
        wrapper: None,
        created_at: now_iso8601(),
        adapter_revision: adapter.adapter_revision().to_owned(),
    }
    .to_instance()?;
    let plan = wrapper_plan_for_alias(adapter, &instance)?;

    create_alias_root(&root, &spec.harness, &spec.name)?;
    let seeded = seed_mcp_set(&root, &spec.harness, adapter, &spec.mcp_servers)
        .and_then(|()| seed_plugin_set(&root, &spec.harness, adapter, &spec.plugins))
        .and_then(|()| generate_alias_wrapper(&instance, &plan, wrapper));
    let wrapper_ref = match seeded {
        Ok(wrapper_ref) => wrapper_ref,
        Err(e) => {
            drop(quarantine_alias_root(base_dir, root.as_path()));
            return Err(e);
        }
    };

    let record = AliasRecord {
        harness: spec.harness.clone(),
        name: spec.name.clone(),
        root,
        binary: spec.binary.clone(),
        wrapper: wrapper_ref,
        created_at: instance.created_at,
        adapter_revision: adapter.adapter_revision().to_owned(),
    };
    insert_manifest_record(base_dir, &record)?;
    Ok(record)
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

/// Seed the MCP set through the existing `mcp` write path at the
/// adapter-declared destination under the alias root.
fn seed_mcp_set(
    root: &AbsolutePath,
    harness: &HarnessId,
    adapter: &dyn Adapter,
    servers: &[McpServerDef],
) -> Result<()> {
    if servers.is_empty() {
        return Ok(());
    }
    let decl = writable_mcp_decl(harness, adapter)?;
    let dest = root.join(&decl.dest_file)?;
    for server in servers {
        mcp::install_mcp_server(dest.as_path(), &decl, server)?;
    }
    Ok(())
}

/// Stage the plugin set through the existing `plugin` machinery, with the
/// plugin registry colocated under the alias root so removing the alias
/// removes the registry with it.
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
    let (content, digest) = wrapper_helper::generate_shell_wrapper(instance, plan);
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
    let op = unique_operation_string("alias-failure");
    superai_config::quarantine::move_to_quarantine_under(base_dir, root, &op)
        .map(|entry| entry.quarantine_path)
        .map_err(CoreError::Config)
}

// ---------------------------------------------------------------------------
// Listing and lookup
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Launch composition
// ---------------------------------------------------------------------------

/// The composed launch environment for an alias: the adapter plan's
/// relocation variables pointing at the alias root. `PATH` is never part of
/// it (refused in [`crate::wrapper`] planning already).
pub fn alias_env(
    base_dir: &Path,
    harness: &HarnessId,
    name: &str,
    adapter: &dyn Adapter,
) -> Result<Vec<(String, String)>> {
    let record = get_alias(base_dir, harness, name)?;
    let instance = record.to_instance()?;
    let plan = wrapper_plan_for_alias(adapter, &instance)?;
    Ok(plan.env_vars)
}

/// A runnable command for an alias: direct `argv` + env for in-process
/// exec, or the deterministic `sh` script text for an on-disk launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchComposition {
    /// argv to exec: resolved binary, the plan's fixed args, then
    /// `extra_args`.
    pub argv: Vec<String>,
    /// Environment variables to set (same set the script exports).
    pub env: Vec<(String, String)>,
    /// Environment variables to unset before exec (WRP-02 leak guard).
    pub env_unset: Vec<String>,
    /// Fixed working directory, when the adapter declares one.
    pub working_dir: Option<String>,
    /// Deterministic `#!/bin/sh` launcher script (execs the binary with the
    /// plan args and forwards the caller's `"$@"`).
    pub script: String,
}

/// Compose how an alias launches: the adapter plan's executable/args/env
/// resolved against the alias root, plus the generated wrapper script text.
///
/// The executable resolves with the wrapper generator's precedence: the
/// plan's own executable, else the record's pinned binary, else the harness
/// default on `PATH`. `PATH` itself is never modified by the composed env.
/// `extra_args` are appended to `argv` for direct exec; the script forwards
/// them at runtime via `"$@"`.
pub fn launch_composition(
    base_dir: &Path,
    harness: &HarnessId,
    name: &str,
    adapter: &dyn Adapter,
    extra_args: &[String],
) -> Result<LaunchComposition> {
    let record = get_alias(base_dir, harness, name)?;
    let instance = record.to_instance()?;
    let plan = wrapper_plan_for_alias(adapter, &instance)?;
    let executable = plan
        .executable
        .clone()
        .or_else(|| instance.binary.as_ref().map(ToString::to_string))
        .unwrap_or_else(|| wrapper_helper::executable_for_harness(&instance.harness));
    let mut argv = Vec::with_capacity(1 + plan.args.len() + extra_args.len());
    argv.push(executable);
    argv.extend(plan.args.iter().cloned());
    argv.extend(extra_args.iter().cloned());
    let (script, _digest) = wrapper_helper::generate_shell_wrapper(&instance, &plan);
    Ok(LaunchComposition {
        argv,
        env: plan.env_vars,
        env_unset: plan.env_unset,
        working_dir: plan.working_dir,
        script,
    })
}

// ---------------------------------------------------------------------------
// Removal
// ---------------------------------------------------------------------------

/// Whether `path` is `base` itself or nested under it (component-wise).
fn path_is_under(path: &Path, base: &Path) -> bool {
    path.starts_with(base)
}

/// Verify the alias marker names this harness and alias (case-folded on the
/// name, matching lookup semantics). A missing or mismatched marker means
/// the directory is not a managed alias root and must not be touched.
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

/// Remove an alias: its wrapper (only when superai-owned with the recorded
/// digest), its root (moved to quarantine — recoverable, never a blind
/// recursive delete), and its manifest entry.
///
/// Refuses up front when the recorded root lies outside the alias base or
/// the root lacks a matching ownership marker.
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
    if let Some(wrapper) = &record.wrapper
        && wrapper.path.as_path().exists()
        && wrapper_helper::is_owned_wrapper(wrapper.path.as_path(), Some(&wrapper.content_digest))
    {
        std::fs::remove_file(wrapper.path.as_path()).map_err(|e| {
            CoreError::Config(superai_config::ConfigError::Io {
                path: wrapper.path.as_path().to_path_buf(),
                source: e,
            })
        })?;
    }
    if record.root.as_path().exists() {
        // Recovery state stays under the alias base, never the user's home.
        let op = unique_operation_string("alias-remove");
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
                effective.len(),
                2,
                "{harness_id}: both seeded servers must round-trip"
            );
            let alpha = McpServerId::new("alpha").unwrap();
            let seeded_alpha = effective
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

    /// Alias quarantine ops currently recorded under the REAL home (prefix
    /// filtering is race-free: only alias.rs mints `alias-*` operation ids,
    /// and after the fix none of them target the home base at all).
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

            let env = alias_env(&base, &harness(harness_id), "envy", adapter.as_ref()).unwrap();
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

    /// Run-5 desktop harnesses: both apps have NO relocation mechanism
    /// (verified-absent), so their plans carry no env vars and alias
    /// creation is refused up front — before any root, marker, or manifest
    /// write. Refusing is the honest outcome, not a missing feature.
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

    /// The redirected refusal (chatgpt-desktop): the store it reads belongs
    /// to codex-cli, so its MCP surface is declared absent with the
    /// remote-only + shared-store citation, and the wrapper plan points
    /// aliasing at codex-cli. Seeding still refuses with the absence reason
    /// even when a plan could be composed.
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
}
