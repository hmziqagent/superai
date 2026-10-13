//! Criterion benches over the discovery/detect hot lanes. Measurement only:
//! every lane calls the production function unchanged; adapter version
//! resolution is deliberately absent because `--version` probes of the live
//! PATH would make the numbers host-dependent noise.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// The lib's regular deps ride along as externs of this target; none are
// referenced directly, so mark them used for `unused_crate_dependencies`.
use duct as _;
use hex as _;
use semver as _;
use serde as _;
use serde_json as _;
use sha2 as _;
use superai_config as _;
use thiserror as _;
use toml_edit as _;
use toride_apps as _;
use toride_proxy as _;
use toride_registry as _;
use toride_runner as _;
use toride_wireguard as _;
use ureq as _;
use yaml_serde as _;

use criterion::{Criterion, criterion_group, criterion_main};
use superai_core::Registry;
#[cfg(unix)]
use superai_core::detect::{self, DetectOptions};
use superai_core::discovery::{self, ScanOptions};
use superai_core::harness_catalog;
#[cfg(unix)]
use superai_core::process;

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-core-bench-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("bench scratch dir must create");
        Self { dir }
    }

    fn path(&self, name: impl AsRef<Path>) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_dir_all(&self.dir) {
            eprintln!("bench scratch cleanup failed: {e}");
        }
    }
}

/// The pre-round-1 boundary walk: one `from_utf8` per backed-off byte.
fn old_backoff_text(data: &[u8], max_bytes: usize) -> String {
    let len = std::cmp::min(data.len(), max_bytes);
    let slice = data.get(..len).unwrap_or_default();
    let mut valid_len = slice.len();
    while valid_len > 0 && std::str::from_utf8(slice.get(..valid_len).unwrap_or_default()).is_err()
    {
        valid_len = valid_len.saturating_sub(1);
    }
    String::from_utf8_lossy(slice.get(..valid_len).unwrap_or_default()).into_owned()
}

/// The shipped single-pass boundary via [`Utf8Error::valid_up_to`],
/// mirroring discovery's `read_bounded` arm for arm.
fn valid_up_to_text(data: &[u8], max_bytes: usize) -> String {
    let len = std::cmp::min(data.len(), max_bytes);
    let slice = data.get(..len).unwrap_or_default();
    match std::str::from_utf8(slice) {
        Ok(text) => text.to_owned(),
        Err(err) => {
            let valid = slice.get(..err.valid_up_to()).unwrap_or_default();
            String::from_utf8_lossy(valid).into_owned()
        }
    }
}

/// Schema text with one invalid byte at the midpoint of the 64KiB cap the
/// fingerprint readers charge, and enough trailing content that the cap
/// truncates: the shape where the byte-at-a-time backoff paid O(cap^2 / 2)
/// validations and `valid_up_to` pays one.
fn payload_with_midcap_invalid_byte() -> Vec<u8> {
    let mut payload = Vec::with_capacity(72 * 1024);
    for i in 0..750 {
        payload.extend_from_slice(
            format!("\"key-{i}\": \"value with words and more words\",\n").as_bytes(),
        );
    }
    payload.push(0xFF);
    payload
        .extend_from_slice(b"tail content after the first invalid byte, padded out past the cap");
    payload.extend_from_slice(&vec![b'x'; 32 * 1024]);
    payload
}

fn bench_read_bounded_algorithms(c: &mut Criterion) {
    let payload = payload_with_midcap_invalid_byte();
    assert_eq!(
        old_backoff_text(&payload, 64 * 1024),
        valid_up_to_text(&payload, 64 * 1024),
        "algorithm copies must agree"
    );
    let mut group = c.benchmark_group("read_bounded");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("boundary/old_byte_backoff", |b| {
        b.iter(|| old_backoff_text(std::hint::black_box(&payload), 64 * 1024));
    });
    group.bench_function("boundary/valid_up_to", |b| {
        b.iter(|| valid_up_to_text(std::hint::black_box(&payload), 64 * 1024));
    });
    group.finish();
}

fn bench_fingerprint_candidate(c: &mut Criterion) {
    let scratch = Scratch::new("fingerprint");
    let root = scratch.path("cand");
    fs::create_dir_all(&root).expect("candidate dir must create");
    fs::write(
        root.join("settings.json"),
        payload_with_midcap_invalid_byte(),
    )
    .expect("settings payload must write");
    let mut group = c.benchmark_group("fingerprint");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("candidate_with_invalid_byte", |b| {
        b.iter(|| discovery::fingerprint_candidate(std::hint::black_box(&root)));
    });
    group.finish();
}

fn bench_scan_wrapper_dirs(c: &mut Criterion) {
    let scratch = Scratch::new("wrapper_dirs");
    let dir = scratch.path("bin");
    fs::create_dir_all(&dir).expect("wrapper dir must create");
    for i in 0..32 {
        let body = if i % 4 == 0 {
            "#!/bin/sh\nexec mise x -- tool \"$@\"\n"
        } else {
            "#!/bin/sh\nexec real-binary \"$@\"\n"
        };
        fs::write(dir.join(format!("tool-{i:02}")), body).expect("wrapper fixture must write");
    }
    let registry = Registry::default();
    let mut group = c.benchmark_group("scan_wrapper_dirs");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("32_files", |b| {
        b.iter(|| {
            discovery::scan_wrapper_dirs(
                std::hint::black_box(std::slice::from_ref(&dir)),
                &registry,
            )
        });
    });
    group.finish();
}

fn bench_drift_report(c: &mut Criterion) {
    let scratch = Scratch::new("drift");
    let mut roots = Vec::new();
    for i in 0..8 {
        let root = scratch.path(format!("custom-{i}"));
        fs::create_dir_all(&root).expect("candidate root must create");
        roots.push(root);
    }
    let options = ScanOptions {
        extra_roots: roots,
        wrapper_dirs: Vec::new(),
        max_entries: 64,
    };
    let registry = Registry::default();
    let mut group = c.benchmark_group("drift_report");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("eight_unmanaged_candidates", |b| {
        b.iter(|| {
            discovery::drift_report_with_options(
                std::hint::black_box(&registry),
                std::hint::black_box(scratch.dir.as_path()),
                std::hint::black_box(&options),
            )
        });
    });
    group.finish();
}

fn bench_drift_report_foreign_files(c: &mut Criterion) {
    let scratch = Scratch::new("drift_foreign");
    fs::create_dir_all(scratch.path(".conductor")).expect("conductor dir must create");
    fs::write(
        scratch.path(".conductor/settings.toml"),
        "theme = \"dark\"\n",
    )
    .expect("conductor settings must write");
    fs::create_dir_all(scratch.path(".sculptor")).expect("sculptor dir must create");
    fs::write(scratch.path(".sculptor/.env"), "SCULPTOR_LOG=debug\n")
        .expect("sculptor env must write");
    let mut roots = Vec::new();
    for i in 0..8 {
        let root = scratch.path(format!("custom-{i}"));
        fs::create_dir_all(&root).expect("candidate root must create");
        roots.push(root);
    }
    let options = ScanOptions {
        extra_roots: roots,
        wrapper_dirs: Vec::new(),
        max_entries: 64,
    };
    let registry = Registry::default();
    let mut group = c.benchmark_group("drift_report");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("eight_candidates_with_foreign_files_present", |b| {
        b.iter(|| {
            discovery::drift_report_with_options(
                std::hint::black_box(&registry),
                std::hint::black_box(scratch.dir.as_path()),
                std::hint::black_box(&options),
            )
        });
    });
    group.finish();
}

/// The corpus build `scan_with_diagnostics` paid per call before the
/// `OnceLock`; the shipped path now pays it once per process.
fn bench_adapter_corpus_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("harness_catalog");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("all_adapters_corpus", |b| {
        b.iter(|| harness_catalog::all_adapters().len());
    });
    group.finish();
}

#[cfg(unix)]
fn write_fake_manager(dir: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join(name);
    fs::write(&path, body).expect("fake manager must write");
    let mut perms = fs::metadata(&path)
        .expect("fake manager must stat")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("fake manager must become executable");
}

/// The pipx/uv/dpkg probe families against deterministic fake managers
/// resolved from the injected PATH dirs (npm/brew/mise resolve bare names
/// through the ambient PATH, so host binaries would drown the signal):
/// three real spawns per call, spread over concurrent families instead of
/// one sequential chain.
#[cfg(unix)]
fn bench_detect_fixtures(
    tag: &str,
) -> (
    Scratch,
    superai_core::install_catalog::InstallCatalogEntry,
    DetectOptions,
    PathBuf,
) {
    use superai_core::install_catalog::{
        DetectHints, InstallCatalogEntry, InstallMethod, InstallMethodKind, PlatformConstraints,
    };
    let scratch = Scratch::new(tag);
    let bin = scratch.path("bin");
    let home = scratch.path("home");
    fs::create_dir_all(&bin).expect("detect bin dir must create");
    fs::create_dir_all(&home).expect("detect home must create");
    write_fake_manager(
        &bin,
        "pipx",
        "#!/bin/sh\necho '{\"venvs\":{\"my-pkg\":{\"package\":{\"package_name\":\"my-pkg\",\"package_version\":\"1.4.2\"}}}}'\n",
    );
    write_fake_manager(&bin, "uv", "#!/bin/sh\necho 'my-pkg v1.4.2'\n");
    write_fake_manager(
        &bin,
        "dpkg",
        "#!/bin/sh\nprintf 'Package: my-pkg\\nStatus: install ok installed\\nVersion: 1.4.2\\n'\n",
    );
    let method = |kind: InstallMethodKind| InstallMethod {
        kind,
        package_name: "my-pkg".to_owned(),
        tap: None,
        repo: None,
        registry: None,
    };
    let entry = InstallCatalogEntry {
        harness: "bench-py-harness".to_owned(),
        executables: vec!["my-exe".to_owned()],
        bundle_ids: Vec::new(),
        apps: Vec::new(),
        methods: vec![
            method(InstallMethodKind::Pipx),
            method(InstallMethodKind::Uv),
            method(InstallMethodKind::Direct),
        ],
        version_source: "my-exe --version".to_owned(),
        constraints: PlatformConstraints {
            os: vec!["any".to_owned()],
            arch: vec!["any".to_owned()],
        },
        detect: DetectHints::default(),
        requires_admin: false,
        checksum: None,
        conflicts: Vec::new(),
        docs: "https://example.com".to_owned(),
        last_verified: "2026-08-26".to_owned(),
    };
    let opts = DetectOptions {
        path_dirs: Some(vec![bin.clone()]),
        home_dir: Some(home),
        probe_mise: false,
        probe_brew: false,
        probe_npm: false,
        probe_cargo: false,
        probe_pipx: true,
        probe_uv: true,
        probe_system: true,
        probe_apps: false,
        probe_timeout: Duration::from_secs(2),
        ..DetectOptions::default()
    };
    (scratch, entry, opts, bin)
}

#[cfg(unix)]
fn bench_detect_package_probes(c: &mut Criterion) {
    let (_scratch, entry, opts, bin) = bench_detect_fixtures("detect");
    let exec_opts = process::ExecuteOpts {
        timeout: Some(Duration::from_secs(2)),
        ..process::ExecuteOpts::default()
    };
    let pipx = bin.join("pipx").to_string_lossy().into_owned();
    let uv = bin.join("uv").to_string_lossy().into_owned();
    let mut group = c.benchmark_group("detect");
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("three_manager_families", |b| {
        b.iter(|| {
            detect::detect_all_for_entry(std::hint::black_box(&entry), &opts)
                .iter()
                .filter(|d| d.version.is_some())
                .count()
        });
    });
    group.bench_function("spawn_baseline_sequential_pair", |b| {
        b.iter(|| {
            process::run_command(
                std::hint::black_box(pipx.as_str()),
                &["list".to_owned(), "--json".to_owned()],
                &exec_opts,
            )
            .expect("fake pipx must answer");
            process::run_command(
                std::hint::black_box(uv.as_str()),
                &["tool".to_owned(), "list".to_owned()],
                &exec_opts,
            )
            .expect("fake uv must answer");
        });
    });
    group.finish();
}

/// The mixed case: a PATH hit whose version probe answers slowly must not
/// delay the package-manager families. Wave overlap keeps the call near
/// max(100ms, package spawns); re-serialization shows up as their sum.
#[cfg(unix)]
fn bench_detect_slow_path_hit_with_managers(c: &mut Criterion) {
    let (scratch, entry, opts, bin) = bench_detect_fixtures("detect-slow");
    let slow_dir = scratch.path("slowbin");
    fs::create_dir_all(&slow_dir).expect("slow bin dir must create");
    let slow = slow_dir.join("my-exe");
    fs::write(&slow, "#!/bin/sh\nsleep 0.1\necho \"slow 7.7.7\"\n").expect("slow exe must write");
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = fs::metadata(&slow)
            .expect("slow exe must stat")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&slow, perms).expect("slow exe must become executable");
    }
    let opts_with_hit = DetectOptions {
        path_dirs: Some(vec![slow_dir, bin]),
        ..opts
    };
    let mut group = c.benchmark_group("detect");
    group.measurement_time(Duration::from_secs(6));
    group.sample_size(30);
    group.bench_function("slow_path_hit_with_managers", |b| {
        b.iter(|| {
            detect::detect_all_for_entry(std::hint::black_box(&entry), &opts_with_hit)
                .iter()
                .filter(|d| d.version.is_some())
                .count()
        });
    });
    group.finish();
}

/// The mise-present case: the hanging shim execs its sleep so the probe
/// budget's kill reclaims the pipe, letting the call land near
/// max(shim budget, manager budget). Re-serializing the shim probes behind
/// the families shows up as the two budgets added together. A spawn-shim
/// (no exec) is also bounded: `run_command`'s post-kill reap abandons a
/// pipe-holding descendant after `REAP_GRACE`, so wall time never tracks
/// the orphan's lifetime (pinned in process.rs).
#[cfg(unix)]
fn bench_detect_broken_shim_with_slow_manager(c: &mut Criterion) {
    let (scratch, entry, opts, bin) = bench_detect_fixtures("detect-shim");
    let shims = scratch.path("home").join(".local/share/mise/shims");
    fs::create_dir_all(&shims).expect("shim dir must create");
    write_fake_manager(&shims, "my-exe", "#!/bin/sh\nexec sleep 2\n");
    write_fake_manager(
        &bin,
        "dpkg",
        "#!/bin/sh\nsleep 1\nprintf 'Package: my-pkg\\nStatus: install ok installed\\nVersion: 1.4.2\\n'\n",
    );
    let opts = DetectOptions {
        probe_mise: true,
        probe_timeout: Duration::from_millis(400),
        ..opts
    };
    let mut group = c.benchmark_group("detect");
    group.measurement_time(Duration::from_secs(6));
    group.sample_size(10);
    group.bench_function("broken_shim_with_slow_manager", |b| {
        b.iter(|| {
            detect::detect_all_for_entry(std::hint::black_box(&entry), &opts)
                .iter()
                .filter(|d| d.version.is_some())
                .count()
        });
    });
    group.finish();
}

/// The per-PATH-hit lane: eight shadowed hits each pay a version spawn plus
/// a `file` arch spawn, run four hits wide instead of one at a time. The
/// sequential shape this replaced pays all sixteen spawns end to end.
#[cfg(unix)]
fn bench_detect_eight_path_hits(c: &mut Criterion) {
    let (scratch, entry, _opts, _bin) = bench_detect_fixtures("detect-hits");
    let mut dirs = Vec::new();
    for i in 0..8 {
        let dir = scratch.path(format!("hit-{i}"));
        fs::create_dir_all(&dir).expect("hit dir must create");
        write_fake_manager(&dir, "my-exe", &format!("#!/bin/sh\necho \"{i}.1.0\"\n"));
        dirs.push(dir);
    }
    let opts = DetectOptions {
        path_dirs: Some(dirs),
        probe_mise: false,
        probe_brew: false,
        probe_npm: false,
        probe_cargo: false,
        probe_pipx: false,
        probe_uv: false,
        probe_system: false,
        probe_apps: false,
        ..DetectOptions::default()
    };
    let mut group = c.benchmark_group("detect");
    group.measurement_time(Duration::from_secs(4));
    group.bench_function("eight_path_hits", |b| {
        b.iter(|| {
            detect::detect_all_for_entry(std::hint::black_box(&entry), &opts)
                .iter()
                .filter(|d| d.version.is_some())
                .count()
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_read_bounded_algorithms,
    bench_fingerprint_candidate,
    bench_scan_wrapper_dirs,
    bench_drift_report,
    bench_drift_report_foreign_files,
    bench_adapter_corpus_build,
);

#[cfg(unix)]
criterion_group!(
    unix_benches,
    bench_detect_package_probes,
    bench_detect_slow_path_hit_with_managers,
    bench_detect_broken_shim_with_slow_manager,
    bench_detect_eight_path_hits,
);

#[cfg(unix)]
criterion_main!(benches, unix_benches);

#[cfg(not(unix))]
criterion_main!(benches);
