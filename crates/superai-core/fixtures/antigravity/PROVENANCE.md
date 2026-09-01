# Provenance — antigravity fixtures

Source: `docs/harness-configs/antigravity-cli.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal valid |
| settings.populated.json | StrictJson | populated: modelProvider, sandbox |
| settings.foreign.json | StrictJson | foreign keys preserved |
| settings.malformed.json | StrictJson | malformed |
| settings.boundary_legacy.json | StrictJson | early-preview settings shape |
| settings.boundary_current.json | StrictJson | current settings shape |
| mcp_config.populated.json | StrictJson | global MCP config |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.