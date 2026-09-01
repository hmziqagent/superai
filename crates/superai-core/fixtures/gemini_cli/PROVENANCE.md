# Provenance — gemini_cli fixtures

Source: `docs/harness-configs/gemini-cli.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal valid |
| settings.populated.json | StrictJson | populated category objects |
| settings.foreign.json | StrictJson | foreign keys preserved |
| settings.malformed.json | StrictJson | malformed |
| settings.boundary_legacy.json | StrictJson | pre-category flat keys |
| settings.boundary_current.json | StrictJson | category-object era |
| .env.minimal | Env | minimal env |
| .env.populated | Env | populated env |
| .env.malformed | Env | malformed (line without =) |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.