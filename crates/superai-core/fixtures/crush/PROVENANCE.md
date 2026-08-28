# Provenance — crush fixtures

Source: `docs/harness-configs/crush.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal with $schema |
| settings.populated.json | StrictJson | populated with providers, models, mcp, options |
| settings.foreign.json | StrictJson | foreign keys for preservation |
| settings.malformed.json | StrictJson | malformed truncated |
| crushrc.minimal | Executable | minimal Bash crushrc |
| crushrc.populated | Executable | populated Bash crushrc |
| wrapper.sh | TextFragment | sanitized wrapper, CRUSH_GLOBAL_CONFIG |
| version.txt | TextFragment | detection version `crush 0.5.1` |
