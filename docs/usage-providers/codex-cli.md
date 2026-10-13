# Codex CLI rollouts (usage source)

Verified 2026-10-13 against this repo's reader
(`crates/superai-usage/src/reader/jsonl.rs`, `scan_codex_rollouts`) and its
fixtures (`crates/superai-usage/tests/fixtures/codex/`). The porting origin is
CodexBar's `CostUsageScanner` (record cap at `CostUsageScanner.swift:4923`);
this repo's code is the authority. Harness background:
[../harness-configs/codex-cli.md](../harness-configs/codex-cli.md),
[../harness-configs/oh-my-codex.md](../harness-configs/oh-my-codex.md).

## Where usage truth lives

Rollout JSONL under the session roots (`codex_sessions_roots` in
`crates/superai-usage/src/reader/jsonl.rs:85`): `CODEX_HOME/sessions` and the
sibling `CODEX_HOME/archived_sessions`, defaulting to `~/.codex` when
`CODEX_HOME` is unset or blank. Every `*.jsonl` file under those roots is
scanned; hidden entries, symlinks, and non-regular files are skipped. The
family is `codex = [codex-cli, oh-my-codex]` because oh-my-codex inherits
`CODEX_HOME`, so the store is scanned once for both harness ids.

### Shape the reader parses

Three line kinds matter:

- `turn_context`: sets the file-scoped model from `payload.model`,
  `payload.model_name`, `payload.info.model`, or `payload.info.model_name`,
  in that order.
- `event_msg` with `payload.type == "token_count"`: the usage record. Needs
  top-level `timestamp` (RFC 3339) and `payload.info` with token counters.
  Model resolution tries `info.model`, `info.model_name`, the payload, the
  line, then the file-scoped `turn_context` model.
- `token_usage_record` lines are skipped whole (legacy shape).

Token counters under `info.total_token_usage` (cumulative) or, when absent,
`info.last_token_usage`: `input_tokens`, `output_tokens`, and cached reads
under either spelling `cached_input_tokens` or `cache_read_input_tokens` (the
larger valid value wins; any present-but-unreadable spelling spoils the
payload). Usage is the per-file delta of the cumulative total against a
watermark, so repeated records count once and a regressing total is treated as
a stale replay that re-emits nothing. Codex records cached reads as a subset
of `input_tokens`; the event splits them, so the input rate covers only the
uncached remainder and cache reads price at the cache-read rate. Events are
tagged provider `openai`. Records cap at 256 KiB.

Fixture example: `tests/fixtures/codex/sessions/2026/10/13/rollout-main.jsonl`.

## Auth material

None. Rollouts carry token counts and model ids, no credentials. The registry
row (`assets/usage-providers.json`, id `codex`) also records the provider's
own endpoints (`https://chatgpt.com/backend-api/wham/usage` and
`.../rate-limit-reset-credits`) and token file `~/.codex/auth.json`; those are
registry data only, nothing in this repo polls them.

## Window strategy and reset semantics

Same as the other local readers: `provider-only` fill, because rollouts carry
no quota denominator. `resets_at` is computed only for the month window (first
instant of the next month, system zone); session and week report null.
DST-deleted next-month midnights fail the month window with an error.

## Pricing

`RateTable` estimates, USD per 1M tokens: bundled first-party list prices
(fetched 2026-10-13 from anomalyco/models.dev, provider `openai`) merged with
any loadable models.dev cache under
`$XDG_CACHE_HOME/superai/model-pricing/models-dev.json`. The openai rows price
input, output, and cache-read; no cache-write rate exists, matching the
artifact shape. Estimates, never a bill; skipped lines and unreadable
artifacts make totals floors.

## Upstream links

- Reader and caps: `crates/superai-usage/src/reader/jsonl.rs`,
  `crates/superai-usage/src/reader.rs` (256 KiB record cap).
- Fixtures: `crates/superai-usage/tests/fixtures/codex/`.
- Registry row: `crates/superai-usage/assets/usage-providers.json` (`codex`).
- Harness config surface: [../harness-configs/codex-cli.md](../harness-configs/codex-cli.md).
