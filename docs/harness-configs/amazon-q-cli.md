# Amazon Q Developer CLI — Configurable Options

> Compiled 2026-08-25. **Lineage note:** the Q Developer CLI (`q chat`) has been superseded by
> **Kiro CLI**; AWS documents the Q CLI paths as legacy in its migration table
> ([kiro.dev/docs/upgrade-guides/migrating-from-q](https://kiro.dev/docs/upgrade-guides/migrating-from-q/)),
> and the IDE plugins hit end-of-support April 30 2027 per
> [docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/what-is.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/what-is.html).
> Everything below describes the Q Developer CLI config surface as documented in the
> [amazon-q-developer-cli GitHub repo](https://github.com/aws/amazon-q-developer-cli) and the
> Amazon Q Developer User Guide.

---

## 1. Config files

### 1a. Settings: `~/.aws/amazonq/settings.json`

Managed through `q settings` (interactive TUI) or subcommands:

```bash
q settings                      # open settings UI
q settings get <key>
q settings set <key> <value>    # e.g. q settings set chat.defaultModel claude-sonnet-4
q settings delete <key>         # (repo release notes mention "make q settings delete user friendly")
```
Sources: [repo Releases](https://github.com/aws/amazon-q-developer-cli/releases);
[mychen76 walkthrough of `q settings` UI](https://mychen76.medium.com/amazon-q-developer-cli-retro-futuristic-way-to-build-aws-cloud-app-e8b86516e081).

Documented keys seen in official pages:
| Key | Purpose | Source |
|---|---|---|
| `mcp.initTimeout` | ms to wait for MCP server init before interaction | [MCP overview](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/qdev-mcp.html) |
| `chat.defaultModel` | default model for chat (also `/model`, agent `model` field) | [agent-format.md#model-field](https://github.com/aws/amazon-q-developer-cli/blob/main/docs/agent-format.md) |

⚠️ **Honest gap:** a dedicated "settings schema" page existed at
`docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/command-line-settings.html` (per the
[doc history entry, Apr 12 2025](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/doc-history.html))
but that URL **now redirects away** — the full key list lives in the Kiro CLI docs / `q settings` TUI.
Do not assume other key names without checking `q settings` locally.

### 1b. Rules: global vs project

Markdown rule/context files injected into every chat:

| Scope | Path |
|---|---|
| Global (user) rules | `~/.aws/amazonq/rules/*.md` |
| Project rules | `<project>/.amazonq/rules/*.md` |
| Project context file | `<project>/AmazonQ.md` |

Confirmed by the official migration table ("Rules / Steering … User: `~/.aws/amazonq/rules`,
Workspace: `.amazonq/rules"` → kiro.dev migrating-from-q) and by
[context-project-rules.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/context-project-rules.html)
("Creating project rules"). Project rules can also be pulled into an *agent* explicitly via the
agent's `resources` field: `"resources": ["file://.amazonq/rules/**/*.md"]`
([agent-format.md#resources-field](https://github.com/aws/amazon-q-developer-cli/blob/main/docs/agent-format.md)).

### 1c. Agent definitions: `.amazonq/cli-agents/*.json`

Global agents: `~/.aws/amazonq/cli-agents/<name>.json`; project agents: `<project>/.amazonq/cli-agents/<name>.json`.
Filename minus `.json` = agent name; select at launch with `q chat --agent <name>`.
Source: [agent-format.md](https://github.com/aws/amazon-q-developer-cli/blob/main/docs/agent-format.md) (full schema below) and
[qdev-mcp.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/qdev-mcp.html).

Schema fields ([agent-format.md](https://github.com/aws/amazon-q-developer-cli/blob/main/docs/agent-format.md)):

| Field | Notes |
|---|---|
| `name` | optional; derived from filename if absent |
| `description` | human-readable |
| `prompt` | system-prompt-style text; supports `file://./prompt.md` URIs (relative resolved against the agent JSON dir) |
| `mcpServers` | map name→`{command,args,env,timeout(ms, default 120000)}` or remote `{type:"http", url:...}` |
| `tools` | allow-list: built-ins by name (`fs_read`, `execute_bash`…), `@server` / `@server/tool` for MCP, `"*"` = all, `@builtin` = all native |
| `toolAliases` | rename colliding tools: `{"@github-mcp/get_issues": "github_issues"}` |
| `allowedTools` | auto-approved (no prompt); glob wildcards `*` `?`; exact beats pattern; **no bare `"*"`** — use patterns/server-level grants like `"@fetch"` or `"fs_*"`. Note: allowing overrides any restrictive `toolsSettings` patterns |
| `toolsSettings` | per-tool config, e.g. `{"fs_write": {"allowedPaths": ["src/**"]}, "use_aws": {"allowedServices": ["s3","lambda"]}}` |
| `resources` | `file://` globs: rules, READMEs, etc. |
| `hooks` | lifecycle commands (see §6) |
| `useLegacyMcpJson` | `true` merges legacy `~/.aws/amazonq/mcp.json` (global) + `cwd/.amazonq/mcp.json` (workspace) servers |
| `model` | model ID; must be one from `/model`; invalid ID falls back to default with warning |

Generate one with AI: run `/agent generate` inside a session.

Legacy MCP-only configs still honored when `useLegacyMcpJson: true`:
`~/.aws/amazonq/mcp.json` and `<project>/.amazonq/mcp.json`
([mcp-ide.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/mcp-ide.html),
[agent-format.md](https://github.com/aws/amazon-q-developer-cli/blob/main/docs/agent-format.md)).

---

## 2. Environment variables

| Variable | Effect on Q CLI | Source |
|---|---|---|
| `AWS_PROFILE` | selects named profile for IAM Identity Center auth; **caveat:** the built-in `use_aws` tool historically ignored it and used the `default` profile — fix tracked in issue #2088 / PR #2924 (agent-level profile configuration added Sep 2025) | [GH issue #2088](https://github.com/aws/amazon-q-developer-cli/issues/2088) |
| `AWS_DEFAULT_REGION` / `AWS_REGION` | region for AWS API calls made via tools/auth | standard AWS CLI semantics; workflow shown in issue #2088 |
| `Q_MCP_TIMEOUT` | override default per-request MCP timeout (120000 ms default) | repo env-var handling ("refactor: centralize environment variable access"); treat as version-dependent — verify with `q --help` |
| `Q_DEVELOPER_LOGIN_METHOD`? | not a documented public var — **no first-party `Q_*`/`AMAZONQ_*` config vars are formally documented** in the User Guide; the CLI reads standard `AWS_*` variables | absence confirmed across fetched docs |
| `HTTP(S)_PROXY`, `NO_PROXY` | corporate proxy support (see firewall/proxy page of the UG) | [firewall.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/firewall.html) |
| Standard `AWS_ACCESS_KEY_ID`/`SECRET`/`SESSION_TOKEN` | only relevant for tool-side AWS calls, not Q chat auth itself (auth is Builder ID / SSO token cache) | see §3 |

Honest assessment: unlike Codex/Claude Code, Q CLI does **not** expose a provider/model-endpoint
env-var layer. Configuration is file-based (§1) plus standard `AWS_*`.

## 3. Authentication

Two modes (`q login`):

1. **AWS Builder ID** — free tier, no AWS account needed. Token cached under `~/.aws/sso/cache/`.
   Used for individual/free usage of the CLI.
2. **IAM Identity Center (SSO)** — required for Pro subscription features. Managed via
   `q login` with IdC start URL + region, or via an existing `AWS_PROFILE` configured with
   `aws sso login`. Pro entitlement comes from the IdC principal having the
   `AmazonQDeveloperAccess` permission set / managed policy.

- Region matters twice: the **IdC/Q service region** chosen at login, and the
  **`AWS_DEFAULT_REGION`/profile region** for tool calls (`use_aws`). These are independent.
- Sources: [getting-started-q-dev.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/getting-started-q-dev.html),
  [what-is.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/what-is.html),
  workflow evidence in [issue #2088](https://github.com/aws/amazon-q-developer-cli/issues/2088).

## 4. Third-party models — honest assessment

- Chat models are **Amazon-Bedrock-backed only**: Claude family and Amazon Nova are selectable via
  `/model` in-session or the agent `model` field
  ([doc history: "Model selection for chat on the command line", June 5 2025](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/doc-history.html);
  [agent-format.md#model-field](https://github.com/aws/amazon-q-developer-cli/blob/main/docs/agent-format.md)).
- The product itself states it is "[p]owered by Amazon Bedrock"
  ([what-is.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/what-is.html)).
- **No BYOK, no custom base URL, no OpenAI/Gemini/local endpoints.** There is no
  `model_providers` block, no gateway support, no `ANTHROPIC_BASE_URL` equivalent anywhere in
  the agent schema or settings. If you need BYO models, this harness cannot do it; the closest
  lever is attaching third-party *tools* via MCP, not models. (The docs even frame MCP's benefit
  as switching LLM providers server-side within Bedrock's set.)
- Model IDs must match the Q model service list (`/model`); unknown IDs silently fall back to
  the default model with a warning — so typos degrade rather than error.

## 5. Multi-instance wrappers

There is no config-home relocation env var (no `Q_HOME`), so isolation levers are:

1. **Auth/account switching:** `AWS_PROFILE=<profile>` + `aws sso login --profile <p>` before
   launching; note the `use_aws` tool caveat above (#2088/#2924) — recent builds add agent-level
   profile config, otherwise the tool may pin `default`.
2. **Context isolation:** distinct agents per instance via `--agent` (different `mcpServers`,
   `toolsSettings`, `resources`, `model` per agent JSON).
3. **Project scoping:** project `.amazonq/` dirs naturally isolate rules/agents/MCP per repo.

Wrapper script example:

```bash
#!/usr/bin/env bash
# q-work: launch Q CLI pinned to a work account + work agent context
export AWS_PROFILE=work-prod            # IdC/SSO profile (aws sso login --profile work-prod first)
export AWS_DEFAULT_REGION=us-east-1     # region for use_aws tool calls
exec q chat --agent aws-expert "$@"     # agent defined in ~/.aws/amazonq/cli-agents/aws-expert.json

# Companion: q-personal with AWS_PROFILE=personal and --agent rust-specialist
```

Limitation: two simultaneous instances share `~/.aws/amazonq/settings.json` and the SSO token
cache, so you can't have fully divergent *global* settings concurrently — diverge via agents
instead. Session state is per-terminal, which does allow side-by-side instances safely.

## 6. MCP schema, hooks, permissions

### MCP servers (inside agent JSON `mcpServers`)
- Local: `{"<name>": {"command": "...", "args": [...], "env": {...}, "timeout": 120000}}`
- Remote HTTP: `{"<name>": {"type": "http", "url": "https://host/mcp"}}` — OAuth or open;
  authenticate via `/mcp` in-session (browser flow)
  ([command-line-mcp-config-CLI.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/command-line-mcp-config-CLI.html)).
- Manage: `q mcp add|remove|list|import|status` (`--args` supports escaped commas or JSON array).
- Servers load in background; check readiness with `/tools`; init timeout via
  `q settings mcp.initTimeout`.
- Tool naming: `@server` / `@server/tool`; prompts exposed as `/prompts`, invoked `@prompt-name args`.

### Hooks ([agent-format.md#hooks-field](https://github.com/aws/amazon-q-developer-cli/blob/main/docs/agent-format.md))
Triggers: `agentSpawn`, `userPromptSubmit`, `preToolUse` (can **block** execution), `postToolUse`,
`stop`. Each hook: `{"matcher": "<tool-glob, pre/postToolUse only>", "command": "<shell cmd>"}`;
hook receives tool input on stdin (pipe-and-log pattern shown in the docs example).

```json
"hooks": {
  "preToolUse": [
    {"matcher": "execute_bash",
     "command": "{ echo \"$(date) - Bash:\"; cat; echo; } >> /tmp/bash_audit_log"}
  ],
  "postToolUse": [{"matcher": "fs_write", "command": "cargo fmt --all"}]
}
```
(Context hooks were deprecated Sept 2025 in favor of these agent hooks — auto-migrated;
[doc-history.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/doc-history.html).)

### Permission model
Three tiers surfaced in `/tools`: **auto-approved** (listed in `allowedTools`), **requires
approval**, and **dangerous**. `allowedTools` supports globs (`fs_*`, `@server/read_*`,
`@git-*/*`) but never a bare `"*"`; `toolsSettings` narrows behavior (`fs_write.allowedPaths`,
`use_aws.allowedServices`) but is overridden where a tool also appears in `allowedTools`
([agent-format.md](https://github.com/aws/amazon-q-developer-cli/blob/main/docs/agent-format.md)).
Enterprise governance: MCP server access control exists at the org level
([mcp-governance.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/mcp-governance.html));
security model details in
[command-line-mcp-security.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/command-line-mcp-security.html).

---
### Unverified / flagged
- Full enumerated `settings.json` key list: source page retired from the AWS guide (redirects);
  verify live with `q settings` / Kiro CLI docs.
- Exact `Q_*` env-var names beyond those cited: not officially documented; repo refactored env
  access centrally but no public table exists — don't trust blog-post var names without testing.
