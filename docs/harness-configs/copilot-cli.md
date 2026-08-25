# GitHub Copilot Agentic Surfaces — Configurable Options

**Scope:** Copilot CLI (local terminal agent) and Copilot Coding Agent (cloud agent on github.com).
**Compiled:** 2026-08-25. Sources cited inline; all from official GitHub Docs unless noted.
**Honesty notes** are flagged where docs are ambiguous, version-skewed, or where a capability does *not* exist.

---

## 1. Copilot CLI — config location, MCP schema, tool permissions, onboarding

### 1.1 Config location (`~/.copilot/`)

Everything lives in one directory, default `~/.copilot` (`$HOME/.copilot`). Override with the `COPILOT_HOME` env var (replaces the whole path — copy existing contents over if you want history/permissions preserved). Legacy XDG locations are migrated automatically; the `--config-dir=DIRECTORY` flag still works but is deprecated in favor of `COPILOT_HOME`. ([CLI config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference))

Key items ([config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)):

| Path | Type | Purpose |
|---|---|---|
| `settings.json` | user-editable | Primary personal config (JSONC). Managed by `/settings [KEY VALUE]` or direct edit. Old `config.json` settings auto-migrate here. Symlink-friendly (dotfiles syncing works). |
| `mcp-config.json` | user-editable | User-level MCP server definitions (all sessions, all projects). |
| `lsp-config.json` | user-editable | User-level LSP server definitions (managed by `/lsp`). |
| `copilot-instructions.md` + `instructions/` | user-editable | Global personal custom instructions (`*.instructions.md`). |
| `agents/` | user-editable | Personal custom agents (`.agent.md`); project-level `.github/agents/` wins name conflicts. |
| `skills/` | user-editable | Personal skills, one subdir each with `SKILL.md`. |
| `hooks/` | user-editable | User-level hook scripts (inline `hooks` key in `settings.json` also works; repo `.github/hooks/` loads alongside). |
| `extensions/`, `installed-plugins/`, `plugin-data/` | mixed | Extensions/plugins. |
| `config.json` | auto-managed | Internal app state: auth data, installed plugins. Do not hand-edit. |
| `permissions-config.json` | auto-managed | Saved per-project tool/directory approvals (schema below). |
| `logs/`, `session-state/`, `session-store.db` | auto-managed | Logs, session history, cross-session SQLite store. |

Repo-level overrides: project MCP configs (`.mcp.json` or `.github/mcp.json`) take precedence over user-level `mcp-config.json` on name conflict ([config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)).

### 1.2 MCP server config schema

Manage via subcommands or by editing the JSON file ([Adding MCP servers](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers)):

```shell
# stdio/local server
copilot mcp add SERVER-NAME -- COMMAND [ARGS...]
copilot mcp add github -e GITHUB_PERSONAL_ACCESS_TOKEN=ghp_pat \
  -- docker run -i --rm -e GITHUB_PERSONAL_ACCESS_TOKEN ghcr.io/github/github-mcp-server

# remote HTTP server (optionally with auth headers)
copilot mcp add --transport http notion https://mcp.notion.com/mcp
copilot mcp add --transport http --header "Authorization: Bearer TOKEN" stripe https://mcp.stripe.com

copilot mcp get|list|remove SERVER-NAME   # inspect/remove
```

File shape in `~/.copilot/mcp-config.json` ([add-mcp-servers](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers)):

```json
{
  "mcpServers": {
    "playwright": {
      "type": "local",
      "command": "npx",
      "args": ["@playwright/mcp@latest"],
      "env": {},
      "tools": ["*"]
    },
    "context7": {
      "type": "http",
      "url": "https://mcp.context7.com/mcp",
      "headers": { "CONTEXT7_API_KEY": "YOUR-API-KEY" },
      "tools": ["*"]
    }
  }
}
```

Extra knobs ([command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)): `--additional-mcp-config` for one-session-only servers; per-server `disableToolCache: true` or global `COPILOT_MCP_TOOL_CACHE=false` to disable tool-snapshot caching; `/mcp` inside a session shows configured server names (needed for permission patterns); `enabledMcpServers` / `disabledMcpServers` keys in `settings.json`.

Enterprise/MDM gate: admins can push `allowedMcpServers` / `deniedMcpServers` matchers (`serverUrl` wildcards, exact `serverCommand` argv, or `serverName`) via managed settings; deny always wins, trusted first-party servers exempt, unset allowlist = all allowed, empty array = deny-all ([config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)).

### 1.3 `--allow-tool` / `--deny-tool` permissions

Read-only ops run automatically; anything mutating prompts for approval (once / rest-of-session choices) ([Allowing and denying tool use](https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli/allowing-tools)).

Pattern syntax is `Kind(argument)`; omitting the argument matches all of that kind ([command reference, Tool permission patterns](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)):

| Kind | Matches | Examples |
|---|---|---|
| `shell` | shell commands (`:*` = stem + space, so `shell(git:*)` matches `git push` but not `gitea`) | `shell(git push)`, `shell(git:*)` |
| `read` | file/dir reads | `read(.env)` |
| `write` | file create/modify (exact or trailing-path-segment match, no globs) | `write(src/*.ts)`, `write(secret.txt)` |
| `url` | URL access via web-fetch/shell | `url(github.com)`, `url(https://*.api.com)` |
| `memory` | storing facts to memory | `memory` |
| `SERVER-NAME` | any tool from your configured MCP server | `MyMCP(create_issue)`, `MyMCP` |

```shell
copilot --allow-tool='shell(git:*)' --deny-tool='shell(git push)'     # git yes, push no
copilot --deny-tool='My-MCP-Server(tool_name)' --allow-tool='My-MCP-Server'  # all-but-one MCP tool
copilot --allow-tool='read, write(.github/copilot-instructions.md)'
copilot --available-tools='bash,edit,view,grep,glob' --allow-tool='shell(git:*)' --deny-tool='shell(git push)'
```

Rules ([allowing-tools](https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli/allowing-tools)):
- **Deny beats allow**, even against `--allow-all` or a saved approval in `permissions-config.json`.
- `--allow-all-tools`, and `--allow-all` / `--yolo` (= all tools + paths + urls); `/allow-all` `/yolo` slash commands in-session. On Business/Enterprise licenses these may be **blocked by an administrator policy**. Docs strongly warn against aliasing yolo into every launch; prefer sandboxes.
- Second control layer: `--available-tools=TOOL,...` restricts which tools exist at all for the model; `--excluded-tools='web_fetch, web_search'` removes some ([allowing-tools](https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli/allowing-tools)).
- Saved approvals live in auto-managed `permissions-config.json`, keyed by absolute Git-root path with approval kinds `commands, read, write, mcp, mcp-sampling, memory, custom-tool, extension-management, extension-permission-access`; reset with `/reset-allowed-tools` ([best practices](https://docs.github.com/en/copilot/how-tos/copilot-cli/cli-best-practices), [config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)).
- Admins can additionally push `permissions.deny/ask/allow` rule families (`Shell(...)`, `PowerShell(...)`, `Read/Edit/Write(path globs)`, `Domain(...)`) via managed settings; fixed precedence deny > ask > allow, unmatched ops default to ask when any list is set ([config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)).

### 1.4 Install & onboarding

([copilot-cli README](https://github.com/github/copilot-cli/blob/main/README.md))

- Install: `curl -fsSL https://gh.io/copilot-install | bash` (supports `PREFIX=`, `VERSION=`), Homebrew `brew install copilot-cli` (+ `@prerelease`), WinGet `winget install GitHub.Copilot`, npm `npm install -g @github/copilot`.
- Prereqs: active Copilot subscription; org/enterprise owners can **disable Copilot CLI entirely via policy**, blocking use.
- First launch: run `copilot`; if unauthenticated you're prompted for `/login` (OAuth web flow locally, device-code flow on SSH/CI). `--banner` replays the splash.
- Token alternative: fine-grained PAT (v2) with the **"Copilot Requests"** permission exported as `GH_TOKEN`/`GITHUB_TOKEN`.
- `copilot login` options ([command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)): `--host https://example.ghe.com` (GH Enterprise Cloud data residency), `--web-flow`, `--device-code`, `--with-token < file`.
- Repo onboarding: `copilot init` / `/init` generates `.github/copilot-instructions.md`; instruction sources merged include `CLAUDE.md`, `GEMINI.md`, `AGENTS.md`, `.github/instructions/**/*.instructions.md` ([command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)).
- Help topics: `copilot help billing|config|commands|environment|logging|monitoring|permissions|providers|sandbox`.

---

## 2. Environment variables

Auth token precedence ([command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)): **`COPILOT_GITHUB_TOKEN` > `GH_TOKEN` > `GITHUB_TOKEN`**. Accepted token types: fine-grained PATs (v2) with "Copilot Requests", OAuth tokens from the Copilot CLI app, OAuth tokens from the `gh` app. **Classic `ghp_` PATs are NOT supported** for CLI auth.

Selected variables ([command reference, Environment variables](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)):

| Variable | Effect |
|---|---|
| `COPILOT_GITHUB_TOKEN` / `GH_TOKEN` / `GITHUB_TOKEN` | Auth token (see precedence). Best route for headless/automation. |
| `COPILOT_HOME` | Override config/state dir (default `$HOME/.copilot`). |
| `COPILOT_CACHE_HOME` | Override cache dir. |
| `COPILOT_MODEL` | Default model selection (same as `--model`). |
| `COPILOT_ALLOW_ALL=true` | Equivalent of `--allow-all`. |
| `COPILOT_GH_HOST` | GitHub hostname for Copilot CLI only (overrides `GH_HOST`) — e.g. point at GHE Cloud data-residency host while `GH_HOST` targets ES. |
| `COPILOT_AUTO_UPDATE=false` | Disable CLI/plugin auto-update. |
| `COPILOT_PLAN_THEN_AUTOPILOT` | Plan→autopilot for harnesses that can only inject env vars. |
| `COPILOT_SUBAGENT_MAX_CONCURRENT` / `_MAX_DEPTH` | Subagent fan-out limits (defaults 32 / 4). |
| `GITHUB_COPILOT_PROMPT_MODE_EXTENSIONS` | `true` loads project extensions in non-interactive prompt mode (`-p`). Default off (untrusted-repo safety). |
| `GITHUB_COPILOT_PROMPT_MODE_REPO_HOOKS` | `true` loads repo hooks in `-p` mode (auto-loads if folder trusted or `COPILOT_ALLOW_ALL`). |
| `GITHUB_COPILOT_PROMPT_MODE_WORKSPACE_MCP` | `true` loads workspace MCP sources in `-p` mode. Default off. |

The three `GITHUB_COPILOT_PROMPT_MODE_*` vars are the main `GITHUB_COPILOT_CLI_*`-family switches documented; they gate what repo-controlled code an unattended `-p` run will execute. Secret hygiene: `--secret-env-vars=VAR` redacts env vars from shell/MCP environments; `GITHUB_TOKEN` and `COPILOT_GITHUB_TOKEN` values are redacted from output by default.

---

## 3. Model selection

- Flag: `copilot --model=MODEL`; env: `COPILOT_MODEL`; interactive: `/model [--session|--global|--repo] [MODEL]` (also `/models`), switchable mid-turn; `auto` lets Copilot pick ([command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)).
- Per the CLI command reference, models include: `claude-sonnet-4.6` (default, general-purpose), `gpt-5.4` (complex reasoning), `claude-haiku-4.5` (fast/lightweight), `gpt-5.3-codex` (code-focused), `gemini-3.1-pro-preview`, `gemini-3.5-flash` / `3.6-flash` / `3.7-flash`, `mai-code-1-flash`.
- The broader [Supported AI models page](https://docs.github.com/copilot/reference/ai-models/supported-models) lists per-client and per-plan availability (Claude Sonnet 4.5/4.6/5, GPT-5 mini/5.3-Codex/5.4/nano/5.5/5.6 Luna/Sol/Terra across Pro/Pro+/Max/Business/Enterprise tiers). Availability varies by plan and client and GitHub manages it dynamically — expect drift between pages ([community discussion #190617](https://github.com/orgs/community/discussions/190617)).
- Version skew warning: the [copilot-cli README](https://github.com/github/copilot-cli/blob/main/README.md) still says "by default utilizes Claude Sonnet 4.5"; current CLI reference says 4.6 is default. Trust the live `/model` picker on your install.
- Reasoning effort is separately configurable: `effortLevel` setting (`low|medium|high|xhigh`, default medium) ([config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)).

---

## 4. Custom endpoints / BYO gateway — honest assessment

**Copilot CLI: YES, real BYOK exists (GA'd April 2026).** You can bypass GitHub-hosted models entirely and point the CLI at any OpenAI-compatible endpoint, Azure OpenAI, or Anthropic — including local Ollama/vLLM ([Using your own LLM models in GitHub Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/use-byok-models), [changelog 2026-04-07](https://github.blog/changelog/2026-04-07-copilot-cli-now-supports-byok-and-local-models/)):

```shell
export COPILOT_PROVIDER_BASE_URL=https://your-gateway.example.com/v1   # required — any base URL
export COPILOT_PROVIDER_TYPE=openai        # openai (default; Ollama/vLLM/anything Chat-Completions compatible), azure, or anthropic
export COPILOT_PROVIDER_API_KEY=...        # optional for keyless local providers
export COPILOT_MODEL=your-model-name       # then select the model as usual
```

Caveats worth knowing: BYOK traffic is yours end-to-end (GitHub premium-request quota doesn't apply); `continueOnAutoMode` rate-limit fallback explicitly does not apply to BYOK providers; BYOK reasoning tokens are stripped on session resume unless `COPILOT_STRIP_REASONING_ON_RESUME=0`. Quick reference lives at `copilot help providers`. So yes — BYO gateway/base URL is genuinely supported on the CLI surface.

**Copilot Coding Agent (cloud): NO BYO endpoint.** There is no documented option to redirect the cloud agent's LLM inference to your own gateway/base URL; inference runs on GitHub's managed backend (Azure OpenAI-hosted models) inside GitHub's cloud runner, and you only choose among the offered models. What enterprises get instead are policy gates, not endpoint control:
- Org/enterprise policy toggles can disable Copilot features incl. the CLI and coding agent ([managing policies for Copilot in your organization](http://docs.github.com/copilot/managing-copilot/managing-github-copilot-in-your-organization/managing-github-copilot-features-in-your-organization/managing-policies-for-copilot-in-your-organization), referenced from the [copilot-cli README](https://github.com/github/copilot-cli/blob/main/README.md)).
- `--allow-all` style permissive options can be blocked by Business/Enterprise admin ([allowing-tools](https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli/allowing-tools)); MDM managed settings can lock settings keys and enforce permission/MCP policy ([config dir reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)).
- Network-side, corporate firewalls/proxies just need the [Copilot allowlist reference](https://docs.github.com/en/copilot/reference/copilot-allowlist-reference) domains — that's egress control, not model-endpoint control.

If you need a fully self-selected backend behind an issue-driven agent, the honest answer is: use Copilot CLI with BYOK locally, not the cloud coding agent.

---

## 5. Copilot Coding Agent (cloud)

### 5.1 Assigning issues

- UI: assign the issue to **Copilot** (or "Assign to Copilot" button) like a teammate; batch-assigning multiple issues is supported; the PR author rule applies — whoever assigned can't be the final approver of Copilot's PR ([About cloud agent](https://docs.github.com/copilot/concepts/agents/cloud-agent/about-cloud-agent), [github.blog walkthrough](https://github.blog/ai-and-ml/github-copilot/assigning-and-completing-issues-with-coding-agent-in-github-copilot/)).
- Assignment dialog extras: choose target repository (cross-repo), starting branch, extra instructions, and a custom agent ([community discussion #173575](https://github.com/orgs/community/discussions/173575)).
- API (Dec 2025 changelog, examples in [#173575](https://github.com/orgs/community/discussions/173575)): REST `POST/PATCH /repos/{owner}/{repo}/issues[/{n}]` with `"assignees": ["copilot-swe-agent[bot]"]` plus an `agent_assignment` object `{target_repo, base_branch, custom_instructions, custom_agent}`; GraphQL mutations (`createIssue` / `updateIssue` / `replaceActorsForAssignable`) with the same `agentAssignment` input and header `GraphQL-Features: issues_copilot_assignment_api_support`. A later discussion thread shows an optional `model` field under `coding_agent_model_selection` feature flag — treat model-via-API as preview/unstable.
- Failure mode seen in the field: assigning token needs read/write to Actions, Contents, Issues, and Pull Requests on the target repo, else the agent errors out instead of starting ([#173575](https://github.com/orgs/community/discussions/173575)).

### 5.2 MCP configuration

Two layers ([Configure MCP servers for your repository](https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/configure-mcp-servers), [Custom agents configuration](https://docs.github.com/en/copilot/reference/custom-agents-configuration)):

1. **Repository-wide (Settings → Copilot → MCP servers):** admin pastes a JSON config shared by cloud agent + code review. Schema: top-level `mcpServers`; each entry has `type` (`local`/`stdio`/`http`/`sse`), `tools` (string[] — strongly recommend allowlisting specific read-only tools; `*` enables all; the agent uses them autonomously with no approval prompt), local entries need `command`+`args`+optional `env`, remote entries need `url`+optional `headers`. Secrets/vars come from Agents secrets named `COPILOT_MCP_*`, referenced as `$VAR`, `${VAR}`, `${VAR:-default}`. Defaults enabled out of the box: the GitHub MCP server and Playwright MCP. Remote OAuth-authenticated MCP servers are **not** supported; resources/prompts are not supported, tools only.
2. **Custom agents** (`.github/agents/*.agent.md` YAML frontmatter, also org/enterprise levels): `mcp-servers:` object mirrors the JSON format; `tools:` filters built-ins and MCP tools with `server/tool` namespacing (`github/*`, `playwright/*` available out-of-box); processing order is built-ins → agent profile → repo settings, lowest level wins on conflicts.

Note vs. the task phrasing: as of these docs pages the repo MCP config is managed through the repo Settings UI (not a committed `.github/copilot/*.yml` file); the YAML surfaces are the custom-agent frontmatter above and `.github/workflows/copilot-setup-steps.yml` for environment setup (e.g., Azure/OIDC login before the agent runs).

### 5.3 Firewall & network allowlists

([Customizing or disabling the firewall](https://docs.github.com/copilot/how-tos/agents/copilot-coding-agent/customizing-or-disabling-the-firewall-for-copilot-coding-agent), [Copilot allowlist reference](https://docs.github.com/en/copilot/reference/copilot-allowlist-reference))

- The cloud agent ships a built-in egress firewall with a **recommended allowlist enabled by default**: OS package repos (Debian/Ubuntu/RHEL…), container registries (Docker Hub, ACR, ECR…), language package registries (~15 ecosystems), certificate authorities, and browser-download hosts for Playwright.
- **Repository level:** Settings → Copilot → firewall → "Custom allowlist"; entries are domains (domain + subdomains) or URLs (scheme+host locked, path + descendants). 
- **Organization level:** "Organization custom allowlist" plus a toggle controlling whether repositories may add their own rules at all.
- Corporate-side (your own perimeter): allowlist the domains in the [allowlist reference](https://docs.github.com/en/copilot/reference/copilot-allowlist-reference) — best fetched dynamically via the `/meta` API endpoint; specifics include `avatars.githubusercontent.com` (auth) and `github.com/copilot/*`, plus agent hosts like `codeload.github.com`, `api.mcp.github.com`.
- Practical gotcha: a domain allowlist entry alone often isn't enough — registry index hosts and CDN subdomains frequently need explicit entries (e.g. PureScript's `packages.registry.purescript.org` case, [discourse thread](https://discourse.purescript.org/t/ps-pursuit-firewall-allowlist-what-addresses-to-allowlist-for-github-copilot-agent/4985)).

---

## 6. Multi-instance wrappers (account switching)

Mechanism: env-var auth beats stored logins, so a wrapper that exports a token gets a clean identity without touching `~/.copilot` state ([command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)). Two hard-won details:

- Precedence is `COPILOT_GITHUB_TOKEN` > `GH_TOKEN` > `GITHUB_TOKEN`. If you already use `GH_TOKEN` for `gh` against another host, prefer `COPILOT_GITHUB_TOKEN` in the wrapper so the two never collide; conversely plain `GH_TOKEN` keeps one var serving both tools.
- Tokens must be fine-grained PATs (v2) with the **Copilot Requests** permission (or an OAuth token minted via `copilot login --with-token` / the `gh` app flow). Classic `ghp_` PATs silently won't authenticate.
- For fully isolated profiles (separate MCP servers, sessions, saved permissions, plugin sets), pair the token with `COPILOT_HOME` pointing at a per-identity directory — otherwise all instances share `~/.copilot` and you'll fight over `permissions-config.json` and session state. `COPILOT_GH_HOST` handles GHE Cloud data-residency accounts independently of `GH_HOST`.

Example wrapper (`~/bin/cx`) — two identities × two models:

```bash
#!/usr/bin/env bash
# cx <persona> [...args] — run Copilot CLI under a specific identity/model/profile
set -euo pipefail

PERSONA="${1:?usage: cx <work|oss> [copilot args...]}"; shift

case "$PERSONA" in
  work)
    export COPILOT_GITHUB_TOKEN="$(pass show github/work-copilot-pat)"  # fine-grained PAT, Copilot Requests perm
    export COPILOT_GH_HOST="https://acme.ghe.com"                       # optional: data-residency host
    export COPILOT_HOME="$HOME/.copilot-work"                           # isolated MCP/permissions/session state
    MODEL="claude-sonnet-4.6"                                           # heavier model for work codebases
    ;;
  oss)
    export COPILOT_GITHUB_TOKEN="$(pass show github/oss-copilot-pat)"
    unset COPILOT_GH_HOST                                               # github.com
    export COPILOT_HOME="$HOME/.copilot-oss"
    MODEL="gpt-5.4"
    ;;
  *) echo "unknown persona: $PERSONA" >&2; exit 1;;
esac

exec copilot --model "$MODEL" "$@"
# Usage: cx work                      # interactive, Sonnet 4.6, work identity
#        cx oss -p "summarize TODOs"  # headless prompt mode, GPT-5.4, OSS identity
```

Notes: `exec` keeps signals/job control sane; keep tokens out of shell history (secret manager or `copilot login --with-token` once per `COPILOT_HOME` instead of env injection if you prefer stored credentials); `--model` can be overridden ad hoc (`cx work --model gpt-5.3-codex`) since later flags win; add `COPILOT_ALLOW_ALL=1` per-persona only inside sandboxed environments.

---

## Source index

- Copilot CLI README (install/onboarding/LSP): https://github.com/github/copilot-cli/blob/main/README.md
- Configuring Copilot CLI (allow/deny basics): https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/configure-copilot-cli
- Allowing and denying tool use: https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli/allowing-tools
- Using Copilot CLI overview (mcp-config location): https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli/overview
- Adding MCP servers for Copilot CLI: https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers
- CLI command reference (login, env vars, tool patterns, models): https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference
- CLI config dir reference (~/.copilot layout, permissions-config, MDM): https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference
- CLI best practices (/reset-allowed-tools, patterns): https://docs.github.com/en/copilot/how-tos/copilot-cli/cli-best-practices
- About Copilot CLI (BYOK summary, security): https://docs.github.com/copilot/concepts/agents/about-copilot-cli
- BYOK how-to: https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/use-byok-models (+ changelog https://github.blog/changelog/2026-04-07-copilot-cli-now-supports-byok-and-local-models/)
- Supported AI models: https://docs.github.com/copilot/reference/ai-models/supported-models
- Configure MCP servers for your repository (cloud agent): https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/configure-mcp-servers
- Custom agents configuration (.agent.md frontmatter): https://docs.github.com/en/copilot/reference/custom-agents-configuration
- Firewall customization: https://docs.github.com/copilot/how-tos/agents/copilot-coding-agent/customizing-or-disabling-the-firewall-for-copilot-coding-agent
- Copilot allowlist reference: https://docs.github.com/en/copilot/reference/copilot-allowlist-reference
- Issue assignment API examples: https://github.com/orgs/community/discussions/173575
