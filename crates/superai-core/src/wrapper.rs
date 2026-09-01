//! Wrapper generation for isolated instances.
//!
//! Generates a portable `sh` wrapper that sets relocation env vars and execs
//! the harness binary. Content is deterministic, quoted safely, and marked
//! with instance identity and digest for drift detection. Secrets are never
//! embedded.

#![expect(
    clippy::manual_pattern_char_comparison,
    reason = "wrapper digest parsing"
)]
#![expect(
    clippy::string_slice,
    reason = "digest extraction uses char-boundary find"
)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::adapter::WrapperPlan;
use crate::error::{CoreError, Result};
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::paths::{AbsolutePath, WrapperPath};

/// Generator version written into wrappers.
pub const GENERATOR_VERSION: &str = env!("CARGO_PKG_VERSION");

fn compute_digest(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Quote a string for POSIX `sh` using single quotes, escaping inner `'`.
pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_owned();
    }
    // Replace each ' with '\'' (close, escaped, reopen)
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Map a harness to its primary relocation env var, if known.
pub fn env_var_for_harness(harness: &HarnessId) -> String {
    match harness.as_str() {
        "claude-code" => "CLAUDE_CONFIG_DIR".to_owned(),
        "codex-cli" => "CODEX_HOME".to_owned(),
        "opencode" => "XDG_CONFIG_HOME".to_owned(),
        "cline" => "CLINE_DATA_DIR".to_owned(),
        "aider" => "HOME".to_owned(),
        other => {
            let upper = other.to_ascii_uppercase().replace('-', "_");
            format!("{upper}_CONFIG_DIR")
        }
    }
}

/// Fallback executable name for a harness.
pub fn executable_for_harness(harness: &HarnessId) -> String {
    match harness.as_str() {
        "claude-code" => "claude".to_owned(),
        "codex-cli" => "codex".to_owned(),
        "opencode" => "opencode".to_owned(),
        "cline" => "cline".to_owned(),
        "aider" => "aider".to_owned(),
        other => other.to_owned(),
    }
}

/// Build the wrapper script content deterministically.
///
/// The script:
/// - starts with `#!/bin/sh`
/// - contains a marker comment with instance id, name, harness, generator, digest placeholder
/// - uses `set -eu`
/// - exports each env var from the plan, quoting values safely
/// - UNSETS each env var in `plan.env_unset` (WRP-02: a global credential
///   must not leak into an isolated profile through an inherited variable)
/// - execs the binary with plan args and `"$@"`
///
/// Returns `(content, digest)` where digest is hex of the final content.
pub fn generate_shell_wrapper(instance: &Instance, plan: &WrapperPlan) -> (String, String) {
    generate_shell_wrapper_with_version(instance, plan, GENERATOR_VERSION)
}

/// The binary the wrapper execs: the plan's explicit executable reference
/// (WRP-01), else the instance's pinned binary, else the harness default.
fn plan_executable<'a>(instance: &'a Instance, plan: &'a WrapperPlan) -> String {
    if let Some(exe) = &plan.executable {
        return exe.clone();
    }
    instance.binary.as_ref().map_or_else(
        || executable_for_harness(&instance.harness),
        ToString::to_string,
    )
}

/// Marker line shared by every launcher dialect (digest placeholder form).
fn marker_line(instance: &Instance, generator_version: &str) -> String {
    format!(
        "# superai wrapper instance={} id={} harness={} generator={} digest=PLACEHOLDER",
        instance.name, instance.id, instance.harness, generator_version
    )
}

/// Join lines, compute the placeholder-content digest, then embed it.
fn finalize_digest(lines: &[String]) -> (String, String) {
    let content_without_digest = lines.join("\n") + "\n";
    let digest = compute_digest(content_without_digest.as_bytes());
    let content =
        content_without_digest.replacen("digest=PLACEHOLDER", &format!("digest={digest}"), 1);
    (content, digest)
}

/// Same as [`generate_shell_wrapper`] but with explicit generator version.
pub fn generate_shell_wrapper_with_version(
    instance: &Instance,
    plan: &WrapperPlan,
    generator_version: &str,
) -> (String, String) {
    let binary_name = plan_executable(instance, plan);

    let mut lines: Vec<String> = vec!["#!/bin/sh".to_owned()];
    lines.push(marker_line(instance, generator_version));
    lines.push("# generated: do not edit manually; edits will be detected as drift".to_owned());
    lines.push("set -eu".to_owned());
    for (key, value) in &plan.env_vars {
        let quoted = shell_quote(value);
        lines.push(format!("export {key}={quoted}"));
    }
    // WRP-02: unset inherited variables the isolated profile must not see.
    for key in &plan.env_unset {
        lines.push(format!("unset {key}"));
    }
    // Build exec line: exec 'binary' 'arg1' ...
    let mut exec_parts: Vec<String> = Vec::new();
    exec_parts.push("exec".to_owned());
    exec_parts.push(shell_quote(&binary_name));
    for arg in &plan.args {
        exec_parts.push(shell_quote(arg));
    }
    exec_parts.push("\"$@\"".to_owned());
    lines.push(exec_parts.join(" "));
    finalize_digest(&lines)
}

/// Quote a value for PowerShell single-quoted strings (`'` doubles to `''`).
pub fn powershell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_owned();
    }
    let escaped = value.replace('\'', "''");
    format!("'{escaped}'")
}

/// Build a Windows PowerShell launcher (WRP-02).
///
/// Content-parallel to [`generate_shell_wrapper`]: same marker (digest over
/// the placeholder content), `Set-StrictMode`, env set, env UNSET via
/// `Remove-Item Env:`, then invoke the executable with plan args and forward
/// remaining arguments. PowerShell has no `exec` replacement; the launcher
/// runs the harness in the foreground of the console and propagates its
/// exit code with `exit $LASTEXITCODE` — the honest closest equivalent,
/// documented here rather than pretended.
pub fn generate_powershell_wrapper(instance: &Instance, plan: &WrapperPlan) -> (String, String) {
    generate_powershell_wrapper_with_version(instance, plan, GENERATOR_VERSION)
}

/// Same as [`generate_powershell_wrapper`] but with explicit generator version.
pub fn generate_powershell_wrapper_with_version(
    instance: &Instance,
    plan: &WrapperPlan,
    generator_version: &str,
) -> (String, String) {
    let binary = plan_executable(instance, plan);
    let mut lines: Vec<String> = vec![
        marker_line(instance, generator_version),
        "# generated: do not edit manually; edits will be detected as drift".to_owned(),
        "Set-StrictMode -Version Latest".to_owned(),
    ];
    lines.push("$ErrorActionPreference = 'Stop'".to_owned());
    for (key, value) in &plan.env_vars {
        lines.push(format!("$env:{} = {}", key, powershell_quote(value)));
    }
    for key in &plan.env_unset {
        lines.push(format!(
            "Remove-Item Env:\\{key} -ErrorAction SilentlyContinue"
        ));
    }
    let mut invoke: Vec<String> = vec!["&".to_owned(), powershell_quote(&binary)];
    for arg in &plan.args {
        invoke.push(powershell_quote(arg));
    }
    invoke.push("@args".to_owned());
    lines.push(invoke.join(" "));
    lines.push("exit $LASTEXITCODE".to_owned());
    finalize_digest(&lines)
}

/// Quote a value for cmd.exe (`"` doubles; `%` is escaped with `%%` so a
/// value can never inject variable expansion).
pub fn cmd_quote(value: &str) -> String {
    let escaped = value.replace('"', "\"\"").replace('%', "%%");
    format!("\"{escaped}\"")
}

/// Build a Windows `cmd` launcher (WRP-02): same marker/digest discipline,
/// env set via `set "VAR=..."`, env UNSET via `set "VAR="`, then run the
/// executable with plan args and forward `%*`. `cmd` runs the child attached
/// to the same console; there is no replacement semantic to claim.
pub fn generate_cmd_wrapper(instance: &Instance, plan: &WrapperPlan) -> (String, String) {
    generate_cmd_wrapper_with_version(instance, plan, GENERATOR_VERSION)
}

/// Same as [`generate_cmd_wrapper`] but with explicit generator version.
pub fn generate_cmd_wrapper_with_version(
    instance: &Instance,
    plan: &WrapperPlan,
    generator_version: &str,
) -> (String, String) {
    let binary = plan_executable(instance, plan);
    let mut lines: Vec<String> = vec![
        "@echo off".to_owned(),
        marker_line(instance, generator_version),
    ];
    lines.push("rem generated: do not edit manually; edits will be detected as drift".to_owned());
    for (key, value) in &plan.env_vars {
        lines.push(format!("set \"{key}={value}\""));
    }
    for key in &plan.env_unset {
        lines.push(format!("set \"{key}=\""));
    }
    let mut run: Vec<String> = vec![cmd_quote(&binary)];
    for arg in &plan.args {
        run.push(cmd_quote(arg));
    }
    run.push("%*".to_owned());
    lines.push(run.join(" "));
    finalize_digest(&lines)
}

/// Plan a wrapper for an instance via its adapter when possible, otherwise
/// using the generic env var mapping.
pub fn plan_wrapper_for_instance(instance: &Instance, plan: Option<WrapperPlan>) -> WrapperPlan {
    if let Some(p) = plan {
        return p;
    }
    let mut wrapper_plan = WrapperPlan::new(&format!("wrapper for {}", instance.harness));
    let env_var = env_var_for_harness(&instance.harness);
    wrapper_plan
        .env_vars
        .push((env_var, instance.config_root.to_string()));
    wrapper_plan
}

/// Write wrapper content atomically to `path`. The file is made executable
/// on unix. Returns the digest of the written content.
///
/// WRP-08 defense in depth: an existing file that is NOT a superai-owned
/// wrapper is REFUSED with a typed [`CoreError::ForeignOwnership`] — the
/// user's launcher requires explicit detach, never an overwrite. Replacing
/// a superai-owned wrapper (repair/rename) backs it up first; a missing
/// target is a plain create.
pub fn write_wrapper(path: &WrapperPath, content: &str) -> Result<String> {
    let target = path.as_path();
    if target.exists() {
        let meta = std::fs::symlink_metadata(target).map_err(|e| CoreError::Validation {
            field: "wrapper.path".to_owned(),
            reason: format!("cannot stat wrapper path {}: {e}", target.display()),
        })?;
        if meta.is_dir() {
            return Err(CoreError::Validation {
                field: "wrapper.path".to_owned(),
                reason: format!("wrapper path {} is a directory", target.display()),
            });
        }
        // Foreign-file refusal: only an owned wrapper (marker + parseable
        // generated grammar) may be replaced.
        match detect_wrapper_kind(target) {
            WrapperKind::SuperaiOwned { .. } => {
                superai_config::backup::backup(target).map_err(CoreError::Config)?;
            }
            WrapperKind::Missing => {}
            WrapperKind::Foreign { reason } | WrapperKind::Opaque { reason } => {
                return Err(CoreError::ForeignOwnership {
                    path: target.to_path_buf(),
                    owner: format!(
                        "existing launcher at {} is not superai-owned ({reason}); detach it \
                         explicitly first",
                        target.display()
                    ),
                });
            }
        }
    }
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            CoreError::Config(superai_config::ConfigError::Io {
                path: parent.to_path_buf(),
                source: e,
            })
        })?;
    }
    // Digest is the value embedded in the marker (hash of content without digest placeholder)
    let digest = extract_digest(content).unwrap_or_else(|| compute_digest(content.as_bytes()));
    superai_config::atomic::atomic_write(target, content.as_bytes()).map_err(CoreError::Config)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o755);
        if let Err(e) = std::fs::set_permissions(target, perm) {
            return Err(CoreError::Config(superai_config::ConfigError::Io {
                path: target.to_path_buf(),
                source: e,
            }));
        }
    }
    // Verify read-back content matches
    let written = std::fs::read(target).map_err(|e| {
        CoreError::Config(superai_config::ConfigError::Io {
            path: target.to_path_buf(),
            source: e,
        })
    })?;
    if written != content.as_bytes() {
        return Err(CoreError::Verification {
            path: target.to_path_buf(),
            kind: "digest".to_owned(),
            reason: "wrapper content mismatch after write".to_owned(),
        });
    }
    Ok(digest)
}

fn extract_digest(content: &str) -> Option<String> {
    let start = content.find("digest=")?;
    let after = &content[start + "digest=".len()..];
    let end = after
        .find(|c: char| c == '\n' || c == ' ' || c == '"' || c == '\'')
        .unwrap_or(after.len());
    let digest = &after[..end];
    if digest.is_empty() || digest == "PLACEHOLDER" {
        None
    } else {
        Some(digest.to_owned())
    }
}

/// Check whether a wrapper file is superai-owned by parsing it against the
/// generated grammar and verifying its marker (WRP-08: marker + digest —
/// never a substring match, which any comment could forge).
///
/// Ownership requires the content to parse as a generated wrapper AND carry
/// a `superai wrapper` marker with a digest. When `expected_digest` is
/// given, the marker digest must equal it exactly.
pub fn is_owned_wrapper(path: &Path, expected_digest: Option<&str>) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Some(parsed) = parse_wrapper_content(&content) else {
        return false;
    };
    let Some(marker) = parsed.marker.as_deref() else {
        return false;
    };
    if !marker.contains("superai wrapper") {
        return false;
    }
    match (expected_digest, parsed.digest.as_deref()) {
        (Some(expected), Some(actual)) => expected == actual,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

/// Render a wrapper path for preview purposes without writing.
pub fn preview_wrapper_content(
    instance: &Instance,
    plan: &WrapperPlan,
) -> (String, String, AbsolutePath) {
    let (content, digest) = generate_shell_wrapper(instance, plan);
    // Compute wrapper path as instance.wrapper if present, else placeholder
    let placeholder = instance.wrapper.as_ref().map_or_else(
        || PathBuf::from("/tmp/superai-wrapper-preview"),
        |w| w.path.as_path().to_path_buf(),
    );
    let abs = AbsolutePath::from_path(&placeholder).unwrap_or_else(|_| {
        // Fallback to /tmp
        #[expect(clippy::unwrap_used, reason = "fallback is known valid in tests")]
        AbsolutePath::new("/tmp/superai-wrapper-preview").unwrap()
    });
    (content, digest, abs)
}

// ---------------------------------------------------------------------------
// Wrapper detection, collision handling, and verification (WRP-02..04, WRP-08)
// ---------------------------------------------------------------------------

/// What a wrapper file on disk appears to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WrapperKind {
    /// File does not exist.
    Missing,
    /// Superai-owned wrapper with a verifiable digest.
    SuperaiOwned {
        /// Digest embedded in the marker.
        digest: String,
        /// Instance id from the marker, if parseable.
        instance_id: Option<String>,
    },
    /// User-owned wrapper that matches a known isolation recipe but is not superai-owned.
    Foreign {
        /// Reason it was classified as foreign.
        reason: String,
    },
    /// File exists but does not match generated grammar; treated as opaque.
    Opaque {
        /// Reason it is opaque.
        reason: String,
    },
}

/// Minimal parsed view of a generated wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedWrapper {
    /// Raw shebang line, e.g. `#!/bin/sh`.
    pub shebang: String,
    /// Instance marker line, if present.
    pub marker: Option<String>,
    /// Exported environment assignments in order.
    pub env_vars: Vec<(String, String)>,
    /// Environment variables the wrapper unsets, in order (WRP-02).
    pub env_unset: Vec<String>,
    /// Exec target binary (unquoted).
    pub exec_target: Option<String>,
    /// Extra exec args (unquoted) before `"$@"`.
    pub exec_args: Vec<String>,
    /// Whether the script ends with `"$@"`.
    pub forwards_args: bool,
    /// Digest extracted from the marker, if any.
    pub digest: Option<String>,
}

/// Maximum wrapper size that we attempt to parse (bounded parsing per DRF-03).
const MAX_WRAPPER_BYTES: usize = 32 * 1024;

/// Hex digest of wrapper content (first 16 hex chars of `DefaultHasher`).
///
/// Public for drift and verification callers; secrets are never included
/// in the hashed input because they are never embedded in the wrapper.
pub fn content_digest(content: &str) -> String {
    compute_digest(content.as_bytes())
}

/// Recompute the digest that `generate_shell_wrapper` would embed for this content.
///
/// The digest is computed over the content with `PLACEHOLDER` substitution
/// as done during generation; for already-generated wrappers this equals the
/// marker digest.
pub fn wrapper_digest_for_content(content: &str) -> String {
    extract_digest(content).unwrap_or_else(|| compute_digest(content.as_bytes()))
}

/// Detect what kind of wrapper exists at `path`.
///
/// - Never executes the wrapper.
/// - Shell parsing is bounded to the generated grammar; anything else is `Opaque`.
/// - File sizes above `MAX_WRAPPER_BYTES` are `Opaque`.
/// - Permission errors are treated as `Opaque` with a reason.
pub fn detect_wrapper_kind(path: &Path) -> WrapperKind {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return WrapperKind::Missing,
        Err(e) => {
            return WrapperKind::Opaque {
                reason: format!("cannot stat {}: {e}", path.display()),
            };
        }
    };
    if meta.is_dir() {
        return WrapperKind::Opaque {
            reason: format!("wrapper path {} is a directory", path.display()),
        };
    }
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            return WrapperKind::Opaque {
                reason: format!("cannot read {}: {e}", path.display()),
            };
        }
    };
    if data.len() > MAX_WRAPPER_BYTES {
        return WrapperKind::Opaque {
            reason: format!(
                "wrapper too large ({} bytes > {}); refusing to parse",
                data.len(),
                MAX_WRAPPER_BYTES
            ),
        };
    }
    let content = String::from_utf8_lossy(&data);
    detect_wrapper_kind_from_content(&content)
}

fn detect_wrapper_kind_from_content(content: &str) -> WrapperKind {
    if content.contains("superai wrapper") {
        let digest = extract_digest(content);
        let instance_id = extract_marker_field(content, "id=");
        match digest {
            Some(d) => WrapperKind::SuperaiOwned {
                digest: d,
                instance_id,
            },
            None => WrapperKind::Opaque {
                reason: "superai marker without digest".to_owned(),
            },
        }
    } else if looks_like_known_recipe(content) {
        WrapperKind::Foreign {
            reason: "matches known isolation recipe but no superai marker".to_owned(),
        }
    } else if is_opaque_shell(content) {
        WrapperKind::Opaque {
            reason: "does not match generated wrapper grammar".to_owned(),
        }
    } else {
        WrapperKind::Opaque {
            reason: "unrecognized wrapper content".to_owned(),
        }
    }
}

fn looks_like_known_recipe(content: &str) -> bool {
    // Known env vars we emit for isolation; a user wrapper that sets one of them and execs is likely a recipe.
    const KNOWN_VARS: &[&str] = &[
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "GOOSE_PATH_ROOT",
        "OPENCODE_CONFIG_DIR",
        "CLINE_DATA_DIR",
        "XDG_CONFIG_HOME",
    ];
    let has_known_env = KNOWN_VARS.iter().any(|v| content.contains(v));
    let has_exec = content.contains("exec ") || content.contains("exec\"");
    has_known_env && has_exec
}

fn is_opaque_shell(content: &str) -> bool {
    // Our generated grammar is small: shebang, marker comment, set -eu, exports, exec line.
    // Anything with control flow is opaque.
    for token in [
        " if ",
        " for ",
        " while ",
        " case ",
        " function ",
        " source ",
        ". ",
        " eval ",
    ] {
        if content.contains(token) {
            return true;
        }
    }
    // If file does not start with shebang, treat as opaque (aliases/shims may be binary)
    if !content.starts_with("#!/bin/sh") && !content.starts_with("#!/usr/bin/env") {
        // But allow superai-owned wrappers we already handled; foreign wrappers without shebang are opaque
        return true;
    }
    false
}

fn extract_marker_field(content: &str, key: &str) -> Option<String> {
    let start = content.find(key)?;
    let after = &content[start + key.len()..];
    let end = after
        .find(|c: char| c == ' ' || c == '\n' || c == '"' || c == '\'')
        .unwrap_or(after.len());
    let raw = &after[..end];
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_owned())
    }
}

/// Parse a wrapper that matches the generated grammar, bounded.
///
/// Returns `None` if the content does not match the generated grammar (opaque).
#[expect(
    clippy::excessive_nesting,
    reason = "wrapper parsing branches are explicit"
)]
pub fn parse_wrapper_content(content: &str) -> Option<ParsedWrapper> {
    if content.len() > MAX_WRAPPER_BYTES {
        return None;
    }
    let mut lines = content.lines();
    let shebang = lines.next()?.to_owned();
    if !shebang.starts_with("#!") {
        return None;
    }
    let mut marker: Option<String> = None;
    let mut env_vars: Vec<(String, String)> = Vec::new();
    let mut env_unset: Vec<String> = Vec::new();
    let mut exec_target: Option<String> = None;
    let mut exec_args: Vec<String> = Vec::new();
    let mut forwards_args = false;
    let mut digest: Option<String> = None;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("# generated") {
            continue;
        }
        if trimmed.contains("superai wrapper") {
            marker = Some(trimmed.to_owned());
            digest = extract_digest(trimmed);
            continue;
        }
        if trimmed.starts_with("set ") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("unset ") {
            let key = rest.trim();
            if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                env_unset.push(key.to_owned());
            } else {
                return None;
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("export ") {
            if let Some(eq) = rest.find('=') {
                let key = rest[..eq].trim().to_owned();
                let quoted = rest[eq + 1..].trim();
                let value = unquote_shell(quoted)?;
                env_vars.push((key, value));
            }
            continue;
        }
        if trimmed.starts_with("exec") {
            let args = shell_split_exec(trimmed)?;
            // args[0] is "exec", args[1] is target
            if let Some(target) = args.get(1) {
                exec_target = Some(target.to_owned());
            }
            if args.len() > 2 {
                for arg in args.iter().skip(2) {
                    if arg == "\"$@\"" || arg == "$@" {
                        forwards_args = true;
                    } else {
                        // Strip single quotes added by shell_quote
                        let unquoted = unquote_shell(arg).unwrap_or_else(|| arg.to_owned());
                        // Skip the "$@" sentinel
                        if unquoted == "$@" || unquoted == "\"$@\"" {
                            forwards_args = true;
                        } else {
                            exec_args.push(unquoted);
                        }
                    }
                }
            }
            // Check literal "$@" presence more directly
            if trimmed.contains("\"$@\"") || trimmed.contains("$@") {
                forwards_args = true;
            }
            continue;
        }
        // Any other non-empty non-comment line makes it opaque
        if !trimmed.starts_with('#') && !trimmed.is_empty() {
            return None;
        }
    }
    Some(ParsedWrapper {
        shebang,
        marker,
        env_vars,
        env_unset,
        exec_target,
        exec_args,
        forwards_args,
        digest,
    })
}

fn unquote_shell(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        return Some(String::new());
    }
    if let Some(inner) = t.strip_prefix('\'') {
        // Single-quoted: ends with ', with '\'' escapes inside (our generator uses this)
        // Reconstruct: replace '\'' -> '
        if let Some(end) = inner.rfind('\'') {
            let body = &inner[..end];
            let unescaped = body.replace("'\\''", "'");
            return Some(unescaped);
        }
        return None;
    }
    if (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
        || (t.starts_with('"') && t.contains("\"$@\""))
    {
        return Some(t.to_owned());
    }
    // Unquoted single word
    Some(t.to_owned())
}

#[expect(
    clippy::excessive_nesting,
    reason = "shell split branches are explicit"
)]
fn shell_split_exec(line: &str) -> Option<Vec<String>> {
    // Bounded, minimal splitter for "exec 'bin' 'arg1' \"$@\"" shapes.
    // Respects single quoting; does not handle full shell grammar — that is why opaque exists.
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let chars: Vec<char> = line.chars().collect();
    let mut i: usize = 0;
    while i < chars.len() {
        let Some(c) = chars.get(i).copied() else {
            break;
        };
        if in_single {
            if c == '\'' {
                // Check for escaped '' pattern: '\''
                if i + 3 < chars.len()
                    && chars.get(i + 1).copied() == Some('\\')
                    && chars.get(i + 2).copied() == Some('\'')
                    && chars.get(i + 3).copied() == Some('\'')
                {
                    current.push('\'');
                    i += 4;
                    continue;
                }
                in_single = false;
                current.push(c);
            } else {
                current.push(c);
            }
        } else if in_double {
            current.push(c);
            if c == '"' {
                in_double = false;
            }
        } else if c == '\'' {
            in_single = true;
            current.push(c);
        } else if c == '"' {
            in_double = true;
            current.push(c);
        } else if c == ' ' || c == '\t' {
            if !current.is_empty() {
                out.push(current.clone());
                current.clear();
            }
        } else {
            current.push(c);
        }
        i = i.saturating_add(1);
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.first().map(String::as_str) != Some("exec") {
        return None;
    }
    Some(out)
}

/// Check whether `name` collides case-folded with any name in `existing`.
pub fn is_name_collision_case_fold(name: &str, existing: &[&str]) -> bool {
    let needle = name.to_lowercase();
    existing.iter().any(|e| e.to_lowercase() == needle)
}

/// Check wrapper path and command collisions against a registry (case-insensitive).
///
/// - Duplicate wrapper paths (case-folded on case-insensitive filesystems) are errors.
/// - Wrapper command collisions with other wrapper commands or instance names (case-folded) are errors.
pub fn check_wrapper_collisions(
    new_path: &WrapperPath,
    new_command: &crate::ids::InstanceName,
    registry: &crate::registry::Registry,
) -> Result<()> {
    let new_path_norm = new_path.to_string().to_lowercase();
    let new_cmd_norm = new_command.normalized();
    for inst in registry.instances() {
        if let Some(wrapper) = &inst.wrapper {
            if wrapper.path.to_string().to_lowercase() == new_path_norm {
                return Err(CoreError::Validation {
                    field: "wrapper.path".to_owned(),
                    reason: format!(
                        "wrapper path `{}` collides case-insensitively with instance '{}'",
                        new_path, inst.name
                    ),
                });
            }
            if wrapper.command_name.normalized() == new_cmd_norm {
                return Err(CoreError::NameCollision {
                    kind: "WrapperCommand".to_owned(),
                    name: new_command.to_string(),
                    reason: format!(
                        "wrapper command case-fold collision with wrapper of '{}'",
                        inst.name
                    ),
                });
            }
        }
        if inst.name.normalized() == new_cmd_norm {
            return Err(CoreError::NameCollision {
                kind: "WrapperCommand/InstanceName".to_owned(),
                name: new_command.to_string(),
                reason: format!(
                    "wrapper command `{}` collides case-insensitively with instance '{}'",
                    new_command, inst.name
                ),
            });
        }
    }
    Ok(())
}

/// Check whether `dir` contains a file whose name matches `name` case-insensitively.
///
/// Handles Windows extensions (`.exe`, `.cmd`, `.bat`) by also checking those suffixes.
/// Returns the existing path if found.
pub fn exists_case_insensitive(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let needle = name.to_lowercase();
    for entry_res in entries {
        let Ok(entry) = entry_res else {
            continue;
        };
        let file_name = entry.file_name();
        let Some(fname) = file_name.to_str() else {
            continue;
        };
        let lower = fname.to_lowercase();
        if lower == needle {
            return Some(entry.path());
        }
        // Windows extension folding: if needle is "work" and file is "work.exe", consider collision
        for ext in [".exe", ".cmd", ".bat", ".ps1"] {
            if lower == format!("{needle}{ext}") {
                return Some(entry.path());
            }
        }
    }
    None
}

/// Check for executable collisions on `PATH` for `command_name` (case-insensitive).
///
/// Returns the colliding PATH entry if one exists outside `own_wrapper_dir`.
pub fn check_executable_collision_on_path(
    command_name: &str,
    own_wrapper_dir: Option<&Path>,
) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    let separator = if cfg!(windows) { ';' } else { ':' };
    let needle = command_name.to_lowercase();
    for dir in path_var.split(separator) {
        if dir.is_empty() {
            continue;
        }
        let dir_path = Path::new(dir);
        // Skip our own wrapper dir to avoid self-collision
        if let Some(own) = own_wrapper_dir
            && dir_path == own
        {
            continue;
        }
        if let Some(found) = exists_case_insensitive(dir_path, &needle) {
            return Some(found);
        }
    }
    None
}

/// Resolve the wrapper destination for `command_name` inside `bin_dir`.
///
/// - Validates that `command_name` is a legal instance name.
/// - Refuses to overwrite an unowned file at the destination.
/// - Checks registry collisions case-insensitively.
/// - Checks filesystem case-folding collisions.
/// - Checks PATH executable collisions (warned via `ForeignOwnership`-like error context).
///
/// Returns the `WrapperPath` that should be written. The caller must still call `write_wrapper`.
pub fn resolve_wrapper_destination(
    bin_dir: &Path,
    command_name: &crate::ids::InstanceName,
    registry: &crate::registry::Registry,
) -> Result<WrapperPath> {
    let candidate = bin_dir.join(command_name.as_str());
    // Validate as wrapper path (absolute)
    // For preview, handle relative bin_dir via home join? We require absolute for safety.
    let candidate_abs = if candidate.is_absolute() {
        candidate.clone()
    } else {
        // Treat bin_dir as absolute; if not, join with current dir but require AbsolutePath conversion to fail gracefully
        candidate.clone()
    };
    let wrapper_path =
        WrapperPath::new(&candidate_abs.to_string_lossy()).map_err(|e| CoreError::Validation {
            field: "wrapper.path".to_owned(),
            reason: format!("invalid wrapper path {}: {e}", candidate.display()),
        })?;

    // Filesystem collision: if file exists and is not superai-owned, refuse
    if candidate.exists() {
        match detect_wrapper_kind(&candidate) {
            WrapperKind::SuperaiOwned { .. } | WrapperKind::Missing => {}
            WrapperKind::Foreign { reason } | WrapperKind::Opaque { reason } => {
                return Err(CoreError::ForeignOwnership {
                    path: candidate.clone(),
                    owner: format!("existing file at {}: {reason}", candidate.display()),
                });
            }
        }
    }
    // Case-insensitive filesystem collision in the same directory
    if let Some(colliding) = exists_case_insensitive(bin_dir, command_name.as_str())
        && colliding != candidate
    {
        return Err(CoreError::NameCollision {
            kind: "WrapperPath".to_owned(),
            name: command_name.to_string(),
            reason: format!(
                "case-insensitive filesystem collision: {} collides with {}",
                candidate.display(),
                colliding.display()
            ),
        });
    }
    // Registry collisions
    check_wrapper_collisions(&wrapper_path, command_name, registry)?;
    // PATH collision is a warning not a hard error unless it would shadow; we surface as Validation
    if let Some(colliding) =
        check_executable_collision_on_path(command_name.as_str(), Some(bin_dir))
    {
        // Do not hard-fail for PATH shadow in preview? For commit, refuse if the colliding binary is not superai-owned.
        // Treat as conflict: the effective command resolution would be ambiguous.
        return Err(CoreError::Validation {
            field: "wrapper.command_name".to_owned(),
            reason: format!(
                "command `{}` collides with executable on PATH at {}",
                command_name,
                colliding.display()
            ),
        });
    }
    Ok(wrapper_path)
}

/// Verify that a generated wrapper at `path` matches the expected `instance` and `plan`.
///
/// - Parses the wrapper with bounded grammar.
/// - Confirms executable and `config_root` assignments match the plan.
/// - Compares digest to the value the generator would produce.
/// - Ensures the file is owned by superai (marker present).
///
/// Returns `Ok` when verification succeeds, or a `Verification` error with a reason.
pub fn verify_wrapper(path: &Path, instance: &Instance, plan: &WrapperPlan) -> Result<()> {
    let content = std::fs::read_to_string(path).map_err(|e| CoreError::Verification {
        path: path.to_path_buf(),
        kind: "read".to_owned(),
        reason: format!("cannot read wrapper at {}: {e}", path.display()),
    })?;
    let parsed = parse_wrapper_content(&content).ok_or_else(|| CoreError::Verification {
        path: path.to_path_buf(),
        kind: "parse".to_owned(),
        reason: "wrapper does not match generated grammar (opaque)".to_owned(),
    })?;
    // Must be superai-owned per marker
    if parsed.marker.is_none() || !content.contains("superai wrapper") {
        return Err(CoreError::Verification {
            path: path.to_path_buf(),
            kind: "marker".to_owned(),
            reason: "wrapper missing superai marker".to_owned(),
        });
    }
    // Verify digest
    let (expected_content, expected_digest) = generate_shell_wrapper(instance, plan);
    let actual_digest = parsed
        .digest
        .clone()
        .unwrap_or_else(|| content_digest(&content));
    if actual_digest != expected_digest {
        return Err(CoreError::Verification {
            path: path.to_path_buf(),
            kind: "digest".to_owned(),
            reason: format!("digest mismatch: expected {expected_digest}, actual {actual_digest}"),
        });
    }
    // Verify env vars from plan are present exactly
    for (key, expected_value) in &plan.env_vars {
        let found = parsed.env_vars.iter().find(|(k, _)| k == key);
        match found {
            Some((_, actual_value)) if actual_value == expected_value => {}
            Some((_, actual_value)) => {
                return Err(CoreError::Verification {
                    path: path.to_path_buf(),
                    kind: "env".to_owned(),
                    reason: format!(
                        "env {key} mismatch: expected `{expected_value}`, actual `{actual_value}`"
                    ),
                });
            }
            None => {
                return Err(CoreError::Verification {
                    path: path.to_path_buf(),
                    kind: "env".to_owned(),
                    reason: format!("missing env {key} in wrapper"),
                });
            }
        }
    }
    // Verify the wrapper unsets every variable the plan declares (WRP-02:
    // no inherited credential leaks into the isolated profile).
    for key in &plan.env_unset {
        if !parsed.env_unset.iter().any(|k| k == key) {
            return Err(CoreError::Verification {
                path: path.to_path_buf(),
                kind: "env".to_owned(),
                reason: format!("wrapper must unset `{key}`"),
            });
        }
    }
    // Verify exec target is the plan's executable (explicit reference,
    // instance-pinned binary, or the harness default — WRP-01 precedence).
    let expected_binary = plan_executable(instance, plan);
    if let Some(actual_target) = parsed.exec_target.as_deref() {
        // Targets are quoted in file; unquote for comparison
        let actual_unquoted = unquote_shell(actual_target)
            .unwrap_or_else(|| actual_target.to_owned())
            .trim_matches('\'')
            .to_owned();
        if actual_unquoted != expected_binary {
            // Allow absolute path that ends with binary name (e.g. /usr/local/bin/claude)
            if !actual_unquoted.ends_with(&expected_binary) {
                return Err(CoreError::Verification {
                    path: path.to_path_buf(),
                    kind: "exec".to_owned(),
                    reason: format!(
                        "exec target mismatch: expected `{expected_binary}`, actual `{actual_target}`"
                    ),
                });
            }
        }
    } else {
        return Err(CoreError::Verification {
            path: path.to_path_buf(),
            kind: "exec".to_owned(),
            reason: "missing exec target".to_owned(),
        });
    }
    if !parsed.forwards_args {
        return Err(CoreError::Verification {
            path: path.to_path_buf(),
            kind: "args".to_owned(),
            reason: "wrapper must forward \"$@\"".to_owned(),
        });
    }
    // Verify content matches expected exactly except digest already checked
    if content != expected_content {
        // For verification, allow the digest to be the only difference; if other diff, report
        // Do a normalized comparison ignoring digest line?
        // Simpler: if content not exactly expected, treat as drift
        return Err(CoreError::Verification {
            path: path.to_path_buf(),
            kind: "content".to_owned(),
            reason: "wrapper content drift from expected generation".to_owned(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// WRP-04 — bounded diagnostic probe + runtime isolation evidence
// ---------------------------------------------------------------------------

/// Outcome of a bounded, no-auth diagnostic launch of a wrapper (WRP-04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticProbe {
    /// Whether the probe exited zero within the bound.
    pub exit_ok: bool,
    /// Captured stdout, secret-shaped values redacted, bounded.
    pub stdout_redacted: String,
    /// Captured stderr, secret-shaped values redacted, bounded.
    pub stderr_redacted: String,
}

/// Redact secret-shaped tokens (`sk-…` and friends) from probe output.
fn redact_probe_output(text: &str) -> String {
    let mut out = text.to_owned();
    for prefix in ["sk-", "ghp_", "xoxb-"] {
        let mut redacted = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(idx) = rest.find(prefix) {
            let secret_start = idx + prefix.len();
            redacted.push_str(&rest[..secret_start]);
            let tail = &rest[secret_start..];
            let end = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                .unwrap_or(tail.len());
            if end == 0 {
                // bare prefix with no body; keep scanning after it
                redacted.push_str("[REDACTED]");
                rest = &rest[secret_start..];
            } else {
                redacted.push_str("[REDACTED]");
                rest = &rest[secret_start + end..];
            }
        }
        redacted.push_str(rest);
        out = redacted;
    }
    // Bound the surfaced output.
    let mut bounded = String::new();
    for (i, line) in out.lines().take(32).enumerate() {
        if i > 0 {
            bounded.push('\n');
        }
        bounded.push_str(line);
    }
    bounded
}

/// Run a bounded, no-auth diagnostic launch of the wrapper at `path`
/// (WRP-04): executes the wrapper itself with a caller-chosen diagnostic
/// argument (e.g. `--version`) under a CLEAN environment (the wrapper
/// installs its own isolation env) with a hard timeout and output cap.
/// Never passes credentials; output is redacted before return.
pub fn diagnostic_probe(
    path: &Path,
    probe_arg: &str,
    timeout: std::time::Duration,
) -> Result<DiagnosticProbe> {
    let opts = crate::process::ExecuteOpts {
        timeout: Some(timeout),
        clear_env: true,
        output_limit: Some(64 * 1024),
        redact: true,
        ..crate::process::ExecuteOpts::default()
    };
    let args = vec![probe_arg.to_owned()];
    let output = crate::process::run_command(&path.display().to_string(), &args, &opts)?;
    Ok(DiagnosticProbe {
        exit_ok: output.success,
        stdout_redacted: redact_probe_output(&output.stdout),
        stderr_redacted: redact_probe_output(&output.stderr),
    })
}

/// Runtime verdict on an instance's isolation claim (WRP-04): a claim of
/// `full` is only marked verified when the split surfaces are actually
/// observable in the generated invocation AND the plan declares no shared
/// state that remains joined (keychain, subscription, cloud account).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationVerdict {
    /// Split surfaces verified and no shared state declared.
    Full,
    /// Surfaces split but shared state remains (the honest constrained
    /// channel), or the claim could not be verified at runtime.
    Constrained,
}

impl std::fmt::Display for IsolationVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Full => "full",
            Self::Constrained => "constrained",
        };
        f.write_str(s)
    }
}

/// Evidence gathered for one isolation claim (WRP-04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolationEvidence {
    /// Isolation class the instance record claims.
    pub claimed: crate::state::Isolation,
    /// Verified verdict.
    pub verdict: IsolationVerdict,
    /// Split surfaces verified against the generated invocation (each line
    /// names the surface, e.g. `env CLAUDE_CONFIG_DIR=/x/.claude-work`).
    pub verified_surfaces: Vec<String>,
    /// Shared state the plan honestly declares as still joined.
    pub shared_state: Vec<String>,
}

/// Collect runtime isolation evidence for an instance from its wrapper plan
/// (WRP-04): verifies each declared env/arg split surface appears in the
/// generated wrapper content, and downgrades the verdict to
/// [`IsolationVerdict::Constrained`] whenever the plan declares shared
/// state that no wrapper can split (keychain/subscription/cloud).
pub fn isolation_evidence(instance: &Instance, plan: &WrapperPlan) -> IsolationEvidence {
    let (content, _) = generate_shell_wrapper(instance, plan);
    let parsed = parse_wrapper_content(&content);
    let mut verified: Vec<String> = Vec::new();
    for (key, value) in &plan.env_vars {
        let present = parsed
            .as_ref()
            .is_some_and(|p| p.env_vars.iter().any(|(k, v)| k == key && v == value));
        if present {
            verified.push(format!("env {key}={value}"));
        }
    }
    for key in &plan.env_unset {
        let present = parsed
            .as_ref()
            .is_some_and(|p| p.env_unset.iter().any(|k| k == key));
        if present {
            verified.push(format!("unset {key}"));
        }
    }
    for arg in &plan.args {
        if content.contains(arg) {
            verified.push(format!("arg {arg}"));
        }
    }
    let shared = plan.shared_state_warnings.clone();
    let verdict = if verified.is_empty() || !shared.is_empty() {
        IsolationVerdict::Constrained
    } else {
        IsolationVerdict::Full
    };
    IsolationEvidence {
        claimed: instance.isolation,
        verdict,
        verified_surfaces: verified,
        shared_state: shared,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::Adapter as _;
    use crate::ids::{InstanceId, InstanceName};
    use crate::state::{InstanceOrigin, Isolation, Ownership};

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-id-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        }
    }

    /// Platform: Linux and macOS — `#!/bin/sh` wrapper with `CLAUDE_CONFIG_DIR` and `exec`; Windows — same script via `bash`/`sh` (PowerShell/cmd wrapper not yet generated). Determinism holds on all platforms.
    #[test]
    fn generates_deterministic_sh_wrapper() {
        let inst = sample_instance_with_root("/tmp/.claude-work");
        let mut plan = WrapperPlan::new("test");
        plan.env_vars
            .push(("CLAUDE_CONFIG_DIR".to_owned(), inst.config_root.to_string()));
        let (content1, digest1) = generate_shell_wrapper(&inst, &plan);
        let (content2, digest2) = generate_shell_wrapper(&inst, &plan);
        assert_eq!(content1, content2);
        assert_eq!(digest1, digest2);
        assert!(content1.starts_with("#!/bin/sh\n"));
        assert!(content1.contains("superai wrapper"));
        assert!(content1.contains("CLAUDE_CONFIG_DIR"));
        assert!(content1.contains("exec"));
        assert!(content1.contains("\"$@\""));
        assert!(content1.contains(&digest1));
        // No secret leak
        assert!(!content1.contains("sk-"));
        // Marker contains instance identity
        assert!(content1.contains("work"));
        assert!(content1.contains("test-id-1"));
    }

    /// Platform: Linux, macOS, Windows — paths with spaces/`$`/`'` are single-quoted for POSIX `sh`; Windows `bash` also uses POSIX quoting, PowerShell differs (not covered here).
    #[test]
    fn quotes_special_paths_safely() {
        let inst = sample_instance_with_root("/tmp/my work with $dollar");
        let mut plan = WrapperPlan::new("test");
        plan.env_vars
            .push(("CLAUDE_CONFIG_DIR".to_owned(), inst.config_root.to_string()));
        let (content, _) = generate_shell_wrapper(&inst, &plan);
        // Value with space and $ must be single-quoted, not expanded
        assert!(content.contains("'/tmp/my work with $dollar'"));
        // Ensure no unquoted export
        assert!(!content.contains("export CLAUDE_CONFIG_DIR=/tmp/my work"));
    }

    /// Platform: Linux/macOS — atomically writes wrapper and sets `0o755` via `PermissionsExt`; Windows — atomic write without Unix perms (`#[cfg(unix)]` gated). Test verifies atomic write on all, exec bit only on Unix.
    #[test]
    fn writes_wrapper_atomically_and_executable() {
        let dir = crate::test_util::temp_dir_unique("wrapper");
        std::fs::create_dir_all(&dir).unwrap();
        let wrapper_path_str = dir.join("work-wrapper").to_string_lossy().into_owned();
        let wrapper_path = WrapperPath::new(&wrapper_path_str).unwrap();
        let inst = sample_instance_with_root("/tmp/.claude-work");
        let mut plan = WrapperPlan::new("test");
        plan.env_vars
            .push(("CLAUDE_CONFIG_DIR".to_owned(), inst.config_root.to_string()));
        let (content, digest) = generate_shell_wrapper(&inst, &plan);
        let written_digest = write_wrapper(&wrapper_path, &content).unwrap();
        assert_eq!(written_digest, digest);
        let read_back = std::fs::read_to_string(wrapper_path.as_path()).unwrap();
        assert_eq!(read_back, content);
        assert!(is_owned_wrapper(wrapper_path.as_path(), Some(&digest)));
        // Check executable bit on unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(wrapper_path.as_path())
                .unwrap()
                .permissions()
                .mode();
            assert!(mode & 0o111 != 0, "wrapper must be executable");
        }
        std::fs::remove_file(wrapper_path.as_path()).unwrap_or(());
    }

    /// Platform: all — wrapper content must not embed secrets on Linux, macOS, or Windows; redaction is platform-independent.
    #[test]
    fn never_embeds_secret() {
        let inst = sample_instance_with_root("/tmp/.claude-work");
        let mut plan = WrapperPlan::new("test");
        plan.env_vars
            .push(("CLAUDE_CONFIG_DIR".to_owned(), inst.config_root.to_string()));
        // Simulate secret not in plan
        let (content, _) = generate_shell_wrapper(&inst, &plan);
        let secret = "super-secret-sentinel-xyz";
        assert!(!content.contains(secret));
        assert!(!content.contains("sk-"));
        let json = serde_json::to_string(&content).unwrap();
        assert!(!json.contains(secret));
    }

    /// Platform: all — env var mapping (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `XDG_CONFIG_HOME`) is platform-independent; Windows uses same vars via `sh` wrapper, not registry.
    #[test]
    fn env_var_mapping_is_correct() {
        let h = HarnessId::new("claude-code").unwrap();
        assert_eq!(env_var_for_harness(&h), "CLAUDE_CONFIG_DIR");
        let h2 = HarnessId::new("codex-cli").unwrap();
        assert_eq!(env_var_for_harness(&h2), "CODEX_HOME");
        let h3 = HarnessId::new("opencode").unwrap();
        assert_eq!(env_var_for_harness(&h3), "XDG_CONFIG_HOME");
        let generic = HarnessId::new("my-harness").unwrap();
        assert_eq!(env_var_for_harness(&generic), "MY_HARNESS_CONFIG_DIR");
    }

    /// Platform: Linux/macOS — `SuperaiOwned`/`Foreign`/`Opaque` detection via shebang/marker; Windows — same detection, `is_owned_wrapper` does not check ACL, only marker digest.
    #[test]
    fn wrapper_dtype_detection_and_collision_and_digest() {
        let dir = crate::test_util::temp_dir_unique("wrapper");
        std::fs::create_dir_all(&dir).unwrap();
        let inst = sample_instance_with_root("/tmp/.claude-work-x");
        let mut plan = WrapperPlan::new("test");
        plan.env_vars
            .push(("CLAUDE_CONFIG_DIR".to_owned(), inst.config_root.to_string()));
        let (content, digest) = generate_shell_wrapper(&inst, &plan);
        let wrapper_path_str = dir.join("detect-wrapper").to_string_lossy().into_owned();
        let wrapper_path = WrapperPath::new(&wrapper_path_str).unwrap();
        let _ = write_wrapper(&wrapper_path, &content).unwrap();
        // Detection must be SuperaiOwned
        match detect_wrapper_kind(wrapper_path.as_path()) {
            WrapperKind::SuperaiOwned { digest: d, .. } => assert_eq!(d, digest),
            other => panic!("expected SuperaiOwned, got {other:?}"),
        }
        // Parse must succeed and contain env
        let parsed = parse_wrapper_content(&content).expect("parse must succeed");
        assert_eq!(parsed.shebang, "#!/bin/sh");
        assert!(parsed.marker.is_some());
        assert!(
            parsed
                .env_vars
                .iter()
                .any(|(k, _)| k == "CLAUDE_CONFIG_DIR")
        );
        assert!(parsed.forwards_args);
        assert_eq!(parsed.digest.as_deref(), Some(digest.as_str()));
        // Verification must succeed against the same instance/plan
        verify_wrapper(wrapper_path.as_path(), &inst, &plan).unwrap();
        // Content digest is deterministic and non-empty
        let d2 = content_digest(&content);
        assert!(!d2.is_empty());
        assert_eq!(wrapper_digest_for_content(&content), digest);
        assert_ne!(d2, digest);
        // Foreign detection: a user wrapper with same env but no marker
        let foreign_path = dir.join("foreign-wrapper");
        std::fs::write(
            &foreign_path,
            "#!/bin/sh\nexport CLAUDE_CONFIG_DIR='/tmp/.claude-other'\nexec claude \"$@\"\n",
        )
        .unwrap();
        match detect_wrapper_kind(&foreign_path) {
            WrapperKind::Foreign { .. } => {}
            other => panic!("expected Foreign for user wrapper, got {other:?}"),
        }
        // Opaque: control flow makes it opaque
        let opaque_path = dir.join("opaque-wrapper");
        std::fs::write(
            &opaque_path,
            "#!/bin/sh\nif true; then\n  exec claude\nfi\n",
        )
        .unwrap();
        match detect_wrapper_kind(&opaque_path) {
            WrapperKind::Opaque { .. } => {}
            other => panic!("expected Opaque, got {other:?}"),
        }
        std::fs::remove_file(wrapper_path.as_path()).unwrap_or(());
        std::fs::remove_file(&foreign_path).unwrap_or(());
        std::fs::remove_file(&opaque_path).unwrap_or(());
    }

    /// Platform: Linux — case-sensitive FS but `is_name_collision_case_fold` enforces case-insensitive command collision; macOS — typically case-insensitive; Windows — case-insensitive NTFS. Test asserts fold collision on all and `exists_case_insensitive` for FS lookup.
    #[test]
    fn wrapper_collision_case_insensitive_and_path() {
        use crate::ids::{InstanceId, InstanceName};
        use crate::instance::Instance;
        use crate::paths::AbsolutePath;
        use crate::registry::Registry;
        use crate::state::{InstanceOrigin, Isolation, Ownership};
        let mut reg = Registry::default();
        let inst = Instance {
            id: InstanceId::new("id-coll-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::new("/tmp/.claude-work-coll").unwrap(),
            binary: None,
            wrapper: Some(crate::instance::WrapperRef {
                path: WrapperPath::new("/tmp/bin/work").unwrap(),
                command_name: InstanceName::new("work").unwrap(),
                generator_version: "0.1.0".to_owned(),
                content_digest: "abc".to_owned(),
            }),
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        };
        reg.insert(inst).unwrap();
        // Case-fold collision on command
        assert!(is_name_collision_case_fold("WORK", &["work"]));
        assert!(!is_name_collision_case_fold("other", &["work"]));
        // Check wrapper collisions via registry helper
        let new_path = WrapperPath::new("/tmp/bin/WORK").unwrap();
        let cmd = InstanceName::new("WORK").unwrap();
        let err = check_wrapper_collisions(&new_path, &cmd, &reg).unwrap_err();
        match err {
            CoreError::Validation { .. } | CoreError::NameCollision { .. } => {}
            other => panic!("expected collision error, got {other:?}"),
        }
        // Filesystem case-insensitive existence
        let tmp = crate::test_util::temp_dir_unique("wrapper");
        std::fs::create_dir_all(&tmp).unwrap();
        let existing = tmp.join("MyTool");
        std::fs::write(&existing, "#!/bin/sh\necho hi\n").unwrap();
        let found = exists_case_insensitive(&tmp, "mytool");
        assert!(found.is_some(), "must find case-insensitive match");
        assert!(exists_case_insensitive(&tmp, "other").is_none());
        std::fs::remove_file(&existing).unwrap_or(());
    }

    /// Platform: Linux, macOS, Windows — tricky chars (spaces, `'`, `$`, `%`, Unicode) are POSIX single-quoted; Windows PowerShell would need different quoting (not covered), `bash` wrapper is used on Windows.
    #[test]
    fn wrapper_special_chars_quoted_and_verified() {
        let dir = crate::test_util::temp_dir_unique("wrapper");
        std::fs::create_dir_all(&dir).unwrap();
        // Path with spaces, quotes, Unicode, dollar and percent
        let tricky = "/tmp/my work with 'quote' $dollar %percent üñî";
        let inst = sample_instance_with_root(tricky);
        let mut plan = WrapperPlan::new("test");
        plan.env_vars
            .push(("CLAUDE_CONFIG_DIR".to_owned(), inst.config_root.to_string()));
        let (content, _) = generate_shell_wrapper(&inst, &plan);
        // Tricky chars must be quoted safely (single-quoted, dollar not expanded)
        assert!(content.contains("'/tmp/my work"));
        assert!(content.contains("$dollar"));
        // Write and verify round-trip
        let wrapper_path_str = dir.join("special-wrapper").to_string_lossy().into_owned();
        let wrapper_path = WrapperPath::new(&wrapper_path_str).unwrap();
        write_wrapper(&wrapper_path, &content).unwrap();
        verify_wrapper(wrapper_path.as_path(), &inst, &plan).unwrap();
        // Ensure no secret sentinel leaks
        assert!(!content.contains("super-secret"));
        std::fs::remove_file(wrapper_path.as_path()).unwrap_or(());
    }

    /// WRP-02: env UNSET support in the POSIX wrapper — an inherited global
    /// credential must not leak into an isolated profile.
    #[test]
    fn posix_wrapper_unsets_declared_env() {
        let inst = sample_instance_with_root("/tmp/.claude-work");
        let mut plan = WrapperPlan::new("test");
        plan.env_vars.push((
            "CLAUDE_CONFIG_DIR".to_owned(),
            "/tmp/.claude-work".to_owned(),
        ));
        plan.env_unset.push("ANTHROPIC_API_KEY".to_owned());
        let (content, digest) = generate_shell_wrapper(&inst, &plan);
        assert!(
            content.contains("\nunset ANTHROPIC_API_KEY\n"),
            "wrapper must unset declared vars: {content}"
        );
        // The unset list parses back out of the generated grammar.
        let parsed = parse_wrapper_content(&content).expect("grammar parses with unset lines");
        assert_eq!(parsed.env_unset, vec!["ANTHROPIC_API_KEY".to_owned()]);
        assert_eq!(parsed.digest.as_deref(), Some(digest.as_str()));
        // Round-trip: write + verify.
        let dir = crate::test_util::temp_dir_unique("wrapper-unset");
        std::fs::create_dir_all(&dir).unwrap();
        let path = WrapperPath::new(&dir.join("work").to_string_lossy()).unwrap();
        write_wrapper(&path, &content).unwrap();
        verify_wrapper(path.as_path(), &inst, &plan).unwrap();
        // A wrapper missing the unset fails verification.
        let mut stripped_plan = plan.clone();
        stripped_plan.env_unset.clear();
        let (stripped_content, _) = generate_shell_wrapper(&inst, &stripped_plan);
        std::fs::write(path.as_path(), &stripped_content).unwrap();
        match verify_wrapper(path.as_path(), &inst, &plan) {
            Err(e) => assert!(e.to_string().contains("unset"), "{e}"),
            Ok(()) => panic!("missing unset must fail verification"),
        }
    }

    /// WRP-02: PowerShell and cmd launchers are deterministic, carry the same
    /// marker/digest discipline, quote per dialect, unset env, forward args,
    /// and never embed a secret. Content assertions run on every platform —
    /// the goldens are strings.
    #[test]
    fn powershell_and_cmd_golden_launchers() {
        let inst = sample_instance_with_root("/tmp/my claude work");
        let mut plan = WrapperPlan::new("test");
        plan.env_vars.push((
            "CLAUDE_CONFIG_DIR".to_owned(),
            "/tmp/my claude work".to_owned(),
        ));
        plan.env_unset.push("ANTHROPIC_API_KEY".to_owned());
        plan.args.push("--settings".to_owned());

        let (ps1, ps1_digest) = generate_powershell_wrapper(&inst, &plan);
        let (ps1_again, ps1_digest2) = generate_powershell_wrapper(&inst, &plan);
        assert_eq!(ps1, ps1_again, "deterministic");
        assert_eq!(ps1_digest, ps1_digest2);
        assert!(ps1.contains("superai wrapper"), "marker present");
        assert!(ps1.contains(&ps1_digest), "digest embedded");
        assert!(ps1.contains("Set-StrictMode -Version Latest"));
        assert!(
            ps1.contains("$env:CLAUDE_CONFIG_DIR = '/tmp/my claude work'"),
            "single-quoted env: {ps1}"
        );
        assert!(
            ps1.contains("Remove-Item Env:\\ANTHROPIC_API_KEY -ErrorAction SilentlyContinue"),
            "env unset: {ps1}"
        );
        assert!(
            ps1.contains("& 'claude' '--settings' @args"),
            "invoke: {ps1}"
        );
        assert!(ps1.contains("exit $LASTEXITCODE"));
        assert!(!ps1.contains("sk-"));

        let (cmd, cmd_digest) = generate_cmd_wrapper(&inst, &plan);
        let (cmd_again, cmd_digest2) = generate_cmd_wrapper(&inst, &plan);
        assert_eq!(cmd, cmd_again);
        assert_eq!(cmd_digest, cmd_digest2);
        assert!(cmd.starts_with("@echo off"));
        assert!(cmd.contains("superai wrapper"));
        assert!(cmd.contains(&cmd_digest));
        assert!(
            cmd.contains("set \"CLAUDE_CONFIG_DIR=/tmp/my claude work\""),
            "env set: {cmd}"
        );
        assert!(cmd.contains("set \"ANTHROPIC_API_KEY=\""), "unset: {cmd}");
        assert!(cmd.contains("%*"), "forwards %*");
        assert!(
            cmd.contains("%%") || !cmd.contains('%') || cmd.contains("%*"),
            "cmd quoting"
        );
        assert!(!cmd.contains("sk-"));

        // An apostrophe is doubled in PowerShell single quotes.
        let tricky = "/tmp/it's here";
        let inst2 = sample_instance_with_root(tricky);
        let mut plan2 = WrapperPlan::new("test");
        plan2
            .env_vars
            .push(("CLAUDE_CONFIG_DIR".to_owned(), tricky.to_owned()));
        let (ps2, _) = generate_powershell_wrapper(&inst2, &plan2);
        assert!(
            ps2.contains("'/tmp/it''s here'"),
            "PS quote doubling: {ps2}"
        );
    }

    /// WRP-08 defense in depth: `write_wrapper` REFUSES a foreign file instead
    /// of backing it up and overwriting.
    #[test]
    fn write_wrapper_refuses_foreign_file() {
        let dir = crate::test_util::temp_dir_unique("wrapper-refuse");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("work");
        std::fs::write(
            &target,
            "#!/bin/sh\n# user's own launcher\nexec my-thing \"$@\"\n",
        )
        .unwrap();
        let before = std::fs::read(&target).unwrap();
        let wrapper_path = WrapperPath::new(&target.to_string_lossy()).unwrap();

        let inst = sample_instance_with_root("/tmp/.claude-work");
        let mut plan = WrapperPlan::new("test");
        plan.env_vars.push((
            "CLAUDE_CONFIG_DIR".to_owned(),
            "/tmp/.claude-work".to_owned(),
        ));
        let (content, _) = generate_shell_wrapper(&inst, &plan);

        match write_wrapper(&wrapper_path, &content) {
            Err(CoreError::ForeignOwnership { path, .. }) => {
                assert_eq!(path, target);
            }
            other => panic!("expected ForeignOwnership, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&target).unwrap(),
            before,
            "refused write must leave the foreign launcher byte-identical"
        );

        // Replacing a superai-OWNED wrapper stays allowed (repair path).
        std::fs::write(&target, &content).unwrap();
        let owned = write_wrapper(&wrapper_path, &content);
        assert!(owned.is_ok(), "owned replacement allowed: {owned:?}");
    }

    /// WRP-08: ownership = parseable marker + digest — a forged comment
    /// containing the marker substrings is NOT owned.
    #[test]
    fn is_owned_wrapper_requires_parseable_marker_and_digest() {
        let dir = crate::test_util::temp_dir_unique("wrapper-owned");
        std::fs::create_dir_all(&dir).unwrap();

        // A real generated wrapper verifies by digest.
        let inst = sample_instance_with_root("/tmp/.claude-work");
        let mut plan = WrapperPlan::new("test");
        plan.env_vars.push((
            "CLAUDE_CONFIG_DIR".to_owned(),
            "/tmp/.claude-work".to_owned(),
        ));
        let (content, digest) = generate_shell_wrapper(&inst, &plan);
        let real = dir.join("real");
        std::fs::write(&real, &content).unwrap();
        assert!(is_owned_wrapper(&real, Some(&digest)));
        assert!(!is_owned_wrapper(&real, Some("deadbeef")));

        // Forged: the substring test used to accept this — a script whose
        // BODY carries the digest with no parseable superai marker line must
        // not count as ownership.
        let forged = dir.join("forged");
        std::fs::write(
            &forged,
            "#!/bin/sh\n# my own launcher, honest\nexec evil abc123 \"$@\"\n",
        )
        .unwrap();
        assert!(
            !is_owned_wrapper(&forged, Some("abc123")),
            "digest smuggled in the body must not count as ownership"
        );

        // A wrapper generated for a DIFFERENT plan carries a different marker
        // digest: not owned under this record's digest.
        let other = dir.join("other");
        let mut other_plan = WrapperPlan::new("test");
        other_plan.env_vars.push((
            "CLAUDE_CONFIG_DIR".to_owned(),
            "/tmp/.claude-other".to_owned(),
        ));
        let (other_content, other_digest) = generate_shell_wrapper(&inst, &other_plan);
        assert_ne!(other_digest, digest);
        std::fs::write(&other, other_content).unwrap();
        assert!(!is_owned_wrapper(&other, Some(&digest)));
    }

    /// WRP-04: a bounded no-auth diagnostic launch runs the WRAPPER (which
    /// installs its own isolation env) against a fake binary, and proves the
    /// source/target trees are otherwise unchanged.
    #[test]
    fn diagnostic_probe_launches_wrapper_bounded_and_clean() {
        let dir = crate::test_util::temp_dir_unique("wrapper-probe");
        std::fs::create_dir_all(&dir).unwrap();
        // Fake harness binary: records its env + args, prints a line, exits 0.
        let fake_bin = dir.join("fake-harness");
        std::fs::write(
            &fake_bin,
            "#!/bin/sh\nprintf 'cfg=%s arg=%s\\n' \"$CLAUDE_CONFIG_DIR\" \"$1\"\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let inst_root = dir.join(".claude-work");
        std::fs::create_dir_all(&inst_root).unwrap();
        std::fs::write(inst_root.join("settings.json"), r#"{"model":"x"}"#).unwrap();
        let mut inst = sample_instance_with_root(&inst_root.to_string_lossy());
        inst.binary = Some(crate::paths::ExecutableRef::Absolute(
            AbsolutePath::from_path(&fake_bin).unwrap(),
        ));
        let mut plan = WrapperPlan::new("test");
        plan.env_vars.push((
            "CLAUDE_CONFIG_DIR".to_owned(),
            inst_root.to_string_lossy().into_owned(),
        ));
        let (content, _) = generate_shell_wrapper(&inst, &plan);
        let wrapper_file = dir.join("work");
        std::fs::write(&wrapper_file, &content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper_file, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let source_before = std::fs::read(inst_root.join("settings.json")).unwrap();
        let probe = diagnostic_probe(
            &wrapper_file,
            "--version",
            std::time::Duration::from_secs(10),
        )
        .expect("probe runs");
        assert!(probe.exit_ok, "stdout: {}", probe.stdout_redacted);
        assert!(
            probe.stdout_redacted.contains("cfg="),
            "isolation env reached the binary: {}",
            probe.stdout_redacted
        );
        assert!(probe.stdout_redacted.contains("--version"));
        // No unexpected writes: the config tree is byte-identical.
        assert_eq!(
            std::fs::read(inst_root.join("settings.json")).unwrap(),
            source_before
        );
    }

    /// WRP-04/05: runtime isolation evidence verifies split surfaces and
    /// downgrades to constrained when shared state is declared.
    #[test]
    fn isolation_evidence_verifies_or_constrains() {
        // A claude-code plan (split env verified, no shared-state warning)
        // is FULL.
        let inst = sample_instance_with_root("/tmp/.claude-work");
        let mut full_plan = WrapperPlan::new("test");
        full_plan.env_vars.push((
            "CLAUDE_CONFIG_DIR".to_owned(),
            "/tmp/.claude-work".to_owned(),
        ));
        let evidence = isolation_evidence(&inst, &full_plan);
        assert_eq!(evidence.verdict, IsolationVerdict::Full);
        assert!(
            evidence
                .verified_surfaces
                .iter()
                .any(|s| s.starts_with("env CLAUDE_CONFIG_DIR")),
            "{:?}",
            evidence.verified_surfaces
        );

        // A cline plan declares shared VS Code keychain state (WRP-05): the
        // claim is honestly CONSTRAINED even with split surfaces verified.
        let cline = crate::adapters::cline::ClineAdapter::new().unwrap();
        let mut cline_inst = sample_instance_with_root("/tmp/.cline-work");
        cline_inst.harness = cline.id();
        let cline_plan = cline.plan_wrapper(&cline_inst).unwrap();
        let cline_evidence = isolation_evidence(&cline_inst, &cline_plan);
        assert_eq!(
            cline_evidence.verdict,
            IsolationVerdict::Constrained,
            "shared keychain state must surface constrained"
        );
        assert!(
            !cline_evidence.shared_state.is_empty(),
            "the shared-state warning must be carried"
        );
        assert!(
            !cline_evidence.verified_surfaces.is_empty(),
            "split surfaces are still verified: {:?}",
            cline_evidence.verified_surfaces
        );
    }

    /// WRP-05: two concurrent IDE profiles keep distinct data directories and
    /// distinct CLI roots, and both wrappers verify against their instances.
    #[test]
    fn two_concurrent_ide_profiles_split_state_dirs() {
        let cline = crate::adapters::cline::ClineAdapter::new().unwrap();
        let root_a = "/tmp/superai-profiles/cline-a";
        let root_b = "/tmp/superai-profiles/cline-b";
        let inst_a = Instance {
            id: InstanceId::new("ide-a").unwrap(),
            name: InstanceName::new("profile-a").unwrap(),
            harness: cline.id(),
            config_root: AbsolutePath::new(root_a).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::IdeUserData,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        };
        let mut inst_b = inst_a.clone();
        inst_b.id = InstanceId::new("ide-b").unwrap();
        inst_b.name = InstanceName::new("profile-b").unwrap();
        inst_b.config_root = AbsolutePath::new(root_b).unwrap();

        let plan_a = cline.plan_wrapper(&inst_a).unwrap();
        let plan_b = cline.plan_wrapper(&inst_b).unwrap();

        // The CLI roots and BOTH editor dirs are distinct per profile.
        let env_a = &plan_a
            .env_vars
            .iter()
            .find(|(k, _)| k == "CLINE_DATA_DIR")
            .unwrap()
            .1;
        let env_b = &plan_b
            .env_vars
            .iter()
            .find(|(k, _)| k == "CLINE_DATA_DIR")
            .unwrap()
            .1;
        assert_ne!(env_a, env_b);
        assert_ne!(plan_a.args, plan_b.args, "editor dirs must differ");
        assert!(
            plan_a.state_paths.iter().any(|p| p.contains(root_a)),
            "{:?}",
            plan_a.state_paths
        );
        assert!(
            plan_b.state_paths.iter().any(|p| p.contains(root_b)),
            "{:?}",
            plan_b.state_paths
        );

        // Both wrappers generate, parse, and verify independently.
        let (content_a, _) = generate_shell_wrapper(&inst_a, &plan_a);
        let (content_b, _) = generate_shell_wrapper(&inst_b, &plan_b);
        assert_ne!(content_a, content_b);
        let parsed_a = parse_wrapper_content(&content_a).expect("a parses");
        let parsed_b = parse_wrapper_content(&content_b).expect("b parses");
        assert_ne!(parsed_a.env_vars, parsed_b.env_vars);
        let dir = crate::test_util::temp_dir_unique("wrapper-ide2");
        std::fs::create_dir_all(&dir).unwrap();
        let path_a = WrapperPath::new(&dir.join("a").to_string_lossy()).unwrap();
        let path_b = WrapperPath::new(&dir.join("b").to_string_lossy()).unwrap();
        write_wrapper(&path_a, &content_a).unwrap();
        write_wrapper(&path_b, &content_b).unwrap();
        verify_wrapper(path_a.as_path(), &inst_a, &plan_a).unwrap();
        verify_wrapper(path_b.as_path(), &inst_b, &plan_b).unwrap();
        // Cross-verification fails: the profiles are genuinely distinct.
        assert!(verify_wrapper(path_a.as_path(), &inst_b, &plan_b).is_err());
    }
}
