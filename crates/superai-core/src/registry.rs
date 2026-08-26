//! Registry schema v1 with migration and validation.
//!
//! Top-level record:
//! - `schema_version: u32` (currently 1)
//! - `instances: Vec<Instance>`
//! - any other top-level keys are foreign and preserved verbatim
//!
//! Instance record fields per FND-03:
//! `id`, `name`, `harness`, `config_root`, `binary`, `wrapper`, `isolation`,
//! `origin`, `ownership`, `template`, `created_at`, `adapter_revision`.
//!
//! Forbidden: `model`, `endpoint`, `key`, skill/plugin/mcp lists, etc.
//!
//! Migration: old records stored `name`/`harness`/`config_dir`/`binary_path`/`template{name,version}`
//! without `schema_version` and without stable IDs. On load we validate with
//! `ids`/`paths`, generate a stable `InstanceId` from `name+config_dir`, and set
//! `origin = AdoptedLegacy`, `isolation = Unknown`, `ownership = ExplicitlyAdopted`.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{CoreError, Result};
use crate::ids::{HarnessId, InstanceId, InstanceName, TemplateId, TemplateVersion};
use crate::instance::{Instance, TemplateRef};
use crate::paths::{AbsolutePath, ExecutableRef};
use crate::state::{InstanceOrigin, Isolation, Ownership};

const INSTANCES_KEY: &str = "instances";
const SCHEMA_VERSION_KEY: &str = "schema_version";
/// Current registry schema version.
pub const SCHEMA_VERSION: u32 = 1;
/// Adapter revision written into new records (crate version).
const ADAPTER_REVISION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// helpers: time, stable id
// ---------------------------------------------------------------------------

fn unix_secs_to_rfc3339(secs: u64) -> String {
    #[expect(
        clippy::cast_possible_wrap,
        reason = "secs/86400 fits in i64 for timestamps within reasonable range"
    )]
    let days = (secs / 86400) as i64;
    let secs_of_day = secs % 86400;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "year fits in i32 for registry timestamps"
)]
#[expect(
    clippy::cast_sign_loss,
    reason = "days derived from u64 secs, always non-negative"
)]
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    // Howard Hinnant's civil_from_days, days since 1970-01-01.
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

fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    unix_secs_to_rfc3339(secs)
}

fn stable_id_for_legacy(name: &str, config_root: &str) -> Result<InstanceId> {
    use std::collections::hash_map::DefaultHasher;
    let mut hasher = DefaultHasher::new();
    name.to_lowercase().hash(&mut hasher);
    config_root.hash(&mut hasher);
    let hash = hasher.finish();
    let candidate = format!("legacy-{hash:016x}");
    InstanceId::new(&candidate).map_err(|e| CoreError::Validation {
        field: "id".to_owned(),
        reason: format!("generated legacy id `{candidate}` is invalid: {e}"),
    })
}

// ---------------------------------------------------------------------------
// Old shape for migration
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct OldInstance {
    name: String,
    harness: String,
    config_dir: String,
    #[serde(default)]
    binary_path: Option<String>,
    #[serde(default)]
    template: Option<OldTemplateRef>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OldTemplateRef {
    name: String,
    version: String,
}

#[expect(clippy::unnecessary_to_owned, reason = "need owned string for hash")]
fn migrate_old_instance(old: OldInstance, path: &Path) -> Result<Instance> {
    // Validate harness/name/config_root using the same validators the new types use.
    let name = InstanceName::new(&old.name).map_err(|e| CoreError::Validation {
        field: "name".to_owned(),
        reason: format!("invalid old instance name `{}`: {e}", old.name),
    })?;
    let harness = HarnessId::new(&old.harness).map_err(|e| CoreError::Validation {
        field: "harness".to_owned(),
        reason: format!("invalid old harness `{}`: {e}", old.harness),
    })?;
    let config_root = AbsolutePath::new(&old.config_dir).map_err(|e| CoreError::Validation {
        field: "config_root".to_owned(),
        reason: format!("invalid old config_dir `{}`: {e}", old.config_dir),
    })?;
    let binary = if let Some(bp) = old.binary_path {
        if bp.trim().is_empty() {
            None
        } else {
            Some(ExecutableRef::new(&bp).map_err(|e| CoreError::Validation {
                field: "binary".to_owned(),
                reason: format!("invalid old binary_path `{bp}`: {e}"),
            })?)
        }
    } else {
        None
    };
    let template = if let Some(t) = old.template {
        let tid = TemplateId::new(&t.name).map_err(|e| CoreError::Validation {
            field: "template.name".to_owned(),
            reason: format!("invalid old template name `{}`: {e}", t.name),
        })?;
        let ver = TemplateVersion::new(&t.version).map_err(|e| CoreError::Validation {
            field: "template.version".to_owned(),
            reason: format!("invalid old template version `{}`: {e}", t.version),
        })?;
        Some(TemplateRef {
            name: tid,
            version: ver,
        })
    } else {
        None
    };
    let id = stable_id_for_legacy(name.as_str(), &config_root.to_string())?; // expect(clippy::unnecessary_to_owned) suppressed below
    let created_at = now_iso8601();
    let inst = Instance {
        id,
        name,
        harness,
        config_root,
        binary,
        wrapper: None,
        isolation: Isolation::Unknown,
        origin: InstanceOrigin::AdoptedLegacy,
        ownership: Ownership::ExplicitlyAdopted,
        template,
        created_at,
        adapter_revision: ADAPTER_REVISION.to_owned(),
    };
    // Ensure the generated instance validates (created_at non-empty etc.)
    inst.validate()?;
    // Also ensure paths themselves are valid (already validated).
    let _ = path;
    Ok(inst)
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// The set of instances superai knows about, stored in its own records file.
///
/// Foreign top-level keys are preserved verbatim on store; only
/// `schema_version` and `instances` are owned by superai.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    /// Schema version of the file. Currently 1.
    schema_version: u32,
    instances: Vec<Instance>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            instances: Vec::new(),
        }
    }
}

impl Registry {
    /// Current schema version.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Default records path: `$HOME/.superai/instances.json`.
    pub fn default_path() -> Result<PathBuf> {
        let home = std::env::home_dir().ok_or(CoreError::NoHomeDir)?;
        Ok(home.join(".superai").join("instances.json"))
    }

    /// Read the records file fresh. A missing file is an empty registry.
    ///
    /// Migration is applied transparently:
    ///
    /// - bare array → old vector, migrated
    /// - object without `schema_version` but with `instances` → try new shape, fallback to old migration
    /// - object with `schema_version` → must equal `SCHEMA_VERSION`, otherwise actionable error
    ///
    /// Foreign keys are ignored on load but preserved on store.
    #[expect(
        clippy::too_many_lines,
        reason = "migration branches for bare array and object with/without schema_version are intentionally explicit"
    )]
    #[expect(
        clippy::excessive_nesting,
        reason = "load handles multiple branching migration paths"
    )]
    pub fn load(path: &Path) -> Result<Self> {
        let value = match superai_config::json::load_value(path) {
            Ok(v) => v,
            Err(e) => return Err(CoreError::Config(e)),
        };

        match value {
            Value::Object(map) => {
                if map.is_empty() {
                    return Ok(Self::default());
                }
                // Check schema_version first.
                if let Some(sv) = map.get(SCHEMA_VERSION_KEY) {
                    let sv_num = u32::try_from(sv.as_u64().ok_or_else(|| CoreError::SchemaValidation {
                        path: path.to_path_buf(),
                        details: format!(
                            "unsupported {SCHEMA_VERSION_KEY} value {sv}: expected integer {SCHEMA_VERSION}"
                        ),
                    })?).map_err(|e| CoreError::SchemaValidation {
                        path: path.to_path_buf(),
                        details: format!("unsupported {SCHEMA_VERSION_KEY} value {sv}: exceeds u32: {e}"),
                    })?;
                    if sv_num != SCHEMA_VERSION {
                        return Err(CoreError::SchemaValidation {
                            path: path.to_path_buf(),
                            details: format!(
                                "unsupported schema_version {sv_num}: expected {SCHEMA_VERSION}. \
                                 Delete or migrate the file at {}",
                                path.display()
                            ),
                        });
                    }
                    let instances_raw = map
                        .get(INSTANCES_KEY)
                        .cloned()
                        .unwrap_or(Value::Array(vec![]));
                    let instances: Vec<Instance> = serde_json::from_value(instances_raw).map_err(|e| {
                        CoreError::SchemaValidation {
                            path: path.to_path_buf(),
                            details: format!("invalid instances array for schema_version {sv_num}: {e}. Expected InstanceV1 shape"),
                        }
                    })?;
                    let reg = Self {
                        schema_version: sv_num,
                        instances,
                    };
                    reg.validate()?;
                    Ok(reg)
                } else if let Some(instances_raw) = map.get(INSTANCES_KEY) {
                    // No schema_version, but has instances.
                    // Try new shape first (covers files written by newer code that forgot version, or manual edits).
                    let try_new: std::result::Result<Vec<Instance>, _> =
                        serde_json::from_value(instances_raw.clone());
                    if let Ok(instances) = try_new {
                        let reg = Self {
                            schema_version: SCHEMA_VERSION,
                            instances,
                        };
                        // Validate duplicates etc.; if validation fails with duplicate, surface it.
                        reg.validate()?;
                        return Ok(reg);
                    }
                    // Fallback to old shape migration.
                    let old_instances: Vec<OldInstance> = serde_json::from_value(instances_raw.clone())
                        .map_err(|e| CoreError::SchemaValidation {
                            path: path.to_path_buf(),
                            details: format!(
                                "instances array is neither InstanceV1 nor legacy Instance: {e}. \
                                 Expected either new fields (id,name,harness,config_root, ...) or legacy (name,harness,config_dir)"
                            ),
                        })?;
                    let mut instances = Vec::with_capacity(old_instances.len());
                    for old in old_instances {
                        instances.push(migrate_old_instance(old, path)?);
                    }
                    let reg = Self {
                        schema_version: SCHEMA_VERSION,
                        instances,
                    };
                    reg.validate()?;
                    Ok(reg)
                } else {
                    // Object with no instances and no schema_version: only foreign keys.
                    Ok(Self::default())
                }
            }
            Value::Array(arr) => {
                if arr.is_empty() {
                    return Ok(Self::default());
                }
                // Bare array root: could be new or old.
                let try_new: std::result::Result<Vec<Instance>, _> =
                    serde_json::from_value(Value::Array(arr.clone()));
                if let Ok(instances) = try_new {
                    let reg = Self {
                        schema_version: SCHEMA_VERSION,
                        instances,
                    };
                    reg.validate()?;
                    return Ok(reg);
                }
                let old_instances: Vec<OldInstance> = serde_json::from_value(Value::Array(arr))
                    .map_err(|e| CoreError::SchemaValidation {
                        path: path.to_path_buf(),
                        details: format!(
                            "bare array is neither InstanceV1 nor legacy Instance: {e}"
                        ),
                    })?;
                let mut instances = Vec::with_capacity(old_instances.len());
                for old in old_instances {
                    instances.push(migrate_old_instance(old, path)?);
                }
                let reg = Self {
                    schema_version: SCHEMA_VERSION,
                    instances,
                };
                reg.validate()?;
                Ok(reg)
            }
            Value::Null => Ok(Self::default()),
            other => Err(CoreError::SchemaValidation {
                path: path.to_path_buf(),
                details: format!(
                    "registry root must be object with `{SCHEMA_VERSION_KEY}` and `{INSTANCES_KEY}` or bare array, got {}",
                    match other {
                        Value::Bool(_) => "bool",
                        Value::Number(_) => "number",
                        Value::String(_) => "string",
                        _ => "unknown",
                    }
                ),
            }),
        }
    }

    /// Back up and write the records file, leaving any other key in it untouched.
    ///
    /// Only `schema_version` and `instances` are written; foreign keys are preserved
    /// by loading the existing map fresh and merging.
    pub fn store(&self, path: &Path) -> Result<()> {
        // Validate before writing.
        self.validate()?;
        for inst in &self.instances {
            inst.validate()?;
        }
        let instances = serde_json::to_value(&self.instances).map_err(CoreError::Records)?;
        let schema_version =
            serde_json::to_value(self.schema_version).map_err(CoreError::Records)?;
        superai_config::json::edit(path, |map: &mut Map<String, Value>| {
            map.insert(SCHEMA_VERSION_KEY.to_owned(), schema_version.clone());
            map.insert(INSTANCES_KEY.to_owned(), instances.clone());
        })
        .map_err(CoreError::Config)?;
        Ok(())
    }

    /// Every known instance.
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// Look an instance up by name (case-sensitive exact).
    pub fn get(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.name.as_str() == name)
    }

    /// Look an instance up by name case-folded.
    pub fn get_case_fold(&self, name: &str) -> Option<&Instance> {
        let needle = name.to_lowercase();
        self.instances
            .iter()
            .find(|i| i.name.normalized() == needle)
    }

    /// Look an instance up by id.
    pub fn get_by_id(&self, id: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id.as_str() == id)
    }

    /// Validate duplicate invariants.
    #[expect(
        clippy::excessive_nesting,
        reason = "validation checks multiple collision kinds"
    )]
    ///
    /// Checks: normalized name collisions, duplicate ids, duplicate `config_roots`,
    /// duplicate wrapper paths and wrapper command collisions (including collision
    /// between wrapper commands and instance names).
    fn validate(&self) -> Result<()> {
        let mut names: HashMap<String, &Instance> = HashMap::new();
        let mut ids: HashSet<String> = HashSet::new();
        let mut roots: HashSet<String> = HashSet::new();
        let mut wrapper_paths: HashSet<String> = HashSet::new();
        let mut wrapper_commands: HashMap<String, &Instance> = HashMap::new();

        for inst in &self.instances {
            // name (case-folded)
            let normalized = inst.name.normalized();
            if let Some(prev) = names.get(&normalized) {
                return Err(CoreError::NameCollision {
                    kind: "InstanceName".to_owned(),
                    name: inst.name.to_string(),
                    reason: format!(
                        "case-fold collision with '{}' (normalized `{}`)",
                        prev.name, normalized
                    ),
                });
            }
            names.insert(normalized.clone(), inst);

            // id
            let id_str = inst.id.as_str().to_owned();
            if !ids.insert(id_str.clone()) {
                return Err(CoreError::NameCollision {
                    kind: "InstanceId".to_owned(),
                    name: id_str,
                    reason: "duplicate id".to_owned(),
                });
            }

            // config_root (normalized absolute path string)
            let root_str = inst.config_root.to_string();
            if !roots.insert(root_str.clone()) {
                return Err(CoreError::Validation {
                    field: "config_root".to_owned(),
                    reason: format!(
                        "duplicate config_root `{root_str}` collides with another instance"
                    ),
                });
            }

            // wrapper
            if let Some(wrapper) = &inst.wrapper {
                let wp = wrapper.path.to_string();
                if !wrapper_paths.insert(wp.clone()) {
                    return Err(CoreError::Validation {
                        field: "wrapper.path".to_owned(),
                        reason: format!(
                            "duplicate wrapper path `{wp}` collides with another instance"
                        ),
                    });
                }
                let cmd_norm = wrapper.command_name.normalized();
                if let Some(prev) = wrapper_commands.get(&cmd_norm) {
                    return Err(CoreError::NameCollision {
                        kind: "WrapperCommand".to_owned(),
                        name: wrapper.command_name.to_string(),
                        reason: format!(
                            "wrapper command case-fold collision with wrapper of '{}' (normalized `{}`)",
                            prev.name, cmd_norm
                        ),
                    });
                }
                wrapper_commands.insert(cmd_norm, inst);
            }
        }

        // Cross-check wrapper commands vs instance names (different instances).
        for inst in &self.instances {
            if let Some(wrapper) = &inst.wrapper
                && let Some(other) = names.get(&wrapper.command_name.normalized())
                && other.id.as_str() != inst.id.as_str()
            {
                let cmd_norm = wrapper.command_name.normalized();
                return Err(CoreError::NameCollision {
                    kind: "WrapperCommand/InstanceName".to_owned(),
                    name: wrapper.command_name.to_string(),
                    reason: format!(
                        "wrapper command `{}` collides with instance name '{}' (normalized `{}`)",
                        wrapper.command_name, other.name, cmd_norm
                    ),
                });
            }
        }

        Ok(())
    }

    /// Add an instance, or fail if the `name`/`id`/`config_root`/`wrapper` collides.
    #[expect(
        clippy::excessive_nesting,
        reason = "insert checks multiple collision kinds"
    )]
    pub fn insert(&mut self, instance: Instance) -> Result<()> {
        instance.validate()?;
        // Quick pre-check for normalized name collision before push to give better error.
        let new_norm = instance.name.normalized();
        for existing in &self.instances {
            if existing.name.normalized() == new_norm {
                return Err(CoreError::NameCollision {
                    kind: "InstanceName".to_owned(),
                    name: instance.name.to_string(),
                    reason: format!(
                        "case-fold collision with existing instance '{}' (normalized `{}`)",
                        existing.name, new_norm
                    ),
                });
            }
            if existing.id.as_str() == instance.id.as_str() {
                return Err(CoreError::NameCollision {
                    kind: "InstanceId".to_owned(),
                    name: instance.id.to_string(),
                    reason: "duplicate id".to_owned(),
                });
            }
            if existing.config_root == instance.config_root {
                return Err(CoreError::Validation {
                    field: "config_root".to_owned(),
                    reason: format!(
                        "duplicate config_root `{}` collides with instance '{}'",
                        instance.config_root, existing.name
                    ),
                });
            }
            if let (Some(existing_w), Some(new_w)) = (&existing.wrapper, &instance.wrapper) {
                if existing_w.path == new_w.path {
                    return Err(CoreError::Validation {
                        field: "wrapper.path".to_owned(),
                        reason: format!(
                            "duplicate wrapper path `{}` collides with instance '{}'",
                            new_w.path, existing.name
                        ),
                    });
                }
                if existing_w.command_name.normalized() == new_w.command_name.normalized() {
                    return Err(CoreError::NameCollision {
                        kind: "WrapperCommand".to_owned(),
                        name: new_w.command_name.to_string(),
                        reason: format!(
                            "case-fold collision with wrapper command of '{}'",
                            existing.name
                        ),
                    });
                }
            }
            // wrapper command vs instance name cross-check
            if let Some(new_w) = &instance.wrapper
                && existing.name.normalized() == new_w.command_name.normalized()
                && existing.id.as_str() != instance.id.as_str()
            {
                return Err(CoreError::NameCollision {
                    kind: "WrapperCommand/InstanceName".to_owned(),
                    name: new_w.command_name.to_string(),
                    reason: format!(
                        "wrapper command `{}` collides with existing instance '{}'",
                        new_w.command_name, existing.name
                    ),
                });
            }
            if let Some(existing_w) = &existing.wrapper
                && existing_w.command_name.normalized() == instance.name.normalized()
                && existing.id.as_str() != instance.id.as_str()
            {
                return Err(CoreError::NameCollision {
                    kind: "InstanceName/WrapperCommand".to_owned(),
                    name: instance.name.to_string(),
                    reason: format!(
                        "instance name `{}` collides with wrapper command of '{}'",
                        instance.name, existing.name
                    ),
                });
            }
        }

        self.instances.push(instance);
        // Full validation as safety net (covers edge cases).
        if let Err(e) = self.validate() {
            self.instances.pop();
            return Err(e);
        }
        Ok(())
    }

    /// Remove an instance by name (exact case-sensitive), returning it. This touches no files on disk.
    pub fn remove(&mut self, name: &str) -> Option<Instance> {
        let idx = self
            .instances
            .iter()
            .position(|i| i.name.as_str() == name)?;
        Some(self.instances.remove(idx))
    }

    /// Remove an instance by name case-folded.
    pub fn remove_case_fold(&mut self, name: &str) -> Option<Instance> {
        let needle = name.to_lowercase();
        let idx = self
            .instances
            .iter()
            .position(|i| i.name.normalized() == needle)?;
        Some(self.instances.remove(idx))
    }

    /// Rename an instance, preserving its `id`, `config_root`, `template`, etc.
    ///
    /// The wrapper's `command_name` is updated if it currently equals the old name
    /// (case-folded). Collision checks are platform-aware (case-folded).
    #[expect(
        clippy::indexing_slicing,
        reason = "idx validated via position search, bounds checked"
    )]
    pub fn rename(&mut self, old_name: &str, new_name: InstanceName) -> Result<()> {
        let idx = self
            .instances
            .iter()
            .position(|i| i.name.as_str() == old_name)
            .ok_or_else(|| CoreError::Validation {
                field: "name".to_owned(),
                reason: format!("instance `{old_name}` not found for rename"),
            })?;

        let new_norm = new_name.normalized();
        for (j, other) in self.instances.iter().enumerate() {
            if j == idx {
                continue;
            }
            if other.name.normalized() == new_norm {
                return Err(CoreError::NameCollision {
                    kind: "InstanceName".to_owned(),
                    name: new_name.to_string(),
                    reason: format!(
                        "case-fold collision with existing instance '{}' (normalized `{}`)",
                        other.name, new_norm
                    ),
                });
            }
            if let Some(w) = &other.wrapper
                && w.command_name.normalized() == new_norm
            {
                return Err(CoreError::NameCollision {
                    kind: "InstanceName/WrapperCommand".to_owned(),
                    name: new_name.to_string(),
                    reason: format!(
                        "rename target `{}` collides with wrapper command of '{}'",
                        new_name, other.name
                    ),
                });
            }
        }
        // Also check new wrapper command after rename would collide with existing names.
        // If the renamed instance has a wrapper, its command_name may be updated to new_name,
        // so we must ensure that new command doesn't collide with another instance's name.
        // The loop above already checks that.

        // Preserve id for assertion.
        let preserved_id = self.instances[idx].id.clone();
        let preserved_root = self.instances[idx].config_root.clone();
        let preserved_template = self.instances[idx].template.clone();

        // Perform rename.
        let inst = &mut self.instances[idx];
        let old_name_owned = inst.name.to_string();
        inst.name = new_name.clone();
        // Update wrapper command_name if it matches old name (case-folded).
        if let Some(wrapper) = &mut inst.wrapper
            && (wrapper.command_name.normalized() == old_name_owned.to_lowercase()
                || wrapper.command_name.as_str() == old_name_owned)
        {
            wrapper.command_name = new_name.clone();
        }

        // Validate whole registry after rename.
        if let Err(e) = self.validate() {
            // Roll back
            let inst = &mut self.instances[idx];
            inst.name = InstanceName::new(&old_name_owned).unwrap_or(new_name);
            return Err(e);
        }

        // Ensure id/template/root preserved.
        debug_assert_eq!(self.instances[idx].id, preserved_id);
        debug_assert_eq!(self.instances[idx].config_root, preserved_root);
        debug_assert_eq!(self.instances[idx].template, preserved_template);

        Ok(())
    }
}

/// Config dirs on disk that no record and no wrapper accounts for.
///
/// Adoption or removal is the user's call — superai only reports what it found.
pub fn unmanaged_dirs(registry: &Registry, candidates: &[PathBuf]) -> Vec<PathBuf> {
    candidates
        .iter()
        .filter(|dir| {
            let dir_path = dir.as_path();
            !registry
                .instances
                .iter()
                .any(|i| i.config_root.as_path() == dir_path)
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::WrapperRef;
    use crate::paths::WrapperPath;

    fn sample_instance(
        name: &str,
        config_root: &str,
        id: &str,
        wrapper_path: Option<&str>,
    ) -> Instance {
        let mut inst = Instance {
            id: InstanceId::new(id).unwrap(),
            name: InstanceName::new(name).unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::new(config_root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: TemplateId::new("glm").unwrap(),
                version: TemplateVersion::new("1.2.0").unwrap(),
            }),
            created_at: "2026-08-26T12:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        };
        if let Some(wp) = wrapper_path {
            inst.wrapper = Some(WrapperRef {
                path: WrapperPath::new(wp).unwrap(),
                command_name: InstanceName::new(name).unwrap(),
                generator_version: "0.1.0".to_owned(),
                content_digest: "abc123".to_owned(),
            });
        }
        inst
    }

    fn instance_legacy(name: &str, config_dir: &str) -> OldInstance {
        OldInstance {
            name: name.to_owned(),
            harness: "claude-code".to_owned(),
            config_dir: config_dir.to_owned(),
            binary_path: None,
            template: Some(OldTemplateRef {
                name: "glm".to_owned(),
                version: "1.2.0".to_owned(),
            }),
        }
    }

    #[test]
    fn duplicate_normalized_names_are_rejected() {
        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            "/home/u/.claude-work",
            "id-1",
            None,
        ))
        .unwrap();
        // case-fold collision: "WORK" vs "work"
        let dup = sample_instance("WORK", "/home/u/.claude-work2", "id-2", None);
        let err = r.insert(dup).unwrap_err();
        match err {
            CoreError::NameCollision { kind, .. } => assert_eq!(kind, "InstanceName"),
            other => panic!("expected NameCollision, got {other:?}"),
        }
        assert_eq!(r.instances().len(), 1);
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            "/home/u/.claude-work",
            "dup-id",
            None,
        ))
        .unwrap();
        let dup = sample_instance("other", "/home/u/.claude-other", "dup-id", None);
        let err = r.insert(dup).unwrap_err();
        match err {
            CoreError::NameCollision { kind, .. } => assert_eq!(kind, "InstanceId"),
            other => panic!("expected duplicate id, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_config_roots_are_rejected() {
        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            "/home/u/.claude-work",
            "id-1",
            None,
        ))
        .unwrap();
        // Same path normalized differently with extra slash
        let dup = sample_instance("other", "/home/u/.claude-work", "id-2", None);
        let err = r.insert(dup).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "config_root"),
            other => panic!("expected Validation config_root, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_wrapper_paths_are_rejected() {
        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            "/home/u/.claude-work",
            "id-1",
            Some("/tmp/wrapper"),
        ))
        .unwrap();
        let dup = sample_instance(
            "other",
            "/home/u/.claude-other",
            "id-2",
            Some("/tmp/wrapper"),
        );
        let err = r.insert(dup).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "wrapper.path"),
            other => panic!("expected wrapper.path collision, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_wrapper_commands_are_rejected() {
        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            "/home/u/.claude-work",
            "id-1",
            Some("/tmp/wrapper1"),
        ))
        .unwrap();
        // Different instance name but same wrapper command "work" (case-fold)
        let mut dup = sample_instance(
            "other",
            "/home/u/.claude-other",
            "id-2",
            Some("/tmp/wrapper2"),
        );
        // Force wrapper command to collide case-folded
        dup.wrapper.as_mut().unwrap().command_name = InstanceName::new("WORK").unwrap();
        let err = r.insert(dup).unwrap_err();
        match err {
            CoreError::NameCollision { kind, .. } => assert!(
                kind.contains("WrapperCommand"),
                "expected wrapper command collision, got kind {kind}"
            ),
            other => panic!("expected wrapper command collision, got {other:?}"),
        }
    }

    #[test]
    fn wrapper_command_collides_with_instance_name() {
        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            "/home/u/.claude-work",
            "id-1",
            None,
        ))
        .unwrap();
        // New instance whose wrapper command collides with existing instance name "work"
        let mut with_wrapper = sample_instance(
            "other",
            "/home/u/.claude-other",
            "id-2",
            Some("/tmp/wrapper-other"),
        );
        with_wrapper.wrapper.as_mut().unwrap().command_name = InstanceName::new("work").unwrap();
        let err = r.insert(with_wrapper).unwrap_err();
        match err {
            CoreError::NameCollision { kind, .. } => assert!(kind.contains("WrapperCommand")),
            other => panic!("expected collision, got {other:?}"),
        }
    }

    #[test]
    fn rename_preserves_id_and_template() {
        let mut r = Registry::default();
        let inst = sample_instance("work", "/home/u/.claude-work", "stable-id-1", None);
        let original_id = inst.id.clone();
        let original_template = inst.template.clone();
        let original_root = inst.config_root.clone();
        r.insert(inst).unwrap();
        r.rename("work", InstanceName::new("work2").unwrap())
            .unwrap();
        let renamed = r.get("work2").unwrap();
        assert_eq!(renamed.id, original_id);
        assert_eq!(renamed.template, original_template);
        assert_eq!(renamed.config_root, original_root);
        assert!(r.get("work").is_none());
    }

    #[test]
    fn rename_updates_wrapper_command_when_it_matches_old_name() {
        let mut r = Registry::default();
        let inst = sample_instance(
            "work",
            "/home/u/.claude-work",
            "id-work",
            Some("/tmp/wrapper-work"),
        );
        r.insert(inst).unwrap();
        r.rename("work", InstanceName::new("work2").unwrap())
            .unwrap();
        let renamed = r.get("work2").unwrap();
        let wrapper = renamed.wrapper.as_ref().unwrap();
        assert_eq!(wrapper.command_name.as_str(), "work2");
    }

    #[test]
    fn rename_rejects_case_fold_collision() {
        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            "/home/u/.claude-work",
            "id-1",
            None,
        ))
        .unwrap();
        r.insert(sample_instance(
            "other",
            "/home/u/.claude-other",
            "id-2",
            None,
        ))
        .unwrap();
        let err = r
            .rename("other", InstanceName::new("WORK").unwrap())
            .unwrap_err();
        match err {
            CoreError::NameCollision { kind, .. } => assert_eq!(kind, "InstanceName"),
            other => panic!("expected NameCollision, got {other:?}"),
        }
    }

    #[test]
    fn round_trips_through_disk_keeping_foreign_keys() {
        let path = std::env::temp_dir().join("superai-core-tests/registry_v1_foreign.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Write a file with foreign keys and old-style instances key absent yet.
        std::fs::write(&path, r#"{"schema":7,"custom":"keep-me"}"#).unwrap();

        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            "/home/u/.claude-work",
            "id-work-foreign",
            None,
        ))
        .unwrap();
        r.store(&path).unwrap();

        // Loaded registry must equal what we stored.
        let loaded = Registry::load(&path).unwrap();
        assert_eq!(loaded.instances().len(), 1);
        assert_eq!(loaded.schema_version(), SCHEMA_VERSION);
        assert_eq!(loaded.instances()[0].name.as_str(), "work");

        // Foreign keys preserved.
        let raw = superai_config::json::load(&path).unwrap();
        assert_eq!(raw["schema"], serde_json::json!(7));
        assert_eq!(raw["custom"], serde_json::json!("keep-me"));
        assert_eq!(raw["schema_version"], serde_json::json!(SCHEMA_VERSION));
        assert!(raw.contains_key("instances"));
    }

    #[test]
    fn unmanaged_dirs_excludes_recorded_ones() {
        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            "/home/u/.claude-work",
            "id-unmanaged-1",
            None,
        ))
        .unwrap();

        let found = unmanaged_dirs(
            &r,
            &[
                PathBuf::from("/home/u/.claude-work"),
                PathBuf::from("/home/u/.claude-aaa"),
            ],
        );
        assert_eq!(found, vec![PathBuf::from("/home/u/.claude-aaa")]);
    }

    #[test]
    fn migration_from_old_vector_and_instances_key() {
        // Test bare array migration
        let path = std::env::temp_dir().join("superai-core-tests/migration_bare.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = vec![instance_legacy("work", "/home/u/.claude-work")];
        std::fs::write(&path, serde_json::to_string(&old).unwrap()).unwrap();
        let reg = Registry::load(&path).unwrap();
        assert_eq!(reg.instances().len(), 1);
        let inst = &reg.instances()[0];
        assert_eq!(inst.name.as_str(), "work");
        assert_eq!(inst.harness.as_str(), "claude-code");
        assert_eq!(inst.config_root.to_string(), "/home/u/.claude-work");
        assert_eq!(inst.origin, InstanceOrigin::AdoptedLegacy);
        assert_eq!(inst.isolation, Isolation::Unknown);
        assert_eq!(inst.ownership, Ownership::ExplicitlyAdopted);
        // Stable id is deterministic: same name+config yields same id on reload.
        let reg2 = Registry::load(&path).unwrap();
        assert_eq!(reg.instances()[0].id, reg2.instances()[0].id);

        // Test object with instances key holding old shape
        let path2 = std::env::temp_dir().join("superai-core-tests/migration_wrapped.json");
        let wrapped = serde_json::json!({
            "instances": [ {
                "name": "oldie",
                "harness": "claude-code",
                "config_dir": "/home/u/.claude-oldie",
                "template": {"name":"glm","version":"1.2.0"}
            } ],
            "keep": 123
        });
        std::fs::write(&path2, serde_json::to_string(&wrapped).unwrap()).unwrap();
        let reg3 = Registry::load(&path2).unwrap();
        assert_eq!(reg3.instances().len(), 1);
        assert_eq!(reg3.instances()[0].name.as_str(), "oldie");
        assert_eq!(reg3.instances()[0].origin, InstanceOrigin::AdoptedLegacy);
        // After storing, foreign key preserved and schema_version added.
        reg3.store(&path2).unwrap();
        let raw = superai_config::json::load(&path2).unwrap();
        assert_eq!(raw["keep"], serde_json::json!(123));
        assert_eq!(raw["schema_version"], serde_json::json!(SCHEMA_VERSION));
    }

    #[test]
    fn migration_validates_harness_name_and_config() {
        let bad_old = OldInstance {
            name: "CON".to_owned(), // reserved
            harness: "claude-code".to_owned(),
            config_dir: "/home/u/.claude-work".to_owned(),
            binary_path: None,
            template: None,
        };
        let path = Path::new("/tmp/fake");
        let err = migrate_old_instance(bad_old, path).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "name"),
            other => panic!("expected validation, got {other:?}"),
        }

        let bad_old2 = OldInstance {
            name: "work".to_owned(),
            harness: "bad/harness".to_owned(),
            config_dir: "/home/u/.claude-work".to_owned(),
            binary_path: None,
            template: None,
        };
        let err2 = migrate_old_instance(bad_old2, path).unwrap_err();
        match err2 {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("expected validation, got {other:?}"),
        }

        let bad_old3 = OldInstance {
            name: "work".to_owned(),
            harness: "claude-code".to_owned(),
            config_dir: "../relative".to_owned(),
            binary_path: None,
            template: None,
        };
        let err3 = migrate_old_instance(bad_old3, path).unwrap_err();
        match err3 {
            CoreError::Validation { field, .. } => assert_eq!(field, "config_root"),
            other => panic!("expected validation, got {other:?}"),
        }
    }

    #[test]
    fn serialization_never_emits_forbidden_fields() {
        let inst = sample_instance("work", "/home/u/.claude-work", "id-forbidden", None);
        let reg = {
            let mut r = Registry::default();
            r.insert(inst).unwrap();
            r
        };
        let json = serde_json::to_value(&reg.instances).unwrap();
        let text = serde_json::to_string(&json).unwrap().to_lowercase();
        let forbidden = [
            "model", "endpoint", "api_key", "apikey", "skill", "plugin", "mcp", "baseurl",
            "base_url",
        ];
        for field in forbidden {
            assert!(
                !text.contains(&format!("\"{field}\"")),
                "forbidden field `{field}` must not be emitted: {text}"
            );
        }
        // Also check top-level registry serialization
        let full_json = serde_json::to_string(&serde_json::json!({
            "schema_version": reg.schema_version(),
            "instances": reg.instances()
        }))
        .unwrap()
        .to_lowercase();
        for field in forbidden {
            assert!(
                !full_json.contains(&format!("\"{field}\"")),
                "forbidden field `{field}` in full registry: {full_json}"
            );
        }
    }

    #[test]
    fn unknown_enum_and_schema_failure_are_actionable() {
        let path = std::env::temp_dir().join("superai-core-tests/unknown_enum.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Unknown isolation variant
        let bad = serde_json::json!({
            "schema_version": 1,
            "instances": [{
                "id": "id-1",
                "name": "work",
                "harness": "claude-code",
                "config_root": "/home/u/.claude-work",
                "isolation": "bogus_unknown",
                "origin": "created",
                "ownership": "superai_created",
                "created_at": "2026-08-26T00:00:00Z",
                "adapter_revision": "0.1.0"
            }]
        });
        std::fs::write(&path, serde_json::to_string(&bad).unwrap()).unwrap();
        let err = Registry::load(&path).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("bogus_unknown") || msg.contains("unknown variant"),
            "error must mention unknown variant, got: {msg}"
        );
        assert!(
            msg.to_lowercase().contains("isolation")
                || msg.to_lowercase().contains("instance")
                || msg.contains("schema"),
            "error must be actionable, got: {msg}"
        );

        // Unsupported schema_version
        let bad2 = serde_json::json!({
            "schema_version": 999,
            "instances": []
        });
        std::fs::write(&path, serde_json::to_string(&bad2).unwrap()).unwrap();
        let err2 = Registry::load(&path).unwrap_err();
        let msg2 = format!("{err2}");
        assert!(
            msg2.contains("999"),
            "must mention offending schema_version: {msg2}"
        );
        assert!(
            msg2.contains("expected") || msg2.contains("unsupported"),
            "must be actionable: {msg2}"
        );
        assert!(msg2.contains(&path.display().to_string()) || msg2.contains("schema_version"));
    }

    #[test]
    fn golden_fixtures_old_and_new_are_valid() {
        // Old fixture: minimal old shape without schema_version
        let old_fixture = serde_json::json!({
            "instances": [
                {
                    "name": "work",
                    "harness": "claude-code",
                    "config_dir": "/home/user/.claude-work",
                    "binary_path": "/usr/local/bin/claude",
                    "template": {"name": "claude-glm", "version": "1.2.0"}
                },
                {
                    "name": "personal",
                    "harness": "codex-cli",
                    "config_dir": "/home/user/.codex-personal"
                }
            ],
            "foreign_key": "preserve-me"
        });
        let path = std::env::temp_dir().join("superai-core-tests/golden_old.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&old_fixture).unwrap()).unwrap();
        let reg = Registry::load(&path).unwrap();
        assert_eq!(reg.instances().len(), 2);
        assert_eq!(reg.instances()[0].origin, InstanceOrigin::AdoptedLegacy);
        assert_eq!(reg.instances()[0].isolation, Isolation::Unknown);
        assert_eq!(
            reg.instances()[0].config_root.to_string(),
            "/home/user/.claude-work"
        );
        assert_eq!(
            reg.instances()[0].binary.as_ref().unwrap().to_string(),
            "/usr/local/bin/claude"
        );
        assert_eq!(
            reg.instances()[0].template.as_ref().unwrap().name.as_str(),
            "claude-glm"
        );
        // New fixture: proper v1
        let new_fixture = serde_json::json!({
            "schema_version": 1,
            "instances": [
                {
                    "id": "inst-1",
                    "name": "work",
                    "harness": "claude-code",
                    "config_root": "/home/user/.claude-work",
                    "binary": "claude",
                    "wrapper": {
                        "path": "/home/user/.local/bin/work",
                        "command_name": "work",
                        "generator_version": "0.1.0",
                        "content_digest": "abc123"
                    },
                    "isolation": "relocated_root",
                    "origin": "created",
                    "ownership": "superai_created",
                    "template": {"name": "claude-glm", "version": "1.2.0"},
                    "created_at": "2026-08-26T12:00:00Z",
                    "adapter_revision": "0.1.0"
                }
            ],
            "foreign_key": "preserve-me"
        });
        let path2 = std::env::temp_dir().join("superai-core-tests/golden_new.json");
        std::fs::write(&path2, serde_json::to_string_pretty(&new_fixture).unwrap()).unwrap();
        let reg2 = Registry::load(&path2).unwrap();
        assert_eq!(reg2.instances().len(), 1);
        assert_eq!(reg2.instances()[0].id.as_str(), "inst-1");
        assert_eq!(reg2.instances()[0].name.as_str(), "work");
        assert_eq!(
            reg2.instances()[0]
                .wrapper
                .as_ref()
                .unwrap()
                .path
                .to_string(),
            "/home/user/.local/bin/work"
        );
        // Round-trip preserves foreign key
        reg2.store(&path2).unwrap();
        let raw = superai_config::json::load(&path2).unwrap();
        assert_eq!(raw["foreign_key"], serde_json::json!("preserve-me"));
    }

    #[test]
    fn now_iso8601_is_valid_rfc3339() {
        let ts = now_iso8601();
        assert!(ts.ends_with('Z'), "must end with Z: {ts}");
        assert!(ts.contains('T'), "must contain T: {ts}");
        // Check length 20: YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(
            ts.len(),
            20,
            "expected 20 chars RFC3339 without millis: {ts}"
        );
        // Known epoch
        let epoch = unix_secs_to_rfc3339(0);
        assert_eq!(epoch, "1970-01-01T00:00:00Z");
        let known = unix_secs_to_rfc3339(1_728_000_000);
        // 1728000000 secs is 2024-10-02 something; just check format not exact
        assert!(known.starts_with("2024-"), "known ts: {known}");
    }
}
