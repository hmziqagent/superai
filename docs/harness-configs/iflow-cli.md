# iFlow CLI — Configurable Options Reference

> **⚠️ Project status:** iFlow CLI's README carries a shutdown notice — *"iFlow CLI will be shutting down on April 17, 2026 (UTC+8)"* — with migration guidance at [vibex.iflow.cn/t/topic/4819](https://vibex.iflow.cn/t/topic/4819). Treat everything below as documentation of the final state ([README.md](https://github.com/iflow-ai/iflow-cli/blob/main/README.md)).
>
> **Lineage:** iFlow CLI is listed in [Awesome Gemini CLI](README badge), a fork of Google's Gemini CLI. Its settings schema largely maps onto Gemini CLI's (`~/.gemini/settings.json` → `~/.iflow/settings.json`), which explains most structural choices below. Docs live in-repo under [`docs_en/`](https://github.com/iflow-ai/iflow-cli/tree/main/docs_en) (`docs_cn/` mirrors them in Chinese).

---

## 1. Config files: locations & schema

### Settings files (`settings.json`)

Three tiers, documented in [`docs_en/configuration/settings.md`](https://github.com/iflow-ai/iflow-cli/blob/main/docs_en/configuration/settings.md):

| Tier | Location | Scope |
|---|---|---|
| User | `~/.iflow/settings.json` | Personal defaults, all projects |
| Project | `<repo>/.iflow/settings.json` | Current project only |
| System | Linux `/etc/iflow-cli/settings.json`; Windows `C:\ProgramData\iflow-cli\settings.json`; macOS `/Library/Application Support/iFlowCli/settings.json` | All users (admin-managed); path overridable via **`IFLOW_CLI_SYSTEM_SETTINGS_PATH`** env var |

**Resolution priority** (high→low, from the same doc): 1) CLI flags (`--model`, `--yolo`, …) → 2) `IFLOW_`-prefixed env vars → 3) system settings file → 4) workspace `.iflow/settings.json` → 5) user `~/.iflow/settings.json` → 6) built-in defaults. *(The doc's own summary list earlier in the page ranks project above system — the two lists disagree; the detailed chain shown here is the operative one.)*

Inside any `settings.json`, values may reference environment variables as `"$VAR_NAME"` / `"${VAR_NAME}"`, resolved at load time (e.g. `"apiKey": "$MY_API_TOKEN"`).

### Key `settings.json` fields (verified against settings.md)

Connection/auth:
- **`selectedAuthType`** (string) — `"iflow"` (iFlow/XinLiu native auth, default) or `"openai-compatible"` (any OpenAI-protocol provider). Doc example also shows `"api_key"`.
- **`apiKey`**, **`baseUrl`**, **`modelName`** — endpoint triple; e.g. `"baseUrl": "https://apis.iflow.cn/v1"`, `"modelName": "Qwen3-Coder"`.
- **`searchApiKey`** — key for the built-in web-search feature (shown in README's sample file).

Behavior/UI: `theme`, `vimMode`, `hideTips`, `hideBanner`, `preferredEditor`, `autoAccept` (auto-approve read-only-safe tool calls), `showMemoryUsage`, `maxSessionTurns` (-1 = unlimited), `tokensLimit` (context window, default 128000), `compressionTokenThreshold` (default 0.8), `shellTimeout` (ms, default 120000), `useRipgrep` (default true), `skipNextSpeakerCheck`, `disableAutoUpdate`, `disableTelemetry`, `usageStatisticsEnabled`.

Tools/safety: `coreTools`, `excludeTools` (see §5), `sandbox` (bool | `"docker"`), `fileFiltering` `{respectGitIgnore, enableRecursiveFileSearch}`, `mcpServers` (§5), `allowMCPServers`/`excludeMCPServers` (§5), `checkpointing.enabled`, `summarizeToolOutput`, `toolDiscoveryCommand`/`toolCallCommand`.

Context/memory: `contextFileName` (string or array, default `IFLOW.md`), `memoryDiscoveryMaxDirs` (subdirectory scan breadth, default 200), `bugCommand.urlTemplate`.

Other runtime dirs/files under `~/.iflow/`: `tmp/<project_hash>/shell_history` (per-project shell history); `agents/` (global subagents, §5); `mcp/config.json` (MCP doc names this as a global MCP config location, §5). A repo-level ignore file is documented at [`docs_en/configuration/iflowignore.md`](https://github.com/iflow-ai/iflow-cli/blob/main/docs_en/configuration/iflowignore.md) (the `.geminiignore` analogue — not fetched here; consult that file for its exact semantics).

### Context file (IFLOW.md)

Documented in [`docs_en/configuration/iflow.md`](https://github.com/iflow-ai/iflow-cli/blob/main/docs_en/configuration/iflow.md) and the Context Files section of settings.md:

- Default filename **`IFLOW.md`**; rename/rename-list via `"contextFileName": ["IFLOW.md", "AGENTS.md", "CONTEXT.md"]`.
- Hierarchy: global `~/.iflow/IFLOW.md` < project-root `IFLOW.md` (searched upward from cwd until a `.git` dir or `$HOME`) < subdirectory `IFLOW.md` files (scanned down, ≤ `memoryDiscoveryMaxDirs`). More-specific layers override/supplement; inspect the merge with `/memory show`, reload with `/memory refresh`; `/memory add` persists facts via save_memory.
- Modular imports with `@./path/file.md`, `@../`, `@/absolute/path`; circular-import detection, depth limit (5), path validation.
- `/init` scans the codebase and generates a populated `IFLOW.md`. Env var interpolation (`$VAR`) also works inside IFLOW.md.

## 2. Environment variables

From the "Environment Variable Configuration" section of [settings.md](https://github.com/iflow-ai/iflow-cli/blob/main/docs_en/configuration/settings.md):

- **Every key in `~/.iflow/settings.json` can be set via env vars with the `IFLOW`/`iflow` prefix.** Four naming forms are recognized, in priority order:
  1. `IFLOW_` + camelCase (recommended): `IFLOW_apiKey`, `IFLOW_baseUrl`, `IFLOW_modelName`
  2. `IFLOW_` + UPPER_SNAKE: `IFLOW_API_KEY`, `IFLOW_BASE_URL`, `IFLOW_MODEL_NAME`
  3. lowercase-prefix camelCase: `iflow_apiKey`…
  4. lowercase-prefix SNAKE: `iflow_API_KEY`…
- Confirmed examples beyond the credential trio: `IFLOW_vimMode`, `IFLOW_showMemoryUsage`, `IFLOW_maxSessionTurns`, `IFLOW_coreTools` (comma list, e.g. `"read,write,shell,grep"`), `IFLOW_theme`.
- Special-purpose: **`IFLOW_CLI_SYSTEM_SETTINGS_PATH`** relocates the system-tier settings file.
- Legacy/back-compat: pre-existing upstream variables such as `GEMINI_API_KEY` still work (migration note in settings.md).
- Validation: valid env config auto-selects the iFlow auth type; invalid config yields explicit errors. Docs recommend `.env` files locally and secret managers/CI secrets in production.

## 3. Auth & providers

From [README §Authentication](https://github.com/iflow-ai/iflow-cli/blob/main/README.md) + settings.md:

1. **Native iFlow account (recommended):** first-run login picker option 1 opens browser-based iFlow OAuth; completing it grants free access to hosted models (Kimi K2, Qwen3-Coder, DeepSeek v3, GLM, MiniMax, etc. via the [iFlow open platform](https://platform.iflow.cn/docs/api-mode)). Stored auth type is `"selectedAuthType": "iflow"`. (Exact OAuth token cache filename isn't stated in these docs; Gemini-fork lineage implies it lands under `~/.iflow/` — unverified.)
2. **API-key mode (headless servers):** register at iflow.cn → profile settings / [iflow.cn/?open=setting](https://iflow.cn/?open=setting) → click **Reset** to mint a key → paste into the terminal prompt, or set it yourself:
   ```json
   { "selectedAuthType": "iflow", "apiKey": "<key>", "searchApiKey": "<same key>",
     "baseUrl": "https://apis.iflow.cn/v1", "modelName": "Qwen3-Coder" }
   ```
3. **Any OpenAI-compatible endpoint (custom base URL pattern):** set `"selectedAuthType": "openai-compatible"` and provide `apiKey` + `baseUrl` + `modelName`. README's canonical demo:
   ```json
   {
     "theme": "Default",
     "selectedAuthType": "iflow",
     "apiKey": "your iflow key",
     "baseUrl": "https://apis.iflow.cn/v1",
     "modelName": "Qwen3-Coder",
     "searchApiKey": "your iflow key"
   }
   ```
   Pure-env variant (no file edits): `IFLOW_apiKey` / `IFLOW_baseUrl` / `IFLOW_modelName`, e.g. `baseUrl=https://api.openai.com/v1`, `modelName=gpt-4` — the docs' own examples show pointing at OpenAI or "your-custom-api.com/v1", so any OpenAI-protocol gateway (CN ecosystem models behind aggregators, vLLM, LiteLLM…) fits the same three-key pattern. Session override via `iflow --model <name>`. There is no Anthropic-protocol provider type documented — OpenAI-compatible only.

## 4. Multi-instance wrappers

There is **no dedicated config-home relocation variable** for `~/.iflow` (nothing like `GEMINI_CLI_HOME` appears in the docs). Two documented mechanisms make clean per-instance isolation possible anyway:

- **Pure env-var instances** (simplest): `IFLOW_*` env vars outrank *all* settings files (only CLI flags beat them), so each wrapper just exports its full connection profile.
- **System-settings-path instances**: `IFLOW_CLI_SYSTEM_SETTINGS_PATH` points the highest-priority *file* tier at an instance-specific JSON; useful when you want many keys shared from a file rather than exported. (OAuth-token state stays in the shared `~/.iflow`, so mixing native-iFlow-OAuth instances on one user account is the weak spot — prefer distinct OS users or API-key mode for true separation.)

```bash
#!/usr/bin/env bash
# ~/bin/iflow-free — iFlow platform account (free hosted models)
exec env \
  IFLOW_selectedAuthType="iflow" \
  IFLOW_apiKey="${IFLOW_FREE_KEY:?set IFLOW_FREE_KEY}" \
  IFLOW_baseUrl="https://apis.iflow.cn/v1" \
  IFLOW_modelName="${IFLOW_FREE_MODEL:-Qwen3-Coder}" \
  iflow "$@"
```

```bash
#!/usr/bin/env bash
# ~/bin/iflow-ds — second instance on a third-party OpenAI-compatible endpoint
exec env \
  IFLOW_selectedAuthType="openai-compatible" \
  IFLOW_apiKey="${DEEPSEEK_API_KEY:?set DEEPSEEK_API_KEY}" \
  IFLOW_baseUrl="https://api.deepseek.com/v1" \
  IFLOW_modelName="deepseek-chat" \
  iflow "$@"
```

File-based variant (keeps long config out of env):

```bash
# ~/.config/iflow/work.settings.json holds the work profile; then:
#!/usr/bin/env bash
exec env IFLOW_CLI_SYSTEM_SETTINGS_PATH="$HOME/.config/iflow/work.settings.json" iflow "$@"
```

Project-scoped alternative: drop a `.iflow/settings.json` per repo — it beats the user file but loses to both env vars and the system path.

## 5. Subagents, file permissions, MCP

### Subagents
Source: [`docs_en/examples/subagent.md`](https://github.com/iflow-ai/iflow-cli/blob/main/docs_en/examples/subagent.md).

- Custom agents are **Markdown files with YAML frontmatter**: project scope in `<repo>/.iflow/agents/*.md`, user/global scope in `~/.iflow/agents/*.md`. Required frontmatter: `agentType`, `systemPrompt`, `whenToUse`; optional: `model`, `allowedTools` (string array, `["*"]` = all), `allowedMcps`, `isInheritTools` (default true — inherits parent tools plus `allowedTools`; false = only listed tools), `isInheritMcps` (same pattern for MCP servers), `proactive`, `color`, `name`, `description`.
- Invoke with quick-call `$<agent-type> <task>` (autocomplete after `$`) or automatically via the Task tool; manage with `/agents list|online|install|refresh` and `iflow agent add <name> --scope project|global | list | get | remove`. Marketplace install wizard supports tool-permission and MCP-access selection per agent.
- Model compatibility is auto-checked per agent, with interactive or YOLO auto-switching to a recommended fallback model.

### Tool / file permissions
Sources: [settings.md](https://github.com/iflow-ai/iflow-cli/blob/main/docs_en/configuration/settings.md), README feature table.

- Approval modes (README): `default` (no permissions without approval) → `autoAccept: true` (safe/read-only ops auto-approved) → "accepting edits" (file edits only) → plan mode → **YOLO** (`--yolo`: every tool call auto-approved).
- **`coreTools`** whitelist / **`excludeTools`** blacklist of built-in tools, with command-granularity syntax: `"coreTools": ["ReadFileTool", "GlobTool", "ShellTool(ls -l)"]` allows only `ls -l`; `"excludeTools": ["ShellTool(rm -rf)"]` blocks that command. Docs warn exclude-side matching is naive string matching, **"is not a security mechanism"**, and prefer `coreTools` whitelisting.
- File discovery respects git: `fileFiltering.respectGitIgnore` (default true) and `enableRecursiveFileSearch` (default true); plus the repo-level `.iflowignore`-style file (see §1).
- Optional Docker sandboxing: `"sandbox": true | "docker"` or `-s`/`--sandbox-image` flags; custom sandbox assets live in project `.iflow/` (`sandbox.Dockerfile`, `sandbox-macos-custom.sb`). Extra workspace roots: `--include-directories`/`--add-dir` (max 5 dirs).

### MCP servers
Sources: [`docs_en/examples/mcp.md`](https://github.com/iflow-ai/iflow-cli/blob/main/docs_en/examples/mcp.md) + `mcpServers` entry in settings.md.

- Declarative config in `mcpServers` (same object in user or project `settings.json`): per-server `command` (required), `args`, `env` (supports `$VAR` refs), `cwd`, `timeout` (ms), `trust` (bypasses tool-call confirmations), `includeTools`/`excludeTools` whitelists (exclude wins on conflict). Name collisions across servers become `serverAlias__actualToolName`.
- Imperative management: `iflow mcp add-json <name> '<json>' [--scope user]`, `iflow mcp add <name> <cmd> [args...]`, `iflow mcp add --transport sse|http <name> <url> [--header "Authorization: Bearer ..."]`, `iflow mcp list|get|remove`, in-session `/mcp list|refresh|online` (marketplace browse/install; project or user scope).
- Config locations named by the MCP doc: global/user `~/.iflow/settings.json` (also `~/.iflow/mcp/config.json`) and project `.iflow/settings.json` (also `{project}/.iflow/mcp.json`); Claude Desktop configs are read automatically.
- Fleet limits: top-level `allowMCPServers` / `excludeMCPServers` filter which configured servers connect (ignored if `--allowed-mcp-server-names` flag is passed); admins should pin `mcpServers` at the system-settings tier since name filtering is bypassable string matching.

---
*Compiled 2026-08-25 from the repo's final-state docs: [README.md](https://github.com/iflow-ai/iflow-cli/blob/main/README.md), [docs_en/configuration/settings.md](https://github.com/iflow-ai/iflow-cli/blob/main/docs_en/configuration/settings.md), [docs_en/configuration/iflow.md](https://github.com/iflow-ai/iflow-cli/blob/main/docs_en/configuration/iflow.md), [docs_en/examples/mcp.md](https://github.com/iflow-ai/iflow-cli/blob/main/docs_en/examples/mcp.md), [docs_en/examples/subagent.md](https://github.com/iflow-ai/iflow-cli/blob/main/docs_en/examples/subagent.md). Items marked unverified were inferred from Gemini-fork lineage, not confirmed by fetched sources.*
