# superai non-UI master implementation plan

Status: subplans 00–13 are complete through commit c164204 (completion run 3, areas 1–9) plus the final-gate-fix round closing the five verifier-flagged narrow gaps (MUT-02 symlink follow-and-preserve policy, QAL-04 adapter-detection fuzz family, INS-02 asset-inheritance request field, DRF-04 orchestrator detector, WRP-07 foreground/shutdown-command spec) and the rename/drift consistency bug; 83/83 exit-gate items across those subplans are closed with residuals disclosed in each plan's completion record, and DoD 16/16 is evidenced by the tested coverage ledger (crates/superai-core/src/coverage_ledger.rs); subplans 14–16 (usage core, usage sources, VPS deploy) are open and being implemented; the six-group final re-verification remains the release gate for 00–13  

## What shipped

Subplans 00–13 are complete through commit c164204 plus the final-gate-fix round. The shipped backend covers the filesystem layer (fresh reads, loss-minimizing codecs, conflict-checked atomic commits, verified backups, multi-file rollback), version-aware adapters for all 48 cataloged surfaces with fixture corpora, and the workflows above them: instance lifecycle, discovery and drift, wrappers, data-driven providers and templates, capability resolution, skills/plugins/MCP, harness installation, and the raw editor backend. The 48 surfaces span 44 harness-config files (orchestrators.md covers Vibe Kanban, Conductor, and Sculptor; copilot-cli.md and kimi-cli.md each carry two surfaces); the detailed support-state ledger lives in [plan 03](03-harness-adapters.md). The definition of done is 16/16, machine-verified by the tested coverage ledger. For 00–13, the remaining work is the six-group final re-verification and the residuals each subplan discloses in its own completion record. Subplans 14–16 (usage core, usage sources, VPS deploy) are open and being implemented.

## Subplan exit gates (run 3)

| Order | Subplan | Main output | Depends on | Estimate | Exit gate (run 3) |
|---:|---|---|---|---:|---|
| 00 | [Domain foundation](00-domain-foundation.md) | IDs, records schema, validation, transactions, ports | — | 5–7 | 5/5 |
| 01 | [Document engine](01-document-engine.md) | Loss-minimizing codecs and typed path edits | 00 | 8–12 | 5/5 |
| 02 | [Safe mutation and backups](02-safe-mutation-and-backups.md) | Conflict-safe atomic commits, backup/restore, rollback | 00, 01 | 8–11 | 7/7 |
| 03 | [Harness adapters](03-harness-adapters.md) | Version-aware adapter contract and all 48 planned surfaces | 00–02 | 50–70 | 7/7 |
| 04 | [Instance lifecycle](04-instance-lifecycle.md) | Create/mirror/adopt/update/remove orchestration | 00–03 | 8–12 | 6/6 |
| 05 | [Discovery, adoption, drift](05-discovery-adoption-drift.md) | Install/config/wrapper scans and ownership classification | 00, 03, 04 | 7–10 | 5/5 |
| 06 | [Wrappers and isolation](06-wrappers-and-isolation.md) | Portable wrapper planning/generation/verification | 00, 02–05 | 7–10 | 5/5 |
| 07 | [Providers and health](07-providers-and-health.md) | Data-driven providers, model catalog, safe probes | 00–04 | 7–10 | 6/6 |
| 08 | [Templates and versioning](08-templates-and-versioning.md) | Direct GitHub fetch, compare, three-way update | 00–04, 07 | 8–12 | 6/6 |
| 09 | [Capabilities](09-capabilities.md) | Complete harness/provider capability resolution | 00, 03, 07, 08 | 4–6 | 5/5 |
| 10 | [Skills, plugins, MCP](10-skills-plugins-mcp.md) | Registry plus per-instance link/copy/config workflows | 00–06 | 10–14 | 7/7 |
| 11 | [Harness installation](11-harness-installation.md) | Detect/install/update/uninstall with mise/duct-backed execution | 00, 03–06 | 8–12 | 6/6 |
| 12 | [Raw editor backend](12-raw-editor-backend.md) | Read/validate/diff/commit API for future editors | 01–03 | 4–6 | 6/6 |
| 13 | [Verification and release](13-verification-and-release.md) | Fixtures, fault tests, platform gates, non-UI release criteria | all | 10–16 | 7/7 |
| 14 | [Usage core](14-usage-and-proxy.md) | UsageSurface, superai-usage crate, local readers, window/cost engines, `usage` CLI | 00–03, 07 | 11–15 | 0/5 |
| 15 | [Usage sources](15-usage-sources.md) | llmproxy ledger client, provider-native API-key pollers, onboarding pattern | 14 | 16–22 | 0/3 |
| 16 | [VPS deploy](16-vps-deploy.md) | `vps deploy/status llmproxy` via toride services, TLS or tunnel-only | 14, 15 | 6–9 | 0/3 |

Exit-gate column = subplan exit-gate checkboxes closed as of run 3 (commit c164204) plus the
final-gate-fix round; each plan's Exit gate section carries the evidence citations and the
residual notes (plan 02's single-transaction-boundary fold and plan 13's macOS/Windows
adversarial platform cases were closed by c164204; remaining residuals are disclosed inline
per plan, e.g. plan 10's unimplemented EXT-10 diagnostic-validation sub-step). Subplan task
boxes are completion records.

## External companion plans

Two export documents under `docs/plans/export/` carry the non-superai halves of the
usage work split out of subplan 14. Their canonical homes are the llmproxy and toride
repositories; the copies here are exports.

- [llmproxy-usage-ledger.md](export/llmproxy-usage-ledger.md): PRX-01..06, the
  proxy-side usage ledger (client keys, SQLite EventBus backend, /usage admin API,
  per-provider egress, release artifacts).
- [toride-vps-services.md](export/toride-vps-services.md): VPS-01, toride's remote
  exec/upload, systemd unit install, and the 0.2 publishes subplan 16 consumes.

## Milestone completion records

> Run-3 status: ACHIEVED — FND-01..07 verified implemented at the verification-run
> baseline (group-A verifier/judge); plan 00 exit gate 5/5.

> Run-3 status: ACHIEVED — DOC-02/08/09/10 closed by area 1, MUT gaps (incl. the §4.2
> conflict window of the original plan) closed by area 2, Windows semantics by area 8; production crash journal
> with startup recovery. One open exit-gate item: the type-enforced single transaction
> boundary (codec store paths remain public, backup-safe — plan 02 note).

> Run-3 status: ACHIEVED — spine adapters carry surface schemas, era-boundary fixtures and
> commit-time schema/era gates (area 3); all 48 catalog rows registered with fixture
> corpora (enforced); plan 03 exit gate 7/7.

> Run-3 status: ACHIEVED — 48/48 rows with support records and fixture corpora
> (test-enforced incl. the 13 formerly corpus-less surfaces), MCP/plugin declarations or
> explicit corpus-grounded absence on every adapter (areas 3+5); skill link/copy/config
> workflows completed (area 7).

> Run-3 status: ACHIEVED — plans 04/05/06 exit gates fully closed (areas 6a/6b,
> judge-verified): real reconfigure mutations, drift/orphan/report completion, wrapper
> invocation spec + POSIX/PowerShell/cmd launchers, fixed-path activation with reconcile
> choices, daemon lifecycle with pid-verified stop.

> Run-3 status: ACHIEVED — plans 07/08/09 exit gates fully closed (area 4, judge-verified
> rounds 1-2): provider rendering/inspection/lifecycle, real bounded probes, template
> replacement pointers + selector-reset policy, fresh data-driven capability resolution
> with InstanceId-keyed queries and use-path completeness gating.

> Run-3 status: ACHIEVED — plans 10/11/12 exit gates fully closed (areas 5+7,
> judge-verified): skills/plugins/MCP lifecycle incl. bulk and scope-transfer operations,
> install receipts/availability/pinning with typed external-install state, raw-editor
> open/validate/diff/create/rebase completion.

> Run-3 status: ACHIEVED — DoD 16/16 evidenced by the tested
> coverage ledger; ledgers/freshness machine-verified, mutation baseline recorded (610
> mutants, 50.2% caught, CI job continue-on-error), windows CI backlog closed, user docs
> completed (area 8). The formerly open macOS application-path and Windows adversarial
> long-path/locked-file platform cases were closed by area 9b (cfg-gated tests executed by
> the platform CI runners); mutation-score uplift and coverage-guided
> fuzz budget are recorded follow-ups.

## 9. Cross-plan coverage of goal.md

| Goal requirement | Owning subplans |
|---|---|
| Existing/default installs are managed targets | 03, 04, 05 |
| Instance records store only superai-owned facts | 00, 04 |
| Template version per instance | 00, 08 |
| Drift scan, adopt/remove, foreign-manager coexistence | 05 |
| Mirror existing then isolate | 04, 06 |
| Remote versioned templates; diff; manual updates | 08 |
| Native/substituted/absent capability matrix | 09 |
| Per-harness version-aware config | 01–03 |
| Fresh disk read; preserve unmodelled keys | 01, 02 |
| Backup before every foreign-file write | 02 |
| Wrapper generation; arbitrary user names | 06 |
| Every researched harness, including awkward ones | 03 |
| Provider endpoints/models/health/API keys, data-driven | 07 |
| Skill registry; whole/specific symlink or copy | 10 |
| Plugins and MCP servers from instance definition | 10 |
| Install/uninstall; existing-install detection | 11 |
| Raw TOML/JSON editors with validation | 12 |
| No vault and no OAuth | 00, 07, 13 |
| claude-multi coexistence; no import | 05 |
| Config → Core → Interface dependency direction | 00, 13 |
| Filesystem first, interface last | roadmap milestones A–H |
| No proxy/routing implementation | 00, 07, 13 |
| Rust/local/one binary | 00, 11, 13 |


## 10. Non-UI definition of done

- [x] Every goal.md non-interface sentence maps to implemented behavior or explicit unsupported
      state in the coverage ledger.
- [x] All 48 planned surfaces have adapter support records; none are omitted.
- [x] Every supported write reads fresh, checks conflict, backs up, atomically commits, and
      verifies.
- [x] Failure injection proves multi-file rollback and backup restoration.
- [x] Instance records contain no harness-owned config value or secret.
- [x] Default installs, named instances, foreign-managed roots, orphans, daemon targets, and
      fixed-path targets have lifecycle tests.
- [x] Provider and template additions are data-only.
- [x] Template update preview shows old defaults, new defaults, local divergence, and conflicts.
- [x] Skills support link-all, link-one, copy-one, update, disable, remove where harness allows it.
- [x] Plugin/MCP support is adapter-specific and preserves foreign entries.
- [x] Harness install/uninstall never removes user data without separate explicit intent.
- [x] Raw editor backend rejects invalid content without touching disk.
- [x] Secret values never appear in records, normal diagnostics, snapshots, or test logs.
- [x] No interface crate dependency exists below layer 3.
- [x] No model proxy, wire translator, chat runtime, OAuth client, or secret vault exists.
- [x] Formatting, clippy, tests, locked build, feature checks, dependency audit, parser fuzzing,
      mutation tests, and supported-platform tests pass.

> Run-3 completion record: all 16 items are machine-verified by the goal-coverage ledger
> (crates/superai-core/src/coverage_ledger.rs — every checkbox maps to named passing tests
> or committed artifacts; `cargo test -p superai-core coverage_ledger` green at c164204).
> Item 16's platform slice: linux/macos/windows/supply-chain CI jobs carry the matrix; the
> windows job is the execution-of-record for cfg(windows) semantics; the macOS/Windows
> adversarial platform-state families were closed by area 9b (commit c164204).

## References

Source baseline: repository at 2026-08-26  
Scope authority: [goal.md](../goal.md), [harness configuration index](../harness-configs/README.md), [AGENTS.md](../../AGENTS.md)
