use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::atomic::compute_digest;

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

/// Owner identity where the platform exposes it (unix uid/gid).
#[cfg(unix)]
#[expect(clippy::unnecessary_wraps, reason = "Option needed for non-unix None")]
fn get_owner_ids(meta: &std::fs::Metadata) -> Option<(u32, u32)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.uid(), meta.gid()))
}

#[cfg(not(unix))]
fn get_owner_ids(_meta: &std::fs::Metadata) -> Option<(u32, u32)> {
    None
}

/// Inode change time where the platform exposes it; hint only, never the
/// sole conflict identity.
#[cfg(unix)]
#[expect(clippy::unnecessary_wraps, reason = "Option needed for non-unix None")]
fn get_ctime(meta: &std::fs::Metadata) -> Option<SystemTime> {
    use std::os::unix::fs::MetadataExt;
    let secs = u64::try_from(meta.ctime().max(0)).unwrap_or(0);
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

#[cfg(not(unix))]
fn get_ctime(_meta: &std::fs::Metadata) -> Option<SystemTime> {
    None
}

/// Fresh snapshot of a filesystem resource, used as a conflict token.
///
/// Carries no contents, only digests and metadata. No secrets are stored.
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
    /// Owner uid where the platform exposes it (MUT-01).
    pub uid: Option<u32>,
    /// Owner gid where the platform exposes it (MUT-01).
    pub gid: Option<u32>,
    /// Inode change time hint where available (MUT-01; hint only).
    pub ctime: Option<SystemTime>,
    /// Symlink target when the path is a symlink (a retarget is a conflict).
    pub symlink_target: Option<PathBuf>,
    /// Kind inferred from the path; adapters stay authoritative.
    pub kind: Option<crate::document::DocumentKind>,
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

/// Take a fresh snapshot of `path`: disk read, no caching. A symlink loop
/// still reports the link itself with `digest: None`.
pub fn snapshot(path: &Path) -> Snapshot {
    let kind = Some(crate::document::DocumentKind::from_path(path));
    let symlink_meta = std::fs::symlink_metadata(path);
    match symlink_meta {
        Err(_) => Snapshot {
            path: path.to_path_buf(),
            digest: None,
            size: None,
            permissions: None,
            uid: None,
            gid: None,
            ctime: None,
            symlink_target: None,
            kind,
            exists: false,
            is_symlink: false,
            is_file: false,
            is_dir: false,
            mtime: None,
        },
        Ok(meta) => {
            let is_symlink = meta.file_type().is_symlink();
            let is_dir = meta.is_dir();
            let symlink_target = if is_symlink {
                std::fs::read_link(path).ok()
            } else {
                None
            };
            let owner = get_owner_ids(&meta);
            let ctime = get_ctime(&meta);
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
                uid: owner.map(|(u, _)| u),
                gid: owner.map(|(_, g)| g),
                ctime,
                symlink_target,
                kind,
                exists: true,
                is_symlink,
                is_file,
                is_dir,
                mtime,
            }
        }
    }
}

/// Whether `current` differs from `previous` by an external modification:
/// exists, digest, size, or symlink target (a retarget conflicts even with
/// identical bytes). Metadata hints never decide.
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
    if previous.symlink_target != current.symlink_target {
        return true;
    }
    false
}

/// Whether `path` is or resolves through a symlink loop: the OS error when
/// available, otherwise a chain walk capped at 20 hops (a chain past the cap
/// is reported as a loop).
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

// tests

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = crate::test_util::temp_dir_unique("config-snapshot");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn unique_scratch(prefix: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
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

    #[test]
    fn snapshot_records_owner_ctime_and_kind() {
        let dir = crate::test_util::temp_dir_unique("config-snapshot-owner");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, br#"{"a":1}"#).unwrap();
        let snap = snapshot(&path);
        assert_eq!(
            snap.kind,
            Some(crate::document::DocumentKind::StrictJson),
            "kind inference must land in the conflict token"
        );
        // ctime is available on unix (stat) and most windows filesystems;
        // assert only where the snapshot records it unconditionally.
        #[cfg(unix)]
        assert!(snap.ctime.is_some(), "ctime hint where the platform has it");
        #[cfg(unix)]
        {
            assert!(snap.uid.is_some(), "uid must be recorded on unix");
            assert!(snap.gid.is_some(), "gid must be recorded on unix");
            let expected_uid =
                std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&path).unwrap());
            assert_eq!(snap.uid, Some(expected_uid));
        }
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn retargeted_symlink_is_a_modification() {
        #[cfg(unix)]
        {
            let target_a = unique_scratch("retarget-a");
            let target_b = unique_scratch("retarget-b");
            let link = unique_scratch("retarget-link");
            std::fs::write(&target_a, b"same-bytes").unwrap();
            std::fs::write(&target_b, b"same-bytes").unwrap();
            std::os::unix::fs::symlink(&target_a, &link).unwrap();
            let before = snapshot(&link);
            // Identical referent bytes: only the recorded target can see it.
            std::fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink(&target_b, &link).unwrap();
            let after = snapshot(&link);
            assert_eq!(before.digest, after.digest);
            assert!(is_modified(&before, &after), "retarget must be a conflict");
            drop(std::fs::remove_file(&link));
            drop(std::fs::remove_file(&target_a));
            drop(std::fs::remove_file(&target_b));
        }
    }

    /// `is_missing` reports existence: absent paths are missing.
    #[test]
    fn is_missing_reports_existence() {
        let path = unique_scratch("missing-flag");
        drop(std::fs::remove_file(&path));
        let absent = snapshot(&path);
        assert!(absent.is_missing(), "an absent path reports missing");
        std::fs::write(&path, b"present").unwrap();
        let present = snapshot(&path);
        assert!(
            !present.is_missing(),
            "an existing path reports not missing"
        );
        drop(std::fs::remove_file(&path));
    }

    /// Same-size content changes are detected by the digest; identical
    /// bytes compare unmodified.
    #[test]
    fn snapshot_detects_same_size_content_changes() {
        let path = unique_scratch("same-size");
        std::fs::write(&path, b"aaa").unwrap();
        let before = snapshot(&path);
        std::fs::write(&path, b"bbb").unwrap();
        let after = snapshot(&path);
        assert_eq!(before.size, after.size, "fixture guard: sizes are equal");
        assert_ne!(
            before.digest, after.digest,
            "equal-size content changes must change the digest"
        );
        assert!(
            is_modified(&before, &after),
            "a same-size content change is a modification"
        );
        std::fs::write(&path, b"bbb").unwrap();
        let reread = snapshot(&path);
        assert!(
            !is_modified(&after, &reread),
            "identical bytes are not a modification"
        );
        drop(std::fs::remove_file(&path));
    }

    /// The snapshot records the target's permission bits for restores.
    #[cfg(unix)]
    #[test]
    fn snapshot_records_target_permission_bits() {
        use std::os::unix::fs::PermissionsExt;
        let path = unique_scratch("perm-bits");
        std::fs::write(&path, b"perms").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let snap = snapshot(&path);
        let mode = snap.permissions.expect("permissions recorded on unix");
        assert_eq!(
            mode & 0o777,
            0o640,
            "the snapshot records the file's permission bits"
        );
        drop(std::fs::remove_file(&path));
    }

    /// The ctime hint is a plausible recent instant, not a constant.
    #[cfg(unix)]
    #[test]
    fn snapshot_ctime_is_a_plausible_recent_instant() {
        /// 2020-01-01T00:00:00Z, and one day, in seconds.
        const SECS_AT_2020: u64 = 1_577_836_800;
        const DAY_SECS: u64 = 24 * 60 * 60;
        let path = unique_scratch("ctime-window");
        std::fs::write(&path, b"ctime").unwrap();
        let snap = snapshot(&path);
        let ctime = snap.ctime.expect("ctime recorded on unix");
        let epoch_2020 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(SECS_AT_2020);
        assert!(
            ctime > epoch_2020,
            "ctime must be after 2020, got {ctime:?}"
        );
        let horizon = SystemTime::now() + std::time::Duration::from_secs(DAY_SECS);
        assert!(
            ctime < horizon,
            "ctime must not be in the far future, got {ctime:?}"
        );
        drop(std::fs::remove_file(&path));
    }

    /// A missing path is not a symlink loop: absence is not a cycle.
    #[test]
    fn is_symlink_loop_is_false_for_missing_path() {
        let path = unique_scratch("loop-missing");
        drop(std::fs::remove_file(&path));
        assert!(!is_symlink_loop(&path), "a missing path is not a loop");
    }

    /// A resolvable chain deeper than the 20-hop walk cutoff (but under the
    /// kernel limits of Linux 40 and macOS 32) is reported as a loop by the
    /// capped walk; the errno fast-path cannot see it.
    #[cfg(unix)]
    #[test]
    fn deep_symlink_chain_beyond_walk_cutoff_is_reported_as_a_loop() {
        /// Over the walk cutoff (20), under the kernel limits (40/32).
        const DEPTH: usize = 25;
        let root = crate::test_util::temp_dir_unique("config-snapshot-deep");
        std::fs::create_dir_all(&root).unwrap();
        let real = root.join("real.txt");
        std::fs::write(&real, b"deep").unwrap();
        // Relative targets so each hop costs one OS-level traversal: absolute
        // targets under the macOS temp root re-traverse /var per hop and
        // would cross MAXSYMLINKS before the 25 hops are done.
        let mut next_target = std::ffi::OsString::from("real.txt");
        for i in (0..DEPTH).rev() {
            let link_name = format!("link-{i}");
            std::os::unix::fs::symlink(&next_target, root.join(&link_name)).unwrap();
            next_target = std::ffi::OsString::from(link_name);
        }
        let head = root.join(next_target);
        assert!(
            std::fs::metadata(&head).is_ok(),
            "fixture guard: the chain must resolve at the OS level (no ELOOP)"
        );
        assert!(
            is_symlink_loop(&head),
            "a chain deeper than the walk cutoff must be reported as a loop"
        );
        drop(std::fs::remove_dir_all(&root));
    }
}
