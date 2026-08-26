# Provenance — aider fixtures

Source: `docs/harness-configs/aider.md` last_verified=2026-08-25
Generated: 2026-08-26
Sanitized: all credentials use `sk-fake-` prefix (fake)

| Fixture | Kind | Description |
|---|---|---|
| aider.minimal.yml | Yaml | minimal valid |
| aider.populated.yml | Yaml | realistic populated |
| aider.foreign.yml | Yaml | realistic with unmodelled keys |
| aider.malformed.yml | Yaml | malformed |
| aider.boundary_legacy.yml | Yaml | legacy schema boundary (gpt-4) |
| aider.boundary_current.yml | Yaml | current schema (openrouter) |
| aider.mcp.yml | Yaml | mcp variant |
| aider.skills.yml | Yaml | skills variant |
| aider.plugins.yml | Yaml | plugins variant |
| aider.comments.yml | Yaml | realistic with comments |
| .env.minimal | Env | minimal env |
| .env.populated | Env | populated env |
| .env.foreign | Env | foreign keys, comments |
| .env.malformed | Env | malformed env (missing =) |
| .env.mcp | Env | mcp env variant |
| .env.skills | Env | skills env variant |
| model.metadata.populated.json | StrictJson | populated metadata |
| model.metadata.boundary_legacy.json | StrictJson | legacy metadata |
| model.metadata.boundary_current.json | StrictJson | current metadata |
| skills/example/SKILL.md | TextFragment | example skill |
| wrapper.sh | TextFragment | sanitized wrapper sets HOME, --config, --env-file |
| version.txt | TextFragment | version output `aider 0.84.0` |

All fixtures load via superai_config parsers without panic, sanitized (fake sk-fake-).
