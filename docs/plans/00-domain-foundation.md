# Subplan 00 — domain foundation

Parent: [master plan](master-plan.md)  
Task prefix: FND  
Estimate: 5–7 reviewable change sets

## Outcome

Stable, validated, serializable core contracts support every later workflow without storing
harness-owned state or importing interface concepts.

## Inputs

- goal.md ownership and layer rules.
- Existing Instance, TemplateRef, Registry, Capability, and Support types.
- Adapter diversity documented under harness-configs.

## Deliverables

- Durable identifiers and validated user-facing names.
- Registry schema v1 plus explicit migrations.
- Operation preview/result/error contracts.
- Isolation, ownership, lifecycle, support, and provenance enums.
- Narrow nondeterministic boundaries for filesystem, process, clock, network, and platform facts.
- Compatibility tests for current instance records.

## Domain model

### FND-01 — identifiers and names

Add newtypes with one canonical public path:

- HarnessId: stable lowercase slug independent of executable/display name.
- InstanceId: immutable generated identity; rename does not break references.
- InstanceName: user-chosen label and default wrapper command.
- TemplateId and TemplateVersion.
- ProviderId, CapabilityId, SkillId, PluginId, McpServerId.
- OperationId and BackupId.

Validation:

- Reject empty, dot/dot-dot, separators, NUL, control chars, reserved device names, trailing
  Windows dots/spaces, and names that normalize to an existing command.
- Preserve display name separately only where needed.
- Compare collisions using target platform semantics, including case folding.
- Do not force harness prefix. work and glm are valid wrapper names.

Tests:

- Unicode names where safe.
- macOS/Linux/Windows reserved and collision cases.
- Serialization round-trip and stable text forms.

### FND-02 — path and executable references

Represent:

- AbsolutePath for resolved existing or planned paths without blindly canonicalizing missing
  targets.
- ConfigRoot and ConfigSurfacePath.
- ExecutableRef as PATH name or absolute binary path.
- WrapperPath and source/target relation.

Rules:

- Expand home/platform variables only at adapter resolution boundary.
- Store normalized absolute paths in records; retain original display path only in previews.
- Never accept traversal from template/adapter-provided relative paths.
- Keep symlink resolution policy for mutation layer; path type alone does not follow links.

### FND-03 — registry schema v1

Top-level record:

| Field | Ownership |
|---|---|
| schema_version | superai |
| instances | superai |
| preserved foreign keys | original file owner |

Instance record fields:

| Field | Required | Notes |
|---|---:|---|
| id | yes | immutable |
| name | yes | mutable user label |
| harness | yes | HarnessId |
| config_root | yes | default install or isolated root |
| binary | optional | only when PATH resolution is insufficient |
| wrapper | optional | path, command name, generator version, content digest |
| isolation | yes | class plus adapter-specific parameters |
| origin | yes | default, created, mirrored, adopted |
| ownership | yes | superai, adopted, foreign, detached |
| template | optional | template ID plus applied version only |
| created_at | yes | record lifecycle fact |
| adapter_revision | yes | migration/revalidation trigger |

Forbidden fields:

- Model ID, provider endpoint, API key, env secret, skill list, plugin list, MCP definitions,
  effective capability results, or copied harness config.

Migration from current records:

1. Parse old vector safely.
2. Validate harness/name/config paths.
3. Assign stable IDs.
4. Infer only facts directly present; origin becomes adopted-legacy and isolation becomes unknown.
5. Preview migrated JSON.
6. Back up and store via safe mutation layer once available.
7. Remain able to read old records until one major schema transition completes.

### FND-04 — lifecycle and ownership states

Define explicit states:

- Install presence: absent, present, broken, unknown-version.
- Instance origin: default, created, mirrored, adopted.
- Ownership: superai-created, explicitly-adopted, foreign-managed, unmanaged, detached.
- Lifecycle: ready, needs-auth, degraded, conflict, missing-config, missing-binary.
- Isolation: relocated-root, explicit-config, project-scope, IDE-user-data, env-only,
  daemon-service, fixed-path-single, OS-bound, unsupported.
- Adapter support: full, constrained, single-instance, read-only, migration-only,
  research-blocked, unsupported.

No boolean such as isolated or supported may collapse these distinctions.

### FND-05 — operation contracts

Every mutating workflow returns a preview before commit:

- operation ID and kind.
- requested target and resolved resources.
- preconditions.
- ordered file/process actions.
- redacted semantic and lexical diffs.
- backups that will be created.
- warnings, conflicts, limitations, auth steps, and restart requirements.
- rollback plan.

Commit result:

- exact actions completed.
- backup IDs/paths.
- verification results.
- rollback status on failure.
- redacted diagnostics.

No interface callback, widget model, color, prompt string, or modal concept enters these contracts.

### FND-06 — error taxonomy

Extend error types without string matching:

- Validation and name/path collision.
- Unsupported harness/version/surface/operation.
- Research blocked.
- Parse and schema validation.
- Concurrent modification.
- Backup, commit, verification, and rollback.
- Binary detection/version probe/process exit.
- Network/template integrity.
- Auth required but externally owned.
- Foreign ownership.
- Port conflict/daemon readiness.

Each error carries safe resource identity and causal source. Secret-bearing values use redacted
wrappers and never implement unconstrained Display/Debug.

### FND-07 — nondeterministic boundaries

Introduce boundaries only where deterministic testing needs them:

- Clock and ID generation.
- Filesystem metadata/replace/sync primitives not expressible through pure functions.
- Platform/home/PATH facts.
- Process execution and version probes.
- HTTP fetch.

Avoid a generic service/manager/factory graph. Pure parsing, validation, diff, registry, and
resolution remain ordinary functions and data types.

## Implementation order

1. Add IDs, names, path refs, and validation.
2. Add ownership/lifecycle/isolation/support states.
3. Add registry v1 wire structs separately from domain structs.
4. Implement old-record migration and compatibility fixtures.
5. Add preview/result/error contracts.
6. Add only boundaries required by immediate filesystem work.

## Acceptance tests

- Existing repository test records load through migration.
- Registry rejects duplicate normalized names, IDs, config roots, and wrapper targets.
- Renaming an instance preserves InstanceId and template association.
- Serializing registry never emits a forbidden harness-owned field.
- Unknown enum values/schema versions fail with actionable compatibility errors.
- Error/debug snapshots contain no supplied secret sentinel.
- superai-config does not depend on superai-core; neither depends on superai-cli.

## Exit gate

- [ ] Registry schema v1 documented with golden fixtures.
- [ ] Current records migrate losslessly for all facts they contain.
- [ ] All identifiers, names, paths, states, previews, results, and errors are validated.
- [ ] No interface or harness-config value appears in instance records.
- [ ] Public types have one canonical path.

