# Provenance — factory-droid fixtures

Source: `docs/harness-configs/factory-droid.md` last_verified=2026-09-18 (MCP destination live-probe droid 0.222.0; earlier sections 2026-08-25)
Generated: 2026-08-27 (mcp.populated.json refreshed to the live writer shape 2026-09-18)
Sanitized: fake

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | Json | minimal valid settings |
| settings.populated.json | Json | realistic populated settings |
| settings.foreign.json | Json | realistic with unmodelled keys for round-trip preservation |
| settings.malformed.json | Json | malformed/truncated, expected invalid |
| mcp.minimal.json | Json | minimal empty `{}` MCP container |
| mcp.populated.json | Json | live-verified writer shape: top-level `mcpServers`, stdio entry `{type, command, args, disabled}` (verbatim structure from `droid mcp add`, droid 0.222.0, evidence .z-workflow/evidence/live/factory-droid/mcp-dest-r6.log) |
| skills/ | TextFragment | example skill |
| wrapper.sh | TextFragment | sanitized wrapper, HOME relocation |
| version.txt | TextFragment | detection/version output |
