use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{ConfigError, Result};

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

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "windows has no POSIX chmod; keeps the unix call sites uniform"
)]
fn apply_mode(path: &Path, _mode: u32) -> Result<()> {
    let _ = path;
    Ok(())
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
            Err(e) => Err(ConfigError::io(parent, e)),
        },
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
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_expecting(path, bytes, WriteExpectation::Any, None)
}

/// Atomically write `bytes` to `path`, failing if the current file digest
/// does not match `expected_digest`.
///
/// `expected_digest` is `None` for a file that is expected not to exist.
/// When `Some`, the current on-disk digest (or empty for missing) must match
/// exactly or a `ConcurrentModification` error is returned. This implements
/// MUT-01 conflict detection.
///
/// The function also rechecks for concurrent modification between temp
/// creation and rename even when `expected_digest` is `None` (detecting any
/// change during the preparation window).
pub fn atomic_write_with_expected_digest(
    path: &Path,
    bytes: &[u8],
    expected_digest: Option<&str>,
) -> Result<()> {
    let expectation = match expected_digest {
        Some(digest) => WriteExpectation::Digest(digest),
        None => WriteExpectation::Missing,
    };
    atomic_write_expecting(path, bytes, expectation, None)
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

    {
        use std::io::Write;
        file.write_all(bytes)
            .map_err(|e| ConfigError::io(&temp_path, e))?;
        file.flush().map_err(|e| ConfigError::io(&temp_path, e))?;
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

    let mut rename_attempts: u64 = 0;
    loop {
        match std::fs::rename(&temp_path, path) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && rename_attempts < 3 => {
                rename_attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(10 * rename_attempts));
            }
            Err(e) => {
                drop(std::fs::remove_file(&temp_path));
                return Err(ConfigError::io(path, e));
            }
        }
    }

    sync_parent(path)?;

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

/// Convenience wrapper that takes an optional [`crate::snapshot::Snapshot`]
/// as the expected token.
///
/// If `expected` is `Some`, its digest must match the current on-disk digest
/// or the write is aborted with `ConcurrentModification`.
pub fn atomic_write_with_snapshot(
    path: &Path,
    bytes: &[u8],
    expected: Option<&crate::snapshot::Snapshot>,
) -> Result<()> {
    let expected_digest = expected.and_then(|s| s.digest.as_deref());
    atomic_write_with_expected_digest(path, bytes, expected_digest)
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
        let res = atomic_write_with_expected_digest(&path, b"new", Some(&snap_digest));
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
        atomic_write_with_expected_digest(&path, b"fresh", None).unwrap();
        let read = std::fs::read(&path).unwrap();
        assert_eq!(read, b"fresh");
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn atomic_write_with_none_expected_fails_if_file_appeared_concurrently() {
        let path = unique_scratch("atomic-appeared");
        drop(std::fs::remove_file(&path));
        std::fs::write(&path, b"concurrent").unwrap();
        let res = atomic_write_with_expected_digest(&path, b"new", None);
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
        let res = atomic_write_with_snapshot(&path, b"v3", Some(&snap));
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
}
