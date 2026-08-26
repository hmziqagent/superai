# ZCode (Z.ai) — Configurable Options Reference

Z.ai (Zhipu) desktop agentic development environment, built around GLM. Proprietary Electron app; agent conversation at the center with file manager, terminal, git panel, and browser preview around it. macOS and Windows, Linux in beta. The app is free — you pay for the models you connect.

Verified 2026-08-26 against zcode.z.ai official docs.

⚠️ **Name collision — read before wiring anything.** Searching for "ZCode config" surfaces mostly community projects, not the official app:

- `guizmo-ai/zai-glm-cli` / npm `@guizmo-ai/zai-cli` — third-party CLI, config at `~/.config/zai/config.json`, env vars `ZAI_API_KEY` / `ZAI_BASE_URL` / `ZAI_MODEL`
- `geoh/z.ai-powered-claude-code` — a recipe for pointing Claude Code at Z.ai endpoints

Neither is ZCode. Their paths and env vars do **not** apply to the official app. If superai wants Claude-Code-against-GLM, that is a Claude Code instance with `ANTHROPIC_BASE_URL` repointed — handled in [claude-code.md](claude-code.md), not here.

## 1. Config file

| Path | Purpose |
|---|---|
| `~/.zcode/v2/config.json` | Provider `options` are stored here |

Note the `v2` path segment — the version is *in the path*. A future ZCode will likely write `v3/`, so superai's config layer should treat the version segment as part of schema resolution rather than a constant.

## 2. Environment variables

Effectively none for configuration. The one documented behavior: the HTTP proxy setting does **not** read system environment variables, but when a proxy is configured in Settings, ZCode injects `HTTP_PROXY` into terminal command subprocesses.

**No documented config-dir relocation variable.** ZCode is a GUI app with a fixed path — the same category as Claude Desktop. superai must write `~/.zcode/v2/config.json` in place; there is no wrapper/alias path to isolated instances.

## 3. Third-party provider registration

Good news for routing: both wire formats are supported, and this is a first-class UI feature ("Connect Models & Plans").

Flow: model selector or welcome screen → **Add Provider** → set the endpoint → enter API key → **Add Model** with the model IDs that service supports → enable via toggle.

| Protocol | Field to set |
|---|---|
| Anthropic | "Anthropic endpoint" → base URL |
| OpenAI | "API base URL" → base URL |

Z.ai's own documented example uses DeepSeek: `https://api.deepseek.com/anthropic` for the Anthropic protocol, `https://api.deepseek.com/v1` for OpenAI. So pointing ZCode at a local superai proxy is squarely supported on either wire format.

**Limitation, quoted:** "Custom request parameters for third-party models is not supported yet" — only `apiKey`, `baseURL`, and `headers` are recognized in the config file. No per-model request shaping, so any wire quirk has to be absorbed by the proxy rather than declared in ZCode. Contrast [deepseek-harness.md](deepseek-harness.md), which exposes a full `compat` switch set.

The docs also note that terminal environment variables and the ZCode desktop model setup are **separate entry points and are not synced automatically** — so configuring one does not configure the other.

## 4. Multi-instance wrappers

Not supported. No config-dir env var, GUI app, fixed path. Instances would require OS-level tricks (separate user accounts, container, or swapping `~/.zcode/v2/config.json` between saved copies). The last is the only realistic option for superai, and it means mutating shared state rather than isolating it — the same problem Claude Desktop and Windsurf present.

## Key doc URLs

- https://zcode.z.ai/en/docs/configuration
- https://zcode.z.ai/en/docs/qa
- https://docs.z.ai/devpack/latest-model

## Unverified

The full `config.json` schema, MCP support, skills/agent configuration, and whether the `v2` path segment has ever changed. Only the configuration and FAQ pages were read. GLM model naming and Z.ai plan tiers were not checked.
