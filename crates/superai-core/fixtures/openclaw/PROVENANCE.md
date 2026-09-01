# Provenance — openclaw fixtures

Source: `docs/harness-configs/openclaw.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` markers with FAKE in the name (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| openclaw.minimal.json | StrictJson | minimal valid |
| openclaw.populated.json | StrictJson | populated: agents.defaults.model, models.providers |
| openclaw.foreign.json | StrictJson | foreign keys preserved |
| openclaw.malformed.json | StrictJson | malformed |
| openclaw.boundary_legacy.json | StrictJson | legacy flat agent key |
| openclaw.boundary_current.json | StrictJson | current agents/models tree |
| .env.minimal | Env | minimal env |
| .env.populated | Env | populated env |
| .env.malformed | Env | malformed (line without =) |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.