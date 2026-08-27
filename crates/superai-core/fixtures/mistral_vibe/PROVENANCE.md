# Provenance — mistral_vibe fixtures

Source: `docs/harness-configs/mistral-vibe.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| config.minimal.toml | Toml | minimal valid config |
| config.populated.toml | Toml | realistic populated with providers, models, mcp_servers |
| config.foreign.toml | Toml | realistic with unmodelled keys for round-trip preservation |
| config.malformed.toml | Toml | malformed/truncated, expected invalid |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets VIBE_HOME, isolated |
| version.txt | TextFragment | detection/version output `vibe 0.8.0` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
