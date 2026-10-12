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

    #[test]
    fn declaring_usage_surfaces_never_writes_disk() {
        let dir = temp_dir_unique("usage-ledger");
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("nested/sentinel.txt"), b"bytes that must not move").unwrap();
        fs::write(dir.join("top.txt"), b"top-level bytes").unwrap();

        for id in OVERLAP_HARNESS_IDS {
            let adapter = overlap_adapter(id);
            assert!(
                adapter.usage_surface().is_some() || adapter.usage_absence_reason().is_some(),
                "{id} declared nothing while the disk state was being read"
            );
        }

        assert_eq!(
            fs::read(dir.join("nested/sentinel.txt")).unwrap(),
            b"bytes that must not move"
        );
        assert_eq!(fs::read(dir.join("top.txt")).unwrap(), b"top-level bytes");
        let mut listed: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        listed.sort();
        assert_eq!(listed, vec!["nested", "top.txt"]);
        let nested: Vec<String> = fs::read_dir(dir.join("nested"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(nested, vec!["sentinel.txt"]);
        drop(fs::remove_dir_all(&dir));
    }
}
