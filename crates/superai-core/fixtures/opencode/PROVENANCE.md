# Provenance — opencode fixtures

Source: `docs/harness-configs/opencode.md` last_verified=2026-08-25
Generated: 2026-08-26
Sanitized: all credentials use `sk-fake-` prefix or `{env:...}` template (fake sk-fake-)

| Fixture | Kind | Description |
|---|---|---|
| opencode.minimal.json | StrictJson | minimal |
| opencode.minimal.jsonc | JsonC | minimal with comments |
| opencode.populated.json | StrictJson | populated |
| opencode.populated.jsonc | JsonC | populated with comments |
| opencode.foreign.json | StrictJson | foreign keys |
| opencode.foreign.jsonc | JsonC | foreign with comments |
| opencode.malformed.json | StrictJson | malformed |
| opencode.malformed.jsonc | JsonC | malformed jsonc |
| opencode.boundary_legacy.json | StrictJson | legacy schema boundary |
| opencode.boundary_current.json | StrictJson | current schema |
| opencode.boundary_legacy.jsonc | JsonC | legacy jsonc |
| opencode.boundary_current.jsonc | JsonC | current jsonc |
| opencode.mcp.json | StrictJson | mcp variant |
| opencode.mcp.jsonc | JsonC | mcp jsonc variant |
| opencode.plugins.json | StrictJson | plugins variant |
| opencode.skills.json | StrictJson | skills variant |
| skills/example/SKILL.md | TextFragment | example skill |
| wrapper.sh | TextFragment | sanitized wrapper sets XDG_CONFIG_HOME, OPENCODE_CONFIG |
| version.txt | TextFragment | version output `opencode 0.12.0` |

All fixtures load via superai_config parsers without panic, sanitized (fake).
