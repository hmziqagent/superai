# Provenance — nanocoder fixtures

Source: `docs/harness-configs/nanocoder.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| agents.config.minimal.json | StrictJson | minimal valid config, empty object |
| agents.config.populated.json | StrictJson | realistic populated with nanocoder.providers, mcpServers |
| agents.config.foreign.json | StrictJson | realistic with unmodelled keys for round-trip preservation |
| agents.config.malformed.json | StrictJson | malformed/truncated, expected invalid |
| mcp.minimal.json | StrictJson | minimal mcp config |
| mcp.populated.json | StrictJson | populated mcp with filesystem server |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets NANOCODER_CONFIG_DIR, isolated |
| version.txt | TextFragment | detection/version output `nanocoder 0.5.0` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
