# Gemini CLI — Configuration Reference (google-gemini/gemini-cli)

Compiled directly from primary sources on 2026-08-25:
- https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md (100KB canonical reference)
- https://github.com/google-gemini/gemini-cli/blob/main/docs/get-started/authentication.mdx
- https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/settings.md

> ⚠️ **Status note:** Gemini CLI's individual free tier was cut off **June 18, 2026**; Google is migrating users to **Antigravity CLI (`agy`)**. See `antigravity-cli.md` in this collection for the successor's config and the migration path (`agy plugin import gemini`, skills paths move from `.gemini/skills/` to `.gemini/antigravity-cli/skills/`). Everything below remains valid for the OSS repo as of its current `main`.

---

## 1. Config layers & precedence

Applied lowest→highest (from `docs/reference/configuration.md`):

1. Hardcoded application defaults
2. **System defaults file** (overridable by everything above it) — path overridable via `GEMINI_CLI_SYSTEM_DEFAULTS_PATH`
3. **User settings file** — `$GEMINI_CLI_HOME/.gemini/settings.json` (default `~/.gemini/settings.json`)
4. **Project settings file** — `<project>/.gemini/settings.json`
5. **System settings file** (overrides all other settings files) — path overridable via `GEMINI_CLI_SYSTEM_SETTINGS_PATH`
6. **Environment variables** (incl. `.env` files)
7. **Command-line arguments**

`.env` file discovery order: CWD `.env` → walk up parents until `.git` root or home → `~/.env`. Some vars (`DEBUG`, `DEBUG_MODE`) are excluded from project `.env` loading (never excluded from `.gemini/.env`); customize with the `advanced.excludedEnvVars` setting.

## 2. `settings.json` schema

All settings live under top-level category objects. Categories present in current docs:
`general`, `ui`, `output`, `ide`, `model`, `modelConfigs`, `context`, `contextManagement`, `tools`, `mcp`, `security`, `privacy`, `billing`, `advanced`, `experimental`, `agents`, `hooks`, `hooksConfig`, `skills`, `policyPaths`, `adminPolicyPaths`, `admin`.

### Key keys

| Key | Type / Values | Default | Notes |
|---|---|---|---|
| `model.name` | string | unset | The conversation model |
| `model.maxSessionTurns` | number | `-1` (unlimited) | user/model/tool turns kept |
| `model.compressionThreshold` | number | (see docs) | context compression trigger fraction |
| `model.summarizeToolOutput` | object | unset | per-tool token budgets, e.g. `{"run_shell_command":{"tokenBudget":2000}}` |
| `modelConfigs.aliases` | object | built-in presets | Named model presets with `extends` inheritance and `generateContentConfig` (temperature, topP, topK, `thinkingConfig.includeThoughts`, `thinkingBudget`, `thinkingLevel`) — e.g. shipped aliases `base`, `chat-base`, `chat-base-3`, `gemini-3-pro*` |
| `general.defaultApprovalMode` | `"default"` \| `"auto_edit"` \| `"plan"` | `"default"` | YOLO only via CLI flag `--yolo` / `--approval-mode=yolo` |
| `general.plan.enabled` / `plan.directory` / `plan.modelRouting` | bool/string/bool | true/unset/true | Plan Mode; modelRouting auto-switches Pro (planning) ↔ Flash (implementation) |
| `general.checkpointing.enabled` | boolean | false | session checkpointing |
| `general.sessionRetention.*` | enabled/maxAge/maxCount/minRetention | true/"30d"/unset/"1d" | auto cleanup of chats |
| `general.maxAttempts` | number ≤10 | 10 | main-model request retries |
| `general.preferredEditor` | enum (vscode, cursor, zed, windsurf, antigravity, vim…) | `$VISUAL/$EDITOR` | |
| `output.format` | `"text"` \| `"json"` | `"text"` | JSON output mode |
| `ui.theme`, `ui.customThemes`, `ui.autoThemeSwitching` | string/object/bool | unset/{}/true | theming |
| `context.fileName` | string \| string[] | unset | name(s) of context/memory file (default concept: `GEMINI.md`) |
| `context.importFormat`, `context.includeDirectoryTree`, `context.discoveryMaxDirs` | string/bool/number | unset/true/200 | memory loading knobs |
| `security.auth.selectedType` | string | unset | selected auth type (**requires restart**) |
| `security.auth.enforcedType` | string | unset | forced auth type; mismatch forces re-auth |
| `security.auth.useExternal` | boolean | unset | external auth flow |
| `advanced.excludedEnvVars` | string[] | `[DEBUG, DEBUG_MODE]` | vars blocked from project `.env` loading |
| `policyPaths`, `adminPolicyPaths` | array | `[]` | extra tool-policy files/dirs |

### MCP servers (`mcpServers`)

```jsonc
{
  "mcpServers": {
    "my-server": {
      "command": "npx", "args": ["-y", "some-mcp-server"], "env": { "KEY": "val" },
      // OR remote: "url": "https://..." (SSE) or "httpUrl": "https://..." (streamable HTTP)
      "trust": true,           // skip per-tool confirmation prompts
      "timeout": 5000, "description": "...", "includeTools": ["..."], "excludeTools": ["..."]
    }
  }
}
```
Precedence if multiple transports given: `httpUrl` > `url` > `command`. Discovered tools get FQN `mcp_<serverAlias>_<toolName>`.

## 3. Environment variables

| Var | Effect |
|---|---|
| `GEMINI_API_KEY` | Gemini API key (AI Studio) — one auth method |
| `GEMINI_MODEL` | Default model override, e.g. `gemini-3-flash-preview` |
| `GEMINI_CLI_HOME` | **Root dir for all user-level config/storage** — CLI creates `.gemini/` inside it. THE multi-instance isolation knob |
| `GEMINI_CLI_SYSTEM_DEFAULTS_PATH` | Override location of the system-defaults settings file |
| `GEMINI_CLI_SYSTEM_SETTINGS_PATH` | Override location of the (highest-priority) system settings file |
| `GEMINI_CLI_TRUSTED_FOLDERS_PATH` | Override location of `trustedFolders.json` |
| `GEMINI_CLI_TRUST_WORKSPACE` | `"true"` = trust this workspace for the session (CI/headless) |
| `GEMINI_CLI_IDE_PID` | Pin IDE integration to a specific process PID |
| `GEMINI_CLI_SURFACE` | Extra label in User-Agent for traffic attribution |
| `GOOGLE_API_KEY` | Google Cloud API key (Vertex express mode; also Code Assist) |
| `GOOGLE_CLOUD_PROJECT` | Project ID — required for Code Assist & Vertex AI (Cloud Shell default-overrides global shell value; use `.env` to override there) |
| `GOOGLE_CLOUD_LOCATION` | Vertex region, e.g. `us-central1` |
| `GOOGLE_APPLICATION_CREDENTIALS` | Path to service-account JSON (Vertex ADC path) |
| `GOOGLE_GENAI_API_VERSION` | Override SDK API version |
| `GOOGLE_GEMINI_BASE_URL` | **Base URL override for Gemini API** (when using gemini-api-key auth). HTTPS enforced unless localhost/127.0.0.1/[::1] — proxy/gateway friendly |
| `GOOGLE_VERTEX_BASE_URL` | Same override for Vertex AI auth mode |
| `GEMINI_SANDBOX` | Force sandbox mode (`1`/`docker`/`podman`/`sandbox-exec`/`0`) |
| `GEMINI_SYSTEM_MD` / `GEMINI_WRITE_SYSTEM_MD` | Read/patch the bundled system prompt |
| `GEMINI_TELEMETRY_ENABLED/_LOG_PROMPTS/_OTLP_ENDPOINT/_OTLP_PROTOCOL/_OUTFILE/_TARGET/_TRACES_ENABLED/_USE_COLLECTOR` | OpenTelemetry telemetry family |
| `NO_COLOR` | Disable color |
| `DEBUG`, `DEBUG_MODE` | Debug flags (blocked from project `.env`s by default) |

## 4. Auth methods

Per `docs/get-started/authentication.mdx`:

| Method | Who | Env/config requirements |
|---|---|---|
| **Login with Google (OAuth)** | Individual free tier *(ended Jun 18 2026)* | browser flow; stored cached credentials |
| **Gemini API key** (AI Studio) | API users; **headless/CI recommended** | `GEMINI_API_KEY`; honors `GOOGLE_GEMINI_BASE_URL` |
| **Vertex AI** | Enterprise/GCP | One of: OAuth, ADC (`GOOGLE_APPLICATION_CREDENTIALS`), service acct, or Cloud API key (`GOOGLE_API_KEY`); plus `GOOGLE_CLOUD_PROJECT` + `GOOGLE_CLOUD_LOCATION`; honors `GOOGLE_VERTEX_BASE_URL` |

Selected type persists in `security.auth.selectedType`; enforce org-wide with `security.auth.enforcedType`. Custom gateways/proxies are supported exactly through the two `*_BASE_URL` vars (HTTPS-only except localhost).

## 5. Context files (GEMINI.md)

Hierarchical instructional memory: global `~/.gemini/GEMINI.md` → project root → subdirectories, merged with precedence toward deeper/closer files. Rename/extend via `context.fileName` (string or array). Supports `@file.txt` imports inside the markdown. `/memory show|refresh|add` commands manage it at runtime. `GEMINI_SYSTEM_MD=1` lets you inspect the system prompt; `GEMINI_WRITE_SYSTEM_MD=1` writes it to disk for editing experiments.

## 6. MULTI-INSTANCE WRAPPERS

Three documented levers compose cleanly:

1. **`GEMINI_CLI_HOME`** relocates ALL user state (settings.json, OAuth cache, tmp) → fully isolated instances.
2. **Auth/provider switching via env**: each instance exports its own key/project/base URL set before exec.
3. **Settings layering**: a shared project `.gemini/settings.json` can stay identical while per-instance user-level settings differ under separate homes.

```bash
#!/usr/bin/env bash
# gemini-personal: personal AI-Studio key via a proxy
export GEMINI_CLI_HOME="$HOME/.gemini-homes/personal"
export GEMINI_API_KEY="AIza...personal"
export GOOGLE_GEMINI_BASE_URL="https://my-proxy.example.com"
exec gemini "$@"

---
#!/usr/bin/env bash
# gemini-work: corporate Vertex backend, isolated state
export GEMINI_CLI_HOME="$HOME/.gemini-homes/work"
export GOOGLE_API_KEY="...work-key"          # or rely on ADC OAuth
export GOOGLE_CLOUD_PROJECT="corp-proj-id"
export GOOGLE_CLOUD_LOCATION="europe-west4"
export GOOGLE_VERTEX_BASE_URL="https://vertex-gw.corp.internal"
exec gemini "$@"
```

First run of each profile completes its own auth flow (`security.auth.selectedType` is stored per-home). For pure headless CI, prefer API-key auth + `GEMINI_CLI_TRUST_WORKSPACE=true` (or `--yolo` deliberately).

## 7. Sandboxing, extensions, misc

- Sandbox: `--sandbox` / `GEMINI_SANDBOX` with docker/podman/macOS Seatbelt profiles; custom images via `.gemini/sandbox.Dockerfile` (runs as `node` user; switch to root for packages).
- Extensions: `~/.gemini/extensions/<name>/extension.toml` (Gemini CLI extension format); commands/context/MCP ship inside an extension. Under Antigravity CLI these become *plugins* (`agy plugin import gemini` converts).
- Shell history, usage-statistics opt-out (`privacy.usageStatisticsEnabled`), and policy-based tool control (`policyPaths`) are all documented in the same reference.

## Sources
- https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md
- https://github.com/google-gemini/gemini-cli/blob/main/docs/get-started/authentication.mdx
- https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/settings.md
