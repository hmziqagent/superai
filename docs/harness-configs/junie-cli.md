# Junie CLI (JetBrains, EAP) — Complete Configuration Reference

**Compiled:** 2026-08-25 · **Status:** Junie CLI is in Early Access Program (EAP)[12]; docs at junie.jetbrains.com/docs were "Last modified: 25 August 2026" at retrieval time. Expect schema drift between builds.

Junie CLI is JetBrains' agentic terminal coding tool for Linux, macOS, and Windows[13]. Install via `curl -fsSL https://junie.jetbrains.com/install.sh | bash` (Windows PowerShell installer equivalent), Homebrew (`brew tap jetbrains-junie/junie && brew install junie`), or npm (`npm install -g @jetbrains/junie`)[13][14]. EAP-specific installer: `https://junie.jetbrains.com/install-eap.sh`; switch back to stable by removing `~/.local/bin/junie` (or `~/.local/bin/junie.bat`) and reinstalling from the stable script[12]. Per-launch channel overrides without changing your default install: `junie --eap`, `--nightly`, `--experimental`, `--release`, explicit form `--channel=eap`, and pinned builds via `junie --eap --use-version=122.1`[14].

---

## 1. Config: file locations & schema

### 1.1 Junie home and file map

| Path | Purpose | Source |
|---|---|---|
| `~/.junie/` | Per-user Junie home (override with `JUNIE_HOME` or `-c/--cache-dir` for caches) | [2][1] |
| `<project>/.junie/config.json` | Project-scope settings (team-shared, commit-friendly) | [3] |
| `~/.junie/config.json` | User-scope personal defaults | [3] |
| `~/.junie/settings.json` | Separate user-settings channel written by the TUI (e.g., subagent settings, transcript view) — *not* part of the `config.json` merge | [3][13] |
| `~/.junie/allowlist.json` | Action Allowlist: approved commands/tools that skip approval prompts | [13][9] |
| `~/.junie/trust/` | Project-trust markers (per exact project or parent dir) | [3][7] |
| `.junie/AGENTS.md`, `AGENTS.md`, `.junie/playbook.md`, `.junie/rules/*.md`, `.junie/guidelines.md` | Guidelines files (resolution order below) | [4][2] |
| `~/.junie/AGENTS.md` | Global guidelines applied to all projects | [4] |
| `.junie/mcp/mcp.json` and `~/.junie/mcp/mcp.json` | MCP server configs (project/user scope) | [10] |
| `.junie/models/*.json` and `$JUNIE_HOME/models/*.json` | Custom LLM model profiles | [11] |
| `~/.junie/extensions` | Default extensions directory (overridable) | [1][2] |
| Session data | Transcript `transcript.md` stored next to session `events.jsonl`; subagent transcripts in a `subagents/` folder | [13] |

macOS/Windows secure storage: the project-trust authentication key is kept in macOS Keychain, Windows Credential Manager, or Linux Secret Service; fallback is an owner-only `authentication-key` file in the trust directory[3].

### 1.2 Auth tokens

- **Junie API key** (`JUNIE_API_KEY`): usage-based-billing token generated at junie.jetbrains.com/cli; passed as `-a/--auth <token>` or the env var[1][2][13].
- **JetBrains Account**: OAuth browser login on first run; subscription-based access[13].
- **License key**: `--auth-license <key>` starts Junie with a specified license[1].
- Interactive credential management via the `/account` slash command (JetBrains account, Junie API key, BYOK keys, custom endpoints)[13].

### 1.3 `config.json` schema

Default locations: user scope `~/.junie/config.json`, project scope `<project-root>/.junie/config.json`. Extra locations via repeatable `--config-location <path>` (env `JUNIE_CONFIG_LOCATION`); disable defaults with `--config-default-locations false` (env `JUNIE_CONFIG_DEFAULT_LOCATIONS`). Relative paths inside a config resolve relative to that config file's folder[3].

Precedence (highest first): **CLI flags → project `config.json` (trusted projects) → user `config.json`**. Example: user sets `"model": "sonnet"`, project sets `"gpt"`, flag `--model opus` wins → effective model `opus`. Note there is deliberately **no single global precedence between `config.json` and `settings.json`** — each setting resolves through its own consumer path[3].

Supported fields[3]:

| Field | Meaning |
|---|---|
| `model` | Default model (built-in ID or `custom:<profile>` ) |
| `effort` | Default reasoning effort |
| `provider` | Default BYOK provider |
| `brave` | Brave mode on/off by default |
| `flags` | Additional feature flags |
| `mcp-locations`, `mcp-default-locations` | MCP discovery folders / toggle defaults |
| `skill-locations`, `skill-default-locations` | Agent skills discovery |
| `command-locations`, `command-default-locations` | Custom slash-command discovery |
| `agent-locations`, `agent-default-locations` | Custom agents discovery |
| `model-locations`, `model-default-locations` | Custom model profile discovery |
| `auto-update` | Automatic update checks on startup |
| `guidelines-location` | Path to guidelines file to use |
| `byok` | Default BYOK API keys per provider (e.g. `{"anthropic": "sk-ant-...", "openai": "sk-..."}`) |
| `proxies` | Custom proxy endpoints (name, kind, api-url, headers) for routing LLM traffic |
| `hooks` | Shell commands on session lifecycle events |

Safety rule: `hooks` from the **default project** config file are ignored — use `~/.junie/config.json` or an explicit `--config-location` file[3].

### 1.4 Project trust

Interactive launches prompt for trust before loading any project-provided inputs; options are Keep untrusted / Trust this project / Trust all projects in `<parent>`. Untrusted projects still work as a workspace, but project config, MCP servers, hooks, extensions, models, skills, agents, commands, guidelines, memory, and migration/onboarding sources are not loaded — a writable temp project directory outside the repo is used instead. Paths given explicitly via flags/env (`--config-location` etc.) stay enabled. Non-interactive runs (one-shot prompts, piped input, ACP, Gateway) are **trusted by design** and load project configuration without prompting[3][7]. Delete markers under `~/.junie/trust` to revoke[7].

### 1.5 Guidelines files (.junie/)

Guidelines are persistent context added to every task; Junie reads `AGENTS.md`-format Markdown[4]. Resolution order (first match wins)[2][4]:

1. **Custom file**: if `JUNIE_GUIDELINES_FILENAME` (or `--guidelines-filename`) is set and exists, it is used exclusively (file placed in `.junie/`)
2. **`.junie/AGENTS.md`** — used exclusively; nothing else combined
3. **Combined defaults**, concatenated in order when present: root `AGENTS.md` + `.junie/playbook.md` + every `.junie/rules/*.md`
4. **Legacy fallback**: `.junie/guidelines.md` file or `.junie/guidelines/` folder (still supported)

Global guidelines from `~/.junie/AGENTS.md` (`%USERPROFILE%\.junie\AGENTS.md` on Windows) apply everywhere; when both global and project exist both are included with clear marking, project takes precedence on conflict, identical content is deduplicated[4]. First open of a repo with other agents' instruction files triggers an import suggestion into `.junie/AGENTS.md`[4]. Technology-specific examples live in JetBrains' junie-guidelines catalog on GitHub[4].

---

## 2. Environment variables

Flags override env vars when both are set (e.g. `JUNIE_MODEL=sonnet` + `--model gpt` → `gpt`) [2]. Full list[2]:

**Authentication**

| Var | Flag equivalent | Notes |
|---|---|---|
| `JUNIE_API_KEY` | `-a, --auth` | Junie API token (junie.jetbrains.com/cli) |
| `JUNIE_ANTHROPIC_API_KEY` | `--anthropic-api-key` | Claude models |
| `JUNIE_OPENAI_API_KEY` | `--openai-api-key` | GPT models |
| `JUNIE_GOOGLE_API_KEY` | `--google-api-key` | Gemini models |
| `JUNIE_GROK_API_KEY` | `--grok-api-key` | Grok models |
| `JUNIE_OPENROUTER_API_KEY` | `--openrouter-api-key` | OpenRouter aggregator |
| `JUNIE_LITELLM_URL` / `JUNIE_LITELLM_API_KEY` | `--litellm-url` / `--litellm-api-key` | LiteLLM proxy base URL / optional key |

**Project & task**: `JUNIE_TASK` (`--task`), `JUNIE_PROMPT` (`--prompt`, interactive auto-submit), `JUNIE_PROJECT` (`-p, --project`).

**Model selection**: `JUNIE_MODEL` (`--model`), `JUNIE_LLM_PROVIDER` (`--provider`; values `openai`, `anthropic`, `google`, `xai`, `openrouter`, `copilot`, `litellm`), `JUNIE_EFFORT` (`--effort`). Custom-model discovery: `JUNIE_MODEL_DEFAULT_LOCATIONS`, `JUNIE_MODEL_LOCATIONS`.

**Config files**: `JUNIE_CONFIG_LOCATION` (repeatable), `JUNIE_CONFIG_DEFAULT_LOCATIONS`, **`JUNIE_HOME`** — overrides the whole `~/.junie` home (no flag equivalent).

**Guidelines**: `JUNIE_GUIDELINES_FILENAME` (`--guidelines-filename`).

**Feature discovery toggles** (all have `*_DEFAULT_LOCATIONS` boolean + `*_LOCATIONS` path-list pairs): MCP (`JUNIE_MCP_*`), skills (`JUNIE_SKILL_*`), slash commands (`JUNIE_COMMAND_*`), custom agents (`JUNIE_AGENT_*`), extensions override (`JUNIE_EXTENSIONS_DEFAULT_LOCATION`).

**Telemetry**: `JUNIE_SHARE_ANONYMOUS_STATISTICS` = `true|false`.

---

## 3. Models: GPT / Claude / Gemini / Grok

### How selection works

Four provider types[5]:

1. **Junie** — models through a JetBrains AI subscription (login or JetBrains AI API key); no extra setup
2. **BYOK** — your own OpenAI / Anthropic / Google / xAI / OpenRouter / GitHub Copilot key
3. **Custom** — JSON model profiles (Ollama, LM Studio, LiteLLM, any OpenAI-/Anthropic-/Google-format endpoint); selected as `custom:<profile-id>`[11]
4. **Proxy** — endpoints configured under `proxies` in `config.json`[3][5]

Switching mechanisms: `/model` in-session (also picks effort; `/effort` changes effort alone)[5][13]; `--model` flag / `JUNIE_MODEL` env / `model` in `config.json` for defaults[5][2][3]; `--provider` / `JUNIE_LLM_PROVIDER` forces a specific BYOK provider[5][2]. Auto-detection: with `--model` but no `--provider`, the Junie provider is preferred when logged in; otherwise the first connected BYOK provider offering that model is used (e.g. `junie --model grok --grok-api-key <key>` routes to xAI automatically)[5].

Built-in aliases (JetBrains-curated; may be repointed over time)[5]:

| Alias | Provider | Current model |
|---|---|---|
| `sonnet` | Anthropic | Claude Sonnet 5 |
| `opus` | Anthropic | Claude Opus 4.8 |
| `gpt` | OpenAI | GPT-5.4 |
| `gpt-codex` | OpenAI | GPT-5.3-codex |
| `gemini-pro` | Google | Gemini 3.1 Pro Preview |
| `gemini-flash` | Google | Gemini 3 Flash |
| `grok` | xAI | Grok 4.3 |

Effort levels: `JUNIE_EFFORT` documents `minimal|low|medium|high|xhigh|max` (availability depends on the model)[5], while the CLI reference lists `low|medium|high` for `--effort`[1] — treat the model-selection page as authoritative and expect per-model subsets. JetBrains recommends keeping default model+effort since higher effort costs more and responds slower[5].

Internal helper model: besides your primary model, Junie always uses a second same-provider model for summarization/routing/memory/filtering (Anthropic→Claude Haiku, Google→Gemini Flash, routing may use GPT-4.1). This applies to BYOK too, so expect calls to models you didn't select[5]. In custom profiles this role is `fasterModel` vs `primaryModel`[11].

### Billing: credits or BYO keys?

Both. Three mutually compatible paths[6][13]:

- **JetBrains AI subscription** (Junie provider) — quota included per subscription plan
- **Junie API key** (`JUNIE_API_KEY`) — usage-based billing against a balance (check with `/usage`: token usage, models used, remaining balance[13])
- **BYOK** — billed entirely by the third-party provider; **no JetBrains subscription required**[6]

If a model is reachable through both BYOK and the JetBrains subscription, **the BYOK key takes priority and requests bill to your provider directly** without consuming JetBrains credits[6][13]. BYOK works standalone or stacked on top of either JetBrains auth method[6][13].

BYOK key entry points: welcome screen, `/account`, or declaratively in `config.json` under `byok`[6][13][3]. Providers and key types: OpenAI, Anthropic, Google, xAI, OpenRouter (API keys); GitHub Copilot (OAuth token)[6]. Custom profiles support `${VAR_NAME}` references in `apiKey`/`extraHeaders` so secrets stay out of committed JSON; missing vars fail profile load with an error naming the variable[11]. Profile fields: `id`, `baseUrl` (full endpoint URL, no path appended), `apiType` (`OpenAICompletion` | `OpenAIResponses` | `Google` | `Anthropic`), `displayName`, `providerName`, `apiKey`, `extraHeaders`, `extraBody`, `temperature`, `maxContextLength`, plus `primaryModel`/`fasterModel` overrides (headers/body merge recursively)[11].

---

## 4. Multi-instance wrappers

Every credential and behavior knob has both a CLI-flag and an env-var form[1][2], which makes per-process isolation straightforward. The two levers that matter for parallel instances:

- **Token switching**: `JUNIE_API_KEY` / `--auth` (Junie billing), provider keys `JUNIE_ANTHROPIC_API_KEY` etc., `JUNIE_LITELLM_URL`+`JUNIE_LITELLM_API_KEY` for a shared proxy[2]
- **State isolation**: `JUNIE_HOME` relocates the entire `~/.junie` tree (settings, allowlist, trust, mcp.json, models, sessions) per instance[2]; `-c/--cache-dir` isolates just caches[1]; `--skip-update-check` prevents concurrent instances racing the updater[1]
- **Project targeting without cd-ing**: `-p/--project` / `JUNIE_PROJECT`[1][2]
- **Trust**: non-interactive runs are trusted by design, so headless wrappers need no trust bootstrap; interactive wrappers can pre-seed markers under `$JUNIE_HOME/trust`[3][7]

CI/headless invocation: positional task or `--task`; pipe stdin with `--input-format text|json`; machine-readable output via `--output-format text|json|json-stream` and `--json-output-file`[1][7]. Canonical CI pattern from the docs: `junie --auth="$JUNIE_API_KEY" "Fix any failing tests"` and `junie --auth="$JUNIE_API_KEY" --review`[13]. Related workflow flags: `--resume`/`--session-id` (resumable sessions), `--merge <branch>` / `--rebase <commit>` (conflict resolution tasks), `--review`, `--plan`, `--prompt`, `--brave`, `--agent-mode classic|chat`[1]. There is also a Junie GitHub Action (`/install-github-action` inside the agent) and GitLab CI/CD integration[14][13].

### Wrapper script example

```bash
#!/usr/bin/env bash
# junie-wrapper.sh — isolated, credentialed Junie CLI invocations
# Usage: junie-wrapper.sh <profile> <project-dir> [task...]
set -euo pipefail

PROFILE="$1"; PROJECT_DIR="$2"; shift 2

case "$PROFILE" in
  jetbrains)   # subscription quota, shared state
    export JUNIE_API_KEY="${JETBRAINS_JUNIE_TOKEN:?unset}"
    ;;
  anthropic)   # BYOK Claude, fully isolated state
    export JUNIE_ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:?unset}"
    export JUNIE_LLM_PROVIDER=anthropic
    export JUNIE_MODEL=sonnet
    ;;
  openai)
    export JUNIE_OPENAI_API_KEY="${OPENAI_API_KEY:?unset}"
    export JUNIE_LLM_PROVIDER=openai
    export JUNIE_MODEL=gpt
    ;;
  grok)
    export JUNIE_GROK_API_KEY="${XAI_API_KEY:?unset}"
    export JUNIE_LLM_PROVIDER=xai
    export JUNIE_MODEL=grok
    ;;
esac

# Isolate everything Junie writes per instance/profile
export JUNIE_HOME="$HOME/.junie-instances/$PROFILE"
mkdir -p "$JUNIE_HOME"

# Headless run: trusted by design, JSON output, no updater races
exec junie \
  --project "$PROJECT_DIR" \
  --output-format json \
  --json-output-file "./junie-out-$$.json" \
  --skip-update-check \
  "$@"
```

Parallel-safe checklist: distinct `JUNIE_HOME` per concurrent instance (or accept shared settings), distinct cache dirs via `--cache-dir` if desired, unique `--session-id`s if resuming, `--config-location` for per-team config overlays, `--config-default-locations=false` + `--mcp-default-locations=false` etc. when you want hermetic runs unaffected by repo-local files[1][2][3].

---

## 5. Plan mode, permissions/approval, MCP

### Plan mode

Read-only codebase analysis producing a design document before any edits; the plan updates alongside requirement changes in-session, then implementation proceeds after confirmation[8]. Enable via: `Shift+Tab` (cycles default → Plan → Debug modes), `/plan` slash command (`/plan <prompt>` submits immediately), or start directly with `junie --plan`, optionally combined with `junie --prompt "..." --plan`[8][13]. Post-plan actions: Confirm and implement / View entire plan (`Ctrl+P`) / Open saved plan Markdown file / Save plan and stop[8]. Plan view tabs typically: Requirements, Technical design, Testing, Delivery steps[8]. Note: plan mode applies only to the next submitted prompt, then reverts to default mode[8].

### Permissions & approvals

Sensitive actions (terminal commands, out-of-project file edits, MCP tool calls) normally require approval[13]. Controls:

- **Action Allowlist** (`~/.junie/allowlist.json`): choosing "Always allow" at a prompt persists the command there; editable by hand[13][9]. Schema: top-level `defaultBehavior` (e.g. `"ask"`), `allowReadonlyCommands` bool, and five rule categories — `fileEditing` (paths outside project / build scripts), `executables` (terminal commands), `mcpTools` (server-prefixed, e.g. `"github-server:"`), `readOutsideProject`, `readSecretFile`. Each rule needs `prefix` (literal string match) **or** `pattern` (glob: `*`, `**`, `?`, `[abc]`, `[!abc]`) plus `action: allow|ask`. Rules evaluate top-to-bottom, first match wins. Chained (`&&`), nested (`$(...)`, backticks, `<(...)`) and multi-line commands must be individually allowed to auto-run[9].
- **Brave mode** — three levels cycled by `/brave` or `Ctrl+B`[13]: **Off** = approve everything not allowlisted; **Auto** = safety-classifier auto-approves commands deemed safe, still prompts for risky/unrecognized ones; **On** = executes all sensitive actions unprompted. Set as default via `brave` in `config.json` or the `--brave` launch flag[3][1].
- **Project trust** (see §1.4) gates whether project-supplied config/MCP/hooks load at all[3].
- Hooks (`hooks` in config.json) can run shell commands on session lifecycle events, e.g. `SessionStart`[3].

### MCP support status

Fully supported in CLI (same JSON config format as the JetBrains IDE plugin)[10]. Config lives at project scope `.junie/mcp/mcp.json` (committable — don't put secrets in it if committed) or user scope `~/.junie/mcp/mcp.json`[10]. Managed interactively via `/mcp` (list servers with name/scope/status Starting|Active|Inactive|Disabled|Failed|Authorization required; enable/disable; edit) including an AI **MCP Installation Assistant** that pulls pre-configured servers or searches the official MCP registry, prompts for secrets, writes `mcp.json`, and verifies startup[10]. Connection types: remote HTTP/HTTPS (with OAuth authorize flow) and local (Docker/npx/binary)[10]. Discovery controls: `--mcp-location <path>` (repeatable) / `--mcp-default-locations true|false`, mirrored by `JUNIE_MCP_LOCATIONS` / `JUNIE_MCP_DEFAULT_LOCATIONS` and `mcp-locations`/`mcp-default-locations` in config.json[10][2][3]. In ACP clients `/mcp` is read-only listing only[10]. Remember that MCP tool usage is itself an approvable action class (`mcpTools`) in the allowlist[9].

---

## Sources

[1] https://junie.jetbrains.com/docs/parameters.html — Junie Docs: CLI reference
[2] https://junie.jetbrains.com/docs/environment-variables.html — Junie Docs: Environment variables
[3] https://junie.jetbrains.com/docs/junie-cli-configuration.html — Junie Docs: config.json
[4] https://junie.jetbrains.com/docs/guidelines-and-memory.html — Junie Docs: Guidelines and memory
[5] https://junie.jetbrains.com/docs/junie-cli-model-selection.html — Junie Docs: Model selection
[6] https://junie.jetbrains.com/docs/byok.html — Junie Docs: BYOK
[7] https://junie.jetbrains.com/docs/junie-headless.html — Junie Docs: Headless mode
[8] https://junie.jetbrains.com/docs/junie-cli-plan-mode.html — Junie Docs: Plan mode
[9] https://junie.jetbrains.com/docs/action-allowlist-junie-cli.html — Junie Docs: Action Allowlist
[10] https://junie.jetbrains.com/docs/junie-cli-mcp-configuration.html — Junie Docs: MCP configuration
[11] https://junie.jetbrains.com/docs/custom-llm-models.html — Junie Docs: Custom LLM models
[12] https://junie.jetbrains.com/docs/junie-cli-eap.html — Junie Docs: Early Access Program
[13] https://junie.jetbrains.com/docs/junie-cli.html — Junie Docs: Quickstart
[14] https://github.com/JetBrains/junie — GitHub: JetBrains/junie README
