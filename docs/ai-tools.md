# Tool catalog

Tools superai is meant to install, configure, or route traffic to. A working list, not an exhaustive one.

**Provenance.** Licenses and repo status below were read from the GitHub API on 2026-08-26, not from third-party write-ups. Config env vars were checked against each project's own docs or source; where a variable works but isn't documented, it says so. Everything here rots — re-run the check rather than trusting the date.

No hardware requirements, OS floors, or install commands. Those change faster than this file will, and the installer should read them from each project's own source at build time.

One distinction drives routing:

- **Clients** run locally but usually call a hosted model (Claude Code, Cursor, Codex). In scope for install and config.
- **Runtimes** execute open models on your hardware (Ollama, llama.cpp, LM Studio). Also in scope as proxy backends.

## Coding agents and CLIs

| Tool | License | Notes |
|---|---|---|
| Claude Code | proprietary | `CLAUDE_CONFIG_DIR` relocates config — but see the caveat below |
| Codex CLI | Apache-2.0 | `$CODEX_HOME` (default `~/.codex/config.toml`); referenced throughout the config reference but never formally defined there |
| OpenClaw | MIT | `OPENCLAW_HOME` documented, with `OPENCLAW_STATE_DIR` / `OPENCLAW_CONFIG_PATH` taking precedence over it |
| Hermes | MIT | `HERMES_HOME` is the documented profile boundary: config, sessions, memory, skills, logs. Wrapper scripts set it before launch — exactly superai's alias pattern |
| OpenCode | MIT | Repo moved to `anomalyco/opencode` |
| Cline | Apache-2.0 | Ollama and LM Studio work as local backends |
| Kilo Code | MIT | Maintained fork of Roo Code |
| Aider | Apache-2.0 | API or local backends. Last push 2026-05-22 |
| Goose | Apache-2.0 | Now `aaif-goose/goose` under the Agentic AI Foundation |
| Open Interpreter | Apache-2.0 | Now `openinterpreter/openinterpreter` |
| Oh My Pi | MIT | Terminal agent, many providers, custom providers via `models.json` |
| Cursor CLI | proprietary | Same model layer as the desktop app |
| Droid CLI | proprietary | Vendor-hosted, sign-in required |
| Continue | Apache-2.0 | **Wound down.** Cursor acqui-hire June 2026; last release v2.1.0-vscode 2026-06-19, then commits retiring login and issue reporting. Repo is *not* archived and still accepts pushes, so it's forkable |
| Roo Code | Apache-2.0 | **Dead.** Repo archived, final push 2026-05-15. Kilo Code is the fork to use |

### The `CLAUDE_CONFIG_DIR` caveat

This matters more than anything else here, because the alias layer is built on it.

`CLAUDE_CONFIG_DIR` is **not in Claude Code's official environment-variables documentation**. It works — but there are open bugs where it is only partly honored: user memory still loads from `~/.claude/CLAUDE.md`, session renames write there, `install.sh` still populates `~/.claude/downloads`, and config-editing tooling writes to `~/.claude/settings.json` regardless.

So "point the tool at a generated config with one env var" is not yet a clean contract for Claude Code. superai should verify per-file which paths actually move, and expect to write some files in place rather than assume the variable covers everything.

## Desktop apps

| Tool | License | Notes |
|---|---|---|
| Claude Desktop | proprietary | Fixed OS config path, no env override — write the file in place |
| Codex App | proprietary | Hosted sign-in |
| Cursor | proprietary | Hosted models via provider integrations |
| ZCode | proprietary | Z.ai's agentic desktop app around GLM; also talks to Ollama |
| LM Studio | app proprietary, `lms` CLI MIT | GGUF and MLX, ships an OpenAI-compatible local server |
| Jan | Apache-2.0 | Local model app |
| GPT4All | MIT | **Dormant** — last push 2025-05-27 |
| OpenCode Desktop | — | Beta. Bundles an `opencode-cli` sidecar |

## Local runtimes

Proxy backends when traffic should stay on the machine.

| Tool | License | Notes |
|---|---|---|
| Ollama | MIT | The default target. Text, vision, embeddings |
| llama.cpp | MIT | Now `ggml-org/llama.cpp`. GGUF, build locally |
| LocalAI | MIT | Self-hosted, auto-detects CPU/GPU backends |
| llamafile | Apache-2.0 | Now `mozilla-ai/llamafile`. Single-file packaged models |
| KoboldCpp | AGPL-3.0 | Single binary |
| textgen | AGPL-3.0 | Now `oobabooga/textgen`. GGUF plus ExLlamaV3 and Transformers |
| MLX LM | MIT | Apple silicon |
| ExLlamaV2 | MIT | EXL2/GPTQ; wheels are ABI-sensitive. Last push 2026-03-04 |
| TabbyAPI | AGPL-3.0 | API server for ExLlamaV2/V3 |
| MLC LLM | Apache-2.0 | Universal deployment, including browser and mobile |
| OpenVINO GenAI | Apache-2.0 | Intel-optimized |
| ONNX Runtime GenAI | MIT | ONNX-format models |

The two AGPL entries (KoboldCpp, TabbyAPI) are worth a second look before superai bundles or ships anything alongside them.

## Serving engines

Concurrency-oriented, mostly Linux and GPU. Low priority — servers you stand up, not tools superai installs.

| Tool | License | Notes |
|---|---|---|
| vLLM | Apache-2.0 | The main open-source serving stack |
| SGLang | Apache-2.0 | LLM and multimodal, OpenAI-compatible API |
| TensorRT-LLM | Apache-2.0 | NVIDIA only |
| Text Generation Inference | Apache-2.0 | **Archived** — final push 2026-03-21. Drop unless something depends on it |

## Self-hosted platforms

UI, RAG, or orchestration on top of a runtime. Out of scope for config generation; in scope for install and as proxy clients.

| Tool | License | Notes |
|---|---|---|
| Open WebUI | custom "Open WebUI License" | Redistribution has branding conditions — read it before shipping. Ollama and OpenAI-compatible APIs, built-in RAG |
| AnythingLLM | MIT | Agents, vector DBs, doc ingestion |
| LibreChat | MIT | Multi-provider, MCP and agents |
| OpenHands | MIT | Now `OpenHands/OpenHands`. Formerly OpenDevin |
| Dify | modified Apache-2.0 | Commercial license required for multi-tenant use. Agentic workflows |
| Langflow | MIT | App builder |
| RAGFlow | Apache-2.0 | RAG engine with agents |
| Flowise | source-available (enterprise dir is commercial) | **Archived** — final push 2026-08-13 |

## What this implies

- **Install/uninstall** — everything here. Needs a per-tool install verb and a presence check. Skip the four dead or dormant entries (Roo Code, Text Generation Inference, Flowise, GPT4All) and treat Continue as frozen.
- **Config and aliases** — only four tools have a config-path env var, and only two of those document it. OpenClaw and Hermes are the clean cases; Claude Code and Codex work but are undocumented, and Claude Code's is leaky. Build the alias layer against Hermes or OpenClaw first, where the contract is written down, then port to Claude Code with per-file verification.
- **Routing** — anything exposing an OpenAI-compatible or Anthropic-style endpoint. Runtimes become local backends, hosted APIs and aggregators remote ones. Both should be config entries, not code.
