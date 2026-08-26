//! Per-test isolated filesystem helper.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

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

/// RAII temporary directory that cleans up on drop.
#[derive(Debug)]
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create a new isolated temporary directory with the given prefix.
    pub(crate) fn new(prefix: &str) -> Self {
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
}

impl Drop for TempDir {
    fn drop(&mut self) {
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
}
