# Usage sources index

Research catalog of every usage source superai has onboarded, one file per
source, mirroring [docs/harness-configs/](../harness-configs/). Each file
records where the usage truth lives (paths or endpoints, with the exact shapes
this repo's code and fixtures parse), what auth material reading needs (kinds
and locations, never values), the window strategy and reset semantics, the
pricing source and units, and a verification date. superai's own code and
fixtures are the authority; the CodexBar snapshot's per-provider docs are a
starting index, not the source of record. All files verified 2026-10-13.

## Local artifact readers

| Source | File | Family (harnesses sharing the store) |
|---|---|---|
| Claude Code transcripts | [claude-code.md](claude-code.md) | `claude-code` |
| Codex CLI rollouts | [codex-cli.md](codex-cli.md) | `codex-cli`, `oh-my-codex` |
| OpenCode history db | [opencode.md](opencode.md) | `opencode`, `oh-my-opencode` |
| Pi / omp sessions | [pi-omp.md](pi-omp.md) | `pi`, `omp` |

## Proxy ledger

| Source | File |
|---|---|
| llmproxy usage ledger | [llmproxy.md](llmproxy.md) |

## API-key pollers

| Source | File |
|---|---|
| OpenRouter | [openrouter.md](openrouter.md) |
| DeepSeek | [deepseek.md](deepseek.md) |
| Zed cloud | [zed.md](zed.md) |
| Warp | [warp.md](warp.md) |
| Factory | [factory.md](factory.md) |
| Kilo | [kilo.md](kilo.md) |
| MiniMax | [minimax.md](minimax.md) |
| Mistral | [mistral.md](mistral.md) |
| ClinePass | [clinepass.md](clinepass.md) |

## Rules every source here follows

Every poller runs explicit, bounded, redacted requests per invocation and
returns; nothing schedules or retries itself (the PRV-07 probe discipline
from subplan 07). OpenRouter is the one two-request poller (`/key` plus
`/credits`). Shared transport rules: https everywhere except plain http on an
exact loopback host, whole-request timeout within 1..=30 s, response body
capped at 1 MiB by Content-Length precheck and capped read, control
characters rejected in headers before any send. Pollers refuse redirects
outright. The llmproxy client is the one exception: it follows redirects
manually, re-validating every hop and requiring the ledger's own origin, and
the admin bearer token is re-sent to each validated hop. Errors carry static
reasons and redacted URLs, never credential bytes or response bodies.

Local readers never write, open only regular files (symlinks, FIFOs, and
device nodes are skipped), and cap each JSONL record (claude 512 KiB, codex
256 KiB, pi 16 MiB) so a hostile file cannot exhaust memory. Skipped lines and
unreadable artifacts are counted, so every period total is a floor, never a
silent zero.

Registry rows for every source live in
`crates/superai-usage/assets/usage-providers.json` (schema version 1, verified
2026-10-13); the registry is data only and carries no secret values.
