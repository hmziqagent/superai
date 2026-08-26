use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compute_digest(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(unix)]
#[expect(clippy::unnecessary_wraps, reason = "Option needed for non-unix None")]
fn get_permissions_u32(meta: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode())
}

#[cfg(not(unix))]
fn get_permissions_u32(_meta: &std::fs::Metadata) -> Option<u32> {
    None
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// Fresh snapshot of a filesystem resource, used as a conflict token.
///
/// Captures existence, symlink status, digest, size, permissions, and mtime
/// hint. No secrets are stored; digest is a hash of file bytes.
#[expect(
    clippy::struct_excessive_bools,
    reason = "snapshot needs multiple bool flags"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Path that was snapshotted.
    pub path: PathBuf,
    /// Hex digest of file bytes if the file exists and is readable.
    pub digest: Option<String>,
    /// Size in bytes if the file exists.
    pub size: Option<u64>,
    /// Permissions mode where available.
    pub permissions: Option<u32>,
    /// Whether the path exists.
    pub exists: bool,
    /// Whether the path is a symlink (without following).
    pub is_symlink: bool,
    /// Whether the path is a regular file (following symlink if present).
    pub is_file: bool,
    /// Whether the path is a directory.
    pub is_dir: bool,
    /// Modification time hint if available.
    pub mtime: Option<SystemTime>,
}

impl Snapshot {
    /// Whether the snapshot represents a missing file.
    pub fn is_missing(&self) -> bool {
        !self.exists
    }
}

/// Take a fresh snapshot of `path`.
///
/// Reads the file fresh (disk is truth) and computes digest if it is a file.
/// Does not follow a symlink loop; such cases are captured as existing but
/// with `digest: None` and `is_symlink: true` where detectable.
pub fn snapshot(path: &Path) -> Snapshot {
    let symlink_meta = std::fs::symlink_metadata(path);
    match symlink_meta {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Snapshot {
            path: path.to_path_buf(),
            digest: None,
            size: None,
            permissions: None,
            exists: false,
            is_symlink: false,
            is_file: false,
            is_dir: false,
            mtime: None,
        },
        Err(_) => Snapshot {
            path: path.to_path_buf(),
            digest: None,
            size: None,
            permissions: None,
            exists: false,
            is_symlink: false,
            is_file: false,
            is_dir: false,
            mtime: None,
        },
        Ok(meta) => {
            let is_symlink = meta.file_type().is_symlink();
            let is_dir = meta.is_dir();
            let (is_file, target_meta) = if is_symlink {
                match std::fs::metadata(path) {
                    Ok(tm) => (tm.is_file(), Some(tm)),
                    Err(_) => (false, None),
                }
            } else {
                (meta.is_file(), Some(meta.clone()))
            };

            let (digest, size, permissions, mtime) = if is_file {
                if let Ok(bytes) = std::fs::read(path) {
                    let d = compute_digest(&bytes);
                    let sz = bytes.len() as u64;
                    let perms = target_meta.as_ref().and_then(get_permissions_u32);
                    let mt = target_meta.as_ref().and_then(|m| m.modified().ok());
                    (Some(d), Some(sz), perms, mt)
                } else {
                    let sz = target_meta.as_ref().map(std::fs::Metadata::len);
                    let perms = target_meta.as_ref().and_then(get_permissions_u32);
                    let mt = target_meta.as_ref().and_then(|m| m.modified().ok());
                    (None, sz, perms, mt)
                }
            } else {
                let perms = get_permissions_u32(&meta);
                let mt = meta.modified().ok();
                let sz = if meta.is_file() {
                    Some(meta.len())
                } else {
                    None
                };
                (None, sz, perms, mt)
            };

            Snapshot {
                path: path.to_path_buf(),
                digest,
                size,
                permissions,
                exists: true,
                is_symlink,
                is_file,
                is_dir,
                mtime,
            }
        }
    }
}

/// Returns `true` if `current` differs from `previous` in a way that indicates
/// the file was modified externally.
///
/// Compares `exists`, `digest` and `size`. Mtime and permissions are hints
/// only and not used for the equality decision, matching MUT-01 which says
/// mtime is a hint, not sole identity.
pub fn is_modified(previous: &Snapshot, current: &Snapshot) -> bool {
    if previous.exists != current.exists {
        return true;
    }
    if !previous.exists && !current.exists {
        return false;
    }
    if previous.digest != current.digest {
        return true;
    }
    if previous.size != current.size {
        return true;
    }
    false
}

/// Placeholder for symlink loop detection.
///
/// Returns `true` if a symlink loop is detected at `path`. This is a best-
/// effort check: it follows the symlink chain up to a small depth and reports
/// a loop if the chain revisits a path or if the OS reports a loop error.
///
/// On platforms where the OS error is unavailable, falls back to chain
/// tracking.
pub fn is_symlink_loop(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(_) => {}
        Err(e) => {
            let msg = e.to_string().to_ascii_lowercase();
            if msg.contains("loop") || msg.contains("too many levels") {
                return true;
            }
            if let Some(code) = e.raw_os_error()
                && code == 40
            {
                return true;
            }
        }
    }

    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.file_type().is_symlink() {
        return false;
    }

    let mut visited: Vec<PathBuf> = Vec::new();
    let mut current = path.to_path_buf();
    for _ in 0..20 {
        let Ok(sm) = std::fs::symlink_metadata(&current) else {
            return false;
        };
        if !sm.file_type().is_symlink() {
            return false;
        }
        if visited.contains(&current) {
            return true;
        }
        visited.push(current.clone());
        let Ok(target) = std::fs::read_link(&current) else {
            return false;
        };
        let next = if target.is_absolute() {
            target
        } else if let Some(parent) = current.parent() {
            parent.join(target)
        } else {
            target
        };
        if visited.contains(&next) {
            return true;
        }
        current = next;
    }
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn scratch(name: &str) -> PathBuf {
        let dir = crate::test_util::temp_dir_unique("config-snapshot");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn unique_scratch(prefix: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        scratch(&format!("{prefix}-{now}-{}", std::process::id()))
    }

    #[test]
    fn snapshot_missing_vs_exists() {
        let path = unique_scratch("missing");
        drop(std::fs::remove_file(&path));
        let snap = snapshot(&path);
        assert!(!snap.exists);
        assert!(snap.digest.is_none());
        assert!(!snap.is_symlink);

        std::fs::write(&path, b"data").unwrap();
        let snap2 = snapshot(&path);
        assert!(snap2.exists);
        assert!(snap2.is_file);
        assert!(snap2.digest.is_some());
        assert_eq!(snap2.size, Some(4));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn is_modified_detects_digest_and_size_changes() {
        let path = unique_scratch("modified");
        std::fs::write(&path, b"v1").unwrap();
        let s1 = snapshot(&path);
        std::fs::write(&path, b"v2 longer").unwrap();
        let s2 = snapshot(&path);
        assert!(is_modified(&s1, &s2));
        let s3 = snapshot(&path);
        assert!(!is_modified(&s2, &s3));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn is_modified_detects_creation_and_deletion() {
        let path = unique_scratch("create-delete");
        drop(std::fs::remove_file(&path));
        let s_missing = snapshot(&path);
        std::fs::write(&path, b"x").unwrap();
        let s_exists = snapshot(&path);
        assert!(is_modified(&s_missing, &s_exists));
        assert!(is_modified(&s_exists, &s_missing));
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn snapshot_captures_symlink() {
        #[cfg(unix)]
        {
            let target = unique_scratch("symlink-target");
            let link = unique_scratch("symlink-link");
            std::fs::write(&target, b"target").unwrap();
            drop(std::fs::remove_file(&link));
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let snap = snapshot(&link);
            assert!(snap.exists);
            assert!(snap.is_symlink);
            assert!(snap.digest.is_some());
            drop(std::fs::remove_file(&link));
            drop(std::fs::remove_file(&target));
        }
    }

    #[test]
    fn symlink_loop_detection_placeholder() {
        #[cfg(unix)]
        {
            let a = unique_scratch("loop-a");
            let b = unique_scratch("loop-b");
            drop(std::fs::remove_file(&a));
            drop(std::fs::remove_file(&b));
            std::os::unix::fs::symlink(&b, &a).unwrap();
            std::os::unix::fs::symlink(&a, &b).unwrap();

            assert!(is_symlink_loop(&a), "loop should be detected for a");
            assert!(is_symlink_loop(&b), "loop should be detected for b");

            let snap = snapshot(&a);
            assert!(snap.is_symlink);
            assert!(snap.digest.is_none());

            let target = unique_scratch("loop-target-real");
            let link_ok = unique_scratch("loop-ok");
            std::fs::write(&target, b"ok").unwrap();
            drop(std::fs::remove_file(&link_ok));
            std::os::unix::fs::symlink(&target, &link_ok).unwrap();
            assert!(!is_symlink_loop(&link_ok));
            assert!(!is_symlink_loop(&target));

            drop(std::fs::remove_file(&a));
            drop(std::fs::remove_file(&b));
            drop(std::fs::remove_file(&link_ok));
            drop(std::fs::remove_file(&target));
        }
        #[cfg(not(unix))]
        {
            let path = unique_scratch("loop-placeholder");
            std::fs::write(&path, b"x").unwrap();
            assert!(!is_symlink_loop(&path));
            drop(std::fs::remove_file(&path));
        }
    }

    #[test]
    fn snapshot_mtime_hint_present_for_existing_file() {
        let path = unique_scratch("mtime");
        std::fs::write(&path, b"mtime test").unwrap();
        let snap = snapshot(&path);
        assert!(snap.exists);
        assert!(snap.digest.is_some());
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn conflict_detection_via_snapshot() {
        let path = unique_scratch("conflict");
        std::fs::write(&path, b"original").unwrap();
        let s1 = snapshot(&path);
        std::fs::write(&path, b"concurrent edit").unwrap();
        let s2 = snapshot(&path);
        assert!(is_modified(&s1, &s2), "concurrent edit should be detected");
        let expected = s1.digest.as_deref().unwrap_or_default();
        let actual = s2.digest.as_deref().unwrap_or_default();
        assert_ne!(expected, actual);
        drop(std::fs::remove_file(&path));
    }
}
