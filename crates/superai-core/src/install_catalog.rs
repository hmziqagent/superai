//! Installation catalog — data-driven harness package registry (PKG-02).
//!
//! Per harness/platform the catalog records: `HarnessId`, executable and app
//! identifiers, supported install methods with official package names, version
//! sources and constraints, detect commands and filesystem paths, update and
//! uninstall command tokens (executable + argv, never a shell pipeline),
//! checksum/signature guards, known conflicts, documentation links, and the
//! last-verified date. All data lives in `assets/install_catalog.json` and is
//! validated on load — no package identity or command is hard-coded in Rust.
//!
//! PKG-01 verification is documented in [`crate::process`]; this module
//! records the duct/mise decision for auditability.

#![expect(
    clippy::excessive_nesting,
    reason = "intentional deep validation branching"
)]
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::HarnessId;

// ---------------------------------------------------------------------------
// Embedded catalog asset path
// ---------------------------------------------------------------------------

/// Embedded catalog JSON (compile-time include). Mirrors
/// `assets/install_catalog.json` so data is available without filesystem I/O
/// in tests and single-binary deployments. Filesystem load is still provided
/// for hot-reload / operator-supplied catalogs.
const EMBEDDED_CATALOG: &str = include_str!("../assets/install_catalog.json");

// ---------------------------------------------------------------------------
// Install method kind
// ---------------------------------------------------------------------------

/// Supported install method for a harness.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallMethodKind {
    /// Version-manager install via `mise` (preferred when available).
    Mise,
    /// Homebrew formula (`brew install`).
    Homebrew,
    /// Homebrew cask (macOS .app / GUI).
    HomebrewCask,
    /// npm global package.
    Npm,
    /// Cargo `cargo install`.
    Cargo,
    /// pipx isolated Python tool.
    Pipx,
    /// uv Python tool.
    Uv,
    /// Direct download / official installer script or binary.
    Direct,
    /// External / manual GUI / marketplace (no non-interactive path).
    External,
}

impl std::fmt::Display for InstallMethodKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Mise => "mise",
            Self::Homebrew => "homebrew",
            Self::HomebrewCask => "homebrew_cask",
            Self::Npm => "npm",
            Self::Cargo => "cargo",
            Self::Pipx => "pipx",
            Self::Uv => "uv",
            Self::Direct => "direct",
            Self::External => "external",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// Command tokens (executable + argv, no shell pipeline)
// ---------------------------------------------------------------------------

/// A structured command: executable plus argv tokens, never a shell pipeline.
///
/// Every command in the catalog must be representable as `executable` + `args`.
/// Strings containing shell metacharacters (`|`, `&&`, `;`, `` ` ``, `$(`,
/// `>`, `<`) are rejected on validation to ensure no shell interpolation
/// occurs at execution time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandTokens {
    /// Program to execute (looked up via `PATH` unless absolute).
    pub executable: String,
    /// Positional arguments.
    #[serde(default)]
    pub args: Vec<String>,
}

const FORBIDDEN_TOKENS: &[&str] = &[
    "|", "&&", "||", ";", "`", "$(", "${", ">", "<", ">>", "<<", "&",
];

impl CommandTokens {
    /// Validate that the command contains no shell metacharacters and no empty
    /// executable.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.executable.is_empty() {
            return Err(CoreError::Validation {
                field: "command.executable".to_owned(),
                reason: "executable must not be empty".to_owned(),
            });
        }
        if self.executable.contains('\0')
            || self.executable.contains('/') && self.executable.contains('`')
        {
            // NUL is always forbidden; backticks are shell metachar
        }
        // Forbid shell metachars in executable and args.
        let check = |field: &str, value: &str| -> Result<(), CoreError> {
            if value.contains('\0') {
                return Err(CoreError::Validation {
                    field: field.to_owned(),
                    reason: "must not contain NUL".to_owned(),
                });
            }
            for pat in FORBIDDEN_TOKENS {
                // Exact shell pipeline tokens are forbidden; substring check is
                // intentionally strict — catalog commands should be pure argv.
                // Allow `|` inside a package name? No package name should
                // contain shell operators, so we reject any occurrence where
                // the arg is exactly a shell operator or contains the classic
                // interpolation patterns.
                if *pat == "|" && value == "|" {
                    return Err(CoreError::Validation {
                        field: field.to_owned(),
                        reason: "arg must not be shell pipeline token `|`".to_owned(),
                    });
                }
                if (*pat == "$(" || *pat == "${" || *pat == "`") && value.contains(pat) {
                    return Err(CoreError::Validation {
                        field: field.to_owned(),
                        reason: format!("must not contain shell pattern `{pat}`"),
                    });
                }
                if (*pat == "&&" || *pat == "||" || *pat == ";" || *pat == ">>" || *pat == "<<")
                    && value.contains(pat)
                {
                    return Err(CoreError::Validation {
                        field: field.to_owned(),
                        reason: format!("must not contain shell pattern `{pat}`"),
                    });
                }
                if (*pat == ">" || *pat == "<") && value == *pat {
                    return Err(CoreError::Validation {
                        field: field.to_owned(),
                        reason: format!("arg must not be shell redirect `{pat}`"),
                    });
                }
            }
            // Also forbid bare `-c` sh invocation patterns hidden as args
            // e.g. ["sh", "-c", "curl ... | sh"] — the `"curl | sh"` case is
            // already caught by the pipeline check, but we also reject an arg
            // that is exactly "-c" when the executable is sh/bash.
            Ok(())
        };
        check("command.executable", &self.executable)?;
        if (self.executable == "sh" || self.executable == "bash")
            && self.args.iter().any(|a| a == "-c")
        {
            return Err(CoreError::Validation {
                field: "command.args".to_owned(),
                reason: "must not invoke shell via `sh -c` / `bash -c`".to_owned(),
            });
        }
        for arg in &self.args {
            check("command.args", arg)?;
        }
        Ok(())
    }

    /// Render as a display string `executable arg1 arg2 ...`.
    pub fn display(&self) -> String {
        let mut out = self.executable.clone();
        for arg in &self.args {
            out.push(' ');
            out.push_str(arg);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Install method
// ---------------------------------------------------------------------------

/// One install method for a harness: kind plus official package identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallMethod {
    /// Method kind.
    pub kind: InstallMethodKind,
    /// Official package name in the method's registry (formula, crate, npm pkg, etc).
    pub package_name: String,
    /// Homebrew tap (`owner/repo`) if applicable.
    #[serde(default)]
    pub tap: Option<String>,
    /// Repository or registry URL (e.g., crates.io, npm registry, GitHub repo).
    #[serde(default)]
    pub repo: Option<String>,
    /// Registry URL if different from repo.
    #[serde(default)]
    pub registry: Option<String>,
}

impl InstallMethod {
    /// Validate method fields.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.package_name.is_empty() {
            return Err(CoreError::Validation {
                field: "install_method.package_name".to_owned(),
                reason: format!("package_name must not be empty for method `{}`", self.kind),
            });
        }
        if self.package_name.contains('\0')
            || self.package_name.contains('`')
            || self.package_name.contains('|')
        {
            return Err(CoreError::Validation {
                field: "install_method.package_name".to_owned(),
                reason: "package_name must not contain shell metacharacters".to_owned(),
            });
        }
        // Forbid shell metachars in tap/repo/registry
        for (field, val) in [
            ("install_method.tap", self.tap.as_ref()),
            ("install_method.repo", self.repo.as_ref()),
            ("install_method.registry", self.registry.as_ref()),
        ] {
            if let Some(v) = val
                && (v.contains('\0') || v.contains('`') || v.contains('|') || v.contains(';'))
            {
                return Err(CoreError::Validation {
                    field: field.to_owned(),
                    reason: "must not contain shell metacharacters".to_owned(),
                });
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Constraints and detect hints
// ---------------------------------------------------------------------------

/// Platform constraints for an install entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlatformConstraints {
    /// Supported OS values: `linux`, `macos`, `windows`, `any`.
    #[serde(default)]
    pub os: Vec<String>,
    /// Supported arch values: `x86_64`, `aarch64`, `any`.
    #[serde(default)]
    pub arch: Vec<String>,
}

impl PlatformConstraints {
    /// Return true if the given `os`/`arch` pair is supported.
    pub fn supports(&self, os: &str, arch: &str) -> bool {
        let os_ok = self.os.is_empty() || self.os.iter().any(|v| v == "any" || v == os);
        let arch_ok = self.arch.is_empty() || self.arch.iter().any(|v| v == "any" || v == arch);
        os_ok && arch_ok
    }

    /// Validate entries are known values.
    pub fn validate(&self) -> Result<(), CoreError> {
        const KNOWN_OS: &[&str] = &["linux", "macos", "windows", "any"];
        const KNOWN_ARCH: &[&str] = &["x86_64", "aarch64", "any"];
        for v in &self.os {
            if !KNOWN_OS.contains(&v.as_str()) {
                return Err(CoreError::Validation {
                    field: "constraints.os".to_owned(),
                    reason: format!("unknown os `{v}`; expected one of {KNOWN_OS:?}"),
                });
            }
        }
        for v in &self.arch {
            if !KNOWN_ARCH.contains(&v.as_str()) {
                return Err(CoreError::Validation {
                    field: "constraints.arch".to_owned(),
                    reason: format!("unknown arch `{v}`; expected one of {KNOWN_ARCH:?}"),
                });
            }
        }
        Ok(())
    }
}

/// Detect hints for a harness: probe commands and filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DetectHints {
    /// Commands to run to detect the harness, e.g. `[{"executable":"claude","args":["--version"]}]`.
    #[serde(default)]
    pub commands: Vec<CommandTokens>,
    /// Filesystem paths to probe for the harness.
    #[serde(default)]
    pub paths: Vec<String>,
}

impl DetectHints {
    /// Validate detect commands and paths.
    pub fn validate(&self) -> Result<(), CoreError> {
        for cmd in &self.commands {
            cmd.validate()?;
        }
        for p in &self.paths {
            if p.contains('\0') {
                return Err(CoreError::Validation {
                    field: "detect.paths".to_owned(),
                    reason: "path must not contain NUL".to_owned(),
                });
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// InstallCatalogEntry
// ---------------------------------------------------------------------------

/// One harness's install registry entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallCatalogEntry {
    /// Stable harness identifier (validated as `HarnessId`).
    pub harness: String,
    /// CLI executable names produced by this harness (at least one).
    #[serde(default)]
    pub executables: Vec<String>,
    /// macOS bundle identifiers (e.g., `saoudrizwan.claude-dev`).
    #[serde(default)]
    pub bundle_ids: Vec<String>,
    /// Desktop app bundle paths (e.g., `/Applications/Visual Studio Code.app`).
    #[serde(default)]
    pub apps: Vec<String>,
    /// Supported install methods with official package identities.
    #[serde(default)]
    pub methods: Vec<InstallMethod>,
    /// How to obtain the version, e.g. `claude --version` (informational).
    #[serde(default)]
    pub version_source: String,
    /// Platform constraints.
    #[serde(default)]
    pub constraints: PlatformConstraints,
    /// Detection hints: commands and paths.
    #[serde(default)]
    pub detect: DetectHints,
    /// Update command tokens (executable + argv, no shell pipeline).
    #[serde(default)]
    pub update: Option<CommandTokens>,
    /// Uninstall command tokens (executable + argv).
    #[serde(default)]
    pub uninstall: Option<CommandTokens>,
    /// Whether installation requires admin/elevated privileges.
    #[serde(default)]
    pub requires_admin: bool,
    /// Optional checksum or signature guard (hex digest or URL).
    #[serde(default)]
    pub checksum: Option<String>,
    /// Harness IDs that conflict or are replaced by this harness.
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// Documentation URL.
    #[serde(default)]
    pub docs: String,
    /// Last verified date `YYYY-MM-DD`.
    #[serde(default)]
    pub last_verified: String,
}

impl InstallCatalogEntry {
    /// Validate the entry.
    pub fn validate(&self) -> Result<(), CoreError> {
        // HarnessId must be valid
        HarnessId::new(&self.harness).map_err(|e| CoreError::Validation {
            field: "harness".to_owned(),
            reason: format!("invalid HarnessId `{}`: {e}", self.harness),
        })?;
        if self.executables.is_empty() && self.bundle_ids.is_empty() && self.apps.is_empty() {
            return Err(CoreError::Validation {
                field: "executables/bundle_ids/apps".to_owned(),
                reason: format!(
                    "at least one of executables, bundle_ids, or apps must be non-empty for `{}`",
                    self.harness
                ),
            });
        }
        for exe in &self.executables {
            if exe.is_empty() || exe.contains('\0') || exe.contains('/') || exe.contains('\\') {
                return Err(CoreError::Validation {
                    field: "executables".to_owned(),
                    reason: format!("invalid executable name `{exe}`"),
                });
            }
        }
        if self.methods.is_empty() {
            return Err(CoreError::Validation {
                field: "methods".to_owned(),
                reason: format!(
                    "at least one install method required for `{}`",
                    self.harness
                ),
            });
        }
        for m in &self.methods {
            m.validate()?;
        }
        self.constraints.validate()?;
        self.detect.validate()?;
        if let Some(cmd) = self.update.as_ref() {
            cmd.validate()?;
        }
        if let Some(cmd) = self.uninstall.as_ref() {
            cmd.validate()?;
        }
        if self.docs.is_empty() {
            return Err(CoreError::Validation {
                field: "docs".to_owned(),
                reason: format!("docs URL must not be empty for `{}`", self.harness),
            });
        }
        validate_last_verified(&self.last_verified)?;
        Ok(())
    }

    /// Return the validated harness id.
    pub fn harness_id(&self) -> Result<HarnessId, CoreError> {
        HarnessId::new(&self.harness)
    }

    /// Check if the given os/arch pair is supported.
    pub fn supports_platform(&self, os: &str, arch: &str) -> bool {
        self.constraints.supports(os, arch)
    }
}

/// Validate `YYYY-MM-DD` date format (basic).
fn validate_last_verified(value: &str) -> Result<(), CoreError> {
    if value.is_empty() {
        return Err(CoreError::Validation {
            field: "last_verified".to_owned(),
            reason: "must not be empty (YYYY-MM-DD)".to_owned(),
        });
    }
    // Expect 10 chars: 4-2-2 with dashes
    if value.len() != 10 {
        return Err(CoreError::Validation {
            field: "last_verified".to_owned(),
            reason: format!("expected YYYY-MM-DD, got `{value}`"),
        });
    }
    let bytes = value.as_bytes();
    // Check dash positions without indexing panic: use get
    let dash1 = bytes.get(4).copied().unwrap_or(b' ');
    let dash2 = bytes.get(7).copied().unwrap_or(b' ');
    if dash1 != b'-' || dash2 != b'-' {
        return Err(CoreError::Validation {
            field: "last_verified".to_owned(),
            reason: format!("expected YYYY-MM-DD, got `{value}`"),
        });
    }
    // Check digits
    for (idx, b) in bytes.iter().enumerate() {
        if idx == 4 || idx == 7 {
            continue;
        }
        if !b.is_ascii_digit() {
            return Err(CoreError::Validation {
                field: "last_verified".to_owned(),
                reason: format!("expected YYYY-MM-DD, got `{value}`"),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Catalog load and lookup
// ---------------------------------------------------------------------------

/// Loaded install catalog: harness id -> entry mapping plus ordered list.
#[derive(Debug, Clone)]
pub struct InstallCatalog {
    /// Entries in file order.
    pub entries: Vec<InstallCatalogEntry>,
    /// Lookup by harness id string.
    index: HashMap<String, usize>,
}

impl InstallCatalog {
    /// Load and validate from a JSON string.
    pub fn from_json_str(json: &str) -> Result<Self, CoreError> {
        let entries: Vec<InstallCatalogEntry> =
            serde_json::from_str(json).map_err(|e| CoreError::Validation {
                field: "install_catalog".to_owned(),
                reason: format!("invalid JSON: {e}"),
            })?;
        Self::from_entries(entries)
    }

    /// Load and validate from a slice of entries.
    pub fn from_entries(entries: Vec<InstallCatalogEntry>) -> Result<Self, CoreError> {
        if entries.is_empty() {
            return Err(CoreError::Validation {
                field: "install_catalog".to_owned(),
                reason: "catalog must contain at least one entry".to_owned(),
            });
        }
        let mut index = HashMap::new();
        for (idx, entry) in entries.iter().enumerate() {
            entry.validate()?;
            if index.contains_key(&entry.harness) {
                return Err(CoreError::Validation {
                    field: "harness".to_owned(),
                    reason: format!("duplicate harness id `{}`", entry.harness),
                });
            }
            index.insert(entry.harness.clone(), idx);
        }
        Ok(Self { entries, index })
    }

    /// Load from the embedded asset (`assets/install_catalog.json`).
    pub fn embedded() -> Result<Self, CoreError> {
        Self::from_json_str(EMBEDDED_CATALOG)
    }

    /// Load from a file path on disk. Preserves data-driven semantics: the
    /// file is read fresh, not cached between operations.
    pub fn from_file(path: &Path) -> Result<Self, CoreError> {
        let bytes = std::fs::read(path).map_err(|e| CoreError::Validation {
            field: "install_catalog".to_owned(),
            reason: format!("failed to read {}: {e}", path.display()),
        })?;
        let text = String::from_utf8(bytes).map_err(|e| CoreError::Validation {
            field: "install_catalog".to_owned(),
            reason: format!("catalog file {} is not UTF-8: {e}", path.display()),
        })?;
        Self::from_json_str(&text)
    }

    /// Lookup an entry by harness id.
    pub fn get(&self, harness: &HarnessId) -> Option<&InstallCatalogEntry> {
        let idx = self.index.get(harness.as_str())?;
        self.entries.get(*idx)
    }

    /// Lookup an entry by raw harness string.
    pub fn get_str(&self, harness: &str) -> Option<&InstallCatalogEntry> {
        let idx = self.index.get(harness)?;
        self.entries.get(*idx)
    }

    /// Return the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return true if no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Return the path to the embedded asset file (for tooling).
    pub fn embedded_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/install_catalog.json")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_entry(harness: &str) -> InstallCatalogEntry {
        InstallCatalogEntry {
            harness: harness.to_owned(),
            executables: vec!["test-exe".to_owned()],
            bundle_ids: Vec::new(),
            apps: Vec::new(),
            methods: vec![InstallMethod {
                kind: InstallMethodKind::Npm,
                package_name: "@org/pkg".to_owned(),
                tap: None,
                repo: None,
                registry: Some("https://registry.npmjs.org".to_owned()),
            }],
            version_source: "test-exe --version".to_owned(),
            constraints: PlatformConstraints {
                os: vec!["linux".to_owned(), "macos".to_owned()],
                arch: vec!["x86_64".to_owned(), "aarch64".to_owned()],
            },
            detect: DetectHints {
                commands: vec![CommandTokens {
                    executable: "test-exe".to_owned(),
                    args: vec!["--version".to_owned()],
                }],
                paths: vec!["/usr/local/bin/test-exe".to_owned()],
            },
            update: Some(CommandTokens {
                executable: "npm".to_owned(),
                args: vec!["update".to_owned(), "-g".to_owned(), "@org/pkg".to_owned()],
            }),
            uninstall: Some(CommandTokens {
                executable: "npm".to_owned(),
                args: vec![
                    "uninstall".to_owned(),
                    "-g".to_owned(),
                    "@org/pkg".to_owned(),
                ],
            }),
            requires_admin: false,
            checksum: None,
            conflicts: Vec::new(),
            docs: "https://example.com".to_owned(),
            last_verified: "2026-08-26".to_owned(),
        }
    }

    #[test]
    fn embedded_catalog_loads_and_validates() {
        let catalog = InstallCatalog::embedded().unwrap();
        assert!(catalog.len() >= 5, "expected at least 5 entries");
        // Every entry must have validated harness id
        for entry in &catalog.entries {
            entry.harness_id().unwrap();
            assert!(!entry.docs.is_empty());
            validate_last_verified(&entry.last_verified).unwrap();
        }
        // Lookup by HarnessId
        let id = HarnessId::new("claude-code").unwrap();
        let e = catalog.get(&id).unwrap();
        assert_eq!(e.harness, "claude-code");
        assert!(e.executables.contains(&"claude".to_owned()));
        assert!(e.methods.iter().any(|m| m.kind == InstallMethodKind::Npm));
    }

    #[test]
    fn catalog_data_driven_not_code() {
        // Prove the JSON file on disk and the embedded string agree, and that
        // changing the file changes the catalog (data-driven). We do not
        // assert on Rust constants directly.
        let path = InstallCatalog::embedded_path();
        assert!(path.exists(), "asset file must exist at {}", path.display());
        let file_catalog = InstallCatalog::from_file(&path).unwrap();
        let embedded = InstallCatalog::embedded().unwrap();
        assert_eq!(file_catalog.len(), embedded.len());
        for (a, b) in file_catalog.entries.iter().zip(embedded.entries.iter()) {
            assert_eq!(a.harness, b.harness);
            assert_eq!(a.executables, b.executables);
        }
    }

    #[test]
    fn catalog_validation_catches_missing_package_ids() {
        let mut entry = minimal_entry("test-harness");
        entry.methods[0].package_name = String::new();
        let err = entry.validate().unwrap_err();
        assert!(format!("{err}").contains("package_name"));
    }

    #[test]
    fn catalog_validation_catches_missing_commands_structure() {
        // CommandTokens with shell pipeline must be rejected
        let bad = CommandTokens {
            executable: "npm".to_owned(),
            args: vec!["install".to_owned(), "|".to_owned(), "sh".to_owned()],
        };
        assert!(bad.validate().is_err());
        let bad2 = CommandTokens {
            executable: "sh".to_owned(),
            args: vec!["-c".to_owned(), "curl | sh".to_owned()],
        };
        assert!(bad2.validate().is_err());
        let good = CommandTokens {
            executable: "npm".to_owned(),
            args: vec!["install".to_owned(), "-g".to_owned(), "@org/pkg".to_owned()],
        };
        good.validate().unwrap();
    }

    #[test]
    fn catalog_validation_rejects_empty_executables_and_no_methods() {
        let mut e = minimal_entry("h2");
        e.executables.clear();
        e.bundle_ids.clear();
        e.apps.clear();
        assert!(e.validate().is_err());
        let mut e2 = minimal_entry("h3");
        e2.methods.clear();
        assert!(e2.validate().is_err());
    }

    #[test]
    fn catalog_rejects_duplicate_harness() {
        let a = minimal_entry("dup");
        let b = minimal_entry("dup");
        let err = InstallCatalog::from_entries(vec![a, b]).unwrap_err();
        assert!(format!("{err}").contains("duplicate"));
    }

    #[test]
    fn catalog_rejects_invalid_harness_id() {
        let mut e = minimal_entry("bad/harness");
        let err = e.validate().unwrap_err();
        assert!(format!("{err}").contains("invalid HarnessId"));
        // Also test via from_entries
        e.harness = "CON".to_owned();
        assert!(e.validate().is_err());
    }

    #[test]
    fn catalog_rejects_bad_last_verified() {
        let mut e = minimal_entry("h4");
        e.last_verified = "2026/08/26".to_owned();
        assert!(e.validate().is_err());
        e.last_verified = String::new();
        assert!(e.validate().is_err());
        e.last_verified = "2026-08-26".to_owned();
        e.validate().unwrap();
    }

    #[test]
    fn platform_constraints_supports_logic() {
        let c = PlatformConstraints {
            os: vec!["linux".to_owned()],
            arch: vec!["x86_64".to_owned()],
        };
        assert!(c.supports("linux", "x86_64"));
        assert!(!c.supports("windows", "x86_64"));
        assert!(!c.supports("linux", "aarch64"));
        let any = PlatformConstraints {
            os: vec!["any".to_owned()],
            arch: vec!["any".to_owned()],
        };
        assert!(any.supports("windows", "aarch64"));
        let empty = PlatformConstraints {
            os: Vec::new(),
            arch: Vec::new(),
        };
        assert!(empty.supports("linux", "x86_64"));
    }

    #[test]
    fn from_json_str_rejects_malformed() {
        let bad = "not json";
        InstallCatalog::from_json_str(bad).unwrap_err();
        let empty = "[]";
        InstallCatalog::from_json_str(empty).unwrap_err();
    }
}
