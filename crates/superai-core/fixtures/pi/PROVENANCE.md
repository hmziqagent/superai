# Provenance — pi fixtures

Source: `docs/harness-configs/pi.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal valid config, empty object |
| settings.populated.json | StrictJson | realistic populated with theme, providers, skills |
| settings.foreign.json | StrictJson | realistic with unmodelled keys for round-trip preservation |
| settings.malformed.json | StrictJson | malformed/truncated, expected invalid |
| models.minimal.json | StrictJson | minimal models config |
| models.populated.json | StrictJson | populated models with providers |
| auth.minimal.json | StrictJson | minimal auth config |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets PI_CODING_AGENT_DIR, isolated |
| version.txt | TextFragment | detection/version output `pi 0.9.0` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
