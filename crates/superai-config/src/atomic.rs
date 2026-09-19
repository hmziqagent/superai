use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{ConfigError, Result};
use crate::injector::{Injector, Point, run as inject};

/// `SipHash` digest of `bytes` as 16 hex chars. Integrity token only: unkeyed,
/// so it proves nothing about who wrote the bytes.
pub(crate) fn compute_digest(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub(crate) fn timestamp_millis_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// Four-hex random suffix from time, pid, and a process-wide counter.
pub(crate) fn generate_random_suffix(millis: u128) -> String {
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

/// Permission bits the replacement file must carry: an explicit `mode` (backup
/// restore) wins; otherwise derived from the current target, or owner-only
/// `0o600` when the target is absent.
#[cfg(unix)]
pub(crate) fn resolve_final_mode(target: &Path, mode: Option<u32>) -> u32 {
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
pub(crate) fn resolve_final_mode(_target: &Path, mode: Option<u32>) -> u32 {
    mode.unwrap_or(0o600)
}

/// Apply `mode` to the file behind `file`, masked to the permission bits and
/// never zero access. `label` names the file in errors. Operating on the open
/// fd means a name swap between creation and rename cannot redirect the
/// chmod onto another file.
#[cfg(unix)]
pub(crate) fn apply_mode(file: &std::fs::File, label: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let masked = mode & 0o777;
    let safe_mode = if masked == 0 { 0o600 } else { masked };
    let perm = std::fs::Permissions::from_mode(safe_mode);
    file.set_permissions(perm)
        .map_err(|e| ConfigError::io(label, e))
}

/// Windows has only the readonly attribute, mapped from the owner-write bit;
/// the replacement must never be readonly or a later rename-over fails.
#[cfg(windows)]
pub(crate) fn apply_mode(file: &std::fs::File, label: &Path, mode: u32) -> Result<()> {
    let masked = mode & 0o777;
    let safe_mode = if masked == 0 { 0o600 } else { masked };
    let mut perm = file
        .metadata()
        .map(|m| m.permissions())
        .map_err(|e| ConfigError::io(label, e))?;
    perm.set_readonly(safe_mode & 0o200 == 0);
    file.set_permissions(perm)
        .map_err(|e| ConfigError::io(label, e))
}

#[cfg(not(any(unix, windows)))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "no POSIX chmod and no windows readonly bit; keeps call sites uniform"
)]
pub(crate) fn apply_mode(file: &std::fs::File, _label: &Path, _mode: u32) -> Result<()> {
    let _ = file;
    Ok(())
}

/// Clear the Windows readonly attribute from a file we are about to replace
/// or delete ourselves (winerror 5 otherwise).
#[cfg(windows)]
pub(crate) fn windows_clear_readonly(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path)
        && meta.permissions().readonly()
    {
        let mut perm = meta.permissions();
        #[expect(
            clippy::permissions_set_readonly_false,
            reason = "windows-only path; the readonly attribute is the only permission bit"
        )]
        perm.set_readonly(false);
        drop(std::fs::set_permissions(path, perm));
    }
}

/// Best-effort fsync of `path`'s parent after the file itself is durable.
/// Windows denies directory-handle syncs and opens on several filesystems.
pub(crate) fn sync_parent(path: &Path) -> Result<()> {
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
        // sync is a durability nicety, not a correctness requirement: the
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

/// Atomically write `bytes` to `path` via a same-directory exclusive temp;
/// the original is replaced only by rename. Crate-internal (crash journal,
/// tests): the public boundary is [`crate::transaction::commit_file`].
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_expecting(path, bytes, WriteExpectation::Any, None, None)
}

/// Required on-disk state of the target. Checked before the temp is created
/// and again once its bytes are flushed, so a target changing inside the
/// window aborts with `ConcurrentModification`, untouched.
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

/// Body of the atomic write family (MUT-04). `mode` picks the
/// replacement's permission bits: `None` derives them from the target
/// (0o600 for a new file); `Some` lands recorded bits verbatim (restore).
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
    // The temp is always created with O_EXCL and the handle is kept open until
    // the bytes are durable: nothing (a pre-planted symlink included) can make
    // this write truncate a file we did not create.
    let mut temp_path = PathBuf::new();
    let mut file: Option<std::fs::File> = None;
    for _ in 0..5 {
        let candidate = generate_temp_path(path)?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => {
                temp_path = candidate;
                file = Some(f);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(ConfigError::io(&candidate, e)),
        }
    }
    let Some(mut file) = file else {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "temp name collision after repeated attempts",
            ),
        ));
    };

    // Owner-only while empty, so payload bytes are never group/world readable
    // regardless of the process umask. The chmod goes through the held fd, so
    // it lands on the inode we created even if the temp name is swapped.
    if let Err(e) = apply_mode(&file, &temp_path, 0o600) {
        drop(std::fs::remove_file(&temp_path));
        return Err(e);
    }

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

    // Final mode after the bytes are durable but before the rename, so the
    // replacement never appears with the interim owner-only mode. Still via
    // the fd: a swapped name cannot redirect it onto a foreign file.
    if let Err(e) = apply_mode(&file, &temp_path, resolve_final_mode(path, mode)) {
        drop(std::fs::remove_file(&temp_path));
        return Err(e);
    }
    drop(file);

    // §4.2 / MUT-01: recheck immediately before the rename. A target that
    // changed anywhere inside the preparation window aborts untouched.
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
    #[cfg(windows)]
    windows_clear_readonly(path);
    let mut rename_attempts: u64 = 0;
    loop {
        match std::fs::rename(&temp_path, path) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && rename_attempts < 3 => {
                // Windows: antivirus or an indexer can hold a fresh file
                // briefly; retry with a growing delay.
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

    /// QAL-09: an atomic write must replace a readonly target. Unix keeps the
    /// target's own bits (0o444 stays); Windows clears the attribute and the
    /// replacement is never readonly.
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

    /// Files this crate creates are never readonly: unix lands owner-only
    /// 0o600, Windows never sets the attribute.
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
        /// chmod the parent directory to read-denied (0o333).
        #[cfg(unix)]
        DenyParentRead { parent: PathBuf },
        /// chmod the parent directory to write-denied (0o555).
        #[cfg(unix)]
        DenyParentWrite { parent: PathBuf },
        /// Remove the landed file and its parent directory.
        VanishParent { file: PathBuf, parent: PathBuf },
        /// Replace the parent directory with a symlink loop (ELOOP).
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
    /// for this process; root bypasses permission checks.
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

    /// Whether chmod 0o555 actually denies file creation for this process.
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

    /// Watchdog bound for calls into the rename-retry loop: the real code
    /// exhausts its three retries in 60ms of sleeps, so 5s is generous while
    /// staying under cargo-mutants' 30s scenario timeout.
    #[cfg(unix)]
    const RENAME_WATCHDOG: std::time::Duration = std::time::Duration::from_secs(5);

    /// Run `op` against a write-denied `parent` on a helper thread and
    /// demand its result within `RENAME_WATCHDOG`, so a non-terminating
    /// retry mutant fails fast instead of hanging the suite. On timeout the
    /// parent is made writable again so the abandoned thread can finish.
    #[cfg(unix)]
    fn with_rename_watchdog<T: Send + 'static>(
        parent: &Path,
        op: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // A send error only means the watchdog already timed out and
            // dropped the receiver; the calling test has already failed.
            drop(tx.send(op()));
        });
        match rx.recv_timeout(RENAME_WATCHDOG) {
            Ok(value) => value,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                set_dir_mode(parent, 0o755);
                panic!(
                    "the rename retry loop did not terminate within {RENAME_WATCHDOG:?} of a \
                     permanent PermissionDenied denial (bounded retries must exhaust)"
                );
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                set_dir_mode(parent, 0o755);
                panic!("the watched write thread ended without reporting a result");
            }
        }
    }

    // rename-retry behaviour

    /// An expectation digest matching the current file lets the write through.
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

    /// A failed temp write leaves the temp behind, so its name is observable:
    /// recent epoch millis plus a four-hex suffix.
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

    /// The parent sync tolerates EACCES on opening the parent (Windows);
    /// the write still succeeds.
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

    /// A parent vanishing after the rename is only noticed at the landed
    /// file's read-back, reported against the file, never the parent.
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

    /// Parent-open errors other than EACCES/ENOENT surface against the
    /// parent path (here ELOOP).
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

    /// A permanent rename denial spends the full growing backoff
    /// (10+20+30ms) before failing. Pairing the denied run with an
    /// identical control that fails at `Point::AtomicReplace` (no sleeps)
    /// and taking alternating-round minima cancels machine jitter, so the
    /// 40ms floor separates the real 60ms from a shrinking mutant's 18ms.
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
        // at the point the retry loop starts: no rename attempts, no sleeps.
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
        // The call runs under the rename watchdog so a non-terminating
        // retry mutant fails this test fast instead of hanging the suite.
        let run_denied = |round: usize| {
            let path = unique_scratch(&format!("atomic-backoff-denied-{round}"));
            let parent = path.parent().unwrap().to_path_buf();
            let sabotage = Sabotage {
                at: Point::AtomicReplace,
                action: SabotageAction::DenyParentWrite {
                    parent: parent.clone(),
                },
            };
            let (elapsed, res) = with_rename_watchdog(&parent, move || {
                let start = Instant::now();
                let res = atomic_write_expecting(
                    &path,
                    b"denied",
                    WriteExpectation::Any,
                    None,
                    Some(&sabotage),
                );
                (start.elapsed(), res)
            });
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

    /// A permanent rename denial surfaces as io `PermissionDenied` and the
    /// target never appears.
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
        let denied_path = path.clone();
        let res = with_rename_watchdog(&parent, move || {
            atomic_write_expecting(
                &denied_path,
                b"never",
                WriteExpectation::Any,
                None,
                Some(&sabotage),
            )
        });
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

    /// The retry loop terminates: a permanent `PermissionDenied` rename
    /// exhausts its retries and surfaces the io error instead of hanging
    /// the suite into the cargo-mutants timeout.
    #[cfg(unix)]
    #[test]
    fn atomic_write_rename_retry_loop_terminates() {
        let path = unique_scratch("atomic-rename-watchdog");
        let parent = path.parent().unwrap().to_path_buf();
        if !perm_denies_dir_write(&parent) {
            // DAC_OVERRIDE (e.g. root): rename cannot be denied for this
            // process; nothing to assert here.
            drop(std::fs::remove_dir_all(&parent));
            return;
        }
        let denied_parent = parent.clone();
        let res = with_rename_watchdog(&parent, move || {
            atomic_write_expecting(
                &path,
                b"payload",
                WriteExpectation::Any,
                None,
                Some(&Sabotage {
                    at: Point::AtomicReplace,
                    action: SabotageAction::DenyParentWrite {
                        parent: denied_parent,
                    },
                }),
            )
        });
        match res {
            Err(ConfigError::Io { source, .. }) => assert_eq!(
                source.kind(),
                std::io::ErrorKind::PermissionDenied,
                "the exhausted rename retries surface the denial"
            ),
            other => panic!("expected Io error from the exhausted retries, got {other:?}"),
        }
        set_dir_mode(&parent, 0o755);
        drop(std::fs::remove_dir_all(&parent));
    }
}
