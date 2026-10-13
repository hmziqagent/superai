//! Deploy planning (VPS-02): request validation, preflight probes, and a
//! stepwise plan previewed with diffs against the live remote tree.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::vps::remote::{HostKeyPolicy, RemoteHost, ensure_exec_argv, ensure_remote_path};
use crate::vps::render::{
    MIN_DISK_FREE_BYTES, REMOTE_BIN_PATH, REMOTE_ENV_PATH, REMOTE_MANIFEST_PATH,
    REMOTE_NGINX_ENABLED_PATH, REMOTE_NGINX_SITE_PATH, REMOTE_PROVIDERS_DIR, REMOTE_UNIT_PATH,
    REMOTE_WG_CONF_PATH, SERVICE_NAME, WG_SERVER_ADDRESS, harness_base_url, policy_note,
    redact_sensitive_lines, render_manifest, render_nginx_site, render_systemd_unit,
    render_wireguard_conf,
};
use crate::vps::{VpsError, validation};

/// How the proxy is reached: public TLS or a private tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngressMode {
    /// nginx terminates TLS for a public domain (certbot/Let's Encrypt).
    Tls {
        /// Public domain users connect on.
        domain: String,
        /// ACME account email for certbot.
        email: String,
    },
    /// Nothing public; only tunnel peers reach the proxy.
    Tunnel(TunnelKind),
}

/// Private tunnel transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelKind {
    /// A wireguard interface on the VPS peered with the workstation.
    Wireguard {
        /// Workstation peer public key (base64).
        client_public_key: String,
    },
    /// A tailscale tailnet hostname on the VPS.
    Tailscale {
        /// Tailnet hostname to advertise.
        tailnet_host: String,
    },
}

/// One provider TOML for the remote config skeleton.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderToml {
    /// Provider id; becomes the remote filename stem.
    pub name: String,
    /// TOML body with secret placeholders, never secret values.
    pub content: String,
}

impl ProviderToml {
    fn carries_egress_table(&self) -> bool {
        self.content.lines().any(|line| line.trim() == "[egress]")
    }

    /// Content as deployed: the user TOML plus the egress table when a
    /// geo-exit forward proxy was requested.
    fn effective(&self, geo_exit: Option<&str>) -> String {
        match geo_exit {
            None => self.content.clone(),
            Some(address) => format!(
                "{}\n[egress]\nproxy = \"socks5://{address}\"\n",
                self.content.trim_end()
            ),
        }
    }
}

/// Everything a deploy needs, validated before any remote contact.
#[derive(Debug, Clone)]
pub struct DeployRequest {
    /// `user@host` ssh target.
    pub host: String,
    /// Public TLS or tunnel-only ingress.
    pub ingress: IngressMode,
    /// Optional forward proxy `host:port` reachable over the tunnel; adds
    /// `egress.proxy` entries to every provider TOML.
    pub geo_exit: Option<String>,
    /// Local musl binary to upload.
    pub binary_path: PathBuf,
    /// Provider TOMLs for the remote skeleton.
    pub providers: Vec<ProviderToml>,
    /// Local env file with the secrets the unit loads; uploaded 0600.
    pub env_file: Option<PathBuf>,
    /// Recorded instance names to point at the deployment address after a
    /// successful commit.
    pub point_instances: Vec<String>,
    /// Host-key acceptance policy for the ssh backend.
    pub policy: HostKeyPolicy,
}

/// One previewed remote mutation or command, redacted for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DeployStep {
    /// Write bytes to a remote path.
    UploadFile {
        /// Remote destination.
        path: String,
        /// Unix mode bits.
        mode: u32,
        /// Payload size in bytes.
        bytes: usize,
        /// Whether the payload is a secret (content never previewed).
        secret: bool,
    },
    /// Install the systemd unit (backup-first).
    InstallUnit {
        /// Unit name without extension.
        name: String,
    },
    /// Run `daemon-reload`.
    ReloadDaemons,
    /// Run one argv on the host.
    Exec {
        /// The exact argv, no shell.
        argv: Vec<String>,
    },
    /// Enable a service at boot.
    EnableService {
        /// Unit name without extension.
        name: String,
    },
    /// Start a service now.
    StartService {
        /// Unit name without extension.
        name: String,
    },
    /// Restart a running service (upgrade path).
    RestartService {
        /// Unit name without extension.
        name: String,
    },
    /// Create a symlink (nginx site enable).
    Symlink {
        /// What the link points at.
        target: String,
        /// Where the link is created.
        link: String,
    },
    /// Fill `${NAME}` placeholders in a remote file from the remote env
    /// file; secret values never transit this process.
    InterpolateEnv {
        /// File whose placeholders are filled in place (mode 0600).
        target: String,
        /// Remote env file carrying the values.
        env: String,
    },
}

/// What execute carries for a step; never serialized, never printed.
#[derive(Clone)]
pub(crate) enum Payload {
    /// Upload bytes with mode and secret flag.
    Upload {
        /// Exact bytes to write.
        bytes: Vec<u8>,
        /// Unix mode bits.
        mode: u32,
        /// Whether the payload is a secret.
        secret: bool,
    },
    /// Unit file body.
    Unit {
        /// Unit file content.
        content: String,
    },
    /// Nothing beyond the step itself.
    Bare,
}

impl std::fmt::Debug for Payload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Upload {
                bytes,
                mode,
                secret,
            } => f
                .debug_struct("Upload")
                .field("bytes", &bytes.len())
                .field("mode", mode)
                .field("secret", secret)
                .finish(),
            Self::Unit { content } => f
                .debug_struct("Unit")
                .field("content", &format!("{} lines", content.lines().count()))
                .finish(),
            Self::Bare => f.write_str("Bare"),
        }
    }
}

/// Who owned the remote file a step would replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Existing {
    /// A previous superai deploy created it (manifest says so).
    Ours,
    /// The user (or someone else) created it; it gets backed up first.
    Foreign,
}

/// The redacted preview for one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepPreview {
    /// Creates a file that is not there yet.
    Create {
        /// Remote path.
        path: String,
        /// Redacted content, `None` for secret payloads.
        content: Option<String>,
    },
    /// Replaces an existing file; the diff is over redacted lines.
    Replace {
        /// Remote path.
        path: String,
        /// Who owned the file being replaced.
        existing: Existing,
        /// `-old/+new` line diff, both sides redacted.
        diff: String,
        /// Redacted new content, `None` for secret payloads.
        content: Option<String>,
    },
    /// Installs the unit, backing up a pre-existing unit first.
    UnitInstall {
        /// Remote unit path.
        path: String,
        /// Whether a pre-existing unit will be backed up.
        backup_planned: bool,
        /// Line diff against the current unit, empty when absent.
        diff: String,
    },
    /// Runs one argv.
    Run {
        /// Exact argv.
        argv: Vec<String>,
    },
    /// Reloads the init system.
    DaemonsReload,
    /// Enable/start a service.
    Service {
        /// Unit name.
        name: String,
        /// Which service verb.
        verb: &'static str,
    },
    /// Creates a symlink pointing at an uploaded site.
    Symlink {
        /// What the link points at.
        target: String,
        /// Where the link is created.
        link: String,
    },
    /// Fills placeholders on the host; values never previewed.
    InterpolateEnv {
        /// File being interpolated.
        target: String,
        /// Remote env file carrying the values.
        env: String,
    },
}

/// A step paired with its payload and redacted preview.
#[derive(Debug, Clone)]
pub struct PlannedStep {
    /// The display-safe step.
    pub step: DeployStep,
    /// The redacted preview.
    pub preview: StepPreview,
    /// Bytes execute will commit.
    pub(crate) payload: Payload,
}

/// One preflight check outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckState {
    /// The check passed; the string is the observed detail.
    Pass(String),
    /// The check failed; the string is why.
    Fail(String),
}

/// A single named preflight check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightCheck {
    /// Check name, kebab-case.
    pub name: &'static str,
    /// Outcome.
    pub state: CheckState,
}

/// Preflight facts (VPS-02 item 1): reachability, host-key policy, OS,
/// systemd, disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightReport {
    /// Target the checks ran against.
    pub host: String,
    /// Backend description.
    pub backend: String,
    /// Host-key policy in force.
    pub policy: String,
    /// The checks, in run order.
    pub checks: Vec<PreflightCheck>,
}

impl PreflightReport {
    /// Whether any check failed.
    pub fn blocked(&self) -> bool {
        self.checks
            .iter()
            .any(|check| matches!(check.state, CheckState::Fail(_)))
    }

    /// First failure's text, for error messages.
    pub fn first_failure(&self) -> Option<String> {
        self.checks.iter().find_map(|check| match &check.state {
            CheckState::Fail(reason) => Some(format!("{}: {reason}", check.name)),
            CheckState::Pass(_) => None,
        })
    }
}

/// The full deploy plan: everything previewed, nothing mutated.
#[derive(Debug, Clone)]
pub struct DeployPlan {
    /// Target host.
    pub host: String,
    /// Backend that produced the previews.
    pub backend: String,
    /// Service being deployed.
    pub service: &'static str,
    /// Ingress mode echo: `tls`, `tunnel-wireguard`, `tunnel-tailscale`.
    pub mode: String,
    /// The base URL local harness configs get pointed at.
    pub harness_base: String,
    /// Recorded instances the commit step points at `harness_base`.
    pub point_instances: Vec<String>,
    /// Public domain or tunnel address harnesses point at.
    pub address: String,
    /// Geo-exit forward proxy, when configured.
    pub geo_exit: Option<String>,
    /// Preflight facts.
    pub preflight: PreflightReport,
    /// Steps in commit order.
    pub steps: Vec<PlannedStep>,
}

impl DeployPlan {
    /// Whether preflight blocks execution.
    pub fn blocked(&self) -> bool {
        self.preflight.blocked()
    }
}

/// Validate a `user@host` target: one `@`, no shell material, no NUL.
pub fn validate_host(host: &str) -> Result<(), VpsError> {
    let refuse = |reason: &str| Err(validation("host", reason.to_owned()));
    if host.trim() != host || host.is_empty() {
        return refuse("must be non-empty and unpadded");
    }
    if crate::process::shell_metachar_in(host).is_some() || host.contains('\0') {
        return refuse("must not contain shell metacharacters or NUL");
    }
    let Some((user, hostname)) = host.split_once('@') else {
        return refuse("must look like user@host");
    };
    if user.is_empty() || hostname.is_empty() || hostname.contains('@') || hostname.contains('/') {
        return refuse("must have a non-empty user and host");
    }
    Ok(())
}

fn validate_slug(value: &str, field: &str, max: usize) -> Result<(), VpsError> {
    let first = value.chars().next();
    let ok = !value.is_empty()
        && value.len() <= max
        && first.is_some_and(|c| c.is_ascii_lowercase())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.'))
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("..");
    if ok {
        Ok(())
    } else {
        Err(validation(field, format!("`{value}` is not a valid slug")))
    }
}

fn validate_email(email: &str) -> Result<(), VpsError> {
    let Some((local, domain)) = email.split_once('@') else {
        return Err(validation("email", "must look like user@domain".to_owned()));
    };
    if local.is_empty() || domain.is_empty() {
        return Err(validation(
            "email",
            "must have a user and a domain".to_owned(),
        ));
    }
    if crate::process::shell_metachar_in(email).is_some() || email.contains('\0') {
        return Err(validation(
            "email",
            "must not contain shell metacharacters".to_owned(),
        ));
    }
    Ok(())
}

fn validate_base64_key(key: &str) -> Result<(), VpsError> {
    let ok = !key.is_empty()
        && key.len() <= 100
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='));
    if ok {
        Ok(())
    } else {
        Err(validation(
            "client_public_key",
            "must be a base64 key".to_owned(),
        ))
    }
}

fn validate_geo_exit(address: &str) -> Result<(), VpsError> {
    let Some((host, port)) = address.rsplit_once(':') else {
        return Err(validation(
            "geo_exit",
            "must look like host:port".to_owned(),
        ));
    };
    if host.is_empty() || port.is_empty() || address.contains('\0') {
        return Err(validation(
            "geo_exit",
            "must be a non-empty host:port".to_owned(),
        ));
    }
    if crate::process::shell_metachar_in(address).is_some() {
        return Err(validation(
            "geo_exit",
            "must not contain shell metacharacters".to_owned(),
        ));
    }
    Ok(())
}

fn validate_request(request: &DeployRequest) -> Result<(&'static str, String), VpsError> {
    validate_host(&request.host)?;
    match &request.ingress {
        IngressMode::Tls { domain, email } => {
            validate_slug(domain, "domain", 253)?;
            validate_email(email)?;
            Ok(("tls", domain.clone()))
        }
        IngressMode::Tunnel(TunnelKind::Wireguard { client_public_key }) => {
            validate_base64_key(client_public_key)?;
            Ok(("tunnel-wireguard", WG_SERVER_ADDRESS.to_owned()))
        }
        IngressMode::Tunnel(TunnelKind::Tailscale { tailnet_host }) => {
            validate_slug(tailnet_host, "tailnet_host", 63)?;
            Ok(("tunnel-tailscale", tailnet_host.clone()))
        }
    }
}

fn validate_providers(request: &DeployRequest) -> Result<(), VpsError> {
    if let Some(geo) = &request.geo_exit {
        validate_geo_exit(geo)?;
    }
    for provider in &request.providers {
        validate_slug(&provider.name, "providers", 64)?;
        if provider.content.is_empty() || provider.content.contains('\0') {
            return Err(validation(
                "providers",
                format!(
                    "provider `{}` content must be non-empty TOML",
                    provider.name
                ),
            ));
        }
        if request.geo_exit.is_some() && provider.carries_egress_table() {
            return Err(validation(
                "geo_exit",
                format!(
                    "provider `{}` already carries an [egress] table; remove it or drop geo-exit",
                    provider.name
                ),
            ));
        }
    }
    Ok(())
}

/// Run the preflight probes (VPS-02 item 1) read-only.
pub fn preflight(
    host: &str,
    policy: HostKeyPolicy,
    remote: &dyn RemoteHost,
) -> Result<PreflightReport, VpsError> {
    validate_host(host)?;
    let mut checks = Vec::new();
    let reach = remote.exec(&["systemctl", "--version"]);
    let reach_detail = match &reach {
        Ok(output) if output.success() => first_line(&output.stdout),
        Ok(output) => format!("systemctl exited {:?}", output.exit),
        Err(error) => error.to_string(),
    };
    let reachable = matches!(&reach, Ok(output) if output.success());
    checks.push(PreflightCheck {
        name: "ssh-reachability",
        state: if reachable {
            CheckState::Pass(reach_detail)
        } else {
            CheckState::Fail(reach_detail)
        },
    });
    checks.push(PreflightCheck {
        name: "host-key-policy",
        state: CheckState::Pass(policy_note(policy)),
    });
    if !reachable {
        for name in ["os", "systemd", "disk"] {
            checks.push(PreflightCheck {
                name,
                state: CheckState::Fail("skipped: host unreachable".to_owned()),
            });
        }
        return Ok(finish_report(host, remote, policy, checks));
    }
    let os = remote.exec(&["cat", "/etc/os-release"]);
    checks.push(PreflightCheck {
        name: "os",
        state: match os {
            Ok(output) if output.success() => CheckState::Pass(os_detail(&output.stdout)),
            Ok(output) => CheckState::Fail(format!("os-release unreadable ({:?})", output.exit)),
            Err(error) => CheckState::Fail(error.to_string()),
        },
    });
    checks.push(PreflightCheck {
        name: "systemd",
        state: CheckState::Pass(match &reach {
            Ok(output) => first_line(&output.stdout),
            Err(_) => String::new(),
        }),
    });
    let disk = remote.exec(&["df", "-B1", "/"]);
    checks.push(PreflightCheck {
        name: "disk",
        state: match disk {
            Ok(output) if output.success() => match parse_disk_available(&output.stdout) {
                Some(free) if free >= MIN_DISK_FREE_BYTES => {
                    CheckState::Pass(format!("{free} bytes free on /"))
                }
                Some(free) => CheckState::Fail(format!(
                    "{free} bytes free on /, need {MIN_DISK_FREE_BYTES}"
                )),
                None => CheckState::Fail("df output unparseable".to_owned()),
            },
            Ok(output) => CheckState::Fail(format!("df exited {:?}", output.exit)),
            Err(error) => CheckState::Fail(error.to_string()),
        },
    });
    Ok(finish_report(host, remote, policy, checks))
}

fn finish_report(
    host: &str,
    remote: &dyn RemoteHost,
    policy: HostKeyPolicy,
    checks: Vec<PreflightCheck>,
) -> PreflightReport {
    PreflightReport {
        host: host.to_owned(),
        backend: remote.describe(),
        policy: policy_note(policy),
        checks,
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_owned()
}

fn os_detail(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .or_else(|| stdout.lines().find_map(|line| line.strip_prefix("ID=")))
        .unwrap_or("unknown")
        .trim_matches('"')
        .to_owned()
}

/// `df -B1` Available column: second line, fourth whitespace field.
pub(crate) fn parse_disk_available(stdout: &str) -> Option<u64> {
    stdout
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()
}

/// Plan a deploy (VPS-02 item 2): validate, read local artifacts, diff every
/// mutation against the remote tree. No remote mutation happens here.
pub fn plan_deploy(
    request: &DeployRequest,
    remote: &dyn RemoteHost,
) -> Result<DeployPlan, VpsError> {
    let (mode, address) = validate_request(request)?;
    validate_providers(request)?;
    let binary = read_binary(&request.binary_path)?;
    let env_bytes = match &request.env_file {
        Some(path) => Some(read_env_file(path)?),
        None => None,
    };
    if let IngressMode::Tunnel(TunnelKind::Wireguard { .. }) = &request.ingress {
        wg_private_key(env_bytes.as_deref())?;
    }
    let mut pre = preflight(&request.host, request.policy, remote)?;
    pre.checks.extend(ingress_checks(remote, &request.ingress));
    let ours = owned_paths(remote)?;

    let mut steps = Vec::new();
    steps.push(upload_step(
        remote,
        REMOTE_BIN_PATH,
        binary,
        0o755,
        false,
        &ours,
    )?);
    if let Some(bytes) = env_bytes.clone() {
        steps.push(upload_step(
            remote,
            REMOTE_ENV_PATH,
            bytes,
            0o600,
            true,
            &ours,
        )?);
    }
    provider_steps(
        &request.providers,
        request.geo_exit.as_deref(),
        env_bytes.is_some(),
        remote,
        &ours,
    )?
    .into_iter()
    .for_each(|step| steps.push(step));
    steps.push(unit_step(remote, &render_systemd_unit())?);
    steps.push(PlannedStep {
        step: DeployStep::ReloadDaemons,
        preview: StepPreview::DaemonsReload,
        payload: Payload::Bare,
    });
    steps.extend(ingress_steps(remote, request, &ours)?);
    steps.push(service_step("enable"));
    steps.push(service_step("start"));
    let created: Vec<String> = steps
        .iter()
        .filter_map(|step| match &step.step {
            DeployStep::UploadFile { path, .. } => Some(path.clone()),
            DeployStep::Symlink { link, .. } => Some(link.clone()),
            _ => None,
        })
        .chain(std::iter::once(REMOTE_UNIT_PATH.to_owned()))
        .collect();
    let manifest = render_manifest(mode, &address, &created);
    steps.push(upload_step(
        remote,
        REMOTE_MANIFEST_PATH,
        manifest.into_bytes(),
        0o644,
        false,
        &ours,
    )?);

    Ok(DeployPlan {
        host: request.host.clone(),
        backend: remote.describe(),
        service: SERVICE_NAME,
        mode: mode.to_owned(),
        harness_base: harness_base_url(mode, &address),
        point_instances: request.point_instances.clone(),
        address,
        geo_exit: request.geo_exit.clone(),
        preflight: pre,
        steps,
    })
}

fn service_step(verb: &'static str) -> PlannedStep {
    let name = SERVICE_NAME.to_owned();
    let (step, preview) = match verb {
        "enable" => (
            DeployStep::EnableService { name: name.clone() },
            StepPreview::Service {
                name,
                verb: "enable",
            },
        ),
        _ => (
            DeployStep::StartService { name: name.clone() },
            StepPreview::Service {
                name,
                verb: "start",
            },
        ),
    };
    PlannedStep {
        step,
        preview,
        payload: Payload::Bare,
    }
}

/// Upload each provider TOML (placeholders only) and, when it carries
/// `${NAME}` placeholders, schedule the on-VPS interpolation step.
fn provider_steps(
    providers: &[ProviderToml],
    geo_exit: Option<&str>,
    env_present: bool,
    remote: &dyn RemoteHost,
    ours: &[String],
) -> Result<Vec<PlannedStep>, VpsError> {
    let mut steps = Vec::new();
    for provider in providers {
        let path = format!("{REMOTE_PROVIDERS_DIR}/{}.toml", provider.name);
        let content = provider.effective(geo_exit);
        let carries_placeholders = content.contains("${");
        if carries_placeholders && !env_present {
            return Err(validation(
                "env_file",
                format!(
                    "provider `{}` carries ${{...}} placeholders; an env file with their \
                     values is required",
                    provider.name
                ),
            ));
        }
        steps.push(upload_step(
            remote,
            &path,
            content.into_bytes(),
            0o600,
            false,
            ours,
        )?);
        if carries_placeholders {
            steps.push(PlannedStep {
                step: DeployStep::InterpolateEnv {
                    target: path.clone(),
                    env: REMOTE_ENV_PATH.to_owned(),
                },
                preview: StepPreview::InterpolateEnv {
                    target: path,
                    env: REMOTE_ENV_PATH.to_owned(),
                },
                payload: Payload::Bare,
            });
        }
    }
    Ok(steps)
}

fn ingress_steps(
    remote: &dyn RemoteHost,
    request: &DeployRequest,
    ours: &[String],
) -> Result<Vec<PlannedStep>, VpsError> {
    match &request.ingress {
        IngressMode::Tls { domain, email } => {
            let site = render_nginx_site(domain);
            Ok(vec![
                upload_step(
                    remote,
                    REMOTE_NGINX_SITE_PATH,
                    site.into_bytes(),
                    0o644,
                    false,
                    ours,
                )?,
                exec_step(&[
                    "certbot",
                    "certonly",
                    "--standalone",
                    "-d",
                    domain,
                    "-m",
                    email,
                    "--agree-tos",
                    "--non-interactive",
                ])?,
                symlink_step(
                    REMOTE_NGINX_SITE_PATH,
                    REMOTE_NGINX_ENABLED_PATH,
                    remote,
                    ours,
                )?,
                exec_step(&["nginx", "-t"])?,
                exec_step(&["systemctl", "reload", "nginx"])?,
            ])
        }
        IngressMode::Tunnel(TunnelKind::Wireguard { client_public_key }) => {
            let conf = render_wireguard_conf(
                WG_SERVER_ADDRESS,
                client_public_key,
                Some("${LLMPROXY_WG_PRIVATE_KEY}"),
            );
            Ok(vec![
                upload_step(
                    remote,
                    REMOTE_WG_CONF_PATH,
                    conf.into_bytes(),
                    0o600,
                    false,
                    ours,
                )?,
                PlannedStep {
                    step: DeployStep::InterpolateEnv {
                        target: REMOTE_WG_CONF_PATH.to_owned(),
                        env: REMOTE_ENV_PATH.to_owned(),
                    },
                    preview: StepPreview::InterpolateEnv {
                        target: REMOTE_WG_CONF_PATH.to_owned(),
                        env: REMOTE_ENV_PATH.to_owned(),
                    },
                    payload: Payload::Bare,
                },
            ])
        }
        IngressMode::Tunnel(TunnelKind::Tailscale { tailnet_host }) => Ok(vec![exec_step(&[
            "tailscale",
            "up",
            "--hostname",
            tailnet_host,
        ])?]),
    }
}

fn wg_private_key(env_bytes: Option<&[u8]>) -> Result<String, VpsError> {
    let refusal = |reason: &str| Err(validation("env_file", reason.to_owned()));
    let Some(bytes) = env_bytes else {
        return refusal("wireguard tunnel mode needs an env file carrying LLMPROXY_WG_PRIVATE_KEY");
    };
    let env = String::from_utf8_lossy(bytes).into_owned();
    let key = env
        .lines()
        .find_map(|line| line.strip_prefix("LLMPROXY_WG_PRIVATE_KEY="))
        .map(str::trim);
    match key {
        Some(key) if !key.is_empty() => Ok(key.to_owned()),
        _ => refusal("env file must carry a non-empty LLMPROXY_WG_PRIVATE_KEY"),
    }
}

pub(crate) fn read_binary(path: &Path) -> Result<Vec<u8>, VpsError> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| VpsError::LocalArtifact {
        path: path.to_owned(),
        reason: format!("unreadable: {e}"),
    })?;
    if !meta.is_file() {
        return Err(VpsError::LocalArtifact {
            path: path.to_owned(),
            reason: "must be a regular file".to_owned(),
        });
    }
    if meta.len() > 128 * 1024 * 1024 {
        return Err(VpsError::LocalArtifact {
            path: path.to_owned(),
            reason: format!("{} bytes exceeds the 128 MiB cap", meta.len()),
        });
    }
    let bytes = std::fs::read(path).map_err(|e| VpsError::LocalArtifact {
        path: path.to_owned(),
        reason: format!("read failed: {e}"),
    })?;
    if bytes.is_empty() {
        return Err(VpsError::LocalArtifact {
            path: path.to_owned(),
            reason: "empty binary".to_owned(),
        });
    }
    Ok(bytes)
}

fn read_env_file(path: &Path) -> Result<Vec<u8>, VpsError> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| VpsError::LocalArtifact {
        path: path.to_owned(),
        reason: format!("unreadable: {e}"),
    })?;
    if !meta.is_file() {
        return Err(VpsError::LocalArtifact {
            path: path.to_owned(),
            reason: "must be a regular file".to_owned(),
        });
    }
    if meta.len() > 64 * 1024 {
        return Err(VpsError::LocalArtifact {
            path: path.to_owned(),
            reason: format!("{} bytes exceeds the 64 KiB cap", meta.len()),
        });
    }
    std::fs::read(path).map_err(|e| VpsError::LocalArtifact {
        path: path.to_owned(),
        reason: format!("read failed: {e}"),
    })
}

/// Paths a previous superai deploy recorded in the remote manifest.
fn owned_paths(remote: &dyn RemoteHost) -> Result<Vec<String>, VpsError> {
    let Some(bytes) = remote.read_file(REMOTE_MANIFEST_PATH)? else {
        return Ok(Vec::new());
    };
    let parsed: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    Ok(parsed
        .get("paths")
        .and_then(|paths| paths.as_array())
        .map(|paths| {
            paths
                .iter()
                .filter_map(|path| path.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

fn upload_step(
    remote: &dyn RemoteHost,
    path: &str,
    bytes: Vec<u8>,
    mode: u32,
    secret: bool,
    ours: &[String],
) -> Result<PlannedStep, VpsError> {
    ensure_remote_path(path)?;
    let current = remote.read_file(path)?;
    let new_text = String::from_utf8(bytes.clone()).ok();
    let content = if secret {
        None
    } else {
        new_text.as_ref().map(|text| redact_sensitive_lines(text))
    };
    let preview = match current {
        None => StepPreview::Create {
            path: path.to_owned(),
            content,
        },
        Some(old) => {
            let existing = if ours.contains(&path.to_owned()) {
                Existing::Ours
            } else {
                Existing::Foreign
            };
            // A secret payload never enters a diff: both sides are replaced
            // by byte counts, so an old value can never leak into a preview.
            let diff = if secret {
                format!(
                    "-secret payload ({} bytes)\n+secret payload ({} bytes)",
                    old.len(),
                    bytes.len()
                )
            } else {
                match (String::from_utf8(old).ok(), new_text.as_ref()) {
                    (Some(old_text), Some(new_text)) => line_diff(
                        &redact_sensitive_lines(&old_text),
                        &redact_sensitive_lines(new_text),
                    ),
                    (Some(old_text), None) => {
                        format!(
                            "-binary payload replaced (was {} text bytes)",
                            old_text.len()
                        )
                    }
                    (None, _) => format!("+binary payload ({} bytes)", bytes.len()),
                }
            };
            StepPreview::Replace {
                path: path.to_owned(),
                existing,
                diff,
                content,
            }
        }
    };
    Ok(PlannedStep {
        step: DeployStep::UploadFile {
            path: path.to_owned(),
            mode,
            bytes: bytes.len(),
            secret,
        },
        preview,
        payload: Payload::Upload {
            bytes,
            mode,
            secret,
        },
    })
}

fn unit_step(remote: &dyn RemoteHost, content: &str) -> Result<PlannedStep, VpsError> {
    let current = remote.read_file(REMOTE_UNIT_PATH)?;
    let exists = current.is_some();
    let diff = match current {
        None => String::new(),
        Some(old) => line_diff(
            &redact_sensitive_lines(&String::from_utf8_lossy(&old)),
            content,
        ),
    };
    Ok(PlannedStep {
        step: DeployStep::InstallUnit {
            name: SERVICE_NAME.to_owned(),
        },
        preview: StepPreview::UnitInstall {
            path: REMOTE_UNIT_PATH.to_owned(),
            backup_planned: exists,
            diff,
        },
        payload: Payload::Unit {
            content: content.to_owned(),
        },
    })
}

fn exec_step(argv: &[&str]) -> Result<PlannedStep, VpsError> {
    ensure_exec_argv(argv)?;
    let argv: Vec<String> = argv.iter().map(|token| (*token).to_owned()).collect();
    Ok(PlannedStep {
        preview: StepPreview::Run { argv: argv.clone() },
        step: DeployStep::Exec { argv },
        payload: Payload::Bare,
    })
}

fn symlink_step(
    target: &str,
    link: &str,
    remote: &dyn RemoteHost,
    ours: &[String],
) -> Result<PlannedStep, VpsError> {
    ensure_remote_path(target)?;
    ensure_remote_path(link)?;
    let _ = (remote, ours);
    Ok(PlannedStep {
        step: DeployStep::Symlink {
            target: target.to_owned(),
            link: link.to_owned(),
        },
        preview: StepPreview::Symlink {
            target: target.to_owned(),
            link: link.to_owned(),
        },
        payload: Payload::Bare,
    })
}

/// Ingress-tooling preflight (VPS-02 item 3): TLS needs nginx, certbot, and
/// openssl on the host; tunnels need their transport binary.
fn ingress_checks(remote: &dyn RemoteHost, ingress: &IngressMode) -> Vec<PreflightCheck> {
    let probes: &[(&'static str, &[&str])] = match ingress {
        IngressMode::Tls { .. } => &[
            ("ingress-nginx", &["nginx", "-v"]),
            ("ingress-certbot", &["certbot", "--version"]),
            ("ingress-openssl", &["openssl", "version"]),
        ],
        IngressMode::Tunnel(TunnelKind::Wireguard { .. }) => {
            &[("ingress-wireguard", &["wg", "--version"])]
        }
        IngressMode::Tunnel(TunnelKind::Tailscale { .. }) => {
            &[("ingress-tailscale", &["tailscale", "version"])]
        }
    };
    probes
        .iter()
        .map(|(name, argv)| {
            let state = match remote.exec(argv) {
                Ok(output) if output.success() => CheckState::Pass(first_line(&output.stdout)),
                Ok(output) => CheckState::Fail(format!("exited {:?}", output.exit)),
                Err(error) => CheckState::Fail(error.to_string()),
            };
            PreflightCheck { name, state }
        })
        .collect()
}

/// Minimal line diff: common prefix and suffix are dropped, the changed
/// middle renders as `-old` and `+new` lines. Both callers pass redacted
/// text; this function never sees a secret.
pub(crate) fn line_diff(old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut start = 0;
    while start < old_lines.len()
        && start < new_lines.len()
        && old_lines.get(start) == new_lines.get(start)
    {
        start += 1;
    }
    let mut end_old = old_lines.len();
    let mut end_new = new_lines.len();
    while end_old > start
        && end_new > start
        && old_lines.get(end_old - 1) == new_lines.get(end_new - 1)
    {
        end_old -= 1;
        end_new -= 1;
    }
    let mut out = String::new();
    for line in old_lines.iter().take(end_old).skip(start) {
        out.push('-');
        out.push_str(line);
        out.push('\n');
    }
    for line in new_lines.iter().take(end_new).skip(start) {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if out.is_empty() {
        "unchanged".to_owned()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vps::remote::{FixtureHost, FixtureRemote, TorideRemote};

    fn tls_request(dir: &Path) -> DeployRequest {
        let binary = dir.join("llmproxy");
        std::fs::write(&binary, b"musl-bytes").unwrap();
        let env_file = dir.join("secrets.env");
        std::fs::write(&env_file, "LLMPROXY_ADMIN_TOKEN=fixture-token\n").unwrap();
        DeployRequest {
            host: "root@203.0.113.10".to_owned(),
            point_instances: Vec::new(),
            ingress: IngressMode::Tls {
                domain: "proxy.example.com".to_owned(),
                email: "ops@example.com".to_owned(),
            },
            geo_exit: None,
            binary_path: binary,
            providers: vec![ProviderToml {
                name: "anthropic".to_owned(),
                content: "api_key = \"PLACEHOLDER\"\nmodel = \"claude-sonnet-4-5\"\n".to_owned(),
            }],
            env_file: Some(env_file),
            policy: HostKeyPolicy::Strict,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("superai-vps-plan-{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn kind_of(step: &PlannedStep) -> &'static str {
        match step.step {
            DeployStep::UploadFile { .. } => "upload-file",
            DeployStep::InstallUnit { .. } => "install-unit",
            DeployStep::ReloadDaemons => "reload-daemons",
            DeployStep::Exec { .. } => "exec",
            DeployStep::EnableService { .. } => "enable-service",
            DeployStep::StartService { .. } => "start-service",
            DeployStep::RestartService { .. } => "restart-service",
            DeployStep::Symlink { .. } => "symlink",
            DeployStep::InterpolateEnv { .. } => "interpolate-env",
        }
    }

    #[test]
    fn host_validation_accepts_plain_targets_and_refuses_the_rest() {
        for good in ["root@203.0.113.10", "deploy@host.example.org"] {
            assert!(validate_host(good).is_ok(), "{good}");
        }
        for bad in [
            "host",
            "@host",
            "root@",
            "root@two@hosts",
            "root@host/path",
            " roobot@host ",
            "root@host;rm",
            "",
        ] {
            assert!(validate_host(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn preflight_passes_on_a_healthy_fixture_and_blocks_when_probes_fail() {
        let remote = FixtureRemote::new("root@host");
        let report = preflight("root@host", HostKeyPolicy::AcceptNew, &remote).unwrap();
        assert!(!report.blocked(), "{report:?}");
        assert!(report.checks.iter().any(|check| check.name == "os"
            && matches!(&check.state, CheckState::Pass(d) if d.contains("debian"))));

        let broken = FixtureRemote::new("root@host");
        broken.set_host(FixtureHost {
            systemd_version: None,
            ..FixtureHost::default()
        });
        let report = preflight("root@host", HostKeyPolicy::Strict, &broken).unwrap();
        assert!(report.blocked());
        assert!(
            report
                .first_failure()
                .is_some_and(|text| text.contains("systemctl"))
        );

        let tight = FixtureRemote::new("root@host");
        tight.set_host(FixtureHost {
            disk_free_bytes: 1024,
            ..FixtureHost::default()
        });
        let report = preflight("root@host", HostKeyPolicy::Strict, &tight).unwrap();
        assert!(
            report
                .first_failure()
                .is_some_and(|text| text.contains("disk"))
        );
    }

    #[test]
    fn preflight_blocks_closed_when_the_backend_cannot_probe() {
        let remote = TorideRemote::new("root@host", HostKeyPolicy::Strict);
        let report = preflight("root@host", HostKeyPolicy::Strict, &remote).unwrap();
        assert!(report.blocked(), "{report:?}");
        assert!(
            report
                .first_failure()
                .is_some_and(|text| text.contains("pending toride-ssh")),
            "the pending gate must be the stated reason: {report:?}"
        );
    }

    #[test]
    fn disk_parser_takes_the_available_column() {
        let stdout =
            "Filesystem 1-blocks Used Available Use% Mounted on\n/dev/vda1 20 8 12 40% /\n";
        assert_eq!(parse_disk_available(stdout), Some(12));
        assert_eq!(parse_disk_available("only header"), None);
    }

    #[test]
    fn plan_previews_a_full_tls_deploy_against_a_fresh_fixture() {
        let dir = temp_dir("tls-fresh");
        let remote = FixtureRemote::new("root@203.0.113.10");
        let plan = plan_deploy(&tls_request(&dir), &remote).unwrap();
        assert!(!plan.blocked());
        assert_eq!(plan.mode, "tls");
        assert_eq!(plan.address, "proxy.example.com");
        let kinds: Vec<&str> = plan.steps.iter().map(kind_of).collect();
        for wanted in [
            "upload-file",
            "install-unit",
            "reload-daemons",
            "exec",
            "enable-service",
            "start-service",
        ] {
            assert!(kinds.contains(&wanted), "{kinds:?}");
        }
        let env_step = plan
            .steps
            .iter()
            .find(|step| match &step.step {
                DeployStep::UploadFile {
                    path, secret: true, ..
                } => path == REMOTE_ENV_PATH,
                _ => false,
            })
            .unwrap();
        match &env_step.preview {
            StepPreview::Create { content, .. } => {
                assert!(content.is_none(), "secrets are never previewed");
            }
            other => panic!("env upload must be a create: {other:?}"),
        }
    }

    #[test]
    fn plan_marks_foreign_files_and_diffs_against_them() {
        let dir = temp_dir("foreign");
        let remote = FixtureRemote::new("root@host");
        remote.insert_file(REMOTE_UNIT_PATH, b"[Unit]\nDescription=foreign\n", 0o644);
        let plan = plan_deploy(&tls_request(&dir), &remote).unwrap();
        let unit_step = plan
            .steps
            .iter()
            .find(|step| matches!(step.step, DeployStep::InstallUnit { .. }))
            .unwrap();
        match &unit_step.preview {
            StepPreview::UnitInstall {
                backup_planned,
                diff,
                ..
            } => {
                assert!(backup_planned, "a foreign unit must be backed up first");
                assert!(diff.contains("-Description=foreign"), "{diff}");
                assert!(diff.contains("+Description="), "{diff}");
            }
            other => panic!("unit preview: {other:?}"),
        }
    }

    #[test]
    fn plan_after_a_deploy_marks_our_files_as_ours() {
        let dir = temp_dir("ours");
        let remote = FixtureRemote::new("root@host");
        let first = plan_deploy(&tls_request(&dir), &remote).unwrap();
        let manifest = first
            .steps
            .iter()
            .find_map(|step| match &step.payload {
                Payload::Upload { bytes, .. }
                    if matches!(&step.step, DeployStep::UploadFile { path, .. } if path == REMOTE_MANIFEST_PATH) =>
                {
                    Some(bytes.clone())
                }
                _ => None,
            })
            .unwrap();
        remote
            .upload(REMOTE_MANIFEST_PATH, &manifest, 0o644)
            .unwrap();
        remote.insert_file(REMOTE_BIN_PATH, b"old-binary", 0o755);
        let second = plan_deploy(&tls_request(&dir), &remote).unwrap();
        let bin_step = second
            .steps
            .iter()
            .find(|step| match &step.step {
                DeployStep::UploadFile { path, .. } => path == REMOTE_BIN_PATH,
                _ => false,
            })
            .unwrap();
        match &bin_step.preview {
            StepPreview::Replace { existing, .. } => assert_eq!(*existing, Existing::Ours),
            other => panic!("binary preview: {other:?}"),
        }
    }

    #[test]
    fn geo_exit_appends_an_egress_table_and_refuses_a_duplicate() {
        let dir = temp_dir("geo");
        let mut request = tls_request(&dir);
        request.geo_exit = Some("10.7.0.1:3128".to_owned());
        let remote = FixtureRemote::new("root@host");
        let plan = plan_deploy(&request, &remote).unwrap();
        let provider_step = plan
            .steps
            .iter()
            .find(|step| match &step.step {
                DeployStep::UploadFile { path, .. } => path.contains("anthropic"),
                _ => false,
            })
            .unwrap();
        match (&provider_step.payload, &provider_step.preview) {
            (
                Payload::Upload { bytes, .. },
                StepPreview::Create {
                    content: Some(content),
                    ..
                },
            ) => {
                let text = String::from_utf8(bytes.clone()).unwrap();
                assert!(
                    text.contains("[egress]\nproxy = \"socks5://10.7.0.1:3128\""),
                    "{text}"
                );
                assert!(content.contains("proxy = \"socks5://10.7.0.1:3128\""));
                assert!(content.contains("api_key=[REDACTED]"), "{content}");
            }
            other => panic!("provider preview: {other:?}"),
        }

        let mut dupe = tls_request(&dir);
        dupe.providers = vec![ProviderToml {
            name: "anthropic".to_owned(),
            content: "[egress]\nproxy = \"x\"\n".to_owned(),
        }];
        dupe.geo_exit = Some("10.7.0.1:3128".to_owned());
        let err = plan_deploy(&dupe, &remote).unwrap_err();
        assert!(err.to_string().contains("[egress]"), "{err}");
    }

    #[test]
    fn tunnel_modes_need_their_inputs() {
        let dir = temp_dir("tunnel");
        let mut request = tls_request(&dir);
        request.ingress = IngressMode::Tunnel(TunnelKind::Wireguard {
            client_public_key: "clientpubkey=".to_owned(),
        });
        let env_file = dir.join("wg.env");
        std::fs::write(&env_file, "LLMPROXY_WG_PRIVATE_KEY=wg-server-secret\n").unwrap();
        request.env_file = Some(env_file);
        let remote = FixtureRemote::new("root@host");
        let plan = plan_deploy(&request, &remote).unwrap();
        assert_eq!(plan.mode, "tunnel-wireguard");
        let wg_step = plan
            .steps
            .iter()
            .find(|step| match &step.step {
                DeployStep::UploadFile { path, .. } => path == REMOTE_WG_CONF_PATH,
                _ => false,
            })
            .unwrap();
        match &wg_step.preview {
            StepPreview::Create {
                content: Some(content),
                ..
            } => {
                assert!(
                    !content.contains("LLMPROXY_WG_PRIVATE_KEY"),
                    "the key placeholder never previews: {content}"
                );
                assert!(content.contains("[REDACTED]"), "{content}");
            }
            other => panic!("wg preview: {other:?}"),
        }
        match &wg_step.payload {
            Payload::Upload { bytes, .. } => {
                let text = String::from_utf8(bytes.clone()).unwrap();
                assert!(
                    text.contains("${LLMPROXY_WG_PRIVATE_KEY}"),
                    "upload carries only the placeholder"
                );
                assert!(!text.contains("wg-server-secret"));
            }
            other => panic!("{other:?}"),
        }
        assert!(plan.steps.iter().any(|step| matches!(
            &step.step,
            DeployStep::InterpolateEnv { target, .. } if target == REMOTE_WG_CONF_PATH
        )));

        request.env_file = None;
        let err = plan_deploy(&request, &remote).unwrap_err();
        assert!(err.to_string().contains("LLMPROXY_WG_PRIVATE_KEY"), "{err}");

        let mut ts = tls_request(&dir);
        ts.ingress = IngressMode::Tunnel(TunnelKind::Tailscale {
            tailnet_host: "proxy-tailnet".to_owned(),
        });
        let plan = plan_deploy(&ts, &remote).unwrap();
        assert_eq!(plan.mode, "tunnel-tailscale");
        assert!(plan.steps.iter().any(|step| match &step.step {
            DeployStep::Exec { argv } => argv.first().is_some_and(|program| program == "tailscale"),
            _ => false,
        }));
    }

    #[test]
    fn invalid_requests_fail_before_any_remote_contact() {
        let dir = temp_dir("invalid");
        let remote = FixtureRemote::new("root@host");
        let mut bad_host = tls_request(&dir);
        bad_host.host = "no-at-sign".to_owned();
        assert!(plan_deploy(&bad_host, &remote).is_err(), "bad host");

        let mut bad_domain = tls_request(&dir);
        bad_domain.ingress = IngressMode::Tls {
            domain: "Proxy;rm".to_owned(),
            email: "ops@example.com".to_owned(),
        };
        assert!(plan_deploy(&bad_domain, &remote).is_err(), "bad domain");

        let mut bad_provider = tls_request(&dir);
        bad_provider.providers = vec![ProviderToml {
            name: "escape-attempt".to_owned(),
            content: String::new(),
        }];
        assert!(plan_deploy(&bad_provider, &remote).is_err(), "bad provider");

        let mut missing_binary = tls_request(&dir);
        missing_binary.binary_path = dir.join("absent");
        assert!(
            plan_deploy(&missing_binary, &remote)
                .unwrap_err()
                .to_string()
                .contains("absent")
        );
    }

    #[test]
    fn line_diff_marks_changed_lines_and_reports_unchanged() {
        assert_eq!(line_diff("a\nb\n", "a\nb\n"), "unchanged");
        let diff = line_diff("a\nold\nz\n", "a\nnew\nz\n");
        assert!(diff.contains("-old"), "{diff}");
        assert!(diff.contains("+new"), "{diff}");
        assert!(!diff.contains('a'), "{diff}");
        assert!(!diff.contains('z'), "{diff}");
        let added = line_diff("", "fresh\n");
        assert!(added.contains("+fresh"), "{added}");
    }
}
