# Subplan 15 — usage sources: llmproxy ledger client and provider-native pollers

Parent: [master plan](master-plan.md)
Task prefixes: USG (usage sources), LNG (long-tail onboarding)
Estimate: 16–22 reviewable change sets, then S per provider batch

## Outcome

Subplan 14 reads local artifacts only. This subplan adds the other two truth sources:
the per-request ledger served by a deployed llmproxy (the proxy-side work is the
llmproxy companion plan, PRX-03 provides the endpoints), and provider-native usage
APIs for providers whose auth material is an API key in config/env. It also fixes the
pattern that onboard the long tail without a Rust edit per provider.

Dependencies: subplan 14 for the `superai-usage` crate, the `registry` module (USG-07),
and the window/cost engines; the llmproxy companion plan's PRX-03 for the `/usage`
endpoints USG-08 calls. USG-09 and LNG-01 are independent of llmproxy.

## USG-08 — llmproxy client in superai-usage

Repo: superai. Size: S/M (4–6 change sets). Depends on PRX-03 (llmproxy companion
plan).

Config: `superai usage` discovers the proxy from an explicit `--proxy <url>` flag, an
env var, or an instance's effective provider config when its base URL points at a
llmproxy deployment (superai already writes such configs; goal.md "Routing"). The
client calls `/usage/summary` and `/usage/requests` with the admin token read from an
env var or a 0600 file, never stored in instance records (same key-sink rule as subplan
07 PRV-04). Output merges with local readers in `usage show`, each row tagged with its
source. When instances route through llmproxy, llmproxy numbers win for those instances
and local-log scans are shown as a cross-check, not summed with the ledger.

## USG-09 — API-key pollers for the overlap table

Repo: superai. Size: M (6–8 change sets). Depends on subplan 14's USG-07; independent
of llmproxy.

HTTP pollers for providers whose usage truth is an authenticated endpoint and whose
auth material is an API key in config/env (no cookies, no Keychain, no browser): from
the verified table, OpenRouter, DeepSeek, Zed cloud, Warp GraphQL, Factory, Kilo api
mode, MiniMax, Mistral, ClinePass (API-key mode; its session-file mode reads
`~/.cline/data/settings/providers.json` read-only the way CodexBar does, per
docs/clinepass.md). Each is a registry entry plus a shared poller that renders
`UsageSnapshot`-equivalent output. Execution follows the probe discipline from subplan
07 PRV-07 (explicit invocation, bounded time and size, redacted failures, no retry
storm, no background polling in core). MiniMax keeps its https-host-override policy
ported as a validation rule (docs/minimax.md documents the strict-host mode).

Cookie- and localStorage-backed providers (Qwen Cloud, Kimi, Qoder, Windsurf, Devin,
Command Code, Copilot web budgets) are explicitly not ported in this subplan; see
Out of scope.

## LNG-01 — long-tail provider onboarding pattern

Repo: superai. Size: M (6–8 change sets), then S per provider batch. Depends on
subplan 14's USG-07.

Mirrors `docs/harness-configs/`: one research doc per provider at
`docs/usage-providers/<provider>.md`, written from the provider's own artifacts and
official docs (CodexBar's `docs/<provider>.md` files are the starting index, not the
authority), recording:

- where usage truth lives (artifact paths or endpoints, with exact shapes),
- auth material kind and its file/env location (never values),
- window strategy and reset semantics,
- pricing source and units,
- verification date and upstream links.

Code side, per provider: a registry data entry (USG-07), a reader or poller wiring to
an existing kind, fixtures captured from real artifacts (redacted), and property tests
over the parser. A provider needing a genuinely new wire format adds a parser module;
one that fits an existing kind adds zero Rust. Onboarding order follows the overlap
table top-down (Codex and Claude cost engines first, then API-key pollers, then local
SQLite readers), because those rows have verified, credential-free or API-key sources.

The CodexBar table's unmapped providers (OpenRouter, Groq, DeepInfra, and the rest
listed in subplan 14) onboard the same way as provider-native sources for instances
that use them, using plan 07 provider ids as the join key.

## Tests

- Cross-check: for instances routed through llmproxy, local-log cost and ledger cost
  are both reported with a divergence line, never silently merged.
- Poller failures and responses follow the PRV-07 discipline: bounded, redacted, no
  retry storm; fixtures come redacted from real artifacts per LNG-01.

## Exit gate

- [ ] `superai usage` reads a deployed llmproxy's `/usage/summary` and
      `/usage/requests` with the admin token from an env var or 0600 file, merges rows
      into `usage show` with source tags, and lets ledger numbers win for routed
      instances with local scans shown as a cross-check.
- [ ] API-key pollers cover the verified overlap-table list under the PRV-07 probe
      discipline, with MiniMax's https-host-override policy ported as a validation
      rule.
- [ ] docs/usage-providers/ exists with an entry for every onboarded provider, each
      carrying a verification date.

## Out of scope

- Cookie- and localStorage-backed providers (Qwen Cloud, Kimi, Qoder, Windsurf, Devin,
  Command Code, Copilot web budgets): not ported here; subplan 14's Out of scope
  carries the full exclusion and its security-review caveat.
- Browser cookie importers, macOS Keychain access, OAuth login flows: excluded for the
  same reasons as subplan 14.
- Background polling or refresh loops: the PRV-07 rule holds for pollers too.

## Unverified and excluded

The llmproxy `/usage` response schemas are assumed, not verified: PRX-03 has not
shipped, so the fixtures in `crates/superai-usage/tests/fixtures/llmproxy/` are
the baseline the client parses, not captures from a live ledger. Checking them
against a deployed PRX-03 is owed when it ships; the punt is tracked as an
`#[ignore]`d repro in `crates/superai-usage/tests/abuse_punted_leaks.rs`.
Otherwise nothing new beyond subplan 14's list. CodexBar's per-provider docs
stay a starting index, not the authority; every onboarding doc records its own
verification date.

## Open questions

- Are the China-line providers (Alibaba, Doubao, Qwen's Team endpoints, Moonshot) wanted
  early enough to schedule in LNG-01's first batch, or after the API-key pollers?
