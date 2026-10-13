//! Deploy execution (VPS-02): commits the previewed steps in order, backs up
//! every foreign file before overwrite, and rolls back completed steps on a
//! mid-sequence failure, journaling everything it did.

use serde::Serialize;
use std::path::PathBuf;

use crate::vps::plan::{
    DeployPlan, DeployStep, Existing, Payload, PlannedStep, StepPreview, preflight, validate_host,
};
use crate::vps::remote::{HostKeyPolicy, RemoteError};
use crate::vps::render::{REMOTE_BIN_PATH, REMOTE_MANIFEST_PATH, REMOTE_UNIT_PATH, SERVICE_NAME};
use crate::vps::{RemoteHost, VpsError, validation};

/// A backup taken before overwriting a remote file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupRecord {
    /// Path that was backed up.
    pub path: String,
    /// Where the backup landed.
    pub backup_path: String,
    /// Mode bits the file carried before the overwrite.
    pub mode: u32,
    /// Whether superai's manifest owned the file it replaced.
    pub origin: Existing,
}

/// What one step did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JournalEntry {
    /// The committed step.
    pub step: DeployStep,
    /// Outcome detail; byte counts only, never payload content.
    pub detail: String,
}

/// Why and how the run rolled back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RollbackInfo {
    /// The step that failed.
    pub failed_step: DeployStep,
    /// Why it failed; carries no payload bytes.
    pub reason: String,
    /// Restore actions taken, in reverse commit order.
    pub notes: Vec<String>,
}

/// The stepwise record of one deploy or upgrade.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeployJournal {
    /// Backend and target the run used.
    pub backend: String,
    /// Steps that committed successfully.
    pub entries: Vec<JournalEntry>,
    /// Backups taken before overwrites.
    pub backups: Vec<BackupRecord>,
    /// Present only when the run failed and rolled back.
    pub rolled_back: Option<RollbackInfo>,
}

/// Request for an in-place binary upgrade of a superai-managed deployment.
#[derive(Debug, Clone)]
pub struct UpgradeRequest {
    /// `user@host` ssh target.
    pub host: String,
    /// Local musl binary that replaces the remote one.
    pub binary_path: PathBuf,
    /// Host-key acceptance policy.
    pub policy: HostKeyPolicy,
}

/// Commit `plan` stepwise. `Ok(journal)` covers both outcomes: a full run,
/// or a failure whose rollback succeeded (`journal.rolled_back` is `Some`).
/// `Err` means the run failed AND rollback could not restore the host.
pub fn execute_deploy(
    plan: &DeployPlan,
    remote: &dyn RemoteHost,
) -> Result<DeployJournal, VpsError> {
    if plan.blocked()
        && let Some(failure) = plan.preflight.first_failure()
    {
        return Err(validation("preflight", failure));
    }
    let owned = manifest_paths(remote)?;
    let mut run = Run {
        backend: remote.describe(),
        entries: Vec::new(),
        backups: Vec::new(),
        created: Vec::new(),
        started: Vec::new(),
        restarted: Vec::new(),
        unit_touched: false,
    };
    match commit_all(&plan.steps, remote, &mut run, &owned) {
        Ok(()) => Ok(finish(run, None)),
        Err((step, reason)) => {
            let notes = rollback(&run, remote)?;
            Ok(finish(
                run,
                Some(RollbackInfo {
                    failed_step: step,
                    reason,
                    notes,
                }),
            ))
        }
    }
}

/// Plan an upgrade: replace the binary on a deployment the remote manifest
/// proves superai owns, then restart the service. A host without the
/// manifest is refused, never adopted.
pub fn plan_upgrade(
    request: &UpgradeRequest,
    remote: &dyn RemoteHost,
) -> Result<DeployPlan, VpsError> {
    validate_host(&request.host)?;
    let manifest = remote.read_file(REMOTE_MANIFEST_PATH)?;
    let Some(bytes) = manifest else {
        return Err(VpsError::RemoteState {
            path: REMOTE_MANIFEST_PATH.to_owned(),
            reason: "no superai manifest on the host; deploy first".to_owned(),
        });
    };
    let parsed: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| VpsError::RemoteState {
            path: REMOTE_MANIFEST_PATH.to_owned(),
            reason: format!("manifest unparseable: {e}"),
        })?;
    if parsed.get("managed_by").and_then(serde_json::Value::as_str) != Some("superai") {
        return Err(VpsError::RemoteState {
            path: REMOTE_MANIFEST_PATH.to_owned(),
            reason: "manifest is not superai's; refusing to touch a foreign deployment".to_owned(),
        });
    }
    if remote.read_file(REMOTE_BIN_PATH)?.is_none() {
        return Err(VpsError::RemoteState {
            path: REMOTE_BIN_PATH.to_owned(),
            reason: "no deployed binary to upgrade".to_owned(),
        });
    }
    let binary = crate::vps::plan::read_binary(&request.binary_path)?;
    let echo = |key: &str| {
        parsed
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let steps = vec![
        PlannedStep {
            step: DeployStep::UploadFile {
                path: REMOTE_BIN_PATH.to_owned(),
                mode: 0o755,
                bytes: binary.len(),
                secret: false,
            },
            preview: StepPreview::Replace {
                path: REMOTE_BIN_PATH.to_owned(),
                existing: Existing::Ours,
                diff: format!("+binary payload ({} bytes)", binary.len()),
                content: None,
            },
            payload: Payload::Upload {
                bytes: binary,
                mode: 0o755,
                secret: false,
            },
        },
        PlannedStep {
            step: DeployStep::RestartService {
                name: SERVICE_NAME.to_owned(),
            },
            preview: StepPreview::Service {
                name: SERVICE_NAME.to_owned(),
                verb: "restart",
            },
            payload: Payload::Bare,
        },
    ];
    let pre = preflight(&request.host, request.policy, remote)?;
    let address = echo("address");
    let mode = echo("mode");
    Ok(DeployPlan {
        host: request.host.clone(),
        backend: remote.describe(),
        service: SERVICE_NAME,
        mode: "upgrade".to_owned(),
        harness_base: crate::vps::render::harness_base_url(&mode, &address),
        point_instances: Vec::new(),
        address,
        geo_exit: None,
        preflight: pre,
        steps,
    })
}

/// Execute an upgrade plan through the same commit/rollback machinery.
pub fn execute_upgrade(
    plan: &DeployPlan,
    remote: &dyn RemoteHost,
) -> Result<DeployJournal, VpsError> {
    execute_deploy(plan, remote)
}

struct Run {
    backend: String,
    entries: Vec<JournalEntry>,
    backups: Vec<BackupRecord>,
    created: Vec<String>,
    started: Vec<String>,
    restarted: Vec<String>,
    unit_touched: bool,
}

fn finish(run: Run, rolled_back: Option<RollbackInfo>) -> DeployJournal {
    DeployJournal {
        backend: run.backend,
        entries: run.entries,
        backups: run.backups,
        rolled_back,
    }
}

fn commit_all(
    steps: &[PlannedStep],
    remote: &dyn RemoteHost,
    run: &mut Run,
    owned: &[String],
) -> Result<(), (DeployStep, String)> {
    for planned in steps {
        commit_one(planned, remote, run, owned).map_err(|reason| (planned.step.clone(), reason))?;
    }
    Ok(())
}

fn commit_one(
    planned: &PlannedStep,
    remote: &dyn RemoteHost,
    run: &mut Run,
    owned: &[String],
) -> Result<(), String> {
    let fail = |error: RemoteError| VpsError::from(error).to_string();
    match (&planned.step, &planned.payload) {
        (
            DeployStep::UploadFile {
                path, mode, secret, ..
            },
            Payload::Upload { bytes, .. },
        ) => {
            backup_or_track(path, remote, run, owned).map_err(fail)?;
            remote.upload(path, bytes, *mode).map_err(fail)?;
            run.entries.push(JournalEntry {
                step: planned.step.clone(),
                detail: if *secret {
                    format!("uploaded {} bytes at mode {mode:o} (secret)", bytes.len())
                } else {
                    format!("uploaded {} bytes at mode {mode:o}", bytes.len())
                },
            });
        }
        (DeployStep::InstallUnit { name }, Payload::Unit { content }) => {
            commit_unit(name, content, planned, remote, run, owned).map_err(fail)?;
        }
        (DeployStep::ReloadDaemons, Payload::Bare) => {
            remote.daemon_reload().map_err(fail)?;
            run.entries.push(JournalEntry {
                step: planned.step.clone(),
                detail: "daemon-reload completed".to_owned(),
            });
        }
        (DeployStep::Exec { argv }, Payload::Bare) => {
            let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
            let output = remote.exec(&borrowed).map_err(fail)?;
            if !output.success() {
                return Err(format!(
                    "`{}` exited {:?}",
                    argv.first().map_or("", String::as_str),
                    output.exit
                ));
            }
            run.entries.push(JournalEntry {
                step: planned.step.clone(),
                detail: format!("`{}` exited 0", argv.first().map_or("", String::as_str)),
            });
        }
        (DeployStep::EnableService { name }, Payload::Bare) => {
            remote.service_enable(name).map_err(fail)?;
            run.entries.push(JournalEntry {
                step: planned.step.clone(),
                detail: format!("{name} enabled at boot"),
            });
        }
        (DeployStep::StartService { name }, Payload::Bare) => {
            // A service already running before this run was not put up by
            // it; rollback must stop only what the run itself started, or
            // a failed redeploy takes a healthy deployment down.
            let was_active = remote.service_is_active(name).map_err(fail)?;
            remote.service_start(name).map_err(fail)?;
            if !was_active {
                run.started.push(name.clone());
            }
            run.entries.push(JournalEntry {
                step: planned.step.clone(),
                detail: if was_active {
                    format!("{name} already active; start reconfirmed it")
                } else {
                    format!("{name} started")
                },
            });
        }
        (DeployStep::RestartService { name }, Payload::Bare) => {
            remote.service_restart(name).map_err(fail)?;
            run.restarted.push(name.clone());
            run.entries.push(JournalEntry {
                step: planned.step.clone(),
                detail: format!("{name} restarted"),
            });
        }
        (DeployStep::Symlink { target, link }, Payload::Bare) => {
            backup_or_track(link, remote, run, owned).map_err(fail)?;
            remote.symlink(target, link).map_err(fail)?;
            run.entries.push(JournalEntry {
                step: planned.step.clone(),
                detail: format!("linked {link} -> {target}"),
            });
        }
        (DeployStep::InterpolateEnv { target, env }, Payload::Bare) => {
            remote.interpolate_env(target, env).map_err(fail)?;
            run.entries.push(JournalEntry {
                step: planned.step.clone(),
                detail: format!(
                    "filled placeholders in {target} from {env} on the host (values never \
                     printed)"
                ),
            });
        }
        (step, payload) => {
            return Err(format!(
                "plan carried a step/payload mismatch: {step:?} with {payload:?}"
            ));
        }
    }
    Ok(())
}

/// Install the unit backup-first: a pre-existing unit is backed up at its
/// original mode, a fresh host records the unit as created so rollback
/// removes it.
fn commit_unit(
    name: &str,
    content: &str,
    planned: &PlannedStep,
    remote: &dyn RemoteHost,
    run: &mut Run,
    owned: &[String],
) -> Result<(), RemoteError> {
    let origin = if owned.contains(&REMOTE_UNIT_PATH.to_owned()) {
        Existing::Ours
    } else {
        Existing::Foreign
    };
    let mode_before = remote.file_mode(REMOTE_UNIT_PATH)?.unwrap_or(0o644);
    match remote.install_unit(name, content)? {
        Some(backup_path) => run.backups.push(BackupRecord {
            path: REMOTE_UNIT_PATH.to_owned(),
            backup_path,
            mode: mode_before,
            origin,
        }),
        None => run.created.push(REMOTE_UNIT_PATH.to_owned()),
    }
    run.unit_touched = true;
    run.entries.push(JournalEntry {
        step: planned.step.clone(),
        detail: format!("installed {name}.service with pre-write backup"),
    });
    Ok(())
}

/// Back up a file that exists before overwrite (recording its mode and
/// origin), or mark a path this run is creating for rollback removal.
fn backup_or_track(
    path: &str,
    remote: &dyn RemoteHost,
    run: &mut Run,
    owned: &[String],
) -> Result<(), RemoteError> {
    if remote.read_file(path)?.is_none() {
        run.created.push(path.to_owned());
        return Ok(());
    }
    let mode_before = remote.file_mode(path)?.unwrap_or(0o644);
    let origin = if owned.contains(&path.to_owned()) {
        Existing::Ours
    } else {
        Existing::Foreign
    };
    if let Some(backup_path) = remote.backup_file(path)? {
        run.backups.push(BackupRecord {
            path: path.to_owned(),
            backup_path,
            mode: mode_before,
            origin,
        });
    }
    Ok(())
}

/// Restore the host: backups return at their original modes, files this run
/// created are removed, a service this run started or restarted is stopped
/// or restarted back onto the restored binary.
fn rollback(run: &Run, remote: &dyn RemoteHost) -> Result<Vec<String>, VpsError> {
    let mut notes = Vec::new();
    for backup in run.backups.iter().rev() {
        let Some(content) = remote.read_file(&backup.backup_path)? else {
            return Err(VpsError::RemoteState {
                path: backup.backup_path.clone(),
                reason: "backup vanished before rollback could restore it".to_owned(),
            });
        };
        remote.upload(&backup.path, &content, backup.mode)?;
        notes.push(format!(
            "restored {} from {} (mode {:o})",
            backup.path, backup.backup_path, backup.mode
        ));
    }
    for path in run.created.iter().rev() {
        remote.remove_file(path)?;
        notes.push(format!("removed {path} (created by this run)"));
    }
    if !run.backups.is_empty() || run.unit_touched {
        remote.daemon_reload()?;
        notes.push("daemon-reload completed after rollback".to_owned());
    }
    for name in &run.started {
        remote.service_stop(name)?;
        notes.push(format!("{name} stopped (started by this run)"));
    }
    for name in &run.restarted {
        remote.service_restart(name)?;
        notes.push(format!("{name} restarted onto the restored binary"));
    }
    Ok(notes)
}

fn manifest_paths(remote: &dyn RemoteHost) -> Result<Vec<String>, VpsError> {
    let Some(bytes) = remote.read_file(REMOTE_MANIFEST_PATH)? else {
        return Ok(Vec::new());
    };
    let parsed: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| VpsError::RemoteState {
            path: REMOTE_MANIFEST_PATH.to_owned(),
            reason: format!("manifest unparseable: {e}"),
        })?;
    if parsed.get("managed_by").and_then(serde_json::Value::as_str) != Some("superai") {
        return Err(VpsError::RemoteState {
            path: REMOTE_MANIFEST_PATH.to_owned(),
            reason: "a foreign manifest exists at the superai path; refusing to deploy over it"
                .to_owned(),
        });
    }
    Ok(parsed
        .get("paths")
        .and_then(serde_json::Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(|path| path.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::abuse;
    use crate::vps::plan::{DeployRequest, IngressMode, ProviderToml, TunnelKind, plan_deploy};
    use crate::vps::remote::{Fault, FixtureHost, FixtureRemote, HostKeyPolicy as Policy};
    use crate::vps::render::{
        REMOTE_ENV_PATH, REMOTE_NGINX_SITE_PATH, REMOTE_SERVER_CONF_PATH, REMOTE_WG_CONF_PATH,
    };

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("superai-vps-exec-{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tls_request(dir: &Path, secret_env: &str) -> DeployRequest {
        let binary = dir.join("llmproxy");
        std::fs::write(&binary, b"musl-bytes").unwrap();
        let env_file = dir.join("secrets.env");
        std::fs::write(&env_file, secret_env).unwrap();
        DeployRequest {
            host: "root@203.0.113.10".to_owned(),
            point_instances: Vec::new(),
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
            policy: Policy::Strict,
        }
    }

    fn sentinel_env() -> String {
        format!(
            "LLMPROXY_ADMIN_TOKEN={}\nLLMPROXY_CLIENT_KEY_ID=workstation\nLLMPROXY_CLIENT_KEY_SECRET=client-secret\n",
            abuse::SENTINEL
        )
    }

    #[test]
    fn full_deploy_commits_every_step_and_starts_the_service() {
        let dir = temp_dir("full");
        let remote = FixtureRemote::new("root@203.0.113.10");
        let plan = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        let journal = execute_deploy(&plan, &remote).unwrap();
        assert!(journal.rolled_back.is_none(), "{journal:?}");
        assert!(remote.service_active("llmproxy"));
        assert!(remote.reload_count() >= 1);
        assert_eq!(remote.file_mode(REMOTE_ENV_PATH), Some(0o600));
        assert!(remote.file(REMOTE_MANIFEST_PATH).is_some_and(|bytes| {
            String::from_utf8_lossy(&bytes).contains("\"managed_by\":\"superai\"")
        }));
        let manifest = remote.file(REMOTE_MANIFEST_PATH).unwrap();
        assert!(String::from_utf8_lossy(&manifest).contains("llmproxy.env"));
    }

    #[test]
    fn journal_and_previews_never_carry_the_secret() {
        let dir = temp_dir("secret");
        let remote = FixtureRemote::new("root@host");
        let plan = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        abuse::assert_no_sentinel_in_debug(&plan, "deploy plan");
        let journal = execute_deploy(&plan, &remote).unwrap();
        abuse::assert_no_sentinel_in_debug(&journal, "deploy journal");
        abuse::assert_no_sentinel_in_json(&journal, "deploy journal json");
        let rendered = serde_json::to_string(&journal).unwrap();
        abuse::assert_no_sentinel_bytes(rendered.as_bytes(), "journal bytes");
        let env_preview = plan
            .steps
            .iter()
            .map(|step| format!("{:?}", step.preview))
            .collect::<Vec<_>>()
            .join("\n");
        abuse::assert_no_sentinel_bytes(env_preview.as_bytes(), "step previews");
    }

    #[test]
    fn foreign_unit_and_site_are_backed_up_before_overwrite() {
        let dir = temp_dir("foreign-exec");
        let remote = FixtureRemote::new("root@host");
        remote.insert_file(
            REMOTE_UNIT_PATH,
            b"[Unit]\nDescription=foreign-unit\n",
            0o644,
        );
        remote.insert_file(REMOTE_NGINX_SITE_PATH, b"server { foreign }\n", 0o644);
        let plan = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        let journal = execute_deploy(&plan, &remote).unwrap();
        assert!(journal.rolled_back.is_none());
        let backed_up: Vec<&str> = journal
            .backups
            .iter()
            .map(|record| record.path.as_str())
            .collect();
        assert!(backed_up.contains(&REMOTE_UNIT_PATH), "{backed_up:?}");
        assert!(backed_up.contains(&REMOTE_NGINX_SITE_PATH), "{backed_up:?}");
        for record in &journal.backups {
            assert!(remote.file(&record.backup_path).is_some());
        }
        assert!(
            remote
                .file(REMOTE_UNIT_PATH)
                .is_some_and(|bytes| String::from_utf8_lossy(&bytes).contains("DynamicUser=yes"))
        );
    }

    #[test]
    fn mid_sequence_failure_rolls_back_created_files_and_restores_backups() {
        let dir = temp_dir("rollback");
        let remote = FixtureRemote::new("root@host");
        let foreign_unit = b"[Unit]\nDescription=foreign-unit\n";
        remote.insert_file(REMOTE_UNIT_PATH, foreign_unit, 0o644);
        let plan = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        let step_count = plan.steps.len();
        remote.fail_next(Fault::Enable);
        let journal = execute_deploy(&plan, &remote).unwrap();
        let rollback = journal.rolled_back.as_ref().unwrap();
        assert!(
            rollback.reason.contains("injected fixture fault"),
            "{}",
            rollback.reason
        );
        assert!(
            !remote.service_active("llmproxy"),
            "rollback must stop the service"
        );
        assert_eq!(
            remote.file(REMOTE_UNIT_PATH),
            Some(foreign_unit.to_vec()),
            "the foreign unit returns untouched"
        );
        assert!(remote.file(REMOTE_BIN_PATH).is_none(), "created files go");
        assert!(remote.file(REMOTE_ENV_PATH).is_none());
        assert!(remote.file(REMOTE_MANIFEST_PATH).is_none());
        assert!(
            !journal.entries.is_empty() && journal.entries.len() < step_count,
            "partial progress is journaled"
        );
        abuse::assert_no_sentinel_in_debug(&journal, "rollback journal");
    }

    #[test]
    fn a_failed_redeploy_leaves_a_pre_existing_service_running() {
        let dir = temp_dir("redeploy-rollback");
        let remote = FixtureRemote::new("root@host");
        let first = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        execute_deploy(&first, &remote).unwrap();
        assert!(remote.service_active(SERVICE_NAME));

        let mut second_request = tls_request(&dir, &sentinel_env());
        let v2 = dir.join("llmproxy-v2");
        std::fs::write(&v2, b"musl-bytes-v2").unwrap();
        second_request.binary_path = v2;
        let second = plan_deploy(&second_request, &remote).unwrap();
        // The manifest upload is the one step after the service start, so
        // failing exactly it puts the failure after StartService committed
        // onto a service the first deploy left running.
        remote.fail_next(Fault::UploadTo(REMOTE_MANIFEST_PATH.to_owned()));
        let journal = execute_deploy(&second, &remote).unwrap();
        let rollback = journal.rolled_back.as_ref().unwrap();
        assert!(
            rollback.reason.contains("injected fixture fault"),
            "{}",
            rollback.reason
        );
        assert!(
            remote.service_active(SERVICE_NAME),
            "a service that was healthy before the run must survive its rollback"
        );
        assert_eq!(
            remote.file(REMOTE_BIN_PATH),
            Some(b"musl-bytes".to_vec()),
            "files still roll back to the pre-deploy state"
        );
        assert!(
            rollback.notes.iter().all(|note| !note.contains("stopped")),
            "no service may be stopped by this rollback: {:?}",
            rollback.notes
        );
        let start_detail = journal.entries.iter().find_map(|entry| match &entry.step {
            DeployStep::StartService { name } if name == SERVICE_NAME => {
                Some(entry.detail.as_str())
            }
            _ => None,
        });
        assert_eq!(
            start_detail,
            Some("llmproxy already active; start reconfirmed it"),
            "the start step must record the pre-existing state"
        );
    }

    #[test]
    fn a_foreign_enabled_link_is_backed_up_and_rollback_restores_it() {
        let dir = temp_dir("link-rollback");
        let remote = FixtureRemote::new("root@host");
        remote.insert_file(
            "/etc/nginx/sites-enabled/llmproxy.conf",
            b"/etc/nginx/sites-available/other.conf",
            0o777,
        );
        let plan = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        remote.fail_next(Fault::Enable);
        let journal = execute_deploy(&plan, &remote).unwrap();
        assert!(journal.rolled_back.is_some());
        assert!(
            journal
                .backups
                .iter()
                .any(|record| record.path == "/etc/nginx/sites-enabled/llmproxy.conf"),
            "{:?}",
            journal.backups
        );
        assert_eq!(
            remote.file("/etc/nginx/sites-enabled/llmproxy.conf"),
            Some(b"/etc/nginx/sites-available/other.conf".to_vec()),
            "rollback restores the link the preview announced replacing"
        );
    }

    #[test]
    fn tunnel_deploy_binds_the_proxy_to_the_wireguard_interface_with_auth() {
        let dir = temp_dir("tunnel-complete");
        let remote = FixtureRemote::new("root@host");
        let plan = plan_deploy(&wg_request(&dir), &remote).unwrap();
        assert!(plan.steps.iter().any(|step| matches!(
            &step.step,
            DeployStep::EnableService { name } if name == "wg-quick@wg0"
        )));
        assert!(plan.steps.iter().any(|step| matches!(
            &step.step,
            DeployStep::StartService { name } if name == "wg-quick@wg0"
        )));
        let journal = execute_deploy(&plan, &remote).unwrap();
        assert!(journal.rolled_back.is_none(), "{journal:?}");
        assert!(remote.service_active("wg-quick@wg0"), "the tunnel comes up");
        assert!(remote.service_active(SERVICE_NAME));

        let conf = remote.file(REMOTE_SERVER_CONF_PATH).unwrap();
        let text = String::from_utf8(conf).unwrap();
        assert!(text.contains("bind = \"10.7.0.1:8080\""), "{text}");
        assert!(text.contains("admin_bind = \"10.7.0.1:8081\""), "{text}");
        assert!(text.contains("[auth.clients]"), "{text}");
        assert!(
            text.contains("\"workstation\" = \"client-secret\""),
            "{text}"
        );
        assert!(
            text.contains("LLMPROXY_ADMIN_TOKEN=x") || text.contains("fixture-admin"),
            "{text}"
        );
        assert!(!text.contains("auth = \"none\""), "{text}");
        assert!(
            !text.contains("${"),
            "interpolation filled every placeholder: {text}"
        );
        abuse::assert_no_sentinel_bytes(format!("{remote:?}").as_bytes(), "fixture backend debug");
    }

    #[test]
    fn tls_deploy_configures_client_keys_behind_the_public_site() {
        let dir = temp_dir("tls-auth");
        let remote = FixtureRemote::new("root@host");
        let plan = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        execute_deploy(&plan, &remote).unwrap();
        let conf = remote.file(REMOTE_SERVER_CONF_PATH).unwrap();
        let text = String::from_utf8(conf).unwrap();
        assert!(text.contains("bind = \"127.0.0.1:8080\""), "{text}");
        assert!(text.contains("admin_bind = \"127.0.0.1:8081\""), "{text}");
        assert!(text.contains("[auth.clients]"), "{text}");
        assert!(!text.contains("auth = \"none\""), "{text}");
        assert!(!text.contains("${"), "{text}");
    }

    #[test]
    fn a_missing_env_key_is_named_without_its_value() {
        let dir = temp_dir("env-keys");
        let mut request = tls_request(&dir, &sentinel_env());
        let partial = dir.join("partial.env");
        std::fs::write(
            &partial,
            format!(
                "LLMPROXY_ADMIN_TOKEN={}\nLLMPROXY_CLIENT_KEY_ID=workstation\n",
                abuse::SENTINEL
            ),
        )
        .unwrap();
        request.env_file = Some(partial);
        let remote = FixtureRemote::new("root@host");
        let err = plan_deploy(&request, &remote).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("LLMPROXY_CLIENT_KEY_SECRET"), "{text}");
        assert!(!text.contains(abuse::SENTINEL), "{text}");
    }

    #[test]
    fn blocked_preflight_refuses_execution_typed() {
        let dir = temp_dir("blocked");
        let remote = FixtureRemote::new("root@host");
        remote.set_host(FixtureHost {
            systemd_version: None,
            ..FixtureHost::default()
        });
        let plan = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        assert!(plan.blocked());
        let err = execute_deploy(&plan, &remote).unwrap_err();
        assert!(err.to_string().contains("preflight"), "{err}");
        assert!(
            remote.file(REMOTE_BIN_PATH).is_none(),
            "nothing was written"
        );
    }

    #[test]
    fn upgrade_replaces_the_binary_and_restarts_on_a_managed_host() {
        let dir = temp_dir("upgrade");
        let remote = FixtureRemote::new("root@host");
        let plan = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        execute_deploy(&plan, &remote).unwrap();
        assert!(remote.service_active("llmproxy"));

        let new_binary = dir.join("llmproxy-new");
        std::fs::write(&new_binary, b"musl-bytes-v2").unwrap();
        let upgrade = plan_upgrade(
            &UpgradeRequest {
                host: "root@203.0.113.10".to_owned(),
                binary_path: new_binary,
                policy: Policy::AcceptNew,
            },
            &remote,
        )
        .unwrap();
        assert_eq!(upgrade.steps.len(), 2);
        let journal = execute_upgrade(&upgrade, &remote).unwrap();
        assert!(journal.rolled_back.is_none());
        assert_eq!(
            remote.file(REMOTE_BIN_PATH),
            Some(b"musl-bytes-v2".to_vec())
        );
        assert!(remote.service_active("llmproxy"));
        assert!(
            journal
                .backups
                .iter()
                .any(|record| record.path == REMOTE_BIN_PATH),
            "the old binary is backed up before replace"
        );
    }

    #[test]
    fn upgrade_refuses_unmanaged_hosts_and_rolls_back_a_failed_restart() {
        let dir = temp_dir("upgrade-refuse");
        let remote = FixtureRemote::new("root@host");
        let request = UpgradeRequest {
            host: "root@203.0.113.10".to_owned(),
            binary_path: dir.join("llmproxy"),
            policy: Policy::Strict,
        };
        let err = plan_upgrade(&request, &remote).unwrap_err();
        assert!(err.to_string().contains("manifest"), "{err}");

        std::fs::write(dir.join("llmproxy"), b"v2").unwrap();
        remote.insert_file(
            REMOTE_MANIFEST_PATH,
            b"{\"managed_by\":\"someone-else\",\"paths\":[]}\n",
            0o644,
        );
        let err = plan_upgrade(&request, &remote).unwrap_err();
        assert!(err.to_string().contains("foreign"), "{err}");

        remote.insert_file(
            REMOTE_MANIFEST_PATH,
            b"{\"managed_by\":\"superai\",\"service\":\"llmproxy\",\"address\":\"x\",\
              \"paths\":[\"/usr/local/lib/llmproxy/llmproxy\"]}\n",
            0o644,
        );
        remote.insert_file(REMOTE_BIN_PATH, b"v1", 0o755);
        let upgrade = plan_upgrade(&request, &remote).unwrap();
        remote.fail_next(Fault::Restart);
        let journal = execute_upgrade(&upgrade, &remote).unwrap();
        assert!(journal.rolled_back.is_some());
        assert_eq!(
            remote.file(REMOTE_BIN_PATH),
            Some(b"v1".to_vec()),
            "failed restart restores the previous binary"
        );
    }

    fn wg_request(dir: &Path) -> DeployRequest {
        let binary = dir.join("llmproxy");
        std::fs::write(&binary, b"musl-bytes").unwrap();
        let env_file = dir.join("wg.env");
        std::fs::write(
            &env_file,
            format!("LLMPROXY_ADMIN_TOKEN=fixture-admin\nLLMPROXY_CLIENT_KEY_ID=workstation\nLLMPROXY_CLIENT_KEY_SECRET=client-secret\nLLMPROXY_WG_PRIVATE_KEY={}\n", abuse::SENTINEL),
        )
        .unwrap();
        DeployRequest {
            host: "root@203.0.113.10".to_owned(),
            point_instances: Vec::new(),
            ingress: IngressMode::Tunnel(TunnelKind::Wireguard {
                client_public_key: "clientpubkey=".to_owned(),
            }),
            geo_exit: Some("10.7.0.1:3128".to_owned()),
            ack_geo_exit_proxy: true,
            binary_path: binary,
            providers: vec![ProviderToml {
                name: "openai".to_owned(),
                content: "api_key = \"PLACEHOLDER\"\n".to_owned(),
            }],
            env_file: Some(env_file),
            policy: Policy::Strict,
        }
    }

    #[test]
    fn wireguard_tunnel_deploy_committed_and_rolls_back_secretly() {
        let dir = temp_dir("wg");
        let request = wg_request(&dir);
        let remote = FixtureRemote::new("root@host");
        let plan = plan_deploy(&request, &remote).unwrap();
        abuse::assert_no_sentinel_in_debug(&plan, "tunnel plan");
        let journal = execute_deploy(&plan, &remote).unwrap();
        assert!(journal.rolled_back.is_none());
        abuse::assert_no_sentinel_in_debug(&journal, "tunnel journal");
        let provider = remote.file("/etc/llmproxy/providers/openai.toml").unwrap();
        let text = String::from_utf8(provider).unwrap();
        assert!(text.contains("socks5h://10.7.0.1:3128"), "{text}");
        let wg = remote.file(REMOTE_WG_CONF_PATH).unwrap();
        let wg_text = String::from_utf8(wg).unwrap();
        assert!(
            wg_text.contains(abuse::SENTINEL) && !wg_text.contains("${"),
            "the VPS-side interpolation filled the private key: {wg_text}"
        );
        assert_eq!(remote.file_mode(REMOTE_WG_CONF_PATH), Some(0o600));
    }

    #[test]
    fn rollback_removes_a_unit_the_failed_run_itself_created() {
        let dir = temp_dir("unit-rollback");
        let remote = FixtureRemote::new("root@host");
        let plan = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        let reloads_before = remote.reload_count();
        remote.fail_next(Fault::Enable);
        let journal = execute_deploy(&plan, &remote).unwrap();
        assert!(journal.rolled_back.is_some());
        assert!(
            remote.file(REMOTE_UNIT_PATH).is_none(),
            "a unit this run installed must not survive rollback"
        );
        assert!(
            remote.reload_count() > reloads_before,
            "rollback reloads the init system after removing the unit"
        );
    }

    #[test]
    fn redeploy_diff_of_a_secret_file_never_shows_either_value() {
        let dir = temp_dir("secret-diff");
        let remote = FixtureRemote::new("root@host");
        let first = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        execute_deploy(&first, &remote).unwrap();

        let rotated = dir.join("rotated.env");
        std::fs::write(
            &rotated,
            format!(
                "LLMPROXY_ADMIN_TOKEN={}
LLMPROXY_CLIENT_KEY_ID=workstation
LLMPROXY_CLIENT_KEY_SECRET=client-secret
",
                abuse::SENTINEL
            ),
        )
        .unwrap();
        let mut second_request = tls_request(&dir, &sentinel_env());
        second_request.env_file = Some(rotated);
        let second = plan_deploy(&second_request, &remote).unwrap();
        abuse::assert_no_sentinel_in_debug(&second, "redeploy plan");
        let env_step = second
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
            StepPreview::Replace { diff, .. } => {
                assert!(diff.contains("secret payload"), "{diff}");
                assert!(!diff.contains(abuse::SENTINEL), "{diff}");
            }
            other => panic!("env re-upload must be a replace: {other:?}"),
        }
    }

    #[test]
    fn provider_placeholders_are_interpolated_on_the_host_not_in_plan() {
        let dir = temp_dir("interpolate");
        let mut request = tls_request(&dir, &sentinel_env());
        request.providers = vec![ProviderToml {
            name: "anthropic".to_owned(),
            content: "api_key = \"${ANTHROPIC_API_KEY}\"\n".to_owned(),
        }];
        let env_with_key = dir.join("with-key.env");
        std::fs::write(
            &env_with_key,
            format!(
                "ANTHROPIC_API_KEY={}
LLMPROXY_ADMIN_TOKEN=x
LLMPROXY_CLIENT_KEY_ID=workstation
LLMPROXY_CLIENT_KEY_SECRET=client-secret
",
                abuse::SENTINEL
            ),
        )
        .unwrap();
        request.env_file = Some(env_with_key);

        let remote = FixtureRemote::new("root@host");
        let plan = plan_deploy(&request, &remote).unwrap();
        abuse::assert_no_sentinel_in_debug(&plan, "interpolating plan");
        assert!(plan.steps.iter().any(|step| matches!(
            &step.step,
            DeployStep::InterpolateEnv { target, .. }
                if target == "/etc/llmproxy/providers/anthropic.toml"
        )));
        let journal = execute_deploy(&plan, &remote).unwrap();
        assert!(journal.rolled_back.is_none(), "{journal:?}");
        abuse::assert_no_sentinel_in_debug(&journal, "interpolating journal");
        let deployed = remote
            .file("/etc/llmproxy/providers/anthropic.toml")
            .unwrap();
        let text = String::from_utf8(deployed).unwrap();
        assert!(
            text.contains(abuse::SENTINEL) && !text.contains("${"),
            "the deployed config carries the real key: {text}"
        );

        request.env_file = None;
        request.providers = vec![ProviderToml {
            name: "anthropic".to_owned(),
            content: "api_key = \"${ANTHROPIC_API_KEY}\"\n".to_owned(),
        }];
        let err = plan_deploy(&request, &remote).unwrap_err();
        assert!(
            err.to_string().contains("env file carrying"),
            "the server config secrets refuse without an env file: {err}"
        );
    }

    #[test]
    fn tls_plans_enable_the_site_and_preflight_the_ingress_tools() {
        let dir = temp_dir("tls-tools");
        let remote = FixtureRemote::new("root@host");
        let plan = plan_deploy(&tls_request(&dir, &sentinel_env()), &remote).unwrap();
        for name in ["ingress-nginx", "ingress-certbot", "ingress-openssl"] {
            assert!(
                plan.preflight.checks.iter().any(|check| check.name == name),
                "missing {name} in {:?}",
                plan.preflight.checks
            );
        }
        assert!(!plan.blocked(), "{:?}", plan.preflight.first_failure());
        assert!(plan.steps.iter().any(|step| matches!(
            &step.step,
            DeployStep::Symlink { target, link }
                if link == "/etc/nginx/sites-enabled/llmproxy.conf"
                    && target == "/etc/nginx/sites-available/llmproxy.conf"
        )));
        assert!(plan.steps.iter().any(|step| matches!(
            &step.step,
            DeployStep::Exec { argv } if argv.first().is_some_and(|p| p == "certbot")
        )));
        assert_eq!(
            plan.harness_base, "https://proxy.example.com",
            "{}",
            plan.harness_base
        );

        let bare = FixtureRemote::new("root@host");
        bare.set_host(FixtureHost {
            missing_tools: vec!["nginx".to_owned(), "certbot".to_owned()],
            ..FixtureHost::default()
        });
        let blocked = plan_deploy(&tls_request(&dir, &sentinel_env()), &bare).unwrap();
        assert!(blocked.blocked());
        assert!(
            blocked
                .preflight
                .first_failure()
                .is_some_and(|text| text.contains("ingress-nginx")),
            "{:?}",
            blocked.preflight.checks
        );
    }
}
