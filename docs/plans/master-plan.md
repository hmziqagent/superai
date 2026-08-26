# superai non-UI master implementation plan

Status: planning baseline  
Source baseline: repository at 2026-08-26  
Scope authority: [goal.md](../goal.md), [harness configuration index](../harness-configs/README.md), [AGENTS.MD](../../AGENTS.MD)

## 1. Purpose

This document is index, execution order, coverage ledger, and completion contract for every
non-interface requirement in goal.md.

It deliberately excludes:

- GPUI application, widgets, screens, navigation, styling, and interaction design.
- TUI and production CLI command design. Existing superai-cli remains a diagnostic placeholder.
- Chat, agent execution, model proxying, request routing, or wire-protocol translation.
- Cloud accounts, hosted state, telemetry service, secret vault, and OAuth implementation.
- Importing claude-multi records or taking ownership of another manager's instances.

Backend APIs needed by future interfaces remain in scope. Raw-editor parsing, validation, diff,
and commit services are included; editor UI is not.

## 2. Required end state

One local Rust binary has interface-independent services that can:

1. Detect installed harnesses and their versions without mutating them.
2. Read every supported config fresh from disk, resolve its version/schema, and preserve every
   unmodelled value plus supported comments/order/formatting.
3. Preview, back up, atomically apply, verify, restore, and audit file changes.
4. Represent existing default installs and named isolated instances without mirroring
   harness-owned model, URL, key, skill, plugin, or MCP values into superai records.
5. Find unmanaged config roots and wrappers, classify foreign ownership, then safely adopt or
   remove only with explicit caller intent.
6. Mirror an existing working instance into an isolated root, apply provider/template settings,
   create a user-chosen wrapper, and prevent state leakage back to its source.
7. Manage provider endpoints, models, health checks, and API-key placement through data-driven
   definitions. No provider addition requires Rust changes.
8. Fetch versioned template files directly from a configured GitHub repository, show update
   availability and semantic diffs, perform conflict-aware manual updates, and record applied
   template version only after verified success.
9. Resolve capability support as native, substituted, or absent for each harness/provider pair.
10. Install, update, enable, disable, and remove skills, plugins, and MCP definitions using each
    harness's supported mechanism.
11. Detect, install, update, and uninstall harness binaries through verified installer backends.
12. Expose raw JSON, JSONC, TOML, YAML, env, and supported text-config parse/validate/diff/commit
    operations without an interface dependency.

## 3. Existing foundation

Current code is a useful skeleton, not an empty repository:

| Area | Present | Missing before production use |
|---|---|---|
| Config | JSON and TOML fresh-load/edit/store; basic copy backup | JSONC/YAML/env; atomic commit; concurrency control; exact preservation contract; backup catalog/retention/integrity; multi-file rollback |
| Core | Instance/TemplateRef records; registry load/store; basic unmanaged-dir filter | schema migration; validation; discovery; ownership; lifecycle transactions; wrappers; templates; providers; skills/plugins/MCP; install |
| Capability | Four capabilities and three support states | data source, resolution, completeness validation, explanations |
| Interface | Registry-listing placeholder CLI | intentionally excluded |
| Quality | strict lint setup, tests, CI, supply-chain checks | filesystem fault tests, fixtures, mutation/fuzz/property testing, platform matrix |

Known foundation corrections are planned, not silently assumed:

- JSON key order preservation is not lexical preservation; current pretty-print rewrites layout.
- Current read-then-write can lose an external edit made between those operations.
- Current direct writes can leave a truncated file after interruption.
- Timestamp-only backup names can collide and no backup manifest/integrity check exists.
- Current unmanaged_dirs receives candidates; it does not discover or establish ownership.
- README status says nothing is built, but skeleton crates already exist.

## 4. Global invariants

Every subplan must preserve these invariants:

### 4.1 Ownership

- Harness-owned config stays authoritative on disk.
- Every operation loads it fresh. No long-lived parsed config or shadow config is consulted.
- Instance records contain only superai-owned facts: identity, paths, wrapper, lifecycle state,
  template identity/version, and explicit ownership/provenance.
- Foreign managers are detected and never modified, adopted, or removed implicitly.

### 4.2 Mutation safety

- Preview and commit use the same deterministic mutation plan.
- Before each write to a file superai did not create, create and verify a recoverable backup.
- Commit uses same-filesystem temporary files, flush, permission preservation, atomic replacement,
  directory sync where supported, then parse/read-back verification.
- Detect a changed source between planning and commit; abort with conflict, never overwrite it.
- Multi-file operations either complete and verify or roll back completed writes.
- Symlinks, hard links, path traversal, case-insensitive collisions, wrapper collisions, and
  Windows/macOS/Linux path differences receive explicit policies and tests.

### 4.3 Honesty

- Unknown harness version or incomplete schema means read-only discovery until compatibility is
  proven.
- Unsupported isolation is represented as constrained or single-instance, never presented as
  full isolation.
- Auto-managed SQLite/keychain/internal state is not edited as if it were user config.
- Executable config such as crushrc is never parsed or rewritten as ordinary data.
- Deprecated/retired harnesses remain discoverable and recoverable, but do not drive new-instance
  defaults.

### 4.4 Secrets

- No superai secret store, credential DB, OAuth client, or keychain integration.
- API-key input is ephemeral and written only to a harness-supported credential sink or
  instance-owned env/config file required by that harness.
- Instance/template/provider records never contain secret values.
- Logs, diffs, errors, health results, backups indexes, and debug output redact secret values.
- OAuth and subscription login remain harness-owned external steps.

### 4.5 Architecture

- superai-config knows files/documents, not instances or interface concerns.
- superai-core owns domain workflows and depends downward on superai-config.
- Interface types never leak into either library.
- Harness identity is allowed inside adapter/core internals. Consumers ask resolved capabilities,
  not harness-specific questions.
- Public items have one canonical path; artificial manager/factory layers are avoided.

## 5. Subplan index

IDs are stable references for issues, branches, tests, and release notes.
Estimate uses reviewable change sets, not calendar promises.

| Order | Subplan | Main output | Depends on | Estimate |
|---:|---|---|---|---:|
| 00 | [Domain foundation](00-domain-foundation.md) | IDs, records schema, validation, transactions, ports | — | 5–7 |
| 01 | [Document engine](01-document-engine.md) | Loss-minimizing codecs and typed path edits | 00 | 8–12 |
| 02 | [Safe mutation and backups](02-safe-mutation-and-backups.md) | Conflict-safe atomic commits, backup/restore, rollback | 00, 01 | 8–11 |
| 03 | [Harness adapters](03-harness-adapters.md) | Version-aware adapter contract and all 48 planned surfaces | 00–02 | 50–70 |
| 04 | [Instance lifecycle](04-instance-lifecycle.md) | Create/mirror/adopt/update/remove orchestration | 00–03 | 8–12 |
| 05 | [Discovery, adoption, drift](05-discovery-adoption-drift.md) | Install/config/wrapper scans and ownership classification | 00, 03, 04 | 7–10 |
| 06 | [Wrappers and isolation](06-wrappers-and-isolation.md) | Portable wrapper planning/generation/verification | 00, 02–05 | 7–10 |
| 07 | [Providers and health](07-providers-and-health.md) | Data-driven providers, model catalog, safe probes | 00–04 | 7–10 |
| 08 | [Templates and versioning](08-templates-and-versioning.md) | Direct GitHub fetch, compare, three-way update | 00–04, 07 | 8–12 |
| 09 | [Capabilities](09-capabilities.md) | Complete harness/provider capability resolution | 00, 03, 07, 08 | 4–6 |
| 10 | [Skills, plugins, MCP](10-skills-plugins-mcp.md) | Registry plus per-instance link/copy/config workflows | 00–06 | 10–14 |
| 11 | [Harness installation](11-harness-installation.md) | Detect/install/update/uninstall with mise/duct-backed execution | 00, 03–06 | 8–12 |
| 12 | [Raw editor backend](12-raw-editor-backend.md) | Read/validate/diff/commit API for future editors | 01–03 | 4–6 |
| 13 | [Verification and release](13-verification-and-release.md) | Fixtures, fault tests, platform gates, non-UI release criteria | all | 10–16 |

Expected total: roughly 144–208 reviewable change sets, dominated by one adapter/fixture set per
documented harness surface. Parallel work becomes safe only after subplans 00–03 freeze contracts.

## 6. Execution roadmap

### Milestone A — contracts frozen

Complete 00. Approve durable IDs, path/provenance types, registry schema v1, error taxonomy,
operation preview/result types, feature-support states, and platform boundary traits.

Exit:

- Existing records migrate or fail with actionable errors.
- Invalid names/paths/template refs cannot enter workflows.
- No interface or harness-specific types leak into public orchestration requests.

### Milestone B — filesystem layer becomes boring

Complete 01 and 02. Implement codecs in increasing risk order:

1. TOML and strict JSON.
2. JSONC and env.
3. YAML.
4. Supported line/fragment config.
5. Explicit command-backed or opaque surfaces.

Exit:

- Fresh-read, foreign-key/comment/order preservation, conflicts, atomic replacement, backup,
  restore, rollback, permissions, and injected failures are tested.
- Unsupported constructs fail before write.
- Config mutations survive interruption tests without corrupting original or backup.

### Milestone C — adapter spine plus representative harnesses

Freeze adapter contract in 03, then implement representative vertical slices:

1. Claude Code: JSON, config-root relocation, default install, skills/plugins/MCP.
2. Codex CLI: TOML, CODEX_HOME, profiles, skills/MCP.
3. Aider: YAML/env/explicit paths.
4. OpenCode or Kilo: JSONC and layered configuration.
5. Cline: IDE user-data isolation.
6. ZCode: version-in-path, fixed-path single-instance behavior.
7. OpenClaw: daemon/port allocation.
8. Crush: executable-config safety and command-backed/managed-fragment decision.

Exit:

- Each isolation/config class has one end-to-end fixture-backed adapter.
- Version mismatch and research-blocked states are observable.
- Adapter conformance suite is reusable for remaining harnesses.

### Milestone D — filesystem-feature completion across harnesses

Finish 03 plus filesystem portions of 10:

- Config edit/remove/restore for every writable adapter.
- Skill registry link-all, link-one, copy-one, update, disable, remove.
- MCP/plugin file operations only where researched and safe.
- Retired, preview, managed-backend, and incomplete surfaces get explicit limited support.

Exit:

- Every row in harness-configs has a support record and tests.
- No row is silently dropped because isolation/provider support is awkward.
- Research gaps are tracked as blockers, not implemented from inference.

### Milestone E — instance workflows

Complete 04, 05, and 06:

- Default install registration.
- Mirror-then-isolate creation.
- Wrapper create/rename/repair/remove.
- Discovery, ownership, unmanaged state, adoption, detach, safe removal.
- Daemon and fixed-path activation semantics.

Exit:

- Existing install, new isolated instance, adopted instance, foreign-managed config, orphan
  wrapper, and single-instance target all pass observable lifecycle tests.

### Milestone F — providers, templates, capabilities

Complete 07, 08, and 09:

- Provider data files and model/health schemas.
- Direct GitHub template fetch and validation.
- Update availability, old-to-new diff, local override conflict detection, manual apply.
- Capability completeness and resolved support/explanation.

Exit:

- Adding a provider/template needs data changes only.
- Failed update never advances instance template version.
- Capability callers never need a harness switch statement.

### Milestone G — install and backend completion

Complete 10–12:

- Skills/plugins/MCP lifecycle.
- Harness detection/install/update/uninstall.
- Raw editor parse/validation/diff/commit services.

Exit:

- All backend use cases are interface-neutral and exercised through tests/examples.
- No production CLI/TUI/GPUI behavior was added.

### Milestone H — release evidence

Complete 13. Run full platform/fixture/failure suite, security and supply-chain gates, mutation
testing, parser fuzzing, and recovery drills.

Exit:

- Non-UI definition of done in section 10 passes.
- Remaining limitations are adapter support states with evidence, not hidden TODOs.

## 7. Dependency flow

Domain contracts → document engine → safe mutation → adapters

Adapters → instance lifecycle → discovery/drift → wrappers

Adapters + instance lifecycle → providers → templates → capabilities

Safe mutation + adapters + lifecycle → skills/plugins/MCP

Adapters + lifecycle + wrappers → harness installation

Document engine + safe mutation + adapters → raw editor backend

All streams → verification/release

No provider/template/capability work may bypass the safe mutation boundary. No lifecycle workflow
may write directly through std::fs when a config transaction is required.

## 8. Harness delivery policy

The research corpus has 44 harness files representing 48 planned product surfaces because
copilot-cli.md covers local and cloud agents, kimi-cli.md covers current and legacy products,
while orchestrators.md covers Vibe Kanban, Conductor, and Sculptor.

Each surface receives one explicit state:

| State | Meaning |
|---|---|
| Full | Supported config and isolation mechanism verified for installed version |
| Constrained | Useful management exists, but identity/state isolation has documented limits |
| SingleInstance | Fixed/shared path; active profile swapping or in-place mutation only |
| ReadOnly | Detect/read/diff only; safe write contract not established |
| MigrationOnly | Retired/archived predecessor; discover, back up, export/migrate, no new defaults |
| ResearchBlocked | Source docs/fixtures insufficient; no writes |
| Unsupported | Technically impossible or outside goal, with evidence and recovery guidance |

Adapter completion requires:

- Binary/version detection.
- Platform path resolution and config precedence.
- Surface ownership: user-editable, harness-managed, superai-owned, or external secret/keychain.
- Version/schema range.
- Parse/mutation mode.
- Isolation class and wrapper recipe.
- Provider/model/API-key mapping or explicit absence.
- Skill/plugin/MCP mapping or explicit absence.
- Default install and candidate-root discovery.
- Backup/restore and remove semantics.
- Golden fixtures plus conformance tests.
- Research source links and last verification date.

Detailed ledger and waves live in [03-harness-adapters.md](03-harness-adapters.md).

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

All items must be true:

- [ ] Every goal.md non-interface sentence maps to implemented behavior or explicit unsupported
      state in section 9.
- [ ] All 48 planned surfaces have adapter support records; none are omitted.
- [ ] Every supported write reads fresh, checks conflict, backs up, atomically commits, and
      verifies.
- [ ] Failure injection proves multi-file rollback and backup restoration.
- [ ] Instance records contain no harness-owned config value or secret.
- [ ] Default installs, named instances, foreign-managed roots, orphans, daemon targets, and
      fixed-path targets have lifecycle tests.
- [ ] Provider and template additions are data-only.
- [ ] Template update preview shows old defaults, new defaults, local divergence, and conflicts.
- [ ] Skills support link-all, link-one, copy-one, update, disable, remove where harness allows it.
- [ ] Plugin/MCP support is adapter-specific and preserves foreign entries.
- [ ] Harness install/uninstall never removes user data without separate explicit intent.
- [ ] Raw editor backend rejects invalid content without touching disk.
- [ ] Secret values never appear in records, normal diagnostics, snapshots, or test logs.
- [ ] No interface crate dependency exists below layer 3.
- [ ] No model proxy, wire translator, chat runtime, OAuth client, or secret vault exists.
- [ ] Formatting, clippy, tests, locked build, feature checks, dependency audit, parser fuzzing,
      mutation tests, and supported-platform tests pass.

## 11. Planning decisions that require evidence during implementation

These are deliberate research gates, not invitations to guess:

1. Select loss-minimizing JSONC/YAML/env editing crates only after maintenance, license,
   round-trip, malformed-input, and comment-preservation spikes.
2. Verify toride's current modular crate names/APIs and mise/duct integration before dependencies
   are added.
3. Decide Crush support after testing its command API or a safe managed-fragment pattern;
   arbitrary Bash rewriting is forbidden.
4. Verify Antigravity, Warp, Kilo extension, Cursor credential, and IDE user-data relocation on
   each supported OS.
5. Complete DeepSeek Harness, Kiro, OpenClaw, ZCode, and other source-marked research gaps before
   enabling writes for those surfaces.
6. Define claude-multi ownership markers using observed files/wrappers; never infer ownership
   solely from a directory name.
7. Validate whether historical template versions remain immutable and fetchable. Without old
   base content, three-way updates are unavailable.

## 12. Change discipline

Each change set should:

1. Reference one or more stable task IDs from a subplan.
2. Add observable behavior tests before or with implementation.
3. Update adapter/support ledger when behavior or upstream schemas change.
4. Keep generated fixtures secret-free and platform-normalized.
5. Pass repository Rust quality gates.
6. Avoid unrelated interface work.

Subplan task boxes are completion records. Master milestone status changes only after each linked
subplan exit gate passes.
