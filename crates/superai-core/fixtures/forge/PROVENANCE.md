# Provenance — forge fixtures

Source: `docs/harness-configs/forge.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| forge.minimal.toml | Toml | minimal valid config (.forge.toml) |
| forge.populated.toml | Toml | realistic populated with providers, session, retry |
| forge.foreign.toml | Toml | realistic with unmodelled keys for round-trip preservation |
| forge.malformed.toml | Toml | malformed/truncated, expected invalid |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets FORGE_CONFIG, isolated |
| version.txt | TextFragment | detection/version output `forge 0.3.0` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
