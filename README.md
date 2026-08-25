# superai

A native desktop app for managing the AI tools I use — providers, configs, installs, and a local proxy that sits in front of all of them. GPUI, so it's one Rust binary on macOS, Linux, and Windows.

**Status: nothing is built yet.** This repo is the design and a catalog of the tools it targets. No code.

## The problem

My AI setup is spread across a pile of config files, env vars, and shell aliases. Switching between Claude Code and Codex means relearning where the settings live. Local runtimes talk to one thing, hosted providers to another, aggregators sit off to the side because wiring them up by hand is annoying.

One app should own all of that.

## The design

**Providers.** Local or hosted, behind one interface: credentials, base URLs, model lists, connection health. Today that's Ollama, OpenRouter, Fireworks, Novita, GLM, DeepSeek, MiMo — but adding one should be a config entry, not a code change.

**A local proxy.** The core of it. Tools point at the proxy instead of at a provider, and the proxy decides where the request actually goes and what shape it takes on the way out — including translating an Anthropic-style call into whatever a provider that doesn't speak it natively expects. One place to change routing, swappable per tool.

**Configs and aliases.** Some tools let you relocate their config with an env var, so the app can generate config files and write aliases that point a tool at the right one before launch — "work mode" and "personal mode" stop meaning two terminals.

The support is uneven, and that shapes the build order. Hermes documents `HERMES_HOME` as a full profile boundary and expects wrapper scripts to set it, which is exactly this pattern; OpenClaw documents `OPENCLAW_HOME` with finer-grained overrides on top. Claude Code's `CLAUDE_CONFIG_DIR` and Codex's `CODEX_HOME` both work but are undocumented, and `CLAUDE_CONFIG_DIR` has open bugs where files still land in `~/.claude` anyway. GUI apps like Claude Desktop have no override at all and need their file written in place. So this layer is per-tool work with verification, not one mechanism applied four times.

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
3. Config generation and aliases, starting with Hermes or OpenClaw where the env-var contract is documented.
4. Port that to Claude Code and Codex, verifying per file which paths actually move; write Claude Desktop's config in place.
5. Install and uninstall.
6. Global skills with per-agent attach/detach.
7. Raw TOML and JSON editors.

[docs/ai-tools.md](docs/ai-tools.md) lists the tools this is meant to cover.
