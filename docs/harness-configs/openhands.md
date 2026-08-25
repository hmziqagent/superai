# OpenHands (All Hands AI) — Configurable Options

Compiled 2026-08-25. Primary sources:
- Legacy TOML schema: [`config.template.toml` @ tag `0.44.0`](https://raw.githubusercontent.com/All-Hands-AI/OpenHands/0.44.0/config.template.toml) — the file no longer exists on `main`; the project moved to a **V1 config model** ([current configuration-options page](https://docs.all-hands.dev/usage/configuration-options)).
- Current docs: [docs.all-hands.dev / docs.openhands.dev](https://docs.openhands.dev/llms.txt) (index). Legacy V0 pages are now parked under *Web → Legacy (V0)* and excluded from the main index.

**Versioning note:** everything below labeled **V0** describes the classic `config.toml` system still used by OpenHands ≤ 0.4x and referenced throughout community material. **V1** (current) moves most config to the Settings UI, `~/.openhands/agent_settings.json`, and environment variables ([V1 config docs](https://docs.all-hands.dev/usage/configuration-options)).

---

## 1. `config.toml` schema (V0)

Loaded from `./config.toml` (working directory) or `~/.openhands/config.toml`. All keys optional — every field has a default ([template](https://raw.githubusercontent.com/All-Hands-AI/OpenHands/0.44.0/config.template.toml)). Sections are matched by prefix: bare `[llm]` / `[agent]` = default; `[llm.gpt3]`, `[agent.CodeActAgent]` = named variants.

### `[core]`
| Key | Default | Purpose |
|---|---|---|
| `workspace_base` | `./workspace` | Host path mounted into the agent sandbox |
| `workspace_mount_path_in_sandbox` | `/workspace` | Mount point inside the container |
| `max_iterations` | `250` | Max agent steps per task |
| `max_budget_per_task` | `0.0` | USD cap; 0 = unlimited |
| `runtime` | `"docker"` | Sandbox backend (`docker` \| `local` \| `remote` \| `e2b` \| `modal` \…) |
| `default_agent` | `CodeActAgent` | Agent used when none specified |
| `debug` | `false` | Verbose logging |
| `disable_color` | `false` | Strip ANSI color |
| `cache_dir` | `/tmp/cache` | |
| `file_store` / `file_store_path` | `memory` / `/tmp/file_store` | Event persistence |
| `save_trajectory_path` / `replay_trajectory_path` | unset | Record / replay trajectories |
| `save_screenshots_in_trajectory` | `false` | Bloats trajectory JSON |
| `run_as_openhands` | `true` | Non-root user in sandbox |
| `reasoning_effort` | unset | For o-series models (`low`/`medium`/`high`) |
| `jwt_secret` | `""` | Auth signing (GUI deployments) |
| `file_uploads_max_file_size_mb`, `file_uploads_restrict_file_types`, `file_uploads_allowed_extensions` | `0`, `false`, `[".*"]` | Upload guards |
| `enable_default_condenser` | `true` | Use LLM-summarizing condenser when none configured |
| `max_concurrent_conversations` / `conversation_max_age_seconds` | `3` / `864000` | Multi-user GUI limits |
| `e2b_api_key`, `modal_api_token_id/secret`, `daytona_api_key`, `daytona_target` | `""` | Remote-runtime credentials |

### `[llm]` blocks
Bare `[llm]` is the default; any `[llm.<name>]` inherits all defaults and overrides selectively ([Custom LLM Configurations](https://docs.openhands.dev/openhands/usage/llms/custom-llm-configs)). Key fields ([template](https://raw.githubusercontent.com/All-Hands-AI/OpenHands/0.44.0/config.template.toml)):

```toml
[llm]
model = "gpt-4o"                # litellm-style id; "openai/<model>" for custom endpoints
api_key = ""                    # Headless/CLI only — overridden by Session Init in the Web GUI
base_url = ""                   # OpenAI-compatible endpoint (Headless/CLI only)
api_version = ""                # e.g. Azure api-version
temperature = 0.0
top_p = 1.0
max_input_tokens = 0            # 0 = model default
max_output_tokens = 0
max_message_chars = 10000       # truncation of observation content
caching_prompt = true           # provider prompt caching
num_retries = 8                 # retry_min_wait 15s / retry_max_wait 120s / retry_multiplier 2.0
timeout = 0
drop_params = false             # silently drop params unsupported by the provider
modify_params = true            # let litellm fix malformed messages
custom_llm_provider = ""        # force a litellm provider for unknown prefixes
ollama_base_url = ""            # dedicated Ollama endpoint
disable_vision = false          # skip image payloads even for vision models
native_tool_calling = None      # true/false/None (auto-evaluated)
input_cost_per_token / output_cost_per_token = 0.0   # cost accounting
aws_access_key_id / aws_secret_access_key / aws_region_name  # Bedrock
custom_tokenizer = ""           # for token counting
```

Named blocks + agent binding:

```toml
[llm.repo-explorer]
model = "gpt-3.5-turbo"
temperature = 0.2

[agent.RepoExplorerAgent]
llm_config = 'repo-explorer'    # case-sensitive agent name
```

Reserved name `[llm.draft_editor]` sets the model used for draft-edit prefilling. ⚠️ Per the docs: *"Custom LLM configurations are only available when using OpenHands in development mode, via `main.py` or `cli.py`. When running via `docker run`, please use the standard configuration options"* ([custom-llm-configs](https://docs.openhands.dev/openhands/usage/llms/custom-llm-configs)).

**Embeddings:** the 0.44.0 template contains no `[embedding]` table; embedding behavior (used by the repo-memory feature) rides on the same LiteLLM layer — an `openai/*` embedding model pointed at an OpenAI-compatible `base_url` works, and the legacy V0 docs cover `EMBEDDING_*` variables. Treat exact embedding keys as version-dependent.

### `[agent]`
Tool toggles and prompt behavior ([template](https://raw.githubusercontent.com/All-Hands-AI/OpenHands/0.44.0/config.template.toml)): `enable_browsing`, `enable_jupyter`, `enable_cmd`, `enable_think`, `enable_finish`, `enable_editor` (str_replace_editor), `enable_llm_editor` (all default sensible), `enable_prompt_extensions` (microagents), `disabled_microagents = []`, `enable_history_truncation = true`, `llm_config = '<named llm block>'`.
Per-agent sections `[agent.<AgentName>]` (case-sensitive) select LLM blocks or register external agents: `classpath = "my_package.my_module.MyCustomAgent"`.

### `[sandbox]` (Docker runtime)
| Key | Default | Notes |
|---|---|---|
| `timeout` | `120` | Command timeout (s) |
| `user_id` | `1000` | UID inside container |
| `base_container_image` | `nikolaik/python-nodejs:python3.12-nodejs22` | Base for the built runtime image |
| `runtime_container_image` | auto-built | Prebuilt alternative (skip build) |
| `volumes` | — | `'host:container[:mode]'` comma-separated mounts |
| `use_host_network` | `false` | Host networking (Linux; security tradeoff) |
| `runtime_extra_build_args` | — | e.g. `["--network=host", "--add-host=host.docker.internal:host-gateway"]` |
| `runtime_extra_deps` / `runtime_startup_env_vars` | — | Pip deps / env baked at launch |
| `enable_auto_lint`, `initialize_plugins`, `enable_gpu` | `false`, `true`, `false` | |
| `platform`, `force_rebuild_runtime` | `""`, `false` | Image build control |
| `keep_runtime_alive`, `pause_closed_runtimes`, `close_delay`, `rm_all_containers` | lifecycle knobs | |
| `docker_runtime_kwargs`, `vscode_port` | `{}` / random | |

**BrowserGym:** in 0.44.0 the eval knob lives here as `browsergym_eval_env = ""` under `[sandbox]` (older releases grouped it in its own `[browsergym]` section — same key).

Adjacent sections worth knowing: `[security]` (`confirmation_mode`, `security_analyzer` — headless/CLI only), `[condenser]` (history compression: `noop`, `observation_masking`, `recent`, `llm`, `amortized`, `llm_attention` + `[llm.condenser]` helper block), `[eval]`.

### Env-var equivalents (V0 rule)
Any `config.toml` value can be overridden with an env var named `<SECTION>_<KEY>` in upper case — e.g. `WORKSPACE_BASE`, `MAX_ITERATIONS`, `RUNTIME`, `LLM_API_KEY`, `SANDBOX_USER_ID`, `SANDBOX_TIMEOUT`. The full mapping is documented on the legacy *V0 Configuration Options* page (now under Web → Legacy (V0)); the V1 page confirms the surviving subset below ([source](https://docs.all-hands.dev/usage/configuration-options)).

---

## 2. Environment variables

**Current V1 set** ([V1 configuration-options](https://docs.all-hands.dev/usage/configuration-options)):
- LLM credentials: `LLM_API_KEY`, `LLM_MODEL` (with the CLI they require `openhands --override-with-envs`; `LLM_BASE_URL` likewise — [command reference](https://docs.openhands.dev/openhands/usage/cli/command-reference))
- Persistence: `OH_PERSISTENCE_DIR` (default `~/.openhands`)
- Public URL: `OH_WEB_URL`
- Sandbox mounts: `SANDBOX_VOLUMES` → [Docker Sandbox guide](https://docs.openhands.dev/openhands/usage/sandboxes/docker)
- Sandbox image pinning: `AGENT_SERVER_IMAGE_REPOSITORY`, `AGENT_SERVER_IMAGE_TAG` (e.g. `ghcr.io/openhands/agent-server` / `1.26.0-python`)
- Reverse-proxy networking: `SANDBOX_CONTAINER_URL_PATTERN` / `OH_SANDBOX_CONTAINER_URL_PATTERN` (default `http://localhost:{port}`), `AGENT_SERVER_USE_HOST_NETWORK`
- Provider selection (legacy-compatible): `RUNTIME=docker` (default) | `process` (= old `local`) | `remote`

**LLM-block prefixes (V0):** each named `[llm.x]` maps to `LLM_X_<FIELD>`-style uppercase vars under the same section-key rule; the plain `[llm]` block reads `LLM_MODEL`, `LLM_API_KEY`, `LLM_BASE_URL`, `LLM_TEMPERATURE`, etc. (same names V1 kept for the three core ones).

**Sandbox/runtime (V0):** `SANDBOX_TIMEOUT`, `SANDBOX_USER_ID`, `SANDBOX_BASE_CONTAINER_IMAGE`, `SANDBOX_VOLUMES`, plus `RUNTIME`. Install docs commonly pass `-e SANDBOX_USER_ID=$(id -u)`.

**Debug:** `core.debug` in TOML; the `DEBUG` environment variable turns on debug logging across the stack (referenced by OpenHands' troubleshooting guidance). CLI additionally exposes `openhands web --debug`.

**CLI-specific:** `OPENHANDS_CLOUD_URL` (default cloud endpoint), `OPENHANDS_VERSION` (image tag for `openhands serve`) ([command reference](https://docs.openhands.dev/openhands/usage/cli/command-reference)).

---

## 3. Third-party & local models

Pattern for anything OpenAI-compatible: **`model = "openai/<served-model-name>"` + `base_url = "http(s)://host:<port>/v1"`**; `api_key` = real key or placeholder ([Local LLMs guide](https://docs.openhands.dev/openhands/usage/llms/local-llms)). OpenHands needs ≥ ~22k context (32k recommended) or even the system prompt won't fit.

Worked examples:

```bash
# Ollama — raise context, bind 0.0.0.0 so Docker containers can reach it
OLLAMA_CONTEXT_LENGTH=32768 OLLAMA_HOST=0.0.0.0:11434 OLLAMA_KEEP_ALIVE=-1 ollama serve &
ollama pull qwen3.6:35b-a3b
# -> model: openai/qwen3.6:35b-a3b   base_url: http://host.docker.internal:11434/v1   api_key: dummy
```
```bash
# vLLM / SGLang — serve on :8000 with an API key
python -m vllm.entrypoints.openai.api_server --model Qwen/Qwen3.6-35B-A3B --api-key mykey
# -> model: openai/Qwen3.6-35B-A3B   base_url: http://host.docker.internal:8000/v1   api_key: mykey
```
- **LM Studio** (recommended quickstart): serve on port `1234`, must enable *Serve on Local Network* (bind `0.0.0.0`) when OpenHands runs in Docker; verify with `docker exec -it openhands-app curl -s http://host.docker.internal:1234/v1/models` ([guide](https://docs.openhands.dev/openhands/usage/llms/local-llms)).
- **Atomic Chat**: desktop server on port `1337` → `base_url: http://127.0.0.1:1337/v1`, placeholder key.
- **LiteLLM proxy** (unify many providers behind one gateway): `litellm --model <any-litellm-id> --port 4000` → `model = "openai/<alias>"`, `base_url = "http://localhost:4000/v1"`, `api_key = "sk-1234"` (standard OpenAI-compat shape; not on the local-LLMs page itself).
- **OpenRouter** (cloud aggregator): `model = "openai/anthropic/claude-sonnet-4"`, `base_url = "https://openrouter.ai/api/v1"`, `api_key = "<OPENROUTER_KEY>"`; add `custom_llm_provider`/`drop_params = true` if a provider rejects OpenHands-specific params (standard OpenRouter OpenAI-compat usage).
- Recommended local model (2026): Qwen3.6-35B-A3B (24GB VRAM quantized / 64GB Apple Silicon). Community notes: weak local models degrade to chatbot-like tool-less behavior — try `qwen2.5-coder-14b-instruct` if tool calls fail ([guide](https://docs.openhands.dev/openhands/usage/llms/local-llms)).
- **Local embeddings:** point the embedding model at the same style of endpoint (`openai/<embed-model>` against an OpenAI-compatible `/v1/embeddings`, e.g. served by Ollama/vLLM/LM Studio); the 0.44 template carries no dedicated `[embedding]` table, so exact keys vary by release — check your installed version's `config.template.toml`.

---

## 4. Multi-instance wrappers

**Honest status of `-c/--config-file`:** the current CLI command reference lists **no** `--config-file` global option ([command reference](https://docs.openhands.dev/openhands/usage/cli/command-reference)). Options for per-run configuration that *are* documented:

1. **Env-var overrides per invocation** — cleanest for wrappers: `LLM_MODEL`, `LLM_API_KEY`, `LLM_BASE_URL` + `--override-with-envs` (not persisted) ([command reference](https://docs.openhands.dev/openhands/usage/cli/command-reference)).
2. **Per-directory `config.toml` (V0)** — V0 loads `./config.toml` before `~/.openhands/config.toml`, so `cd` into a project holding its own config gives you per-project instances.
3. **Per-user settings file (V1)** — `~/.openhands/agent_settings.json` holds `{"llm": {"model": ..., "api_key": ..., "base_url": ...}}`; point `HOME`/`OH_PERSISTENCE_DIR` elsewhere per instance to isolate them ([command reference](https://docs.openhands.dev/openhands/usage/cli/command-reference), [V1 config](https://docs.all-hands.dev/usage/configuration-options)).
4. **Docker isolation** — one container per instance with distinct `--name`, `-p` port, `-v` workspace mount, shared `/var/run/docker.sock` for sandbox spawning; pin `AGENT_SERVER_IMAGE_REPOSITORY`/`_TAG`; mount `~/.openhands` for state ([local setup / local-LLM guide](https://docs.openhands.dev/openhands/usage/llms/local-llms)).

Wrapper script sketch — two providers side by side (GUI-in-Docker instance + local-model instance):

```bash
#!/usr/bin/env bash
# oh-wrapper: run OpenHands against a chosen provider without touching saved settings
set -euo pipefail

case "${PROVIDER:?PROVIDER=openrouter|ollama}" in
  openrouter)
    export LLM_MODEL="openai/anthropic/claude-sonnet-4"
    export LLM_BASE_URL="https://openrouter.ai/api/v1"
    export LLM_API_KEY="${OPENROUTER_API_KEY:?}"
    ;;
  ollama)
    export LLM_MODEL="openai/qwen3.6:35b-a3b"
    export LLM_BASE_URL="http://host.docker.internal:11434/v1"
    export LLM_API_KEY="dummy"
    ;;
esac

if [[ "${MODE:-cli}" == "gui" ]]; then
  # isolated GUI instance: unique name/port/workspace, pinned agent-server image
  exec docker run -it --rm --pull=always \
    -e AGENT_SERVER_IMAGE_REPOSITORY=ghcr.io/openhands/agent-server \
    -e AGENT_SERVER_IMAGE_TAG=1.26.0-python \
    -e LOG_ALL_EVENTS=true \
    -e LLM_MODEL="$LLM_MODEL" -e LLM_BASE_URL="$LLM_BASE_URL" -e LLM_API_KEY="$LLM_API_KEY" \
    -v /var/run/docker.sock:/var/run/docker.sock \
    -v "${PWD}":/workspace -w /workspace \
    -p "${PORT:-3000}":3000 \
    --add-host host.docker.internal:host-gateway \
    --name "openhands-${PROVIDER}-${PORT:-3000}" \
    docker.openhands.dev/openhands/openhands:1.8
else
  # headless CLI run with non-persisted env overrides
  exec openhands --headless --override-with-envs "$@"
fi
# usage: PROVIDER=ollama PORT=3001 MODE=headless ./oh-wrapper.sh -f task.txt
```

(For V0 installs, swap step 1 for `cd`-into-project-dir with that project's own `config.toml`.)

---

## 5. CLI vs GUI vs headless config differences

| Aspect | CLI (terminal) | GUI (web/Docker) | Headless (`--headless`) |
|---|---|---|---|
| Primary config store | `~/.openhands/agent_settings.json` + Settings palette (`Ctrl+P` → Settings; no restart needed) | Same settings UI in browser; first-run LLM setup wizard | Same settings file, but env overrides shine |
| `[llm]` `api_key`/`model`/`base_url` in `config.toml` | Honored ("Headless / CLI only") | **Overridden by Session Init** — values come from the logged-in session settings (explicit comment in the template) | Honored |
| Approval flow | `--always-approve` / `--llm-approve` toggle | Confirmation mode from `[security]`/session | **Always approve, cannot be changed** (`--llm-approve` unavailable) ([headless docs](https://docs.openhands.dev/openhands/usage/cli/headless)) |
| Task input | Interactive chat; `-t` seeds | Chat box | Requires `--task`/`--file`; `--json` emits JSONL events for CI ([headless docs](https://docs.openhands.dev/openhands/usage/cli/headless)) |
| Named `[llm.*]` blocks / `[agent.*]` | Dev-mode only (`main.py`/`cli.py`); not via `docker run` ([custom-llm-configs](https://docs.openhands.dev/openhands/usage/llms/custom-llm-configs)) | Not applicable (session settings) | Dev-mode only |
| Other config files | `~/.openhands/cli_config.json` (prefs), `mcp.json`, `conversations/` | Server-managed; `OH_PERSISTENCE_DIR` | Exit codes 0/1/2 for scripting |
| Launch | `openhands [-t ...]`, subcommands `serve` (GUI via Docker, `--mount-cwd`, `--gpu`), `web`, `acp`, `cloud`, `mcp` | `openhands serve` or plain `docker run … :3000` | `openhands --headless -t/-f [--json]` ([command reference](https://docs.openhands.dev/openhands/usage/cli/command-reference)) |

---

## 6. Software Agent SDK note

OpenHands is splitting into (a) the product (Web app / Cloud / Enterprise / CLI) documented above, and (b) the **OpenHands Software Agent SDK** — a separate Python framework ([github.com/OpenHands/software-agent-sdk](https://github.com/OpenHands/software-agent-sdk); docs index: [docs.openhands.dev/sdk](https://docs.openhands.dev/llms.txt)). Key implications for configuration:

- No `config.toml`: LLM/agent/sandbox settings are **Python objects** — `LLM(model=..., base_url=..., api_key=...)`, `Agent(...)`, `Conversation(...)` ([Hello World](https://docs.openhands.dev/sdk/guides/hello-world), [Agent Settings](https://docs.openhands.dev/sdk/guides/agent-settings)).
- Reusable profiles replace named TOML blocks: the **LLM Profile Store** saves/loads named LLM configs programmatically ([guide](https://docs.openhands.dev/sdk/guides/llm-profile-store)); **LLM Registry** does dynamic routing ([guide](https://docs.openhands.dev/sdk/guides/llm-registry)).
- Sandboxes are pluggable classes: Docker, Apptainer (HPC), remote API-based, cloud workspace ([SDK sandboxes](https://docs.openhands.dev/sdk/guides/agent-server/docker-sandbox)).
- Execution happens in an **Agent Server** (local or remote) configured via its own env/REST interface rather than TOML ([Agent Server docs](https://docs.openhands.dev/sdk/guides/agent-server/local-server)).

If you're scripting fresh integrations, prefer the SDK; the `config.toml` surface above applies to the shipped product (≤ V0) and legacy workflows.
