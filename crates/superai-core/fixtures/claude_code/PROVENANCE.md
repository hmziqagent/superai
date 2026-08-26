# Provenance — claude_code fixtures

Source: `docs/harness-configs/claude-code.md` last_verified=2026-08-25
Generated: 2026-08-26
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal valid config, empty object |
| settings.populated.json | StrictJson | realistic populated with $schema, env, permissions, hooks, plugins |
| settings.foreign.json | StrictJson | realistic with unmodelled keys for round-trip preservation |
| settings.malformed.json | StrictJson | malformed/truncated, expected invalid |
| settings.boundary_legacy.json | StrictJson | previous schema boundary (simpler, no hooks/plugins) |
| settings.boundary_current.json | StrictJson | current schema boundary (with hooks, plugins, statusLine) |
| settings.mcp.json | StrictJson | MCP variant with mcpServers |
| settings.plugins.json | StrictJson | plugins variant with enabledPlugins |
| settings.skills.json | StrictJson | skills variant |
| settings.comments.jsonc | JsonC | realistic with comments, trailing commas, unmodelled keys |
| mcp.json | StrictJson | project .mcp.json variant |
| skills/example/SKILL.md | TextFragment | example SKILL.md |
| wrapper.sh | TextFragment | sanitized wrapper, sets CLAUDE_CONFIG_DIR, isolated |
| version.txt | TextFragment | detection/version output `2.0.76 (Claude Code)` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
