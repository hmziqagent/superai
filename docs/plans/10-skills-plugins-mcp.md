# Subplan 10 — skills, plugins, and MCP servers

Parent: [master plan](master-plan.md)  
Task prefix: EXT  
Estimate: 10–14 reviewable change sets

## Outcome

One local skill registry can feed instances through whole-registry links, selected links, or
selected copies. Plugins and MCP servers use the same ownership/transaction discipline but remain
adapter-specific.

## Shared principles

- Registry/source metadata is superai-owned; harness destination config is harness-owned.
- Read destination fresh before every operation.
- Preserve foreign skills/plugins/MCP entries.
- Symlink/copy behavior is a caller choice per instance when harness supports it.
- Executable content is untrusted; installation does not execute it.
- Disable differs from remove.

## Skills

### EXT-01 — registry layout and records

Default logical root: ~/.superai/skills.

Registry record:

- SkillId/name.
- source kind and source locator.
- pinned revision/version.
- content digest.
- install/update timestamp.
- license/provenance metadata where available.
- local modification state.

Skill content remains ordinary files in registry. No copy of destination harness state is stored.

Validate:

- directory boundaries and symlinks.
- required SKILL.md where standard applies.
- frontmatter name/description.
- duplicate normalized names.
- file count/size.
- forbidden traversal/device files.

### EXT-02 — source acquisition/update

Supported sources are explicitly scoped:

- Existing local directory adoption.
- Git/GitHub source after URL/revision validation.
- Harness/marketplace source only through documented non-executing download path.

Update:

- Fetch to staging.
- Validate tree and digest.
- Detect registry-local edits.
- Preview file diff.
- Atomically replace or preserve conflict.

No post-install scripts.

### EXT-03 — destination modes

Per instance:

- LinkAll: destination points to registry root.
- LinkSelected: destination contains links to chosen skill dirs.
- CopySelected: destination owns copies.

Adapter declares available modes and path/config mechanism. Windows link privilege failure offers
copy only as explicit alternate plan, not silent fallback.

### EXT-04 — enable/disable/remove

Enable/disable mechanisms:

- Harness config allow/deny list.
- Add/remove search path.
- Create/remove owned destination link.
- Reversible rename only where harness documents it.

Remove registry skill:

- First report every linked/copied consumer.
- Refuse breaking links unless request includes consumer migration/removal.
- Copied destinations may remain as divergent copies; report them.

### EXT-05 — copied-skill drift

Track provenance beside superai-owned registry records, not inside foreign skill unless adapter
permits metadata:

- source SkillId/revision/digest at copy.
- destination digest observed fresh.

Update three-way:

- unchanged copy → replace/update.
- locally changed copy → preview conflict.
- destination missing → offer reinstall.

## Plugins

### EXT-06 — plugin abstraction

Plugin type is adapter-specific:

- directory/bundle.
- config entry.
- npm/package reference.
- marketplace install record.
- extension script.

Adapter declares:

- source and destination.
- whether install requires executing harness/package command.
- enable/disable/remove semantics.
- dependency and permission effects.
- restart requirement.

Initial safe scope prefers file/config plugins. Executing package installers requires separate
supply-chain preview and caller approval.

### EXT-07 — plugin lifecycle

Plan:

- Validate source identity/version/digest.
- Inspect existing entry/content.
- Detect name/path collisions.
- Back up foreign config.
- Stage files/config.
- Validate harness discovery.
- Commit and verify.

Removal affects only owned entry/content. Shared package dependency is not removed until no
consumer remains.

## MCP servers

### EXT-08 — canonical MCP definition

Represent:

- stable server ID/name.
- transport: stdio, HTTP, SSE where supported.
- command/args or URL.
- environment/header references.
- OAuth requirement as external harness-owned state.
- enabled state.
- tool include/exclude/permissions where harness supports.
- timeout.

Secret values are ephemeral and rendered only to adapter-declared sinks.

### EXT-09 — adapter rendering

Each adapter maps canonical MCP definition to native schema:

- mcpServers, mcp, context_servers, separate mcp.json, TOML table, CLI command, or unsupported.
- Preserve unknown server fields and other servers.
- Respect project/user/instance scopes and precedence.
- Remote transport downgrade is forbidden unless explicitly equivalent.
- OAuth-capable server may be configured, but superai does not perform OAuth.

### EXT-10 — MCP lifecycle

Operations:

- Inspect effective servers.
- Add/update one server.
- Enable/disable.
- Remove owned server.
- Move/copy between supported scopes with conflict preview.
- Validate via harness diagnostic command where non-mutating.

Colliding name:

- Same semantic definition → no-op/adopt choice.
- Different definition → explicit rename/replace conflict.

### EXT-11 — cross-instance bulk operations

Bulk enable/disable/update:

- Build all per-instance plans fresh.
- Show unsupported/constrained targets before commit.
- Use compensated multi-file transaction.
- Failure report identifies completed and rolled-back targets.

No global in-memory cached config.

## Tests

- LinkAll, LinkSelected, CopySelected on supported platforms.
- Link collision, dangling link, insufficient Windows privilege.
- Copy update clean/local conflict/missing.
- Registry removal with active consumers refuses by default.
- Plugin config preserves foreign entries.
- MCP add/update/disable/remove across JSON, JSONC, TOML schemas.
- Same-name MCP conflict.
- Secret sentinel absent from metadata/previews.
- Bulk operation external-edit conflict and rollback.
- Pi returns MCP absent natively unless verified extension path is selected.

## Exit gate

- [ ] Skill registry acquisition/update/remove works without executing content.
- [ ] Three destination modes work where adapter supports them.
- [ ] Enable/disable/remove semantics are distinct.
- [ ] Copied-skill drift is conflict-aware.
- [ ] Plugin lifecycle is supply-chain/ownership aware.
- [ ] MCP mappings cover every writable adapter or explicit absence.
- [ ] Foreign entries and secrets remain protected.
