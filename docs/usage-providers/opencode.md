# OpenCode history database (usage source)

Verified 2026-10-13 against this repo's reader
(`crates/superai-usage/src/reader/opencode.rs`) and its tests
(`crates/superai-usage/tests/opencode_reader.rs`). The query mirrors opencode's
own usage scan (`packages/core/src/session/stats.ts` at v2.0.27). Harness
background: [../harness-configs/opencode.md](../harness-configs/opencode.md).

## Where usage truth lives

SQLite database on the v2 line, resolved per
`history_db_path` in `crates/superai-usage/src/reader/opencode.rs:19`:

- a non-empty `OPENCODE_DB` value wins outright;
- else `$XDG_DATA_HOME/opencode/opencode.db` (default
  `~/.local/share/opencode/opencode.db`); a relative or unset
  `XDG_DATA_HOME` falls back to the default.

Non-default channels write channel-suffixed names (`opencode-beta.db`), but
the channel is not env-readable (see
[../harness-configs/opencode.md](../harness-configs/opencode.md) §6), so
superai reads only `OPENCODE_DB` or the default `opencode.db`. The family is
`opencode = [opencode, oh-my-opencode]`; the store is scanned once.

### Shape the reader parses

A read-only connection (`SQLITE_OPEN_READ_ONLY`) runs one query joining
`session_message` with `session_v2`; those two tables are the allowlist, and
credential tables (`credential`, `account`, `kv`, ...) are never selected.
Rows are `type = 'assistant'` messages, dropped when they predate their
session's fork point (`session.fork_session_id` set and
`message.time_created < session.time_created`). Per row, via `json_extract`
over `message.data`:

- `model.providerID`, `model.id` (provider and model ids);
- `tokens.input`, `tokens.output`, `tokens.reasoning` (reasoning folds into
  output), `tokens.cache.read`, `tokens.cache.write`;
- `message.time_created` as epoch milliseconds.

Malformed JSON in `data` becomes a counted skip, not a query error. Absent
token cells read as 0; malformed cells count as unscanned; all-zero rows are
dropped. Provider and model ids pass through as stored, whatever provider the
row names.

## Auth material

None for the reader: usage rows carry no credentials, and the query never
touches the credential tables the same database holds. The v1
`~/.local/share/opencode/auth.json` file is not read. The registry row
(`assets/usage-providers.json`, id `opencode`) also lists the console billing
endpoint (`https://opencode.ai/console/api/billing/status`, browser session);
nothing in this repo polls it.

## Window strategy and reset semantics

`provider-only` fill: the database carries no quota denominator. `resets_at`
is computed only for the month window (first instant of the next month, system
zone); session and week report null. Failure modes split deliberately: a
missing or locked database renders an empty row plus a note (exit 0), while a
scan that fails after opening exits 1.

## Pricing

`RateTable` estimates, USD per 1M tokens, from the bundled table merged with
any loadable models.dev cache. Rows arrive with arbitrary provider ids, so
pricing is a lookup on the row's own provider/model pair; unpriced pairs
surface as `missing_pricing` rather than borrowing another provider's rates.
Estimates, never a bill; unscanned rows make totals floors.

## Upstream links

- Reader and query: `crates/superai-usage/src/reader/opencode.rs`.
- Tests and fixture databases: `crates/superai-usage/tests/opencode_reader.rs`.
- Registry row: `crates/superai-usage/assets/usage-providers.json` (`opencode`).
- Upstream scan this mirrors: opencode `packages/core/src/session/stats.ts`
  (v2.0.27), cited in the reader's source comment.
