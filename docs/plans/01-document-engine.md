# Subplan 01 — loss-minimizing document engine

Parent: [master plan](master-plan.md)  
Task prefix: DOC  
Estimate: 8–12 reviewable change sets

## Outcome

superai-config can parse, inspect, validate, and apply narrow structured edits to every required
config format while preserving content it does not own. Unsupported or ambiguous syntax fails
before write.

## Scope

In scope:

- Strict JSON, JSONC, TOML, YAML, env files, and supported line-oriented/text fragments.
- Semantic path lookup and mutation.
- Lexical preservation contracts per codec.
- Parse/schema diagnostics and redacted diff inputs.
- Fresh disk load on every operation.

Out of scope:

- Atomic write, backup, rollback: subplan 02.
- Harness keys and version mapping: subplan 03.
- Full general-purpose source editing for executable configs.

## Preservation contract

Preservation is codec-specific and tested:

| Format | Required preservation |
|---|---|
| Strict JSON | Unknown values and key order; unchanged subtrees retain value shape; formatting preservation if selected codec supports it |
| JSONC | Comments, trailing commas, unknown values, key order, indentation, newline style |
| TOML | Comments, key order, whitespace, table layout, dotted-key style where untouched |
| YAML | Comments, anchors, aliases, tags, scalar style, document markers, unknown nodes where supported |
| env | Comments, blank lines, export prefix, quoting, spacing, duplicate-key policy, newline style |
| Text/fragment | Untouched bytes outside a uniquely identified managed span |

Exact byte identity is required for a no-op. For an edit, unchanged source regions must remain
byte-identical unless the chosen codec documents a narrower safe guarantee. If that guarantee
cannot be met, adapter state becomes read-only until a safe method exists.

## Work packages

### DOC-01 — source document envelope

Define a format-neutral loaded document:

- source bytes and detected encoding/BOM/newline style.
- DocumentKind.
- source digest.
- parse tree/token tree.
- diagnostics with spans.
- file metadata needed later by mutation layer.

Rules:

- UTF-8 is default; adapters must opt into any other encoding.
- Invalid encoding is diagnostic, not lossy replacement.
- Missing file and empty file are distinct.
- Root-type expectations are adapter/schema concerns.

### DOC-02 — typed selector and edit operations

Use typed selectors, not ad-hoc dotted strings:

- object/map key.
- array index only when adapter schema proves stable.
- identity-selected array item, such as model id or server name.
- TOML table/key.
- managed text span.

Operations:

- set scalar/value.
- insert map/table entry at policy-defined position.
- remove key/entry.
- merge owned fields while retaining foreign fields.
- enable/disable without deleting definition.
- append/remove identity-keyed item.
- ensure directory/list entry.

Each operation declares:

- owned keys.
- expected old value or absence.
- duplicate handling.
- create-parent policy.
- redaction policy.

### DOC-03 — strict JSON

Replace Map-only mutation with a document abstraction.

Requirements:

- Preserve arbitrary root type for raw editor reads; adapter may require object.
- Reject duplicate keys unless adapter explicitly defines resolution.
- Avoid float/integer coercion.
- No-op byte identity.
- Deterministic insertion policy.
- Semantic diff ignores formatting; lexical diff exposes actual file change.

Retain serde_json for semantic validation only if lexical layer remains authoritative.

### DOC-04 — TOML

Build on toml_edit while testing:

- comments and table decor.
- arrays of tables.
- inline tables.
- dotted keys.
- quoted keys.
- duplicate/invalid declarations.
- CRLF and missing final newline.

Never rebuild a full typed struct and serialize over the source document.

### DOC-05 — JSONC

Dependency/research gate:

1. Verify candidate crate exists on crates.io, spelling/owners/activity/license.
2. Test comments, trailing commas, duplicate keys, source spans, insertion, removal, and no-op.
3. Reject a codec that parses JSONC but serializes normalized JSON.

Required fixtures include OpenCode, Kilo, Amp, Copilot, and IDE settings variants.

### DOC-06 — YAML

Required for Aider, Continue, Goose, SWE-agent, Trae, workflows, and legacy surfaces.

Research tests:

- anchors/aliases and merge keys.
- block/folded scalars.
- tags.
- comments.
- flow style.
- multiple documents.
- duplicate keys.
- quoted numeric/string ambiguity.

Policy:

- Do not mutate through an alias if ownership/effect is ambiguous.
- Do not expand anchors into duplicated values.
- A parser without safe comment/style preservation may power validation/read-only support, not
  writes.

### DOC-07 — env files

Support:

- KEY=value and export KEY=value.
- single/double/unquoted values.
- comments and escaped characters.
- blank lines and duplicate definitions.
- CRLF.

Duplicate policy is adapter-declared: edit effective last value, reject ambiguity, or manage a
dedicated generated file. Never silently deduplicate.

### DOC-08 — line/text fragments

For non-structured supported files:

- Managed spans use stable start/end sentinels.
- Existing duplicate/partial sentinels fail closed.
- Insertion preserves all bytes outside span.
- Removal removes only complete owned span.
- Shell quoting is not interpreted here.

Executable configs are not automatically eligible. Crushrc and similar surfaces require an
adapter-proven command API or safe source/managed-fragment contract.

### DOC-09 — schema and version validation hooks

Document engine exposes primitives:

- syntax validation.
- adapter-supplied semantic validator.
- path/value type checks.
- unknown-key preservation.
- deprecation diagnostics.

No universal harness schema is embedded in codec modules.

### DOC-10 — diff model

Produce:

- semantic operations: old/new at owned selectors.
- lexical unified diff for actual file output.
- redaction spans.
- no-op indication.
- warnings when surrounding formatting must change.

Semantic diff remains stable across whitespace. Lexical diff proves preservation behavior.

## Test strategy

- Golden input/output fixtures for every syntax construct.
- Property: parse then no-op emits identical bytes.
- Property: edit then parse succeeds and intended selector has intended value.
- Property: unrelated selectors keep semantic values.
- Lexical assertion: bytes outside edited spans stay unchanged where codec promises it.
- Malformed/truncated/huge/nested inputs fail without write.
- Duplicate keys and YAML alias edge cases.
- Secret sentinel absent from diagnostics/diff snapshots after redaction.
- Parser fuzz targets added in subplan 13.

## Exit gate

- [ ] All six required format classes have explicit read/write/read-only support decisions.
- [ ] No-op byte identity passes.
- [ ] Unknown data and supported lexical material survive edits.
- [ ] Unsupported ambiguous constructs fail before mutation planning.
- [ ] Adapters can express every owned edit without format-specific logic in core workflows.

