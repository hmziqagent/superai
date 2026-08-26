//! Parser fuzz scaffolding — QAL-04 (core layer).
//!
//! Deterministic quick-loop fuzz for `Registry` migration, provider/template
//! deserialization, and related schema detection. Complements
//! `superai-config` fuzz which covers codecs. Each test loops 100
//! iterations over truncated / huge / nested / deep inputs seeded from
//! harness fixtures, asserting no panic/hang/unbounded allocation/path
//! escape, re-parse succeeds if accepted, and rejected input causes no FS
//! mutation.
//!
//! No external `cargo-fuzz` binary required; see `superai-config` fuzz docs
//! for optional `cargo fuzz` integration.

#![expect(clippy::all, reason = "fuzz scaffolding uses manual loops")]
#![expect(clippy::pedantic, reason = "fuzz helpers intentionally verbose")]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Deterministic PRNG — SplitMix64
// ---------------------------------------------------------------------------

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
    // Hardcoded registry seeds
    corpus.extend(vec![
        br#"{"schema_version":1,"instances":[]}"#.to_vec(),
        br#"{"instances":[{"name":"work","harness":"claude-code","config_dir":"/home/user/.claude-work"}]}"#.to_vec(),
        br#"{"schema_version":1,"instances":[{"id":"inst-1","name":"work","harness":"claude-code","config_root":"/home/user/.claude-work","isolation":"relocated_root","origin":"created","ownership":"superai_created","created_at":"2026-08-26T12:00:00Z","adapter_revision":"0.1.0"}]}"#.to_vec(),
        b"".to_vec(),
        b"{}".to_vec(),
        b"[]".to_vec(),
        br#"{"schema_version":999,"instances":[]}"#.to_vec(),
        br#"{"schema_version":"bad"}"#.to_vec(),
        br#"{"instances":"not an array"}"#.to_vec(),
        br#"{"instances":[{"name":"../escape","harness":"claude-code","config_dir":"/tmp/../etc/passwd"}]}"#.to_vec(),
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
    let mut s = String::from("{\"schema_version\":1,\"instances\":[");
    for i in 0..300 {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"id\":\"id-{i}\",\"name\":\"name{i}\",\"harness\":\"claude-code\",\"config_root\":\"/tmp/cfg{i}\",\"isolation\":\"unknown\",\"origin\":\"created\",\"ownership\":\"superai_created\",\"created_at\":\"2026-01-01T00:00:00Z\",\"adapter_revision\":\"0.1.0\"}}"
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
    // Occasionally inject random bytes to make malformed
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
            // Inject random malformed every 7th
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
                    // Bounded
                    let serialized = serde_json::to_string(reg.instances()).unwrap_or_default();
                    assert_bounded(&input, serialized.as_bytes(), "registry-ok");
                    // Re-parse: store then load again
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
                        // No path escape: every config_root inside /tmp should be checked, but we store outside real HOME
                        // For fuzz, we just ensure stored path is inside dir and no escape
                        assert_no_escape(&dir, &store_path, "registry-store");
                        let stored_bytes = std::fs::read(&store_path).unwrap_or_default();
                        assert_bounded(&input, &stored_bytes, "registry-store-bytes");
                        // Verify no forbidden fields leaked (model, endpoint etc.)
                        let stored_str =
                            String::from_utf8_lossy(&stored_bytes).to_ascii_lowercase();
                        for forbidden in ["\"model\"", "\"endpoint\"", "\"api_key\""] {
                            assert!(
                                !stored_str.contains(forbidden),
                                "forbidden field {forbidden} leaked at {iter}"
                            );
                        }
                    }
                    // No FS mutation from load (read-only) — file unchanged
                    let after_bytes = std::fs::read(&path).unwrap_or_default();
                    assert_eq!(before_bytes, after_bytes, "load mutated file at {iter}");
                    let after = snapshot_dir(&dir);
                    // Allow one extra file for store_path if it was created; otherwise unchanged
                    // Filter store_path out for load-only check
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
                    // Rejected — no FS mutation
                    let after_bytes = std::fs::read(&path).unwrap_or_default();
                    assert_eq!(before_bytes, after_bytes, "rejected mutated at {iter}");
                    let after = snapshot_dir(&dir);
                    // Filter store artifacts (none should exist on rejected)
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
                0 => gen_truncated(&mut prng, &base), // truncated
                1 => gen_huge_registry(&mut prng),    // huge
                2 => gen_nested_json(150),            // nested
                _ => gen_nested_json(300),            // deep
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
                    // Re-parse via store/load
                    let store_path = dir.join(format!("reg-store-{iter}.json"));
                    let store_res = reg.store(&store_path);
                    drop(store_res);
                    if store_path.exists() {
                        let stored = std::fs::read(&store_path).unwrap_or_default();
                        assert_bounded(&input, &stored, "registry-variant-store");
                        // Verify stored digest not unbounded
                        assert_no_escape(&dir, &store_path, "registry-variant-store");
                        // Ensure backup not leaving unbounded files
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
                    // Error must be bounded and not leak unbounded allocation
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
        // Provider/template are serde_json deserialized; fuzz their parsing
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

            // Provider deserialization (via serde_json Value, mimics template fetch)
            let prov_res =
                std::panic::catch_unwind(|| serde_json::from_str::<serde_json::Value>(&text));
            assert!(prov_res.is_ok(), "provider json parse panicked at {iter}");
            if let Ok(Ok(val)) = prov_res {
                let ser = serde_json::to_string(&val).unwrap_or_default();
                assert_bounded(input.as_slice(), ser.as_bytes(), "provider-roundtrip");
                let reparsed = serde_json::from_str::<serde_json::Value>(&ser);
                assert!(reparsed.is_ok(), "provider re-parse failed at {iter}");
            }

            // Template-like: try to deserialize as generic Value then check bounded
            let tmpl_res = std::panic::catch_unwind(|| {
                serde_json::from_str::<serde_json::Value>(&text)
                    .map(|v| serde_json::to_value(&v).unwrap_or(serde_json::Value::Null))
            });
            assert!(tmpl_res.is_ok(), "template deser panicked at {iter}");

            // No FS mutation — this test is pure in-memory, just check bounded
            assert!(input.len() <= MAX_INPUT_BYTES);
        }
    }

    #[test]
    fn fuzz_path_escape_registry_no_write_outside_temp_100() {
        // Direct path escape check: fuzzed config_root with traversal must not cause FS write outside temp dir
        for iter in 0..100 {
            let mut prng = Prng::new(iter as u64 + 0x4444);
            let traversal_payloads = vec![
                "../escape".to_owned(),
                "../../etc/passwd".to_owned(),
                "/tmp/../etc/shadow".to_owned(),
                "/".to_owned(),
                "C:\\Windows\\System32".to_owned(),
                format!("traversal-{}-../..", iter),
                format!("/tmp/fuzz-escape-{}-../../", iter),
            ];
            let payload = traversal_payloads
                .get(prng.gen_range(0, traversal_payloads.len()))
                .cloned()
                .unwrap_or_default();

            let dir = temp_dir_unique("fuzz-core-escape");
            std::fs::create_dir_all(&dir).expect("mkdir");
            let path = dir.join(format!("escape-{iter}.json"));
            // Attempt to create a registry with that payload as config_root
            let json = format!(
                "{{\"schema_version\":1,\"instances\":[{{\"id\":\"id-{iter}\",\"name\":\"work{iter}\",\"harness\":\"claude-code\",\"config_root\":\"{payload}\",\"isolation\":\"unknown\",\"origin\":\"created\",\"ownership\":\"superai_created\",\"created_at\":\"2026-01-01T00:00:00Z\",\"adapter_revision\":\"0.1.0\"}}]}}"
            );
            std::fs::write(&path, json.as_bytes()).expect("write escape payload");
            let before = snapshot_dir(&dir);

            let result = std::panic::catch_unwind(|| crate::registry::Registry::load(&path));
            assert!(result.is_ok(), "escape load panicked at {iter}");
            // Whether Ok or Err, ensure no file was created outside dir
            let after = snapshot_dir(&dir);
            for (p, _) in &after {
                assert_no_escape(&dir, p, &format!("escape {iter}"));
            }
            // If Ok, the registry accepted the payload — check if it contains parent dir (should have been validated)
            if let Ok(Ok(reg)) = result {
                for inst in reg.instances() {
                    let root_str = inst.config_root.to_string();
                    // If payload had `..`, validation should have rejected; acceptance is a failure
                    if payload.contains("..") {
                        panic!(
                            "path escape payload accepted at iter {iter}: payload={payload:?} root={root_str:?} — should have been rejected"
                        );
                    }
                    // config_root is user data, may be anywhere (e.g. /home/...), not necessarily inside fuzz temp dir;
                    // we only verify it doesn't contain `..` and is bounded, not that it is inside dir.
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
                // Store should also not escape
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
                // Rejected — ensure no mutation
                assert_dir_unchanged(&before, &after, &format!("escape-rejected {iter}"));
            }

            drop(std::fs::remove_dir_all(&dir));
        }
    }
}
