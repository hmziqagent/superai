# Subplan 09 — capability matrix and resolution

Parent: [master plan](master-plan.md)  
Task prefix: CAP  
Estimate: 4–6 reviewable change sets

## Outcome

Core answers what an instance can do, how support is satisfied, and why, using current
harness/provider/template data. Consumers do not branch on harness identity.

## Capability model

### CAP-01 — catalog

Initial required capabilities:

- web_search.
- vision.
- computer_use.
- mcp.

Catalog entry:

- CapabilityId.
- stable meaning.
- optional aliases/deprecations.
- validation rules.

Keep typed known IDs for ergonomic Rust use while allowing schema validation to reject unknown
template IDs cleanly. Do not silently treat unknown as absent.

### CAP-02 — support value

Support remains:

- Native: harness implements capability against provider.
- Substituted: provider or configured extension satisfies it differently.
- Absent: unavailable.

Resolved entry also includes:

- satisfaction source: harness, provider, template, plugin/MCP, or policy.
- concise explanation.
- evidence/version range.
- limitations.

Support is not a boolean. Substituted is usable but semantically distinct.

### CAP-03 — matrix source and precedence

Sources:

1. Adapter declares harness-native behavior and version constraints.
2. Provider data declares server-side/modal capabilities.
3. Harness-provider template declares pair-specific overrides/substitutions.
4. Installed plugin/MCP state may add a capability only when adapter can verify it.
5. Local/admin policy may disable native support.

Resolution rule is explicit per capability. A generic maximum ranking is insufficient because a
provider substitution can be blocked by harness wire behavior or policy.

### CAP-04 — completeness validation

For every active template/harness/provider combination:

- All catalog capabilities must resolve.
- No duplicate/conflicting rules.
- Native claim must be compatible with harness version.
- Substituted claim names its provider/extension source.
- Absent claim may include required upgrade/add-on.

Incomplete matrix blocks template publication/use; it does not default to absent.

### CAP-05 — runtime resolution

Resolve from:

- Fresh adapter inspection.
- Current installed harness version.
- Provider/template data.
- Fresh extension/MCP state if relevant.
- Current policy.

Do not persist result in instance registry. Optional within-operation memoization ends with
operation.

Public query:

- Resolve all capabilities for instance.
- Resolve one capability.
- Filter instances by capability/support class.

Return InstanceId plus capability result; harness identity remains internal diagnostic metadata.

### CAP-06 — update effects

Template/provider/extension preview includes capability delta:

- native→substituted.
- substituted→absent.
- absent→native.
- source/limitation changes.

Capability change is visible before template/update commit.

## Reference scenarios

- Claude Code + Anthropic: web search native.
- Claude Code + GLM where server search works: web search substituted.
- Same pair without supported vision transport: vision absent even if model marketing says vision.
- Pi: MCP absent natively; verified extension may provide substituted.
- Managed backend without custom provider: provider template is unsupported rather than a fake
  absent capability matrix.

## Tests

- Every active fixture pair is complete.
- Same harness resolves differently for two providers.
- Provider capability cannot override incompatible harness transport.
- Disabled plugin/policy removes or changes support.
- Unknown capability/template entry fails validation.
- Public consumer test filters on capability without matching HarnessId.
- Template update preview contains capability delta.

## Exit gate

- [ ] Matrix resolves all required capability IDs for every supported pair.
- [ ] Native/substituted/absent carries source and explanation.
- [ ] Resolution is fresh and not stored in records.
- [ ] Consumer APIs do not require harness checks.
- [ ] Capability deltas appear in update previews.

