# MiMo Code (Xiaomi) — Complete Configuration Reference

> Researched 2026-08-25 from primary sources:
> - Repo: [XiaomiMiMo/MiMo-Code](https://github.com/XiaomiMiMo/MiMo-Code) (cloned to `/home/remixer/mimo-code-src`; engine at `packages/opencode/src/`, config schema modules at `packages/opencode/src/config/*.ts`)
> - Official docs: [mimo.xiaomi.com/mimocode/env-vars](https://mimo.xiaomi.com/mimocode/env-vars), [cli-options](https://mimo.xiaomi.com/mimocode/cli-options)
> - README.md "Configuration" section ([README.md](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/README.md))
>
> Note: MiMo Code is an OpenCode fork rebranded (`packages/opencode/`); many internal env aliases retain the `OPENCODE_` prefix, but user-facing vars are `MIMOCODE_*`.

---

## 1. Config files: locations & schema

### File locations ([README.md §File Locations](https://github.com/XiaomiMiMo/MiMo-Code#file-locations); source: `config/config.ts` `globalConfigFile()`, `loadGlobal()`; `config/paths.ts`; `shared/src/global.ts`)

| File | Project-level | Global |
|---|---|---|
| Main config | `.mimocode/mimocode.jsonc` (also `.json`) | `~/.config/mimocode/mimocode.jsonc` (also `mimocode.json`, legacy `config.json`) |
| TUI config | `.mimocode/tui.json` | `~/.config/mimocode/tui.json` |
| Auth credentials | — | `~/.local/share/mimocode/auth.json` (mode 0600; source `auth/index.ts`) |
| MCP OAuth tokens | — | `~/.local/share/mimocode/mcp-auth.json` (source `mcp/auth.ts`) |

- All XDG paths overridable by `MIMOCODE_HOME` (must be absolute; holds `config/ data/ state/ cache/` subdirs — `shared/src/global.ts` `resolveMimocodeHome()`).
- Data dirs: data=`~/.local/share/mimocode/` (SQLite DB, auth.json, memory, logs), state=`~/.local/state/mimocode/` (`kv.json`, recent models `model.json`), cache=`~/.cache/mimocode/` (LSPs, model catalog, skills). Windows/macOS fall under `%LOCALAPPDATA%\mimocode\` / `~/Library/Application Support/mimocode/`.
- JSONC and JSON both accepted; `$schema` auto-injected on load: main = `https://mimo.xiaomi.com/mimocode/config.json`, TUI = `https://mimo.xiaomi.com/mimocode/tui.json` (`config.ts` `loadConfig()`).

### Merge order (lowest→highest precedence; `config.ts` `loadInstanceState()`)

1. Well-known remote configs (`auth.json` entries of type `wellknown` → `<url>/.well-known/opencode`)
2. Global dir files (`config.json` → `mimocode.json` → `mimocode.jsonc`)
3. `MIMOCODE_CONFIG` custom file
4. Per-project `.mimocode/mimocode.{json,jsonc}` walked up to worktree root
5. `.mimocode/` directories + `MIMOCODE_CONFIG_DIR`
6. `MIMOCODE_CONFIG_CONTENT` inline JSON
7. Console/org-managed config (`<url>/api/config`) + managed config dir + macOS MDM `.mobileconfig` prefs (`config/managed.ts`)
8. Claude Code compat merges (`.claude.json` mcpServers unless `MIMOCODE_DISABLE_CLAUDE_CODE_MCP`)
9. CLI flags: `MIMOCODE_PERMISSION` JSON, `--dangerously-skip-permissions` (allow-all base injected **under** user config so explicit deny wins)

Arrays in `instructions` are concatenated+deduped across levels instead of replaced (`mergeConfigConcatArrays`). Legacy top-level `theme`/`keybinds`/`tui` keys are dropped with a warning — move them to `tui.json`. A legacy TOML `~/.config/mimocode/config` file is auto-migrated to `config.json`.

### Main schema (`config/config.ts` `InfoSchema`, ~v0.1.13) — every field

| Key | Type / values | Purpose |
|---|---|---|
| `$schema` | string | Schema URL for editor validation |
| `logLevel` | DEBUG\|INFO\|WARN\|ERROR | Log level |
| `server` | object | For `mimo serve`/`web`: `port`, `hostname`, `mdns`, `cors` (see `config/server.ts`) |
| `llmServer` | object | Token-lifetime defaults for local LLM server (`mimo llm-server`; `config/llm-server.ts`) |
| `command` | Record<name, Info> | Custom commands (`config/command.ts`), docs: mimo.xiaomi.com/mimocode/commands |
| `skills` | `{ paths?: string[], urls?: string[] }` | Extra skill folders / well-known skill URLs (`config/skills.ts`) |
| `compose` | `{ docs?, docs_absolute? }` | Compose-mode docs directory (default `docs/compose`) (`config/compose.ts`) |
| `watcher` | `{ ignore: string[] }` | File-watcher ignore globs |
| `snapshot` | boolean (default true) | Filesystem snapshot tracking; false disables undo/revert of file changes |
| `plugin` | Spec[] | Plugins (npm pkg, URL, or local path) (`config/plugin.ts`) |
| `share` | manual\|auto\|disabled | Session sharing (`autoshare` boolean deprecated alias) |
| `autoupdate` | true\|false\|"notify" | Auto-update behavior |
| `disabled_providers` / `enabled_providers` | string[] | Filter auto-loaded providers; `enabled_providers` is an exclusive allowlist |
| `model` | `"provider/model"` | Default model |
| `small_model` | provider/model | Title-gen etc. |
| `vision_model` | provider/model | Vision subagent tasks (auto-picks in-house→cheapest if unset) |
| `model_groups` | Record<name, modelID \| {default, models[]}> | Named capability tiers (ultra/standard/lite built-in); group name usable wherever provider/model is accepted |
| `default_agent` | string | Fallback primary agent (else `build`) |
| `username` | string | Display username |
| `mode` | Record<string, AgentInfo> | @deprecated alias of `agent` |
| `agent` | Record<string, AgentInfo> | Agents; built-ins: plan, build, general, explore, title, summary, compaction (schema below) |
| `provider` | Record<id, ProviderInfo> | Provider overrides (Section 3) |
| `retry` | Info | Retry budgets for requests/streams/network recovery (`config/retry.ts`) |
| `mcp` | Record<name, Local\|Remote\|{enabled:false}> | MCP servers (Section 5) |
| `formatter` | Info | Formatters (`config/formatter.ts`) |
| `lsp` | Info | Language servers (`config/lsp.ts`) |
| `instructions` | string[] (concatenated across files) | Extra instruction files/patterns (AGENTS.md-style context) |
| `layout` | — | @deprecated (always stretch) |
| `permission` | PermissionConfig | Rules (Section 5) |
| `tools` | Record<toolID, bool> | Shorthand that maps to permission allow/deny (write/edit/patch/multiedit → `edit`) |
| `tool` | `{ invocation_style?: json\|shell, invocation_style_by_tool?: Record }` | Tool invocation style |
| `enterprise` | `{ url }` | Enterprise/console URL |
| `compaction` | `{ auto?=true, prune?=true, tail_turns?=2, preserve_recent_tokens?, reserved?, max_context? }` | Context compaction. `max_context` accepts int, `"300K"`/`"1M"`/`"50%"`, or map keyed by `"prov/model*"` wildcard; only lowers trigger, clamped to real window |
| `checkpoint` | rich object | Checkpoint/memory subsystem: `thresholds` (default `["40%","60%","80%"]`), `reserved` (20k), `fork`, `push_caps.*` per-section token caps (tasks_ledger 2000, focus_task 4000, actor_ledger 500, memory_titles 500, global 6000, checkpoint 11000, memory 10000, notes 6000, design_decisions 3000, open_notes 800, recent_user 16000, recent_user_per_msg 2000), `task_archive_days` (7), `memory_reconcile_on_search`, `memory_search_score_floor` (0.15) |
| `memory` | `{ disable_write?, cc_index? }` | Stop writing new memory (read stays available); `cc_index: true` indexes Claude Code memory under scope `cc` |
| `history` | Info | Trajectory FTS index (`config/history.ts`) |
| `dream` | `{ auto?=false, interval_days?=7 }` | Auto memory-consolidation runs |
| `distill` | `{ auto?=false, interval_days?=30 }` | Auto workflow packaging runs |
| `voice` | `{ asr_model?="xiaomi/mimo-v2.5-asr", control_model?="xiaomi/mimo-v2.5" }` | Voice input models |
| `experimental` | object | `disable_paste_summary`, `batch_tool`, `openTelemetry`, `primary_tools[]`, `continue_loop_on_deny`, `try_best{edit_window=12,edit_similarity=0.8,edit_matches=2,action_streak=4}`, `mcp_timeout`, `predict_next_prompt=true`, `maxMode{candidates=5}` (parallel best-of-N reasoning w/ judge) |
| `workflow` | `{ maxConcurrentAgents=min(16,2×cores), maxDepth=8, maxLifecycleAgents=1000, scriptDeadlineMs=12h }` | Dynamic workflow runtime limits |

### Agent schema (`config/agent.ts`)

Per agent: `model` (literal or group/tier name), `variant`, `temperature`, `top_p`, `prompt`, `description`, `tools` (@deprecated→permission), `disable`, `mode` (`subagent`|`primary`|`all`), `hidden`, `options`, `color` (hex or theme name), `steps` (max iterations; `maxSteps` deprecated), `tool_allowlist[]`, `permission`, plus arbitrary extra keys promoted into `options`. Markdown agent files also load from `<config-dir>/agent/*.md` and legacy `mode/*.md` (`ConfigAgent.load/loadMode`).

---

## 2. Environment variables

Authoritative table: [mimo.xiaomi.com/mimocode/env-vars](https://mimo.xiaomi.com/mimocode/env-vars). Booleans: `true`/`1` on, `false`/`0` off. Env vars are **not** a general fallback for config fields — inject config via `MIMOCODE_CONFIG`/`MIMOCODE_CONFIG_CONTENT`. Definitions in source: `flag/flag.ts`, `flag/*`.

### Resource location
| Var | Effect |
|---|---|
| `MIMOCODE_HOME` | Profile root holding `config/ data/ state/ cache/`; overrides all XDG dirs; must be absolute |
| `MIMOCODE_CONFIG` | Custom config file path |
| `MIMOCODE_CONFIG_DIR` | Extra config directory (structured like `.mimocode/`) |
| `MIMOCODE_CONFIG_CONTENT` | Inline JSON config content (merged as *local* scope) |
| `MIMOCODE_TUI_CONFIG` | Custom tui.json path |
| `MIMOCODE_PERMISSION` | Inline JSON permission rules (merged last) |
| `MIMOCODE_DB` | Override database path |
| `MIMOCODE_MODELS_URL` | Model-catalog base URL (default `https://models.dev`, fetched at `/api.json` — `provider/models.ts` `url()`) |
| `MIMOCODE_MODELS_PATH` | Local model-manifest path (skips fetch entirely) |
| `MIMOCODE_GIT_BASH_PATH` | Git Bash path (Windows) |

### Runtime switches
`MIMOCODE_PURE` (disables all plugins), `MIMOCODE_AUTO_SHARE`, `MIMOCODE_DISABLE_SHARE`, `MIMOCODE_DISABLE_AUTOUPDATE` (**default true**), `MIMOCODE_ALWAYS_NOTIFY_UPDATE`, `MIMOCODE_DISABLE_AUTOCOMPACT`, `MIMOCODE_DISABLE_PRUNE`, `MIMOCODE_DISABLE_TERMINAL_TITLE`, `MIMOCODE_DISABLE_MOUSE`, `MIMOCODE_DISABLE_DEFAULT_PLUGINS`, `MIMOCODE_DISABLE_LSP_DOWNLOAD`, `MIMOCODE_DISABLE_MODELS_FETCH`, `MIMOCODE_DISABLE_EMBEDDED_WEB_UI`, `MIMOCODE_ENABLE_ANALYSIS` (telemetry; **default true**, set false to opt out), `MIMOCODE_ENABLE_EXPERIMENTAL_MODELS`, `MIMOCODE_ENABLE_EXA` (Exa web search tool), `MIMOCODE_ENABLE_QUESTION_TOOL`, `MIMOCODE_DISABLE_PROJECT_CONFIG` (ignore in-project `.mimocode/`), `MIMOCODE_DISABLE_GIT`, `MIMOCODE_DISABLE_CHANNEL_DB` (**default true**: one shared `mimocode.db`; set false for per-channel isolation), `MIMOCODE_DANGEROUSLY_SKIP_PERMISSIONS`, `MIMOCODE_AUTO_APPROVE_DELETE` (skip destructive-bash second confirmation), `MIMOCODE_CODEX_MODE` (GPT system prompt + Codex toolset for all models), `MIMOCODE_DISABLE_CHECKPOINT`, `MIMOCODE_DISABLE_CRON` (cron kill switch), `MIMOCODE_DISABLE_LOG_ROTATION`, `MIMOCODE_DISABLE_COMPOSE_SKILLS` / `_BUILTIN_SKILLS` / `_OFFICIAL_SKILLS` (docx/pdf/pptx/xlsx/html-to-video bundle).

### Claude Code compatibility ("pure-mimo mode")
- `MIMOCODE_MIMO_ONLY` — **default true** in current builds ([env-vars doc](https://mimo.xiaomi.com/mimocode/env-vars)); when truthy: no `.claude/` inheritance, no provider API keys read from env, default model falls back to free `mimo-auto` (`flag/flag.ts` comment).
- `MIMOCODE_DISABLE_PROVIDER_ENV` (implicit in mimo-only), `MIMOCODE_DISABLE_CLAUDE_CODE` (prompt+skills, not MCP), `MIMOCODE_DISABLE_CLAUDE_CODE_PROMPT`, `MIMOCODE_DISABLE_CLAUDE_CODE_SKILLS`, `MIMOCODE_DISABLE_CLAUDE_CODE_MCP`, `MIMOCODE_DISABLE_EXTERNAL_SKILLS`, `MIMOCODE_DISABLE_CODEX_SKILLS`, `MIMOCODE_DISABLE_OPENCODE_SKILLS`, `MIMOCODE_DISABLE_CLAUDE_CODE_COMMANDS` (source `paths.ts`).

### Auth & endpoints
| Var | Effect |
|---|---|
| `MIMOCODE_SERVER_PASSWORD` | Basic auth password for `serve`/`web`; auto-generates one for implicit listeners if unset (`flag.ts` `generateServerPassword`) |
| `MIMOCODE_SERVER_USERNAME` | Basic-auth username (default `mimocode`) |
| `MIMOCODE_AUTH_CONTENT` | Inline JSON replacing whole `auth.json` (CI injection; scrubbed from child-process env — `util/credential-env.ts`) |
| `MIMOCODE_CONSOLE_TOKEN` | Console auth token (set automatically when org account active) |
| `MIMOCODE_WORKSPACE_ID` | Workspace identifier |
| `MIMOCODE_CLIENT` | Client id used in USER_AGENT/tool registration (default `cli`) |

### Xiaomi/MiMo platform keys
- `XIAOMI_API_KEY` / `MIMO_API_KEY` / `ANTHROPIC_API_KEY` etc.: provider env keys are honored via each catalog provider's `env: [...]` list **only when `MIMOCODE_DISABLE_PROVIDER_ENV` is off** (i.e., mimo-only mode must be disabled) — `provider/provider.ts` env-loading block.
- `MIMO_PLATFORM_URL` — Xiaomi platform base for browser OAuth login; default `https://platform.xiaomimimo.com` (`plugin/mimo.ts`). Login returns an encrypted secret key + per-user `base_url` stored as auth metadata.
- `MIMO_GATEWAY_PROVIDERS` — internal: gateway error handling treats `xiaomi` and `mimo` as MiMo-router providers (`provider/error.ts`).
- Internal/diagnostics: `MIMO_PYTHON`, `MIMO_NODE`, `MIMO_SOFFICE`, `MIMO_QPDF`, `MIMO_FDS_*` (endpoint/bucket/AK/SK/prefix/base — Xiaomi FDS object storage used by official-skill document pipelines), `MIMO_GRAY`, `MIMO_ORANGE`, `MIMO_ONLY`, `MIMO_NODE_MODULES`.

### Experimental
Umbrella `MIMOCODE_EXPERIMENTAL=true` enables all of: `MIMOCODE_EXPERIMENTAL_ICON_DISCOVERY`, `_DISABLE_COPY_ON_SELECT` (true on Windows), `_BASH_DEFAULT_TIMEOUT_MS` (number), `_OUTPUT_TOKEN_MAX`, `_FILEWATCHER` / `_DISABLE_FILEWATCHER`, `_OXFMT`, `_LSP_TOOL`, `_LSP_TY`, `_MARKDOWN` (**default true**), `_HTTPAPI`, `_WORKSPACES`, plus separately gated `MIMOCODE_EXPERIMENTAL_ORCHESTRATOR`, `_WORKFLOW_TOOL`, `_MCP_TOOL_SEARCH`, `MIMOCODE_ENABLE_EXEC_TOOL`, `MIMOCODE_ENABLE_TRY_BEST_HANDOFF`, `MIMOCODE_ENABLE_DYNAMIC_SYSTEM_PROMPT`, `MIMOCODE_ENABLE_FUZZY_EDIT`, `MIMOCODE_EXPERIMENTAL_CRON` (**default true**), `MIMOCODE_LOOP_KEEPALIVE_BUDGET` (1) / `_DELAY_S` (1200), token-efficiency pipeline (`_TOKEN_EFFICIENCY`, `_..._MAX_LINE_CHARS`=500, `_LINE_HEAD_KEEP`=160, `_NEVER_WORSE_MARGIN`=0, `_HEURISTIC`).

### Diagnostics / tuning
`MIMOCODE_SHOW_TTFD`, `MIMOCODE_AUTO_HEAP_SNAPSHOT`, `MIMOCODE_SKIP_MIGRATIONS`, `MIMOCODE_STRICT_CONFIG_DEPS`, `MIMOCODE_FAST_BOOT`, `MIMOCODE_PLUGIN_META_FILE`, `MIMOCODE_FAKE_VCS`, `MIMOCODE_OUTPUT_LENGTH_CONTINUATION_LIMIT` (3), `MIMOCODE_INVALID_OUTPUT_CONTINUATION_LIMIT` (2), `MIMOCODE_TEXT_TOOL_CALL_RETRY_LIMIT` (2), repetition detection `MIMOCODE_TEXT_NGRAM_N` (4)/`_TEXT_REPEAT_THRESHOLD` (20)/`_TEXT_WINDOW_TOKENS` (500), image caps `MIMOCODE_MAX_PROMPT_IMAGES` / `MIMOCODE_MAX_PROMPT_IMAGE_SIZE` (~4.5 MB default), `MIMOCODE_FORCE_ANTHROPIC_REASONING_CONTENT`. OTel: standard `OTEL_EXPORTER_OTLP_ENDPOINT` / `OTEL_EXPORTER_OTLP_HEADERS`. Legacy `OPENCODE_*` names still exist internally (e.g. `OPENCODE_SERVER_PASSWORD` aliases appear in code paths) but `MIMOCODE_*` is the documented surface.

### Base URL
No single global base-URL env var. Base URLs are configured per provider via `provider.<id>.options.baseURL` in config, or come from the MiMo platform login (stored `metadata.base_url` in `auth.json`). The model-catalog endpoint is `MIMOCODE_MODELS_URL` (default `https://models.dev/api.json`).

---

## 3. Providers

### MiMo models API (in-house)
- Two in-house providers: `xiaomi` (subscription, api `https://api.xiaomimimo.com/v1` per models.dev catalog) and free-tier `mimo`. Both are treated as MiMo-router providers for error mapping (`provider/error.ts` `MIMO_GATEWAY_PROVIDERS`).
- Auth via browser OAuth login (X25519 key exchange against `MIMO_PLATFORM_URL`/`platform.xiaomimimo.com/authorize`, callback delivers encrypted `sk` + per-account `base_url` saved as auth metadata; `plugin/mimo.ts` `MimoAuthPlugin`). The plugin registers `xiaomi` even before login and sends header `X-Mimo-Source: mimocode-cli`.
- `xiaomi` uses a custom loader (`provider/provider.ts`): models supporting the MiMo Responses API go through `sdk.responses(modelID)`; others use plain chat completion (`usesMimoResponsesApi` in `tool/gpt.ts`).
- Default-model resolution order (`defaultModel()`): config `model` → recently-used model (`state/model.json`) → `mimo/mimo-auto` (free routing alias; vision-capable) → first sorted provider model.

### OpenAI-compatible endpoints — fully supported
From [README §Custom OpenAI-Compatible Endpoints](https://github.com/XiaomiMiMo/MiMo-Code#custom-openai-compatible-endpoints):

```jsonc
{
  "$schema": "https://mimo.xiaomi.com/mimocode/config.json",
  "model": "custom/MODEL_NAME",
  "provider": {
    "custom": {
      "name": "Custom",
      "npm": "@ai-sdk/openai-compatible",
      "only_configured_models": true,
      "models": { "MODEL_NAME": { "name": "MODEL_NAME" } },
      "options": { "baseURL": "BASE_URL", "apiKey": "API_KEY" }
    }
  }
}
```
Model IDs containing `/` work (only first `/` splits provider/model). Non-OpenAI wire protocols need their specific adapter npm package.

### Anthropic-compatible?
Yes, two ways:
1. Bundled `@ai-sdk/anthropic` SDK — define a provider with `"npm": "@ai-sdk/anthropic"` + `options.baseURL`/`apiKey`; honors `ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` env (when provider-env loading enabled). An `AnthropicProxyPlugin` (`plugin/mimo.ts`) strips the `anthropic-beta` header and early-closes SSE after `message_stop` for proxy endpoints that don't fully implement streaming.
2. Reasoning-content round-trip support: `MIMOCODE_FORCE_ANTHROPIC_REASONING_CONTENT` gives unsigned historical thinking blocks placeholder signatures; `cachePromptTTL: "5m"|"1h"` per model controls prompt-cache breakpoints on Anthropic/OpenRouter (`config/provider.ts`).

### Provider config schema (`config/provider.ts` `Info`)
`api`, `name`, `env` (env-var name list that activates it), `id`, `npm` (SDK package), `whitelist`/`blacklist` (model filters), `options` (`apiKey`, `baseURL`, `enterpriseUrl`, `setCacheKey`, `timeout` ms|false (default 300000), `headerTimeout`, `chunkTimeout` (default 480000, tuned for mimo-v2.5-pro cold TTFT), + arbitrary SDK options), `retry`, `models` (per-model overrides), `only_configured_models` (hide catalog models not listed). Per-model (`Model`): `id,name,family,release_date,attachment,reasoning,temperature,tool_call,voice_design,voice_clone,interleaved,cost{input,output,cache_read,cache_write,context_over_200k},limit{context,input,output},modalities{input[],output[]},experimental,status,alpha/beta/deprecated,cachePromptTTL,provider{npm,api},options,headers,variants{...disabled}`.

Provider activation precedence: env keys → `auth.json` entries → plugin loaders/custom loaders → config `provider` block re-applied last. Catalog comes from models.dev (`MIMOCODE_MODELS_URL`/`_PATH`, cached, `mimo models --refresh` refreshes).

---

## 4. MULTI-INSTANCE WRAPPERS

MiMo Code has no built-in named-profile switcher like some CLIs; isolation is achieved through **`MIMOCODE_HOME` profile roots** + **inline config injection**. Everything (config, auth.json, DB, logs) lives under the profile root, so separate roots = fully independent instances (source: `shared/src/global.ts` — "Single profile root ... overrides all XDG base dirs"; env-vars doc scenario "Switch the profile root for isolated testing").

Switching levers:
- **Profile root**: `MIMOCODE_HOME=/abs/path mimo …` — isolates config/, auth, DB, cache, state.
- **Inline config**: `MIMOCODE_CONFIG_CONTENT='{"model":"xiaomi/mimo-v2.5-pro",…}'` merged as local-scope config without touching disk ([env-vars doc](https://mimo.xiaomi.com/mimocode/env-vars)).
- **Inline credentials**: `MIMOCODE_AUTH_CONTENT='{"xiaomi":{"type":"api","key":"sk-..."}}'` replaces auth.json wholesale (kept out of spawned children's env — `util/credential-env.ts`).
- **Extra config layer**: `MIMOCODE_CONFIG=/path/to/file.jsonc` or whole alternate dir `MIMOCODE_CONFIG_DIR`.
- **Server mode**: run one instance as `mimo serve --port 4096` and attach others with `mimo attach http://localhost:4096` or `mimo run --attach http://localhost:4096` ([cli-options](https://mimo.xiaomi.com/mimocode/cli-options)).

Wrapper script example:

```bash
#!/usr/bin/env bash
# mimo-profile <profile> [args...] — run MiMo Code under an isolated profile
set -euo pipefail
PROFILE="$1"; shift
ROOT="$HOME/.mimo-profiles/$PROFILE"
mkdir -p "$ROOT"
exec env \
  MIMOCODE_HOME="$ROOT" \                      # isolated config/data/state/cache + auth.json
  MIMOCODE_CONFIG_CONTENT="${MIMO_PROFILE_CONFIG:-}" \
  mimo "$@"
# Usage:
#   mimo-profile work .            # project config picked up normally
#   MIMO_PROFILE_CONFIG='{"model":"openai/gpt-5"}' mimo-profile gpt .
```

CI/headless variant straight from the docs:

```sh
MIMOCODE_CONFIG_CONTENT='{"model":"xiaomi/mimo-v2.5-pro","share":"disabled"}' \
MIMOCODE_AUTH_CONTENT='{"anthropic":{"apiKey":"sk-..."}}' \
  mimo run "Generate release notes"
```

Parallel instances: each process gets its own random server port by default (`mimo run --port` to pin); `MIMOCODE_DB` can further split databases within one root. `MIMOCODE_WORKSPACE_ID` tags instances for workspace-aware control-plane features.

---

## 5. Hooks / Skills / MCP / git-worktree config

### Hooks (via plugins — no shell-hook config file)
There is no Claude-Code-style `hooks` config key; lifecycle hooks are TypeScript plugin hooks implementing the `Hooks` interface (`packages/plugin/src/index.ts`): `event`, `config`, `tool` (custom tools), `auth`, `provider`, `chat.message`, `chat.params` (temperature/topP/topK/maxOutputTokens/options), `chat.headers`, `permission.ask`, `command.execute.before`, `shell.env`, `tool.execute.before` (can cancel), `tool.execute.after`, `tool.definition`, `actor.preStop`/`actor.postStop`, `session.pre`, `experimental.chat.messages.transform`, `experimental.chat.system.transform`, `experimental.session.compacting`, `experimental.compaction.autocontinue`, `experimental.text.complete`. Reference implementations: `plugin/mimo.ts` (auth + chat.headers). Plugins registered via config `"plugin": ["npm-pkg", "https://…", "./local/plugin.ts"]`, auto-discovered from `<config-dir>/plugin(s)/`, defaults disabled with `MIMOCODE_DISABLE_DEFAULT_PLUGINS`, all plugins off with `MIMOCODE_PURE=true`. Plugin deps auto-installed into each config dir (`@mimo-ai/plugin`).

### Skills
- Locations scanned (`skill/index.ts`): builtin skills extracted from binary (`MIMOCODE_DISABLE_BUILTIN_SKILLS`, `MIMOCODE_DISABLE_OFFICIAL_SKILLS`), compose-internal skills (`compose:*`, `MIMOCODE_DISABLE_COMPOSE_SKILLS`), project/global `{skill,skills}/**/SKILL.md` under `.mimocode/` dirs and `~/.config/mimocode/`, plus external sources: Claude Code `~/.claude` + project `.claude/skills`, Codex skills, OpenCode skills (each toggleable via the `MIMOCODE_DISABLE_*_SKILLS` family above).
- Config: `"skills": { "paths": [...extra folders], "urls": ["https://example.com/.well-known/skills/"] }` (`config/skills.ts`); remote discovery cached under `~/.cache/mimocode/skills`. SKILL.md frontmatter kebab-case, compatible with Claude Code / agentskills.io. User skills override builtins with the same name.

### MCP servers (`config/mcp.ts`)
```jsonc
"mcp": {
  "local-tools": {                       // stdio
    "type": "local",
    "command": ["uvx", "mcp-server-fetch"],
    "environment": { "KEY": "value" },   // env for the subprocess
    "enabled": true,
    "timeout": 5000,                     // ms, default 5000
    "sampling": "ask"                    // deny|ask(default)|allow for sampling/createMessage
  },
  "remote-api": {
    "type": "remote",
    "url": "https://example.com/mcp",
    "headers": { "Authorization": "Bearer ..." },
    "oauth": { "clientId": "...", "clientSecret": "...", "scope": "...", "redirectUri": "http://127.0.0.1:19876/mcp/oauth/callback" }, // or false to disable auto-detection
    "enabled": true,
    "timeout": 5000,
    "sampling": "allow"
  }
}
```
Legacy disable form: `{ "name": { "enabled": false } }`. Claude Code interop: `mcpServers` from `~/.claude.json` and `./.claude.json` are converted automatically (stdio/`http`/`streamable-http` types; `sse` unsupported) unless native config already defines the same name or `MIMOCODE_DISABLE_CLAUDE_CODE_MCP` is set. Related knobs: `experimental.mcp_timeout`, `MIMOCODE_EXPERIMENTAL_MCP_TOOL_SEARCH`, OAuth tokens in `~/.local/share/mimocode/mcp-auth.json`.

### Permissions (`config/permission.ts`)
Rule map keyed by permission name with glob patterns allowed as sub-keys; actions `ask` (default) | `allow` | `deny`; later keys win (`findLast` on original key order). Names: `read, edit, glob, grep, list, bash, task, actor, external_directory, question, webfetch, websearch, codesearch, lsp, doom_loop, skill`, arbitrary tool IDs, and `"*"` catch-all. A bare string `"deny"` expands to `{"*":"deny"}`. Example from README: `"permission": { "external_directory": { "/tmp/**": "allow" } }`. Overrides: `tools` shorthand maps to allow/deny; `MIMOCODE_PERMISSION` inline JSON; `--dangerously-skip-permissions`/`MIMOCODE_DANGEROUSLY_SKIP_PERMISSIONS` injects allow-all underneath (explicit denies still win; destructive bash delete still forces its own prompt unless `MIMOCODE_AUTO_APPROVE_DELETE`).

### git-worktree
Worktrees are handled natively rather than configured:
- Instance resolution walks config up to the **worktree root** (`ctx.worktree` threaded through `project/instance.ts`, `config/paths.ts`), so linked worktrees share repo-level `.mimocode/` while allowing worktree-local overrides.
- Compose mode auto-parallelizes independent tasks into **isolated git worktrees** (README §Compose Mode; `control-plane/adaptors/worktree.ts`); `compose.docs_absolute` anchors the docs dir to the active worktree root (`config/compose.ts`).
- `MIMOCODE_DISABLE_GIT` skips all git/worktree detection entirely; `MIMOCODE_GIT_BASH_PATH` supplies Git Bash on Windows; `MIMOCODE_FAKE_VCS` mocks VCS for tests. Snapshot/undo (`snapshot` config) is git-backed.
