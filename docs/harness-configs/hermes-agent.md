# Hermes Agent — Configurable Options Reference

**Subject:** Hermes Agent (Nous Research) — open-source terminal/messaging AI agent harness.
**Written:** 2026-08-25 · **Local install:** v0.20.5 (2026.8.19), git install at `~/.hermes/hermes-agent`, upstream commit `a4f16e3fef` (2026-08-22), Python 3.11.16, `_config_version: 38`.

**Provenance legend** — every claim is tagged:
- **[L]** = verified from the LOCAL install (CLI output, live `~/.hermes/config.yaml`, `.env` key names, or source code on disk). Ground truth.
- **[D]** = from official docs (URLs cited inline).
- **[S]** = verified in local source code (`~/.hermes/hermes-agent/…`), i.e. stronger than docs but not user-facing docs.

All secrets/keys are masked below as `<redacted>`. Local config was read and scrubbed programmatically; no secret values appear here.

---

## 1. `~/.hermes/config.yaml` schema

Config lives at `~/.hermes/config.yaml`; secrets go in `~/.hermes/.env`. Manage with `hermes config` / `config edit` / `config get KEY` / `config set KEY VAL` / `config unset KEY` / `config check` / `config migrate`. **[L]** `hermes config path` → `/home/remixer/.hermes/config.yaml`, `hermes config env-path` → `/home/remixer/.hermes/.env`.

Precedence **[D]** ([Configuration](https://hermes-agent.nousresearch.com/docs/user-guide/configuration)): CLI args > `config.yaml` > `.env` > built-in defaults. `${VAR}` env substitution works inside config.yaml (also Cursor-style `${env:VAR}`); undefined refs stay verbatim with a warning.

Full top-level section list observed locally **[L]**: `model, providers, fallback_providers, credential_pool_strategies, toolsets, database, runtime, max_concurrent_sessions, max_live_sessions, session, agent, terminal, web, browser, checkpoints, context_file_max_chars, file_read_max_chars, mcp_discovery_timeout, mcp_single_query_discovery_timeout, mcp, tool_output, tool_loop_guardrails, compression, prompt_caching, openrouter, bedrock, auxiliary, display, dashboard, privacy, tts, stt, voice, wake_word, human_delay, context, memory, delegation, prefill_messages_file, goals, loops, moa, skills, curator, honcho, timezone, slack, discord, whatsapp, telegram, mattermost, matrix, approvals, command_allowlist, quick_commands, platform_hints, hooks, hooks_auto_accept, personalities, security, cron, kanban, code_execution, tools, logging, model_catalog, model_overrides, models_dev, network, monitoring, gateway, streaming, sessions, onboarding, telemetry, doctor, updates, lsp, x_search, secrets, paste_collapse_*, computer_use, proxy, desktop, vertex, _config_version` (this is `DEFAULT_CONFIG.keys()` imported straight from `hermes_cli/config.py`) **[S]**.

### 1.1 Model + primary/fallback chain

**Primary model** **[L]** (actual values from this install):

```yaml
model:
  default: stealth/ox-alpha      # 'model:' is an accepted alias for 'default:' [D]
  provider: nous
```

**Fallback chain** — canonical top-level list; tried in order when primary fails (rate-limit / 5xx / connection errors); swap is mid-session without losing the conversation; one-shot activation per session **[L][D]**:

```yaml
fallback_providers:
  - provider: opencode-go
    model: ox-alpha-free
  - provider: openrouter
    model: stealth/ox-alpha
```

Verified locally via `hermes fallback list`: `Primary: stealth/ox-alpha (via nous)` → `1. ox-alpha-free (via opencode-go)`, `2. stealth/ox-alpha (via openrouter)` **[L]**. Manage with `hermes fallback add|remove|list` **[L]**. Legacy single-pair `fallback_model:` dict still accepted **[D]**. Supported fallback provider ids (docs): `openrouter, nous, novita, openai-codex, copilot, copilot-acp, anthropic, gemini, qwen-oauth, huggingface, zai, kimi-coding, kimi-coding-cn, minimax, minimax-cn, minimax-oauth, deepseek, nvidia, xai, xai-oauth, ollama-cloud, bedrock, ai-gateway, azure-foundry, opencode-zen, opencode-go, commandcode(+anthropic), kilocode, xiaomi, arcee, gmi, actual, stepfun, lmstudio, alibaba(+coding-plan), tencent-tokenhub, custom` **[D]** ([Providers → Fallback Providers](https://hermes-agent.nousresearch.com/docs/integrations/providers#fallback-providers)). Optional per-entry keys: `base_url`, `api_mode` **[D]**.

Related knobs seen locally **[L]**: `openrouter.response_cache/response_cache_ttl/min_coding_score`, `bedrock.region/discovery/guardrail`, `prompt_caching.cache_ttl: "5m"`, `model_catalog.{enabled,url,ttl_hours}` (catalog fetched from `https://hermes-agent.nousresearch.com/docs/api/model-catalog.json`), `agent.reasoning_effort: medium` (+ per-model `agent.reasoning_overrides` per CLI help), `moa.*` (Mixture-of-Agents slots via `hermes moa`).

### 1.2 `api_keys` / credential structure

Hermes does **not** keep an `api_keys:` block in `config.yaml`. Keys live in:
- `~/.hermes/.env` as flat env vars (see §2). `hermes config set OPENROUTER_API_KEY sk-...` auto-routes key-like values into `.env` **[D]**.
- `~/.hermes/auth.json` for OAuth credentials + **credential pools** (multiple rotating keys per provider): manage via `hermes auth add|list|remove|reset` **[L]** (file confirmed present locally).
- Inline per-provider `api_key` fields are allowed only inside `providers.<name>` entries and `auxiliary.*` blocks (both accept `${ENV_VAR}` references) **[D]**.
- `secrets.bitwarden.*` block (Bitwarden Secrets Manager: `enabled, access_token_env, project_id, cache_ttl_seconds, override_existing, auto_install, server_url` — present locally, disabled) plus `hermes secrets` for Bitwarden/1Password external sources **[L]**.
- `credential_pool_strategies` top-level section exists in defaults **[S]**.

### 1.3 `tts.providers` / TTS+STT

There is no literal `tts.providers:` key; TTS providers are sibling keys under `tts:`. Verified local structure **[L]**:

```yaml
tts:
  provider: edge            # active provider selector
  edge:       { voice: en-US-AriaNeural }
  elevenlabs: { voice_id: <redacted>, model_id: eleven_multilingual_v2 }
  openai:     { model: gpt-4o-mini-tts, voice: alloy }
  xai:        { voice_id: eve, language: en, sample_rate: 24000, bit_rate: 128000 }
  mistral:    { model: voxtral-mini-tts-2603, voice_id: <redacted> }
  neutts:     { ref_audio: "", ref_text: "", model: neuphonic/neutts-air-q4-gguf, device: cpu }
  piper:      { voice: en_US-lessac-medium }
```

Env vars: `ELEVENLABS_API_KEY`, `VOICE_TOOLS_OPENAI_KEY`, `MINIMAX_API_KEY`, `MISTRAL_API_KEY`; edge/neutts/piper need none **[D]**.

STT **[L]**: `stt.{enabled, provider: local, local.model: base, local.language, openai.model: whisper-1, mistral.model: voxtral-mini-latest}`; providers `local | groq | openai | mistral` **[D]**. Plus a `voice:` block (record_key, max_recording_seconds, auto_tts, beep, silence thresholds) **[L]**.

### 1.4 Discord gateway keys (incl. `channel_prompts`, `channel_skill_bindings`)

Live structure from this install's config **[L]** (channel prompt texts abbreviated):

```yaml
discord:
  require_mention: true
  free_response_channels: ""        # channels bot answers without mention
  allowed_channels: ""
  auto_thread: true
  thread_require_mention: false
  history_backfill: true
  history_backfill_limit: 50
  reactions: true
  dm_role_auth_guild: ""
  server_actions: ""
  allow_any_attachment: false
  max_attachment_bytes: 33554432
  channel_prompts:                  # per-channel system-prompt preamble
    "1540677988373233714": "This is the #minecraft channel … load the 'Minecraft' skill …"
    "1540678020640153611": "#research channel: deep research, source verification …"
    # … keyed by Discord channel/thread snowflake ID; threads inherit parent prompt
  channel_skill_bindings:           # auto-loaded skills per channel ID
    - { id: "1540677988373233714", skills: ["minecraft"] }
    - { id: "1540678020640153611", skills: ["web-research", "grounded-citations"] }
    - { id: "1540701626031935598", skills: ["discord-channel-config"] }
```

Bot token itself lives in `.env` as `DISCORD_BOT_TOKEN` (+ `DISCORD_HOME_CHANNEL`, `DISCORD_ALLOWED_USERS` present locally) **[L]**. Docs require Message Content Intent enabled for the bot **[D]**. Sibling per-platform blocks exist for `telegram` (`reactions`, `allowed_chats`), `slack`, `mattermost`, `matrix` — all present locally **[L]**.

### 1.5 `mcp_servers`

Verified local entry **[L]**:

```yaml
mcp_servers:
  dokploy:
    command: bunx          # stdio transport: command + args + env
    args: ["-y", "@dokploy/mcp"]
    env: { DOKPLOY_URL: "http://localhost:3000", DOKPLOY_API_KEY: <redacted> }
    enabled: false         # failed test saves as disabled; patch to re-test
```

Manage via `hermes mcp add NAME --url … | --command BIN --args A --env K=V`, `remove`, `list`, `test`, `configure`; run Hermes itself as an MCP server with `hermes mcp serve`; `/reload-mcp` in session **[L]**. HTTP servers use `--url`. `${env:VAR}` SecretRef syntax resolves inside `mcp_servers` too **[D]**. Related defaults: `mcp_discovery_timeout`, `mcp_single_query_discovery_timeout`, `auxiliary.mcp.*` **[S]**.

### 1.6 Tool toggles

Two layers, verified locally **[L]**:

```yaml
toolsets: ["hermes-cli"]          # global extra toolsets
platform_toolsets:                # which named bundle each platform inherits
  cli:       [bfl, browser, clarify, code_execution, computer_use, cronjob,
              delegation, file, kanban, memory, session_search, skills,
              terminal, todo, tts, vision, web]
  telegram:  [hermes-telegram]
  discord:   [hermes-discord]     # …whatsapp/slack/signal/homeassistant/qqbot/yuanbao
disabled_toolsets: []             # under agent:
known_builtin_toolsets.cli: [bfl, browser, clarify, code_execution, computer_use,
  context_engine, cronjob, delegation, discord, discord_admin, file, homeassistant,
  image_gen, memory, session_search, skills, spotify, stt, terminal, todo, tts,
  video, video_gen, vision, web, x_search, yuanbao]
known_plugin_toolsets.cli: [a2a, spotify]   # contributed by plugins
```

Toggle interactively with `hermes tools` / `tools enable|disable NAME` / `tools list`; changes take effect on new session (`/reset`) **[L]**. Per-invocation: `-t/--toolsets csv` **[L]**.

### 1.7 Other notable sections (all seen in local config) **[L]**

| Section | Key options (observed values) |
|---|---|
| `agent` | `max_turns: 999`, `gateway_timeout: 1800`, `tool_use_enforcement: auto`, `reasoning_effort`, `personalities{}` (13 built-ins incl. helpful/concise/catgirl/noir) |
| `terminal` | `backend: local` (local/docker/ssh/modal/daytona/vercel_sandbox/singularity **[D]**), `cwd`, `timeout: 180`, `env_passthrough: []`, per-backend images, `persistent_shell`, `container_*` resources |
| `compression` | `enabled`, `threshold: 0.5`, `target_ratio: 0.2`, `protect_first_n/last_n`, `hygiene_hard_message_limit` |
| `memory` | `memory_enabled: true`, `user_profile_enabled: true`, `memory_char_limit: 2200`, `user_char_limit: 1375`, `nudge_interval`, `provider: ""` |
| `delegation` | `max_iterations: 250`, `child_timeout_seconds: 600`, `max_concurrent_children: 10`, `max_spawn_depth: 1`, `orchestrator_enabled`, per-child `model/provider/base_url/api_key/api_mode` |
| `approvals` | `mode: false`, `timeout: 60`, `cron_mode: deny`, `destructive_slash_confirm` |
| `security` | `redact_secrets: true`, `tirith_enabled: true` (+path/timeout/fail_open), `allow_private_urls`, `website_blocklist.{enabled,domains}`, `allow_lazy_installs` |
| `checkpoints` | `enabled: false`, `max_snapshots`, retention/prune knobs |
| `skills` | `external_dirs: [/home/remixer/.agents/skills]`, `template_vars`, `inline_shell`, `guard_agent_created`, `creation_nudge_interval` |
| `curator` | `enabled`, `interval_hours: 168`, `stale_after_days: 30`, `archive_after_days: 90`, `backup.{enabled,keep}` |
| `kanban` | `dispatch_in_gateway: true`, `dispatch_interval_seconds: 60`, `failure_limit`, `auto_decompose`, `orchestrator_profile` |
| `cron` | `wrap_response: true` |
| `sessions` | `retention_days: 90`, `auto_prune`, `vacuum_after_prune`, `write_json_snapshots` |
| `session_reset` | `mode: both`, `idle_minutes: 1440`, `at_hour: 4` |
| `display` | `skin`, `streaming`, `show_reasoning/show_cost`, `inline_diffs`, `runtime_footer.{enabled,fields}` |
| `updates` | `pre_update_backup` (quick/full/off **[D]**), `backup_keep: 5` |
| `lsp` | `enabled: true`, `wait_mode`, `wait_timeout`, `install_strategy: auto` |

---

## 2. Environment variables & profiles

### Env vars **[L]** unless noted

- **`HERMES_HOME`** — relocates the whole Hermes home (default `~/.hermes`). Read by `get_hermes_home()` in `hermes_constants.py`; tests redirect it to temp dirs **[S]**. Cannot be persisted via `hermes config set` (with `HERMES_PROFILE`, `HERMES_CONFIG`, `HERMES_ENV` it's boot-level only) **[S]**.
- **`HERMES_PROFILE`** — selects profile without CLI flag (used by gateway spawn + kanban worker env) **[S]**. Also `HERMES_CONFIG`, `HERMES_ENV` **[S]**.
- Provider keys (docs table; names cross-checked against local `.env` where present):
  - Present locally **[L]**: `OPENROUTER_API_KEY`, `DEEPSEEK_API_KEY`, `OPENCODE_GO_API_KEY`, `DISCORD_BOT_TOKEN`, `DISCORD_HOME_CHANNEL`, `DISCORD_ALLOWED_USERS`.
  - Full documented set **[D]**: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` (+`OPENAI_BASE_URL`), `GOOGLE_API_KEY`/`GEMINI_API_KEY`, `XAI_API_KEY`, `GLM_API_KEY`, `KIMI_API_KEY`/`KIMI_CN_API_KEY`, `MINIMAX_API_KEY`/`MINIMAX_CN_API_KEY`, `DASHSCOPE_API_KEY`, `HF_TOKEN`, `XIAOMI_API_KEY`, `KILOCODE_API_KEY`, `AI_GATEWAY_API_KEY`, `OPENCODE_ZEN_API_KEY`, `FIREWORKS_API_KEY`, `NOVITA_API_KEY`, `ARCEEAI_API_KEY`, `GMI_API_KEY`, `ACTUAL_API_KEY`, `TOKENHUB_API_KEY`, `COMMANDCODE_API_KEY`, `STEPFUN_API_KEY`, `NVIDIA_API_KEY`, `COPILOT_GITHUB_TOKEN`, voice: `ELEVENLABS_API_KEY`, `VOICE_TOOLS_OPENAI_KEY`, `MISTRAL_API_KEY`, STT: `GROQ_API_KEY`.
- Behavior overrides **[L]**: `HERMES_INFERENCE_MODEL`, `HERMES_YOLO_MODE=1`, `HERMES_ACCEPT_HOOKS=1`, `TERMINAL_ENV`, `TERMINAL_TIMEOUT`, `TERMINAL_LIFETIME_SECONDS`, `TERMINAL_MODAL_IMAGE`, legacy `HERMES_API_TIMEOUT`/`HERMES_API_CALL_STALE_TIMEOUT` (superseded by `providers.<id>.request_timeout_seconds`) **[D]**.
- Rule of thumb **[D]**: `.env` is for secrets only; all behavioral settings belong in `config.yaml` (upstream AGENTS.md rejects new non-secret `HERMES_*` vars) **[S]**.

### Profiles **[L]**

- Directory layout `~/.hermes/profiles/<name>/` mirrors `$HERMES_HOME` (config.yaml, .env, SOUL.md, memories/, skills/, cron/, sessions/, logs/) — same layout documented **[D]**. *Note:* on THIS install no profiles have been created yet (`profiles/` dir absent; `hermes profile list` shows only `◆default` with its gateway running) — layout claim is from docs + `profile create` behavior.
- CLI: `hermes -p/--profile NAME` (global flag, verified in completion script + gateway spawn code) **[S]**; sticky default via `hermes profile use NAME`.
- Management: `hermes profile create [--clone|--clone-all|--clone-from SRC|--no-alias|--no-skills|--description]`, `use`, `delete`, `show`, `alias`, `rename`, `export` (tar.gz), `import`; `hermes gateway list` shows per-profile gateway status **[L]**.

---

## 3. Third-party provider registration (custom endpoints)

Current format — keyed `providers:` dict in `config.yaml` **[D]** ([Named Custom Providers](https://hermes-agent.nousresearch.com/docs/integrations/providers#named-custom-providers)); schema cross-checked against `_normalize_custom_provider_entry()` in local `hermes_cli/config.py` **[S]**:

```yaml
providers:
  local-vllm:                      # any name you choose
    api: http://localhost:8080/v1  # base URL ('base_url'/'url' aliases OK)
    # api_key omitted → "no-key-required" for keyless local servers
  work-gpu:
    api: https://gpu-server.internal.corp/v1
    key_env: CORP_API_KEY          # read from .env (or inline api_key:)
    transport: chat_completions    # OpenAI-compatible wire
  anthropic-proxy:
    api: https://proxy.example.com/anthropic
    key_env: ANTHROPIC_PROXY_KEY
    transport: anthropic_messages  # Anthropic-compatible proxy
  corp-broker:
    base_url: https://gateway.internal.example.com/v1
    api_mode: chat_completions     # legacy alias of transport
    key_cmd: "gcloud auth print-access-token"   # short-lived token minting
```

Accepted per-entry keys (union of docs + source `_KNOWN_KEYS`) **[S][D]**: `api`/`base_url`/`url`, `name`, `key_env` (aliases `api_key_env`, `keyEnv`, `apiKeyEnv`), inline `api_key`, `key_cmd` (prints bare token or JSON `access_token`+`expires_in`; cached till near expiry; beats static key), `api_mode`/`transport` (`chat_completions` | `anthropic_messages` | `codex_responses`), `default_model` (legacy `model`), `models`, `models_discovered`, `discover_models`, `context_length`, `rate_limit_delay`, `request_timeout_seconds`, `stale_timeout_seconds`, `extra_body`, `extra_headers`, `ssl_ca_cert`, `ssl_verify`, `enabled: false` to hide without deleting. camelCase keys auto-map with a one-time warning.

Legacy format: top-level `custom_providers:` **list** still readable; `hermes update` migrates it to `providers:` dict (config v12) **[D]**; runtime merges both views deduplicated (`get_compatible_custom_providers`) **[S]**. Wizard path: `hermes model` → "Custom endpoint" saves here **[D]**. Self-hosted recipes (Ollama, vLLM, SGLang, llama.cpp, LM Studio, LiteLLM) on the Providers page **[D]**.

---

## 4. Multi-instance wrappers

Three isolation mechanisms compose:

1. **Profiles** (same install, isolated state): `hermes -p work chat` / `HERMES_PROFILE=work hermes gateway start`. Each profile gets own config.yaml/.env/sessions/memory/skills; `--clone-from` copies an existing one; optional shell alias per profile (`hermes profile alias`, skipped with `--no-alias`). Gateway runs per profile — restart each after edits **[L]**.
2. **Separate `HERMES_HOME` instances** (fully separate data roots, can even pin different checkouts): `HERMES_HOME=/srv/hermes-a hermes gateway run`. `HERMES_HOME`/`HERMES_PROFILE` cannot be set through `hermes config set` — export them before launch **[S]**.
3. **Per-invocation overrides**: `-m MODEL --provider P --reasoning LEVEL -t TOOLSETS` apply to `-z` one-shot and TUI runs **[L]**.

**Gateway restart after edits:** config/tool changes require `hermes gateway restart` (service mode; also `/restart` slash command from chat) or process relaunch in foreground mode; tool changes additionally need a fresh session (`/new`) because toolsets snapshot per-conversation **[L]**.

Wrapper script example (two independent instances, different providers) — flags verified against local `hermes --help`:

```bash
#!/usr/bin/env bash
# /usr/local/bin/hermes-work — work instance: Anthropic primary, OpenRouter fallback
export HERMES_HOME="$HOME/.hermes-instances/work"
exec hermes "$@"                    # config.yaml in $HERMES_HOME sets:
                                    #   model: {default: claude-sonnet-4-6, provider: anthropic}
                                    #   fallback_providers: [{provider: openrouter, model: anthropic/claude-sonnet-4}]

#!/usr/bin/env bash
# /usr/local/bin/hermes-local — cheap local instance: vLLM endpoint, no cloud
export HERMES_HOME="$HOME/.hermes-instances/local"
exec hermes --provider custom "$@"  # providers.local-vllm.api: http://localhost:8080/v1
```

First-run each home once (`HERMES_HOME=… hermes setup`), then install each gateway: `HERMES_HOME=… hermes gateway install && … gateway restart` after any config edit **[L]**. For parallel agents in one repo, `-w/--worktree` gives isolated git worktrees instead of separate homes **[L]**.

---

## 5. Skills, memory, cron delivery, plugins

**Skills** **[L/D]** — installed under `$HERMES_HOME/skills/` (bundled ones ship in-repo). CLI: `hermes skills list|search|install ID|inspect|uninstall|update|check|browse|publish|tap add REPO|config` (per-platform enablement); hub supports direct SKILL.md URLs; `hermes bundles` makes multi-skill aliases; Skill Sync across devices (`hermes sync`). Extra scan dirs via `skills.external_dirs` **[L]**. Agent-created skills carry `created_by: agent` provenance and are maintained by the **curator** (`hermes curator status/run/pause/pin/archive/restore/prune/backup/rollback`; never deletes, archives only; telemetry sidecar `skills/.usage.json`) **[L]**.

**Memory stores** **[L/S]** — built-in memory files (`memories/MEMORY.md`, `USER.md`) configured under `memory:` (§1.7). Pluggable backends live in `plugins/memory/`: **honcho, mem0, hindsight, holographic, supermemory, byterover, openviking, retaindb** (directories verified locally). Configure with `hermes memory setup/status/off`; Honcho has its own `hermes honcho setup/status` + `honcho:` config section **[S]**.

**Cron deliver targets** **[L]** — `deliver` field accepts `"origin"` (chat/channel where the job was created — the default when origin exists), `"local"` (store result locally), or a platform name such as `"telegram"`/`"discord"` (docstring: `'origin', 'local', 'telegram', etc.`). Per-job knobs verified in `cron/jobs.py`: `skills[]`, `model/provider/base_url` override, `script` (stdout feeds job; `no_agent=True` = script IS the job), `context_from` (chain job outputs), `workdir`, `enabled_toolsets`, `repeat`. Jobs file under `$HERMES_HOME/cron/`; CLI `hermes cron list/add/edit/pause/resume/run/remove/status`. Delivery failures tracked separately from agent errors; `cron.wrap_response: true` frames delivered output **[S]**.

**Plugins** **[L/S]** — bundled plugin dirs observed in `plugins/`: `browser, context_engine, cron_providers, dashboard_auth, disk-cleanup, google_meet, hermes-achievements, image_gen, kanban, memory, model-providers, observability, platforms, security-guidance, spotify, teams_pipeline, video_gen, web`. User plugins install into `~/.hermes/plugins/` (third-party products ship as standalone repos, not in core tree — upstream policy) **[S]**. Manage with `hermes plugins list/install/remove`; plugins can register toolsets (locally `a2a`, `spotify` came from plugins).

---

## 6. Multi-platform gateways

One gateway process serves many platforms; adapters verified in local source `gateway/platforms/` **[S]**: `api_server, bluebubbles, qqbot/, signal, webhook, weixin, whatsapp_cloud, yuanbao` (+ core telegram/discord/slack/matrix/mattermost/email adapters elsewhere in the gateway package). Documented platform set **[D]** ([Messaging](https://hermes-agent.nousresearch.com/docs/user-guide/messaging/)): Telegram, Discord, Slack, WhatsApp (Cloud API), Signal, Matrix, Mattermost, Email, SMS, Home Assistant, DingTalk, Feishu, WeCom, BlueBubbles (iMessage), Weixin, QQ Bot, Yuanbao, API Server, Webhooks; Open WebUI connects via API Server adapter.

Service lifecycle **[L]**: `hermes gateway run` (foreground), `install/start/stop/restart/status/uninstall` (systemd/launchd), `list` (per-profile status), `setup` (platform wizard), `enroll` (relay connector writes creds to .env). Runtime state on this box: `gateway.pid`, `gateway.lock`, `gateway_state.json`, logs in `~/.hermes/logs/gateway.log` **[L]**.

Platform gotchas **[D]**: Discord needs Message Content Intent; Slack needs `message.channels` event subscription; WSL2 needs systemd for persistent service. Platform behavior knobs: §1.4 discord block + `slack/mattermost/matrix/telegram` siblings (`require_mention`, `allowed_channels/rooms/chats`, `free_response_*`, `reactions`) **[L]**. Extras: DM pairing codes (`hermes pairing`), peer-to-peer bot DMs (`hermes peer`), cross-platform send from scripts/cron (`hermes send`) **[L]**.

---

## Key doc URLs cited

- Configuration reference: https://hermes-agent.nousresearch.com/docs/user-guide/configuration
- Providers & custom endpoints & fallback: https://hermes-agent.nousresearch.com/docs/integrations/providers
- Messaging platforms: https://hermes-agent.nousresearch.com/docs/user-guide/messaging/
- Fallback details: https://hermes-agent.nousresearch.com/docs/user-guide/features/fallback-providers
- Repo README/source: https://github.com/NousResearch/hermes-agent (local checkout `~/.hermes/hermes-agent`)
- Env-vars reference: https://hermes-agent.nousresearch.com/docs/reference/environment-variables
