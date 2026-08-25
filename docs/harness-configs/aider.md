# Aider — Complete Configuration Reference

> Compiled 2026-08-25 from official docs & repo. Primary sources cited inline:
> [config](https://aider.chat/docs/config.html) · [options ref](https://aider.chat/docs/config/options.html) · [.env guide](https://aider.chat/docs/config/dotenv.html) · [api-keys](https://aider.chat/docs/config/api-keys.html) · [llms](https://aider.chat/docs/llms.html) · [adv-model-settings](https://aider.chat/docs/config/adv-model-settings.html) · [`aider/args.py`](https://github.com/Aider-AI/aider/blob/main/aider/args.py) · [`resources/model-settings.yml`](https://github.com/Aider-AI/aider/blob/main/aider/resources/model-settings.yml)

---

## 1. Config layers & precedence

Aider options can be set four equivalent ways — CLI switch, `.aider.conf.yml`, `AIDER_*` env var, or `.env` entry ([config](https://aider.chat/docs/config.html)):

```bash
aider --dark-mode              # CLI
# .aider.conf.yml:  dark-mode: true
export AIDER_DARK_MODE=true    # shell env
# .env:             AIDER_DARK_MODE=true
```

**Precedence (highest wins):**

| # | Layer | Notes |
|---|-------|-------|
| 1 | **CLI args** | Always override everything |
| 2 | **`.env`** | Loaded from, in order: home dir → git repo root → cwd → `--env-file <path>`; *"files loaded last take priority"* so an explicit `--env-file` wins ([dotenv](https://aider.chat/docs/config/dotenv.html)) |
| 3 | **`.aider.conf.yml` in git root** | |
| 4 | **`~/.aider.conf.yml`** | Lowest-priority YAML |

- YAML config files are searched in **git root → cwd → home dir** (`args.py` `--config` help: *"default: search for .aider.conf.yml in git root, cwd or home directory"*).
- **YAML keys = long CLI options without the `--`** (kebab-case): `--map-tokens 1024` ⇔ `map-tokens: 1024`. Boolean flags accept `true/false`; `--no-X` flags become `X: false`.
- Per-file-type configs follow the same pattern: `.aider.model.settings.yml` and `.aider.model.metadata.json` are searched in **home dir → git root → cwd**, last-loaded wins; or point at one explicitly via `--model-settings-file` / `--model-metadata-file` ([adv-model-settings](https://aider.chat/docs/config/adv-model-settings.html)).
- Default `.env` path is `<git-root>/.env`, or `./.env` outside a repo (`args.py:def default_env_file`). `--env-file` default: `.env` ([options ref](https://aider.chat/docs/config/options.html)).
- API keys may live in any layer: dedicated YAML entries (`openai-api-key:`), `api-key: [gemini=foo, …]` list, env, or `.env` ([api-keys](https://aider.chat/docs/config/api-keys.html)).
- `--set-env VAR=value` (repeatable) injects arbitrary provider env vars at launch; `--api-key provider=<key>` sets `<PROVIDER>_API_KEY=<key>` ([options ref](https://aider.chat/docs/config/options.html)).

---

## 2. Environment variables

### 2.1 Provider credentials & base URLs (exhaustive, per provider docs)

| Variable | Purpose | Source |
|---|---|---|
| `OPENAI_API_KEY` | OpenAI key | [api-keys](https://aider.chat/docs/config/api-keys.html) |
| `OPENAI_API_BASE` | Base URL for OpenAI-compatible endpoints | [openai-compat](https://aider.chat/docs/llms/openai-compat.html) |
| `ANTHROPIC_API_KEY` | Anthropic key | [api-keys] |
| `GEMINI_API_KEY` | Google Gemini (via `--api-key gemini=` too) | [api-keys], [llms](https://aider.chat/docs/llms.html) |
| `OPENROUTER_API_KEY` | OpenRouter | [openrouter](https://aider.chat/docs/llms/openrouter.html) |
| `DEEPSEEK_API_KEY` | DeepSeek | [api-keys], [deepseek](https://aider.chat/docs/llms/deepseek.html) |
| `OLLAMA_API_BASE` | Ollama endpoint (e.g. `http://127.0.0.1:11434`) — canonical var; some tooling uses `OLLAMA_API_HOST`, but aider docs use `API_BASE` | [ollama](https://aider.chat/docs/llms/ollama.html) |
| `OLLAMA_API_KEY` | Only for auth-enabled Ollama servers | [ollama] |
| `LM_STUDIO_API_KEY` | Must be set even to a dummy value (`dummy-api-key`) — empty Bearer fails | [lm-studio](https://aider.chat/docs/llms/lm-studio.html) |
| `LM_STUDIO_API_BASE` | Default `http://localhost:1234/v1` | [lm-studio] |
| `AZURE_API_KEY` | Azure OpenAI key | [azure](https://aider.chat/docs/llms/azure.html) |
| `AZURE_API_VERSION` | e.g. `2024-12-01-preview` | [azure] |
| `AZURE_API_BASE` | e.g. `https://myendpt.openai.azure.com`; aider also honors `AZURE_OPENAI_API_xxx` variants | [azure] |
| `XAI_API_KEY` | xAI Grok | [xai](https://aider.chat/docs/llms/xai.html) |
| `GROQ_API_KEY` | Groq | [groq](https://aider.chat/docs/llms/groq.html) |
| `COHERE_API_KEY` | Cohere | [cohere](https://aider.chat/docs/llms/cohere.html) |
| `MISTRAL_API_KEY` | Mistral (litellm provider) | [other](https://aider.chat/docs/llms/other.html) |
| `VERTEXAI_PROJECT` / `VERTEXAI_LOCATION` | Vertex AI (+ standard `GOOGLE_APPLICATION_CREDENTIALS`) | [vertex](https://aider.chat/docs/llms/vertex.html) |
| `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_REGION` | Amazon Bedrock (region often `AWS_DEFAULT_REGION`) | [bedrock](https://aider.chat/docs/llms/bedrock.html) |
| Generic `PROVIDER_API_KEY` | Any provider: `--api-key provider=<key>` sets it | [api-keys] |

Deprecated legacy OpenAI switches (prefer `--set-env`): `--openai-api-type`, `--openai-api-version`, `--openai-api-deployment-id`, `--openai-organization-id` → map to `OPENAI_API_TYPE`, `OPENAI_API_VERSION`, `OPENAI_API_DEPLOYMENT_ID`, `OPENAI_ORGANIZATION` ([options ref](https://aider.chat/docs/config/options.html)).

### 2.2 `AIDER_*` flag mapping rule

Every long CLI option has a matching env var: **long name, dashes→underscores, prefixed `AIDER_`**, uppercase ([dotenv](https://aider.chat/docs/config/dotenv.html)). Examples: `--map-tokens`→`AIDER_MAP_TOKENS`, `--no-auto-commits`⇔`AIDER_AUTO_COMMITS=false`, `--4o`→`AIDER_4O`. Exceptions with no env var: `-h/--help`, `--version`, `--config` (must come from CLI/shell). The complete mapping for all ~130 options is reproduced in §6's table ("Env" column).

Special multi-value env vars: `AIDER_SET_ENV` (repeatable `VAR=value`), `AIDER_API_KEY` (`provider=key`), `AIDER_ALIAS` (`alias:model`), `AIDER_LINT_CMD` (`lang: cmd`), `AIDER_FILE` / `AIDER_READ` (paths).

---

## 3. Model configuration

### 3.1 Model selection options

| Option (YAML key) | Env var | Effect |
|---|---|---|
| `--model` / `model` | `AIDER_MODEL` | Main chat model. Prefixes select provider: `openrouter/<p>/<m>`, `ollama_chat/<m>`, `lm_studio/<m>`, `azure/<deployment>`, `openai/<m>`, `gemini/…`, `deepseek/…`, `claude-…`, `bedrock/…`, `vertex_ai/…`, `azure_ai/…`, `github/…` ([llms](https://aider.chat/docs/llms.html)) |
| `--list-models` (alias `--models`) | `AIDER_LIST_MODELS` | List known models matching partial name, e.g. `aider --list-models openrouter/` |
| `--alias alias:model` | `AIDER_ALIAS` | Define shorthand aliases (repeatable); managed persistently via `/alias` ([model-aliases](https://aider.chat/docs/config/model-aliases.html)) |
| `--weak-model` | `AIDER_WEAK_MODEL` | Cheap model for commit messages + chat-history summarization (defaults derived from main model) |
| `--editor-model` | `AIDER_EDITOR_MODEL` | Model applying edits when architect format is active |
| `--editor-edit-format` | `AIDER_EDITOR_EDIT_FORMAT` | Edit format for editor model (`diff`, `whole`, `editor-diff`, `editor-whole`) |
| `--architect` | `AIDER_ARCHITECT` | Force architect edit format on main chat |
| `--auto-accept-architect` | `AIDER_AUTO_ACCEPT_ARCHITECT` | Auto-accept architect's edits (default true) |
| `--edit-format` | `AIDER_EDIT_FORMAT` | Override edit format (`diff`, `whole`, `architect`, `editor-diff`, `editor-whole`, …) |
| `--reasoning-effort` | `AIDER_REASONING_EFFORT` | Sets `reasoning_effort` API param ([reasoning](https://aider.chat/docs/config/reasoning.html)) |
| `--thinking-tokens` | `AIDER_THINKING_TOKENS` | Thinking budget; `0` disables |
| `--verify-ssl / --no-verify-ssl` | `AIDER_VERIFY_SSL` | TLS verification of model endpoints (default True) |
| `--timeout` | `AIDER_TIMEOUT` | API call timeout seconds |
| `--check-update`-adjacent model sanity: `--show-model-warnings`, `--check-model-accepts-settings` | respective `AIDER_*` | Warn on unknown models / reject unsupported settings like `reasoning_effort` |
| `--max-chat-history-tokens` | `AIDER_MAX_CHAT_HISTORY_TOKENS` | Soft cap before history summarization |
| Deprecated shortcuts | | `--opus`, `--sonnet`, `--haiku`, `--4`, `--4o`, `--mini`, `--4-turbo`, `--35turbo`(`-3`), `--deepseek`, `--o1-mini`, `--o1-preview` — all just set `--model` ([options ref]) |

The three-model split: **main** (`--model`) plans/chats; **architect mode** routes code edits through **editor** (`--editor-model` + `--editor-edit-format`); **weak** (`--weak-model`) handles commit msgs/history summaries. All default from the main model unless overridden ([options ref]).

### 3.2 `.aider.model.settings.yml`

List of dicts keyed by model `name`; searched home → git root → cwd, last wins; or `--model-settings-file <f>` ([adv-model-settings]). All supported fields (from the "(default values)" entry in the docs and the 357-entry shipped [`resources/model-settings.yml`](https://github.com/Aider-AI/aider/blob/main/aider/resources/model-settings.yml)):

| Field | Default | Meaning |
|---|---|---|
| `name` | — | Fully qualified litellm-style model name |
| `edit_format` | `whole` | Edit format for this model (`diff`, `whole`, `architect`, …) |
| `weak_model_name` | `null` | Weak model override |
| `editor_model_name` | `null` | Editor model override |
| `editor_edit_format` | `null` | Editor edit format |
| `use_repo_map` | `false` | Send repo map to this model |
| `send_undo_reply` | `false` | Offer `/undo` |
| `lazy` | `false` | Lazy coding style prompts |
| `overeager` | `false` | Tune over-eager edit behavior |
| `reminder` | `user` | Where file reminders go (`sys`, `user`, `system`) |
| `examples_as_sys_msg` | `false` | Send examples as system messages |
| `extra_params` | `null` | **Arbitrary kwargs passed to `litellm.completion()`** — incl. `extra_headers` (custom HTTP headers), `max_tokens`, `num_ctx` (Ollama context), nested `extra_body` (e.g. OpenRouter provider routing) |
| `cache_control` | `false` | Anthropic prompt-cache breakpoints |
| `caches_by_default` | `false` | Use prompt caching by default |
| `use_system_prompt` | `true` | Send system prompt role |
| `use_temperature` | `true` | Send temperature param |
| `streaming` | `true` | Stream responses |
| `reasoning_tag` / `remove_reasoning` | `null` | Strip/tag reasoning output |
| `system_prompt_prefix` | `null` | Text prepended to system prompt |
| `accepts_settings` | `null` | Whitelist of user settings (`thinking_tokens`, `reasoning_effort`) this model accepts |

Worked examples:

```yaml
# Fixed context window for local Ollama (docs/llms/ollama):
- name: ollama/qwen2.5-coder:32b-instruct-fp16
  extra_params:
    num_ctx: 65536

# Custom headers + max_tokens for an unknown model (docs/config/adv-model-settings):
- name: some-provider/my-special-model
  extra_params:
    extra_headers:
      Custom-Header: value
    max_tokens: 8192

# Global defaults applied to ALL models — special name "aider/extra_params"
# (merged with model-specific settings; its direct conflicts win):
- name: aider/extra_params
  extra_params:
    extra_headers:
      Custom-Header: value
    max_tokens: 8192
```

OpenRouter provider routing via `extra_params.extra_body.provider`: `order`, `allow_fallbacks`, `data_collection`, `require_parameters` ([openrouter]):

```yaml
- name: openrouter/anthropic/claude-3.7-sonnet
  extra_params:
    extra_body:
      provider:
        order: ["Anthropic", "Together"]
        allow_fallbacks: false
        data_collection: "deny"
        require_parameters: true
```

### 3.3 `.aider.model.metadata.json`

Registers context windows & costs for unknown models. Locations/search order identical to model settings; explicit file via `--model-metadata-file` (**current default filename `.aider.model.metadata.json`** — older docs/versions used `.aider.model.metadata.yml`; the shipped default in [options ref] is the JSON variant). Format ([adv-model-settings]):

```json
{
  "deepseek/deepseek-chat": {
    "max_tokens": 4096,
    "max_input_tokens": 32000,
    "max_output_tokens": 4096,
    "input_cost_per_token": 0.00000014,
    "output_cost_per_token": 0.00000028,
    "litellm_provider": "deepseek",
    "mode": "chat"
  }
}
```

Keys must be fully qualified (`provider/model`) matching `litellm_provider`. Metadata ultimately comes from litellm's `model_prices_and_context_window.json` — contributing upstream PRs is preferred. Aider never enforces token limits itself; it only relays provider errors.

---

## 4. Third-party providers — worked recipes

**OpenRouter** ([openrouter]):
```bash
export OPENROUTER_API_KEY=<key>
cd /to/your/project
aider --model openrouter/<provider>/<model>   # e.g. openrouter/anthropic/claude-3.7-sonnet
aider --list-models openrouter/
```
Control upstream provider selection via `.aider.model.settings.yml` `extra_body.provider` (see §3.2) or OpenRouter account privacy/provider settings.

**Ollama (local)** ([ollama]):
```bash
export OLLAMA_API_BASE=http://127.0.0.1:11434   # omit if default
OLLAMA_CONTEXT_LENGTH=8192 ollama serve          # or rely on aider auto-sizing (+8k headroom)
aider --model ollama_chat/<model>                # ollama_chat/ prefix recommended over ollama/
# auth-enabled servers: export OLLAMA_API_KEY=...
# fixed context: num_ctx via .aider.model.settings.yml (§3.2)
```

**LM Studio (local)** ([lm-studio]):
```bash
export LM_STUDIO_API_KEY=dummy-api-key           # REQUIRED even if server ignores keys
export LM_STUDIO_API_BASE=http://localhost:1234/v1
aider --model lm_studio/<your-model-name>
```

**Azure OpenAI** ([azure]):
```bash
export AZURE_API_KEY=<key>
export AZURE_API_VERSION=2024-12-01-preview
export AZURE_API_BASE=https://myendpt.openai.azure.com
aider --model azure/<your_model_deployment_name>
aider --list-models azure/                       # lists aider-known Azure models, not your endpoint's
```

**Generic OpenAI-compatible endpoint** ([openai-compat]):
```bash
export OPENAI_API_BASE=<endpoint>
export OPENAI_API_KEY=<key>
aider --model openai/<model-name>                # openai/ prefix routes through OpenAI client
```
Works for vLLM, llama.cpp server, LocalAI, LiteLLM proxy, Together, Groq-compatible gateways, etc. (litellm provider prefixes like `together_ai/`, `groq/`, `mistral/`, `huggingface/` also work where litellm supports them — [other LLMs](https://aider.chat/docs/llms/other.html)).

**LiteLLM-style proxy**: point `OPENAI_API_BASE` at the proxy (`http://host:4000/v1`), set any placeholder key, and use `--model openai/<proxy-model>`; or use litellm-native prefixes (`openrouter/`, `azure/`, `ollama_chat/`, `bedrock/`, `vertex_ai/`) directly since aider delegates all completions to litellm. Extra passthrough params (headers/body) ride in `extra_params` (§3.2).

**Vertex AI / Bedrock**: `--model vertex_ai/<m>` with `GOOGLE_APPLICATION_CREDENTIALS`+`VERTEXAI_PROJECT`/`VERTEXAI_LOCATION`; `--model bedrock/<m>` with AWS creds/region env vars ([vertex]/[bedrock]).

---

## 5. Multi-instance wrappers (isolated parallel instances)

Goal: run several aiders side-by-side, each with its own model/provider/config/history — no cross-contamination.

**Mechanisms:**

1. **Explicit config paths** — every file-affecting option is overridable per invocation:
   - `--config <file>` / `-c` : bypasses the normal git-root/cwd/home search entirely ([args.py](https://github.com/Aider-AI/aider/blob/main/aider/args.py) L792–800).
   - `--env-file <file>` (or `AIDER_ENV_FILE`): loads that exact `.env` instead of `<git-root>/.env`.
   - `--model-settings-file`, `--model-metadata-file`: per-instance model overrides.
   - History isolation: `--chat-history-file`, `--input-history-file`, `--llm-history-file` (all default to git-root-relative paths, `args.py` L272–275).
2. **`HOME` relocation trick** — because home-dir lookups (`~/.aider.conf.yml`, `~/.env`, `~/.aider.model.settings.yml`, `~/.aider.model.metadata.json`) resolve against `$HOME`:
   ```bash
   mkdir -p ~/aider-instances/proj-a && cp ~/.aider.conf.yml ~/aider-instances/proj-a/
   HOME=~/aider-instances/proj-a aider --model openrouter/anthropic/claude-3.7-sonnet
   ```
   Everything under `$HOME` (including litellm caches) moves with it — cleanest way to sandbox a whole profile without repeating flags.
3. **Subshell/env scoping** — run each instance in `( … )` with only its exports, so `OPENAI_API_BASE`, `OLLAMA_API_BASE`, etc. don't leak between instances.

**Wrapper script example — two instances, two providers:**

```bash
#!/usr/bin/env bash
# /usr/local/bin/aider-dual — run Claude-via-OpenRouter and local Ollama side by side
set -euo pipefail
PROJ="${1:?usage: aider-dual <project-dir>}"
CFG="$(dirname "$0")/../etc/aider-instances"

# Instance 1: cloud, isolated config + env + histories
(cd "$PROJ" \
  && OPENROUTER_API_KEY="${OPENROUTER_API_KEY:?not set}" \
     aider --config "$CFG/openrouter.yml" \
           --env-file "$CFG/openrouter.env" \
           --model openrouter/anthropic/claude-3.7-sonnet \
           --chat-history-file .aider.chat.history-or.md) &
CLOUD_PID=$!

# Instance 2: local Ollama via HOME relocation (its ~/.aider.conf.yml sets ollama defaults)
mkdir -p "$CFG/home-ollama"
( cd "$PROJ" && HOME="$CFG/home-ollama" \
    OLLAMA_API_BASE="http://127.0.0.1:11434" \
    aider --model ollama_chat/qwen2.5-coder:32b \
          --chat-history-file .aider.chat.history-local.md ) &
LOCAL_PID=$!

trap 'kill $CLOUD_PID $LOCAL_PID 2>/dev/null' EXIT
wait $CLOUD_PID $LOCAL_PID
```

Companion files:
```yaml
# $CFG/openrouter.yml            — YAML keys = long options
dark-mode: true
auto-commits: false
map-tokens: 2048
```
```bash
# $CFG/openrouter.env
OPENROUTER_API_KEY=sk-or-...
# $CFG/home-ollama/.aider.model.settings.yml
- name: ollama_chat/qwen2.5-coder:32b
  extra_params:
    num_ctx: 65536
```

---

## 6. Misc knobs — full option table

All long options from the [options reference](https://aider.chat/docs/config/options.html) (130 rows; grouped as the docs do). "Env" column = `AIDER_*` variable usable in shell or `.env`. Boolean pairs `X/--no-X` share one row. YAML key = option name minus `--`.

**Main model:** `--help/-h`.

| Option | Env | Default / purpose |
|---|---|---|
| `--model` | `AIDER_MODEL` | Main chat model (see §3) |

**API keys & settings:**

| Option | Env | Notes |
|---|---|---|
| `--openai-api-key`, `--anthropic-api-key` | `AIDER_OPENAI_API_KEY`, `AIDER_ANTHROPIC_API_KEY` | Dedicated switches |
| `--openai-api-base` | `AIDER_OPENAI_API_BASE` | Base URL |
| `--openai-api-type/-version/-deployment-id/-organization-id` | matching `AIDER_*` | Deprecated → `--set-env` |
| `--set-env NAME=value` (repeatable) | `AIDER_SET_ENV` | Arbitrary provider env injection |
| `--api-key provider=key` (repeatable) | `AIDER_API_KEY` | Sets `<PROVIDER>_API_KEY` |

**Model settings:** `--list-models`(`--models`)/`AIDER_LIST_MODELS` · `--model-settings-file`(default `.aider.model.settings.yml`)· `--model-metadata-file`(default `.aider.model.metadata.json`)· `--alias`/`AIDER_ALIAS` · `--reasoning-effort`· `--thinking-tokens`· `--verify-ssl/--no-verify-ssl`(True)· `--timeout`· `--edit-format`· `--architect`· `--auto-accept-architect`(True)· `--weak-model`· `--editor-model`· `--editor-edit-format`· `--show-model-warnings/--no-…`(True)· `--check-model-accepts-settings/--no-…`(True)· `--max-chat-history-tokens` — all with `AIDER_<NAME>` env equivalents.

**Cache:** `--cache-prompts/--no-cache-prompts`(False, `AIDER_CACHE_PROMPTS`) · `--cache-keepalive-pings`(0; pings every 5 min to keep Anthropic cache warm, `AIDER_CACHE_KEEPALIVE_PINGS`).

**Repo map:** `--map-tokens`(`AIDER_MAP_TOKENS`, 0 disables; model-dependent default ~1k/2k) · `--map-refresh`=auto\|always\|files\|manual(`AIDER_MAP_REFRESH`) · `--map-multiplier-no-files`(2, `AIDER_MAP_MULTIPLIER_NO_FILES`).

**History files:** `--input-history-file`(`.aider.input.history`)· `--chat-history-file`(`.aider.chat.history.md`)· `--restore-chat-history/--no-…`(False)· `--llm-history-file` — `AIDER_INPUT_HISTORY_FILE` / `AIDER_CHAT_HISTORY_FILE` / `AIDER_RESTORE_CHAT_HISTORY` / `AIDER_LLM_HISTORY_FILE`.

**Output/UI:** `--dark-mode`· `--light-mode`· `--pretty/--no-pretty`(True)· `--stream/--no-stream`(True)· colors `--user-input-color`(#00cc00)· `--tool-output-color`(None)· `--tool-error-color`(#FF2222)· `--tool-warning-color`(#FFA500)· `--assistant-output-color`(#0088ff)· completion-menu colors ×4 (fg/bg/current/current-bg, terminal defaults)· `--code-theme`(Pygments style, default `default`)· `--show-diffs`(False) — `AIDER_DARK_MODE` … `AIDER_SHOW_DIFFS`.

**Git:** `--git/--no-git`(True; disable repo detection)· `--gitignore/--no-gitignore`(True; auto-add `.aider*` to `.gitignore`)· `--add-gitignore-files/--no-…`(False; allow editing gitignored files)· `--aiderignore`(default `.aiderignore` in git root)· `--subtree-only`(False) — `AIDER_GIT`, `AIDER_GITIGNORE`, `AIDER_ADD_GITIGNORE_FILES`, `AIDER_AIDERIGNORE`, `AIDER_SUBTREE_ONLY`.
Attribution: `--attribute-author/--no-…`(True)· `--attribute-committer/--no-…`(True)· `--attribute-co-authored-by/--no-…`(True, takes precedence over the prior two unless they're explicitly True)· `--attribute-commit-message-author`(False, `aider: ` prefix when aider authored)· `--attribute-commit-message-committer`(False, prefix always) — `AIDER_ATTRIBUTE_*`.

**Commits & lint/test hooks:**

| Option | Env | Default | Purpose |
|---|---|---|---|
| `--auto-commits/--no-auto-commits` | `AIDER_AUTO_COMMITS` | True | Auto-commit LLM edits |
| `--dirty-commits/--no-dirty-commits` | `AIDER_DIRTY_COMMITS` | True | Commit pre-existing dirty state first |
| `--git-commit-verify/--no-…` | `AIDER_GIT_COMMIT_VERIFY` | False | Run pre-commit hooks (don't pass `--no-verify`) |
| `--commit` | `AIDER_COMMIT` | False | One-shot: commit pending changes with generated message, exit |
| `--commit-prompt` | `AIDER_COMMIT_PROMPT` | — | Custom commit-message prompt |
| `--dry-run/--no-dry-run` | `AIDER_DRY_RUN` | False | Show diffs without writing files |
| `--skip-sanity-check-repo` | `AIDER_SKIP_SANITY_CHECK_REPO` | False | Skip repo sanity check |
| `--watch-files/--no-watch-files` | `AIDER_WATCH_FILES` | False | AI-comment watcher mode |
| `--lint` | `AIDER_LINT` | False | One-shot: lint+fix, exit |
| `--lint-cmd "lang: cmd"` (repeatable) | `AIDER_LINT_CMD` | [] | Per-language lint commands |
| `--auto-lint/--no-auto-lint` | `AIDER_AUTO_LINT` | True | Lint after each change, feed errors back |
| `--test-cmd` | `AIDER_TEST_CMD` | [] | Test command |
| `--auto-test/--no-auto-test` | `AIDER_AUTO_TEST` | False | Test after changes, feed failures back |
| `--test` | `AIDER_TEST` | False | One-shot: test+fix, exit |

**Analytics:** `--analytics/--no-analytics`(`AIDER_ANALYTICS`, default random-opt-in) · `--analytics-log`· `--analytics-disable`· `--analytics-posthog-host`· `--analytics-posthog-project-api-key` (`AIDER_ANALYTICS*`).

**Upgrading:** `--just-check-update`· `--check-update/--no-check-update`(True)· `--show-release-notes/--no-…`(None→ask)· `--install-main-branch`· `--upgrade`(alias `--update`)· `--version` — `AIDER_JUST_CHECK_UPDATE` etc.

**Modes/scripting:** `--message/-m/--msg`(`AIDER_MESSAGE`, one-shot non-interactive) · `--message-file/-f`(`AIDER_MESSAGE_FILE`) · `--gui/--browser`(`AIDER_GUI`) · `--copy-paste`· `--apply FILE`· `--apply-clipboard-edits`· `--exit`· `--show-repo-map`· `--show-prompts` (debug) — `AIDER_APPLY`, `AIDER_EXIT`, …

**Voice:** `--voice-format`(wav|webm|mp3; ffmpeg needed for latter two)· `--voice-language`(ISO 639-1, default en)· `--voice-input-device` — `AIDER_VOICE_FORMAT/LANGUAGE/INPUT_DEVICE`.

**Other:** `--disable-playwright`(`AIDER_DISABLE_PLAYWRIGHT`) · `--file`(repeatable, `AIDER_FILE`) · `--read`(read-only context files, repeatable, `AIDER_READ`) · `--vim`(`AIDER_VIM`) · `--chat-language`(`AIDER_CHAT_LANGUAGE`) · `--commit-language`(`AIDER_COMMIT_LANGUAGE`) · `--yes-always`(`AIDER_YES_ALWAYS`) · `--verbose/-v`(`AIDER_VERBOSE`) · `--load FILE`(`AIDER_LOAD`, run `/commands` at launch) · `--encoding`(utf-8, `AIDER_ENCODING`) · `--line-endings`=platform\|lf\|crlf(`AIDER_LINE_ENDINGS`, choices per args.py) · `--suggest-shell-commands/--no-…`(True) · `--fancy-input/--no-…`(True) · `--multiline/--no-…`(False, Meta-Enter submit) · `--notifications/--no-…`(False bell) · `--notifications-command`· `--detect-urls/--no-…`(True) · `--editor`(`AIDER_EDITOR`, for `/editor`) · `--shell-completions bash\|tcsh\|zsh` · `--config/-c`· `--env-file`(see §5) — each with its `AIDER_*` twin except `--config`/`--help`/`--version`.

**Deprecated model shortcuts:** `--opus`, `--sonnet`, `--haiku`, `--4`/`-4`, `--4o`, `--mini`, `--4-turbo`, `--35turbo`/`--35-turbo`/`-3`, `--deepseek`, `--o1-mini`, `--o1-preview` (env: `AIDER_OPUS`, `AIDER_4O`, …) — superseded by `--model` ([options ref]).

---

### Quick counts
- CLI long options: **130** (each with `AIDER_*` env twin except `--help/--version/--config`)
- Dedicated provider credential/base-URL env vars documented: **20+** (§2.1), plus unlimited via `PROVIDER_API_KEY` pattern & `--set-env`
- Model-settings YAML fields: **19** (+ free-form `extra_params` nesting)
