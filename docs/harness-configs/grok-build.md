# Grok Build — Configuration Reference (xai-org/grok-build)

Compiled from primary sources on 2026-08-25. Sources:
- https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/05-configuration.md
- https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/02-authentication.md
- https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/11-custom-models.md
- https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/14-headless-mode.md

Rust-based agent harness/TUI ("grok") from xAI with ACP support (`grok agent stdio`), headless mode, and rich config layering.

## 1. Config files & file locations

| Path | Purpose |
|---|---|
| `~/.grok/config.toml` | Main configuration |
| `~/.grok/pager.toml` | TUI appearance (terminal, animation, prompt, scrollback, block styling) |
| `~/.grok/auth.json` | Credentials (auto-managed) |
| `~/.grok/sessions/`, `memory/`, `skills/`, `plugins/`, `agents/`, `lsp.json`, `logs/` | User-scoped state |
| `.grok/config.toml` (project) | Project-scoped MCP servers, plugins, permission rules |
| `.grok/skills/`, `plugins/`, `agents/`, `hooks/`, `lsp.json` (project) | Project-scoped extensions |

### Config precedence
Documented order: defaults → user `config.toml` → **env overlay** (`GROK_CONFIG` / `GROK_CONFIG_PATH`) → managed/enterprise layers (`requirements.toml` / MDM wins over everything).

### Env-config injection (wrapper-friendly!)
- **`GROK_CONFIG`** — inline JSON object deep-merged on top of `config.toml` (only overrides keys it sets). Designed exactly for harnesses/ACP clients launching `grok agent stdio` without touching files.
- **`GROK_CONFIG_PATH`** — additional JSON/TOML file overlay; `GROK_CONFIG` wins if both set.
- ⚠️ Security-confined: only allowlisted soft settings pass (`models`, `features`, narrowed `toolset`, `shell_environment_policy` filters). Cannot inject env into tool subprocesses or change auth/network/trust.

## 2. Environment variables

| Var | Effect |
|---|---|
| `XAI_API_KEY` | API key from console.x.ai |
| `GROK_HOME` | **Config-dir override** (default `~/.grok`) — the multi-instance knob |
| `GROK_CLI_CHAT_PROXY_BASE_URL` | Override API proxy base URL (gateways/proxies) |
| `GROK_OIDC_ISSUER` / `GROK_OIDC_CLIENT_ID` | Customer SSO via OIDC |
| `GROK_AUTH_PROVIDER_COMMAND` / `_LABEL` / `GROK_AUTH_TOKEN_TTL` / `GROK_AUTH_EARLY_INVALIDATION_SECS` | External auth provider binary contract (token on stdout) |
| `GROK_MEMORY` / `GROK_SUBAGENTS` / `GROK_WORKFLOWS` / `GROK_WEB_FETCH` | Feature toggles (`1`/`0`) |
| `GROK_WEB_FETCH_ALLOW_LOCAL` | Allow web_fetch to loopback only |
| `GROK_AGENT` | Custom agent definition path/name |
| `GROK_SANDBOX` | Sandbox profile: off/workspace/devbox/read-only/strict/custom |
| `GROK_DEFAULT_SELECTED_PERMISSION` | Headless permission control |
| `GROK_RESPECT_GITIGNORE` | Force gitignore filtering 1/0 |
| `GROK_LOG_FILE` + `RUST_LOG` | Logging |
| `GROK_TELEMETRY_ENABLED/_TRACE_UPLOAD/_MIXPANEL_ENABLED/_EXTERNAL_OTEL` | Telemetry |

## 3. Auth methods

1. **Browser login** (default; OAuth-style flow, creds cached in `auth.json`)
2. **API key**: `export XAI_API_KEY="xai-..."`
3. **OIDC SSO** for enterprises: register public client in IdP → set `GROK_OIDC_ISSUER`, `GROK_OIDC_CLIENT_ID`, optional `GROK_CLI_CHAT_PROXY_BASE_URL` for a corporate proxy
4. **External auth provider**: any binary printing tokens to stdout per documented contract (`GROK_AUTH_PROVIDER_COMMAND`), TTL-managed

Credential resolution for models: `[model.*].api_key` > `env_key` > signed-in session token > `XAI_API_KEY`.

## 4. Custom models / third-party endpoints ✅ fully supported

```toml
# ~/.grok/config.toml
[model.my-model]
model = "model-id"                       # id sent to API
base_url = "https://api.example.com/v1"  # ANY OpenAI-compatible endpoint
name = "Display Name"
description = "..."
api_key = "sk-..."                       # OR env_key below
env_key = "OPENROUTER_API_KEY"           # string or array; first non-empty wins
temperature = 0.7
top_p = 0.95
max_completion_tokens = 8192
context_window = 128000                  # drives auto-compact
query_params = { api-version = "2026-07-22" }        # e.g. Azure
env_http_headers = { "X-Tenant" = "TENANT_TOKEN" }   # headers resolved from env at client build
```
Built-in model override: reuse its name as the section key with only the fields you need.

## 5. Headless mode & permissions

```bash
grok -p "prompt"                                  # one-shot
--output-format plain|json|streaming-json|streaming-messages-json
--include-partial-messages                         # raw stream_event deltas
--tools "read_file,grep,list_dir"                  # allowlist tools
--disallowed-tools "web_search,run_terminal_cmd"   # denylist (supports Agent(explore))
--allow "Bash(npm*)" --deny "Bash(sudo*)"          # permission rules
```

## 6. Other notable config sections in `config.toml`
General settings (input mode, default selected permission, vim mode, screen mode, scroll snapping, scrolling), `[toolset]`, authentication, custom models (above), MCP servers, memory, subagents, goal mode/background workflows, skills, harness compatibility, plugins, hints, notifications (+ hooks, terminal support matrix), status line, keyboard shortcuts, telemetry, version pinning, enterprise deployment — plus separate `pager.toml` for appearance and project-scoped `.grok/config.toml`.

## 6a. MCP servers — `config.toml` `[mcp_servers.<name>]` + `grok mcp` CLI ✅ writable

**Live-verified 2026-09-18 against grok 1.0.34** (evidence: `.z-workflow/evidence/live/grok-build/mcp-probe.txt`; the binary also materializes its own user guide at `$GROK_HOME/docs/user-guide/07-mcp-servers.md`). Corrects the earlier corpus gap that recorded MCP as absent.

- **Destination**: user-scope `config.toml` (i.e. `$GROK_HOME/config.toml` — GROK_HOME relocation honored by the MCP writer, probe-verified) under `[mcp_servers.<name>]` tables → dest key `mcp_servers`, name-map shape (same as codex-cli). Project-scope `.grok/config.toml` (cwd → git-root chain, deepest wins) also contributes `[mcp_servers]` but `grok mcp add` only writes it with `--scope project`.
- **stdio form**: `command`, `args`, `env` (table), `enabled` (default `true` — omitted-key entries still load, probe-verified), `startup_timeout_sec`, `tool_timeout_sec`, `tool_timeouts`. **Remote form**: `url`, `headers` (table). `${VAR}` expansion on string fields at load time.
- **CLI (own writer, probe-verified)**: `grok mcp list [--json]`, `grok mcp add <name> [-e K=V…] [--transport stdio|http|sse] [--scope user|project] -- cmd args…` (default user scope; exit 0, prints `File modified: $GROK_HOME/config.toml`), `grok mcp remove <name>` (exit 1 when absent), `grok mcp enable|disable`, `grok mcp doctor`. `enable/disable` persist to `disabled_mcp_servers` / `[mcp_servers.<name>].enabled` in user config.
- **Pickup without restart**: config hot-reload + `/mcps` modal refresh (`r`) — no full restart required (basis for `RestartBehavior::Reload`).
- **Read-side compat sources** (never written by superai): `.mcp.json` (project root), `~/.claude.json`, `~/.cursor/mcp.json` — merged priority config.toml > Claude > Cursor > `.mcp.json`.
- Grok's own writer is not comment-preserving (probe: rewrite dropped a leading `#` comment but kept sibling tables like `[user_config]`); superai's TOML codec is format-preserving and restores comments — moot for correctness either way.
- Interop nuance (probe): grok reads minimal superai-shaped tables (command/args only) and tolerates superai's `disabled = true` marker key without strict-schema rejection, but its native toggle key is `enabled` (default `true`) — so a superai-disabled entry still loads as enabled in grok until flipped via `enabled = false` / `grok mcp disable`.

## MULTI-INSTANCE WRAPPERS

Two clean mechanisms:
```bash
#!/usr/bin/env bash
# grok-xai: direct xAI, isolated state
export GROK_HOME="$HOME/.grok-homes/xai"
exec grok "$@"
---
#!/usr/bin/env bash
# grok-openrouter: OpenRouter via custom model + env-injected config
export GROK_HOME="$HOME/.grok-homes/or"
export OPENROUTER_API_KEY="sk-or-..."
export GROK_CONFIG='{"models":{"custom":{"base_url":"https://openrouter.ai/api/v1","env_key":"OPENROUTER_API_KEY","model":"anthropic/claude-sonnet-4"}}}'
exec grok "$@"
```
(`models` is inside the env-overlay allowlist; for full custom-model blocks use `config.toml` per `GROK_HOME` instead.)

## Sources
All four URLs above, fetched 2026-08-25. MCP section additionally from the binary-shipped user guide `crates/codegen/xai-grok-pager/docs/user-guide/07-mcp-servers.md` (same repo path), cross-checked against the live binary 2026-09-18 (grok 1.0.34; probe transcript in `.z-workflow/evidence/live/grok-build/mcp-probe.txt`).
