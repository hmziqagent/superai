//! Operation executor (DOC-02).
//!
//! Applies a typed [`Operation`] through the codec layer, enforcing every
//! declared policy before any mutation happens:
//!
//! - `owned_keys` — the operation's addressed key must fall inside the
//!   declared owned set (prefix paths count, mirroring template selector
//!   ownership). An empty set owns nothing and rejects everything:
//!   ownership is declared, never assumed.
//! - `expected_old` — the current value at the selector (or its absence)
//!   must match the expectation, else a typed conflict error and no write.
//! - `duplicate_handling` — per declared mode when the edit would hit an
//!   existing entry or identity item.
//! - `create_parent` — missing parents are only created when allowed.
//! - `redaction_policy` — previews and errors render values redacted when
//!   the policy demands it or the value is secret-shaped.
//!
//! Two entry points:
//! - [`apply_to_value`] — the policy-enforcing core over a semantic value
//!   tree (used by callers that already hold the parsed document, e.g. the
//!   three-way template update).
//! - [`apply`] — file level: fresh read through the codec for the document
//!   kind, policy enforcement, mutation, and write back through the same
//!   codec only when the semantic value changed (no-op byte identity).
//!   JSONC/YAML changing writes keep their `LossyWrite` refusals; TOML
//!   edits go through `toml_edit` so comments survive; text fragments
//!   accept managed-span operations only (DOC-08).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};
use toml_edit::{DocumentMut, Item};

use crate::document::{
    DocumentKind, DuplicateHandling, EditOperation, Operation, RedactionPolicy, Selector,
};
use crate::error::{ConfigError, Result};

/// Outcome of applying one operation through the executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationOutcome {
    /// Whether the document's semantic value changed (a write happened or
    /// is warranted).
    pub changed: bool,
    /// Redacted human-readable summary of the edit; never contains
    /// secret-shaped values or values covered by the redaction policy.
    pub redacted_summary: String,
    /// Codec warnings (e.g. DOC-10 formatting-change notes emitted when a
    /// changing write must reformat surrounding layout).
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Redaction rendering
// ---------------------------------------------------------------------------

const SECRET_MARKERS: [&str; 8] = [
    "api_key",
    "apikey",
    "secret",
    "token",
    "password",
    "authorization",
    "bearer",
    "sk-",
];

/// Whether a rendered value looks secret-shaped (DOC-02 redaction policy).
fn is_secret_shaped(rendered: &str) -> bool {
    let lower = rendered.to_ascii_lowercase();
    SECRET_MARKERS.iter().any(|marker| lower.contains(marker))
}

/// Render a value for summaries/errors per the operation's redaction policy.
fn render_value(op: &Operation, value: &Value) -> String {
    if matches!(
        op.redaction_policy,
        RedactionPolicy::RedactValue | RedactionPolicy::Full
    ) {
        return "[REDACTED]".to_owned();
    }
    let raw = serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"));
    if is_secret_shaped(&raw) {
        "[REDACTED]".to_owned()
    } else {
        raw
    }
}

/// Render a selector for summaries/errors per the operation's redaction
/// policy.
fn render_selector(op: &Operation, selector: &str) -> String {
    if matches!(
        op.redaction_policy,
        RedactionPolicy::RedactKey | RedactionPolicy::Full
    ) {
        "[REDACTED]".to_owned()
    } else {
        selector.to_owned()
    }
}

fn render_slot(op: &Operation, value: Option<&Value>) -> String {
    match value {
        Some(v) => render_value(op, v),
        None => "(absent)".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Ownership
// ---------------------------------------------------------------------------

/// Canonical addressed form of a selector: dotted path for `Key` and
/// `TomlTable`, typed string otherwise.
fn selector_address(selector: &Selector) -> String {
    match selector {
        Selector::Key(k) => k
            .split('.')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("."),
        Selector::TomlTable(parts) => parts.join("."),
        other => other.to_typed_string(),
    }
}

/// Addressed keys this operation touches (one per variant; merged keys are
/// checked separately inside [`merge_into`]).
fn addressed_keys(op: &Operation) -> Vec<String> {
    let base = selector_address(op.selector());
    match &op.kind {
        EditOperation::InsertEntry { key, .. } => {
            let joined = if base.is_empty() {
                key.clone()
            } else {
                format!("{base}.{key}")
            };
            vec![joined]
        }
        _ => vec![base],
    }
}

/// String forms an owned-key declaration can take (raw, key-only, typed).
fn candidate_forms(declared: &str) -> Vec<String> {
    let mut forms = vec![declared.to_owned()];
    if let Ok(parsed) = Selector::parse(declared) {
        match &parsed {
            Selector::Key(k) if k != declared => {
                forms.push(k.clone());
                forms.push(format!("key:{k}"));
            }
            Selector::ManagedSpan(name) if name != declared => {
                forms.push(name.clone());
                forms.push(format!("span:{name}"));
            }
            _ => forms.push(parsed.to_typed_string()),
        }
    }
    forms.sort();
    forms.dedup();
    forms
}

/// Whether `addressed` falls inside an owned-key declaration (exact or
/// prefix path, mirroring template selector ownership).
fn owned_covers(owned: &str, addressed: &str) -> bool {
    let owned_forms = candidate_forms(owned);
    let addressed_forms = candidate_forms(addressed);
    addressed_forms.iter().any(|a| {
        owned_forms
            .iter()
            .any(|o| a == o || a.starts_with(&format!("{o}.")) || o.starts_with(&format!("{a}.")))
    })
}

/// Whether every addressed key of `op` is inside `op.owned_keys`.
fn ownership_holds(op: &Operation) -> bool {
    !op.owned_keys.is_empty()
        && addressed_keys(op).iter().all(|addressed| {
            op.owned_keys
                .iter()
                .any(|owned| owned_covers(owned, addressed))
        })
}

fn selector_string(op: &Operation) -> String {
    render_selector(op, &op.selector().to_typed_string())
}

/// Enforce `owned_keys` (DOC-02): reject selectors outside the owned set.
fn check_ownership(path: &Path, op: &Operation) -> Result<()> {
    if ownership_holds(op) {
        return Ok(());
    }
    Err(ConfigError::not_owned(path, selector_string(op)))
}

// ---------------------------------------------------------------------------
// expected_old
// ---------------------------------------------------------------------------

/// Enforce `expected_old` (DOC-02): mismatch is a typed conflict, no write.
fn check_expected_old(path: &Path, op: &Operation, current: Option<&Value>) -> Result<()> {
    let Some(expected) = &op.expected_old else {
        return Ok(());
    };
    let matches = match (expected, current) {
        (Some(exp), Some(cur)) => exp == cur,
        (None, None) => true,
        _ => false,
    };
    if matches {
        return Ok(());
    }
    Err(ConfigError::operation_conflict(
        path,
        selector_string(op),
        render_slot(op, expected.as_ref()),
        render_slot(op, current),
    ))
}

// ---------------------------------------------------------------------------
// Navigation over the semantic value tree
// ---------------------------------------------------------------------------

/// Navigate `segments` read-only. `Err` when an intermediate segment is not
/// an object; `Ok(None)` when the path is missing.
fn navigate_ref<'a>(
    node: &'a Value,
    segments: &[String],
    path: &Path,
    selector: &str,
) -> Result<Option<&'a Value>> {
    let Some((first, rest)) = segments.split_first() else {
        return Ok(Some(node));
    };
    let Value::Object(map) = node else {
        return Err(ConfigError::unsupported_operation(
            path,
            selector,
            format!("segment `{first}` is not an object"),
        ));
    };
    match map.get(first) {
        Some(child) => navigate_ref(child, rest, path, selector),
        None => Ok(None),
    }
}

/// Navigate `segments` mutably, creating missing objects when `create` is
/// allowed (DOC-02 `create_parent`).
fn navigate_mut<'a>(
    node: &'a mut Value,
    segments: &[String],
    create: bool,
    path: &Path,
    selector: &str,
) -> Result<&'a mut Value> {
    let Some((first, rest)) = segments.split_first() else {
        return Ok(node);
    };
    let Value::Object(map) = node else {
        return Err(ConfigError::unsupported_operation(
            path,
            selector,
            format!("segment `{first}` is not an object"),
        ));
    };
    if !map.contains_key(first) {
        if !create {
            return Err(ConfigError::parent_missing(path, selector));
        }
        map.insert(first.clone(), Value::Object(Map::new()));
    }
    match map.get_mut(first) {
        Some(child) => navigate_mut(child, rest, create, path, selector),
        None => Err(ConfigError::unsupported_operation(
            path,
            selector,
            format!("segment `{first}` vanished during navigation"),
        )),
    }
}

/// Split a `Key` (or `TomlTable`, joined as a dotted path) selector into
/// (parent segments, leaf key).
fn key_target(selector: &Selector, path: &Path, op: &Operation) -> Result<(Vec<String>, String)> {
    let raw = match selector {
        Selector::Key(k) => k.as_str(),
        Selector::TomlTable(parts) => &parts.join("."),
        other => {
            return Err(ConfigError::unsupported_operation(
                path,
                render_selector(op, &other.to_typed_string()),
                "this operation requires a Key selector for structured documents",
            ));
        }
    };
    let segments: Vec<String> = raw
        .split('.')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let Some(leaf) = segments.last().cloned() else {
        return Err(ConfigError::unsupported_operation(
            path,
            selector_string(op),
            "key selector is empty",
        ));
    };
    let parent = segments
        .iter()
        .take(segments.len().saturating_sub(1))
        .cloned()
        .collect();
    Ok((parent, leaf))
}

fn full_key_segments(selector: &Selector, path: &Path, op: &Operation) -> Result<Vec<String>> {
    let (parent, leaf) = key_target(selector, path, op)?;
    let mut segments = parent;
    segments.push(leaf);
    Ok(segments)
}

// ---------------------------------------------------------------------------
// Variant handlers (all policy checks run before any mutation)
// ---------------------------------------------------------------------------

fn serde_variant_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// `Set` / `InsertEntry` / `EnableDisable` at a key path.
fn upsert_leaf(
    path: &Path,
    op: &Operation,
    root: &mut Value,
    parent_segments: &[String],
    leaf: &str,
    new_value: &Value,
) -> Result<bool> {
    // Read-only policy checks.
    let selector = op.selector().to_typed_string();
    let parent = navigate_ref(root, parent_segments, path, &selector)?;
    let parent_ok = match parent {
        None => op.create_parent,
        Some(Value::Object(_)) => true,
        Some(_) => false,
    };
    if !parent_ok {
        return Err(match parent {
            None => ConfigError::parent_missing(path, selector_string(op)),
            Some(_) => ConfigError::unsupported_operation(
                path,
                selector_string(op),
                "selector parent is not an object".to_owned(),
            ),
        });
    }
    let current = parent.and_then(|p| p.get(leaf));
    check_expected_old(path, op, current)?;
    if current.is_some() {
        match op.duplicate_handling {
            DuplicateHandling::Error => {
                return Err(ConfigError::duplicate_rejected(
                    path,
                    selector_string(op),
                    "entry already exists and duplicate_handling is Error",
                ));
            }
            DuplicateHandling::KeepFirst => return Ok(false),
            DuplicateHandling::Overwrite | DuplicateHandling::Append => {}
        }
    }
    if matches!(op.kind, EditOperation::EnableDisable { .. })
        && let Some(existing) = current
        && !existing.is_boolean()
    {
        return Err(ConfigError::operation_conflict(
            path,
            selector_string(op),
            "boolean".to_owned(),
            serde_variant_name(existing).to_owned(),
        ));
    }

    // Mutation (every policy has passed).
    let parent_mut = navigate_mut(root, parent_segments, op.create_parent, path, &selector)?;
    let Value::Object(map) = parent_mut else {
        return Err(ConfigError::unsupported_operation(
            path,
            selector,
            "selector parent is not an object".to_owned(),
        ));
    };
    map.insert(leaf.to_owned(), new_value.clone());
    Ok(true)
}

/// Remove at a key path (idempotent for absent entries).
fn remove_leaf(
    path: &Path,
    op: &Operation,
    root: &mut Value,
    parent_segments: &[String],
    leaf: &str,
) -> Result<bool> {
    let selector = op.selector().to_typed_string();
    let parent = navigate_ref(root, parent_segments, path, &selector)?;
    let current = parent.and_then(|p| p.get(leaf));
    check_expected_old(path, op, current)?;
    if current.is_none() {
        return Ok(false);
    }
    let parent_mut = navigate_mut(root, parent_segments, false, path, &selector)?;
    if let Value::Object(map) = parent_mut {
        map.remove(leaf);
        return Ok(true);
    }
    Ok(false)
}

/// Merge owned fields into the object at `segments`, retaining foreign keys.
fn merge_into(
    path: &Path,
    op: &Operation,
    root: &mut Value,
    segments: &[String],
    merge_value: &Value,
) -> Result<bool> {
    let Value::Object(merge_map) = merge_value else {
        return Err(ConfigError::unsupported_operation(
            path,
            selector_string(op),
            "merge value must be an object".to_owned(),
        ));
    };
    // Merged keys must each be owned (DOC-02).
    let target_address = segments.join(".");
    for key in merge_map.keys() {
        let sub_address = if target_address.is_empty() {
            key.clone()
        } else {
            format!("{target_address}.{key}")
        };
        if !op
            .owned_keys
            .iter()
            .any(|owned| owned_covers(owned, &sub_address))
        {
            return Err(ConfigError::not_owned(path, selector_string(op)));
        }
    }

    let selector = op.selector().to_typed_string();
    let target = navigate_ref(root, segments, path, &selector)?;
    let target_ok = match target {
        None => op.create_parent,
        Some(Value::Object(_)) => true,
        Some(_) => false,
    };
    if !target_ok {
        return Err(match target {
            None => ConfigError::parent_missing(path, selector_string(op)),
            Some(_) => ConfigError::unsupported_operation(
                path,
                selector_string(op),
                "merge target is not an object".to_owned(),
            ),
        });
    }
    check_expected_old(path, op, target)?;

    let target_mut = navigate_mut(root, segments, op.create_parent, path, &selector)?;
    let Value::Object(map) = target_mut else {
        return Err(ConfigError::unsupported_operation(
            path,
            selector,
            "merge target is not an object".to_owned(),
        ));
    };
    for (key, value) in merge_map {
        map.insert(key.clone(), value.clone());
    }
    Ok(true)
}

/// Shared read-only array resolution for identity/directory operations:
/// `Ok(None)` means the array is absent (creation allowed), `Err` when the
/// selector does not address an array or the parent may not be created.
fn resolve_array<'a>(
    path: &Path,
    op: &Operation,
    root: &'a Value,
    segments: &[String],
) -> Result<Option<&'a Vec<Value>>> {
    let selector = op.selector().to_typed_string();
    match navigate_ref(root, segments, path, &selector)? {
        None => {
            if op.create_parent {
                Ok(None)
            } else {
                Err(ConfigError::parent_missing(path, selector_string(op)))
            }
        }
        Some(Value::Array(items)) => Ok(Some(items)),
        Some(_) => Err(ConfigError::unsupported_operation(
            path,
            selector_string(op),
            "selector does not address an array".to_owned(),
        )),
    }
}

/// Ensure the array at (parent, leaf) exists and return it mutably.
fn ensure_array_mut<'a>(
    root: &'a mut Value,
    parent_segments: &[String],
    leaf: &str,
    create: bool,
    path: &Path,
    selector: &str,
) -> Result<&'a mut Vec<Value>> {
    let parent = navigate_mut(root, parent_segments, create, path, selector)?;
    let Value::Object(map) = parent else {
        return Err(ConfigError::unsupported_operation(
            path,
            selector,
            "selector parent is not an object".to_owned(),
        ));
    };
    if !map.contains_key(leaf) {
        if !create {
            return Err(ConfigError::parent_missing(path, selector));
        }
        map.insert(leaf.to_owned(), Value::Array(Vec::new()));
    }
    match map.get_mut(leaf) {
        Some(Value::Array(items)) => Ok(items),
        _ => Err(ConfigError::unsupported_operation(
            path,
            selector,
            "selector does not address an array".to_owned(),
        )),
    }
}

/// Append (or per duplicate mode, upsert) an identity-keyed array item.
fn append_identity_item(
    path: &Path,
    op: &Operation,
    root: &mut Value,
    segments: &[String],
    item: &Value,
    identity_key: &str,
) -> Result<bool> {
    let Some(identity) = item.get(identity_key) else {
        return Err(ConfigError::unsupported_operation(
            path,
            selector_string(op),
            format!("item lacks the identity field `{identity_key}`"),
        ));
    };
    let array = resolve_array(path, op, root, segments)?;
    let existing = array.and_then(|items| {
        items
            .iter()
            .position(|it| it.get(identity_key) == Some(identity))
    });
    let existing_value = existing.and_then(|idx| array.and_then(|items| items.get(idx)));
    check_expected_old(path, op, existing_value)?;

    let selector = op.selector().to_typed_string();
    let (parent, leaf) = split_last(segments);
    let leaf = leaf.to_owned();
    let Some(idx) = existing else {
        let items = ensure_array_mut(root, &parent, &leaf, op.create_parent, path, &selector)?;
        items.push(item.clone());
        return Ok(true);
    };
    match op.duplicate_handling {
        DuplicateHandling::Error => Err(ConfigError::duplicate_rejected(
            path,
            selector_string(op),
            format!("an item with the same `{identity_key}` already exists"),
        )),
        DuplicateHandling::KeepFirst => Ok(false),
        DuplicateHandling::Overwrite => {
            let items = ensure_array_mut(root, &parent, &leaf, op.create_parent, path, &selector)?;
            match items.get_mut(idx) {
                Some(slot) => {
                    *slot = item.clone();
                    Ok(true)
                }
                None => Ok(false),
            }
        }
        DuplicateHandling::Append => {
            let items = ensure_array_mut(root, &parent, &leaf, op.create_parent, path, &selector)?;
            items.push(item.clone());
            Ok(true)
        }
    }
}

/// Ensure a string entry exists in the array at `segments`.
fn ensure_dir_entry(
    path: &Path,
    op: &Operation,
    root: &mut Value,
    segments: &[String],
    entry: &str,
) -> Result<bool> {
    let selector = op.selector().to_typed_string();
    let current = navigate_ref(root, segments, path, &selector)?;
    check_expected_old(path, op, current)?;
    if let Some(items) = resolve_array(path, op, root, segments)?
        && items.iter().any(|v| v.as_str() == Some(entry))
    {
        return Ok(false);
    }
    let (parent, leaf) = split_last(segments);
    let items = ensure_array_mut(root, &parent, leaf, op.create_parent, path, &selector)?;
    items.push(Value::String(entry.to_owned()));
    Ok(true)
}

fn split_last(segments: &[String]) -> (Vec<String>, &str) {
    match segments.split_last() {
        Some((leaf, parent)) => (parent.to_vec(), leaf.as_str()),
        None => (Vec::new(), ""),
    }
}

/// Set/Remove at an `Index` or `Identity` selector on the root array.
fn root_array_edit(
    path: &Path,
    op: &Operation,
    root: &mut Value,
    remove: bool,
    new_value: Option<&Value>,
) -> Result<bool> {
    let selector = op.selector().to_typed_string();
    let Value::Array(items) = root else {
        return Err(ConfigError::unsupported_operation(
            path,
            selector,
            "Index/Identity selectors address the document root, which is not an array",
        ));
    };
    let found = match op.selector() {
        Selector::Index(idx) => (*idx < items.len()).then_some(*idx),
        Selector::Identity { key, value: want } => items
            .iter()
            .position(|it| it.get(key).and_then(Value::as_str) == Some(want.as_str())),
        _ => {
            return Err(ConfigError::unsupported_operation(
                path,
                selector,
                "unsupported selector for root array editing".to_owned(),
            ));
        }
    };
    check_expected_old(
        path,
        op,
        found.and_then(|idx| root.as_array().and_then(|a| a.get(idx))),
    )?;

    let Some(idx) = found else {
        return Ok(false);
    };
    let Some(items) = root.as_array_mut() else {
        return Ok(false);
    };
    if remove {
        if idx < items.len() {
            items.remove(idx);
            return Ok(true);
        }
        return Ok(false);
    }
    let Some(replacement) = new_value else {
        return Ok(false);
    };
    match items.get_mut(idx) {
        Some(slot) => {
            *slot = replacement.clone();
            Ok(true)
        }
        None => Ok(false),
    }
}

// ---------------------------------------------------------------------------
// Core: apply to a semantic value tree
// ---------------------------------------------------------------------------

/// Enforce every declared policy of `op` against `value` and apply the edit
/// in place (DOC-02 executor core).
///
/// On any policy violation the value is left untouched and a typed error is
/// returned. `path` is used for error context only (pass the target file's
/// path, or a synthetic name for in-memory documents).
pub fn apply_to_value(path: &Path, value: &mut Value, op: &Operation) -> Result<OperationOutcome> {
    check_ownership(path, op)?;
    let original = value.clone();

    match &op.kind {
        EditOperation::Set {
            selector,
            value: new_value,
        } => {
            apply_set_variant(path, op, value, selector, new_value)?;
        }
        EditOperation::InsertEntry {
            selector,
            key,
            value: new_value,
        } => {
            let parent_segments = full_key_segments(selector, path, op)?;
            upsert_leaf(path, op, value, &parent_segments, key, new_value)?;
        }
        EditOperation::Remove { selector } => {
            apply_remove_variant(path, op, value, selector)?;
        }
        EditOperation::Merge {
            selector,
            value: merge_value,
        } => {
            let segments = full_key_segments(selector, path, op)?;
            merge_into(path, op, value, &segments, merge_value)?;
        }
        EditOperation::EnableDisable { selector, enabled } => {
            let (parent, leaf) = key_target(selector, path, op)?;
            upsert_leaf(path, op, value, &parent, &leaf, &Value::Bool(*enabled))?;
        }
        EditOperation::AppendIdentityItem {
            selector,
            value: item,
            identity_key,
        } => {
            let segments = full_key_segments(selector, path, op)?;
            append_identity_item(path, op, value, &segments, item, identity_key)?;
        }
        EditOperation::EnsureDirEntry {
            selector,
            path: entry,
        } => {
            let segments = full_key_segments(selector, path, op)?;
            ensure_dir_entry(path, op, value, &segments, entry)?;
        }
    }

    let changed = *value != original;
    let summary = summarize(op, &original, value);
    Ok(OperationOutcome {
        changed,
        redacted_summary: summary,
        warnings: Vec::new(),
    })
}

fn apply_set_variant(
    path: &Path,
    op: &Operation,
    value: &mut Value,
    selector: &Selector,
    new_value: &Value,
) -> Result<()> {
    match selector {
        Selector::Key(_) | Selector::TomlTable(_) => {
            let (parent, leaf) = key_target(selector, path, op)?;
            upsert_leaf(path, op, value, &parent, &leaf, new_value)?;
            Ok(())
        }
        Selector::Index(_) | Selector::Identity { .. } => {
            root_array_edit(path, op, value, false, Some(new_value))?;
            Ok(())
        }
        Selector::ManagedSpan(_) => Err(ConfigError::unsupported_operation(
            path,
            selector_string(op),
            "managed spans require the text fragment codec (DOC-08)".to_owned(),
        )),
    }
}

fn apply_remove_variant(
    path: &Path,
    op: &Operation,
    value: &mut Value,
    selector: &Selector,
) -> Result<()> {
    match selector {
        Selector::Key(_) | Selector::TomlTable(_) => {
            let (parent, leaf) = key_target(selector, path, op)?;
            remove_leaf(path, op, value, &parent, &leaf)?;
            Ok(())
        }
        Selector::Index(_) | Selector::Identity { .. } => {
            root_array_edit(path, op, value, true, None)?;
            Ok(())
        }
        Selector::ManagedSpan(_) => Err(ConfigError::unsupported_operation(
            path,
            selector_string(op),
            "managed spans require the text fragment codec (DOC-08)".to_owned(),
        )),
    }
}

/// Value at the operation's selector, for before/after summaries.
fn value_at<'a>(value: &'a Value, selector: &Selector) -> Option<&'a Value> {
    let summary_path = Path::new("(summary)");
    match selector {
        Selector::Key(_) | Selector::TomlTable(_) => {
            let segments: Vec<String> = selector_address(selector)
                .split('.')
                .map(str::to_owned)
                .collect();
            navigate_ref(value, &segments, summary_path, &selector.to_typed_string())
                .ok()
                .flatten()
        }
        Selector::Index(idx) => value.as_array().and_then(|a| a.get(*idx)),
        Selector::Identity { key, value: want } => value.as_array().and_then(|a| {
            a.iter()
                .find(|it| it.get(key).and_then(Value::as_str) == Some(want.as_str()))
        }),
        Selector::ManagedSpan(_) => None,
    }
}

fn summarize(op: &Operation, before: &Value, after: &Value) -> String {
    let verb = match &op.kind {
        EditOperation::Set { .. } => "set",
        EditOperation::InsertEntry { .. } => "insert",
        EditOperation::Remove { .. } => "remove",
        EditOperation::Merge { .. } => "merge",
        EditOperation::EnableDisable { enabled, .. } if *enabled => "enable",
        EditOperation::EnableDisable { .. } => "disable",
        EditOperation::AppendIdentityItem { .. } => "append",
        EditOperation::EnsureDirEntry { .. } => "ensure",
    };
    let selector = render_selector(op, &op.selector().to_typed_string());
    format!(
        "{verb} {selector}: {} -> {}",
        render_slot(op, value_at(before, op.selector())),
        render_slot(op, value_at(after, op.selector()))
    )
}

// ---------------------------------------------------------------------------
// File-level application through the codecs
// ---------------------------------------------------------------------------

/// Apply `op` to the document at `path` through the codec for `kind`.
///
/// Fresh read, policy enforcement, mutation, write back only when the
/// semantic value changed (no-op byte identity). See the module docs for
/// per-kind guarantees; opaque documents are refused.
pub fn apply(path: &Path, kind: DocumentKind, op: &Operation) -> Result<OperationOutcome> {
    match kind {
        DocumentKind::StrictJson => apply_json_family(path, op, JsonFamily::Strict),
        DocumentKind::JsonC => apply_json_family(path, op, JsonFamily::JsonC),
        DocumentKind::Yaml => apply_json_family(path, op, JsonFamily::Yaml),
        DocumentKind::Toml => apply_toml(path, op),
        DocumentKind::Env => apply_env(path, op),
        DocumentKind::TextFragment => apply_text_fragment(path, op),
        DocumentKind::Opaque => Err(ConfigError::unsupported_operation(
            path,
            op.selector().to_typed_string(),
            "opaque documents are read-only".to_owned(),
        )),
    }
}

#[derive(Debug, Clone, Copy)]
enum JsonFamily {
    Strict,
    JsonC,
    Yaml,
}

fn read_text_or_empty(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(ConfigError::io(path, e)),
    }
}

fn apply_json_family(path: &Path, op: &Operation, family: JsonFamily) -> Result<OperationOutcome> {
    let mut value = match family {
        JsonFamily::Strict => crate::json::load_value(path)?,
        JsonFamily::JsonC => crate::jsonc::load_value(path)?,
        JsonFamily::Yaml => crate::yaml::load_value(path)?,
    };
    let old_text = read_text_or_empty(path)?;
    let mut outcome = apply_to_value(path, &mut value, op)?;
    if outcome.changed {
        match family {
            JsonFamily::Strict => crate::json::store_value(path, &value)?,
            JsonFamily::JsonC => crate::jsonc::store_value(path, &value)?,
            JsonFamily::Yaml => crate::yaml::store_value(path, &value)?,
        }
        let note = match family {
            JsonFamily::Strict => crate::json::formatting_change_warning(&old_text),
            JsonFamily::JsonC => crate::jsonc::formatting_change_warning(&old_text),
            JsonFamily::Yaml => None,
        };
        if let Some(note) = note {
            outcome.warnings.push(note.to_owned());
        }
    }
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// TOML path (comments preserved via toml_edit)
// ---------------------------------------------------------------------------

/// Convert a `toml_edit` document to its semantic JSON value (used for
/// policy checks and semantic validation).
pub fn toml_document_to_value(doc: &DocumentMut) -> Value {
    table_to_value(doc.as_table())
}

fn table_to_value(table: &toml_edit::Table) -> Value {
    let mut map = Map::new();
    for (key, item) in table {
        if item.is_none() {
            continue;
        }
        map.insert(key.to_owned(), item_to_value(item));
    }
    Value::Object(map)
}

fn item_to_value(item: &Item) -> Value {
    match item {
        Item::Value(v) => toml_value_to_value(v),
        Item::Table(t) => table_to_value(t),
        Item::ArrayOfTables(a) => Value::Array(a.iter().map(table_to_value).collect()),
        Item::None => Value::Null,
    }
}

fn toml_value_to_value(v: &toml_edit::Value) -> Value {
    use toml_edit::Value as Tv;
    match v {
        Tv::String(s) => Value::String(s.value().to_owned()),
        Tv::Integer(i) => Value::Number((*i.value()).into()),
        Tv::Float(f) => serde_json::Number::from_f64(*f.value()).map_or(Value::Null, Value::Number),
        Tv::Boolean(b) => Value::Bool(*b.value()),
        Tv::Datetime(d) => Value::String(d.to_string()),
        Tv::Array(a) => Value::Array(a.iter().map(toml_value_to_value).collect()),
        Tv::InlineTable(t) => {
            let mut map = Map::new();
            for (key, value) in t {
                map.insert(key.to_owned(), toml_value_to_value(value));
            }
            Value::Object(map)
        }
    }
}

fn json_value_to_toml_item(value: &Value, path: &Path, selector: &str) -> Result<Item> {
    use toml_edit::Value as Tv;
    let item = match value {
        Value::Null => {
            return Err(ConfigError::unsupported_operation(
                path,
                selector,
                "toml cannot represent null".to_owned(),
            ));
        }
        Value::Bool(b) => toml_edit::value(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml_edit::value(i)
            } else {
                toml_edit::value(n.as_f64().unwrap_or_default())
            }
        }
        Value::String(s) => toml_edit::value(s.as_str()),
        Value::Array(items) => {
            let mut arr = toml_edit::Array::new();
            for item in items {
                if let Item::Value(v) = json_value_to_toml_item(item, path, selector)? {
                    arr.push(v);
                }
            }
            Item::Value(Tv::Array(arr))
        }
        Value::Object(map) => {
            let mut inline = toml_edit::InlineTable::new();
            for (key, inner) in map {
                if let Item::Value(v) = json_value_to_toml_item(inner, path, selector)? {
                    inline.insert(key.as_str(), v);
                }
            }
            Item::Value(Tv::InlineTable(inline))
        }
    };
    Ok(item)
}

fn toml_navigate_table<'a>(
    table: &'a mut toml_edit::Table,
    segments: &[String],
    create: bool,
    path: &Path,
    selector: &str,
) -> Result<&'a mut toml_edit::Table> {
    let Some((first, rest)) = segments.split_first() else {
        return Ok(table);
    };
    if !table.contains_key(first) {
        if !create {
            return Err(ConfigError::parent_missing(path, selector));
        }
        table.insert(first.as_str(), Item::Table(toml_edit::Table::new()));
    }
    match table.get_mut(first) {
        Some(Item::Table(t)) => toml_navigate_table(t, rest, create, path, selector),
        Some(_) => Err(ConfigError::unsupported_operation(
            path,
            selector,
            format!("segment `{first}` is not a table"),
        )),
        None => Err(ConfigError::unsupported_operation(
            path,
            selector,
            format!("segment `{first}` vanished during navigation"),
        )),
    }
}

fn toml_segments(selector: &Selector, path: &Path, op: &Operation) -> Result<Vec<String>> {
    match selector {
        Selector::Key(_) | Selector::TomlTable(_) => {
            let address = selector_address(selector);
            let segments: Vec<String> = address.split('.').map(str::to_owned).collect();
            if segments.is_empty() || segments.iter().any(String::is_empty) {
                return Err(ConfigError::unsupported_operation(
                    path,
                    selector_string(op),
                    "selector is empty".to_owned(),
                ));
            }
            Ok(segments)
        }
        _ => Err(ConfigError::unsupported_operation(
            path,
            selector_string(op),
            "toml edits require Key or TomlTable selectors".to_owned(),
        )),
    }
}

fn toml_set_at(
    doc: &mut DocumentMut,
    segments: &[String],
    value: &Value,
    create: bool,
    path: &Path,
    selector: &str,
) -> Result<()> {
    let item = json_value_to_toml_item(value, path, selector)?;
    let (parent, leaf) = split_last(segments);
    if leaf.is_empty() {
        return Err(ConfigError::unsupported_operation(
            path,
            selector,
            "selector is empty".to_owned(),
        ));
    }
    let table = toml_navigate_table(doc.as_table_mut(), &parent, create, path, selector)?;
    if table.contains_key(leaf) {
        // Index assignment preserves the existing key's position and decor
        // (comments above it) — `Table::insert` would replace the item
        // wholesale and drop the decor (DOC-04).
        table[leaf] = item;
    } else {
        table.insert(leaf, item);
    }
    Ok(())
}

fn apply_toml(path: &Path, op: &Operation) -> Result<OperationOutcome> {
    // TOML edits are table/key shaped; identity/array operations are refused
    // up front so policy checks and mutation cannot diverge.
    if matches!(
        op.kind,
        EditOperation::AppendIdentityItem { .. } | EditOperation::EnsureDirEntry { .. }
    ) {
        return Err(ConfigError::unsupported_operation(
            path,
            selector_string(op),
            "the toml executor path supports table/key operations only".to_owned(),
        ));
    }
    let mut doc = crate::toml_file::load(path)?;
    let mut working = toml_document_to_value(&doc);
    let mut outcome = apply_to_value(path, &mut working, op)?;
    if !outcome.changed {
        return Ok(outcome);
    }

    toml_apply_mutation(path, op, &mut doc)?;

    let old_text = read_text_or_empty(path)?;
    crate::toml_file::store(path, &doc)?;
    if let Some(note) = crate::toml_file::formatting_change_warning(&old_text) {
        outcome.warnings.push(note.to_owned());
    }
    Ok(outcome)
}

/// Mirror the (already policy-approved) operation onto the `toml_edit`
/// document so comments and decor survive the write (DOC-04).
fn toml_apply_mutation(path: &Path, op: &Operation, doc: &mut DocumentMut) -> Result<()> {
    let selector = op.selector().to_typed_string();
    match &op.kind {
        EditOperation::Set {
            selector: sel,
            value: new_value,
        } => {
            let segments = toml_segments(sel, path, op)?;
            toml_set_at(doc, &segments, new_value, op.create_parent, path, &selector)?;
        }
        EditOperation::InsertEntry {
            selector: sel,
            key,
            value: new_value,
        } => {
            let mut segments = toml_segments(sel, path, op)?;
            segments.push(key.clone());
            toml_set_at(doc, &segments, new_value, op.create_parent, path, &selector)?;
        }
        EditOperation::Remove { selector: sel } => {
            let segments = toml_segments(sel, path, op)?;
            let (parent, leaf) = split_last(&segments);
            if leaf.is_empty() {
                return Err(ConfigError::unsupported_operation(
                    path,
                    selector,
                    "selector is empty".to_owned(),
                ));
            }
            let table = toml_navigate_table(doc.as_table_mut(), &parent, false, path, &selector)?;
            table.remove(leaf);
        }
        EditOperation::Merge {
            selector: sel,
            value: merge_value,
        } => {
            let segments = toml_segments(sel, path, op)?;
            let Value::Object(merge_map) = merge_value else {
                return Err(ConfigError::unsupported_operation(
                    path,
                    selector,
                    "merge value must be an object".to_owned(),
                ));
            };
            for (key, value) in merge_map {
                let mut target = segments.clone();
                target.push(key.clone());
                toml_set_at(doc, &target, value, true, path, &selector)?;
            }
        }
        EditOperation::EnableDisable {
            selector: sel,
            enabled,
        } => {
            let segments = toml_segments(sel, path, op)?;
            toml_set_at(
                doc,
                &segments,
                &Value::Bool(*enabled),
                op.create_parent,
                path,
                &selector,
            )?;
        }
        EditOperation::AppendIdentityItem { .. } | EditOperation::EnsureDirEntry { .. } => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Env path (lexical preservation via env_file::edit)
// ---------------------------------------------------------------------------

fn env_map_to_value(vars: &BTreeMap<String, String>) -> Value {
    Value::Object(
        vars.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    )
}

fn value_to_env_map(
    value: &Value,
    path: &Path,
    op: &Operation,
) -> Result<BTreeMap<String, String>> {
    let Value::Object(map) = value else {
        return Err(ConfigError::unsupported_operation(
            path,
            selector_string(op),
            "env documents must stay flat string maps".to_owned(),
        ));
    };
    let mut out = BTreeMap::new();
    for (key, val) in map {
        let rendered = match val {
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            _ => {
                return Err(ConfigError::unsupported_operation(
                    path,
                    selector_string(op),
                    "env values must be scalars (string, number, boolean)".to_owned(),
                ));
            }
        };
        out.insert(key.clone(), rendered);
    }
    Ok(out)
}

fn apply_env(path: &Path, op: &Operation) -> Result<OperationOutcome> {
    let vars = crate::env_file::load(path)?;
    let mut view = env_map_to_value(&vars);
    let outcome = apply_to_value(path, &mut view, op)?;
    if !outcome.changed {
        return Ok(outcome);
    }
    let new_vars = value_to_env_map(&view, path, op)?;
    // Apply the delta through the lexical-preserving edit path: keys the
    // operation removed are dropped, keys it set (old or new) are written.
    let removed: Vec<String> = vars
        .keys()
        .filter(|k| !new_vars.contains_key(*k))
        .cloned()
        .collect();
    crate::env_file::edit(path, |map| {
        for key in &removed {
            map.remove(key);
        }
        for (key, val) in &new_vars {
            map.insert(key.clone(), val.clone());
        }
    })?;
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Text fragment path (managed spans, DOC-08)
// ---------------------------------------------------------------------------

fn apply_text_fragment(path: &Path, op: &Operation) -> Result<OperationOutcome> {
    check_ownership(path, op)?;
    let codec = crate::span_codec::SpanCodec::default();
    let snap = crate::snapshot::snapshot(path);
    let old_bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(ConfigError::io(path, e)),
    };
    let old_text = std::str::from_utf8(&old_bytes).map_err(|err| {
        ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("fragment is not utf-8: {err}"),
            ),
        )
    })?;
    codec
        .validate(old_text)
        .map_err(|e| e.into_config_error(path))?;

    let Selector::ManagedSpan(span_name) = op.selector() else {
        return Err(ConfigError::unsupported_operation(
            path,
            selector_string(op),
            "text fragments accept managed-span operations only (DOC-08)".to_owned(),
        ));
    };
    // expected_old applies to the span body (DOC-02).
    let current_body = codec
        .span_body(old_text, span_name)
        .map_err(|e| e.into_config_error(path))?;
    check_expected_old(
        path,
        op,
        current_body
            .as_ref()
            .map(|body| Value::String(body.clone()))
            .as_ref(),
    )?;

    let new_text = match &op.kind {
        EditOperation::Set { value, .. } | EditOperation::InsertEntry { value, .. } => {
            let Value::String(body) = value else {
                return Err(ConfigError::unsupported_operation(
                    path,
                    selector_string(op),
                    "span bodies must be strings".to_owned(),
                ));
            };
            let is_insert = matches!(op.kind, EditOperation::InsertEntry { .. });
            let exists = current_body.is_some();
            if is_insert && exists {
                codec.insert_span(old_text, span_name, body) // duplicate span fails closed inside
            } else if exists {
                codec.replace_span(old_text, span_name, body)
            } else {
                codec.insert_span(old_text, span_name, body)
            }
        }
        EditOperation::Remove { .. } => match current_body {
            Some(_) => codec.remove_span(old_text, span_name),
            None => Ok(old_text.to_owned()),
        },
        _ => {
            return Err(ConfigError::unsupported_operation(
                path,
                selector_string(op),
                "text fragments accept managed-span operations only (DOC-08)".to_owned(),
            ));
        }
    }
    .map_err(|e| e.into_config_error(path))?;

    let changed_summary = |after: Option<&str>| {
        let before_value = current_body
            .as_deref()
            .map(|b| Value::String(redact_span_body(op, b)));
        let after_value = after.map(|b| Value::String(redact_span_body(op, b)));
        format!(
            "span {span_name}: {} -> {}",
            render_slot(op, before_value.as_ref()),
            render_slot(op, after_value.as_ref())
        )
    };

    if new_text == old_text {
        return Ok(OperationOutcome {
            changed: false,
            redacted_summary: changed_summary(current_body.as_deref()),
            warnings: Vec::new(),
        });
    }

    // Commit through the raw editor so the DOC-08 span-only gate, backup,
    // conflict detection, and read-back verification all apply.
    crate::raw_editor::commit_with_snapshot(path, new_text.as_bytes(), Some(&snap))?;
    Ok(OperationOutcome {
        changed: true,
        redacted_summary: changed_summary(
            codec
                .span_body(&new_text, span_name)
                .ok()
                .flatten()
                .as_deref(),
        ),
        warnings: Vec::new(),
    })
}

/// Render a span body for summaries, redacting when policy or content
/// demands it. Span bodies can hold anything, so secret-shaping is checked
/// on the rendered form.
fn redact_span_body(op: &Operation, body: &str) -> String {
    let rendered = Value::String(body.to_owned());
    render_value(op, &rendered)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str, ext: &str) -> std::path::PathBuf {
        let dir = crate::test_util::temp_dir_unique("config-executor");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{name}{ext}"))
    }

    fn set_op(selector: &str, value: Value) -> Operation {
        Operation::new(EditOperation::Set {
            selector: Selector::parse(selector).unwrap(),
            value,
        })
    }

    // ---- owned_keys ----

    #[test]
    fn owned_keys_accept_declared_selector() {
        let mut doc = json!({"model": "opus"});
        let op = set_op("key:model", json!("sonnet")).with_owned_keys(vec!["key:model".into()]);
        let outcome = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert!(outcome.changed);
        assert_eq!(doc["model"], json!("sonnet"));
    }

    #[test]
    fn owned_keys_reject_outside_selector_without_write() {
        let mut doc = json!({"model": "opus"});
        let op = set_op("key:model", json!("sonnet")).with_owned_keys(vec!["key:other".into()]);
        let err = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap_err();
        match err {
            ConfigError::NotOwned { selector, .. } => assert_eq!(selector, "key:model"),
            other => panic!("expected NotOwned, got {other:?}"),
        }
        assert_eq!(doc["model"], json!("opus"), "no write on rejection");
    }

    #[test]
    fn owned_keys_prefix_paths_count() {
        let mut doc = json!({"outer": {"inner": 1}});
        let op = set_op("key:outer.inner", json!(2)).with_owned_keys(vec!["outer".into()]);
        apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert_eq!(doc["outer"]["inner"], json!(2));
    }

    #[test]
    fn empty_owned_keys_own_nothing() {
        let mut doc = json!({"model": "opus"});
        let err = apply_to_value(Path::new("(mem)"), &mut doc, &set_op("key:model", json!(1)))
            .unwrap_err();
        assert!(matches!(err, ConfigError::NotOwned { .. }));
    }

    // ---- expected_old ----

    #[test]
    fn expected_old_match_proceeds_and_mismatch_conflicts() {
        let path = Path::new("(mem)");
        let mut doc = json!({"model": "opus"});

        let ok = set_op("key:model", json!("sonnet"))
            .with_owned_keys(vec!["model".into()])
            .with_expected_old(Some(json!("opus")));
        apply_to_value(path, &mut doc, &ok).unwrap();
        assert_eq!(doc["model"], json!("sonnet"));

        let mut doc2 = json!({"model": "opus"});
        let conflict = set_op("key:model", json!("sonnet"))
            .with_owned_keys(vec!["model".into()])
            .with_expected_old(Some(json!("something-else")));
        let err = apply_to_value(path, &mut doc2, &conflict).unwrap_err();
        match err {
            ConfigError::OperationConflict {
                expected, actual, ..
            } => {
                assert!(expected.contains("something-else"));
                assert!(actual.contains("opus"));
            }
            other => panic!("expected OperationConflict, got {other:?}"),
        }
        assert_eq!(doc2["model"], json!("opus"), "conflict must not write");
    }

    #[test]
    fn expected_old_absence_expectation() {
        let path = Path::new("(mem)");
        // Absent as expected -> applies.
        let mut doc = json!({});
        let op = set_op("key:model", json!("m1"))
            .with_owned_keys(vec!["model".into()])
            .with_expected_old(None);
        apply_to_value(path, &mut doc, &op).unwrap();
        assert_eq!(doc["model"], json!("m1"));

        // Present but absence expected -> conflict.
        let mut doc2 = json!({"model": "x"});
        let err = apply_to_value(path, &mut doc2, &op).unwrap_err();
        assert!(matches!(err, ConfigError::OperationConflict { .. }));
    }

    #[test]
    fn expected_old_conflict_redacts_secret_values() {
        let mut doc = json!({"api_key": "sk-live-supersecret"});
        let op = set_op("key:api_key", json!("sk-new-value"))
            .with_owned_keys(vec!["api_key".into()])
            .with_expected_old(Some(json!("sk-wrong")));
        let err = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap_err();
        let rendered = err.to_string();
        assert!(!rendered.contains("sk-live-supersecret"), "{rendered}");
        assert!(!rendered.contains("sk-new-value"), "{rendered}");
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
    }

    // ---- duplicate_handling ----

    #[test]
    fn duplicate_error_rejects_existing_entry() {
        let mut doc = json!({"mcpServers": {"a": 1}});
        let op = Operation::new(EditOperation::InsertEntry {
            selector: Selector::Key("mcpServers".into()),
            key: "a".into(),
            value: json!(2),
        })
        .with_owned_keys(vec!["mcpServers".into()])
        .with_duplicate_handling(DuplicateHandling::Error);
        let err = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap_err();
        assert!(matches!(err, ConfigError::DuplicateRejected { .. }));
        assert_eq!(doc["mcpServers"]["a"], json!(1));
    }

    #[test]
    fn duplicate_keep_first_leaves_entry_unchanged() {
        let mut doc = json!({"mcpServers": {"a": 1}});
        let op = Operation::new(EditOperation::InsertEntry {
            selector: Selector::Key("mcpServers".into()),
            key: "a".into(),
            value: json!(2),
        })
        .with_owned_keys(vec!["mcpServers".into()])
        .with_duplicate_handling(DuplicateHandling::KeepFirst);
        let outcome = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert!(!outcome.changed);
        assert_eq!(doc["mcpServers"]["a"], json!(1));
    }

    #[test]
    fn duplicate_overwrite_replaces_entry() {
        let mut doc = json!({"mcpServers": {"a": 1}});
        let op = Operation::new(EditOperation::InsertEntry {
            selector: Selector::Key("mcpServers".into()),
            key: "a".into(),
            value: json!(2),
        })
        .with_owned_keys(vec!["mcpServers".into()])
        .with_duplicate_handling(DuplicateHandling::Overwrite);
        apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert_eq!(doc["mcpServers"]["a"], json!(2));
    }

    #[test]
    fn duplicate_append_pushes_second_identity_item() {
        let mut doc = json!({"servers": [{"name": "a"}]});
        let op = Operation::new(EditOperation::AppendIdentityItem {
            selector: Selector::Key("servers".into()),
            value: json!({"name": "a", "url": "u2"}),
            identity_key: "name".into(),
        })
        .with_owned_keys(vec!["servers".into()])
        .with_duplicate_handling(DuplicateHandling::Append);
        apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert_eq!(doc["servers"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn duplicate_error_rejects_existing_identity_item() {
        let mut doc = json!({"servers": [{"name": "a"}]});
        let op = Operation::new(EditOperation::AppendIdentityItem {
            selector: Selector::Key("servers".into()),
            value: json!({"name": "a", "url": "u2"}),
            identity_key: "name".into(),
        })
        .with_owned_keys(vec!["servers".into()])
        .with_duplicate_handling(DuplicateHandling::Error);
        let err = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap_err();
        assert!(matches!(err, ConfigError::DuplicateRejected { .. }));
        assert_eq!(doc["servers"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn duplicate_overwrite_replaces_identity_item_in_place() {
        let mut doc = json!({"servers": [{"name": "a", "url": "u1"}, {"name": "b"}]});
        let op = Operation::new(EditOperation::AppendIdentityItem {
            selector: Selector::Key("servers".into()),
            value: json!({"name": "a", "url": "u2"}),
            identity_key: "name".into(),
        })
        .with_owned_keys(vec!["servers".into()])
        .with_duplicate_handling(DuplicateHandling::Overwrite);
        apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        let servers = doc["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0], json!({"name": "a", "url": "u2"}));
        assert_eq!(servers[1], json!({"name": "b"}));
    }

    // ---- create_parent ----

    #[test]
    fn create_parent_true_creates_nested_objects() {
        let mut doc = json!({"keep": 1});
        let op = set_op("key:a.b.c", json!(true))
            .with_owned_keys(vec!["a".into()])
            .with_create_parent(true);
        apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert_eq!(doc["a"]["b"]["c"], json!(true));
        assert_eq!(doc["keep"], json!(1));
    }

    #[test]
    fn create_parent_false_rejects_missing_parent() {
        let mut doc = json!({"keep": 1});
        let op = set_op("key:a.b", json!(true)).with_owned_keys(vec!["a".into()]);
        let err = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap_err();
        match err {
            ConfigError::ParentMissing { .. } => {}
            other => panic!("expected ParentMissing, got {other:?}"),
        }
        assert_eq!(doc, json!({"keep": 1}), "no partial creation");
    }

    #[test]
    fn non_object_intermediate_is_typed_error_not_clobber() {
        let mut doc = json!({"a": 5});
        let op = set_op("key:a.b", json!(1))
            .with_owned_keys(vec!["a".into()])
            .with_create_parent(true);
        let err = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap_err();
        assert!(matches!(err, ConfigError::UnsupportedOperation { .. }));
        assert_eq!(doc["a"], json!(5), "scalar intermediate is not clobbered");
    }

    // ---- redaction policy ----

    #[test]
    fn redaction_policy_redacts_summary_values() {
        let mut doc = json!({"api_key": "old"});
        let op = set_op("key:api_key", json!("sk-brand-new"))
            .with_owned_keys(vec!["api_key".into()])
            .with_redaction_policy(RedactionPolicy::RedactValue);
        let outcome = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert!(outcome.changed);
        assert!(
            !outcome.redacted_summary.contains("sk-brand-new"),
            "{}",
            outcome.redacted_summary
        );
        assert!(outcome.redacted_summary.contains("[REDACTED]"));
        // The document itself still carries the real value.
        assert_eq!(doc["api_key"], json!("sk-brand-new"));
    }

    #[test]
    fn secret_shaped_values_redact_even_without_policy() {
        let mut doc = json!({"model": "x"});
        let op = set_op("key:model", json!("sk-secret-token-value"))
            .with_owned_keys(vec!["model".into()]);
        let outcome = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert!(!outcome.redacted_summary.contains("sk-secret-token-value"));
    }

    // ---- merge / ensure / enable-disable / remove ----

    #[test]
    fn merge_retains_foreign_keys_and_requires_owned_merge_keys() {
        let mut doc = json!({"env": {"owned": 1, "foreign": "keep"}});
        let op = Operation::new(EditOperation::Merge {
            selector: Selector::Key("env".into()),
            value: json!({"owned": 2, "newKey": true}),
        })
        .with_owned_keys(vec!["env.owned".into(), "env.newKey".into()]);
        apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert_eq!(doc["env"]["owned"], json!(2));
        assert_eq!(doc["env"]["newKey"], json!(true));
        assert_eq!(doc["env"]["foreign"], json!("keep"));

        let mut doc2 = json!({"env": {"foreign": "keep"}});
        let unowned = Operation::new(EditOperation::Merge {
            selector: Selector::Key("env".into()),
            value: json!({"sneaky": 1}),
        })
        .with_owned_keys(vec!["env.other".into()]);
        let err = apply_to_value(Path::new("(mem)"), &mut doc2, &unowned).unwrap_err();
        assert!(matches!(err, ConfigError::NotOwned { .. }));
    }

    #[test]
    fn ensure_dir_entry_is_idempotent() {
        let mut doc = json!({"skills": ["/a"]});
        let op = Operation::new(EditOperation::EnsureDirEntry {
            selector: Selector::Key("skills".into()),
            path: "/b".into(),
        })
        .with_owned_keys(vec!["skills".into()]);
        apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert_eq!(doc["skills"], json!(["/a", "/b"]));

        let again = Operation::new(EditOperation::EnsureDirEntry {
            selector: Selector::Key("skills".into()),
            path: "/b".into(),
        })
        .with_owned_keys(vec!["skills".into()]);
        let outcome = apply_to_value(Path::new("(mem)"), &mut doc, &again).unwrap();
        assert!(!outcome.changed);
    }

    #[test]
    fn ensure_dir_entry_creates_missing_array_only_with_create_parent() {
        let mut doc = json!({});
        let op = Operation::new(EditOperation::EnsureDirEntry {
            selector: Selector::Key("skills".into()),
            path: "/a".into(),
        })
        .with_owned_keys(vec!["skills".into()]);
        assert!(matches!(
            apply_to_value(Path::new("(mem)"), &mut doc, &op),
            Err(ConfigError::ParentMissing { .. })
        ));

        let create = op.with_create_parent(true);
        apply_to_value(Path::new("(mem)"), &mut doc, &create).unwrap();
        assert_eq!(doc["skills"], json!(["/a"]));
    }

    #[test]
    fn enable_disable_toggles_bool_and_rejects_non_bool() {
        let mut doc = json!({"feature": true});
        let off = Operation::new(EditOperation::EnableDisable {
            selector: Selector::Key("feature".into()),
            enabled: false,
        })
        .with_owned_keys(vec!["feature".into()]);
        apply_to_value(Path::new("(mem)"), &mut doc, &off).unwrap();
        assert_eq!(doc["feature"], json!(false));

        let mut bad = json!({"feature": "yes"});
        let err = apply_to_value(Path::new("(mem)"), &mut bad, &off).unwrap_err();
        assert!(matches!(err, ConfigError::OperationConflict { .. }));
    }

    #[test]
    fn remove_deletes_key_and_is_idempotent() {
        let mut doc = json!({"a": 1, "b": 2});
        let op = Operation::new(EditOperation::Remove {
            selector: Selector::Key("a".into()),
        })
        .with_owned_keys(vec!["a".into()]);
        apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert_eq!(doc, json!({"b": 2}));
        let outcome = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap();
        assert!(!outcome.changed);
    }

    // ---- Index / Identity root-array selectors ----

    #[test]
    fn index_and_identity_selectors_edit_root_arrays() {
        let mut doc = json!([{"name": "a"}, {"name": "b"}]);
        let by_identity = Operation::new(EditOperation::Set {
            selector: Selector::Identity {
                key: "name".into(),
                value: "b".into(),
            },
            value: json!({"name": "b", "extra": 1}),
        })
        .with_owned_keys(vec!["identity:name=b".into()]);
        apply_to_value(Path::new("(mem)"), &mut doc, &by_identity).unwrap();
        assert_eq!(doc[1], json!({"name": "b", "extra": 1}));

        let by_index = Operation::new(EditOperation::Remove {
            selector: Selector::Index(0),
        })
        .with_owned_keys(vec!["index:0".into()]);
        apply_to_value(Path::new("(mem)"), &mut doc, &by_index).unwrap();
        assert_eq!(doc, json!([{"name": "b", "extra": 1}]));
    }

    #[test]
    fn index_selector_on_non_array_root_is_typed_error() {
        let mut doc = json!({"a": 1});
        let op = Operation::new(EditOperation::Remove {
            selector: Selector::Index(0),
        })
        .with_owned_keys(vec!["index:0".into()]);
        let err = apply_to_value(Path::new("(mem)"), &mut doc, &op).unwrap_err();
        assert!(matches!(err, ConfigError::UnsupportedOperation { .. }));
    }

    // ---- Property: executor equals hand-applied for simple cases ----

    struct Prng(u64);
    impl Prng {
        fn next_u64(&mut self) -> u64 {
            let mut z = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            self.0 = z;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: usize) -> usize {
            if n == 0 {
                0
            } else {
                usize::try_from(self.next_u64() % n as u64).unwrap_or(0)
            }
        }
    }

    /// Random two-level document; every 7th iteration keeps `t1` scalar so
    /// property runs also exercise documents where parents cannot exist.
    fn random_doc(rng: &mut Prng, iteration: usize) -> Map<String, Value> {
        let mut doc = Map::new();
        for top_idx in 0..3usize {
            let top = format!("t{top_idx}");
            if iteration.is_multiple_of(7) && top_idx == 1 {
                let n = i64::try_from(rng.below(50)).unwrap_or(i64::MAX);
                doc.insert(top, Value::Number(n.into()));
                continue;
            }
            let mut nested = Map::new();
            for nested_idx in 0..3usize {
                nested.insert(
                    format!("n{nested_idx}"),
                    Value::Bool(rng.next_u64().is_multiple_of(2)),
                );
            }
            doc.insert(top, Value::Object(nested));
        }
        doc
    }

    fn hand_set(root: &mut Value, segments: &[String], value: Value) {
        let mut current = root;
        for segment in &segments[..segments.len().saturating_sub(1)] {
            let Value::Object(map) = current else {
                return;
            };
            current = map
                .entry(segment.clone())
                .or_insert_with(|| Value::Object(Map::new()));
        }
        if let Some(leaf) = segments.last()
            && let Value::Object(map) = current
        {
            map.insert(leaf.clone(), value);
        }
    }

    fn hand_remove(root: &mut Value, segments: &[String]) {
        let Some((leaf, parent)) = segments.split_last() else {
            return;
        };
        let mut current = root;
        for segment in parent {
            let Value::Object(map) = current else { return };
            match map.get_mut(segment) {
                Some(Value::Object(_)) => {
                    // reborrow for next iteration
                    current = map.get_mut(segment).unwrap();
                }
                _ => return,
            }
        }
        if let Value::Object(map) = current {
            map.remove(leaf);
        }
    }

    #[test]
    fn property_executor_set_equals_hand_applied() {
        let mut rng = Prng(0x5eed_1234);
        for iteration in 0..200 {
            let doc = random_doc(&mut rng, iteration);
            // Random path of one or two segments within t0/t2 roots.
            let top = if rng.next_u64().is_multiple_of(2) {
                "t0"
            } else {
                "t2"
            };
            let segments: Vec<String> = if rng.next_u64().is_multiple_of(2) {
                vec![top.to_owned()]
            } else {
                vec![top.to_owned(), format!("n{}", rng.below(3))]
            };
            let new_value =
                Value::Number(i64::try_from(rng.below(1000)).unwrap_or(i64::MAX).into());

            let mut via_executor = Value::Object(doc.clone());
            let mut by_hand = Value::Object(doc);

            let op = Operation::new(EditOperation::Set {
                selector: Selector::Key(segments.join(".")),
                value: new_value.clone(),
            })
            .with_owned_keys(vec![top.to_owned()])
            .with_create_parent(true);
            apply_to_value(Path::new("(prop)"), &mut via_executor, &op)
                .unwrap_or_else(|e| panic!("iteration {iteration} failed: {e}"));
            hand_set(&mut by_hand, &segments, new_value);

            assert_eq!(via_executor, by_hand, "divergence at iteration {iteration}");
        }
    }

    #[test]
    fn property_executor_remove_equals_hand_applied() {
        let mut rng = Prng(0xfeed_5678);
        for iteration in 0..200 {
            let mut doc = Map::new();
            let mut nested = Map::new();
            for nested_idx in 0..4usize {
                nested.insert(format!("n{nested_idx}"), Value::Bool(true));
            }
            doc.insert("t0".to_owned(), Value::Object(nested));
            doc.insert("t1".to_owned(), Value::Bool(false));

            let segments: Vec<String> = if rng.next_u64().is_multiple_of(2) {
                vec!["t1".to_owned()]
            } else {
                vec!["t0".to_owned(), format!("n{}", rng.below(4))]
            };

            let mut via_executor = Value::Object(doc.clone());
            let mut by_hand = Value::Object(doc);

            let op = Operation::new(EditOperation::Remove {
                selector: Selector::Key(segments.join(".")),
            })
            .with_owned_keys(vec!["t0".into(), "t1".into()]);
            apply_to_value(Path::new("(prop)"), &mut via_executor, &op).unwrap();
            hand_remove(&mut by_hand, &segments);

            assert_eq!(via_executor, by_hand, "divergence at iteration {iteration}");
        }
    }

    // ---- File-level application through the codecs ----

    #[test]
    fn file_apply_strict_json_set_and_noop_byte_identity() {
        let path = scratch("apply-json", ".json");
        std::fs::write(&path, b"{\"model\":\"opus\",\"foreign\":1}").unwrap();

        let op = set_op("key:model", json!("sonnet"))
            .with_owned_keys(vec!["model".into()])
            .with_expected_old(Some(json!("opus")));
        let outcome = apply(&path, DocumentKind::StrictJson, &op).unwrap();
        assert!(outcome.changed);
        let after = crate::json::load_value(&path).unwrap();
        assert_eq!(after["model"], json!("sonnet"));
        assert_eq!(after["foreign"], json!(1));
        // Minified source: the changing write had to normalize layout.
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains("surrounding formatting"));

        // No-op: byte identity preserved.
        let before = std::fs::read(&path).unwrap();
        let noop = set_op("key:model", json!("sonnet")).with_owned_keys(vec!["model".into()]);
        let outcome2 = apply(&path, DocumentKind::StrictJson, &noop).unwrap();
        assert!(!outcome2.changed);
        assert_eq!(std::fs::read(&path).unwrap(), before);

        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn file_apply_conflict_leaves_file_untouched() {
        let path = scratch("apply-conflict", ".json");
        std::fs::write(&path, b"{\"model\":\"opus\"}").unwrap();
        let before = std::fs::read(&path).unwrap();

        let op = set_op("key:model", json!("sonnet"))
            .with_owned_keys(vec!["model".into()])
            .with_expected_old(Some(json!("stale")));
        let err = apply(&path, DocumentKind::StrictJson, &op).unwrap_err();
        assert!(matches!(err, ConfigError::OperationConflict { .. }));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn file_apply_jsonc_lossy_write_gate_stays_intact() {
        let path = scratch("apply-jsonc", ".jsonc");
        let original = b"{\n  // comment\n  \"model\": \"opus\"\n}";
        std::fs::write(&path, original).unwrap();

        let op = set_op("key:model", json!("sonnet")).with_owned_keys(vec!["model".into()]);
        let err = apply(&path, DocumentKind::JsonC, &op).unwrap_err();
        match err {
            ConfigError::LossyWrite { format, .. } => assert_eq!(format, "jsonc"),
            other => panic!("expected LossyWrite, got {other:?}"),
        }
        assert_eq!(std::fs::read(&path).unwrap(), original);
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn file_apply_toml_preserves_comments() {
        let path = scratch("apply-toml", ".toml");
        std::fs::write(
            &path,
            "# keep me\nmodel = \"opus\"\n\n[other]\nkeep = true\n",
        )
        .unwrap();

        let op = set_op("key:model", json!("sonnet")).with_owned_keys(vec!["model".into()]);
        let outcome = apply(&path, DocumentKind::Toml, &op).unwrap();
        assert!(outcome.changed);

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# keep me"));
        assert!(after.contains("model = \"sonnet\""));
        assert!(after.contains("keep = true"));

        // Table selector under an existing table.
        let nested = set_op("table:other.keep", json!(false))
            .with_owned_keys(vec!["other".into()])
            .with_expected_old(Some(json!(true)));
        apply(&path, DocumentKind::Toml, &nested).unwrap();
        let after2 = std::fs::read_to_string(&path).unwrap();
        assert!(after2.contains("keep = false"));
        assert!(after2.contains("# keep me"));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn file_apply_env_preserves_comments_and_neighbours() {
        let path = scratch("apply-env", ".env");
        std::fs::write(&path, "# top comment\nMODEL=opus\nOTHER=1\n").unwrap();

        let op = set_op("key:MODEL", json!("sonnet"))
            .with_owned_keys(vec!["MODEL".into()])
            .with_expected_old(Some(json!("opus")));
        apply(&path, DocumentKind::Env, &op).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# top comment"), "{after}");
        assert!(after.contains("MODEL=sonnet"));
        assert!(after.contains("OTHER=1"));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn file_apply_env_rejects_nested_paths() {
        let path = scratch("apply-env-nested", ".env");
        std::fs::write(&path, "A=1\n").unwrap();
        let op = set_op("key:a.b", json!(1))
            .with_owned_keys(vec!["a".into()])
            .with_create_parent(true);
        let err = apply(&path, DocumentKind::Env, &op).unwrap_err();
        assert!(matches!(err, ConfigError::UnsupportedOperation { .. }));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn file_apply_opaque_document_is_refused() {
        let path = scratch("apply-bin", ".bin");
        std::fs::write(&path, b"raw").unwrap();
        let op = set_op("key:x", json!(1)).with_owned_keys(vec!["x".into()]);
        let err = apply(&path, DocumentKind::Opaque, &op).unwrap_err();
        assert!(matches!(err, ConfigError::UnsupportedOperation { .. }));
        drop(std::fs::remove_file(&path));
    }

    // ---- File-level text fragments (DOC-08 through the executor) ----

    #[test]
    fn file_apply_text_fragment_span_lifecycle() {
        let path = scratch("apply-frag", ".txt");
        drop(std::fs::remove_file(&path));

        let owned = vec!["span:managed".to_owned()];
        let insert = Operation::new(EditOperation::InsertEntry {
            selector: Selector::ManagedSpan("managed".into()),
            key: String::new(),
            value: Value::String("first body".into()),
        })
        .with_owned_keys(owned.clone());
        let outcome = apply(&path, DocumentKind::TextFragment, &insert).unwrap();
        assert!(outcome.changed);
        let disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            disk,
            "# superai:begin:managed\nfirst body\n# superai:end:managed\n"
        );

        // Replace through Set, with a foreign prelude added by the user.
        std::fs::write(&path, format!("user prelude\n{disk}")).unwrap();
        let replace = Operation::new(EditOperation::Set {
            selector: Selector::ManagedSpan("managed".into()),
            value: Value::String("second body".into()),
        })
        .with_owned_keys(owned.clone())
        .with_expected_old(Some(Value::String("first body".into())));
        apply(&path, DocumentKind::TextFragment, &replace).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            after,
            "user prelude\n# superai:begin:managed\nsecond body\n# superai:end:managed\n"
        );

        // expected_old mismatch on a span body is a typed conflict.
        let stale = Operation::new(EditOperation::Set {
            selector: Selector::ManagedSpan("managed".into()),
            value: Value::String("third".into()),
        })
        .with_owned_keys(owned.clone())
        .with_expected_old(Some(Value::String("stale".into())));
        let err = apply(&path, DocumentKind::TextFragment, &stale).unwrap_err();
        assert!(matches!(err, ConfigError::OperationConflict { .. }));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), after);

        // Remove restores the unmanaged bytes exactly.
        let remove = Operation::new(EditOperation::Remove {
            selector: Selector::ManagedSpan("managed".into()),
        })
        .with_owned_keys(owned);
        apply(&path, DocumentKind::TextFragment, &remove).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "user prelude\n");
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn file_apply_text_fragment_requires_owned_span() {
        let path = scratch("apply-frag-owned", ".txt");
        std::fs::write(&path, "# superai:begin:x\nb\n# superai:end:x\n").unwrap();
        let op = Operation::new(EditOperation::Set {
            selector: Selector::ManagedSpan("x".into()),
            value: Value::String("new".into()),
        })
        .with_owned_keys(vec!["span:other".into()]);
        let err = apply(&path, DocumentKind::TextFragment, &op).unwrap_err();
        assert!(matches!(err, ConfigError::NotOwned { .. }));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn file_apply_text_fragment_rejects_non_span_selectors() {
        let path = scratch("apply-frag-key", ".txt");
        std::fs::write(&path, "text\n").unwrap();
        let op = set_op("key:model", json!(1)).with_owned_keys(vec!["model".into()]);
        let err = apply(&path, DocumentKind::TextFragment, &op).unwrap_err();
        assert!(matches!(err, ConfigError::UnsupportedOperation { .. }));
        drop(std::fs::remove_file(&path));
    }
}
