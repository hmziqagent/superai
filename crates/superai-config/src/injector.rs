//! Mutation-boundary failure injection (QAL-06): production paths take an
//! optional [`Injector`] fired at every [`Point`]; `None` costs one branch.

/// A boundary in the mutation pipeline that can fail. The set mirrors the
/// failure tests plus the §4.2 recheck; variant order is stable.
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
    /// The prepare-to-commit recheck (§4.2): on-disk state vs the expectation.
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
    /// Journal written at `plan`; a failure simulates a crash there (MUT-09).
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

/// Deterministic failure injection: `Err` simulates the boundary failing.
/// Implementations stay cheap and side-effect free apart from counters.
pub trait Injector: Send + Sync + std::fmt::Debug {
    /// Possibly fail for `point`.
    fn inject(&self, point: Point) -> crate::Result<()>;
}

/// Invoke an optional injector, short-circuiting on failure.
pub(crate) fn run(injector: Option<&dyn Injector>, point: Point) -> crate::Result<()> {
    if let Some(injector) = injector {
        injector.inject(point)
    } else {
        Ok(())
    }
}
