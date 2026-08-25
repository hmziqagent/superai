# Plandex 2 — Configuration Reference (plandex-ai/plandex)

Compiled from primary sources on 2026-08-25. Sources:
- https://github.com/plandex-ai/plandex/blob/main/docs/docs/environment-variables.md
- https://github.com/plandex-ai/plandex/blob/main/docs/docs/models/model-providers.md
- https://github.com/plandex-ai/plandex/blob/main/docs/docs/models/custom-models.md
- https://github.com/plandex-ai/plandex/blob/main/docs/docs/models/model-settings.md

Plan-first coding agent with a client/server split (Go CLI + server), version-controlled model settings, and a role-based model-pack system.

## 1. Config surfaces

- **CLI**: no config file — everything is env vars + per-plan model settings (stored in plan, version controlled).
- **Custom models file**: `plandex models custom` (or `\models custom` in REPL) opens/creates a JSON file (schema: `https://plandex.ai/schemas/models-input.schema.json`) defining custom providers/models/packs.
- **Server** (self-host): configured via env vars in `app/docker-compose.yml` or your own environment.

## 2. Environment variables

### CLI — providers
| Var | Provider |
|---|---|
| `OPENROUTER_API_KEY` | OpenRouter (quickest path; also automatic failover target) |
| `OPENAI_API_KEY` (+ `OPENAI_ORG_ID`) | OpenAI direct |
| `ANTHROPIC_API_KEY` | Anthropic direct |
| `GEMINI_API_KEY` | Google AI Studio |
| `GOOGLE_APPLICATION_CREDENTIALS`, `VERTEXAI_PROJECT`, `VERTEXAI_LOCATION` | Google Vertex AI |
| `AZURE_OPENAI_API_KEY`, `AZURE_API_BASE`, `AZURE_API_VERSION`, `AZURE_DEPLOYMENTS_MAP` | Azure OpenAI |
| `DEEPSEEK_API_KEY` / `PERPLEXITY_API_KEY` | DeepSeek / Perplexity |
| `PLANDEX_AWS_PROFILE`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`, `AWS_SESSION_TOKEN`, `AWS_INFERENCE_PROFILE_ARN` | Amazon Bedrock |

### CLI — general
- `PLANDEX_API_HOST` — override API host (default `https://api.plandex.ai`; `http://localhost:8099` in dev)
- `PLANDEX_ENV=development` — target local dev server
- `PLANDEX_SKIP_UPGRADE=1` — skip auto-upgrade check

### Server (self-host)
- `GOENV` (`development`|`production`), `PORT` (default 8099), `API_HOST`
- `PLANDEX_BASE_DIR` — data dir (default `$HOME/plandex-server` dev / `/plandex-server` prod)
- `DATABASE_URL` — PostgreSQL DSN (docker-compose default provided)
- `LOCAL_MODE` — local mode toggle
- `OLLAMA_BASE_URL` — Ollama endpoint when server runs in Docker against host models

## 3. Provider selection & failover

- Direct-provider keys take precedence over OpenRouter for their own models.
- With `OPENROUTER_API_KEY` set alongside others, Plandex **fails over to OpenRouter** on provider errors — built-in redundancy layer.

## 4. Model roles & packs

Per-plan roles (JSON via `set-model [--json]`, defaults via `set-model default`):
`planner`, `coder`, `architect`, `summarizer`, `builder`, `wholeFileBuilder`, `names`, `commitMessages`, `autoContinue`.

Each role = string modelId OR object with:
```jsonc
{
  "modelId": "anthropic/claude-sonnet-4",
  "temperature": 0.7, "topP": 0.9,
  "strongModel": "openai/o3-medium",          // strong/weak split for builder roles
  "largeContextFallback": { "modelId": "google/gemini-2.5-pro", "largeOutputFallback": "openai/o4-mini-low" },
  "errorFallback": "openai/gpt-4.1"
}
```

## 5. Custom models/providers (the third-party integration)

```bash
plandex models custom   # creates/opens JSON template with schema autocomplete
```
```jsonc
{
  "$schema": "https://plandex.ai/schemas/models-input.schema.json",
  "providers": [
    { "name": "togetherai", "baseUrl": "https://api.together.xyz/v1", "apiKeyEnvVar": "TOGETHER_API_KEY" },
    { "name": "local-llm", "baseUrl": "http://localhost:8080/v1", "skipAuth": true }   // Ollama/vLLM/lmstudio style
  ],
  "models": [
    {
      "modelId": "my-model", "publisher": "...",
      "maxTokens": 128000, "maxOutputTokens": 8192, "reservedOutputTokens": 8192,
      "defaultMaxConvoTokens": 50000,
      "preferredOutputFormat": "xml",            // or "tool-call-json"
      "providers": [
        { "provider": "custom", "customProvider": "togetherai", "modelName": "exact-id-on-provider" },
        { "provider": "openrouter", "modelName": "vendor/model" }
      ]
    }
  ],
  "modelPacks": [ { "name": "custom-pack", "planner": "...", "coder": "...", /* full role map */ } ]
}
```
Support matrix: **self-hosted = everything**; Cloud+BYO keys = custom models/packs on built-in providers; Cloud integrated = custom packs of built-in models only.

Provider fields: `name`, `baseUrl` (must be OpenAI-compatible), `apiKeyEnvVar`, `skipAuth`, `extraAuthVars`.

## MULTI-INSTANCE WRAPPERS

Mechanisms: all-provider state is env-driven → per-instance env switching is trivial; custom models JSON is per-user-file (edit per profile); self-hosted servers are per-deploy (`PLANDEX_API_HOST` points the CLI at any server).

```bash
#!/usr/bin/env bash
# plandex-openrouter: everything through OpenRouter
export OPENROUTER_API_KEY="sk-or-..."
exec plandex "$@"
---
#!/usr/bin/env bash
# plandex-anthropic: direct Anthropic + self-hosted server
export ANTHROPIC_API_KEY="sk-ant-..."
export PLANDEX_API_HOST="https://plandex.mydomain.dev"
exec plandex "$@"
```
For hard isolation run two self-hosted stacks (different `PORT`/`DATABASE_URL`/`PLANDEX_BASE_DIR`) and point each wrapper at its own `PLANDEX_API_HOST`.

## Sources
The four URLs above, fetched 2026-08-25.
