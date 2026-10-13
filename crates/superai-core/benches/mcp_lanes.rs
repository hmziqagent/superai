//! Criterion benches over the mcp install lane.
//! Measurement only: every lane calls the production function unchanged.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use duct as _;
use hex as _;
use semver as _;
use serde as _;
use serde_json as _;
use sha2 as _;
use superai_config as _;
use thiserror as _;
use toml_edit as _;
use toride_apps as _;
use toride_proxy as _;
use toride_registry as _;
use toride_runner as _;
use toride_wireguard as _;
use ureq as _;
use yaml_serde as _;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use superai_core::adapter::{ConfigScope, DocumentKind, McpAdapterDecl, RestartBehavior};
use superai_core::ids::McpServerId;
use superai_core::mcp::{McpServerDef, install_mcp_server};

fn scratch_dir(label: &str) -> PathBuf {
    let uniq = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("superai-bench-mcp-{label}-{uniq}"));
    fs::create_dir_all(&dir).expect("bench scratch dir must be creatable");
    dir
}

/// Scratch on the home filesystem, not tmpfs, so the contractual fsync
/// trio (backup, staged temp, parent dir) is visible in the measurement.
fn home_scratch_dir(label: &str) -> PathBuf {
    let uniq = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let home = std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from);
    let dir = home
        .join(".cache")
        .join("superai-bench-mcp")
        .join(format!("{label}-{uniq}"));
    fs::create_dir_all(&dir).expect("bench home scratch dir must be creatable");
    dir
}

fn dest_decl() -> McpAdapterDecl {
    McpAdapterDecl::new(
        "settings.json",
        "mcpServers",
        DocumentKind::Json,
        ConfigScope::User,
        RestartBehavior::None,
    )
}

/// Eight foreign servers plus one unrelated top-level key, so every install
/// pays the foreign-preserve path over a realistic document.
fn seed_document() -> String {
    let mut servers = String::from("{\"top\": {\"a\": 1}, \"mcpServers\": {");
    for i in 0..8 {
        if i > 0 {
            servers.push(',');
        }
        write!(
            servers,
            "\"foreign-{i}\": {{\"command\": \"node\", \"args\": [\"s{i}.js\"], \"env\": {{\"K\": \"v{i}\"}}}}"
        )
        .expect("writing to an in-memory String cannot fail");
    }
    servers.push_str("}}");
    servers
}

fn bench_install_mcp(c: &mut Criterion) {
    let mut group = c.benchmark_group("mcp_install");
    group.sample_size(30);
    let dir = scratch_dir("install");
    let path = dir.join("settings.json");
    let decl = dest_decl();
    group.bench_function("install_new_server_over_8_foreign", |b| {
        b.iter_batched(
            || {
                if dir.exists() {
                    fs::remove_dir_all(&dir).expect("bench scratch reset must clean");
                }
                fs::create_dir_all(&dir).expect("bench scratch reset must recreate");
                fs::write(&path, seed_document()).expect("bench seed must write");
            },
            |()| {
                let id = McpServerId::new("bench-new").expect("bench server id is valid");
                let server = McpServerDef::stdio(id, "uvx", vec!["bench-server".to_owned()])
                    .expect("bench server definition is valid");
                install_mcp_server(&path, &decl, &server)
                    .expect("bench install over seeded destination must commit");
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
    if let Err(e) = fs::remove_dir_all(&dir) {
        eprintln!("bench scratch cleanup failed: {e}");
    }
}

fn toml_dest_decl() -> McpAdapterDecl {
    McpAdapterDecl::new(
        "config.toml",
        "mcp_servers",
        DocumentKind::Toml,
        ConfigScope::User,
        RestartBehavior::None,
    )
}

/// Eight foreign servers as `[mcp_servers.*]` tables plus one top-level key,
/// exercising the `toml_edit` write path.
fn seed_toml_document() -> String {
    let mut doc = String::from("top = 1\n");
    for i in 0..8 {
        write!(
            doc,
            "\n[mcp_servers.foreign-{i}]\ncommand = \"node\"\nargs = [\"s{i}.js\"]\n"
        )
        .expect("writing to an in-memory String cannot fail");
    }
    doc
}

fn bench_install_mcp_toml(c: &mut Criterion) {
    let mut group = c.benchmark_group("mcp_install");
    group.sample_size(30);
    let dir = scratch_dir("install-toml");
    let path = dir.join("config.toml");
    let decl = toml_dest_decl();
    group.bench_function("install_new_server_toml_over_8_foreign", |b| {
        b.iter_batched(
            || {
                if dir.exists() {
                    fs::remove_dir_all(&dir).expect("bench scratch reset must clean");
                }
                fs::create_dir_all(&dir).expect("bench scratch reset must recreate");
                fs::write(&path, seed_toml_document()).expect("bench seed must write");
            },
            |()| {
                let id = McpServerId::new("bench-new").expect("bench server id is valid");
                let server = McpServerDef::stdio(id, "uvx", vec!["bench-server".to_owned()])
                    .expect("bench server definition is valid");
                install_mcp_server(&path, &decl, &server)
                    .expect("bench install over seeded toml destination must commit");
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
    if let Err(e) = fs::remove_dir_all(&dir) {
        eprintln!("bench scratch cleanup failed: {e}");
    }
}

fn bench_install_mcp_real_disk(c: &mut Criterion) {
    let mut group = c.benchmark_group("mcp_install_real_disk");
    group.sample_size(20);
    let dir = home_scratch_dir("install");
    let path = dir.join("settings.json");
    let decl = dest_decl();
    group.bench_function("install_new_server_over_8_foreign", |b| {
        b.iter_batched(
            || {
                if dir.exists() {
                    fs::remove_dir_all(&dir).expect("bench scratch reset must clean");
                }
                fs::create_dir_all(&dir).expect("bench scratch reset must recreate");
                fs::write(&path, seed_document()).expect("bench seed must write");
            },
            |()| {
                let id = McpServerId::new("bench-new").expect("bench server id is valid");
                let server = McpServerDef::stdio(id, "uvx", vec!["bench-server".to_owned()])
                    .expect("bench server definition is valid");
                install_mcp_server(&path, &decl, &server)
                    .expect("bench install over seeded destination must commit");
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
    if let Err(e) = fs::remove_dir_all(&dir) {
        eprintln!("bench scratch cleanup failed: {e}");
    }
    if let Some(parent) = dir.parent()
        && let Err(e) = fs::remove_dir(parent)
    {
        eprintln!("bench scratch parent cleanup failed: {e}");
    }
}

criterion_group!(
    benches,
    bench_install_mcp,
    bench_install_mcp_toml,
    bench_install_mcp_real_disk
);
criterion_main!(benches);
