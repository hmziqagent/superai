# OpenRouter (usage source)

Verified 2026-10-13 against this repo's poller
(`crates/superai-usage/src/poller.rs`, `poll_openrouter` and
`parse_openrouter_responses`) and fixtures
(`crates/superai-usage/tests/fixtures/usage-providers/openrouter-key.json`,
`openrouter-credits.json`). The porting origin is CodexBar's `openrouter.js`
fetcher; the fixtures are the assumed shape, no live account was polled.

## Where usage truth lives

Two documented endpoints under `https://openrouter.ai/api/v1`, both GET with
a Bearer key:

- `/key` (required): per-key spend and limit counters.
- `/credits` (optional second request): account credit totals. An HTTP-level
  failure (auth, rate limit, 5xx, unexpected status) degrades the snapshot to
  key data only; a parse failure of a successful credits body fails loudly.

Shapes (`parse_openrouter_responses`):

- `/key` body: `{data: {usage, usage_daily, usage_weekly, usage_monthly,
  limit, limit_remaining, limit_reset}}`, all USD numbers. With a capped key
  (`limit > 0`) the primary lane fills from `limit - clamp(limit_remaining,
  0, limit)`, or from the window counter matching `limit_reset`
  (`usage_daily`/`usage_weekly`/`usage_monthly`) when `limit_remaining` is
  absent; an unrecognized `limit_reset` falls back to total `usage`, and
  failing that to 0. With no cap, the cost lane fills from `usage_monthly`
  (period "api key this month") or `usage` ("api key total").
- `/credits` body: `{data: {total_credits, total_usage}}`; balance lane
  available = `total_credits - total_usage`, floored at 0, USD.

Fixture example: `tests/fixtures/usage-providers/openrouter-key.json`.

## Auth material

`OPENROUTER_API_KEY` env var (registry row `openrouter`), sent as
`Authorization: Bearer`. The secret lives in `PollSecret`, redacted in every
rendering; errors never carry it.

## Window strategy and reset semantics

Registry row says `endpoint`. The endpoint reports no window lengths and no
reset instants, so the primary lane carries a percent only
(`window_minutes` and `resets_at` null). Spend counters are period-labeled by
the key's own `limit_reset` field, not by a computed calendar window.

## Pricing

Provider-reported USD amounts (cost and balance lanes); no per-token rate
estimates are involved. Registry `pricing` is `subscription`: spend recorded
here is the provider's own accounting of credits and limits, not a bill
superai computes.

## Upstream links

- Poller and parser: `crates/superai-usage/src/poller.rs`.
- Fixtures: `crates/superai-usage/tests/fixtures/usage-providers/`.
- Registry row: `crates/superai-usage/assets/usage-providers.json`
  (`openrouter`), verified 2026-10-13.
