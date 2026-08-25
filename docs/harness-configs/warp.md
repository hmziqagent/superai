# Warp Terminal (Warp 2.0, AGPL) — Agentic System Configuration Reference

*Compiled 2026-08-25 from docs.warp.dev primary sources (fetched live this session) plus the `warpdotdev/warp` GitHub org. Every claim cites its page inline. Where documentation is silent or capability is absent, that is stated explicitly.*

Warp is an "Agentic Development Environment": a Rust terminal whose agent is reachable three ways — the **Warp app** (GUI), the **Warp Agent CLI** (`warp` binary, standalone, no app required), and **cloud agents** on Warp's "Automation Platform" (triggers/schedules/parallelism). Account, rules, skills, MCP servers, and model access carry across all three ([docs.warp.dev](https://docs.warp.dev/)). The client is open source under **AGPL v3** at [`warpdotdev/warp`](https://github.com/warpdotdev/warp) ([docs.warp.dev](https://docs.warp.dev/) — "Open source").

---

## 1. Config surfaces

### 1.1 On-disk paths (`~/.warp/` and siblings)

| Surface | macOS | Linux | Windows |
|---|---|---|---|
| Custom themes | `~/.warp/themes/` (subdirs `base16/`, `holiday/`, `standard/`) | same convention (community-documented) | n/a |
| YAML workflows (local) | `$HOME/.warp/workflows/` | `${XDG_DATA_HOME:-$HOME/.local/share}/warp-terminal/workflows/` | `$env:APPDATA\warp\Warp\data\workflows\` |
| Global file-based MCP | `~/.warp/.mcp.json` | `~/.warp/.mcp.json` | `~/.warp/.mcp.json` |
| Custom model routers | `~/.warp/custom_model_routers/` (YAML) | same | same |
| **Agent CLI settings** | `~/.warp_cli/settings.toml` | `~/.config/warp-terminal/cli/settings.toml` (**respects `$XDG_CONFIG_HOME`**) | `%LOCALAPPDATA%\warp\Warp\config\cli\settings.toml` |
| **Agent CLI MCP config** | `~/.warp_cli/.mcp.json` | analogous under CLI config dir | analogous |
| MCP auth tokens | local files; reset all with `rm -rf ~/.mcp-auth` | same | same |
| MCP logs | `$HOME/Library/Group Containers/2BBY89MBSN.dev.warp/Library/Application Support/dev.warp/…` | local on disk (path per OS tab in docs) | local |

Sources: [YAML Workflows](https://docs.warp.dev/terminal/entry/yaml-workflows/) (workflow paths per OS), [CLI configuration](https://docs.warp.dev/agents/cli/configuration/) ("The settings file" section — TOML paths, hot-reload, "never synced to the cloud", independent from app settings), [Customizing the CLI → MCP servers](https://docs.warp.dev/agents/cli/configuration/#mcp-servers) (separate CLI `.mcp.json`), [MCP](https://docs.warp.dev/agents/capabilities/mcp/) (auth-reset command, log locations), [Models and usage in the Agent CLI](https://docs.warp.dev/agents/cli/models-and-usage/) (router YAML location).

Key behavioral notes:
- **App settings and CLI settings are two separate stores.** The CLI's `settings.toml` is plain dotted-section TOML (e.g. `[appearance] theme = "dark"`), watched and hot-reloaded on save, never cloud-synced ([CLI configuration](https://docs.warp.dev/agents/cli/configuration/)).
- You can also ask the agent itself to change settings — the CLI ships a bundled skill plus a schema of every setting key, so prompts like "add the time to my statusline" edit the file safely ([CLI configuration](https://docs.warp.dev/agents/cli/configuration/)).
- **Global Rules are NOT a disk file** — they live in Warp Drive (account-synced), edited via Settings → Agents → Knowledge → Manage Rules, menu AI → Open Rules, or slash commands `/add-rule` and `/open-project-rules` ([Rules](https://docs.warp.dev/agents/capabilities/rules/)). Only *project* rules and *workflows/MCP/routers* are plain files you can version-control.

### 1.2 YAML workflows

Still fully supported ("indefinitely"), though Warp Drive workflows are the recommended successor ([Warp Drive Workflows](https://docs.warp.dev/knowledge-and-collaboration/warp-drive/workflows/)). Scope: **local** (machine-wide dir above), **repository** (`{{repo}}/.warp/workflows/` — anyone cloning gets them), or contributed upstream to [`warpdotdev/workflows`](https://github.com/warpdotdev/workflows/tree/main/specs) ([YAML Workflows](https://docs.warp.dev/terminal/entry/yaml-workflows/)).

File format (`.yml`/`.yaml`), per the spec outlined in [YAML Workflows](https://docs.warp.dev/terminal/entry/yaml-workflows/) and FORMAT.md in the workflows repo:

```yaml
name: Git fixup push            # required
command: git commit --fixup={{ref}} && git push   # required; args as {{double_braces}}
tags: ["git", "GitHub"]         # optional
description: Fixup against ref and push           # optional, indexed for search
author: …                       # optional attribution (surfaces on commands.dev)
shells: [zsh, bash, fish]       # optional validity list
arguments:
  - name: ref                   # argument names map into {{ref}}
    description: Commit to fixup against
    default_value: HEAD         # optional
```

Argument placeholders `{{name}}` sync multi-cursors in the editor; enums and dynamic values come from Warp Drive workflow arguments, which can also inject Warp Drive **environment variables** (static values or dynamic secrets pulled at runtime from 1Password/LastPass/Vault/custom commands — Warp stores the retrieval command, never the secret) ([Warp Drive Workflows](https://docs.warp.dev/knowledge-and-collaboration/warp-drive/workflows/), [Environment variables](https://docs.warp.dev/knowledge-and-collaboration/warp-drive/environment-variables/)).

### 1.3 WARP.md / AGENTS.md rules (project) and Global Rules

From [Rules for agents](https://docs.warp.dev/agents/capabilities/rules/):

- **Project Rules** live in-repo as `AGENTS.md` (the current default) or `WARP.md` (back-compat, still honored; `WARP.md` wins if both exist in the same directory). **Filename must be ALL CAPS.** Place at repo root and/or in subdirectories for targeted guidance.
- Application logic: the root file and the current-directory file apply automatically; when the agent edits files in another subdirectory it makes a best-effort attempt to include that directory's file too.
- **Precedence:** current subdirectory rules → root rules → Global Rules (most specific wins).
- `/init` (Agent Mode) indexes the codebase and generates a starter `AGENTS.md`, and can *link* existing external rule files instead: supported sources are `CLAUDE.md`, `.cursorrules`, `AGENT.md`, `GEMINI.md`, `.clinerules`, `.windsurfrules`, `.github/copilot-instructions.md`.
- **Global Rules** are per-account, created/edited in Warp Drive (Personal → Rules → Global), apply everywhere, and Warp may suggest new ones from usage patterns. They are synced cloud objects, not local files.
- The Agent CLI shares the exact same layered context system, re-scoping rules and skills automatically as you `cd` ([CLI configuration](https://docs.warp.dev/agents/cli/configuration/#project-context-and-rules)).
- Known caveat: community issue [#7199](https://github.com/warpdotdev/warp/issues/7199) reports WARP.md/rules being dropped once the context window fills or after summarization.

### 1.4 Per-project agent configuration summary

Per-project knobs are: `AGENTS.md`/`WARP.md` (rules), `.warp/workflows/*.yaml` (commands), `.warp/.mcp.json` (project MCP servers, manual-approval gated — see §5), and codebase indexing state. There is no per-project permissions file; permission profiles are user-level, not repo-level (see §4/§5).

---

## 2. Environment variables (`WARP_*`)

Documented `WARP_*` variables are sparse — Warp is mostly configured by files/GUI:

| Variable | Purpose | Source |
|---|---|---|
| `WARP_API_KEY` | Authenticate the **Agent CLI** headlessly (CI/servers) instead of browser device-code login: `WARP_API_KEY=YOUR_KEY warp`. Prefer over `--api-key` flag (shell history/process listing leakage). Keys created per [API keys](https://docs.warp.dev/reference/cli/api-keys/). | [CLI quickstart](https://docs.warp.dev/agents/cli/quickstart/) |
| `WARP_TUI_DISABLE_AUTOUPDATE` | Set to any value to skip the CLI's update check for a single launch (persistent off-switch is `general.autoupdate_enabled = false` in `settings.toml`). | [CLI quickstart](https://docs.warp.dev/agents/cli/quickstart/) |
| per-server `env` objects | Not `WARP_*`, but the standard way MCP servers get credentials (`"env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "…"}` inside each server definition). | [MCP](https://docs.warp.dev/agents/capabilities/mcp/) |

Also relevant: **Warp Drive environment variables** are a product feature (click-to-load static vars or dynamic secret-manager-backed values into sessions/subshells/workflows) — user-facing env injection, not configuration-of-Warp ([Environment variables](https://docs.warp.dev/knowledge-and-collaboration/warp-drive/environment-variables/)).

Honest gap: **there is no documented env var to relocate Warp's config home** (no equivalent of `CODEX_HOME`). The only relocation hook documented anywhere is the CLI settings path honoring `$XDG_CONFIG_HOME` **on Linux** ([CLI configuration](https://docs.warp.dev/agents/cli/configuration/)) — see §4 for what that means for wrappers.

---

## 3. Models & providers

### 3.1 Catalog (curated; `model_id`s usable via CLI/Automation Platform)

Source: [Agent model choice](https://docs.warp.dev/agents/inference/model-choice/):

- **Auto routers:** `auto` (responsive), `auto-efficient` (cost), `auto-genius` (adapts up for hard tasks / `/plan`), `auto-open` (best open-weights).
- **OpenAI:** GPT-5.6 Sol/Terra/Luna, GPT-5.5, GPT-5.4, GPT-5.3 Codex, GPT-5.2 Codex, GPT-5.2 — each × reasoning level low→xhigh (e.g. `gpt-5-6-sol-xhigh`).
- **Anthropic:** Claude Opus 5, Fable 5, Sonnet 5, Opus 4.8/4.7/4.6/4.5, Sonnet 4.6/4.5, Haiku 4.5 — effort variants incl. `max`, `xhigh-fast`, thinking on/off (e.g. `claude-5-opus-max`). ⚠️ Claude Fable 5 requires provider-side retention → **not available under ZDR**, enterprise-off by default.
- **Google:** Gemini 3.1 Pro, 3.7/3.6/3.5 Flash.
- **xAI:** Grok 4.6/4.5/4.3 (low→xhigh), Grok Build 0.1; optionally billed via your own SuperGrok subscription instead of Warp credits.
- **Fireworks-hosted open weights:** GLM 5.2, Kimi K3/K2.7 Code/K2.6, Minimax 3/2.7, Qwen 3.7/3.6 Plus, DeepSeek V4 Pro (`*-fireworks` ids).
- Selection: model picker in input/statusline, or `/model` in the CLI; choice persists as the **active profile's base model**. A hidden automatic **model fallback chain** swaps in a comparable model during provider outages and back. **Custom routers** (Settings → Agents → Warp Agent → Custom Routers) let you define your own task→model routing logic in YAML under `~/.warp/custom_model_routers/`.

### 3.2 BYO-key? Yes — but scoped

From [Models and usage in the Warp Agent CLI](https://docs.warp.dev/agents/cli/models-and-usage/) and its linked [BYOK page](https://docs.warp.dev/agents/inference/bring-your-own-api-key/):

- The **Agent CLI supports BYOK for OpenAI, Anthropic, and Google** models, plus connecting an **X Premium / SuperGrok subscription for Grok**. When a covered model is selected, requests bill through *your provider account* and consume no Warp credits.
- Key management: CLI flags `--set-provider-api-key` / `--clear-provider-api-key`; in-session `/api-keys` and `/connect-grok` (Grok connections only manageable in-session).
- Trap: **built-in Auto models always consume Warp credits even with BYOK configured** — you must select a concrete provider model or a custom router whose targets your keys cover.
- The Warp app exposes the same model set; BYOK coverage in-app follows the same linked BYOK page (app-side key entry lives in Settings → Warp Agent per Warp's settings layout).

### 3.3 Server-side vs local — honest assessment

- **All LLM inference routes through Warp's platform.** Warp integrates providers (OpenAI, Anthropic, Google, xAI, Fireworks) under Zero Data Retention contracts; there is **no documented way to point the agent at an arbitrary endpoint** — no `base_url` override, no OpenAI-compatible/Ollama/LiteLLM gateway support anywhere in the model docs ([model choice](https://docs.warp.dev/agents/inference/model-choice/)). BYOK changes *billing*, not the transport: requests still flow via Warp rather than straight from your machine to the provider (that is how credit-exempt-but-platform-mediated billing and the fallback chain work; the docs do not claim direct-to-provider calls).
- **What runs locally:** tool execution (shell commands, file edits, MCP stdio servers spawned on your machine), codebase indexing context gathering, and the terminal itself. What is a hosted service: the Automation Platform (cloud agents, triggers, schedules, observability) and model traffic. The AGPL-open artifact is the **client** ([docs.warp.dev](https://docs.warp.dev/)); the platform backend is Warp-operated (self-host option exists only in the enterprise "your own infrastructure" sense for cloud agents, per the docs' cloud-agent framing — not something the public repo gives you).
- Practical consequence for customization: you can swap *which curated model/router* answers, and extend *tools* via MCP, but you cannot make Warp speak to an uncatalogued provider. If arbitrary-endpoint inference is a hard requirement, pair Warp with a third-party CLI agent it wraps (it runs Claude Code, Codex, OpenCode with its "agent toolbelt" — [docs.warp.dev](https://docs.warp.dev/)), or use those agents directly.

---

## 4. Multi-instance wrappers

### 4.1 Profiles & sessions

- **Agent Profiles** (app: Settings → Agents → Profiles; CLI mirrors them) bundle per-profile: base model, planning model, autonomy levels, command allowlist/denylist, and MCP access rules — e.g. "Safe & cautious" vs "YOLO". New profiles copy Default ([Profiles & Permissions](https://docs.warp.dev/agents/capabilities/agent-profiles-permissions/)). In the CLI the chosen model persists per active profile ([models-and-usage](https://docs.warp.dev/agents/cli/models-and-usage/)). This is the closest thing to named "personas," not separate identities/accounts.
- **Sessions/resume:** CLI prints `warp --resume CONVERSATION_TOKEN` on exit; past conversations are browsable in-session ([CLI quickstart](https://docs.warp.dev/agents/cli/quickstart/)).
- **Parallelism story:** sanctioned fan-out is **cloud agents** (Slack/Linear/GitHub/webhook triggers, schedules, many concurrent agents, shared review) on the Automation Platform ([docs.warp.dev](https://docs.warp.dev/)) — i.e., paid/hosted, not local process spawning.

### 4.2 Headless / CLI automation feasibility

Real automation surface = the **Warp Agent CLI** (`curl -fsSL https://app.warp.dev/download/agent-cli | bash`, or `brew install --cask warp-agent-cli`; macOS/Linux/Windows) ([quickstart](https://docs.warp.dev/agents/cli/quickstart/)). It runs without the app, authenticates headlessly via `WARP_API_KEY`, and re-scopes context per working directory — genuinely usable on CI boxes and over SSH.

Honest limits: every interaction mode the docs describe is **interactive TUI** (device-code browser login interactively, `/`-commands, Ctrl+C handling, shell-mode toggle). I found **no documented non-interactive one-shot flag** (nothing like `claude -p` / `codex exec`) in the quickstart, configuration, or reference pages fetched. Scripting therefore means driving the TUI (expect/piping stdin) or using the **Automation Platform API/SDK** for programmatic runs — the latter being the officially supported path ([docs.warp.dev](https://docs.warp.dev/)).

**`warp://` URIs** exist as *deep links into the app*, e.g. the MCP docs literally link `warp://settings/mcp` to open the MCP pane ([MCP](https://docs.warp.dev/agents/capabilities/mcp/)). These open UI panes; they are **not** a documented RPC/automation interface.

### 4.3 Wrapper script (honest version)

Because there is no `WARP_HOME`-style override, per-instance isolation leans on (a) the CLI's documented `$XDG_CONFIG_HOME` honoring on Linux, (b) `WARP_API_KEY` per credential, and (c) cwd-derived rules/MCP. Full app-data isolation (themes, app settings, Drive cache) is **not achievable by any documented knob** — say so rather than pretending otherwise:

```bash
#!/usr/bin/env bash
# warp-instance: run an isolated Warp Agent CLI instance
#   ./warp-instance work-a "fix flaky tests"
set -euo pipefail
NAME="${1:?usage: warp-instance <name> [prompt...]}"
shift || true

# (a) isolate CLI state. Documented: Linux CLI settings honor $XDG_CONFIG_HOME
#     (~/.config/warp-terminal/cli/settings.toml). Redirecting it moves the CLI's
#     settings + its .mcp.json. NOT documented for the Warp app itself.
export XDG_CONFIG_HOME="$HOME/.warp-instances/$NAME/config"

# (b) per-instance credential (create at docs.warp.dev/reference/cli/api-keys/)
export WARP_API_KEY="$(cat "$HOME/.warp-instances/$NAME/api_key")"

# (c) project context comes from cwd: AGENTS.md/WARP.md, .warp/workflows, .warp/.mcp.json
cd "$HOME/projects/${NAME#*/}" 2>/dev/null || cd "$HOME"

exec warp "$@"
```

Caveats to keep this honest: `XDG_CONFIG_HOME` redirection for the CLI is only documented for the **Linux settings path** ([CLI configuration](https://docs.warp.dev/agents/cli/configuration/)); whether it also relocates the CLI's `.mcp.json` and caches is plausible-but-unverified. Two instances sharing one Warp account still share credits, Drive content, and profiles — profiles differentiate behavior, not identity. True multi-account/multi-provider isolation would need OS users or containers, and even then all instances funnel through Warp's inference layer (§3.3).

---

## 5. MCP configuration & agent permissions/allowlists

### 5.1 MCP servers ([MCP docs](https://docs.warp.dev/agents/capabilities/mcp/))

- **Entry points (app):** Settings → Agents → MCP servers (deep-link `warp://settings/mcp`), Warp Drive → Personal → MCP Servers, or Settings → Agents → Warp Agent → Manage MCP servers. `+ Add` accepts pasted JSON from most MCP clients.
- **Two transport types:**
  - Command/stdio: `{ "name": { "command": "npx", "args": [...], "env": {...}, "working_directory": "/abs/path" } }` — set `working_directory` explicitly when args contain relative paths.
  - Streamable HTTP / SSE: `{ "name": { "url": "https://…", "headers": {"Authorization": "Bearer …"} } }`.
- **Multiple servers at once:** paste a single `{"mcpServers": { … }}` JSON blob; every entry is added.
- **File-based (git-friendly) servers:** global `~/.warp/.mcp.json`; project-scoped `{repo}/.warp/.mcp.json`. The `/agent-add-mcp` bundled skill lets the agent write these for you (choose global vs project). Approval gates: edits to MCP config files require explicit approval, and **project-scoped servers never auto-spawn** — each must be manually started, session-scoped (re-toggle after restart). Global Warp servers auto-spawn; third-party globals spawn only with the "Auto-spawn servers from third-party agents" toggle.
- **Reads other agents' MCP configs too:** Claude Code (`~/.claude.json` + project `.mcp.json`), Codex (`~/.codex/config.toml`), and a generic `~/.agents/.mcp.json` — same definitions follow you across tools (third-party rows require the toggle above).
- **Auth:** env vars, custom headers, or OAuth (browser flow on first spawn; credentials cached on-device; revoke from the MCP pane). Reset all local tokens: `rm -rf ~/.mcp-auth`. Out-of-the-box shared servers ship in the "Shared" section; servers are shareable to teammates; logs land on local disk.
- **CLI has its own separate MCP store**: `~/.warp_cli/.mcp.json` (macOS naming), same `mcpServers` format, hot-reloaded, managed via `/mcp` (start/stop/retry, OAuth reopen, logout) ([CLI configuration → MCP servers](https://docs.warp.dev/agents/cli/configuration/)).

### 5.2 Permissions & allowlists ([Profiles & Permissions](https://docs.warp.dev/agents/capabilities/agent-profiles-permissions/))

Per-profile autonomy across action types — **apply code diffs, read files, create plans, execute commands, interact with running commands (Full Terminal Use), ask clarifying questions** — at four levels:

| Level | Behavior |
|---|---|
| Agent decides | autonomous when confident, asks when uncertain (for diffs currently ≡ Always ask) |
| Always ask | explicit approval every time |
| Always allow | never prompts (all-permissions-allow = "YOLO") |
| Never | action disabled entirely |

- **Command allowlist** (empty by default): regexes that auto-execute, e.g. `ls(\s.*)?`, `grep(\s.*)?`, `find.*`, `echo(\s.*)?`.
- **Command denylist** (default includes `wget(\s.*)?`, `curl(\s.*)?`, `rm(\s.*)?`, `eval(\s.*)?`): **takes precedence over both the allowlist and "Always allow."** Escape hatch: **Run until completion** (auto-approve, `Cmd/Ctrl+Shift+I`) bypasses the denylist for the current task.
- **MCP permissions per profile:** allowlist (call without asking), denylist (require approval, wins), or "agent decides."
- **Ask-questions modes:** never / unless-auto-approve / always (even under auto-approve).
- All of this is GUI-managed (Settings → Agents → Profiles); there is no documented declarative file for permission profiles.

---

## 6. What AGPL v3 means for customization

Facts: Warp's **client** is licensed **AGPL v3** (`LICENSE-AGPL` in [`warpdotdev/warp`](https://github.com/warpdotdev/warp/blob/master/LICENSE-AGPL)), announced as "open source… development happens in the open" ([docs.warp.dev](https://docs.warp.dev/)). Docs describe the open artifact as the client specifically; the Automation Platform/cloud-agent backend is a hosted Warp service and is not part of what the docs place under AGPL.

Practical consequences for someone customizing it:

- **Config-level customization is unaffected** — themes, `AGENTS.md`/`WARP.md`, workflows, MCP servers, custom routers, profiles are data consumed by the client, not derivative works of it. Fork/patch freely.
- **Code-level forks are encouraged and contagious:** modify the client and distribute it → you owe recipients the modified source under AGPL v3. The network clause (AGPL §13) extends this: if you offer the *modified client itself* as a network service, you must provide source to its users. Running stock Warp while doing unrelated dev work triggers nothing.
- **Boundary honesty:** most of the agentic value (model routing, credits, cloud agents, Drive sync) lives behind Warp's API, not in the client tree — so a fork can change UX/local behavior but cannot self-host the brain. Anything the client calls server-side stays proprietary unless Warp documents otherwise (they don't).
- **Trademark/name:** AGPL grants no trademark rights — renamed distributions shouldn't present as "Warp."
- Bottom line: AGPL makes Warp auditable and forkable as a *terminal/client*, with the usual strong-reciprocal obligations if you redistribute or serve a modified build; it does **not** turn Warp into a self-hostable agent stack, and inference remains Warp-mediated regardless (§3.3).

---

## Quick gaps-and-gotchas recap

- Global Rules ≠ files; project rules = `AGENTS.md` (ALL CAPS) with `WARP.md` back-compat and priority.
- No `WARP_*` config-home variable; only documented relocation hooks are `WARP_API_KEY`, `WARP_TUI_DISABLE_AUTOUPDATE`, and Linux CLI settings honoring `$XDG_CONFIG_HOME`.
- BYOK = OpenAI/Anthropic/Google (+SuperGrok for xAI) in the Agent CLI, billing-only; **Auto models always bill Warp credits**; no arbitrary endpoints, ever — inference is platform-mediated.
- No documented headless one-shot prompt flag; automation = interactive CLI + `WARP_API_KEY`, or the Automation Platform API.
- Project MCP servers never auto-spawn; denylist regexes beat "Always allow"; Run-until-completion beats the denylist.
