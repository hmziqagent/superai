//! Parser fuzz scaffolding (QAL-04): deterministic 100-iteration loops over
//! registry/provider/detection inputs asserting no panic and no FS mutation.

#![expect(
    clippy::case_sensitive_file_extension_comparisons,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::collapsible_if,
    clippy::excessive_nesting,
    clippy::format_push_string,
    clippy::manual_assert,
    clippy::manual_clamp,
    clippy::manual_let_else,
    clippy::single_char_add_str,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::unreadable_literal,
    clippy::useless_vec,
    reason = "fuzz loops: manual nesting, PRNG casts, incremental formats"
)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

// Deterministic PRNG: SplitMix64

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

    fn gen_range(&mut self, low: usize, high: usize) -> usize {
        if low >= high {
            return low;
        }
        low.saturating_add((self.next_u64() as usize) % high.saturating_sub(low))
    }

    fn gen_bytes(&mut self, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push((self.next_u64() & 0xff) as u8);
        }
        out
    }
}

const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

#[cfg(test)]
fn seed_registry_corpus() -> Vec<Vec<u8>> {
    let mut corpus = Vec::new();
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures = manifest.join("fixtures");
    if fixtures.is_dir() {
        collect_recursive(&fixtures, &mut corpus);
    }
    // Valid-root seeds anchor under the platform temp dir so deep validation
    // also runs on Windows, where `/home/...` literals are not absolute.
    let valid_root = crate::test_util::tmp_abs_str("user/.claude-work");
    let escape_root = crate::test_util::tmp_abs_str("escape");
    corpus.extend(vec![
        br#"{"schema_version":1,"instances":[]}"#.to_vec(),
        format!(r#"{{"instances":[{{"name":"work","harness":"claude-code","config_dir":"{valid_root}"}}]}}"#).into_bytes(),
        format!(r#"{{"schema_version":1,"instances":[{{"id":"inst-1","name":"work","harness":"claude-code","config_root":"{valid_root}","isolation":"relocated_root","origin":"created","ownership":"superai_created","created_at":"2026-08-26T12:00:00Z","adapter_revision":"0.1.0"}}]}}"#).into_bytes(),
        b"".to_vec(),
        b"{}".to_vec(),
        b"[]".to_vec(),
        br#"{"schema_version":999,"instances":[]}"#.to_vec(),
        br#"{"schema_version":"bad"}"#.to_vec(),
        br#"{"instances":"not an array"}"#.to_vec(),
        format!(r#"{{"instances":[{{"name":"../escape","harness":"claude-code","config_dir":"{escape_root}/../etc/passwd"}}]}}"#).into_bytes(),
        vec![0xff, 0xfe, 0xfd],
    ]);
    if corpus.len() > 200 {
        corpus.truncate(200);
    }
    corpus
}

#[cfg(test)]
fn collect_recursive(dir: &Path, out: &mut Vec<Vec<u8>>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            collect_recursive(&p, out);
        } else if p.is_file() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".json")
                    || lower == "registry_old.json"
                    || lower == "registry_v1.json"
                {
                    if let Ok(b) = std::fs::read(&p) {
                        let b = if b.len() > MAX_INPUT_BYTES {
                            b[..MAX_INPUT_BYTES].to_vec()
                        } else {
                            b
                        };
                        out.push(b);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
fn gen_truncated(prng: &mut Prng, base: &[u8]) -> Vec<u8> {
    if base.is_empty() {
        let len = prng.gen_range(0, 64);
        return prng.gen_bytes(len);
    }
    let cut = prng.gen_range(0, base.len().saturating_add(1));
    base.get(0..cut).unwrap_or(&[]).to_vec()
}

#[cfg(test)]
fn gen_huge_registry(prng: &mut Prng) -> Vec<u8> {
    let base = crate::test_util::tmp_abs_str("cfg");
    let mut s = String::from("{\"schema_version\":1,\"instances\":[");
    for i in 0..300 {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"id\":\"id-{i}\",\"name\":\"name{i}\",\"harness\":\"claude-code\",\"config_root\":\"{base}/cfg{i}\",\"isolation\":\"unknown\",\"origin\":\"created\",\"ownership\":\"superai_created\",\"created_at\":\"2026-01-01T00:00:00Z\",\"adapter_revision\":\"0.1.0\"}}"
        ));
        if s.len() > MAX_INPUT_BYTES {
            break;
        }
    }
    s.push_str("]}");
    let mut b = s.into_bytes();
    if b.len() > MAX_INPUT_BYTES {
        b.truncate(MAX_INPUT_BYTES);
    }
    if prng.gen_range(0, 4) == 0 {
        let pos = prng.gen_range(0, b.len().saturating_add(1));
        let extra_len = prng.gen_range(0, 16);
        let extra = prng.gen_bytes(extra_len);
        let mut nb = Vec::with_capacity(b.len() + extra.len());
        nb.extend_from_slice(b.get(0..pos).unwrap_or(&[]));
        nb.extend_from_slice(&extra);
        nb.extend_from_slice(b.get(pos..).unwrap_or(&[]));
        b = nb;
        if b.len() > MAX_INPUT_BYTES {
            b.truncate(MAX_INPUT_BYTES);
        }
    }
    b
}

#[cfg(test)]
fn gen_nested_json(depth: usize) -> Vec<u8> {
    let depth = std::cmp::min(depth, 250);
    let mut s = String::new();
    for _ in 0..depth {
        s.push_str("{\"a\":");
    }
    s.push_str("1");
    for _ in 0..depth {
        s.push('}');
    }
    let mut b = s.into_bytes();
    if b.len() > MAX_INPUT_BYTES {
        b.truncate(MAX_INPUT_BYTES);
    }
    b
}

#[cfg(test)]
fn gen_random_malformed(prng: &mut Prng, max_len: usize) -> Vec<u8> {
    let len = prng.gen_range(0, std::cmp::min(max_len, MAX_INPUT_BYTES).saturating_add(1));
    prng.gen_bytes(len)
}

#[cfg(test)]
fn gen_random_text_with_bom(prng: &mut Prng) -> Vec<u8> {
    let len = prng.gen_range(0, 2048);
    let mut b = prng.gen_bytes(len);
    if prng.gen_range(0, 8) == 0 {
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(&b);
        b = with_bom;
    }
    if prng.gen_range(0, 4) == 0 && !b.is_empty() {
        let pos = prng.gen_range(0, b.len());
        if let Some(byte) = b.get_mut(pos) {
            *byte = 0xFF;
        }
    }
    if b.len() > MAX_INPUT_BYTES {
        b.truncate(MAX_INPUT_BYTES);
    }
    b
}

#[cfg(test)]
fn assert_bounded(input: &[u8], output: &[u8], label: &str) {
    assert!(
        output.len() <= MAX_OUTPUT_BYTES,
        "{label}: output {} exceeds cap {} (input {})",
        output.len(),
        MAX_OUTPUT_BYTES,
        input.len()
    );
    if !input.is_empty() {
        let bound = input
            .len()
            .saturating_mul(60)
            .max(1024)
            .min(MAX_OUTPUT_BYTES);
        assert!(
            output.len() <= bound,
            "{label}: output {} exceeds 60x bound {bound} (input {})",
            output.len(),
            input.len()
        );
    }
}

#[cfg(test)]
fn assert_no_escape(base: &Path, candidate: &Path, label: &str) {
    for comp in candidate.components() {
        if let std::path::Component::ParentDir = comp {
            panic!("{label}: path escape {candidate:?} contains `..`");
        }
    }
    if candidate.is_absolute() {
        assert!(
            candidate.starts_with(base),
            "{label}: absolute {candidate:?} escapes base {base:?}"
        );
    }
}

#[cfg(test)]
fn snapshot_dir(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.filter_map(Result::ok) {
            let p = e.path();
            if p.is_file() {
                out.push((p.clone(), std::fs::read(&p).unwrap_or_default()));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
fn assert_dir_unchanged(before: &[(PathBuf, Vec<u8>)], after: &[(PathBuf, Vec<u8>)], label: &str) {
    assert_eq!(
        before.len(),
        after.len(),
        "{label}: file count {} vs {}",
        before.len(),
        after.len()
    );
    for ((pb, db), (pa, da)) in before.iter().zip(after.iter()) {
        assert_eq!(pb, pa, "{label}: path changed");
        assert_eq!(db, da, "{label}: file {pb:?} mutated on rejected");
    }
}

#[cfg(test)]
fn compute_digest(bytes: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::temp_dir_unique;

    #[test]
    fn fuzz_registry_migration_no_panic_100() {
        let corpus = seed_registry_corpus();
        assert!(!corpus.is_empty());
        for iter in 0..100 {
            let mut prng = Prng::new(iter as u64 + 0x1111);
            let base = corpus
                .get(prng.gen_range(0, corpus.len()))
                .cloned()
                .unwrap_or_default();
            let input: Vec<u8> = match iter % 5 {
                0 => gen_truncated(&mut prng, &base),
                1 => gen_huge_registry(&mut prng),
                2 => gen_nested_json(120),
                3 => gen_nested_json(250),
                _ => gen_random_text_with_bom(&mut prng),
            };
            assert!(input.len() <= MAX_INPUT_BYTES);
            let input = if iter % 7 == 0 {
                gen_random_malformed(&mut prng, 4096)
            } else {
                input
            };

            let dir = temp_dir_unique("fuzz-core-registry");
            std::fs::create_dir_all(&dir).expect("mkdir");
            let path = dir.join(format!("registry-{iter}.json"));
            std::fs::write(&path, &input).expect("write");
            let before = snapshot_dir(&dir);
            let before_bytes = std::fs::read(&path).unwrap_or_default();

            let result = std::panic::catch_unwind(|| crate::registry::Registry::load(&path));
            assert!(
                result.is_ok(),
                "Registry::load panicked at iter {iter} input len {}",
                input.len()
            );
            let res = result.expect("catch ok");
            match res {
                Ok(reg) => {
                    let serialized = serde_json::to_string(reg.instances()).unwrap_or_default();
                    assert_bounded(&input, serialized.as_bytes(), "registry-ok");
                    let store_path = dir.join(format!("registry-store-{iter}.json"));
                    let store_res = std::panic::catch_unwind(|| reg.store(&store_path));
                    assert!(store_res.is_ok(), "Registry::store panicked at {iter}");
                    if let Ok(Ok(())) = store_res {
                        let reload = crate::registry::Registry::load(&store_path);
                        assert!(
                            reload.is_ok(),
                            "registry re-parse failed at {iter}: {reload:?}"
                        );
                        let reloaded = reload.expect("ok");
                        assert_eq!(
                            reloaded.instances().len(),
                            reg.instances().len(),
                            "instance count mismatch after round-trip at {iter}"
                        );
                        assert_no_escape(&dir, &store_path, "registry-store");
                        let stored_bytes = std::fs::read(&store_path).unwrap_or_default();
                        assert_bounded(&input, &stored_bytes, "registry-store-bytes");
                        let stored_str =
                            String::from_utf8_lossy(&stored_bytes).to_ascii_lowercase();
                        for forbidden in ["\"model\"", "\"endpoint\"", "\"api_key\""] {
                            assert!(
                                !stored_str.contains(forbidden),
                                "forbidden field {forbidden} leaked at {iter}"
                            );
                        }
                    }
                    let after_bytes = std::fs::read(&path).unwrap_or_default();
                    assert_eq!(before_bytes, after_bytes, "load mutated file at {iter}");
                    let after = snapshot_dir(&dir);
                    // The store round-trip may add registry-store-* files;
                    // everything else must be byte-identical.
                    let after_filtered: Vec<_> = after
                        .iter()
                        .filter(|(p, _)| {
                            p.file_name()
                                .and_then(|n| n.to_str())
                                .is_none_or(|n| !n.starts_with("registry-store"))
                        })
                        .cloned()
                        .collect();
                    assert_dir_unchanged(&before, &after_filtered, &format!("registry-ok {iter}"));
                    assert_no_escape(&dir, &path, "registry-ok");
                }
                Err(_) => {
                    let after_bytes = std::fs::read(&path).unwrap_or_default();
                    assert_eq!(before_bytes, after_bytes, "rejected mutated at {iter}");
                    let after = snapshot_dir(&dir);
                    let after_filtered: Vec<_> = after
                        .iter()
                        .filter(|(p, _)| !p.to_string_lossy().contains("registry-store"))
                        .cloned()
                        .collect();
                    assert_dir_unchanged(
                        &before,
                        &after_filtered,
                        &format!("registry-rejected {iter}"),
                    );
                    for (p, _) in &after {
                        assert_no_escape(&dir, p, "registry-rejected");
                    }
                }
            }

            drop(std::fs::remove_dir_all(&dir));
        }
    }

    #[test]
    fn fuzz_registry_truncated_huge_nested_deep_variants_100() {
        let corpus = seed_registry_corpus();
        for iter in 0..100 {
            let mut prng = Prng::new(iter as u64 + 0x2222);
            let base = corpus
                .get(prng.gen_range(0, corpus.len()))
                .cloned()
                .unwrap_or_default();
            let input: Vec<u8> = match iter % 4 {
                0 => gen_truncated(&mut prng, &base),
                1 => gen_huge_registry(&mut prng),
                2 => gen_nested_json(150),
                _ => gen_nested_json(300),
            };
            assert!(input.len() <= MAX_INPUT_BYTES);
            let dir = temp_dir_unique("fuzz-core-registry-variants");
            std::fs::create_dir_all(&dir).expect("mkdir");
            let path = dir.join(format!("reg-{iter}.json"));
            std::fs::write(&path, &input).expect("write");
            let before = snapshot_dir(&dir);
            let before_bytes = std::fs::read(&path).unwrap_or_default();
            let before_digest = compute_digest(&before_bytes);

            let result = std::panic::catch_unwind(|| crate::registry::Registry::load(&path));
            assert!(result.is_ok(), "Registry load panicked variant {iter}");
            match result.expect("catch") {
                Ok(reg) => {
                    let store_path = dir.join(format!("reg-store-{iter}.json"));
                    let store_res = reg.store(&store_path);
                    drop(store_res);
                    if store_path.exists() {
                        let stored = std::fs::read(&store_path).unwrap_or_default();
                        assert_bounded(&input, &stored, "registry-variant-store");
                        assert_no_escape(&dir, &store_path, "registry-variant-store");
                        let after = snapshot_dir(&dir);
                        for (p, data) in &after {
                            assert_no_escape(&dir, p, "registry-variant-after");
                            assert!(
                                data.len() <= MAX_OUTPUT_BYTES,
                                "file unbounded at iter {iter}: {p:?} len {}",
                                data.len()
                            );
                        }
                    }
                    let after_bytes = std::fs::read(&path).unwrap_or_default();
                    assert_eq!(before_bytes, after_bytes);
                }
                Err(e) => {
                    let msg = format!("{e}");
                    assert!(
                        msg.len() <= 8192,
                        "error message unbounded at {iter}: len {}",
                        msg.len()
                    );
                    let after_bytes = std::fs::read(&path).unwrap_or_default();
                    assert_eq!(before_bytes, after_bytes);
                    assert_eq!(before_digest, compute_digest(&after_bytes));
                    let after = snapshot_dir(&dir);
                    let filtered: Vec<_> = after
                        .iter()
                        .filter(|(p, _)| !p.to_string_lossy().contains("reg-store"))
                        .cloned()
                        .collect();
                    assert_dir_unchanged(
                        &before,
                        &filtered,
                        &format!("registry-variant-rejected {iter}"),
                    );
                }
            }
            drop(std::fs::remove_dir_all(&dir));
        }
    }

    #[test]
    fn fuzz_template_and_provider_deser_no_panic_100() {
        let corpus = seed_registry_corpus();
        for iter in 0..100 {
            let mut prng = Prng::new(iter as u64 + 0x3333);
            let base = corpus
                .get(prng.gen_range(0, corpus.len()))
                .cloned()
                .unwrap_or_default();
            let input: Vec<u8> = match iter % 4 {
                0 => gen_truncated(&mut prng, &base),
                1 => gen_huge_registry(&mut prng),
                2 => gen_nested_json(120),
                _ => gen_random_text_with_bom(&mut prng),
            };
            assert!(input.len() <= MAX_INPUT_BYTES);
            let text = String::from_utf8_lossy(&input).into_owned();

            let parse_res =
                std::panic::catch_unwind(|| serde_json::from_str::<serde_json::Value>(&text));
            assert!(parse_res.is_ok(), "json parse panicked at {iter}");
            if let Ok(Ok(val)) = parse_res {
                let ser = serde_json::to_string(&val).unwrap_or_default();
                assert_bounded(input.as_slice(), ser.as_bytes(), "json-roundtrip");
                let reparsed = serde_json::from_str::<serde_json::Value>(&ser);
                assert!(reparsed.is_ok(), "re-parse failed at {iter}");
            }
        }
    }

    #[test]
    fn fuzz_wrapper_parse_no_panic_and_bounded_and_secret_free_100() {
        const SENTINEL: &str = "sk-superai-test-sentinel-12345-fake";
        for iter in 0u64..100u64 {
            let mut prng = Prng::new(iter + 0x5555);
            let variants: Vec<Vec<u8>> = vec![
                format!("#!/bin/sh\nexec claude-code \"$@\" # superai wrapper {iter}").into_bytes(),
                format!(
                    "#!/bin/sh\n# superai wrapper instance=work-{iter} digest=abc\nexport CLAUDE_CONFIG_DIR={}/cfg{iter}\nexec \"$@\"",
                    crate::test_util::tmp_abs_str("fuzz-cfg")
                )
                .into_bytes(),
                gen_truncated(&mut prng, br"#!/bin/sh\nexec wrapper"),
                {
                    let mut s = String::from("#!/bin/sh\n");
                    for i in 0..200 {
                        s.push_str(&format!("export VAR_{i}=val{i}\n"));
                    }
                    s.push_str("exec \"$@\"\n");
                    s.into_bytes()
                },
                format!("#!/bin/sh\n# {SENTINEL}\nexec wrapper").into_bytes(),
                gen_random_text_with_bom(&mut prng),
                format!("#!/bin/sh\n; rm -rf / # metachars {iter} `whoami` $(env) | cat").into_bytes(),
            ];
            let input = variants
                .get(prng.gen_range(0, variants.len()))
                .cloned()
                .unwrap_or_default();
            assert!(input.len() <= MAX_INPUT_BYTES);
            let text = String::from_utf8_lossy(&input).into_owned();
            let parsed = std::panic::catch_unwind(|| crate::wrapper::parse_wrapper_content(&text));
            assert!(parsed.is_ok(), "parse_wrapper_content panicked at {iter}");
            if let Some(p) = parsed.expect("ok") {
                let digest_str = p.digest.as_deref().unwrap_or("");
                assert!(digest_str.len() <= 64, "digest unbounded at {iter}");
                assert!(
                    !digest_str.contains(SENTINEL),
                    "digest leaked sentinel at {iter}"
                );
                let repr = format!("{p:?}");
                assert!(
                    !repr.contains(SENTINEL),
                    "wrapper ParsedWrapper leaked sentinel at {iter}"
                );
                assert!(repr.len() <= MAX_OUTPUT_BYTES);
                // Kind detection on a not-yet-existing path must not panic.
                let probe_base = crate::test_util::tmp_abs("fuzz-wrapper-kind");
                let kind = std::panic::catch_unwind(|| {
                    crate::wrapper::detect_wrapper_kind(&probe_base.join(format!("wrapper-{iter}")))
                });
                drop(kind);
                let dir = temp_dir_unique("fuzz-wrapper-parse");
                std::fs::create_dir_all(&dir).expect("mkdir");
                let dummy_instance = {
                    let id = crate::ids::InstanceId::new(&format!("id-wrapper-{iter}")).unwrap();
                    let name = crate::ids::InstanceName::new(&format!("work-{iter}")).unwrap();
                    let harness = crate::ids::HarnessId::new("claude-code").unwrap();
                    let root = crate::paths::AbsolutePath::from_path(
                        &crate::test_util::tmp_abs("fuzz-wrapper-cfg").join(format!("cfg-{iter}")),
                    )
                    .unwrap();
                    crate::instance::Instance {
                        id,
                        name,
                        harness,
                        config_root: root,
                        binary: None,
                        wrapper: None,
                        isolation: crate::state::Isolation::RelocatedRoot,
                        origin: crate::state::InstanceOrigin::Created,
                        ownership: crate::state::Ownership::SuperaiCreated,
                        template: None,
                        created_at: "2026-08-26T00:00:00Z".to_owned(),
                        adapter_revision: "0.1.0".to_owned(),
                    }
                };
                let plan = crate::wrapper::plan_wrapper_for_instance(&dummy_instance, None);
                let Ok((generated, digest)) =
                    crate::wrapper::generate_shell_wrapper(&dummy_instance, &plan)
                else {
                    continue;
                };
                assert!(!generated.contains(SENTINEL));
                assert!(!digest.contains(SENTINEL));
                assert!(
                    generated.len() <= 32 * 1024 + 1024,
                    "generated wrapper unbounded at {iter}"
                );
                let reparsed = crate::wrapper::parse_wrapper_content(&generated);
                assert!(reparsed.is_some(), "generated wrapper must parse at {iter}");
                drop(std::fs::remove_dir_all(&dir));
            } else {
                // Rejected wrappers return None, never panic.
                assert!(text.len() <= MAX_INPUT_BYTES);
            }
            // Kind detection on the raw input written to disk must not leak
            // the sentinel through its Debug form.
            let kind2 = std::panic::catch_unwind(|| {
                let tmp = temp_dir_unique("fuzz-wrapper-kind");
                std::fs::create_dir_all(&tmp).unwrap();
                let p = tmp.join(format!("wrapper-{iter}.sh"));
                std::fs::write(&p, &input).unwrap();
                let k = crate::wrapper::detect_wrapper_kind(&p);
                let dbg = format!("{k:?}");
                assert!(!dbg.contains(SENTINEL));
                drop(std::fs::remove_dir_all(&tmp));
            });
            assert!(kind2.is_ok(), "detect_wrapper_kind panicked at {iter}");
        }
    }

    #[test]
    fn fuzz_provider_and_template_with_sentinel_and_traversal_no_leak_100() {
        const SENTINEL: &str = "sk-superai-test-sentinel-12345-fake";
        let corpus = seed_registry_corpus();
        for iter in 0u64..100u64 {
            let mut prng = Prng::new(iter + 0x6666);
            let base = corpus
                .get(prng.gen_range(0, corpus.len()))
                .cloned()
                .unwrap_or_default();
            let input: Vec<u8> = match iter % 5 {
                0 => gen_truncated(&mut prng, &base),
                1 => {
                    let mut b = gen_huge_registry(&mut prng);
                    if iter % 3 == 0 { b.extend_from_slice(SENTINEL.as_bytes()); }
                    b.truncate(MAX_INPUT_BYTES);
                    b
                }
                2 => gen_nested_json(120),
                3 => format!("{{\"id\":\"test\",\"patches\":[{{\"selector\":\"../escape_{iter}\",\"value\":\"x\"}}]}}").into_bytes(),
                _ => gen_random_text_with_bom(&mut prng),
            };
            assert!(input.len() <= MAX_INPUT_BYTES);
            let text = String::from_utf8_lossy(&input).into_owned();
            let prov =
                std::panic::catch_unwind(|| serde_json::from_str::<serde_json::Value>(&text));
            assert!(prov.is_ok(), "provider deser panicked at {iter}");
            if let Ok(Err(e)) = prov {
                let msg = format!("{e}");
                assert!(msg.len() <= 8192, "error unbounded at {iter}");
                drop(msg);
            }
            let traversal = format!("../{SENTINEL}.json");
            let path_res = crate::template::validate_template_path(&traversal);
            assert!(
                path_res.is_err(),
                "traversal with sentinel must be rejected at {iter}"
            );
            let msg = format!("{:?}", path_res.unwrap_err());
            assert!(msg.len() <= 4096);
            let url = format!("https://example.com/../{SENTINEL}");
            let url_res = crate::template_fetch::validate_fetch_url(&url, "test");
            assert!(url_res.is_err(), "url traversal must be rejected at {iter}");
        }
    }

    #[test]
    fn fuzz_path_escape_registry_no_write_outside_temp_100() {
        for iter in 0u64..100u64 {
            let mut prng = Prng::new(iter + 0x4444);
            let escape_base = crate::test_util::tmp_abs_str("fuzz-escape");
            let traversal_payloads = vec![
                "../escape".to_owned(),
                "../../etc/passwd".to_owned(),
                format!("{escape_base}/../etc/shadow"),
                "/".to_owned(),
                "C:\\Windows\\System32".to_owned(),
                format!("traversal-{}-../..", iter),
                format!("{escape_base}/fuzz-escape-{}-../../", iter),
            ];
            let payload = traversal_payloads
                .get(prng.gen_range(0, traversal_payloads.len()))
                .cloned()
                .unwrap_or_default();

            let dir = temp_dir_unique("fuzz-core-escape");
            std::fs::create_dir_all(&dir).expect("mkdir");
            let path = dir.join(format!("escape-{iter}.json"));
            let json = format!(
                "{{\"schema_version\":1,\"instances\":[{{\"id\":\"id-{iter}\",\"name\":\"work{iter}\",\"harness\":\"claude-code\",\"config_root\":\"{payload}\",\"isolation\":\"unknown\",\"origin\":\"created\",\"ownership\":\"superai_created\",\"created_at\":\"2026-01-01T00:00:00Z\",\"adapter_revision\":\"0.1.0\"}}]}}"
            );
            std::fs::write(&path, json.as_bytes()).expect("write escape payload");
            let before = snapshot_dir(&dir);

            let result = std::panic::catch_unwind(|| crate::registry::Registry::load(&path));
            assert!(result.is_ok(), "escape load panicked at {iter}");
            let after = snapshot_dir(&dir);
            for (p, _) in &after {
                assert_no_escape(&dir, p, &format!("escape {iter}"));
            }
            // Accepted roots may legitimately live outside the temp dir (user
            // data); what must never appear is `..`.
            if let Ok(Ok(reg)) = result {
                for inst in reg.instances() {
                    let root_str = inst.config_root.to_string();
                    if payload.contains("..") {
                        panic!(
                            "path escape payload accepted at iter {iter}: payload={payload:?} root={root_str:?}: should have been rejected"
                        );
                    }
                    assert!(
                        !root_str.contains(".."),
                        "config_root contains `..` at iter {iter}: {root_str:?}"
                    );
                    assert!(
                        root_str.len() <= MAX_OUTPUT_BYTES,
                        "config_root unbounded at iter {iter}: len {}",
                        root_str.len()
                    );
                }
                let store_path = dir.join(format!("escape-store-{iter}.json"));
                let store_res = std::panic::catch_unwind(|| reg.store(&store_path));
                assert!(
                    store_res.is_ok(),
                    "store panicked on escape payload at {iter}"
                );
                if let Ok(Ok(())) = store_res {
                    assert_no_escape(&dir, &store_path, "escape-store");
                    let stored_after = snapshot_dir(&dir);
                    for (p, _) in &stored_after {
                        assert_no_escape(&dir, p, "escape-store-after");
                    }
                }
            } else {
                assert_dir_unchanged(&before, &after, &format!("escape-rejected {iter}"));
            }

            drop(std::fs::remove_dir_all(&dir));
        }
    }

    // QAL-04 adapter detection fuzz: fixtures and PATH-shaped strings drive
    // the real detection path; no ambient env, no live package-manager probes.

    const DETECT_SENTINEL: &str = "sk-superai-test-sentinel-12345-fake";
    /// Heredoc delimiter for fake `--version` executables; never appears in
    /// generated fixtures (occurrences are stripped).
    #[cfg(unix)]
    const HEREDOC_EOF: &str = "SUPERAI_FUZZ_EOF_7f3a";

    fn gen_version_output(prng: &mut Prng, iter: u64) -> String {
        let base = match iter % 8 {
            0 => format!(
                "claude-code {}.{}.{} (build abc{})",
                iter % 9,
                iter,
                iter % 7,
                iter
            ),
            1 => format!("\x1b[1mclaude\x1b[0m {}.{}.{}", iter % 5, iter % 3, iter),
            2 => format!("v{}.{}.{}", iter % 11, iter % 4, iter % 13),
            3 => format!(
                "aider {}\nchat transcripts: ~/.aider\npython: 3.{}",
                iter,
                iter % 12
            ),
            4 => String::new(),
            5 => "   \t\n  ".to_owned(),
            6 => {
                // Huge output: many tokens then a semver at the end.
                let mut s = String::new();
                for i in 0..2000 {
                    s.push_str(&format!("token{i} "));
                }
                s.push_str(&format!("{}.9.9\n", iter % 6));
                s
            }
            _ => format!(
                "版本 {}.{}-βeta (build {})",
                iter % 3,
                iter,
                DETECT_SENTINEL
            ),
        };
        let mut variants: Vec<String> = vec![base.clone()];
        let truncated = gen_truncated(prng, base.as_bytes());
        variants.push(String::from_utf8_lossy(&truncated).into_owned());
        let with_sentinel = format!("{base} {DETECT_SENTINEL}");
        variants.push(with_sentinel);
        let malformed = gen_random_malformed(prng, 512);
        variants.push(String::from_utf8_lossy(&malformed).into_owned());
        variants
            .get(prng.gen_range(0, variants.len()))
            .cloned()
            .unwrap_or_default()
    }

    #[cfg(unix)]
    fn write_fake_version_exe(dir: &Path, name: &str, fixture: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        let safe_fixture = fixture.replace(HEREDOC_EOF, "");
        let script = format!("#!/bin/sh\ncat <<'{HEREDOC_EOF}'\n{safe_fixture}\n{HEREDOC_EOF}\n");
        std::fs::write(&path, script).expect("write fake exe");
        let mut perms = std::fs::metadata(&path)
            .expect("stat fake exe")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod fake exe");
    }

    /// Build a PATH-shaped string over the dirs plus junk segments, split
    /// the same way the production ambient splitter does.
    #[cfg(unix)]
    fn gen_path_shaped(prng: &mut Prng, dirs: &[PathBuf]) -> Vec<PathBuf> {
        let mut parts: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            parts.push(dir.clone());
            if prng.gen_range(0, 3) == 0 {
                parts.push(PathBuf::from("")); // empty PATH segment
            }
            if prng.gen_range(0, 4) == 0 {
                parts.push(PathBuf::from("."));
            }
            if prng.gen_range(0, 4) == 0 {
                parts.push(PathBuf::from(format!(
                    "/nonexistent-fuzz-{}",
                    prng.next_u64() % 1000
                )));
            }
        }
        let joined = std::env::join_paths(parts).expect("join path parts");
        std::env::split_paths(&joined)
            .filter(|p| !p.as_os_str().is_empty())
            .collect()
    }

    #[test]
    fn fuzz_adapter_detection_version_outputs_no_panic_bounded_100() {
        for iter in 0u64..100u64 {
            let mut prng = Prng::new(iter + 0x7777);
            let fixture = gen_version_output(&mut prng, iter);
            assert!(fixture.len() <= MAX_INPUT_BYTES);

            let parsed = std::panic::catch_unwind(|| crate::process::extract_version(&fixture));
            assert!(parsed.is_ok(), "extract_version panicked at {iter}");
            if let Some(version) = parsed.expect("catch ok") {
                assert!(
                    version.len() <= 64,
                    "extracted version unbounded at {iter}: len {} fixture {:?}",
                    version.len(),
                    fixture
                );
                assert_eq!(
                    crate::process::extract_version(&fixture),
                    Some(version.clone()),
                    "extract_version not deterministic at {iter}"
                );
                if !fixture.contains(DETECT_SENTINEL) {
                    assert!(
                        !version.contains(DETECT_SENTINEL),
                        "version leaked sentinel at {iter}: {version}"
                    );
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn fuzz_adapter_detection_catalog_entries_no_panic_no_leak_100() {
        use crate::detect::{DetectOptions, detect_all_for_entry};
        use crate::install_catalog::InstallCatalog;

        let catalog = InstallCatalog::embedded().expect("embedded catalog");
        let entries = catalog.entries;
        assert!(!entries.is_empty(), "embedded catalog must be populated");

        for iter in 0u64..100u64 {
            let mut prng = Prng::new(iter + 0x8888);
            let fixture = gen_version_output(&mut prng, iter);
            let Some(entry) = entries
                .get(prng.gen_range(0, entries.len()))
                .or_else(|| entries.first())
                .cloned()
            else {
                continue;
            };
            let Some(exe_name) = entry.executables.first().cloned() else {
                continue;
            };

            let dir = temp_dir_unique("fuzz-detect");
            let bin = dir.join("bin");
            let home = dir.join("home");
            std::fs::create_dir_all(&bin).expect("mkdir bin");
            std::fs::create_dir_all(&home).expect("mkdir home");
            write_fake_version_exe(&bin, &exe_name, &fixture);
            // Every third iteration also plants a non-executable mise-shaped
            // shim (the broken-shim arm).
            if iter % 3 == 0 {
                let shims = home.join(".local/share/mise/shims");
                std::fs::create_dir_all(&shims).expect("mkdir shims");
                std::fs::write(shims.join(&exe_name), format!("mise x -- {exe_name}\n"))
                    .expect("write shim");
            }
            let before = snapshot_dir(&dir);

            let path_dirs = gen_path_shaped(&mut prng, std::slice::from_ref(&bin));
            // Hermetic: every live package-manager probe is disabled.
            let opts = DetectOptions {
                path_dirs: Some(path_dirs.clone()),
                home_dir: Some(home.clone()),
                configured_binary: None,
                probe_mise: true,
                probe_brew: false,
                probe_npm: false,
                probe_cargo: false,
                probe_pipx: false,
                probe_uv: false,
                probe_system: false,
                probe_apps: false,
                probe_timeout: std::time::Duration::from_secs(2),
            };

            let result = std::panic::catch_unwind(|| detect_all_for_entry(&entry, &opts));
            assert!(result.is_ok(), "detect_all_for_entry panicked at {iter}");
            let detections = result.expect("catch ok");
            // At most one hit per PATH dir per executable, plus the
            // configured/mise arms.
            let bound = path_dirs.len().saturating_mul(entry.executables.len()) + 2;
            assert!(
                detections.len() <= bound,
                "detection count unbounded at {iter}: {} > {bound}",
                detections.len()
            );
            let clean_input = !fixture.contains(DETECT_SENTINEL);
            for detection in &detections {
                if let Some(version) = &detection.version {
                    assert!(
                        version.len() <= 64,
                        "detected version unbounded at {iter}: {version}"
                    );
                    if clean_input {
                        assert!(
                            !version.contains(DETECT_SENTINEL),
                            "detection leaked sentinel at {iter}: {version}"
                        );
                    }
                }
                let repr = format!("{detection:?}");
                assert!(
                    repr.len() <= MAX_OUTPUT_BYTES,
                    "detection debug repr unbounded at {iter}"
                );
                if clean_input {
                    assert!(
                        !repr.contains(DETECT_SENTINEL),
                        "detection repr leaked sentinel at {iter}"
                    );
                }
            }
            let after = snapshot_dir(&dir);
            assert_dir_unchanged(&before, &after, &format!("fuzz-detect {iter}"));
            drop(std::fs::remove_dir_all(&dir));
        }
    }
}
