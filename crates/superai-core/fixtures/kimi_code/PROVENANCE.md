# Provenance — kimi_code fixtures

Source: `docs/harness-configs/kimi-cli.md` last_verified=2026-08-25
Generated: 2026-08-26
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| config.minimal.toml | Toml | minimal valid config |
| config.populated.toml | Toml | realistic populated with providers, models, mcp |
| config.foreign.toml | Toml | realistic with unmodelled keys for round-trip preservation |
| config.malformed.toml | Toml | malformed/truncated, expected invalid |
| mcp.minimal.json | StrictJson | minimal mcp config |
| mcp.populated.json | StrictJson | populated mcp |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets KIMI_CODE_HOME, isolated |
| version.txt | TextFragment | detection/version output `kimi 0.20.0` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
