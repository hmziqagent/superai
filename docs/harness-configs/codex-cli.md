# OpenAI Codex CLI — Complete Configuration Reference

Compiled 2026-08-25. Primary sources, cited inline throughout:
- Config reference: https://developers.openai.com/codex/config-reference
- Advanced config: https://developers.openai.com/codex/config-advanced
- Environment variables: https://developers.openai.com/codex/config-file/environment-variables
- Auth: https://developers.openai.com/codex/auth
- Repo docs index: https://github.com/openai/codex/blob/main/docs/config.md (points to developers.openai.com pages above)
- Historical repo config doc (pre-0.134 schema, incl. `wire_api = "chat"`, `[profiles.*]`): archived mirrors of openai/codex `docs/config.md`, e.g. https://github.com/chaitin/monkeycode-cli/blob/main/docs/config.md

---

## 1) `~/.codex/config.toml` — TOML schema

User-level config lives in `~/.codex/config.toml`. Project-scoped overrides live in `<repo>/.codex/config.toml` and load **only when the project is trusted**. Profile layers live at `$CODEX_HOME/<profile-name>.config.toml` and are selected with `--profile <name>` ([config-reference](https://developers.openai.com/codex/config-reference), [config-advanced](https://developers.openai.com/codex/config-advanced)).

**Layer precedence (low → high)**: user `config.toml` → profile file (`$CODEX_HOME/name.config.toml`) → project `.codex/config.toml` (trusted projects; closest-to-cwd wins among multiple) → CLI flags / `-c key=value` overrides. [config-advanced]

**Keys ignored in project-local `.codex/config.toml`** (security: they redirect credentials/telemetry/notifications): `openai_base_url`, `chatgpt_base_url`, `apps_mcp_product_sku`, `model_provider`, `model_providers`, `notify`, `profile`, `profiles`, `experimental_realtime_ws_base_url`, `otel`. Codex prints a startup warning if it sees them there. [config-reference, config-advanced]

### Core model & provider keys

| Key | Type / Values | Notes |
|---|---|---|
| `model` | string | Model id, e.g. `"gpt-5.5"`. Must match provider's exact id format (`vendor/model-name` on OpenRouter, bare ids like `gpt-5.3-codex` for OpenAI). |
| `model_provider` | string | Provider id from `model_providers` map. Default `"openai"`. |
| `model_context_window` | number | Context window tokens available to the active model. |
| `model_auto_compact_token_limit` | number | Token threshold triggering automatic history compaction. |
| `model_auto_compact_token_limit_scope` | `total` \| `body_after_prefix` | What the compaction threshold counts. |
| `model_reasoning_effort` | `minimal \| low \| medium \| high \| xhigh` | Reasoning effort (Responses API only; `xhigh` model-dependent). |
| `model_reasoning_summary` | `auto \| concise \| detailed \| none` | Reasoning summary detail. |
| `model_supports_reasoning_summaries` | boolean | Force send/not-send reasoning metadata. |
| `model_verbosity` | `low \| medium \| high` | GPT-5 Responses API verbosity override (ignored by Chat Completions providers). |
| `model_instructions_file` | string (path) | Replace built-in instructions (instead of `AGENTS.md`). |
| `model_catalog_json` | string (path) | Optional JSON model catalog loaded at startup; overridable per-profile. |
| `review_model` | string | Model override for `/review`. |
| `oss_provider` | `lmstudio \| ollama` | Default local provider used with `--oss`. |
| `openai_base_url` | string | Base URL override for the **built-in** `openai` provider — use this instead of redefining `[model_providers.openai]` (built-in ids are reserved). |
| `chatgpt_base_url` | string | Override base URL used during ChatGPT login flow. |

### `model_providers.<id>`

Custom provider definition table. Built-in provider IDs (`openai`, `ollama`, `lmstudio`) are reserved and cannot be overridden; a built-in `amazon-bedrock` provider also exists. [config-reference]

| Key | Type / Values | Notes |
|---|---|---|
| `name` | string | Display name shown in the Codex UI. |
| `base_url` | string | API base URL. For chat-completions wire APIs, `/chat/completions` is appended; for responses, `/responses` is appended. |
| `env_key` | string | Name of env var supplying the API key; value must be non-empty, sent as `Bearer TOKEN`. |
| `env_key_instructions` | string | Optional setup guidance shown to users for that key. |
| `wire_api` | see note | Protocol. **Current docs (2026): `responses` is the only supported value and the default when omitted.** Historical docs (pre-2026 codex releases) allowed `"chat"` (OpenAI Chat Completions) with default `"chat"` when omitted. If you're on an older release, `wire_api = "chat"` works against any OpenAI-compatible Chat Completions endpoint. On current builds, endpoints must speak the Responses API or sit behind a translating gateway (LiteLLM, OpenRouter's Responses-compatible surface). |
| `query_params` | map<string,string> | Extra query params appended to request URLs (e.g. Azure `api-version`). |
| `http_headers` | map<string,string> | Static HTTP headers added to provider requests. |
| `env_http_headers` | map<string,string> | Headers populated from environment variables when present. |
| `requires_openai_auth` | boolean | Provider uses OpenAI auth (ChatGPT sign-in or API key); when true, `env_key` is ignored. Useful for OpenAI-through-proxy setups. |
| `experimental_bearer_token` | string | Direct bearer token (discouraged; use `env_key`). |
| `request_max_retries` | number | HTTP retry count (default 4). |
| `stream_max_retries` | number | SSE stream interruption retries (default 5). |
| `stream_idle_timeout_ms` | number | SSE idle timeout ms (default 300000). |
| `supports_standalone_web_search` | boolean | Advertise compatible standalone web-search endpoint (default false; feature still off by default). |
| `supports_websockets` | boolean | Provider supports the Responses API WebSocket transport. |
| `auth` | table | Command-backed bearer token. Fields: `command` (prints token to stdout), `args` [], `cwd`, `timeout_ms` (default 5000), `refresh_interval_ms` (default 300000; 0 = refresh only after auth retry). **Do not combine with `env_key`, `experimental_bearer_token`, or `requires_openai_auth`.** |

Built-in Bedrock overrides:
```toml
model_provider = "amazon-bedrock"
model = "<bedrock-model-id>"
[model_providers.amazon-bedrock.aws]
profile = "default"        # omit → standard AWS credential chain
region  = "eu-central-1"
```
[config-advanced § Amazon Bedrock provider]

### Profiles

Two mechanisms exist across versions ([config-advanced § Profiles]):

- **Current (Codex ≥ 0.134.0)**: each profile is its own file `$CODEX_HOME/<name>.config.toml`, selected via `codex --profile <name>`. Use top-level keys in the profile file; do NOT nest under `[profiles.<name>]`. The legacy top-level `profile = "name"` selector is no longer supported.
  ```toml
  # ~/.codex/deep-review.config.toml
  model = "gpt-5.5"
  model_reasoning_effort = "xhigh"
  approval_policy = "on-request"
  ```
  ```bash
  codex --profile deep-review
  codex exec --profile deep-review "review this change"
  ```
- **Legacy (< 0.134.0)**: inline tables in config.toml:
  ```toml
  [profiles.o3]
  model = "o3"
  model_provider = "openai"
  approval_policy = "never"
  model_reasoning_effort = "high"
  model_reasoning_summary = "detailed"
  ```

Profile files may override anything the user layer can, including `model_provider`, providers, and `model_catalog_json`.

### Approvals & sandbox

```toml
approval_policy = "on-request"   # untrusted | on-request | never | { granular = {...} }
sandbox_mode    = "workspace-write"  # read-only | workspace-write | danger-full-access
allow_login_shell = false            # hardening: reject login-shell requests

[sandbox_workspace_write]
exclude_tmpdir_env_var = false   # allow $TMPDIR
exclude_slash_tmp      = false   # allow /tmp
writable_roots         = ["/Users/YOU/.pyenv/shims"]
network_access         = false   # opt in to outbound network from sandboxed commands
```
[config-advanced § Approval policies and sandbox modes]

- `approval_policy` granular form: `{ granular = { sandbox_approval, rules, mcp_elicitations, request_permissions, skill_approval } }` — allow (true) or auto-reject (false) individual prompt categories. `on-failure` is deprecated.
- `approvals_reviewer = "user" | "auto_review"` — route eligible prompts through the reviewer subagent; `[auto_review].policy` supplies local Markdown reviewer policy.
- Beta permission profiles: `default_permissions = ":read-only" | ":workspace" | ":danger-full-access" | "<custom-name>"` plus `[permissions.<name>]` tables with `extends`, `filesystem.<path-or-glob>` (`read|write|deny`), `network.enabled/domains/proxy_url/...`. Don't combine `default_permissions` with `sandbox_mode`/`[sandbox_workspace_write]`.

### MCP servers (`mcp_servers`)

```toml
[mcp_servers.context7]
command = "npx"
args    = ["-y", "@upstash/context7-mcp"]
env     = { "API_KEY" = "value" }
startup_timeout_sec = 20     # default 10
tool_timeout_sec    = 120    # default 60
enabled_tools = ["resolve-library-id", "get-library-docs"]

[mcp_server_http_example]         # streamable HTTP form uses url:
# url = "https://mcp.example.com/mcp"
# bearer_token_env_var = "MY_TOKEN"
# http_headers  = { "X-Custom" = "v" }
# env_http_headers = { "X-Token" = "MY_TOKEN_ENV" }
# auth = "oauth" | "chatgpt"
```
Full per-server keys ([config-reference]): `command`, `args`, `env`, `env_vars`, `cwd`, `url`, `bearer_token_env_var`, `http_headers`, `env_http_headers`, `auth` (`oauth|chatgpt`), `scopes`, `oauth_resource`, `enabled`, `enabled_tools`/`disabled_tools` (deny wins), `default_tools_approval_mode`, `tools.<tool>.approval_mode` (`auto|prompt|writes|approve`), `required`, `startup_timeout_sec`/`_ms`, `tool_timeout_sec`, `experimental_environment` (`local|remote`). Admin requirements add an identity-based allowlist (`mcp_servers.<id>.identity.command/url` matchers).

### Notifications (`notify`) and history

```toml
notify = ["python3", "/path/to/notify.py"]   # receives one JSON argv payload
[history]
persistence = "save-all"   # save-all | none  → ~/.codex/history.jsonl
max_bytes   = 104857600    # cap size; drops oldest entries
```
Payload fields for `agent-turn-complete`: `type`, `thread-id`, `turn-id`, `cwd`, `input-messages`, `last-assistant-message`. TUI-side alternatives: `tui.notifications`, `tui.notification_method` (`auto|osc9|bel`), `tui.notification_condition` (`unfocused|always`). [config-advanced § Notifications]

### Shell environment policy

```toml
[shell_environment_policy]
inherit = "core"                  # all | core | none
set = { MY_FLAG = "1" }
ignore_default_excludes = false   # false → auto-strip names containing KEY/SECRET/TOKEN

[shell_environment_policy.filters]
"AWS_*" = "exclude"               # case-insensitive globs; "include" makes it an allowlist
```
Order: auto-exclusions → custom exclusions → `set` (can restore excluded vars) → include allowlist. Legacy arrays `exclude` / `include_only` still work but must not be combined with `filters` in the same layer. [config-advanced § Shell environment policy]

### Project trust & per-project config

- Codex walks from project root down to cwd loading every `<repo>/.codex/config.toml`; loaded only for **trusted projects** (trust prompt per directory; untrusted projects ignore project `.codex/` layers entirely).
- Relative paths inside project config resolve against the containing `.codex/` folder.
- Project root detection: directory containing `.git` by default; customize with `project_root_markers = [".git", ".hg", ".sl"]` (or `[]` to treat cwd as root). [config-advanced]
- AGENTS.md knobs: `project_doc_max_bytes`, `project_doc_fallback_filenames`.

### Other notable top-level keys (selected)

`file_opener` (`vscode|vscode-insiders|windsurf|cursor|none`), `hide_agent_reasoning`, `show_raw_agent_reasoning`, `developer_instructions`, `check_for_update_on_startup`, `disable_paste_burst`, `background_terminal_max_timeout`, `compact_prompt`, `log_dir` (defaults `$CODEX_HOME/log`; setting it enables plaintext `codex-tui.log`), `forced_login_method` (`chatgpt|api`), `forced_chatgpt_workspace_id`, `cli_auth_credentials_store` (`file|keyring|auto`), `[analytics] enabled`, `[feedback] enabled`, `[otel]` (exporter/metrics/trace exporters, TLS, headers), `[hooks]` lifecycle hooks, `[agents]` subagent roles, `[features.*]` toggles (web_search, unified_exec, network_proxy, memories, multi_agent, …), `[tui.*]`. Full list: [config-reference](https://developers.openai.com/codex/config-reference).

One-off overrides without editing files ([config-advanced]):
```bash
codex --model gpt-5.6-terra
codex -c model='"gpt-5.6-terra"'
codex -c sandbox_workspace_write.network_access=true
codex -c mcp_servers.context7.enabled=false     # dot notation for nested keys
```
(`-c` values parse as TOML; unparseable values are treated as strings.)

---

## 2) Complete environment variable list

Stable public variables Codex reads directly ([environment-variables](https://developers.openai.com/codex/config-file/environment-variables)):

| Variable | Purpose |
|---|---|
| **`CODEX_HOME`** | Root for all Codex state — config.toml, auth.json, logs, sessions, skills, packages. Default `~/.codex`. Directory must already exist. **This is the key var for multi-instance wrappers** (see §4). |
| `CODEX_SQLITE_HOME` | Where SQLite-backed state lives (default `$CODEX_HOME`); `sqlite_home` config takes precedence. |
| `CODEX_API_KEY` | Provides an API key to non-interactive processes (exec, review, TS SDK, remote exec-server). Prefer inline over job-wide in CI running untrusted repo code. |
| `CODEX_ACCESS_TOKEN` | ChatGPT/Codex access token for trusted automation; persist via `printenv CODEX_ACCESS_TOKEN \| codex login --with-access-token`. |
| `OPENAI_FEDERATION_RULE_ID` / `OPENAI_IDENTITY_TOKEN_FILE` / `OPENAI_WORKLOAD_IDENTITY_CONTEXT` | Workload identity federation (OIDC/SPIFFE). |
| `CODEX_CA_CERTIFICATE` | PEM CA bundle for corporate TLS interception; beats `SSL_CERT_FILE`. Applies to HTTPS, login, WebSockets. |
| `SSL_CERT_FILE` | Fallback PEM CA bundle path. |
| `CODEX_NON_INTERACTIVE` | Installer scripts: skip prompts (`1\|true\|yes`). |
| `CODEX_INSTALL_DIR` | Installer target dir (default `~/.local/bin`; Windows `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`). |
| `RUST_LOG` | Rust log filtering (`error\|warn\|info\|debug\|trace`, or targeted e.g. `codex_core=debug`). `codex exec` defaults to `error`. |

Provider/API-key related (not fixed names — Codex reads whatever var `env_key` names):
- `OPENAI_API_KEY` — de facto standard; used as `env_key` of the built-in OpenAI provider and accepted by `codex login --with-api-key` via stdin. Historically also honored directly for exec/API-key flows.
- Any custom name you declare, e.g. `OPENROUTER_API_KEY`, `MISTRAL_API_KEY`, `AZURE_OPENAI_API_KEY`, `GROQ_API_KEY`, etc.
- `OPENAI_BASE_URL` — widely honored convention for overriding the OpenAI base URL in older releases; current canonical knob is the `openai_base_url` config key ([config-advanced]). Set both if you need compatibility.

---

## 3) Third-party / OSS providers — worked examples

⚠️ **`wire_api` version note**: current official reference says `responses` is the *only* supported value and the default. Older releases (2025-era, most tutorials) accept `wire_api = "chat"` for plain Chat Completions endpoints, defaulting to `"chat"`. Check your `codex --version`: if your endpoint only speaks Chat Completions and your build rejects `chat`, front it with LiteLLM (which exposes a Responses-compatible surface) or use a router like OpenRouter. Examples below show both forms.

### OpenRouter (any OpenAI-compatible model)

```toml
# ~/.codex/config.toml
model = "anthropic/claude-sonnet-4.6"       # vendor/model format
model_provider = "openrouter"

[model_providers.openrouter]
name = "OpenRouter"
base_url = "https://openrouter.ai/api/v1"
env_key = "OPENROUTER_API_KEY"
wire_api = "responses"                       # omit on current builds; use "chat" on old ones
http_headers = { "HTTP-Referer" = "https://yourapp.example", "X-Title" = "Codex" }
```
```bash
export OPENROUTER_API_KEY="sk-or-..."
codex
```

### Ollama (local, no auth)

```toml
[model_providers.local_ollama]
name = "Ollama"
base_url = "http://localhost:11434/v1"
# no env_key → no authentication assumed
wire_api = "responses"          # legacy: "chat"
```
Or simply use built-in OSS mode:
```bash
oss_provider = "ollama"   # or "lmstudio", in config.toml
codex --oss -m qwen3.5-coder:latest
codex --oss --local-provider lmstudio
```
([config-advanced § OSS mode])

### LM Studio (local)

```toml
[model_providers.lmstudio_local]
name = "LM Studio"
base_url = "http://localhost:1234/v1"
wire_api = "responses"          # legacy: "chat"
```

### Mistral

```toml
[model_providers.mistral]
name = "Mistral"
base_url = "https://api.mistral.ai/v1"
env_key = "MISTRAL_API_KEY"
wire_api = "responses"          # legacy: "chat"
```
```bash
export MISTRAL_API_KEY=...
codex -c model='"mistral-large-latest"' --profile mistral
```
(Official Mistral example from [config-advanced § Custom model providers].)

### Azure OpenAI (official example)

```toml
[model_providers.azure]
name = "Azure"
base_url = "https://YOUR_PROJECT_NAME.openai.azure.com/openai"
env_key = "AZURE_OPENAI_API_KEY"           # or OPENAI_API_KEY
query_params = { api-version = "2025-04-01-preview" }
wire_api = "responses"
request_max_retries = 4
stream_max_retries = 10
stream_idle_timeout_ms = 300000
```
[config-advanced § Azure provider and per-provider tuning; historical repo config.md § Azure example]

### Amazon Bedrock (built-in)

See §1 Bedrock block — `model_provider = "amazon-bedrock"` + `[model_providers.amazon-bedrock.aws] profile/region`.

### Generic OpenAI-compatible endpoint behind a command-token auth (e.g. internal gateway)

```toml
[model_providers.gateway]
name = "Internal gateway"
base_url = "https://gateway.corp.example/v1"
wire_api = "responses"

[model_providers.gateway.auth]
command = "/usr/local/bin/fetch-codex-token"   # must print token to stdout, no stdin
args = ["--audience", "codex"]
timeout_ms = 5000
refresh_interval_ms = 300000                    # 0 → refresh only after auth retry
```
[config-advanced § Custom model providers]

### Data residency

```toml
model_provider = "openaidr"
[model_providers.openaidr]
name = "OpenAI Data Residency"
base_url = "https://us.api.openai.com/v1"   # replace 'us' prefix per region
```
[config-advanced § API organizations using data residency]

**Rules of thumb**: never define `[model_providers.openai]` (reserved — silently ignored); use `openai_base_url` instead. The `model` id must match the target provider's exact format. Models without tool-calling support will chat but never edit files.

---

## 4) Multi-instance wrappers — CODEX_HOME relocation + profiles

Three isolation levels:

1. **Profiles (lightweight, shared state)** — `codex --profile work` overlays `$CODEX_HOME/work.config.toml` onto your base config. Shares history/auth/sessions.
2. **`CODEX_HOME` relocation (full isolation)** — completely separate config, `auth.json`, history, sessions, skills, logs per instance. Ideal for running two instances with different providers/logins simultaneously.
3. **Per-invocation `-c` overrides** — zero persistence, ad-hoc: `codex -c model_provider='"openrouter"'`.

Isolation recipe:
```bash
mkdir -p ~/codex-instances/openrouter ~/codex-instances/local
cat > ~/codex-instances/openrouter/config.toml <<'EOF'
model = "anthropic/claude-sonnet-4.6"
model_provider = "openrouter"
[model_providers.openrouter]
name = "OpenRouter"
base_url = "https://openrouter.ai/api/v1"
env_key = "OPENROUTER_API_KEY"
EOF
cat > ~/codex-instances/local/config.toml <<'EOF'
model = "qwen3.5-coder:latest"
model_provider = "local_ollama"
[model_providers.local_ollama]
name = "Ollama"
base_url = "http://localhost:11434/v1"
EOF
```
Then launch each with its own `CODEX_HOME` (see wrapper script §6). Each home needs its own login (`codex login` / API key) since `auth.json` is per-home. Remember: **the directory must already exist** before setting `CODEX_HOME` ([environment-variables]).

---

## 5) Auth modes, reasoning effort, per-project trust

### Auth modes ([auth](https://developers.openai.com/codex/auth))

- **ChatGPT sign-in**: `codex login` → browser OAuth flow (localhost callback default port `1455`; forwardable over SSH). Subscription-based usage; subject to workspace RBAC/residency. Headless options: `codex login --device-auth` (beta), copy `auth.json`, or SSH tunnel `-L 1455:localhost:1455`.
- **API key**: `printenv OPENAI_API_KEY | codex login --with-api-key`. Pay-per-use Platform billing. Recommended for CI/automation.
- **Access tokens (enterprise)**: `printenv CODEX_ACCESS_TOKEN | codex login --with-access-token`.
- Status/logout: `codex login status`, `codex logout`.
- Credential storage: plaintext `~/.codex/auth.json` or OS keychain — controlled by `cli_auth_credentials_store = "file" | "keyring" | "auto"`. Treat `auth.json` like a password.
- Enforcement (managed environments): `forced_login_method = "chatgpt" | "api"`, `forced_chatgpt_workspace_id = "<uuid>"`. Mismatched credentials force logout.
- Custom-provider auth choices: `requires_openai_auth = true` (reuse OpenAI login, ignores `env_key`), `env_key = "VAR"` (provider-specific key), or neither (no auth — local models).

### Reasoning settings

```toml
model_reasoning_effort = "high"             # minimal | low | medium | high | xhigh (Responses API only)
model_reasoning_summary = "detailed"        # auto | concise | detailed | none
model_supports_reasoning_summaries = true   # force reasoning metadata on/off
plan_mode_reasoning_effort = "medium"       # plan-mode-specific override
hide_agent_reasoning = true                 # suppress reasoning output in TUI/exec
show_raw_agent_reasoning = true             # surface raw reasoning when the model emits it
```
Also settable per-invocation: `codex -c model_reasoning_effort='"high"'`.

### Per-project trusted configs

Trust is granted per-directory (interactive prompt). Once trusted, the repo's `.codex/config.toml`, `.codex/hooks.json`, rules, and skills load, layered under user config; nearest-file-wins for duplicate keys. All credential/provider/notification/telemetry keys listed in §1 are ignored at project level regardless of trust. Customize root detection with `project_root_markers`.

---

## 6) Example wrapper script — two isolated instances, different providers

```bash
#!/usr/bin/env bash
# codex-multi: run isolated Codex instances side by side.
# Usage: codex-multi <instance> [codex args...]
set -euo pipefail

INSTANCES_DIR="${HOME}/codex-instances"

launch() {
  local inst="$1"; shift
  local home="${INSTANCES_DIR}/${inst}"
  mkdir -p "$home"                       # CODEX_HOME must exist beforehand
  export CODEX_HOME="$home"              # isolates config, auth.json, history, sessions
  exec codex "$@"
}

case "${1:-}" in
  remote)  shift; launch openrouter "$@" ;;   # OpenRouter instance (uses $OPENROUTER_API_KEY)
  local)   shift; launch local "$@" ;;        # Ollama/LM Studio instance (no auth)
  *) echo "usage: $0 {remote|local} [codex args...]"; exit 1 ;;
esac
```

Run both concurrently in separate terminals:
```bash
./codex-multi remote                      # interactive, Claude via OpenRouter
./codex-multi local exec "fix the tests"  # non-interactive, local Qwen via Ollama
```

Profile-based alternative (shared state, lighter weight):
```bash
codex --profile deep-review
CODEX_API_KEY=sk-... codex exec --profile fast-lane "summarize the diff"
```

First-run checklist per isolated home: `mkdir -p <home>`, write `config.toml`, then authenticate once inside that home (`codex login`, or pipe an API key to `codex login --with-api-key`, or rely purely on `env_key` vars exported before launch).

---
*Version caveat: this document reflects current developers.openai.com docs (2026). Key breaking changes vs 2025-era docs: profiles moved from inline `[profiles.*]` tables to separate `$CODEX_HOME/<name>.config.toml` files (≥0.134.0); `wire_api = "chat"` removed (only `responses` supported now); legacy `--approval-mode`/`--provider` flags replaced by config keys.*
