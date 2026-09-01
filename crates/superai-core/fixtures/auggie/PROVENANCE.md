# Provenance — auggie fixtures

Source: `docs/harness-configs/auggie.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal valid |
| settings.populated.json | StrictJson | populated: autoUpdate, defaultProfile |
| settings.foreign.json | StrictJson | foreign keys preserved |
| settings.malformed.json | StrictJson | malformed |
| settings.comments.jsonc | JsonC | comments + trailing comma (JSONC supported) |
| settings.boundary_legacy.json | StrictJson | legacy auto-update key |
| settings.boundary_current.json | StrictJson | current autoUpdate key |
| guidelines.minimal.md | TextFragment | workspace guidelines stub |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.