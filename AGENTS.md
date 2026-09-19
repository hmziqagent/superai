# Rules

- Always load /caveman skill

## Layout

| Crate | Layer | Owns |
|---|---|---|
| `crates/superai-config` | 1: Config | harness config files: read fresh, back up, write back preserving unmodelled keys |
| `crates/superai-core` | 2: Core | instances, templates, capabilities, skills, install |
| `crates/superai-cli` | 3: Interface | placeholder CLI; GPUI comes later, nothing below layer 3 knows an interface exists |

Order of work is `docs/goal.md`: filesystem layer until boring, then the rest of
core, interface last.

## Quality gates

Run after every change:

    cargo fmt --all
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace --all-features

## Hard rules

Every rule below is a hard rule. A change that breaks one is not done, even
with all three gates green.

### Failure handling

No `unwrap()`, `expect()`, `panic!`, `todo!`, or `unimplemented!` in non-test
code. Propagate errors with `?`; library crates define their error types with
`thiserror`. Never write `let _ = …` over a `Result` or a `Future`: handle it
or log it. Indexing (`v[i]`) and string slicing (`&s[..n]`) panic on a miss or
a bad boundary, so non-test code uses `.get()` and respects UTF-8 char
boundaries when truncating strings.

Never silence a lint with `#[allow(...)]`. Fix the code, or use
`#[expect(lint, reason = "…")]` at the narrowest scope that covers it.
Crate-level `#![allow(...)]` is forbidden.

### Config data

Never cache a harness config in memory between operations. Disk is the truth:
read fresh, edit, write back, and preserve every key superai does not model.
Back up any file superai did not create before writing to it. No exceptions.

### Comments

One or two lines, and only what the code itself cannot say: an invariant, a
hazard, a non-obvious constraint. Delete comments that narrate the obvious,
restate the code, or explain a change's history. No banner blocks, no
separator rules, no multi-paragraph doc essays where a line fits.

Doc comments and examples meet the same bar: they must assert behaviour,
compiling and holding true against the code, or they get deleted.

### Writing

All copy in this repo, from comments to the README, reads like a person wrote
it. Load the humanizer and humanize-writing skills before writing prose. No
AI tells: no inflated diction, no filler openers, no rule-of-three padding,
no em dashes standing in for commas, no bolded inline-header lists. Write
plainly and keep the facts. User-facing docs carry no design narratives, no
"why we chose X over Y" essays, no self-report tables.

### Security review on every change

Before calling any change done, check it for path traversal, symlink races,
argv and command injection, archive extraction mistakes (zip-slip), TOCTOU,
and secret leakage. superai writes into users' config directories and runs
other tools' binaries, so treat every path, argument, and archive entry as
untrusted input.

### Bulk

Ship the small version. No dead code, no redundant clones, no needless
indirection. Prefer `&str` over `String` and `&[T]` over `Vec<T>` in function
parameters. Avoid gratuitous allocations in hot paths.

### Dependencies

Do not add a dependency without checking that it exists on crates.io, is
spelled correctly, and is actively maintained. A near-miss name
(`proc-macro1` vs `proc-macro2`) is a supply-chain red flag: stop and report
it.

### Tests and public surface

Tests assert observable behaviour. A test that restates a constant
(`assert_eq!(RETRIES, 3)`) or mirrors the implementation's branches is
worthless; test a property instead. Public items are reachable through
exactly one path; no re-exports to paper over a refactor.

CLAUDE.md is a symlink to this file; edit AGENTS.md.
