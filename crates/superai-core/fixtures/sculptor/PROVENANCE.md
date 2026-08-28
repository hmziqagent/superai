# Provenance — sculptor fixtures

Source: `docs/harness-configs/orchestrators.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| env.minimal | Env | minimal global .env empty |
| env.populated | Env | populated with ANTHROPIC_BASE_URL sk-fake |
| env.foreign | Env | foreign keys |
| project-env.example | Env | per-repo .sculptor/.env example |
| harnesses.minimal.json | StrictJson | minimal empty |
| harnesses.populated.json | StrictJson | populated with claude/pi/dependencies |
| harnesses.foreign.json | StrictJson | foreign keys preservation |
| harnesses.malformed.json | StrictJson | malformed truncated |
| wrapper.sh | TextFragment | sanitized wrapper, SCULPTOR_WORKSPACE_PATH/ENV_FILE, workspace/container |
| version.txt | TextFragment | detection version `sculptor 0.46.0-dev` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
