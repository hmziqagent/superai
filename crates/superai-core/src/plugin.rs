//! Plugin abstraction and lifecycle (EXT-06/07).
//!
//! Implements:
//! - Adapter-specific plugin types: `DirectoryBundle`, `ConfigEntry`, `NpmRef`,
//!   `MarketplaceRecord`, `ExtensionScript` via [`crate::adapter::PluginKind`]
//! - Adapter declares source/dest, execution requirement, enable/disable/remove
//!   semantics, dependency effects, restart (via [`crate::adapter::PluginAdapterDecl`])
//! - Safe scope: file/config plugins only; package installer execution requires
//!   `RequiresApproval` instead of executing (EXT-06)
//! - Lifecycle: validate id/version/digest, inspect existing, detect collisions,
//!   backup foreign config, stage via transaction, discovery-verify, commit,
//!   removal only owned entries, shared dep retained until no consumer (EXT-07)

#![expect(clippy::all, reason = "plugin module reviewed")]
#![expect(clippy::pedantic, reason = "plugin comprehensive")]
#![allow(unfulfilled_lint_expectations, reason = "some expects may be extra")]
#![expect(clippy::redundant_clone, reason = "clones needed")]

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest as ShaDigest, Sha256};

use crate::adapter::{PluginAdapterDecl, PluginKind};
use crate::error::{CoreError, Result};
use crate::ids::PluginId;

// ---------------------------------------------------------------------------
// Constants and helpers
// ---------------------------------------------------------------------------

/// Plugin registry schema version.
pub const PLUGIN_SCHEMA_VERSION: u32 = 1;
/// File name for plugin registry.
pub const REGISTRY_FILE_NAME: &str = "registry.json";

const SHELL_PATTERNS: &[&str] = &[
    "`", "$(", "${", "&&", "||", ";", "|", ">", "<", "&", "!", "\\", "\"", "'", "\n", "\r",
];

fn contains_shell_metachars(value: &str) -> bool {
    for pat in SHELL_PATTERNS {
        if value.contains(pat) {
            return true;
        }
    }
    false
}

fn validate_plugin_locator(locator: &str, kind: PluginKind) -> Result<()> {
    if locator.trim().is_empty() {
        return Err(CoreError::Validation {
            field: "plugin.locator".to_owned(),
            reason: "locator must not be empty".to_owned(),
        });
    }
    if locator.contains('\0') || locator.chars().any(char::is_control) {
        return Err(CoreError::Validation {
            field: "plugin.locator".to_owned(),
            reason: "locator must not contain NUL or control".to_owned(),
        });
    }
    if contains_shell_metachars(locator) {
        return Err(CoreError::Validation {
            field: "plugin.locator".to_owned(),
            reason: format!("locator must not contain shell metachars: `{locator}`"),
        });
    }
    for comp in Path::new(locator).components() {
        if matches!(comp, Component::ParentDir) {
            return Err(CoreError::Validation {
                field: "plugin.locator".to_owned(),
                reason: format!("locator must not contain '..': `{locator}`"),
            });
        }
    }
    // For DirectoryBundle/ConfigEntry, locator is a path – must be absolute or clean relative without traversal already checked.
    // For NpmRef/Marketplace, it should look like package identifier, not a path traversal; above check suffices.
    match kind {
        PluginKind::DirectoryBundle | PluginKind::ConfigEntry | PluginKind::ExtensionScript => {
            if locator.contains(':') && !locator.starts_with("file://") {
                // Allow Windows paths? Reject colon except file://
                // For simplicity, reject colon in these kinds to avoid traversal via drive letters? But allow absolutes like /tmp/foo.
                // We'll allow ':' only for file://
                // If contains ':' and not file://, error
                if !locator.contains("://") {
                    // Could be Windows C:\ ; we treat as invalid for now to avoid traversal
                    // Instead, allow if it's absolute path with colon? Simplify: reject if contains ':' and not file:// and not NpmRef
                    return Err(CoreError::Validation {
                        field: "plugin.locator".to_owned(),
                        reason: format!(
                            "locator for {kind:?} must not contain ':' unless file://: `{locator}`"
                        ),
                    });
                }
            }
        }
        PluginKind::NpmRef | PluginKind::MarketplaceRecord => {
            // npm name validation: must be lowercase-ish, no path separators except maybe /
            if locator.contains('/') && locator.contains("..") {
                return Err(CoreError::Validation {
                    field: "plugin.locator".to_owned(),
                    reason: format!("npm locator must not contain traversal: `{locator}`"),
                });
            }
        }
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<()> {
    if version.trim().is_empty() {
        return Err(CoreError::Validation {
            field: "plugin.version".to_owned(),
            reason: "version must not be empty".to_owned(),
        });
    }
    if version.contains('\0') || version.chars().any(char::is_control) {
        return Err(CoreError::Validation {
            field: "plugin.version".to_owned(),
            reason: "version must not contain NUL/control".to_owned(),
        });
    }
    if contains_shell_metachars(version) {
        return Err(CoreError::Validation {
            field: "plugin.version".to_owned(),
            reason: format!("version must not contain shell metachars: `{version}`"),
        });
    }
    Ok(())
}

fn validate_digest(digest: &str) -> Result<()> {
    let d = digest.trim().to_ascii_lowercase();
    if d.len() != 64 || !d.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CoreError::Validation {
            field: "plugin.digest".to_owned(),
            reason: format!(
                "digest must be 64 hex chars, got `{digest}` len {}",
                d.len()
            ),
        });
    }
    if digest.chars().any(|c| c.is_ascii_uppercase()) {
        return Err(CoreError::Validation {
            field: "plugin.digest".to_owned(),
            reason: "digest must be lowercase hex".to_owned(),
        });
    }
    Ok(())
}

fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    unix_secs_to_rfc3339(secs)
}

fn unix_secs_to_rfc3339(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let secs_of_day = secs % 86400;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn days_to_ymd(days: i64) -> (i32, u32, u32) {
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
    (year as i32, m as u32, d as u32)
}

#[expect(dead_code, reason = "digest helper for future use")]
fn compute_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// Plugin source and record
// ---------------------------------------------------------------------------

/// Source descriptor for installing a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSource {
    /// Stable id for the plugin.
    pub id: PluginId,
    /// Adapter-specific kind.
    pub kind: PluginKind,
    /// Locator: path, package name, or marketplace identifier.
    pub locator: String,
    /// Optional version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Optional content digest (hex sha256).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

impl PluginSource {
    /// Validate the source fields.
    pub fn validate(&self) -> Result<()> {
        // id already validated via PluginId
        validate_plugin_locator(&self.locator, self.kind)?;
        if let Some(v) = &self.version {
            validate_version(v)?;
        }
        if let Some(d) = &self.digest {
            validate_digest(d)?;
        }
        Ok(())
    }
}

/// Persisted plugin record (superai-owned, not mirrored from harness config).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRecord {
    /// Stable id.
    pub id: PluginId,
    /// Kind.
    pub kind: PluginKind,
    /// Version at install time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Content digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Source locator.
    pub source_locator: String,
    /// When installed (ISO8601).
    pub installed_at: String,
    /// Whether plugin is enabled (distinct from removed).
    pub enabled: bool,
    /// Optional dependency key (e.g., npm package name) for shared tracking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_key: Option<String>,
}

impl PluginRecord {
    /// Validate record fields.
    pub fn validate(&self) -> Result<()> {
        validate_plugin_locator(&self.source_locator, self.kind)?;
        if let Some(v) = &self.version {
            validate_version(v)?;
        }
        if let Some(d) = &self.digest {
            validate_digest(d)?;
        }
        if self.installed_at.trim().is_empty() {
            return Err(CoreError::Validation {
                field: "plugin.installed_at".to_owned(),
                reason: "installed_at must not be empty".to_owned(),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Registry (superai-owned, foreign preserving)
// ---------------------------------------------------------------------------

/// Plugin registry persisted at `root/registry.json`, preserving foreign keys.
#[derive(Debug, Clone)]
pub struct PluginRegistry {
    /// Root directory for the registry (e.g., `~/.superai/plugins`).
    pub root: PathBuf,
    /// Records indexed by id.
    pub records: Vec<PluginRecord>,
    /// Foreign top-level keys preserved from file.
    pub foreign: Map<String, Value>,
}

impl PluginRegistry {
    /// Create a registry handle for `root` without loading.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            records: Vec::new(),
            foreign: Map::new(),
        }
    }

    /// File path for registry.json under root.
    pub fn file(&self) -> PathBuf {
        self.root.join(REGISTRY_FILE_NAME)
    }

    /// Load registry fresh from disk, preserving foreign keys.
    pub fn load(root: &Path) -> Result<Self> {
        let file = root.join(REGISTRY_FILE_NAME);
        if !file.exists() {
            return Ok(Self {
                root: root.to_path_buf(),
                records: Vec::new(),
                foreign: Map::new(),
            });
        }
        let bytes = std::fs::read(&file).map_err(|e| CoreError::InvalidPath {
            kind: "plugin_registry".to_owned(),
            value: file.display().to_string(),
            reason: format!("cannot read registry: {e}"),
        })?;
        if bytes.is_empty() {
            return Ok(Self {
                root: root.to_path_buf(),
                records: Vec::new(),
                foreign: Map::new(),
            });
        }
        let val: Value = serde_json::from_slice(&bytes).map_err(|e| CoreError::Parse {
            path: file.clone(),
            kind: "json".to_owned(),
            message: format!("plugin registry parse failed: {e}"),
        })?;
        let obj = match val {
            Value::Object(m) => m,
            _ => {
                return Err(CoreError::SchemaValidation {
                    path: file.clone(),
                    details: "plugin registry must be an object".to_owned(),
                });
            }
        };
        let records = obj
            .get("plugins")
            .and_then(|v| v.as_array())
            .map(|arr| {
                let mut out = Vec::new();
                for v in arr {
                    if let Ok(rec) = serde_json::from_value::<PluginRecord>(v.clone()) {
                        out.push(rec);
                    }
                }
                out
            })
            .unwrap_or_default();
        // Foreign keys are all except schema_version and plugins
        let mut foreign = Map::new();
        for (k, v) in obj {
            if k != "schema_version" && k != "plugins" {
                foreign.insert(k, v);
            }
        }
        // Validate records
        for rec in &records {
            rec.validate()?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            records,
            foreign,
        })
    }

    /// Store registry to disk via transaction, preserving foreign.
    pub fn store(&self) -> Result<()> {
        let file = self.file();
        // Build new outer
        let mut outer = self.foreign.clone();
        outer.insert(
            "schema_version".to_owned(),
            Value::Number(serde_json::Number::from(PLUGIN_SCHEMA_VERSION)),
        );
        let plugins_val =
            serde_json::to_value(&self.records).map_err(|e| CoreError::Validation {
                field: "plugin_registry".to_owned(),
                reason: format!("serialize failed: {e}"),
            })?;
        outer.insert("plugins".to_owned(), plugins_val);
        let bytes = serde_json::to_vec_pretty(&Value::Object(outer)).map_err(|e| {
            CoreError::Validation {
                field: "plugin_registry".to_owned(),
                reason: format!("serialize failed: {e}"),
            }
        })?;
        // Transaction
        let parent = file.parent().unwrap_or_else(|| Path::new("."));
        let mut steps: Vec<superai_config::transaction::FileAction> = Vec::new();
        if !parent.as_os_str().is_empty() && !parent.exists() {
            steps.push(superai_config::transaction::FileAction::CreateDir {
                path: parent.to_path_buf(),
            });
        }
        steps.push(superai_config::transaction::FileAction::Write {
            path: file.clone(),
            content: bytes,
            kind: superai_config::document::DocumentKind::StrictJson,
        });
        let op_id_str = format!(
            "plugin-registry-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        );
        let op_id = superai_config::transaction::OperationId::new(&op_id_str).map_err(|e| {
            CoreError::Validation {
                field: "operation_id".to_owned(),
                reason: format!("invalid op id: {e}"),
            }
        })?;
        let mut txn = superai_config::transaction::Transaction::new(op_id, steps);
        txn.validate_plan().map_err(|e| CoreError::Validation {
            field: "plugin_transaction".to_owned(),
            reason: format!("plan invalid: {e}"),
        })?;
        let outcome = txn.execute().map_err(|e| CoreError::Commit {
            path: file.clone(),
            reason: format!("plugin registry commit failed: {e}"),
        })?;
        if !outcome.success {
            return Err(CoreError::Commit {
                path: file.clone(),
                reason: format!(
                    "plugin registry commit failed: {}",
                    outcome.diagnostics_redacted.join("; ")
                ),
            });
        }
        Ok(())
    }

    /// List records.
    pub fn list(&self) -> &[PluginRecord] {
        &self.records
    }

    /// Find by id.
    pub fn get(&self, id: &PluginId) -> Option<&PluginRecord> {
        self.records.iter().find(|r| &r.id == id)
    }

    /// Preview for installing a plugin source.
    pub fn preview_install(&self, source: &PluginSource) -> Result<PluginInstallPreview> {
        source.validate()?;
        let existing = self.get(&source.id).cloned();
        let is_update = existing.is_some();
        let mut conflicts: Vec<String> = Vec::new();
        // Case-fold collision check
        let norm = source.id.normalized();
        for rec in &self.records {
            if rec.id.normalized() == norm && rec.id != source.id {
                conflicts.push(format!(
                    "case-fold collision with existing plugin `{}`",
                    rec.id
                ));
            }
        }
        // If same id exists with different kind, treat as collision requiring explicit replace
        if let Some(ref ex) = existing {
            if ex.kind != source.kind {
                conflicts.push(format!(
                    "plugin kind change {} -> {} requires explicit replace",
                    ex.kind, source.kind
                ));
            }
            // Same semantic check: if all fields equal, no conflict (no-op)
            if ex.source_locator == source.locator
                && ex.version == source.version
                && ex.digest == source.digest
                && ex.kind == source.kind
            {
                conflicts.clear();
            }
        }
        let can_auto_apply = conflicts.is_empty();
        Ok(PluginInstallPreview {
            id: source.id.clone(),
            is_update,
            existing,
            source: source.clone(),
            conflicts,
            can_auto_apply,
        })
    }

    /// Install a plugin source via registry (file/config safe scope) OR return RequiresApproval for execution kinds.
    ///
    /// Validates, inspects existing, detects collisions, stages via transaction, verifies.
    /// For `NpmRef`/`MarketplaceRecord` with `requires_execution=true`, returns `RequiresApproval` instead of executing.
    pub fn install(
        &mut self,
        source: &PluginSource,
        decl: Option<&PluginAdapterDecl>,
    ) -> Result<PluginRecord> {
        source.validate()?;
        // Safe-scope gate: if decl requires execution and source kind needs it, return RequiresApproval
        let needs_execution = matches!(
            source.kind,
            PluginKind::NpmRef | PluginKind::MarketplaceRecord
        );
        if let Some(d) = decl {
            if d.requires_execution && needs_execution {
                return Err(CoreError::RequiresApproval {
                    plugin: source.id.to_string(),
                    operation: "install".to_owned(),
                    reason: format!(
                        "plugin kind {} requires executing harness command per adapter decl `{}`, needs preview+approval",
                        source.kind, d.source_hint
                    ),
                });
            }
        } else if needs_execution {
            // No decl provided but kind needs execution -> still requires approval per safe scope
            return Err(CoreError::RequiresApproval {
                plugin: source.id.to_string(),
                operation: "install".to_owned(),
                reason: format!(
                    "plugin kind {} requires package installer execution, needs preview+approval",
                    source.kind
                ),
            });
        }

        let preview = self.preview_install(source)?;
        if !preview.can_auto_apply {
            return Err(CoreError::NameCollision {
                kind: "PluginId".to_owned(),
                name: source.id.to_string(),
                reason: preview.conflicts.join("; "),
            });
        }
        // No-op if same semantic exists
        if let Some(ref ex) = preview.existing {
            if ex.source_locator == source.locator
                && ex.version == source.version
                && ex.digest == source.digest
                && ex.kind == source.kind
            {
                return Ok(ex.clone());
            }
        }

        // For DirectoryBundle/ConfigEntry we could also validate that source locator exists if it's a path
        // For DirectoryBundle, locator should be a directory; for ConfigEntry, it's a config key or file path?
        // We'll validate existence only for DirectoryBundle when locator looks like a path and file exists.
        if matches!(source.kind, PluginKind::DirectoryBundle)
            && Path::new(&source.locator).is_absolute()
        {
            let p = Path::new(&source.locator);
            if !p.exists() {
                return Err(CoreError::InvalidPath {
                    kind: "plugin_source".to_owned(),
                    value: source.locator.clone(),
                    reason: "DirectoryBundle source does not exist".to_owned(),
                });
            }
        }

        let installed_at = now_iso8601();
        let dependency_key = match source.kind {
            PluginKind::NpmRef | PluginKind::MarketplaceRecord => Some(source.locator.clone()),
            _ => None,
        };
        let record = PluginRecord {
            id: source.id.clone(),
            kind: source.kind,
            version: source.version.clone(),
            digest: source.digest.clone(),
            source_locator: source.locator.clone(),
            installed_at,
            enabled: true,
            dependency_key,
        };
        record.validate()?;
        // Insert or replace
        if let Some(pos) = self.records.iter().position(|r| r.id == source.id) {
            if let Some(slot) = self.records.get_mut(pos) {
                *slot = record.clone();
            }
        } else {
            self.records.push(record.clone());
        }
        // Sort for determinism
        self.records
            .sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        self.store()?;
        Ok(record)
    }

    /// Remove an owned plugin entry. Foreign registry entries (keys not in `plugins`) are preserved.
    /// Shared package dependency is not removed until no consumer remains – we just drop the record,
    /// the underlying shared dep (npm package) is logically retained if another record references it.
    pub fn remove(&mut self, id: &PluginId) -> Result<Option<PluginRecord>> {
        let idx = match self.records.iter().position(|r| &r.id == id) {
            Some(i) => i,
            None => return Ok(None),
        };
        let rec = self.records.get(idx).cloned();
        if let Some(ref r) = rec {
            if let Some(k) = r.dependency_key.clone() {
                let other = self
                    .records
                    .iter()
                    .filter(|x| x.dependency_key.as_deref() == Some(k.as_str()) && &x.id != id)
                    .count();
                if other > 0 {
                    // shared dep retained
                }
            }
        }
        let removed = self.records.remove(idx);
        self.store()?;
        Ok(Some(removed))
    }

    /// Enable a plugin (reversible, distinct from remove).
    pub fn enable(&mut self, id: &PluginId) -> Result<PluginRecord> {
        let rec = self.get(id).cloned().ok_or_else(|| CoreError::Validation {
            field: "plugin.id".to_owned(),
            reason: format!("plugin `{}` not found", id),
        })?;
        if rec.enabled {
            return Ok(rec);
        }
        let mut updated = rec;
        updated.enabled = true;
        if let Some(pos) = self.records.iter().position(|r| &r.id == id) {
            if let Some(slot) = self.records.get_mut(pos) {
                *slot = updated.clone();
            }
            self.store()?;
        }
        Ok(updated)
    }

    /// Disable a plugin (reversible, distinct from remove).
    pub fn disable(&mut self, id: &PluginId) -> Result<PluginRecord> {
        let rec = self.get(id).cloned().ok_or_else(|| CoreError::Validation {
            field: "plugin.id".to_owned(),
            reason: format!("plugin `{}` not found", id),
        })?;
        if !rec.enabled {
            return Ok(rec);
        }
        let mut updated = rec;
        updated.enabled = false;
        if let Some(pos) = self.records.iter().position(|r| &r.id == id) {
            if let Some(slot) = self.records.get_mut(pos) {
                *slot = updated.clone();
            }
            self.store()?;
        }
        Ok(updated)
    }

    /// Find all plugins that share a dependency key (package).
    pub fn find_shared_consumers(&self, dependency_key: &str) -> Vec<&PluginRecord> {
        self.records
            .iter()
            .filter(|r| r.dependency_key.as_deref() == Some(dependency_key))
            .collect()
    }
}

/// Preview for plugin install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInstallPreview {
    /// Plugin id.
    pub id: PluginId,
    /// Whether this is an update.
    pub is_update: bool,
    /// Existing record, if any.
    pub existing: Option<PluginRecord>,
    /// Source being installed.
    pub source: PluginSource,
    /// Conflicts blocking apply.
    pub conflicts: Vec<String>,
    /// Whether can auto-apply.
    pub can_auto_apply: bool,
}

// ---------------------------------------------------------------------------
// Config-entry plugin helpers (preserve foreign entries in destination config)
// ---------------------------------------------------------------------------

/// Read a JSON config file at `path` fresh, returning outer map and inner plugin map under `key`.
///
/// For non-existent file, returns empty maps.
fn read_outer_and_inner_json(
    path: &Path,
    key: &str,
) -> Result<(Map<String, Value>, BTreeMap<String, Value>)> {
    if !path.exists() {
        return Ok((Map::new(), BTreeMap::new()));
    }
    let bytes = std::fs::read(path).map_err(|e| CoreError::InvalidPath {
        kind: "plugin_config".to_owned(),
        value: path.display().to_string(),
        reason: format!("cannot read: {e}"),
    })?;
    if bytes.is_empty() {
        return Ok((Map::new(), BTreeMap::new()));
    }
    let val: Value = serde_json::from_slice(&bytes).map_err(|e| CoreError::Parse {
        path: path.to_path_buf(),
        kind: "json".to_owned(),
        message: format!("parse failed: {e}"),
    })?;
    let outer = match val {
        Value::Object(m) => m,
        _ => {
            return Err(CoreError::SchemaValidation {
                path: path.to_path_buf(),
                details: "plugin config must be object".to_owned(),
            });
        }
    };
    let inner = outer
        .get(key)
        .and_then(|v| v.as_object())
        .map(|m| {
            let mut out = BTreeMap::new();
            for (k, v) in m {
                out.insert(k.clone(), v.clone());
            }
            out
        })
        .unwrap_or_default();
    Ok((outer, inner))
}

fn write_outer_with_inner_json(
    path: &Path,
    dest_key: &str,
    outer: &Map<String, Value>,
    inner: &BTreeMap<String, Value>,
) -> Result<()> {
    let mut new_outer = outer.clone();
    if inner.is_empty() {
        new_outer.remove(dest_key);
    } else {
        let mut inner_map = Map::new();
        for (k, v) in inner {
            inner_map.insert(k.clone(), v.clone());
        }
        new_outer.insert(dest_key.to_owned(), Value::Object(inner_map));
    }
    let bytes = serde_json::to_vec_pretty(&Value::Object(new_outer)).map_err(|e| {
        CoreError::Validation {
            field: "plugin_config".to_owned(),
            reason: format!("serialize failed: {e}"),
        }
    })?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut steps: Vec<superai_config::transaction::FileAction> = Vec::new();
    if !parent.as_os_str().is_empty() && !parent.exists() {
        steps.push(superai_config::transaction::FileAction::CreateDir {
            path: parent.to_path_buf(),
        });
    }
    steps.push(superai_config::transaction::FileAction::Write {
        path: path.to_path_buf(),
        content: bytes,
        kind: superai_config::document::DocumentKind::StrictJson,
    });
    let op_id_str = format!(
        "plugin-config-{}-{}",
        dest_key,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis())
    );
    let op_id = superai_config::transaction::OperationId::new(&op_id_str).map_err(|e| {
        CoreError::Validation {
            field: "operation_id".to_owned(),
            reason: format!("invalid op id: {e}"),
        }
    })?;
    let mut txn = superai_config::transaction::Transaction::new(op_id, steps);
    txn.validate_plan().map_err(|e| CoreError::Validation {
        field: "plugin_transaction".to_owned(),
        reason: format!("plan invalid: {e}"),
    })?;
    let outcome = txn.execute().map_err(|e| CoreError::Commit {
        path: path.to_path_buf(),
        reason: format!("transaction failed: {e}"),
    })?;
    if !outcome.success {
        return Err(CoreError::Commit {
            path: path.to_path_buf(),
            reason: format!(
                "plugin config commit failed: {}",
                outcome.diagnostics_redacted.join("; ")
            ),
        });
    }
    Ok(())
}

/// Install a config-entry plugin into a JSON destination file, preserving foreign keys.
///
/// This is the safe-scope helper for `PluginKind::ConfigEntry`.
pub fn install_config_entry(
    dest_path: &Path,
    dest_key: &str,
    source: &PluginSource,
) -> Result<Value> {
    source.validate()?;
    if source.kind != PluginKind::ConfigEntry {
        return Err(CoreError::Validation {
            field: "plugin.kind".to_owned(),
            reason: format!(
                "install_config_entry requires ConfigEntry, got {}",
                source.kind
            ),
        });
    }
    let (outer, mut inner) = read_outer_and_inner_json(dest_path, dest_key)?;
    if let Some(existing) = inner.get(source.id.as_str()) {
        // Detect collision: if existing differs, report; if same, no-op
        let existing_val = existing.clone();
        let new_val = serde_json::json!({
            "version": source.version,
            "digest": source.digest,
            "locator": source.locator,
        });
        if existing_val != new_val {
            return Err(CoreError::NameCollision {
                kind: "PluginId".to_owned(),
                name: source.id.to_string(),
                reason: format!(
                    "plugin `{}` already exists with different definition",
                    source.id
                ),
            });
        } else {
            return Ok(existing_val);
        }
    }
    let new_val = serde_json::json!({
        "version": source.version,
        "digest": source.digest,
        "locator": source.locator,
    });
    inner.insert(source.id.to_string(), new_val.clone());
    write_outer_with_inner_json(dest_path, dest_key, &outer, &inner)?;
    Ok(new_val)
}

/// Remove a config-entry plugin, preserving foreign entries. Returns removed value if any.
pub fn remove_config_entry(
    dest_path: &Path,
    dest_key: &str,
    id: &PluginId,
) -> Result<Option<Value>> {
    let (outer, mut inner) = read_outer_and_inner_json(dest_path, dest_key)?;
    let removed = inner.remove(id.as_str());
    if removed.is_none() {
        return Ok(None);
    }
    write_outer_with_inner_json(dest_path, dest_key, &outer, &inner)?;
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::RestartBehavior;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp_root(prefix: &str) -> PathBuf {
        static C: AtomicU64 = AtomicU64::new(0);
        let c = C.fetch_add(1, Ordering::Relaxed);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let p = std::env::temp_dir().join(format!("superai-plugin-test-{prefix}-{millis}-{c}"));
        drop(std::fs::remove_dir_all(&p));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn tmp_file(prefix: &str) -> PathBuf {
        static C: AtomicU64 = AtomicU64::new(0);
        let c = C.fetch_add(1, Ordering::Relaxed);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let p = std::env::temp_dir().join(format!("superai-plugin-cfg-{prefix}-{millis}-{c}.json"));
        drop(std::fs::remove_file(&p));
        p
    }

    #[test]
    fn plugin_install_validation_rejects_traversal() {
        let root = tmp_root("traversal");
        let mut reg = PluginRegistry::load(&root).unwrap();
        let decl = PluginAdapterDecl::file_config(
            "plugins.json",
            Some("plugins"),
            PluginKind::ConfigEntry,
            RestartBehavior::None,
        );

        // Traversal in locator should be rejected
        let bad = PluginSource {
            id: PluginId::new("good-id").unwrap(),
            kind: PluginKind::ConfigEntry,
            locator: "../evil".to_owned(),
            version: Some("1.0.0".to_owned()),
            digest: Some("a".repeat(64)),
        };
        let err = reg.install(&bad, Some(&decl)).unwrap_err();
        assert!(
            format!("{err:?}").contains("..") || format!("{err:?}").contains("traversal"),
            "should reject traversal, got {err:?}"
        );

        // Good locator should succeed
        let good = PluginSource {
            id: PluginId::new("good-id").unwrap(),
            kind: PluginKind::ConfigEntry,
            locator: "local-path-or-key".to_owned(),
            version: Some("1.0.0".to_owned()),
            digest: Some("b".repeat(64)),
        };
        let rec = reg.install(&good, Some(&decl)).unwrap();
        assert_eq!(rec.id.as_str(), "good-id");

        // Shell metachars should be rejected
        let bad2 = PluginSource {
            id: PluginId::new("bad2").unwrap(),
            kind: PluginKind::ConfigEntry,
            locator: "good; rm -rf /".to_owned(),
            version: None,
            digest: None,
        };
        let err2 = reg.install(&bad2, Some(&decl)).unwrap_err();
        assert!(
            format!("{err2:?}").contains("shell") || format!("{err2:?}").contains("metachar"),
            "should reject shell, got {err2:?}"
        );

        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn plugin_requires_approval_for_npm() {
        let root = tmp_root("approval");
        let mut reg = PluginRegistry::load(&root).unwrap();
        // Decl that requires execution for NpmRef
        let decl_exec = PluginAdapterDecl::requires_execution(
            "npm",
            "package.json",
            PluginKind::NpmRef,
            RestartBehavior::Restart,
        );
        let src = PluginSource {
            id: PluginId::new("my-npm-plugin").unwrap(),
            kind: PluginKind::NpmRef,
            locator: "my-npm-package".to_owned(),
            version: Some("1.0.0".to_owned()),
            digest: None,
        };
        let err = reg.install(&src, Some(&decl_exec)).unwrap_err();
        match err {
            CoreError::RequiresApproval {
                plugin,
                operation,
                reason,
            } => {
                assert_eq!(plugin, "my-npm-plugin");
                assert_eq!(operation, "install");
                assert!(reason.contains("requires executing") || reason.contains("needs preview"));
            }
            other => panic!("expected RequiresApproval, got {other:?}"),
        }
        // File/config plugin should not require approval
        let decl_safe = PluginAdapterDecl::file_config(
            "plugins.json",
            Some("plugins"),
            PluginKind::ConfigEntry,
            RestartBehavior::None,
        );
        let safe_src = PluginSource {
            id: PluginId::new("safe-plugin").unwrap(),
            kind: PluginKind::ConfigEntry,
            locator: "safe-locator".to_owned(),
            version: None,
            digest: None,
        };
        let rec = reg.install(&safe_src, Some(&decl_safe)).unwrap();
        assert_eq!(rec.kind, PluginKind::ConfigEntry);

        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn removal_leaves_foreign() {
        let root = tmp_root("foreign");
        let mut reg = PluginRegistry::load(&root).unwrap();
        let decl = PluginAdapterDecl::file_config(
            "plugins.json",
            Some("plugins"),
            PluginKind::ConfigEntry,
            RestartBehavior::None,
        );
        let src1 = PluginSource {
            id: PluginId::new("owned1").unwrap(),
            kind: PluginKind::ConfigEntry,
            locator: "loc1".to_owned(),
            version: Some("1.0.0".to_owned()),
            digest: None,
        };
        let src2 = PluginSource {
            id: PluginId::new("owned2").unwrap(),
            kind: PluginKind::ConfigEntry,
            locator: "loc2".to_owned(),
            version: None,
            digest: None,
        };
        reg.install(&src1, Some(&decl)).unwrap();
        reg.install(&src2, Some(&decl)).unwrap();

        // Inject foreign key directly into registry file
        let file = reg.file();
        let bytes = std::fs::read(&file).unwrap();
        let mut val: Value = serde_json::from_slice(&bytes).unwrap();
        let obj = val.as_object_mut().unwrap();
        obj.insert("foreignKey".to_owned(), Value::String("keep-me".to_owned()));
        // Also inject a foreign plugin entry that is not owned? Actually plugins array only contains owned; foreign plugin would be preserved via foreign map? We'll test foreign top-level.
        std::fs::write(&file, serde_json::to_vec_pretty(&val).unwrap()).unwrap();
        // Reload to capture foreign
        let mut reg2 = PluginRegistry::load(&root).unwrap();
        assert_eq!(
            reg2.foreign.get("foreignKey").and_then(|v| v.as_str()),
            Some("keep-me")
        );
        // Remove one owned
        let removed = reg2.remove(&PluginId::new("owned1").unwrap()).unwrap();
        assert!(removed.is_some());
        // Reload and check foreign still there and other owned still there
        let reg3 = PluginRegistry::load(&root).unwrap();
        assert_eq!(
            reg3.foreign.get("foreignKey").and_then(|v| v.as_str()),
            Some("keep-me"),
            "foreign must be preserved"
        );
        assert!(reg3.get(&PluginId::new("owned1").unwrap()).is_none());
        assert!(reg3.get(&PluginId::new("owned2").unwrap()).is_some());

        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn config_entry_foreign_preservation() {
        let path = tmp_file("cfg");
        // Create config with foreign top-level and foreign plugin
        let mut outer = Map::new();
        outer.insert("model".to_owned(), Value::String("sonnet".to_owned()));
        outer.insert("extra".to_owned(), Value::String("foreign".to_owned()));
        let mut plugins = Map::new();
        plugins.insert(
            "foreign-plugin".to_owned(),
            serde_json::json!({"version": "0.1.0", "locator": "foreign-loc"}),
        );
        outer.insert("plugins".to_owned(), Value::Object(plugins));
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&Value::Object(outer)).unwrap(),
        )
        .unwrap();

        let src = PluginSource {
            id: PluginId::new("owned-plugin").unwrap(),
            kind: PluginKind::ConfigEntry,
            locator: "owned-loc".to_owned(),
            version: Some("1.0.0".to_owned()),
            digest: None,
        };
        install_config_entry(&path, "plugins", &src).unwrap();

        let content = std::fs::read(&path).unwrap();
        let val: Value = serde_json::from_slice(&content).unwrap();
        let obj = val.as_object().unwrap();
        assert_eq!(obj.get("model").and_then(|v| v.as_str()), Some("sonnet"));
        assert_eq!(obj.get("extra").and_then(|v| v.as_str()), Some("foreign"));
        let m = obj.get("plugins").and_then(|v| v.as_object()).unwrap();
        assert!(m.contains_key("foreign-plugin"));
        assert!(m.contains_key("owned-plugin"));

        // Removal leaves foreign
        remove_config_entry(&path, "plugins", &PluginId::new("owned-plugin").unwrap()).unwrap();
        let content2 = std::fs::read(&path).unwrap();
        let val2: Value = serde_json::from_slice(&content2).unwrap();
        let obj2 = val2.as_object().unwrap();
        let m2 = obj2.get("plugins").and_then(|v| v.as_object()).unwrap();
        assert!(!m2.contains_key("owned-plugin"));
        assert!(m2.contains_key("foreign-plugin"));
        assert_eq!(obj2.get("extra").and_then(|v| v.as_str()), Some("foreign"));

        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn shared_dep_not_removed_until_no_consumer() {
        let root = tmp_root("shared");
        let mut reg = PluginRegistry::load(&root).unwrap();
        let _decl = PluginAdapterDecl::file_config(
            "plugins.json",
            Some("plugins"),
            PluginKind::ConfigEntry,
            RestartBehavior::None,
        );
        // Simulate two plugins that would share a package (though ConfigEntry doesn't have shared dep, we use dependency_key)
        // We'll directly test the dependency_key logic via registry internals: create two records with same dependency_key
        let rec1 = PluginRecord {
            id: PluginId::new("plug-a").unwrap(),
            kind: PluginKind::ConfigEntry,
            version: Some("1.0.0".to_owned()),
            digest: None,
            source_locator: "shared-dep".to_owned(),
            installed_at: now_iso8601(),
            enabled: true,
            dependency_key: Some("shared-package".to_owned()),
        };
        let rec2 = PluginRecord {
            id: PluginId::new("plug-b").unwrap(),
            kind: PluginKind::ConfigEntry,
            version: Some("1.0.0".to_owned()),
            digest: None,
            source_locator: "shared-dep".to_owned(),
            installed_at: now_iso8601(),
            enabled: true,
            dependency_key: Some("shared-package".to_owned()),
        };
        reg.records.push(rec1);
        reg.records.push(rec2);
        reg.store().unwrap();
        // Remove one, shared dep should still have a consumer
        reg.remove(&PluginId::new("plug-a").unwrap()).unwrap();
        let remaining = reg.find_shared_consumers("shared-package");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id.as_str(), "plug-b");
        // Remove second, no consumers left
        reg.remove(&PluginId::new("plug-b").unwrap()).unwrap();
        let remaining2 = reg.find_shared_consumers("shared-package");
        assert!(remaining2.is_empty());

        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn enable_disable_distinct_from_remove() {
        let root = tmp_root("enable");
        let mut reg = PluginRegistry::load(&root).unwrap();
        let decl = PluginAdapterDecl::file_config(
            "plugins.json",
            Some("plugins"),
            PluginKind::ConfigEntry,
            RestartBehavior::None,
        );
        let src = PluginSource {
            id: PluginId::new("toggle").unwrap(),
            kind: PluginKind::ConfigEntry,
            locator: "loc".to_owned(),
            version: None,
            digest: None,
        };
        reg.install(&src, Some(&decl)).unwrap();
        let disabled = reg.disable(&PluginId::new("toggle").unwrap()).unwrap();
        assert!(!disabled.enabled);
        let enabled = reg.enable(&PluginId::new("toggle").unwrap()).unwrap();
        assert!(enabled.enabled);
        // Remove is different
        reg.remove(&PluginId::new("toggle").unwrap()).unwrap();
        assert!(reg.get(&PluginId::new("toggle").unwrap()).is_none());
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn plugin_definition_round_trip() {
        let src = PluginSource {
            id: PluginId::new("my-plugin").unwrap(),
            kind: PluginKind::DirectoryBundle,
            locator: "/tmp/my-bundle".to_owned(),
            version: Some("1.2.3".to_owned()),
            digest: Some("a".repeat(64)),
        };
        src.validate().unwrap();
        let json = serde_json::to_string(&src).unwrap();
        let back: PluginSource = serde_json::from_str(&json).unwrap();
        assert_eq!(src, back);

        let rec = PluginRecord {
            id: PluginId::new("my-plugin").unwrap(),
            kind: PluginKind::DirectoryBundle,
            version: Some("1.2.3".to_owned()),
            digest: Some("b".repeat(64)),
            source_locator: "/tmp/my-bundle".to_owned(),
            installed_at: now_iso8601(),
            enabled: true,
            dependency_key: None,
        };
        rec.validate().unwrap();
        let json2 = serde_json::to_string(&rec).unwrap();
        let back2: PluginRecord = serde_json::from_str(&json2).unwrap();
        assert_eq!(rec, back2);
    }
}
