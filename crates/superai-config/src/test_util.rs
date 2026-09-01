//! Per-test isolated filesystem helper.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Environment variable selecting the retain-on-failure policy (QAL-01).
pub(crate) const KEEP_ENV: &str = "SUPERAI_TEST_KEEP";

/// Retain-on-failure policy for [`TempDir`] (QAL-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeepMode {
    /// Delete on drop (default; the suite stays hermetic).
    No,
    /// Keep the directory when the dropping test panicked.
    Failed,
    /// Keep every directory (debugging aid).
    All,
}

/// Parse the policy from the environment variable value.
pub(crate) fn keep_mode_from_env_value(value: Option<&str>) -> KeepMode {
    match value {
        Some("all" | "ALL") => KeepMode::All,
        Some("failed" | "FAILED") => KeepMode::Failed,
        _ => KeepMode::No,
    }
}

/// Read the policy from the process environment.
pub(crate) fn keep_mode_from_env() -> KeepMode {
    keep_mode_from_env_value(std::env::var(KEEP_ENV).ok().as_deref())
}

thread_local! {
    /// Whether a panic has been observed on this thread. The default test
    /// harness runs each test on its own thread, so a thread-local flag is a
    /// per-test failure signal for the retain-on-failure policy.
    static PANICKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Install the panic tracker used by retain-on-failure. Idempotent; chains to
/// the previous hook so test failure output is unchanged.
pub(crate) fn install_panic_tracker() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            PANICKED.with(|flag| flag.set(true));
            previous(info);
        }));
    });
}

/// Create a unique temporary directory for a per-test isolated filesystem.
///
/// Uses `SystemTime` millis, an atomic counter, process id, and a hasher
/// for uniqueness. The directory is created on disk. No global `HOME` or
/// cwd mutation is performed.
pub(crate) fn temp_dir_unique(prefix: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut hasher = DefaultHasher::new();
    millis.hash(&mut hasher);
    count.hash(&mut hasher);
    pid.hash(&mut hasher);
    prefix.hash(&mut hasher);
    let hash = hasher.finish() & 0xffff;
    let dir = std::env::temp_dir().join(format!(
        "superai-test-{prefix}-{millis}-{pid}-{count:04x}-{hash:04x}"
    ));
    drop(std::fs::create_dir_all(&dir));
    dir
}

/// Clear the Windows readonly attribute from every file under `root`.
///
/// `std::fs::remove_dir_all` cannot delete readonly files on Windows; the
/// suite never creates them deliberately, but backups of readonly sources
/// legitimately carry the attribute and must still clean up.
#[cfg(windows)]
fn clear_readonly_recursive(root: &Path) {
    fn clear_one(path: &Path) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        if !meta.is_file() {
            return;
        }
        let mut perm = meta.permissions();
        if perm.readonly() {
            // Windows-only code path: the readonly attribute is the only
            // permission bit that exists there.
            #[expect(
                clippy::permissions_set_readonly_false,
                reason = "windows-only path; the readonly attribute is the only permission bit"
            )]
            perm.set_readonly(false);
            drop(std::fs::set_permissions(path, perm));
        }
    }
    fn visit_dir(dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            visit(&entry.path());
        }
    }

    fn visit(path: &Path) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        if meta.is_dir() {
            visit_dir(path);
            return;
        }
        clear_one(path);
    }
    visit(root);
}

/// RAII temporary directory that cleans up on drop, honoring the
/// retain-on-failure policy from [`KEEP_ENV`] (QAL-01).
#[derive(Debug)]
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create a new isolated temporary directory with the given prefix.
    ///
    /// Installs the panic tracker so `SUPERAI_TEST_KEEP=failed` can observe
    /// the dropping test's failure state.
    pub(crate) fn new(prefix: &str) -> Self {
        install_panic_tracker();
        Self {
            path: temp_dir_unique(prefix),
        }
    }

    /// Borrow the directory path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Join a file name onto the temp directory.
    pub(crate) fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// The decision applied at drop time, exposed for tests so the policy can
    /// be exercised without mutating the process environment.
    pub(crate) fn should_keep(mode: KeepMode, panicked: bool) -> bool {
        match mode {
            KeepMode::All => true,
            KeepMode::Failed => panicked,
            KeepMode::No => false,
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let panicked = PANICKED.with(std::cell::Cell::get);
        let keep = Self::should_keep(keep_mode_from_env(), panicked);
        if keep {
            eprintln!(
                "superai test util: keeping temp dir {} (SUPERAI_TEST_KEEP)",
                self.path.display()
            );
            return;
        }
        #[cfg(windows)]
        clear_readonly_recursive(&self.path);
        drop(std::fs::remove_dir_all(&self.path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn temp_dir_unique_is_isolated_and_exists() {
        let dir = temp_dir_unique("config-iso");
        assert!(dir.exists());
        assert!(dir.is_dir());
        // Write a file to ensure isolation.
        let file = dir.join("probe.txt");
        std::fs::write(&file, b"hello").unwrap();
        assert!(file.exists());
        drop(std::fs::remove_dir_all(&dir));
        assert!(!dir.exists());
    }

    #[test]
    fn temp_dir_drop_cleans_up() {
        let path: PathBuf;
        {
            let tmp = TempDir::new("config-drop");
            path = tmp.path().to_path_buf();
            assert!(path.exists());
            std::fs::write(tmp.join("x"), b"y").unwrap();
        }
        assert!(!path.exists(), "TempDir should clean up on drop");
    }

    #[test]
    fn parallel_100_threads_no_collision() {
        let threads: usize = 100;
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                std::thread::spawn(move || {
                    let dir = temp_dir_unique("config-parallel");
                    assert!(dir.exists(), "thread {i} dir missing");
                    // Ensure we can create a file inside.
                    let probe = dir.join("t.txt");
                    std::fs::write(&probe, format!("{i}").as_bytes()).unwrap();
                    assert_eq!(std::fs::read_to_string(&probe).unwrap(), format!("{i}"));
                    dir
                })
            })
            .collect();
        let mut seen = HashSet::new();
        for h in handles {
            let dir = h.join().expect("thread panicked");
            assert!(seen.insert(dir.clone()), "duplicate dir {dir:?}");
            drop(std::fs::remove_dir_all(&dir));
        }
        assert_eq!(seen.len(), threads);
    }

    // ---- QAL-01: retain-on-failure policy ----

    #[test]
    fn keep_mode_parses_env_values() {
        assert_eq!(keep_mode_from_env_value(None), KeepMode::No);
        assert_eq!(keep_mode_from_env_value(Some("")), KeepMode::No);
        assert_eq!(keep_mode_from_env_value(Some("bogus")), KeepMode::No);
        assert_eq!(keep_mode_from_env_value(Some("all")), KeepMode::All);
        assert_eq!(keep_mode_from_env_value(Some("ALL")), KeepMode::All);
        assert_eq!(keep_mode_from_env_value(Some("failed")), KeepMode::Failed);
        assert_eq!(keep_mode_from_env_value(Some("FAILED")), KeepMode::Failed);
    }

    #[test]
    fn keep_decision_matrix() {
        // default: always delete
        assert!(!TempDir::should_keep(KeepMode::No, false));
        assert!(!TempDir::should_keep(KeepMode::No, true));
        // failed: keep only when the dropping test panicked
        assert!(!TempDir::should_keep(KeepMode::Failed, false));
        assert!(TempDir::should_keep(KeepMode::Failed, true));
        // all: always keep
        assert!(TempDir::should_keep(KeepMode::All, false));
        assert!(TempDir::should_keep(KeepMode::All, true));
    }

    #[test]
    fn drop_respects_injected_keep_decision() {
        // The drop-path behavior for each policy, driven through the same
        // decision the Drop impl applies (the env var itself is read at drop
        // time and cannot be safely mutated in a parallel test process).
        for (mode, panicked, kept) in [
            (KeepMode::No, false, false),
            (KeepMode::No, true, false),
            (KeepMode::Failed, false, false),
            (KeepMode::Failed, true, true),
            (KeepMode::All, false, true),
        ] {
            let dir = temp_dir_unique("config-keep");
            std::fs::write(dir.join("marker"), b"m").unwrap();
            assert_eq!(
                TempDir::should_keep(mode, panicked),
                kept,
                "{mode:?} with panicked={panicked}"
            );
            drop(std::fs::remove_dir_all(&dir));
            assert!(!dir.exists(), "manual cleanup always removes the dir");
        }
    }

    #[test]
    fn panic_tracker_flags_panicking_thread_only() {
        install_panic_tracker();
        let result = std::panic::catch_unwind(|| {
            panic!("intentional tracker probe");
        });
        assert!(result.is_err(), "probe must have panicked");
        assert!(
            PANICKED.with(std::cell::Cell::get),
            "the panicking thread must be flagged"
        );
        // A separate thread stays unflagged.
        let other = std::thread::spawn(|| PANICKED.with(std::cell::Cell::get))
            .join()
            .unwrap_or(true);
        assert!(!other, "other threads must not inherit the flag");
        // The flag is observable exactly where Drop reads it.
        assert!(TempDir::should_keep(
            KeepMode::Failed,
            PANICKED.with(std::cell::Cell::get)
        ));
    }
}
