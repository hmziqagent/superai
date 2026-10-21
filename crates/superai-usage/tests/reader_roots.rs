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
    use ureq as _;

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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn claude_swap_slots_join_the_roots_even_when_claude_config_dir_is_set() {
        let home = temp_dir("claude-env-swap-home");
        let xdg = temp_dir("claude-env-swap-xdg");
        let legacy = home.join(".claude-swap-backup").join("sessions");
        fs::create_dir_all(legacy.join("7-live").join("projects"))
            .unwrap_or_else(|e| panic!("legacy slot: {e}"));
        let xdg_slot = xdg.join("claude-swap").join("sessions").join("2-two");
        fs::create_dir_all(xdg_slot.join("projects")).unwrap_or_else(|e| panic!("xdg slot: {e}"));
        assert_eq!(
            claude_projects_roots(Some("/custom/claude"), Some(&xdg), &home),
            vec![
                PathBuf::from("/custom/claude/projects"),
                canonical(&legacy.join("7-live").join("projects")),
                canonical(&xdg_slot.join("projects")),
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
    fn pi_default_selectors_cover_agent_stores_xdg_and_on_disk_profiles() {
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
    fn pi_explicit_default_profile_still_limits_discovery_to_the_agent_stores() {
        let home = fixtures().join("pi/home");
        let xdg = fixtures().join("pi/xdg");
        let selectors = PiSelectors {
            omp_profile: Some("default"),
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

    #[cfg(unix)]
    #[test]
    fn pi_config_dir_through_an_escaping_symlink_is_refused() {
        let home = temp_dir("pi-config-symlink");
        let outside = temp_dir("pi-config-outside");
        fs::create_dir_all(home.join("mountpoint")).unwrap_or_else(|e| panic!("mountpoint: {e}"));
        std::os::unix::fs::symlink(&outside, home.join("escape"))
            .unwrap_or_else(|e| panic!("symlink: {e}"));
        let selectors = PiSelectors {
            config_dir: Some("escape"),
            ..PiSelectors::default()
        };
        let resolved = pi_session_roots(&selectors, &home);
        assert_eq!(
            resolved.refusal,
            Some("PI_CONFIG_DIR must be a relative path inside HOME")
        );
        assert!(resolved.roots.is_empty());
    }

    #[test]
    fn pi_profile_discovery_caps_deterministically_past_64_profiles() {
        let home = temp_dir("pi-many-profiles");
        for n in 0..70 {
            fs::create_dir_all(
                home.join(".omp")
                    .join("profiles")
                    .join(format!("p{n:02}"))
                    .join("sessions"),
            )
            .unwrap_or_else(|e| panic!("profile dir: {e}"));
        }
        let resolved = pi_session_roots(&PiSelectors::default(), &home);
        assert_eq!(resolved.refusal, None);
        assert!(
            resolved.profile_cap_hit,
            "dropping profiles past the cap must be flagged, not silent"
        );
        let discovered: Vec<String> = resolved
            .roots
            .iter()
            .filter(|root| root.components().any(|c| c.as_os_str() == "profiles"))
            .map(|root| {
                root.parent()
                    .and_then(Path::file_name)
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect();
        let expected: Vec<String> = (0..64).map(|n| format!("p{n:02}")).collect();
        assert_eq!(discovered.len(), 64);
        assert_eq!(
            discovered, expected,
            "the cap must keep the sorted-first profiles"
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
                profile_cap_hit: false,
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
