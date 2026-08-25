# Google Antigravity CLI (`agy`) — Configurable Options

> Compiled 2026-08-25. Product is new (~May–Aug 2026); **official docs are thin**. Everything below is restricted to what Google's own pages actually document; gaps are flagged explicitly rather than filled by inference. Doc version observed at compile time: CLI v1.1.17 (docs header); the hands-on Codelab still shows v1.0.7.

**Primary sources** (cited inline throughout):
- [Overview](https://antigravity.google/docs/cli/overview/) · [Installation & Auth](https://antigravity.google/docs/cli/install/) · [Getting Started](https://antigravity.google/docs/cli/getting-started/) · [Using AGY CLI](https://antigravity.google/docs/cli/using/) · [Plugins & Skills](https://antigravity.google/docs/cli/plugins/) · [Sandbox](https://antigravity.google/docs/cli/sandbox/) · [Migrating from Gemini CLI](https://antigravity.google/docs/cli/gcli-migration/) — all under antigravity.google/docs/cli/
- [Google Developers Blog: "An important update: Transitioning Gemini CLI to Antigravity CLI"](https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/) (May 19, 2026)
- [Codelab: Hands-on with Antigravity CLI](https://codelabs.developers.google.com/antigravity-cli-hands-on)

---

## 1) Config files & settings locations

All documented config lives under one tree (note: still namespaced under `~/.gemini/`, not `~/.antigravity/`):

| Path | Purpose | Source |
|---|---|---|
| `~/.gemini/antigravity-cli/settings.json` | Main config: plain JSON. Workspace behavior, safety restrictions, editor prefs, visual style, performance; also `modelProvider`, sandbox toggle, hook definitions | [Using](https://antigravity.google/docs/cli/using/), [Install](https://antigravity.google/docs/cli/install/), [Sandbox](https://antigravity.google/docs/cli/sandbox/) |
| `~/.gemini/antigravity-cli/keybindings.json` | Custom keybindings; delete to reset to defaults; malformed file → valid parts used, broken actions fall back to defaults | [Using](https://antigravity.google/docs/cli/using/) |
| `~/.gemini/antigravity-cli/plugins/<name>/` | Installed plugin bundles (see §5) | [Plugins & Skills](https://antigravity.google/docs/cli/plugins/) |
| `~/.gemini/antigravity-cli/skills/` | Global skills (any `.md` here becomes a global slash command in every workspace) | [Plugins & Skills](https://antigravity.google/docs/cli/plugins/) |
| `.agents/skills/` (project root) | Workspace-local skills | [Plugins & Skills](https://antigravity.google/docs/cli/plugins/) |
| `~/.gemini/config/mcp_config.json` | Global MCP servers | [gcli-migration](https://antigravity.google/docs/cli/gcli-migration/) |
| `.agents/mcp_config.json` (project root) | Workspace MCP servers | [gcli-migration](https://antigravity.google/docs/cli/gcli-migration/) |
| `GEMINI.md`, `AGENTS.md` (workspace) + `~/.gemini/GEMINI.md` (global) | Context/rule files — identical to Gemini CLI, unchanged in migration | [gcli-migration](https://antigravity.google/docs/cli/gcli-migration/) |

Settings management:
- `/config` or `/settings` opens a full-screen overlay listing all options; selections save immediately to disk ([Using](https://antigravity.google/docs/cli/using/)).
- **Launch-flag overrides**: certain settings are overridden per-session by CLI flags — docs cite `--sandbox` and `--dangerously-skip-permissions`. The settings UI shows the override source (e.g., *"Sandbox Mode on overridden by `--sandbox`"*). Other flags documented elsewhere: `-p "<prompt>"` (non-interactive/headless mode) and `--model "Gemini 3.5 Flash (Low)"` ([Codelab](https://codelabs.developers.google.com/antigravity-cli-hands-on)). A dense "CLI Reference" page exists at [antigravity.google/docs/cli/reference](https://antigravity.google/docs/cli/reference) (not fetched here).
- Session management: `/resume` lists previous sessions; on exit the CLI prints the exact command to resume that session; `/fork` branches a conversation into a separate workspace ([Using](https://antigravity.google/docs/cli/using/)).

### Shared harness with Antigravity IDE (Antigravity 2.0)
Verified claims ([Overview](https://antigravity.google/docs/cli/overview/)):
- Both run the **same agent core** ("shared agent harness"); reasoning/tool/code-comprehension improvements land on both.
- **Shared settings sync**: core preferences, permissions, and security configs sync automatically between CLI and desktop IDE — a permission rule change on one platform updates the other.
- **Conversation export**: active conversations can be exported from CLI to the IDE to continue visually.
- The transition blog corroborates: CLI "shares the same agent harness as Antigravity 2.0… ensuring that all future improvements to core agents are automatically applied wherever you use them" ([blog](https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/)).
- ⚠️ Not documented (in fetched pages): whether the sync is file-based or server-side, and exactly *which* keys sync vs. stay local.

---

## 2) Environment variables

Documented (verified):

| Var | Effect | Source |
|---|---|---|
| `GEMINI_API_KEY` | Enables API-key auth — but **only together with `"modelProvider": "gemini"` in settings.json**; setting the env var alone "has no effect". Read strictly from the process environment: `GOOGLE_API_KEY` and `.env` files are **not** consulted. Startup checks only non-emptiness; invalid keys fail on first request | [Install](https://antigravity.google/docs/cli/install/) |
| `GOOGLE_GEMINI_BASE_URL` | Points model requests at a different Gemini-compatible endpoint (custom gateway/base URL) | [Install](https://antigravity.google/docs/cli/install/) |

⚠️ **Not documented** (as of 2026-08-25, in the pages fetched):
- **No `GOOGLE_CLOUD_PROJECT` / Vertex credential variables** appear in the CLI docs I retrieved. The transition blog says enterprise users "can use it now with your Google Cloud projects" (linking to an "Antigravity for enterprises" post), implying a GCP/Vertex path exists, but the concrete env vars (`GOOGLE_CLOUD_PROJECT`, `GOOGLE_APPLICATION_CREDENTIALS`, etc.) are **not documented on any fetched official page**. Do not assume Gemini CLI's Vertex vars carry over verbatim.
- **No `AGY_*` or `ANTIGRAVITY_*` variables are documented anywhere** in the official material retrieved. If they exist, they're undocumented/internal.

---

## 3) Authentication

Three documented methods plus a custom-endpoint escape hatch ([Install](https://antigravity.google/docs/cli/install/)):

1. **Google account OAuth (default)** — On launch, the CLI reads the OS native keyring (Apple Keychain / Linux Secret Service-dbus / Windows Credential Manager) and signs in silently if a valid token profile exists. Otherwise it opens the default browser for sign-in.
   - **Remote SSH flow**: over SSH the CLI detects no local browser and prints a unique authorization URL; you open it locally, sign in, receive an alphanumeric code, and paste it back into the terminal.
   - `/logout` disconnects and purges saved keyring profiles (also clears local cache dirs).
2. **Gemini API key** (headless/CI) — requires BOTH:
   ```json
   // ~/.gemini/antigravity-cli/settings.json
   { "modelProvider": "gemini" }
   ```
   and `export GEMINI_API_KEY="..."`. The CLI then skips sign-in; the header shows "Gemini API key" instead of the account email. Gotchas: `modelProvider` accepts only `gemini`; unset key + set provider = CLI refuses to start; `/logout` is a no-op in this mode.
3. **Custom gateway / base URL** — `export GOOGLE_GEMINI_BASE_URL="https://your-endpoint.example.com"` sends requests to any Gemini-compatible endpoint (documented under the API-key section, i.e., pairs with key auth).

⚠️ **Vertex AI as an explicit auth mode is not documented** in the fetched CLI docs (see §2). Enterprise access is asserted narratively in the [transition blog](https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/) ("use it now with your Google Cloud projects"; Gemini CLI itself survives for enterprises "via paid Gemini and Gemini Enterprise Agent Platform API keys") — treat Vertex-on-agy specifics as unverified until Google publishes them.

---

## 4) Multi-instance wrappers (accounts/profiles)

⚠️ **Honest status: Google does not document any built-in multi-profile/account-switch mechanism for `agy`** — no `--profile`, `--account`, or `AGY_CONFIG_DIR` appears in the fetched official docs. What *is* verified and usable for wrappers:

- **API-key identity is per-environment**: `GEMINI_API_KEY` + `modelProvider` are read per shell/process, so each shell can be a different "account" trivially ([Install](https://antigravity.google/docs/cli/install/)).
- **OAuth tokens live in the OS keyring**, keyed implicitly per user session — not per configurable profile ([Install](https://antigravity.google/docs/cli/install/)).
- **All disk config sits under `~/.gemini/antigravity-cli/`**, so config isolation is possible by relocating `$HOME` (undocumented workaround, standard Unix technique — verify before relying on it).

Wrapper example (community-style pattern, built only on the documented facts above):

```bash
#!/usr/bin/env bash
# agy-acct — run Antigravity CLI as a specific "account".
# Mode A (documented): distinct API-key identities per shell.
# Mode B (undocumented workaround): isolated $HOME => isolated keyring/config tree.
set -euo pipefail

case "${1:-}" in
  work)
    # A: API-key identity (per Google docs: env var + modelProvider setting)
    exec env GEMINI_API_KEY="$(pass show agy/work-key)" agy "${@:2}"
    ;;
  personal-home)
    # B: full config isolation. All agy state lives under ~/.gemini/antigravity-cli/,
    # so swapping $HOME swaps accounts, plugins, skills, sessions, and keyring fallbacks.
    export HOME="$HOME/.agy-homes/personal"
    mkdir -p "$HOME"
    exec agy "${@:2}"
    ;;
  *)
    echo "usage: agy-acct {work|personal-home} [agy args...]"; exit 1 ;;
esac
```

Caveats to test yourself: whether `agy` honors `$HOME` overrides on macOS (keychain may ignore it), and whether concurrent instances lock `settings.json` or the session store. Nothing in the docs addresses concurrency — assume unverified.

---

## 5) Sandboxing, Skills/Hooks, parallel subagents, MCP

### Sandboxing ([Sandbox](https://antigravity.google/docs/cli/sandbox/))
- Toggle in `~/.gemini/antigravity-cli/settings.json`: `"enableTerminalSandbox": true` (**boolean, default `false`**). Per-session overrides: `--sandbox` flag, `--dangerously-skip-permissions` ([Using](https://antigravity.google/docs/cli/using/)).
- Native OS containment (no VM/container overhead): Linux → `nsjail` (namespaces+cgroups: CPU/memory/path visibility); macOS → `sandbox-exec` (policy profiles restricting FS access and raw TCP); Windows → `AppContainer`.
- Approval prompts adapt to sandbox state: with sandbox on, agents get "run without sandbox restrictions" escape option per single execution; with it off, a "run in sandbox" option exists for risky commands. Fine-grained allow/deny lives in a separate "Permissions Engine" doc at [antigravity.google/docs/cli/permissions](https://antigravity.google/docs/cli/permissions) (not fetched).

### Skills ([Plugins & Skills](https://antigravity.google/docs/cli/plugins/))
- Markdown blueprints with frontmatter (`name:`, `description:`) that auto-compile into TUI slash commands (e.g., `.agents/skills/format-tests.md` → `/format-tests`).
- Locations: workspace `.agents/skills/` (repo-committed) or global `~/.gemini/antigravity-cli/skills/`.

### Plugins (bundles) + Hooks ([Plugins & Skills](https://antigravity.google/docs/cli/plugins/))
- Plugin layout under `~/.gemini/antigravity-cli/plugins/<name>/`: required `plugin.json` manifest (fields: `name` matching `^[a-zA-Z0-9-_]+$`, optional `description`; schema at `https://antigravity.google/schemas/v1/plugin.json`), optional `mcp_config.json`, `hooks.json`, `skills/`, `agents/` (subagent definition templates), `rules/`.
- Subcommands: `agy plugin list | install <path> | enable <n> | disable <n> | uninstall <n>` and `agy plugin import gemini` (§6).
- **Hooks**: intercept actions pre/post tool execution (e.g., run `prettier` after writes). Defined in a plugin's `hooks.json` *or* directly in primary `settings.json`. Inspect live hooks with `/hooks`.

### Parallel subagents
- Verified: plugins can ship `agents/` subagent definition templates ([Plugins & Skills](https://antigravity.google/docs/cli/plugins/)); the blog states Antigravity CLI "orchestrates multiple agents for complex tasks in the background," enabling large refactors/research without blocking the terminal ([blog](https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/)); Subagents are listed among features carried over from Gemini CLI.
- ⚠️ Thin docs: I found **no fetched page documenting how to define/invoke subagents step-by-step or configure their parallelism** (no limits, model assignment, or orchestration syntax). The `/fork` command (separate workspace + branched conversation) is the closest documented manual parallelism primitive ([Using](https://antigravity.google/docs/cli/using/)).

### MCP ([Plugins & Skills](https://antigravity.google/docs/cli/plugins/), [gcli-migration](https://antigravity.google/docs/cli/gcli-migration/))
- Servers live in dedicated `mcp_config.json` profiles (global: `~/.gemini/config/mcp_config.json`; workspace: `.agents/mcp_config.json`) instead of being nested in settings like Gemini CLI did.
- Schema change for remote SSE/websocket servers: legacy `url`/`httpUrl` keys → **`serverUrl`**; per-server `env` blocks supported (e.g., `AUTH_TOKEN`).
- Interactive manager overlay: `/mcp`. Full server/auth documentation lives at [antigravity.google/docs/mcp](https://antigravity.google/docs/mcp) (not fetched; shared with the IDE product family).

---

## 6) Gemini CLI migration

### Timeline (all from the official [transition blog](https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/), posted May 19, 2026)
- **May 19, 2026**: Antigravity CLI becomes available to everyone.
- **June 18, 2026**: Gemini CLI and Gemini Code Assist IDE extensions **stop serving requests** for Google AI Pro/Ultra subscribers and for free individual users (Gemini Code Assist for individuals). Same date: Gemini Code Assist for GitHub — no new org installs; existing requests stop "in the following weeks." No grace period is mentioned.
- **Enterprises exempt**: Gemini Code Assist Standard/Enterprise license holders keep Gemini CLI + IDE extensions with latest models; Gemini CLI remains reachable "via paid Gemini and Gemini Enterprise Agent Platform API keys."
- Third-party posts (e.g., [vibecoder.me](https://blog.vibecoder.me/gemini-cli-shutdown-june-18-antigravity-migration)) claim the Antigravity free tier is 20 agent-requests/day, down from 250 earlier in 2026 — ⚠️ **unverified by official pages fetched here**; check [antigravity.google/docs/cli/credits](https://antigravity.google/docs/cli/credits) ("AI Credits") for current quotas/pricing.

### What carries over ([gcli-migration](https://antigravity.google/docs/cli/gcli-migration/), [blog](https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/))
| Asset | Status |
|---|---|
| Agent Skills, Hooks, Subagents, Extensions | Explicitly preserved; Extensions rebranded **Antigravity plugins** |
| One-time onboarding import | First `agy` launch auto-detects legacy Gemini CLI profiles, offers checklist conversion, migrates session tokens into OS keyring, maps visual/render settings |
| Extensions → plugins | `agy plugin import gemini` parses legacy extension manifests; commands→skills, mcpServers→`mcp_config.json` |
| Context rules | `GEMINI.md` + `AGENTS.md` (workspace) and `~/.gemini/GEMINI.md` (global) — **unchanged, no edits needed** |
| Global skills | `~/.gemini/skills/` → `~/.gemini/antigravity-cli/skills/` |
| Workspace skills | `.gemini/skills/` → `.agents/skills/` |
| MCP config | Inline `mcpServers` in `~/.gemini/settings.json` → standalone `mcp_config.json` (global `~/.gemini/config/`, workspace `.agents/`); rename `url`/`httpUrl` → `serverUrl` |

### What does NOT carry over / differs
- The binary and repo: Gemini CLI was OSS (~100k stars, ~6k PRs per the blog); Antigravity CLI is a **new closed-source Go binary** installed via `curl -fsSL https://antigravity.google/cli/install.sh | bash` (binary at `~/.local/bin/agy`; Windows `%LOCALAPPDATA%\agy\bin`). Installer flags: `--skip-aliases`, `--skip-path` ([Install](https://antigravity.google/docs/cli/install/)).
- Auth storage moves from Gemini CLI's credentials to OS-keyring "token profiles" (imported once by onboarding).
- ⚠️ Blog admits "there won't be 1:1 feature parity right out of the gate" — audit any CI scripts against the [CLI Reference](https://antigravity.google/docs/cli/reference) before the cutover; headless runs should prefer the documented API-key mode (§3) since browser OAuth dies with the free consumer endpoint.
