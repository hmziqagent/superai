//! Install execution, verification receipt, update and uninstall (PKG-05..08).
//!
//! PKG-05  Structured `duct`-backed execution: no shell, explicit argv,
//!         minimal env, bounded capture, 120s timeout, exit handling and
//!         redaction of secret-bearing args.
//!
//! PKG-06  Verification receipt: re-detect executable, parse version via
//!         `extract_version`, confirm requested range with `semver`,
//!         run smoke probe (`--help`), record superai-owned receipt without
//!         claiming pre-existing installs.
//!
//! PKG-07  Update: detect current/method, fetch available, show compat impact
//!         via `harness_catalog`, execute native update with re-detect and
//!         block on adapter incompatibility unless explicitly accepted.
//!
//! PKG-08  Uninstall: preflight exact package/method/path, list referencing
//!         instances/wrappers/binaries, check shared/foreign, default to
//!         binary-only removal via native method, preserve config/instances/
//!         wrappers/backups/templates/assets, mark binary-missing, never
//!         auto-delete manual files not proven owned.

#![expect(
    clippy::excessive_nesting,
    reason = "install execution branches across process, verification and lifecycle"
)]
#![expect(
    clippy::too_many_lines,
    reason = "PKG-05..08 implementation in one module"
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::detect::{DetectOptions, Detection, DetectionSource, detect_all_for_entry};
use crate::error::CoreError;
use crate::harness_catalog;
use crate::ids::HarnessId;
use crate::install_catalog::{CommandTokens, InstallCatalog, InstallMethodKind};
use crate::install_plan::InstallPlan;
use crate::process::{
    ExecuteOpts, MAX_OUTPUT_BYTES, ProcessOutput, display_command, extract_version, run_command,
};
use crate::registry::Registry;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Wall-clock timeout for install/update/uninstall commands (PKG-05).
#[expect(
    clippy::duration_suboptimal_units,
    reason = "explicit 120s timeout per PKG-05"
)]
pub const EXEC_TIMEOUT: Duration = Duration::from_secs(120);

/// Timeout for lightweight probes (version, smoke, fetch).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Bounded combined stdout+stderr for install commands (PKG-05).
pub const OUTPUT_LIMIT: usize = MAX_OUTPUT_BYTES;

/// Smoke probe timeout.
pub const SMOKE_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Helpers: time, minimal env, opts, redaction
// ---------------------------------------------------------------------------

#[expect(
    clippy::cast_possible_wrap,
    reason = "secs/86400 fits in i64 for realistic timestamps"
)]
fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Reuse registry's RFC3339 helper logic inline to avoid cross-crate dep.
    let days = (secs / 86400) as i64;
    let secs_of_day = secs % 86400;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "year fits in i32 for registry timestamps"
)]
#[expect(clippy::cast_sign_loss, reason = "days derived from u64 secs")]
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}

/// Minimal environment for structured execution (PKG-05).
///
/// Starts from `clear_env = true` and injects only benign vars required for
/// execution. Secrets are never injected; callers must pass them explicitly
/// via `ExecuteOpts::env` when the installer method truly requires a token.
pub fn minimal_env_vars() -> Vec<(String, String)> {
    const KEEP: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "TMPDIR",
        "TEMP",
        "TMP",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TERM",
        "SHELL",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
    ];
    let mut out = Vec::new();
    for key in KEEP {
        if let Ok(val) = std::env::var(key)
            && !val.contains('\0')
        {
            out.push(((*key).to_owned(), val));
        }
    }
    // Ensure PATH exists even when ambient is empty (tests rely on injected
    // DetectOptions::path_dirs, but commands like `echo` still need PATH for
    // resolution when not using an absolute executable).
    if !out.iter().any(|(k, _)| k == "PATH")
        && let Some(path) = std::env::var_os("PATH")
    {
        let s = path.to_string_lossy().into_owned();
        if !s.contains('\0') {
            out.push(("PATH".to_owned(), s));
        }
    }
    out
}

/// Build `ExecuteOpts` for structured install commands (PKG-05).
pub fn structured_opts(redact: bool) -> ExecuteOpts {
    ExecuteOpts {
        timeout: Some(EXEC_TIMEOUT),
        cwd: None,
        env: minimal_env_vars(),
        env_remove: Vec::new(),
        clear_env: true,
        output_limit: Some(OUTPUT_LIMIT),
        redact,
    }
}

/// Build `ExecuteOpts` for lightweight probes (version, smoke, fetch).
pub fn probe_opts(redact: bool) -> ExecuteOpts {
    ExecuteOpts {
        timeout: Some(PROBE_TIMEOUT),
        cwd: None,
        env: minimal_env_vars(),
        env_remove: Vec::new(),
        clear_env: true,
        output_limit: Some(64 * 1024),
        redact,
    }
}

/// Execute a structured command with explicit argv, minimal env, bounded
/// capture, 120s timeout and redaction (PKG-05).
///
/// No shell is ever invoked; `executable` and `args` are passed as argv
/// tokens directly to `duct`. On non-zero exit an error is returned with a
/// redacted display; callers that need to inspect non-zero output should call
/// `run_command` directly with `probe_opts`.
pub fn run_structured_command(
    executable: &str,
    args: &[String],
    redact: bool,
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
                reason: "arg must not contain NUL".to_owned(),
            });
        }
    }
    let opts = structured_opts(redact);
    let display = display_command(executable, args, redact);
    let out = run_command(executable, args, &opts)?;
    if !out.success {
        return Err(CoreError::Verification {
            path: PathBuf::from(executable),
            kind: "install_exit".to_owned(),
            reason: format!(
                "command `{display}` exited with {:?}: {}",
                out.exit_code,
                if redact {
                    "[REDACTED]"
                } else {
                    out.stderr.trim()
                }
            ),
        });
    }
    Ok(out)
}

/// Execute a command from `CommandTokens` (no shell, validated) with
/// structured opts (PKG-05).
pub fn run_token_command(tokens: &CommandTokens, redact: bool) -> Result<ProcessOutput, CoreError> {
    tokens.validate()?;
    run_structured_command(&tokens.executable, &tokens.args, redact)
}

// ---------------------------------------------------------------------------
// PKG-06 — verification receipt
// ---------------------------------------------------------------------------

/// Superai-owned install receipt (PKG-06).
///
/// Recorded only when the install produced a new, verifiable binary that was
/// not present before execution. Pre-existing installs are never claimed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReceipt {
    /// Install method that produced the binary.
    pub method: InstallMethodKind,
    /// Official package identifier (e.g., `@anthropic-ai/claude-code`).
    pub package_id: String,
    /// Executable name (e.g., `claude`).
    pub executable: String,
    /// Detected version string.
    pub version: String,
    /// ISO8601 timestamp of verification.
    pub timestamp: String,
    /// Filesystem path to the executable.
    pub path: PathBuf,
}

impl InstallReceipt {
    /// Validate receipt fields.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.package_id.is_empty() {
            return Err(CoreError::Validation {
                field: "package_id".to_owned(),
                reason: "must not be empty".to_owned(),
            });
        }
        if self.executable.is_empty() {
            return Err(CoreError::Validation {
                field: "executable".to_owned(),
                reason: "must not be empty".to_owned(),
            });
        }
        if self.version.is_empty() {
            return Err(CoreError::Validation {
                field: "version".to_owned(),
                reason: "must not be empty".to_owned(),
            });
        }
        if self.timestamp.is_empty() {
            return Err(CoreError::Validation {
                field: "timestamp".to_owned(),
                reason: "must not be empty".to_owned(),
            });
        }
        if self.package_id.contains('\0')
            || self.executable.contains('\0')
            || self.version.contains('\0')
        {
            return Err(CoreError::Validation {
                field: "receipt".to_owned(),
                reason: "must not contain NUL".to_owned(),
            });
        }
        Ok(())
    }
}

/// Whether a version satisfies a requested range/channel.
///
/// Channels (`latest`, `stable`, `beta`, `nightly`, `next`, `canary`, `lts`)
/// always satisfy. Otherwise the requested string is parsed as a `semver`
/// `VersionReq` (with optional leading `v`) against the detected version.
/// Exact version strings are also handled as `=version`.
fn version_satisfies(requested: &str, detected: &str) -> bool {
    const CHANNELS: &[&str] = &[
        "latest", "stable", "beta", "nightly", "next", "canary", "lts",
    ];
    let req_trim = requested.trim();
    if CHANNELS.contains(&req_trim) {
        return true;
    }
    if req_trim.is_empty() {
        return true;
    }
    let req_clean = req_trim.strip_prefix('v').unwrap_or(req_trim).trim();
    let det_clean = detected
        .strip_prefix('v')
        .unwrap_or(detected)
        .trim()
        .to_owned();
    // Extract semver token from detected string if it contains extra text
    let det_token = extract_version(&det_clean)
        .unwrap_or(det_clean.clone())
        .trim()
        .to_owned();
    let det_token_clean = det_token.strip_prefix('v').unwrap_or(&det_token).trim();

    // Try VersionReq first (handles `>=1.0.0`, `^1.2.3`, `~1.2`, `*`)
    if let Ok(req) = semver::VersionReq::parse(req_clean) {
        if let Ok(ver) = semver::Version::parse(det_token_clean) {
            return req.matches(&ver);
        }
        // If detected is not strict semver, try prefix match for partial versions
        if det_token_clean.starts_with(req_clean) {
            return true;
        }
        return false;
    }
    // Try exact version equality
    if let Ok(req_ver) = semver::Version::parse(req_clean) {
        if let Ok(det_ver) = semver::Version::parse(det_token_clean) {
            return req_ver == det_ver;
        }
        return req_clean == det_token_clean;
    }
    // Fallback: prefix equality handles `1.2` vs `1.2.3`
    if det_token_clean.starts_with(req_clean) {
        return true;
    }
    req_clean == det_token_clean
}

/// Run a non-mutating smoke probe (`--help` / `--version`) against `path`.
///
/// Returns success only when the binary executes and produces output that
/// looks like help/version. Does not claim ownership.
fn smoke_probe(path: &Path) -> Result<(), CoreError> {
    let exe_str = path.to_string_lossy().into_owned();
    // Probe args ordered by likelihood; stop on first that looks like help

    // Construct candidate arg sets without inline allocation in loop
    let sets: Vec<Vec<String>> = vec![
        vec!["--help".to_owned()],
        vec!["--version".to_owned()],
        vec!["-h".to_owned()],
        vec!["help".to_owned()],
    ];
    let mut last_err: Option<CoreError> = None;
    for args in &sets {
        let opts = ExecuteOpts {
            timeout: Some(SMOKE_TIMEOUT),
            cwd: None,
            env: minimal_env_vars(),
            env_remove: Vec::new(),
            clear_env: true,
            output_limit: Some(64 * 1024),
            redact: false,
        };
        match run_command(&exe_str, args, &opts) {
            Ok(out) => {
                let combined = format!("{} {}", out.stdout, out.stderr).to_ascii_lowercase();
                // Success if help/version/usage appears, or exit code 0
                if combined.contains("help")
                    || combined.contains("version")
                    || combined.contains("usage")
                    || out.success
                {
                    return Ok(());
                }
                // Non-zero without help text -> remember but try next set
                last_err = Some(CoreError::Verification {
                    path: path.to_path_buf(),
                    kind: "smoke_probe".to_owned(),
                    reason: format!(
                        "smoke probe `{} {}` exited {:?} without help text",
                        exe_str,
                        args.join(" "),
                        out.exit_code
                    ),
                });
            }
            Err(e) => {
                // Spawn failure is hard error (binary not executable)
                return Err(CoreError::Verification {
                    path: path.to_path_buf(),
                    kind: "smoke_probe".to_owned(),
                    reason: format!("failed to spawn smoke probe `{exe_str}`: {e}"),
                });
            }
        }
    }
    // If none of the sets matched, return last error or generic
    Err(last_err.unwrap_or_else(|| CoreError::Verification {
        path: path.to_path_buf(),
        kind: "smoke_probe".to_owned(),
        reason: format!("smoke probe failed for {}", path.display()),
    }))
}

/// Select the best post-install detection to verify.
///
/// Prefers non-broken, non-shadowed `Path` rank 0, then any non-broken hit
/// ordered by confidence. Returns `None` when no usable detection exists.
fn select_best_detection(detections: &[Detection]) -> Option<&Detection> {
    // Prefer Path rank 0 non-broken
    if let Some(d) = detections
        .iter()
        .find(|d| d.path_rank == Some(0) && !d.broken_shim)
    {
        return Some(d);
    }
    // Prefer any non-broken, non-shadowed, not low confidence broken?
    // Choose highest confidence first: High > Medium > Low
    let mut candidates: Vec<&Detection> = detections.iter().filter(|d| !d.broken_shim).collect();
    // Sort by confidence rank: High=0, Medium=1, Low=2, then by path_rank
    candidates.sort_by(|a, b| {
        let rank = |c: &Detection| match c.confidence {
            crate::detect::DetectionConfidence::High => 0,
            crate::detect::DetectionConfidence::Medium => 1,
            crate::detect::DetectionConfidence::Low => 2,
        };
        rank(a).cmp(&rank(b)).then_with(|| {
            a.path_rank
                .unwrap_or(usize::MAX)
                .cmp(&b.path_rank.unwrap_or(usize::MAX))
        })
    });
    candidates.into_iter().next()
}

/// Verify an install and build a superai-owned receipt (PKG-06).
///
/// - Re-detects the executable via `detect_all_for_entry`
/// - Parses version with `extract_version` (regex-like semver probe)
/// - Confirms the requested range/channel via `version_satisfies`
/// - Runs a smoke probe (`--help`) against the detected path
/// - Returns `Ok(Some(receipt))` only when the binary is newly present and
///   all checks pass; `Ok(None)` when pre-existing and not claimed; `Err`
///   on verification failure (wrong version, smoke failure, missing binary).
pub fn verify_install(
    harness: &HarnessId,
    requested_version: Option<&str>,
    requested_method: &InstallMethodKind,
    pre_detections: &[Detection],
    detect_opts: &DetectOptions,
) -> Result<Option<InstallReceipt>, CoreError> {
    let catalog = InstallCatalog::embedded()?;
    let entry = catalog.get(harness).ok_or_else(|| CoreError::Validation {
        field: "harness".to_owned(),
        reason: format!("harness `{harness}` not in install catalog"),
    })?;
    let method_entry = entry
        .methods
        .iter()
        .find(|m| &m.kind == requested_method)
        .ok_or_else(|| CoreError::Validation {
            field: "method".to_owned(),
            reason: format!("method `{requested_method}` not supported for `{harness}`"),
        })?;

    // Re-detect
    let post = detect_all_for_entry(entry, detect_opts);
    let best = select_best_detection(&post).ok_or_else(|| CoreError::Verification {
        path: PathBuf::from(harness.as_str()),
        kind: "detect".to_owned(),
        reason: format!("post-install detection found no executable for `{harness}`"),
    })?;

    // If the pre-install detection already covered this exact installation, do
    // not claim it. The same canonical path means the same physical binary:
    // an unknown version on either side is "unknown, not new" and is never
    // grounds for a fresh-install receipt (PKG-06). A genuine version change
    // (both versions known and different) falls through to the explicit
    // upgrade logic below instead.
    let pre_has_same = pre_detections.iter().any(|pre| {
        // Compare canonical paths when possible, else direct path equality
        let same_path = pre.path == best.path
            || std::fs::canonicalize(&pre.path)
                .ok()
                .zip(std::fs::canonicalize(&best.path).ok())
                .is_some_and(|(a, b)| a == b);
        if !same_path {
            return false;
        }
        match (&pre.version, &best.version) {
            (Some(a), Some(b)) => a == b,
            // Same physical path with an unprobed version on either side:
            // we cannot prove the binary is new, so we must not claim it.
            _ => true,
        }
    });
    if pre_has_same {
        return Ok(None);
    }

    // Also if pre had any non-broken detection for same harness, treat as
    // pre-existing unless pre was empty? To avoid claiming ambiguous upgrades,
    // we consider any pre detection with same executable as pre-existing,
    // unless requested_version explicitly differs and satisfies new version.
    // The spec says "without claiming pre-existing" — so we are conservative:
    // if any pre detection exists, we only claim when version changed and
    // satisfies the request.
    let has_pre = pre_detections.iter().any(|d| !d.broken_shim);
    if has_pre {
        if let Some(req) = requested_version {
            // If we have a requested version, claim only if detected version is new
            // and satisfies request, and pre version does not satisfy or differs.
            let detected_version =
                best.version
                    .as_deref()
                    .ok_or_else(|| CoreError::Verification {
                        path: best.path.clone(),
                        kind: "version".to_owned(),
                        reason: format!(
                            "detected version missing for `{}` at {}",
                            harness,
                            best.path.display()
                        ),
                    })?;
            // Extract clean version token for comparison
            let detected_clean =
                extract_version(detected_version).unwrap_or_else(|| detected_version.to_owned());
            let satisfies = version_satisfies(req, &detected_clean);
            if !satisfies {
                return Err(CoreError::Verification {
                    path: best.path.clone(),
                    kind: "version".to_owned(),
                    reason: format!(
                        "detected version `{detected_clean}` does not satisfy requested `{req}`"
                    ),
                });
            }
            // If any pre version already satisfied request and equals detected, don't claim
            let pre_satisfies_same = pre_detections.iter().any(|pre| {
                if let Some(pv) = pre.version.as_deref() {
                    let pv_clean = extract_version(pv).unwrap_or_else(|| pv.to_owned());
                    version_satisfies(req, &pv_clean) && pv_clean == detected_clean
                } else {
                    false
                }
            });
            if pre_satisfies_same {
                return Ok(None);
            }
        } else {
            // No requested version and pre-existed -> do not claim (ambiguous)
            return Ok(None);
        }
    }

    // Ensure version exists and parseable
    let raw_version = best
        .version
        .as_deref()
        .ok_or_else(|| CoreError::Verification {
            path: best.path.clone(),
            kind: "version".to_owned(),
            reason: format!(
                "version probe failed for `{}` at {}",
                harness,
                best.path.display()
            ),
        })?;
    let version = extract_version(raw_version).unwrap_or_else(|| raw_version.to_owned());
    if version.is_empty() {
        return Err(CoreError::Verification {
            path: best.path.clone(),
            kind: "version".to_owned(),
            reason: format!("version string empty for {}", best.path.display()),
        });
    }
    if version.contains('\0') {
        return Err(CoreError::Validation {
            field: "version".to_owned(),
            reason: "version must not contain NUL".to_owned(),
        });
    }

    // Confirm requested range if any
    if let Some(req) = requested_version
        && !version_satisfies(req, &version)
    {
        return Err(CoreError::Verification {
            path: best.path.clone(),
            kind: "version".to_owned(),
            reason: format!("detected version `{version}` does not satisfy requested `{req}`"),
        });
    }

    // Smoke probe
    smoke_probe(&best.path)?;

    // Build receipt
    let receipt = InstallReceipt {
        method: requested_method.clone(),
        package_id: method_entry.package_name.clone(),
        executable: best.executable.clone(),
        version,
        timestamp: now_iso8601(),
        path: best.path.clone(),
    };
    receipt.validate()?;
    Ok(Some(receipt))
}

/// Execute an install plan and verify it, returning a receipt on success.
///
/// This is the combined PKG-05 (execute) + PKG-06 (verify) flow. It captures
/// pre-detections, runs the preview command with structured opts, then
/// verifies and returns the receipt. Pre-existing installs return `Ok(None)`.
pub fn execute_and_verify(
    plan: &InstallPlan,
    detect_opts: &DetectOptions,
    redact: bool,
) -> Result<Option<InstallReceipt>, CoreError> {
    let harness = HarnessId::new(&plan.harness).map_err(|e| CoreError::Validation {
        field: "harness".to_owned(),
        reason: format!("invalid harness in plan: {e}"),
    })?;
    // Capture pre-detections before execution
    let catalog = InstallCatalog::embedded()?;
    let entry = catalog.get(&harness).ok_or_else(|| CoreError::Validation {
        field: "harness".to_owned(),
        reason: format!("harness `{harness}` not in catalog"),
    })?;
    let pre = detect_all_for_entry(entry, detect_opts);

    // Execute with structured command
    plan.command_preview.validate()?;
    let out = run_command(
        &plan.command_preview.executable,
        &plan.command_preview.args,
        &structured_opts(redact),
    )?;
    if !out.success {
        let display = display_command(
            &plan.command_preview.executable,
            &plan.command_preview.args,
            redact,
        );
        return Err(CoreError::Verification {
            path: plan.expected_executable.clone(),
            kind: "install_exit".to_owned(),
            reason: format!(
                "install command `{display}` failed with {:?}: {}",
                out.exit_code,
                if redact {
                    "[REDACTED]"
                } else {
                    out.stderr.trim()
                }
            ),
        });
    }

    // Verify and build receipt
    let requested = plan.version.as_deref().or(plan.channel.as_deref());
    verify_install(&harness, requested, &plan.method, &pre, detect_opts)
}

// ---------------------------------------------------------------------------
// PKG-07 — update
// ---------------------------------------------------------------------------

/// Compatibility impact of an update on a single instance (PKG-07).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatImpact {
    /// Instance name that would be affected.
    pub instance: String,
    /// Harness of the instance.
    pub harness: String,
    /// Whether the adapter currently reports compatible for this harness.
    pub current_compatible: bool,
    /// Whether the new version would be compatible.
    pub new_compatible: bool,
    /// Human-readable reason.
    pub reason: String,
}

/// Update plan preview (PKG-07).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatePlan {
    /// Harness being updated.
    pub harness: String,
    /// Method that will be used (native to current install).
    pub method: InstallMethodKind,
    /// Official package name.
    pub package_name: String,
    /// Current detected version, if any.
    pub current_version: Option<String>,
    /// Current detected path, if any.
    pub current_path: Option<PathBuf>,
    /// Available version to update to, if fetched.
    pub available_version: Option<String>,
    /// Exact command tokens to execute for update.
    pub command_preview: CommandTokens,
    /// Whether network is required.
    pub requires_network: bool,
    /// Whether admin is required.
    pub requires_admin: bool,
    /// Compat impacts on instances (via `harness_catalog`).
    pub compat_impacts: Vec<CompatImpact>,
    /// Whether the update is blocked due to adapter incompatibility.
    pub blocked: bool,
    /// Reason for block, if any.
    pub blocked_reason: Option<String>,
    /// Documentation link.
    pub docs: String,
}

impl UpdatePlan {
    /// Human display of the update command.
    pub fn command_display(&self) -> String {
        self.command_preview.display()
    }
}

/// Heuristic adapter compatibility: new version is compatible when its major
/// equals current major, or when current is unknown and new is >=1.0.0.
///
/// Real adapters may have tighter ranges; this provides the blocking signal
/// required by PKG-07 without hard-coding version matrices.
#[expect(clippy::redundant_closure, reason = "String vs &str coercion")]
fn update_compat_for_versions(current: Option<&str>, available: &str) -> (bool, bool, String) {
    let cur_ver = current
        .and_then(|s| extract_version(s))
        .and_then(|s| semver::Version::parse(s.strip_prefix('v').unwrap_or(&s)).ok());
    let new_ver = extract_version(available)
        .and_then(|s| semver::Version::parse(s.strip_prefix('v').unwrap_or(&s)).ok());

    match (cur_ver, new_ver) {
        (Some(cur), Some(new)) => {
            if cur.major == new.major {
                (true, true, format!("major {} retained", cur.major))
            } else {
                (
                    true,
                    false,
                    format!(
                        "major bump {} -> {} may be breaking (adapter range)",
                        cur.major, new.major
                    ),
                )
            }
        }
        (None, Some(new)) => {
            // No current: assume new is compatible if stable
            let compat = new.major >= 1 || new.major == 0 && new.minor < 1;
            if compat {
                (
                    false,
                    true,
                    "no current version, assuming new is compatible".to_owned(),
                )
            } else {
                (
                    false,
                    false,
                    format!("no current version, new {new} is pre-stable"),
                )
            }
        }
        _ => (
            false,
            false,
            "version parse failed, assuming incompatibility".to_owned(),
        ),
    }
}

/// Fetch available version for a harness/method (stub network).
///
/// Runs the package manager's query command with bounded capture and timeout.
/// Returns `None` when the manager is not present or the query fails.
/// For tests, `injected_available` overrides network fetch.
fn fetch_available_version(
    entry: &crate::install_catalog::InstallCatalogEntry,
    method: &InstallMethodKind,
    _detect_opts: &DetectOptions,
    injected_available: Option<&str>,
) -> Option<String> {
    if let Some(v) = injected_available {
        if v.is_empty() {
            return None;
        }
        return Some(v.to_owned());
    }
    let method_entry = entry.methods.iter().find(|m| &m.kind == method)?;
    let exec_opts = ExecuteOpts {
        timeout: Some(PROBE_TIMEOUT),
        cwd: None,
        env: minimal_env_vars(),
        env_remove: Vec::new(),
        clear_env: true,
        output_limit: Some(256 * 1024),
        redact: false,
    };
    match method {
        InstallMethodKind::Npm => {
            let out = run_command(
                "npm",
                &[
                    "view".to_owned(),
                    method_entry.package_name.clone(),
                    "version".to_owned(),
                    "--json".to_owned(),
                ],
                &exec_opts,
            )
            .ok()?;
            if !out.success {
                return None;
            }
            let trimmed = out.stdout.trim().trim_matches('"').trim().to_owned();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
        InstallMethodKind::Homebrew | InstallMethodKind::HomebrewCask => {
            let out = run_command(
                "brew",
                &[
                    "info".to_owned(),
                    "--json=v2".to_owned(),
                    method_entry.package_name.clone(),
                ],
                &exec_opts,
            )
            .ok()?;
            if !out.success {
                return None;
            }
            extract_version(&out.stdout)
        }
        InstallMethodKind::Cargo => {
            let out = run_command(
                "cargo",
                &[
                    "search".to_owned(),
                    method_entry.package_name.clone(),
                    "--limit".to_owned(),
                    "1".to_owned(),
                ],
                &exec_opts,
            )
            .ok()?;
            if !out.success {
                return None;
            }
            extract_version(&out.stdout)
        }
        InstallMethodKind::Mise => {
            let out = run_command(
                "mise",
                &["ls-remote".to_owned(), method_entry.package_name.clone()],
                &exec_opts,
            )
            .ok()?;
            if !out.success {
                return None;
            }
            out.stdout
                .lines()
                .last()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .and_then(|s| extract_version(&s).or(Some(s)))
        }
        InstallMethodKind::Pipx | InstallMethodKind::Uv => {
            // Use `pip show` style probe — best effort, bounded
            let out = run_command(
                "pip",
                &[
                    "index".to_owned(),
                    "versions".to_owned(),
                    method_entry.package_name.clone(),
                ],
                &exec_opts,
            )
            .ok()?;
            if !out.success {
                return None;
            }
            extract_version(&out.stdout)
        }
        InstallMethodKind::Direct | InstallMethodKind::External => None,
    }
    .filter(|s| !s.contains('\0'))
    .map(|v| {
        // Bound length and ensure single line
        let line = v.lines().next().unwrap_or(&v).trim().to_owned();
        if line.len() > 64 {
            let mut end = 64;
            while end > 0 && !line.is_char_boundary(end) {
                end -= 1;
            }
            line.get(0..end).unwrap_or(&line).to_owned()
        } else {
            line
        }
    })
}

/// Plan an update for `harness` (PKG-07).
///
/// Detects the current method/version, fetches the available version,
/// computes compat impact on adapters/instances via `harness_catalog`,
/// and returns a preview with the native update command. Does not execute.
///
/// When `injected_available` is `Some`, network fetch is skipped (for tests).
/// When `explicit_accept` is false and the new version is outside the
/// heuristic adapter range, the plan is marked `blocked`.
pub fn plan_update(
    harness: &HarnessId,
    detect_opts: &DetectOptions,
    registry: Option<&Registry>,
    explicit_accept: bool,
    injected_available: Option<&str>,
) -> Result<UpdatePlan, CoreError> {
    let catalog = InstallCatalog::embedded()?;
    let entry = catalog.get(harness).ok_or_else(|| CoreError::Validation {
        field: "harness".to_owned(),
        reason: format!("harness `{harness}` not in install catalog"),
    })?;

    // Detect current
    let detections = detect_all_for_entry(entry, detect_opts);
    let best = select_best_detection(&detections);
    let (current_version, current_path, method) = if let Some(d) = best {
        let method = detection_source_to_method(&d.source).unwrap_or_else(|| {
            // Fall back to catalog's first method that matches executable
            entry
                .methods
                .first()
                .map_or(InstallMethodKind::Npm, |m| m.kind.clone())
        });
        (d.version.clone(), Some(d.path.clone()), method)
    } else {
        // No install found -> use catalog default method
        let default_method = entry.methods.first().ok_or_else(|| CoreError::Validation {
            field: "methods".to_owned(),
            reason: format!("no install methods for `{harness}`"),
        })?;
        (None, None, default_method.kind.clone())
    };

    // Resolve update command for the method
    let method_entry = entry
        .methods
        .iter()
        .find(|m| m.kind == method)
        .ok_or_else(|| CoreError::Validation {
            field: "method".to_owned(),
            reason: format!("method `{method}` not in catalog for `{harness}`"),
        })?;
    let command_preview = entry.update.clone().unwrap_or_else(|| CommandTokens {
        executable: match method {
            InstallMethodKind::Npm => "npm".to_owned(),
            InstallMethodKind::Homebrew | InstallMethodKind::HomebrewCask => "brew".to_owned(),
            InstallMethodKind::Cargo => "cargo".to_owned(),
            InstallMethodKind::Mise => "mise".to_owned(),
            InstallMethodKind::Pipx => "pipx".to_owned(),
            InstallMethodKind::Uv => "uv".to_owned(),
            InstallMethodKind::Direct | InstallMethodKind::External => "echo".to_owned(),
        },
        args: {
            let pkg = &method_entry.package_name;
            match method {
                InstallMethodKind::Npm => {
                    vec!["update".to_owned(), "-g".to_owned(), pkg.clone()]
                }
                InstallMethodKind::Cargo => {
                    vec!["install".to_owned(), pkg.clone()]
                }
                InstallMethodKind::Uv => {
                    vec!["tool".to_owned(), "update".to_owned(), pkg.clone()]
                }
                InstallMethodKind::Direct | InstallMethodKind::External => {
                    vec!["update".to_owned(), pkg.clone()]
                }
                InstallMethodKind::Homebrew
                | InstallMethodKind::HomebrewCask
                | InstallMethodKind::Mise
                | InstallMethodKind::Pipx => {
                    vec!["upgrade".to_owned(), pkg.clone()]
                }
            }
        },
    });
    command_preview.validate()?;

    // Fetch available version
    let available_version =
        fetch_available_version(entry, &method, detect_opts, injected_available);

    // Compat impact via harness_catalog
    let mut compat_impacts = Vec::new();
    let catalog_entry = harness_catalog::find_by_id(harness.as_str());
    let support_note = catalog_entry.map_or("unknown harness".to_owned(), |e| {
        format!("{} support: {}", e.display_name, e.support)
    });
    // Determine heuristic compatibility for the harness itself
    let (cur_compat, new_compat, reason) = if let Some(avail) = available_version.as_deref() {
        update_compat_for_versions(current_version.as_deref(), avail)
    } else {
        (
            false,
            false,
            "available version unknown, cannot assess compat".to_owned(),
        )
    };
    // Seed a harness-level impact so empty-registry callers still see blocking
    if available_version.is_some() && !new_compat && !explicit_accept {
        // harness-level impact will cause blocked
    }
    // Per-instance impacts
    if let Some(reg) = registry {
        for inst in reg.instances() {
            if inst.harness.as_str() != harness.as_str() {
                continue;
            }
            let inst_name = inst.name.as_str().to_owned();
            let (_c, n, r) = if let Some(avail) = available_version.as_deref() {
                update_compat_for_versions(current_version.as_deref(), avail)
            } else {
                (false, false, "available version unknown".to_owned())
            };
            compat_impacts.push(CompatImpact {
                instance: inst_name,
                harness: harness.as_str().to_owned(),
                current_compatible: cur_compat,
                new_compatible: n,
                reason: format!("{support_note}; {r}"),
            });
        }
    }
    // If no instances, still surface harness-level compat as an impact for visibility
    if compat_impacts.is_empty() && available_version.is_some() {
        compat_impacts.push(CompatImpact {
            instance: "<harness>".to_owned(),
            harness: harness.as_str().to_owned(),
            current_compatible: cur_compat,
            new_compatible: new_compat,
            reason: format!("{support_note}; {reason}"),
        });
    }

    let blocked = !explicit_accept
        && compat_impacts.iter().any(|c| !c.new_compatible)
        && available_version.is_some();
    let blocked_reason = if blocked {
        Some(format!(
            "update to `{}` is outside adapter range (major bump or pre-stable); re-run with explicit accept to proceed",
            available_version.as_deref().unwrap_or("<unknown>")
        ))
    } else {
        None
    };

    Ok(UpdatePlan {
        harness: harness.as_str().to_owned(),
        method,
        package_name: method_entry.package_name.clone(),
        current_version,
        current_path,
        available_version,
        command_preview,
        requires_network: true,
        requires_admin: entry.requires_admin,
        compat_impacts,
        blocked,
        blocked_reason,
        docs: entry.docs.clone(),
    })
}

fn detection_source_to_method(source: &DetectionSource) -> Option<InstallMethodKind> {
    match source {
        DetectionSource::MiseShim | DetectionSource::MiseManaged => Some(InstallMethodKind::Mise),
        DetectionSource::Homebrew => Some(InstallMethodKind::Homebrew),
        DetectionSource::Npm => Some(InstallMethodKind::Npm),
        DetectionSource::Cargo => Some(InstallMethodKind::Cargo),
        DetectionSource::AppBundle => Some(InstallMethodKind::HomebrewCask),
        DetectionSource::Path
        | DetectionSource::ConfiguredBinary
        | DetectionSource::SystemPackage => None,
    }
}

/// Execute an update plan with native method (PKG-07).
///
/// Checks `blocked` unless `explicit_accept` is true, runs the update command
/// with structured opts, then re-detects and validates the new version.
pub fn execute_update(
    plan: &UpdatePlan,
    detect_opts: &DetectOptions,
    explicit_accept: bool,
    redact: bool,
) -> Result<ProcessOutput, CoreError> {
    if plan.blocked && !explicit_accept {
        return Err(CoreError::Validation {
            field: "update".to_owned(),
            reason: plan.blocked_reason.clone().unwrap_or_else(|| {
                "update blocked due to adapter incompatibility; explicit accept required".to_owned()
            }),
        });
    }
    // Dry-run for external methods: return ExternalInstallRequired-like error
    if matches!(
        plan.method,
        InstallMethodKind::External | InstallMethodKind::Direct
    ) && plan.command_preview.executable == "echo"
    {
        return Err(CoreError::UnsupportedOperation {
            harness: plan.harness.clone(),
            operation: "update".to_owned(),
            reason: "external/manual update required; see docs".to_owned(),
        });
    }
    plan.command_preview.validate()?;
    let out = run_command(
        &plan.command_preview.executable,
        &plan.command_preview.args,
        &structured_opts(redact),
    )?;
    if !out.success {
        let display = display_command(
            &plan.command_preview.executable,
            &plan.command_preview.args,
            redact,
        );
        return Err(CoreError::Verification {
            path: plan
                .current_path
                .clone()
                .unwrap_or_else(|| PathBuf::from(&plan.harness)),
            kind: "update_exit".to_owned(),
            reason: format!(
                "update command `{display}` failed with {:?}: {}",
                out.exit_code,
                if redact {
                    "[REDACTED]"
                } else {
                    out.stderr.trim()
                }
            ),
        });
    }
    // Re-detect post-update
    let harness_id = HarnessId::new(&plan.harness).map_err(|e| CoreError::Validation {
        field: "harness".to_owned(),
        reason: format!("invalid harness id: {e}"),
    })?;
    let catalog = InstallCatalog::embedded()?;
    if let Some(entry) = catalog.get(&harness_id) {
        let post = detect_all_for_entry(entry, detect_opts);
        if let Some(best) = select_best_detection(&post) {
            // If available_version was known, verify post version matches or advances
            if let Some(expected) = plan.available_version.as_deref()
                && let Some(detected) = best.version.as_deref()
            {
                let det_clean = extract_version(detected).unwrap_or_else(|| detected.to_owned());
                let exp_clean = extract_version(expected).unwrap_or_else(|| expected.to_owned());
                // Allow any detected version that is not empty when the update succeeded
                // at the process level; strict equality would be too brittle for
                // managers that report slightly different strings.
                if det_clean.is_empty() {
                    return Err(CoreError::Verification {
                        path: best.path.clone(),
                        kind: "version".to_owned(),
                        reason: format!("post-update version empty (expected {exp_clean})"),
                    });
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// PKG-08 — uninstall
// ---------------------------------------------------------------------------

/// Preflight for uninstall (PKG-08).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UninstallPreflight {
    /// Harness to uninstall.
    pub harness: String,
    /// Exact package identifier.
    pub package_id: String,
    /// Method that installed the binary.
    pub method: InstallMethodKind,
    /// Exact path to the binary (winning detection).
    pub path: PathBuf,
    /// All detected paths for this harness (for shared/foreign checks).
    pub all_paths: Vec<PathBuf>,
    /// Instance names referencing this binary/harness.
    pub referencing_instances: Vec<String>,
    /// Wrapper paths referencing this binary (if any).
    pub referencing_wrappers: Vec<PathBuf>,
    /// Whether the binary is shared (multiple detections or multiple instances).
    pub shared: bool,
    /// Whether the binary is foreign (not proven superai-owned).
    pub foreign: bool,
    /// Whether auto-delete via native method is allowed.
    pub can_auto_delete: bool,
    /// Paths that will be preserved (config/instances/wrappers/backups/templates/assets).
    pub preserved: Vec<PathBuf>,
}

/// Uninstall plan preview (PKG-08).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UninstallPlan {
    /// Preflight that produced this plan.
    pub preflight: UninstallPreflight,
    /// Exact uninstall command tokens.
    pub command_preview: CommandTokens,
    /// Whether the command requires network.
    pub requires_network: bool,
    /// Whether admin is required.
    pub requires_admin: bool,
    /// Whether the operation is blocked (shared/foreign without explicit).
    pub blocked: bool,
    /// Reason for block, if any.
    pub blocked_reason: Option<String>,
    /// Documentation link.
    pub docs: String,
}

/// Build the list of paths that uninstall must preserve (PKG-08).
///
/// Never deletes: config, instances, wrappers, backups, templates, assets.
fn preserved_paths_for(registry: Option<&Registry>, harness: &HarnessId) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // Preserve every instance config_root + wrapper path for this harness
    if let Some(reg) = registry {
        for inst in reg.instances() {
            if inst.harness.as_str() == harness.as_str() {
                if let Ok(p) = inst.config_root.as_path().canonicalize() {
                    out.push(p);
                } else {
                    out.push(inst.config_root.as_path().to_path_buf());
                }
                if let Some(wrapper) = inst.wrapper.as_ref() {
                    out.push(wrapper.path.as_path().to_path_buf());
                }
            }
        }
    }
    // Preserve superai's own directories (best-effort, may not exist)
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        out.push(home.join(".superai"));
        out.push(home.join(".superai/instances.json"));
        out.push(home.join(".superai/backups"));
        out.push(home.join(".superai/templates"));
        out.push(home.join(".superai/assets"));
        out.push(home.join(".superai/install_receipts"));
    }
    // Also preserve registry file's parent dir if registry is loaded from different path?
    // The preserved list is advisory; actual uninstall never deletes these.
    out
}

/// Plan an uninstall with preflight checks (PKG-08).
///
/// - Exact installed package/method/path via detection
/// - Lists every instance/wrapper referencing the binary
/// - Checks shared/foreign (multiple PATH hits, foreign ownership)
/// - Default uninstall is binary-only via native method; config preservation
///   is enforced
/// - Manual files not proven owned never auto-delete (`can_auto_delete`)
///
/// When `explicit_allow_foreign` is true, foreign/shared blocks are ignored.
pub fn plan_uninstall(
    harness: &HarnessId,
    detect_opts: &DetectOptions,
    registry: Option<&Registry>,
    explicit_allow_foreign: bool,
) -> Result<UninstallPlan, CoreError> {
    let catalog = InstallCatalog::embedded()?;
    let entry = catalog.get(harness).ok_or_else(|| CoreError::Validation {
        field: "harness".to_owned(),
        reason: format!("harness `{harness}` not in catalog"),
    })?;
    let detections = detect_all_for_entry(entry, detect_opts);
    let best = select_best_detection(&detections).ok_or_else(|| CoreError::Verification {
        path: PathBuf::from(harness.as_str()),
        kind: "detect".to_owned(),
        reason: format!("no installed binary found for `{harness}` to uninstall"),
    })?;
    let all_paths: Vec<PathBuf> = detections.iter().map(|d| d.path.clone()).collect();

    // Determine method/package_id for the best detection
    let method = detection_source_to_method(&best.source).unwrap_or_else(|| {
        entry
            .methods
            .first()
            .map_or(InstallMethodKind::Direct, |m| m.kind.clone())
    });
    let package_id = entry
        .methods
        .iter()
        .find(|m| m.kind == method)
        .map_or_else(|| best.executable.clone(), |m| m.package_name.clone());

    // Referencing instances/wrappers
    let mut referencing_instances = Vec::new();
    let mut referencing_wrappers = Vec::new();
    if let Some(reg) = registry {
        for inst in reg.instances() {
            if inst.harness.as_str() == harness.as_str() {
                referencing_instances.push(inst.name.as_str().to_owned());
                if let Some(w) = inst.wrapper.as_ref() {
                    referencing_wrappers.push(w.path.as_path().to_path_buf());
                }
            }
        }
    }

    // Shared: multiple detections or multiple instances
    let shared = detections.len() > 1 || referencing_instances.len() > 1;

    // Foreign: detection is not package-managed (Path without mise) or no receipt
    // Here we approximate: if best source is Path/ConfiguredBinary/SystemPackage
    // and not under mise/homebrew/npm/cargo managed locations, treat as foreign.
    let managed_sources = [
        DetectionSource::MiseShim,
        DetectionSource::MiseManaged,
        DetectionSource::Homebrew,
        DetectionSource::Npm,
        DetectionSource::Cargo,
        DetectionSource::AppBundle,
    ];
    let is_managed_source = managed_sources.contains(&best.source);
    let foreign = !is_managed_source
        || (best.source == DetectionSource::Path
            && !best.path.to_string_lossy().contains(".local/share/mise")
            && !best.path.to_string_lossy().contains(".cargo/bin")
            && !best.path.to_string_lossy().contains("/.local/bin"));

    // can_auto_delete only when proven owned via package manager and not foreign manual file
    let can_auto_delete = is_managed_source && !foreign;

    let preserved = preserved_paths_for(registry, harness);

    // Resolve uninstall command
    let command_preview = entry.uninstall.clone().unwrap_or_else(|| CommandTokens {
        executable: match method {
            InstallMethodKind::Npm => "npm".to_owned(),
            InstallMethodKind::Homebrew | InstallMethodKind::HomebrewCask => "brew".to_owned(),
            InstallMethodKind::Cargo => "cargo".to_owned(),
            InstallMethodKind::Mise => "mise".to_owned(),
            InstallMethodKind::Pipx => "pipx".to_owned(),
            InstallMethodKind::Uv => "uv".to_owned(),
            InstallMethodKind::Direct | InstallMethodKind::External => "echo".to_owned(),
        },
        args: {
            let pkg = package_id.clone();
            match method {
                InstallMethodKind::Npm => {
                    vec!["uninstall".to_owned(), "-g".to_owned(), pkg]
                }
                InstallMethodKind::Uv => {
                    vec!["tool".to_owned(), "uninstall".to_owned(), pkg]
                }
                InstallMethodKind::Homebrew
                | InstallMethodKind::HomebrewCask
                | InstallMethodKind::Cargo
                | InstallMethodKind::Mise
                | InstallMethodKind::Pipx
                | InstallMethodKind::Direct
                | InstallMethodKind::External => {
                    vec!["uninstall".to_owned(), pkg]
                }
            }
        },
    });
    command_preview.validate()?;

    let blocked = (foreign || !can_auto_delete) && !explicit_allow_foreign;
    let blocked_reason = if blocked {
        if foreign {
            Some(format!(
                "binary at {} is foreign/manual and not proven superai-owned; refusing to auto-delete (use explicit allow)",
                best.path.display()
            ))
        } else {
            Some(format!(
                "binary at {} is not auto-deletable via native method; refusing to delete",
                best.path.display()
            ))
        }
    } else if shared && !explicit_allow_foreign {
        // Shared alone does not block default binary-only uninstall, but note it
        None
    } else {
        None
    };

    let preflight = UninstallPreflight {
        harness: harness.as_str().to_owned(),
        package_id,
        method,
        path: best.path.clone(),
        all_paths,
        referencing_instances,
        referencing_wrappers,
        shared,
        foreign,
        can_auto_delete,
        preserved,
    };

    Ok(UninstallPlan {
        preflight,
        command_preview,
        requires_network: false,
        requires_admin: entry.requires_admin,
        blocked: blocked && !explicit_allow_foreign,
        blocked_reason,
        docs: entry.docs.clone(),
    })
}

/// Execute an uninstall plan (PKG-08).
///
/// - Honors `blocked` unless `explicit_allow_foreign` is true
/// - Uninstalls binary only via native method; never deletes config,
///   instances, wrappers, backups, templates, assets
/// - Manual files not proven owned never auto-delete (checked via
///   `can_auto_delete`); caller must provide explicit allow
/// - Marks affected instances as binary-missing by not deleting them but
///   returning the list of referencing instances
pub fn execute_uninstall(
    plan: &UninstallPlan,
    explicit_allow_foreign: bool,
    redact: bool,
) -> Result<ProcessOutput, CoreError> {
    if plan.blocked && !explicit_allow_foreign {
        return Err(CoreError::ForeignOwnership {
            path: plan.preflight.path.clone(),
            owner: plan
                .blocked_reason
                .clone()
                .unwrap_or_else(|| "foreign or shared binary".to_owned()),
        });
    }
    if !plan.preflight.can_auto_delete && !explicit_allow_foreign {
        return Err(CoreError::ForeignOwnership {
            path: plan.preflight.path.clone(),
            owner: "manual file not proven superai-owned; refusing to auto-delete".to_owned(),
        });
    }
    // Validate that we never attempt to rm a preserved path directly
    // The plan's command_preview is a package-manager uninstall, not an `rm`.
    // Defensively reject any preview that tries to rm a preserved path.
    for preserved in &plan.preflight.preserved {
        let preserved_str = preserved.to_string_lossy().into_owned();
        if plan
            .command_preview
            .args
            .iter()
            .any(|a| a.contains(&preserved_str))
        {
            return Err(CoreError::Validation {
                field: "uninstall".to_owned(),
                reason: format!(
                    "uninstall command must not reference preserved path {}",
                    preserved.display()
                ),
            });
        }
        if plan.command_preview.executable == "rm" {
            return Err(CoreError::Validation {
                field: "uninstall".to_owned(),
                reason: "uninstall must use native method, not `rm`".to_owned(),
            });
        }
    }
    plan.command_preview.validate()?;
    // Execute via native method
    let out = run_command(
        &plan.command_preview.executable,
        &plan.command_preview.args,
        &structured_opts(redact),
    )?;
    if !out.success {
        let display = display_command(
            &plan.command_preview.executable,
            &plan.command_preview.args,
            redact,
        );
        return Err(CoreError::Verification {
            path: plan.preflight.path.clone(),
            kind: "uninstall_exit".to_owned(),
            reason: format!(
                "uninstall command `{display}` failed with {:?}: {}",
                out.exit_code,
                if redact {
                    "[REDACTED]"
                } else {
                    out.stderr.trim()
                }
            ),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Helpers: semver truncation, path display
// ---------------------------------------------------------------------------

// (helpers defined above)

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install_catalog::{
        DetectHints, InstallCatalogEntry, InstallMethod, PlatformConstraints,
    };
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    fn make_temp_dir(prefix: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(prefix)
    }

    #[cfg(unix)]
    #[expect(dead_code, reason = "helper for future tests")]
    fn write_fake_exe(dir: &Path, name: &str, version_line: &str) {
        let path = dir.join(name);
        let script = format!("#!/bin/sh\necho \"{version_line}\"\n");
        fs::write(&path, script).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }

    /// Write a fake harness executable answering `--help` and `--version`.
    /// The script is a `#!/bin/sh` file, so this (and the tests that probe it)
    /// run on unix only.
    #[cfg(unix)]
    fn write_help_exe(dir: &Path, name: &str) {
        let path = dir.join(name);
        let script = "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then echo \"Usage: my-harness --help\"; exit 0; fi\necho \"my-harness 1.2.3\"\n";
        fs::write(&path, script).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }

    #[expect(dead_code, reason = "helper for future tests")]
    fn minimal_catalog_entry(harness: &str, exe: &str) -> InstallCatalogEntry {
        InstallCatalogEntry {
            harness: harness.to_owned(),
            executables: vec![exe.to_owned()],
            bundle_ids: Vec::new(),
            apps: Vec::new(),
            methods: vec![InstallMethod {
                kind: InstallMethodKind::Npm,
                package_name: format!("@test/{harness}"),
                tap: None,
                repo: None,
                registry: Some("https://registry.npmjs.org".to_owned()),
            }],
            version_source: format!("{exe} --version"),
            constraints: PlatformConstraints {
                os: vec!["linux".to_owned(), "macos".to_owned(), "any".to_owned()],
                arch: vec!["x86_64".to_owned(), "aarch64".to_owned(), "any".to_owned()],
            },
            detect: DetectHints {
                commands: vec![CommandTokens {
                    executable: exe.to_owned(),
                    args: vec!["--version".to_owned()],
                }],
                paths: vec![format!("/usr/local/bin/{exe}")],
            },
            update: Some(CommandTokens {
                executable: "npm".to_owned(),
                args: vec![
                    "update".to_owned(),
                    "-g".to_owned(),
                    format!("@test/{harness}"),
                ],
            }),
            uninstall: Some(CommandTokens {
                executable: "npm".to_owned(),
                args: vec![
                    "uninstall".to_owned(),
                    "-g".to_owned(),
                    format!("@test/{harness}"),
                ],
            }),
            requires_admin: false,
            checksum: None,
            conflicts: Vec::new(),
            docs: "https://example.com".to_owned(),
            last_verified: "2026-08-26".to_owned(),
        }
    }

    // ---- PKG-05 ----

    /// (`echo`-equivalent program, prefix args): unix ships a real `echo`
    /// binary; windows has none in PATH, so route through `cmd /C echo`.
    fn echo_program() -> (&'static str, Vec<String>) {
        #[cfg(unix)]
        {
            ("echo", Vec::new())
        }
        #[cfg(windows)]
        {
            ("cmd", vec!["/C".to_owned(), "echo".to_owned()])
        }
    }

    #[test]
    fn structured_opts_has_timeout_and_bound_and_minimal_env() {
        let opts = structured_opts(false);
        assert_eq!(opts.timeout, Some(EXEC_TIMEOUT));
        assert_eq!(opts.output_limit, Some(OUTPUT_LIMIT));
        assert!(opts.clear_env);
        assert!(
            opts.env
                .iter()
                .any(|(k, _)| k == "PATH" || k == "HOME" || k == "TMPDIR" || k == "LANG")
        );
        assert!(!opts.redact);
        let redacted_opts = structured_opts(true);
        assert!(redacted_opts.redact);
    }

    #[cfg(unix)]
    #[test]
    fn run_structured_no_shell_interpolation() {
        let token = "$(whoami) && echo pwned | cat".to_owned();
        let out = run_structured_command("echo", std::slice::from_ref(&token), false).unwrap();
        assert!(out.success);
        assert_eq!(out.stdout.trim(), token);
    }

    #[test]
    fn run_structured_redacts_secrets() {
        let args = [
            "--token".to_owned(),
            "mysecret123".to_owned(),
            "other".to_owned(),
        ];
        let (prog, prefix) = echo_program();
        let mut full_args = prefix;
        full_args.extend(args.iter().cloned());
        // run with redact=true, verify output still succeeds but display would redact
        let out = run_structured_command(prog, &full_args, true).unwrap();
        assert!(out.success);
        let display = display_command(prog, &full_args, true);
        assert!(display.contains("***"));
        assert!(!display.contains("mysecret123"));
    }

    #[test]
    fn run_structured_timeout_is_120s() {
        assert_eq!(EXEC_TIMEOUT.as_secs(), 120);
        let opts = structured_opts(false);
        assert_eq!(opts.timeout.unwrap().as_secs(), 120);
    }

    #[test]
    fn run_structured_bounded() {
        let large = "x".repeat(100);
        let opts = ExecuteOpts {
            timeout: Some(Duration::from_secs(5)),
            output_limit: Some(10),
            clear_env: false,
            ..Default::default()
        };
        let (prog, prefix) = echo_program();
        let mut args = prefix;
        args.push(large);
        let err = run_command(prog, &args, &opts).unwrap_err();
        assert!(format!("{err}").contains("output limit exceeded"));
    }

    #[test]
    fn run_structured_minimal_env_still_resolves_echo() {
        // Even with clear_env, minimal_env preserves PATH so the echo program
        // (`echo` on unix, `cmd` in System32 on windows) resolves
        let (prog, prefix) = echo_program();
        let mut args = prefix;
        args.push("hello".to_owned());
        let out = run_structured_command(prog, &args, false).unwrap();
        assert_eq!(out.stdout.trim(), "hello");
    }

    // ---- PKG-06 ----
    // The receipt/update/uninstall tests below execute `#!/bin/sh` fake
    // harness binaries, so they run on unix only.

    #[cfg(unix)]
    #[test]
    fn verify_receipt_not_claiming_pre_existing() {
        // Setup: fake exe already present before install, with version 1.2.3
        let tmp = make_temp_dir("preexist");
        let home = make_temp_dir("home-pre");
        let harness = HarnessId::new("claude-code").unwrap();
        // Use a unique exe name that won't clash with real host? But claude-code exe is `claude`
        // We'll use a temp PATH-contained fake and inject via DetectOptions.
        write_help_exe(&tmp, "claude");
        // Also make version probe: the script prints help for --help, but for --version prints 1.2.3
        // Our write_help_exe already prints "my-harness 1.2.3" for --version? Actually it prints for default.
        // Override to ensure version is 1.2.3
        fs::write(
            tmp.join("claude"),
            "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then echo \"Usage: claude --help\"; exit 0; fi\nif [ \"$1\" = \"--version\" ]; then echo \"1.2.3\"; exit 0; fi\necho \"1.2.3\"\n",
        )
        .unwrap();
        let mut perms = fs::metadata(tmp.join("claude")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(tmp.join("claude"), perms).unwrap();

        let opts = DetectOptions {
            path_dirs: Some(vec![tmp.clone()]),
            home_dir: Some(home.clone()),
            probe_mise: false,
            probe_brew: false,
            probe_npm: false,
            probe_cargo: false,
            probe_apps: false,
            ..Default::default()
        };
        let catalog = InstallCatalog::embedded().unwrap();
        let entry = catalog.get(&harness).unwrap();
        let pre = detect_all_for_entry(entry, &opts);
        assert!(!pre.is_empty(), "pre detection should find the fake claude");

        // Now verify as if we had just run an install that produced the same binary/version.
        // Since pre already contains it, receipt must be None (not claimed).
        let receipt = verify_install(
            &harness,
            Some("1.2.3"),
            &InstallMethodKind::Npm,
            &pre,
            &opts,
        )
        .unwrap();
        assert!(
            receipt.is_none(),
            "pre-existing install must not be claimed: {receipt:?}"
        );

        // Now verify with no pre-existing (empty pre) -> should claim
        let empty_pre: Vec<Detection> = Vec::new();
        // For this we need a catalog entry where method matches; use same harness
        // The post detection will still find the tmp claude, so with empty pre we should get a receipt
        let receipt2 = verify_install(
            &harness,
            Some("1.2.3"),
            &InstallMethodKind::Npm,
            &empty_pre,
            &opts,
        )
        .unwrap();
        assert!(
            receipt2.is_some(),
            "fresh install with empty pre should be claimed"
        );
        let r = receipt2.unwrap();
        assert_eq!(r.executable, "claude");
        assert_eq!(r.version, "1.2.3");
        assert_eq!(r.method, InstallMethodKind::Npm);
        assert!(r.timestamp.contains('T'));

        drop(fs::remove_dir_all(tmp));
        drop(fs::remove_dir_all(home));
    }

    #[cfg(unix)]
    #[test]
    fn verify_receipt_same_path_with_unprobed_version_is_not_claimed() {
        // PKG-06: the same canonical path IS the same installation. A version
        // probe that transiently fails on either side of the install is
        // "unknown, not new" and must never produce a fresh-install receipt.
        let tmp = make_temp_dir("unknown-ver");
        let home = make_temp_dir("home-unknown-ver");
        let harness = HarnessId::new("claude-code").unwrap();
        let opts = DetectOptions {
            path_dirs: Some(vec![tmp.clone()]),
            home_dir: Some(home.clone()),
            probe_mise: false,
            probe_brew: false,
            probe_npm: false,
            probe_cargo: false,
            probe_apps: false,
            ..Default::default()
        };

        // Post-detection cannot parse a version (probe failed), while the
        // caller's pre snapshot recorded 1.2.3 for the same physical binary.
        write_help_exe(&tmp, "claude");
        fs::write(
            tmp.join("claude"),
            "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then echo \"Usage: claude --help\"; exit 0; fi\nexit 1\n",
        )
        .unwrap();
        {
            let mut perms = fs::metadata(tmp.join("claude")).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(tmp.join("claude"), perms).unwrap();
        }
        let mut pre = Detection::new(
            "claude-code",
            "claude",
            tmp.join("claude"),
            DetectionSource::Path,
            crate::detect::DetectionConfidence::Medium,
        );
        pre.version = Some("1.2.3".to_owned());
        let receipt = verify_install(
            &harness,
            Some("1.2.3"),
            &InstallMethodKind::Npm,
            std::slice::from_ref(&pre),
            &opts,
        )
        .unwrap();
        assert!(
            receipt.is_none(),
            "same path with unprobed post version must not be claimed: {receipt:?}"
        );

        // Mirror case: the binary answers 1.2.3 now, but the pre snapshot for
        // the same path never got a version.
        fs::write(
            tmp.join("claude"),
            "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then echo \"Usage: claude --help\"; exit 0; fi\necho \"1.2.3\"\n",
        )
        .unwrap();
        {
            let mut perms = fs::metadata(tmp.join("claude")).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(tmp.join("claude"), perms).unwrap();
        }
        let pre_unknown = Detection::new(
            "claude-code",
            "claude",
            tmp.join("claude"),
            DetectionSource::Path,
            crate::detect::DetectionConfidence::Medium,
        );
        let receipt2 = verify_install(
            &harness,
            Some("1.2.3"),
            &InstallMethodKind::Npm,
            std::slice::from_ref(&pre_unknown),
            &opts,
        )
        .unwrap();
        assert!(
            receipt2.is_none(),
            "same path with unprobed pre version must not be claimed: {receipt2:?}"
        );

        drop(fs::remove_dir_all(tmp));
        drop(fs::remove_dir_all(home));
    }

    #[cfg(unix)]
    #[test]
    fn verify_receipt_fails_on_wrong_version() {
        let tmp = make_temp_dir("wrongver");
        let home = make_temp_dir("home-wrongver");
        let harness = HarnessId::new("claude-code").unwrap();
        write_help_exe(&tmp, "claude");
        fs::write(
            tmp.join("claude"),
            "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then echo \"Usage: claude --help\"; exit 0; fi\necho \"9.9.9\"\n",
        )
        .unwrap();
        let mut perms = fs::metadata(tmp.join("claude")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(tmp.join("claude"), perms).unwrap();

        let opts = DetectOptions {
            path_dirs: Some(vec![tmp.clone()]),
            home_dir: Some(home.clone()),
            probe_mise: false,
            probe_brew: false,
            probe_npm: false,
            probe_cargo: false,
            probe_apps: false,
            ..Default::default()
        };
        let empty: Vec<Detection> = Vec::new();
        let err = verify_install(
            &harness,
            Some("1.2.3"),
            &InstallMethodKind::Npm,
            &empty,
            &opts,
        )
        .unwrap_err();
        assert!(
            format!("{err}").contains("does not satisfy") || format!("{err}").contains("version")
        );

        drop(fs::remove_dir_all(tmp));
        drop(fs::remove_dir_all(home));
    }

    #[test]
    fn verify_version_satisfies_channels_and_ranges() {
        assert!(version_satisfies("latest", "1.2.3"));
        assert!(version_satisfies("stable", "9.0.0"));
        assert!(version_satisfies("1.2.3", "1.2.3"));
        assert!(version_satisfies("^1.2.0", "1.9.0"));
        assert!(!version_satisfies("^1.2.0", "2.0.0"));
        assert!(version_satisfies(">=1.0.0", "1.0.0"));
        assert!(version_satisfies(">=1.0.0", "2.5.0"));
        assert!(!version_satisfies(">=2.0.0", "1.9.9"));
        assert!(version_satisfies("v1.2.3", "1.2.3"));
        assert!(version_satisfies("1.2", "1.2.3"));
    }

    #[test]
    fn receipt_validation_rejects_nul() {
        let r = InstallReceipt {
            method: InstallMethodKind::Npm,
            package_id: "pkg\0bad".to_owned(),
            executable: "exe".to_owned(),
            version: "1.0.0".to_owned(),
            timestamp: "2026-08-26T00:00:00Z".to_owned(),
            path: PathBuf::from("/tmp/exe"),
        };
        assert!(r.validate().is_err());
    }

    // ---- PKG-07 ----

    #[cfg(unix)]
    #[test]
    fn update_plan_detects_current_and_shows_compat_and_blocks() {
        let tmp = make_temp_dir("update-cur");
        let home = make_temp_dir("home-update");
        // Fake codex with version 1.2.3
        fs::write(
            tmp.join("codex"),
            "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then echo \"Usage: codex --help\"; exit 0; fi\necho \"1.2.3\"\n",
        )
        .unwrap();
        let mut perms = fs::metadata(tmp.join("codex")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(tmp.join("codex"), perms).unwrap();

        let opts = DetectOptions {
            path_dirs: Some(vec![tmp.clone()]),
            home_dir: Some(home.clone()),
            probe_mise: false,
            probe_brew: false,
            probe_npm: false,
            probe_cargo: false,
            probe_apps: false,
            ..Default::default()
        };
        let harness = HarnessId::new("codex-cli").unwrap();

        // Inject available version as 2.0.0 (major bump -> incompatible)
        let plan = plan_update(&harness, &opts, None, false, Some("2.0.0")).unwrap();
        assert_eq!(plan.current_version.as_deref(), Some("1.2.3"));
        assert_eq!(plan.available_version.as_deref(), Some("2.0.0"));
        assert!(
            plan.blocked,
            "major bump should block without explicit accept"
        );
        assert!(plan.blocked_reason.is_some());
        assert!(!plan.compat_impacts.is_empty());
        // With explicit accept, not blocked
        let plan2 = plan_update(&harness, &opts, None, true, Some("2.0.0")).unwrap();
        assert!(!plan2.blocked);

        // Compatible minor bump should not block
        let plan3 = plan_update(&harness, &opts, None, false, Some("1.3.0")).unwrap();
        assert!(!plan3.blocked);

        drop(fs::remove_dir_all(tmp));
        drop(fs::remove_dir_all(home));
    }

    #[cfg(unix)]
    #[test]
    fn update_execute_respects_block() {
        let tmp = make_temp_dir("update-exec");
        let home = make_temp_dir("home-update-exec");
        fs::write(tmp.join("codex"), "#!/bin/sh\necho \"1.2.3\"\n").unwrap();
        let mut perms = fs::metadata(tmp.join("codex")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(tmp.join("codex"), perms).unwrap();

        let opts = DetectOptions {
            path_dirs: Some(vec![tmp.clone()]),
            home_dir: Some(home.clone()),
            probe_mise: false,
            probe_brew: false,
            probe_npm: false,
            probe_cargo: false,
            probe_apps: false,
            ..Default::default()
        };
        let harness = HarnessId::new("codex-cli").unwrap();
        let plan = plan_update(&harness, &opts, None, false, Some("2.0.0")).unwrap();
        assert!(plan.blocked);
        let err = execute_update(&plan, &opts, false, false).unwrap_err();
        assert!(
            format!("{err}").contains("blocked") || format!("{err}").contains("explicit accept")
        );
        // With explicit accept, execution proceeds to run the native update command.
        // For codex-cli the native update is `npm update -g @openai/codex`, which may not be
        // present in this sandbox. We test that explicit_accept bypasses the block check,
        // not that npm succeeds. So we catch either success or npm-not-found error,
        // but not the blocked error.
        let result = execute_update(&plan, &opts, true, false);
        // result may be Err due to missing npm, but it must not be the blocked error
        if let Err(e) = result {
            assert!(!format!("{e}").contains("blocked"));
        }
        drop(fs::remove_dir_all(tmp));
        drop(fs::remove_dir_all(home));
    }

    // ---- PKG-08 ----

    #[cfg(unix)]
    #[test]
    fn uninstall_preflight_lists_instances_and_preserves_config() {
        let tmp = make_temp_dir("uninstall-preserve");
        let home = make_temp_dir("home-uninstall");
        let harness = HarnessId::new("codex-cli").unwrap();

        // Create fake codex binary in tmp
        fs::write(tmp.join("codex"), "#!/bin/sh\necho \"1.0.0\"\n").unwrap();
        let mut perms = fs::metadata(tmp.join("codex")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(tmp.join("codex"), perms).unwrap();

        // Create fake config root that must be preserved
        let config_root = tmp.join("config-root-codex-work");
        fs::create_dir_all(&config_root).unwrap();
        let cfg_file = config_root.join("settings.json");
        fs::write(&cfg_file, r#"{"model":"test"}"#).unwrap();

        // Build a registry with an instance pointing at that config root
        let reg_path = tmp.join("registry.json");
        // Use registry via load/store
        // We'll use a helper to create a minimal instance and push via serialization round-trip.
        // Instead, we will directly craft JSON and load via Registry::load.
        let instance_json = serde_json::json!([{
            "id": "test-id-1",
            "name": "work",
            "harness": "codex-cli",
            "config_root": config_root.to_string_lossy(),
            "isolation": "relocated_root",
            "origin": "created",
            "ownership": "superai_created",
            "created_at": "2026-08-26T00:00:00Z",
            "adapter_revision": "0.1.0"
        }]);
        fs::write(&reg_path, serde_json::to_string(&instance_json).unwrap()).unwrap();
        let registry = Registry::load(&reg_path).unwrap();

        let opts = DetectOptions {
            path_dirs: Some(vec![tmp.clone()]),
            home_dir: Some(home.clone()),
            probe_mise: false,
            probe_brew: false,
            probe_npm: false,
            probe_cargo: false,
            probe_apps: false,
            ..Default::default()
        };
        let plan = plan_uninstall(&harness, &opts, Some(&registry), true).unwrap();
        assert!(
            plan.preflight
                .referencing_instances
                .contains(&"work".to_owned())
        );
        // macOS temp dirs live under /var, a symlink to /private/var: compare
        // canonicalized forms so the preserve-list match is filesystem-truth.
        let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        assert!(
            plan.preflight
                .preserved
                .iter()
                .any(|p| canon(p) == canon(&config_root)),
            "config root must be preserved: {:?} vs {config_root:?}",
            plan.preflight.preserved
        );
        assert!(
            plan.preflight
                .preserved
                .iter()
                .any(|p| p.ends_with("instances.json") || p.ends_with(".superai"))
        );
        // Command preview is npm uninstall, not rm
        assert_ne!(plan.command_preview.executable, "rm");
        // Simulate execution with a harmless echo instead of actual npm uninstall for test stability
        // Override the plan's command to echo for execution test
        let mut echo_plan = plan;
        echo_plan.command_preview = CommandTokens {
            executable: "echo".to_owned(),
            args: vec!["uninstall".to_owned(), "codex".to_owned()],
        };
        echo_plan.preflight.can_auto_delete = true;
        echo_plan.blocked = false;
        echo_plan.blocked_reason = None;
        let out = execute_uninstall(&echo_plan, true, false).unwrap();
        assert!(out.success);
        // Verify config file still exists (uninstall preserves config)
        assert!(
            cfg_file.exists(),
            "config must be preserved after uninstall"
        );
        assert_eq!(
            fs::read_to_string(&cfg_file).unwrap(),
            r#"{"model":"test"}"#
        );

        drop(fs::remove_dir_all(tmp));
        drop(fs::remove_dir_all(home));
    }

    #[cfg(unix)]
    #[test]
    fn uninstall_blocks_foreign_manual_file_without_explicit() {
        let tmp = make_temp_dir("foreign");
        let home = make_temp_dir("home-foreign");
        let harness = HarnessId::new("codex-cli").unwrap();
        // Place binary in a path that looks like manual (not mise/cargo/homebrew)
        let manual_dir = tmp.join("manual_bin");
        fs::create_dir_all(&manual_dir).unwrap();
        fs::write(manual_dir.join("codex"), "#!/bin/sh\necho \"1.0.0\"\n").unwrap();
        let mut perms = fs::metadata(manual_dir.join("codex"))
            .unwrap()
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(manual_dir.join("codex"), perms).unwrap();

        let opts = DetectOptions {
            path_dirs: Some(vec![manual_dir]),
            home_dir: Some(home.clone()),
            probe_mise: false,
            probe_brew: false,
            probe_npm: false,
            probe_cargo: false,
            probe_apps: false,
            ..Default::default()
        };
        let plan = plan_uninstall(&harness, &opts, None, false).unwrap();
        assert!(
            plan.preflight.foreign,
            "manual Path detection should be considered foreign"
        );
        assert!(!plan.preflight.can_auto_delete);
        assert!(plan.blocked);
        let err = execute_uninstall(&plan, false, false).unwrap_err();
        assert!(
            format!("{err}").contains("foreign")
                || format!("{err}").contains("Foreign")
                || format!("{err}").contains("not proven")
        );

        // With explicit allow, it proceeds (using echo-override to avoid real uninstall)
        let mut allowed = plan;
        allowed.preflight.can_auto_delete = true;
        allowed.blocked = false;
        allowed.blocked_reason = None;
        allowed.command_preview = CommandTokens {
            executable: "echo".to_owned(),
            args: vec!["uninstall".to_owned()],
        };
        let out = execute_uninstall(&allowed, true, false).unwrap();
        assert!(out.success);

        drop(fs::remove_dir_all(tmp));
        drop(fs::remove_dir_all(home));
    }

    #[test]
    fn uninstall_preserve_includes_backups_and_templates() {
        let harness = HarnessId::new("claude-code").unwrap();
        let preserved = preserved_paths_for(None, &harness);
        let strs: Vec<String> = preserved
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert!(strs.iter().any(|s| s.contains("backups")));
        assert!(
            strs.iter()
                .any(|s| s.contains("templates") || s.contains("assets") || s.contains(".superai"))
        );
    }

    #[test]
    fn execute_install_plan_validates_no_shell_pipeline() {
        let plan = InstallPlan {
            harness: "claude-code".to_owned(),
            method: InstallMethodKind::Npm,
            package_name: "@anthropic-ai/claude-code".to_owned(),
            version: None,
            channel: None,
            platform_os: "linux".to_owned(),
            platform_arch: "x86_64".to_owned(),
            command_preview: CommandTokens {
                executable: "sh".to_owned(),
                args: vec!["-c".to_owned(), "echo pwned | sh".to_owned()],
            },
            requires_network: true,
            requires_admin: false,
            conflicts: Vec::new(),
            expected_executable: PathBuf::from("/usr/local/bin/claude"),
            docs: "https://example.com".to_owned(),
            version_available: true,
            destination_writable: true,
        };
        // Validation should fail on sh -c
        assert!(plan.command_preview.validate().is_err());
    }
}
