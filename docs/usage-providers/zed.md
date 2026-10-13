# Zed cloud (usage source)

Verified 2026-10-13 against this repo's poller
(`crates/superai-usage/src/poller.rs`, `poll_zed` and `parse_zed_billing`)
and the fixture
`crates/superai-usage/tests/fixtures/usage-providers/zed-billing.json`. The
porting origin is CodexBar's `zed.js` fetcher; the fixture is the assumed
shape, no live account was polled.

## Where usage truth lives

One documented endpoint, GET `https://cloud.zed.dev/frontend/billing/usage`.
Shape (`parse_zed_billing`):

- `plan` (required non-empty string).
- `current_usage.edit_predictions`: `{used: integer >= 0, limit}` where
  `limit` is a positive number, null, or the string `"unlimited"`. A finite
  limit fills the primary lane with `used / limit * 100` and a
  `"N / M predictions"` description; null or `"unlimited"` renders an
  unlimited lane at 0 percent.
- `current_usage.token_spend`: `{spend_in_cents, limit_in_cents}`, both
  cent-denominated numbers; when both are present the cost lane reports USD
  (cents / 100) against the cap, period "current billing period".

The registry row (`zed`) also lists `https://cloud.zed.dev/client/users/me`
(editor identity, Keychain-backed); that endpoint is not ported. `poll_zed`
covers the billing usage endpoint only.

Fixture example: `tests/fixtures/usage-providers/zed-billing.json`.

## Auth material

The full `Cookie` request-header value (`zed.session=...`), exactly what
`zed.js` sends, supplied by the caller to `poll_zed` and passed verbatim to
the wire as a `Cookie` header. Registry kind is `browser_session`; superai
itself never reads browser storage, Keychain, or cookie jars, so the value
must come from the operator. Held in `PollSecret`, redacted in every
rendering.

## Window strategy and reset semantics

Registry `window_strategy` is `provider_only`: the endpoint reports current
period counters, not quota windows with lengths or resets, so secondary and
tertiary lanes stay `Unavailable` and no reset instant is invented.

## Pricing

Cost lane is provider-reported USD from the cent fields. Registry `pricing`
is `subscription`: the amounts are plan accounting, not a rate card superai
computes.

## Upstream links

- Poller and parser: `crates/superai-usage/src/poller.rs`.
- Fixture: `crates/superai-usage/tests/fixtures/usage-providers/zed-billing.json`.
- Registry row: `crates/superai-usage/assets/usage-providers.json` (`zed`),
  verified 2026-10-13.
- Editor-side context: [../harness-configs/zed-acp.md](../harness-configs/zed-acp.md).
