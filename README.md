# superai

An all-in-one configurator for AI coding harnesses: instances, templates,
skills, and providers. Rust, local only, one binary.

Status: backend implemented through master-plan milestones A through H
(document engine, safe mutation, adapters with a 51-surface ledger, lifecycle,
discovery, wrappers, providers, templates, capabilities, skills/plugins/MCP,
install, raw editor, verification harness, platform gates). The filesystem
layer beneath it is boring, as intended: atomic commits, backups, rollback,
and conflict detection are all in place. 1480+ tests, strict lints, fixtures,
verification harness. The GPUI interface is still planned.

- [docs/goal.md](docs/goal.md): what this is and why
- [docs/plans/master-plan.md](docs/plans/master-plan.md): non-UI implementation index and subplans
- [docs/harness-configs/](docs/harness-configs/): per-harness config paths, env vars, and multi-instance wrapper techniques

## Supported harnesses

The catalog tracks 51 harness surfaces. Every entry records where its config
lives, which isolation mechanism multi-instance setups use (relocated root,
per-profile files, `--user-data-dir`, or fixed-path with activation), which
platforms it runs on, and when its research was last verified. Per-harness
details and adapter support states (full, partial, constrained,
single-instance, unsupported) live in
[docs/harness-configs/](docs/harness-configs/). Awkward harnesses stay in the
catalog with an accurate state rather than being dropped; a harness that
genuinely cannot be isolated is supported single-instance.

## Backups, recovery, and where data lives

superai never edits a harness config in place. Every write goes through fresh
read → backup → atomic replace → read-back verify. If anything goes wrong
mid-operation, the transaction layer rolls back, and for the journaled
multi-file arms the crash journal makes the next startup offer recovery.

Data locations (all under your home directory, nothing in the cloud):

| Path | What it is |
|---|---|
| `~/.superai/instances.json` | superai's own instance records: which instances exist, where their config dirs are, which template each came from |
| `~/.superai/quarantine/` | recoverable deletions: removed instance roots are moved here (digest-verified) before any final delete, and can be restored |
| `~/.superai/journal/` | per-operation crash journal for the journaled multi-file arms (fixed-path profile activation, instance reconfigure, provider changes); written before the mutation and removed after verified success; skills/MCP/plugin/template-update transactions commit through the same atomic write boundary with side-by-side backups but are not journal-recovered yet. Leftover journals are detected at startup and recovered from |
| `~/.superai/install_receipts/` | receipts for harness binaries superai installed |
| `~/.superai/templates/`, `~/.superai/assets/`, `~/.superai/backups/` | fetched template files, shared assets, and superai-managed backup artifacts; uninstall never touches these without explicit intent |
| `<config>.bak.<millis>.<rand>` | side-by-side backups of every foreign config file before each write; listing and verified restore work from them |

Recovery is always local and file-based: restore a backup, restore from
quarantine, or let startup recovery replay or undo a journaled operation. A
crash at any point leaves configs at either the old or the new bytes, never
half-written.

## Security posture

superai is local only. No server, no accounts, and the only outbound traffic
is an explicit, bounded HTTPS fetch of template files from the repository you
configure. There is no secret vault and no OAuth: your API key is written
where the harness already expects it, in the harness's own config, and
superai adds no store on top (see [docs/goal.md](docs/goal.md#secrets)).

Secrets are never echoed. Keys are redacted from diffs, previews,
diagnostics, records, and test output, and new files carrying secret-shaped
content are written owner-only (`0600` on unix). superai is not a proxy and
has no chat runtime: it configures tools, it does not sit between you and a
provider. Routing belongs to other projects.

## Template repository configuration

Templates are not baked into the binary. They live in a plain GitHub
repository (the catalog plus one directory per template), so a new model or
provider is a file edit, not a release. Point superai at a repository, and
every instance built from a template sees new versions with a diff of what
actually changed; applying an update is always your explicit choice per
instance. Fetches are HTTPS-only, size-capped, digest-verified, and cached on
disk. The on-disk layout and versioning rules are in
[docs/plans/08-templates.md](docs/plans/08-templates.md).

## Development

```
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

CI runs these plus locked builds, feature-matrix checks, unused-dependency
and advisory checks, and a scoped mutation-testing job on Linux, macOS, and
Windows. Before a release, re-run the catalog freshness check
(`coverage_ledger::tests::catalog_freshness_within_policy_window`) and update
`FRESHNESS_AS_OF` after re-verifying harness research docs. Dependency review
records live in [docs/dependency-review.md](docs/dependency-review.md).
