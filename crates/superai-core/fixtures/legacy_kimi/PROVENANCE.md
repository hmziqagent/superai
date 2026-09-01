# Provenance — legacy_kimi fixtures

Source: `docs/harness-configs/kimi-cli.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| config.minimal.toml | Toml | minimal valid |
| config.populated.toml | Toml | populated legacy shape |
| config.foreign.toml | Toml | foreign keys preserved |
| config.malformed.toml | Toml | malformed |
| config.boundary_legacy.toml | Toml | early legacy shape |
| config.boundary_current.toml | Toml | final legacy shape (kimi migrate source) |
| mcp.populated.json | StrictJson | MCP servers |
| tui.populated.toml | Toml | TUI settings |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.