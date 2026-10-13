//! Remote layout plus the pure renders the deploy plan previews: the
//! hardened systemd unit, the nginx TLS site (toride-proxy), the wireguard
//! interface conf (toride-wireguard), and credential redaction for previews.

use toride_proxy::render::render_nginx_server_block;
use toride_proxy::spec::{ServerBlock, TlsConfig};
use toride_wireguard::render::render_interface_conf;
use toride_wireguard::spec::{PeerSpec, WireguardSpec};

use crate::vps::HostKeyPolicy;

/// The only service this slice deploys.
pub(crate) const SERVICE_NAME: &str = "llmproxy";
/// Remote binary destination (musl static build).
pub(crate) const REMOTE_BIN_PATH: &str = "/usr/local/lib/llmproxy/llmproxy";
/// Remote configuration root, root-owned.
pub(crate) const REMOTE_CONF_DIR: &str = "/etc/llmproxy";
/// Remote secrets env file the unit loads.
pub(crate) const REMOTE_ENV_PATH: &str = "/etc/llmproxy/llmproxy.env";
/// Remote provider TOML directory.
pub(crate) const REMOTE_PROVIDERS_DIR: &str = "/etc/llmproxy/providers";
/// Remote unit path derived from the service name.
pub(crate) const REMOTE_UNIT_PATH: &str = "/etc/systemd/system/llmproxy.service";
/// Remote nginx site for TLS ingress.
pub(crate) const REMOTE_NGINX_SITE_PATH: &str = "/etc/nginx/sites-available/llmproxy.conf";
/// Remote wireguard interface conf (tunnel mode).
pub(crate) const REMOTE_WG_CONF_PATH: &str = "/etc/wireguard/wg0.conf";
/// Remote superai ownership manifest; lists paths this tool created.
pub(crate) const REMOTE_MANIFEST_PATH: &str = "/etc/llmproxy/superai-manifest.json";

/// Certificate directory certbot provisions for a TLS domain.
pub(crate) fn cert_dir(domain: &str) -> String {
    format!("/etc/letsencrypt/live/{domain}")
}

/// Wireguard interface name the tunnel mode manages.
pub(crate) const WG_INTERFACE: &str = "wg0";
/// Wireguard UDP listen port on the VPS.
pub(crate) const WG_LISTEN_PORT: u16 = 51_820;
/// Default VPS-side tunnel address (CIDR on the wireguard interface).
pub(crate) const WG_SERVER_ADDRESS: &str = "10.7.0.1/24";
/// Workstation-side tunnel address baked into the plan notes.
pub(crate) const WG_CLIENT_TUNNEL_ADDRESS: &str = "10.7.0.2";
/// Disk floor the preflight demands on `/`.
pub(crate) const MIN_DISK_FREE_BYTES: u64 = 512 * 1024 * 1024;
/// The local proxy listener nginx forwards to.
pub(crate) const PROXY_UPSTREAM: &str = "127.0.0.1:8080";

/// The hardened unit (VPS-02 item 2): dynamic user, no new privileges, a
/// strict read-only system surface, and one writable state directory.
pub(crate) fn render_systemd_unit() -> String {
    format!(
        "[Unit]\n\
         Description=llmproxy usage ledger (superai-managed)\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={REMOTE_BIN_PATH} --config {REMOTE_CONF_DIR}\n\
         EnvironmentFile={REMOTE_ENV_PATH}\n\
         DynamicUser=yes\n\
         NoNewPrivileges=yes\n\
         ProtectSystem=strict\n\
         ProtectHome=yes\n\
         PrivateTmp=yes\n\
         PrivateDevices=yes\n\
         ProtectKernelTunables=yes\n\
         ProtectKernelModules=yes\n\
         ProtectControlGroups=yes\n\
         ProtectClock=yes\n\
         ProtectHostname=yes\n\
         RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6\n\
         RestrictNamespaces=yes\n\
         RestrictRealtime=yes\n\
         LockPersonality=yes\n\
         MemoryDenyWriteExecute=yes\n\
         SystemCallFilter=@system-service\n\
         SystemCallErrorNumber=EPERM\n\
         CapabilityBoundingSet=\n\
         AmbientCapabilities=\n\
         StateDirectory=llmproxy\n\
         WorkingDirectory=/var/lib/llmproxy\n\
         Restart=on-failure\n\
         RestartSec=2s\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// The nginx TLS site terminating for `domain` and forwarding to the local
/// proxy listener; rendered through toride-proxy's pure renderer.
pub(crate) fn render_nginx_site(domain: &str) -> String {
    let dir = cert_dir(domain);
    let tls = TlsConfig::new(
        domain,
        format!("{dir}/fullchain.pem"),
        format!("{dir}/privkey.pem"),
    );
    let block = ServerBlock::new(domain, 443, PROXY_UPSTREAM)
        .with_tls(tls)
        .with_directive("location /.well-known/acme-challenge/ { root /var/www/html; }".to_owned());
    render_nginx_server_block(&block)
}

/// The VPS-side wireguard interface conf. `server_private_key` is a secret:
/// pass `None` for previews and the rendered conf omits the key line.
pub(crate) fn render_wireguard_conf(
    server_tunnel_address: &str,
    client_public_key: &str,
    server_private_key: Option<&str>,
) -> String {
    let peer = PeerSpec::new(
        client_public_key.to_owned(),
        vec![format!("{WG_CLIENT_TUNNEL_ADDRESS}/32")],
    )
    .with_persistent_keepalive(25);
    let mut spec = WireguardSpec::new(WG_INTERFACE.to_owned(), server_tunnel_address.to_owned())
        .with_listen_port(WG_LISTEN_PORT)
        .with_peer(peer);
    if let Some(key) = server_private_key {
        spec = spec.with_private_key(key.to_owned());
    }
    render_interface_conf(&spec)
}

/// Key-name markers whose values are redacted from every preview.
const SECRET_KEY_MARKERS: &[&str] = &["key", "token", "secret", "password", "credential"];

/// Redact `key = value` / `KEY=value` lines whose key names a credential.
/// Over-redaction is fine; leaking a provider key is not.
pub(crate) fn redact_sensitive_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        out.push_str(&redact_line(line));
        out.push('\n');
    }
    out
}

fn redact_line(line: &str) -> String {
    let Some(key) = setting_key(line) else {
        return line.to_owned();
    };
    let lowered = key.to_ascii_lowercase();
    if SECRET_KEY_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
    {
        format!("{key}=[REDACTED]")
    } else {
        line.to_owned()
    }
}

/// The trimmed key of a `key = value` line, when it looks like a setting.
fn setting_key(line: &str) -> Option<&str> {
    let (key, _) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(key)
}

/// The ownership manifest body: the paths superai created, the ingress mode
/// and the address status should report. No secrets, ever.
pub(crate) fn render_manifest(mode: &str, address: &str, paths: &[String]) -> String {
    let listed: Vec<String> = paths
        .iter()
        .map(|path| serde_json::to_string(path).unwrap_or_default())
        .collect();
    let address = serde_json::to_string(address).unwrap_or_default();
    format!(
        "{{\"managed_by\":\"superai\",\"service\":\"{SERVICE_NAME}\",\"mode\":\"{mode}\",\
         \"address\":{address},\"paths\":[{}]}}\n",
        listed.join(",")
    )
}

/// Host-key policy line for plan echoes.
pub(crate) fn policy_note(policy: HostKeyPolicy) -> String {
    match policy {
        HostKeyPolicy::Strict => "host keys: strict known-hosts".to_owned(),
        HostKeyPolicy::AcceptNew => "host keys: accept-new".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_render_pins_the_hardened_directives() {
        let unit = render_systemd_unit();
        for needle in [
            "DynamicUser=yes",
            "NoNewPrivileges=yes",
            "ProtectSystem=strict",
            "ProtectHome=yes",
            "PrivateTmp=yes",
            "SystemCallFilter=@system-service",
            "CapabilityBoundingSet=",
            "StateDirectory=llmproxy",
            &format!("ExecStart={REMOTE_BIN_PATH}"),
            &format!("EnvironmentFile={REMOTE_ENV_PATH}"),
        ] {
            assert!(unit.contains(needle), "unit must contain `{needle}`");
        }
    }

    #[test]
    fn nginx_render_terminates_tls_and_forwards_locally() {
        let site = render_nginx_site("proxy.example.com");
        assert!(site.contains("listen 443 ssl"), "{site}");
        assert!(site.contains("server_name proxy.example.com;"), "{site}");
        assert!(
            site.contains("ssl_certificate /etc/letsencrypt/live/proxy.example.com/fullchain.pem;"),
            "{site}"
        );
        assert!(
            site.contains(&format!("proxy_pass http://{PROXY_UPSTREAM};")),
            "{site}"
        );
        assert!(site.contains("acme-challenge"), "{site}");
    }

    #[test]
    fn wireguard_render_carries_the_peer_but_hides_the_key_when_asked() {
        let public = render_wireguard_conf(WG_SERVER_ADDRESS, "pub-key-material", None);
        assert!(public.contains("[Interface]"), "{public}");
        assert!(public.contains("ListenPort = 51820"), "{public}");
        assert!(public.contains("[Peer]"), "{public}");
        assert!(public.contains("PublicKey = pub-key-material"), "{public}");
        assert!(!public.contains("PrivateKey"), "preview must omit the key");

        let private = render_wireguard_conf(
            WG_SERVER_ADDRESS,
            "pub-key-material",
            Some("server-secret-key"),
        );
        assert!(private.contains("PrivateKey = server-secret-key"));
    }

    #[test]
    fn redaction_covers_credential_named_settings_and_keeps_the_rest() {
        let toml =
            "model = \"gpt-5\"\napi_key = \"sk-live-abcdef\"\nAUTH_TOKEN=x\nhost = \"a=b\"\n";
        let redacted = redact_sensitive_lines(toml);
        assert!(redacted.contains("model = \"gpt-5\""), "{redacted}");
        assert!(redacted.contains("api_key=[REDACTED]"), "{redacted}");
        assert!(redacted.contains("AUTH_TOKEN=[REDACTED]"), "{redacted}");
        assert!(!redacted.contains("abcdef"), "{redacted}");
        assert!(redacted.contains("host = \"a=b\""), "{redacted}");
    }

    #[test]
    fn manifest_render_lists_paths_and_mode() {
        let manifest = render_manifest(
            "tls",
            "proxy.example.com",
            &[REMOTE_BIN_PATH.to_owned(), REMOTE_ENV_PATH.to_owned()],
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&manifest).unwrap_or_else(|e| panic!("manifest json: {e}"));
        assert_eq!(parsed["managed_by"], "superai");
        assert_eq!(parsed["mode"], "tls");
        assert_eq!(parsed["address"], "proxy.example.com");
        assert_eq!(parsed["paths"][0], REMOTE_BIN_PATH);
        assert_eq!(parsed["paths"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn split_setting_ignores_non_setting_lines() {
        assert_eq!(setting_key("plain line"), None);
        assert_eq!(setting_key("=value"), None);
        assert_eq!(setting_key("[section]"), None);
        assert_eq!(setting_key("api_key = \"x\""), Some("api_key"));
    }
}
