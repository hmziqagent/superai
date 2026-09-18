# Provenance — gemini_cli fixtures

Source: `docs/harness-configs/gemini-cli.md` (see last-verified date in that doc)
Generated: 2026-09-01
Updated: 2026-09-18 — `layout.isolated` corrected after live verification (z-workflow run 3,
`.z-workflow/evidence/live/gemini-cli/`): real gemini 0.60.0 nests a `.gemini/` dir inside
`GEMINI_CLI_HOME`, so the relocated root is `$GEMINI_CLI_HOME/.gemini` (doc lines 18, 78), not
`$GEMINI_CLI_HOME` itself. `layout.default` (`~/.gemini`) unchanged.
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