# Agent Harness Configuration Bible — Master Index

One document per harness, documenting **every configurable option**: config files (exact paths + schema), complete environment variables, third-party/custom API integration (OpenAI-compatible & Anthropic-compatible endpoints, gateways, local models), and **multi-instance wrapper techniques** (running N isolated instances of the same harness with different providers/keys).

Compiled 2026-08-25 by parallel research agents against live official docs/repos, inline-cited in every file. Where a capability doesn't exist (e.g., no BYO endpoint), the docs say so explicitly.

## The Documents

### Terminal / CLI harnesses
| Harness | File | Isolation knob for multi-instance wrappers |
|---|---|---|
| Claude Code | [claude-code.md](claude-code.md) | `CLAUDE_CONFIG_DIR` |
| Codex CLI (OpenAI) | [codex-cli.md](codex-cli.md) | `CODEX_HOME` (+ `$NAME.config.toml` profiles ≥0.134) |
| Gemini CLI (Google) | [gemini-cli.md](gemini-cli.md) | ⚠️ **Retired 2026-06-18** for AI Pro/Ultra/free Code Assist; enterprise licences still served. `GEMINI_CLI_HOME`. Successor: [antigravity-cli.md](antigravity-cli.md) |
| Antigravity CLI (`agy`, Google) | [antigravity-cli.md](antigravity-cli.md) | none documented — `$HOME` swap workaround |
| OpenCode (Anomaly) | [opencode.md](opencode.md) | `OPENCODE_CONFIG` (layer) / `XDG_CONFIG_HOME` (full incl. auth.json) |
| Crush (Charmbracelet) | [crush.md](crush.md) | per-project `crush.json`/crushrc + XDG dirs |
| Aider | [aider.md](aider.md) | `--config` / `--env-file` explicit paths |
| Goose (Block→Linux Fdn) | [goose.md](goose.md) | `GOOSE_PATH_ROOT` |
| Amp (Sourcegraph) | [amp.md](amp.md) | `AMP_API_KEY` + `AMP_SETTINGS_FILE` per instance |
| Qwen Code (Alibaba) | [qwen-code.md](qwen-code.md) | settings-path relocation vars (`QWEN_CODE_*_PATH`) |
| Kimi Code CLI (Moonshot) | [kimi-cli.md](kimi-cli.md) | `KIMI_CODE_HOME` |
| Grok Build (xAI) | [grok-build.md](grok-build.md) | `GROK_HOME` (+ `GROK_CONFIG` JSON overlay!) |
| Mistral Vibe | [mistral-vibe.md](mistral-vibe.md) | `VIBE_HOME` |
| iFlow CLI | [iflow-cli.md](iflow-cli.md) | Gemini-fork lineage — see file |
| Warp Agent Mode | [warp.md](warp.md) | GUI-bound; honest limits documented |
| Amazon Q Developer CLI | [amazon-q-cli.md](amazon-q-cli.md) | ⚠️ **Sunsetting** — new signups blocked 2026-05-15, EOS 2027-04-30. Superseded by [kiro.md](kiro.md) |
| Kiro CLI (AWS) | [kiro.md](kiro.md) | `KIRO_HOME` + AWS credential vars. No BYO endpoint |
| GitHub Copilot CLI | [copilot-cli.md](copilot-cli.md) | `COPILOT_HOME` + `COPILOT_GITHUB_TOKEN` (GA BYOK via `COPILOT_PROVIDER_BASE_URL`) |
| Junie CLI (JetBrains) | [junie-cli.md](junie-cli.md) | `JUNIE_HOME` (BYOK + credits both supported) |
| Forge (ForgeCode) | [forge.md](forge.md) | `FORGE_CONFIG` dir switch |
| Kode CLI | [kode.md](kode.md) | `KODE_CONFIG_DIR` (also reads `CLAUDE_CONFIG_DIR`) |
| Pi (Earendil Works) | [pi.md](pi.md) | `PI_CODING_AGENT_DIR` |
| gptme | [gptme.md](gptme.md) | workspace separation + `GPTME_LOGS_HOME` |
| Nanocoder | [nanocoder.md](nanocoder.md) | `NANOCODER_CONFIG_DIR` / `NANOCODER_PROVIDERS_FILE` |
| Trae Agent (ByteDance) | [trae-agent.md](trae-agent.md) | `--config-file` + `TRAE_CONFIG_FILE` |
| Plandex 2 | [plandex.md](plandex.md) | env-driven provider switching; per-server `PLANDEX_API_HOST` |

### IDE / editor-integrated agents
| Harness | File | Wrapper notes |
|---|---|---|
| Cursor (IDE + CLI) | [cursor.md](cursor.md) | `CURSOR_CONFIG_DIR`; BYOK limited (chat-only), agent needs Cursor backend |
| Cline | [cline.md](cline.md) | VS Code `--user-data-dir` isolation + `CLINE_DATA_DIR` |
| Roo Code (archived 5/2026) | [roo-code.md](roo-code.md) | same isolation pattern; successor = Kilo |
| Kilo Code | [kilo-code.md](kilo-code.md) | kilo.jsonc global+project; CLI shares OpenCode lineage config |
| Continue.dev (now Cursor's) | [continue-dev.md](continue-dev.md) | global vs workspace `.continue/` layers; `cn` headless CLI |
| Zed + external agents via ACP | [zed-acp.md](zed-acp.md) | register wrapper scripts as separate `agent_servers` entries |
| Windsurf | [windsurf.md](windsurf.md) | no env override — `--user-data-dir` IDE isolation; managed-backend limits documented |
| Auggie (Augment) | [auggie.md](auggie.md) | proprietary context engine; headless `--print` |
| Hermes Agent (Nous) | [hermes-agent.md](hermes-agent.md) | `HERMES_HOME` + `--profile`; local-install-verified `[L]` tags |
| OpenClaw | [openclaw.md](openclaw.md) | `OPENCLAW_HOME` / `OPENCLAW_CONFIG_PATH`. Daemon, so instances mean running services |

### Self-hosted / cloud platforms & frameworks
| Harness | File | Wrapper notes |
|---|---|---|
| OpenHands | [openhands.md](openhands.md) | V0 TOML vs V1 env model; `OH_PERSISTENCE_DIR`, docker-per-instance |
| SWE-agent (Princeton) | [swe-agent.md](swe-agent.md) | YAML config files + batch instances w/ different settings |
| Factory Droid | [factory-droid.md](factory-droid.md) | `~/.factory/settings.json` + `customModels` OpenAI-compatible blocks |
| Copilot Coding Agent | [copilot-cli.md](copilot-cli.md) | cloud side has NO BYO endpoint (documented) |
| Orchestrators: Vibe Kanban · Conductor · Sculptor | [orchestrators.md](orchestrators.md) | THE meta-wrapper doc: how each GUI injects env/creds into spawned harnesses |

## Universal patterns (the cheat-sheet)

1. **Config-dir relocation** is the standard isolation mechanism: `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GEMINI_CLI_HOME`, `KIMI_CODE_HOME`, `JUNIE_HOME`, `GROK_HOME`, `GOOSE_PATH_ROOT`, `FORGE_CONFIG`, `KODE_CONFIG_DIR`, `NANOCODER_CONFIG_DIR`, `VIBE_HOME`, `COPILOT_HOME`, `HERMES_HOME`, `PI_CODING_AGENT_DIR`, `CURSOR_CONFIG_DIR`, `OPENCLAW_HOME`, `KIRO_HOME`. Relocate → run its auth flow once → you have an independent instance.
2. **Env beats config for providers** almost everywhere: every harness that speaks OpenAI-compatible takes `<PROVIDER>_API_KEY` + some base-URL override (`ANTHROPIC_BASE_URL`, `OPENAI_BASE_URL`, `GOOGLE_GEMINI_BASE_URL`, `GROK_CLI_CHAT_PROXY_BASE_URL`, `COPILOT_PROVIDER_BASE_URL`, `OPENROUTER_API_KEY`…). Wrappers just export different values before `exec`.
3. **Anthropic-compatible endpoints are a de-facto standard too** — Claude Code (`ANTHROPIC_BASE_URL`+`ANTHROPIC_AUTH_TOKEN`), Kimi Code (`anthropic` provider with custom `base_url`), Crush/Kilo/Cline ("anthropic" provider type) all repoint at GLM/OpenRouter-style Anthropic-format gateways.
4. **Inline config injection** where supported: Codex `CODEX_CONFIG`, Grok `GROK_CONFIG` (JSON deep-merge), OpenCode `OPENCODE_CONFIG_CONTENT`, Amp `AMP_SETTINGS_FILE`, Nanocoder `NANOCODER_PROVIDERS_FILE`.
5. **IDE extensions can't be env-isolated** — wrap VS Code itself: `code --user-data-dir <dir>` gives fully separate extension state (Cline/Roo/Kilo pattern).
6. **Orchestrators are prebuilt wrappers**: Vibe Kanban injects per-profile `ANTHROPIC_BASE_URL`/`AUTH_TOKEN`; Conductor overrides executable paths + Bedrock/Vertex; Sculptor passes env through to containerized Claude Code sessions — see orchestrators.md before rolling your own.

Start here if you want "same harness, two providers side by side": pick the harness's file above → §MULTI-INSTANCE WRAPPERS has a ready script.
