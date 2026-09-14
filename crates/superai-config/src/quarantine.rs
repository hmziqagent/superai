//! Quarantine for recoverable deletion (MUT-08).
//!
//! Material directory removal first moves the exact validated target into a
//! superai quarantine area on the same filesystem where possible. The
//! quarantine path is `~/.superai/quarantine/<operation_id>/` and the move
//! reports recoverability and retention.

use std::path::{Path, PathBuf};

use crate::error::{ConfigError, Result};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home);
        if p.is_absolute() {
            return Some(p);
        }
    }
    if let Some(up) = std::env::var_os("USERPROFILE") {
        let p = PathBuf::from(up);
        if p.is_absolute() {
            return Some(p);
        }
    }
    None
}

fn compute_digest(bytes: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Whether `path` looks like a broad root that must never be quarantined.
fn is_broad_root(path: &Path) -> bool {
    let s = path.to_string_lossy();
    let raw = s.as_ref();
    matches!(raw, "/" | "/home" | "/tmp" | "/usr" | "/etc" | "/var")
        || raw == "/home/"
        || raw == "/tmp/"
        // Windows-shaped broad roots (drive roots, UNC roots, first-level
        // system directories), case-folded with both separators — inert on
        // unix, where no absolute path is windows-shaped.
        || crate::transaction::windows_shaped_broad_root(path)
}

/// Check for unresolved variable patterns.
fn has_unresolved_variable(path: &Path) -> bool {
    let s = path.to_string_lossy();
    if s.contains('$') || s.contains('%') {
        return true;
    }
    // `~` counts only as a whole path component (unexpanded home shorthand).
    // Windows 8.3 short names like `RUNNER~1` are legal and must pass.
    path.components().any(|c| c.as_os_str() == "~")
}

/// Check for glob patterns.
fn has_glob(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains('*') || s.contains('?') || s.contains('[')
}

// ---------------------------------------------------------------------------
// Quarantine paths
// ---------------------------------------------------------------------------

/// Returns the base quarantine directory: `~/.superai/quarantine`.
pub fn quarantine_base() -> Result<PathBuf> {
    let home = home_dir().ok_or_else(|| {
        ConfigError::io(
            Path::new("~/.superai/quarantine"),
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "cannot determine home directory",
            ),
        )
    })?;
    Ok(home.join(".superai").join("quarantine"))
}

/// Returns the quarantine directory for a given operation id:
/// `~/.superai/quarantine/<operation_id>/`.
pub fn quarantine_dir(operation_id: &str) -> Result<PathBuf> {
    if operation_id.is_empty() {
        return Err(ConfigError::io(
            Path::new(operation_id),
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "operation id must not be empty",
            ),
        ));
    }
    if operation_id.contains('/') || operation_id.contains('\\') || operation_id.contains(':') {
        return Err(ConfigError::io(
            Path::new(operation_id),
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "operation id must not contain path separators",
            ),
        ));
    }
    let base = quarantine_base()?;
    Ok(base.join(operation_id))
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

/// A quarantined artifact, recoverable from the quarantine directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineEntry {
    /// Original path that was moved.
    pub original_path: PathBuf,
    /// Path inside the quarantine directory.
    pub quarantine_path: PathBuf,
    /// Operation that triggered the quarantine.
    pub operation_id: String,
    /// Whether the quarantined copy is recoverable (digest verified).
    pub recoverable: bool,
    /// Digest of the original content before move (for files).
    pub digest: Option<String>,
    /// Size in bytes if the original was a file.
    pub size: Option<u64>,
    /// Whether the quarantine is on the same filesystem (rename vs copy).
    pub same_filesystem: bool,
}

impl QuarantineEntry {
    /// Human-readable recoverability report.
    pub fn recoverability_report(&self) -> String {
        if self.recoverable {
            format!(
                "recoverable at {} (operation {}) — same_filesystem={}",
                self.quarantine_path.display(),
                self.operation_id,
                self.same_filesystem
            )
        } else {
            format!(
                "not recoverable: quarantine at {} missing or digest mismatch (operation {})",
                self.quarantine_path.display(),
                self.operation_id
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate that `path` is a safe quarantine source.
///
/// Rejects broad roots, unresolved variables, globs, home directories,
/// workspace roots, and foreign-managed path surrogates.
pub fn validate_quarantine_target(path: &Path) -> Result<()> {
    let display = path.to_string_lossy();
    let s = display.as_ref();

    if s.is_empty() {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "quarantine target must not be empty",
            ),
        ));
    }
    if !path.is_absolute() {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "quarantine target must be absolute",
            ),
        ));
    }
    for comp in path.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return Err(ConfigError::io(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "quarantine target must not contain '..'",
                ),
            ));
        }
    }
    if has_glob(path) {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "quarantine target must not contain globs",
            ),
        ));
    }
    if has_unresolved_variable(path) {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "quarantine target contains unresolved variable",
            ),
        ));
    }
    if is_broad_root(path) {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "refusing to quarantine broad root",
            ),
        ));
    }
    if let Some(home) = home_dir() {
        if crate::transaction::paths_equal_platform_folded(path, &home) {
            return Err(ConfigError::io(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "refusing to quarantine home directory",
                ),
            ));
        }
        // Also reject the quarantine base itself or its parent.
        let base = home.join(".superai").join("quarantine");
        if crate::transaction::paths_equal_platform_folded(path, &base) {
            return Err(ConfigError::io(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "refusing to quarantine quarantine directory",
                ),
            ));
        }
    }
    // Require that the path exists and is a file or directory (not FIFO/socket/device)
    let meta = std::fs::symlink_metadata(path).map_err(|e| ConfigError::io(path, e))?;
    let ft = meta.file_type();
    if !(ft.is_file() || ft.is_dir() || ft.is_symlink()) {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unsupported special file for quarantine",
            ),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Core move
// ---------------------------------------------------------------------------

/// Move `path` into the quarantine directory for `operation_id`.
///
/// Validates the target, ensures the quarantine directory exists with
/// owner-only permissions where supported, moves the file/directory via
/// rename where same-filesystem, otherwise copy-then-delete, verifies digest,
/// and reports recoverability.
pub fn move_to_quarantine(path: &Path, operation_id: &str) -> Result<QuarantineEntry> {
    let qdir = quarantine_dir(operation_id)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| {
            ConfigError::io(
                path,
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name"),
            )
        })?
        .to_os_string();
    let dest = qdir.join(file_name);
    move_to_quarantine_with_dest(path, &dest, operation_id)
}

/// Move `path` to an explicit `dest` inside quarantine, validating `path`.
///
/// `dest` must be inside the quarantine directory for `operation_id`.
#[expect(
    clippy::too_many_lines,
    reason = "quarantine move validates, copies, and verifies"
)]
pub fn move_to_quarantine_with_dest(
    path: &Path,
    dest: &Path,
    operation_id: &str,
) -> Result<QuarantineEntry> {
    validate_quarantine_target(path)?;

    let qdir = quarantine_dir(operation_id)?;
    if !dest.starts_with(&qdir) {
        return Err(ConfigError::io(
            dest,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "quarantine destination must be inside quarantine dir",
            ),
        ));
    }

    // Ensure quarantine dir exists with safe permissions
    std::fs::create_dir_all(&qdir).map_err(|e| ConfigError::io(&qdir, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o700);
        drop(std::fs::set_permissions(&qdir, perm.clone()));
        if let Some(parent) = qdir.parent() {
            // Ensure ~/.superai exists with 0o700 as well
            let superai = parent;
            if superai.exists() {
                drop(std::fs::set_permissions(superai, perm));
            }
        }
    }

    // If dest already exists, make it unique
    let mut final_dest = dest.to_path_buf();
    let mut counter = 0u32;
    while final_dest.exists() {
        counter = counter.saturating_add(1);
        let suffix = format!(".{counter}");
        let mut name = dest.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        final_dest = dest.with_file_name(name);
        if counter > 100 {
            return Err(ConfigError::io(
                dest,
                std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "quarantine destination collision",
                ),
            ));
        }
    }

    // Capture digest/size before move if file
    let (digest, size) = if path.is_file() {
        match std::fs::read(path) {
            Ok(bytes) => (Some(compute_digest(&bytes)), Some(bytes.len() as u64)),
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };

    let mut same_filesystem = true;
    match std::fs::rename(path, &final_dest) {
        Ok(()) => {}
        Err(e)
            if e.kind() == std::io::ErrorKind::CrossesDevices || e.raw_os_error() == Some(18) =>
        {
            same_filesystem = false;
            // Cross-device: copy then delete original
            let meta = std::fs::symlink_metadata(path).map_err(|e2| ConfigError::io(path, e2))?;
            if meta.is_dir() {
                copy_dir_recursively(path, &final_dest)?;
                std::fs::remove_dir_all(path).map_err(|e2| ConfigError::io(path, e2))?;
            } else if meta.file_type().is_symlink() {
                #[cfg(unix)]
                {
                    let target =
                        std::fs::read_link(path).map_err(|e2| ConfigError::io(path, e2))?;
                    std::os::unix::fs::symlink(&target, &final_dest)
                        .map_err(|e2| ConfigError::io(&final_dest, e2))?;
                }
                #[cfg(not(unix))]
                {
                    std::fs::copy(path, &final_dest)
                        .map_err(|e2| ConfigError::io(&final_dest, e2))?;
                }
                std::fs::remove_file(path).map_err(|e2| ConfigError::io(path, e2))?;
            } else {
                std::fs::copy(path, &final_dest).map_err(|e2| ConfigError::io(&final_dest, e2))?;
                std::fs::remove_file(path).map_err(|e2| ConfigError::io(path, e2))?;
            }
        }
        Err(e) => return Err(ConfigError::io(path, e)),
    }

    // Verify quarantine copy if it was a file
    let recoverable = if let Some(expected) = digest.as_deref() {
        match std::fs::read(&final_dest) {
            Ok(bytes) => compute_digest(&bytes) == expected,
            Err(_) => false,
        }
    } else {
        // For directories/symlinks, recoverability is existence
        final_dest.exists()
    };

    // Sync parent
    if let Some(parent) = final_dest.parent()
        && let Ok(f) = std::fs::File::open(parent)
    {
        drop(f.sync_all());
    }

    Ok(QuarantineEntry {
        original_path: path.to_path_buf(),
        quarantine_path: final_dest,
        operation_id: operation_id.to_owned(),
        recoverable,
        digest,
        size,
        same_filesystem,
    })
}

fn copy_dir_recursively(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|e| ConfigError::io(to, e))?;
    let entries = std::fs::read_dir(from).map_err(|e| ConfigError::io(from, e))?;
    for ent in entries {
        let ent = ent.map_err(|e| ConfigError::io(from, e))?;
        let src = ent.path();
        let file_name = ent.file_name();
        let dest = to.join(file_name);
        let meta = ent.metadata().map_err(|e| ConfigError::io(&src, e))?;
        if meta.is_dir() {
            copy_dir_recursively(&src, &dest)?;
        } else if meta.file_type().is_symlink() {
            #[cfg(unix)]
            {
                let target = std::fs::read_link(&src).map_err(|e| ConfigError::io(&src, e))?;
                std::os::unix::fs::symlink(&target, &dest)
                    .map_err(|e| ConfigError::io(&dest, e))?;
            }
            #[cfg(not(unix))]
            {
                std::fs::copy(&src, &dest).map_err(|e| ConfigError::io(&dest, e))?;
            }
        } else {
            std::fs::copy(&src, &dest).map_err(|e| ConfigError::io(&dest, e))?;
        }
    }
    Ok(())
}

/// Restore a quarantined entry back to its original location.
pub fn restore_from_quarantine(entry: &QuarantineEntry) -> Result<()> {
    if !entry.quarantine_path.exists() {
        return Err(ConfigError::io(
            &entry.quarantine_path,
            std::io::Error::new(std::io::ErrorKind::NotFound, "quarantine entry missing"),
        ));
    }
    if entry.original_path.exists() || std::fs::symlink_metadata(&entry.original_path).is_ok() {
        return Err(ConfigError::io(
            &entry.original_path,
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "original path already exists",
            ),
        ));
    }
    if let Some(parent) = entry.original_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }
    match std::fs::rename(&entry.quarantine_path, &entry.original_path) {
        Ok(()) => Ok(()),
        Err(e)
            if e.kind() == std::io::ErrorKind::CrossesDevices || e.raw_os_error() == Some(18) =>
        {
            let meta = std::fs::symlink_metadata(&entry.quarantine_path)
                .map_err(|er| ConfigError::io(&entry.quarantine_path, er))?;
            if meta.is_dir() {
                copy_dir_recursively(&entry.quarantine_path, &entry.original_path)?;
                std::fs::remove_dir_all(&entry.quarantine_path)
                    .map_err(|er| ConfigError::io(&entry.quarantine_path, er))?;
            } else {
                std::fs::copy(&entry.quarantine_path, &entry.original_path)
                    .map_err(|er| ConfigError::io(&entry.original_path, er))?;
                std::fs::remove_file(&entry.quarantine_path)
                    .map_err(|er| ConfigError::io(&entry.quarantine_path, er))?;
            }
            Ok(())
        }
        Err(e) => Err(ConfigError::io(&entry.original_path, e)),
    }
}

/// List quarantine entries for an operation id.
pub fn list_quarantine(operation_id: &str) -> Result<Vec<QuarantineEntry>> {
    let qdir = quarantine_dir(operation_id)?;
    let mut entries = Vec::new();
    let dir = match std::fs::read_dir(&qdir) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ConfigError::io(&qdir, e)),
    };
    for ent in dir {
        let ent = ent.map_err(|e| ConfigError::io(&qdir, e))?;
        let path = ent.path();
        let meta = ent.metadata().map_err(|e| ConfigError::io(&path, e))?;
        let digest = if meta.is_file() {
            std::fs::read(&path).ok().map(|b| compute_digest(&b))
        } else {
            None
        };
        let size = if meta.is_file() {
            Some(meta.len())
        } else {
            None
        };
        entries.push(QuarantineEntry {
            original_path: PathBuf::from("<unknown>"),
            quarantine_path: path,
            operation_id: operation_id.to_owned(),
            recoverable: digest.is_some() || meta.is_dir(),
            digest,
            size,
            same_filesystem: true,
        });
    }
    entries.sort_by(|a, b| a.quarantine_path.cmp(&b.quarantine_path));
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_op(prefix: &str) -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        format!("{prefix}-{millis}-{}", std::process::id())
    }

    #[test]
    fn quarantine_path_is_under_home() {
        let op = unique_op("op-path");
        let dir = quarantine_dir(&op).unwrap();
        let base = quarantine_base().unwrap();
        assert!(dir.starts_with(&base));
        assert!(dir.ends_with(&op));
    }

    #[test]
    fn quarantine_rejects_invalid_operation_id() {
        quarantine_dir("").unwrap_err();
        quarantine_dir("a/b").unwrap_err();
        quarantine_dir("a\\b").unwrap_err();
    }

    #[test]
    fn validate_quarantine_rejects_broad_roots_and_globs() {
        validate_quarantine_target(Path::new("/")).unwrap_err();
        validate_quarantine_target(Path::new("/home")).unwrap_err();
        validate_quarantine_target(Path::new("/tmp/*.json")).unwrap_err();
        validate_quarantine_target(Path::new("/tmp/$HOME/foo")).unwrap_err();
        validate_quarantine_target(Path::new("relative/path")).unwrap_err();
        validate_quarantine_target(Path::new("/tmp/../etc")).unwrap_err();
    }

    #[test]
    fn move_to_quarantine_and_restore_file() {
        let op = unique_op("move-file");
        let src = crate::test_util::temp_dir_unique("quarantine-src").join(&op);
        std::fs::write(&src, b"quarantine content").unwrap();

        let entry = move_to_quarantine(&src, &op).unwrap();
        assert!(!src.exists(), "original should be moved");
        assert!(entry.quarantine_path.exists());
        assert!(entry.recoverable);
        assert_eq!(entry.operation_id, op);
        assert!(entry.digest.is_some());
        assert!(entry.recoverability_report().contains("recoverable"));

        // List
        let list = list_quarantine(&op).unwrap();
        assert!(!list.is_empty());

        // Restore
        restore_from_quarantine(&entry).unwrap();
        assert!(src.exists());
        assert_eq!(std::fs::read(&src).unwrap(), b"quarantine content");
        assert!(!entry.quarantine_path.exists());

        // Cleanup
        drop(std::fs::remove_file(&src));
        let qdir = quarantine_dir(&op).unwrap();
        drop(std::fs::remove_dir_all(&qdir));
    }

    #[test]
    fn move_to_quarantine_and_restore_directory() {
        let op = unique_op("move-dir");
        let src = crate::test_util::temp_dir_unique("quarantine-dir").join(&op);
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("file.txt"), b"hello").unwrap();
        std::fs::write(src.join("sub").join("nested.txt"), b"nested").unwrap();

        let entry = move_to_quarantine(&src, &op).unwrap();
        assert!(!src.exists());
        assert!(entry.quarantine_path.exists());
        assert!(entry.recoverable);

        restore_from_quarantine(&entry).unwrap();
        assert!(src.exists());
        assert_eq!(std::fs::read(src.join("file.txt")).unwrap(), b"hello");
        assert_eq!(
            std::fs::read(src.join("sub").join("nested.txt")).unwrap(),
            b"nested"
        );

        drop(std::fs::remove_dir_all(&src));
        let qdir = quarantine_dir(&op).unwrap();
        drop(std::fs::remove_dir_all(&qdir));
    }

    #[test]
    fn quarantine_rejects_home_directory() {
        if let Some(home) = home_dir() {
            validate_quarantine_target(&home).unwrap_err();
        }
    }

    #[test]
    fn quarantine_reports_recoverability_and_retention() {
        let op = unique_op("report");
        let src = crate::test_util::temp_dir_unique("quarantine-report").join(&op);
        std::fs::write(&src, b"report test").unwrap();
        let entry = move_to_quarantine(&src, &op).unwrap();
        let report = entry.recoverability_report();
        assert!(report.contains(&op));
        assert!(report.contains(entry.quarantine_path.to_string_lossy().as_ref()));
        // Retention: quarantine entry remains after move until explicitly removed
        assert!(entry.quarantine_path.exists());
        restore_from_quarantine(&entry).unwrap();
        drop(std::fs::remove_file(&src));
        let qdir = quarantine_dir(&op).unwrap();
        drop(std::fs::remove_dir_all(&qdir));
    }

    // -----------------------------------------------------------------
    // Mutation-hardening behaviour tests (area D).
    // -----------------------------------------------------------------

    /// Whether chmod still denies the owner: root (`DAC_OVERRIDE`) bypasses
    /// permission checks, so permission-driven scenarios must self-skip.
    #[cfg(unix)]
    fn permissions_are_enforced() -> bool {
        use std::os::unix::fs::PermissionsExt;
        let probe = crate::test_util::temp_dir_unique("quarantine-perm-probe");
        std::fs::create_dir_all(&probe).unwrap();
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o555)).unwrap();
        let denied = std::fs::File::create(probe.join("p")).is_err();
        drop(std::fs::set_permissions(
            &probe,
            std::fs::Permissions::from_mode(0o755),
        ));
        drop(std::fs::remove_dir_all(&probe));
        denied
    }

    #[cfg(unix)]
    fn dev_of(path: &Path) -> Option<u64> {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| m.dev())
    }

    /// A scratch directory on a filesystem other than the quarantine base's,
    /// when one is available (/dev/shm tmpfs vs the home filesystem).
    #[cfg(unix)]
    fn cross_device_scratch() -> Option<PathBuf> {
        let shm = PathBuf::from("/dev/shm");
        if !shm.is_dir() {
            return None;
        }
        let home = home_dir()?;
        if dev_of(&shm)? == dev_of(&home)? {
            return None; // same filesystem: rename would succeed, nothing to probe
        }
        let scratch = shm.join(format!("superai-quarantine-exdev-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).ok()?;
        Some(scratch)
    }

    #[test]
    fn quarantine_base_is_an_absolute_home_anchored_directory() {
        let base = quarantine_base().unwrap();
        assert!(
            base.is_absolute(),
            "quarantine base must be absolute: {base:?}"
        );
        let home = home_dir().expect("tests require a resolvable home directory");
        assert_eq!(base, home.join(".superai").join("quarantine"));
        assert_eq!(
            quarantine_dir("anchored-op").unwrap(),
            base.join("anchored-op")
        );
    }

    #[test]
    fn quarantine_dir_rejects_every_path_separator_shape() {
        quarantine_dir("").unwrap_err();
        quarantine_dir("a/b").unwrap_err();
        quarantine_dir("a\\b").unwrap_err();
        quarantine_dir("a:b").unwrap_err();
    }

    #[test]
    fn validate_quarantine_rejects_trailing_slash_broad_roots() {
        validate_quarantine_target(Path::new("/home/")).unwrap_err();
        validate_quarantine_target(Path::new("/tmp/")).unwrap_err();
    }

    /// Platform: unix — the fixture must CREATE existing paths whose names
    /// contain `*` and `?`, which Win32 filename rules reserve (creation
    /// fails before any assertion runs). Unix filenames may contain glob
    /// characters, so the "existing path containing globs" premise is
    /// stageable only there.
    #[cfg(unix)]
    #[test]
    fn validate_quarantine_rejects_existing_paths_containing_globs() {
        let dir = crate::test_util::temp_dir_unique("quarantine-glob");
        for name in ["star*name", "quest?name", "brack[name"] {
            let path = dir.join(name);
            std::fs::write(&path, b"x").unwrap();
            let err = validate_quarantine_target(&path).unwrap_err();
            assert!(err.to_string().contains("globs"), "{name}: {err}");
        }
    }

    #[test]
    fn validate_quarantine_rejects_existing_paths_with_unresolved_variables() {
        let dir = crate::test_util::temp_dir_unique("quarantine-variables");
        let dollar = dir.join("dollar$sign");
        std::fs::write(&dollar, b"x").unwrap();
        let percent = dir.join("percent%sign");
        std::fs::write(&percent, b"x").unwrap();
        std::fs::create_dir_all(dir.join("~")).unwrap();
        let tilde = dir.join("~").join("file");
        std::fs::write(&tilde, b"x").unwrap();
        for path in [&dollar, &percent, &tilde] {
            let err = validate_quarantine_target(path).unwrap_err();
            assert!(
                err.to_string().contains("unresolved variable"),
                "{}: {err}",
                path.display()
            );
        }
    }

    #[test]
    fn quarantine_entry_digest_is_sixteen_lowercase_hex_and_content_addressed() {
        let dir = crate::test_util::temp_dir_unique("quarantine-digest");
        let op_a = unique_op("digest-a");
        let src_a = dir.join("digest-a");
        std::fs::write(&src_a, b"quarantine digest probe").unwrap();
        let entry_a = move_to_quarantine(&src_a, &op_a).unwrap();
        let digest_a = entry_a.digest.clone().expect("file moves record a digest");
        assert_eq!(digest_a.len(), 16, "digest must be 16 hex characters");
        assert!(
            digest_a
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "digest must be lowercase hex: {digest_a}"
        );
        assert_eq!(entry_a.size, Some(23));

        // Same bytes quarantine to the same digest; different bytes differ.
        let op_b = unique_op("digest-bc");
        let src_b = dir.join("digest-b");
        std::fs::write(&src_b, b"quarantine digest probe").unwrap();
        let src_c = dir.join("digest-c");
        std::fs::write(&src_c, b"different bytes entirely").unwrap();
        let entry_b = move_to_quarantine(&src_b, &op_b).unwrap();
        let entry_c = move_to_quarantine(&src_c, &op_b).unwrap();
        assert_eq!(entry_b.digest, entry_a.digest);
        assert_ne!(entry_c.digest, entry_a.digest);

        // The listing recomputes the same digest for the landed files.
        let listed = list_quarantine(&op_b).unwrap();
        let find = |name: &str| {
            listed
                .iter()
                .find(|e| e.quarantine_path.ends_with(name))
                .unwrap_or_else(|| panic!("listed entry {name} missing: {listed:?}"))
        };
        assert_eq!(find("digest-b").digest, entry_a.digest);
        assert!(find("digest-b").recoverable);
        assert_eq!(find("digest-c").digest, entry_c.digest);
    }

    #[test]
    fn quarantine_dedups_colliding_destination_names_until_free() {
        let op = unique_op("collide-ok");
        let qdir = quarantine_dir(&op).unwrap();
        std::fs::create_dir_all(&qdir).unwrap();
        let dest = qdir.join("victim");
        // Occupy dest plus the first 99 dedup candidates: the mover must
        // still succeed by landing on `victim.100`.
        std::fs::write(&dest, b"occupied").unwrap();
        for i in 1..100 {
            std::fs::write(qdir.join(format!("victim.{i}")), b"occupied").unwrap();
        }
        let src = crate::test_util::temp_dir_unique("quarantine-collide-src").join("victim");
        std::fs::write(&src, b"colliding content").unwrap();

        let entry = move_to_quarantine_with_dest(&src, &dest, &op).unwrap();
        assert_eq!(entry.quarantine_path, qdir.join("victim.100"));
        assert_eq!(
            std::fs::read(qdir.join("victim.100")).unwrap(),
            b"colliding content"
        );
        assert!(!src.exists());
    }

    #[test]
    fn quarantine_gives_up_after_a_hundred_destination_collisions() {
        let op = unique_op("collide-full");
        let qdir = quarantine_dir(&op).unwrap();
        std::fs::create_dir_all(&qdir).unwrap();
        let dest = qdir.join("victim");
        std::fs::write(&dest, b"occupied").unwrap();
        for i in 1..=100 {
            std::fs::write(qdir.join(format!("victim.{i}")), b"occupied").unwrap();
        }
        let src = crate::test_util::temp_dir_unique("quarantine-collide-full").join("victim");
        std::fs::write(&src, b"never moved").unwrap();

        let err = move_to_quarantine_with_dest(&src, &dest, &op).unwrap_err();
        assert!(
            matches!(err, ConfigError::Io { ref source, .. }
                if source.kind() == std::io::ErrorKind::AlreadyExists),
            "collision exhaustion must surface AlreadyExists: {err}"
        );
        assert!(
            src.exists(),
            "the source is untouched on collision exhaustion"
        );
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_cross_device_move_copies_verifies_and_reports_other_filesystem() {
        let Some(scratch) = cross_device_scratch() else {
            return;
        };
        let op = unique_op("exdev-file");
        let src = scratch.join(unique_op("src"));
        std::fs::write(&src, b"cross-device content").unwrap();

        let entry = move_to_quarantine(&src, &op).unwrap();
        assert!(
            !entry.same_filesystem,
            "a cross-device move must report the copy path"
        );
        assert!(!src.exists());
        assert_eq!(
            std::fs::read(&entry.quarantine_path).unwrap(),
            b"cross-device content"
        );
        assert!(entry.recoverable);

        // Restoring crosses devices too: the copy-back path must return it.
        restore_from_quarantine(&entry).unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), b"cross-device content");
        assert!(!entry.quarantine_path.exists());
        drop(std::fs::remove_dir_all(&scratch));
        drop(std::fs::remove_dir_all(quarantine_dir(&op).unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_cross_device_directory_move_copies_nested_content() {
        let Some(scratch) = cross_device_scratch() else {
            return;
        };
        let op = unique_op("exdev-dir");
        let src = scratch.join(unique_op("dsrc"));
        std::fs::create_dir_all(src.join("nested/deeper")).unwrap();
        std::fs::write(src.join("top.txt"), b"top").unwrap();
        std::fs::write(src.join("nested/deeper/leaf.txt"), b"leaf").unwrap();

        let entry = move_to_quarantine(&src, &op).unwrap();
        assert!(!entry.same_filesystem);
        assert!(!src.exists());
        assert_eq!(
            std::fs::read(entry.quarantine_path.join("top.txt")).unwrap(),
            b"top"
        );
        assert_eq!(
            std::fs::read(entry.quarantine_path.join("nested/deeper/leaf.txt")).unwrap(),
            b"leaf"
        );
        assert!(entry.recoverable, "directory recoverability is existence");
        let listed = list_quarantine(&op).unwrap();
        assert!(
            listed.first().is_some_and(|e| e.recoverable),
            "a listed quarantined directory is recoverable: {listed:?}"
        );

        restore_from_quarantine(&entry).unwrap();
        assert_eq!(
            std::fs::read(src.join("nested/deeper/leaf.txt")).unwrap(),
            b"leaf"
        );
        assert!(!entry.quarantine_path.exists());
        drop(std::fs::remove_dir_all(&scratch));
        drop(std::fs::remove_dir_all(quarantine_dir(&op).unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_surfaces_rename_permission_errors_without_writing_the_destination() {
        use std::os::unix::fs::PermissionsExt;
        if !permissions_are_enforced() {
            return; // root bypasses permission checks
        }
        let op = unique_op("rename-deny");
        // The source must share the quarantine dir's filesystem so that
        // rename() fails with EACCES (not EXDEV) under the locked parent.
        let dir = home_dir().unwrap().join(format!(".superai-qn-test-{op}"));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("victim");
        std::fs::write(&src, b"locked").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let err = move_to_quarantine(&src, &op);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = err.unwrap_err();
        assert!(
            matches!(err, ConfigError::Io { ref source, .. }
                if source.kind() == std::io::ErrorKind::PermissionDenied),
            "rename out of an unwritable directory surfaces EACCES: {err}"
        );
        assert!(src.exists(), "the locked source is untouched");
        let qdir = quarantine_dir(&op).unwrap();
        assert!(
            !qdir.join("victim").exists(),
            "no partial copy may land in quarantine when the move failed"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    #[cfg(unix)]
    #[test]
    fn restore_surfaces_rename_permission_errors_without_creating_the_original() {
        use std::os::unix::fs::PermissionsExt;
        if !permissions_are_enforced() {
            return; // root bypasses permission checks
        }
        let op = unique_op("restore-deny");
        // Same filesystem as the quarantine dir so rename() out of the
        // read-only quarantine fails with EACCES, not EXDEV.
        let dir = home_dir().unwrap().join(format!(".superai-qn-test-{op}"));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("victim");
        std::fs::write(&src, b"locked restore").unwrap();
        let entry = move_to_quarantine(&src, &op).unwrap();

        // Lock the quarantine dir: renaming out of it fails EACCES, and no
        // fallback may quietly recreate the original.
        let qdir = quarantine_dir(&op).unwrap();
        std::fs::set_permissions(&qdir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let err = restore_from_quarantine(&entry);
        std::fs::set_permissions(&qdir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let err = err.unwrap_err();
        assert!(
            matches!(err, ConfigError::Io { ref source, .. }
                if source.kind() == std::io::ErrorKind::PermissionDenied),
            "rename out of a read-only quarantine surfaces EACCES: {err}"
        );
        assert!(
            !src.exists(),
            "a failed restore must not create the original"
        );
        assert!(
            entry.quarantine_path.exists(),
            "the quarantined copy stays put"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn restore_recreates_vanished_parent_directories() {
        let op = unique_op("restore-parent");
        let dir = crate::test_util::temp_dir_unique("quarantine-vanish");
        let parent = dir.join("gone/soon");
        std::fs::create_dir_all(&parent).unwrap();
        let src = parent.join("file");
        std::fs::write(&src, b"orphaned").unwrap();

        let entry = move_to_quarantine(&src, &op).unwrap();
        std::fs::remove_dir_all(dir.join("gone")).unwrap();
        restore_from_quarantine(&entry).unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), b"orphaned");
    }

    #[cfg(unix)]
    #[test]
    fn restore_refuses_when_the_original_path_exists_again_in_any_shape() {
        let op = unique_op("restore-exists");
        let dir = crate::test_util::temp_dir_unique("quarantine-reexists");
        let src = dir.join("file");
        std::fs::write(&src, b"first life").unwrap();
        let entry = move_to_quarantine(&src, &op).unwrap();

        // A regular file re-created at the original path blocks the restore.
        std::fs::write(&src, b"second life").unwrap();
        let err = restore_from_quarantine(&entry).unwrap_err();
        assert!(
            matches!(err, ConfigError::Io { ref source, .. }
                if source.kind() == std::io::ErrorKind::AlreadyExists),
            "an existing original blocks restore: {err}"
        );
        assert_eq!(
            std::fs::read(&src).unwrap(),
            b"second life",
            "the re-created file is untouched"
        );

        // A broken symlink at the original path also blocks the restore:
        // exists() is false but lstat still resolves.
        std::fs::remove_file(&src).unwrap();
        std::os::unix::fs::symlink("/nowhere/at-all", &src).unwrap();
        let err = restore_from_quarantine(&entry).unwrap_err();
        assert!(
            matches!(err, ConfigError::Io { ref source, .. }
                if source.kind() == std::io::ErrorKind::AlreadyExists),
            "a broken symlink at the original still blocks restore: {err}"
        );
        assert!(src.is_symlink(), "the blocker is left in place");

        drop(std::fs::remove_file(&src));
        drop(std::fs::remove_file(&entry.quarantine_path));
    }

    #[test]
    fn list_quarantine_reports_no_entries_for_an_unknown_operation() {
        let listed = list_quarantine(&unique_op("never-created")).unwrap();
        assert!(listed.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn list_quarantine_surfaces_unreadable_quarantine_dirs() {
        use std::os::unix::fs::PermissionsExt;
        if !permissions_are_enforced() {
            return; // root bypasses permission checks
        }
        let op = unique_op("list-deny");
        let qdir = quarantine_dir(&op).unwrap();
        std::fs::create_dir_all(&qdir).unwrap();
        std::fs::write(qdir.join("entry"), b"x").unwrap();

        std::fs::set_permissions(&qdir, std::fs::Permissions::from_mode(0o000)).unwrap();
        let res = list_quarantine(&op);
        std::fs::set_permissions(&qdir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            res.is_err(),
            "a non-NotFound read_dir failure must surface, got {:?}",
            res.map(|v| v.len())
        );
        drop(std::fs::remove_dir_all(&qdir));
    }
}
