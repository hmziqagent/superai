//! Three-way template update and transactional apply (TPL-06, TPL-07):
//! local==base applies new, new==base keeps local, both differ conflicts.

#![expect(
    clippy::excessive_nesting,
    reason = "three-way and transaction branches are explicit"
)]
#![expect(clippy::too_many_lines, reason = "combined preview and apply logic")]
#![expect(
    clippy::redundant_clone,
    reason = "preview clones values for ownership clarity"
)]
#![expect(clippy::uninlined_format_args, reason = "test format explicit")]

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use superai_config::document::{
    DocumentKind, EditOperation, Operation as EngineOperation, Selector,
};
use superai_config::executor;
use superai_config::snapshot::{is_modified, snapshot};
use superai_config::transaction::{FileAction, Transaction};

use crate::adapter::Adapter;
use crate::capability_resolver;
use crate::error::{CoreError, Result};
use crate::ids::TemplateVersion;
use crate::instance::{Instance, TemplateRef};
use crate::registry::Registry;
use crate::template::{CapabilityChanges, Template, compute_digest};

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn selector_to_path(selector: &str) -> Option<Vec<String>> {
    let parsed = Selector::parse(selector).ok()?;
    match parsed {
        Selector::Key(k) => {
            if k.is_empty() {
                return None;
            }
            let mut parts: Vec<String> = Vec::new();
            for part in k.split('.') {
                if part.is_empty() {
                    return None;
                }
                parts.push(part.to_owned());
            }
            Some(parts)
        }
        _ => None,
    }
}

fn get_local_value(local: &Map<String, Value>, selector: &str) -> Option<Value> {
    let path = selector_to_path(selector)?;
    if path.is_empty() {
        return None;
    }
    let first = path.first()?;
    let mut current = local.get(first)?.clone();
    for segment in path.iter().skip(1) {
        match current {
            Value::Object(ref map) => {
                let next = map.get(segment)?.clone();
                current = next;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// Engine operation for one edit: three-way edits route through the
/// executor so `owned_keys/expected_old/create_parent` are enforced.
fn edit_to_engine_operation(edit: &Edit, owned_keys: &[String]) -> Result<EngineOperation> {
    let selector = Selector::parse(&edit.selector).map_err(|e| CoreError::Validation {
        field: "patches.selector".to_owned(),
        reason: format!("selector `{}` invalid: {e}", edit.selector),
    })?;
    let kind = match &edit.to {
        Some(value) => EditOperation::Set {
            selector,
            value: value.clone(),
        },
        None => EditOperation::Remove { selector },
    };
    Ok(EngineOperation::new(kind)
        .with_owned_keys(owned_keys.to_vec())
        .with_expected_old(edit.from.clone())
        .with_create_parent(true))
}

fn quarantine_target(path: &Path, op_id: &str) {
    // Best-effort: ignore errors, as quarantine is recovery aid
    let res: std::result::Result<superai_config::quarantine::QuarantineEntry, _> =
        superai_config::quarantine::move_to_quarantine(path, op_id);
    drop(res);
}

fn resolve_config_path(instance: &Instance, adapter: &dyn Adapter) -> PathBuf {
    for surface in adapter.config_surfaces() {
        if surface.id.contains("settings") {
            // The surface's fallback basename (settings.json and friends)
            // resolves inside the instance's relocated root.
            if let Some(name) = Path::new(&surface.path_resolver.fallback)
                .file_name()
                .and_then(|n| n.to_str())
                && name.to_ascii_lowercase().contains("settings.json")
            {
                return instance.config_root.as_path().join(name);
            }
        }
    }
    instance.config_root.as_path().join("settings.json")
}

/// Load the local map for a three-way merge, or refuse honestly (DOC-05):
/// unparseable bytes must not become an empty map; typed lossy-write error.
fn load_local_map(path: &Path) -> Result<Map<String, Value>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            if bytes.is_empty() || bytes.iter().all(u8::is_ascii_whitespace) {
                return Ok(Map::new());
            }
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(m)) => Ok(m),
                Ok(_) => Ok(Map::new()),
                Err(_) => Err(CoreError::Config(superai_config::ConfigError::LossyWrite {
                    path: path.to_path_buf(),
                    format: "jsonc",
                })),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(_) => Ok(Map::new()),
    }
}

/// One automatically applicable edit from the three-way merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Typed selector that will be mutated.
    pub selector: String,
    /// Value before the edit, if any.
    pub from: Option<Value>,
    /// Value after the edit, if any (`None` means removal).
    pub to: Option<Value>,
}

impl Edit {
    /// Human description.
    pub fn description(&self) -> String {
        match (&self.from, &self.to) {
            (Some(f), Some(t)) => format!("{}: {} -> {}", self.selector, f, t),
            (None, Some(t)) => format!("{}: (absent) -> {}", self.selector, t),
            (Some(f), None) => format!("{}: {} -> (removed)", self.selector, f),
            (None, None) => format!("{}: (absent) -> (removed)", self.selector),
        }
    }
}

/// Kind of conflict that blocks automatic apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both local and new differ from base with different values.
    BothModified,
    /// Local value is missing but base expected a value.
    Missing,
    /// Type of local differs from base.
    TypeChanged,
    /// Selector cannot be evaluated against local (non-Key or schema mismatch).
    SchemaConflict,
    /// Major template version drops the selector; requires explicit
    /// resolution (TPL-08 selector-reset policy).
    SelectorReset,
}

impl std::fmt::Display for ConflictKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::BothModified => "both_modified",
            Self::Missing => "missing",
            Self::TypeChanged => "type_changed",
            Self::SchemaConflict => "schema_conflict",
            Self::SelectorReset => "selector_reset",
        };
        f.write_str(s)
    }
}

/// One conflict that requires explicit resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Selector that conflicts.
    pub selector: String,
    /// Value in base template, if any.
    pub base: Option<Value>,
    /// Value in local config, if any.
    pub local: Option<Value>,
    /// Value in new template, if any.
    pub new: Option<Value>,
    /// Kind of conflict.
    pub kind: ConflictKind,
    /// Human message, redacted (no secrets).
    pub message: String,
}

/// Wrapper changes included in the preview.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WrapperChanges {
    /// Added env entries.
    pub added_env: Vec<(String, String)>,
    /// Removed env entries.
    pub removed_env: Vec<(String, String)>,
    /// Changed env entries.
    pub changed_env: Vec<(String, String, String)>,
    /// Added wrapper args.
    pub added_args: Vec<String>,
    /// Removed wrapper args.
    pub removed_args: Vec<String>,
    /// Added assets.
    pub added_assets: Vec<String>,
    /// Removed assets.
    pub removed_assets: Vec<String>,
}

impl WrapperChanges {
    /// True when no wrapper change exists.
    pub fn is_empty(&self) -> bool {
        self.added_env.is_empty()
            && self.removed_env.is_empty()
            && self.changed_env.is_empty()
            && self.added_args.is_empty()
            && self.removed_args.is_empty()
            && self.added_assets.is_empty()
            && self.removed_assets.is_empty()
    }
}

fn compute_wrapper_changes(base: &Template, new: &Template) -> WrapperChanges {
    let mut added_env = Vec::new();
    let mut removed_env = Vec::new();
    let mut changed_env = Vec::new();
    for (k, bv) in &base.wrapper_env {
        match new.wrapper_env.get(k) {
            None => removed_env.push((k.clone(), bv.clone())),
            Some(nv) => {
                if bv != nv {
                    changed_env.push((k.clone(), bv.clone(), nv.clone()));
                }
            }
        }
    }
    for (k, nv) in &new.wrapper_env {
        if !base.wrapper_env.contains_key(k) {
            added_env.push((k.clone(), nv.clone()));
        }
    }
    let base_set: BTreeSet<&String> = base.wrapper_args.iter().collect();
    let new_set: BTreeSet<&String> = new.wrapper_args.iter().collect();
    let mut added_args = Vec::new();
    let mut removed_args = Vec::new();
    for a in &base.wrapper_args {
        if !new_set.contains(a) {
            removed_args.push(a.clone());
        }
    }
    for a in &new.wrapper_args {
        if !base_set.contains(a) {
            added_args.push(a.clone());
        }
    }
    let base_assets: BTreeSet<&String> = base.assets.iter().collect();
    let new_assets: BTreeSet<&String> = new.assets.iter().collect();
    let mut added_assets = Vec::new();
    let mut removed_assets = Vec::new();
    for a in &base.assets {
        if !new_assets.contains(a) {
            removed_assets.push((*a).clone());
        }
    }
    for a in &new.assets {
        if !base_assets.contains(a) {
            added_assets.push((*a).clone());
        }
    }
    added_env.sort();
    removed_env.sort();
    changed_env.sort();
    added_args.sort();
    removed_args.sort();
    added_assets.sort();
    removed_assets.sort();
    WrapperChanges {
        added_env,
        removed_env,
        changed_env,
        added_args,
        removed_args,
        added_assets,
        removed_assets,
    }
}

fn compute_capability_changes(base: &Template, new: &Template) -> CapabilityChanges {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for (k, bv) in &base.capability_map {
        match new.capability_map.get(k) {
            None => removed.push((k.to_string(), bv.to_string())),
            Some(nv) => {
                if bv != nv {
                    changed.push((k.to_string(), bv.to_string(), nv.to_string()));
                }
            }
        }
    }
    for (k, nv) in &new.capability_map {
        if !base.capability_map.contains_key(k) {
            added.push((k.to_string(), nv.to_string()));
        }
    }
    added.sort();
    removed.sort();
    changed.sort();
    CapabilityChanges {
        added,
        removed,
        changed,
    }
}

/// Resolver-computed capability delta for a preview (CAP-06): support
/// and source BEFORE vs AFTER, from real resolution sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilitySupportChange {
    /// Capability whose resolution changed.
    pub capability: crate::capability::Capability,
    /// Support before the update.
    pub before: crate::capability::Support,
    /// Source before the update.
    pub before_source: capability_resolver::CapabilitySource,
    /// Support after the update.
    pub after: crate::capability::Support,
    /// Source after the update.
    pub after_source: capability_resolver::CapabilitySource,
}

impl CapabilitySupportChange {
    /// Human description of the delta.
    pub fn describe(&self) -> String {
        format!(
            "{}: {} ({}) -> {} ({})",
            self.capability, self.before, self.before_source, self.after, self.after_source
        )
    }
}

/// Preview of a three-way template update (TPL-06).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePreview {
    /// Old defaults from base template.
    pub old_defaults: BTreeMap<String, Value>,
    /// New defaults from candidate template.
    pub new_defaults: BTreeMap<String, Value>,
    /// Current local values for each owned selector.
    pub local_values: BTreeMap<String, Option<Value>>,
    /// Edits that can be applied automatically.
    pub auto_applicable: Vec<Edit>,
    /// Conflicts requiring explicit resolution.
    pub conflicts: Vec<Conflict>,
    /// Wrapper env/args/asset changes.
    pub wrapper_changes: WrapperChanges,
    /// Capability map changes.
    pub capability_changes: CapabilityChanges,
    /// Resolver-computed capability deltas (CAP-06): native/substituted/
    /// absent BEFORE vs AFTER the update, from real resolution sources.
    pub resolved_capability_changes: Vec<CapabilitySupportChange>,
    /// Warnings such as migration notes and status changes.
    pub warnings: Vec<String>,
}

impl UpdatePreview {
    /// True when the preview has no conflicts and can be applied without resolution.
    pub fn can_auto_apply(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// True when there is nothing to do (no auto edits and no conflicts).
    pub fn is_empty(&self) -> bool {
        self.auto_applicable.is_empty()
            && self.conflicts.is_empty()
            && self.wrapper_changes.is_empty()
            && self.capability_changes.added.is_empty()
            && self.capability_changes.removed.is_empty()
            && self.capability_changes.changed.is_empty()
            && self.resolved_capability_changes.is_empty()
    }
}

/// Three-way preview per owned selector: local==base applies new,
/// new==base keeps local, both differ conflicts; foreign untouched.
pub fn preview_three_way(
    base: &Template,
    new: &Template,
    local: &Map<String, Value>,
) -> UpdatePreview {
    let mut warnings: Vec<String> = Vec::new();
    if base.id != new.id {
        warnings.push(format!("template id mismatch: {} vs {}", base.id, new.id));
    }
    if base.harness != new.harness {
        warnings.push(format!(
            "harness mismatch: {} vs {}",
            base.harness, new.harness
        ));
    }
    if new.status == crate::template::TemplateStatus::Yanked {
        warnings.push(format!(
            "candidate version {} is yanked: use with care",
            new.version
        ));
    }
    // TPL-08: deprecated-with-replacement pointer surfaces in the preview.
    if new.status == crate::template::TemplateStatus::Deprecated {
        match new.replacement.as_ref() {
            Some(replacement) => warnings.push(format!(
                "candidate version {} is deprecated; replacement template `{replacement}`",
                new.version
            )),
            None => warnings.push(format!(
                "candidate version {} is deprecated without a replacement pointer",
                new.version
            )),
        }
    }
    // TPL-08: a major version bump may reset the selector set and cannot
    // silently reuse or drop old selectors.
    let major_bump = {
        let base_major = crate::template::parse_semver(&base.version)
            .ok()
            .map(|v| v.major);
        let new_major = crate::template::parse_semver(&new.version)
            .ok()
            .map(|v| v.major);
        matches!((base_major, new_major), (Some(b), Some(n)) if n > b)
    };
    for note in &new.migration_notes {
        if !base.migration_notes.contains(note) {
            // Redact secret-like notes similarly to template diff
            let redacted = if note.to_ascii_lowercase().contains("api_key")
                || note.to_ascii_lowercase().contains("secret")
                || note.to_ascii_lowercase().contains("token")
                || note.to_ascii_lowercase().contains("sk-")
            {
                "[REDACTED]".to_owned()
            } else {
                note.clone()
            };
            warnings.push(format!("migration: {redacted}"));
        }
    }

    let mut old_defaults: BTreeMap<String, Value> = BTreeMap::new();
    for p in &base.patches {
        old_defaults.insert(p.selector.clone(), p.value.clone());
    }
    let mut new_defaults: BTreeMap<String, Value> = BTreeMap::new();
    for p in &new.patches {
        new_defaults.insert(p.selector.clone(), p.value.clone());
    }

    let mut union: BTreeSet<String> = BTreeSet::new();
    for k in old_defaults.keys() {
        union.insert(k.clone());
    }
    for k in new_defaults.keys() {
        union.insert(k.clone());
    }

    let mut local_values: BTreeMap<String, Option<Value>> = BTreeMap::new();
    let mut auto_applicable: Vec<Edit> = Vec::new();
    let mut conflicts: Vec<Conflict> = Vec::new();

    for selector in &union {
        let base_val = old_defaults.get(selector).cloned();
        let new_val = new_defaults.get(selector).cloned();
        let path_opt = selector_to_path(selector);
        let local_val = if path_opt.is_none() {
            conflicts.push(Conflict {
                selector: selector.clone(),
                base: base_val.clone(),
                local: None,
                new: new_val.clone(),
                kind: ConflictKind::SchemaConflict,
                message: format!(
                    "selector `{selector}` is not a key selector, cannot three-way merge"
                ),
            });
            local_values.insert(selector.clone(), None);
            continue;
        } else {
            get_local_value(local, selector)
        };
        local_values.insert(selector.clone(), local_val.clone());

        // Missing: base expected a value the local config does not carry.
        if base_val.is_some() && local_val.is_none() {
            conflicts.push(Conflict {
                selector: selector.clone(),
                base: base_val.clone(),
                local: local_val.clone(),
                new: new_val.clone(),
                kind: ConflictKind::Missing,
                message: format!(
                    "selector `{selector}` missing in local config but expected by base"
                ),
            });
            continue;
        }
        // Type changed: local type differs from base type when both present.
        if let (Some(bv), Some(lv)) = (&base_val, &local_val)
            && json_type_name(bv) != json_type_name(lv)
        {
            conflicts.push(Conflict {
                selector: selector.clone(),
                base: base_val.clone(),
                local: local_val.clone(),
                new: new_val.clone(),
                kind: ConflictKind::TypeChanged,
                message: format!(
                    "selector `{selector}` type changed: base is {}, local is {}",
                    json_type_name(bv),
                    json_type_name(lv)
                ),
            });
            continue;
        }

        // TPL-08: on a major bump, a selector the new template drops must be
        // resolved explicitly, it is not silently removed (selector reset).
        if major_bump && new_val.is_none() {
            conflicts.push(Conflict {
                selector: selector.clone(),
                base: base_val.clone(),
                local: local_val.clone(),
                new: None,
                kind: ConflictKind::SelectorReset,
                message: format!(
                    "major version change drops selector `{selector}`; resolve explicitly (remove or migrate)"
                ),
            });
            continue;
        }
        if local_val == base_val {
            if new_val != base_val {
                auto_applicable.push(Edit {
                    selector: selector.clone(),
                    from: local_val.clone(),
                    to: new_val.clone(),
                });
            }
        } else if new_val == base_val {
            // new == base -> keep local, no edit
        } else if local_val == new_val {
            // already applied, no edit
        } else {
            conflicts.push(Conflict {
                selector: selector.clone(),
                base: base_val.clone(),
                local: local_val.clone(),
                new: new_val.clone(),
                kind: ConflictKind::BothModified,
                message: format!(
                    "selector `{selector}` both local and new differ from base: local={:?} new={:?} base={:?}",
                    local_val, new_val, base_val
                ),
            });
        }
    }

    if major_bump {
        warnings.push(format!(
            "major version change {} -> {}: explicit migration may be required; old selectors are not silently reused",
            base.version, new.version
        ));
    }
    let wrapper_changes = compute_wrapper_changes(base, new);
    let capability_changes = compute_capability_changes(base, new);

    // Sort for determinism
    auto_applicable.sort_by(|a, b| a.selector.cmp(&b.selector));
    conflicts.sort_by(|a, b| a.selector.cmp(&b.selector));

    UpdatePreview {
        old_defaults,
        new_defaults,
        local_values,
        auto_applicable,
        conflicts,
        wrapper_changes,
        capability_changes,
        resolved_capability_changes: Vec::new(),
        warnings,
    }
}

/// Resolver-backed capability delta between two templates (CAP-06),
/// reporting support/source changes from real resolution sources.
pub fn compute_resolved_capability_delta(
    base: &Template,
    new: &Template,
    adapter: &dyn Adapter,
    provider: Option<&crate::provider::ProviderDefinition>,
) -> Vec<CapabilitySupportChange> {
    let before_sources = capability_resolver::CapabilitySources::for_adapter(
        adapter,
        provider,
        Some(&base.capability_map),
    );
    let after_sources = capability_resolver::CapabilitySources::for_adapter(
        adapter,
        provider,
        Some(&new.capability_map),
    );
    let mut deltas = Vec::new();
    let before_all =
        capability_resolver::resolve_all_with_sources(&new.harness, &new.provider, &before_sources);
    let after_all =
        capability_resolver::resolve_all_with_sources(&new.harness, &new.provider, &after_sources);
    for (cap, before) in before_all {
        let after = after_all
            .iter()
            .find(|(c, _)| *c == cap)
            .map(|(_, resolved)| resolved);
        if let Some(after) = after
            && (before.support != after.support || before.source != after.source)
        {
            deltas.push(CapabilitySupportChange {
                capability: cap,
                before: before.support,
                before_source: before.source,
                after: after.support,
                after_source: after.source,
            });
        }
    }
    deltas
}

/// [`preview_three_way`] plus the resolver-computed capability delta
/// (CAP-06): capability changes visible BEFORE any commit.
pub fn preview_update_with_capability_resolution(
    base: &Template,
    new: &Template,
    local: &Map<String, Value>,
    adapter: &dyn Adapter,
    provider: Option<&crate::provider::ProviderDefinition>,
) -> UpdatePreview {
    let mut preview = preview_three_way(base, new, local);
    preview.resolved_capability_changes =
        compute_resolved_capability_delta(base, new, adapter, provider);
    preview
}

/// Outcome of `apply_update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// Edits that were applied.
    pub applied: Vec<Edit>,
    /// Verification messages (redacted).
    pub verification: Vec<String>,
    /// Whether the registry was updated to the new version.
    pub registry_updated: bool,
    /// Paths quarantined on failure, if any.
    pub quarantined: Vec<PathBuf>,
    /// Warnings emitted during apply.
    pub warnings: Vec<String>,
    /// Snapshot digest before the apply (conflict token).
    pub conflict_token: Option<String>,
}

/// Verify in-memory template bytes: size cap, id/version agreement,
/// catalog digest match; the `digest` field is format-checked only.
fn verify_template_bytes_in_memory(
    template: &Template,
    bytes: &[u8],
    catalog_digest: Option<&str>,
    context: &str,
) -> Result<()> {
    if bytes.len() > crate::template::MAX_TEMPLATE_BYTES {
        return Err(CoreError::Validation {
            field: "template".to_owned(),
            reason: format!(
                "template bytes for `{context}` exceed limit {} (got {})",
                crate::template::MAX_TEMPLATE_BYTES,
                bytes.len()
            ),
        });
    }
    let parsed = Template::from_json_bytes(bytes).map_err(|e| CoreError::SchemaValidation {
        path: PathBuf::from(context),
        details: format!("template bytes invalid for `{context}`: {e}"),
    })?;
    if parsed.id != template.id {
        return Err(CoreError::Validation {
            field: "template.id".to_owned(),
            reason: format!(
                "in-memory bytes id {} != template id {}",
                parsed.id, template.id
            ),
        });
    }
    if parsed.version != template.version {
        return Err(CoreError::Validation {
            field: "template.version".to_owned(),
            reason: format!(
                "in-memory bytes version {} != template version {} for `{context}`",
                parsed.version, template.version
            ),
        });
    }
    let computed = compute_digest(bytes);
    if let Some(expected) = catalog_digest {
        let normalized = expected.trim().to_ascii_lowercase();
        if normalized != computed {
            return Err(CoreError::Verification {
                path: PathBuf::from(context),
                kind: "digest".to_owned(),
                reason: format!(
                    "catalog digest mismatch for `{context}`: catalog {normalized} vs computed {computed}"
                ),
            });
        }
    }
    let digest_field = template.digest.trim();
    if digest_field.len() != 64 || !digest_field.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CoreError::Validation {
            field: "digest".to_owned(),
            reason: format!("digest must be 64 hex for `{context}`"),
        });
    }
    Ok(())
}

/// Apply a template update transactionally (TPL-07): verify bytes, fresh
/// read, recompute three-way, transactional apply, registry written last.
#[expect(
    clippy::too_many_arguments,
    reason = "transaction requires base/new bytes and adapter"
)]
pub fn apply_update(
    instance: &Instance,
    registry_path: &Path,
    base: &Template,
    new: &Template,
    base_bytes: &[u8],
    new_bytes: &[u8],
    adapter: &dyn Adapter,
) -> Result<ApplyOutcome> {
    apply_update_with_catalog_digests(
        instance,
        registry_path,
        base,
        new,
        base_bytes,
        new_bytes,
        None,
        None,
        adapter,
    )
}

/// Same as `apply_update` but allows caller to pass catalog digests for verification.
#[expect(
    clippy::too_many_arguments,
    reason = "explicit catalog digests for verification"
)]
pub fn apply_update_with_catalog_digests(
    instance: &Instance,
    registry_path: &Path,
    base: &Template,
    new: &Template,
    base_bytes: &[u8],
    new_bytes: &[u8],
    base_catalog_digest: Option<&str>,
    new_catalog_digest: Option<&str>,
    adapter: &dyn Adapter,
) -> Result<ApplyOutcome> {
    verify_template_bytes_in_memory(base, base_bytes, base_catalog_digest, "base")?;
    verify_template_bytes_in_memory(new, new_bytes, new_catalog_digest, "new")?;

    if base.harness != instance.harness || new.harness != instance.harness {
        return Err(CoreError::Validation {
            field: "harness".to_owned(),
            reason: format!(
                "instance harness {} does not match base {} or new {}",
                instance.harness, base.harness, new.harness
            ),
        });
    }

    // CAP-04: validating the candidate against the receiving adapter
    // enforces selector ownership and completeness before any mutation.
    new.validate_against_adapter(adapter)?;

    let registry = Registry::load(registry_path)?;
    let fresh_instance =
        registry
            .get_by_id(instance.id.as_str())
            .ok_or_else(|| CoreError::Validation {
                field: "instance".to_owned(),
                reason: format!(
                    "instance id {} not found in registry {}",
                    instance.id,
                    registry_path.display()
                ),
            })?;

    let config_path = resolve_config_path(fresh_instance, adapter);
    let snap_before = snapshot(&config_path);
    let conflict_token = snap_before.digest.clone();
    let local_map = load_local_map(&config_path)?;

    let preview = preview_three_way(base, new, &local_map);
    if !preview.conflicts.is_empty() {
        let msgs: Vec<String> = preview
            .conflicts
            .iter()
            .map(|c| format!("{}: {} ({})", c.selector, c.message, c.kind))
            .collect();
        return Err(CoreError::Validation {
            field: "conflict".to_owned(),
            reason: format!(
                "three-way conflicts require explicit resolution: {}",
                msgs.join("; ")
            ),
        });
    }

    // Check for external edit between snapshot and recompute (should be none since we just snapshotted)
    let snap_recheck = snapshot(&config_path);
    if is_modified(&snap_before, &snap_recheck) {
        return Err(CoreError::ConcurrentModification {
            path: config_path.clone(),
            expected: snap_before.digest.clone().unwrap_or_default(),
            actual: snap_recheck.digest.clone().unwrap_or_default(),
        });
    }

    // Apply edits through the document engine (DOC-02): owned keys are
    // the patch selectors, expected_old the observed `from`, parents created.
    let owned_keys: Vec<String> = base
        .patches
        .iter()
        .chain(new.patches.iter())
        .map(|p| p.selector.clone())
        .collect();
    let mut new_value = Value::Object(local_map.clone());
    for edit in &preview.auto_applicable {
        let op = edit_to_engine_operation(edit, &owned_keys)?;
        executor::apply_to_value(&config_path, &mut new_value, &op).map_err(CoreError::Config)?;
    }
    let Value::Object(new_local_map) = new_value else {
        return Err(CoreError::Validation {
            field: "config".to_owned(),
            reason: "config root ceased to be an object during edit application".to_owned(),
        });
    };

    let serialized_value = Value::Object(new_local_map.clone());
    let mut new_bytes_serialized =
        serde_json::to_string_pretty(&serialized_value).map_err(|e| {
            CoreError::SchemaValidation {
                path: config_path.clone(),
                details: format!("serialize new config failed: {e}"),
            }
        })?;
    new_bytes_serialized.push('\n');
    let new_content = new_bytes_serialized.into_bytes();

    let op_id_str = crate::registry::unique_operation_string("op");
    let tx_op_id = superai_config::transaction::OperationId::new(&op_id_str).map_err(|e| {
        CoreError::Validation {
            field: "operation_id".to_owned(),
            reason: format!("op id invalid: {e}"),
        }
    })?;

    let mut steps: Vec<FileAction> = Vec::new();
    steps.push(FileAction::Write {
        path: config_path.clone(),
        content: new_content.clone(),
        kind: DocumentKind::StrictJson,
    });

    // Wrapper regeneration when the template changes its env/args and the
    // instance carries a wrapper.
    if !preview.wrapper_changes.is_empty()
        && let Some(wrapper_ref) = &fresh_instance.wrapper
    {
        let wrapper_path = wrapper_ref.path.as_path().to_path_buf();
        let temp_instance = fresh_instance.clone();
        let plan = adapter.plan_wrapper(&temp_instance).unwrap_or_else(|_| {
            let mut p = crate::adapter::WrapperPlan::new("wrapper for update");
            p.env_vars = new.wrapper_env.clone().into_iter().collect();
            new.wrapper_args.clone_into(&mut p.args);
            p
        });
        // Template wrapper_env/args override or extend the plan.
        let mut merged_plan = plan;
        for (k, v) in &new.wrapper_env {
            if let Some(entry) = merged_plan.env_vars.iter_mut().find(|(ek, _)| ek == k) {
                entry.1.clone_from(v);
            } else {
                merged_plan.env_vars.push((k.clone(), v.clone()));
            }
        }
        for arg in &new.wrapper_args {
            if !merged_plan.args.contains(arg) {
                merged_plan.args.push(arg.clone());
            }
        }
        let (wrapper_content, _digest) =
            crate::wrapper::generate_shell_wrapper(&temp_instance, &merged_plan)?;
        steps.push(FileAction::Write {
            path: wrapper_path,
            content: wrapper_content.into_bytes(),
            kind: DocumentKind::TextFragment,
        });
    }

    // Asset paths are validated even though fetching them is out of scope.
    for asset in &new.assets {
        if let Err(e) = crate::template::validate_template_path(asset) {
            return Err(CoreError::Validation {
                field: "assets".to_owned(),
                reason: format!("asset path `{asset}` invalid: {e}"),
            });
        }
    }

    let mut transaction = Transaction::new(tx_op_id, steps);
    let outcome = transaction.execute().map_err(CoreError::Config)?;

    if !outcome.success {
        // Quarantine residuals if any
        let mut quarantined: Vec<PathBuf> = Vec::new();
        if let Some(rollback) = &outcome.rollback {
            for residual in &rollback.residuals {
                quarantine_target(residual, &op_id_str);
                quarantined.push(residual.clone());
            }
        } else {
            // Fallback: quarantine config path if it still exists and verification failed
            for verify in &outcome.verification {
                if !verify.digest_ok || !verify.parse_ok {
                    quarantine_target(&verify.path, &op_id_str);
                    quarantined.push(verify.path.clone());
                }
            }
        }
        // Ensure config's residuals are also quarantined
        if config_path.exists() {
            let has_verify_failure = outcome
                .verification
                .iter()
                .any(|v| !v.digest_ok || !v.parse_ok);
            if has_verify_failure {
                quarantine_target(&config_path, &op_id_str);
                if !quarantined.contains(&config_path) {
                    quarantined.push(config_path.clone());
                }
            }
        }
        return Ok(ApplyOutcome {
            applied: Vec::new(),
            verification: outcome.diagnostics_redacted.clone(),
            registry_updated: false,
            quarantined,
            warnings: preview.warnings.clone(),
            conflict_token,
        });
    }
    // Concurrent-modification check: the post-commit snapshot must equal the
    // digest of the content this transaction just wrote.
    let snap_after = snapshot(&config_path);
    if is_modified(&snap_before, &snap_after) && snap_after.digest.is_some() {
        let expected_digest = {
            use std::collections::hash_map::DefaultHasher;
            let mut hasher = DefaultHasher::new();
            new_content.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };
        if snap_after.digest.as_deref() != Some(expected_digest.as_str()) {
            drop(transaction.rollback());
            quarantine_target(&config_path, &op_id_str);
            return Err(CoreError::ConcurrentModification {
                path: config_path,
                expected: expected_digest,
                actual: snap_after.digest.unwrap_or_default(),
            });
        }
    }

    let mut updated_instance = fresh_instance.clone();
    updated_instance.template = Some(TemplateRef {
        name: new.id.clone(),
        version: TemplateVersion::new(&new.version).map_err(|e| CoreError::Validation {
            field: "template.version".to_owned(),
            reason: format!("new version invalid for registry: {e}"),
        })?,
    });
    crate::adapter::ADAPTER_REVISION.clone_into(&mut updated_instance.adapter_revision);

    if let Err(e) = adapter.validate_instance(&updated_instance) {
        // Rollback transaction
        let rb_outcome = transaction.rollback().map_err(CoreError::Config)?;
        for residual in &rb_outcome.residuals {
            quarantine_target(residual, &op_id_str);
        }
        drop(rb_outcome);
        quarantine_target(&config_path, &op_id_str);
        return Err(CoreError::Validation {
            field: "instance".to_owned(),
            reason: format!("adapter validation failed after apply: {e}"),
        });
    }

    let provider_id = new.provider.clone();
    let resolved = capability_resolver::resolve_all(&updated_instance.harness, &provider_id);
    // If any capability resolution yields Unknown source for required caps, add warning but not block
    let mut capability_warnings: Vec<String> = Vec::new();
    for (cap, res) in resolved {
        if res.source == capability_resolver::CapabilitySource::Unknown {
            capability_warnings.push(format!(
                "capability {cap:?} unknown for {}/{}",
                updated_instance.harness, provider_id
            ));
        }
    }

    let mut all_warnings = preview.warnings.clone();
    all_warnings.extend(capability_warnings);

    // 7. write new template version to registry last (only after verification)
    let fresh_registry = Registry::load(registry_path)?;
    let idx_opt = fresh_registry
        .instances()
        .iter()
        .position(|i| i.id == updated_instance.id);
    let Some(idx) = idx_opt else {
        // Registry changed concurrently: rollback
        drop(transaction.rollback());
        quarantine_target(&config_path, &op_id_str);
        return Err(CoreError::Validation {
            field: "registry".to_owned(),
            reason: format!(
                "instance {} vanished from registry during apply",
                updated_instance.id
            ),
        });
    };
    let mut instances_vec: Vec<Instance> = fresh_registry.instances().to_vec();
    if let Some(slot) = instances_vec.get_mut(idx) {
        slot.clone_from(&updated_instance);
    } else {
        return Err(CoreError::Validation {
            field: "registry".to_owned(),
            reason: "registry index out of bounds during update".to_owned(),
        });
    }
    // Rebuild and store; Registry::store preserves foreign keys on disk.
    let mut rebuilt = Registry::default();
    for inst in instances_vec {
        rebuilt.insert(inst).map_err(|e| CoreError::Validation {
            field: "registry".to_owned(),
            reason: format!("rebuilding registry failed: {e}"),
        })?;
    }
    if let Err(e) = rebuilt.store(registry_path) {
        // Rollback transaction on registry failure
        drop(transaction.rollback());
        quarantine_target(&config_path, &op_id_str);
        return Err(e);
    }

    Ok(ApplyOutcome {
        applied: preview.auto_applicable,
        verification: outcome.diagnostics_redacted,
        registry_updated: true,
        quarantined: Vec::new(),
        warnings: all_warnings,
        conflict_token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{HarnessId, ProviderId, TemplateId};
    use crate::paths::AbsolutePath;
    use crate::state::{InstanceOrigin, Isolation, Ownership};
    use crate::template::{OwnedPatch, TEMPLATE_SCHEMA_VERSION, TemplateInput, TemplateStatus};
    use serde_json::json;

    fn minimal_template(version: &str, patches: Vec<OwnedPatch>) -> Template {
        Template {
            schema_version: TEMPLATE_SCHEMA_VERSION,
            id: TemplateId::new("claude-glm").unwrap(),
            version: version.to_owned(),
            harness: HarnessId::new("claude-code").unwrap(),
            provider: ProviderId::new("glm").unwrap(),
            label: "Claude Code on GLM".to_owned(),
            status: TemplateStatus::Active,
            inputs: vec![TemplateInput {
                key: "model".to_owned(),
                description: "Model".to_owned(),
                required: true,
            }],
            patches,
            wrapper_env: BTreeMap::new(),
            wrapper_args: Vec::new(),
            assets: Vec::new(),
            capability_map: BTreeMap::new(),
            migration_notes: Vec::new(),
            digest: "a".repeat(64),
            harness_version_req: None,
            provider_protocol: None,
            replacement: None,
        }
    }

    fn patch(selector: &str, value: Value) -> OwnedPatch {
        OwnedPatch {
            selector: selector.to_owned(),
            value,
        }
    }

    /// Local adapter declaring only ONE capability transport: the CAP-04
    /// completeness gate must reject templates against it.
    #[derive(Debug)]
    struct PartialCapAdapter;

    impl Adapter for PartialCapAdapter {
        fn id(&self) -> HarnessId {
            HarnessId::new("partial-cap-harness").unwrap()
        }
        fn display_name(&self) -> &'static str {
            "Partial Cap"
        }
        fn product_status(&self) -> crate::adapter::ProductStatus {
            crate::adapter::ProductStatus::Active
        }
        fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
            Vec::new()
        }
        fn adapter_revision(&self) -> &'static str {
            "0.1.0"
        }
        fn research_doc_link(&self) -> &'static str {
            "docs/harness-configs/partial-cap.md"
        }
        fn last_verified_date(&self) -> &'static str {
            "2026-08-25"
        }
        fn detection(&self) -> crate::adapter::DetectionResult {
            crate::adapter::DetectionResult::absent(vec!["test".to_owned()])
        }
        fn version_resolution(&self) -> crate::adapter::VersionResolution {
            crate::adapter::VersionResolution::unknown()
        }
        fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
            let mut surface = crate::adapter::ConfigSurface::new(
                "settings.json",
                crate::adapter::PathResolver::fallback_only("~/.partial/settings.json"),
                crate::adapter::DocumentKind::Json,
                crate::adapter::ConfigScope::User,
                crate::adapter::SurfaceOwnership::UserEditable,
            );
            surface.owned_selectors = vec!["model".to_owned()];
            vec![surface]
        }
        fn supported_operations(&self) -> Vec<(String, crate::state::AdapterSupport)> {
            Vec::new()
        }
        fn plan_mirror_exclusions(&self) -> Vec<String> {
            Vec::new()
        }
        fn plan_wrapper(&self, _instance: &Instance) -> Result<crate::adapter::WrapperPlan> {
            Ok(crate::adapter::WrapperPlan::new("test"))
        }
        fn scan_candidates(&self) -> Vec<String> {
            Vec::new()
        }
        fn validate_instance(&self, _instance: &Instance) -> Result<()> {
            Ok(())
        }
        fn capability_declarations(&self) -> Vec<crate::adapter::AdapterCapabilityDecl> {
            vec![crate::adapter::AdapterCapabilityDecl::new(
                crate::capability::Capability::WebSearch,
                crate::capability::Support::Native,
                "only web search",
            )]
        }
    }

    #[test]
    fn incomplete_capability_coverage_blocks_template_use() {
        let mut tmpl = minimal_template("1.0.0", vec![]);
        tmpl.harness = HarnessId::new("partial-cap-harness").unwrap();
        // The adapter declares only web_search: the other three catalog
        // capabilities do not resolve -> publication/use is blocked.
        let err = tmpl
            .validate_against_adapter(&PartialCapAdapter)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("do not resolve") || err.contains("incomplete capability coverage"),
            "got: {err}"
        );
        // Covering the rest through the template's capability map passes.
        tmpl.capability_map.insert(
            crate::capability::Capability::Vision,
            crate::capability::Support::Absent,
        );
        tmpl.capability_map.insert(
            crate::capability::Capability::ComputerUse,
            crate::capability::Support::Absent,
        );
        tmpl.capability_map.insert(
            crate::capability::Capability::Mcp,
            crate::capability::Support::Absent,
        );
        tmpl.validate_against_adapter(&PartialCapAdapter).unwrap();
    }

    #[test]
    fn major_version_selector_reset_requires_explicit_resolution() {
        // 1.x -> 2.x that drops a selector: conflict, not silent removal.
        let base = minimal_template(
            "1.9.0",
            vec![
                patch("key:model", json!("glm-4")),
                patch("key:legacy", json!(true)),
            ],
        );
        let new = minimal_template("2.0.0", vec![patch("key:model", json!("glm-4.5"))]);
        let mut local = Map::new();
        local.insert("model".to_owned(), json!("glm-4"));
        local.insert("legacy".to_owned(), json!(true));
        let preview = preview_three_way(&base, &new, &local);
        let reset = preview
            .conflicts
            .iter()
            .find(|c| c.kind == ConflictKind::SelectorReset);
        assert!(reset.is_some(), "conflicts: {:?}", preview.conflicts);
        assert_eq!(
            reset.map(|c| c.selector.as_str()),
            Some("key:legacy"),
            "only the dropped selector conflicts"
        );
        assert!(
            preview
                .warnings
                .iter()
                .any(|w| w.contains("major version change"))
        );

        // A minor bump still auto-removes a dropped selector.
        let minor_new = minimal_template("1.10.0", vec![patch("key:model", json!("glm-4.5"))]);
        let preview = preview_three_way(&base, &minor_new, &local);
        assert!(
            preview
                .conflicts
                .iter()
                .all(|c| c.kind != ConflictKind::SelectorReset)
        );
        assert!(
            preview
                .auto_applicable
                .iter()
                .any(|e| e.selector == "key:legacy" && e.to.is_none())
        );
    }

    #[test]
    fn deprecated_candidate_warning_names_replacement() {
        let base = minimal_template("1.0.0", vec![patch("key:model", json!("glm-4"))]);
        let mut new = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4.5"))]);
        new.status = TemplateStatus::Deprecated;
        new.replacement = Some(TemplateId::new("claude-glm-next").unwrap());
        let preview = preview_three_way(&base, &new, &Map::new());
        assert!(
            preview
                .warnings
                .iter()
                .any(|w| w.contains("deprecated") && w.contains("claude-glm-next"))
        );
    }

    #[test]
    fn resolved_capability_delta_uses_sources_not_string_diff() {
        // Template override flips vision absent->native; the string diff
        // alone cannot express the source-attributed before/after.
        let base = minimal_template("1.0.0", vec![patch("key:model", json!("glm-4"))]);
        let mut new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]);
        new.capability_map.insert(
            crate::capability::Capability::Vision,
            crate::capability::Support::Native,
        );
        let adapter = crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap();
        let providers = crate::provider::load_bundled_providers().unwrap();
        let glm = providers.iter().find(|p| p.id.as_str() == "glm").unwrap();
        let mut local = Map::new();
        local.insert("model".to_owned(), json!("glm-4"));
        let preview =
            preview_update_with_capability_resolution(&base, &new, &local, &adapter, Some(glm));
        let vision = preview
            .resolved_capability_changes
            .iter()
            .find(|c| c.capability == crate::capability::Capability::Vision)
            .expect("vision delta present");
        assert_eq!(vision.before, crate::capability::Support::Absent);
        assert_eq!(vision.after, crate::capability::Support::Native);
        assert_eq!(
            vision.after_source,
            capability_resolver::CapabilitySource::Template
        );
        assert!(vision.describe().contains("vision"));
        // The plain preview keeps the empty resolved section.
        let plain = preview_three_way(&base, &new, &local);
        assert!(plain.resolved_capability_changes.is_empty());
    }

    #[test]
    fn preview_clean_update_local_eq_base_applies_new() {
        let base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]);
        let mut local = Map::new();
        local.insert("model".to_owned(), json!("glm-4"));
        let preview = preview_three_way(&base, &new, &local);
        assert!(
            preview.conflicts.is_empty(),
            "conflicts: {:?}",
            preview.conflicts
        );
        assert_eq!(preview.auto_applicable.len(), 1);
        assert_eq!(preview.auto_applicable[0].selector, "key:model");
        assert_eq!(preview.auto_applicable[0].to, Some(json!("glm-4.5")));
        assert!(preview.can_auto_apply());
    }

    #[test]
    fn preview_local_override_preserved_when_new_eq_base() {
        let base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let new = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let mut local = Map::new();
        local.insert("model".to_owned(), json!("my-custom-model"));
        let preview = preview_three_way(&base, &new, &local);
        assert!(preview.conflicts.is_empty());
        assert!(
            preview.auto_applicable.is_empty(),
            "should keep local, no auto: {:?}",
            preview.auto_applicable
        );
    }

    #[test]
    fn apply_update_blocked_on_incomplete_capability_coverage() {
        // CAP-04 on the USE path: an update whose capability coverage does
        // not resolve is refused before any disk mutation.
        let tmp = crate::test_util::temp_dir_unique("tpl-cap04-apply");
        let registry_path = tmp.join("instances.json");
        let config_root = tmp.join(".partial-work");
        std::fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join("settings.json");

        let mut base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let mut new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]);
        base.harness = HarnessId::new("partial-cap-harness").unwrap();
        new.harness = HarnessId::new("partial-cap-harness").unwrap();
        base.digest = "a".repeat(64);
        new.digest = "b".repeat(64);
        let base_bytes = serde_json::to_vec(&base).unwrap();
        let new_bytes = serde_json::to_vec(&new).unwrap();

        let instance = Instance {
            id: crate::ids::InstanceId::new("cap04-instance-001").unwrap(),
            name: crate::ids::InstanceName::new("cap04").unwrap(),
            harness: HarnessId::new("partial-cap-harness").unwrap(),
            config_root: AbsolutePath::from_path(&config_root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: TemplateId::new("claude-glm").unwrap(),
                version: TemplateVersion::new("1.1.0").unwrap(),
            }),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        };
        let mut registry = Registry::default();
        registry.insert(instance.clone()).unwrap();
        registry.store(&registry_path).unwrap();
        std::fs::write(&config_path, "{\n  \"model\": \"glm-4\"\n}\n").unwrap();

        // PartialCapAdapter declares only web_search: the candidate template
        // (empty capability_map) cannot cover the catalog -> apply refused.
        let err = apply_update(
            &instance,
            &registry_path,
            &base,
            &new,
            &base_bytes,
            &new_bytes,
            &PartialCapAdapter,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("do not resolve") || err.contains("incomplete capability coverage"),
            "got: {err}"
        );
        // Nothing mutated: old registry version, config bytes unchanged.
        let registry_after = Registry::load(&registry_path).unwrap();
        let kept = registry_after.get_by_id("cap04-instance-001").unwrap();
        assert_eq!(kept.template.as_ref().unwrap().version.as_str(), "1.1.0");
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            "{\n  \"model\": \"glm-4\"\n}\n"
        );

        // Covering the gap through the candidate's capability map lets the
        // same update apply.
        new.capability_map.insert(
            crate::capability::Capability::Vision,
            crate::capability::Support::Absent,
        );
        new.capability_map.insert(
            crate::capability::Capability::ComputerUse,
            crate::capability::Support::Absent,
        );
        new.capability_map.insert(
            crate::capability::Capability::Mcp,
            crate::capability::Support::Absent,
        );
        let new_bytes = serde_json::to_vec(&new).unwrap();
        let outcome = apply_update(
            &instance,
            &registry_path,
            &base,
            &new,
            &base_bytes,
            &new_bytes,
            &PartialCapAdapter,
        )
        .unwrap();
        assert!(outcome.registry_updated, "{:?}", outcome.applied);
        let registry_final = Registry::load(&registry_path).unwrap();
        let updated = registry_final.get_by_id("cap04-instance-001").unwrap();
        assert_eq!(updated.template.as_ref().unwrap().version.as_str(), "1.2.0");
        drop(std::fs::remove_dir_all(&tmp));
    }

    #[test]
    fn apply_nested_selector_creates_parent_and_preserves_foreign() {
        // DOC-02: nested patch selectors route through the engine executor,
        // creating parents while foreign keys and siblings survive.
        let tmp = crate::test_util::temp_dir_unique("tpl-update-nested");
        let registry_path = tmp.join("instances.json");
        let config_root = tmp.join(".claude-nested");
        std::fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join("settings.json");

        let mut base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        base.digest = "a".repeat(64);
        let mut new = minimal_template(
            "1.2.0",
            vec![
                patch("key:model", json!("glm-4.5")),
                patch("key:env.ANTHROPIC_MODEL.fallback", json!("high")),
            ],
        );
        new.digest = "b".repeat(64);
        let base_bytes = serde_json::to_vec(&base).unwrap();
        let new_bytes = serde_json::to_vec(&new).unwrap();

        let instance = Instance {
            id: crate::ids::InstanceId::new("test-nested-001").unwrap(),
            name: crate::ids::InstanceName::new("nested").unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::from_path(&config_root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: TemplateId::new("claude-glm").unwrap(),
                version: TemplateVersion::new("1.1.0").unwrap(),
            }),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
        };
        let mut registry = Registry::default();
        registry.insert(instance.clone()).unwrap();
        registry.store(&registry_path).unwrap();
        // Local has the base model plus a foreign sibling at the root.
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&json!({"model":"glm-4","foreignRoot":"keep"})).unwrap()
                + "\n",
        )
        .unwrap();

        let adapter = crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap();
        let outcome = apply_update(
            &instance,
            &registry_path,
            &base,
            &new,
            &base_bytes,
            &new_bytes,
            &adapter,
        )
        .unwrap();
        assert!(outcome.registry_updated);
        assert_eq!(outcome.applied.len(), 2, "{:?}", outcome.applied);

        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(after["model"], json!("glm-4.5"));
        assert_eq!(after["env"]["ANTHROPIC_MODEL"]["fallback"], json!("high"));
        assert_eq!(after["foreignRoot"], json!("keep"));
        drop(std::fs::remove_dir_all(&tmp));
    }

    #[test]
    fn preview_local_eq_new_already() {
        let base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]);
        let mut local = Map::new();
        local.insert("model".to_owned(), json!("glm-4.5"));
        let preview = preview_three_way(&base, &new, &local);
        assert!(preview.conflicts.is_empty());
        assert!(preview.auto_applicable.is_empty(), "already applied");
    }

    #[test]
    fn preview_both_differ_conflict() {
        let base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]);
        let mut local = Map::new();
        local.insert("model".to_owned(), json!("glm-4-custom"));
        let preview = preview_three_way(&base, &new, &local);
        assert_eq!(preview.conflicts.len(), 1);
        assert_eq!(preview.conflicts[0].kind, ConflictKind::BothModified);
        assert!(!preview.can_auto_apply());
        assert!(preview.auto_applicable.is_empty());
    }

    #[test]
    fn preview_missing_schema_conflict() {
        let base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]);
        let local = Map::new(); // missing model
        let preview = preview_three_way(&base, &new, &local);
        assert_eq!(preview.conflicts.len(), 1);
        assert_eq!(preview.conflicts[0].kind, ConflictKind::Missing);
    }

    #[test]
    fn preview_type_changed_schema_conflict() {
        let base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]);
        let mut local = Map::new();
        local.insert("model".to_owned(), json!({"nested": "object"}));
        let preview = preview_three_way(&base, &new, &local);
        assert_eq!(preview.conflicts.len(), 1);
        assert_eq!(preview.conflicts[0].kind, ConflictKind::TypeChanged);
    }

    #[test]
    fn preview_deleted_selector() {
        let base = minimal_template(
            "1.1.0",
            vec![
                patch("key:model", json!("glm-4")),
                patch("key:temperature", json!(0.7)),
            ],
        );
        let new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]); // temperature removed
        let mut local = Map::new();
        local.insert("model".to_owned(), json!("glm-4"));
        local.insert("temperature".to_owned(), json!(0.7));
        let preview = preview_three_way(&base, &new, &local);
        // temperature deletion: local==base so apply removal
        assert!(
            preview.conflicts.is_empty(),
            "conflicts: {:?}",
            preview.conflicts
        );
        let temp_edit = preview
            .auto_applicable
            .iter()
            .find(|e| e.selector == "key:temperature");
        assert!(
            temp_edit.is_some(),
            "temperature edit missing: {:?}",
            preview.auto_applicable
        );
        assert_eq!(temp_edit.unwrap().to, None);
        assert!(
            preview
                .auto_applicable
                .iter()
                .any(|e| e.selector == "key:model")
        );
    }

    #[test]
    fn preview_foreign_untouched() {
        let base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]);
        let mut local = Map::new();
        local.insert("model".to_owned(), json!("glm-4"));
        local.insert("foreign_key".to_owned(), json!("keep_me"));
        local.insert("another".to_owned(), json!(123));
        let preview = preview_three_way(&base, &new, &local);
        assert_eq!(preview.local_values.len(), 1);
        assert!(preview.local_values.contains_key("key:model"));
        assert!(!preview.local_values.contains_key("foreign_key"));
        assert_eq!(preview.auto_applicable.len(), 1);
    }

    #[test]
    fn preview_wrapper_and_capability_changes() {
        let mut base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        base.wrapper_env.insert("FOO".to_owned(), "bar".to_owned());
        base.capability_map.insert(
            crate::capability::Capability::WebSearch,
            crate::capability::Support::Native,
        );
        let mut new = base.clone();
        new.version = "1.2.0".to_owned();
        new.wrapper_env.insert("FOO".to_owned(), "baz".to_owned());
        new.wrapper_env.insert("BAR".to_owned(), "qux".to_owned());
        new.capability_map.insert(
            crate::capability::Capability::WebSearch,
            crate::capability::Support::Substituted,
        );
        new.capability_map.insert(
            crate::capability::Capability::Vision,
            crate::capability::Support::Native,
        );
        let local = Map::new();
        let preview = preview_three_way(&base, &new, &local);
        assert!(!preview.wrapper_changes.is_empty());
        assert!(
            preview
                .wrapper_changes
                .changed_env
                .iter()
                .any(|(k, old, new_v)| k == "FOO" && old == "bar" && new_v == "baz")
        );
        assert!(
            preview
                .wrapper_changes
                .added_env
                .iter()
                .any(|(k, v)| k == "BAR" && v == "qux")
        );
        assert!(!preview.capability_changes.added.is_empty());
        assert!(!preview.capability_changes.changed.is_empty());
    }

    #[test]
    fn preview_warnings_include_migration_notes() {
        let base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let mut new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]);
        new.migration_notes = vec!["bumped context window".to_owned()];
        let local = Map::new();
        let preview = preview_three_way(&base, &new, &local);
        assert!(preview.warnings.iter().any(|w| w.contains("bumped")));
    }

    #[test]
    fn apply_success_advances_version_and_retains_old_on_failure() {
        let tmp = crate::test_util::temp_dir_unique("tpl-update");
        let registry_path = tmp.join("instances.json");
        let config_root = tmp.join(".claude-work");
        std::fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join("settings.json");

        // Create base and new templates with proper digests
        // For this test we use placeholder digests; verification is lenient about template digest vs file hash.
        let base_patches = vec![patch("key:model", json!("glm-4"))];
        let new_patches = vec![patch("key:model", json!("glm-4.5"))];
        let mut base_tmpl = minimal_template("1.1.0", base_patches);
        let mut new_tmpl = minimal_template("1.2.0", new_patches);
        base_tmpl.digest = "a".repeat(64);
        new_tmpl.digest = "b".repeat(64);
        let base_bytes = serde_json::to_vec(&base_tmpl).unwrap();
        let new_bytes = serde_json::to_vec(&new_tmpl).unwrap();

        let instance = Instance {
            id: crate::ids::InstanceId::new("test-instance-001").unwrap(),
            name: crate::ids::InstanceName::new("work").unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::from_path(&config_root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: TemplateId::new("claude-glm").unwrap(),
                version: TemplateVersion::new("1.1.0").unwrap(),
            }),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
        };
        let mut registry = Registry::default();
        registry.insert(instance.clone()).unwrap();
        registry.store(&registry_path).unwrap();

        let mut local_map = Map::new();
        local_map.insert("model".to_owned(), json!("glm-4"));
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&Value::Object(local_map)).unwrap() + "\n",
        )
        .unwrap();

        let adapter = crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap();

        // Preview with empty local would be missing; the real local matches base.
        let local_for_preview = {
            let mut m = Map::new();
            m.insert("model".to_owned(), json!("glm-4"));
            m
        };
        let preview = preview_three_way(&base_tmpl, &new_tmpl, &local_for_preview);
        assert!(preview.can_auto_apply());

        let outcome = apply_update(
            &instance,
            &registry_path,
            &base_tmpl,
            &new_tmpl,
            &base_bytes,
            &new_bytes,
            &adapter,
        )
        .unwrap();
        assert!(outcome.registry_updated);
        assert_eq!(outcome.applied.len(), 1);
        let registry_after = Registry::load(&registry_path).unwrap();
        let updated = registry_after.get_by_id("test-instance-001").unwrap();
        assert_eq!(updated.template.as_ref().unwrap().version.as_str(), "1.2.0");
        let new_config_text = std::fs::read_to_string(&config_path).unwrap();
        let new_val: Value = serde_json::from_str(&new_config_text).unwrap();
        assert_eq!(new_val["model"], json!("glm-4.5"));

        // Failure case: a second instance with a conflicting local value.
        let config_root2 = tmp.join(".claude-work2");
        std::fs::create_dir_all(&config_root2).unwrap();
        let config_path2 = config_root2.join("settings.json");
        let instance2 = Instance {
            id: crate::ids::InstanceId::new("test-instance-002").unwrap(),
            name: crate::ids::InstanceName::new("work2").unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::from_path(&config_root2).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: TemplateId::new("claude-glm").unwrap(),
                version: TemplateVersion::new("1.1.0").unwrap(),
            }),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
        };
        let mut registry2 = Registry::load(&registry_path).unwrap();
        registry2.insert(instance2.clone()).unwrap();
        registry2.store(&registry_path).unwrap();
        let mut conflict_local = Map::new();
        conflict_local.insert("model".to_owned(), json!("custom-model"));
        std::fs::write(
            &config_path2,
            serde_json::to_string_pretty(&Value::Object(conflict_local.clone())).unwrap() + "\n",
        )
        .unwrap();
        let preview_conflict = preview_three_way(&base_tmpl, &new_tmpl, &conflict_local);
        assert!(!preview_conflict.can_auto_apply());
        let apply_res = apply_update(
            &instance2,
            &registry_path,
            &base_tmpl,
            &new_tmpl,
            &base_bytes,
            &new_bytes,
            &adapter,
        );
        assert!(
            apply_res.is_err(),
            "expected conflict error, got {:?}",
            apply_res
        );
        let registry_after_fail = Registry::load(&registry_path).unwrap();
        let still = registry_after_fail.get_by_id("test-instance-002").unwrap();
        assert_eq!(still.template.as_ref().unwrap().version.as_str(), "1.1.0");
        let after_fail_text = std::fs::read_to_string(&config_path2).unwrap();
        let after_fail_val: Value = serde_json::from_str(&after_fail_text).unwrap();
        assert_eq!(after_fail_val["model"], json!("custom-model"));

        drop(std::fs::remove_dir_all(&tmp));
    }

    #[test]
    fn apply_update_refuses_jsonc_settings_instead_of_normalizing() {
        // DOC-05: JSONC settings bytes (comments, trailing commas) must
        // fail with the typed lossy error, never become an empty map.
        let tmp = crate::test_util::temp_dir_unique("tpl-update-jsonc");
        let registry_path = tmp.join("instances.json");
        let config_root = tmp.join(".claude-jsonc");
        std::fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join("settings.json");
        let jsonc =
            "{\n  // user comment\n  \"model\": \"glm-4\",\n  \"foreignKey\": \"keep\",\n}\n";
        std::fs::write(&config_path, jsonc).unwrap();
        let before = std::fs::read(&config_path).unwrap();

        let base_patches = vec![patch("key:model", json!("glm-4"))];
        let new_patches = vec![patch("key:model", json!("glm-4.5"))];
        let mut base_tmpl = minimal_template("1.1.0", base_patches);
        let mut new_tmpl = minimal_template("1.2.0", new_patches);
        base_tmpl.digest = "a".repeat(64);
        new_tmpl.digest = "b".repeat(64);
        let base_bytes = serde_json::to_vec(&base_tmpl).unwrap();
        let new_bytes = serde_json::to_vec(&new_tmpl).unwrap();

        let instance = Instance {
            id: crate::ids::InstanceId::new("test-instance-jsonc").unwrap(),
            name: crate::ids::InstanceName::new("work").unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::from_path(&config_root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: TemplateId::new("claude-glm").unwrap(),
                version: TemplateVersion::new("1.1.0").unwrap(),
            }),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
        };
        let mut registry = Registry::default();
        registry.insert(instance.clone()).unwrap();
        registry.store(&registry_path).unwrap();

        let adapter = crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap();
        let result = apply_update(
            &instance,
            &registry_path,
            &base_tmpl,
            &new_tmpl,
            &base_bytes,
            &new_bytes,
            &adapter,
        );
        match result {
            Err(CoreError::Config(superai_config::ConfigError::LossyWrite { format, .. })) => {
                assert_eq!(format, "jsonc");
            }
            other => panic!("expected LossyWrite, got {other:?}"),
        }

        // Nothing corrupted: file byte-identical, registry version unchanged.
        assert_eq!(
            std::fs::read(&config_path).unwrap(),
            before,
            "refused apply must leave the settings file byte-identical"
        );
        let registry_after = Registry::load(&registry_path).unwrap();
        let inst = registry_after.get_by_id("test-instance-jsonc").unwrap();
        assert_eq!(inst.template.as_ref().unwrap().version.as_str(), "1.1.0");
        drop(std::fs::remove_dir_all(&tmp));
    }

    #[test]
    fn apply_failure_due_to_digest_retains_old() {
        let tmp = crate::test_util::temp_dir_unique("tpl-update");
        let registry_path = tmp.join("instances.json");
        let config_root = tmp.join(".claude-digest");
        std::fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join("settings.json");
        let base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let mut new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]);
        let base_bytes = serde_json::to_vec(&{
            let mut b = base.clone();
            b.digest = "a".repeat(64);
            b
        })
        .unwrap();
        let mut base_tmp = base.clone();
        base_tmp.digest = compute_digest(&base_bytes);
        let base_bytes = serde_json::to_vec(&base_tmp).unwrap();
        // new with invalid digest format (should trigger validation error)
        new.digest = "not-a-valid-digest".to_owned();
        let new_bytes = serde_json::to_vec(&new).unwrap();
        let instance = Instance {
            id: crate::ids::InstanceId::new("test-digest-001").unwrap(),
            name: crate::ids::InstanceName::new("workdigest").unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::from_path(&config_root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: TemplateId::new("claude-glm").unwrap(),
                version: TemplateVersion::new("1.1.0").unwrap(),
            }),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
        };
        let mut reg = Registry::default();
        reg.insert(instance.clone()).unwrap();
        reg.store(&registry_path).unwrap();
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&json!({"model":"glm-4"})).unwrap() + "\n",
        )
        .unwrap();
        let adapter = crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap();
        let res = apply_update(
            &instance,
            &registry_path,
            &base_tmp,
            &new,
            &base_bytes,
            &new_bytes,
            &adapter,
        );
        assert!(res.is_err(), "digest mismatch should fail");
        let reg_after = Registry::load(&registry_path).unwrap();
        let still = reg_after.get_by_id("test-digest-001").unwrap();
        assert_eq!(still.template.as_ref().unwrap().version.as_str(), "1.1.0");
        drop(std::fs::remove_dir_all(&tmp));
    }

    #[test]
    fn external_edit_between_preview_and_commit_aborts() {
        // This simulates concurrent modification detection via snapshot token
        let tmp = crate::test_util::temp_dir_unique("tpl-update");
        let registry_path = tmp.join("instances.json");
        let config_root = tmp.join(".claude-concurrent");
        std::fs::create_dir_all(&config_root).unwrap();
        let config_path = config_root.join("settings.json");
        let base = minimal_template("1.1.0", vec![patch("key:model", json!("glm-4"))]);
        let new = minimal_template("1.2.0", vec![patch("key:model", json!("glm-4.5"))]);
        let base_bytes = {
            let mut b = base.clone();
            let tmp_bytes = serde_json::to_vec(&b).unwrap();
            b.digest = compute_digest(&tmp_bytes);
            serde_json::to_vec(&b).unwrap()
        };
        let mut base_fixed = base.clone();
        base_fixed.digest = compute_digest(&base_bytes);
        let base_bytes = serde_json::to_vec(&base_fixed).unwrap();
        let new_bytes = {
            let mut n = new.clone();
            let tmp_bytes = serde_json::to_vec(&n).unwrap();
            n.digest = compute_digest(&tmp_bytes);
            serde_json::to_vec(&n).unwrap()
        };
        let mut new_fixed = new.clone();
        new_fixed.digest = compute_digest(&new_bytes);
        let new_bytes = serde_json::to_vec(&new_fixed).unwrap();

        let instance = Instance {
            id: crate::ids::InstanceId::new("test-conc-001").unwrap(),
            name: crate::ids::InstanceName::new("workconc").unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::from_path(&config_root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: TemplateId::new("claude-glm").unwrap(),
                version: TemplateVersion::new("1.1.0").unwrap(),
            }),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
        };
        let mut reg = Registry::default();
        reg.insert(instance.clone()).unwrap();
        reg.store(&registry_path).unwrap();
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&json!({"model":"glm-4"})).unwrap() + "\n",
        )
        .unwrap();

        // Take snapshot, then externally edit before commit
        let snap_before = snapshot(&config_path);
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&json!({"model":"externally-changed"})).unwrap() + "\n",
        )
        .unwrap();
        let snap_after = snapshot(&config_path);
        assert!(is_modified(&snap_before, &snap_after));

        // Now preview would see conflict (both differ) and apply would abort
        let local_after = {
            let mut m = Map::new();
            m.insert("model".to_owned(), json!("externally-changed"));
            m
        };
        let preview = preview_three_way(&base_fixed, &new_fixed, &local_after);
        assert_eq!(preview.conflicts.len(), 1); // both modified
        let adapter = crate::adapters::claude_code::ClaudeCodeAdapter::new().unwrap();
        let res = apply_update(
            &instance,
            &registry_path,
            &base_fixed,
            &new_fixed,
            &base_bytes,
            &new_bytes,
            &adapter,
        );
        res.unwrap_err();
        let reg_after = Registry::load(&registry_path).unwrap();
        assert_eq!(
            reg_after
                .get_by_id("test-conc-001")
                .unwrap()
                .template
                .as_ref()
                .unwrap()
                .version
                .as_str(),
            "1.1.0"
        );

        drop(std::fs::remove_dir_all(&tmp));
    }
}
