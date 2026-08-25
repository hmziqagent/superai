# Kimi CLI / Kimi Code CLI — Complete Configuration Reference

Compiled 2026-08-25. **Important naming note:** Moonshot's original **Kimi CLI** (Python/uv, repo [`MoonshotAI/kimi-cli`](https://github.com/MoonshotAI/kimi-cli), config at `~/.kimi/config.toml`) is being wound down and has been succeeded by **Kimi Code CLI** (Node.js single binary, repo [`MoonshotAI/kimi-code`](https://github.com/MoonshotAI/kimi-code), config at `~/.kimi-code/`). The old README states: "Kimi CLI is evolving into Kimi Code CLI … This project will be gradually wound down" ([kimi-cli README](https://github.com/MoonshotAI/kimi-cli)). Migration: `kimi migrate` carries over `config.toml`, MCP servers, history, and (optionally) sessions from `~/.kimi/`; OAuth credentials and MCP authorizations are NOT migrated ([Migration guide](https://www.kimi.com/code/docs/en/kimi-code-cli/guides/migration.html)). Both are documented below; the current product (Kimi Code CLI) is primary.

---

## 1. Config files: location & schema

### Current (Kimi Code CLI)
- User-level data root: `~/.kimi-code`, relocatable with `KIMI_CODE_HOME` ([Config files](https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/config-files.html), [Env vars](https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/env-vars.html)).
  - `$KIMI_CODE_HOME/config.toml` — agent/runtime settings, TOML, snake_case keys; quote keys containing dots (`[models."gpt-4.1"]`).
  - `$KIMI_CODE_HOME/tui.toml` — terminal UI preferences; `/reload` reloads both, `/reload-tui` only tui.toml.
  - `$KIMI_CODE_HOME/mcp.json` — user-level MCP servers.
  - Project-local: `<project-root>/.kimi-code/local.toml` (`[workspace] additional_dir = [...]`, written by `/add-dir`; gitignore it) and `.kimi-code/mcp.json` (project MCP, overrides user-level same-named entries; gated by workspace trust prompt).
- There is **no YAML/JSON long-form config** — TOML only (verified on the config-files page); JSON is used only for `mcp.json`.

#### `config.toml` top-level fields ([Config files](https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/config-files.html))
| Field | Type | Default | Notes |
|---|---|---|---|
| `default_model` | string | — | alias defined in `[models]` |
| `default_permission_mode` | string | `manual` | `manual` \| `yolo` \| `auto` |
| `default_plan_mode` | bool | `false` | |
| `merge_all_available_skills` | bool | `true` | |
| `extra_skill_dirs` / `extra_agent_dirs` | array\<string\> | — | extra skills/agents search dirs |
| `builtin_product_skills` | bool | `true` | built-in doc skills (`update-config`, `mcp-config`, …); v2 engine only |
| `telemetry` | bool | `true` | |
| `providers` / `models` / `thinking` / `loop_control` / `background` / `subagent` / `mcp` / `token_counting` / `tools` / `image` / `services` / `permission` / `hooks` / `identity` / `secondary_model` | tables | see below | |

Key sub-tables:
- **`[providers.<name>]`** — `type` (required: `kimi`|`anthropic`|`openai`|`openai_responses`|`google-genai`|`vertexai`), `api_key`, `base_url`, `oauth {storage,key}`, `env` (fallback credential map), `custom_headers`. Credential priority: `api_key` field > `[providers.<name>.env]` key > startup error. **Shell env vars are never auto-read for credentials.**
- **`[models."<alias>"]`** — `provider`, `model`, `max_context_size` (required), optional `max_input_size`, `max_output_size` (anthropic only), `capabilities` (`thinking`,`always_thinking`,`image_in`,`video_in`,`audio_in`,`tool_use` — unioned with autodetect), `support_efforts`, `default_effort`, `off_effort`, `base_url` (per-model endpoint override), `display_name`, `reasoning_key` (openai), `adaptive_thinking` (anthropic). Plus `[models."<alias>".overrides]` for refresh-proof user pins (accepts model fields; not `provider`/`model`/`protocol`/`beta_api`/`base_url`).
- **`[secondary_model]`** (experimental; needs `KIMI_CODE_EXPERIMENTAL_SECONDARY_MODEL=1` or master flag) — `default_model`, `models` pool table (alias → hint), `force`.
- **`[thinking]`** — `enabled` (default true), `effort` (`low…max`), `keep` (default `"all"`; anthropic keep routes to beta Messages API). Deprecated: `default_thinking`, `thinking.mode` (both → `enabled`, since 0.21.0).
- **`[loop_control]`** — `max_steps_per_turn` (0/unset = unlimited), `max_attempts_per_step` (10), `reserved_context_size`. Deprecated renames since 0.32.0: `max_retries_per_step`, `max_steps_per_run`.
- **`[background]`** — `max_running_tasks`, `keep_alive_on_exit` (false), `kill_grace_period_ms` (5000), `bash_auto_background_on_timeout` (true), `bash_task_timeout_s` (600; 0 = none), print-mode: `print_background_mode` (`exit|drain|steer`, default steer), `print_wait_ceiling_s`, `print_max_turns`.
- **`[subagent]`** — `timeout_ms` (7200000; 0 = no timeout).
- **`[mcp]`** — `startup_timeout_ms` (30000), `tool_timeout_ms` (60000).
- **`[token_counting]`** — `strategy`: `measured+estimated` (default) | `measured` | `estimated`.
- **`[tools]`** — global allow/deny: `enabled`, `disabled`; exact names for built-ins, globs for MCP (`mcp__github__*`).
- **`[image]`** — `max_edge_px` (2000), `read_byte_budget` (262144).
- **`[services.moonshot_search]` / `[services.moonshot_fetch]`** — `base_url`, `api_key`, `oauth`, `custom_headers`.
- **`[[permission.rules]]`** — see permissions below.
- **`[[hooks]]`** — lifecycle hooks: `event` (e.g. `PreToolUse`), `matcher`, `command`, `timeout` ([Hooks docs](https://www.kimi.com/code/docs/en/kimi-code-cli/customization/hooks.html)).
- **`[identity]`** — `name`, `slug` (User-Agent token / MCP client name).

#### `tui.toml` ([Config files](https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/config-files.html#tui-toml))
`theme` (`auto|dark|light|custom`), `render_latex` (true), `disable_paste_burst` (false), `cache_expiry_hint` (true), `[editor].command`, `[notifications].enabled` + `notification_condition` (`unfocused|always`), `[upgrade].auto_install` (true), `[status_line].items` (`mode,goal,model,tasks,cwd,git,tips`) + `[status_line].command` (300ms cap, JSON snapshot on stdin).

#### Permissions ([Config files § permission](https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/config-files.html#permission), [MCP docs](https://www.kimi.com/code/docs/en/kimi-code-cli/customization/mcp.html))
```toml
default_permission_mode = "manual"   # manual | yolo | auto

[[permission.rules]]                 # first match wins
decision = "allow"                   # allow | deny | ask
scope   = "user"                     # turn-override | session-runtime | project | user
pattern = "Bash(rm -rf*)"            # ToolName or ToolName(arg-pattern)
reason  = "block recursive deletes"
```
MCP tools match as `mcp__<server>__<tool>` with `*`/`**` wildcards; argument patterns unsupported for `AgentSwarm`, MCP, and custom tools. YOLO mode auto-approves all MCP calls.

### Legacy (kimi-cli, Python) — for reference
Config: `~/.kimi/config.toml`, overridable per-invocation via `kimi --config-file /path/to/config.toml` or inline `kimi --config '{"default_model": …}'` ([legacy Config Files](https://moonshotai.github.io/kimi-cli/en/configuration/config-files.html)). Same TOML shape but different defaults/fields: `default_thinking`, `default_yolo`, `skip_afk_prompt_injection`, `default_editor`, `theme`, `show_thinking_stream`, `[loop_control] max_ralph_iterations` / `compaction_trigger_ratio`, `[background] agent_task_timeout_s`, `[mcp.client] tool_call_timeout_ms`. Provider types were `kimi`, `openai_legacy`, `openai_responses`, `gemini`, `vertexai` (no `anthropic` type).

---

## 2. Environment variables (complete list)

Source of truth: [Env vars](https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/env-vars.html). **Critical:** `KIMI_API_KEY` etc. in your shell do nothing by themselves — credentials must be written into `config.toml` under `[providers.<name>]` or `[providers.<name>.env]`. Only the `KIMI_MODEL_*` family reads credentials from the shell.

**Paths & identity**
- `KIMI_CODE_HOME` — data root (default `~/.kimi-code`). *No `KIMI_HOME` exists in the new CLI*; the legacy Python CLI used `~/.kimi` implicitly.
- `KIMI_CODE_IDENTITY_NAME`, `KIMI_CODE_IDENTITY_SLUG`
- `KIMI_SHELL_PATH` (Windows Git Bash path)

**OAuth / managed endpoints** — `KIMI_CODE_OAUTH_HOST` (> `KIMI_OAUTH_HOST` > `https://auth.kimi.com`); `KIMI_CODE_BASE_URL` (managed API after OAuth login, default `https://api.kimi.com/coding/v1`). Note: `KIMI_CODE_BASE_URL` (kimi.com managed) ≠ `KIMI_BASE_URL` (moonshot.ai direct API key).

**Credential key names (inside `config.toml [providers.<name>.env]`, not shell)** — `KIMI_API_KEY`, `KIMI_BASE_URL` (kimi type, default `https://api.moonshot.ai/v1`); `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL` (SDK default); `OPENAI_API_KEY`, `OPENAI_BASE_URL` (default `https://api.openai.com/v1`, both openai types); `GOOGLE_API_KEY`; Vertex: `VERTEXAI_API_KEY`, `GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION` (+ `GOOGLE_APPLICATION_CREDENTIALS` read directly by the Google SDK). Gateway-only extras: `GOOGLE_GEMINI_BASE_URL`, `GOOGLE_VERTEX_BASE_URL`.

**Model-from-env family** (enable switch = setting `KIMI_MODEL_NAME`; beats `default_model`, loses to `-m`): `KIMI_MODEL_NAME`, `KIMI_MODEL_API_KEY` (required), `KIMI_MODEL_PROVIDER_TYPE` (`kimi|anthropic|openai`), `KIMI_MODEL_BASE_URL`, `KIMI_MODEL_MAX_CONTEXT_SIZE` (262144), `KIMI_MODEL_CAPABILITIES` (csv, default `image_in,thinking`), `KIMI_MODEL_DISPLAY_NAME`, `KIMI_MODEL_MAX_OUTPUT_SIZE` (anthropic), `KIMI_MODEL_REASONING_KEY` (openai), `KIMI_MODEL_THINKING_EFFORT`, `KIMI_MODEL_ADAPTIVE_THINKING`.

**Runtime switches**: `KIMI_DISABLE_TELEMETRY`, `KIMI_DISABLE_CRON`, `KIMI_CODE_PASSWORD` (auth for `kimi web`), `KIMI_CODE_BACKGROUND_KEEP_ALIVE_ON_EXIT`, `KIMI_CODE_BACKGROUND_MAX_RUNNING_TASKS`, `KIMI_SUBAGENT_TIMEOUT_MS`, `KIMI_IMAGE_MAX_EDGE_PX`, `KIMI_IMAGE_READ_BYTE_BUDGET`, `KIMI_MCP_STARTUP_TIMEOUT_MS`, `KIMI_MCP_TOOL_TIMEOUT_MS`, `KIMI_LOOP_MAX_STEPS_PER_TURN`, `KIMI_LOOP_MAX_ATTEMPTS_PER_STEP` (deprecated alias `KIMI_LOOP_MAX_RETRIES_PER_STEP` still honored), `KIMI_TOKEN_COUNTING_STRATEGY`, `KIMI_WEB_SEARCH_BASE_URL`/`_API_KEY`, `KIMI_WEB_FETCH_BASE_URL`/`_API_KEY`, `KIMI_CODE_PLUGIN_MARKETPLACE_URL`, `KIMI_CODE_AGENT_SWARM_MAX_CONCURRENCY`, `KIMI_CODE_BUILTIN_PRODUCT_SKILLS`, `KIMI_CODE_TUI_FULL_SCREEN`, `KIMI_CODE_NO_AUTO_UPDATE` (alias `KIMI_CLI_NO_AUTO_UPDATE`), `KIMI_CODE_CUSTOM_HEADERS` (newline-sep `Name: Value`, added 0.20.2), engine flags `KIMI_CODE_LEGACY_FLAG`, `KIMI_CODE_EXPERIMENTAL_FLAG`, `KIMI_CODE_EXPERIMENTAL_SECONDARY_MODEL`, `KIMI_CODE_EXPERIMENTAL_SUBAGENT_FORK`. kimi-provider wire tuning: `KIMI_MODEL_MAX_COMPLETION_TOKENS`, `KIMI_MODEL_TEMPERATURE`, `KIMI_MODEL_TOP_P`, `KIMI_MODEL_THINKING_EFFORT` (global variant), `KIMI_MODEL_THINKING_KEEP`.

**Logs**: `KIMI_LOG_LEVEL` (`off|error|warn|info|debug`), `KIMI_LOG_GLOBAL_MAX_BYTES` (6 MB), `KIMI_LOG_GLOBAL_FILES` (5), `KIMI_LOG_SESSION_MAX_BYTES` (5 MB), `KIMI_LOG_SESSION_FILES` (3).

**System/proxy (read-only)**: `HOME`, `VISUAL`/`EDITOR`, `PATH`, `NO_COLOR`/`FORCE_COLOR`, `CI`, `TERM_PROGRAM`/`TERM`/`TMUX`, `DISPLAY`/`WAYLAND_DISPLAY`/`XDG_SESSION_TYPE`, `WSL_DISTRO_NAME`/`WSLENV`, `LOCALAPPDATA`; proxies `HTTP(S)_PROXY`, `ALL_PROXY` (SOCKS: `socks5://`, `socks5h://`, `socks4://`), `NO_PROXY`; loopback always bypasses.

---

## 3. Providers

Current provider `type`s ([Providers](https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/providers.html)): `kimi` (OpenAI-compatible — managed service `https://api.kimi.com/coding/v1` via `/login` OAuth, or platform API key against `https://api.moonshot.ai/v1`; also `https://api.moonshot.cn/v1` for CN; video-in supported), `anthropic`, `openai`, `openai_responses`, `google-genai`, `vertexai`. Interactive management: `/provider` TUI command or `kimi provider` shell command; imports from models.dev catalog or a custom `api.json` registry URL.

```toml
# Moonshot Open Platform API key (or .cn mirror)
[providers.kimi]
type = "kimi"
base_url = "https://api.moonshot.ai/v1"
api_key = "sk-xxx"
```

**Anthropic-compatible repointing (GLM-style): YES, two ways.** (a) Set `type = "anthropic"` with any `base_url` + `ANTHROPIC_API_KEY`/`ANTHROPIC_BASE_URL` in the `env` sub-table — this speaks Anthropic Messages to whatever endpoint you point it at (works for GLM/Kimi Anthropic-compatible gateways; the docs even describe "Kimi's Anthropic-compatible mode" re: `[thinking] keep` routing to the beta Messages API). (b) Any OpenAI-compatible gateway via `type = "openai"` + custom `base_url` (DeepSeek/Qwen/OneAPI reasoning fields handled automatically; `reasoning_key` override available). Per-model `base_url` overrides are also supported on `[models]` entries.

```toml
# GLM-style Anthropic-compatible endpoint
[providers.glm]
type = "anthropic"
base_url = "https://your-anthropic-compat-gateway.example"
[providers.glm.env]
ANTHROPIC_API_KEY = "xxx"

# Custom OpenAI-compatible
[providers.custom]
type = "openai"
base_url = "https://api.example.com/v1"
api_key = "sk-xxx"
```

Legacy kimi-cli had no `anthropic` type — third-party meant `openai_legacy`/`openai_responses`/`gemini`/`vertexai` ([legacy Providers](https://moonshotai.github.io/kimi-cli/en/configuration/providers.html)).

---

## 4. Multi-instance wrappers (env/config isolation)

The documented isolation mechanism is `KIMI_CODE_HOME`: "Multiple `kimi` instances sharing the same `KIMI_CODE_HOME` will share config and credential files" — so give each instance its own home ([Env vars](https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/env-vars.html)). Combine per-instance home + per-instance model via `KIMI_MODEL_*`:

```bash
#!/usr/bin/env bash
# ~/bin/kimi-work — isolated Kimi instance with its own home, key, model
export KIMI_CODE_HOME="$HOME/.kimi-instances/work"
export KIMI_MODEL_NAME="kimi-for-coding"          # temporary model, not persisted
export KIMI_MODEL_API_KEY="sk-work-key"
export KIMI_MODEL_BASE_URL="https://api.moonshot.ai/v1"
export KIMI_DISABLE_TELEMETRY=1
exec kimi "$@"          # first run: `kimi-work` then /login inside it
```

Notes: an instance pointed at a fresh `KIMI_CODE_HOME` starts unauthenticated — run `/login` once inside that home (OAuth creds live under the home dir too). For ACP/editor launches, put these exports in Zed's `agent_servers.*.env` object instead of your shell. For fully ephemeral runs, `--config '{...}'`-style inline config was the legacy CLI mechanism; the current CLI prefers `KIMI_MODEL_*` or editing `config.toml`.

---

## 5. ACP integration (Zed / JetBrains), skills & MCP

Both CLIs speak the Agent Client Protocol out of the box; log in once in the terminal (`/login`), then point your editor at `kimi acp` — no extra login ([kimi-cli README](https://github.com/MoonshotAI/kimi-cli), [kimi-code README](https://github.com/MoonshotAI/kimi-code)). Zed `~/.config/zed/settings.json`:

```json
{
  "agent_servers": {
    "Kimi Code CLI": {
      "type": "custom",
      "command": "kimi",
      "args": ["acp"],
      "env": {}
    }
  }
}
```
JetBrains uses `~/.jetbrains/acp.json` with the same shape ([IDEs guide](https://www.kimi.com/code/docs/en/kimi-code-cli/guides/ides.html); full capability matrix: [`kimi acp` reference](https://www.kimi.com/code/docs/en/kimi-code-cli/reference/kimi-acp.html)). Zed also forwards its own `agent_servers` MCP declarations to the agent ([Kimi Help Center](https://www.kimi.com/en/help/kimi-code/cli-ides)). Listed in the ACP registry: [zed.dev/acp/agent/kimi-cli](https://zed.dev/acp/agent/kimi-cli).

**Skills**: directories merged from default locations plus `extra_skill_dirs` / `extra_agent_dirs` in `config.toml`; `merge_all_available_skills = true`; `builtin_product_skills` toggles the built-in doc skills. Plugins (marketplace via `/plugins`, URL overridable with `KIMI_CODE_PLUGIN_MARKETPLACE_URL`) can package skills + MCP servers + data sources.

**MCP config** ([MCP docs](https://www.kimi.com/code/docs/en/kimi-code-cli/customization/mcp.html)):
- Files: user `~/.kimi-code/mcp.json`, project `.kimi-code/mcp.json` (project wins on name collision; stdio entries require trusting the folder). Interactive: `/mcp-config` (add/edit/auth), `/mcp` (status).
- CLI management: `kimi mcp add --transport http|stdio [--auth oauth] [--header "K: v"] <name> <url-or--- cmd>`, `kimi mcp list|remove|auth <name>`; ad-hoc file: `kimi --mcp-config-file /path/to/mcp.json`.
- Entry fields: `command`+`args` (stdio), `url` (HTTP; `transport:"sse"` for legacy SSE), `env`, `cwd`, `headers`, `bearerTokenEnvVar`, `enabled`, `startupTimeoutMs` (30000), `toolTimeoutMs` (60000), `enabledTools`, `disabledTools`. Tools surface as `mcp__<server>__<tool>`; permission rules accept wildcards.

---

## 6. Model selection

- Managed models provisioned by `/login` (aliases under `managed:kimi-code` / `kimi-code/*`): **`k3`** (`max_context_size = 1048576`, efforts `low|high|max`, default `max`), **`kimi-for-coding`**, **`kimi-for-coding-highspeed`** (both 262144 ctx) — from the official complete example ([Config files](https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/config-files.html)). Legacy-era examples referenced `kimi-k2-thinking-turbo` style ids on the open platform; current platform serves the K2.5/K3 family through the same `[models]` mechanism — declare any id your endpoint accepts.
- Selection precedence: `-m <alias>` CLI flag > `KIMI_MODEL_NAME` env > `default_model` in `config.toml`. In-session: `/model`.
- Custom alias example:
```toml
default_model = "k2p5"
[models.k2p5]
provider = "kimi"
model = "kimi-k2.5"                # id sent to the API
max_context_size = 262144
capabilities = ["thinking", "image_in", "tool_use"]
support_efforts = ["low", "medium", "high"]
default_effort = "high"
```
- Subagent model pool (`[secondary_model]`, experimental) lets main agents spawn subagents on cheaper/faster aliases; `"primary"` = caller's model; effort variants via `overrides.default_effort`.

### Unverified / caveats
- No official `KIMI_HOME` var exists today — the relocation knob is `KIMI_CODE_HOME` (new) and implicit `~/.kimi` (legacy).
- Exact K2.5 model-id strings on api.moonshot.ai were not verifiable from fetched pages (docs show `k3`, `kimi-for-coding`, `kimi-k2-thinking-turbo` examples); check `platform.moonshot.ai` model list when wiring a raw API key.
