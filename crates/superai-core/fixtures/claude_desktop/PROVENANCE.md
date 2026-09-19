# Provenance — claude_desktop fixtures

Source: `docs/harness-configs/claude-desktop.md` last_verified=2026-09-18 (research evidence at
`.z-workflow/evidence/desktop-research/`: `modelcontextprotocol-io-quickstart-user.md` for the
`mcpServers` schema, `claude-com-download.md` / `code-claude-com-desktop-linux.md` for platform
paths, `anthropic-com-desktop-extensions.md` for the `.mcpb` gap).
Generated: 2026-09-18
Sanitized: no credentials (fixture server commands reference public packages or fake paths only).
Platform-normalized: LF line endings only.

| Fixture | Kind | Description |
|---|---|---|
| claude_desktop_config.minimal.json | Json | minimal valid (`mcpServers` empty map) |
| claude_desktop_config.populated.json | Json | populated stdio servers (command/args/env, absolute paths) |
| claude_desktop_config.foreign.json | Json | foreign keys alongside `mcpServers` preserved on write |
| claude_desktop_config.malformed.json | Json | malformed (truncated array + trailing comma), rejected by design |

Notes: paths inside `args` are absolute per the documented loader rule; remote servers are
UI-managed and intentionally NOT fixture'd; the `.mcpb` extension store has NO documented install
directory, so no plugin fixture exists (adapter declares plugin absence). The 3P/enterprise
`configLibrary/` + managed-prefs surface is out of alias scope and not fixture'd.
All fixtures load via superai_config parsers without panic; the malformed variant fails to parse
by design.
