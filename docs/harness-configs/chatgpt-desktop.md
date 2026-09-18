# ChatGPT Desktop (OpenAI — Chat + Work + Codex) — Configuration Reference

**Compiled:** 2026-09-18 · **Status:** product `active`; the unified desktop app (Chat + Work + Codex) since 2026-07-09 for macOS 14+/Windows[2]; **Linux PREVIEW since 2026-08-11** (.deb/.rpm, x64+ARM64 — overturns the "no Linux build" assumption)[3]. The app has **no config-relocation mechanism of its own** (verified-absent)[8]. The only verifiable local config surface is **`~/.codex`, which belongs to the [codex-cli.md](codex-cli.md) harness** — officially, "The app picks up your session history and configuration from the Codex CLI and IDE extension"[4]. This harness is therefore modeled **read-only with a shared-state warning**, and aliasing routes through codex-cli.

History: a standalone "Codex app" shipped for macOS 2026-02-02 and Windows 2026-03-04, then folded into the unified ChatGPT app on 2026-07-09 ("Existing Codex app users can update to ChatGPT and open Codex"); the old macOS app continues as "ChatGPT Classic"[2][4].

---

## 1. Config: file locations & schema

The **Chat** side of the app has NO officially documented local config file[5]. The **Codex** side reads the exact CLI store (official docs)[4][6]:

| Surface | Path | Kind | Source |
|---|---|---|---|
| Codex user config (shared with CLI) | `~/.codex/config.toml` (`$CODEX_HOME`, default `~/.codex`) | TOML | [6] |
| Desktop-only key inside that config | `desktop.custom_file_handlers` (documented "User-level only" — smoking gun that the app's Codex reads `~/.codex/config.toml`) | TOML table | [6] |
| Codex profiles (shared with CLI) | `$CODEX_HOME/<name>.config.toml`, selected with `--profile` | TOML | [6] |
| Credentials | `~/.codex/auth.json` (`cli_auth_credentials_store = file\|keyring\|auto\|ephemeral`; keyring = OS credential store) | JSON, secret-bearing | [6] |
| Sessions | `$CODEX_HOME` session store — community-confirmed the desktop app ingests CLI/codex-exec sessions from it | internal | [4][6] |
| MCP servers (local, Codex side) | `[mcp_servers.<id>]` tables **inside `config.toml`** — no separate MCP file | TOML | [6] |
| MCP connectors (Chat side) | remote-only, web-managed (Settings → Apps; Business/Enterprise/Edu) | server-side | [5] |

**Modeling consequence:** the local surface set above is the codex-cli surface already modeled by the `codex-cli` adapter (`CODEX_HOME`, `config.toml` `mcp_servers` — alias-live-verified run 3/4). The `chatgpt-desktop` adapter re-declares these surfaces **read-only for inspection** and adds the shared-state warning; it deliberately does NOT add a duplicate writable MCP declaration (that would double-own codex-cli's destination).

## 2. Platforms, detection, versioning

- macOS 14+ (Apple Silicon or Intel), Windows[2]; **Linux preview**: Ubuntu 24.04/26.04 LTS, Debian 13, Fedora 43/44, Arch; x64 + ARM64; `.deb` and `.rpm`. Computer Use absent on Linux preview. Flatpak reports are CONFLICTING — treat as unconfirmed[3].
- Version scheme is CalVer-ish (`26.812.10818` cited for an Enterprise/Edu feature gate)[2].
- No `--version` CLI probe documented for the GUI binary; exact package binary name **unverified** — detection for the live-verification round should key on `~/.codex/config.toml` / `~/.codex/auth.json` existence evidence (and must note that the same files indicate a codex-cli install — the stores are shared and not distinguishable at file level).

## 3. MCP in the app — honest split

- **Official position for Chat:** local stdio servers "Not directly. ChatGPT connects to remote MCP servers." Private servers go through "Secure MCP Tunnel"; definitions are server-side and web-managed (developer mode, Workspace Settings → Apps; Business/Enterprise/Edu)[5].
- **Desktop local-MCP via developer mode** (Settings → Connectors → Advanced) exists in the desktop apps (later than web) with the standard `mcpServers` JSON pasted in the UI — community-documented only. A community path claim `~/Library/Application Support/ChatGPT/config/mcp.json` (macOS) / `~/.config/ChatGPT/config/mcp.json` (Linux) is **community-grade, NOT official**; persistence and Windows path undocumented[5].
- **Codex side:** local `[mcp_servers.<id>]` in `~/.codex/config.toml` is official[6] — but that destination is owned by the codex-cli adapter. → `chatgpt-desktop` declares **MCP absence for its own surface** with this exact split as the reason; the community `config/mcp.json` path is flagged here, never modeled writable.

## 4. Plugins and skills

No local plugin mechanism is documented for the app: MCP "apps" are server-side and web-managed (drafts → publish, admin review, RBAC)[5]. Skills: no local skill-file surface documented for the app (Codex skills live under the shared `$CODEX_HOME` already modeled by codex-cli). → plugin absence declared.

## 5. Relocation / environment variables

**Verified-absent for the app**: no env var / flag documented that relocates the app's own state[8]. `CODEX_HOME` is documented for the CLI/IDE extension[6]; **whether the GUI app honors `CODEX_HOME` when launched is NOT documented** — only the CLI honors it. Do not claim GUI-level relocation.

## 6. MULTI-INSTANCE WRAPPERS

**Honest verdict: alias via codex-cli; the app itself is not aliasable.** `chatgpt-desktop` is isolation class `fixed_path_single` at the shared `~/.codex` store, support `ReadOnly` (inspect-only; the store belongs to codex-cli). `superai alias create chatgpt-desktop …` is refused at the wrapper-planning stage: the adapter's plan declares **no relocation env vars of its own** (setting `CODEX_HOME` here would fabricate GUI-level relocation the docs do not support). The plan text and the catalog row point at the verified path instead:

```bash
# The verified multi-instance story (run-4 live evidence, .z-workflow/evidence/aliases/codex-cli/):
superai-alias create codex-cli work-a --mcp a=...   # seeds $CODEX_HOME=<root>/config.toml [mcp_servers]
CODEX_HOME=~/aliases/codex-cli/work-a codex mcp list # the CLI demonstrably reads only the alias set

# Then launch the desktop app normally: the app MAY pick up the same store
# (it reads ~/.codex per [4][6]) but whether it honors a non-default CODEX_HOME
# is undocumented — verify per install before relying on it.
```

**Shared-state warning (declared in the adapter's wrapper plan):** `shares ~/.codex with codex-cli` — `config.toml` (incl. `mcp_servers`), `auth.json`, profiles, sessions. Two harness rows now point at one store; only codex-cli owns writes to it.

## 7. NOT FOUND (searched 2026-09-18, do not infer)

- `~/.chatgpt/` — does not exist in any source; the Chat side has no documented local config file[5].
- An official local MCP file + schema for the app (only the community `config/mcp.json` path claim, macOS/Linux)[5].
- The Windows path for that community MCP file[5].
- Whether the GUI honors `CODEX_HOME`[6][8].
- Flatpak availability (conflicting reports)[3].

## Sources

All fetched 2026-09-18; per-source extracts at `.z-workflow/evidence/desktop-research/`:

1. `help-openai-com-9275200-download.md` — https://help.openai.com/en/articles/9275200 + https://chatgpt.com/download (unified app 2026-07-09, macOS 14+, Classic rename, CalVer)
2. `help-openai-com-9275200-download.md` — same fetch (macOS/Windows availability, version gate, Work plans)
3. `openai-linux-codexapp-sources.md` — Linux preview 2026-08-11 distro matrix + Flatpak conflict + community announcement
4. `openai-linux-codexapp-sources.md` — https://openai.com/index/introducing-the-codex-app ("picks up your session history and configuration from the Codex CLI and IDE extension"; app history 2026-02-02/2026-03-04/2026-07-09)
5. `help-openai-com-12584461-developer-mode-mcp.md` — https://help.openai.com/en/articles/12584461 (remote-only MCP position, developer mode, community desktop local-MCP + `config/mcp.json` path claim)
6. `learn-chatgpt-com-config-reference.md` — https://learn.chatgpt.com/docs/config-file/config-reference (+ config-advanced): `~/.codex/config.toml`, `CODEX_HOME`, `desktop.custom_file_handlers`, profiles, `auth.json`, `mcp_servers.<id>`
7. `relocation-search-summary.md` — relocation search aggregate (no app-level mechanism found)
8. `relocation-search-summary.md` — same aggregate (CODEX_HOME GUI-honoring undocumented; community symlink workaround is Claude-Desktop-specific)
