# Provenance — chatgpt_desktop fixtures

Source: `docs/harness-configs/chatgpt-desktop.md` last_verified=2026-09-18 (research evidence at
`.z-workflow/evidence/desktop-research/`: `learn-chatgpt-com-config-reference.md` for
`~/.codex/config.toml` + `desktop.custom_file_handlers` + `mcp_servers.<id>`,
`openai-linux-codexapp-sources.md` for the desktop-shares-CLI-store statement,
`help-openai-com-12584461-developer-mode-mcp.md` for the remote-only Chat MCP position).
Generated: 2026-09-18
Sanitized: no credential material — the sign-in file is deliberately excluded from
the corpus (the adapter declares that surface detect-only).
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| config.minimal.toml | Toml | minimal valid (comment only — the shared store exists, nothing set) |
| config.populated.toml | Toml | desktop-only key + shared `[mcp_servers.<id>]` tables |
| config.foreign.toml | Toml | unmodelled keys (notify/projects trust) preserved alongside desktop keys |
| config.malformed.toml | Toml | malformed (truncated table header), rejected by design |

Notes: the surface BELONGS to codex-cli (chatgpt-desktop is read-only on it); the community-grade
Chat-side `config/mcp.json` path claim is NOT modeled and NOT fixture'd. `desktop.custom_file_handlers`
value shape is unverified in the corpus, so fixtures carry an empty list only.
All fixtures load via superai_config parsers without panic; the malformed variant fails to parse
by design.
