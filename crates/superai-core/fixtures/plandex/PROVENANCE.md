# Provenance — plandex fixtures

Source: `docs/harness-configs/plandex.md` last_verified=2026-08-25
Generated: 2026-08-27
Sanitized: all credentials use `sk-fake-` prefix, no real credentials (fake)
Generator: superai QAL-02 fixture corpus

| Fixture | Kind | Description |
|---|---|---|
| custom-models.minimal.json | StrictJson | minimal with $schema only |
| custom-models.populated.json | StrictJson | populated with providers/models/modelPacks, local no-auth entry (fake marker) |
| custom-models.foreign.json | StrictJson | foreign keys for preservation |
| custom-models.malformed.json | StrictJson | malformed truncated |
| env.example | Env | example provider env vars, redacted fake |
| wrapper.sh | TextFragment | sanitized wrapper, PLANDEX_API_HOST + OPENROUTER_API_KEY (fake) + HOME relocation to <root> (v2 home ~/.plandex-home-v2 is HOME-relative; no relocation env — live cli/v2.2.1), constrained env_only |
| version.txt | TextFragment | detection version `plandex v2.1.0` |

All fixtures pass `superai_config` parser without panic and are sanitized per verification (fake sk-fake-).

Updated 2026-09-18 (round 6): wrapper.sh dropped the fictitious `PLANDEX_MODELS_FILE` env (absent from the live v2 binary) in favor of HOME relocation, matching the adapter `plan_wrapper` fix; custom-models file content fixtures are unchanged (the v2 `custom-models.json` schema is the doc §5 shape they already carry).
