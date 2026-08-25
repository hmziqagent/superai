# gptme — Configurable Options Reference

> Compiled 2026-08-25 from official docs: [gptme.org/docs](https://gptme.org/docs/) (config, providers, usage, tools, server, cli pages) and cross-checked against search snippets of the same pages. Inline citations point at the exact section anchors.

---

## 1. Configuration files & the workspace concept

gptme has **three** configuration files plus environment variables (env vars take precedence over files; CLI flags take precedence over env vars — [config.html#environment-variables](https://gptme.org/docs/config.html#environment-variables)):

| File | Path | Scope |
|---|---|---|
| Global | `~/.config/gptme/config.toml` (or `$XDG_CONFIG_HOME/gptme/`) | user-wide |
| Global secrets overlay | `~/.config/gptme/config.local.toml` | merged into main config |
| Project | `gptme.toml` in **workspace root** | per-project |
| Project secrets overlay | `gptme.local.toml` next to `gptme.toml` | merged into project config |
| Chat | `~/.local/share/gptme/logs/<conversation>/config.toml` | per-conversation |

**Naming note:** the *global* file is `config.toml`; the name `gptme.toml` is used for the *project-local* file. Secrets belong in `config.local.toml`, never `config.toml`, so the latter can be dotfiled/versioned ([config.html#global-config](https://gptme.org/docs/config.html#global-config), [#local-overrides-config-local](https://gptme.org/docs/config.html#local-overrides-config-local)).

### 1.1 Global `~/.config/gptme/config.toml` schema

Annotated example (from the official example config — [config.html#global-config](https://gptme.org/docs/config.html#global-config)):

```toml
[user]
name = "Erik"                       # shown at CLI prompt / web UI tooltip (default "User")
about = "I am a curious human programmer."   # injected into system prompt
response_preference = "Basic concepts don't need to be explained."
avatar = "~/Pictures/avatar.jpg"    # ~ expansion or URL

[prompt]
files = ["~/notes/llm-tips.md"]     # extra files always included in context
#[prompt.project]                   # project descriptions keyed by repo name,
#myproject = "..."                  # injected when git root name matches

[[hooks.scripts]]                   # global lifecycle hooks (see §5)
event = "session.end"
command = "~/bin/save-agent-context"
timeout = 30
priority = 10

[env]
#MODEL = "anthropic/claude-sonnet-4-6"      # default model (fallback env layer)
OPENAI_API_KEY = ""                          # one or more provider keys
ANTHROPIC_API_KEY = ""
OPENROUTER_API_KEY = ""
XAI_API_KEY = ""
GEMINI_API_KEY = ""
GROQ_API_KEY = ""
DEEPSEEK_API_KEY = ""
#MODEL = "local/<model-name>"                # Ollama-style local setup
#OPENAI_BASE_URL = "http://localhost:11434/v1"
#TOOL_FORMAT = "markdown"                    # markdown | xml | tool
#TOOL_ALLOWLIST = "save,append,patch,ipython,shell,browser"
#TOOL_MODULES = "gptme.tools,custom.tools"   # extra Python tool modules

[models]
#default = "anthropic/claude-sonnet-4-6"     # formal alternative to MODEL env
#favorites = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"]

[settings]
gear = 2   # default autonomy preset for new conversations (0–4)

# [mcp] — MCP server config, see https://gptme.org/docs/mcp.html
```

Key semantics ([config.html#global-config](https://gptme.org/docs/config.html#global-config)):
- `[env]` holds **fallback** env values: used only when the variable isn't set in the shell. Shell env beats it.
- `[models].default` beats the `MODEL` env var (incl. `[env].MODEL`) but loses to an explicit `--model` flag or per-chat model saved via `/model`. Don't set `MODEL` twice — TOML forbids duplicate keys in a table.
- Backward-compat: `about_user` / `response_preference` under `[prompt]` still work if absent from `[user]`.
- Merging of `config.local.toml`: dicts merge recursively, MCP servers merge by `name`, scalars override.

### 1.2 Project-local `gptme.toml`

Looked up in the **workspace root** (cwd unless overridden by `--workspace`). Documented keys ([config.html#project-config](https://gptme.org/docs/config.html#project-config)):

```toml
files = ["README.md", "Makefile"]        # always included in context
prompt = "This is gptme."                # added under "# Current Project" header
base_prompt = "You are ..."              # replaces the global base system prompt
context_cmd = 'my-retriever --query-env GPTME_PROMPT_INITIAL'  # cmd run in workspace root;
                                         # output injected into system prompt. The initial
                                         # user prompt is exposed via $GPTME_PROMPT_INITIAL
                                         # (unset if too large / absent).

[[hooks.scripts]]                        # same schema as global hooks; lists are additive
event = "session.end"
command = "scripts/save-context.sh"
timeout = 30
priority = 10

[settings]
gear = 3                                 # overrides global settings.gear in this workspace

[rag]                                    # RAG tool config (see tools docs)
[plugins]
paths = ["./plugins", "~/.config/gptme/plugins"]
enabled = ["my_project_plugin"]

[agent]
name = "Bob"                             # agent identity for autonomous agents (gptme-bob)
avatar = "assets/avatar.png"             # relative to workspace, or URL

[env]                                    # project env; beats global [env], loses to shell

[mcp]                                    # project MCP servers
```

Behavior notes:
- With no `gptme.toml` (or no `files` key), gptme auto-includes common project files: `README.md`, `pyproject.toml`, `package.json`, `Cargo.toml`, `Makefile`, `.cursor/rules/**.mdc`, `CLAUDE.md`, `GEMINI.md` ([config.html#project-config](https://gptme.org/docs/config.html#project-config)).
- Hook/context commands run with shell interpretation — **review `gptme.toml` before running gptme in untrusted repos** ([security warning](https://gptme.org/docs/config.html#project-config), [docs/security.html](https://gptme.org/docs/security.html)).
- Hook commands receive `GPTME_HOOK_EVENT`, `GPTME_LOGDIR`, `GPTME_WORKSPACE`, `GPTME_MODEL` in their environment.
- Add `gptme.local.toml` to `.gitignore` for uncommitted secrets/personal overrides.

### 1.3 Workspace directory concept

- The **workspace** is the directory the agent treats as its working directory — where `gptme.toml` is discovered, where tools operate, and where project context comes from ([config.html#project-config](https://gptme.org/docs/config.html#project-config), [server.html#workspace-patch-semantics](https://gptme.org/docs/server.html#workspace-patch-semantics)).
- Default = current working directory. Override per run with `-w/--workspace <path>`; the literal value `@log` points the workspace at the conversation's log directory ([cli.html#gptme](https://gptme.org/docs/cli.html#gptme)).
- Equivalent env var: `GPTME_WORKSPACE` (= `--workspace`) ([config.html#environment-variables](https://gptme.org/docs/config.html#environment-variables)).
- `--no-workspace` skips all workspace context (prompt files + `context_cmd`) while keeping core prompt and tools ([usage.html#minimal-context-mode](https://gptme.org/docs/usage.html#minimal-context-mode)).
- `--agent-path <path>` sets a separate *agent* workspace directory ([cli.html#gptme](https://gptme.org/docs/cli.html#gptme)).
- On the server API: conversation creation (`PUT`) may set any workspace; `PATCH .../config` refuses to redirect an existing conversation's workspace outside its log dir (confused-deputy protection) ([server.html#workspace-patch-semantics](https://gptme.org/docs/server.html#workspace-patch-semantics)).

---

## 2. Environment variables

Precedence: **CLI args > env vars > config file values** ([config.html#environment-variables](https://gptme.org/docs/config.html#environment-variables)). All booleans accept `1`/`true` (case-insensitive).

### Provider API keys

`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, `GEMINI_API_KEY`, `GROQ_API_KEY`, `XAI_API_KEY`, `DEEPSEEK_API_KEY`, `MOONSHOT_API_KEY`, `AZURE_OPENAI_API_KEY`, plus `REQUESTY_API_KEY` (Requesty gateway) ([providers.html#configuring-credentials](https://gptme.org/docs/providers.html#configuring-credentials), [config.html#how-model-selection-works](https://gptme.org/docs/config.html#how-model-selection-works)).

If **no model** is configured, gptme scans these keys and auto-picks the first available provider in order: `openai → anthropic → openrouter → gemini → groq → xai → deepseek → moonshot → azure`.

Subscription/cloud credentials: OAuth tokens land in `~/.config/gptme/oauth/openai_subscription.json` and `.../grok_subscription.json`; managed-service token in `~/.config/gptme/auth/gptme-cloud-<hash>.json`; manual `/account setup` keys in `~/.config/gptme/credentials.toml` ([providers.html](https://gptme.org/docs/providers.html)).

Cloud managed-service extras: `GPTME_CLOUD_API_KEY`, `GPTME_CLOUD_BASE_URL` (default `https://fleet.gptme.ai/v1`) ([providers.html#gptme-managed-service](https://gptme.org/docs/providers.html#gptme-managed-service)).

### `GPTME_*` feature flags ([config.html#environment-variables](https://gptme.org/docs/config.html#environment-variables))

| Var | Purpose / default |
|---|---|
| `GPTME_MODEL` | model, equivalent to `--model` |
| `GPTME_WORKSPACE` | workspace, equivalent to `--workspace` |
| `GPTME_TOOL_FORMAT` | equivalent to `--tool-format` |
| `GPTME_TOOL_ALLOWLIST` | allowed tools, equivalent to `--tools` |
| `GPTME_CHECK` | pre-commit checks after edits (default true if `.pre-commit-config.yaml` present) |
| `GPTME_CHAT_HISTORY` | cross-conversation context: injects summaries of the 3 most recent substantial chats into new sessions (default false) |
| `GPTME_COSTS` | cost reporting for API calls (default false) |
| `GPTME_SESSION_BUDGET_USD` / `GPTME_SESSION_BUDGET_TOKENS` | per-session cost/token budget; warns past threshold |
| `GPTME_BUDGET_WARN_PCT` | warn percentage of budget (default 80) |
| `GPTME_FRESH` | fresh-context mode (default false) |
| `GPTME_BREAK_ON_TOOLUSE` | interrupt generation at tool use; model-dependent default; `0`=force parallel calls, `1`=one call per response (= `--multi-tool`) |
| `GPTME_PATCH_RECOVERY` | return file content on failed patches (default false) |
| `GPTME_SUGGEST_LLM` | LLM-powered prompt completion (default false) |
| `LLM_API_TIMEOUT` | LLM request timeout seconds (default 600); raise for slow local models |
| `GPTME_ANTHROPIC_FAST_MODE` | Anthropic `speed:"fast"` research preview (Opus 4.8+, premium pricing) |
| `GPTME_BROWSER_CDP_URL` | attach Playwright backend to existing Chromium over CDP, e.g. `http://127.0.0.1:9222` |
| `GPTME_LOGS_HOME` | override default conversation-log folder location |
| `GPTME_INJECTION_HYGIENE` | default for `--injection-hygiene`: off/warn/block |
| `GPTME_MANIFEST_DIR` | default for `--manifest-dir` (tool-call audit records) |
| `GPTME_OPENAI_RESPONSES_API` | `0/false/no/off` force legacy chat-completions for direct `openai/*` models (default: GPT-5/o-series route via Responses API) |

Rule of thumb: **every CLI option can also be set as `GPTME_<PARAMETER_NAME_UPPERCASE>`** ([config.html#environment-variables](https://gptme.org/docs/config.html#environment-variables)).

Provider-specific: `OPENROUTER_DATA_COLLECTION` (default `"deny"`; set `"allow"` for providers requiring consent), `OPENROUTER_QUANTIZATION` (e.g. `"fp16,bf16"`; common values fp16/bf16/fp8/int8/int4/unknown) ([providers.html#openrouter](https://gptme.org/docs/providers.html#openrouter)).

Shell-tool tuning: `GPTME_SHELL_TIMEOUT` (seconds; 0 disables; unset→1200s; invalid→1200s), `GPTME_SHELL_MEMORY_LIMIT` (POSIX `ulimit -v`, e.g. `512M`), `GPTME_SHELL_TRUNC_PRE_TOKENS`/`POST` (stdout truncation, defaults 2000/8000) and `_STDERR_PRE_`/`_STDERR_POST_` (2000/2000) ([tools.html#shell](https://gptme.org/docs/tools.html#shell)).

Server vars: `GPTME_SERVER_TOKEN`, `GPTME_DISABLE_AUTH`, `GPTME_SERVER_ALLOWED_HOSTS`, `GPTME_SERVER_HOST`, `GPTME_SERVER_PORT`, `GPTME_WEBUI_DIR`, `GPTME_SERVER_DEFAULT_PROFILE` ([cli.html#gptme-server-serve](https://gptme.org/docs/cli.html#gptme-server-serve), [server.html#security](https://gptme.org/docs/server.html#security)).

---

## 3. Providers & models

### Selection syntax

Always `<provider>/<model>`; bare provider uses that provider's default ([providers.html#selecting-a-provider-and-model](https://gptme.org/docs/providers.html#selecting-a-provider-and-model)):

```
gptme "hello" -m openai/gpt-5.5
gptme "hello" -m anthropic                     # provider default
gptme "hello" -m openrouter/x-ai/grok-4        # nested vendor path under openrouter
gptme "hello" -m openrouter/deepseek/deepseek-v4-pro
gptme "hello" -m deepseek/deepseek-reasoner
gptme "hello" -m gemini/gemini-2.5-flash
gptme "hello" -m groq/llama-3.3-70b-versatile
gptme "hello" -m xai/grok-4
gptme "hello" -m openai-subscription/gpt-5.5-pro[:low|medium|high|xhigh]
gptme "hello" -m grok-subscription/grok-4.6
gptme "hello" -m local/llama3.2:1b
gptme "hello" -m gptme/claude-sonnet-4-6       # gptme.ai managed gateway
gptme "hello" -m requesty/openai/gpt-4o-mini   # Requesty gateway, OpenRouter-style naming
```

List known models: `gptme '/models' - '/exit'` or `gptme models list` ([providers.html](https://gptme.org/docs/providers.html), [cli.html](https://gptme.org/docs/cli.html)). The `<provider>` prefix decides which API key is used.

OpenRouter extras: pin a backend with `model@provider` (e.g. `anthropic/claude-sonnet-4-20250514@anthropic`); defaults enforce `require_parameters` and `data_collection:"deny"` ([providers.html#openrouter](https://gptme.org/docs/providers.html#openrouter)).

### Local models (Ollama, llama.cpp, LM Studio, any OpenAI-compatible server)

Set `MODEL=local/<model-name>` and aim `OPENAI_BASE_URL` at the server ([config.html#global-config](https://gptme.org/docs/config.html#global-config), [providers.html#local](https://gptme.org/docs/providers.html#local)):

```bash
ollama pull llama3.2:1b && ollama serve
OPENAI_BASE_URL="http://127.0.0.1:11434/v1" gptme 'hello' -m local/llama3.2:1b
```

Critical scoping rule: **`OPENAI_BASE_URL` applies ONLY to `local/`-prefixed models** — it does not affect OpenAI/Anthropic/etc. Also: pointing `OPENAI_BASE_URL` at Groq (`https://api.groq.com/openai/v1`) with `OPENAI_API_KEY` returns **401** — use `groq/<model>` + `GROQ_API_KEY` instead ([providers.html#groq](https://gptme.org/docs/providers.html#groq)). Small local models handle tools poorly (see evals page).

### Custom base URLs / custom providers

- Interactive: `gptme-util providers add` prompts for name/base URL/API key/default model and writes a `[[providers]]` entry into `gptme.toml`; works for Ollama `:11434`, LM Studio `:1234`, vLLM, any OpenAI-compatible relay ([cli.html#gptme-util-providers-add](https://gptme.org/docs/cli.html#gptme-util-providers-add)).
- `gptme-util providers list [--discover]` probes well-known local endpoints without writing config; `providers test NAME` checks connectivity.
- Programmatic: third-party packages register providers via the `gptme.providers` entry-point group (`ProviderPlugin(name, api_key_env, base_url, models, init)`), usable right after `pip install` as `minimax/MiniMax-M3` style prefixes ([providers.html#provider-plugins-entry-points](https://gptme.org/docs/providers.html#provider-plugins-entry-points)); configurable custom providers doc: [providers-custom.html](https://gptme.org/docs/providers-custom.html).

### Model resolution priority ([config.html#how-model-selection-works](https://gptme.org/docs/config.html#how-model-selection-works))

1. `--model`/`-m` CLI flag → 2. per-chat model (`/model`) → 3. `[models].default` → 4. `MODEL` env (shell or `[env]`) → 5. auto-detect from present API keys (order above).

---

## 4. Multi-instance wrappers (parallel agents with separate identities)

Everything needed for isolated concurrent instances is runtime-flag based — no profile system required:

**Levers**
- **Workspace separation:** `-w/--workspace <dir>` per instance (or `GPTME_WORKSPACE`). Each instance reads its own `gptme.toml` from that root, so project-level `[env]`, tools, hooks, and `settings.gear` diverge cleanly ([config.html#project-config](https://gptme.org/docs/config.html#project-config), [cli.html#gptme](https://gptme.org/docs/cli.html#gptme)). Use distinct dirs — not just distinct `--name`s — if you want independent `gptme.toml` behavior.
- **Model runtime override:** `-m/--model <provider/model>` has top precedence over every config-layer default ([config.html#how-model-selection-works](https://gptme.org/docs/config.html#how-model-selection-works)).
- **Conversation separation:** `--name <id>` names/resumes a conversation; omit for random names. Queue work across instances with `gptme-util chats send ID MSG` ([usage.html#managing-conversations](https://gptme.org/docs/usage.html#managing-conversations)).
- **Useful batch flags:** `-n/--non-interactive` (implies `-y`), `--output-format json` (JSONL stdout), `-t/--tools`, `--tool-format`, `--system short`, `--no-workspace`, `--gear N` ([cli.html#gptme](https://gptme.org/docs/cli.html#gptme)).
- Keys can come from the shared global `[env]`; per-instance divergence via exported shell env (highest env precedence) or per-workspace `gptme.toml` `[env]`.

**Wrapper script example — two providers, two workspaces, non-interactive:**

```bash
#!/usr/bin/env bash
# gp.sh — run gptme instances pinned to different providers/workspaces
set -euo pipefail

# Instance A: OpenRouter-backed planner (read-only-ish toolset)
gpt() {
  local ws="$1"; shift
  gptme \
    --workspace "$ws" \
    --name "$(basename "$ws")-openrouter" \
    --non-interactive "$@"
}
gp() {  # planner: cheap/fast OpenRouter model
  OPENROUTER_API_KEY="${OPENROUTER_API_KEY:?}" \
  gpt ~/agents/planner \
    --model openrouter/deepseek/deepseek-v4-pro \
    --tool-format markdown \
    -t "read,shell,hint:read-only" \
    "$@"
}

ga() {  # implementer: Anthropic Sonnet, full tools
  ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:?}" \
  gpt ~/agents/implementer \
    --model anthropic \
    --tool-format markdown \
    -y \
    "$@"
}

"$@"   # usage: ./gp.sh "draft refactor plan" ;  ./ga.sh "implement step 1"
```

Notes: because `-m` outranks all config, the same machine/global config serves both providers; isolation comes purely from `--workspace` + `--name`. For interactive side-by-side use drop `-n`. `GPTME_MODEL`/`GPTME_WORKSPACE` env exports work equally well inside each function body ([config.html#environment-variables](https://gptme.org/docs/config.html#environment-variables)).

---

## 5. Tools configuration, hooks, server mode

### Tool selection & allowlist ([tools.html#tool-selection-allowlists](https://gptme.org/docs/tools.html#tool-selection-allowlists))

```bash
gptme --tools save,patch,shell,python "refactor"     # exact set replaces defaults
gptme --tools +rag,browser "research X"              # additive to defaults
gptme --tools -shell,computer "safer mode"           # subtractive from defaults
gptme --tools "" "just talk"                         # disable all tools ('none' also works)
gptme --tools read-only "audit this repo"            # named preset: built-in `read` only
gptme --tools "hint:read-only" "summarise"           # category via capability hints
```
- Glob patterns supported (`fnmatch`); hints: `read-only`, `destructive`, `idempotent`, `closed-world`; MCP tool annotations map onto these hints automatically.
- Config-side equivalents: `TOOL_ALLOWLIST` env (or `[env]` entry) and `TOOL_MODULES` for custom Python tool modules ([config.html#global-config](https://gptme.org/docs/config.html#global-config)). Custom single-file tools: `-t path/to/tool.py` ([cli.html#gptme](https://gptme.org/docs/cli.html#gptme)).
- Tool wire format: `--tool-format markdown|xml|tool` (`markdown` = default programmatic tool calling via fenced code blocks; `tool` = provider-native JSON-schema calling) ([tools.html#tool-interface-architecture](https://gptme.org/docs/tools.html#tool-interface-architecture), [tool-formats.html](https://gptme.org/docs/tool-formats.html)).

### Notable built-ins ([tools.html](https://gptme.org/docs/tools.html))
- **Shell**: stateful bash; background jobs (`bg/jobs/output/kill`); tuned by `GPTME_SHELL_TIMEOUT`, `GPTME_SHELL_MEMORY_LIMIT`, `GPTME_SHELL_TRUNC_*`.
- **Browser**: needs `[browser]` extra; reads URLs, screenshots pages; attach-to-existing-browser via `GPTME_BROWSER_CDP_URL`; guide at [browser.html](https://gptme.org/docs/browser.html).
- **Vision / Screenshot / Computer**: image analysis, screen capture, desktop control (`--tools +computer`; Linux/X11/xdotool locally, experimental).
- **Save / Patch / Morph / Hashline Edit**: file create & edit tools.
- **RAG**: configured via `gptme.toml [rag]`; index with `gptme context index` ([config.html#project-config](https://gptme.org/docs/config.html#project-config), [cli.html](https://gptme.org/docs/cli.html)).
- **Autocommit**: prompts for git commits after file modifications; **Precommit**: runs pre-commit hooks post-save (toggle `GPTME_CHECK`).

### Hooks ([config.html#project-config](https://gptme.org/docs/config.html#project-config))
- Declared as `[[hooks.scripts]]` in global **and/or** project config; lists are additive. Events (initial allowlist): `session.start`, `session.end`. Fields: `event`, `command`, `timeout` (s), `priority` (desc order; default 0; global before project at ties).
- Run synchronously in the workspace; env includes `GPTME_HOOK_EVENT`, `GPTME_LOGDIR`, `GPTME_WORKSPACE`, `GPTME_MODEL`; failures/timeouts are logged, never break the session. ⚠️ Shell-interpreted — inspect `gptme.toml` in untrusted repos.
- Richer programmatic hooks/plugins exist via the plugin system ([plugins.html](https://gptme.org/docs/plugins.html)).

### Server mode ([server.html](https://gptme.org/docs/server.html), [cli.html#gptme-server-serve](https://gptme.org/docs/cli.html#gptme-server-serve))
- Install `pipx install 'gptme[server]'`; run `gptme-server` → UI at `http://localhost:5700` (webui bundled, same origin).
- `gptme-server serve` options: `--host/--port` (env `GPTME_SERVER_HOST/_PORT`), `--model` (default model, overridable per request), `--tools` (comma list or `none`), `--cors-origin` (for separately hosted UI), `--allowed-hosts`, `--webui-dir`, `--default-profile` (e.g. `computer-use`, `browser-use`), `--exit-on-parent-death`, `--watch-pid`.
- Auth: bearer token always required on capability routes; `GPTME_SERVER_TOKEN` pins it (else generated & printed; `gptme-server token` displays it). `GPTME_DISABLE_AUTH` only behind an authenticated ingress. ⚠️ Any token holder can execute arbitrary shell through the agent — the boundary is server access, not endpoints.
- Deployments: docker-compose at repo root; nginx TLS reverse proxy recipe (disable proxy buffering for SSE!); systemd unit template `scripts/gptme-server.service`.

---

## 6. Git-backed memory / logs locations

- **Conversation logs:** every conversation persists under `~/.local/share/gptme/logs/<YYYY-MM-DD-descriptive-name>/`, including a per-chat `config.toml` storing model, toolset, tool format, streaming mode — regenerated/applied on resume ([config.html#chat-config](https://gptme.org/docs/config.html#chat-config)). Relocate with `GPTME_LOGS_HOME` ([config.html#environment-variables](https://gptme.org/docs/config.html#environment-variables)). Hooks/scripts receive the active log dir as `GPTME_LOGDIR` ([config.html#project-config](https://gptme.org/docs/config.html#project-config)), and `-w @log` makes the log dir itself the workspace ([cli.html#gptme](https://gptme.org/docs/cli.html#gptme)).
- **Working with history:** `gptme chats list [--metadata|--json]`, `gptme chats search "query"` (alias `gptme search`), fork with `gptme-util chats fork NAME --at-turn N`, queue prompts with `chats send` ([usage.html#managing-conversations](https://gptme.org/docs/usage.html#managing-conversations), [cli.html](https://gptme.org/docs/cli.html)).
- **Git integration:** `/checkpoint` rolls the workspace back to last *committed* git state, `/snapshot` captures any state ([usage.html#managing-conversations note](https://gptme.org/docs/usage.html#managing-conversations)); the **autocommit** tool prompts for commits after edits and **precommit** runs `.pre-commit-config.yaml` checks after saves ([tools.html](https://gptme.org/docs/tools.html), [usage.html#pre-commit-integration](https://gptme.org/docs/usage.html#pre-commit-integration)).
- **Cross-session memory:** `GPTME_CHAT_HISTORY=true` injects summaries of recent substantial conversations into new sessions ([config.html#cross-conversation-context](https://gptme.org/docs/config.html#cross-conversation-context)); lessons/skills are discoverable per workspace via `gptme skills list/show` ([cli.html](https://gptme.org/docs/cli.html)).
- Practical pattern: since chat logs are plain directories of JSONL + TOML, pointing `GPTME_LOGS_HOME` (or the whole `~/.local/share/gptme`) into a git repo gives versioned, diffable memory; hook `session.end` scripts receiving `GPTME_LOGDIR` are the documented place to commit them.
