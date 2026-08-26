# Provenance — cline fixtures

Source: `docs/harness-configs/cline.md` last_verified=2026-08-25
Generated: 2026-08-26
Sanitized: all credentials use `sk-fake-` prefix (fake)

| Fixture | Kind | Description |
|---|---|---|
| providers.minimal.json | StrictJson | minimal |
| providers.populated.json | StrictJson | populated with fake sk-fake- |
| providers.foreign.json | StrictJson | foreign keys |
| providers.malformed.json | StrictJson | malformed |
| providers.boundary_legacy.json | StrictJson | legacy schema boundary |
| providers.boundary_current.json | StrictJson | current schema |
| providers.mcp.json | StrictJson | mcp variant |
| providers.skills.json | StrictJson | skills variant |
| providers.plugins.json | StrictJson | plugins variant |
| cline_mcp_settings.minimal.json | StrictJson | minimal mcp |
| cline_mcp_settings.populated.json | StrictJson | populated mcp |
| cline_mcp_settings.foreign.json | StrictJson | foreign mcp |
| cline_mcp_settings.malformed.json | StrictJson | malformed mcp |
| cline_mcp_settings.boundary_legacy.json | StrictJson | legacy mcp |
| cline_mcp_settings.boundary_current.json | StrictJson | current mcp |
| settings.minimal.json | StrictJson | minimal settings |
| settings.populated.json | StrictJson | populated settings |
| settings.foreign.json | StrictJson | foreign settings |
| settings.malformed.json | StrictJson | malformed settings |
| settings.boundary_legacy.json | StrictJson | legacy settings |
| settings.boundary_current.json | StrictJson | current settings |
| .clinerules | TextFragment | rules example |
| rules/example.md | TextFragment | rule with frontmatter |
| skills/example/SKILL.md | TextFragment | example skill |
| wrapper.sh | TextFragment | sanitized wrapper sets CLINE_DATA_DIR, code --user-data-dir |
| version.txt | TextFragment | version output `cline 1.2.3` |

All fixtures load without panic, sanitized (fake sk-fake-).
