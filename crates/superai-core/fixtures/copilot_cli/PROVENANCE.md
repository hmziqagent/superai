# Provenance — copilot_cli fixtures

Source: `docs/harness-configs/copilot-cli.md` last_verified=2026-08-25
Generated: 2026-08-26
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal valid config, empty object |
| settings.populated.json | StrictJson | realistic populated with $schema, provider, mcpServers |
| settings.foreign.json | StrictJson | realistic with unmodelled keys for round-trip preservation |
| settings.malformed.json | StrictJson | malformed/truncated, expected invalid |
| mcp-config.minimal.json | StrictJson | minimal mcp config |
| mcp-config.populated.json | StrictJson | populated mcp with fake token |
| lsp-config.minimal.json | StrictJson | minimal lsp config |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets COPILOT_HOME, isolated |
| version.txt | TextFragment | detection/version output `0.1.0 (Copilot CLI)` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
