# Provenance — letta_code fixtures

Source: `docs/harness-configs/letta-code.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal valid config, empty object |
| settings.populated.json | StrictJson | populated with model, providers, mcpServers, memory |
| settings.foreign.json | StrictJson | foreign keys for preservation |
| settings.malformed.json | StrictJson | malformed truncated |
| wrapper.sh | TextFragment | sanitized wrapper, LETTA_LOCAL_BACKEND_DIR + LETTA_BASE_URL, constrained separate server |
| version.txt | TextFragment | detection version `letta 0.2.1` |
