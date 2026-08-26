# superai

A native desktop app for managing the AI tools I use — providers, configs, installs, and a local proxy that sits in front of all of them. GPUI, so it's one Rust binary on macOS, Linux, and Windows.

**Status: nothing is built yet.** This repo is the design and a catalog of the tools it targets. No code.

## The problem

My AI setup is spread across a pile of config files, env vars, and shell aliases. Switching between Claude Code and Codex means relearning where the settings live. Local runtimes talk to one thing, hosted providers to another, aggregators sit off to the side because wiring them up by hand is annoying.

One app should own all of that.

## The design

**Providers.** Local or hosted, behind one interface: credentials, base URLs, model lists, connection health. Today that's Ollama, OpenRouter, Fireworks, Novita, GLM, DeepSeek, MiMo — but adding one should be a config entry, not a code change.

**A local proxy.** The core of it. Tools point at the proxy instead of at a provider, and the proxy decides where the request actually goes and what shape it takes on the way out — including translating an Anthropic-style call into whatever a provider that doesn't speak it natively expects. One place to change routing, swappable per tool.

**Configs and aliases.** Most harnesses support config-dir relocation via an env var, which is the sanctioned multi-instance mechanism — Claude Code's docs literally give `alias claude-work='CLAUDE_CONFIG_DIR=~/.claude-work claude'` as the example. So the app generates the config files and writes the wrappers that set the right variable before exec.

`docs/harness-configs/` documents the knob for ~40 harnesses: `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GEMINI_CLI_HOME`, `GOOSE_PATH_ROOT`, `HERMES_HOME`, `OPENCLAW_HOME`, and a dozen more. The exceptions matter for scoping: IDE extensions can't be env-isolated and need `code --user-data-dir` instead, and GUI apps like Claude Desktop have a fixed path that has to be written in place.

**Install and uninstall.** A package-manager view for the tools themselves — detect what's present, install, remove — without copying commands out of a readme.

**Skills.** Stored globally, attached per agent. Enable a skill for one tool, disable it for another, without editing five files.

**Raw editors.** A TOML editor and a JSON editor underneath the friendly UI, with validation, editing the same files the tools actually read.

## Why GPUI

Native rendering and one codebase across platforms, in a language I'm comfortable holding API keys and live proxy traffic in. The proxy runs in-process with the UI rather than as a daemon I have to keep alive.

## Not

Not a chat client — it manages tools, the tools do the work. Not a cloud service — credentials stay on the machine.

## Build order

1. App shell and provider connections: one local, one hosted.
2. The proxy: one aggregator passthrough, one format translation.
3. Config generation and wrapper aliases for Claude Code.
4. Extend to the other relocatable harnesses from the descriptor table; handle the fixed-path and IDE cases separately.
5. Install and uninstall.
6. Global skills with per-agent attach/detach.
7. Raw TOML and JSON editors.

[docs/ai-tools.md](docs/ai-tools.md) lists the tools this is meant to cover.
