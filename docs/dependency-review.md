# Dependency review (QAL-12)

Per-crate review evidence for every external dependency, recorded in-repo so
the AGENTS.MD rule ("dependency additions need verification: crates.io
existence, exact spelling, active maintenance") has an artifact, not just a
process. Re-review whenever `Cargo.lock` diff shows a new participant.

Tooling that enforces this at CI time (see `.github/workflows/ci.yml`):
`cargo deny check` (advisories v2, license allowlist, unknown-source deny),
`cargo shear --deny-warnings` (no unused deps), `cargo hack --each-feature`
(feature-combination builds), `cargo build --locked` (lockfile discipline).

## Lockfile-diff discipline

- `Cargo.lock` is committed; CI builds with `--locked`, so any dependency
  change must arrive as an intentional lockfile diff in the same commit.
- A dependency addition requires: (1) a row in the tables below, (2) the
  crates.io spelling verified against the registry page (near-miss names are
  a supply-chain red flag and stop the change), (3) `cargo deny check` clean,
  (4) a stated reason the dependency is not implementable in-tree.
- Version bumps are reviewed as diffs: minor bumps of parser crates
  (`serde_json`, `toml_edit`, `yaml-serde`) additionally re-run the codec
  property tests (`cargo test -p superai-config property`) because round-trip
  fidelity is the product.

## superai-config (L1 — config files)

| Dependency | Version | Role | Owner / maintenance | build.rs | Notes |
|---|---|---|---|---|---|
| serde | 1.x (derive) | value model for every codec | dtolnay, crates.io `serde`, one of the most-maintained Rust crates | YES (feature probing only, no external code) | derive macro only; no I/O |
| serde_json | 1.x (preserve_order) | StrictJson codec, catalog/registry serialization | dtolnay/serde-rs, crates.io `serde_json` | YES (limbs detection) | `preserve_order` keeps unmodelled-key order stable (goal: round-trip) |
| toml_edit | 0.23 | TOML codec with decor preservation | toml-rs org, crates.io `toml_edit` | no | the only crate that can edit TOML without reformatting — the reason it exists in-tree |
| thiserror | 2.x | error taxonomy derivation | dtolnay, crates.io `thiserror` | YES (has_dynamic_ast probing) | no runtime cost beyond Display |
| yaml-serde (`yaml_serde`) | 0.10 | YAML codec | crates.io `yaml_serde` (successor of serde_yaml); pure-Rust libyaml binding via `libyaml-rs` | no | spelling verified on crates.io; safe-libyaml, no unsafe parser in our tree |

## superai-core (L2 — instances/templates/capabilities)

All of superai-config's dependencies (same versions, workspace-inherited),
plus:

| Dependency | Version | Role | Owner / maintenance | build.rs | Notes |
|---|---|---|---|---|---|
| superai-config | path (workspace) | L1 layer | in-tree | no | the only allowed downward dependency |
| semver | 1.x (serde) | harness/template version ranges | semver-rs, crates.io `semver` | no | used for adapter version gates and update compatibility |
| sha2 | 0.11 | content digests (receipts, drift detection) | RustCrypto, crates.io `sha2` | no | pure Rust, no build script |
| hex | 0.4 | digest rendering | crates.io `hex` | no | trivial, stable |
| ureq | 3.x (rustls, gzip; default-features off) | bounded HTTPS template fetch (PRV-07/TPL-03) | crates.io `ureq` (algesten) | no | default features disabled to avoid TLS implementation choice; rustls only; usage mirrors template_fetch.rs discipline (timeouts, size caps, redirect policy) |
| duct | 1.x (timeout) | bounded subprocess execution (install/exec, git checkout) | crates.io `duct` (oconnor663) | no | timeout feature required for bounded probes; all call sites go through process.rs limits |

Transitive participants that warrant standing attention (from `Cargo.lock`,
97 packages total):

| Package | Via | Attention |
|---|---|---|
| ring | ureq→rustls | **ships build.rs and compiles C code**; pinned by rustls; audited by the rustls project; acceptable because the TLS surface is confined to template fetch |
| zeroize | ring/rustls | no build script; memory zeroing |
| libyaml-rs | yaml-serde | pure-Rust safe-libyaml port; no unsafe exposed to this workspace (`unsafe_code = "forbid"` holds in our crates) |

## superai-cli (L3 — the one binary)

| Dependency | Version | Role | Notes |
|---|---|---|---|
| superai-core | path (workspace) | L2 layer | the only upward dependency in the workspace; no interface types exist below this crate |

## Dev/tooling dependencies (not in the shipped graph)

| Tool | How it enters | Review |
|---|---|---|
| cargo-mutants 27.1.0 | `cargo install cargo-mutants --locked`, CI `mutation-testing` job | installed from crates.io with `--locked`; scope pinned by `.cargo/mutants.toml` |
| cargo-hack / cargo-shear / cargo-deny | CI supply-chain job, `--locked` installs | same discipline; deny.toml policy in-repo |

Review date: 2026-09-01 (area 8, QAL-12). Re-verify on every lockfile diff.
