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

/// Shared with `daemon.rs` for identity timestamps.
pub(crate) fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    unix_secs_to_rfc3339(secs)
}

// SSRF gate, shared by template_fetch, health, and skills fetch. One home so
// a new bypass spelling is fixed once, not per copy.

/// Host of an http(s) URL, lowercased. Strips userinfo (`user:pass@`) and
/// unwraps bracketed IPv6 literals (`[::1]:8443` -> `::1`), the forms that
/// otherwise hide the real host from the private-range check. The authority
/// ends at the first '/', '?', '#', or '\' (WHATWG special schemes treat '\'
/// like '/'); anything before that is host, not decoy.
pub(crate) fn extract_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
    let host_port = rest.get(0..end)?;
    let host_port = host_port.rsplit('@').next().unwrap_or_default();
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default()
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    Some(host.to_ascii_lowercase())
}

/// True for hosts a fetch must never reach: `localhost`, empty, loopback,
/// link-local, and RFC1918 space in every `inet_aton` spelling (dotted,
/// hex, octal, and bare-u32 forms), and IPv6 literals judged numerically
/// (unspecified, loopback, link-local, unique-local, and v4-mapped or
/// v4-compatible tails judged by the embedded v4 address).
pub(crate) fn is_private_host(host: &str) -> bool {
    // A trailing dot is the DNS root label: "localhost." is localhost.
    let h = host.to_ascii_lowercase();
    let h = h.trim_end_matches('.');
    if h == "localhost" || h.is_empty() {
        return true;
    }
    if h.contains(':') {
        // A colon host is an IPv6 literal; anything unparseable is refused
        // rather than guessed at (no real parser would dial it).
        return parse_ipv6(h).is_none_or(is_private_v6);
    }
    is_private_v4_literal(h)
}

/// Dotted-IPv4-shaped literal in private/loopback/link-local space. Parses
/// `inet_aton` forms numerically; unparseable dotted shapes fall back to
/// the textual prefixes so coverage never widens.
fn is_private_v4_literal(h: &str) -> bool {
    if let Some(v) = parse_inet_aton(h) {
        return is_private_v4_u32(v);
    }
    if h.starts_with("0.")
        || h.starts_with("10.")
        || h.starts_with("127.")
        || h.starts_with("192.168.")
        || h.starts_with("169.254.")
    {
        return true;
    }
    if h.starts_with("172.") {
        let second = h.split('.').nth(1).unwrap_or_default();
        return second.parse::<u8>().is_ok_and(|v| (16..=31).contains(&v));
    }
    false
}

fn is_private_v4_u32(v: u32) -> bool {
    let first = v >> 24;
    let second = (v >> 16) & 0xff;
    matches!(first, 0 | 10 | 127)
        || (first == 172 && (16..=31).contains(&second))
        || (first == 192 && second == 168)
        || (first == 169 && second == 254)
}

/// `inet_aton` parse: 1-4 dot-separated parts, each decimal, octal (leading
/// `0`), or hex (`0x`); the last part fills the remaining bytes. Digits-only
/// and `0x` parts mean "address", never domain, so `beef` stays a domain.
fn parse_inet_aton(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.is_empty()
        || parts.len() > 4
        || !parts.iter().all(|p| {
            if let Some(hex) = p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
                !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit())
            } else {
                !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())
            }
        })
    {
        return None;
    }
    let mut value: u32 = 0;
    let last = parts.len() - 1;
    for (i, part) in parts.iter().enumerate() {
        let num = if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            u32::from_str_radix(hex, 16).ok()?
        } else if let Some(octal) = part.strip_prefix('0') {
            if octal.is_empty() {
                0
            } else {
                u32::from_str_radix(octal, 8).ok()?
            }
        } else {
            part.parse::<u32>().ok()?
        };
        if i < last {
            if num > 0xff {
                return None;
            }
            value = (value << 8) | num;
        } else {
            // The final part spans every byte the earlier parts did not fill.
            let bits = 32 - 8 * last;
            if u64::from(num) >= 1u64 << bits {
                return None;
            }
            let combined = (u64::from(value) << bits) | u64::from(num);
            value = u32::try_from(combined).ok()?;
        }
    }
    Some(value)
}

fn is_private_v6(v: u128) -> bool {
    // fe80::/10 link-local, fc00::/7 unique-local.
    if v >> 118 == 0x3fa || v >> 121 == 0x7e {
        return true;
    }
    // v4-mapped (::ffff:0:0/96) and v4-compatible (::/96) tails are judged
    // by their embedded v4 address, compressed or full-form alike.
    if v >> 32 == 0xffff || v >> 32 == 0 {
        return is_private_v4_u32((v & 0xffff_ffff) as u32);
    }
    false
}

/// Numeric value of an IPv6 literal, accepting `::` compression (once) and
/// a dotted-quad tail. `None` when `h` is not a valid literal.
fn parse_ipv6(h: &str) -> Option<u128> {
    let (head, compressed_tail) = match h.split_once("::") {
        Some((a, b)) => (a, Some(b)),
        None => (h, None),
    };
    let mut head_words: Vec<u16> = Vec::new();
    let mut head_v4: Option<u32> = None;
    if !parse_v6_side(head, &mut head_words, &mut head_v4) {
        return None;
    }
    // A dotted tail is only valid as the literal's last bytes.
    if head_v4.is_some() && compressed_tail.is_some() {
        return None;
    }
    let mut tail_words: Vec<u16> = Vec::new();
    let mut tail_v4: Option<u32> = None;
    if let Some(tail) = compressed_tail
        && !parse_v6_side(tail, &mut tail_words, &mut tail_v4)
    {
        return None;
    }
    let head_len = head_words.len() + 2 * usize::from(head_v4.is_some());
    let tail_len = tail_words.len() + 2 * usize::from(tail_v4.is_some());
    let mut words: Vec<u16> = head_words;
    if let Some(v4) = head_v4 {
        words.push((v4 >> 16) as u16);
        words.push((v4 & 0xffff) as u16);
    }
    if compressed_tail.is_some() {
        if head_len + tail_len > 8 {
            return None;
        }
        words.resize(head_len + (8 - head_len - tail_len), 0);
        words.extend(tail_words);
        if let Some(v4) = tail_v4 {
            words.push((v4 >> 16) as u16);
            words.push((v4 & 0xffff) as u16);
        }
    } else if words.len() != 8 {
        return None;
    }
    Some(
        words
            .iter()
            .fold(0u128, |acc, w| (acc << 16) | u128::from(*w)),
    )
}

/// Parse one `::`-free side into 16-bit groups; a final dotted-quad segment
/// (the v4 tail) lands in `v4` instead. Empty sides are fine (`::` edges).
fn parse_v6_side(side: &str, groups: &mut Vec<u16>, v4: &mut Option<u32>) -> bool {
    if side.is_empty() {
        return true;
    }
    let segments: Vec<&str> = side.split(':').collect();
    let last = segments.len() - 1;
    for (i, seg) in segments.iter().enumerate() {
        if i == last && seg.contains('.') {
            match parse_inet_aton(seg) {
                Some(v) => *v4 = Some(v),
                None => return false,
            }
        } else if seg.is_empty() || seg.len() > 4 || !seg.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        } else if let Ok(g) = u16::from_str_radix(seg, 16) {
            groups.push(g);
        } else {
            return false;
        }
    }
    true
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
fn migrate_old_instance(old: OldInstance) -> Result<Instance> {
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
    let id = stable_id_for_legacy(name.as_str(), &config_root.to_string())?;
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
    inst.validate()?;
    Ok(inst)
}

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
                    // Files written without schema_version: try the v1 shape, fall back to legacy migration.
                    let try_new: std::result::Result<Vec<Instance>, _> =
                        serde_json::from_value(instances_raw.clone());
                    if let Ok(instances) = try_new {
                        let reg = Self {
                            schema_version: SCHEMA_VERSION,
                            instances,
                        };
                        reg.validate()?;
                        return Ok(reg);
                    }
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
                        instances.push(migrate_old_instance(old)?);
                    }
                    let reg = Self {
                        schema_version: SCHEMA_VERSION,
                        instances,
                    };
                    reg.validate()?;
                    Ok(reg)
                } else {
                    Ok(Self::default())
                }
            }
            Value::Array(arr) => {
                if arr.is_empty() {
                    return Ok(Self::default());
                }
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
                    instances.push(migrate_old_instance(old)?);
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

    /// Checks normalized-name, id, config-root, wrapper-path, and wrapper-command
    /// collisions, including wrapper commands colliding with instance names.
    #[expect(
        clippy::excessive_nesting,
        reason = "validation checks multiple collision kinds"
    )]
    fn validate(&self) -> Result<()> {
        let mut names: HashMap<String, &Instance> = HashMap::new();
        let mut ids: HashSet<String> = HashSet::new();
        let mut roots: HashSet<String> = HashSet::new();
        let mut wrapper_paths: HashSet<String> = HashSet::new();
        let mut wrapper_commands: HashMap<String, &Instance> = HashMap::new();

        for inst in &self.instances {
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

            let id_str = inst.id.as_str().to_owned();
            if !ids.insert(id_str.clone()) {
                return Err(CoreError::NameCollision {
                    kind: "InstanceId".to_owned(),
                    name: id_str,
                    reason: "duplicate id".to_owned(),
                });
            }

            let root_str = inst.config_root.to_string();
            if !roots.insert(root_str.clone()) {
                return Err(CoreError::Validation {
                    field: "config_root".to_owned(),
                    reason: format!(
                        "duplicate config_root `{root_str}` collides with another instance"
                    ),
                });
            }

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
        // Pre-check gives insert-specific error text; validate() after push is the safety net.
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
        let inst = &mut self.instances[idx];
        let old_name_owned = inst.name.to_string();
        let old_command = inst.wrapper.as_ref().map(|w| w.command_name.clone());
        inst.name = new_name.clone();
        if let Some(wrapper) = &mut inst.wrapper
            && wrapper.command_name.normalized() == old_name_owned.to_lowercase()
        {
            wrapper.command_name = new_name.clone();
        }

        if let Err(e) = self.validate() {
            // Roll back the name and any wrapper command the rename touched.
            let inst = &mut self.instances[idx];
            inst.name = InstanceName::new(&old_name_owned).unwrap_or(new_name);
            if let (Some(wrapper), Some(command)) = (&mut inst.wrapper, old_command) {
                wrapper.command_name = command;
            }
            return Err(e);
        }

        Ok(())
    }
}

/// Config dirs on disk that no record and no wrapper accounts for.
///
/// Adoption or removal is the user's call; superai only reports what it found.
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
            &crate::test_util::tmp_abs_str(".claude-work"),
            "id-1",
            None,
        ))
        .unwrap();
        // case-fold collision: "WORK" vs "work"
        let dup = sample_instance(
            "WORK",
            &crate::test_util::tmp_abs_str("u/.claude-work2"),
            "id-2",
            None,
        );
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
            &crate::test_util::tmp_abs_str(".claude-work"),
            "dup-id",
            None,
        ))
        .unwrap();
        let dup = sample_instance(
            "other",
            &crate::test_util::tmp_abs_str("u/.claude-other"),
            "dup-id",
            None,
        );
        let err = r.insert(dup).unwrap_err();
        match err {
            CoreError::NameCollision { kind, .. } => assert_eq!(kind, "InstanceId"),
            other => panic!("expected duplicate id, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_config_roots_are_rejected() {
        let tmp_root = crate::test_util::tmp_abs_str("u/.claude-work");
        let mut r = Registry::default();
        r.insert(sample_instance("work", tmp_root.as_str(), "id-1", None))
            .unwrap();
        // Same path normalized differently with extra slash
        let dup = sample_instance("other", tmp_root.as_str(), "id-2", None);
        let err = r.insert(dup).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "config_root"),
            other => panic!("expected Validation config_root, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_wrapper_paths_are_rejected() {
        let tmp_root = crate::test_util::tmp_abs_str("wrapper");
        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            &crate::test_util::tmp_abs_str(".claude-work"),
            "id-1",
            Some(tmp_root.as_str()),
        ))
        .unwrap();
        let dup = sample_instance(
            "other",
            &crate::test_util::tmp_abs_str(".claude-other"),
            "id-2",
            Some(tmp_root.as_str()),
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
            &crate::test_util::tmp_abs_str(".claude-work"),
            "id-1",
            Some(crate::test_util::tmp_abs_str("wrapper1").as_str()),
        ))
        .unwrap();
        // Different instance name but same wrapper command "work" (case-fold)
        let mut dup = sample_instance(
            "other",
            &crate::test_util::tmp_abs_str(".claude-other"),
            "id-2",
            Some(crate::test_util::tmp_abs_str("wrapper2").as_str()),
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
            &crate::test_util::tmp_abs_str(".claude-work"),
            "id-1",
            None,
        ))
        .unwrap();
        // New instance whose wrapper command collides with existing instance name "work"
        let mut with_wrapper = sample_instance(
            "other",
            &crate::test_util::tmp_abs_str(".claude-other"),
            "id-2",
            Some(crate::test_util::tmp_abs_str("wrapper-other").as_str()),
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
        let inst = sample_instance(
            "work",
            &crate::test_util::tmp_abs_str("u/.claude-work"),
            "stable-id-1",
            None,
        );
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
            &crate::test_util::tmp_abs_str(".claude-work"),
            "id-work",
            Some(crate::test_util::tmp_abs_str("wrapper-work").as_str()),
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
            &crate::test_util::tmp_abs_str(".claude-work"),
            "id-1",
            None,
        ))
        .unwrap();
        r.insert(sample_instance(
            "other",
            &crate::test_util::tmp_abs_str(".claude-other"),
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
        let path = crate::test_util::temp_dir_unique("registry").join("registry_v1_foreign.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Write a file with foreign keys and old-style instances key absent yet.
        std::fs::write(&path, r#"{"schema":7,"custom":"keep-me"}"#).unwrap();

        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            &crate::test_util::tmp_abs_str(".claude-work"),
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
        let tmp_root2 = crate::test_util::tmp_abs_str("u/.claude-aaa");
        let tmp_root = crate::test_util::tmp_abs_str("u/.claude-work");
        let mut r = Registry::default();
        r.insert(sample_instance(
            "work",
            tmp_root.as_str(),
            "id-unmanaged-1",
            None,
        ))
        .unwrap();

        let found = unmanaged_dirs(
            &r,
            &[PathBuf::from(tmp_root), PathBuf::from(tmp_root2.clone())],
        );
        assert_eq!(found, vec![PathBuf::from(tmp_root2)]);
    }

    #[test]
    fn migration_from_old_vector_and_instances_key() {
        let tmp_root = crate::test_util::tmp_abs_str("u/.claude-work");
        // Test bare array migration
        let path = crate::test_util::temp_dir_unique("registry").join("migration_bare.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = vec![instance_legacy("work", tmp_root.as_str())];
        std::fs::write(&path, serde_json::to_string(&old).unwrap()).unwrap();
        let reg = Registry::load(&path).unwrap();
        assert_eq!(reg.instances().len(), 1);
        let inst = &reg.instances()[0];
        assert_eq!(inst.name.as_str(), "work");
        assert_eq!(inst.harness.as_str(), "claude-code");
        assert_eq!(inst.config_root.to_string(), tmp_root.as_str());
        assert_eq!(inst.origin, InstanceOrigin::AdoptedLegacy);
        assert_eq!(inst.isolation, Isolation::Unknown);
        assert_eq!(inst.ownership, Ownership::ExplicitlyAdopted);
        // Stable id is deterministic: same name+config yields same id on reload.
        let reg2 = Registry::load(&path).unwrap();
        assert_eq!(reg.instances()[0].id, reg2.instances()[0].id);

        // Test object with instances key holding old shape
        let path2 = crate::test_util::temp_dir_unique("registry").join("migration_wrapped.json");
        let wrapped = serde_json::json!({
            "instances": [ {
                "name": "oldie",
                "harness": "claude-code",
                "config_dir": crate::test_util::tmp_abs_str("u/.claude-oldie"),
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
        let tmp_root = crate::test_util::tmp_abs_str("u/.claude-work");
        let bad_old = OldInstance {
            name: "CON".to_owned(), // reserved
            harness: "claude-code".to_owned(),
            config_dir: tmp_root.clone(),
            binary_path: None,
            template: None,
        };
        let err = migrate_old_instance(bad_old).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "name"),
            other => panic!("expected validation, got {other:?}"),
        }

        let bad_old2 = OldInstance {
            name: "work".to_owned(),
            harness: "bad/harness".to_owned(),
            config_dir: tmp_root,
            binary_path: None,
            template: None,
        };
        let err2 = migrate_old_instance(bad_old2).unwrap_err();
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
        let err3 = migrate_old_instance(bad_old3).unwrap_err();
        match err3 {
            CoreError::Validation { field, .. } => assert_eq!(field, "config_root"),
            other => panic!("expected validation, got {other:?}"),
        }
    }

    #[test]
    fn serialization_never_emits_forbidden_fields() {
        let inst = sample_instance(
            "work",
            &crate::test_util::tmp_abs_str("u/.claude-work"),
            "id-forbidden",
            None,
        );
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
        let path = crate::test_util::temp_dir_unique("registry").join("unknown_enum.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Unknown isolation variant
        let bad = serde_json::json!({
            "schema_version": 1,
            "instances": [{
                "id": "id-1",
                "name": "work",
                "harness": "claude-code",
                "config_root": crate::test_util::tmp_abs_str("u/.claude-work"),
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
        let tmp_root2 = crate::test_util::tmp_abs_str("user/.local/bin/work");
        let tmp_root = crate::test_util::tmp_abs_str("user/.claude-work");
        let tmp_bin = crate::test_util::tmp_abs_str("usr/local/bin/claude");
        // Old fixture: minimal old shape without schema_version
        let old_fixture = serde_json::json!({
            "instances": [
                {
                    "name": "work",
                    "harness": "claude-code",
                    "config_dir": tmp_root.as_str(),
                    "binary_path": tmp_bin.as_str(),
                    "template": {"name": "claude-glm", "version": "1.2.0"}
                },
                {
                    "name": "personal",
                    "harness": "codex-cli",
                    "config_dir": crate::test_util::tmp_abs_str("user/.codex-personal")
                }
            ],
            "foreign_key": "preserve-me"
        });
        let path = crate::test_util::temp_dir_unique("registry").join("golden_old.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&old_fixture).unwrap()).unwrap();
        let reg = Registry::load(&path).unwrap();
        assert_eq!(reg.instances().len(), 2);
        assert_eq!(reg.instances()[0].origin, InstanceOrigin::AdoptedLegacy);
        assert_eq!(reg.instances()[0].isolation, Isolation::Unknown);
        assert_eq!(
            reg.instances()[0].config_root.to_string(),
            tmp_root.as_str()
        );
        assert_eq!(
            reg.instances()[0].binary.as_ref().unwrap().to_string(),
            tmp_bin.as_str()
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
                    "config_root": tmp_root.as_str(),
                    "binary": "claude",
                    "wrapper": {
                        "path": tmp_root2.as_str(),
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
        let path2 = crate::test_util::temp_dir_unique("registry").join("golden_new.json");
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
            tmp_root2.as_str()
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

    #[test]
    fn private_host_detection() {
        assert!(is_private_host("localhost"));
        assert!(is_private_host("127.0.0.1"));
        assert!(is_private_host("10.0.0.1"));
        assert!(is_private_host("192.168.1.1"));
        assert!(is_private_host("172.16.5.4"));
        assert!(is_private_host("172.31.255.1"));
        assert!(!is_private_host("172.32.0.1"));
        assert!(!is_private_host("8.8.8.8"));
        assert!(!is_private_host("api.example.com"));
        // Domain-shaped words that parse as hex are still domains.
        assert!(!is_private_host("beef"));
        assert!(!is_private_host("deadbeef.example"));
    }

    /// SSRF shorthands that bypass prefix-only checks: `inet_aton` digit
    /// forms, trailing-dot root labels, cloud metadata space, and IPv6
    /// loopback/link-local/ULA/v4-mapped literals.
    #[test]
    fn private_host_detection_covers_ssrf_shorthands() {
        for host in [
            "127.1",
            "127.1.2.3",
            "2130706433",
            "localhost.",
            "LOCALHOST.",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
            "::",
            "fe80::1",
            "fd12:3456::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.5",
        ] {
            assert!(is_private_host(host), "{host} must count as private");
        }
        assert!(!is_private_host("8.8.4.4"));
        assert!(!is_private_host("2001:db8::1"));
    }

    /// The `inet_aton` hex/octal spellings and full-form IPv6 literals that
    /// the old textual checks missed.
    #[test]
    fn private_host_rejects_hex_octal_and_full_form_v6() {
        for host in [
            "0x7f.0.0.1",
            "0x7f000001",
            "0177.0.0.1",
            "017700000001",
            "0x7f.1",
            "0177.1",
            "0x7f.0.0.001",
            // v4-mapped in full form and v4-compatible tails, dotted or hex.
            "::ffff:a00:1",
            "0:0:0:0:0:ffff:a00:1",
            "::ffff:10.0.0.5",
            "::10.0.0.5",
            "::a00:1",
            "0:0:0:0:0:0:10.0.0.5",
            "::ffff:169.254.169.254",
        ] {
            assert!(is_private_host(host), "{host} must count as private");
        }
        // Public controls: hex that lands outside private ranges, public
        // v4-mapped and documentation-space literals, and full public v6.
        assert!(!is_private_host("0x08080808"));
        assert!(!is_private_host("::ffff:8.8.8.8"));
        assert!(!is_private_host("::8.8.8.8"));
        assert!(!is_private_host("1:2:3:4:5:6:7:8"));
        assert!(!is_private_host("2001:db8:0:0:0:0:0:1"));
    }

    /// `extract_host` defeats the decoy spellings: userinfo, brackets,
    /// delimiters that end the authority early, and extra scheme slashes.
    #[test]
    fn extract_host_cuts_the_real_authority() {
        assert_eq!(
            extract_host("https://example.com/a"),
            Some("example.com".to_owned())
        );
        assert_eq!(
            extract_host("http://example.com"),
            Some("example.com".to_owned())
        );
        assert_eq!(
            extract_host("https://user:pass@10.0.0.5/x"),
            Some("10.0.0.5".to_owned())
        );
        assert_eq!(extract_host("https://[::1]:8443/x"), Some("::1".to_owned()));
        assert_eq!(
            extract_host("https://127.0.0.1?@x.example.com/"),
            Some("127.0.0.1".to_owned())
        );
        assert_eq!(
            extract_host("https://169.254.169.254#@api.example.com/"),
            Some("169.254.169.254".to_owned())
        );
        assert_eq!(
            extract_host("https://127.0.0.1\\@x.example.com/"),
            Some("127.0.0.1".to_owned())
        );
        // The mirror spelling keeps its public host: the '@' is in the query.
        assert_eq!(
            extract_host("https://x.example.com?@127.0.0.1/"),
            Some("x.example.com".to_owned())
        );
        assert_eq!(extract_host("https:///127.0.0.1/"), Some(String::new()));
        assert_eq!(extract_host("ftp://example.com/"), None);
    }

    #[test]
    fn ipv6_parser_accepts_and_rejects_literals() {
        assert_eq!(parse_ipv6("::"), Some(0));
        assert_eq!(parse_ipv6("::1"), Some(1));
        assert_eq!(
            parse_ipv6("2001:db8::1"),
            Some(0x2001_0db8_0000_0000_0000_0000_0000_0001)
        );
        // Dotted tail without compression, and one compression too many.
        assert_eq!(parse_ipv6("::ffff:1.2.3.4"), parse_ipv6("::ffff:102:304"));
        assert_eq!(
            parse_ipv6("1:2:3:4:5:6:1.2.3.4"),
            Some(0x0001_0002_0003_0004_0005_0006_0102_0304)
        );
        for bad in [
            "1::2::3",
            ":::",
            "12345::",
            "1:2:3:4:5:6:7:8:9",
            "gg::1",
            "1.2.3.4::",
        ] {
            assert_eq!(parse_ipv6(bad), None, "{bad} must not parse");
        }
    }
}
