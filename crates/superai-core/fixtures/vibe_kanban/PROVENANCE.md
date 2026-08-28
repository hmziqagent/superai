# Provenance — vibe_kanban fixtures

Source: `docs/harness-configs/orchestrators.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| profiles.minimal.json | StrictJson | minimal empty profiles |
| profiles.populated.json | StrictJson | populated with claude-glm profile, ANTHROPIC_BASE_URL fake |
| profiles.foreign.json | StrictJson | foreign keys for preservation |
| profiles.malformed.json | StrictJson | malformed truncated |
| mcp.minimal.json | StrictJson | minimal mcpServers |
| mcp.populated.json | StrictJson | populated mcp |
| mcp.malformed.json | StrictJson | malformed |
| wrapper.sh | TextFragment | MigrationOnly placeholder, blocked |
| version.txt | TextFragment | detection version `vibe-kanban 0.1.44` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
