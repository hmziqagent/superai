# Provenance — iflow_cli fixtures

Source: `docs/harness-configs/iflow-cli.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| settings.minimal.json | StrictJson | minimal with $schema |
| settings.populated.json | StrictJson | populated with selectedAuthType, apiKey, baseUrl, modelName, mcpServers |
| settings.foreign.json | StrictJson | foreign keys for preservation |
| settings.malformed.json | StrictJson | malformed truncated |
| wrapper.sh | TextFragment | sanitized wrapper, MigrationOnly blocked |
| version.txt | TextFragment | detection version `iflow 0.9.0` |
