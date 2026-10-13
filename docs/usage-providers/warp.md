# Warp (usage source)

Verified 2026-10-13 against this repo's poller
(`crates/superai-usage/src/poller.rs`, `poll_warp` and `parse_warp_graphql`)
and the fixture
`crates/superai-usage/tests/fixtures/usage-providers/warp-graphql.json`. The
porting origin is CodexBar's `WarpUsageFetcher.swift`; the query text is
ported verbatim, and the fixture is the assumed shape, no live account was
polled.

## Where usage truth lives

One documented GraphQL endpoint, POST
`https://app.warp.dev/graphql/v2?op=GetRequestLimitInfo`. The request body is
the documented `GetRequestLimitInfo` query with a RequestContext variable
carrying the OS name. Warp's edge limiter rejects non-Warp user agents, so the
request sends `User-Agent: Warp/1.0` plus `x-warp-client-id`,
`x-warp-os-category`, and `x-warp-os-name` headers.

Shape (`parse_warp_graphql`):

- `data.user.user.requestLimitInfo`: `{isUnlimited, requestLimit,
  requestsUsedSinceLastRefresh, nextRefreshTime}`. The primary lane fills
  with `requestsUsedSinceLastRefresh / requestLimit * 100` and a
  `"N / M credits"` description; `isUnlimited` renders an unlimited lane at
  0 percent. `nextRefreshTime` (string or null) becomes the lane's reset
  instant.
- Bonus grants: `data.user.user.bonusGrants[]` plus
  `workspaces[].bonusGrantsInfo.grants[]`, each `{requestCreditsGranted,
  requestCreditsRemaining, expiration}`. All grants combine into one secondary
  "add-on credits" lane from summed granted/remaining counts. Per-grant
  expirations are not aggregated into a next-expiration lane (that ordering
  was not ported).
- Any non-empty GraphQL `errors` array fails the response.

Fixture example: `tests/fixtures/usage-providers/warp-graphql.json`.

## Auth material

`WARP_API_KEY` env var (registry row `warp`), sent as `Authorization:
Bearer`. Held in `PollSecret`, redacted in every rendering.

## Window strategy and reset semantics

Registry `window_strategy` is `endpoint`. The primary lane's reset semantics
are whatever `nextRefreshTime` reports; no window length is reported, so
`window_minutes` stays null. The secondary add-on lane carries no reset.

## Pricing

Registry `pricing` is `subscription`: request credits and grants, not a
currency amount; no cost or balance lane is synthesized.

## Upstream links

- Poller, query text, and parser: `crates/superai-usage/src/poller.rs`
  (`WARP_GRAPHQL_QUERY`).
- Fixture: `crates/superai-usage/tests/fixtures/usage-providers/warp-graphql.json`.
- Registry row: `crates/superai-usage/assets/usage-providers.json` (`warp`),
  verified 2026-10-13.
- Agent-side context: [../harness-configs/warp.md](../harness-configs/warp.md).
