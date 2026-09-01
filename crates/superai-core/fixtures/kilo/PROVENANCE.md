# Provenance — kilo fixtures

Source: `docs/harness-configs/kilo-code.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` markers with FAKE in the name references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| kilo.minimal.jsonc | JsonC | minimal valid |
| kilo.populated.jsonc | JsonC | populated: model, provider options, mcp |
| kilo.foreign.jsonc | JsonC | foreign keys preserved |
| kilo.malformed.jsonc | JsonC | malformed |
| kilo.boundary_legacy.json | StrictJson | legacy kilo.json shape (deep-merged) |
| kilo.boundary_current.jsonc | JsonC | current kilo.jsonc shape |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.