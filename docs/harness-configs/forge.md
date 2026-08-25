# Forge (ForgeCode) — Complete Configuration Reference

*Compiled 2026-08-25. Primary sources: [tailcallhq/forgecode README](https://github.com/tailcallhq/forgecode), [forgecode.dev docs](https://forgecode.dev/docs/) (`.forge.toml`, Custom Providers, Permissions, Creating Agents, MCP, Commands pages), and repo source (`crates/forge_config/src/config.rs`).*

---

## 1. Config files: exact paths & schema

| File | Location | Purpose |
|---|---|---|
| Global config | `~/.forge/.forge.toml` (Windows: `%USERPROFILE%\.forge\.forge.toml`) | Limits, sampling, retry, HTTP, compaction, providers, session defaults ([forgecode-config](https://forgecode.dev/docs/forgecode-config/)) |
| Custom config dir | `$FORGE_CONFIG/.forge.toml` when `FORGE_CONFIG` is set (default base dir: `~/.forge`) ([README, System Configuration](https://github.com/tailcallhq/forgecode)) |
| Project config | `forge.yaml` in project root | `model`, `custom_rules`, `commands[]`, `temperature`, `top_p/top_k/max_tokens`, `max_walker_depth`, `max_requests_per_turn`, `max_tool_failure_per_turn` ([README forge.yaml section](https://github.com/tailcallhq/forgecode)) |
| Policy file | `~/.forge/permissions.yaml` (or `$FORGE_CONFIG/permissions.yaml`) | Tool approval policies; only active with `restricted = true` ([permissions](https://forgecode.dev/docs/permissions/)) |
| MCP config | project `.mcp.json` + global `~/.forge/.mcp.json` (project wins) ([mcp-integration](https://forgecode.dev/docs/mcp-integration/)) |
| Env file | `~/.env` — loaded automatically on every run ([custom-providers#environment-variables](https://forgecode.dev/docs/custom-providers/)) |
| Agents | `.forge/agents/*.md` (project) > `~/forge/agents/*.md` (global) ([creating-agents](https://forgecode.dev/docs/creating-agents/)) |
| Skills | `.forge/skills/<name>/SKILL.md` > `~/forge/skills/<name>/SKILL.md` > built-in ([README Skills](https://github.com/tailcallhq/forgecode)) |
| Commands | `.forge/commands/*.md` > `~/.agents/commands/` > `~/forge/commands/*.md` ([commands](https://forgecode.dev/docs/commands/)) |
| Rules file | `AGENTS.md` in project root or `~/forge/AGENTS.md` — persistent instructions for all agents ([README](https://github.com/tailcallhq/forgecode)) |

Note: the task's guesses "`~/.config/forge/config.toml`" and "theme" are **wrong** — it's `~/.forge/.forge.toml`, and there is no theme option (display customization is via env vars, §2). Edit via `:config-edit` in-session; changes apply on next start.

### `.forge.toml` full schema (defaults shown)

Top-level keys ([forgecode-config](https://forgecode.dev/docs/forgecode-config/); cross-checked against `ForgeConfig` struct in [crates/forge_config/src/config.rs](https://github.com/tailcallhq/forgecode/blob/main/crates/forge_config/src/config.rs)):
`auto_open_dump=false`, `max_conversations=100`, `max_extensions=15`, `max_fetch_chars=50000`, `max_file_read_batch_size=50`, `max_file_size_bytes=104857600`, `max_image_size_bytes=262144`, `max_line_chars=2000`, `max_parallel_file_reads=64`, `max_read_lines=2000`, `max_requests_per_turn=100`, `max_search_lines=1000`, `max_search_result_bytes=10240`, `max_sem_search_results=100`, `max_stdout_line_chars=500`, `max_stdout_prefix_lines=100`, `max_stdout_suffix_lines=100`, `max_tokens=20480`, `max_tool_failure_per_turn=3`, `model_cache_ttl_secs=604800`, `restricted=false`, `sem_search_top_k=10`, `services_url="https://api.forgecode.dev/"`, `tool_supported=true`, `tool_timeout_secs=300`, `top_k=30`, `top_p=0.8`, plus undocumented-in-docs but present in source: `debug_requests`, `custom_history_path`, `auto_dump`, `currency_symbol`, `currency_conversion_rate`, `verify_todos`, `use_text_patch_fallback`, `research_subagent`, `subagents`, `auto_install_vscode_extension`, `merge_system_messages`, `use_forge_committer`, `max_commit_count`, `reasoning`, `temperature`.

**Context-engine knobs** = the sem-search/file-limit block above (`max_sem_search_results`, `sem_search_top_k`, `max_search_lines`, `max_search_result_bytes`, `services_url` indexing server).

Sub-tables:

```toml
[session]   provider_id = "..."; model_id = "..."        # default model/provider
[commit]    # ModelConfig — model used by :commit
[suggest]   # ModelConfig — model used by :suggest
[retry]     initial_backoff_ms=200, backoff_factor=2, max_attempts=8,
            min_delay_ms=1000, suppress_errors=false,
            status_codes=[429,500,502,503,504,408,522,520,529]
[http]      connect_timeout_secs=30, read_timeout_secs=900, pool_idle_timeout_secs=90,
            pool_max_idle_per_host=5, max_redirects=10, hickory=false,
            tls_backend="default", min_tls_version/max_tls_version, adaptive_window=true,
            keep_alive_interval_secs=60, keep_alive_timeout_secs=10,
            keep_alive_while_idle=true, accept_invalid_certs=false,
            root_cert_paths=["..."]        # custom CA support
[compact]   eviction_window=0.2, max_tokens=2000, message_threshold=200,
            on_turn_end=false, retention_window=6, token_threshold=100000
[updates]   auto_update=true, frequency="daily"
[[providers]]  # see §3
```

### Git settings
No `[git]` table exists. Git behavior is: AI commit via `forge commit [--preview]` / `:commit` / `:commit-preview`; commit model settable with `:config-commit-model <id>` or the `[commit]` block; `use_forge_committer` and `max_commit_count` in source; `--sandbox <name>` creates an isolated git worktree+branch. ([README CLI options/subcommands](https://github.com/tailcallhq/forgecode))

### Policy/approvals
`restricted = true` in `.forge.toml` activates `permissions.yaml`: top-level `policies:` list; each entry `{permission: allow|deny|confirm}` + one rule of `{read|write|command|url}` glob, optionally scoped by `dir:` glob; supports logical `all`/`any`/`not`. Evaluation: matching deny stops → confirm asks → allow noted but scanning continues; **no match = confirm**. Confirmation offers Accept / Reject / Accept-and-Remember (appends a generated pattern to the file). Exempt tools: SemSearch, Undo, Plan, Task; **MCP tools bypass this policy entirely**. Default auto-created file is allow-all. ([permissions](https://forgecode.dev/docs/permissions/))

---

## 2. Complete environment variable list

All from the README "Advanced Configuration" section unless noted. Provider API-key vars go through `api_key_vars` in `.forge.toml` (§3); there is **no `FORGE_OPENAI_API_KEY`-style naming** — keys use plain provider names (`OPENAI_API_KEY`, etc.).

**Core/system**
| Var | Meaning (default) |
|---|---|
| `FORGE_CONFIG` | Base dir for all config files (default `~/.forge`) |
| `FORGE_API_KEY` | ForgeCode Services key (deprecated .env form; use `forge provider login`) |
| `FORGE_API_URL` | Forge services URL (default `https://api.forgecode.dev`) |
| `FORGE_WORKSPACE_SERVER_URL` | Indexing server (default `https://api.forgecode.dev/`) |
| `FORGE_HISTORY_FILE` | Custom history file path |
| `FORGE_BANNER` | Custom startup banner text |
| `FORGE_MAX_CONVERSATIONS` | Max conversations in list (100) |
| `FORGE_MAX_SEARCH_RESULT_BYTES` | Search result byte cap (10240) |
| `FORGE_SEM_SEARCH_LIMIT` | Initial vector-search result cap (200) |
| `FORGE_SEM_SEARCH_TOP_K` | Semantic search top-k (20) |
| `FORGE_MAX_LINE_LENGTH` | File-read line cap (2000) |
| `FORGE_STDOUT_MAX_LINE_LENGTH` | Shell-output line cap (2000) |
| `FORGE_TOOL_TIMEOUT` | Per-tool timeout seconds (300) |
| `FORGE_MAX_IMAGE_SIZE` | read_image size cap bytes (10485760) |
| `FORGE_DUMP_AUTO_OPEN` | Auto-open dump files (false) |
| `FORGE_DEBUG_REQUESTS` | Path to write debug HTTP request files |
| `FORGE_LOG` | tracing filter, e.g. `forge=info` (info w/ tracking, debug without) |
| `FORGE_TRACKER` | Telemetry enrichment metadata (true) |
| `FORGE_BIN` | Binary name the ZSH plugin invokes (default `forge`) |
| `SHELL` / `COMSPEC` | Shell used for command execution |

**Retry**: `FORGE_RETRY_INITIAL_BACKOFF_MS=1000`, `FORGE_RETRY_BACKOFF_FACTOR=2`, `FORGE_RETRY_MAX_ATTEMPTS=3`, `FORGE_RETRY_STATUS_CODES=429,500,...`, `FORGE_SUPPRESS_RETRY_ERRORS=false`

**HTTP** (all mirror `[http]`): `FORGE_HTTP_CONNECT_TIMEOUT`, `_READ_TIMEOUT`, `_POOL_IDLE_TIMEOUT`, `_POOL_MAX_IDLE_PER_HOST`, `_MAX_REDIRECTS`, `_USE_HICKORY`, `_TLS_BACKEND`, `_MIN_TLS_VERSION`, `_MAX_TLS_VERSION`, `_ADAPTIVE_WINDOW`, `_KEEP_ALIVE_INTERVAL`, `_KEEP_ALIVE_TIMEOUT`, `_KEEP_ALIVE_WHILE_IDLE`, `_ACCEPT_INVALID_CERTS`, `_ROOT_CERT_PATHS` (comma-separated)

**Display**: `FORGE_CURRENCY_SYMBOL="$"`, `FORGE_CURRENCY_CONVERSION_RATE=1.0`, `NERD_FONT` / `USE_NERD_FONT` (ZSH-theme icons)

**Provider credential vars (legacy/deprecated)**: `OPENROUTER_API_KEY`, `REQUESTY_API_KEY`, `XAI_API_KEY`, `ZAI_API_KEY` / `ZAI_CODING_API_KEY`, `CEREBRAS_API_KEY`, `NEURALWATT_API_KEY`, `ORCAROUTER_API_KEY`, `META_API_KEY`, `IO_INTELLIGENCE_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, Vertex trio `PROJECT_ID`/`LOCATION`/`VERTEX_AI_AUTH_TOKEN`, generic OpenAI-compatible pair `OPENAI_API_KEY`+`OPENAI_URL` (Groq example: `OPENAI_URL=https://api.groq.com/openai/v1`). On first run these migrate to file-based storage via `forge provider login`.

---

## 3. Providers & model selection

Built-ins out of the box ([custom-providers](https://forgecode.dev/docs/custom-providers/)): **OpenRouter, OpenAI, Anthropic, Google Vertex AI, Groq, Amazon Bedrock** (plus legacy-env-documented: Requesty, x-ai, z.ai, Cerebras, Neuralwatt, OrcaRouter, Meta, IO Intelligence, ForgeCode Services). List live with `forge list provider` / `forge list model`.

Credentials: recommended `forge provider login` / `forge provider logout` (interactive, stored file-based under the config dir); `:login` / `:logout` in-session.

### Custom OpenAI-compatible provider — worked examples

OpenRouter:
```toml
[[providers]]
id             = "openrouter-custom"
url            = "https://openrouter.ai/api/v1/chat/completions"
api_key_vars   = "OPENROUTER_API_KEY"
response_type  = "OpenAI"
auth_methods   = ["api_key"]
url_param_vars = []

[session]
provider_id = "openrouter-custom"
model_id    = "anthropic/claude-sonnet-4"
```

Ollama (local, no key):
```toml
[[providers]]
id = "ollama"
url = "http://localhost:11434/v1/chat/completions"
models = "http://localhost:11434/v1/models"     # dynamic model list
response_type = "OpenAI"
auth_methods = ["api_key"]
```
vLLM/litellm identical shape — swap the base URL (`http://host:8000/v1/chat/completions`, `http://host:4000/v1/chat/completions`) and set `api_key_vars` if a key is required.

Full `[[providers]]` field reference: `id`* , `url`* (supports `{{VAR}}` templates), `api_key_vars` (env var *name*, not value), `auth_methods` (`["api_key"]` default or `["google_adc"]`), `custom_headers` (via following `[providers.custom_headers]` table), `models` (URL or inline `[[providers.models]]` array with `id/name/description/context_length/tools_supported/supports_parallel_tool_calls/supports_reasoning/input_modalities`), `provider_type` (`"llm"` default or `"context_engine"` for indexing/search), `response_type` (`OpenAI`, `OpenAIResponses`, `Anthropic`, `Bedrock`, `Google`, `OpenCode`), `url_param_vars`. An entry whose `id` matches a built-in **overrides** that built-in (e.g. point `openai` at a corporate proxy). Keys can live in `~/.env` which Forge loads automatically.

### Model selection
In-session slash commands (TUI/ZSH): `:model` / `:m <id>` (session only, interactive picker), `:config-model` / `:cm <id>` (persistent), `:provider` / `:p` (switch provider persistently), `:config-commit-model` / `:ccm`, `:config-suggest-model` / `:csm`, `:reasoning-effort` / `:re` (none…max), `:config-reasoning-effort` / `:cre`, `:config-reload` / `:cr`, `:info`, `:config` (dump resolved TOML). CLI: `forge --agent <id>`, `forge conversation resume <id>`. Project default: `model:` in `forge.yaml`. ([README Session & Configuration](https://github.com/tailcallhq/forgecode), [model-selection-guide](https://forgecode.dev/docs/model-selection-guide/))

---

## 4. Multi-instance wrappers

The single supported switch is **`FORGE_CONFIG`** — it relocates the entire config tree (`.forge.toml`, credentials, `permissions.yaml`, MCP user scope, skills, agents, commands). Combine with per-provider env vars for isolation. Headless/non-interactive: `-p/--prompt` (one-shot, also reads piped stdin), `-e/--event <JSON>`, `--conversation <file.json>`, `--conversation-id <ID>`, `--agent <AGENT>`, `-C <dir>`, `--sandbox <NAME>`, `--verbose`; subcommands `forge commit --preview`, `forge suggest`, `forge workspace query`, `forge mcp …` all exit without TUI. There is no `--headless` flag — `-p` *is* headless mode.

Wrapper script example (two isolated instances):

```bash
#!/usr/bin/env bash
# forge-openai: instance backed by OpenAI, own config dir & history
forge_openai() {
  FORGE_CONFIG="$HOME/.forge-openai" \
  OPENAI_API_KEY="sk-..." \
  exec forge "$@"
}
# forge-local: Ollama instance with restricted mode + its own policies
forge_local() {
  FORGE_CONFIG="$HOME/.forge-ollama" \
  FORGE_LOG=forge=warn \
  exec forge -C "$PWD" "$@"
}
# non-interactive usage: forge_openai -p "refactor src/auth.rs"
```
Each `FORGE_CONFIG` dir gets its own `.forge.toml` with distinct `[session] provider_id/model_id` and its own credential store, so logins don't collide.

---

## 5. Agents (forge/sage/muse), web search, MCP

**Built-in agent split** ([README Agents](https://github.com/tailcallhq/forgecode); definitions in [`crates/forge_repo/src/agents/{forge,muse,sage}.md`](https://github.com/tailcallhq/forgecode/tree/main/crates/forge_repo/src/agents)):
- `forge` — implementation agent; writes files, runs tests (full toolset).
- `sage` (alias `:ask`) — read-only research; tools `sem_search, search, read, fetch`; reasoning enabled.
- `muse` (alias `:plan`) — planning; writes plans to `plans/`; tools include `plan`, `sage` (subagent), `mcp_*`.
Invoke: `:<prompt>` for active agent, `:sage/:muse/<custom> <prompt>`, `:agent <name>` to switch, `forge --agent <id>`. Custom agents: Markdown + YAML frontmatter (`id` required; optional `title, description, model, provider, temperature, top_p, top_k, max_tokens, max_turns, max_requests_per_turn, max_tool_failure_per_turn, tool_supported, reasoning{enabled,effort,max_tokens,exclude}, tools[], user_prompt` Handlebars template) in `.forge/agents/` (project, wins) or `~/forge/agents/`. Overriding a built-in id replaces it entirely. Agents need a `description` to be callable as tools by other agents. ([creating-agents](https://forgecode.dev/docs/creating-agents/))

**Web access:** no dedicated "web search engine" setting exists. Web capability = the `fetch` tool (capped by `max_fetch_chars`), network-gated through `url` rules in `permissions.yaml`, plus `sem_search` over your indexed codebase (`FORGE_WORKSPACE_SERVER_URL` self-host override).

**MCP:** fully supported, both stdio (`command`/`args`/`env`) and remote (`url`) servers in `.mcp.json`; scopes local (project) > user (`$FORGE_CONFIG/.mcp.json`); `"disable": true` toggles without deleting; CLI `forge mcp import/list/show/remove/reload` (`import -s user|local`); tools auto-register to all agents (`tools: mcp_*` glob in agents); MCP tools bypass permissions.yaml. ([mcp-integration](https://forgecode.dev/docs/mcp-integration/))

---

## 6. Repo lineage note

- The project originated as **antinomyhq/forge** (Antinomy / TailCall team; assets still served from `assets.antinomy.ai`, npm package `npm-code-forge` under antinomyhq).
- GitHub now returns **301 Moved Permanently** for `antinomyhq/forge` → repository id `900461318` = **tailcallhq/forgecode** (verified via GitHub API on 2026-08-25). All CI badges, install URLs (`nix run github:tailcallhq/forgecode`) and docs point there.
- **sst/forge does not exist** (GitHub API 404) — the "sst" hop in the sst→antinomy→tailcall lineage could not be verified against any primary source; treat it as unconfirmed. Confirmed lineage: **antinomyhq/forge → tailcallhq/forgecode**, website [forgecode.dev](https://forgecode.dev).
