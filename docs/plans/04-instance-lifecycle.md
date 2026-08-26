# Subplan 04 — instance lifecycle

Parent: [master plan](master-plan.md)  
Task prefix: INS  
Estimate: 8–12 reviewable change sets

## Outcome

Core orchestrates default, mirrored, isolated, adopted, detached, updated, and removed instances
through previewable compensated transactions. Harness-owned state is always read through adapter
at operation time.

## Instance principles

- Instance is named harness setup, not provider record or binary install.
- Default existing install is first-class.
- One binary install may serve many relocated instances.
- Fixed-path product may expose only one active instance/profile.
- Instance record never mirrors model, base URL, key, skills, plugins, or MCP content.
- Create starts from working source, then isolates.

## Work packages

### INS-01 — inspect/default registration

Given HarnessId:

1. Detect binary/app/version.
2. Resolve default config target.
3. Inspect config fresh.
4. Determine whether already recorded or foreign-managed.
5. Preview record creation without changing harness files.
6. Commit registry only.

Do not create missing default config by inspection. A missing config may still be a detected
needs-auth/default target.

### INS-02 — creation request and preflight

Request:

- new name.
- harness.
- source instance/config root.
- target isolation strategy.
- optional template and provider input.
- wrapper target/name.
- asset inheritance choices only where adapter permits exclusions.

Preflight:

- validate names/paths/collisions.
- source exists and readable.
- target absent or empty/owned.
- harness supports chosen isolation.
- disk space and permissions.
- template/provider compatible.
- planned secret sink valid.
- no daemon port conflict.
- no foreign manager ownership.

### INS-03 — mirror plan

Adapter supplies include/exclude policy:

- Include user-editable settings, permissions, instructions, skills/plugins/MCP, and other
  reusable setup required for working behavior.
- Exclude sessions/history/logs/caches/locks/telemetry IDs/temp files and device-local transient
  state unless goal explicitly requires them.
- OAuth/keychain credentials stay external unless harness stores them inside relocated root and
  adapter has verified safe copying. Default is needs-auth.
- Preserve modes and permissions unless template explicitly owns a field.

Preview lists every copied, linked, skipped, transformed, and externally-authenticated resource.

### INS-04 — isolate and configure

Transaction order:

1. Create target root.
2. Copy mirror according to plan.
3. Apply template/provider mutations to target only.
4. Install/link selected shared assets.
5. Generate wrapper or activation artifact.
6. Validate target using adapter.
7. Probe launch safely where supported.
8. Add registry record last.

Failure before registry commit rolls target/wrapper back or quarantines exact residuals.

### INS-05 — rename

Rename can affect:

- InstanceName.
- wrapper command/path.
- display labels in superai-owned files.

It does not rename config root automatically. Root relocation is separate move workflow with
adapter validation. Rename collision checks are platform-aware and wrapper replacement is atomic.

### INS-06 — reconfigure

Provider/template/skill/plugin/MCP changes:

- Load instance record.
- Re-inspect harness files fresh.
- Build adapter mutations.
- Preview semantic/lexical diffs.
- Commit through transaction layer.
- Re-resolve capabilities and health without persisting mirrors.

Registry changes only for superai-owned provenance/version facts after file verification.

### INS-07 — detach

Detach removes registry and optionally owned wrapper, leaving harness config/root untouched.

Use for:

- handing instance to another manager.
- keeping an isolated setup without superai tracking.
- recovering from missing binary while retaining data.

Preview clearly distinguishes wrapper removal from config retention.

### INS-08 — remove

Separate choices:

- record only.
- record plus wrapper.
- record/wrapper plus superai-created instance root.
- fixed-path config entries only.
- binary uninstall, delegated to subplan 11.

Adopted/default/foreign roots are never recursively removed under generic instance removal.
Created roots use quarantine and recoverability from subplan 02.

### INS-09 — repair

Detect and plan repair for:

- missing wrapper.
- moved binary.
- missing config root.
- wrapper content drift.
- adapter version change.
- incomplete transaction journal.
- template version record ahead/behind actual verified update.

Repair never overwrites a changed wrapper unless ownership/content digest proves it is
superai-created or caller explicitly adopts it.

### INS-10 — fixed-path and daemon lifecycle

Fixed-path:

- Multiple saved profiles may be superai-owned, but only one active harness path.
- Activation swaps via backed-up verified transaction.
- Active identity is derived from content/provenance, not assumed registry flag.
- Concurrent activation is locked/conflict checked.

Daemon:

- Record service identity, port allocation, pid/readiness mechanism, config root, wrapper/control
  command.
- Start/stop are explicit process actions.
- Remove stops only verified owned process/service.
- Stale pid files never authorize killing a process without identity verification.

## Observable lifecycle tests

- Register an existing default install without touching config.
- Mirror source, change provider in target, prove source bytes unchanged.
- Failure after target copy leaves no false registry record.
- Failure after wrapper creation rolls wrapper/root back.
- Rename preserves instance ID and target root.
- Detach leaves target bytes intact.
- Remove adopted/default target refuses data deletion.
- Fixed-path activation creates backups and restores prior profile.
- Daemon port conflict aborts before start.
- Reconfigure sees an external edit made after prior inspection.

## Exit gate

- [ ] Default, created, mirrored, adopted, fixed-path, and daemon flows exist.
- [ ] Record is committed only after target verification.
- [ ] Source instance cannot change during mirror/create.
- [ ] Remove choices are distinct and recoverable.
- [ ] Repair is ownership-aware.
- [ ] Instance records remain free of harness-owned values/secrets.

