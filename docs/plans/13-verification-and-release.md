# Subplan 13 — verification, quality gates, and non-UI release

Parent: [master plan](master-plan.md)  
Task prefix: QAL  
Estimate: 10–16 reviewable change sets

## Outcome

Release claims are backed by fixtures, property/fuzz/mutation/failure/platform tests, security
checks, and a complete goal-to-behavior ledger.

## Test architecture

### QAL-01 — isolated test filesystem

Replace shared deterministic temp paths with per-test unique directories.

Requirements:

- Parallel-safe.
- Automatic cleanup with retained-on-failure option.
- No process-global cwd or HOME mutation.
- Platform path fixtures passed explicitly.
- Permission/symlink/hardlink tests feature-gated by platform capability.

### QAL-02 — harness fixture corpus

For every adapter/version:

- minimal valid config.
- realistic config with unmodelled keys/comments/order.
- malformed/truncated config.
- previous/current schema boundary.
- default and isolated path layouts.
- skills/plugins/MCP variants.
- sanitized wrapper.
- detection/version output.

Fixtures contain obvious fake credentials and pass secret scanners. Source/version/provenance is
recorded beside fixture.

### QAL-03 — unit and property tests

Properties:

- No-op round-trip byte identity.
- Intended selector changes, unrelated values survive.
- Registry serialization contains no forbidden values.
- Preview deterministic for same snapshot/input.
- Commit result matches preview or aborts on conflict.
- Restore returns exact pre-write bytes/permissions.
- Name/path normalization is collision-safe.
- Capability matrix complete.

Use property-based generation for nested documents, names, paths, and operation sequences where
crate choice passes dependency verification.

### QAL-04 — parser fuzzing

Fuzz targets:

- JSON/JSONC/TOML/YAML/env loads.
- selector/edit application.
- adapter version/schema detection.
- wrapper parser for generated grammar.
- provider/template deserialization.
- registry migration.

Assertions:

- no panic, hang, unbounded allocation, path escape, or secret leak.
- output re-parses when operation succeeds.
- rejected input causes no filesystem mutation.

Seed corpus comes from all harness fixtures and malformed cases.

### QAL-05 — mutation testing

Use cargo-mutants:

- PR/diff-scoped for changed core/config logic where runtime permits.
- Scheduled sharded full runs.

Prioritize:

- backup-before-write guards.
- conflict checks.
- rollback branches.
- ownership/deletion checks.
- secret redaction.
- template three-way resolution.
- capability resolution.

Do not write tautological tests to kill mutants. Skip equivalent mutants with reason.

### QAL-06 — failure and crash injection

Execute failure matrix from subplan 02 across:

- single-file config.
- multi-file instance creation.
- template update.
- bulk skill/MCP update.
- wrapper replace.
- daemon start.

Simulate abandoned journal at each phase and verify next-run recovery result.

### QAL-07 — fake process/network harness

Process fixtures:

- version output variants.
- timeout/signal/non-zero/huge output.
- install success with wrong binary/version.
- daemon readiness and unrelated PID.

HTTP fixtures:

- GitHub catalog/template success, digest mismatch, redirect, rate limit, timeout, oversized body.
- Health auth/rate-limit/TLS-like error classification.
- Cross-host redirect header stripping.

Tests remain deterministic and do not need live providers.

## Platform matrix

### QAL-08 — CI platforms

Required:

- Linux latest stable.
- macOS latest available runner.
- Windows latest stable.

Each runs:

- cargo fmt --all -- --check.
- cargo clippy --workspace --all-targets --all-features -- -D warnings.
- cargo test --workspace --all-features.
- cargo build --locked.
- relevant adapter/path/wrapper tests.

Linux retains supply-chain job:

- cargo hack check --workspace --each-feature --no-dev-deps.
- cargo shear --deny-warnings.
- cargo deny check.

Pin exact Rust toolchain already defined by repository.

### QAL-09 — filesystem/platform cases

- Atomic replace semantics.
- CRLF.
- case-insensitive collision simulation/real platform.
- Windows reserved names and long paths.
- symlink privileges/junction behavior.
- file permissions/ACL limitation reporting.
- macOS application paths.
- shell quoting for POSIX and PowerShell/cmd.
- locked/open files on Windows.

Support claims are per platform, not generalized from Linux.

## Security verification

### QAL-10 — secrets

Inject unique sentinels through every API-key/header/env path, then scan:

- registry and superai-owned metadata.
- operation preview/result/errors.
- logs and snapshots.
- journal and backup catalog.
- wrapper content.
- test output.

Expected secret locations are limited to harness-required config/env and its content backup.
Backups inherit restrictive permissions.

### QAL-11 — path/process/network abuse

Test:

- traversal and absolute paths from templates.
- symlink swap race.
- broad deletion targets.
- shell metacharacters in names/paths.
- malicious binary version output.
- template URLs/redirects to unsupported/private targets.
- huge/deep config and decompression/package traps.
- plugin/skill symlink escape.
- unrelated PID reuse.

### QAL-12 — dependency/supply chain

For every new dependency:

- crates.io spelling and owners.
- repository/activity/license.
- build.rs/proc-macro review.
- lockfile diff.
- cargo deny/advisory result.

No dependency added solely to avoid a small clear standard-library implementation unless its
correctness/security value is documented in review.

## Coverage and documentation

### QAL-13 — goal coverage test

Maintain machine-readable or test-validated ledgers for:

- every harness surface.
- every adapter operation/support state.
- every active provider/template capability entry.
- every non-UI definition-of-done item.

CI fails when a new harness research file lacks ledger entry.

### QAL-14 — source freshness

Adapter/provider/template data carries verified date and version range.

Before release:

- Recheck active/previews with highest schema churn.
- Recheck all source-marked research gaps.
- Reclassify sunset/archived products.
- Do not silently widen version range after docs change.

### QAL-15 — user-facing docs

Update final-state docs only:

- README status/capabilities/limitations.
- Supported harness matrix with state/version/platform.
- Backup/recovery and data locations.
- Security/no-vault/no-OAuth behavior.
- Template repository configuration.

Avoid implementation diary, design self-report, or UI promises.

## Release gates

### Filesystem foundation release gate

- Codecs, mutation, backup/restore, representative adapters, and skills filesystem ops pass
  property/failure/platform tests.

### Core beta gate

- Instance lifecycle, discovery/drift, wrappers, providers/templates/capabilities, MCP/plugins, and
  install workflows pass.
- Research-blocked adapters cannot write.
- Migration-only adapters cannot create defaults.

### Non-UI completion gate

- Master plan definition of done complete.
- Full CI and extended checks green.
- Parser fuzz budget completed with no unresolved crash.
- Mutation score reviewed; surviving safety mutants resolved/justified.
- Recovery drill restores configs after injected multi-file failure.
- Secret scan has no unexpected location.
- All 48 product-surface ledger rows accounted for.

## Standard commands

Run after every change:

    cargo fmt --all
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace --all-features

Before release:

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace --all-features
    cargo build --locked
    cargo hack check --workspace --each-feature --no-dev-deps
    cargo shear --deny-warnings
    cargo deny check

Fuzz/mutation commands use pinned documented tool versions in CI once added.

## Exit gate

- [ ] Tests cover every supported adapter/platform state.
- [ ] Fault/crash recovery evidence exists.
- [ ] Fuzzing and mutation testing guard parsers/safety branches.
- [ ] Secret/path/process/network abuse suites pass.
- [ ] Dependency audit and lockfile discipline pass.
- [ ] Goal/harness/provider/template ledgers are complete.
- [ ] Non-UI release claims match observable behavior.
