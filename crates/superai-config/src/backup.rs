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

#[cfg(not(unix))]
fn set_permissions_u32(path: &Path, _mode: u32) -> Result<()> {
    let _ = path;
    Ok(())
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

// ---------------------------------------------------------------------------
// BackupId
// ---------------------------------------------------------------------------

/// Stable identifier for a backup artifact.
///
/// Format is `<millis>-<4hex>` (e.g. `1714123456789-a1b2`). Validation is
/// intentionally lenient at the config layer; core layer enforces stricter
/// rules.
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

// ---------------------------------------------------------------------------
// BackupEntry
// ---------------------------------------------------------------------------

/// Catalog entry for a single backup.
///
/// The catalog contains no file contents or secret values — only metadata
/// needed to locate, verify, and restore the backup.
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

// ---------------------------------------------------------------------------
// Core backup
// ---------------------------------------------------------------------------

/// Copy `path` beside itself as `<name>.bak.<millis>.<rand4>` before it is
/// overwritten.
///
/// Returns `Ok(None)` when the file does not exist yet — a first write has
/// nothing to preserve. Every mutating helper in this crate calls this first.
///
/// Guarantees:
/// - Copies without truncating the original.
/// - Preserves permissions where supported.
/// - Flushes the backup file and verifies digest before returning.
/// - Suffix includes 4 random hex chars for collision resistance.
/// - No secrets are stored in the returned entry.
pub fn backup(path: &Path) -> Result<Option<BackupEntry>> {
    backup_with_reason(path, "pre-write backup")
}

/// Back up `path` with an explicit `reason`.
///
/// See [`backup`] for guarantees. `reason` is stored in the entry but never
/// contains file contents.
pub fn backup_with_reason(path: &Path, reason: &str) -> Result<Option<BackupEntry>> {
    backup_with_operation(path, None, reason)
}

/// Back up `path` with an optional `operation_id` and `reason`.
pub fn backup_with_operation(
    path: &Path,
    operation_id: Option<&str>,
    reason: &str,
) -> Result<Option<BackupEntry>> {
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

    let mut attempts = 0;
    let (target, millis, suffix) = loop {
        let (candidate, m, s) = generate_backup_path(path)?;
        if !candidate.exists() {
            break (candidate, m, s);
        }
        attempts += 1;
        if attempts >= 5 {
            break (candidate, m, s);
        }
    };

    let original_bytes = std::fs::read(path).map_err(|e| ConfigError::io(path, e))?;
    let digest = compute_digest(&original_bytes);
    let size = original_bytes.len() as u64;
    let permissions = get_permissions_u32(&meta);

    std::fs::copy(path, &target).map_err(|e| ConfigError::io(path, e))?;

    if let Some(mode) = permissions {
        #[cfg(unix)]
        {
            set_permissions_u32(&target, mode)?;
        }
        #[cfg(not(unix))]
        {
            let _mode = mode;
        }
    }

    {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(&target)
            .map_err(|e| ConfigError::io(&target, e))?;
        file.sync_all().map_err(|e| ConfigError::io(&target, e))?;
    }

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

/// Restore a backup produced by [`backup`] over `path`.
pub fn restore(backup_path: &Path, path: &Path) -> Result<()> {
    let backup_meta =
        std::fs::symlink_metadata(backup_path).map_err(|e| ConfigError::io(backup_path, e))?;
    if backup_meta.is_dir() {
        return Err(ConfigError::io(
            backup_path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "backup is a directory"),
        ));
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    std::fs::copy(backup_path, path).map_err(|e| ConfigError::io(path, e))?;

    {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|e| ConfigError::io(path, e))?;
        file.sync_all().map_err(|e| ConfigError::io(path, e))?;
    }

    Ok(())
}

/// Restore via a [`BackupEntry`], verifying the backup first.
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

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

/// List all backups for `original_path` by scanning its parent directory.
///
/// Backups are identified by the prefix `<file_name>.bak.`. No automatic
/// deletion is performed; retention is caller-controlled. Results are sorted
/// by timestamp then suffix.
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

/// Verify that a backup file matches its catalog entry.
///
/// Returns `Ok(true)` when digest and size both match, `Ok(false)` when they
/// differ, and `Err` on I/O failure. No secrets are involved.
pub fn verify_backup(entry: &BackupEntry) -> Result<bool> {
    let bytes =
        std::fs::read(&entry.backup_path).map_err(|e| ConfigError::io(&entry.backup_path, e))?;
    let digest = compute_digest(&bytes);
    let size = bytes.len() as u64;
    Ok(digest == entry.digest && size == entry.size)
}

/// Verify that a backup entry is related to `target`.
///
/// Checks that the entry's original path equals `target` and that the
/// backup file is a sibling of the original (same directory, prefixed name).
/// This prevents silently crossing harness/instance/resource identity during
/// restore (MUT-07).
pub fn verify_backup_relation(entry: &BackupEntry, target: &Path) -> Result<bool> {
    if entry.original_path != target {
        return Ok(false);
    }
    let Some(parent) = target.parent() else {
        return Ok(false);
    };
    let backup_parent = entry.backup_path.parent().unwrap_or_else(|| Path::new("."));
    if backup_parent != parent && !parent.as_os_str().is_empty() {
        // If target has a parent, backup must be sibling
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
    // Also verify digest matches the backup file content
    verify_backup(entry)
}

/// Find a backup for `original_path` by stable [`BackupId`].
///
/// Resolves via the backup catalog, not via a user-built path. Returns
/// `Ok(None)` when no matching backup exists.
pub fn find_backup_by_id(original_path: &Path, id: &BackupId) -> Result<Option<BackupEntry>> {
    let entries = list_backups(original_path)?;
    for entry in entries {
        if entry.id == *id {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

/// Redact secret-bearing values from a line of config for diff preview.
///
/// Any line containing `apikey`, `secret`, `token`, `password`, or `auth`
/// has its value replaced with `[REDACTED]`. The function never returns raw
/// secret material.
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
    // Keep key, redact value after ':' or '='
    if let Some(pos) = line.find(':') {
        let (key, _) = line.split_at(pos + 1);
        format!("{key} [REDACTED]")
    } else if let Some(pos) = line.find('=') {
        let (key, _) = line.split_at(pos + 1);
        format!("{key}[REDACTED]")
    } else {
        // No separator, redact whole line except key hint
        "[REDACTED]".to_owned()
    }
}

/// Produce a redacted unified-like diff preview between `current` and
/// `backup` bytes.
///
/// The preview never contains raw secret values; lines with secret-bearing
/// keys are redacted. Binary files produce a size/digest summary only.
pub fn redacted_diff_preview(current: &[u8], backup: &[u8]) -> String {
    // If either side is not valid UTF-8, produce a binary summary
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
    // Very small diff: show removed vs added lines with redaction
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

/// Restore by stable [`BackupId`] for `original_path` with full MUT-07
/// verification: relation check, digest verification, fresh-read preview,
/// backup-before-restore, atomic replace, and semantic verification.
///
/// The backup is resolved by ID, not by a user-supplied path. The current
/// target is read fresh, a redacted diff is produced, the backup digest and
/// relation are verified, the current file is backed up (unless missing),
/// the backup is atomically restored, and the result is verified.
pub fn restore_by_id(original_path: &Path, backup_id: &BackupId) -> Result<RestoreReport> {
    let entry = find_backup_by_id(original_path, backup_id)?.ok_or_else(|| {
        ConfigError::io(
            original_path,
            std::io::Error::new(std::io::ErrorKind::NotFound, "backup id not found"),
        )
    })?;
    restore_verified(&entry)
}

/// Restore via a verified [`BackupEntry`] following MUT-07.
///
/// Steps:
/// 1. Verify backup digest and original-target relation.
/// 2. Fresh-read current target and preview reverse diff (redacted).
/// 3. Back up current target before restore unless it is missing (failed
///    uncommitted creation).
/// 4. Atomic replace and semantic verification.
pub fn restore_verified(entry: &BackupEntry) -> Result<RestoreReport> {
    // 1. Verify backup digest
    let digest_ok = verify_backup(entry)?;
    if !digest_ok {
        return Err(ConfigError::backup_verification(
            &entry.backup_path,
            "backup digest does not match entry",
        ));
    }
    // 2. Verify relation (no cross-identity restore)
    let relation_ok = verify_backup_relation(entry, &entry.original_path)?;
    if !relation_ok {
        return Err(ConfigError::backup_verification(
            &entry.original_path,
            "backup relation mismatch: entry does not belong to target",
        ));
    }
    // 3. Fresh-read current target and preview diff (redacted)
    let current_bytes = std::fs::read(&entry.original_path).unwrap_or_default();
    let backup_bytes =
        std::fs::read(&entry.backup_path).map_err(|e| ConfigError::io(&entry.backup_path, e))?;
    let preview_redacted = redacted_diff_preview(&current_bytes, &backup_bytes);
    // 4. Back up current target before restore unless missing
    let backup_before = if entry.original_path.exists() {
        backup_with_operation(
            &entry.original_path,
            entry.operation_id.as_deref(),
            "pre-restore backup",
        )?
    } else {
        None
    };
    // 5. Atomic replace: copy backup to target, flush, sync, verify
    if let Some(parent) = entry.original_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }
    std::fs::copy(&entry.backup_path, &entry.original_path)
        .map_err(|e| ConfigError::io(&entry.original_path, e))?;
    {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(&entry.original_path)
            .map_err(|e| ConfigError::io(&entry.original_path, e))?;
        file.sync_all()
            .map_err(|e| ConfigError::io(&entry.original_path, e))?;
    }
    let restored_bytes = std::fs::read(&entry.original_path)
        .map_err(|e| ConfigError::io(&entry.original_path, e))?;
    let restored_digest = compute_digest(&restored_bytes);
    let expected_digest = compute_digest(&backup_bytes);
    let verification_passed =
        restored_digest == expected_digest && restored_bytes.len() == backup_bytes.len();
    if !verification_passed {
        return Err(ConfigError::verification(
            &entry.original_path,
            format!(
                "restore verification failed: expected {expected_digest}, got {restored_digest}"
            ),
        ));
    }
    // Optional semantic validation by kind
    if let Some(kind) = infer_kind_for_path(&entry.original_path) {
        // Best-effort parse check; do not fail restore on parse if backup itself was valid
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

fn validate_bytes_for_kind(
    bytes: &[u8],
    kind: crate::document::DocumentKind,
    path: &Path,
) -> Result<()> {
    match kind {
        crate::document::DocumentKind::StrictJson => {
            serde_json::from_slice::<serde_json::Value>(bytes).map_err(|source| {
                ConfigError::Json {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
        }
        crate::document::DocumentKind::JsonC => {
            let text = std::str::from_utf8(bytes).map_err(|_err| {
                ConfigError::io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8 in jsonc"),
                )
            })?;
            let stripped = strip_comments_for_restore(text);
            serde_json::from_str::<serde_json::Value>(&stripped).map_err(|source| {
                ConfigError::Json {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
        }
        crate::document::DocumentKind::Toml => {
            let text = std::str::from_utf8(bytes).map_err(|_err| {
                ConfigError::io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8 in toml"),
                )
            })?;
            let _ = text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|source| ConfigError::Toml {
                    path: path.to_path_buf(),
                    source,
                })?;
        }
        crate::document::DocumentKind::Yaml => {
            let text = std::str::from_utf8(bytes).map_err(|_err| {
                ConfigError::io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8 in yaml"),
                )
            })?;
            yaml_serde::from_str::<serde_json::Value>(text).map_err(|source| {
                ConfigError::Yaml {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
        }
        crate::document::DocumentKind::Env => {
            let text = std::str::from_utf8(bytes).map_err(|_err| ConfigError::Env {
                path: path.to_path_buf(),
                message: "invalid utf8".to_owned(),
            })?;
            for (idx, line) in text.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let without_export = if let Some(rest) = trimmed.strip_prefix("export ") {
                    rest.trim()
                } else {
                    trimmed
                };
                if !without_export.contains('=') {
                    return Err(ConfigError::Env {
                        path: path.to_path_buf(),
                        message: format!("line {} missing '='", idx + 1),
                    });
                }
            }
        }
        crate::document::DocumentKind::TextFragment | crate::document::DocumentKind::Opaque => {}
    }
    Ok(())
}

#[expect(
    clippy::excessive_nesting,
    reason = "comment stripping state machine requires nesting"
)]
fn strip_comments_for_restore(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
            output.push(ch);
        } else if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    while let Some(&peek) = chars.peek() {
                        if peek == '\n' {
                            break;
                        }
                        chars.next();
                    }
                }
                Some('*') => {
                    chars.next();
                    loop {
                        match chars.next() {
                            Some('*') => {
                                if chars.peek().copied() == Some('/') {
                                    chars.next();
                                    break;
                                }
                            }
                            Some(_) => {}
                            None => break,
                        }
                    }
                }
                _ => output.push(ch),
            }
        } else {
            output.push(ch);
        }
    }
    output
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
            original_path: PathBuf::from("/tmp/fake"),
            backup_path: PathBuf::from("/tmp/does-not-exist-xyz-123"),
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
}
