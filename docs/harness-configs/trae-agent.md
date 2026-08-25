# Trae Agent (ByteDance) — Complete Configuration Reference

**Repo:** https://github.com/bytedance/trae-agent · **Doc snapshot:** main @ `e839e55` (commit dated Feb 5, 2026; repo last active then). Python ≥3.12, MIT license.
**Primary sources cited inline as:** [README] `README.md`, [yaml.example] `trae_config.yaml.example`, [json.example] `trae_config.json.example`, [legacy.md] `docs/legacy_config.md`, [cli.py] `trae_agent/cli.py`, [config.py] `trae_agent/utils/config.py`, [eval-README] `evaluation/README.md`, [traj.md] `docs/TRAJECTORY_RECORDING.md`, [tools.md] `docs/tools.md`, [pyproject] `pyproject.toml`.

---

## 1. Config file

### 1a. YAML (current, recommended): `trae_config.yaml`

- Created via `cp trae_config.yaml.example trae_config.yaml` in the repo root [README]. The file is git-ignored to protect API keys [README].
- Default lookup path is the **CWD**: `--config-file` defaults to `"trae_config.yaml"` on every CLI command (`run`, `interactive`, `show-config`) [cli.py].
- If a `.yaml` path is given but missing, the CLI silently falls back to the sibling `.json` file (backward compat); if neither exists it exits with an error telling you to pass `--config-file` [cli.py `resolve_config_file()`].
- A config file ending in `.json` is parsed by the legacy JSON loader instead of YAML [config.py `Config.create`].
- Formatting rule: spaces only — tabs are not allowed in the YAML file [README].

Full schema (all keys from `trae_config.yaml.example` + dataclasses in `config.py`):

```yaml
agents:
  trae_agent:                      # only agent name accepted; unknown names raise ConfigError [config.py]
    enable_lakeview: true          # bool, default true [config.py TraeAgentConfig]; if true, a lakeview section is REQUIRED
    model: trae_agent_model        # REQUIRED; name of an entry under `models:` [config.py]
    max_steps: 200                 # int; agent step budget. Example uses 200; interactive --max-steps default is 20 [cli.py]
    tools:                         # list[str], default: bash, str_replace_based_edit_tool,
      - bash                       #   sequentialthinking, task_done [config.py TraeAgentConfig];
      - str_replace_based_edit_tool#   see docs/tools.md for all 5 built-ins (incl. json_edit_tool)
      - sequentialthinking
      - task_done

allow_mcp_servers:                 # optional list[str]; MCP server names the agent may use [config.py]
  - playwright
mcp_servers:                       # optional map of stdio MCP servers [README / yaml.example]
  playwright:
    command: npx                   # MCPServerConfig fields [config.py]: command, args, env, cwd (stdio);
    args:                          #   url (SSE); http_url + headers (streamable HTTP); tcp (websocket);
      - "@playwright/mcp@0.0.27"   #   timeout, trust, description (common)

lakeview:                          # REQUIRED iff any agent has enable_lakeview: true [config.py]
  model: lakeview_model            # name of an entry under `models:`; omitting → ConfigError

model_providers:                   # REQUIRED non-empty [config.py]; key = provider instance name
  anthropic:
    api_key: your_anthropic_api_key
    provider: anthropic            # one of: openai|anthropic|google|azure|ollama|openrouter|doubao
    base_url: <optional>           # custom OpenAI-compatible endpoint (see §3)
    api_version: <optional>        # required for azure [config.py ModelProvider docstring]
  openai:
    api_key: your_openai_api_key
    provider: openai

models:                            # REQUIRED non-empty [config.py]; key = reusable model-profile name
  trae_agent_model:
    model_provider: anthropic      # must match a model_providers key, else ConfigError
    model: claude-sonnet-4-20250514
    max_tokens: 4096               # int | null; fallback default 4096 if unset [config.py get_max_tokens_param]
    temperature: 0.5               # float
    top_p: 1                       # float
    top_k: 0                       # int (Anthropic-style; shown in show-config only for anthropic [cli.py])
    max_retries: 10                # int
    parallel_tool_calls: true      # bool
    supports_tool_calling: true    # bool, default true [config.py]
    candidate_count: <optional>    # Gemini-specific [config.py]
    stop_sequences: <optional>     # list[str] [config.py]
    max_completion_tokens: <optional>  # Azure OpenAI-specific; takes priority over max_tokens;
                                   #   used instead of max_tokens for azure+gpt-5/o3/o4-mini [config.py]
  lakeview_model:                  # second profile used by Lakeview summarizer
    model_provider: anthropic
    model: claude-3.5-sonnet
    max_tokens: 4096
    temperature: 0.5
    top_p: 1
    top_k: 0
    max_retries: 10
    parallel_tool_calls: true
```

Hard validation rules enforced at load time [config.py `Config.create`]: `model_providers` required; `models` required; every `model_provider` reference must exist; every agent `model` reference must exist; `enable_lakeview: true` without a `lakeview:` section → ConfigError.

Inspect resolved config with `trae-cli show-config [--config-file …] [-p …] [-m …] [--max-steps N]` — prints General Settings (provider, max steps) and provider table (model, base URL, API version, masked API key, max tokens, temperature, top-p, top-k) [cli.py].

### 1b. Legacy JSON (deprecated): `trae_config.json`

Setup: `cp trae_config.json.example trae_config.json`; deprecated but still parsed whenever `--config-file` points at a `.json` path [legacy.md; config.py]. Flat schema — one settings block per provider (no separate `agents:`/`models:` layers) [json.example]:

```json
{
  "default_provider": "anthropic",       // which model_providers entry the agent uses
  "max_steps": 20,                        // note: legacy default/example is 20, YAML example is 200
  "enable_lakeview": true,
  "allow_mcp_servers": ["playwright"],
  "mcp_servers": { "playwright": { "command": "npx", "args": ["@playwright/mcp@0.0.27"] } },
  "lakeview_config": { "model_provider": null, "model_name": null },
  "model_providers": {
    "openai":     { "api_key": "...", "base_url": "https://api.openai.com/v1", "model": "gpt-4o",
                    "max_tokens": 128000, "temperature": 0.5, "top_p": 1, "max_retries": 10 },
    "anthropic":  { "api_key": "...", "base_url": "https://api.anthropic.com",
                    "model": "claude-sonnet-4-20250514", "max_tokens": 4096, "temperature": 0.5,
                    "top_p": 1, "top_k": 0, "max_retries": 10 },
    "google":     { "api_key": "...", "model": "gemini-2.5-flash", "max_tokens": 120000,
                    "temperature": 0.5, "top_p": 1, "top_k": 0, "max_retries": 10 },
    "azure":      { "api_key": "...", "base_url": "<your_azure_base_url>",
                    "api_version": "2024-03-01-preview", "model": "<name>", "max_tokens": 4096,
                    "temperature": 0.5, "top_p": 1, "top_k": 0, "max_retries": 10 },
    "ollama":     { "api_key": "ollama", "base_url": "http://localhost:11434/v1", ... },
    "openrouter": { "api_key": "...", "base_url": "https://openrouter.ai/api/v1",
                    "model": "openai/gpt-4o", ... },
    "doubao":     { "api_key": "...", "base_url": "<your_doubao_base_url>", "max_tokens": 8192,
                    "temperature": 0.5, "top_p": 1, "max_retries": 20 }
  }
}
```
(All keys shown; per-provider blocks accept `parallel_tool_calls`, `candidate_count`, `stop_sequences` additionally [config.py legacy conversion].) Legacy JSON maps internally to the new schema under the model name `"default_model"` [config.py `create_from_legacy_config`]. Migration guide: [legacy.md §Migration to YAML].

### 1c. Precedence

**Command-line arguments > Configuration file > Environment variables > Default values** [README; legacy.md; config.py `resolve_config_value`]. Env vars are consulted *only when the key is absent from both CLI and config file*.

---

## 2. Environment variables

Loaded automatically from a local `.env` via python-dotenv at CLI startup [cli.py `load_dotenv()`; pyproject dependency]. Documented set [README]:

```bash
export OPENAI_API_KEY="your-openai-api-key"
export OPENAI_BASE_URL="your-openai-base-url"
export ANTHROPIC_API_KEY="your-anthropic-api-key"
export ANTHROPIC_BASE_URL="your-anthropic-base-url"
export GOOGLE_API_KEY="your-google-api-key"
export GOOGLE_BASE_URL="your-google-base-url"
export OPENROUTER_API_KEY="your-openrouter-api-key"
export OPENROUTER_BASE_URL="https://openrouter.ai/api/v1"
export DOUBAO_API_KEY="your-doubao-api-key"
export DOUBAO_BASE_URL="https://ark.cn-beijing.volces.com/api/v3/"
```

Mechanics [config.py `resolve_config_values` on ModelConfig]: env var names are derived generically as `<PROVIDER>.upper() + "_API_KEY"` and `<PROVIDER>.upper() + "_BASE_URL"` — so `AZURE_API_KEY`/`AZURE_BASE_URL` work too even though not listed in the README. Env vars apply to api_key and base_url only (not temperature/max_steps etc.). Special CLI envvar binding: `TRAE_CONFIG_FILE` overrides the default config-file path for `run`, `interactive`, and `show-config` (declared via click `envvar=`) [cli.py]. Azure also needs its `api_version` from the config file (no env var documented).

---

## 3. Providers & models

Supported providers (clients in `trae_agent/utils/llm_clients/`): **OpenAI, Anthropic, Google Gemini, Azure OpenAI, Ollama, OpenRouter, Doubao** [README Features; tree listing]. Provider selection:

```bash
trae-cli run "Fix the bug" --provider openai     --model gpt-4o
trae-cli run "Add unit tests" --provider anthropic --model claude-sonnet-4-20250514
trae-cli run "Optimize this" --provider google   --model gemini-2.5-flash
trae-cli run "Review code"   --provider openrouter --model "anthropic/claude-3-5-sonnet"
trae-cli run "Refactor DB"   --provider doubao   --model doubao-seed-1.6
trae-cli run "Comment code"  --provider ollama   --model qwen3
```
[README Provider-Specific Examples]

**Custom / OpenAI-compatible base URL:** add `base_url` under the provider entry [README "Using Base URL"]:
```yaml
model_providers:
  openai:
    api_key: your_openrouter_api_key
    provider: openai
    base_url: https://openrouter.ai/api/v1
```
Any unrecognized `provider:` string can also be registered on the fly, provided `api_key` is supplied (via `--provider X --api-key … --model-base-url …`); otherwise ConfigError "To register a new model provider, an api_key should be provided" [config.py]. There is a dedicated `openai_compatible_base.py` client module underlying OpenAI-family providers [repo tree]. Ollama runs locally at `http://localhost:11434/v1` by default [json.example].

Per-model knobs (YAML `models:` entries): `model`, `model_provider`, `max_tokens`, `max_completion_tokens` (Azure), `temperature`, `top_p`, `top_k`, `max_retries`, `parallel_tool_calls`, `supports_tool_calling`, `candidate_count` (Gemini), `stop_sequences` — full list with semantics in §1a [config.py `ModelConfig`].

---

## 4. Multi-instance wrappers

**Config-file override flag exists:** every command accepts `--config-file PATH` (default `trae_config.yaml`, overridable by env `TRAE_CONFIG_FILE`) [cli.py]. This is the hook for running parallel instances with different providers/models/keys: give each worker its own YAML (or legacy JSON) file and point `--config-file` at it.

Complete non-interactive `run` option list (click declarations verbatim-summarized) [cli.py]:

| Flag | Short | Meaning |
|---|---|---|
| `TASK` (positional) | — | Task description string (or use `--file`) |
| `--file` | `-f` | Read task description from a file (mutually exclusive with TASK) |
| `--provider` | `-p` | LLM provider override |
| `--model` | `-m` | Model override |
| `--model-base-url` | — | Base URL override |
| `--api-key` | `-k` | API key inline (else env var) |
| `--max-steps` | — | Max execution steps (int) |
| `--working-dir` | `-w` | Agent working directory (made absolute; created if missing) |
| `--must-patch` | `-mp` | Flag: require a patch (`must_patch=true` task arg) |
| `--config-file` | — | Config path (default `trae_config.yaml`; env `TRAE_CONFIG_FILE`) |
| `--trajectory-file` | `-t` | Trajectory output path |
| `--patch-path` | `-pp` | Where to save the patch |
| `--docker-image` | — | Run inside a new container from image (mutually exclusive with the next three) |
| `--docker-container-id` | — | Attach to existing container (then `--working-dir` is invalid) |
| `--dockerfile-path` | — | Build environment from Dockerfile |
| `--docker-image-file` | — | Load image from local tar archive |
| `--docker-keep` | — | Keep container after run (default true) |
| `--console-type` | `-ct` | `simple` \| `rich` (default simple) |
| `--agent-type` | `-at` | Only `trae_agent` currently |

Other commands: `interactive` (`-p/-m/--model-base-url/-k/--config-file/--max-steps [default 20]/-t/-ct/-at`), `show-config` (`--config-file/-p/-m/--model-base-url/-k/--max-steps`), `tools` (list registered tools), global `--version` (0.1.0) [cli.py]. Console entry point is `trae-cli` = `trae_agent.cli:main` [pyproject `[project.scripts]`]; older blog posts show `trae run … --config-file trae-config-local.json` [trae.ai blog product_update_0625] — current entrypoint spelling is `trae-cli`.

**Wrapper script example** (N instances, distinct configs/workdirs/trajectories):

```bash
#!/usr/bin/env bash
# run_trae_fleet.sh — launch isolated trae-agent workers
set -euo pipefail
TASKS=($*)                     # one task string per argument
i=0
for task in "${TASKS[@]}"; do
  cfg="configs/worker_${i}.yaml"                  # each: own model/provider/api_key/max_steps
  out="runs/worker_${i}"
  mkdir -p "$out"
  TRAE_CONFIG_FILE="$cfg" \
  trae-cli run "$task" \
    --config-file "$cfg" \
    --working-dir "$(pwd)/workspaces/worker_${i}" \
    --trajectory-file "$out/trajectory.json" \
    --patch-path "$out/patch.diff" \
    --max-steps 200 \
    --console-type simple \
    --must-patch &
  i=$((i+1))
done
wait
```

Notes for wrappers: precedence means CLI flags always win over the YAML, so per-worker deltas can also be passed as flags against one shared config (`--model`, `--provider`, `--api-key`, `--max-steps`, `--model-base-url`); trajectories auto-name to `trajectories/trajectory_YYYYMMDD_HHMMSS.json` when `--trajectory-file` is omitted, so set it explicitly to avoid collisions across instances [cli.py; traj.md]. Docker-mode flags let each worker run in its own container for stronger isolation [cli.py; README].

---

## 5. SWE-bench tooling, Lakeview, trajectory

### SWE-bench evaluation harness (`evaluation/`)
Supports **SWE-bench, SWE-bench-Live, Multi-SWE-bench** [eval-README]. Setup: `uv sync --extra evaluation && cd evaluation && ./setup.sh [swe_bench|swe_bench_live|multi_swe_bench]` (clones benchmark harness at pinned commit, venv, install). Requires Docker + Python 3.12+. Datasets: `SWE-bench_Verified/Lite/full`; `SWE-bench-Lite/lite|verified|full`; `Multi-SWE-bench-flash` / `Multi-SWE-bench_mini` (JSONLs downloaded manually from Hugging Face into `evaluation/`).

Eval-agent config recommendation differs from default: `enable_lakeview: false`, `top_p: 0.9`, `top_k: 40`, `max_retries: 1`, `parallel_tool_calls: 1` in `trae_config.yaml` [eval-README §Configure Trae Agent]. Optional `docker_env_config.json` injects container env:
```json
{ "preparation_env": {"HTTP_PROXY": "..."}, "experiment_env": {"CUSTOM_VAR": "value"} }
```

Runner `run_evaluation.py` full flag set [eval-README]:
`--benchmark` (SWE-bench…), `--dataset` (e.g. SWE-bench_Verified), `--config-file ./trae_config.yaml` (Trae config override for eval), `--run-id experiment-1`, `--benchmark-harness-path ./SWE-bench` (required for eval modes), `--docker-env-config ./docker_env_config.json`, `--mode e2e|expr|eval` (expr=generate patches only, eval=score existing patches, e2e=default both), `--max_workers 4` (parallel workers — the eval-side multi-instance knob), `--instance_ids django__django-12345 …`. Outputs land in `results/{benchmark}_{dataset}_{run_id}/` (`predictions.json`, `results.json`, per-instance patch + trajectory JSON) plus `trae-workspace/` artifacts (`trae-agent.tar`, `uv.tar`, `uv_shared.tar`). A separate `evaluation/patch_selection/` subsystem (selector agent + sandboxed tools) picks best-of-n patches [repo tree; PR #291].

### Lakeview
Step-summarization feature: "short and concise summarisation for agent steps" [README Features]. Toggle per agent with `agents.trae_agent.enable_lakeview` (default `true`) [config.py]; its LLM is configured in the top-level `lakeview.model` key pointing at a named model profile (example uses `claude-3.5-sonnet`) [yaml.example]. Enabling without a lakeview section raises ConfigError [config.py]. Implementation: `trae_agent/utils/lake_view.py`, wired through the console layer (`ConsoleFactory.create_console(lakeview_config=…)`) [repo tree; cli.py].

### Trajectory recording
Always-on recording of raw LLM interactions (messages, responses, token usage incl. cache/reasoning tokens, tool calls), agent steps (state transitions, tool results, reflections, errors), and metadata (task, timestamps, provider, model, max_steps) into a single JSON file [traj.md]. Controlled solely by CLI: `--trajectory-file PATH` (or auto `trajectories/trajectory_YYYYMMDD_HHMMSS.json`); files written continuously during execution; `trajectories/` dir is git-excluded; API keys are not logged [traj.md File Management/Security]. Full JSON schema documented in [traj.md §Trajectory File Format] (root: `task/start_time/end_time/provider/model/max_steps/success/final_result/execution_time`; arrays: `llm_interactions[]`, `agent_steps[]`). Programmatic attach: `agent.setup_trajectory_recording(path)` [traj.md].

---

## Quick gotchas
- `max_steps` examples differ by surface: YAML example 200, legacy JSON example 20, `interactive --max-steps` default 20 [yaml.example; json.example; cli.py].
- Lakeview ON + missing `lakeview:` section = hard startup error; set `enable_lakeview: false` for headless/batch or eval runs [config.py; eval-README].
- YAML tabs are rejected [README]; JSON config only recognized if the path literally ends `.json` [config.py].
- Missing `.yaml` falls back to same-name `.json` silently (with a warning print) — beware stale JSON shadowing during migration [cli.py].
- Env vars never override explicit config-file values (they lose to the file, per precedence) [config.py].
