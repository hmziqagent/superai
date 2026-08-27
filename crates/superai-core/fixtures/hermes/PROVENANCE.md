# Provenance — hermes fixtures

Source: `docs/harness-configs/hermes-agent.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| config.minimal.yaml | Yaml | minimal valid config with model |
| config.populated.yaml | Yaml | realistic populated with providers, mcp_servers, skills |
| config.foreign.yaml | Yaml | realistic with unmodelled keys for round-trip preservation |
| config.malformed.yaml | Yaml | malformed/truncated, expected invalid |
| env.minimal | Env | minimal .env with fake keys |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets HERMES_HOME, isolated |
| version.txt | TextFragment | detection/version output `hermes 0.20.5` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
