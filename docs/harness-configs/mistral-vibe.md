# Mistral Vibe CLI — Configurable Options Reference

*Compiled 2026-08-25. Sources: [mistralai/mistral-vibe GitHub README](https://github.com/mistralai/mistral-vibe) (`README.md`, raw `main`) and Mistral official docs under `docs.mistral.ai/vibe/code/cli/*`. Inline citations mark each claim.*

Mistral Vibe is Mistral's open-source Python CLI coding agent (`pip install mistral-vibe`, Python 3.12+). It is "highly configurable: customize models, providers, tool permissions, and UI preferences through a simple `config.toml` file." — [GitHub README §Features](https://github.com/mistralai/mistral-vibe)

---

## 1) Config files: locations & schema

### File map (under `$VIBE_HOME`, default `~/.vibe/`)

| Path | Purpose |
|---|---|
| `config.toml` | Main configuration |
| `.env` | **Credentials only** — API keys / provider creds (never general config) |
| `trusted_folders.toml` | Remembered trusted directories |
| `hooks.toml` | Hook declarations (`pre_tool` / `post_tool` / `post_agent`) |
| `agents/*.toml` | Custom agent profiles |
| `prompts/*.md` | Custom system & compaction prompts (`system_prompt_id`, `compaction_prompt_id`) |
| `skills/`, `tools/` | Skills; legacy custom tools (deprecated in favor of skills) |
| `logs/vibe.log` | Structured logs |
| `shell-tool/sessions/` | Managed shell session logs |
| `worktrees/<repo>-<hash>/<name>` + `worktrees/.claims/...` | Git worktrees created by `--worktree`, with ownership records |

— [GitHub README §Configuration / §Custom Vibe Home](https://github.com/mistralai/mistral-vibe); [docs: Configuration](https://docs.mistral.ai/vibe/code/cli/configuration)

### Lookup order & precedence
- `config.toml` is searched first at `./.vibe/config.toml` (project-level), then `~/.vibe/config.toml` (user-level). **Project config wins over user config**, and project config is loaded *only when the working directory is trusted*. — [README §Configuration File Location](https://github.com/mistralai/mistral-vibe); [docs: Configuration](https://docs.mistral.ai/vibe/code/cli/configuration)
- Full precedence, highest→lowest: **admin config** → **CLI flags** → **environment variables** → project `config.toml` → user `config.toml`. — [docs: Configuration §Configuration precedence](https://docs.mistral.ai/vibe/code/cli/configuration)
- Hooks: `<project>/.vibe/hooks.toml` loads before `~/.vibe/hooks.toml`; duplicate hook names resolve to the project entry; project files load only in trusted folders. — [README §Hooks](https://github.com/mistralai/mistral-vibe)
- Admin/enterprise policy can pin settings org-wide, e.g. `active_model`, `enable_auto_update = false`, `disabled_tools`, and provider blocks. — [docs: Admin config](https://docs.mistral.ai/vibe/code/cli/admin-config)
- Defaults are built-in; no `config.toml` is created until you save a setting. Editable live via the `/config` slash command. — [README §Quick Start](https://github.com/mistralai/mistral-vibe)

### Representative `config.toml` schema
```toml
active_model = "mistral-medium-latest"   # or an alias you defined
default_agent = "plan"                   # interactive default agent
theme = "dracula"
enable_update_checks = true
enable_auto_update = true
enable_notifications = true
enable_telemetry = true
enable_otel = true                       # OpenTelemetry tracing (OTLP/HTTP)
# otel_endpoint = "https://collector.example.com:4318"
# otel_redaction = "default"             # default | strict | none
log_level = "INFO"
system_prompt_id = "my_custom_prompt"    # ~/.vibe/prompts/my_custom_prompt.md
skill_paths = ["/path/to/custom/skills"]
enabled_skills  = ["code-review", "test-*"]
disabled_skills = ["experimental-*"]
enabled_tools  = ["read_file", "grep"]   # exact / glob / re:^…$ regex
disabled_tools = ["bash"]

[tools.bash]
permission = "ask"
allow = ["git status", "pnpm test"]
deny  = ["rm -rf *"]

[[providers]]  # see section 3
name = "openrouter"
api_base = "https://openrouter.ai/api/v1"
api_key_env_var = "OPENROUTER_API_KEY"
api_style = "openai"
backend = "generic"

[[models]]
name = "mistralai/devstral-2512:free"    # upstream model id
provider = "openrouter"
alias = "devstral-openrouter"
temperature = 0.2
input_price = 0.0
output_price = 0.0

[[mcp_servers]]  # see section 5
```
— assembled from [README](https://github.com/mistralai/mistral-vibe) §§Configuration/Tool Management/Skills/MCP and [docs: Configuration](https://docs.mistral.ai/vibe/code/cli/configuration), [docs: API keys and profiles](https://docs.mistral.ai/vibe/code/cli/api-keys-profiles)

### Auth storage
- First-run wizard (`vibe` or `vibe --setup`) prompts for the key and **saves it to `~/.vibe/.env`**; that file is loaded automatically at startup. — [README §Quick Start / §API Key Configuration](https://github.com/mistralai/mistral-vibe); [docs: API keys and profiles](https://docs.mistral.ai/vibe/code/cli/api-keys-profiles)
- Browser-based sign-in is enabled by default for Mistral-provider models: the flow "provisions and stores credentials for you," so no manual API key is needed for typical setups. — [docs: API keys and profiles](https://docs.mistral.ai/vibe/code/cli/api-keys-profiles)
- Keys are created at [chat.mistral.ai/code/extensions](https://chat.mistral.ai/code/extensions); one key works across Free mode, paid plan, or pay-as-you-go. — [docs: API keys and profiles](https://docs.mistral.ai/vibe/code/cli/api-keys-profiles)
- MCP OAuth tokens are stored by Vibe; `vibe mcp remove <name>` deletes stored tokens, client info, and config fingerprint. — [README §MCP Server Configuration](https://github.com/mistralai/mistral-vibe)

---

## 2) Environment variables

| Variable | Effect |
|---|---|
| `MISTRAL_API_KEY` | Primary credential. Three supply paths, highest→lowest: shell env var → `~/.vibe/.env` → interactive setup prompt (which persists into `.env`). "Environment variables take precedence over values stored in `~/.vibe/.env`." — [docs: API keys and profiles](https://docs.mistral.ai/vibe/code/cli/api-keys-profiles); [README §API Key Configuration](https://github.com/mistralai/mistral-vibe) |
| `VIBE_HOME` | Redirects the entire Vibe home (`config.toml`, `.env`, `agents/`, `prompts/`, `skills/`, `tools/`, `logs/`, worktrees). — [README §Custom Vibe Home Directory](https://github.com/mistralai/mistral-vibe); [docs: Configuration](https://docs.mistral.ai/vibe/code/cli/configuration) |
| `LOG_LEVEL` | Overrides `log_level` in config at startup (session override > `LOG_LEVEL` > config > default `WARNING`). `DEBUG_MODE=true` forces DEBUG (also enables debugpy under `vibe-acp`). — [README §Logging](https://github.com/mistralai/mistral-vibe) |
| `SSL_CERT_FILE`, `SSL_CERT_DIR` | Extra TLS trust anchors alongside bundled certifi roots; opt into OS trust store with `enable_system_trust_store = true`. — [README §TLS and Corporate Certificate Authorities](https://github.com/mistralai/mistral-vibe) |
| `OTEL_EXPORTER_OTLP_*` | Auth/endpoint env vars for external OTLP collectors (endpoint itself set via `otel_endpoint`; Vibe appends `/v1/traces`). — [README §OpenTelemetry Tracing](https://github.com/mistralai/mistral-vibe) |

**Base URL overrides:** there is no documented `MISTRAL_BASE_URL`-style env var. Base URLs are overridden per-provider in `config.toml` instead:
- `[[providers]] api_base = "..."` — e.g. `api_base = "https://api.mistral.ai"` for the default Mistral provider ([docs: Admin config](https://docs.mistral.ai/vibe/code/cli/admin-config)), `"https://openrouter.ai/api/v1"` for OpenRouter ([docs: Configuration](https://docs.mistral.ai/vibe/code/cli/configuration)), or `"http://localhost:8080/v1"` for a local server ([docs: Using offline models](https://docs.mistral.ai/vibe/code/cli/offline-models)).
- For **browser sign-in against a Mistral-compatible deployment**, run `vibe --setup` → *Launch browser* → *Other*, enter your domain (auth base derived as `DOMAIN/api`); it persists as an overridden `mistral` provider. A pre-existing `browser_auth_base_url` in `config.toml` is read and pre-filled. "The credential is still a Mistral API key." — [README §Custom Domains](https://github.com/mistralai/mistral-vibe)

---

## 3) Providers & models

### Mistral platform API (la Plateforme)
- Default provider; browser sign-in provisions credentials automatically for Mistral-provider models, or use `MISTRAL_API_KEY` explicitly. — [docs: API keys and profiles](https://docs.mistral.ai/vibe/code/cli/api-keys-profiles)
- Built-in chat model IDs (use as `[[models]].name` or pass via `--model`): `mistral-medium-latest` (recommended flagship), `zai-glm-5-2`, `mistral-large-latest`, `mistral-small-latest`, `codestral-latest`, `ministral-14b-latest`, `ministral-8b-latest`, `ministral-3b-latest`; pin dated versions like `codestral-2508`. Switch models anytime with `/model` or `/config`. — [docs: Configuration §Providers and models](https://docs.mistral.ai/vibe/code/cli/configuration)

### Le Chat subscription auth
- Browser sign-in ties Vibe to your Mistral account: keys created from Code › Vibe CLI work "in Free mode, with a paid plan, or with pay-as-you-go enabled." Monthly included usage is shared across Studio, API, and Vibe Code; once exhausted, behavior depends on org settings (stop until next period, or bill pay-as-you-go if enabled — off by default; some partner-billed Pro plans cannot enable it). — [docs: API keys and profiles §Plan and billing notes](https://docs.mistral.ai/vibe/code/cli/api-keys-profiles)
- Related UX: `--worktree` branch naming "match[es] the worktrees Le Chat Desktop creates" (`vibe/<name>`), and sessions can be teleported between CLI and web. — [README §Working Directory Control](https://github.com/mistralai/mistral-vibe); [docs nav: Teleport from CLI to web](https://docs.mistral.ai/vibe/code/cli/teleport-cli-web)

### Custom OpenAI-compatible endpoints — yes, supported
Any OpenAI-style endpoint works via a generic provider preset:
```toml
active_model = "devstral-openrouter"

[[providers]]
name = "openrouter"
api_base = "https://openrouter.ai/api/v1"
api_key_env_var = "OPENROUTER_API_KEY"   # any env var name
api_style = "openai"
backend = "generic"

[[models]]
name = "mistralai/devstral-2512:free"
provider = "openrouter"
alias = "devstral-openrouter"
```
Enterprise gateways add `extra_headers`, region, and header overrides:
```toml
[[providers]]
name = "acme-enterprise"
backend = "mistral"
api_base = "https://mistral.acme.internal/v1"
api_key_env_var = "ACME_MISTRAL_API_KEY"
api_style = "openai"
region = "eu-west-1"
[providers.extra_headers]
"x-acme-tenant" = "engineering"
```
— [docs: API keys and profiles](https://docs.mistral.ai/vibe/code/cli/api-keys-profiles); [docs: Admin config](https://docs.mistral.ai/vibe/code/cli/admin-config)

Local/offline servers (vLLM recommended; also llama.cpp, LM Studio, Ollama):
```toml
[[providers]]
name = "local"
api_base = "http://localhost:8080/v1"    # 8080 is the CLI's assumed local port
api_style = "openai"
backend = "generic"

[[models]]
name = "mistralai/Devstral-Small-2-24B-Instruct-2512"
provider = "local"
alias = "devstral-local"

active_model = "devstral-local"
```
Serve with `vllm serve mistralai/Devstral-Small-2-24B-Instruct-2512 --tool-call-parser mistral --enable-auto-tool-choice --port 8080`. Recommended local models: Devstral Small 2 (dense 24B) or Mistral Small 4 (119B MoE). Fully-offline setups should also set `enable_telemetry = false` and `enable_auto_update = false`. — [docs: Using offline models](https://docs.mistral.ai/vibe/code/cli/offline-models)

### Devstral model selection
- Hosted: no `devstral-*` ID appears in the current hosted model table — select Devstral through third-party hosts (e.g. alias `devstral-openrouter` → `mistralai/devstral-2512:free` above). — [docs: Configuration](https://docs.mistral.ai/vibe/code/cli/configuration), [docs: API keys and profiles](https://docs.mistral.ai/vibe/code/cli/api-keys-profiles)
- Self-hosted: point a generic provider at your vLLM/llama.cpp/LM Studio/Ollama endpoint and set `active_model` to the alias; switch at runtime with `/config`. — [docs: Using offline models](https://docs.mistral.ai/vibe/code/cli/offline-models)

---

## 4) Multi-instance wrappers (parallel/isolated Vibe runs)

**Isolation mechanism:** `VIBE_HOME` is the single switch — it relocates `config.toml`, `.env` (credentials), logs, agents/prompts/skills/tools, worktree claims, and connector cache, so two instances get fully separate config + auth. Combine with `--workdir` (run against another project dir) or `--worktree NAME` (auto-managed git worktree under `$VIBE_HOME/worktrees/`) for workspace separation; `--add-dir` grants extra read dirs per session. Sessions are scoped per directory (`--continue`/`--resume` only see sessions from that dir/worktree). — [README §Custom Vibe Home Directory, §Session Management, §Working Directory Control](https://github.com/mistralai/mistral-vibe); [docs: Configuration](https://docs.mistral.ai/vibe/code/cli/configuration)

Wrapper script pattern:
```bash
#!/usr/bin/env bash
# vibe-instance.sh — isolated Vibe instance per profile
# usage: vibe-instance.sh <profile> [vibe args...]
set -euo pipefail
PROFILE="$1"; shift

case "$PROFILE" in
  work)      export VIBE_HOME="$HOME/.vibe-work";
             export MISTRAL_API_KEY="${WORK_MISTRAL_API_KEY:?unset}" ;;
  personal)  export VIBE_HOME="$HOME/.vibe-personal";
             export MISTRAL_API_KEY="${PERSONAL_MISTRAL_API_KEY:?unset}" ;;
  local)     export VIBE_HOME="$HOME/.vibe-local";
             unset MISTRAL_API_KEY ;;   # uses local provider preset (offline_models doc)
  *) echo "unknown profile: $PROFILE" >&2; exit 1 ;;
esac

mkdir -p "$VIBE_HOME"
exec vibe "$@"
```
Notes grounded in cited behavior:
- Env vars outrank `~/.vibe/.env` ([docs: API keys and profiles](https://docs.mistral.ai/vibe/code/cli/api-keys-profiles)), so exporting a profile-specific key cleanly overrides whatever each home's `.env` holds; omitting it falls back to that home's stored credential.
- Each `VIBE_HOME` needs its own trust decisions (`trusted_folders.toml`) and its own `vibe mcp add` state — nothing is shared between homes ([README §Trust Folder System, §MCP](https://github.com/mistralai/mistral-vibe)).
- For throwaway parallel coding tasks, prefer `--worktree` inside one home: names never collide across concurrent sessions and cleanup is ownership-tracked ([README §Worktree ownership](https://github.com/mistralai/mistral-vibe)).
- Programmatic/headless runs: `vibe --prompt ... [--agent plan] [--max-turns N|--max-price $|--max-tokens N] [--output json|streaming] [--trust]`; programmatic mode never prompts for trust and defaults to `auto-approve` without `--agent`. — [README §Programmatic Mode](https://github.com/mistralai/mistral-vibe); [docs: Safety §Trusted folders](https://docs.mistral.ai/vibe/code/safety-approvals-permissions)

---

## 5) MCP, tools, hooks, and sandbox/approval configuration

### MCP servers
Declared under `[[mcp_servers]]` in `config.toml` (or added via CLI / slash command):
```toml
[[mcp_servers]]
name = "my_http_server"                 # tool prefix: {server_name}_{tool_name}
transport = "http"                      # http | streamable-http | stdio
url = "http://localhost:8000"
startup_timeout_sec = 15                # default 10
tool_timeout_sec = 120                  # default 60

[mcp_servers.auth]
type = "static"
headers = { "X-Client" = "vibe" }
api_key_env = "MY_API_KEY_ENV_VAR"      # env var holding the key
api_key_header = "Authorization"
api_key_format = "Bearer {token}"

[[mcp_servers]]
name = "fetch_server"
transport = "stdio"
command = "uvx"
args = ["mcp-server-fetch"]
env = { "DEBUG" = "1", "LOG_LEVEL" = "info" }
```
- CLI management: `vibe mcp add <name> --url … --transport streamable-http [--api-key-env VAR | --header …]`, `vibe mcp remove <name>`; static auth is selected when `--api-key-env`/`--header` is given, otherwise OAuth browser login starts by default (`--no-login` to skip). In-session: `/mcp` (alias `/connectors`) to browse, `/mcp add <url> [--name X --scope read --transport http --no-login]` (OAuth-only shortcut). Legacy top-level `api_key_env`/`headers` keys are still accepted and promoted into the `auth` block. — [README §MCP Server Configuration](https://github.com/mistralai/mistral-vibe)
- ⚠️ Version skew between sources: the official docs page states "**the CLI does not yet support MCP servers that require OAuth authentication** — use `stdio` or `http` transport with an API key or other static credential" ([docs: MCP servers](https://docs.mistral.ai/vibe/code/cli/mcp-servers)), while the GitHub README documents full OAuth flows (`auth.type = "oauth"` with `scopes`). Trust the docs caveat for older installed versions; the README reflects newer builds.
- MCP tools are filtered/permissioned like built-ins: `enabled_tools`/`disabled_tools` (exact, glob, `re:` regex; underscores not dots, e.g. `serena_list`) and `[tools.<server>_<tool>] permission = "always" | "ask"`. — [docs: MCP servers §Tool naming and permissions](https://docs.mistral.ai/vibe/code/cli/mcp-servers)

### Tools & permissions
- Global filters: `enabled_tools` narrows first, then `disabled_tools` removes. — [README §Tool Management](https://github.com/mistralai/mistral-vibe)
- Per-tool approvals: `[tools.read_file] permission = "always"` / `[tools.bash] permission = "ask"`; bash additionally supports command `allow`/`deny` glob lists (safe commands like `ls`, `pwd` auto-allowed by default). Anything touching outside the cwd prompts regardless of agent. — [docs: Safety, approvals, and permissions](https://docs.mistral.ai/vibe/code/safety-approvals-permissions)
- Shell surface: managed shell tools (`bash`, `bash_output`, `bash_stdin`, `bash_sessions`, `bash_log_file`; Windows `git_bash`/`powershell` variants) controlled by `managed_shell_tools_enabled` + server-managed rollout; sessions log under `~/.vibe/shell-tool/sessions/`. — [README §Tool Management](https://github.com/mistralai/mistral-vibe)
- Agent profiles bundle prompt + tools + approvals: `ask`/`default` (approve everything), `plan` (read-only), `accept-edits` (auto-approves edits; interactive default), `auto-approve` (everything). Select with `--agent` or `Shift+Tab`; persist with `default_agent`. Custom agents: TOML files in `~/.vibe/agents/` or `.vibe/agents/` (e.g. `redteam.toml` setting `active_model`, `system_prompt_id`, `disabled_tools`, `[tools.bash] permission`). — [README §Built-in Agents, §Custom Agent Configurations](https://github.com/mistralai/mistral-vibe); [docs: Safety §Agents](https://docs.mistral.ai/vibe/code/safety-approvals-permissions) *(note: README calls the strictest built-in `ask`, the docs safety page calls it `default`)*

### Hooks (programmatic gating/auditing)
Declared in `hooks.toml` (project-first, trusted only): each hook gets JSON-on-stdin (session context + `tool_input` etc.) and replies via exit code/stdout JSON (`decision: allow|deny`, `reason`, `hook_specific_output.tool_input` rewrite for `pre_tool`, `additional_context` append for `post_tool`); `match` supports fnmatch globs and `re:` regex; `strict = true` turns hook failures into denials; subagents inherit hook config. — [README §Hooks](https://github.com/mistralai/mistral-vibe)

### Sandbox posture
There is no OS-sandbox/container subsystem in the documented config — containment is layered through the trust + approval model above: trusted-folder gating of project `.vibe/` content (`~/.vibe/trusted_folders.toml`; `vibe --trust` for one-shot programmatic trust), per-tool permissions, bash deny-lists, hooks, and agents. Mistral's own guidance: reserve `auto-approve` "for disposable environments (containers, CI runners, ephemeral VMs)" — i.e., bring your own sandbox for unsupervised runs. — [docs: Safety §Trusted folders, §Best practices](https://docs.mistral.ai/vibe/code/safety-approvals-permissions); [README §Trust Folder System](https://github.com/mistralai/mistral-vibe)

### Telemetry/tracing toggles relevant to all of the above
`enable_telemetry` (default true; anonymous usage/errors only), `enable_otel` + `otel_endpoint` + `otel_redaction` for exporting agent/model/tool traces, `enable_update_checks`/`enable_auto_update`, `enable_notifications`. — [README §§Update/Notification/OpenTelemetry](https://github.com/mistralai/mistral-vibe); [docs: Configuration](https://docs.mistral.ai/vibe/code/cli/configuration)
