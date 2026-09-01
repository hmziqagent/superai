# Subplan 05 — discovery, adoption, ownership, and drift

Parent: [master plan](master-plan.md)  
Task prefix: DRF  
Estimate: 7–10 reviewable change sets

## Outcome

superai finds installed binaries, default and alternate config roots, wrappers, and stale records;
classifies ownership; reports drift; and offers safe adoption, detach, quarantine, or repair plans.

## Drift categories

Do not collapse all findings into unmanaged:

- RecordedHealthy.
- RecordedConfigMissing.
- RecordedBinaryMissing.
- RecordedWrapperMissing or WrapperChanged.
- RecordedVersionUnsupported.
- DefaultUnrecorded.
- CandidateUnmanaged.
- ForeignManaged.
- AmbiguousOwnership.
- OrphanWrapper.
- DuplicateRoot or DuplicateWrapper.
- FixedPathProfileInactive.
- DaemonStopped, PortConflict, or ProcessIdentityMismatch.

## Work packages

### DRF-01 — discovery roots

Inputs:

- Adapter default paths.
- Adapter glob/prefix candidate rules.
- XDG/platform application directories.
- Config-root relocation hints from wrappers.
- User-specified scan roots.

Limits:

- No unrestricted home-directory crawl.
- Bounded depth, entry count, size, and time.
- Skip permissions errors with diagnostics.
- Never parse known secret stores to identify a harness.

### DRF-02 — config-root fingerprinting

Adapter fingerprints use multiple signals:

- canonical filenames and schema keys.
- path pattern.
- version marker.
- adjacent state layout.
- matching binary/app install.

Return confidence and evidence. A directory name alone cannot establish harness or ownership.

### DRF-03 — wrapper discovery

Scan configured wrapper directories only.

Recognize:

- superai marker/version/digest.
- exec target.
- env/config-root assignments.
- user-owned wrappers matching known recipes.
- aliases or shims where inspectable.

Never execute an unknown wrapper during scan. Shell parsing is bounded to generated grammar;
otherwise report opaque wrapper.

### DRF-04 — foreign-manager ownership

Build explicit detectors for:

- claude-multi record paths and wrapper/config associations.
- package-manager/mise shims versus instance wrappers.
- orchestrator-managed profiles where local evidence exists.
- generic ownership marker files.

Rules:

- superai does not import claude-multi.
- A config root/wrapper linked by another manager becomes ForeignManaged.
- Shared default paths can be managed targets without claiming exclusive ownership.
- Ambiguous evidence blocks adopt/remove.

### DRF-05 — registry reconciliation

Compare fresh discovery with registry:

- Match stable InstanceId marker first.
- Then exact normalized config root.
- Then exact owned wrapper metadata.
- Never merge records based only on user-facing name.

Produce a read-only drift report with recommended operations and risks.

### DRF-06 — adoption

Adoption request requires exact candidate and intended ownership:

1. Re-inspect candidate and prove harness/version.
2. Verify no foreign ownership.
3. Read config fresh.
4. Decide default/constrained/fixed/isolated state.
5. Optionally create superai wrapper without changing config.
6. Record origin adopted and observed paths.

Adoption does not copy, migrate, normalize, or reformat harness config.

### DRF-07 — orphan handling

Orphan config root choices:

- Ignore.
- Adopt.
- Quarantine only if caller explicitly requests and ownership is unmanaged.

Orphan wrapper choices:

- Ignore.
- Adopt/record.
- Repair to a chosen instance.
- Quarantine if target/digest prove safe.

The scan itself never offers a generic recursive delete path.

### DRF-08 — drift scan API

Return:

- timestamped scan scope.
- findings grouped by harness/instance.
- evidence/confidence.
- current adapter support/version.
- risk level.
- available next operations.

No UI formatting. Stable data supports future GPUI/TUI/CLI.

## Tests

- Goal examples .claude-aaa, .claude-abogo, .claude-claude-g2, .claude-tester appear as
  unmanaged only when fixtures have no record/wrapper/foreign manager.
- claude-multi-associated root is ForeignManaged and adoption/removal is blocked.
- User wrapper with same command name does not become superai-owned.
- Missing record path and moved binary are distinct.
- Symlinked roots deduplicate by file identity without losing display path.
- Scan bounds prevent huge-tree traversal.
- No scan mutates access time/content where platform permits read-only access.

## Exit gate

- [x] Candidate discovery is bounded and adapter-driven.
- [x] Foreign ownership has evidence and blocks mutation.
- [x] Adoption is record-first and config-preserving.
- [x] Orphan handling is explicit/recoverable.
- [x] Drift report covers config, binary, wrapper, version, fixed-path, and daemon state.

> Completion record (run 3): DRF-01..08 closed by area 6b (judge-verified: ScanOptions user
> roots/wrapper dirs with deadline + surfaced permission diagnostics + glob expansion;
> version-marker/binary-adjacency signals; wrapper-dir scan; mise/asdf shim detection with
> typed AmbiguousOwnership blocking adopt/register/quarantine; marker-first reconciliation;
> OrphanWrapperChoice + quarantine_unmanaged_root; 15-category drift report with risk and
> next-operations — `drift_report_groups_by_harness_with_risk_and_next_ops`). DRF-06 adoption
> was already implemented at baseline (12 tests).

