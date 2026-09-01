# Provenance — zed_acp fixtures

Source: `docs/harness-configs/zed-acp.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal valid |
| settings.populated.json | StrictJson | populated: language_models, context_servers |
| settings.foreign.json | StrictJson | foreign keys preserved |
| settings.malformed.json | StrictJson | malformed |
| settings.boundary_legacy.json | StrictJson | assistant-era keys |
| settings.boundary_current.json | StrictJson | language_models era |
| keymap.populated.json | StrictJson | agent panel keymap |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.