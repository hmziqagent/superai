//! Property tests (QAL-03): manual loops with a deterministic RNG, covering
//! no-op identity, unrelated survival, exact restore, deterministic previews.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::collapsible_if,
    clippy::excessive_nesting,
    clippy::format_push_string,
    clippy::len_zero,
    clippy::manual_is_multiple_of,
    clippy::uninlined_format_args,
    clippy::unreadable_literal,
    reason = "property loops keep PRNG casts and manual nesting"
)]

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};
    use std::path::PathBuf;

    use serde_json::{Map, Number, Value};

    use crate::backup::{backup, list_backups, restore_entry, verify_backup};
    use crate::document::Selector;
    use crate::quarantine::validate_quarantine_target;
    use crate::snapshot::{is_modified, snapshot};
    use crate::test_util::temp_dir_unique;

    struct Prng {
        state: u64,
    }

    impl Prng {
        fn new(seed: u64) -> Self {
            Self {
                state: seed.wrapping_add(0x9e3779b97f4a7c15),
            }
        }

        fn next_u64(&mut self) -> u64 {
            let mut z = self.state.wrapping_add(0x9e3779b97f4a7c15);
            self.state = z;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            z ^ (z >> 31)
        }

        #[expect(
            clippy::cast_possible_truncation,
            reason = "prng test helper truncation intentional"
        )]
        fn next_u32(&mut self) -> u32 {
            self.next_u64() as u32
        }

        fn gen_range(&mut self, low: usize, high: usize) -> usize {
            if low >= high {
                return low;
            }
            let range = high.saturating_sub(low);
            let v = self.next_u64() as usize;
            low.saturating_add(v % range)
        }

        fn gen_bool(&mut self) -> bool {
            self.next_u32() % 2 == 0
        }

        fn gen_string(&mut self, min_len: usize, max_len: usize, charset: &[u8]) -> String {
            let len = self.gen_range(min_len, max_len.saturating_add(1));
            let mut s = String::with_capacity(len);
            for _ in 0..len {
                let idx = self.gen_range(0, charset.len());
                let b = charset[idx];
                s.push(b as char);
            }
            s
        }
    }

    const KEY_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";
    const SIMPLE_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    const VALUE_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 ";

    fn random_key(rng: &mut Prng) -> String {
        // Keys are 1..10 chars; a leading digit gets an alpha prefix so the
        // key stays valid for env and TOML.
        let mut k = rng.gen_string(1, 10, KEY_CHARSET);
        if let Some(first) = k.chars().next() {
            if first.is_ascii_digit() {
                let prefix = if rng.gen_bool() { "k" } else { "a" };
                k = format!("{prefix}{k}");
            }
        }
        k
    }

    fn random_value(rng: &mut Prng) -> Value {
        let choice = rng.gen_range(0, 6);
        match choice {
            0 => Value::Null,
            1 => Value::Bool(rng.gen_bool()),
            2 => {
                let n = rng.gen_range(0, 1000);
                if rng.gen_bool() {
                    Value::Number(Number::from(n as i64))
                } else {
                    // Use f64 with one decimal to keep 1.0 distinct from 1.
                    let f = (n as f64) + 0.5;
                    Number::from_f64(f).map_or(Value::Number(Number::from(n as i64)), Value::Number)
                }
            }
            3 => {
                let s = rng.gen_string(0, 12, VALUE_CHARSET);
                Value::String(s)
            }
            4 => {
                let len = rng.gen_range(0, 4);
                let mut arr = Vec::with_capacity(len);
                for _ in 0..len {
                    let v = match rng.gen_range(0, 3) {
                        0 => Value::Bool(rng.gen_bool()),
                        1 => Value::Number(Number::from(rng.gen_range(0, 100) as i64)),
                        _ => Value::String(rng.gen_string(0, 8, VALUE_CHARSET)),
                    };
                    arr.push(v);
                }
                Value::Array(arr)
            }
            _ => {
                let len = rng.gen_range(0, 3);
                let mut map = Map::new();
                for _ in 0..len {
                    let k = random_key(rng);
                    let v = Value::String(rng.gen_string(0, 8, VALUE_CHARSET));
                    if !map.contains_key(&k) {
                        map.insert(k, v);
                    }
                }
                Value::Object(map)
            }
        }
    }

    fn random_json_map(rng: &mut Prng, max_keys: usize) -> Map<String, Value> {
        let n = rng.gen_range(0, max_keys.saturating_add(1));
        let mut map = Map::new();
        for _ in 0..n {
            let k = random_key(rng);
            if map.contains_key(&k) {
                continue;
            }
            map.insert(k, random_value(rng));
        }
        map
    }

    fn scratch_path(prefix: &str, name: &str) -> PathBuf {
        let dir = temp_dir_unique(prefix);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn property_no_op_byte_identity_json() {
        for iter in 0..100 {
            let mut rng = Prng::new(iter as u64 + 0xabc123);
            let map = random_json_map(&mut rng, 5);
            let json_text = if rng.gen_bool() {
                serde_json::to_string_pretty(&Value::Object(map.clone())).unwrap()
            } else {
                serde_json::to_string(&Value::Object(map.clone())).unwrap()
            };
            // Add trailing newline like store does, but we write raw to test preservation.
            let mut file_bytes = json_text.into_bytes();
            file_bytes.push(b'\n');

            let path = scratch_path("prop-noop-json", &format!("iter-{iter}.json"));
            std::fs::write(&path, &file_bytes).unwrap();
            let before = std::fs::read(&path).unwrap();

            crate::json::edit(&path, |_| {}).unwrap();

            let after = std::fs::read(&path).unwrap();
            assert_eq!(
                before, after,
                "no-op byte identity failed at iter {iter}: map={map:?}"
            );

            let backups = list_backups(&path).unwrap();
            assert!(
                backups.is_empty(),
                "no-op should not create backup at iter {iter}"
            );

            drop(std::fs::remove_dir_all(path.parent().unwrap()));
        }
    }

    #[test]
    fn property_no_op_byte_identity_toml() {
        for iter in 0..80 {
            let mut rng = Prng::new(iter as u64 + 0x00def33);
            let mut doc = toml_edit::DocumentMut::new();
            let n = rng.gen_range(0, 5);
            for _ in 0..n {
                let k = random_key(&mut rng);
                let v = rng.gen_range(0, 100) as i64;
                doc[&k] = toml_edit::value(v);
            }
            let text = doc.to_string();
            let path = scratch_path("prop-noop-toml", &format!("iter-{iter}.toml"));
            std::fs::write(&path, text.as_bytes()).unwrap();
            let before = std::fs::read(&path).unwrap();

            crate::toml_file::edit(&path, |_| {}).unwrap();

            let after = std::fs::read(&path).unwrap();
            assert_eq!(before, after, "toml no-op failed at {iter}");

            let backups = list_backups(&path).unwrap();
            assert!(backups.is_empty(), "toml no-op created backup at {iter}");

            drop(std::fs::remove_dir_all(path.parent().unwrap()));
        }
    }

    #[test]
    fn property_no_op_byte_identity_yaml() {
        for iter in 0..80 {
            let mut rng = Prng::new(iter as u64 + 0x112233);
            let map = random_json_map(&mut rng, 4);
            let text = if map.is_empty() {
                String::new()
            } else {
                yaml_serde::to_string(&Value::Object(map.clone())).unwrap()
            };
            let path = scratch_path("prop-noop-yaml", &format!("iter-{iter}.yaml"));
            std::fs::write(&path, text.as_bytes()).unwrap();
            let before = std::fs::read(&path).unwrap();

            crate::yaml::edit(&path, |_| {}).unwrap();

            let after = std::fs::read(&path).unwrap();
            assert_eq!(before, after, "yaml no-op failed at {iter}");

            let backups = list_backups(&path).unwrap();
            assert!(backups.is_empty(), "yaml no-op created backup at {iter}");

            drop(std::fs::remove_dir_all(path.parent().unwrap()));
        }
    }

    #[test]
    fn property_unrelated_survive_json() {
        for iter in 0..100 {
            let mut rng = Prng::new(iter as u64 + 0x7777);
            let mut map = random_json_map(&mut rng, 5);
            while map.len() < 2 {
                let k = random_key(&mut rng);
                if !map.contains_key(&k) {
                    map.insert(k, random_value(&mut rng));
                }
            }
            let keys: Vec<String> = map.keys().cloned().collect();
            let target_idx = rng.gen_range(0, keys.len());
            let target_key = keys[target_idx].clone();
            let new_val = Value::String(format!(
                "updated-{iter}-{}",
                rng.gen_string(2, 6, VALUE_CHARSET)
            ));

            let path = scratch_path("prop-unrelated-json", &format!("iter-{iter}.json"));
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&Value::Object(map.clone()))
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap();

            crate::json::edit(&path, |m| {
                m.insert(target_key.clone(), new_val.clone());
            })
            .unwrap();

            let after = crate::json::load(&path).unwrap();
            assert_eq!(
                after.get(&target_key),
                Some(&new_val),
                "target not updated at {iter}"
            );
            for k in keys {
                if k == target_key {
                    continue;
                }
                let before_val = &map[&k];
                let after_val = after
                    .get(&k)
                    .unwrap_or_else(|| panic!("missing unrelated key {k} at {iter}"));
                assert_eq!(
                    before_val, after_val,
                    "unrelated key {k} mutated at iter {iter}"
                );
            }
            let before_order: Vec<&String> = map.keys().collect();
            let after_order: Vec<&String> = after.keys().collect();
            let before_unrelated: Vec<&String> = before_order
                .into_iter()
                .filter(|k| *k != &target_key)
                .collect();
            let after_unrelated: Vec<&String> = after_order
                .into_iter()
                .filter(|k| *k != &target_key)
                .collect();
            assert_eq!(
                before_unrelated, after_unrelated,
                "order of unrelated keys changed at {iter}"
            );

            drop(std::fs::remove_dir_all(path.parent().unwrap()));
        }
    }

    #[test]
    fn property_unrelated_survive_env() {
        for iter in 0..100 {
            let mut rng = Prng::new(iter as u64 + 0x8888);
            let n = rng.gen_range(2, 6);
            let mut vars = BTreeMap::new();
            let mut keys = Vec::new();
            for i in 0..n {
                let k = format!("KEY_{}_{}", iter, i);
                let v = rng.gen_string(2, 12, SIMPLE_CHARSET);
                vars.insert(k.clone(), v);
                keys.push(k);
            }
            let mut text = String::new();
            text.push_str("# generated\n");
            for k in &keys {
                let v = &vars[k];
                text.push_str(&format!("{k}={v}\n"));
                if rng.gen_bool() {
                    text.push_str("# comment\n");
                }
            }
            let path = scratch_path("prop-unrelated-env", &format!("iter-{iter}.env"));
            std::fs::write(&path, text.as_bytes()).unwrap();

            let target = keys[rng.gen_range(0, keys.len())].clone();
            let new_val = format!("newval-{iter}");

            crate::env_file::edit(&path, |m| {
                m.insert(target.clone(), new_val.clone());
            })
            .unwrap();

            let after = crate::env_file::load(&path).unwrap();
            assert_eq!(&after[&target], &new_val, "env target not updated");
            for k in &keys {
                if k == &target {
                    continue;
                }
                assert_eq!(&vars[k], &after[k], "env unrelated key {k} lost at {iter}");
            }

            drop(std::fs::remove_dir_all(path.parent().unwrap()));
        }
    }

    #[test]
    fn property_restore_exact_backup() {
        for iter in 0..50 {
            let mut rng = Prng::new(iter as u64 + 0x9999);
            let size = rng.gen_range(0, 4096);
            let mut bytes = Vec::with_capacity(size);
            for _ in 0..size {
                let b = rng.gen_range(0, 256) as u8;
                bytes.push(b);
            }
            let path = scratch_path("prop-restore", &format!("iter-{iter}.bin"));
            std::fs::write(&path, &bytes).unwrap();

            #[cfg(unix)]
            let orig_mode = {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(&path).unwrap().permissions().mode()
            };

            let entry = backup(&path).unwrap().expect("backup should exist");
            assert_eq!(entry.size, bytes.len() as u64, "size mismatch at {iter}");
            let new_bytes = format!(
                "overwritten-{iter}-{}",
                rng.gen_string(4, 20, VALUE_CHARSET)
            )
            .into_bytes();
            std::fs::write(&path, &new_bytes).unwrap();
            assert_ne!(std::fs::read(&path).unwrap(), bytes, "overwrite failed");

            restore_entry(&entry).unwrap();
            let restored = std::fs::read(&path).unwrap();
            assert_eq!(restored, bytes, "restore exact failed at {iter}");

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let restored_mode = std::fs::metadata(&path).unwrap().permissions().mode();
                assert_eq!(
                    orig_mode, restored_mode,
                    "permissions not preserved at {iter}"
                );
            }

            assert!(
                verify_backup(&entry).unwrap(),
                "verify backup failed at {iter}"
            );

            drop(std::fs::remove_dir_all(path.parent().unwrap()));
        }
    }

    #[test]
    fn property_preview_deterministic_raw_diff() {
        use crate::document::DocumentKind;
        let kinds = [
            DocumentKind::StrictJson,
            DocumentKind::JsonC,
            DocumentKind::Toml,
            DocumentKind::Yaml,
            DocumentKind::Env,
            DocumentKind::Opaque,
        ];
        for iter in 0..100 {
            let mut rng = Prng::new(iter as u64 + 0xaaaa);
            let old_len = rng.gen_range(0, 200);
            let new_len = rng.gen_range(0, 200);
            let old: Vec<u8> = (0..old_len).map(|_| rng.gen_range(32, 127) as u8).collect();
            let new: Vec<u8> = (0..new_len).map(|_| rng.gen_range(32, 127) as u8).collect();
            let kind = kinds[rng.gen_range(0, kinds.len())];

            let diff1 = crate::raw_editor::diff(&old, &new, kind);
            let diff2 = crate::raw_editor::diff(&old, &new, kind);

            assert_eq!(
                diff1.is_noop, diff2.is_noop,
                "is_noop not deterministic at {iter}"
            );
            assert_eq!(
                diff1.lexical_unified_diff, diff2.lexical_unified_diff,
                "lexical diff not deterministic at {iter}"
            );
            assert_eq!(
                diff1.semantic_ops, diff2.semantic_ops,
                "semantic ops not deterministic at {iter}"
            );
            assert_eq!(
                diff1.redaction_spans, diff2.redaction_spans,
                "redaction spans not deterministic at {iter}"
            );
            assert_eq!(
                (old == new),
                diff1.is_noop,
                "is_noop should equal byte equality at {iter}"
            );
        }
    }

    #[test]
    fn property_collision_safe_backup_suffix() {
        let path = scratch_path("prop-collision", "target.json");
        std::fs::write(&path, b"initial").unwrap();

        let mut ids = HashSet::new();

        for iter in 0..100 {
            let content = format!("content-{iter}").into_bytes();
            std::fs::write(&path, &content).unwrap();
            let entry = backup(&path).unwrap().unwrap();
            let id_str = entry.id.as_str().to_owned();
            assert!(
                ids.insert(id_str.clone()),
                "backup id collision at iter {iter}: {id_str}"
            );
            assert!(entry.backup_path.exists(), "backup file missing at {iter}");
            assert!(verify_backup(&entry).unwrap(), "verify failed at {iter}");
        }

        let listed = list_backups(&path).unwrap();
        let mut seen_paths = HashSet::new();
        for e in listed {
            assert!(
                seen_paths.insert(e.backup_path.clone()),
                "backup path collision {:?}",
                e.backup_path
            );
        }

        assert!(
            ids.len() >= 90,
            "expected many unique ids, got {}",
            ids.len()
        );

        drop(std::fs::remove_dir_all(path.parent().unwrap()));
    }

    #[test]
    fn property_commit_matches_preview_or_aborts_raw_editor() {
        for iter in 0..50 {
            let mut rng = Prng::new(iter as u64 + 0xbbbb);
            let initial = format!("{{\"a\":{}}}", rng.gen_range(0, 100));
            let path = scratch_path("prop-commit-preview", &format!("iter-{iter}.json"));
            std::fs::write(&path, initial.as_bytes()).unwrap();

            let raw = crate::raw_editor::read(&path).unwrap();
            let expected_digest = raw.digest.clone();
            let before_bytes = raw.content.expose().to_vec();

            let make_invalid = rng.gen_bool() && iter % 3 == 0;
            let new_content = if make_invalid {
                b"{ invalid json ".to_vec()
            } else {
                let new_val = rng.gen_range(0, 1000);
                format!("{{\"a\":{new_val}}}").into_bytes()
            };

            let preview_docs = crate::raw_editor::validate(
                &new_content,
                crate::document::DocumentKind::StrictJson,
            );
            let is_valid = preview_docs.is_empty();

            let inject_conflict = rng.gen_bool() && iter % 4 == 0 && is_valid;
            if inject_conflict {
                std::fs::write(&path, b"{\"a\":9999}").unwrap();
            }

            let commit_res = crate::raw_editor::commit(&path, &new_content, Some(&expected_digest));

            if !is_valid {
                assert!(
                    commit_res.is_err(),
                    "invalid content should not commit at {iter}"
                );
                let cur = std::fs::read(&path).unwrap();
                assert_ne!(cur, new_content, "invalid commit mutated file at {iter}");
                if !inject_conflict {
                    assert_eq!(
                        cur, before_bytes,
                        "invalid commit altered file without conflict at {iter}"
                    );
                }
            } else if inject_conflict {
                assert!(commit_res.is_err(), "conflict should abort at {iter}");
                let cur = std::fs::read(&path).unwrap();
                assert_ne!(
                    cur, new_content,
                    "conflict commit should not have written at {iter}"
                );
            } else {
                assert!(
                    commit_res.is_ok(),
                    "valid commit failed at {iter}: {:?}",
                    commit_res.err()
                );
                let cur = std::fs::read(&path).unwrap();
                assert_eq!(
                    cur, new_content,
                    "commit result not matching preview at {iter}"
                );
            }

            drop(std::fs::remove_dir_all(path.parent().unwrap()));
        }
    }

    #[test]
    fn mutant_backup_before_write_is_not_skippable() {
        // Mutant-killer: backup must exist before any successful write; skipping backup must be detectable.
        for iter in 0..30 {
            let path = scratch_path("prop-backup-mutant", &format!("iter-{iter}.json"));
            let original = format!(r#"{{"a":{iter}}}"#).into_bytes();
            std::fs::write(&path, &original).unwrap();
            let snap = snapshot(&path);
            assert!(snap.exists);
            let entry = backup(&path).unwrap().expect("backup");
            assert!(verify_backup(&entry).unwrap());
            assert_eq!(std::fs::read(&entry.backup_path).unwrap(), original);
            let backups = list_backups(&path).unwrap();
            assert!(backups.len() >= 1, "at least one backup at {iter}");
            for b in &backups {
                let dbg = format!("{b:?}");
                assert!(!dbg.contains("sk-superai-test-sentinel"));
                assert!(b.backup_path.exists());
            }
            let new_content = format!(r#"{{"a":{}}}"#, iter + 1000).into_bytes();
            crate::transaction::commit_file_expecting(
                "prop-backup-mutant",
                &path,
                &new_content,
                crate::document::DocumentKind::StrictJson,
                Some(&snap),
            )
            .unwrap();
            assert_eq!(
                std::fs::read(&path).unwrap(),
                new_content,
                "write should succeed with correct snapshot at {iter}"
            );
            std::fs::write(&path, &original).unwrap();
            let snap2 = snapshot(&path);
            std::fs::write(&path, br#"{"a":9999}"#).unwrap();
            let snap3 = snapshot(&path);
            assert!(
                is_modified(&snap2, &snap3),
                "is_modified must detect external edit at {iter} (mutant would return false)"
            );
            let stale_res = crate::transaction::commit_file_expecting(
                "prop-backup-mutant",
                &path,
                &new_content,
                crate::document::DocumentKind::StrictJson,
                Some(&snap2),
            );
            assert!(stale_res.is_err(), "stale snapshot must abort at {iter}");
            drop(std::fs::remove_dir_all(path.parent().unwrap()));
        }
    }

    #[test]
    fn mutant_secret_redaction_is_not_removable() {
        // Mutant-killer: removing find_redaction_spans or masking must cause this test to fail.
        let sentinel = "sk-superai-test-sentinel-12345-fake";
        let json_with_secret = format!(r#"{{"api_key":"{sentinel}","model":"opus"}}"#);
        let bytes = json_with_secret.as_bytes();
        let spans = crate::raw_editor::find_redaction_spans(
            bytes,
            crate::document::DocumentKind::StrictJson,
        );
        assert!(
            !spans.is_empty(),
            "secret spans must be found (mutant removed detection would yield empty)"
        );
        let new_bytes = format!(r#"{{"api_key":"{sentinel}2","model":"sonnet"}}"#).into_bytes();
        let diff =
            crate::raw_editor::diff(bytes, &new_bytes, crate::document::DocumentKind::StrictJson);
        assert!(
            diff.redaction_spans.len() >= spans.len(),
            "diff must preserve redaction spans"
        );
        assert!(
            !diff.lexical_unified_diff.contains(sentinel),
            "diff lexical must not leak sentinel"
        );
    }

    #[test]
    fn mutant_template_selector_traversal_is_rejected() {
        let traversals = ["../", "a/../b", "..\\", "key:../escape", "table:../"];
        for t in traversals {
            drop(Selector::parse(t));
        }
        assert!(validate_quarantine_target(&std::env::temp_dir().join("../etc")).is_err());
        assert!(validate_quarantine_target(std::path::Path::new("relative")).is_err());
    }
}
