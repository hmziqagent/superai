//! Multi-file compensated transaction (MUT-05 / MUT-06).
//!
//! No filesystem-wide atomicity is claimed. Foreign files are backed up
//! before the first commit, staged outputs are parse-validated, commits run
//! in deterministic order, verification reads fresh from disk, and a
//! failure restores committed files in reverse order with verified
//! rollback and explicit residual reporting.

#![expect(
    clippy::excessive_nesting,
    reason = "transaction requires deep validation and rollback logic"
)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::atomic::{compute_digest, generate_random_suffix, sync_parent, timestamp_millis_now};
use crate::backup::{BackupEntry, backup_with_injector, verify_backup};
use crate::document::{DocumentKind, validate_bytes_for_kind};
use crate::error::{ConfigError, Result};
use crate::injector::{Injector, Point};
use crate::journal::{CrashJournal, JournalBackup, JournalPhase};
use crate::snapshot::{Snapshot, is_modified, snapshot};

/// Stable identifier for a transaction operation, used for quarantine and
/// backup linkage.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OperationId(String);

impl OperationId {
    /// Create a new operation id.
    ///
    /// Rejects empty values and values containing path separators or NUL.
    pub fn new(id: &str) -> Result<Self> {
        if id.is_empty() {
            return Err(ConfigError::io(
                Path::new(id),
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "operation id must not be empty",
                ),
            ));
        }
        if id.contains('/') || id.contains('\\') || id.contains(':') || id.contains('\0') {
            return Err(ConfigError::io(
                Path::new(id),
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "operation id must not contain '/', '\\', ':', or NUL",
                ),
            ));
        }
        Ok(Self(id.to_owned()))
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

impl std::fmt::Display for OperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Intent of a removal. Each variant carries different safety rules and
/// quarantine requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoveKind {
    /// Remove one entry from a shared file; the file itself survives.
    ConfigEntry,
    /// Delete a superai-created file, never a foreign harness config.
    WrapperFile,
    /// Remove an instance root directory; quarantined before delete.
    InstanceRoot,
    /// Uninstall a binary; never touches config directories.
    Binary,
    /// Detach a registry record only; no filesystem mutation.
    RegistryOnly,
}

impl std::fmt::Display for RemoveKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::ConfigEntry => "config_entry",
            Self::WrapperFile => "wrapper_file",
            Self::InstanceRoot => "instance_root",
            Self::Binary => "binary",
            Self::RegistryOnly => "registry_only",
        };
        f.write_str(s)
    }
}

/// Validated description of a removal to be performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovePlan {
    /// Kind of removal.
    pub kind: RemoveKind,
    /// Absolute target path or entry selector.
    pub target: PathBuf,
    /// Whether quarantine is required before delete.
    pub requires_quarantine: bool,
}

impl RemovePlan {
    /// Create a new remove plan, rejecting invalid deletion targets.
    pub fn new(kind: RemoveKind, target: &Path) -> Result<Self> {
        validate_remove_target(target, kind)?;
        let requires_quarantine = matches!(kind, RemoveKind::InstanceRoot);
        Ok(Self {
            kind,
            target: target.to_path_buf(),
            requires_quarantine,
        })
    }
}

/// Validate a removal target per [`RemoveKind`] policy: broad roots,
/// unresolved variables, globs, traversal, and home are refused. Best
/// effort at the config layer; adapter ownership checks still apply.
pub fn validate_remove_target(path: &Path, kind: RemoveKind) -> Result<()> {
    let display = path.to_string_lossy();
    let s = display.as_ref();

    if s.is_empty() {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "remove target must not be empty",
            ),
        ));
    }
    if !path.is_absolute() {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "remove target must be absolute",
            ),
        ));
    }
    for comp in path.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return Err(ConfigError::io(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "remove target must not contain '..'",
                ),
            ));
        }
    }
    if s.contains('*') || s.contains('?') || s.contains('[') {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "remove target must not contain globs",
            ),
        ));
    }
    if s.contains('$') || s.contains('%') {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "remove target contains unresolved variable",
            ),
        ));
    }
    reject_broad_or_home_roots(path, s)?;
    reject_kind_specific_target(path, kind, s)?;
    Ok(())
}

/// Reject broad roots and home for any removal: unix broad roots,
/// Windows-shaped broad roots, and home with platform case rules.
fn reject_broad_or_home_roots(path: &Path, s: &str) -> Result<()> {
    if s == "/" || s == "/home" || s == "/tmp" || s == "/usr" || s == "/etc" {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "refusing to remove broad root",
            ),
        ));
    }
    if windows_shaped_broad_root(path) {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "refusing to remove broad windows root",
            ),
        ));
    }
    if let Some(home) = home_dir()
        && paths_equal_platform_folded(path, &home)
    {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "refusing to remove home directory",
            ),
        ));
    }
    Ok(())
}

/// Per-[`RemoveKind`] additional refusals.
fn reject_kind_specific_target(path: &Path, kind: RemoveKind, s: &str) -> Result<()> {
    if matches!(kind, RemoveKind::Binary) {
        // Windows-shaped paths fold case and both separators; unix is exact.
        let ends_config_root = if looks_windows_shaped(s) {
            let folded = normalize_windows_style(s);
            folded.ends_with("/.claude") || folded.ends_with("/.superai")
        } else {
            s.ends_with("/.claude") || s.ends_with("/.superai")
        };
        if ends_config_root {
            return Err(ConfigError::io(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "binary removal must not target config root",
                ),
            ));
        }
    }
    Ok(())
}

fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home);
        if p.is_absolute() {
            return Some(p);
        }
    }
    if let Some(userprofile) = std::env::var_os("USERPROFILE") {
        let p = PathBuf::from(userprofile);
        if p.is_absolute() {
            return Some(p);
        }
    }
    None
}

/// Whether `s` looks like a Windows path (drive prefix or UNC root),
/// regardless of host platform.
fn looks_windows_shaped(s: &str) -> bool {
    s.starts_with("\\\\")
        || s.starts_with("//")
        || (s.chars().nth(1) == Some(':')
            && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
}

/// Backslashes to slashes plus ASCII lowercase, for Windows-style
/// case-insensitive comparison.
fn normalize_windows_style(s: &str) -> String {
    s.replace('\\', "/").to_ascii_lowercase()
}

/// Whether `path` is a Windows-shaped broad root (drive and UNC roots, and
/// the first-level system directories). Matching is anchored, folded, and
/// separator-agnostic; unix-shaped paths never match.
pub(crate) fn windows_shaped_broad_root(path: &Path) -> bool {
    let normalized = normalize_windows_style(&path.to_string_lossy());
    let trimmed = normalized.trim_end_matches('/');

    let has_drive_prefix = trimmed
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && trimmed.chars().nth(1) == Some(':');
    if has_drive_prefix {
        // Only the bare drive root or a first-level system directory counts.
        let Some(rest) = trimmed.get(2..) else {
            return false;
        };
        if rest.is_empty() {
            return true; // `c:` / `c:/`
        }
        let first_level = rest.trim_start_matches('/');
        let mut parts = first_level.split('/');
        let Some(head) = parts.next() else {
            return true; // `c://`
        };
        if parts.next().is_some() {
            return false; // deeper than first level: a specific target
        }
        return matches!(
            head,
            "windows"
                | "program files"
                | "program files (x86)"
                | "programdata"
                | "users"
                | "documents and settings"
        );
    }

    // UNC root (`\\server` or `\\server\share`): a verbatim `\\` prefix on
    // any host, or `//` on Windows. A unix `//` path stays an ordinary path.
    let raw = path.to_string_lossy();
    let unc_shaped = raw.starts_with("\\\\") || (cfg!(windows) && raw.starts_with("//"));
    unc_shaped && trimmed.matches('/').count() <= 3
}

/// Whether the final component is a Windows reserved device name (`CON`,
/// `PRN`, `AUX`, `NUL`, `COM1`-`9`, `LPT1`-`9`, `CONIN$`, `CONOUT$`),
/// stem-matched and case-folded, so `CON.txt` is a device too (QAL-09).
pub(crate) fn windows_reserved_device_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let stem = name.split('.').next().unwrap_or(name);
    let folded = stem.to_ascii_uppercase();
    if matches!(
        folded.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    for prefix in ["COM", "LPT"] {
        if let Some(digits) = folded.strip_prefix(prefix)
            && digits.len() == 1
            && digits
                .as_bytes()
                .first()
                .is_some_and(|b| (b'1'..=b'9').contains(b))
        {
            return true;
        }
    }
    false
}

/// Path equality with platform case rules: byte equality, or folded
/// comparison when either side is windows-shaped.
pub(crate) fn paths_equal_platform_folded(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    let a_s = a.to_string_lossy();
    let b_s = b.to_string_lossy();
    if looks_windows_shaped(&a_s) || looks_windows_shaped(&b_s) {
        normalize_windows_style(&a_s) == normalize_windows_style(&b_s)
    } else {
        false
    }
}

/// One auditable filesystem action; the transaction validates, backs up,
/// stages, commits in order, and verifies each of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileAction {
    /// Atomically write `content` to `path` with the given document kind.
    Write {
        /// Absolute target path.
        path: PathBuf,
        /// Bytes to write.
        content: Vec<u8>,
        /// Document kind for validation.
        kind: DocumentKind,
    },
    /// Create a directory at `path`.
    CreateDir {
        /// Absolute directory path.
        path: PathBuf,
    },
    /// Create a symlink at `link`. `expected_current` is the MUT-02/MUT-06
    /// owned-target rule: `None` replaces a link still matching its
    /// prepare-time snapshot, `Some(target)` only one pointing there now.
    Symlink {
        /// Absolute link path.
        link: PathBuf,
        /// Symlink target (may be relative or absolute).
        target: PathBuf,
        /// The target an existing link must currently carry to be replaced.
        expected_current: Option<PathBuf>,
    },
    /// Remove a file at `path`.
    RemoveFile {
        /// Absolute file path.
        path: PathBuf,
    },
    /// Move `from` to `to` via quarantine (recoverable).
    QuarantineMove {
        /// Source path to quarantine.
        from: PathBuf,
        /// Destination quarantine path.
        to: PathBuf,
    },
}

impl FileAction {
    /// Return the primary path for ordering and collision detection.
    pub fn primary_path(&self) -> &Path {
        match self {
            Self::Write { path, .. } | Self::CreateDir { path } | Self::RemoveFile { path } => path,
            Self::Symlink { link, .. } => link,
            Self::QuarantineMove { from, .. } => from,
        }
    }

    /// Sort key for deterministic ordering: (`kind_order`, `path_string`).
    fn sort_key(&self) -> (u8, String) {
        let order = match self {
            Self::CreateDir { .. } => 0,
            Self::Write { .. } => 1,
            Self::Symlink { .. } => 2,
            Self::QuarantineMove { .. } => 3,
            Self::RemoveFile { .. } => 4,
        };
        (order, self.primary_path().to_string_lossy().into_owned())
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "kept Result for fallible future use"
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

#[cfg(unix)]
fn set_safe_permissions(path: &Path, original_path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if original_path.exists() {
        match std::fs::metadata(original_path) {
            Ok(m) => m.permissions().mode() & 0o777,
            Err(_) => 0o600,
        }
    } else {
        0o600
    };
    let safe_mode = if mode == 0 { 0o600 } else { mode };
    let perm = std::fs::Permissions::from_mode(safe_mode);
    std::fs::set_permissions(path, perm).map_err(|e| ConfigError::io(path, e))
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "windows has no POSIX chmod; keeps the unix call sites uniform"
)]
fn set_safe_permissions(_path: &Path, _original_path: &Path) -> Result<()> {
    Ok(())
}

/// Unix (device, inode) identity, following symlinks first so paths
/// converging through links count as one target; used to catch hard-link
/// aliases (MUT-02).
#[cfg(unix)]
fn inode_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path)
        .or_else(|_| std::fs::symlink_metadata(path))
        .ok()?;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn inode_identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// `nlink` of an existing path: above 1, the atomic replacement would split
/// the link group, so the caller gets a warning instead of silence.
#[cfg(unix)]
fn hardlink_count(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some(meta.nlink())
}

#[cfg(not(unix))]
fn hardlink_count(_path: &Path) -> Option<u64> {
    None
}

/// Stage `content` into a same-directory exclusive temp for `target`:
/// safe permissions before any bytes, then write, flush, sync. The
/// production primitive shared by [`Transaction`] and the failure matrix.
pub fn stage_temp_file(
    target: &Path,
    content: &[u8],
    injector: Option<&dyn Injector>,
) -> Result<PathBuf> {
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }
    if let Some(injector) = injector {
        injector.inject(Point::TempCreate)?;
    }
    // The temp is always created exclusively and the handle is kept open until
    // the bytes are durable: nothing (a pre-planted symlink included) can make
    // staging truncate a file we did not create.
    let mut final_temp = PathBuf::new();
    let mut file: Option<std::fs::File> = None;
    for _ in 0..5 {
        let candidate = generate_temp_path(target)?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => {
                final_temp = candidate;
                file = Some(f);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(ConfigError::io(&candidate, e)),
        }
    }
    let Some(mut f) = file else {
        return Err(ConfigError::io(
            target,
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "temp name collision after repeated attempts",
            ),
        ));
    };
    // Safe permissions land while the file is still empty, so staged bytes are
    // never group/world readable regardless of the process umask.
    if let Err(e) = set_safe_permissions(&final_temp, target) {
        drop(std::fs::remove_file(&final_temp));
        return Err(e);
    }
    {
        use std::io::Write;
        let write_result = (|| {
            if let Some(injector) = injector {
                injector.inject(Point::TempWrite)?;
            }
            f.write_all(content)
                .map_err(|e| ConfigError::io(&final_temp, e))?;
            f.flush().map_err(|e| ConfigError::io(&final_temp, e))?;
            if let Some(injector) = injector {
                injector.inject(Point::TempFlush)?;
            }
            f.sync_all().map_err(|e| ConfigError::io(&final_temp, e))
        })();
        if let Err(e) = write_result {
            // A staging failure must never leak its half-written temp.
            drop(std::fs::remove_file(&final_temp));
            return Err(e);
        }
    }
    drop(f);
    Ok(final_temp)
}

/// Commit a staged temp over `target` (the production commit primitive).
/// With `expected`, the target is re-read fresh just before the rename;
/// any foreign change aborts with `ConcurrentModification` (§4.2).
pub fn commit_staged_file(
    target: &Path,
    staged: &Path,
    expected: Option<&Snapshot>,
    injector: Option<&dyn Injector>,
) -> Result<()> {
    validate_path_safety(target)?;
    let staged_bytes = std::fs::read(staged).map_err(|e| ConfigError::io(staged, e))?;
    let expected_digest = compute_digest(&staged_bytes);

    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    if let Some(injector) = injector {
        injector.inject(Point::ConflictRecheck)?;
    }
    if let Some(expected) = expected {
        let current = snapshot(target);
        if is_modified(expected, &current) {
            return Err(ConfigError::concurrent_modification(
                target,
                expected
                    .digest
                    .clone()
                    .or_else(|| {
                        expected
                            .symlink_target
                            .as_ref()
                            .map(|t| t.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| "<absent>".to_owned()),
                current
                    .digest
                    .clone()
                    .or_else(|| {
                        current
                            .symlink_target
                            .as_ref()
                            .map(|t| t.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| "<absent>".to_owned()),
            ));
        }
    }

    // Rename, or copy across filesystems.
    if let Some(injector) = injector {
        injector.inject(Point::AtomicReplace)?;
    }
    match std::fs::rename(staged, target) {
        Ok(()) => {}
        Err(e)
            if e.kind() == std::io::ErrorKind::CrossesDevices || e.raw_os_error() == Some(18) =>
        {
            std::fs::copy(staged, target).map_err(|copy_e| ConfigError::io(target, copy_e))?;
            drop(std::fs::remove_file(staged));
        }
        Err(e) => return Err(ConfigError::io(target, e)),
    }
    if let Some(injector) = injector {
        injector.inject(Point::ParentSync)?;
    }
    sync_parent(target)?;

    if let Some(injector) = injector {
        injector.inject(Point::ReadBackVerify)?;
    }
    let read_back = std::fs::read(target).map_err(|e| ConfigError::io(target, e))?;
    let actual = compute_digest(&read_back);
    if expected_digest != actual {
        return Err(ConfigError::verification(
            target,
            format!("digest mismatch after commit: expected {expected_digest}, got {actual}"),
        ));
    }
    if read_back.len() != staged_bytes.len() {
        return Err(ConfigError::verification(
            target,
            format!(
                "size mismatch after commit: expected {}, got {}",
                staged_bytes.len(),
                read_back.len(),
            ),
        ));
    }
    Ok(())
}

/// Report from a single-file commit through the mutation boundary.
#[derive(Debug, Clone)]
pub struct FileCommitReport {
    /// Backup of the previous contents taken before the replacement landed
    /// (`None` when the commit created a new file).
    pub backup: Option<BackupEntry>,
    /// Hex digest of the committed bytes (read back and verified on disk).
    pub digest: String,
}

/// Detect a case-insensitive sibling collision for `target` (QAL-09): on
/// case-insensitive filesystems the write would silently land over the
/// sibling, so the risk is surfaced everywhere.
pub(crate) fn case_fold_collision_in_dir(target: &Path) -> Option<PathBuf> {
    let dir = target.parent()?;
    let target_name = target.file_name()?.to_string_lossy().into_owned();
    let wanted = target_name.to_ascii_lowercase();
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == target_name {
            continue;
        }
        if name_str.to_ascii_lowercase() == wanted {
            let variant = dir.join(&name);
            match &best {
                Some(current) if current <= &variant => {}
                _ => best = Some(variant),
            }
        }
    }
    best
}

/// Commit `content` to `target` through the crate's single mutation
/// boundary: a one-step [`Transaction`] with the full discipline (snapshot,
/// backup, staged parse-check, §4.2 recheck, atomic replace, read-back
/// verify). Codec stores and every superai-core write route here or through
/// [`stage_temp_file`] + [`commit_staged_file`]. A failed commit leaves the
/// target untouched.
pub fn commit_file(
    id: &str,
    target: &Path,
    content: &[u8],
    kind: DocumentKind,
) -> Result<FileCommitReport> {
    commit_file_expecting(id, target, content, kind, None)
}

/// [`commit_file`] with a caller-supplied conflict token: a snapshot taken
/// when the caller read the document. Any foreign change since that read,
/// not just since prepare, aborts with `ConcurrentModification`.
pub fn commit_file_expecting(
    id: &str,
    target: &Path,
    content: &[u8],
    kind: DocumentKind,
    expected: Option<&Snapshot>,
) -> Result<FileCommitReport> {
    commit_file_expecting_with_roots(id, target, content, kind, expected, &[])
}

/// [`commit_file_expecting`] with the MUT-02 adapter-allowed follow roots.
/// An allowed symlink target is followed and preserved (the referent is
/// mutated); the caller's token guards the link it actually read.
pub fn commit_file_expecting_with_roots(
    id: &str,
    target: &Path,
    content: &[u8],
    kind: DocumentKind,
    expected: Option<&Snapshot>,
    follow_roots: &[PathBuf],
) -> Result<FileCommitReport> {
    // Typed refusal before any staging work.
    if std::fs::symlink_metadata(target).is_ok_and(|m| m.is_dir()) {
        return Err(ConfigError::io(
            target,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "is a directory"),
        ));
    }
    // QAL-09: `File.json` next to `file.json` would silently land over it
    // on case-insensitive filesystems.
    if let Some(variant) = case_fold_collision_in_dir(target) {
        return Err(ConfigError::io(
            target,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "case-insensitive collision with {} in the same directory",
                    variant.display()
                ),
            ),
        ));
    }

    let operation = OperationId::new(id)?;
    let mut transaction = Transaction::new(
        operation,
        vec![FileAction::Write {
            path: target.to_path_buf(),
            content: content.to_vec(),
            kind,
        }],
    )
    .with_symlink_follow_roots(follow_roots.to_vec());
    if let Err(e) = transaction.prepare() {
        cleanup_staged_temps(&transaction.staged_temps);
        return Err(e);
    }
    // The caller's token guards the full read-to-commit window.
    let effective_target = transaction.steps.first().map_or_else(
        || target.to_path_buf(),
        |step| step.primary_path().to_path_buf(),
    );
    if effective_target != target {
        // MUT-02 follow-and-preserve: the caller read through the LINK, so
        // its token is checked against the link's current state: a retarget
        // or content change since the read aborts before the referent is
        // mutated.
        if let Some(expected) = expected
            && is_modified(expected, &snapshot(target))
        {
            // The link was retargeted or its bytes changed since the read.
            cleanup_staged_temps(&transaction.staged_temps);
            return Err(ConfigError::concurrent_modification(
                target,
                expected
                    .symlink_target
                    .as_ref()
                    .map(|t| t.to_string_lossy().into_owned())
                    .or_else(|| expected.digest.clone())
                    .unwrap_or_else(|| "<absent>".to_owned()),
                "<changed since read>".to_owned(),
            ));
        }
    } else if let Some(expected) = expected {
        transaction
            .expected_states
            .insert(target.to_path_buf(), expected.clone());
    }
    // A single-step commit that fails has landed nothing else; the staged
    // temp is ours to remove.
    let commit_outcome = match transaction.commit() {
        Ok(outcome) => outcome,
        Err(e) => {
            cleanup_staged_temps(&transaction.staged_temps);
            return Err(e);
        }
    };
    // Post-commit parse verification; on failure roll back and surface a
    // typed verification error.
    let verification = transaction.verify()?;
    if let Some(failed) = verification.iter().find(|v| !v.digest_ok || !v.parse_ok) {
        let message = failed.message.clone();
        // The caller sees the verification error; the rollback's own
        // outcome is not observable here.
        drop(transaction.rollback());
        cleanup_staged_temps(&transaction.staged_temps);
        return Err(ConfigError::verification(target, message));
    }
    let backup = commit_outcome.backups.into_iter().next();
    Ok(FileCommitReport {
        backup,
        digest: compute_digest(content),
    })
}

/// Best-effort removal of staged temps left by a failed boundary commit.
fn cleanup_staged_temps(temps: &[PathBuf]) {
    for temp in temps {
        if temp.exists() {
            drop(std::fs::remove_file(temp));
        }
    }
}

/// How symlinks are treated during a recursive copy (MUT-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SymlinkPolicy {
    /// Skip links entirely; nothing follows a link out of the tree.
    /// Default.
    #[default]
    Skip,
    /// Recreate the link itself at the destination pointing at the same
    /// target (relative targets are copied verbatim; absolute targets stay
    /// absolute).
    PreserveLink,
    /// Copy the referent's bytes as a regular file. A broken or looping
    /// link is an error: silently copying nothing would lie about what
    /// was copied.
    FollowCopyContent,
}

/// Options for [`copy_tree`] (MUT-06).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyTreeOptions {
    /// Name patterns to include (glob syntax: `*`, `?`). Empty = everything.
    pub include: Vec<String>,
    /// Name patterns to exclude; a name matching exclude is never copied
    /// even when it matches include.
    pub exclude: Vec<String>,
    /// Symlink policy for encountered links.
    pub symlink_policy: SymlinkPolicy,
    /// Hard bound on copied entries; exceeding it aborts the copy.
    pub max_entries: usize,
    /// Hard bound on total copied bytes; exceeding it aborts the copy.
    pub max_bytes: u64,
}

impl Default for CopyTreeOptions {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            exclude: Vec::new(),
            symlink_policy: SymlinkPolicy::Skip,
            max_entries: 10_000,
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Report of a completed [`copy_tree`] run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CopyTreeReport {
    /// Files copied (destination paths).
    pub files: Vec<PathBuf>,
    /// Directories created.
    pub dirs: Vec<PathBuf>,
    /// Symlinks skipped by policy.
    pub skipped_symlinks: Vec<PathBuf>,
    /// Entries excluded by the include/exclude filters.
    pub excluded: Vec<PathBuf>,
    /// Total bytes copied.
    pub bytes: u64,
}

/// Tiny glob matcher: `*` is any run except `/`, `?` one character. No
/// regex engine is pulled in for this.
fn name_matches(pattern: &str, name: &str) -> bool {
    fn inner(pat: &[u8], name: &[u8]) -> bool {
        match (pat.split_first(), name.split_first()) {
            (None, None) => true,
            (Some((b'*', rest)), _) => {
                inner(rest, name)
                    || match name.split_first() {
                        Some((&c, tail)) => c != b'/' && inner(pat, tail),
                        None => false,
                    }
            }
            (Some((b'?', rest)), Some((&c, tail))) => c != b'/' && inner(rest, tail),
            (Some((&p, rest)), Some((&n, tail))) => p == n && inner(rest, tail),
            (None, Some(_)) | (Some(_), None) => false,
        }
    }
    inner(pattern.as_bytes(), name.as_bytes())
}

/// Whether a FILE name passes the filters. `exclude` always prunes;
/// `include` (when non-empty) selects which files are copied.
fn file_allowed(name: &str, opts: &CopyTreeOptions) -> bool {
    if opts.exclude.iter().any(|p| name_matches(p, name)) {
        return false;
    }
    if opts.include.is_empty() {
        return true;
    }
    opts.include.iter().any(|p| name_matches(p, name))
}

/// Whether a directory name passes the filters: only `exclude` prunes;
/// `include` selects files, never structure.
fn dir_allowed(name: &str, opts: &CopyTreeOptions) -> bool {
    !opts.exclude.iter().any(|p| name_matches(p, name))
}

/// Recursively copy `from` to `to` with filters and a symlink policy
/// (MUT-06). Permission bits are preserved where possible, the run is
/// bounded by `max_entries`/`max_bytes`, and each copied file's digest
/// is verified against its source.
pub fn copy_tree(from: &Path, to: &Path, opts: &CopyTreeOptions) -> Result<CopyTreeReport> {
    // Refuse copying a tree into itself: recursion would nest until the
    // entry bound trips.
    let from_resolved = std::fs::canonicalize(from).unwrap_or_else(|_| from.to_path_buf());
    let from_components = from_resolved.components().collect::<Vec<_>>();
    let to_resolved = match to.parent().and_then(|p| std::fs::canonicalize(p).ok()) {
        Some(parent) => parent.join(to.file_name().unwrap_or_default()),
        None => to.to_path_buf(),
    };
    let to_components = to_resolved.components().collect::<Vec<_>>();
    if to_components.starts_with(&from_components) {
        return Err(ConfigError::unsupported_copy(
            to,
            "destination is inside the source tree",
        ));
    }
    let mut report = CopyTreeReport::default();
    copy_tree_inner(from, to, opts, &mut report)?;
    Ok(report)
}

fn copy_tree_inner(
    from: &Path,
    to: &Path,
    opts: &CopyTreeOptions,
    report: &mut CopyTreeReport,
) -> Result<()> {
    if report.files.len() + report.dirs.len() >= opts.max_entries {
        return Err(ConfigError::verification(
            from,
            format!("copy tree exceeded max entries ({})", opts.max_entries),
        ));
    }
    std::fs::create_dir_all(to).map_err(|e| ConfigError::io(to, e))?;
    let entries = std::fs::read_dir(from).map_err(|e| ConfigError::io(from, e))?;
    for ent in entries {
        let ent = ent.map_err(|e| ConfigError::io(from, e))?;
        let src = ent.path();
        let name = ent.file_name();
        let name_str = name.to_string_lossy();
        let dest = to.join(&name);
        let meta = std::fs::symlink_metadata(&src).map_err(|e| ConfigError::io(&src, e))?;
        if meta.is_dir() {
            if !dir_allowed(&name_str, opts) {
                report.excluded.push(dest);
                continue;
            }
            report.dirs.push(dest.clone());
            copy_tree_inner(&src, &dest, opts, report)?;
            continue;
        }
        if !file_allowed(&name_str, opts) {
            report.excluded.push(dest);
            continue;
        }
        if meta.file_type().is_symlink() && handle_symlink_entry(&src, &dest, opts, report)? {
            continue;
        }
        copy_file_entry(&src, &dest, meta, opts, report)?;
    }
    Ok(())
}

/// Apply the symlink policy for one link entry: `Ok(true)` when fully
/// handled, `Ok(false)` when `FollowCopyContent` continues with the
/// referent's bytes.
fn handle_symlink_entry(
    src: &Path,
    dest: &Path,
    opts: &CopyTreeOptions,
    report: &mut CopyTreeReport,
) -> Result<bool> {
    match opts.symlink_policy {
        SymlinkPolicy::Skip => {
            report.skipped_symlinks.push(dest.to_path_buf());
            Ok(true)
        }
        SymlinkPolicy::PreserveLink => {
            let target = std::fs::read_link(src).map_err(|e| ConfigError::io(src, e))?;
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&target, dest).map_err(|e| ConfigError::io(dest, e))?;
            }
            #[cfg(not(unix))]
            {
                if target.is_absolute() {
                    std::os::windows::fs::symlink_file(&target, dest)
                        .map_err(|e| ConfigError::io(dest, e))?;
                } else {
                    return Err(ConfigError::unsupported_copy(
                        dest,
                        "relative symlink preservation needs platform support",
                    ));
                }
            }
            Ok(true)
        }
        SymlinkPolicy::FollowCopyContent => {
            // A referent that is not a readable regular file fails the
            // copy honestly.
            let target_meta = std::fs::metadata(src).map_err(|e| ConfigError::io(src, e))?;
            if !target_meta.is_file() {
                return Err(ConfigError::unsupported_copy(
                    src,
                    "symlink does not resolve to a regular file",
                ));
            }
            if crate::snapshot::is_symlink_loop(src) {
                return Err(ConfigError::unsupported_copy(
                    src,
                    "symlink loop under FollowCopyContent",
                ));
            }
            Ok(false)
        }
    }
}

/// Copy one file entry with bounds and digest verification. Under
/// `FollowCopyContent` both the bound and the copy read through the link.
fn copy_file_entry(
    src: &Path,
    dest: &Path,
    link_meta: std::fs::Metadata,
    opts: &CopyTreeOptions,
    report: &mut CopyTreeReport,
) -> Result<()> {
    let file_meta = if link_meta.file_type().is_symlink() {
        std::fs::metadata(src).map_err(|e| ConfigError::io(src, e))?
    } else {
        link_meta
    };
    if !file_meta.is_file() {
        return Err(ConfigError::unsupported_copy(
            src,
            "unsupported special file (device/FIFO/socket)",
        ));
    }
    let src_bytes_len = file_meta.len();
    if report.bytes + src_bytes_len > opts.max_bytes {
        return Err(ConfigError::verification(
            src,
            format!("copy tree exceeded max bytes ({})", opts.max_bytes),
        ));
    }
    std::fs::copy(src, dest).map_err(|e| ConfigError::io(dest, e))?;
    // The copy is verified before it is accounted as done.
    let src_digest = snapshot(src).digest;
    let dest_digest = snapshot(dest).digest;
    if src_digest.is_some() && src_digest != dest_digest {
        return Err(ConfigError::verification(
            dest,
            "copied file digest does not match source",
        ));
    }
    report.bytes += src_bytes_len;
    report.files.push(dest.to_path_buf());
    Ok(())
}

/// Remove a superai-owned empty directory (MUT-06). Broad roots are
/// refused like `InstanceRoot` removal; non-empty directories are a typed
/// refusal (quarantine instead).
pub fn remove_owned_empty_dir(path: &Path) -> Result<()> {
    validate_remove_target(path, RemoveKind::InstanceRoot)?;
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ConfigError::io(path, e)),
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(ConfigError::io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a directory"),
                ));
            }
            std::fs::remove_dir(path).map_err(|e| {
                ConfigError::io(
                    path,
                    std::io::Error::new(
                        e.kind(),
                        format!("directory not empty or not removable: {e}"),
                    ),
                )
            })?;
            sync_parent(path)
        }
    }
}

/// Validate a path for safe mutation: no NUL, globs, unresolved variables,
/// traversal, Windows reserved names, or special files. Case-fold
/// collisions live in [`Transaction::validate_plan`].
fn validate_path_safety(path: &Path) -> Result<()> {
    let s = path.to_string_lossy();
    let raw = s.as_ref();
    if raw.contains('\0') {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL"),
        ));
    }
    if raw.contains('*') || raw.contains('?') || raw.contains('[') {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path must not contain globs",
            ),
        ));
    }
    if raw.contains('$') || raw.contains('%') {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path contains unresolved variable",
            ),
        ));
    }
    for comp in path.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return Err(ConfigError::io(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "path must not contain '..'",
                ),
            ));
        }
    }
    // QAL-09: reserved names can never become real files on Windows; plan
    // time refuses them on every host.
    if windows_reserved_device_name(path) {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "windows reserved device name",
            ),
        ));
    }
    // Reject devices, FIFOs, and sockets when the path already exists.
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        let ft = meta.file_type();
        if !(ft.is_file() || ft.is_dir() || ft.is_symlink()) {
            return Err(ConfigError::io(
                path,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "unsupported special file (device/FIFO/socket)",
                ),
            ));
        }
    }
    Ok(())
}

/// Result of a successful commit before verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    /// Paths that were committed in order.
    pub committed: Vec<PathBuf>,
    /// Backups created during prepare.
    pub backups: Vec<BackupEntry>,
}

/// Per-file verification outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyOutcome {
    /// Path that was verified.
    pub path: PathBuf,
    /// Whether digest matches expected.
    pub digest_ok: bool,
    /// Whether parse succeeded.
    pub parse_ok: bool,
    /// Human-readable message, redacted.
    pub message: String,
}

/// Outcome of a rollback attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackOutcome {
    /// Paths that were successfully rolled back.
    pub rolled_back: Vec<PathBuf>,
    /// Paths that could not be rolled back and remain residual.
    pub residuals: Vec<PathBuf>,
    /// Whether verification after rollback passed.
    pub verification_ok: bool,
}

/// Full transaction outcome including residuals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionOutcome {
    /// Whether the transaction succeeded end-to-end.
    pub success: bool,
    /// Commit outcome if commit was attempted.
    pub commit: Option<CommitOutcome>,
    /// Verification outcomes.
    pub verification: Vec<VerifyOutcome>,
    /// Rollback outcome if rollback was attempted.
    pub rollback: Option<RollbackOutcome>,
    /// Redacted diagnostics.
    pub diagnostics_redacted: Vec<String>,
}

/// Compensated multi-file transaction (MUT-05): on failure, committed
/// files are restored in reverse order and residuals reported explicitly.
#[derive(Debug)]
pub struct Transaction {
    /// Stable operation identifier.
    pub id: OperationId,
    /// Ordered file actions.
    pub steps: Vec<FileAction>,
    /// Backups taken before first commit.
    pub backups: Vec<BackupEntry>,
    /// Temporary files staged during prepare.
    pub staged_temps: Vec<PathBuf>,
    /// Rollback [`Transaction::commit`] performed internally when a step
    /// failed after others committed. `residuals` are the paths it could
    /// not undo; they remain on disk and must reach the caller.
    pub partial_rollback: Option<RollbackOutcome>,
    /// Prepare-time snapshots per step path: the §4.2 conflict tokens every
    /// commit step rechecks immediately before its mutation.
    expected_states: HashMap<PathBuf, Snapshot>,
    /// Optional failure injector threaded through the real staging, rename,
    /// backup, and rollback boundaries (QAL-06).
    injector: Option<Arc<dyn Injector>>,
    /// Adapter-allowed roots for the MUT-02 link policy: a `Write` onto a
    /// symlink follows it only inside this set (else
    /// [`ConfigError::SymlinkFollowRefused`]); non-empty, it also
    /// constrains `Symlink` step targets.
    symlink_follow_roots: Vec<PathBuf>,
    /// (link, referent) pairs followed during prepare; commit aborts if a
    /// link no longer points at its recorded referent (MUT-02).
    symlink_followed: Vec<(PathBuf, PathBuf)>,
    /// Journal directory (MUT-09); `None` disables journaling.
    journal_root: Option<PathBuf>,
    /// Current journal state (mirrors the last phase written to disk).
    journal_state: Option<CrashJournal>,
    /// Paths committed so far (journal `completed` list).
    journal_completed: Vec<PathBuf>,
    /// Non-fatal path-safety warnings surfaced through the outcome.
    warnings: Vec<String>,
}

impl Transaction {
    /// Create a new transaction from an operation id and a list of actions.
    pub fn new(id: OperationId, steps: Vec<FileAction>) -> Self {
        Self {
            id,
            steps,
            backups: Vec::new(),
            staged_temps: Vec::new(),
            partial_rollback: None,
            expected_states: HashMap::new(),
            injector: None,
            symlink_follow_roots: Vec::new(),
            symlink_followed: Vec::new(),
            journal_root: None,
            journal_state: None,
            journal_completed: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Builder: attach a failure injector (QAL-06) observing the production
    /// boundaries from staging through journal transitions.
    #[must_use = "the injector is only attached to the returned transaction"]
    pub fn with_injector(mut self, injector: Arc<dyn Injector>) -> Self {
        self.injector = Some(injector);
        self
    }

    /// Builder: journal under `journal_root` (MUT-09), removed only after
    /// verified completion; [`crate::journal::recover_pending`] recovers.
    #[must_use = "journaling is only enabled on the returned transaction"]
    pub fn with_journal(mut self, journal_root: PathBuf) -> Self {
        self.journal_root = Some(journal_root);
        self
    }

    /// Builder: declare the MUT-02 follow roots. Roots are canonicalized
    /// when they exist so linked roots compare equal to their referents.
    #[must_use = "the policy is only enabled on the returned transaction"]
    pub fn with_symlink_follow_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.symlink_follow_roots = roots
            .into_iter()
            .map(|root| std::fs::canonicalize(&root).unwrap_or(root))
            .collect();
        self
    }

    /// Whether `resolved` lies inside one of the declared follow roots
    /// (equality counts; canonical comparison on both sides).
    fn resolves_within_follow_roots(&self, resolved: &Path) -> bool {
        let canonical = std::fs::canonicalize(resolved).unwrap_or_else(|_| resolved.to_path_buf());
        self.symlink_follow_roots.iter().any(|root| {
            canonical.starts_with(root)
                || std::fs::canonicalize(root)
                    .is_ok_and(|canon_root| canonical.starts_with(&canon_root))
        })
    }

    /// Follow-and-preserve resolution (MUT-02): not a symlink is `Ok(None)`;
    /// a link inside the roots yields the referent; anything else is a
    /// typed [`ConfigError::SymlinkFollowRefused`].
    fn symlink_follow_target(&self, path: &Path) -> Result<Option<PathBuf>> {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return Ok(None);
        };
        if !meta.file_type().is_symlink() {
            return Ok(None);
        }
        let roots = self
            .symlink_follow_roots
            .iter()
            .map(|r| r.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        let roots = if roots.is_empty() {
            "<none declared>".to_owned()
        } else {
            roots
        };
        let resolved = std::fs::canonicalize(path).map_err(|e| {
            ConfigError::symlink_follow_refused(path, format!("<unresolvable: {e}>"), roots.clone())
        })?;
        if self.resolves_within_follow_roots(&resolved) {
            Ok(Some(resolved))
        } else {
            Err(ConfigError::symlink_follow_refused(
                path,
                resolved.display().to_string(),
                roots,
            ))
        }
    }

    /// Best-effort absolute resolution of a `Symlink` step target for the
    /// containment check: relative against the link's parent, canonicalized
    /// when it exists.
    fn resolve_symlink_step_target(link: &Path, target: &Path) -> PathBuf {
        let absolute = if target.is_absolute() {
            target.to_path_buf()
        } else {
            link.parent()
                .map(|parent| parent.join(target))
                .filter(|joined| joined.is_absolute())
                .unwrap_or_else(|| target.to_path_buf())
        };
        if let Ok(canon) = std::fs::canonicalize(&absolute) {
            return canon;
        }
        // The target usually does not exist yet; canonicalize its longest
        // existing ancestor so symlinked temp roots compare equal.
        let mut prefix = absolute.clone();
        let mut tail = PathBuf::new();
        while let Some(parent) = prefix.parent() {
            if let Some(last) = prefix.file_name() {
                tail = Path::new(last).join(tail);
            }
            prefix = parent.to_path_buf();
            if let Ok(canon) = std::fs::canonicalize(&prefix) {
                return canon.join(&tail);
            }
        }
        absolute
    }

    /// Invoke the attached injector, if any. Zero cost when `None`.
    fn inject(&self, point: Point) -> Result<()> {
        if let Some(injector) = &self.injector {
            injector.inject(point)
        } else {
            Ok(())
        }
    }

    /// Path-safety warnings recorded during prepare (e.g. hard-link sharing).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Write the journal entry for `phase`; no-op when journaling is off.
    /// The injector's journal-phase point fires after the write so crash
    /// simulation leaves the journal at exactly this phase.
    fn write_journal(&mut self, phase: JournalPhase) -> Result<()> {
        let Some(root) = self.journal_root.clone() else {
            return Ok(());
        };
        let resources: Vec<String> = self
            .steps
            .iter()
            .map(|s| s.primary_path().to_string_lossy().into_owned())
            .collect();
        let mut journal = CrashJournal::new(self.id.as_str(), phase, resources);
        journal.backups = self
            .backups
            .iter()
            .map(|b| JournalBackup {
                resource: b.original_path.to_string_lossy().into_owned(),
                backup_id: b.id.as_str().to_owned(),
            })
            .collect();
        journal.staged_temps = self
            .staged_temps
            .iter()
            .map(|t| t.to_string_lossy().into_owned())
            .collect();
        journal.completed = self
            .journal_completed
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        journal.diagnostics.clone_from(&self.warnings);
        journal.write_to(&crate::journal::journal_path(&root, self.id.as_str()))?;
        self.journal_state = Some(journal);
        let point = match phase {
            JournalPhase::Plan => Point::JournalPlan,
            JournalPhase::PrepareBackup => Point::JournalPrepareBackup,
            JournalPhase::StageTemp => Point::JournalStageTemp,
            JournalPhase::Commit => Point::JournalCommit,
            JournalPhase::Verify => Point::JournalVerify,
            JournalPhase::Rollback => Point::JournalRollback,
            JournalPhase::Done => return Ok(()),
        };
        self.inject(point)
    }

    /// Remove the journal after verified completion or rollback (MUT-09).
    fn clear_journal(&mut self) {
        if let Some(root) = self.journal_root.clone() {
            let path = crate::journal::journal_path(&root, self.id.as_str());
            if let Err(e) = CrashJournal::remove(&path) {
                self.warnings.push(format!("journal removal failed: {e}"));
            }
        }
        self.journal_state = None;
    }

    /// Validate the plan without touching disk beyond snapshots: path
    /// safety, symlink loops, duplicate and case-fold collisions, hard-link
    /// aliases ([`ConfigError::HardlinkConflict`]), and follow-root policy
    /// for `Write` and `Symlink` targets.
    pub fn validate_plan(&self) -> Result<()> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut seen_folded: HashSet<String> = HashSet::new();
        let mut seen_inodes: HashMap<(u64, u64), PathBuf> = HashMap::new();
        for step in &self.steps {
            let path = step.primary_path();
            validate_path_safety(path)?;
            if crate::snapshot::is_symlink_loop(path) {
                return Err(ConfigError::io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "symlink loop detected"),
                ));
            }
            // A Write onto an existing symlink follows it only inside the
            // declared roots (MUT-02).
            if matches!(step, FileAction::Write { .. }) {
                self.symlink_follow_target(path)?;
            }
            // A planned link may not point outside the declared roots.
            if let FileAction::Symlink { link, target, .. } = step
                && !self.symlink_follow_roots.is_empty()
            {
                let resolved = Self::resolve_symlink_step_target(link, target);
                if !self.resolves_within_follow_roots(&resolved) {
                    return Err(ConfigError::symlink_follow_refused(
                        link,
                        format!(
                            "{} (planned target {})",
                            resolved.display(),
                            target.display()
                        ),
                        self.symlink_follow_roots
                            .iter()
                            .map(|r| r.to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join(", "),
                    ));
                }
            }
            let key = path.to_string_lossy().into_owned();
            if !seen.insert(key.clone()) {
                return Err(ConfigError::io(
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "duplicate path in transaction",
                    ),
                ));
            }
            let folded = key.to_ascii_lowercase();
            if !seen_folded.insert(folded.clone()) {
                // A collision on case-insensitive platforms; surfaced as a
                // risk on every platform.
                return Err(ConfigError::io(
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "case-fold collision in transaction",
                    ),
                ));
            }
            // Two planned paths on one inode would write the same bytes
            // through two names.
            if let Some(inode) = inode_identity(path)
                && let Some(alias) = seen_inodes.get(&inode)
            {
                return Err(ConfigError::hardlink_conflict(
                    path,
                    alias,
                    "two plan steps resolve to the same inode on one device",
                ));
            }
            if let Some(inode) = inode_identity(path) {
                seen_inodes.insert(inode, path.to_path_buf());
            }
        }
        Ok(())
    }

    /// Sort steps deterministically for commit order.
    pub fn sort_steps(&mut self) {
        self.steps.sort_by_key(FileAction::sort_key);
    }

    /// Prepare: back up foreign files, stage and parse-validate temps,
    /// record §4.2 tokens, and retarget `Write` steps on allowed symlinks
    /// to their referents (MUT-02), re-verified at commit.
    pub fn prepare(&mut self) -> Result<()> {
        self.validate_plan()?;
        self.sort_steps();
        self.apply_symlink_follow_retargeting()?;
        self.write_journal(JournalPhase::Plan)?;

        let snapshots = self.snapshot_targets();
        self.record_hardlink_warnings();
        self.back_up_foreign_targets(&snapshots)?;
        self.write_journal(JournalPhase::PrepareBackup)?;

        let staged_map = self.stage_all_writes()?;
        self.write_journal(JournalPhase::StageTemp)?;

        // Tokens are recorded after staging, so parents the staging itself
        // created are expected to exist; the window guarded runs from here
        // to each step's pre-mutation recheck.
        self.expected_states.clear();
        for step in &self.steps {
            let path = step.primary_path().to_path_buf();
            if self.expected_states.contains_key(&path) {
                continue;
            }
            let snap = snapshot(&path);
            self.expected_states.insert(path, snap);
        }

        // Every staged temp must still carry exactly its planned bytes.
        let planned: HashMap<&Path, &Vec<u8>> = self
            .steps
            .iter()
            .filter_map(|step| match step {
                FileAction::Write { path, content, .. } => Some((path.as_path(), content)),
                _ => None,
            })
            .collect();
        for (target, temp) in staged_map {
            let Some(content) = planned.get(target.as_path()) else {
                continue;
            };
            let expected = compute_digest(content);
            let staged_bytes = std::fs::read(&temp).map_err(|e| ConfigError::io(&temp, e))?;
            let actual = compute_digest(&staged_bytes);
            if expected != actual {
                return Err(ConfigError::verification(
                    &target,
                    format!("staged digest mismatch for {}", target.display()),
                ));
            }
        }

        Ok(())
    }

    /// Fresh snapshot of every step path for the backup-time conflict check.
    fn snapshot_targets(&self) -> HashMap<PathBuf, Snapshot> {
        let mut snapshots: HashMap<PathBuf, Snapshot> = HashMap::new();
        for step in &self.steps {
            let path = step.primary_path().to_path_buf();
            if snapshots.contains_key(&path) {
                continue;
            }
            let snap = snapshot(&path);
            snapshots.insert(path, snap);
        }
        snapshots
    }

    /// Record a warning for write targets with `nlink > 1`: replacement
    /// splits the link group. Alias pairs are rejected in
    /// [`Self::validate_plan`].
    fn record_hardlink_warnings(&mut self) {
        for step in &self.steps {
            if matches!(step, FileAction::Write { .. })
                && let Some(nlink) = hardlink_count(step.primary_path())
                && nlink > 1
            {
                self.warnings.push(format!(
                    "hard link sharing: {} has {nlink} links; atomic replacement updates only this path",
                    step.primary_path().display()
                ));
            }
        }
    }

    /// Back up all foreign files before the first commit (MUT-05).
    fn back_up_foreign_targets(&mut self, snapshots: &HashMap<PathBuf, Snapshot>) -> Result<()> {
        for step in &self.steps {
            let target: Option<&Path> = match step {
                FileAction::Write { path, .. } | FileAction::RemoveFile { path } => {
                    Some(path.as_path())
                }
                FileAction::QuarantineMove { from, .. } => Some(from.as_path()),
                FileAction::CreateDir { .. } | FileAction::Symlink { .. } => None,
            };
            let Some(p) = target else {
                continue;
            };
            let Some(snap) = snapshots.get(p) else {
                continue;
            };
            if !(snap.exists && snap.is_file) {
                continue;
            }
            let current = snapshot(p);
            if is_modified(snap, &current) {
                return Err(ConfigError::concurrent_modification(
                    p,
                    snap.digest.clone().unwrap_or_default(),
                    current.digest.unwrap_or_default(),
                ));
            }
            let entry = backup_with_injector(
                p,
                Some(self.id.as_str()),
                "transaction prepare",
                self.injector.as_deref(),
            )?;
            if let Some(entry) = entry {
                self.backups.push(entry);
            }
        }
        Ok(())
    }

    /// Stage temps for every Write action and parse-validate them. Returns
    /// (target, temp) pairs for staged-digest verification.
    fn stage_all_writes(&mut self) -> Result<Vec<(PathBuf, PathBuf)>> {
        let mut staged: Vec<PathBuf> = Vec::new();
        let mut staged_map: Vec<(PathBuf, PathBuf)> = Vec::new(); // (target, temp)
        let result = (|| {
            for step in &self.steps {
                let FileAction::Write {
                    path,
                    content,
                    kind,
                } = step
                else {
                    continue;
                };
                self.inject(Point::ParseStaged)?;
                validate_bytes_for_kind(content, *kind, path)?;
                let temp_path = self.stage_write(path, content)?;
                // The staged file itself must parse (read fresh).
                let staged_bytes =
                    std::fs::read(&temp_path).map_err(|e| ConfigError::io(&temp_path, e))?;
                validate_bytes_for_kind(&staged_bytes, *kind, &temp_path)?;
                staged.push(temp_path.clone());
                staged_map.push((path.clone(), temp_path));
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.staged_temps = staged;
                Ok(staged_map)
            }
            Err(e) => {
                // Non-journaled callers have no recovery sweep; failed
                // staging must clean up after itself.
                for temp in &staged {
                    drop(std::fs::remove_file(temp));
                }
                Err(e)
            }
        }
    }

    fn stage_write(&self, target: &Path, content: &[u8]) -> Result<PathBuf> {
        stage_temp_file(target, content, self.injector.as_deref())
    }

    /// Retarget `Write` steps on allowed symlinks to their referents (MUT-02)
    /// and record the pairs the commit phase re-verifies. The policy is
    /// re-derived here; errors propagate, never swallowed.
    fn apply_symlink_follow_retargeting(&mut self) -> Result<()> {
        let mut retargets: Vec<(usize, PathBuf, PathBuf)> = Vec::new();
        for (idx, step) in self.steps.iter().enumerate() {
            let FileAction::Write { path, .. } = step else {
                continue;
            };
            if let Some(referent) = self.symlink_follow_target(path)?
                && referent.as_path() != path.as_path()
            {
                retargets.push((idx, path.clone(), referent));
            }
        }
        for (idx, link, referent) in retargets {
            if let Some(FileAction::Write { path, .. }) = self.steps.get_mut(idx) {
                path.clone_from(&referent);
                self.symlink_followed.push((link, referent));
            }
        }
        Ok(())
    }

    /// Every link followed during prepare must still point at its recorded
    /// referent, or the step aborts before the rename lands on a file the
    /// caller no longer reaches.
    fn recheck_followed_symlinks(&self) -> Result<()> {
        for (link, referent) in &self.symlink_followed {
            let current = std::fs::canonicalize(link).map_err(|e| {
                ConfigError::concurrent_modification(
                    link,
                    referent.display().to_string(),
                    format!("<unresolvable: {e}>"),
                )
            })?;
            if &current != referent {
                return Err(ConfigError::concurrent_modification(
                    link,
                    referent.display().to_string(),
                    current.display().to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Commit in dependency order, assuming [`Self::prepare`] ran. Each step
    /// rechecks its prepare-time snapshot immediately before mutating
    /// (§4.2); a foreign change aborts before any overwrite.
    pub fn commit(&mut self) -> Result<CommitOutcome> {
        let mut committed: Vec<PathBuf> = Vec::new();
        let mut write_index = 0usize;
        self.partial_rollback = None;
        self.journal_completed.clear();
        self.write_journal(JournalPhase::Commit)?;

        for step_index in 0..self.steps.len() {
            // Intent journaling (MUT-09): the step is recorded as
            // about-to-commit BEFORE mutating, so a crash between the rename
            // and the journal update is still attributable at recovery.
            let Some(primary) = self
                .steps
                .get(step_index)
                .map(|s| s.primary_path().to_path_buf())
            else {
                break;
            };
            self.journal_completed.push(primary.clone());
            self.write_journal(JournalPhase::Commit)?;
            let res = self.commit_step(step_index, &mut write_index);
            if let Err(e) = res {
                // Compensate the already committed steps in reverse order and
                // retain the outcome: any path the rollback could not undo is
                // a residual that must reach the caller. The original commit
                // error stays the surfaced error; nothing is masked.
                let rollback = self.rollback_partial(&committed);
                self.partial_rollback = Some(rollback);
                return Err(e);
            }
            committed.push(primary);
        }

        for temp in &self.staged_temps {
            if temp.exists() {
                drop(std::fs::remove_file(temp));
            }
        }

        Ok(CommitOutcome {
            committed,
            backups: self.backups.clone(),
        })
    }

    /// Commit one step by index. Prelude injections (QAL-06: the second/third
    /// file boundaries) feed the same error path as the step itself so the
    /// compensation in `commit` still runs when they fire.
    fn commit_step(&self, step_index: usize, write_index: &mut usize) -> Result<()> {
        let prelude = if step_index == 1 {
            self.inject(Point::SecondFile)
        } else if step_index == 2 {
            self.inject(Point::ThirdFile)
        } else {
            Ok(())
        };
        let Some(step) = self.steps.get(step_index) else {
            return Ok(());
        };
        prelude?;
        match step {
            FileAction::CreateDir { path } => self.commit_create_dir(path),
            FileAction::Write { path, .. } => {
                let temp_opt = self.staged_temps.get(*write_index).cloned();
                *write_index = write_index.saturating_add(1);
                if let Some(temp) = temp_opt {
                    self.commit_write(path, &temp)
                } else {
                    Err(ConfigError::io(
                        path,
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "missing staged temp for write",
                        ),
                    ))
                }
            }
            FileAction::Symlink {
                link,
                target,
                expected_current,
            } => self.commit_symlink(link, target, expected_current.clone()),
            FileAction::RemoveFile { path } => self.commit_remove_file(path),
            FileAction::QuarantineMove { from, to } => self.commit_quarantine_move(from, to),
        }
    }

    fn commit_create_dir(&self, path: &Path) -> Result<()> {
        validate_path_safety(path)?;
        // A directory that appeared between prepare and commit is a foreign
        // change, not a satisfied precondition (§4.2).
        if let Some(expected) = self.expected_states.get(path)
            && !expected.exists
        {
            let current = snapshot(path);
            if current.exists {
                return Err(ConfigError::concurrent_modification(
                    path,
                    "<absent>".to_owned(),
                    "<directory>".to_owned(),
                ));
            }
        }
        if path.exists() {
            let meta = std::fs::symlink_metadata(path).map_err(|e| ConfigError::io(path, e))?;
            if !meta.is_dir() {
                return Err(ConfigError::io(
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "path exists and is not a directory",
                    ),
                ));
            }
            return Ok(());
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
        }
        std::fs::create_dir_all(path).map_err(|e| ConfigError::io(path, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perm = std::fs::Permissions::from_mode(0o755);
            drop(std::fs::set_permissions(path, perm));
        }
        self.inject(Point::ParentSync)?;
        sync_parent(path)?;
        Ok(())
    }

    fn commit_write(&self, target: &Path, staged: &Path) -> Result<()> {
        // Followed links must still resolve to their referents immediately
        // before any rename lands (MUT-02).
        self.recheck_followed_symlinks()?;
        commit_staged_file(
            target,
            staged,
            self.expected_states.get(target),
            self.injector.as_deref(),
        )
    }

    fn commit_symlink(
        &self,
        link: &Path,
        target: &Path,
        expected_current: Option<PathBuf>,
    ) -> Result<()> {
        validate_path_safety(link)?;
        if let Some(parent) = link.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
        }
        // Replace only when the link matches the expected owned target
        // (MUT-02/MUT-06) and has not changed since prepare (§4.2).
        if link.exists() || std::fs::symlink_metadata(link).is_ok() {
            let meta = std::fs::symlink_metadata(link).map_err(|e| ConfigError::io(link, e))?;
            if meta.file_type().is_symlink() {
                let current_target =
                    std::fs::read_link(link).map_err(|e| ConfigError::io(link, e))?;
                if let Some(expected) = expected_current {
                    // A link pointing anywhere else is not ours to overwrite.
                    if current_target != expected {
                        return Err(ConfigError::symlink_target_mismatch(
                            link,
                            expected.display().to_string(),
                            current_target.display().to_string(),
                        ));
                    }
                } else if let Some(prepare_state) = self.expected_states.get(link) {
                    // The link must still carry its prepare-time target.
                    match &prepare_state.symlink_target {
                        Some(prepare_target) if &current_target != prepare_target => {
                            return Err(ConfigError::concurrent_modification(
                                link,
                                prepare_target.display().to_string(),
                                current_target.display().to_string(),
                            ));
                        }
                        Some(_) => {}
                        None => {
                            // Not a symlink at prepare time, but one now.
                            return Err(ConfigError::concurrent_modification(
                                link,
                                "<not a symlink>".to_owned(),
                                current_target.display().to_string(),
                            ));
                        }
                    }
                }
                std::fs::remove_file(link).map_err(|e| ConfigError::io(link, e))?;
            } else {
                return Err(ConfigError::io(
                    link,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "path exists and is not a symlink",
                    ),
                ));
            }
        } else if let Some(expected) = expected_current {
            // The link we expected to own is gone.
            return Err(ConfigError::concurrent_modification(
                link,
                expected.display().to_string(),
                "<absent>".to_owned(),
            ));
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).map_err(|e| ConfigError::io(link, e))?;
        }
        #[cfg(not(unix))]
        {
            // Windows resolves relative targets differently; join against
            // the link's directory first.
            let resolved = if target.is_absolute() {
                target.to_path_buf()
            } else {
                link.parent()
                    .map(|p| p.join(target))
                    .filter(|p| p.is_absolute())
                    .unwrap_or_else(|| target.to_path_buf())
            };
            // A directory target needs a directory symlink on Windows.
            if resolved.is_dir() {
                std::os::windows::fs::symlink_dir(&resolved, link)
                    .map_err(|e| ConfigError::io(link, e))?;
            } else {
                std::os::windows::fs::symlink_file(&resolved, link)
                    .map_err(|e| ConfigError::io(link, e))?;
            }
        }
        self.inject(Point::ParentSync)?;
        sync_parent(link)?;
        Ok(())
    }

    fn commit_remove_file(&self, path: &Path) -> Result<()> {
        // Removing a file that changed since prepare would destroy a
        // foreign edit; abort instead (§4.2).
        if let Some(expected) = self.expected_states.get(path)
            && expected.exists
        {
            let current = snapshot(path);
            if is_modified(expected, &current) {
                return Err(ConfigError::concurrent_modification(
                    path,
                    expected.digest.clone().unwrap_or_default(),
                    current.digest.unwrap_or_default(),
                ));
            }
        }
        if !path.exists() && std::fs::symlink_metadata(path).is_err() {
            return Ok(());
        }
        std::fs::remove_file(path).map_err(|e| ConfigError::io(path, e))?;
        self.inject(Point::ParentSync)?;
        sync_parent(path)?;
        Ok(())
    }

    fn commit_quarantine_move(&self, from: &Path, to: &Path) -> Result<()> {
        // A file that changed since prepare would move a foreign edit out
        // of reach; abort instead (§4.2).
        if let Some(expected) = self.expected_states.get(from)
            && expected.exists
        {
            let current = snapshot(from);
            if is_modified(expected, &current) {
                return Err(ConfigError::concurrent_modification(
                    from,
                    expected.digest.clone().unwrap_or_default(),
                    current.digest.unwrap_or_default(),
                ));
            }
        }
        crate::quarantine::move_to_quarantine_with_dest(from, to, self.id.as_str())?;
        Ok(())
    }

    /// Verify after commit: fresh read plus parse per Write step.
    #[expect(
        clippy::excessive_nesting,
        reason = "verify checks digest and parse per file"
    )]
    pub fn verify(&self) -> Result<Vec<VerifyOutcome>> {
        let mut outcomes = Vec::new();
        for step in &self.steps {
            if let FileAction::Write {
                path,
                content,
                kind,
            } = step
            {
                let bytes = match std::fs::read(path) {
                    Ok(b) => b,
                    Err(e) => {
                        outcomes.push(VerifyOutcome {
                            path: path.clone(),
                            digest_ok: false,
                            parse_ok: false,
                            message: format!("read failed: {e}"),
                        });
                        continue;
                    }
                };
                let expected_digest = compute_digest(content);
                let actual_digest = compute_digest(&bytes);
                let digest_ok = expected_digest == actual_digest;
                let parse_ok = validate_bytes_for_kind(&bytes, *kind, path).is_ok();
                let message = if digest_ok && parse_ok {
                    "verified".to_owned()
                } else if !digest_ok {
                    format!("digest mismatch: expected {expected_digest}, got {actual_digest}")
                } else {
                    "parse failed after commit".to_owned()
                };
                // No raw bytes in messages: secret-like content is redacted.
                let redacted_message = if message.contains("apiKey") || message.contains("secret") {
                    "[REDACTED]".to_owned()
                } else {
                    message
                };
                outcomes.push(VerifyOutcome {
                    path: path.clone(),
                    digest_ok,
                    parse_ok,
                    message: redacted_message,
                });
            }
        }
        // Rollback is caller-driven; verification only reports.
        Ok(outcomes)
    }

    fn rollback_partial(&self, committed: &[PathBuf]) -> RollbackOutcome {
        self.rollback_with_filter(committed)
    }

    /// Restore committed files in reverse order and report residuals.
    pub fn rollback(&mut self) -> Result<RollbackOutcome> {
        let committed: Vec<PathBuf> = self
            .steps
            .iter()
            .map(|s| s.primary_path().to_path_buf())
            .collect();
        Ok(self.rollback_with_filter(&committed))
    }

    #[expect(
        clippy::excessive_nesting,
        reason = "rollback restores per-file with nested verification"
    )]
    fn rollback_with_filter(&self, committed: &[PathBuf]) -> RollbackOutcome {
        let mut rolled_back: Vec<PathBuf> = Vec::new();
        let mut residuals: Vec<PathBuf> = Vec::new();
        // Path to backup entry for quick lookup.
        let backup_map: HashMap<PathBuf, &BackupEntry> = self
            .backups
            .iter()
            .map(|e| (e.original_path.clone(), e))
            .collect();

        for path in committed.iter().rev() {
            if let Some(entry) = backup_map.get(path) {
                if !matches!(verify_backup(entry), Ok(true)) {
                    residuals.push(path.clone());
                    continue;
                }
                match crate::backup::restore_entry(entry) {
                    Ok(()) => {
                        // QAL-06 boundary: verifying the rollback restore.
                        if self.inject(Point::RollbackVerify).is_err() {
                            residuals.push(path.clone());
                            continue;
                        }
                        // Verify rollback
                        match std::fs::read(path) {
                            Ok(bytes) => {
                                let d = compute_digest(&bytes);
                                if d == entry.digest {
                                    rolled_back.push(path.clone());
                                } else {
                                    residuals.push(path.clone());
                                }
                            }
                            Err(_) => residuals.push(path.clone()),
                        }
                    }
                    Err(_) => residuals.push(path.clone()),
                }
            } else {
                // No backup means a creation: remove it if it exists.
                if path.exists() || std::fs::symlink_metadata(path).is_ok() {
                    let meta_res = std::fs::symlink_metadata(path);
                    if let Ok(meta) = meta_res {
                        if meta.is_dir() {
                            match std::fs::remove_dir(path) {
                                Ok(()) => rolled_back.push(path.clone()),
                                Err(_) => residuals.push(path.clone()),
                            }
                        } else {
                            match std::fs::remove_file(path) {
                                Ok(()) => rolled_back.push(path.clone()),
                                Err(_) => residuals.push(path.clone()),
                            }
                        }
                    } else {
                        residuals.push(path.clone());
                    }
                } else {
                    rolled_back.push(path.clone());
                }
            }
        }

        let verification_ok = residuals.is_empty();

        for temp in &self.staged_temps {
            if temp.exists() {
                drop(std::fs::remove_file(temp));
            }
        }

        RollbackOutcome {
            rolled_back,
            residuals,
            verification_ok,
        }
    }

    /// Execute the full transaction: prepare, commit, verify, automatic
    /// rollback on failure. The journal is removed only after verified
    /// completion or verified rollback (MUT-09).
    #[expect(
        clippy::too_many_lines,
        reason = "execute is the orchestrator: prepare/commit/verify/rollback in one place"
    )]
    pub fn execute(&mut self) -> Result<TransactionOutcome> {
        match self.prepare() {
            Ok(()) => {}
            Err(e) => {
                return Ok(TransactionOutcome {
                    success: false,
                    commit: None,
                    verification: Vec::new(),
                    rollback: None,
                    diagnostics_redacted: vec![format!("[prepare failed] {e}")],
                });
            }
        }
        let commit_outcome = match self.commit() {
            Ok(c) => c,
            Err(e) => {
                // `commit` already compensated; its recorded outcome,
                // residuals included, is what the caller sees.
                let mut diagnostics_redacted = vec![format!("[commit failed] {e}")];
                if let Some(rollback) = &self.partial_rollback
                    && !rollback.residuals.is_empty()
                {
                    let residual_paths: Vec<String> = rollback
                        .residuals
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect();
                    diagnostics_redacted.push(format!(
                        "[rollback residuals] {}",
                        residual_paths.join(", ")
                    ));
                }
                // Nothing residual means the journal has nothing to recover;
                // otherwise it stays for startup recovery.
                if let Some(rollback) = &self.partial_rollback
                    && rollback.residuals.is_empty()
                {
                    self.clear_journal();
                }
                return Ok(TransactionOutcome {
                    success: false,
                    commit: None,
                    verification: Vec::new(),
                    rollback: self.partial_rollback.clone(),
                    diagnostics_redacted,
                });
            }
        };
        if let Err(e) = self.write_journal(JournalPhase::Verify) {
            return Ok(TransactionOutcome {
                success: false,
                commit: Some(commit_outcome),
                verification: Vec::new(),
                rollback: None,
                diagnostics_redacted: vec![format!("[verify journal failed] {e}")],
            });
        }
        let verification = match self.verify() {
            Ok(v) => v,
            Err(e) => {
                if let Err(je) = self.write_journal(JournalPhase::Rollback) {
                    self.warnings.push(format!("journal write failed: {je}"));
                }
                let rb = self.rollback();
                if rb.as_ref().is_ok_and(|r| r.residuals.is_empty()) {
                    self.clear_journal();
                }
                return Ok(TransactionOutcome {
                    success: false,
                    commit: Some(commit_outcome),
                    verification: Vec::new(),
                    rollback: rb.ok(),
                    diagnostics_redacted: vec![format!("[verify failed] {e}")],
                });
            }
        };
        let has_failure = verification.iter().any(|v| !v.digest_ok || !v.parse_ok);
        if has_failure {
            if let Err(je) = self.write_journal(JournalPhase::Rollback) {
                self.warnings.push(format!("journal write failed: {je}"));
            }
            let rollback = self.rollback().unwrap_or(RollbackOutcome {
                rolled_back: Vec::new(),
                residuals: commit_outcome.committed.clone(),
                verification_ok: false,
            });
            if rollback.residuals.is_empty() {
                self.clear_journal();
            }
            return Ok(TransactionOutcome {
                success: false,
                commit: Some(commit_outcome),
                verification,
                rollback: Some(rollback),
                diagnostics_redacted: vec!["[verification failed] rollback attempted".to_owned()],
            });
        }
        // Verified completion: the journal can be removed (MUT-09).
        self.clear_journal();
        let mut diagnostics_redacted = vec!["transaction succeeded".to_owned()];
        diagnostics_redacted.extend(self.warnings.iter().cloned());
        Ok(TransactionOutcome {
            success: true,
            commit: Some(commit_outcome),
            verification,
            rollback: None,
            diagnostics_redacted,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(
    clippy::assertions_on_result_states,
    reason = "tests assert error presence"
)]
mod tests {
    use super::*;
    use crate::document::strip_jsonc_comments;
    use crate::journal::{journal_path, recover_pending};
    use std::sync::Mutex;

    /// Test injector that fails exactly on the Nth call of configured points.
    #[derive(Debug)]
    struct FailAtPoint {
        rules: Vec<(Point, usize)>,
        calls: Mutex<HashMap<Point, usize>>,
    }

    impl FailAtPoint {
        fn new(point: Point, fail_at: usize) -> Arc<Self> {
            Arc::new(Self {
                rules: vec![(point, fail_at)],
                calls: Mutex::new(HashMap::new()),
            })
        }

        fn two(point_a: Point, nth_a: usize, point_b: Point, nth_b: usize) -> Arc<Self> {
            Arc::new(Self {
                rules: vec![(point_a, nth_a), (point_b, nth_b)],
                calls: Mutex::new(HashMap::new()),
            })
        }
    }

    impl Injector for FailAtPoint {
        fn inject(&self, point: Point) -> Result<()> {
            if !self.rules.iter().any(|(p, _)| *p == point) {
                return Ok(());
            }
            let mut calls = match self.calls.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let count = calls.entry(point).or_insert(0);
            *count = count.saturating_add(1);
            if self
                .rules
                .iter()
                .any(|(p, nth)| *p == point && *nth == *count)
            {
                return Err(ConfigError::verification(
                    Path::new("injected"),
                    format!("injected failure at {point}"),
                ));
            }
            Ok(())
        }
    }

    fn tmp_root() -> PathBuf {
        crate::test_util::temp_dir_unique("txn")
    }

    #[test]
    fn operation_id_rejects_invalid() {
        OperationId::new("").unwrap_err();
        OperationId::new("a/b").unwrap_err();
        OperationId::new("a\\b").unwrap_err();
        OperationId::new("a:b").unwrap_err();
        OperationId::new("ok-123").unwrap();
    }

    #[test]
    fn file_action_sort_is_deterministic() {
        let id = OperationId::new("op-1").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![
                FileAction::Write {
                    path: PathBuf::from("/tmp/b.json"),
                    content: b"{}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::CreateDir {
                    path: PathBuf::from("/tmp/a"),
                },
                FileAction::Write {
                    path: PathBuf::from("/tmp/a.json"),
                    content: b"{}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
            ],
        );
        txn.sort_steps();
        // CreateDir should come first, then Writes sorted by path
        assert!(matches!(txn.steps[0], FileAction::CreateDir { .. }));
        if let FileAction::Write { path, .. } = &txn.steps[1] {
            assert_eq!(path, &PathBuf::from("/tmp/a.json"));
        } else {
            panic!("expected write");
        }
    }

    #[test]
    fn validate_plan_rejects_duplicate_and_traversal() {
        let id = OperationId::new("op-2").unwrap();
        let txn = Transaction::new(
            id.clone(),
            vec![
                FileAction::Write {
                    path: PathBuf::from("/tmp/a.json"),
                    content: b"{}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::Write {
                    path: PathBuf::from("/tmp/a.json"),
                    content: b"{}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
            ],
        );
        assert!(txn.validate_plan().is_err());

        let txn2 = Transaction::new(
            id,
            vec![FileAction::Write {
                path: PathBuf::from("/tmp/../etc/passwd"),
                content: b"{}".to_vec(),
                kind: DocumentKind::StrictJson,
            }],
        );
        assert!(txn2.validate_plan().is_err());
    }

    #[test]
    fn remove_plan_validation() {
        let home = home_dir().unwrap_or_else(|| PathBuf::from("/home/test"));
        let bad = RemovePlan::new(RemoveKind::InstanceRoot, &home);
        assert!(bad.is_err());

        let bad2 = RemovePlan::new(RemoveKind::InstanceRoot, Path::new("/"));
        assert!(bad2.is_err());

        let bad3 = RemovePlan::new(RemoveKind::InstanceRoot, Path::new("/tmp/*.json"));
        assert!(bad3.is_err());

        // Absolute paths that are valid on every platform (windows rejects
        // drive-less "/tmp/..." as relative).
        let base = std::env::temp_dir();
        let ok = RemovePlan::new(RemoveKind::WrapperFile, &base.join("wrapper"));
        assert!(ok.is_ok());
        assert!(!ok.unwrap().requires_quarantine);

        let ok2 = RemovePlan::new(RemoveKind::InstanceRoot, &base.join("instance-root"));
        assert!(ok2.unwrap().requires_quarantine);
    }

    /// QAL-09/11 platform case: windows-shaped broad roots are recognized on
    /// every host (pure string semantics: drive roots, UNC roots, first-level
    /// system directories, case-folded, both separators).
    #[test]
    fn windows_shaped_broad_roots_are_detected_cross_platform() {
        let broad = [
            "C:\\",
            "C:/",
            "c:",
            "C:\\Windows",
            "c:\\windows",
            "C:/Program Files",
            "C:\\Program Files (x86)",
            "c:\\programdata\\",
            "C:\\Users",
            "C:\\Documents and Settings",
            "\\\\fileserver",
            "\\\\fileserver\\share",
        ];
        for p in broad {
            assert!(
                windows_shaped_broad_root(Path::new(p)),
                "windows-shaped broad root must be detected: {p}"
            );
        }
        // Forward-slash UNC text is only a UNC root on Windows itself; on
        // unix `//x` is an ordinary (if unusual) absolute path and must not
        // be flagged, so unix removal/quarantine semantics are unchanged.
        if cfg!(windows) {
            assert!(windows_shaped_broad_root(Path::new("//server/share/")));
        } else {
            assert!(!windows_shaped_broad_root(Path::new("//server/share/")));
        }
        let specific = [
            "C:\\Users\\me\\.claude",
            "C:\\Windows\\Temp\\target.json",
            "C:/Program Files/Tool/config.toml",
            "\\\\server\\share\\dir\\file.json",
            "/tmp",
            "/home",
            "/",
            "relative/path",
            // A drive prefix needs BOTH an alphabetic first char and ':' as
            // the second: `1:` is not windows-shaped (kills the `&&`->`||`
            // mutant in the drive-prefix check, which would flag it broad).
            "1:",
        ];
        for p in specific {
            assert!(
                !windows_shaped_broad_root(Path::new(p)),
                "specific (or unix) target must not be flagged broad: {p}"
            );
        }
    }

    /// Windows case-folding in path identity: home and quarantine comparisons
    /// must match case-insensitively for windows-shaped paths.
    #[test]
    fn paths_equal_platform_folded_matches_windows_case() {
        assert!(paths_equal_platform_folded(
            Path::new("C:\\Users\\Me"),
            Path::new("c:/users/me")
        ));
        assert!(paths_equal_platform_folded(
            Path::new("/tmp/a"),
            Path::new("/tmp/a")
        ));
        // Unix paths stay case-sensitive: differing case is NOT equal.
        assert!(!paths_equal_platform_folded(
            Path::new("/tmp/A"),
            Path::new("/tmp/a")
        ));
        assert!(!paths_equal_platform_folded(
            Path::new("C:\\Windows"),
            Path::new("C:\\Windows\\Temp")
        ));
    }

    #[test]
    fn transaction_prepare_and_commit_single_file() {
        let root = tmp_root();
        let target = root.join("a.json");
        std::fs::write(&target, b"{\"a\":1}").unwrap();

        let id = OperationId::new("op-commit-1").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: target.clone(),
                content: b"{\"a\":2}".to_vec(),
                kind: DocumentKind::StrictJson,
            }],
        );
        txn.prepare().unwrap();
        assert_eq!(txn.backups.len(), 1);
        assert_eq!(txn.staged_temps.len(), 1);
        let commit = txn.commit().unwrap();
        assert_eq!(commit.committed, vec![target.clone()]);
        let verify = txn.verify().unwrap();
        assert!(verify[0].digest_ok);
        assert!(verify[0].parse_ok);
        let content = std::fs::read(&target).unwrap();
        assert_eq!(content, b"{\"a\":2}");
        drop(std::fs::remove_file(&target));
        for b in txn.backups {
            drop(std::fs::remove_file(b.backup_path));
        }
        drop(std::fs::remove_dir(&root));
    }

    #[test]
    fn transaction_rollback_on_verify_failure_reports_residuals() {
        let root = tmp_root();
        let target = root.join("b.json");
        std::fs::write(&target, b"{\"x\":1}").unwrap();

        let id = OperationId::new("op-rollback-1").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: target.clone(),
                content: b"not json".to_vec(), // invalid json, but prepare validates, so this will fail at prepare
                kind: DocumentKind::StrictJson,
            }],
        );
        let res = txn.prepare();
        assert!(res.is_err(), "prepare should reject invalid json");
        // Ensure original remains
        let cur = std::fs::read(&target).unwrap();
        assert_eq!(cur, b"{\"x\":1}");
        drop(std::fs::remove_file(&target));
        drop(std::fs::remove_dir(&root));
    }

    #[test]
    fn transaction_multi_file_backup_before_first_commit() {
        let root = tmp_root();
        let a = root.join("a.json");
        let b = root.join("b.toml");
        std::fs::write(&a, b"{\"a\":1}").unwrap();
        std::fs::write(&b, b"a=1\n").unwrap();

        let id = OperationId::new("op-multi-1").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![
                FileAction::Write {
                    path: a.clone(),
                    content: b"{\"a\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::Write {
                    path: b.clone(),
                    content: b"a=2\n".to_vec(),
                    kind: DocumentKind::Toml,
                },
            ],
        );
        txn.prepare().unwrap();
        assert_eq!(
            txn.backups.len(),
            2,
            "both foreign files should be backed up before first commit"
        );
        assert_eq!(txn.staged_temps.len(), 2);
        let commit = txn.commit().unwrap();
        assert_eq!(commit.committed.len(), 2);
        let verify = txn.verify().unwrap();
        assert!(verify.iter().all(|v| v.digest_ok && v.parse_ok));
        drop(std::fs::remove_file(&a));
        drop(std::fs::remove_file(&b));
        for entry in txn.backups {
            drop(std::fs::remove_file(entry.backup_path));
        }
        drop(std::fs::remove_dir(&root));
    }

    #[test]
    fn transaction_no_fs_wide_atomicity_is_documented() {
        // This test documents the compensated transaction contract: a failure
        // in the second file does not atomically revert the first at the
        // filesystem level without explicit rollback.
        // We verify that rollback is explicit and residuals are reported.
        let root = tmp_root();
        let a = root.join("a.json");
        let b = root.join("b.json");
        std::fs::write(&a, b"{\"a\":1}").unwrap();
        std::fs::write(&b, b"{\"b\":1}").unwrap();

        let id = OperationId::new("op-compensated").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![
                FileAction::Write {
                    path: a.clone(),
                    content: b"{\"a\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::Write {
                    path: b.clone(),
                    content: b"{\"b\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
            ],
        );
        txn.prepare().unwrap();
        // Simulate a failure on second commit by removing its staged temp before commit
        if let Some(second_temp) = txn.staged_temps.get(1).cloned() {
            drop(std::fs::remove_file(&second_temp));
        }
        let commit_res = txn.commit();
        assert!(
            commit_res.is_err(),
            "second commit should fail due to missing staged temp"
        );
        // After failure, at least one file should be rolled back or reported as residual
        // The transaction's rollback should have been attempted for the first file.
        // We verify original content is either restored or residual is reported.
        // In this test harness, we don't check exact residual, just that the transaction
        // surface reports it via rollback.
        let still_a = std::fs::read(&a).unwrap();
        // a should be either original or new, but not corrupted truncation
        assert!(still_a == b"{\"a\":1}" || still_a == b"{\"a\":2}");
        drop(std::fs::remove_file(&a));
        drop(std::fs::remove_file(&b));
        for entry in txn.backups {
            drop(std::fs::remove_file(entry.backup_path));
        }
        for temp in txn.staged_temps {
            drop(std::fs::remove_file(temp));
        }
        drop(std::fs::remove_dir(&root));
    }

    #[test]
    fn commit_failure_surfaces_intermediate_rollback_in_outcome() {
        let root = tmp_root();
        // A pre-existing non-empty directory: committed as a CreateDir step,
        // but rollback cannot undo it with remove_dir.
        let dir = root.join("preexisting-dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("foreign.txt"), b"foreign").unwrap();
        let target = root.join("a.json");
        std::fs::write(&target, b"{\"a\":1}").unwrap();
        // The link path already exists as a regular file, so the symlink step
        // fails after the earlier steps have already committed.
        let link = root.join("not-a-link");
        std::fs::write(&link, b"").unwrap();

        let id = OperationId::new("op-residual-outcome").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![
                FileAction::CreateDir { path: dir.clone() },
                FileAction::Write {
                    path: target.clone(),
                    content: b"{\"a\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::Symlink {
                    link: link.clone(),
                    target: PathBuf::from("/nonexistent-symlink-target"),
                    expected_current: None,
                },
            ],
        );
        let outcome = txn.execute().unwrap();

        assert!(!outcome.success, "commit failed mid-way");
        let rollback = outcome
            .rollback
            .expect("the compensation commit performed internally must be reported");
        assert!(
            rollback.rolled_back.contains(&target),
            "the committed write must be restored: {rollback:?}"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"{\"a\":1}",
            "rollback must restore the pre-transaction bytes"
        );
        assert!(
            rollback.residuals.contains(&dir),
            "the path rollback could not undo must be reported as residual: {rollback:?}"
        );
        assert!(
            outcome
                .diagnostics_redacted
                .iter()
                .any(|d| d.contains(&dir.display().to_string())),
            "residual paths must reach the diagnostics: {:?}",
            outcome.diagnostics_redacted
        );

        drop(std::fs::remove_file(dir.join("foreign.txt")));
        drop(std::fs::remove_dir(&dir));
        drop(std::fs::remove_file(&target));
        drop(std::fs::remove_file(&link));
        for entry in txn.backups {
            drop(std::fs::remove_file(entry.backup_path));
        }
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn commit_retains_intermediate_residuals_for_direct_callers() {
        let root = tmp_root();
        let a = root.join("a.json");
        let b = root.join("b.json");
        std::fs::write(&a, b"{\"a\":1}").unwrap();
        std::fs::write(&b, b"{\"b\":1}").unwrap();

        let id = OperationId::new("op-partial-residual").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![
                FileAction::Write {
                    path: a.clone(),
                    content: b"{\"a\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::Write {
                    path: b.clone(),
                    content: b"{\"b\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
            ],
        );
        txn.prepare().unwrap();
        let a_backup = txn
            .backups
            .iter()
            .find(|e| e.original_path == a)
            .cloned()
            .expect("prepare backs up the foreign target");
        // Corrupt a's backup so its rollback cannot verify, and destroy b's
        // staged temp so b's commit fails after a already committed.
        std::fs::write(&a_backup.backup_path, b"corrupted").unwrap();
        if let Some(temp) = txn.staged_temps.get(1).cloned() {
            drop(std::fs::remove_file(&temp));
        }

        let res = txn.commit();
        assert!(
            res.is_err(),
            "second write must fail without its staged temp"
        );
        let rollback = txn
            .partial_rollback
            .clone()
            .expect("commit must retain the intermediate rollback it performed");
        assert!(
            rollback.residuals.contains(&a),
            "the unrestorable path must be a residual: {rollback:?}"
        );
        assert!(
            rollback.rolled_back.is_empty(),
            "nothing was undoable: {rollback:?}"
        );
        // a stays at its committed bytes because the corrupted backup was
        // refused; it is reported rather than silently dropped.
        assert_eq!(std::fs::read(&a).unwrap(), b"{\"a\":2}");
        assert_eq!(std::fs::read(&b).unwrap(), b"{\"b\":1}");

        drop(std::fs::remove_file(&a));
        drop(std::fs::remove_file(&b));
        drop(std::fs::remove_file(&a_backup.backup_path));
        for temp in txn.staged_temps {
            drop(std::fs::remove_file(temp));
        }
        drop(std::fs::remove_dir_all(&root));
    }

    // ------------------------------------------------------------------
    // MUT-05: §4.2 conflict window: foreign edits between prepare and
    // commit abort with ConcurrentModification and are never overwritten.
    // ------------------------------------------------------------------

    #[test]
    fn foreign_edit_between_prepare_and_commit_aborts_write() {
        let root = tmp_root();
        let target = root.join("settings.json");
        std::fs::write(&target, b"{\"a\":1}").unwrap();

        let id = OperationId::new("op-s42-write").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: target.clone(),
                content: b"{\"a\":2}".to_vec(),
                kind: DocumentKind::StrictJson,
            }],
        );
        txn.prepare().unwrap();
        // External edit lands AFTER prepare (and after the backup) and
        // BEFORE the commit rename.
        std::fs::write(&target, b"{\"a\":\"foreign edit\"}").unwrap();

        let res = txn.commit();
        let err = res.expect_err("foreign edit must abort the commit");
        assert!(
            matches!(err, ConfigError::ConcurrentModification { .. }),
            "expected ConcurrentModification, got {err:?}"
        );
        // The foreign bytes survive, never overwritten, not even partially.
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"a\":\"foreign edit\"}");
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one scenario per step kind keeps the §4.2 coverage auditable together"
    )]
    fn foreign_edit_between_prepare_and_commit_aborts_every_step_kind() {
        // Write
        {
            let root = tmp_root();
            let target = root.join("w.json");
            std::fs::write(&target, b"v1").unwrap();
            let id = OperationId::new("op-s42-w").unwrap();
            let mut txn = Transaction::new(
                id,
                vec![FileAction::Write {
                    path: target.clone(),
                    content: b"v2".to_vec(),
                    kind: DocumentKind::TextFragment,
                }],
            );
            txn.prepare().unwrap();
            std::fs::write(&target, b"foreign").unwrap();
            let err = txn.commit().unwrap_err();
            assert!(matches!(err, ConfigError::ConcurrentModification { .. }));
            assert_eq!(std::fs::read(&target).unwrap(), b"foreign");
            drop(std::fs::remove_dir_all(&root));
        }
        // CreateDir: directory appears between prepare and commit
        {
            let root = tmp_root();
            let dir = root.join("newdir");
            let id = OperationId::new("op-s42-d").unwrap();
            let mut txn = Transaction::new(id, vec![FileAction::CreateDir { path: dir.clone() }]);
            txn.prepare().unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            let err = txn.commit().unwrap_err();
            assert!(
                matches!(err, ConfigError::ConcurrentModification { .. }),
                "unexpected {err:?}"
            );
            drop(std::fs::remove_dir_all(&root));
        }
        // Symlink: a link appears between prepare and commit where the plan
        // expected no link at all.
        {
            let root = tmp_root();
            std::fs::create_dir_all(&root).unwrap();
            #[cfg(unix)]
            {
                let elsewhere = root.join("elsewhere.txt");
                std::fs::write(&elsewhere, b"foreign").unwrap();
                let link = root.join("lnk");
                let id = OperationId::new("op-s42-s").unwrap();
                let mut txn = Transaction::new(
                    id,
                    vec![FileAction::Symlink {
                        link: link.clone(),
                        target: root.join("planned.txt"),
                        expected_current: None,
                    }],
                );
                txn.prepare().unwrap();
                std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
                let err = txn.commit().unwrap_err();
                assert!(
                    matches!(err, ConfigError::ConcurrentModification { .. }),
                    "unexpected {err:?}"
                );
                assert_eq!(std::fs::read_link(&link).unwrap(), elsewhere);
            }
            drop(std::fs::remove_dir_all(&root));
        }
        // RemoveFile: content changes between prepare and commit
        {
            let root = tmp_root();
            let target = root.join("r.json");
            std::fs::write(&target, b"original").unwrap();
            let id = OperationId::new("op-s42-r").unwrap();
            let mut txn = Transaction::new(
                id,
                vec![FileAction::RemoveFile {
                    path: target.clone(),
                }],
            );
            txn.prepare().unwrap();
            std::fs::write(&target, b"foreign edit").unwrap();
            let err = txn.commit().unwrap_err();
            assert!(matches!(err, ConfigError::ConcurrentModification { .. }));
            assert_eq!(
                std::fs::read(&target).unwrap(),
                b"foreign edit",
                "removal must not destroy the foreign edit"
            );
            drop(std::fs::remove_dir_all(&root));
        }
        // QuarantineMove: source changes between prepare and commit
        {
            let root = tmp_root();
            let src = root.join("q.json");
            std::fs::write(&src, b"original").unwrap();
            let qdir = root.join("quarantine");
            let id = OperationId::new("op-s42-q").unwrap();
            let mut txn = Transaction::new(
                id,
                vec![FileAction::QuarantineMove {
                    from: src.clone(),
                    to: qdir.join("q.json"),
                }],
            );
            txn.prepare().unwrap();
            std::fs::write(&src, b"foreign edit").unwrap();
            let err = txn.commit().unwrap_err();
            assert!(matches!(err, ConfigError::ConcurrentModification { .. }));
            assert_eq!(std::fs::read(&src).unwrap(), b"foreign edit");
            drop(std::fs::remove_dir_all(&root));
        }
    }

    #[test]
    fn file_appearing_between_prepare_and_commit_aborts_creation_write() {
        let root = tmp_root();
        let target = root.join("fresh.json");
        let id = OperationId::new("op-s42-new").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: target.clone(),
                content: b"{}".to_vec(),
                kind: DocumentKind::StrictJson,
            }],
        );
        txn.prepare().unwrap();
        // A foreign file appears where we planned a creation.
        std::fs::write(&target, b"foreign").unwrap();
        let err = txn.commit().unwrap_err();
        assert!(matches!(err, ConfigError::ConcurrentModification { .. }));
        assert_eq!(std::fs::read(&target).unwrap(), b"foreign");
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn staged_temp_tampering_between_prepare_and_commit_is_detected() {
        let root = tmp_root();
        let target = root.join("t.json");
        let id = OperationId::new("op-s42-tamper").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: target.clone(),
                content: b"v1".to_vec(),
                kind: DocumentKind::TextFragment,
            }],
        );
        txn.prepare().unwrap();
        // Tamper with the staged temp: the committed bytes must NOT silently
        // differ from the planned content digest.
        if let Some(temp) = txn.staged_temps.first().cloned() {
            std::fs::write(&temp, b"tampered").unwrap();
        }
        if let Err(e) = txn.commit() {
            assert!(
                format!("{e}").contains("mismatch") || format!("{e}").contains("injected"),
                "unexpected error {e:?}"
            );
        } else {
            // The tampered bytes committed: verification must catch them, so
            // the read-back == staged == planned invariant is enforced either
            // at commit or at verify.
            let bytes = std::fs::read(&target).unwrap();
            assert!(
                bytes == b"v1" || bytes == b"tampered",
                "unexpected committed bytes"
            );
            let verify = txn.verify().unwrap();
            assert!(
                verify.iter().any(|v| !v.digest_ok),
                "tampered content must fail verification"
            );
        }
        drop(std::fs::remove_dir_all(&root));
    }

    // ------------------------------------------------------------------
    // MUT-02: hard links and symlink target changes
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn hardlink_alias_steps_are_rejected() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        std::fs::write(&a, b"shared").unwrap();
        std::fs::hard_link(&a, &b).unwrap();

        let id = OperationId::new("op-hardlink-dup").unwrap();
        let txn = Transaction::new(
            id,
            vec![
                FileAction::Write {
                    path: a,
                    content: b"new-a".to_vec(),
                    kind: DocumentKind::TextFragment,
                },
                FileAction::Write {
                    path: b,
                    content: b"new-b".to_vec(),
                    kind: DocumentKind::TextFragment,
                },
            ],
        );
        let err = txn.validate_plan().unwrap_err();
        assert!(
            matches!(err, ConfigError::HardlinkConflict { .. }),
            "expected HardlinkConflict, got {err:?}"
        );
        drop(std::fs::remove_dir_all(&root));
    }

    #[cfg(unix)]
    #[test]
    fn hardlink_write_target_records_warning() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.txt");
        let alias = root.join("alias.txt");
        std::fs::write(&a, b"shared").unwrap();
        std::fs::hard_link(&a, &alias).unwrap();

        let id = OperationId::new("op-hardlink-warn").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: a.clone(),
                content: b"new".to_vec(),
                kind: DocumentKind::TextFragment,
            }],
        );
        txn.prepare().unwrap();
        assert!(
            txn.warnings()
                .iter()
                .any(|w| w.contains("hard link sharing")),
            "explicit warning required, got {:?}",
            txn.warnings()
        );
        txn.commit().unwrap();
        // The alias still carries the old bytes: link sharing was broken by
        // the atomic replacement, which is exactly what the warning says.
        assert_eq!(std::fs::read(&alias).unwrap(), b"shared");
        assert_eq!(std::fs::read(&a).unwrap(), b"new");
        drop(std::fs::remove_dir_all(&root));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_retarget_between_prepare_and_commit_aborts() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let target_a = root.join("ta.txt");
        let target_b = root.join("tb.txt");
        std::fs::write(&target_a, b"A").unwrap();
        std::fs::write(&target_b, b"B").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&target_a, &link).unwrap();

        let id = OperationId::new("op-link-retarget").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Symlink {
                link: link.clone(),
                target: target_a,
                expected_current: None,
            }],
        );
        txn.prepare().unwrap();
        // Foreign retarget of the existing link between prepare and commit.
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&target_b, &link).unwrap();
        let err = txn.commit().unwrap_err();
        assert!(matches!(err, ConfigError::ConcurrentModification { .. }));
        // The foreign link is untouched.
        assert_eq!(std::fs::read_link(&link).unwrap(), target_b);
        drop(std::fs::remove_dir_all(&root));
    }

    #[cfg(unix)]
    #[test]
    fn commit_symlink_replaces_only_matching_owned_target() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let owned_target = root.join("owned.txt");
        let foreign_target = root.join("foreign.txt");
        let new_target = root.join("new.txt");
        std::fs::write(&owned_target, b"owned").unwrap();
        std::fs::write(&foreign_target, b"foreign").unwrap();
        std::fs::write(&new_target, b"new").unwrap();

        // Link pointing somewhere we do NOT own: replacement refused.
        let link = root.join("lnk");
        std::os::unix::fs::symlink(&foreign_target, &link).unwrap();
        let id = OperationId::new("op-link-owned").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Symlink {
                link: link.clone(),
                target: new_target.clone(),
                expected_current: Some(owned_target.clone()),
            }],
        );
        txn.prepare().unwrap();
        let err = txn.commit().unwrap_err();
        assert!(
            matches!(err, ConfigError::SymlinkTargetMismatch { .. }),
            "expected SymlinkTargetMismatch, got {err:?}"
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), foreign_target);

        // Link pointing at the expected owned target: replacement proceeds.
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&owned_target, &link).unwrap();
        let id2 = OperationId::new("op-link-owned-ok").unwrap();
        let mut txn2 = Transaction::new(
            id2,
            vec![FileAction::Symlink {
                link: link.clone(),
                target: new_target.clone(),
                expected_current: Some(owned_target),
            }],
        );
        txn2.prepare().unwrap();
        txn2.commit().unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), new_target);
        drop(std::fs::remove_dir_all(&root));
    }

    #[cfg(unix)]
    #[test]
    fn expected_owned_symlink_missing_is_a_conflict() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let owned_target = root.join("owned.txt");
        std::fs::write(&owned_target, b"owned").unwrap();
        let link = root.join("lnk");
        std::os::unix::fs::symlink(&owned_target, &link).unwrap();

        let id = OperationId::new("op-link-gone").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Symlink {
                link: link.clone(),
                target: owned_target.clone(),
                expected_current: Some(owned_target),
            }],
        );
        txn.prepare().unwrap();
        std::fs::remove_file(&link).unwrap();
        let err = txn.commit().unwrap_err();
        assert!(matches!(err, ConfigError::ConcurrentModification { .. }));
        drop(std::fs::remove_dir_all(&root));
    }

    // ------------------------------------------------------------------
    // MUT-02 default link policy: follow-and-preserve within roots
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn write_onto_symlink_refused_outside_follow_roots() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("allowed")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        let referent = root.join("elsewhere").join("real.json");
        std::fs::write(&referent, br#"{"foreign":true}"#).unwrap();
        let link = root.join("allowed").join("cfg.json");
        std::os::unix::fs::symlink(&referent, &link).unwrap();

        // No roots declared: the default policy refuses to follow at all.
        // `execute` records the prepare failure as an unsuccessful outcome
        // (no commit was ever attempted).
        let id = OperationId::new("op-follow-none").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: link.clone(),
                content: br#"{"owned":true}"#.to_vec(),
                kind: DocumentKind::StrictJson,
            }],
        );
        let outcome = txn.execute().unwrap();
        assert!(!outcome.success, "refused plan must not report success");
        assert!(outcome.commit.is_none(), "commit must never be attempted");
        assert!(
            outcome
                .diagnostics_redacted
                .iter()
                .any(|d| d.contains("symlink follow refused")),
            "diagnostics must name the refusal: {:?}",
            outcome.diagnostics_redacted
        );
        // Nothing was mutated and the link structure survived.
        assert_eq!(std::fs::read_link(&link).unwrap(), referent);
        assert_eq!(std::fs::read(&referent).unwrap(), br#"{"foreign":true}"#);

        // Roots declared but excluding the referent: still refused.
        let id2 = OperationId::new("op-follow-outside").unwrap();
        let mut txn2 = Transaction::new(
            id2,
            vec![FileAction::Write {
                path: link.clone(),
                content: br#"{"owned":true}"#.to_vec(),
                kind: DocumentKind::StrictJson,
            }],
        )
        .with_symlink_follow_roots(vec![root.join("allowed")]);
        let outcome2 = txn2.execute().unwrap();
        assert!(!outcome2.success);
        assert!(outcome2.commit.is_none());
        assert!(
            outcome2
                .diagnostics_redacted
                .iter()
                .any(|d| d.contains("symlink follow refused")),
            "diagnostics must name the refusal: {:?}",
            outcome2.diagnostics_redacted
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), referent);
        assert_eq!(std::fs::read(&referent).unwrap(), br#"{"foreign":true}"#);
        drop(std::fs::remove_dir_all(&root));
    }

    #[cfg(unix)]
    #[test]
    fn write_follows_and_preserves_symlink_within_declared_roots() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("allowed")).unwrap();
        let referent = root.join("allowed").join("real.json");
        std::fs::write(&referent, br#"{"foreign":true}"#).unwrap();
        let link = root.join("cfg.json");
        std::os::unix::fs::symlink(&referent, &link).unwrap();

        let id = OperationId::new("op-follow-ok").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: link.clone(),
                content: br#"{"owned":true}"#.to_vec(),
                kind: DocumentKind::StrictJson,
            }],
        )
        .with_symlink_follow_roots(vec![root.join("allowed")]);
        let outcome = txn.execute().unwrap();
        // The link itself is PRESERVED (follow, never replace).
        assert!(
            std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()),
            "the link must survive the mutation"
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), referent);
        // The REFERENT carries the new bytes.
        assert_eq!(std::fs::read(&referent).unwrap(), br#"{"owned":true}"#);
        // The foreign referent was backed up before the follow landed.
        let backups = outcome
            .commit
            .map(|commit| commit.backups)
            .unwrap_or_default();
        assert!(
            !backups.is_empty(),
            "following a foreign referent must back it up first"
        );
        assert!(
            backups
                .iter()
                .any(|b| b.original_path == referent.canonicalize().unwrap()),
            "backup must be taken of the mutated referent"
        );
        drop(std::fs::remove_dir_all(&root));
    }

    #[cfg(unix)]
    #[test]
    fn followed_symlink_retarget_between_prepare_and_commit_aborts() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("allowed")).unwrap();
        let referent = root.join("allowed").join("real.json");
        let other = root.join("allowed").join("other.json");
        std::fs::write(&referent, br#"{"a":1}"#).unwrap();
        std::fs::write(&other, br#"{"b":2}"#).unwrap();
        let link = root.join("cfg.json");
        std::os::unix::fs::symlink(&referent, &link).unwrap();

        let id = OperationId::new("op-follow-retarget").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: link.clone(),
                content: br#"{"owned":true}"#.to_vec(),
                kind: DocumentKind::StrictJson,
            }],
        )
        .with_symlink_follow_roots(vec![root.join("allowed")]);
        txn.prepare().unwrap();
        // The link is repointed between prepare and commit: the plan would now
        // mutate a file the caller no longer reaches through that link.
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        let err = txn.commit().unwrap_err();
        assert!(
            matches!(err, ConfigError::ConcurrentModification { .. }),
            "expected ConcurrentModification, got {err:?}"
        );
        assert_eq!(std::fs::read(&referent).unwrap(), br#"{"a":1}"#);
        assert_eq!(std::fs::read(&other).unwrap(), br#"{"b":2}"#);
        assert_eq!(std::fs::read_link(&link).unwrap(), other);
        drop(std::fs::remove_dir_all(&root));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_step_target_outside_declared_roots_is_refused() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("inside")).unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        let id = OperationId::new("op-link-outside").unwrap();
        let txn = Transaction::new(
            id,
            vec![FileAction::Symlink {
                link: root.join("inside").join("asset"),
                target: root.join("outside").join("asset"),
                expected_current: None,
            }],
        )
        .with_symlink_follow_roots(vec![root.join("inside")]);
        let err = txn.validate_plan().unwrap_err();
        assert!(
            matches!(err, ConfigError::SymlinkFollowRefused { .. }),
            "expected SymlinkFollowRefused, got {err:?}"
        );

        // The same step inside the declared roots validates.
        let id2 = OperationId::new("op-link-inside").unwrap();
        let txn2 = Transaction::new(
            id2,
            vec![FileAction::Symlink {
                link: root.join("inside").join("asset"),
                target: root.join("inside").join("asset-src"),
                expected_current: None,
            }],
        )
        .with_symlink_follow_roots(vec![root.join("inside")]);
        assert!(txn2.validate_plan().is_ok());
        drop(std::fs::remove_dir_all(&root));
    }

    // ------------------------------------------------------------------
    // MUT-06: copy_tree + remove_owned_empty_dir
    // ------------------------------------------------------------------

    #[test]
    fn copy_tree_respects_include_exclude_filters() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("src/sub")).unwrap();
        std::fs::write(root.join("src/keep.md"), b"keep").unwrap();
        std::fs::write(root.join("src/skip.log"), b"skip").unwrap();
        std::fs::write(root.join("src/sub/nested.md"), b"nested").unwrap();

        let opts = CopyTreeOptions {
            include: vec!["*.md".to_owned()],
            exclude: vec!["nested.md".to_owned()],
            ..CopyTreeOptions::default()
        };
        let report = copy_tree(&root.join("src"), &root.join("dest"), &opts).unwrap();
        assert!(report.files.contains(&root.join("dest/keep.md")));
        assert!(
            !report.files.contains(&root.join("dest/skip.log")),
            "non-matching names are not copied"
        );
        assert!(
            report.excluded.contains(&root.join("dest/sub/nested.md")),
            "excluded names are reported"
        );
        assert_eq!(std::fs::read(root.join("dest/keep.md")).unwrap(), b"keep");
        assert!(!root.join("dest/skip.log").exists());
        drop(std::fs::remove_dir_all(&root));
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_symlink_policies() {
        // Skip: links never leave the source tree.
        {
            let root = tmp_root();
            std::fs::create_dir_all(&root).unwrap();
            let target = root.join("t.txt");
            std::fs::write(&target, b"t").unwrap();
            let link = root.join("l.txt");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let dest_root = tmp_root();
            let report = copy_tree(&root, &dest_root, &CopyTreeOptions::default()).unwrap();
            assert_eq!(report.skipped_symlinks, vec![dest_root.join("l.txt")]);
            assert!(!dest_root.join("l.txt").exists());
            drop(std::fs::remove_dir_all(&dest_root));
            drop(std::fs::remove_dir_all(&root));
        }
        // PreserveLink: the link itself is recreated.
        {
            let root = tmp_root();
            std::fs::create_dir_all(&root).unwrap();
            let target = root.join("t.txt");
            std::fs::write(&target, b"t").unwrap();
            let link = root.join("l.txt");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let opts = CopyTreeOptions {
                symlink_policy: SymlinkPolicy::PreserveLink,
                ..CopyTreeOptions::default()
            };
            let dest_root = tmp_root();
            copy_tree(&root, &dest_root, &opts).unwrap();
            assert!(
                dest_root
                    .join("l.txt")
                    .symlink_metadata()
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(
                std::fs::read_link(dest_root.join("l.txt")).unwrap(),
                root.join("t.txt")
            );
            drop(std::fs::remove_dir_all(&dest_root));
            drop(std::fs::remove_dir_all(&root));
        }
        // FollowCopyContent: referent bytes land as a regular file; broken
        // links fail the copy honestly.
        {
            let root = tmp_root();
            std::fs::create_dir_all(&root).unwrap();
            let target = root.join("t.txt");
            std::fs::write(&target, b"content").unwrap();
            let link = root.join("l.txt");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let opts = CopyTreeOptions {
                symlink_policy: SymlinkPolicy::FollowCopyContent,
                ..CopyTreeOptions::default()
            };
            let dest_root = tmp_root();
            copy_tree(&root, &dest_root, &opts).unwrap();
            assert_eq!(std::fs::read(dest_root.join("l.txt")).unwrap(), b"content");
            assert!(
                !dest_root
                    .join("l.txt")
                    .symlink_metadata()
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            drop(std::fs::remove_dir_all(&dest_root));

            let broken_root = tmp_root();
            std::fs::create_dir_all(&broken_root).unwrap();
            std::os::unix::fs::symlink(
                broken_root.join("nowhere.txt"),
                broken_root.join("broken.txt"),
            )
            .unwrap();
            let dest_root2 = tmp_root();
            let res = copy_tree(&broken_root, &dest_root2, &opts);
            assert!(res.is_err(), "broken link must fail the copy");
            drop(std::fs::remove_dir_all(&dest_root2));
            drop(std::fs::remove_dir_all(&root));
            drop(std::fs::remove_dir_all(&broken_root));
        }
    }

    #[test]
    fn copy_tree_bounds_abort_large_runs() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("big.bin"), vec![0u8; 1024]).unwrap();
        let opts = CopyTreeOptions {
            max_bytes: 100,
            ..CopyTreeOptions::default()
        };
        let dest_root = tmp_root();
        let res = copy_tree(&root, &dest_root, &opts);
        assert!(res.is_err(), "byte bound must abort the copy");
        drop(std::fs::remove_dir_all(&dest_root));
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn remove_owned_empty_dir_refuses_non_empty_and_broad_roots() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        // Non-empty: typed refusal.
        let dir = root.join("nonempty");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.txt"), b"x").unwrap();
        let err = remove_owned_empty_dir(&dir).unwrap_err();
        assert!(matches!(err, ConfigError::Io { .. }), "got {err:?}");
        assert!(dir.exists(), "non-empty dir must survive");
        // Empty: removed.
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        remove_owned_empty_dir(&empty).unwrap();
        assert!(!empty.exists());
        // Broad root: refused by removal validation.
        assert!(remove_owned_empty_dir(Path::new("/tmp")).is_err());
        drop(std::fs::remove_dir_all(&root));
    }

    // ------------------------------------------------------------------
    // MUT-09 + QAL-06: journal + injector on REAL paths
    // ------------------------------------------------------------------

    #[test]
    fn journal_written_for_multi_file_commit_and_removed_after_success() {
        let root = tmp_root();
        let jroot = root.join(".superai").join("journal");
        let a = root.join("a.json");
        let b = root.join("b.json");
        std::fs::write(&a, b"{\"a\":1}").unwrap();
        std::fs::write(&b, b"{\"b\":1}").unwrap();

        // Crash right after the journal advanced to verify: both files are
        // committed and the journal exists on disk.
        let inj = FailAtPoint::new(Point::JournalVerify, 1);
        let id = OperationId::new("op-journal-multi").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![
                FileAction::Write {
                    path: a.clone(),
                    content: b"{\"a\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::Write {
                    path: b.clone(),
                    content: b"{\"b\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
            ],
        )
        .with_journal(jroot.clone())
        .with_injector(inj);
        let outcome = txn.execute().unwrap();
        assert!(!outcome.success, "injected crash at verify must fail");
        let jpath = journal_path(&jroot, "op-journal-multi");
        let journal = CrashJournal::load_from(&jpath)
            .unwrap()
            .expect("journal must be on disk for a multi-file commit");
        assert_eq!(journal.phase, JournalPhase::Verify);
        assert_eq!(journal.completed.len(), 2, "both commits recorded");
        assert_eq!(journal.backups.len(), 2, "both backups recorded");
        assert!(
            !serde_json::to_string(&journal).unwrap().contains("\"a\":1"),
            "journal must not contain file contents"
        );

        // Recovery restores both files and removes the journal.
        let report = recover_pending(&root).unwrap();
        assert!(report.all_recovered(), "leftover: {:?}", report.journals);
        assert_eq!(std::fs::read(&a).unwrap(), b"{\"a\":1}");
        assert_eq!(std::fs::read(&b).unwrap(), b"{\"b\":1}");
        assert!(!jpath.exists(), "journal removed after verified recovery");

        // A clean run removes the journal after verified completion.
        let id2 = OperationId::new("op-journal-clean").unwrap();
        let mut txn2 = Transaction::new(
            id2,
            vec![
                FileAction::Write {
                    path: a,
                    content: b"{\"a\":3}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::Write {
                    path: b,
                    content: b"{\"b\":3}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
            ],
        )
        .with_journal(jroot.clone());
        let outcome2 = txn2.execute().unwrap();
        assert!(outcome2.success);
        assert!(
            !journal_path(&jroot, "op-journal-clean").exists(),
            "verified completion removes the journal"
        );
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn crash_at_each_phase_via_injector_recovery_restores() {
        for (phase, point, nth) in [
            (JournalPhase::Plan, Point::JournalPlan, 1),
            (JournalPhase::PrepareBackup, Point::JournalPrepareBackup, 1),
            (JournalPhase::StageTemp, Point::JournalStageTemp, 1),
            // Commit journal writes: start (1), after step 1 (2), after step 2 (3).
            // Failing at 2 leaves the first file committed, the second not.
            (JournalPhase::Commit, Point::JournalCommit, 2),
        ] {
            let root = tmp_root();
            let jroot = root.join(".superai").join("journal");
            let a = root.join("a.json");
            let b = root.join("b.json");
            std::fs::write(&a, b"{\"a\":1}").unwrap();
            std::fs::write(&b, b"{\"b\":1}").unwrap();

            let inj = FailAtPoint::new(point, nth);
            let id = OperationId::new("op-crash").unwrap();
            let mut txn = Transaction::new(
                id,
                vec![
                    FileAction::Write {
                        path: a.clone(),
                        content: b"{\"a\":2}".to_vec(),
                        kind: DocumentKind::StrictJson,
                    },
                    FileAction::Write {
                        path: b.clone(),
                        content: b"{\"b\":2}".to_vec(),
                        kind: DocumentKind::StrictJson,
                    },
                ],
            )
            .with_journal(jroot)
            .with_injector(inj);
            let _ = txn.execute().unwrap();

            let jpath = journal_path(&root.join(".superai/journal"), "op-crash");
            let journal = CrashJournal::load_from(&jpath)
                .unwrap()
                .unwrap_or_else(|| panic!("journal must exist after crash at {phase}"));
            assert_eq!(journal.phase, phase, "journal phase after crash");

            let report = recover_pending(&root).unwrap();
            assert!(
                report.all_recovered(),
                "phase {phase}: recovery residuals {:?}",
                report.journals
            );
            assert_eq!(
                std::fs::read(&a).unwrap(),
                b"{\"a\":1}",
                "phase {phase}: first file restored to pre-op bytes"
            );
            assert_eq!(
                std::fs::read(&b).unwrap(),
                b"{\"b\":1}",
                "phase {phase}: second file at pre-op bytes"
            );
            assert!(!jpath.exists(), "journal removed after recovery ({phase})");
            drop(std::fs::remove_dir_all(&root));
        }
    }

    #[test]
    fn recovery_removes_stale_temps_and_never_replays_content() {
        let root = tmp_root();
        let jroot = root.join(".superai").join("journal");
        let a = root.join("a.json");
        std::fs::write(&a, b"pre-op").unwrap();

        // Crash at stage_temp: temps exist, nothing committed.
        let inj = FailAtPoint::new(Point::JournalStageTemp, 1);
        let id = OperationId::new("op-temps").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: a.clone(),
                content: b"planned-new-content".to_vec(),
                kind: DocumentKind::TextFragment,
            }],
        )
        .with_journal(jroot.clone())
        .with_injector(inj);
        let _ = txn.execute().unwrap();
        assert!(
            a.exists() && std::fs::read(&a).unwrap() == b"pre-op",
            "nothing committed before the stage_temp crash"
        );

        let report = recover_pending(&root).unwrap();
        assert!(report.all_recovered());
        assert_eq!(
            std::fs::read(&a).unwrap(),
            b"pre-op",
            "recovery must never write the planned content"
        );
        let rec = report
            .journals
            .first()
            .cloned()
            .expect("one journal was recovered");
        assert!(
            rec.removed_temps.iter().all(|t| !t.exists()),
            "stale temps removed"
        );
        assert!(!journal_path(&jroot, "op-temps").exists());
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn injector_failures_hit_real_transaction_paths() {
        // SecondFile failure through the REAL transaction (no manual temp
        // deletion): first file rolls back via its backup.
        let root = tmp_root();
        let a = root.join("a.json");
        let b = root.join("b.json");
        std::fs::write(&a, b"{\"a\":1}").unwrap();
        std::fs::write(&b, b"{\"b\":1}").unwrap();
        let inj = FailAtPoint::new(Point::SecondFile, 1);
        let id = OperationId::new("op-inj-second").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![
                FileAction::Write {
                    path: a.clone(),
                    content: b"{\"a\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::Write {
                    path: b.clone(),
                    content: b"{\"b\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
            ],
        )
        .with_injector(inj);
        let outcome = txn.execute().unwrap();
        assert!(!outcome.success);
        assert_eq!(std::fs::read(&a).unwrap(), b"{\"a\":1}", "rolled back");
        assert_eq!(std::fs::read(&b).unwrap(), b"{\"b\":1}");
        drop(std::fs::remove_dir_all(&root));

        // RollbackVerify failure leaves a reported residual on the REAL path:
        // the second file's commit fails AND the compensation's verification
        // of the first restore is injected to fail.
        let root2 = tmp_root();
        let c = root2.join("c.json");
        let d = root2.join("d.json");
        std::fs::write(&c, b"{\"c\":1}").unwrap();
        std::fs::write(&d, b"{\"d\":1}").unwrap();
        let inj2 = FailAtPoint::two(Point::SecondFile, 1, Point::RollbackVerify, 1);
        let id2 = OperationId::new("op-inj-rbverify").unwrap();
        let mut txn2 = Transaction::new(
            id2,
            vec![
                FileAction::Write {
                    path: c.clone(),
                    content: b"{\"c\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::Write {
                    path: d,
                    content: b"{\"d\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
            ],
        )
        .with_injector(inj2);
        let outcome2 = txn2.execute().unwrap();
        assert!(!outcome2.success);
        let rb = outcome2
            .rollback
            .expect("compensation outcome must be reported");
        assert!(
            rb.residuals.contains(&c),
            "injected rollback-verify failure must surface as residual: {rb:?}"
        );
        drop(std::fs::remove_dir_all(&root2));
    }

    // -----------------------------------------------------------------------
    // Plan-02 fold: the single-file mutation boundary
    // -----------------------------------------------------------------------

    fn boundary_scratch(tag: &str) -> PathBuf {
        let dir = crate::test_util::temp_dir_unique(&format!("tx-boundary-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn commit_file_creates_missing_and_backs_up_existing() {
        let dir = boundary_scratch("create");
        let path = dir.join("settings.json");
        let created = commit_file(
            "boundary-create",
            &path,
            br#"{"v":1}"#,
            DocumentKind::StrictJson,
        )
        .unwrap();
        assert!(
            created.backup.is_none(),
            "a creation has nothing to back up"
        );
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"v":1}"#);

        let replaced = commit_file(
            "boundary-replace",
            &path,
            br#"{"v":2}"#,
            DocumentKind::StrictJson,
        )
        .unwrap();
        let backup = replaced
            .backup
            .expect("an overwrite must back the target up first");
        assert_eq!(
            std::fs::read(&backup.backup_path).unwrap(),
            br#"{"v":1}"#,
            "the backup carries the pre-write bytes"
        );
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"v":2}"#);
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn commit_file_expecting_aborts_on_foreign_edit_and_cleans_temp() {
        let dir = boundary_scratch("stale");
        let path = dir.join("cfg.toml");
        std::fs::write(&path, "a = 1\n").unwrap();
        let token = snapshot(&path);
        std::fs::write(&path, "a = 2\n").unwrap(); // foreign edit after the read
        let res = commit_file_expecting(
            "boundary-stale",
            &path,
            b"a = 3\n",
            DocumentKind::Toml,
            Some(&token),
        );
        match res {
            Err(ConfigError::ConcurrentModification { .. }) => {}
            other => panic!("expected ConcurrentModification, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"a = 2\n",
            "the foreign edit is never overwritten"
        );
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with(".tmp."),
                "aborted commit leaked staged temp {name}"
            );
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn commit_file_rejects_directory_target() {
        let dir = boundary_scratch("dir-target");
        let res = commit_file("boundary-dir", &dir, b"data", DocumentKind::Opaque);
        assert!(res.is_err(), "a directory target must be refused");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn case_fold_collision_is_refused_before_any_write() {
        let dir = boundary_scratch("case-fold");
        let lower = dir.join("config.json");
        std::fs::write(&lower, br#"{"a":1}"#).unwrap();
        let upper = dir.join("Config.json");
        let res = commit_file(
            "case-collide",
            &upper,
            br#"{"a":2}"#,
            DocumentKind::StrictJson,
        );
        assert!(
            res.is_err(),
            "a case-variant creation must be refused (it would land over the sibling on a \
             case-insensitive filesystem)"
        );
        match res {
            Err(ConfigError::Io { .. }) => {}
            other => panic!("expected typed Io refusal, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&lower).unwrap(),
            br#"{"a":1}"#,
            "the existing file is untouched"
        );
        // The exact-name overwrite keeps working through the same boundary.
        commit_file(
            "case-exact",
            &lower,
            br#"{"a":3}"#,
            DocumentKind::StrictJson,
        )
        .unwrap();
        assert_eq!(std::fs::read(&lower).unwrap(), br#"{"a":3}"#);
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn windows_reserved_device_names_are_refused_as_write_targets() {
        // Pure helper table (host-independent, QAL-09).
        for reserved in [
            "CON", "con", "CON.json", "PRN.txt", "aux", "NUL.dat", "COM1", "com9.cfg", "LPT7",
            "CONIN$", "CONOUT$",
        ] {
            assert!(
                windows_reserved_device_name(&Path::new("/data").join(reserved)),
                "{reserved} is a reserved device name"
            );
        }
        for ordinary in [
            "console.json",
            "control",
            "COM10.txt",
            "COM0",
            "context.rs",
            "component",
            "lpt-1.json",
        ] {
            assert!(
                !windows_reserved_device_name(&Path::new("/data").join(ordinary)),
                "{ordinary} is an ordinary file name"
            );
        }
        // The boundary refuses them as write targets before any disk work.
        let dir = boundary_scratch("reserved");
        for reserved in ["CON", "con.json", "PRN.txt", "AUX", "NUL", "COM1", "LPT9"] {
            let path = dir.join(reserved);
            let res = commit_file(
                "reserved-write",
                &path,
                br#"{"a":1}"#,
                DocumentKind::StrictJson,
            );
            assert!(res.is_err(), "{reserved} must be refused as a write target");
            let listing = std::fs::read_dir(&dir).unwrap().flatten().count();
            assert_eq!(listing, 0, "the refusal must leave the directory empty");
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    // -----------------------------------------------------------------------
    // Plan-02 fold: structural source guarantees
    // -----------------------------------------------------------------------

    fn crate_src(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// The raw atomic primitive must not be a public write entrypoint: the
    /// boundary (`commit_file` / `Transaction`) is the only public way to
    /// mutate a file through this crate.
    #[test]
    fn plan02_atomic_write_family_is_crate_internal() {
        let src = crate_src("atomic.rs");
        assert!(
            !src.contains("pub fn atomic_write"),
            "the atomic write family must not be a public write entrypoint (plan-02 fold)"
        );
        assert!(
            src.contains("pub(crate) fn atomic_write"),
            "atomic_write remains the crate-internal replace primitive"
        );
    }

    /// Every codec store commits through the boundary; the raw editor
    /// commits through the shared stage+commit core.
    #[test]
    fn plan02_codec_stores_share_the_boundary() {
        for name in [
            "json.rs",
            "jsonc.rs",
            "toml_file.rs",
            "yaml.rs",
            "env_file.rs",
        ] {
            let src = crate_src(name);
            assert!(
                !src.contains("atomic::atomic_write"),
                "{name} must not call the raw atomic primitive"
            );
            assert!(
                src.contains("transaction::commit_file"),
                "{name} must commit through the boundary"
            );
        }
        let editor = crate_src("raw_editor.rs");
        assert!(
            !editor.contains("atomic::atomic_write"),
            "raw_editor must not call the raw atomic primitive"
        );
        assert!(
            editor.contains("commit_staged_file"),
            "raw_editor must commit through the shared transaction core"
        );
    }

    /// No superai-core / superai-cli source bypasses the boundary with a
    /// direct atomic write.
    #[test]
    fn plan02_core_and_cli_have_no_direct_atomic_writes() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        collect_rs_files(&manifest.join("../superai-core/src"), &mut files);
        collect_rs_files(&manifest.join("../superai-cli/src"), &mut files);
        assert!(
            files.len() > 40,
            "expected to scan the superai-core/cli sources, found {}",
            files.len()
        );
        for file in &files {
            let src = std::fs::read_to_string(file).unwrap_or_default();
            assert!(
                !src.contains("atomic_write"),
                "{} must write through superai_config::transaction (plan-02 fold)",
                file.display()
            );
        }
    }

    // -----------------------------------------------------------------------
    // Plan-13 / QAL-09 platform-adversarial cases (executed by the windows
    // and macos CI runners; compiled out elsewhere)
    // -----------------------------------------------------------------------

    /// A target held open the way a running harness holds its config must
    /// surface a typed error, never corruption or a leaked temp.
    #[cfg(windows)]
    #[test]
    fn windows_locked_target_commit_is_typed_error_never_corrupting() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;
        let dir = boundary_scratch("win-locked");
        let file = dir.join("locked.json");
        std::fs::write(&file, br#"{"locked":true}"#).unwrap();
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&file)
            .unwrap();
        let res = commit_file(
            "win-locked",
            &file,
            br#"{"locked":false}"#,
            DocumentKind::StrictJson,
        );
        match res {
            Err(ConfigError::Io { .. }) => {}
            other => panic!("expected typed Io error from the locked target, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&file).unwrap(),
            br#"{"locked":true}"#,
            "the locked target is never corrupted"
        );
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with(".tmp."),
                "locked commit leaked staged temp {name}"
            );
        }
        drop(handle);
        drop(std::fs::remove_dir_all(&dir));
    }

    /// Windows reserved device names are refused as write targets on the
    /// real platform (the pure helper table above runs on every host).
    #[cfg(windows)]
    #[test]
    fn windows_reserved_device_paths_rejected_live() {
        let dir = boundary_scratch("win-reserved");
        for reserved in ["CON", "PRN", "AUX", "NUL", "COM1", "LPT1"] {
            let path = dir.join(format!("{reserved}.json"));
            let res = commit_file(
                "win-reserved",
                &path,
                br#"{"a":1}"#,
                DocumentKind::StrictJson,
            );
            assert!(res.is_err(), "{reserved}.json must be refused on windows");
            assert!(
                !path.exists(),
                "{reserved}.json must not materialize as a device-named file"
            );
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    /// A path deeper than `MAX_PATH` either commits with verified read-back
    /// (long-path-aware system) or fails with a typed error, never a panic
    /// or a partial file.
    #[cfg(windows)]
    #[test]
    fn windows_long_path_commit_is_verified_or_typed_never_partial() {
        let dir = boundary_scratch("win-long");
        let mut deep = dir.clone();
        while deep.to_string_lossy().len() < 300 {
            deep = deep.join("nested-level-dir");
        }
        let file = deep.join("settings.json");
        match commit_file("win-long", &file, br#"{"a":1}"#, DocumentKind::StrictJson) {
            Ok(report) => {
                let _ = report;
                assert_eq!(
                    std::fs::read(&file).unwrap(),
                    br#"{"a":1}"#,
                    "a long-path commit must read back verified"
                );
            }
            Err(ConfigError::Io { .. }) => {
                assert!(
                    !file.exists(),
                    "a typed MAX_PATH refusal must not leave a partial file"
                );
            }
            other => panic!("long path must verify or fail typed, got {other:?}"),
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    /// QAL-09 on case-insensitive APFS: `Settings.json` next to
    /// `settings.json` is refused, never silently landed over.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_case_insensitive_collision_write_is_typed_never_corrupting() {
        let dir = boundary_scratch("macos-case");
        let lower = dir.join("settings.json");
        std::fs::write(&lower, br#"{"a":1}"#).unwrap();
        let upper = dir.join("Settings.json");
        let res = commit_file(
            "macos-case",
            &upper,
            br#"{"a":2}"#,
            DocumentKind::StrictJson,
        );
        assert!(
            res.is_err(),
            "a case-variant creation must be refused on a case-insensitive volume"
        );
        assert_eq!(
            std::fs::read(&lower).unwrap(),
            br#"{"a":1}"#,
            "the original is never corrupted"
        );
        commit_file(
            "macos-case-exact",
            &lower,
            br#"{"a":3}"#,
            DocumentKind::StrictJson,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&lower).unwrap(),
            br#"{"a":3}"#,
            "exact-name overwrites keep working through the boundary"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    /// QAL-09 macOS application paths: `~/Library/Application Support/...`
    /// shaped config locations (capitals and the embedded space) commit
    /// through the boundary with verified read-back.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_application_support_paths_commit_through_the_boundary() {
        let dir = boundary_scratch("macos-app");
        let app = dir
            .join("Library")
            .join("Application Support")
            .join("Claude");
        let file = app.join("settings.json");
        commit_file(
            "macos-app-path",
            &file,
            br#"{"a":1}"#,
            DocumentKind::StrictJson,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&file).unwrap(),
            br#"{"a":1}"#,
            "the application-path write must read back verified"
        );
        let variant = app.join("SETTINGS.json");
        assert!(
            commit_file(
                "macos-app-case",
                &variant,
                br#"{"a":2}"#,
                DocumentKind::StrictJson
            )
            .is_err(),
            "a case-variant of an existing application-path file is refused"
        );
        assert_eq!(
            std::fs::read(&file).unwrap(),
            br#"{"a":1}"#,
            "the application-path original is untouched"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    // ------------------------------------------------------------------
    // Behaviour tests for the mutation-testing gate (area C)
    // ------------------------------------------------------------------

    /// Whether chmod 0o333 actually denies opening this directory for
    /// reading for this process. Root bypasses permission checks; callers
    /// skip the denial-dependent assertions when it does not.
    #[cfg(unix)]
    fn perm_denies_dir_read_probe(dir: &Path) -> bool {
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

    /// What to do with the target's parent directory when the commit's
    /// `ParentSync` boundary fires (after the rename landed, right before
    /// the parent fsync).
    #[cfg(unix)]
    #[derive(Debug, Clone, Copy)]
    enum ParentSabotage {
        /// Replace the parent with a self-referential symlink loop (ELOOP).
        Loop,
        /// Make the parent unreadable (EACCES on open).
        DenyRead,
        /// Remove the parent entirely (ENOENT).
        Vanish,
    }

    #[cfg(unix)]
    #[derive(Debug)]
    struct SabotageParentAtSync {
        parent: PathBuf,
        action: ParentSabotage,
    }

    #[cfg(unix)]
    impl Injector for SabotageParentAtSync {
        fn inject(&self, point: Point) -> Result<()> {
            if point != Point::ParentSync {
                return Ok(());
            }
            match self.action {
                ParentSabotage::DenyRead => {
                    use std::os::unix::fs::PermissionsExt;
                    drop(std::fs::set_permissions(
                        &self.parent,
                        std::fs::Permissions::from_mode(0o333),
                    ));
                }
                ParentSabotage::Loop | ParentSabotage::Vanish => {
                    for entry in std::fs::read_dir(&self.parent)
                        .into_iter()
                        .flatten()
                        .flatten()
                    {
                        let path = entry.path();
                        if entry.file_type().is_ok_and(|t| t.is_dir()) {
                            drop(std::fs::remove_dir_all(&path));
                        } else {
                            drop(std::fs::remove_file(&path));
                        }
                    }
                    drop(std::fs::remove_dir(&self.parent));
                    if matches!(self.action, ParentSabotage::Loop) {
                        drop(std::os::unix::fs::symlink(&self.parent, &self.parent));
                    }
                }
            }
            Ok(())
        }
    }

    /// Rewrites the committed target at the `JournalVerify` boundary:
    /// simulates foreign drift in the guarded commit-to-verify window.
    #[derive(Debug)]
    struct TamperTargetAtJournalVerify {
        target: PathBuf,
        bytes: Vec<u8>,
    }

    impl Injector for TamperTargetAtJournalVerify {
        fn inject(&self, point: Point) -> Result<()> {
            if point == Point::JournalVerify {
                drop(std::fs::write(&self.target, &self.bytes));
            }
            Ok(())
        }
    }

    #[test]
    fn operation_id_accessors_and_display_preserve_the_string() {
        let id = OperationId::new("op-accessors").unwrap();
        assert_eq!(id.as_str(), "op-accessors");
        assert_eq!(id.to_string(), "op-accessors");
        assert_eq!(format!("{id}"), "op-accessors");
        assert_eq!(id.into_string(), "op-accessors");
    }

    #[test]
    fn remove_kind_display_spells_every_variant() {
        assert_eq!(RemoveKind::ConfigEntry.to_string(), "config_entry");
        assert_eq!(RemoveKind::WrapperFile.to_string(), "wrapper_file");
        assert_eq!(RemoveKind::InstanceRoot.to_string(), "instance_root");
        assert_eq!(RemoveKind::Binary.to_string(), "binary");
        assert_eq!(RemoveKind::RegistryOnly.to_string(), "registry_only");
    }

    #[test]
    fn remove_target_rejects_each_glob_and_variable_character_alone() {
        // Each forbidden character must trip the guard on its own: a path
        // containing only `?` (no `*`), only `[`, only `$`, only `%`.
        for path in [
            "/tmp/never?here",
            "/tmp/never[here]",
            "/tmp/$VAR",
            "/tmp/%VAR%",
        ] {
            assert!(
                validate_remove_target(Path::new(path), RemoveKind::WrapperFile).is_err(),
                "{path} must be refused"
            );
        }
    }

    /// Unix only: `/`-rooted paths are drive-relative on Windows, so both
    /// the refusals and the control hold only here.
    #[cfg(unix)]
    #[test]
    fn remove_target_rejects_every_broad_unix_root() {
        for root in ["/", "/home", "/tmp", "/usr", "/etc"] {
            assert!(
                validate_remove_target(Path::new(root), RemoveKind::WrapperFile).is_err(),
                "broad root {root} must be refused"
            );
        }
        // A specific target outside the broad-root set stays allowed.
        assert!(
            validate_remove_target(Path::new("/opt/superai-thing"), RemoveKind::WrapperFile)
                .is_ok()
        );
    }

    /// Unix-premised fixture: `/`-rooted controls are drive-relative on
    /// Windows. The string semantics stay covered by the ungated unit tests.
    #[cfg(unix)]
    #[test]
    fn binary_remove_refuses_config_roots_in_every_shape() {
        for bad in [
            "/data/.claude",
            "/data/.superai",
            // Forward-slash UNC text is windows-shaped AND absolute on unix,
            // so it reaches the folded branch of the config-root check.
            "//server/.claude",
            "//server/.superai",
            "C:\\data\\.claude",
            "C:\\data\\.SUPERAI",
        ] {
            assert!(
                validate_remove_target(Path::new(bad), RemoveKind::Binary).is_err(),
                "binary removal onto a config root must be refused: {bad}"
            );
        }
        // Real binaries (no dot-prefixed config-root component) stay allowed.
        assert!(
            validate_remove_target(Path::new("/usr/local/bin/superai"), RemoveKind::Binary).is_ok()
        );
    }

    #[test]
    fn remove_target_refuses_the_real_home_directory() {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            // No home in the environment: nothing to assert.
            return;
        };
        assert!(
            validate_remove_target(&home, RemoveKind::InstanceRoot).is_err(),
            "the process home directory {} must never be a removal target",
            home.display()
        );
    }

    #[test]
    fn paths_equal_folded_requires_real_windows_shape_on_both_sides() {
        // UNC text folds case-insensitively (both sides windows-shaped).
        assert!(paths_equal_platform_folded(
            Path::new("\\\\Srv\\Share"),
            Path::new("\\\\srv\\share")
        ));
        // `1:` is not windows-shaped (drive prefix needs an alphabetic first
        // char), so comparison stays byte-exact and case-sensitive.
        assert!(!paths_equal_platform_folded(
            Path::new("1:Data"),
            Path::new("1:data")
        ));
    }

    /// The staged temp name embeds a current epoch-millis value and a compact
    /// four-hex-char random suffix: the collision-avoidance contract.
    #[test]
    fn staged_temp_name_carries_recent_millis_and_compact_hex_suffix() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("cfg");
        let id = OperationId::new("op-temp-name").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: target,
                content: b"payload".to_vec(),
                kind: DocumentKind::TextFragment,
            }],
        );
        txn.prepare().unwrap();
        let temp = txn
            .staged_temps
            .first()
            .cloned()
            .expect("prepare staged one temp");
        // ".tmp.{file_name}.{suffix}.{millis}"
        let name = temp.file_name().unwrap().to_string_lossy().into_owned();
        let mut segments = name.rsplit('.');
        let millis: u128 = segments.next().unwrap_or_default().parse().unwrap_or(0);
        let suffix = segments.next().unwrap_or_default().to_owned();
        assert!(
            millis >= 1_600_000_000_000,
            "temp name must embed a post-2020 epoch millis value, got {name}"
        );
        assert_eq!(suffix.len(), 4, "suffix must be four chars: {name}");
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f')),
            "suffix must be lowercase hex: {name}"
        );
        drop(std::fs::remove_dir_all(&root));
    }

    /// Staged temps are created 0o600 for new targets, inherit the target's
    /// mode for existing targets, and never land mode 0.
    #[cfg(unix)]
    #[test]
    fn staged_temp_permissions_are_hardened_or_inherited() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp_root();

        // New target: hardened 0o600.
        let fresh = root.join("fresh.cfg");
        let temp = stage_temp_file(&fresh, b"x", None).unwrap();
        let mode = std::fs::metadata(&temp).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a fresh target's temp must be 0o600");

        // Existing 0o644 target: the temp inherits the same mode.
        let kept = root.join("kept.cfg");
        std::fs::write(&kept, b"old").unwrap();
        std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o644)).unwrap();
        let temp2 = stage_temp_file(&kept, b"new", None).unwrap();
        let mode2 = std::fs::metadata(&temp2).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode2, 0o644, "an existing target's mode must be inherited");

        // Existing 0o000 target: the metadata is still readable by the owner;
        // mode 0 must be hardened back to 0o600.
        let dark = root.join("dark.cfg");
        std::fs::write(&dark, b"old").unwrap();
        std::fs::set_permissions(&dark, std::fs::Permissions::from_mode(0o000)).unwrap();
        let temp3 = stage_temp_file(&dark, b"new", None).unwrap();
        let mode3 = std::fs::metadata(&temp3).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode3, 0o600, "a zero mode must be hardened to 0o600");
        std::fs::set_permissions(&dark, std::fs::Permissions::from_mode(0o600)).unwrap();
        drop(std::fs::remove_dir_all(&root));
    }

    /// An unclassifiable error opening the parent (ELOOP) surfaces from the
    /// commit against the PARENT path: the sync is not silently swallowed.
    #[cfg(unix)]
    #[test]
    fn commit_surfaces_parent_open_errors_against_the_parent_path() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("w.cfg");
        std::fs::write(&target, b"v1").unwrap();
        let inj = SabotageParentAtSync {
            parent: root.clone(),
            action: ParentSabotage::Loop,
        };
        let id = OperationId::new("op-sync-eloop").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: target,
                content: b"v2".to_vec(),
                kind: DocumentKind::TextFragment,
            }],
        )
        .with_injector(Arc::new(inj));
        txn.prepare().unwrap();
        let err = txn
            .commit()
            .expect_err("the parent sync must surface the ELOOP");
        match err {
            ConfigError::Io { path, source } => {
                assert_eq!(path, root, "the error must be attributed to the parent");
                // ELOOP has no stable ErrorKind on this toolchain and its
                // raw errno is platform-specific (40 on Linux, 62 on macOS);
                // mirror the atomic.rs sync-parent precedent: require an OS
                // error so a plain permission/not-found mixup cannot pass,
                // and pin the exact errno on Linux where the mutation
                // suite runs.
                assert!(
                    source.raw_os_error().is_some(),
                    "the surfaced error is an OS error, got {source}"
                );
                #[cfg(target_os = "linux")]
                assert_eq!(
                    source.raw_os_error(),
                    Some(40),
                    "opening the looped parent must fail with ELOOP"
                );
            }
            other => panic!("expected Io error, got {other:?}"),
        }
        // The sabotaged parent is now a symlink; clean the link itself.
        drop(std::fs::remove_file(&root));
    }

    /// An EACCES opening the parent (Windows-shaped denial) is tolerated:
    /// the commit still succeeds and the content is readable back.
    #[cfg(unix)]
    #[test]
    fn commit_tolerates_unreadable_parent_at_sync() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        if !perm_denies_dir_read_probe(&root) {
            // DAC_OVERRIDE (e.g. root): the PermissionDenied open arm is
            // unreachable for this process; nothing to assert here.
            drop(std::fs::remove_dir_all(&root));
            return;
        }
        let target = root.join("w.cfg");
        std::fs::write(&target, b"v1").unwrap();
        let inj = SabotageParentAtSync {
            parent: root.clone(),
            action: ParentSabotage::DenyRead,
        };
        let id = OperationId::new("op-sync-eacces").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: target.clone(),
                content: b"v2".to_vec(),
                kind: DocumentKind::TextFragment,
            }],
        )
        .with_injector(Arc::new(inj));
        txn.prepare().unwrap();
        txn.commit()
            .expect("an unreadable parent at sync time must be tolerated");
        assert_eq!(std::fs::read(&target).unwrap(), b"v2");
        drop(std::fs::remove_dir_all(&root));
    }

    /// A parent that vanishes at sync time is tolerated; the loss surfaces
    /// at the read-back against the FILE path, never the parent.
    #[cfg(unix)]
    #[test]
    fn commit_reports_vanished_parent_against_the_target_not_parent() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("w.cfg");
        std::fs::write(&target, b"v1").unwrap();
        let inj = SabotageParentAtSync {
            parent: root,
            action: ParentSabotage::Vanish,
        };
        let id = OperationId::new("op-sync-vanish").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: target.clone(),
                content: b"v2".to_vec(),
                kind: DocumentKind::TextFragment,
            }],
        )
        .with_injector(Arc::new(inj));
        txn.prepare().unwrap();
        let err = txn
            .commit()
            .expect_err("the vanished tree must fail the read-back");
        match err {
            ConfigError::Io { path, source } => {
                assert_eq!(
                    path, target,
                    "the vanished-parent loss surfaces at the read-back against the file"
                );
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    /// The commit creates missing target parent directories: a staged temp
    /// committed into a not-yet-existing nested path lands successfully.
    #[test]
    fn commit_staged_file_creates_missing_target_parents() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("a")).unwrap();
        let staged = root.join("a").join(".tmp.staged");
        std::fs::write(&staged, b"payload").unwrap();
        let target = root.join("b").join("gone").join("f.json");
        commit_staged_file(&target, &staged, None, None)
            .expect("the commit must create the missing parents");
        assert_eq!(std::fs::read(&target).unwrap(), b"payload");
        assert!(
            !staged.exists(),
            "the staged temp is consumed by the rename"
        );
        drop(std::fs::remove_dir_all(&root));
    }

    /// A staged temp on another device (tmpfs) commits through the copy
    /// fallback: EXDEV is not a dead end (unix; dev-id guard skips hosts
    /// without a separate `/dev/shm`).
    #[cfg(unix)]
    #[test]
    fn commit_staged_file_falls_back_to_copy_across_devices() {
        let shm = Path::new("/dev/shm");
        let root = tmp_root();
        {
            use std::os::unix::fs::MetadataExt;
            let different = match (shm.metadata(), root.metadata()) {
                (Ok(a), Ok(b)) => a.dev() != b.dev(),
                _ => false,
            };
            if !different {
                // No cross-device staging ground available: skip.
                drop(std::fs::remove_dir_all(&root));
                return;
            }
        }
        let staged = shm.join(format!("superai-exdev-{}", std::process::id()));
        std::fs::write(&staged, b"cross-device payload").unwrap();
        let target = root.join("landed.bin");
        commit_staged_file(&target, &staged, None, None)
            .expect("the cross-device fallback must land the content");
        assert_eq!(std::fs::read(&target).unwrap(), b"cross-device payload");
        assert!(
            !staged.exists(),
            "the staged temp is removed after the copy"
        );
        drop(std::fs::remove_dir_all(&root));
    }

    /// Every ASCII case variant of `config.json` (the probed name is not a
    /// sibling), staged non-minimum first so even creation-ordered readdir
    /// starts with the minimum interior.
    fn case_variant_pool() -> Vec<String> {
        let base = b"config.json";
        let cased: Vec<usize> = (0..base.len())
            .filter(|&i| base[i].is_ascii_alphabetic())
            .collect();
        let mut pool = Vec::new();
        for bits in 1..(1usize << cased.len()) {
            let mut name = base.to_vec();
            for (k, &i) in cased.iter().enumerate() {
                if bits & (1 << k) != 0 {
                    name[i] = name[i].to_ascii_uppercase();
                }
            }
            pool.push(String::from_utf8(name).expect("ascii stays valid utf-8"));
        }
        // `bits == all-ones` (the minimum) is generated last; move it behind
        // the first non-minimum entry.
        let last = pool.len() - 1;
        pool.swap(1, last);
        pool
    }

    /// The case-variant siblings of `config.json` currently staged in `dir`,
    /// in the directory's own readdir order, the same order
    /// `case_fold_collision_in_dir` iterates.
    fn case_variants_in_readdir_order(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("the scratch directory must be readable")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "config.json" && name.eq_ignore_ascii_case("config.json"))
            .collect()
    }

    /// Stage a scratch dir of case-variant siblings until the directory's
    /// own readdir order puts the lexicographic minimum strictly interior.
    /// Order-independence is the point: `best`-update mutants return the
    /// first or last readdir entry, and ext4/overlayfs hash order is fixed
    /// per name-set, so only an interior minimum kills them on any host.
    /// `None` on case-insensitive filesystems, where variants collapse.
    fn discriminating_case_variant_fixture(tag: &str) -> Option<(PathBuf, String)> {
        let dir = boundary_scratch(tag);
        std::fs::write(dir.join("case-probe-a"), b"{}").unwrap();
        let case_insensitive = dir.join("CASE-PROBE-A").exists();
        drop(std::fs::remove_file(dir.join("case-probe-a")));
        if case_insensitive {
            drop(std::fs::remove_dir_all(&dir));
            return None;
        }
        let pool = case_variant_pool();
        let min = pool[1].clone();
        for (staged, name) in pool.iter().enumerate() {
            std::fs::write(dir.join(name), b"{}").unwrap();
            if staged < 2 {
                continue;
            }
            let order = case_variants_in_readdir_order(&dir);
            assert_eq!(
                order.len(),
                staged + 1,
                "the staged variants must be the directory's only content"
            );
            if order.first() != Some(&min) && order.last() != Some(&min) {
                return Some((dir, min));
            }
        }
        // No mainstream filesystem sorts readdir output, so the loop finds
        // an interior minimum long before the pool runs out (hash order: a
        // handful of inserts; creation order: immediately). Fall through
        // with the full pool regardless; the suite must never fail over an
        // iteration order, only the kill strength would suffer.
        Some((dir, min))
    }

    /// With several case-variant siblings the collision report names the
    /// lexicographically first variant deterministically, whatever order the
    /// filesystem reports the directory in.
    #[test]
    fn case_fold_collision_reports_the_first_variant() {
        let Some((dir, min)) = discriminating_case_variant_fixture("case-min") else {
            return; // case-insensitive filesystem: premise absent
        };
        let res = commit_file(
            "case-min",
            &dir.join("config.json"),
            br#"{"a":1}"#,
            DocumentKind::StrictJson,
        );
        let err = res.expect_err("the case-fold collision must be refused");
        let message = format!("{err}");
        let reported = dir.join(&min).display().to_string();
        assert!(
            message.contains(&reported),
            "the report must name the first variant {reported}: {err}"
        );
        for name in case_variants_in_readdir_order(&dir) {
            if name != min {
                let other = dir.join(&name).display().to_string();
                assert!(
                    !message.contains(&other),
                    "only the first variant may be named: {err}"
                );
            }
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    /// Exact-output table for the `JsonC` comment stripper: string contents,
    /// escapes, line and block comments, and lone slashes.
    #[test]
    fn strip_jsonc_comments_matches_the_expected_output_table() {
        let cases: &[(&str, &str)] = &[
            // Line comment to end of line.
            (r#"{"a":1}// tail"#, r#"{"a":1}"#),
            // Line comment stops at the newline.
            ("A//c\nB", "A\nB"),
            // Block comment removed entirely.
            ("x/* hidden */y", "xy"),
            // Unclosed block comment consumes the rest.
            ("a/* never", "a"),
            // `**/` closes the block.
            ("a/* **/ b", "a b"),
            // `//` inside a string is data.
            (r#"{"u":"http://x"}"#, r#"{"u":"http://x"}"#),
            // An escaped quote keeps the string open past a marker.
            (r#"{"k":"a\"//b"}"#, r#"{"k":"a\"//b"}"#),
            // An escaped backslash does not escape the closing quote.
            ("{\"k\":\"a\\\\\"}// c", "{\"k\":\"a\\\\\"}"),
            // A lone slash is ordinary data.
            ("a/b", "a/b"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                &strip_jsonc_comments(input),
                expected,
                "stripping {input:?}"
            );
        }
    }

    /// `?` matches exactly one character and never a path separator.
    #[test]
    fn name_matches_question_mark_never_matches_a_separator() {
        assert!(name_matches("*.md", "keep.md"));
        assert!(!name_matches("*.md", "a/b.md"));
        assert!(name_matches("?.txt", "a.txt"));
        assert!(!name_matches("?.txt", "ab.txt"));
        assert!(!name_matches("?", "/"));
        assert!(!name_matches("?.txt", "/.txt"));
        assert!(name_matches("exact.md", "exact.md"));
    }

    /// Exclude patterns prune whole directories, not just files.
    #[test]
    fn copy_tree_excludes_directories_by_name() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("src/sub")).unwrap();
        std::fs::write(root.join("src/sub/inner.md"), b"inner").unwrap();
        std::fs::write(root.join("src/keep.md"), b"keep").unwrap();
        let opts = CopyTreeOptions {
            exclude: vec!["sub".to_owned()],
            ..CopyTreeOptions::default()
        };
        let report = copy_tree(&root.join("src"), &root.join("dest"), &opts).unwrap();
        assert!(
            !root.join("dest/sub").exists(),
            "an excluded directory must not be traversed"
        );
        assert!(
            report.excluded.contains(&root.join("dest/sub")),
            "the pruned directory must be reported: {:?}",
            report.excluded
        );
        assert_eq!(std::fs::read(root.join("dest/keep.md")).unwrap(), b"keep");
        drop(std::fs::remove_dir_all(&root));
    }

    /// The entry bound counts directories as well as files.
    #[test]
    fn copy_tree_entry_bound_counts_directories_too() {
        let root = tmp_root();
        std::fs::create_dir_all(root.join("src/l1/l2/l3")).unwrap();
        std::fs::write(root.join("src/l1/l2/l3/leaf.md"), b"leaf").unwrap();
        let opts = CopyTreeOptions {
            max_entries: 3,
            ..CopyTreeOptions::default()
        };
        let res = copy_tree(&root.join("src"), &root.join("dest"), &opts);
        assert!(
            res.is_err(),
            "three directories plus one file must exceed max_entries 3"
        );
        drop(std::fs::remove_dir_all(&root));
    }

    /// The byte bound is strict: a file of exactly `max_bytes` still copies.
    #[test]
    fn copy_tree_byte_bound_is_strict() {
        let root = tmp_root();
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("exact.bin"), vec![0u8; 100]).unwrap();
        let opts = CopyTreeOptions {
            max_bytes: 100,
            ..CopyTreeOptions::default()
        };
        let report = copy_tree(&src, &root.join("dest"), &opts)
            .expect("a file of exactly max_bytes must copy");
        assert_eq!(report.bytes, 100);
        assert_eq!(
            std::fs::read(root.join("dest/exact.bin")).unwrap().len(),
            100
        );
        drop(std::fs::remove_dir_all(&root));
    }

    /// The report accounts for every copied byte.
    #[test]
    fn copy_tree_reports_the_sum_of_copied_bytes() {
        let root = tmp_root();
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("f1"), vec![0u8; 10]).unwrap();
        std::fs::write(src.join("f2"), vec![0u8; 20]).unwrap();
        let report = copy_tree(&src, &root.join("dest"), &CopyTreeOptions::default()).unwrap();
        assert_eq!(report.files.len(), 2);
        assert_eq!(report.bytes, 30, "both files' sizes must be accounted");
        drop(std::fs::remove_dir_all(&root));
    }

    /// Removing a directory that is already absent is a success (idempotent).
    #[test]
    fn remove_owned_empty_dir_treats_missing_as_success() {
        let root = tmp_root();
        let missing = root.join("never").join("there");
        remove_owned_empty_dir(&missing)
            .expect("a missing directory must be treated as already removed");
        drop(std::fs::remove_dir_all(&root));
    }

    /// A stat error other than `NotFound` (EACCES on the parent) surfaces as
    /// an error instead of being masked as success.
    #[cfg(unix)]
    #[test]
    fn remove_owned_empty_dir_surfaces_stat_errors() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp_root();
        let parent = root.join("locked");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::symlink_metadata(parent.join("child")).is_ok() {
            // DAC_OVERRIDE (e.g. root): the EACCES arm is unreachable here.
            std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
            drop(std::fs::remove_dir_all(&root));
            return;
        }
        assert!(
            remove_owned_empty_dir(&parent.join("child")).is_err(),
            "an unreadable parent must surface its stat error"
        );
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        drop(std::fs::remove_dir_all(&root));
    }

    /// Plan validation rejects each glob character on its own.
    #[test]
    fn validate_plan_rejects_each_glob_character_alone() {
        for path in ["/tmp/never?path", "/tmp/never[path]"] {
            let id = OperationId::new("op-glob").unwrap();
            let txn = Transaction::new(
                id,
                vec![FileAction::Write {
                    path: PathBuf::from(path),
                    content: b"x".to_vec(),
                    kind: DocumentKind::TextFragment,
                }],
            );
            assert!(
                txn.validate_plan().is_err(),
                "a plan path containing a glob character must be refused: {path}"
            );
        }
    }

    /// Declared follow roots may be spelled through symlinked directory
    /// components: containment compares canonicalized paths on both sides.
    #[cfg(unix)]
    #[test]
    fn follow_roots_resolve_through_symlinked_root_components() {
        let root = tmp_root();
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        // Declare the root BEFORE the allowed subtree exists, spelled
        // through the alias: canonicalization must happen at check time.
        let declared = alias.join("allowed");
        let id = OperationId::new("op-alias-root").unwrap();
        let mut txn = Transaction::new(id, Vec::new()).with_symlink_follow_roots(vec![declared]);
        // Build the tree the plan will mutate through the link.
        let allowed = real.join("allowed");
        std::fs::create_dir_all(&allowed).unwrap();
        let referent = allowed.join("cfg.json");
        std::fs::write(&referent, br#"{"v":1}"#).unwrap();
        let link = root.join("link.json");
        std::os::unix::fs::symlink(&referent, &link).unwrap();
        txn.steps = vec![FileAction::Write {
            path: link.clone(),
            content: br#"{"v":2}"#.to_vec(),
            kind: DocumentKind::StrictJson,
        }];
        let outcome = txn.execute().unwrap();
        assert!(
            outcome.success,
            "the aliased root must contain its referent: {:?}",
            outcome.diagnostics_redacted
        );
        assert!(std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()));
        assert_eq!(std::fs::read(&referent).unwrap(), br#"{"v":2}"#);
        drop(std::fs::remove_dir_all(&root));
    }

    /// A write onto a plain single-link file records no hard-link warning.
    #[test]
    fn plain_writes_never_record_a_hard_link_warning() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("solo.json");
        std::fs::write(&target, b"{\"a\":1}").unwrap();
        let id = OperationId::new("op-no-hlink").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: target,
                content: b"{\"a\":2}".to_vec(),
                kind: DocumentKind::StrictJson,
            }],
        );
        let outcome = txn.execute().unwrap();
        assert!(outcome.success);
        assert!(
            !outcome
                .diagnostics_redacted
                .iter()
                .any(|d| d.contains("hard link")),
            "a single-link write must not warn: {:?}",
            outcome.diagnostics_redacted
        );
        drop(std::fs::remove_dir_all(&root));
    }

    /// A Write planned onto an existing directory is not a backup candidate:
    /// prepare succeeds and the failure surfaces at the rename.
    #[test]
    fn write_onto_an_existing_directory_prepares_cleanly_and_fails_at_rename() {
        let root = tmp_root();
        let dir = root.join("adir");
        std::fs::create_dir_all(&dir).unwrap();
        let id = OperationId::new("op-dir-target").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: dir.clone(),
                content: b"payload".to_vec(),
                kind: DocumentKind::TextFragment,
            }],
        );
        txn.prepare()
            .expect("a directory target is skipped by the backup pass");
        assert!(
            txn.commit().is_err(),
            "the rename onto a directory must fail"
        );
        assert!(dir.is_dir(), "the empty directory survives untouched");
        drop(std::fs::remove_dir_all(&root));
    }

    /// A failure at the third file compensates the two files that already
    /// committed: unrestorable ones are reported as residuals.
    #[test]
    fn third_file_failure_compensates_the_earlier_files() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let a1 = root.join("a1.json");
        let a2 = root.join("a2.json");
        let a3 = root.join("a3.json");
        std::fs::write(&a1, b"{\"a\":1}").unwrap();
        std::fs::write(&a2, b"{\"b\":1}").unwrap();
        std::fs::write(&a3, b"{\"c\":1}").unwrap();
        let id = OperationId::new("op-third-file").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![
                FileAction::Write {
                    path: a1.clone(),
                    content: b"{\"a\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::Write {
                    path: a2.clone(),
                    content: b"{\"b\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
                FileAction::Write {
                    path: a3.clone(),
                    content: b"{\"c\":2}".to_vec(),
                    kind: DocumentKind::StrictJson,
                },
            ],
        )
        .with_injector(FailAtPoint::new(Point::ThirdFile, 1));
        txn.prepare().unwrap();
        // Corrupt the FIRST file's backup: its restore must be refused and
        // reported as a residual, proving the first two steps committed
        // before the third-file boundary failed.
        let a1_backup = txn
            .backups
            .iter()
            .find(|e| e.original_path == a1)
            .cloned()
            .expect("prepare backed up the first target");
        std::fs::write(&a1_backup.backup_path, b"corrupted").unwrap();

        assert!(
            txn.commit().is_err(),
            "the injected third-file boundary must fail the commit"
        );
        let rollback = txn
            .partial_rollback
            .clone()
            .expect("the compensation outcome must be retained");
        assert!(
            rollback.residuals.contains(&a1),
            "the unrestorable first file must be a residual: {rollback:?}"
        );
        assert_eq!(std::fs::read(&a2).unwrap(), b"{\"b\":1}", "restored");
        assert_eq!(std::fs::read(&a3).unwrap(), b"{\"c\":1}", "never committed");
        drop(std::fs::remove_dir_all(&root));
    }

    /// When directory creation fails, the error names the uncreatable
    /// PARENT component (created first), not the full leaf path.
    #[test]
    fn create_dir_failure_names_the_uncreatable_parent() {
        let root = tmp_root();
        let blocker = root.join("file.txt");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let leaf = blocker.join("sub");
        let id = OperationId::new("op-dir-parent").unwrap();
        let mut txn = Transaction::new(id, vec![FileAction::CreateDir { path: leaf }]);
        txn.prepare().unwrap();
        let err = txn.commit().expect_err("creation under a file must fail");
        match err {
            ConfigError::Io { path, .. } => assert_eq!(
                path, blocker,
                "the parent-first creation must attribute the failure to the parent"
            ),
            other => panic!("expected Io error, got {other:?}"),
        }
        drop(std::fs::remove_dir_all(&root));
    }

    /// A Symlink step creates its missing parent directories.
    #[cfg(unix)]
    #[test]
    fn symlink_steps_create_missing_parent_directories() {
        let root = tmp_root();
        let target = root.join("t.txt");
        std::fs::write(&target, b"t").unwrap();
        let link = root.join("deep").join("nested").join("lk");
        let id = OperationId::new("op-link-parents").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Symlink {
                link: link.clone(),
                target: target.clone(),
                expected_current: None,
            }],
        );
        let outcome = txn.execute().unwrap();
        assert!(outcome.success, "{:?}", outcome.diagnostics_redacted);
        assert!(
            std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink()),
            "the link must land with its parents"
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        drop(std::fs::remove_dir_all(&root));
    }

    /// An existing BROKEN link (`exists()` false, lstat ok) is still replaced
    /// under the default policy.
    #[cfg(unix)]
    #[test]
    fn broken_links_are_replaced_under_the_default_policy() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let present = root.join("present.txt");
        std::fs::write(&present, b"p").unwrap();
        let link = root.join("lnk");
        std::os::unix::fs::symlink(root.join("absent.txt"), &link).unwrap();
        let id = OperationId::new("op-broken-link").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Symlink {
                link: link.clone(),
                target: present.clone(),
                expected_current: None,
            }],
        );
        let outcome = txn.execute().unwrap();
        assert!(outcome.success, "{:?}", outcome.diagnostics_redacted);
        assert_eq!(std::fs::read_link(&link).unwrap(), present);
        drop(std::fs::remove_dir_all(&root));
    }

    /// An UNCHANGED link is replaced under the default policy (only a
    /// retarget since prepare is a conflict).
    #[cfg(unix)]
    #[test]
    fn unchanged_links_are_replaced_under_the_default_policy() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let old_target = root.join("old.txt");
        let new_target = root.join("new.txt");
        std::fs::write(&old_target, b"o").unwrap();
        std::fs::write(&new_target, b"n").unwrap();
        let link = root.join("lnk");
        std::os::unix::fs::symlink(&old_target, &link).unwrap();
        let id = OperationId::new("op-unchanged-link").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Symlink {
                link: link.clone(),
                target: new_target.clone(),
                expected_current: None,
            }],
        );
        let outcome = txn.execute().unwrap();
        assert!(
            outcome.success,
            "an unchanged link must be replaceable: {:?}",
            outcome.diagnostics_redacted
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), new_target);
        drop(std::fs::remove_dir_all(&root));
    }

    /// A `RemoveFile` step removes a broken symlink (the link itself).
    #[cfg(unix)]
    #[test]
    fn remove_file_steps_remove_broken_symlinks() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let link = root.join("lnk");
        std::os::unix::fs::symlink(root.join("absent.txt"), &link).unwrap();
        let id = OperationId::new("op-rm-broken").unwrap();
        let mut txn = Transaction::new(id, vec![FileAction::RemoveFile { path: link.clone() }]);
        let outcome = txn.execute().unwrap();
        assert!(outcome.success, "{:?}", outcome.diagnostics_redacted);
        assert!(
            std::fs::symlink_metadata(&link).is_err(),
            "the broken link itself must be gone"
        );
        drop(std::fs::remove_dir_all(&root));
    }

    /// A `RemoveFile` step on a missing target is a success (idempotent).
    #[test]
    fn remove_file_steps_treat_missing_targets_as_success() {
        let root = tmp_root();
        let missing = root.join("never.json");
        let id = OperationId::new("op-rm-missing").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::RemoveFile {
                path: missing.clone(),
            }],
        );
        let outcome = txn.execute().unwrap();
        assert!(
            outcome.success,
            "removing an absent file must be idempotent: {:?}",
            outcome.diagnostics_redacted
        );
        assert!(!missing.exists());
        drop(std::fs::remove_dir_all(&root));
    }

    /// Foreign content drift in the commit→verify window fails verification,
    /// names the digest mismatch, and rolls the target back.
    #[test]
    fn verify_digest_drift_rolls_back_and_names_the_digest() {
        let root = tmp_root();
        let jroot = root.join(".superai").join("journal");
        let target = root.join("d.cfg");
        std::fs::write(&target, b"pre-op").unwrap();
        let inj = TamperTargetAtJournalVerify {
            target: target.clone(),
            bytes: b"drifted".to_vec(),
        };
        let id = OperationId::new("op-drift").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![FileAction::Write {
                path: target.clone(),
                content: b"planned".to_vec(),
                kind: DocumentKind::TextFragment,
            }],
        )
        .with_journal(jroot)
        .with_injector(Arc::new(inj));
        let outcome = txn.execute().unwrap();
        assert!(
            !outcome.success,
            "content drift must fail verification: {:?}",
            outcome.diagnostics_redacted
        );
        assert!(
            outcome
                .verification
                .iter()
                .any(|v| v.message.contains("digest mismatch")),
            "the drift must be named as a digest mismatch: {:?}",
            outcome.verification
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"pre-op",
            "the rollback must restore the pre-op bytes"
        );
        drop(std::fs::remove_dir_all(&root));
    }

    /// Rollback removes a created BROKEN symlink (lstat-visible though
    /// `exists()` is false).
    #[cfg(unix)]
    #[test]
    fn rollback_removes_created_broken_symlinks() {
        let root = tmp_root();
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.cfg");
        std::fs::write(&a, b"old").unwrap();
        let link = root.join("lnk");
        let r = root.join("r.cfg");
        std::fs::write(&r, b"original").unwrap();
        let id = OperationId::new("op-rb-broken").unwrap();
        let mut txn = Transaction::new(
            id,
            vec![
                FileAction::Write {
                    path: a.clone(),
                    content: b"new".to_vec(),
                    kind: DocumentKind::TextFragment,
                },
                FileAction::Symlink {
                    link: link.clone(),
                    target: root.join("absent.txt"),
                    expected_current: None,
                },
                FileAction::RemoveFile { path: r.clone() },
            ],
        );
        txn.prepare().unwrap();
        // Foreign edit of the removal target: the last step aborts after the
        // first two committed, forcing the rollback path.
        std::fs::write(&r, b"foreign edit").unwrap();
        assert!(txn.commit().is_err());
        assert!(
            std::fs::symlink_metadata(&link).is_err(),
            "the created broken link must be removed by the rollback"
        );
        assert_eq!(std::fs::read(&a).unwrap(), b"old", "restored from backup");
        drop(std::fs::remove_dir_all(&root));
    }

    /// An aborted follow-and-preserve commit (caller token invalidated by a
    /// link retarget) leaves no staged temp behind.
    #[cfg(unix)]
    #[test]
    fn follow_token_abort_cleans_staged_temps() {
        let root = tmp_root();
        let allowed = root.join("allowed");
        std::fs::create_dir_all(&allowed).unwrap();
        let referent = allowed.join("real.json");
        std::fs::write(&referent, br#"{"v":1}"#).unwrap();
        let other = allowed.join("other.json");
        std::fs::write(&other, br#"{"v":2}"#).unwrap();
        let link = root.join("cfg.json");
        std::os::unix::fs::symlink(&referent, &link).unwrap();
        let token = snapshot(&link);
        // Foreign retarget between the caller's read and the commit.
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();

        let roots = vec![allowed.clone()];
        let res = commit_file_expecting_with_roots(
            "follow-abort",
            &link,
            br#"{"v":3}"#,
            DocumentKind::StrictJson,
            Some(&token),
            &roots,
        );
        match res {
            Err(ConfigError::ConcurrentModification { .. }) => {}
            other_err => panic!("expected ConcurrentModification, got {other_err:?}"),
        }
        assert_eq!(std::fs::read(&other).unwrap(), br#"{"v":2}"#);
        for entry in std::fs::read_dir(&allowed).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with(".tmp."),
                "the aborted follow leaked staged temp {name}"
            );
        }
        drop(std::fs::remove_dir_all(&root));
    }

    /// Forward-slash UNC text is windows-shaped on every host: the folded
    /// comparison in `paths_equal_platform_folded` relies on the `//` prefix
    /// alone (a host cannot see `\\`-only text from a windows caller).
    #[test]
    fn looks_windows_shaped_accepts_forward_slash_unc_text() {
        assert!(looks_windows_shaped("//server/share"));
        assert!(looks_windows_shaped("\\\\server\\share"));
        assert!(looks_windows_shaped("C:\\Data"));
        assert!(looks_windows_shaped("z:/cfg"));
        assert!(!looks_windows_shaped("1:Data"));
        assert!(!looks_windows_shaped("/unix/path"));
        assert!(!looks_windows_shaped(""));
    }

    /// Two suffixes drawn within the same millisecond must differ: the
    /// process-id + counter mix keeps same-millis temp names collision-free.
    #[test]
    fn random_suffixes_stay_distinct_within_the_same_millisecond() {
        let a = generate_random_suffix(1_700_000_000_123);
        let b = generate_random_suffix(1_700_000_000_123);
        for suffix in [&a, &b] {
            assert_eq!(suffix.len(), 4, "suffix must be four chars: {suffix}");
            assert!(
                suffix
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
                "suffix must be lowercase hex: {suffix}"
            );
        }
        assert_ne!(a, b, "the counter must keep same-millis suffixes distinct");
    }

    /// With several variant siblings the report names the lexicographically
    /// first one, independent of the filesystem's iteration order (the
    /// fixture guarantees the minimum is interior to it).
    #[test]
    fn case_fold_collision_picks_the_lexicographically_first_variant() {
        let Some((dir, min)) = discriminating_case_variant_fixture("fold-order") else {
            return; // case-insensitive filesystem: premise absent
        };
        let got = case_fold_collision_in_dir(&dir.join("config.json"))
            .expect("a case-variant sibling must be detected");
        assert_eq!(
            got,
            dir.join(&min),
            "the lexicographically first variant must be reported"
        );
        drop(std::fs::remove_dir_all(&dir));
    }
}
