# AI tools catalog

Catalog of locally installable AI software that superai is expected to install, configure, and route through its proxy. Working list, not a fixed target. Entries describe what each tool is and how it runs locally so the installer, config, and proxy layers can treat them uniformly.

One distinction matters for routing: a **local-running client** is installed on your machine but may still depend on vendor-hosted models or online sign-in (Cursor, Claude Desktop, Codex App, Droid CLI). A **local-inference runtime** executes open models on your own CPU or GPU (Ollama, llama.cpp, LM Studio). Clients are in scope for install and config. Runtimes are also in scope as provider backends the proxy can route to.

## Coding agents and CLI clients

Things you run from a terminal or IDE to do work against a model. Mostly provider-backed; a few will talk to a local runtime.

| Project | OS | Install | Requirements | Notes |
|---|---|---|---|---|
| OpenCode | macOS, Windows, Linux | Install script, npm/bun/Homebrew | WSL recommended on Windows | Provider-backed via Copilot/OpenAI; MCP local tools supported |
| Claude Code | macOS 13+, Windows 10 1809+, Ubuntu 20.04+, Debian 10+, Alpine 3.19+ | Native install, Homebrew, WinGet | 4 GB+ RAM, x64/ARM64, internet | Reads main config from an env-var-overridable path. This is what superai's alias layer targets |
| Codex CLI | macOS, Windows, Linux | Standalone installer or binary | WSL2 on Windows | Apache-2.0. OpenAI Codex/GPT models |
| Cursor CLI | macOS, Windows, Linux | Cursor CLI docs | unspecified | Same model layer as the desktop app |
| Droid CLI | macOS, Linux, Windows | Factory shell script or npm | `xdg-utils` on Linux, sign-in required | Vendor-hosted Factory agent layer |
| Cline | VS Code-family, Cursor, JetBrains, terminal | Extension or `npm i -g cline` | Provider auth; Ollama/LM Studio as local backends | Apache-2.0. Backend determines model sizes |
| Kilo Code | VS Code, JetBrains, CLI | Extension or `npm i -g @kilocode/cli` | unspecified | Apache-2.0. 500+ models via Kilo Gateway, BYOK |
| Continue | macOS, Windows, Linux | Extension or CLI | Local/offline path documented | Apache-2.0. Model-agnostic. Repo now read-only after Cursor acquisition |
| Aider | macOS, Windows, Linux | Installer or pip | Python 3.8+ | Apache-2.0. Works with API and local backends |
| Goose | macOS, Linux, Windows | Official release/docs | unspecified | Apache-2.0. Now under Agentic AI Foundation / Linux Foundation |
| Open Interpreter | macOS, Windows, Linux | One-line installer or pip | Local Python setup attempted by installer | Works with local and hosted LLMs |
| Oh My Pi | Cross-platform | Shell script, npm, Bun, PowerShell | unspecified | Multi-provider LLM client |
| Roo Code | macOS Apple Silicon, Linux x64/ARM64 | Install script / extension docs | Node.js 20+ | Apache-2.0. Extension shut down 2026-05-15, treat as discontinued |

## Desktop apps and IDE extensions

GUI surfaces. Some run their own local model; some are shells over hosted models.

| Project | OS | Install | Requirements | Notes |
|---|---|---|---|---|
| OpenCode Desktop | macOS, Windows, Linux | Official downloads | Bundles a local `opencode-cli` sidecar server | Beta. Same agent layer as OpenCode |
| Claude Desktop | macOS, Windows | Official download | Online account required | Claude chat/app models |
| Codex App | macOS, Windows | Official download | Online sign-in required | OpenAI Codex/GPT models |
| ZCode | macOS, Windows, Linux beta | Official installer/docs | unspecified | Optimized for GLM-5.2 |
| Cursor | macOS, Windows, Linux | Official site | unspecified | Frontier coding models via provider integrations |
| LM Studio | Apple Silicon macOS, Windows x64/ARM, Linux x64/ARM64 | App + `lms` CLI | macOS 14+, 16 GB RAM recommended (8 GB workable for small models) | MIT CLI. GGUF and MLX. Ships a local OpenAI-compatible server |
| Jan | Windows, macOS, Linux | Official app + GitHub | macOS 13.6+; 8 GB for 3B, 16 GB for 7B, 32 GB for 13B | Llama, Gemma, Qwen, GPT-oss |
| GPT4All | Windows, macOS, Linux | App or `pip install gpt4all` | Python venv recommended | llama.cpp-compatible models; Nomic and SBert embeddings |

## Local model runtimes and packaging

Software that executes open models on your own hardware. These are the backends the proxy routes to when you want traffic to stay local.

| Project | OS | Install | Requirements | Notes |
|---|---|---|---|---|
| Ollama | macOS 14+, Windows 10+, Linux | Package/script | 4 GB+ disk for binary; models can need tens to hundreds of GB; CUDA optional, ROCm v7 on Linux | Llama, Qwen, Gemma, DeepSeek; text, vision, embeddings |
| llama.cpp | Broad, build locally | Clone and build | Varies by quantization and model | GGUF/GGML open LLMs and multimodal |
| LocalAI | Docker/self-host; Linux x86/ARM, NVIDIA ARM64 | Docker or recommended method | Consumer hardware; auto-detects CPU/NVIDIA/AMD/Intel backends | LLMs, image generation, audio |
| textgen | Linux, Windows, macOS | Portable build | Portable builds ship with CUDA, Vulkan, ROCm, CPU-only options | GGUF via llama.cpp; ExLlamaV3 and Transformers backends |
| KoboldCpp | Single-file binary | Release binary | Self-contained distributable | AGPL-3.0. GGML/GGUF models |
| llamafile | Cross-platform single-file | Download or build a `.llamafile` | Depends on bundled model | llama.cpp and whisper.cpp packaged models |
| MLC LLM | Windows, Linux, macOS, browser/mobile | pip in conda env | 6 GB+ VRAM recommended for int4 Llama 3 8B example | Universal deployment engine |
| MLX LM | Apple silicon macOS | Install MLX/MLX LM | Apple silicon, RAM depends on model | Thousands of HF LLMs through MLX; quantization, distributed inference |
| ExLlamaV2 | Consumer GPUs | Matching wheel or JIT build | Wheel must match Python, CUDA, PyTorch ABI | EXL2/GPTQ-style models |
| TabbyAPI | Python-based, follows Python support | Python/uv/conda or Docker | Python 3.x, preferably 3.12 | Official API backend for ExLlamaV2 and V3. Rolling release |
| OpenVINO GenAI | PC/laptop | Package/archive | Optimized for resource-constrained execution | Popular GenAI pipelines on OpenVINO Runtime |
| ONNX Runtime GenAI | Windows, Linux, macOS | `pip install onnxruntime-genai` or source build | Variants for CPU, DirectML, CUDA 11/12 | ONNX-format generative models |

## High-performance serving engines

For when a runtime is not enough and you need to serve many concurrent requests. Mostly Linux and GPU.

| Project | OS | Install | Requirements | Notes |
|---|---|---|---|---|
| vLLM | Linux GPU serving | Wheel or source build | CUDA 12.9 default; wheels for 12.8 and 13.0; Blackwell needs 12.8+ | Major open-source LLM serving |
| SGLang | Single GPU to distributed clusters | pip/source/Docker | Python 3.10+; CUDA 13 images default, cu12/cu129 variants | LLM and multimodal serving, OpenAI-compatible API |
| TensorRT-LLM | NVIDIA GPU environments | Per quickstart | NVIDIA GPU required | LLM and visual-gen inference via TensorRT |
| Text Generation Inference | x86_64 Docker/server | Docker image with `--model-id` | GPU path for common use; ARM64 not officially supported | Llama, Falcon, StarCoder, BLOOM, GPT-NeoX, T5 |

## Self-hosted chat, workflow, and agent platforms

Larger stacks that sit on top of a runtime and add UI, RAG, or workflow orchestration. Usually run in Docker.

| Project | OS | Install | Requirements | Notes |
|---|---|---|---|---|
| OpenHands | Local Docker/npm | npm or Docker | Self-host via Docker/npm; docs recommend local models and document Ollama/LM Studio/SGLang/vLLM examples | Formerly OpenDevin. Self-hosted agent platform |
| AnythingLLM | macOS, Windows, Linux, Docker | Desktop app or self-host container | Desktop/self-host, baseline HW mostly unspecified | MIT. Agents, vector DBs, doc ingestion |
| Open WebUI | Docker/self-host; desktop for Mac, Windows, Linux | Docker or official desktop app | Self-hosted offline; desktop supports offline after first launch | Ollama and OpenAI-compatible APIs; built-in RAG |
| LibreChat | Local server via Docker | Docker self-host | unspecified | Unifies major providers plus MCP/agents |
| Flowise | Local npm or Docker | `npm i -g flowise` then `npx flowise start`, or Docker Compose | unspecified | Model-agnostic workflow builder |
| Langflow | Desktop app plus pip/Docker | `pip/uv install langflow` then `langflow run`, or Docker | unspecified | LLM/vector-store agnostic app builder |
| Dify | Self-host via Docker Compose | Clone release, `cd dify/docker`, copy `.env`, start containers | CPU 2+ cores, RAM 4 GiB+, Docker Compose 2.24+ | Agentic workflows; model sizes depend on backend |
| RAGFlow | Docker/self-host on x86 | Docker quickstart | CPU 4+ cores, 16 GB RAM, 50 GB disk, Docker 24+, Compose 2.26.1+; Python 3.13+ for source path | RAG engine with agent capabilities |

## What this means for superai

The tools split into three concerns the app handles, each with a different shape.

Install and uninstall applies to everything above. Whether the install verb is a desktop installer, an npm global, a pip package, a Homebrew formula, or a Docker image, superai needs the install and uninstall commands for each, and a way to detect whether the tool is already present.

Config and aliases applies to the agents and clients that read settings from an env-var-overridable path. Claude Code is the canonical case. Codex, Codex Desktop, Claude Desktop, OpenClaw, and Hermes follow the same pattern. For each, the app generates the config file and writes the alias that preloads the right settings path before launch.

Routing and proxy applies to providers, aggregators, and anything that talks to a model endpoint. Anything exposing an OpenAI-compatible or Anthropic-style API is a target the proxy can route to, rewrite, or translate between. The runtimes in this doc become local backends; hosted APIs and aggregators (OpenRouter, Fireworks, Novita, GLM, DeepSeek, Xiaomi MiMo, and future ones) become remote backends. Adding either should be a config entry, not a code change.

## Gaps

A working catalog, not an exhaustive one. The ecosystem moves weekly and several projects blend desktop, extension, CLI, self-host, and cloud surfaces in a way that makes strict boundaries fuzzy. License metadata, exact release dates for proprietary apps, and full hardware matrices were the widest gaps; where a field was unknown it is marked unspecified rather than guessed.

A few related projects were left out because they read as companion repos, deployment variants, or transitional surfaces rather than distinct end-user tools. Several cloud-tied local clients were kept in scope because they have a real local install path and superai is expected to install and configure them.

A fully local stack today usually looks like Ollama or llama.cpp/LM Studio/Jan for model execution, optionally Open WebUI or AnythingLLM for UI and RAG, then a coding or automation layer like Cline, Kilo Code, Goose, Continue, or OpenHands depending on whether you want IDE-native, CLI-native, or self-hosted orchestration.
