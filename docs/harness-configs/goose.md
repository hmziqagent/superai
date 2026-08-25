# Goose — Configurable Options Reference

> **Project status:** Goose originated at Block and moved to the **Agentic AI Foundation (AAIF)** (announced 2026-04-07; repo now `aaif-goose/goose`). Docs moved from `block.github.io/goose` to **`goose-docs.ai`** — all old `/docs/guides/config-files-and-env-vars` and `/docs/guides/provider/configuration` URLs now 404. This doc was compiled 2026-08-25.
>
> **Primary sources** (fetched & verified):
> - **[EV]** Environment Variables guide — https://goose-docs.ai/docs/guides/environment-variables/ (full page captured)
> - Search results corroborating: GitHub issue [aaif-goose/goose#4036](https://github.com/aaif-goose/goose/issues/4036) (multi-provider/lead-worker consolidation proposal), [agent-safehouse.dev Goose sandbox report](https://agent-safehouse.dev/docs/agent-investigations/goose) (§10 confirms `GOOSE_PROVIDER/MODEL/MODE/GOOSE_LEAD_*` are the live runtime vars), GitCode mirror of `documentation/docs/guides/environment-variables.md` (confirms a dedicated "Lead/Worker Model Configuration" section exists upstream).
>
> Items marked ⚠️ *unverified* come from prior knowledge of the codebase/community docs and could not be re-fetched within this session's budget — double-check against `goose-docs.ai` before relying on exact spellings.

---

## 1. `config.yaml` schema

### Where config lives

| Platform | Path |
|---|---|
| Linux | `~/.config/goose/config.yaml` (XDG base-dir convention) |
| macOS | `~/Library/Application Support/Block/goose/config/` |
| Windows | `%APPDATA%\Block\goose\config\` |

Secrets (`secrets.yaml`) sit next to `config.yaml`: `~/.config/goose/secrets.yaml` (Linux) / `%APPDATA%\Block\goose\config\secrets.yaml` (Windows) when the system keyring is disabled or unavailable (**[EV]** Security & Privacy tip box). By default goose prefers the OS **keyring** for API keys; `GOOSE_DISABLE_KEYRING=1` forces file storage (**[EV]**).

**Changing the location:** set `GOOSE_PATH_ROOT` — overrides the root for *all* data, config, and state; goose then creates `config/`, `data/`, and `state/` subdirectories under it. Explicitly recommended for "isolating test environments, running multiple configurations, or CI/CD pipelines," e.g. `GOOSE_PATH_ROOT="/tmp/goose-isolated" goose run --recipe my-recipe.yaml` (**[EV]** Development & Testing). On Linux, honoring `XDG_CONFIG_HOME` follows from standard XDG dir resolution ⚠️ *unverified as an officially documented knob*. There is no separate `GOOSE_CONFIG_HOME`; `GOOSE_PATH_ROOT` is the documented relocation mechanism.

### Core keys (top-level)

These keys are managed interactively by `goose configure` and written to `config.yaml`; every one can be overridden by an environment variable of the same name (**[EV]** Notes: "Environment variables take precedence over configuration files").

```yaml
# --- Model / provider -------------------------------------------------
GOOSE_PROVIDER: anthropic          # LLM backend (see §3)
GOOSE_MODEL: claude-sonnet-4-5     # model name for that provider
GOOSE_FAST_MODEL: gpt-4o-mini      # aux calls: tool-selection, classification, session titles
GOOSE_TEMPERATURE: 0.7             # 0.0–1.0 float
GOOSE_MAX_TOKENS: 8192             # per-response cap
# --- Behavior ---------------------------------------------------------
GOOSE_MODE: approve                # approve | auto | chat | smart_approve
GOOSE_CLI_MIN_PRIORITY: 0.0        # 0.0–1.0; filters tool-output verbosity in CLI
PERIODIC_HINTS: true               # ⚠️ unverified exact casing — periodic tips during long runs
# --- Lead/Worker (see below) ------------------------------------------
GOOSE_LEAD_PROVIDER: anthropic
GOOSE_LEAD_MODEL: claude-opus-4-5
# --- Planner ----------------------------------------------------------
GOOSE_PLANNER_PROVIDER: openai     # falls back to GOOSE_PROVIDER
GOOSE_PLANNER_MODEL: gpt-4         # falls back to GOOSE_MODEL
# --- Extensions -------------------------------------------------------
enabled_extensions:                # bundled/built-in extensions to activate
  - developer
  - computercontroller             # ⚠️ example set; actual list is whatever you ticked in `goose configure`
extensions: {}                     # user-added MCP servers (see §5 for schema)
```

Key semantics, all from **[EV]** unless noted:

| Key / env twin | Purpose | Values | Default |
|---|---|---|---|
| `GOOSE_PROVIDER` | Which LLM provider | see provider list, §3 | none (must configure) |
| `GOOSE_MODEL` | Model within the provider | e.g. `gpt-4`, `claude-sonnet-4-20250514` | none |
| `GOOSE_MODE` | Tool-execution handling | `auto`, `approve`, `chat`, **`smart_approve`** | `auto` |
| `GOOSE_FAST_MODEL` | Overrides provider's default fast model for auxiliary calls (tool-selection, classification, session titles) | model name | provider default |
| `GOOSE_TEMPERATURE` | Sampling temperature | 0.0–1.0 | model default |
| `GOOSE_MAX_TOKENS` | Max tokens per response | positive int | model default |
| `GOOSE_CLI_MIN_PRIORITY` | Verbosity filter for tool output in CLI | float 0.0–1.0 (`0.2` = only medium+importance lines) | `0.0` |
| `GOOSE_PLANNER_PROVIDER` / `GOOSE_PLANNER_MODEL` | Separate model for planning mode; fall back to main model if unset | provider / model | main model |
| `GOOSE_PLANNER_CONTEXT_LIMIT` | Context-limit override for the planner | int tokens | falls back to `GOOSE_CONTEXT_LIMIT` |

### Lead/Worker mode (`GOOSE_LEAD_PROVIDER` / `GOOSE_LEAD_MODEL`)

A two-tier pattern: a powerful **lead** model does planning/orchestration and complex reasoning while a cheaper **worker** model executes routine tool calls. Confirmed to be configured through dedicated variables — the upstream env-var guide carries a dedicated *"Lead/Worker Model Configuration"* section ("a powerful lead model handles initial planning and complex…" tasks; GitCode mirror of `documentation/docs/guides/environment-variables.md`), and the agent-safehouse sandbox report lists `GOOSE_LEAD_MODEL / GOOSE_LEAD_PROVIDER / etc.` among live configuration variables. GitHub issue [#4036](https://github.com/aaif-goose/goose/issues/4036) proposes consolidating this into a richer multi-model `models:` block (with per-model `purpose: oracle | lead | worker`), confirming the current mechanism is the flat pair of vars.

```bash
export GOOSE_LEAD_PROVIDER="anthropic"
export GOOSE_LEAD_MODEL="claude-opus-4-5"
# worker tier stays GOOSE_PROVIDER / GOOSE_MODEL
```

⚠️ *Unverified exact names* (from the same upstream section, not re-read in full this session):
- `GOOSE_LEAD_TEMPERATURE` — temperature override for the lead model; falls back to `GOOSE_TEMPERATURE`.
- `GOOSE_LEAD_MAX_TOKENS` — max-token override for the lead model; falls back to `GOOSE_MAX_TOKENS`.
Setting lead+provider is what switches routing on; `smart_approve` is the `GOOSE_MODE` value most associated with lead-style approval flows ⚠️ (mode value itself is verified from **[EV]**).

### `PERIODIC_HINTS` and experiment flags

- `PERIODIC_HINTS` — boolean in `config.yaml` controlling whether goose shows periodic hints/tips during sessions ⚠️ *unverified this session; appears in historical config-file docs.*
- **Experiment flags** — optional features are toggled through `goose configure` → "Experimental Features"; they persist into `config.yaml` as boolean flags named after each experiment (historically things like a planner, ledger/double-check, deep-search, developer-extra) ⚠️ *exact key spelling unverified.* Anything gated behind an experiment is off unless explicitly enabled.

### Per-extension config in `config.yaml`

Two mechanisms coexist:
1. `enabled_extensions:` — flat list of built-in/bundled extension IDs to activate (written by `goose configure`).
2. `extensions:` — map of user-installed MCP servers; each entry carries its own command/URL **and private env config** (see full schema in §5). Secrets referenced by extensions go to `secrets.yaml`/keyring, not plaintext `config.yaml`.

---

## 2. Environment-variable table

Precedence rule: **env vars > config.yaml**; some changes need a restart (**[EV]** Notes).

### Provider credential keys

⚠️ Credential variable names below follow the standard provider integrations documented in goose's provider guide; the retry knobs are verified from **[EV]**, individual key names marked where not re-verified.

| Variable | Used for | Notes |
|---|---|---|
| `ANTHROPIC_API_KEY` | Anthropic (Claude) | also works via Databricks proxy w/ thinking support (**[EV]** Claude Thinking section) |
| `OPENAI_API_KEY` | OpenAI | |
| `OPENROUTER_API_KEY` | OpenRouter aggregator | model IDs namespaced `vendor/model`, e.g. `google/gemini-2.5-flash` (**[EV]** example) |
| `OLLAMA_HOST` | local Ollama daemon | default `http://localhost:11434`; input limit maps to `num_ctx` via `GOOSE_INPUT_LIMIT` (**[EV]**) |
| `DATABRICKS_HOST` + `DATABRICKS_TOKEN` (PAT) *or* `DATABRICKS_CLIENT_ID`/`DATABRICKS_CLIENT_SECRET` (OAuth ⚠️) | Databricks-hosted foundation-model APIs | OAuth callback port fixed via `GOOSE_OAUTH_CALLBACK_PORT` (`http://localhost:8080` for Databricks, **[EV]**); retries tunable (below) |
| `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/`AWS_SESSION_TOKEN`/`AWS_PROFILE`, `AWS_REGION` ⚠️ | Amazon Bedrock | standard AWS credential chain; retries tunable (below) |
| `GOOGLE_API_KEY` / `GOOGLE_APPLICATION_CREDENTIALS` ⚠️ | Gemini / Vertex AI | SA JSON path for Vertex |
| `CLAUDE_THINKING_TYPE` | Claude reasoning mode | `adaptive` \| `enabled` \| `disabled` — adaptive default on Claude 4.6+ (**[EV]**) |
| `GEMINI3_THINKING_LEVEL` | Gemini 3 global thinking level | `low` \| `high`, default `low` (**[EV]**) |

Advanced endpoint override trio (custom/internal gateways) — **[EV]** Advanced Provider Configuration:

```bash
export GOOSE_PROVIDER__TYPE="anthropic"
export GOOSE_PROVIDER__HOST="https://internal-gw.corp.example.com"
export GOOSE_PROVIDER__API_KEY="sk-..."
```

Provider **retry tuning** (all defaults verified, **[EV]**):

| Variable | Default | Variable | Default |
|---|---|---|---|
| `BEDROCK_MAX_RETRIES` | 6 | `DATABRICKS_MAX_RETRIES` | 3 |
| `BEDROCK_INITIAL_RETRY_INTERVAL_MS` | 2000 | `DATABRICKS_INITIAL_RETRY_INTERVAL_MS` | 1000 |
| `BEDROCK_BACKOFF_MULTIPLIER` | 2 | `DATABRICKS_BACKOFF_MULTIPLIER` | 2 |
| `BEDROCK_MAX_RETRY_INTERVAL_MS` | 120000 | `DATABRICKS_MAX_RETRY_INTERVAL_MS` | 30000 |

### GOOSE_* runtime variables (selected, all **[EV]** unless noted)

| Variable | Purpose | Default |
|---|---|---|
| `GOOSE_MAX_TURNS` | turns allowed without user input | 1000 |
| `GOOSE_GATEWAY_MAX_TURNS` | stricter cap for gateway sessions (Telegram etc.) | falls back to `GOOSE_MAX_TURNS`, then 5 |
| `GOOSE_SUBAGENT_MAX_TURNS` | subagent completion budget (overridable per-recipe via `settings.max_turns`) | 25 |
| `GOOSE_MAX_BACKGROUND_TASKS` | concurrent background subagents | 5 |
| `GOOSE_AUTO_COMPACT_THRESHOLD` | fraction of tokens triggering auto-compaction (0.0 disables) | 0.8 |
| `GOOSE_CONTEXT_LIMIT` / `GOOSE_INPUT_LIMIT` | context-window override (main model / ollama `num_ctx`) | model default or 128k |
| `GOOSE_TOOLSHIM`, `GOOSE_TOOLSHIM_BACKEND`, `GOOSE_TOOLSHIM_OLLAMA_MODEL` | text→tool-call shim for weak models (`ollama`\|`local`\|`llama.cpp`; model default `mistral-nemo`) | off |
| `GOOSE_DEBUG`, `GOOSE_SHOW_FULL_OUTPUT` | show full tool parameters / disable truncation | off |
| `GOOSE_MAX_TOOL_RESPONSE_SIZE` | chars before a tool response spills to temp file | 200000 |
| `GOOSE_SHELL` | shell for Developer extension commands (flags injected automatically) | `bash`→`sh` on Unix, `cmd` on Windows |
| `GOOSE_SEARCH_PATHS` | JSON array of dirs prepended for extension binaries | built-ins + PATH |
| `CONTEXT_FILE_NAMES` | JSON array of hint/context filenames | `[".goosehints","AGENTS.md"]` |
| `GOOSE_MOIM_MESSAGE_TEXT` / `_FILE` | persistent instruction injected into working memory every turn (file ≤64 KB) | unset |
| `GOOSE_DISABLE_SESSION_NAMING` | skip AI session titles (good for CI) | false |
| `GOOSE_PROMPT_EDITOR` | external editor for prompts (`vim`, `code --wait`) | unset |
| `GOOSE_CLI_THEME` / `_LIGHT_THEME` / `_DARK_THEME` / `_NEWLINE_KEY` / `_SHOW_THINKING` / `_SHOW_COST` / `GOOSE_RANDOM_THINKING_MESSAGES` | CLI UX knobs | `ansi` / `GitHub` / `zenburn` / Ctrl-J / off / off / true |
| `GOOSE_ALLOWLIST` | URL of an allowed-extensions allowlist | unset |
| `GOOSE_TELEMETRY_ENABLED` | anonymous usage data | false |
| `SECURITY_PROMPT_ENABLED` (+`_THRESHOLD`, `_CLASSIFIER_*`) | prompt-injection detection | false / 0.8 |
| `GOOSE_OAUTH_CALLBACK_PORT` | fixed OAuth callback port for strict IdPs | random |
| `HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` | corporate proxy (HTTPS takes precedence) | none |
| `OTEL_EXPORTER_OTLP_ENDPOINT` (+ per-signal vars), `LANGFUSE_*` | telemetry export (OTLP/Langfuse) | off |
| `GOOSE_TLS`, `GOOSE_TLS_CERT_PATH`, `GOOSE_TLS_KEY_PATH`, `GOOSE_SERVER__SECRET_KEY` | `goose serve` ACP server (remote server for Desktop) | TLS off; secret required |
| `GOOSE_RECIPE_PATH`, `GOOSE_RECIPE_GITHUB_REPO`, `GOOSE_RECIPE_RETRY_TIMEOUT_SECONDS`, `GOOSE_RECIPE_ON_FAILURE_TIMEOUT_SECONDS` | recipe discovery/timeouts | none |
| `GOOSE_DOCS_ROOT` | offline docs root for the `goose-doc-guide` skill | `https://goose-docs.ai` |
| `GOOSE_TERMINAL`, `AGENT`, `AGENT_SESSION_ID` | **set by goose**, for shell scripts to detect agent execution & session-isolate handoffs | — |
| `GOOSE_PATH_ROOT` | relocate entire config/data/state root (§1) | platform default |

### Recipe variables

Recipes (`.yaml` workflows) pull configuration from four places:
1. **`GOOSE_RECIPE_*` discovery vars** above (**[EV]**).
2. **In-recipe `parameters`** — declared inputs interpolated into the prompt (`{{param_name}}`), with `default` and `required` flags; overridable on the CLI (`--params key=value` ⚠️ flag spelling unverified).
3. **In-recipe `settings`** — per-run overrides baked into the recipe file, including `goose_provider`, `goose_model`, `temperature`, `max_tokens`, `max_turns` (the last confirmed as the subagent-turn override knob in **[EV]**) ⚠️ remaining field names from recipe-reference docs.
4. **Shell env passthrough** — the Developer extension's shell inherits your session env (**[EV]** "Environment Variable Passthrough"), so recipes can rely on exported credentials; `AGENT_SESSION_ID` enables step-to-step handoff paths (**[EV]**).

---

## 3. Providers

Built-in provider families (from **[EV]** links to the providers page + standard goose provider set ⚠️ list membership partly unverified): **Anthropic, OpenAI, OpenRouter, Ollama, Google (Gemini/Vertex), Amazon Bedrock, Azure OpenAI, xAI/Grok, GitHub Copilot, Databricks, Snowflake, Venice, LiteLLM/proxy endpoints, and any OpenAI-compatible server**. Configure via `goose configure` (interactive picker writes `config.yaml` + keyring) or purely via env (preferred for scripting).

**Custom / OpenAI-compatible endpoint** (verified pattern, **[EV]** Advanced Provider Configuration):

```bash
export GOOSE_PROVIDER=openai            # or the matching family
export GOOSE_PROVIDER__TYPE=openai
export GOOSE_PROVIDER__HOST=https://your-endpoint/v1   # internal gateway, vLLM, LiteLLM…
export GOOSE_PROVIDER__API_KEY=sk-local-or-real
export GOOSE_CONTEXT_LIMIT=200000       # needed for proxies/custom models (Smart Context Management docs)
```

**Per-provider quick recipes**

```bash
# Ollama (local)
export GOOSE_PROVIDER=ollama
export GOOSE_MODEL=llama3.2
export OLLAMA_HOST=http://localhost:11434
export GOOSE_INPUT_LIMIT=32000          # becomes num_ctx
export GOOSE_TOOLSHIM=true              # shim for weak tool-callers
export GOOSE_TOOLSHIM_OLLAMA_MODEL=llama3.2

# OpenRouter
export GOOSE_PROVIDER=openrouter
export OPENROUTER_API_KEY=sk-or-...
export GOOSE_MODEL=google/gemini-2.5-flash

# Amazon Bedrock
export GOOSE_PROVIDER=bedrock
export AWS_PROFILE=my-org-profile       # or key/session env vars; region per AWS convention
export BEDROCK_MAX_RETRIES=10           # retry tuning verified [EV]
export BEDROCK_INITIAL_RETRY_INTERVAL_MS=1000
export BEDROCK_BACKOFF_MULTIPLIER=3
export BEDROCK_MAX_RETRY_INTERVAL_MS=300000

# Google Vertex AI
export GOOSE_PROVIDER=gcp_vertex_ai     # ⚠️ exact provider slug unverified
export GOOGLE_APPLICATION_CREDENTIALS=/path/service-account.json
export GOOSE_MODEL=gemini-2.5-pro

# Databricks
export GOOSE_PROVIDER=databricks
export DATABRICKS_HOST=https://adb-xxxx.xx.azuredatabricks.net
export DATABRICKS_TOKEN=dapi...          # or OAuth client id/secret
export DATABRICKS_MAX_RETRIES=5
export GOOSE_OAUTH_CALLBACK_PORT=8080    # if IdP requires fixed redirect URI [EV]
```

---

## 4. Multi-instance wrappers (isolated parallel configurations)

Goose has no first-class "profile" CLI flag; isolation comes from (a) env-var overrides beating the config file, and (b) `GOOSE_PATH_ROOT` relocating the whole config/data/state tree — the docs' own suggested mechanism for "running multiple configurations" (**[EV]**). Two styles:

**Style A — same install, provider switched per invocation** (shares extensions/sessions/history; fine when you just want different brains):

```bash
#!/usr/bin/env bash
# ~/bin/goose-anthropic — Claude via Anthropic direct
exec env \
  GOOSE_PROVIDER=anthropic \
  GOOSE_MODEL=claude-sonnet-4-5 \
  GOOSE_LEAD_PROVIDER=anthropic \
  GOOSE_LEAD_MODEL=claude-opus-4-5 \
  ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:?set me}" \
  goose "$@"

#!/usr/bin/env bash
# ~/bin/goose-openrouter — cheap worker + strong lead, both via OpenRouter
exec env \
  GOOSE_PROVIDER=openrouter \
  GOOSE_MODEL=qwen/qwen3-coder \
  GOOSE_LEAD_PROVIDER=openrouter \
  GOOSE_LEAD_MODEL=anthropic/claude-opus-4.5 \
  OPENROUTER_API_KEY="${OPENROUTER_API_KEY:?set me}" \
  goose "$@"
```

**Style B — fully isolated instances** (separate config.yaml, secrets, extensions, sessions — use when the two profiles need different installed extensions or different `GOOSE_MODE`s):

```bash
#!/usr/bin/env bash
# usage: goose-profile <name> <args…>
PROFILE_DIR="$HOME/.goose-profiles/$1"; shift
mkdir -p "$PROFILE_DIR"
case "$1" in
  anthropic) PROVIDER_ENV=(GOOSE_PROVIDER=anthropic GOOSE_MODEL=claude-sonnet-4-5
                           ANTHROPIC_API_KEY_FILE="$PROFILE_DIR/key") ;;
  ollama)    PROVIDER_ENV=(GOOSE_PROVIDER=ollama GOOSE_MODEL=llama3.2
                           OLLAMA_HOST=http://localhost:11434 GOOSE_MODE=auto) ;;
esac
exec env \
  "${PROVIDER_ENV[@]}" \
  GOOSE_PATH_ROOT="$PROFILE_DIR" \        # isolates config/, data/, state/ [EV]
  GOOSE_DISABLE_SESSION_NAMING=true \     # cheaper CI-ish runs
  goose "$@"
# one-time init per profile:  goose-profile anthropic goose configure
```

Notes: because `GOOSE_PATH_ROOT` moves config *and* state, each profile re-runs `goose configure` once; Desktop and CLI share the same root only if launched with the same var. For headless/parallel CI, prefer Style A plus `GOOSE_DISABLE_SESSION_NAMING=true` and a fresh temp root (`GOOSE_PATH_ROOT="$(mktemp -d)"` — exact pattern from **[EV]**).

---

## 5. MCP / extensions, permissions, recipes

**Extensions = MCP servers.** Two transports, configured either via `goose configure` → Extensions or directly in `config.yaml` under `extensions:` (schema ⚠️ field spellings from goose's `ExtensionConfig` serialization; structure consistent with docs):

```yaml
enabled_extensions: [developer]        # bundled ones

extensions:
  filesystem-tools:                    # STDIO (local process)
    enabled: true
    type: stdio
    cmd: npx
    args: ["-y", "@modelcontextprotocol/server-filesystem", "/srv"]
    timeout: 300
    envs:                              # per-extension env config lives here
      NODE_OPTIONS: "--max-old-space-size=4096"
    bundled: false
    description: "File access"
  team-remote:                         # REMOTE (HTTP/SSE)
    enabled: true
    type: sse                          # or streamable_http
    uri: https://mcp.corp.example.com/sse
    env_keys: ["TEAM_MCP_TOKEN"]       # prompted once, stored in keyring/secrets.yaml
```

Runtime knobs around extensions: `GOOSE_SEARCH_PATHS` (binary lookup), `GOOSE_ALLOWLIST` (org-wide load restriction), `GOOSE_DEBUG` (full tool params), `AGENT_SESSION_ID` auto-injected into STDIO extensions and shell tools (**[EV]**). Bundled `developer` extension provides shell/file editing; its shell honors `GOOSE_SHELL` and env passthrough (**[EV]**).

**Permission / approval modes** — `GOOSE_MODE` (**[EV]** Tool Configuration):
- `approve` — ask before every tool execution (safest interactive default).
- `auto` — execute tools without asking (headless/automation).
- `chat` — no tool execution at all (conversation only).
- `smart_approve` — selective approval flow (pairs naturally with lead/worker routing).
Complementary guardrails: `SECURITY_PROMPT_ENABLED` injection detection, `GOOSE_ALLOWLIST`, keyring secret storage, per-tool output filtering via `GOOSE_CLI_MIN_PRIORITY`.

**Recipes** — declarative `.yaml` workflows run with `goose run --recipe f.yaml`. Discovery: working dir, `~/.config/goose/recipes` ⚠️, plus `GOOSE_RECIPE_PATH` (colon-separated) and a shared team repo via `GOOSE_RECIPE_GITHUB_REPO=owner/repo` (**[EV]** Recipe Configuration). Anatomy (fields per recipe-reference docs ⚠️ except where noted): `title`, `description`, `prompt` (with `{{parameter}}` interpolation), `parameters` (defaults/required), `extensions` (activate specific MCP servers for the run), `context` (sub-recipes, static files), `retry`/`on_failure`/`on_success` hooks (timeouts globally capped by the `GOOSE_RECIPE_*_TIMEOUT_SECONDS` vars, **[EV]**), and `settings` — including `goose_provider`, `goose_model`, `temperature`, `max_tokens` and `max_turns` (the subagent-turn override named in **[EV]**) — letting a single recipe pin its own brain regardless of your global config. Sub-recipe concurrency is a config option (`GOOSE_SUBRECIPE_CONCURRENCY` ⚠️ unverified).

---

## 6. Desktop app config note

- goose **Desktop** reads/writes the same config layer as the CLI (same `GOOSE_*` semantics; env vars still win — relevant because Desktop may not see shell-exported vars, so set them via launchctl/`~/.zshenv` equivalents ⚠️ platform detail). Settings UI covers provider/model selection, extensions (MCP marketplace + custom), permission mode, and updates; API keys go to the **system keyring** by default, falling back to `secrets.yaml` beside `config.yaml` (**[EV]** security tip).
- Platform paths: macOS `~/Library/Application Support/Block/goose/`, Windows `%APPDATA%\Block\goose\config\` (**[EV]**).
- Desktop can connect to a **remote/self-hosted goose** instead of running locally: start `goose serve --platform desktop --enable-scheduler --host 0.0.0.0 --port 3000 --tls` with `GOOSE_SERVER__SECRET_KEY` set; Desktop pins the cert via the printed `GOOSED_CERT_FINGERPRINT` (**[EV]** ACP Server section).
- Claude-thinking output needs no extra flag in Desktop (collapsible "Show reasoning"), whereas the CLI requires `GOOSE_CLI_SHOW_THINKING=1` (**[EV]**).
