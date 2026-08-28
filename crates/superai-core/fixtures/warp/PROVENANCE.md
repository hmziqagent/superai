# Provenance — warp fixtures

Source: `docs/harness-configs/warp.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.toml | Toml | minimal empty |
| settings.populated.toml | Toml | populated with [appearance]/[general]/[agents]/[profiles] + mcpServers |
| settings.foreign.toml | Toml | foreign keys for preservation |
| settings.malformed.toml | Toml | malformed missing brackets |
| mcp.minimal.json | StrictJson | minimal mcpServers {} |
| mcp.populated.json | StrictJson | populated with stdio + http servers, fake Bearer |
| mcp.foreign.json | StrictJson | foreign keys preservation |
| mcp.malformed.json | StrictJson | malformed truncated |
| workflow.minimal.yaml | Yaml | minimal workflow name+command |
| workflow.populated.yaml | Yaml | populated with tags/shells/arguments |
| workflow.malformed.yaml | Yaml | malformed unclosed bracket |
| wrapper.sh | TextFragment | sanitized wrapper, XDG_CONFIG_HOME/XDG_DATA_HOME + WARP_API_KEY, Linux XDG constrained |
| version.txt | TextFragment | detection version `warp 1.2.3` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).
