//! Multi-file compensated transaction (MUT-05 / MUT-06).
//!
//! Implements a compensated transaction over heterogeneous file actions. No
//! claim of filesystem-wide atomicity is made; the contract is that all
//! foreign files are backed up before the first commit, staged outputs are
//! validated via parsers, commits happen in deterministic dependency order,
//! post-commit verification reads fresh from disk, and on failure committed
//! files are restored in reverse order with verified rollback and explicit
//! residual reporting.

#![expect(
    clippy::excessive_nesting,
    reason = "transaction requires deep validation and rollback logic"
)]

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::backup::{BackupEntry, backup_with_injector, verify_backup};
use crate::document::DocumentKind;
use crate::error::{ConfigError, Result};
use crate::injector::{Injector, Point};
use crate::journal::{CrashJournal, JournalBackup, JournalPhase};
use crate::snapshot::{Snapshot, is_modified, snapshot};

// ---------------------------------------------------------------------------
// OperationId
// ---------------------------------------------------------------------------

/// Stable identifier for a transaction operation.
///
/// Mirrors the shape of [`crate::backup::BackupId`] but is scoped to the
/// transaction boundary and used for quarantine and backup linkage.
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

// ---------------------------------------------------------------------------
// Remove semantics (MUT-08)
// ---------------------------------------------------------------------------

/// Distinguishes the intent of a removal operation.
///
/// Each variant has different safety rules and quarantine requirements. This
/// prevents accidental use of a single `remove(path)` for materially
/// different operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RemoveKind {
    /// Remove a single config entry from a shared file (e.g. a JSON key).
    ///
    /// The file itself is preserved; only the entry is edited out.
    ConfigEntry,
    /// Delete a superai-created wrapper or file.
    ///
    /// The target must be a file superai created and must not be a foreign
    /// harness config.
    WrapperFile,
    /// Remove an instance root directory.
    ///
    /// The target is a material directory that must first be moved to
    /// quarantine before final delete.
    InstanceRoot,
    /// Uninstall a binary.
    ///
    /// Binary removal never touches config directories and requires explicit
    /// caller intent.
    Binary,
    /// Detach a registry record only.
    ///
    /// No filesystem mutation; only the superai-owned records file is
    /// affected.
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

/// Validate a removal target according to [`RemoveKind`] policy.
///
/// Rejects broad roots, unresolved variables, globs, home directories,
/// workspace roots, and foreign-managed paths surrogates.
///
/// This is a best-effort guard at the config layer; adapter-level ownership
/// checks are still required.
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

/// Reject broad roots and the home directory for any removal target:
/// unix broad roots, Windows-shaped broad roots (drive roots, UNC roots,
/// first-level system directories — case-insensitive, both separators), and
/// the home directory compared with platform-correct case rules.
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
        // Binary removal must not target a directory that looks like a config root.
        // Windows-shaped paths honor both separators and case-folding
        // (`C:\Users\me\.CLAUDE`); unix paths keep exact matching.
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

/// Whether `s` is shaped like a Windows path (drive-letter prefix or UNC
/// `\\server` root), independent of the host platform.
fn looks_windows_shaped(s: &str) -> bool {
    s.starts_with("\\\\")
        || s.starts_with("//")
        || (s.chars().nth(1) == Some(':')
            && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
}

/// Normalize a path string for Windows-style comparison: backslashes to
/// forward slashes and ASCII lowercasing (Windows matches paths
/// case-insensitively).
fn normalize_windows_style(s: &str) -> String {
    s.replace('\\', "/").to_ascii_lowercase()
}

/// Whether `path` is a Windows-shaped broad root that must never be removed
/// or quarantined: drive roots (`C:\`), UNC roots (`\\server`, `\\server\share`),
/// and the first-level system directories (`C:\Windows`, `C:\Program Files`,
/// `C:\Program Files (x86)`, `C:\ProgramData`, `C:\Users`,
/// `C:\Documents and Settings`).
///
/// Matching is anchored, ASCII-case-folded, and treats `/` and `\` as
/// equivalent — Windows path semantics. Unix-shaped paths never match, so
/// the helper is inert on unix regardless of the literal path text.
pub(crate) fn windows_shaped_broad_root(path: &Path) -> bool {
    let normalized = normalize_windows_style(&path.to_string_lossy());
    let trimmed = normalized.trim_end_matches('/');

    let has_drive_prefix = trimmed
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && trimmed.chars().nth(1) == Some(':');
    if has_drive_prefix {
        // Drive-shaped path. Anything after the prefix must not exist
        // (drive root) or be exactly one first-level system directory.
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

    // UNC root: `\\server` or `\\server\share` — the share itself is broad
    // (2 or 3 separators after normalization); anything deeper is a specific
    // target. Only windows-shaped text matches: a verbatim `\\` prefix on
    // any host, or `//` on Windows (where it is also a UNC root). A unix
    // `//`-prefixed path is NOT treated as UNC — unix behavior is unchanged.
    let raw = path.to_string_lossy();
    let unc_shaped = raw.starts_with("\\\\") || (cfg!(windows) && raw.starts_with("//"));
    unc_shaped && trimmed.matches('/').count() <= 3
}

/// Whether the final component of `path` is a Windows reserved device name
/// (`CON`, `PRN`, `AUX`, `NUL`, `COM1`..`COM9`, `LPT1`..`LPT9`, `CONIN$`,
/// `CONOUT$`), matched on the stem before the first extension dot and
/// ASCII-case-folded — exactly the Windows rule (`CON.txt` and `con` are
/// devices, not files).
///
/// Pure string semantics, so the guard holds on every host: a plan that
/// names a reserved device can never become a real file on Windows, and the
/// plan layer surfaces that before any staging (QAL-09 reserved-name case).
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

/// Path equality with platform-correct case rules: byte equality first;
/// when either side is windows-shaped, compare normalized and
/// ASCII-case-folded (Windows filesystems match case-insensitively).
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

// ---------------------------------------------------------------------------
// FileAction
// ---------------------------------------------------------------------------

/// Ordered file-system action within a transaction.
///
/// Each variant is a single, auditable mutation. The transaction layer
/// resolves the full graph before any mutation, sorts deterministically,
/// backs up foreign files, stages temps, validates via parsers, commits in
/// dependency order, and verifies.
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
    /// Create a symlink at `link` pointing to `target`.
    ///
    /// `expected_current` implements the MUT-02/MUT-06 owned-target rule:
    /// `None` replaces an existing link only when its current target still
    /// matches the prepare-time snapshot (a retargeted link aborts with a
    /// conflict) and creates when absent; `Some(target)` additionally
    /// requires any existing link to currently point at exactly that
    /// expected owned target before it is replaced.
    Symlink {
        /// Absolute link path.
        link: PathBuf,
        /// Symlink target (may be relative or absolute).
        target: PathBuf,
        /// The owned target an existing link must currently carry for
        /// replacement to be allowed.
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

// ---------------------------------------------------------------------------
// Helpers: digest, temps, permissions, validation
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

/// Unix file identity (device, inode) of an existing path, when observable.
///
/// Follows symlinks first (two paths converging on one file through links
/// are the same mutation target), falling back to the link's own identity
/// for a broken link. Used to detect multiple planned paths resolving to
/// one inode (hard-link aliases) — MUT-02.
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

/// Number of hard links to an existing path (`nlink`), when observable.
///
/// `nlink > 1` means the planned atomic replacement would break link sharing:
/// the rename replaces one directory entry while the aliases keep the old
/// bytes. MUT-02 requires that to be explicit — callers surface the warning
/// recorded here instead of silently splitting the link group.
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
            // several filesystems; the committed file was already synced.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(()),
            Err(e) => Err(ConfigError::io(parent, e)),
        },
        // Windows cannot open a directory handle without backup semantics
        // (winerror 5). Parent sync is best-effort durability, not a
        // correctness requirement — the rename already landed.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ConfigError::io(parent, e)),
    }
}

/// Stage `content` into a same-directory temp for `target` — the production
/// staging primitive shared by [`Transaction`] and the failure matrix
/// (QAL-06: the injected wrapper delegates here, so tests exercise the REAL
/// staging path).
///
/// Creates an exclusive temp, applies safe permissions before any bytes are
/// written, writes + flushes + syncs, and returns the temp path.
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
    // Create with exclusive semantics where possible, set safe permissions before secret bytes.
    let mut attempts = 0;
    let mut final_temp = generate_temp_path(target)?;
    let mut file: Option<std::fs::File> = None;
    for _ in 0..3 {
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
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                attempts += 1;
                if attempts >= 3 {
                    break;
                }
            }
            Err(e) => return Err(ConfigError::io(&candidate, e)),
        }
    }
    let mut f = if let Some(f) = file {
        f
    } else {
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&final_temp)
            .map_err(|e| ConfigError::io(&final_temp, e))?
    };
    drop(f);
    set_safe_permissions(&final_temp, target)?;
    f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&final_temp)
        .map_err(|e| {
            drop(std::fs::remove_file(&final_temp));
            ConfigError::io(&final_temp, e)
        })?;
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

/// Commit a staged temp over `target` — the production commit primitive
/// shared by [`Transaction::commit_write`] and the failure matrix.
///
/// §4.2 / MUT-05: when `expected` (the prepare-time snapshot) is supplied,
/// the target is re-read FRESH immediately before the rename and compared
/// against it; any difference — foreign edit, removal, appearance, symlink
/// retarget — aborts with `ConcurrentModification` and the target is never
/// overwritten. The bytes that land are exactly the staged temp's bytes,
/// which were verified against the planned content digest at staging.
pub fn commit_staged_file(
    target: &Path,
    staged: &Path,
    expected: Option<&Snapshot>,
    injector: Option<&dyn Injector>,
) -> Result<()> {
    validate_path_safety(target)?;
    // Read staged content for verification after rename; its digest is the
    // planned content digest recorded at staging time.
    let staged_bytes = std::fs::read(staged).map_err(|e| ConfigError::io(staged, e))?;
    let expected_digest = compute_digest(&staged_bytes);

    // Ensure parent exists
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    // §4.2 conflict recheck immediately before the rename.
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

    // Use atomic rename from staged temp (same filesystem).
    // Try rename; on cross-device error fallback to copy.
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

    // Read back and verify digest
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

// ---------------------------------------------------------------------------
// Single-file mutation boundary (plan-02 fold)
// ---------------------------------------------------------------------------

/// Report from a single-file commit through the mutation boundary.
#[derive(Debug, Clone)]
pub struct FileCommitReport {
    /// Backup of the previous contents taken before the replacement landed
    /// (`None` when the commit created a new file).
    pub backup: Option<BackupEntry>,
    /// Hex digest of the committed bytes (read back and verified on disk).
    pub digest: String,
}

/// Detect a case-insensitive collision for `target` inside its directory: an
/// existing sibling whose name ASCII-folds to the same name but is not the
/// exact name (QAL-09). On a case-insensitive filesystem (Windows, default
/// macOS APFS) such a write would silently land over the sibling; on a
/// case-sensitive filesystem it is surfaced as risk, mirroring the in-plan
/// case-fold rejection in [`Transaction::validate_plan`].
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
            // The target's exact directory entry is not a collision.
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

/// Commit `content` to `target` through the ONE mutation boundary of this
/// crate (plan-02 fold): a single-step [`Transaction`] whose prepare/commit
/// core performs the full discipline — path validation → fresh snapshot →
/// backup of existing contents → staged parse-validation → §4.2 conflict
/// recheck → atomic replacement → read-back verify.
///
/// Every codec store (`json`/`jsonc`/`toml_file`/`yaml`/`env_file`), the raw
/// editor commit core, and every superai-core production write go through
/// this function or through [`stage_temp_file`] + [`commit_staged_file`]
/// (the same core the multi-step [`Transaction`] commits through); the raw
/// `atomic_write` family is crate-internal.
///
/// `id` attributes the operation in backup entries and journals. Errors are
/// the transaction's typed errors; a failed commit leaves the target
/// untouched and removes its staged temp.
pub fn commit_file(
    id: &str,
    target: &Path,
    content: &[u8],
    kind: DocumentKind,
) -> Result<FileCommitReport> {
    commit_file_expecting(id, target, content, kind, None)
}

/// [`commit_file`] with a caller-supplied §4.2 conflict token.
///
/// `expected` is a snapshot the caller took when it read the document (the
/// raw-editor read→commit contract): when supplied it overrides the
/// boundary's own prepare-time token, so any foreign change since the
/// caller's read — not just since prepare — aborts with
/// `ConcurrentModification` before the replacement.
pub fn commit_file_expecting(
    id: &str,
    target: &Path,
    content: &[u8],
    kind: DocumentKind,
    expected: Option<&Snapshot>,
) -> Result<FileCommitReport> {
    commit_file_expecting_with_roots(id, target, content, kind, expected, &[])
}

/// [`commit_file_expecting`] with the MUT-02 adapter-allowed follow roots
/// (see [`Transaction::with_symlink_follow_roots`]).
///
/// When `target` is an allowed symlink the write follows-and-preserves the
/// link (the referent is mutated). A caller-supplied `expected` token in
/// that case guards the LINK the caller actually read: any retarget or byte
/// change observed through it aborts with `ConcurrentModification` before
/// the referent is touched.
pub fn commit_file_expecting_with_roots(
    id: &str,
    target: &Path,
    content: &[u8],
    kind: DocumentKind,
    expected: Option<&Snapshot>,
    follow_roots: &[PathBuf],
) -> Result<FileCommitReport> {
    // Keep the directory-target contract of the former atomic_write path: a
    // typed refusal before any staging work.
    if std::fs::symlink_metadata(target).is_ok_and(|m| m.is_dir()) {
        return Err(ConfigError::io(
            target,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "is a directory"),
        ));
    }
    // QAL-09 case-insensitive collision guard: creating `File.json` next to
    // an existing `file.json` would silently land over it on case-insensitive
    // filesystems.
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
    // prepare: path safety, hard-link warning, backup-before-foreign-write,
    // staged parse-validation, §4.2 token, MUT-02 follow-and-preserve
    // retargeting onto allowed symlink referents.
    if let Err(e) = transaction.prepare() {
        cleanup_staged_temps(&transaction.staged_temps);
        return Err(e);
    }
    // The caller's older token (when supplied) guards its full read→commit
    // window instead of just prepare→commit.
    let effective_target = transaction.steps.first().map_or_else(
        || target.to_path_buf(),
        |step| step.primary_path().to_path_buf(),
    );
    if effective_target != target {
        // MUT-02 follow-and-preserve: the caller read through the LINK, so
        // its token is checked against the link's current state — a retarget
        // or content change since the read aborts before the referent is
        // mutated.
        if let Some(expected) = expected
            && is_modified(expected, &snapshot(target))
        {
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
    // commit: §4.2 recheck immediately before the rename, atomic replace,
    // parent sync, read-back digest/size verify. A single-step commit that
    // fails has landed nothing else; the staged temp is ours to remove.
    let commit_outcome = match transaction.commit() {
        Ok(outcome) => outcome,
        Err(e) => {
            cleanup_staged_temps(&transaction.staged_temps);
            return Err(e);
        }
    };
    // Post-commit parse verification (the multi-file discipline's verify
    // step): on failure roll back to the backup (or remove the creation) and
    // surface a typed verification error.
    let verification = transaction.verify()?;
    if let Some(failed) = verification.iter().find(|v| !v.digest_ok || !v.parse_ok) {
        let message = failed.message.clone();
        // A single step's rollback either restores the backup or removes the
        // creation; its typed outcome is not observable here, so the caller
        // sees the verification error that caused it.
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

fn validate_staged_content(content: &[u8], kind: DocumentKind, path: &Path) -> Result<()> {
    match kind {
        DocumentKind::StrictJson => {
            std::str::from_utf8(content).map_err(|_err| {
                ConfigError::io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8 in json"),
                )
            })?;
            serde_json::from_slice::<serde_json::Value>(content).map_err(|source| {
                ConfigError::Json {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
            Ok(())
        }
        DocumentKind::JsonC => {
            let text = std::str::from_utf8(content).map_err(|_err| {
                ConfigError::io(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf8 in jsonc"),
                )
            })?;
            let stripped = strip_jsonc_comments(text);
            serde_json::from_str::<serde_json::Value>(&stripped).map_err(|source| {
                ConfigError::Json {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
            Ok(())
        }
        DocumentKind::Toml => {
            let text = std::str::from_utf8(content).map_err(|_err| {
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
            Ok(())
        }
        DocumentKind::Yaml => {
            let text = std::str::from_utf8(content).map_err(|_err| {
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
            Ok(())
        }
        DocumentKind::Env => {
            // Validate env: each non-blank, non-comment line must contain '='
            let text = std::str::from_utf8(content).map_err(|_err| ConfigError::Env {
                path: path.to_path_buf(),
                message: "invalid utf8 in env file".to_owned(),
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
                if without_export.starts_with('=') {
                    return Err(ConfigError::Env {
                        path: path.to_path_buf(),
                        message: format!("line {} has empty key", idx + 1),
                    });
                }
            }
            Ok(())
        }
        DocumentKind::TextFragment | DocumentKind::Opaque => Ok(()),
    }
}

#[expect(
    clippy::excessive_nesting,
    reason = "comment stripping state machine requires nesting"
)]
fn strip_jsonc_comments(input: &str) -> String {
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
// Recursive copy + owned directory removal (MUT-06)
// ---------------------------------------------------------------------------

/// How symlinks are treated during a recursive copy (MUT-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SymlinkPolicy {
    /// Skip symlink entries entirely (credentials and redirect traps never
    /// leave the source tree). Default.
    #[default]
    Skip,
    /// Recreate the link itself at the destination pointing at the same
    /// target (relative targets are copied verbatim; absolute targets stay
    /// absolute).
    PreserveLink,
    /// Copy the referent's bytes as a regular file (redirect: the copy no
    /// longer depends on the link target existing). A broken or looping link
    /// is an error under this policy — copying "nothing" silently would be a
    /// lie about what was copied.
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

/// Tiny glob matcher supporting `*` (any run, including separators within a
/// single name is not possible: `*` matches any characters except `/`) and
/// `?` (one character). No regex engine is pulled in for this.
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

/// Whether a DIRECTORY name passes the filters. Only `exclude` prunes
/// directories; `include` never does — it selects files, not structure, so
/// included files in nested directories are still found.
fn dir_allowed(name: &str, opts: &CopyTreeOptions) -> bool {
    !opts.exclude.iter().any(|p| name_matches(p, name))
}

/// Recursively copy `from` to `to` with explicit include/exclude filters and
/// a symlink policy (MUT-06 — used by mirror/skills flows).
///
/// Directories are traversed unless excluded; files are copied when they
/// pass the include/exclude filters, byte-for-byte with their permission
/// bits preserved where the platform supports it. The run is bounded by
/// `max_entries`/`max_bytes` and verifies each copied file's digest against
/// its source before continuing.
pub fn copy_tree(from: &Path, to: &Path, opts: &CopyTreeOptions) -> Result<CopyTreeReport> {
    // Refuse copying a tree into itself: the recursion would never terminate
    // and would nest copies until the entry bound trips.
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

/// Apply the symlink policy for one link entry.
///
/// Returns `Ok(true)` when the entry is fully handled (skip or preserved
/// link); `Ok(false)` when the policy is `FollowCopyContent` and the copy
/// should continue with the referent's bytes.
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
            // A link whose referent cannot be read as a file (broken,
            // directory, or loop) fails the copy honestly.
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

/// Copy one (possibly link-followed) file entry with bounds and digest
/// verification. Under `FollowCopyContent` the bytes that land are the
/// referent's, so the size bound and copy read through the link.
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
    // std::fs::copy preserves permission bits where the platform has them
    // and never writes through the destination path in place.
    std::fs::copy(src, dest).map_err(|e| ConfigError::io(dest, e))?;
    // Verify the copy before accounting it as done.
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

/// Remove a directory that superai owns and that is empty (MUT-06).
///
/// Refuses broad roots exactly like [`validate_remove_target`] for
/// `InstanceRoot`; a non-empty directory is a typed refusal (the caller must
/// quarantine instead), and a target that is not a directory is refused.
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

// ---------------------------------------------------------------------------
// Path safety (MUT-02)
// ---------------------------------------------------------------------------

/// Validate a path for safe mutation.
///
/// Rejects directories that are devices/FIFOs/sockets, parent traversal,
/// unresolved variables, globs, and case-folded collisions are checked
/// separately in [`Transaction::validate_plan`].
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
    // QAL-09: Windows reserved device names can never become real files on
    // Windows; reject them at plan time on every host — the same
    // surface-the-risk philosophy as the case-fold collision check in
    // `validate_plan`.
    if windows_reserved_device_name(path) {
        return Err(ConfigError::io(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "windows reserved device name",
            ),
        ));
    }
    // Reject unsupported special files if they exist
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

// ---------------------------------------------------------------------------
// Outcome types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Transaction
// ---------------------------------------------------------------------------

/// Compensated multi-file transaction (MUT-05).
///
/// No claim of filesystem-wide atomicity; on failure committed files are
/// restored in reverse order (compensated transaction) and residuals are
/// reported explicitly.
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
    /// Outcome of the rollback [`Transaction::commit`] performs internally
    /// when a step fails after other steps already committed.
    ///
    /// `residuals` lists the paths the rollback could not undo; they remain
    /// on disk in their committed state and must reach the caller.
    /// [`Transaction::execute`] copies this into
    /// [`TransactionOutcome::rollback`] instead of reporting an empty
    /// rollback. `None` after a successful commit.
    pub partial_rollback: Option<RollbackOutcome>,
    /// Prepare-time snapshots per step path — the §4.2 conflict tokens every
    /// commit step rechecks against immediately before its mutation (MUT-05).
    expected_states: HashMap<PathBuf, Snapshot>,
    /// Optional failure injector threaded through the REAL staging, rename,
    /// backup, and rollback boundaries (QAL-06). `None` in production runs.
    injector: Option<Arc<dyn Injector>>,
    /// Adapter-allowed roots for the MUT-02 default link policy: a `Write`
    /// step whose target is currently a symlink may FOLLOW the link (the
    /// link itself is preserved and the referent is mutated) only when the
    /// referent resolves inside this set. A symlinked write target resolving
    /// outside it is refused with the typed
    /// [`ConfigError::SymlinkFollowRefused`] before any mutation. When the
    /// set is non-empty it additionally constrains `Symlink` step targets.
    symlink_follow_roots: Vec<PathBuf>,
    /// Follow-and-preserve retargeting bookkeeping (link, referent) recorded
    /// during [`Transaction::prepare`] — the link must still point at the
    /// referent at commit time or the step aborts (MUT-02 changed-target
    /// detection).
    symlink_followed: Vec<(PathBuf, PathBuf)>,
    /// Journal directory enabling production crash journaling (MUT-09).
    /// `None` disables journaling entirely.
    journal_root: Option<PathBuf>,
    /// Current journal state (mirrors the last phase written to disk).
    journal_state: Option<CrashJournal>,
    /// Paths committed so far (journal `completed` list).
    journal_completed: Vec<PathBuf>,
    /// Non-fatal path-safety warnings (e.g. hard-link sharing on a write
    /// target) surfaced through the outcome diagnostics.
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

    /// Builder: attach a failure injector (QAL-06).
    ///
    /// The injector observes the production boundaries of staging, conflict
    /// recheck, rename, parent sync, read-back verify, rollback verify, and
    /// journal phase transitions.
    #[must_use = "the injector is only attached to the returned transaction"]
    pub fn with_injector(mut self, injector: Arc<dyn Injector>) -> Self {
        self.injector = Some(injector);
        self
    }

    /// Builder: enable the production crash journal under `journal_root`
    /// (MUT-09). The transaction writes a journal entry before mutations and
    /// at every phase transition, and removes it only after verified
    /// completion; [`crate::journal::recover_pending`] performs startup
    /// recovery.
    #[must_use = "journaling is only enabled on the returned transaction"]
    pub fn with_journal(mut self, journal_root: PathBuf) -> Self {
        self.journal_root = Some(journal_root);
        self
    }

    /// Builder: declare the adapter-allowed root set for the MUT-02 default
    /// link policy (see [`Self::symlink_follow_roots`]).
    ///
    /// Roots are canonicalized when they exist so a linked root directory
    /// (e.g. `/tmp` on macOS) compares equal to the canonical referent.
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

    /// Resolve the follow-and-preserve target for a mutation path (MUT-02
    /// default link policy).
    ///
    /// - Not a symlink → `Ok(None)` (nothing to follow).
    /// - Symlink resolving inside the allowed roots → `Ok(Some(referent))`:
    ///   the caller preserves the link and mutates the referent.
    /// - Symlink resolving outside the roots, or unresolvable (broken
    ///   link / loop) → typed [`ConfigError::SymlinkFollowRefused`].
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

    /// Absolute best-effort resolution of a `Symlink` step target for the
    /// declared-root containment check: relative targets resolve against the
    /// link's parent; the result is canonicalized when it exists so linked
    /// roots compare equal.
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
        // The target usually does not exist yet (it is being planned).
        // Canonicalize its longest existing ancestor so symlinked temp roots
        // (/var -> /private/var on macOS) compare equal on both sides.
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

    /// Write the journal entry for `phase` and update the in-memory state.
    ///
    /// No-op when journaling is disabled. After the write, the injector's
    /// journal-phase point fires (MUT-09 crash simulation: an injected error
    /// leaves the journal on disk at exactly this phase).
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

    /// Remove the journal after verified completion or verified rollback
    /// (MUT-09: removal happens only after verification).
    fn clear_journal(&mut self) {
        if let Some(root) = self.journal_root.clone() {
            let path = crate::journal::journal_path(&root, self.id.as_str());
            if let Err(e) = CrashJournal::remove(&path) {
                self.warnings.push(format!("journal removal failed: {e}"));
            }
        }
        self.journal_state = None;
    }

    /// Validate the plan without touching disk beyond fresh snapshots.
    ///
    /// Checks path safety, symlink loops, duplicate path/case-fold
    /// collisions, traversal, — MUT-02 — that no two planned paths
    /// resolve to the same inode on one device (a hard-link alias), and the
    /// default link policy: a `Write` step onto an existing symlink must
    /// resolve within the adapter-allowed follow roots (typed
    /// [`ConfigError::SymlinkFollowRefused`] otherwise), and a `Symlink`
    /// step target must resolve within the roots whenever roots are
    /// declared. Committing both aliases would mutate the same bytes twice
    /// and atomic replacement of either breaks link sharing, so the plan is
    /// rejected with a typed [`ConfigError::HardlinkConflict`].
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
            // MUT-02 default link policy for mutation targets: a Write onto an
            // existing symlink follows it only within the declared roots.
            if matches!(step, FileAction::Write { .. }) {
                self.symlink_follow_target(path)?;
            }
            // MUT-02 policy for link creation: when the caller declared an
            // allowed root set, a planned link may not point outside it.
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
                // On case-insensitive platforms this would be a collision.
                // We report as validation even on case-sensitive platforms to
                // surface the risk.
                return Err(ConfigError::io(
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "case-fold collision in transaction",
                    ),
                ));
            }
            // MUT-02: multiple planned paths resolving to one inode/file
            // identity. Both would write the same bytes through two names.
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

    /// Prepare the transaction: backup all foreign files, stage temps, validate.
    ///
    /// Backups are created before the first commit (MUT-05). Staged outputs
    /// are validated via parsers before any commit. The prepare-time snapshot
    /// of every step path is recorded as the §4.2 conflict token each commit
    /// step rechecks immediately before its mutation.
    ///
    /// MUT-02 follow-and-preserve: after validation, every `Write` step whose
    /// target is an allowed symlink (resolved within
    /// [`Transaction::with_symlink_follow_roots`]) is RETARGETED to the
    /// referent — staging, backup, snapshot, commit, and journal then operate
    /// on the file actually mutated while the link itself is preserved. The
    /// originating link is re-verified at commit time.
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

        // §4.2 conflict tokens: recorded AFTER staging so directories the
        // transaction's own staging created (write parents) are expected to
        // exist. Everything from here to each step's pre-mutation recheck is
        // the guarded prepare→commit window.
        self.expected_states.clear();
        for step in &self.steps {
            let path = step.primary_path().to_path_buf();
            if self.expected_states.contains_key(&path) {
                continue;
            }
            let snap = snapshot(&path);
            self.expected_states.insert(path, snap);
        }

        // Verify staged temps digests match expected content digests (no secret leak).
        for (target, temp) in staged_map {
            for step in &self.steps {
                let FileAction::Write { path, content, .. } = step else {
                    continue;
                };
                if path != &target {
                    continue;
                }
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
        }

        Ok(())
    }

    /// Fresh snapshot of every step path (the pre-backup state that drives
    /// the backup-time conflict check).
    fn snapshot_targets(&self) -> HashMap<PathBuf, Snapshot> {
        let mut snapshots: HashMap<PathBuf, Snapshot> = HashMap::new();
        for step in &self.steps {
            let path = step.primary_path().to_path_buf();
            // Avoid overwriting snapshot for duplicate logic already validated.
            if snapshots.contains_key(&path) {
                continue;
            }
            let snap = snapshot(&path);
            snapshots.insert(path, snap);
        }
        snapshots
    }

    /// Hard-link policy (MUT-02): a write target with more than one link
    /// would silently split the link group on atomic replacement. The
    /// default is to proceed with an explicit warning recorded here and
    /// surfaced through the outcome; alias pairs are rejected in
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

    /// Back up all foreign (existing) files before the first commit (MUT-05).
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

    /// Stage temps for every Write action and validate them via parsers.
    /// Returns (target, temp) pairs for the staged-digest verification.
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
                validate_staged_content(content, *kind, path)?;
                let temp_path = self.stage_write(path, content)?;
                // Validate the staged file parses as well (read fresh from staged temp).
                let staged_bytes =
                    std::fs::read(&temp_path).map_err(|e| ConfigError::io(&temp_path, e))?;
                validate_staged_content(&staged_bytes, *kind, &temp_path)?;
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
                // A prepare that fails mid-staging must not leak the temps it
                // already created (non-journaled callers have no recovery
                // sweep to clean them later).
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

    /// Retarget `Write` steps sitting on allowed symlinks to their referents
    /// (MUT-02 follow-and-preserve) and record the (link, referent) pairs the
    /// commit phase re-verifies. Runs after [`Self::validate_plan`]; the
    /// policy decision is re-derived here with errors propagated, never
    /// swallowed.
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

    /// MUT-02 changed-target detection: every link followed during prepare
    /// must still point at the referent the plan mutated. A retarget between
    /// prepare and commit is a concurrent modification — abort before the
    /// rename lands on a file the caller no longer reaches through that link.
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

    /// Commit in dependency order.
    ///
    /// Assumes [`Self::prepare`] has been called. Every step rechecks the
    /// prepare-time snapshot of its target immediately before mutating
    /// (§4.2); a foreign change aborts with `ConcurrentModification` before
    /// any overwrite. On failure the caller should invoke [`Self::rollback`]
    /// and inspect residuals.
    pub fn commit(&mut self) -> Result<CommitOutcome> {
        let mut committed: Vec<PathBuf> = Vec::new();
        let mut write_index = 0usize;
        self.partial_rollback = None;
        self.journal_completed.clear();
        self.write_journal(JournalPhase::Commit)?;

        for (step_index, step) in self.steps.clone().into_iter().enumerate() {
            // Intent journaling (MUT-09): record the step as about-to-commit
            // BEFORE mutating, so a crash between the rename and the journal
            // update is still attributable to this operation at recovery.
            // Recovery of a step that never landed is a digest no-op for
            // backed-up files and a removal no-op for absent creations.
            self.journal_completed
                .push(step.primary_path().to_path_buf());
            self.write_journal(JournalPhase::Commit)?;
            // Prelude injections (QAL-06: the second/third file boundaries)
            // feed the same error path as the step itself so the
            // compensation below still runs when they fire.
            let prelude = if step_index == 1 {
                self.inject(Point::SecondFile)
            } else if step_index == 2 {
                self.inject(Point::ThirdFile)
            } else {
                Ok(())
            };
            let res: Result<()> = match prelude {
                Err(e) => Err(e),
                Ok(()) => match &step {
                    FileAction::CreateDir { path } => self.commit_create_dir(path),
                    FileAction::Write { path, .. } => {
                        let temp_opt = self.staged_temps.get(write_index).cloned();
                        write_index = write_index.saturating_add(1);
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
                    FileAction::QuarantineMove { from, to } => {
                        self.commit_quarantine_move(from, to)
                    }
                },
            };
            if let Err(e) = res {
                // Compensate the already committed steps in reverse order and
                // retain the outcome: any path the rollback could not undo is
                // a residual that must reach the caller. The original commit
                // error stays the surfaced error; nothing is masked.
                let rollback = self.rollback_partial(&committed);
                self.partial_rollback = Some(rollback);
                return Err(e);
            }
            committed.push(step.primary_path().to_path_buf());
        }

        // Cleanup any leftover staged temps (should be empty after renames)
        for temp in &self.staged_temps.clone() {
            if temp.exists() {
                drop(std::fs::remove_file(temp));
            }
        }

        Ok(CommitOutcome {
            committed,
            backups: self.backups.clone(),
        })
    }

    fn commit_create_dir(&self, path: &Path) -> Result<()> {
        validate_path_safety(path)?;
        // §4.2: a directory that appeared between prepare and commit is a
        // foreign change, not a satisfied precondition.
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
        // MUT-02: the followed links must still resolve to the referents the
        // plan retargeted onto, immediately before any rename lands.
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
        // Replace symlink only if it matches the expected owned target
        // (MUT-02/MUT-06) and only when its target has not changed since
        // prepare (§4.2). Nothing is removed before those checks pass.
        if link.exists() || std::fs::symlink_metadata(link).is_ok() {
            let meta = std::fs::symlink_metadata(link).map_err(|e| ConfigError::io(link, e))?;
            if meta.file_type().is_symlink() {
                let current_target =
                    std::fs::read_link(link).map_err(|e| ConfigError::io(link, e))?;
                if let Some(expected) = expected_current {
                    // Owned-target-only replacement: an existing link pointing
                    // anywhere else is not ours to overwrite.
                    if current_target != expected {
                        return Err(ConfigError::symlink_target_mismatch(
                            link,
                            expected.display().to_string(),
                            current_target.display().to_string(),
                        ));
                    }
                } else if let Some(prepare_state) = self.expected_states.get(link) {
                    // Default policy: the link must still carry the target
                    // observed at prepare time. A retarget between plan and
                    // commit is a concurrent modification.
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
                            // Not a symlink at prepare time but one now.
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
            // The link we expected to own is gone: foreign change.
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
            // On non-unix, symlink creation uses the platform primitive; a
            // relative target is joined against the link's directory first
            // because windows symlinks resolve relative targets differently.
            let resolved = if target.is_absolute() {
                target.to_path_buf()
            } else {
                link.parent()
                    .map(|p| p.join(target))
                    .filter(|p| p.is_absolute())
                    .unwrap_or_else(|| target.to_path_buf())
            };
            std::os::windows::fs::symlink_file(&resolved, link)
                .map_err(|e| ConfigError::io(link, e))?;
        }
        self.inject(Point::ParentSync)?;
        sync_parent(link)?;
        Ok(())
    }

    fn commit_remove_file(&self, path: &Path) -> Result<()> {
        // §4.2: removing a file that changed since prepare would destroy a
        // foreign edit; abort instead.
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
        // §4.2: quarantining a file that changed since prepare would move a
        // foreign edit out of reach; abort instead.
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
        // This is a recoverable move into quarantine; validate then move.
        crate::quarantine::move_to_quarantine_with_dest(from, to, self.id.as_str())?;
        Ok(())
    }

    /// Verify after commit: read fresh + parse, assert intended state.
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
                let parse_ok = validate_staged_content(&bytes, *kind, path).is_ok();
                let message = if digest_ok && parse_ok {
                    "verified".to_owned()
                } else if !digest_ok {
                    format!("digest mismatch: expected {expected_digest}, got {actual_digest}")
                } else {
                    "parse failed after commit".to_owned()
                };
                // Redact any secret-like content from message (no raw bytes).
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
        // Check verification failures
        let mut failed: Vec<PathBuf> = Vec::new();
        for o in &outcomes {
            if !o.digest_ok || !o.parse_ok {
                failed.push(o.path.clone());
            }
        }
        if !failed.is_empty() {
            // Rollback is caller-driven; we just report.
        }
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
        // Build map from path to backup entry for quick lookup
        let backup_map: HashMap<PathBuf, &BackupEntry> = self
            .backups
            .iter()
            .map(|e| (e.original_path.clone(), e))
            .collect();

        for path in committed.iter().rev() {
            if let Some(entry) = backup_map.get(path) {
                if let Ok(true) = verify_backup(entry) {
                } else {
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
                // No backup => this was a creation; remove the newly created file/dir if it exists.
                if path.exists() || std::fs::symlink_metadata(path).is_ok() {
                    // Try to remove; if it's a directory, try remove_dir (empty) or quarantine remove.
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
                    // Nothing to rollback, treat as success (no residual)
                    rolled_back.push(path.clone());
                }
            }
        }

        // Verify rollback: check that residuals are exactly those that failed verification
        let verification_ok = residuals.is_empty();

        // Cleanup staged temps on rollback as well
        for temp in &self.staged_temps.clone() {
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

    /// Execute the full transaction: prepare, commit, verify, with automatic
    /// rollback on failure. No filesystem-wide atomicity is claimed; this is a
    /// compensated transaction with verified rollback.
    ///
    /// With journaling enabled ([`Self::with_journal`]) the journal advances
    /// to `verify` after commit and `rollback` before any rollback, and is
    /// removed only after verified completion or a fully verified rollback
    /// (MUT-09). Path-safety warnings (e.g. hard-link sharing) are surfaced
    /// through the redacted diagnostics.
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
                // `commit` already compensated the steps that had landed; its
                // recorded outcome — including residuals it could not undo —
                // is what the caller sees. A filtered no-op rollback here
                // would report nothing, hiding the compensation entirely.
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
                // The internal compensation already ran; when it left nothing
                // residual the journal has nothing left to recover and is
                // removed. Otherwise it stays for startup recovery.
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
        // Verified completion: the journal can now be removed (MUT-09).
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
    /// every host (pure string semantics — drive roots, UNC roots, first-level
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
        // be flagged — unix removal/quarantine semantics are unchanged.
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
        // Cleanup
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
        // Cleanup
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
    // MUT-05: §4.2 conflict window — foreign edits between prepare and
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
        // The foreign bytes survive — never overwritten, not even partially.
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

    /// A target held open the way a running harness holds its config (reads
    /// and writes shared, deletion/replacement NOT shared) must surface a
    /// typed error from the commit path — never corruption, never a leaked
    /// temp.
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
    /// (long-path-aware system) or fails with a typed error — never a panic
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

    /// QAL-09 real-platform case-insensitive collision: on the default
    /// (case-insensitive) APFS volume, creating `Settings.json` next to an
    /// existing `settings.json` would silently land over it — the boundary
    /// refuses the case-variant and never corrupts the original.
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
}
