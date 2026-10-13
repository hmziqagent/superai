# Subplan 14 — usage core: surfaces, readers, windows, costs, CLI

Parent: [master plan](master-plan.md)
Task prefix: USG (superai usage)
Estimate: 11–15 reviewable change sets

Status: done. All five exit-gate items pass with in-repo evidence. Two narrowings are
disclosed in the Residual record: the abuse extension lives in the usage crate's own
suites rather than inside `superai-core/src/abuse.rs`, and the punted leak surfaces
stay recorded as `#[ignore]`d repros.

## Outcome

`superai usage` answers two questions per harness and per provider: how much of the
current quota window is burned, and what did it cost. The truth sources, in order:
local usage artifacts (transcript logs, local databases), llmproxy's per-request
ledger once it exists (subplan 15 reads it; the proxy itself is the llmproxy
companion plan), and provider-native usage APIs for API-key providers (subplan 15).
CodexBar's core strategies (window model, reset handling, cost periods, pricing
metadata) are ported as Rust, none of its GUI. superai still does not proxy anything
itself (goal.md "Routing"); deploying and operating the proxy on a VPS is subplan 16.

Scope bases, all verified this round:

- superai at commit 586e154: 91 harness ids in `crates/superai-core/src/harness_catalog.rs`,
  the `Adapter` trait at `crates/superai-core/src/adapter.rs:1246`, no usage code anywhere
  in `crates/superai-core/src/lib.rs`'s module list, and a placeholder CLI
  (`crates/superai-cli/src/main.rs` prints instance records, no subcommands).
- CodexBar at tree `f66d18f` (snapshot under `/tmp/steipete-CodexBar-f66d18f`): 99
  provider directories in the core tree under `Sources/CodexBarCore/Providers/` (the
  descriptor/strategy set) plus 60 app-tree implementation directories under
  `Sources/CodexBar/Providers/` and a `Shared` support dir in each tree (counts by
  `find -maxdepth 1 -type d` this round; the two-tree split is `docs/provider.md`'s
  architecture overview), per-provider docs
  under `docs/`, the provider authoring guide at `docs/provider.md`, the window model at
  `Sources/CodexBarCore/UsageFetcher.swift:148` (`UsageSnapshot`) and `:3` (`RateWindow`),
  the cost-period model at `docs/cost-reporting-periods.md`, pricing metadata at
  `docs/model-pricing.md`, and the local-log cost scanner at
  `Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift` (Codex session roots at
  `:2232`, Claude projects roots via `Sources/CodexBarCore/Providers/Claude/ClaudeConfigPaths.swift:20`).

The llmproxy and toride scope bases live in their companion plans (below); this subplan
consumes their published artifacts only.

Claims that could not be verified are listed under "Unverified and excluded" and must not
be implemented on assumption.

## CodexBar core being ported

Strategies, not UI (all from the snapshot):

- Window model: `RateWindow { usedPercent, windowMinutes, resetsAt, resetDescription,
  nextRegenPercent }` (`UsageFetcher.swift:3-15`); a provider reports up to
  primary/secondary/tertiary windows plus named extras (`UsageFetcher.swift:148-152`).
  Over-quota raw values are preserved, not clamped upstream (comment at `:4-6`).
- Cost periods: rolling day windows (1–365), month-to-date in a pinned time zone, and
  all-history, with explicit "recorded spend is not a bill" semantics and distinct
  unavailable/zero/incomplete states (`docs/cost-reporting-periods.md`).
- Pricing metadata: models.dev `api.json` as additive source over bundled fallback rates,
  24h TTL cache, atomic writes, unknown-model retry cooldown (`docs/model-pricing.md`).
- Fetch strategy pipelines: each provider declares ordered strategies with a kind
  (cli, web cookies, oauth, api token, local probe, web dashboard), availability checks,
  and fallback policy (`docs/provider.md` "Fetch strategies").
- Auth kinds across the provider table: API keys in config/env (OpenRouter, DeepSeek,
  Factory, Kilo api mode), OAuth token files on disk (`~/.codex/auth.json` per
  `docs/codex.md`, Gemini CLI OAuth per `docs/gemini.md`, `~/.grok/auth.json` per
  `docs/grok.md`), CLI auth-file fallbacks (Kilo per `docs/kilo.md`), browser cookies and
  localStorage (OpenCode, Qwen Cloud, Kimi, Qoder, Windsurf, Devin, Command Code per
  their docs), and macOS Keychain (`KeychainAPI`, `docs/provider.md` host APIs).
- Guardrails worth keeping: identity silo per provider, timeout-bounded fetches,
  preserving each source's rounding contract, cached-data-over-flapping degradation
  (`docs/provider.md` "Guardrails").

Not ported: the menu bar app, widgets, Sparkle updates, PTY session keepalive helpers,
the WebView dashboard scraper, cookie importers, Keychain access, and the adaptive
refresh timer loop (see Out of scope).

## Overlap table (CodexBar provider → superai harness ids)

Verified by matching every one of the 99 core-tree provider directories against
the 91 catalog ids; each directory lands in exactly one of the table, the unmapped
list, or the unresolved pair, and the accounting below sums to 99:

| CodexBar provider | superai harness ids | Primary usage truth (per CodexBar docs) |
|---|---|---|
| Codex | codex-cli, oh-my-codex | local rollout JSONL + account usage endpoint |
| Claude | claude-code | local transcript JSONL + OAuth/admin API |
| ClinePass | cline | cline auth session or CLINE_API_KEY, 5h/weekly/monthly windows |
| Cursor | cursor, cursor-agent | Cursor.app auth + usage API, billing-cycle windows |
| Copilot | copilot-cli, copilot-coding-agent, copilot-vscode | GitHub device flow + internal usage API |
| Gemini | gemini-cli | CLI OAuth creds + private quota APIs |
| Antigravity | antigravity-cli | OAuth usage + local LSP probe |
| OpenCode | opencode, oh-my-opencode | opencode.ai session (cookies/API key) + local SQLite |
| OpenCodeGo | opencode (zen go endpoints) | GET opencode.ai/zen/go/v1/usage + local history db |
| Qwen Cloud | qwen-code | cookie auth, Team/Individual quota endpoints |
| Kimi | kimi-code-cli, legacy-kimi-cli | cookie auth, weekly quota, rate-limit parsing |
| Kilo | kilo-code, kilocode-cli | app.kilo.ai tRPC API or CLI auth-file fallback |
| Zed | zed-acp | cloud.zed.dev billing/usage endpoint |
| Windsurf | windsurf | localStorage session + local SQLite + GetPlanStatus API |
| Amp | amp | CLI usage + web fallback, credits |
| Grok | grok-build | grok agent stdio JSON-RPC + billing fallbacks |
| DeepSeek | deepseek-harness | API key balance endpoints |
| Command Code | command-code | cookie auth + api.commandcode.ai billing |
| Muse | muse-code | subscription endpoint + local session logs |
| MiniMax | minimax-code | API token endpoints (strict https host policy) |
| Factory | factory-droid | API key or session bearer, billing-limits |
| Warp | warp | app.warp.dev GraphQL GetRequestLimitInfo |
| Devin | devin | localStorage + quota endpoint |
| Kiro | kiro | (see docs/kiro.md; uses Kiro login runner) |
| MiMo | mimo-code | local usage fallback reader exists in CodexBar |
| Mistral | mistral-vibe | admin.mistral.ai credits/console endpoints |
| WorkBuddy | workbuddy | documented in docs/workbuddy.md |
| Qoder | qoder-cli, qoder-ide | qoder.com / qoder.com.cn account dashboards, cookie auth |
| Pi | pi, omp | local transcripts only, no credentials, models.dev rate estimates |

29 clean provider mappings covering 37 harness ids (OpenCodeGo lands on the `opencode`
harness's zen-go endpoints, so it adds a mapping without a new id). ClinePass is a real
Cline quota source, not a near-miss: docs/clinepass.md tracks the subscription's
five-hour, weekly, and monthly limits from Cline's own session file
(`~/.cline/data/settings/providers.json`, read-only, no login flow) or `CLINE_API_KEY`,
and superai's cline adapter exists for it. Pi needs no
credentials at all (docs/pi.md), so it belongs in the first local-reader batch alongside
the JSONL readers. The remaining 68 core-tree providers have no superai harness:
Abacus, Aerostack, AiAnd, Aixy, Alibaba, AtlasCloud, Augment, AzureOpenAI, Bedrock,
Bifrost, Chutes, ClawRouter, Codebuff, CodeRabbit, Cosmic, Deepgram, DeepInfra, DevPass,
Doubao, ElevenLabs, Exa, Fireworks, GitKraken, Groq, Helmcode, HuggingFace, Hyper,
IBMBob, JetBrains, Langdock, Linkup, LiteLLM, LithosAI, LLMMan, LLMProxy, LongCat,
Manus, Moonshot, MuseAI, NeuralWatt, Notion, Nous, Ollama, OpenAI, OpenRouter,
Perplexity, Poe, Raycast, Replicate, SailResearch, Sakana, Sofya, StepFun, Sub2API,
Synthetic, Tavily, TinyApi, TypeSafe, V0, Venice, Vercel, VertexAI, Wayfinder, XAI,
XAPI, XKiro, ZenMux, ZoomMate. XKiro is deliberately not mapped to the `kiro` harness:
it is api.xkiro.com's free-token meter, a different service from AWS's Kiro.
Accounting: 29 mapped + 68 unmapped + 2 unresolved (T3Chat, Zai, below) = 99 core-tree
provider directories. The unmapped ones still matter as provider-native usage
sources once superai instances route through them (plan 07's `ProviderDefinition` ids);
LLMProxy among them is CodexBar reading an llmproxy-compatible proxy's aggregate usage,
which the llmproxy companion plan returns to (PRX-03).

## Phase 1 — superai-usage foundation and first local readers

Repo: superai. Size: M/L (11–15 change sets). No dependency on other repos.

### USG-00 — register the subplan

The split that produced this plan appended rows 14, 15, and 16 to
`docs/plans/master-plan.md`'s subplan table plus its external-companion note, so the
table no longer stops at 13. The "What shipped" section still reads as if the shipped
subplans are the whole story; reword that header so the index is honest about subplans
being open again. First change set of the phase, so the master plan never lies about
scope while the work runs.

### USG-01 — UsageSurface on the Adapter trait

Add to the trait at `adapter.rs:1246`, following the `mcp_decl`/`mcp_absence_reason`
precedent (`adapter.rs:1342-1351`):

- `fn usage_surface(&self) -> Option<UsageSurface>` — the declaration, `None` meaning
  not yet modeled.
- `fn usage_absence_reason(&self) -> Option<&'static str>` — verified "this harness has
  no readable usage artifact" beats a declaration.

`UsageSurface` declares artifact locators (reusing the `PathResolver` machinery defined
at `adapter.rs:344`,
never a `ConfigSurface`: usage artifacts are foreign-owned read-only telemetry, not
config superai mutates, so `ConfigSurface`'s ownership/backup fields
(`adapter.rs:384-403`) do not apply), the reader kind, and which provider account the
artifacts belong to. Reading never writes, so no backup path exists; that invariant gets
a test, not a comment.

### USG-02 — superai-usage crate

New workspace member `crates/superai-usage`, layered above superai-core and below
superai-cli. Depends on superai-core plus serde/thiserror, and ureq from the start of
the phase: USG-05's models.dev fetch needs the same rustls-only, bounded client
discipline as `template_fetch.rs`. AGENTS.md's layout table gains the row;
`docs/dependency-review.md` gains its section
before the first merge that adds it. Modules: `window` (USG-04), `cost` (USG-05),
`reader` (local artifact readers), `registry` (USG-07), `llmproxy` (subplan 15,
USG-08), `poller` (subplan 15, USG-09).

### USG-03 — first local readers, chosen from the overlap table

First three, in order, each justified by the overlap data above:

1. `claude-code`: local transcript JSONL under the Claude config root's `projects/`
   (roots per `ClaudeConfigPaths.swift:20-60`, including the no-env `~/.config/claude`
   fallback; CodexBar scans `*.jsonl` at `CostUsageScanner+Claude.swift:692`). Reads
   tokens/cost from assistant usage entries. Highest value: it is the harness superai
   already models most deeply and the logs hold no credentials at all.
2. `codex-cli`: local rollout JSONL under `CODEX_HOME/sessions` else `~/.codex/sessions`,
   plus the archived-sessions sibling root (`CostUsageScanner.swift:2232-2254`).
   Same no-credential property.
3. `opencode`: read-only scan of the local v2 history database. This one is not
   credential-free: superai's own research doc says v2 moved the credential store into
   `opencode.db` (`docs/harness-configs/opencode.md`, "Credential storage" under "What
   v2 changes"), so the reader opens the database read-only, resolves the path the way
   the doc specifies (`$XDG_DATA_HOME/opencode/opencode.db` default, channel-suffixed
   names like `opencode-beta.db` on non-default channels, `OPENCODE_DB` overriding, per
   the doc's environment table), and reads only an allowlisted set of history tables.
   Credential tables are never selected, never logged, and their absence from the
   allowlist is pinned by a test that fails if the reader's SQL touches any other table.
   Quota lanes stay out until subplan 15's USG-09 (they live behind opencode.ai
   endpoints).

These three cover five harness ids (claude-code, codex-cli, oh-my-codex, opencode,
oh-my-opencode) with zero network and zero writes, and no credential bytes ever leaving
the file they live in. Pi is the fourth reader in the same batch: docs/pi.md's session
discovery section gives the roots (`~/.pi/agent/sessions` plus the OMP agent/profile
stores, honoring `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`, OMP XDG roots,
and `OMP_PROFILE`), the transcripts need no credentials, and costs are models.dev rate
estimates over recorded assistant usage, never a bill.

Readers parse
defensively: unknown line shapes are skipped and counted, never fatal (log formats drift
across harness versions; an era-style refusal belongs at the reader boundary, mirroring
`era_conflict_reason` at `adapter.rs:1326`).

### USG-04 — window engine

Port `RateWindow` semantics: raw over-quota values preserved (display clamps happen at
the interface, not in core, per the comment at `UsageFetcher.swift:4-6`), window minutes,
provider-reported `resets_at` preferred over locally computed countdowns, distinct
Unavailable/Zero/Incomplete states, no fabricated quota lane (CodexBar's synthesized-
window flag at `UsageFetcher.swift:13-15` becomes an explicit enum variant). Session and
weekly windows are computed from local logs where the provider computes them server-side
only when the provider's own semantics are documented; otherwise the window is reported
as provider-only and left to the account endpoint or llmproxy (subplan 15).

### USG-05 — cost engine and pricing registry

- Cost periods: rolling N days (1–365), month-to-date pinned to a configurable time zone,
  all-history; day bucketing handles DST 23/25h days (`docs/cost-reporting-periods.md`).
  Recorded-spend-not-a-bill semantics and the unavailable/zero/incomplete distinction
  are part of the public types.
- Pricing: models.dev `api.json` with bundled fallback rates, 24h TTL cache in superai's
  own cache root (never inside a harness config dir), atomic writes, bounded fetch under
  the same discipline as `template_fetch.rs` (rustls-only ureq, size caps, timeouts).
  Cost math uses exact decimal arithmetic; llmproxy's TOML pricing comment about
  unquoted-decimal precision loss (`providers/opencode-zen.toml.example`) is the cautionary
  precedent.

### USG-06 — `superai usage` CLI

The current binary has no subcommands (`crates/superai-cli/src/main.rs` is a printer).
Add a hand-rolled subcommand surface, no new dependency: `superai usage show
[--harness <id>] [--window session|week|month]`, `superai usage cost [--days N |
--period month-to-date|all]`, both with `--json`. Errors are typed and redacted before
printing. Output shows window fill, reset time, tokens, and cost per source, and marks
each row with its truth source (local artifact, llmproxy, provider API).

### USG-07 — usage provider registry

Data-only registry entries keyed by plan 07's `ProviderId`, mirroring the data-only
provider rule from subplan 07: entry fields are labels, usage-truth locators (endpoint
URLs, artifact path templates), auth-material kind (which file or env, never values),
window strategy, pricing reference, and documentation link with verification date.
Adding a standard usage provider is a data edit plus fixtures, not a Rust edit, unless
it needs a new reader kind. Bundled entries seed from the overlap table's provider
column; entries carry `schema_version` and `verified_at` like provider definitions do.

## Tests

- Reader property tests: fixture JSONL corpora with malformed lines, era drift, UTF-8
  boundary cases, and duplicate session ids; parsers never panic and always report
  skip counts. The opencode reader's SQL is pinned to its table allowlist by a test
  that fails on any other table name, and credential-table bytes never reach parsed
  output.
- Window engine: DST-crossing month-to-date, leap February, 1/365 rolling bounds,
  over-quota raw preservation, unavailable-vs-zero-vs-incomplete distinctness.
- Cost engine: decimal pricing precision (no f64 anywhere in cost math), missing
  pricing yields Unavailable with the model named, recorded-spend semantics.
- Registry: adding a provider via data only requires no Rust edit (ledger-style test
  mirroring subplan 07's `data_only_adding_provider_requires_no_code_change`).

## Exit gate

- [x] `superai usage show` and `superai usage cost` work for claude-code, codex-cli,
      oh-my-codex, opencode, oh-my-opencode, pi, omp from local artifacts alone,
      `--json` included.
- [x] UsageSurface declared or explicitly absent on every catalog adapter that has an
      overlap-table row.
- [x] Window and cost engines port CodexBar semantics with the unavailable/zero/
      incomplete distinction intact.
- [x] Pricing registry is data-only, cached, and never writes inside harness dirs.
- [x] No token, key, cookie, or account identity ever reaches a log, record, or CLI
      output; verified by superai's abuse suite (`crates/superai-core/src/abuse.rs`)
      extended.
      > The extension landed beside the code it guards, not inside core's abuse.rs:
      > `crates/superai-usage/src/abuse.rs` re-exports the sentinel helpers and the
      > suites live in `crates/superai-usage/tests/abuse_*.rs` plus the CLI's
      > planted-sentinel tests. Same proofs, different file; core's own abuse.rs had
      > no usage path to guard.

## Residual

> Completion record (closure run, through commit 0621bf1): USG-01 shipped as
> `usage_surface`/`usage_absence_reason` on the Adapter trait (`adapter.rs`), with the
> 37-id overlap ledger machine-checked in `crates/superai-core/src/usage_ledger.rs`:
> every overlap adapter declares exactly one usage state, non-overlap adapters stay
> unmodeled, and declaring never writes disk. USG-02 through USG-07 live in
> `crates/superai-usage` (readers under `src/reader/`, `window.rs`, `cost.rs`, and a
> registry seeded from the checked-in `assets/usage-providers.json`) and in
> `crates/superai-cli/src/usage.rs`. Evidence at the closure tree: `cargo test -p
> superai-usage --all-features` 230 passed, 3 ignored; `cargo test -p superai-cli` 58
> passed, of which 39 exercise the usage CLI against planted artifacts for all seven
> served ids with exact token and cost figures and `--json` shapes. Residuals, each
> an `#[ignore]`d repro in `crates/superai-usage/tests/abuse_punted_leaks.rs`:
> registry parse errors can echo artifact values, and error Displays echo
> caller-supplied paths (a standing crate convention the owners must decide on). The
> t3code/zaly mapping exclusions under "Unverified and excluded" stand unchanged.

## Out of scope

- GUI of any kind, including anything GPUI; this subplan is CLI and library only.
- superai proxying model traffic or doing wire translation (goal.md "Routing"; master
  plan DoD "No proxy/routing implementation" still holds for superai itself).
- Browser cookie importers, localStorage readers, and macOS Keychain access. These are
  CodexBar host APIs tied to macOS browser internals; porting them is a separate
  decision with its own security review, not part of reaching usage truth for the
  overlap set.
- OAuth login flows. Reading an existing token file to call a documented usage endpoint
  is in; performing logins, device flows, or token refresh is out (goal.md "Secrets").
- Background daemons, refresh loops, and the adaptive/agent-aware refresh policy. superai
  core keeps the no-background-polling rule from subplan 07 PRV-07; a `usage watch` loop
  can be argued for after the one-shot surface is boring.
- The PTY-based CLI usage strategies (Codex CLI RPC via app-server, Claude CLI PTY
  sessions). The account endpoints and local logs cover the same numbers without a
  terminal-session lifecycle.
- Changes inside the llmproxy or toride repos; each has its own companion plan, and
  superai only consumes published artifacts from them.

## Unverified and excluded

These items could not be verified from the sources this round and are excluded until
they are; none may appear as fact in implementation:

- A CodexBar provider mapping for superai's `t3code` (CodexBar's T3Chat is a web chat
  product; the superai harness is a terminal tool; no shared usage truth was found) and
  for `zaly` (CodexBar's Zai is the z.ai API; no superai-side link was confirmed).
- The exact count of CodexBar's JavaScript-plugin providers beyond the Swift provider
  directories and the docs index; the onboarding pattern does not depend on it.

## Open questions

None open in this subplan's own scope. The VPS baseline and China-line scheduling
questions moved to subplans 16 and 15 with the phases that own them.

## Companion plans

- [export/llmproxy-usage-ledger.md](export/llmproxy-usage-ledger.md): the llmproxy
  repo's own plan (PRX-01..06) making the proxy the usage-truth ledger. Canonical home
  is the llmproxy repository; the copy here is an export.
- [export/toride-vps-services.md](export/toride-vps-services.md): the toride repo's own
  plan (VPS-01) adding remote exec, upload, and systemd unit install plus the 0.2
  publishes. Canonical home is the toride repository; the copy here is an export.
- [Subplan 15](15-usage-sources.md): the llmproxy ledger client (USG-08), API-key
  pollers (USG-09), and the long-tail provider onboarding pattern (LNG-01).
- [Subplan 16](16-vps-deploy.md): `superai vps deploy llmproxy` (VPS-02) and
  `superai vps status` (VPS-03).
