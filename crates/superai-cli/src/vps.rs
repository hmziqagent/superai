//! `superai vps` subcommands (subplan 16): deploy/status/upgrade for the
//! llmproxy service on a remote host. Remote work runs through the
//! toride-backed port, which refuses typed until toride 0.2 publishes; the
//! fixture-proven flow lives in superai-core's tests.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use superai_core::vps as core_vps;
use superai_core::vps::{
    DeployJournal, DeployPlan, DeployRequest, DeployStep, HostKeyPolicy, IngressMode, ProviderToml,
    StepPreview, TunnelKind, UpgradeRequest,
};
use superai_usage::llmproxy::{
    self, LedgerClient, SummaryGroupBy, load_admin_token, resolve_base_url,
};
use superai_usage::poller::PollLimits;

use crate::CliError;

const LLMPROXY_WINDOW: &str = "30d";

pub(crate) fn run(args: &[OsString], home: &Path) -> Result<(), CliError> {
    let (verb, rest) = args.split_first().ok_or_else(|| CliError::Args {
        reason: "vps needs a verb: deploy, status, or upgrade".to_owned(),
    })?;
    let verb = verb.to_str().ok_or_else(not_utf8)?;
    if !matches!(verb, "deploy" | "status" | "upgrade") {
        return Err(CliError::Args {
            reason: format!(
                "unknown vps verb `{}`; available: deploy, status, upgrade",
                crate::bounded(verb)
            ),
        });
    }
    let (service, flags) = rest.split_first().ok_or_else(|| CliError::Args {
        reason: format!("vps {verb} needs a service; available: llmproxy"),
    })?;
    let service = service.to_str().ok_or_else(not_utf8)?;
    if service != "llmproxy" {
        return Err(CliError::Args {
            reason: format!(
                "unknown vps service `{}`; available: llmproxy",
                crate::bounded(service)
            ),
        });
    }
    match verb {
        "deploy" => run_deploy(flags),
        "status" => run_status(flags, home),
        "upgrade" => run_upgrade(flags),
        _ => Err(CliError::Args {
            reason: format!(
                "unknown vps verb `{}`; available: deploy, status, upgrade",
                crate::bounded(verb)
            ),
        }),
    }
}

fn run_deploy(args: &[OsString]) -> Result<(), CliError> {
    let parsed = DeployArgs::parse(args)?;
    let request = build_request(&parsed)?;
    let remote = core_vps::TorideRemote::new(&parsed.host, parsed.policy);
    let plan = core_vps::plan_deploy(&request, &remote).map_err(CliError::from)?;
    if !parsed.json {
        crate::write_stdout(&render_plan(&plan))?;
    }
    let journal = core_vps::execute_deploy(&plan, &remote).map_err(CliError::from)?;
    let pointing = point_harness_configs(&plan, &request)?;
    if parsed.json {
        write_json(&deploy_report_json(&plan, &journal, &pointing))?;
    } else {
        crate::write_stdout(&render_journal(&journal))?;
        render_pointing(&pointing)?;
    }
    if journal.rolled_back.is_some() {
        return Err(CliError::Args {
            reason: "deploy failed and rolled back; see the output above".to_owned(),
        });
    }
    Ok(())
}

/// Point every recorded instance the request named at the deployment base.
fn point_harness_configs(
    plan: &DeployPlan,
    request: &DeployRequest,
) -> Result<Vec<core_vps::PointOutcome>, CliError> {
    if plan.point_instances.is_empty() {
        return Ok(Vec::new());
    }
    let registry_path = superai_core::Registry::default_path()?;
    let provider_ids: Vec<&str> = request
        .providers
        .iter()
        .map(|provider| provider.name.as_str())
        .collect();
    plan.point_instances
        .iter()
        .map(|name| {
            core_vps::point_recorded_instance(
                &registry_path,
                name,
                &provider_ids,
                &plan.harness_base,
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(CliError::from)
        .map(|batches| batches.into_iter().flatten().collect())
}

fn render_pointing(outcomes: &[core_vps::PointOutcome]) -> Result<(), CliError> {
    for outcome in outcomes {
        let applied = if outcome.applied.is_empty() {
            "no changes needed".to_owned()
        } else {
            outcome.applied.join("; ")
        };
        crate::write_stdout(&format!(
            "pointed {} provider {} at deployment ({}): {applied}\n",
            outcome.instance,
            outcome.provider,
            outcome.path.display()
        ))?;
    }
    Ok(())
}

fn run_upgrade(args: &[OsString]) -> Result<(), CliError> {
    let parsed = UpgradeArgs::parse(args)?;
    let request = UpgradeRequest {
        host: parsed.host.clone(),
        binary_path: parsed.binary.clone(),
        policy: parsed.policy,
    };
    let remote = core_vps::TorideRemote::new(&parsed.host, parsed.policy);
    let plan = core_vps::plan_upgrade(&request, &remote).map_err(CliError::from)?;
    if !parsed.json {
        crate::write_stdout(&render_plan(&plan))?;
    }
    let journal = core_vps::execute_upgrade(&plan, &remote).map_err(CliError::from)?;
    if parsed.json {
        write_json(&deploy_report_json(&plan, &journal, &[]))?;
    } else {
        crate::write_stdout(&render_journal(&journal))?;
    }
    if journal.rolled_back.is_some() {
        return Err(CliError::Args {
            reason: "upgrade failed and rolled back; see the output above".to_owned(),
        });
    }
    Ok(())
}

fn run_status(args: &[OsString], home: &Path) -> Result<(), CliError> {
    let parsed = StatusArgs::parse(args)?;
    let remote = core_vps::TorideRemote::new(&parsed.host, parsed.policy);
    let report = core_vps::remote_status(&parsed.host, &remote).map_err(CliError::from)?;
    let ledger = LedgerView::probe(home);
    if parsed.json {
        write_json(&StatusJson::of(&report, &ledger))?;
    } else {
        crate::write_stdout(&render_status(&report, &ledger))?;
    }
    Ok(())
}

fn build_request(args: &DeployArgs) -> Result<DeployRequest, CliError> {
    let ingress = match (&args.domain, &args.email, args.tunnel, &args.tailscale) {
        (Some(domain), Some(email), false, None) => IngressMode::Tls {
            domain: domain.clone(),
            email: email.clone(),
        },
        (None, None, true, None) => IngressMode::Tunnel(TunnelKind::Wireguard {
            client_public_key: args.wg_client_key.clone().ok_or_else(|| CliError::Args {
                reason: "--tunnel needs --wg-client-key with the workstation peer key".to_owned(),
            })?,
        }),
        (None, None, false, Some(tailnet)) => IngressMode::Tunnel(TunnelKind::Tailscale {
            tailnet_host: tailnet.clone(),
        }),
        _ => {
            return Err(CliError::Args {
                reason: "pick one ingress: --domain with --email, --tunnel, or --tailscale"
                    .to_owned(),
            });
        }
    };
    let mut providers = Vec::new();
    for path in &args.providers {
        providers.push(read_provider(path)?);
    }
    Ok(DeployRequest {
        host: args.host.clone(),
        ingress,
        geo_exit: args.geo_exit.clone(),
        binary_path: args.binary.clone(),
        providers,
        env_file: args.env_file.clone(),
        point_instances: args.point.clone(),
        policy: args.policy,
    })
}

/// Provider TOMLs are read fresh from disk here; the name is the file stem.
fn read_provider(path: &Path) -> Result<ProviderToml, CliError> {
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .ok_or_else(|| CliError::Args {
            reason: format!("provider path `{}` has no file name", path.display()),
        })?;
    let content = std::fs::read_to_string(path).map_err(|e| CliError::Args {
        reason: format!("provider file `{}` unreadable: {e}", path.display()),
    })?;
    Ok(ProviderToml { name, content })
}

fn not_utf8() -> CliError {
    CliError::Args {
        reason: "an argument is not valid UTF-8".to_owned(),
    }
}

fn dup(flag: &str) -> CliError {
    CliError::Args {
        reason: format!("{flag} given twice"),
    }
}

fn unknown_flag(flag: &str) -> CliError {
    CliError::Args {
        reason: format!("unknown flag `{}`", crate::bounded(flag)),
    }
}

fn value_of(words: &mut std::slice::Iter<'_, OsString>, flag: &str) -> Result<String, CliError> {
    let Some(value) = words.next() else {
        return Err(CliError::Args {
            reason: format!("{flag} needs a value"),
        });
    };
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| CliError::Args {
            reason: format!("{flag} value is not valid UTF-8"),
        })
}

fn once<T>(slot: &mut Option<T>, flag: &str, value: T) -> Result<(), CliError> {
    if slot.is_some() {
        return Err(dup(flag));
    }
    *slot = Some(value);
    Ok(())
}

fn parse_policy(text: &str) -> Result<HostKeyPolicy, CliError> {
    match text {
        "strict" => Ok(HostKeyPolicy::Strict),
        "accept-new" => Ok(HostKeyPolicy::AcceptNew),
        "off" => Err(CliError::Args {
            reason: "host-key policy `off` is never accepted; use strict or accept-new".to_owned(),
        }),
        _ => Err(CliError::Args {
            reason: "--host-keys expects strict or accept-new".to_owned(),
        }),
    }
}

fn host_of(words: &mut std::slice::Iter<'_, OsString>) -> Result<String, CliError> {
    value_of(words, "--host")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeployArgs {
    host: String,
    policy: HostKeyPolicy,
    domain: Option<String>,
    email: Option<String>,
    tunnel: bool,
    tailscale: Option<String>,
    wg_client_key: Option<String>,
    geo_exit: Option<String>,
    binary: PathBuf,
    providers: Vec<PathBuf>,
    env_file: Option<PathBuf>,
    point: Vec<String>,
    json: bool,
}

impl DeployArgs {
    fn parse(args: &[OsString]) -> Result<Self, CliError> {
        let mut host: Option<String> = None;
        let mut policy: Option<HostKeyPolicy> = None;
        let mut domain: Option<String> = None;
        let mut email: Option<String> = None;
        let mut tunnel = false;
        let mut tailscale: Option<String> = None;
        let mut wg_client_key: Option<String> = None;
        let mut geo_exit: Option<String> = None;
        let mut binary: Option<PathBuf> = None;
        let mut providers: Vec<PathBuf> = Vec::new();
        let mut env_file: Option<PathBuf> = None;
        let mut point: Vec<String> = Vec::new();
        let mut json = false;
        let mut words = args.iter();
        while let Some(word) = words.next() {
            match word.to_str().ok_or_else(not_utf8)? {
                "--json" => json = true,
                "--tunnel" => tunnel = true,
                "--host" => once(&mut host, "--host", host_of(&mut words)?)?,
                "--host-keys" => once(
                    &mut policy,
                    "--host-keys",
                    parse_policy(&value_of(&mut words, "--host-keys")?)?,
                )?,
                "--domain" => once(&mut domain, "--domain", value_of(&mut words, "--domain")?)?,
                "--email" => once(&mut email, "--email", value_of(&mut words, "--email")?)?,
                "--tailscale" => once(
                    &mut tailscale,
                    "--tailscale",
                    value_of(&mut words, "--tailscale")?,
                )?,
                "--wg-client-key" => once(
                    &mut wg_client_key,
                    "--wg-client-key",
                    value_of(&mut words, "--wg-client-key")?,
                )?,
                "--geo-exit" => once(
                    &mut geo_exit,
                    "--geo-exit",
                    value_of(&mut words, "--geo-exit")?,
                )?,
                "--binary" => once(
                    &mut binary,
                    "--binary",
                    PathBuf::from(value_of(&mut words, "--binary")?),
                )?,
                "--provider" => providers.push(PathBuf::from(value_of(&mut words, "--provider")?)),
                "--env-file" => once(
                    &mut env_file,
                    "--env-file",
                    PathBuf::from(value_of(&mut words, "--env-file")?),
                )?,
                "--point" => point.push(value_of(&mut words, "--point")?),
                other => return Err(unknown_flag(other)),
            }
        }
        let host = host.ok_or_else(|| CliError::Args {
            reason: "vps deploy needs --host user@host".to_owned(),
        })?;
        let binary = binary.ok_or_else(|| CliError::Args {
            reason: "vps deploy needs --binary with the local musl build".to_owned(),
        })?;
        let ingress_choices = [
            domain.is_some() || email.is_some(),
            tunnel,
            tailscale.is_some(),
        ];
        if ingress_choices.iter().filter(|chosen| **chosen).count() != 1 {
            return Err(CliError::Args {
                reason: "pick one ingress: --domain with --email, --tunnel, or --tailscale"
                    .to_owned(),
            });
        }
        if domain.is_some() != email.is_some() {
            return Err(CliError::Args {
                reason: "--domain and --email go together".to_owned(),
            });
        }
        if tunnel && wg_client_key.is_none() {
            return Err(CliError::Args {
                reason: "--tunnel needs --wg-client-key with the workstation peer key".to_owned(),
            });
        }
        Ok(Self {
            host,
            policy: policy.unwrap_or(HostKeyPolicy::Strict),
            domain,
            email,
            tunnel,
            tailscale,
            wg_client_key,
            geo_exit,
            binary,
            providers,
            env_file,
            point,
            json,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusArgs {
    host: String,
    policy: HostKeyPolicy,
    json: bool,
}

impl StatusArgs {
    fn parse(args: &[OsString]) -> Result<Self, CliError> {
        let mut host: Option<String> = None;
        let mut policy: Option<HostKeyPolicy> = None;
        let mut json = false;
        let mut words = args.iter();
        while let Some(word) = words.next() {
            match word.to_str().ok_or_else(not_utf8)? {
                "--json" => json = true,
                "--host" => once(&mut host, "--host", host_of(&mut words)?)?,
                "--host-keys" => once(
                    &mut policy,
                    "--host-keys",
                    parse_policy(&value_of(&mut words, "--host-keys")?)?,
                )?,
                other => return Err(unknown_flag(other)),
            }
        }
        let host = host.ok_or_else(|| CliError::Args {
            reason: "vps status needs --host user@host".to_owned(),
        })?;
        Ok(Self {
            host,
            policy: policy.unwrap_or(HostKeyPolicy::Strict),
            json,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpgradeArgs {
    host: String,
    policy: HostKeyPolicy,
    binary: PathBuf,
    json: bool,
}

impl UpgradeArgs {
    fn parse(args: &[OsString]) -> Result<Self, CliError> {
        let mut host: Option<String> = None;
        let mut policy: Option<HostKeyPolicy> = None;
        let mut binary: Option<PathBuf> = None;
        let mut json = false;
        let mut words = args.iter();
        while let Some(word) = words.next() {
            match word.to_str().ok_or_else(not_utf8)? {
                "--json" => json = true,
                "--host" => once(&mut host, "--host", host_of(&mut words)?)?,
                "--host-keys" => once(
                    &mut policy,
                    "--host-keys",
                    parse_policy(&value_of(&mut words, "--host-keys")?)?,
                )?,
                "--binary" => once(
                    &mut binary,
                    "--binary",
                    PathBuf::from(value_of(&mut words, "--binary")?),
                )?,
                other => return Err(unknown_flag(other)),
            }
        }
        let host = host.ok_or_else(|| CliError::Args {
            reason: "vps upgrade needs --host user@host".to_owned(),
        })?;
        let binary = binary.ok_or_else(|| CliError::Args {
            reason: "vps upgrade needs --binary with the local musl build".to_owned(),
        })?;
        Ok(Self {
            host,
            policy: policy.unwrap_or(HostKeyPolicy::Strict),
            binary,
            json,
        })
    }
}

#[derive(Debug, Serialize)]
struct StatusJson<'a> {
    host: &'a str,
    backend: &'a str,
    service: ServiceJson<'a>,
    disk_free_bytes: Option<u64>,
    loadavg: Option<&'a str>,
    mode: Option<&'a str>,
    address: Option<&'a str>,
    cert: Option<CertJson<'a>>,
    ledger: LedgerJson,
    notes: &'a [String],
}

#[derive(Debug, Serialize)]
struct ServiceJson<'a> {
    name: &'a str,
    active: bool,
}

#[derive(Debug, Serialize)]
struct CertJson<'a> {
    domain: &'a str,
    not_after: &'a str,
    days_remaining: i64,
}

#[derive(Debug, Serialize)]
struct LedgerJson {
    base: Option<String>,
    state: &'static str,
    window: &'static str,
    total_cost_usd: Option<String>,
    rows: usize,
    reason: Option<String>,
}

impl<'a> StatusJson<'a> {
    fn of(report: &'a core_vps::VpsStatusReport, ledger: &LedgerView) -> Self {
        Self {
            host: &report.host,
            backend: &report.backend,
            service: ServiceJson {
                name: &report.service.name,
                active: report.service.active,
            },
            disk_free_bytes: report.disk_free_bytes,
            loadavg: report.loadavg.as_deref(),
            mode: report.mode.as_deref(),
            address: report.address.as_deref(),
            cert: report.cert.as_ref().map(|cert| CertJson {
                domain: &cert.domain,
                not_after: &cert.not_after,
                days_remaining: cert.days_remaining,
            }),
            ledger: ledger.to_json(),
            notes: &report.notes,
        }
    }
}

/// The `/usage/summary` probe through the llmproxy client. Read-only; the
/// base comes from the operator's environment, never from a guess.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LedgerView {
    base: Option<String>,
    state: &'static str,
    total_cost_usd: Option<String>,
    rows: usize,
    reason: Option<String>,
}

impl LedgerView {
    fn probe(home: &Path) -> Self {
        let env_url = utf8_env(llmproxy::PROXY_URL_ENV_VAR);
        let base = match resolve_base_url(None, env_url.as_deref(), &[]) {
            Ok(Some(base)) => base,
            Ok(None) => {
                return Self::unconfigured();
            }
            Err(error) => {
                return Self {
                    base: None,
                    state: "refused",
                    total_cost_usd: None,
                    rows: 0,
                    reason: Some(error.to_string()),
                };
            }
        };
        let env_token = utf8_env(llmproxy::ADMIN_TOKEN_ENV_VAR);
        let token_file = home.join(".superai").join("llmproxy-admin-token");
        let token = match load_admin_token(env_token.as_deref(), Some(&token_file)) {
            Ok(token) => token,
            Err(error) => {
                return Self {
                    base: Some(base),
                    state: "unavailable",
                    total_cost_usd: None,
                    rows: 0,
                    reason: Some(error.to_string()),
                };
            }
        };
        let limits = PollLimits::new(Duration::from_secs(10), 1024 * 1024);
        let limits = match limits {
            Ok(limits) => limits,
            Err(error) => {
                return Self {
                    base: Some(base),
                    state: "unavailable",
                    total_cost_usd: None,
                    rows: 0,
                    reason: Some(error.to_string()),
                };
            }
        };
        match LedgerClient::new(&base, &token, limits) {
            Ok(client) => match client.summary(LLMPROXY_WINDOW, SummaryGroupBy::Provider) {
                Ok(summary) => Self {
                    base: Some(base),
                    state: "reachable",
                    total_cost_usd: summary.total_cost().map(|usd| usd.to_string()),
                    rows: summary.rows.len(),
                    reason: None,
                },
                Err(error) => Self {
                    base: Some(base),
                    state: "unreachable",
                    total_cost_usd: None,
                    rows: 0,
                    reason: Some(error.to_string()),
                },
            },
            Err(error) => Self {
                base: Some(base),
                state: "refused",
                total_cost_usd: None,
                rows: 0,
                reason: Some(error.to_string()),
            },
        }
    }

    fn unconfigured() -> Self {
        Self {
            base: None,
            state: "unconfigured",
            total_cost_usd: None,
            rows: 0,
            reason: Some(format!(
                "set {} (or point it at the deployment's admin listener) to read the ledger",
                llmproxy::PROXY_URL_ENV_VAR
            )),
        }
    }

    fn to_json(&self) -> LedgerJson {
        LedgerJson {
            base: self.base.clone(),
            state: self.state,
            window: LLMPROXY_WINDOW,
            total_cost_usd: self.total_cost_usd.clone(),
            rows: self.rows,
            reason: self.reason.clone(),
        }
    }
}

fn utf8_env(name: &str) -> Option<String> {
    std::env::var_os(name).and_then(|value| value.into_string().ok())
}

fn write_json(value: &impl Serialize) -> Result<(), CliError> {
    let text = serde_json::to_string_pretty(value).map_err(|e| CliError::Report {
        reason: format!("encoding failed: {e}"),
    })?;
    crate::write_stdout(&format!("{text}\n"))
}

#[derive(Debug, Serialize)]
struct DeployReportJson {
    plan: serde_json::Value,
    journal: serde_json::Value,
    pointing: Vec<serde_json::Value>,
}

fn deploy_report_json(
    plan: &DeployPlan,
    journal: &DeployJournal,
    pointing: &[core_vps::PointOutcome],
) -> DeployReportJson {
    DeployReportJson {
        plan: serde_json::to_value(PlanJson::of(plan)).unwrap_or(serde_json::Value::Null),
        journal: serde_json::to_value(journal).unwrap_or(serde_json::Value::Null),
        pointing: pointing
            .iter()
            .map(|outcome| {
                serde_json::json!({
                    "instance": outcome.instance,
                    "provider": outcome.provider,
                    "path": outcome.path.display().to_string(),
                    "applied": outcome.applied,
                })
            })
            .collect(),
    }
}

#[derive(Debug, Serialize)]
struct PlanJson<'a> {
    host: &'a str,
    backend: &'a str,
    service: &'a str,
    mode: &'a str,
    address: &'a str,
    geo_exit: Option<&'a str>,
    blocked: bool,
    preflight: Vec<CheckJson<'a>>,
    steps: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct CheckJson<'a> {
    name: &'a str,
    passed: bool,
    detail: &'a str,
}

impl<'a> PlanJson<'a> {
    fn of(plan: &'a DeployPlan) -> Self {
        Self {
            host: &plan.host,
            backend: &plan.backend,
            service: plan.service,
            mode: &plan.mode,
            address: &plan.address,
            geo_exit: plan.geo_exit.as_deref(),
            blocked: plan.blocked(),
            preflight: plan
                .preflight
                .checks
                .iter()
                .map(|check| CheckJson {
                    name: check.name,
                    passed: matches!(check.state, superai_core::vps::CheckState::Pass(_)),
                    detail: match &check.state {
                        superai_core::vps::CheckState::Pass(detail)
                        | superai_core::vps::CheckState::Fail(detail) => detail,
                    },
                })
                .collect(),
            steps: plan.steps.iter().map(step_json).collect(),
        }
    }
}

fn step_json(step: &superai_core::vps::PlannedStep) -> serde_json::Value {
    let mut value = serde_json::to_value(&step.step).unwrap_or(serde_json::Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "preview".to_owned(),
            serde_json::Value::String(preview_text(&step.preview)),
        );
    }
    value
}

fn preview_text(preview: &StepPreview) -> String {
    match preview {
        StepPreview::Create { path, content } => format!(
            "create {path}{}",
            content
                .as_ref()
                .map_or_else(String::new, |text| format!(":\n{text}"))
        ),
        StepPreview::Replace {
            path,
            existing,
            diff,
            content,
        } => format!(
            "replace {path} ({}):\n{diff}{}",
            match existing {
                superai_core::vps::Existing::Ours => "ours",
                superai_core::vps::Existing::Foreign => "foreign, backs up first",
            },
            content
                .as_ref()
                .map_or_else(String::new, |text| format!("\n{text}"))
        ),
        StepPreview::UnitInstall {
            path,
            backup_planned,
            diff,
        } => format!("install unit {path} (backup first: {backup_planned}):\n{diff}"),
        StepPreview::Run { argv } => format!("run `{}`", argv.join(" ")),
        StepPreview::DaemonsReload => "daemon-reload".to_owned(),
        StepPreview::Service { name, verb } => format!("{verb} service {name}"),
        StepPreview::Symlink { target, link } => format!("link {link} -> {target}"),
        StepPreview::InterpolateEnv { target, env } => {
            format!("fill ${{...}} in {target} from {env} on the host")
        }
    }
}

fn step_line(step: &DeployStep) -> String {
    match step {
        DeployStep::UploadFile {
            path,
            mode,
            bytes,
            secret,
        } => format!(
            "upload {path} ({bytes} bytes, mode {mode:o}{})",
            if *secret { ", secret" } else { "" }
        ),
        DeployStep::InstallUnit { name } => format!("install unit {name}.service"),
        DeployStep::ReloadDaemons => "daemon-reload".to_owned(),
        DeployStep::Exec { argv } => format!("run `{}`", argv.join(" ")),
        DeployStep::EnableService { name } => format!("enable {name}.service"),
        DeployStep::StartService { name } => format!("start {name}.service"),
        DeployStep::RestartService { name } => format!("restart {name}.service"),
        DeployStep::Symlink { target, link } => format!("link {link} -> {target}"),
        DeployStep::InterpolateEnv { target, env } => {
            format!("interpolate {target} from {env}")
        }
    }
}

fn render_plan(plan: &DeployPlan) -> String {
    let mut lines = vec![format!(
        "plan: {} on {} via {}",
        plan.service, plan.host, plan.backend
    )];
    for check in &plan.preflight.checks {
        let (verdict, detail) = match &check.state {
            superai_core::vps::CheckState::Pass(detail) => ("pass", detail.clone()),
            superai_core::vps::CheckState::Fail(detail) => ("FAIL", detail.clone()),
        };
        lines.push(format!("  {}: {verdict}: {detail}", check.name));
    }
    lines.push(format!(
        "ingress: {}{}; address: {}",
        plan.mode,
        plan.geo_exit
            .as_deref()
            .map_or_else(String::new, |geo| format!(" (geo-exit {geo})")),
        plan.address
    ));
    if plan.blocked() {
        lines.push("blocked: preflight failed; nothing will run".to_owned());
    }
    for (index, step) in plan.steps.iter().enumerate() {
        lines.push(format!("  {}. {}", index + 1, step_line(&step.step)));
        for line in preview_text(&step.preview)
            .lines()
            .filter(|line| !line.is_empty())
        {
            lines.push(format!("     {line}"));
        }
    }
    crate::finish_lines(lines)
}

fn render_journal(journal: &DeployJournal) -> String {
    let mut lines = vec![format!("journal: committed via {}", journal.backend)];
    for entry in &journal.entries {
        lines.push(format!(
            "  done: {}: {}",
            step_line(&entry.step),
            entry.detail
        ));
    }
    if !journal.backups.is_empty() {
        lines.push("backups:".to_owned());
        for record in &journal.backups {
            lines.push(format!("  {} -> {}", record.path, record.backup_path));
        }
    }
    if let Some(rollback) = &journal.rolled_back {
        lines.push(format!(
            "rolled back at: {}: {}",
            step_line(&rollback.failed_step),
            rollback.reason
        ));
        for note in &rollback.notes {
            lines.push(format!("  rollback: {note}"));
        }
    }
    crate::finish_lines(lines)
}

fn render_status(report: &core_vps::VpsStatusReport, ledger: &LedgerView) -> String {
    let mut lines = vec![format!(
        "status: {} on {} via {}",
        report.service.name, report.host, report.backend
    )];
    lines.push(format!(
        "service: {}",
        if report.service.active {
            "active"
        } else {
            "inactive"
        }
    ));
    lines.push(format!(
        "disk: {}",
        report.disk_free_bytes.map_or_else(
            || "unknown".to_owned(),
            |bytes| format!("{bytes} bytes free")
        )
    ));
    lines.push(format!(
        "load: {}",
        report.loadavg.as_deref().unwrap_or("unknown")
    ));
    if let Some(mode) = &report.mode {
        lines.push(format!(
            "deployed: {mode} at {}",
            report.address.as_deref().unwrap_or("unknown address")
        ));
    }
    if let Some(cert) = &report.cert {
        lines.push(format!(
            "cert: {} expires {} ({} days)",
            cert.domain, cert.not_after, cert.days_remaining
        ));
    }
    lines.push(format!(
        "ledger: {} (window {LLMPROXY_WINDOW}){}{}",
        ledger.state,
        ledger
            .total_cost_usd
            .as_deref()
            .map_or_else(String::new, |usd| format!(" total ${usd}")),
        if ledger.rows == 0 {
            String::new()
        } else {
            format!(" across {} rows", ledger.rows)
        },
    ));
    if let Some(reason) = &ledger.reason {
        lines.push(format!("  ledger note: {reason}"));
    }
    for note in &report.notes {
        lines.push(format!("note: {note}"));
    }
    crate::finish_lines(lines)
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn err_text(result: Result<impl Sized, CliError>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn deploy_args_need_host_binary_and_one_ingress() {
        let parsed = DeployArgs::parse(&os(&[
            "--host",
            "root@h",
            "--binary",
            "/tmp/llmproxy",
            "--domain",
            "proxy.example.com",
            "--email",
            "ops@example.com",
        ]))
        .unwrap_or_else(|e| panic!("parse: {e}"));
        assert_eq!(parsed.host, "root@h");
        assert_eq!(parsed.policy, HostKeyPolicy::Strict);
        assert_eq!(parsed.domain.as_deref(), Some("proxy.example.com"));
        assert!(!parsed.tunnel);

        assert!(err_text(DeployArgs::parse(&os(&["--binary", "/tmp/x"]))).contains("needs --host"));
        assert!(err_text(DeployArgs::parse(&os(&["--host", "root@h"]))).contains("needs --binary"));
        assert!(
            err_text(DeployArgs::parse(&os(&[
                "--host",
                "root@h",
                "--binary",
                "/tmp/x",
                "--domain",
                "d.example.com",
            ])))
            .contains("go together")
        );
        assert!(
            err_text(DeployArgs::parse(&os(&[
                "--host",
                "root@h",
                "--binary",
                "/tmp/x",
                "--tunnel",
                "--tailscale",
                "ts-host",
            ])))
            .contains("pick one ingress")
        );
        assert!(
            err_text(DeployArgs::parse(&os(&[
                "--host", "root@h", "--binary", "/tmp/x", "--tunnel",
            ])))
            .contains("--wg-client-key")
        );
        assert!(err_text(DeployArgs::parse(&os(&["--wat"]))).contains("unknown flag"));
        assert!(
            err_text(DeployArgs::parse(&os(&[
                "--host", "root@h", "--host", "root@h2"
            ])))
            .contains("given twice")
        );
    }

    #[test]
    fn host_key_policy_off_is_refused() {
        let text = err_text(DeployArgs::parse(&os(&[
            "--host",
            "root@h",
            "--binary",
            "/tmp/x",
            "--tunnel",
            "--wg-client-key",
            "k=",
            "--host-keys",
            "off",
        ])));
        assert!(text.contains("never accepted"), "{text}");
        let ok = DeployArgs::parse(&os(&[
            "--host",
            "root@h",
            "--binary",
            "/tmp/x",
            "--tunnel",
            "--wg-client-key",
            "k=",
            "--host-keys",
            "accept-new",
        ]));
        assert_eq!(
            ok.unwrap_or_else(|e| panic!("parse: {e}")).policy,
            HostKeyPolicy::AcceptNew
        );
    }

    #[test]
    fn deploy_args_take_repeatable_point_and_provider_flags() {
        let parsed = DeployArgs::parse(&os(&[
            "--host",
            "root@h",
            "--binary",
            "/tmp/x",
            "--domain",
            "d.example.com",
            "--email",
            "ops@example.com",
            "--point",
            "work",
            "--point",
            "scratch",
        ]))
        .unwrap_or_else(|e| panic!("parse: {e}"));
        assert_eq!(parsed.point, vec!["work".to_owned(), "scratch".to_owned()]);
        assert!(
            err_text(DeployArgs::parse(&os(&[
                "--host",
                "root@h",
                "--binary",
                "/tmp/x",
                "--domain",
                "d.example.com",
                "--email",
                "ops@example.com",
                "--point"
            ])))
            .contains("needs a value")
        );
    }

    #[test]
    fn status_and_upgrade_args_parse_their_surface() {
        let status = StatusArgs::parse(&os(&["--host", "root@h", "--json"]))
            .unwrap_or_else(|e| panic!("parse: {e}"));
        assert_eq!(status.host, "root@h");
        assert!(status.json);
        assert!(err_text(StatusArgs::parse(&[])).contains("needs --host"));

        let upgrade = UpgradeArgs::parse(&os(&["--host", "root@h", "--binary", "/tmp/x"]))
            .unwrap_or_else(|e| panic!("parse: {e}"));
        assert_eq!(upgrade.binary, PathBuf::from("/tmp/x"));
        assert!(
            err_text(UpgradeArgs::parse(&os(&["--host", "root@h"]))).contains("needs --binary")
        );
    }

    #[test]
    fn dispatch_rejects_unknown_verbs_and_services() {
        assert!(err_text(run(&os(&["frobnicate"]), Path::new("/tmp"))).contains("deploy, status"));
        assert!(err_text(run(&os(&[]), Path::new("/tmp"))).contains("needs a verb"));
        assert!(err_text(run(&os(&["deploy"]), Path::new("/tmp"))).contains("needs a service"));
        assert!(err_text(run(&os(&["deploy", "caddy"]), Path::new("/tmp"))).contains("llmproxy"));
    }

    #[test]
    fn provider_files_are_read_fresh_with_their_stem_as_name() {
        let dir = std::env::temp_dir().join(format!("superai-cli-vps-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("anthropic.toml");
        std::fs::write(&file, "api_key = \"PLACEHOLDER\"\n").unwrap();
        let provider = read_provider(&file).unwrap_or_else(|e| panic!("read: {e}"));
        assert_eq!(provider.name, "anthropic");
        assert!(provider.content.contains("PLACEHOLDER"));
        let missing = err_text(read_provider(&dir.join("absent.toml")));
        assert!(missing.contains("unreadable"), "{missing}");
        drop(std::fs::remove_dir_all(&dir));
    }

    #[test]
    fn ledger_view_reports_the_summary_through_a_local_mock() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let body = concat!(
                r#"{"window":"30d","rows":[{"group":"anthropic","requests":2,"errors":0,"#,
                r#""input_tokens":10,"output_tokens":5,"cache_read_tokens":0,"#,
                r#""cache_write_tokens":null,"cost_usd":"0.01"}]}"#
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            drop(stream.write_all(response.as_bytes()));
            drop(stream.flush());
            request
        });
        let base = format!("http://127.0.0.1:{port}");
        let limits = PollLimits::new(Duration::from_secs(5), 64 * 1024).unwrap();
        let token = llmproxy::AdminToken::new("fixture-admin-token");
        let client = LedgerClient::new(&base, &token, limits).unwrap();
        let summary = client
            .summary(LLMPROXY_WINDOW, SummaryGroupBy::Provider)
            .unwrap_or_else(|e| panic!("summary: {e}"));
        let request = thread.join().unwrap_or_default();
        assert!(
            request.contains("GET /usage/summary") && request.contains("Bearer "),
            "{request}"
        );
        assert_eq!(summary.rows.len(), 1);
        assert_eq!(
            summary.total_cost().map(|usd| usd.to_string()),
            Some("0.01".to_owned())
        );
    }

    #[test]
    fn renderers_keep_secrets_and_pending_gates_visible() {
        let plan_line = step_line(&DeployStep::UploadFile {
            path: "/etc/llmproxy/llmproxy.env".to_owned(),
            mode: 0o600,
            bytes: 42,
            secret: true,
        });
        assert!(plan_line.contains("secret"), "{plan_line}");
        assert!(!plan_line.contains("PLACEHOLDER"));

        let preview = preview_text(&StepPreview::Create {
            path: "/etc/llmproxy/llmproxy.env".to_owned(),
            content: None,
        });
        assert!(!preview.contains("token"), "{preview}");

        let view = LedgerView::unconfigured();
        assert_eq!(view.state, "unconfigured");
        assert!(
            view.reason
                .as_deref()
                .is_some_and(|reason| reason.contains("SUPERAI_LLMPROXY_URL"))
        );
    }
}
