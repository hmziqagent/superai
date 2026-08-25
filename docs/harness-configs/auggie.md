# Auggie CLI (Augment Code) — Complete Configuration Reference

> Compiled 2026-08-25 from primary sources: docs.augmentcode.com (Auggie CLI section), github.com/augmentcode/augment-agent, raw action.yml, augmentcode.com. Every claim carries an inline citation. Sections follow the requested order; a few adjacent features (hooks, indexing, diagnostics) are folded into the nearest section.

---

## 0. Install & runtime basics

- **Install:** `npm install -g @augmentcode/auggie`. Requires **Node.js 20+**; platforms macOS, Linux, Windows WSL; shells zsh/bash/fish ([Install Auggie CLI](https://docs.augmentcode.com/cli/setup-auggie/install-auggie-cli)).
- **Auto-update:** enabled by default **in interactive mode only**; completely disabled in print/automation mode so version pinning works in CI ([Automatic Updates](https://docs.augmentcode.com/cli/autoupgrade)). Disable persistently with `"autoUpdate": false` in `~/.augment/settings.json`, or per-process with `AUGMENT_DISABLE_AUTO_UPDATE=1` ([Automatic Updates](https://docs.augmentcode.com/cli/autoupgrade)). Manual update: `auggie upgrade [--skip-confirmation]` ([CLI reference](https://docs.augmentcode.com/cli/reference)).
- Interactive mode needs an ANSI-capable terminal (Ghostty, iTerm2, Windows Terminal, Alacritty, Kitty recommended) ([Install](https://docs.augmentcode.com/cli/setup-auggie/install-auggie-cli)).

---

## 1. Config: auth flow, `.augment` directory, commands, settings files

### 1.1 Login / authentication flow

| Command | Effect |
|---|---|
| `auggie login` | Browser-driven login; stores the session token **locally** (`~/.augment/session.json`) |
| `auggie logout` | Removes the local token only |
| `auggie token print` | Prints current session JSON: `{"accessToken": "...", "tenantURL": "https://<org>.api.augmentcode.com"}` |
| `auggie token revoke` | Revokes **all** tokens for that user server-side |
| `auggie --augment-session-json '<json-or-path>'` | Supply session JSON inline or point at a session file |
| `auggie --github-api-token <path>` | Override the GitHub integration token (path to file) |

Source: [Login and authentication](https://docs.augmentcode.com/cli/setup-auggie/authentication), [CLI reference §Authentication](https://docs.augmentcode.com/cli/reference#authentication), session-file path per [augment-agent README](https://github.com/augmentcode/augment-agent#readme).

Key properties:
- Tokens are **OAuth tokens tied to the individual user**, not to team/enterprise accounts — each user has a unique token. Treat as sensitive credentials ([authentication docs](https://docs.augmentcode.com/cli/setup-auggie/authentication), [augment-agent security warning](https://github.com/augmentcode/augment-agent)).
- Automation supplies auth **per invocation**: `AUGMENT_SESSION_AUTH='<session-json>' auggie --print "..."` or the `--augment-session-json` flag ([authentication docs](https://docs.augmentcode.com/cli/setup-auggie/authentication)).
- **Service Accounts** (team/enterprise plans, managed by tenant admin at [app.augmentcode.com/settings/service-accounts](https://app.augmentcode.com/settings/service-accounts)): non-human identities with non-expiring API tokens; you download a ready-made `session.json` (`{"accessToken", "tenantURL", "scopes": ["read","write"]}`) and drop it into `~/.augment/session.json` or pass it per-run. Recommended pattern: one service account per automation task for per-task token lifecycle and credit metering ([Service Accounts](https://docs.augmentcode.com/cli/automation/service-accounts)).

### 1.2 The `.augment` directory — full layout

Auggie has **no single config file**; state is spread over `~/.augment/`, `<workspace>/.augment/`, and system paths:

| Path | Purpose | Source |
|---|---|---|
| `~/.augment/session.json` | Auth session (written by `auggie login`; replace with service-account JSON) | [augment-agent README](https://github.com/augmentcode/augment-agent), [Service Accounts](https://docs.augmentcode.com/cli/automation/service-accounts) |
| `~/.augment/settings.json` | User-global settings (JSONC supported — comments + trailing commas OK) | [Configuration Wizard](https://docs.augmentcode.com/cli/config) |
| `<workspace>/.augment/settings.json` | Shared project settings; commit to VCS | [Configuration Wizard](https://docs.augmentcode.com/cli/config) |
| `<workspace>/.augment/settings.local.json` | Personal per-project overrides; auto-added to `.gitignore`; never commit | [Configuration Wizard](https://docs.augmentcode.com/cli/config) |
| `/etc/augment/settings.json` (macOS/Linux), `C:\ProgramData\augment\settings.json` (Windows) | Managed/org settings — read-only, cannot be overridden by users | [Configuration Wizard](https://docs.augmentcode.com/cli/config) |
| `~/.augment/commands/<name>.md`, `./.augment/commands/<name>.md` | Custom slash commands | [Custom Slash Commands](https://docs.augmentcode.com/cli/custom-commands) |
| `~/.claude/commands/`, `./.claude/commands/`, `~/.agents/commands/`, `./.agents/commands/` | Additional command locations (Claude Code / Agents compat) | [Custom Slash Commands](https://docs.augmentcode.com/cli/custom-commands) |
| `~/.augment/rules/*.md`, `<workspace>/.augment/rules/*.md` | Rules files (recursive `.md` search) | [Rules & Guidelines](https://docs.augmentcode.com/cli/rules) |
| `<workspace_root>/.augment-guidelines` | Legacy single-file workspace guidelines (still honored) | [Rules & Guidelines](https://docs.augmentcode.com/cli/rules) |
| `.augment/skills/`, `.claude/skills/` (home + workspace) | Agent Skills (agentskills.io spec), loaded automatically | [CLI reference](https://docs.augmentcode.com/cli/reference#configuration), [Agent Skills](https://docs.augmentcode.com/cli/skills) |
| `<workspace>/.augmentignore` | Indexing ignore patterns (gitignore syntax; `!` re-includes `.gitignored` paths) | [Index your workspace](https://docs.augmentcode.com/cli/setup-auggie/workspace-indexing) |
| `$TMPDIR/augment-log.txt` (default) | Log file; override with `--log-file <path>` (`-` = stderr console) | [Logs](https://docs.augmentcode.com/troubleshooting/logs), [CLI reference §Diagnostics](https://docs.augmentcode.com/cli/reference#diagnostics) |

Cache/index location can be moved with `auggie --augment-cache-dir /path/to/cache` ([CLI reference](https://docs.augmentcode.com/cli/reference#configuration)).

### 1.3 Settings hierarchy & merge semantics

Four tiers, high→low precedence ([Configuration Wizard](https://docs.augmentcode.com/cli/config), [Hooks](https://docs.augmentcode.com/cli/hooks)):
1. Managed `/etc/augment/settings.json` (immutable)
2. `<workspace>/.augment/settings.local.json`
3. `<workspace>/.augment/settings.json`
4. `~/.augment/settings.json`

Merge rules (documented verbatim in [config docs](https://docs.augmentcode.com/cli/config)):
- Simple values: higher-precedence tier wins.
- **MCP servers and plugin entries are replaced whole**, never deep-merged — same-named server in two tiers ⇒ entire config from the higher tier wins.
- Tool-permission rules from all tiers are **concatenated**; higher-precedence rules evaluated first (first-match).
- `removedTools` and `indexingAllowDirs` lists are **unioned + deduped** (additive only — no way to subtract a higher tier's entry).
- `verbose` and `vimMode` are read from **user settings only**.
- Settings-modifying subcommands accept `--project` / `--local` to pick the target file (e.g., `auggie mcp add --local`).

### 1.4 Documented `settings.json` schema (keys that appear in official docs/examples)

```jsonc
{
  // /config wizard-managed:
  "shell": "zsh",                        // bash | zsh | fish | powershell
  "startupScript": "source ~/.augment/startup.sh",
  "enableChatInputCompletions": true,
  "autoUpdate": true,
  "autoUpdateMarketplaces": true,
  "notificationMode": "desktop_notification",  // off | bell | desktop_notification
  "theme": "default-dark",               // default-dark (truecolor) | ansi
  "verbose": true,                       // user-settings only
  "vimMode": false,                      // user-settings only
  // integrations:
  "mcpServers": { "<name>": { /* stdio|sse|http config */ } },
  "enableToolSearch": false,             // hide MCP tools behind find-tool/execute-tool
  "recommendedMarketplaces": [], "dismissedMarketplaces": [],
  // control:
  "toolPermissions": [ /* see §5.3 */ ],
  "hooks": { /* see §5.4 */ },
  "removedTools": [],                    // written by `auggie tools remove`
  "indexingAllowDirs": []
}
```
Sources: [Configuration Wizard](https://docs.augmentcode.com/cli/config) (wizard keys + example file), [Integrations and MCP](https://docs.augmentcode.com/cli/integrations) (`mcpServers`, `enableToolSearch`), [Plugins](https://docs.augmentcode.com/cli/plugins) (marketplace keys), [Permissions](https://docs.augmentcode.com/cli/permissions), [Hooks](https://docs.augmentcode.com/cli/hooks), merge accordion in [config](https://docs.augmentcode.com/cli/config) (`removedTools`, `indexingAllowDirs`). *Not every key has a published exhaustive schema — the above is everything the docs expose.*

The interactive editor for these is the `/config` slash command in the TUI ([Configuration Wizard](https://docs.augmentcode.com/cli/config)); manual edits require restarting Auggie.

### 1.5 Custom commands (`.augment/commands`)

Markdown prompt files invoked as `/<name>` in the TUI or `auggie command <name>` from any shell; `auggie command list` enumerates them ([Custom Slash Commands](https://docs.augmentcode.com/cli/custom-commands), [CLI reference §Custom Commands](https://docs.augmentcode.com/cli/reference#custom-commands)).
- Locations, in precedence order: `~/.augment/commands/` → `./.augment/commands/` → `~/.claude/commands/` → `./.claude/commands/` → `~/.agents/commands/` → `./.agents/commands/`.
- Subdirectories become namespaces: `.augment/commands/frontend/component.md` ⇒ `/frontend:component`.
- Frontmatter: `description`, `argument-hint`, `model` (per-command model override, e.g. `gpt-4o`).
- Arguments pass through positionally: `/fix-issue 123`.

---

## 2. Environment variables

| Variable | Purpose | Source |
|---|---|---|
| `AUGMENT_SESSION_AUTH` | Full session JSON for headless auth (preferred CI mechanism) | [Authentication](https://docs.augmentcode.com/cli/setup-auggie/authentication), [reference](https://docs.augmentcode.com/cli/reference#environment-variables) |
| `GITHUB_API_TOKEN` | GitHub integration token (overrides the logged-in user's GitHub config) | [Automation overview](https://docs.augmentcode.com/cli/automation/overview), [reference](https://docs.augmentcode.com/cli/reference#environment-variables) |
| `AUGMENT_DISABLE_AUTO_UPDATE=1` | Kill auto-update (recommended for CI/scripts) | [Automatic Updates](https://docs.augmentcode.com/cli/autoupgrade) |
| `AUGMENT_AGENT` | **Set to `1` by Auggie inside shells it spawns** (`launch-process`/`terminal` tool) — lets scripts detect they're running under the agent | [reference §Shell Environment](https://docs.augmentcode.com/cli/reference#environment-variables) |

```sh
if [ -n "$AUGMENT_AGENT" ]; then
  echo "Running inside Auggie"
fi
```

That is the complete officially documented set ([CLI reference](https://docs.augmentcode.com/cli/reference#environment-variables)). Undocumented internals exist (the GitHub Action passes `INPUT_*` variables to its runner, [action.yml](https://github.com/augmentcode/augment-agent/blob/main/action.yml)), but they're implementation details, not configuration surface.

Related non-env knobs that overlap with env-var duties: `--github-api-token <path>` (token from file instead of `GITHUB_API_TOKEN`) and `--retry-timeout <sec>` (rate-limit retry window) ([CLI reference](https://docs.augmentcode.com/cli/reference)).

---

## 3. Models & the Context Engine backend — honest limits

### 3.1 What you can configure

- **Selection:** `auggie --model "name"` (accepts long or short names), `/model` slash command in TUI, or omit it to use your last selection / the org-set default. Discover names with `auggie models list`, `auggie models list --json`, `auggie models list --full-info` (display names, cost tiers, effort levels, account default) ([Available Models](https://docs.augmentcode.com/models/available-models), [CLI reference §Models](https://docs.augmentcode.com/cli/reference#models)). Per-command overrides via command frontmatter `model:` ([custom-commands](https://docs.augmentcode.com/cli/custom-commands)); the GitHub Action forwards its `model` input to `--model` ([action.yml](https://github.com/augmentcode/augment-agent/blob/main/action.yml)).
- **Catalog (live docs, 2026):** Anthropic Claude family (Fable 5, Opus 5, Opus 4.8–4.5, Sonnet 5, Sonnet 4.6/4.5, Haiku 4.5), Google Gemini 3.1 Pro, Zhipu GLM 5.2 (hosted on Fireworks), OpenAI GPT‑5.6 Sol/Terra/Luna, GPT‑5.5/5.4/5.2/5.1, Moonshot Kimi K3/K2.6 (Fireworks/Baseten), plus two **Prism** auto-routers — `Prism (Claude + Gemini)` routes across Opus 5/Sonnet 5/Gemini Flash; `Prism (GPT)` routes across GPT‑5.6 tiers ([Available Models](https://docs.augmentcode.com/models/available-models)). (The catalog rotates; treat the list as a snapshot.)
- Every model gets identical Augment features because retrieval/tool-use is mediated by Augment's **Context Engine** (their proprietary codebase-indexing service, marketed around ~200k-token effective context), not by the raw LLM ([Available Models](https://docs.augmentcode.com/models/available-models): "All listed models support … Deep code understanding with Augment's Context Engine").
- Billing is credit/token-based; inspect with `auggie account status` and add `--show-credits` to print runs for usage summaries ([Token-Based Pricing](https://docs.augmentcode.com/models/token-based-pricing), [CLI reference](https://docs.augmentcode.com/cli/reference)).

### 3.2 Can you point the backend elsewhere? **No — and here's exactly why**

- There is **no bring-your-own-endpoint, no OpenAI-compatible base URL, no BYO API key** anywhere in the documented surface. The only endpoint string a user ever handles is `tenantURL` inside the session JSON — and that is **your org's Augment API host**, not a swappable provider URL ([augment-agent README example session](https://github.com/augmentcode/augment-agent): `"tenantURL": "https://your-tenant.api.augmentcode.com"`).
- The Auggie SDK's `apiKey`/`apiUrl` init params are the same pair — an Augment access token + Augment tenant URL — not generic provider credentials ([Auggie SDK](https://docs.augmentcode.com/cli/sdk): "The session JSON includes both your access token and tenant URL"). Even the TypeScript SDK's Vercel AI SDK provider routes through Augment's API rather than letting you substitute another backend ([SDK overview](https://docs.augmentcode.com/cli/sdk)).
- Model choice is limited to **Augment's hosted catalog**; third-party/community requests for BYO-API (e.g., pointing at your own Anthropic/OpenAI keys) remain a standing user ask with no shipped support ([community thread](https://www.reddit.com/r/AugmentCodeAI/comments/1njuuut/can_augment_code_ever_let_us_bring_our_own_api/)).
- Practical consequence: the agent's intelligence is a **closed stack** — your token → your tenant → Augment's Context Engine → one of their hosted models. You configure *which* model, *who* authenticates (user vs service account), and *what tools* it may touch; you do not configure *where inference happens*.
- Adjacent enterprise lever: non-interactive (`--print`) capability itself "may be disabled if it is not included in your agreement (enterprise)" ([automation overview](https://docs.augmentcode.com/cli/automation/overview)) — i.e., even the automation surface is contract-gated upstream.

---

## 4. Multi-instance wrappers: accounts, workspaces, headless mode, CI, wrapper script

### 4.1 Running multiple accounts / workspaces

- **Per-process identity:** auth resolves in the order local flag → environment → stored session. So concurrent instances each get their own account simply by scoping `AUGMENT_SESSION_AUTH` (or `--augment-session-json <file>`) per process — e.g., one token file per role: `~/.augment-tokens/bot-ci.json`, `~/.augment-tokens/alice.json` ([authentication](https://docs.augmentcode.com/cli/setup-auggie/authentication)). Tokens are strictly per-user, so this is the sanctioned multi-account mechanism ([authentication](https://docs.augmentcode.com/cli/setup-auggie/authentication)).
- **Isolating caches:** `--augment-cache-dir /path/to/cache` gives each instance its own index/cache directory — useful when several instances index different trees in parallel ([CLI reference](https://docs.augmentcode.com/cli/reference#configuration)).
- **Workspaces:** `--workspace-root /path` sets the primary tree; `--add-workspace /other/path` indexes additional repos alongside it (repeat for several); non-git directories fall back to CWD ([Workspace context](https://docs.augmentcode.com/cli/setup-auggie/workspace-context), [reference](https://docs.augmentcode.com/cli/reference)). Indexing behavior flags: `--allow-indexing` (skip confirmation — required for headless), `--wait-for-indexing` (block until indexed before retrieval) ([reference](https://docs.augmentcode.com/cli/reference)).
- **Session history is per-workspace:** `auggie session list` shows the current workspace's sessions, `--all` spans workspaces; `--continue`/`--resume <id-prefix>` resume, `--dont-save-session` keeps ephemeral runs out of history ([reference §Sessions](https://docs.augmentcode.com/cli/reference#sessions)).
- **MCP-server mode fan-out:** `auggie --mcp` exposes the codebase-retrieval tool to other agents; `--mcp-auto-workspace` lets clients address multiple projects dynamically via a `directory_path` param, optionally pre-indexing a primary with `-w` ([reference §MCP Server Mode](https://docs.augmentcode.com/cli/reference#mcp-server-mode)).
- **Org-scale identities:** prefer one Service Account per automation with its own token ([service accounts](https://docs.augmentcode.com/cli/automation/service-accounts)).

### 4.2 Headless `--print` mode (full flag set)

Non-interactive automation surface ([overview](https://docs.augmentcode.com/cli/overview#using-auggie-in-your-automations), [reference](https://docs.augmentcode.com/cli/reference)):

| Flag | Behavior |
|---|---|
| `--print` / `-p` | Run one instruction, stream, exit |
| `--quiet` | Only final assistant message (clean structured output) |
| `--compact` | Compact streaming output |
| `--output-format json` | Structured JSON response for machines |
| `--show-credits` | Append credit-usage summary to output/log |
| `--max-turns N` | Hard cap on agentic turns (cost/runaway bound) |
| `--queue "step"` (repeatable) | Sequential instruction queue after the initial prompt |
| `--ask` / `-a` | Ask mode — retrieval/non-editing tools only |
| `--instruction "…"` / `--instruction-file path` | Prompt via flag/file instead of argv |
| `--image img.png` | Attach image(s) to the initial prompt |
| `--enhance-prompt` | Run prompt enhancer before sending |
| stdin pipe / redirect | `cat build.log \| auggie --print "…"`, `auggie --print "…" < file` |

Notes: auto-update is force-disabled in print mode (pin versions safely) ([autoupgrade](https://docs.augmentcode.com/cli/autoupgrade)); enterprise agreements can gate the feature off entirely ([automation overview](https://docs.augmentcode.com/cli/automation/overview)).

### 4.3 GitHub Actions PR-review setup (official path)

Official wrapper action: [`augmentcode/augment-agent`](https://github.com/augmentcode/augment-agent) (composite action: setup-node 22 → `npm i -g @augmentcode/auggie` → runs instruction through Auggie; pinned SHA-tagged releases like `@v0`) ([action.yml](https://github.com/augmentcode/augment-agent/blob/main/action.yml)).

Setup steps ([README](https://github.com/augmentcode/augment-agent#readme)):
1. Locally run `auggie token print` (or copy `~/.augment/session.json`) → session JSON `{accessToken, tenantURL}`.
2. Store it as repo secret **`AUGMENT_SESSION_AUTH`**. Alternative auth inputs: `augment_api_token` + `augment_api_url`. Never commit tokens; revoke leaks with `auggie token revoke`.
3. Add workflow. Ready-made review flows: [`augmentcode/review-pr`](https://github.com/augmentcode/review-pr), [`augmentcode/describe-pr`](https://github.com/augmentcode/describe-pr), or generate with Auggie's `/github-workflow` wizard ([automation overview](https://docs.augmentcode.com/cli/automation/overview)).

Action inputs ([README table](https://github.com/augmentcode/augment-agent#inputs), [action.yml](https://github.com/augmentcode/augment-agent/blob/main/action.yml)): `augment_session_auth`, `augment_api_token`, `augment_api_url`, `github_token` (needs `repo` + `user:email` scopes), `instruction` | `instruction_file` | `template_directory`(+`template_name`, `pull_number`, `repo_name`, `custom_context`; nunjuck-style templates), plus pass-through `model` (→`--model`), `rules` (JSON array → repeated `--rules`), `mcp_configs` (JSON array → repeated `--mcp-config`).

Canonical PR-review workflow, straight from [`example-workflows/code-review.yml`](https://github.com/augmentcode/augment-agent/blob/main/example-workflows/code-review.yml):

```yaml
name: Augment Agent - Code Review
on:
  pull_request:
    types: [opened]

jobs:
  code-review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - name: Create instruction file
        env:
          PR_NUMBER: ${{ github.event.pull_request.number }}
          REPOSITORY: ${{ github.repository }}
          BASE_BRANCH: ${{ github.event.pull_request.base.ref }}
          HEAD_BRANCH: ${{ github.event.pull_request.head.ref }}
        run: |
          cat > /tmp/review-instruction.txt << EOF
          Perform a comprehensive code review of the following pull request:

          **Pull Request Information:**
          - PR Number: ${PR_NUMBER}
          - Repository: ${REPOSITORY}
          - Base Branch: ${BASE_BRANCH}
          - Head Branch: ${HEAD_BRANCH}

          **Review Focus:**
          Analyze the modified files and provide detailed feedback on:
          - Code quality and adherence to best practices
          - Potential bugs, errors, or security vulnerabilities
          - Performance implications of the changes
          - Suggestions for improvement or optimization
          - Any missing error handling or edge cases
          - Code maintainability and readability

          Please provide specific, actionable feedback with file and line references where applicable.
          Focus on the actual code changes and their impact on the codebase.

          Please post your review as a review comment on the PR. Do not approve or request changes.
          EOF
      - name: Code Review
        uses: augmentcode/augment-agent@v0
        with:
          augment_session_auth: ${{ secrets.AUGMENT_SESSION_AUTH }}
          github_token: ${{ secrets.GITHUB_TOKEN }}
          instruction_file: /tmp/review-instruction.txt
```

For hardening in CI, combine with committed repo-level permissions (§5.3) — e.g., read-only or allowlisted-shell policies apply to the Action's Auggie run too, since the CLI loads `.augment/settings.json` from the checked-out repo ([permissions](https://docs.augmentcode.com/cli/permissions)).

### 4.4 Wrapper-script example (multi-account, headless)

Illustrative glue using only documented flags/env vars:

```bash
#!/usr/bin/env bash
# aug-run.sh — run Auggie headless under a named account, JSON out.
# Usage: AUG_ACCOUNT=ci-review aug-run.sh "Review the staged diff"
set -euo pipefail

ACCOUNT="${AUG_ACCOUNT:-default}"
TOKEN_FILE="$HOME/.augment-tokens/${ACCOUNT}.json"   # one session.json copy per account

[ -f "$TOKEN_FILE" ] || { echo "no token file: $TOKEN_FILE" >&2; exit 2; }

exec env \
  AUGMENT_SESSION_AUTH="$(cat "$TOKEN_FILE")" \
  AUGMENT_DISABLE_AUTO_UPDATE=1 \
  auggie --print \
         --output-format json \
         --max-turns 12 \
         --show-credits \
         --retry-timeout 30 \
         --allow-indexing \
         --augment-cache-dir "$HOME/.cache/auggie-${ACCOUNT}" \
         "$@"
```

Every flag shown is documented: `--print/--output-format/--max-turns/--show-credits/--retry-timeout/--allow-indexing/--augment-cache-dir` ([CLI reference](https://docs.augmentcode.com/cli/reference)), env vars ([auth](https://docs.augmentcode.com/cli/setup-auggie/authentication), [autoupgrade](https://docs.augmentcode.com/cli/autoupgrade)). Pattern echoes Augment's own guidance (pipe-friendly subprocess, per-task service accounts, `--show-credits` in automation logs) ([automation overview](https://docs.augmentcode.com/cli/automation/overview), [service accounts](https://docs.augmentcode.com/cli/automation/service-accounts)).

---

## 5. Rules files, MCP support, permissions (plus hooks)

### 5.1 Rules / guidelines files

Load order (first found wins for the "primary" guideline slot; folders are additive) ([Rules & Guidelines](https://docs.augmentcode.com/cli/rules)):
1. `--rules /path/to/custom.md` (appended to whatever else loads)
2. `CLAUDE.md` (workspace root)
3. `AGENTS.md` (workspace root)
4. `<workspace_root>/.augment-guidelines`
5. `<workspace_root>/.augment/rules/**/*.md` (recursive)
6. `~/.augment/rules/**/*.md` (recursive, user-wide)

Semantics:
- Workspace-rule frontmatter: `type: always_apply` (default; injected into every prompt) or `type: agent_requested` (attached when the agent judges it relevant via `description`). **`manual` is skipped in the CLI** (IDE-only @-mention feature). User rules (`~/.augment/rules/`) are forced `always_apply` regardless of frontmatter ([rules](https://docs.augmentcode.com/cli/rules)).
- **Hierarchical rules:** `AGENTS.md`/`CLAUDE.md` in subdirectories are discovered by walking up from the touched file to workspace root (only those two filenames; `.augment/rules/` is root-only). Example: editing `src/frontend/App.tsx` loads `src/frontend/AGENTS.md` + `src/AGENTS.md` + root, but not `src/backend/`'s ([rules](https://docs.augmentcode.com/cli/rules)).
- Inspect what's active: `auggie rules list` ([reference](https://docs.augmentcode.com/cli/reference#rules)).
- Same rule engine as the VS Code/JetBrains extensions ([rules](https://docs.augmentcode.com/cli/rules)); richer IDE-side docs at [guidelines](https://docs.augmentcode.com/setup-augment/guidelines).
- Distinct sibling feature: **Skills** (`.augment/skills/<name>/SKILL.md`, agentskills.io spec) for domain knowledge, also invocable as slash commands ([skills](https://docs.augmentcode.com/cli/skills)).

### 5.2 MCP support

**As client** ([Integrations and MCP](https://docs.augmentcode.com/cli/integrations)):
- Persist in `settings.json` → `mcpServers`: transports `stdio` (`command`/`args`/`env`), `http` and `sse` (`url`, `headers`). `${workspaceFolder}` expands in `command`, `args`, `url` when running inside a workspace.
- CLI management (writes to settings; `--project`/`--local` select the tier): `auggie mcp add <name> [--command|--args|-e|-t stdio|sse|http|-u|-h|-r]`, `auggie mcp add-json <name> '<json>'`, `auggie mcp list [--json]`, `auggie mcp remove <name>`.
- Ad-hoc per-run: `--mcp-config /path/mcp.json` or `--mcp-config '{json}'` — applied **last**, overriding settings.
- Context-window guard: `--enable-tool-search` or `"enableToolSearch": true` collapses all MCP tools behind `find-tool`/`execute-tool` meta-tools.
- Verify with `/mcp` in the TUI.
- Native integrations (GitHub, Linear, Notion, …) are configured once in the VS Code/JetBrains extension and become available to Auggie automatically ([integrations](https://docs.augmentcode.com/cli/integrations)).

**As server:** `auggie --mcp` exposes `codebase-retrieval` to external tools (Claude Code, Cursor, …); `-w <dir>` pins a workspace; `--mcp-auto-workspace` adds on-demand indexing of client-requested directories (v0.14.0+) ([reference §MCP Server Mode](https://docs.augmentcode.com/cli/reference#mcp-server-mode)).

MCP tool naming for permissions/hooks: `{tool-name}_{server-name}`, truncated at 64 chars; matcher special-cases `mcp:*` and `mcp:<regex>` ([permissions](https://docs.augmentcode.com/cli/permissions), [hooks](https://docs.augmentcode.com/cli/hooks)).

### 5.3 Tool permissions (`toolPermissions`)

Enforced by **Auggie CLI and Cosmos cloud agents; NOT enforced in the IDE extension**. Read from `~/.augment/settings.json` and repo-committed `.augment/settings.json` at startup — commit a repo policy (e.g., block `git merge`) to bind every agent running there ([permissions](https://docs.augmentcode.com/cli/permissions)).

Rule shape:
```jsonc
{
  "toolPermissions": [
    {
      "toolName": "terminal",              // current names: terminal | read | edit | write | web-search | web-fetch
      "shellInputRegex": "\\bgit\\s+merge(\\s|$)",  // optional; regex against shell input (terminal only)
      "eventType": "tool-call",            // tool-call (default, pre-exec) | tool-response (post-exec)
      "permission": { "type": "deny" }     // MUST be object: allow | deny | webhook-policy | script-policy
    }
  ]
}
```
Gotchas the docs call out explicitly: a **bare-string** `"permission": "deny"` is malformed → silently dropped (now with startup warning) ([permissions Warning](https://docs.augmentcode.com/cli/permissions)); legacy tool names `launch-process`/`view`/`str-replace-editor`/`save-file` alias to `terminal`/`read`/`edit`/`write` (same source).

Evaluation: within one policy, **top-down first match wins**; unmatched tools follow implicit runtime behavior. Across policies (settings tiers × `--permission` flag), **most-restrictive wins**: `deny > webhook-policy > script-policy > allow` — so `--permission launch-process:allow` on the command line can never override a settings-file deny ([permissions §Precedence](https://docs.augmentcode.com/cli/permissions)).

External decision points:
- `webhook-policy`: POSTs `{tool-name, event-type, details, timestamp}` (details: `cwd`+`command` for terminal, `path` for read/write, `path`+`command` for edit, `url` for web-fetch) to `webhookUrl`; expects `{"allow": bool, "output": "optional msg"}`.
- `script-policy`: same JSON on stdin to local script; exit 0 = allow, non-zero = deny; stdout/stderr surfaced to agent.

Ready-made postures documented: read-only mode (review), dev mode (deny `rm|sudo|chmod`), CI/CD mode (allowlist `npm test|lint|jest`, deny rest) ([permissions §Common Configurations](https://docs.augmentcode.com/cli/permissions)). Session-scope complement: `auggie --remove-tool <name>` (repeatable, wins over settings) and persistent `auggie tools remove/add/list/schemas` writing `removedTools` ([reference §Tools](https://docs.augmentcode.com/cli/reference#tools)). Shell/runtime knobs: `--shell zsh`, `--startup-script "source .venv/bin/activate"`, `--startup-script-file` ([reference](https://docs.augmentcode.com/cli/reference)).

### 5.4 Hooks (lifecycle interception, settings-driven)

Configured under `"hooks"` in any settings tier (same hierarchy/precedence as §1.3) ([hooks](https://docs.augmentcode.com/cli/hooks)):
- Events: `PreToolUse` (can **block** via `permissionDecision: "deny"` — the only blocking pre-hook; input modification `updatedInput` not yet implemented), `PostToolUse` (observe only, sees `tool_output`/`tool_error`), `Stop` (can block agent completion via `decision:"block"`; sees `agent_stop_cause`), `SessionStart` (stdout injected as agent context), `SessionEnd` (logged only).
- Matchers: regex on tool name, defaults `.*`; special forms `mcp:*`, `mcp:.*_my-server$`; omitted for session events.
- Handlers: `{type:"command", command:"/path/script.sh", timeout:5000, metadata:{}}`; extensions `.sh` (shebang-respected on Unix), `.ps1`/`.bat`/`.cmd` (Windows dispatch); JSON event on **stdin**; sequential execution; default timeout 60s; synchronous — the agent waits.
- Docs carry an explicit "USE AT YOUR OWN RISK" warning and hardening checklist (absolute paths, quote vars, skip `.env`/`.git`, timeouts) ([hooks §Security](https://docs.augmentcode.com/cli/hooks)); runnable recipes in [hooks-examples](https://docs.augmentcode.com/cli/hooks-examples).

### 5.5 Diagnostics & misc flags worth knowing

`--log-file <path|->` + `--log-level error|warn|info|debug` (undocumented in `--help` but declared stable for scripting; `-` ignored under `--mcp`/`--acp` since stdout/stderr carry protocol), `--no-bell`, `--no-update-terminal-title`, `auggie account status`, request/session IDs for support escalations ([reference §Diagnostics](https://docs.augmentcode.com/cli/reference#diagnostics), [logs](https://docs.augmentcode.com/troubleshooting/logs), [request IDs](https://docs.augmentcode.com/troubleshooting/request-id)). Corporate networks must allowlist Augment endpoints/firewall rules per [network configuration](https://docs.augmentcode.com/setup-augment/network-configuration) (another reminder that traffic terminates at Augment's cloud, cf. §3.2).

Adjacent programmatic surfaces: `--acp` (Agent Client Protocol for Zed/Neovim/Emacs) ([ACP](https://docs.augmentcode.com/cli/acp/agent)), TypeScript/Python SDKs (`@augmentcode/auggie-sdk`, `auggie-sdk`) ([SDK](https://docs.augmentcode.com/cli/sdk)), and `auggie cloud` (YAML-bundle GitOps over the Cosmos control plane) ([cloud](https://docs.augmentcode.com/cli/cloud)).

---

## Cheat sheet: where does X live?

| Want to change… | Where |
|---|---|
| Account/token | `auggie login`, `~/.augment/session.json`, `AUGMENT_SESSION_AUTH`, `--augment-session-json` |
| Shell / theme / notifications / autoupdate | `/config` wizard or `~/.augment/settings.json` |
| Team-wide policy | Commit `.augment/settings.json` (toolPermissions, mcpServers, hooks); lock via `/etc/augment/settings.json` |
| Personal per-repo override | `.augment/settings.local.json` |
| Project instructions | `AGENTS.md` / `CLAUDE.md` (hierarchical) or `.augment/rules/*.md` |
| Reusable prompts | `.augment/commands/*.md` (slash + `auggie command`) |
| Extra tools | `mcpServers` in settings, `auggie mcp add*`, `--mcp-config` |
| What the agent may execute | `toolPermissions` + `removedTools` + hooks |
| Index scope | `.gitignore` + `.augmentignore`, `--workspace-root`, `--add-workspace`, `--allow-indexing`, `--wait-for-indexing` |
| Which LLM | `--model` / `/model` (Augment-hosted catalog only — no BYO endpoint) |
