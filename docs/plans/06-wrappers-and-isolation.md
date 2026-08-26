# Subplan 06 — wrappers and isolation

Parent: [master plan](master-plan.md)  
Task prefix: WRP  
Estimate: 7–10 reviewable change sets

## Outcome

superai generates, verifies, repairs, and removes user-named launchers for every documented
isolation class while honestly representing shared or non-isolatable state.

## Isolation classes

| Class | Typical mechanism | Examples |
|---|---|---|
| Relocated root | Harness home/config-root env | Claude, Codex, Goose, Kimi, Pi |
| Explicit config | CLI points to settings/env files | Aider, Amp, Trae |
| Inline/env profile | Per-process settings and keys | Qwen, iFlow, Plandex |
| Project scope | Cwd/repo config selects state | Crush, Continue, SWE-agent |
| IDE user-data | Launch editor with isolated storage | Cline, Roo, Kilo, Windsurf |
| ACP/executor wrapper | Parent editor/orchestrator spawns wrapper | Zed, orchestrators |
| Fixed path | Shared config must be activated in place | ZCode |
| Daemon | Separate roots plus ports/services | OpenClaw, Letta/OpenHands variants |
| OS-bound | Separate account/container may be only full split | Antigravity/Warp edge cases |

## Work packages

### WRP-01 — invocation specification

Adapter returns structured invocation:

- executable reference.
- ordered argv.
- environment set/unset operations.
- working-directory policy.
- config/state paths.
- stdin/stdout/daemon behavior.
- auth prerequisites.
- isolation guarantees and shared-state warnings.

Secrets are references or file-backed values where harness supports them, not embedded in registry
or preview output.

### WRP-02 — generated wrapper grammar

Generate minimal deterministic wrappers:

- POSIX sh for macOS/Linux where safe.
- PowerShell or cmd launcher for Windows based on verified harness behavior.
- Optional shim strategy only after executable resolution tests.

Rules:

- Quote every path/arg using target shell rules.
- Use exec/replacement semantics for interactive/ACP processes.
- Set only adapter-required environment.
- Avoid HOME swap when a narrower documented variable exists.
- Generated marker includes instance ID, generator version, and non-secret content digest.
- Never interpolate API-key value into a comment or marker.

### WRP-03 — name/path resolution

Resolve wrapper destination from configured user-owned bin directory:

- Check executable name collisions on PATH.
- Check filesystem case folding and Windows extensions.
- Refuse overwrite of unowned file.
- Permit any valid user name; no forced harness prefix.
- Preview effective command resolution before commit.

### WRP-04 — isolation verification

After generation, verify without destructive launch:

- Parse generated grammar.
- Confirm executable and paths.
- Compare env/root parameters to instance record.
- Where harness supports diagnostic config output, run bounded no-auth probe.
- Inspect target and source before/after to prove no unexpected writes.

Mark full only when state surfaces are actually split. Shared subscription, keychain, cloud
account, or project state yields constrained status.

### WRP-05 — IDE launchers

IDE adapter specifies:

- user-data directory.
- extensions directory/profile.
- workspace argument.
- CLI backend root if separate.
- login/globalStorage/keychain sharing caveats.

Test two concurrent profiles. If credentials remain shared through OS keychain, expose constrained
isolation even when settings files split.

### WRP-06 — fixed-path activation

Do not pretend this is a normal wrapper:

- Store superai-owned saved profiles outside active harness path.
- Wrapper/activation command acquires lock.
- Fresh-read and back up active file.
- Apply selected profile through safe transaction.
- Launch app if requested.
- Never auto-swap back while app may still write.
- Next activation reconciles external edits and offers capture/discard choices.

### WRP-07 — daemon/service wrappers

Specify:

- root/config env.
- unique port set and bind address.
- pid/service identity.
- readiness URL/command with timeout.
- foreground/background launch.
- shutdown command/signal.

Port allocation is conflict checked at commit/start, not persisted as unquestionably free.
Killing requires verified executable, instance marker, and process start identity.

### WRP-08 — repair/remove

Repair:

- Recompute expected wrapper.
- Compare marker/digest and semantic invocation.
- Show diff.
- Replace only owned wrapper.

Remove:

- Delete/quarantine only exact owned wrapper.
- Preserve instance root and record unless lifecycle operation says otherwise.
- Opaque/user-edited wrapper requires explicit detach, not overwrite.

## Representative tests

- Paths with spaces, quotes, Unicode, dollar signs, and percent signs.
- Empty/unset API-key variables do not fall through to another global credential unexpectedly.
- Two wrappers point to distinct roots and keep source config unchanged.
- Existing user command blocks creation.
- Wrapper rename is atomic and rollback-safe.
- IDE profiles run concurrently without same data directory.
- Fixed-path activation conflicts with external edit.
- Daemon refuses occupied port and never kills unrelated process.
- POSIX and Windows golden launchers contain no secret sentinel.

## Exit gate

- [ ] Every adapter declares one isolation class and exact limitations.
- [ ] Full/constrained/single-instance claims have runtime evidence.
- [ ] Generated wrappers are deterministic, marked, quoted, and secret-free.
- [ ] User wrappers are never overwritten.
- [ ] Fixed-path and daemon flows use dedicated lifecycle behavior.
