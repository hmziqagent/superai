//! Registered harness catalog — the 48 planned product surfaces.
//!
//! Every row from `docs/plans/03-harness-adapters.md` provisional ledger is
//! present with its entry gate, source link, and reason. This satisfies the
//! HAD exit gate "every ledger row is registered in code".

use serde::{Deserialize, Serialize};

use crate::adapter::{Adapter, GenericAdapter, ProductStatus, SkillMode};
use crate::adapters::aider::AiderAdapter;
use crate::adapters::amazon_q::AmazonQAdapter;
use crate::adapters::amp::AmpAdapter;
use crate::adapters::antigravity::AntigravityAdapter;
use crate::adapters::auggie::AuggieAdapter;
use crate::adapters::claude_code::ClaudeCodeAdapter;
use crate::adapters::cline::ClineAdapter;
use crate::adapters::codex_cli::CodexCliAdapter;
use crate::adapters::continue_dev::ContinueDevAdapter;
use crate::adapters::copilot_cli::CopilotCliAdapter;
use crate::adapters::crush::CrushAdapter;
use crate::adapters::cursor::CursorAdapter;
use crate::adapters::deepseek::DeepSeekAdapter;
use crate::adapters::factory_droid::FactoryDroidAdapter;
use crate::adapters::forge::ForgeAdapter;
use crate::adapters::gemini_cli::GeminiCliAdapter;
use crate::adapters::goose::GooseAdapter;
use crate::adapters::gptme::GptmeAdapter;
use crate::adapters::grok_build::GrokBuildAdapter;
use crate::adapters::hermes::HermesAdapter;
use crate::adapters::iflow::IflowAdapter;
use crate::adapters::junie::JunieAdapter;
use crate::adapters::kilo::KiloAdapter;
use crate::adapters::kimi_code::KimiCodeAdapter;
use crate::adapters::kiro::KiroAdapter;
use crate::adapters::kode::KodeAdapter;
use crate::adapters::legacy_kimi::LegacyKimiAdapter;
use crate::adapters::letta::LettaAdapter;
use crate::adapters::mimo::MimoAdapter;
use crate::adapters::mistral_vibe::MistralVibeAdapter;
use crate::adapters::nanocoder::NanocoderAdapter;
use crate::adapters::openclaw::OpenClawAdapter;
use crate::adapters::opencode::OpenCodeAdapter;
use crate::adapters::pi::PiAdapter;
use crate::adapters::qwen_code::QwenCodeAdapter;
use crate::adapters::roo_code::RooCodeAdapter;
use crate::adapters::swe_agent::SweAgentAdapter;
use crate::adapters::trae_agent::TraeAgentAdapter;
use crate::adapters::windsurf::WindsurfAdapter;
use crate::adapters::zcode::ZcodeAdapter;
use crate::adapters::zed_acp::ZedAcpAdapter;
use crate::error::CoreError;
use crate::ids::HarnessId;
use crate::state::{AdapterSupport, Isolation};

// ---------------------------------------------------------------------------
// Catalog entry
// ---------------------------------------------------------------------------

/// One provisional ledger row, registered in code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Stable harness identifier.
    pub id: &'static str,
    /// Human display name.
    pub display_name: &'static str,
    /// Source research document.
    pub source: &'static str,
    /// Provisional support state from the master plan ledger.
    pub support: AdapterSupport,
    /// Reason or entry-gate note.
    pub reason: &'static str,
    /// Isolation class for the harness.
    pub isolation: Isolation,
    /// Product lifecycle status.
    pub product_status: ProductStatus,
    /// Link to the research document (same as source, kept for adapter).
    pub research_doc: &'static str,
    /// Last verified date, `YYYY-MM-DD`.
    pub last_verified: &'static str,
}

impl CatalogEntry {
    /// Validate that the id is a legal [`HarnessId`].
    pub fn validate_id(&self) -> Result<HarnessId, CoreError> {
        HarnessId::new(self.id)
    }
}

// ---------------------------------------------------------------------------
// Static catalog
// ---------------------------------------------------------------------------

/// All 48 provisional ledger rows.
///
/// Order follows the table in `docs/plans/03-harness-adapters.md` with the
/// subsequent orchestrator additions. Every surface has a source link and
/// a non-empty reason.
pub const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        id: "aider",
        display_name: "Aider",
        source: "docs/harness-configs/aider.md",
        support: AdapterSupport::Full,
        reason: "YAML/env/JSON, explicit config — full candidate",
        isolation: Isolation::ExplicitConfig,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/aider.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "amazon-q-cli",
        display_name: "Amazon Q Developer CLI",
        source: "docs/harness-configs/amazon-q-cli.md",
        support: AdapterSupport::MigrationOnly,
        reason: "sunsetting 2026-05-15, EOS 2027-04-30 — migration only",
        isolation: Isolation::ProjectScope,
        product_status: ProductStatus::Sunset,
        research_doc: "docs/harness-configs/amazon-q-cli.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "amp",
        display_name: "Amp",
        source: "docs/harness-configs/amp.md",
        support: AdapterSupport::Constrained,
        reason: "explicit settings file, account constrained",
        isolation: Isolation::ExplicitConfig,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/amp.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "antigravity-cli",
        display_name: "Antigravity CLI",
        source: "docs/harness-configs/antigravity-cli.md",
        support: AdapterSupport::ResearchBlocked,
        reason: "settings/harness-owned auth incomplete, HOME workaround",
        isolation: Isolation::OsBound,
        product_status: ProductStatus::Preview,
        research_doc: "docs/harness-configs/antigravity-cli.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "auggie",
        display_name: "Auggie",
        source: "docs/harness-configs/auggie.md",
        support: AdapterSupport::Constrained,
        reason: "account/workspace constrained",
        isolation: Isolation::ProjectScope,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/auggie.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "claude-code",
        display_name: "Claude Code",
        source: "docs/harness-configs/claude-code.md",
        support: AdapterSupport::Full,
        reason: "relocated-root verified, full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/claude-code.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "cline",
        display_name: "Cline",
        source: "docs/harness-configs/cline.md",
        support: AdapterSupport::Full,
        reason: "IDE user-data, full candidate after OS verification",
        isolation: Isolation::IdeUserData,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/cline.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "codex-cli",
        display_name: "Codex CLI",
        source: "docs/harness-configs/codex-cli.md",
        support: AdapterSupport::Full,
        reason: "TOML, relocated-root/profile — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/codex-cli.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "continue-dev",
        display_name: "Continue",
        source: "docs/harness-configs/continue-dev.md",
        support: AdapterSupport::Constrained,
        reason: "YAML/env, project/explicit, hosted features excluded",
        isolation: Isolation::ProjectScope,
        product_status: ProductStatus::Acquired,
        research_doc: "docs/harness-configs/continue-dev.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "copilot-cli",
        display_name: "GitHub Copilot CLI",
        source: "docs/harness-configs/copilot-cli.md",
        support: AdapterSupport::Full,
        reason: "JSONC/MCP, relocated-root — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/copilot-cli.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "copilot-coding-agent",
        display_name: "Copilot Coding Agent",
        source: "docs/harness-configs/copilot-cli.md",
        support: AdapterSupport::Unsupported,
        reason: "cloud-owned repo/org settings, no local mutation",
        isolation: Isolation::Unsupported,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/copilot-cli.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "crush",
        display_name: "Crush",
        source: "docs/harness-configs/crush.md",
        support: AdapterSupport::ResearchBlocked,
        reason: "executable crushrc, deprecated JSON — research blocked for writes",
        isolation: Isolation::ProjectScope,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/crush.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "cursor",
        display_name: "Cursor IDE and Agent CLI",
        source: "docs/harness-configs/cursor.md",
        support: AdapterSupport::Constrained,
        reason: "CLI root plus IDE user-data — constrained",
        isolation: Isolation::IdeUserData,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/cursor.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "deepseek-harness",
        display_name: "DeepSeek Harness",
        source: "docs/harness-configs/deepseek-harness.md",
        support: AdapterSupport::ResearchBlocked,
        reason: "provider catalog incomplete, developer preview",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Preview,
        research_doc: "docs/harness-configs/deepseek-harness.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "factory-droid",
        display_name: "Factory Droid",
        source: "docs/harness-configs/factory-droid.md",
        support: AdapterSupport::Constrained,
        reason: "project/HOME constrained, layered JSON",
        isolation: Isolation::ProjectScope,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/factory-droid.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "forge",
        display_name: "Forge",
        source: "docs/harness-configs/forge.md",
        support: AdapterSupport::Full,
        reason: "relocated config — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/forge.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "gemini-cli",
        display_name: "Gemini CLI",
        source: "docs/harness-configs/gemini-cli.md",
        support: AdapterSupport::MigrationOnly,
        reason: "retired consumer tiers 2026-06-18, successor Antigravity",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Retired,
        research_doc: "docs/harness-configs/gemini-cli.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "goose",
        display_name: "Goose",
        source: "docs/harness-configs/goose.md",
        support: AdapterSupport::Full,
        reason: "YAML config/recipes, relocated-root — full candidate after unverified keys close",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/goose.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "gptme",
        display_name: "gptme",
        source: "docs/harness-configs/gptme.md",
        support: AdapterSupport::Constrained,
        reason: "workspace plus explicit state — constrained",
        isolation: Isolation::ProjectScope,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/gptme.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "grok-build",
        display_name: "Grok Build",
        source: "docs/harness-configs/grok-build.md",
        support: AdapterSupport::Full,
        reason: "TOML + JSON overlay, relocated-root — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/grok-build.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "hermes-agent",
        display_name: "Hermes Agent",
        source: "docs/harness-configs/hermes-agent.md",
        support: AdapterSupport::Full,
        reason: "YAML/env, relocated-root/profile — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/hermes-agent.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "iflow-cli",
        display_name: "iFlow CLI",
        source: "docs/harness-configs/iflow-cli.md",
        support: AdapterSupport::MigrationOnly,
        reason: "shutdown 2026-04-17 — migration only",
        isolation: Isolation::EnvOnly,
        product_status: ProductStatus::Sunset,
        research_doc: "docs/harness-configs/iflow-cli.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "junie-cli",
        display_name: "Junie CLI",
        source: "docs/harness-configs/junie-cli.md",
        support: AdapterSupport::Full,
        reason: "relocated-root, EAP gated — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Eap,
        research_doc: "docs/harness-configs/junie-cli.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "kilo-code",
        display_name: "Kilo Code extension and CLI",
        source: "docs/harness-configs/kilo-code.md",
        support: AdapterSupport::Constrained,
        reason: "layered JSONC, inline/HOME plus IDE — constrained until root verified",
        isolation: Isolation::IdeUserData,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/kilo-code.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "kimi-code-cli",
        display_name: "Kimi Code CLI",
        source: "docs/harness-configs/kimi-cli.md",
        support: AdapterSupport::Full,
        reason: "TOML plus MCP JSON, relocated-root — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/kimi-cli.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "legacy-kimi-cli",
        display_name: "Legacy Kimi CLI",
        source: "docs/harness-configs/kimi-cli.md",
        support: AdapterSupport::MigrationOnly,
        reason: "legacy root, wound down — migration only",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Retired,
        research_doc: "docs/harness-configs/kimi-cli.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "kiro",
        display_name: "Kiro CLI/IDE",
        source: "docs/harness-configs/kiro.md",
        support: AdapterSupport::ReadOnly,
        reason: "read-only until research gaps close, BYO limitations",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/kiro.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "kode",
        display_name: "Kode CLI",
        source: "docs/harness-configs/kode.md",
        support: AdapterSupport::Full,
        reason: "JSON/MCP/agents, relocated-root — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/kode.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "letta-code",
        display_name: "Letta Code",
        source: "docs/harness-configs/letta-code.md",
        support: AdapterSupport::Constrained,
        reason: "client plus server/provider state, separate server",
        isolation: Isolation::DaemonService,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/letta-code.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "mimo-code",
        display_name: "MiMo Code",
        source: "docs/harness-configs/mimo-code.md",
        support: AdapterSupport::Full,
        reason: "JSON/JSONC, relocated-root — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/mimo-code.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "mistral-vibe",
        display_name: "Mistral Vibe",
        source: "docs/harness-configs/mistral-vibe.md",
        support: AdapterSupport::Full,
        reason: "TOML, relocated-root — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/mistral-vibe.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "nanocoder",
        display_name: "Nanocoder",
        source: "docs/harness-configs/nanocoder.md",
        support: AdapterSupport::Full,
        reason: "JSON provider/MCP, relocated/explicit — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/nanocoder.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "openclaw",
        display_name: "OpenClaw",
        source: "docs/harness-configs/openclaw.md",
        support: AdapterSupport::ResearchBlocked,
        reason: "daemon state, gateway/schema incomplete",
        isolation: Isolation::DaemonService,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/openclaw.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "opencode",
        display_name: "OpenCode",
        source: "docs/harness-configs/opencode.md",
        support: AdapterSupport::Full,
        reason: "layered JSONC, relocated/inline — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/opencode.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "openhands",
        display_name: "OpenHands",
        source: "docs/harness-configs/openhands.md",
        support: AdapterSupport::Constrained,
        reason: "V1 JSON/env plus V0 TOML, Docker — version split required",
        isolation: Isolation::OsBound,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/openhands.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "pi",
        display_name: "Pi",
        source: "docs/harness-configs/pi.md",
        support: AdapterSupport::Full,
        reason: "JSON settings/auth, relocated-root, MCP absent by design",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/pi.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "plandex",
        display_name: "Plandex",
        source: "docs/harness-configs/plandex.md",
        support: AdapterSupport::Constrained,
        reason: "env + server/model-pack, provider/server scoped",
        isolation: Isolation::EnvOnly,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/plandex.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "qwen-code",
        display_name: "Qwen Code",
        source: "docs/harness-configs/qwen-code.md",
        support: AdapterSupport::Full,
        reason: "layered JSON/env/MCP, relocated settings — full candidate",
        isolation: Isolation::RelocatedRoot,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/qwen-code.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "roo-code",
        display_name: "Roo Code",
        source: "docs/harness-configs/roo-code.md",
        support: AdapterSupport::MigrationOnly,
        reason: "VS Code storage, archived 2026-05 — migration only",
        isolation: Isolation::IdeUserData,
        product_status: ProductStatus::Archived,
        research_doc: "docs/harness-configs/roo-code.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "swe-agent",
        display_name: "SWE-agent",
        source: "docs/harness-configs/swe-agent.md",
        support: AdapterSupport::Full,
        reason: "composed YAML, explicit config/batch — full candidate",
        isolation: Isolation::ExplicitConfig,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/swe-agent.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "trae-agent",
        display_name: "Trae Agent",
        source: "docs/harness-configs/trae-agent.md",
        support: AdapterSupport::Full,
        reason: "YAML + deprecated JSON, explicit config/env — full candidate",
        isolation: Isolation::ExplicitConfig,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/trae-agent.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "warp",
        display_name: "Warp Agent CLI/app",
        source: "docs/harness-configs/warp.md",
        support: AdapterSupport::Constrained,
        reason: "CLI TOML + MCP JSON, Linux XDG/profile constrained",
        isolation: Isolation::OsBound,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/warp.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "windsurf",
        display_name: "Windsurf/Devin Desktop",
        source: "docs/harness-configs/windsurf.md",
        support: AdapterSupport::Constrained,
        reason: "MCP JSON + rules/skills, IDE user-data — constrained",
        isolation: Isolation::IdeUserData,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/windsurf.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "zcode",
        display_name: "ZCode",
        source: "docs/harness-configs/zcode.md",
        support: AdapterSupport::SingleInstance,
        reason: "versioned-path JSON, fixed path — single instance",
        isolation: Isolation::FixedPathSingle,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/zcode.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "zed-acp",
        display_name: "Zed AI/ACP",
        source: "docs/harness-configs/zed-acp.md",
        support: AdapterSupport::Constrained,
        reason: "JSON settings, ACP wrappers, wrapper registrations — version gates",
        isolation: Isolation::IdeUserData,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/zed-acp.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "vibe-kanban",
        display_name: "Vibe Kanban",
        source: "docs/harness-configs/orchestrators.md",
        support: AdapterSupport::MigrationOnly,
        reason: "orchestrator profiles/env/MCP/worktrees, community-maintained",
        isolation: Isolation::ProjectScope,
        product_status: ProductStatus::Sunset,
        research_doc: "docs/harness-configs/orchestrators.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "conductor",
        display_name: "Conductor",
        source: "docs/harness-configs/orchestrators.md",
        support: AdapterSupport::Constrained,
        reason: "user/repo TOML, macOS worktrees/profiles",
        isolation: Isolation::OsBound,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/orchestrators.md",
        last_verified: "2026-08-25",
    },
    CatalogEntry {
        id: "sculptor",
        display_name: "Sculptor",
        source: "docs/harness-configs/orchestrators.md",
        support: AdapterSupport::Constrained,
        reason: "env + harness settings, workspace/container",
        isolation: Isolation::OsBound,
        product_status: ProductStatus::Active,
        research_doc: "docs/harness-configs/orchestrators.md",
        last_verified: "2026-08-25",
    },
];

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

/// Return the full catalog slice.
pub fn all_entries() -> &'static [CatalogEntry] {
    ENTRIES
}

/// Return the number of registered surfaces.
pub fn len() -> usize {
    ENTRIES.len()
}

/// Whether the catalog is empty (never true, kept for API symmetry).
pub fn is_empty() -> bool {
    ENTRIES.is_empty()
}

/// Find an entry by harness id string.
pub fn find_by_id(id: &str) -> Option<&'static CatalogEntry> {
    ENTRIES.iter().find(|entry| entry.id == id)
}

/// Build generic adapters for every catalog entry.
///
/// Invalid ids are skipped; with the current ledger all ids are valid so the
/// returned count equals `ENTRIES.len()`.
#[expect(
    clippy::too_many_lines,
    reason = "catalog has 48 entries with per-adapter branching"
)]
pub fn all_adapters() -> Vec<Box<dyn Adapter>> {
    let mut out = Vec::with_capacity(ENTRIES.len());
    for entry in ENTRIES {
        if entry.id == crate::adapters::claude_code::HARNESS_ID_STR
            && let Ok(adapter) = ClaudeCodeAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::codex_cli::HARNESS_ID_STR
            && let Ok(adapter) = CodexCliAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::aider::HARNESS_ID_STR
            && let Ok(adapter) = AiderAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::opencode::HARNESS_ID_STR
            && let Ok(adapter) = OpenCodeAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::cline::HARNESS_ID_STR
            && let Ok(adapter) = ClineAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::copilot_cli::HARNESS_ID_STR
            && let Ok(adapter) = CopilotCliAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::goose::HARNESS_ID_STR
            && let Ok(adapter) = GooseAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::qwen_code::HARNESS_ID_STR
            && let Ok(adapter) = QwenCodeAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::kimi_code::HARNESS_ID_STR
            && let Ok(adapter) = KimiCodeAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::grok_build::HARNESS_ID_STR
            && let Ok(adapter) = GrokBuildAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::mistral_vibe::HARNESS_ID_STR
            && let Ok(adapter) = MistralVibeAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::forge::HARNESS_ID_STR
            && let Ok(adapter) = ForgeAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::kode::HARNESS_ID_STR
            && let Ok(adapter) = KodeAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::pi::HARNESS_ID_STR
            && let Ok(adapter) = PiAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::nanocoder::HARNESS_ID_STR
            && let Ok(adapter) = NanocoderAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::hermes::HARNESS_ID_STR
            && let Ok(adapter) = HermesAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::mimo::HARNESS_ID_STR
            && let Ok(adapter) = MimoAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::junie::HARNESS_ID_STR
            && let Ok(adapter) = JunieAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::amp::HARNESS_ID_STR
            && let Ok(adapter) = AmpAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::continue_dev::HARNESS_ID_STR
            && let Ok(adapter) = ContinueDevAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::factory_droid::HARNESS_ID_STR
            && let Ok(adapter) = FactoryDroidAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::gptme::HARNESS_ID_STR
            && let Ok(adapter) = GptmeAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::cursor::HARNESS_ID_STR
            && let Ok(adapter) = CursorAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::kilo::HARNESS_ID_STR
            && let Ok(adapter) = KiloAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::windsurf::HARNESS_ID_STR
            && let Ok(adapter) = WindsurfAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::zed_acp::HARNESS_ID_STR
            && let Ok(adapter) = ZedAcpAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::auggie::HARNESS_ID_STR
            && let Ok(adapter) = AuggieAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::trae_agent::HARNESS_ID_STR
            && let Ok(adapter) = TraeAgentAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::swe_agent::HARNESS_ID_STR
            && let Ok(adapter) = SweAgentAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::gemini_cli::HARNESS_ID_STR
            && let Ok(adapter) = GeminiCliAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::amazon_q::HARNESS_ID_STR
            && let Ok(adapter) = AmazonQAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::roo_code::HARNESS_ID_STR
            && let Ok(adapter) = RooCodeAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::legacy_kimi::HARNESS_ID_STR
            && let Ok(adapter) = LegacyKimiAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::antigravity::HARNESS_ID_STR
            && let Ok(adapter) = AntigravityAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::kiro::HARNESS_ID_STR
            && let Ok(adapter) = KiroAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::openclaw::HARNESS_ID_STR
            && let Ok(adapter) = OpenClawAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::zcode::HARNESS_ID_STR
            && let Ok(adapter) = ZcodeAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::crush::HARNESS_ID_STR
            && let Ok(adapter) = CrushAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::deepseek::HARNESS_ID_STR
            && let Ok(adapter) = DeepSeekAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::iflow::HARNESS_ID_STR
            && let Ok(adapter) = IflowAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        if entry.id == crate::adapters::letta::HARNESS_ID_STR
            && let Ok(adapter) = LettaAdapter::new()
        {
            out.push(Box::new(adapter) as Box<dyn Adapter>);
            continue;
        }
        // Ledger alias: catalog uses kimi-code-cli but adapter is kimi-code
        if entry.id == crate::adapters::kimi_code::HARNESS_ID_LEDGER_ALIAS
            && KimiCodeAdapter::new().is_ok()
        {
            #[expect(clippy::unwrap_used, reason = "catalog ids are static valid")]
            let ledger_id = HarnessId::new(entry.id).unwrap();
            let aliased = KimiCodeAdapter::from_ledger_id(ledger_id);
            out.push(Box::new(aliased) as Box<dyn Adapter>);
            continue;
        }
        if let Ok(harness_id) = HarnessId::new(entry.id) {
            let adapter = GenericAdapter::new(
                harness_id,
                entry.display_name,
                entry.product_status,
                entry.research_doc,
                entry.last_verified,
                entry.support,
                entry.reason,
                entry.source,
            );
            out.push(Box::new(adapter) as Box<dyn Adapter>);
        }
    }
    out
}

/// Convenience: adapter ids as strings.
pub fn all_ids() -> Vec<&'static str> {
    ENTRIES.iter().map(|entry| entry.id).collect()
}

/// Whether the harness supports skill registry workflows.
///
/// Only harnesses with `Full`, `Constrained`, or `SingleInstance` support
/// are considered skill-capable; others are read-only or blocked.
pub fn supports_skills(harness_id: &str) -> bool {
    match find_by_id(harness_id) {
        Some(entry) => matches!(
            entry.support,
            AdapterSupport::Full | AdapterSupport::Constrained | AdapterSupport::SingleInstance
        ),
        None => false,
    }
}

/// Skill modes available for a harness, derived from its adapter support.
///
/// Returns empty for unknown or unsupported harnesses.
pub fn skill_modes_for(harness_id: &str) -> Vec<SkillMode> {
    match find_by_id(harness_id) {
        Some(entry) => match entry.support {
            AdapterSupport::Full | AdapterSupport::Constrained => {
                vec![
                    SkillMode::LinkAll,
                    SkillMode::LinkSelected,
                    SkillMode::CopySelected,
                ]
            }
            AdapterSupport::SingleInstance => vec![SkillMode::CopySelected],
            _ => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// All harness ids that support at least one skill mode.
pub fn all_skill_supported_ids() -> Vec<&'static str> {
    ENTRIES
        .iter()
        .filter(|entry| supports_skills(entry.id))
        .map(|entry| entry.id)
        .collect()
}

/// Verify that catalog skill support is consistent with adapter `supported_skill_modes`.
///
/// For every catalog entry, the catalog helper `skill_modes_for` must agree
/// with the live adapter's `supported_skill_modes`. This catches ledger drift.
pub fn verify_skill_support_consistency() -> Result<(), CoreError> {
    for entry in ENTRIES {
        let catalog_modes = skill_modes_for(entry.id);
        let adapters = all_adapters();
        let adapter = adapters
            .iter()
            .find(|adapter| adapter.id().as_str() == entry.id);
        if let Some(adapter) = adapter {
            let live_modes = adapter.supported_skill_modes();
            // Compare as sets (order independent)
            let mut cat_set = std::collections::BTreeSet::new();
            let mut live_set = std::collections::BTreeSet::new();
            for mode in catalog_modes {
                cat_set.insert(mode.to_string());
            }
            for mode in live_modes {
                live_set.insert(mode.to_string());
            }
            if cat_set != live_set {
                return Err(CoreError::Validation {
                    field: "skill_support".to_owned(),
                    reason: format!(
                        "catalog vs adapter skill mode mismatch for `{}`: catalog {:?} vs adapter {:?}",
                        entry.id, cat_set, live_set
                    ),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalog_has_48_entries() {
        assert_eq!(len(), 48, "catalog must contain exactly 48 ledger rows");
        assert_eq!(ENTRIES.len(), 48);
        assert_eq!(all_entries().len(), 48);
        assert_eq!(all_ids().len(), 48);
        assert!(!is_empty());
    }

    #[test]
    fn all_ids_are_valid_harness_ids() {
        for entry in ENTRIES {
            let parsed = HarnessId::new(entry.id);
            assert!(
                parsed.is_ok(),
                "catalog id `{}` must be a valid HarnessId: {:?}",
                entry.id,
                parsed.err()
            );
            let harness_id = parsed.unwrap();
            assert_eq!(harness_id.as_str(), entry.id);
        }
    }

    #[test]
    fn ids_are_unique() {
        let mut seen = HashSet::new();
        for entry in ENTRIES {
            let lower = entry.id.to_lowercase();
            assert!(
                seen.insert(lower.clone()),
                "duplicate id `{}` in catalog",
                entry.id
            );
        }
        assert_eq!(seen.len(), 48);
    }

    #[test]
    fn every_entry_has_source_and_reason() {
        use std::path::Path;
        for entry in ENTRIES {
            assert!(
                !entry.source.is_empty(),
                "entry `{}` must have a source link",
                entry.id
            );
            assert!(
                Path::new(entry.source)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("md")),
                "entry `{}` source must be a markdown file, got `{}`",
                entry.id,
                entry.source
            );
            assert!(
                entry.source.starts_with("docs/harness-configs/"),
                "entry `{}` source must be under docs/harness-configs/, got `{}`",
                entry.id,
                entry.source
            );
            assert!(
                !entry.reason.is_empty(),
                "entry `{}` must have a non-empty reason",
                entry.id
            );
            assert!(
                !entry.display_name.is_empty(),
                "entry `{}` must have a display name",
                entry.id
            );
            assert!(
                !entry.research_doc.is_empty(),
                "entry `{}` must have research_doc",
                entry.id
            );
            assert!(
                !entry.last_verified.is_empty(),
                "entry `{}` must have last_verified date",
                entry.id
            );
        }
    }

    #[test]
    fn every_entry_has_support_state() {
        // Ensure we exercise every AdapterSupport variant at least once and
        // that each entry has a ledger support that is not accidentally left
        // as a default.
        let mut has_full = false;
        let mut has_constrained = false;
        let mut has_single = false;
        let mut has_migration = false;
        let mut has_research = false;
        let mut has_unsupported = false;
        let mut has_read_only = false;
        for entry in ENTRIES {
            let support_str = entry.support.to_string();
            assert!(!support_str.is_empty());
            let json = serde_json::to_string(&entry.support).unwrap();
            let back: AdapterSupport = serde_json::from_str(&json).unwrap();
            assert_eq!(back, entry.support);
            match entry.support {
                AdapterSupport::Full => has_full = true,
                AdapterSupport::Constrained => has_constrained = true,
                AdapterSupport::SingleInstance => has_single = true,
                AdapterSupport::MigrationOnly => has_migration = true,
                AdapterSupport::ResearchBlocked => has_research = true,
                AdapterSupport::Unsupported => has_unsupported = true,
                AdapterSupport::ReadOnly => has_read_only = true,
            }
        }
        // We expect to see all 7 provisional states in the catalog.
        assert!(has_full, "must have at least one Full");
        assert!(has_constrained, "must have at least one Constrained");
        assert!(has_single, "must have at least one SingleInstance");
        assert!(has_migration, "must have at least one MigrationOnly");
        assert!(has_research, "must have at least one ResearchBlocked");
        assert!(has_unsupported, "must have at least one Unsupported");
        assert!(has_read_only, "must have at least one ReadOnly");
    }

    #[test]
    fn find_by_id_works() {
        let found = find_by_id("claude-code").unwrap();
        assert_eq!(found.display_name, "Claude Code");
        assert_eq!(found.support, AdapterSupport::Full);
        assert!(find_by_id("does-not-exist").is_none());
        // orchestrator entries are discoverable too
        let sculptor = find_by_id("sculptor").unwrap();
        assert_eq!(sculptor.display_name, "Sculptor");
    }

    #[test]
    fn all_adapters_span_catalog() {
        let adapters = all_adapters();
        assert_eq!(
            adapters.len(),
            48,
            "all_adapters must return an adapter for each catalog row"
        );
        let mut ids: Vec<String> = adapters
            .iter()
            .map(|adapter| adapter.id().to_string())
            .collect();
        ids.sort();
        let mut expected: Vec<String> = ENTRIES.iter().map(|entry| entry.id.to_owned()).collect();
        expected.sort();
        assert_eq!(ids, expected, "adapter ids must match catalog ids");

        // Check that each adapter's research link matches catalog source.
        for adapter in &adapters {
            let id = adapter.id().to_string();
            let entry = find_by_id(&id).unwrap();
            assert_eq!(
                adapter.research_doc_link(),
                entry.research_doc,
                "adapter `{id}` research link must match catalog"
            );
            assert_eq!(
                adapter.last_verified_date(),
                entry.last_verified,
                "adapter `{id}` verified date must match catalog"
            );
            assert_eq!(
                adapter.display_name(),
                entry.display_name,
                "adapter `{id}` display name must match catalog"
            );
        }
    }

    #[test]
    fn catalog_support_counts_match_ledger() {
        // Counts derived from the provisional ledger table.
        let mut full = 0;
        let mut constrained = 0;
        let mut single = 0;
        let mut migration_only = 0;
        let mut research_blocked = 0;
        let mut unsupported = 0;
        let mut read_only = 0;
        for entry in ENTRIES {
            match entry.support {
                AdapterSupport::Full => full += 1,
                AdapterSupport::Constrained => constrained += 1,
                AdapterSupport::SingleInstance => single += 1,
                AdapterSupport::MigrationOnly => migration_only += 1,
                AdapterSupport::ResearchBlocked => research_blocked += 1,
                AdapterSupport::Unsupported => unsupported += 1,
                AdapterSupport::ReadOnly => read_only += 1,
            }
        }
        assert_eq!(full, 20, "expected 20 Full per ledger");
        assert_eq!(constrained, 15, "expected 15 Constrained per ledger");
        assert_eq!(single, 1, "expected 1 SingleInstance per ledger");
        assert_eq!(migration_only, 6, "expected 6 MigrationOnly per ledger");
        assert_eq!(research_blocked, 4, "expected 4 ResearchBlocked per ledger");
        assert_eq!(unsupported, 1, "expected 1 Unsupported per ledger");
        assert_eq!(read_only, 1, "expected 1 ReadOnly per ledger");
        assert_eq!(
            full + constrained
                + single
                + migration_only
                + research_blocked
                + unsupported
                + read_only,
            48
        );
    }

    #[test]
    #[expect(
        clippy::excessive_nesting,
        reason = "test branching for support states"
    )]
    fn adapters_are_object_safe_and_usable_as_trait_objects() {
        let adapters = all_adapters();
        for adapter in adapters {
            // Exercise every trait method to ensure no panic.
            let _ = adapter.id();
            let _ = adapter.display_name();
            let _ = adapter.product_status();
            let _ = adapter.supported_platforms();
            let _ = adapter.adapter_revision();
            let _ = adapter.research_doc_link();
            let _ = adapter.last_verified_date();
            let _ = adapter.detection();
            let _ = adapter.version_resolution();
            let _ = adapter.config_surfaces();
            let _ = adapter.supported_operations();
            let _ = adapter.plan_mirror_exclusions();
            let _ = adapter.scan_candidates();
            // validate_instance on a synthetic instance for that adapter
            let harness_id = adapter.id();
            let inst = crate::instance::Instance {
                id: crate::ids::InstanceId::new("test-id-catalog-1").unwrap(),
                name: crate::ids::InstanceName::new("work").unwrap(),
                harness: harness_id.clone(),
                config_root: crate::paths::AbsolutePath::new("/tmp/.test-catalog-work").unwrap(),
                binary: None,
                wrapper: None,
                isolation: Isolation::RelocatedRoot,
                origin: crate::state::InstanceOrigin::Created,
                ownership: crate::state::Ownership::SuperaiCreated,
                template: None,
                created_at: "2026-08-26T00:00:00Z".to_owned(),
                adapter_revision: crate::adapter::ADAPTER_REVISION.to_owned(),
            };
            let res = adapter.validate_instance(&inst);
            let entry = find_by_id(harness_id.as_str());
            let is_research_blocked =
                entry.is_some_and(|e| e.support == AdapterSupport::ResearchBlocked);
            // Only strictly enforce ResearchBlocked for concrete adapters (antigravity, openclaw);
            // generic ResearchBlocked adapters (crush, deepseek) currently return Ok and are allowed.
            let is_concrete_research_blocked = matches!(
                harness_id.as_str(),
                crate::adapters::antigravity::HARNESS_ID_STR
                    | crate::adapters::openclaw::HARNESS_ID_STR
                    | crate::adapters::crush::HARNESS_ID_STR
                    | crate::adapters::deepseek::HARNESS_ID_STR
            );
            if is_research_blocked && is_concrete_research_blocked {
                assert!(
                    res.is_err(),
                    "validate_instance should be ResearchBlocked for `{harness_id}`"
                );
                if let Err(err) = &res {
                    let dbg = format!("{err:?}");
                    assert!(
                        dbg.contains("ResearchBlocked") || dbg.contains("research"),
                        "expected ResearchBlocked error for `{harness_id}`: {dbg}"
                    );
                }
            } else {
                // For generic ResearchBlocked (crush, deepseek) allow Ok; for others require Ok.
                assert!(
                    res.is_ok(),
                    "validate_instance should succeed for matching harness `{harness_id}`: {:?}",
                    res.err()
                );
            }
            let plan_res = adapter.plan_wrapper(&inst);
            let is_migration_only =
                entry.is_some_and(|e| e.support == AdapterSupport::MigrationOnly);
            // Only strictly enforce blocking for concrete migration/research adapters;
            // generic adapters still return Ok and are allowed until they get concrete impls.
            let is_concrete_blocked = matches!(
                harness_id.as_str(),
                crate::adapters::gemini_cli::HARNESS_ID_STR
                    | crate::adapters::amazon_q::HARNESS_ID_STR
                    | crate::adapters::roo_code::HARNESS_ID_STR
                    | crate::adapters::legacy_kimi::HARNESS_ID_STR
                    | crate::adapters::antigravity::HARNESS_ID_STR
                    | crate::adapters::openclaw::HARNESS_ID_STR
                    | crate::adapters::crush::HARNESS_ID_STR
                    | crate::adapters::deepseek::HARNESS_ID_STR
                    | crate::adapters::iflow::HARNESS_ID_STR
            );
            if (is_research_blocked || is_migration_only) && is_concrete_blocked {
                assert!(
                    plan_res.is_err(),
                    "plan_wrapper should be blocked for `{harness_id}` (support {support:?})",
                    support = entry.map(|e| e.support)
                );
            } else {
                let plan = match plan_res {
                    Ok(p) => p,
                    Err(err) => panic!("plan_wrapper should succeed for `{harness_id}`: {err:?}"),
                };
                assert!(!plan.description.is_empty());
            }
        }
    }

    #[test]
    fn isolation_coverage_is_not_all_one_value() {
        let mut distinct: Vec<Isolation> = Vec::new();
        for entry in ENTRIES {
            if !distinct.contains(&entry.isolation) {
                distinct.push(entry.isolation);
            }
        }
        // We expect several isolation classes to be represented.
        assert!(
            distinct.len() >= 5,
            "expected at least 5 distinct isolation classes, got {distinct:?}"
        );
    }

    #[test]
    fn product_status_is_set_and_serializable() {
        for entry in ENTRIES {
            let json = serde_json::to_string(&entry.product_status).unwrap();
            let back: ProductStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, entry.product_status);
            assert!(!entry.product_status.to_string().is_empty());
        }
    }
}
