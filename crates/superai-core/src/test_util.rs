//! Per-test isolated filesystem helper.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Create a unique temporary directory for a per-test isolated filesystem
/// (millis + atomic counter + pid + hash). No `HOME` or cwd mutation.
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

/// Cross-platform replacement for unix-only `/tmp/...` literals: a unique,
/// created absolute dir (`/tmp` has no drive on Windows).
pub(crate) fn tmp_abs(prefix: &str) -> PathBuf {
    temp_dir_unique(prefix)
}

/// String form of [`tmp_abs`] for `&str` call sites. Rendered through
/// `components` so embedded `/` become native separators on Windows.
pub(crate) fn tmp_abs_str(prefix: &str) -> String {
    let dir = temp_dir_unique(prefix);
    let mut native = PathBuf::new();
    for comp in dir.components() {
        native.push(comp.as_os_str());
    }
    native.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::Path;

    #[test]
    fn temp_dir_unique_is_isolated_and_exists() {
        let dir = temp_dir_unique("core-iso");
        assert!(dir.exists());
        assert!(dir.is_dir());
        let file = dir.join("probe.txt");
        std::fs::write(&file, b"hello").unwrap();
        assert!(file.exists());
        drop(std::fs::remove_dir_all(&dir));
        assert!(!dir.exists());
    }

    #[test]
    fn parallel_100_threads_no_collision() {
        let threads: usize = 100;
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                std::thread::spawn(move || {
                    let dir = temp_dir_unique("core-parallel");
                    assert!(dir.exists(), "thread {i} dir missing");
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

    #[test]
    fn tmp_abs_returns_absolute_created_dir() {
        for dir in [tmp_abs("core-tmp-abs"), tmp_abs("core-tmp-abs")] {
            assert!(dir.is_absolute(), "tmp_abs must be absolute: {dir:?}");
            assert!(
                dir.exists() && dir.is_dir(),
                "tmp_abs must be created: {dir:?}"
            );
            drop(std::fs::remove_dir_all(&dir));
        }
        let s = tmp_abs_str("core-tmp-abs-str");
        assert!(
            Path::new(&s).is_absolute(),
            "tmp_abs_str must be absolute: {s}"
        );
        assert!(
            Path::new(&s).exists(),
            "tmp_abs_str dir must be created: {s}"
        );
        drop(std::fs::remove_dir_all(&s));
    }
}
