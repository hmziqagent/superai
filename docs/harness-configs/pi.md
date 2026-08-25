# Pi Coding Agent — Configurable Options

Pi (`@earendil-works/pi-coding-agent`) is a minimal terminal coding harness. Repo lives at [github.com/earendil-works/pi-mono](https://github.com/earendil-works/pi-mono) (formerly `badlogic/pi-mono`; redirects work). Docs: [pi.dev/docs/latest](https://pi.dev/docs/latest), source-of-truth markdown in [`packages/coding-agent/docs/`](https://github.com/earendil-works/pi-mono/tree/main/packages/coding-agent/docs). Researched 2026-08-25.

---

## 1. Monorepo layers

Per the [root README](https://raw.githubusercontent.com/earendil-works/pi-mono/main/README.md):

| Package | Path | Role |
|---|---|---|
| **pi-ai** | `packages/ai` | Unified multi-provider LLM API (OpenAI, Anthropic, Google, …); owns the env-var→provider key map (`env-api-keys.ts`) |
| **pi-agent-core** | `packages/agent` | Agent runtime: tool calling loop + state management |
| **pi-tui** | `packages/tui` | Terminal UI library with differential rendering (also usable by extensions) |
| **pi-coding-agent** (`pi` CLI) | `packages/coding-agent` | The interactive coding agent CLI — where all user-facing config below lives |
| pi-telemetry | `packages/telemetry` | Vendor-neutral telemetry contracts (supporting cast) |

Chat/Slack automation is split out into a separate repo ([earendil-works/pi-chat](https://github.com/earendil-works/pi-chat)).

## 2. Config & auth

### Where things live (default config dir: `~/.pi/agent`)
All under `~/.pi/agent/` unless overridden by `PI_CODING_AGENT_DIR` ([environment-variables.md](https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/environment-variables.md), [usage.md](https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/usage.md)):

| Path | Purpose |
|---|---|
| `auth.json` | API keys + OAuth tokens (chmod 0600). **Auth file beats env vars** |
| `models.json` | Custom providers/models (see §2.3) |
| `models-store.json` | Cached refreshed provider model catalogs for offline use |
| `settings.json` | Global settings (thinking level, theme, TUI mode, `defaultProjectTrust`, `skills` array, …); project equivalent `.pi/settings.json` |
| `sessions/` | JSONL session files, organized by working dir (`--session-dir` overrides) |
| `trust.json` | Saved project-trust decisions (written by `/trust`) |
| `AGENTS.md` / `SYSTEM.md` / `APPEND_SYSTEM.md` | Global context file / full system-prompt replacement / system-prompt append (project versions: `.pi/SYSTEM.md`) |

Note: there is no `~/.agents` *config* dir, but Pi also reads skills from `~/.agents/skills/` (§4).

### Auth flows
- **Subscriptions**: `/login` → ChatGPT Plus/Pro (Codex), Claude Pro/Max, GitHub Copilot, xAI, OpenRouter (PKCE; on headless boxes paste the redirect URL), Radius. Tokens stored in `auth.json`, auto-refreshed; `/logout` clears ([providers.md](https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/providers.md)).
- **API keys**: set env vars or `/login <provider>` writes them into `auth.json`. ~35 built-ins incl. Anthropic, OpenAI, Google, xAI, DeepSeek, Groq, Mistral, OpenRouter, Bedrock, MiniMax, Qwen, Kimi, Hugging Face (`HF_TOKEN`)…
- **Credential value resolution** (both `auth.json` and `models.json`): `"!command"` runs a shell command and uses stdout (e.g. `!security find-generic-password -ws 'anthropic'`, `!op read ...`); `$VAR`/`${VAR}` interpolation; `$$`/`$!` escapes; plain literals. `auth.json` entries may also carry an `"env": {...}` object whose values take priority over the process environment (useful for per-instance Cloudflare/Azure/Bedrock settings).
- **Resolution order**: CLI `--api-key` → `auth.json` → environment variable → `models.json` custom-provider keys.

### Model selection
- Interactive: `/model` picker (availability = configured auth presence), `/scoped-models` for Ctrl+P cycling list.
- CLI: `--provider <name>`, `--model <pattern>` (accepts `provider/id` and thinking shorthand like `sonnet:high`), `--thinking off|minimal|low|medium|high|xhigh|max`, `--models "claude-*,gpt-4o"` cycling patterns, `--list-models [search]`.
- `models.json` hot-reloads every time `/model` opens — edit mid-session, no restart.

### Custom providers config format (`~/.pi/agent/models.json`)
From [models.md](https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/models.md):

```json
{
  "providers": {
    "my-google": {
      "baseUrl": "https://generativelanguage.googleapis.com/v1beta",
      "api": "google-generative-ai",
      "apiKey": "$GEMINI_API_KEY",
      "headers": { "x-portkey-api-key": "$PORTKEY_API_KEY" },
      "authHeader": false,
      "compat": { "supportsDeveloperRole": false },
      "models": [
        { "id": "gemma-4-31b-it", "name": "Gemma 4 31B", "reasoning": true,
          "input": ["text", "image"], "contextWindow": 262144,
          "maxTokens": 8192, "cost": { "input": 0, "output": 0 } }
      ]
    }
  }
}
```

- Supported `api` types: `openai-completions`, `openai-responses`, `anthropic-messages`, `google-generative-ai` — covers Ollama, LM Studio, vLLM, SGLang, proxies.
- Provider fields: `baseUrl`, `api`, `apiKey`, `oauth` (`"radius"` gateways), `headers`, `authHeader`, `models[]`, `modelOverrides` (patch built-ins), `compat`.
- Model fields: `id` (required), `name`, `api`, `reasoning`, `thinkingLevelMap`, `input`, `contextWindow`, `maxTokens`, `samplingParams`, `cost` (+ tiers), `compat` (incl. `openRouterRouting`, `vercelGatewayRouting`).
- Providers needing bespoke APIs/OAuth are implemented as TS extensions instead ([custom-provider.md](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/custom-provider.md)).

## 3. Environment variables

([environment-variables.md](https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/environment-variables.md))

**Provider keys** (full table in [providers.md](https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/providers.md)): `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY`, `OPENROUTER_API_KEY`, `DEEPSEEK_API_KEY`, `GROQ_API_KEY`, `MISTRAL_API_KEY`, `NVIDIA_API_KEY`, `CEREBRAS_API_KEY`, `TOGETHER_API_KEY`, `FIREWORKS_API_KEY`, `MINIMAX_API_KEY`, `KIMI_API_KEY`, `HF_TOKEN`, `RADIUS_API_KEY`, … Cloud: `AZURE_OPENAI_API_KEY` (+`AZURE_OPENAI_BASE_URL|RESOURCE_NAME|API_VERSION|DEPLOYMENT_NAME_MAP`), AWS standard chain (`AWS_PROFILE` / `AWS_ACCESS_KEY_ID`+`AWS_SECRET_ACCESS_KEY` / `AWS_BEARER_TOKEN_BEDROCK`, `AWS_REGION`, `AWS_BEDROCK_FORCE_CACHE=1`), Vertex via ADC (`GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION`, `GOOGLE_APPLICATION_CREDENTIALS`), `CLOUDFLARE_API_KEY`(+`ACCOUNT_ID`,`GATEWAY_ID`). Also `VISUAL`/`EDITOR`, `HTTP_PROXY`/`HTTPS_PROXY`.

**PI_\* process configuration**:

| Var | Effect |
|---|---|
| `PI_CODING_AGENT_DIR` | Override config dir (default `~/.pi/agent`) — the multi-instance switch |
| `PI_CODING_AGENT_SESSION_DIR` | Override session storage (`--session-dir` wins) |
| `PI_PACKAGE_DIR` | Override package dir (Nix/Guix store paths) |
| `PI_OFFLINE` | Disable startup network ops (update checks, package updates, telemetry) |
| `PI_SKIP_VERSION_CHECK` | Skip pi.dev version request |
| `PI_TELEMETRY` | `1`/`0` override install/update telemetry + attribution headers |
| `PI_CACHE_RETENTION` | `long` = extended provider prompt caching where supported |
| `PI_SHARE_VIEWER_URL` | Base URL override for `/share` |
| `PI_HARDWARE_CURSOR=1`, `PI_TUI_ESC_TIMEOUT=<ms>` (default 10, 100 over SSH) | Terminal tuning |
| `PI_ALLOW_LOCKFILE_CHANGE=1` | Repo-dev only: allow lockfile commits ([README](https://raw.githubusercontent.com/earendil-works/pi-mono/main/README.md)) |

**Injected for child processes**: markers `AI_AGENT=pi`, `PI_CODING_AGENT=true`; LLM-called bash/powershell tools get session state `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL`, `PI_REASONING_LEVEL` (resolved per command; not injected into user `!`/`!!` commands).

## 4. Extensibility

- **Skills** ([skills.md](https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/skills.md)): implements the [Agent Skills standard](https://agentskills.io/specification). Locations: `~/.pi/agent/skills/`, `~/.agents/skills/` (global); `.pi/skills/`, `.agents/skills/` walking up to git root (project, post-trust); package `skills/` dirs; `settings.json` `"skills": [...]` array (this is how you reuse Claude Code/Codex skills: `"skills": ["~/.claude/skills", "~/.codex/skills"]`); CLI `--skill <path>`. A skill = directory with `SKILL.md` (frontmatter: `name`, `description` required; optional `license`, `compatibility`, `metadata`, `allowed-tools`, `disable-model-invocation`). Progressive disclosure: descriptions in system prompt, body read on demand; invocable as `/skill:name`.
- **Extensions** ([extensions.md](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/extensions.md)): TypeScript modules registering custom tools, slash commands, event handlers, custom UI, even full custom providers. Load with `-e/--extension <path|npm|git>` (repeatable), disable discovery with `--no-extensions`. This is the escape hatch replacing most built-in machinery.
- **MCP**: **not built in — by design**. Usage docs: Pi "intentionally does not include built-in MCP, sub-agents, permission popups, plan mode, to-dos, or background bash." You add MCP-style integrations by writing/installing an extension or package.
- **Custom tools**: via extensions (`registerTool`) or by trimming built-ins: `--tools read,grep,find,ls` allowlist, `--exclude-tools ask_question`, `--no-builtin-tools`, `--no-tools` (built-ins: `read`, `bash`, `powershell`(Win), `edit`, `write`, `grep`, `find`, `ls`).
- **Themes** ([themes.md](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/themes.md)): `/settings` theme picker; `--use-theme <name>` one-shot; `--theme <path>` load; `--no-themes` disable discovery.
- **Prompt templates** ([prompt-templates.md](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/prompt-templates.md)): reusable prompts expanding from bare slash commands (`/templatename`).
- **Pi packages** ([packages.md](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/packages.md)): bundle+share any mix of the above; managed via `pi install|remove|update [--all|--self] |list|config`.
- Everything reloadable live with `/reload`.

## 5. Multi-instance wrappers

The knobs you need to run several isolated Pi instances side-by-side:

- **Config dir switching**: `PI_CODING_AGENT_DIR=/path/to/dir` repoints *everything* — auth.json, settings.json, models.json, skills, packages, sessions. Two dirs = two fully independent identities/providers.
- **Headless / print modes** ([usage.md](https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/usage.md)):
  - `-p` / `--print` — print response and exit; merges piped stdin into the prompt.
  - `--mode json` — structured JSON-event stream output.
  - `--mode rpc` — bidirectional JSONL RPC over stdin/stdout for embedding ([rpc.md](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/rpc.md)).
  - Non-interactive modes never show the project-trust prompt: control via `defaultProjectTrust` (`ask`|`always`|`never`) in settings, or per-run `-a/--approve` / `-na/--no-approve`.
- **Other useful per-invocation flags**: `--provider`, `--model p/id[:level]`, `--api-key`, `--thinking`, `--session-dir`, `-c/--continue`, `-r/--resume`, `--no-session`, `--name`, `--system-prompt`/`--append-system-prompt`, tool allowlists (§4), `--tui-mode regular|fullscreen`.

### Wrapper script example — two isolated instances

```bash
#!/usr/bin/env bash
# pi-wrapper.sh: two provider-isolated pi instances, separate creds/sessions/extensions
set -euo pipefail

pi_claude() {   # Claude Max subscription identity
  PI_CODING_AGENT_DIR="$HOME/.pi-claude" \
    exec pi --provider anthropic "$@"
}

pi_openai() {   # OpenAI key identity
  ANTHROPIC_API_KEY="" \
  OPENAI_API_KEY="${OPENAI_API_KEY:?set OPENAI_API_KEY}" \
  PI_CODING_AGENT_DIR="$HOME/.pi-openai" \
    exec pi --provider openai --model gpt-5.1:high "$@"
}

case "${1:-}" in
  claude) shift; pi_claude "$@" ;;
  openai) shift; pi_openai "$@" ;;
  headless) shift; pi_openai -p "$@" ;;        # batch/scripting use
  json)     shift; pi_openai --mode json "$@" ;; # programmatic consumers
  *) echo "usage: $0 claude|openai|headless|json [args...]"; exit 2 ;;
esac
```

Because `auth.json` is resolved inside `PI_CODING_AGENT_DIR`, each instance keeps its own OAuth tokens/keys, model catalogs, skills and session history; unset the competing provider's env var (as done for `ANTHROPIC_API_KEY` above) to avoid cross-contamination. Add `--session-dir` if you want shared code but separate transcripts.

## 6. Minimal-harness philosophy

Pi's stated design principle ([usage.md → Design Principles](https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/usage.md); echoed on [pi.dev](https://pi.dev/): "Adapt Pi to your workflows, not the other way around"): keep the core small and push workflow-specific behavior into extensions, skills, prompt templates, and packages. It deliberately ships **without** MCP, sub-agents, permission popups, plan mode, to-dos, or background bash — anything beyond read/bash/edit/write/grep/find/ls is something you compose yourself or install as a package. There is likewise no built-in permission system: Pi runs with your user's privileges, and the official answer to sandboxing is containerization (Gondolin micro-VM, plain Docker, OpenShell — [containerization.md](https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/containerization.md)). Full rationale: Mario Zechner's post [*Pi: a coding agent that stays out of your way*](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/). For harness-config purposes this means: nearly every capability boundary you'd want to toggle (tools, providers, prompts, UI) is exposed as data (`settings.json`, `models.json`, `auth.json`) or a CLI flag rather than buried in app logic.
