# Subplan 11 — harness detection, installation, update, and uninstall

Parent: [master plan](master-plan.md)  
Task prefix: PKG  
Estimate: 8–12 reviewable change sets

## Outcome

superai can detect existing harness installations and safely plan/install/update/uninstall
supported binaries/apps through modular installer backends. Binary lifecycle stays separate from
instance config/data lifecycle.

## Principles

- Existing install detection precedes every install.
- Package operation never claims/removes user config.
- Uninstalling a binary does not remove instances, wrappers, or config unless separately requested.
- Installer command/output is previewed and bounded.
- Official sources/package identifiers are data with platform/version constraints.
- No invented dependency or package name.

## Work packages

### PKG-01 — dependency verification spike

Before Cargo.toml changes:

1. Identify toride's current modular crates and exact crates.io names.
2. Verify maintainers, recent releases, licenses, source repository, build scripts, transitive
   dependencies, and MSRV 1.97 compatibility.
3. Verify which crate wraps duct and which exposes mise installation.
4. Prototype one read-only detect and one sandboxed install-plan path.
5. Record whether runtime mise binary is required or library downloads/manages it.

If names/APIs cannot be verified, stop dependency work. Near-miss crate names are supply-chain
blockers.

### PKG-02 — installation catalog schema

Per harness/platform:

- HarnessId.
- executable/app/extension IDs.
- supported install methods.
- official package name/tap/registry/repository.
- version source and constraint.
- detect commands/paths.
- update/uninstall command.
- admin privilege requirement.
- checksum/signature/source verification.
- known conflicts/replacements.
- documentation and last verified date.

No arbitrary shell pipeline in catalog. Command is executable plus argv tokens.

### PKG-03 — install detection

Collect all matches:

- PATH resolution.
- configured binary path.
- mise-managed tool.
- Homebrew/npm/cargo/pipx/uv/system package metadata where supported.
- desktop app/extension installation.

Return:

- version and path.
- installation method confidence.
- shadowed duplicates in PATH order.
- architecture/platform mismatch.
- broken shim.

Do not select first match silently when multiple versions affect instances.

### PKG-04 — install planning

Request chooses harness, version/channel, method, destination.

Plan validates:

- platform/architecture support.
- official package identity.
- version availability.
- writable destination.
- network and admin requirements.
- conflicts with existing installs.
- expected executable after install.

Prefer mise-backed versioned installs when supported by verified backend. Use package managers or
official installers only through explicit method-specific adapters.

### PKG-05 — process execution

Use structured duct-backed commands:

- no shell interpolation by default.
- minimal environment.
- bounded stdout/stderr capture with redaction.
- timeout/cancellation.
- exit status and signal handling.
- working directory explicit.

Never pass API keys to installer process unless official install method requires a token and
caller explicitly supplies an ephemeral credential.

### PKG-06 — install verification and receipt

After command:

- Re-detect exact executable/app.
- Parse version.
- Confirm requested range/channel.
- Run non-mutating help/version smoke probe.
- Record superai-owned install receipt: method, package ID, executable, version, timestamp.

Receipt does not claim package ownership if install pre-existed or verification is ambiguous.

Install does not launch auth flow automatically. Instance can remain needs-auth.

### PKG-07 — update

Plan:

- Detect method/current version.
- Fetch/resolve available version.
- Show compatibility impact on adapters/instances.
- Back up no harness configs merely for binary update, but scan adapter support first.
- Execute method-native update.
- Re-detect and revalidate instances read-only.

If new version is outside adapter range, warn/block update unless caller explicitly accepts
read-only instance state.

### PKG-08 — uninstall

Preflight:

- Exact installed package/method/path.
- Every instance and wrapper referencing binary.
- Whether binary is shared with foreign managers/users.
- Package receipt ownership.

Default:

- Uninstall binary only through native method.
- Preserve config, instances, wrappers, backups, templates, and assets.
- Mark affected instances binary-missing.

Manual binary files not proven superai-owned are not deleted automatically.

### PKG-09 — multiple versions and pinning

Support:

- Instance absolute binary_path.
- mise/version-manager shims with resolved version evidence.
- Side-by-side versions where method allows.
- Adapter version compatibility per instance.

Wrapper generation pins exact binary when user selects it; PATH-based wrapper reports ambiguity.

### PKG-10 — desktop/extensions/unsupported installers

Desktop apps and editor extensions may require:

- open marketplace/install URL.
- package manager/cask.
- user-driven GUI installer.

Core returns ExternalInstallRequired when no safe non-interactive path exists. This is supported
workflow state, not an excuse to download unknown binaries.

## Tests

- Existing install avoids duplicate installation.
- Multiple PATH matches are reported.
- Fake process verifies argv has no shell concatenation.
- Failed install creates no receipt.
- Successful command but wrong version fails verification.
- Update compatibility warns before command.
- Uninstall preserves config and marks instances.
- Foreign/shared binary blocks destructive manual removal.
- Secret and package-manager auth token redaction.
- Platform catalog validation catches missing package IDs/commands.

## Exit gate

- [ ] toride/mise/duct dependencies and package identifiers verified before use.
- [ ] Detection covers pre-existing and multiple installs.
- [ ] Install/update/uninstall are structured, previewed, and verified.
- [ ] Binary lifecycle never implies config/data deletion.
- [ ] Adapter compatibility is checked around updates.
- [ ] External/manual installation is represented honestly.

