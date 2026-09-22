use std::path::{Path, PathBuf};

use crate::atomic::{
    WriteExpectation, atomic_write_expecting, compute_digest, generate_random_suffix,
    timestamp_millis_now,
};
use crate::document::validate_bytes_for_kind;
use crate::error::{ConfigError, Result};
use crate::injector::{Injector, Point, run as inject};

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "Option needed for cross-platform None"
)]
fn get_permissions_u32(meta: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode())
}

#[cfg(not(unix))]
fn get_permissions_u32(_meta: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn set_permissions_u32(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perm = std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(path, perm).map_err(|e| ConfigError::io(path, e))
}

/// Off unix there are no mode bits; `fs::copy` already propagates the
/// readonly attribute, so backups record `None` there.
#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "windows has no POSIX chmod; keeps the unix call sites uniform"
)]
fn set_permissions_u32(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

/// Flush + sync the backup (MUT-03). Windows must clear readonly to flush;
/// unix fsyncs through a read-only descriptor.
fn flush_backup_file(target: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let was_readonly = std::fs::metadata(target).is_ok_and(|m| m.permissions().readonly());
        if was_readonly {
            crate::atomic::windows_clear_readonly(target);
        }
        let sync_result = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(target)
            .and_then(|f| f.sync_all());
        let outcome = sync_result.map_err(|e| ConfigError::io(target, e));
        if was_readonly && let Ok(meta) = std::fs::metadata(target) {
            let mut perm = meta.permissions();
            perm.set_readonly(true);
            drop(std::fs::set_permissions(target, perm));
        }
        outcome
    }
    #[cfg(not(windows))]
    {
        let write_sync = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(target)
            .and_then(|f| f.sync_all());
        if write_sync.is_ok() {
            return Ok(());
        }
        // Read-only mode backup: open read-only and fsync through it.
        let f = std::fs::OpenOptions::new()
            .read(true)
            .open(target)
            .map_err(|e| ConfigError::io(target, e))?;
        f.sync_all().map_err(|e| ConfigError::io(target, e))
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "kept Result for fallible future use"
)]
fn generate_backup_path(original: &Path) -> Result<(PathBuf, u128, String)> {
    let millis = timestamp_millis_now();
    let suffix = generate_random_suffix(millis);
    let file_name = original.file_name().unwrap_or_default().to_os_string();
    let mut name = file_name;
    name.push(format!(".bak.{millis}.{suffix}"));
    let target = original.with_file_name(name);
    Ok((target, millis, suffix))
}

/// Steer away from taken names; after 5 collisions the last candidate is
/// returned and the caller decides (overwrite off unix, refuse on unix).
fn pick_backup_path(
    original: &Path,
    mut taken: impl FnMut(&Path) -> bool,
) -> Result<(PathBuf, u128, String)> {
    let mut attempts = 0;
    loop {
        let candidate = generate_backup_path(original)?;
        if !taken(&candidate.0) {
            return Ok(candidate);
        }
        attempts += 1;
        if attempts >= 5 {
            return Ok(candidate);
        }
    }
}

/// `create_new` write: any occupied name, a planted symlink included, fails
/// `AlreadyExists`, so the write can never go through a link.
#[cfg(unix)]
fn write_backup_exclusive(target: &Path, bytes: &[u8], mode: Option<u32>) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    // Create with at most the recorded permission bits (umask may narrow
    // them); the chmod after landing makes them exact.
    options
        .write(true)
        .create_new(true)
        .mode(mode.unwrap_or(0o600) & 0o777);
    let mut file = options
        .open(target)
        .map_err(|e| ConfigError::io(target, e))?;
    file.write_all(bytes)
        .map_err(|e| ConfigError::io(target, e))
}

/// Exclusive-create a fresh name, retrying a bounded number of collisions;
/// refuses to overwrite after repeated collisions (unix).
#[cfg(unix)]
fn write_backup_bytes(
    original: &Path,
    bytes: &[u8],
    mode: Option<u32>,
) -> Result<(PathBuf, u128, String)> {
    let mut collisions = 0u32;
    loop {
        let (target, millis, suffix) = pick_backup_path(original, Path::exists)?;
        match write_backup_exclusive(&target, bytes, mode) {
            Ok(()) => return Ok((target, millis, suffix)),
            Err(ConfigError::Io { ref source, .. })
                if source.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                collisions += 1;
                if collisions >= 5 {
                    return Err(ConfigError::io(
                        &target,
                        std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            "no free backup name after repeated collisions",
                        ),
                    ));
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// Stable backup identifier `<millis>-<4hex>`; validation is lenient here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BackupId(String);

impl BackupId {
    /// Create a new `BackupId` from a string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow as `str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume into `String`.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for BackupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<BackupId> for String {
    fn from(id: BackupId) -> Self {
        id.0
    }
}

/// Catalog entry for one backup: locate/verify/restore metadata, no contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupEntry {
    /// Stable backup identifier.
    pub id: BackupId,
    /// Operation that triggered the backup, if any.
    pub operation_id: Option<String>,
    /// Original file that was backed up.
    pub original_path: PathBuf,
    /// Path to the backup file on disk.
    pub backup_path: PathBuf,
    /// Millis since epoch when the backup was created.
    pub timestamp_millis: u128,
    /// Collision-resistant 4-hex suffix.
    pub suffix: String,
    /// Hex digest of the original file before write.
    pub digest: String,
    /// Size in bytes of the original file before write.
    pub size: u64,
    /// Permissions mode where available (unix `mode`).
    pub permissions: Option<u32>,
    /// Human-readable reason for the backup.
    pub reason: String,
}

/// Back up `path` beside itself, digest-verified; `Ok(None)` for a missing file.
pub fn backup(path: &Path) -> Result<Option<BackupEntry>> {
    backup_with_reason(path, "pre-write backup")
}

/// Back up `path` with an explicit `reason` (stored, never contents).
pub fn backup_with_reason(path: &Path, reason: &str) -> Result<Option<BackupEntry>> {
    backup_with_operation(path, None, reason)
}

/// Back up `path` with an optional `operation_id` and `reason`.
pub fn backup_with_operation(
    path: &Path,
    operation_id: Option<&str>,
    reason: &str,
) -> Result<Option<BackupEntry>> {
    backup_inner(path, operation_id, reason, None)
}

/// [`backup_with_operation`] with a failure injector at the backup boundaries.
pub fn backup_with_injector(
    path: &Path,
    operation_id: Option<&str>,
    reason: &str,
    injector: Option<&dyn Injector>,
) -> Result<Option<BackupEntry>> {
    backup_inner(path, operation_id, reason, injector)
}

fn backup_inner(
    path: &Path,
    operation_id: Option<&str>,
    reason: &str,
    injector: Option<&dyn Injector>,
) -> Result<Option<BackupEntry>> {
    inject(injector, Point::BackupOpen)?;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ConfigError::io(path, e)),
    };

    if meta.is_dir() {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "is a directory"),
        ));
    }

    if meta.file_type().is_symlink() {
        let target_meta = std::fs::metadata(path).map_err(|e| ConfigError::io(path, e))?;
        if target_meta.is_dir() {
            return Err(ConfigError::io(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "symlink target is a directory",
                ),
            ));
        }
    } else if !meta.is_file() {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
        ));
    }

    let original_bytes = std::fs::read(path).map_err(|e| ConfigError::io(path, e))?;
    let digest = compute_digest(&original_bytes);
    let size = original_bytes.len() as u64;
    let permissions = get_permissions_u32(&meta);

    inject(injector, Point::BackupWrite)?;
    // Unix exclusive-creates the backup: a symlink planted at the name is
    // refused, never followed. Windows fs::copy leaves a probe-to-copy race.
    #[cfg(unix)]
    let (target, millis, suffix) = write_backup_bytes(path, &original_bytes, permissions)?;
    #[cfg(not(unix))]
    let (target, millis, suffix) = {
        let picked = pick_backup_path(path, Path::exists)?;
        std::fs::copy(path, &picked.0).map_err(|e| ConfigError::io(path, e))?;
        picked
    };

    // Off unix `permissions` is always None, so this is a no-op there.
    if let Some(mode) = permissions {
        set_permissions_u32(&target, mode)?;
    }

    {
        inject(injector, Point::BackupFlush)?;
        flush_backup_file(&target)?;
    }

    inject(injector, Point::BackupVerify)?;
    let backup_bytes = std::fs::read(&target).map_err(|e| ConfigError::io(&target, e))?;
    let backup_digest = compute_digest(&backup_bytes);
    if backup_digest != digest {
        return Err(ConfigError::backup_verification(
            &target,
            format!("digest mismatch after copy: expected {digest}, got {backup_digest}"),
        ));
    }

    if (backup_bytes.len() as u64) != size {
        return Err(ConfigError::backup_verification(
            &target,
            format!(
                "size mismatch after copy: expected {size}, got {}",
                backup_bytes.len()
            ),
        ));
    }

    let id = BackupId::new(format!("{millis}-{suffix}"));

    Ok(Some(BackupEntry {
        id,
        operation_id: operation_id.map(ToOwned::to_owned),
        original_path: path.to_path_buf(),
        backup_path: target,
        timestamp_millis: millis,
        suffix,
        digest,
        size,
        permissions,
        reason: reason.to_owned(),
    }))
}

/// Restore a backup over `path` through the atomic write discipline:
/// interrupted leaves `path` fully old or fully restored, never truncated.
pub fn restore(backup_path: &Path, path: &Path) -> Result<()> {
    let backup_meta =
        std::fs::symlink_metadata(backup_path).map_err(|e| ConfigError::io(backup_path, e))?;
    if backup_meta.is_dir() {
        return Err(ConfigError::io(
            backup_path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "backup is a directory"),
        ));
    }
    let backup_bytes = std::fs::read(backup_path).map_err(|e| ConfigError::io(backup_path, e))?;
    // `backup` copies the original's permission bits onto the backup file, so
    // deriving the mode from the backup reinstates the recorded permissions.
    let mode = get_permissions_u32(&backup_meta);
    atomic_write_expecting(path, &backup_bytes, WriteExpectation::Any, mode, None)
}

/// Restore via a [`BackupEntry`]: digest/size must match or the target is untouched.
pub fn restore_entry(entry: &BackupEntry) -> Result<()> {
    let verified = verify_backup(entry)?;
    if !verified {
        return Err(ConfigError::backup_verification(
            &entry.backup_path,
            "backup digest does not match entry",
        ));
    }
    restore(&entry.backup_path, &entry.original_path)
}

/// List `<file_name>.bak.*` siblings sorted by timestamp then suffix.
pub fn list_backups(original_path: &Path) -> Result<Vec<BackupEntry>> {
    let parent = original_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = original_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if file_name.is_empty() {
        return Ok(Vec::new());
    }
    let prefix = format!("{file_name}.bak.");

    let dir = match std::fs::read_dir(parent) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ConfigError::io(parent, e)),
    };

    let mut entries = Vec::new();

    for ent in dir {
        let ent = ent.map_err(|e| ConfigError::io(parent, e))?;
        let name = ent.file_name();
        let name_str = name.to_string_lossy();
        let Some(rest) = name_str.strip_prefix(prefix.as_str()) else {
            continue;
        };
        let backup_path = ent.path();
        let Ok(meta) = std::fs::metadata(&backup_path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let mut parts = rest.split('.');
        let millis_str = parts.next().unwrap_or_default();
        let suffix = parts.next().unwrap_or("0000").to_owned();
        let timestamp_millis: u128 = millis_str.parse().unwrap_or(0);

        let Ok(bytes) = std::fs::read(&backup_path) else {
            continue;
        };
        let digest = compute_digest(&bytes);
        let size = bytes.len() as u64;
        let permissions = get_permissions_u32(&meta);
        let id = BackupId::new(format!("{timestamp_millis}-{suffix}"));

        entries.push(BackupEntry {
            id,
            operation_id: None,
            original_path: original_path.to_path_buf(),
            backup_path,
            timestamp_millis,
            suffix,
            digest,
            size,
            permissions,
            reason: String::new(),
        });
    }

    entries.sort_by(|a, b| {
        a.timestamp_millis
            .cmp(&b.timestamp_millis)
            .then_with(|| a.suffix.cmp(&b.suffix))
    });

    Ok(entries)
}

/// Whether the backup file matches its entry: `Ok(false)` on mismatch.
pub fn verify_backup(entry: &BackupEntry) -> Result<bool> {
    let bytes =
        std::fs::read(&entry.backup_path).map_err(|e| ConfigError::io(&entry.backup_path, e))?;
    let digest = compute_digest(&bytes);
    let size = bytes.len() as u64;
    Ok(digest == entry.digest && size == entry.size)
}

/// Whether the entry belongs to `target`: same original path and a properly
/// named sibling backup, so restores cannot cross identities (MUT-07).
pub fn verify_backup_relation(entry: &BackupEntry, target: &Path) -> Result<bool> {
    if entry.original_path != target {
        return Ok(false);
    }
    let Some(parent) = target.parent() else {
        return Ok(false);
    };
    let backup_parent = entry.backup_path.parent().unwrap_or_else(|| Path::new("."));
    if backup_parent != parent && !parent.as_os_str().is_empty() {
        return Ok(false);
    }
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let backup_name = entry
        .backup_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if !backup_name.starts_with(file_name) {
        return Ok(false);
    }
    if !backup_name.contains(".bak.") {
        return Ok(false);
    }
    verify_backup(entry)
}

/// Find a backup by [`BackupId`] via the catalog, never a user-built path.
pub fn find_backup_by_id(original_path: &Path, id: &BackupId) -> Result<Option<BackupEntry>> {
    let entries = list_backups(original_path)?;
    for entry in entries {
        if entry.id == *id {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

/// Redact any line whose key looks secret-bearing; never raw secret material.
fn redact_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let needs_redact = lower.contains("apikey")
        || lower.contains("api_key")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("authorization")
        || lower.contains("bearer");
    if !needs_redact {
        return line.to_owned();
    }
    if let Some(pos) = line.find(':') {
        let (key, _) = line.split_at(pos + 1);
        format!("{key} [REDACTED]")
    } else if let Some(pos) = line.find('=') {
        let (key, _) = line.split_at(pos + 1);
        format!("{key}[REDACTED]")
    } else {
        "[REDACTED]".to_owned()
    }
}

/// Redacted diff preview; non-UTF-8 content yields a size/digest summary.
pub fn redacted_diff_preview(current: &[u8], backup: &[u8]) -> String {
    let current_text = std::str::from_utf8(current);
    let backup_text = std::str::from_utf8(backup);
    if current_text.is_err() || backup_text.is_err() {
        return format!(
            "binary diff: current {} bytes ({}), backup {} bytes ({})",
            current.len(),
            compute_digest(current),
            backup.len(),
            compute_digest(backup)
        );
    }
    let current_str = current_text.unwrap_or_default();
    let backup_str = backup_text.unwrap_or_default();
    if current_str == backup_str {
        return "no changes".to_owned();
    }
    let current_lines: Vec<&str> = current_str.lines().collect();
    let backup_lines: Vec<&str> = backup_str.lines().collect();
    let mut out = String::new();
    let max = usize::max(current_lines.len(), backup_lines.len());
    for idx in 0..max {
        let cur = current_lines.get(idx).copied();
        let bak = backup_lines.get(idx).copied();
        match (cur, bak) {
            (Some(c), Some(b)) if c == b => {}
            (Some(c), Some(b)) => {
                out.push_str("- ");
                out.push_str(&redact_line(b));
                out.push('\n');
                out.push_str("+ ");
                out.push_str(&redact_line(c));
                out.push('\n');
            }
            (Some(c), None) => {
                out.push_str("+ ");
                out.push_str(&redact_line(c));
                out.push('\n');
            }
            (None, Some(b)) => {
                out.push_str("- ");
                out.push_str(&redact_line(b));
                out.push('\n');
            }
            (None, None) => {}
        }
        if out.len() > 4096 {
            out.push_str("... truncated\n");
            break;
        }
    }
    if out.is_empty() {
        "no line-level changes (whitespace or binary)".to_owned()
    } else {
        out
    }
}

/// Report produced after a verified restore (MUT-07).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    /// Redacted diff preview between current and backup before restore.
    pub preview_redacted: String,
    /// Backup taken of the current target before restore, if any.
    pub backup_before: Option<BackupEntry>,
    /// Whether the restore verification passed (digest + parse).
    pub verification_passed: bool,
    /// The backup entry that was restored.
    pub restored_entry: BackupEntry,
}

/// Restore by [`BackupId`] (MUT-07): resolved by ID, never a user path.
pub fn restore_by_id(original_path: &Path, backup_id: &BackupId) -> Result<RestoreReport> {
    let entry = find_backup_by_id(original_path, backup_id)?.ok_or_else(|| {
        ConfigError::io(
            original_path,
            std::io::Error::new(std::io::ErrorKind::NotFound, "backup id not found"),
        )
    })?;
    restore_verified(&entry)
}

/// Verify digest and relation, back up the current bytes, replace atomically,
/// verify the read-back (MUT-07).
pub fn restore_verified(entry: &BackupEntry) -> Result<RestoreReport> {
    let digest_ok = verify_backup(entry)?;
    if !digest_ok {
        return Err(ConfigError::backup_verification(
            &entry.backup_path,
            "backup digest does not match entry",
        ));
    }
    let relation_ok = verify_backup_relation(entry, &entry.original_path)?;
    if !relation_ok {
        return Err(ConfigError::backup_verification(
            &entry.original_path,
            "backup relation mismatch: entry does not belong to target",
        ));
    }
    // A target unreadable for any reason other than absence aborts the
    // restore instead of diffing against empty bytes.
    let current_bytes = match std::fs::read(&entry.original_path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(ConfigError::io(&entry.original_path, e)),
    };
    let backup_bytes =
        std::fs::read(&entry.backup_path).map_err(|e| ConfigError::io(&entry.backup_path, e))?;
    let preview_redacted =
        redacted_diff_preview(current_bytes.as_deref().unwrap_or_default(), &backup_bytes);
    let backup_before = if current_bytes.is_some() {
        backup_with_operation(
            &entry.original_path,
            entry.operation_id.as_deref(),
            "pre-restore backup",
        )?
    } else {
        None
    };
    // The fresh-read digest is the conflict token: a target that changed
    // since aborts instead of clobbering newer bytes.
    let current_digest = current_bytes.as_ref().map(|bytes| compute_digest(bytes));
    let expectation = match current_digest.as_deref() {
        Some(digest) => WriteExpectation::Digest(digest),
        None => WriteExpectation::Missing,
    };
    atomic_write_expecting(
        &entry.original_path,
        &backup_bytes,
        expectation,
        entry.permissions,
        None,
    )?;
    let restored_bytes = std::fs::read(&entry.original_path)
        .map_err(|e| ConfigError::io(&entry.original_path, e))?;
    let restored_digest = compute_digest(&restored_bytes);
    let backup_digest = compute_digest(&backup_bytes);
    let verification_passed =
        restored_digest == backup_digest && restored_bytes.len() == backup_bytes.len();
    if !verification_passed {
        return Err(ConfigError::verification(
            &entry.original_path,
            format!("restore verification failed: expected {backup_digest}, got {restored_digest}"),
        ));
    }
    // Best-effort parse check; a failure never fails the restore itself.
    if let Some(kind) = infer_kind_for_path(&entry.original_path) {
        drop(validate_bytes_for_kind(
            &restored_bytes,
            kind,
            &entry.original_path,
        ));
    }
    Ok(RestoreReport {
        preview_redacted,
        backup_before,
        verification_passed,
        restored_entry: entry.clone(),
    })
}

fn infer_kind_for_path(path: &Path) -> Option<crate::document::DocumentKind> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "json" => Some(crate::document::DocumentKind::StrictJson),
        "jsonc" => Some(crate::document::DocumentKind::JsonC),
        "toml" => Some(crate::document::DocumentKind::Toml),
        "yaml" | "yml" => Some(crate::document::DocumentKind::Yaml),
        _ => {
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && (name == ".env" || name.starts_with(".env."))
            {
                return Some(crate::document::DocumentKind::Env);
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::strip_jsonc_comments;

    fn scratch(name: &str) -> PathBuf {
        let dir = crate::test_util::temp_dir_unique("config-backup");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn unique_scratch(prefix: &str) -> PathBuf {
        let millis = timestamp_millis_now();
        let suffix = generate_random_suffix(millis);
        scratch(&format!("{prefix}-{millis}-{suffix}"))
    }

    #[test]
    fn backup_returns_none_for_missing_file() {
        let path = scratch("missing-for-backup.json");
        drop(std::fs::remove_file(&path));
        let entry = backup(&path).unwrap();
        assert!(entry.is_none(), "missing file should return None");
    }

    #[test]
    fn backup_creates_catalog_entry_with_digest_and_verifies() {
        let path = unique_scratch("digest-verify");
        std::fs::write(&path, b"hello backbone").unwrap();
        let entry = backup(&path).unwrap().expect("should create backup");
        assert!(!entry.digest.is_empty());
        assert_eq!(entry.digest.len(), 16);
        assert_eq!(entry.size, 14);
        assert!(entry.backup_path.exists());
        assert!(entry.timestamp_millis > 0);
        assert_eq!(entry.suffix.len(), 4);
        let ok = verify_backup(&entry).unwrap();
        assert!(ok, "fresh backup should verify");

        std::fs::write(&entry.backup_path, b"corrupted").unwrap();
        let ok2 = verify_backup(&entry).unwrap();
        assert!(!ok2, "corrupted backup should not verify");

        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[test]
    fn backup_preserves_permissions_where_supported() {
        let path = unique_scratch("perms");
        std::fs::write(&path, b"perms test").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perm = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&path, perm).unwrap();
        }

        let entry = backup(&path).unwrap().expect("backup");
        if entry.permissions.is_some() {
            let backup_meta = std::fs::metadata(&entry.backup_path).unwrap();
            let backup_perms = get_permissions_u32(&backup_meta);
            assert_eq!(backup_perms, entry.permissions);
        }

        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[test]
    #[cfg(windows)]
    fn backup_of_readonly_source_keeps_readonly_attribute_and_flushes() {
        let path = unique_scratch("perms-ro-win");
        std::fs::write(&path, b"readonly source").unwrap();
        let mut src_perm = std::fs::metadata(&path).unwrap().permissions();
        src_perm.set_readonly(true);
        std::fs::set_permissions(&path, src_perm).unwrap();

        let entry = backup(&path)
            .unwrap()
            .expect("backup of a readonly source must succeed (flush clears readonly momentarily)");
        assert!(
            entry.permissions.is_none(),
            "windows records no POSIX mode for backups"
        );
        let readonly = std::fs::metadata(&entry.backup_path)
            .unwrap()
            .permissions()
            .readonly();
        assert!(readonly, "fs::copy propagates the readonly attribute");
        restore(&entry.backup_path, &path).unwrap();
        let restored = std::fs::read(&path).unwrap();
        assert_eq!(restored, b"readonly source");

        let mut clear = std::fs::metadata(&path).unwrap().permissions();
        #[expect(
            clippy::permissions_set_readonly_false,
            reason = "windows-only test cleanup of the readonly attribute"
        )]
        clear.set_readonly(false);
        std::fs::set_permissions(&path, clear).unwrap();
        crate::atomic::windows_clear_readonly(&entry.backup_path);
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[test]
    fn backup_suffix_is_collision_resistant() {
        let path = unique_scratch("collision");
        std::fs::write(&path, b"v1").unwrap();
        let e1 = backup(&path).unwrap().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        std::fs::write(&path, b"v2").unwrap();
        let e2 = backup(&path).unwrap().unwrap();
        assert_ne!(e1.backup_path, e2.backup_path, "backup paths must differ");
        assert!(
            e1.id != e2.id || e1.backup_path != e2.backup_path,
            "ids or paths must differ"
        );
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&e1.backup_path));
        drop(std::fs::remove_file(&e2.backup_path));
    }

    #[test]
    fn pick_backup_path_takes_the_first_free_name() {
        let path = scratch("steer-free").with_file_name("cfg.json");
        let mut probes: Vec<PathBuf> = Vec::new();
        let (target, millis, suffix) = pick_backup_path(&path, |c| {
            probes.push(c.to_path_buf());
            false
        })
        .unwrap();
        assert_eq!(probes.len(), 1, "a free name needs exactly one probe");
        assert_eq!(probes.first().map(PathBuf::as_path), Some(target.as_path()));
        let expected = PathBuf::from(format!("cfg.json.bak.{millis}.{suffix}"));
        assert_eq!(
            target.file_name().and_then(|n| n.to_str()),
            expected.file_name().and_then(|n| n.to_str()),
            "the returned name keeps the <name>.bak.<millis>.<suffix> shape"
        );
        assert_eq!(suffix.len(), 4);
    }

    fn busy_prober(
        busy: usize,
    ) -> (
        std::rc::Rc<std::cell::RefCell<Vec<PathBuf>>>,
        impl FnMut(&Path) -> bool,
    ) {
        let probes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorder = std::rc::Rc::clone(&probes);
        let seen = std::cell::Cell::new(0usize);
        let taken = move |candidate: &Path| {
            recorder.borrow_mut().push(candidate.to_path_buf());
            let n = seen.get();
            seen.set(n + 1);
            n < busy
        };
        (probes, taken)
    }

    fn assert_all_distinct(paths: &[PathBuf]) {
        let mut sorted = paths.to_vec();
        sorted.sort();
        let duplicates = sorted.windows(2).any(|pair| pair[0] == pair[1]);
        assert!(!duplicates, "candidates must differ: {paths:?}");
    }

    #[test]
    fn pick_backup_path_steers_to_the_first_free_name() {
        let path = scratch("steer-busy").with_file_name("cfg.json");
        let (probes, taken) = busy_prober(2);
        let (target, _, _) = pick_backup_path(&path, taken).unwrap();
        let candidates = probes.borrow().clone();
        assert_eq!(candidates.len(), 3, "two busy candidates, then a free one");
        assert_all_distinct(&candidates);
        assert_eq!(target, candidates[2], "the first free candidate wins");
    }

    #[test]
    fn pick_backup_path_gives_up_after_five_busy_candidates() {
        let path = scratch("steer-full").with_file_name("cfg.json");
        let (probes, taken) = busy_prober(usize::MAX);
        let (target, _, _) = pick_backup_path(&path, taken).unwrap();
        let candidates = probes.borrow().clone();
        assert_eq!(candidates.len(), 5, "the give-up bound is five probes");
        assert_all_distinct(&candidates);
        assert_eq!(
            target, candidates[4],
            "the fifth busy candidate is returned for overwrite"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_backup_exclusive_lands_at_most_the_recorded_mode_bits() {
        use std::os::unix::fs::PermissionsExt;
        let path = unique_scratch("exclusive-mode");
        write_backup_exclusive(&path, b"backup bytes", Some(0)).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0, "a zero mode must not be widened at create");
        drop(std::fs::remove_file(&path));
    }

    #[cfg(unix)]
    #[test]
    fn write_backup_bytes_surfaces_non_collision_errors_untouched() {
        let original = crate::test_util::temp_dir_unique("config-backup-no-retry")
            .join("absent-parent")
            .join("cfg.json");
        let err = write_backup_bytes(&original, b"payload", None).unwrap_err();
        match &err {
            ConfigError::Io { source, .. } => assert_eq!(
                source.kind(),
                std::io::ErrorKind::NotFound,
                "the missing parent must surface, got {err}"
            ),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn list_backups_filters_and_sorts() {
        let path = unique_scratch("list-filter");
        std::fs::write(&path, b"base").unwrap();
        let e1 = backup(&path).unwrap().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let e2 = backup_with_reason(&path, "second").unwrap().unwrap();

        let list = list_backups(&path).unwrap();
        assert!(list.len() >= 2, "should list at least 2 backups");
        let ids: Vec<String> = list.iter().map(|e| e.id.to_string()).collect();
        assert!(ids.contains(&e1.id.to_string()));
        assert!(ids.contains(&e2.id.to_string()));

        let unrelated = scratch("unrelated.json");
        std::fs::write(&unrelated, b"x").unwrap();
        let list2 = list_backups(&unrelated).unwrap();
        for e in list2 {
            assert!(!ids.contains(&e.id.to_string()));
            drop(std::fs::remove_file(e.backup_path));
        }
        drop(std::fs::remove_file(&unrelated));

        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&e1.backup_path));
        drop(std::fs::remove_file(&e2.backup_path));
    }

    #[test]
    fn verify_backup_fails_on_missing_file() {
        let entry = BackupEntry {
            id: BackupId::new("0-0000"),
            operation_id: None,
            original_path: std::env::temp_dir().join("fake"),
            backup_path: std::env::temp_dir().join("does-not-exist-xyz-123"),
            timestamp_millis: 0,
            suffix: "0000".to_owned(),
            digest: "deadbeefdeadbeef".to_owned(),
            size: 0,
            permissions: None,
            reason: String::new(),
        };
        let res = verify_backup(&entry);
        assert!(res.is_err(), "missing backup should error");
    }

    #[test]
    fn backup_rejects_directory() {
        let dir = crate::test_util::temp_dir_unique("config-backup");
        std::fs::create_dir_all(&dir).unwrap();
        let err = backup(&dir).unwrap_err();
        match err {
            ConfigError::Io { .. } => {}
            other => panic!("unexpected error: {other:?}"),
        }
        drop(std::fs::remove_dir(&dir));
    }

    #[test]
    fn backup_with_operation_stores_operation_id() {
        let path = unique_scratch("op-id");
        std::fs::write(&path, b"op").unwrap();
        let entry = backup_with_operation(&path, Some("op-123"), "test")
            .unwrap()
            .unwrap();
        assert_eq!(entry.operation_id, Some("op-123".to_owned()));
        assert_eq!(entry.reason, "test");
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[test]
    fn no_auto_delete_listing_supports_filtering() {
        let path = unique_scratch("retention");
        std::fs::write(&path, b"a").unwrap();
        let mut ids = Vec::new();
        for i in 0..3 {
            std::fs::write(&path, format!("v{i}")).unwrap();
            let e = backup(&path).unwrap().unwrap();
            ids.push(e.backup_path.clone());
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let list = list_backups(&path).unwrap();
        assert!(list.len() >= 3, "all backups retained");
        for p in ids {
            drop(std::fs::remove_file(p));
        }
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn digest_is_stable() {
        let d1 = compute_digest(b"hello");
        let d2 = compute_digest(b"hello");
        let d3 = compute_digest(b"world");
        assert_eq!(d1, d2);
        assert_ne!(d1, d3);
        assert_eq!(d1.len(), 16);
    }

    fn assert_no_temp_litter(dir: &Path) {
        let entries = std::fs::read_dir(dir).unwrap();
        for e in entries.filter_map(std::result::Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with(".tmp."),
                "restore must not leave temp files behind: {name}"
            );
        }
    }

    #[test]
    fn restore_replaces_target_by_rename_never_in_place_truncation() {
        let path = unique_scratch("atomic-restore");
        std::fs::write(&path, b"original bytes that were backed up").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perm = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&path, perm).unwrap();
        }
        let entry = backup(&path).unwrap().expect("backup");

        #[cfg(unix)]
        let inode_before = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&path).unwrap().ino()
        };

        std::fs::write(&path, b"changed bytes that differ in length").unwrap();
        restore_entry(&entry).unwrap();

        let restored = std::fs::read(&path).unwrap();
        assert_eq!(
            restored, b"original bytes that were backed up",
            "restore must land the backup bytes exactly, with no tail residue"
        );

        // The replacement arrives by rename: an in-place copy keeps the inode,
        // a rename over the target replaces it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let meta = std::fs::metadata(&path).unwrap();
            assert_ne!(
                meta.ino(),
                inode_before,
                "restore must rename a new file into place, never truncate the target"
            );
            assert_eq!(
                meta.permissions().mode() & 0o777,
                0o600,
                "restored file must carry the permissions recorded in the entry"
            );
        }

        assert_no_temp_litter(path.parent().unwrap());
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[cfg(unix)]
    #[test]
    fn restore_failure_leaves_target_bytes_fully_intact() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_util::temp_dir_unique("config-backup-restore-fail");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("target.json");
        std::fs::write(&path, b"current bytes that must survive a failed restore").unwrap();
        let entry = backup(&path).unwrap().expect("backup");
        std::fs::write(&path, b"newer bytes").unwrap();

        let read_only = std::fs::Permissions::from_mode(0o500);
        std::fs::set_permissions(&dir, read_only).unwrap();
        let result = restore_entry(&entry);
        let writable = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(&dir, writable).unwrap();

        let err = result.expect_err("restore must fail while the directory rejects new files");
        assert!(
            matches!(err, ConfigError::Io { .. }),
            "expected io error, got {err:?}"
        );
        let after = std::fs::read(&path).unwrap();
        assert_eq!(
            after, b"newer bytes",
            "a failed restore must leave the target fully at its pre-restore bytes"
        );
        assert_no_temp_litter(&dir);
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
        drop(std::fs::remove_dir(&dir));
    }

    #[test]
    fn restore_verified_round_trips_and_recreates_missing_target() {
        let path = unique_scratch("verified-restore");
        std::fs::write(&path, b"v1").unwrap();
        let entry = backup(&path).unwrap().expect("backup");
        std::fs::write(&path, b"v2 which is longer than v1").unwrap();

        let report = restore_verified(&entry).unwrap();
        assert!(report.verification_passed, "read-back verification");
        assert_eq!(std::fs::read(&path).unwrap(), b"v1");
        let before = report
            .backup_before
            .expect("an existing target must be backed up before restore");
        assert_eq!(before.reason, "pre-restore backup");
        assert_eq!(before.digest, compute_digest(b"v2 which is longer than v1"));
        assert_no_temp_litter(path.parent().unwrap());

        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&before.backup_path));
        let second = restore_verified(&entry).unwrap();
        assert!(second.verification_passed);
        assert!(
            second.backup_before.is_none(),
            "nothing to back up for a missing target"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"v1");
        assert_no_temp_litter(path.parent().unwrap());

        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[cfg(unix)]
    #[test]
    fn restore_over_symlinked_original_replaces_the_link_not_its_referent() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_util::temp_dir_unique("config-backup-restore-symlink");
        std::fs::create_dir_all(&dir).unwrap();
        let referent = dir.join("referent.json");
        std::fs::write(&referent, b"referent bytes").unwrap();
        let link = dir.join("link.json");
        std::os::unix::fs::symlink(&referent, &link).unwrap();

        let entry = backup(&link)
            .unwrap()
            .expect("backup through a file symlink");
        std::fs::write(&link, b"changed by writing through the link").unwrap();
        assert_eq!(
            std::fs::read(&referent).unwrap(),
            b"changed by writing through the link"
        );

        restore_entry(&entry).unwrap();

        // The link itself is replaced by a regular file; the restore never
        // writes through it.
        let link_meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(
            !link_meta.file_type().is_symlink(),
            "restore must replace the link itself, not follow it"
        );
        assert!(link_meta.is_file());
        assert_eq!(std::fs::read(&link).unwrap(), b"referent bytes");
        assert_eq!(
            std::fs::read(&referent).unwrap(),
            b"changed by writing through the link",
            "the referent the link pointed at must be untouched"
        );
        assert_no_temp_litter(&dir);

        std::fs::set_permissions(&link, std::fs::Permissions::from_mode(0o600)).unwrap();
        drop(std::fs::remove_file(&link));
        drop(std::fs::remove_file(&referent));
        drop(std::fs::remove_file(&entry.backup_path));
        drop(std::fs::remove_dir(&dir));
    }

    #[cfg(unix)]
    #[test]
    fn restore_succeeds_over_read_only_target_and_lands_recorded_mode() {
        use std::os::unix::fs::PermissionsExt;
        fn set_mode(path: &Path, mode: u32) {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        }

        let path = unique_scratch("restore-readonly");
        std::fs::write(&path, b"backed up bytes").unwrap();
        set_mode(&path, 0o400);

        let entry = backup(&path).unwrap().expect("backup of a read-only file");
        assert_eq!(entry.permissions.map(|m| m & 0o777), Some(0o400));

        set_mode(&path, 0o600);
        std::fs::write(&path, b"newer bytes").unwrap();
        set_mode(&path, 0o400);

        restore_entry(&entry).unwrap();

        let restored = std::fs::read(&path).unwrap();
        assert_eq!(restored, b"backed up bytes");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o400, "the recorded read-only mode must be landed");
        assert_no_temp_litter(path.parent().unwrap());

        set_mode(&path, 0o600);
        set_mode(&entry.backup_path, 0o600);
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    fn backup_files_next_to(path: &Path) -> Vec<String> {
        let parent = path.parent().unwrap();
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let prefix = format!("{file_name}.bak.");
        let mut names = Vec::new();
        for entry in std::fs::read_dir(parent)
            .unwrap()
            .filter_map(std::result::Result::ok)
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(prefix.as_str()) {
                names.push(name);
            }
        }
        names.sort();
        names
    }

    #[derive(Debug)]
    struct FailAt(Point);

    impl Injector for FailAt {
        fn inject(&self, point: Point) -> Result<()> {
            if point == self.0 {
                Err(ConfigError::io(
                    Path::new("injected"),
                    std::io::Error::other("injected failure"),
                ))
            } else {
                Ok(())
            }
        }
    }

    /// Swap the fresh backup for a symlink to /dev/null at the flush
    /// boundary; meaningful only where that fsync fails EINVAL (Linux).
    #[cfg(target_os = "linux")]
    #[derive(Debug)]
    struct SwapBackupForDevNull {
        dir: PathBuf,
        prefix: String,
    }

    #[cfg(target_os = "linux")]
    impl Injector for SwapBackupForDevNull {
        fn inject(&self, point: Point) -> Result<()> {
            if point != Point::BackupFlush {
                return Ok(());
            }
            let backup_path = std::fs::read_dir(&self.dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .find(|path| {
                    path.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with(self.prefix.as_str()))
                });
            if let Some(backup_path) = backup_path {
                drop(std::fs::remove_file(&backup_path));
                drop(std::os::unix::fs::symlink("/dev/null", &backup_path));
            }
            Ok(())
        }
    }

    #[test]
    fn backup_with_injector_runs_the_real_pipeline_and_copies_the_file() {
        let path = unique_scratch("injector-none");
        std::fs::write(&path, b"payload for injector").unwrap();
        let entry = backup_with_injector(&path, Some("op-1"), "reason", None)
            .unwrap()
            .expect("a clean run through the injector plumbing must back up");
        assert_eq!(
            std::fs::read(&entry.backup_path).unwrap(),
            b"payload for injector"
        );
        assert_eq!(entry.operation_id.as_deref(), Some("op-1"));
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[test]
    fn backup_with_injector_failures_at_each_boundary() {
        for point in [
            Point::BackupOpen,
            Point::BackupWrite,
            Point::BackupFlush,
            Point::BackupVerify,
        ] {
            let path = unique_scratch(&format!("injector-{point}"));
            std::fs::write(&path, b"boundaries").unwrap();
            let injector = FailAt(point);
            let res = backup_with_injector(&path, None, "reason", Some(&injector));
            assert!(res.is_err(), "the {point} failure must surface as an error");
            assert_eq!(
                std::fs::read(&path).unwrap(),
                b"boundaries",
                "a backup boundary failure must never touch the original"
            );
            let leftovers = backup_files_next_to(&path);
            match point {
                Point::BackupOpen | Point::BackupWrite => assert!(
                    leftovers.is_empty(),
                    "{point} fires before the copy; found {leftovers:?}"
                ),
                _ => assert_eq!(
                    leftovers.len(),
                    1,
                    "{point} leaves exactly the copied backup behind: {leftovers:?}"
                ),
            }
            let parent = path.parent().unwrap();
            for leftover in leftovers {
                drop(std::fs::remove_file(parent.join(&leftover)));
            }
            drop(std::fs::remove_file(&path));
            drop(std::fs::remove_dir(parent));
        }
    }

    /// The flush is real: fsync on /dev/null fails EINVAL on Linux, so a
    /// stubbed flush surfaces before digest verification could pass.
    #[cfg(target_os = "linux")]
    #[test]
    fn backup_surfaces_flush_sync_errors_before_digest_verification() {
        if !Path::new("/dev/null").exists() {
            return;
        }
        let path = unique_scratch("flush-einval");
        std::fs::write(&path, b"flushable").unwrap();
        let injector = SwapBackupForDevNull {
            dir: path.parent().unwrap().to_path_buf(),
            prefix: format!("{}.bak.", path.file_name().unwrap().to_string_lossy()),
        };
        let res = backup_with_injector(&path, None, "reason", Some(&injector));
        match res {
            Err(ConfigError::Io { source, .. }) => assert_eq!(
                // EINVAL from fsync(/dev/null); the kind is unstable
                // `Uncategorized`, so assert the raw OS error.
                source.raw_os_error(),
                Some(22),
                "the flush must surface the fsync EINVAL from /dev/null"
            ),
            other => panic!("expected the flush io error, got {other:?}"),
        }
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_dir_all(path.parent().unwrap()));
    }

    #[test]
    fn backup_id_accessors_preserve_the_string() {
        let id = BackupId::new("1714123456789-a1b2");
        assert_eq!(id.as_str(), "1714123456789-a1b2");
        assert_eq!(id.to_string(), "1714123456789-a1b2");
        assert_eq!(id.clone().into_string(), "1714123456789-a1b2");
        assert_eq!(String::from(id), "1714123456789-a1b2");
    }

    #[test]
    fn backup_entry_and_file_name_embed_recent_millis_and_compact_hex_suffix() {
        let path = unique_scratch("naming");
        std::fs::write(&path, b"naming").unwrap();
        let entry = backup(&path).unwrap().expect("backup");
        assert!(
            entry.timestamp_millis >= 1_600_000_000_000,
            "backup millis must be a post-2020 epoch value, got {}",
            entry.timestamp_millis
        );
        assert_eq!(entry.suffix.len(), 4, "suffix must be four chars");
        assert!(
            entry
                .suffix
                .chars()
                .all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "suffix must be lowercase hex, got {}",
            entry.suffix
        );
        assert_eq!(
            entry.id.to_string(),
            format!("{}-{}", entry.timestamp_millis, entry.suffix),
            "the id is <millis>-<suffix>"
        );
        let expected_name = format!(
            "{}.bak.{}.{}",
            path.file_name().unwrap().to_string_lossy(),
            entry.timestamp_millis,
            entry.suffix
        );
        assert_eq!(
            entry.backup_path.file_name().unwrap(),
            std::ffi::OsStr::new(expected_name.as_str()),
            "the backup file name carries the same millis and suffix"
        );
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[cfg(unix)]
    #[test]
    fn backup_reports_io_error_for_symlink_loop() {
        let dir = crate::test_util::temp_dir_unique("config-backup-loop");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.json");
        let b = dir.join("b.json");
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        match backup(&a) {
            Err(ConfigError::Io { .. }) => {}
            other => panic!("expected io error for an unresolvable symlink, got {other:?}"),
        }
        drop(std::fs::remove_file(&a));
        drop(std::fs::remove_file(&b));
        drop(std::fs::remove_dir(&dir));
    }

    #[cfg(unix)]
    #[test]
    fn backup_reports_io_error_when_parent_directory_is_unsearchable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_util::temp_dir_unique("config-backup-lstat-denied");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, b"settings").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        if path.symlink_metadata().is_ok() {
            // Root bypasses permission checks; the arm is unreachable here.
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            drop(std::fs::remove_file(&path));
            drop(std::fs::remove_dir(&dir));
            return;
        }
        let res = backup(&path);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        match res {
            Err(ConfigError::Io { source, .. }) => assert_eq!(
                source.kind(),
                std::io::ErrorKind::PermissionDenied,
                "lstat through an unsearchable parent must surface EACCES"
            ),
            other => panic!("expected io error for an unsearchable parent, got {other:?}"),
        }
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_dir(&dir));
    }

    #[cfg(unix)]
    #[test]
    fn backup_rejects_non_regular_files() {
        let dev_null = Path::new("/dev/null");
        if !dev_null.exists() {
            return;
        }
        match backup(dev_null) {
            Err(ConfigError::Io { source, .. }) => assert_eq!(
                source.kind(),
                std::io::ErrorKind::InvalidInput,
                "character devices must be rejected as non-regular files"
            ),
            other => panic!("expected rejection of a non-regular file, got {other:?}"),
        }
    }

    #[test]
    fn list_backups_returns_empty_for_missing_parent() {
        let root = crate::test_util::temp_dir_unique("config-backup-list-missing");
        let target = root.join("never-created").join("settings.json");
        let list = list_backups(&target).unwrap();
        assert!(list.is_empty(), "an absent parent directory lists nothing");
        drop(std::fs::remove_dir(&root));
    }

    #[cfg(unix)]
    #[test]
    fn list_backups_surfaces_unreadable_parent_errors() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_util::temp_dir_unique("config-backup-list-denied");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o333)).unwrap();
        if std::fs::read_dir(&dir).is_ok() {
            // The chmod does not deny this process (root bypasses permission
            // checks); the arm is unreachable here.
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            drop(std::fs::remove_dir(&dir));
            return;
        }
        let res = list_backups(&dir.join("settings.json"));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        match res {
            Err(ConfigError::Io { path, .. }) => assert_eq!(
                path, dir,
                "the read_dir failure is reported against the parent"
            ),
            other => panic!("expected io error for an unreadable parent, got {other:?}"),
        }
        drop(std::fs::remove_dir(&dir));
    }

    #[test]
    fn list_backups_lists_only_regular_backup_files() {
        let path = unique_scratch("list-nonfile");
        std::fs::write(&path, b"real").unwrap();
        let entry = backup(&path).unwrap().expect("backup");
        let decoy = path.parent().unwrap().join(format!(
            "{}.bak.0.dead",
            path.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir(&decoy).unwrap();
        let list = list_backups(&path).unwrap();
        assert_eq!(list.len(), 1, "only the real backup file is listed");
        assert_eq!(list[0].id, entry.id);
        drop(std::fs::remove_dir(&decoy));
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
        drop(std::fs::remove_dir(path.parent().unwrap()));
    }

    #[test]
    fn verify_backup_requires_both_digest_and_size_to_match() {
        let path = unique_scratch("verify-parts");
        std::fs::write(&path, b"verify me").unwrap();
        let entry = backup(&path).unwrap().expect("backup");
        assert!(verify_backup(&entry).unwrap(), "the fresh entry verifies");

        let wrong_size = BackupEntry {
            size: entry.size + 1,
            ..entry.clone()
        };
        assert!(
            !verify_backup(&wrong_size).unwrap(),
            "a matching digest with a wrong size must not verify"
        );

        let wrong_digest = BackupEntry {
            digest: compute_digest(b"other bytes"),
            ..entry.clone()
        };
        assert!(
            !verify_backup(&wrong_digest).unwrap(),
            "a matching size with a wrong digest must not verify"
        );

        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[test]
    fn verify_backup_relation_accepts_only_named_siblings_of_the_target() {
        let dir = crate::test_util::temp_dir_unique("config-backup-relation");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("cfg.json");
        std::fs::write(&target, b"relation").unwrap();
        let entry = backup(&target).unwrap().expect("backup");

        assert!(
            verify_backup_relation(&entry, &target).unwrap(),
            "a fresh backup relates to its target"
        );

        let other = dir.join("other.json");
        assert!(
            !verify_backup_relation(&entry, &other).unwrap(),
            "a backup of cfg.json does not relate to other.json"
        );

        let foreign_dir = crate::test_util::temp_dir_unique("config-backup-relation-foreign");
        std::fs::create_dir_all(&foreign_dir).unwrap();
        let foreign = foreign_dir.join(entry.backup_path.file_name().unwrap());
        std::fs::copy(&entry.backup_path, &foreign).unwrap();
        let foreign_entry = BackupEntry {
            backup_path: foreign,
            ..entry.clone()
        };
        assert!(
            !verify_backup_relation(&foreign_entry, &target).unwrap(),
            "a backup in another directory is not a sibling"
        );
        drop(std::fs::remove_dir_all(&foreign_dir));

        let misnamed = dir.join("xcfg.json.bak.1.abcd");
        std::fs::copy(&entry.backup_path, &misnamed).unwrap();
        let misnamed_entry = BackupEntry {
            backup_path: misnamed.clone(),
            ..entry.clone()
        };
        assert!(
            !verify_backup_relation(&misnamed_entry, &target).unwrap(),
            "a backup name must start with the target's file name"
        );

        let unmarked = dir.join("cfg.json.old");
        std::fs::copy(&entry.backup_path, &unmarked).unwrap();
        let unmarked_entry = BackupEntry {
            backup_path: unmarked.clone(),
            ..entry.clone()
        };
        assert!(
            !verify_backup_relation(&unmarked_entry, &target).unwrap(),
            "a backup name must carry the .bak. marker"
        );

        let bare = BackupEntry {
            original_path: PathBuf::from("cfg.json"),
            backup_path: entry.backup_path.clone(),
            ..entry.clone()
        };
        assert!(
            verify_backup_relation(&bare, Path::new("cfg.json")).unwrap(),
            "an empty parent component must not fail the sibling check"
        );

        drop(std::fs::remove_file(&misnamed));
        drop(std::fs::remove_file(&unmarked));
        drop(std::fs::remove_file(&target));
        drop(std::fs::remove_file(&entry.backup_path));
        drop(std::fs::remove_dir(&dir));
    }

    #[test]
    fn find_backup_by_id_resolves_only_known_ids() {
        let path = unique_scratch("find-by-id");
        std::fs::write(&path, b"findable").unwrap();
        let entry = backup(&path).unwrap().expect("backup");
        let found = find_backup_by_id(&path, &entry.id)
            .unwrap()
            .expect("the fresh backup id must resolve");
        assert_eq!(found.id, entry.id);
        assert_eq!(found.digest, entry.digest);
        assert_eq!(found.size, entry.size);
        let missing = find_backup_by_id(&path, &BackupId::new("0-dead")).unwrap();
        assert!(missing.is_none(), "an unknown id must not resolve");
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[test]
    fn redact_line_redacts_each_secret_keyword_in_both_separator_styles() {
        assert_eq!(redact_line("apikey: sk-123"), "apikey: [REDACTED]");
        assert_eq!(redact_line("password=x"), "password=[REDACTED]");
        assert_eq!(redact_line("ordinary line"), "ordinary line");
        assert_eq!(redact_line("token"), "[REDACTED]");

        let keyword_lines = [
            "apikey: sk-123",
            "api_key=abc",
            "the secret: value",
            "token: t",
            "password: p",
            "authorization: Bearer x",
            "bearer: y",
        ];
        for line in keyword_lines {
            let redacted = redact_line(line);
            assert!(
                redacted.contains("[REDACTED]"),
                "{line:?} must be redacted, got {redacted:?}"
            );
            assert!(
                !redacted.contains("sk-123") && !redacted.contains("abc"),
                "the raw value must never survive: {redacted:?}"
            );
        }
    }

    #[test]
    fn redacted_diff_preview_marks_no_changes_and_line_changes() {
        assert_eq!(redacted_diff_preview(b"same", b"same"), "no changes");
        assert_eq!(
            redacted_diff_preview(b"new line", b"old line"),
            "- old line\n+ new line\n"
        );
        assert_eq!(
            redacted_diff_preview(b"x\nsame\nz", b"a\nsame\nc"),
            "- a\n+ x\n- c\n+ z\n"
        );
        assert_eq!(
            redacted_diff_preview(b"password: new", b"password: old"),
            "- password: [REDACTED]\n+ password: [REDACTED]\n"
        );
        let small = redacted_diff_preview(b"x\ny\nz", b"a\nb\nc");
        assert_eq!(small, "- a\n+ x\n- b\n+ y\n- c\n+ z\n");
        assert!(!small.contains("truncated"));
    }

    #[test]
    fn redacted_diff_preview_summarizes_binary_content() {
        let summary = redacted_diff_preview(b"plain text", b"\xff\xfe binary");
        assert!(
            summary.starts_with("binary diff: current 10 bytes"),
            "unexpected summary: {summary}"
        );
        assert!(
            summary.contains("backup 9 bytes"),
            "unexpected summary: {summary}"
        );
        assert!(
            summary.contains(compute_digest(b"plain text").as_str()),
            "the summary embeds the current digest: {summary}"
        );
        assert!(
            summary.contains(compute_digest(b"\xff\xfe binary").as_str()),
            "the summary embeds the backup digest: {summary}"
        );
    }

    /// The budget truncates only past 4096 bytes: the first pair below
    /// contributes exactly 4096, so the second pair must still appear.
    #[test]
    fn redacted_diff_preview_truncates_only_past_the_size_budget() {
        let old_first = "B".repeat(2000);
        let new_first = "C".repeat(2090);
        let backup = format!("{old_first}\nsecond-old");
        let current = format!("{new_first}\nsecond-new");
        let out = redacted_diff_preview(current.as_bytes(), backup.as_bytes());
        assert!(
            out.contains("... truncated\n"),
            "a >4 KiB diff must be truncated, got {} bytes",
            out.len()
        );
        assert!(
            out.contains("second-old") && out.contains("second-new"),
            "the second pair still fits after the exact-4096 first pair"
        );
        assert!(
            out.contains(old_first.as_str()),
            "the first pair is included"
        );
    }

    #[test]
    fn validate_bytes_rejects_invalid_documents_and_accepts_valid_ones() {
        use crate::document::DocumentKind;
        let p = Path::new("probe");
        assert!(validate_bytes_for_kind(b"{ bad json", DocumentKind::StrictJson, p).is_err());
        validate_bytes_for_kind(b"{}", DocumentKind::StrictJson, p).unwrap();
        assert!(validate_bytes_for_kind(b"not toml ]", DocumentKind::Toml, p).is_err());
        validate_bytes_for_kind(b"a = 1\n", DocumentKind::Toml, p).unwrap();
        validate_bytes_for_kind(b"KEY=1\n\n# comment\nOTHER =2\n", DocumentKind::Env, p).unwrap();
        assert!(validate_bytes_for_kind(b"KEY=1\nNO_EQUALS_HERE\n", DocumentKind::Env, p).is_err());
        validate_bytes_for_kind(b"KEY=1\nexport EXPORTED=3\n", DocumentKind::Env, p).unwrap();
        validate_bytes_for_kind(b"{ \"a\": 1 } // tail\n", DocumentKind::JsonC, p).unwrap();
        assert!(validate_bytes_for_kind(b"{ broken // x\n", DocumentKind::JsonC, p).is_err());
        validate_bytes_for_kind(b"a: 1\n", DocumentKind::Yaml, p).unwrap();
        assert!(validate_bytes_for_kind(b"a: [1,\n", DocumentKind::Yaml, p).is_err());
    }

    #[test]
    fn infer_kind_covers_every_supported_extension_and_env_names() {
        use crate::document::DocumentKind;
        assert_eq!(
            infer_kind_for_path(Path::new("a/config.json")),
            Some(DocumentKind::StrictJson)
        );
        assert_eq!(
            infer_kind_for_path(Path::new("b.JSON")),
            Some(DocumentKind::StrictJson)
        );
        assert_eq!(
            infer_kind_for_path(Path::new("c.jsonc")),
            Some(DocumentKind::JsonC)
        );
        assert_eq!(
            infer_kind_for_path(Path::new("d.toml")),
            Some(DocumentKind::Toml)
        );
        assert_eq!(
            infer_kind_for_path(Path::new("e.yaml")),
            Some(DocumentKind::Yaml)
        );
        assert_eq!(
            infer_kind_for_path(Path::new("f.yml")),
            Some(DocumentKind::Yaml)
        );
        assert_eq!(
            infer_kind_for_path(Path::new(".env")),
            Some(DocumentKind::Env)
        );
        assert_eq!(
            infer_kind_for_path(Path::new(".env.production")),
            Some(DocumentKind::Env)
        );
        assert_eq!(infer_kind_for_path(Path::new("notes.txt")), None);
        assert_eq!(infer_kind_for_path(Path::new("noext")), None);
        assert_eq!(infer_kind_for_path(Path::new(".envx")), None);
    }

    #[test]
    fn strip_comments_removes_line_and_block_comments_outside_strings() {
        assert_eq!(strip_jsonc_comments("a // tail\nb"), "a \nb");
        assert_eq!(strip_jsonc_comments("a /* hidden */ b"), "a  b");
        assert_eq!(strip_jsonc_comments("/* multi\nline */x"), "x");
        assert_eq!(strip_jsonc_comments("keep // only"), "keep ");
        assert_eq!(strip_jsonc_comments("plain"), "plain");
        assert_eq!(strip_jsonc_comments(""), "");
        assert_eq!(strip_jsonc_comments("s/**/e"), "se");
        assert_eq!(strip_jsonc_comments("a/b"), "a/b");
        assert_eq!(strip_jsonc_comments("a/"), "a/");
    }

    #[test]
    fn strip_comments_preserves_string_contents_and_escapes() {
        assert_eq!(strip_jsonc_comments(r#""a//b""#), r#""a//b""#);
        assert_eq!(strip_jsonc_comments(r#""x"// c"#), r#""x""#);
        assert_eq!(
            strip_jsonc_comments(r#""a\""// c"#),
            r#""a\"""#,
            "an escaped quote keeps the string open past the comment"
        );
        assert_eq!(
            strip_jsonc_comments(r#"q"// c""#),
            r#"q"// c""#,
            "a bare quote opens a string: the comment marker inside is data"
        );
        assert_eq!(
            strip_jsonc_comments(r#""a\\" // c"#),
            r#""a\\" "#,
            "an escaped backslash does not hide the closing quote"
        );
    }

    #[test]
    fn restore_refuses_a_corrupted_backup_and_keeps_the_target() {
        let path = unique_scratch("restore-corrupt");
        std::fs::write(&path, b"backed up").unwrap();
        let entry = backup(&path).unwrap().expect("backup");
        std::fs::write(&path, b"current").unwrap();
        // Same length, different bytes: only the digest can catch it.
        std::fs::write(&entry.backup_path, b"corrupted").unwrap();
        let res = restore_entry(&entry);
        assert!(res.is_err(), "a corrupted backup must be refused");
        match res {
            Err(ConfigError::BackupVerification { .. }) => {}
            other => panic!("expected BackupVerification, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"current",
            "the target must stay untouched by a refused restore"
        );
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[test]
    fn restore_verified_refuses_a_backup_from_another_target() {
        let dir = crate::test_util::temp_dir_unique("config-backup-wrong-target");
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.json");
        std::fs::write(&source, b"source bytes").unwrap();
        let entry = backup(&source).unwrap().expect("backup");
        let victim = dir.join("victim.json");
        std::fs::write(&victim, b"victim bytes").unwrap();
        let forged = BackupEntry {
            original_path: victim.clone(),
            ..entry.clone()
        };
        match restore_verified(&forged) {
            Err(ConfigError::BackupVerification { .. }) => {}
            other => panic!("expected BackupVerification for a relation mismatch, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"victim bytes",
            "the unrelated target must stay untouched"
        );
        drop(std::fs::remove_file(&source));
        drop(std::fs::remove_file(&victim));
        drop(std::fs::remove_file(&entry.backup_path));
        drop(std::fs::remove_dir(&dir));
    }

    #[cfg(unix)]
    #[test]
    fn restore_verified_aborts_when_the_current_target_cannot_be_read() {
        use std::os::unix::fs::PermissionsExt;
        let path = unique_scratch("verified-unreadable");
        std::fs::write(&path, b"backed up").unwrap();
        let entry = backup(&path).unwrap().expect("backup");
        std::fs::write(&path, b"current secret").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&path).is_ok() {
            // Root bypasses permission checks; the arm is unreachable here.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            drop(std::fs::remove_file(&path));
            drop(std::fs::remove_file(&entry.backup_path));
            return;
        }
        let res = restore_verified(&entry);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        match res {
            Err(ConfigError::Io { source, .. }) => assert_eq!(
                source.kind(),
                std::io::ErrorKind::PermissionDenied,
                "the unreadable current target must abort the restore"
            ),
            other => panic!("expected io error for an unreadable target, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"current secret",
            "the target must stay untouched by the aborted restore"
        );
        drop(std::fs::remove_file(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[cfg(unix)]
    #[test]
    fn backup_of_a_symlink_lands_the_link_mode_not_the_referent_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_util::temp_dir_unique("config-backup-link-mode");
        std::fs::create_dir_all(&dir).unwrap();
        let referent = dir.join("referent.json");
        std::fs::write(&referent, b"symlinked content").unwrap();
        std::fs::set_permissions(&referent, std::fs::Permissions::from_mode(0o644)).unwrap();
        let link = dir.join("link.json");
        std::os::unix::fs::symlink(&referent, &link).unwrap();

        let entry = backup(&link)
            .unwrap()
            .expect("backup of a symlink to a regular file must succeed");

        // The link's own mode is platform-given (Linux 0o777, macOS 0o755):
        // derive it from a fresh lstat; the premise only needs link != referent.
        let link_mode = std::fs::symlink_metadata(&link)
            .unwrap()
            .permissions()
            .mode();
        let referent_mode = std::fs::metadata(&referent).unwrap().permissions().mode();
        if link_mode & 0o777 == referent_mode & 0o777 {
            // A platform where the link carries the referent's mode: the
            // distinguishing premise is absent (not hit on Linux or macOS).
            drop(std::fs::remove_file(&link));
            drop(std::fs::remove_file(&entry.backup_path));
            drop(std::fs::remove_file(&referent));
            drop(std::fs::remove_dir(&dir));
            return;
        }
        assert_eq!(
            entry.permissions,
            Some(link_mode),
            "the entry records the link's own mode from symlink_metadata"
        );
        let backup_mode = std::fs::metadata(&entry.backup_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            backup_mode & 0o777,
            link_mode & 0o777,
            "the landed backup must carry the recorded link mode, not the referent's 0o644"
        );
        drop(std::fs::remove_file(&link));
        drop(std::fs::remove_file(&entry.backup_path));
        drop(std::fs::remove_file(&referent));
        drop(std::fs::remove_dir(&dir));
    }

    #[cfg(unix)]
    #[test]
    fn restore_verified_surfaces_is_a_directory_when_the_original_became_a_directory() {
        let path = unique_scratch("verified-isdir");
        std::fs::write(&path, b"backed up").unwrap();
        let entry = backup(&path).unwrap().expect("backup");
        drop(std::fs::remove_file(&path));
        std::fs::create_dir(&path).unwrap();

        let res = restore_verified(&entry);
        match res {
            Err(ConfigError::Io { source, .. }) => assert_eq!(
                source.kind(),
                std::io::ErrorKind::IsADirectory,
                "EISDIR from the fresh read must surface, not be masked to missing"
            ),
            other => panic!("expected io error for a directory at the original, got {other:?}"),
        }
        assert!(
            path.is_dir(),
            "the aborted restore must leave the directory untouched"
        );
        drop(std::fs::remove_dir(&path));
        drop(std::fs::remove_file(&entry.backup_path));
    }

    #[cfg(unix)]
    #[test]
    fn exclusive_backup_refuses_a_held_backup_name_instead_of_following_it() {
        let dir = crate::test_util::temp_dir_unique("config-backup-excl");
        std::fs::create_dir_all(&dir).unwrap();
        let victim = dir.join("victim.dat");
        std::fs::write(&victim, b"precious bytes that must survive").unwrap();
        let planted = dir.join("cfg.json.bak.1.abcd");
        std::os::unix::fs::symlink(&victim, &planted).unwrap();

        let res = write_backup_exclusive(&planted, b"backup payload", Some(0o600));
        match &res {
            Err(ConfigError::Io { source, .. }) => assert_eq!(
                source.kind(),
                std::io::ErrorKind::AlreadyExists,
                "a held backup name must be refused, never written through"
            ),
            other => panic!("expected AlreadyExists, got {other:?}"),
        }
        assert!(
            std::fs::symlink_metadata(&planted).is_ok_and(|m| m.file_type().is_symlink()),
            "the planted link itself must be untouched"
        );
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"precious bytes that must survive",
            "following the link would have truncated the referent"
        );

        let fresh = dir.join("cfg.json.bak.2.abcd");
        write_backup_exclusive(&fresh, b"backup payload", Some(0o600)).unwrap();
        let meta = std::fs::symlink_metadata(&fresh).unwrap();
        assert!(meta.is_file(), "the landed backup is its own regular file");
        assert_eq!(std::fs::read(&fresh).unwrap(), b"backup payload");
        drop(std::fs::remove_file(&planted));
        drop(std::fs::remove_file(&fresh));
        drop(std::fs::remove_file(&victim));
        drop(std::fs::remove_dir(&dir));
    }
}
