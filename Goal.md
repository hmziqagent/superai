# Goal

A native desktop app for managing every AI tool I actually use. Built with GPUI so it runs the same on macOS, Linux, and Windows. One binary. No Electron.

## The problem

Right now my AI setup is spread across half a dozen config files, a pile of env vars, three terminal aliases I copied from a gist and never fully understood, and skills that live in different folders depending on which agent needs them. Every time I switch from Claude Code to Codex I re-teach myself where the settings live. Local providers like Ollama talk to one thing, cloud providers talk to another, aggregators like OpenRouter sit off to the side because wiring them in by hand is annoying.

I want one app that owns all of that.

## What it does

**Provider integrations.** Any provider, local or hosted. The ones I use today are Ollama, Xiaomi MiMo, GLM, DeepSeek, Fireworks, Novita AI, OpenRouter, but that list is just what is on my desk right now. Adding a new one should be a config entry, not a code change. Credentials, base URLs, model lists, and connection health all live in one place.

**A built-in proxy.** This is the part I actually need. Instead of pointing each tool at a provider directly, requests go through a local proxy the app runs. From there I can rewrite a request on the way out: send it to OpenRouter (or Fireworks, or whatever), or translate an Anthropic-style messages call into whatever shape a provider expects that doesn't speak that format natively. One source of truth for routing, swappable per tool or per task. Same idea for any aggregator or direct provider.

**Settings and aliases.** Claude Code reads its main config from a path you can override with an environment variable. Same shape applies to Codex, Codex Desktop, Claude Desktop, OpenClaw, and Hermes. The app generates those config files and writes the aliases that preload the right settings path before the tool launches. "Work mode" and "personal mode" stop meaning two terminals with different env vars.

**Install and uninstall.** A package manager view for the tools themselves. Install Claude Code, pull Codex, add or remove a model in Ollama, without leaving the app or copying install commands out of a readme.

**Skills management.** Skills live globally and get attached to agents. I can enable a skill for one agent and disable it for another, turn a skill off everywhere temporarily, or add a new one and pick which tools see it. No more editing five files to share one prompt.

**Raw editors.** When the friendly UI is wrong or missing a field, a real TOML editor and a real JSON editor sit underneath. Edit the file the tool actually reads, with validation, so I'm not hand-debugging a trailing comma in a text editor.

## Why GPUI

Native rendering, shared codebase across platforms, and it is Rust, which matters when the app is holding API keys, proxying live traffic, and touching the filesystem. The proxy layer especially benefits from being in the same process as the UI instead of a separate daemon I have to keep alive.

## What it is not

Not another chat client. The app manages tools and config; the tools do the actual work. Not a cloud service either. Credentials and config stay on the machine.

## Rough build order

1. App shell and provider connections, starting with one local and one hosted provider. Add more by config from there.
2. The proxy, with one aggregator passthrough and one format translation.
3. Config generation and aliases for Claude Code.
4. Extend config and aliases to Codex, Codex Desktop, Claude Desktop.
5. Install and uninstall for tools.
6. Global skills with per-agent attach and detach.
7. The TOML and JSON raw editors, wired to the same files the rest of the app reads.
