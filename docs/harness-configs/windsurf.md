# Windsurf (Devin Desktop / Cascade) — Configuration Reference

Compiled from primary sources on 2026-08-25. Sources:
- https://docs.windsurf.com/windsurf/cascade/mcp.md
- https://docs.windsurf.com/windsurf/cascade/memories.md
- https://docs.windsurf.com/windsurf/models.md
- https://docs.windsurf.com/context-awareness/windsurf-ignore.md
- https://docs.windsurf.com/windsurf/advanced.md
- https://docs.windsurf.com/llms.txt (docs index)

> **Status note:** After Cognition's absorption of Windsurf, the product is converging with Devin Desktop ("Devin Desktop" is now the primary name in current docs; legacy `windsurf` paths redirect to `docs.devin.ai/desktop/...`). Config paths below show both forms where they differ.

## 1. Config surfaces

| Surface | Path | Notes |
|---|---|---|
| MCP servers | `~/.codeium/windsurf/mcp_config.json` | JSON `{ "mcpServers": { "<name>": { "command", "args", "env" } } }` — stdio transport; HTTP/SSE remote servers also supported (`serverUrl`), incl. OAuth. One-click deeplink installs supported |
| Cascade memories | `~/.codeium/windsurf/memories/` | auto-generated per workspace, stored locally, not committed, no credit cost; Memories apply to the legacy Cascade agent only (Devin Local agent uses its own memory) |
| Workspace rules | `.devin/rules/*.md` (preferred) or `.windsurf/rules/*.md` (fallback) | one file per rule, ≤12,000 chars each; frontmatter sets activation mode |
| Legacy rules | `.windsumrfrules` → `.windsurfrules` at workspace root | still read |
| AGENTS.md | any directory in the workspace | processed by the same rules engine: root-level = always-on; subdirectory = auto-glob for that directory |
| System rules (enterprise) | `/etc/devin/rules/`, legacy `/etc/windsurf/rules/` | deployed by IT, read-only |
| Ignore files | `.devinignore` / `.codeiumignore` (+ global variant) | gitignore syntax; excluded from indexing and agent editing; gitignored files can't be edited by the agent |

### Rule activation modes

| Mode | Trigger | How it reaches the model |
|---|---|---|
| Always On | `always_on` | full content in system prompt every message |
| Model Decision | `model_decision` | only description shown; full rule read when Cascade deems it relevant |
| Glob | `globs` pattern | applied when reading/editing matching files |
| Manual | no frontmatter | invoked on demand (like workflows via slash command) |

Workflows = prompt templates for repeatable multi-step tasks (manual via `/[name]`). Skills = multi-step procedures bundled with scripts/templates, dynamically invoked or @-mentioned.

## 2. Environment variables

**No first-party config env vars are documented** (no `WINDSURF_*`/`CODEIUM_*` settings overrides). What exists:
- Standard proxy configuration (HTTP/HTTPS auto-detect + manual, SSH-remote proxies) — documented in the proxy page
- Env vars inside `mcpServers.*.env` blocks for MCP server processes

Honest assessment: Windsurf is a managed-backend IDE; there is no config-home relocation variable.

## 3. Models & providers

- In-house SWE family: **SWE-1.7** (Max/Medium reasoning variants), SWE-1.7 Lightning (Cerebras), SWE-1.6 / SWE-1.6 Fast, SWE-1-mini (Tab), swe-grep (Fast Context retrieval), swe-check (Quick Review)
- Third-party frontier models (Claude, GPT families); some models are **Devin Local only** (e.g., GPT-5.6 variants)
- **Adaptive**: Cognition's intelligent router that picks the model per task (recommended default)
- **BYOK**: historical BYO-key options existed for chat models; current Devin Desktop docs describe a curated catalog billed via credits/ACUs — **no custom base URL / gateway / local-model endpoint support is documented**. All inference is mediated by Windsurf/Cognition infrastructure.
- Enterprise: self-hosted options center on data-plane controls (FedRAMP guide, enterprise policies for allowlists/updates), not self-hosted inference.

## 4. MULTI-INSTANCE WRAPPERS

No native profile/account switching is documented, and there is no env-based config-dir override. Feasible isolation:
```bash
#!/usr/bin/env bash
# windsurf-profile2: fully isolated IDE instance (own settings, extensions, login)
exec windsurf --user-data-dir "$HOME/.windsurf-profiles/p2" "$@"
```
Caveats: heavier than CLI harness wrappers (full second editor instance), license/login state lives inside the user-data dir, and model/provider choice remains account-bound in the managed backend — you cannot point one instance at a different API provider.

## 5. Other knobs

- Cascade access to .gitignore'd paths can be enabled (advanced setting)
- Agent diff zones, SSH remote support, Dev Containers, WSL (beta), extension marketplace policy — all under Advanced Configuration
- Teams/Enterprise: admin-controlled MCP allowlists, config interpolation, custom MCP registries, central policies (group policy / MDM / JSON policy files)

## Sources
The six URLs above, fetched 2026-08-25.
