# Provenance — amazon_q fixtures

Source: `docs/harness-configs/amazon-q-cli.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal valid |
| settings.populated.json | StrictJson | populated: chat.defaultModel, mcp.initTimeout |
| settings.foreign.json | StrictJson | foreign keys preserved |
| settings.malformed.json | StrictJson | malformed |
| settings.boundary_legacy.json | StrictJson | legacy era (older default model) |
| settings.boundary_current.json | StrictJson | current era (initTimeout + newer model) |
| cli-agents.populated.json | StrictJson | agent definition (agent-format.md) |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.