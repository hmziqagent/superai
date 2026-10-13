# Kilo (usage source)

Verified 2026-10-13 against this repo's poller
(`crates/superai-usage/src/poller.rs`, `poll_kilo` and `parse_kilo_credits`)
and the fixture
`crates/superai-usage/tests/fixtures/usage-providers/kilo-credits.json`. The
porting origin is CodexBar's `KiloUsageFetcher.swift`; the fixture is the
assumed shape, no live account was polled.

## Where usage truth lives

One tRPC call, GET
`https://app.kilo.ai/api/trpc/user.getCreditBlocks?batch=1&input={"0":{"json":null}}`
(the input JSON percent-encoded), with a Bearer key. Shape
(`parse_kilo_credits`): a one-element batch array whose
`result.data.json.creditBlocks` holds credit blocks. Each block is
`{amount_mUsd, balance_mUsd}`, micro-USD numbers; blocks sum into one credits
window, and when both totals were seen and the granted sum is positive, the
primary lane fills with `(granted - remaining) / granted * 100` and an
`"N / M credits"` description. A response whose blocks carry no amounts at
all fails loudly.

Not ported: the kiloPass.getState pass lane, org scoping via
`X-KILOCODE-ORGANIZATIONID`, and the fallback-key-zoo credit parsing
CodexBar's fetcher carries. Only the `user.getCreditBlocks` credits window is
onboarded.

Fixture example: `tests/fixtures/usage-providers/kilo-credits.json`.

## Auth material

`KILO_API_KEY` env var, or the token file `~/.local/share/kilo/auth.json`
(both on the registry row `kilo`; the file is declared as registry data, the
poller takes whichever secret the caller loaded). Sent as `Authorization:
Bearer`, held in `PollSecret`, redacted in every rendering.

## Window strategy and reset semantics

Registry `window_strategy` is `endpoint`. The credits window carries no
length and no reset instant (the endpoint reports none), so `window_minutes`
and `resets_at` stay null; credit blocks do have expirations upstream, but
this window does not aggregate them.

## Pricing

Registry `pricing` is `subscription`: micro-USD credit accounting reported by
the provider; no per-token estimates are computed.

## Upstream links

- Poller and parser: `crates/superai-usage/src/poller.rs`.
- Fixture: `crates/superai-usage/tests/fixtures/usage-providers/kilo-credits.json`.
- Registry row: `crates/superai-usage/assets/usage-providers.json` (`kilo`),
  verified 2026-10-13.
- Harness-side context: [../harness-configs/kilo-code.md](../harness-configs/kilo-code.md),
  [../harness-configs/kilocode-cli.md](../harness-configs/kilocode-cli.md).
