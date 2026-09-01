//! Harness adapters — concrete implementations.

pub mod aider;
pub mod amazon_q;
pub mod amp;
pub mod antigravity;
pub mod auggie;
pub mod claude_code;
pub mod cline;
pub mod codex_cli;
pub mod conductor;
pub mod continue_dev;
pub mod copilot_cli;
pub mod copilot_coding_agent;
pub mod crush;
pub mod cursor;
pub mod deepseek;
pub mod factory_droid;
pub mod forge;
pub mod gemini_cli;
pub mod goose;
pub mod gptme;
pub mod grok_build;
pub mod hermes;
pub mod iflow;
pub mod junie;
pub mod kilo;
pub mod kimi_code;
pub mod kiro;
pub mod kode;
pub mod legacy_kimi;
pub mod letta;
pub mod mimo;
pub mod mistral_vibe;
pub mod nanocoder;
pub mod openclaw;
pub mod opencode;
pub mod openhands;
pub mod pi;
pub mod plandex;
pub mod qwen_code;
pub mod roo_code;
pub mod sculptor;
pub mod swe_agent;
pub mod trae_agent;
pub mod vibe_kanban;
pub mod warp;
pub mod windsurf;
pub mod zcode;
pub mod zed_acp;

#[cfg(test)]
mod decl_tests {
    //! EXT-06/09 declaration coverage: every catalog adapter declares exactly
    //! one of an MCP destination or a corpus-grounded absence (and likewise
    //! for plugins), the declarations' dest keys match the corpus-documented
    //! mechanisms, and every WRITABLE declaration actually round-trips
    //! foreign-preserving server installs through the MCP lifecycle.

    use crate::adapter::DocumentKind;
    use crate::harness_catalog;

    /// (harness id, expected dest file, expected dest key) for every adapter
    /// that declares an MCP destination, per docs/harness-configs/<doc>.md.
    const MCP_DESTS: &[(&str, &str, &str)] = &[
        ("amazon-q-cli", "settings.json", "mcpServers"),
        ("amp", "settings.json", "amp.mcpServers"),
        ("antigravity-cli", "mcp_config.json", "mcpServers"),
        ("auggie", "settings.json", "mcpServers"),
        ("claude-code", ".mcp.json", "mcpServers"),
        ("cline", "cline_mcp_settings.json", "mcpServers"),
        ("codex-cli", "config.toml", "mcp_servers"),
        ("continue-dev", "config.yaml", "mcpServers"),
        ("copilot-cli", "mcp-config.json", "mcpServers"),
        ("crush", "crush.json (global)", "mcp"),
        ("cursor", "mcp.json", "mcpServers"),
        ("forge", ".mcp.json", "mcpServers"),
        ("gemini-cli", "settings.json", "mcpServers"),
        ("goose", "config.yaml", "extensions"),
        ("hermes-agent", "config.yaml", "mcp_servers"),
        ("iflow-cli", "settings.json (user)", "mcpServers"),
        ("junie-cli", "mcp/mcp.json", "mcpServers"),
        ("kilo-code", "global kilo.jsonc", "mcp"),
        ("kimi-code-cli", "mcp.json", "mcpServers"),
        ("kiro", "settings/mcp.json", "mcpServers"),
        ("kode", "config.json", "mcpServers"),
        ("legacy-kimi-cli", "mcp.json", "mcpServers"),
        ("mimo-code", "mimocode.jsonc", "mcp"),
        ("mistral-vibe", "config.toml", "mcp_servers"),
        ("nanocoder", ".mcp.json", "mcpServers"),
        ("opencode", "opencode.json", "mcp"),
        ("openhands", "mcp.json", "mcpServers"),
        ("qwen-code", "settings.json", "mcpServers"),
        ("roo-code", ".roo/mcp.json", "mcpServers"),
        ("trae-agent", "trae_config.yaml", "mcp_servers"),
        ("warp", ".mcp.json", "mcpServers"),
        ("windsurf", "mcp_config.json", "mcpServers"),
        ("zed-acp", "settings.json", "context_servers"),
    ];

    /// Harnesses whose corpus documents NO MCP mechanism (explicit absence).
    const MCP_ABSENT: &[&str] = &[
        "aider",
        "factory-droid",
        "grok-build",
        "conductor",
        "copilot-coding-agent",
        "deepseek-harness",
        "gptme",
        "letta-code",
        "openclaw",
        "pi",
        "plandex",
        "sculptor",
        "swe-agent",
        "vibe-kanban",
        "zcode",
    ];

    /// (harness id, expected plugin kind, requires execution) for adapters
    /// that declare a plugin mechanism.
    const PLUGIN_DECLS: &[(&str, &str, bool)] = &[
        ("amp", "directory_bundle", false),
        ("claude-code", "directory_bundle", false),
        ("grok-build", "directory_bundle", false),
        ("hermes-agent", "npm_ref", true),
        ("junie-cli", "directory_bundle", false),
        ("kimi-code-cli", "marketplace_record", true),
        ("opencode", "directory_bundle", false),
    ];

    /// Expected document kind for a declared MCP destination: JSONC configs
    /// are not always named `*.jsonc` (amp/kilo/mimo/opencode).
    fn expected_dest_kind(harness_id: &str, dest_file: &str) -> DocumentKind {
        const JSONC_DESTS: &[&str] = &["amp", "kilo-code", "mimo-code", "opencode"];
        let ext = std::path::Path::new(dest_file)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if JSONC_DESTS.contains(&harness_id) {
            DocumentKind::Jsonc
        } else {
            match ext.as_str() {
                "toml" => DocumentKind::Toml,
                "yaml" | "yml" => DocumentKind::Yaml,
                _ => DocumentKind::Json,
            }
        }
    }

    /// Build a nested JSON-family seed value for a dotted `dest_key`.
    fn nested_json_seed(dest_key: &str, inner: &serde_json::Value) -> String {
        let segments: Vec<&str> = dest_key.split('.').filter(|s| !s.is_empty()).collect();
        let (last, parents) = segments
            .split_last()
            .unwrap_or((&"mcpServers", &[] as &[&str]));
        let last = *last;
        let mut value = serde_json::json!({ last: inner.clone() });
        for seg in parents.iter().rev() {
            let seg = *seg;
            value = serde_json::json!({ seg: value });
        }
        serde_json::to_string_pretty(&value).unwrap()
    }

    fn ensure_parent(path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
    }

    /// Seed a foreign server entry into `path` in the destination's native
    /// format so round-trip tests exercise foreign preservation for real.
    fn seed_foreign_server(
        path: &std::path::Path,
        decl: &crate::adapter::McpAdapterDecl,
        foreign_id: &str,
    ) {
        ensure_parent(path);
        let identity_list = matches!(
            decl.shape,
            crate::adapter::McpDestShape::IdentityList { .. }
        );
        match decl.kind {
            DocumentKind::Toml if identity_list => std::fs::write(
                path,
                format!("# foreign comment\n[[{key}]]\nname = \"{foreign_id}\"\ncommand = \"foreign-cmd\"\n", key = decl.dest_key),
            )
            .unwrap(),
            DocumentKind::Toml => std::fs::write(
                path,
                format!(
                    "# foreign comment\n[{key}.{foreign_id}]\ncommand = \"foreign-cmd\"\n",
                    key = decl.dest_key
                ),
            )
            .unwrap(),
            DocumentKind::Yaml => std::fs::write(
                path,
                format!(
                    "{key}:\n  {foreign_id}:\n    command: foreign-cmd\n    args:\n      - x\n",
                    key = decl.dest_key
                ),
            )
            .unwrap(),
            _ => {
                let inner = serde_json::json!({
                    foreign_id: {"command": "foreign-cmd", "args": ["x"]}
                });
                std::fs::write(path, nested_json_seed(&decl.dest_key, &inner)).unwrap();
            }
        }
    }

    /// Assert a declared plugin mechanism matches the expected corpus table
    /// row (kind, execution requirement, staging destination).
    fn assert_plugin_decl_matches(id: &str, decl: crate::adapter::PluginAdapterDecl) {
        let expected = PLUGIN_DECLS
            .iter()
            .find(|(hid, _, _)| *hid == id)
            .unwrap_or_else(|| panic!("{id} declares a plugin decl but is not in PLUGIN_DECLS"));
        assert_eq!(
            decl.kind.to_string(),
            expected.1,
            "{id} plugin kind mismatch"
        );
        assert_eq!(
            decl.requires_execution, expected.2,
            "{id} execution requirement mismatch"
        );
        if decl.kind.to_string() == "directory_bundle" {
            assert!(
                decl.dest_dir.is_some_and(|d| !d.is_empty()),
                "{id}: directory bundle must declare a staging destination"
            );
        }
    }

    #[test]
    fn every_adapter_declares_mcp_dest_or_explicit_absence() {
        let adapters = harness_catalog::all_adapters();
        assert_eq!(adapters.len(), 48, "catalog must list 48 adapters");
        let mut declared = 0;
        let mut absent = 0;
        for adapter in &adapters {
            let id = adapter.id().as_str().to_owned();
            let decl = adapter.mcp_decl();
            let absence = adapter.mcp_absence_reason();
            assert!(
                decl.is_some() != absence.is_some(),
                "{id}: exactly one of mcp_decl/mcp_absence_reason must be set (EXT-09 coverage)"
            );
            if let Some(reason) = absence {
                assert!(!reason.is_empty(), "{id}: absence must carry a reason");
                assert!(
                    MCP_ABSENT.contains(&id.as_str()),
                    "{id} declares absence but is not in the expected-absent table: {reason}"
                );
                absent += 1;
            } else {
                let decl = decl.unwrap();
                let expected = MCP_DESTS
                    .iter()
                    .find(|(hid, _, _)| *hid == id)
                    .unwrap_or_else(|| panic!("{id} declares an MCP dest but is not in MCP_DESTS"));
                assert_eq!(decl.dest_file, expected.1, "{id} dest file");
                assert_eq!(decl.dest_key, expected.2, "{id} dest key");
                assert!(!decl.dest_key.is_empty());
                declared += 1;
            }
        }
        assert_eq!(declared + absent, 48);
        // Every table row is exercised (no stale expectations).
        assert_eq!(declared, MCP_DESTS.len(), "MCP_DESTS rows must all match");
        assert_eq!(absent, MCP_ABSENT.len(), "MCP_ABSENT rows must all match");
    }

    #[test]
    fn read_only_decls_carry_an_honest_reason() {
        for adapter in harness_catalog::all_adapters() {
            let Some(reason) = adapter.mcp_decl().and_then(|d| d.read_only) else {
                continue;
            };
            assert!(
                reason.len() > 20,
                "{}: read-only reason must explain the refusal, got {reason:?}",
                adapter.id()
            );
        }
    }

    #[test]
    fn mcp_decl_kinds_match_document_formats() {
        for adapter in harness_catalog::all_adapters() {
            let Some(decl) = adapter.mcp_decl() else {
                continue;
            };
            let id = adapter.id().as_str().to_owned();
            assert_eq!(
                decl.kind,
                expected_dest_kind(&id, &decl.dest_file),
                "{id}: dest file {} kind mismatch",
                decl.dest_file
            );
        }
    }

    #[test]
    fn every_adapter_declares_plugin_mechanism_or_explicit_absence() {
        let mut declared = 0;
        let mut absent = 0;
        for adapter in harness_catalog::all_adapters() {
            let id = adapter.id().as_str().to_owned();
            let decl = adapter.plugin_decl();
            let absence = adapter.plugin_absence_reason();
            assert!(
                decl.is_some() != absence.is_some(),
                "{id}: exactly one of plugin_decl/plugin_absence_reason must be set (EXT-06 coverage)"
            );
            if let Some(reason) = absence {
                assert!(!reason.is_empty(), "{id}: absence must carry a reason");
                absent += 1;
            } else {
                assert_plugin_decl_matches(&id, decl.unwrap());
                declared += 1;
            }
        }
        assert_eq!(declared, PLUGIN_DECLS.len());
        assert_eq!(absent, 48 - PLUGIN_DECLS.len());
    }

    #[test]
    fn writable_mcp_decls_round_trip_foreign_preserving() {
        // Every WRITABLE declaration must actually work through the MCP
        // lifecycle in a temp instance root: install an owned server next to
        // a foreign one, verify both survive, remove the owned one, verify
        // the foreign entry is untouched.
        for adapter in harness_catalog::all_adapters() {
            let Some(decl) = adapter.mcp_decl() else {
                continue;
            };
            if decl.read_only.is_some() {
                continue;
            }
            let id = adapter.id().as_str().to_owned();
            let dir = crate::test_util::temp_dir_unique(&format!("mcp-decl-{id}"));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(&decl.dest_file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }

            let foreign_id = "foreign-server";
            seed_foreign_server(&path, &decl, foreign_id);

            let owned = crate::mcp::McpServerDef::stdio(
                crate::ids::McpServerId::new("owned-server").unwrap(),
                "node",
                vec!["server.js".to_owned()],
            )
            .unwrap();
            crate::mcp::install_mcp_server(&path, &decl, &owned)
                .unwrap_or_else(|e| panic!("{id}: install through declared dest failed: {e}"));

            // Both servers inspectable; foreign survived.
            let inspected = crate::mcp::inspect_servers(&path, &decl)
                .unwrap_or_else(|e| panic!("{id}: inspect failed: {e}"));
            assert!(
                inspected.contains_key(&crate::ids::McpServerId::new(foreign_id).unwrap()),
                "{id}: foreign server lost after install"
            );
            assert!(inspected.contains_key(&owned.id), "{id}: owned missing");

            // Removal leaves the foreign entry bytes.
            crate::mcp::remove_mcp_server(&path, &decl, &owned.id)
                .unwrap_or_else(|e| panic!("{id}: remove failed: {e}"));
            let after = std::fs::read_to_string(&path).unwrap();
            assert!(
                after.contains(foreign_id),
                "{id}: foreign server must survive removal: {after}"
            );
            drop(std::fs::remove_dir_all(&dir));
        }
    }

    #[test]
    fn read_only_mcp_decls_refuse_writes_and_still_inspect() {
        for adapter in harness_catalog::all_adapters() {
            let Some(decl) = adapter.mcp_decl() else {
                continue;
            };
            let Some(reason) = decl.read_only.clone() else {
                continue;
            };
            assert_read_only_refuses(adapter.id().as_str(), &decl, &reason);
        }
    }

    /// One read-only destination: inspection still works, writes refuse with
    /// the declared reason (or the codec's `LossyWrite`), and no byte changes.
    fn assert_read_only_refuses(id: &str, decl: &crate::adapter::McpAdapterDecl, reason: &str) {
        let dir = crate::test_util::temp_dir_unique(&format!("mcp-ro-{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(&decl.dest_file);
        seed_empty_container(&path, decl);
        let before = std::fs::read(&path).unwrap();
        // Refusing writes never blinds reads.
        let inspected = crate::mcp::inspect_servers(&path, decl);
        assert!(
            inspected.is_ok(),
            "{id}: read-only decl must still inspect: {:?}",
            inspected.unwrap_err()
        );
        let owned = crate::mcp::McpServerDef::stdio(
            crate::ids::McpServerId::new("owned").unwrap(),
            "node",
            vec!["s.js".to_owned()],
        )
        .unwrap();
        let err = crate::mcp::install_mcp_server(&path, decl, &owned)
            .expect_err(&format!("{id}: read-only decl must refuse writes"));
        match err {
            crate::error::CoreError::UnsupportedOperation { reason: r, .. } => {
                assert_eq!(r, reason);
            }
            crate::error::CoreError::Config(superai_config::ConfigError::LossyWrite { .. }) => {}
            other => panic!("{id}: expected refusal, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "{id}: bytes must not change"
        );
        drop(std::fs::remove_dir_all(&dir));
    }

    /// Seed a minimal parseable empty container for a declared destination.
    fn seed_empty_container(path: &std::path::Path, decl: &crate::adapter::McpAdapterDecl) {
        ensure_parent(path);
        let seed = match decl.kind {
            DocumentKind::Toml => format!("[{}]\n", decl.dest_key.replace('.', "_")),
            DocumentKind::Yaml => format!("{}: {{}}\n", decl.dest_key),
            DocumentKind::Jsonc | DocumentKind::Json => {
                nested_json_seed(&decl.dest_key, &serde_json::json!({}))
            }
            _ => "{}".to_owned(),
        };
        std::fs::write(path, seed).unwrap();
    }

    #[test]
    fn directory_bundle_plugin_decls_stage_through_a_transaction() {
        for adapter in harness_catalog::all_adapters() {
            let Some(decl) = adapter.plugin_decl() else {
                continue;
            };
            if decl.kind.to_string() != "directory_bundle" {
                continue;
            }
            let id = adapter.id().as_str().to_owned();
            // Smoke: the declared destination names a directory under the
            // instance root and staging through it works end to end.
            let dir = crate::test_util::temp_dir_unique(&format!("plug-decl-{id}"));
            let instance_root = dir.join("instance");
            let source_dir = dir.join("bundle");
            std::fs::create_dir_all(source_dir.join("skills")).unwrap();
            std::fs::write(source_dir.join("skills").join("s.md"), b"# skill\n").unwrap();
            let mut registry = crate::plugin::PluginRegistry::load(&dir.join("registry")).unwrap();
            let source = crate::plugin::PluginSource {
                id: crate::ids::PluginId::new("smoke-plugin").unwrap(),
                kind: crate::adapter::PluginKind::DirectoryBundle,
                locator: source_dir.display().to_string(),
                version: None,
                digest: None,
            };
            let record = crate::plugin::install_directory_bundle(
                &mut registry,
                &source,
                &decl,
                &instance_root,
            )
            .unwrap_or_else(|e| panic!("{id}: staging through declared dest failed: {e}"));
            let staged_dir = decl.dest_dir.clone().unwrap_or_default();
            assert!(
                instance_root
                    .join(&staged_dir)
                    .join("smoke-plugin")
                    .join("skills")
                    .join("s.md")
                    .is_file(),
                "{id}: staged file must land in the declared destination"
            );
            assert!(record.staged_files.is_some_and(|f| !f.is_empty()));
            drop(std::fs::remove_dir_all(&dir));
        }
    }
}
