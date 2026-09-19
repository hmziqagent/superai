# Provenance — workbuddy fixtures

Source: `docs/harness-configs/workbuddy.md` last_verified=2026-09-08
Generated: 2026-09-08
Sanitized: all credentials use `sk-fake-`/`test`/`example` markers, no real credentials (fake); the
workbuddy-bench preset's `${CBC_API_KEY}` was renamed to `${CBC_TEST_API_KEY}` to carry an explicit
fake marker — shape otherwise identical to the upstream preset.
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| models.minimal.json | StrictJson | minimal `~/.codebuddy/models.json`: one model entry + availableModels |
| models.populated.json | StrictJson | populated catalog: DeepSeek-style entry (numeric token caps, relatedModels) + Ollama local |
| models.foreign.json | StrictJson | valid catalog with unmodelled keys for round-trip preservation |
| models.malformed.json | StrictJson | malformed/truncated, expected invalid |
| models.boundary_legacy.json | StrictJson | cbc < 2.103.4 compaction era: `${ENV}`-string token caps (workbuddy-bench preset shape) |
| models.boundary_current.json | StrictJson | cbc >= 2.103.4 era: maxInputTokens omitted, window via CODEBUDDY_AUTO_COMPACT_WINDOW |
| settings.minimal.json | StrictJson | minimal settings, empty object |
| settings.populated.json | StrictJson | bench preset: permissions.deny, bypassPermissions, thinking/compact toggles, env block |
| settings.foreign.json | StrictJson | settings with unmodelled keys for round-trip preservation |
| settings.malformed.json | StrictJson | malformed/truncated, expected invalid |
| mcp.minimal.json | StrictJson | minimal `~/.codebuddy/.mcp.json`, empty mcpServers |
| mcp.populated.json | StrictJson | populated MCP: stdio server (command/args/env) + http server (url/headers) |
| env.minimal.env | Env | minimal auth env (CODEBUDDY_API_KEY, fake value) |
| env.populated.env | Env | populated env: auth trio, CBC_* bench names, edition/sandbox/updater knobs |
| wrapper.sh | TextFragment | sanitized wrapper, relocates CODEBUDDY_CONFIG_DIR, fake key |
| version.txt | TextFragment | version evidence: npm package `@tencent-ai/codebuddy-code 2.147.0` (exact `cbc --version` output format unverified — see research doc §7) |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake
sk-fake-/test/example markers). Config shapes derived from the DeepSeek WorkBuddy integration guide
(models.json), CodeBuddy CLI docs (settings/.mcp.json precedence and schema), and Tencent's
workbuddy-bench presets (models/settings) as cited in the research doc.
