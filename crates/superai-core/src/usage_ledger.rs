//! USG-01 ledger: subplan 14's overlap table as a tested artifact. Every
//! overlap-row adapter declares exactly one usage state; every other adapter
//! stays unmodeled.

#[cfg(test)]
mod tests {
    use crate::adapter::Adapter;
    use crate::harness_catalog;
    use crate::test_util::temp_dir_unique;
    use std::fs;

    /// Harness ids with a row in subplan 14's overlap table
    /// (docs/plans/14-usage-and-proxy.md): 29 provider mappings, 37 ids.
    const OVERLAP_HARNESS_IDS: &[&str] = &[
        "claude-code",
        "codex-cli",
        "oh-my-codex",
        "cline",
        "cursor",
        "cursor-agent",
        "copilot-cli",
        "copilot-coding-agent",
        "copilot-vscode",
        "gemini-cli",
        "antigravity-cli",
        "opencode",
        "oh-my-opencode",
        "qwen-code",
        "kimi-code-cli",
        "legacy-kimi-cli",
        "kilo-code",
        "kilocode-cli",
        "zed-acp",
        "windsurf",
        "amp",
        "grok-build",
        "deepseek-harness",
        "command-code",
        "muse-code",
        "minimax-code",
        "factory-droid",
        "warp",
        "devin",
        "kiro",
        "mimo-code",
        "mistral-vibe",
        "workbuddy",
        "qoder-cli",
        "qoder-ide",
        "pi",
        "omp",
    ];

    fn overlap_adapter(id: &str) -> Box<dyn Adapter> {
        harness_catalog::concrete_adapter_for(id)
            .unwrap_or_else(|| panic!("no concrete adapter for overlap id {id}"))
    }

    #[test]
    fn overlap_ledger_matches_subplan_count() {
        assert_eq!(OVERLAP_HARNESS_IDS.len(), 37);
        let mut sorted = OVERLAP_HARNESS_IDS.to_vec();
        sorted.sort_unstable();
        for pair in sorted.windows(2) {
            assert_ne!(pair[0], pair[1], "duplicate overlap id {}", pair[0]);
        }
        for id in OVERLAP_HARNESS_IDS {
            assert!(
                harness_catalog::find_by_id(id).is_some(),
                "overlap id {id} missing from the harness catalog"
            );
        }
    }

    #[test]
    fn overlap_adapters_declare_exactly_one_usage_state() {
        for id in OVERLAP_HARNESS_IDS {
            let adapter = overlap_adapter(id);
            let surface = adapter.usage_surface();
            let absence = adapter.usage_absence_reason();
            assert!(
                surface.is_some() ^ absence.is_some(),
                "{id} must declare exactly one of usage_surface/usage_absence_reason"
            );
            if let Some(reason) = absence {
                assert!(!reason.is_empty(), "{id} absence reason is empty");
            }
        }
    }

    #[test]
    fn declared_surfaces_carry_locators_and_honest_accounts() {
        for id in OVERLAP_HARNESS_IDS {
            let Some(surface) = overlap_adapter(id).usage_surface() else {
                continue;
            };
            assert!(
                !surface.artifacts.is_empty(),
                "{id} declares a usage surface with no artifacts"
            );
            for artifact in &surface.artifacts {
                assert!(!artifact.id.is_empty(), "{id} has an empty artifact id");
                assert!(
                    !artifact.path_resolver.fallback.is_empty(),
                    "{id}/{} has an empty fallback hint",
                    artifact.id
                );
            }
            assert!(
                surface
                    .provider_account
                    .as_deref()
                    .is_none_or(|a| !a.is_empty()),
                "{id} account label must be a non-empty name or None",
            );
            for note in &surface.not_modeled_roots {
                assert!(
                    !note.trim().is_empty(),
                    "{id} carries a blank not-modeled root note"
                );
            }
        }
    }

    #[test]
    fn non_overlap_adapters_leave_usage_unmodeled() {
        for entry in harness_catalog::all_entries() {
            if OVERLAP_HARNESS_IDS.contains(&entry.id) {
                continue;
            }
            let Some(adapter) = harness_catalog::adapter_for_id_fold(entry.id) else {
                panic!("no adapter resolves for catalog id {}", entry.id);
            };
            assert!(
                adapter.usage_surface().is_none() && adapter.usage_absence_reason().is_none(),
                "{} has no overlap row; its usage state must stay unmodeled",
                entry.id
            );
        }
    }

    /// Env vars the declared usage roots resolve through, each bound to a
    /// path inside the observed tree.
    const TREE_ENV: &[(&str, &str)] = &[
        ("HOME", "home"),
        ("XDG_DATA_HOME", "data"),
        ("CLAUDE_CONFIG_DIR", "home/.claude"),
        ("CODEX_HOME", "home/.codex"),
        ("GROK_HOME", "home/.grok"),
        ("GEMINI_CLI_HOME", "home/.gemini"),
        ("PI_CODING_AGENT_DIR", "home/.pi/agent"),
        ("MUSE_SESSIONS_DIR", "data/muse/sessions"),
        ("OPENCODE_DB", "data/opencode/opencode.db"),
    ];

    const SWEEP_ENV: &str = "SUPERAI_USAGE_LEDGER_SWEEP";
    const SWEEP_OUT_ENV: &str = "SUPERAI_USAGE_LEDGER_SWEEP_OUT";
    const NEVER_WRITES_TEST: &str =
        "usage_ledger::tests::declaring_usage_surfaces_never_writes_disk";

    fn tree_env_for(root: &std::path::Path) -> Vec<(String, String)> {
        TREE_ENV
            .iter()
            .map(|(var, rel)| {
                (
                    (*var).to_owned(),
                    root.join(rel).to_string_lossy().into_owned(),
                )
            })
            .collect()
    }

    fn build_bound_tree(root: &std::path::Path) {
        let files: &[(&str, &[u8])] = &[
            (
                "home/.claude/projects/proj/s.jsonl",
                b"claude transcript bytes",
            ),
            (
                "home/.config/claude/projects/proj/s.jsonl",
                b"no-env claude bytes",
            ),
            (
                "home/.claude-swap-backup/sessions/1-work/projects/proj/s.jsonl",
                b"legacy swap-slot bytes",
            ),
            (
                "data/claude-swap/sessions/2-alt/projects/proj/s.jsonl",
                b"xdg swap-slot bytes",
            ),
            (
                "home/.codex/sessions/2026/10/12/r.jsonl",
                b"codex rollout bytes",
            ),
            (
                "home/.codex/archived_sessions/r.jsonl",
                b"archived rollout bytes",
            ),
            ("data/opencode/opencode.db", b"opencode history bytes"),
            (
                "data/muse/sessions/2026/10/12/session/session.jsonl",
                b"muse session bytes",
            ),
            (
                "home/.grok/sessions/encoded-cwd-1/session-1/signals.json",
                b"{\"contextTokensUsed\": 2968}",
            ),
            (
                "home/.gemini/antigravity-cli/conversations/c.db",
                b"antigravity cli conversations bytes",
            ),
            ("home/.gemini/antigravity/a.db", b"antigravity ide bytes"),
            (
                "home/.gemini/antigravity/conversations/v.db",
                b"antigravity ide conversations bytes",
            ),
            ("home/.pi/agent/sessions/pi.jsonl", b"pi session bytes"),
            ("home/.omp/agent/sessions/omp.jsonl", b"omp session bytes"),
            (
                "home/Library/Application Support/Windsurf/User/globalStorage/state.vscdb",
                b"windsurf plan-cache bytes",
            ),
        ];
        for (rel, bytes) in files {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }

    fn walk_tree(dir: &std::path::Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk_tree(&entry.path(), out);
            } else {
                out.insert(
                    entry.path().to_string_lossy().into_owned(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }

    fn snapshot_tree(root: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut out = std::collections::BTreeMap::new();
        walk_tree(root, &mut out);
        out
    }

    /// Bound path for a hint: alternative before ", else", prose after '('
    /// dropped, `$VAR` via `TREE_ENV`, `~` as HOME, stop at a shape marker.
    fn bound_root(label: &str, hint: &str) -> String {
        let hint = hint.split_once(", else ").map_or(hint, |(bound, _)| bound);
        let hint = hint
            .split_once('(')
            .map_or(hint, |(path, _)| path)
            .trim_end();
        let mut segments: Vec<&str> = Vec::new();
        for segment in hint.split('/') {
            if segment.is_empty() {
                continue;
            }
            if segment.contains(['<', '>', '*', '?']) {
                break;
            }
            if let Some(name) = segment.strip_prefix('$') {
                match TREE_ENV.iter().find(|(var, _)| *var == name) {
                    Some((_, bound)) => segments.extend(bound.split('/')),
                    None => panic!("{label} declares unbound env var ${name}"),
                }
            } else if segment == "~" {
                segments.push("home");
            } else {
                segments.push(segment);
            }
        }
        let bound = segments.join("/");
        assert!(!bound.is_empty(), "{label} hint {hint:?} binds no path");
        bound
    }

    /// Every artifact root the overlap adapters declare, resolved against
    /// `TREE_ENV` inside the planted tree.
    fn declared_bound_roots(root: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
        OVERLAP_HARNESS_IDS
            .iter()
            .filter_map(|id| {
                overlap_adapter(id)
                    .usage_surface()
                    .map(|surface| (*id, surface))
            })
            .flat_map(|(id, surface)| {
                surface
                    .artifacts
                    .iter()
                    .map(|artifact| {
                        let resolver = &artifact.path_resolver;
                        let hint = resolver.linux.as_deref().unwrap_or(&resolver.fallback);
                        let label = format!("{id}/{}", artifact.id);
                        let bound = root.join(bound_root(&label, hint));
                        (label, bound)
                    })
                    .collect::<Vec<(String, std::path::PathBuf)>>()
            })
            .collect()
    }

    #[test]
    fn declaring_usage_surfaces_never_writes_disk() {
        if std::env::var_os(SWEEP_ENV).is_some() {
            let mut swept = 0usize;
            for id in OVERLAP_HARNESS_IDS {
                let adapter = overlap_adapter(id);
                assert!(
                    adapter.usage_surface().is_some() || adapter.usage_absence_reason().is_some(),
                    "{id} declared nothing under the bound roots"
                );
                swept += 1;
            }
            if let Some(out) = std::env::var_os(SWEEP_OUT_ENV) {
                fs::write(out, swept.to_string()).unwrap();
            }
            return;
        }

        let root = temp_dir_unique("usage-never-writes");
        let proof_root = temp_dir_unique("usage-never-writes-proof");
        build_bound_tree(&root);
        let before = snapshot_tree(&root);
        let declared = declared_bound_roots(&root);
        assert!(
            !declared.is_empty(),
            "no overlap adapter declared a bound root"
        );
        for (label, expected) in &declared {
            assert!(
                before
                    .keys()
                    .any(|file| std::path::Path::new(file).starts_with(expected)),
                "{label} declares bound root {} with no fixture in the planted tree",
                expected.display()
            );
        }
        for file in before.keys() {
            assert!(
                declared
                    .iter()
                    .any(|(_, expected)| std::path::Path::new(file).starts_with(expected)),
                "fixture {file} is claimed by no declared usage root"
            );
        }

        // Env is process-global and sibling tests read HOME, so the sweep
        // runs in a child process whose env points at the observed tree.
        let proof = proof_root.join("swept");
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(NEVER_WRITES_TEST)
            .envs(tree_env_for(&root))
            .env(SWEEP_ENV, "1")
            .env(SWEEP_OUT_ENV, &proof)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "sweep child failed: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        let swept: usize = fs::read_to_string(&proof)
            .unwrap_or_default()
            .trim()
            .parse()
            .unwrap_or(0);
        assert_eq!(
            swept,
            OVERLAP_HARNESS_IDS.len(),
            "sweep child must have exercised every overlap adapter"
        );

        let after = snapshot_tree(&root);
        assert_eq!(
            after, before,
            "usage declarations must not write inside the bound roots"
        );
        drop(fs::remove_dir_all(&root));
        drop(fs::remove_dir_all(&proof_root));
    }
}
