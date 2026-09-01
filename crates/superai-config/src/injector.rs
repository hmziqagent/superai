//! Failure-injection surface for the mutation family (QAL-06).
//!
//! Production mutation functions (`atomic`, `backup`, `transaction`) accept an
//! optional [`Injector`] trait object. The production code calls
//! [`Injector::inject`] at every boundary named by [`Point`]; when no injector
//! is supplied (`None`) the calls compile away to a single branch on an
//! `Option`, so the production path is the single implementation exercised by
//! both real runs and the failure matrix.
//!
//! The trait lives in this crate (not `superai-core`) because the mutation
//! family is layer 1; higher layers implement the trait with their own
//! deterministic counters (see `superai-core` `failure::TestInjector`).

/// A boundary in the mutation pipeline that can fail.
///
/// The set mirrors the failure tests required by subplan 02 plus the §4.2
/// prepare→commit conflict recheck. Ordering of variants is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Point {
    /// Opening the existing file for backup.
    BackupOpen,
    /// Writing the backup copy.
    BackupWrite,
    /// Flushing/syncing the backup.
    BackupFlush,
    /// Verifying the backup digest.
    BackupVerify,
    /// Creating the same-directory temp file.
    TempCreate,
    /// Writing staged content to the temp.
    TempWrite,
    /// Flushing/syncing the temp.
    TempFlush,
    /// Validating staged output (parse).
    ParseStaged,
    /// The prepare→commit conflict recheck (§4.2): comparing the current
    /// on-disk state to the expectation recorded at prepare time.
    ConflictRecheck,
    /// Atomic rename/replace.
    AtomicReplace,
    /// Parent directory sync.
    ParentSync,
    /// Reading back and verifying the digest after commit.
    ReadBackVerify,
    /// Verifying a rollback restore.
    RollbackVerify,
    /// The second file of a multi-file transaction.
    SecondFile,
    /// The third file of a multi-file transaction.
    ThirdFile,
    /// The operation journal was just written at the `plan` phase; a failure
    /// here simulates a crash with the journal left at that phase (MUT-09).
    JournalPlan,
    /// Journal advanced to `prepare_backup`; crash simulation.
    JournalPrepareBackup,
    /// Journal advanced to `stage_temp`; crash simulation.
    JournalStageTemp,
    /// Journal advanced to `commit`; crash simulation.
    JournalCommit,
    /// Journal advanced to `verify`; crash simulation.
    JournalVerify,
    /// Journal advanced to `rollback`; crash simulation.
    JournalRollback,
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::BackupOpen => "backup_open",
            Self::BackupWrite => "backup_write",
            Self::BackupFlush => "backup_flush",
            Self::BackupVerify => "backup_verify",
            Self::TempCreate => "temp_create",
            Self::TempWrite => "temp_write",
            Self::TempFlush => "temp_flush",
            Self::ParseStaged => "parse_staged",
            Self::ConflictRecheck => "conflict_recheck",
            Self::AtomicReplace => "atomic_replace",
            Self::ParentSync => "parent_sync",
            Self::ReadBackVerify => "read_back_verify",
            Self::RollbackVerify => "rollback_verify",
            Self::SecondFile => "second_file",
            Self::ThirdFile => "third_file",
            Self::JournalPlan => "journal_plan",
            Self::JournalPrepareBackup => "journal_prepare_backup",
            Self::JournalStageTemp => "journal_stage_temp",
            Self::JournalCommit => "journal_commit",
            Self::JournalVerify => "journal_verify",
            Self::JournalRollback => "journal_rollback",
        };
        f.write_str(s)
    }
}

/// Deterministic failure injection into production mutation paths.
///
/// `inject` returns `Err` to simulate the named boundary failing; `Ok(())`
/// lets the production code continue. Implementations must be cheap and
/// side-effect free apart from their counters.
pub trait Injector: Send + Sync + std::fmt::Debug {
    /// Possibly fail for `point`.
    fn inject(&self, point: Point) -> crate::Result<()>;
}

/// Invoke an optional injector, short-circuiting on failure.
///
/// `None` costs one branch — production callers pass `Option<&dyn Injector>`.
pub(crate) fn run(injector: Option<&dyn Injector>, point: Point) -> crate::Result<()> {
    if let Some(injector) = injector {
        injector.inject(point)
    } else {
        Ok(())
    }
}
