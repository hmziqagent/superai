# Provenance — plandex fixtures

Source: `docs/harness-configs/plandex.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| custom-models.minimal.json | StrictJson | minimal with $schema only |
| custom-models.populated.json | StrictJson | populated with providers/models/modelPacks, skipAuth local |
| custom-models.foreign.json | StrictJson | foreign keys for preservation |
| custom-models.malformed.json | StrictJson | malformed truncated |
| env.example | Env | example provider env vars, redacted fake |
| wrapper.sh | TextFragment | sanitized wrapper, PLANDEX_API_HOST + OPENROUTER_API_KEY, constrained env_only |
| version.txt | TextFragment | detection version `plandex v2.1.0` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
