# Factory (usage source)

Verified 2026-10-13 against this repo's poller
(`crates/superai-usage/src/poller.rs`, `poll_factory` and
`parse_factory_limits`) and the fixture
`crates/superai-usage/tests/fixtures/usage-providers/factory-limits.json`.
The porting origin is CodexBar's `FactoryStatusProbe.swift`; the fixture is
the assumed shape, no live account was polled.

## Where usage truth lives

One documented endpoint, GET `https://api.factory.ai/api/billing/limits` with
a Bearer key. Shape (`parse_factory_limits`):

- `extraUsageBalanceCents` (number): balance lane, USD (cents / 100).
- `limits.standard`: `fiveHour`, `weekly`, `monthly` windows filling the three
  standard lanes (5 h, 7 day, monthly). Each window is `{usedPercent,
  windowEnd, secondsRemaining}`; `usedPercent` is required when the window is
  present, `windowEnd` may be an RFC 3339 string or an epoch number (seconds
  versus milliseconds split at `> 1e12`), and `secondsRemaining > 0` renders a
  "resets in Ns" description.
- `limits.core`: the same three windows, emitted as named extras
  (`factory-core-5h`, `factory-core-7-day`, `factory-core-monthly`) only when
  the pool holds any data.
- A missing `limits` object leaves every window `Unavailable` and only the
  balance survives.

The registry row (`factory`) also lists a browser-session path for the
console; only the API-key endpoint is ported. Factory's legacy
`/api/organization/subscription/usage` (Standard/Premium usage) is not
ported.

Fixture example: `tests/fixtures/usage-providers/factory-limits.json`.

## Auth material

`FACTORY_API_KEY` env var or the key in `~/.factory/.env` (registry kinds
`env_var` and `env_file`), sent as `Authorization: Bearer`. Whichever the
caller loads, it reaches the poller as a `PollSecret`, redacted in every
rendering.

## Window strategy and reset semantics

Registry `window_strategy` is `endpoint`: three provider-reported quota
windows plus named core-pool extras. Reset semantics come from each window's
own `windowEnd`; the monthly window reports no length, so its
`window_minutes` is null.

## Pricing

Registry `pricing` is `subscription`: `usedPercent` values are plan quota
percentages, and the only currency amount is the extra-usage balance. No
per-token estimates are computed.

## Upstream links

- Poller and parser: `crates/superai-usage/src/poller.rs`.
- Fixture: `crates/superai-usage/tests/fixtures/usage-providers/factory-limits.json`.
- Registry row: `crates/superai-usage/assets/usage-providers.json`
  (`factory`), verified 2026-10-13.
- Harness-side context: [../harness-configs/factory-droid.md](../harness-configs/factory-droid.md).
