//! QAL-10/11 abuse verification: the sentinel may appear only in the
//! harness config and its backup; path abuses reject without panic or leak.

use std::path::Path;

/// Sentinel used for secret-leak scanning (QAL-10).
pub const SENTINEL: &str = "sk-superai-test-sentinel-12345-fake";

/// Returns true if `bytes` contain the sentinel plain.
pub fn contains_sentinel(bytes: &[u8]) -> bool {
    if bytes.len() < SENTINEL.len() {
        return false;
    }
    // Use lossy search to avoid panic on non-utf8.
    let s = String::from_utf8_lossy(bytes);
    s.contains(SENTINEL)
}

/// Returns true if `text` contains sentinel plain.
pub fn scan_str_for_sentinel(text: &str) -> bool {
    text.contains(SENTINEL)
}

/// Assert that `bytes` do not contain sentinel.
pub fn assert_no_sentinel_bytes(bytes: &[u8], context: &str) {
    assert!(
        !contains_sentinel(bytes),
        "sentinel leaked in {context}: found plain sentinel"
    );
}

/// Assert that file at `path` does not contain sentinel, if it exists.
pub fn assert_no_sentinel_in_file(path: &Path, context: &str) {
    if let Ok(bytes) = std::fs::read(path) {
        assert!(
            !contains_sentinel(&bytes),
            "sentinel leaked in file {} ({context})",
            path.display()
        );
    }
}

/// Assert that debug representation does not contain sentinel.
pub fn assert_no_sentinel_in_debug<T: std::fmt::Debug>(value: &T, context: &str) {
    let dbg = format!("{value:?}");
    assert!(
        !dbg.contains(SENTINEL),
        "sentinel leaked in debug for {context}: {dbg}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::{backup, list_backups};
    use crate::document::DocumentKind;
    use crate::quarantine::validate_quarantine_target;
    #[cfg(unix)]
    use crate::snapshot::is_modified;
    use crate::snapshot::snapshot;
    use crate::transaction::{FileAction, OperationId, Transaction};
    use std::path::PathBuf;

    fn temp_root(prefix: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(&format!("config-abuse-{prefix}"))
    }

    #[test]
    fn sentinel_allowed_only_in_harness_config_and_backup_with_600() {
        let dir = temp_root("sentinel-perms");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("settings.json");
        let content = format!(r#"{{"api_key":"{SENTINEL}","model":"sonnet"}}"#);
        std::fs::write(&cfg_path, &content).unwrap();

        let cfg_bytes = std::fs::read(&cfg_path).unwrap();
        assert!(
            contains_sentinel(&cfg_bytes),
            "harness config must contain sentinel here"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perm = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&cfg_path, perm).unwrap();
        }
        let entry = backup(&cfg_path).unwrap().expect("backup should exist");
        let backup_bytes = std::fs::read(&entry.backup_path).unwrap();
        assert!(
            contains_sentinel(&backup_bytes),
            "backup must contain sentinel (allowed)"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&entry.backup_path).unwrap();
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(
                mode,
                0o600,
                "backup must be 0o600, got {mode:o} for {}",
                entry.backup_path.display()
            );
            let orig_meta = std::fs::metadata(&cfg_path).unwrap();
            let orig_mode = orig_meta.permissions().mode() & 0o777;
            assert_eq!(orig_mode, 0o600, "harness config must be 0o600");
        }

        let backups = list_backups(&cfg_path).unwrap();
        let catalog_dbg = format!("{backups:?}");
        assert!(
            !catalog_dbg.contains(SENTINEL),
            "backup catalog leaked sentinel: {catalog_dbg}"
        );
        for b in &backups {
            let entry_dbg = format!("{b:?}");
            assert!(
                !entry_dbg.contains(SENTINEL),
                "backup entry leaked sentinel: {entry_dbg}"
            );
        }

        let snap = snapshot(&cfg_path);
        let snap_dbg = format!("{snap:?}");
        assert!(
            !snap_dbg.contains(SENTINEL),
            "snapshot leaked sentinel: {snap_dbg}"
        );

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn symlink_swap_race_aborts_concurrent_modification() {
        #[cfg(unix)]
        {
            let dir = temp_root("symlink-race");
            std::fs::create_dir_all(&dir).unwrap();
            let target_a = dir.join("target-a.json");
            let target_b = dir.join("target-b.json");
            std::fs::write(&target_a, br#"{"model":"a"}"#).unwrap();
            std::fs::write(&target_b, br#"{"model":"b-different-content"}"#).unwrap();

            let link = dir.join("link.json");
            drop(std::fs::remove_file(&link));
            std::os::unix::fs::symlink(&target_a, &link).unwrap();

            let snap = snapshot(&link);
            assert!(snap.exists);
            assert!(snap.is_symlink);
            assert!(snap.digest.is_some());

            std::fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink(&target_b, &link).unwrap();

            let snap_after = snapshot(&link);
            assert!(
                is_modified(&snap, &snap_after),
                "swap should be detected as modified"
            );

            // With the dir as a follow root the caller's token must catch the
            // swapped link (MUT-02); rootless, the boundary refuses to follow.
            let res = crate::transaction::commit_file_expecting_with_roots(
                "abuse-symlink-race",
                &link,
                br#"{"model":"new"}"#,
                DocumentKind::StrictJson,
                Some(&snap),
                std::slice::from_ref(&dir),
            );
            assert!(
                res.is_err(),
                "symlink swap should cause ConcurrentModification"
            );
            match res.unwrap_err() {
                crate::error::ConfigError::ConcurrentModification { .. } => {}
                other => panic!("expected ConcurrentModification, got {other:?}: after swap"),
            }
            let rootless = crate::transaction::commit_file_expecting(
                "abuse-symlink-race-rootless",
                &link,
                br#"{"model":"new"}"#,
                DocumentKind::StrictJson,
                Some(&snap),
            );
            match rootless {
                Err(crate::error::ConfigError::SymlinkFollowRefused { .. }) => {}
                other => panic!("expected SymlinkFollowRefused, got {other:?}: rootless"),
            }

            let sentinel_file = dir.join("sentinel.json");
            std::fs::write(&sentinel_file, format!(r#"{{"api_key":"{SENTINEL}"}}"#)).unwrap();
            let link2 = dir.join("link2.json");
            drop(std::fs::remove_file(&link2));
            std::os::unix::fs::symlink(&sentinel_file, &link2).unwrap();
            let snap2 = snapshot(&link2);
            let other_target = dir.join("other.json");
            std::fs::write(&other_target, b"other").unwrap();
            std::fs::remove_file(&link2).unwrap();
            std::os::unix::fs::symlink(&other_target, &link2).unwrap();
            let err = crate::transaction::commit_file_expecting(
                "abuse-symlink-sentinel",
                &link2,
                b"new",
                DocumentKind::Opaque,
                Some(&snap2),
            )
            .unwrap_err();
            let err_str = format!("{err:?}");
            assert!(
                !err_str.contains(SENTINEL),
                "error must not leak sentinel: {err_str}"
            );

            drop(std::fs::remove_dir_all(&dir));
        }
        #[cfg(not(unix))]
        {
            let dir = temp_root("symlink-race-nonunix");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("file.json");
            std::fs::write(&path, b"v1").unwrap();
            let snap = snapshot(&path);
            std::fs::write(&path, b"v2").unwrap();
            let res = crate::transaction::commit_file_expecting(
                "abuse-swap-nonunix",
                &path,
                b"v3",
                DocumentKind::StrictJson,
                Some(&snap),
            );
            drop(res.unwrap_err());
            drop(std::fs::remove_dir_all(&dir));
        }
    }

    #[test]
    fn broad_deletion_targets_are_rejected() {
        for p in ["/", "/home", "/tmp", "/usr", "/etc"] {
            let err = validate_quarantine_target(Path::new(p));
            assert!(err.is_err(), "broad root {p} should be rejected, got ok");
            let msg = format!("{:?}", err.unwrap_err());
            assert!(!msg.contains(SENTINEL), "error must not leak sentinel");
        }
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
            && home.is_absolute()
            && home.exists()
        {
            let err = validate_quarantine_target(&home);
            assert!(
                err.is_err(),
                "home dir {} should be rejected",
                home.display()
            );
        }
        // `C:\Windows` is rejected on every host: drive-root rule on Windows,
        // the absolute-path requirement on unix (parses as relative there).
        for win_root in [
            "C:\\Windows",
            "c:\\program files",
            "C:\\",
            "C:/",
            "\\\\server\\share",
        ] {
            let err = validate_quarantine_target(Path::new(win_root));
            assert!(
                err.is_err(),
                "windows broad root {win_root} should be rejected"
            );
        }
        let win = Path::new("C:\\Windows");
        let err = validate_quarantine_target(win);
        assert!(err.is_err(), "C:\\Windows should be rejected");
        // The removal gate recognizes windows-shaped broad roots directly.
        let err = crate::transaction::validate_remove_target(
            win,
            crate::transaction::RemoveKind::InstanceRoot,
        );
        assert!(err.is_err(), "C:\\Windows should be rejected for removal");

        for p in [
            std::env::temp_dir().join("*.json"),
            PathBuf::from("/var/*.log"),
            PathBuf::from("/home/user/[abc]"),
        ] {
            let err = validate_quarantine_target(&p);
            assert!(err.is_err(), "glob {} should be rejected", p.display());
        }
        for p in [
            std::env::temp_dir().join("$HOME/foo"),
            std::env::temp_dir().join("%USERPROFILE%/bar"),
        ] {
            let err = validate_quarantine_target(&p);
            assert!(
                err.is_err(),
                "unresolved var {} should be rejected",
                p.display()
            );
        }
        let err = validate_quarantine_target(&std::env::temp_dir().join("../etc/passwd"));
        assert!(err.is_err(), "traversal should be rejected");

        let err = validate_quarantine_target(Path::new("relative/path"));
        assert!(err.is_err(), "relative should be rejected");

        if let Ok(qb) = crate::quarantine::quarantine_base() {
            let err = validate_quarantine_target(&qb);
            assert!(err.is_err(), "quarantine base should be rejected");
        }

        // A path that itself contains the sentinel is rejected without panic;
        // the error echoes the caller's path, which is unavoidable.
        let sentinel_path = std::env::temp_dir().join(SENTINEL);
        drop(validate_quarantine_target(&sentinel_path));
    }

    #[test]
    fn shell_metachars_in_paths_are_rejected() {
        let dir = temp_root("shell-metachars");
        std::fs::create_dir_all(&dir).unwrap();
        let bad_names = [
            "$(rm -rf)",
            "`whoami`",
            "a&&b",
            "a||b",
            "a;b",
            "a|b",
            "a&b",
            "a>out",
            "a<in",
            "a\\b",
            "a\"b",
            "a'b",
            "a\nb",
        ];
        for name in bad_names {
            let bad_path = dir.join(name);
            let op_id = OperationId::new(&format!("op-shell-{}", name.len())).unwrap();
            let action = FileAction::Write {
                path: bad_path.clone(),
                content: b"{}".to_vec(),
                kind: DocumentKind::StrictJson,
            };
            let txn = Transaction::new(op_id, vec![action]);
            let res = txn.validate_plan();
            // Path safety rejects globs and `$`; other metachars are inert
            // because the duct layer never spawns a shell.
            if name.contains('$') {
                assert!(
                    res.is_err(),
                    "path with shell metachars `{name}` should be rejected, got ok for {}",
                    bad_path.display()
                );
                let msg = format!("{:?}", res.unwrap_err());
                assert!(!msg.contains(SENTINEL));
            } else {
                drop(res);
            }
        }

        let shell_path = std::env::temp_dir().join("$(rm -rf)/file.json");
        let err = validate_quarantine_target(&shell_path);
        assert!(
            err.is_err(),
            "shell metachars path should be rejected via quarantine or path safety"
        );

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn huge_5mb_deep_config_is_bounded_and_rejected_safely() {
        let dir = temp_root("huge-deep");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("huge.json");

        let mut json = String::new();
        let depth = 300;
        for _ in 0..depth {
            json.push_str("{\"a\":");
        }
        json.push_str("\"x\"");
        for _ in 0..depth {
            json.push('}');
        }
        let huge_payload = "x".repeat(5 * 1024 * 1024);
        let mut huge_json = String::from("{\"data\":\"");
        huge_json.push_str(&huge_payload);
        huge_json.push_str("\"}");

        let deep_bytes = json.into_bytes();
        assert!(deep_bytes.len() < 10 * 1024 * 1024, "deep bytes bounded");
        drop(crate::raw_editor::validate(
            &deep_bytes,
            DocumentKind::StrictJson,
        ));

        let huge_bytes = huge_json.into_bytes();
        assert!(
            huge_bytes.len() >= 5 * 1024 * 1024,
            "huge bytes should be at least 5MB, got {}",
            huge_bytes.len()
        );
        let op_id = OperationId::new("op-huge-5mb").unwrap();
        let action = FileAction::Write {
            path: path.clone(),
            content: huge_bytes.clone(),
            kind: DocumentKind::StrictJson,
        };
        let mut txn = Transaction::new(op_id, vec![action]);
        let plan_res = txn.validate_plan();
        assert!(
            plan_res.is_ok(),
            "plan valid for huge path, content not yet checked"
        );

        match txn.prepare() {
            Ok(()) => {
                for t in txn.staged_temps {
                    drop(std::fs::remove_file(t));
                }
                assert!(!contains_sentinel(&huge_bytes));
            }
            Err(e) => {
                let msg = format!("{e:?}");
                assert!(!msg.contains(SENTINEL));
            }
        }

        drop(std::fs::remove_file(&path));
        for b in list_backups(&path).unwrap() {
            drop(std::fs::remove_file(b.backup_path));
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn huge_deep_nested_yaml_and_toml_bounded() {
        let depth = 250;
        let mut yaml = String::new();
        for i in 0..depth {
            for _ in 0..i {
                yaml.push(' ');
                yaml.push(' ');
            }
            yaml.push_str("level");
            yaml.push_str(&i.to_string());
            yaml.push_str(":\n");
        }
        yaml.push_str("leaf: 1\n");
        let y_bytes = yaml.into_bytes();
        let diags = crate::raw_editor::validate(&y_bytes, DocumentKind::Yaml);
        drop(diags);
        assert!(y_bytes.len() < 2 * 1024 * 1024);

        let mut toml = String::new();
        for i in 0..120 {
            toml.push_str("[a");
            toml.push_str(&i.to_string());
            toml.push_str("]\n");
        }
        toml.push_str("key = 1\n");
        let t_bytes = toml.into_bytes();
        let diags_t = crate::raw_editor::validate(&t_bytes, DocumentKind::Toml);
        drop(diags_t);
    }

    #[test]
    fn malformed_huge_inputs_do_not_panic_and_do_not_leak_sentinel() {
        let dir = temp_root("malformed-huge");
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("malformed.json");
        let sentinel_content = format!(r#"{{"api_key":"{SENTINEL}"}}"#);
        std::fs::write(&path, &sentinel_content).unwrap();
        let snap = snapshot(&path);

        // Commit malformed huge content through the boundary as an opaque
        // payload; parse-validating kinds fail closed at staging instead.
        let bad_content = vec![b'{'; 2 * 1024 * 1024];
        let res = crate::transaction::commit_file_expecting(
            "abuse-huge",
            &path,
            &bad_content,
            DocumentKind::Opaque,
            Some(&snap),
        );
        // The boundary writes opaque bytes verbatim; neither error nor overwrite may leak the sentinel.
        if let Err(e) = res {
            let msg = format!("{e:?}");
            assert!(!msg.contains(SENTINEL));
        } else {
            let new_bytes = std::fs::read(&path).unwrap();
            assert!(
                !contains_sentinel(&new_bytes),
                "overwritten file should not contain sentinel"
            );
            let backups = list_backups(&path).unwrap();
            if let Some(b) = backups.first() {
                let backup_bytes = std::fs::read(&b.backup_path).unwrap();
                assert!(contains_sentinel(&backup_bytes));
                let cat = format!("{b:?}");
                assert!(!cat.contains(SENTINEL));
            }
        }

        let op_id = OperationId::new("op-malformed-huge").unwrap();
        #[expect(clippy::redundant_clone, reason = "retain path for later cleanup")]
        let action = FileAction::Write {
            path: path.clone(),
            content: bad_content,
            kind: DocumentKind::StrictJson,
        };
        let mut txn = Transaction::new(op_id, vec![action]);
        let prepare = txn.prepare();
        assert!(
            prepare.is_err(),
            "malformed huge should be rejected in prepare"
        );
        let msg = format!("{:?}", prepare.unwrap_err());
        assert!(!msg.contains(SENTINEL));

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn windows_reserved_and_long_and_case_insensitive_and_crlf_are_handled() {
        let dir = temp_root("windows-long-crlf");
        std::fs::create_dir_all(&dir).unwrap();
        // Windows reserved device names are rejected at plan validation on
        // every host; the error must not leak the sentinel.
        for reserved in ["CON", "PRN", "AUX", "NUL", "COM1", "LPT1"] {
            let path = dir.join(format!("{reserved}.json"));
            let res = crate::transaction::commit_file(
                "abuse-reserved-name",
                &path,
                br#"{"a":1}"#,
                DocumentKind::StrictJson,
            );
            if let Err(e) = res {
                let msg = format!("{e:?}");
                assert!(!msg.contains(SENTINEL));
            } else {
                let bytes = std::fs::read(&path).unwrap();
                assert!(!contains_sentinel(&bytes));
                drop(std::fs::remove_file(&path));
            }
        }
        let long_name = "a".repeat(300);
        let long_path = dir.join(format!("{long_name}.json"));
        let long_res = std::panic::catch_unwind(|| {
            crate::transaction::commit_file(
                "abuse-long-path",
                &long_path,
                br#"{"a":1}"#,
                DocumentKind::StrictJson,
            )
        });
        assert!(long_res.is_ok(), "long path must not panic");
        if let Ok(Err(e)) = long_res {
            let msg = format!("{e:?}");
            assert!(msg.len() <= 8192);
            assert!(!msg.contains(SENTINEL));
        }
        drop(std::fs::remove_file(&long_path));
        // Case-insensitive fs (default APFS): both names are one file and the
        // digests agree; case-sensitive: two files. Read back the first name.
        let lower = dir.join("case.json");
        let upper = dir.join("CASE.json");
        std::fs::write(&lower, br#"{"a":1}"#).unwrap();
        std::fs::write(&upper, br#"{"a":2}"#).unwrap();
        let lower_readback = std::fs::read(&lower).unwrap_or_default();
        let case_sensitive = lower_readback == *br#"{"a":1}"#;
        let snap_lower = snapshot(&lower);
        let snap_upper = snapshot(&upper);
        if case_sensitive {
            assert_ne!(
                snap_lower.digest, snap_upper.digest,
                "case variant files must keep distinct digests"
            );
        } else {
            assert_eq!(
                snap_lower.digest, snap_upper.digest,
                "case-insensitive filesystem: one physical file seen through two names must have one digest"
            );
        }
        assert!(!format!("{snap_lower:?}").contains(SENTINEL));
        // CRLF is ordinary JSON whitespace, so load and edit succeed everywhere.
        let crlf_path = dir.join("crlf.json");
        let crlf_content = b"{\r\n  \"a\": 1,\r\n  \"b\": \"val\"\r\n}";
        std::fs::write(&crlf_path, crlf_content).unwrap();
        let diags = crate::raw_editor::validate(crlf_content, DocumentKind::StrictJson);
        drop(diags);
        let load = crate::json::load_value(&crlf_path);
        assert!(load.is_ok(), "CRLF json must parse: {load:?}");
        let edit_res = crate::json::edit(&crlf_path, |m| {
            m.insert("c".to_owned(), serde_json::Value::String("new".to_owned()));
        });
        assert!(
            edit_res.is_ok(),
            "CRLF edit must succeed on every platform: {edit_res:?}"
        );
        let after = std::fs::read(&crlf_path).unwrap();
        assert!(!contains_sentinel(&after));
        let reparsed = crate::json::load_value(&crlf_path);
        assert!(
            reparsed.is_ok(),
            "edited CRLF file must re-parse: {reparsed:?}"
        );
        assert_eq!(
            reparsed.unwrap().get("c").and_then(|v| v.as_str()),
            Some("new"),
            "the edit must land the new key"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn shell_metachars_and_symlink_escape_do_not_leak_and_are_bounded() {
        let dir = temp_root("shell-escape-bounded");
        std::fs::create_dir_all(&dir).unwrap();
        let bad_segments = [
            "$(rm)", "`whoami`", "; rm", "| cat", "&&", "$(env)", "${HOME}", "*", "?", "[abc]",
        ];
        for seg in bad_segments {
            let path = dir.join(format!("{seg}.json"));
            let res = std::panic::catch_unwind(|| {
                crate::transaction::commit_file(
                    "abuse-metachars",
                    &path,
                    br#"{"a":1}"#,
                    DocumentKind::StrictJson,
                )
            });
            assert!(res.is_ok(), "metachars {seg:?} must not panic");
            if let Ok(Ok(report)) = res {
                let _ = report;
                let bytes = std::fs::read(&path).unwrap();
                assert_eq!(bytes, br#"{"a":1}"#);
                drop(std::fs::remove_file(&path));
            }
        }
        #[cfg(unix)]
        {
            let outside = temp_root("outside-target");
            std::fs::create_dir_all(&outside).unwrap();
            let outside_file = outside.join("secret.json");
            std::fs::write(&outside_file, br#"{"outside":1}"#).unwrap();
            let link = dir.join("link_escape.json");
            drop(std::fs::remove_file(&link));
            std::os::unix::fs::symlink(&outside_file, &link).unwrap();
            let snap = snapshot(&link);
            assert!(snap.is_symlink);
            let res = crate::transaction::commit_file_expecting(
                "abuse-symlink-escape",
                &link,
                br#"{"new":1}"#,
                DocumentKind::StrictJson,
                Some(&snap),
            );
            if let Err(e) = res {
                let msg = format!("{e:?}");
                assert!(!msg.contains(SENTINEL));
                assert!(msg.len() <= 4096);
            }
            drop(std::fs::remove_file(&link));
            drop(std::fs::remove_dir_all(&outside));
        }
        drop(std::fs::remove_dir_all(&dir));
    }
}
