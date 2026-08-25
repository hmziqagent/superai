# Cline — Complete Configuration Reference

Compiled 2026-08-25 from primary sources: [docs.cline.bot](https://docs.cline.bot/) (index: [/llms.txt](https://docs.cline.bot/llms.txt)) and the `cline/cline` GitHub repo. Cline now ships as four surfaces — **VS Code/JetBrains extension**, **CLI** (`cline`), **TUI**, and **SDK/ACP** — all sharing a global config root ([Config guide](https://docs.cline.bot/getting-started/config)). Roo Code and Kilo Code are Cline forks with their own config deltas (final section).

---

## 1. Settings Surfaces

### 1.1 Config directory layout (global vs per-project)

Per the [Config page](https://docs.cline.bot/getting-started/config):

- **Global**: `~/.cline/` — applies across IDE, CLI, SDK on the machine
- **Project**: `.cline/` at repo root — team-shared, committable

```
~/.cline/
  data/
    settings/
      providers.json           # API keys + provider configuration
      global-settings.json     # Global settings
      cline_mcp_settings.json  # MCP settings
    teams/  sessions/  db/     # Team state, session data, SQLite (e.g. cron.db)
    workflows/
  rules/  hooks/  skills/  agents/  plugins/  cron/
~/Documents/Cline/{Rules,Hooks,Plugins,Workflows}/   # legacy compat search paths

<project>/.cline/
  rules/  skills/  hooks/  agents/  plugins/  cron/
```

### 1.2 VS Code extension settings (`settings.json`, `cline.*`)

The extension contributes settings under the `cline.` namespace in VS Code's `settings.json` (see `contributes.configuration` in [cline/cline → src/package.json](https://github.com/cline/cline/blob/main/src/package.json)). Notable keys:

| Key | Purpose |
|---|---|
| `cline.autoApprove` | Legacy global auto-approve string ("yolo" etc.) |
| `cline.preferredLanguage` | Preferred response language |
| `cline.enableCheckpoints` | Shadow-git checkpoints on/off (also toggleable in UI) |
| `cline.chromeExecutablePath` | Browser tool Chrome binary override |
| `cline.remoteBrowserHost` | Remote browser host for the browser tool |
| `cline.vscodeLmModelSelector` | Model selector for the "VS Code LM API" provider |
| `cline.telemetrySetting` | Telemetry opt level (`unset`/`enabled`/`disabled`) |

Most runtime behavior (auto-approve granularity, plan/act models, browser) lives in Cline's own panel/settings UI rather than `settings.json`; the panel state is persisted under the extension's global storage.

### 1.3 Provider panel (⚙️ in the Cline sidebar)

Flow per [Authorization guide](https://docs.cline.bot/getting-started/authorizing-with-cline): pick provider → authenticate → pick model.

- **First-party providers**: *Cline (usage-billing)* (OAuth sign-in, credits at app.cline.bot), *ClinePass* ($9.99/mo flat subscription).
- **BYOK dropdown includes**: Anthropic (+ separate **Claude Code** subscription provider using your local `claude` CLI path), OpenAI (incl. Codex OAuth), **OpenAI Compatible**, OpenRouter, Google Gemini, AWS Bedrock (API key / IAM credentials / CLI SSO profile — see [Bedrock guides](https://docs.cline.bot/provider-config/aws-bedrock/api-key.md)), Azure, GCP Vertex, Ollama, LM Studio, DeepSeek, Requesty, Together, Qwen (Alibaba), Doubao, Z AI (GLM), MiniMax, Poolside, VS Code LM API, and ~30 more ([Other 30+ providers](https://docs.cline.bot/provider-config/other-30-plus-providers.md)).
- **Common fields** ([OpenAI Compatible page](https://docs.cline.bot/provider-config/openai-compatible)):
  - **Base URL** — endpoint-specific; must NOT be `https://api.openai.com/v1` for third parties
  - **API Key** — or "Use Azure Identity Authentication" checkbox (uses existing managed identity, e.g. `az login`)
  - **Model ID** — free-text or dropdown depending on provider
  - **Custom headers** — supported via provider config types (extension UI where offered; always available in SDK, §3)
  - **Model Configuration overrides**: max output tokens, context window size, image support, computer use, input/output price per M tokens

### 1.4 MCP settings — `cline_mcp_settings.json`

Two generations of location:

- **VS Code extension (current)**: inside extension globalStorage —
  - macOS: `~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json`
  - Windows: `%APPDATA%\Code\User\globalStorage\saoudrizwan.claude-dev\settings\cline_mcp_settings.json`
  - Linux: `~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json`
  (per [IONOS integration doc](https://docs.ionos.com/cloud/ai/mcp-server/connect-to-an-ai-client/cline); it is separate from VS Code's `.vscode/mcp.json`)
- **CLI/SDK/shared config root**: `~/.cline/data/settings/cline_mcp_settings.json` ([Config page](https://docs.cline.bot/getting-started/config))

Schema example:

```json
{
  "mcpServers": {
    "linear": {
      "command": "node",
      "args": ["/path/to/build/index.js"],
      "env": { "LINEAR_API_KEY": "..." },
      "disabled": false,
      "autoApprove": []
    }
  }
}
```

HTTP/SSE servers use `"url"` instead of `command`/`args`. Restrict permissions with `chmod 600`.

### 1.5 Custom instructions / rules — `.clinerules`

Per [Rules guide](https://docs.cline.bot/customization/cline-rules):

- Workspace: `.clinerules/` dir at project root (all `.md`/`.txt` files combined; numeric prefixes optional)
- Also auto-detected: `.cursorrules`, `.windsurfrules`, `AGENTS.md`
- Global: `~/.cline/rules/` or `~/Documents/Cline/Rules` (Win/mac/Linux); cross-tool `~/.agents/AGENTS.md` also read
- Per-file YAML frontmatter conditionals:
  ```yaml
  ---
  paths:
    - "src/components/**"
  ---
  ```
  Glob syntax matches gitignore-style (`*`, `**`, `?`, `[abc]`, `{a,b}`). No frontmatter = always active; `paths: []` = never active; invalid YAML fails open.
- Rules are individually toggleable in the Rules panel (scale icon); workspace rules take precedence over global on conflict.

### 1.6 `.clineignore` (deprecated)

Per [.clineignore page](https://docs.cline.bot/customization/clineignore): gitignore-syntax file at project root controlling what Cline loads automatically. **Not a security boundary** — explicit `@` mentions and shell commands still read ignored files. Being phased out; replacement is the SDK plugin pattern ("Block Ignored File Access") hooking `beforeTool` against `.gitignore`:

```bash
cline plugin install https://github.com/cline/cline/blob/main/sdk/examples/plugins/gitignore-read-files-guard.ts --cwd .
```

Multi-root workspaces support one `.clineignore` per root.

---

## 2. Env / Key Handling & Profile Management

- **API keys** entered in the provider panel are stored in **VS Code secret storage** (encrypted, per-machine); provider config metadata lives in `~/.cline/data/settings/providers.json` ([Config page](https://docs.cline.bot/getting-started/config)). Frequent re-auth troubleshooting note confirms secrets live with IDE storage ("Ensure you're not clearing IDE secrets").
- **SDK env vars** ([SDK Model Providers](https://docs.cline.bot/sdk/model-providers.md)): `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY` / `GOOGLE_APPLICATION_CREDENTIALS`, AWS standard chain (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`), `MISTRAL_API_KEY`.
- **CLI env vars** ([CLI config env table](https://docs.cline.bot/getting-started/config#environment-variables)):
  | Var | Meaning |
  |---|---|
  | `CLINE_DATA_DIR` | Replace `~/.cline/data/` |
  | `CLINE_HUB_ADDRESS` | Hub daemon address (default `127.0.0.1:25463`) |
  | `CLINE_SESSION_BACKEND_MODE` | `local` / `hub` / `remote` / `auto` |
  | `CLINE_SANDBOX`, `CLINE_SANDBOX_DATA_DIR` | Sandbox mode + its storage |
  | `CLINE_HOOKS_DIR` | Extra hooks directory |
  | `CLINE_COMMAND_PERMISSIONS` | JSON shell policy, e.g. `'{"allow":["npm *","git *"],"deny":["rm -rf *"],"allowRedirects":false}'` (deny overrides allow) |
- **Profiles**: no first-class named profile system yet. Isolation is achieved by pointing different instances at different data dirs — CLI: `cline --config /path/to/custom/config "task"` or `CLINE_DATA_DIR=...`; VS Code: `--user-data-dir` wrapper (§4).
- **Bedrock profiles**: Bedrock provider supports AWS CLI SSO profiles natively ([CLI Profile guide](https://docs.cline.bot/provider-config/aws-bedrock/cli-profile.md)).
- Enterprise remote configuration exists: admins push org-wide provider configs (Anthropic, LiteLLM member/admin guides under `enterprise-solutions/configuration/remote-configuration/`).

---

## 3. Third-party / Local Providers — Worked Examples

### 3a. OpenAI Compatible → LiteLLM proxy

Panel: API Provider = **OpenAI Compatible**
- Base URL: `http://localhost:4000/v1` (LiteLLM proxy)
- API Key: your LiteLLM master/virtual key
- Model ID: e.g. `claude-sonnet-4-6` (whatever LiteLLM route name is)
- Optionally expand **Model Configuration** to set true context window / pricing if auto-detection is wrong.

### 3b. OpenAI Compatible → Ollama

- Base URL: `http://localhost:11434/v1`
- API Key: any non-empty placeholder (Ollama ignores it)
- Model ID: e.g. `qwen2.5-coder:32b`
(Dedicated **Ollama** provider also exists; see [Running Models Locally](https://docs.cline.bot/running-models-locally/overview). LM Studio: `http://localhost:1234/v1`.)

### 3c. Anthropic through a gateway

Panel: API Provider = **Anthropic** → check **"Use custom base URL"** → enter gateway URL (e.g. an Anthropic-compatible proxy such as LiteLLM `/anthropic` route or corporate gateway) → paste key scoped to the gateway. Or route Claude models through **OpenRouter** as provider. Subscription alternative: **Claude Code** provider with CLI path set to `which claude` ([Anthropic page](https://docs.cline.bot/provider-config/anthropic)); caveats: non-streaming, limited image upload/prompt caching.

### 3d. SDK equivalent

```ts
const agent = new Agent({
  providerId: "openai-compatible",     // or "anthropic", "bedrock"
  modelId: "my-model",
  apiKey: process.env.PROVIDER_API_KEY,
  baseUrl: "https://your-provider.com/v1",
  headers: { "X-Custom": "value" },    // provider config supports apiKey/baseUrl/headers
})
// Bedrock: providerConfig: { awsRegion: "us-east-1" }
```
([SDK Model Providers](https://docs.cline.bot/sdk/model-providers.md); registries: `registerProvider`, `registerModel`, `DefaultGateway`.)

### 3e. Proxies/networking

Corporate firewall/proxy setup documented at [Networking and Proxies](https://docs.cline.bot/troubleshooting/networking-and-proxies.md).

---

## 4. Multi-Instance Wrappers (isolated Cline instances)

Cline has no built-in multi-profile switcher, so isolation comes from VS Code-level separation. Three mechanisms:

1. **VS Code profiles** (built-in Profiles feature): each profile has its own extensions+globalStorage, so each gets its own `saoudrizwan.claude-dev/settings/cline_mcp_settings.json` and secret-storage entries → independent providers/MCP. Launch: `code --profile work`.
2. **Portable mode** (official VS Code mechanism): drop a `data/` folder next to the VS Code install; all user data (including globalStorage and secrets) becomes relative to the install — copy the install to clone a full Cline environment. (Docs: code.visualstudio.com/docs/editor/portable.)
3. **Separate user-data dirs** — strongest isolation:

```bash
#!/usr/bin/env bash
# cline-instance.sh <name> — launch a fully isolated VS Code+Cline instance
NAME="${1:-default}"
DIR="$HOME/.clinstances/$NAME"
mkdir -p "$DIR"/{data,extensions}
export CLINE_DATA_DIR="$DIR/cline-data"   # isolates CLI/SDK-side ~/.cline/data too
code \
  --user-data-dir="$DIR/data" \
  --extensions-dir="$DIR/extensions" \
  --profile "$NAME"
```

Each `$NAME` then has completely independent: installed extensions, VS Code secret storage (so different API keys/providers), `cline_mcp_settings.json`, and Cline task history. Configure the provider once per instance on first launch. For headless/CI variants prefer the CLI: `CLINE_DATA_DIR=$DIR/cline-data cline --config "$DIR/cline-config" "task"`.

---

## 5. Auto-Approve, Plan/Act, Checkpoints, Browser Tool

### Auto-approve ([guide](https://docs.cline.bot/features/auto-approve))

Evaluated per tool call; categories:

| Toggle | Grants |
|---|---|
| Read project files / Read all files | Reads within/outside workspace (base toggle required for "all files") |
| Edit project files / Edit all files | Edits within/outside workspace |
| Execute safe commands / Execute all commands | Terminal, tiered by model-set `requires_approval` flag (no fixed allowlist) |
| Use the browser | Browser fetch/search tool |
| Use MCP servers | MCP tools/resources |
| Enable notifications | OS notifications incl. 30s long-running command alerts |

- **YOLO Mode** (Settings → Features): approves everything including mode transitions. Explicitly warned as dangerous.
- Hard shell-command restriction independent of the model: `CLINE_COMMAND_PERMISSIONS='{"allow":[...],"deny":[...],"allowRedirects":false}'`.
- SDK side mirrors this via permission-handling APIs ([SDK Permission Handling](https://docs.cline.bot/sdk/guides/permission-handling.md)).

### Plan / Act modes ([guide](https://docs.cline.bot/core-workflows/plan-and-act))

- **Plan**: read/search/discuss only, no edits or commands. **Act**: full execution; conversation carries over.
- **Separate models per mode**: enable "Use different models for Plan and Act" in Settings; selection preserved across switches (e.g., Opus plans, Sonnet acts).
- `/deep-planning` slash command for extended multi-file planning; `/newrule` creates rule files interactively ([Commands](https://docs.cline.bot/core-workflows/using-commands)).

### Checkpoints ([guide](https://docs.cline.bot/core-workflows/checkpoints))

- On by default; shadow Git repo separate from project history, committed after every tool use; captures untracked files too.
- Toggle: Settings → Feature Settings → "Enable Checkpoints". Disable for very large repos (storage/perf cost).
- Restore options: **Files only**, **Task only**, **Files & Task**; Compare opens diff view; integrates with message editing ("Restore All").

### Browser tool

- Uses local Chrome/Chromium; configurable via VS Code settings `cline.chromeExecutablePath` and `cline.remoteBrowserHost` for remote browsers (repo `src/package.json` contributes these). Auto-approve category "Use the browser" gates it; YOLO approves automatically.

---

## 6. Roo Code & Kilo Code Deltas (forks)

Both forks share Cline's core architecture (provider panel, `.clineignore`, MCP settings in their own globalStorage IDs) but diverge:

### Roo Code (RooCodeInc/Roo-Code — archived May 2026)

- **Modes replace plan/act as a general system**: default modes Code / Architect / Ask / Debug plus unlimited **custom modes**, defined in YAML/JSON:
  - Project-level: `.roomodes` (workspace root)
  - Global: `custom_modes.yaml` / `custom_modes.json` in extension settings storage
  - Properties: `slug`, `name`, `description`, `roleDefinition`, `groups` (toolsets + file-glob restrictions), `whenToUse`, `customInstructions`; same-slug project modes fully override globals; per-mode "sticky model" memory ([Custom Modes docs](https://roocodeinc.github.io/Roo-Code/features/custom-modes/))
- **Orchestrator mode ("Boomerang Tasks")**: parent mode spawns subtasks into other modes via the `new_task` tool, guided by `whenToUse`; results aggregated back.
- Extra providers beyond Cline's set historically (e.g., Chutes, Glama, Unbound, native Vertex/Azure variants); own `roo.*` VS Code settings namespace instead of `cline.*`.
- Official migration recommendation post-archive: move to Cline; Kilo reads `.roomodes`.

### Kilo Code (Kilo-Org/kilocode)

- Started as a superset fork of Roo; current generation replaces Roo's YAML custom modes with **Markdown agent files with YAML frontmatter**: `.kilo/agent/*.md` project-side, markdown agents in globalStorage globally; a migration wizard converts `.roomodes` and `custom_modes.yaml` on launch ([Roo→Kilo migration guide](https://kilo.ai/articles/roo-to-kilo-migration-guide)).
- **Orchestrator/Boomerang** replaced by native subagent delegation from any full-tool agent; legacy v5.x pins retain Orchestrator, Memory Bank, Qdrant codebase indexing.
- Keeps `.kilocodemodes` compatibility path and its own globalStorage ID; MCP config analogous to Cline's but under Kilo's storage directory ([Kilo MCP docs](https://kilo.ai/docs/automate/mcp/overview)).

---

## Source Index

- Config layout/env: https://docs.cline.bot/getting-started/config
- Authorization/provider menu: https://docs.cline.bot/getting-started/authorizing-with-cline
- OpenAI Compatible fields: https://docs.cline.bot/provider-config/openai-compatible
- Anthropic/Claude Code: https://docs.cline.bot/provider-config/anthropic
- Bedrock variants: https://docs.cline.bot/provider-config/aws-bedrock/{api-key,iam-credentials,cli-profile}
- Rules: https://docs.cline.bot/customization/cline-rules · .clineignore: https://docs.cline.bot/customization/clineignore
- MCP paths: IONOS doc + https://docs.cline.bot/getting-started/config
- Auto-approve/YOLO: https://docs.cline.bot/features/auto-approve · Checkpoints: https://docs.cline.bot/core-workflows/checkpoints · Plan/Act: https://docs.cline.bot/core-workflows/plan-and-act
- SDK providers: https://docs.cline.bot/sdk/model-providers.md · llms.txt index: https://docs.cline.bot/llms.txt
- Extension settings keys: cline/cline repo `src/package.json` (`contributes.configuration`)
- Roo modes: https://roocodeinc.github.io/Roo-Code/features/custom-modes/ · Kilo deltas: https://kilo.ai/articles/roo-to-kilo-migration-guide
