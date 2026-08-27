# Provenance — kode fixtures

Source: `docs/harness-configs/kode.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| config.minimal.json | StrictJson | minimal valid config, empty object |
| config.populated.json | StrictJson | realistic populated with modelProfiles, modelPointers, mcpServers |
| config.foreign.json | StrictJson | realistic with unmodelled keys for round-trip preservation |
| config.malformed.json | StrictJson | malformed/truncated, expected invalid |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets KODE_CONFIG_DIR + CLAUDE_CONFIG_DIR, isolated |
| version.txt | TextFragment | detection/version output `kode 1.2.3` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
