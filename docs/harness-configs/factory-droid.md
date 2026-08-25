# Factory Droid CLI — Configurable Options Reference

Compiled 2026-08-25. Primary sources: `docs.factory.ai` (Settings, CLI Reference, Droid Exec, BYOK/Custom Models, Quickstart pages), cross-checked against an independent sandbox analysis (agent-safehouse.dev) where noted. Items that could **not** be verified in official docs are explicitly flagged.

---

## 1) Config files: `~/.factory/` paths + `settings.json` schema

### Verified paths

| Path | Purpose |
|---|---|
| `~/.factory/settings.json` (macOS/Linux) / `%USERPROFILE%\.factory\settings.json` (Windows) | Personal CLI configuration; created with defaults on first run ([settings](https://docs.factory.ai/droid-cli/settings)) |
| `~/.factory/settings.local.json` | User-level local overrides; merged **on top of** same-level `settings.json`; gitignore it for machine-specific prefs ([settings](https://docs.factory.ai/droid-cli/settings)) |
| `<project>/.factory/settings.json` and `<project>/.factory/settings.local.json` | Project-level settings + local overrides, same merge semantics ([settings](https://docs.factory.ai/droid-cli/settings)) |
| `~/.factory/config.json` | **Legacy** custom-models file with snake_case fields (`custom_models`, `base_url`). Still loaded and merged with `settings.json` (settings.json wins). ⚠️ `${VAR}` expansion does **not** apply here ([byok](https://docs.factory.ai/model-independence/byok)) |
| `.droid.yaml` | **Legacy/deprecated** project config surface → replaced by `.factory/` files + AGENTS.md ([settings](https://docs.factory.ai/droid-cli/settings)) |
| `~/.factory/mcp.json` | User MCP servers; `~/.factory/AGENTS.md` personal instructions; `~/.factory/skills/<n>/SKILL.md`; `~/.factory/droids/<n>.md` subagents; `~/.factory/commands/<n>.md` (legacy slash commands) *(paths reported by third-party analysis, consistent with docs' `/mcp`,`/skills`,`/droids` commands)* ([agent-safehouse](https://agent-safehouse.dev/docs/agent-investigations/droid)) |
| `~/.factory/specs` | Default `specSaveDir` for Spec Mode; `~/.factory/worktrees` default parent for `--worktree` (both official defaults) ([settings](https://docs.factory.ai/droid-cli/settings)) |

### `settings.json` schema — core (personal)

Source: [docs.factory.ai/droid-cli/settings](https://docs.factory.ai/droid-cli/settings)

| Key | Options | Default | Notes |
|---|---|---|---|
| `model` | any available model ID | product default | |
| `reasoningEffort` | `none,dynamic,off,minimal,low,medium,high,xhigh,max` (per-model subset) | model-dependent | |
| `sessionDefaultSettings.interactionMode` | `auto`, `spec` | `auto` | |
| `sessionDefaultSettings.autonomyLevel` | `off,low,medium,high` | `off` | `sessionDefaultSettings.autonomyMode` deprecated |
| `cloudSessionSync` | bool | `true` | mirror CLI sessions to Factory web |
| `diffMode` | `github`, `unified` | `github` | |
| `completionSound` / `awaitingInputSound` | `off,bell,fx-ok01,fx-ack01,<file path>` | `fx-ok01` / `fx-ack01` | `soundFocusMode`: `always/focused/unfocused` |
| `commandAllowlist` | array | safe defaults | run without extra confirmation |
| `commandDenylist` | array | restrictive defaults | always require confirmation (approvable) |
| `commandBlocklist` | array | `[]` | never runs — not even under `--skip-permissions-unsafe`; droid resolves the real binary so wrappers/absolute paths/substitution can't bypass |
| `includeCoAuthoredByDroid` | bool | `true` | commit co-author trailer |
| `enableDroidShield` | bool | `true` | secret scanning + git guardrails |
| `hooksDisabled` | bool | `false` | global hooks kill-switch |
| `disabledSkills` | array | `[]` | user+project arrays combine |
| `ideAutoConnect` | bool | `false` | |
| `showThinkingInMainView` | bool | `false` | |
| **`customModels`** | array of model configs | `[]` | see §3 |
| `blockOnMcpLoad` | bool | `false` | |

Other documented groups: display/UI (`toolResultDisplay`, `theme`, `nerdFont`, `logoAnimation`…), sounds extras (`subagentSounds`), missions (`missionModelSettings.workerModel`, `skipScrutiny`, `missionOrchestratorModel`, `keepSystemAwakeDuringMissions`…), subagents (`subagentAutonomyLevel` `inherit/off/low/medium/high`; `subagentModelSettings.{light,medium,heavy}{Model,ReasoningEffort}`), compaction (`compactionTokenLimit`, `compactionTokenLimitPerModel`, `compactionModel`, `modelFallbacks`), spec (`specSaveDir`), infra (`statusLine {command,padding,maxRows}`, `worktreeDirectory`, `llmRequestTimeout`, `subagentInactivityTimeout`, `remoteAccessEnabled`) — all from [settings](https://docs.factory.ai/droid-cli/settings).

### Enterprise / org-managed keys (same file, pushed by admins)

`maxAutonomyLevel` (`off..high`, clamps users), `subagentAutonomyLevel`, `modelPolicy {allowedModelIds, blockedModelIds, allowCustomModels, allowedBaseUrls}`, `mcpPolicy {enabled, allowlist}`, `mcpAutonomyOverrides`, `mcpAutonomyUrlOverrides`, `missionPolicy {restrictedAccess, allowedUserIds}`, `networkPolicy {allowedIps}`, `sandbox {enabled, mode, filesystem, network}`, `allowManagedHooksOnly`, `disableAutoUpdate`, BYOM/computer settings, `sessionRetentionDays`, `wikiCloudSync` — ([settings#enterprise](https://docs.factory.ai/droid-cli/settings#enterprise-and-org-level-settings), [hierarchical-settings-and-org-control](https://docs.factory.ai/docs/enterprise/hierarchical-settings-and-org-control))

### ⚠️ Telemetry — verification result

The official settings reference defines **no top-level `telemetry` key**. Closest documented data-sharing knobs: `cloudSessionSync` (personal), `sessionRetentionDays` / `wikiCloudSync` / `disableWeeklyUsageSummary` (org), plus customer-owned telemetry exports described on the compliance page ([compliance-audit-and-monitoring](https://docs.factory.ai/enterprise/compliance-audit-and-monitoring)). If you've seen a `telemetry` block in the wild it is not part of the published schema.

---

## 2) Environment variables

### Official (Factory docs)

| Var | Purpose | Source |
|---|---|---|
| `FACTORY_API_KEY` | API-key auth for non-interactive/CI use (`fk-...`). Used directly (`export FACTORY_API_KEY=...`) and as GH Actions secret | [cli-reference](https://docs.factory.ai/droid-cli/cli-reference) CI example; [code-review-ci](https://docs.factory.ai/software-factory/code-review-ci) |
| `${VAR_NAME}` refs inside `settings.json`/`settings.local.json` | `apiKey` (and Bedrock `awsRegion`, `awsProfile`, `bedrockBaseUrl`, `requestMetadata` values+keys) expand environment references at parse time; missing var ⇒ fail-fast error naming the variable | [byok](https://docs.factory.ai/model-independence/byok) |
| `FACTORY_API_KEY_HELPER_TTL_MS` | Overrides per-model `apiKeyHelperTtlMs` (analog of Claude Code's `CLAUDE_CODE_API_KEY_HELPER_TTL_MS`) | [byok](https://docs.factory.ai/model-independence/byok) |
| `FACTORY_DROID_AUTO_UPDATE_ENABLED` | Set `true` to force-enable auto-updates (overrides npm-build default); unset/false disables, also disables `droid update`. Org analog: `disableAutoUpdate` | [cli-reference](https://docs.factory.ai/droid-cli/cli-reference) |

### Reported by independent analysis (not in official docs I could reach — treat as unverified-but-plausible)

`FACTORY_TOKEN` (CI token auth), `FACTORY_DISABLE_KEYRING`, `FACTORY_LOG_FILE`, `FACTORY_PROJECT_DIR` + `DROID_CWD` + `DROID_PLUGIN_ROOT` (hook/plugin context), `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY`, `NODE_EXTRA_CA_CERTS`, `GH_TOKEN` — ([agent-safehouse](https://agent-safehouse.dev/docs/agent-investigations/droid))

### ⚠️ `FORCECODE_API_URL` / `FORCECODE_API_KEY`

**Not found.** A targeted search for `FORCECODE_API_URL` / `FORCECODE_API_KEY` returned zero results across official and community sources (2026-08-25). These look like internal/historical codenames (Factory's earlier product was "forcecode"-era); do not rely on them. The documented mechanisms for pointing droid elsewhere are the BYOK `baseUrl` + `apiKey` fields (§3) and `FACTORY_API_KEY`.

---

## 3) Custom Models (BYOK) — OpenAI-compatible providers

Official feature. Keys never leave your machine; custom models work in CLI + desktop app only (not hosted web/mobile) ([byok](https://docs.factory.ai/model-independence/byok)). Configure under the `customModels` array in `~/.factory/settings.json`.

### Field reference ([byok#configuration-reference](https://docs.factory.ai/model-independence/byok))

| Field | Req | Description |
|---|---|---|
| `model` | ✅ | ID sent over the wire (`gpt-5.3-codex`, `qwen3:4b`, …) |
| `displayName` | – | Label in `/model` picker |
| `baseUrl` | ✅* | Endpoint base (*required for all non-Bedrock models) |
| `apiKey` | – | Optional; omit for keyless endpoints; if set cannot be empty; supports `${VAR}` |
| `apiKeyHelper` | – | Shell command whose stdout is the credential; request-time minted, TTL-cached, refreshed after 401; **org-managed settings only** (stripped from user/project/folder settings); takes precedence over static `apiKey` |
| `apiKeyHelperTtlMs` | – | Default 300000 ms; env `FACTORY_API_KEY_HELPER_TTL_MS` wins |
| `provider` | ✅ | `anthropic` (Messages API), `openai` (Responses API), `generic-chat-completion-api` (Chat Completions — OpenRouter/Fireworks/Together/Ollama/vLLM/etc.) |
| `maxOutputTokens` | – | Cap on responses |
| `noImageSupport` | – | `true` disables image inputs |
| `extraArgs` | – | Extra body params, e.g. `{"temperature":0.7,"top_p":0.9}` |
| `extraHeaders` | – | Extra HTTP headers (can carry auth for keyless gateways) |
| `bedrock` | – | `{awsRegion, awsProfile, bedrockBaseUrl, awsAuthRefresh, awsCredentialExport, requestMetadata}` |

Provider table incl. canonical base URLs: OpenRouter `https://openrouter.ai/api/v1`, Fireworks `https://api.fireworks.ai/inference/v1`, DeepInfra `https://api.deepinfra.com/v1/openai`, Groq `https://api.groq.com/openai/v1`, Baseten `https://inference.baseten.co/v1`, HF `https://router.huggingface.co/v1`, Gemini `https://generativelanguage.googleapis.com/v1beta/` ([byok#provider-reference](https://docs.factory.ai/model-independence/byok)). Docs warn: only official Anthropic/OpenAI routes are fully tested; models <30B parameters perform significantly worse on agentic coding.

### Worked examples

```jsonc
// ~/.factory/settings.json
{
  "customModels": [
    // OpenRouter (official example model id)
    {
      "model": "openai/gpt-oss-20b",
      "displayName": "GPT-OSS 20B (OpenRouter)",
      "baseUrl": "https://openrouter.ai/api/v1",
      "apiKey": "${OPENROUTER_API_KEY}",          // export OPENROUTER_API_KEY=sk-or-...
      "provider": "generic-chat-completion-api",
      "maxOutputTokens": 16384
    },
    // Ollama local (official guidance: raise ctx to >=32k, e.g. OLLAMA_CONTEXT_LENGTH=32000 ollama serve;
    // no apiKey needed, some builds want a placeholder)
    {
      "model": "qwen3:30b",
      "displayName": "Qwen3 30B (Ollama)",
      "baseUrl": "http://localhost:11434/v1",
      "provider": "generic-chat-completion-api",
      "noImageSupport": true
    },
    // vLLM serve (standard OpenAI-compatible endpoint; pattern follows docs' generic provider,
    // URL is vLLM's default --api-server origin, not verbatim in Factory docs)
    {
      "model": "Qwen/Qwen3-Coder-30B-A3B-Instruct",
      "displayName": "Qwen3-Coder (vLLM)",
      "baseUrl": "http://localhost:8000/v1",
      "apiKey": "EMPTY",                           // vLLM ignores value unless --api-key set
      "provider": "generic-chat-completion-api",
      "extraArgs": { "temperature": 0.2 }
    },
    // LiteLLM proxy (OpenAI-compatible at /v1; same caveat — endpoint per LiteLLM docs)
    {
      "model": "anthropic/claude-sonnet-4-5",
      "displayName": "Sonnet 4.5 (LiteLLM gw)",
      "baseUrl": "http://localhost:4000/v1",
      "apiKey": "${LITELLM_KEY}",
      "provider": "generic-chat-completion-api"
    },
    // Anthropic-protocol gateway (Z.AI example, anthropic provider + maxOutputTokens)
    {
      "model": "glm-5.2",
      "displayName": "GLM (Anthropic proto)",
      "baseUrl": "https://api.z.ai/api/anthropic",
      "apiKey": "${ZAI_API_KEY}",
      "provider": "anthropic",
      "maxOutputTokens": 131072
    },
    // AWS Bedrock routing
    {
      "model": "anthropic.claude-sonnet-4-5-20250929-v1:0",
      "displayName": "Sonnet 4.5 [Bedrock]",
      "provider": "anthropic",
      "apiKey": "not-used-for-bedrock",
      "bedrock": { "awsRegion": "${AWS_REGION}", "awsProfile": "${AWS_PROFILE}" }
    },
    // Short-lived-token gateway (org-managed settings only)
    {
      "model": "internal-model", "displayName": "Gateway Model",
      "baseUrl": "https://llm-gateway.internal.example.com/v1",
      "provider": "generic-chat-completion-api",
      "apiKeyHelper": "/usr/local/bin/mint-gateway-token.sh",
      "apiKeyHelperTtlMs": 300000
    }
  ]
}
```

Using them: `/model` lists a separate **“Custom models”** section interactively ([byok](https://docs.factory.ai/model-independence/byok)); headless: `droid exec --model "custom:My-Custom-Model-0" ...` ([droid-exec overview](https://docs.factory.ai/droid-exec/overview)). Org controls: `modelPolicy.allowCustomModels` / `allowedBaseUrls` can forbid custom models or restrict endpoints; net-new org-managed custom models may not ship static `apiKey`s ([byok](https://docs.factory.ai/model-independence/byok), [settings](https://docs.factory.ai/droid-cli/settings)). Legacy snake_case (`custom_models`, `base_url`) in `~/.factory/config.json` still merges, without `${VAR}` expansion.

---

## 4) Multi-instance wrappers (config dir / env switching)

⚠️ **No official env var relocates the config dir.** Searches for a `FACTORY_CONFIG_DIR`-style knob found nothing; the documented resolution order is fixed: user `~/.factory/{settings.json, settings.local.json}` ← then project `.factory/settings.local.json` overrides on top ([settings](https://docs.factory.ai/droid-cli/settings)). Documented levers for running parallel instances with different identity/models:

1. **Per-instance `FACTORY_API_KEY`** — switches Factory account/auth cleanly (official var, §2).
2. **`${VAR}` indirection** — one shared settings file, different exported keys per instance (e.g. `OPENROUTER_API_KEY` vs gateway key) (official, §2/§3).
3. **Project-scoped `.factory/` dirs** — each checkout carries its own overrides (official).
4. **`HOME` relocation** — point `$HOME` at a stub dir containing its own `.factory/settings.json` for full isolation (sessions, skills, everything). *Undocumented hack — works because every path above is `$HOME`-relative; not covered by docs.*
5. Session hygiene per instance: `-s <id>` / `--fork <id>` / `--tag <spec>` / `--cwd <path>` (official flags, §5).

```bash
#!/usr/bin/env bash
# droid-wrapper.sh — isolated droid instance: own account, own models, own state
# usage: DROID_PROFILE=openrouter droid-wrapper.sh exec --auto medium "task"
set -euo pipefail
PROFILE="${DROID_PROFILE:-default}"
ROOT="$HOME/.droid-profiles/$PROFILE"

mkdir -p "$ROOT/.factory"

# Option A (official): auth via env only
case "$PROFILE" in
  work)        export FACTORY_API_KEY="${WORK_FACTORY_KEY}" ;;
  openrouter)  export OPENROUTER_API_KEY="${OPENROUTER_API_KEY:?}" ;;
esac

# Option B (hack): fully isolated config/state tree via $HOME override
if [[ "${ISOLATE:-0}" == "1" ]]; then
  [[ -f "$ROOT/.factory/settings.json" ]] || cat > "$ROOT/.factory/settings.json" <<EOF
{ "customModels": [ { "model": "openai/gpt-oss-20b", "displayName": "OR-$PROFILE",
  "baseUrl": "https://openrouter.ai/api/v1", "apiKey": "\${OPENROUTER_API_KEY}",
  "provider": "generic-chat-completion-api" } ] }
EOF
  exec env HOME="$ROOT" droid "$@"
fi

exec droid "$@"
```

---

## 5) `droid exec` headless: flags, autonomy levels, allowlists

### Full option surface ([droid-exec overview](https://docs.factory.ai/droid-exec/overview), [cli-reference](https://docs.factory.ai/droid-cli/cli-reference))

```
Usage: droid exec [options] [prompt]
  -o, --output-format <text|json|stream-json|stream-jsonrpc>   (stream-jsonrpc recommended for multi-turn)
  --input-format <format>            must match output format
  -f, --file <path>                  prompt from file       | cat file | droid exec  (pipes work)
  -s, --session-id <id>              continue session       | --fork <id> fork-and-continue
  -m, --model <id>                   incl. "custom:<name>"  | -r, --reasoning-effort <level>
  --use-spec / --spec-model <id> / --spec-reasoning-effort <level>
  --auto <low|medium|high>           autonomy level (see below)
  --skip-permissions-unsafe          skip ALL permission checks (dangerous)
  --restrict-tools <ids>             ONLY these tools       | --additional-tools <ids>
  --disabled-tools <ids>                                     | --disable-builtin-skills
  --list-tools                       print tools and exit
  --cwd <path> | -w, --worktree [name] | --worktree-dir <path>
  --tag <spec> (repeatable) | --log-group-id <id>
  --append-system-prompt <text> | --append-system-prompt-file <path>
  --mission                          Mission Mode (requires --auto high or --skip-permissions-unsafe)
  --worker-model/--worker-reasoning-effort, --validator-model/--validator-reasoning-effort
```
Note the flag collision: in `droid exec`, `-r` = `--reasoning-effort`; in interactive `droid`, `-r` = `--resume` ([cli-reference](https://docs.factory.ai/droid-cli/cli-reference)). Exit codes: `0` success, `1` runtime error, `2` bad args *(observed values from third-party analysis)* ([agent-safehouse](https://agent-safehouse.dev/docs/agent-investigations/droid)).

### Autonomy levels — ⚠️ correction to “auto medium/full”

The real ladder is **`low | medium | high`** (there is no `full`; interactive settings use `off` as the floor). Exec starts **read-only by default** ([droid-exec overview](https://docs.factory.ai/droid-exec/overview)):
- default (no flag): analyze/plan only
- `--auto low`: simple file edits · `--auto medium`: normal local dev ops (install deps, run tests, fix) · `--auto high`: deployment-grade ops (commit, push)
- `--skip-permissions-unsafe`: bypass everything (recommended only in disposable containers; docs show a Docker CI example)
Interactive equivalents: `sessionDefaultSettings.autonomyLevel` (`off..high`) + TUI `ctrl+L`; enterprise clamp via `maxAutonomyLevel` ([settings](https://docs.factory.ai/droid-cli/settings)).

### Allowlists (command gating in settings.json) ([settings](https://docs.factory.ai/droid-cli/settings))

```json
{
  "commandAllowlist": ["ls", "pwd", "dir"],
  "commandDenylist":  ["rm -rf /", "mkfs", "shutdown"],
  "commandBlocklist": ["shutdown", "mkfs", "curl"]
}
```
Precedence: **blocklist > denylist > allowlist**; anything unlisted falls through to the session autonomy level; denylist = needs explicit approval, blocklist = impossible even under `--skip-permissions-unsafe`, with binary-resolution anti-spoofing. Tool-level gating in headless runs: `--restrict-tools` / `--additional-tools` / `--disabled-tools` / `--list-tools` ([droid-exec overview](https://docs.factory.ai/droid-exec/overview)).

### Canonical CI pattern ([cli-reference](https://docs.factory.ai/droid-cli/cli-reference))
```yaml
- name: Run Droid Analysis
  env:
    FACTORY_API_KEY: ${{ secrets.FACTORY_API_KEY }}
  run: droid exec --auto medium -f .github/prompts/deploy.md
```

---

## 6) Slack / Linear / CI touchpoints

- **Integrations**: “Connect Jira, Notion, **Slack**, **Linear**, PagerDuty, and MCP tools” from the CLI/platform ([droid-cli overview](https://docs.factory.ai/droid-cli/overview)). MCP servers managed via `droid mcp add/remove` + `/mcp`, gated by org `mcpPolicy` ([cli-reference](https://docs.factory.ai/droid-cli/cli-reference), [settings](https://docs.factory.ai/droid-cli/settings)).
- **Delegated sessions**: a `--delegation-url <url>` flag “URL for delegated sessions (Slack/Linear)” is observed in the binary’s flag set *(third-party analysis; not on the official flag table)* ([agent-safehouse](https://agent-safehouse.dev/docs/agent-investigations/droid)).
- **GitHub App / Action**: [`Factory-AI/droid-action`](https://github.com/Factory-AI/droid-action) powers the Droid app; triggers on `@droid` mentions in issue comments, PR review comments, PR reviews, and issues; inputs include `factory_api_key: ${{ secrets.FACTORY_API_KEY }}` and optional `github_token`; required workflow permissions: `contents: write`, `pull-requests: write`, `issues: write`, `id-token: write`, `actions: read` ([code-review-ci](https://docs.factory.ai/software-factory/code-review-ci), [agent-safehouse](https://agent-safehouse.dev/docs/agent-investigations/droid)).
- **GitLab**: a GitLab CI/CD component ships from the same repo ([code-review-ci](https://docs.factory.ai/software-factory/code-review-ci)).
- **Headless CI hardening**: run in disposable containers when escalating (`docker run … droid exec --skip-permissions-unsafe`), structured artifacts via `-o json/stream-jsonrpc` ([droid-exec overview](https://docs.factory.ai/droid-exec/overview)); audit posture (runner-local execution, logs) in [compliance-audit-and-monitoring](https://docs.factory.ai/enterprise/compliance-audit-and-monitoring).

---

## Sources

1. https://docs.factory.ai/droid-cli/settings — settings.json paths, full schema, allowlists, enterprise keys
2. https://docs.factory.ai/model-independence/byok — customModels field reference, providers, apiKeyHelper, Bedrock, legacy config.json
3. https://docs.factory.ai/droid-exec/overview — exec help text, autonomy levels, output formats, custom:`name` usage
4. https://docs.factory.ai/droid-cli/cli-reference — command/flag tables, env-gated auto-update, CI example
5. https://docs.factory.ai/cli/getting-started/quickstart — install, slash commands, TUI controls
6. https://docs.factory.ai/droid-cli/overview — Slack/Linear/Jira/PagerDuty integration claim
7. https://docs.factory.ai/software-factory/code-review-ci + https://github.com/Factory-AI/droid-action — CI touchpoints
8. https://agent-safehouse.dev/docs/agent-investigations/droid — third-party sandbox analysis (env vars, exit codes, `--delegation-url`; flagged inline wherever used)
