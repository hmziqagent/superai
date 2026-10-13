# Dependency review (QAL-12)

Per-crate review evidence for every external dependency, recorded in-repo so
the AGENTS.md rule ("dependency additions need verification: crates.io
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
| toride-apps | 0.1 (default-features off) | backend execution for npm, cargo, pipx, uv, Homebrew installs (toride_bridge.rs) | crates.io `toride-apps` (freeoxide, first-party: same owner as this repo) | no | sync twins only; no tokio feature; argv pinned by toride's own tests; bridge forces this repo's process posture on every spec |
| toride-runner | 0.1 | runner seam the bridge implements to enforce env/PATH/cap/timeout policy | crates.io `toride-runner` (freeoxide, first-party) | no | default features; no tokio-runner/stream |
| toride-registry | 0.1 (default-features off) | source model behind toride-apps planning | crates.io `toride-registry` (freeoxide, first-party) | no | no http feature; no network at runtime |

Transitive participants that warrant standing attention (from `Cargo.lock`,
193 packages total):

| Package | Via | Attention |
|---|---|---|
| ring | ureq→rustls | **ships build.rs and compiles C code**; pinned by rustls; audited by the rustls project; acceptable because the TLS surface is confined to template fetch |
| zeroize | ring/rustls | no build script; memory zeroing |
| libyaml-rs | yaml-serde | pure-Rust safe-libyaml port; no unsafe exposed to this workspace (`unsafe_code = "forbid"` holds in our crates) |
| async-trait | toride-apps | compile-time desugaring only; no runtime, no executor; the sync twins are the only path this repo calls |
| option-ext (MPL-2.0) | toride-apps→dirs→dirs-sys | license added to deny.toml allowlist in dba6694; OSI-approved weak copyleft, used as a library |
| dirs / dirs-sys / libredox / redox_users | toride-apps | path resolution inside toride; no build scripts beyond libredox (pure Rust) |
| tracing / tracing-core / tracing-attributes | toride-runner | log framework compiled in; no subscriber installed here, so nothing is emitted |
| which | toride-runner | binary discovery for runner defaults; the bridge supplies explicit argv, so this path is not exercised |
| camino / serde_yaml / unsafe-libyaml | toride-registry | camino is a UTF-8 path wrapper; serde_yaml is upstream-archived, enters only through first-party toride-registry with no network feature, candidate to drop in a toride-side cleanup |
| libsqlite3-sys | rusqlite (superai-usage) | ships build.rs and compiles the bundled SQLite C amalgamation; pinned by rusqlite's `bundled` feature, workspace-level decision recorded in the superai-usage section below |
| foldhash (Zlib) | rusqlite→sqlite-wasm-rs→rsqlite-vfs→hashbrown, and hashlink→hashbrown | license added to deny.toml allowlist 2026-10-13; OSI-approved permissive (orlp, crates.io `foldhash`, 0.2.0, active), same rationale as option-ext/MPL-2.0 |

## superai-usage (L3 — usage truth)

serde and thiserror as in superai-config (workspace-inherited, reviewed
above), plus:

| Dependency | Version | Role | Owner / maintenance | build.rs | Notes |
|---|---|---|---|---|---|
| superai-core | path (workspace) | L2 layer; `ProviderId` keys the usage registry | in-tree | no | sits between core and cli; consumes ids and adapter declarations, adds no interface types |
| serde_json | 1.x (preserve_order, raw_value; workspace) | parse harness usage artifacts (USG-03 readers, cost tables) | dtolnay/serde-rs; 1.0.151 in the lock, active | YES (limbs detection) | raw_value used by cost.rs's models.dev table parsing (cost.rs:14); same codec discipline as superai-config |
| jiff | 0.2.x (serde; workspace) | window resets, cost periods, DST-correct day bucketing (USG-04/05) | BurntSushi; 0.2.38 in the lock, active | no | typed timestamps, zone-aware month-to-date; used by cost.rs and both readers |
| rusqlite | 0.40.x (bundled; workspace) | opencode v2 history reader (USG-03) | rusqlite org (gwenn); 0.40.2 in the lock, active | YES (libsqlite3-sys, see participants table) | decision: `features = ["bundled"]` pinned at the workspace root (Cargo.toml), so the reader needs no system libsqlite3 and behaves identically on every machine; the trade is the C-amalgamation participant above; foreign dbs open with SQLITE_OPEN_READ_ONLY only (reader/opencode.rs) |

Still planned: ureq 3.x (rustls, gzip; default-features off) for the USG-05
models.dev pricing fetch (algesten; 3.4.2 current, active; verified on
crates.io 2026-10-12). Same bounded-client discipline as core's
template_fetch.rs; row moves into the table above when it lands.

## superai-cli (L4 — the one binary)

| Dependency | Version | Role | Notes |
|---|---|---|---|
| superai-core | path (workspace) | L2 layer | an upward dependency, as superai-usage also has; no interface types exist below this crate |

## Dev/tooling dependencies (not in the shipped graph)

| Tool | How it enters | Review |
|---|---|---|
| cargo-mutants 27.1.0 | `cargo install cargo-mutants --locked`, CI `mutation-testing` job | installed from crates.io with `--locked`; scope pinned by `.cargo/mutants.toml` |
| cargo-hack / cargo-shear / cargo-deny | CI supply-chain job, `--locked` installs | same discipline; deny.toml policy in-repo |

Review date: 2026-09-01 (area 8, QAL-12); re-reviewed 2026-10-11 for the
toride participants added by PR #8; re-reviewed 2026-10-12 for the
superai-usage crate scaffold (USG-02, one path dependency, no new external
lockfile participants; the package count rose by the crate itself);
re-reviewed 2026-10-13 for the USG-03 reader dependencies (serde_json with
raw_value, jiff with serde, rusqlite 0.40 bundled) at the 193-package lock.
Re-verify on every lockfile diff.
