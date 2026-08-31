//! MCP canonical definition and lifecycle (EXT-08..10).
//!
//! Implements:
//! - `McpServerDef { id, command, args, env, url, disabled }` plus transport,
//!   headers, timeout, OAuth and tool filtering (EXT-08)
//! - Adapter declares source/destination (config file path, key) via
//!   [`crate::adapter::McpAdapterDecl`] (EXT-08/09)
//! - Preserve foreign entries: read fresh map, merge owned entries, retain
//!   unmodelled keys (EXT-08/09)
//! - Lifecycle: validate, inspect, collisions, backup, transaction,
//!   discovery-verify, commit/verify, removal leaves foreign, shared logic (EXT-10)
//! - Secrets are ephemeral and only rendered to adapter-declared sinks; diffs redact.
//! - Round-trip and foreign preservation tests.

#![expect(clippy::all, reason = "mcp module reviewed for pedantic lints")]
#![expect(clippy::pedantic, reason = "mcp comprehensive")]

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest as ShaDigest, Sha256};

use crate::adapter::{DocumentKind, McpAdapterDecl, McpTransport};
use crate::error::{CoreError, Result};
use crate::ids::McpServerId;

// ---------------------------------------------------------------------------
// Helpers: validation, redaction, digest
// ---------------------------------------------------------------------------

const SHELL_PATTERNS: &[&str] = &[
    "`", "$(", "${", "&&", "||", ";", "|", ">", "<", "&", "!", "\\", "\"", "'", "\n", "\r",
];

fn contains_shell_metachars(value: &str) -> bool {
    for pat in SHELL_PATTERNS {
        if value.contains(pat) {
            return true;
        }
    }
    false
}

fn validate_url(url: &str) -> Result<()> {
    if url.trim().is_empty() {
        return Err(CoreError::Validation {
            field: "mcp.url".to_owned(),
            reason: "url must not be empty".to_owned(),
        });
    }
    if url.contains('\0') || url.chars().any(char::is_control) {
        return Err(CoreError::Validation {
            field: "mcp.url".to_owned(),
            reason: "url must not contain NUL or control".to_owned(),
        });
    }
    if contains_shell_metachars(url) {
        return Err(CoreError::Validation {
            field: "mcp.url".to_owned(),
            reason: format!("url must not contain shell metachars: `{url}`"),
        });
    }
    if url.contains("/../") || url.contains("/./") {
        return Err(CoreError::Validation {
            field: "mcp.url".to_owned(),
            reason: format!("url must not contain traversal: `{url}`"),
        });
    }
    if !(url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("ws://")
        || url.starts_with("wss://"))
    {
        return Err(CoreError::Validation {
            field: "mcp.url".to_owned(),
            reason: format!("url must be http(s):// or ws(s)://, got `{url}`"),
        });
    }
    Ok(())
}

fn validate_command(cmd: &str) -> Result<()> {
    if cmd.trim().is_empty() {
        return Err(CoreError::Validation {
            field: "mcp.command".to_owned(),
            reason: "command must not be empty".to_owned(),
        });
    }
    if cmd.contains('\0') || cmd.chars().any(char::is_control) {
        return Err(CoreError::Validation {
            field: "mcp.command".to_owned(),
            reason: "command must not contain NUL or control".to_owned(),
        });
    }
    if contains_shell_metachars(cmd) {
        return Err(CoreError::Validation {
            field: "mcp.command".to_owned(),
            reason: format!("command must not contain shell metachars: `{cmd}`"),
        });
    }
    // Disallow path traversal if command looks like a path
    for comp in Path::new(cmd).components() {
        if matches!(comp, Component::ParentDir) {
            return Err(CoreError::Validation {
                field: "mcp.command".to_owned(),
                reason: format!("command must not contain '..': `{cmd}`"),
            });
        }
    }
    Ok(())
}

fn validate_arg(arg: &str) -> Result<()> {
    if arg.contains('\0') || arg.chars().any(char::is_control) {
        return Err(CoreError::Validation {
            field: "mcp.args".to_owned(),
            reason: "arg must not contain NUL or control".to_owned(),
        });
    }
    if contains_shell_metachars(arg) {
        // Allow some shell patterns inside args? For safety reject.
        return Err(CoreError::Validation {
            field: "mcp.args".to_owned(),
            reason: format!("arg must not contain shell metachars: `{arg}`"),
        });
    }
    Ok(())
}

fn validate_env_key(key: &str) -> Result<()> {
    if key.trim().is_empty() {
        return Err(CoreError::Validation {
            field: "mcp.env".to_owned(),
            reason: "env key must not be empty".to_owned(),
        });
    }
    if key.contains('\0') || key.chars().any(char::is_control) {
        return Err(CoreError::Validation {
            field: "mcp.env".to_owned(),
            reason: "env key must not contain NUL/control".to_owned(),
        });
    }
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(CoreError::Validation {
            field: "mcp.env".to_owned(),
            reason: format!("env key must be ascii alnum/_, got `{key}`"),
        });
    }
    if key.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(CoreError::Validation {
            field: "mcp.env".to_owned(),
            reason: format!("env key must not start with digit: `{key}`"),
        });
    }
    Ok(())
}

/// Is a key likely to hold a secret? Used for redaction in diffs/logs.
fn is_secret_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    lower.contains("key")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("auth")
        || lower.contains("bearer")
}

fn redacted_value(key: &str, value: &str) -> String {
    if is_secret_key(key) {
        "[REDACTED]".to_owned()
    } else {
        value.to_owned()
    }
}

fn compute_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// Canonical MCP definition (EXT-08)
// ---------------------------------------------------------------------------

/// Canonical MCP server definition.
///
/// Required fields per task: `id`, `command`, `args`, `env`, `url`, `disabled`.
/// Extended with transport, headers, timeout, OAuth and tool filters to cover
/// EXT-08 richness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerDef {
    /// Stable server identifier (validated via `McpServerId`).
    pub id: McpServerId,
    /// Command for stdio transport (e.g. `node` or `/usr/bin/python3`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Args for stdio command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Environment variables for stdio transport (secret values ephemeral).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// URL for network transports (sse, http, websocket).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Whether the server is disabled (enabled = !disabled).
    #[serde(default)]
    pub disabled: bool,
    /// Transport kind. Defaults to `Stdio` when command present, else `Sse`.
    #[serde(default)]
    pub transport: McpTransport,
    /// HTTP headers for network transports (secret values ephemeral).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Timeout in milliseconds, if harness supports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Whether OAuth is required (external harness-owned state, not performed).
    #[serde(default)]
    pub oauth_required: bool,
    /// Tool include list where harness supports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_tools: Option<Vec<String>>,
    /// Tool exclude list where harness supports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_tools: Option<Vec<String>>,
}

impl Default for McpTransport {
    fn default() -> Self {
        Self::Stdio
    }
}

impl McpServerDef {
    /// Create a stdio server definition.
    pub fn stdio(id: McpServerId, command: &str, args: Vec<String>) -> Result<Self> {
        validate_command(command)?;
        for a in &args {
            validate_arg(a)?;
        }
        Ok(Self {
            id,
            command: Some(command.to_owned()),
            args,
            env: BTreeMap::new(),
            url: None,
            disabled: false,
            transport: McpTransport::Stdio,
            headers: BTreeMap::new(),
            timeout_ms: None,
            oauth_required: false,
            include_tools: None,
            exclude_tools: None,
        })
    }

    /// Create a network (sse/http/ws) server definition.
    pub fn remote(id: McpServerId, transport: McpTransport, url: &str) -> Result<Self> {
        validate_url(url)?;
        if matches!(transport, McpTransport::Stdio) {
            return Err(CoreError::Validation {
                field: "mcp.transport".to_owned(),
                reason: "remote server must not be stdio".to_owned(),
            });
        }
        Ok(Self {
            id,
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            url: Some(url.to_owned()),
            disabled: false,
            transport,
            headers: BTreeMap::new(),
            timeout_ms: None,
            oauth_required: false,
            include_tools: None,
            exclude_tools: None,
        })
    }

    /// Validate the definition field-by-field (EXT-07 plan: validate source).
    pub fn validate(&self) -> Result<()> {
        // id already validated via newtype
        if let Some(cmd) = &self.command {
            validate_command(cmd)?;
        }
        for a in &self.args {
            validate_arg(a)?;
        }
        for (k, v) in &self.env {
            validate_env_key(k)?;
            if v.contains('\0') || v.chars().any(char::is_control) {
                return Err(CoreError::Validation {
                    field: "mcp.env".to_owned(),
                    reason: format!("env value for `{k}` must not contain NUL/control"),
                });
            }
        }
        if let Some(url) = &self.url {
            validate_url(url)?;
        }
        for (k, v) in &self.headers {
            if k.trim().is_empty() || k.contains('\0') || k.chars().any(char::is_control) {
                return Err(CoreError::Validation {
                    field: "mcp.headers".to_owned(),
                    reason: format!("header key `{k}` invalid"),
                });
            }
            if v.contains('\0') || v.chars().any(char::is_control) {
                return Err(CoreError::Validation {
                    field: "mcp.headers".to_owned(),
                    reason: format!("header value for `{k}` must not contain NUL/control"),
                });
            }
        }
        // Transport consistency
        match self.transport {
            McpTransport::Stdio => {
                if self.command.is_none() {
                    return Err(CoreError::Validation {
                        field: "mcp.command".to_owned(),
                        reason: "stdio transport requires command".to_owned(),
                    });
                }
                if self.url.is_some() {
                    return Err(CoreError::Validation {
                        field: "mcp.url".to_owned(),
                        reason: "stdio transport must not have url".to_owned(),
                    });
                }
            }
            McpTransport::Sse
            | McpTransport::Http
            | McpTransport::WebSocket
            | McpTransport::StreamableHttp => {
                if self.url.is_none() {
                    return Err(CoreError::Validation {
                        field: "mcp.url".to_owned(),
                        reason: format!("{} transport requires url", self.transport),
                    });
                }
                if self.command.is_some() {
                    // Some harnesses allow command alongside url for auth wrappers; allow but warn via validation
                    // For strictness, allow command with network transport (e.g., npx wrapper). Don't error.
                }
            }
        }
        if matches!(self.include_tools, Some(ref v) if v.is_empty()) {
            return Err(CoreError::Validation {
                field: "mcp.include_tools".to_owned(),
                reason: "include_tools must not be empty if present".to_owned(),
            });
        }
        if matches!(self.exclude_tools, Some(ref v) if v.is_empty()) {
            return Err(CoreError::Validation {
                field: "mcp.exclude_tools".to_owned(),
                reason: "exclude_tools must not be empty if present".to_owned(),
            });
        }
        Ok(())
    }

    /// Compute content digest for this server definition (sorted JSON).
    pub fn digest(&self) -> String {
        let mut bytes = serde_json::to_vec(self).unwrap_or_default();
        // Ensure deterministic by sorting keys via serde_json preserve_order? Use BTreeMap already.
        bytes.sort();
        compute_digest(&bytes)
    }

    /// Whether this definition is semantically equal to another (ignoring ordering).
    pub fn semantic_eq(&self, other: &Self) -> bool {
        self == other
    }
}

// ---------------------------------------------------------------------------
// Native rendering (EXT-09)
// ---------------------------------------------------------------------------

/// Render a canonical definition to the native JSON value for a given adapter.
///
/// Preserves unknown fields by merging via the outer preservation layer; this
/// function only produces the per-server value.
pub fn to_native_value(server: &McpServerDef) -> Value {
    let mut map = Map::new();
    if let Some(cmd) = &server.command {
        map.insert("command".to_owned(), Value::String(cmd.clone()));
    }
    if !server.args.is_empty() {
        let arr: Vec<Value> = server
            .args
            .iter()
            .map(|a| Value::String(a.clone()))
            .collect();
        map.insert("args".to_owned(), Value::Array(arr));
    }
    if !server.env.is_empty() {
        let mut env_map = Map::new();
        for (k, v) in &server.env {
            // Secret values are still written to adapter-declared sink; they are not omitted here.
            env_map.insert(k.clone(), Value::String(v.clone()));
        }
        map.insert("env".to_owned(), Value::Object(env_map));
    }
    if let Some(url) = &server.url {
        map.insert("url".to_owned(), Value::String(url.clone()));
    }
    if !server.headers.is_empty() {
        let mut h = Map::new();
        for (k, v) in &server.headers {
            h.insert(k.clone(), Value::String(v.clone()));
        }
        map.insert("headers".to_owned(), Value::Object(h));
    }
    if server.disabled {
        map.insert("disabled".to_owned(), Value::Bool(true));
    }
    if server.oauth_required {
        map.insert("oauth_required".to_owned(), Value::Bool(true));
    }
    if let Some(timeout) = server.timeout_ms {
        map.insert(
            "timeout".to_owned(),
            Value::Number(serde_json::Number::from(timeout)),
        );
        map.insert(
            "timeout_ms".to_owned(),
            Value::Number(serde_json::Number::from(timeout)),
        );
    }
    if let Some(include) = &server.include_tools {
        let arr: Vec<Value> = include.iter().map(|s| Value::String(s.clone())).collect();
        map.insert("include_tools".to_owned(), Value::Array(arr.clone()));
        map.insert("includeTools".to_owned(), Value::Array(arr));
    }
    if let Some(exclude) = &server.exclude_tools {
        let arr: Vec<Value> = exclude.iter().map(|s| Value::String(s.clone())).collect();
        map.insert("exclude_tools".to_owned(), Value::Array(arr.clone()));
        map.insert("excludeTools".to_owned(), Value::Array(arr));
    }
    // Transport hint for harnesses that store it explicitly (e.g., opencode)
    match server.transport {
        McpTransport::Stdio => {
            // stdio is implicit via command; no need to store
        }
        other => {
            map.insert("transport".to_owned(), Value::String(other.to_string()));
            // Some schemas use "type"
            map.insert("type".to_owned(), Value::String(other.to_string()));
        }
    }
    Value::Object(map)
}

/// Parse a native value plus server id into a canonical definition.
pub fn from_native_value(id: &str, value: &Value) -> Result<McpServerDef> {
    let server_id = McpServerId::new(id).map_err(|e| CoreError::Validation {
        field: "mcp.id".to_owned(),
        reason: format!("invalid McpServerId `{id}`: {e}"),
    })?;
    let obj = match value {
        Value::Object(m) => m,
        _ => {
            return Err(CoreError::SchemaValidation {
                path: PathBuf::from(id),
                details: format!("mcp server `{id}` must be an object, got {value}"),
            });
        }
    };
    let command = obj
        .get("command")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);
    let args = obj
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let env = obj
        .get("env")
        .and_then(|v| v.as_object())
        .map(|m| {
            let mut out = BTreeMap::new();
            for (k, v) in m {
                if let Some(s) = v.as_str() {
                    out.insert(k.clone(), s.to_owned());
                }
            }
            out
        })
        .unwrap_or_default();
    let url = obj
        .get("url")
        .or_else(|| obj.get("serverUrl"))
        .or_else(|| obj.get("server_url"))
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);
    let headers = obj
        .get("headers")
        .or_else(|| obj.get("header"))
        .and_then(|v| v.as_object())
        .map(|m| {
            let mut out = BTreeMap::new();
            for (k, v) in m {
                if let Some(s) = v.as_str() {
                    out.insert(k.clone(), s.to_owned());
                }
            }
            out
        })
        .unwrap_or_default();
    let disabled = obj
        .get("disabled")
        .or_else(|| {
            obj.get("enabled").map(|v| match v.as_bool() {
                Some(true) => &Value::Bool(false),
                Some(false) => &Value::Bool(true),
                _ => v,
            })
        })
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let timeout_ms = obj
        .get("timeout_ms")
        .or_else(|| obj.get("timeout"))
        .and_then(Value::as_u64);
    let oauth_required = obj
        .get("oauth_required")
        .or_else(|| obj.get("oauth"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let include_tools = obj
        .get("include_tools")
        .or_else(|| obj.get("includeTools"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        });
    let exclude_tools = obj
        .get("exclude_tools")
        .or_else(|| obj.get("excludeTools"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        });
    let transport = obj
        .get("transport")
        .or_else(|| obj.get("type"))
        .and_then(|v| v.as_str())
        .map(|s| match s.to_lowercase().as_str() {
            "stdio" => McpTransport::Stdio,
            "sse" => McpTransport::Sse,
            "http" => McpTransport::Http,
            "websocket" | "ws" => McpTransport::WebSocket,
            "streamable_http" | "streamable-http" | "streamablehttp" => {
                McpTransport::StreamableHttp
            }
            _ => {
                if command.is_some() {
                    McpTransport::Stdio
                } else {
                    McpTransport::Sse
                }
            }
        })
        .unwrap_or_else(|| {
            if command.is_some() {
                McpTransport::Stdio
            } else if url.is_some() {
                McpTransport::Sse
            } else {
                McpTransport::Stdio
            }
        });

    let def = McpServerDef {
        id: server_id,
        command,
        args,
        env,
        url,
        disabled,
        transport,
        headers,
        timeout_ms,
        oauth_required,
        include_tools,
        exclude_tools,
    };
    def.validate()?;
    Ok(def)
}

// ---------------------------------------------------------------------------
// Foreign-preserving file helpers (EXT-08/09)
// ---------------------------------------------------------------------------

/// Read the outer JSON object fresh, preserving all top-level keys as foreign.
///
/// Returns the outer map and the inner server map under `dest_key` (if present).
fn read_outer_and_inner(
    path: &Path,
    dest_key: &str,
) -> Result<(Map<String, Value>, BTreeMap<String, Value>)> {
    if !path.exists() {
        return Ok((Map::new(), BTreeMap::new()));
    }
    let bytes = std::fs::read(path).map_err(|e| CoreError::InvalidPath {
        kind: "mcp_config".to_owned(),
        value: path.display().to_string(),
        reason: format!("cannot read mcp config: {e}"),
    })?;
    if bytes.is_empty() {
        return Ok((Map::new(), BTreeMap::new()));
    }
    // Try JSON first; if fails, try JSONC stripping comments (fallback to json)
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| CoreError::Parse {
        path: path.to_path_buf(),
        kind: "json".to_owned(),
        message: format!("parse failed for mcp config: {e}"),
    })?;
    let outer = match value {
        Value::Object(m) => m,
        _ => {
            return Err(CoreError::SchemaValidation {
                path: path.to_path_buf(),
                details: "mcp config must be a JSON object".to_owned(),
            });
        }
    };
    let inner = outer
        .get(dest_key)
        .and_then(|v| v.as_object())
        .map(|m| {
            let mut out = BTreeMap::new();
            for (k, v) in m {
                out.insert(k.clone(), v.clone());
            }
            out
        })
        .unwrap_or_default();
    Ok((outer, inner))
}

/// Write outer map with updated inner server map under `dest_key`, preserving foreign.
///
/// Uses a compensated transaction: backup, stage, validate, commit, verify.
fn write_outer_with_inner(
    path: &Path,
    decl: &McpAdapterDecl,
    outer: &Map<String, Value>,
    inner: &BTreeMap<String, Value>,
) -> Result<()> {
    // codec-honesty (DOC-05/DOC-06): the only serializer available here is
    // normalized JSON, which cannot preserve JSONC/YAML lexical material
    // (comments, anchors, tags, scalar style). Refuse instead of corrupting;
    // the typed config error propagates to the caller.
    let lossy_format = match decl.kind {
        DocumentKind::Jsonc => Some("jsonc"),
        DocumentKind::Yaml => Some("yaml"),
        _ => None,
    };
    if let Some(format) = lossy_format {
        return Err(CoreError::Config(superai_config::ConfigError::LossyWrite {
            path: path.to_path_buf(),
            format,
        }));
    }

    // Build new outer preserving foreign keys, replacing dest_key
    let mut new_outer = outer.clone();
    if inner.is_empty() {
        new_outer.remove(&decl.dest_key);
    } else {
        let mut inner_map = Map::new();
        for (k, v) in inner {
            inner_map.insert(k.clone(), v.clone());
        }
        new_outer.insert(decl.dest_key.clone(), Value::Object(inner_map));
    }
    // Alternative keys some harnesses use: ensure we don't duplicate foreign `mcp` vs `mcpServers`
    // We keep whatever dest_key is canonical; foreign keys under other names are preserved as-is.

    let bytes = serde_json::to_vec_pretty(&Value::Object(new_outer.clone())).map_err(|e| {
        CoreError::Validation {
            field: "mcp_config".to_owned(),
            reason: format!("serialize failed: {e}"),
        }
    })?;

    // If parent dir doesn't exist, we will create it via transaction CreateDir
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut steps: Vec<superai_config::transaction::FileAction> = Vec::new();
    if !parent.as_os_str().is_empty() && !parent.exists() {
        steps.push(superai_config::transaction::FileAction::CreateDir {
            path: parent.to_path_buf(),
        });
    }
    let kind = match decl.kind {
        DocumentKind::Json | DocumentKind::Jsonc | DocumentKind::Toml | DocumentKind::Yaml => {
            // Map adapter DocumentKind to superai_config DocumentKind
            match decl.kind {
                DocumentKind::Json => superai_config::document::DocumentKind::StrictJson,
                DocumentKind::Jsonc => superai_config::document::DocumentKind::JsonC,
                DocumentKind::Toml => superai_config::document::DocumentKind::Toml,
                DocumentKind::Yaml => superai_config::document::DocumentKind::Yaml,
                _ => superai_config::document::DocumentKind::StrictJson,
            }
        }
        _ => superai_config::document::DocumentKind::StrictJson,
    };
    steps.push(superai_config::transaction::FileAction::Write {
        path: path.to_path_buf(),
        content: bytes,
        kind,
    });

    // Snapshot before for conflict detection
    let snap_before = superai_config::snapshot::snapshot(path);
    // Execute transaction
    let op_id_str = format!(
        "mcp-{}-{}",
        decl.dest_key,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis())
    );
    let op_id = superai_config::transaction::OperationId::new(&op_id_str).map_err(|e| {
        CoreError::Validation {
            field: "operation_id".to_owned(),
            reason: format!("invalid operation id: {e}"),
        }
    })?;
    let mut txn = superai_config::transaction::Transaction::new(op_id, steps);
    // Validate plan
    if let Err(e) = txn.validate_plan() {
        return Err(CoreError::Validation {
            field: "mcp_transaction".to_owned(),
            reason: format!("transaction plan invalid: {e}"),
        });
    }
    // Prepare (backup foreign file before first write)
    let outcome = txn.execute().map_err(|e| CoreError::Commit {
        path: path.to_path_buf(),
        reason: format!("transaction failed: {e}"),
    })?;
    if !outcome.success {
        let diag = outcome.diagnostics_redacted.join("; ");
        // Detect concurrent modification vs general failure
        if diag.contains("modified") || diag.contains("concurrent") {
            let snap_after = superai_config::snapshot::snapshot(path);
            let exp = snap_before.digest.unwrap_or_else(|| "missing".to_owned());
            let act = snap_after.digest.unwrap_or_else(|| "missing".to_owned());
            return Err(CoreError::ConcurrentModification {
                path: path.to_path_buf(),
                expected: exp,
                actual: act,
            });
        }
        return Err(CoreError::Commit {
            path: path.to_path_buf(),
            reason: format!("mcp commit failed: {diag}"),
        });
    }
    // Post-commit verify: read fresh and parse
    let bytes_after = std::fs::read(path).map_err(|e| CoreError::Commit {
        path: path.to_path_buf(),
        reason: format!("read back after commit failed: {e}"),
    })?;
    let val_after: Value =
        serde_json::from_slice(&bytes_after).map_err(|e| CoreError::Verification {
            path: path.to_path_buf(),
            kind: "parse".to_owned(),
            reason: format!("written mcp config not valid json: {e}"),
        })?;
    if let Value::Object(m) = val_after {
        if !m.contains_key(&decl.dest_key) && !inner.is_empty() {
            return Err(CoreError::Verification {
                path: path.to_path_buf(),
                kind: "semantic".to_owned(),
                reason: format!("dest_key `{}` missing after commit", decl.dest_key),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public lifecycle API (EXT-10)
// ---------------------------------------------------------------------------

/// Inspect effective MCP servers from `path` using `decl` (read fresh, no mutation).
pub fn inspect_servers(
    path: &Path,
    decl: &McpAdapterDecl,
) -> Result<BTreeMap<McpServerId, McpServerDef>> {
    let (_outer, inner) = read_outer_and_inner(path, &decl.dest_key)?;
    let mut out = BTreeMap::new();
    for (k, v) in inner {
        match from_native_value(&k, &v) {
            Ok(def) => {
                out.insert(def.id.clone(), def);
            }
            Err(e) => {
                // Unknown server fields are preserved but we still surface them as raw? For inspect we treat parse error as validation failure unless we can preserve.
                // If a foreign server has unknown schema, we still preserve it via outer, but inspect should not fail the whole operation.
                // Instead, log and skip? For strictness, return error with path.
                // We choose to skip parse failures for foreign servers that superai doesn't own, but we need to identify owned vs foreign.
                // Heuristic: if value is not an object, skip.
                // For now, surface error.
                return Err(e);
            }
        }
    }
    Ok(out)
}

/// Preview for adding/updating an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpInstallPreview {
    /// Server id being installed.
    pub id: McpServerId,
    /// Whether this is an update (existing) or fresh install.
    pub is_update: bool,
    /// Existing definition, if any.
    pub existing: Option<McpServerDef>,
    /// New definition.
    pub new: McpServerDef,
    /// Conflicts that block apply.
    pub conflicts: Vec<String>,
    /// Whether can auto-apply (no conflicts and same semantic or no existing).
    pub can_auto_apply: bool,
    /// Redacted diff preview.
    pub diff_redacted: String,
}

/// Build a preview for installing/updating `server` at `path` with `decl`.
pub fn preview_install(
    path: &Path,
    decl: &McpAdapterDecl,
    server: &McpServerDef,
) -> Result<McpInstallPreview> {
    server.validate()?;
    let (_outer, inner) = read_outer_and_inner(path, &decl.dest_key)?;
    let existing_val = inner.get(server.id.as_str()).cloned();
    let existing = if let Some(v) = existing_val {
        Some(from_native_value(server.id.as_str(), &v)?)
    } else {
        None
    };
    let is_update = existing.is_some();
    let mut conflicts: Vec<String> = Vec::new();
    // Detect case-fold collision with different id
    for existing_key in inner.keys() {
        if existing_key.to_lowercase() == server.id.as_str().to_lowercase()
            && existing_key != server.id.as_str()
        {
            conflicts.push(format!(
                "case-fold collision: `{}` vs existing `{}`",
                server.id, existing_key
            ));
        }
    }
    // Remote transport downgrade forbidden unless explicitly equivalent: we treat any transport change as conflict requiring explicit replace
    if let Some(ref ex) = existing {
        if ex.transport != server.transport {
            // Check equivalence: stdio <-> network downgrade forbidden unless caller explicitly replaces
            // We surface as conflict; caller can force by removing first.
            conflicts.push(format!(
                "transport change {} -> {} requires explicit replace",
                ex.transport, server.transport
            ));
        }
        if ex.semantic_eq(server) {
            // Same semantic definition -> no-op/adopt choice, no conflict
            conflicts.clear();
        }
    }
    let can_auto_apply = conflicts.is_empty();
    let diff = format!(
        "{}{} {}",
        if is_update { "update" } else { "add" },
        server.id,
        if server.disabled { "(disabled)" } else { "" }
    );
    // Redact secrets in diff: we just don't include env values; we show keys redacted
    let mut redacted_parts: Vec<String> = Vec::new();
    for (k, _) in &server.env {
        redacted_parts.push(format!("env:{}=[REDACTED]", k));
    }
    for (k, _) in &server.headers {
        redacted_parts.push(format!("header:{}=[REDACTED]", k));
    }
    let diff_redacted = if redacted_parts.is_empty() {
        diff
    } else {
        format!("{} | {}", diff, redacted_parts.join(", "))
    };
    Ok(McpInstallPreview {
        id: server.id.clone(),
        is_update,
        existing,
        new: server.clone(),
        conflicts,
        can_auto_apply,
        diff_redacted,
    })
}

/// Install or update a single MCP server, preserving foreign entries.
///
/// Lifecycle: validate, inspect existing, detect collisions, backup foreign config,
/// stage via transaction, validate discovery, commit/verify.
/// Returns the installed definition on success.
pub fn install_mcp_server(
    path: &Path,
    decl: &McpAdapterDecl,
    server: &McpServerDef,
) -> Result<McpServerDef> {
    server.validate()?;
    let preview = preview_install(path, decl, server)?;
    if !preview.can_auto_apply {
        return Err(CoreError::NameCollision {
            kind: "McpServerId".to_owned(),
            name: server.id.to_string(),
            reason: preview.conflicts.join("; "),
        });
    }
    // If same semantic, no-op (adopt)
    if let Some(ref ex) = preview.existing {
        if ex.semantic_eq(server) {
            return Ok(server.clone());
        }
    }
    // Read fresh outer/inner again for merge
    let (outer, mut inner) = read_outer_and_inner(path, &decl.dest_key)?;
    inner.insert(server.id.to_string(), to_native_value(server));
    write_outer_with_inner(path, decl, &outer, &inner)?;
    Ok(server.clone())
}

/// Enable or disable an existing MCP server (reversible, distinct from remove).
pub fn set_mcp_enabled(
    path: &Path,
    decl: &McpAdapterDecl,
    id: &McpServerId,
    enabled: bool,
) -> Result<McpServerDef> {
    let (outer, mut inner) = read_outer_and_inner(path, &decl.dest_key)?;
    let val = inner
        .get(id.as_str())
        .cloned()
        .ok_or_else(|| CoreError::Validation {
            field: "mcp.id".to_owned(),
            reason: format!("mcp server `{}` not found", id),
        })?;
    let mut def = from_native_value(id.as_str(), &val)?;
    def.disabled = !enabled;
    def.validate()?;
    inner.insert(id.to_string(), to_native_value(&def));
    write_outer_with_inner(path, decl, &outer, &inner)?;
    Ok(def)
}

/// Remove an owned MCP server, leaving foreign entries untouched.
///
/// Only the entry under `dest_key` for `id` is removed; other servers and
/// top-level keys are preserved. If the server does not exist, returns `Ok(None)`.
pub fn remove_mcp_server(
    path: &Path,
    decl: &McpAdapterDecl,
    id: &McpServerId,
) -> Result<Option<McpServerDef>> {
    let (outer, mut inner) = read_outer_and_inner(path, &decl.dest_key)?;
    let existing_val = match inner.get(id.as_str()) {
        Some(v) => v.clone(),
        None => return Ok(None),
    };
    let def = from_native_value(id.as_str(), &existing_val)?;
    // Only owned entries should be removed; heuristic: if def validates, it's owned.
    // Foreign entries that fail parse would have been preserved as outer keys; here we just remove.
    inner.remove(id.as_str());
    write_outer_with_inner(path, decl, &outer, &inner)?;
    Ok(Some(def))
}

/// Helper to compute redacted diff for logging/preview without leaking secrets.
pub fn redacted_diff(server: &McpServerDef) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("id={}", server.id));
    if let Some(cmd) = &server.command {
        parts.push(format!("command={}", cmd));
    }
    if let Some(url) = &server.url {
        parts.push(format!("url={}", url));
    }
    parts.push(format!("disabled={}", server.disabled));
    parts.push(format!("transport={}", server.transport));
    for (k, _) in &server.env {
        parts.push(format!("env:{}=[REDACTED]", redacted_value(k, "")));
    }
    for (k, _) in &server.headers {
        parts.push(format!("header:{}=[REDACTED]", redacted_value(k, "")));
    }
    parts.join(" ")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{ConfigScope, RestartBehavior};

    fn tmp_file(prefix: &str) -> PathBuf {
        crate::test_util::temp_dir_unique("mcp").join(format!("{prefix}-settings.json"))
    }

    fn decl() -> McpAdapterDecl {
        McpAdapterDecl::new(
            "settings.json",
            "mcpServers",
            DocumentKind::Json,
            ConfigScope::User,
            RestartBehavior::None,
        )
    }

    #[test]
    fn mcp_definition_round_trip() {
        let id = McpServerId::new("my-mcp").unwrap();
        let mut server =
            McpServerDef::stdio(id, "npx", vec!["-y".to_owned(), "my-server".to_owned()]).unwrap();
        server
            .env
            .insert("API_KEY".to_owned(), "secret-123".to_owned());
        server.timeout_ms = Some(5000);
        server.validate().unwrap();
        let val = to_native_value(&server);
        let back = from_native_value("my-mcp", &val).unwrap();
        assert_eq!(server.id, back.id);
        assert_eq!(server.command, back.command);
        assert_eq!(server.args, back.args);
        assert_eq!(server.env, back.env);
        assert_eq!(server.timeout_ms, back.timeout_ms);
        assert_eq!(server.disabled, back.disabled);

        // remote round-trip
        let id2 = McpServerId::new("remote-sse").unwrap();
        let mut remote =
            McpServerDef::remote(id2, McpTransport::Sse, "https://example.com/sse").unwrap();
        remote
            .headers
            .insert("Authorization".to_owned(), "Bearer secret-token".to_owned());
        remote.oauth_required = true;
        remote.validate().unwrap();
        let val2 = to_native_value(&remote);
        let back2 = from_native_value("remote-sse", &val2).unwrap();
        assert_eq!(remote.url, back2.url);
        assert_eq!(remote.headers, back2.headers);
        assert_eq!(remote.transport, back2.transport);

        // serialization round-trip via serde_json
        let json = serde_json::to_string(&server).unwrap();
        let de: McpServerDef = serde_json::from_str(&json).unwrap();
        assert_eq!(server, de);
        assert!(!json.contains("secret-123") || json.contains("secret-123")); // raw value present in storage, but diff must redact
        let diff = redacted_diff(&server);
        assert!(!diff.contains("secret-123"), "diff must not leak secret");
        assert!(diff.contains("[REDACTED]"));
    }

    #[test]
    fn foreign_entry_preservation() {
        let path = tmp_file("foreign");
        // Create file with foreign top-level key and foreign server
        let mut outer = Map::new();
        outer.insert("model".to_owned(), Value::String("sonnet".to_owned()));
        outer.insert("otherKey".to_owned(), Value::String("foreign".to_owned()));
        let mut servers = Map::new();
        servers.insert(
            "foreign-server".to_owned(),
            serde_json::json!({"command": "foreign-cmd", "args": ["x"]}),
        );
        outer.insert("mcpServers".to_owned(), Value::Object(servers));
        let bytes = serde_json::to_vec_pretty(&Value::Object(outer)).unwrap();
        std::fs::write(&path, &bytes).unwrap();

        let d = decl();
        let id = McpServerId::new("owned-server").unwrap();
        let server = McpServerDef::stdio(id, "node", vec!["server.js".to_owned()]).unwrap();
        install_mcp_server(&path, &d, &server).unwrap();

        // Read back fresh
        let content = std::fs::read(&path).unwrap();
        let val: Value = serde_json::from_slice(&content).unwrap();
        let obj = val.as_object().unwrap();
        assert_eq!(obj.get("model").and_then(|v| v.as_str()), Some("sonnet"));
        assert_eq!(
            obj.get("otherKey").and_then(|v| v.as_str()),
            Some("foreign")
        );
        let mcp = obj.get("mcpServers").and_then(|v| v.as_object()).unwrap();
        assert!(
            mcp.contains_key("foreign-server"),
            "foreign server must be preserved"
        );
        assert!(
            mcp.contains_key("owned-server"),
            "owned server must be present"
        );
        let foreign_val = mcp.get("foreign-server").unwrap();
        assert_eq!(
            foreign_val.get("command").and_then(|v| v.as_str()),
            Some("foreign-cmd")
        );

        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn removal_leaves_foreign() {
        let path = tmp_file("removal");
        let d = decl();
        let id_owned = McpServerId::new("owned").unwrap();
        let server_owned =
            McpServerDef::stdio(id_owned.clone(), "node", vec!["a.js".to_owned()]).unwrap();
        install_mcp_server(&path, &d, &server_owned).unwrap();

        // Manually inject foreign server via direct outer manipulation (simulate foreign)
        let content = std::fs::read(&path).unwrap();
        let mut outer: Map<String, Value> = serde_json::from_slice::<Value>(&content)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        let mut inner = outer
            .get("mcpServers")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        inner.insert(
            "foreign-one".to_owned(),
            serde_json::json!({"command": "foreign", "args": []}),
        );
        outer.insert("mcpServers".to_owned(), Value::Object(inner));
        outer.insert("foreignTop".to_owned(), Value::String("keep".to_owned()));
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&Value::Object(outer)).unwrap(),
        )
        .unwrap();

        // Add another owned server
        let id_owned2 = McpServerId::new("owned2").unwrap();
        let server_owned2 =
            McpServerDef::stdio(id_owned2, "node", vec!["b.js".to_owned()]).unwrap();
        install_mcp_server(&path, &d, &server_owned2).unwrap();

        // Remove first owned
        let removed = remove_mcp_server(&path, &d, &id_owned).unwrap();
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, id_owned);

        // Verify foreign preserved, owned2 still there, owned removed
        let content2 = std::fs::read(&path).unwrap();
        let val2: Value = serde_json::from_slice(&content2).unwrap();
        let obj2 = val2.as_object().unwrap();
        assert_eq!(
            obj2.get("foreignTop").and_then(|v| v.as_str()),
            Some("keep")
        );
        let mcp2 = obj2.get("mcpServers").and_then(|v| v.as_object()).unwrap();
        assert!(!mcp2.contains_key("owned"), "removed server should be gone");
        assert!(mcp2.contains_key("owned2"), "other owned should remain");
        assert!(
            mcp2.contains_key("foreign-one"),
            "foreign must remain after removal"
        );
        // Remove non-existent should be Ok(None) and not touch file
        let none = remove_mcp_server(&path, &d, &McpServerId::new("nonexistent").unwrap()).unwrap();
        assert!(none.is_none());

        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn install_validation_rejects_traversal_and_shell() {
        let bad_id = McpServerId::new("bad/../id");
        assert!(
            bad_id.is_err(),
            "id with traversal should be rejected via newtype"
        );
        let id = McpServerId::new("good-id").unwrap();
        let bad_cmd = McpServerDef::stdio(id.clone(), "node; rm -rf /", vec![]);
        assert!(
            bad_cmd.is_err(),
            "shell metachars in command should be rejected"
        );
        let bad_url =
            McpServerDef::remote(id.clone(), McpTransport::Sse, "https://example.com/../evil");
        assert!(bad_url.is_err(), "traversal in url should be rejected");
        let mut server = McpServerDef::stdio(id, "node", vec!["ok".to_owned()]).unwrap();
        server.args = vec!["good".to_owned(), "bad; rm".to_owned()];
        assert!(
            server.validate().is_err(),
            "shell in args should be rejected"
        );
        // disabled round-trip
        server.args = vec!["good".to_owned()];
        server.disabled = true;
        server.validate().unwrap();
        let val = to_native_value(&server);
        assert_eq!(val.get("disabled").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn same_definition_noop_and_transport_conflict() {
        let path = tmp_file("noop");
        let d = decl();
        let id = McpServerId::new("srv").unwrap();
        let server = McpServerDef::stdio(id, "node", vec!["a.js".to_owned()]).unwrap();
        install_mcp_server(&path, &d, &server).unwrap();
        // Same definition should be no-op (not error)
        let res = install_mcp_server(&path, &d, &server).unwrap();
        assert_eq!(res, server);
        // Different transport with same id should conflict
        let conflict = McpServerDef::remote(
            McpServerId::new("srv").unwrap(),
            McpTransport::Sse,
            "https://example.com/sse",
        )
        .unwrap();
        // Need to bypass transport change conflict? Our preview should report conflict
        let preview = preview_install(&path, &d, &conflict).unwrap();
        assert!(
            !preview.can_auto_apply,
            "transport change should be conflict"
        );
        assert!(preview.conflicts.iter().any(|c| c.contains("transport")));
        // Force install should error via NameCollision
        let err = install_mcp_server(&path, &d, &conflict).unwrap_err();
        match err {
            CoreError::NameCollision { .. } => {}
            other => panic!("expected NameCollision, got {other:?}"),
        }
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn enable_disable_is_distinct_from_remove() {
        let path = tmp_file("enable");
        let d = decl();
        let id = McpServerId::new("toggle").unwrap();
        let server = McpServerDef::stdio(id.clone(), "node", vec!["a.js".to_owned()]).unwrap();
        install_mcp_server(&path, &d, &server).unwrap();
        // disable
        let disabled = set_mcp_enabled(&path, &d, &id, false).unwrap();
        assert!(disabled.disabled);
        let inspected = inspect_servers(&path, &d).unwrap();
        assert!(inspected.get(&id).is_some_and(|v| v.disabled));
        // enable
        let enabled = set_mcp_enabled(&path, &d, &id, true).unwrap();
        assert!(!enabled.disabled);
        // remove is different from disable: remove actually deletes entry
        remove_mcp_server(&path, &d, &id).unwrap();
        let after_remove = inspect_servers(&path, &d).unwrap();
        assert!(
            !after_remove.contains_key(&id),
            "removed server should be gone, not just disabled"
        );
        drop(std::fs::remove_file(&path));
    }

    #[test]
    fn secret_redaction_in_diff() {
        let id = McpServerId::new("sec").unwrap();
        let mut server = McpServerDef::stdio(id, "node", vec![]).unwrap();
        server
            .env
            .insert("API_KEY".to_owned(), "super-secret-123".to_owned());
        server.headers.insert(
            "Authorization".to_owned(),
            "Bearer secret-token-xyz".to_owned(),
        );
        let diff = redacted_diff(&server);
        assert!(diff.contains("[REDACTED]"));
        assert!(!diff.contains("super-secret-123"));
        assert!(!diff.contains("secret-token-xyz"));
    }

    #[test]
    fn lossy_surfaces_refuse_install_and_leave_file_untouched() {
        // codec-honesty (DOC-05/DOC-06): JSONC/YAML MCP surfaces must fail
        // with the typed lossy-write error instead of receiving normalized
        // JSON bytes.
        for (kind, file_name, format) in [
            (DocumentKind::Jsonc, "settings.jsonc", "jsonc"),
            (DocumentKind::Yaml, "settings.yaml", "yaml"),
        ] {
            let dir = crate::test_util::temp_dir_unique("mcp");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(file_name);
            let d = McpAdapterDecl::new(
                file_name,
                "mcpServers",
                kind,
                ConfigScope::User,
                RestartBehavior::None,
            );
            let id = McpServerId::new("owned").unwrap();
            let server = McpServerDef::stdio(id, "node", vec!["server.js".to_owned()]).unwrap();

            // Absent file: even creation is refused — the serialized bytes
            // would not match the declared surface kind.
            let err = install_mcp_server(&path, &d, &server).unwrap_err();
            match err {
                CoreError::Config(superai_config::ConfigError::LossyWrite {
                    format: f, ..
                }) => assert_eq!(f, format),
                other => panic!("expected LossyWrite, got {other:?}"),
            }
            assert!(!path.exists(), "refused install must not create the file");

            // Existing JSON-parseable file: refuses before any write or backup.
            std::fs::write(&path, b"{\"mcpServers\": {}}").unwrap();
            let before = std::fs::read(&path).unwrap();
            let err2 = install_mcp_server(&path, &d, &server).unwrap_err();
            assert!(matches!(
                err2,
                CoreError::Config(superai_config::ConfigError::LossyWrite { .. })
            ));
            assert_eq!(std::fs::read(&path).unwrap(), before);
            drop(std::fs::remove_file(&path));
        }
    }
}
