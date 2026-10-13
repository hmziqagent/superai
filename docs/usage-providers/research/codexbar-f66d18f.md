# CodexBar f66d18f dossier

Frozen record of upstream CodexBar facts the superai usage port depends on. Upstream is
[steipete/CodexBar](https://github.com/steipete/CodexBar) at commit
`f66d18fc6fbb85c7df056bdeef56992de0619038` (short `f66d18f`, committed 2026-10-12T13:29:06Z,
"fix(menu-bar): redraw meters after leaving a stored layout (#4463)").

Verification date for every fact below: 2026-10-13.

## Snapshot provenance

The local snapshot lives at `/tmp/steipete-CodexBar-f66d18f`. It carries no `.git` directory,
so it was checked against upstream by content: all 14 files cited in this dossier were fetched
from `raw.githubusercontent.com/steipete/CodexBar/f66d18f/<path>` with `curl -fsSL` and byte-
compared against the snapshot with `diff`. All 14 identical. Each GitHub link below goes to the
same ref, so the trail no longer needs the `/tmp` copy.

## Claude cost-project roots

Upstream: `Sources/CodexBarCore/Providers/Claude/ClaudeConfigPaths.swift:20-63`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Providers/Claude/ClaudeConfigPaths.swift#L20-L63)).

`costProjectsRoots` always includes `configRoot/projects`, where `configRoot` is
`CLAUDE_CONFIG_DIR` when set to a nonempty literal, else `~/.claude` (lines 9-17, 32). Only when
`CLAUDE_CONFIG_DIR` is unset or empty does it prepend `~/.config/claude/projects` (line 34) and
append the desktop locator roots (lines 35-36). The swap-slot roots described below are appended
regardless of `CLAUDE_CONFIG_DIR`.

Official doc: Claude Code settings
([code.claude.com/docs/en/settings](https://code.claude.com/docs/en/settings)) documents
`CLAUDE_CONFIG_DIR`: "To keep the home-directory files somewhere else, set `CLAUDE_CONFIG_DIR`;
Claude Code then stores your settings, session history, and plugins there instead." Default is
`~/.claude`. Verified 2026-10-13.

## Claude Desktop projects locator

Upstream: `Sources/CodexBarCore/Providers/Claude/ClaudeDesktopProjectsLocator.swift:3-124`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Providers/Claude/ClaudeDesktopProjectsLocator.swift#L3-L124)).

Seeds are `~/Library/Application Support/Claude/local-agent-mode-sessions` and
`~/Library/Application Support/Claude/claude-code-sessions` (lines 4-7, 23-29). The scan is a
breadth-first walk capped at depth 4 (line 39) that skips `.build`, `.git`, `build`,
`DerivedData`, `node_modules`, `outputs`, and `target` (lines 9-17) and never descends into
symlinked directories (lines 113-121). A directory contributes a root when it contains a
`.claude/projects` child (lines 93-101). Comment at lines 35-38 explains why the
`claude-code-sessions` seed is kept even though its Desktop entries are metadata: their
`cliSessionId` maps into the shared `~/.claude/projects` logs.

## Claude swap-slot roots are unconditional

Upstream: `Sources/CodexBarCore/Providers/Claude/ClaudeConfigPaths.swift:39-60`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Providers/Claude/ClaudeConfigPaths.swift#L39-L60)).

Two swap-session families feed the slot loop: legacy `~/.claude-swap-backup/sessions` (line 39)
and, on Linux only, `$XDG_DATA_HOME/claude-swap/sessions` with `~/.local/share/claude-swap/sessions`
as the default (lines 40-46). For each family the code lists child directories and keeps entries
whose name splits on the first `-` into a positive integer prefix plus a remainder, then appends
`<slot>/projects` when it exists as a directory (lines 48-60). This loop sits after the
`CLAUDE_CONFIG_DIR`-guarded block at lines 33-37, so swap roots are appended even when
`CLAUDE_CONFIG_DIR` is set. Comment at line 47: cswap run stores profiles one level below
`sessions`, and shared-history projects may be symlinks.

Returned roots are deduplicated by `standardizedFileURL` and resolved symlink path (lines 61-62).

## Claude row replacement prefers complete rows

Upstream: `Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+Claude.swift:304-306`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner%2BClaude.swift#L304-L306)).

```swift
private static func shouldReplaceClaudeRow(_ existing: ClaudeUsageRow?, with row: ClaudeUsageRow) -> Bool {
    row.isIncomplete != true || existing == nil || existing?.isIncomplete == true
}
```

Replacement is decided by completeness, not by row size. A row is incomplete when
`stop_reason` is null while input tokens are positive and output tokens are zero
(+Claude.swift:220). `mergeClaudeRows` (lines 308-330) keys rows by message id plus request id,
falling back to session id plus message id when a proxy response omits `requestId`
(lines 291-302).

## Codex token_usage_record lines are decoded, not skipped

Upstream: `Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift:3661-3670, 3725-3747`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L3661-L3670),
[second span](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L3725-L3747)).

In the fallback JSON path, line type `session_meta` and `token_usage_record` are decoded with a
full `JSONSerialization` pass and handed to `codexLine(from:)` (line 3666). There,
`token_usage_record` becomes `.tokenUsageRecord` when `codexRequestUsageRecord(from:)` succeeds,
otherwise `codexRejectedRequestUsage` becomes `.invalidRequestUsage` (lines 3744-3747). A record
whose timestamp cannot be parsed still surfaces as an invalid-usage marker rather than being
dropped (lines 3728-3735). Skipping these lines wholesale means a rollout contributes no request
usage at all upstream would have counted once.

## Ledger records pair with token_count mirrors

Upstream: `Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift:3480-3518, 4240-4255`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L3480-L3518),
[second span](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L4240-L4255)).

`CodexFastLine` carries both shapes: `tokenCount` from `event_msg` payloads of type
`token_count` (line 3709) and `tokenUsageRecord` from ledger lines. `boundaryTokenCount`
(lines 3493-3505) projects a ledger record into the token-count shape so the two can be paired.
Pairing happens through mirror keys built from turn id, usage sizes, and timestamp adjacency;
a response id already present in the ledger marks a replay (`isReplay`, line 4240), and
"only original observations advance the ledger counter" (comment, line 4249-4250), so a mirrored
pair is counted once. The comment at lines 4254-4255 notes the ordering constraint: a
token_count written before its ledger record pairs only inside the window.

## 1h ephemeral cache-write tokens are priced separately

Upstream: `Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+Claude.swift:214, 281-285`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner%2BClaude.swift#L281-L285))
and `Sources/CodexBarCore/Vendored/CostUsage/CostUsagePricing.swift:763-794`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Vendored/CostUsage/CostUsagePricing.swift#L763-L794)).

`claudeOneHourCacheCreationTokens` reads
`usage.cache_creation.ephemeral_1h_input_tokens`, clamped to `[0, total]` (+Claude.swift:281-285),
and feeds it into the cost tokens at line 214. Pricing splits cache creation into a 5-minute part
billed at the cache-creation rate and a 1-hour part billed at `inputRate * 2`
(CostUsagePricing.swift:770-792), with long-context threshold rates applied per component when
the input exceeds the model's threshold.

Official doc: Anthropic prompt caching
([platform.claude.com/docs/en/build-with-claude/prompt-caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching))
states "5-minute cache write tokens are 1.25 times the base input tokens price" and "1-hour cache
write tokens are 2 times the base input tokens price", with cache reads at 0.1x (model-specific
exceptions noted there). The upstream `inputRate * 2` matches the documented 1-hour multiplier.
Verified 2026-10-13.

## RateWindow fields and resets_at typing

Upstream: `Sources/CodexBarCore/UsageFetcher.swift:3-100`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/UsageFetcher.swift#L3-L100)).

`RateWindow` fields: `usedPercent: Double` (required; the provider's raw value is intentionally
not normalized, lines 4-6), `windowMinutes: Int?`, `resetsAt: Date?`, `resetDescription: String?`
(textual reset description used by the Claude CLI UI scrape), `nextRegenPercent: Double?`, and
`isSyntheticPlaceholder: Bool` defaulting to false (lines 6-21).

`resetsAt` is decoded as a typed date, `decodeIfPresent(Date.self, ...)` at line 52, not a
string. `isSyntheticPlaceholder` marks windows synthesized for a lane the provider did not
report, distinct from a real zero-usage window (lines 13-21); `measured` returns nil for such a
window (lines 72-75).

`backfillingResetTime(from:now:)` (lines 81-100) reuses a cached `resetsAt` only when the fresh
window has none and the cached one is still in the future, preserving the placeholder marker so a
backfilled reset cannot turn Claude web's null-session placeholder into a real lane.

## nextRegenPercent semantics

Upstream: `Sources/CodexBarCore/UsageFetcher.swift:11-12`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/UsageFetcher.swift#L11-L12)).

The upstream doc comment: "Optional percent restored on the next regeneration tick for providers
with rolling recovery." It is an optional Double, decode at line 54, encode only when present at
line 65.

Producers: the plugin snapshot mapper reads a plugin-supplied `nextRegenPercent` as a finite
number and clamps it to `[0, 100]`
(`Sources/CodexBarCore/Plugins/ProviderPluginSnapshotMapper.swift:252-258`,
[GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Plugins/ProviderPluginSnapshotMapper.swift#L252-L258)).
The plugin type contract declares it optional
(`Sources/CodexBarCore/Resources/Plugins/codexbar-plugin.d.ts:34-39`,
[GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Resources/Plugins/codexbar-plugin.d.ts#L34-L39)).
The bundled synthetic plugin maps payload fields `tickPercent`, `tick_percent`, `nextTickPercent`,
`next_tick_percent` into it
(`Sources/CodexBarCore/Resources/Plugins/synthetic.js:269-272`,
[GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/Sources/CodexBarCore/Resources/Plugins/synthetic.js#L269-L272)).
Native descriptor code only passes the field through (`ProviderDescriptor.swift:294`,
`CodexProviderDescriptor.swift:219`).

## Provider usage endpoints

All endpoints below are from the upstream docs at `f66d18f`. Official provider documentation was
checked where it exists; several of these are web-console contracts with no public doc, noted per
provider. Verified 2026-10-13.

### Devin

Upstream: `docs/devin.md:109-114`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/docs/devin.md#L109-L114)).

`GET https://app.devin.ai/api/<internal-org-id>/billing/quota/usage`, authenticated with the
signed-in browser session; the session token is sent only to `app.devin.ai` (devin.md:23). The
response carries daily and weekly usage percentages and reset timestamps; the daily quota is
omitted when Devin sets `hide_daily_quota` to true (devin.md:114). No official public API doc
found for this endpoint; searched 2026-10-13.

### Command Code

Upstream: `docs/command-code.md:16-17, 31`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/docs/command-code.md#L16-L17)).

Billing endpoints under `https://api.commandcode.ai`, authenticated with the signed-in web
session. A bare pasted token is sent as cookie `__Secure-commandcode_prod_.session_token`
(command-code.md:31); explicit cookie pairs keep their names. No public API doc found; searched
2026-10-13.

### Xiaomi MiMo

Upstream: `docs/mimo.md:42-44`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/docs/mimo.md#L42-L44)).

Balance and token-plan detail/usage endpoints under
`https://platform.xiaomimimo.com/api/v1`, requiring cookies `api-platform_serviceToken` and
`userId` (optional `api-platform_ph`, `api-platform_slh`). `MIMO_API_URL` overrides the base URL,
normalized to HTTPS. The platform itself is official
([platform.xiaomimimo.com](https://platform.xiaomimimo.com)); its public pages document the
inference APIs (OpenAI- and Anthropic-compatible token-plan endpoints per Xiaomi MiMo's official
account), not these console `/api/v1` calls. Verified 2026-10-13.

### MiniMax

Upstream: `docs/minimax.md:40-46, 54`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/docs/minimax.md#L40-L46)).

Web sessions use the global host or the China mainland host; `MINIMAX_HOST` defaults the example
to `platform.minimaxi.com`, and `MINIMAX_CODING_PLAN_URL` / `MINIMAX_REMAINS_URL` override full
URLs (https only). The cookie-capture instructions name the coding-plan request as
`/v1/api/openplatform/coding_plan/remains` (minimax.md:54). The endpoint is not in MiniMax's
public docs as indexed; searched 2026-10-13.

### Qwen Cloud

Upstream: `docs/qwen-cloud.md:52-59`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/docs/qwen-cloud.md#L52-L59)).

Individual token-plan calls go through the `sfm_bailian` console gateway:
`personal/api/v2/usage`, `personal/api/v2/subscription`, `personal/api/v2/quota-config`, with
form-encoded fields `product=sfm_bailian`, `action=IntlBroadScopeAspnGateway`,
`region=ap-southeast-1`, `language=en-US`, a resolved `sec_token`, and the provider-native
payload. Team mode reads `/tool/user/info.json`, `ea-service / LoadHumanInfo`, then
`BssOpenAPI-V3 / GetSeatSubscriptionSummary` (qwen-cloud.md:43-47).

First-party check, 2026-10-13: the console's own public client bundles referenced by the doc
contain the cited strings. `shared.js` (2,169,452 bytes) contains `personal/api/v2/quota-config`,
`IntlBroadScopeAspnGateway`, `sfm_bailian`; `analytics.js` (587,077 bytes) contains
`personal/api/v2/usage`, `personal/api/v2/subscription`, `personal/api/v2/addon`, `sfm_bailian`.
Both at
`https://q.alcasset.com/code/qwen-cloud/console-home/1.1.44/assets/`.

### Kiro

Upstream: `docs/kiro.md:26-36`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/docs/kiro.md#L26-L36)).

Usage comes from the `kiro-cli /usage` report, enriched best-effort by the `GetUsageLimits` API:
`POST https://codewhisperer.us-east-1.amazonaws.com/` for US East profiles, `POST
https://q.eu-central-1.amazonaws.com/` for Frankfurt, header
`X-Amz-Target: AmazonCodeWhispererService.GetUsageLimits`, body `{"profileArn": ...}`. Tokens
come from the CLI's own state at `~/Library/Application Support/kiro-cli/data.sqlite3`, opened
read-only (`auth_kv` key `kirocli:odic:token`, `state` key `api.codewhisperer.profile`).

Official doc: the AWS Q Developer firewall allowlist
([docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/firewall.html](https://docs.aws.amazon.com/amazonq/latest/qdeveloper-ug/firewall.html))
lists `https://codewhisperer.us-east-1.amazonaws.com` and
`https://q.{{profile-region}}.amazonaws.com` as "Amazon Q Developer features" endpoints, which
covers both endpoint shapes upstream uses. `GetUsageLimits` itself is not publicly documented.
Verified 2026-10-13.

### Zed

Upstream: `docs/zed.md:16-38`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/docs/zed.md#L16-L38)).

Browser billing: `GET https://cloud.zed.dev/frontend/billing/usage` with cookie `zed.session`;
the plugin maps `current_usage.token_spend.spend_in_cents` and `limit_in_cents` to USD, and a
missing limit stays unknown. Local probe: `GET https://cloud.zed.dev/client/users/me` with
`Authorization: {user_id} {access_token}` from the credentials Zed stores after GitHub sign-in.

Upstream calls the billing endpoint "an undocumented frontend contract" with the endpoint
evidence in [issue #3172](https://github.com/steipete/CodexBar/issues/3172), titled "Zed: Token
Spend reachable via browser cookie at cloud.zed.dev/frontend/billing/usage; keychain credentials
rejected (revisit #1995)", state closed (checked 2026-10-13). The official Zed docs page for LLM
providers does not document either endpoint; checked 2026-10-13.

### Warp

Upstream: `docs/warp.md:33-41`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/docs/warp.md#L33-L41)).

`https://app.warp.dev/graphql/v2?op=GetRequestLimitInfo`, query `GetRequestLimitInfo`, fields
`isUnlimited`, `nextRefreshTime`, `requestLimit`, `requestsUsedSinceLastRefresh` (the API uses
request-named fields for credits). Auth is an API key via `WARP_API_KEY` or `WARP_TOKEN`, or the
Settings providers entry.

Official doc: Warp's add-on credits page
([docs.warp.dev/support-and-community/plans-and-billing/add-on-credits](https://docs.warp.dev/support-and-community/plans-and-billing/add-on-credits))
states "Warp uses your included allowance first, then available purchased usage", matching the
consumption order upstream renders (warp.md:39). The GraphQL operation itself is not publicly
documented. Verified 2026-10-13.

### Kilo

Upstream: `docs/kilo.md:14-21`
([GitHub](https://github.com/steipete/CodexBar/blob/f66d18f/docs/kilo.md#L14-L21)).

API mode calls `https://app.kilo.ai/api/trpc` with a token from `KILO_API_KEY` or the app config.
CLI mode reads `~/.local/share/kilo/auth.json` and uses `kilo.access`. Auto tries API first and
falls back to CLI only on missing or unauthorized (401/403) API credentials. No public API doc
found for the trpc endpoint; searched 2026-10-13.
