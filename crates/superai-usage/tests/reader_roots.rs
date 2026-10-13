//! Local-reader root-resolution tests (USG-03): env overrides, fallback
//! order, swap slots, profiles, dedup, and selector refusals.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use jiff as _;
    use rusqlite as _;
    use serde as _;
    use serde_json as _;
    use superai_core as _;
    use superai_usage::reader::jsonl::{
        PiSelectors, claude_projects_roots, codex_sessions_roots, pi_session_roots,
    };
    use thiserror as _;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-reader-roots-{tag}-{}-{n}-{nanos}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
        dir
    }

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    fn canonical(path: &Path) -> PathBuf {
        path.canonicalize()
            .unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
    }

    #[test]
    fn claude_config_dir_wins_over_every_derived_root() {
        let home = temp_dir("claude-env-home");
        assert_eq!(
            claude_projects_roots(Some("/custom/claude"), None, &home),
            vec![PathBuf::from("/custom/claude/projects")]
        );
    }

    #[test]
    fn claude_without_env_lists_the_config_fallback_first() {
        let home = temp_dir("claude-default-home");
        assert_eq!(
            claude_projects_roots(None, None, &home),
            vec![
                home.join(".config").join("claude").join("projects"),
                home.join(".claude").join("projects"),
            ]
        );
        assert_eq!(
            claude_projects_roots(Some(""), None, &home),
            vec![
                home.join(".config").join("claude").join("projects"),
                home.join(".claude").join("projects"),
            ]
        );
    }

    #[test]
    fn claude_swap_slots_with_positive_numeric_prefixes_join_the_roots() {
        let home = temp_dir("claude-swap-home");
        let sessions = home.join(".claude-swap-backup").join("sessions");
        fs::create_dir_all(sessions.join("1-alpha").join("projects"))
            .unwrap_or_else(|e| panic!("slot: {e}"));
        fs::create_dir_all(sessions.join("0-zero").join("projects"))
            .unwrap_or_else(|e| panic!("slot: {e}"));
        fs::create_dir_all(sessions.join("2-beta")).unwrap_or_else(|e| panic!("slot: {e}"));
        fs::create_dir_all(sessions.join("notaslot").join("projects"))
            .unwrap_or_else(|e| panic!("slot: {e}"));
        let roots = claude_projects_roots(None, None, &home);
        assert_eq!(
            roots,
            vec![
                home.join(".config").join("claude").join("projects"),
                home.join(".claude").join("projects"),
                canonical(&sessions.join("1-alpha").join("projects")),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_symlinked_duplicate_roots_collapse_to_one() {
        let home = temp_dir("claude-dedup-home");
        let real = home.join(".claude").join("projects");
        let linked = home.join(".config").join("claude").join("projects");
        fs::create_dir_all(&real).unwrap_or_else(|e| panic!("real: {e}"));
        fs::create_dir_all(linked.parent().unwrap()).unwrap_or_else(|e| panic!("base: {e}"));
        std::os::unix::fs::symlink(&real, &linked).unwrap_or_else(|e| panic!("symlink: {e}"));
        let roots = claude_projects_roots(None, None, &home);
        let resolved: Vec<PathBuf> = roots.iter().map(|root| canonical(root)).collect();
        assert_eq!(resolved, vec![canonical(&real)]);
    }

    #[test]
    fn codex_roots_follow_codex_home_and_add_the_archive_sibling() {
        let home = temp_dir("codex-home");
        assert_eq!(
            codex_sessions_roots(Some("/opt/codex"), &home),
            vec![
                PathBuf::from("/opt/codex/sessions"),
                PathBuf::from("/opt/codex/archived_sessions"),
            ]
        );
        assert_eq!(
            codex_sessions_roots(Some("  "), &home),
            vec![
                home.join(".codex").join("sessions"),
                home.join(".codex").join("archived_sessions")
            ]
        );
        assert_eq!(
            codex_sessions_roots(None, &home),
            vec![
                home.join(".codex").join("sessions"),
                home.join(".codex").join("archived_sessions")
            ]
        );
    }

    #[test]
    fn pi_explicit_session_dir_wins_outright() {
        let home = temp_dir("pi-session-dir");
        let selectors = PiSelectors {
            session_dir: Some("/tmp/pi-sessions"),
            ..PiSelectors::default()
        };
        assert_eq!(
            pi_session_roots(&selectors, &home).roots,
            vec![PathBuf::from("/tmp/pi-sessions")]
        );
    }

    #[test]
    fn pi_agent_dir_replaces_the_default_agent_roots() {
        let home = temp_dir("pi-agent-dir");
        let selectors = PiSelectors {
            agent_dir: Some("/agent/root"),
            ..PiSelectors::default()
        };
        let resolved = pi_session_roots(&selectors, &home);
        assert_eq!(resolved.refusal, None);
        assert_eq!(resolved.roots, vec![PathBuf::from("/agent/root/sessions")]);
    }

    #[test]
    fn pi_default_selectors_cover_the_agent_stores_and_existing_xdg_root() {
        let home = fixtures().join("pi/home");
        let xdg = fixtures().join("pi/xdg");
        let selectors = PiSelectors {
            xdg_data_home: Some(&xdg),
            ..PiSelectors::default()
        };
        let resolved = pi_session_roots(&selectors, &home);
        assert_eq!(resolved.refusal, None);
        assert_eq!(
            resolved.roots,
            vec![
                canonical(&home.join(".pi").join("agent").join("sessions")),
                canonical(&home.join(".omp").join("agent").join("sessions")),
                canonical(&xdg.join("omp").join("sessions")),
            ]
        );
    }

    #[test]
    fn pi_named_profile_limits_omp_discovery_to_that_profile() {
        let home = fixtures().join("pi/home");
        let xdg = fixtures().join("pi/xdg");
        let selectors = PiSelectors {
            omp_profile: Some("work"),
            xdg_data_home: Some(&xdg),
            ..PiSelectors::default()
        };
        let resolved = pi_session_roots(&selectors, &home);
        assert_eq!(resolved.refusal, None);
        assert_eq!(
            resolved.roots,
            vec![
                canonical(&home.join(".pi").join("agent").join("sessions")),
                canonical(
                    &home
                        .join(".omp")
                        .join("profiles")
                        .join("work")
                        .join("sessions")
                ),
            ]
        );
    }

    #[test]
    fn pi_named_profile_without_directories_keeps_a_placeholder_root() {
        let home = temp_dir("pi-ghost-profile");
        let selectors = PiSelectors {
            omp_profile: Some("ghost"),
            ..PiSelectors::default()
        };
        let resolved = pi_session_roots(&selectors, &home);
        assert_eq!(resolved.refusal, None);
        assert_eq!(
            resolved.roots,
            vec![
                home.join(".pi").join("agent").join("sessions"),
                home.join(".omp")
                    .join("profiles")
                    .join("ghost")
                    .join("agent")
                    .join("sessions"),
            ]
        );
    }

    #[test]
    fn pi_invalid_selectors_refuse_instead_of_reading_the_wrong_tree() {
        let home = temp_dir("pi-refusals");
        let profile = PiSelectors {
            omp_profile: Some("../escape"),
            ..PiSelectors::default()
        };
        assert_eq!(
            pi_session_roots(&profile, &home),
            superai_usage::reader::jsonl::PiSessionRoots {
                roots: Vec::new(),
                refusal: Some("OMP_PROFILE/PI_PROFILE is not a valid profile name"),
            }
        );
        for config_dir in ["/abs/root", "../../etc", ".."] {
            let selectors = PiSelectors {
                config_dir: Some(config_dir),
                ..PiSelectors::default()
            };
            let resolved = pi_session_roots(&selectors, &home);
            assert_eq!(
                resolved.refusal,
                Some("PI_CONFIG_DIR must be a relative path inside HOME"),
                "`{config_dir}` must be refused"
            );
            assert!(resolved.roots.is_empty());
        }
    }
}
