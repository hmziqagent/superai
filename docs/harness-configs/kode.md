# Kode CLI — Configurable Options Reference

**Kode** (`@shareai-lab/kode`, ShareAI Lab) is a terminal AI coding agent in the Claude Code mold: multi-provider, multi-model, with subagents, skills/plugins, MCP, and Claude Code legacy-format compatibility. Repo: [github.com/shareAI-lab/Kode-cli](https://github.com/shareAI-lab/Kode-cli) · npm: `@shareai-lab/kode` (aliases `kode` / `kwa` / `kd`). Sources cited inline; all fetched 2026-08-25 from `main`.

---

## 1. Config layout & schema

### File locations ([README §Configuration/API keys](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md), [docs/develop/configuration.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md))

| Path | Role |
|---|---|
| `~/.kode.json` | **Global config file** (model profiles, pointers, theme, etc.) — primary location |
| `<KODE_CONFIG_DIR>/config.json` | Global config when `KODE_CONFIG_DIR` (or legacy `CLAUDE_CONFIG_DIR`) is set |
| `~/.kode/` | **Data dir**: logs, tasks, memory files, skills, agents, commands, output-styles |
| `<KODE_CONFIG_DIR>/` | Data dir relocates here when the env var is set |
| `./.kode/settings.json` | Per-project settings (legacy `./.claude/settings.json` read as fallback) |
| `./.kode/settings.local.json` | Local/per-project overrides, e.g. `"outputStyle"` (legacy `.claude/settings.local.json`) |
| `./.mcp.json` (rec.) / `./.mcprc` | Project MCP servers ([docs/mcp.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/mcp.md)) |
| `./.kode/agents/*.md`, `~/.kode/agents/*.md` | Subagent templates (legacy `.claude/agents` also loaded — [docs/compatibility.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/compatibility.md)) |
| `./.kode/skills/<name>/SKILL.md`, `~/.kode/skills/...` | Agent Skills (legacy `.claude/skills`, `.claude/commands` discovered too) |
| `./.kode-plugin/**` | Plugin/marketplace manifests (legacy `.claude-plugin/**`) |

Docker docs confirm the split explicitly: *"Kode uses both `~/.kode` directory for additional data (like memory files) and `~/.kode.json` file for global configuration"* ([README §Docker](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)).

### Global config schema — models ([configuration.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md))

Two complementary layers: **global config** (app-wide) + **settings files** (user/project/local). Models live in the global file as `modelProfiles` (array of named providers) + `modelPointers` (default assignments):

```json
{
  "modelProfiles": [
    {
      "name": "o3",
      "provider": "openai",
      "modelName": "o3",
      "baseURL": "https://api.openai.com/v1",
      "apiKey": "<YOUR_API_KEY>",
      "maxTokens": 8192,
      "contextLength": 200000,
      "isActive": true,
      "createdAt": 1710000000000
    }
  ],
  "modelPointers": {
    "main": "o3",      // main conversation model
    "task": "o3",      // sub-agent/task model
    "compact": "o3",   // context-compression model
    "quick": "o3"      // quick-operations model
  }
}
```

The same shape appears in the README's multi-model example (`main` / `task` / `compact` / `quick` pointers) ([README §Key Implementation Mechanisms](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)). The richer internal `ModelProfile` interface adds per-profile knobs ([docs/develop/modules/model-management.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/modules/model-management.md)):

```typescript
interface ModelProfile {
  id: string; name: string;
  provider: ModelProvider;            // 'anthropic' | 'openai' | 'custom' ... (validated per-provider)
  config: {
    model: string;                    // API model identifier
    baseURL?: string;                 // custom endpoint override
    apiKey?: string;
    maxTokens?, temperature?, topP?, topK?,
    stopSequences?: string[],
    systemPrompt?: string,
    headers?: Record<string,string>,  // custom headers
    timeout?, retryConfig?
  }
  requestStrategy?: 'auto' | 'kode' | 'compat_headers' | 'compat_headers_system'
                     | 'compat_full' | 'claude_code_*' // legacy aliases — for fingerprint-gated Claude gateways
  capabilities?: { supportsTools, supportsVision, supportsStreaming, maxContextTokens, costPer1kTokens }
}
```

Other global-config keys seen in docs: `theme` (`dark`|`light`), `autoUpdaterStatus` (`"enabled"` opts into update checks — otherwise no telemetry/network beyond your providers & web tools, per [README §Network & Privacy](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)), `mcpServers` (global-scope servers), `projects[...]` (per-project keys such as `enableArchitectTool`), and a `context` block (`projectType`, `framework`, `testingFramework`, `buildTool`, `customContext`) ([configuration.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)). Config is Zod-validated on load, backed up to `<path>.backup` before writes, and auto-migrated (v1→v3 formats) ([configuration.md §Migration/Validation](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)).

### Default model & management commands

- Set defaults via `modelPointers` above; the onboarding flow or `/model` picks the initial model ([README](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)).
- `kode models list` — show profiles/pointers; `kode models export --output kode-models.yaml` / `kode models import [--replace] kode-models.yaml` — shareable YAML whose exported `apiKey` defaults to `{ fromEnv: ... }` so secrets stay in env vars ([configuration.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)).
- `kode config get|set|list [-g]` exists **only for safe keys** (theme, verbosity, `enableArchitectTool`, …) — docs say prefer `/model` or `kode models import/export` for models; `kode config paths`, `kode config reset` for diagnostics ([configuration.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)).
- Slash commands: `/model`, `/config`, `/agents`, `/output-style`, `/statusline`, `/cost`, `/clear`, `/init`, `/plugin`, `/help`, `/mcp` ([README §Commands](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md), [docs/mcp.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/mcp.md)).

### Permissions (default posture)

⚠️ *"Kode runs in YOLO mode by default (equivalent to `--dangerously-skip-permissions`)"*; use `kode --safe` for manual approval of Bash + file writes/edits; Plan mode restricts to read-only/planning tools until you approve exiting ([README §Security Notice / §Permissions & Approvals](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)). See §5.

### MCP ([docs/mcp.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/mcp.md))

Project file format `.mcp.json` (recommended) or simplified `.mcprc`; tools surface as `mcp__<server>__<tool>`:

```json
{
  "mcpServers": {
    "my-stdio": { "type": "stdio", "command": "python", "args": ["-m","my_mcp_server"], "env": {"FOO":"BAR"} },
    "my-http":  { "type": "http",  "url": "http://127.0.0.1:3333/mcp" },
    "my-sse":   { "type": "sse",   "url": "http://127.0.0.1:3333/sse" },
    "my-ws":    { "type": "ws",    "url": "ws://127.0.0.1:3333/mcp" }
  }
}
```

CLI: `kode mcp add <name> <command> [args...]` (stdio; `-e KEY=v`; `--scope local|user|project`), `kode mcp add <name> <url> --transport http|sse` (plus `-H "Header: v"`, `add-http`/`add-sse`), `kode mcp list|get|remove`. First launch prompts approval of project-file servers (`kode mcp reset-project-choices` to redo). Global `mcpServers` can also live in the global config ([configuration.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)). Status: `/mcp` in-session.

---

## 2. Environment variables

Precedence: **env var → project config → global config → built-in default**; exception: *Anthropic env overrides are disabled* — put Anthropic keys in Kode settings ([configuration.md §Environment Variables](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)).

| Variable | Purpose | Default / notes |
|---|---|---|
| `OPENAI_API_KEY` | API key for OpenAI-type providers | [configuration.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md) |
| `CLAUDE_MODEL` | Model-selection override (e.g. `claude-3-5-sonnet-20241022`) | ibid. |
| `DEFAULT_MODEL_PROFILE` | Name of profile to default to (e.g. `fast`) | ibid. |
| `ENABLE_ARCHITECT_TOOL` | Feature flag (`true`) | ibid. |
| `DEBUG_MODE`, `VERBOSE`, `LOG_LEVEL`, `NODE_ENV` | Debug/verbosity/dev toggles | ibid. |
| `MCP_SERVER_URL`, `MCP_TIMEOUT` | Legacy MCP tuning | ibid. |
| `MCP_CONNECTION_TIMEOUT_MS` | Per-server connect timeout | `30000`; `0` disables ([docs/mcp.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/mcp.md)) |
| `MCP_SERVER_CONNECTION_BATCH_SIZE` | Concurrent MCP connects | `3` (max `50`) — lower it if servers are slow ([docs/mcp.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/mcp.md), [README §Troubleshooting](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) |
| `MCP_TOOL_TIMEOUT` | Single MCP tool-call timeout (ms) | unset = unlimited ([docs/mcp.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/mcp.md)) |
| `KODE_CONFIG_DIR` | **Relocates global config + data dir**: `<dir>/config.json` + `<dir>/…` | falls back to `CLAUDE_CONFIG_DIR` ([README](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md), [configuration.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)) |
| `KODE_PROJECT_DOC_MAX_BYTES` | Cap on concatenated AGENTS.md instructions | 32 KiB ([README §Instruction Discovery](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) |
| `KODE_RIPGREP_PATH` | Path to `rg` when optional deps skipped | bundled via `@shareai-lab/kode-ripgrep-*` ([README](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) |
| `KODE_SYSTEM_SANDBOX` | Linux bwrap sandbox for Bash calls (`1`=on, `0`=off, `required`=fail closed) | auto in `--safe` ([README §System Sandbox](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) |
| `KODE_SYSTEM_SANDBOX_NETWORK` | Sandbox network policy | blocked; `inherit` allows network (ibid.) |
| `$EDITOR` / `$VISUAL` | Editor for Alt+G message handoff | falls back code/nano/vim/notepad ([README §Authoring Comfort](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) |
| `CLAUDE_CONFIG_DIR` | Legacy fallback for `KODE_CONFIG_DIR` | [docs/compatibility.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/compatibility.md) |
| `CLAUDE_PLUGIN_ROOT`, `CLAUDE_PROJECT_DIR`, `CLAUDE_ENV_FILE` | Legacy hook/plugin-script compat vars honored | ibid. |

Misc: historical `CLAUDE_CODE_*` toggles may be recognized as fallbacks ([compatibility.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/compatibility.md)); the repo's own [.env.example](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/.env.example) is test-harness-only (`TEST_GPT5_API_KEY`, `TEST_GPT5_BASE_URL`, …), not runtime config.

---

## 3. Providers

Provider types validated by the profile manager: `anthropic`, `openai`, `custom` (each with its own validation branch), plus capability inference for known families (gpt-5 → Responses API; o1 → chat completions with `max_completion_tokens` + fixed temperature; GLM → limited tool flags) ([model-management.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/modules/model-management.md)). README: *"Cross-platform — Works with 20+ AI models and providers"* and *"As long as you have an openai-like endpoint, it should work"* ([README](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)).

Setup paths, best-first:

1. **Onboarding UI / `/model`** — pick from provider list or configure manually; `/config` for manual edits ([README §Docker note](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)).
2. **Hand-edit global config** — add named `modelProfiles` entries with `baseURL`/`apiKey`/`modelName` (schema in §1) ([configuration.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)).
3. **YAML fleet import** — `kode models import kode-models.yaml` (`--replace` to overwrite); keep secrets as `apiKey: {fromEnv: VAR}` ([configuration.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)).

Worked examples (profile JSON snippets; endpoint URLs marked ◆ are the providers' standard public endpoints — Kode docs themselves only assert "any openai-like endpoint"):

```jsonc
// Anthropic — key goes in Kode settings, NOT env (env overrides disabled)
{ "name": "claude-main", "provider": "anthropic", "modelName": "claude-sonnet-4",
  "apiKey": "<ANTHROPIC_KEY>", "maxTokens": 8192, "contextLength": 200000 }

// OpenRouter ◆ (standard endpoint https://openrouter.ai/api/v1) via baseURL override
{ "name": "or-deepseek", "provider": "openai", "modelName": "deepseek/deepseek-chat",
  "baseURL": "https://openrouter.ai/api/v1", "apiKey": "<OPENROUTER_KEY>" }

// Ollama local ◆ (standard endpoint http://localhost:11434/v1)
{ "name": "local-qwen", "provider": "openai", "modelName": "qwen2.5-coder:32b",
  "baseURL": "http://localhost:11434/v1", "apiKey": "ollama" }

// DeepSeek direct ◆ (standard endpoint https://api.deepseek.com)
{ "name": "ds", "provider": "openai", "modelName": "deepseek-reasoner",
  "baseURL": "https://api.deepseek.com", "apiKey": "<DEEPSEEK_KEY>" }

// Generic custom provider with extra headers (documented "custom" type)
{ "custom-llm": { "type": "custom", "name": "My Custom LLM",
    "config": { "baseURL": "https://my-llm-api.com", "apiKey": "custom-key",
                "model": "my-model-v1", "headers": { "X-Custom-Header": "value" } } } }
```

(The last block is verbatim from [configuration.md §Advanced Configuration › Custom Model Providers](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md).)

**Mid-session switching:** `/model` changes models live — *"Flexible Switching: Switch models based on task requirements without restarting sessions"* with context inheritance across switches ([README §Multi-Model Collaboration / Context Manager](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)); `Option+M` cycles the active model instantly ([README §Authoring Comfort](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)); `@ask-<model-name> …` consults another configured model inline without leaving the current one (AskExpertModel tool). Model references anywhere accept: pointer (`main|task|compact|quick`), profile name, bare modelName, or `provider:modelName` (e.g. `openai:o3`) ([configuration.md §Model Selectors](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)). For Claude gateways that reject non-Claude clients, pick a `requestStrategy` compat profile during setup or let Kode auto-fallback on "restricted client" signals ([model-management.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/modules/model-management.md), [compatibility.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/compatibility.md)).

---

## 4. Multi-instance wrappers

The supported isolation mechanism is `KODE_CONFIG_DIR`: it moves **both** the global config file (`<dir>/config.json`) **and** the data dir (memory/logs/tasks) in one shot; `CLAUDE_CONFIG_DIR` works identically as a legacy fallback ([configuration.md §File locations](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md), [README §Configuration](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)). Lighter-weight switching without separate dirs: `DEFAULT_MODEL_PROFILE` / `CLAUDE_MODEL` env vars (§2), which outrank config files per the documented precedence chain.

Wrapper script sketch — two coexisting instances, each fully isolated (config, keys, memory):

```bash
#!/usr/bin/env bash
# kode-wrapper: isolated Kode instances per provider.
# Each dir needs a config.json with that provider's modelProfiles/modelPointers (§1 schema);
# seed once with: mkdir -p ~/.kode-anthropic && $EDITOR ~/.kode-anthropic/config.json
set -euo pipefail

instance="${1:-}"
shift || true

case "$instance" in
  anthropic)
    export KODE_CONFIG_DIR="$HOME/.kode-anthropic"     # config.json + data dir both move here
    exec kode --safe "$@"                              # approval mode on for untrusted work
    ;;
  openrouter)
    export KODE_CONFIG_DIR="$HOME/.kode-openrouter"
    # Alternative to a second config file: same dir, different default profile
    export DEFAULT_MODEL_PROFILE="or-deepseek"         # env > project > global > default
    exec kode "$@"
    ;;
  ollama-local)
    export KODE_CONFIG_DIR="$HOME/.kode-local"
    export KODE_SYSTEM_SANDBOX_NETWORK=inherit         # let sandboxed Bash reach localhost:11434
    exec kode "$@"
    ;;
  *)
    echo "usage: kode-wrapper {anthropic|openrouter|ollama-local} [kode args]" >&2
    exit 64 ;;
esac
```

Notes: `--safe` / default-YOLO can differ per instance (see §5); `kode models export > $dir/../shared.yaml` then `KODE_CONFIG_DIR=… kode models import shared.yaml` clones a fleet between instances; container-level isolation follows the documented Docker mounts `-v ~/.kode:/root/.kode -v ~/.kode.json:/root/.kode.json` ([README §Docker Usage](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)).

---

## 5. Permissions, hooks, subagents, memory

**Permissions** ([README §Permissions](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)):
- Default = YOLO (all checks bypassed); `kode --safe` gates Bash commands and file writes/edits behind approval; **Plan mode** allows only read-only/planning tools plus the plan file until you approve exit.
- Config scopes ([configuration.md §Configuration Scopes](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)): *global* (preferences/models/global MCP/updater), *project* (**tool permissions, allowed commands**, project context, local MCP, cost tracking), *session* (runtime flags, temporary permissions, active MCP, current model). CLI: `--setting-sources user,project,local` controls which layers load ([README §Agents](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)).
- Linux hardening: agent Bash runs inside a bubblewrap sandbox when available in safe mode or with `KODE_SYSTEM_SANDBOX=1`; network off unless `KODE_SYSTEM_SANDBOX_NETWORK=inherit`; `=required` fails closed ([README §System Sandbox](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)); the repo also carries seccomp helper scripts (`scripts/seccomp/`).
- Skills declare their own tool allowlists via frontmatter: `allowed-tools: Read Bash(git:*) Bash(jq:*)` ([README §Create a skill](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)).

**Hooks:** No dedicated hooks doc exists in the repo tree (no `docs/hooks.md`). What's documented is **compatibility support for existing Claude Code-style hook/plugin scripts**: Kode honors `CLAUDE_PLUGIN_ROOT`, `CLAUDE_PROJECT_DIR`, `CLAUDE_ENV_FILE`, and "some historical `CLAUDE_CODE_*` toggles … as fallbacks where needed" ([docs/compatibility.md §Environment Variables (Compatibility)](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/compatibility.md)). Treat native hooks as undocumented/unverified.

**Subagents** ([README §Agents/Subagents](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)):
- Loaded from `./.kode/agents`, `~/.kode/agents`, legacy `./.claude/agents` (both levels), plus plugins/policy and injected `--agents <json>`.
- Template = Markdown with frontmatter: `name`, `description`, `tools: ["Read","Grep"]`, `model:` — model accepts aliases `inherit|opus|sonnet|haiku` (mapped to pointers) or full selectors (`main`, profile name, modelName, `provider:modelName`).
- Invoke via `@run-agent-<agentType> …` mention or `Task(subagent_type:"<agentType>", …)`; validate with `kode agents validate`; manage with `/agents` (writes new agents to `.kode/agents` by default).

**Memory / instructions**:
- Data dir holds memory files alongside logs/tasks ([configuration.md §File locations](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md), [README §Docker](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)).
- Instruction discovery is AGENTS.md-native, Codex-style: walk Git root → cwd, prefer `AGENTS.override.md` over `AGENTS.md`, concatenate root→leaf capped at 32 KiB (`KODE_PROJECT_DOC_MAX_BYTES`); `CLAUDE.md` read as legacy extra ([README §AGENTS.md / §Instruction Discovery](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)).
- `# <request>` documentation mode generates/appends structured docs to AGENTS.md ([README §AGENTS.md Documentation Mode](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)).
- Skills/plugins extend this: `SKILL.md` packs in `.kode/skills` (user/project), marketplaces via `kode plugin marketplace add owner/repo`, installs scoped `--scope user|project` ([README §Skills & Plugins](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)).

---

## 6. Claude Code UX deltas

Kode positions itself as Claude Code-compatible-but-Kode-first (*"not affiliated with Anthropic"*, reads/writes legacy `.claude` layouts — [docs/compatibility.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/compatibility.md)). Behavioral differences vs Claude Code:

| Area | Kode | Claude Code baseline |
|---|---|---|
| Permission default | **YOLO by default** (`--dangerously-skip-permissions` equivalent); opt *in* to safety with `--safe` ([README Security Notice](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) | permission prompts on by default, bypass is opt-in flag |
| Models | Unlimited providers/profiles; pointers `main/task/compact/quick` give different defaults per job type; `Option+M` hot-cycle; `/cost` tracks per-model spend; parallel subagents on different models ([README §Comparison](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) | single Anthropic model family per session |
| Expert consult | `@ask-<model>` mentions query other models inline (AskExpertModel tool) ([README](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) | none |
| Instructions | AGENTS.md standard native (+`AGENTS.override.md`, 32 KiB concat cap); `CLAUDE.md` only as legacy ([README](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) | `CLAUDE.md` native |
| On-disk layout | writes `.kode/**`; merely *reads* `.claude/**`; `/agents` creates under `.kode/agents` ([compatibility.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/compatibility.md)) | writes `.claude/**` |
| Env/config | `KODE_CONFIG_DIR` preferred over `CLAUDE_CONFIG_DIR`; Anthropic env overrides deliberately disabled (keys live in Kode settings) ([compatibility.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/compatibility.md), [configuration.md](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/docs/develop/configuration.md)) | `CLAUDE_CODE_*`/Anthropic env conventions |
| Extra surfaces | ACP server mode (`kode-acp` / `kode --acp` for Zed/Toad) ([README §ACP](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)); plugin marketplaces `.kode-plugin/marketplace.json` (accepts legacy `.claude-plugin`) ; fuzzy completion (`gp5` → `@ask-gpt-5`) | n/a / Claude-native equivalents differ |
| Input niceties | `Alt+G` opens prompt in `$EDITOR`; `Alt+Enter` newline; large-paste placeholder expansion; pasting file paths auto-inserts `@path` mentions; macOS clipboard images via `Ctrl+V` ([README §Authoring Comfort/Paste](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) | different bindings |
| Binary/distro | npm with per-platform native binaries (`@shareai-lab/kode-bin-*`) or Bun standalone releases; aliases `kwa`/`kd`; `@dev` channel ([README §Installation](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) | official installer |
| Telemetry | none by default; update checks strictly opt-in (`autoUpdaterStatus: enabled`) ([README §Network & Privacy](https://raw.githubusercontent.com/shareAI-lab/Kode-cli/main/README.md)) | has usage telemetry/opt-out flow |
