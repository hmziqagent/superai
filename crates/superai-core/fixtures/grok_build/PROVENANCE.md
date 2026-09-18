# Provenance — grok_build fixtures

Source: `docs/harness-configs/grok-build.md` last_verified=2026-09-18 (MCP section live-probe; earlier sections 2026-08-25)
Generated: 2026-08-27 (config.mcp.toml added 2026-09-18)
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| config.minimal.toml | Toml | minimal valid config |
| config.populated.toml | Toml | realistic populated with providers, models, features |
| config.foreign.toml | Toml | realistic with unmodelled keys for round-trip preservation |
| config.malformed.toml | Toml | malformed/truncated, expected invalid |
| config.mcp.toml | Toml | MCP variant `[mcp_servers.*]` (stdio + url forms, live-verified grok 1.0.34) |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets GROK_HOME, isolated |
| version.txt | TextFragment | detection/version output `grok 0.5.0` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
