# Provenance — kiro fixtures

Source: `docs/harness-configs/kiro.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| cli.minimal.json | StrictJson | minimal valid |
| cli.populated.json | StrictJson | populated: defaultAgent, context |
| cli.foreign.json | StrictJson | foreign keys preserved |
| cli.malformed.json | StrictJson | malformed |
| cli.boundary_legacy.json | StrictJson | legacy key names |
| cli.boundary_current.json | StrictJson | current key names |
| mcp.populated.json | StrictJson | MCP servers |
| permissions.populated.yaml | Yaml | permissions rules |
| permissions.malformed.yaml | Yaml | malformed permissions |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.