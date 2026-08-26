# OpenClaw — Configurable Options Reference

Self-hosted agent runtime and message router — a long-running Node.js service that connects chat surfaces (WhatsApp, Discord, and others) to an agent that runs shell commands, drives browsers, and touches local files. Renamed from its earlier name in January 2026. MIT, `openclaw/openclaw`, copyright "OpenClaw Foundation".

Not a coding harness in the Claude Code sense — it is a persistent personal-agent daemon. It earns a place here because it has a first-class config-dir relocation knob and a genuinely good custom-provider model, which makes it one of the easier targets for superai's instance and proxy layers.

Verified 2026-08-26 against official docs and the repo.

## 1. Config file and paths

| Path | Purpose |
|---|---|
| `~/.openclaw/openclaw.json` | Main config. JSON5 — comments and unquoted keys are legal, so **edits must be format-preserving** |
| `~/.openclaw/.env` | Global env file, also addressable as `$OPENCLAW_STATE_DIR/.env` |
| `~/.openclaw` | Default mutable state dir |

There is also a `config.env` block inside `openclaw.json` for env vars scoped to the config itself.

## 2. Environment variables

| Variable | Effect |
|---|---|
| `OPENCLAW_HOME` | "Override the home directory used for OpenClaw path defaults." Replaces the system home for the default state directory, config path, agent directories, credentials, installer onboarding workspace, and default dev checkout |
| `OPENCLAW_STATE_DIR` | Override the mutable state directory (default `~/.openclaw`) |
| `OPENCLAW_CONFIG_PATH` | Override the active config file path (default `~/.openclaw/openclaw.json`) |
| `OPENCLAW_GIT_DIR` | Override the git dir |

**Precedence.** Explicit path variables (`OPENCLAW_STATE_DIR`, `OPENCLAW_CONFIG_PATH`, `OPENCLAW_GIT_DIR`) take precedence over `OPENCLAW_HOME`. In the OS home resolution chain, `OPENCLAW_HOME` > `$HOME` > `USERPROFILE`.

Env resolution order for provider credentials: process environment first, then `.env` in the current working directory.

## 3. Third-party provider registration

The strongest part of OpenClaw for superai's purposes: custom providers are declarative, and both wire formats are first-class.

```json5
{
  agents: { defaults: { model: { primary: "provider-id/model-name" } } },
  models: {
    mode: "merge",
    providers: {
      "provider-id": {
        baseUrl: "https://your-endpoint.com/v1",
        apiKey: "${API_KEY_ENV_VAR}",
        api: "openai-completions",   // or "anthropic-messages"
        timeoutSeconds: 300,
        models: [{
          id: "model-name",
          name: "Display Name",
          contextWindow: 200000,
          maxTokens: 8192,
          input: ["text"],
          reasoning: false,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        }],
      },
    },
  },
}
```

Notes that matter for routing through superai's proxy:

- `api` selects the wire format — `"openai-completions"` or `"anthropic-messages"`. Both supported, so an Anthropic-format local proxy is a legal provider.
- `apiKey` takes `${ENV_VAR}` interpolation, so keys never have to be written into the config file. This suits keychain-backed injection.
- Pointing at a local proxy on a private address needs `models.providers.<id>.request.allowPrivateNetwork: true`.
- For non-native OpenAI-compatible proxies OpenClaw sets `supportsDeveloperRole: false` automatically, and proxy routes skip native OpenAI features — `service_tier`, prompt caching, and reasoning compatibility shaping.
- `mode: "merge"` merges into the built-in catalog (60+ providers ship in-box, including Ollama, LM Studio, vLLM, and llama.cpp) rather than replacing it.

## 4. Multi-instance wrappers

`OPENCLAW_HOME` relocates everything at once, which is the clean path:

```sh
#!/bin/sh
# /usr/local/bin/openclaw-work
export OPENCLAW_HOME="$HOME/.openclaw-work"
exec openclaw "$@"
```

For sharing state but splitting config only, use the narrower knob:

```sh
export OPENCLAW_CONFIG_PATH="$HOME/.openclaw/profiles/local.json"
```

Caveat: OpenClaw is a **long-running daemon**, not a per-invocation CLI. Two instances mean two running services, so superai has to manage ports and gateway state per instance rather than just exporting a variable and exec'ing. See the gateway docs before assuming instances are free.

## 5. Gateway and multi-agent

`/gateway` (with `configuration`, `remote`, `security`, `tailscale`, `troubleshooting` sub-pages) covers the network surface; `/concepts/multi-agent` covers running several agents. Both are relevant to instance modelling and neither was read in depth for this pass — flagged as a gap.

## Key doc URLs

- https://docs.openclaw.ai/help/environment
- https://docs.openclaw.ai/concepts/model-providers
- https://docs.openclaw.ai/providers
- https://docs.openclaw.ai/gateway/configuration
- https://github.com/openclaw/openclaw

## Unverified

Skills/plugin layout, the full `openclaw.json` schema, and gateway config keys were not walked end to end. The provider and env-var sections above are quoted from official docs; treat the rest as incomplete rather than absent.
