//! `ChatGPT` Desktop adapter — read-only view of the `~/.codex` store shared
//! with codex-cli.
//!
//! Research source: `docs/harness-configs/chatgpt-desktop.md` (verified
//! 2026-09-18; evidence at `.z-workflow/evidence/desktop-research/`).
//! The unified `ChatGPT` desktop app (Chat + Work + Codex; macOS 14+/Windows
//! since 2026-07-09, Linux PREVIEW since 2026-08-11) has NO local config of
//! its own that is officially documented: officially it "picks up your
//! session history and configuration from the Codex CLI and IDE extension" —
//! the `~/.codex` store (`config.toml` incl. the desktop-only
//! `desktop.custom_file_handlers` key and `[mcp_servers.<id>]`, `auth.json`,
//! profiles, sessions). That store BELONGS to the `codex-cli` harness, so
//! this adapter is `ReadOnly`, declares NO MCP destination of its own (the
//! Chat surface is remote-MCP-only per the official position), and warns
//! `shares ~/.codex with codex-cli`. App-level relocation is verified-absent
//! (whether the GUI honors `CODEX_HOME` is undocumented) — aliasing routes
//! through codex-cli, so `plan_wrapper` declares no env vars of its own.

use std::path::PathBuf;

use superai_config::document::ValueType;

use crate::adapter::{
    ADAPTER_REVISION, Adapter, Arch, ConfigScope, ConfigSurface, DetectionConfidence,
    DetectionResult, DocumentKind, Os, PathResolver, Platform, ProductStatus, RestartBehavior,
    RootShape, SurfaceOwnership, SurfaceSchema, VersionResolution, WrapperPlan,
};
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::instance::Instance;
use crate::state::{AdapterSupport, InstallPresence, Isolation};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Harness identifier for the `ChatGPT` desktop app.
pub const HARNESS_ID_STR: &str = "chatgpt-desktop";

/// Human display name.
pub const DISPLAY_NAME: &str = "ChatGPT Desktop (Codex)";

/// The shared Codex store this app reads (owned by the codex-cli harness).
pub const SHARED_STORE_HINT: &str = "~/.codex";

/// Env var that relocates the shared store for the CLI only — documented for
/// codex-cli; whether the GUI app honors it is NOT documented.
pub const CODEX_HOME_ENV_VAR: &str = "CODEX_HOME";

/// Desktop-only key inside the shared `config.toml` (official
/// learn.chatgpt.com config-reference: "User-level only").
pub const DESKTOP_OWNED_SELECTOR: &str = "desktop.custom_file_handlers";

/// Official statement on local MCP in Chat (help center 12584461).
pub const REMOTE_MCP_POSITION: &str =
    "Chat connects to remote MCP servers only (official: local stdio is \"Not directly\")";

/// Shared-state warning pinned in the wrapper plan and catalog.
pub const SHARED_STATE_WARNING: &str = "shares ~/.codex with codex-cli";

/// Verified-absent relocation note (chatgpt-desktop.md section 5).
pub const NO_RELOCATION_NOTE: &str = "no app-level relocation documented; CODEX_HOME is a CLI-documented knob — \
whether the GUI honors it is undocumented";

/// Research document link.
pub const RESEARCH_DOC: &str = "docs/harness-configs/chatgpt-desktop.md";

/// Last verified date.
pub const LAST_VERIFIED: &str = "2026-09-18";

/// Schema version.
pub const SCHEMA_VERSION_STR: &str = "1";

// ---------------------------------------------------------------------------
// Adapter struct
// ---------------------------------------------------------------------------

/// Concrete adapter for the `ChatGPT` desktop app (`ReadOnly`).
#[derive(Debug, Clone)]
pub struct ChatGptDesktopAdapter {
    id: HarnessId,
}

impl ChatGptDesktopAdapter {
    /// Create a new adapter instance.
    pub fn new() -> Result<Self, CoreError> {
        let id = HarnessId::new(HARNESS_ID_STR)?;
        Ok(Self { id })
    }

    /// Borrow harness id.
    pub fn harness_id(&self) -> &HarnessId {
        &self.id
    }

    /// The shared store root: `$CODEX_HOME` when set (the CLI-documented
    /// knob), else the default `~/.codex`. Read-only for this adapter.
    fn shared_store_root() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var(CODEX_HOME_ENV_VAR)
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
        Some(PathBuf::from(home).join(".codex"))
    }

    /// Collect filesystem evidence for detection. The GUI binary name is
    /// unverified, so evidence keys on the shared store — and must say that
    /// the same files indicate a codex-cli install (stores are shared and
    /// not distinguishable at file level).
    #[expect(clippy::excessive_nesting, reason = "evidence branches explicit")]
    fn collect_config_evidence(evidence: &mut Vec<String>) {
        evidence.push(format!(
            "app reads the codex-cli store {SHARED_STORE_HINT} ({SHARED_STATE_WARNING})"
        ));
        match Self::shared_store_root() {
            Some(root) => {
                let config = root.join("config.toml");
                if config.exists() {
                    evidence.push(format!("shared config.toml exists at {}", config.display()));
                    if let Ok(text) = std::fs::read_to_string(&config)
                        && text.contains("desktop.")
                    {
                        evidence.push(
                            "config.toml carries desktop.* keys (app-written marker)".to_owned(),
                        );
                    }
                } else {
                    evidence.push(format!(
                        "shared config.toml missing at {} (app not configured)",
                        config.display()
                    ));
                }
                let auth = root.join("auth.json");
                if auth.exists() {
                    evidence.push(format!(
                        "auth.json exists at {} (secret-bearing, detect-only)",
                        auth.display()
                    ));
                }
            }
            None => evidence.push("could not resolve home for the shared store".to_owned()),
        }
        evidence.push(format!(
            "GUI binary name unverified; {SHARED_STORE_HINT} presence may equally be a \
             codex-cli install"
        ));
    }
}

impl Default for ChatGptDesktopAdapter {
    fn default() -> Self {
        #[expect(clippy::unwrap_used, reason = "chatgpt-desktop is static valid")]
        let id = HarnessId::new(HARNESS_ID_STR).unwrap();
        Self { id }
    }
}

impl Adapter for ChatGptDesktopAdapter {
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

    fn detection(&self) -> DetectionResult {
        let mut evidence = Vec::new();
        Self::collect_config_evidence(&mut evidence);
        let store_seen = evidence
            .iter()
            .any(|e| e.contains("shared config.toml exists") || e.contains("auth.json exists"));
        let present = if store_seen {
            InstallPresence::Present
        } else {
            InstallPresence::Absent
        };
        // The store is shared with codex-cli, so file evidence alone is a
        // low-confidence signal for the APP specifically.
        let confidence = if store_seen {
            DetectionConfidence::Low
        } else {
            DetectionConfidence::High
        };
        DetectionResult::new(present, None, evidence, confidence)
    }

    fn version_resolution(&self) -> VersionResolution {
        let detection = self.detection();
        let mut res = VersionResolution::new(None, Some(SCHEMA_VERSION_STR.to_owned()), false);
        res.notes = detection.evidence;
        res.notes.push(
            "desktop app versioning is CalVer (e.g. 26.812.10818) with no documented \
             --version probe; writes are codex-cli's business, not this adapter's"
                .to_owned(),
        );
        res
    }

    fn config_surfaces(&self) -> Vec<ConfigSurface> {
        let mut surfaces = Vec::new();

        // Shared user config (owned by codex-cli): the ONE desktop-specific
        // owned selector is desktop.custom_file_handlers.
        let config_resolver = PathResolver::new(
            Some("$CODEX_HOME/config.toml (shared with codex-cli)"),
            Some("$CODEX_HOME/config.toml (shared with codex-cli)"),
            Some("%CODEX_HOME%\\config.toml (shared with codex-cli)"),
            "~/.codex/config.toml",
        );
        let mut config = ConfigSurface::new(
            "config.toml",
            config_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        config.precedence = 10;
        config.owned_selectors = vec![DESKTOP_OWNED_SELECTOR.to_owned()];
        config.backup_required = true;
        config.restart_behavior = RestartBehavior::Reload;
        surfaces.push(config);

        // Shared profiles: $CODEX_HOME/<name>.config.toml.
        let profile_resolver = PathResolver::new(
            Some("$CODEX_HOME/<name>.config.toml (shared with codex-cli)"),
            Some("$CODEX_HOME/<name>.config.toml (shared with codex-cli)"),
            Some("%CODEX_HOME%\\<name>.config.toml (shared with codex-cli)"),
            "~/.codex/<name>.config.toml",
        );
        let mut profiles = ConfigSurface::new(
            "profile.config.toml",
            profile_resolver,
            DocumentKind::Toml,
            ConfigScope::User,
            SurfaceOwnership::UserEditable,
        );
        profiles.precedence = 20;
        profiles.backup_required = true;
        profiles.restart_behavior = RestartBehavior::Reload;
        surfaces.push(profiles);

        // Credentials: secret-bearing, detect-only.
        let auth_resolver = PathResolver::new(
            Some("$CODEX_HOME/auth.json"),
            Some("$CODEX_HOME/auth.json"),
            Some("%CODEX_HOME%\\auth.json"),
            "~/.codex/auth.json",
        );
        let mut auth = ConfigSurface::new(
            "auth.json",
            auth_resolver,
            DocumentKind::Json,
            ConfigScope::User,
            SurfaceOwnership::ExternalSecretStore,
        );
        auth.precedence = 0;
        auth.backup_required = false;
        auth.restart_behavior = RestartBehavior::ReLogin;
        surfaces.push(auth);

        // Sessions the app ingests from the CLI store (community-corroborated).
        let sessions_resolver = PathResolver::fallback_only(
            "$CODEX_HOME session store (internal, app ingests CLI sessions)",
        );
        let mut sessions = ConfigSurface::new(
            "sessions",
            sessions_resolver,
            DocumentKind::Opaque,
            ConfigScope::Internal,
            SurfaceOwnership::HarnessManaged,
        );
        sessions.precedence = 0;
        sessions.backup_required = false;
        sessions.restart_behavior = RestartBehavior::None;
        surfaces.push(sessions);

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
            "sessions/*".to_owned(),
            "history/*".to_owned(),
            "log/*".to_owned(),
            "*.log".to_owned(),
            "cache/*".to_owned(),
        ]
    }

    /// Honest no-env plan: setting `CODEX_HOME` here would fabricate GUI-level
    /// relocation the docs do not support. The empty env set is deliberate —
    /// the alias core refuses aliasing on exactly this, and the plan text
    /// redirects to the verified path (alias codex-cli).
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
        let mut plan = WrapperPlan::new(
            "read-only on the shared ~/.codex store; alias via codex-cli (CODEX_HOME)",
        );
        plan.description = format!(
            " chatgpt-desktop {SHARED_STATE_WARNING} and reads it at its default location; \
             {NO_RELOCATION_NOTE}; alias codex-cli instead — the CLI demonstrably honors \
             {CODEX_HOME_ENV_VAR} (run-3/4 live evidence); config_root {} is informative only \
             (chatgpt-desktop.md section 6)",
            instance.config_root
        );
        plan.shared_state_warnings = vec![
            format!(
                "{SHARED_STATE_WARNING} (config.toml incl. mcp_servers, auth.json, profiles, \
                 sessions) — only codex-cli owns writes to it"
            ),
            format!(
                "{CODEX_HOME_ENV_VAR} relocation is CLI-documented only; the GUI following it \
                 is undocumented (chatgpt-desktop.md section 5)"
            ),
        ];
        Ok(plan)
    }

    fn scan_candidates(&self) -> Vec<String> {
        vec![
            format!("{SHARED_STORE_HINT}/config.toml ({SHARED_STATE_WARNING})"),
            format!("{SHARED_STORE_HINT}/auth.json (detect-only)"),
            "$CODEX_HOME/<name>.config.toml (profiles, shared)".to_owned(),
            "$CODEX_HOME session store (app ingests CLI sessions)".to_owned(),
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
            Isolation::FixedPathSingle | Isolation::RelocatedRoot | Isolation::Unknown => {
                crate::adapter::validate_instance_surfaces(self, instance.config_root.as_path())
            }
            other => Err(CoreError::Validation {
                field: "isolation".to_owned(),
                reason: format!(
                    "chatgpt-desktop reads the shared default store with no app-level \
                     relocation (fixed_path_single); got {other} — alias codex-cli for \
                     {CODEX_HOME_ENV_VAR} isolation"
                ),
            }),
        }
    }

    fn surface_schema(&self, surface_id: &str) -> Option<SurfaceSchema> {
        // The shared config's TOML table shape plus the one desktop-owned
        // key; everything else in the file belongs to codex-cli's model.
        match surface_id {
            "config.toml" => Some(
                SurfaceSchema::new()
                    .with_root_shape(RootShape::Table)
                    .with_owned_key(DESKTOP_OWNED_SELECTOR, ValueType::Array),
            ),
            _ => None,
        }
    }

    fn supported_skill_modes(&self) -> Vec<crate::adapter::SkillMode> {
        Vec::new()
    }

    /// EXT-09: explicit MCP absence — the app's Chat surface is remote-MCP
    /// only, and the local `[mcp_servers]` destination inside `~/.codex/
    /// config.toml` belongs to the codex-cli harness (already modeled there;
    /// a duplicate decl would double-own one file). The community-grade
    /// `config/mcp.json` path claim is flagged in the doc, never modeled.
    fn mcp_absence_reason(&self) -> Option<&'static str> {
        Some(
            "chat connects to remote MCP servers only (official position); the local \
             [mcp_servers] surface inside ~/.codex/config.toml belongs to the codex-cli \
             harness (chatgpt-desktop.md section 3)",
        )
    }

    /// EXT-06: explicit plugin-mechanism absence (corpus-grounded): MCP
    /// "apps" are server-side and web-managed; no local plugin mechanism is
    /// documented for the app.
    fn plugin_absence_reason(&self) -> Option<&'static str> {
        Some(
            "MCP apps are server-side and web-managed (developer mode); no local plugin \
             mechanism documented (chatgpt-desktop.md section 4)",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChatGptDesktopAdapter, DESKTOP_OWNED_SELECTOR, DISPLAY_NAME, HARNESS_ID_STR,
        REMOTE_MCP_POSITION, RESEARCH_DOC, SHARED_STATE_WARNING,
    };
    use crate::adapter::{Adapter, ConfigScope, DocumentKind, ProductStatus, SurfaceOwnership};
    use crate::error::CoreError;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::instance::Instance;
    use crate::paths::AbsolutePath;
    use crate::state::{AdapterSupport, InstanceOrigin, Isolation, Ownership};

    fn adapter() -> ChatGptDesktopAdapter {
        ChatGptDesktopAdapter::new().unwrap()
    }

    fn sample_instance_with_root(root: &str) -> Instance {
        Instance {
            id: InstanceId::new("test-chatgpt-desktop-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new(HARNESS_ID_STR).unwrap(),
            config_root: AbsolutePath::new(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::FixedPathSingle,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-09-18T00:00:00Z".to_owned(),
            adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
        }
    }

    #[test]
    fn adapter_identity() {
        let a = adapter();
        assert_eq!(a.id().as_str(), HARNESS_ID_STR);
        assert_eq!(a.display_name(), DISPLAY_NAME);
        assert_eq!(a.product_status(), ProductStatus::Active);
        assert_eq!(a.research_doc_link(), RESEARCH_DOC);
        assert_eq!(a.last_verified_date(), "2026-09-18");
    }

    #[test]
    fn detection_is_honest_about_the_shared_store() {
        let a = adapter();
        let r = a.detection();
        assert!(!r.evidence.is_empty());
        assert!(
            r.evidence.iter().any(|e| e.contains(SHARED_STATE_WARNING)),
            "evidence must name the shared store: {:?}",
            r.evidence
        );
        assert!(
            r.evidence.iter().any(|e| e.contains("codex-cli install")),
            "evidence must say the store may be a codex-cli install: {:?}",
            r.evidence
        );
    }

    #[test]
    fn config_surfaces_mirror_the_shared_codex_store_read_only() {
        let a = adapter();
        let surfaces = a.config_surfaces();
        let config = surfaces
            .iter()
            .find(|s| s.id == "config.toml")
            .expect("config.toml surface");
        assert_eq!(config.kind, DocumentKind::Toml);
        assert_eq!(config.scope, ConfigScope::User);
        assert_eq!(config.ownership, SurfaceOwnership::UserEditable);
        assert_eq!(
            config.owned_selectors,
            vec![DESKTOP_OWNED_SELECTOR.to_owned()],
            "only the desktop-specific key is owned here; the rest belongs to codex-cli"
        );
        assert!(
            config
                .path_resolver
                .linux
                .as_deref()
                .is_some_and(|h| h.contains("shared with codex-cli")),
            "resolver hints must flag the shared store"
        );
        let auth = surfaces
            .iter()
            .find(|s| s.id == "auth.json")
            .expect("auth.json surface");
        assert_eq!(auth.ownership, SurfaceOwnership::ExternalSecretStore);
        assert!(
            surfaces.iter().any(|s| s.id == "sessions"),
            "the ingested session store must be declared"
        );
    }

    #[test]
    fn mcp_absence_splits_chat_remote_from_codex_local() {
        let a = adapter();
        assert!(
            a.mcp_decl().is_none(),
            "no duplicate MCP dest over codex-cli's file"
        );
        let reason = a.mcp_absence_reason().expect("absence must be explicit");
        let lowered = reason.to_ascii_lowercase();
        assert!(
            lowered.contains("remote mcp servers"),
            "absence must cite the remote-only position ({REMOTE_MCP_POSITION}): {reason}"
        );
        assert!(
            reason.contains("codex-cli"),
            "absence must redirect local MCP to codex-cli: {reason}"
        );
        let plugin = a.plugin_absence_reason().expect("plugin absence explicit");
        assert!(!plugin.is_empty());
    }

    #[test]
    fn supported_operations_read_only() {
        let a = adapter();
        let ops = a.supported_operations();
        assert!(!ops.is_empty());
        for (_, support) in ops {
            assert_eq!(support, AdapterSupport::ReadOnly);
        }
    }

    /// The alias-contract pin: no env vars of its own (setting `CODEX_HOME`
    /// here would fabricate GUI-level relocation), and the plan redirects to
    /// codex-cli — `alias::create_alias` refuses on the empty env set.
    #[test]
    fn plan_wrapper_sets_no_env_vars_and_redirects_to_codex_cli() {
        let a = adapter();
        let inst =
            sample_instance_with_root(&crate::test_util::tmp_abs_str(".chatgpt-desktop-work"));
        let plan = a.plan_wrapper(&inst).unwrap();
        assert!(
            plan.env_vars.is_empty(),
            "no GUI-level relocation may be fabricated: {:?}",
            plan.env_vars
        );
        assert!(!plan.description.is_empty());
        assert!(
            plan.description.contains("codex-cli"),
            "plan must redirect aliasing to codex-cli: {}",
            plan.description
        );
        assert!(
            plan.shared_state_warnings
                .iter()
                .any(|w| w.contains(SHARED_STATE_WARNING)),
            "shared-state warning must name ~/.codex: {:?}",
            plan.shared_state_warnings
        );
    }

    #[test]
    fn plan_wrapper_rejects_mismatched_harness() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".cgd-work"));
        inst.harness = HarnessId::new("codex-cli").unwrap();
        match a.plan_wrapper(&inst).unwrap_err() {
            CoreError::Validation { field, .. } => assert_eq!(field, "harness"),
            other => panic!("expected harness validation, got {other:?}"),
        }
    }

    #[test]
    fn scan_candidates_name_the_shared_store() {
        let a = adapter();
        let candidates = a.scan_candidates();
        assert!(
            candidates
                .iter()
                .any(|c| c.contains("~/.codex") && c.contains(SHARED_STATE_WARNING)),
            "candidates must flag the shared store: {candidates:?}"
        );
    }

    #[test]
    fn validate_instance_accepts_fixed_path_and_relocated() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".cgd-work"));
        a.validate_instance(&inst).unwrap();
        inst.isolation = Isolation::RelocatedRoot;
        a.validate_instance(&inst).unwrap();
    }

    #[test]
    fn validate_instance_rejects_wrong_isolation() {
        let a = adapter();
        let mut inst = sample_instance_with_root(&crate::test_util::tmp_abs_str(".cgd-work"));
        inst.isolation = Isolation::IdeUserData;
        match a.validate_instance(&inst).unwrap_err() {
            CoreError::Validation { field, .. } => assert_eq!(field, "isolation"),
            other => panic!("expected isolation validation, got {other:?}"),
        }
    }

    #[test]
    fn adapter_is_object_safe() {
        let a = adapter();
        let boxed: Box<dyn Adapter> = Box::new(a);
        assert_eq!(boxed.id().as_str(), HARNESS_ID_STR);
        assert!(!boxed.config_surfaces().is_empty());
    }

    // -------------------------------------------------------------------
    // HAD-06: on-disk fixture corpus (QAL-02)
    // -------------------------------------------------------------------

    #[test]
    fn fixture_corpus_validity_and_secret_free() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/chatgpt_desktop");
        let report = crate::verification::fixture_report(&path);
        assert!(report.validity_pass, "chatgpt_desktop corpus validity");
        assert!(
            report.secret_free_pass,
            "chatgpt_desktop corpus secret-free"
        );
        let populated = path.join("config.populated.toml");
        assert!(
            populated.exists(),
            "fixture missing: {}",
            populated.display()
        );
        let text = std::fs::read_to_string(&populated).unwrap();
        assert!(
            text.contains(
                DESKTOP_OWNED_SELECTOR
                    .split('.')
                    .next()
                    .unwrap_or("desktop")
            ),
            "populated fixture must carry the desktop table"
        );
        assert!(
            text.contains("[mcp_servers."),
            "shared MCP tables must be present"
        );
    }

    /// Schema round-trip: the populated corpus satisfies the declared table
    /// root + desktop-key rule; a non-table root is rejected (HAD-03).
    #[test]
    fn surface_schema_accepts_corpus_and_rejects_bad_root() {
        let a = adapter();
        let corpus = b"[desktop]\ncustom_file_handlers = []\n";
        let diags = crate::adapter::validate_surface_content(
            &a,
            "config.toml",
            corpus,
            superai_config::document::DocumentKind::Toml,
        );
        assert!(diags.is_empty(), "corpus shape must validate: {diags:?}");
        let bad_root = b"just a scalar\n";
        let diags = crate::adapter::validate_surface_content(
            &a,
            "config.toml",
            bad_root,
            superai_config::document::DocumentKind::Toml,
        );
        assert!(
            !diags.is_empty(),
            "non-table root must be rejected: {diags:?}"
        );
    }
}
