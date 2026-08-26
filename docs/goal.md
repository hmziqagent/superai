# Goal

An all-in-one configurator for every AI tool I use. Native desktop app, Rust, GPUI. One binary, local only.

## The core idea

The unit is an **instance**: a named, isolated setup of a harness with its own config, credentials, provider, skills, plugins, and MCP servers. "Claude Code for work", "Claude Code on GLM", and my plain everyday Claude Code are three instances, not three tools.

It manages my **existing installs too**, not just new ones. The default `~/.claude` is a managed target like any wrapper instance. Mix and match freely.

## What it does

**Configures harnesses.** Generates the config files and the wrapper scripts that set the right env var before launch. Most CLI harnesses support config-dir relocation (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GOOSE_PATH_ROOT`, …); IDE extensions need `--user-data-dir` isolation instead; some GUI apps have a fixed path and get written in place. See `harness-configs/`.

**Manages skills globally.** One registry. Enable a skill for one harness, disable it for another, turn it off everywhere temporarily. Per-harness *and* per-instance, not one flat list.

**Runs a proxy.** Requests go through a local proxy instead of straight to a provider. One source of truth for routing, swappable per instance, including translating between Anthropic-style and OpenAI-compatible wire formats.

**Third-party integration — the actual point.** Drive capabilities that are normally first-party-only through whatever provider I want. Claude Code's computer use running against a third-party API is the example worth chasing.

**Manages providers.** Local or hosted, one place for credentials, base URLs, model lists, health. Adding one is a config entry, not a code change.

**Installs and uninstalls** the harnesses themselves, with detection for what's already present.

**Raw editors.** A real TOML editor and JSON editor underneath the UI, with validation, editing the same files the tools actually read.

## Layers

1. **Config** — per-harness, version-aware. Owns each harness's schema, paths, capability gates, and migrations across breaking changes. Only ever models the keys superai writes; everything else round-trips untouched.
2. **Core** — per-harness too, because capability surfaces genuinely differ. Owns instances, skills, install, backups, mutation. Exposes capabilities upward, never harness identity.
3. **Proxy** — local listener, routing table, wire translation.
4. **Interface** — GPUI first, so no GPUI types below this layer.

## Not

Not a chat client — it configures the tools, the tools do the work. Not a cloud service — credentials stay on the machine.
