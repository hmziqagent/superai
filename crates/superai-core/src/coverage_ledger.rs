//! QAL-13/QAL-14: goal-sentence and DoD ledgers as TESTED artifacts.
//!
//! This module machine-checks the two planning ledgers that were previously
//! only prose:
//!
//! - every `docs/goal.md` requirement row of master-plan §9 maps to at least
//!   one existing test (or an explicit unsupported-state citation), and
//! - every master-plan §10 non-UI DoD checkbox maps the same way.
//!
//! The check is bidirectional against the plan document itself (embedded at
//! compile time): if a §9 row is added or reworded, or a §10 checkbox is
//! added or removed, the corresponding test here fails until the ledger is
//! updated. Every cited test name is verified to exist in the cited source
//! file, so renamed or deleted tests break the build, not just the docs.
//!
//! QAL-14 freshness lives here too: [`staleness_days`] gives the pre-release
//! recheck workflow its age computation, and the freshness test enforces the
//! catalog's `last_verified` discipline against a recorded recheck date.

use std::path::PathBuf;

/// Reference date for the freshness ledger (QAL-14). The pre-release recheck
/// workflow updates this constant after re-verifying the catalog against the
/// research docs; entries older than [`MAX_ENTRY_AGE_DAYS`] as of this date
/// fail the freshness test.
pub const FRESHNESS_AS_OF: &str = "2026-09-01";

/// Maximum tolerated age of a catalog `last_verified` date, in days, at the
/// last recorded recheck.
pub const MAX_ENTRY_AGE_DAYS: i64 = 365;

// ---------------------------------------------------------------------------
// Evidence model
// ---------------------------------------------------------------------------

/// A piece of evidence backing one ledger row.
#[derive(Debug, Clone, Copy)]
pub enum Evidence {
    /// A test function: `name` must appear as `fn <name>(` in `file`.
    Test {
        /// Source file, relative to this crate's manifest directory.
        file: &'static str,
        /// The test function name.
        name: &'static str,
    },
    /// A non-test artifact: `file` must exist and contain `needle`
    /// (e.g. a CI gate step, a committed tool configuration).
    Contains {
        /// Artifact file, relative to this crate's manifest directory.
        file: &'static str,
        /// Text the artifact must contain.
        needle: &'static str,
    },
}

/// Resolve an evidence `file` (relative to this crate's manifest dir).
fn resolve(file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(file)
}

/// Verify one evidence item; returns a human-readable problem on failure.
pub fn verify_evidence(evidence: &Evidence) -> Result<(), String> {
    match evidence {
        Evidence::Test { file, name } => {
            let path = resolve(file);
            let src =
                std::fs::read_to_string(&path).map_err(|e| format!("evidence file {file}: {e}"))?;
            if src.contains(&format!("fn {name}(")) {
                Ok(())
            } else {
                Err(format!("test `{name}` not found in {file}"))
            }
        }
        Evidence::Contains { file, needle } => {
            let path = resolve(file);
            let src =
                std::fs::read_to_string(&path).map_err(|e| format!("evidence file {file}: {e}"))?;
            if src.contains(needle) {
                Ok(())
            } else {
                Err(format!("{file} does not contain `{needle}`"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// §9 ledger: goal.md requirement rows → owning tests
// ---------------------------------------------------------------------------

/// One master-plan §9 row: the goal requirement sentence (must match the
/// plan's table text exactly), plus its evidence.
#[derive(Debug)]
pub struct GoalRow {
    /// The requirement sentence, first column of the §9 table.
    pub requirement: &'static str,
    /// Evidence: tests or explicit artifacts.
    pub evidence: &'static [Evidence],
}

/// The full §9 ledger. The bidirectional test guarantees this list covers
/// exactly the plan's rows — no orphans, no missing.
pub const GOAL_ROWS: &[GoalRow] = &[
    GoalRow {
        requirement: "Existing/default installs are managed targets",
        evidence: &[
            Evidence::Test {
                file: "src/discovery.rs",
                name: "foreign_managed_blocks_adoption",
            },
            Evidence::Test {
                file: "src/activation.rs",
                name: "activation_swaps_content_with_backup_and_restores_prior_profile",
            },
        ],
    },
    GoalRow {
        requirement: "Instance records store only superai-owned facts",
        evidence: &[Evidence::Test {
            file: "src/property_tests.rs",
            name: "property_registry_no_forbidden_fields",
        }],
    },
    GoalRow {
        requirement: "Template version per instance",
        evidence: &[Evidence::Test {
            file: "src/lifecycle.rs",
            name: "repair_detects_and_heals_template_version_drift",
        }],
    },
    GoalRow {
        requirement: "Drift scan, adopt/remove, foreign-manager coexistence",
        evidence: &[Evidence::Test {
            file: "src/discovery.rs",
            name: "drift_report_groups_by_harness_with_risk_and_next_ops",
        }],
    },
    GoalRow {
        requirement: "Mirror existing then isolate",
        evidence: &[Evidence::Test {
            file: "src/lifecycle.rs",
            name: "mirror_source_to_target_isolation_proof",
        }],
    },
    GoalRow {
        requirement: "Remote versioned templates; diff; manual updates",
        evidence: &[
            Evidence::Test {
                file: "src/failure.rs",
                name: "template_fetch_digest_mismatch_fails_and_preserves",
            },
            Evidence::Test {
                file: "src/template_update.rs",
                name: "preview_both_differ_conflict",
            },
        ],
    },
    GoalRow {
        requirement: "Native/substituted/absent capability matrix",
        evidence: &[Evidence::Test {
            file: "src/capability_resolver.rs",
            name: "matrix_has_native_substituted_absent_with_explanations",
        }],
    },
    GoalRow {
        requirement: "Per-harness version-aware config",
        evidence: &[Evidence::Test {
            file: "src/adapters/claude_code.rs",
            name: "boundary_fixtures_split_settings_eras",
        }],
    },
    GoalRow {
        requirement: "Fresh disk read; preserve unmodelled keys",
        evidence: &[
            Evidence::Test {
                file: "../superai-config/src/json.rs",
                name: "edit_preserves_unmodelled_keys_and_their_order",
            },
            Evidence::Test {
                file: "../superai-config/src/raw_editor.rs",
                name: "unknown_key_survives_commit",
            },
        ],
    },
    GoalRow {
        requirement: "Backup before every foreign-file write",
        evidence: &[
            Evidence::Test {
                file: "../superai-config/src/json.rs",
                name: "writing_leaves_a_backup_of_the_previous_contents",
            },
            Evidence::Test {
                file: "../superai-config/src/property_tests.rs",
                name: "mutant_backup_before_write_is_not_skippable",
            },
        ],
    },
    GoalRow {
        requirement: "Wrapper generation; arbitrary user names",
        evidence: &[
            Evidence::Test {
                file: "src/wrapper.rs",
                name: "generates_deterministic_sh_wrapper",
            },
            Evidence::Test {
                file: "src/wrapper.rs",
                name: "wrapper_special_chars_quoted_and_verified",
            },
        ],
    },
    GoalRow {
        requirement: "Every researched harness, including awkward ones",
        evidence: &[Evidence::Test {
            file: "src/verification.rs",
            name: "ledger_coverage_has_known_harnesses",
        }],
    },
    GoalRow {
        requirement: "Provider endpoints/models/health/API keys, data-driven",
        evidence: &[Evidence::Test {
            file: "src/provider.rs",
            name: "data_only_adding_provider_requires_no_code_change",
        }],
    },
    GoalRow {
        requirement: "Skill registry; whole/specific symlink or copy",
        evidence: &[
            Evidence::Test {
                file: "src/skills.rs",
                name: "link_all",
            },
            Evidence::Test {
                file: "src/skills.rs",
                name: "copy_selected",
            },
        ],
    },
    GoalRow {
        requirement: "Plugins and MCP servers from instance definition",
        evidence: &[
            Evidence::Test {
                file: "src/mcp.rs",
                name: "foreign_entry_preservation",
            },
            Evidence::Test {
                file: "src/plugin.rs",
                name: "removal_leaves_foreign",
            },
        ],
    },
    GoalRow {
        requirement: "Install/uninstall; existing-install detection",
        evidence: &[
            Evidence::Test {
                file: "src/install_execute.rs",
                name: "uninstall_preflight_lists_instances_and_preserves_config",
            },
            Evidence::Test {
                file: "src/detect.rs",
                name: "detect_does_not_pick_silently_when_multiple",
            },
        ],
    },
    GoalRow {
        requirement: "Raw TOML/JSON editors with validation",
        evidence: &[Evidence::Test {
            file: "../superai-config/src/raw_editor.rs",
            name: "invalid_json_rejected_without_touching_disk",
        }],
    },
    GoalRow {
        requirement: "No vault and no OAuth",
        evidence: &[
            Evidence::Test {
                file: "src/abuse.rs",
                name: "errors_and_debug_never_contain_sentinel_plain",
            },
            Evidence::Test {
                file: "src/coverage_ledger.rs",
                name: "no_proxy_vault_oauth_or_chat_runtime_tokens_in_sources",
            },
        ],
    },
    GoalRow {
        requirement: "claude-multi coexistence; no import",
        evidence: &[Evidence::Test {
            file: "src/discovery.rs",
            name: "classify_ownership_respects_registry_and_foreign",
        }],
    },
    GoalRow {
        requirement: "Config → Core → Interface dependency direction",
        evidence: &[Evidence::Test {
            file: "src/coverage_ledger.rs",
            name: "dependency_direction_is_config_core_cli",
        }],
    },
    GoalRow {
        requirement: "Filesystem first, interface last",
        evidence: &[Evidence::Test {
            file: "src/coverage_ledger.rs",
            name: "dependency_direction_is_config_core_cli",
        }],
    },
    GoalRow {
        requirement: "No proxy/routing implementation",
        evidence: &[Evidence::Test {
            file: "src/coverage_ledger.rs",
            name: "no_proxy_vault_oauth_or_chat_runtime_tokens_in_sources",
        }],
    },
    GoalRow {
        requirement: "Rust/local/one binary",
        evidence: &[Evidence::Test {
            file: "src/coverage_ledger.rs",
            name: "single_local_binary_and_workspace_layering",
        }],
    },
];

// ---------------------------------------------------------------------------
// §10 ledger: non-UI DoD checkboxes → owning tests
// ---------------------------------------------------------------------------

/// One master-plan §10 checkbox: the 1-based checkbox number plus evidence.
#[derive(Debug)]
pub struct DodItem {
    /// 1-based checkbox position in §10 (order-stable in the document).
    pub number: usize,
    /// Short label for diagnostics.
    pub label: &'static str,
    /// Evidence: tests or explicit artifacts.
    pub evidence: &'static [Evidence],
}

/// The §10 ledger (16 checkboxes).
pub const DOD_ITEMS: &[DodItem] = &[
    DodItem {
        number: 1,
        label: "goal sentence mapping",
        evidence: &[Evidence::Test {
            file: "src/coverage_ledger.rs",
            name: "goal_ledger_covers_every_section9_row",
        }],
    },
    DodItem {
        number: 2,
        label: "48 surfaces adapter support records",
        evidence: &[
            Evidence::Test {
                file: "src/harness_catalog.rs",
                name: "all_adapters_span_catalog",
            },
            Evidence::Test {
                file: "src/verification.rs",
                name: "ledger_coverage_has_known_harnesses",
            },
        ],
    },
    DodItem {
        number: 3,
        label: "writes read fresh/conflict/backup/atomic/verify",
        evidence: &[
            Evidence::Test {
                file: "../superai-config/src/transaction.rs",
                name: "transaction_prepare_and_commit_single_file",
            },
            Evidence::Test {
                file: "src/failure.rs",
                name: "conflict_recheck_injection_aborts_transaction_before_overwrite",
            },
            Evidence::Test {
                file: "../superai-config/src/backup.rs",
                name: "restore_replaces_target_by_rename_never_in_place_truncation",
            },
        ],
    },
    DodItem {
        number: 4,
        label: "failure injection proves rollback + restoration",
        evidence: &[
            Evidence::Test {
                file: "src/failure.rs",
                name: "abandoned_journal_at_each_phase_recovers_via_production_journal",
            },
            Evidence::Test {
                file: "../superai-config/src/transaction.rs",
                name: "commit_failure_surfaces_intermediate_rollback_in_outcome",
            },
        ],
    },
    DodItem {
        number: 5,
        label: "records carry no harness value/secret",
        evidence: &[Evidence::Test {
            file: "src/property_tests.rs",
            name: "property_registry_no_forbidden_fields",
        }],
    },
    DodItem {
        number: 6,
        label: "lifecycle tests for all target classes",
        evidence: &[
            Evidence::Test {
                file: "src/lifecycle.rs",
                name: "adopt_with_wrapper_creates_wrapper_and_preserves_config",
            },
            Evidence::Test {
                file: "src/activation.rs",
                name: "journaled_activation_leaves_no_journal_residue_on_success",
            },
            Evidence::Test {
                file: "src/daemon.rs",
                name: "daemon_start_ready_stop_round_trip",
            },
            Evidence::Test {
                file: "src/daemon.rs",
                name: "allocate_port_yields_portconflict_when_range_exhausted",
            },
            Evidence::Test {
                file: "src/wrapper.rs",
                name: "two_concurrent_ide_profiles_split_state_dirs",
            },
        ],
    },
    DodItem {
        number: 7,
        label: "provider/template additions data-only",
        evidence: &[
            Evidence::Test {
                file: "src/provider.rs",
                name: "data_only_adding_provider_requires_no_code_change",
            },
            Evidence::Test {
                file: "src/capability_resolver.rs",
                name: "file_driven_matrix_data_only",
            },
        ],
    },
    DodItem {
        number: 8,
        label: "template update preview old/new/divergence/conflicts",
        evidence: &[
            Evidence::Test {
                file: "src/template_update.rs",
                name: "preview_clean_update_local_eq_base_applies_new",
            },
            Evidence::Test {
                file: "src/template_update.rs",
                name: "preview_both_differ_conflict",
            },
        ],
    },
    DodItem {
        number: 9,
        label: "skills link-all/link-one/copy-one/update/disable/remove",
        evidence: &[
            Evidence::Test {
                file: "src/skills.rs",
                name: "link_all",
            },
            Evidence::Test {
                file: "src/skills.rs",
                name: "copy_selected",
            },
            Evidence::Test {
                file: "src/skills.rs",
                name: "update_skill",
            },
            Evidence::Test {
                file: "src/skills.rs",
                name: "disable_skill",
            },
            Evidence::Test {
                file: "src/skills.rs",
                name: "remove_skill",
            },
        ],
    },
    DodItem {
        number: 10,
        label: "plugin/MCP adapter-specific + foreign preserved",
        evidence: &[
            Evidence::Test {
                file: "src/adapter.rs",
                name: "mcp_decl_shape_and_read_only_round_trip",
            },
            Evidence::Test {
                file: "src/adapter.rs",
                name: "plugin_decl_directory_bundle_carries_staging_fields",
            },
            Evidence::Test {
                file: "src/mcp.rs",
                name: "foreign_entry_preservation",
            },
            Evidence::Test {
                file: "src/plugin.rs",
                name: "removal_leaves_foreign",
            },
        ],
    },
    DodItem {
        number: 11,
        label: "install/uninstall never removes user data without intent",
        evidence: &[
            Evidence::Test {
                file: "src/install_execute.rs",
                name: "uninstall_blocks_foreign_manual_file_without_explicit",
            },
            Evidence::Test {
                file: "src/install_execute.rs",
                name: "uninstall_preserve_includes_backups_and_templates",
            },
        ],
    },
    DodItem {
        number: 12,
        label: "raw editor rejects invalid without touching disk",
        evidence: &[
            Evidence::Test {
                file: "../superai-config/src/raw_editor.rs",
                name: "invalid_json_rejected_without_touching_disk",
            },
            Evidence::Test {
                file: "../superai-config/src/raw_editor.rs",
                name: "invalid_toml_rejected_without_touching_disk",
            },
        ],
    },
    DodItem {
        number: 13,
        label: "secrets never in records/diagnostics/snapshots/logs",
        evidence: &[
            Evidence::Test {
                file: "src/abuse.rs",
                name: "errors_and_debug_never_contain_sentinel_plain",
            },
            Evidence::Test {
                file: "src/verification.rs",
                name: "fixture_populated_valid_and_secret_free",
            },
        ],
    },
    DodItem {
        number: 14,
        label: "no interface dependency below L3",
        evidence: &[Evidence::Test {
            file: "src/coverage_ledger.rs",
            name: "dependency_direction_is_config_core_cli",
        }],
    },
    DodItem {
        number: 15,
        label: "no proxy/wire/chat/OAuth/vault",
        evidence: &[Evidence::Test {
            file: "src/coverage_ledger.rs",
            name: "no_proxy_vault_oauth_or_chat_runtime_tokens_in_sources",
        }],
    },
    DodItem {
        number: 16,
        label: "fmt/clippy/tests/locked/features/audit/fuzz/mutation/platform pass",
        evidence: &[
            Evidence::Test {
                file: "../superai-config/src/fuzz.rs",
                name: "fuzz_json_load_no_panic_100",
            },
            Evidence::Test {
                file: "../superai-config/src/fuzz.rs",
                name: "fuzz_executor_operations_no_panic_and_no_mutation_on_reject_100",
            },
            Evidence::Test {
                file: "../superai-config/src/fuzz.rs",
                name: "fuzz_span_codec_no_panic_and_invariants_hold_100",
            },
            Evidence::Contains {
                file: "../../.cargo/mutants.toml",
                needle: "examine_globs",
            },
            Evidence::Contains {
                file: "../../.github/workflows/ci.yml",
                needle: "mutation-testing",
            },
            Evidence::Contains {
                file: "../../.github/workflows/ci.yml",
                needle: "quality-windows",
            },
        ],
    },
];

// ---------------------------------------------------------------------------
// §9/§10 document parsing (provenance of the rows themselves)
// ---------------------------------------------------------------------------

/// Extract the §9 requirement sentences (first column of the goal-coverage
/// table), skipping the header and separator rows.
pub fn section9_requirements(plan: &str) -> Vec<String> {
    let mut in_section = false;
    let mut rows = Vec::new();
    for line in plan.lines() {
        if line.starts_with("## ") {
            in_section = line.starts_with("## 9.");
            continue;
        }
        if !in_section || !line.starts_with('|') {
            continue;
        }
        if line.contains("---") || line.starts_with("| Goal requirement") {
            continue;
        }
        let first = line.trim_start_matches('|');
        let col = first
            .split('|')
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if !col.is_empty() {
            rows.push(col);
        }
    }
    rows
}

/// Count the `§10` `DoD` checkboxes and return their first-line texts.
pub fn section10_items(plan: &str) -> Vec<String> {
    let mut in_section = false;
    let mut items = Vec::new();
    for line in plan.lines() {
        if line.starts_with("## ") {
            in_section = line.starts_with("## 10.");
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(text) = line.strip_prefix("- [ ] ") {
            items.push(text.trim().to_owned());
        } else if let Some(text) = line.strip_prefix("- [x] ") {
            items.push(text.trim().to_owned());
        }
    }
    items
}

// ---------------------------------------------------------------------------
// QAL-14: freshness helpers
// ---------------------------------------------------------------------------

/// Parse `YYYY-MM-DD` into `(year, month, day)`.
pub fn parse_ymd(s: &str) -> Option<(i32, u32, u32)> {
    let mut parts = s.trim().split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(if month <= 2 { year - 1 } else { year });
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let m = i64::from(month);
    let d = i64::from(day);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Age in days of a `last_verified` date relative to `as_of` (both
/// `YYYY-MM-DD`). `None` when either date does not parse.
pub fn staleness_days(last_verified: &str, as_of: &str) -> Option<i64> {
    let (ly, lm, ld) = parse_ymd(last_verified)?;
    let (ay, am, ad) = parse_ymd(as_of)?;
    Some(days_from_civil(ay, am, ad) - days_from_civil(ly, lm, ld))
}

// ---------------------------------------------------------------------------
// Tests — the ledger is a tested artifact (QAL-13)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The master plan, embedded so the ledger cannot drift from the document.
    const MASTER_PLAN: &str = include_str!("../../../docs/plans/master-plan.md");

    fn manifest_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    /// The §9 ledger covers EXACTLY the plan's requirement rows — every plan
    /// row has an entry, and no ledger entry is orphaned. Rewording a plan
    /// row or deleting its tests fails here (`DoD` #1).
    #[test]
    fn goal_ledger_covers_every_section9_row() {
        let plan_rows = section9_requirements(MASTER_PLAN);
        assert!(
            plan_rows.len() >= 20,
            "suspiciously few §9 rows parsed: {plan_rows:?}"
        );
        let ledger_rows: Vec<&str> = GOAL_ROWS.iter().map(|r| r.requirement).collect();
        for plan_row in &plan_rows {
            assert!(
                ledger_rows.contains(&plan_row.as_str()),
                "§9 row `{plan_row}` has no ledger entry — map it to a test or an explicit unsupported-state citation"
            );
        }
        for ledger_row in &ledger_rows {
            assert!(
                plan_rows.iter().any(|p| p == ledger_row),
                "ledger row `{ledger_row}` does not match any §9 plan row — fix the ledger text"
            );
        }
        assert_eq!(
            plan_rows.len(),
            ledger_rows.len(),
            "§9 ledger and plan row counts differ"
        );
    }

    /// Every §9 ledger row carries at least one resolvable evidence item.
    #[test]
    fn goal_ledger_evidence_resolves() {
        for row in GOAL_ROWS {
            assert!(
                !row.evidence.is_empty(),
                "§9 row `{}` has no evidence",
                row.requirement
            );
            for ev in row.evidence {
                verify_evidence(ev).unwrap_or_else(|e| panic!("§9 `{}`: {e}", row.requirement));
            }
        }
    }

    /// The §10 ledger covers exactly the plan's 16 checkboxes (by position),
    /// each with resolvable evidence.
    #[test]
    fn dod_ledger_covers_every_section10_item() {
        let items = section10_items(MASTER_PLAN);
        assert_eq!(
            items.len(),
            16,
            "expected 16 DoD checkboxes in §10, found {}: {items:?}",
            items.len()
        );
        assert_eq!(DOD_ITEMS.len(), 16, "ledger must track all 16 DoD items");
        for item in DOD_ITEMS {
            assert!(
                (1..=items.len()).contains(&item.number),
                "DoD ledger item {} out of range",
                item.number
            );
            assert!(
                !item.evidence.is_empty(),
                "DoD item {} ({}) has no evidence",
                item.number,
                item.label
            );
            for ev in item.evidence {
                verify_evidence(ev)
                    .unwrap_or_else(|e| panic!("DoD #{} ({}): {e}", item.number, item.label));
            }
        }
    }

    /// Layering is data-checkable: superai-config depends on nothing in this
    /// workspace, superai-core depends only on superai-config, and only the
    /// L3 CLI depends upward. No interface crate exists below L3 (`DoD` #14,
    /// §9 layering rows).
    #[test]
    fn dependency_direction_is_config_core_cli() {
        let config_toml =
            std::fs::read_to_string(manifest_root().join("../superai-config/Cargo.toml"))
                .expect("config Cargo.toml");
        assert!(
            !config_toml.contains("superai-core") && !config_toml.contains("superai-cli"),
            "L1 superai-config must not depend on higher layers"
        );
        let core_toml =
            std::fs::read_to_string(manifest_root().join("Cargo.toml")).expect("core Cargo.toml");
        assert!(
            core_toml.contains("superai-config") && !core_toml.contains("superai-cli"),
            "L2 superai-core depends only on L1"
        );
        let cli_toml = std::fs::read_to_string(manifest_root().join("../superai-cli/Cargo.toml"))
            .expect("cli Cargo.toml");
        assert!(
            cli_toml.contains("superai-core"),
            "L3 CLI is the only layer that may depend upward"
        );
    }

    /// §9 "Rust/local/one binary": exactly one binary target, in the L3 CLI;
    /// the library crates define none.
    #[test]
    fn single_local_binary_and_workspace_layering() {
        let cli_toml = std::fs::read_to_string(manifest_root().join("../superai-cli/Cargo.toml"))
            .expect("cli Cargo.toml");
        let bin_count = cli_toml.matches("[[bin]]").count();
        assert_eq!(bin_count, 1, "exactly one binary target in the workspace");
        for lib in ["Cargo.toml", "../superai-config/Cargo.toml"] {
            let toml = std::fs::read_to_string(manifest_root().join(lib)).unwrap_or_default();
            assert!(
                !toml.contains("[[bin]]"),
                "{lib} must not define binary targets"
            );
        }
    }

    /// `DoD` #15 / `§9` no-proxy-no-vault rows: the forbidden runtime concepts
    /// appear in no crate source. Tokens are assembled from halves at runtime
    /// so this guard file never contains the verbatim strings it scans for.
    #[test]
    fn no_proxy_vault_oauth_or_chat_runtime_tokens_in_sources() {
        let forbidden: Vec<String> = [
            ("oauth", "client"),
            ("OAuth", "Client"),
            ("secret", "vault"),
            ("Secret", "Vault"),
            ("vault", "client"),
            ("wire", "translator"),
            ("chat", "runtime"),
            ("proxy", "server"),
        ]
        .iter()
        .map(|(a, b)| format!("{a}{b}"))
        .collect();
        let roots = [
            manifest_root().join("src"),
            manifest_root().join("../superai-config/src"),
        ];
        for root in &roots {
            scan_for_tokens(root, &forbidden);
        }
    }

    fn scan_for_tokens(dir: &Path, tokens: &[String]) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                scan_for_tokens(&path, tokens);
            } else {
                assert_file_free_of_tokens(&path, tokens);
            }
        }
    }

    fn assert_file_free_of_tokens(path: &Path, tokens: &[String]) {
        if path.extension().is_none_or(|e| e != "rs") {
            return;
        }
        let src = std::fs::read_to_string(path).unwrap_or_default();
        for token in tokens {
            assert!(
                !src.contains(token.as_str()),
                "{} mentions forbidden concept `{token}`",
                path.display()
            );
        }
    }

    // ---- QAL-14: freshness ----

    #[test]
    fn ymd_parsing_and_civil_days() {
        assert_eq!(parse_ymd("2026-08-25"), Some((2026, 8, 25)));
        assert_eq!(parse_ymd("2026-13-01"), None);
        assert_eq!(parse_ymd("2026-00-10"), None);
        assert_eq!(parse_ymd("not-a-date"), None);
        assert_eq!(parse_ymd("2026-08"), None);
        // Epoch and known offsets.
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(
            days_from_civil(2026, 1, 1),
            days_from_civil(1970, 1, 1) + 20_454
        );
    }

    #[test]
    fn staleness_days_computes_age() {
        assert_eq!(staleness_days("2026-08-25", "2026-09-01"), Some(7));
        assert_eq!(staleness_days("2026-09-01", "2026-09-01"), Some(0));
        assert_eq!(staleness_days("2027-09-01", "2026-09-01"), Some(-365));
        assert_eq!(staleness_days("bogus", "2026-09-01"), None);
    }

    /// QAL-13 ledger completeness in the research direction: every harness
    /// research document under `docs/harness-configs/` (the README ledger
    /// itself excepted) must be claimed by at least one catalog entry's
    /// `research_doc` — a new research file without a ledger entry fails
    /// here instead of silently shipping unresearched.
    #[test]
    fn research_files_have_ledger_entries() {
        let docs = manifest_root().join("../../docs/harness-configs");
        let entries = std::fs::read_dir(&docs).unwrap_or_else(|e| panic!("read {docs:?}: {e}"));
        let mut referenced = 0usize;
        let mut orphans = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if Path::new(&name).extension().is_none_or(|e| e != "md") || name == "README.md" {
                continue;
            }
            let rel = format!("docs/harness-configs/{name}");
            if crate::harness_catalog::ENTRIES
                .iter()
                .any(|e| e.research_doc == rel)
            {
                referenced += 1;
            } else {
                orphans.push(rel);
            }
        }
        assert!(
            referenced >= 44,
            "suspiciously few research docs matched: {referenced}"
        );
        assert!(
            orphans.is_empty(),
            "research docs without a catalog ledger entry: {orphans:?}"
        );
    }

    /// QAL-14 freshness gate: every catalog entry carries a parseable
    /// `last_verified` that is neither in the future nor older than
    /// [`MAX_ENTRY_AGE_DAYS`] as of the recorded recheck date, and its
    /// research document exists. The pre-release recheck workflow updates
    /// [`FRESHNESS_AS_OF`] and re-runs this test.
    #[test]
    fn catalog_freshness_within_policy_window() {
        for entry in crate::harness_catalog::ENTRIES {
            let age = staleness_days(entry.last_verified, FRESHNESS_AS_OF).unwrap_or_else(|| {
                panic!("{}: bad last_verified `{}`", entry.id, entry.last_verified)
            });
            assert!(
                age >= 0,
                "{}: last_verified {} is in the future relative to {FRESHNESS_AS_OF}",
                entry.id,
                entry.last_verified
            );
            assert!(
                age <= MAX_ENTRY_AGE_DAYS,
                "{}: last_verified {} is {age} days stale (> {MAX_ENTRY_AGE_DAYS}); re-verify against {}",
                entry.id,
                entry.last_verified,
                entry.research_doc
            );
            let doc = manifest_root().join("../../").join(entry.research_doc);
            assert!(
                doc.exists(),
                "{}: research doc {} missing",
                entry.id,
                entry.research_doc
            );
        }
    }
}
