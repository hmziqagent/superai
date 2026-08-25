# Cursor — Complete Configuration Reference

**Scope:** Cursor IDE (editor agent) + Cursor Agent CLI (`agent`, historical binary name `cursor-agent`).
**Compiled:** 2026-08-25. Primary sources: [cursor.com/docs](https://cursor.com/docs), [cursor.com/help](https://cursor.com/help), plus forum/GitHub sources where the docs are silent (flagged inline).

---

## 1. Config surfaces

### 1.1 Editor settings

Cursor is a VS Code fork, so it inherits the standard VS Code settings stack (User `settings.json`, workspace `.vscode/settings.json`, keybindings) **plus** Cursor-specific surfaces:

| Surface | Location | Notes |
|---|---|---|
| Cursor Settings UI | `Cmd/Ctrl+Shift+J` | Models/API keys ([§3](#3-custom-api-bring-your-own-key)), Privacy Mode ([§5.4](#54-privacy-modes)), Features (incl. Hierarchical Cursor Ignore), Indexing, Rules/Customize |
| VS Code-style user settings | Standard VS Code path, e.g. `~/.config/Cursor/User/settings.json` (Linux), `%APPDATA%\Cursor\User\settings.json` (Win), `~/Library/Application Support/Cursor/User/settings.json` (macOS) | Inherited VS Code behavior; not restated in Cursor docs |
| Model selection | Model picker in chat/agent panel, or `Cmd/Ctrl+/` to cycle; persists across conversations | [Available models](https://cursor.com/help/models-and-usage/available-models) |
| Customize page (sidebar) | MCP servers, Plugins, Rules, Skills management | [Customize overview](https://cursor.com/docs/customize-cursor) |

### 1.2 MCP — `~/.cursor/mcp.json` (global) and `<project>/.cursor/mcp.json`

Source: [MCP docs](https://cursor.com/docs/context/mcp)

- Configure servers via JSON file or one-click install from the Cursor Marketplace / team marketplace (team-distributed servers appear alongside personal and workspace ones).
- Transports: `stdio` (local, single user, manual auth), `SSE` and Streamable `HTTP` (remote, multi-user, OAuth).
- Supported protocol capabilities: Tools, Prompts, Resources, Roots, Elicitation, and the **MCP Apps** extension (interactive UI returned by tools).

**STDIO server fields** (all in `mcpServers.<name>`):

| Field | Required | Description |
|---|---|---|
| `type` | Yes | `"stdio"` |
| `command` | Yes | Executable (on PATH or absolute path): `"npx"`, `"python"`, … |
| `args` | No | Argument array |
| `env` | No | Env vars for the server process; supports `${env:VAR}` interpolation |
| `envFile` | No | Path to an env file, e.g. `"${workspaceFolder}/.env"` |

```json
{
  "mcpServers": {
    "server-name": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "mcp-server"],
      "env": { "API_KEY": "${env:MY_API_KEY}" }
    }
  }
}
```

**Remote server fields:** `url` + optional `headers`; OAuth via dynamic registration or **static OAuth** block:

```json
{
  "mcpServers": {
    "oauth-server": {
      "url": "https://api.example.com/mcp",
      "auth": {
        "CLIENT_ID": "${env:MCP_CLIENT_ID}",
        "CLIENT_SECRET": "${env:MCP_CLIENT_SECRET}",
        "scopes": ["read", "write"]
      }
    }
  }
}
```

Static-OAuth redirect URLs to register with the provider: `https://www.cursor.com/agents/mcp/oauth/callback` (web/cloud agents) and `http://localhost:8787/callback` (desktop app).

Operational controls: per-server enable/disable toggle (**Customize → MCP**, disabled servers don't load); logs in Output panel → "MCP Logs"; server crashes are isolated. The CLI manages these with `agent mcp list|list-tools|login|enable|disable` against `.cursor/mcp.json` / `~/.cursor/mcp.json` ([CLI parameters](https://cursor.com/docs/cli/reference/parameters)); the CLI auto-detects and respects `mcp.json` ([CLI using](https://cursor.com/docs/cli/using)).

### 1.3 Project rules — `.cursor/rules/*.mdc`

Source: [Rules docs](https://cursor.com/docs/context/rules)

Four rule types: **Project Rules** (`.cursor/rules/`, versioned), **User Rules** (global, Customize → Rules, used by Agent/Chat only — *not* applied to Inline Edit `Cmd/Ctrl+K`), **Team Rules** (dashboard-managed, Team/Enterprise), and **AGENTS.md** (below).

Each project rule is a `.mdc` file with YAML frontmatter. A plain `.md` in `.cursor/rules` is **ignored** (no frontmatter). Frontmatter interaction matrix:

| `alwaysApply` | `description` | `globs` | Behavior |
|---|---|---|---|
| `true` | — | — | Always included (globs/description ignored) |
| `false` | — | provided | Auto-attached when a matching file is in context |
| `false` | provided | omitted | Agent pulls the rule in when it deems it relevant ("Apply Intelligently") |
| `false` | omitted | omitted | Only included when `@`-mentioned in chat |

Glob syntax: comma-separate multiple patterns (`docs/**/*.md, docs/**/*.mdx`); `*` = one segment, `**` = recursive, `tailwind.config.*` style works. Example auto-attach rule:

```markdown
---
globs: src/components/**/*.tsx
alwaysApply: false
---
- Use named exports, not default exports
- Keep components under 200 lines
```

Creation: `/create-rule` in chat, or Customize → Rules → Add Rule; remote rules importable from any GitHub repo (synced into `.cursor/rules/imported/<repoName>/`). Rules do not affect Tab. Best practice caps: keep under ~500 lines, split composable rules, reference files with `@filename`.

### 1.4 `.cursorignore` / `.cursorindexingignore`

Source: [Ignore file docs](https://cursor.com/docs/reference/ignore-file)

- `.cursorignore` (project root, gitignore syntax) blocks file access for **Agent, Tab, Inline Edit and @-mentions**. Caveat stated by docs: *terminal and MCP tools used by Agent cannot be blocked* by it.
- Syntax: `*`, `**`, `?`, `!` negation, `#` comments. Negation limits match gitignore semantics (can't re-include under a parent excluded via `public/*`; exclude sibling dirs explicitly instead).
- **Hierarchical ignore**: setting `Cursor Settings > Features > Editor > Hierarchical Cursor Ignore` (moved to `Indexing > Ignore Files` in Cursor 3.11+) searches parent dirs for `.cursorignore`.
- **Global ignore files**: user-level ignore patterns in settings apply to all projects (empty by default; suggested: `**/.env*`, `**/*.pem`, `**/id_rsa`, …).
- `.cursorindexingignore`: excludes files from **indexing/search only** — they remain accessible to AI features. Use for vendored/generated trees.
- Defaults: everything in `.gitignore` plus a long built-in index-ignore list (lockfiles, binaries, media, `.venv`, `node_modules`, `.next`, caches, etc.). Test patterns with `git check-ignore -v <file>`.

### 1.5 AGENTS.md

Source: [Rules docs §AGENTS.md](https://cursor.com/docs/context/rules#agentsmd)

- Plain markdown alternative to `.cursor/rules`; place at project root. **Nested AGENTS.md supported**: files in any subdirectory apply when working there; child instructions combine with parents, more specific take precedence.
- The **CLI additionally reads `CLAUDE.md`** at the project root and applies it alongside `.cursor/rules` and `AGENTS.md` ([CLI using](https://cursor.com/docs/cli/using#rules)).

### 1.6 CLI configuration — VERIFIED: `~/.cursor/cli-config.json`

Source: [CLI Configuration](https://cursor.com/docs/cli/reference/configuration). Yes — the path in the task guess is correct.

| Type | Platform | Path |
|---|---|---|
| Global | macOS/Linux | `~/.cursor/cli-config.json` |
| Global | Windows | `$env:USERPROFILE\.cursor\cli-config.json` |
| Project | All | `<project>/.cursor/cli.json` |

- **Only `permissions` may be set at project level**; everything else must be global.
- Directory overrides: `CURSOR_CONFIG_DIR=<dir>` (custom dir), or `XDG_CONFIG_HOME` on Linux/BSD → `$XDG_CONFIG_HOME/cursor/cli-config.json`.
- Pure JSON, no comments; missing fields self-repair; corrupted file backed up as `.bad` and recreated (recovery: `mv ~/.cursor/cli-config.json ~/.cursor/cli-config.json.bad`).

**Schema (current `version: 1`)**

Required fields:

| Field | Type | Description |
|---|---|---|
| `version` | number | Schema version (`1`) |
| `editor.vimMode` | boolean | Vim keybindings (default `false`) |
| `permissions.allow` | string[] | Permitted operations |
| `permissions.deny` | string[] | Forbidden operations |

Optional fields:

| Field | Type | Description |
|---|---|---|
| `channel` | string | Release channel for CLI updates |
| `model` | object | Selected model configuration |
| `maxMode` | boolean | Persisted max-mode preference in model picker |
| `hasChangedDefaultModel` | boolean | CLI-managed model override flag |
| `notifications` | boolean | Terminal notification when agent finishes/needs input |
| `hints` | boolean | Show CLI hints while working |
| `rewind` | boolean | Enable `/rewind` |
| `suggestNextPrompt` | boolean | Suggest follow-up prompt each turn |
| `display.showLineNumbers` | boolean | Line numbers in code blocks |
| `display.showThinkingBlocks` | boolean | Render thinking blocks |
| `display.showStatusIndicators` | boolean | Terminal title status indicators |
| `display.showStatusLineRunningTime` | boolean | Elapsed time in status line |
| `approvalMode` | string | `allowlist` \| `auto-review` \| `unrestricted` |
| `sandbox.mode` | string | Sandbox mode override |
| `sandbox.networkAccess` | string | Sandbox network access setting |
| `network.useHttp1ForAgent` | boolean | HTTP/1.1+SSE fallback for proxies like Zscaler (default `false`) |
| `attribution.attributeCommitsToAgent` | boolean | "Made with Cursor" commit trailer (default `true`) |
| `attribution.attributePRsToAgent` | boolean | "Made with Cursor" PR footer (default `true`) |

**Permissions syntax** ([Permissions](https://cursor.com/docs/cli/reference/permissions)):

```json
{
  "version": 1,
  "editor": { "vimMode": false },
  "permissions": {
    "allow": ["Shell(ls)", "Shell(git)", "Read(src/**/*.ts)", "Write(package.json)",
              "WebFetch(docs.github.com)", "WebFetch(*.github.com)", "Mcp(datadog:*)"],
    "deny":  ["Shell(rm)", "Read(.env*)", "Write(**/*.key)", "WebFetch(malicious-site.com)"]
  }
}
```

Known quirk (forum, May 2026): permission allowlists chosen interactively have been written to the global config instead of project `.cursor/cli.json` ([forum #160343](https://forum.cursor.com/t/cli-permission-allowlist-written-to-global-config-instead-of-project-level-cursor-cli-json/160343)).

### 1.7 Other project dotfiles Cursor reads

| File | Purpose | Source |
|---|---|---|
| `.cursor/environment.json` | Cloud Agent environment config ([§5](#5-backgroundcloud-agents--config)) | [Cloud setup](https://cursor.com/docs/cloud-agent/setup) |
| `.cursor/worktrees.json` | Worktree setup scripts (skippable via `--skip-worktree-setup`) | [CLI parameters](https://cursor.com/docs/cli/reference/parameters) |
| `.cursor/Dockerfile` + install scripts | Cloud agent image build | [Cloud setup](https://cursor.com/docs/cloud-agent/setup) |
| `.cursor/rules/`, `.cursor/mcp.json`, `.cursorignore` | As above | — |

---

## 2. Environment variables

### 2.1 Documented by Cursor

| Var | Where | Effect | Source |
|---|---|---|---|
| `CURSOR_API_KEY` | CLI / SDK / CI | API-key auth for `agent` (alternative: `--api-key <key>` flag); generated at Dashboard → API Keys | [Authentication](https://cursor.com/docs/cli/reference/authentication), [Headless](https://cursor.com/docs/cli/headless) |
| `CURSOR_CONFIG_DIR` | CLI | Override config directory (holds `cli-config.json`) | [Configuration](https://cursor.com/docs/cli/reference/configuration) |
| `XDG_CONFIG_HOME` | CLI (Linux/BSD) | Falls back to `$XDG_CONFIG_HOME/cursor/cli-config.json` | same |
| `NO_OPEN_BROWSER=1` | CLI | `agent login` prints URL instead of opening a browser | [Authentication](https://cursor.com/docs/cli/reference/authentication) |
| `HTTP_PROXY`, `HTTPS_PROXY`, `NODE_USE_ENV_PROXY=1` | CLI | Route CLI traffic through corporate proxy | [Configuration §Proxy](https://cursor.com/docs/cli/reference/configuration#proxy-configuration) |
| `NODE_EXTRA_CA_CERTS=/path/ca.pem` | CLI | Trust MITM/SSL-inspection CA certs | same |
| `CURSOR_WORKER_LABELS_FILE` | CLI worker | Labels file for `agent worker` (alt to `--labels-file`) | [Parameters §Worker](https://cursor.com/docs/cli/reference/parameters#worker) |
| `CURSOR_AGENT` | IDE-set | Set by Cursor while it runs shell commands — detectable in shell rc to disable themes/heavy prompt for agent-run commands (community-documented; see also open FR for `AGENT=1`) | [learncursor.dev](https://www.learncursor.dev/learn/cursor-agents/agent-terminal-tool), [forum FR #41487](https://forum.cursor.com/t/add-agent-1-environment-variable-for-composer-runs/41487) |
| `CURSOR_CLI` | IDE-set | Set inside Cursor's integrated terminal; **breaks `cursor-agent` launched from there** — workaround `unset CURSOR_CLI` | [gist: johnlindquist](https://gist.github.com/johnlindquist/9a90c5f1aedef0477c60d0de4171da3f) |

### 2.2 OPENAI/ANTHROPIC overrides for the CLI?

**None exist.** There is no `OPENAI_API_KEY`/`ANTHROPIC_API_KEY`-style provider override for the Cursor CLI: CLI auth is exclusively browser login or `CURSOR_API_KEY`/`--api-key` against Cursor's backend ([Authentication](https://cursor.com/docs/cli/reference/authentication)). BYO provider keys (§3) are an **IDE-only** feature and even then requests are proxied through Cursor's servers. To point a terminal agent at your own providers you'd need a different tool; within Cursor's own stack the closest is the IDE's OpenAI base-URL override (§3.3), which does not extend to the CLI. (An OpenAI-compatible public endpoint for Cursor's cloud API has been requested but doesn't exist: [forum FR #164522](https://forum.cursor.com/t/openai-compatible-v1-chat-completions-for-cloud-api/164522).)

---

## 3. Custom API (bring-your-own key)

### 3.1 Providers & how to configure

Source: [Bring your own API key](https://cursor.com/help/models-and-usage/api-keys) — Cursor Settings → Models → pick provider → paste key → Save.

| Provider | What you supply | Notes |
|---|---|---|
| **OpenAI** | API key (+ optional base-URL override) | Docs: "Standard, non-reasoning chat models" only — no o-series/reasoning models via BYOK ([community corroboration](https://www.cursor-ide.com/blog/cursor-custom-api-key-guide-2025)) |
| **Anthropic** | API key | All Claude models available through the Anthropic API |
| **Google** | API key | Gemini models via Google AI API |
| **Azure OpenAI** | API key | Models deployed in *your* Azure OpenAI resource |
| **AWS Bedrock** | Access key + secret **in the IDE**, or IAM role via dashboard | See §3.4 |

Provider models appear in the model picker once saved; an invalid/rejected key makes that provider's requests fail until updated or removed.

### 3.2 Hard limitations (what stays on Cursor's backend)

From [API keys FAQ](https://cursor.com/help/models-and-usage/api-keys) + [LLM Gateway guide](https://docs.llmgateway.io/guides/cursor):

- **Chat models only**: Tab completion always uses Cursor's built-in models; inline edit (`Cmd/Ctrl+K`) also stays on Cursor's backend even with a custom OpenAI key + base URL.
- **Requests are still routed through Cursor's servers** for final prompt building — your API key is transmitted (encrypted, not persisted) with every request. This is why the base URL must be publicly reachable; localhost endpoints don't work ([getmaxim.ai](https://www.getmaxim.ai/articles/self-hosted-ai-gateway-for-cursor-with-claude-or-ollama)).
- **ZDR does not apply** to BYOK traffic — your data handling follows your provider's policy instead.
- **Data residency**: BYOK is explicitly unsupported under US-only data residency ([Privacy & Data Governance](https://cursor.com/docs/enterprise/privacy-and-data-governance)); custom models reached via a base-URL override carry the region of that gateway.
- Usage still appears on the dashboard billed as bring-your-own-key usage; Teams/Enterprise pay the Cursor Token Rate ($0.25/M tokens) even on BYOK requests ([Models & Pricing](https://cursor.com/docs/models-and-pricing)).
- Known bugs with base-URL override + third-party gateways: agent-mode failures like `Missing required parameter: tools` when the gateway doesn't implement OpenAI tool-calling params exactly ([forum #167894](https://forum.cursor.com/t/bug-agent-chat-fails-with-missing-required-parameter-tools-6-custom-when-using-custom-openai-api-key-and-base-url-override/167894)); Anthropic-model breakage when the OpenAI base-URL override is set ([forum #144899](https://forum.cursor.com/t/anthropic-models-break-when-override-openai-baseurl-is-set/144899)); strict validation rejecting valid model IDs ([forum #158958](https://forum.cursor.com/t/cursor-custom-openai-provider-rejects-valid-gloo-model-ids-as-model-name-is-not-valid/158958)); custom model entries vanishing on Windows ([forum #157704](https://forum.cursor.com/t/custom-openai-compatible-model-disappears-immediately-after-add-on-windows-works-on-mac-with-same-account/157704)).

### 3.3 Base-URL override

- Exactly **one** override exists: **"Override OpenAI Base URL"** in Settings → Models (OpenAI-compatible endpoints: gateways like LLM Gateway `https://api.llmgateway.io/v1`, Groq `https://api.groq.com/openai/v1`, etc.). It routes the AI panel — plan **and** agent mode — through the endpoint ([LLM Gateway guide](https://docs.llmgateway.io/guides/cursor)).
- **No Anthropic/Google/Azure base-URL override** — top-voted feature request, still absent ([forum #161445](https://forum.cursor.com/t/request-to-override-anthropic-base-url/161445), [#158805](https://forum.cursor.com/t/missing-anthropic-base-url-override-in-cursor-byok/158805)). Setting the OpenAI override affects Anthropic-model calls too (bug above).
- Override is global, not per-model ([ofox.ai comparison](https://ofox.ai/blog/cursor-claude-code-cline-custom-api-setup-2026/)).

### 3.4 AWS Bedrock specifics

Source: [AWS Bedrock guide](https://cursor.com/docs/customizing/aws-bedrock)

Two paths:
1. **IAM role (recommended, teams)** — dashboard-only config: trust policy allowing Cursor's cross-account principal `arn:aws:iam::289469326074:role/roleAssumer` with a generated **External ID**; policy granting `bedrock:InvokeModel(WithResponseStream)` on chosen model ARNs; enable models in the Bedrock console (Model access / Model catalog). Dashboard fields: **AWS IAM Role ARN**, **AWS Region**, **Test Model ID** → Validate & Save.
2. **Access keys** — AWS Access Key ID + Secret pasted directly in Cursor Settings → Models (simpler, less secure).

Per-user opt-in: after team IAM validation, each user must still flip **Settings → Models → AWS Bedrock toggle** (off by default). Once on, models appear under raw Bedrock IDs (`us.anthropic.claude-sonnet-5`; regional prefixes `eu.`/`apac.`/`ca.`). Selecting a standard model name or Auto still routes through Cursor; selecting a non-Bedrock ID fails with "not supported by bedrock". Requests remain visible in usage reporting as BYOK-kind events.

### 3.5 Enterprise gateway / self-hosted

There is **no self-hosted Cursor inference plane**. Enterprise options short of that ([Enterprise docs](https://cursor.com/docs/enterprise/privacy-and-data-governance), [Network Configuration](https://cursor.com/docs/enterprise/network-configuration), [Private Connectivity](https://cursor.com/docs/cloud-agent/private-connectivity)):

- Corporate **proxies** (HTTP_PROXY family + CA certs + HTTP/1.1 fallback, §2.1).
- **Private connectivity** to self-hosted infra (GitHub Enterprise Server, GitLab Enterprise, Bitbucket Data Center, Artifactory, Nexus) via **AWS PrivateLink** or **Cloudflare Tunnel** — covers both directions including webhooks back over `api2.cursor.sh`. Requirements: publicly trusted TLS, DNS ownership, HTTPS:443 only (no SSH, no custom ports, no self-signed certs). Google PSC not offered.
- **Data residency** (US-only today; 10% pricing uplift), **CMEK**, org-enforced Privacy Mode, audit logs/AI-code-tracking API.
- `agent worker` (§5.3) runs cloud agents **inside your own machine/VPC** while still orchestrated by Cursor's control plane.

---

## 4. Multi-instance wrappers (headless CLI)

### 4.1 Binary & invocation

The installer puts the binary on PATH; current primary command is **`agent`**, with **`cursor-agent`** as the historical alias still shipped ([awslabs cli-agent-orchestrator docs](https://github.com/awslabs/cli-agent-orchestrator/blob/main/docs/cursor-cli.md), [codeagentswarm guide](https://www.codeagentswarm.com/en/guides/cursor-agent-cli-acp-codeagentswarm)); some distributions also add a `cursor` shim ([linuxcommandlibrary man page](https://linuxcommandlibrary.com/man/cursor)). Both names accept the same flags below ([Parameters reference](https://cursor.com/docs/cli/reference/parameters)):

Headless pattern ([Headless/CI](https://cursor.com/docs/cli/headless)):

```bash
export CURSOR_API_KEY=key_from_dashboard_api_page
agent -p "prompt"                       # print mode; proposes changes only
agent -p --force "prompt"               # (--yolo alias) actually applies edits
agent -p --output-format json "..."     # text (default) | json | stream-json
agent -p --output-format stream-json --stream-partial-output "..."   # token deltas
agent -p --trust --workspace /repo "…"  # skip trust prompt; explicit repo root
agent --model gpt-5 -p "…" ; agent --list-models ; agent models
agent --mode=ask|-–plan ...             # read-only / planning modes
agent --continue | --resume=<chatId> | resume | ls | create-chat
agent acp                               # ACP server over stdio (JSON-RPC)
```

Other useful flags: `-H "Name: Value"` custom headers on agent requests, `--approve-mcps`, `--sandbox enabled|disabled` (+ `agent sandbox run --network --allow-paths …`), `-w/--worktree [name]` (+ `--worktree-base`), `--plugin-dir <path>` (repeatable), `--skip-worktree-setup`.

Auth: `agent login` (browser flow; `NO_OPEN_BROWSER=1` for headless hosts), `agent status`/`whoami [--format json]`, `agent logout`.

### 4.2 Environment/config switching levers

| Lever | Mechanism | Verified? |
|---|---|---|
| Separate accounts | Different `CURSOR_API_KEY` per process (or `--api-key`) | ✅ docs |
| Separate config dirs | `CURSOR_CONFIG_DIR=$HOME/.cursor-profileB` → isolated `cli-config.json`, permissions, preferences | ✅ docs (path isolation documented; whether cached credentials/session state fully follow this dir is **not documented** — verify empirically per version) |
| XDG layout | `XDG_CONFIG_HOME` respected on Linux/BSD | ✅ docs |
| Inside-Cursor-terminal fix | `unset CURSOR_CLI` before invoking the binary | ⚠️ community gist (§2.1) |
| MCP isolation | Per-project `.cursor/mcp.json` merges with global `~/.cursor/mcp.json`; `agent mcp enable/disable` maintains local approved list | ✅ docs |

### 4.3 Wrapper example — two "providers" (two accounts/config sets)

Since the CLI cannot use raw OpenAI/Anthropic keys (§2.2), multi-provider means multi-*profile*: two Cursor accounts (or an account + service-account key), each with its own config dir, wrapped as distinct commands:

```bash
#!/usr/bin/env bash
# /usr/local/bin/cursor-work  — profile switcher for Cursor Agent CLI
set -euo pipefail
PROFILE="${CURSOR_PROFILE:-personal}"   # personal | work
BASE="$HOME/.cursor-profiles"

case "$PROFILE" in
  personal)
    export CURSOR_CONFIG_DIR="$BASE/personal"
    export CURSOR_API_KEY="$(cat "$BASE/personal/api_key")"
    ;;
  work)
    export CURSOR_CONFIG_DIR="$BASE/work"          # work-owned permissions/model defaults
    export CURSOR_API_KEY="$WORK_CURSOR_API_KEY"   # e.g. from secret store
    ;;
  *) echo "unknown profile: $PROFILE" >&2; exit 2 ;;
esac

mkdir -p "$CURSOR_CONFIG_DIR"

# Corporate proxy only for the work profile
[ "$PROFILE" = work ] && {
  export HTTPS_PROXY=http://proxy.corp:3128 NODE_USE_ENV_PROXY=1
  export NODE_EXTRA_CA_CERTS="$BASE/work/corp-ca.pem"
}

BIN=agent; command -v agent >/dev/null || BIN=cursor-agent   # version tolerance
exec "$BIN" "$@"
```

Usage:

```bash
cursor-work -p "summarize failing tests"                    # personal profile
CURSOR_PROFILE=work cursor-work -p --force --trust "fix lint errors"   # work profile
```

CI variant (single-purpose container): bake one `CURSOR_API_KEY` + minimal `cli-config.json` (`{"version":1,"editor":{"vimMode":false},"permissions":{"allow":["Shell(npm test)","Read(**)","Write(src/**)"],"deny":["Shell(rm)","Read(.env*)"]}}`) into the image and always run `agent -p --force --trust --workspace "$CI_PROJECT_DIR"`.

---

## 5. Background/cloud agents + config, privacy modes

### 5.1 Environments (`environment.json`, Dockerfile, dashboard)

Sources: [Cloud Environment Setup](https://cursor.com/docs/cloud-agent/setup), [Dashboard settings](https://cursor.com/docs/cloud-agent/settings), field reference cross-checked with [learncursor.dev](https://www.learncursor.dev/learn/cursor-agents/cloud-agent-environment-json)

Cloud Agents run on isolated Ubuntu VMs. Resolution order for environment config: repo `.cursor/environment.json` → personal saved environment → team saved environment. Two setup modes: agent-driven guided setup, or manual **Dockerfile** (referenced from `.cursor/environment.json`; don't `COPY` the project; layer caching applied; no direct machine access).

`.cursor/environment.json` fields:

| Field | Meaning |
|---|---|
| `build.dockerfile`, `build.context` | Dockerfile + build context (paths relative to `.cursor/`) |
| `install` | Install/update script run during each Build |
| `start` | Startup command for long-lived processes (e.g. docker daemon) |
| `terminals` | Named tmux terminals shared by you and the agent (`{name, command, description}`) |
| `ports` | Forwarded ports `{name, port}` |
| `user` | Unix user the env runs as |
| `snapshot` / `agentCanUpdateSnapshot` | Base snapshot id; allow agent to update it |
| `repositoryDependencies` | Extra repos (`github.com/org/repo`) included in the env's GitHub token scope |
| `name` | Environment display name |

Dashboard-managed knobs: runtime/build **secrets**; **network access** per user/team/environment — three modes (*allow all*, *default domains + allowlist*, *allowlist only*); default model / default repository / base branch; security toggles (hide agent summaries, external-channel summaries, team follow-ups: Disabled | Service-accounts-only | All — lateral-movement warning documented); team feature flags (**long-running agents**, **computer use** — enterprise). Recipes documented for Docker-in-Docker, Tailscale (userspace networking + ALL_PROXY/HTTP(S)_PROXY exports), Cloudflare Tunnel (with CF Access service-token secrets).

### 5.2 Handoff & automation surfaces

- From any CLI/editor session, prefix a message with **`&`** to push it to a Cloud Agent ([CLI overview](https://cursor.com/docs/cli/overview#cloud-agent-handoff)).
- **Automations**, Bugbot, Security Agents, Slack/Teams/Jira/Linear triggers — configured on dashboards, not local files.
- **`agent worker start`** — attach *your own machine* as a private cloud worker: `--auth-token-file`, repeatable `--worker-dir`, `--management-addr` (health/metrics), `--label`/`--labels-file`/`CURSOR_WORKER_LABELS_FILE`, `--idle-release-timeout`, `--pool`(+`--pool-name`, legacy alias `--single-use`), `--name`, `--data-dir`, `worker debug [--json]` preflight ([Parameters §Worker](https://cursor.com/docs/cli/reference/parameters#worker)).

### 5.3 Enterprise cloud-agent networking

Private connectivity (PrivateLink / Cloudflare Tunnel) covers Cloud Agents + Bugbot + webhooks to self-hosted Git (§3.5). Cloud-Agent VM egress can be restricted via the network-access modes above.

### 5.4 Privacy modes

Sources: [Privacy and data](https://cursor.com/help/security-and-privacy/privacy), [Privacy & Data Governance](https://cursor.com/docs/enterprise/privacy-and-data-governance)

- **Privacy Mode** = your code never trains Cursor or provider models. Toggle: Cursor Settings (`Cmd/Ctrl+Shift+J`) → General → Privacy Mode.
- Default/enforcement: **on by default for Enterprise**; Teams default on; admins can enforce org-wide (members can't disable) via dashboard; MDM hardening via Allowed Team IDs policy.
- **ZDR exceptions**: BYOK traffic (follows provider policy); select models needing provider-side retention (e.g. Claude Fable 5) are off by default and gated behind admin approval per team.
- Extras: US-only data residency (per-team, 10% uplift, eligible-model list, BYOK excluded), Customer Managed Encryption Keys (embeddings + Cloud Agent data), SOC 2 / DPA / subprocessor list at trust.cursor.com. Cloud Agents are the only feature that stores code (encrypted VM copies, deleted after completion) — skippable entirely if policy forbids it.

---

## 6. Model-list customization

### 6.1 Editor

- Picker in chat/agent panel; `Cmd/Ctrl+/` cycles; selection persists per conversation span ([Available models](https://cursor.com/help/models-and-usage/available-models)).
- Catalog spans Cursor first-party models (Grok 4.5/4.6, Composer 2.5) + third-party frontier models; two usage pools (Cursor Models / Other Models) ([Models & Pricing](https://cursor.com/docs/models-and-pricing)).
- **Adding models**: with an OpenAI key + "Override OpenAI Base URL", type arbitrary model IDs into Settings → Models ("+ Add model") — this is the mechanism for OpenAI-compatible providers/gateways (Groq, DashScope/Qwen, LLM Gateway etc.). Practical guidance from gateway docs: use canonical unprefixed IDs; prefixed forms like `custom/my-model` where the gateway expects them ([LLM Gateway](https://docs.llmgateway.io/guides/cursor), [community guide](https://github.com/bilal77511/custom-models-in-cursor-IDE)).
- Constraints/bugs: BYO OpenAI key serves standard non-reasoning chat models only; Cursor validates model-name strings and rejects some legitimate IDs (#158958); added custom models have been observed disappearing on Windows (#157704); don't re-add IDs that already exist natively (duplicate shadows native routing — [forum #162489](https://forum.cursor.com/t/add-custom-model/162489)).
- **Auto / Cursor Router**: routes across GPT‑5.5, Claude Opus 5, Grok 4.5, Claude Fable 5 (Grok 4.5 required; blocking both GPT-5.5+Opus disables router); team admins control model access/visibility from Team Settings → Models; blocked models skipped with allowlisted fallback (enterprise).
- **Subagents**: custom subagent frontmatter accepts `model:` (`inherit` by default) ([Available models FAQ](https://cursor.com/help/models-and-usage/available-models)).

### 6.2 CLI

- `/model auto | gpt-5 | sonnet-4-thinking` slash command; persisted in `cli-config.json` (`model`, `maxMode`, `hasChangedDefaultModel`) ([Configuration §Models](https://cursor.com/docs/cli/reference/configuration#models)).
- Flags: `--model <model>` per run, `--list-models`, `agent models` (account-scoped listing) ([Parameters](https://cursor.com/docs/cli/reference/parameters)).
- The CLI model namespace mirrors your account's picker — BYOK-added custom models are an IDE-side construct; no CLI-side model registry exists beyond `--model <id>` against account-available models.

### 6.3 Admin-level model governance

Enterprise: restrict model access per group, enforce visibility of routed models (hidden vs displayed), approve retention-bearing models, regional eligibility under data residency ([Model & Integration Management](https://cursor.com/docs/enterprise/model-and-integration-management), [Privacy & Data Governance](https://cursor.com/docs/enterprise/privacy-and-data-governance)).

---

## Quick verification checklist

```bash
agent --version && agent status          # binary + auth
echo $CURSOR_API_KEY | wc -c             # CI auth present?
ls ~/.cursor/cli-config.json .cursor/cli.json .cursor/mcp.json \
   ~/.cursor/mcp.json .cursor/rules .cursorignore .cursor/environment.json 2>/dev/null
agent --list-models                      # effective model list for this account
```
