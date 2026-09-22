//! Kiro adapter: relocated root via `KIRO_HOME` over `settings/cli.json`,
//! `mcp.json`, `permissions.yaml`; read-only until BYO and schema gaps close.

use std::path::PathBuf;

use crate::adapter::{
    ADAPTER_REVISION, Adapter, Arch, ConfigScope, ConfigSurface, DetectionConfidence,
    DetectionResult, DocumentKind, Os, PathResolver, Platform, ProductStatus, RestartBehavior,
    SurfaceOwnership, VersionResolution, WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, InstallPresence, Isolation};

/// Harness identifier for Kiro.
pub const HARNESS_ID_STR: &str = "kiro";

/// Human display name.
pub const DISPLAY_NAME: &str = "Kiro CLI/IDE";

/// Primary executable name: the only binary the vendor installer ships
/// (live-verified 2026-09-18; the installer materializes no bare `kiro`).
pub const EXECUTABLE: &str = "kiro-cli";

/// Alias: bare `kiro` exists only where a wrapper or bridge provides it.
pub const EXECUTABLE_ALT: &str = "kiro";

/// Environment variable that relocates the config root.
pub const CONFIG_ENV_VAR: &str = "KIRO_HOME";

/// Default config root fallback.
pub const DEFAULT_CONFIG_ROOT_FALLBACK: &str = "~/.kiro";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/kiro.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-08-25";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

/// `ReadOnly` reason.
pub const READONLY_REASON: &str = "read-only until research gaps close: BYO endpoint not supported, full cli.json schema unverified, AWS credential isolation via AWS_PROFILE/AWS_CONFIG_FILE";

/// Concrete adapter for Kiro (`ReadOnly`).
#[derive(Debug, Clone)]
pub struct KiroAdapter {
    id: HarnessId,
}

impl KiroAdapter {
    /// Create a new adapter.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    /// Borrow harness id.
    pub fn harness_id(&self) -> &HarnessId {
        &self.id
    }

    /// Executable name.
    pub fn executable_name(&self) -> &str {
        EXECUTABLE
    }

    /// Config env var.
    pub fn config_env_var(&self) -> &str {
        CONFIG_ENV_VAR
    }

    /// `ReadOnly` reason.
    pub fn readonly_reason(&self) -> &str {
        READONLY_REASON
    }

    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(CONFIG_ENV_VAR)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir));
        }
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())?;
        if home.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(home).join(".kiro"))
    }

    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        evidence.push(format!("read-only: {READONLY_REASON}"));
        match Self::default_config_root() {
            Some(root) => {
                if root.exists() {
                    evidence.push(format!("config root exists at {}", root.display()));
                    let cli_json = root.join("settings").join("cli.json");
                    if cli_json.exists() {
                        evidence.push(format!("cli.json found at {}", cli_json.display()));
                    } else {
                        evidence.push(format!("cli.json missing at {}", cli_json.display()));
                    }
                    let mcp = root.join("settings").join("mcp.json");
                    if mcp.exists() {
                        evidence.push(format!("mcp.json found at {}", mcp.display()));
                    }
                    let permissions = root.join("settings").join("permissions.yaml");
                    if permissions.exists() {
                        evidence.push(format!(
                            "permissions.yaml found at {}",
                            permissions.display()
                        ));
                    }
                } else {
                    evidence.push(format!("config root missing at {}", root.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
    }
}

impl Default for KiroAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "kiro is static valid HarnessId")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for KiroAdapter {
    fn id(&self) -> HarnessId {
        self.id.clone()
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    fn product_status(&self) -> ProductStatus {
        ProductStatus::Active
    }

    fn supported_platforms(&self) -> Vec<Platform> {
        vec![
            Platform::new(Os::Linux, Arch::Any),
            Platform::new(Os::Macos, Arch::Any),
            Platform::new(Os::Windows, Arch::Any),
        ]
    }

    fn adapter_revision(&self) -> &str {
        ADAPTER_REVISION
    }

    fn research_doc_link(&self) -> &str {
        RESEARCH_DOC
    }

    fn last_verified_date(&self) -> &str {
        LAST_VERIFIED
    }

    #[expect(clippy::single_match_else, reason = "detection branching explicit")]
    fn detection(&self) -> DetectionResult {
        let mut evidence = Vec::new();
        let mut version: Option<String> = None;
        let mut binary_path: Option<PathBuf> = None;

        match super::find_in_path(&[EXECUTABLE, EXECUTABLE_ALT]) {
            Some(path) => {
                evidence.push(format!(
                    "found binary `{}` at {}",
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(EXECUTABLE),
                    path.display()
                ));
                match super::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `{EXECUTABLE} --version`"));
                        version = Some(v);
                    }
                    None => {
                        evidence.push(format!("version probe failed for `{EXECUTABLE} --version`"));
                    }
                }
                binary_path = Some(path);
            }
            None => {
                evidence.push(format!("binary `{EXECUTABLE}` not found in PATH"));
                evidence.push(format!(
                    "binary `{EXECUTABLE_ALT}` (alias) not found in PATH"
                ));
            }
        }

        self.collect_config_evidence(&mut evidence);

        let present = match (&binary_path, &version) {
            (Some(_), Some(_)) => InstallPresence::Present,
            (Some(_), None) => InstallPresence::UnknownVersion,
            (None, _) => InstallPresence::Absent,
        };

        let confidence = if present == InstallPresence::Absent {
            if evidence.iter().any(|e| e.contains("config root exists")) {
                DetectionConfidence::Low
            } else {
                DetectionConfidence::High
            }
        } else if binary_path.is_some() && version.is_none() {
            DetectionConfidence::Medium
        } else {
            DetectionConfidence::High
        };

        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution(&self) -> VersionResolution {
        let detection = self.detection();
        if let Some(v) = detection.version {
            let mut notes = Vec::new();
            notes.push(format!("detected kiro version {v}"));
            notes.push(format!("mapped to schema version {SCHEMA_VERSION_STR}"));
            notes.push(format!("read-only: {READONLY_REASON}"));
            let mut res =
                VersionResolution::new(Some(v), Some(SCHEMA_VERSION_STR.to_owned()), false);
            res.notes = notes;
            res
        } else {
            let mut res = VersionResolution::unknown();
            res.notes = detection.evidence;
            res.notes.push(format!("read-only: {READONLY_REASON}"));
            res
        }
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let cli_resolver = PathResolver::new(
            Some("$KIRO_HOME/settings/cli.json"),
            Some("$KIRO_HOME/settings/cli.json"),
            Some("%KIRO_HOME%\\settings\\cli.json"),
            "~/.kiro/settings/cli.json",
        );
        let mut cli_surface = ConfigSurface::new(
            "settings/cli.json",
            cli_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        cli_surface.precedence = 10;
        cli_surface.owned_selectors = vec!["chat.defaultModel".to_owned()];
        cli_surface.backup_required = true;
        cli_surface.restart_behavior = RestartBehavior::Reload;
        surfaces.push(cli_surface);

        let mcp_resolver = PathResolver::new(
            Some("$KIRO_HOME/settings/mcp.json"),
            Some("$KIRO_HOME/settings/mcp.json"),
            Some("%KIRO_HOME%\\settings\\mcp.json"),
            "~/.kiro/settings/mcp.json",
        );
        let mut mcp_surface = ConfigSurface::new(
            "settings/mcp.json",
            mcp_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        mcp_surface.precedence = 12;
        mcp_surface.owned_selectors = vec!["mcpServers".to_owned()];
        mcp_surface.backup_required = true;
        surfaces.push(mcp_surface);

        let permissions_resolver = PathResolver::new(
            Some("$KIRO_HOME/settings/permissions.yaml"),
            Some("$KIRO_HOME/settings/permissions.yaml"),
            Some("%KIRO_HOME%\\settings\\permissions.yaml"),
            "~/.kiro/settings/permissions.yaml",
        );
        let mut permissions_surface = ConfigSurface::new(
            "settings/permissions.yaml",
            permissions_resolver,
            DocumentKind::Yaml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        permissions_surface.precedence = 8;
        permissions_surface.backup_required = true;
        surfaces.push(permissions_surface);

        let agents_resolver = PathResolver::new(
            Some("$KIRO_HOME/agents/<name>.md"),
            Some("$KIRO_HOME/agents/<name>.md"),
            Some("%KIRO_HOME%\\agents\\<name>.md"),
            "~/.kiro/agents/<name>.md",
        );
        let mut agents_surface = ConfigSurface::new(
            "agents",
            agents_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        agents_surface.precedence = 14;
        agents_surface.backup_required = false;
        surfaces.push(agents_surface);

        let skills_resolver = PathResolver::new(
            Some("$KIRO_HOME/skills/<name>/SKILL.md"),
            Some("$KIRO_HOME/skills/<name>/SKILL.md"),
            Some("%KIRO_HOME%\\skills\\<name>\\SKILL.md"),
            "~/.kiro/skills/<name>/SKILL.md",
        );
        let mut skills_surface = ConfigSurface::new(
            "skills",
            skills_resolver,
            DocumentKind::TextFragment,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        skills_surface.precedence = 6;
        skills_surface.backup_required = false;
        surfaces.push(skills_surface);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        vec![
            ("detect".to_owned(), AdapterSupport::ReadOnly),
            ("read_config".to_owned(), AdapterSupport::ReadOnly),
            ("write_config".to_owned(), AdapterSupport::ReadOnly),
            ("manage_skills".to_owned(), AdapterSupport::ReadOnly),
            ("manage_mcp".to_owned(), AdapterSupport::ReadOnly),
            ("manage_plugins".to_owned(), AdapterSupport::ReadOnly),
            ("configure_provider".to_owned(), AdapterSupport::ReadOnly),
            ("plan_mirror".to_owned(), AdapterSupport::ReadOnly),
            ("plan_wrapper".to_owned(), AdapterSupport::ReadOnly),
            ("scan_candidates".to_owned(), AdapterSupport::ReadOnly),
            ("validate_instance".to_owned(), AdapterSupport::ReadOnly),
        ]
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "history/*".to_owned(),
            "sessions/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
            "telemetry/*".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        if instance.harness != self.id {
            return Err(CoreError::Validation {
                field: "harness".to_owned(),
                reason: format!(
                    "instance harness `{}` does not match adapter `{}`",
                    instance.harness, self.id
                ),
            });
        }
        instance.validate()?;
        let mut plan = WrapperPlan::new("relocated-root via KIRO_HOME (read-only)");
        plan.env_vars
            .push((CONFIG_ENV_VAR.to_owned(), instance.config_root.to_string()));
        plan.description = format!(
            " Wrapper sets {CONFIG_ENV_VAR}={} and execs `{EXECUTABLE}` (or the `{EXECUTABLE_ALT}` alias) (read-only; writes blocked: {READONLY_REASON})",
            instance.config_root
        );
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.kiro/settings/cli.json".to_owned(),
            "~/.kiro/settings/mcp.json".to_owned(),
            "~/.kiro/settings/permissions.yaml".to_owned(),
            "~/.kiro/agents".to_owned(),
            "~/.kiro/skills".to_owned(),
            "$KIRO_HOME/settings/cli.json via KIRO_HOME".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        if instance.harness != self.id {
            return Err(CoreError::Validation {
                field: "harness".to_owned(),
                reason: format!("expected harness `{}`, got `{}`", self.id, instance.harness),
            });
        }
        instance.validate()?;
        match instance.isolation {
            Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "kiro expects isolation relocated_root via KIRO_HOME, got {other}; {READONLY_REASON}"
                ),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        Vec::new()
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(
            crate::adapter::McpAdapterDecl::new(
                "settings/mcp.json",
                "mcpServers",
                DocumentKind::Json,
                ConfigScope::User,
                RestartBehavior::Reload,
            )
            .with_read_only("read-only supported harness (BYO unsupported); inspect/diff only"),
        )
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some("no plugin mechanism documented (kiro.md)")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        DISPLAY_NAME, EXECUTABLE, EXECUTABLE_ALT, HARNESS_ID_STR, KiroAdapter, RESEARCH_DOC,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> KiroAdapter {
        KiroAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-kiro-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
        }
    }

    #[test]
    fn adapter_identity() {
        let a = adapter();
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(a.display_name(), DISPLAY_NAME);
        assert_eq!(a.executable_name(), EXECUTABLE);
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert!(!a.last_verified_date().is_empty());
        assert!(a.readonly_reason().contains("BYO"));
    }

    #[test]
    fn supported_platforms_covers_all() {
        let a = adapter();
        let platforms = a.supported_platforms();
        assert!(platforms.len() >= 3);
        let os_set: HashSet<String> = platforms.iter().map(|p| p.os.to_string()).collect();
        assert!(os_set.contains("linux"));
        assert!(os_set.contains("macos"));
        assert!(os_set.contains("windows"));
    }

    #[test]
    fn detection_returns_evidence_with_readonly_reason() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        assert!(result.evidence.iter().any(|e| e.contains("read-only")));
    }

    #[test]
    fn version_resolution_is_not_compatible() {
        let a = adapter();
        let res = a.version_resolution();
        assert!(!res.compatible);
        assert!(!res.notes.is_empty());
        assert!(res.notes.iter().any(|n| n.contains("read-only")));
    }

    #[test]
    fn parse_version_output_cases() {
        let cases = vec![
            ("kiro-cli 2.22.0", Some("2.22.0")),
            ("kiro 0.5.1", Some("0.5.1")),
            ("0.3.0", Some("0.3.0")),
            ("v1.0.0", Some("1.0.0")),
            ("", None),
            ("not a version", None),
        ];
        for (input, expected) in cases {
            let got = crate::adapters::parse_version_output(input);
            assert_eq!(got.as_deref(), expected, "input: {input:?}");
        }
    }

    /// Live-probe regression pin (2026-09-18): the vendor installer ships
    /// `kiro-cli` only; bare `kiro` is a wrapper/bridge alias.
    #[test]
    fn executable_pins_vendor_name_with_alias() {
        assert_eq!(EXECUTABLE, "kiro-cli");
        assert_eq!(EXECUTABLE_ALT, "kiro");
        assert_ne!(EXECUTABLE, EXECUTABLE_ALT);
        assert_eq!(adapter().executable_name(), "kiro-cli");
    }

    #[test]
    fn config_surfaces_include_cli_and_mcp() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 3);
        let cli = surfaces
            .iter()
            .find(|s| s.id == "settings/cli.json")
            .expect("settings/cli.json must exist");
        assert_eq!(cli.kind, DocumentKind::Json);
        assert_eq!(cli.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(cli.scope, ConfigScope::User);
        let mcp = surfaces
            .iter()
            .find(|s| s.id == "settings/mcp.json")
            .expect("settings/mcp.json must exist");
        assert_eq!(mcp.kind, DocumentKind::Json);
        let perm = surfaces
            .iter()
            .find(|s| s.id == "settings/permissions.yaml")
            .expect("permissions.yaml must exist");
        assert_eq!(perm.kind, DocumentKind::Yaml);
    }

    #[test]
    fn supported_operations_are_read_only() {
        let a = adapter();
        let ops = a.supported_operations();
        for (_, support) in ops {
            assert_eq!(support, AdapterSupport::ReadOnly);
        }
    }

    #[test]
    fn plan_wrapper_succeeds_but_marks_readonly() {
        let tmp_root = crate::test_util::tmp_abs_str(".kiro-work");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars
                .iter()
                .any(|(k, v)| k == "KIRO_HOME" && v == tmp_root.as_str())
        );
        assert!(plan.description.contains("read-only"));
    }

    #[test]
    fn scan_candidates_include_kiro_paths() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains("cli.json")));
        assert!(candidates.iter().any(|c| c.contains("KIRO_HOME")));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kiro-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kiro-work"));
        inst.isolation = Isolation::FixedPathSingle;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            crate::error::CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn supported_skill_modes_is_empty() {
        let a = adapter();
        assert!(a.supported_skill_modes().is_empty());
    }
}
