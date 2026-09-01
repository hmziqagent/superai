# Provenance — zcode fixtures

Source: `docs/harness-configs/zcode.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| config.minimal.json | StrictJson | minimal valid |
| config.populated.json | StrictJson | populated provider options |
| config.foreign.json | StrictJson | foreign keys preserved |
| config.malformed.json | StrictJson | malformed |
| config.boundary_legacy.json | StrictJson | pre-v2 options shape |
| config.boundary_current.json | StrictJson | v2 options shape (headers) |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.