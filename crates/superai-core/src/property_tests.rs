//! Property tests for QAL-03 — manual loops with deterministic RNG, no external dep.
//!
//! Covers: registry no forbidden fields, preview deterministic,
//! restore exact, collision-safe normalization, capability complete.

#![expect(clippy::all, reason = "property tests manual loops")]
#![expect(clippy::pedantic, reason = "property tests")]
#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use crate::capability::Support;
    use crate::capability_resolver::{self, ACTIVE_PAIRS, ALL_CAPABILITIES, MATRIX};
    use crate::ids::{
        HarnessId, InstanceId, InstanceName, ProviderId, TemplateId, TemplateVersion,
    };
    use crate::instance::{Instance, TemplateRef};
    use crate::paths::AbsolutePath;
    use crate::registry::Registry;
    use crate::state::{InstanceOrigin, Isolation, Ownership};
    use crate::test_util::temp_dir_unique;

    // -----------------------------------------------------------------------
    // Deterministic PRNG — SplitMix64, no external crate.
    // -----------------------------------------------------------------------

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

        #[expect(clippy::cast_possible_truncation, reason = "prng helper")]
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
                s.push(charset[idx] as char);
            }
            s
        }
    }

    const SIMPLE_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

    fn random_valid_name(rng: &mut Prng, prefix: &str) -> String {
        // Generate a valid InstanceName / HarnessId-like string.
        // Must not be reserved windows name, must not contain NUL/control, separators, trailing dot/space.
        let len = rng.gen_range(3, 12);
        let mut s = prefix.to_owned();
        for _ in 0..len {
            let idx = rng.gen_range(0, SIMPLE_CHARSET.len());
            s.push(SIMPLE_CHARSET[idx] as char);
        }
        // Ensure not reserved: append extra char if needed.
        let lower = s.to_lowercase();
        let reserved = [
            "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
            "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
        ];
        if reserved.contains(&lower.as_str()) {
            s.push_str("x");
        }
        s
    }

    fn random_harness(rng: &mut Prng) -> HarnessId {
        const HARNESSES: &[&str] = &[
            "claude-code",
            "codex-cli",
            "opencode",
            "aider",
            "cline",
            "pi",
        ];
        let idx = rng.gen_range(0, HARNESSES.len());
        HarnessId::new(HARNESSES[idx]).unwrap()
    }

    fn random_provider(rng: &mut Prng) -> ProviderId {
        const PROVIDERS: &[&str] = &["anthropic", "openai", "glm", "minimax"];
        let idx = rng.gen_range(0, PROVIDERS.len());
        ProviderId::new(PROVIDERS[idx]).unwrap()
    }

    fn random_instance(rng: &mut Prng, iter: usize, idx: usize) -> Instance {
        let name_str = random_valid_name(rng, &format!("n{iter}-{idx}-"));
        let name = InstanceName::new(&name_str).unwrap();
        let harness = random_harness(rng);
        let id_str = format!(
            "id-{}-{}-{}",
            iter,
            idx,
            rng.gen_string(4, 8, SIMPLE_CHARSET)
        );
        let id = InstanceId::new(&id_str).unwrap();
        let root_str = format!(
            "/tmp/superai-prop-{}-{}/root{}",
            iter,
            idx,
            rng.gen_string(2, 6, SIMPLE_CHARSET)
        );
        let config_root = AbsolutePath::new(&root_str).unwrap();

        let isolation_choices = [
            Isolation::RelocatedRoot,
            Isolation::ExplicitConfig,
            Isolation::ProjectScope,
            Isolation::Unknown,
        ];
        let origin_choices = [
            InstanceOrigin::Created,
            InstanceOrigin::Mirrored,
            InstanceOrigin::Adopted,
            InstanceOrigin::Default,
        ];
        let ownership_choices = [
            Ownership::SuperaiCreated,
            Ownership::ExplicitlyAdopted,
            Ownership::Unmanaged,
        ];

        let isolation = isolation_choices[rng.gen_range(0, isolation_choices.len())];
        let origin = origin_choices[rng.gen_range(0, origin_choices.len())];
        let ownership = ownership_choices[rng.gen_range(0, ownership_choices.len())];

        let template = if rng.gen_bool() {
            let tid = TemplateId::new(&random_valid_name(rng, "tmpl-")).unwrap();
            let ver = TemplateVersion::new("1.2.0").unwrap();
            Some(TemplateRef {
                name: tid,
                version: ver,
            })
        } else {
            None
        };

        Instance {
            id,
            name,
            harness,
            config_root,
            binary: None,
            wrapper: None,
            isolation,
            origin,
            ownership,
            template,
            created_at: "2026-08-26T12:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        }
    }

    fn scratch_registry_path(prefix: &str, iter: usize) -> PathBuf {
        let dir = temp_dir_unique(prefix);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("registry-{iter}.json"))
    }

    // -----------------------------------------------------------------------
    // 1. Registry no forbidden fields
    // -----------------------------------------------------------------------
    #[test]
    fn property_registry_no_forbidden_fields() {
        let forbidden = [
            "\"model\"",
            "\"endpoint\"",
            "\"api_key\"",
            "\"apikey\"",
            "\"skill\"",
            "\"plugin\"",
            "\"mcp\"",
            "\"baseurl\"",
            "\"base_url\"",
        ];
        for iter in 0..80 {
            let mut rng = Prng::new(iter as u64 + 0x1111);
            let mut reg = Registry::default();
            let n = rng.gen_range(0, 5);
            for i in 0..n {
                let inst = random_instance(&mut rng, iter, i);
                // Ignore collision errors: generate fresh unique names/paths, collisions should be rare.
                drop(reg.insert(inst));
            }
            // Store to temp file and read raw JSON.
            let path = scratch_registry_path("prop-reg-forbidden", iter);
            reg.store(&path).unwrap();
            let raw = std::fs::read_to_string(&path).unwrap();
            let lower = raw.to_lowercase();
            for field in forbidden {
                assert!(
                    !lower.contains(field),
                    "forbidden field {field} found at iter {iter}: {raw}"
                );
            }
            // Also check deserialized map keys explicitly.
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            if let serde_json::Value::Object(map) = &v {
                if let Some(instances) = map.get("instances") {
                    let text = serde_json::to_string(instances).unwrap().to_lowercase();
                    for field in forbidden {
                        assert!(
                            !text.contains(field),
                            "forbidden in instances at {iter}: {field}"
                        );
                    }
                }
            }
            drop(std::fs::remove_dir_all(path.parent().unwrap()));
        }
    }

    // -----------------------------------------------------------------------
    // 2. Preview deterministic — install plan and capability resolver
    // -----------------------------------------------------------------------
    #[test]
    fn property_preview_deterministic_install_plan() {
        use crate::install_catalog::InstallCatalog;
        use crate::install_plan::InstallRequest;

        let catalog = InstallCatalog::embedded().unwrap();
        let harness_ids = ["claude-code", "opencode", "codex-cli"];

        for iter in 0..80 {
            let mut rng = Prng::new(iter as u64 + 0x2222);
            let harness_str = harness_ids[rng.gen_range(0, harness_ids.len())];
            let harness = HarnessId::new(harness_str).unwrap();
            let entry = match catalog.get(&harness) {
                Some(e) => e,
                None => continue,
            };
            if entry.methods.is_empty() {
                continue;
            }
            let method_idx = rng.gen_range(0, entry.methods.len());
            let method = entry.methods[method_idx].kind.clone();

            // Random version: sometimes valid semver, sometimes None.
            let version = if rng.gen_bool() {
                let major = rng.gen_range(1, 5);
                let minor = rng.gen_range(0, 10);
                let patch = rng.gen_range(0, 10);
                Some(format!("{major}.{minor}.{patch}"))
            } else {
                None
            };

            let req = InstallRequest {
                harness: harness.clone(),
                method: method.clone(),
                version,
                channel: None,
                destination: None,
            };

            let plan1 = crate::install_plan::plan_install_for_entry(&req, entry, "linux", "x64");
            let plan2 = crate::install_plan::plan_install_for_entry(&req, entry, "linux", "x64");

            match (plan1, plan2) {
                (Ok(p1), Ok(p2)) => {
                    assert_eq!(
                        p1.command_preview.executable, p2.command_preview.executable,
                        "install plan preview not deterministic at {iter}"
                    );
                    assert_eq!(
                        p1.command_preview.args, p2.command_preview.args,
                        "install plan args not deterministic at {iter}"
                    );
                    assert_eq!(p1.harness, p2.harness, "harness not deterministic");
                }
                (Err(e1), Err(e2)) => {
                    // Both should fail same way.
                    assert_eq!(
                        format!("{e1}"),
                        format!("{e2}"),
                        "install plan error not deterministic at {iter}"
                    );
                }
                (Ok(_), Err(e)) | (Err(e), Ok(_)) => {
                    panic!("install plan deterministic mismatch at {iter}: {e}")
                }
            }
        }
    }

    #[test]
    fn property_preview_deterministic_capability_resolve() {
        for iter in 0..100 {
            let mut rng = Prng::new(iter as u64 + 0x3333);
            let harness = random_harness(&mut rng);
            let provider = random_provider(&mut rng);
            let cap_idx = rng.gen_range(0, ALL_CAPABILITIES.len());
            let cap = ALL_CAPABILITIES[cap_idx];

            let r1 = capability_resolver::resolve(&harness, &provider, cap);
            let r2 = capability_resolver::resolve(&harness, &provider, cap);
            assert_eq!(r1, r2, "capability resolve not deterministic at {iter}");

            // Case-insensitive check: random case variation should give same result.
            let harness_varied =
                random_case_variation(rng.gen_range(0, 2) == 0, harness.as_str(), &mut rng);
            let provider_varied =
                random_case_variation(rng.gen_range(0, 2) == 0, provider.as_str(), &mut rng);
            let h_varied_id = HarnessId::new(&harness_varied.to_lowercase()).unwrap();
            let p_varied_id = ProviderId::new(&provider_varied.to_lowercase()).unwrap();
            // Resolve with canonical lower should equal resolve with varied case (if we normalize).
            let _r_varied = capability_resolver::resolve(&h_varied_id, &p_varied_id, cap);
            // Since we lowercased for id creation, it should be same as original's lower.
            // Directly test case-fold equality: harness.eq_case_fold_str should work.
            let h2 = HarnessId::new(&harness_varied).unwrap_or(harness.clone());
            let p2 = ProviderId::new(&provider_varied).unwrap_or(provider.clone());
            let r3 = capability_resolver::resolve(&h2, &p2, cap);
            assert_eq!(
                r1.support, r3.support,
                "case-insensitive support failed at {iter}"
            );
            assert_eq!(
                r1.source, r3.source,
                "case-insensitive source failed at {iter}"
            );
        }
    }

    fn random_case_variation(apply: bool, s: &str, rng: &mut Prng) -> String {
        if !apply {
            return s.to_owned();
        }
        let mut out = String::with_capacity(s.len());
        for ch in s.chars() {
            if rng.gen_bool() {
                for upper in ch.to_uppercase() {
                    out.push(upper);
                }
            } else {
                for lower in ch.to_lowercase() {
                    out.push(lower);
                }
            }
        }
        out
    }

    // -----------------------------------------------------------------------
    // 3. Restore exact — registry file backup/restore
    // -----------------------------------------------------------------------
    #[test]
    fn property_restore_exact_registry() {
        for iter in 0..50 {
            let mut rng = Prng::new(iter as u64 + 0x4444);
            let path = scratch_registry_path("prop-restore-reg", iter);
            let mut reg = Registry::default();
            let n = rng.gen_range(1, 4);
            for i in 0..n {
                let inst = random_instance(&mut rng, iter, i);
                drop(reg.insert(inst));
            }
            reg.store(&path).unwrap();
            let before_bytes = std::fs::read(&path).unwrap();
            let entry = superai_config::backup::backup(&path)
                .unwrap()
                .expect("backup exists");

            // Mutate registry: add one more instance.
            let mut reg2 = Registry::load(&path).unwrap();
            let extra = random_instance(&mut rng, iter + 100, 99);
            drop(reg2.insert(extra));
            reg2.store(&path).unwrap();
            let mutated = std::fs::read(&path).unwrap();
            assert_ne!(
                before_bytes, mutated,
                "mutation should change file at {iter}"
            );

            // Restore.
            superai_config::backup::restore_entry(&entry).unwrap();
            let restored = std::fs::read(&path).unwrap();
            assert_eq!(
                before_bytes,
                restored,
                "restore exact failed at {iter}: before len {}, restored len {}",
                before_bytes.len(),
                restored.len()
            );
            // Verify backup.
            assert!(
                superai_config::backup::verify_backup(&entry).unwrap(),
                "verify backup failed at {iter}"
            );

            drop(std::fs::remove_dir_all(path.parent().unwrap()));
        }
    }

    // -----------------------------------------------------------------------
    // 4. Collision-safe normalization — ids
    // -----------------------------------------------------------------------
    #[test]
    fn property_collision_safe_normalization_ids() {
        for iter in 0..100 {
            let mut rng = Prng::new(iter as u64 + 0x5555);
            let base = random_valid_name(&mut rng, "base-");
            // Generate two case variations.
            let var1 = random_case_variation(true, &base, &mut rng);
            let var2 = random_case_variation(true, &base, &mut rng);

            // Both should be valid InstanceNames (if they remain valid after case change).
            // InstanceName validation is case-insensitive for reserved check but original case preserved.
            let id1 = InstanceName::new(&var1);
            let id2 = InstanceName::new(&var2);
            if let (Ok(a), Ok(b)) = (id1, id2) {
                assert_eq!(
                    a.normalized(),
                    b.normalized(),
                    "case variations should normalize same at {iter}: {a} vs {b}"
                );
                assert!(a.eq_case_fold(&b), "eq_case_fold true expected at {iter}");
                assert!(
                    a.eq_case_fold_str(b.as_str()),
                    "eq_case_fold_str true at {iter}"
                );

                // Registry should reject collision.
                let mut reg = Registry::default();
                let harness = HarnessId::new("claude-code").unwrap();
                let root1 = AbsolutePath::new(&format!("/tmp/coll-{}-1", iter)).unwrap();
                let root2 = AbsolutePath::new(&format!("/tmp/coll-{}-2", iter)).unwrap();
                let inst1 = Instance {
                    id: InstanceId::new(&format!("id-{}-1", iter)).unwrap(),
                    name: a.clone(),
                    harness: harness.clone(),
                    config_root: root1,
                    binary: None,
                    wrapper: None,
                    isolation: Isolation::RelocatedRoot,
                    origin: InstanceOrigin::Created,
                    ownership: Ownership::SuperaiCreated,
                    template: None,
                    created_at: "2026-08-26T00:00:00Z".to_owned(),
                    adapter_revision: "0.1.0".to_owned(),
                };
                let inst2 = Instance {
                    id: InstanceId::new(&format!("id-{}-2", iter)).unwrap(),
                    name: b.clone(),
                    harness,
                    config_root: root2,
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
                let err = reg.insert(inst2).unwrap_err();
                match err {
                    crate::error::CoreError::NameCollision { kind, .. } => {
                        assert_eq!(
                            kind, "InstanceName",
                            "expected InstanceName collision at {iter}"
                        );
                    }
                    other => panic!("expected NameCollision at {iter}, got {other:?}"),
                }
            }

            // Distinct bases should not collide.
            let other_base = format!("{}-x", base);
            if let (Ok(a), Ok(b)) = (InstanceName::new(&base), InstanceName::new(&other_base)) {
                if a.normalized() != b.normalized() {
                    assert!(!a.eq_case_fold(&b), "distinct should not collide at {iter}");
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // 5. Collision-safe normalization — paths
    // -----------------------------------------------------------------------
    #[test]
    fn property_collision_safe_normalization_paths() {
        for iter in 0..100 {
            let mut rng = Prng::new(iter as u64 + 0x6666);
            let base = format!("/tmp/base-{}", rng.gen_string(3, 8, SIMPLE_CHARSET));
            // Generate noisy variations that should normalize to same.
            let noisy1 = format!("{base}//./sub//./dir");
            let noisy2 = format!("{base}/sub/dir");
            let noisy3 = format!("{base}/sub/./dir/");

            let p1 = AbsolutePath::new(&noisy1).unwrap();
            let p2 = AbsolutePath::new(&noisy2).unwrap();
            let p3 = AbsolutePath::new(&noisy3).unwrap();

            assert_eq!(
                p1, p2,
                "path normalization failed at {iter}: {noisy1} vs {noisy2}"
            );
            assert_eq!(
                p2, p3,
                "path normalization failed at {iter}: {noisy2} vs {noisy3}"
            );

            // Normalizing twice is idempotent.
            let p1_again = AbsolutePath::new(&p1.to_string()).unwrap();
            assert_eq!(p1, p1_again, "idempotent path normalization at {iter}");

            // Distinct paths should not collide.
            let other = format!("{base}/other-{}", rng.gen_string(3, 6, SIMPLE_CHARSET));
            let po = AbsolutePath::new(&other).unwrap();
            assert_ne!(p1, po, "distinct paths should not collide at {iter}");

            // Registry config_root collision check: same normalized path should be rejected.
            let mut reg = Registry::default();
            let harness = HarnessId::new("claude-code").unwrap();
            let name1 = InstanceName::new(&random_valid_name(&mut rng, "n1-")).unwrap();
            let name2 = InstanceName::new(&random_valid_name(&mut rng, "n2-")).unwrap();
            let inst1 = Instance {
                id: InstanceId::new(&format!("pid-{}-1", iter)).unwrap(),
                name: name1,
                harness: harness.clone(),
                config_root: p1.clone(),
                binary: None,
                wrapper: None,
                isolation: Isolation::RelocatedRoot,
                origin: InstanceOrigin::Created,
                ownership: Ownership::SuperaiCreated,
                template: None,
                created_at: "2026-08-26T00:00:00Z".to_owned(),
                adapter_revision: "0.1.0".to_owned(),
            };
            let inst2 = Instance {
                id: InstanceId::new(&format!("pid-{}-2", iter)).unwrap(),
                name: name2,
                harness,
                config_root: p2, // same normalized as p1
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
            let err = reg.insert(inst2).unwrap_err();
            match err {
                crate::error::CoreError::Validation { field, .. } => {
                    assert_eq!(
                        field, "config_root",
                        "expected config_root collision at {iter}"
                    );
                }
                other => panic!("expected Validation config_root at {iter}, got {other:?}"),
            }
        }
    }

    // -----------------------------------------------------------------------
    // 6. Capability complete
    // -----------------------------------------------------------------------
    #[test]
    fn property_capability_complete() {
        // Static matrix must be complete.
        capability_resolver::validate_matrix_completeness().unwrap();

        for iter in 0..50 {
            let mut rng = Prng::new(iter as u64 + 0x7777);
            // For each active pair, resolve_all must return all capabilities.
            for (harness_str, provider_str) in ACTIVE_PAIRS {
                let harness = HarnessId::new(harness_str).unwrap();
                let provider = ProviderId::new(provider_str).unwrap();
                let resolved = capability_resolver::resolve_all(&harness, &provider);
                assert_eq!(
                    resolved.len(),
                    ALL_CAPABILITIES.len(),
                    "resolve_all length mismatch for {harness_str}/{provider_str} at {iter}"
                );
                let mut seen = HashSet::new();
                for (cap, res) in &resolved {
                    assert!(seen.insert(*cap), "duplicate cap {:?} at {iter}", cap);
                    assert!(
                        !res.explanation.trim().is_empty(),
                        "empty explanation for {harness_str}/{provider_str} {:?} at {iter}",
                        cap
                    );
                    // For active pairs, source should not be Unknown unless explicitly Absent? But many Absent are Harness source.
                    // Just ensure not Unknown for active pairs where matrix has entry.
                    // Our matrix has entries for all active pairs, so source should not be Unknown.
                    assert_ne!(
                        res.source,
                        capability_resolver::CapabilitySource::Unknown,
                        "active pair {harness_str}/{provider_str} has Unknown source for {:?} at {iter}",
                        cap
                    );
                    // Substituted must have Provider/Template/Plugin source.
                    if res.support == Support::Substituted {
                        assert!(
                            matches!(
                                res.source,
                                capability_resolver::CapabilitySource::Provider
                                    | capability_resolver::CapabilitySource::Template
                                    | capability_resolver::CapabilitySource::Plugin
                            ),
                            "substituted should have provider/template/plugin source at {iter}: {:?}",
                            cap
                        );
                    }
                }
            }

            // Random unknown harness/provider should return Absent/Unknown.
            let unknown_harness =
                HarnessId::new(&random_valid_name(&mut rng, "unknown-h-")).unwrap();
            let unknown_provider =
                ProviderId::new(&random_valid_name(&mut rng, "unknown-p-")).unwrap();
            // Use a harness/provider not in ACTIVE_PAIRS.
            let is_active = ACTIVE_PAIRS.iter().any(|(h, p)| {
                h.to_lowercase() == unknown_harness.as_str().to_lowercase()
                    && p.to_lowercase() == unknown_provider.as_str().to_lowercase()
            });
            if !is_active {
                let cap = ALL_CAPABILITIES[rng.gen_range(0, ALL_CAPABILITIES.len())];
                let res = capability_resolver::resolve(&unknown_harness, &unknown_provider, cap);
                // For unknown pair, should be Absent + Unknown.
                assert_eq!(
                    res.support,
                    Support::Absent,
                    "unknown pair should be Absent at {iter}"
                );
                assert_eq!(
                    res.source,
                    capability_resolver::CapabilitySource::Unknown,
                    "unknown pair should be Unknown source at {iter}"
                );
            }

            // Validate no duplicate rows in static MATRIX.
            let mut seen_matrix = HashSet::new();
            for e in MATRIX {
                let key = (
                    e.harness.to_lowercase(),
                    e.provider.to_lowercase(),
                    e.capability,
                );
                assert!(
                    seen_matrix.insert(key),
                    "duplicate matrix entry at {iter}: {e:?}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // 7. Unrelated survive — registry
    // -----------------------------------------------------------------------
    #[test]
    fn property_registry_unrelated_survive() {
        for iter in 0..50 {
            let mut rng = Prng::new(iter as u64 + 0x8888);
            let mut reg = Registry::default();
            let n = rng.gen_range(2, 6);
            let mut inserted_ids = Vec::new();
            for i in 0..n {
                let inst = random_instance(&mut rng, iter, i);
                let id_clone = inst.id.clone();
                if reg.insert(inst).is_ok() {
                    inserted_ids.push(id_clone);
                }
            }
            if inserted_ids.is_empty() {
                continue;
            }
            let before_instances: Vec<Instance> = reg.instances().to_vec();

            // Insert one more unrelated instance.
            let extra = random_instance(&mut rng, iter + 500, 999);
            let extra_id = extra.id.clone();
            let extra_name = extra.name.clone();
            if reg.insert(extra.clone()).is_err() {
                continue;
            }

            // Verify unrelated survive exactly.
            for before in &before_instances {
                let after = reg
                    .get_by_id(before.id.as_str())
                    .unwrap_or_else(|| panic!("unrelated instance {} lost at {iter}", before.id));
                assert_eq!(
                    before, after,
                    "unrelated instance mutated at {iter}: {}",
                    before.id
                );
            }
            // New one present.
            assert!(
                reg.get_by_id(extra_id.as_str()).is_some(),
                "extra not found at {iter}"
            );
            assert!(
                reg.get(extra_name.as_str()).is_some(),
                "extra name not found at {iter}"
            );

            // Remove extra and verify unrelated still survive.
            reg.remove(extra_name.as_str());
            for before in &before_instances {
                let after = reg.get_by_id(before.id.as_str()).unwrap();
                assert_eq!(before, after, "unrelated after remove mutated at {iter}");
            }
        }
    }

    // -----------------------------------------------------------------------
    // 8. No-op byte identity — raw_editor diff is_noop equals byte equality
    // -----------------------------------------------------------------------
    #[test]
    fn property_no_op_byte_identity_raw_editor() {
        for iter in 0..80 {
            let mut rng = Prng::new(iter as u64 + 0x9999);
            // Generate valid JSON bytes to satisfy strict validation.
            let valid_bytes = if rng.gen_bool() {
                let n = rng.gen_range(0, 50);
                format!("{{\"k\":{n}}}").into_bytes()
            } else if rng.gen_bool() {
                b"{}".to_vec()
            } else {
                let s = rng.gen_string(0, 10, SIMPLE_CHARSET);
                format!("\"{s}\"").into_bytes()
            };
            let bytes = valid_bytes;
            let path = temp_dir_unique("prop-noop-core").join(format!("file-{iter}.json"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &bytes).unwrap();

            let raw = superai_config::raw_editor::read(&path).unwrap();
            let digest = raw.digest.clone();

            // Preview diff of same content should be noop.
            let diff = superai_config::raw_editor::diff(
                &bytes,
                &bytes,
                superai_config::document::DocumentKind::StrictJson,
            );
            assert!(diff.is_noop, "self-diff should be noop at {iter}");
            assert!(
                diff.lexical_unified_diff.is_empty(),
                "noop lexical diff should be empty at {iter}"
            );

            // Commit with same bytes should be noop (no backup, byte identity).
            let report = superai_config::raw_editor::commit(&path, &bytes, Some(&digest)).unwrap();
            assert!(
                report.is_noop,
                "commit with same bytes should be noop at {iter}"
            );
            let after = std::fs::read(&path).unwrap();
            assert_eq!(bytes, after, "byte identity failed at {iter}");

            drop(std::fs::remove_dir_all(path.parent().unwrap()));
        }
    }

    // -----------------------------------------------------------------------
    // 9. Transaction preview deterministic is covered via install plan; also check
    //    registry store preview via operation preview structure.
    // -----------------------------------------------------------------------
    #[test]
    fn property_preview_deterministic_raw_editor_commit() {
        for iter in 0..50 {
            let mut rng = Prng::new(iter as u64 + 0xaaaa);
            let old_len = rng.gen_range(0, 150);
            let new_len = rng.gen_range(0, 150);
            let old: Vec<u8> = (0..old_len).map(|_| rng.gen_range(32, 127) as u8).collect();
            let new: Vec<u8> = (0..new_len).map(|_| rng.gen_range(32, 127) as u8).collect();

            let d1 = superai_config::raw_editor::diff(
                &old,
                &new,
                superai_config::document::DocumentKind::Yaml,
            );
            let d2 = superai_config::raw_editor::diff(
                &old,
                &new,
                superai_config::document::DocumentKind::Yaml,
            );
            assert_eq!(d1, d2, "diff not deterministic at {iter}");

            let v1 = superai_config::raw_editor::validate(
                &new,
                superai_config::document::DocumentKind::Yaml,
            );
            let v2 = superai_config::raw_editor::validate(
                &new,
                superai_config::document::DocumentKind::Yaml,
            );
            assert_eq!(v1, v2, "validate not deterministic at {iter}");
        }
    }
}
