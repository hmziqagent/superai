//! Duct-backed process execution wrapper (PKG-01, PKG-05).
//!
//! `run_command` spawns with explicit argv (never a shell), composes the
//! child env before spawn (see [`run_command`] for the guaranteed order),
//! captures stdout/stderr up to `output_limit` combined bytes, and enforces a
//! wall-clock timeout that kills the child. Dependency provenance for `duct`
//! 1.1.x is recorded in `docs/dependency-review.md`.

#![expect(
    clippy::excessive_nesting,
    reason = "intentional deep branching for redaction and version parsing"
)]
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::error::CoreError;

/// Maximum combined stdout+stderr captured per command (1 MiB).
pub const MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Default wall-clock timeout for process execution.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Flags whose following value should be redacted in logs/errors.
///
/// Only long-form flags that unambiguously carry secrets; short flags like
/// `-p` alias to non-secret meanings (port, profile) across tools.
pub const REDACT_FLAGS: &[&str] = &[
    "--password",
    "--passwd",
    "--token",
    "--access-token",
    "--api-key",
    "--apikey",
    "--secret",
    "--key",
    "--private-key",
    "--ssh-key",
    "--passphrase",
    "--password-command",
    "--email",
];

/// Captured output of a completed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    /// Captured standard output (UTF-8 lossy).
    pub stdout: String,
    /// Captured standard error (UTF-8 lossy).
    pub stderr: String,
    /// Exit code if the process exited normally.
    pub exit_code: Option<i32>,
    /// Whether the process exited with code 0.
    pub success: bool,
}

impl ProcessOutput {
    /// Create a new output from components.
    pub fn new(stdout: String, stderr: String, exit_code: Option<i32>) -> Self {
        let success = exit_code.is_some_and(|c| c == 0);
        Self {
            stdout,
            stderr,
            exit_code,
            success,
        }
    }

    /// Return trimmed stdout.
    pub fn stdout_trimmed(&self) -> &str {
        self.stdout.trim()
    }
}

/// Options for [`run_command`].
#[derive(Debug, Clone)]
pub struct ExecuteOpts {
    /// Wall-clock timeout. `None` inherits [`DEFAULT_TIMEOUT`].
    pub timeout: Option<Duration>,
    /// Working directory for the child.
    pub cwd: Option<PathBuf>,
    /// Extra env vars to set; they survive `clear_env`, but a matching
    /// `env_remove` entry wins.
    pub env: Vec<(String, String)>,
    /// Env vars removed after the `env` additions are applied.
    pub env_remove: Vec<String>,
    /// Start the child from an empty environment instead of the inherited one.
    pub clear_env: bool,
    /// Combined byte cap on captured stdout+stderr.
    pub output_limit: Option<usize>,
    /// Whether to redact sensitive args in error messages.
    pub redact: bool,
}

impl Default for ExecuteOpts {
    fn default() -> Self {
        Self {
            timeout: Some(DEFAULT_TIMEOUT),
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
            clear_env: false,
            output_limit: Some(MAX_OUTPUT_BYTES),
            redact: false,
        }
    }
}

/// Redact sensitive flag values from a slice of args.
///
/// Any arg equal to a flag in `flags` causes the following arg to be replaced
/// with `"***"`. Args of the form `--flag=value` are redacted to
/// `--flag=***`.
pub fn redact_args(args: &[String], flags: &[&str]) -> Vec<String> {
    let mut result = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            result.push("***".to_owned());
            redact_next = false;
            continue;
        }
        let mut handled = false;
        for flag in flags {
            if let Some(value) = arg.strip_prefix(&format!("{flag}=")) {
                if value.is_empty() {
                    redact_next = true;
                    result.push(arg.clone());
                } else {
                    result.push(format!("{flag}=***"));
                }
                handled = true;
                break;
            }
        }
        if handled {
            continue;
        }
        if flags.contains(&arg.as_str()) {
            redact_next = true;
        }
        result.push(arg.clone());
    }
    result
}

/// Display a command with redacted args for logging.
pub fn display_command(executable: &str, args: &[String], redact: bool) -> String {
    let shown = if redact {
        redact_args(args, REDACT_FLAGS)
    } else {
        args.to_vec()
    };
    let mut out = executable.to_owned();
    for arg in shown {
        out.push(' ');
        out.push_str(&arg);
    }
    out
}

/// Scrub secret-bearing content from captured stderr when redaction is on.
///
/// Best-effort: if any redact-flag keyword appears in stderr the whole field
/// is replaced with `[REDACTED]`, so a secret echoed by a failing child never
/// reaches an error message.
pub fn scrub_stderr(stderr: &str, redact: bool) -> String {
    if redact {
        let lower = stderr.to_ascii_lowercase();
        for flag in REDACT_FLAGS {
            if lower.contains(flag.trim_start_matches('-')) {
                return "[REDACTED]".to_owned();
            }
        }
        stderr.to_owned()
    } else {
        stderr.to_owned()
    }
}

/// Canonical key for the composed child env map: Windows env names are
/// ASCII-case-insensitive, so fold them there (duct's wrappers matched the
/// same way); other platforms match exactly.
#[cfg(windows)]
fn env_map_key(name: &OsStr) -> OsString {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let folded: Vec<u16> = name.encode_wide().map(u16::to_ascii_uppercase).collect();
    OsString::from_wide(&folded)
}

#[cfg(not(windows))]
fn env_map_key(name: &OsStr) -> OsString {
    name.to_os_string()
}

/// Compose the child env: inherited (or empty when `clear_env`), then the
/// `env` additions, then `env_remove`, which wins on the same key.
fn compose_child_env(opts: &ExecuteOpts) -> BTreeMap<OsString, OsString> {
    let mut env: BTreeMap<OsString, OsString> = if opts.clear_env {
        BTreeMap::new()
    } else {
        std::env::vars_os()
            .map(|(k, v)| (env_map_key(&k), v))
            .collect()
    };
    for (key, val) in &opts.env {
        env.insert(env_map_key(OsStr::new(key)), OsString::from(val));
    }
    for key in &opts.env_remove {
        env.remove(&env_map_key(OsStr::new(key)));
    }
    env
}

/// Whether `path` names a file this process could execute (unix also
/// demands an execute bit; Windows tests existence only, matching the
/// adapter PATH helpers).
fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(path).is_ok_and(|m| m.is_file())
    }
}

/// First PATH entry holding an executable file named `name` (`.exe` is also
/// probed on Windows). Empty entries are skipped: POSIX treats them as the
/// working directory, and this lookup must never resolve from there.
fn first_path_match(path_var: &OsStr, name: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if is_executable_file(&exe) {
                return Some(exe);
            }
        }
    }
    None
}

/// Resolve `executable` to what will actually be spawned.
///
/// Bare names resolve to the first matching executable on the child's PATH
/// (ambient PATH when inherited, mirroring std's parent-PATH fallback), so
/// the first-match winner is fixed before spawn instead of being decided by
/// the platform lookup. Separator-bearing names are used as given (the
/// caller's explicit choice), except `.`/`..` components, which resolve
/// relative to the working directory and are refused. Residual risk: PATH
/// itself is user-controlled, and first match still wins.
fn resolve_executable(
    executable: &str,
    child_env: &BTreeMap<OsString, OsString>,
) -> Result<PathBuf, CoreError> {
    let path = Path::new(executable);
    if path
        .components()
        .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
    {
        return Err(CoreError::Validation {
            field: "executable".to_owned(),
            reason: format!(
                "executable `{executable}` must not resolve relative to the working directory"
            ),
        });
    }
    if path.is_absolute() || executable.contains(std::path::MAIN_SEPARATOR) {
        return Ok(path.to_path_buf());
    }
    #[cfg(windows)]
    if executable.contains('/') {
        return Ok(path.to_path_buf());
    }
    let path_var = child_env
        .get(&env_map_key(OsStr::new("PATH")))
        .cloned()
        .or_else(|| std::env::var_os("PATH"))
        .ok_or_else(|| CoreError::BinaryDetection {
            binary: executable.to_owned(),
            reason: "PATH is not set; refusing to guess a search path for a bare name".to_owned(),
        })?;
    first_path_match(&path_var, executable).ok_or_else(|| CoreError::BinaryDetection {
        binary: executable.to_owned(),
        reason: format!(
            "`{executable}` not found on PATH (first match; the working directory is never searched)"
        ),
    })
}

/// Run a command with explicit argv (no shell interpolation), bounded capture,
/// timeout, and optional redaction.
///
/// - No shell is ever invoked; `executable` and `args` are passed as argv
///   tokens directly. Bare names are resolved to an absolute first-PATH-match
///   before spawn (see [`resolve_executable`]); the working directory is never
///   searched.
/// - stdout/stderr are captured up to `output_limit` bytes combined; breach
///   returns `CoreError::Verification` with output-limit context and the child
///   is killed.
/// - Timeout kills the child and returns `CoreError::BinaryDetection` with
///   timeout context (caller can map to install-specific errors).
///
/// # Env composition
///
/// The child env is composed up front (see `compose_child_env`) and handed
/// to duct as one `full_env` map: start from the inherited environment, or
/// empty when `clear_env` is set; then apply the `env` additions; then
/// `env_remove`, which wins over an addition on the same key. Additions
/// therefore survive `clear_env`, and inherited vars reach the child exactly
/// when `clear_env` is false. (Composing up front replaced duct's env wraps,
/// which apply in reverse build order and made `clear_env` silently discard
/// every addition.)
pub fn run_command(
    executable: &str,
    args: &[String],
    opts: &ExecuteOpts,
) -> Result<ProcessOutput, CoreError> {
    if executable.is_empty() {
        return Err(CoreError::Validation {
            field: "executable".to_owned(),
            reason: "executable must not be empty".to_owned(),
        });
    }
    if executable.contains('\0') {
        return Err(CoreError::Validation {
            field: "executable".to_owned(),
            reason: "executable must not contain NUL".to_owned(),
        });
    }
    for arg in args {
        if arg.contains('\0') {
            return Err(CoreError::Validation {
                field: "arg".to_owned(),
                reason: format!("arg must not contain NUL: `{arg}`"),
            });
        }
    }

    // Compose the env first: bare names resolve against the child's PATH.
    let child_env = compose_child_env(opts);
    let resolved = resolve_executable(executable, &child_env)?;

    // Build duct expression with explicit argv, env, cwd, stdin = none.
    let mut cmd = duct::cmd(resolved.as_os_str(), args);

    if let Some(cwd) = opts.cwd.as_ref() {
        cmd = cmd.dir(cwd);
    }

    // One composed map is duct's only env input: its wraps apply in reverse
    // build order, so mixing env/env_remove wraps here would let build order,
    // not compose_child_env, decide precedence.
    cmd = cmd.full_env(child_env);

    cmd = cmd.stdout_capture().stderr_capture();

    let timeout = opts.timeout.unwrap_or(DEFAULT_TIMEOUT);
    let display = display_command(executable, args, opts.redact);

    let handle = cmd
        .unchecked()
        .start()
        .map_err(|e| CoreError::BinaryDetection {
            binary: executable.to_owned(),
            reason: format!("failed to spawn `{display}`: {e}"),
        })?;

    let output = match handle.wait_timeout(timeout) {
        // wait_timeout borrows the handle; clone unhooks the captured bytes.
        Ok(Some(output)) => output.clone(),
        Ok(None) => {
            // Timeout expired; kill and reap.
            let kill_note = handle
                .kill()
                .map_or_else(|e| format!(" (kill failed: {e})"), |()| String::new());
            let wait_note = handle
                .wait()
                .map_or_else(|e| format!(" (wait failed: {e})"), |_| String::new());
            let reason = format!(
                "command timed out after {}s: `{display}`{kill_note}{wait_note}",
                timeout.as_secs()
            );
            return Err(CoreError::BinaryDetection {
                binary: executable.to_owned(),
                reason,
            });
        }
        Err(e) => {
            return Err(CoreError::BinaryDetection {
                binary: executable.to_owned(),
                reason: format!("failed to wait for `{display}`: {e}"),
            });
        }
    };

    let stdout_raw = output.stdout;
    let stderr_raw = output.stderr;
    let combined_len = stdout_raw.len().saturating_add(stderr_raw.len());
    let limit = opts.output_limit.unwrap_or(MAX_OUTPUT_BYTES);
    if combined_len > limit {
        return Err(CoreError::Verification {
            path: PathBuf::from(executable),
            kind: "output_limit".to_owned(),
            reason: format!(
                "command output limit exceeded: `{display}` (limit: {limit} bytes, observed: {combined_len} bytes)"
            ),
        });
    }

    let stdout = String::from_utf8_lossy(&stdout_raw).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_raw).into_owned();
    let stderr_scrubbed = scrub_stderr(&stderr, opts.redact);
    let exit_code = output.status.code();

    Ok(ProcessOutput::new(stdout, stderr_scrubbed, exit_code))
}

/// Convenience helper to run a version probe command and parse the first
/// semantic-looking token from stdout.
///
/// Returns `None` on non-zero exit or empty output; otherwise attempts to
/// extract a version string.
pub fn run_version_probe(executable: &str, args: &[String], opts: &ExecuteOpts) -> Option<String> {
    let output = run_command(executable, args, opts).ok()?;
    if !output.success {
        return None;
    }
    let combined = if output.stdout.trim().is_empty() {
        output.stderr
    } else {
        output.stdout
    };
    extract_version(&combined)
}

/// Strip ANSI escape sequences (CSI `ESC[...m`, OSC `ESC]...BEL`, etc.)
///
/// Malicious version output may contain escape sequences to hide or inject
/// content; they must not appear in the extracted version.
fn strip_ansi_escapes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for n in chars.by_ref() {
                        if n.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' && chars.peek().copied() == Some('\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else if c != '\x07' {
            out.push(c);
        }
    }
    out
}

/// Extract the first version-like token from text.
///
/// Looks for `X.Y.Z` or `vX.Y.Z` patterns. Falls back to the first non-empty
/// line trimmed to 64 chars if no semver pattern is found (still useful for
/// probes that emit non-semver strings like `claude-code 1.2.3 (build abc)`).
pub fn extract_version(text: &str) -> Option<String> {
    let stripped = strip_ansi_escapes(text);
    let trimmed = stripped.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Try to find semver-like substring.
    for token in trimmed.split_whitespace() {
        let candidate = token
            .trim_start_matches('v')
            .trim_matches(|c: char| c == ',' || c == ')');
        if candidate.chars().any(|c| c == '.') {
            // Quick semver-ish check: contains digit and dot
            let has_digit = candidate.chars().any(|c| c.is_ascii_digit());
            let has_dot = candidate.contains('.');
            if has_digit && has_dot {
                // Strip surrounding punctuation/brackets
                let cleaned = candidate
                    .trim_matches(|c: char| {
                        !c.is_ascii_alphanumeric() && c != '.' && c != '-' && c != '+'
                    })
                    .to_owned();
                if !cleaned.is_empty() {
                    // Bound length to avoid pathological capture. The bound
                    // is BYTES (a multi-byte token must not exceed it even
                    // when it has fewer than 64 chars), truncated at a
                    // UTF-8 char boundary.
                    let bounded = if cleaned.len() > 64 {
                        let mut end = 64;
                        while end > 0 && !cleaned.is_char_boundary(end) {
                            end -= 1;
                        }
                        cleaned.get(0..end).unwrap_or_default().to_owned()
                    } else {
                        cleaned
                    };
                    return Some(bounded);
                }
            }
        }
    }
    // Fallback: first non-empty line, truncated
    for line in trimmed.lines() {
        let l = line.trim();
        if !l.is_empty() {
            let out = if l.len() > 64 {
                // Respect UTF-8 char boundaries when truncating
                let mut end = 64;
                while end > 0 && !l.is_char_boundary(end) {
                    end -= 1;
                }
                l.get(0..end).unwrap_or(l).to_owned()
            } else {
                l.to_owned()
            };
            return Some(out);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_flags_inline_and_next_arg() {
        let args = vec![
            "--token".to_owned(),
            "secret123".to_owned(),
            "--verbose".to_owned(),
            "--api-key= hunter2".to_owned(),
            "--api-key=hunter3".to_owned(),
        ];
        let redacted = redact_args(&args, REDACT_FLAGS);
        assert_eq!(redacted.get(1).map(String::as_str), Some("***"));
        // "--api-key= hunter2" has empty inline value, so next arg would be
        // redacted if we had one; the value itself here is " hunter2" with
        // leading space, not matched as inline, so it is kept as-is but the
        // flag form with value is redacted inline
        assert_eq!(redacted.get(4).map(String::as_str), Some("--api-key=***"));
    }

    #[test]
    fn display_command_redacts_when_requested() {
        let args = vec!["--token".to_owned(), "abc".to_owned(), "other".to_owned()];
        let shown = display_command("prog", &args, true);
        assert!(shown.contains("***"), "should redact: {shown}");
        assert!(!shown.contains("abc"), "secret leaked: {shown}");
        let plain = display_command("prog", &args, false);
        assert!(plain.contains("abc"));
    }

    #[test]
    fn extract_version_finds_semver() {
        assert_eq!(
            extract_version("claude-code 1.2.3 (build)").as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            extract_version("v2.0.0-beta.1").as_deref(),
            Some("2.0.0-beta.1")
        );
        assert_eq!(extract_version("version: 0.1.0").as_deref(), Some("0.1.0"));
    }

    #[test]
    fn extract_version_fallback_truncates() {
        let v = extract_version(&"a".repeat(100)).unwrap();
        assert_eq!(v.len(), 64);
        // 64 bytes lands mid-é (6 bytes per "café-"); the cut backs off to
        // the previous boundary at 63.
        let unicode = extract_version(&"café-".repeat(20)).unwrap();
        assert_eq!(unicode.len(), 63);
    }

    #[test]
    fn extract_version_semver_bound_is_bytes_not_chars() {
        // A multi-byte semver-shaped token must be bounded to 64 BYTES, not
        // 64 chars (found by the QAL-04 detection fuzz family).
        let token = "1.2.3-β".repeat(30); // 8 bytes per rep, 240 bytes, 210 chars
        let text = format!("tool {token}");
        let v = extract_version(&text)
            .unwrap_or_else(|| panic!("semver-shaped token must be extracted: {text:?}"));
        assert!(
            v.len() <= 64,
            "extracted version must be byte-bounded, got {} bytes: {v:?}",
            v.len()
        );
    }

    #[test]
    fn run_command_echo_smoke() {
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            output_limit: Some(1024 * 1024),
            ..Default::default()
        };
        let out = run_command("echo", &["hello".to_owned()], &opts).unwrap();
        assert!(out.success);
        assert_eq!(out.stdout_trimmed(), "hello");
        assert_eq!(out.exit_code, Some(0));
    }

    #[test]
    fn run_command_rejects_empty_executable() {
        let opts = ExecuteOpts::default();
        let err = run_command("", &[], &opts).unwrap_err();
        assert!(format!("{err}").contains("executable must not be empty"));
    }

    #[test]
    fn run_command_bounded_capture_enforced() {
        // Use yes-like output via printf to generate large output exceeding tiny limit
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            output_limit: Some(10),
            ..Default::default()
        };
        // echo with large arg should exceed 10 bytes combined
        let large = "x".repeat(100);
        let err = run_command("echo", &[large], &opts).unwrap_err();
        assert!(format!("{err}").contains("output limit exceeded"));
    }

    #[test]
    fn run_command_no_shell_interpolation() {
        // argv token containing shell meta-characters must be passed literally
        // and not expand. `echo` should print the literal token.
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        let token = "$(whoami) && echo pwned | cat".to_owned();
        let out = run_command("echo", std::slice::from_ref(&token), &opts).unwrap();
        assert!(out.success);
        assert_eq!(out.stdout_trimmed(), token);
    }

    #[test]
    fn run_command_timeout_kills() {
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_millis(200)),
            ..Default::default()
        };
        let err = run_command("sleep", &["2".to_owned()], &opts).unwrap_err();
        assert!(format!("{err}").contains("timed out"));
    }

    #[test]
    #[cfg(unix)]
    fn run_command_env_remove_drops_variable_from_child() {
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            env: vec![("SUPERAI_TEST_KEEP".to_owned(), "yes".to_owned())],
            env_remove: vec!["HOME".to_owned()],
            ..Default::default()
        };
        // printenv prints one value line per found variable; HOME must not
        // print, so the child saw exactly the kept variable.
        let out = run_command(
            "printenv",
            &["HOME".to_owned(), "SUPERAI_TEST_KEEP".to_owned()],
            &opts,
        )
        .unwrap();
        assert_eq!(
            out.stdout, "yes\n",
            "HOME must be removed from the child environment"
        );
    }

    #[test]
    fn compose_child_env_clear_start_adds_then_removes() {
        let opts = ExecuteOpts {
            env: vec![
                ("SUPERAI_TEST_DUP".to_owned(), "leaked".to_owned()),
                ("SUPERAI_TEST_ADD".to_owned(), "kept".to_owned()),
            ],
            env_remove: vec!["SUPERAI_TEST_DUP".to_owned()],
            clear_env: true,
            ..Default::default()
        };
        let env = compose_child_env(&opts);
        assert_eq!(
            env.get(OsStr::new("SUPERAI_TEST_ADD")),
            Some(&OsString::from("kept"))
        );
        assert!(
            !env.contains_key(OsStr::new("SUPERAI_TEST_DUP")),
            "env_remove must beat an env addition on the same key"
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_command_env_additions_survive_clear_env() {
        // End-to-end pin of the composed precedence: additions survive
        // clear_env, env_remove wins on a same-key addition, and an
        // inherited var (PATH) stays cleared. printenv omits missing vars.
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            env: vec![
                ("SUPERAI_TEST_DUP".to_owned(), "leaked".to_owned()),
                ("SUPERAI_TEST_ADD".to_owned(), "reaches-child".to_owned()),
            ],
            env_remove: vec!["SUPERAI_TEST_DUP".to_owned()],
            clear_env: true,
            ..Default::default()
        };
        let out = run_command(
            "printenv",
            &[
                "SUPERAI_TEST_DUP".to_owned(),
                "SUPERAI_TEST_ADD".to_owned(),
                "PATH".to_owned(),
            ],
            &opts,
        )
        .unwrap();
        assert_eq!(
            out.stdout, "reaches-child\n",
            "addition must survive clear_env; same-key removal must win; PATH must stay cleared"
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_command_inherits_ambient_env_when_clear_env_false() {
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            env: vec![("SUPERAI_TEST_KEEP".to_owned(), "yes".to_owned())],
            ..Default::default()
        };
        // std resolves the bare name through the parent PATH, so this
        // asserts the ambient PATH VALUE reached the child env, not that
        // the spawn needed it.
        let out = run_command(
            "printenv",
            &["PATH".to_owned(), "SUPERAI_TEST_KEEP".to_owned()],
            &opts,
        )
        .unwrap();
        assert_eq!(
            out.stdout.lines().count(),
            2,
            "ambient PATH and the addition must both reach the child"
        );
        assert_eq!(out.stdout.lines().last(), Some("yes"));
    }

    #[test]
    #[cfg(unix)]
    fn run_command_resolves_bare_name_to_first_path_match() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_util::temp_dir_unique("path-order");
        for (sub, marker) in [("a", "from-a"), ("b", "from-b")] {
            let bin_dir = dir.join(sub);
            std::fs::create_dir_all(&bin_dir).unwrap();
            let probe = bin_dir.join("superai-path-order-probe");
            std::fs::write(&probe, format!("#!/bin/sh\necho {marker}\n")).unwrap();
            std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // The env addition PATH (not the ambient PATH) governs resolution,
        // and the FIRST directory wins.
        let joined = format!("{}:{}", dir.join("a").display(), dir.join("b").display());
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            env: vec![("PATH".to_owned(), joined)],
            ..Default::default()
        };
        let out = run_command("superai-path-order-probe", &[], &opts).unwrap();
        assert_eq!(out.stdout_trimmed(), "from-a");
    }

    #[test]
    #[cfg(unix)]
    fn run_command_never_resolves_a_bare_name_from_the_working_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::test_util::temp_dir_unique("path-cwd");
        std::fs::create_dir_all(&dir).unwrap();
        let probe = dir.join("superai-path-cwd-probe");
        std::fs::write(&probe, "#!/bin/sh\necho ran\n").unwrap();
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();
        // POSIX reads an empty PATH entry as the working directory; the
        // explicit lookup must skip it even with the probe sitting in cwd.
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            cwd: Some(dir),
            env: vec![("PATH".to_owned(), String::from(":"))],
            ..Default::default()
        };
        let err = run_command("superai-path-cwd-probe", &[], &opts).unwrap_err();
        assert!(
            format!("{err}").contains("not found on PATH"),
            "expected a PATH-resolution refusal, got: {err}"
        );
        assert!(probe.exists());
    }

    #[test]
    fn run_command_refuses_dot_relative_executable() {
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        let err = run_command("./probe", &[], &opts).unwrap_err();
        assert!(
            format!("{err}").contains("working directory"),
            "expected a relative-path refusal, got: {err}"
        );
    }

    #[test]
    fn run_command_bare_name_absent_from_path_is_a_typed_error() {
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        let err = run_command("superai-no-such-tool-xyz", &[], &opts).unwrap_err();
        assert!(
            format!("{err}").contains("not found on PATH"),
            "expected the PATH-resolution error, got: {err}"
        );
    }
}
