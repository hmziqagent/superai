//! `superai vps` integration tests: the real binary against temp HOMEs.
//! The production backend refuses typed until toride 0.2 publishes, so these
//! prove dispatch, validation, and the honesty of that refusal; the
//! fixture-proven deploy flow lives in superai-core's tests.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::process::{Command, Output, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    // unused_crate_dependencies demands an ack per unreferenced extern.
    use jiff as _;
    use rusqlite as _;
    use serde as _;
    use serde_json as _;
    use superai_core as _;
    use superai_usage as _;
    use thiserror as _;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_home(tag: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "superai-cli-vps-{tag}-{}-{n}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("temp home {}: {e}", dir.display()));
        dir
    }

    fn superai(home: &PathBuf, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_superai"))
            .args(args)
            .env("HOME", home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap_or_else(|e| panic!("spawn superai: {e}"))
    }

    fn stderr_text(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    fn stdout_text(output: &Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[test]
    fn unknown_top_level_commands_now_list_vps() {
        let home = temp_home("unknown");
        let out = superai(&home, &["frobnicate"]);
        assert_eq!(out.status.code(), Some(1));
        let text = stderr_text(&out);
        assert!(text.contains("unknown command `frobnicate`"), "{text}");
        assert!(text.contains("usage, vps"), "{text}");
    }

    #[test]
    fn vps_without_a_verb_names_the_verbs() {
        let home = temp_home("noverb");
        let out = superai(&home, &["vps"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("deploy, status, or upgrade"),
            "{}",
            stderr_text(&out)
        );
    }

    #[test]
    fn vps_rejects_unknown_verbs_and_services_typed() {
        let home = temp_home("badverb");
        let out = superai(&home, &["vps", "frobnicate"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("unknown vps verb"),
            "{}",
            stderr_text(&out)
        );

        let out = superai(&home, &["vps", "deploy"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("needs a service"),
            "{}",
            stderr_text(&out)
        );

        let out = superai(&home, &["vps", "deploy", "caddy"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("unknown vps service `caddy`"),
            "{}",
            stderr_text(&out)
        );
    }

    #[test]
    fn vps_deploy_validates_flags_before_any_remote_work() {
        let home = temp_home("deploy-args");
        let out = superai(&home, &["vps", "deploy", "llmproxy"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("needs --host"),
            "{}",
            stderr_text(&out)
        );

        let out = superai(&home, &["vps", "deploy", "llmproxy", "--host", "root@h"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("needs --binary"),
            "{}",
            stderr_text(&out)
        );

        let out = superai(
            &home,
            &[
                "vps",
                "deploy",
                "llmproxy",
                "--host",
                "root@h",
                "--binary",
                "/tmp/none",
                "--domain",
                "only.example.com",
            ],
        );
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("go together"),
            "{}",
            stderr_text(&out)
        );

        let out = superai(
            &home,
            &[
                "vps",
                "deploy",
                "llmproxy",
                "--host",
                "root@h;rm",
                "--binary",
                "/tmp/none",
                "--tunnel",
                "--wg-client-key",
                "k=",
            ],
        );
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("must not contain shell metacharacters"),
            "{}",
            stderr_text(&out)
        );

        let out = superai(
            &home,
            &[
                "vps",
                "deploy",
                "llmproxy",
                "--host",
                "root@h",
                "--binary",
                "/tmp/none",
                "--tunnel",
                "--wg-client-key",
                "k=",
                "--host-keys",
                "off",
            ],
        );
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("never accepted"),
            "{}",
            stderr_text(&out)
        );
    }

    #[test]
    fn vps_deploy_and_status_refuse_honestly_until_toride_02() {
        let home = temp_home("pending");
        let binary = home.join("llmproxy-musl");
        fs::write(&binary, b"musl-bytes").unwrap();
        let provider = home.join("anthropic.toml");
        fs::write(&provider, "api_key = \"PLACEHOLDER\"\n").unwrap();
        let env_file = home.join("secrets.env");
        fs::write(&env_file, "LLMPROXY_ADMIN_TOKEN=integration-secret\nLLMPROXY_CLIENT_KEY_ID=workstation\nLLMPROXY_CLIENT_KEY_SECRET=client-secret\n").unwrap();

        let out = superai(
            &home,
            &[
                "vps",
                "deploy",
                "llmproxy",
                "--host",
                "root@203.0.113.10",
                "--binary",
                binary.to_str().unwrap_or_default(),
                "--provider",
                provider.to_str().unwrap_or_default(),
                "--env-file",
                env_file.to_str().unwrap_or_default(),
                "--domain",
                "proxy.example.com",
                "--email",
                "ops@example.com",
            ],
        );
        assert_eq!(out.status.code(), Some(1));
        let text = stderr_text(&out);
        assert!(
            text.contains("pending toride-ssh 0.2 / toride-service 0.2"),
            "the pending gate must be the failure: {text}"
        );
        assert!(
            !stdout_text(&out).contains("musl-bytes"),
            "no partial success output"
        );
        assert!(
            !text.contains("integration-secret"),
            "the env file content never reaches errors"
        );

        let out = superai(
            &home,
            &["vps", "status", "llmproxy", "--host", "root@203.0.113.10"],
        );
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("pending toride-ssh 0.2"),
            "{}",
            stderr_text(&out)
        );

        let out = superai(
            &home,
            &[
                "vps",
                "upgrade",
                "llmproxy",
                "--host",
                "root@203.0.113.10",
                "--binary",
                binary.to_str().unwrap_or_default(),
            ],
        );
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("pending toride-ssh 0.2"),
            "{}",
            stderr_text(&out)
        );
    }

    #[test]
    fn vps_deploy_tunnel_shape_validates_the_env_file_presence() {
        let home = temp_home("tunnel-shape");
        let binary = home.join("llmproxy-musl");
        fs::write(&binary, b"musl-bytes").unwrap();
        let env_file = home.join("wg.env");
        fs::write(&env_file, "LLMPROXY_ADMIN_TOKEN=x\nLLMPROXY_CLIENT_KEY_ID=workstation\nLLMPROXY_CLIENT_KEY_SECRET=client-secret\n").unwrap();
        let out = superai(
            &home,
            &[
                "vps",
                "deploy",
                "llmproxy",
                "--host",
                "root@203.0.113.10",
                "--binary",
                binary.to_str().unwrap_or_default(),
                "--tunnel",
                "--wg-client-key",
                "clientpubkey=",
                "--env-file",
                env_file.to_str().unwrap_or_default(),
            ],
        );
        assert_eq!(out.status.code(), Some(1));
        let text = stderr_text(&out);
        assert!(
            text.contains("LLMPROXY_WG_PRIVATE_KEY"),
            "wireguard mode names the missing key: {text}"
        );
    }

    #[test]
    fn vps_status_without_configured_ledger_is_not_a_pending_claim() {
        let home = temp_home("status-shape");
        let out = superai(&home, &["vps", "status", "llmproxy"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("needs --host"),
            "{}",
            stderr_text(&out)
        );
        let out = superai(
            &home,
            &["vps", "status", "llmproxy", "--host", "no-at-sign"],
        );
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_text(&out).contains("must look like user@host"),
            "{}",
            stderr_text(&out)
        );
    }
}
