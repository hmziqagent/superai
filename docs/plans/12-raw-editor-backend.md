# Subplan 12 — raw config editor backend

Parent: [master plan](master-plan.md)  
Task prefix: RAW  

All RAW tasks are closed: RAW-04 was verified at baseline, the rest by area 7 (judge-verified).

What shipped: interface-neutral open, validate, diff, and commit services for harness config
files, with sensitive raw-source handling, conflict-checked commits through safe mutation, and
transactional creation of missing files. No editor widget or interface code exists.

## Exit gate

- [x] Read/validate/diff/commit services are interface-neutral.
- [x] Raw source is treated as sensitive.
- [x] Every commit uses safe mutation and fresh conflict check.
- [x] Adapter schema/version rules apply.
- [x] Opaque/internal stores cannot be edited.
- [x] No GPUI/TUI/CLI editor work exists.

## Completion record

> Completion record (run 3): RAW-04 verified at baseline; RAW-01 (surface-identity open
> with scope/precedence + explicit-path policy), RAW-02 (validate-time adapter schema +
> size gate), RAW-03 (scope/precedence warning, restart requirement, template-owned
> divergence markers), RAW-05 (gated transactional create with rollback + empty-buffer
> format rule), RAW-06 (env duplicate diagnostics as warnings at the effective definition),
> RAW-07 (reopen/rebase with manual-rebase guidance, no auto-apply) closed by area 7
> (judge-verified). Era/version gates at the commit boundary come from area 3's
> surface_gates_for_adapter (era conflict → UnsupportedVersion before any disk mutation).

Decisive test for RAW-02: `cargo test -p superai-core --all-features
validate_draft_runs_schema_and_version_gate_without_disk`
(crates/superai-core/src/raw_editor.rs:1219) asserts the schema blocking
diagnostic, the version-gate refusal, and the oversize size diagnostic through
`validate_draft` with no disk access. Re-run 2026-10-23, passing.
