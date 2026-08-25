# Roo Code — Configurable Options Reference

> **⚠️ ARCHIVED PROJECT (May 2026).** Roo Code (`RooCodeInc/Roo-Code`, VS Code extension ID `RooVeterinaryInc.roo-cline`, command namespace `roo-cline.*`) was archived in May 2026 and no longer receives updates. **Kilo Code** (`kilocode.Kilo-Code`) is the active successor fork built on the same codebase ([Kilo migration guide](https://kilo.ai/articles/roo-to-kilo-migration-guide)). Docs mirrored at `roocodeinc.github.io/Roo-Code/` (snapshot of `docs.roocode.com`) and in the archived repo's `docs/` via raw.githubusercontent.com.

Roo Code is a Cline-descended autonomous coding agent inside VS Code. Its configuration spans five layers: VS Code `settings.json` keys (`roo-cline.*` / legacy `roo.*`), per-provider API profiles stored in extension state/SecretStorage, project-level files (`.roomodes`, `.roo/rules-*`, `.rooignore`, `.roo/mcp.json`), global files under `~/.roo/` (rules, modes) and extension-global storage (`mcp_settings.json`), plus auto-approve/checkpoint settings ([settings management docs](https://roocodeinc.github.io/Roo-Code/features/settings-management/)).

---

## 1. Settings

### 1.1 VS Code settings keys (`roo-cline.*`)

Configured via VS Code `settings.json` or the settings UI (gear icon). Documented reference sections: Command & Execution, Task Management, API & Network, Storage & Import, Code Index, Editor Integration, Rules & Instructions, Debug ([VS Code Settings Reference](https://roocodeinc.github.io/Roo-Code/features/settings-management/#vs-code-settings-reference)). Key entries:

| Setting | Type / Default | Purpose |
|---|---|---|
| `roo-cline.allowedCommands` | string[]; default `["git log","git diff","git show"]` | Commands auto-executable without approval |
| `roo-cline.deniedCommands` | string[]; default `[]` | Always-blocked commands (supports `*`) |
| `roo-cline.commandExecutionTimeout` | number 0–600s; default `0` (= none) | Kill long-running commands |
| `roo-cline.commandTimeoutAllowlist` | string[] | Commands exempt from timeout |
| `roo-cline.autoImportSettingsPath` | path; default empty | Auto-import a settings JSON on every startup — absolute or `~/`-relative paths; silent failure if missing ([Automatic Configuration Import](https://roocodeinc.github.io/Roo-Code/features/settings-management/#automatic-configuration-import)) |
| Code Index group | e.g. embedding/vector DB provider keys | Configurable codebase-indexing backend ([DataCamp overview](https://www.datacamp.com/tutorial/roo-code)) |
| Editor integration | diagnostics, editor context toggles | Feed lints/open-file context into prompts |

Other notable toggles live only in the Roo settings UI (not `settings.json`): system-prompt context toggles ("Include Current Time", "Include Current Cost"), "Collapse thinking messages by default", write-delay under Context Management → Diagnostics ([settings docs](https://roocodeinc.github.io/Roo-Code/features/settings-management/#ui-setting)).

Command-palette equivalents: `roo-cline.importSettings`, custom storage path command ([Command Palette Commands](https://roocodeinc.github.io/Roo-Code/features/settings-management/#command-palette-commands)).

### 1.2 API provider panel

Provider selection happens on the welcome screen and later via the provider dropdown atop the chat panel; API keys go into VS Code SecretStorage ([Connecting your first LLM provider](https://roocodeinc.github.io/Roo-Code/getting-started/connecting-api-provider/)). Providers relevant to BYO endpoints:

- **OpenAI Compatible** — fields: Base URL (any OpenAI-compatible `/v1` endpoint, e.g. LiteLLM/vLLM gateways), API key, model ID. This is the generic escape hatch for third-party gateways.
- **Requesty** — Requesty router API key; routes to many models through one key.
- **OpenRouter** — single key for multi-lab models; the docs' *recommended* starter provider ([connecting-api-provider](https://roocodeinc.github.io/Roo-Code/getting-started/connecting-api-provider/); [providers/openrouter](https://roocodeinc.github.io/Roo-Code/providers/openrouter)).
- **Google Gemini** — Gemini API key, model picker.
- **Ollama** — local base URL (default `http://localhost:11434`), model ID; no key.
- **LM Studio** — local server base URL (default `http://localhost:1234`), model ID; no key.
- **VS Code LM API** — uses VS Code's built-in Language Model API (e.g. Copilot-provided models); no external key.

Common per-request knobs across providers: temperature override, reasoning effort, context window, max tokens, rate-limit/cost settings. Profiles are exported/imported as JSON — **export contains API keys in plaintext** ([Export Settings](https://roocodeinc.github.io/Roo-Code/features/settings-management/#export-settings)).

### 1.3 Per-mode overrides

Each mode remembers its last-used model ("Sticky Models") and persists between sessions, so you can pin e.g. Gemini for Architect and Claude for Code without reconfiguring ([Using Modes](https://roocodeinc.github.io/Roo-Code/basic-usage/using-modes/); [Customizing Modes](https://roocodeinc.github.io/Roo-Code/features/custom-modes/)). Provider-profile selection is therefore effectively per-mode.

## 2. Modes system

Built-in modes: 💻 Code (full tool access), ❓ Ask (`read`,`mcp` only), 🏗️ Architect (`read`,`mcp`, edit restricted to markdown), 🪲 Debug (full access + methodical instructions), 🪃 Orchestrator/Boomerang (no direct tools; delegates via `new_task`). Tool groups are `read`, `edit`, `command`, `mcp` (+ `browser`). Switching: dropdown, slash commands (`/code`, `/ask`, …), `Ctrl/Cmd+.` cycling, accepted suggestions ([Using Modes](https://roocodeinc.github.io/Roo-Code/basic-usage/using-modes/)).

### 2.1 `.roomodes` / `custom_modes.yaml` schema

Custom modes can be created via the Modes view, by asking Roo itself, or hand-written YAML/JSON ([custom-modes](https://roocodeinc.github.io/Roo-Code/features/custom-modes/)). Project-level file: `.roomodes`; global: `custom_modes.yaml` in Roo's global config directory. Top level:

```yaml
customModes:
  - slug: docs-writer        # unique id; ties mode to rules dirs
    name: Documentation Writer
    description: Short UI summary shown in the mode selector
    roleDefinition: Core identity/expertise; start of system prompt
    whenToUse: Optional guidance for Orchestrator/mode-switch decisions
    groups:
      - read
      - - edit               # group with file-restriction regex
        - fileRegex: \.md$
          description: Markdown only
      - command
    customInstructions: Optional behavioral rules appended near prompt end
```

Precedence: project `.roomodes` > global modes > built-ins; defining a mode with a built-in slug overrides it globally or per-project. Modes import/export as portable YAML bundling the mode plus its `.roo/rules-{slug}/` rules ([import/export](https://roocodeinc.github.io/Roo-Code/features/custom-modes/#importexport-modes)). A community Marketplace offers one-click mode installs ([marketplace](https://roocodeinc.github.io/Roo-Code/features/marketplace)).

### 2.2 `.roorules` / `.clinerules`

Rules/instructions hierarchy: project root `.roorules` (and `.roorules-{mode-slug}` for mode-specific rules; `.clinerules` kept as Cline-lineage fallback), plus directory form `.roo/rules/*.md` (global rules dir `~/.roo/rules/`), all concatenated into the system prompt ([Rules & Instructions](https://roocodeinc.github.io/Roo-Code/docs/getting-started/custom-instructions); [Global Rules Directory](https://roocodeinc.github.io/Roo-Code/features/custom-modes/#global-rules-directory)). Mode-scoped directories `.roo/rules-{slug}/*.md` load only when that mode is active.

### 2.3 `.rooignore`

Project-root file using `.gitignore` glob syntax. Matched files are hidden from Roo's read/list/search tools (the agent cannot read them), marked in listings, and excluded from codebase indexing; respects `.gitignore` semantics with negation support. A shield toggle in the chat input lets you "lock" the ignore list so Roo cannot modify it ([RooIgnore docs](https://docs.roocode.com/features/rooignore); behavior discussed in [issue #972](https://github.com/RooCodeInc/Roo-Code/issues/972) and [#5655](https://github.com/RooCodeInc/Roo-Code/issues/5655)).

## 3. Key handling / profiles

- API keys are stored in **VS Code SecretStorage**, not plain files ([migration guide notes keys don't port](https://kilo.ai/articles/roo-to-kilo-migration-guide)).
- Multiple named **API Provider Profiles** can be saved and switched; each profile bundles provider, endpoint, model, temperature, etc. Profiles export via the settings Export button (plaintext keys warning) and merge on Import ([settings-management](https://roocodeinc.github.io/Roo-Code/features/settings-management/)).
- `roo-cline.autoImportSettingsPath` gives declarative profile syncing across machines/team ([auto-import](https://roocodeinc.github.io/Roo-Code/features/settings-management/#automatic-configuration-import)).
- Keyboard shortcuts: `Ctrl/Cmd+.` cycles modes; commands namespaced `roo-cline.*` in `keybindings.json`.

## 4. Multi-instance wrappers

Roo has no env-var config home of its own — all state lives inside VS Code's user-data/profile directories. Isolation is done at the **VS Code level** with `--user-data-dir` (+ optionally `--extensions-dir`), giving each instance separate extensions, SecretStorage keys, and Roo settings:

```bash
#!/usr/bin/env bash
# roo-work: isolated Roo Code instance #1 (own extensions, keys, profiles)
exec code \
  --user-data-dir "$HOME/.vscode-roo-work" \
  --extensions-dir "$HOME/.vscode-roo-work/exts" \
  "$@"
# repeat with another dir for instance #2 → fully independent provider/API configs
```

Because each `--user-data-dir` gets its own SecretStorage and global storage (where `mcp_settings.json` and `custom_modes.yaml` live), this is the only reliable way to run parallel Roo instances with different providers/accounts on one machine. Alternatively use distinct VS Code profiles (UI-level separation, shared process).

## 5. Auto-approve, checkpoints, MCP paths

### Auto-approve granularity
Dropdown quick-permissions (Execute approved commands, Use the browser, Switch modes, Create & complete subtasks, Answer follow-up questions) plus an advanced panel: read operations (with outside-workspace flag), write operations (with configurable write-delay), MCP tools, mode switching, subtasks, command execution, follow-up questions. MCP auto-approve needs both the global toggle *and* per-tool `alwaysAllow`. Terminal allow/deny lists come from `roo-cline.allowedCommands` / `deniedCommands`. Security warning applies: auto-approve bypasses confirmation ([Auto-Approving Actions](https://roocodeinc.github.io/Roo-Code/features/auto-approving-actions/)).

### Checkpoints
Enabled by default; require Git installed but no repo/account. A shadow Git repository snapshots workspace state before each file modification; restore options: Files Only vs Files & Task. Settings: enable/disable checkbox and initialization timeout (10–60s, default 30s) under Settings → Checkpoints ([Checkpoints](https://roocodeinc.github.io/Roo-Code/features/checkpoints/)).

### MCP paths
Two levels ([Using MCP in Roo](https://roocodeinc.github.io/Roo-Code/features/mcp/using-mcp-in-roo/)):
- **Global**: `mcp_settings.json` in the extension's global storage (editable via "Edit Global MCP" button). Applies to all workspaces.
- **Project**: `.roo/mcp.json` at project root (created on demand via "Edit Project MCP"); commit-friendly. **Project config takes precedence** on name collisions.
Schema: `{ "mcpServers": { "<name>": { "command", "args", "cwd", "env", "alwaysAllow": [...], "disabled", "timeout" (1–3600s, default 60), "watchPaths", "disabledTools" } } }`. `${env:VAR}` expansion in `args`. Transports: STDIO, Streamable HTTP, legacy SSE.

## 6. Post-archive guidance → Kilo Code migration

Kilo Code ships an in-extension Migration Wizard (Settings → Migration) covering settings, API keys (re-auth needed — keys don't port), custom modes, and MCP configs; task history does not migrate ([wizard issue](https://github.com/Kilo-Org/kilocode/issues/11243)). File mapping ([official guide](https://kilo.ai/articles/roo-to-kilo-migration-guide)):

| Roo | Kilo |
|---|---|
| `.roorules` | `AGENTS.md` |
| `.roorules-{slug}` / `.roo/rules-{slug}/` | body of `.kilo/agents/{slug}.md` |
| `.roo/rules/*.md` | `.kilo/rules/*.md` (`.kilocode/rules/` fallback) |
| `.roomodes` / `custom_modes.yaml` | `.kilocodemodes` / markdown agent files; auto-converted on launch |
| `.rooignore` | `.kilocodeignore` |
| `.roo/mcp.json` / `mcp_settings.json` | `.kilocode/mcp.json` / `mcp` key in `~/.config/kilo/kilo.jsonc` |
| Checkpoints (shadow git) | Snapshots at `~/.local/share/kilo/snapshot/` |
| Extension `RooVeterinaryInc.roo-cline`, commands `roo-cline.*` | `kilocode.Kilo-Code`, commands `kilo-code.*` (find-and-replace in `keybindings.json`) |

Install: `code --install-extension kilocode.Kilo-Code`. Before uninstalling Roo, rotate any secrets (Roo settings exports contain plaintext API keys) and inventory `.roomodes`, rules, profiles, and approval settings ([Kilo vs Roo](https://kilo.ai/kilo-code/vs/roo-code)).
