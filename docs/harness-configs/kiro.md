# Kiro (AWS) — Configurable Options Reference

AWS's agentic IDE and CLI, and the **replacement for Amazon Q Developer** — not a rebrand of the IDE product, though the Amazon Q Developer CLI itself was rebranded into Kiro CLI. Relevant to superai twice over: as a harness in its own right, and because [amazon-q-cli.md](amazon-q-cli.md) now documents a sunsetting product.

Verified 2026-08-26 against kiro.dev docs and the AWS end-of-support announcement.

## 0. Migration status of Amazon Q Developer

| Date | What happens |
|---|---|
| 2026-05-15 | New Q Developer signups blocked — no new Free Tier accounts, no new subscriptions |
| 2026-05-29 | Opus 4.6 removed from Q Developer Pro; latest coding models are Kiro-exclusive |
| 2027-04-30 | End of support for Q Developer IDE plugins and paid subscriptions |

JetBrains, Eclipse, and Visual Studio plugins are gone with no replacement planned. The AWS Management Console version of Q Developer and the Slack/Teams integrations are **not** affected by the sunset.

## 1. Config file paths

Global scope, under `~/.kiro/`:

| Path | Purpose |
|---|---|
| `~/.kiro/settings/cli.json` | CLI settings |
| `~/.kiro/settings/mcp.json` | MCP servers |
| `~/.kiro/settings/permissions.yaml` | Permissions |
| `~/.kiro/agents/` | Custom agents |
| `~/.kiro/skills/` | Skills |
| `~/.kiro/steering/` | Steering docs |
| `~/.kiro/hooks/` | Hooks |
| `~/.kiro/powers/` | Powers |

Project scope, under `.kiro/`: `settings/mcp.json`, `agents/`, `steering/`, `skills/`, `hooks/`, `specs/`.

Note the mixed formats in one tree — JSON for settings and MCP, YAML for permissions, directories of files for agents/skills/steering. superai's config layer needs all three per harness, not one.

## 2. Environment variables

| Variable | Effect |
|---|---|
| `KIRO_HOME` | Redirects the global `~/.kiro` directory. Agents, prompts, skills, steering, settings, and sessions all resolve against it — "handy for keeping separate Kiro profiles on the same machine" |
| `KIRO_LOG_NO_COLOR` | `1` disables colored log output |
| `KIRO_CLI_TOOL_SEARCH_MATCHING_THRESHOLD` | Minimum relevance score for Tool Search results (default `1.5`) |
| `NO_COLOR` | Any value disables terminal UI color |

For AWS credential isolation, the standard AWS variables apply and are the documented approach:

```sh
export AWS_CONFIG_FILE=~/.aws/kiro/config
export AWS_SHARED_CREDENTIALS_FILE=~/.aws/kiro/credentials
```

## 3. Model settings

From `~/.kiro/settings/cli.json`:

| Key | Type | Meaning |
|---|---|---|
| `chat.defaultModel` | string | Default model for conversations |
| `chat.defaultAgent` | string | Default agent configuration |
| `chat.modelDefaults` | object | Per-model defaults (e.g. effort level) applied to new sessions |

## 4. Third-party / BYO endpoint — NOT SUPPORTED

**No documented custom provider, BYO API key, or OpenAI-compatible endpoint configuration.** The settings reference covers chat behavior, knowledge-base parameters, MCP timeouts, and feature toggles — there is no credential or provider-management surface. Models are AWS-served and selected by name.

For superai this means Kiro is an **install-and-configure target only, not a routing target**. The proxy layer cannot be pointed at it. If that changes, `chat.defaultModel` plus a provider block would be where to look.

## 5. Multi-instance wrappers

`KIRO_HOME` is the isolation knob, and it explicitly covers sessions and settings:

```sh
#!/bin/sh
# /usr/local/bin/kiro-work
export KIRO_HOME="$HOME/.kiro-work"
export AWS_CONFIG_FILE="$HOME/.aws/kiro-work/config"
export AWS_SHARED_CREDENTIALS_FILE="$HOME/.aws/kiro-work/credentials"
exec kiro "$@"
```

Because auth is AWS-side rather than a per-instance API key, separating instances means separating AWS profiles — pair `KIRO_HOME` with the AWS credential variables above, or with `AWS_PROFILE`.

## Key doc URLs

- https://kiro.dev/docs/configuration/
- https://kiro.dev/docs/reference/settings/
- https://kiro.dev/docs/cli/chat/configuration/
- https://kiro.dev/docs/mcp/configuration/
- https://kiro.dev/docs/custom-agents/configuration-reference/
- https://kiro.dev/blog/introducing-kiro-cli/
- https://aws.amazon.com/blogs/devops/amazon-q-developer-end-of-support-announcement/
- https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/upgrade-to-kiro.html

## Unverified

The custom-agents configuration reference, Crew configuration, and the full `cli.json` key list were not walked end to end. The "no BYO endpoint" finding is from the settings reference and is stated as documented-absence — worth re-checking, since it's the single fact that decides whether Kiro is proxy-routable.
