# Provenance — windsurf fixtures

Source: `docs/harness-configs/windsurf.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| mcp_config.minimal.json | StrictJson | minimal valid |
| mcp_config.populated.json | StrictJson | populated: stdio + serverUrl |
| mcp_config.foreign.json | StrictJson | foreign keys preserved |
| mcp_config.malformed.json | StrictJson | malformed |
| mcp_config.boundary_legacy.json | StrictJson | legacy remote url entry |
| mcp_config.boundary_current.json | StrictJson | current serverUrl entry |
| rules.minimal.md | TextFragment | rules stub |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.