# Crush (Charmbracelet) — Complete Configuration Reference

Compiled 2026-08-25 from the primary sources:
- Repo: https://github.com/charmbracelet/crush (`README.md`, `docs/config/README.md`, `schema.json`, `internal/config/*.go`) — cloned to `/home/remixer/crush-repo` at commit ~06345cc (Aug 25, 2026)
- License: **FSL-1.1-MIT**; Crush began as a fork of OpenCode (SST) in mid-2025 and has since diverged heavily (see §6).
- Note: `https://crush.cli` does not resolve (blocked/unreachable). The canonical docs are the repo `docs/` folder.

---

## ⚠️ Big picture first: two config formats

Crush's **primary config format is now `crushrc`** — a Bash script executed at startup using Crush-specific builtins (`provider`, `model`, `mcp`, `lsp`, `hook`, `permissions`, `option`). Source: `docs/config/README.md` and repo `AGENTS.md`.

> "`crush.json` is still supported but is deprecated in favor of `crushrc` and may be removed in a future release." (repo AGENTS.md; docs/config/README.md §"Legacy JSON": "JSON is still supported but is deprecated... new configuration options will only be added to Bash-based config.")

Both formats are merged if present (project overrides global; `crushrc` overrides JSON in the same directory, with a warning).

### Config discovery order (lower number wins per docs, project overrides global)

| Priority | Unix-like | Windows |
|---|---|---|
| 1 | `./.crushrc` / `./.crush.json` | same |
| 2 | `./crushrc` / `./crush.json` | same |
| 3 | `$XDG_CONFIG_HOME/crush/crushrc` (`~/.config/crush/crushrc`) | `%XDG_CONFIG_HOME%\crush\crushrc` |

Source: docs/config/README.md "Where config lives". Override the global file with `CRUSH_GLOBAL_CONFIG=<dir>` (load.go:1181).

---

## 1. Config file: complete `crush.json` schema

Authoritative machine-readable schema: [`schema.json`](https://github.com/charmbracelet/crush/blob/main/schema.json) (902 lines, generated from `internal/config/config.go`). Published schema URL used in configs: `"https://charm.land/crush.json"` (docs/config/README.md legacy example).

Top-level keys (schema.json `$defs.Config`, additionalProperties=false):

| Key | Type | Notes |
|---|---|---|
| `$schema` | string | e.g. `"https://charm.land/crush.json"` |
| `models` | object of `SelectedModel` keyed by `large` / `small` | model slots |
| `providers` | map[string]→`ProviderConfig` | AI providers |
| `mcp` | map[string]→`MCPConfig` | MCP servers (key is `mcp`, not `mcp_servers` — note: older OpenCode-era docs said `mcp_servers`; current Crush uses **`mcp`**) |
| `lsp` | map[string]→`LSPConfig` | language servers |
| `options` | `Options` | general behavior incl. `debug` |
| `permissions` | `Permissions` | tool allow-list |
| `tools` | `Tools` | ls/grep/glob tuning |
| `hooks` | map[event][]HookConfig | e.g. `PreToolUse` |
| `env` | map[string]string | env vars set on startup |

Internal-only fields not settable in user config: `recent_models`, `agents` (`Agents map[string]Agent json:"-"` — built programmatically by `SetupAgents()`, config.go:667,847).

### 1.1 `models` — SelectedModel (schema.json:701)

```jsonc
"models": {
  "large": {                       // "large" = main coding agent model
    "provider": "anthropic",       // required; must match a providers key
    "model": "claude-sonnet-4-20250514", // required
    "reasoning_effort": "low|medium|high", // OpenAI-style reasoning models
    "think": true,                 // Anthropic extended-thinking toggle
    "max_tokens": 8192,            // ≤200000
    "temperature": 0.7,            // 0..1
    "top_p": 0.9,                  // 0..1
    "top_k": 40,
    "frequency_penalty": 0.0,
    "presence_penalty": 0.0,
    "provider_options": {}         // free-form provider-specific passthrough
  },
  "small": { "provider": "...", "model": "..." }  // cheap model for titles/sub-tasks
}
```
Only `large` and `small` are valid slot names (config.go:54-57).

### 1.2 `providers` — ProviderConfig (schema.json:596, config.go:90)

```jsonc
"providers": {
  "<your-provider-id>": {
    "id": "my-provider",              // optional (defaults to key)
    "name": "My Provider",            // display name
    "type": "openai-compat",          // see enum below; default "openai"
    "base_url": "https://api.example.com/v1",
    "api_key": "$MY_PROVIDER_API_KEY",// $VAR or ${VAR} shell-expanded at load time;
                                      // can also be "$(cmd)" or a literal key
    "disable": false,
    "system_prompt_prefix": "...",
    "extra_headers": { "X-Org": "$OPENAI_ORG_ID" },   // values shell-expanded ($VAR, $(cmd)); empty-resolving headers are dropped
    "extra_body": { "metadata": {"team":"x"} },        // verbatim JSON merge into request bodies (openai-compat only); NOT shell-expanded
    "provider_options": {},           // provider-specific options object
    "aws_auth_refresh": "aws sso login", // Bedrock only: run when AWS creds expire
    "flat_rate": false,               // skip cost accounting (subscription billing)
    "discover_models": true,          // auto-discover via GET {base_url}/models; merges with explicit models (yours win)
    "oauth": { "access_token": "...", "refresh_token": "...", "expires_in": 3600, "expires_at": 1700000000,
                "client": {"client_id":"","client_secret":"","auth_url":"","token_url":"","auth_style":0} },
    "models": [ /* array of Model, see 1.3 */ ]
  }
}
```

**Provider `type` enum** (schema.json:620-638):

| type | Protocol / notes |
|---|---|
| `openai` | native OpenAI API — use when proxying/routing through OpenAI (README:680) |
| `openai-compat` | non-OpenAI vendors with OpenAI-compatible APIs (DeepSeek, Together, local gateways…) (README:681) |
| `openrouter` | OpenRouter (Bearer key; connection test hits `{base_url}/credits`) |
| `anthropic` | Anthropic Messages API (`x-api-key` + `anthropic-version` headers) |
| `google` | Gemini API (`generativelanguage.googleapis.com`) |
| `azure` | Azure OpenAI (endpoint/key via `AZURE_OPENAI_API_ENDPOINT` / `AZURE_OPENAI_API_KEY`) |
| `google-vertex` | Vertex AI (`VERTEXAI_PROJECT`, `VERTEXAI_LOCATION`) |
| `bedrock` | Amazon Bedrock (AWS credential chain or `AWS_BEARER_TOKEN_BEDROCK`) |
| `ollama` | local Ollama |
| `litellm`, `llamacpp`, `lmstudio`, `omlx` | other local/self-hosted backends |
| `vercel` | Vercel AI Gateway (`vck_…` keys) |
| `hyper` | Charm Hyper (official subscription provider) |

Note: the requested "gemini/cloudflare" spellings are actually `google` / `google-vertex`; there is no dedicated Cloudflare Workers-AI type — use `openai-compat` with a base_url. Source: schema.json enum above.

### 1.3 Per-model entry inside `providers.<id>.models` (schema.json:335)

Required: `id`, `name`, `cost_per_1m_in`, `cost_per_1m_out`, `cost_per_1m_in_cached`, `cost_per_1m_out_cached`, `context_window`, `default_max_tokens`, `can_reason`, `supports_attachments`. Optional: `reasoning_levels[]`, `default_reasoning_effort`, `options` ({temperature, top_p, top_k, frequency_penalty, presence_penalty, provider_options}).

### 1.4 `options` (schema.json:440, config.go:314)

| Key | Type / default | Purpose |
|---|---|---|
| `context_paths` | string[] | project context files for the AI. Defaults (config.go:28): `.github/copilot-instructions.md`, `.cursorrules`, `.cursor/rules/`, `CLAUDE.md(.local)`, `GEMINI.md`, `CRUSH.md(.local)` (case variants), `AGENTS.md` |
| `global_context_paths` | string[] | default `~/.config/crush/CRUSH.md`, `~/.config/AGENTS.md` |
| `skills_paths` | string[] | Agent Skills dirs (SKILL.md folders). Auto-loaded without config: `.agents/skills`, `.crush/skills`, `.claude/skills`, `.cursor/skills` (docs/config/README.md:546) |
| `tui` | TUIOptions | see below |
| `debug` | bool, false | debug logging |
| `debug_lsp` | bool, false | LSP debug logging |
| `disable_auto_summarize` | bool, false | stop auto conversation summarization |
| `data_directory` | string, `.crush` | per-project state (SQLite DB etc.); relative → cwd-relative, stored absolute |
| `disabled_tools` | string[] | hide built-in tools (names: agent, bash, edit, multiedit, view, write, glob, grep, ls, fetch, agentic_fetch, download, sourcegraph, todos, question, lsp_diagnostics, lsp_references, lsp_symbols, lsp_definition, lsp_call_hierarchy, lsp_rename, lsp_replace_symbol, lsp_restart, crush_info, crush_logs, job_output, job_kill, list_mcp_resources, read_mcp_resource — config.go:787) |
| `disable_provider_auto_update` | bool, false | freeze the Catwalk provider catalog |
| `disable_default_providers` | bool, false | ignore all embedded providers; you must fully specify every provider |
| `attribution` | `{trailer_style: none\|co-authored-by\|assisted-by (default assisted-by), co_authored_by: deprecated, generated_with: bool}` | git commit/PR attribution |
| `disable_metrics` | bool, false | opt out of PostHog telemetry |
| `initialize_as` | string, `AGENTS.md` | context filename created by `crush init` |
| `auto_lsp` | bool, true | auto-configure LSPs from root markers |
| `progress` | bool, true | indeterminate progress indicators |
| `notifications` | `auto\|native\|osc\|bell\|disabled`, default auto | OSC 99/777 detection over SSH |
| `disabled_skills` | string[] | hide named skills |

`options.tui`: `compact_mode` (bool), `diff_mode` (`unified`|`split`), `transparent` (bool), `scrollbar` (`default`|`always`|`never`), `completions` ({`max_depth`, `max_items`}).

**On "context_path" and "disable response storage":** the current option is plural **`context_paths`** (singular existed in very old versions). There is **no** `disable_response_storage` option in current Crush's schema — that was an OpenAI-API concern handled via `extra_body` if a gateway needs it (e.g. `{"store": false}`). Nothing named that exists in schema.json.

### 1.5 `mcp` — MCPConfig (schema.json:222)

```jsonc
"mcp": {
  "github": {
    "type": "http",                    // required: stdio | sse | http (default stdio)
    "url": "https://api.githubcopilot.com/mcp/",   // http/sse
    "headers": { "Authorization": "Bearer $GH_PAT" }, // shell-expanded; empty→dropped
    "oauth": false,                    // OAuth 2.1 flow (HTTP transport only)
    "oauth_client_id": "",             // for servers w/o dynamic registration
    "oauth_client_secret": "",
    "oauth_callback_port": 0,          // pin localhost callback port
    "command": "npx",                  // stdio
    "args": ["-y", "@modelcontextprotocol/server-github"],
    "env": { "KEY": "$VALUE" },
    "timeout": 10,                     // seconds
    "disabled": false,
    "enabled_tools": ["get-library-doc"],   // allow-list
    "disabled_tools": ["dangerous_tool"]    // deny-list
  }
}
```

### 1.6 `permissions` (schema.json:579)

```jsonc
"permissions": {
  "allowed_tools": ["view", "ls", "grep", "edit"]  // run without approval prompts
}
```
In `crushrc`: `permissions allow <tool>…` and `permissions deny <tool>…` (deny = hide from agent entirely; deny maps onto options.disabled_tools behavior). Source: docs/config/README.md §permissions.

### 1.7 Agents

Not directly configurable via top-level JSON (`Agents` has tag `json:"-"`). Two built-in agents constructed by `Config.SetupAgents()` (config.go:847):
- **coder** — main agent, uses the `large` model, all tools.
- **task** — sub-agent spawned for search/context work, read-only tools (glob, grep, ls, lsp_call_hierarchy, lsp_definition, lsp_symbols, sourcegraph, view), no MCPs by default.

The `Agent` struct (config.go:549) exposes: `id`, `name`, `description`, `disabled`, `model` (large|small), `allowed_tools`, `allowed_mcp` (map name→tool list), `context_paths`. Sub-agents are invoked through the built-in `agent` tool (task spawning). Skills under `.agents/skills` etc. extend behavior.

---

## 2. Environment variables

### 2.1 Provider key variables (README "API Keys" table, lines 202–230)

`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `GEMINI_API_KEY`, `ZAI_API_KEY`, `MINIMAX_API_KEY`, `SYNTHETIC_API_KEY`, `HF_TOKEN`, `CEREBRAS_API_KEY`, `IONET_API_KEY`, `ALIBABA_SINGAPORE_API_KEY`, `ALIBABA_US_API_KEY`, `GROQ_API_KEY`, `AVIAN_API_KEY`, `VERCEL_API_KEY`, `OPENCODE_API_KEY` (OpenCode Zen & Go), `HYPER_API_KEY`.
Cloud: `VERTEXAI_PROJECT` + `VERTEXAI_LOCATION` (Vertex), `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/`AWS_REGION`/`AWS_PROFILE`/`AWS_BEARER_TOKEN_BEDROCK` (Bedrock), `AZURE_OPENAI_API_ENDPOINT` + `AZURE_OPENAI_API_KEY` (Azure; optional with Entra ID).
For **custom** providers there is no convention — you reference any var yourself: `"api_key": "$WHATEVER_VAR"`.

### 2.2 CRUSH_* variables (all verified in source)

| Var | Effect | Source |
|---|---|---|
| `CRUSH_DISABLE_METRICS=1` | disable telemetry | cmd/root.go:967; README |
| `DO_NOT_TRACK=1` | also disables metrics | README |
| `CRUSH_DISABLE_PROVIDER_AUTO_UPDATE` | freeze provider catalog | load.go:608 |
| `CRUSH_DISABLE_DEFAULT_PROVIDERS` | skip embedded providers | load.go:612 |
| `CRUSH_GLOBAL_CONFIG=<dir>` | alternate global config dir (`<dir>/crush.json`) | load.go:1181 |
| `CRUSH_GLOBAL_DATA=<dir>` | alternate global data dir | load.go:1228 |
| `CRUSH_CACHE_DIR=<dir>` | alternate cache dir | load.go:1204 |
| `CRUSH_SKILLS_DIR=<dir>` | extra skills dir | load.go:1334 |
| `CRUSH_SKIP_DATADIR_LOCK=1` | bypass data-dir SQLite lock (CI) | db/datadirlock.go:84 |
| `CRUSH_DISABLE_ANTHROPIC_CACHE` | disable Anthropic prompt caching | agent/agent.go:1481 |
| `CRUSH_CLIENT_SERVER`, `CRUSH_SERVER_READY_TIMEOUT` | client/server mode internals | cmd/root.go:297,708 |
| `CRUSH_UI_DEBUG=true` | UI debug | ui/model/ui.go:3078 |
| `CRUSH_CORE_UTILS` | use real coreutils instead of builtins in bash tool | shell/coreutils.go:14 |
| `CRUSH=1` | marker set for non-interactive `crush run` | event/event.go:43 |
| `CRUSH_VERSION` | available inside crushrc for version gating | docs/config/README.md:76 |

Also: **`CRUSH_<ANYTHING>` push/pop mechanism** (config/load.go:185 `PushPopCrushEnv`): during provider configuration, every `CRUSH_FOO` in the environment is mirrored onto `FOO`. So `CRUSH_OPENROUTER_API_KEY=sk-… crush …` makes `OPENROUTER_API_KEY` resolve — this is the official env-based key-switching mechanism for multi-instance wrappers.

### 2.3 XDG directories

- Global config: `$XDG_CONFIG_HOME/crush/crushrc` (default `~/.config/crush/crushrc`); Windows `%USERPROFILE%\.config\crush\crushrc`.
- State/data: `$XDG_DATA_HOME/crush` (default `~/.local/share/crush`); Windows `%LOCALAPPDATA%\crush` — machine-owned JSON state, do not hand-edit (docs/config/README.md:102).
- Cache: `$XDG_CACHE_HOME` fallback chain (load.go:1208).
- Per-project state: `.crush/` in the repo (override via `options.data_directory`).
- `CATWALK_URL` overrides the provider-catalog endpoint (provider.go:60; PR #3585 docs).

---

## 3. Third-party providers — worked examples

### 3a. Custom OpenAI-compatible provider (JSON)

```jsonc
{
  "$schema": "https://charm.land/crush.json",
  "providers": {
    "deepseek": {
      "id": "deepseek",
      "name": "Deepseek",
      "type": "openai-compat",
      "base_url": "https://api.deepseek.com/v1",
      "api_key": "$DEEPSEEK_API_KEY",
      "models": [{
        "id": "deepseek-chat", "name": "Deepseek V3",
        "context_window": 64000, "default_max_tokens": 5000,
        "cost_per_1m_in": 0.27, "cost_per_1m_out": 1.1,
        "cost_per_1m_in_cached": 1.1, "cost_per_1m_out_cached": 0.07,
        "can_reason": false, "supports_attachments": false
      }]
    }
  },
  "models": { "large": { "provider": "deepseek", "model": "deepseek-chat" } }
}
```

Same thing in modern `crushrc` (README:688):
```bash
provider add deepseek --type openai-compat \
  --base-url "https://api.deepseek.com/v1" \
  --api-key "$DEEPSEEK_API_KEY"

model add deepseek/deepseek-chat \
  --name "Deepseek V3" --context-window 64000 --default-max-tokens 5000 \
  --price-input 0.27 --price-output 1.1 --price-cache-create 1.1 --price-cache-hit 0.07

model large deepseek/deepseek-chat
```

### 3b. Anthropic-compatible (custom endpoint)
```bash
provider add custom-anthropic --type anthropic \
  --base-url "https://api.anthropic.com/v1" \
  --api-key "$ANTHROPIC_API_KEY" \
  --extra-header anthropic-version 2023-06-01
model add custom-anthropic/claude-sonnet-4-20250514 \
  --name "Claude Sonnet 4" --context-window 200000 --default-max-tokens 50000 \
  --can-reason true --supports-images true \
  --price-input 3 --price-output 15 --price-cache-create 3.75 --price-cache-hit 0.3
```
(README:707–724.) Stock Anthropic needs nothing: set `ANTHROPIC_API_KEY` and pick a model in the picker (ctrl+l).

### 3c. OpenRouter
```bash
export OPENROUTER_API_KEY=sk-or-...        # recognized natively
# or fully custom:
provider add openrouter-custom --type openrouter \
  --base-url "https://openrouter.ai/api/v1" \
  --api-key "$OPENROUTER_API_KEY"
```

### 3d. Ollama (local)
```bash
provider add ollama --type ollama --base-url "http://localhost:11434/v1"
model add ollama/llama3.3 --name "Llama 3.3" --context-window 128000
model large ollama/llama3.3
```
(docs/config/README.md:22–25.) No api_key required.

---

## 4. Multi-instance wrappers

### Per-project config
Drop `.crushrc` or `crush.json` in each repo root — it overrides the global config (§discovery). Use `data_directory` to isolate state if two projects share a directory tree.

### Env-based key switching (no config duplication)
Because `api_key: "$VAR"` resolves at load time, and `CRUSH_X` mirrors to `X` during provider setup:

```bash
# Instance A uses org account, instance B uses personal account
CRUSH_ANTHROPIC_API_KEY="$ORG_KEY" crush          # in terminal 1
CRUSH_ANTHROPIC_API_KEY="$PERSONAL_KEY" crush     # in terminal 2
```

### Sessions
- `crush --continue` / `-C` — resume most recent session (root.go:65-67)
- `crush --session <id>` / `-s <id>` — resume by ID (short 7-char hash shown on exit works too; root.go:205-208)
- `-s` and `--continue` are mutually exclusive. Non-interactive mode: `crush run "prompt"`.

### Wrapper script example — two providers, isolated instances

```bash
#!/usr/bin/env bash
# crush-switch: pick provider backend per invocation
# usage: crush-switch claude|deepseek [-- args]
set -euo pipefail

PROJ="$(pwd)"

case "${1:-}" in
  claude)
    shift
    exec env \
      ANTHROPIC_API_KEY="${ANTHROPIC_ORG_KEY:?}" \
      crush --cwd "$PROJ" "$@"
    ;;
  deepseek)
    shift
    # point the "anthropic" slot at DeepSeek's anthropic-compatible endpoint
    exec env \
      DEEPSEEK_API_KEY="${DEEPSEEK_API_KEY:?}" \
      CRUSH_GLOBAL_DATA="$HOME/.local/share/crush-ds" \
      crush --cwd "$PROJ" "$@"
    ;;
  *) echo "usage: $0 claude|deepseek" >&2; exit 1 ;;
esac
```

Alternative pure-config approach: per-project `.crushrc` files:

```bash
# ~/proj-a/.crushrc  (Anthropic)
provider add anthropic --type anthropic \
  --base-url "https://api.anthropic.com/v1" --api-key "$ORG_KEY"
model large anthropic/claude-sonnet-4-20250514
```
```bash
# ~/proj-b/.crushrc  (DeepSeek)
provider add ds --type openai-compat \
  --base-url "https://api.deepseek.com/v1" --api-key "$DEEPSEEK_API_KEY"
model large ds/deepseek-chat
option data-directory /absolute/path/to/.crush-ds   # optional state isolation
```
Since crushrc is plain Bash, conditional logic (`$HOSTNAME`, `$CRUSH_VERSION`, `source team-base.sh`) works natively — the documented pattern for shared/team base configs (docs/config/README.md "Composing configs").

---

## 5. LSP / formatter / debug tools, permissions, agent spawning

### LSP (schema.json:140 LSPConfig)
```jsonc
"lsp": {
  "go": {
    "disabled": false,
    "command": "gopls",
    "args": [],
    "env": { "GOPATH": "$HOME/go" },
    "filetypes": ["go", "mod"],
    "root_markers": ["go.mod"],
    "init_options": {},
    "options": {},
    "timeout": 30
  }
}
```
crushrc equivalent: `lsp add go --command gopls --env GOPATH "$HOME/go"` (docs/config/README.md:359). `options.auto_lsp` (default true) auto-starts servers from root markers; `options.debug_lsp` logs LSP traffic. LSPs start on demand.

### Formatter
There is **no** `formatter` block in current Crush's schema (that was OpenCode's `formatter` section). Formatting happens through the bash tool/hooks; if you want enforced formatting, wire it via a PreToolUse hook or instruct it in your context file. (Verified absent in schema.json.)

### Debug tooling
`options.debug`, `options.debug_lsp`, `CRUSH_UI_DEBUG`, plus built-in diagnostic tools `crush_info`, `crush_logs`, `job_output`, `job_kill` (config.go:787). Logs/state live in `data_directory` (default `.crush/`) and `$XDG_DATA_HOME/crush`.

### Hooks (related: pre-tool automation)
```jsonc
"hooks": { "PreToolUse": [
  { "name": "no-haskell", "matcher": "^bash$", "command": "./hooks/no-haskell.sh", "timeout": 30 }
]}
```
Fields: name, matcher (regex on tool name; empty = all), command (required), timeout (s, default 30). Hooks run before permission checks, receive a JSON payload on stdin, may return decisions (allow/deny); Claude Code-compatible output format supported (repo AGENTS.md, docs/hooks/).

### Permission model
- Unlisted non-read-only tools prompt interactively.
- `permissions.allowed_tools` (JSON) or `permissions allow` (crushrc) pre-approves tools.
- `permissions deny` / `options.disabled_tools` removes tools entirely.
- Read-only tools (glob, grep, ls, view, lsp_*, sourcegraph) are considered safe; the Task sub-agent gets only those.

### Agent spawning
Two built-ins (`coder`, `task`, §1.7). The `agent` tool lets coder spawn task sub-agents (read-only, large model by default). Extensibility via MCP tools and Agent Skills (`.agents/skills`, `.crush/skills`, `.claude/skills`, `.cursor/skills` — SKILL.md folders).

---

## 6. Differences from OpenCode ancestry

Crush forked OpenCode (FSL-1.1-MIT lineage preserved — LICENSE.md is FSL-1.1-MIT) but the config surface has diverged substantially:

| Aspect | OpenCode | Crush (current) |
|---|---|---|
| Config format | `opencode.json` (+ `opencode.jsonc`) | **`crushrc` (Bash)** primary; `crush.json` legacy/deprecated |
| Schema top-level | `provider`, `mcp` ("mcp_servers" era), `lsp`, `formatter`, `keybinds`, `theme`, `autoupdate` | `providers`, `mcp`, `lsp`, `options`, `permissions`, `tools`, `hooks`, `env` — no `formatter`, no `keybinds` in schema |
| Model selection | `model` string field | `models.large` / `models.small` slots |
| Agents/modes | `mode`s (build/plan), later `agent`s with prompt/permission/model per agent | built-in `coder` + `task` agents, not user-defined in config; extension via MCP/skills |
| Auth | `opencode auth login` (credentials store) | API keys pasted in picker, env vars, or `api_key` in config; OAuth tokens for some providers/MCP |
| Hooks | none historically | `hooks` (PreToolUse) with Claude Code-compatible protocol — Crush addition |
| Data/state | `~/.local/share/opencode` | `~/.local/share/crush` + per-project `.crush/` (`data_directory`) |
| Telemetry | minimal | PostHog pseudonymous metrics, `CRUSH_DISABLE_METRICS` / `DO_NOT_TRACK` |
| Provider catalog | static-ish | Catwalk live catalog (`CATWALK_URL` override, `crush update-providers`, auto-refresh unless disabled) |
| Shell integration | — | embedded first-class Bash interpreter (also powers crushrc itself and the bash tool) |
| Extra provider types | fewer | adds `hyper` (Charm's own), `bedrock`, `vercel`, `vertex`, `litellm`, `llamacpp`, `lmstudio`, `omlx` |

Legacy migration: old singular `context_path` → `context_paths`; `co_authored_by` attribution flag deprecated in favor of `trailer_style` (schema.json deprecations). If migrating from OpenCode JSON, expect to rename `mcp_servers`→`mcp`, restructure `provider`→`providers`+`models`, and drop `formatter`/`keybinds` sections; docs suggest asking Crush itself to convert configs (docs/config/README.md tip).

---

*All line references are to the charmbracelet/crush repository as cloned 2026-08-25 (main @ 06345cc).* 
