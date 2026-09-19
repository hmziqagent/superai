//! Symlink-swap profiles over fixed config paths (run-5 area A, decision 2).
//!
//! Some harnesses (claude-desktop is the declared [`Isolation::FixedPathSingle`]
//! case) read one fixed per-OS config path and honor NO relocation env var,
//! so an "instance" of them is modeled as a superai-managed PROFILE TREE
//! swapped into the fixed path by an atomic symlink flip:
//!
//! 1. `create_profile` makes a fresh managed root under the caller-chosen
//!    base (`<base>/<harness>/<name>/`) carrying the ownership marker
//!    `.superai-profile`. The caller seeds the harness config INTO that
//!    tree — e.g. the claude-desktop third-party inference keys through
//!    [`crate::adapters::claude_desktop::commit_third_party_inference`].
//! 2. `activate_profile` points the PARAMETERIZED fixed path at the managed
//!    root: any pre-existing real content at the path is first moved to a
//!    recoverable backup under the profile base (the quarantine machinery,
//!    never deleted), then a symlink is created under a temporary name and
//!    `rename(2)`d onto the fixed path — the swap itself is atomic. A fixed
//!    path that is already one of OUR symlinks is swapped the same way; a
//!    symlink pointing anywhere outside the profile base is foreign and
//!    refused.
//! 3. `deactivate_profile` removes the symlink and restores the backed-up
//!    content byte-identically (digest-verified against the digest recorded
//!    before the swap).
//! 4. `list_profiles` / `remove_profile` manage the on-disk manifest.
//!
//! THE FIXED PATH IS ALWAYS A PARAMETER. Nothing in this module resolves or
//! writes the real user home; tests operate on fake roots only.
//!
//! Layout under the caller-chosen base directory:
//!
//! ```text
//! <base>/profiles.json                                  profile manifest
//! <base>/<harness>/<profile-name>/                      managed profile tree
//! <base>/<harness>/<profile-name>/.superai-profile      ownership marker
//! <base>/.superai/profile-active/<harness>.json         active-swap state
//! <base>/.superai/profile-locks/<harness>/activation.lock
//! <base>/.superai/quarantine/<operation_id>/            pre-swap backups
//! ```
//!
//! Honest caveats (run-5 research A.1, evidence
//! `desktop-multi-instance-alternatives.md`):
//! - offline-only while the app runs: Electron `Singleton*` lock files live
//!   INSIDE the swapped tree, so deactivate/switch while the app is quit;
//! - Windows MSIX installs virtualize AppData, so a symlink at the visible
//!   path may not be what the packaged app reads (unreliable there);
//!   Linux/macOS are fine;
//! - concurrent activations of one harness serialize through the WRP-06
//!   activation lock (stale locks are pid-liveness recovered).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use superai_config::document::DocumentKind;
use superai_config::transaction::{FileAction, Transaction, commit_file};

use crate::activation::ActivationLock;
use crate::error::{CoreError, Result};
use crate::ids::{HarnessId, InstanceName};
use crate::paths::AbsolutePath;

/// Marker file inside every managed profile root proving superai ownership.
/// Line 1 is the harness id, line 2 the profile name (exact case).
pub const PROFILE_MARKER_FILE: &str = ".superai-profile";

/// Manifest file under the profile base directory listing every profile.
pub const PROFILE_MANIFEST_FILE: &str = "profiles.json";

/// Current profile manifest schema version.
pub const PROFILE_MANIFEST_SCHEMA_VERSION: u32 = 1;

const MANIFEST_PROFILES_KEY: &str = "profiles";
const MANIFEST_SCHEMA_KEY: &str = "schema_version";
/// Superai-owned state directory under the profile base.
const PROFILE_STATE_DIR: &str = ".superai";
/// Active-swap state directory (one JSON file per harness).
const ACTIVE_DIR_NAME: &str = "profile-active";
/// Activation lock directory (one lockfile per harness).
const LOCK_DIR_NAME: &str = "profile-locks";

fn compute_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

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

fn unique_operation_string(prefix: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hasher = DefaultHasher::new();
    millis.hash(&mut hasher);
    count.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    let suffix = hasher.finish() & 0xffff;
    format!("{prefix}-{millis:013}-{suffix:04x}-{count:04x}")
}

// ---------------------------------------------------------------------------
// Spec and record
// ---------------------------------------------------------------------------

/// Request to create a profile: a harness and a name. The managed tree the
/// fixed path will point at is created fresh; seeding harness config into it
/// is the caller's business (through the adapter-declared write paths).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSpec {
    /// Harness whose fixed config path the profile swaps.
    pub harness: HarnessId,
    /// User-chosen profile label (also the managed root's last segment).
    pub name: InstanceName,
}

impl ProfileSpec {
    /// Create a minimal spec.
    pub fn new(harness: HarnessId, name: InstanceName) -> Self {
        Self { harness, name }
    }

    /// The profile's managed root derived from `base_dir` (see [`profile_root`]).
    pub fn root(&self, base_dir: &Path) -> Result<AbsolutePath> {
        profile_root(base_dir, &self.harness, &self.name)
    }
}

/// A recorded profile in the on-disk manifest.
///
/// Forbidden fields (never serialized): model/provider data, api keys — a
/// profile is a config TREE, and its effective content lives in the harness's
/// own files inside `root`, read fresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRecord {
    /// Harness the profile swaps for.
    pub harness: HarnessId,
    /// Profile label.
    pub name: InstanceName,
    /// The managed profile tree the fixed path will point at.
    pub root: AbsolutePath,
    /// When the profile was created (ISO8601 UTC).
    pub created_at: String,
}

/// Result of a successful activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileActivation {
    /// The profile that became active.
    pub record: ProfileRecord,
    /// The fixed path that now points at the managed root.
    pub fixed_path: PathBuf,
    /// Where pre-existing real content was moved (recoverable), when any.
    pub backup_path: Option<PathBuf>,
    /// Digest (SHA-256 hex) of the pre-existing content, when any.
    pub preexisting_digest: Option<String>,
}

/// Result of a successful deactivation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDeactivation {
    /// The fixed path that no longer points at a managed root.
    pub fixed_path: PathBuf,
    /// The profile that was active.
    pub profile: String,
    /// Whether backed-up content was restored (no backup = the path did not
    /// exist before the swap and is simply gone again).
    pub restored: bool,
}

/// Recorded state of one active swap (superai-owned, under the profile base).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ActiveSwap {
    profile: String,
    fixed_path: String,
    backup_path: Option<String>,
    preexisting_digest: Option<String>,
    activated_at: String,
}

// ---------------------------------------------------------------------------
// Roots and manifest
// ---------------------------------------------------------------------------

/// Derive the managed profile root: `<base>/<harness>/<profile-name>`.
pub fn profile_root(
    base_dir: &Path,
    harness: &HarnessId,
    name: &InstanceName,
) -> Result<AbsolutePath> {
    AbsolutePath::from_path(base_dir)?
        .join(harness.as_str())?
        .join(name.as_str())
}

fn manifest_path(base_dir: &Path) -> PathBuf {
    base_dir.join(PROFILE_MANIFEST_FILE)
}

fn load_manifest(base_dir: &Path) -> Result<Vec<ProfileRecord>> {
    let path = manifest_path(base_dir);
    let map = superai_config::json::load(&path).map_err(CoreError::Config)?;
    if let Some(version) = map.get(MANIFEST_SCHEMA_KEY) {
        let parsed = version.as_u64().and_then(|v| u32::try_from(v).ok());
        if parsed != Some(PROFILE_MANIFEST_SCHEMA_VERSION) {
            return Err(CoreError::SchemaValidation {
                path,
                details: format!(
                    "unsupported {MANIFEST_SCHEMA_KEY} {version}: expected \
                     {PROFILE_MANIFEST_SCHEMA_VERSION}"
                ),
            });
        }
    }
    let profiles = map
        .get(MANIFEST_PROFILES_KEY)
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    let records: Vec<ProfileRecord> =
        serde_json::from_value(profiles).map_err(CoreError::Records)?;
    Ok(records)
}

fn manifest_entry_matches(entry: &serde_json::Value, harness: &HarnessId, name: &str) -> bool {
    let entry_harness = entry.get("harness").and_then(serde_json::Value::as_str);
    let entry_name = entry.get("name").and_then(serde_json::Value::as_str);
    entry_harness == Some(harness.as_str())
        && entry_name.is_some_and(|n| n.eq_ignore_ascii_case(name))
}

/// Insert a record into the manifest through the config mutation boundary
/// (foreign keys preserved; existing content backed up before replace).
fn insert_manifest_record(base_dir: &Path, record: &ProfileRecord) -> Result<()> {
    let path = manifest_path(base_dir);
    let encoded = serde_json::to_value(record).map_err(CoreError::Records)?;
    superai_config::json::edit(&path, |map| {
        if !map.contains_key(MANIFEST_SCHEMA_KEY) {
            map.insert(
                MANIFEST_SCHEMA_KEY.to_owned(),
                serde_json::Value::Number(serde_json::Number::from(
                    PROFILE_MANIFEST_SCHEMA_VERSION,
                )),
            );
        }
        let mut profiles: Vec<serde_json::Value> = map
            .get(MANIFEST_PROFILES_KEY)
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        profiles.push(encoded.clone());
        map.insert(
            MANIFEST_PROFILES_KEY.to_owned(),
            serde_json::Value::Array(profiles),
        );
    })
    .map_err(CoreError::Config)?;
    Ok(())
}

/// Remove a profile's manifest entry, leaving foreign keys untouched.
fn remove_manifest_record(base_dir: &Path, harness: &HarnessId, name: &str) -> Result<()> {
    let path = manifest_path(base_dir);
    superai_config::json::edit(&path, |map| {
        let kept: Vec<serde_json::Value> = map
            .get(MANIFEST_PROFILES_KEY)
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| !manifest_entry_matches(entry, harness, name))
            .collect();
        map.insert(
            MANIFEST_PROFILES_KEY.to_owned(),
            serde_json::Value::Array(kept),
        );
    })
    .map_err(CoreError::Config)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Active-swap state
// ---------------------------------------------------------------------------

fn state_dir(base_dir: &Path) -> PathBuf {
    base_dir.join(PROFILE_STATE_DIR).join(ACTIVE_DIR_NAME)
}

fn state_path(base_dir: &Path, harness: &HarnessId) -> PathBuf {
    state_dir(base_dir).join(format!("{}.json", harness.as_str()))
}

fn load_active_swap(base_dir: &Path, harness: &HarnessId) -> Result<Option<ActiveSwap>> {
    let path = state_path(base_dir, harness);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let swap: ActiveSwap =
                serde_json::from_slice(&bytes).map_err(|e| CoreError::SchemaValidation {
                    path,
                    details: format!("malformed active-swap state: {e}"),
                })?;
            Ok(Some(swap))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CoreError::Config(superai_config::ConfigError::Io {
            path,
            source: e,
        })),
    }
}

fn store_active_swap(base_dir: &Path, harness: &HarnessId, swap: &ActiveSwap) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(swap).map_err(|e| CoreError::Validation {
        field: "active_swap".to_owned(),
        reason: format!("cannot serialize active-swap state: {e}"),
    })?;
    commit_file(
        "profile-active-swap",
        &state_path(base_dir, harness),
        &bytes,
        DocumentKind::StrictJson,
    )
    .map_err(CoreError::Config)?;
    Ok(())
}

fn clear_active_swap(base_dir: &Path, harness: &HarnessId) -> Result<()> {
    let path = state_path(base_dir, harness);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CoreError::Config(superai_config::ConfigError::Io {
            path,
            source: e,
        })),
    }
}

/// The currently active profile name for `harness`, read fresh from the
/// superai-owned swap state (None when no swap is recorded).
pub fn active_profile(base_dir: &Path, harness: &HarnessId) -> Result<Option<String>> {
    Ok(load_active_swap(base_dir, harness)?.map(|swap| swap.profile))
}

// ---------------------------------------------------------------------------
// Symlink helpers
// ---------------------------------------------------------------------------

/// Create a symlink `link` pointing at `target`.
///
/// Windows note (research A.1): creating directory symlinks may require
/// privileges, and MSIX-packaged apps virtualize `AppData` anyway — the
/// symlink-swap alternative is a Linux/macOS mechanism; failures map to the
/// typed error rather than being papered over.
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_dir(target, link);
    result.map_err(|e| CoreError::InvalidPath {
        kind: "symlink".to_owned(),
        value: link.display().to_string(),
        reason: format!(
            "cannot symlink {} -> {}: {e}",
            link.display(),
            target.display()
        ),
    })
}

/// Resolve what `path` currently is: a symlink (payload = resolved target),
/// real content, or absent.
enum FixedPathState {
    /// Symlink; the caller decides managed vs foreign against the recorded
    /// profile roots.
    Symlink(PathBuf),
    /// Real file/directory content (not a symlink).
    RealContent,
    /// Nothing at the path.
    Absent,
}

fn classify_fixed_path(path: &Path) -> Result<FixedPathState> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(path).map_err(|e| CoreError::InvalidPath {
                    kind: "symlink".to_owned(),
                    value: path.display().to_string(),
                    reason: format!("cannot read symlink target: {e}"),
                })?;
                let resolved = if target.is_absolute() {
                    target
                } else {
                    // Relative targets resolve against the link's parent.
                    path.parent()
                        .map_or_else(|| target.clone(), |parent| parent.join(&target))
                };
                Ok(FixedPathState::Symlink(resolved))
            } else {
                Ok(FixedPathState::RealContent)
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FixedPathState::Absent),
        Err(e) => Err(CoreError::Config(superai_config::ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })),
    }
}

/// A symlink target is MANAGED only when it is a recorded profile root under
/// the profile base — anything else (including arbitrary paths under the
/// base) is foreign and must not be swapped over.
fn is_managed_target(target: &Path, base_dir: &Path, harness: &HarnessId) -> Result<bool> {
    if !target.starts_with(base_dir) {
        return Ok(false);
    }
    Ok(load_manifest(base_dir)?
        .iter()
        .any(|record| record.harness == *harness && record.root.as_path() == target))
}

// ---------------------------------------------------------------------------
// Creation, listing, lookup
// ---------------------------------------------------------------------------

/// Create a profile: a fresh managed tree under the base, marked with the
/// ownership marker. Refuses case-fold name collisions and pre-existing
/// roots before any write, exactly like the alias core.
pub fn create_profile(base_dir: &Path, spec: &ProfileSpec) -> Result<ProfileRecord> {
    let root = profile_root(base_dir, &spec.harness, &spec.name)?;
    for record in load_manifest(base_dir)? {
        if record.harness == spec.harness && record.name.eq_case_fold(&spec.name) {
            return Err(CoreError::NameCollision {
                kind: "ProfileName".to_owned(),
                name: spec.name.to_string(),
                reason: format!(
                    "case-fold collision with existing profile `{}`",
                    record.name
                ),
            });
        }
    }
    if root.as_path().exists() {
        return Err(CoreError::ForeignOwnership {
            path: root.as_path().to_path_buf(),
            owner: "profile root already exists on disk".to_owned(),
        });
    }
    let marker = format!("{}\n{}\n", spec.harness.as_str(), spec.name.as_str());
    let steps = vec![
        FileAction::CreateDir {
            path: root.as_path().to_path_buf(),
        },
        FileAction::Write {
            path: root.as_path().join(PROFILE_MARKER_FILE),
            content: marker.into_bytes(),
            kind: DocumentKind::TextFragment,
        },
    ];
    let operation_id =
        superai_config::transaction::OperationId::new(&unique_operation_string("profile-create"))
            .map_err(|e| CoreError::Validation {
            field: "operation_id".to_owned(),
            reason: format!("generated operation id invalid: {e}"),
        })?;
    let mut transaction = Transaction::new(operation_id, steps);
    let outcome = transaction.execute().map_err(CoreError::Config)?;
    if !outcome.success {
        return Err(CoreError::Commit {
            path: root.as_path().to_path_buf(),
            reason: outcome.diagnostics_redacted.join("; "),
        });
    }
    let record = ProfileRecord {
        harness: spec.harness.clone(),
        name: spec.name.clone(),
        root,
        created_at: now_iso8601(),
    };
    insert_manifest_record(base_dir, &record)?;
    Ok(record)
}

/// List every recorded profile, read fresh from the on-disk manifest.
pub fn list_profiles(base_dir: &Path) -> Result<Vec<ProfileRecord>> {
    load_manifest(base_dir)
}

/// Look up one profile by harness and name (case-folded), read fresh.
pub fn get_profile(base_dir: &Path, harness: &HarnessId, name: &str) -> Result<ProfileRecord> {
    load_manifest(base_dir)?
        .into_iter()
        .find(|record| record.harness == *harness && record.name.eq_case_fold_str(name))
        .ok_or_else(|| CoreError::Validation {
            field: "profile".to_owned(),
            reason: format!("profile `{harness}@{name}` not found"),
        })
}

/// Verify the profile marker names this harness and profile (case-folded on
/// the name). A missing or mismatched marker means the tree is not a managed
/// profile root and must not be touched.
fn verify_profile_marker(root: &Path, harness: &HarnessId, name: &str) -> Result<()> {
    let marker_path = root.join(PROFILE_MARKER_FILE);
    let text = std::fs::read_to_string(&marker_path).map_err(|e| CoreError::ForeignOwnership {
        path: marker_path.clone(),
        owner: format!("profile marker unreadable ({e}); not a managed profile root"),
    })?;
    let mut lines = text.lines();
    let marker_harness = lines.next();
    let marker_name = lines.next();
    match (marker_harness, marker_name) {
        (Some(h), Some(n)) if h == harness.as_str() && n.eq_ignore_ascii_case(name) => Ok(()),
        _ => Err(CoreError::ForeignOwnership {
            path: marker_path,
            owner: "profile marker does not name this profile".to_owned(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Activation / deactivation
// ---------------------------------------------------------------------------

/// Activate profile `name` at `fixed_path`: point the path at the managed
/// root via an atomic symlink swap.
///
/// Pre-existing REAL content at the path is moved to a recoverable backup
/// under the profile base first (digest recorded for the byte-identical
/// restore); a pre-existing FOREIGN symlink (target outside the base) is
/// refused; a pre-existing MANAGED symlink is swapped in place. Activation
/// while another fixed path still holds a profile of the same harness is
/// refused (deactivate first); switching profiles at the SAME path is the
/// Validate and prepare the fixed path for the swap: refuse a foreign
/// symlink, back pre-existing REAL content up (recoverable, under the base —
/// never a blind delete; files additionally record a digest so the
/// deactivate restore is provably byte-identical, directories are moved
/// untouched which is byte-identical by construction), and pass a managed
/// symlink through (the rename replaces it atomically).
fn prepare_fixed_path_for_swap(
    base: &Path,
    base_dir: &Path,
    harness: &HarnessId,
    fixed_path: &Path,
) -> Result<(Option<PathBuf>, Option<String>)> {
    match classify_fixed_path(fixed_path)? {
        FixedPathState::Symlink(target) => {
            if !is_managed_target(&target, base_dir, harness)? {
                return Err(CoreError::ForeignOwnership {
                    path: fixed_path.to_path_buf(),
                    owner: format!(
                        "fixed path is a symlink to {}, which superai does not manage",
                        target.display()
                    ),
                });
            }
            Ok((None, None))
        }
        FixedPathState::RealContent => {
            let mut digest = None;
            if fixed_path.is_file()
                && let Ok(bytes) = std::fs::read(fixed_path)
            {
                digest = Some(compute_digest(&bytes));
            }
            let op = unique_operation_string("profile-swap");
            let entry = superai_config::quarantine::move_to_quarantine_under(base, fixed_path, &op)
                .map_err(CoreError::Config)?;
            Ok((Some(entry.quarantine_path), digest))
        }
        FixedPathState::Absent => Ok((None, None)),
    }
}

/// supported flow. Concurrent activations serialize through the WRP-06
/// activation lock.
pub fn activate_profile(
    base_dir: &Path,
    harness: &HarnessId,
    name: &str,
    fixed_path: &Path,
) -> Result<ProfileActivation> {
    let base = AbsolutePath::from_path(base_dir)?;
    let record = get_profile(base_dir, harness, name)?;
    verify_profile_marker(record.root.as_path(), harness, name)?;
    if !fixed_path.is_absolute() {
        return Err(CoreError::Validation {
            field: "fixed_path".to_owned(),
            reason: format!(
                "fixed path {} must be absolute (parameterized, never the real home)",
                fixed_path.display()
            ),
        });
    }
    if fixed_path == base.as_path() || fixed_path.starts_with(base.as_path()) {
        return Err(CoreError::ForeignOwnership {
            path: fixed_path.to_path_buf(),
            owner: "fixed path must not live inside the superai profile base".to_owned(),
        });
    }
    let lock_dir = base
        .as_path()
        .join(PROFILE_STATE_DIR)
        .join(LOCK_DIR_NAME)
        .join(harness.as_str());
    let _lock = ActivationLock::acquire(&lock_dir, harness.as_str())?;

    if let Some(active) = load_active_swap(base_dir, harness)?
        && active.fixed_path != fixed_path.display().to_string()
    {
        return Err(CoreError::Validation {
            field: "fixed_path".to_owned(),
            reason: format!(
                "profile `{}` is already active at {}; deactivate it first",
                active.profile, active.fixed_path
            ),
        });
    }

    let (backup_path, preexisting_digest) =
        prepare_fixed_path_for_swap(base.as_path(), base_dir, harness, fixed_path)?;

    // Atomic swap: create the symlink under a temporary sibling name, then
    // rename(2) it onto the fixed path. rename replaces an existing symlink
    // atomically; real content was moved away above (rename cannot replace a
    // directory with a symlink).
    let parent = fixed_path.parent().ok_or_else(|| CoreError::InvalidPath {
        kind: "fixed_path".to_owned(),
        value: fixed_path.display().to_string(),
        reason: "fixed path has no parent directory".to_owned(),
    })?;
    std::fs::create_dir_all(parent).map_err(|e| CoreError::InvalidPath {
        kind: "fixed_path".to_owned(),
        value: fixed_path.display().to_string(),
        reason: format!("cannot create parent {}: {e}", parent.display()),
    })?;
    let file_name = fixed_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| CoreError::InvalidPath {
            kind: "fixed_path".to_owned(),
            value: fixed_path.display().to_string(),
            reason: "fixed path has no usable file name".to_owned(),
        })?;
    let temp_link = parent.join(format!(
        ".{file_name}.superai-swap-{}",
        unique_operation_string("tmp")
    ));
    create_symlink(record.root.as_path(), &temp_link)?;
    if let Err(e) = std::fs::rename(&temp_link, fixed_path) {
        // Best-effort cleanup of the temporary link; the swap did not happen.
        drop(std::fs::remove_file(&temp_link));
        return Err(CoreError::Commit {
            path: fixed_path.to_path_buf(),
            reason: format!(
                "atomic symlink swap failed ({} -> {}): {e}",
                temp_link.display(),
                fixed_path.display()
            ),
        });
    }

    let swap = ActiveSwap {
        profile: record.name.to_string(),
        fixed_path: fixed_path.display().to_string(),
        backup_path: backup_path.as_ref().map(|p| p.display().to_string()),
        preexisting_digest: preexisting_digest.clone(),
        activated_at: now_iso8601(),
    };
    store_active_swap(base_dir, harness, &swap)?;
    Ok(ProfileActivation {
        record,
        fixed_path: fixed_path.to_path_buf(),
        backup_path,
        preexisting_digest,
    })
}

/// Deactivate the active profile at `fixed_path`: remove the managed symlink
/// and restore the backed-up pre-existing content byte-identically.
///
/// Refuses when no swap is recorded for the harness, when the recorded path
/// differs, or when the path is no longer one of our symlinks (someone
/// replaced it — that content is foreign and must not be touched).
pub fn deactivate_profile(
    base_dir: &Path,
    harness: &HarnessId,
    fixed_path: &Path,
) -> Result<ProfileDeactivation> {
    let base = AbsolutePath::from_path(base_dir)?;
    let lock_dir = base
        .as_path()
        .join(PROFILE_STATE_DIR)
        .join(LOCK_DIR_NAME)
        .join(harness.as_str());
    let _lock = ActivationLock::acquire(&lock_dir, harness.as_str())?;
    let swap = load_active_swap(base_dir, harness)?.ok_or_else(|| CoreError::Validation {
        field: "active_swap".to_owned(),
        reason: format!("no active profile swap recorded for harness `{harness}`"),
    })?;
    if swap.fixed_path != fixed_path.display().to_string() {
        return Err(CoreError::Validation {
            field: "fixed_path".to_owned(),
            reason: format!(
                "recorded swap is at {}, not {}",
                swap.fixed_path,
                fixed_path.display()
            ),
        });
    }
    match classify_fixed_path(fixed_path)? {
        FixedPathState::Symlink(target) => {
            if !is_managed_target(&target, base_dir, harness)? {
                return Err(CoreError::ForeignOwnership {
                    path: fixed_path.to_path_buf(),
                    owner: format!(
                        "fixed path now points at {}, which superai does not manage",
                        target.display()
                    ),
                });
            }
        }
        FixedPathState::RealContent | FixedPathState::Absent => {
            return Err(CoreError::ForeignOwnership {
                path: fixed_path.to_path_buf(),
                owner: "fixed path is no longer a superai-managed symlink".to_owned(),
            });
        }
    }
    std::fs::remove_file(fixed_path).map_err(|e| CoreError::Commit {
        path: fixed_path.to_path_buf(),
        reason: format!("cannot remove managed symlink: {e}"),
    })?;
    let mut restored = false;
    if let Some(backup) = swap.backup_path.as_deref() {
        // The quarantine entry path IS the moved pre-existing content
        // (file or directory tree) — move it back.
        let entry = PathBuf::from(backup);
        if !entry.exists() {
            return Err(CoreError::Verification {
                path: entry,
                kind: "restore".to_owned(),
                reason: "recorded backup no longer exists under the profile base".to_owned(),
            });
        }
        std::fs::rename(&entry, fixed_path).map_err(|e| CoreError::Commit {
            path: fixed_path.to_path_buf(),
            reason: format!("cannot restore backup {}: {e}", entry.display()),
        })?;
        if let Some(expected) = swap.preexisting_digest.as_deref() {
            let restored_bytes = std::fs::read(fixed_path).map_err(|e| {
                CoreError::Config(superai_config::ConfigError::Io {
                    path: fixed_path.to_path_buf(),
                    source: e,
                })
            })?;
            let actual = compute_digest(&restored_bytes);
            if actual != expected {
                return Err(CoreError::Verification {
                    path: fixed_path.to_path_buf(),
                    kind: "digest".to_owned(),
                    reason: format!(
                        "restored content digest {actual} does not match recorded {expected}"
                    ),
                });
            }
        }
        restored = true;
    }
    clear_active_swap(base_dir, harness)?;
    Ok(ProfileDeactivation {
        fixed_path: fixed_path.to_path_buf(),
        profile: swap.profile,
        restored,
    })
}

/// Whether `path` is `base` itself or nested under it (component-wise).
fn path_is_under(path: &Path, base: &Path) -> bool {
    path.starts_with(base)
}

/// Remove a profile: its managed root is moved to quarantine (recoverable,
/// never a blind delete) and its manifest entry dropped.
///
/// Refuses the currently-active profile (deactivate first), roots outside
/// the profile base, and roots without a matching ownership marker.
pub fn remove_profile(base_dir: &Path, harness: &HarnessId, name: &str) -> Result<ProfileRecord> {
    let base = AbsolutePath::from_path(base_dir)?;
    let record = get_profile(base_dir, harness, name)?;
    if active_profile(base_dir, harness)?.as_deref() == Some(name) {
        return Err(CoreError::Validation {
            field: "profile".to_owned(),
            reason: format!("profile `{name}` is active; deactivate it first"),
        });
    }
    if !path_is_under(record.root.as_path(), base.as_path()) {
        return Err(CoreError::ForeignOwnership {
            path: record.root.as_path().to_path_buf(),
            owner: "profile root outside the superai profile base".to_owned(),
        });
    }
    verify_profile_marker(record.root.as_path(), harness, name)?;
    if record.root.as_path().exists() {
        let op = unique_operation_string("profile-remove");
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

    fn base(tag: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(&format!("profile-{tag}"))
    }

    fn harness(id: &str) -> HarnessId {
        HarnessId::new(id).unwrap()
    }

    fn name(n: &str) -> InstanceName {
        InstanceName::new(n).unwrap()
    }

    /// A fake fixed-path root OUTSIDE the profile base (tests never touch the
    /// real home; the operated path is always a parameter).
    fn fixed_root() -> PathBuf {
        crate::test_util::temp_dir_unique("profile-fake-home")
    }

    #[test]
    fn activate_swaps_preexisting_content_with_recoverable_backup() {
        let b = base("activate");
        let record =
            create_profile(&b, &ProfileSpec::new(harness("claude-desktop"), name("p1"))).unwrap();
        std::fs::write(
            record.root.join("claude_desktop_config.json").unwrap(),
            br#"{"mcpServers": {}}"#,
        )
        .unwrap();
        let fixed = fixed_root().join(".config").join("Claude");
        std::fs::create_dir_all(&fixed).unwrap();
        std::fs::write(fixed.join("claude_desktop_config.json"), b"real user bytes").unwrap();

        let activation = activate_profile(&b, &harness("claude-desktop"), "p1", &fixed).unwrap();
        assert!(
            activation.backup_path.is_some(),
            "real content must be backed up"
        );

        // Directory backups carry no digest (files do); the content itself
        // is the proof below.
        assert!(activation.preexisting_digest.is_none());
        // The fixed path is now a symlink at the managed root, and the
        // profile's content is visible THROUGH the fixed path.
        assert!(fixed.is_symlink(), "fixed path must be a symlink");
        assert_eq!(
            std::fs::read(fixed.join("claude_desktop_config.json")).unwrap(),
            br#"{"mcpServers": {}}"#.to_vec()
        );
        // The backup holds the pre-existing bytes, recoverable under the base.
        let backup = activation.backup_path.unwrap();
        assert!(backup.starts_with(b.join(".superai").join("quarantine")));
        // The quarantine entry path IS the moved tree (its last component
        // keeps the original name).
        assert_eq!(
            std::fs::read(backup.join("claude_desktop_config.json")).unwrap(),
            b"real user bytes".to_vec()
        );
        assert_eq!(
            active_profile(&b, &harness("claude-desktop"))
                .unwrap()
                .as_deref(),
            Some("p1")
        );
    }

    #[test]
    fn deactivate_removes_symlink_and_restores_byte_identical_content() {
        let b = base("deactivate");
        let record =
            create_profile(&b, &ProfileSpec::new(harness("claude-desktop"), name("p1"))).unwrap();
        let fixed = fixed_root().join(".config").join("Claude");
        std::fs::create_dir_all(fixed.parent().unwrap()).unwrap();
        let original = b"user config\nwith two lines\n";
        std::fs::write(&fixed, original).unwrap();
        // A FILE (not a dir) at the fixed path is backed up and swapped too.
        activate_profile(&b, &harness("claude-desktop"), "p1", &fixed).unwrap();

        let deactivation = deactivate_profile(&b, &harness("claude-desktop"), &fixed).unwrap();
        assert!(deactivation.restored);
        assert!(!fixed.is_symlink(), "symlink must be gone");
        assert_eq!(std::fs::read(&fixed).unwrap(), original.to_vec());
        assert_eq!(
            active_profile(&b, &harness("claude-desktop")).unwrap(),
            None,
            "swap state must be cleared"
        );
        // Deactivating again refuses: no swap recorded.
        drop(deactivate_profile(&b, &harness("claude-desktop"), &fixed).unwrap_err());
        drop(record);
    }

    #[test]
    fn switching_profiles_at_the_same_path_swaps_atomically() {
        let b = base("switch");
        let p1 = create_profile(
            &b,
            &ProfileSpec::new(harness("claude-desktop"), name("one")),
        )
        .unwrap();
        let p2 = create_profile(
            &b,
            &ProfileSpec::new(harness("claude-desktop"), name("two")),
        )
        .unwrap();
        std::fs::write(p1.root.join("who.txt").unwrap(), b"one").unwrap();
        std::fs::write(p2.root.join("who.txt").unwrap(), b"two").unwrap();
        let fixed = fixed_root().join(".config").join("Claude");

        activate_profile(&b, &harness("claude-desktop"), "one", &fixed).unwrap();
        assert_eq!(
            std::fs::read(fixed.join("who.txt")).unwrap(),
            b"one".to_vec()
        );
        // No deactivate needed when the path is already our managed symlink.
        activate_profile(&b, &harness("claude-desktop"), "two", &fixed).unwrap();
        assert_eq!(
            std::fs::read(fixed.join("who.txt")).unwrap(),
            b"two".to_vec()
        );
        assert_eq!(
            active_profile(&b, &harness("claude-desktop"))
                .unwrap()
                .as_deref(),
            Some("two")
        );
    }

    #[test]
    fn activation_at_a_second_path_refuses_until_deactivated() {
        let b = base("second-path");
        create_profile(&b, &ProfileSpec::new(harness("claude-desktop"), name("p1"))).unwrap();
        let fixed_a = fixed_root().join("a").join("Claude");
        let fixed_b = fixed_root().join("b").join("Claude");
        activate_profile(&b, &harness("claude-desktop"), "p1", &fixed_a).unwrap();
        match activate_profile(&b, &harness("claude-desktop"), "p1", &fixed_b).unwrap_err() {
            CoreError::Validation { field, reason } => {
                assert_eq!(field, "fixed_path");
                assert!(reason.contains("deactivate it first"), "{reason}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn foreign_symlink_target_is_refused_without_touching_it() {
        let b = base("foreign-link");
        create_profile(&b, &ProfileSpec::new(harness("claude-desktop"), name("p1"))).unwrap();
        // Foreign target: OUTSIDE the profile base and not a recorded root.
        let elsewhere = fixed_root().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let fixed = fixed_root().join(".config").join("Claude");
        std::fs::create_dir_all(fixed.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &fixed).unwrap();

        match activate_profile(&b, &harness("claude-desktop"), "p1", &fixed).unwrap_err() {
            CoreError::ForeignOwnership { owner, .. } => {
                assert!(owner.contains("does not manage"), "{owner}");
            }
            other => panic!("expected ForeignOwnership, got {other:?}"),
        }
        // The foreign symlink is untouched and still points where it pointed.
        assert_eq!(std::fs::read_link(&fixed).unwrap(), elsewhere);
        assert!(
            active_profile(&b, &harness("claude-desktop"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn fixed_path_inside_the_base_is_refused() {
        let b = base("self-ref");
        create_profile(&b, &ProfileSpec::new(harness("claude-desktop"), name("p1"))).unwrap();
        let inside = b.join("claude-desktop").join("inside-target");
        match activate_profile(&b, &harness("claude-desktop"), "p1", &inside).unwrap_err() {
            CoreError::ForeignOwnership { .. } => {}
            other => panic!("expected ForeignOwnership, got {other:?}"),
        }
    }

    #[test]
    fn concurrent_activation_is_blocked_by_the_lock() {
        let b = base("locked");
        create_profile(&b, &ProfileSpec::new(harness("claude-desktop"), name("p1"))).unwrap();
        let lock_dir = b
            .join(".superai")
            .join(LOCK_DIR_NAME)
            .join("claude-desktop");
        std::fs::create_dir_all(&lock_dir).unwrap();
        std::fs::write(
            lock_dir.join("activation.lock"),
            serde_json::to_vec(&serde_json::json!({
                "pid": std::process::id(),
                "harness": "claude-desktop",
                "acquired_at": "2026-09-19T00:00:00Z",
            }))
            .unwrap(),
        )
        .unwrap();
        let fixed = fixed_root().join("Claude");
        match activate_profile(&b, &harness("claude-desktop"), "p1", &fixed).unwrap_err() {
            CoreError::ActivationLockHeld { holder_pid, .. } => {
                assert_eq!(holder_pid, Some(std::process::id()));
            }
            other => panic!("expected ActivationLockHeld, got {other:?}"),
        }
    }

    #[test]
    fn removal_refuses_active_unmarked_and_outside_roots() {
        let b = base("remove-refuse");
        let record =
            create_profile(&b, &ProfileSpec::new(harness("claude-desktop"), name("p1"))).unwrap();
        let fixed = fixed_root().join("Claude");
        activate_profile(&b, &harness("claude-desktop"), "p1", &fixed).unwrap();
        match remove_profile(&b, &harness("claude-desktop"), "p1").unwrap_err() {
            CoreError::Validation { reason, .. } => {
                assert!(reason.contains("active"), "{reason}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
        deactivate_profile(&b, &harness("claude-desktop"), &fixed).unwrap();

        // Tampered marker: foreign, untouched.
        std::fs::write(
            record.root.join(PROFILE_MARKER_FILE).unwrap().as_path(),
            b"claude-desktop\nother\n",
        )
        .unwrap();
        match remove_profile(&b, &harness("claude-desktop"), "p1").unwrap_err() {
            CoreError::ForeignOwnership { .. } => {}
            other => panic!("expected ForeignOwnership, got {other:?}"),
        }
        assert!(record.root.as_path().exists(), "unmarked root untouched");

        // Outside-base root recorded in the manifest: refused.
        let outside = crate::test_util::temp_dir_unique("profile-outside");
        std::fs::create_dir_all(&outside).unwrap();
        let manifest = serde_json::json!({
            "schema_version": 1,
            "profiles": [
                {"harness": "claude-desktop", "name": "outside",
                 "root": outside.display().to_string(),
                 "created_at": "2026-09-19T00:00:00Z"}
            ]
        });
        std::fs::write(
            b.join(PROFILE_MANIFEST_FILE),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        match remove_profile(&b, &harness("claude-desktop"), "outside").unwrap_err() {
            CoreError::ForeignOwnership { .. } => {}
            other => panic!("expected ForeignOwnership, got {other:?}"),
        }
        assert!(outside.exists());
    }

    #[test]
    fn removal_quarantines_under_the_base_and_drops_the_entry() {
        let b = base("remove");
        let record = create_profile(
            &b,
            &ProfileSpec::new(harness("claude-desktop"), name("gone")),
        )
        .unwrap();
        std::fs::write(record.root.join("config.json").unwrap(), b"x").unwrap();
        let removed = remove_profile(&b, &harness("claude-desktop"), "gone").unwrap();
        assert_eq!(removed.name.as_str(), "gone");
        assert!(!record.root.as_path().exists(), "root quarantined away");
        let qbase = b.join(".superai").join("quarantine");
        let ops: Vec<std::ffi::OsString> = std::fs::read_dir(&qbase)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|e| e.file_name())
            .filter(|n| n.to_string_lossy().starts_with("profile-remove-"))
            .collect();
        assert_eq!(
            ops.len(),
            1,
            "one profile-remove quarantine op under the base"
        );
        assert!(
            qbase.join(ops.first().unwrap()).join("gone").is_dir(),
            "the moved root stays recoverable"
        );
        assert!(list_profiles(&b).unwrap().is_empty());
    }

    #[test]
    fn create_refuses_collisions_and_existing_roots() {
        let b = base("collide");
        let spec = ProfileSpec::new(harness("claude-desktop"), name("team"));
        create_profile(&b, &spec).unwrap();
        let folded = ProfileSpec::new(harness("claude-desktop"), name("TEAM"));
        match create_profile(&b, &folded).unwrap_err() {
            CoreError::NameCollision { kind, .. } => assert_eq!(kind, "ProfileName"),
            other => panic!("expected NameCollision, got {other:?}"),
        }
        std::fs::create_dir_all(b.join("claude-desktop").join("fresh")).unwrap();
        let occupied = ProfileSpec::new(harness("claude-desktop"), name("fresh"));
        match create_profile(&b, &occupied).unwrap_err() {
            CoreError::ForeignOwnership { .. } => {}
            other => panic!("expected ForeignOwnership, got {other:?}"),
        }
    }

    #[test]
    fn manifest_round_trips_and_preserves_foreign_keys() {
        let b = base("manifest");
        create_profile(
            &b,
            &ProfileSpec::new(harness("claude-desktop"), name("one")),
        )
        .unwrap();
        superai_config::json::edit(&b.join(PROFILE_MANIFEST_FILE), |map| {
            map.insert(
                "foreign_note".to_owned(),
                serde_json::Value::String("keep-me".to_owned()),
            );
        })
        .unwrap();
        create_profile(
            &b,
            &ProfileSpec::new(harness("claude-desktop"), name("two")),
        )
        .unwrap();
        let raw = superai_config::json::load(&b.join(PROFILE_MANIFEST_FILE)).unwrap();
        assert_eq!(
            raw.get("foreign_note"),
            Some(&serde_json::Value::String("keep-me".to_owned()))
        );
        let listed = list_profiles(&b).unwrap();
        assert_eq!(listed.len(), 2);
        remove_profile(&b, &harness("claude-desktop"), "one").unwrap();
        let raw_after = superai_config::json::load(&b.join(PROFILE_MANIFEST_FILE)).unwrap();
        assert_eq!(
            raw_after.get("foreign_note"),
            Some(&serde_json::Value::String("keep-me".to_owned()))
        );
        assert_eq!(list_profiles(&b).unwrap().len(), 1);
    }
}
