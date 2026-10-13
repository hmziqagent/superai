//! Usage provider registry tests (USG-07): the bundled overlap-table seed,
//! documentation links that resolve, and the data-only onboarding proof
//! mirroring subplan 07's `data_only_adding_provider_requires_no_code_change`.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use superai_usage::registry::{
        USAGE_PROVIDER_SCHEMA_VERSION, UsageProviderEntry, UsageRegistry,
    };

    // Lib deps that ride along as externs of this target without being
    // referenced directly.
    use jiff as _;
    use rusqlite as _;
    use serde as _;
    use superai_core as _;
    use thiserror as _;
    use ureq as _;

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/usage-providers")
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    // Subplan 14's overlap table; the bundled seed must carry every one,
    // while additions to the data file stay data-only.
    const OVERLAP_TABLE_PROVIDERS: &[&str] = &[
        "amp",
        "antigravity",
        "claude",
        "cline-pass",
        "codex",
        "command-code",
        "copilot",
        "cursor",
        "deepseek",
        "devin",
        "factory",
        "gemini",
        "grok",
        "kilo",
        "kimi",
        "kiro",
        "mimo",
        "minimax",
        "mistral",
        "muse",
        "opencode",
        "opencode-go",
        "pi",
        "qoder",
        "qwen-cloud",
        "warp",
        "windsurf",
        "workbuddy",
        "zed",
    ];

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-usage-registry-{tag}-{}-{nanos}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp dir {}: {e}", dir.display()));
        dir
    }

    fn copy_fixture_into(fixture: &str, dir: &Path) {
        let from = fixtures_dir().join(fixture);
        let name = from
            .file_name()
            .unwrap_or_else(|| panic!("fixture `{fixture}` has no file name"));
        fs::copy(&from, dir.join(name)).unwrap_or_else(|e| panic!("copy fixture `{fixture}`: {e}"));
    }

    fn slugify(heading: &str) -> String {
        heading
            .to_lowercase()
            .chars()
            .map(|c| if c.is_whitespace() { '-' } else { c })
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    }

    fn heading_anchors(path: &Path) -> Vec<String> {
        let text =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        text.lines()
            .filter_map(|line| {
                let trimmed = line.trim_start();
                let heading = trimmed.trim_start_matches('#');
                if heading.len() == trimmed.len() {
                    return None;
                }
                Some(slugify(heading.strip_prefix(' ').unwrap_or(heading)))
            })
            .collect()
    }

    fn assert_documentation_links_resolve<'a>(
        entries: impl Iterator<Item = &'a UsageProviderEntry>,
    ) {
        for row in entries {
            let link = row
                .documentation_url
                .as_deref()
                .unwrap_or_else(|| panic!("entry `{}` carries no documentation link", row.id));
            let (file, fragment) = match link.split_once('#') {
                Some((file, fragment)) => (file, Some(fragment)),
                None => (link, None),
            };
            let path = repo_root().join(file);
            assert!(
                path.is_file(),
                "entry `{}` links `{file}`, which is not a file in this repo",
                row.id
            );
            if let Some(fragment) = fragment {
                let anchors = heading_anchors(&path);
                assert!(
                    anchors.iter().any(|anchor| anchor == fragment),
                    "entry `{}` links `#{fragment}`, which is no heading in `{file}` (anchors: {anchors:?})",
                    row.id
                );
            }
        }
    }

    #[test]
    fn bundled_covers_the_overlap_table_providers() {
        let registry =
            UsageRegistry::bundled().unwrap_or_else(|e| panic!("bundled seed invalid: {e:?}"));
        let ids: Vec<&str> = registry.entries().map(|e| e.id.as_str()).collect();
        for expected in OVERLAP_TABLE_PROVIDERS {
            assert!(
                ids.contains(expected),
                "overlap-table provider `{expected}` missing from the bundled seed: {ids:?}"
            );
        }
    }

    #[test]
    fn bundled_entries_validate_and_link_real_documentation() {
        let registry =
            UsageRegistry::bundled().unwrap_or_else(|e| panic!("bundled seed invalid: {e:?}"));
        registry
            .validate()
            .unwrap_or_else(|e| panic!("bundled entry invalid: {e:?}"));
        for row in registry.entries() {
            assert_eq!(row.schema_version, USAGE_PROVIDER_SCHEMA_VERSION);
        }
        assert_documentation_links_resolve(registry.entries());
    }

    #[test]
    fn bundled_entries_survive_a_serde_round_trip() {
        let registry =
            UsageRegistry::bundled().unwrap_or_else(|e| panic!("bundled seed invalid: {e:?}"));
        for row in registry.entries() {
            let text = serde_json::to_string(row)
                .unwrap_or_else(|e| panic!("serialize `{}`: {e}", row.id));
            let back: UsageProviderEntry = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("deserialize `{}`: {e}", row.id));
            assert_eq!(back, *row, "`{}` changed shape through JSON", row.id);
        }
    }

    #[test]
    fn bundled_seed_is_the_checked_in_data_file() {
        let asset = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/usage-providers.json");
        let dir = temp_dir("bundled-data");
        fs::copy(&asset, dir.join("bundled.json"))
            .unwrap_or_else(|e| panic!("copy {}: {e}", asset.display()));
        let from_data = UsageRegistry::load_dir(&dir)
            .unwrap_or_else(|e| panic!("bundled data file rejected: {e:?}"));
        drop(fs::remove_dir_all(&dir));
        let bundled =
            UsageRegistry::bundled().unwrap_or_else(|e| panic!("bundled seed invalid: {e:?}"));
        assert_eq!(
            from_data.entries().collect::<Vec<_>>(),
            bundled.entries().collect::<Vec<_>>(),
            "bundled() must be exactly what the checked-in data file loads through load_dir"
        );
    }

    #[test]
    fn data_only_adding_provider_requires_no_code_change() {
        let dir = temp_dir("data-only");
        for fixture in [
            "synthetic-api-42.json",
            "synthetic-local-7.json",
            "llmproxy.json",
        ] {
            copy_fixture_into(fixture, &dir);
        }
        let first =
            UsageRegistry::load_dir(&dir).unwrap_or_else(|e| panic!("fixture dir rejected: {e:?}"));
        assert_eq!(first.len(), 3);
        assert_documentation_links_resolve(first.entries());

        let new_entry = r#"{
  "id": "synthetic-new-99",
  "label": "Synthetic New",
  "locators": [{ "kind": "endpoint", "url": "https://usage.new-99.example.com/v1/usage" }],
  "auth_material": [{ "kind": "env_var", "name": "SYNTHETIC_NEW_KEY" }],
  "window_strategy": "endpoint",
  "pricing": "models_dev"
}"#;
        fs::write(dir.join("synthetic-new-99.json"), new_entry)
            .unwrap_or_else(|e| panic!("write new provider: {e}"));
        let second = UsageRegistry::load_dir(&dir)
            .unwrap_or_else(|e| panic!("reload with new provider rejected: {e:?}"));
        assert_eq!(second.len(), 4);
        let added = second
            .entries()
            .find(|e| e.id.as_str() == "synthetic-new-99")
            .unwrap_or_else(|| panic!("data-added provider missing from registry"));
        assert_eq!(added.label, "Synthetic New");
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn load_dir_rejects_duplicate_provider_ids_across_files() {
        let dir = temp_dir("duplicate");
        copy_fixture_into("synthetic-api-42.json", &dir);
        let same_id = r#"{
  "id": "synthetic-api-42",
  "label": "Synthetic API Again",
  "window_strategy": "endpoint",
  "pricing": "unverified"
}"#;
        fs::write(dir.join("duplicate.json"), same_id)
            .unwrap_or_else(|e| panic!("write duplicate: {e}"));
        let err = UsageRegistry::load_dir(&dir)
            .err()
            .unwrap_or_else(|| panic!("duplicate provider id accepted"));
        assert_eq!(
            err.to_string(),
            "duplicate usage provider entry `synthetic-api-42`"
        );
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn load_dir_rejects_a_plain_http_public_endpoint() {
        let dir = temp_dir("public-http");
        let insecure = r#"{
  "id": "insecure-endpoint",
  "label": "Insecure Endpoint",
  "locators": [{ "kind": "endpoint", "url": "http://usage.insecure.example.com/v1/usage" }],
  "window_strategy": "endpoint",
  "pricing": "unverified"
}"#;
        fs::write(dir.join("insecure-endpoint.json"), insecure)
            .unwrap_or_else(|e| panic!("write insecure provider: {e}"));
        let msg = UsageRegistry::load_dir(&dir)
            .err()
            .unwrap_or_else(|| panic!("plain-http public endpoint accepted"))
            .to_string();
        assert!(msg.contains("loopback"), "got: {msg}");
        drop(fs::remove_dir_all(&dir));
    }

    #[test]
    fn load_dir_ignores_non_json_files_but_reads_arrays() {
        let dir = temp_dir("array");
        fs::write(dir.join("notes.txt"), "not a provider")
            .unwrap_or_else(|e| panic!("write notes: {e}"));
        let pair = r#"[
  { "id": "array-a", "label": "Array A", "window_strategy": "cost_only", "pricing": "unverified" },
  { "id": "array-b", "label": "Array B", "window_strategy": "cost_only", "pricing": "unverified" }
]"#;
        fs::write(dir.join("pair.json"), pair).unwrap_or_else(|e| panic!("write pair: {e}"));
        let registry =
            UsageRegistry::load_dir(&dir).unwrap_or_else(|e| panic!("array file rejected: {e:?}"));
        assert_eq!(registry.len(), 2);
        drop(fs::remove_dir_all(&dir));
    }

    #[cfg(unix)]
    fn make_fifo(path: &Path) -> bool {
        std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn load_dir_opens_regular_data_files_only() {
        let dir = temp_dir("non-regular");
        let entry = r#"{
  "id": "regular-file-provider",
  "label": "Regular File",
  "window_strategy": "cost_only",
  "pricing": "unverified"
}"#;
        fs::write(dir.join("regular.json"), entry)
            .unwrap_or_else(|e| panic!("write regular provider: {e}"));
        fs::create_dir_all(dir.join("nested.json"))
            .unwrap_or_else(|e| panic!("create dir named *.json: {e}"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("regular.json", dir.join("alias.json"))
                .unwrap_or_else(|e| panic!("symlink a data file: {e}"));
            assert!(
                make_fifo(&dir.join("evil.json")),
                "mkfifo(1) must exist to run this test"
            );
        }
        let registry = UsageRegistry::load_dir(&dir)
            .unwrap_or_else(|e| panic!("non-regular entries broke the load: {e:?}"));
        assert_eq!(registry.len(), 1);
        assert!(
            registry
                .entries()
                .all(|row| row.id.as_str() == "regular-file-provider")
        );
        drop(fs::remove_dir_all(&dir));
    }
}
