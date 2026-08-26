# Subplan 02 — safe mutation, backups, restore, and rollback

Parent: [master plan](master-plan.md)  
Task prefix: MUT  
Estimate: 8–11 reviewable change sets

## Outcome

Every supported file change is conflict-aware, backed up when required, atomic, verified, and
recoverable. Multi-file workflows do not leave silent partial state.

## Safety model

Mutation has four phases:

1. Plan: fresh-read resources, validate, compute edits/diffs/preconditions.
2. Prepare: recheck metadata/digests, create backups, stage temp files, validate staged output.
3. Commit: atomically replace in deterministic order.
4. Verify: read fresh, parse, assert intended observable state; roll back on failure.

No caller can bypass these phases for harness-owned files.

## Work packages

### MUT-01 — resource snapshot and conflict token

Capture:

- resolved path.
- file existence/type.
- content digest.
- size, permissions, owner where available.
- modification and change metadata as hints, not sole identity.
- symlink target/chain policy.
- document kind/schema version.

Before backup and again before replacement, compare the source to the plan token. Any difference
returns ConcurrentModification with a new preview requirement.

### MUT-02 — path/link safety

Policies:

- Reject directories, devices, FIFOs, sockets, and unsupported special files.
- Default: follow an existing symlink only after resolving target within adapter-allowed roots;
  preserve link itself and mutate target.
- Detect symlink loops and changed targets between plan/commit.
- Detect multiple planned paths resolving to one inode/file identity.
- Hard-link behavior is explicit: warn that atomic replacement breaks link sharing or reject
  unless adapter policy allows it.
- Parent traversal and template-controlled absolute paths are rejected.
- Case-folded collisions are checked for target platform.

### MUT-03 — backup catalog

Backup before every write to a file superai did not create.

Backup entry:

- BackupId, operation ID, original path, backup path.
- timestamp plus collision-resistant suffix.
- original content digest, size, permissions.
- harness/instance/resource identity.
- reason and mutation kind.

Rules:

- Copy without following an unexpected changed link.
- Preserve permissions and relevant metadata where supported.
- Flush backup and verify digest before staging replacement.
- A failed backup aborts write.
- First creation records that no prior file existed; rollback removes only that newly created file.
- Catalog contains no file contents or secret values.

Retention:

- No automatic deletion in initial implementation.
- Listing/filtering supported.
- Later pruning requires explicit policy and never removes the last valid backup for a resource.

### MUT-04 — atomic single-file commit

Prepare a same-directory, same-filesystem temporary file:

1. Create with exclusive random name.
2. Apply safe permissions before secret-bearing bytes are written.
3. Write all bytes and flush.
4. Parse and semantically validate temp content.
5. Recheck original conflict token.
6. Atomically replace using platform-correct primitive.
7. Sync parent directory where supported.
8. Read fresh and verify digest/semantic assertions.

Never truncate/write original in place. Windows replacement, antivirus locks, read-only bits,
ACLs, and rename retry semantics need dedicated tests/policies.

### MUT-05 — multi-file transaction

Use for instance creation/update, skills, wrappers, and installs:

- Resolve full action graph before mutation.
- Sort/lock resources deterministically.
- Back up all foreign files before first commit.
- Stage and validate all file outputs.
- Commit in dependency order.
- Run post-commit process actions only after required files exist.
- On failure, restore committed files in reverse order.
- Verify rollback and report any residual state precisely.

No claim of filesystem-wide atomicity. Contract is compensated transaction with verified
rollback.

### MUT-06 — directories, copies, links, and permissions

Safe primitives:

- create directory with expected owner-only or normal mode.
- recursive copy with explicit include/exclude and symlink policy.
- create relative/absolute symlink per platform capability.
- replace symlink only if it matches expected owned target.
- remove owned empty directory.
- move material targets to recoverable quarantine before final delete.

Secret-bearing config/env files default owner-readable/writable only where platform supports it.
Do not chmod unrelated existing files beyond an adapter-declared security correction preview.

### MUT-07 — restore

Restore workflow:

1. Resolve backup by ID, not user-built path.
2. Verify backup digest and original-target relation.
3. Fresh-read current target and preview reverse diff.
4. Back up current target before restore unless it is a failed uncommitted creation.
5. Atomic replace and semantic verification.
6. Record restore operation linking both backup generations.

Restore never silently crosses harness/instance/resource identity.

### MUT-08 — removal and quarantine

Differentiate:

- Remove config entry from shared file.
- Delete superai-created wrapper/file.
- Detach registry record only.
- Remove instance root.
- Uninstall binary.

Material directory removal first moves exact validated target into a superai quarantine area on
same filesystem where possible. Report recoverability and retention. Broad roots, unresolved
variables, globs, home directories, workspace roots, and foreign-managed paths are invalid
deletion targets.

### MUT-09 — operation journal and crash recovery

Maintain a minimal superai-owned journal:

- operation ID, phase, resource IDs, staged temp paths, backup IDs, completed actions.
- no config contents and no secrets.

At next startup:

- detect abandoned operation.
- inspect actual filesystem state.
- finish verification or offer deterministic rollback.
- never blindly replay writes from stale content.

Journal removal happens only after verified completion/rollback.

## Integration changes

Current json::store, toml_file::store, backup, and restore become internal building blocks or are
replaced. Public callers receive transaction APIs; direct unsafe store paths are deprecated then
removed.

Registry writes use same engine even though registry is superai-owned; backup requirement may be
lighter, but atomic/conflict behavior is identical.

## Failure tests

Inject failure at every boundary:

- backup open/write/flush.
- temp create/write/flush.
- parse staged output.
- source changes after preview or backup.
- atomic replace.
- parent sync.
- read-back verification.
- second/third file in transaction.
- rollback replacement and rollback verification.
- process action after file commit.
- crash journal at every phase.

Observable assertions:

- Original remains valid or verified backup restores it.
- Foreign edits are never overwritten.
- Partial state is fully enumerated.
- Backup digest matches pre-write content.
- Secret sentinels never reach journal/errors.

## Exit gate

- [ ] All supported writes go through one transaction boundary.
- [ ] Backup-before-foreign-write is structurally unavoidable.
- [ ] Same-file conflicts abort.
- [ ] Single-file replacement is atomic per supported platform.
- [ ] Multi-file failure rolls back or reports verified residuals.
- [ ] Backup listing and restore work by stable IDs.
- [ ] Crash recovery tests pass.
