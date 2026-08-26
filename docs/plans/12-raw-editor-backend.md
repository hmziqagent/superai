# Subplan 12 — raw config editor backend

Parent: [master plan](master-plan.md)  
Task prefix: RAW  
Estimate: 4–6 reviewable change sets

## Outcome

Future interfaces can open the exact harness files, display parse/schema diagnostics, edit raw
content, preview semantic/lexical changes, and commit safely. No editor widget or interface type
is implemented.

## Work packages

### RAW-01 — open document

Request identifies:

- instance/harness surface, not arbitrary unrestricted path by default.
- optional explicit path for advanced local editing after path-policy validation.
- expected harness version/schema.

Response:

- sensitive source text wrapper.
- source digest/conflict token.
- format/encoding/newline.
- syntax and schema diagnostics with spans.
- effective scope/precedence.
- read-only reason where applicable.

Source text may contain API keys. Sensitive wrapper must avoid Debug/log/telemetry serialization.
Returning it to an authorized local interface is intentional; incidental diagnostics remain
redacted.

### RAW-02 — validate draft

Given original conflict token plus draft text:

- Validate encoding/size.
- Parse using document engine.
- Run adapter version/schema validator.
- Identify deprecated/unknown owned keys.
- Confirm root/type constraints.
- Report diagnostics without touching disk.

Unknown unmodelled keys are not errors unless harness schema forbids them. Executable and opaque
surfaces remain read-only unless dedicated validator exists.

### RAW-03 — diff

Produce:

- lexical unified diff original→draft.
- semantic diff where parser can calculate it.
- scope/precedence warning.
- secret-bearing spans marked sensitive for UI redaction/reveal policy.
- restart/reload requirement.
- affected capability/provider/template-owned fields.

If draft removes/changes template-owned fields, report divergence but do not forbid it. Disk is
authoritative and users may intentionally diverge.

### RAW-04 — commit

Commit:

1. Fresh-read file.
2. Compare conflict token.
3. Revalidate exact draft bytes.
4. Build single/multi-file transaction if adapter requires companion files.
5. Back up.
6. Atomic replace.
7. Read-back parse/schema verification.
8. Return backup and updated conflict token.

No autosave. No stale document cache. A caller must reopen/rebase after conflict.

### RAW-05 — missing/new files

Creating missing config:

- Adapter must permit creation and define initial root/document shape.
- Preview parent directories, permissions, precedence effect, and first-run risk.
- New file rollback removes only created file/empty owned parents.

An empty editor buffer is not automatically an empty object; format/adapter decides valid empty
document.

### RAW-06 — format-specific behavior

- JSON/JSONC: preserve or explicitly show formatting change.
- TOML: preserve comments/decor in structured edits; raw draft is authoritative if valid.
- YAML: protect anchors/aliases and multi-document constraints.
- env: show duplicate/effective-definition diagnostics.
- Text fragment: raw whole-file editing only when adapter allows it.
- Internal SQLite/keychain/auth stores: never open for editing.

### RAW-07 — schema evolution

If harness upgrades while draft is open:

- Version/config conflict invalidates commit.
- Reopen with new adapter schema.
- Offer draft text to caller for manual rebase, but do not auto-apply under new schema.

## Tests

- Open→no change→commit is no-op and creates no unnecessary foreign-file write.
- Invalid JSON/TOML/YAML draft never writes.
- External edit after open causes conflict.
- Raw secret content is accessible only through explicit sensitive value, absent from Debug/errors.
- Unknown key survives draft/commit.
- Wrong harness version blocks commit.
- Missing-file create and rollback.
- Read-only internal/keychain surface cannot commit.

## Exit gate

- [ ] Read/validate/diff/commit services are interface-neutral.
- [ ] Raw source is treated as sensitive.
- [ ] Every commit uses safe mutation and fresh conflict check.
- [ ] Adapter schema/version rules apply.
- [ ] Opaque/internal stores cannot be edited.
- [ ] No GPUI/TUI/CLI editor work exists.

