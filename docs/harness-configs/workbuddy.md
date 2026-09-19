# WorkBuddy / CodeBuddy CLI (Tencent) — Complete Configuration Reference

**Compiled:** 2026-09-08 · **Status:** WorkBuddy desktop app active (changelog 5.2.7, 2026-07-17)[16]; the shared `cbc` CLI (`@tencent-ai/codebuddy-code` v2.147.0)[4] is the only documented programmatic surface. There is **no public workbuddy.ai HTTP API**[15] and **no headless mode of the desktop app**[16] — mutation targets the CLI config tree only.

WorkBuddy (www.workbuddy.ai) is Tencent's desktop AI agent for everyday office work: describe a task in one sentence and it autonomously plans and executes it (reports, slides, data analysis, local file ops)[1]. It shares the CodeBuddy product family and config tree with Tencent's terminal agent "CodeBuddy Code" (npm `@tencent-ai/codebuddy-code`, binaries `codebuddy`/`cbc`, install `npm i -g @tencent-ai/codebuddy-code`, Node >= 18.20/22+, zero deps)[4][5]. Both read `~/.codebuddy/`[5]. Tencent's own eval framework **workbuddy-bench** defines a "harness" as an agent CLI in a Docker sandbox and drives cbc by pre-writing `~/.codebuddy/models.json` + `settings.json` then running headless[9][10] — that is the canonical integration pattern for this surface. Do not confuse with worksbuddy.ai / workbuddy.io / workbuddy.com (field service, AU) — unrelated products.

---

## 1. Config: file locations & schema

### 1.1 Config dir and file map

| Path | Purpose | Source |
|---|---|---|
| `~/.codebuddy/` (Win `%USERPROFILE%\.codebuddy`) | User config root; **override with `CODEBUDDY_CONFIG_DIR`** | [5] |
| `~/.codebuddy/models.json` | Custom OpenAI-compatible model catalog (shared by WorkBuddy app and cbc CLI) | [3] |
| `<project>/.codebuddy/models.json` | Project-level model overrides (overrides user level) | [3] |
| `~/.codebuddy/settings.json` | User settings (permissions, thinking, autocompact) | [5] |
| `<project>/.codebuddy/settings.json` | Project settings (commit to VCS) | [5] |
| `<project>/.codebuddy/settings.local.json` | Local settings (gitignored) | [5] |
| `~/.codebuddy/.mcp.json` | Recommended user MCP config (JSONC) | [6] |
| `~/.codebuddy/mcp.json`, `~/.codebuddy.json` | Deprecated / legacy MCP locations (first-existing wins per scope) | [6] |
| `<project>/.mcp.json` | Project MCP config (recommended); `<project>/mcp.json` deprecated | [6] |

Settings precedence: **CLI args > local > project > user**[5]. MCP conflict resolution: **local > project > user**; "local" scope lives in the user file at JSON Pointer `/projects/<workspace_path>`[6]. Credentials are stored in the OS keychain/keyring/credential manager by the interactive CLI[8] — detectable, not writable.

### 1.2 `models.json` schema

Format: strict JSON, **UTF-8 without BOM**; a BOM or syntax error makes the file silently not load[3]. Top level: `{"models": [...], "availableModels": [ids]}`[3].

Model entry fields[3][10]:

| Field | Meaning |
|---|---|
| `id` | Model identifier used by `--model` / `availableModels` |
| `name` | Display name |
| `vendor` | Provider label (e.g. `"OpenAI"`, `"DeepSeek"`) |
| `url` | Full chat-completions URL; **must end `/v1/chat/completions`** (standard protocol) |
| `apiKey` | Key or `${ENV_VAR}` expansion (secrets stay out of the file) |
| `maxInputTokens`, `maxOutputTokens` | Context caps; documented as numbers[3], but Tencent's own bench preset writes them as `${ENV}` strings[10] — accept both |
| `supportsToolCall`, `supportsImages`, `supportsReasoning` | Capability flags |
| `relatedModels` | Optional `{lite, reasoning}` companions |

Errors: 401 bad apiKey, 404 wrong model id, config-not-read means JSON syntax/BOM/path issues[3]. Full app restart required after `setx` env changes[3].

### 1.3 `settings.json` schema (partially documented — flag)

No single published schema; fields verified across Tencent's bench preset and CLI docs[6][7][8][10]:

| Field | Meaning | Source |
|---|---|---|
| `permissions.defaultMode` | e.g. `"bypassPermissions"` (non-interactive runs) | [10] |
| `permissions.deny` | Tool deny list (e.g. `WebSearch`, `AskUserQuestion`, `Workflow`, `ComputerUse`, `WaitForMcpServers`, `EnterPlanMode`) | [10] |
| `alwaysThinkingEnabled`, `autoCompactEnabled`, `autoUpdates` | Behaviour toggles | [10] |
| `includeCoAuthoredBy`, `promptSuggestionEnabled`, `cleanupPeriodDays`, `enableAllProjectMcpServers` | Misc documented keys | [6][10] |
| `apiKeyHelper` | Script that prints a bearer token (enterprise OAuth; 30 s timeout, 5 min cache) | [8] |
| `env` | Extra env vars cbc applies to itself (`CODEBUDDY_AUTH_TOKEN` can live here) | [8] |
| `endpoint` | Dedicated/self-hosted edition endpoint | [8] |
| `enabledMcpjsonServers` | Allowlist of project `.mcp.json` servers | [6] |

⚠️ **Gap:** the complete `settings.json` key set is not published anywhere; the list above is verified-only. Preserve unmodelled keys on write; unknown keys are expected.

### 1.4 `.mcp.json` schema

JSONC (comments + trailing commas allowed). Shape `{"mcpServers": {name: {...}}}`[6]. Server types:

- **stdio**: `{type, command, args, env, defer_loading, tools}` — `type` optional when `command` present
- **http** / **sse**: `{type, url, headers, ...}` — `type` optional when `url` present (defaults http)

`${VAR}` and `${VAR:-default}` expansion in `command`/`args`/`env` values and in `url`/`headers` values[6]. Managed via CLI: `cbc mcp add --scope user <name> -- <cmd> <args…>`, `cbc mcp add-json --scope user <name> '<json>'`, `--transport sse|http`, `cbc mcp list|get|remove`[6]. Headless enablement: `--settings '{"enableAllProjectMcpServers": true}'` or `enabledMcpjsonServers`[6]. `MAX_MCP_OUTPUT_TOKENS` default 20000; oversized output goes to a session `tool-results/` dir (`CODEBUDDY_DISABLE_MCP_LARGE_OUTPUT_FILES=1` to disable)[6]. MCP tools are exposed as `mcp__<server>__<tool>`; permission rules `deny > ask > allow` with wildcards `mcp__server`, `mcp__server__tool`, `mcp__*` (deny/ask only), case-insensitive, `-`/`.` treated as `_`[6]. MCP prompts become `/server:prompt` slash commands[6].

---

## 2. Environment variables

**Auth priority: `CODEBUDDY_AUTH_TOKEN` > `apiKeyHelper` (settings.json) > `CODEBUDDY_API_KEY`**[8].

| Var | Purpose | Source |
|---|---|---|
| `CODEBUDDY_AUTH_TOKEN` | Raw bearer token for CI | [8] |
| `CODEBUDDY_API_KEY` | Individual key from codebuddy.ai/profile/keys (intl), copilot.tencent.com/profile (CN) | [8] |
| `CODEBUDDY_BASE_URL` | Base URL for third-party models (e.g. https://openrouter.ai/api/v1) | [8] |
| `CODEBUDDY_INTERNET_ENVIRONMENT` | Edition: unset/`public` (intl), `internal` (CN), `ioa`, `cloudhosted`, `selfhosted` | [8] |
| `CODEBUDDY_CONFIG_DIR` | Relocates the whole `~/.codebuddy` config root | [5] |
| `CBC_BASE_URL` / `CBC_API_KEY` | Bench-side names for model endpoint/key (written into `models.json`) | [10] |
| `CBC_MAX_TURNS` | Harness-side default for `--max-turns` (cbc arg, not a cbc env) | [10] |
| `CODEBUDDY_AUTO_COMPACT_WINDOW` | Absolute autocompact window; cbc >= 2.103.4; clamped [100k, 1M] | [10] |
| `CODEBUDDY_AUTOCOMPACT_PCT_OVERRIDE` | Percentage override; cbc < 2.103.4; inert on newer | [10] |
| `CODEBUDDY_IS_SANDBOX` | `=1` suppresses all prompts incl. HIGH/CRITICAL (high risk) | [7][10] |
| `DISABLE_AUTOUPDATER` | `=1` disables auto-update (`cbc update` manual) | [5] |
| `CODEBUDDY_CODE_DISABLE_BACKGROUND_TASKS` | `=1` disables task_started/progress events | [7] |
| `CODEBUDDY_COMPUTER_USE_ENABLED`, `CODEBUDDY_WAIT_FOR_MCP_SERVERS_ENABLED`, `CODEBUDDY_CODE_DISABLE_SESSION_TITLE_REFRESH` | Bench-pinned behaviour toggles (per-version presets) | [10] |

Enterprise OAuth: `POST https://copilot.tencent.com/oauth2/token` with `grant_type=client_credentials` yields the token an `apiKeyHelper` script prints; helper TTL via `CODEBUDDY_CODE_API_KEY_HELPER_TTL_MS`[8].

---

## 3. Models: BYO endpoints

WorkBuddy/cbc consume any OpenAI-compatible endpoint via `models.json`[3][10]:

- **Custom providers**: enter id/url/apiKey per model (DeepSeek example: id `deepseek-v4-pro`, url `https://api.deepseek.com/v1/chat/completions`, `apiKey "${DEEPSEEK_API_KEY}"`, `maxInputTokens 128000`, `maxOutputTokens 8192`)[3].
- **Ollama local**: default port 11434, OpenAI-compatible interface[2].
- **Tencent Cloud Token Plan**: endpoint `https://tokenhub-intl.tencentcloudmaas.com/plan/v3/chat/completions` + API key + model id from the subscribed package[14].
- **Desktop app UI**: Settings → Model dialog (add/edit/delete, auto-saved; legacy `~/.codebuddy/models.json` still honoured and visible after upgrade); "Custom Protocol" toggle ON sends directly to the entered URL and skips validation, OFF uses the standard `/chat/completions` path with URL auto-completion[2]. Auto Mode picks models per task[2].

Protocol is standard OpenAI `POST <url>` with Bearer auth[3]. The bench additionally supports an Anthropic protocol (`/v1/messages`) via its local proxy bridge — that is proxy-side, not a cbc `models.json` capability[9].

---

## 4. Headless invocation & bench harness

Non-interactive: `cbc -p "<prompt>"`; **`-y`/`--dangerously-skip-permissions` required** or file I/O/commands/network are blocked; HIGH/CRITICAL commands may still prompt unless `CODEBUDDY_IS_SANDBOX=1`[7]. Flags: `--output-format text|json|stream-json`, `--resume <session-id>`/`-r`, `--continue`/`-c`, `--verbose`, `--append-system-prompt`, `--allowedTools`/`--disallowedTools` (e.g. `"Bash,Read"` or `"Bash(npm install)"`), `--settings '<json>'|<file>`, `--setting-sources user,project,local`, `--mcp-config <file>`, `--json-schema` (structured output with `--output-format json`), `--input-format=stream-json`, `--model`, `--max-turns`; stdin piping works (`echo "..." | cbc -p`)[7][10].

Output events: init system message → user/assistant messages → final `result` system message with stats; JSON output carries `session_id`, `usage`, `result`, `structured_output`[7]. stream-json events mirror Claude Code: `system/init`, assistant messages (`thinking`/`text`/`tool_use` + usage), final `result` event (authoritative usage, cost, `is_error`, `errors`)[10]. Canonical invocation used by Tencent's own bench[10]:

```
cbc -p --output-format stream-json -y --model <id> --max-turns <N> -- "<instruction>"
```

workbuddy-bench[9] wraps this in Docker (Harbor runtime): pins the npm version (`@tencent-ai/codebuddy-code@${CBC_VERSION}`), base64-writes `~/.codebuddy/models.json` (single entry from a `${ENV}`-templated preset) and `settings.json` (deterministic preset + `alwaysThinkingEnabled` overlay), forces `HOME` to the passwd home, and parses stream-json into a trajectory with token/cost usage[10]. Compaction era boundary: cbc **< 2.103.4** carries the window via `models.json` `maxInputTokens` (pct via `CODEBUDDY_AUTOCOMPACT_PCT_OVERRIDE`); **>= 2.103.4** omits `maxInputTokens` and uses `CODEBUDDY_AUTO_COMPACT_WINDOW` (absolute, clamped 100k–1M)[10]. Bench config is 4-layer deep-merged YAML (bench → dataset → harness → model → job, v3 schemas) with secrets referenced by env-var name only[9]. ⚠️ workbuddy-bench is under a custom "Tencent" license, not standard OSS — verify terms before vendoring code[9].

---

## 5. Multi-instance wrappers

Two levers give per-instance isolation[5][8]:

- **State isolation**: `CODEBUDDY_CONFIG_DIR` relocates the entire config tree (models, settings, MCP, sessions) per instance[5]. Project files (`.codebuddy/`, `.mcp.json`) travel with the checkout — `--setting-sources user` / explicit `--settings` keep hermetic runs unaffected by repo-local files[7].
- **Credential switching**: `CODEBUDDY_AUTH_TOKEN` (raw token) or `CODEBUDDY_API_KEY` (+ `CODEBUDDY_BASE_URL` for third-party endpoints) exported per process[8].
- **No updater races**: `DISABLE_AUTOUPDATER=1`[5].
- **Prompt-free runs**: `-y` plus `permissions.defaultMode: bypassPermissions` in settings; `CODEBUDDY_IS_SANDBOX=1` only for disposable sandboxes[7][10].

### Wrapper script example

```bash
#!/usr/bin/env bash
# cbc-wrapper.sh — isolated, credentialed WorkBuddy/cbc CLI invocations
# Usage: cbc-wrapper.sh <profile> [prompt...]
set -euo pipefail

PROFILE="$1"; shift

case "$PROFILE" in
  subscription)   # Tencent key, isolated state
    export CODEBUDDY_API_KEY="${CODEBUDDY_TEST_KEY:?unset}"
    ;;
  openai)         # third-party OpenAI-compatible endpoint
    export CODEBUDDY_API_KEY="${OPENAI_TEST_KEY:?unset}"
    export CODEBUDDY_BASE_URL="https://api.openai.com/v1"
    ;;
esac

# Isolate everything cbc writes per instance/profile
export CODEBUDDY_CONFIG_DIR="$HOME/.codebuddy-instances/$PROFILE"
mkdir -p "$CODEBUDDY_CONFIG_DIR"

# Headless run: stream-json trajectory, no prompts, no updater races
exec cbc -p --output-format stream-json -y \
  --setting-sources user \
  --model test-model \
  -- "$@"
```

Parallel-safe checklist: distinct `CODEBUDDY_CONFIG_DIR` per concurrent instance, distinct keys via env per instance, `DISABLE_AUTOUPDATER=1`, unique `--resume` session ids, and Docker sandboxing per workbuddy-bench practice when running untrusted tasks[5][8][9][10].

---

## 6. Desktop-app surfaces (documented, not mutated)

- **MCP client** (Settings → MCP → Add MCP Server: URL + auth, OAuth with automatic token refresh, per-server/per-tool toggles)[11]. No config file path or JSON example is published for the desktop app — the file facts in §1.4 are the shared CLI surface[11]. Third-party guides show `{"mcpServers":{...,"type":"http","url":...}}` entries[11].
- **Connectors**: GitHub, GitLab, Jira, Confluence, Google Drive, Gmail, Notion, Slack via OAuth or API credentials; no custom-connector SDK[13].
- **Permission modes**: exactly two — "Default Permissions" (sandbox-first, confirm high-risk) and "Full Access" (auto-confirm); chosen per task in the UI[12].
- **Skills**: market install + AI-generated custom skills (`skill.yml` + implementation files + README); **no public path/schema/loading mechanics documented**[17][18].
- **Automation/schedules and Slack ("Claw") pairing**: UI-only, no API/webhooks[16].

⚠️ **Gap:** the desktop app's own config storage location is undocumented; the app is treated as document-only. Only the shared `~/.codebuddy` CLI tree is a mutation surface.

---

## 7. NOT FOUND (searched 2026-09-08; do not infer)

- **No public workbuddy.ai HTTP API**: api./docs./developers./app.workbuddy.ai do not resolve; robots.txt hides `/api/` and `/trpc/` (private Next.js backends); no REST/GraphQL/tRPC docs[15].
- **No WorkBuddy OAuth clients or API keys for third parties**; desktop login is Google/GitHub OAuth as a user; no Zapier/Make/n8n integration for workbuddy.ai[15].
- **No PyPI package**; no official GitHub repo for the WorkBuddy app itself (only workbuddy-bench)[15].
- **No official WorkBuddy MCP server** — the product (and cbc) are MCP *clients*[15].
- **No headless/CLI mode of the desktop app**; no documented `skill.yml` JSON schema beyond field names[15][17].
- **Exact `cbc --version` output format not captured** — version resolution should prefer npm package metadata (`npm ls -g @tencent-ai/codebuddy-code` / pinned install version per HAD-02)[4].
- npm scope `@workbuddy/*` belongs to workbuddy.com (field service, AU) — unrelated[15].

---

## Sources

[1] https://www.workbuddy.ai/docs/workbuddy/Overview — WorkBuddy Docs: Overview
[2] https://www.workbuddy.ai/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Model — WorkBuddy Docs: Model
[3] https://api-docs.deepseek.com/quick_start/agent_integrations/workbuddy/ — DeepSeek: WorkBuddy custom-model integration (models.json schema)
[4] https://www.npmjs.com/package/@tencent-ai/codebuddy-code — npm: @tencent-ai/codebuddy-code
[5] https://www.codebuddy.ai/docs/cli/installation — CodeBuddy Docs: CLI installation & config locations
[6] https://www.codebuddy.ai/docs/cli/mcp — CodeBuddy Docs: CLI MCP configuration
[7] https://www.codebuddy.ai/docs/cli/headless — CodeBuddy Docs: CLI headless mode
[8] https://www.codebuddy.ai/docs/cli/iam — CodeBuddy Docs: CLI auth (IAM)
[9] https://github.com/Tencent/workbuddy-bench — GitHub: Tencent/workbuddy-bench (README, configs/)
[10] https://raw.githubusercontent.com/Tencent/workbuddy-bench/main/src/workbuddy_bench/agents/cbc_agent.py — workbuddy-bench: CbcAgent (models.json/settings.json presets, stream-json parsing)
[11] https://www.workbuddy.ai/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/MCP-Guide — WorkBuddy Docs: MCP Guide
[12] https://www.workbuddy.ai/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Permission-Modes — WorkBuddy Docs: Permission Modes
[13] https://www.workbuddy.ai/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Connector — WorkBuddy Docs: Connector
[14] https://intl.cloud.tencent.com/document/product/1300/81046 — Tencent Cloud: LLM Token Plan + WorkBuddy
[15] https://www.workbuddy.ai/robots.txt — workbuddy.ai robots.txt (private /api/, /trpc/); verified-absent list from research 2026-09-08
[16] https://www.workbuddy.ai/docs/workbuddy/Changelog — WorkBuddy Docs: Changelog (5.2.7, 2026-07-17)
[17] https://www.workbuddy.ai/docs/workbuddy/From-Beginner-to-Expert-Guide/Practice-Cases/Create-Skills — WorkBuddy Docs: Create Skills
[18] https://www.workbuddy.ai/docs/workbuddy/From-Beginner-to-Expert-Guide/Function-Description/Skills-Market — WorkBuddy Docs: Skills Market
