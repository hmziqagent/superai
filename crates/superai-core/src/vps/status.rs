//! Read-only VPS status (VPS-03): service state, disk, load, tunnel address,
//! and cert expiry parsed from the remote openssl answer. The ledger probes
//! live one layer up, in the CLI, through the llmproxy client.

use toride_proxy::certs_parse::{days_until, parse_openssl_enddate_epoch};

use crate::vps::plan::{parse_disk_available, validate_host};
use crate::vps::render::{REMOTE_MANIFEST_PATH, SERVICE_NAME, cert_dir};
use crate::vps::{RemoteHost, VpsError};

/// Service state as status reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceView {
    /// Unit name.
    pub name: String,
    /// Whether systemd reports it active.
    pub active: bool,
}

/// Certificate expiry parsed from remote cert data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertView {
    /// Domain the certificate covers.
    pub domain: String,
    /// The `notAfter` string the host reported.
    pub not_after: String,
    /// Whole days until expiry; negative once expired.
    pub days_remaining: i64,
}

/// Everything `superai vps status` reports; all read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpsStatusReport {
    /// Target host.
    pub host: String,
    /// Backend the facts came from.
    pub backend: String,
    /// Service state.
    pub service: ServiceView,
    /// Bytes free on `/`, when `df` answered.
    pub disk_free_bytes: Option<u64>,
    /// `/proc/loadavg` first field, when it answered.
    pub loadavg: Option<String>,
    /// Deploy mode recorded in the manifest, when present.
    pub mode: Option<String>,
    /// Address harnesses should point at (domain or tunnel address).
    pub address: Option<String>,
    /// Cert expiry for TLS deployments.
    pub cert: Option<CertView>,
    /// Degrading observations; never secrets.
    pub notes: Vec<String>,
}

/// Collect the status facts from `remote`. Read-only: no uploads, no exec
/// that mutates, no service verbs.
pub fn remote_status(host: &str, remote: &dyn RemoteHost) -> Result<VpsStatusReport, VpsError> {
    validate_host(host)?;
    let mut notes = Vec::new();
    let service = ServiceView {
        name: SERVICE_NAME.to_owned(),
        active: remote.service_is_active(SERVICE_NAME)?,
    };
    let disk_free_bytes = match remote.exec(&["df", "-B1", "/"]) {
        Ok(output) if output.success() => parse_disk_available(&output.stdout),
        Ok(_) => {
            notes.push("df did not answer; disk unknown".to_owned());
            None
        }
        Err(error) => {
            notes.push(format!("disk probe failed: {error}"));
            None
        }
    };
    let loadavg = match remote.exec(&["cat", "/proc/loadavg"]) {
        Ok(output) if output.success() => {
            output.stdout.split_whitespace().next().map(str::to_owned)
        }
        Ok(_) => {
            notes.push("loadavg unreadable".to_owned());
            None
        }
        Err(error) => {
            notes.push(format!("load probe failed: {error}"));
            None
        }
    };
    let manifest = read_manifest(remote)?;
    let mode = manifest.as_ref().map(|value| echo(value, "mode"));
    let address = manifest.as_ref().map(|value| echo(value, "address"));
    let cert = match (&manifest, &address) {
        (Some(_), Some(domain))
            if manifest
                .as_ref()
                .is_some_and(|value| echo(value, "mode") == "tls") =>
        {
            let probed = cert_expiry(remote, domain)?;
            if probed.is_none() {
                notes.push(format!("certificate for {domain} unreadable"));
            }
            probed
        }
        _ => None,
    };
    Ok(VpsStatusReport {
        host: host.to_owned(),
        backend: remote.describe(),
        service,
        disk_free_bytes,
        loadavg,
        mode,
        address,
        cert,
        notes,
    })
}

fn echo(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn read_manifest(remote: &dyn RemoteHost) -> Result<Option<serde_json::Value>, VpsError> {
    match remote.read_file(REMOTE_MANIFEST_PATH)? {
        None => Ok(None),
        Some(bytes) => {
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| VpsError::RemoteState {
                    path: REMOTE_MANIFEST_PATH.to_owned(),
                    reason: format!("manifest unparseable: {e}"),
                })
        }
    }
}

/// Parse the remote `openssl x509 -enddate` answer into a [`CertView`].
fn cert_expiry(remote: &dyn RemoteHost, domain: &str) -> Result<Option<CertView>, VpsError> {
    let cert_path = format!("{}/fullchain.pem", cert_dir(domain));
    let output = remote.exec(&["openssl", "x509", "-in", &cert_path, "-noout", "-enddate"])?;
    if !output.success() {
        return Ok(None);
    }
    let Some(not_after) = output
        .stdout
        .trim()
        .strip_prefix("enddate=")
        .map(str::to_owned)
    else {
        return Ok(None);
    };
    let Some(epoch) = parse_openssl_enddate_epoch(&not_after) else {
        return Ok(None);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    Ok(Some(CertView {
        domain: domain.to_owned(),
        not_after,
        days_remaining: days_until(now, epoch),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vps::execute::{UpgradeRequest, execute_deploy};
    use crate::vps::plan::{DeployRequest, IngressMode, ProviderToml, plan_deploy};
    use crate::vps::remote::FixtureRemote;

    fn tls_request(dir: &std::path::Path) -> DeployRequest {
        let binary = dir.join("llmproxy");
        std::fs::write(&binary, b"musl-bytes").unwrap();
        let env_file = dir.join("secrets.env");
        std::fs::write(&env_file, "LLMPROXY_ADMIN_TOKEN=x\n").unwrap();
        DeployRequest {
            point_instances: Vec::new(),
            host: "root@203.0.113.10".to_owned(),
            ingress: IngressMode::Tls {
                domain: "proxy.example.com".to_owned(),
                email: "ops@example.com".to_owned(),
            },
            geo_exit: None,
            ack_geo_exit_proxy: false,
            binary_path: binary,
            providers: vec![ProviderToml {
                name: "anthropic".to_owned(),
                content: "api_key = \"PLACEHOLDER\"\n".to_owned(),
            }],
            env_file: Some(env_file),
            policy: crate::vps::HostKeyPolicy::Strict,
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("superai-vps-status-{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn status_reports_a_deployed_tls_host_with_cert_expiry() {
        let dir = temp_dir("tls");
        let remote = FixtureRemote::new("root@203.0.113.10");
        let plan = plan_deploy(&tls_request(&dir), &remote).unwrap();
        execute_deploy(&plan, &remote).unwrap();

        let report = remote_status("root@203.0.113.10", &remote).unwrap();
        assert!(report.service.active);
        assert!(report.disk_free_bytes.is_some_and(|bytes| bytes > 0));
        assert!(report.loadavg.is_some());
        assert_eq!(report.mode.as_deref(), Some("tls"));
        assert_eq!(report.address.as_deref(), Some("proxy.example.com"));
        let cert = report
            .cert
            .clone()
            .unwrap_or_else(|| panic!("cert: {report:?}"));
        assert_eq!(cert.domain, "proxy.example.com");
        let expected = {
            let epoch = parse_openssl_enddate_epoch(crate::vps::remote::FIXTURE_CERT_NOT_AFTER)
                .unwrap_or(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_secs());
            days_until(now, epoch)
        };
        assert_eq!(cert.days_remaining, expected);
        assert!(cert.days_remaining > 0);
    }

    #[test]
    fn status_on_an_undeployed_host_is_reported_not_failed() {
        let remote = FixtureRemote::new("root@host");
        let report = remote_status("root@host", &remote).unwrap();
        assert!(!report.service.active);
        assert_eq!(report.mode, None);
        assert_eq!(report.cert, None);
    }

    #[test]
    fn status_refuses_a_bad_host() {
        let remote = FixtureRemote::new("root@host");
        assert!(remote_status("host", &remote).is_err(), "bad host");
    }

    #[test]
    fn upgrade_then_status_still_reports_tls_facts() {
        let dir = temp_dir("upgrade-status");
        let remote = FixtureRemote::new("root@host");
        let plan = plan_deploy(&tls_request(&dir), &remote).unwrap();
        execute_deploy(&plan, &remote).unwrap();
        let new_binary = dir.join("llmproxy-v2");
        std::fs::write(&new_binary, b"musl-v2").unwrap();
        let upgrade = crate::vps::execute::plan_upgrade(
            &UpgradeRequest {
                host: "root@203.0.113.10".to_owned(),
                binary_path: new_binary,
                policy: crate::vps::HostKeyPolicy::Strict,
            },
            &remote,
        )
        .unwrap();
        crate::vps::execute::execute_upgrade(&upgrade, &remote).unwrap();
        let report = remote_status("root@203.0.113.10", &remote).unwrap();
        assert!(report.service.active);
        assert_eq!(report.mode.as_deref(), Some("tls"));
    }
}
