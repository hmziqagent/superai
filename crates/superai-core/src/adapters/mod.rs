//! Harness adapters: concrete implementations.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::adapter::DetectionConfidence;
use crate::instance::Instance;
use crate::state::{AdapterSupport, InstallPresence};

pub mod agentty;
pub mod aider;
pub mod amazon_q;
pub mod amp;
pub mod antigravity;
pub mod auggie;
pub mod avante_nvim;
pub mod bb_app;
pub mod chatgpt_desktop;
pub mod claude_code;
pub mod claude_desktop;
pub mod claw_code;
pub mod cline;
pub mod coco;
pub mod codecompanion_nvim;
pub mod codegeex;
pub mod codex_cli;
pub mod command_code;
pub mod conductor;
pub mod continue_dev;
pub mod copilot_cli;
pub mod copilot_coding_agent;
pub mod copilot_vscode;
pub mod crush;
pub mod cursor;
pub mod cursor_agent;
pub mod deepseek;
pub mod devin;
pub mod eca;
pub mod every_code;
pub mod factory_droid;
pub mod forge;
pub mod freebuff;
pub mod fx;
pub mod gemini_cli;
pub mod gitclaw;
pub mod goose;
pub mod gptme;
pub mod grok_build;
pub mod hax;
pub mod hermes;
pub mod iflow;
pub mod jcode;
pub mod jules;
pub mod junie;
pub mod kilo;
pub mod kilocode_cli;
pub mod kimi_code;
pub mod kiro;
pub mod kode;
pub mod legacy_kimi;
pub mod letta;
pub mod mimo;
pub mod minimax_code;
pub mod mistral_vibe;
pub mod muse_code;
pub mod nanocoder;
pub mod oh_my_codex;
pub mod oh_my_opencode;
pub mod omo_ai;
pub mod omp;
pub mod openclaw;
pub mod opencode;
pub mod openfang;
pub mod openhands;
pub mod orca;
pub mod ori;
pub mod pi;
pub mod plandex;
pub mod prime_agent;
pub mod qoder_cli;
pub mod qoder_ide;
pub mod qwen_code;
pub mod reasonix;
pub mod replit_agent;
pub mod roo_code;
pub mod sculptor;
pub mod swe_agent;
pub mod t3code;
pub mod terminus;
pub mod trae_agent;
pub mod unreal_agent;
pub mod vibe_kanban;
pub mod vix;
pub mod warp;
pub mod windsurf;
pub mod workbuddy;
pub mod zaly;
pub mod zcode;
pub mod zed_acp;
pub mod zulu_cli;

/// First PATH hit for `names`, name-major: an earlier name wins over an
/// earlier directory.
pub(crate) fn find_in_path(names: &[&str]) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    find_in_path_within(&path_var, names)
}

/// [`find_in_path`] against an explicit PATH value, for callers that must
/// pin exactly what gets resolved before spawning.
pub(crate) fn find_in_path_within(path_var: &std::ffi::OsStr, names: &[&str]) -> Option<PathBuf> {
    let path_var = path_var.to_string_lossy();
    let separator = if cfg!(windows) { ';' } else { ':' };
    for name in names {
        for dir in path_var.split(separator) {
            if let Some(hit) = probe_path_dir(dir, name) {
                return Some(hit);
            }
        }
    }
    None
}

/// First PATH hit for `names`, dir-major: the earliest directory holding
/// any name wins; a non-UTF8 `PATH` yields `None`, unlike lossy
/// [`find_in_path`] — per-adapter contract, not duplication.
pub(crate) fn find_in_path_dir_first(names: &[&str]) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    find_in_path_dir_first_within(&path_var, names)
}

/// [`find_in_path_dir_first`] against an explicit PATH value; the caller owns
/// the UTF-8 gate.
pub(crate) fn find_in_path_dir_first_within(path_var: &str, names: &[&str]) -> Option<PathBuf> {
    let separator = if cfg!(windows) { ';' } else { ':' };
    for dir in path_var.split(separator) {
        for name in names {
            if let Some(hit) = probe_path_dir(dir, name) {
                return Some(hit);
            }
        }
    }
    None
}

fn probe_path_dir(dir: &str, name: &str) -> Option<PathBuf> {
    if dir.is_empty() {
        return None;
    }
    let candidate = Path::new(dir).join(name);
    if candidate.is_file() {
        return Some(candidate);
    }
    if cfg!(windows) {
        let exe_candidate = Path::new(dir).join(format!("{name}.exe"));
        if exe_candidate.is_file() {
            return Some(exe_candidate);
        }
    }
    None
}

/// Parse the first version-shaped token (`1.2.3`, `v1.2`, `1.0.0-rc1`);
/// `name/1.2.3` slash compounds are split so harness-prefixed output parses.
#[expect(clippy::excessive_nesting, reason = "token fallback chain is explicit")]
pub(crate) fn parse_version_output(output: &str) -> Option<String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }
    for token in trimmed.split_whitespace() {
        for segment in token.split('/') {
            let mut candidate = segment;
            if let Some(stripped) = candidate.strip_prefix('v') {
                candidate = stripped;
            } else if let Some(stripped) = candidate.strip_prefix('V') {
                candidate = stripped;
            }
            let cleaned = candidate.trim_matches(|c: char| c == ',' || c == ')' || c == '(');
            if cleaned.is_empty() {
                continue;
            }
            let has_dot = cleaned.contains('.');
            let starts_digit = cleaned.chars().next().is_some_and(|c| c.is_ascii_digit());
            if has_dot && starts_digit {
                let is_version_like = cleaned
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+');
                if is_version_like {
                    return Some(cleaned.to_owned());
                }
                let mut version_part = String::new();
                for ch in cleaned.chars() {
                    if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '+' {
                        version_part.push(ch);
                    } else {
                        break;
                    }
                }
                if version_part.contains('.') && !version_part.is_empty() {
                    return Some(version_part);
                }
            }
        }
    }
    None
}

/// Run `<binary> args...` under `budget` and return the combined stdout and
/// stderr; a child still running at the budget is killed and reaped, and a
/// pipe held by an inherited grandchild is abandoned at the same bound.
pub(crate) fn run_capturing(binary: &Path, args: &[&str], budget: Duration) -> Option<String> {
    let owned = binary.to_path_buf();
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let (tx, rx) = mpsc::channel();
    // The send Result is the closure's value: a timed-out caller has dropped
    // the receiver, and that failure is expected, not an error to log.
    thread::spawn(move || tx.send(run_probe(&owned, &args, budget)));
    // A timeout here never leaves the child running: the worker kills it at
    // the same deadline and itself exits within the reclaim grace after it.
    rx.recv_timeout(budget).unwrap_or_default()
}

const PIPE_RECLAIM_GRACE: Duration = Duration::from_millis(500);

const MAX_PROBE_OUTPUT_BYTES: u64 = 1024 * 1024;

fn drain_pipe(pipe: Option<impl Read>, buf: &mut Vec<u8>, binary: &Path, side: &str) {
    let Some(mut pipe) = pipe else {
        return;
    };
    if let Err(err) = (&mut pipe).take(MAX_PROBE_OUTPUT_BYTES).read_to_end(buf) {
        eprintln!(
            "superai-core: probe of {} lost {side} output: {err}",
            binary.display()
        );
        return;
    }
    let mut scratch = [0u8; 8192];
    loop {
        match pipe.read(&mut scratch) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                eprintln!(
                    "superai-core: probe of {} lost {side} tail past the cap: {err}",
                    binary.display()
                );
                break;
            }
        }
    }
}

fn drain_pipe_concurrently(
    pipe: Option<impl Read + Send + 'static>,
    binary: &Path,
    side: &'static str,
    tx: mpsc::Sender<Vec<u8>>,
) {
    let binary = binary.to_path_buf();
    // The send Result is the thread's value: the worker departs at the
    // reclaim deadline, and a send into a departed worker is expected.
    thread::spawn(move || {
        let mut buf = Vec::new();
        drain_pipe(pipe, &mut buf, &binary, side);
        tx.send(buf)
    });
}

fn reclaim_pipe(
    rx: &mpsc::Receiver<Vec<u8>>,
    by: Instant,
    binary: &Path,
    side: &'static str,
) -> Option<Vec<u8>> {
    match rx.recv_timeout(by.saturating_duration_since(Instant::now())) {
        Ok(buf) => Some(buf),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            eprintln!(
                "superai-core: probe of {} {side} pipe held past the reclaim deadline",
                binary.display()
            );
            None
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            eprintln!(
                "superai-core: probe of {} {side} reader failed before delivering",
                binary.display()
            );
            None
        }
    }
}

fn run_probe(binary: &Path, args: &[String], budget: Duration) -> Option<String> {
    let spawned = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child: Child = match spawned {
        Ok(child) => child,
        Err(err) => {
            eprintln!(
                "superai-core: probe spawn failed for {}: {err}",
                binary.display()
            );
            return None;
        }
    };
    let deadline = Instant::now() + budget;
    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    drain_pipe_concurrently(child.stdout.take(), binary, "stdout", out_tx);
    drain_pipe_concurrently(child.stderr.take(), binary, "stderr", err_tx);
    let mut poll = Duration::from_micros(500);
    let status: ExitStatus = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                if let Err(err) = child.kill() {
                    eprintln!(
                        "superai-core: killing timed-out probe of {}: {err}",
                        binary.display()
                    );
                }
                break match child.wait() {
                    Ok(status) => status,
                    Err(err) => {
                        eprintln!(
                            "superai-core: reaping killed probe of {}: {err}",
                            binary.display()
                        );
                        return None;
                    }
                };
            }
            Ok(None) => {
                thread::sleep(poll);
                poll = (poll * 2).min(Duration::from_millis(10));
            }
            Err(err) => {
                eprintln!(
                    "superai-core: probe of {} wait failed: {err}",
                    binary.display()
                );
                return None;
            }
        }
    };
    let reclaim_by = Instant::now() + PIPE_RECLAIM_GRACE;
    let out = reclaim_pipe(&out_rx, reclaim_by, binary, "stdout");
    let err = reclaim_pipe(&err_rx, reclaim_by, binary, "stderr");
    let (Some(out), Some(err)) = (out, err) else {
        return None;
    };
    if !status.success() && out.is_empty() && err.is_empty() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out);
    let stderr = String::from_utf8_lossy(&err);
    Some(if stdout.trim().is_empty() {
        stderr.into_owned()
    } else if stderr.trim().is_empty() {
        stdout.into_owned()
    } else {
        format!("{stdout} {stderr}")
    })
}

/// Parse a version from `<binary>` argv runs: `primary` first, then `fallback`
/// under a second `budget` when that run or its parse fails; empty = single-shot.
pub(crate) fn probe_version_with_fallback(
    binary: &Path,
    primary: &[&str],
    fallback: &[&str],
    budget: Duration,
) -> Option<String> {
    let probe = |argv: &[&str]| {
        run_capturing(binary, argv, budget).and_then(|out| parse_version_output(&out))
    };
    match probe(primary) {
        Some(version) => Some(version),
        None if fallback.is_empty() => None,
        None => probe(fallback),
    }
}

/// Run `<binary> --version` under a 2s budget and parse the version from the
/// combined output.
pub(crate) fn probe_version(binary: &Path) -> Option<String> {
    probe_version_with_fallback(binary, &["--version"], &[], Duration::from_secs(2))
}

/// The one mirror-exclusion matcher, shared by the adapters' exclusion tests:
/// exact, `/*` prefix, `*.` suffix, and single-star infix glob patterns.
#[cfg(test)]
pub(crate) fn exclusion_matches(patterns: &[String], file: &str) -> bool {
    patterns.iter().any(|pat| {
        if let Some(prefix) = pat.strip_suffix("/*") {
            file.starts_with(prefix)
        } else if let Some(suffix) = pat.strip_prefix("*.") {
            file.ends_with(suffix)
        } else if pat.contains('*') {
            match pat.split('*').collect::<Vec<_>>()[..] {
                [head, tail] => file.starts_with(head) && file.ends_with(tail),
                _ => file == pat,
            }
        } else {
            file == pat
        }
    })
}

/// HOME, falling back to USERPROFILE on Windows; None when neither is usable.
pub(crate) fn home_dir() -> Option<PathBuf> {
    for var in ["HOME", "USERPROFILE"] {
        if let Ok(home) = std::env::var(var)
            && !home.trim().is_empty()
        {
            return Some(PathBuf::from(home));
        }
    }
    None
}

/// The desktop triple every catalog harness supports today.
pub(crate) fn desktop_platforms() -> Vec<crate::adapter::Platform> {
    use crate::adapter::{Arch, Os, Platform};
    vec![
        Platform::new(Os::Linux, Arch::Any),
        Platform::new(Os::Macos, Arch::Any),
        Platform::new(Os::Windows, Arch::Any),
    ]
}

/// The 11-operation row, every op at `support`, shared by adapters whose
/// whole surface sits at one support level.
pub(crate) fn operations_at(support: AdapterSupport) -> Vec<(String, AdapterSupport)> {
    [
        "detect",
        "read_config",
        "write_config",
        "manage_skills",
        "manage_mcp",
        "manage_plugins",
        "configure_provider",
        "plan_mirror",
        "plan_wrapper",
        "scan_candidates",
        "validate_instance",
    ]
    .into_iter()
    .map(|op| (op.to_owned(), support))
    .collect()
}

/// The 11-operation row, every op [`AdapterSupport::Full`], shared by
/// adapters that support the whole catalog surface.
pub(crate) fn all_operations_full() -> Vec<(String, AdapterSupport)> {
    operations_at(AdapterSupport::Full)
}

/// Skill modes, link-first: `relink_skills` takes the first mode.
pub(crate) fn skill_modes_link_first() -> Vec<crate::adapter::SkillMode> {
    use crate::adapter::SkillMode;
    vec![
        SkillMode::LinkAll,
        SkillMode::LinkSelected,
        SkillMode::CopySelected,
    ]
}

/// Probe outcome for a found-or-missing binary and a parsed-or-missing version.
pub(crate) fn install_presence(binary_found: bool, version_found: bool) -> InstallPresence {
    match (binary_found, version_found) {
        (true, true) => InstallPresence::Present,
        (true, false) => InstallPresence::UnknownVersion,
        (false, _) => InstallPresence::Absent,
    }
}

/// Shared detection rule: a missing binary is High unless leftover config
/// evidence marks the harness uninstalled (Low); unparsable probe is Medium.
pub(crate) fn detection_confidence(
    binary_found: bool,
    version_found: bool,
    absent_config_seen: bool,
) -> DetectionConfidence {
    if !binary_found {
        return if absent_config_seen {
            DetectionConfidence::Low
        } else {
            DetectionConfidence::High
        };
    }
    if version_found {
        DetectionConfidence::High
    } else {
        DetectionConfidence::Medium
    }
}

/// Map a probe outcome onto the resolution, naming `harness` in the notes.
pub(crate) fn resolution_from_detection(
    detection: &crate::adapter::DetectionResult,
    harness: &str,
    schema_version: &str,
) -> crate::adapter::VersionResolution {
    let Some(version) = detection.version.clone() else {
        let mut res = crate::adapter::VersionResolution::unknown();
        res.notes.clone_from(&detection.evidence);
        return res;
    };
    let notes = vec![
        format!("detected {harness} version {version}"),
        format!("mapped to schema version {schema_version}"),
    ];
    let mut res = crate::adapter::VersionResolution::new(
        Some(version),
        Some(schema_version.to_owned()),
        true,
    );
    res.notes = notes;
    res
}

/// Refuse instances whose harness is not `id`; the guard every adapter's
/// `plan_wrapper` and `validate_instance` opens with.
pub(crate) fn ensure_instance_harness(
    id: &crate::ids::HarnessId,
    instance: &Instance,
) -> Result<(), crate::error::CoreError> {
    if instance.harness != *id {
        return Err(crate::error::CoreError::Validation {
            field: "harness".to_owned(),
            reason: format!(
                "instance harness `{}` does not match adapter `{id}`",
                instance.harness
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod decl_tests {
    //! Every catalog adapter declares exactly one of an MCP destination or a
    //! corpus-grounded absence (likewise plugins); writable dests round-trip.

    use crate::adapter::DocumentKind;
    use crate::harness_catalog;

    /// The shared version parser's contract, tested once here instead of
    /// once per adapter file.
    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("conductor 1.2.3", Some("1.2.3")),
            ("1.0.0", Some("1.0.0")),
            ("v1.0.0", Some("1.0.0")),
            ("Version: 2.0.0", Some("2.0.0")),
            ("vibe-kanban/0.1.44 linux-x64 node-v22.23.2", Some("0.1.44")),
            ("linux-x64 node-v22.23.2", None),
            ("tool 0.1.0-beta", Some("0.1.0-beta")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            assert_eq!(
                crate::adapters::parse_version_output(input).as_deref(),
                expected,
                "input: {input:?}"
            );
        }
    }

    /// One canonical check for every adapter's owned selectors, replacing the
    /// per-file stability copies: unique and non-empty within each surface.
    #[test]
    fn owned_selectors_unique_and_non_empty_per_surface() {
        for adapter in harness_catalog::all_adapters() {
            let id = adapter.id().as_str().to_owned();
            for surface in adapter.config_surfaces() {
                let set: std::collections::HashSet<&str> =
                    surface.owned_selectors.iter().map(String::as_str).collect();
                assert_eq!(
                    set.len(),
                    surface.owned_selectors.len(),
                    "{id}/{}: duplicate owned selectors",
                    surface.id
                );
                assert!(
                    surface.owned_selectors.iter().all(|s| !s.is_empty()),
                    "{id}/{}: empty owned selector",
                    surface.id
                );
            }
        }
    }

    /// A probe past its budget returns None and the child is killed, not left
    /// running. The sweep matches only our own child: this sandbox's worker
    /// keeps an ambient `sleep 30` alive forever (distinct parent pid).
    #[test]
    #[cfg(unix)]
    fn run_capturing_kills_a_hung_child() {
        let out = super::run_capturing(
            std::path::Path::new("sleep"),
            &["30"],
            std::time::Duration::from_millis(150),
        );
        assert!(out.is_none(), "a hung probe must yield None");
        let victim = ["sleep", "30"];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while argv_alive(&victim) {
            assert!(
                std::time::Instant::now() < deadline,
                "timed-out probe child survived: {victim:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// A probe whose output passes the OS pipe capacity must capture the
    /// version at the stream's end — only an unblocked child that wrote
    /// everything and exited can produce it; budget-paying failures fail.
    #[test]
    #[cfg(unix)]
    fn probe_drains_output_past_pipe_capacity() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_util::temp_dir_unique("probe-chatty");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("chatty");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s 9.9.9\\n' '{}'\n",
                "x".repeat(96 * 1024)
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let budget = std::time::Duration::from_secs(2);
        let mut version = None;
        for _ in 0..3 {
            let started = std::time::Instant::now();
            version = super::probe_version(&script);
            if version.is_some() || started.elapsed() >= budget {
                break;
            }
        }
        assert_eq!(
            version.as_deref(),
            Some("9.9.9"),
            "version past the pipe capacity was lost"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    /// A grandchild inheriting the pipe cannot hold the probe open: the
    /// reclaim is abandoned inside the grace, so the caller gets None well
    /// inside the budget instead of waiting out the grandchild's lifetime.
    #[test]
    #[cfg(unix)]
    fn run_capturing_abandons_a_grandchild_held_pipe_inside_the_budget() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_util::temp_dir_unique("probe-orphan");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("spawner");
        std::fs::write(&script, "#!/bin/sh\n(sleep 10) &\nprintf 'tool 1.2.3\\n'\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = std::time::Instant::now();
        let out = super::run_capturing(&script, &[], std::time::Duration::from_secs(2));
        assert!(
            out.is_none(),
            "unreclaimable output must yield None, got {out:?}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "grandchild-held pipe outlived the reclaim grace: {:?}",
            started.elapsed()
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    /// A probe's captured output is capped and the drain keeps discarding
    /// past the cap, so a binary streaming far past it is neither buffered
    /// whole nor left blocked on a full pipe.
    #[test]
    #[cfg(unix)]
    fn run_capturing_caps_captured_output_and_keeps_draining() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_util::temp_dir_unique("probe-flood");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("flooder");
        let line = "x".repeat(4096);
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf 'tool 7.7.7\\n'\nyes '{line}' | head -c 8388608\n"),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = std::time::Instant::now();
        let out = super::run_capturing(&script, &[], std::time::Duration::from_secs(2));
        let out = out.expect("flooder probe must return its leading output");
        assert!(
            out.starts_with("tool 7.7.7"),
            "leading version line lost: {:?}",
            out.chars().take(64).collect::<String>()
        );
        assert!(
            out.len() <= 2 * 1024 * 1024,
            "captured {} bytes, past the per-pipe cap",
            out.len()
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "capped writer was left blocked: {:?}",
            started.elapsed()
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    /// A probe child sees EOF on stdin, never the parent's terminal: `cat`
    /// blocks on any inherited open stdin until the kill budget, and only a
    /// terminated child prints at all; budget-paying failures do not retry.
    #[test]
    #[cfg(unix)]
    fn probe_child_gets_null_stdin_and_prompts_end_inside_budget() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_util::temp_dir_unique("probe-stdin");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("asker");
        std::fs::write(
            &script,
            "#!/bin/sh\ncat >/dev/null\nprintf 'stdin=%s 1.0.0\\n' \"$(readlink /proc/self/fd/0)\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let budget = std::time::Duration::from_secs(2);
        let mut out = None;
        for _ in 0..3 {
            let started = std::time::Instant::now();
            out = super::run_capturing(&script, &[], budget);
            if out.is_some() || started.elapsed() >= budget {
                break;
            }
        }
        let out = out.expect("stdin-reading probe child never produced output");
        if cfg!(target_os = "linux") {
            assert!(
                out.contains("stdin=/dev/null"),
                "probe child stdin was not /dev/null: {out:?}"
            );
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    /// The two PATH lookup orders resolve different binaries when a later
    /// name sits in an earlier directory: name-major keeps the first name,
    /// dir-major keeps the first directory.
    #[test]
    fn path_lookup_orders_diverge_as_declared() {
        let dir_a = crate::test_util::temp_dir_unique("path-order-a");
        let dir_b = crate::test_util::temp_dir_unique("path-order-b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::write(dir_a.join("beta"), b"#!bin\n").unwrap();
        std::fs::write(dir_b.join("alpha"), b"#!bin\n").unwrap();
        let separator = if cfg!(windows) { ';' } else { ':' };
        let path_var = format!("{}{separator}{}", dir_a.display(), dir_b.display());
        let name_major =
            super::find_in_path_within(std::ffi::OsStr::new(&path_var), &["alpha", "beta"]);
        let dir_major = super::find_in_path_dir_first_within(&path_var, &["alpha", "beta"]);
        assert_eq!(
            name_major.as_deref(),
            Some(dir_b.join("alpha").as_path()),
            "name-major must prefer the earlier name over the earlier directory"
        );
        assert_eq!(
            dir_major.as_deref(),
            Some(dir_a.join("beta").as_path()),
            "dir-major must prefer the earlier directory over the earlier name"
        );
        drop(std::fs::remove_dir_all(&dir_a));
        drop(std::fs::remove_dir_all(&dir_b));
    }

    /// Whether any live process's argv is exactly `wanted`; /proc entries
    /// that vanish mid-scan or have unreadable cmdlines never match.
    #[cfg(all(test, unix))]
    fn argv_alive(wanted: &[&str]) -> bool {
        let parent = std::process::id();
        std::fs::read_dir("/proc").is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| argv_entry_is_our_child(&entry, wanted, parent))
        })
    }

    #[cfg(all(test, unix))]
    fn argv_entry_is_our_child(entry: &std::fs::DirEntry, wanted: &[&str], parent: u32) -> bool {
        let Ok(cmd) = std::fs::read_to_string(entry.path().join("cmdline")) else {
            return false;
        };
        if !cmd
            .split('\0')
            .filter(|a| !a.is_empty())
            .eq(wanted.iter().copied())
        {
            return false;
        }
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            return false;
        };
        let ppid = format!("PPid:\t{parent}");
        status.lines().any(|line| line == ppid)
    }

    /// Runtime backstop for `from_validated_const`: every adapter literal,
    /// including kimi-code's alias-keyed catalog entry, passes validation.
    #[test]
    fn adapter_harness_id_literals_validate() {
        for adapter in harness_catalog::all_adapters() {
            let id = adapter.id();
            assert!(
                crate::ids::HarnessId::new(id.as_str()).is_ok(),
                "catalog id `{id}` must validate"
            );
        }
        assert!(
            crate::ids::HarnessId::new(crate::adapters::kimi_code::HARNESS_ID_STR).is_ok(),
            "kimi-code literal must validate"
        );
    }

    /// Detection is a pure function of machine state: two runs must agree.
    /// One canonical check instead of per-file determinism copies.
    #[test]
    fn detection_is_deterministic_per_adapter() {
        for adapter in harness_catalog::all_adapters() {
            let id = adapter.id().as_str().to_owned();
            let first = adapter.detection();
            let second = adapter.detection();
            // A probe under a wall-clock budget can flap under load, flipping
            // present between Present and UnknownVersion with the version;
            // absence never depends on a probe and must always agree.
            assert_eq!(
                first.present == crate::state::InstallPresence::Absent,
                second.present == crate::state::InstallPresence::Absent,
                "{id}"
            );
            if first.version == second.version {
                assert_eq!(first.present, second.present, "{id}");
                assert_eq!(first.confidence, second.confidence, "{id}");
            }
        }
    }

    /// `version_resolution()` equals deriving from a fresh detection, so a
    /// caller holding one may substitute `version_resolution_from` and skip
    /// the second probe cycle. Ambient harness binaries (claude, pi, gh, ...)
    /// make `detection()` a budgeted probe that flaps under parallel load, so
    /// the check runs in a child whose PATH resolves nothing: every detection
    /// is probe-free and the equality is deterministic.
    #[test]
    fn version_resolution_matches_derivation_from_a_fresh_detection() {
        const PINNED: &str = "SUPERAI_VERSION_RESOLUTION_PINNED";
        const PINNED_OUT: &str = "SUPERAI_VERSION_RESOLUTION_PINNED_OUT";
        if std::env::var_os(PINNED).is_some() {
            let mut checked = 0usize;
            for adapter in harness_catalog::all_adapters() {
                let id = adapter.id().as_str().to_owned();
                let first = adapter.detection();
                assert_eq!(
                    first,
                    adapter.detection(),
                    "{id}: probe-free detection must be reproducible"
                );
                assert_eq!(
                    adapter.version_resolution_from(&first),
                    adapter.version_resolution(),
                    "{id}: derivation diverged from the probing path"
                );
                checked += 1;
            }
            if let Some(out) = std::env::var_os(PINNED_OUT) {
                std::fs::write(&out, checked.to_string())
                    .unwrap_or_else(|e| panic!("report checked count: {e}"));
            }
            return;
        }
        let proof_root = crate::test_util::temp_dir_unique("version-resolution-pinned");
        let proof = proof_root.join("checked");
        let child = std::process::Command::new(
            std::env::current_exe().unwrap_or_else(|e| panic!("current exe: {e}")),
        )
        .arg("--exact")
        .arg("adapters::decl_tests::version_resolution_matches_derivation_from_a_fresh_detection")
        .env("PATH", "")
        .env(PINNED, "1")
        .env(PINNED_OUT, &proof)
        .output()
        .unwrap_or_else(|e| panic!("spawn pinned child: {e}"));
        assert!(
            child.status.success(),
            "pinned child failed: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        let checked: usize = std::fs::read_to_string(&proof)
            .unwrap_or_default()
            .trim()
            .parse()
            .unwrap_or(0);
        assert!(
            checked == harness_catalog::all_adapters().len(),
            "pinned child checked {checked} adapters"
        );
        drop(std::fs::remove_dir_all(&proof_root));
    }

    /// An adapter overriding neither resolution method terminates in the
    /// fail-safe `unknown()` refusal with the detection's evidence, never in
    /// mutual recursion between the provided pair.
    #[test]
    fn unoverridden_resolution_methods_do_not_recurse() {
        use crate::adapter::Adapter as _;
        #[derive(Debug)]
        struct BareAdapter;
        impl crate::adapter::Adapter for BareAdapter {
            fn id(&self) -> crate::ids::HarnessId {
                crate::ids::HarnessId::new("bare-probe").unwrap()
            }
            fn display_name(&self) -> &'static str {
                "bare"
            }
            fn product_status(&self) -> crate::adapter::ProductStatus {
                crate::adapter::ProductStatus::Unknown
            }
            fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
                Vec::new()
            }
            fn adapter_revision(&self) -> &'static str {
                "0"
            }
            fn research_doc_link(&self) -> &'static str {
                "about:blank"
            }
            fn last_verified_date(&self) -> &'static str {
                "1970-01-01"
            }
            fn detection(&self) -> crate::adapter::DetectionResult {
                crate::adapter::DetectionResult::new(
                    crate::state::InstallPresence::UnknownVersion,
                    None,
                    vec!["bare evidence".to_owned()],
                    crate::adapter::DetectionConfidence::Low,
                )
            }
            fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
                Vec::new()
            }
            fn supported_operations(&self) -> Vec<(String, crate::state::AdapterSupport)> {
                Vec::new()
            }
            fn plan_mirror_exclusions(&self) -> Vec<String> {
                Vec::new()
            }
            fn plan_wrapper(
                &self,
                _instance: &crate::instance::Instance,
            ) -> Result<crate::adapter::WrapperPlan, crate::error::CoreError> {
                Ok(crate::adapter::WrapperPlan::new("bare"))
            }
            fn scan_candidates(&self) -> Vec<String> {
                Vec::new()
            }
            fn validate_instance(
                &self,
                _instance: &crate::instance::Instance,
            ) -> Result<(), crate::error::CoreError> {
                Ok(())
            }
        }
        let adapter = BareAdapter;
        let detection = adapter.detection();
        let via_hook = adapter.version_resolution_from(&detection);
        assert!(!via_hook.compatible, "default must refuse writes");
        assert_eq!(via_hook.notes, detection.evidence);
        let via_composition = adapter.version_resolution();
        assert_eq!(
            via_composition, via_hook,
            "provided composition must terminate on the fail-safe default"
        );
    }

    /// `version_resolution_from` never probes, and the provided
    /// `version_resolution()` composition probes exactly once — pinned by a
    /// counting detection, immune to probe flapping.
    #[test]
    fn version_resolution_from_never_probes() {
        use crate::adapter::Adapter as _;
        use std::sync::atomic::{AtomicU32, Ordering};
        #[derive(Debug)]
        struct CountingAdapter {
            probes: AtomicU32,
        }
        impl crate::adapter::Adapter for CountingAdapter {
            fn id(&self) -> crate::ids::HarnessId {
                crate::ids::HarnessId::new("counting-probe").unwrap()
            }
            fn display_name(&self) -> &'static str {
                "counting"
            }
            fn product_status(&self) -> crate::adapter::ProductStatus {
                crate::adapter::ProductStatus::Unknown
            }
            fn supported_platforms(&self) -> Vec<crate::adapter::Platform> {
                Vec::new()
            }
            fn adapter_revision(&self) -> &'static str {
                "0"
            }
            fn research_doc_link(&self) -> &'static str {
                "about:blank"
            }
            fn last_verified_date(&self) -> &'static str {
                "1970-01-01"
            }
            fn detection(&self) -> crate::adapter::DetectionResult {
                self.probes.fetch_add(1, Ordering::SeqCst);
                crate::adapter::DetectionResult::new(
                    crate::state::InstallPresence::Present,
                    Some("1.0.0".to_owned()),
                    Vec::new(),
                    crate::adapter::DetectionConfidence::High,
                )
            }
            fn version_resolution_from(
                &self,
                detection: &crate::adapter::DetectionResult,
            ) -> crate::adapter::VersionResolution {
                super::resolution_from_detection(detection, "counting", "1")
            }
            fn config_surfaces(&self) -> Vec<crate::adapter::ConfigSurface> {
                Vec::new()
            }
            fn supported_operations(&self) -> Vec<(String, crate::state::AdapterSupport)> {
                Vec::new()
            }
            fn plan_mirror_exclusions(&self) -> Vec<String> {
                Vec::new()
            }
            fn plan_wrapper(
                &self,
                _instance: &crate::instance::Instance,
            ) -> Result<crate::adapter::WrapperPlan, crate::error::CoreError> {
                Ok(crate::adapter::WrapperPlan::new("counting"))
            }
            fn scan_candidates(&self) -> Vec<String> {
                Vec::new()
            }
            fn validate_instance(
                &self,
                _instance: &crate::instance::Instance,
            ) -> Result<(), crate::error::CoreError> {
                Ok(())
            }
        }
        let adapter = CountingAdapter {
            probes: AtomicU32::new(0),
        };
        let detection = adapter.detection();
        assert_eq!(adapter.probes.load(Ordering::SeqCst), 1);
        let derived = adapter.version_resolution_from(&detection);
        let rederived = adapter.version_resolution_from(&detection);
        assert_eq!(adapter.probes.load(Ordering::SeqCst), 1);
        assert!(derived.compatible && rederived.compatible);
        let via_composition = adapter.version_resolution();
        assert_eq!(adapter.probes.load(Ordering::SeqCst), 2);
        assert_eq!(via_composition, derived);
    }

    /// (harness id, expected dest file, expected dest key) for every adapter
    /// that declares an MCP destination, per docs/harness-configs/<doc>.md.
    const MCP_DESTS: &[(&str, &str, &str)] = &[
        ("agentty", "mcp.json", "mcpServers"),
        ("amazon-q-cli", "settings.json", "mcpServers"),
        ("amp", "settings.json", "amp.mcpServers"),
        ("antigravity-cli", "mcp_config.json", "mcpServers"),
        ("auggie", "settings.json", "mcpServers"),
        ("claude-code", ".mcp.json", "mcpServers"),
        ("claude-desktop", "claude_desktop_config.json", "mcpServers"),
        ("claw-code", "settings.json", "mcpServers"),
        ("cline", "cline_mcp_settings.json", "mcpServers"),
        ("coco", "cortex/mcp.json", "mcpServers"),
        ("codex-cli", "config.toml", "mcp_servers"),
        ("command-code", "mcp.json", "mcpServers"),
        ("continue-dev", "config.yaml", "mcpServers"),
        ("copilot-cli", "mcp-config.json", "mcpServers"),
        ("copilot-vscode", ".vscode/mcp.json", "servers"),
        ("crush", "crush.json (global)", "mcp"),
        ("cursor", "mcp.json", "mcpServers"),
        ("cursor-agent", "mcp.json", "mcpServers"),
        ("devin", ".devin/mcp_config.json", "mcpServers"),
        ("every-code", "config.toml", "mcp_servers"),
        ("factory-droid", ".factory/mcp.json", "mcpServers"),
        ("forge", ".mcp.json", "mcpServers"),
        ("fx", "mcp.json", "mcp"),
        ("gemini-cli", "settings.json", "mcpServers"),
        ("gitclaw", "agent.yaml", "mcp_servers"),
        ("goose", "config.yaml", "extensions"),
        ("grok-build", "config.toml", "mcp_servers"),
        ("hermes-agent", "config.yaml", "mcp_servers"),
        ("iflow-cli", "settings.json (user)", "mcpServers"),
        ("jcode", "mcp.json", "mcpServers"),
        ("junie-cli", "mcp/mcp.json", "mcpServers"),
        ("kilo-code", "global kilo.jsonc", "mcp"),
        ("kilocode-cli", "kilo.jsonc", "mcp"),
        ("kimi-code-cli", "mcp.json", "mcpServers"),
        ("kiro", "settings/mcp.json", "mcpServers"),
        ("kode", "config.json", "mcpServers"),
        ("legacy-kimi-cli", "mcp.json", "mcpServers"),
        ("mimo-code", "mimocode.jsonc", "mcp"),
        ("minimax-code", ".mcp.json", "mcpServers"),
        ("mistral-vibe", "config.toml", "mcp_servers"),
        ("muse-code", "settings.json", "mcp_servers"),
        ("nanocoder", ".mcp.json", "mcpServers"),
        ("omo-ai", "omo.jsonc", "mcp"),
        ("omp", ".mcp.json", "mcpServers"),
        ("opencode", "opencode.json", "mcp"),
        ("openfang", "config.toml", "mcp_servers"),
        ("openhands", "mcp.json", "mcpServers"),
        ("qoder-cli", "settings.json", "mcpServers"),
        ("qwen-code", "settings.json", "mcpServers"),
        ("roo-code", ".roo/mcp.json", "mcpServers"),
        ("trae-agent", "trae_config.yaml", "mcp_servers"),
        ("warp", ".mcp.json", "mcpServers"),
        ("windsurf", "mcp_config.json", "mcpServers"),
        ("workbuddy", ".mcp.json", "mcpServers"),
        ("zed-acp", "settings.json", "context_servers"),
        ("zulu-cli", "mcp.json", "mcpServers"),
    ];

    /// Harnesses whose corpus documents NO MCP mechanism (explicit absence).
    const MCP_ABSENT: &[&str] = &[
        "aider",
        "avante-nvim",
        "bb-app",
        "chatgpt-desktop",
        "codecompanion-nvim",
        "codegeex",
        "conductor",
        "copilot-coding-agent",
        "deepseek-harness",
        "eca",
        "freebuff",
        "gptme",
        "hax",
        "jules",
        "letta-code",
        "oh-my-codex",
        "oh-my-opencode",
        "openclaw",
        "orca",
        "ori",
        "pi",
        "plandex",
        "prime-agent",
        "qoder-ide",
        "reasonix",
        "replit-agent",
        "sculptor",
        "swe-agent",
        "t3code",
        "terminus",
        "unreal-agent",
        "vibe-kanban",
        "vix",
        "zaly",
        "zcode",
    ];

    /// (harness id, expected plugin kind, requires execution) for adapters
    /// that declare a plugin mechanism.
    const PLUGIN_DECLS: &[(&str, &str, bool)] = &[
        ("amp", "directory_bundle", false),
        ("claude-code", "directory_bundle", false),
        ("coco", "directory_bundle", false),
        ("gitclaw", "directory_bundle", false),
        ("grok-build", "directory_bundle", false),
        ("hermes-agent", "npm_ref", true),
        ("junie-cli", "directory_bundle", false),
        ("kimi-code-cli", "marketplace_record", true),
        ("minimax-code", "marketplace_record", true),
        ("opencode", "directory_bundle", false),
        ("qoder-cli", "marketplace_record", true),
    ];

    /// Expected document kind for a declared MCP destination: JSONC configs
    /// are not always named `*.jsonc` (amp/kilo/mimo/omo-ai/opencode).
    fn expected_dest_kind(harness_id: &str, dest_file: &str) -> DocumentKind {
        const JSONC_DESTS: &[&str] = &[
            "amp",
            "kilo-code",
            "kilocode-cli",
            "mimo-code",
            "omo-ai",
            "opencode",
        ];
        let ext = std::path::Path::new(dest_file)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if JSONC_DESTS.contains(&harness_id) {
            DocumentKind::Jsonc
        } else {
            match ext.as_str() {
                "toml" => DocumentKind::Toml,
                "yaml" | "yml" => DocumentKind::Yaml,
                _ => DocumentKind::Json,
            }
        }
    }

    fn nested_json_seed(dest_key: &str, inner: &serde_json::Value) -> String {
        let segments: Vec<&str> = dest_key.split('.').filter(|s| !s.is_empty()).collect();
        let (last, parents) = segments
            .split_last()
            .unwrap_or((&"mcpServers", &[] as &[&str]));
        let last = *last;
        let mut value = serde_json::json!({ last: inner.clone() });
        for seg in parents.iter().rev() {
            let seg = *seg;
            value = serde_json::json!({ seg: value });
        }
        serde_json::to_string_pretty(&value).unwrap()
    }

    fn ensure_parent(path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
    }

    /// Seed a foreign server entry into `path` in the destination's native
    /// format so round-trip tests exercise foreign preservation for real.
    fn seed_foreign_server(
        path: &std::path::Path,
        decl: &crate::adapter::McpAdapterDecl,
        foreign_id: &str,
    ) {
        ensure_parent(path);
        let identity_list = matches!(
            decl.shape,
            crate::adapter::McpDestShape::IdentityList { .. }
        );
        match decl.kind {
            DocumentKind::Toml if identity_list => std::fs::write(
                path,
                format!("# foreign comment\n[[{key}]]\nname = \"{foreign_id}\"\ncommand = \"foreign-cmd\"\n", key = decl.dest_key),
            )
            .unwrap(),
            DocumentKind::Toml => std::fs::write(
                path,
                format!(
                    "# foreign comment\n[{key}.{foreign_id}]\ncommand = \"foreign-cmd\"\n",
                    key = decl.dest_key
                ),
            )
            .unwrap(),
            DocumentKind::Yaml => std::fs::write(
                path,
                format!(
                    "{key}:\n  {foreign_id}:\n    command: foreign-cmd\n    args:\n      - x\n",
                    key = decl.dest_key
                ),
            )
            .unwrap(),
            _ => {
                let inner = serde_json::json!({
                    foreign_id: {"command": "foreign-cmd", "args": ["x"]}
                });
                std::fs::write(path, nested_json_seed(&decl.dest_key, &inner)).unwrap();
            }
        }
    }

    /// Assert a declared plugin mechanism matches the expected corpus table
    /// row (kind, execution requirement, staging destination).
    fn assert_plugin_decl_matches(id: &str, decl: crate::adapter::PluginAdapterDecl) {
        let expected = PLUGIN_DECLS
            .iter()
            .find(|(hid, _, _)| *hid == id)
            .unwrap_or_else(|| panic!("{id} declares a plugin decl but is not in PLUGIN_DECLS"));
        assert_eq!(
            decl.kind.to_string(),
            expected.1,
            "{id} plugin kind mismatch"
        );
        assert_eq!(
            decl.requires_execution, expected.2,
            "{id} execution requirement mismatch"
        );
        if decl.kind.to_string() == "directory_bundle" {
            assert!(
                decl.dest_dir.is_some_and(|d| !d.is_empty()),
                "{id}: directory bundle must declare a staging destination"
            );
        }
    }

    /// Verified corpus partition of the MCP declarations: any drift in the
    /// writable/read-only/absence split fails with the real numbers.
    const EXPECTED_MCP_WRITABLE: usize = 34;
    const EXPECTED_MCP_READ_ONLY: usize = 22;
    const EXPECTED_MCP_ABSENT: usize = 35;

    #[test]
    fn every_adapter_declares_mcp_dest_or_explicit_absence() {
        let adapters = harness_catalog::all_adapters();
        let mut writable = 0;
        let mut read_only = 0;
        let mut absent = 0;
        for adapter in &adapters {
            let id = adapter.id().as_str().to_owned();
            let decl = adapter.mcp_decl();
            let absence = adapter.mcp_absence_reason();
            assert!(
                decl.is_some() != absence.is_some(),
                "{id}: exactly one of mcp_decl/mcp_absence_reason must be set (EXT-09 coverage)"
            );
            if let Some(reason) = absence {
                assert!(!reason.is_empty(), "{id}: absence must carry a reason");
                assert!(
                    MCP_ABSENT.contains(&id.as_str()),
                    "{id} declares absence but is not in the expected-absent table: {reason}"
                );
                absent += 1;
            } else {
                let decl = decl.unwrap();
                let expected = MCP_DESTS
                    .iter()
                    .find(|(hid, _, _)| *hid == id)
                    .unwrap_or_else(|| panic!("{id} declares an MCP dest but is not in MCP_DESTS"));
                assert_eq!(decl.dest_file, expected.1, "{id} dest file");
                assert_eq!(decl.dest_key, expected.2, "{id} dest key");
                assert!(!decl.dest_key.is_empty());
                let is_read_only = decl.read_only.is_some();
                read_only += usize::from(is_read_only);
                writable += usize::from(!is_read_only);
            }
        }
        assert_eq!(
            writable + read_only + absent,
            adapters.len(),
            "writable {writable} + read-only {read_only} + absence {absent} must exactly \
             partition the adapter set"
        );
        assert_eq!(
            writable, EXPECTED_MCP_WRITABLE,
            "writable MCP dest count drifted (was {writable}, expected {EXPECTED_MCP_WRITABLE})"
        );
        assert_eq!(
            read_only, EXPECTED_MCP_READ_ONLY,
            "read-only MCP dest count drifted (was {read_only}, expected {EXPECTED_MCP_READ_ONLY})"
        );
        assert_eq!(
            absent, EXPECTED_MCP_ABSENT,
            "explicit-absence count drifted (was {absent}, expected {EXPECTED_MCP_ABSENT})"
        );
        for (hid, _, _) in MCP_DESTS {
            assert!(
                adapters
                    .iter()
                    .any(|a| a.id().as_str() == *hid && a.mcp_decl().is_some()),
                "MCP_DESTS row {hid} has no adapter declaring that destination"
            );
        }
        for hid in MCP_ABSENT {
            assert!(
                adapters
                    .iter()
                    .any(|a| a.id().as_str() == *hid && a.mcp_absence_reason().is_some()),
                "MCP_ABSENT row {hid} has no adapter declaring that absence"
            );
        }
    }

    #[test]
    fn read_only_decls_carry_an_honest_reason() {
        for adapter in harness_catalog::all_adapters() {
            let Some(reason) = adapter.mcp_decl().and_then(|d| d.read_only) else {
                continue;
            };
            assert!(
                reason.len() > 20,
                "{}: read-only reason must explain the refusal, got {reason:?}",
                adapter.id()
            );
        }
    }

    #[test]
    fn mcp_decl_kinds_match_document_formats() {
        for adapter in harness_catalog::all_adapters() {
            let Some(decl) = adapter.mcp_decl() else {
                continue;
            };
            let id = adapter.id().as_str().to_owned();
            assert_eq!(
                decl.kind,
                expected_dest_kind(&id, &decl.dest_file),
                "{id}: dest file {} kind mismatch",
                decl.dest_file
            );
        }
    }

    #[test]
    fn every_adapter_declares_plugin_mechanism_or_explicit_absence() {
        let adapters = harness_catalog::all_adapters();
        let mut declared = 0;
        let mut absent = 0;
        for adapter in &adapters {
            let id = adapter.id().as_str().to_owned();
            let decl = adapter.plugin_decl();
            let absence = adapter.plugin_absence_reason();
            assert!(
                decl.is_some() != absence.is_some(),
                "{id}: exactly one of plugin_decl/plugin_absence_reason must be set (EXT-06 coverage)"
            );
            if let Some(reason) = absence {
                assert!(!reason.is_empty(), "{id}: absence must carry a reason");
                absent += 1;
            } else {
                assert_plugin_decl_matches(&id, decl.unwrap());
                declared += 1;
            }
        }
        assert_eq!(
            declared + absent,
            adapters.len(),
            "plugin decl/absence must exactly partition the adapter set"
        );
        assert_eq!(
            declared,
            PLUGIN_DECLS.len(),
            "PLUGIN_DECLS rows must match the declaring adapters one-to-one"
        );
        for (hid, _, _) in PLUGIN_DECLS {
            assert!(
                adapters
                    .iter()
                    .any(|a| a.id().as_str() == *hid && a.plugin_decl().is_some()),
                "PLUGIN_DECLS row {hid} has no adapter declaring that plugin mechanism"
            );
        }
    }

    #[test]
    fn writable_mcp_decls_round_trip_foreign_preserving() {
        // Writable dests must survive the full lifecycle: install beside a
        // foreign server, then remove ours without touching the foreign bytes.
        for adapter in harness_catalog::all_adapters() {
            let Some(decl) = adapter.mcp_decl() else {
                continue;
            };
            if decl.read_only.is_some() {
                continue;
            }
            let id = adapter.id().as_str().to_owned();
            let dir = crate::test_util::temp_dir_unique(&format!("mcp-decl-{id}"));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(&decl.dest_file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }

            let foreign_id = "foreign-server";
            seed_foreign_server(&path, &decl, foreign_id);

            let owned = crate::mcp::McpServerDef::stdio(
                crate::ids::McpServerId::new("owned-server").unwrap(),
                "node",
                vec!["server.js".to_owned()],
            )
            .unwrap();
            crate::mcp::install_mcp_server(&path, &decl, &owned)
                .unwrap_or_else(|e| panic!("{id}: install through declared dest failed: {e}"));

            let inspected = crate::mcp::inspect_servers(&path, &decl)
                .unwrap_or_else(|e| panic!("{id}: inspect failed: {e}"));
            assert!(
                inspected.contains_key(&crate::ids::McpServerId::new(foreign_id).unwrap()),
                "{id}: foreign server lost after install"
            );
            assert!(inspected.contains_key(&owned.id), "{id}: owned missing");

            crate::mcp::remove_mcp_server(&path, &decl, &owned.id)
                .unwrap_or_else(|e| panic!("{id}: remove failed: {e}"));
            let after = std::fs::read_to_string(&path).unwrap();
            assert!(
                after.contains(foreign_id),
                "{id}: foreign server must survive removal: {after}"
            );
            drop(std::fs::remove_dir_all(&dir));
        }
    }

    #[test]
    fn read_only_mcp_decls_refuse_writes_and_still_inspect() {
        for adapter in harness_catalog::all_adapters() {
            let Some(decl) = adapter.mcp_decl() else {
                continue;
            };
            let Some(reason) = decl.read_only.clone() else {
                continue;
            };
            assert_read_only_refuses(adapter.id().as_str(), &decl, &reason);
        }
    }

    /// One read-only destination: inspection still works, writes refuse with
    /// the declared reason (or the codec's `LossyWrite`), and no byte changes.
    fn assert_read_only_refuses(id: &str, decl: &crate::adapter::McpAdapterDecl, reason: &str) {
        let dir = crate::test_util::temp_dir_unique(&format!("mcp-ro-{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(&decl.dest_file);
        seed_empty_container(&path, decl);
        let before = std::fs::read(&path).unwrap();
        let inspected = crate::mcp::inspect_servers(&path, decl);
        assert!(
            inspected.is_ok(),
            "{id}: read-only decl must still inspect: {:?}",
            inspected.unwrap_err()
        );
        let owned = crate::mcp::McpServerDef::stdio(
            crate::ids::McpServerId::new("owned").unwrap(),
            "node",
            vec!["s.js".to_owned()],
        )
        .unwrap();
        let err = crate::mcp::install_mcp_server(&path, decl, &owned)
            .expect_err(&format!("{id}: read-only decl must refuse writes"));
        match err {
            crate::error::CoreError::UnsupportedOperation { reason: r, .. } => {
                assert_eq!(r, reason);
            }
            crate::error::CoreError::Config(superai_config::ConfigError::LossyWrite { .. }) => {}
            other => panic!("{id}: expected refusal, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "{id}: bytes must not change"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    /// Seed a minimal parseable empty container for a declared destination.
    fn seed_empty_container(path: &std::path::Path, decl: &crate::adapter::McpAdapterDecl) {
        ensure_parent(path);
        let seed = match decl.kind {
            DocumentKind::Toml => format!("[{}]\n", decl.dest_key.replace('.', "_")),
            DocumentKind::Yaml => format!("{}: {{}}\n", decl.dest_key),
            DocumentKind::Jsonc | DocumentKind::Json => {
                nested_json_seed(&decl.dest_key, &serde_json::json!({}))
            }
            _ => "{}".to_owned(),
        };
        std::fs::write(path, seed).unwrap();
    }

    #[test]
    fn directory_bundle_plugin_decls_stage_through_a_transaction() {
        for adapter in harness_catalog::all_adapters() {
            let Some(decl) = adapter.plugin_decl() else {
                continue;
            };
            if decl.kind.to_string() != "directory_bundle" {
                continue;
            }
            let id = adapter.id().as_str().to_owned();
            // The declared destination must name a real directory under the
            // instance root.
            let dir = crate::test_util::temp_dir_unique(&format!("plug-decl-{id}"));
            let instance_root = dir.join("instance");
            let source_dir = dir.join("bundle");
            std::fs::create_dir_all(source_dir.join("skills")).unwrap();
            std::fs::write(source_dir.join("skills").join("s.md"), b"# skill\n").unwrap();
            if let Some(manifest) = &decl.discovery_manifest {
                std::fs::write(source_dir.join(manifest), b"{}\n").unwrap();
            }
            let mut registry = crate::plugin::PluginRegistry::load(&dir.join("registry")).unwrap();
            let source = crate::plugin::PluginSource {
                id: crate::ids::PluginId::new("smoke-plugin").unwrap(),
                kind: crate::adapter::PluginKind::DirectoryBundle,
                locator: source_dir.display().to_string(),
                version: None,
                digest: None,
            };
            let record = crate::plugin::install_directory_bundle(
                &mut registry,
                &source,
                &decl,
                &instance_root,
            )
            .unwrap_or_else(|e| panic!("{id}: staging through declared dest failed: {e}"));
            let staged_dir = decl.dest_dir.clone().unwrap_or_default();
            assert!(
                instance_root
                    .join(&staged_dir)
                    .join("smoke-plugin")
                    .join("skills")
                    .join("s.md")
                    .is_file(),
                "{id}: staged file must land in the declared destination"
            );
            assert!(record.staged_files.is_some_and(|f| !f.is_empty()));
            drop(std::fs::remove_dir_all(&dir));
        }
    }

    /// Fixture for one execution-requiring decl: temp home with a seeded
    /// marketplace record, an instance config root, and an instance registry.
    /// The record is seeded by hand because `install` refuses these kinds.
    struct ExecutionDeclFixture {
        root: std::path::PathBuf,
        home: std::path::PathBuf,
        plugin_root: std::path::PathBuf,
        config_root: std::path::PathBuf,
        registry_file: std::path::PathBuf,
        registry_path: std::path::PathBuf,
    }

    fn execution_decl_fixture(
        id: &str,
        decl: &crate::adapter::PluginAdapterDecl,
    ) -> ExecutionDeclFixture {
        let dir = crate::test_util::temp_dir_unique(&format!("plug-preview-{id}"));
        let home = dir.join("home");
        let plugin_root = home.join(".superai").join("plugins");
        std::fs::create_dir_all(&plugin_root).unwrap();
        let config_root = dir.join("instance");
        std::fs::create_dir_all(&config_root).unwrap();

        let mut plugins = crate::plugin::PluginRegistry::load(&plugin_root).unwrap();
        plugins.records.push(crate::plugin::PluginRecord {
            id: crate::ids::PluginId::new("market").unwrap(),
            kind: decl.kind,
            version: None,
            digest: None,
            source_locator: "marketplace://example".to_owned(),
            installed_at: "2026-10-12T00:00:00Z".to_owned(),
            enabled: true,
            dependency_key: None,
            staged_files: None,
        });
        plugins.store().unwrap();

        let registry_file = plugin_root.join(crate::plugin::REGISTRY_FILE_NAME);
        let registry_path = dir.join("instances.json");
        ExecutionDeclFixture {
            root: dir,
            home,
            plugin_root,
            config_root,
            registry_file,
            registry_path,
        }
    }

    /// Decls whose commit path is `RequiresApproval` (qoder/kimi/minimax
    /// marketplace records today) must preview as a refusal: conflict raised,
    /// no directory-creation action planned, nothing written to disk.
    #[test]
    fn execution_requiring_plugin_decls_preview_refuses_without_mutation() {
        for adapter in harness_catalog::all_adapters() {
            let Some(decl) = adapter.plugin_decl() else {
                continue;
            };
            if !decl.requires_execution {
                continue;
            }
            let id = adapter.id().as_str().to_owned();
            let fx = execution_decl_fixture(&id, &decl);
            let registry_bytes_before = std::fs::read(&fx.registry_file).unwrap();

            let mut registry = crate::registry::Registry::load(&fx.registry_path).unwrap();
            registry
                .insert(crate::instance::Instance {
                    id: crate::ids::InstanceId::new(&format!("id-{id}")).unwrap(),
                    name: crate::ids::InstanceName::new("work").unwrap(),
                    harness: adapter.id(),
                    config_root: crate::paths::AbsolutePath::from_path(&fx.config_root).unwrap(),
                    binary: None,
                    wrapper: None,
                    isolation: crate::state::Isolation::RelocatedRoot,
                    origin: crate::state::InstanceOrigin::Created,
                    ownership: crate::state::Ownership::SuperaiCreated,
                    template: None,
                    created_at: "2026-10-12T00:00:00Z".to_owned(),
                    adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
                })
                .unwrap();
            registry.store(&fx.registry_path).unwrap();

            let request = crate::lifecycle::ReconfigureRequest::new(vec![
                crate::lifecycle::ReconfigureAction::SetPluginEnabled {
                    plugin: "market".to_owned(),
                    enabled: false,
                },
            ]);
            let preview = crate::lifecycle::preview_reconfigure_with_home(
                &registry,
                "work",
                adapter.as_ref(),
                &request,
                Some(&fx.home),
            )
            .unwrap();
            assert!(
                preview
                    .conflicts
                    .iter()
                    .any(|c| c.code == "plugin_requires_approval"),
                "{id}: preview must refuse execution-requiring plugins with a conflict, got {:?}",
                preview.conflicts
            );
            assert!(
                preview
                    .actions
                    .iter()
                    .all(|a| a.kind != crate::operation::ActionKind::CreateDir),
                "{id}: preview must plan no directory creation where commit refuses, got {:?}",
                preview.actions
            );
            let created: Vec<std::ffi::OsString> = std::fs::read_dir(&fx.config_root)
                .unwrap()
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .collect();
            assert!(
                created.is_empty(),
                "{id}: preview created entries in the config root: {created:?}"
            );
            assert_eq!(
                std::fs::read(&fx.registry_file).unwrap(),
                registry_bytes_before,
                "{id}: preview must not touch the plugin registry"
            );
            drop(std::fs::remove_dir_all(&fx.root));
        }
    }

    /// The commit side of the same property: `set_plugin_enabled` refuses
    /// with `RequiresApproval` and leaves the registry flag, registry.json,
    /// and the destination exactly as they were.
    #[test]
    fn execution_requiring_plugin_decls_commit_refuses_without_mutation() {
        for adapter in harness_catalog::all_adapters() {
            let Some(decl) = adapter.plugin_decl() else {
                continue;
            };
            if !decl.requires_execution {
                continue;
            }
            let id = adapter.id().as_str().to_owned();
            let fx = execution_decl_fixture(&id, &decl);
            let registry_bytes_before = std::fs::read(&fx.registry_file).unwrap();

            let source = crate::plugin::PluginSource {
                id: crate::ids::PluginId::new("market").unwrap(),
                kind: decl.kind,
                locator: "marketplace://example".to_owned(),
                version: None,
                digest: None,
            };
            let dest = fx.config_root.join(&decl.dest_file);
            let mut plugins = crate::plugin::PluginRegistry::load(&fx.plugin_root).unwrap();
            let err = crate::plugin::set_plugin_enabled(&mut plugins, &decl, &dest, &source, false)
                .unwrap_err();
            assert!(
                matches!(err, crate::error::CoreError::RequiresApproval { .. }),
                "{id}: expected RequiresApproval, got {err:?}"
            );
            assert!(
                plugins.get(&source.id).is_some_and(|r| r.enabled),
                "{id}: the enabled flag must survive the refusal"
            );
            assert_eq!(
                std::fs::read(&fx.registry_file).unwrap(),
                registry_bytes_before,
                "{id}: the refusal must leave registry.json untouched"
            );
            assert!(
                !dest.exists(),
                "{id}: no destination may materialize where commit refuses"
            );
            drop(std::fs::remove_dir_all(&fx.root));
        }
    }
}
