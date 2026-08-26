# Provenance — codex_cli fixtures

Source: `docs/harness-configs/codex-cli.md` last_verified=2026-08-25
Generated: 2026-08-26
Sanitized: all credentials use `sk-fake-` prefix (fake)

| Fixture | Kind | Description |
|---|---|---|
| config.minimal.toml | Toml | minimal valid, comment only |
| config.populated.toml | Toml | realistic populated with model_providers, mcp_servers |
| config.foreign.toml | Toml | realistic with unmodelled keys |
| config.malformed.toml | Toml | malformed, truncated |
| config.boundary_legacy.toml | Toml | legacy schema with inline [profiles.o3], wire_api chat |
| config.boundary_current.toml | Toml | current schema with responses wire_api, profile file style |
| config.mcp.toml | Toml | MCP variant |
| config.skills.toml | Toml | skills variant |
| config.plugins.toml | Toml | plugins variant |
| skills/example/SKILL.md | TextFragment | example skill |
| wrapper.sh | TextFragment | sanitized wrapper sets CODEX_HOME |
| version.txt | TextFragment | version output `codex-cli 0.134.0` |

All fixtures load via superai_config parsers without panic, sanitized (fake sk-fake-).
