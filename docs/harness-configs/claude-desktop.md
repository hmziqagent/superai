# Claude Desktop (Anthropic) — Configuration Reference

**Compiled:** 2026-09-18 · **Status:** product `active`; GA since 2025-10-21[2]. **Linux BETA exists** (Ubuntu/Debian x64+arm64, .deb + apt repo)[1][4] — this overturns the older "macOS/Windows only" assumption. The one solid, writable, officially documented config surface is `claude_desktop_config.json` (`mcpServers`, stdio)[6]; **no config-relocation mechanism exists** (verified-absent — no env var, no portable mode, no `--config-dir`)[8], so multi-instance isolation is honestly REFUSED, not emulated.

Claude Desktop is Anthropic's consumer desktop app (chat + Cowork + a Claude Code integration hosted in a Code tab)[3]. It is a separate surface from the [claude-code.md](claude-code.md) CLI harness (`CLAUDE_CONFIG_DIR` relocates the CLI's `~/.claude`, not this app), though the two overlap on personal skills (`~/.claude/skills`) and (on Windows) the app's Claude Code sessions actually run inside a WSL2 distro[3].

---

## 1. Config: file locations (platform-gated)

| Platform | Config path | Status | Source |
|---|---|---|---|
| macOS | `~/Library/Application Support/Claude/claude_desktop_config.json` | official | [6] |
| Windows | `%APPDATA%\Claude\claude_desktop_config.json` | official, **caveat below** | [6] |
| Linux | `~/.config/Claude/claude_desktop_config.json` | **community-corroborated only** — not yet in official consumer docs (3P docs corroborate `~/.config/Claude/logs` on Linux) | [5][8] |

- The file is created on demand by Settings → Developer → "Edit Config"[6].
- **Windows MSIX dual-path bug** (GitHub issue #26073): "Edit Config" opens `%APPDATA%\Claude\claude_desktop_config.json` but an MSIX install may read `C:\Users\<u>\AppData\Local\Packages\...` instead — custom MCP servers broke after an update. Enterprise MSIX installs are the risky shape[2][8].
- Logs (harness-managed, detect-only): macOS `~/Library/Logs/Claude/` (`mcp.log`, `mcp-server-<name>.log`); Windows `%APPDATA%\Claude\logs`[6]; Linux `~/.config/Claude/logs` (3P docs)[5].
- The 3P/enterprise variant is a SEPARATE surface (out of alias scope): managed prefs `com.anthropic.claudefordesktop.plist` (macOS MDM) / `HKLM\SOFTWARE\Policies\Claude` (Windows) / `/etc/claude-desktop/managed-settings.json` (Linux), local `configLibrary/` with `_meta.json` + `<id>.json`, auto-update managed keys (`disableAutoUpdates`, `configRecheckIntervalMinutes` = 10 min from v1.46388.1), enterprise `managedMcpServers` key[5].

### 1.1 `claude_desktop_config.json` schema

Official shape (local stdio servers only)[6]:

```json
{
  "mcpServers": {
    "<name>": { "command": "<abs path>", "args": ["..."], "env": { "K": "v" } }
  }
}
```

- `command` required; `args` array; `env` object of strings (per-server). **Paths inside `args` must be absolute** — relative paths make the server not load[6].
- Remote servers are NOT configured in this file; they go through the Customize → Connectors UI[6].
- **Restart required after editing** (full quit + restart of the app)[6] — the adapter models `RestartBehavior::Restart`.
- Same `mcpServers` shape as workbuddy's `.mcp.json`, so the repo's JSON MCP writer round-trips it (foreign keys preserved).

## 2. Versioning, platforms, detection

- Versioning is date-stamped builds (`v<yyyymmdd>.n`); e.g. Code panes need "Claude Desktop v1.2581.0 or later"[3]. No `--version` CLI probe is documented for the GUI binary.
- Platform availability: macOS universal .dmg; Windows x64/arm64 (+ MSIX/PKG enterprise); **Linux BETA .deb-only** — apt repo `https://downloads.claude.ai/claude-desktop/apt/stable`, `sudo apt install claude-desktop`, Ubuntu 22.04+/Debian 12, x86_64/arm64; updates via apt, NOT in-app auto-update (in-app auto-update is macOS/Windows only)[1][4].
- Linux feature gaps: Computer Use and Dictation absent; Cowork needs a QEMU/vhost_vsock VM (the .deb pulls qemu-system-x86/ovmf/virtiofsd); Quick Entry X11-only[1][4].
- `/etc/default/claude-desktop` `CLAUDE_DESKTOP_ADD_REPO="false"` skips apt-repo registration — it is a packaging knob, **not** config relocation[4].
- Detection expectations for the live-verification round: config-root existence evidence per platform (table above) is the reliable signal; a PATH binary named `claude-desktop` exists on Linux .deb installs (package name), but the exact binary name per platform is **unverified** — treat config evidence as primary.

## 3. Plugins and skills

- **Desktop Extensions (`.mcpb`)** are the plugin mechanism: zip archives with `manifest.json` + `server/` + bundled deps (renamed from `.dxt` on 2025-09-11; spec open-sourced in `anthropics/dxt`). The app ships a built-in Node runtime, auto-updates extensions, and stores extension **secrets in the OS keychain** (shared!)[7]. Install is open-with / drag onto Settings, or the in-app extension directory; enterprise admins control extension approval[2][7].
- **GAP (explicit):** the per-OS **install location of `.mcpb` extensions is NOT published** — `${__dirname}` refers to the unpacked extension dir but no concrete path is documented[7]. Without a documented directory there is no honest `directory_bundle` staging target → the adapter declares **plugin absence**, not a guessed path.
- Skills: account-level skills are provisioned by org admins via Customize → Skills (support-article flow)[3]; desktop/Cowork ALSO loads **personal skills from `~/.claude/skills/<name>/`** per the Claude Code docs — a surface shared with the [claude-code.md](claude-code.md) harness[3].

## 4. Relocation / environment variables

**Verified-absent** (searched 2026-09-18): no `CLAUDE_DESKTOP*` config env var, no portable mode, no `--config-dir` flag; consumer paths are hardcoded per-OS (§1); the only community workaround is a symlink/junction of the whole config dir[8]. Do NOT fabricate a `CLAUDE_DESKTOP_CONFIG_DIR` (the generic `<HARNESS>_CONFIG_DIR` fallback would be wrong for this harness).

## 5. MULTI-INSTANCE WRAPPERS

**Honest verdict: no isolation knob exists → aliasing is refused, not emulated.** `claude-desktop` is isolation class `fixed_path_single` (config writable in place at the default app-support root); `superai alias create claude-desktop …` is refused at the wrapper-planning stage because the adapter's plan declares **no relocation env vars** — exactly the guard the alias core applies. MCP installs/inspection at the default root remain fully supported (writable `mcpServers` decl, §1.1).

What a user can honestly do today (documented, not superai-automated):

```bash
# Swap the ONE default config set in place (single instance only):
claude-desktop-edit-config   # Settings → Developer → Edit Config
# ... edit mcpServers, save, restart the app (restart required[6])

# Community workaround for a second, hand-managed set (NOT isolation, no env):
mv ~/Library/Application\ Support/Claude ~/claude-set-b
ln -s ~/claude-set-b ~/Library/Application\ Support/Claude   # symlink swap[8]
```

Shared state that a wrapper can never split (per research): the OS keychain (extension secrets[7]), the subscription/account, and — on Windows — the WSL2 distro in which the app's Claude Code sessions run[3].

## 6. Desktop app ↔ Claude Code CLI relationship

- The app hosts a Claude Code integration (Code tab/sessions; Pro/Max/Team/Enterprise)[3].
- **On Windows the session's Claude Code process, tools, and git all execute inside a WSL2 distro** (WSL2 only; config lives in the distro home, not `%USERPROFILE%\.claude`; trust is per-distro+folder; connectors/plugins not yet available in WSL sessions)[3].
- Whether the app's Claude Code shares `~/.claude` with the `claude` CLI is **NOT documented** (gap)[3].

## 7. NOT FOUND (searched 2026-09-18, do not infer)

- Any config-relocation env var / portable mode / `--config-dir` for the app[8].
- The official Linux consumer-doc page for `~/.config/Claude/claude_desktop_config.json` (path is community-corroborated + 3P-logs corroborated only)[5][8].
- The per-OS `.mcpb` extension install directory[7].
- Whether the app's Claude Code tab shares `~/.claude` with the CLI[3].
- Any CLI flags for the GUI binary.

## Sources

All fetched 2026-09-18; per-source extracts at `.z-workflow/evidence/desktop-research/`:

1. `claude-com-download.md` — https://claude.com/download (platform availability, Linux beta, enterprise MSIX/PKG)
2. `claude-com-download.md` / `anthropic-com-desktop-extensions.md` — GA/enterprise/admin-extension-approval, extension mechanics
3. `code-claude-com-desktop-wsl.md` — https://code.claude.com/docs/en/desktop-wsl (+ /docs/en/desktop-quickstart, /docs/en/skills snippets)
4. `code-claude-com-desktop-linux.md` — https://code.claude.com/docs/en/desktop-linux (apt repo, QEMU, `/etc/default/claude-desktop`)
5. `claude-com-3p-configuration.md` — https://claude.com/docs/third-party/claude-desktop/configuration (3P/enterprise variant, Linux logs path, managed keys)
6. `modelcontextprotocol-io-quickstart-user.md` — https://modelcontextprotocol.io/quickstart/user (canonical consumer config paths + schema, restart rule, logs)
7. `anthropic-com-desktop-extensions.md` — https://www.anthropic.com/engineering/desktop-extensions (.mcpb manifest, keychain secrets, unpublished install dir)
8. `relocation-search-summary.md` — relocation NOT-FOUND aggregate + MSIX dual-path issue + symlink workaround + Linux path corroboration
