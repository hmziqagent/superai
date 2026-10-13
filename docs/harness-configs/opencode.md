# OpenCode (anomalyco/opencode) — Complete Configuration Reference

> Compiled 2026-08-25 from official docs, re-verified 2026-10-12 against the v2 line: [config](https://opencode.ai/docs/config/), [providers](https://opencode.ai/docs/providers/), [models](https://opencode.ai/docs/models/), [permissions](https://opencode.ai/docs/permissions/), [agents](https://opencode.ai/docs/agents/), [keybinds](https://opencode.ai/docs/keybinds/), [mcp-servers](https://opencode.ai/docs/mcp-servers/), plus every `/v2/docs/` page named in §6. Repo: `anomalyco/opencode` (moved out of `sst`; install `npm i -g opencode-ai@latest` or `brew install anomalyco/tap/opencode` for the v1 line). This doc covers the v1 schema as primary because `opencode-ai@latest` still ships it; §6 records what changes on the v2 line, which is stable and installs side by side under the same `opencode` binary name.

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
| `OPENCODE_CONFIG` | Path to an extra config file, layered between global and project; works the same on both lines ([docs/config#custom-path](https://opencode.ai/docs/config/#custom-path), v2 source at v2.0.27) |
| `OPENCODE_CONFIG_CONTENT` | Inline JSON config string, highest app-level precedence (runtime override); works the same on both lines |
| `OPENCODE_CONFIG_DIR` | Extra directory scanned like `.opencode/` (agents/commands/plugins); in current v1 (1.19.0) and v2 releases it also **relocates the global config root** (`$OPENCODE_CONFIG_DIR` replaces `~/.config/opencode` outright), so an inherited value overrides `XDG_CONFIG_HOME` |
| `OPENCODE_TUI_CONFIG` | Custom path to `tui.json` ([docs/config#tui](https://opencode.ai/docs/config/#tui)); still read by the 1.x line |
| `OPENCODE_CLI_CONFIG_CONTENT` | v2 only: inline JSON merged over `cli.json` for terminal settings ([v2 cli config](https://opencode.ai/v2/docs/cli/config/)) |
| `OPENCODE_DB` | v2 only: overrides the credential database path (default `$XDG_DATA_HOME/opencode/opencode.db`, channel-suffixed on non-default channels) |
| `XDG_CONFIG_HOME` | Moves the whole global config dir (`$XDG_CONFIG_HOME/opencode/opencode.json` instead of `~/.config/...`); both lines also honor `XDG_DATA_HOME`/`XDG_CACHE_HOME`/`XDG_STATE_HOME` for data, cache, and state |
| AWS: `AWS_ACCESS_KEY_ID`+`AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE`, `AWS_BEARER_TOKEN_BEDROCK`, `AWS_REGION`, `AWS_WEB_IDENTITY_TOKEN_FILE`/`AWS_ROLE_ARN` (EKS IRSA) | Bedrock auth; precedence: bearer token → credential chain ([docs/providers#amazon-bedrock](https://opencode.ai/docs/providers/#amazon-bedrock)) |
| `AZURE_RESOURCE_NAME` | Required for Azure OpenAI (endpoint derives from resource name) |
| Credentials store | v1: `/connect` saves keys to `~/.local/share/opencode/auth.json` ([docs/providers#credentials](https://opencode.ai/docs/providers/#credentials)). v2: same flow writes to `opencode.db` (SQLite) in that directory, importing a legacy `auth.json` once |

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

## 6. The v2 line (`@opencode/cli` 2.x)

Re-verified 2026-10-12. OpenCode now ships two stable lines that install side by side; both binaries are named `opencode` and answer `opencode --version` (1.x vs 2.x tells them apart):

| Line | Install | Docs | Notes |
|---|---|---|---|
| v1 (1.x) | `npm i -g opencode-ai@latest` (1.19.0), `brew install anomalyco/tap/opencode`, `curl -fsSL https://opencode.ai/install \| bash` (tracks GitHub `releases/latest`), scoop/choco/pacman | https://opencode.ai/docs/ | The schema in §1–§5. 1.19.0 also reads v2 keys through a compat layer (see below). |
| v2 (2.x) | `npm i -g @opencode/cli@latest` (2.0.27), `brew install anomalyco/tap/opencode-v2`, AUR `opencode-beta`; a desktop client (beta: macOS/Windows/Linux installers, `brew install --cask opencode-desktop`, also attached to GitHub releases) exists alongside both lines | https://opencode.ai/v2/docs/ | Renamed keys, `cli.json`, SQLite credential store. |

### What v2 keeps from v1
- Global config stays `$XDG_CONFIG_HOME/opencode/opencode.json(c)` (default `~/.config/opencode/`); file names, JSONC format, and the `$schema` URL are unchanged. `XDG_CONFIG_HOME`/`XDG_DATA_HOME`/`XDG_CACHE_HOME`/`XDG_STATE_HOME` all still root the global paths (source: `packages/util/src/global-roots.ts` at tag v2.0.27).
- `OPENCODE_CONFIG` (file layered between global and project) and `OPENCODE_CONFIG_CONTENT` (inline, highest) work unchanged (source: `packages/cli/src/server-process.ts` at v2.0.27 wires both into the config service).
- MCP still lives under `mcp` in `opencode.json`; plugins still auto-load from `.opencode/plugins/` and `~/.config/opencode/plugins/`.

### What v2 changes
- **Key renames** ([v2 config](https://opencode.ai/v2/docs/config/), verified against `packages/schema/src/config.ts` at v2.0.27): `provider` → `providers` (per-provider `npm`/`options` → `package`/`settings`; `settings.baseURL` replaces `options.baseURL`; a provider's credential env vars are declared in `env`), `agent` → `agents`, `command` → `commands`, `plugin` → `plugins` (array entries may be `package`+`options` objects; arrays from multiple files merge instead of override), `permission` → `permissions` (a map of tool rules becomes an ordered array of `{action, resource, effect}` rules), `autoupdate` → `update` (`true|false|"notify"` becomes `"auto"|"disable"|"notify"`), `snapshot` → `snapshots`, `attachment.image` → `media`. Dropped: `small_model`, `disabled_providers`, `enabled_providers`, `keybinds`, `theme` (TUI settings move to `cli.json`). New keys: `username`, `skills`, `references`, `websearch`, `warming`, `worktree`, `tool_output`.
- **MCP nesting** ([v2 mcp-servers](https://opencode.ai/v2/docs/mcp-servers/)): servers sit under `mcp.servers.<name>`, not directly under `mcp`; the per-server `enabled: false` flag becomes `disabled: true`; `mcp.timeout` (`startup`/`request`) is new. The docs state it outright: "V2 does not place server names directly under mcp."
- **Terminal settings file**: v1 `tui.json` becomes `cli.json` in the same config dir (`$XDG_CONFIG_HOME/opencode/cli.json`, JSONC, written 0600 with a lock and atomic rename), with `OPENCODE_CLI_CONFIG_CONTENT` as its inline override; schema at `https://opencode.ai/v2/cli.json`. v2 auto-migrates an existing `tui.json` (plus state `kv.json`) into `cli.json` on first run ([v2 cli config](https://opencode.ai/v2/docs/cli/config/)).
- **Credential storage**: v1's `~/.local/share/opencode/auth.json` becomes a SQLite database at `$XDG_DATA_HOME/opencode/opencode.db` (channel-suffixed names like `opencode-beta.db` on non-default channels; `OPENCODE_DB` overrides the path). A one-time database migration imports a legacy `auth.json` when present (source at v2.0.27: `packages/core/src/database/migration/20260805200742_import_legacy_credentials.ts`).
- **Project layering** ([v2 config](https://opencode.ai/v2/docs/config/)): v2 walks from cwd to the filesystem root, merging every direct `opencode.json(c)` and every `.opencode/opencode.json(c)` it passes; `.opencode` files override direct ones, nearer overrides farther, global stays lowest. `OPENCODE_CONFIG_CONTENT` remains highest. v2 also discovers `.claude/` and `.agents/` directories during the same walk (global `~/.claude` and `~/.agents` included), so a Claude Code installation can feed an OpenCode v2 run; v1 never reads those.
- **`OPENCODE_CONFIG_DIR` now relocates the global config root** in both current lines (v1.19.0 `Global` service and v2.0.27 `Global.layerWith` both resolve `OPENCODE_CONFIG_DIR ?? XDG_CONFIG_HOME/opencode`). It still doubles as the extra scanned directory from §1. Wrappers that isolate via `XDG_CONFIG_HOME` must clear this variable or the relocation is bypassed.

### Cross-era compat (verified in source, both directions)
- v2 binaries read v1 keys by normalizing them up, with diagnostics (`packages/core/src/config/normalize.ts` at v2.0.27). A v1-era config file keeps working under v2.
- v1 binaries (1.19.0) read v2 keys by lowering them down, with one hard failure: a top-level `permissions` key (or `permissions` inside an `agent`/`agents`/`mode` entry) throws "V2 permissions are not supported by OpenCode V1. Use V1 permission rules or run opencode2" and the config fails to load. `plugins`, `providers`, `websearch`, and `warming` are dropped with diagnostics (silently ignored); `agents`, `commands`, `snapshots`, `media`, and `mcp.servers` lower to their v1 equivalents (source: `packages/opencode/src/config/v2-compat.ts` at v1.19.0).

Not verified upstream: whether v2 keeps the v1 managed-config dirs (`/etc/opencode/`, `/Library/Application Support/opencode/`, `%ProgramData%\opencode`) and the `.well-known/opencode` remote layer (the wellknown loader is present in v2 source but the admin dirs are not documented on `/v2/docs/`); where the desktop client stores its own Electron state beyond the shared server config.

---

## 7. v2 history schema (frozen at v2.0.27)

Frozen 2026-10-13 against `anomalyco/opencode` tag `v2.0.27` (commit `1f4d07a4a519`). The superai reader (`crates/superai-usage/src/reader/opencode.rs`) mirrors one upstream query: `SessionStats.get` in `packages/core/src/session/stats.ts`, which reads the two tables defined in `packages/core/src/session/sql.ts` (baseline DDL in `packages/core/src/database/schema.gen.ts`; all at v2.0.27).

### `session_message` (source: `packages/core/src/session/sql.ts` at v2.0.27)

| Column | Type | Notes |
|---|---|---|
| `id` | text, PK | message id, `msg_` prefix |
| `session_id` | text, NOT NULL | FK to `session_v2.id`, ON DELETE CASCADE |
| `type` | text, NOT NULL | message tag, union below |
| `seq` | integer, NOT NULL | per-session order, unique with `session_id` |
| `time_created` | integer, NOT NULL | epoch milliseconds |
| `time_updated` | integer, NOT NULL | epoch milliseconds |
| `data` | text, NOT NULL | JSON payload, shape below |

### `session_v2` (same source)

Columns the usage query reads:

| Column | Type | Notes |
|---|---|---|
| `id` | text, PK | session id |
| `fork_session_id` | text | set when this session forked another |
| `time_created` | integer, NOT NULL | epoch milliseconds; stats.ts compares forked rows against this column, not the separate `fork_boundary` column |

The table also carries `parent_id` (set on subagent sessions), project/workspace binding, directory, title, per-session `cost` and `tokens_*` summaries (all `DEFAULT 0 NOT NULL`), and lifecycle timestamps; the usage query reads none of those.

### Payload paths (`session_message.data`)

`data` stores the encoded `SessionMessage.Info` with `type` and `id` stripped into their own columns (`SessionMessageData` in `session/sql.ts`). The message union (`packages/schema/src/session-message.ts`): `agent-switched`, `model-switched`, `location-switched`, `user`, `synthetic`, `system`, `skill`, `shell`, `assistant`, `compaction`, `idle`. Only `assistant` rows carry a model and tokens:

- `$.model.providerID` and `$.model.id`: `model` is required on assistant rows and encodes as `{id, providerID, variant?}` (`Model.Ref` in `packages/schema/src/model.ts`); the keys are camelCase in the stored JSON.
- `$.tokens.input`, `$.tokens.output`, `$.tokens.reasoning`, `$.tokens.cache.read`, `$.tokens.cache.write`: `tokens` is optional; when present, all five fields are finite numbers (`TokenUsage.Info` in `packages/schema/src/token-usage.ts`).
- `$.model.variant`: optional, upstream uses it to group models; superai does not read it.
- `$.cost`: optional USD number; superai does not read it either.

### Fork rule

Every query in stats.ts joins `session_v2` on `session.id = message.session_id` and keeps a row only when

```sql
(session.fork_session_id IS NULL OR message.time_created >= session.time_created)
```

A fork copies the parent's history, so pre-fork rows would otherwise count twice; rows older than the fork session's own `time_created` stay attributed to the parent. The same predicate appears in all four stats queries (minimum-time, message scan, both tool queries).

### Type filter and compaction usage

The message scan filters `type IN ('user', 'assistant')` and aggregates tokens and cost from assistant rows only; user rows just feed the prompt count. Compaction usage never enters that scan: upstream reads it from durable `event` rows of type `session.usage.recorded` where `json_extract(data, '$.source') = 'compaction'` (stats.ts lines 288-315; `UsageRecorded` in `packages/schema/src/session-event.ts` carries `source` (`title` or `compaction`), `cost`, and the same token object). Title-generation usage is excluded upstream. Compaction also writes a `session_message` row of type `compaction` carrying its own `tokens` and `cost`, which the `type` filter drops.

The superai reader selects the same two tables with `type = 'assistant'` and the same fork predicate, extracts `$.model.providerID`, `$.model.id`, and the five token paths, folds `reasoning` into `output_tokens`, and wraps `data` in a `json_valid` check so a malformed payload counts as skipped rather than failing the scan. It does not read the `event` table, so compaction usage is outside superai's totals.

---

## Quick gotchas

- `theme`, `keybinds`, TUI toggles belong in `tui.json` now; leaving them in `opencode.json` works only until migration fails. On the v2 line the same settings live in `cli.json` and migrate automatically.
- `share` is the sharing key (not "autoshare"); values manual/auto/disabled.
- `formatter` and `lsp` are **opt-in**: omitted = disabled; `true` = enable all built-ins.
- Custom provider IDs must match exactly between `/connect` and `opencode.json`, or credentials won't bind.
- Wrong npm package is the top custom-provider failure: OpenAI-compatible chat → `@ai-sdk/openai-compatible`; responses-API models → `@ai-sdk/openai` (per-model npm override possible). On v2 the package names change to `@opencode/ai/providers/*`.
- Under a 1.x binary, a v2 `permissions` key makes the whole config fail to load; superai refuses that write (era gate). Everything else cross-loads with diagnostics.
- `OPENCODE_TUI_CONFIG` (v1) and `OPENCODE_CLI_CONFIG_CONTENT` (v2) redirect terminal settings; isolation wrappers must also clear `OPENCODE_CONFIG_DIR` or it overrides the config root on both lines.
