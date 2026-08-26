# Goal

An all-in-one configurator for every AI tool I use. Rust, local only, one binary.

It configures agents. It is not an agent, not a chat client, not a cloud service.

## The core idea

The unit is an **instance**: a named, isolated setup of a harness with its own config, API key, provider, skills, plugins, and MCP servers. "Claude Code for work", "Claude Code on GLM", and my plain everyday Claude Code are three instances, not three tools.

It manages my **existing installs too**. The default `~/.claude` is a managed target like any wrapper instance — that is the point, not a nice-to-have. If I want every Claude Code I own to drop from Opus back to Sonnet, that is one change in one place, not N hand-edits across N directories.

## What superai stores

Two kinds of data, and only one of them is off-limits.

**Harness config is theirs.** `~/.claude-glm/settings.json` belongs to Claude Code. superai reads it fresh, edits, writes back. It never keeps its own copy of `model` or `baseUrl` and reads that instead.

**Instance records are superai's.** Which instances exist, where their config dirs are, which wrapper points at them, which template each came from and at what version. Claude Code has never heard of any of it, so superai's file is the only copy — nothing to go stale, nothing to conflict with.

`claude-multi` already does exactly this: `~/.claude-multi/config.json` holds `name`, `configDir`, `binaryPath`, `providerTemplate` for 13 instances, and mirrors nothing out of the harness configs. superai keeps that shape, and goes further on two things:

- **Template version per instance,** separate from the tool's own version. `providerTemplate: "glm"` alone can't answer "is this instance behind?" — that needs the template version it was built from.
- **Drift scan.** Right now `~/.claude-aaa`, `~/.claude-abogo`, `~/.claude-claude-g2` and `~/.claude-tester` are real config dirs on disk with no wrapper and no record in any tool. superai scans, lists what it finds unmanaged, and offers to adopt or remove — leaving alone anything another tool is actively managing. Owning existing installs means finding them, not just the ones it made.

## Templates

A **template** is a versioned preset that turns "harness + provider" into a working instance in one click. Kilo Code on GLM. Kilo Code on MiniMax. Claude Code on GLM. Pick the template, name it, done.

A new instance starts as a **mirror of an existing one, then isolated**. It inherits a working setup rather than booting empty — no re-adding skills, no re-approving permissions, no starting from a blank `settings.json`. From the moment it exists it diverges: its own config dir, its own key, its own provider, and nothing it does leaks back into what it was copied from. `claude-multi` does this and it is the right model.

A template carries:

- the config the harness needs for that provider — base URL, auth style, model list, defaults
- the capability map (below)
- a version

Templates update. When GLM ships a new model I update the template, and every instance built from it shows *"new version available"* — with a diff of what actually changed in the defaults, so a bumped context window or a swapped default model is visible before I accept it. Updating is my choice per instance, never automatic.

Templates are **not baked into the binary** — a new GLM model must not require shipping a superai release. For now they live in a GitHub repo that superai fetches. How they are distributed properly is still open; that they are separate from the binary is not.

## Capabilities

A capability is not present or absent. It is **native, substituted, or absent**, and it depends on the harness *and* the provider together.

Claude Code's web search on Anthropic is native — a client-side tool. Point the same Claude Code at GLM and that tool does not work, but GLM does search server-side, so the capability still exists, just satisfied differently. Vision has the same shape. So does computer use.

Core models that matrix and exposes capabilities upward. The interface asks "can this instance search the web", never "is this harness Claude Code".

## What it does

**Owns the config files.** Per-harness, version-aware. Parses, edits, writes back. Only ever models the keys superai writes; everything else round-trips untouched. This is the whole product right now — everything below is downstream of getting parsing right.

**Never caches, never claims to be the source of truth.** `~/.claude/settings.json` is a plain editable file. VS Code edits it. Nano edits it. The harness itself edits it. So does another machine over a synced folder. Any shadow copy superai kept would be wrong the moment something else touched the file, and acting on a stale copy means silently destroying someone's edit. Disk is the truth. superai reads fresh on every operation, writes back preserving every key it doesn't model, and holds no state between operations.

**Backs up before every write.** Non-negotiable, and the reason the no-cache rule is safe: if a write goes wrong, or I want yesterday's config back, the previous version is on disk to restore.

**Generates wrappers.** Config-dir relocation where the harness supports it (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GOOSE_PATH_ROOT`, …), `--user-data-dir` for IDE extensions, in-place writes for fixed-path GUIs. See `harness-configs/`.

The wrapper's name is mine to choose. `claude-glm` is one convention, not the rule — an instance can be named anything, with or without the harness as a prefix, so the command I type can be `glm` or `work` if that's what I want.

**Targets every harness in `harness-configs/`.** Try the documented isolation knob, try the workarounds, and if a harness genuinely can't be isolated, say so and support it single-instance. No harness gets dropped for being awkward until it's been tried.

**Manages providers.** Base URLs, model lists, health, and the API key. Adding a provider is a config entry, not a code change.

**Manages skills.** One registry, and how a skill reaches an instance is my choice per instance: symlink the whole registry directory, symlink specific skills, or copy specific skills. Enable and disable per harness and per instance.

**Installs and uninstalls harnesses,** with detection for what's already there. [toride](https://github.com/freeoxide/toride) is a TUI app, but underneath it is modular crates over `duct` for running CLIs and `mise` for installing them. superai depends on those crates, not on toride itself.

**Raw editors.** A real TOML editor and JSON editor with validation, on the same files the tools read.

## Secrets

There is no secret store, and no OAuth support.

All that's needed is an API key, and every harness already writes its key to disk in its own config. Adding a vault on top of that protects nothing. superai writes the key where the harness expects it, and that's the end of it.

## Relationship to claude-multi

superai does not replace `claude-multi` and does not import from it. They coexist — different tools with different scope, both managing Claude Code instances on the same machine without stepping on each other.

What superai takes from it is the design: mirror-then-isolate instance creation, a records file that mirrors nothing out of the harness config, and `providerTemplate` recorded per instance — because providers genuinely diverge on models, release cadence, and deprecation timelines, and an instance has to remember which one it was built for.

## Layers

1. **Config** — per-harness, version-aware. Owns each harness's schema, paths, and migrations across breaking changes.
2. **Core** — instances, templates, capabilities, skills, install, backups, mutation. Exposes capabilities upward, never harness identity.
3. **Interface** — pluggable. GPUI, TUI, CLI — whichever comes first is a later decision, so no interface types leak below this layer.

## Order of work

**First, the filesystem layer, until it is boring.** Create, edit, remove, back up, restore — on real harness config files, round-tripping every key superai doesn't model. Same bar for skills: install, update, remove, symlink the registry, symlink one skill, copy one skill. Not "works on my machine on Claude Code" — stable, correct, and tested across harnesses.

**Then the rest of core:** instances, templates and their versioning, capabilities, install and uninstall.

**Interface last, and only then.** Which toolkit it is can be decided when there is something to put behind it. A GUI over a half-working config layer is worth nothing, and picking the toolkit early only invites its types to leak downward.

## Routing

superai does not proxy. Routing and wire translation belong to [llmproxy](https://github.com/freeoxide/llmproxy), a separate project. superai's share is writing the config that points a harness at it.

## Not

Not a chat client — it configures the tools, the tools do the work. Not a cloud service — everything stays on the machine. Not an agent.
