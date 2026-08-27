# Provenance — qwen_code fixtures

Source: `docs/harness-configs/qwen-code.md` last_verified=2026-08-25
Generated: 2026-08-26
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal valid config, empty object |
| settings.populated.json | StrictJson | realistic populated with modelProviders, mcpServers |
| settings.foreign.json | StrictJson | realistic with unmodelled keys for round-trip preservation |
| settings.malformed.json | StrictJson | malformed/truncated, expected invalid |
| env.minimal | Env | minimal env |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets QWEN_HOME, QWEN_RUNTIME_DIR isolated |
| version.txt | TextFragment | detection/version output `qwen 0.1.0` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
