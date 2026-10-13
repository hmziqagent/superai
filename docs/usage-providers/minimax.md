# MiniMax (usage source)

Verified 2026-10-13 against this repo's poller
(`crates/superai-usage/src/poller.rs`, `poll_minimax`,
`validate_minimax_endpoint_override`, `parse_minimax_remains`) and the fixture
`crates/superai-usage/tests/fixtures/usage-providers/minimax-remains.json`.
The porting origin is CodexBar's `MiniMaxUsageFetcher.swift` and
`MiniMaxModelRemains.swift`; the fixture is the assumed shape, no live account
was polled.

## Where usage truth lives

One documented endpoint, GET
`<host>/v1/api/openplatform/coding_plan/remains` with a Bearer token, plus
`MM-API-Source: superai`. The default host is `https://www.minimax.io`;
`www.minimaxi.com` is the China mainland region host. A host override must
pass `validate_minimax_endpoint_override`: https only (a bare host is
https-qualified), no userinfo, a host of ASCII letters, digits, hyphens, and
dots only (which refuses literal and percent-encoded delimiters and non-ASCII
homographs of MiniMax domains in one fail-closed rule), an optional digits-only
port or bracketed IPv6 literal. With strict mode (the
`MINIMAX_REQUIRE_PROVIDER_ENDPOINT_OVERRIDES` posture) the host must be
`minimax.io` or `minimaxi.com` or a subdomain of either. The result is a
normalized `https://` origin.

Shape (`parse_minimax_remains`): `data.model_remains[]`, one entry per model:

- `model_name`; the first entry fills the primary lane, the rest become named
  extras keyed by model name.
- Usage percent: `100 - current_interval_remaining_percent`, falling back to
  `(current_interval_total_count - current_interval_usage_count) /
  current_interval_total_count` when the percent field is absent.
- Window length from `start_time`/`end_time` epochs (seconds versus
  milliseconds split at `> 1e12`); reset instant from `end_time`.
- A non-zero `base_resp.status_code`, at the data level or the root, is an
  error, not a zero reading.

MiniMax's weekly `current_weekly_*` fields are ignored: CodexBar renders them
only for model names its own rule accepts, a rule not verifiable from the
snapshot docs. The HTML coding-plan page, billing history, and the web/cookie
path are excluded by subplan 15's API-key-only rule.

Fixture example: `tests/fixtures/usage-providers/minimax-remains.json`.

## Auth material

`MINIMAX_CODING_API_KEY` or `MINIMAX_API_KEY` env var (registry row
`minimax`), sent as `Authorization: Bearer`. Held in `PollSecret`, redacted
in every rendering.

## Window strategy and reset semantics

Registry `window_strategy` is `endpoint`: per-model interval windows whose
length and reset instant both come from the response's own epoch pair. No
calendar month is computed locally.

## Pricing

Registry `pricing` is `subscription`: quota percentages and points, not
currency; the `points_balance` field in responses is tolerated as an unknown
extra and not interpreted.

## Upstream links

- Poller, host policy, and parser: `crates/superai-usage/src/poller.rs`
  (`MINIMAX_PROVIDER_DOMAINS`, `MINIMAX_DEFAULT_HOST`,
  `MINIMAX_REMAINS_PATH`).
- Fixture: `crates/superai-usage/tests/fixtures/usage-providers/minimax-remains.json`.
- Registry row: `crates/superai-usage/assets/usage-providers.json`
  (`minimax`), verified 2026-10-13.
- Harness-side context: [../harness-configs/minimax-code.md](../harness-configs/minimax-code.md).
