//! Fixed-path profile activation (INS-10 / WRP-06).
//!
//! Fixed-path harnesses (zcode is the declared `SingleInstance`
//! `FixedPathSingle` target, `~/.zcode/v2/config.json`) expose exactly one
//! active config path, so "instances" are modeled as superai-owned saved
//! profiles stored OUTSIDE the harness's config tree, swapped into the fixed
//! path through a locked, backed-up, verified transaction:
//!
//! 1. `save_active_profile` fresh-reads the current fixed-path config into a
//!    named profile in the store.
//! 2. `activate_profile` takes an exclusive lock in the store (create-new
//!    lockfile, stale locks detected by pid liveness), fresh-reads the current
//!    file, and — when it was edited externally since the last activation —
//!    reconciles per an explicit choice (capture as a new profile / discard /
//!    abort) instead of silently overwriting. It then applies the chosen
//!    profile through a `superai-config` `Transaction` (backup + atomic
//!    replace + §4.2 conflict recheck + verify, optionally crash-journaled)
//!    and records the active identity derived from the applied content
//!    digest — never an assumed registry flag.
//! 3. `list_profiles` / `remove_profile` manage the store.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use superai_config::atomic::atomic_write;
use superai_config::document::DocumentKind;
use superai_config::transaction::{FileAction, OperationId as TxOperationId, Transaction};

use crate::daemon::pid_is_alive;
use crate::error::{CoreError, Result};
use crate::ids::{HarnessId, InstanceName};

/// Default superai-owned store root (`<home>/.superai/fixed-path-profiles`) —
/// deliberately outside every harness config tree.
#[must_use]
pub fn default_store_root(home: &Path) -> PathBuf {
    home.join(".superai").join("fixed-path-profiles")
}

/// Lockfile name inside the per-harness store directory.
const LOCK_FILE_NAME: &str = "activation.lock";

/// Metadata file suffix for a stored profile.
const PROFILE_META_SUFFIX: &str = ".profile.json";

/// Raw content file suffix for a stored profile.
const PROFILE_CONTENT_SUFFIX: &str = ".profile.content";

/// Active-identity record name inside the per-harness store directory.
const ACTIVE_FILE_NAME: &str = "active.json";

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

/// Days since 1970-01-01 to y/m/d (civil-from-days algorithm).
fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 { y + 1 } else { y },
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

// ---------------------------------------------------------------------------
// Stored types
// ---------------------------------------------------------------------------

/// Metadata for one saved profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSummary {
    /// Profile name.
    pub name: String,
    /// Harness the profile belongs to.
    pub harness: String,
    /// SHA-256 of the stored content (hex).
    pub content_digest: String,
    /// Stored content size in bytes.
    pub size_bytes: u64,
    /// ISO-8601 save timestamp.
    pub saved_at: String,
    /// Fixed path the content was captured from.
    pub fixed_path: String,
}

/// Identity of the last activation, derived from applied content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveIdentity {
    /// Profile name that was applied.
    pub profile: String,
    /// SHA-256 of the content as applied (hex) — the snapshot external edits
    /// are reconciled against.
    pub applied_digest: String,
    /// ISO-8601 activation timestamp.
    pub activated_at: String,
    /// Fixed path the profile was applied to.
    pub fixed_path: String,
}

/// Result of removing a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedProfile {
    /// Name of the removed profile.
    pub name: String,
    /// Whether the removed profile was the recorded active one.
    pub was_active: bool,
}

/// How to reconcile an externally edited fixed-path config (WRP-06).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileChoice {
    /// Save the current on-disk content as a new profile, then apply.
    CaptureAsProfile(InstanceName),
    /// Discard the external edit and apply the chosen profile.
    Discard,
    /// Abort: surface the conflict without touching the file.
    Abort,
}

/// Options for [`FixedPathProfileStore::activate_profile`].
#[derive(Debug, Clone, Default)]
pub struct ActivationOptions {
    /// Superai-owned crash-journal root; when set, activation transactions
    /// journal there so startup recovery (`failure::recover_pending`) covers
    /// interrupted swaps (MUT-09 enablement). Use
    /// [`ActivationOptions::for_home`] for the default location.
    pub journal_root: Option<PathBuf>,
}

impl ActivationOptions {
    /// Options with the default superai journal root for `home`.
    #[must_use]
    pub fn for_home(home: &Path) -> Self {
        Self {
            journal_root: Some(superai_config::journal::journal_dir(home)),
        }
    }
}

/// Outcome of a successful activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationOutcome {
    /// The profile that was applied.
    pub profile: ProfileSummary,
    /// Backups taken of the fixed-path file before the swap.
    pub backup_paths: Vec<PathBuf>,
    /// Profile captured from an external edit, when the choice was
    /// [`ReconcileChoice::CaptureAsProfile`].
    pub captured: Option<ProfileSummary>,
    /// The recorded active identity.
    pub activated: ActiveIdentity,
}

// ---------------------------------------------------------------------------
// Activation lock
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LockFile {
    pid: u32,
    harness: String,
    acquired_at: String,
}

/// Exclusive activation lock held for the duration of one activation
/// (INS-10: "Concurrent activation is locked/conflict checked").
///
/// Create-new semantics: acquiring fails with the typed
/// [`CoreError::ActivationLockHeld`] when a live holder exists. A lockfile
/// whose recorded pid is provably dead (or whose content is unparsable —
/// it sits in the superai-owned store) is stale and gets recovered exactly
/// once; on platforms without a std-visible process table stale locks are
/// never guessed away.
#[derive(Debug)]
pub struct ActivationLock {
    path: PathBuf,
}

impl ActivationLock {
    /// Acquire the lock for the harness store directory `dir`.
    pub fn acquire(dir: &Path, harness: &str) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| CoreError::Validation {
            field: "activation_lock_dir".to_owned(),
            reason: format!("cannot create {}: {e}", dir.display()),
        })?;
        let path = dir.join(LOCK_FILE_NAME);
        // One stale-recovery retry, then a typed conflict.
        if Self::try_create(&path, harness)? {
            return Ok(Self { path });
        }
        if lock_is_stale(&path) {
            drop(std::fs::remove_file(&path));
            if Self::try_create(&path, harness)? {
                return Ok(Self { path });
            }
        }
        let holder_pid = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<LockFile>(&b).ok())
            .map(|l| l.pid);
        Err(CoreError::ActivationLockHeld { path, holder_pid })
    }

    /// Create the lockfile with create-new semantics.
    ///
    /// `Ok(true)` means the lockfile was created (lock acquired); `Ok(false)`
    /// means it already exists (a live or unrecoverable holder); `Err` is an
    /// I/O failure.
    fn try_create(path: &Path, harness: &str) -> Result<bool> {
        let mut file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
            Err(e) => {
                return Err(CoreError::Validation {
                    field: "activation_lock".to_owned(),
                    reason: format!("cannot open lockfile {}: {e}", path.display()),
                });
            }
        };
        let lock = LockFile {
            pid: std::process::id(),
            harness: harness.to_owned(),
            acquired_at: now_iso8601(),
        };
        let body = serde_json::to_vec(&lock).map_err(|e| CoreError::Validation {
            field: "activation_lock".to_owned(),
            reason: format!("cannot serialize lockfile: {e}"),
        })?;
        file.write_all(&body).map_err(|e| CoreError::Validation {
            field: "activation_lock".to_owned(),
            reason: format!("cannot write lockfile {}: {e}", path.display()),
        })?;
        Ok(true)
    }
}

impl Drop for ActivationLock {
    fn drop(&mut self) {
        drop(std::fs::remove_file(&self.path));
    }
}

/// Whether the lockfile at `path` is provably stale.
fn lock_is_stale(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return true;
    };
    match serde_json::from_slice::<LockFile>(&bytes) {
        Ok(lock) => lock.pid != std::process::id() && !pid_is_alive(lock.pid),
        Err(_) => true,
    }
}

// ---------------------------------------------------------------------------
// Profile store
// ---------------------------------------------------------------------------

/// Superai-owned profile store for one fixed-path harness.
#[derive(Debug, Clone)]
pub struct FixedPathProfileStore {
    root: PathBuf,
    harness: HarnessId,
    harness_root: PathBuf,
}

impl FixedPathProfileStore {
    /// Open the store for `harness`.
    ///
    /// `root` is the superai-owned store location; `harness_root` is the
    /// harness-owned config tree containing the fixed path (e.g. `~/.zcode`).
    /// The store refuses to live inside the harness tree — profiles must
    /// survive activation swaps of the very tree they are stored beside, and
    /// a store inside the active path would be its own activation victim.
    pub fn new(root: &Path, harness: HarnessId, harness_root: &Path) -> Result<Self> {
        if !root.is_absolute() || !harness_root.is_absolute() {
            return Err(CoreError::Validation {
                field: "profile_store".to_owned(),
                reason: "store root and harness root must be absolute paths".to_owned(),
            });
        }
        if root == harness_root || root.starts_with(harness_root) {
            return Err(CoreError::Validation {
                field: "profile_store".to_owned(),
                reason: format!(
                    "profile store {} must live outside the harness config tree {}",
                    root.display(),
                    harness_root.display()
                ),
            });
        }
        Ok(Self {
            root: root.to_path_buf(),
            harness,
            harness_root: harness_root.to_path_buf(),
        })
    }

    /// Per-harness directory inside the store.
    #[must_use]
    pub fn harness_dir(&self) -> PathBuf {
        self.root.join(self.harness.as_str())
    }

    fn meta_path(&self, name: &InstanceName) -> PathBuf {
        self.harness_dir()
            .join(format!("{}{PROFILE_META_SUFFIX}", name.as_str()))
    }

    fn content_path(&self, name: &InstanceName) -> PathBuf {
        self.harness_dir()
            .join(format!("{}{PROFILE_CONTENT_SUFFIX}", name.as_str()))
    }

    fn active_path(&self) -> PathBuf {
        self.harness_dir().join(ACTIVE_FILE_NAME)
    }

    /// Validate that `fixed_path` is a file path inside the harness tree.
    fn require_fixed_path(&self, fixed_path: &Path) -> Result<()> {
        if !fixed_path.is_absolute() || !fixed_path.starts_with(&self.harness_root) {
            return Err(CoreError::Validation {
                field: "fixed_path".to_owned(),
                reason: format!(
                    "fixed path {} must live inside the harness config tree {}",
                    fixed_path.display(),
                    self.harness_root.display()
                ),
            });
        }
        if fixed_path == self.harness_root {
            return Err(CoreError::Validation {
                field: "fixed_path".to_owned(),
                reason: "fixed path must be a file, not the harness root".to_owned(),
            });
        }
        Ok(())
    }

    /// Fresh-read the current fixed-path config into profile `name`.
    ///
    /// The capture is a fresh disk read; the stored content is verbatim bytes
    /// (never interpreted, never rewritten in the harness's format).
    pub fn save_active_profile(
        &self,
        name: &InstanceName,
        fixed_path: &Path,
    ) -> Result<ProfileSummary> {
        self.require_fixed_path(fixed_path)?;
        let content = std::fs::read(fixed_path).map_err(|e| CoreError::Validation {
            field: "fixed_path".to_owned(),
            reason: format!(
                "cannot capture {} as profile {}: {e}",
                fixed_path.display(),
                name
            ),
        })?;
        self.write_profile(name, fixed_path, &content)
    }

    /// Write profile content + metadata into the superai-owned store.
    fn write_profile(
        &self,
        name: &InstanceName,
        fixed_path: &Path,
        content: &[u8],
    ) -> Result<ProfileSummary> {
        std::fs::create_dir_all(self.harness_dir()).map_err(|e| CoreError::Validation {
            field: "profile_store".to_owned(),
            reason: format!("cannot create {}: {e}", self.harness_dir().display()),
        })?;
        let summary = ProfileSummary {
            name: name.to_string(),
            harness: self.harness.to_string(),
            content_digest: compute_digest(content),
            size_bytes: u64::try_from(content.len()).unwrap_or(0),
            saved_at: now_iso8601(),
            fixed_path: fixed_path.display().to_string(),
        };
        atomic_write(&self.content_path(name), content).map_err(CoreError::Config)?;
        let meta = serde_json::to_vec_pretty(&summary).map_err(|e| CoreError::Validation {
            field: "profile_metadata".to_owned(),
            reason: format!("cannot serialize profile metadata: {e}"),
        })?;
        atomic_write(&self.meta_path(name), &meta).map_err(CoreError::Config)?;
        Ok(summary)
    }

    /// Load a stored profile (metadata + content), digest-verified.
    fn load_profile(&self, name: &InstanceName) -> Result<(ProfileSummary, Vec<u8>)> {
        let meta_bytes =
            std::fs::read(self.meta_path(name)).map_err(|e| CoreError::Validation {
                field: "profile".to_owned(),
                reason: format!("profile {name} not found in store: {e}"),
            })?;
        let summary: ProfileSummary =
            serde_json::from_slice(&meta_bytes).map_err(|e| CoreError::Validation {
                field: "profile_metadata".to_owned(),
                reason: format!("profile {name} metadata is malformed: {e}"),
            })?;
        let content = std::fs::read(self.content_path(name)).map_err(|e| {
            CoreError::Config(superai_config::ConfigError::Io {
                path: self.content_path(name),
                source: e,
            })
        })?;
        let actual = compute_digest(&content);
        if actual != summary.content_digest {
            return Err(CoreError::Verification {
                path: self.content_path(name),
                kind: "digest".to_owned(),
                reason: format!(
                    "profile {name} content digest {actual} does not match recorded {}",
                    summary.content_digest
                ),
            });
        }
        Ok((summary, content))
    }

    /// List stored profiles (read-only, fresh scan, sorted by name).
    pub fn list_profiles(&self) -> Result<Vec<ProfileSummary>> {
        let mut out = Vec::new();
        let dir = self.harness_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(out);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !file_name.ends_with(PROFILE_META_SUFFIX) {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&path)
                && let Ok(summary) = serde_json::from_slice::<ProfileSummary>(&bytes)
            {
                out.push(summary);
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Fresh-read the recorded active identity, if any.
    #[must_use]
    pub fn active_identity(&self) -> Option<ActiveIdentity> {
        let bytes = std::fs::read(self.active_path()).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Remove a stored profile.
    ///
    /// Removing the currently active profile is allowed — the active file on
    /// disk still carries the applied content and the recorded identity
    /// derives from that content — but the outcome reports `was_active` so
    /// callers can surface it.
    pub fn remove_profile(&self, name: &InstanceName) -> Result<RemovedProfile> {
        let meta = self.meta_path(name);
        let content = self.content_path(name);
        if !meta.exists() {
            return Err(CoreError::Validation {
                field: "profile".to_owned(),
                reason: format!("profile {name} not found in store"),
            });
        }
        let was_active = self
            .active_identity()
            .is_some_and(|a| a.profile == name.as_str());
        drop(std::fs::remove_file(&meta));
        drop(std::fs::remove_file(content));
        Ok(RemovedProfile {
            name: name.to_string(),
            was_active,
        })
    }

    /// Activate profile `name` onto the fixed path (INS-10/WRP-06).
    ///
    /// Locked end-to-end: lock → fresh-read current state → reconcile an
    /// external edit per `choice` → backup + transactional apply (atomic
    /// replace, §4.2 conflict recheck, read-back verify, optional crash
    /// journal) → verify digest → record the active identity.
    pub fn activate_profile(
        &self,
        name: &InstanceName,
        fixed_path: &Path,
        choice: &ReconcileChoice,
        opts: &ActivationOptions,
    ) -> Result<ActivationOutcome> {
        self.require_fixed_path(fixed_path)?;
        let _lock = ActivationLock::acquire(&self.harness_dir(), self.harness.as_str())?;

        let (summary, content) = self.load_profile(name)?;

        // Fresh-read the current fixed-path state (disk is truth), then
        // reconcile an external edit vs the last-activated snapshot.
        let current = std::fs::read(fixed_path).ok();
        let captured =
            self.reconcile_external_edit(name, fixed_path, choice, current.as_deref())?;

        // Transactional apply: backup of the existing foreign file, atomic
        // replace, fresh-snapshot conflict recheck, read-back verify.
        let op_id =
            TxOperationId::new(&format!("activate-{}-{}", self.harness, name)).map_err(|e| {
                CoreError::Validation {
                    field: "operation_id".to_owned(),
                    reason: format!("activation op id invalid: {e}"),
                }
            })?;
        let mut steps = Vec::new();
        if let Some(parent) = fixed_path.parent()
            && !parent.exists()
        {
            steps.push(FileAction::CreateDir {
                path: parent.to_path_buf(),
            });
        }
        steps.push(FileAction::Write {
            path: fixed_path.to_path_buf(),
            content,
            kind: DocumentKind::from_path(fixed_path),
        });
        let mut transaction = Transaction::new(op_id, steps);
        if let Some(journal_root) = &opts.journal_root {
            transaction = transaction.with_journal(journal_root.clone());
        }
        let outcome = transaction.execute().map_err(CoreError::Config)?;
        if !outcome.success {
            return Err(CoreError::Commit {
                path: fixed_path.to_path_buf(),
                reason: format!(
                    "activation transaction failed: {:?}",
                    outcome.diagnostics_redacted
                ),
            });
        }
        let backup_paths = outcome
            .commit
            .map(|c| c.backups.into_iter().map(|b| b.backup_path).collect())
            .unwrap_or_default();

        // Verify the applied content by digest (content/provenance-derived
        // identity, not an assumed flag).
        let applied = std::fs::read(fixed_path).map_err(|e| CoreError::Verification {
            path: fixed_path.to_path_buf(),
            kind: "digest".to_owned(),
            reason: format!("cannot read back {}: {e}", fixed_path.display()),
        })?;
        let applied_digest = compute_digest(&applied);
        if applied_digest != summary.content_digest {
            return Err(CoreError::Verification {
                path: fixed_path.to_path_buf(),
                kind: "digest".to_owned(),
                reason: format!(
                    "applied digest {applied_digest} does not match profile {} digest {}",
                    name, summary.content_digest
                ),
            });
        }

        let activated = ActiveIdentity {
            profile: name.to_string(),
            applied_digest,
            activated_at: now_iso8601(),
            fixed_path: fixed_path.display().to_string(),
        };
        let active_bytes =
            serde_json::to_vec_pretty(&activated).map_err(|e| CoreError::Validation {
                field: "active_identity".to_owned(),
                reason: format!("cannot serialize active identity: {e}"),
            })?;
        atomic_write(&self.active_path(), &active_bytes).map_err(CoreError::Config)?;

        Ok(ActivationOutcome {
            profile: summary,
            backup_paths,
            captured,
            activated,
        })
    }

    /// Reconcile a fixed-path file that changed since the last activation
    /// (WRP-06): abort with the digest evidence, capture the edit as a new
    /// profile, or discard it — never a silent overwrite. Unchanged (or
    /// never-activated) files need no reconciliation.
    fn reconcile_external_edit(
        &self,
        name: &InstanceName,
        fixed_path: &Path,
        choice: &ReconcileChoice,
        current: Option<&[u8]>,
    ) -> Result<Option<ProfileSummary>> {
        let Some(current) = current else {
            return Ok(None);
        };
        let Some(active) = self.active_identity() else {
            return Ok(None);
        };
        let digest = compute_digest(current);
        if active.applied_digest == digest {
            return Ok(None);
        }
        match choice {
            ReconcileChoice::Abort => Err(CoreError::ConcurrentModification {
                path: fixed_path.to_path_buf(),
                expected: active.applied_digest,
                actual: digest,
            }),
            ReconcileChoice::CaptureAsProfile(capture_name) => {
                if capture_name == name {
                    return Err(CoreError::Validation {
                        field: "reconcile_choice".to_owned(),
                        reason: format!(
                            "capture profile {capture_name} would overwrite the profile being activated"
                        ),
                    });
                }
                Ok(Some(self.write_profile(
                    capture_name,
                    fixed_path,
                    current,
                )?))
            }
            ReconcileChoice::Discard => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_dir(prefix: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(prefix)
    }

    struct Fixture {
        home: PathBuf,
        fixed_path: PathBuf,
        store: FixedPathProfileStore,
    }

    impl Fixture {
        fn new(prefix: &str) -> Self {
            let home = tmp_dir(prefix);
            let harness_root = home.join(".zcode");
            let fixed_path = harness_root.join("v2").join("config.json");
            fs::create_dir_all(fixed_path.parent().unwrap()).unwrap();
            fs::write(&fixed_path, br#"{"provider": "initial"}"#).unwrap();
            let store = FixedPathProfileStore::new(
                &default_store_root(&home),
                HarnessId::new("zcode").unwrap(),
                &harness_root,
            )
            .unwrap();
            Self {
                home,
                fixed_path,
                store,
            }
        }

        fn name(n: &str) -> InstanceName {
            InstanceName::new(n).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.home));
        }
    }

    #[test]
    fn activation_swaps_content_with_backup_and_restores_prior_profile() {
        let fx = Fixture::new("act-swap");
        let opts = ActivationOptions::default();

        // Capture the initial content as profile p1.
        let p1 = fx
            .store
            .save_active_profile(&Fixture::name("p1"), &fx.fixed_path)
            .unwrap();
        // User moves on to different content; capture it as p2.
        fs::write(&fx.fixed_path, br#"{"provider": "second"}"#).unwrap();
        fx.store
            .save_active_profile(&Fixture::name("p2"), &fx.fixed_path)
            .unwrap();

        // Activate p1: current content differs from the (empty) active
        // record — first activation, no snapshot to reconcile against, but
        // the foreign file is still backed up before the swap.
        let out = fx
            .store
            .activate_profile(
                &Fixture::name("p1"),
                &fx.fixed_path,
                &ReconcileChoice::Abort,
                &opts,
            )
            .unwrap();
        assert_eq!(
            fs::read(&fx.fixed_path).unwrap(),
            br#"{"provider": "initial"}"#
        );
        assert!(
            !out.backup_paths.is_empty(),
            "foreign fixed-path file must be backed up before the swap"
        );
        for backup in &out.backup_paths {
            assert!(backup.exists(), "backup {backup:?} must exist on disk");
            assert!(
                backup
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("config.json.bak."))
            );
        }
        assert_eq!(fx.store.active_identity().unwrap().profile, "p1");

        // Swap to p2 and back to p1: each swap restores the prior profile's
        // content exactly.
        fx.store
            .activate_profile(
                &Fixture::name("p2"),
                &fx.fixed_path,
                &ReconcileChoice::Abort,
                &opts,
            )
            .unwrap();
        assert_eq!(
            fs::read(&fx.fixed_path).unwrap(),
            br#"{"provider": "second"}"#
        );
        fx.store
            .activate_profile(
                &Fixture::name("p1"),
                &fx.fixed_path,
                &ReconcileChoice::Abort,
                &opts,
            )
            .unwrap();
        assert_eq!(
            fs::read(&fx.fixed_path).unwrap(),
            br#"{"provider": "initial"}"#
        );
        assert_eq!(
            p1.content_digest,
            compute_digest(br#"{"provider": "initial"}"#)
        );
    }

    #[test]
    fn external_edit_triggers_reconcile_choice_not_silent_overwrite() {
        let fx = Fixture::new("act-reconcile");
        let opts = ActivationOptions::default();
        fx.store
            .save_active_profile(&Fixture::name("p1"), &fx.fixed_path)
            .unwrap();
        fx.store
            .activate_profile(
                &Fixture::name("p1"),
                &fx.fixed_path,
                &ReconcileChoice::Abort,
                &opts,
            )
            .unwrap();

        // External edit after the last activation.
        fs::write(&fx.fixed_path, br#"{"provider": "user-typed"}"#).unwrap();

        // Abort: typed conflict, file untouched.
        let err = fx
            .store
            .activate_profile(
                &Fixture::name("p1"),
                &fx.fixed_path,
                &ReconcileChoice::Abort,
                &opts,
            )
            .unwrap_err();
        match err {
            CoreError::ConcurrentModification {
                path,
                expected,
                actual,
            } => {
                assert_eq!(path, fx.fixed_path);
                assert_eq!(expected, compute_digest(br#"{"provider": "initial"}"#));
                assert_eq!(actual, compute_digest(br#"{"provider": "user-typed"}"#));
            }
            other => panic!("expected ConcurrentModification, got {other:?}"),
        }
        assert_eq!(
            fs::read(&fx.fixed_path).unwrap(),
            br#"{"provider": "user-typed"}"#,
            "abort must not touch the file"
        );

        // Capture: the external edit becomes a profile, then the chosen
        // profile is applied.
        let out = fx
            .store
            .activate_profile(
                &Fixture::name("p1"),
                &fx.fixed_path,
                &ReconcileChoice::CaptureAsProfile(Fixture::name("user-edit")),
                &opts,
            )
            .unwrap();
        let captured = out.captured.expect("captured profile reported");
        assert_eq!(captured.name, "user-edit");
        assert_eq!(
            fs::read(
                fx.store
                    .harness_dir()
                    .join(format!("user-edit{PROFILE_CONTENT_SUFFIX}"))
            )
            .unwrap(),
            br#"{"provider": "user-typed"}"#,
            "captured profile holds the external edit verbatim"
        );
        assert_eq!(
            fs::read(&fx.fixed_path).unwrap(),
            br#"{"provider": "initial"}"#
        );
        assert!(
            fx.store
                .list_profiles()
                .unwrap()
                .iter()
                .any(|p| p.name == "user-edit")
        );

        // Discard: the external edit is explicitly thrown away.
        fs::write(&fx.fixed_path, br#"{"provider": "user-typed-2"}"#).unwrap();
        fx.store
            .activate_profile(
                &Fixture::name("p1"),
                &fx.fixed_path,
                &ReconcileChoice::Discard,
                &opts,
            )
            .unwrap();
        assert_eq!(
            fs::read(&fx.fixed_path).unwrap(),
            br#"{"provider": "initial"}"#
        );
    }

    #[test]
    fn capture_over_the_activated_profile_is_refused() {
        let fx = Fixture::new("act-capture-self");
        let opts = ActivationOptions::default();
        fx.store
            .save_active_profile(&Fixture::name("p1"), &fx.fixed_path)
            .unwrap();
        fx.store
            .activate_profile(
                &Fixture::name("p1"),
                &fx.fixed_path,
                &ReconcileChoice::Abort,
                &opts,
            )
            .unwrap();
        fs::write(&fx.fixed_path, br#"{"provider": "edited"}"#).unwrap();
        let err = fx
            .store
            .activate_profile(
                &Fixture::name("p1"),
                &fx.fixed_path,
                &ReconcileChoice::CaptureAsProfile(Fixture::name("p1")),
                &opts,
            )
            .unwrap_err();
        assert!(matches!(err, CoreError::Validation { .. }));
    }

    #[test]
    fn concurrent_activation_blocked_by_live_lock() {
        let fx = Fixture::new("act-locked");
        fx.store
            .save_active_profile(&Fixture::name("p1"), &fx.fixed_path)
            .unwrap();
        // Simulate a concurrent holder: a lockfile naming a live pid —
        // this test process itself.
        let dir = fx.store.harness_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(LOCK_FILE_NAME),
            serde_json::to_vec(&LockFile {
                pid: std::process::id(),
                harness: "zcode".to_owned(),
                acquired_at: now_iso8601(),
            })
            .unwrap(),
        )
        .unwrap();

        let err = fx
            .store
            .activate_profile(
                &Fixture::name("p1"),
                &fx.fixed_path,
                &ReconcileChoice::Discard,
                &ActivationOptions::default(),
            )
            .unwrap_err();
        match err {
            CoreError::ActivationLockHeld { path, holder_pid } => {
                assert!(path.ends_with(LOCK_FILE_NAME));
                assert_eq!(holder_pid, Some(std::process::id()));
            }
            other => panic!("expected ActivationLockHeld, got {other:?}"),
        }
        // The foreign config is untouched.
        assert_eq!(
            fs::read(&fx.fixed_path).unwrap(),
            br#"{"provider": "initial"}"#
        );
    }

    #[test]
    fn unparsable_lockfile_is_recovered_as_stale() {
        let dir = tmp_dir("act-lock-corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(LOCK_FILE_NAME), b"corrupt-not-json").unwrap();
        assert!(lock_is_stale(&dir.join(LOCK_FILE_NAME)));
        let lock = ActivationLock::acquire(&dir, "zcode").unwrap();
        assert!(dir.join(LOCK_FILE_NAME).exists());
        drop(lock);
        assert!(!dir.join(LOCK_FILE_NAME).exists(), "lock released on drop");
        drop(fs::remove_dir_all(&dir));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stale_lock_from_dead_pid_is_detected_and_recoverable() {
        // Spawn a short-lived child so its pid is provably dead afterwards.
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .status()
            .unwrap();
        assert!(status.success());
        let dir = tmp_dir("act-lock-stale");
        fs::create_dir_all(&dir).unwrap();
        // u32::MAX is beyond any allocated pid: /proc lookup proves liveness.
        fs::write(
            dir.join(LOCK_FILE_NAME),
            serde_json::to_vec(&LockFile {
                pid: u32::MAX,
                harness: "zcode".to_owned(),
                acquired_at: now_iso8601(),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(lock_is_stale(&dir.join(LOCK_FILE_NAME)));

        // Acquisition recovers the stale lock and proceeds.
        let lock = ActivationLock::acquire(&dir, "zcode").unwrap();
        assert!(dir.join(LOCK_FILE_NAME).exists());
        drop(lock);
        assert!(!dir.join(LOCK_FILE_NAME).exists());
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn store_refuses_to_live_inside_the_harness_config_tree() {
        let home = tmp_dir("act-store-inside");
        let harness_root = home.join(".zcode");
        let err = FixedPathProfileStore::new(
            &harness_root.join("v2").join("profiles"),
            HarnessId::new("zcode").unwrap(),
            &harness_root,
        )
        .unwrap_err();
        match err {
            CoreError::Validation { field, reason } => {
                assert_eq!(field, "profile_store");
                assert!(
                    reason.contains("outside the harness config tree"),
                    "{reason}"
                );
            }
            other => panic!("expected Validation, got {other:?}"),
        }
        // And equally for the store root being the harness root itself.
        let err2 = FixedPathProfileStore::new(
            &harness_root,
            HarnessId::new("zcode").unwrap(),
            &harness_root,
        )
        .unwrap_err();
        assert!(matches!(err2, CoreError::Validation { .. }));
        drop(fs::remove_dir_all(&home));
    }

    #[test]
    fn default_store_root_lives_outside_the_zcode_tree() {
        let home = tmp_dir("act-default-root");
        let layout = crate::adapters::zcode::fixed_path_layout(&home);
        let root = default_store_root(&home);
        assert!(root.starts_with(home.join(".superai")));
        assert!(
            !root.starts_with(&layout.harness_root),
            "default profile store must never sit inside the harness tree"
        );
        FixedPathProfileStore::new(
            &root,
            HarnessId::new("zcode").unwrap(),
            &layout.harness_root,
        )
        .unwrap();
        drop(fs::remove_dir_all(&home));
    }

    #[test]
    fn fixed_path_outside_the_harness_tree_is_refused() {
        let fx = Fixture::new("act-fixed-outside");
        let elsewhere = fx.home.join("elsewhere").join("config.json");
        let err = fx
            .store
            .save_active_profile(&Fixture::name("p1"), &elsewhere)
            .unwrap_err();
        match err {
            CoreError::Validation { field, reason } => {
                assert_eq!(field, "fixed_path");
                assert!(
                    reason.contains("inside the harness config tree"),
                    "{reason}"
                );
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn missing_profile_and_missing_fixed_path_are_typed_errors() {
        let fx = Fixture::new("act-missing");
        let err = fx
            .store
            .activate_profile(
                &Fixture::name("ghost"),
                &fx.fixed_path,
                &ReconcileChoice::Discard,
                &ActivationOptions::default(),
            )
            .unwrap_err();
        assert!(matches!(err, CoreError::Validation { .. }));

        let gone = fx.home.join(".zcode").join("v2").join("gone.json");
        let err2 = fx
            .store
            .save_active_profile(&Fixture::name("p1"), &gone)
            .unwrap_err();
        match err2 {
            CoreError::Validation { field, .. } => assert_eq!(field, "fixed_path"),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn tampered_profile_content_fails_digest_verification() {
        let fx = Fixture::new("act-tamper");
        fx.store
            .save_active_profile(&Fixture::name("p1"), &fx.fixed_path)
            .unwrap();
        // Tamper with the stored content out from under the metadata.
        fs::write(
            fx.store
                .harness_dir()
                .join(format!("p1{PROFILE_CONTENT_SUFFIX}")),
            br#"{"provider": "tampered"}"#,
        )
        .unwrap();
        let err = fx
            .store
            .activate_profile(
                &Fixture::name("p1"),
                &fx.fixed_path,
                &ReconcileChoice::Discard,
                &ActivationOptions::default(),
            )
            .unwrap_err();
        match err {
            CoreError::Verification { kind, .. } => assert_eq!(kind, "digest"),
            other => panic!("expected digest Verification, got {other:?}"),
        }
        assert_eq!(
            fs::read(&fx.fixed_path).unwrap(),
            br#"{"provider": "initial"}"#
        );
    }

    #[test]
    fn list_and_remove_profiles() {
        let fx = Fixture::new("act-list-remove");
        let opts = ActivationOptions::default();
        fx.store
            .save_active_profile(&Fixture::name("b"), &fx.fixed_path)
            .unwrap();
        fs::write(&fx.fixed_path, br#"{"provider": "second"}"#).unwrap();
        fx.store
            .save_active_profile(&Fixture::name("a"), &fx.fixed_path)
            .unwrap();

        let names: Vec<String> = fx
            .store
            .list_profiles()
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, vec!["a".to_owned(), "b".to_owned()]);

        let err = fx
            .store
            .remove_profile(&Fixture::name("ghost"))
            .unwrap_err();
        assert!(matches!(err, CoreError::Validation { .. }));

        let removed = fx.store.remove_profile(&Fixture::name("a")).unwrap();
        assert!(!removed.was_active);
        assert!(
            fx.store
                .list_profiles()
                .unwrap()
                .iter()
                .all(|p| p.name != "a")
        );

        // Activating then removing the active profile reports was_active.
        fx.store
            .activate_profile(
                &Fixture::name("b"),
                &fx.fixed_path,
                &ReconcileChoice::Discard,
                &opts,
            )
            .unwrap();
        let removed_active = fx.store.remove_profile(&Fixture::name("b")).unwrap();
        assert!(removed_active.was_active);
        // The applied content is still on disk: identity is content-derived.
        assert_eq!(
            fs::read(&fx.fixed_path).unwrap(),
            br#"{"provider": "initial"}"#
        );
    }

    #[test]
    fn journaled_activation_leaves_no_journal_residue_on_success() {
        let fx = Fixture::new("act-journal");
        let journal_root = fx.home.join(".superai").join("journal");
        fx.store
            .save_active_profile(&Fixture::name("p1"), &fx.fixed_path)
            .unwrap();
        fx.store
            .activate_profile(
                &Fixture::name("p1"),
                &fx.fixed_path,
                &ReconcileChoice::Abort,
                &ActivationOptions {
                    journal_root: Some(journal_root.clone()),
                },
            )
            .unwrap();
        assert!(
            !journal_root
                .read_dir()
                .is_ok_and(|mut d| d.next().is_some()),
            "verified activation must remove its crash journal"
        );
        assert_eq!(
            fs::read(&fx.fixed_path).unwrap(),
            br#"{"provider": "initial"}"#
        );
    }

    #[test]
    fn activation_options_for_home_points_at_the_superai_journal() {
        let opts = ActivationOptions::for_home(Path::new("/home/tester"));
        assert_eq!(
            opts.journal_root,
            Some(PathBuf::from("/home/tester/.superai/journal"))
        );
    }
}
