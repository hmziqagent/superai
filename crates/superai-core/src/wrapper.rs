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
/// - execs the binary with plan args and `"$@"`
///
/// Returns `(content, digest)` where digest is hex of the final content.
pub fn generate_shell_wrapper(instance: &Instance, plan: &WrapperPlan) -> (String, String) {
    generate_shell_wrapper_with_version(instance, plan, GENERATOR_VERSION)
}

/// Same as [`generate_shell_wrapper`] but with explicit generator version.
pub fn generate_shell_wrapper_with_version(
    instance: &Instance,
    plan: &WrapperPlan,
    generator_version: &str,
) -> (String, String) {
    let binary_name = instance.binary.as_ref().map_or_else(
        || executable_for_harness(&instance.harness),
        ToString::to_string,
    );

    // Build marker without digest first, then compute digest, then re-emit marker with digest.
    // To keep deterministic, compute content once with placeholder, then compute digest, then embed.
    let mut lines: Vec<String> = Vec::new();
    lines.push("#!/bin/sh".to_owned());
    // Marker will be updated after digest known; use placeholder then replace.
    let marker_placeholder = format!(
        "# superai wrapper instance={} id={} harness={} generator={} digest=PLACEHOLDER",
        instance.name, instance.id, instance.harness, generator_version
    );
    lines.push(marker_placeholder);
    lines.push("# generated: do not edit manually; edits will be detected as drift".to_owned());
    lines.push("set -eu".to_owned());
    for (key, value) in &plan.env_vars {
        let quoted = shell_quote(value);
        lines.push(format!("export {key}={quoted}"));
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
    let content_without_digest = lines.join("\n") + "\n";
    let digest = compute_digest(content_without_digest.as_bytes());
    // Now replace placeholder digest
    let content =
        content_without_digest.replacen("digest=PLACEHOLDER", &format!("digest={digest}"), 1);
    (content, digest)
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

/// Write wrapper content atomically to `path`, backing up any existing file
/// that superai did not create (foreign file). The file is made executable
/// on unix. Returns the digest of the written content.
pub fn write_wrapper(path: &WrapperPath, content: &str) -> Result<String> {
    let target = path.as_path();
    // Backup if file exists (foreign file protection)
    if target.exists() {
        // Check if it's a directory -> error
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
        // Backup via superai-config if it's a regular file
        if meta.is_file() || meta.file_type().is_symlink() {
            let backup_res = superai_config::backup::backup(target);
            match backup_res {
                Ok(_) => {}
                Err(e) => {
                    return Err(CoreError::Config(e));
                }
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

/// Check whether a wrapper file appears to be superai-owned by inspecting its marker.
pub fn is_owned_wrapper(path: &Path, expected_digest: Option<&str>) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    if !content.contains("superai wrapper") {
        return false;
    }
    if let Some(digest) = expected_digest {
        content.contains(digest)
    } else {
        true
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
    // Verify exec target is the harness binary (or instance binary if set)
    let expected_binary = instance.binary.as_ref().map_or_else(
        || executable_for_harness(&instance.harness),
        ToString::to_string,
    );
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
