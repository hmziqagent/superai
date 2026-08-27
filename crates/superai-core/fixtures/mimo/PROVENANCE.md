# Provenance — mimo fixtures

Source: `docs/harness-configs/mimo-code.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| mimocode.minimal.jsonc | JsonC | minimal with comments |
| mimocode.minimal.json | StrictJson | minimal |
| mimocode.populated.jsonc | JsonC | populated with model, provider, mcp, plugin |
| mimocode.populated.json | StrictJson | populated |
| mimocode.foreign.jsonc | JsonC | foreign keys with comments |
| mimocode.foreign.json | StrictJson | foreign keys |
| mimocode.malformed.jsonc | JsonC | malformed |
| mimocode.malformed.json | StrictJson | malformed |
| skills/example/SKILL.md | TextFragment | example skill |
| wrapper.sh | TextFragment | sanitized wrapper sets MIMOCODE_HOME, isolated |
| version.txt | TextFragment | version output `mimo 0.1.13` |

All fixtures pass `superai_config` parser without panic, sanitized (fake).
