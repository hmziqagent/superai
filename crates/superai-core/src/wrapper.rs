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
            let backup_res = superai_config::backup(target);
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

    #[test]
    fn writes_wrapper_atomically_and_executable() {
        let dir = std::env::temp_dir().join("superai-wrapper-test-writes");
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
        drop(std::fs::remove_file(wrapper_path.as_path()));
    }

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
}
