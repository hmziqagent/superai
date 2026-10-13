# Pi and omp sessions (usage source)

Verified 2026-10-13 against this repo's reader
(`crates/superai-usage/src/reader/jsonl.rs`, `scan_pi_sessions`) and its
fixtures (`crates/superai-usage/tests/fixtures/pi/`). The porting origin is
CodexBar's `PiFamilySessionScanner` (record cap at
`PiSessionCostScanner.swift:106`); this repo's code is the authority. Harness
background: [../harness-configs/pi.md](../harness-configs/pi.md),
[../harness-configs/omp.md](../harness-configs/omp.md).

## Where usage truth lives

Session JSONL under the pi agent store and the omp (oh-my-pi) profile store.
Root resolution (`pi_session_roots` in
`crates/superai-usage/src/reader/jsonl.rs:128`), selectors read by the caller
and passed in:

- `PI_CODING_AGENT_SESSION_DIR` (an explicit sessions directory) wins
  outright; nothing else joins.
- Otherwise the agent root is `PI_CODING_AGENT_DIR/sessions` when set, else
  `~/.pi/agent/sessions`.
- The omp config root is `PI_CONFIG_DIR` when it names a path relative to
  HOME (absolute or escaping values are refused), default `~/.omp`. Its
  `agent/sessions` and `$XDG_DATA_HOME/omp/sessions` join only when
  `PI_CODING_AGENT_DIR` is unset: an explicit agent dir narrows the scan to
  that root plus the config-root profile discovery below.
- Profile selection: `OMP_PROFILE`, else `PI_PROFILE`, names one profile under
  `~/.omp/profiles/<name>` or `$XDG_DATA_HOME/omp/profiles/<name>` (each
  checked as `sessions/` and `agent/sessions/`, with or without an explicit
  agent dir). With no selector, every on-disk profile under the config root
  joins, sorted, capped at 64 per discovery pass, and profiles under
  `$XDG_DATA_HOME/omp/profiles` join too, but only when
  `PI_CODING_AGENT_DIR` is unset. Profiles past a cap are dropped and
  flagged, so totals over those roots are floors.

The family is `pi = [pi, omp]`; the scan dedups entries across the overlapping
roots by `(session id, entry id)` pair, so a session visible through both
stores counts once.

### Shape the reader parses

- `session` line: the session id (`id`, `sessionId`, or `session_id`).
- `model_change` line: sets provider and model context (`provider` plus
  `modelId` or `model`).
- `message` lines with `message.role == "assistant"`: usage records. Provider
  mapping accepts `openai-codex` (to `openai`), `anthropic`, and
  `amazon-bedrock`; a present-but-unmapped provider is skipped rather than
  priced under another backend's rates.

Usage keys come in several spellings (`input`/`inputTokens`/`input_tokens`/
`promptTokens` and friends for input, cache-read, cache-write, output); the
first readable spelling per lane wins, absent reads as 0, unreadable spoils
the row, and a row with no counter at all is skipped. Timestamps are epoch
seconds or milliseconds (split at the `1_000_000_000_000` heuristic) or RFC
3339 strings. A `cacheWrite1h`/`cttl.ephemeral_1h` value above the cache-write
counter spoils the row. On the openai backend, cache-write tokens fold into
input at the input rate (no openai cache-write tariff exists here); anthropic
and bedrock keep cache-write as its own lane. Records cap at 16 MiB.

Fixture example: `tests/fixtures/pi/home/.pi/agent/sessions/alpha.jsonl`.

## Auth material

None. Session files carry token counts, model ids, and entry ids, no
credentials. The registry row (`assets/usage-providers.json`, id `pi`) lists
no auth material.

## Window strategy and reset semantics

`provider-only` fill (no quota denominator in the artifacts); `resets_at`
computed only for the month window (first instant of next month, system zone),
null for session and week. Selector refusals (an invalid profile name, an
absolute `PI_CONFIG_DIR`) surface as notes with nothing read, not as failures;
the 64-profile cap surfaces as a coverage note.

## Pricing

`RateTable` estimates, USD per 1M tokens (bundled table merged with any
loadable models.dev cache). Provider ids arriving here are `anthropic`,
`openai`, and `amazon-bedrock`; unpriced pairs surface as `missing_pricing`.
Estimates, never a bill; skipped rows and the profile cap make totals floors.

## Upstream links

- Reader and caps: `crates/superai-usage/src/reader/jsonl.rs`,
  `crates/superai-usage/src/reader.rs` (16 MiB record cap).
- Fixtures: `crates/superai-usage/tests/fixtures/pi/`.
- Registry row: `crates/superai-usage/assets/usage-providers.json` (`pi`).
- Harness config surface: [../harness-configs/pi.md](../harness-configs/pi.md),
  [../harness-configs/omp.md](../harness-configs/omp.md).
