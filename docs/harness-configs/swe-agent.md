# SWE-agent (Princeton NLP) — Configurable Options

Research date: 2026-08-25. Primary sources: `SWE-agent/SWE-agent` @ `main` on GitHub (`raw.githubusercontent.com/SWE-agent/SWE-agent/main/docs/...`); the same pages are served at `swe-agent.com` (paths below map 1:1, e.g. `docs/config/models.md` → `swe-agent.com/config/models/`). Quotes/flags marked **[verified]** were pulled verbatim from those pages during this pass.

---

## 1) YAML config composition

A configuration is "one or more `.yaml` files", selected with the `--config` flag ([docs/config/config.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/config/config.md)):

```bash
sweagent run --config config/your_config.yaml
sweagent run-batch --config config/your_config.yaml
# Multiple files are merged *nested* (repeat the flag):
sweagent run --config config/default.yaml --config my_config.yaml   # [verified]
```

What a config lets you control **[verified]**: tools the agent may use, prompts/templates shown over a trajectory, demonstrations, model behavior, and the input/output interface between agent and environment. Relative paths inside configs resolve against `$SWE_AGENT_CONFIG_ROOT` (default: the package directory) **[verified]**.

### Top-level blocks

**`agent:` block**
- `agent.name` — display/run label for the agent (e.g. `name: claude-sonnet-4-20250514`) **[verified]**.
- `agent.model:` — model sub-block: `name`, `temperature`, `top_p`, `per_instance_cost_limit`, `completion_kwargs` (arbitrary kwargs passed through, e.g. `reasoning_effort: 'high'` for Claude extended thinking), `litellm_model_registry` (custom pricing JSON), `custom_tokenizer` **[verified]**; plus `api_key` / host URL fields for routing (§2). Examples from docs **[verified]**:
  ```yaml
  agent:
    model:
      temperature: 1.
      completion_kwargs:
        reasoning_effort: 'high'
  ```
  and for o1-series (only supported values) **[verified]**:
  ```yaml
  agent:
    model:
      top_p: null
      temperature: 1.
  ```
- `agent.templates:` — Jinja-style prompt templates (system/instance/step/format-error templates) and demonstration trajectories; see [docs/config/templates.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/config/templates.md) and [docs/config/demonstrations.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/config/demonstrations.md). Two template keys shown in docs **[verified]**: `disable_image_processing` and `max_observation_length`.
- `agent.history_processors:` — pipeline transforming message history before each call; `type: cache_control` with `last_n_messages: 2` sets Anthropic prompt-cache break points **[verified]**; `type: image_parsing` processes image-tool outputs **[verified]**.
- `agent.tools:` — see below (also nestable under agent in shipped configs).

**`tools:` block** ([docs/config/tools.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/config/tools.md))
- Tools ship as *bundles*: folders with `bin/` executables, `config.yaml`, `install.sh`, optional `pyproject.toml` **[verified]**. Wired via:
  ```yaml
  tools:
    bundles:
      - path: tools/windowed        # classic windowed-edit + linting bundle
      - path: tools/image_tools     # multimodal
      - path: tools/web_browser     # multimodal          [verified]
  ```
- Per-command spec in each bundle's `config.yaml` uses `signature` / `docstring` / `arguments[]` **[verified]**.
- `state_command:` — special command executed after every action whose JSON output feeds template variables (e.g. `state_command: "_state"` returning `{"open_file":..., "working_dir":...}`) **[verified]**.
- Action *parsers* extract the action from the model response — configured separately and documented at [docs/reference/parsers.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/reference/parsers.md); important when the backend can't do function calling **[verified]**.

**`environment:` block** (a.k.a. `env` on the CLI)
- Docker sandbox deployment: `env.deployment.type: docker` + `env.deployment.image: <docker image>`, e.g. `python:3.11` **[verified]** (shown in expert batch-instances YAML). Batch mode can populate it automatically from SWE-bench metadata; single runs take `--env`-family CLI flags.
- Repo to work on: `env.repo.type: github` with `github_url` (or local-path repos) **[verified]**.
- Python-interpreter/version selection for the container lives in this env/deployment config — exact field list: [docs/reference/env_config.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/reference/env_config.md) and [docs/reference/env.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/reference/env.md) (field naming has shifted slightly across releases; consult the reference for your pinned version).
- Background/architecture rationale: [docs/background/aci.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/background/aci.md) (agent–computer interface), [docs/background/architecture.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/background/architecture.md).

### `--config` flag & shipped defaults **[verified]**
- Defaults live in [`config/`](https://github.com/SWE-agent/SWE-agent/tree/main/config); with no `--config` flag, `config/default.yaml` loads automatically.
- Multimodal: `config/default_mm_with_images.yaml` (image processing, `max_observation_length: 10_000_000`, `image_tools` + `web_browser` bundles, `image_parsing` history processor).
- Heavier prompt-caching-aware setup: `config/sweagent_heavy.yaml` (referenced from models docs).
- If installed non-editable, point the runtime at your copies via `SWE_AGENT_CONFIG_DIR`, `SWE_AGENT_TOOLS_DIR`, `SWE_AGENT_TRAJECTORY_DIR`; logging via `SWE_AGENT_LOG_TIME`, `SWE_AGENT_LOG_STREAM_LEVEL`; most vars may live in `.env` ([docs/config/env.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/config/env.md), **[verified]**).

---

## 2) Model/provider routing

Model resolution goes through LiteLLM, so the model `name` follows LiteLLM provider-prefix conventions ([docs/config/models.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/config/models.md), [docs/installation/keys.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/installation/keys.md)):

- **Prefixes**: bare names like `gpt-4o` route to OpenAI by default; explicit provider prefixes select other backends — `anthropic/claude-…`, `openrouter/…`, `together_ai/…`, `deepseek/…`, `gemini/…`, `azure/<deployment>`, `ollama/…`, `bedrock/…`, etc. (full matrix table in `installation/keys.md`, the "required reading" for models.md).
- **API keys** are read from standard per-provider env vars (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, `GEMINI_API_KEY`, `DEEPSEEK_API_KEY`, …); "All API keys (for LMs and GitHub) can be set as an environment variable" **[verified]**, and can be persisted in `.env` **[verified]**. Standard OpenAI-compatible env pairs such as `OPENAI_API_KEY` + `OPENAI_BASE_URL` are the usual proxy route (keys.md table). You can also supply the key directly in config/CLI as `agent.model.api_key` / `--agent.model.api_key` **[verified flag]**.
- **Local vLLM / OpenAI-compatible servers**: give the served model an `openai/`-prefixed name (any `openai/…` name is sent to the OpenAI-compatible endpoint) and point SWE-agent at your server URL via the model config's host/base-URL field, i.e. `--agent.model.host_url http://localhost:8000/v1` (mechanism documented in keys.md "Local models"; the exact field name — `host_url` vs `api_base` — has shifted across releases, so confirm against [docs/reference/model_config.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/reference/model_config.md)). Servers that don't check auth accept a dummy `api_key`.
- **Custom host+key pairs for proxies**: combine the two knobs above — set the proxy endpoint as the host/base URL and the proxy token as `api_key` (flag or env pair), per run or per batch worker.
- **Multiple keys / rotation**: "We support rotating through multiple keys for run-batch… concatenate all keys with `:::` and set them via the `--agent.model.api_key` flag. Every thread… will stick to one key during the entire run," which preserves prompt caching **[verified]**.
- **Local-model extras**: unset spending limits (cost limits assume metered APIs) and pick an action parser if the server lacks function calling; provide `litellm_model_registry` JSON for pseudo-cost tracking **[verified]**. Test models need no credits: `HumanModel`, `HumanThoughtModel`, `ReplayModel`, `InstantEmptySubmitTestModel` **[verified]**.

---

## 3) Multi-instance wrappers

SWE-agent gives you three composable levers; a wrapper just combines them per run.

**Lever 1 — layered configs:** `--config` repeats and merges *nested*, so keep one immutable `base.yaml` and thin per-run overlays (model, cost limit, tools) **[verified]**. Relative paths in each layer resolve via `SWE_AGENT_CONFIG_ROOT` **[verified]**.

**Lever 2 — env switching:** every credential/routing knob is an env var (provider keys, `SWE_AGENT_CONFIG_ROOT/CONFIG_DIR/TOOLS_DIR/TRAJECTORY_DIR`, `.env` files) **[verified]**, so distinct instances can run with disjoint keys/dirs purely by exporting different environments.

**Lever 3 — batch sharding:** `sweagent run-batch` takes `--instances.{type,subset,split,slice,shuffle}` (slice = Python slicing, `shuffle` deterministic), `--num_workers N`, `--random_delay_multiplier` to stagger container startup, and per-thread sticky keys via the `:::` concatenation **[verified]**. Different settings per shard = one process per shard with its own overlay config/slice.

**Wrapper script example** (synthesis grounded in the flags above):

```bash
#!/usr/bin/env bash
# run_sweagent_shards.sh — N parallel batch instances with different settings
set -euo pipefail

MODELS=("openai/gpt-4o" "anthropic/claude-sonnet-4-20250514")
KEYS=("${OPENAI_API_KEY:?}" "${ANTHROPIC_API_KEY:?}")
SHARDS=("0:50" "50:100")

mkdir -p runs
i=0
for m in "${!MODELS[@]}"; do
  for s in "${SHARDS[@]}"; do
    out="runs/$(basename "${MODELS[$m]}")__shard_${s//:/-}"
    mkdir -p "$out"

    # Per-run overlay config on top of the shipped default (nested merge)
    cat > "$out/overlay.yaml" <<EOF
agent:
  name: shard-$i
  model:
    name: ${MODELS[$m]}
    per_instance_cost_limit: 3.00
  history_processors:
    - type: cache_control   # Anthropic prompt-cache break points
      last_n_messages: 2
EOF

    # Env switching: isolated trajectory dir (+ optional proxy host/key pair)
    (
      export SWE_AGENT_TRAJECTORY_DIR="$PWD/$out/trajectories"
      export OPENAI_API_KEY="${KEYS[$m]}"            # or OPENAI_BASE_URL+key for a proxy
      sweagent run-batch \
        --config config/default.yaml \
        --config "$out/overlay.yaml" \
        --agent.model.per_instance_cost_limit 3.00 \
        --num_workers 8 \
        --random_delay_multiplier 1 \
        --instances.type swe_bench \
        --instances.subset lite \
        --instances.split dev \
        --instances.slice "$s" \
        --instances.shuffle=True || true &
    )
    i=$((i+1))
  done
done
wait
# Reassemble interrupted/partial outputs:
#   sweagent merge-preds ...
```

Batch-instance sources beyond `swe_bench` **[verified]**: `file` (`.jsonl`/`.json`/`.yaml` with `instance_id` [formerly `id`], `problem_statement`, per-instance `image_name`), `huggingface` (`dataset_name`), and `expert_file` giving each instance its own full `env:` (deployment image) + `repo:` + problem statement — i.e. heterogeneous settings within one batch file.

---

## 4) Costs, caching, output

- **Cost limits**: `agent.model.per_instance_cost_limit` (CLI: `--agent.model.per_instance_cost_limit 2.00` **[verified]**), plus total-cost and call-limit variants on the model config; unset/disable for local models **[verified]**.
- **Custom pricing**: `litellm_model_registry` JSON overrides entries in LiteLLM's community cost file (new models, stale prices, local-model pseudo-costs); `custom_tokenizer` fixes the tokenizer used for cost math **[verified]**.
- **Prompt caching**: automatic for models like `gpt-4o`; Anthropic needs manual break points via the `cache_control` history processor **[verified]**. Claude allows ~4 cache break points per key and a run consumes two, so cap at **two parallel agents per key** — scale by rotating `:::`-joined keys **[verified]**. Verify hit rate with `grep -o "cached_tokens=[0-9]*" <id>.debug.log` in the trajectory directory **[verified]**. Claude 3.7/4 max output tokens can be raised via extra headers or overridden post-#1036 **[verified]**.
- **Output artifacts**: trajectories land under `SWE_AGENT_TRAJECTORY_DIR` (default `<package>/trajectories`), containing per-run trajectory records and `.debug.log` files **[verified]**; browse them with the Inspector ([docs/usage/inspector.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/usage/inspector.md)).
- **Predictions**: batch runs emit `preds.json`; interrupted runs are repaired with `sweagent merge-preds`; convert `preds.json` ↔ SWE-bench `.jsonl` with the documented snippet **[verified]**.

---

## 5) Relationship to SWE-bench

- Same lineage: SWE-agent is Princeton NLP's agent framework built around the SWE-bench benchmark family (swe-bench.github.io / swebench.com); its canonical workflow is resolving SWE-bench issues in Docker sandboxes derived from the benchmark's environment specs.
- First-class dataset loading: `--instances.type swe_bench` auto-downloads tasks, with `subset` (`lite`, `verified`, `multimodal`, …), `split` (`dev`/`test`), `slice`, `shuffle` **[verified]**.
- Evaluation hook: `--evaluate=True` submits predictions to `sb-cli` mid-run for official SWE-bench scoring within ~a minute of finishing **[verified]**; the `preds.json`→`.jsonl` converter matches SWE-bench's prediction format **[verified]**.
- Multimodal: dedicated support for SWE-bench Multimodal (issues with screenshots/diagrams) via `config/default_mm_with_images.yaml` + `--instances.subset multimodal` **[verified]**.
- Tutorials aimed at benchmark/competition use: [docs/usage/competitive_runs.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/usage/competitive_runs.md), [docs/usage/hello_world.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/usage/hello_world.md), [docs/usage/cl_tutorial.md](https://github.com/SWE-agent/SWE-agent/blob/main/docs/usage/cl_tutorial.md).

---

### Source index
- Config composition/defaults: `docs/config/config.md`, `templates.md`, `demonstrations.md`, `tools.md`, `env.md` (all @ `main`)
- Models/routing/caching: `docs/config/models.md`, `docs/installation/keys.md`, `docs/reference/model_config.md`, `docs/reference/history_processor_config.md`
- Batch/wrappers/output: `docs/usage/batch_mode.md`, `docs/reference/run_batch_config.md`, `docs/reference/batch_instances.md`, `docs/usage/inspector.md`
- Env/schema references: `docs/reference/env_config.md`, `docs/reference/env.md`, `docs/reference/parsers.md`, `docs/reference/bundle_config.md`
- SWE-bench: `docs/usage/competitive_runs.md`, swebench.com, `sb-cli` docs
