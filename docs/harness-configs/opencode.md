# OpenCode (sst/opencode → anomalyco) — Complete Configuration Reference

> Compiled 2026-08-25 from official docs: [config](https://opencode.ai/docs/config/), [providers](https://opencode.ai/docs/providers/), [models](https://opencode.ai/docs/models/), [permissions](https://opencode.ai/docs/permissions/), [agents](https://opencode.ai/docs/agents/), [keybinds](https://opencode.ai/docs/keybinds/), [mcp-servers](https://opencode.ai/docs/mcp-servers/). Repo: `sst/opencode` (now under `anomalyco`; install `npm i -g opencode-ai@latest` or `brew install anomalyco/tap/opencode`). Note: a v2 docs preview exists at https://opencode.ai/v2/docs/providers/ with renamed keys (`provider(s)`, `settings.baseURL`, `package`, `env`, `headers`, `body`) — this doc covers the current stable v1 schema.

---

## 1. Config files & schema

### File formats & locations ([docs/config](https://opencode.ai/docs/config/#format))
- JSON **or JSONC** (comments allowed), always named `opencode.json`/`opencode.jsonc`.
- **Global:** `~/.config/opencode/opencode.json(.c)` — user prefs (providers, models, permissions). TUI-only settings go in `~/.config/opencode/tui.json(.c)`.
- **Per-project:** `opencode.json` in project root (same schema; safe to commit). On startup OpenCode looks in cwd then traverses up to the nearest Git root.
- **TUI config:** `tui.json` next to either of the above (scroll_speed, cursor, mouse, attention notifications/sound, diff_style); custom path via `OPENCODE_TUI_CONFIG`. Legacy `theme`/`keybinds`/`tui` keys inside `opencode.json` are deprecated but auto-migrated ([docs/config#tui](https://opencode.ai/docs/config/#tui)).
- Managed/admin (cannot be overridden): `/Library/Application Support/opencode/` (macOS), `/etc/opencode/` (Linux), `%ProgramData%\opencode` (Windows); macOS also reads MDM `.mobileconfig` domain `ai.opencode.managed` ([docs/config#managed-settings](https://opencode.ai/docs/config/#managed-settings)).
- Remote org defaults: fetched from `.well-known/opencode` endpoint when authenticating with a supporting provider.

### Layering order (later overrides earlier; merge, not replace) ([docs/config#precedence-order](https://opencode.ai/docs/config/#precedence-order))
1. Remote config (`.well-known/opencode`)
2. Global `~/.config/opencode/opencode.json`
3. Custom config — `OPENCODE_CONFIG` env var (file path)
4. Project `opencode.json`
5. `.opencode/` directories (agents, commands, plugins)
6. Inline config — `OPENCODE_CONFIG_CONTENT` env var (runtime overrides; highest app-level tier)
7. Managed config files (admin dirs above)
8. macOS managed preferences (.mobileconfig via MDM)

Non-conflicting settings across all files are preserved. `OPENCODE_CONFIG_DIR` points to an extra directory searched for agents/commands/plugins like `.opencode/`, loaded after global+project so it can override them.

### Schema top-level keys ([schema](https://opencode.ai/config.json), TUI: [tui.json](https://opencode.ai/tui.json))

| Key | Purpose |
|---|---|
| `$schema` | `"https://opencode.ai/config.json"` — enables editor autocomplete/validation |
| `theme` | UI theme name (**deprecated in opencode.json** → set in `tui.json`: `"theme": "tokyonight"`) |
| `model` | Default model, `provider_id/model_id` (e.g. `"anthropic/claude-sonnet-4-5"`); override per-run with `-m/--model` |
| `small_model` | Cheap model for lightweight tasks (title gen); default: cheapest model from your provider, else main model |
| `share` | Sharing: `"manual"` (default) \| `"auto"` \| `"disabled"` (note: task brief said `autoshare`; the real key is `share`) |
| `autoupdate` | `true` \| `false` \| `"notify"` (auto-download updates at startup; notify = banner only; no-op if installed via package manager) |
| `disabled_providers` | Array of provider IDs never loaded even if creds/env exist (`["openai","gemini"]`) |
| `enabled_providers` | Allowlist inverse; if both set, `disabled_providers` wins |
| `provider` | Provider config map — see below |
| `agent` | Agent definitions — see §5 |
| `default_agent` | Primary agent used when none specified (`"build"`, `"plan"`, or custom; falls back to build w/ warning) |
| `subagent_depth` | Nesting depth of subagent→subagent calls; default `1` (0 disables subagents) |
| `mcp` | MCP servers — see below |
| `permission` | Permission rules — see §5 |
| `tools` | Legacy tool booleans (`{"write": false, "bash": false}`); deprecated since v1.1.1, merged into `permission` |
| `keybinds` | **Deprecated in opencode.json** → `tui.json` `keybinds` (merged over built-in defaults) |
| `lsp` | `true` to enable built-ins, object to configure/disable (`{"typescript": {"disabled": true}}`), omit = off ([docs/lsp](https://opencode.ai/docs/lsp)) |
| `formatter` | Same shape: `true`, or `{"prettier": {"disabled": true}, "custom-prettier": {"command": ["npx","prettier","--write","$FILE"], "environment": {...}, "extensions": [".js",".ts"]}}` ([docs/formatters](https://opencode.ai/docs/formatters)) |
| `instructions` | Extra instruction files/globs: `["CONTRIBUTING.md", ".cursor/rules/*.md"]` |
| `plugin` | npm plugins: `["opencode-helicone-session"]` (files also in `.opencode/plugins/` or `~/.config/opencode/plugins/`) |
| `command` | Custom slash commands (`{"test": {"template": "...", "description": "...", "agent": "build", "model": "..."}}`; `$ARGUMENTS` placeholder; also markdown in `~/.config/opencode/command(s)/`) |
| `server` | For `opencode serve/web`: `port`, `hostname`, `mdns`, `mdnsDomain`, `cors: ["https://app.example.com"]` |
| `shell` | Shell for interactive terminal + agent bash (`"pwsh"`, abs path, or short name) |
| `snapshot` | `false` disables git-snapshot undo tracking (saves disk on big repos; loses UI rollback) |
| `compaction` | `{"auto": true, "prune": false, "reserved": 10000}` — context compaction behavior |
| `watcher` | `{"ignore": ["node_modules/**", "dist/**"]}` glob ignores for file watching |
| `attachment.image` | `{"auto_resize": true, "max_width": 2000, "max_height": 2000, "max_base64_bytes": 5242880}` |
| `experimental.policies` | e.g. `[{"effect":"deny","action":"provider.use","resource":"openai"}]` |
| `experimental` | Catch-all for features under active development |

### Variable substitution in any config value ([docs/config#variables](https://opencode.ai/docs/config/#variables))
- `{env:VARIABLE_NAME}` — env var; empty string if unset.
- `{file:path}` — file contents (relative to config dir, or absolute/`~/`).

### provider.* options ([docs/providers#config](https://opencode.ai/docs/providers/#config))
```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "model": "anthropic/claude-sonnet-4-5",
  "small_model": "anthropic/claude-haiku-4-5",
  "provider": {
    "anthropic": {
      // "npm": "@ai-sdk/anthropic",          // optional SDK package override (built-ins preloaded)
      "options": {
        "baseURL": "https://api.anthropic.com/v1",   // proxy/custom endpoint
        "apiKey": "{env:ANTHROPIC_API_KEY}",         // or {file:~/.secrets/key}
        "timeout": 600000,        // ms, default 300000; false = disable
        "chunkTimeout": 30000,    // abort if no stream chunk within this window
        "setCacheKey": true,
        "headers": { "Authorization": "Bearer custom-token" }  // custom headers
      },
      "models": {                                   // extend/override model catalog
        "claude-sonnet-4-5": {
          "name": "Claude Sonnet 4.5",
          "limit": { "context": 200000, "output": 65536 },   // token limits (auto from models.dev for known providers)
          "options": { "thinking": { "type": "enabled", "budgetTokens": 16000 } },
          "variants": { "high": { "reasoningEffort": "high" }, "fast": { "disabled": true } }
        }
      },
      "blacklist": ["claude-opus-4-20250514"],      // hide from /models picker
      "whitelist": ["claude-sonnet-4-5"]            // inverse; combined: whitelist then blacklist
    }
  },
  "disabled_providers": []
}
```

### mcp servers ([docs/mcp-servers](https://opencode.ai/docs/mcp-servers/))
```jsonc
{
  "mcp": {
    "local-server": {                       // stdio server
      "type": "local",
      "command": ["npx", "-y", "@modelcontextprotocol/server-everything"],
      "enabled": true,
      "environment": { "MY_ENV_VAR": "value" }
    },
    "remote-server": {                      // HTTP/SSE server
      "type": "remote",
      "url": "https://jira.example.com/mcp",
      "enabled": true                       // false = defined but off (also how you opt into org-pushed remote defaults)
      // "headers": { ... }
    }
  }
}
```

---

## 2. Environment variables

There is **no dedicated env-var docs page** (the old `/docs/env-vars-and-secrets` URL 404s); variables are documented inline across pages. Full list:

| Var | Effect |
|---|---|
| `<PROVIDER>_API_KEY` pattern | Auto-detection: each provider in the [models.dev](https://models.dev/) catalog declares its env var (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `GROQ_API_KEY`, `CEREBRAS_API_KEY`, …). Setting it makes that provider appear without `/connect`. Custom providers can declare their own via the provider's `env` field (see v2 preview: `"env": ["ACME_API_KEY"]`, [v2 docs/providers](https://opencode.ai/v2/docs/providers/)). Quirk: setting an unrelated var does nothing — only the exact declared names count; disable unwanted ones with `disabled_providers`. |
| `OPENCODE_CONFIG` | Path to an extra config file, layered between global and project ([docs/config#custom-path](https://opencode.ai/docs/config/#custom-path)) |
| `OPENCODE_CONFIG_CONTENT` | Inline JSON config string — highest app-level precedence (runtime override) |
| `OPENCODE_CONFIG_DIR` | Extra directory scanned like `.opencode/` (agents/commands/plugins) |
| `OPENCODE_TUI_CONFIG` | Custom path to `tui.json` ([docs/config#tui](https://opencode.ai/docs/config/#tui)) |
| `XDG_CONFIG_HOME` | Moves the whole global config dir (`$XDG_CONFIG_HOME/opencode/opencode.json` instead of `~/.config/...`) |
| AWS: `AWS_ACCESS_KEY_ID`+`AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE`, `AWS_BEARER_TOKEN_BEDROCK`, `AWS_REGION`, `AWS_WEB_IDENTITY_TOKEN_FILE`/`AWS_ROLE_ARN` (EKS IRSA) | Bedrock auth; precedence: bearer token → credential chain ([docs/providers#amazon-bedrock](https://opencode.ai/docs/providers/#amazon-bedrock)) |
| `AZURE_RESOURCE_NAME` | Required for Azure OpenAI (endpoint derives from resource name) |
| Credentials store | `/connect` saves keys to `~/.local/share/opencode/auth.json` ([docs/providers#credentials](https://opencode.ai/docs/providers/#credentials)) |

---

## 3. Third-party & local models

OpenCode uses the [AI SDK](https://ai-sdk.dev/) + the [models.dev](https://models.dev/) catalog → 75+ providers preloaded ([docs/models](https://opencode.ai/docs/models/)). Model IDs are always `provider_id/model_id`.

### Generic custom provider (any OpenAI-compatible endpoint) ([docs/providers#custom-provider](https://opencode.ai/docs/providers/#custom-provider))
```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "provider": {
    "myprovider": {
      "npm": "@ai-sdk/openai-compatible",     // /v1/chat/completions; use "@ai-sdk/openai" for /v1/responses APIs;
                                              // use provider-specific packages (@ai-sdk/cerebras etc.) where they exist
      "name": "My Provider Display Name",
      "options": {
        "baseURL": "https://api.myprovider.com/v1",
        "apiKey": "{env:MY_PROVIDER_API_KEY}",   // optional if using /connect auth
        "headers": { "HTTP-Referer": "https://mysite.com" }
      },
      "models": {
        "my-model-id": {
          "name": "My Model",
          "limit": { "context": 200000, "output": 65536 }
        }
      }
    }
  },
  "model": "myprovider/my-model-id"
}
```
Workflow: `/connect` → **Other** → enter provider id + key (stores in auth.json) → add the block above with matching id → `/models`. `limit.context/output` matter for local models so OpenCode knows remaining context.

### Local runtimes
- **Ollama** ([docs/providers#ollama](https://opencode.ai/docs/providers/#ollama)): `"npm": "@ai-sdk/openai-compatible"`, `"baseURL": "http://localhost:11434/v1"`, models keyed by tag (`"qwen3-coder": {}` after `ollama pull qwen3-coder`). Ollama Cloud uses an API key via `/connect`.
- **LM Studio** ([docs/providers#lm-studio](https://opencode.ai/docs/providers/#lm-studio)): `"baseURL": "http://127.0.0.1:1234/v1"`, e.g. `"models": {"google/gemma-3n-e4b": {"name": "Gemma"}}` → usable as `lmstudio/google/gemma-3n-e4b`.
- **Atomic Chat** (desktop local LLMs): `"baseURL": "http://127.0.0.1:1337/v1"`; model ids must match `GET /v1/models`.
- **OpenRouter**: either `/connect` (preloaded) or explicit block: `"npm": "@ai-sdk/openai-compatible"`, `"baseURL": "https://openrouter.ai/api/v1"`, `"apiKey": "{env:OPENROUTER_API_KEY}"`, models keyed by vendor slug (`"anthropic/claude-sonnet-4"`).
- **Anthropic-compatible proxies**: reuse provider id `anthropic` with just `"options": {"baseURL": "https://proxy.example.com/v1"}` (keeps the Anthropic SDK/model list), or define a fresh provider id with `npm: @ai-sdk/openai-compatible` pointing at the proxy.
- **Bedrock inference profiles**: override ARN per model — `"models": {"anthropic-claude-sonnet-4.5": {"id": "arn:aws:bedrock:...:application-inference-profile/yyy"}}`.
- **OpenCode Zen / Go**: first-party curated model lists, connect via `/connect` ([docs/zen](https://opencode.ai/docs/zen)).

Model resolution priority ([docs/models#loading-models](https://opencode.ai/docs/models/#loading-models)): `-m` flag → config `model` → last-used model → internal priority order. Per-model `variants` switch reasoning effort (Anthropic `high|max`; OpenAI `none…xhigh`; Google `low|high`); cycle with `variant_cycle` keybind.

---

## 4. Multi-instance wrappers (isolated configs/providers)

Two independent isolation knobs:
- **`OPENCODE_CONFIG=path/to/file.json`** — adds one extra config layer between global and project. Global still applies underneath (and `auth.json` is shared).
- **`XDG_CONFIG_HOME=$DIR`** — relocates *everything*: global config, `tui.json`, `agents/`, plugins, and data dir become `$DIR/opencode/...`. Total isolation per instance (recommended when two instances must not share providers/auth).

Layering recap for wrapper design: remote → global → `OPENCODE_CONFIG` → project → `.opencode/` → `OPENCODE_CONFIG_CONTENT` → managed. So a wrapper should inject instance-specific provider/model/permission settings via `OPENCODE_CONFIG`, and use `XDG_CONFIG_HOME` only when full separation is needed.

Wrapper script example — two parallel instances with different providers:

```bash
#!/usr/bin/env bash
# oc-anthropic / oc-openrouter: isolated OpenCode instances
set -euo pipefail
BASE="$HOME/.opencode-instances"
mkdir -p "$BASE"

oc_wrapper() {
  local name="$1" provider_cfg="$2"; shift 2
  local dir="$BASE/$name"
  mkdir -p "$dir"
  # full isolation: own global config + own auth.json (skip XDG line to share ~/.config/opencode instead)
  export XDG_CONFIG_HOME="$dir"
  export OPENCODE_CONFIG="$dir/provider.json"
  cat > "$dir/provider.json" <<EOF
$provider_cfg
EOF
  exec opencode "$@"
}

case "$(basename "$0")" in
  oc-anthropic)
    oc_wrapper anthropic '{
      "$schema": "https://opencode.ai/config.json",
      "model": "anthropic/claude-sonnet-4-5",
      "provider": { "anthropic": { "options": { "apiKey": "{env:ANTHROPIC_API_KEY}" } } },
      "autoupdate": false, "share": "disabled"
    }' "$@" ;;
  oc-openrouter)
    oc_wrapper openrouter '{
      "$schema": "https://opencode.ai/config.json",
      "model": "openrouter/anthropic/claude-sonnet-4",
      "provider": { "openrouter": {
        "npm": "@ai-sdk/openai-compatible",
        "name": "OpenRouter",
        "options": { "baseURL": "https://openrouter.ai/api/v1", "apiKey": "{env:OPENROUTER_API_KEY}" }
      }},
      "permission": { "bash": "ask", "edit": "allow" },
      "autoupdate": false, "share": "disabled"
    }' "$@" ;;
esac
```
Symlink as `~/bin/oc-anthropic` + `~/bin/oc-openrouter`. Lighter variant without XDG: keep one global config and pass only `OPENCODE_CONFIG=/path/per-instance.json` — providers/models/permissions merge per instance while auth.json stays shared. Verify resolved config with `opencode debug config`.

---

## 5. Agents, subagents, permissions

### Agents ([docs/agents](https://opencode.ai/docs/agents/))
Two types: **primary** agents (main conversation; cycle with Tab) and **subagents** (invoked automatically by primary agents or manually via `@mention`). Built-in primary: `build` (all tools, default), `plan` (edits+bash default to `ask`), plus hidden system agents `compaction`, `title`, `summary`. Built-in subagents: `general` (full access except todo), `explore` (read-only codebase search), `scout` (read-only dependency/docs research in managed cache).

Define in JSON (`opencode.json` → `agent`) or Markdown frontmatter in `~/.config/opencode/agents/` (global) or `.opencode/agents/` (project); filename becomes agent name. Create scaffold: `opencode agent create`.

```jsonc
{ "agent": {
  "code-reviewer": {
    "description": "Reviews code for best practices",       // required (drives auto-delegation)
    "mode": "subagent",                                     // "primary" | "subagent"
    "model": "anthropic/claude-sonnet-4-5",                 // optional; else session model
    "prompt": "You are a code reviewer...",                 // supports {file:./prompts/x.txt}
    "temperature": 0.1,                                     // 0–1 (defaults: 0 most models, 0.55 Qwen)
    "top_p": 0.9,                                           // alternative randomness control
    "steps": 20,                                            // max agentic iterations before forced text reply
    "tools": { "write": false, "edit": false },             // legacy boolean toggles (still honored)
    "permission": { "edit": "deny", "bash": "ask", "webfetch": "deny" },
    "reasoningEffort": "high", "textVerbosity": "low"       // unknown keys pass through to provider
}}}
```
Markdown equivalent: frontmatter keys `description`, `mode`, `model`, `temperature`, `permission`, then prompt body. Agent-level `permission` merges over global (agent wins); agent model options override global `provider.models.*.options`.

### Permissions ([docs/permissions](https://opencode.ai/docs/permissions/))
Each rule resolves to `"allow"` / `"ask"` / `"deny"`. Whole config may be a single string (`"permission": "allow"`). Keys (tool names + guards):

`read` (file path; **`.env*` denied by default**, rest allow) · `edit` (all modify ops: edit/write/patch) · `glob` · `grep` · `bash` (parsed command) · `task` (subagent launches) · `skill` · `lsp` · `question` · `webfetch` (URL) · `websearch` (query) · `external_directory` (paths outside cwd; default `ask`) · `doom_loop` (same tool call ×3 identical input; default `ask`). Everything else defaults `allow`.

```jsonc
{ "permission": {
    "*": "ask",
    "bash": { "*": "ask", "git status*": "allow", "rm -rf *": "deny" },   // last matching rule wins
    "external_directory": { "~/projects/personal/**": "allow" }
}}
```
Wildcards: `*` (any chars) and `?` (one char); `~`/`$HOME` expand at pattern start. Ask prompts offer once / always (session-scoped pattern approval) / reject. CLI/TUI auto mode: `opencode --auto`, `opencode run --auto` — approves everything except explicit `deny`.

---

## Quick gotchas
- `theme`, `keybinds`, TUI toggles belong in `tui.json` now; leaving them in `opencode.json` works only until migration fails.
- `share` is the sharing key (not "autoshare"); values manual/auto/disabled.
- `formatter` and `lsp` are **opt-in**: omitted = disabled; `true` = enable all built-ins.
- Custom provider IDs must match exactly between `/connect` and `opencode.json`, or credentials won't bind.
- Wrong npm package is the top custom-provider failure: OpenAI-compatible chat → `@ai-sdk/openai-compatible`; responses-API models → `@ai-sdk/openai` (per-model npm override possible).
