# Letta Code (MemGPT memory-first coding agent) — Configurable Options

Letta Code (`npm install -g @letta-ai/letta-code`, requires Node.js 22.19+) is a stateful coding-agent CLI whose agents keep memory, identity, and skills across sessions ([README](https://raw.githubusercontent.com/letta-ai/letta-code/main/README.md), [CLI overview](https://docs.letta.com/letta-code/cli)). Compiled 2026-08-25 from official sources; every claim below cites its source. Items I could **not** verify against live docs are explicitly flagged ⚠️.

---

## 1. Client configuration (how the CLI connects)

### 1.1 Three backends (where agent *state* lives)

Source: [Self-hosting](https://docs.letta.com/self-hosting), [CLI reference → Backend selection](https://docs.letta.com/platform/cli/reference#backend-selection)

| Mode | Behavior |
|---|---|
| **Local** | Agent state (messages, memory, provider connections) stored on-device in-process; **no Letta account required**. Default dir `~/.letta/lc-local-backend`. |
| **Letta Cloud** | Agents stored in Letta Cloud keep memory/identity/conversations there; harness runs anywhere (laptop, GitHub Actions, VM, Mac mini). Sign in with `/login`. Free accounts: up to 3 agents ([README](https://raw.githubusercontent.com/letta-ai/letta-code/main/README.md)). |
| **Self-hosted App Server** | Central always-on machine hosting local agents, exposed to clients via Agent SDK / `LETTA_BASE_URL`. |

Backend selection flags ([CLI reference](https://docs.letta.com/platform/cli/reference#backend-selection)):
```bash
letta --backend cloud        # Letta-hosted backend
letta --backend local        # store state locally
letta --backend <name>       # overrides saved first-run preference for this invocation
                             # (legacy name "api" still accepted)
letta backend                # show saved default
letta backend cloud          # change saved default
letta setup                  # re-run interactive setup menu
```

Local-state isolation ([Self-hosting](https://docs.letta.com/self-hosting#where-local-state-is-stored)):
```bash
export LETTA_LOCAL_BACKEND_DIR="$PWD/.letta-local"   # default: ~/.letta/lc-local-backend
letta --backend local --new-agent                    # clean-room testing: export LETTA_LOCAL_BACKEND_DIR="$(mktemp -d)"
```
Each agent's MemFS lives under `~/.letta/lc-local-backend/memfs/<agent-id>/memory`.

### 1.2 Connecting to a Letta server — exact variable spellings

Source: [CLI reference → Environment variables](https://docs.letta.com/platform/cli/reference#environment-variables)

```bash
export LETTA_API_KEY="your-key-here"            # key from platform.letta.com/api-keys
export LETTA_BASE_URL="http://localhost:8283"   # base URL of a self-hosted Letta server
```

Exact spellings confirmed verbatim in docs: **`LETTA_API_KEY`**, **`LETTA_BASE_URL`** (example value literally given as `http://localhost:8283`). Set them in `~/.bashrc`/`~/.zshrc` or a `.env` file. Other client env vars: `LETTA_DEBUG=1` (debug logging), `LETTA_CODE_TELEM=0` (telemetry off), `DISABLE_AUTOUPDATER=1`, `LETTA_PACKAGE_MANAGER` (`npm|bun|pnpm`); read-only session vars injected into shell tools: `MEMORY_DIR`, `AGENT_ID`, `CONVERSATION_ID`.

For the newer App Server flavor, the SDK/client uses `LETTA_APP_SERVER_URL` + `LETTA_APP_SERVER_TOKEN` and listens on `ws://127.0.0.1:4500` by default (`letta server --backend local --listen ws://127.0.0.1:4500`) ([Self-hosting](https://docs.letta.com/self-hosting)); health check `/readyz`, persistent volume `/root/.letta` ([Self-hosting → Deployment](https://docs.letta.com/self-hosting#deployment)).

### 1.3 Agent identity selection

Source: [CLI reference → Basic usage](https://docs.letta.com/platform/cli/reference#basic-usage)

```bash
letta                        # resume default conversation, last-used agent
letta --agent <id>, -a       # specific agent by ID
letta --name "Name", -n      # resume agent by name (pinned/recent)
letta --new-agent             # force-create a new agent
letta --import <path>         # new agent from AgentFile (.af) or registry @author/name
letta --conversation <id>, --conv    # specific conversation
letta --resume, -r            # conversation picker
letta --default               # agent's default conversation (needs --agent/--name)
letta --info                  # project info, skills dir, pinned agents (no session)
```
Interactive: `/agents` swap, `/pin` pin, `/new` new conversation. Tutorial agent: `letta --new-agent --personality tutorial` ([README](https://raw.githubusercontent.com/letta-ai/letta-code/main/README.md)).

Remote/multi-env identity routing ([README → Remote environments](https://raw.githubusercontent.com/letta-ai/letta-code/main/README.md)): register a machine with `letta server --env-name "work-laptop"`; list with `letta environments list --online-only`; current env via `letta environments current`; route a headless run onto an environment: `letta -p --agent <id> --environment "work-laptop" "..."` (`--environment cloud` = agent's cloud sandbox).

---

## 2. Server-side provider configuration

### 2.1 Current harness flow: providers attached via `/connect` (not env files)

Sources: [Models](https://docs.letta.com/configuration/models), [Self-hosting → Setup](https://docs.letta.com/self-hosting#setup)

In today's Letta Code, model providers are attached through the `/connect` slash command or its CLI twin `letta connect <provider>` — "Use `/connect` to connect external API keys, coding plans, and local inference servers." Verified examples:

```bash
letta --backend local connect ollama                                  # local Ollama
letta --backend local connect anthropic --api-key "$ANTHROPIC_API_KEY"
letta --backend local connect lmstudio --base-url http://127.0.0.1:1234/v1
```

Provider coverage (from the [Models provider table](https://docs.letta.com/configuration/models#connecting-model-providers)):

| Requested provider | How it connects |
|---|---|
| **OpenAI** | API key (local backend) or via Letta LLM gateway when signed in |
| **Anthropic** | API key (both modes) |
| **Ollama** | Local endpoint (local backend only; gateway doesn't proxy local inference) |
| **vLLM** ⚠️ | Not listed by name — use the generic **"OpenAI-compatible API"** row: "API key + base URL"; endpoint must support Chat Completions + tool calling ([Models → Local models](https://docs.letta.com/configuration/models#local-models)) |
| **OpenRouter** | API key (both modes) |
| **Groq** | API key (local backend) |
| Also supported | Gemini, Bedrock, Vertex, Azure, DeepSeek, Fireworks, Mistral, Together, xAI, Cerebras, Cloudflare AI Gateway, GitHub Copilot, ChatGPT/Codex plans, zAI plans, MiniMax, Moonshot/Kimi, HuggingFace, llama.cpp, LM Studio, Ollama Cloud |

Cloud vs local split: "Cloud providers are managed through Letta's LLM gateway… direct integrations with local inference providers such as LM Studio, Ollama, and llama.cpp are available only for local agents." To point a *cloud* agent at your own server, add an OpenAI-compatible custom provider with a publicly reachable HTTPS base URL ([Models → Local models](https://docs.letta.com/configuration/models#local-models)).

### 2.2 Classic self-hosted Letta server env vars (Docker, port 8283)

The `http://localhost:8283` example under `LETTA_BASE_URL` refers to this classic single-container server ([CLI reference](https://docs.letta.com/platform/cli/reference#environment-variables)).

⚠️ *Confidence caveat:* the dedicated classic-server Docker page was unreachable within this research pass (both `/selfhosting` and alternate paths returned 404). The following env-var spellings match Letta's long-standing Docker deployment convention but were **not re-verified today** — confirm against the current self-hosting guide before relying on them:
- `OPENAI_API_KEY` — enables the built-in OpenAI provider on the server
- `ANTHROPIC_API_KEY` — enables Anthropic
- `OLLAMA_BASE_URL` — e.g. `http://host.docker.internal:11434`, registers Ollama models
- `EMBEDDING_ENDPOINT` / `EMBEDDING_ENDPOINT_API_KEY` / `EMBEDDING_CHUNK_SIZE` (and `OPENAI_API_KEY` for the default OpenAI embedder) — archival-memory embedding config

What *is* verified for deployed servers: set "at least one model provider key" (e.g. `ANTHROPIC_API_KEY`) plus `LETTA_APP_SERVER_TOKEN` in `.env`/Railway vars/`fly secrets` for the [letta-app-server-deployment](https://github.com/letta-ai/letta-app-server-deployment) Docker Compose / Railway / Fly.io setups ([Self-hosting → Deployment](https://docs.letta.com/self-hosting#deployment)).

### 2.3 Per-agent model override

Sources: [Models](https://docs.letta.com/configuration/models), [CLI reference](https://docs.letta.com/platform/cli/reference#model-and-configuration)

- **Agents are model-agnostic**: "Users can change an agent's underlying model at any time, even mid-conversation" via `/model`.
- CLI creation-time overrides: `--model <m>` (`sonnet`, `auto`, `gpt-5-codex`), `--embedding <model>` (embeddings for *new* agents), `--system <preset>` (`letta-claude`, `codex`), `--system-custom "<text>"`, `--personality <name>`.
- **Toolsets switch automatically** with the model family ("GPT models… patch-based editing tool, while Claude models work better with string-based edit tools"); force one with `/toolset` or `--toolset default|codex|gemini`.
- Reasoning effort for BYOK OpenAI-compatible gateways is chosen in the `/model` selector and sent as `reasoning_effort` (`none|minimal|low|medium|high|xhigh|max`; Default omits the field).

---

## 3. Multi-instance wrappers (separate servers per provider)

No official "wrapper script" exists in the docs; the building blocks below are all documented, and the script is an assembled pattern.

Documented primitives:
1. **Per-invocation backend override**: `letta --backend cloud|local …` "overrides the saved first-run backend preference for this invocation" ([CLI reference](https://docs.letta.com/platform/cli/reference#backend-selection)).
2. **Isolated local state dirs**: `LETTA_LOCAL_BACKEND_DIR` gives each instance its own agent store ([Self-hosting](https://docs.letta.com/self-hosting#where-local-state-is-stored)).
3. **Server URL switching**: `LETTA_BASE_URL` selects which self-hosted server the client talks to ([CLI reference](https://docs.letta.com/platform/cli/reference#environment-variables)).
4. **Multiple servers**: run one container/process per provider (different ports, different provider keys, separate volumes at `/root/.letta`), then aim clients at each via `LETTA_BASE_URL` ([Self-hosting → Deployment](https://docs.letta.com/self-hosting#deployment)).
5. SDK equivalent: `new LettaAgentClient({ backend: "remote", url: process.env.LETTA_APP_SERVER_URL ?? "http://127.0.0.1:4500", authToken: process.env.LETTA_APP_SERVER_TOKEN })` ([Self-hosting → Agent SDK](https://docs.letta.com/self-hosting#agent-sdk)).

Illustrative wrapper (assembled pattern, not from docs):

```bash
#!/usr/bin/env bash
# letta-wrapper: pick a provider backend, each with its own isolated server+state
set -euo pipefail

case "${1:-anthropic}" in
  anthropic)
    export LETTA_BASE_URL="http://localhost:8283"     # server started with ANTHROPIC_API_KEY
    export LETTA_LOCAL_BACKEND_DIR="$HOME/.letta/profiles/anthropic"
    shift ;;
  ollama)
    export LETTA_BASE_URL="http://localhost:8284"     # second server wired to OLLAMA_BASE_URL=http://127.0.0.1:11434
    export LETTA_LOCAL_BACKEND_DIR="$HOME/.letta/profiles/ollama"
    shift ;;
  cloud)
    unset LETTA_BASE_URL                              # fall through to Letta Cloud + LETTA_API_KEY
    shift ;;
esac

exec letta --backend local "$@"
# usage: letta-wrapper anthropic            # interactive on the Anthropic-backed server
#        letta-wrapper ollama -p "refactor" # headless on the Ollama-backed server
```

Companion server side (one process per provider):
```bash
docker run -d -p 8283:8283 -v ~/.letta_a:/root/.letta -e ANTHROPIC_API_KEY=... letta/letta   # ⚠️ image/env per classic docs, unverified today
docker run -d -p 8284:8283 -v ~/.letta_o:/root/.letta -e OLLAMA_BASE_URL=http://host.docker.internal:11434 letta/letta
```
Or with the current App Server: `letta server --backend local --listen ws://127.0.0.1:4500` twice with distinct `LETTA_LOCAL_BACKEND_DIR`s.

---

## 4. Memory knobs & skills

### 4.1 Memory model (letta-code: MemFS + dreaming)

Sources: [Memory & dreaming](https://docs.letta.com/letta-code/memory), [CLI reference](https://docs.letta.com/platform/cli/reference)

Letta Code implements memory as **MemFS** — "a git-backed memory filesystem that they can inspect and edit," shared across conversations ([Memory](https://docs.letta.com/letta-code/memory)). Knobs:

| Knob | Command / flag | Notes |
|---|---|---|
| Bootstrap memory | `/init` | Inspects repo, asks about working style, reviews prior sessions |
| Teach | `/remember <fact>` | Agent chooses placement, commits to MemFS |
| Audit | `/doctor` | Checks placement, duplication, system-prompt token usage |
| View | `/memory`, `/palace`, `$MEMORY_DIR` | Desktop memory viewer too |
| Context budget | `/context-limit 200000` | Set/reset max context window |
| Search history | `/search <q>` | Across all messages and agents (≈ recall layer) |
| **Sleep-time compute ("dreaming")** | `/sleeptime` (CLI) or Dream settings (app) | Background subagents consolidate lessons; trigger = after N completed steps **or** on context compaction; optional "Agent reviews before applying" second-pass review |
| Sleeptime flags | `--reflection-trigger off\|step-count\|compaction-event`, `--reflection-step-count <n>` | `--reflection-behavior` deprecated/ignored |
| Blocks at creation | `--init-blocks "persona,project"`, `--block-value label=value`, `--memory-blocks '<json>'`, `--memfs` / `--no-memfs` | Custom memory blocks for new agents ([CLI reference → Memory configuration](https://docs.letta.com/platform/cli/reference#memory-configuration)) |
| Maintenance | `letta memory status\|diff\|pull\|backup\|backups\|restore --force\|export --out\|tokens` | JSON-only subcommands; `letta memfs` is a legacy alias |
| Shared memory | `letta shared-memory create\|attach\|detach\|sync\|history` | Cross-agent repositories |
| Git sync | `/memory-repository set git@github.com:...` | Sync MemFS to a repo ([README](https://raw.githubusercontent.com/letta-ai/letta-code/main/README.md)) |

Classic MemGPT/Letta-server framing for orientation: **core** = always-in-context memory blocks, **archival** = vector-DB retrieval, **recall** = searchable conversation history. In Letta Code these surface as MemFS blocks (+ `/doctor` token audit), semantic/message search (`/search`, `letta messages search`), and full message history (`letta messages list`) respectively ([CLI reference → Subcommands](https://docs.letta.com/platform/cli/reference)).

### 4.2 Skills

Source: [Skills](https://docs.letta.com/letta-code/skills)

Implements the open Agent Skills standard; four scopes:

| Location | Scope |
|---|---|
| `${MEMORY_DIR}/skills/` | **Agent** — persists inside the agent's git-backed MemFS, cloned to whatever machine it runs on |
| `.agents/skills/` | **Project** — committed with the repo, client-side |
| `~/.letta/skills/` | **Computer** — all agents on the machine |
| bundled | Built-in (memory mgmt, search, mods, skill-creator, letta-help…) |

Install/manage:
```bash
letta install <skill-source> --agent <id>   # GitHub dir, Hermes official/..., ClawHub clawhub:<slug>
letta skills list --agent <id>
letta skills delete <skill-name> --agent <id>
```
or just ask the agent to install a skill; or desktop app → Skills → Add skill → Import from GitHub. Invoke directly with `/<skill-name> [instructions]`; browse with `/skills`; create with the built-in `/skill-creator`.

Disabling knobs ([CLI reference](https://docs.letta.com/platform/cli/reference#model-and-configuration)): `--skills <path>` (custom dir), `--skill-sources all,bundled,global,agent,project`, `--no-skills`, `--no-bundled-skills`.

---

## 5. MCP & tools configuration

⚠️ A dedicated Letta-Code MCP config page could not be retrieved in this pass (404 at tried paths). Verified pieces:

- **Skills bridge to MCP**: "A skill can… connect to an MCP server by having the agent run its bundled scripts with the tools available on the selected computer" ([Skills](https://docs.letta.com/letta-code/skills)) — i.e., MCP access is mediated through skills + permissions, credentials belong in [Secrets](https://docs.letta.com/letta-code/secrets), not in the skill.
- **Tool limiting flags** (headless-focused, [CLI reference](https://docs.letta.com/platform/cli/reference#headless-mode)): `--tools "Tool1,Tool2"` (limit set), `--allowedTools "…"` / `--disallowedTools "…"` (pattern allow/block), `--base-tools "…"` (attach at `--new-agent` time), `--toolset default|codex|gemini`.
- **Permissions gate tool execution**: Shift+Tab cycles modes; `--permission-mode unrestricted|standard|acceptEdits`; `--yolo` = unrestricted ([CLI reference](https://docs.letta.com/platform/cli/reference#headless-mode); [Permissions](https://docs.letta.com/letta-code/permissions)). Skills' API calls follow these permissions.
- Related automation surfaces: [Hooks](https://docs.letta.com/letta-code/hooks) (scripts at execution points), [Crons](https://docs.letta.com/platform/cli/reference#scheduling) (`letta cron add --every|--at|--cron`, `--runner local|cloud`), mods (`letta mods enable/disable/update/remove/package`).

---

## 6. CLI flags — headless / non-interactive

Source: [CLI reference → Headless mode](https://docs.letta.com/platform/cli/reference#headless-mode)

```bash
letta -p "commit the changes and push"                       # one-off prompt, non-interactive
letta -p --agent <id> --output-format json "query"           # scripted consumption
letta -p --from-agent <id> "…"                               # agent→agent headless message
letta -p --agent <id> --environment "work-laptop" "…"        # route to a remote environment
```

| Flag | Purpose |
|---|---|
| `-p "prompt"` | Headless mode entry point |
| `--output-format text\|json\|stream-json` | Output shape |
| `--input-format stream-json` | Bidirectional streaming input |
| `--include-partial-messages` | Emit `stream_event` wrappers per chunk (stream-json only) |
| `--yolo` | Permission mode → `unrestricted` |
| `--permission-mode unrestricted\|standard\|acceptEdits` | Explicit permission mode |
| `--disable-memory-guard` | Disable cross-agent memory guard for this parent process |
| `--from-agent <id>` | Inject agent-to-agent system reminder |
| `--tags <csv>` | Tag agents (headless) |

Also relevant for automation: `--no-system-info-reminder` (drop first-turn device/git/cwd reminder), `letta cron` schedules (recurring tasks persist until deleted; `--cron` expressions evaluated in UTC on cloud runner), `letta update`, `LETTA_DEBUG=1`, `LETTA_CODE_TELEM=0`.

---

## Sources

- https://raw.githubusercontent.com/letta-ai/letta-code/main/README.md (letta-ai/letta-code README)
- https://docs.letta.com/letta-code/cli (overview)
- https://docs.letta.com/platform/cli/reference (CLI reference — flags, env vars, subcommands)
- https://docs.letta.com/self-hosting (backends, App Server, deployment, state dirs)
- https://docs.letta.com/configuration/models (providers, toolsets, reasoning)
- https://docs.letta.com/letta-code/memory (memory & dreaming)
- https://docs.letta.com/letta-code/skills (skill scopes & installation)

Unverified/flagged items (marked ⚠️ above): classic-Docker-server env var spellings (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `OLLAMA_BASE_URL`, `EMBEDDING_*`) beyond the deployment guides' `ANTHROPIC_API_KEY` usage; vLLM-by-name support (use OpenAI-compatible custom provider); dedicated MCP configuration page.
