# Nanocoder — Configurable Options Reference

Nanocoder (`@nanocollective/nanocoder`) is an open-source, local-first terminal coding agent by the Nano Collective. Bring-your-own-model: any OpenAI-compatible API, native Anthropic/Gemini/Copilot via `sdkProvider`, or fully local runners.
Primary sources: [README](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/README.md) · [docs/configuration/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/index.md) · [providers/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/index.md) · [preferences.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/preferences.md) · [mcp-configuration.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/mcp-configuration.md) · [features/custom-tools.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/features/custom-tools.md). All verified against `main` on 2026-08-25.

> **Naming note:** there is **no `providers.json`** in current Nanocoder. Providers are configured inside **`agents.config.json`** (under `"nanocoder": {"providers": [...]}`), and user preferences live in **`nanocoder-preferences.json`** (not `preferences.json`). Docs: [configuration/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/index.md).

---

## 1. Config files & directory layout

### Lookup order (first found wins) — [configuration/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/index.md)

| Priority | File | Location |
|---|---|---|
| 1 (highest) | Project | `./agents.config.json` (current working directory) |
| 2 | User/global | Linux: `~/.config/nanocoder/agents.config.json` · macOS: `~/Library/Preferences/nanocoder/agents.config.json` · Windows: `%APPDATA%\nanocoder\agents.config.json` |
| override-all | `NANOCODER_CONFIG_DIR` set | Only `<dir>/agents.config.json` is read; project-level and home lookups are **skipped entirely** |

- Linux path respects `XDG_CONFIG_HOME`. `/setup-config` lists all config files and opens one in `$EDITOR`.
- Project-level wins over global whenever both exist.

### Provider schema — [providers/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/index.md)

Providers are an **array of named entries** under the `nanocoder.providers` key:

```json
{
  "nanocoder": {
    "providers": [
      {
        "name": "...",              // display name shown in /model picker
        "baseUrl": "http(s)://.../v1",
        "apiKey": "optional",       // not needed for local providers / Copilot
        "models": ["model-a"],      // list offered by /model
        "caCertPath": "/path/ca.pem",           // trust private/self-signed TLS
        "contextWindow": 32768,                 // default tokens for all models
        "contextWindows": {"model-a": 131072},  // per-model overrides
        "sdkProvider": "openai-compatible",     // | google | anthropic | github-copilot
        "organizationId": "...",                // OpenAI org (optional)
        "disableTools": false,                  // kill tool calling per-provider
        "disableToolModels": ["model-b"],       // ...or per model
        "requestTimeout": 120000,               // ms; -1 disables
        "socketTimeout": 120000,                // ms; -1 disables
        "connectionPool": {                     // optional
          "idleTimeout": 4000,
          "cumulativeMaxIdleTimeout": 600000
        }
      }
    ]
  }
}
```

Context-limit resolution order ([configuration/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/index.md)): `/context-max` or `--context-max` session override → provider `contextWindows[model]` → provider `contextWindow` → `NANOCODER_CONTEXT_LIMIT` env → models.dev metadata → built-in Ollama fallback map.

### App-level settings (`nanocoder` key in `agents.config.json`) — [configuration/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/index.md)

- `nanocoder.autoCompact` — `{enabled:true, threshold:60 (50–95 %), strategy:"llm"|"mechanical", mode:"conservative", notifyUser:true}`
- `nanocoder.sessions` — `{autoSave:true, saveInterval:30000, maxSessions:100, maxMessages:1000, retentionDays:30, directory:""}`
- `nanocoder.headless.maxTurns` — default 200 LLM turns for `run --plain`/ACP before forcing a tool-free final answer
- `nanocoder.paste.singleLineThreshold` — default 800 chars (stored in preferences file)
- `nanocoder.defaultMode` — `"normal"|"auto-accept"|"yolo"|"plan"`; `--mode` CLI always wins; `run` defaults to auto-accept regardless
- `nanocoder.alwaysAllow: [...]` — tool ids exempt from approval prompts (also applies to non-interactive runs)
- `nanocoder.disabledTools: [...]` — removes tools everywhere (chat, subagents, /tune profiles); model is told they don't exist
- `nanocoder.systemPrompt` — `{mode:"replace"|"append", content:"..."}` or `{file:"./.nanocoder/system-prompt.md"}`
- `nanocoder.nanocoderTools.webSearch.apiKey` — Brave Search API key enables the built-in `web_search` tool (supports `${VAR}` substitution)

### Preferences — `nanocoder-preferences.json` — [preferences.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/preferences.md)

Same hierarchy as above: project-level `./nanocoder-preferences.json` overrides user-level in the platform config dir (`~/.config/nanocoder/nanocoder-preferences.json` on Linux). Auto-saved fields:

| Field | Meaning |
|---|---|
| `lastProvider`, `lastModel`, `providerModels` | last-selected provider/model, remembered per provider |
| `selectedTheme`, `titleShape`, `nanocoderShape` | UI appearance |
| `trustedDirectories` | dirs approved through the first-run trust disclaimer |
| `lastUpdateCheck` | update-check throttle timestamp |
| `alternateScreen` | `true` → start fullscreen (`--alt-screen`/`--no-alt-screen` override per run) |
| `nanocoder.paste.singleLineThreshold` | paste placeholder threshold (default 800) |
| `reasoningExpanded` | show full reasoning traces (toggle Ctrl+R) |
| `nanocoder.notifications.*` | `{enabled:false, sound:false, events:{toolConfirmation,questionPrompt,generationComplete}}` |

Delete any `nanocoder-preferences.json` to reset. Internal data (usage stats) lives separately in `$XDG_DATA_HOME/nanocoder` or `~/.local/share/nanocoder` (macOS: `~/Library/Application Support/nanocoder`; Windows: `%APPDATA%\nanocoder`), overridable via `NANOCODER_DATA_DIR`.

### Custom tools directories — [features/custom-tools.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/features/custom-tools.md)

| Dir | Scope |
|---|---|
| `.nanocoder/tools/*.md` (project root) | project tools; travel with repo; **override personal tools by name** |
| `~/.config/nanocoder/tools/*.md` | personal tools across projects |

One markdown file = one tool (YAML frontmatter: `name` snake_case, `description`, typed `parameters`, `approval: never|always|destructive`, `read_only`, `timeout_ms` ≤300000, `cwd`, `env`, `shell`); body is a shell script templated with `{{ param }}` / `{{# param }}…{{/ param }}` sections, shell-quoted against injection. Scaffold with `/tools create <name>`; list with `/tools`.

---

## 2. Environment variables

### General — [configuration/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/index.md)

| Variable | Purpose |
|---|---|
| `NANOCODER_CONFIG_DIR` | Override global config dir; **skips all other config lookups** |
| `NANOCODER_DATA_DIR` | Override internal data dir (usage stats) |
| `NANOCODER_CONTEXT_LIMIT` | Default context limit (tokens) fallback; `--context-max` flag takes priority |
| `NANOCODER_INSTALL_METHOD` | Force install detection (`npm`,`homebrew`,`nix`,`unknown`) |
| `NANOCODER_DEFAULT_SHUTDOWN_TIMEOUT` | Graceful shutdown ms (default 5000) |
| `NANOCODER_MAX_TURNS` | Max turns for headless runs (`--plain`, ACP); overrides `nanocoder.headless.maxTurns` |

### Provider / MCP JSON overrides (highest precedence over any config file)

| Variable | Format |
|---|---|
| `NANOCODER_PROVIDERS` | Inline JSON: direct array `[{"name":"my-provider","baseUrl":"http://localhost:1234/v1","models":["model-1"]}]` **or** wrapper `{"nanocoder":{"providers":[...]}}` |
| `NANOCODER_PROVIDERS_FILE` | Path to a JSON file with the same content (used if `NANOCODER_PROVIDERS` unset) |
| `NANOCODER_MCPSERVERS` | Inline JSON: direct array **or** `{"mcpServers":{...}}` wrapper |
| `NANOCODER_MCPSERVERS_FILE` | Path to a JSON file with same content |

([configuration/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/index.md), [providers/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/index.md), [mcp-configuration.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/mcp-configuration.md))

### Logging

`NANOCODER_LOG_LEVEL` (`trace|debug|info|warn|error|fatal`), `NANOCODER_LOG_TO_FILE`, `NANOCODER_LOG_DISABLE_FILE`, `NANOCODER_LOG_DIR`, `NANOCODER_LOG_TRANSPORTS`, `NANOCODER_CORRELATION_ENABLED`, `NANOCODER_CORRELATION_DEBUG` — details in [logging.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/logging.md).

### Provider API keys

There is **no fixed per-provider env var**; keys are referenced *from* config via substitution. Any string field in provider and MCP configs supports `$VAR`, `${VAR}`, `${VAR:-default}`, applied recursively ([configuration/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/index.md)). Variables come from the shell environment or a `.env` file in the working directory (see repo `.env.example`). Example: `"apiKey": "${OPENROUTER_API_KEY}"` ([providers/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/index.md)). Local providers need no key at all. Related third-party vars appear only in troubleshooting tips, e.g. Ollama's own `OLLAMA_NUM_CTX=32768` to enlarge context ([providers/ollama.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/ollama.md)).

---

## 3. Provider worked examples

### Ollama (local, no key) — [providers/ollama.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/ollama.md)
```json
{ "nanocoder": { "providers": [
  { "name": "Ollama", "baseUrl": "http://localhost:11434/v1", "models": ["qwen3-coder"] }
] } }
```
Serve with a big context: `OLLAMA_NUM_CTX=32768 ollama serve` (default 2048 breaks agentic tool calling).

### LM Studio (local, no key) — [providers/lm-studio.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/lm-studio.md)
```json
{ "nanocoder": { "providers": [
  { "name": "LM Studio", "baseUrl": "http://localhost:1234/v1", "models": ["local-model"] }
] } }
```
Start server from LM Studio's "Local Server" tab; raise Context Length in Settings → Model Settings.

### OpenRouter (cloud) — [providers/openrouter.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/openrouter.md)
```json
{ "nanocoder": { "providers": [
  {
    "name": "OpenRouter",
    "baseUrl": "https://openrouter.ai/api/v1",
    "apiKey": "${OPENROUTER_API_KEY}",
    "models": ["anthropic/claude-4.5-sonnet"],
    "openrouter": {
      "provider": {"sort": "price", "allow_fallbacks": true},
      "reasoning": {"effort": "high"},
      "service_tier": "flex"
    }
  }
] } }
```
Model names are `provider/model-name`. The `openrouter` block (detected case-insensitively by provider name) forwards routing fields: `provider` (order/sort/only/ignore/max_price/zdr…), `reasoning.effort` (`xhigh…none`), `plugins[]`, fallback `models[]`, `service_tier: flex|priority`, `route`, `user`, `extraBody` escape hatch. Always-on; not gated by `/tune`.

### Generic OpenAI-compatible / custom — [providers/custom.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/custom.md), [providers/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/index.md)
```json
{ "nanocoder": { "providers": [
  {
    "name": "My Provider",
    "baseUrl": "https://my-api.example.com/v1",
    "apiKey": "${MY_API_KEY}",
    "caCertPath": "/path/to/internal-ca.pem",
    "models": ["model-name"],
    "requestTimeout": -1,
    "socketTimeout": -1,
    "connectionPool": {"idleTimeout": 30000, "cumulativeMaxIdleTimeout": 3600000}
  }
] } }
```
Any OpenAI-compatible endpoint works; `-1` disables timeouts (recommended pairing both). Other documented presets: llama.cpp, vLLM, MLX Server, LocalAI, llama-swap, Together, Mistral, GitHub Models, Poe, Atlas Cloud, Requesty, OrcaRouter, Z.ai — plus native-SDK providers via `sdkProvider`: `anthropic`, `google`, `github-copilot` (device OAuth), ChatGPT/Codex, Kimi Code, MiniMax, Thesean ([providers/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/index.md)).

Launch selection: `nanocoder --provider openrouter --model google/gemini-3.1-flash run "analyze src/app.ts"` — flags require the provider to already exist in config ([getting-started/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/getting-started/index.md)).

---

## 4. Multi-instance wrappers

Two supported isolation mechanisms:

**A. `NANOCODER_CONFIG_DIR` — full state isolation.** When set, project-level and home-dir lookups are skipped and Nanocoder reads only `<dir>/agents.config.json` ([configuration/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/index.md)). Give each instance its own directory containing its own `agents.config.json`, `nanocoder-preferences.json`, `.mcp.json`, and `tools/` — complete separation of providers, prefs, MCP servers, and personal custom tools.

```bash
#!/usr/bin/env bash
# nanocoder-local — instance on Ollama
export NANOCODER_CONFIG_DIR="$HOME/.config/nanocoder-local"   # own agents.config.json + prefs + .mcp.json
exec nanocoder "$@"
```

```bash
#!/usr/bin/env bash
# nanocoder-cloud — same machine, OpenRouter instance
export NANOCODER_CONFIG_DIR="$HOME/.config/nanocoder-cloud"
# key lives in the cloud dir's agents.config.json as ${OPENROUTER_API_KEY}; keep it out of rc files:
export OPENROUTER_API_KEY="sk-or-..."
exec nanocoder "$@"
```

Companion `~/.config/nanocoder-local/agents.config.json`:
```json
{ "nanocoder": { "providers": [
  {"name":"Ollama","baseUrl":"http://localhost:11434/v1","models":["qwen3-coder"]}
] } }
```

**B. `NANOCODER_PROVIDERS[_FILE]` — provider-only switching without duplicating the config dir.** Highest precedence over every config file; accepts a direct array or the `{"nanocoder":{"providers":[...]}}` wrapper ([providers/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/providers/index.md)). Pair two named providers in one launcher and pick with `--provider`:

```bash
#!/usr/bin/env bash
# nanocoder-switch — two providers, one binary, chosen by first arg
MODE="${1:-local}"; shift || true
if [[ "$MODE" == "cloud" ]]; then
  export NANOCODER_PROVIDERS_FILE="$HOME/.nanocoder-providers-openrouter.json"
  exec nanocoder --provider OpenRouter "$@"
else
  export NANOCODER_PROVIDERS='[{"name":"Ollama","baseUrl":"http://localhost:11434/v1","models":["qwen3-coder"]}]'
  exec nanocoder --provider Ollama "$@"
fi
```

Notes: `NANOCODER_MCPSERVERS[_FILE]` can be switched the same way; precedence everywhere is env vars > project config > global config. `--provider`/`--model` must reference names defined in the active configuration ([getting-started/index.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/getting-started/index.md)).

---

## 5. MCP servers & custom tools

### MCP servers — [mcp-configuration.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/configuration/mcp-configuration.md)

File locations: project root **`.mcp.json`**, and global `.mcp.json` in the platform config dir (`~/.config/nanocoder/` on Linux, `~/Library/Preferences/nanocoder/` macOS, `%APPDATA%\nanocoder\` Windows). Both load together; duplicate names resolve project-first. Env overrides `NANOCODER_MCPSERVERS`/`_FILE` beat both. Inspect with `/mcp`; interactive setup with `/settings mcp` (templates + Ctrl+E opens the file in `$EDITOR`).

```json
{
  "mcpServers": {
    "filesystem": {
      "transport": "stdio",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "./src"],
      "alwaysAllow": ["list_directory", "read_file"]
    },
    "github": {
      "transport": "stdio",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": { "GITHUB_TOKEN": "$GITHUB_TOKEN" }
    },
    "github-remote": {
      "transport": "http",
      "url": "https://api.githubcopilot.com/mcp/",
      "headers": { "Authorization": "Bearer $GITHUB_TOKEN" },
      "timeout": 30000
    }
  }
}
```

- Transports: `stdio` (`command`,`args`,`env`; uvx gets automatic `--native-tls`), `http` (StreamableHTTP: `url`,`headers`,`timeout`), `websocket` (`url`,`timeout`).
- Common fields: `description`, `alwaysAllow` (skip confirmation in normal mode; auto-accept/yolo approve everything anyway), `enabled` (default true), `tags`.
- Env refs `$VAR` / `${VAR}` / `${VAR:-default}` work in `env` blocks and headers; unset vars resolve empty. Use them in version-controlled project files.
- Caveat: `/tune` profiles filter MCP tools — visible to the model only under the `full` profile; the default `auto` profile may switch small local models to `minimal`/`nano` and hide them.

### Custom tools vs MCP

Custom tools (markdown in `.nanocoder/tools/` or `~/.config/nanocoder/tools/`) are the lightweight middle ground: no separate process, body is a shell script with injected parameters; run with your full user privileges (not sandboxed). Use MCP when a tool needs its own process/state or sharing across users ([features/custom-tools.md](https://raw.githubusercontent.com/Nano-Collective/nanocoder/main/docs/features/custom-tools.md)).
