//! Operation journal and crash recovery (MUT-09).
//!
//! A journal is written before mutations and at every phase transition, and
//! removed only after verified completion. Startup recovery inspects the real
//! filesystem state and restores each resource from its recorded backup; it
//! never replays stale staged content. Journals hold paths, backup ids,
//! phase, and redacted diagnostics only: no contents, no secrets.
//! Backups and digests are integrity-checked, not authenticated: they are
//! unkeyed and recomputable, so a writer with access to the journal and
//! resource directories can forge records that recovery will act on.
//! Closing that boundary needs a keyed digest and a key store superai
//! does not have.

use std::path::{Path, PathBuf};

use crate::backup::{BackupId, find_backup_by_id, restore_verified, verify_backup};
use crate::error::{ConfigError, Result};

/// Pending-operation journals for `home`, under `<home>/.superai/journal`.
pub fn journal_dir(home: &Path) -> PathBuf {
    home.join(".superai").join("journal")
}

/// Journal file path for `operation_id` inside `journal_root`.
pub fn journal_path(journal_root: &Path, operation_id: &str) -> PathBuf {
    journal_root.join(format!("{operation_id}.journal.json"))
}

/// Phase of an operation reached before a crash (MUT-09).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalPhase {
    /// Plan validated; nothing mutated yet.
    Plan,
    /// Foreign files backed up; no temps staged yet.
    PrepareBackup,
    /// Temps staged and validated; no commits yet.
    StageTemp,
    /// Committing or committed; `completed` lists steps that landed.
    Commit,
    /// Commit finished; verification pending.
    Verify,
    /// Rolling back after a failure.
    Rollback,
    /// Fully completed and verified; the journal is about to be removed.
    Done,
}

impl std::fmt::Display for JournalPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Plan => "plan",
            Self::PrepareBackup => "prepare_backup",
            Self::StageTemp => "stage_temp",
            Self::Commit => "commit",
            Self::Verify => "verify",
            Self::Rollback => "rollback",
            Self::Done => "done",
        };
        f.write_str(s)
    }
}

/// Backup recorded in the journal for one resource.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JournalBackup {
    /// Original path the backup was taken of.
    pub resource: String,
    /// Stable backup id resolvable via the backup catalog.
    pub backup_id: String,
}

/// Minimal journal record written to disk; no secrets, no contents.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CrashJournal {
    /// Operation id.
    pub operation_id: String,
    /// Phase reached before the crash.
    pub phase: JournalPhase,
    /// Resource paths involved (no content).
    pub resources: Vec<String>,
    /// Backups created during prepare, per resource.
    #[serde(default)]
    pub backups: Vec<JournalBackup>,
    /// Staged temp paths (if any).
    pub staged_temps: Vec<String>,
    /// Primary paths of steps that were committed.
    #[serde(default)]
    pub completed: Vec<String>,
    /// Redacted diagnostics (no secret).
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

impl CrashJournal {
    /// Create a journal for `operation_id` covering `resources`.
    pub fn new(operation_id: &str, phase: JournalPhase, resources: Vec<String>) -> Self {
        Self {
            operation_id: operation_id.to_owned(),
            phase,
            resources,
            backups: Vec::new(),
            staged_temps: Vec::new(),
            completed: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    /// Serialize and atomically write the journal to `path`, so an
    /// interrupted write can never leave a half-written file.
    pub fn write_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
        }
        let json = serde_json::to_vec(self)
            .map_err(|_e| ConfigError::verification(path, "journal serialization failed"))?;
        crate::atomic::atomic_write(path, &json)
    }

    /// Load a journal from `path` if it exists.
    pub fn load_from(path: &Path) -> Result<Option<Self>> {
        match std::fs::read(path) {
            Ok(bytes) => {
                if bytes.is_empty() {
                    return Ok(None);
                }
                let journal: Self = serde_json::from_slice(&bytes)
                    .map_err(|e| ConfigError::io(path, std::io::Error::other(e)))?;
                Ok(Some(journal))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ConfigError::io(path, e)),
        }
    }

    /// Remove the journal file; `NotFound` is success.
    pub fn remove(path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ConfigError::io(path, e)),
        }
    }
}

/// Outcome of recovering one abandoned journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRecovery {
    /// Journal file that was processed.
    pub journal_path: PathBuf,
    /// Operation id of the recovered journal.
    pub operation_id: String,
    /// Phase the journal was abandoned at.
    pub phase: JournalPhase,
    /// Whether recovery completed without residuals.
    pub recovered: bool,
    /// Stale temp files removed.
    pub removed_temps: Vec<PathBuf>,
    /// Resources restored from their recorded backups.
    pub restored: Vec<PathBuf>,
    /// Creations (no backup) removed because the journal shows them committed.
    pub removed_creations: Vec<PathBuf>,
    /// Paths that could not be recovered; the journal is retained for them.
    pub residuals: Vec<PathBuf>,
    /// Human-readable outcome (redacted).
    pub outcome: String,
}

/// Aggregate recovery report for a journal directory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    /// One entry per journal file found.
    pub journals: Vec<JournalRecovery>,
}

impl RecoveryReport {
    /// Whether every journal recovered without residuals.
    pub fn all_recovered(&self) -> bool {
        self.journals.iter().all(|j| j.recovered)
    }
}

/// Startup recovery for `home` (MUT-09): remove stale temps, restore
/// resources that differ from their recorded backup (backing up current
/// bytes first), remove committed creations, drop the journal only when
/// nothing residual remains. Stale planned content is never written.
pub fn recover_pending(home: &Path) -> Result<RecoveryReport> {
    let dir = journal_dir(home);
    let entries = match std::fs::read_dir(&dir) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RecoveryReport::default());
        }
        Err(e) => return Err(ConfigError::io(&dir, e)),
    };
    let mut journals = Vec::new();
    for ent in entries {
        let ent = ent.map_err(|e| ConfigError::io(&dir, e))?;
        let name = ent.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.ends_with(".journal.json") {
            continue;
        }
        journals.push(recover_journal_file(&ent.path())?);
    }
    Ok(RecoveryReport { journals })
}

/// Recover one journal file (see [`recover_pending`]). A journal that
/// cannot be read or parsed is quarantined beside itself (renamed
/// `.corrupt`) and reported as its own outcome, so it never aborts the
/// recovery of the remaining journals.
pub fn recover_journal_file(journal_path: &Path) -> Result<JournalRecovery> {
    let journal = match CrashJournal::load_from(journal_path) {
        Ok(Some(journal)) => journal,
        Ok(None) => {
            // Nothing to recover; treat as done and remove the stray file.
            CrashJournal::remove(journal_path)?;
            return Ok(JournalRecovery {
                journal_path: journal_path.to_path_buf(),
                operation_id: String::new(),
                phase: JournalPhase::Done,
                recovered: true,
                removed_temps: Vec::new(),
                restored: Vec::new(),
                removed_creations: Vec::new(),
                residuals: Vec::new(),
                outcome: "no journal to recover".to_owned(),
            });
        }
        Err(cause) => return Ok(quarantine_corrupt_journal(journal_path, &cause)),
    };

    let (removed_temps, mut residuals) = remove_stale_temps(&journal);
    let (restored, restore_residuals) = restore_recorded_backups(&journal)?;
    residuals.extend(restore_residuals);
    let (removed_creations, creation_residuals) = remove_committed_creations(&journal);
    residuals.extend(creation_residuals);

    let recovered = residuals.is_empty();
    let outcome = if recovered {
        format!(
            "recovered operation {} from phase {}",
            journal.operation_id, journal.phase
        )
    } else {
        format!(
            "operation {} left {} residual(s) after recovery from phase {}",
            journal.operation_id,
            residuals.len(),
            journal.phase
        )
    };
    // Journal removal happens only after verified recovery (MUT-09).
    if recovered {
        CrashJournal::remove(journal_path)?;
    }
    Ok(JournalRecovery {
        journal_path: journal_path.to_path_buf(),
        operation_id: journal.operation_id,
        phase: journal.phase,
        recovered,
        removed_temps,
        restored,
        removed_creations,
        residuals,
        outcome,
    })
}

/// Set one unreadable or unparseable journal aside beside itself so the
/// scan can continue without it; the renamed file no longer matches the
/// `.journal.json` suffix, so later runs leave it for inspection. A rename
/// failure keeps the journal in place and reports it as a residual.
fn quarantine_corrupt_journal(journal_path: &Path, cause: &ConfigError) -> JournalRecovery {
    let operation_id = journal_path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_suffix(".journal"))
        .unwrap_or_default()
        .to_owned();
    let (recovered, outcome) = match quarantine_aside(journal_path) {
        Ok(aside) => (
            true,
            format!(
                "corrupt journal ({cause}) quarantined aside as {}",
                aside.display()
            ),
        ),
        Err(e) => (
            false,
            format!("corrupt journal ({cause}) could not be quarantined: {e}"),
        ),
    };
    JournalRecovery {
        journal_path: journal_path.to_path_buf(),
        operation_id,
        phase: JournalPhase::Done,
        recovered,
        removed_temps: Vec::new(),
        restored: Vec::new(),
        removed_creations: Vec::new(),
        residuals: if recovered {
            Vec::new()
        } else {
            vec![journal_path.to_path_buf()]
        },
        outcome,
    }
}

/// Rename the journal aside under a fresh `.corrupt.<millis>.<4hex>` name
/// (the backup naming idiom), so a repeat corruption of the same journal
/// name never overwrites prior quarantined evidence. Refuses after
/// repeated name collisions rather than clobber anything.
fn quarantine_aside(journal_path: &Path) -> std::io::Result<PathBuf> {
    for _ in 0..5 {
        let millis = crate::atomic::timestamp_millis_now();
        let suffix = crate::atomic::generate_random_suffix(millis);
        let mut aside_name = journal_path.as_os_str().to_os_string();
        aside_name.push(format!(".corrupt.{millis}.{suffix}"));
        let aside = PathBuf::from(aside_name);
        if std::fs::symlink_metadata(&aside).is_ok() {
            continue;
        }
        return std::fs::rename(journal_path, &aside).map(|()| aside);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no fresh quarantine name after repeated collisions",
    ))
}

/// Remove stale staged temps: the recorded ones plus unrecorded siblings
/// next to each resource that match superai's own temp naming
/// (`.tmp.<resource file name>.` from `atomic::generate_temp_path`).
/// Foreign `.tmp.*` files from other tools are never touched. Returns
/// (removed, residuals).
fn remove_stale_temps(journal: &CrashJournal) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut removed = Vec::new();
    let mut residuals = Vec::new();
    for staged in &journal.staged_temps {
        let p = PathBuf::from(staged);
        if p.exists() || std::fs::symlink_metadata(&p).is_ok() {
            match std::fs::remove_file(&p) {
                Ok(()) => removed.push(p),
                Err(_) => residuals.push(p),
            }
        }
    }
    for res in &journal.resources {
        let resource = PathBuf::from(res);
        // No file name to tie the pattern to: sweep nothing rather than
        // everything (a bare `.tmp.` prefix would match foreign temps too).
        let Some(file_name) = resource.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if file_name.is_empty() {
            continue;
        }
        let prefix = format!(".tmp.{file_name}.");
        let dir = match resource.parent() {
            Some(parent) => parent.to_path_buf(),
            None => PathBuf::from("."),
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in entries.flatten() {
            if !ent
                .file_name()
                .to_string_lossy()
                .starts_with(prefix.as_str())
            {
                continue;
            }
            let p = ent.path();
            if std::fs::remove_file(&p).is_ok() {
                removed.push(p);
            }
        }
    }
    (removed, residuals)
}

/// Restore each resource whose current bytes differ from its recorded
/// pre-transaction backup. Returns (restored, residuals).
fn restore_recorded_backups(journal: &CrashJournal) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut restored = Vec::new();
    let mut residuals = Vec::new();
    for backup in &journal.backups {
        let resource = PathBuf::from(&backup.resource);
        let id = BackupId::new(backup.backup_id.clone());
        let Ok(entry) = find_backup_by_id(&resource, &id) else {
            residuals.push(resource);
            continue;
        };
        let Some(entry) = entry else {
            residuals.push(resource);
            continue;
        };
        if !verify_backup(&entry)? {
            residuals.push(resource);
            continue;
        }
        let current_matches_pre = crate::snapshot::snapshot(&resource)
            .digest
            .as_deref()
            .is_some_and(|d| d == entry.digest.as_str());
        if current_matches_pre {
            // Already at pre-transaction bytes: nothing to do.
            continue;
        }
        match restore_verified(&entry) {
            Ok(_) => restored.push(resource),
            Err(_) => residuals.push(resource),
        }
    }
    Ok((restored, residuals))
}

/// Remove committed creations (resources without a backup that the journal
/// shows as committed); uncommitted paths are left untouched.
fn remove_committed_creations(journal: &CrashJournal) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut removed = Vec::new();
    let mut residuals = Vec::new();
    for res in &journal.resources {
        if journal.backups.iter().any(|b| b.resource == *res) {
            continue;
        }
        let resource = PathBuf::from(res);
        let committed = journal.completed.contains(res);
        let exists = resource.exists() || std::fs::symlink_metadata(&resource).is_ok();
        if committed && exists {
            match std::fs::remove_file(&resource) {
                Ok(()) => removed.push(resource),
                Err(_) => residuals.push(resource),
            }
        }
    }
    (removed, residuals)
}

// tests

#[cfg(test)]
mod tests {
    use super::*;

    fn home_dir() -> PathBuf {
        let dir = crate::test_util::temp_dir_unique("journal");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn recover_pending_without_journal_dir_is_a_clean_noop() {
        let home = home_dir();
        let report = recover_pending(&home).unwrap();
        assert!(report.journals.is_empty());
        assert!(report.all_recovered());
        drop(std::fs::remove_dir_all(&home));
    }

    #[test]
    fn journal_round_trips_and_keeps_secrets_out() {
        let dir = home_dir();
        let path = dir.join("j.json");
        let mut journal = CrashJournal::new("op-x", JournalPhase::Commit, vec![]);
        let resource = std::env::temp_dir()
            .join("a.json")
            .to_string_lossy()
            .into_owned();
        journal.backups.push(JournalBackup {
            resource: resource.clone(),
            backup_id: "1-0001".to_owned(),
        });
        journal.completed.push(resource);
        journal
            .diagnostics
            .push("sk-REDACTED-marker only".to_owned());
        journal.write_to(&path).unwrap();
        let loaded = CrashJournal::load_from(&path).unwrap().unwrap();
        assert_eq!(loaded, journal);
        CrashJournal::remove(&path).unwrap();
        assert!(CrashJournal::load_from(&path).unwrap().is_none());
        // Removing twice is fine (NotFound = success).
        CrashJournal::remove(&path).unwrap();
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn recovery_reports_residual_when_backup_catalog_is_missing() {
        let home = home_dir();
        let resource = home.join("res.json");
        std::fs::write(&resource, b"state").unwrap();
        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let mut journal = CrashJournal::new(
            "op-missing-backup",
            JournalPhase::Commit,
            vec![resource.to_string_lossy().into_owned()],
        );
        journal.backups.push(JournalBackup {
            resource: resource.to_string_lossy().into_owned(),
            backup_id: "1-9999".to_owned(),
        });
        journal
            .completed
            .push(resource.to_string_lossy().into_owned());
        let jpath = journal_path(&jroot, "op-missing-backup");
        journal.write_to(&jpath).unwrap();

        let report = recover_pending(&home).unwrap();
        assert!(!report.all_recovered(), "unresolvable backup is a residual");
        let rec = report
            .journals
            .first()
            .cloned()
            .expect("one journal was found");
        assert_eq!(rec.residuals, vec![resource.clone()]);
        assert!(jpath.exists(), "journal is retained while residuals stand");
        assert_eq!(
            std::fs::read(&resource).unwrap(),
            b"state",
            "recovery never writes planned content or touches unknown state"
        );
        drop(std::fs::remove_dir_all(&home));
    }

    #[test]
    fn recovery_never_removes_uncommitted_foreign_files() {
        let home = home_dir();
        // A journal claims a creation that was never committed, but the path
        // exists (someone else put it there): recovery must not delete it.
        let foreign = home.join("foreign-new.json");
        std::fs::write(&foreign, b"foreign").unwrap();
        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let journal = CrashJournal::new(
            "op-uncommitted",
            JournalPhase::StageTemp,
            vec![foreign.to_string_lossy().into_owned()],
        );
        journal
            .write_to(&journal_path(&jroot, "op-uncommitted"))
            .unwrap();

        let report = recover_pending(&home).unwrap();
        assert!(
            report.all_recovered(),
            "nothing this operation touched is unrecovered: {:?}",
            report.journals
        );
        assert_eq!(
            std::fs::read(&foreign).unwrap(),
            b"foreign",
            "uncommitted path is never deleted"
        );
        assert!(!journal_path(&jroot, "op-uncommitted").exists());
        drop(std::fs::remove_dir_all(&home));
    }

    #[test]
    fn recovery_removes_committed_creations() {
        let home = home_dir();
        let created = home.join("created.json");
        std::fs::write(&created, b"committed").unwrap();
        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let mut journal = CrashJournal::new(
            "op-creation",
            JournalPhase::Verify,
            vec![created.to_string_lossy().into_owned()],
        );
        journal
            .completed
            .push(created.to_string_lossy().into_owned());
        journal
            .write_to(&journal_path(&jroot, "op-creation"))
            .unwrap();

        let report = recover_pending(&home).unwrap();
        assert!(report.all_recovered());
        assert!(
            !created.exists(),
            "committed creation without a backup is rolled back by removal"
        );
        drop(std::fs::remove_dir_all(&home));
    }

    #[test]
    fn recovery_restores_committed_foreign_file_from_backup() {
        let home = home_dir();
        let resource = home.join("settings.json");
        std::fs::write(&resource, b"original").unwrap();
        // Production backup of the pre-op state.
        let entry = crate::backup::backup(&resource).unwrap().unwrap();
        // The operation committed new bytes before the crash.
        std::fs::write(&resource, b"committed-new").unwrap();

        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let mut journal = CrashJournal::new(
            "op-restore",
            JournalPhase::Verify,
            vec![resource.to_string_lossy().into_owned()],
        );
        journal.backups.push(JournalBackup {
            resource: resource.to_string_lossy().into_owned(),
            backup_id: entry.id.as_str().to_owned(),
        });
        journal
            .completed
            .push(resource.to_string_lossy().into_owned());
        journal
            .write_to(&journal_path(&jroot, "op-restore"))
            .unwrap();

        let report = recover_pending(&home).unwrap();
        assert!(report.all_recovered());
        assert_eq!(
            std::fs::read(&resource).unwrap(),
            b"original",
            "restore from the recorded backup"
        );
        assert!(!journal_path(&jroot, "op-restore").exists());
        drop(std::fs::remove_dir_all(&home));
    }

    #[test]
    fn recovery_preserves_post_crash_edit_via_pre_restore_backup() {
        let home = home_dir();
        let resource = home.join("edited.json");
        std::fs::write(&resource, b"original").unwrap();
        let entry = crate::backup::backup(&resource).unwrap().unwrap();
        // Committed bytes, then a foreign edit AFTER the crash.
        std::fs::write(&resource, b"post-crash-foreign-edit").unwrap();

        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let mut journal = CrashJournal::new(
            "op-postcrash",
            JournalPhase::Verify,
            vec![resource.to_string_lossy().into_owned()],
        );
        journal.backups.push(JournalBackup {
            resource: resource.to_string_lossy().into_owned(),
            backup_id: entry.id.as_str().to_owned(),
        });
        journal
            .write_to(&journal_path(&jroot, "op-postcrash"))
            .unwrap();

        let report = recover_pending(&home).unwrap();
        assert!(report.all_recovered());
        // Deterministic rollback restored the pre-op bytes...
        assert_eq!(std::fs::read(&resource).unwrap(), b"original");
        // The post-crash edit is recoverable: restore_verified backed it up.
        let backups = crate::backup::list_backups(&resource).unwrap();
        assert!(
            backups.iter().any(|b| b.digest != entry.digest),
            "the post-crash bytes must exist as a backup, found {:?}",
            backups.len()
        );
        drop(std::fs::remove_dir_all(&home));
    }

    #[test]
    fn recovery_sweeps_only_own_temp_pattern_next_to_resources() {
        let home = home_dir();
        let resource = home.join("a.json");
        std::fs::write(&resource, b"x").unwrap();
        let stray = home.join(".tmp.a.json.abcd.123");
        std::fs::write(&stray, b"stale").unwrap();
        let foreign = home.join(".tmp.untracked");
        std::fs::write(&foreign, b"other tool's file").unwrap();
        let other_resource = home.join(".tmp.b.json.beef.9");
        std::fs::write(&other_resource, b"concurrent neighbor").unwrap();
        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let journal = CrashJournal::new(
            "op-stray",
            JournalPhase::PrepareBackup,
            vec![resource.to_string_lossy().into_owned()],
        );
        journal.write_to(&journal_path(&jroot, "op-stray")).unwrap();

        let report = recover_pending(&home).unwrap();
        assert!(report.all_recovered());
        assert!(
            !stray.exists(),
            "a stale temp of this resource's replace is removed ({:?})",
            report.journals
        );
        assert!(
            foreign.exists(),
            "a foreign `.tmp.*` file with no tie to the resource survives"
        );
        assert!(
            other_resource.exists(),
            "a temp named for a different resource survives"
        );
        assert_eq!(
            std::fs::read(&foreign).unwrap(),
            b"other tool's file",
            "foreign temps keep their bytes"
        );
        drop(std::fs::remove_dir_all(&home));
    }

    // mutation-hardening behaviour tests

    #[test]
    fn journal_phase_display_matches_the_recorded_names() {
        assert_eq!(JournalPhase::Plan.to_string(), "plan");
        assert_eq!(JournalPhase::PrepareBackup.to_string(), "prepare_backup");
        assert_eq!(JournalPhase::StageTemp.to_string(), "stage_temp");
        assert_eq!(JournalPhase::Commit.to_string(), "commit");
        assert_eq!(JournalPhase::Verify.to_string(), "verify");
        assert_eq!(JournalPhase::Rollback.to_string(), "rollback");
        assert_eq!(JournalPhase::Done.to_string(), "done");
    }

    #[test]
    fn journal_write_to_creates_missing_parent_directories() {
        let dir = home_dir();
        let path = dir.join("deep/nested/parents/j.json");
        CrashJournal::new("op-parents", JournalPhase::Plan, vec![])
            .write_to(&path)
            .unwrap();
        assert_eq!(
            CrashJournal::load_from(&path)
                .unwrap()
                .unwrap()
                .operation_id,
            "op-parents"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn journal_load_from_surfaces_non_notfound_read_errors() {
        let dir = home_dir();
        let occupied = dir.join("occupied");
        std::fs::create_dir_all(&occupied).unwrap();
        let res = CrashJournal::load_from(&occupied);
        assert!(
            res.is_err(),
            "reading a directory is not NotFound: {:?}",
            res.map(|o| o.map(|j| j.operation_id))
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn journal_remove_surfaces_non_notfound_errors() {
        let dir = home_dir();
        let occupied = dir.join("occupied");
        std::fs::create_dir_all(&occupied).unwrap();
        let res = CrashJournal::remove(&occupied);
        assert!(
            res.is_err(),
            "removing a directory is not NotFound: {res:?}"
        );
        assert!(occupied.exists(), "the directory is still there");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn recover_pending_surfaces_journal_dir_read_errors() {
        let home = home_dir();
        let superai = home.join(".superai");
        std::fs::create_dir_all(&superai).unwrap();
        // read_dir fails with ENOTDIR, which is not NotFound: must surface.
        std::fs::write(superai.join("journal"), b"not a directory").unwrap();
        assert!(
            recover_pending(&home).is_err(),
            "ENOTDIR on the journal dir is not a clean no-op"
        );
    }

    #[test]
    fn recover_pending_ignores_files_that_are_not_journals() {
        let home = home_dir();
        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let stray = jroot.join("notes.txt");
        std::fs::write(&stray, b"not json").unwrap();

        let report = recover_pending(&home).unwrap();
        assert!(
            report.journals.is_empty(),
            "non-journal files are not recovered: {:?}",
            report.journals
        );
        assert_eq!(
            std::fs::read_to_string(&stray).unwrap(),
            "not json",
            "non-journal files are left in place"
        );
        drop(std::fs::remove_dir_all(&home));
    }

    #[test]
    fn recover_journal_file_disposes_of_empty_stray_journals() {
        let dir = home_dir();
        let path = dir.join("empty.journal.json");
        std::fs::write(&path, b"").unwrap();

        let rec = recover_journal_file(&path).unwrap();
        assert!(rec.recovered);
        assert_eq!(rec.operation_id, "");
        assert_eq!(rec.phase, JournalPhase::Done);
        assert_eq!(rec.outcome, "no journal to recover");
        assert!(!path.exists(), "the stray empty journal is removed");
        drop(std::fs::remove_dir_all(&dir));
    }

    /// One corrupt journal must not abort the scan: it is set aside with its
    /// bytes intact while the remaining journals recover normally.
    #[test]
    fn one_corrupt_journal_does_not_abort_recovery_of_the_rest() {
        let home = home_dir();
        let created = home.join("created.json");
        std::fs::write(&created, b"committed").unwrap();
        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();

        std::fs::write(jroot.join("op-bad.journal.json"), b"not json at all").unwrap();
        let mut good = CrashJournal::new(
            "op-good",
            JournalPhase::Verify,
            vec![created.to_string_lossy().into_owned()],
        );
        good.completed.push(created.to_string_lossy().into_owned());
        good.write_to(&journal_path(&jroot, "op-good")).unwrap();

        let report = recover_pending(&home).unwrap();
        assert!(
            report.all_recovered(),
            "the quarantined journal leaves no residual: {:?}",
            report.journals
        );
        assert_eq!(report.journals.len(), 2, "both journals get an outcome");
        let bad = report
            .journals
            .iter()
            .find(|j| j.operation_id == "op-bad")
            .expect("the corrupt journal has its own reported outcome");
        assert!(bad.recovered);
        assert!(
            bad.outcome.contains("corrupt") && bad.outcome.contains(".corrupt"),
            "the outcome names the quarantine: {}",
            bad.outcome
        );
        assert!(
            !jroot.join("op-bad.journal.json").exists(),
            "the corrupt journal no longer sits in the pending set"
        );
        let mut asides = quarantined_files(&jroot, "op-bad.journal.json.corrupt");
        assert_eq!(asides.len(), 1, "exactly one quarantine evidence file");
        let aside = asides.swap_remove(0);
        assert_eq!(
            std::fs::read(&aside).unwrap(),
            b"not json at all",
            "quarantine preserves the corrupt bytes for inspection"
        );
        assert!(
            !created.exists(),
            "the healthy journal still recovers its committed creation"
        );
        assert!(
            !journal_path(&jroot, "op-good").exists(),
            "the healthy journal is consumed as usual"
        );
        drop(std::fs::remove_dir_all(&home));
    }

    /// An unreadable journal is quarantined exactly like an unparseable one:
    /// the bytes are unverifiable either way. The chmod-000 variant needs
    /// the permissions to actually bind (root reads through them).
    #[cfg(unix)]
    #[test]
    fn an_unreadable_journal_is_quarantined_and_kept_for_inspection() {
        use std::os::unix::fs::PermissionsExt;
        let home = home_dir();
        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let path = journal_path(&jroot, "op-denied");
        std::fs::write(&path, b"never parseable").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let report = recover_pending(&home).unwrap();
        let rec = report
            .journals
            .first()
            .expect("the unreadable journal is reported, not skipped");
        assert!(rec.recovered, "{}", rec.outcome);
        assert!(rec.outcome.contains("corrupt"), "{}", rec.outcome);
        assert!(!path.exists(), "it is out of the pending set");
        assert_eq!(
            quarantined_files(&jroot, "op-denied.journal.json.corrupt").len(),
            1,
            "the quarantined file is retained"
        );
        drop(std::fs::remove_dir_all(&home));
    }

    /// Corrupting the same journal name twice must keep both evidence
    /// files: the aside name carries millis and hex, so a repeat quarantine
    /// never overwrites the earlier copy.
    #[test]
    fn repeat_corruption_of_the_same_journal_name_keeps_both_evidence_files() {
        let home = home_dir();
        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let pending = jroot.join("op-again.journal.json");

        std::fs::write(&pending, b"first corruption").unwrap();
        let first = recover_pending(&home).unwrap();
        assert!(first.all_recovered(), "{:?}", first.journals);

        std::fs::write(&pending, b"second corruption").unwrap();
        let second = recover_pending(&home).unwrap();
        assert!(second.all_recovered(), "{:?}", second.journals);

        let asides = quarantined_files(&jroot, "op-again.journal.json.corrupt");
        assert_eq!(
            asides.len(),
            2,
            "both corruption events leave their own evidence file: {asides:?}"
        );
        let mut bodies: Vec<Vec<u8>> = asides.iter().map(|p| std::fs::read(p).unwrap()).collect();
        bodies.sort();
        assert_eq!(
            bodies,
            vec![b"first corruption".to_vec(), b"second corruption".to_vec()],
            "each quarantine retains its own bytes"
        );
        assert!(!pending.exists(), "nothing stays in the pending set");
        drop(std::fs::remove_dir_all(&home));
    }

    /// Quarantine evidence files in `dir` whose name starts with `prefix`,
    /// sorted by name.
    fn quarantined_files(dir: &Path, prefix: &str) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(prefix))
            })
            .collect();
        found.sort();
        found
    }

    /// When the quarantine rename itself fails, the journal stays in place
    /// and is reported as a residual instead of being silently dropped.
    #[cfg(unix)]
    #[test]
    fn a_corrupt_journal_that_cannot_be_renamed_is_reported_as_a_residual() {
        use std::os::unix::fs::PermissionsExt;
        let home = home_dir();
        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let path = journal_path(&jroot, "op-stuck");
        std::fs::write(&path, b"corrupt bytes").unwrap();
        std::fs::set_permissions(&jroot, std::fs::Permissions::from_mode(0o555)).unwrap();
        let probe = jroot.join("probe-write");
        let denied = std::fs::File::create(&probe).is_err();
        let rec = recover_journal_file(&path);
        std::fs::set_permissions(&jroot, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !denied {
            // Root bypasses the rename denial; the arm is unreachable here.
            assert!(rec.is_ok());
            drop(std::fs::remove_dir_all(&home));
            return;
        }
        let rec = rec.expect("a failed quarantine is a report, not an error");
        assert!(!rec.recovered, "{}", rec.outcome);
        assert_eq!(rec.residuals, vec![path.clone()]);
        assert!(path.exists(), "the journal is retained for the operator");
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_dir_all(&home));
    }

    #[cfg(unix)]
    #[test]
    fn recovery_removes_staged_broken_symlink_temps() {
        let home = home_dir();
        // Not named `.tmp.*`: the lstat fallback in exists() is what sees it.
        let broken = home.join("staged-broken-link");
        std::os::unix::fs::symlink("/definitely/not/present", &broken).unwrap();
        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let mut journal = CrashJournal::new(
            "op-broken-staged",
            JournalPhase::StageTemp,
            vec![home.join("resource.json").to_string_lossy().into_owned()],
        );
        journal
            .staged_temps
            .push(broken.to_string_lossy().into_owned());
        journal
            .write_to(&journal_path(&jroot, "op-broken-staged"))
            .unwrap();

        let report = recover_pending(&home).unwrap();
        assert!(report.all_recovered(), "{:?}", report.journals);
        assert!(
            std::fs::symlink_metadata(&broken).is_err(),
            "a staged broken symlink is a temp and must be removed"
        );
        let rec = report.journals.first().expect("one journal was found");
        assert_eq!(rec.removed_temps, vec![broken]);
    }

    #[cfg(unix)]
    #[test]
    fn recovery_removes_committed_creations_that_are_broken_symlinks() {
        let home = home_dir();
        let created = home.join("created-link.json");
        std::os::unix::fs::symlink("/definitely/not/present", &created).unwrap();
        let jroot = journal_dir(&home);
        std::fs::create_dir_all(&jroot).unwrap();
        let mut journal = CrashJournal::new(
            "op-link-creation",
            JournalPhase::Verify,
            vec![created.to_string_lossy().into_owned()],
        );
        journal
            .completed
            .push(created.to_string_lossy().into_owned());
        journal
            .write_to(&journal_path(&jroot, "op-link-creation"))
            .unwrap();

        let report = recover_pending(&home).unwrap();
        assert!(report.all_recovered(), "{:?}", report.journals);
        assert!(
            std::fs::symlink_metadata(&created).is_err(),
            "a committed creation that is a broken symlink is removed"
        );
        let rec = report.journals.first().expect("one journal was found");
        assert_eq!(rec.removed_creations, vec![created]);
    }
}
