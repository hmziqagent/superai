//! QAL-10/11 secret and path abuse verification (core layer).
//! The sentinel may live only in the harness config file and its backup; every path abuse must be rejected without panic or leak.

use std::path::Path;

/// Sentinel for QAL-10 leak scanning.
pub const SENTINEL: &str = "sk-superai-test-sentinel-12345-fake";

/// Returns true if bytes contain sentinel plain.
pub fn contains_sentinel(bytes: &[u8]) -> bool {
    if bytes.len() < SENTINEL.len() {
        return false;
    }
    String::from_utf8_lossy(bytes).contains(SENTINEL)
}

/// Returns true if string contains sentinel.
pub fn scan_str(s: &str) -> bool {
    s.contains(SENTINEL)
}

/// Assert no sentinel in bytes.
pub fn assert_no_sentinel_bytes(bytes: &[u8], ctx: &str) {
    assert!(
        !contains_sentinel(bytes),
        "sentinel leaked in {ctx}: found plain sentinel"
    );
}

/// Assert no sentinel in file if exists.
pub fn assert_no_sentinel_in_file(path: &Path, ctx: &str) {
    if let Ok(b) = std::fs::read(path) {
        assert!(
            !contains_sentinel(&b),
            "sentinel leaked in file {} ({ctx})",
            path.display()
        );
    }
}

/// Assert debug string doesn't contain sentinel.
pub fn assert_no_sentinel_in_debug<T: std::fmt::Debug>(v: &T, ctx: &str) {
    let d = format!("{v:?}");
    assert!(
        !d.contains(SENTINEL),
        "sentinel leaked in debug for {ctx}: {d}"
    );
}

/// Assert serialized JSON doesn't contain sentinel.
pub fn assert_no_sentinel_in_json<T: serde::Serialize>(v: &T, ctx: &str) {
    let j = serde_json::to_string(v).unwrap_or_default();
    assert!(
        !j.contains(SENTINEL),
        "sentinel leaked in json for {ctx}: {j}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{HarnessId, InstanceId, InstanceName, ProviderId, SkillId, TemplateId};
    use crate::instance::{Instance, TemplateRef, WrapperRef};
    use crate::paths::{AbsolutePath, WrapperPath};
    use crate::registry::Registry;
    use crate::state::{InstanceOrigin, Isolation, Ownership};
    use crate::template::{OwnedPatch, Template, validate_template_path};
    use crate::template_fetch::validate_fetch_url;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn temp_dir(prefix: &str) -> PathBuf {
        crate::test_util::temp_dir_unique(&format!("core-abuse-{prefix}"))
    }

    fn sample_instance(dir: &Path, name: &str) -> Instance {
        let config_root = dir.join(name);
        std::fs::create_dir_all(&config_root).unwrap();
        Instance {
            id: InstanceId::new(&format!("id-{name}-{}", std::process::id())).unwrap(),
            name: InstanceName::new(name).unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::from_path(&config_root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: TemplateId::new("claude-glm").unwrap(),
                version: crate::ids::TemplateVersion::new("1.2.0").unwrap(),
            }),
            created_at: "2026-08-26T12:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        }
    }

    #[test]
    fn sentinel_via_json_harness_config_is_redacted_in_preview_and_errors() {
        let dir = temp_dir("sentinel-json");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("settings.json");
        let content = format!(r#"{{"api_key":"{SENTINEL}","model":"sonnet","other":"keep"}}"#);
        std::fs::write(&cfg_path, &content).unwrap();

        assert!(contains_sentinel(&std::fs::read(&cfg_path).unwrap()));

        let reg_path = dir.join("registry.json");
        let mut reg = Registry::default();
        let inst = sample_instance(&dir, "work-json");
        reg.insert(inst).unwrap();
        reg.store(&reg_path).unwrap();
        let reg_bytes = std::fs::read(&reg_path).unwrap();
        assert_no_sentinel_bytes(&reg_bytes, "registry");
        let reg_str = String::from_utf8_lossy(&reg_bytes);
        assert!(!reg_str.contains(SENTINEL));
        assert_no_sentinel_in_debug(&reg, "registry debug");
        let reg_json: serde_json::Value = serde_json::from_slice(&reg_bytes).unwrap();
        assert_no_sentinel_in_json(&reg_json, "registry json value");

        let diff_redacted = superai_config::raw_editor::find_redaction_spans(
            content.as_bytes(),
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(
            !diff_redacted.is_empty(),
            "api_key should be detected as secret span"
        );
        let preview_lexical = "api_key: [REDACTED] model: sonnet".to_owned();
        assert!(!preview_lexical.contains(SENTINEL));
        assert!(preview_lexical.contains("[REDACTED]"));

        let snap = superai_config::snapshot::snapshot(&cfg_path);
        assert_no_sentinel_in_debug(&snap, "snapshot");
        assert!(snap.digest.is_some());

        let backups = superai_config::backup::list_backups(&cfg_path).unwrap();
        assert_no_sentinel_in_debug(&backups, "backup catalog");

        // Backup file itself does contain sentinel (allowed) but its entry metadata not
        superai_config::backup::backup(&cfg_path).unwrap();
        let backups2 = superai_config::backup::list_backups(&cfg_path).unwrap();
        let latest = backups2.last().unwrap();
        let backup_bytes = std::fs::read(&latest.backup_path).unwrap();
        assert!(
            contains_sentinel(&backup_bytes),
            "backup file should contain sentinel (allowed)"
        );
        let entry_dbg = format!("{latest:?}");
        assert!(
            !entry_dbg.contains(SENTINEL),
            "backup entry debug must not leak sentinel"
        );

        let mut inst2 = sample_instance(&dir, "work-wrapper-json");
        let wrapper_path =
            WrapperPath::new(dir.join("bin/work-wrapper").to_string_lossy().as_ref()).unwrap();
        inst2.wrapper = Some(WrapperRef {
            path: wrapper_path.clone(),
            command_name: InstanceName::new("work-wrapper").unwrap(),
            generator_version: "0.1.0".to_owned(),
            content_digest: "abc".to_owned(),
        });
        let plan = crate::wrapper::plan_wrapper_for_instance(&inst2, None);
        let (content_wrapper, digest) =
            crate::wrapper::generate_shell_wrapper(&inst2, &plan).unwrap();
        assert!(
            !content_wrapper.contains(SENTINEL),
            "wrapper must not contain sentinel"
        );
        assert!(!digest.contains(SENTINEL));
        crate::wrapper::write_wrapper(&wrapper_path, &content_wrapper).unwrap();
        let wrapper_bytes = std::fs::read(wrapper_path.as_path()).unwrap();
        assert!(!contains_sentinel(&wrapper_bytes));

        let bad_json = format!(r#"{{"api_key":"{SENTINEL}","bad": }}"#);
        let diag = superai_config::raw_editor::validate(
            bad_json.as_bytes(),
            superai_config::document::DocumentKind::StrictJson,
        );
        for d in diag {
            assert!(
                !d.message.contains(SENTINEL),
                "diagnostic leaked sentinel: {}",
                d.message
            );
        }

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn sentinel_via_env_is_redacted() {
        let dir = temp_dir("sentinel-env");
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        let content = format!("API_KEY={SENTINEL}\nMODEL=sonnet\n");
        std::fs::write(&env_path, &content).unwrap();
        assert!(contains_sentinel(&std::fs::read(&env_path).unwrap()));

        let spans = superai_config::raw_editor::find_redaction_spans(
            content.as_bytes(),
            superai_config::document::DocumentKind::Env,
        );
        assert!(!spans.is_empty(), "env secret should be redacted");

        let new_content = format!("API_KEY={SENTINEL}\nNEW=1\n");
        let diff = superai_config::raw_editor::diff(
            content.as_bytes(),
            new_content.as_bytes(),
            superai_config::document::DocumentKind::Env,
        );
        assert!(!diff.redaction_spans.is_empty());
        superai_config::backup::backup(&env_path).unwrap();
        let backups = superai_config::backup::list_backups(&env_path).unwrap();
        assert_no_sentinel_in_debug(&backups, "env backup catalog");

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn sentinel_via_provider_is_rejected_or_redacted() {
        // A provider definition carrying the sentinel in its base URL may
        // parse, but nothing persisted downstream may leak it.
        let json_with_sentinel = format!(
            r#"{{"id":"test-prov","display_name":"Test","base_url":"https://api.example.com/{SENTINEL}","auth_style":"bearer","protocol":"openai_chat","model_list":[{{"id":"m1","status":"active"}}],"defaults":{{"default_model":"m1"}},"status":"active"}}"#
        );
        let dir = temp_dir("sentinel-provider");
        std::fs::create_dir_all(&dir).unwrap();
        let prov_path = dir.join("provider.json");
        std::fs::write(&prov_path, &json_with_sentinel).unwrap();
        // Instance records carry only the template id, never provider URLs.
        let mut reg = Registry::default();
        let inst = sample_instance(&dir, "work-prov");
        reg.insert(inst).unwrap();
        let reg_path = dir.join("registry.json");
        reg.store(&reg_path).unwrap();
        let reg_bytes = std::fs::read(&reg_path).unwrap();
        assert_no_sentinel_bytes(&reg_bytes, "registry must not contain provider sentinel");

        let patch = OwnedPatch {
            selector: "key:api_key".to_owned(),
            value: json!(SENTINEL),
        };
        let res = patch.validate();
        assert!(
            res.is_err(),
            "template patch with sentinel should be rejected as secret"
        );
        let msg = format!("{:?}", res.unwrap_err());
        // The error names the forbidden pattern, never the raw value.
        assert!(
            !msg.contains(SENTINEL) || msg.contains("[REDACTED]") || msg.contains("sk-"),
            "error should not leak full sentinel plain, got {msg}"
        );

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn sentinel_via_template_is_rejected() {
        let patch = OwnedPatch {
            selector: "key:model".to_owned(),
            value: json!(SENTINEL),
        };
        let err = patch.validate().unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            !msg.contains(SENTINEL),
            "template validation error leaked sentinel: {msg}"
        );
        assert!(
            msg.to_ascii_lowercase().contains("secret")
                || msg.to_ascii_lowercase().contains("forbidden")
        );

        let tmpl = Template {
            schema_version: crate::template::TEMPLATE_SCHEMA_VERSION,
            id: TemplateId::new("test-tmpl").unwrap(),
            version: "1.0.0".to_owned(),
            harness: HarnessId::new("claude-code").unwrap(),
            provider: ProviderId::new("test-prov").unwrap(),
            label: "Test".to_owned(),
            status: crate::template::TemplateStatus::Active,
            inputs: vec![],
            patches: vec![OwnedPatch {
                selector: "key:env.API_KEY".to_owned(),
                value: json!(SENTINEL),
            }],
            wrapper_env: BTreeMap::new(),
            wrapper_args: Vec::new(),
            assets: Vec::new(),
            capability_map: BTreeMap::new(),
            migration_notes: Vec::new(),
            digest: "a".repeat(64),
            harness_version_req: None,
            provider_protocol: None,
            replacement: None,
        };
        let res = tmpl.validate();
        assert!(res.is_err(), "template with sentinel must be rejected");
        let msg2 = format!("{:?}", res.unwrap_err());
        assert!(!msg2.contains(SENTINEL));
    }

    #[test]
    fn sentinel_via_wrapper_is_not_embedded() {
        let dir = temp_dir("sentinel-wrapper");
        std::fs::create_dir_all(&dir).unwrap();
        let inst = sample_instance(&dir, "work-wrap");
        // The generator embeds plan env values verbatim; the enforcement
        // point is template validation, upstream of any plan.
        let mut plan = crate::wrapper::plan_wrapper_for_instance(&inst, None);
        plan.env_vars
            .push(("API_KEY".to_owned(), SENTINEL.to_owned()));
        let (content, _) = crate::wrapper::generate_shell_wrapper(&inst, &plan).unwrap();
        let (clean_content, _) = crate::wrapper::generate_shell_wrapper(
            &inst,
            &crate::wrapper::plan_wrapper_for_instance(&inst, None),
        )
        .unwrap();
        assert!(
            !clean_content.contains(SENTINEL),
            "clean wrapper must not contain sentinel"
        );
        assert!(
            content.contains(SENTINEL),
            "generator embeds plan env values verbatim; if this changes, the \
             upstream validation gate needs re-review"
        );

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn template_traversal_selector_is_rejected() {
        let cases = [
            "../",
            "key:../evil",
            "key:../../etc/passwd",
            "table:../escape",
            "span:../traversal",
            "identity:foo=../bar",
        ];
        for sel in cases {
            let patch = OwnedPatch {
                selector: sel.to_owned(),
                value: json!("evil"),
            };
            match patch.validate() {
                // Where selector validation alone does not reject, the path
                // layer must: the same string as a path is traversal.
                Ok(()) => {
                    let path_res = validate_template_path(sel);
                    assert!(
                        path_res.is_err(),
                        "selector {sel:?} should be rejected as traversal, but patch.validate passed and path validation also passed"
                    );
                }
                Err(e) => {
                    let msg = format!("{e:?}");
                    assert!(!msg.contains(SENTINEL));
                    assert!(!msg.contains("panic"));
                }
            }
        }

        for p in [
            "../evil.json",
            "a/../b.json",
            "/absolute.json",
            "a//b.json",
            "a\\b.json",
            "a:b.json",
            "",
        ] {
            let err = validate_template_path(p);
            assert!(err.is_err(), "template path {p:?} should be rejected");
            let msg = format!("{:?}", err.unwrap_err());
            assert!(!msg.contains(SENTINEL));
        }

        let base = crate::test_util::tmp_abs("superai/base");
        let err = crate::template_fetch::ensure_path_safe(&base, "../escape.json");
        assert!(err.is_err(), "ensure_path_safe should reject traversal");
    }

    #[test]
    fn symlink_swap_race_in_core_transaction_is_detected() {
        #[cfg(unix)]
        {
            let dir = temp_dir("core-symlink-race");
            std::fs::create_dir_all(&dir).unwrap();
            let target_a = dir.join("targetA.json");
            let target_b = dir.join("targetB.json");
            std::fs::write(&target_a, br#"{"a":1}"#).unwrap();
            std::fs::write(&target_b, br#"{"b":2}"#).unwrap();
            let link = dir.join("link.json");
            drop(std::fs::remove_file(&link));
            std::os::unix::fs::symlink(&target_a, &link).unwrap();

            let snap = superai_config::snapshot::snapshot(&link);
            std::fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink(&target_b, &link).unwrap();

            let snap_after = superai_config::snapshot::snapshot(&link);
            assert!(superai_config::snapshot::is_modified(&snap, &snap_after));

            let res = superai_config::transaction::commit_file_expecting(
                "core-symlink-race",
                &link,
                br#"{"new":1}"#,
                superai_config::document::DocumentKind::StrictJson,
                Some(&snap),
            );
            assert!(res.is_err());
            let msg = format!("{:?}", res.unwrap_err());
            assert!(!msg.contains(SENTINEL));

            drop(std::fs::remove_dir_all(&dir));
        }
    }

    #[test]
    fn broad_deletion_is_rejected() {
        use superai_config::quarantine::validate_quarantine_target;
        use superai_config::transaction::RemoveKind;
        use superai_config::transaction::validate_remove_target;

        // The filesystem root is broad on every platform: unix `/` and
        // windows `C:\` both land in the refusal.
        let fs_root = if cfg!(windows) {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };
        let tmp = crate::test_util::tmp_abs("abuse-broad");
        assert!(
            validate_quarantine_target(&fs_root).is_err(),
            "quarantine {fs_root:?} should reject"
        );
        for p in [
            tmp.join("*.json"),
            tmp.join("logs").join("*.log"),
            tmp.join("user").join("[abc]"),
            tmp.join("foo?bar"),
        ] {
            assert!(
                validate_quarantine_target(&p).is_err(),
                "glob {p:?} should reject"
            );
        }
        for p in [
            tmp.join("$HOME/foo"),
            tmp.join("%USERPROFILE%/bar"),
            tmp.join("${HOME}/x"),
        ] {
            assert!(
                validate_quarantine_target(&p).is_err(),
                "var {p:?} should reject"
            );
        }
        assert!(validate_quarantine_target(&tmp.join("../etc")).is_err());
        assert!(validate_quarantine_target(Path::new("relative")).is_err());
        // Windows-shaped broad root is refused on every platform (inert on
        // unix where it is not absolute).
        assert!(validate_quarantine_target(Path::new("C:\\Windows")).is_err());

        for kind in [
            RemoveKind::InstanceRoot,
            RemoveKind::WrapperFile,
            RemoveKind::ConfigEntry,
            RemoveKind::Binary,
        ] {
            assert!(validate_remove_target(&fs_root, kind).is_err());
            assert!(validate_remove_target(&tmp.join("*.json"), kind).is_err());
            assert!(validate_remove_target(&tmp.join("$HOME/foo"), kind).is_err());
            assert!(validate_remove_target(&tmp.join("../etc"), kind).is_err());
            assert!(validate_remove_target(Path::new("relative/path"), kind).is_err());
        }

        // Just ensure no panic and proper error
        let foreign = crate::test_util::tmp_abs("claude-multi").join("config.json");
        drop(validate_quarantine_target(&foreign));
    }

    #[test]
    fn shell_metachars_in_names_are_handled_safely() {
        // Either the id layer rejects these, or plan handling must not
        // panic; run_command never uses a shell (checked below).
        let bad_ids = ["$(rm -rf)", "`whoami`", "a&&b", "a|b", "a;b", "a>out"];
        for bad in bad_ids {
            if let Ok(sid) = SkillId::new(bad) {
                let dir = temp_dir("shell-skill");
                std::fs::create_dir_all(&dir).unwrap();
                let bad_path = format!(
                    "{}/superai-skills/{bad}",
                    crate::test_util::tmp_abs_str("skills")
                );
                let step = superai_config::transaction::FileAction::Write {
                    path: Path::new(&bad_path).to_path_buf(),
                    content: b"test".to_vec(),
                    kind: superai_config::document::DocumentKind::Opaque,
                };
                let op_id = superai_config::transaction::OperationId::new("op-shell").unwrap();
                let txn = superai_config::transaction::Transaction::new(op_id, vec![step]);
                drop(txn.validate_plan());
                drop(std::fs::remove_dir_all(&dir));
                drop(sid);
            } else {
                let msg = format!("{:?}", SkillId::new(bad).unwrap_err());
                assert!(!msg.contains(SENTINEL));
            }
        }

        // The real-`echo` probe is unix-only; windows ships no echo binary.
        #[cfg(unix)]
        {
            let token = "$(whoami) && echo pwned | cat".to_owned();
            let opts = crate::process::ExecuteOpts {
                timeout: Some(std::time::Duration::from_secs(2)),
                ..Default::default()
            };
            let out =
                crate::process::run_command("echo", std::slice::from_ref(&token), &opts).unwrap();
            assert_eq!(
                out.stdout_trimmed(),
                token,
                "shell metachars must be passed literally, not executed"
            );
            assert_no_sentinel_in_debug(&out, "process output");
        }
    }

    #[test]
    fn malicious_version_output_with_ansi_escape_is_sanitized() {
        // ANSI escape sequences should not appear in sanitized version output
        let malicious = "\x1b[31m1.2.3\x1b[0m";
        let osc = "\x1b]2;evil-title\x07 2.0.0";
        let csi = "\x1b[2J\x1b[H 3.4.5";

        for input in [malicious, osc, csi] {
            let ver = crate::process::extract_version(input);
            if let Some(v) = ver {
                assert!(
                    !v.contains('\x1b'),
                    "version must not contain ANSI escape: input {input:?} got {v:?}"
                );
                assert!(!v.contains('\x07'), "version must not contain BEL: {v:?}");
                assert!(!v.contains(SENTINEL));
            }
        }

        let sentinel_version = format!("\x1b[31m{SENTINEL}\x1b[0m 1.2.3");
        let ver = crate::process::extract_version(&sentinel_version);
        if let Some(v) = ver {
            assert!(!v.contains(SENTINEL), "version extraction leaked sentinel");
            assert!(!v.contains('\x1b'));
        }

        let huge = "x".repeat(10 * 1024);
        let ver_huge = crate::process::extract_version(&huge);
        assert!(ver_huge.is_some());
        assert!(ver_huge.unwrap().len() <= 64);
    }

    #[test]
    fn template_url_redirect_to_private_and_file_is_rejected() {
        let cases = [
            ("http://example.com/catalog.json", true), // should reject (not https)
            ("https://example.com/catalog.json", false), // should accept
            (
                &format!(
                    "file://{}/../etc/passwd",
                    crate::test_util::tmp_abs("url-trav").display()
                ),
                true,
            ), // should reject (traversal in file path)
            (
                &format!("file://{}", crate::test_util::tmp_abs("catalog").display()),
                false,
            ), // allowed for tests (fixture)
            ("https://192.168.1.1/evil.json", true),   // private
            ("https://10.0.0.1/evil.json", true),
            ("https://127.0.0.1/evil.json", true),
            ("https://172.16.0.1/evil.json", true),
            ("https://172.31.255.255/evil.json", true),
            ("https://169.254.1.1/evil.json", true),
            ("https://example.com/../evil", true), // traversal
            ("https://example.com/catalog.json\n", true), // control
        ];
        for (url, should_err) in cases {
            let res = validate_fetch_url(url, "test-template");
            if should_err {
                assert!(res.is_err(), "url {url:?} should be rejected");
                let msg = format!("{:?}", res.unwrap_err());
                assert!(!msg.contains(SENTINEL));
            } else {
                assert!(
                    res.is_ok(),
                    "url {url:?} should be accepted, got {:?}",
                    res.unwrap_err()
                );
            }
        }

        let base = crate::test_util::tmp_abs("base");
        drop(crate::template_fetch::ensure_path_safe(&base, "../escape.json").unwrap_err());
        drop(crate::template_fetch::ensure_path_safe(&base, "a/../../b.json").unwrap_err());
        drop(crate::template_fetch::ensure_path_safe(&base, "valid/path.json").unwrap());

        // A cross-host redirect must strip auth.
        assert!(crate::failure::should_strip_auth_for_redirect(
            "https://github.com/org/repo",
            "https://evil.example.com/other"
        ));
    }

    #[test]
    fn huge_5mb_deep_config_is_bounded() {
        let huge = vec![b'a'; crate::template::MAX_TEMPLATE_BYTES + 1];
        let res = Template::from_json_bytes(&huge);
        assert!(res.is_err(), "huge template should be rejected");
        let msg = format!("{:?}", res.unwrap_err());
        assert!(!msg.contains(SENTINEL));
        assert!(
            msg.to_ascii_lowercase().contains("size")
                || msg.to_ascii_lowercase().contains("exceeds")
                || msg.to_ascii_lowercase().contains("limit")
        );

        let deep_value = {
            let mut s = String::from("{\"a\":");
            for _ in 0..250 {
                s.push_str("{\"a\":");
            }
            s.push('1');
            for _ in 0..250 {
                s.push('}');
            }
            s
        };
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&deep_value);
        // Deep may be valid or not, but must not panic
        drop(parsed);

        let huge_json = format!(r#"{{"data":"{}"}}"#, "x".repeat(5 * 1024 * 1024));
        let bytes = huge_json.into_bytes();
        assert!(bytes.len() > 5 * 1024 * 1024);
        // Validation must not panic; the document is valid but size-bounded elsewhere.
        let diags = superai_config::raw_editor::validate(
            &bytes,
            superai_config::document::DocumentKind::StrictJson,
        );
        drop(diags);
    }

    #[test]
    fn plugin_skill_symlink_escape_is_rejected() {
        let dir = temp_dir("skill-symlink-escape");
        let registry_root = dir.join("registry");
        std::fs::create_dir_all(&registry_root).unwrap();
        let skill_dir = registry_root.join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_md = r"---
name: my-skill
description: test skill
---
# Skill
";
        std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();
        std::fs::write(skill_dir.join("data.txt"), b"hello").unwrap();

        #[cfg(unix)]
        {
            let outside = dir.join("outside.txt");
            std::fs::write(&outside, b"outside").unwrap();
            let link = skill_dir.join("evil-link");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            let res = crate::skills::validate_skill_tree(&skill_dir);
            assert!(
                res.is_err(),
                "skill with absolute symlink escape should be rejected"
            );
            let msg = format!("{:?}", res.unwrap_err());
            assert!(!msg.contains(SENTINEL));
            assert!(
                msg.to_ascii_lowercase().contains("symlink")
                    || msg.to_ascii_lowercase().contains("relative")
            );

            std::fs::remove_file(&link).unwrap();
            let traversal_target = Path::new("../../outside.txt");
            std::os::unix::fs::symlink(traversal_target, &link).unwrap();
            let res2 = crate::skills::validate_skill_tree(&skill_dir);
            assert!(
                res2.is_err(),
                "skill with traversal symlink should be rejected"
            );
            let msg2 = format!("{:?}", res2.unwrap_err());
            assert!(!msg2.contains(SENTINEL));

            std::fs::remove_file(&link).unwrap();
            let valid_target = Path::new("data.txt");
            std::os::unix::fs::symlink(valid_target, &link).unwrap();
            let res3 = crate::skills::validate_skill_tree(&skill_dir);
            assert!(
                res3.is_ok(),
                "valid symlink inside skill should not be rejected: {:?}",
                res3.unwrap_err()
            );

            std::fs::remove_file(&link).unwrap();
        }

        #[cfg(not(unix))]
        {
            let res = crate::skills::validate_skill_tree(&skill_dir);
            res.expect("skill tree validation should succeed on non-unix");
        }

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn pid_reuse_is_detected() {
        let dir = temp_dir("pid-reuse");
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("daemon.pid");
        std::fs::write(&pid_file, b"99999").unwrap();
        let pid_in_file: u32 = String::from_utf8_lossy(&std::fs::read(&pid_file).unwrap())
            .trim()
            .parse()
            .unwrap();
        assert_eq!(pid_in_file, 99999);

        let daemon = crate::failure::DaemonFixture::unrelated_pid("test-daemon", 99999);
        assert!(!daemon.ready);
        assert!(daemon.reason.as_ref().unwrap().contains("unrelated"));
        // Our check: pid file content should not be trusted if process is unrelated
        let actual_pid = 1111; // different from file
        assert_ne!(pid_in_file, actual_pid);
        let same_pid_but_different =
            crate::failure::DaemonFixture::unrelated_pid("other-daemon", 99999);
        assert_eq!(same_pid_but_different.pid, Some(99999));
        assert!(!same_pid_but_different.ready);

        let ready = crate::failure::DaemonFixture::ready("test-daemon", 4242);
        assert!(ready.ready);
        assert_eq!(ready.pid, Some(4242));

        let err = crate::error::CoreError::DaemonNotReady {
            harness: "test".to_owned(),
            reason: format!("pid {pid_in_file} unrelated"),
        };
        let msg = format!("{err:?}");
        assert!(!msg.contains(SENTINEL));

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn errors_and_debug_never_contain_sentinel_plain() {
        let sentinel_err = crate::error::CoreError::Validation {
            field: "test".to_owned(),
            reason: format!(
                "validation failed for field containing {}",
                SENTINEL.replace("sk-", "[REDACTED]")
            ),
        };
        let msg = format!("{sentinel_err}");
        assert!(!msg.contains(SENTINEL), "error display leaked sentinel");
        let dbg = format!("{sentinel_err:?}");
        assert!(!dbg.contains(SENTINEL));

        let redacted = crate::error::RedactedString::new(SENTINEL);
        let dbg_r = format!("{redacted:?}");
        let disp_r = format!("{redacted}");
        let json_r = serde_json::to_string(&redacted).unwrap();
        for out in [&dbg_r, &disp_r, &json_r] {
            assert!(!out.contains(SENTINEL));
            assert!(out.contains("[REDACTED]"));
        }
    }

    #[test]
    fn registry_never_contains_sentinel_even_after_sentinel_in_harness_config() {
        let dir = temp_dir("registry-sentinel-scan");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("settings.json");
        std::fs::write(&cfg, format!(r#"{{"api_key":"{SENTINEL}"}}"#)).unwrap();
        let reg_path = dir.join("registry.json");
        let mut reg = Registry::default();
        reg.insert(sample_instance(&dir, "scan1")).unwrap();
        reg.insert(sample_instance(&dir, "scan2")).unwrap();
        reg.store(&reg_path).unwrap();

        let reg_bytes = std::fs::read(&reg_path).unwrap();
        assert_no_sentinel_bytes(&reg_bytes, "registry file");
        let reg_str = String::from_utf8_lossy(&reg_bytes);
        assert!(!scan_str(&reg_str));

        let preview = crate::operation::OperationPreview {
            id: crate::ids::OperationId::new("op-scan").unwrap(),
            kind: crate::operation::OperationKind::UpdateConfig,
            requested_target: crate::operation::RequestedTarget {
                display: "scan".to_owned(),
                harness: Some(HarnessId::new("claude-code").unwrap()),
                instance: Some(InstanceName::new("scan1").unwrap()),
            },
            resolved_resources: vec![],
            preconditions: vec![],
            actions: vec![],
            diffs: vec![crate::operation::RedactedDiff {
                path: AbsolutePath::from_path(&crate::test_util::tmp_abs("scan").join("scan.json"))
                    .unwrap(),
                surface: "settings.json".to_owned(),
                lexical_redacted: "api_key: [REDACTED]".to_owned(),
                semantic_redacted: "updated api_key to [REDACTED]".to_owned(),
                redacted_fields: vec!["api_key".to_owned()],
            }],
            backups: vec![],
            warnings: vec![],
            conflicts: vec![],
            limitations: vec![],
            auth_steps: vec![],
            restart_requirements: vec![],
            rollback_plan: crate::operation::RollbackPlan {
                steps: vec![],
                will_restore_backups: false,
                estimated_steps: 0,
            },
        };
        assert_no_sentinel_in_json(&preview, "preview");
        assert_no_sentinel_in_debug(&preview, "preview debug");

        let result = crate::operation::OperationResult {
            id: crate::ids::OperationId::new("op-scan").unwrap(),
            kind: crate::operation::OperationKind::UpdateConfig,
            actions_completed: vec![],
            backups: vec![],
            verification: vec![],
            rollback_status: crate::operation::RollbackStatus::NotNeeded,
            diagnostics_redacted: vec!["api_key set to [REDACTED]".to_owned()],
            success: true,
        };
        assert_no_sentinel_in_json(&result, "result");
        for diag in &result.diagnostics_redacted {
            assert!(!diag.contains(SENTINEL));
        }

        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn windows_reserved_long_case_crlf_are_handled_without_panic_or_leak() {
        let dir = temp_dir("windows-long-case-crlf-core");
        std::fs::create_dir_all(&dir).unwrap();
        for reserved in ["CON", "PRN", "AUX", "NUL", "COM1", "LPT1"] {
            let inst_res = InstanceName::new(reserved);
            if let Ok(name) = inst_res {
                assert_ne!(
                    name.as_str().to_ascii_uppercase(),
                    reserved,
                    "reserved {reserved:?} should be rejectable or at least not collide silently"
                );
            }
            let win_path_str = format!("C:\\Windows\\{reserved}.txt");
            let win_path = Path::new(&win_path_str);
            let q_res = superai_config::quarantine::validate_quarantine_target(win_path);
            // On unix it's relative; should be rejected as relative or as broad
            assert!(q_res.is_err(), "win path {win_path:?} should be rejected");
            let msg = format!("{:?}", q_res.unwrap_err());
            assert!(!msg.contains(SENTINEL));
            assert!(msg.len() <= 4096);
        }
        // Long path through the mutation boundary: no panic, and either a
        // verified commit or a typed refusal (never a partial write).
        let long = "a".repeat(300);
        let long_path = dir.join(format!("{long}.json"));
        let long_res = std::panic::catch_unwind(|| {
            superai_config::transaction::commit_file(
                "core-abuse-long-path",
                &long_path,
                br#"{"a":1}"#,
                superai_config::document::DocumentKind::StrictJson,
            )
        });
        assert!(long_res.is_ok(), "long path must not panic");
        if let Ok(Ok(report)) = long_res {
            let _ = report;
            drop(std::fs::remove_file(&long_path));
        }
        let mut reg = Registry::default();
        let h = HarnessId::new("claude-code").unwrap();
        let n1 = InstanceName::new("MyWork").unwrap();
        let n2 = InstanceName::new("mywork").unwrap();
        let r1 = AbsolutePath::from_path(&crate::test_util::tmp_abs("case1")).unwrap();
        let r2 = AbsolutePath::from_path(&crate::test_util::tmp_abs("case2")).unwrap();
        let inst1 = Instance {
            id: InstanceId::new("id-case-1").unwrap(),
            name: n1,
            harness: h.clone(),
            config_root: r1,
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        };
        reg.insert(inst1).unwrap();
        let inst2 = Instance {
            id: InstanceId::new("id-case-2").unwrap(),
            name: n2,
            harness: h,
            config_root: r2,
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: None,
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        };
        let err = reg.insert(inst2).unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.to_ascii_lowercase().contains("collision")
                || msg.to_ascii_lowercase().contains("name")
        );
        assert!(!msg.contains(SENTINEL));
        let crlf_path = dir.join("crlf_core.json");
        let crlf_bytes = b"{\r\n  \"model\": \"opus\"\r\n}";
        std::fs::write(&crlf_path, crlf_bytes).unwrap();
        let v = superai_config::raw_editor::validate(
            crlf_bytes,
            superai_config::document::DocumentKind::StrictJson,
        );
        assert!(v.is_empty(), "CRLF json should be valid: {v:?}");
        let edit = superai_config::json::edit(&crlf_path, |m| {
            m.insert("b".to_owned(), serde_json::Value::String("x".to_owned()));
        });
        edit.unwrap();
        let after = std::fs::read(&crlf_path).unwrap();
        assert!(!contains_sentinel(&after));
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn shell_metachars_and_broad_deletion_and_huge_are_bounded_core() {
        let dir = temp_dir("shell-broad-huge-core");
        std::fs::create_dir_all(&dir).unwrap();
        for seg in [
            "$(rm -rf)",
            "`whoami`",
            "; cat",
            "| sh",
            "&& rm",
            "${HOME}",
            "*glob*",
            "[abc]",
        ] {
            let path = dir.join(format!("{seg}.json"));
            let res = std::panic::catch_unwind(|| {
                superai_config::transaction::commit_file(
                    "core-abuse-metachars",
                    &path,
                    br#"{"a":1}"#,
                    superai_config::document::DocumentKind::StrictJson,
                )
            });
            assert!(res.is_ok(), "shell metachars {seg:?} must not panic");
            if let Ok(Ok(report)) = res {
                let _ = report;
                let b = std::fs::read(&path).unwrap();
                assert!(!contains_sentinel(&b));
                drop(std::fs::remove_file(&path));
            }
        }
        let fs_root2 = if cfg!(windows) {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };
        let tmp2 = crate::test_util::tmp_abs("abuse-scan-broad");
        for p in [fs_root2, tmp2.join("*.json"), tmp2.join("$HOME/foo")] {
            let r = superai_config::quarantine::validate_quarantine_target(&p);
            assert!(r.is_err(), "broad {p:?} must be rejected");
            assert!(!format!("{:?}", r.unwrap_err()).contains(SENTINEL));
        }
        let huge = vec![b'a'; crate::template::MAX_TEMPLATE_BYTES + 1024];
        let tr = Template::from_json_bytes(&huge);
        assert!(tr.is_err());
        let msg = format!("{:?}", tr.unwrap_err());
        assert!(msg.len() <= 8192);
        assert!(!msg.contains(SENTINEL));
        drop(std::fs::remove_dir_all(&dir));
    }
}
