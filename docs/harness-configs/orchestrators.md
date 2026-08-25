# Multi-Harness Orchestrator GUIs — Configuration Reference

**Scope:** How three GUI orchestrators — wrappers that spawn and manage *other* coding-agent harnesses — are configured: which harnesses they can launch, how their executors are located, how credentials/env vars are injected, and how isolated workspaces are created.

**Apps covered:** (A) Vibe Kanban · (B) Conductor · (C) Sculptor
**Compiled:** 2026-08-25, from primary documentation (doc sites + GitHub repos/docs folders) cited inline.

---

## A) Vibe Kanban

> **Status:** Sunsetting as a company product; the project "will continue as open source and community maintained" ([vibekanban.com](https://vibekanban.com/) banner; announcement linked from the repo README, [github.com/BloopAI/vibe-kanban](https://github.com/BloopAI/vibe-kanban)). Repo remains the OSS home under Apache-2.0 (last release v0.1.44, Apr 2026); third-party write-up confirms the Apache-2.0 community-maintenance transition ([nimbalyst.com](https://nimbalyst.com/blog/vibe-kanban-after-bloop-whats-next/)). Docs below are from vibekanban.com/docs, which was live at compile time.

### What it is & install

Kanban board + "workspaces" where external coding agents execute tasks. Not an agent itself: "We're not a coding agent, but work seamlessly with … Claude Code, Codex, Gemini CLI and OpenCode" ([vibekanban.com/docs](https://vibekanban.com/docs/)). Run locally with `npx vibe-kanban`; recent builds auto-install a Tauri desktop app by default (`--browser` for headless server) ([repo README](https://github.com/BloopAI/vibe-kanban)).

### Which harnesses it spawns (supported coding agents)

Ten integrations, each requiring you to install + authenticate the underlying CLI yourself before Vibe Kanban will offer it ([Supported Coding Agents](https://vibekanban.com/docs/supported-coding-agents)):

| Harness | Notes |
|---|---|
| Claude Code | auth via its own login flow before launching VK ([agent page](https://vibekanban.com/docs/agents/claude-code)) |
| OpenAI Codex | Codex CLI |
| GitHub Copilot | Copilot CLI |
| Gemini CLI | Google |
| Amp | Amp Code |
| Cursor Agent CLI | Cursor's CLI agent |
| OpenCode | SST OpenCode |
| Droid | Factory AI |
| CCR (Claude Code Router) | load-balances across multiple Claude Code instances ([agent-configurations doc](https://vibekanban.com/docs/settings/agent-configurations)) |
| Qwen Code | Qwen CLI |

### Executor config (profiles, models, permissions)

- Per-agent reusable **Agent Profiles** ("configurations"): plan mode, permission skipping, model choice, reasoning effort, sandbox/approval levels. Examples: CLAUDE_CODE (`plan`, `claude_code_router`, `dangerously_skip_permissions`), CODEX (`sandbox`: read-only/workspace-write/danger-full-access; `approval`; `model_reasoning_effort`), CURSOR (`force`, `model`), OPENCODE (`model`, `agent`), GEMINI (`model`, `yolo`), AMP (`dangerously_allow_all`), DROID (`autonomy`, `model`, `reasoning_effort`) ([Agent Profiles & Configuration](https://vibekanban.com/docs/settings/agent-configurations)).
- **Install paths:** there is no documented executable-path override; VK expects each CLI already on PATH, installed and authenticated (troubleshooting flow: "Check that your agent … is installed: run the CLI command manually"; "Verify API keys are configured in Settings → Agents") ([creating-workspaces troubleshooting](https://vibekanban.com/docs/workspaces/creating-workspaces)).
- Default agent set globally in Settings → General → Default Coding Agent; overridden per attempt in the chat-input dropdown.

### Credential injection (the key wrapper mechanic)

Each agent profile has an **Environment Variables** section whose values are injected into the agent process at launch **and override shell-set variables** ([agent-configurations](https://vibekanban.com/docs/settings/agent-configurations)). Documented recipes:

- Z.ai/GLM behind Claude Code: `ANTHROPIC_BASE_URL=https://api.z.ai/api/anthropic`, `ANTHROPIC_AUTH_TOKEN=<key>`
- OpenRouter: `ANTHROPIC_BASE_URL=https://openrouter.ai/api`, `ANTHROPIC_AUTH_TOKEN=<key>`, `ANTHROPIC_API_KEY=""`
- Generic Anthropic-compatible endpoint: `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_API_KEY`

Profiles are fully isolated, so a GLM/OpenRouter profile coexists with your normal Anthropic login.

### MCP configuration

Settings → **MCP Servers**: pick the coding agent, then edit a per-agent `{"mcpServers": {...}}` JSON block (stdio command/args style), or one-click-install popular servers (Context7, Playwright, Exa, Chrome DevTools…). **Important mechanic:** saving writes into *that agent's own global config file*, so "these changes … will persist even if you stop using Vibe Kanban" ([Connecting MCP Servers](https://vibekanban.com/docs/settings/mcp-servers)). Separately, VK itself exposes an MCP server so external clients (e.g., Claude) can drive it ([VK MCP Server](https://vibekanban.com/docs/integrations/vibe-kanban-mcp-server)).

### Worktree setup

Creating a workspace: (1) creates a **git worktree** (isolated dir + branch, original repo untouched), (2) makes a working branch off your target branch (auto names like `vk/abc123-add-login-page`), (3) starts the selected agent session, (4) runs the repo's setup script. Worktrees live under `.vibe-kanban-workspaces/` (**configurable** in Settings → General → Workspace Directory); multi-repo workspaces are supported ([Creating Workspaces](https://vibekanban.com/docs/workspaces/creating-workspaces)).

### Per-project / per-repo config

Settings → Projects (project-level overrides) and Settings → Repositories ([Projects & Repositories](https://vibekanban.com/docs/settings/projects-repositories)):
- **Dev-server script** — enables the built-in preview browser; must print its URL to stdout.
- **Setup script** — runs once when the workspace starts, before the agent works (e.g., `npm install`). A project flag allows running the setup script **in parallel** with the coding agent instead of sequentially (repo PR #1446, [commit history](https://github.com/BloopAI/vibe-kanban/commit/76877ea6317644f879cb8cc4b6732ec337b0198d)).
- **Cleanup script** — runs when a workspace closes (stop containers, free ports, format code); should be idempotent.

### API / port / server configuration

Runtime env vars for the VK server itself ([repo README, Environment Variables section](https://github.com/BloopAI/vibe-kanban)):

| Variable | Default | Purpose |
|---|---|---|
| `PORT` | auto-assign | Production server port (dev: frontend port, backend takes PORT+1) |
| `BACKEND_PORT` | `0` (auto) | Backend port (dev only) |
| `FRONTEND_PORT` | `3000` | Frontend dev port (dev only) |
| `HOST` | `127.0.0.1` | Backend bind host |
| `MCP_HOST` / `MCP_PORT` | HOST / BACKEND_PORT | VK's own MCP server endpoint |
| `VK_ALLOWED_ORIGINS` | unset | Required behind a reverse proxy/custom domain, else 403 |
| `DISABLE_WORKTREE_CLEANUP` | unset | Debug: disables worktree cleanup |
| `VIBEKANBAN_REMOTE_JWT_SECRET` | — | Remote-access server secret (validated base64 ≥32 bytes; from commit history) |

Remote access pairs a phone/client to your host instance via pairing code through [cloud.vibekanban.com](https://vibekanban.com/docs/remote-access); self-hosting Cloud via Docker Compose is documented ([self-hosting docs index](https://vibekanban.com/docs/self-hosting/deploy-docker)).

---

## B) Conductor (Mac)

Source: [conductor.build/docs](https://www.conductor.build/docs) unless noted.

### What it is & install

macOS desktop app that "lets you run Claude Code, Codex, Cursor, and OpenCode in parallel," giving every task "its own workspace, branch, files, terminal, diff, and review path" ([Introduction](https://www.conductor.build/docs)). Under the hood these are ordinary git worktrees of your repo — one checkout per task — with your locally-authenticated CLIs doing the work ([Isolated workspaces](https://www.conductor.build/docs/concepts/workspaces-and-branches); workspaces live under `~/conductor/workspaces/` per [Troubleshooting](https://www.conductor.build/docs/troubleshooting/issues)).

### Which agents selectable & executor paths

Four harnesses: Claude Code, Codex, Cursor, OpenCode ([docs intro](https://www.conductor.build/docs); YouTube walkthrough lists "Claude Code, Codex, Cursor Composer"). Executor/provider config lives in TOML settings ([Settings reference](https://www.conductor.build/docs/reference/settings/reference)):

| Key | Meaning |
|---|---|
| `claude_code_executable_path` | Override Claude Code binary location |
| `codex_executable_path` | Override Codex binary location |
| `claude_provider` | Anthropic / Bedrock / Vertex |
| `bedrock_region`, `vertex_project_id` | Cloud-provider routing for Claude |
| `codex_provider` | Provider backing Codex |
| `ssh_key_path` | SSH key for cloud workspaces |

Provider-side model behavior is user-level (`~/.conductor/settings.toml`): `models.default`, `models.review`, `models.claude_code.default_effort_level`, `models.codex.*` (thinking level, personality, tool-approval policy), `tool_approvals_enabled`. Organizations can force settings via `~/.conductor/settings.managed.toml` (overrides user + repo), including managed `environmentVariables.local/cloud`.

### Per-repo setup / run / archive scripts

Configured in-app or committed as `<repo>/.conductor/settings.toml` ([Scripts reference](https://www.conductor.build/docs/reference/scripts)):

```toml
"$schema" = "https://conductor.build/schemas/settings.repo.schema.json"
[scripts]
setup   = "pnpm install"
run     = "pnpm dev --port $CONDUCTOR_PORT"      # legacy single run script
archive = "./script/workspace-archive.sh"
run_mode = "concurrent"                          # vs nonconcurrent (single fixed port/db)

[scripts.run.web]
command = "pnpm dev --port $CONDUCTOR_PORT"
available_in = ["local", "cloud"]
default = true
icon = "play"
```

Setup runs after the workspace's git checkout (use `$CONDUCTOR_ROOT_PATH` to reach root-checkout files, e.g. symlink `.env`); archive cleans up before archiving. Secrets belong in `.conductor/settings.local.toml` (not committed) ([Environment variables doc](https://www.conductor.build/docs/reference/environment-variables)). Untracked files that every workspace needs are copied via `.worktreeinclude` or `file_include_globs` fallback ([settings reference](https://www.conductor.build/docs/reference/settings/reference)).

### Environment-variable injection (wrapper mechanics)

Built-ins exposed to terminals, agents, and scripts ([reference](https://www.conductor.build/docs/reference/environment-variables)): `CONDUCTOR_WORKSPACE_PATH`, `CONDUCTOR_ROOT_PATH`, `CONDUCTOR_WORKSPACE_NAME`, `CONDUCTOR_DEFAULT_BRANCH`, `CONDUCTOR_PORT` (first of a **10-port range** per local workspace, so parallel workspaces don't collide), `CONDUCTOR_IS_LOCAL`; cloud adds `CONDUCTOR_BASE_DIR`, `CONDUCTOR_API_URL`, `CONDUCTOR_API_TOKEN`/`CONDUCTOR_API_KEY` (workspace-scoped API token), `CONDUCTOR_SESSION_ID`.

Custom vars for all agents/scripts in a repo:

```toml
[environment_variables]        # both environments
API_BASE_URL = "http://localhost:3000"
[environment_variables.local]  # local-only
CONDUCTOR_TARGET = "local"
[environment_variables.cloud]  # cloud-only
CONDUCTOR_TARGET = "cloud"
```

Docs' canonical trick for credential pass-through: setup script does `ln -s "$CONDUCTOR_ROOT_PATH/.env" .env` — i.e., the harness reads your existing `.env`/CLI logins; Conductor doesn't proxy model credentials itself (it launches your authenticated `claude`/`codex` binaries, optionally redirected via `*_executable_path` / `claude_provider`). Community threads confirm users inject e.g. `ANTHROPIC_BASE_URL` this way ([r/conductorbuild](https://www.reddit.com/r/conductorbuild/comments/1tlbo19/question_feature_request_environment_variables_or/)).

### Parallel Claude Code instances

⌘N spawns another workspace = another worktree + branch + terminal + diff; multiple agents can also share ONE workspace when they should touch the same branch ([Parallel agents concept](https://www.conductor.build/docs/concepts/parallel-agents)); dedicated guides exist for running multiple Claude Code / Codex / Cursor sessions in parallel. Port ranges (`CONDUCTOR_PORT..+9`) keep N parallel dev servers conflict-free ([Scripts](https://www.conductor.build/docs/reference/scripts)). Conductor Cloud moves the same model into sandboxed cloud computers (org-level Personal/Shared env vars, per [changelog](https://www.conductor.build/changelog)).

---

## C) Sculptor (Imbue)

Sources: [imbue-ai/sculptor README + docs/help/*](https://github.com/imbue-ai/sculptor) (MIT, actively developed — v0.46.0.dev commits Aug 2026; "experimental research preview"), plus the Show HN thread for launch-era architecture details.

### What it is

Desktop app (Mac Apple Silicon, Linux x64/ARM64) for running coding agents in parallel against isolated copies of your repo. Original pitch: "safely run Claude Code agents by putting them in separate docker containers" ([Show HN](https://news.ycombinator.com/item?id=45427697)); current docs describe **workspaces = git worktrees** under `~/.sculptor/workspaces/<id>/code/`, with containerization available via the experimental **Container Backend** ([Workspaces doc](https://github.com/imbue-ai/sculptor/blob/main/docs/help/workspaces.md); [Container Backend doc](https://github.com/imbue-ai/sculptor/blob/main/docs/help/experimental/container_backend.md)).

### Which harnesses attach

- **Claude Code** — deep-integrated "integrated harness": run as a streaming-JSON process with the control protocol enabled; tool permissions **auto-approved**; Sculptor swaps in its own ask-user/plan tools, injects system-prompt additions, loads bundled plugins (`/help`, `/plan`, `/review`), registers a compaction hook, and sets model/fast-mode/effort at launch ([Integrated Harnesses](https://github.com/imbue-ai/sculptor/blob/main/docs/help/integrated_harnesses.md)).
- **Pi harness** ([pi.dev](https://pi.dev/)) — second integrated harness, driven in RPC mode with Sculptor-curated, version-pinned extensions; **provider logins inside Sculptor populate pi's model picker**, and mid-session model switching goes through pi directly (same doc).
- **Any terminal-based agent** can be run/managed generically (README: "Sculptor also allows you to run and manage *any* terminal-based agents"); samples ship a terminal-agent wrapper for Claude Code ([samples/terminal_agents/claude-code](https://github.com/imbue-ai/sculptor/tree/main/samples/terminal_agents/claude-code)).

### Executor config (Dependencies & Harness settings)

Settings → **Dependencies**: the Claude CLI can be **managed by Sculptor or pointed at a custom binary**; git and GitHub CLI likewise ([Settings doc](https://github.com/imbue-ai/sculptor/blob/main/docs/help/settings.md)). Under **Harnesses**: Claude defaults (model, fast mode, effort level); Pi (managed vs custom binary, **the API-key environment variables passed to it**, and LLM provider connections — i.e., provider credentials for Pi are configured in-app and handed to the harness).

### Env / credential pass-through

- Global env file `~/.sculptor/.env` and per-repo `.sculptor/.env` (add to `.gitignore`) are loaded into agent environments; a toggle controls whether they **override pre-existing shell variables** ([Workspaces → Per-repo setup](https://github.com/imbue-ai/sculptor/blob/main/docs/help/workspaces.md); [Settings doc](https://github.com/imbue-ai/sculptor/blob/main/docs/help/settings.md)).
- Model credentials normally come from your logged-in Claude CLI (host keychain on macOS). Founder guidance for custom endpoints (e.g., `ANTHROPIC_BASE_URL`): put them in `~/.sculptor/.env`, which "will be injected into the environment for the agent" — offered experimentally (~20% odds at the time), verifiable via the Terminal tab ([HN comment by thejash](https://news.ycombinator.com/item?id=45432922)).
- **Container credential caveat:** macOS stores Claude Code creds in the system keychain, unreachable from inside a container — you must re-run `claude` auth inside the container (documented workaround; credential forwarding improvements planned) ([Container Backend doc](https://github.com/imbue-ai/sculptor/blob/main/docs/help/experimental/container_backend.md)). Sculptor also ships "Claude settings sync" to carry settings like MCP servers into sessions ([LinkedIn demo](https://www.linkedin.com/posts/imbue-ai_our-latest-sculptor-release-enables-claude-activity-7404928076511510529-R7iM)).

### Container image / environment configuration

Two layers:

1. **Per-agent sandboxes (original model):** each Claude session runs in its own Docker container; custom images/devcontainer-style configs supported so you choose shared vs separate services ("We support custom docker containers, so you should be able to configure it however you want… we support the devcontainer spec") ([HN answers by thejash & penlu](https://news.ycombinator.com/item?id=45427697)). Containers sync bidirectionally with your local worktree ("Pairing Mode") so uncommitted changes stream both ways ([HN, Imbue team](https://news.ycombinator.com/item?id=45428185)).
2. **Backend-in-container (experimental, current):** Settings → Experimental → **Custom Backend Command** pointing at a launcher (e.g. `run-backend.py`); Sculptor spawns your command, which prints a backend URL the UI connects to. The shipped recipe (`container/recipes/docker/`) builds an image with git + Claude CLI + runtime deps, auto-downloads backend binaries (~100 MB, cached), forwards signals; ports, volumes, and binary sources are configurable via the recipe's env options. Same mechanism covers remote SSH/VM backends. Scratch repo provided at `/workspace` in the image ([Container Backend doc](https://github.com/imbue-ai/sculptor/blob/main/docs/help/experimental/container_backend.md)).

### Workspaces

Default **worktree** mode (branch pattern `<user>/<slug>`, configurable target branch, branch-deletion policy); experimental **Clone** mode (full clone, explicit push/pull to bring changes back) and **In-place** mode; per-repo **setup command** (e.g. `npm install`) run at workspace creation ([Workspaces doc](https://github.com/imbue-ai/sculptor/blob/main/docs/help/workspaces.md)).

---

## Comparison — wrapper mechanics side by side

| | **Vibe Kanban** | **Conductor** | **Sculptor** |
|---|---|---|---|
| Platform / form | Web/desktop via `npx vibe-kanban`; self-hostable; **sunsetting → community-maintained OSS (Apache-2.0)** | macOS desktop (+ Conductor Cloud) | macOS (Apple Silicon) + Linux desktop; OSS (MIT) |
| Harnesses attachable | Claude Code, Codex, Copilot CLI, Gemini CLI, Amp, Cursor CLI, OpenCode, Droid, CCR, Qwen Code — broadest multi-harness menu | Claude Code, Codex, Cursor, OpenCode | Claude Code (deep integration), Pi harness (deep integration), any terminal agent generically |
| Executor/binary config | None documented — CLIs must be pre-installed on PATH & authenticated | Explicit TOML overrides: `claude_code_executable_path`, `codex_executable_path`; providers `claude_provider`/`codex_provider`/Bedrock/Vertex | Claude CLI "managed by Sculptor" or **custom binary**; same for Pi |
| Credential injection | **Per-profile env vars** (e.g. `ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN`) injected at spawn, overriding shell — per-provider profiles | Inherits your logged-in CLIs; repo `[environment_variables]` (+ `.local`/`.cloud` scopes) injected to agents/scripts; managed org vars; `.env` symlink pattern | `~/.sculptor/.env` + per-repo `.sculptor/.env` injected into agent env (override-shell toggle); Pi gets API-key env vars + provider connections from Settings; container needs in-container re-auth on macOS |
| MCP | Per-agent `mcpServers` JSON written into each agent's own global config (persists outside VK); plus VK exposes its own MCP server | MCP server lets clients create/manage cloud workspaces ([api/mcp](https://www.conductor.build/docs/api/mcp)); agents keep their native MCP config | Bundled plugins/slash-commands injected into Claude; "Claude settings sync" carries synced MCP servers into sessions |
| Isolation unit | Git worktree per workspace (dir configurable), branch `vk/*` | Git worktree per workspace under `~/conductor/workspaces/` | Worktree by default; Clone/In-place modes; agent sandboxing in Docker containers; whole backend can move into Docker/SSH/VM |
| Ports | Server: `PORT`/`HOST`/`MCP_HOST`/`MCP_PORT`/`VK_ALLOWED_ORIGINS` | `CONDUCTOR_PORT` + 9 reserved ports per workspace for parallel dev servers | Recipe-level env (ports/volumes) for containerized backend |
| Setup automation | Per-repo dev-server/setup/cleanup scripts; optional parallel setup | `[scripts]` setup/run(named)/archive + `run_mode`; `.worktreeinclude` file copy | Per-repo setup command; CI Babysitter auto-fixes PRs |

**Bottom line:** all three are thin orchestration shells over independently-installed, independently-authenticated CLIs. Vibe Kanban gives the widest harness choice and per-profile credential swapping via injected env vars; Conductor adds explicit binary-path/provider overrides and structured per-repo TOML with scoped env injection; Sculptor is the most opinionated — deepest integration (and container sandboxing) but effectively Claude Code + Pi as first-class harnesses, with generic terminal agents as the escape hatch.

---

## Source index

- Vibe Kanban: [docs home](https://vibekanban.com/docs/) · [llms.txt index](https://vibekanban.com/docs/llms.txt) · [Supported Coding Agents](https://vibekanban.com/docs/supported-coding-agents) · [Agent Profiles](https://vibekanban.com/docs/settings/agent-configurations) · [MCP Servers](https://vibekanban.com/docs/settings/mcp-servers) · [Projects & Repositories](https://vibekanban.com/docs/settings/projects-repositories) · [Creating Workspaces](https://vibekanban.com/docs/workspaces/creating-workspaces) · [Remote Access](https://vibekanban.com/docs/remote-access) · [GitHub README (env vars, sunset)](https://github.com/BloopAI/vibe-kanban) · [sunset coverage](https://nimbalyst.com/blog/vibe-kanban-after-bloop-whats-next/)
- Conductor: [docs intro](https://www.conductor.build/docs) · [Settings reference](https://www.conductor.build/docs/reference/settings/reference) · [Environment variables](https://www.conductor.build/docs/reference/environment-variables) · [Scripts](https://www.conductor.build/docs/reference/scripts) · [Parallel agents](https://www.conductor.build/docs/concepts/parallel-agents) · [Troubleshooting](https://www.conductor.build/docs/troubleshooting/issues) · [Changelog](https://www.conductor.build/changelog)
- Sculptor: [GitHub repo/README](https://github.com/imbue-ai/sculptor) · [Workspaces](https://github.com/imbue-ai/sculptor/blob/main/docs/help/workspaces.md) · [Integrated Harnesses](https://github.com/imbue-ai/sculptor/blob/main/docs/help/integrated_harnesses.md) · [Settings](https://github.com/imbue-ai/sculptor/blob/main/docs/help/settings.md) · [Getting Started](https://github.com/imbue-ai/sculptor/blob/main/docs/help/getting_started.md) · [Container Backend](https://github.com/imbue-ai/sculptor/blob/main/docs/help/experimental/container_backend.md) · [Show HN thread](https://news.ycombinator.com/item?id=45427697) · [announcement post](https://imbue.com/blog/sculptor-announce)

*Note: Vibe Kanban is post-sunset; its hosted docs reflect the final maintained state and may not track future community forks.*
