# Subplan 08 — remote templates, versioning, diffs, and updates

Parent: [master plan](master-plan.md)  
Task prefix: TPL  
Estimate: 8–12 reviewable change sets

## Outcome

Versioned harness/provider templates are fetched as direct files from a configurable GitHub
repository. Instances can show available versions and exact default changes, then apply a
conflict-aware update only by explicit choice.

## Repository contract

### TPL-01 — direct-file layout

Proposed logical layout:

- catalog file listing template IDs, latest versions, file paths, digests, statuses.
- one immutable file per TemplateId and TemplateVersion.
- optional model/provider data files referenced by digest.
- schema files and fixtures in same repository.

This is a static GitHub repository, not a registry service:

- No package/archive format.
- No publishing API or server.
- No executable install hook.
- Client downloads raw files directly.
- Updating repository files makes versions discoverable without a superai binary release.

Exact paths stay configurable; do not hard-code one owner/repository into domain logic.

### TPL-02 — template schema

Template fields:

- schema version, TemplateId, SemVer version.
- harness ID and supported adapter/harness version ranges.
- provider ID/protocol requirements.
- human label and status.
- required user inputs: API key, region, model choice, endpoint override.
- ordered owned config patches.
- wrapper environment/args additions.
- asset requirements.
- capability map.
- migration notes as structured warnings.
- source references and content digest.

Forbidden:

- Secret values.
- Shell code.
- Arbitrary paths outside adapter-owned selectors.
- Binary/plugin payloads.
- Automatic update flag.

Template validation resolves every selector against adapter schema before preview.

### TPL-03 — fetch client

Fetch:

- HTTPS only by default.
- Configurable GitHub repository and pinned ref/channel.
- Bounded redirects, response size, and timeout.
- User agent/version.
- Digest verification from catalog.
- Schema validation before use.
- Clear rate-limit/not-found/offline errors.

Template content is network data and untrusted. No field reaches filesystem/process APIs without
typed validation.

No harness config caching rule is unaffected: harness configs still read fresh. Optional
template-file cache, if later approved, must be digest-addressed and never treated newer than a
successful remote check.

### TPL-04 — version discovery

For an instance with template ID/version:

1. Fetch catalog fresh.
2. Validate current entry and latest compatible version.
3. Compare SemVer and harness/adapter constraints.
4. Return up-to-date, update-available, yanked, current-missing, incompatible, or offline.

Never advance registry or touch instance during check.

### TPL-05 — old/new template diff

Fetch immutable applied version and candidate version.

Diff:

- provider endpoint/auth-style changes.
- model additions/removals/default changes.
- context/capability changes.
- wrapper/env changes.
- asset changes.
- selector/schema changes.

Show semantic changes, not only source YAML/JSON lines. Redact secret placeholders consistently.

If applied historical version cannot be fetched/verified, update may be inspected as candidate
but automatic three-way application is blocked.

### TPL-06 — three-way instance update

Inputs:

- Base: applied template version.
- New: candidate template version.
- Local: fresh current harness config.

For each template-owned selector:

- Local equals base → apply new.
- New equals base → keep local.
- Local equals new → already applied.
- Local and new both differ from base → conflict requiring explicit resolution.
- Selector missing/type-changed → schema conflict.

Foreign/unowned selectors remain untouched.

Preview contains:

- template old→new defaults.
- current local values.
- automatically applicable edits.
- conflicts and resolution options.
- resulting wrapper/asset/capability changes.

### TPL-07 — apply and record

Transaction:

1. Re-fetch/verify required template files or pin in-memory verified bytes.
2. Fresh-read instance.
3. Recompute three-way plan and conflict token.
4. Apply config/wrapper/assets.
5. Validate harness instance.
6. Resolve capabilities.
7. Write new template version to registry last.

Failure/rollback retains old version record. Registry may never claim a version that did not
verify on disk.

### TPL-08 — template lifecycle

Statuses:

- active.
- preview.
- deprecated with replacement.
- yanked for unsafe/broken content.

Rules:

- No automatic instance update.
- Existing pinned yanked version remains inspectable; warn before use/update.
- Version files are immutable; correction requires new version.
- Catalog points to compatible latest but retains history.
- Major template update may require explicit migration steps and cannot silently reuse old
  selectors.

## Tests

- Catalog/template schema and digest.
- Malicious traversal, huge file, wrong harness, unknown selector, and shell content rejected.
- No update available and incompatible update behavior.
- Exact semantic old/new diff.
- Three-way clean update, local override preservation, conflict, deleted selector.
- External file edit between preview/commit aborts.
- Failure before registry update keeps old TemplateRef.
- Adding template version in fixture repo needs no Rust edit.

## Exit gate

- [ ] Direct GitHub file distribution works without registry service/package.
- [ ] Historical versions are immutable and verified.
- [ ] Update checks never mutate.
- [ ] Three-way merge protects local divergence.
- [ ] Updates are explicit and registry version advances last.
- [ ] Template data cannot escape adapter selectors or execute code.

