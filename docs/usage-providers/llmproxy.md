# llmproxy usage ledger (usage source)

Verified 2026-10-13 against this repo's client
(`crates/superai-usage/src/llmproxy.rs`), its fixtures
(`crates/superai-usage/tests/fixtures/llmproxy/`), and the frozen endpoint
contract in [../plans/export/llmproxy-usage-ledger.md](../plans/export/llmproxy-usage-ledger.md)
(PRX-01..06). The llmproxy implementation lives in its own repo and had not
shipped when this was written, so the fixtures are the assumed response
baseline, not captures from a live ledger; drift against a deployed PRX-03 is
an open item tracked in subplan 15.

## Where usage truth lives

Three admin endpoints on a deployed llmproxy, read-only:

- `GET /usage/summary?window=...&group_by=provider|model|caller` (constant
  `SUMMARY_PATH`): window aggregate, one row per group.
- `GET /usage/requests?since=...&until=...&provider=...&limit=...`
  (`REQUESTS_PATH`): bounded raw rows. Responses carry no request bodies,
  ever.
- `GET /usage/keys` (`KEYS_PATH`): caller list with last-seen.

Base URL discovery (`resolve_base_url`): an explicit flag, then the
`SUPERAI_LLMPROXY_URL` env var, then the first valid base among instance
provider configs already known to route through llmproxy. A base may carry a
known `/usage` path, which is stripped. Validation cannot tell an llmproxy
base from any other https URL, so callers must pass only bases known to be
routed through llmproxy: the client sends the admin token to whatever base it
is given. https is required, plain http only on an exact loopback host;
userinfo, query, fragment, and control characters are refused. Redirects are
followed manually with each hop re-validated and same-origin enforced.

### Shapes the client parses

Recorded in `tests/fixtures/llmproxy/` as the drift baseline:

- `summary-provider.json`: `{window?, rows[]}`; each row has `group`,
  `requests`, `errors`, `input_tokens`, `output_tokens`, `cache_read_tokens`,
  `cache_write_tokens`, `cost_usd`.
- `requests-window.json`: `{rows[]}`; each row has `request_id`,
  `received_at`/`completed_at`, `provider`, `model`, `upstream_model`,
  `route_kind`, `client_protocol`, `streaming`, `caller`, the four token
  counts, `cost_usd`, `stop_reason`, `error_kind`, `http_status`,
  `latency_ms`, `body_hash`. Timestamps arrive as RFC 3339 strings or
  epoch-millisecond numbers; cost as decimal strings or JSON numbers.
- `keys.json`: `{keys[]}` with `caller` and `last_seen`.

Unknown fields are ignored; missing fields become `None`; a wrong-typed known
field or a missing `rows`/`keys` envelope fails the response loudly.

## Auth material

An admin bearer token, read by `load_admin_token`: a non-empty trimmed
`SUPERAI_LLMPROXY_ADMIN_TOKEN` env value wins, else a token file that must be
a regular file, mode 0600, at most 4096 bytes, non-empty (symlinks, FIFOs,
directories, wrong modes, oversize, and empty are all refused; mode and size
come from the open fd's own stat). The token is held in `AdminToken`, whose
`Debug`, `Display`, and `Serialize` all emit `[REDACTED]`; it is never stored
in instance records, and no error variant carries it.

## Window strategy and reset semantics

`window=` values (for example `24h`) pass through as untyped strings: PRX-03
fixes semantics, not a grammar, so none is invented here. `since`/`until` on
`/usage/requests` are typed timestamps serialized as RFC 3339. These are query
windows over the ledger, not quota windows; the ledger models no reset
instants. When instances route through llmproxy, the ledger number wins for
those instances and the local artifact scan is shown as a cross-check with an
absolute divergence line (`LedgerCrossCheck`); the two are never summed.

## Pricing

The ledger records exact per-request cost (`cost_usd`, USD) computed proxy-side;
`UsageSummary::total_cost` returns `None` if any row's cost is unrepresentable,
so a partial total can never pose as the whole one.

## Upstream links

- Client: `crates/superai-usage/src/llmproxy.rs`; tests
  `crates/superai-usage/tests/llmproxy_client.rs` (loopback mocks only; no
  live deployment has been verified).
- Frozen contract: [../plans/export/llmproxy-usage-ledger.md](../plans/export/llmproxy-usage-ledger.md).
- Fixtures: `crates/superai-usage/tests/fixtures/llmproxy/`.
