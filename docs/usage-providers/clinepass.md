# ClinePass (usage source)

Verified 2026-10-13 against this repo's poller
(`crates/superai-usage/src/poller.rs`, `poll_clinepass`,
`clinepass_providers_path`, `clinepass_session`, `parse_clinepass_limits`)
and the fixtures
`crates/superai-usage/tests/fixtures/usage-providers/clinepass-limits.json`,
`clinepass-providers.json`. The porting origin is CodexBar's `clinepass.ts`;
the fixtures are the assumed shapes, no live account was polled.

## Where usage truth lives

One documented endpoint, GET
`https://api.cline.bot/api/v1/users/me/plan/usage-limits`. Shape
(`parse_clinepass_limits`): `success` must be true and `data.limits` an
array; each entry is `{type, percentUsed, resetsAt}` with `type` one of
`five_hour`, `weekly`, `monthly`. The three types map onto the three standard
lanes with fixed window lengths (5 h, 7 day, 30 day); `percentUsed` is taken
raw, over-quota magnitudes included, and `resetsAt` (string or null) becomes
the lane's reset instant. Unknown window types are skipped.

Session-file mode reads Cline's own settings, read-only. File resolution
(`clinepass_providers_path`): `CLINE_PROVIDER_SETTINGS_PATH` (a file), then
`CLINE_DATA_DIR/settings/providers.json`, then
`CLINE_DIR/data/settings/providers.json`, then
`~/.cline/data/settings/providers.json`, with `~` expanded against HOME. The
file must be a regular file (FIFOs and the like are refused before any read)
within the byte cap. Only the `providers.cline.settings` entry is consulted,
with Cline's own precedence: `auth.accessToken` (a browser sign-in token,
`workos:`-prefixed, marked oauth), then `settings.apiKey`, then
`auth.apiKey`. A missing or credential-less file yields nothing rather than
an error.

Fixtures: `clinepass-limits.json` (endpoint response),
`clinepass-providers.json` (session file, all three credential spellings
present; accessToken wins).

## Auth material

An API key (`CLINE_API_KEY` or `CLINEPASS_API_KEY` env var, registry row
`cline-pass`) sent as `Authorization: Bearer`, or the session token from the
providers file above (registry kind `token_file`). Whichever the caller
loads, it reaches the poller as a `PollSecret`, redacted in every rendering.

## Window strategy and reset semantics

Registry `window_strategy` is `endpoint`: three provider-reported quota
windows with their own reset instants. One deliberate divergence from the
porting origin: CodexBar's plugin clamps `percentUsed` to 0..=100, while
subplan 14's USG-04 rule mandates raw over-quota preservation, so superai
keeps the raw value.

## Pricing

Registry `pricing` is `subscription`: quota percentages only; no currency
amounts and no per-token estimates.

## Upstream links

- Poller, session reader, and parser: `crates/superai-usage/src/poller.rs`.
- Fixtures: `crates/superai-usage/tests/fixtures/usage-providers/clinepass-limits.json`,
  `clinepass-providers.json`.
- Registry row: `crates/superai-usage/assets/usage-providers.json`
  (`cline-pass`), verified 2026-10-13.
- Harness-side context: [../harness-configs/cline.md](../harness-configs/cline.md).
