# DeepSeek Harness (dsh) — Configurable Options Reference

DeepSeek's official agent harness. MIT, `deepseek-ai/deepseek-harness`, ~195k stars, launched **2026-08-13** as a Developer Preview. Tagline: "Everything is a Plugin."

DeepSeek stood up a dedicated harness team in March 2026 explicitly benchmarking against Claude Code. Framing is `Agent = Model + Harness` — the model thinks; the harness reads files, runs the terminal, calls tools.

Two surfaces: `npx @deepseek-ai/dsh web` (local web UI) and `dsh cli`, an interactive full-screen terminal agent. npm `@deepseek-ai/dsh` is at `0.1.1-rc.2`, described as "dsh CLI: profile boot, plugin management, and the browser UI alias."

⚠️ **Developer preview.** The README warns of compatibility-breaking changes to APIs and plugin contracts. For superai this is the highest-churn harness in the set — pin a version and expect the config layer's version gating to earn its keep here first.

Verified 2026-08-26 against the repo (`docs/config-catalog.md`) and the npm registry.

## 1. Home directory and config

| Path | Purpose |
|---|---|
| `~/.dsh` | Harness home. Config root |
| `$DSH_HOME` | Overrides it |

Resolution, quoted from `config-catalog.md`: "Harness home used when `path` is omitted; defaults to `$DSH_HOME` or `~/.dsh`." Also "Harness home containing the fixed user-global `AGENTS.md`."

`DSH_HOME` is both read as an override and exposed to child processes.

## 2. Third-party provider registration

The provider model is `providers`, keyed by route — "the `providers` dict key IS the route". Built on `@earendil-works/pi-ai`, the same engine behind [pi.md](pi.md).

| Key | Meaning |
|---|---|
| `apiKeyEnv` | **Env var name**, not the key itself — "credential reference (environment-variable name) resolved per request" |
| `baseURL` | Endpoint for this route's models; defaults to the installed catalog's endpoint |
| `api` | Wire protocol every model on this route speaks. A route the catalog doesn't ship must name one |
| `models` | Explicit list *replaces* the catalog for that route |
| `modelOverrides` | Reshapes individual catalog models while the rest keep serving. Only valid on a catalog route with no `models` list |
| `compat` | Wire-compatibility switches (below) |
| `defaultContextWindow` | Default 262,144 |
| `defaultMaxTokens` | Default 32,768 |

`apiKeyEnv` taking a variable *name* rather than a value is the right shape for superai: keys stay in the keychain and never enter the config file.

Note the failure posture — an override on a route the catalog doesn't ship, or naming a model the catalog doesn't describe, is "refused rather than skipped". Config errors surface loudly instead of silently no-op'ing. Good for a generator to rely on.

## 3. `compat` — the wire-quirk catalogue

Worth reading in full even if you never use dsh, because it enumerates the ways "OpenAI-compatible" endpoints actually differ. Directly applicable to superai's proxy translation layer. Most apply to `openai-completions`:

| Switch | What it papers over |
|---|---|
| `supportsDeveloperRole` | Endpoint accepts the `developer` role for system prompt; `false` keeps `system` |
| `supportsStore` | Endpoint accepts `store` |
| `supportsReasoningEffort` | Endpoint accepts `reasoning_effort` |
| `supportsUsageInStreaming` | Endpoint accepts `stream_options: {include_usage: true}` |
| `maxTokensField` | *Which* output-cap field the endpoint reads |
| `requiresToolResultName` | Tool results must carry `name` |
| `requiresAssistantAfterToolResult` | A user message after tool results needs an assistant message between |
| `requiresThinkingAsText` | Thinking blocks must travel as text in `<thinking>` delimiters |
| `requiresReasoningContentOnAssistantMessages` | Replayed assistant messages need an empty `reasoning_content` while reasoning is on |
| `thinkingFormat` | Reasoning parameter format the endpoint expects |
| `chatTemplateKwargs` | Sent as `chat_template_kwargs`, read only under the two `chat-template` thinking formats |

Precedence: route-level `compat` defaults every model on the route; each model's own `compat` overrides per field; what neither sets falls back to the installed catalog entry, then pi-ai's own detection. "A switch no model on the route could read is refused rather than left looking applied."

One documented sharp edge: `chatTemplateKwargs` set beside a non-`chat-template` thinking format are "sent nowhere", and nothing checks the pairing — because the format in force may come from the catalog or from pi-ai's baseURL detection, neither of which resolution can read.

## 4. Multi-instance wrappers

```sh
#!/bin/sh
# /usr/local/bin/dsh-work
export DSH_HOME="$HOME/.dsh-work"
exec dsh "$@"
```

npm's "profile boot" suggests a first-class profile mechanism beyond the env var — not yet traced. See §Unverified.

## Key doc URLs

- https://github.com/deepseek-ai/deepseek-harness
- https://github.com/deepseek-ai/deepseek-harness/blob/main/docs/config-catalog.md
- https://github.com/deepseek-ai/deepseek-harness/blob/main/docs/architecture.md
- https://github.com/deepseek-ai/deepseek-harness/blob/main/docs/api-gateway.md
- https://www.npmjs.com/package/@deepseek-ai/dsh

## Unverified

`config-catalog.md` is 3,352 lines and only the home-directory and provider/compat sections were read. Not yet traced: the plugin system and its contracts, `docs/api-gateway.md`, `docs/agent-lifecycle.md`, the "profile boot" mechanism, skills/`AGENTS.md` handling, sandbox runner config (`runnerCommand`, bwrap), and the full env-var list. This file covers the routing surface, not the harness.
