# Continue.dev — Complete Configuration Reference
*(VS Code / JetBrains extensions + Continue CLI `cn`)*

Compiled 2026-08-25 from primary sources: [docs.continue.dev/reference](https://docs.continue.dev/reference), [.../customize/deep-dives/configuration](https://docs.continue.dev/customize/deep-dives/configuration), [.../reference/yaml-migration](https://docs.continue.dev/reference/yaml-migration), [.../guides/configuring-models-rules-tools](https://docs.continue.dev/guides/configuring-models-rules-tools), [.../faqs](https://docs.continue.dev/faqs), [.../cli/configuration](https://docs.continue.dev/cli/configuration), [.../cli/quickstart](https://docs.continue.dev/cli/quickstart), [.../cli/headless-mode](https://docs.continue.dev/cli/headless-mode), [.../customize/deep-dives/custom-providers](https://docs.continue.dev/customize/deep-dives/custom-providers). ⚠️ **Continue was acquired by Cursor in June 2026 — see §6 Status note at the end** ([continue.dev homepage](https://continue.dev/), [The New Stack](https://thenewstack.io/cursor-acquires-continue-coding/)).

---

## 1. Configuration: `config.yaml` complete schema

Location: `~/.continue/config.yaml` (macOS/Linux) or `%USERPROFILE%\.continue\config.yaml` (Windows). Auto-created on first use; auto-refreshed on save from the IDE ([configuration deep dive](https://docs.continue.dev/customize/deep-dives/configuration)). Full reference: [docs.continue.dev/reference](https://docs.continue.dev/reference).

### 1.1 Top-level properties

All properties optional unless marked required ([reference](https://docs.continue.dev/reference#properties)):

| Property | Req | Description |
|---|---|---|
| `name` | **required** | Name of project/configuration |
| `version` | **required** | Version string, e.g. `0.0.1`, `1.0.0` |
| `schema` | **required** | Schema version, currently `v1` |
| `models` | – | Array of model entries (below) |
| `context` | – | Context providers array |
| `rules` | – | Rules concatenated into system message for Agent/Chat/Edit |
| `prompts` | – | Slash-command prompt files |
| `docs` | – | Doc sites to index |
| `mcpServers` | – | MCP servers (tools) |
| `data` | – | Development-data export destinations |

```yaml
name: My Config
version: 1.0.0
schema: v1
```

### 1.2 `models[]` entries

Each model requires `name`, `provider`, `model`; everything else is optional ([reference#models](https://docs.continue.dev/reference#models)):

- **`name`** *(required)* — unique identifier shown in the UI.
- **`provider`** *(required)* — e.g. `openai`, `anthropic`, `ollama` (see §3).
- **`model`** *(required)* — model id, e.g. `gpt-4o`, `starcoder`. Special value `AUTODETECT` for Ollama auto-detects installed models.
- **`apiBase`** — override the provider's default endpoint URL.
- **`apiKey`** — provider API key; use `${{ secrets.NAME }}` (see §2).
- **`roles`** — array of `chat | autocomplete | embed | rerank | edit | apply | summarize` (summarize currently unused). Default: `[chat, edit, apply, summarize]`.
- **`capabilities`** — overrides autodetection; supported values include `tool_use` (required for Agent mode) and `image_input` ([model capabilities guide](https://docs.continue.dev/customize/deep-dives/model-capabilities)).
- **`maxStopWords`** — cap stop-word list to avoid API errors.
- **`promptTemplates`** — override built-in templates per role; keys `chat` (named template like `llama3`/`anthropic`), `edit`, `apply`, `autocomplete`.
- **`useLegacyCompletionsEndpoint`** (`true|false`) — force `/completions` instead of chat completions (for local servers).
- **`useResponsesApi`** (`false`) — disable OpenAI Responses API.
- **`env`** — environment-variable block usable with hub blocks' `with:` mapping (e.g. `with: { ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }} }`) ([configuring guide](https://docs.continue.dev/guides/configuring-models-rules-tools)).
- **`chatOptions`** (if role includes `chat`) — `baseSystemMessage` (Chat mode), `baseAgentSystemMessage` (Agent mode), `basePlanSystemMessage` (Plan mode) system-prompt overrides.
- **`embedOptions`** (role `embed`) — `maxChunkSize` (min 128 tokens), `maxBatchSize` (min 1).
- **`autocompleteOptions`** (role `autocomplete`):
  - `disable`, `maxPromptTokens`, `debounceDelay` (ms), `modelTimeout` (ms)
  - `maxSuffixPercentage`, `prefixPercentage`
  - `transform` (default `true`; set `false` to keep raw multiline completions)
  - `template` — Mustache with `{{{ prefix }}} {{{ suffix }}} {{{ filename }}} {{{ reponame }}} {{{ language }}}`
  - `onlyMyCode`, `useCache`, `useImports`, `useRecentlyEdited`, `useRecentlyOpened`
- **`defaultCompletionOptions`**:
  - `contextLength`, `maxTokens`, `temperature` (0.0–1.0), `topP`, `topK`, `stop[]`
  - `reasoning: true` (Anthropic Claude 3.7+, some Ollama models), `reasoningBudgetTokens` (Anthropic thinking budget)
  - `keepAlive` (Ollama seconds-to-stay-loaded; default 1800)
- **`requestOptions`**:
  - `timeout`, `verifySsl`, `caBundlePath` (string or array), `proxy`, `noProxy[]`
  - `headers` (map — common apiBase-auth workaround: `Authorization: Bearer <key>` or `X-Auth-Token`)
  - `extraBodyProperties` (merged into request body)
  - `clientCertificate`: `{ cert, key, passphrase }`

Example ([reference#models](https://docs.continue.dev/reference#models)):

```yaml
name: My Config
version: 1.0.0
schema: v1
models:
  - name: GPT-4o
    provider: openai
    model: gpt-4o
    roles: [chat, edit, apply]
    defaultCompletionOptions:
      temperature: 0.7
      maxTokens: 1500
  - name: Codestral
    provider: mistral
    model: codestral-latest
    roles: [autocomplete]
    autocompleteOptions:
      debounceDelay: 250
      maxPromptTokens: 1024
      onlyMyCode: true
  - name: My Model - OpenAI-Compatible
    provider: openai
    apiBase: http://my-endpoint/v1
    model: my-custom-model
    capabilities: [tool_use, image_input]
    roles: [chat, edit]
```

YAML anchors dedupe shared model fields (requires `%YAML 1.1` header) ([reference#anchors](https://docs.continue.dev/reference#using-yaml-anchors-to-avoid-config-duplication)):

```yaml
%YAML 1.1
---
name: My Config
version: 1.0.0
schema: v1
model_defaults: &model_defaults
  provider: openai
  apiKey: my-api-key
  apiBase: https://api.example.com/llm
models:
  - name: mistral
    <<: *model_defaults
    model: mistral-7b-instruct
    roles: [chat, edit]
```

### 1.3 `rules`

Strings or `uses:` references to rule blocks/files; concatenated into the system message for Agent, Chat, and Edit requests ([reference#rules](https://docs.continue.dev/reference#rules); [rules deep dive](https://docs.continue.dev/customize/deep-dives/rules)):

```yaml
rules:
  - Give concise responses
  - uses: sanity/sanity-opinionated
  - uses: file:///Users/me/Desktop/rules.md   # local rules file with YAML frontmatter `name:`
```

Organize reusable local rules in `.continue/rules/` (global) or workspace-level `.continue/rules/` ([configuring guide](https://docs.continue.dev/guides/configuring-models-rules-tools#organization)).

### 1.4 `prompts`

Slash-invokable prompts (`/name`). Markdown files with frontmatter (`name:`, `invokable: true` makes it parameterizable) or inline `prompt:` ([reference#prompts](https://docs.continue.dev/reference#prompts)):

```yaml
prompts:
  - uses: supabase/create-functions          # hub block
  - uses: file:///Users/me/Desktop/prompts.md
  - name: test
    description: Unit test a function
    prompt: |
      Please write a complete suite of unit tests for this function...
```

Local prompts live in `.continue/prompts/`.

### 1.5 `docs`

Indexed documentation sites ([reference#docs](https://docs.continue.dev/reference#docs)):

- **`name`** *(required)* — display name
- **`startUrl`** *(required)* — crawl start page
- **`favicon`** — defaults to `<startUrl>/favicon.ico`
- **`useLocalCrawling`** — skip the default cloud crawler

```yaml
docs:
  - name: Continue
    startUrl: https://docs.continue.dev/intro
    favicon: https://docs.continue.dev/favicon.ico
```

### 1.6 `data`

Development-data export destinations ([reference#data](https://docs.continue.dev/reference#data); [development data deep dive](https://docs.continue.dev/customize/deep-dives/development-data)):

- **`name`** *(required)*, **`destination`** *(required)* — HTTP POST endpoint or `file://` dir dumped as `.jsonl`
- **`schema`** *(required)* — blob schema version: `0.1.0` or `0.2.0`
- **`events[]`** — event-name filter; all events by default
- **`level`** — `all` (default) or `noCode` (strips file contents/prompts/completions)
- **`apiKey`** — sent as Bearer header
- **`requestOptions`** — same format as model requestOptions

```yaml
data:
  - name: Local Data Bank
    destination: file:///path/to/dir
    schema: 0.2.0
    level: all
  - name: My Private Company
    destination: https://mycompany.com/ingest
    schema: 0.2.0
    level: noCode
    events: [autocomplete, chatInteraction]
```

### 1.7 `mcpServers` / context

MCP server entry properties ([reference#mcpservers](https://docs.continue.dev/reference#mcpservers)):

- **`name`** *(required)*, **`command`** *(required)* (stdio transport)
- `args[]`, `env` (map for the server process), `cwd`
- `requestOptions` — for `sse`/`streamable-http` servers (same shape as model requestOptions)
- `connectionTimeout` — initial-connection timeout

```yaml
mcpServers:
  - name: My MCP Server
    command: uvx
    args: [mcp-server-sqlite, --db-path, ./test.db]
    cwd: /Users/NAME/project
    env:
      NODE_ENV: production
```

Reusable MCP tool blocks live in `.continue/mcpServers/` locally, or are pulled from the Hub by slug (`--mcp <slug>` on the CLI) ([configuring guide](https://docs.continue.dev/guides/configuring-models-rules-tools), [CLI quickstart](https://docs.continue.dev/cli/quickstart)). See §5 for context providers.

### 1.8 Hub blocks: `uses` / `with` / `override`

Any of models/rules/prompts can import Hub blocks by `owner/slug` and customize them ([configuring guide](https://docs.continue.dev/guides/configuring-models-rules-tools)):

```yaml
models:
  - uses: anthropic/claude-sonnet-4-6
    with:                       # maps inputs → secrets/values
      ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
      TEMP: 0.9
    override:                   # override block properties
      roles: [chat]
      defaultCompletionOptions:
        temperature: 0.8
```

`${{ inputs.NAME }}` placeholders inside a block are filled from the consumer's `with:` mapping; `${{ secrets.NAME }}` is resolved directly from `.env` sources (§2).

### 1.9 `config.json` legacy migration

`config.yaml` replaces deprecated `config.json`; if both exist, `config.yaml` wins ([migration guide](https://docs.continue.dev/reference/yaml-migration)). Key mappings:

| Legacy config.json | config.yaml |
|---|---|
| `models[].title` | `models[].name` |
| `models[]` (role implied) | `roles: [chat]` |
| `tabAutocompleteModel` | entry with `roles: [autocomplete]` |
| `embeddingsProvider` | entry with `roles: [embed]`; `maxEmbeddingChunkSize/BatchSize` → `embedOptions.maxChunkSize/maxBatchSize` |
| `reranker` | entry with `roles: [rerank]` |
| `experimental.modelRoles` (`inlineEdit`→edit, `applyCodeBlock`→apply) | plain `roles` array |
| `completionOptions` | `defaultCompletionOptions` |
| `contextProviders[{name,params}]` | `context: [{provider,params}]` |
| `systemMessage` | `rules:` array |
| `customCommands` | `prompts:` |
| `docs[].title` | `docs[].name` (`startUrl` unchanged) |
| `experimental.modelContextProtocolServers` | top-level `mcpServers:` |

Deprecated without a YAML equivalent (auto-migrated to IDE user settings): `slashCommands`, top-level `requestOptions`/`completionOptions`, `tabAutocompleteOptions.*`, `analytics`, `customCommands`, `experimental`, `userToken`. `repoMapFileSelection` remains JSON-only.

Legacy extras ([configuration deep dive](https://docs.continue.dev/customize/deep-dives/configuration)):
- **`.continuerc.json`** — workspace-level JSON config merged over `config.json`; extra property `mergeBehavior: "merge"|"overwrite"` (default merge).
- **`config.ts`** — programmatic override at `~/.continue/config.ts`; must export `modifyConfig(config): Config` (TypeScript SDK access to `sdk.ide.*`, `sdk.llm.streamComplete`).

---

## 2. Environment variables & secrets

Secret syntax anywhere in `config.yaml`: **`${{ secrets.SECRET_NAME }}`**, e.g. `apiKey: ${{ secrets.OPENAI_API_KEY }}` ([FAQs](https://docs.continue.dev/faqs#managing-local-secrets-and-environment-variables), [configuring guide](https://docs.continue.dev/guides/configuring-models-rules-tools#working-with-secrets)).

Resolution order when `${{ secrets.X }}` is encountered ([FAQs](https://docs.continue.dev/faqs#where-secrets-are-resolved-from)):

1. Workspace `.env` — `<workspace-root>/.env`
2. Workspace Continue `.env` — `<workspace-root>/.continue/.env`
3. Global `.env` — `~/.continue/.env`
4. Process environment variables — **CLI only**

⚠️ IDE extensions cannot read shell-exported variables (`export OPENAI_API_KEY=...` does nothing in VS Code/JetBrains); use `.env` files there. The CLI reads process env directly: `export OPENAI_API_KEY=sk-... && cn` ([FAQs](https://docs.continue.dev/faqs#managing-local-secrets-and-environment-variables)).

Common provider key names used in configs/examples: `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY`, `VOYAGE_API_KEY`, plus arbitrary names for custom endpoints (any name works — it's just an env lookup). For hub blocks, map secret names into block inputs via `with:` (§1.8). Use `inputs.` only when you want users to remap which secret feeds a reusable block.

CLI-specific auth env var: **`CONTINUE_API_KEY`** — headless/CI login without a browser (`export CONTINUE_API_KEY=... && cn -p "..."`), alternative to `cn login` browser flow or entering an Anthropic key interactively ([CLI quickstart](https://docs.continue.dev/cli/quickstart#authentication)).

Other notable env vars: `OLLAMA_HOST=0.0.0.0:11434` (+`OLLAMA_ORIGINS=*`) for remote Ollama targets ([FAQs/Ollama](https://docs.continue.dev/faqs#ollama-issues)); MCP server processes receive their own `env:` map from config.

---

## 3. Third-party & local model providers

Providers are selected via `provider:`; each has a documented default `apiBase` you can override ([model-providers pages](https://docs.continue.dev/customize/model-providers/top-level/openai), [chat role page](https://docs.continue.dev/customize/model-roles/chat)):

**Cloud:** `openai`, `anthropic`, `azure` (Azure OpenAI), `azureai`, `bedrock` + `bedrockimport` (AWS), `gemini` (Google), `mistral`, `deepseek`, `together`, `openrouter`, `cohere`, `voyage` (embed/rerank), `xai`, `huggingface` (Inference API/TGI), `groq`, `novita`, `lamini`, `flowise`, `kindproxy`, plus others under docs.continue.dev/customize/model-providers/.

**Local:** `ollama` (default base `http://localhost:11434`; supports `apiBase` for remote hosts and `AUTODETECT`), `lmstudio` (`http://localhost:1234/v1`), `vllm`, `llamacpp`, `msty`, `jan`, `sagemaker`.

**Universal OpenAI-compatible pattern** ([openai provider page](https://docs.continue.dev/customize/model-providers/top-level/openai#openai-api-compatible-providers)) — point `provider: openai` at ANY compatible endpoint via `apiBase`:

```yaml
models:
  - name: vLLM Qwen
    provider: openai            # generic OpenAI driver
    model: qwen2.5-coder-32b-instruct
    apiBase: http://localhost:8000/v1
    apiKey: <YOUR_CUSTOM_API_KEY>
    capabilities: [tool_use]
    useLegacyCompletionsEndpoint: false   # true forces /v1/completions
```

Auth alternatives for custom endpoints: `apiKey` field, or `requestOptions.headers.Authorization/X-Auth-Token` (migration example shows `X-Auth-Token` headers against `http://3.3.3.3/v1`) ([migration guide](https://docs.continue.dev/reference/yaml-migration#models)). Self-hosted TLS: `requestOptions.caBundlePath` / `clientCertificate` ([troubleshooting](https://docs.continue.dev/troubleshooting#ssl-certificate-errors)).

Provider-specific knobs: `useResponsesApi: false` (OpenAI), `defaultCompletionOptions.keepAlive` (Ollama), `reasoning`/`reasoningBudgetTokens` (Claude 3.7+).

---

## 4. Multi-instance wrappers

### 4.1 Config resolution & per-workspace configuration

- Global config: `~/.continue/config.yaml` applies everywhere ([configuration deep dive](https://docs.continue.dev/customize/deep-dives/configuration)).
- Local organization layers: global vs **workspace** `.continue/{models,rules,mcpServers,prompts}/` directories apply automatically when working in that project ([configuring guide](https://docs.continue.dev/guides/configuring-models-rules-tools#local)).
- Legacy per-workspace overlay: `.continuerc.json` in repo root (JSON schema + `mergeBehavior`) ([configuration deep dive](https://docs.continue.dev/customize/deep-dives/configuration#how-to-use-continuercjson-for-workspace-configuration)).

**Directory relocation:** the docs consistently hard-code `~/.continue` / `%USERPROFILE%\.continue` and describe reset-by-deletion of that exact directory ([FAQs](https://docs.continue.dev/faqs#how-do-i-reset-the-state-of-the-extension)). **No documented `CONTINUE_GLOBAL_DIR`-style env var exists** in the current documentation — do not rely on one. To run isolated instances, use the wrapper techniques below instead.

### 4.2 Multiple named models + quick switch

Define several `models[]` entries; the IDE lets you pick per-mode from dropdowns, and role assignment routes them automatically (one model can serve multiple roles). In the CLI TUI, `/config` lists available configs to switch mid-session ([CLI configuration page's saved-config behavior](https://docs.continue.dev/cli/configuration)). On the CLI you can also compose at launch:

```bash
cn --config ./team-review.yaml          # named config file or assistant slug
cn --model anthropic/claude-sonnet-4-6 --rule ./rules/style.md --mcp github
```

### 4.3 `cn` CLI headless usage ([headless mode](https://docs.continue.dev/cli/headless-mode), [quickstart](https://docs.continue.dev/cli/quickstart))

```bash
cn -p "prompt"                          # single-shot, prints to stdout
git diff --staged | cn -p "Write a commit message"
cn -p "..." --silent                    # strip <think> tags/whitespace
cn -p "..." --format json               # structured output
cn -p "Fix errors" --allow Write --allow Edit --allow Bash   # 'ask'-permission tools are excluded in headless otherwise
cn -p "..." --readonly                  # plan mode
cn -p --resume                          # replay last session headlessly
```

Flags (repeatable where noted): `-p`, `--config <path|slug>`, `--resume`, `--auto`, `--readonly`, `--allow <tool>`, `--exclude <tool>`, `--rule <rule>`, `--mcp <slug>`, `--model <slug>`, `--agent <slug>`, `--verbose`. Install: `curl -fsSL https://raw.githubusercontent.com/continuedev/continue/main/extensions/cli/scripts/install.sh | bash` (or npm; Node 20+). Auth: `cn login` (browser), interactive Anthropic key, or `CONTINUE_API_KEY` env var for CI.

### 4.4 Wrapper example — two providers, two instances

Since there is no global-dir override, isolate "instances" by pairing explicit `--config` files with scoped env vars (works because the CLI reads process env):

```bash
#!/usr/bin/env bash
# continue-wrapper.sh — two-provider multi-instance launcher
set -euo pipefail
CONF_DIR="$HOME/continue-profiles"     # your own profile store (not a supported relocation)

case "${1:-}" in
  claude)
    export ANTHROPIC_API_KEY="$(cat "$CONF_DIR/anthropic.key")"
    exec cn --config "$CONF_DIR/claude.yaml" "${@:2}"
    ;;
  local)
    unset ANTHROPIC_API_KEY
    exec cn --config "$CONF_DIR/local.yaml" "${@:2}"
    ;;
  *) echo "usage: $0 claude|local [args]" >&2; exit 1 ;;
esac
```

`$HOME/continue-profiles/local.yaml` (all-local, no account needed):

```yaml
name: local-instance
version: 0.0.1
schema: v1
models:
  - name: Ollama Chat
    provider: ollama
    model: llama3.1:8b
    roles: [chat, edit, apply]
  - name: Ollama Codestral
    provider: ollama
    model: codestral-latest        # or starcoder2:3b
    roles: [autocomplete]
  - name: Local Embedder
    provider: openai               # any OpenAI-compatible server
    model: nomic-embed-text
    apiBase: http://localhost:8000/v1
    roles: [embed]
rules:
  - Prefer concise responses
```

`$HOME/continue-profiles/claude.yaml`:

```yaml
name: claude-instance
version: 0.0.1
schema: v1
models:
  - uses: anthropic/claude-sonnet-4-6
    with:
      ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
    override:
      roles: [chat, edit, apply]
```

For IDE-side isolation the practical equivalents are: different OS users, per-workspace `.continue/` dirs + workspace `.env` files (secrets resolve workspace-first), or `.continuerc.json` overlays.

---

## 5. Roles system, context providers, MCP

### Roles ([model-roles/chat](https://docs.continue.dev/customize/model-roles/chat), [reference](https://docs.continue.dev/reference#models))

| Role | Purpose |
|---|---|
| `chat` | Chat mode; also backs Edit and Apply when those roles have no dedicated model |
| `edit` | Inline edits (Ctrl/Cmd+I) |
| `apply` | Applying agent/code-block diffs into files |
| `autocomplete` | Tab autocompletion (small fast model recommended, e.g. Codestral, Starcoder2) |
| `embed` | Codebase indexing embeddings (`embedOptions.maxChunkSize` min 128) |
| `rerank` | Reranking retrieved chunks (e.g. `voyage` `rerank-2`) |
| `summarize` | Defined but not currently used |

Default when omitted: `[chat, edit, apply, summarize]`. Agent mode additionally requires `tool_use` capability (native or system-message fallback) ([FAQs/Agent mode](https://docs.continue.dev/faqs#agent-mode-is-unavailable-or-tools-arent-working)).

### Context providers (@ mentions) ([custom-providers deep dive](https://docs.continue.dev/customize/deep-dives/custom-providers))

Active built-ins: `file`, `code`, `diff`, `currentFile`, `terminal`, `open` (`params.onlyPinned`), `clipboard`, `tree`, `problems`, `debugger` (`params.stackDepth`, VS Code only), `repo-map` (`params.includeSignatures`), `os`, `http` (`params.url` + `headers`; server must return `{name,description,content}` item(s)), plus the `mcp` provider that surfaces configured MCP resources.

Deprecated providers (recommend MCP replacements): `codebase`, `folder`, `docs`, `greptile`, `commits`, `discord`, `jira`, `gitlab-mr`, `google`, `database`, `issue`, `url`, `search`, `web`.

Config form: `context: [{provider: ..., name: ..., params: {...}}]`.

### MCP

Top-level `mcpServers` (§1.7) covers stdio (`command`/`args`/`env`/`cwd`) and remote (`requestOptions` for sse/streamable-http, `connectionTimeout`). Tools appear automatically in Agent mode; resources via the `@MCP` context entry ([reference#mcpservers](https://docs.continue.dev/reference#mcpservers)). macOS gotcha: fully-qualify commands (`/usr/local/bin/npx`) to avoid `spawn ENAMETOOLONG` ([troubleshooting](https://docs.continue.dev/troubleshooting#spawn-enametoolong-error-on-macos)). CLI adds servers ad hoc via `--mcp <slug>`.

---

## 6. Post-acquisition status note

- **Cursor acquired Continue around mid-June 2026.** Continue's homepage was replaced with "Continue (acquired by Cursor)" and a FAQ; recurring billing disabled and users given until **July 15, 2026** to export platform data before deletion ([continue.dev](https://continue.dev/), [The New Stack, ~June 2026](https://thenewstack.io/cursor-acquires-continue-coding/)).
- Practical consequences for configuration: the **open-source extension, `config.yaml` schema, and `cn` CLI remain usable with local/API-key models** (the docs above still describe IDE + CLI operating independently of the hosted platform), but **Hub blocks (`uses: owner/slug`), `cn login` platform sync, and cloud features (hosted crawling, assistants) are tied to the sunsetted platform** — prefer fully local configs (own `apiBase`s, `.env` secrets, local `models/rules/prompts/mcpServers` directories).
- Community reports indicate post-acquisition development has narrowed (forum discussion of the plugin continuing mainly as autocomplete with Sweep/Zeta NES models — unverified, treat as anecdotal: [level1techs forum](https://forum.level1techs.com/t/continue-dev-acquired-by-cursor-acquired-by-spacex/251651)). Docs URLs may rot; archived copies of docs.continue.dev are advisable for long-term reference.
