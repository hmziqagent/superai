# Provenance — junie fixtures

Source: `docs/harness-configs/junie-cli.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| config.minimal.json | StrictJson | minimal valid config, empty object |
| config.populated.json | StrictJson | realistic populated with model, provider, mcpServers, proxies |
| config.foreign.json | StrictJson | realistic with unmodelled keys for round-trip preservation |
| config.malformed.json | StrictJson | malformed/truncated, expected invalid |
| mcp.minimal.json | StrictJson | minimal mcp config |
| mcp.populated.json | StrictJson | populated mcp |
| models.minimal.json | StrictJson | minimal custom model profile |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets JUNIE_HOME, isolated |
| version.txt | TextFragment | detection/version output `junie 0.5.0` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
