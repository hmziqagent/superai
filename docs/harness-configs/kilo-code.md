# Kilo Code — Configurable Options Reference

**Scope:** Kilo Code VS Code extension + Kilo CLI (Kilo CLI 1.x, a fork of OpenCode).
**Sources:** kilo.ai/docs (current home of the docs, formerly docs.kilocode.ai), GitHub `Kilo-Org/kilocode`. Retrieved 2026-08-25. Where a claim comes from general Cline/Roo-lineage behavior rather than a fetched page, it is marked *(inferred)*.

---

## 1) Settings & Providers

### 1.1 Where settings live (shared by extension and CLI)

All clients — VS Code extension, JetBrains, CLI — read the **same JSONC config files** ([Settings](https://kilo.ai/docs/getting-started/settings)):

| Level | Path |
|---|---|
| Global | `~/.config/kilo/kilo.jsonc` (Windows: `C:\Users\<user>\.config\kilo\kilo.jsonc`) |
| Project | `./kilo.jsonc`, or `./.kilo/kilo.jsonc` (**`.kilo/` wins if both exist**) |
| Legacy (deep-merged) | `kilo.json`, `opencode.json[c]`, `config.json` in the same locations |
| TUI-only | `~/.config/kilo/tui.jsonc` global; `.kilo/tui.json` per project |

- Kilo does **not** fall back to `.opencode/` directories (`~/.config/opencode`, `./.opencode/`) — migrate those into `~/.config/kilo/` and `./.kilo/` ([Settings warning](https://kilo.ai/docs/getting-started/settings)).
- The Settings webview (gear icon) has tabs: **Providers, Auto-Approve, Models, Agent Behaviour (Agents / MCP Servers), Display, Sandboxing, Experimental, About**. It writes back into the JSONC files; "Local Config" / "Global Config" buttons open the exact file ([Settings](https://kilo.ai/docs/getting-started/settings)).
- You can also just **ask the agent** to change settings — it has a built-in skill that knows the `kilo.jsonc` schema and edits configs itself ([Settings](https://kilo.ai/docs/getting-started/settings)).
- Export/import whole config via **About tab** (`kilo-settings.json`); config files are portable plain text ([Settings](https://kilo.ai/docs/getting-started/settings)).
- Top-level `kilo.jsonc` keys seen in docs: `model` (`provider_id/model_id`), `provider.{id}.options.{apiKey,...}` with `{env:VAR}` interpolation, `mcp`, `permission`, `disabled_providers[]`, `enabled_providers[]`, `hide_prompt_training_models`, `auto_collapse_reasoning`, `terminal_command_display` (`expanded|collapsed`), `remote_control`, `autoupdate` (upstream OpenCode), `agent{}`, `experimental{}` ([CLI config](https://kilo.ai/docs/code-with-ai/platforms/cli), [Settings](https://kilo.ai/docs/getting-started/settings)). Schema: `$schema: "https://app.kilo.ai/config.json"` ([CLI Config Schema](https://kilo.ai/docs/contributing/architecture/config-schema)).
- Experimental block examples: `"experimental": {"batch_tool": false, "openTelemetry": true, "disable_paste_summary": false, "mcp_timeout": 30000, "speech_to_text_model": "openai/whisper-large-v3-turbo"}`; share mode `manual|auto|disabled`; LSP diagnostics exposure ([Settings → Experimental](https://kilo.ai/docs/getting-started/settings)).

### 1.2 Provider panel options

The Providers tab offers ([AI Providers index](https://kilo.ai/docs/ai-providers), [Kilo built-in](https://kilo.ai/docs/ai-providers/kilocode)):

- **Kilo (built-in gateway)** — sign up once, no API-key management, 500+ models at provider rates with zero markup; free models for new users (with a temporary card hold for abuse prevention); credits managed at app.kilo.ai/profile; OAuth flow opens VS Code to authorize ([Kilo provider](https://kilo.ai/docs/ai-providers/kilocode)). Subscription tiers are **Kilo Pass** ($19/$49/$199 mo); pay-as-you-go gateway credits ([CLI comparison](https://kilo.ai/cli/opencode)).
- **OpenRouter**, **Requesty**, **Gemini**, **OpenAI**, **Anthropic**, **DeepSeek**, **Mistral**, **Alibaba (DashScope/Qwen)**, **Cloudflare Workers AI**, AWS Bedrock, Google Vertex, Zhipu AI, etc. ([provider categories](https://kilo.ai/docs/ai-providers)).
- **Local/self-hosted:** **Ollama**, **LM Studio**, **Atomic Chat** (TurboQuant + auto-discovery), **Anaconda Desktop**, and any **OpenAI Compatible** endpoint ([local providers](https://kilo.ai/docs/ai-providers)).
- Enable/disable providers globally: `"disabled_providers": ["kilo","openai"]` or whitelist with `"enabled_providers": ["anthropic"]` ([AI Providers](https://kilo.ai/docs/ai-providers)).

### 1.3 OpenAI-Compatible custom providers (base URL)

Configured in Settings → Providers → **Custom provider** dialog, or directly in `kilo.jsonc` ([OpenAI Compatible](https://kilo.ai/docs/ai-providers/openai-compatible)):

- Fields: **Provider ID** (unique key), **Display name**, **Provider API** (`OpenAI Compatible` chat-completions | `OpenAI Responses` | `Anthropic Messages`), **Base URL** (e.g. `https://api.provider.com/v1`; full endpoint URLs like `.../v1/chat/completions` also accepted), **API key** (optional if header-auth), **Models** (manual or auto-fetched from `/v1/models` with fuzzy search), optional custom **Headers**.
- Model-level extras (context window/token limits, pricing, reasoning effort, variants, tool-calling flags) are edited in `kilo.jsonc` under `provider.<id>.models` ([Custom Models](https://kilo.ai/docs/code-with-ai/agents/custom-models), [CLI](https://kilo.ai/docs/code-with-ai/platforms/cli)).
- Caveat: Azure OpenAI GPT-5 deployments must use the native `azure` provider — generic OpenAI-compatible sends `max_tokens` which Azure GPT-5 rejects ([OpenAI Compatible troubleshooting](https://kilo.ai/docs/ai-providers/openai-compatible)).

### 1.4 Per-mode (per-agent) provider/model overrides

Each agent/mode pins its own model and sampling params — this is the per-mode provider override mechanism in v1.x ([Custom Modes](https://kilo.ai/docs/customize/custom-modes)):

```jsonc
// kilo.jsonc
{ "agent": {
    "code":       { "model": "openai/gpt-4o", "temperature": 0.2 },
    "docs-writer":{ "model": "anthropic/claude-sonnet-4-20250514", "top_p": 0.9 }
} }
```
The model selector also remembers your **last manual pick per agent** across sessions; a config-pinned `model` is only the default until you pick manually (reset button restores config control) ([Custom Modes](https://kilo.ai/docs/customize/custom-modes)). In the *legacy* extension this was per-mode provider/profile selection stored in `custom_modes.yaml` / VS Code globalStorage *(inferred from migration notes below)*.

### 1.5 VS Code `kilo-code.*` settings keys

The current extension keeps almost everything in `kilo.jsonc` instead of VS Code settings. Documented VS Code-level keys are sparse, e.g. `kilo-code.new.diff.renderMarkdown: true` (render Markdown diffs) ([Settings](https://kilo.ai/docs/getting-started/settings)); legacy extension used many `kilo-code.*` workspace keys for auto-approve etc. *(inferred — see §5.3)*. Legacy global storage lives at `~/.config/Code/User/globalStorage/kilocode.kilo-code/settings/` (Linux) ([Custom Modes → Legacy File Locations](https://kilo.ai/docs/customize/custom-modes)).

---

## 2) Modes / Agents System

### 2.1 Built-in agents

Current buildins (CLI naming): **code** (default implementer), **plan** (= legacy *architect*), **ask**, **debug**, **orchestrator** (delegates via `task` tool), **explore**, **general**, plus **review** ([README Agents](https://github.com/Kilo-Org/kilocode#readme), [Custom Modes](https://kilo.ai/docs/customize/custom-modes)). Any of them can be overridden by redefining an agent with the same name ([Overriding Built-in Agents](https://kilo.ai/docs/customize/custom-modes)).

Legacy extension mode slugs mapped during migration: `build`→`code`, `architect`→`plan`; `code`, `ask`, `debug`, `orchestrator` map to their same-named built-ins and are skipped in migration ([Migration](https://kilo.ai/docs/customize/custom-modes)).

### 2.2 Custom modes = agent Markdown files (or config entries)

Defined four ways ([Custom Modes](https://kilo.ai/docs/customize/custom-modes)): (1) ask Kilo to create one; (2) Settings → Agent Behaviour → Agents subtab; (3) `.md` files with YAML frontmatter in `.kilo/agents/` or `.kilo/agent/` (project), `~/.config/kilo/agent/` (global), legacy `.kilocode/agents/` also read; nested dirs namespace names (`backend/sql.md` → agent `backend/sql`); (4) the `agent` key in `kilo.jsonc`.

Frontmatter properties: `description`, `model` (`provider/model`), markdown body = `prompt`, `mode: primary|subagent|all`, `permission` (glob-scoped `allow|deny|ask`), `color` (hex or theme keyword), `steps` (max agentic rounds before forced text answer), `temperature`/`top_p`, `variant`, `hidden`, `disable`. Permissions evaluate **last-match-wins** ([property reference](https://kilo.ai/docs/customize/custom-modes)).

Precedence (low→high): built-in defaults → global `kilo.jsonc` → project `kilo.jsonc` → `.kilo/`(legacy `.kilocode/`) dirs + agent `.md`s → env `KILO_CONFIG_CONTENT`. Same-named agents are **merged property-by-property**, not replaced ([Configuration Precedence](https://kilo.ai/docs/customize/custom-modes)). Organization-managed agents can shadow even built-in names and cannot be removed locally ([Org modes](https://kilo.ai/docs/customize/custom-modes)).

### 2.3 Rules files: `.kilocoderules` / `.clinerules`

Kilo inherited Cline/Roo's custom-instructions mechanism: project rule files named `.kilocoderules` (single file or a `.kilocoderules/` directory of `.md` files) and Cline-compatible `.clinerules`, plus AGENTS.md support via `/init`, layered global + workspace + per-agent custom instructions ([Custom Instructions docs](https://kilo.ai/docs/getting-started/custom-instructions) — page timed out during retrieval; file names are the stable, long-documented set *(partly inferred)*).

### 2.4 `.kiloignore`

Ignore-file support controls which paths the agent/indexer excludes from reading/editing context, analogous to Roo/Cline `.gitignore`-style exclusion *(inferred from lineage; current docs center on sandbox filesystem boundaries instead)*. In the CLI/OpenCode layer, filesystem access scoping is done through `permission.edit` glob rules per agent ([Custom Modes §Restricting Agent File Access](https://kilo.ai/docs/customize/custom-modes)) and the [Sandboxing tab](https://kilo.ai/docs/getting-started/settings/sandboxing) (limits agent FS writes + outbound network; macOS/Linux only, off by default) ([Settings → Sandbox](https://kilo.ai/docs/getting-started/settings)).

---

## 3) Keys & Token Handling

- **Kilo gateway:** after OAuth signup ("Try Kilo Code for Free" → Google sign-in → VS Code authorization), auth is handled seamlessly — **no API key to manage**; web IDEs copy a key manually. Free models need no payment method beyond a temporary verification hold; credits top-up at [app.kilo.ai/profile](https://app.kilo.ai/profile); teams/orgs selectable via `/teams` ([Kilo provider](https://kilo.ai/docs/ai-providers/kilocode), [CLI](https://kilo.ai/docs/code-with-ai/platforms/cli)).
- **BYOK providers:** keys entered via `/connect` (CLI interactive) or `kilo auth`; stored in the CLI auth file, referenced in `kilo.jsonc` as `provider.<id>.options.apiKey` — prefer `{env:VAR}` interpolation so secrets stay out of version-controlled configs ([CLI config](https://kilo.ai/docs/code-with-ai/platforms/cli), [Settings warning](https://kilo.ai/docs/getting-started/settings)).
- **Env-var overrides** ([CLI](https://kilo.ai/docs/code-with-ai/platforms/cli)): `KILO_PROVIDER` (active provider id); `KILOCODE_<FIELD>` maps to `kilocode<Model>`-style fields (e.g. `KILOCODE_MODEL`); other providers `KILO_<FIELD>` (e.g. `KILO_API_KEY` → `apiKey`); `KILO_ORG_ID` selects the organization non-interactively; `KILO_CONFIG_CONTENT` injects full config content (highest agent-config precedence).
- **Privacy mode** (`/privacy` or `privacy_mode` config): blurs PII (balance, team name, Kilo Pass usage) in the TUI and gates `/profile` reveals ([CLI](https://kilo.ai/docs/code-with-ai/platforms/cli)).
- Usage/cost visibility: `kilo stats` (token usage & cost), `kilo profile` ([CLI reference](https://kilo.ai/docs/code-with-ai/platforms/cli)).

---

## 4) Multi-Instance Wrappers

Goal: several isolated Kilo instances (different accounts/providers/configs) side by side.

### 4.1 VS Code extension isolation

Standard VS Code mechanism: launch separate profiles with distinct data + extensions dirs *(inferred — generic VS Code CLI, not Kilo-specific)*:

```bash
code --user-data-dir="$HOME/.vscode-kilo-a" \
     --extensions-dir="$HOME/.vscode-ext-a" \
     --new-window ~/projectA
```
Because Kilo's extension delegates to the CLI backend and reads `~/.config/kilo/`, also isolate the CLI config dir. On Linux/macOS the global path derives from `$HOME/.config`, so a wrapper that sets `HOME` (or runs the CLI with its own `XDG_CONFIG_HOME`) yields a fully separate Kilo identity *(inferred from the documented path layout)*. Per-instance config can additionally be injected wholesale via `KILO_CONFIG_CONTENT` (documented precedence level 5) ([Custom Modes → Precedence](https://kilo.ai/docs/customize/custom-modes)).

### 4.2 CLI headless / automation flags

Documented ([CLI reference](https://kilo.ai/docs/code-with-ai/platforms/cli)):

```bash
kilo run [message..]          # one-shot non-interactive run
kilo run --auto "fix tests"   # autonomous: NO permission prompts — trusted envs/CI only
kilo -c / --continue          # resume most recent session for this workspace
kilo serve                    # headless server; kilo attach <url> to reconnect
kilo acp                      # Agent Client Protocol server (embed in editors)
kilo daemon                   # background daemon management
--print-logs / --log-level DEBUG|INFO|WARN|ERROR   # global flags
```
CI example from docs: `kilo run "Implement the new feature" --auto`. Org routing in scripts uses `KILO_ORG_ID` (there is intentionally no `--org` flag on `kilo run`). Session data export/import via `kilo export <sessionID>` / `kilo import <file>`.

### 4.3 Wrapper script example (isolated account + project)

```bash
#!/usr/bin/env bash
# kilo-wrapper.sh — run an isolated Kilo instance
#   ./kilo-wrapper.sh <instance-name> <project-dir> ["prompt"]
INST="$1"; PROJ="$2"; PROMPT="${3:-}"
ROOT="$HOME/.kilo-instances/$INST"
mkdir -p "$ROOT/config"

# Isolated VS Code instance (extension side)
if command -v code >/dev/null && [[ "${GUI:-0}" == "1" ]]; then
  HOME="$ROOT" code --user-data-dir="$ROOT/vscode-data" \
    --extensions-dir="$ROOT/vscode-ext" --new-window "$PROJ"
fi

# Isolated CLI instance: HOME redirects ~/.config/kilo (auth, kilo.jsonc)
export HOME="$ROOT"
export KILO_CONFIG_CONTENT='{"remote_control": true}'   # optional per-instance overrides
cd "$PROJ" || exit 1
exec kilo ${PROMPT:+run "$PROMPT"}                       # TUI or one-shot run
```
*(HOME/XDG redirection and `KILO_CONFIG_CONTENT` composition are inferred from documented paths + precedence; `kilo run --auto`, `serve/attach/acp`, env overrides are documented.)*

---

## 5) MCP, Auto-Approve, Checkpoints

### 5.1 MCP configuration paths

MCP servers live inside the main config under the top-level `mcp` key — global `~/.config/kilo/kilo.jsonc`, project `./kilo.jsonc` or `./.kilo/kilo.jsonc` (project beats global). UI: Settings → **Agent Behaviour → MCP Servers** ([Using MCP in Kilo Code](https://kilo.ai/docs/automate/mcp/using-in-kilo-code)).

```jsonc
{ "mcp": {
    "my-local-server":  { "type": "local",  "command": ["node","/path/server.js"],
                          "environment": {"API_KEY":"..."}, "enabled": true, "timeout": 10000 },
    "my-remote-server": { "type": "remote", "url": "https://host/mcp",
                          "headers": {"Authorization":"Bearer ..."}, "enabled": true,
                          "timeout": 15000 } } }
```
Transports: local STDIO child process; remote tries StreamableHTTP then falls back to SSE (SSE deprecated per MCP spec 2025-03-26). Remote servers get automatic OAuth 2.0 flows (`"oauth": false` disables). Timeouts default 10 s (local) / 15 s (remote). Windows STDIO servers should be wrapped via `cmd` + args. Disabling unused MCP shrinks the system prompt ([MCP docs](https://kilo.ai/docs/automate/mcp/using-in-kilo-code)). Manage servers via `kilo mcp` ([CLI reference](https://kilo.ai/docs/code-with-ai/platforms/cli)).

### 5.2 Auto-approve granularity

Two layers:

1. **Permission rules** (current, CLI-grade): top-level `permission` key with per-tool values `allow|deny|ask` and **glob scoping** — e.g. `"edit": {"*.py":"allow","*":"deny"}, "bash": "ask"`. Known tools: `read, edit, bash, glob, grep, task, webfetch, websearch, todowrite, todoread`. Rules are last-match-wins; per-agent `permission` blocks override these per mode ([Custom Modes](https://kilo.ai/docs/customize/custom-modes)).
2. **MCP tools**: each namespaced as `{server}_{tool}`; approve prompts offer **Approve Always**, persisting `"my_server_do_something": "allow"` (wildcards OK: `"my_server_*": "allow"`) ([MCP docs](https://kilo.ai/docs/automate/mcp/using-in-kilo-code)). Toggle everything at once with `/auto-approve` (persisted to global config) ([CLI](https://kilo.ai/docs/code-with-ai/platforms/cli)). `kilo run --auto` bypasses all prompts (CI only) ([README](https://github.com/Kilo-Org/kilocode#readme)).
3. *(Legacy extension)* Settings → Auto-Approve offered checkboxes for read/edit/execute/browser/MCP/retry plus an editable allowed-commands list and max-requests cap — inherited from Cline/Roo *(inferred; superseded by permission rules in 1.x)*.

### 5.3 Checkpoints

Checkpointing (snapshot/restore of workspace state around each file edit, shadow-git based) is carried over from the Cline/Roo lineage and exposed in the chat timeline for compare/restore *(inferred — not covered by retrieved pages; verify against https://kilo.ai/docs/getting-started/checkpoints or equivalent)*. The modern safety rails documented explicitly are the [Sandboxing tab](https://kilo.ai/docs/getting-started/settings/sandboxing) (FS-write boundaries + network blocking) and per-step `/undo` in the CLI TUI ([CLI slash commands](https://kilo.ai/docs/code-with-ai/platforms/cli)).

---

## 6) CLI-on-OpenCode Relation

- **Lineage:** "Kilo CLI is a fork of [OpenCode](https://github.com/anomalyco/opencode)" ([README FAQ](https://github.com/Kilo-Org/kilocode#readme)); it "uses the same underlying technology that powers the IDE extensions" and "supports the same configuration options" — the docs point users to [opencode.ai/docs/config](https://opencode.ai/docs/config) for full option coverage ([CLI](https://kilo.ai/docs/code-with-ai/platforms/cli)).
- **Same schema shape, different schema URL & paths:** OpenCode uses `$schema: https://opencode.ai/config.json` at `~/.config/opencode/opencode.json` + `./opencode.json` + `.opencode/` dirs; Kilo uses `$schema: https://app.kilo.ai/config.json` at `~/.config/kilo/kilo.jsonc` + `./kilo.jsonc` / `./.kilo/` ([Config Schema](https://kilo.ai/docs/contributing/architecture/config-schema), [Settings](https://kilo.ai/docs/getting-started/settings), [opencode config docs](https://opencode.ai/docs/config)). Shared keys carry over verbatim: `model`, `provider.<id>.options.{apiKey,baseURL}`, `mcp`, `permission`, `agent`, `command`, `autoupdate`, `keybinds` (in `tui.json`), `{env:VAR}` interpolation.
- **Kilo-specific overlay buckets** maintained on top of upstream: `top` (top-level Kilo keys), `agents` (Kilo primary agents), `experimental` (Kilo experimental keys) — i.e., upstream keys pass through, Kilo adds layers ([CLI Config Schema](https://kilo.ai/docs/contributing/architecture/config-schema)).
- **Migration semantics:** legacy `opencode.json[c]`/`config.json` in the same locations are still read and deep-merged, but `.opencode/` directories are NOT consulted anymore ([Settings](https://kilo.ai/docs/getting-started/settings)). ⚠️ Known bug: merge order is `["kilo.jsonc","kilo.json","opencode.jsonc","opencode.json"]` (`packages/opencode/src/kilocode/config/config.ts` L44), so `opencode.json` currently **wins over** `kilo.jsonc` when they disagree; fix tracked in issue [#7621](https://github.com/Kilo-Org/kilocode/issues/7621) / PR #8781. Debug merges at runtime with `kilo debug config`.
- **Distribution/runtime differences:** Kilo ships as `@kilocode/cli` npm, brew tap, AUR, and release binaries (incl. `-baseline` no-AVX and musl builds), self-updates via `kilo upgrade`; upstream OpenCode remains terminal-first while Kilo adds gateway accounts, teams/SSO, Cloud Agent, remote relay (`remote_control`), and the IDE extensions on the same backend ([README](https://github.com/Kilo-Org/kilocode#readme), [CLI comparison](https://kilo.ai/cli/opencode)).

---

### Quick-reference: every documented knob in one list

`model` · `provider.<id>.options.*` (+`models`, headers, baseURL) · `mcp.<name>.{type,command,url,headers,environment,enabled,timeout,oauth}` · `permission.{read,edit,bash,glob,grep,task,webfetch,websearch,todowrite,todoread,<server>_<tool>}` · `agent.<name>.{description,model,prompt,mode,permission,color,steps,temperature,top_p,variant,hidden,disable}` · `disabled_providers` / `enabled_providers` · `hide_prompt_training_models` · `auto_collapse_reasoning` · `terminal_command_display` · `remote_control` · `experimental.{share,LSP,paste_summary,batch_tool,openTelemetry,mcp_timeout,speech_to_text_model,…}` · `tui.jsonc: attention.{enabled,notifications,sound,volume,sounds}` · env: `KILO_PROVIDER`, `KILOCODE_*`, `KILO_*`, `KILO_ORG_ID`, `KILO_CONFIG_CONTENT`.
