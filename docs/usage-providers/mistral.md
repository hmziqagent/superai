# Mistral (usage source)

Verified 2026-10-13 against this repo's poller
(`crates/superai-usage/src/poller.rs`, `poll_mistral` and
`parse_mistral_credits`) and the fixture
`crates/superai-usage/tests/fixtures/usage-providers/mistral-credits.json`.
The porting origin is CodexBar's `MistralUsageFetcher.swift`; the fixture is
the assumed shape, no live account was polled.

## Where usage truth lives

One documented endpoint, GET `https://admin.mistral.ai/api/billing/credits`,
sent with the admin console session as a `Cookie` header plus `Referer` and
`Origin` headers bound to `https://admin.mistral.ai`. Shape
(`parse_mistral_credits`): `{wallet_amount, credit_notes_amount,
ongoing_usage_balance, currency}`. `wallet_amount` is required;
`credit_notes_amount` and `ongoing_usage_balance` are optional and read as 0
when absent. Available credits = `wallet + notes - ongoing`. The currency
code is uppercased when present, `XXX` when absent.

The registry row (`mistral`) also lists
`https://admin.mistral.ai/api/billing/v2/usage` (spend tables) and CodexBar
carries a console Vibe fallback endpoint; both stay unported because they
need CSRF tokens and origin-bound cookie forwarding, which the API-key-only
rule excludes. Only the credits endpoint is onboarded.

Fixture example: `tests/fixtures/usage-providers/mistral-credits.json`.

## Auth material

The admin console session cookie header value, supplied by the caller to
`poll_mistral` (registry kind `browser_session`). superai itself never reads
browser storage or cookie jars. Held in `PollSecret`, redacted in every
rendering.

## Window strategy and reset semantics

Registry `window_strategy` is `provider_only`: the credits endpoint reports
no quota windows, so the primary lane is `ProviderOnly` (the lane exists, but
only the provider's account surface reads it) rather than a phantom zero. No
reset instants.

## Pricing

Provider-reported balance in the response's own currency (EUR in the
fixture). Registry `pricing` is `subscription`; no per-token estimates are
computed.

## Upstream links

- Poller and parser: `crates/superai-usage/src/poller.rs`.
- Fixture: `crates/superai-usage/tests/fixtures/usage-providers/mistral-credits.json`.
- Registry row: `crates/superai-usage/assets/usage-providers.json`
  (`mistral`), verified 2026-10-13.
- Harness-side context: [../harness-configs/mistral-vibe.md](../harness-configs/mistral-vibe.md).
