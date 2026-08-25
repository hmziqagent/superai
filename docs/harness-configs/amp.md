# Amp (Sourcegraph) — Complete Configuration Reference

Compiled 2026-08-25 from primary sources: [ampcode.com/manual](https://ampcode.com/manual) (Owner's Manual), [ampcode.com/modes](https://ampcode.com/modes) (Modes & Models), [ampcode.com/how-i-use-amp](https://ampcode.com/how-i-use-amp), and the Sourcegraph [Amp Security Reference](https://ampcode.com/security). Third-party sources cited inline where used.

---

## 1. Config Surfaces

### CLI (`amp`)
Install: `curl -fsSL https://ampcode.com/install.sh | bash` (macOS/Linux/WSL), `powershell -c "irm https://ampcode.com/install.ps1 | iex"` (Windows), Homebrew `brew install ampcode/tap/ampcode`, or npm `@ampcode/cli` (not recommended). Update with `amp update`. ([manual#installation](https://ampcode.com/manual#installation))

Key CLI invocations:
- `amp` — interactive TUI
- `echo "..." | amp` — pipe input becomes first user message
- `amp -x "prompt"` / `amp --execute` — headless execute mode; prints final message and exits; auto-enabled when stdout is redirected
- `amp --no-tui` — runner-only instance for remotely created threads (`--runner-id <id>` to name it)
- `amp --mcp-config '<json>'` — per-invocation MCP servers without editing settings
- `amp --settings-file <path>` — custom user settings file
- `amp --dangerously-allow-all` — bypass command allowlist warnings ([sourcegraph/amp-examples-and-guides CLI guide](https://github.com/sourcegraph/amp-examples-and-guides/blob/main/guides/cli/README.md))
- `amp config edit [--workspace]`, `amp config keymap`, `amp usage`, `amp login` / `amp logout`, `amp mcp add/approve/oauth login|logout`, `amp tools ...`, `amp skills ...`, `amp plugins ...`, `amp clone <repo>`

Command palette: `Ctrl+O`; mode switch: type `mode` or `Ctrl+S`. Full keymap via `amp config keymap`. ([manual#cli-keymap](https://ampcode.com/manual#cli-keymap))

### Settings files (JSON/JSONC — there is no `amp.toml`)
Amp does **not** use `amp.toml`. It reads JSON(C) settings ([manual#configuration](https://ampcode.com/manual#configuration)):
- **User**: `~/.config/amp/settings.json` (or `.jsonc`) on macOS/Linux; `%USERPROFILE%\.config\amp\settings.json(c)` on Windows
- **Workspace**: nearest `.amp/settings.json` (or `.jsonc`), searched upward from cwd to repo root; workspace overrides user
- **Custom**: `--settings-file <path>`
- **Enterprise managed policy** (override users + workspace): `/Library/Application Support/ampcode/managed-settings.json` (macOS), `/etc/ampcode/managed-settings.json` (Linux), `%ProgramData%\ampcode\managed-settings.json` (Windows); adds `amp.admin.compatibilityDate`
- All settings keys are prefixed `amp.`

Documented core settings keys: `amp.fuzzy.alwaysIncludePaths`, `amp.showCosts`, `amp.git.commit.ampThread.enabled`, `amp.git.commit.coauthor.enabled`, `amp.keymap`, `amp.mcpServers`, `amp.defaultVisibility`, `amp.notifications.enabled`, `amp.remoteThreadCreation.enabled`, `amp.skills.disableClaudeCodeSkills`, `amp.skills.path`, `amp.terminal.copyOnSelect`, `amp.terminal.detailsExpandedByDefault`, `amp.thread.autoArchiveOnQuit`, `amp.tools.disable`, `amp.mcpPermissions`, `amp.updates.mode`. ([manual#core-settings](https://ampcode.com/manual#core-settings))

Other file-based configuration:
- **AGENTS.md guidance files**: repo `AGENTS.md` (cwd, parents, subtrees); personal `$HOME/.config/amp/AGENTS.md` / `$HOME/.config/AGENTS.md`; system-wide `/etc/ampcode/AGENTS.md`, `/Library/Application Support/ampcode/AGENTS.md`, `%ProgramData%\ampcode\AGENTS.md`; fallbacks `AGENT.md` / `CLAUDE.md`. Global AGENTS.md editable in web Settings → Advanced. ([manual#AGENTS.md](https://ampcode.com/manual#AGENTS.md))
- **Skills**: `.agents/skills/` (project), `~/.config/agents/skills/`, `~/.agents/skills/`, `~/.config/amp/skills/`, Claude-compatible `.claude/skills/`, `~/.claude/skills/`, `~/.claude/plugins/cache/`, plus `amp.skills.path`; precedence documented in manual. Skills can bundle MCP servers in sibling `mcp.json`.
- **Plugins**: project `.amp/plugins/`, system `$XDG_CONFIG_HOME/amp/plugins/` (default `~/.config/amp/plugins/`), personal/workspace via ampcode.com settings; precedence: project → system → personal → workspace.
- **MCP OAuth tokens**: stored in `~/.amp/oauth/` ([manual#mcp-oauth](https://ampcode.com/manual#mcp-oauth))

### Credentials storage & auth
Per the [Security Reference](https://ampcode.com/security): "The Amp CLI stores credentials in `~/.local/share/amp/secrets.json` on Linux and macOS, and `%USERPROFILE%\.local\share\amp\secrets.json` on Windows." (Plain file, not OS keychain.)
- Login: `amp login` (browser flow); logout: `amp logout`. Headless: set `AMP_API_KEY` instead.
- Access tokens created at ampcode.com/settings/security#access-token (format `sgamp_...` per the [SDK docs](https://ampcode.com/manual/sdk)).

### VS Code extension / IDE integrations
Install the Amp CLI, ensure your editor is running, then run `amp` — works with VS Code and VS Code-based editors (Cursor, Windsurf), Zed, and Neovim (via [amp.nvim](https://github.com/ampcode/amp.nvim)). Connect via palette command `ide connect`. The JetBrains plugin is deprecated (existing installs work with `amp --jetbrains`, no updates). Integration gives Amp the open file/selection context and IDE-native edits with undo. ([manual#ide](https://ampcode.com/manual#ide))

### Organization/team setup (ampcode.com)
Workspaces at [ampcode.com/workspace](https://ampcode.com/workspace): create or join by invitation; pooled billing, shared threads (feed at `/feed`), workspace plugins/skills repositories managed by admins, integrations (Slack) at `/workspace/integrations`. Enterprise tier ($1,000 one-time purchase) adds SSO/directory sync, thread visibility controls, entitlements, MCP registry allowlists, managed settings, and controls preventing members' personal model routing. ([manual#workspaces](https://ampcode.com/manual#workspaces), [manual#enterprise](https://ampcode.com/manual#enterprise))

---

## 2. Environment Variables (documented)

| Variable | Purpose | Source |
|---|---|---|
| `AMP_API_KEY` | Access token for non-interactive environments (scripts, CI/CD). Set instead of `amp login`. | [manual#cli-non-interactive-environments](https://ampcode.com/manual#cli-non-interactive-environments), [SDK](https://ampcode.com/manual/sdk) |
| `AMP_SETTINGS_FILE` | Custom settings location (community-documented in official examples repo) | [amp-examples-and-guides CLI guide](https://github.com/sourcegraph/amp-examples-and-guides/blob/main/guides/cli/README.md) |
| `AMP_LOG_LEVEL` | Log level: error/warn/info/debug (examples repo) | same |
| `AMP_SKIP_UPDATE_CHECK=1` | Overrides `amp.updates.mode`, disables all update checking | [manual#core-settings](https://ampcode.com/manual#core-settings) |
| `AMP_DISABLE_AMP_THREAD_TRAILER=1` | Disable `Amp-Thread-ID:` commit trailer (equivalent of `amp.git.commit.ampThread.enabled=false`) | manual |
| `AMP_DISABLE_AMP_COAUTHOR_TRAILER=1` | Disable `Co-authored-by: Amp <amp@ampcode.com>` trailer | manual |
| `AMP_FORCE_BEL` | Force terminal bell instead of host audio notifications | manual |
| `AMP_REMOTE_CONTROL_TERMINAL=1/0` | Enable/disable web terminal control of a runner; explicit flags take precedence | [manual#remote-control](https://ampcode.com/manual#remote-control) |
| `HTTP_PROXY`, `HTTPS_PROXY`, `NODE_EXTRA_CA_CERTS` | Standard Node.js proxy/custom-CA variables for corporate networks | [manual#proxies-and-certificates](https://ampcode.com/manual#proxies-and-certificates) |
| `AMP_TOOLBOX` | Colon-separated search path for custom toolbox tools (`.amp/tools` etc.) | [More Tools for the Agent](https://ampcode.com/news/more-tools-for-the-agent) |
| `${VAR}` expansion inside `amp.mcpServers` values (e.g. `"${SRC_ACCESS_TOKEN}"`) | Env interpolation in MCP config fields | [manual#mcp](https://ampcode.com/manual#mcp) |

**Not documented / does not exist:**
- `AMP_URL`: **not present in any official ampcode.com documentation.** It appears only in a third-party Elixir wrapper's README ([hex.pm/packages/amp_sdk](https://hex.pm/packages/amp_sdk)). There is no supported self-hosting or gateway option: Amp is a hosted service; model inference is routed through Sourcegraph's backend (with optional BYOK routing configured on ampcode.com, see §3).
- `NO_COLOR`: no mention in the manual, changelog notes, or security reference as of this date.

---

## 3. Model Selection

**You do not pick raw models in normal use.** Amp exposes four *agent modes* ("the dial") — capability presets, not model selectors ([manual#agent-modes](https://ampcode.com/manual#agent-modes), [modes page](https://ampcode.com/modes)):
- `low` — fast/cheap; Agent GLM-5.2 (med), Oracle GPT-5.6 Sol (high)
- `medium` (default) — Agent GPT-5.6 Sol (med), Oracle GPT-5.6 Sol (high)
- `high` — Agent GPT-5.6 Sol (x-high), Oracle Fable 5 (high)
- `ultra` — Agent Fable 5 (high), Oracle GPT-5.6 Sol (high)

Switch modes via command palette (`Ctrl+O` → `mode`) or `Ctrl+S`. Routing can shift based on connected subscriptions, workspace restrictions, and availability. Deprecated legacy mode names `smart`/`deep`/`rush` still map onto these (`rush`→low; `smart`/`deep`→medium).

System models (fixed by Amp): Puck GPT-5.6 Sol; subagents Search GPT-5.6 Terra, Librarian GPT-5.6 Sol, Read Thread GLM-5.2; dictation GPT-4o Transcribe; realtime voice GPT Realtime 2.1; media view Gemini 3.7 Flash; Painter GPT Image 2; titling GPT-5.6 Luna; compaction GPT-5.6 Sol. Experimental model plugins (Inkling, Kimi K3, Grok 4.6, Fable 5, GLM 5.2) install via `amp plugins add @amp/<name>-mode`.

Reasoning effort toggles: `Alt+D` (per-model where supported), `Alt+R` fast mode toggle; `--fast` flag aliases `--features fast`.

**Custom models/endpoints:** No general custom-provider support exists. What IS possible:
- **BYOK model routing** at ampcode.com/settings/model-routing (personal) and ampcode.com/workspace/model-routing (workspace-wide): add provider API keys (e.g., Anthropic key for Claude Fable 5, which requires Anthropic data-retention enabled on the key's workspace) and link a ChatGPT subscription for more GPT-5.6 routing. Enterprise adds "regional endpoint support" for BYOK providers.
- **Plugin-defined agents** can hardcode a `model:` field (e.g. `'openai/gpt-5.5'`) via `amp.createAgent(...)` + `amp.registerAgentMode(...)`; list usable IDs with `amp plugins show-agent-options`. This is the only way to pin an arbitrary listed model ID, and only from Amp's supported catalog.
There is **no custom base URL / arbitrary OpenAI-compatible endpoint setting** documented anywhere.

---

## 4. Multi-Instance Wrappers

Threads are the parallelism primitive: multiple threads run concurrently in one TUI (sidebar `Ctrl+\`), each thread gets its own conversation; archive-and-new via `Ctrl+C Ctrl+N`. Background threads and plugin-created threads coexist in one CLI process. Remote runners (`amp --no-tui --runner-id X`) let ampcode.com spawn threads into specific machines/directories — effectively N instances addressable by runner ID.

Headless scripting: `amp -x "..."` (+ stdin piping, `--stream-json` for machine-readable output, `--stream-json-input` for programmatic multi-turn, `amp threads continue` to resume threads). ([manual#cli-streaming-json](https://ampcode.com/manual#cli-streaming-json))

**Per-instance auth switching:** since credentials normally live in a single `~/.local/share/amp/secrets.json`, the clean way to give each concurrent instance its own identity is `AMP_API_KEY` in the process environment (it overrides logged-in auth for non-interactive runs). Combined with `--settings-file`, each wrapped invocation can have isolated identity and settings:

```bash
#!/usr/bin/env bash
# amp-as: run amp under a specific account/token + settings profile
# usage: AMP_KEY_NAME=work amp-as "refactor the auth module"
set -euo pipefail

KEY_NAME="${AMP_KEY_NAME:-personal}"
KEYFILE="$HOME/.config/amp-keys/${KEY_NAME}.key"
[[ -f "$KEYFILE" ]] || { echo "no key for '$KEY_NAME'" >&2; exit 1 }

exec env \
  AMP_API_KEY="$(cat "$KEYFILE")" \
  AMP_SKIP_UPDATE_CHECK=1 \
  AMP_SETTINGS_FILE="$HOME/.config/amp/profiles/${KEY_NAME}-settings.json" \
  amp "${@:-}"   # e.g.: amp-as -x "summarize this repo"
```

For true interactive parallelism, launch separate terminal tabs each with its own env (`AMP_KEY_NAME=a amp-as`, `AMP_KEY_NAME=b amp-as`) — note interactive TUI sessions share the secrets file, so env-key isolation is most reliable for `-x`/scripted runs; for interactive isolation use different OS users or containers.

---

## 5. MCP, Subagents/Oracle, Sharing & Privacy

### MCP
Three loading routes, precedence high→low ([manual#mcp-loading-order](https://ampcode.com/manual#mcp-loading-order)): ① CLI flag `--mcp-config`, ② workspace `amp.mcpServers` in `.amp/settings.json`, ③ user `amp.mcpServers` in `~/.config/amp/settings.json`, ④ skills. Local servers: `command`/`args`/`env`; remote: `url`/`headers`; both accept `includeTools` globs; `${VAR}` env expansion supported. Recommended pattern: bundle servers in skill `mcp.json` so tools stay hidden until loaded. Workspace-sourced MCP requires explicit approval (`amp mcp approve <name>`, status via `amp mcp doctor`). OAuth: automatic flow for auto-registering servers; manual via `amp mcp oauth login <name> --server-url ... --client-id ... --client-secret ... --scopes ...` (redirect URI `http://localhost:8976/oauth/callback`); tokens in `~/.amp/oauth/`. Server allow/block rules via `amp.mcpPermissions` array (first match wins; default allow).

### Subagents & Oracle
Subagents spawn automatically (mostly `medium` mode) for independent multi-step work; encourage by mentioning them in prompts. Oracle = second-opinion tool; invoke explicitly ("use the oracle..."). Routing depends on mode + ChatGPT subscription link. Librarian searches GitHub (public + your private repos after connecting GitHub at ampcode.com/settings#code-host-connections). Custom subagents definable in plugins via `createAgent` + registered tool with `parentThreadID`. ([manual#subagents](https://ampcode.com/manual#subagents), [#oracle](https://ampcode.com/manual#oracle), [#librarian](https://ampcode.com/manual#librarian))

### Permissions & safety
By default **Amp never asks before running tools**. Control comes from plugins (`tool.call` handlers returning allow/reject/modify/synthesize), the legacy permission engine (activates if `amp.permissions`, `amp.guardedFiles.allowlist`, or `amp.dangerouslyAllowAll:false` appear in settings), and `amp.tools.disable` for builtin tools. ([manual#permissions](https://ampcode.com/manual#permissions))

### Sharing / privacy
Thread visibility levels: **Unlisted** (anyone with link + workspace), **Workspace-shared**, **Group-shared** (Enterprise), **Private**. Set per-thread via palette `thread: set visibility` or web sharing menu; defaults per-repo via `amp.defaultVisibility` (e.g. `{"github.com/org/repo": "workspace"}`). Solo users default private; workspace members share with workspace by default, admins can change defaults and external-sharing controls. Enterprise extras: Minimal Data Retention, passkey-required interaction, thread retention controls, IP allowlisting. ([manual#thread-sharing](https://ampcode.com/manual#thread-sharing), [appendix](https://ampcode.com/manual/appendix))

---

## 6. Pricing / Token Billing (BYO-key expectations)

([manual#pricing](https://ampcode.com/manual#pricing))
- Monthly subscription includes agent + orbs usage; you can also **link a ChatGPT subscription** for extra GPT-5.6 usage at no extra charge.
- Beyond included usage, billing is **pass-through of actual LLM/tool costs** (LLM APIs, web search) deducted from prepaid Amp credits: "$2 Anthropic + $0.50 OpenAI → $2.50 credits". Zero markup for individuals/non-enterprise workspaces; credits need no subscription.
- **Enterprise costs 50% more** than individual/team pricing.
- Credits: purchased credits expire after 12 months; subscription-included usage expires each cycle (no rollover); workspace credits are pooled (joining transfers personal paid credits to pool, non-refundable on leaving). Invoicing via Stripe. Check balance with `amp usage` or web settings; per-thread cost behind the `$` sidebar icon.
- BYOK reality check: adding your own Anthropic/OpenAI keys via Model Routing changes *which credential pays the provider*, but you still consume Amp credits for Amp's own service layer, and non-BYOK usage bills through credits either way. There is no fully self-hosted/BYO-endpoint mode; Enterprise's "regional endpoint support for bring-your-own-key providers" is the closest feature, gated behind the $1,000 Enterprise upgrade.

---

## Honest gaps summary

- ❌ No `amp.toml` — JSON(C) settings only.
- ❌ No self-hosting/gateway; `AMP_URL` is not an official variable.
- ❌ No arbitrary custom model endpoints/base URLs; model choice limited to Amp's dial + curated BYOK routing + plugin agent modes.
- ❌ No built-in approval prompts for shell commands (opt-in via plugins/legacy permissions).
- ❌ `NO_COLOR` undocumented as of 2026-08-25.
- ✅ Credentials are plain files (`~/.local/share/amp/secrets.json`, `~/.amp/oauth/`), not OS keychain.
