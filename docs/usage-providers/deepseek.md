# DeepSeek (usage source)

Verified 2026-10-13 against this repo's poller
(`crates/superai-usage/src/poller.rs`, `poll_deepseek` and
`parse_deepseek_balance`) and the fixture
`crates/superai-usage/tests/fixtures/usage-providers/deepseek-balance.json`.
The porting origin is DeepSeek's own API docs as indexed by the CodexBar
snapshot; the fixture is the assumed shape, no live account was polled.

## Where usage truth lives

One documented endpoint, GET `https://api.deepseek.com/user/balance` with a
Bearer key. Shape (`parse_deepseek_balance`): `{balance_infos: [...]}` where
each entry has `currency`, `total_balance` (required decimal string),
`granted_balance` and `topped_up_balance` (optional decimal strings). The USD
entry is preferred when present, otherwise the first entry wins. The balance
lane reports available = `total_balance`, with granted = `granted_balance` and
paid = `topped_up_balance` split out when present.

Fixture example: `tests/fixtures/usage-providers/deepseek-balance.json` (CNY
and USD entries; the reader picks USD).

## Auth material

`DEEPSEEK_API_KEY` or `DEEPSEEK_KEY` env var (registry row `deepseek`), sent
as `Authorization: Bearer`. Held in `PollSecret`, redacted in every rendering.

## Window strategy and reset semantics

Registry `window_strategy` is `cost_only`: the endpoint reports a balance,
not quota windows, so all window lanes are `Unavailable` and nothing is
fabricated. No reset instants exist.

## Pricing

Registry `pricing` is `unverified`: the reported amounts are an account
balance in the entry's currency (USD or CNY), not a token rate card. No
per-token estimates are computed for this source.

## Upstream links

- Poller and parser: `crates/superai-usage/src/poller.rs`.
- Fixture: `crates/superai-usage/tests/fixtures/usage-providers/deepseek-balance.json`.
- Registry row: `crates/superai-usage/assets/usage-providers.json`
  (`deepseek`), verified 2026-10-13.
