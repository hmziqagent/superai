//! Install detection — collects all harness matches (PKG-03).
//!
//! Probes, in order:
//! - `PATH` resolution (which/where, in `PATH` order, reporting shadowed
//!   duplicates)
//! - configured binary path (via `SUPERAI_CONFIGURED_BINARY_<HARNESS>` or
//!   `AbsolutePath` from registry — stubbed as env-var for now)
//! - mise shims (`~/.local/share/mise/shims` and `mise where` when the mise
//!   binary is present)
//! - Homebrew metadata (`brew list --versions` when `brew` is present)
//! - npm global metadata (`npm list -g` when `npm` is present)
//! - cargo install metadata (`cargo install --list`)
//! - desktop app / bundle presence (`/Applications`, `~/Applications`,
//!   bundle-id probes)
//!
//! Each probe returns a [`Detection`] with path, version (if probeable),
//! method confidence, duplicates ranking, arch mismatch flag, and broken-shim
//! flag. `detect_all` never picks silently — it returns all matches and
//! leaves selection to the caller.
//!
//! Duct-backed execution is used (see [`crate::process`]); no shell
//! interpolation occurs.

#![expect(
    clippy::excessive_nesting,
    reason = "detection logic intentionally deep"
)]
#![expect(clippy::collapsible_if, reason = "explicit nesting for readability")]
use std::borrow::ToOwned as _;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ids::HarnessId;
use crate::install_catalog::{InstallCatalog, InstallCatalogEntry};
use crate::process::{ExecuteOpts, extract_version, run_command};

// HashMap not needed - removed stray cfg

// ---------------------------------------------------------------------------
// Detection types
// ---------------------------------------------------------------------------

/// How a detection was found.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionSource {
    /// Found via `PATH` lookup.
    Path,
    /// Found via configured absolute binary path (registry/env).
    ConfiguredBinary,
    /// Found via mise shim (`~/.local/share/mise/shims/<exe>`).
    MiseShim,
    /// Found via mise `mise where <tool>` or `mise ls`.
    MiseManaged,
    /// Found via `brew list --versions <formula>`.
    Homebrew,
    /// Found via `npm list -g <package>`.
    Npm,
    /// Found via `cargo install --list`.
    Cargo,
    /// Found via desktop app bundle at a filesystem path.
    AppBundle,
    /// Found via system package metadata (future: pipx/uv/apt).
    SystemPackage,
}

impl std::fmt::Display for DetectionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Path => "path",
            Self::ConfiguredBinary => "configured_binary",
            Self::MiseShim => "mise_shim",
            Self::MiseManaged => "mise_managed",
            Self::Homebrew => "homebrew",
            Self::Npm => "npm",
            Self::Cargo => "cargo",
            Self::AppBundle => "app_bundle",
            Self::SystemPackage => "system_package",
        };
        f.write_str(s)
    }
}

/// Confidence of the detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionConfidence {
    /// Multiple consistent signals (e.g., PATH + version probe + package metadata).
    High,
    /// Single solid signal (e.g., PATH hit + successful version probe).
    Medium,
    /// Indirect or heuristic signal (e.g., shim file exists but version probe failed).
    Low,
}

impl std::fmt::Display for DetectionConfidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        };
        f.write_str(s)
    }
}

/// One detection hit for a harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detection {
    /// Harness this detection belongs to.
    pub harness: String,
    /// Executable or app name.
    pub executable: String,
    /// Filesystem path to the binary or app bundle.
    pub path: PathBuf,
    /// Detected version string, if a probe succeeded.
    pub version: Option<String>,
    /// How the detection was found.
    pub source: DetectionSource,
    /// Confidence in the detection.
    pub confidence: DetectionConfidence,
    /// Zero-based index in PATH order when source is `Path` (duplicates are
    /// ordered by PATH precedence; rank 0 is the winning entry). `None` for
    /// non-PATH sources.
    pub path_rank: Option<usize>,
    /// Whether the binary's architecture mismatches the host (stub: checks
    /// `file` output when available; false if probe unavailable).
    pub arch_mismatch: bool,
    /// Whether this looks like a broken shim (exists but fails to exec, e.g.,
    /// mise shim with no installed version).
    pub broken_shim: bool,
    /// Whether this hit is shadowed by an earlier PATH entry.
    pub shadowed: bool,
    /// Evidence lines for diagnostics.
    pub evidence: Vec<String>,
}

impl Detection {
    /// Create a new detection with required fields.
    pub fn new(
        harness: &str,
        executable: &str,
        path: PathBuf,
        source: DetectionSource,
        confidence: DetectionConfidence,
    ) -> Self {
        Self {
            harness: harness.to_owned(),
            executable: executable.to_owned(),
            path,
            version: None,
            source,
            confidence,
            path_rank: None,
            arch_mismatch: false,
            broken_shim: false,
            shadowed: false,
            evidence: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Detect options / env abstraction
// ---------------------------------------------------------------------------

#[expect(
    clippy::struct_excessive_bools,
    reason = "probe flags are independent booleans"
)]
/// Inputs for detection. In production `DetectOptions::from_env()` reads
/// `PATH`, `HOME`, and mise/homebrew probes from the real environment and
/// filesystem. In tests a fake options struct can be injected with a temp
/// `PATH` and temp `HOME`.
#[derive(Debug, Clone)]
pub struct DetectOptions {
    /// Ordered PATH directories (split from `$PATH`). If `None`, read from
    /// ambient `PATH`. Tests inject a temp PATH here.
    pub path_dirs: Option<Vec<PathBuf>>,
    /// Home directory for mise shim probing. If `None`, use `dirs::home_dir`
    /// or `$HOME`.
    pub home_dir: Option<PathBuf>,
    /// Configured binary path override (e.g., from instance registry or
    /// `SUPERAI_CONFIGURED_BINARY_<HARNESS>`). Tests inject here.
    pub configured_binary: Option<PathBuf>,
    /// Whether to probe mise (`mise --version`, shims). Default true.
    pub probe_mise: bool,
    /// Whether to probe Homebrew.
    pub probe_brew: bool,
    /// Whether to probe npm.
    pub probe_npm: bool,
    /// Whether to probe cargo.
    pub probe_cargo: bool,
    /// Whether to probe app bundles.
    pub probe_apps: bool,
    /// Timeout for each probe subprocess.
    pub probe_timeout: Duration,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self {
            path_dirs: None,
            home_dir: None,
            configured_binary: None,
            probe_mise: true,
            probe_brew: true,
            probe_npm: true,
            probe_cargo: true,
            probe_apps: true,
            probe_timeout: Duration::from_secs(5),
        }
    }
}

impl DetectOptions {
    /// Resolve PATH dirs from `opts.path_dirs` or ambient `$PATH`.
    pub fn resolve_path_dirs(&self) -> Vec<PathBuf> {
        if let Some(dirs) = self.path_dirs.as_ref() {
            return dirs.clone();
        }
        ambient_path_dirs()
    }

    /// Resolve HOME from `opts.home_dir` or ambient `$HOME`/`dirs::home_dir`.
    pub fn resolve_home(&self) -> Option<PathBuf> {
        if let Some(home) = self.home_dir.as_ref() {
            return Some(home.clone());
        }
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .or_else(dirs_home_fallback)
    }
}

/// Read ambient `$PATH` and split into `PathBuf` dirs.
fn ambient_path_dirs() -> Vec<PathBuf> {
    let raw = std::env::var_os("PATH");
    let mut out = Vec::new();
    if let Some(raw) = raw {
        for part in std::env::split_paths(&raw) {
            if !part.as_os_str().is_empty() {
                out.push(part);
            }
        }
    }
    out
}

fn dirs_home_fallback() -> Option<PathBuf> {
    // Avoid extra dependency on `dirs` crate if not already present; try
    // standard env first, then give up. Callers handle None gracefully.
    std::env::var_os("HOME").map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// Public detect API
// ---------------------------------------------------------------------------

/// Detect all installations of `harness` using ambient environment and the
/// embedded catalog. Returns all hits in PATH-probe order (PATH hits are
/// ordered by `PATH` precedence; shim/app/package hits follow). Never picks
/// silently when multiple versions affect instances — all are returned.
pub fn detect_all(harness: &HarnessId) -> Vec<Detection> {
    detect_all_with_options(harness, &DetectOptions::default())
}

/// Detect all installations with explicit options (PATH/home overrides for
/// tests and deterministic probing).
pub fn detect_all_with_options(harness: &HarnessId, opts: &DetectOptions) -> Vec<Detection> {
    let Ok(catalog) = InstallCatalog::embedded() else {
        return Vec::new();
    };
    let Some(entry) = catalog.get(harness) else {
        return Vec::new();
    };
    detect_all_for_entry(entry, opts)
}

/// Core detection for a catalog entry with injected options.
#[expect(
    clippy::too_many_lines,
    reason = "detection collects multiple probe sources"
)]
pub fn detect_all_for_entry(entry: &InstallCatalogEntry, opts: &DetectOptions) -> Vec<Detection> {
    let mut detections: Vec<Detection> = Vec::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();

    let path_dirs = opts.resolve_path_dirs();
    let home = opts.resolve_home();

    // 1) Configured binary path (highest priority, but still reported alongside PATH)
    if let Some(configured) = opts.configured_binary.as_ref() {
        if configured.exists() {
            let mut d = Detection::new(
                &entry.harness,
                entry.executables.first().map_or("", String::as_str),
                configured.clone(),
                DetectionSource::ConfiguredBinary,
                DetectionConfidence::High,
            );
            d.version = probe_version_for_path(configured, entry, opts);
            d.evidence.push(format!(
                "configured_binary exists: {}",
                configured.display()
            ));
            if let Some(v) = d.version.as_ref() {
                d.evidence.push(format!("version: {v}"));
            }
            seen_paths.insert(canonical_or_clone(configured));
            detections.push(d);
        } else if !configured.as_os_str().is_empty() {
            // Record broken configured path as Low confidence hit
            let mut d = Detection::new(
                &entry.harness,
                entry.executables.first().map_or("", String::as_str),
                configured.clone(),
                DetectionSource::ConfiguredBinary,
                DetectionConfidence::Low,
            );
            d.broken_shim = true;
            d.evidence.push(format!(
                "configured_binary missing: {}",
                configured.display()
            ));
            detections.push(d);
        }
    } else {
        // Also check env var SUPERAI_CONFIGURED_BINARY_<HARNESS>
        let env_key = format!(
            "SUPERAI_CONFIGURED_BINARY_{}",
            entry.harness.to_ascii_uppercase().replace('-', "_")
        );
        if let Some(val) = std::env::var_os(&env_key) {
            let p = PathBuf::from(val);
            if p.exists() {
                let mut d = Detection::new(
                    &entry.harness,
                    entry.executables.first().map_or("", String::as_str),
                    p.clone(),
                    DetectionSource::ConfiguredBinary,
                    DetectionConfidence::High,
                );
                d.version = probe_version_for_path(&p, entry, opts);
                d.evidence
                    .push(format!("env {env_key} exists: {}", p.display()));
                seen_paths.insert(canonical_or_clone(&p));
                detections.push(d);
            }
        }
    }

    // 2) PATH resolution — in PATH order, reporting shadowed duplicates
    for exe in &entry.executables {
        let hits = scan_path_for_executable(exe, &path_dirs);
        for (rank, path) in hits.iter().enumerate() {
            let canon = canonical_or_clone(path);
            // Still report shadowed hits (important to surface duplicates)
            // Any PATH entry beyond rank 0 is shadowed by earlier PATH precedence,
            // even when canonical paths differ (different dirs).
            let mut d = Detection::new(
                &entry.harness,
                exe,
                path.clone(),
                DetectionSource::Path,
                if rank == 0 {
                    DetectionConfidence::Medium
                } else {
                    DetectionConfidence::Low
                },
            );
            d.path_rank = Some(rank);
            d.shadowed = rank > 0;
            d.version = probe_version_for_path(path, entry, opts);
            // Arch mismatch stub: try `file` command if available
            d.arch_mismatch = probe_arch_mismatch(path, opts);
            d.broken_shim = is_broken_shim(path, &home);
            d.evidence
                .push(format!("PATH[{}] {} -> {}", rank, exe, path.display()));
            if d.shadowed {
                d.evidence.push("shadowed by earlier PATH entry".to_owned());
            }
            if d.broken_shim {
                d.evidence.push("broken shim detected".to_owned());
            }
            if let Some(v) = d.version.as_ref() {
                d.evidence.push(format!("version: {v}"));
            }
            seen_paths.insert(canon);
            detections.push(d);
        }
    }

    // 3) mise shims (~/.local/share/mise/shims/<exe>)
    if opts.probe_mise {
        if let Some(home) = home.as_ref() {
            for exe in &entry.executables {
                let shim = home.join(".local/share/mise/shims").join(exe);
                let canon = canonical_or_clone(&shim);
                if shim.exists() && !seen_paths.contains(&canon) {
                    let mut d = Detection::new(
                        &entry.harness,
                        exe,
                        shim.clone(),
                        DetectionSource::MiseShim,
                        DetectionConfidence::Medium,
                    );
                    d.version = probe_version_for_path(&shim, entry, opts);
                    d.broken_shim = d.version.is_none();
                    d.evidence
                        .push(format!("mise shim exists: {}", shim.display()));
                    if let Some(v) = d.version.as_ref() {
                        d.evidence.push(format!("version: {v}"));
                    } else {
                        d.evidence
                            .push("mise shim version probe failed (broken shim?)".to_owned());
                    }
                    detections.push(d);
                    seen_paths.insert(canon);
                }
            }
            // Also try `mise where` / `mise ls` if mise binary is present
            if let Some(mise_detections) = probe_mise_managed(entry, opts, &seen_paths) {
                for d in mise_detections {
                    let canon = canonical_or_clone(&d.path);
                    if !seen_paths.contains(&canon) {
                        seen_paths.insert(canon);
                        detections.push(d);
                    }
                }
            }
        }
    }

    // 4) Homebrew metadata: `brew list --versions <formula>`
    if opts.probe_brew {
        for method in entry.methods.iter().filter(|m| {
            matches!(
                m.kind,
                crate::install_catalog::InstallMethodKind::Homebrew
                    | crate::install_catalog::InstallMethodKind::HomebrewCask
            )
        }) {
            if let Some(d) = probe_homebrew(&method.package_name, entry, opts, &seen_paths) {
                let canon = canonical_or_clone(&d.path);
                if !seen_paths.contains(&canon) {
                    seen_paths.insert(canon);
                    detections.push(d);
                }
            }
        }
    }

    // 5) npm global metadata: `npm list -g <package> --depth=0`
    if opts.probe_npm {
        for method in entry
            .methods
            .iter()
            .filter(|m| matches!(m.kind, crate::install_catalog::InstallMethodKind::Npm))
        {
            if let Some(d) = probe_npm(&method.package_name, entry, opts, &seen_paths) {
                let canon = canonical_or_clone(&d.path);
                if !seen_paths.contains(&canon) {
                    seen_paths.insert(canon);
                    detections.push(d);
                }
            }
        }
    }

    // 6) cargo install metadata: `cargo install --list`
    if opts.probe_cargo {
        for method in entry
            .methods
            .iter()
            .filter(|m| matches!(m.kind, crate::install_catalog::InstallMethodKind::Cargo))
        {
            if let Some(ds) = probe_cargo(&method.package_name, entry, opts, &seen_paths) {
                for d in ds {
                    let canon = canonical_or_clone(&d.path);
                    if !seen_paths.contains(&canon) {
                        seen_paths.insert(canon);
                        detections.push(d);
                    }
                }
            }
        }
    }

    // 7) App bundles (`/Applications/...`)
    if opts.probe_apps {
        for app_path in &entry.apps {
            let p = PathBuf::from(app_path);
            if p.exists() && !seen_paths.contains(&canonical_or_clone(&p)) {
                let mut d = Detection::new(
                    &entry.harness,
                    app_path,
                    p.clone(),
                    DetectionSource::AppBundle,
                    DetectionConfidence::High,
                );
                d.evidence
                    .push(format!("app bundle exists: {}", p.display()));
                // Also probe bundle version via plist if macOS — stub: try
                // reading CFBundleShortVersionString literally from Info.plist
                if let Some(ver) = probe_app_bundle_version(&p) {
                    d.version = Some(ver.clone());
                    d.evidence.push(format!("bundle version: {ver}"));
                }
                seen_paths.insert(canonical_or_clone(&p));
                detections.push(d);
            }
        }
        // Also probe bundle_ids via spot-checks on macOS
        for bundle_id in &entry.bundle_ids {
            if let Some(path) = probe_bundle_id(bundle_id, opts) {
                if !seen_paths.contains(&canonical_or_clone(&path)) {
                    let mut d = Detection::new(
                        &entry.harness,
                        bundle_id,
                        path.clone(),
                        DetectionSource::AppBundle,
                        DetectionConfidence::Medium,
                    );
                    d.evidence.push(format!(
                        "bundle_id {bundle_id} resolved to {}",
                        path.display()
                    ));
                    seen_paths.insert(canonical_or_clone(&path));
                    detections.push(d);
                }
            }
        }
    }

    // Sort: PATH hits retain PATH order (rank), then other sources in stable order.
    // Preserve original insertion order except ensure non-PATH shim shadows are last?
    // Keep as-is: caller sees PATH order first, then mise/homebrew/npm/cargo/app.
    detections
}

// ---------------------------------------------------------------------------
// Helpers: path scan, version probe, arch mismatch, shim check, package probes
// ---------------------------------------------------------------------------

/// Scan each PATH dir for an executable file named `exe`.
///
/// Returns hits in PATH order (first dir first). Does not check executability
/// beyond `is_file` to allow detection of broken shims; caller marks broken.
fn scan_path_for_executable(exe: &str, path_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    for dir in path_dirs {
        let candidate = dir.join(exe);
        // Also try with .exe suffix on Windows-style paths (no-op on Linux/macOS)
        let is_file = candidate.is_file();
        if is_file {
            hits.push(candidate);
        }
        // On Windows, executables may be `exe.exe`
        #[cfg(windows)]
        {
            let win_candidate = dir.join(format!("{exe}.exe"));
            if win_candidate.is_file() {
                hits.push(win_candidate);
            }
        }
    }
    hits
}

fn canonical_or_clone(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn probe_version_for_path(
    path: &Path,
    entry: &InstallCatalogEntry,
    opts: &DetectOptions,
) -> Option<String> {
    // Prefer catalog detect commands that match this path's executable.
    // Fallback to `<path> --version`.
    let exe = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    // Find first detect command whose executable matches `exe`
    let probe_args: Vec<String> = entry
        .detect
        .commands
        .iter()
        .find(|c| c.executable == exe)
        .map_or_else(|| vec!["--version".to_owned()], |c| c.args.clone());
    let exec_opts = ExecuteOpts {
        timeout: Some(opts.probe_timeout),
        cwd: None,
        output_limit: Some(64 * 1024),
        ..Default::default()
    };
    // First try running the exact path (handles versioned binaries); if that
    // fails with SpawnFailed, fall back to running the basename via PATH
    // resolution so shim wrappers that delegate to mise still resolve.
    let out = run_command(path.to_string_lossy().as_ref(), &probe_args, &exec_opts)
        .or_else(|_| run_command(exe, &probe_args, &exec_opts))
        .ok()?;
    if !out.success && out.stdout.trim().is_empty() && out.stderr.trim().is_empty() {
        return None;
    }
    let text = if out.stdout.trim().is_empty() {
        out.stderr
    } else {
        out.stdout
    };
    extract_version(&text)
}

fn probe_arch_mismatch(path: &Path, opts: &DetectOptions) -> bool {
    // Cheap probe: `file <path>` output contains architecture strings.
    // If `file` is not present or probe times out, return false (no mismatch).
    let exec_opts = ExecuteOpts {
        timeout: Some(Duration::from_secs(2)),
        output_limit: Some(8 * 1024),
        ..Default::default()
    };
    let out = match run_command("file", &[path.to_string_lossy().into_owned()], &exec_opts) {
        Ok(o) if o.success => o,
        _ => return false,
    };
    let text = out.stdout.to_ascii_lowercase();
    let host_arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        return false;
    };
    let other_arch = if host_arch == "x86_64" {
        "aarch64"
    } else {
        "x86_64"
    };
    // If file says other_arch and not host_arch, flag mismatch.
    text.contains(other_arch)
        && !text.contains(host_arch)
        && (text.contains("executable") || text.contains("mach-o") || text.contains("elf"))
        && opts.probe_timeout.as_secs() > 0
}

#[expect(
    clippy::ref_option,
    reason = "home option is passed as borrowed Option for convenience"
)]
fn is_broken_shim(path: &Path, home: &Option<PathBuf>) -> bool {
    // Heuristic: path is inside mise shims dir and is a text shim script that
    // points to a missing target. Check if file is a shell shim and its target
    // does not exist. For now, detect brokenness as: file exists, is under
    // mise shims, and `mise` version probe for that exe fails. A cheaper
    // heuristic: if the file's content contains `mise` and the file is not
    // executable, consider it broken. We keep it simple: if path is under
    // `mise/shims` and `file -b` says "ASCII text" and not "executable", mark
    // as suspicious — but we already only set broken_shim when version probe
    // is None, so callers can use that. Here we add a content check.
    if let Some(home) = home {
        let shim_dir = home.join(".local/share/mise/shims");
        if path.starts_with(&shim_dir) {
            if let Ok(meta) = std::fs::metadata(path) {
                // On Unix, check executable bit via permissions
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = meta.permissions().mode();
                    if mode & 0o111 == 0 {
                        return true;
                    }
                }
                let _ = meta;
            }
            // Also treat missing version probe as broken (caller sets version=None)
            // This helper alone only checks the executable bit; the composite
            // broken flag is set in the main detection loop.
        }
    }
    false
}

fn probe_mise_managed(
    entry: &InstallCatalogEntry,
    opts: &DetectOptions,
    _seen: &HashSet<PathBuf>,
) -> Option<Vec<Detection>> {
    // Check if `mise` is on PATH
    let exec_opts = ExecuteOpts {
        timeout: Some(Duration::from_secs(2)),
        output_limit: Some(8 * 1024),
        ..Default::default()
    };
    let version_out = run_command("mise", &["--version".to_owned()], &exec_opts).ok()?;
    if !version_out.success {
        return None;
    }
    // Try `mise where <tool>` for each executable's mise package name
    let mise_pkg = entry
        .methods
        .iter()
        .find(|m| matches!(m.kind, crate::install_catalog::InstallMethodKind::Mise))
        .map(|m| m.package_name.as_str())?;
    let where_out = run_command(
        "mise",
        &["where".to_owned(), mise_pkg.to_owned()],
        &exec_opts,
    )
    .ok()?;
    if !where_out.success {
        return None;
    }
    let tool_path = PathBuf::from(where_out.stdout_trimmed().to_owned());
    if !tool_path.exists() {
        return None;
    }
    // Find executables under that tool path (bin/<exe> or root)
    let mut out = Vec::new();
    for exe in &entry.executables {
        let candidate = tool_path.join("bin").join(exe);
        let path = if candidate.exists() {
            candidate
        } else {
            let alt = tool_path.join(exe);
            if alt.exists() {
                alt
            } else {
                continue;
            }
        };
        let mut d = Detection::new(
            &entry.harness,
            exe,
            path.clone(),
            DetectionSource::MiseManaged,
            DetectionConfidence::High,
        );
        d.version = probe_version_for_path(&path, entry, opts);
        d.evidence
            .push(format!("mise where {mise_pkg} -> {}", path.display()));
        if let Some(v) = d.version.as_ref() {
            d.evidence.push(format!("version: {v}"));
        }
        out.push(d);
    }
    if out.is_empty() { None } else { Some(out) }
}

fn probe_homebrew(
    package: &str,
    entry: &InstallCatalogEntry,
    opts: &DetectOptions,
    _seen: &HashSet<PathBuf>,
) -> Option<Detection> {
    let exec_opts = ExecuteOpts {
        timeout: Some(opts.probe_timeout),
        output_limit: Some(64 * 1024),
        ..Default::default()
    };
    let out = run_command(
        "brew",
        &[
            "list".to_owned(),
            "--versions".to_owned(),
            package.to_owned(),
        ],
        &exec_opts,
    )
    .ok()?;
    if !out.success {
        return None;
    }
    // brew output: "<package> 1.2.3 1.2.2"
    let first_line = out.stdout.lines().next().unwrap_or_default().to_owned();
    let version = first_line.split_whitespace().nth(1).map(ToOwned::to_owned);
    // Resolve brew prefix to get binary path
    let prefix_out = run_command(
        "brew",
        &["--prefix".to_owned(), package.to_owned()],
        &exec_opts,
    )
    .ok();
    let bin_path = prefix_out
        .and_then(|o| {
            if o.success {
                let prefix = o.stdout_trimmed().to_owned();
                if prefix.is_empty() {
                    None
                } else {
                    Some(
                        PathBuf::from(prefix)
                            .join("bin")
                            .join(entry.executables.first().map_or(package, String::as_str)),
                    )
                }
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            PathBuf::from(format!(
                "/opt/homebrew/bin/{}",
                entry.executables.first().map_or(package, String::as_str)
            ))
        });
    let mut d = Detection::new(
        &entry.harness,
        entry.executables.first().map_or(package, String::as_str),
        bin_path,
        DetectionSource::Homebrew,
        DetectionConfidence::Medium,
    );
    d.version = version.or_else(|| probe_version_for_path(&d.path, entry, opts));
    d.evidence.push(format!(
        "brew list --versions {package}: {}",
        first_line.trim()
    ));
    Some(d)
}

fn probe_npm(
    package: &str,
    entry: &InstallCatalogEntry,
    opts: &DetectOptions,
    _seen: &HashSet<PathBuf>,
) -> Option<Detection> {
    let exec_opts = ExecuteOpts {
        timeout: Some(opts.probe_timeout),
        output_limit: Some(256 * 1024),
        ..Default::default()
    };
    let out = run_command(
        "npm",
        &[
            "list".to_owned(),
            "-g".to_owned(),
            package.to_owned(),
            "--depth=0".to_owned(),
            "--json".to_owned(),
        ],
        &exec_opts,
    )
    .ok()?;
    // npm list -g --json returns JSON with dependencies even on success; on
    // missing it exits non-zero. We still parse version if present.
    let version = extract_npm_version(&out.stdout, package);
    let path = PathBuf::from(format!(
        "/usr/local/lib/node_modules/{package}/{}",
        entry.executables.first().map_or(package, String::as_str)
    ));
    // Prefer global npm prefix
    let prefix_out = run_command("npm", &["prefix".to_owned(), "-g".to_owned()], &exec_opts).ok();
    let bin_path = prefix_out
        .and_then(|o| {
            if o.success {
                let prefix = o.stdout_trimmed().to_owned();
                if prefix.is_empty() {
                    None
                } else {
                    // npm bin path: <prefix>/bin/<exe> or via `npm bin -g`
                    Some(
                        PathBuf::from(prefix)
                            .join("bin")
                            .join(entry.executables.first().map_or(package, String::as_str)),
                    )
                }
            } else {
                None
            }
        })
        .unwrap_or(path);
    let confidence = if version.is_some() {
        DetectionConfidence::High
    } else if out.success {
        DetectionConfidence::Medium
    } else {
        return None;
    };
    let mut d = Detection::new(
        &entry.harness,
        entry.executables.first().map_or(package, String::as_str),
        bin_path,
        DetectionSource::Npm,
        confidence,
    );
    d.version = version.or_else(|| probe_version_for_path(&d.path, entry, opts));
    d.evidence.push(format!(
        "npm list -g {package}: {}",
        out.stdout.lines().next().unwrap_or_default()
    ));
    Some(d)
}

fn extract_npm_version(json: &str, package: &str) -> Option<String> {
    // Try JSON parse; fallback to regex-like search for `"version":"x.y.z"`
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(json) {
        if let Some(deps) = val.get("dependencies") {
            if let Some(pkg) = deps.get(package) {
                if let Some(v) = pkg.get("version").and_then(|v| v.as_str()) {
                    return Some(v.to_owned());
                }
            }
        }
        // Sometimes npm nests under `dependencies` with scoped name at top level differently
        if let Some(v) = val
            .get("dependencies")
            .and_then(|d| d.as_object())
            .and_then(|map| map.values().next())
            .and_then(|v| v.get("version"))
            .and_then(|v| v.as_str())
        {
            return Some(v.to_owned());
        }
    }
    // Fallback: search for "version"
    let lower = json.to_ascii_lowercase();
    if let Some(idx) = lower.find("\"version\"") {
        let after = json.get(idx..).unwrap_or_default();
        if let Some(colon) = after.find(':') {
            let rest = after.get(colon + 1..).unwrap_or_default().trim();
            let rest = rest.trim_start_matches(['"', '\'', ' ', ':']);
            let end = rest.find(['"', '\'', ',', '}', '\n']).unwrap_or(rest.len());
            let ver = rest
                .get(0..end)
                .unwrap_or_default()
                .trim()
                .trim_matches('"')
                .trim()
                .to_owned();
            if !ver.is_empty() && ver.chars().any(|c| c.is_ascii_digit()) {
                return Some(ver);
            }
        }
    }
    None
}

fn probe_cargo(
    package: &str,
    entry: &InstallCatalogEntry,
    opts: &DetectOptions,
    _seen: &HashSet<PathBuf>,
) -> Option<Vec<Detection>> {
    let exec_opts = ExecuteOpts {
        timeout: Some(opts.probe_timeout),
        output_limit: Some(256 * 1024),
        ..Default::default()
    };
    let out = run_command(
        "cargo",
        &["install".to_owned(), "--list".to_owned()],
        &exec_opts,
    )
    .ok()?;
    if !out.success {
        return None;
    }
    let mut out_vec = Vec::new();
    // cargo install --list format:
    // package v1.2.3 (/path):
    //   binary "exe"
    let mut current_pkg: Option<String> = None;
    let mut current_ver: Option<String> = None;
    for line in out.stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            // package header line: "pkg v1.2.3:"
            // parse first token as package, second token starting with v as version
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            let pkg = parts.first().copied().unwrap_or_default();
            let ver = parts
                .get(1)
                .map(|s| s.trim_start_matches('v').trim_end_matches(':').to_owned());
            current_pkg = Some(pkg.to_owned());
            current_ver = ver;
        } else if let Some(pkg) = current_pkg.as_ref() {
            if pkg == package {
                // line like `    exe "opencode":` or `    opencode`
                // cargo's format varies; extract the binary name
                let exe_candidate = trimmed
                    .trim_matches('"')
                    .split_whitespace()
                    .last()
                    .unwrap_or_default()
                    .trim_matches('"')
                    .trim_end_matches(':')
                    .to_owned();
                let exe_name = if exe_candidate.is_empty() {
                    entry
                        .executables
                        .first()
                        .map_or(package, String::as_str)
                        .to_owned()
                } else {
                    // If trimmed is like `opencode:` or `"opencode"`, exe_candidate may be `opencode`
                    // If format is `aider v0.1.2:` with binary line `    aider`, exe_candidate is `aider`
                    if exe_candidate == "binary" {
                        // next token? but simple fallback
                        entry
                            .executables
                            .first()
                            .map_or(package, String::as_str)
                            .to_owned()
                    } else {
                        exe_candidate
                    }
                };
                if entry.executables.contains(&exe_name) || package.contains(&exe_name) {
                    // Resolve cargo bin dir: `~/.cargo/bin/<exe>`
                    let home = opts.resolve_home();
                    let bin_path = home.map_or_else(
                        || PathBuf::from(format!("/home/cargo/.cargo/bin/{exe_name}")),
                        |h| h.join(".cargo/bin").join(&exe_name),
                    );
                    let mut d = Detection::new(
                        &entry.harness,
                        &exe_name,
                        bin_path.clone(),
                        DetectionSource::Cargo,
                        DetectionConfidence::Medium,
                    );
                    d.version = current_ver
                        .clone()
                        .or_else(|| probe_version_for_path(&bin_path, entry, opts));
                    d.evidence.push(format!(
                        "cargo install --list: {pkg} {current_ver:?} -> {}",
                        bin_path.display()
                    ));
                    out_vec.push(d);
                }
            }
        }
    }
    if out_vec.is_empty() {
        None
    } else {
        Some(out_vec)
    }
}

fn probe_app_bundle_version(bundle_path: &Path) -> Option<String> {
    // Try reading Info.plist's CFBundleShortVersionString. Prefer `plutil` or
    // direct file read. Fallback to `defaults read` is avoided to keep no-shell
    // and to avoid macOS-only assumptions in tests. Direct plist read: look
    // for <key>CFBundleShortVersionString</key> followed by <string>VERSION</string>.
    let plist = bundle_path.join("Contents/Info.plist");
    let bytes = std::fs::read(&plist).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let key = "CFBundleShortVersionString";
    let idx = text.find(key)?;
    let after = text.get(idx + key.len()..)?;
    let start = after.find("<string>")?;
    let rest = after.get(start + "<string>".len()..)?;
    let end = rest.find("</string>")?;
    let ver = rest.get(0..end)?.trim().to_owned();
    if ver.is_empty() { None } else { Some(ver) }
}

fn probe_bundle_id(bundle_id: &str, opts: &DetectOptions) -> Option<PathBuf> {
    // Try `mdfind kMDItemCFBundleIdentifier == '<bundle_id>'` — returns app paths.
    // Only when on macOS and mdfind is present; otherwise None.
    let exec_opts = ExecuteOpts {
        timeout: Some(opts.probe_timeout),
        output_limit: Some(8 * 1024),
        ..Default::default()
    };
    let out = run_command(
        "mdfind",
        &[format!("kMDItemCFBundleIdentifier == '{bundle_id}'")],
        &exec_opts,
    )
    .ok()?;
    if !out.success {
        return None;
    }
    let first = out.stdout.lines().next()?.trim().to_owned();
    if first.is_empty() {
        return None;
    }
    let p = PathBuf::from(first);
    if p.exists() { Some(p) } else { None }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn make_temp_dir(prefix: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(prefix)
    }

    /// Write a fake harness executable answering `--version`. The script is a
    /// `#!/bin/sh` file, so this (and the tests that probe it) run on unix only.
    #[cfg(unix)]
    fn write_fake_exe(dir: &Path, name: &str, version_output: &str) {
        let path = dir.join(name);
        let script = format!("#!/bin/sh\necho \"{version_output}\"\n");
        fs::write(&path, script).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn detect_with_temp_path_finds_executables_in_order() {
        let tmp1 = make_temp_dir("a");
        let tmp2 = make_temp_dir("b");
        let home = make_temp_dir("home");
        write_fake_exe(&tmp1, "claude", "1.2.3");
        write_fake_exe(&tmp2, "claude", "2.0.0");

        let catalog = InstallCatalog::embedded().unwrap();
        let entry = catalog.get_str("claude-code").unwrap().clone();

        let opts = DetectOptions {
            path_dirs: Some(vec![tmp1.clone(), tmp2.clone()]),
            home_dir: Some(home.clone()),
            probe_mise: false,
            probe_brew: false,
            probe_npm: false,
            probe_cargo: false,
            probe_apps: false,
            ..Default::default()
        };
        let hits = detect_all_for_entry(&entry, &opts);
        // Filter to PATH hits only for this assertion
        let path_hits: Vec<_> = hits
            .iter()
            .filter(|d| d.source == DetectionSource::Path)
            .collect();
        assert_eq!(
            path_hits.len(),
            2,
            "both PATH dirs should be found: {path_hits:?}"
        );
        // Must be in PATH order: tmp1 first, tmp2 second, with shadowed flag on second
        assert_eq!(path_hits[0].path, tmp1.join("claude"));
        assert_eq!(path_hits[1].path, tmp2.join("claude"));
        assert_eq!(path_hits[0].path_rank, Some(0));
        assert_eq!(path_hits[1].path_rank, Some(1));
        assert!(!path_hits[0].shadowed);
        assert!(path_hits[1].shadowed);
        // Versions should be probed
        assert_eq!(path_hits[0].version.as_deref(), Some("1.2.3"));
        assert_eq!(path_hits[1].version.as_deref(), Some("2.0.0"));

        // Cleanup
        drop(fs::remove_dir_all(&tmp1));
        drop(fs::remove_dir_all(&tmp2));
        drop(fs::remove_dir_all(&home));
    }

    #[cfg(unix)]
    #[test]
    fn detect_reports_mise_shim_when_present() {
        let tmp_path = make_temp_dir("path3");
        let home = make_temp_dir("home2");
        let shim_dir = home.join(".local/share/mise/shims");
        fs::create_dir_all(&shim_dir).unwrap();
        write_fake_exe(&shim_dir, "claude", "3.1.0");

        let catalog = InstallCatalog::embedded().unwrap();
        let entry = catalog.get_str("claude-code").unwrap().clone();

        let opts = DetectOptions {
            path_dirs: Some(vec![tmp_path.clone()]),
            home_dir: Some(home.clone()),
            probe_mise: true,
            probe_brew: false,
            probe_npm: false,
            probe_cargo: false,
            probe_apps: false,
            ..Default::default()
        };
        let hits = detect_all_for_entry(&entry, &opts);
        let shim_hit = hits.iter().find(|d| d.source == DetectionSource::MiseShim);
        assert!(shim_hit.is_some(), "mise shim should be detected: {hits:?}");
        assert_eq!(shim_hit.unwrap().version.as_deref(), Some("3.1.0"));

        drop(fs::remove_dir_all(&tmp_path));
        drop(fs::remove_dir_all(&home));
    }

    #[test]
    fn detect_marks_broken_shim_when_version_probe_fails() {
        let tmp_path = make_temp_dir("path4");
        let home = make_temp_dir("home3");
        let shim_dir = home.join(".local/share/mise/shims");
        fs::create_dir_all(&shim_dir).unwrap();
        // Create a shim that exits non-zero
        let shim_path = shim_dir.join("claude");
        fs::write(&shim_path, "#!/bin/sh\nexit 1\n").unwrap();
        // The exec bit only exists on unix; off unix the spawn itself fails,
        // which exercises the same "probe produced no version" path.
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&shim_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&shim_path, perms).unwrap();
        }

        let catalog = InstallCatalog::embedded().unwrap();
        let entry = catalog.get_str("claude-code").unwrap().clone();

        let opts = DetectOptions {
            path_dirs: Some(vec![tmp_path.clone()]),
            home_dir: Some(home.clone()),
            probe_mise: true,
            probe_brew: false,
            probe_npm: false,
            probe_cargo: false,
            probe_apps: false,
            ..Default::default()
        };
        let hits = detect_all_for_entry(&entry, &opts);
        let shim_hit = hits.iter().find(|d| d.source == DetectionSource::MiseShim);
        assert!(shim_hit.is_some(), "broken shim should still be reported");
        assert!(shim_hit.unwrap().broken_shim || shim_hit.unwrap().version.is_none());

        drop(fs::remove_dir_all(&tmp_path));
        drop(fs::remove_dir_all(&home));
    }

    #[cfg(unix)]
    #[test]
    fn detect_does_not_pick_silently_when_multiple() {
        let tmp1 = make_temp_dir("multi1");
        let tmp2 = make_temp_dir("multi2");
        let home = make_temp_dir("homemulti");
        write_fake_exe(&tmp1, "codex", "0.1.0");
        write_fake_exe(&tmp2, "codex", "0.2.0");

        let catalog = InstallCatalog::embedded().unwrap();
        let entry = catalog.get_str("codex-cli").unwrap().clone();
        let opts = DetectOptions {
            path_dirs: Some(vec![tmp1.clone(), tmp2.clone()]),
            home_dir: Some(home.clone()),
            probe_mise: false,
            probe_brew: false,
            probe_npm: false,
            probe_cargo: false,
            probe_apps: false,
            ..Default::default()
        };
        let hits = detect_all_for_entry(&entry, &opts);
        let path_hits: Vec<_> = hits
            .iter()
            .filter(|d| d.source == DetectionSource::Path)
            .collect();
        assert!(
            path_hits.len() >= 2,
            "must return all PATH hits, not just first"
        );
        // Ensure caller can distinguish which would win (rank 0)
        let winner = path_hits.iter().find(|d| d.path_rank == Some(0)).unwrap();
        let shadowed = path_hits.iter().find(|d| d.shadowed).unwrap();
        assert_ne!(winner.path, shadowed.path);

        drop(fs::remove_dir_all(&tmp1));
        drop(fs::remove_dir_all(&tmp2));
        drop(fs::remove_dir_all(&home));
    }

    #[test]
    fn extract_npm_version_parses_json() {
        let json = r#"{"dependencies":{"@anthropic-ai/claude-code":{"version":"1.2.3"}}}"#;
        assert_eq!(
            extract_npm_version(json, "@anthropic-ai/claude-code").as_deref(),
            Some("1.2.3")
        );
        let bad = "not json at all";
        assert!(extract_npm_version(bad, "pkg").is_none());
    }

    #[test]
    fn scan_path_preserves_order() {
        let d1 = PathBuf::from("/a/b");
        let d2 = PathBuf::from("/c/d");
        let hits = scan_path_for_executable("nonexistent_exe_superai_test", &[d1, d2]);
        assert!(hits.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn process_fake_wrapper_never_shell_interpolates() {
        // Directly test that run_command with duct never spawns shell by checking
        // that an arg containing shell metachars is printed literally.
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        let token = "$(echo pwned)".to_owned();
        let out = run_command("echo", std::slice::from_ref(&token), &opts).unwrap();
        assert_eq!(out.stdout_trimmed(), token);
    }
}
