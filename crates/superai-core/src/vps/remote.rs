//! The remote-exec port (VPS-02): argv-disciplined verbs over a host. Two
//! backends: `FixtureRemote` (in-memory tree, the in-repo proof) and
//! `TorideRemote` (typed pending refusals until toride 0.2 publishes).

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

/// Reason every `TorideRemote` verb refuses with; states the external gate.
pub(crate) const PENDING_TORIDE_02: &str = "pending toride-ssh 0.2 / toride-service 0.2 \
     (toride plan VPS-01; both were 0.1.0 on crates.io as of 2026-10-13)";

/// Typed backend failure; never carries command output bytes or secrets.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    /// The verb needs toride-ssh/toride-service 0.2, which is not published.
    #[error("remote verb `{verb}` unsupported: {reason}")]
    Pending {
        /// Verb that was refused.
        verb: &'static str,
        /// Why it cannot run yet.
        reason: String,
    },
    /// A remote command ran and exited nonzero; output bytes are not carried.
    #[error("remote command `{program}` failed (exit {exit:?})")]
    ExecFailed {
        /// Program that was invoked.
        program: String,
        /// Exit code, `None` when the command could not run at all.
        exit: Option<i32>,
    },
    /// The host could not be reached or answered.
    #[error("remote host unreachable: {reason}")]
    Unreachable {
        /// What failed; never contains remote output bytes.
        reason: String,
    },
    /// A remote path was refused before any mutation.
    #[error("remote path `{path}` refused: {reason}")]
    Path {
        /// The offending path.
        path: String,
        /// Why it was refused.
        reason: String,
    },
}

/// Result of one remote command: exit code plus captured stdout only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    /// Captured stdout, bounded by the backend.
    pub stdout: String,
    /// Exit code; `None` when the command could not run at all.
    pub exit: Option<i32>,
}

impl ExecOutput {
    /// Whether the command exited zero.
    pub fn success(&self) -> bool {
        self.exit == Some(0)
    }

    /// Build a successful output carrying `stdout`.
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            exit: Some(0),
        }
    }

    /// Build a failed output carrying no bytes.
    pub fn fail(exit: i32) -> Self {
        Self {
            stdout: String::new(),
            exit: Some(exit),
        }
    }
}

/// Host-key acceptance policy for the ssh backend (VPS-02 preflight).
/// An unverified host key is a typed error, never a warning (toride plan
/// VPS-01 item 4), so no "off" variant exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyPolicy {
    /// Only keys already in `known_hosts` connect.
    Strict,
    /// Unknown keys are accepted once and recorded.
    AcceptNew,
}

/// The remote verbs deploy and status need. Implementations keep argv
/// discipline (no shell strings, validated paths), back up before
/// overwriting a file the deployer did not create, and never let payload
/// bytes or credentials into their errors.
pub trait RemoteHost: std::fmt::Debug {
    /// Backend and target identity for journals; must not carry a secret.
    fn describe(&self) -> String;

    /// Run one argv on the host; no shell, no string assembly.
    fn exec(&self, argv: &[&str]) -> Result<ExecOutput, RemoteError>;

    /// Remote file bytes, `None` when absent.
    fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>, RemoteError>;

    /// Write `bytes` to `path` with mode `mode` (unix bits).
    fn upload(&self, path: &str, bytes: &[u8], mode: u32) -> Result<(), RemoteError>;

    /// Copy `path` aside if it exists; returns the backup path, `None` when
    /// there was nothing to back up.
    fn backup_file(&self, path: &str) -> Result<Option<String>, RemoteError>;

    /// Install a systemd unit atomically; returns the backup path when a
    /// pre-existing unit was backed up first.
    fn install_unit(&self, name: &str, content: &str) -> Result<Option<String>, RemoteError>;

    /// Remove a remote file this deploy created; refuses a file that is
    /// still present afterwards.
    fn remove_file(&self, path: &str) -> Result<(), RemoteError>;

    /// Mode bits at `path`, `None` when absent (exact rollback restores).
    fn file_mode(&self, path: &str) -> Result<Option<u32>, RemoteError>;

    /// Run `daemon-reload` on the remote init system.
    fn daemon_reload(&self) -> Result<(), RemoteError>;

    /// Whether `name` is currently active.
    fn service_is_active(&self, name: &str) -> Result<bool, RemoteError>;

    /// Enable `name` at boot.
    fn service_enable(&self, name: &str) -> Result<(), RemoteError>;

    /// Start `name` now.
    fn service_start(&self, name: &str) -> Result<(), RemoteError>;

    /// Restart `name` now.
    fn service_restart(&self, name: &str) -> Result<(), RemoteError>;

    /// Stop `name` (rollback of a start).
    fn service_stop(&self, name: &str) -> Result<(), RemoteError>;
}

/// Refuse a remote path before use: absolute, no traversal, no empty or
/// NUL-containing segments.
pub(crate) fn ensure_remote_path(path: &str) -> Result<(), RemoteError> {
    let refuse = |reason: String| RemoteError::Path {
        path: path.to_owned(),
        reason,
    };
    if !path.starts_with('/') {
        return Err(refuse("must be absolute".to_owned()));
    }
    if path.contains('\0') {
        return Err(refuse("must not contain NUL".to_owned()));
    }
    if path != "/" && path.split('/').skip(1).any(|s| s.is_empty() || s == "..") {
        return Err(refuse(
            "must not contain traversal or empty segments".to_owned(),
        ));
    }
    Ok(())
}

/// Refuse argv that could smuggle a shell or a NUL into the remote command.
pub(crate) fn ensure_exec_argv(argv: &[&str]) -> Result<(), RemoteError> {
    let Some((program, rest)) = argv.split_first() else {
        return Err(RemoteError::Unreachable {
            reason: "empty argv".to_owned(),
        });
    };
    if matches!(*program, "sh" | "bash") && rest.contains(&"-c") {
        return Err(RemoteError::Unreachable {
            reason: "argv must not invoke a shell via `-c`".to_owned(),
        });
    }
    for token in std::iter::once(*program).chain(rest.iter().copied()) {
        if token.contains('\0') {
            return Err(RemoteError::Unreachable {
                reason: format!("argv token contains NUL near `{program}`"),
            });
        }
        if let Some(pattern) = crate::process::shell_metachar_in(token) {
            return Err(RemoteError::Unreachable {
                reason: format!("argv token contains shell metacharacter `{pattern}`"),
            });
        }
    }
    Ok(())
}

/// The production backend: every verb refuses typed until toride 0.2 ships
/// exec/upload/unit-install (toride plan VPS-01). It cannot fake a remote
/// success because no verb has a success path.
#[derive(Debug, Clone)]
pub struct TorideRemote {
    target: String,
    policy: HostKeyPolicy,
}

impl TorideRemote {
    /// Bind to `user@host` under a host-key policy.
    pub fn new(target: &str, policy: HostKeyPolicy) -> Self {
        Self {
            target: target.to_owned(),
            policy,
        }
    }

    fn pending(&self, verb: &'static str) -> RemoteError {
        RemoteError::Pending {
            verb,
            reason: format!("{PENDING_TORIDE_02}; host-key policy {:?}", self.policy),
        }
    }
}

impl RemoteHost for TorideRemote {
    fn describe(&self) -> String {
        format!("toride (pending 0.2) via {}", self.target)
    }

    fn exec(&self, argv: &[&str]) -> Result<ExecOutput, RemoteError> {
        ensure_exec_argv(argv)?;
        Err(self.pending("exec"))
    }

    fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>, RemoteError> {
        ensure_remote_path(path)?;
        Err(self.pending("exec"))
    }

    fn upload(&self, path: &str, _bytes: &[u8], _mode: u32) -> Result<(), RemoteError> {
        ensure_remote_path(path)?;
        Err(self.pending("upload"))
    }

    fn backup_file(&self, path: &str) -> Result<Option<String>, RemoteError> {
        ensure_remote_path(path)?;
        Err(self.pending("upload"))
    }

    fn install_unit(&self, _name: &str, _content: &str) -> Result<Option<String>, RemoteError> {
        Err(self.pending("unit-install"))
    }

    fn remove_file(&self, path: &str) -> Result<(), RemoteError> {
        ensure_remote_path(path)?;
        Err(self.pending("upload"))
    }

    fn file_mode(&self, path: &str) -> Result<Option<u32>, RemoteError> {
        ensure_remote_path(path)?;
        Err(self.pending("exec"))
    }

    fn daemon_reload(&self) -> Result<(), RemoteError> {
        Err(self.pending("unit-install"))
    }

    fn service_is_active(&self, _name: &str) -> Result<bool, RemoteError> {
        Err(self.pending("exec"))
    }

    fn service_enable(&self, _name: &str) -> Result<(), RemoteError> {
        Err(self.pending("unit-install"))
    }

    fn service_start(&self, _name: &str) -> Result<(), RemoteError> {
        Err(self.pending("unit-install"))
    }

    fn service_restart(&self, _name: &str) -> Result<(), RemoteError> {
        Err(self.pending("unit-install"))
    }

    fn service_stop(&self, _name: &str) -> Result<(), RemoteError> {
        Err(self.pending("unit-install"))
    }
}

/// Facts the fixture host answers probes with; tests mutate them to force
/// preflight failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureHost {
    /// Contents served for `/etc/os-release`.
    pub os_release: String,
    /// Version line for `systemctl --version`; `None` means no systemd.
    pub systemd_version: Option<String>,
    /// Bytes available on `/` as `df -B1 /` reports.
    pub disk_free_bytes: u64,
    /// Contents served for `/proc/loadavg`.
    pub loadavg: String,
}

impl Default for FixtureHost {
    fn default() -> Self {
        Self {
            os_release: "ID=debian\nVERSION_ID=\"12\"\n".to_owned(),
            systemd_version: Some("systemd 255".to_owned()),
            disk_free_bytes: 10 * 1024 * 1024 * 1024,
            loadavg: "0.30 0.25 0.20 1/900 4321".to_owned(),
        }
    }
}

/// Expiry the fixture's `certbot` writes into its certificate PEM.
pub(crate) const FIXTURE_CERT_NOT_AFTER: &str = "May 21 12:00:00 2027 GMT";

/// A fault a test schedules on the next matching verb, to prove rollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// Fail the next `exec` of this program.
    Exec(String),
    /// Fail the next `upload`.
    Upload,
    /// Fail the next `install_unit`.
    UnitInstall,
    /// Fail the next `daemon_reload`.
    Reload,
    /// Fail the next `service_enable`.
    Enable,
    /// Fail the next `service_start`.
    Start,
    /// Fail the next `service_restart`.
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceFlags {
    Inactive,
    Enabled,
    Active,
}

#[derive(Debug, Clone, Default)]
struct FixtureState {
    host: Option<FixtureHost>,
    files: BTreeMap<String, (Vec<u8>, u32)>,
    backups: BTreeMap<String, String>,
    unit_backups: usize,
    reloads: usize,
    services: BTreeMap<String, ServiceFlags>,
    faults: VecDeque<Fault>,
}

/// In-memory remote tree with scripted probe answers. It interprets exactly
/// the argv the plan emits and refuses anything else, so a new remote command
/// surfaces as a test failure instead of passing vacuously.
#[derive(Debug, Clone, Default)]
pub struct FixtureRemote {
    target: String,
    state: RefCell<FixtureState>,
}

impl FixtureRemote {
    /// A Debian/systemd fixture host named `target`.
    pub fn new(target: &str) -> Self {
        Self {
            target: target.to_owned(),
            state: RefCell::new(FixtureState {
                host: Some(FixtureHost::default()),
                ..FixtureState::default()
            }),
        }
    }

    /// Replace the probe-answer facts.
    pub fn set_host(&self, host: FixtureHost) {
        self.state.borrow_mut().host = Some(host);
    }

    /// Seed a pre-existing remote file (foreign content for backup tests).
    pub fn insert_file(&self, path: &str, bytes: &[u8], mode: u32) {
        self.state
            .borrow_mut()
            .files
            .insert(path.to_owned(), (bytes.to_vec(), mode));
    }

    /// Schedule a fault for the next matching verb.
    pub fn fail_next(&self, fault: Fault) {
        self.state.borrow_mut().faults.push_back(fault);
    }

    /// Current bytes at `path`, if any (test assertions).
    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.state
            .borrow()
            .files
            .get(path)
            .map(|(bytes, _)| bytes.clone())
    }

    /// Mode at `path`, if any (test assertions).
    pub fn file_mode(&self, path: &str) -> Option<u32> {
        self.state.borrow().files.get(path).map(|(_, mode)| *mode)
    }

    /// Backup paths recorded so far, original path -> backup path.
    pub fn backups(&self) -> BTreeMap<String, String> {
        self.state.borrow().backups.clone()
    }

    /// How many `daemon-reload` calls ran.
    pub fn reload_count(&self) -> usize {
        self.state.borrow().reloads
    }

    /// Whether an install backed up a pre-existing unit (backup-first proof).
    pub fn unit_backup_count(&self) -> usize {
        self.state.borrow().unit_backups
    }

    /// Whether `name` is active (test assertions).
    pub fn service_active(&self, name: &str) -> bool {
        self.state
            .borrow()
            .services
            .get(name)
            .is_some_and(|flags| *flags == ServiceFlags::Active)
    }

    fn host(&self) -> FixtureHost {
        self.state.borrow().host.clone().unwrap_or_default()
    }

    fn pop_fault(cell: &RefCell<FixtureState>, matches: impl Fn(&Fault) -> bool) -> bool {
        let mut state = cell.borrow_mut();
        match state.faults.iter().position(matches) {
            Some(index) => {
                drop(state.faults.remove(index));
                true
            }
            None => false,
        }
    }

    fn backup_locked(state: &mut FixtureState, path: &str) -> Option<String> {
        let (bytes, mode) = state.files.get(path).cloned()?;
        let mut n = 1;
        while state.files.contains_key(&format!("{path}.superai-bak-{n}")) {
            n += 1;
        }
        let backup = format!("{path}.superai-bak-{n}");
        state.files.insert(backup.clone(), (bytes, mode));
        state.backups.insert(path.to_owned(), backup.clone());
        Some(backup)
    }
}

impl RemoteHost for FixtureRemote {
    fn describe(&self) -> String {
        format!("fixture {}", self.target)
    }

    fn exec(&self, argv: &[&str]) -> Result<ExecOutput, RemoteError> {
        ensure_exec_argv(argv)?;
        let program = argv.first().ok_or_else(|| RemoteError::Unreachable {
            reason: "empty argv".to_owned(),
        })?;
        if Self::pop_fault(
            &self.state,
            |fault| matches!(fault, Fault::Exec(fail_program) if fail_program == program),
        ) {
            return Err(RemoteError::Unreachable {
                reason: "injected fixture fault".to_owned(),
            });
        }
        let value_after = |flag: &str| -> Option<String> {
            argv.iter()
                .position(|token| *token == flag)
                .and_then(|index| argv.get(index + 1).copied())
                .map(|token: &str| token.to_owned())
        };
        match *program {
            "systemctl" if argv.get(1) == Some(&"--version") => {
                return match self.host().systemd_version {
                    Some(version) => Ok(ExecOutput::ok(version)),
                    None => Ok(ExecOutput::fail(1)),
                };
            }
            "df" if argv.get(1) == Some(&"-B1") => {
                let free = self.host().disk_free_bytes;
                return Ok(ExecOutput::ok(format!(
                    "Filesystem 1-blocks Used Available Use% Mounted on\n\
                     /dev/vda1 {} {} {} 50% /\n",
                    free.saturating_mul(2),
                    free,
                    free
                )));
            }
            "cat" if argv.get(1) == Some(&"/proc/loadavg") => {
                return Ok(ExecOutput::ok(self.host().loadavg));
            }
            "cat" if argv.get(1) == Some(&"/etc/os-release") => {
                return Ok(ExecOutput::ok(self.host().os_release));
            }
            "openssl" if argv.get(1) == Some(&"x509") => {
                let Some(path) = value_after("-in") else {
                    return Ok(ExecOutput::fail(1));
                };
                let not_after = self
                    .state
                    .borrow()
                    .files
                    .get(&path)
                    .and_then(|(bytes, _)| pem_not_after(bytes));
                return match not_after {
                    Some(not_after) => Ok(ExecOutput::ok(format!("enddate={not_after}"))),
                    None => Ok(ExecOutput::fail(1)),
                };
            }
            "certbot" => {
                let domain = value_after("-d").unwrap_or_else(|| "fixture".to_owned());
                let cert_dir = format!("/etc/letsencrypt/live/{domain}");
                let pem = format!(
                    "-----BEGIN CERTIFICATE-----\nnotAfter={FIXTURE_CERT_NOT_AFTER}\n\
                     -----END CERTIFICATE-----\n"
                );
                let mut state = self.state.borrow_mut();
                state.files.insert(
                    format!("{cert_dir}/fullchain.pem"),
                    (pem.into_bytes(), 0o644),
                );
                state.files.insert(
                    format!("{cert_dir}/privkey.pem"),
                    (b"fixture-key\n".to_vec(), 0o600),
                );
                return Ok(ExecOutput::ok(""));
            }
            "nginx" if argv.get(1) == Some(&"-t") => {
                return Ok(ExecOutput::ok("syntax is ok"));
            }
            "systemctl" if argv.get(1) == Some(&"reload") => {
                return Ok(ExecOutput::ok(""));
            }
            "tailscale" => {
                return Ok(ExecOutput::ok(""));
            }
            _ => {}
        }
        Err(RemoteError::Unreachable {
            reason: format!("fixture host has no handler for `{program}`"),
        })
    }

    fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>, RemoteError> {
        ensure_remote_path(path)?;
        Ok(self.state.borrow().files.get(path).map(|(b, _)| b.clone()))
    }

    fn upload(&self, path: &str, bytes: &[u8], mode: u32) -> Result<(), RemoteError> {
        ensure_remote_path(path)?;
        if Self::pop_fault(&self.state, |fault| matches!(fault, Fault::Upload)) {
            return Err(RemoteError::Unreachable {
                reason: "injected fixture fault".to_owned(),
            });
        }
        self.state
            .borrow_mut()
            .files
            .insert(path.to_owned(), (bytes.to_vec(), mode));
        Ok(())
    }

    fn backup_file(&self, path: &str) -> Result<Option<String>, RemoteError> {
        ensure_remote_path(path)?;
        let mut state = self.state.borrow_mut();
        Ok(Self::backup_locked(&mut state, path))
    }
    fn install_unit(&self, name: &str, content: &str) -> Result<Option<String>, RemoteError> {
        let path = unit_path(name)?;
        if Self::pop_fault(&self.state, |fault| matches!(fault, Fault::UnitInstall)) {
            return Err(RemoteError::Unreachable {
                reason: "injected fixture fault".to_owned(),
            });
        }
        let mut state = self.state.borrow_mut();
        let backup = Self::backup_locked(&mut state, &path);
        if backup.is_some() {
            state.unit_backups += 1;
        }
        state
            .files
            .insert(path, (content.as_bytes().to_vec(), 0o644));
        Ok(backup)
    }

    fn remove_file(&self, path: &str) -> Result<(), RemoteError> {
        ensure_remote_path(path)?;
        if Self::pop_fault(&self.state, |fault| matches!(fault, Fault::Upload)) {
            return Err(RemoteError::Unreachable {
                reason: "injected fixture fault".to_owned(),
            });
        }
        let removed = self.state.borrow_mut().files.remove(path);
        if removed.is_none() {
            return Err(RemoteError::Path {
                path: path.to_owned(),
                reason: "nothing to remove".to_owned(),
            });
        }
        Ok(())
    }

    fn file_mode(&self, path: &str) -> Result<Option<u32>, RemoteError> {
        ensure_remote_path(path)?;
        Ok(self.state.borrow().files.get(path).map(|(_, mode)| *mode))
    }

    fn daemon_reload(&self) -> Result<(), RemoteError> {
        if Self::pop_fault(&self.state, |fault| matches!(fault, Fault::Reload)) {
            return Err(RemoteError::Unreachable {
                reason: "injected fixture fault".to_owned(),
            });
        }
        self.state.borrow_mut().reloads += 1;
        Ok(())
    }

    fn service_is_active(&self, name: &str) -> Result<bool, RemoteError> {
        Ok(self
            .state
            .borrow()
            .services
            .get(name)
            .is_some_and(|flags| *flags == ServiceFlags::Active))
    }

    fn service_enable(&self, name: &str) -> Result<(), RemoteError> {
        if Self::pop_fault(&self.state, |fault| matches!(fault, Fault::Enable)) {
            return Err(RemoteError::Unreachable {
                reason: "injected fixture fault".to_owned(),
            });
        }
        let mut state = self.state.borrow_mut();
        let flags = state
            .services
            .entry(name.to_owned())
            .or_insert(ServiceFlags::Inactive);
        if *flags == ServiceFlags::Inactive {
            *flags = ServiceFlags::Enabled;
        }
        Ok(())
    }

    fn service_start(&self, name: &str) -> Result<(), RemoteError> {
        if Self::pop_fault(&self.state, |fault| matches!(fault, Fault::Start)) {
            return Err(RemoteError::Unreachable {
                reason: "injected fixture fault".to_owned(),
            });
        }
        self.state
            .borrow_mut()
            .services
            .insert(name.to_owned(), ServiceFlags::Active);
        Ok(())
    }

    fn service_restart(&self, name: &str) -> Result<(), RemoteError> {
        if Self::pop_fault(&self.state, |fault| matches!(fault, Fault::Restart)) {
            return Err(RemoteError::Unreachable {
                reason: "injected fixture fault".to_owned(),
            });
        }
        self.state
            .borrow_mut()
            .services
            .insert(name.to_owned(), ServiceFlags::Active);
        Ok(())
    }

    fn service_stop(&self, name: &str) -> Result<(), RemoteError> {
        let mut state = self.state.borrow_mut();
        if let Some(flags) = state.services.get_mut(name)
            && *flags == ServiceFlags::Active
        {
            *flags = ServiceFlags::Enabled;
        }
        Ok(())
    }
}

/// The `notAfter=` marker line the fixture's certificate PEMs carry.
fn pem_not_after(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("notAfter="))
        .map(str::to_owned)
}

/// Validate a unit name and return its remote unit-file path.
pub(crate) fn unit_path(name: &str) -> Result<String, RemoteError> {
    let valid = !name.is_empty()
        && name.len() <= 200
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'));
    if !valid {
        return Err(RemoteError::Path {
            path: name.to_owned(),
            reason: "invalid unit name".to_owned(),
        });
    }
    Ok(format!("/etc/systemd/system/{name}.service"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths_refuse_traversal_relative_and_nul() {
        assert!(
            ensure_remote_path("/etc/llmproxy/llmproxy.env").is_ok(),
            "valid path"
        );
        for bad in [
            "etc/llmproxy",
            "/etc/../etc/passwd",
            "/etc//llmproxy",
            "/etc/\0llmproxy",
            "/etc/llmproxy/",
        ] {
            assert!(ensure_remote_path(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn exec_argv_refuse_metachars_and_nul() {
        assert!(
            ensure_exec_argv(&["systemctl", "--version"]).is_ok(),
            "probe argv"
        );
        for bad in [
            vec!["sh", "-c", "true"],
            vec!["echo", "a;b"],
            vec!["echo", "a|b"],
            vec!["echo\u{0}"],
            vec![],
        ] {
            assert!(ensure_exec_argv(&bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn toride_backend_refuses_every_verb_typed() {
        let remote = TorideRemote::new("root@203.0.113.10", HostKeyPolicy::Strict);
        assert!(remote.describe().contains("root@203.0.113.10"));
        let pending = |verb: &'static str, result: Result<ExecOutput, RemoteError>| match result {
            Err(RemoteError::Pending { verb: got, reason }) => {
                assert_eq!(got, verb);
                assert!(reason.contains("toride"), "{reason}");
            }
            other => panic!("verb `{verb}` must refuse typed, got {other:?}"),
        };
        pending("exec", remote.exec(&["systemctl", "--version"]));
        pending(
            "exec",
            remote
                .read_file("/etc/hostname")
                .map(|_| ExecOutput::ok("")),
        );
        pending(
            "upload",
            remote
                .upload("/etc/llmproxy/llmproxy.env", b"x", 0o600)
                .map(|()| ExecOutput::ok("")),
        );
        pending(
            "unit-install",
            remote
                .install_unit("llmproxy", "[Unit]")
                .map(|_| ExecOutput::ok("")),
        );
        pending(
            "upload",
            remote
                .remove_file("/etc/llmproxy/llmproxy.env")
                .map(|()| ExecOutput::ok("")),
        );
        pending(
            "exec",
            remote
                .file_mode("/etc/llmproxy/llmproxy.env")
                .map(|_| ExecOutput::ok("")),
        );
        pending(
            "unit-install",
            remote.daemon_reload().map(|()| ExecOutput::ok("")),
        );
        pending(
            "exec",
            remote
                .service_is_active("llmproxy")
                .map(|_| ExecOutput::ok("")),
        );
        assert!(
            remote.exec(&["echo", "a; rm -rf /"]).is_err(),
            "metachars refused"
        );
        assert!(remote.upload("/etc/../x", b"y", 0o600).is_err());
    }

    #[test]
    fn fixture_answers_the_probe_argv_and_refuses_the_rest() {
        let remote = FixtureRemote::new("root@host");
        assert!(remote.exec(&["systemctl", "--version"]).unwrap().success());
        assert!(remote.exec(&["df", "-B1", "/"]).unwrap().success());
        assert!(
            remote
                .exec(&["cat", "/proc/loadavg"])
                .unwrap()
                .stdout
                .contains("0.30")
        );
        assert!(
            remote.exec(&["curl", "http://example.com"]).is_err(),
            "unknown program refused"
        );
        let no_systemd = FixtureRemote::new("root@host");
        no_systemd.set_host(FixtureHost {
            systemd_version: None,
            ..FixtureHost::default()
        });
        assert!(
            !no_systemd
                .exec(&["systemctl", "--version"])
                .unwrap()
                .success()
        );
    }

    #[test]
    fn fixture_backup_upload_and_unit_discipline() {
        let remote = FixtureRemote::new("root@host");
        remote.insert_file("/etc/llmproxy/llmproxy.env", b"old\n", 0o600);
        let backup = remote
            .backup_file("/etc/llmproxy/llmproxy.env")
            .unwrap()
            .unwrap_or_default();
        assert_eq!(
            remote.file(&backup).unwrap(),
            b"old\n".to_vec(),
            "backup preserves the original bytes"
        );
        remote
            .upload("/etc/llmproxy/llmproxy.env", b"new\n", 0o600)
            .unwrap();
        assert_eq!(remote.file_mode("/etc/llmproxy/llmproxy.env"), Some(0o600));
        assert_eq!(remote.file("/etc/llmproxy/../x"), None);

        remote.insert_file("/etc/systemd/system/llmproxy.service", b"[Unit]\n", 0o644);
        let unit_backup = remote.install_unit("llmproxy", "[Unit]\nnew\n").unwrap();
        assert!(
            unit_backup.is_some(),
            "install over a foreign unit returns its backup path"
        );
        assert_eq!(remote.unit_backup_count(), 1, "unit install backs up first");
        assert_eq!(
            remote.file("/etc/systemd/system/llmproxy.service").unwrap(),
            b"[Unit]\nnew\n".to_vec()
        );
        assert!(
            remote.install_unit("evil/name", "x").is_err(),
            "bad unit name refused"
        );
        assert!(remote.install_unit("fresh", "[Unit]\n").unwrap().is_none());
        remote.daemon_reload().unwrap();
        assert_eq!(remote.reload_count(), 1);

        assert!(!remote.service_is_active("llmproxy").unwrap());
        remote.service_enable("llmproxy").unwrap();
        remote.service_start("llmproxy").unwrap();
        assert!(remote.service_is_active("llmproxy").unwrap());
        remote.service_stop("llmproxy").unwrap();
        assert!(!remote.service_is_active("llmproxy").unwrap());

        remote.upload("/etc/x", b"y", 0o644).unwrap();
        assert_eq!(remote.file_mode("/etc/x"), Some(0o644));
        remote.remove_file("/etc/x").unwrap();
        assert_eq!(remote.file_mode("/etc/x"), None);
        assert!(remote.remove_file("/etc/x").is_err());
    }

    #[test]
    fn fixture_certbot_then_openssl_round_trip() {
        let remote = FixtureRemote::new("root@host");
        remote
            .exec(&[
                "certbot",
                "certonly",
                "--standalone",
                "-d",
                "proxy.example.com",
                "-m",
                "ops@example.com",
                "--agree-tos",
                "--non-interactive",
            ])
            .unwrap();
        let enddate = remote
            .exec(&[
                "openssl",
                "x509",
                "-in",
                "/etc/letsencrypt/live/proxy.example.com/fullchain.pem",
                "-noout",
                "-enddate",
            ])
            .unwrap();
        assert!(
            enddate.stdout.contains(FIXTURE_CERT_NOT_AFTER),
            "{}",
            enddate.stdout
        );
    }

    #[test]
    fn fixture_faults_fire_on_the_matching_verb() {
        let remote = FixtureRemote::new("root@host");
        remote.fail_next(Fault::Upload);
        assert!(remote.upload("/etc/x", b"y", 0o644).is_err());
        remote.upload("/etc/x", b"y", 0o644).unwrap();

        remote.fail_next(Fault::Exec("nginx".to_owned()));
        assert!(
            remote.exec(&["nginx", "-t"]).is_err(),
            "scheduled fault fires"
        );
        assert!(remote.exec(&["nginx", "-t"]).is_ok(), "fault fires once");
    }
}
