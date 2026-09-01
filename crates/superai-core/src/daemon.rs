//! Daemon lifecycle machinery (WRP-07).
//!
//! Generic start/stop for daemon-class harnesses (`Isolation::DaemonService`,
//! e.g. openclaw and letta-code): port allocation is probe-and-reserve with a
//! fresh conflict check at start (a recorded port is never trusted as
//! unquestionably free), the pid/service identity lives in a superai-owned
//! file, readiness is a bounded poll of a command probe, and stop signals a
//! pid only after its start identity has been re-verified — a stale or reused
//! pid file never authorizes killing a process.
//!
//! The machinery is generic on purpose: which harness daemons superai may
//! actually drive is an adapter research question (openclaw stays
//! `ResearchBlocked` until its gateway/port facts are verified; see
//! `adapters::openclaw::daemon_constraints`).

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use superai_config::document::DocumentKind;
use superai_config::transaction::commit_file;

use crate::error::{CoreError, Result};
use crate::process::{ExecuteOpts, run_command};

/// Default bind address used when a harness does not pin one.
pub const DEFAULT_BIND_ADDR: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Default port range for allocation (the IANA ephemeral range).
pub const DEFAULT_PORT_RANGE_START: u16 = 49152;

/// Inclusive end of the default port range.
pub const DEFAULT_PORT_RANGE_END: u16 = 65535;

/// Upper bound on bind probes per allocation attempt (keeps worst-case
/// allocation bounded even over the full ephemeral range).
const MAX_PORT_PROBES: usize = 512;

// ---------------------------------------------------------------------------
// Process probing
// ---------------------------------------------------------------------------

/// Evidence source for the liveness and identity of an OS process.
///
/// Production uses [`SystemProcessProbe`]; tests inject deterministic fakes
/// (the same discipline as `failure::DaemonFixture`, which pinned the
/// unrelated-pid and pid-reuse classes).
pub trait ProcessProbe: Send + Sync {
    /// Whether `pid` names a live process.
    fn is_alive(&self, pid: u32) -> bool;

    /// Kernel start time of `pid` (Linux `/proc/<pid>/stat` field 22), the
    /// stable "process start identity" a recorded pid is re-verified against.
    fn start_time(&self, pid: u32) -> Option<u64>;

    /// Executable path of `pid`, when observable.
    fn executable(&self, pid: u32) -> Option<String>;
}

/// Production probe over the OS process table.
///
/// Linux reads `/proc` directly. On platforms without a std-visible process
/// table the probe reports no evidence (`start_time`/`executable` `None`) and
/// conservative liveness — which makes identity verification refuse rather
/// than guess, per the "never kill an unverified pid" rule. Signaling itself
/// goes through the platform `kill`/`taskkill` binary (no unsafe code).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProcessProbe;

impl ProcessProbe for SystemProcessProbe {
    fn is_alive(&self, pid: u32) -> bool {
        pid_is_alive(pid)
    }

    fn start_time(&self, pid: u32) -> Option<u64> {
        proc_start_time(pid)
    }

    fn executable(&self, pid: u32) -> Option<String> {
        proc_executable(pid)
    }
}

/// Whether `pid` names a live process.
///
/// Linux: `/proc/<pid>` existence, with a zombie (state `Z`) counted as dead
/// — an exited-but-unreaped daemon must not hold locks or ports. Other
/// platforms: conservatively `true` — liveness can never be disproven with
/// std alone, and a false "dead" would authorize removing another process's
/// identity state.
#[must_use]
pub fn pid_is_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        let proc_dir = Path::new("/proc").join(pid.to_string());
        if !proc_dir.exists() {
            return false;
        }
        match proc_state(pid) {
            Some(state) => state != 'Z',
            None => true,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

/// Process state character from `/proc/<pid>/stat` on Linux (e.g. `R`, `Z`).
#[must_use]
fn proc_state(pid: u32) -> Option<char> {
    #[cfg(target_os = "linux")]
    {
        let stat =
            std::fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat")).ok()?;
        let after_parens = stat.rsplit(')').next()?;
        after_parens
            .split_whitespace()
            .next()
            .and_then(|state| state.chars().next())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// Kernel start time of `pid` from `/proc/<pid>/stat` (field 22) on Linux;
/// `None` elsewhere or when the process is gone.
#[must_use]
pub fn proc_start_time(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let stat =
            std::fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat")).ok()?;
        // The comm field (2) may contain spaces and parens; fields after the
        // final ')' start at state (3). starttime is field 22 => index 19.
        let after_parens = stat.rsplit(')').next()?;
        after_parens.split_whitespace().nth(19)?.parse().ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// Executable path of `pid` from `/proc/<pid>/exe` on Linux; `None` elsewhere.
#[must_use]
pub fn proc_executable(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let raw = std::fs::read_link(Path::new("/proc").join(pid.to_string()).join("exe")).ok()?;
        let text = raw.to_string_lossy().into_owned();
        let trimmed = text.strip_suffix(" (deleted)").unwrap_or(&text);
        Some(trimmed.to_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

// ---------------------------------------------------------------------------
// Daemon identity (superai-owned)
// ---------------------------------------------------------------------------

/// Default superai-owned root for daemon identity files (`<home>/.superai/daemons`).
#[must_use]
pub fn default_identity_root(home: &Path) -> PathBuf {
    home.join(".superai").join("daemons")
}

/// Identity file path for a harness/instance daemon pair.
#[must_use]
pub fn identity_path(root: &Path, harness: &str, instance: &str) -> PathBuf {
    root.join(format!("{harness}-{instance}.identity.json"))
}

/// Recorded service identity of a daemon superai started (WRP-07).
///
/// Carries only safe facts: no argv and no environment are recorded, so a
/// leaked identity file never exposes a secret a daemon was launched with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonIdentity {
    /// Harness the daemon belongs to.
    pub harness: String,
    /// Instance name the daemon belongs to.
    pub instance: String,
    /// OS process id.
    pub pid: u32,
    /// Port the daemon was started on.
    pub port: u16,
    /// Bind address the port was probed on.
    pub bind_addr: String,
    /// Executable path as recorded at start.
    pub executable: String,
    /// Kernel start time of the pid at start, when the platform provides it —
    /// the token that detects pid reuse before any signal is sent.
    pub start_time: Option<u64>,
    /// ISO-8601 start timestamp.
    pub started_at: String,
    /// Random-enough token distinguishing restarts of the same pid range.
    pub identity_token: String,
}

/// Fresh-read one identity file.
///
/// Missing files and unparsable content are typed errors, never guessed past.
pub fn load_identity(path: &Path) -> Result<DaemonIdentity> {
    let bytes = std::fs::read(path).map_err(|e| CoreError::Validation {
        field: "daemon_identity".to_owned(),
        reason: format!("cannot read daemon identity at {}: {e}", path.display()),
    })?;
    serde_json::from_slice(&bytes).map_err(|e| CoreError::Validation {
        field: "daemon_identity".to_owned(),
        reason: format!("daemon identity at {} is malformed: {e}", path.display()),
    })
}

/// Fresh-scan a superai-owned daemon root for recorded identities.
///
/// Unparsable entries are skipped (they carry no authority: every consumer
/// re-verifies liveness and start identity before acting on a record).
#[must_use]
pub fn read_identities(root: &Path) -> Vec<(PathBuf, DaemonIdentity)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        if let Ok(id) = load_identity(&path) {
            out.push((path, id));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Port allocation
// ---------------------------------------------------------------------------

/// Whether a TCP port can be bound right now on `addr`.
///
/// The probe listener is dropped immediately; per WRP-07 any port derived
/// here is re-checked at start and never persisted as unquestionably free.
#[must_use]
pub fn port_is_free(addr: IpAddr, port: u16) -> bool {
    TcpListener::bind((addr, port)).is_ok()
}

/// Ports currently claimed by recorded daemon identities whose pid is alive.
fn held_ports(root: &Path, probe: &dyn ProcessProbe) -> HashSet<u16> {
    read_identities(root)
        .into_iter()
        .filter(|(_, id)| probe.is_alive(id.pid))
        .map(|(_, id)| id.port)
        .collect()
}

/// Verify a port is free now, naming the live recorded daemon that holds it
/// when one does. Produces the typed [`CoreError::PortConflict`] (INS-02's
/// daemon precondition and WRP-07's commit/start check both route here).
pub fn check_port_free(
    addr: IpAddr,
    port: u16,
    identity_root: &Path,
    probe: &dyn ProcessProbe,
) -> Result<()> {
    for (_, id) in read_identities(identity_root) {
        if id.port == port && probe.is_alive(id.pid) {
            return Err(CoreError::PortConflict {
                port,
                holder: Some(format!("{}/{} (pid {})", id.harness, id.instance, id.pid)),
                reason: "recorded superai daemon is live and holds this port".to_owned(),
            });
        }
    }
    if port_is_free(addr, port) {
        Ok(())
    } else {
        Err(CoreError::PortConflict {
            port,
            holder: None,
            reason: format!("bind probe on {addr} failed: address already in use"),
        })
    }
}

/// Allocate a free port from `range` by probing.
///
/// Ports held by live recorded daemons are skipped first; each remaining
/// candidate is bind-probed. Exhausting the probes yields the typed
/// [`CoreError::PortConflict`] — allocation never falls back to an unprobed
/// port.
pub fn allocate_port(
    addr: IpAddr,
    range: &RangeInclusive<u16>,
    identity_root: &Path,
    probe: &dyn ProcessProbe,
) -> Result<u16> {
    let held = held_ports(identity_root, probe);
    let mut probes = 0usize;
    for port in range.clone() {
        if probes >= MAX_PORT_PROBES {
            break;
        }
        probes += 1;
        if held.contains(&port) {
            continue;
        }
        if port_is_free(addr, port) {
            return Ok(port);
        }
    }
    Err(CoreError::PortConflict {
        port: *range.start(),
        holder: None,
        reason: format!(
            "no free port in range {}..={} after {probes} bind probes",
            range.start(),
            range.end()
        ),
    })
}

// ---------------------------------------------------------------------------
// Readiness
// ---------------------------------------------------------------------------

/// How a daemon announces readiness (WRP-07): a command probe, polled
/// bounded, executed through the process module (argv tokens, no shell).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadinessSpec {
    /// Poll `executable args` until it exits 0 or the timeout elapses.
    Command {
        /// Probe executable (argv token, never a shell string).
        executable: String,
        /// Probe argv; every `{port}` placeholder is the resolved port.
        args: Vec<String>,
        /// Extra environment for the probe.
        env: Vec<(String, String)>,
        /// Total wall-clock budget for readiness.
        timeout: Duration,
        /// Delay between probes.
        interval: Duration,
    },
}

impl ReadinessSpec {
    /// Materialize the probe against a resolved port (replaces `{port}`).
    #[must_use]
    fn for_port(&self, port: u16) -> Self {
        match self {
            Self::Command {
                executable,
                args,
                env,
                timeout,
                interval,
            } => Self::Command {
                executable: executable.clone(),
                args: args
                    .iter()
                    .map(|a| a.replace("{port}", &port.to_string()))
                    .collect(),
                env: env.clone(),
                timeout: *timeout,
                interval: *interval,
            },
        }
    }
}

/// Wait until the readiness probe succeeds, within its budget.
///
/// Timeout surfaces the typed [`CoreError::DaemonNotReady`] naming the probe.
pub fn wait_for_ready(harness: &str, spec: &ReadinessSpec, port: u16) -> Result<()> {
    let materialized = spec.for_port(port);
    let ReadinessSpec::Command {
        executable,
        args,
        env,
        timeout,
        interval,
    } = &materialized;
    let deadline = Instant::now() + *timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let attempt = remaining.min(Duration::from_secs(2));
        let opts = ExecuteOpts {
            timeout: Some(attempt),
            env: env.clone(),
            ..ExecuteOpts::default()
        };
        if let Ok(out) = run_command(executable, args, &opts)
            && out.success
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(CoreError::DaemonNotReady {
                harness: harness.to_owned(),
                reason: format!(
                    "readiness probe `{executable}` did not succeed within {} ms",
                    timeout.as_millis()
                ),
            });
        }
        thread::sleep(*interval);
    }
}

// ---------------------------------------------------------------------------
// Start / stop
// ---------------------------------------------------------------------------

/// Everything needed to start one daemon.
#[derive(Debug, Clone)]
pub struct DaemonStartConfig {
    /// Harness the daemon belongs to.
    pub harness: crate::ids::HarnessId,
    /// Instance name the daemon belongs to.
    pub instance: crate::ids::InstanceName,
    /// Daemon executable (argv token, never a shell string).
    pub executable: String,
    /// Daemon argv; a `{port}` placeholder receives the resolved port.
    pub args: Vec<String>,
    /// Extra environment for the daemon.
    pub env: Vec<(String, String)>,
    /// Working directory for the daemon.
    pub cwd: Option<PathBuf>,
    /// Bind address ports are probed on.
    pub bind_addr: IpAddr,
    /// Explicit port (conflict-checked now) or `None` to allocate.
    pub port: Option<u16>,
    /// Allocation range when `port` is `None`.
    pub port_range: RangeInclusive<u16>,
    /// Environment variable receiving the resolved port.
    pub port_env: Option<String>,
    /// Optional single argv token carrying the port (e.g. `--port={port}`).
    pub port_arg: Option<String>,
    /// Readiness probe.
    pub readiness: ReadinessSpec,
    /// Superai-owned root for the identity file.
    pub identity_root: PathBuf,
}

/// Handle to a started, ready daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonHandle {
    /// Path of the recorded identity file.
    pub identity_path: PathBuf,
    /// OS pid of the daemon.
    pub pid: u32,
    /// Port the daemon was started on.
    pub port: u16,
}

/// Stop tuning.
#[derive(Debug, Clone, Copy)]
pub struct StopOptions {
    /// Grace period between terminate and escalate.
    pub grace: Duration,
    /// Poll interval while waiting for exit.
    pub poll_interval: Duration,
}

impl Default for StopOptions {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(5),
            poll_interval: Duration::from_millis(50),
        }
    }
}

/// Result of a stop request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonStopOutcome {
    /// The daemon was signaled and confirmed dead; identity cleaned up.
    Stopped {
        /// Pid that was stopped.
        pid: u32,
        /// Port it held.
        port: u16,
    },
    /// The recorded pid was already dead; identity cleaned up, nothing killed.
    AlreadyStopped {
        /// Pid that was already gone.
        pid: u32,
        /// Port it had held.
        port: u16,
    },
}

fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86400).unwrap_or(0);
    let secs_of_day = secs % 86400;
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// Calendar conversion for the identity timestamp (days since 1970-01-01 to
/// y/m/d; civil-from-days algorithm).
fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 { y + 1 } else { y },
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

fn identity_token(pid: u32) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let mut hasher = DefaultHasher::new();
    millis.hash(&mut hasher);
    pid.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    format!("dmt-{:016x}", hasher.finish())
}

/// Start a daemon: resolve the port (conflict-checked), spawn detached via
/// duct with the port in env/args, record the superai-owned identity, and
/// wait bounded for readiness.
///
/// On readiness timeout the just-spawned process is killed (it is our own
/// child, killed by handle — never by pid), the identity file is removed, and
/// the typed [`CoreError::DaemonNotReady`] is returned.
pub fn start_daemon(config: &DaemonStartConfig, probe: &dyn ProcessProbe) -> Result<DaemonHandle> {
    if config.executable.is_empty() || config.executable.contains('\0') {
        return Err(CoreError::Validation {
            field: "executable".to_owned(),
            reason: "daemon executable must be a non-empty argv token".to_owned(),
        });
    }
    std::fs::create_dir_all(&config.identity_root).map_err(|e| CoreError::Validation {
        field: "identity_root".to_owned(),
        reason: format!(
            "cannot create daemon identity root {}: {e}",
            config.identity_root.display()
        ),
    })?;

    // One live daemon per harness/instance.
    let id_path = identity_path(
        &config.identity_root,
        config.harness.as_str(),
        config.instance.as_str(),
    );
    if id_path.exists()
        && let Ok(existing) = load_identity(&id_path)
        && probe.is_alive(existing.pid)
    {
        return Err(CoreError::Validation {
            field: "daemon".to_owned(),
            reason: format!(
                "daemon {}/{} already running as pid {} on port {}",
                config.harness, config.instance, existing.pid, existing.port
            ),
        });
    }

    let port = match config.port {
        Some(explicit) => {
            check_port_free(config.bind_addr, explicit, &config.identity_root, probe)?;
            explicit
        }
        None => allocate_port(
            config.bind_addr,
            &config.port_range,
            &config.identity_root,
            probe,
        )?,
    };

    let (handle, pid) = spawn_daemon_process(config, port)?;
    let identity = DaemonIdentity {
        harness: config.harness.to_string(),
        instance: config.instance.to_string(),
        pid,
        port,
        bind_addr: config.bind_addr.to_string(),
        executable: probe
            .executable(pid)
            .unwrap_or_else(|| config.executable.clone()),
        start_time: probe.start_time(pid),
        started_at: now_iso8601(),
        identity_token: identity_token(pid),
    };
    let id_bytes = serde_json::to_vec_pretty(&identity).map_err(|e| CoreError::Validation {
        field: "daemon_identity".to_owned(),
        reason: format!("cannot serialize daemon identity: {e}"),
    })?;
    // Plan-02 fold: the identity record persists through the config crate's
    // ONE mutation boundary (pretty JSON, staged parse-validation included).
    commit_file(
        "daemon-identity",
        &id_path,
        &id_bytes,
        DocumentKind::StrictJson,
    )
    .map_err(CoreError::Config)?;

    if let Err(e) = wait_for_ready(config.harness.as_str(), &config.readiness, port) {
        // Our own child: kill by handle, never by pid; wait reaps it so the
        // recorded pid is provably gone before we drop the record.
        drop(handle.kill());
        drop(handle.wait());
        drop(std::fs::remove_file(&id_path));
        return Err(e);
    }

    Ok(DaemonHandle {
        identity_path: id_path,
        pid,
        port,
    })
}

/// Spawn the daemon process detached (no capture, no shell), with the
/// resolved port substituted into args/env per the plan. Returns the duct
/// handle (for kill-by-handle on failure) and the pid.
fn spawn_daemon_process(config: &DaemonStartConfig, port: u16) -> Result<(duct::Handle, u32)> {
    let mut args: Vec<String> = config
        .args
        .iter()
        .map(|a| a.replace("{port}", &port.to_string()))
        .collect();
    if let Some(port_arg) = &config.port_arg {
        args.push(port_arg.replace("{port}", &port.to_string()));
    }
    let mut env = config.env.clone();
    if let Some(port_env) = &config.port_env {
        env.push((port_env.clone(), port.to_string()));
    }

    let mut cmd = duct::cmd(&config.executable, &args)
        .stdin_null()
        .stdout_null()
        .stderr_null();
    if let Some(cwd) = &config.cwd {
        cmd = cmd.dir(cwd);
    }
    for (k, v) in &env {
        cmd = cmd.env(k, v);
    }
    let handle = cmd
        .unchecked()
        .start()
        .map_err(|e| CoreError::BinaryDetection {
            binary: config.executable.clone(),
            reason: format!("failed to spawn daemon `{}`: {e}", config.executable),
        })?;
    let Some(pid) = handle.pids().first().copied() else {
        return Err(CoreError::BinaryDetection {
            binary: config.executable.clone(),
            reason: "spawned daemon reported no pid".to_owned(),
        });
    };
    Ok((handle, pid))
}

/// Signal a process via the platform binary (argv tokens, no shell, no
/// unsafe). `force` escalates to a hard kill.
fn send_signal(pid: u32, force: bool) -> Result<()> {
    #[cfg(unix)]
    let (binary, args) = (
        "kill",
        vec![
            (if force { "-KILL" } else { "-TERM" }).to_owned(),
            pid.to_string(),
        ],
    );
    #[cfg(windows)]
    let (binary, args) = (
        "taskkill",
        vec!["/PID".to_owned(), pid.to_string(), "/F".to_owned()],
    );
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, force);
        return Err(CoreError::UnsupportedOperation {
            harness: "*".to_owned(),
            operation: "signal_daemon".to_owned(),
            reason: "process signaling is not implemented on this platform".to_owned(),
        });
    }
    #[cfg(any(unix, windows))]
    {
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            ..ExecuteOpts::default()
        };
        // A non-zero exit (e.g. the process died between check and signal)
        // is not an error here; the exit poll below is the truth.
        run_command(binary, &args, &opts).map_err(|e| CoreError::BinaryDetection {
            binary: binary.to_owned(),
            reason: format!("cannot signal pid {pid}: {e}"),
        })?;
        Ok(())
    }
}

/// Re-verify a recorded identity against the live process table.
///
/// Requires matching process start times when both are known, falls back to
/// the executable when start times are unavailable, and refuses when no
/// platform evidence exists — a stale or reused pid file never authorizes
/// signaling the process that now owns the pid.
pub fn verify_process_identity(id: &DaemonIdentity, probe: &dyn ProcessProbe) -> Result<()> {
    if let (Some(expected), Some(observed)) = (id.start_time, probe.start_time(id.pid)) {
        if expected != observed {
            return Err(CoreError::ProcessIdentityMismatch {
                pid: id.pid,
                reason: format!(
                    "recorded start time {expected} but observed {observed} — pid reuse suspected"
                ),
            });
        }
        return Ok(());
    }
    if let Some(observed_exe) = probe.executable(id.pid) {
        if observed_exe != id.executable {
            return Err(CoreError::ProcessIdentityMismatch {
                pid: id.pid,
                reason: format!(
                    "recorded executable `{}` but observed `{observed_exe}`",
                    id.executable
                ),
            });
        }
        return Ok(());
    }
    Err(CoreError::ProcessIdentityMismatch {
        pid: id.pid,
        reason: "no platform identity evidence available; refusing to signal an unverified pid"
            .to_owned(),
    })
}

/// Stop a daemon recorded at `identity_path`.
///
/// Fresh-reads the identity (disk is truth), refuses when the pid cannot be
/// proven to still be the process superai started, signals TERM then (after
/// the grace period) escalates to a hard kill, and removes the identity file
/// only once the process is confirmed dead (or was already dead).
pub fn stop_daemon(
    identity_path: &Path,
    probe: &dyn ProcessProbe,
    opts: &StopOptions,
) -> Result<DaemonStopOutcome> {
    let id = load_identity(identity_path)?;
    if !probe.is_alive(id.pid) {
        drop(std::fs::remove_file(identity_path));
        return Ok(DaemonStopOutcome::AlreadyStopped {
            pid: id.pid,
            port: id.port,
        });
    }
    verify_process_identity(&id, probe)?;

    send_signal(id.pid, false)?;
    if wait_for_exit(id.pid, probe, opts.grace, opts.poll_interval) {
        drop(std::fs::remove_file(identity_path));
        return Ok(DaemonStopOutcome::Stopped {
            pid: id.pid,
            port: id.port,
        });
    }
    send_signal(id.pid, true)?;
    if wait_for_exit(id.pid, probe, opts.grace, opts.poll_interval) {
        drop(std::fs::remove_file(identity_path));
        return Ok(DaemonStopOutcome::Stopped {
            pid: id.pid,
            port: id.port,
        });
    }
    Err(CoreError::Commit {
        path: identity_path.to_path_buf(),
        reason: format!(
            "daemon pid {} (port {}) did not exit after terminate and kill",
            id.pid, id.port
        ),
    })
}

/// Poll for process death within `grace`.
fn wait_for_exit(pid: u32, probe: &dyn ProcessProbe, grace: Duration, interval: Duration) -> bool {
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if !probe.is_alive(pid) {
            return true;
        }
        thread::sleep(interval);
    }
    !probe.is_alive(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Debug, Clone, Default)]
    struct FakeProbe {
        alive: HashMap<u32, bool>,
        starts: HashMap<u32, u64>,
        exes: HashMap<u32, String>,
    }

    impl FakeProbe {
        fn set_alive(&mut self, pid: u32, alive: bool) {
            self.alive.insert(pid, alive);
        }
        fn set_start(&mut self, pid: u32, start: u64) {
            self.starts.insert(pid, start);
        }
        fn set_exe(&mut self, pid: u32, exe: &str) {
            self.exes.insert(pid, exe.to_owned());
        }
    }

    impl ProcessProbe for FakeProbe {
        fn is_alive(&self, pid: u32) -> bool {
            self.alive.get(&pid).copied().unwrap_or(false)
        }
        fn start_time(&self, pid: u32) -> Option<u64> {
            self.starts.get(&pid).copied()
        }
        fn executable(&self, pid: u32) -> Option<String> {
            self.exes.get(&pid).cloned()
        }
    }

    fn tmp_dir(prefix: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(prefix)
    }

    fn write_identity(root: &Path, harness: &str, instance: &str, id: &DaemonIdentity) -> PathBuf {
        let path = identity_path(root, harness, instance);
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(&path, serde_json::to_vec(id).unwrap()).unwrap();
        path
    }

    fn sample_identity(pid: u32, port: u16) -> DaemonIdentity {
        DaemonIdentity {
            harness: "openclaw".to_owned(),
            instance: "work".to_owned(),
            pid,
            port,
            bind_addr: "127.0.0.1".to_owned(),
            executable: "/usr/bin/openclaw".to_owned(),
            start_time: Some(111_111),
            started_at: "2026-09-01T00:00:00Z".to_owned(),
            identity_token: "dmt-0000000000000000".to_owned(),
        }
    }

    // ------------------------------------------------------------------
    // Port allocation / conflict
    // ------------------------------------------------------------------

    #[test]
    fn port_conflict_detected_and_refused_with_live_holder() {
        let dir = tmp_dir("daemon-port-holder");
        let mut probe = FakeProbe::default();
        probe.set_alive(4242, true);
        write_identity(&dir, "openclaw", "work", &sample_identity(4242, 51515));

        let err = check_port_free(DEFAULT_BIND_ADDR, 51515, &dir, &probe).unwrap_err();
        match err {
            CoreError::PortConflict { port, holder, .. } => {
                assert_eq!(port, 51515);
                let holder = holder.expect("holder must name the live daemon");
                assert!(holder.contains("openclaw/work"), "holder: {holder}");
                assert!(holder.contains("4242"), "holder: {holder}");
            }
            other => panic!("expected PortConflict, got {other:?}"),
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn port_conflict_detected_by_bind_probe_without_holder() {
        // Bind a real listener so the port is genuinely occupied.
        let listener = TcpListener::bind((DEFAULT_BIND_ADDR, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = tmp_dir("daemon-port-bind");

        let err = check_port_free(DEFAULT_BIND_ADDR, port, &dir, &SystemProcessProbe).unwrap_err();
        match err {
            CoreError::PortConflict { holder, .. } => assert!(holder.is_none()),
            other => panic!("expected PortConflict, got {other:?}"),
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn check_port_free_passes_when_free() {
        // A released ephemeral port can be grabbed by any concurrent process,
        // so try a handful of candidates and require one clean pass.
        let dir = tmp_dir("daemon-port-free");
        let mut passed = false;
        for _ in 0..5 {
            let listener = TcpListener::bind((DEFAULT_BIND_ADDR, 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            if check_port_free(DEFAULT_BIND_ADDR, port, &dir, &SystemProcessProbe).is_ok() {
                passed = true;
                break;
            }
        }
        assert!(
            passed,
            "at least one released ephemeral port must check free"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn allocate_port_skips_live_identity_ports_then_probes() {
        let dir = tmp_dir("daemon-alloc");
        let mut probe = FakeProbe::default();
        probe.set_alive(1, true);
        // Live identity claims one port in the range.
        write_identity(&dir, "letta-code", "srv", &sample_identity(1, 51001));

        let got = allocate_port(DEFAULT_BIND_ADDR, &(51001..=51001), &dir, &probe);
        match got {
            Err(CoreError::PortConflict { port, .. }) => assert_eq!(port, 51001),
            other => panic!("expected PortConflict, got {other:?}"),
        }

        // A wider range must return a bind-probed free port, skipping 51001
        // (the range is generous enough to survive unrelated listeners).
        let range = 51001..=51030;
        let free = allocate_port(DEFAULT_BIND_ADDR, &range, &dir, &probe).unwrap();
        assert_ne!(free, 51001, "held port must be skipped");
        assert!(range.contains(&free));
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn allocate_port_yields_portconflict_when_range_exhausted() {
        // Occupy every port in a small range with real listeners.
        let mut listeners = Vec::new();
        let first = TcpListener::bind((DEFAULT_BIND_ADDR, 0)).unwrap();
        let base = first.local_addr().unwrap().port();
        listeners.push(first);
        while listeners.len() < 3 {
            match TcpListener::bind((DEFAULT_BIND_ADDR, 0)) {
                Ok(l) => listeners.push(l),
                Err(_) => break,
            }
        }
        let end = base + u16::try_from(listeners.len() - 1).unwrap_or(0);
        // If the OS did not allocate sequentially the range may include free
        // ports; then skip the exhaustion assertion (still exercised by the
        // held-identity test above deterministically).
        let dir = tmp_dir("daemon-alloc-exhaust");
        let result = allocate_port(DEFAULT_BIND_ADDR, &(base..=end), &dir, &SystemProcessProbe);
        let held: HashSet<u16> = listeners
            .iter()
            .map(|l| l.local_addr().unwrap().port())
            .collect();
        let all_held = (base..=end).all(|p| held.contains(&p));
        if all_held {
            match result {
                Err(CoreError::PortConflict { .. }) => {}
                Ok(p) => panic!("expected PortConflict, got {p}"),
                other => panic!("expected PortConflict, got {other:?}"),
            }
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    // ------------------------------------------------------------------
    // Identity verification / stop refusals
    // ------------------------------------------------------------------

    #[test]
    fn stop_refuses_when_start_time_mismatched() {
        let dir = tmp_dir("daemon-stop-mismatch");
        let id = sample_identity(4242, 51515);
        let path = write_identity(&dir, "openclaw", "work", &id);
        let mut probe = FakeProbe::default();
        probe.set_alive(4242, true);
        probe.set_start(4242, 999_999); // reused pid

        let err = stop_daemon(&path, &probe, &StopOptions::default()).unwrap_err();
        match err {
            CoreError::ProcessIdentityMismatch { pid, reason } => {
                assert_eq!(pid, 4242);
                assert!(reason.contains("pid reuse"), "reason: {reason}");
            }
            other => panic!("expected ProcessIdentityMismatch, got {other:?}"),
        }
        // Identity file untouched on refusal.
        assert!(path.exists());
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn stop_refuses_without_platform_evidence() {
        let dir = tmp_dir("daemon-stop-noevidence");
        let id = sample_identity(4242, 51515);
        write_identity(&dir, "openclaw", "work", &id);
        let mut probe = FakeProbe::default();
        probe.set_alive(4242, true);
        // No start time, no executable observable.

        let err = verify_process_identity(&id, &probe).unwrap_err();
        match err {
            CoreError::ProcessIdentityMismatch { pid, reason } => {
                assert_eq!(pid, 4242);
                assert!(reason.contains("refusing"), "reason: {reason}");
            }
            other => panic!("expected ProcessIdentityMismatch, got {other:?}"),
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn verify_accepts_matching_start_time_and_exe_fallback() {
        let id = sample_identity(4242, 51515);
        let mut probe = FakeProbe::default();
        probe.set_alive(4242, true);
        probe.set_start(4242, 111_111);
        verify_process_identity(&id, &probe).unwrap();

        // Fallback: start time unknown, executable matches.
        let mut fallback = FakeProbe::default();
        fallback.set_alive(4242, true);
        fallback.set_exe(4242, "/usr/bin/openclaw");
        verify_process_identity(&id, &fallback).unwrap();

        // Fallback with a DIFFERENT executable refuses.
        let mut wrong_exe = FakeProbe::default();
        wrong_exe.set_alive(4242, true);
        wrong_exe.set_exe(4242, "/usr/bin/something-else");
        let err = verify_process_identity(&id, &wrong_exe).unwrap_err();
        assert!(matches!(err, CoreError::ProcessIdentityMismatch { .. }));
    }

    #[test]
    fn stop_reports_already_stopped_and_cleans_identity() {
        let dir = tmp_dir("daemon-stop-already");
        let id = sample_identity(4242, 51515);
        let path = write_identity(&dir, "openclaw", "work", &id);
        let probe = FakeProbe::default(); // 4242 not alive

        match stop_daemon(&path, &probe, &StopOptions::default()).unwrap() {
            DaemonStopOutcome::AlreadyStopped { pid, port } => {
                assert_eq!(pid, 4242);
                assert_eq!(port, 51515);
            }
            other @ DaemonStopOutcome::Stopped { .. } => {
                panic!("expected AlreadyStopped, got {other:?}")
            }
        }
        assert!(!path.exists(), "stale identity must be cleaned up");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn stop_missing_identity_is_typed_error() {
        let dir = tmp_dir("daemon-stop-missing");
        let err = stop_daemon(
            &dir.join("nope.identity.json"),
            &FakeProbe::default(),
            &StopOptions::default(),
        )
        .unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "daemon_identity"),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn read_identities_skips_unparsable_entries() {
        let dir = tmp_dir("daemon-read-ids");
        std::fs::create_dir_all(&dir).unwrap();
        write_identity(&dir, "openclaw", "work", &sample_identity(4242, 51515));
        std::fs::write(dir.join("broken-x.identity.json"), b"not json").unwrap();
        let ids = read_identities(&dir);
        assert_eq!(ids.len(), 1, "unparsable entry must be skipped");
        drop(std::fs::remove_dir_all(&dir));
    }

    // ------------------------------------------------------------------
    // System probe facts (linux)
    // ------------------------------------------------------------------

    #[cfg(target_os = "linux")]
    #[test]
    fn system_probe_sees_the_current_process_identity() {
        let pid = std::process::id();
        assert!(SystemProcessProbe.is_alive(pid));
        assert!(SystemProcessProbe.start_time(pid).is_some());
        let exe = SystemProcessProbe.executable(pid).expect("exe");
        assert!(!exe.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn zombie_process_counts_as_dead_for_locks_and_ports() {
        // Spawn a child this test never waits on: once it exits it lingers as
        // a zombie until reaped, and it must NOT hold locks or ports.
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .unwrap();
        let pid = child.id();
        let deadline = Instant::now() + Duration::from_secs(5);
        while SystemProcessProbe.is_alive(pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !SystemProcessProbe.is_alive(pid),
            "exited (zombie) child must read as not alive"
        );
        drop(child.wait());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn system_probe_reports_dead_pid_as_not_alive() {
        // Spawn a short-lived child and wait for it to exit.
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .status()
            .unwrap();
        assert!(status.success());
        // Pid 2^31-1 is beyond any allocated pid on default kernels.
        assert!(!SystemProcessProbe.is_alive(u32::MAX));
        assert!(SystemProcessProbe.start_time(u32::MAX).is_none());
    }

    // ------------------------------------------------------------------
    // Start/stop round trip with a real process (linux)
    // ------------------------------------------------------------------

    #[cfg(target_os = "linux")]
    fn sh_available() -> bool {
        Path::new("/bin/sh").exists()
    }

    #[cfg(target_os = "linux")]
    fn default_start_config(dir: &Path, ready_file: &Path, pid_file: &Path) -> DaemonStartConfig {
        DaemonStartConfig {
            harness: crate::ids::HarnessId::new("daemon-test").unwrap(),
            instance: crate::ids::InstanceName::new("t1").unwrap(),
            executable: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                format!(
                    "echo $$ > {} ; touch {} ; exec sleep 60",
                    pid_file.display(),
                    ready_file.display()
                ),
            ],
            env: Vec::new(),
            cwd: None,
            bind_addr: DEFAULT_BIND_ADDR,
            port: None,
            port_range: 49160..=49180,
            port_env: Some("SUPERAI_TEST_PORT".to_owned()),
            port_arg: None,
            readiness: ReadinessSpec::Command {
                executable: "test".to_owned(),
                args: vec!["-f".to_owned(), format!("{}", ready_file.display())],
                env: Vec::new(),
                // Generous budget: probe scheduling can lag under parallel
                // test load; readiness is still bounded.
                timeout: Duration::from_secs(15),
                interval: Duration::from_millis(50),
            },
            identity_root: dir.join("daemons"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn daemon_start_ready_stop_round_trip() {
        if !sh_available() {
            return;
        }
        let dir = tmp_dir("daemon-happy");
        let ready_file = dir.join("ready.flag");
        let pid_file = dir.join("daemon.pid");
        let config = default_start_config(&dir, &ready_file, &pid_file);
        let probe = SystemProcessProbe;

        let handle = start_daemon(&config, &probe).unwrap();
        assert!(ready_file.exists(), "daemon must be ready");
        let identity = load_identity(&handle.identity_path).unwrap();
        assert_eq!(identity.pid, handle.pid);
        assert_eq!(identity.port, handle.port);
        assert!(identity.start_time.is_some(), "start identity recorded");
        assert!(probe.is_alive(handle.pid));

        let outcome = stop_daemon(
            &handle.identity_path,
            &probe,
            &StopOptions {
                grace: Duration::from_secs(5),
                poll_interval: Duration::from_millis(50),
            },
        )
        .unwrap();
        match outcome {
            DaemonStopOutcome::Stopped { pid, port } => {
                assert_eq!(pid, handle.pid);
                assert_eq!(port, handle.port);
            }
            other @ DaemonStopOutcome::AlreadyStopped { .. } => {
                panic!("expected Stopped, got {other:?}")
            }
        }
        assert!(!handle.identity_path.exists(), "identity cleaned up");
        assert!(!probe.is_alive(handle.pid), "daemon is dead");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn daemon_readiness_timeout_surfaces_typed_error_and_cleans_up() {
        if !sh_available() {
            return;
        }
        let dir = tmp_dir("daemon-timeout");
        // Daemon that never becomes ready: it touches its own ready file but
        // the probe watches a different one that is never written.
        let ready_file = dir.join("ready.flag");
        let watched = dir.join("never-ready.flag");
        let pid_file = dir.join("daemon.pid");
        let mut config = default_start_config(&dir, &ready_file, &pid_file);
        config.readiness = ReadinessSpec::Command {
            executable: "test".to_owned(),
            args: vec!["-f".to_owned(), format!("{}", watched.display())],
            env: Vec::new(),
            timeout: Duration::from_millis(600),
            interval: Duration::from_millis(50),
        };
        let probe = SystemProcessProbe;

        let err = start_daemon(&config, &probe).unwrap_err();
        match err {
            CoreError::DaemonNotReady { harness, reason } => {
                assert_eq!(harness, "daemon-test");
                assert!(reason.contains("readiness"), "reason: {reason}");
            }
            other => panic!("expected DaemonNotReady, got {other:?}"),
        }
        // Cleanup: no identity record, spawned process killed by handle.
        let id_path = identity_path(
            &config.identity_root,
            config.harness.as_str(),
            config.instance.as_str(),
        );
        assert!(!id_path.exists(), "failed start must drop the identity");
        if let Ok(pid_text) = std::fs::read_to_string(&pid_file) {
            let pid: u32 = pid_text.trim().parse().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while probe.is_alive(pid) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(50));
            }
            assert!(!probe.is_alive(pid), "failed start must kill the child");
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn start_refuses_when_explicit_port_is_held() {
        if !sh_available() {
            return;
        }
        let dir = tmp_dir("daemon-start-port");
        let listener = TcpListener::bind((DEFAULT_BIND_ADDR, 0)).unwrap();
        let held_port = listener.local_addr().unwrap().port();
        let mut config = default_start_config(&dir, &dir.join("r.flag"), &dir.join("p.pid"));
        config.port = Some(held_port);
        let err = start_daemon(&config, &SystemProcessProbe).unwrap_err();
        match err {
            CoreError::PortConflict { port, .. } => assert_eq!(port, held_port),
            other => panic!("expected PortConflict, got {other:?}"),
        }
        assert!(
            !identity_path(
                &config.identity_root,
                config.harness.as_str(),
                config.instance.as_str()
            )
            .exists()
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    // ------------------------------------------------------------------
    // Misc
    // ------------------------------------------------------------------

    #[test]
    fn readiness_substitutes_port_placeholder() {
        let spec = ReadinessSpec::Command {
            executable: "curl".to_owned(),
            args: vec![
                "-sf".to_owned(),
                "http://127.0.0.1:{port}/health".to_owned(),
            ],
            env: Vec::new(),
            timeout: Duration::from_secs(1),
            interval: Duration::from_millis(10),
        };
        let ReadinessSpec::Command { args, .. } = spec.for_port(4242);
        assert_eq!(
            args.get(1).map(String::as_str),
            Some("http://127.0.0.1:4242/health")
        );
    }

    #[test]
    fn wait_for_ready_times_out_with_typed_error() {
        let spec = ReadinessSpec::Command {
            executable: "false".to_owned(),
            args: Vec::new(),
            env: Vec::new(),
            timeout: Duration::from_millis(200),
            interval: Duration::from_millis(50),
        };
        let err = wait_for_ready("openclaw", &spec, 1234).unwrap_err();
        match err {
            CoreError::DaemonNotReady { harness, reason } => {
                assert_eq!(harness, "openclaw");
                assert!(reason.contains("readiness"), "reason: {reason}");
            }
            other => panic!("expected DaemonNotReady, got {other:?}"),
        }
    }

    #[test]
    fn wait_for_ready_succeeds_on_success_command() {
        let spec = ReadinessSpec::Command {
            executable: "true".to_owned(),
            args: Vec::new(),
            env: Vec::new(),
            timeout: Duration::from_secs(2),
            interval: Duration::from_millis(20),
        };
        wait_for_ready("openclaw", &spec, 1234).unwrap();
    }

    #[test]
    fn identity_records_no_argv_or_env() {
        let id = sample_identity(4242, 51515);
        let json = serde_json::to_string(&id).unwrap();
        assert!(!json.contains("args"));
        assert!(!json.contains("env"));
    }
}
