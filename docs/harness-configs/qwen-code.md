# Qwen Code — Configurable Options

> Qwen Code (`@qwen-code/qwen-code`, repo [QwenLM/qwen-code](https://github.com/QwenLM/qwen-code)) is Alibaba's open-source terminal coding agent. Node ≥ 22, installed via npm/Homebrew/curl script ([README](https://github.com/QwenLM/qwen-code)). This doc maps its configuration surface: settings files, env vars, auth, multi-instance switching, MCP, and its Gemini CLI fork lineage.
>
> Docs live at [qwenlm.github.io/qwen-code-docs](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/settings/) (the old in-repo `docs/cli/*.md` paths are gone; pages last updated 2026-08-24). Note the project iterates fast — treat exact defaults as version-sensitive.

---

## 1. Settings files, precedence & QWEN.md context hierarchy

### 1.1 Configuration layers (highest wins)

Precedence order — lower numbers are overridden by higher ones ([Settings](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/settings/#configuration-layers)):

| Level | Source |
|---|---|
| 1 | Hardcoded defaults |
| 2 | System defaults file (`system-defaults.json`) |
| 3 | **User settings** — `~/.qwen/settings.json` |
| 4 | **Project settings** — `.qwen/settings.json` in project root (overrides user) |
| 5 | System settings file (overrides user *and* project; enterprise admin knob) |
| 6 | Environment variables (incl. auto-loaded `.env` files) |
| 7 | Command-line arguments |

File locations ([Settings §files](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/settings/#settings-files)):

| File | Path |
|---|---|
| System defaults | Linux `/etc/qwen-code/system-defaults.json` · macOS `/Library/Application Support/QwenCode/system-defaults.json` · Windows `C:\ProgramData\qwen-code\system-defaults.json`; relocatable via `QWEN_CODE_SYSTEM_DEFAULTS_PATH` |
| User | `~/.qwen/settings.json` (dir renameable via `QWEN_HOME`) |
| Project | `<project>/.qwen/settings.json` |
| System | `/etc/qwen-code/settings.json` (+ macOS/Windows equivalents); relocatable via `QWEN_CODE_SYSTEM_SETTINGS_PATH` |

String values anywhere in `settings.json` support `$VAR_NAME` / `${VAR_NAME}` interpolation, resolved at load time (e.g. `"apiKey": "$MY_API_TOKEN"`).

The project `.qwen/` directory also holds sandbox profiles (`.qwen/sandbox.Dockerfile`, `.qwen/sandbox-macos-custom.sb`) and Agent Skills under `.qwen/skills/<name>/SKILL.md`.

Legacy flat keys are auto-migrated into nested namespaced ones (with backup); e.g. `disableAutoUpdate`+`disableUpdateNag` → `general.enableAutoUpdate` (booleans inverted; if *either* old flag was `true`, auto-update becomes `false`), `disableLoadingPhrases` → `ui.accessibility.enableLoadingPhrases`, `disableFuzzySearch` → `context.fileFiltering.enableFuzzySearch`, `disableCacheControl` → `model.generationConfig.enableCacheControl`.

### 1.2 `settings.json` schema (key categories)

Full reference: [Settings §available-settings](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/settings/#available-settings-in-settingsjson). Condensed map:

| Category | Notable keys (default) |
|---|---|
| `general` | `preferredEditor`, `vimMode` (false), `enableAutoUpdate` (true), `language`/`outputLanguage` ("auto"), `terminalBell` (true), `chatRecording` (true), `cleanupPeriodDays` (30), voice dictation (`voice.*`) |
| `output` | `format`: `"text"` \| `"json"` \| `"stream-json"`, `showTimestamps` (false) |
| `model` | `name` — default model id · `reasoningEffort` (`low…max`) · `sessionTokenLimit` (-1) · `maxSessionTurns` (-1) · `maxWallTimeSeconds` (-1; exit 55) · `maxToolCalls` / `maxToolCallsPerTurn` (100 adaptive) · `generationConfig{timeout, maxRetries, samplingParams{temperature,top_p,max_tokens}, contextWindowSize, modalities, customHeaders, extra_body, enableCacheControl, splitToolMedia, toolResultContentFormat}` — `extra_body` is OpenAI-compatible-only |
| Specialized models | top-level `fastModel`, `advisorModel`, `visionModel`, `compactionModel`, `imageModel`, `voiceModel`, `modelFallbacks` (comma-sep, ≤3, tried on 429/503/529), `modelPricing` |
| `context` | `fileName` (context filename(s), e.g. `["QWEN.md"]`), `autoCompactThreshold` (0.85), `includeDirectories[]`, `loadFromIncludeDirectories` (false), `fileFiltering.{respectGitIgnore,respectQwenIgnore,enableFuzzySearch,…}` |
| `tools` | `approvalMode`: `plan`\|`default`\|`auto-edit`\|`auto`\|`yolo`, `sandbox`, `sandboxImage`, `shell.defaultTimeoutMs`, `truncateToolOutputThreshold` (25000), `useRipgrep` (true), `toolSearch.enabled` (true); legacy `core`/`exclude`/`allowed` auto-migrate to `permissions` |
| `permissions` | `allow` / `ask` / `deny` rule arrays; priority deny > ask > allow; syntax `"Bash(git *)"` , `"Read(./secrets/**)"`, `"mcp__puppeteer"`; path prefixes `//` absolute, `~/` home, `/` project root |
| `mcp` | `allowed[]` / `excluded[]` (glob-capable server filters), `toolIdleTimeoutMs` (300000; env `QWEN_CODE_MCP_TOOL_IDLE_TIMEOUT_MS`) |
| `mcpServers` | server connection map — see §5 |
| `security.auth` | `selectedType` (startup protocol: `openai`/`anthropic`/`gemini`/`vertex-ai`/`qwen-oauth`), `enforcedType`; deprecated `apiKey`/`baseUrl` → migrate to `modelProviders` |
| `privacy` | `usageStatisticsEnabled` (true; opt-out also via `QWEN_USAGE_STATISTICS_ENABLED=false`) |
| `modelProviders` | per-protocol model catalogs (`openai`/`anthropic`/`gemini`) — see §3/§5 |
| Other | `ui.*` (theme, statusLine, accessibility), `ide.enabled`, `memory.*` (auto-memory/auto-skill), `agents.*`, `slashCommands.disabled[]`, `skills.*`, `telemetry.*` (OTLP endpoint/outfile), `serve.*`, `advanced.{excludedEnvVars,bugCommand}`, `experimental.*`, `proxy`, `plansDirectory` |

Example skeleton (from official example): `{ "proxy": "...", "general": {...}, "ui": {"theme": "GitHub"}, "tools": {"approvalMode": "yolo"}, "mcpServers": {...}, "model": {"name": "qwen3-coder-plus", "maxSessionTurns": 10}, "context": {"fileName": ["CONTEXT.md","QWEN.md"], "loadFromIncludeDirectories": true} }`.

### 1.3 QWEN.md hierarchical context

Context files (default name `QWEN.md`, renameable via `context.fileName`) are loaded hierarchically and concatenated into the system prompt ([Settings §context-files](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/settings/#context-files-hierarchical-instructional-context)):

1. **Global**: `~/.qwen/QWEN.md` — instructions for all projects.
2. **Project root & ancestors**: searched upward from cwd to the project root (`.git` marker) or home dir — monorepo/subdirectory layering works.
   - More-specific (lower) files override/supplement more-general ones; exact concatenation order is inspectable in the `/memory` dialog; footer shows loaded-file count.
   - Modularize with `@path/to/file.md` imports inside any context file.
   - `/memory refresh` re-scans; `context.loadFromIncludeDirectories: true` extends loading to `context.includeDirectories`.
   - A welcome-back summary lives at `.qwen/PROJECT_SUMMARY.md` (`ui.enableWelcomeBack`).

---

## 2. Environment variables

### 2.1 Provider / auth variables

From the [protocol table](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/auth/#supported-protocols) and env table:

| Variable | Role |
|---|---|
| `OPENAI_API_KEY` | Default API-key env for the **OpenAI-compatible** protocol (fallback when a `modelProviders` entry omits `envKey`). Works for OpenAI, Azure OpenAI, OpenRouter, Requesty, ModelScope, DashScope, any compatible endpoint |
| `OPENAI_BASE_URL` | Endpoint override for that protocol — this is the main custom-provider lever (DashScope, Coding Plan, proxies, local vLLM/Ollama) |
| `OPENAI_MODEL` | Model id when using env-driven setup (**alias: `QWEN_MODEL`**) |
| `ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` / `ANTHROPIC_MODEL` | Anthropic protocol |
| `GEMINI_API_KEY` / `GEMINI_MODEL` | Google GenAI protocol (current naming) |
| `GOOGLE_GEMINI_BASE_URL` | Gemini-CLI-era base-URL override for the Gemini/GenAI generator, inherited from the fork parent; current docs steer you to `GEMINI_*` keys instead |
| `GOOGLE_API_KEY` or `GOOGLE_CLOUD_PROJECT` (+ optional `GOOGLE_CLOUD_LOCATION`), `GOOGLE_MODEL` | Vertex AI (needs explicit `--auth-type vertex-ai` or `security.auth.selectedType`) |
| `DASHSCOPE_API_KEY` | Alibaba Cloud DashScope/ModelStudio standard API key (used as `envKey` for `https://dashscope.aliyuncs.com/compatible-mode/v1`) |
| `BAILIAN_CODING_PLAN_API_KEY` | Coding Plan subscription key (`sk-sp-…`) with the dedicated `coding[.-intl.]dashscope` endpoint |
| `OPENROUTER_API_KEY`, `REQUESTY_API_KEY` | With respective base URLs (see §3) |

### 2.2 `QWEN_*` operational variables (selection)

([Env table](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/settings/#environment-variables-table))

| Variable | Purpose |
|---|---|
| `QWEN_HOME` | Relocates the whole global config/state dir (default `~/.qwen`) — the key to per-instance isolation |
| `QWEN_RUNTIME_DIR` | Separate runtime output (conversations/logs/todos) from persistent config |
| `QWEN_CODE_SYSTEM_DEFAULTS_PATH` / `QWEN_CODE_SYSTEM_SETTINGS_PATH` | Relocate system-level settings files |
| `QWEN_SANDBOX` / `QWEN_SANDBOX_IMAGE` | Sandbox enablement (`true\|false\|docker\|podman\|cmd`) and image override |
| `QWEN_CODE_API_TIMEOUT_MS` | Per-request timeout (mirrors `generationConfig.timeout`) |
| `QWEN_STREAM_IDLE_TIMEOUT_MS` / `QWEN_STREAM_MAX_LIFETIME_MS` | Stream inactivity guard (default 240 s) / total stream lifetime cap (default 900 s) |
| `QWEN_CODE_MAX_OUTPUT_TOKENS` | Fixed output-token limit (disables auto-escalation; overridden by `samplingParams.max_tokens`) |
| `QWEN_CODE_UNATTENDED_RETRY=1` | Retry 429/529 indefinitely with backoff (CI mode) |
| `QWEN_DISABLED_SLASH_COMMANDS` | Unioned with `slashCommands.disabled` |
| `QWEN_USAGE_STATISTICS_ENABLED`, `QWEN_TELEMETRY_*` | Privacy/telemetry overrides |
| `QWEN_COMPACT_MAX_RECENT_FILES/_IMAGES`, `QWEN_COMPACT_SCREENSHOT_TRIGGER/_THRESHOLD` | Compaction behavior |
| `CODE_ASSIST_ENDPOINT` | Code-assist server endpoint (dev/testing; Gemini-CLI inheritance) |

Loader-affecting vars (`NODE_OPTIONS`, `LD_PRELOAD`, `BASH_ENV`, …) are always rejected from `.env` files and `settings.json.env` as a security measure.

### 2.3 `.env` loading & secret priority

Auto-loads the **first** found `.env` (no merging across files), walking up from cwd: `.qwen/.env` → `.env` → `~/.qwen/.env` → `~/.env` ([Auth §step-2](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/auth/#step-2-set-environment-variables)). Overall priority for API keys: **CLI flags (`--openai-api-key`) > shell environment > `.env` file > `settings.json` → `env` block** (lowest-priority plaintext fallback).

---

## 3. Authentication

First-run `/auth` menu has three branches ([Authentication](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/auth/)): **Alibaba ModelStudio** (Coding Plan / Token Plan / Standard API Key), **Third-party Providers** (DeepSeek, Grok, MiniMax, Z.AI, Kimi, ModelScope, OpenRouter, Requesty, …), **Custom Provider** (any OpenAI-/Anthropic-/Gemini-compatible endpoint).

### 3.1 Qwen OAuth device flow — discontinued

Historically: launching `qwen` opened a browser login against a `qwen.ai` account; tokens cached locally with automatic refresh, no key management ([Auth §option-1](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/auth/#option-1-qwen-oauth-discontinued)). **The free tier was discontinued 2026-04-15**; the entry was removed from the `/auth` dialog (remains a hard-coded `qwen-oauth` type). Browser flows also don't work headless (CI/containers) — use API keys there.

### 3.2 Alibaba Cloud Coding Plan / ModelStudio API key

- **Interactive**: `/auth` → Alibaba ModelStudio → Coding Plan → region → paste `sk-sp-xxxx` key; then `/model` switches among included models (qwen3-coder-plus, qwen3-coder-next, glm-5, kimi-k2.5, MiniMax-M2.5, …).
- **Headless env setup** ([Auth §headless](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/auth/#headless-or-scripted-setup)):
  ```bash
  export BAILIAN_CODING_PLAN_API_KEY="sk-sp-xxxxxxxxx"
  export OPENAI_BASE_URL="https://coding.dashscope.aliyuncs.com/v1"      # China (Beijing)
  # international: https://coding-intl.dashscope.aliyuncs.com/v1
  export OPENAI_MODEL="qwen3-coder-plus"
  ```
- **Regional consoles/endpoints**: China = Aliyun ModelStudio Beijing (`bailian.console.aliyun.com`); International = `modelstudio.console.alibabacloud.com` / `bailian.console.alibabacloud.com`.
- **Standard (non-plan) DashScope API key** uses the OpenAI-compatible endpoint `https://dashscope.aliyuncs.com/compatible-mode/v1` with `DASHSCOPE_API_KEY` — one-file `~/.qwen/settings.json`:
  ```json
  {
    "modelProviders": { "openai": [{ "id": "qwen3-coder-plus",
      "baseUrl": "https://dashscope.aliyuncs.com/compatible-mode/v1",
      "envKey": "DASHSCOPE_API_KEY" }] },
    "env": { "DASHSCOPE_API_KEY": "sk-…" },
    "security": { "auth": { "selectedType": "openai" } },
    "model": { "name": "qwen3-coder-plus" }
  }
  ```

### 3.3 Custom OpenAI-compatible base URL

Two mechanisms: (a) pure-env — `OPENAI_BASE_URL` + `OPENAI_API_KEY` + `OPENAI_MODEL` (e.g. OpenRouter `https://openrouter.ai/api/v1`, Requesty `https://router.requesty.ai/v1`, local servers); (b) declarative — a `modelProviders.openai[]` entry with `id` (required), `baseUrl` (endpoint/proxy override), `envKey` (falls back to `OPENAI_API_KEY`), plus optional `generationConfig` (timeouts, `customHeaders`, `extra_body`, `samplingParams`). Set `security.auth.selectedType: "openai"` to skip interactive `/auth`. Recommended to define `modelProviders` in **user** scope to avoid project/user merge conflicts. The standalone `qwen auth` CLI command was **removed** — `/auth`, `/doctor`, or env/settings are the replacements.

---

## 4. Multi-instance wrappers (env-driven switching)

Because every provider knob is environment-driven (`OPENAI_BASE_URL`/`OPENAI_API_KEY`/`OPENAI_MODEL`, or `BAILIAN_CODING_PLAN_API_KEY` + coding endpoint) and all persistent state hangs off `QWEN_HOME`, parallel isolated instances reduce to per-invocation env scoping. Complementary levers: per-terminal model pinning with `qwen --model <id>` (selection persists per session store), `--openai-logging-dir` separation, and `QWEN_RUNTIME_DIR` if you share a home but want separate run history.

```bash
#!/usr/bin/env bash
# qwen-wrapper — run Qwen Code against one of two providers, fully env-isolated.
# Usage: qwen-wrapper dashscope "refactor this module"
set -euo pipefail

PROVIDER="${1:?provider: dashscope|plan}"; shift

case "$PROVIDER" in
  dashscope)   # Standard ModelStudio API key, pay-as-you-go
    export OPENAI_API_KEY="${DASHSCOPE_API_KEY:?export DASHSCOPE_API_KEY first}"
    export OPENAI_BASE_URL="https://dashscope.aliyuncs.com/compatible-mode/v1"
    export OPENAI_MODEL="qwen3-coder-plus"
    ;;
  plan)        # Alibaba Cloud Coding Plan subscription (international endpoint here)
    export BAILIAN_CODING_PLAN_API_KEY="${BAILIAN_CODING_PLAN_API_KEY:?export it first}"
    export OPENAI_BASE_URL="https://coding-intl.dashscope.aliyuncs.com/v1"
    export OPENAI_MODEL="qwen3-coder-next"
    ;;
  *) echo "unknown provider: $PROVIDER" >&2; exit 2 ;;
esac

exec qwen "$@"          # or: exec qwen -p "$*" for headless runs
```

Notes: secrets stay out of the wrapper (read from your shell profile / secret manager); `settings.json`'s `env:` block would be *lower* priority than these exports, so exports win cleanly; for hard isolation (separate credentials cache, settings, sessions) additionally set `QWEN_HOME=~/.qwen-$PROVIDER` before `exec`.

---

## 5. MCP configuration & model selection

### 5.1 `mcpServers` schema

Loaded from `mcpServers` in settings (user scope default; `qwen mcp add --scope project` writes project scope) ([MCP docs](https://qwenlm.github.io/qwen-code-docs/en/users/features/mcp/)). At least one of `command`/`url`/`httpUrl` required; precedence `httpUrl` > `url` > `command`:

| Property | Transport | Notes |
|---|---|---|
| `command`, `args`, `cwd`, `env` | `stdio` | Local process; `env` supports `$VAR` interpolation |
| `httpUrl`, `headers` | `http` | Streamable HTTP — preferred for remote |
| `url`, `headers` | `sse` | Legacy Server-Sent Events |
| `timeout` | any | Per-tool-call timeout (default 10 min) |
| `discoveryTimeoutMs` | any | Handshake cap (stdio 30 s, remote 5 s defaults) |
| `trust` | any | Skip tool-call confirmations in trusted workspaces |
| `description`, `includeTools[]`, `excludeTools[]` | any | Filtering (`excludeTools` wins) |

Discovery is progressive/background (`N/M MCP servers ready` pill); tools lazy-load via ToolSearch unless `tools.toolSearch.enabled: false`. Same-named tools get `serverAlias__toolName` prefixes. Server prompts become slash commands; resources injectable via `@server:uri`. Remote servers support OAuth flags (`--oauth-client-id/-secret/-scopes/…`, SSE/HTTP only). Manage via `qwen mcp add|remove [-t http|sse|stdio] [-s user|project] [-e K=V] [--header ...] [--include-tools/--exclude-tools ...]`; inspect with `/mcp`. Global filters: `mcp.allowed`/`mcp.excluded` (glob patterns).

```json
{ "mcpServers": {
    "pythonTools": { "command": "python", "args": ["-m","my_mcp_server","--port","8080"],
                     "cwd": "./mcp-servers/python",
                     "env": { "DATABASE_URL": "$DB_CONNECTION_STRING" }, "timeout": 15000 },
    "remote": { "httpUrl": "https://example.com/mcp",
                "headers": { "Authorization": "Bearer token" } } } }
```

### 5.2 Model selection

- Default model: `model.name` in settings, `--model`/`-m` CLI flag (per-session override), or runtime `/model` picker (persisted across sessions; grouped by protocol).
- Catalog: entries under `modelProviders.<protocol>[]`; each needs `id` (the wire model id), optional `name`, `envKey`, `baseUrl`, `generationConfig`.
- Coder models: `qwen3-coder-plus` and `qwen3-coder-next` (Coding Plan), plain `qwen3-coder` referenced in pricing examples; smaller `qwen3-coder-flash` suggested as `fastModel`. Plan roster also includes qwen3.5/3.6/3.7-plus, qwen3-max, GLM/Kimi/MiniMax.
- Aux models: `fastModel` (suggestions/speculation), `compactionModel` (auto-compaction), `visionModel` (image bridge for text-only mains), `advisorModel` (`/advisor` second opinion), `imageModel`, `voiceModel` (`qwen3-asr-*`), `modelFallbacks` (on 429/503/529).
- Reasoning depth via `model.reasoningEffort` (`low|medium|high|xhigh|max`), clamped per-provider.

---

## 6. Fork-lineage note (vs Gemini CLI)

Per the README's Acknowledgments, Qwen Code "was originally based on Google Gemini CLI v0.8.2"; **from v0.1 onward it stopped syncing upstream and developed independently** as a multi-protocol (OpenAI/Anthropic/Gemini/Qwen + local Ollama/vLLM), multi-platform agent framework ([README](https://github.com/QwenLM/qwen-code)). Visible inheritances vs differences:

- **Kept from Gemini CLI**: layered settings architecture (`settings.json` with system-defaults/user/project/system tiers), namespaced settings categories (`general`, `ui`, `model`, `context`, `tools`, `mcpServers`, `telemetry`, `privacy`, `security`, `advanced`) mirroring gemini-cli's schema; hierarchical memory-file concept (`QWEN.md` ≅ `GEMINI.md`, loaded globally → ancestors → cwd); `.env` discovery walk; sandbox/Seatbelt machinery; `CODE_ASSIST_ENDPOINT`; MCP server schema shape; `-p` headless mode; ACP/editor integration concepts.
- **Renamed/replaced**: config dir `~/.qwen` (vs `~/.gemini`), `QWEN_*` env prefix (vs `GEMINI_*`/`GOOGLE_GEMINI_*`), context file `QWEN.md`, system paths `/etc/qwen-code/…`. The Gemini protocol remains just *one* selectable backend (`gemini` in `modelProviders` / `GEMINI_API_KEY`), not the native one; legacy knobs like `GOOGLE_GEMINI_BASE_URL` persist mainly as compatibility surfaces.
- **Diverged**: Qwen/DashScope/Coding-Plan auth stack (`/auth` ModelStudio branch, `sk-sp-` keys, regional endpoints), `modelProviders` multi-protocol catalog with runtime switching, Claude-Code-parity features (subagents/agent teams, hooks, auto-memory/skills, plan mode, SDK, daemon `qwen serve`, IM channels), and removal of the standalone `qwen auth` command.

---

## Sources

- Settings/config reference: https://qwenlm.github.io/qwen-code-docs/en/users/configuration/settings/
- Authentication: https://qwenlm.github.io/qwen-code-docs/en/users/configuration/auth/
- MCP: https://qwenlm.github.io/qwen-code-docs/en/users/features/mcp/
- README (install, capabilities, fork acknowledgment): https://github.com/QwenLM/qwen-code
