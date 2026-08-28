# Provenance — openhands fixtures

Source: `docs/harness-configs/openhands.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| agent_settings.minimal.json | StrictJson | minimal V1 JSON, empty object |
| agent_settings.populated.json | StrictJson | populated V1 with llm/sandbox/mcpServers |
| agent_settings.foreign.json | StrictJson | foreign keys for preservation |
| agent_settings.malformed.json | StrictJson | malformed truncated |
| config.minimal.toml | Toml | minimal V0 TOML comment-only |
| config.populated.toml | Toml | populated V0 with [core]/[llm]/[sandbox]/[agent] |
| config.foreign.toml | Toml | foreign keys/unsection for preservation |
| config.malformed.toml | Toml | malformed missing brackets |
| wrapper.sh | TextFragment | sanitized wrapper, OH_PERSISTENCE_DIR + LLM_* + RUNTIME docker, version split note |
| version.txt | TextFragment | detection version `openhands 1.8.0` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
