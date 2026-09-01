# Provenance — roo_code fixtures

Source: `docs/harness-configs/roo-code.md` (see last-verified date in that doc)
Generated: 2026-09-01
Sanitized: all credentials use `sk-fake-` prefix or `{env:VAR}` references (fake)
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| mcp_settings.minimal.json | StrictJson | minimal valid |
| mcp_settings.populated.json | StrictJson | populated: alwaysAllow |
| mcp_settings.foreign.json | StrictJson | foreign keys preserved |
| mcp_settings.malformed.json | StrictJson | malformed |
| mcp_settings.boundary_legacy.json | StrictJson | autoApprove-era entry |
| mcp_settings.boundary_current.json | StrictJson | alwaysAllow-era entry |
| custom_modes.populated.yaml | Yaml | custom modes |
| custom_modes.malformed.yaml | Yaml | malformed modes |
| vscode-settings.boundary_legacy.json | StrictJson | legacy roo.* keys |
| vscode-settings.boundary_current.json | StrictJson | current roo-cline.* keys |

All fixtures load via superai_config parsers without panic; malformed variants fail to parse by design.