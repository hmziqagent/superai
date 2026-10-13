# Claude Code transcripts (usage source)

Verified 2026-10-13 against this repo's reader
(`crates/superai-usage/src/reader/jsonl.rs`, `scan_claude_transcripts`) and its
fixtures (`crates/superai-usage/tests/fixtures/claude/`). The porting origin is
CodexBar's `ClaudeConfigPaths.costProjectsRoots` scanner; this repo's code is
the authority. Harness background: [../harness-configs/claude-code.md](../harness-configs/claude-code.md).

## Where usage truth lives

One JSON object per line in session transcripts under the project roots,
resolved in this order (`claude_projects_roots` in
`crates/superai-usage/src/reader/jsonl.rs:64`):

- `CLAUDE_CONFIG_DIR/projects` when `CLAUDE_CONFIG_DIR` is set and
  non-empty; otherwise `~/.config/claude/projects`, then
  `~/.claude/projects`.
- Cswap slot roots, appended either way:
  `~/.claude-swap-backup/sessions/<N>-*/projects` and
  `$XDG_DATA_HOME/claude-swap/sessions/<N>-*/projects` (default
  `~/.local/share`). Slot directories must start with a positive number.

Every `*.jsonl` file under the roots is scanned, deduplicated by canonical
path. Hidden entries (leading dot), symlinks, and non-regular files are
skipped.

### Shape the reader parses

Only `type: "assistant"` lines carry usage. A usage-bearing row needs, all at
the top level unless noted:

- `timestamp`: RFC 3339 string.
- `message.id`: dedup key part (strings up to 1024 bytes; longer ids are
  refused whole, not truncated).
- `message.model`, `message.usage` object with `input_tokens`,
  `cache_read_input_tokens`, `cache_creation_input_tokens`, `output_tokens`.
  An absent counter reads as 0; a present-but-unreadable one spoils the whole
  payload into the skipped count.
- Dedup key: `message.id` plus `requestId` when present, else `sessionId`,
  checked per source in order line then `message`, each source trying
  `sessionId`, `session_id`, then `metadata.sessionId`.

Streaming chunks repeat cumulative usage per message; per dedup key the
reader keeps the most complete row, so a copied transcript under two roots
counts once. Rows with all four counters at zero parse but emit nothing. A
proxy-era estimate (`stop_reason` null, input only, no cache counters)
never displaces a full row for its key whatever the totals; it keeps its
tokens and flags the scan as a coverage gap (`unscanned_records`). Within
one class the row with the larger token total wins.

Fixture example: `tests/fixtures/claude/projects/home-project/session-one.jsonl`.

## Auth material

None. Transcripts carry token counts and model ids, no credentials, and the
reader opens nothing else. The registry row (`assets/usage-providers.json`,
id `claude`) also records the provider's own usage endpoint
(`https://api.anthropic.com/api/oauth/usage?cedar_ember=1`) and its token file
`~/.claude/.credentials.json`; that endpoint is registry data only, nothing in
this repo polls it.

## Window strategy and reset semantics

Window strategy `local_artifacts` in reader terms, but the honest fill is
`provider-only`: transcripts carry no quota denominator, so `superai usage
show` never fabricates a window fill. `resets_at` is computed only for the
month window (first instant of the next month in the system zone); session and
week report null. A zone whose next-month-start midnight is DST-deleted (for
example America/Santiago Septembers) fails the month window with an error
rather than approximating. Totals scope to the selected window's period range.

## Pricing

Rate estimates from the cost engine's `RateTable`: bundled first-party list
prices (fetched 2026-10-13 from anomalyco/models.dev, provider `anthropic`)
merged with any loadable models.dev cache under
`$XDG_CACHE_HOME/superai/model-pricing/models-dev.json` (else
`$HOME/.cache/superai/...`). Units are USD per 1M tokens, input, output,
cache-read, and cache-write each priced where the catalog prices them. These
are estimates, never a bill. Unpriced model ids surface as `missing_pricing`;
skipped lines and unreadable artifacts make every total a floor.

## Upstream links

- Reader and caps: `crates/superai-usage/src/reader/jsonl.rs`,
  `crates/superai-usage/src/reader.rs` (512 KiB record cap).
- Fixtures: `crates/superai-usage/tests/fixtures/claude/`.
- Registry row: `crates/superai-usage/assets/usage-providers.json` (`claude`).
- Harness config surface: [../harness-configs/claude-code.md](../harness-configs/claude-code.md).
