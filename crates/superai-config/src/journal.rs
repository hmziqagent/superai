//! Operation journal and crash recovery (MUT-09).
//!
//! The transaction layer writes a small journal file before its mutations and
//! at every phase transition, and removes it only after verified completion.
//! A journal left behind at startup marks an abandoned operation; recovery
//! inspects the actual filesystem state and restores each resource from its
//! recorded backup — it NEVER replays writes from stale staged content.
//!
//! Journals live under `<home>/.superai/journal/<operation-id>.journal.json`
//! and contain no config contents and no secrets: only paths, backup ids,
//! phase, and redacted diagnostics.

use std::path::{Path, PathBuf};

use crate::backup::{BackupId, find_backup_by_id, restore_verified, verify_backup};
use crate::error::{ConfigError, Result};

/// Directory holding pending operation journals for `home`.
///
/// Sibling of the registry records (`<home>/.superai`).
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
    /// Committing (or committed) — `completed` lists steps that landed.
    Commit,
    /// Commit finished; verification pending.
    Verify,
    /// Rolling back after a failure.
    Rollback,
    /// Fully completed and verified (journal is about to be removed).
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

/// Minimal journal record written to disk; no secrets and no contents.
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

    /// Serialize and atomically write the journal to `path`.
    ///
    /// Uses the crate's production atomic write (temp + rename + read-back),
    /// so an interrupted journal write can never leave a half-written file.
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

/// Perform startup recovery for `home` (MUT-09).
///
/// Scans `<home>/.superai/journal/*.journal.json` and recovers each abandoned
/// operation by inspecting the actual filesystem: stale staged temps are
/// removed, resources whose current bytes differ from their recorded backup
/// are restored (with a fresh backup of the current bytes first, so a
/// post-crash edit is never lost), committed creations are removed, and the
/// journal file itself is removed only once nothing residual remains.
/// Nothing is ever written from stale planned content.
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

/// Recover one journal file (see [`recover_pending`]).
pub fn recover_journal_file(journal_path: &Path) -> Result<JournalRecovery> {
    let Some(journal) = CrashJournal::load_from(journal_path)? else {
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

/// Remove stale staged temps: the ones the journal recorded, plus unrecorded
/// `.tmp.` siblings next to each resource (a crash between staging and the
/// journal update can leave those behind). Returns (removed, residuals).
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
        let dir = match PathBuf::from(res).parent() {
            Some(parent) => parent.to_path_buf(),
            None => PathBuf::from("."),
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in entries.flatten() {
            if !ent.file_name().to_string_lossy().starts_with(".tmp.") {
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
/// pre-transaction backup (deterministic rollback; `restore_verified` backs
/// up the current bytes first, so a post-crash edit stays recoverable).
/// Returns (restored, residuals).
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

/// Remove committed creations: resources without a recorded backup that the
/// journal shows as committed. A resource the journal shows as never
/// committed was not mutated by this operation and is left untouched.
/// Returns (removed, residuals).
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
        journal.backups.push(JournalBackup {
            resource: "/tmp/a.json".to_owned(),
            backup_id: "1-0001".to_owned(),
        });
        journal.completed.push("/tmp/a.json".to_owned());
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
        // ...and the post-crash edit is recoverable: restore_verified took a
        // backup of the current bytes before replacing them.
        let backups = crate::backup::list_backups(&resource).unwrap();
        assert!(
            backups.iter().any(|b| b.digest != entry.digest),
            "the post-crash bytes must exist as a backup, found {:?}",
            backups.len()
        );
        drop(std::fs::remove_dir_all(&home));
    }

    #[test]
    fn recovery_removes_stray_temp_files_next_to_resources() {
        let home = home_dir();
        let resource = home.join("a.json");
        std::fs::write(&resource, b"x").unwrap();
        let stray = home.join(".tmp.a.json.abcd.123");
        std::fs::write(&stray, b"stale").unwrap();
        let unrelated = home.join(".tmp.untracked");
        std::fs::write(&unrelated, b"keep").unwrap();
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
            "stray transaction temp removed ({:?})",
            report.journals
        );
        assert!(
            !unrelated.exists(),
            "the sibling sweep covers untracked `.tmp.` files in the resource directory"
        );
        drop(std::fs::remove_dir_all(&home));
    }
}
