use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{ConfigError, Result};
use crate::injector::{Injector, Point, run as inject};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compute_digest(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn timestamp_millis_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

fn generate_random_suffix(millis: u128) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = u64::from((millis & 0xffff_ffff) as u32)
        .wrapping_add(u64::from(std::process::id()))
        .wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed));
    let mut hasher = DefaultHasher::new();
    millis.hash(&mut hasher);
    count.hash(&mut hasher);
    let n = hasher.finish() & 0xFFFF;
    format!("{n:04x}")
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "kept Result for future fallible name generation"
)]
fn generate_temp_path(target: &Path) -> Result<PathBuf> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let millis = timestamp_millis_now();
    let suffix = generate_random_suffix(millis);
    let tmp_name = format!(".tmp.{file_name}.{suffix}.{millis}");
    Ok(parent.join(tmp_name))
}

/// Resolve the permission bits the replacement file must carry.
///
/// An explicit `mode` (used by backup restore to reinstate the permissions
/// recorded in the catalog entry) wins; otherwise the bits are derived from
/// the current target, falling back to owner-only `0o600` for a target that
/// does not exist or cannot be read.
#[cfg(unix)]
fn resolve_final_mode(target: &Path, mode: Option<u32>) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        return mode;
    }
    let derived = if target.exists() {
        match std::fs::metadata(target) {
            Ok(m) => m.permissions().mode() & 0o777,
            Err(_) => 0o600,
        }
    } else {
        0o600
    };
    if derived == 0 { 0o600 } else { derived }
}

#[cfg(not(unix))]
fn resolve_final_mode(_target: &Path, mode: Option<u32>) -> u32 {
    mode.unwrap_or(0o600)
}

/// Apply `mode` to `path`, masking to the permission bits and never leaving
/// the file with no access at all.
#[cfg(unix)]
fn apply_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let masked = mode & 0o777;
    let safe_mode = if masked == 0 { 0o600 } else { masked };
    let perm = std::fs::Permissions::from_mode(safe_mode);
    std::fs::set_permissions(path, perm).map_err(|e| ConfigError::io(path, e))
}

/// Windows permission semantics: POSIX mode bits do not exist beyond the
/// readonly attribute, and the only stable std API is
/// `Permissions::set_readonly`. The platform-correct projection of `0o600`
/// hardening is therefore: files we create or replace always carry the
/// owner-write bit (`0o200`) in the requested mode, which maps to "not
/// readonly" — the attribute that would block a later rename-over or delete
/// of the same file is never set by us.
#[cfg(windows)]
fn apply_mode(path: &Path, mode: u32) -> Result<()> {
    let masked = mode & 0o777;
    let safe_mode = if masked == 0 { 0o600 } else { masked };
    let mut perm = std::fs::metadata(path)
        .map(|m| m.permissions())
        .map_err(|e| ConfigError::io(path, e))?;
    // Only the owner-write bit has a Windows equivalent: without it the file
    // is readonly; with it the file is writable.
    perm.set_readonly(safe_mode & 0o200 == 0);
    std::fs::set_permissions(path, perm).map_err(|e| ConfigError::io(path, e))
}

#[cfg(not(any(unix, windows)))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "no POSIX chmod and no windows readonly bit; keeps call sites uniform"
)]
fn apply_mode(path: &Path, _mode: u32) -> Result<()> {
    let _ = path;
    Ok(())
}

/// Clear the Windows readonly attribute from `path` if set.
///
/// Used only where we are about to replace or delete the file ourselves
/// (rename-over in an atomic write, cleanup of our own temp/backup files):
/// a readonly destination makes `MoveFileEx`/delete fail with winerror 5,
/// so the attribute is cleared right before the replacement lands.
#[cfg(windows)]
pub(crate) fn windows_clear_readonly(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path)
        && meta.permissions().readonly()
    {
        let mut perm = meta.permissions();
        // Windows-only code path: the readonly attribute is the only bit that
        // exists there, so clearing it cannot make a unix file world-writable.
        #[expect(
            clippy::permissions_set_readonly_false,
            reason = "windows-only path; the readonly attribute is the only permission bit"
        )]
        perm.set_readonly(false);
        drop(std::fs::set_permissions(path, perm));
    }
}

fn sync_parent(path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    match std::fs::File::open(parent) {
        Ok(f) => match f.sync_all() {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => Ok(()),
            // Windows FlushFileBuffers on a directory handle is denied on
            // several filesystems; the data file itself is already synced,
            // so the parent sync is best-effort there.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(()),
            Err(e) => Err(ConfigError::io(parent, e)),
        },
        // Windows cannot open a directory handle without backup semantics,
        // so `File::open(parent)` fails with winerror 5 there. The parent
        // sync is a durability nicety, not a correctness requirement — the
        // replacement file was flushed and synced before the rename.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ConfigError::io(parent, e)),
    }
}

fn read_digest_if_exists(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(compute_digest(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ConfigError::io(path, e)),
    }
}

fn is_directory(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(m) => m.is_dir(),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Atomic write
// ---------------------------------------------------------------------------

/// Atomically write `bytes` to `path` via a same-directory temporary file.
///
/// Crate-internal since the plan-02 fold: this is the low-level replace
/// primitive used by the crash journal (superai's own bookkeeping file) and
/// this module's own tests — never a public write path. The ONE public
/// mutation boundary is [`crate::transaction::commit_file`] (plus the
/// multi-step [`crate::transaction::Transaction`]); everything else routes
/// there.
///
/// Steps:
/// 1. Create same-directory temp with exclusive name.
/// 2. Hold the temp owner-only while it carries bytes.
/// 3. Write bytes and flush.
/// 4. Apply the final permission bits (derived from the current target, or
///    owner-only for a new file) before the rename.
/// 5. Recheck the original state (detect change since the temp was started).
/// 6. Atomically rename via `std::fs::rename`.
/// 7. Sync parent directory where supported.
/// 8. Read back and verify digest and size.
///
/// Never truncates the original in place; the original is only replaced via
/// atomic rename.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_expecting(path, bytes, WriteExpectation::Any, None, None)
}

/// How the current on-disk state of the target must relate to the write.
///
/// The expectation is checked before the temporary file is created and again
/// once its bytes are flushed, so a target that changes anywhere inside the
/// preparation window aborts the write with `ConcurrentModification` and
/// leaves the target untouched.
#[derive(Clone, Copy, Debug)]
pub(crate) enum WriteExpectation<'a> {
    /// No prior-state requirement; only mid-write change detection applies.
    Any,
    /// The target must be absent.
    Missing,
    /// The target must currently carry exactly this digest.
    Digest(&'a str),
}

impl WriteExpectation<'_> {
    fn check(self, path: &Path, observed: Option<&str>) -> Result<()> {
        match self {
            Self::Any => Ok(()),
            Self::Missing => match observed {
                None => Ok(()),
                Some(actual) => Err(ConfigError::concurrent_modification(
                    path,
                    String::new(),
                    actual.to_owned(),
                )),
            },
            Self::Digest(expected) => {
                let actual = observed.unwrap_or_default();
                if actual == expected {
                    Ok(())
                } else {
                    Err(ConfigError::concurrent_modification(
                        path,
                        expected.to_owned(),
                        actual.to_owned(),
                    ))
                }
            }
        }
    }
}

/// Shared body of the atomic write family (MUT-04).
///
/// `mode` selects the permission bits the replacement carries: `None`
/// derives them from the current target (owner-only `0o600` for a new file);
/// `Some(mode)` applies the recorded bits verbatim, which is how backup
/// restore reinstates the original permissions. The temporary file is held
/// owner-only while it carries bytes, and the final mode is applied before
/// the rename so the replacement never appears with the interim mode.
#[expect(
    clippy::too_many_lines,
    reason = "atomic write steps are sequential and clearer together"
)]
pub(crate) fn atomic_write_expecting(
    path: &Path,
    bytes: &[u8],
    expectation: WriteExpectation<'_>,
    mode: Option<u32>,
    injector: Option<&dyn Injector>,
) -> Result<()> {
    if is_directory(path) {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "is a directory"),
        ));
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    let original_digest = read_digest_if_exists(path)?;
    expectation.check(path, original_digest.as_deref())?;

    inject(injector, Point::TempCreate)?;
    let mut temp_path: PathBuf = generate_temp_path(path)?;
    let mut attempts = 0;
    while temp_path.exists() && attempts < 5 {
        temp_path = generate_temp_path(path)?;
        attempts += 1;
    }

    let create_result: Result<std::fs::File> = (|| {
        for _ in 0..3 {
            let p = generate_temp_path(path)?;
            let open = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&p);
            match open {
                Ok(f) => {
                    temp_path = p;
                    return Ok(f);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(ConfigError::io(&p, e)),
            }
        }
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)
            .map_err(|e| ConfigError::io(&temp_path, e))?;
        Ok(f)
    })();

    let mut file = create_result?;

    drop(file);
    // The temp is held owner-only from creation until the final mode is
    // known, so payload bytes are never group/world readable regardless of
    // the process umask.
    if let Err(e) = apply_mode(&temp_path, 0o600) {
        drop(std::fs::remove_file(&temp_path));
        return Err(e);
    }
    file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&temp_path)
        .map_err(|e| ConfigError::io(&temp_path, e))?;

    inject(injector, Point::TempWrite)?;
    {
        use std::io::Write;
        file.write_all(bytes)
            .map_err(|e| ConfigError::io(&temp_path, e))?;
        file.flush().map_err(|e| ConfigError::io(&temp_path, e))?;
        inject(injector, Point::TempFlush)?;
        file.sync_all()
            .map_err(|e| ConfigError::io(&temp_path, e))?;
    }
    drop(file);

    // The final mode lands after the bytes are durable but before the
    // rename, so the replacement never appears with the interim owner-only
    // mode and a read-only recorded mode cannot block the write itself.
    if let Err(e) = apply_mode(&temp_path, resolve_final_mode(path, mode)) {
        drop(std::fs::remove_file(&temp_path));
        return Err(e);
    }

    // §4.2 / MUT-01: the conflict recheck immediately before the rename. A
    // target that changed anywhere inside the preparation window aborts the
    // write with ConcurrentModification and leaves the target untouched.
    inject(injector, Point::ConflictRecheck)?;
    let current_digest = read_digest_if_exists(path)?;
    if original_digest != current_digest {
        drop(std::fs::remove_file(&temp_path));
        let expected = original_digest.unwrap_or_default();
        let actual = current_digest.unwrap_or_default();
        return Err(ConfigError::concurrent_modification(path, expected, actual));
    }
    if let Err(e) = expectation.check(path, current_digest.as_deref()) {
        drop(std::fs::remove_file(&temp_path));
        return Err(e);
    }

    inject(injector, Point::AtomicReplace)?;
    // Windows: a readonly destination makes MoveFileEx-with-replace fail with
    // winerror 5. We are replacing the target right now, so clear the
    // attribute first; the replacement itself carries the final mode. Only
    // the destination is touched — never a file we merely read.
    #[cfg(windows)]
    windows_clear_readonly(path);
    let mut rename_attempts: u64 = 0;
    loop {
        match std::fs::rename(&temp_path, path) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && rename_attempts < 3 => {
                // Windows: antivirus/indexer can hold a fresh file briefly;
                // retry. Clearing readonly again covers the attribute having
                // been re-applied by another writer mid-window.
                #[cfg(windows)]
                windows_clear_readonly(path);
                rename_attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(10 * rename_attempts));
            }
            Err(e) => {
                drop(std::fs::remove_file(&temp_path));
                return Err(ConfigError::io(path, e));
            }
        }
    }

    inject(injector, Point::ParentSync)?;
    sync_parent(path)?;

    inject(injector, Point::ReadBackVerify)?;
    let read_back = std::fs::read(path).map_err(|e| ConfigError::io(path, e))?;
    let expected = compute_digest(bytes);
    let actual = compute_digest(&read_back);
    if expected != actual {
        return Err(ConfigError::verification(
            path,
            format!("digest mismatch after atomic write: expected {expected}, got {actual}"),
        ));
    }
    if read_back.len() != bytes.len() {
        return Err(ConfigError::verification(
            path,
            format!(
                "size mismatch after atomic write: expected {}, got {}",
                bytes.len(),
                read_back.len()
            ),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = crate::test_util::temp_dir_unique("config-atomic");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn unique_scratch(prefix: &str) -> PathBuf {
        let millis = timestamp_millis_now();
        let suffix = generate_random_suffix(millis);
        scratch(&format!("{prefix}-{millis}-{suffix}"))
    }

    #[test]
    fn atomic_write_creates_file_and_is_not_truncated() {
        let path = unique_scratch("atomic-create");
        drop(std::fs::remove_file(&path));
        let data = b"atomic content that must not be truncated";
        atomic_write(&path, data).unwrap();
        let read = std::fs::read(&path).unwrap();
        assert_eq!(read, data);
        assert_eq!(read.len(), data.len());
        drop(std::fs::remove_file(&path));
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        let parent = path.parent().unwrap();
        let entries = std::fs::read_dir(parent).unwrap();
        for e in entries.filter_map(std::result::Result::ok) {
            let n = e.file_name().to_string_lossy().into_owned();
            let prefix = format!(".tmp.{file_name}");
            assert!(
                !n.starts_with(prefix.as_str()),
                "temp file should be cleaned up: {n}"
            );
        }
    }

    #[test]
    fn atomic_write_overwrites_atomically_without_truncation() {
        let path = unique_scratch("atomic-overwrite");
        std::fs::write(&path, b"old content that is longer than new").unwrap();
        let new_data = b"new";
        atomic_write(&path, new_data).unwrap();
        let read = std::fs::read(&path).unwrap();
        assert_eq!(read, new_data);
        assert_ne!(read, b"old content that is longer than new");
        assert_eq!(read.len(), 3);
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn atomic_write_detects_concurrent_modification_via_expected_digest() {
        let path = unique_scratch("atomic-conflict");
        std::fs::write(&path, b"original").unwrap();
        let snap_digest = compute_digest(b"original");
        std::fs::write(&path, b"concurrent edit").unwrap();
        let res = atomic_write_expecting(
            &path,
            b"new",
            WriteExpectation::Digest(&snap_digest),
            None,
            None,
        );
        assert!(res.is_err(), "should detect concurrent modification");
        match res.unwrap_err() {
            ConfigError::ConcurrentModification { .. } => {}
            other => panic!("expected ConcurrentModification, got {other:?}"),
        }
        let cur = std::fs::read(&path).unwrap();
        assert_eq!(cur, b"concurrent edit");
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn atomic_write_with_none_expected_succeeds_for_new_file() {
        let path = unique_scratch("atomic-new-none");
        drop(std::fs::remove_file(&path));
        atomic_write_expecting(&path, b"fresh", WriteExpectation::Missing, None, None).unwrap();
        let read = std::fs::read(&path).unwrap();
        assert_eq!(read, b"fresh");
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn atomic_write_with_none_expected_fails_if_file_appeared_concurrently() {
        let path = unique_scratch("atomic-appeared");
        drop(std::fs::remove_file(&path));
        std::fs::write(&path, b"concurrent").unwrap();
        let res = atomic_write_expecting(&path, b"new", WriteExpectation::Missing, None, None);
        assert!(res.is_err());
        match res.unwrap_err() {
            ConfigError::ConcurrentModification { .. } => {}
            other => panic!("expected ConcurrentModification, got {other:?}"),
        }
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn atomic_write_rejects_directory() {
        let dir = crate::test_util::temp_dir_unique("config-atomic");
        std::fs::create_dir_all(&dir).unwrap();
        let res = atomic_write(&dir, b"data");
        assert!(res.is_err());
        drop(std::fs::remove_dir(&dir));
    }

    #[test]
    fn atomic_write_with_snapshot_uses_digest() {
        let path = unique_scratch("atomic-snap");
        std::fs::write(&path, b"v1").unwrap();
        let snap = crate::snapshot::snapshot(&path);
        std::fs::write(&path, b"v2").unwrap();
        let digest = snap.digest.unwrap_or_default();
        let res =
            atomic_write_expecting(&path, b"v3", WriteExpectation::Digest(&digest), None, None);
        assert!(res.is_err());
        let cur = std::fs::read(&path).unwrap();
        assert_eq!(cur, b"v2");
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn atomic_write_verifies_after_write() {
        let path = unique_scratch("atomic-verify");
        let data = b"verify me";
        atomic_write(&path, data).unwrap();
        let digest = compute_digest(data);
        let read = std::fs::read(&path).unwrap();
        assert_eq!(compute_digest(&read), digest);
        drop(std::fs::remove_file(&path));
    }

    /// Mark `path` readonly the platform-native way (unix mode bits, the
    /// Windows readonly attribute) for the write-over-readonly test.
    fn mark_readonly(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perm = std::fs::Permissions::from_mode(0o444);
            std::fs::set_permissions(path, perm).unwrap();
        }
        #[cfg(windows)]
        {
            let mut perm = std::fs::metadata(path).unwrap().permissions();
            perm.set_readonly(true);
            std::fs::set_permissions(path, perm).unwrap();
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
        }
    }

    /// QAL-09 platform case: an atomic write must be able to replace a file
    /// that currently carries the readonly state. The final state asserts the
    /// platform truth: unix derives the replacement mode from the existing
    /// target (0o444 stays 0o444 — the recorded mode is honored); Windows has
    /// no POSIX bits, the readonly attribute is cleared for the replacement
    /// and the replacement itself is never readonly.
    #[test]
    fn atomic_write_replaces_readonly_target_with_platform_correct_mode() {
        let path = unique_scratch("atomic-ro");
        std::fs::write(&path, b"old").unwrap();
        mark_readonly(&path);
        atomic_write(&path, b"new").unwrap();
        let read = std::fs::read(&path).unwrap();
        assert_eq!(read, b"new");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o444,
                "unix derives the replacement mode from the target"
            );
        }
        #[cfg(windows)]
        {
            let readonly = std::fs::metadata(&path).unwrap().permissions().readonly();
            assert!(
                !readonly,
                "windows replacement never carries the readonly attribute"
            );
        }
        // Restore writability so cleanup can delete the file.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perm = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&path, perm).unwrap();
        }
        drop(std::fs::remove_file(&path));
    }

    /// Files this crate creates are always owner-writable: unix lands
    /// owner-only `0o600`, Windows never sets the readonly attribute. This is
    /// the invariant that keeps every later rename-over/delete working.
    #[test]
    fn atomic_write_new_file_is_never_readonly() {
        let path = unique_scratch("atomic-fresh-mode");
        atomic_write(&path, b"fresh").unwrap();
        let perm = std::fs::metadata(&path).unwrap().permissions();
        assert!(
            !perm.readonly(),
            "a fresh atomic file must not be readonly on any platform"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = perm.mode() & 0o777;
            assert_eq!(mode, 0o600, "unix fresh files are owner-only 0o600");
        }
        drop(std::fs::remove_file(&path));
    }

    // ---- QAL-06 fault-injection helpers (test-only) ----

    /// Test injector that either fails at one point or sabotages the
    /// filesystem at one point, so the later pipeline phases run against the
    /// sabotaged on-disk state.
    #[derive(Debug)]
    struct Sabotage {
        /// The pipeline point the sabotage fires at.
        at: Point,
        /// What happens when it fires.
        action: SabotageAction,
    }

    #[derive(Debug)]
    enum SabotageAction {
        /// Return an injected error at the point.
        Fail,
        /// chmod the parent directory to read-denied (0o333). Unix-only
        /// premise (chmod modes); only unix-gated tests construct it.
        #[cfg(unix)]
        DenyParentRead { parent: PathBuf },
        /// chmod the parent directory to write-denied (0o555). Unix-only
        /// premise (chmod modes); only unix-gated tests construct it.
        #[cfg(unix)]
        DenyParentWrite { parent: PathBuf },
        /// Remove the landed file and its parent directory.
        VanishParent { file: PathBuf, parent: PathBuf },
        /// Replace the parent directory with a symlink loop. Unix-only
        /// premise (symlinks); only unix-gated tests construct it.
        #[cfg(unix)]
        LoopParent {
            file: PathBuf,
            parent: PathBuf,
            helper: PathBuf,
        },
    }

    impl Injector for Sabotage {
        fn inject(&self, point: Point) -> Result<()> {
            if point != self.at {
                return Ok(());
            }
            if matches!(self.action, SabotageAction::Fail) {
                return Err(ConfigError::io(
                    Path::new("sabotage"),
                    std::io::Error::other("injected failure"),
                ));
            }
            self.fire();
            Ok(())
        }
    }

    impl Sabotage {
        fn fire(&self) {
            match &self.action {
                SabotageAction::Fail => {}
                #[cfg(unix)]
                SabotageAction::DenyParentRead { parent } => set_dir_mode(parent, 0o333),
                #[cfg(unix)]
                SabotageAction::DenyParentWrite { parent } => set_dir_mode(parent, 0o555),
                SabotageAction::VanishParent { file, parent } => remove_file_and_dir(file, parent),
                #[cfg(unix)]
                SabotageAction::LoopParent {
                    file,
                    parent,
                    helper,
                } => replace_dir_with_symlink_loop(file, parent, helper),
            }
        }
    }

    /// chmod `dir` to `mode` (unix; a no-op elsewhere).
    #[cfg(unix)]
    fn set_dir_mode(dir: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        drop(std::fs::set_permissions(
            dir,
            std::fs::Permissions::from_mode(mode),
        ));
    }

    /// Remove `file` and then its (now empty) parent directory.
    fn remove_file_and_dir(file: &Path, parent: &Path) {
        drop(std::fs::remove_file(file));
        drop(std::fs::remove_dir(parent));
    }

    /// Remove `file` and its parent directory, then leave a symlink loop in
    /// the parent's place so directory operations fail with ELOOP.
    #[cfg(unix)]
    fn replace_dir_with_symlink_loop(file: &Path, parent: &Path, helper: &Path) {
        remove_file_and_dir(file, parent);
        std::os::unix::fs::symlink(helper, parent).unwrap();
        std::os::unix::fs::symlink(parent, helper).unwrap();
    }

    /// Whether chmod 0o333 actually denies opening the directory for reading
    /// for this process. Root bypasses permission checks; callers skip the
    /// denial-dependent assertions when it does.
    #[cfg(unix)]
    fn perm_denies_dir_read(dir: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        drop(std::fs::set_permissions(
            dir,
            std::fs::Permissions::from_mode(0o333),
        ));
        let denied = std::fs::File::open(dir).is_err();
        drop(std::fs::set_permissions(
            dir,
            std::fs::Permissions::from_mode(0o755),
        ));
        denied
    }

    /// Whether chmod 0o555 actually denies creating files in the directory
    /// for this process (root bypasses permission checks).
    #[cfg(unix)]
    fn perm_denies_dir_write(dir: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        drop(std::fs::set_permissions(
            dir,
            std::fs::Permissions::from_mode(0o555),
        ));
        let probe = dir.join("probe-write");
        let denied = std::fs::File::create(&probe).is_err();
        drop(std::fs::remove_file(&probe));
        drop(std::fs::set_permissions(
            dir,
            std::fs::Permissions::from_mode(0o755),
        ));
        denied
    }

    // ---- Behaviour tests for the mutation-testing gate ----

    /// An expectation digest that matches the current file must let the
    /// write through: optimistic concurrency succeeds when nothing changed.
    #[test]
    fn atomic_write_succeeds_when_digest_expectation_matches_unchanged_file() {
        let path = unique_scratch("atomic-digest-ok");
        std::fs::write(&path, b"v1").unwrap();
        let digest = crate::snapshot::snapshot(&path)
            .digest
            .expect("snapshot records a digest");
        atomic_write_expecting(&path, b"v2", WriteExpectation::Digest(&digest), None, None)
            .expect("a matching digest expectation must allow the write");
        assert_eq!(std::fs::read(&path).unwrap(), b"v2");
        drop(std::fs::remove_file(&path));
    }

    /// The write itself creates missing parent directories, so a target in
    /// not-yet-existing nested directories lands successfully.
    #[test]
    fn atomic_write_creates_missing_parent_directories() {
        let root = crate::test_util::temp_dir_unique("config-atomic-parents");
        let target = root.join("nested/deeper/settings.json");
        atomic_write(&target, b"parents").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"parents");
        drop(std::fs::remove_dir_all(&root));
    }

    /// A directory target is rejected up front with an `InvalidInput` io
    /// error (not some later filesystem error from trying to treat it as a
    /// file).
    #[test]
    fn atomic_write_rejects_directory_with_invalid_input() {
        let dir = crate::test_util::temp_dir_unique("config-atomic-dir");
        std::fs::create_dir_all(&dir).unwrap();
        match atomic_write(&dir, b"data") {
            Err(ConfigError::Io { source, .. }) => assert_eq!(
                source.kind(),
                std::io::ErrorKind::InvalidInput,
                "directory targets are rejected up front with InvalidInput"
            ),
            other => panic!("expected Io error, got {other:?}"),
        }
        drop(std::fs::remove_dir(&dir));
    }

    /// Replacing an existing file derives the replacement's permission bits
    /// from the current target: 0o644 in, 0o644 out.
    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_existing_target_mode() {
        use std::os::unix::fs::PermissionsExt;
        let path = unique_scratch("atomic-mode");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        atomic_write(&path, b"new").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o644,
            "the replacement must carry the mode derived from the target"
        );
        drop(std::fs::remove_file(&path));
    }

    /// A target that cannot be read at all surfaces an io error instead of
    /// being silently treated as absent.
    #[cfg(unix)]
    #[test]
    fn atomic_write_reports_io_error_when_target_becomes_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let path = unique_scratch("atomic-unreadable");
        std::fs::write(&path, b"secret").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&path).is_ok() {
            // DAC_OVERRIDE (e.g. root): the unreadable-target arm is
            // unreachable for this process; nothing to assert here.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            return;
        }
        match atomic_write(&path, b"new") {
            Err(ConfigError::Io { .. }) => {}
            other => panic!("expected Io error for an unreadable target, got {other:?}"),
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        drop(std::fs::remove_file(&path));
    }

    /// The temp file name embeds a current epoch-millis value and a compact
    /// four-hex-char random suffix; both are part of the collision-avoidance
    /// contract. A failed temp write leaves the temp behind, so the name is
    /// observable.
    #[test]
    fn lingering_temp_name_carries_recent_millis_and_compact_hex_suffix() {
        let path = unique_scratch("atomic-tempname");
        let sabotage = Sabotage {
            at: Point::TempWrite,
            action: SabotageAction::Fail,
        };
        let res = atomic_write_expecting(
            &path,
            b"payload",
            WriteExpectation::Any,
            None,
            Some(&sabotage),
        );
        assert!(res.is_err(), "the injected temp-write failure must surface");
        let parent = path.parent().unwrap().to_path_buf();
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let prefix = format!(".tmp.{file_name}.");
        let mut temps: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&parent)
            .unwrap()
            .filter_map(std::result::Result::ok)
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(prefix.as_str()) {
                temps.push(name);
            }
        }
        assert_eq!(temps.len(), 1, "exactly one temp should linger: {temps:?}");
        let name = temps.first().cloned().unwrap_or_default();
        let mut segments = name.rsplit('.');
        let millis = segments.next().unwrap_or_default().to_owned();
        let suffix = segments.next().unwrap_or_default().to_owned();
        let millis: u128 = millis.parse().unwrap_or(0);
        assert!(
            millis >= 1_600_000_000_000,
            "temp name must embed a post-2020 epoch millis value, got {name}"
        );
        assert_eq!(
            suffix.len(),
            4,
            "temp suffix must be four chars, got {name}"
        );
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f')),
            "temp suffix must be lowercase hex, got {name}"
        );
        drop(std::fs::remove_dir_all(&parent));
    }

    /// The parent-directory sync after the rename tolerates an EACCES on
    /// opening the parent (Windows opens directories without backup
    /// semantics); the write as a whole still succeeds and the content is
    /// readable back.
    #[cfg(unix)]
    #[test]
    fn sync_parent_tolerates_permission_denied_on_parent_open() {
        let path = unique_scratch("atomic-sync-eacces");
        let parent = path.parent().unwrap().to_path_buf();
        if !perm_denies_dir_read(&parent) {
            // DAC_OVERRIDE (e.g. root): the PermissionDenied open arm is
            // unreachable for this process; nothing to assert here.
            return;
        }
        let sabotage = Sabotage {
            at: Point::ParentSync,
            action: SabotageAction::DenyParentRead {
                parent: parent.clone(),
            },
        };
        atomic_write_expecting(
            &path,
            b"payload",
            WriteExpectation::Any,
            None,
            Some(&sabotage),
        )
        .expect("the parent sync must tolerate EACCES on the parent open");
        assert_eq!(std::fs::read(&path).unwrap(), b"payload");
        drop(std::fs::remove_dir_all(&parent));
    }

    /// The parent-directory sync tolerates the parent disappearing entirely
    /// (the replacement itself already landed and was synced): the loss is
    /// only noticed later, by the read-back of the landed file — reported
    /// against the file path, never against the parent.
    #[test]
    fn sync_parent_tolerates_vanishing_parent() {
        let path = unique_scratch("atomic-sync-enoent");
        let parent = path.parent().unwrap().to_path_buf();
        let sabotage = Sabotage {
            at: Point::ParentSync,
            action: SabotageAction::VanishParent {
                file: path.clone(),
                parent: parent.clone(),
            },
        };
        let res =
            atomic_write_expecting(&path, b"gone", WriteExpectation::Any, None, Some(&sabotage));
        match res {
            Err(ConfigError::Io {
                path: reported,
                source,
            }) => {
                assert_eq!(
                    reported, path,
                    "the vanished parent is tolerated; the loss surfaces at the file read-back"
                );
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected the read-back to report the vanished file, got {other:?}"),
        }
        assert!(!parent.exists());
    }

    /// A parent-open error that is neither `PermissionDenied` nor `NotFound` is
    /// surfaced as an io error reported against the parent path (here a
    /// symlink loop, ELOOP).
    #[cfg(unix)]
    #[test]
    fn sync_parent_surfaces_other_parent_open_errors() {
        let root = crate::test_util::temp_dir_unique("config-atomic-loop");
        let parent = root.join("inner");
        std::fs::create_dir_all(&parent).unwrap();
        let target = parent.join("file.json");
        let helper = root.join("inner-loop");
        let sabotage = Sabotage {
            at: Point::ParentSync,
            action: SabotageAction::LoopParent {
                file: target.clone(),
                parent: parent.clone(),
                helper: helper.clone(),
            },
        };
        let res = atomic_write_expecting(
            &target,
            b"looped",
            WriteExpectation::Any,
            None,
            Some(&sabotage),
        );
        match res {
            Err(ConfigError::Io { path, source }) => {
                assert_eq!(
                    path, parent,
                    "the parent-sync failure is reported against the parent"
                );
                // ELOOP has no stable ErrorKind; assert the OS error is set
                // so a plain permission/not-found mixup cannot pass either.
                assert!(
                    source.raw_os_error().is_some(),
                    "the surfaced error is an OS error, got {source}"
                );
            }
            other => panic!("expected Io error from the parent sync, got {other:?}"),
        }
        drop(std::fs::remove_file(&parent));
        drop(std::fs::remove_file(&helper));
        drop(std::fs::remove_dir(&root));
    }

    /// A rename that fails with `PermissionDenied` is retried on a GROWING
    /// backoff (`10 * attempt` ms), so a denial that never clears must burn
    /// the full budget — 10 + 20 + 30 = 60ms of sleeps — before the error
    /// surfaces. A shrinking schedule such as `10 / attempt` (10 + 5 + 3 =
    /// 18ms) gives up almost immediately, and a retry loop that never
    /// retries measures no backoff at all.
    ///
    /// Timing-robust discriminator, replacing the earlier 20ms-restorer
    /// race: loaded CI runners routinely spend more than 20ms on the write
    /// pipeline before the first rename attempt, so the denial sometimes
    /// cleared before any attempt and the shrinking-schedule mutant escaped
    /// (missed on CI run 34895207523). Instead, the denied write is paired
    /// with a CONTROL write that runs the identical pipeline (temp create,
    /// write, flush, fsync, mode apply, digest recheck) and fails at the
    /// very same `Point::AtomicReplace` via an injected error — zero
    /// retries, zero sleeps. The minimum elapsed over several rounds whose
    /// measurement order alternates converges both sides to the machine's
    /// best-case setup, so fsync/scheduler jitter cancels in the
    /// difference and only the sleep schedule remains. The 40ms threshold
    /// sits at the midpoint of the 60ms vs 18ms totals: the real code has
    /// a hard 20ms slack (a sleep never returns early, so its difference
    /// is always at least 60ms), while the shrinking mutant would need
    /// more than 22ms of cumulative sleep overshoot in its single best
    /// round to cross the threshold. `+=` mutants never return at all and
    /// are caught by the mutant timeout, not by this assertion.
    #[cfg(unix)]
    #[test]
    fn atomic_write_rename_retry_backoff_spends_the_full_delay_budget() {
        use std::time::{Duration, Instant};

        const ROUNDS: usize = 6;
        const BACKOFF_FLOOR: Duration = Duration::from_millis(40);

        let probe_parent = unique_scratch("atomic-backoff-probe")
            .parent()
            .unwrap()
            .to_path_buf();
        if !perm_denies_dir_write(&probe_parent) {
            // DAC_OVERRIDE (e.g. root): rename cannot be denied for this
            // process; nothing to assert here.
            drop(std::fs::remove_dir_all(&probe_parent));
            return;
        }
        drop(std::fs::remove_dir_all(&probe_parent));

        // Control measurement: identical pipeline, injected failure exactly
        // at the point the retry loop starts — no rename attempts, no sleeps.
        let run_control = |round: usize| {
            let path = unique_scratch(&format!("atomic-backoff-ctrl-{round}"));
            let parent = path.parent().unwrap().to_path_buf();
            let sabotage = Sabotage {
                at: Point::AtomicReplace,
                action: SabotageAction::Fail,
            };
            let start = Instant::now();
            let res = atomic_write_expecting(
                &path,
                b"control",
                WriteExpectation::Any,
                None,
                Some(&sabotage),
            );
            let elapsed = start.elapsed();
            assert!(
                res.is_err(),
                "the injected AtomicReplace failure must surface"
            );
            drop(std::fs::remove_dir_all(&parent));
            elapsed
        };
        // Denied measurement: the rename itself fails with PermissionDenied
        // and every retry sleeps the backoff delay before the final error.
        let run_denied = |round: usize| {
            let path = unique_scratch(&format!("atomic-backoff-denied-{round}"));
            let parent = path.parent().unwrap().to_path_buf();
            let sabotage = Sabotage {
                at: Point::AtomicReplace,
                action: SabotageAction::DenyParentWrite {
                    parent: parent.clone(),
                },
            };
            let start = Instant::now();
            let res = atomic_write_expecting(
                &path,
                b"denied",
                WriteExpectation::Any,
                None,
                Some(&sabotage),
            );
            let elapsed = start.elapsed();
            match res {
                Err(ConfigError::Io { source, .. }) => assert_eq!(
                    source.kind(),
                    std::io::ErrorKind::PermissionDenied,
                    "the exhausted retries surface the rename denial"
                ),
                other => panic!("expected Io error from rename, got {other:?}"),
            }
            set_dir_mode(&parent, 0o755);
            drop(std::fs::remove_dir_all(&parent));
            elapsed
        };

        // Discarded warmup: pays the one-time pipeline costs before timing.
        let _ = run_control(ROUNDS);
        let _ = run_denied(ROUNDS);

        let mut best_control = Duration::MAX;
        let mut best_denied = Duration::MAX;
        for round in 0..ROUNDS {
            // Alternating order cancels any run-first-of-the-round bias
            // (cold caches, journal contention) that a fixed order would
            // hand to one side's minimum.
            let (control, denied) = if round % 2 == 0 {
                let control = run_control(round);
                let denied = run_denied(round);
                (control, denied)
            } else {
                let denied = run_denied(round);
                let control = run_control(round);
                (control, denied)
            };
            best_control = best_control.min(control);
            best_denied = best_denied.min(denied);
        }

        let backoff = best_denied
            .checked_sub(best_control)
            .expect("the denied pipeline contains the control pipeline plus retries");
        assert!(
            backoff >= BACKOFF_FLOOR,
            "a permanent rename denial must spend the full growing backoff \
             (10+20+30 = 60ms of sleeps) before failing; measured only \
             {backoff:?} (best denied {best_denied:?} vs best control \
             {best_control:?}): the retry delays are not growing"
        );
    }

    /// A rename denial that never clears is reported as an io
    /// `PermissionDenied` error, and the target never appears.
    #[cfg(unix)]
    #[test]
    fn atomic_write_reports_permanent_rename_permission_error() {
        let path = unique_scratch("atomic-rename-permanent");
        let parent = path.parent().unwrap().to_path_buf();
        if !perm_denies_dir_write(&parent) {
            // DAC_OVERRIDE (e.g. root): rename cannot be denied for this
            // process; nothing to assert here.
            return;
        }
        let sabotage = Sabotage {
            at: Point::AtomicReplace,
            action: SabotageAction::DenyParentWrite {
                parent: parent.clone(),
            },
        };
        let res = atomic_write_expecting(
            &path,
            b"never",
            WriteExpectation::Any,
            None,
            Some(&sabotage),
        );
        match res {
            Err(ConfigError::Io { source, .. }) => assert_eq!(
                source.kind(),
                std::io::ErrorKind::PermissionDenied,
                "a permanent rename denial surfaces as PermissionDenied"
            ),
            other => panic!("expected Io error from rename, got {other:?}"),
        }
        assert!(
            !path.exists(),
            "the target must not appear when the rename never succeeds"
        );
        set_dir_mode(&parent, 0o755);
        drop(std::fs::remove_dir_all(&parent));
    }
}
