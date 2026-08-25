# Zed AI Configuration & External Agent Hosting via Agent Client Protocol (ACP)

> Compiled 2026-08-25. Primary sources are cited inline next to each claim.
> Note on doc drift: Zed restructured its AI docs in 2025–2026 — provider/model config now lives
> under `language_models.*`, agent behavior under `agent.*`, and External Agents install via the
> ACP Registry instead of editor extensions. Both current and legacy forms are noted below.

---

## 1. `settings.json` AI Schema

Zed's AI config lives in `~/.config/zed/settings.json` (macOS/Linux; `%APPDATA%\Zed\settings.json`
on Windows). Open it with `agent: open settings` from the command palette
([Use API Access](https://zed.dev/docs/ai/use-api-access.html)).

### 1.1 `agent` block — default model, available models, profiles

The **current documented** shape scopes the default model *per profile* under
`agent.profiles` ([Agent Profiles](https://zed.dev/docs/ai/agent-profiles.html)):

```json
{
  "agent": {
    "profiles": {
      "ask": {
        "name": "Ask",
        "tools": {
          "read_file": true,
          "grep": true,
          "terminal": false,
          "edit_file": false
        },
        "enable_all_context_servers": false,
        "context_servers": {},
        "default_model": {
          "provider": "zed.dev",
          "model": "claude-sonnet-4-5"
        }
      }
    }
  }
}
```

"The exact model IDs and provider IDs depend on your configured LLM Providers"
([Agent Profiles](https://zed.dev/docs/ai/agent-profiles.html)).

**Legacy top-level form** (older Zed releases, pre-docs-restructure): a flat
`agent.default_model` plus an `agent.available_models` array of `{provider, model}` pairs — e.g.
`"available_models": [{"provider": "openrouter", "model": "anthropic/claude-sonnet-4"}]`.
Current docs no longer show this form; today custom models are declared under
`language_models.<provider>.available_models` instead (see §2), so treat the flat array as
historical. ⚠️ *Legacy shape from prior Zed docs versions, not present in the pages fetched for
this document — verify against your installed Zed version.*

Built-in profiles are `Write` (read/edit/command tools), `Ask` (read-only codebase questions),
and `Minimal` (no project tools). Custom profiles can also be created/forked via the Agent Panel
profile selector → `Configure`, or `agent: manage profiles`
([Agent Profiles](https://zed.dev/docs/ai/agent-profiles.html)). Profiles only decide tool
*availability*; allow/deny/confirm gating is separate **Tool Permissions**
([Agent Profiles](https://zed.dev/docs/ai/agent-profiles.html)).

### 1.2 `context_servers` (MCP)

MCP servers are configured under `context_servers` in `settings.json` and extend the Zed Agent
with external tools/data sources
([Model Context Protocol](https://zed.dev/docs/ai/mcp.html), referenced from the docs index
[llms.txt](https://zed.dev/docs/llms.txt)). Profiles reference them two ways
([Agent Profiles](https://zed.dev/docs/ai/agent-profiles.html)):

```json
{
  "agent": {
    "profiles": {
      "write": {
        "enable_all_context_servers": true,
        "context_servers": { "my-mcp-server": { "tools": { "*": true } } }
      }
    }
  }
}
```

Zed-configured MCP servers "may be forwarded to External Agents over ACP"; if an MCP tool doesn't
show up in an External Agent, check both Zed's MCP config *and* the agent's native MCP config
([External Agents](https://zed.dev/docs/ai/external-agents.html)).

---

## 2. Provider Setup Blocks

All first-class providers go under `language_models` in `settings.json`. Keys entered through the
Settings UI are stored in the **system keychain, not settings.json**; environment variables take
precedence over keychain values
([Use API Access](https://zed.dev/docs/ai/use-api-access.html)). Env vars per provider:
`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` (falls back to `GOOGLE_AI_API_KEY`),
`MISTRAL_API_KEY`, `DEEPSEEK_API_KEY`, `XAI_API_KEY`, `OPENCODE_API_KEY`,
`OPENROUTER_API_KEY`, `VERCEL_AI_GATEWAY_API_KEY`, `OLLAMA_API_KEY`, `LMSTUDIO_API_KEY`
([Use API Access](https://zed.dev/docs/ai/use-api-access.html)).

Custom headers attach via `language_models.<provider>.custom_headers` (supported by Bedrock,
Anthropic, DeepSeek, Google AI, LM Studio, Mistral, Ollama, OpenAI, OpenAI-compatible,
OpenCode, OpenRouter, Vercel AI Gateway, xAI); Zed-managed auth headers cannot be overridden
([Use API Access](https://zed.dev/docs/ai/use-api-access.html)).

### Anthropic
Key via UI or `ANTHROPIC_API_KEY`. Custom models support `max_output_tokens`, `tool_override`,
and thinking modes ([Use API Access §Anthropic](https://zed.dev/docs/ai/use-api-access.html#anthropic)):

```json
{
  "language_models": {
    "anthropic": {
      "available_models": [
        {
          "name": "claude-sonnet-4-latest",
          "display_name": "claude-sonnet-4-thinking",
          "max_tokens": 200000,
          "mode": { "type": "thinking", "budget_tokens": 4096 }
        },
        {
          "name": "claude-3-5-sonnet-20240620",
          "display_name": "Sonnet 2024-June",
          "max_tokens": 128000,
          "max_output_tokens": 2560,
          "tool_override": "some-model-that-supports-toolcalling"
        }
      ]
    }
  }
}
```

### OpenAI
Key via UI or `OPENAI_API_KEY`. "You must provide the model's context window in `max_tokens`.
For reasoning-focused models, set `max_completion_tokens` to avoid high reasoning-token costs"
([Use API Access §OpenAI](https://zed.dev/docs/ai/use-api-access.html#openai)):

```json
{
  "language_models": {
    "openai": {
      "available_models": [
        {
          "name": "gpt-5.2",
          "display_name": "gpt-5.2 high",
          "reasoning_effort": "high",
          "max_tokens": 272000,
          "max_completion_tokens": 20000
        }
      ]
    }
  }
}
```

### Google AI
Key via UI or `GEMINI_API_KEY`/`GOOGLE_AI_API_KEY`
([Use API Access §Google AI](https://zed.dev/docs/ai/use-api-access.html#google-ai)):

```json
{
  "language_models": {
    "google": {
      "available_models": [
        {
          "name": "gemini-3.1-pro-preview",
          "display_name": "Gemini 3.1 Pro",
          "max_tokens": 1000000,
          "mode": { "type": "thinking", "budget_tokens": 24000 }
        }
      ]
    }
  }
}
```

### Ollama (local)
Local providers are configured under [Use a Local Model](https://zed.dev/docs/ai/use-a-local-model.html)
(llama.cpp, Ollama, LM Studio, local OpenAI-compatible servers). Key: `OLLAMA_API_KEY` if the
server requires one ([Use API Access](https://zed.dev/docs/ai/use-api-access.html#api-keys)).
Canonical shape (⚠️ per that page's pattern; not fetched this run — confirm model IDs against
your Ollama install):

```json
{
  "language_models": {
    "ollama": {
      "api_url": "http://localhost:11434",
      "available_models": [
        {
          "name": "qwen2.5-coder:32b",
          "display_name": "Qwen 2.5 Coder 32B",
          "max_tokens": 32000,
          "supports_tools": true
        }
      ]
    }
  }
}
```

### LM Studio (local)
Same pattern; default server URL `http://localhost:1234/api/v0`, optional `LMSTUDIO_API_KEY`
([Use a Local Model](https://zed.dev/docs/ai/use-a-local-model.html);
[Use API Access](https://zed.dev/docs/ai/use-api-access.html#api-keys) for the env var):

```json
{
  "language_models": {
    "lmstudio": {
      "api_url": "http://localhost:1234/api/v0",
      "available_models": [
        { "name": "qwen2.5-coder-32b-instruct", "display_name": "Qwen 2.5 Coder 32B", "max_tokens": 96000 }
      ]
    }
  }
}
```

### OpenRouter (gateway)
Gateways (OpenRouter, Bedrock, Vercel AI Gateway) are their own path:
[Use a Gateway](https://zed.dev/docs/ai/use-a-gateway.html). Key: `OPENROUTER_API_KEY`
([Use API Access](https://zed.dev/docs/ai/use-api-access.html#api-keys)). Canonical shape
(⚠️ gateway-page pattern; not fetched this run):

```json
{
  "language_models": {
    "openrouter": {
      "api_url": "https://openrouter.ai/api/v1",
      "available_models": [
        { "name": "anthropic/claude-sonnet-4", "display_name": "Claude Sonnet 4 (OpenRouter)", "max_tokens": 200000 }
      ]
    }
  }
}
```

### Custom OpenAI-compatible endpoint
Add via Agent Settings → LLM Providers → `Add Provider`, or in settings
([Use API Access §OpenAI-compatible](https://zed.dev/docs/ai/use-api-access.html#openai-compatible)):

```json
{
  "language_models": {
    "openai_compatible": {
      "my-provider": {
        "api_url": "https://example.com/v1",
        "available_models": [
          { "name": "my-model", "display_name": "My Model", "max_tokens": 128000 }
        ]
      }
    }
  }
}
```

Default inherited capabilities: `tools: true`, `images: false`, `parallel_tool_calls: false`,
`prompt_cache_key: false`, `chat_completions: true`, `interleaved_reasoning: false`,
`max_tokens_parameter: false`. Responses-API-only models set `capabilities.chat_completions:
false`; reasoning effort accepts `"none" | "minimal" | "low" | "medium" | "high" | "xhigh" |
"max"` (ibid.). There is a symmetric Anthropic-compatible path under
`language_models.anthropic_compatible` with `prompt_caching` capability control
([§Anthropic-compatible](https://zed.dev/docs/ai/use-api-access.html#anthropic-compatible)).
OpenAI-compatible env vars are auto-generated as upper-snake-case `<PROVIDER_ID>_API_KEY`
(e.g., provider id `my-gateway` → `MY_GATEWAY_API_KEY`)
([Use API Access](https://zed.dev/docs/ai/use-api-access.html#api-keys)). Docs repeatedly warn:
**do not put API keys in `settings.json`** — use the UI/keychain or env vars.

---

## 3. Agent Client Protocol (ACP): Hosting External Agents

ACP lets an editor (**Client**) host generative-AI coding **Agents** as subprocesses. Zed hosts
the thread in its Agent Panel while the External Agent owns its own runtime, auth, model
selection, tools, and native configuration; billing/legal/data-handling are between you and the
agent provider, and Zed does not charge for External Agents
([External Agents](https://zed.dev/docs/ai/external-agents.html)).

### Protocol basics ([ACP Overview](https://agentclientprotocol.com/protocol/overview))
- Transport: **JSON-RPC 2.0 over stdio** — the agent "typically runs as a subprocess of the
  Client." Two message types: request/response **Methods**, one-way **Notifications**.
- Flow: `initialize` (version/capability negotiation) → `authenticate` (if required) →
  `session/new` (or `session/load` with `loadSession` capability) → prompt turn: client sends
  `session/prompt`, agent streams `session/update` notifications (message chunks, tool calls,
  plans, slash commands, mode changes), client answers `session/request_permission` for gated
  tool calls, `session/cancel` interrupts; the turn ends with the `session/prompt` response +
  stop reason.
- Optional client capabilities: `fs/read_text_file`, `fs/write_text_file`, terminal methods
  (`terminal/create|output|release|wait_for_exit|kill`), elicitation. Optional agent extras:
  `logout`, `session/set_mode`.
- Conventions: all file paths absolute, line numbers 1-based, camelCase keys / snake_case
  discriminator strings; extensibility via `_meta`, `_`-prefixed custom methods, advertised
  capabilities.

### Configuring an External Agent in `settings.json`
Install from the **ACP Registry** (`zed: acp registry`, or Agent Settings → External Agents →
`Add Agent` → Install from Registry). Registry-installed agents can get per-agent settings under
`agent_servers.<agent-id>`
([External Agents](https://zed.dev/docs/ai/external-agents.html)). For anything else, add a
custom agent — the `agent_servers` entry takes `type`, `command`, `args`, `env`
([External Agents §Custom Agents](https://zed.dev/docs/ai/external-agents.html#custom-agents)):

```json
{
  "agent_servers": {
    "my-agent": {
      "type": "custom",
      "command": "node",
      "args": ["~/projects/agent/index.js", "--acp"],
      "env": {}
    }
  }
}
```

Real-world registry example (Poolside manual config, same block shape):

```json
{
  "agent_servers": {
    "Poolside": {
      "command": "pool",
      "args": ["acp"],
      "type": "custom"
    }
  }
}
```

Zed detects settings changes automatically — no restart needed
([External Agents](https://zed.dev/docs/ai/external-agents.html)). Debug wire traffic with
`dev: open acp logs` (ibid.).

### Which agents speak ACP
Curated list in Zed's docs: **Claude (Claude Code), Codex, OpenCode, Copilot, Cursor, Pi Coding
Agent**, plus **Gemini CLI** and **Poolside** ("curated, not exhaustive — open the ACP Registry
for the current list") ([External Agents](https://zed.dev/docs/ai/external-agents.html)). On the
ACP side, [agentclientprotocol.com](https://agentclientprotocol.com/) maintains an agents
directory that additionally includes community adapters such as **Kimi CLI** (Kimi for Coding;
run with its ACP mode, e.g. `kim --acp`) and others. ⚠️ Kimi detail is from general knowledge of
the ACP ecosystem, not from the pages fetched here — confirm the exact launch command on the
agent's own page before wiring it up.

Per-agent ownership notes ([External Agents](https://zed.dev/docs/ai/external-agents.html)):
- **Claude**: owns its auth/billing (`/login` inside the thread); a Zed-Agent Anthropic key does
  *not* configure it; reads `CLAUDE.md` natively.
- **Codex**: owns auth/billing (ChatGPT login, Codex/OpenAI keys, native config).
- **Gemini CLI**: if `GEMINI_API_KEY`/`GOOGLE_AI_API_KEY` is in its environment it uses that;
  otherwise Zed forwards its configured Google AI key to the agent as `GEMINI_API_KEY`.
- **Pi**: an agent harness — configure all provider auth/models inside Pi.
- Extension-provided agents are **deprecated**; migrated to registry equivalents
  ([External Agents](https://zed.dev/docs/ai/external-agents.html),
  [Agent Server Extensions](https://zed.dev/docs/extensions/agent-servers.html)).

Capability boundary in External Agent threads: model/provider config and auth usually owned by
the agent; Zed Agent profiles and Zed Skills don't apply; Zed MCP servers may be forwarded over
ACP; native instruction files read per-agent
([External Agents](https://zed.dev/docs/ai/external-agents.html)).

---

## 4. Multi-Instance Wrappers (same ACP agent, multiple provider configs)

Because `agent_servers.*` takes any executable plus an `env` map, you can register the *same*
ACP agent binary several times, each entry pointing at a small wrapper script that pins
different credentials/endpoints. This is the standard way to run, say, Claude Code against two
different gateways side-by-side. Zed spawns the wrapper as a subprocess speaking ACP over stdio
([External Agents](https://zed.dev/docs/ai/external-agents.html);
[ACP Overview](https://agentclientprotocol.com/protocol/overview)).

**Wrapper script** — `~/.local/bin/claude-acp-work.sh`:

```bash
#!/usr/bin/env bash
# Route Claude Code's ACP adapter through the work gateway.
export ANTHROPIC_BASE_URL="https://gateway.corp.example.com"
export ANTHROPIC_AUTH_TOKEN="$HOME/.secrets/work-anthropic-token" && \
  export ANTHROPIC_AUTH_TOKEN="$(cat "$ANTHROPIC_AUTH_TOKEN")"
exec npx -y @zed-industries/claude-code-acp "$@"
```

**Second wrapper** — `~/.local/bin/kimi-acp-personal.sh` (personal account, different vendor):

```bash
#!/usr/bin/env bash
export KIMI_API_KEY="$(cat ~/.secrets/moonshot-key)"
export NO_PROXY="*"   # bypass corporate proxy for this instance
exec kim --acp "$@"
```

**Registration** — both appear as independent agents in Zed's new-thread menu:

```json
{
  "agent_servers": {
    "Claude (work gateway)": {
      "type": "custom",
      "command": "/home/me/.local/bin/claude-acp-work.sh",
      "args": [],
      "env": {}
    },
    "Kimi (personal)": {
      "type": "custom",
      "command": "/home/me/.local/bin/kimi-acp-personal.sh",
      "args": [],
      "env": {}
    },
    "Gemini (staging key)": {
      "type": "custom",
      "command": "gemini",
      "args": ["--experimental-acp"],
      "env": {
        "GEMINI_API_KEY": "staging-key-here"
      }
    }
  }
}
```

Notes:
- You can pass secrets either inside the wrapper script (recommended: read from a file/secret
  store) or directly in the `env` object — the latter lands in plaintext `settings.json`, which
  Zed's own guidance discourages for provider keys
  ([Use API Access](https://zed.dev/docs/ai/use-api-access.html)); the same hygiene applies here.
- Each entry gets its own process and environment, so per-entry env never leaks between threads.
- The wrapper must `exec` the real ACP server so stdio JSON-RPC passes through cleanly.
- Verify handoff with `dev: open acp logs`
  ([External Agents](https://zed.dev/docs/ai/external-agents.html)).
- Wrapper-script composition and the Kimi/Gemini flags above are community-standard patterns,
  not verbatim from the fetched pages; the underlying mechanism (`command`/`args`/`env`,
  subprocess + stdio) is documented.

---

## 5. Rules Files (`.rules`, `AGENTS.md`)

Zed's rules system was replaced by **Skills + Instructions**
([Instructions](https://zed.dev/docs/ai/instructions.html)): reusable on-demand rules became
Skills; always-on rules became personal `AGENTS.md`; project `.rules` files remain supported as
compatibility project instruction files (ibid.).

**Personal instructions** apply to every project opened with the Zed Agent:
`~/.config/zed/AGENTS.md` (Windows: `%APPDATA%\Zed\AGENTS.md`)
([Instructions](https://zed.dev/docs/ai/instructions.html)).

**Project instructions** — Zed uses the first match in this precedence order
([Instructions](https://zed.dev/docs/ai/instructions.html)):

1. `.rules`
2. `.cursorrules`
3. `.windsurfrules`
4. `.clinerules`
5. `.github/copilot-instructions.md`
6. `AGENT.md`
7. [`AGENTS.md`](https://agents.md/)
8. `CLAUDE.md`
9. `GEMINI.md`

Project instructions override personal `AGENTS.md` on conflict (ibid.).

Cross-agent loading matrix ([Instructions](https://zed.dev/docs/ai/instructions.html)):

| File | Zed Agent | External Agents | Terminal Threads |
| --- | --- | --- | --- |
| `~/.config/zed/AGENTS.md` | Personal instructions | Not generally used | Not used unless the CLI reads it |
| Project `AGENTS.md` | Project instructions | Depends on the agent | Depends on the CLI |
| `CLAUDE.md` | Loaded as compatible instructions by Zed Agent | Claude reads natively | Claude Code CLI reads natively |
| `.github/copilot-instructions.md` | Loaded as compatible instructions | Depends on the agent | Depends on the CLI |

Practical upshot: for External Agents, the *agent's own* loader decides what it reads — Claude
Code picks up `CLAUDE.md`, Gemini reads `GEMINI.md`, ACP-generic agents typically honor
`AGENTS.md` — so keep a root `AGENTS.md` as the portable baseline and layer vendor files on top
([Instructions](https://zed.dev/docs/ai/instructions.html);
[External Agents](https://zed.dev/docs/ai/external-agents.html)).

---

## Source index
- https://zed.dev/docs/ai/external-agents — External Agents, ACP Registry, `agent_servers`, per-agent notes (fetched 2026-08-25)
- https://agentclientprotocol.com/protocol/overview — ACP protocol overview, JSON-RPC methods/notifications (fetched 2026-08-25)
- https://zed.dev/docs/ai/llm-providers — model-access path overview (fetched 2026-08-25)
- https://zed.dev/docs/ai/use-api-access — provider blocks, env vars, compatible endpoints (fetched 2026-08-25)
- https://zed.dev/docs/ai/agent-profiles — `agent.profiles`, built-ins, `default_model` (fetched 2026-08-25)
- https://zed.dev/docs/ai/instructions — `.rules` precedence, `AGENTS.md` matrix (fetched 2026-08-25)
- https://zed.dev/docs/ai/zed-agent — Zed Agent capability map (fetched 2026-08-25)
- https://zed.dev/docs/llms.txt — docs index used to resolve current URLs (fetched 2026-08-25)
- Referenced but not fetched this run: use-a-local-model, use-a-gateway, mcp, extensions/agent-servers (⚠️ items marked above rely on them)
