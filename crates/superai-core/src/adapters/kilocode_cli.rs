//! Kilo CLI adapter: `kilo` binary, OpenCode-fork JSONC config at
//! `~/.config/kilo/kilo.jsonc` (XDG relocation, `KILO_CONFIG` file override,
//! `KILO_CONFIG_CONTENT` inline trusted config).

use std::path::{Path, PathBuf};

use crate::adapter::{
    ADAPTER_REVISION, Adapter, ConfigScope, ConfigSurface, DetectionResult, DocumentKind,
    PathResolver, Platform, ProductStatus, RestartBehavior, SurfaceOwnership, VersionResolution,
    WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, Isolation};

/// Catalog id superai registers this harness under.
pub const HARNESS_ID_STR: &str = "kilocode-cli";

/// Name shown for this harness in listings and errors.
pub const DISPLAY_NAME: &str = "Kilo CLI";

/// Binary name resolved on PATH during detection.
pub const EXECUTABLE: &str = "kilo";

/// Environment variable pointing at a config file (trusted, high precedence).
pub const CONFIG_FILE_ENV_VAR: &str = "KILO_CONFIG";

/// Environment variable carrying inline config content (trusted, highest).
pub const INLINE_CONFIG_ENV_VAR: &str = "KILO_CONFIG_CONTENT";

/// Environment variable relocating the OS config home (the root lever).
pub const XDG_CONFIG_ENV_VAR: &str = "XDG_CONFIG_HOME";

/// Research doc whose findings the constants in this file encode.
pub const RESEARCH_DOC: &str = "docs/harness-configs/kilocode-cli.md";

/// Date the research doc was last checked against the harness.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version this adapter maps detected versions to.
pub const SCHEMA_VERSION_STR: &str = "1";

/// Owned selectors inside `kilo.jsonc` (OpenCode-shared keys plus overlays).
pub const OWNED_SELECTORS: &[&str] = &[
    "model",
    "provider",
    "mcp",
    "permission",
    "agent",
    "command",
    "top",
    "agents",
    "experimental",
    "remote_control",
];

/// Concrete adapter for the Kilo CLI.
#[derive(Debug, Clone)]
pub struct KilocodeCliAdapter {
    id: HarnessId,
}

impl KilocodeCliAdapter {
    /// Create a new adapter instance, validating the static harness id.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    fn default_config_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(XDG_CONFIG_ENV_VAR)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir).join("kilo"));
        }
        let home = super::home_dir()?;
        Some(home.join(".config").join("kilo"))
    }

    fn global_config_for_root(root: &Path) -> PathBuf {
        root.join("kilo.jsonc")
    }

    #[expect(
        clippy::excessive_nesting,
        reason = "detection branches are explicit for evidence"
    )]
    #[expect(clippy::unused_self, reason = "uses adapter constants via Self")]
    fn collect_config_evidence(&self, evidence: &mut Vec<String>) {
        match Self::default_config_root() {
            Some(root) => {
                let global = Self::global_config_for_root(&root);
                if global.exists() {
                    evidence.push(format!("kilo.jsonc found at {}", global.display()));
                    match std::fs::read_to_string(&global) {
                        Ok(text) if text.contains("mcp") || text.contains("model") => {
                            evidence.push("kilo.jsonc contains mcp/model".to_owned());
                        }
                        Ok(_) => {}
                        Err(err) => evidence
                            .push(format!("config unreadable at {}: {err}", global.display())),
                    }
                } else {
                    evidence.push(format!("kilo.jsonc missing at {}", global.display()));
                }
                let tui = root.join("tui.jsonc");
                if tui.exists() {
                    evidence.push(format!("tui.jsonc present at {}", tui.display()));
                }
            }
            None => {
                evidence.push("could not resolve default config root (no HOME)".to_owned());
            }
        }
        for var in [CONFIG_FILE_ENV_VAR, INLINE_CONFIG_ENV_VAR] {
            if let Ok(val) = std::env::var(var)
                && !val.trim().is_empty()
            {
                if var.contains("CONTENT") {
                    evidence.push(format!("{var} is set (inline trusted config)"));
                } else {
                    evidence.push(format!("{var} set to {val}"));
                }
            }
        }
        if Path::new("./kilo.jsonc").exists() {
            evidence.push("./kilo.jsonc present in cwd (project)".to_owned());
        }
        if Path::new("./.kilo/kilo.jsonc").exists() {
            evidence.push("./.kilo/kilo.jsonc present in cwd (project, wins)".to_owned());
        }
    }
}

impl Default for KilocodeCliAdapter {
    fn default() -> Self {
        let id = HarnessId::from_validated_const(HARNESS_ID_STR);
        Self { id }
    }
}

impl Adapter for KilocodeCliAdapter {
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
        super::desktop_platforms()
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

    fn detection(&self) -> DetectionResult {
        let mut evidence = Vec::new();
        let mut version: Option<String> = None;
        let mut binary_path: Option<PathBuf> = None;

        match super::find_in_path(&[EXECUTABLE]) {
            Some(path) => {
                evidence.push(format!(
                    "found binary `{}` at {}",
                    EXECUTABLE,
                    path.display()
                ));
                match super::probe_version(&path) {
                    Some(v) => {
                        evidence.push(format!("version `{v}` via `{EXECUTABLE} --version`"));
                        version = Some(v);
                    }
                    None => {
                        evidence.push(format!(
                            "version probe failed for `{EXECUTABLE} --version` (timeout or non-zero)"
                        ));
                    }
                }
                binary_path = Some(path);
            }
            None => {
                evidence.push(format!("binary `{EXECUTABLE}` not found in PATH"));
            }
        }

        self.collect_config_evidence(&mut evidence);

        let present = super::install_presence(binary_path.is_some(), version.is_some());

        let confidence =
            super::detection_confidence(binary_path.is_some(), version.is_some(), false);
        DetectionResult::new(present, version, evidence, confidence)
    }

    fn version_resolution_from(&self, detection: &DetectionResult) -> VersionResolution {
        super::resolution_from_detection(detection, "kilocode-cli", SCHEMA_VERSION_STR)
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        let global_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/kilo/kilo.jsonc"),
            Some("$XDG_CONFIG_HOME/kilo/kilo.jsonc"),
            Some("%USERPROFILE%\\.config\\kilo\\kilo.jsonc"),
            "~/.config/kilo/kilo.jsonc",
        );
        let mut global = ConfigSurface::new(
            "kilo.jsonc",
            global_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        global.precedence = 10;
        global.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        global.backup_required = true;
        global.restart_behavior = RestartBehavior::Reload;
        surfaces.push(global);

        let project_resolver = PathResolver::fallback_only("./kilo.jsonc / ./.kilo/kilo.jsonc");
        let mut project = ConfigSurface::new(
            "project kilo.jsonc",
            project_resolver,
            DocumentKind::Jsonc,
            ConfigScope::ProjectWorkspace,
            SurfaceOwnership::UserEditable,
        );
        project.precedence = 14;
        project.owned_selectors = OWNED_SELECTORS.iter().map(|s| (*s).to_owned()).collect();
        project.backup_required = false;
        surfaces.push(project);

        let tui_resolver = PathResolver::new(
            Some("$XDG_CONFIG_HOME/kilo/tui.jsonc"),
            Some("$XDG_CONFIG_HOME/kilo/tui.jsonc"),
            Some("%USERPROFILE%\\.config\\kilo\\tui.jsonc"),
            "~/.config/kilo/tui.jsonc",
        );
        let mut tui = ConfigSurface::new(
            "tui.jsonc",
            tui_resolver,
            DocumentKind::Jsonc,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        tui.precedence = 9;
        tui.owned_selectors = vec!["keybinds".to_owned()];
        tui.backup_required = true;
        surfaces.push(tui);

        let inline_resolver =
            PathResolver::fallback_only("$KILO_CONFIG_CONTENT (inline trusted, highest)");
        let mut inline = ConfigSurface::new(
            "inline config",
            inline_resolver,
            DocumentKind::Jsonc,
            ConfigScope::SessionInline,
            SurfaceOwnership::UserEditable,
        );
        inline.precedence = 20;
        inline.backup_required = false;
        surfaces.push(inline);

        surfaces
    }

    fn supported_operations(&self) -> Vec<(String, AdapterSupport)> {
        super::all_operations_full()
    }

    fn plan_mirror_exclusions(&self) -> Vec<String> {
        vec![
            "sessions/*".to_owned(),
            "logs/*".to_owned(),
            "cache/*".to_owned(),
            "*.log".to_owned(),
            "*.tmp".to_owned(),
            "*.db".to_owned(),
        ]
    }

    fn plan_wrapper(&self, instance: &Instance) -> Result<WrapperPlan, CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        let config_home = instance.config_root.as_path().join("config");
        let mut plan = WrapperPlan::new("relocated config home via XDG_CONFIG_HOME");
        plan.env_vars.push((
            XDG_CONFIG_ENV_VAR.to_owned(),
            config_home.display().to_string(),
        ));
        plan.description = format!(
            " Wrapper sets {}={} so kilo.jsonc resolves inside the instance and execs `{}`",
            XDG_CONFIG_ENV_VAR,
            config_home.display(),
            EXECUTABLE
        );
        plan.shared_state_warnings = vec![
            "kilo auth gateway credentials live at an undocumented path and stay shared across profiles"
                .to_owned(),
            "{env:VAR} interpolation resolves only in trusted config: global files, KILO_CONFIG/KILO_CONFIG_CONTENT, or org-managed config"
                .to_owned(),
        ];
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            "~/.config/kilo".to_owned(),
            "$XDG_CONFIG_HOME/kilo".to_owned(),
            "kilo.jsonc".to_owned(),
            ".kilo/kilo.jsonc".to_owned(),
            ".kilo/tui.json".to_owned(),
        ]
    }

    fn validate_instance(&self, instance: &Instance) -> Result<(), CoreError> {
        super::ensure_instance_harness(&self.id, instance)?;
        instance.validate()?;
        match instance.isolation {
            Isolation::RelocatedRoot | Isolation::Unknown => Ok(()),
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!("kilocode-cli requires isolation relocated_root, got {other}"),
            }),
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        super::skill_modes_link_first()
    }

    fn mcp_decl(&self) -> Option<crate::adapter::McpAdapterDecl> {
        Some(
            crate::adapter::McpAdapterDecl::new(
                "kilo.jsonc",
                "mcp",
                DocumentKind::Jsonc,
                ConfigScope::User,
                RestartBehavior::Reload,
            )
            .with_read_only(
                "jsonc writes refuse (LossyWrite) until a preserving codec exists; inspect/diff only",
            ),
        )
    }

    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "a `kilo plugin <module>` subcommand is listed but no plugin layout is documented (kilocode-cli.md sections 4, 7)",
        )
    }
    fn usage_absence_reason(&self) -> Option<&'static str> {
        Some(
            "usage truth is app.kilo.ai's tRPC API; the local auth file holds credentials, not usage",
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{HARNESS_ID_STR, KilocodeCliAdapter, OWNED_SELECTORS, XDG_CONFIG_ENV_VAR};
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstallPresence, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> KilocodeCliAdapter {
        KilocodeCliAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-kilocode-1").unwrap(),
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
        let entry = crate::harness_catalog::find_by_id(HARNESS_ID_STR).unwrap();
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(a.display_name(), entry.display_name);
        assert_eq!(a.product_status(), entry.product_status);
        assert_eq!(a.research_doc_link(), entry.research_doc);
        assert_eq!(a.last_verified_date(), entry.last_verified);
    }

    #[test]
    fn supported_platforms_covers_all() {
        let a = adapter();
        let os_set: HashSet<String> = a
            .supported_platforms()
            .iter()
            .map(|p| p.os.to_string())
            .collect();
        assert!(os_set.contains("linux"));
        assert!(os_set.contains("macos"));
        assert!(os_set.contains("windows"));
    }

    #[test]
    fn detection_returns_evidence_and_confidence() {
        let a = adapter();
        let result = a.detection();
        assert!(!result.evidence.is_empty());
        match result.present {
            InstallPresence::Absent => {
                assert!(result.version.is_none());
            }
            InstallPresence::Present => {
                assert!(result.version.is_some());
            }
            InstallPresence::UnknownVersion => {
                assert!(result.evidence.iter().any(|e| e.contains("found binary")));
            }
            InstallPresence::Broken => {
                assert!(!result.evidence.is_empty());
            }
        }
    }

    #[test]
    fn version_resolution_maps_detected() {
        let a = adapter();
        let res = a.version_resolution();
        if res.detected_version.is_some() {
            assert_eq!(
                res.schema_version.as_deref(),
                Some(super::SCHEMA_VERSION_STR)
            );
            assert!(res.compatible);
        } else {
            assert!(!res.compatible);
            assert!(res.schema_version.is_none());
        }
        assert!(!res.notes.is_empty());
    }

    #[test]
    fn config_surfaces_include_global_and_project_jsonc() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        assert!(surfaces.len() >= 4);
        let global = surfaces
            .iter()
            .find(|s| s.id == "kilo.jsonc")
            .expect("kilo.jsonc surface must exist");
        assert_eq!(global.kind, DocumentKind::Jsonc);
        assert_eq!(global.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(global.scope, ConfigScope::User);
        assert!(global.backup_required);
        for sel in OWNED_SELECTORS {
            assert!(global.owned_selectors.contains(&(*sel).to_owned()));
        }

        for other in ["project kilo.jsonc", "tui.jsonc", "inline config"] {
            assert!(surfaces.iter().any(|s| s.id == other), "missing {other}");
        }

        let inline = surfaces.iter().find(|s| s.id == "inline config").unwrap();
        assert_eq!(inline.scope, ConfigScope::SessionInline);
    }

    #[test]
    fn supported_operations_cover_full() {
        let a = adapter();
        let ops = a.supported_operations();
        for (_, support) in &ops {
            assert_eq!(*support, AdapterSupport::Full);
        }
        let names: HashSet<String> = ops.iter().map(|(n, _)| n.clone()).collect();
        for required in [
            "detect",
            "read_config",
            "write_config",
            "manage_mcp",
            "plan_wrapper",
        ] {
            assert!(names.contains(required), "missing op {required}");
        }
    }

    #[test]
    fn plan_mirror_exclusions_cover_sessions_and_dbs() {
        let a = adapter();
        let exclusions = a.plan_mirror_exclusions();
        let is_excluded = |file: &str| crate::adapters::exclusion_matches(&exclusions, file);
        assert!(!is_excluded("kilo.jsonc"));
        assert!(!is_excluded("tui.jsonc"));
        assert!(is_excluded("sessions/abc.json"));
        assert!(is_excluded("state.db"));
    }

    #[test]
    fn plan_wrapper_sets_xdg_config_home() {
        let tmp_root = crate::test_util::tmp_abs_str(".kilo-work");
        let a = adapter();
        let inst = sample_instance_with_root(&tmp_root);
        let plan = a.plan_wrapper(&inst).unwrap();
        let (key, val) = plan
            .env_vars
            .iter()
            .find(|(k, _)| k == XDG_CONFIG_ENV_VAR)
            .unwrap();
        assert_eq!(key, XDG_CONFIG_ENV_VAR);
        assert!(
            val.starts_with(tmp_root.as_str()),
            "XDG_CONFIG_HOME must point inside the instance root, got {val}"
        );
        assert!(!plan.description.is_empty());
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kilo-work"));
        inst.harness = HarnessId::new("kilo-code").unwrap();
        let err = a.plan_wrapper(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_include_config_root_and_project_files() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(candidates.iter().any(|c| c.contains(".config/kilo")));
        assert!(candidates.iter().any(|c| c.contains("XDG_CONFIG_HOME")));
        assert!(candidates.iter().any(|c| c == "kilo.jsonc"));
    }

    #[test]
    fn validate_instance_accepts_relocated_root() {
        let a = adapter();
        let inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kilo-work"));
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kilo-work"));
        inst.isolation = Isolation::EnvOnly;
        let err = a.validate_instance(&inst).unwrap_err();
        match err {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn validate_instance_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".kilo-work"));
        inst.harness = HarnessId::new("aider").unwrap();
        assert!(a.validate_instance(&inst).is_err());
    }

    fn fixtures_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/kilocode_cli")
    }

    fn fixture_path(name: &str) -> PathBuf {
        fixtures_root().join(name)
    }

    #[test]
    fn fixture_missing_file_loads_as_empty() {
        let path = fixture_path("nonexistent.jsonc");
        let map = superai_config::jsonc::load(&path).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn fixture_minimal_parses() {
        let path = fixture_path("kilo.minimal.jsonc");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::jsonc::load(&path).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn fixture_populated_carriers_schema_model_and_mcp() {
        let path = fixture_path("kilo.populated.jsonc");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let map = superai_config::jsonc::load(&path).unwrap();
        assert_eq!(
            map.get("$schema").and_then(|v| v.as_str()),
            Some("https://app.kilo.ai/config.json")
        );
        assert!(map.contains_key("model"));
        assert!(map.contains_key("mcp"));
    }

    #[test]
    fn fixture_foreign_preserves_unknown_keys_on_edit() {
        let path = fixture_path("kilo.foreign.jsonc");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let original = superai_config::jsonc::load(&path).unwrap();
        assert!(
            original.contains_key("vendorKey") || original.contains_key("foreignKey"),
            "foreign fixture must carry a foreign key"
        );
        let dir = crate::test_util::temp_dir_unique("kilocode");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join("kilo.foreign.copy.jsonc");
        std::fs::copy(&path, &tmp).unwrap();
        superai_config::jsonc::edit(&tmp, |map| {
            map.insert(
                "model".to_owned(),
                serde_json::Value::String("kilocode/other".to_owned()),
            );
        })
        .unwrap();
        let after = superai_config::jsonc::load(&tmp).unwrap();
        assert!(
            after.contains_key("vendorKey") || after.contains_key("foreignKey"),
            "foreign keys must be preserved, got {after:?}"
        );
        drop(std::fs::remove_file(&tmp));
    }

    #[test]
    fn fixture_malformed_fails_to_parse() {
        let path = fixture_path("kilo.malformed.jsonc");
        assert!(path.exists(), "fixture missing: {}", path.display());
        let result = superai_config::jsonc::load(&path);
        assert!(result.is_err(), "malformed fixture must fail to parse");
    }

    #[test]
    fn mcp_decl_is_read_only_jsonc() {
        let a = adapter();
        let decl = a.mcp_decl().unwrap();
        assert_eq!(decl.dest_file, "kilo.jsonc");
        assert_eq!(decl.dest_key, "mcp");
        assert_eq!(decl.kind, DocumentKind::Jsonc);
        assert!(
            decl.read_only
                .as_deref()
                .is_some_and(|r| r.contains("LossyWrite")),
            "jsonc dest must refuse writes honestly"
        );
        assert!(a.mcp_absence_reason().is_none());
    }

    #[test]
    fn plugin_absence_names_the_subcommand_gap() {
        let a = adapter();
        let reason = a.plugin_absence_reason().unwrap();
        assert!(reason.contains("kilo plugin"));
        assert!(a.plugin_decl().is_none());
    }

    #[test]
    fn adapter_is_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
        assert!(!boxed.config_surfaces().is_empty());
        assert!(!boxed.plan_mirror_exclusions().is_empty());
    }
}
