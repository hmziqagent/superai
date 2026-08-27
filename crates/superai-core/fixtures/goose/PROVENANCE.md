# Provenance — goose fixtures

Source: `docs/harness-configs/goose.md` last_verified=2026-08-25
Generated: 2026-08-26
Sanitized: all credentials use `sk-fake-` prefix (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| config.minimal.yaml | Yaml | minimal valid config |
| config.populated.yaml | Yaml | realistic populated with provider, model, extensions |
| config.foreign.yaml | Yaml | realistic with unmodelled keys for round-trip preservation |
| config.malformed.yaml | Yaml | malformed/truncated, expected invalid |
| recipe.minimal.yaml | Yaml | minimal recipe |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets GOOSE_PATH_ROOT, isolated |
| version.txt | TextFragment | detection/version output `goose 1.2.3` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake).
