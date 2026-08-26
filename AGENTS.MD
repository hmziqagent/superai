# Rules
- Always load /caveman skill

## Layout

| Crate | Layer | Owns |
|---|---|---|
| `crates/superai-config` | 1 — Config | harness config files: read fresh, back up, write back preserving unmodelled keys |
| `crates/superai-core` | 2 — Core | instances, templates, capabilities, skills, install |
| `crates/superai-cli` | 3 — Interface | placeholder CLI; GPUI comes later, nothing below layer 3 knows an interface exists |

Order of work is `docs/goal.md`: filesystem layer until boring, then the rest of core, interface last.

## Rust quality gates

Run after every change:

    cargo fmt --all
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace --all-features

### Rules

- Never silence a lint with `#[allow(...)]`. Use `#[expect(lint, reason = "…")]` at the
  narrowest scope, or fix the code. Crate-level `#![allow(...)]` is forbidden.
- No `unwrap()`, `expect()`, `panic!`, `todo!`, or `unimplemented!` in non-test code.
  Propagate errors with `?`. `thiserror` in the library crates.
- No indexing (`v[i]`) or string slicing (`&s[..n]`) in non-test code — they panic.
  Use `.get()`, and respect UTF-8 char boundaries when truncating strings.
- Never write `let _ = …` on a `Result` or a `Future`. Handle or log it.
- Never cache a harness config in memory between operations. Disk is the truth: read
  fresh, edit, write back, and preserve every key superai does not model.
- Back up before every write to a file superai did not create.
- Prefer `&str` over `String` and `&[T]` over `Vec<T>` in function parameters.
- Do not add a dependency without checking it exists on crates.io, is spelled correctly,
  and is actively maintained. A near-miss name (`proc-macro1` vs `proc-macro2`) is a
  supply-chain red flag — stop and report it.
- Tests must assert observable behaviour. A test that restates a constant
  (`assert_eq!(RETRIES, 3)`) or mirrors the implementation's branches is worthless —
  test a property instead.
- Public items should be reachable through exactly one path. No re-exports to paper
  over a refactor.
- No design narratives, "why we chose X over Y" essays, or self-report tables in
  user-facing docs.
