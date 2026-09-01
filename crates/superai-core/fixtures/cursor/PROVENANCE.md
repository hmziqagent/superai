# Provenance — cursor fixtures

Source: `docs/harness-configs/cursor.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| cli-config.minimal.json | StrictJson | minimal valid |
| cli-config.populated.json | StrictJson | populated: model, autoApprove |
| cli-config.foreign.json | StrictJson | foreign keys preserved |
| cli-config.malformed.json | StrictJson | malformed |
| cli-config.boundary_legacy.json | StrictJson | legacy key names |
| cli-config.boundary_current.json | StrictJson | current key names |
| mcp.populated.json | StrictJson | MCP servers (global mcp.json) |
| mcp.malformed.json | StrictJson | malformed MCP config |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.