# Provenance — conductor fixtures

Source: `docs/harness-configs/orchestrators.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: no credentials in TOML fixtures (fake paths), no real secrets
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.toml | Toml | minimal empty |
| settings.populated.toml | Toml | populated user settings with executables/provider/models/scripts/env |
| settings.foreign.toml | Toml | foreign keys for preservation |
| settings.malformed.toml | Toml | malformed missing brackets |
| repo-settings.minimal.toml | Toml | minimal repo settings |
| repo-settings.populated.toml | Toml | populated repo settings with scripts.run.web |
| wrapper.sh | TextFragment | sanitized wrapper, CONDUCTOR_WORKSPACE_PATH/ROOT_PATH/PORT/IS_LOCAL, os_bound macOS worktrees |
| version.txt | TextFragment | detection version `conductor 1.2.3` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification.
