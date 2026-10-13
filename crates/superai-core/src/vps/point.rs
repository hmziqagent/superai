//! Pointing local harness configs at a deployed proxy (VPS-02 item 4):
//! the subplan's "harness configs then point at the tunnel address", done
//! through the provider-render commit path so configs are read fresh,
//! backed up, and unmodelled keys survive.

use std::path::{Path, PathBuf};

use crate::adapter::Adapter;
use crate::instance::Instance;
use crate::provider::{ProviderDefinition, load_bundled_providers};
use crate::provider_render::{
    ProviderChange, ProviderChangeOptions, ProviderChangeOutcome, commit_provider_change,
    preview_provider_change,
};
use crate::registry::Registry;
use crate::vps::VpsError;

/// One pointing commit: what changed and where (redacted descriptions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointOutcome {
    /// Instance whose config was pointed.
    pub instance: String,
    /// Provider whose base URL now routes through the deployment.
    pub provider: String,
    /// Harness config file that changed.
    pub path: PathBuf,
    /// Redacted edit descriptions from the commit; never secrets.
    pub applied: Vec<String>,
}

/// Point one recorded instance's provider entry at `base_url` by committing
/// an [`ProviderChange::AddOrUpdate`] whose provider definition carries the deployment address.
pub fn point_instance_at(
    instance: &Instance,
    adapter: &dyn Adapter,
    provider_id: &str,
    base_url: &str,
) -> Result<PointOutcome, VpsError> {
    let mut provider = bundled_provider(provider_id)?;
    provider.base_url = String::from(base_url);
    let change = ProviderChange::AddOrUpdate {
        provider: &provider,
    };
    let preview = preview_provider_change(instance, adapter, &change);
    if !preview.supported {
        return Err(VpsError::RemoteState {
            path: preview.path.display().to_string(),
            reason: preview
                .unsupported_reason
                .unwrap_or_else(|| "harness cannot express the change".to_owned()),
        });
    }
    let outcome: ProviderChangeOutcome = commit_provider_change(
        instance,
        adapter,
        &change,
        &ProviderChangeOptions::default(),
    )?;
    Ok(PointOutcome {
        instance: instance.name.as_str().to_owned(),
        provider: provider_id.to_owned(),
        path: outcome.path,
        applied: outcome.applied,
    })
}

/// Point a recorded instance by name, resolving its adapter from the
/// registry record; the registry is read fresh from `registry_path`.
pub fn point_recorded_instance(
    registry_path: &Path,
    instance_name: &str,
    provider_ids: &[&str],
    base_url: &str,
) -> Result<Vec<PointOutcome>, VpsError> {
    let registry = Registry::load(registry_path)?;
    let instance = registry.get(instance_name).ok_or_else(|| {
        crate::vps::validation(
            "point",
            format!("no recorded instance named `{instance_name}`"),
        )
    })?;
    let adapter = crate::harness_catalog::adapter_for_id_fold(instance.harness.as_str())
        .ok_or_else(|| {
            crate::vps::validation(
                "point",
                format!("no adapter for harness `{}`", instance.harness),
            )
        })?;
    provider_ids
        .iter()
        .map(|provider_id| point_instance_at(instance, adapter.as_ref(), provider_id, base_url))
        .collect()
}

fn bundled_provider(provider_id: &str) -> Result<ProviderDefinition, VpsError> {
    load_bundled_providers()?
        .into_iter()
        .find(|provider| provider.id.as_str() == provider_id)
        .ok_or_else(|| {
            crate::vps::validation(
                "point",
                format!("no bundled provider named `{provider_id}`"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{HarnessId, InstanceId, InstanceName};
    use crate::paths::AbsolutePath;
    use crate::state::{InstanceOrigin, Isolation, Ownership};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("superai-vps-point-{tag}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn claude_instance(root: &Path) -> Instance {
        Instance {
            id: InstanceId::new("point-test").unwrap(),
            name: InstanceName::new("pointed").unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::from_path(root).unwrap(),
            binary: None,
            wrapper: None,
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Default,
            ownership: Ownership::ExplicitlyAdopted,
            template: None,
            created_at: "2026-10-13T00:00:00Z".to_owned(),
            adapter_revision: String::new(),
        }
    }

    #[test]
    fn pointing_writes_the_base_url_through_the_render_commit() {
        let root = temp_dir("write");
        let instance = claude_instance(&root);
        let adapter = crate::harness_catalog::adapter_for_id_fold("claude-code").unwrap();

        let outcome = point_instance_at(
            &instance,
            adapter.as_ref(),
            "anthropic",
            "https://proxy.example.com",
        )
        .unwrap_or_else(|e| panic!("point: {e}"));

        assert_eq!(outcome.instance, "pointed");
        assert!(!outcome.applied.is_empty(), "{outcome:?}");
        let written = std::fs::read_to_string(&outcome.path).unwrap_or_default();
        assert!(written.contains("proxy.example.com"), "{written}");

        let again = point_instance_at(
            &instance,
            adapter.as_ref(),
            "anthropic",
            "https://proxy.example.com",
        )
        .unwrap_or_else(|e| panic!("re-point: {e}"));
        assert_eq!(again.path, outcome.path, "re-pointing stays on one file");
        drop(std::fs::remove_dir_all(&root));
    }

    #[test]
    fn unknown_providers_and_instances_refuse_typed() {
        let root = temp_dir("refuse");
        let instance = claude_instance(&root);
        let adapter = crate::harness_catalog::adapter_for_id_fold("claude-code").unwrap();
        let err = point_instance_at(&instance, adapter.as_ref(), "no-such-provider", "https://x")
            .unwrap_err();
        assert!(err.to_string().contains("no bundled provider"), "{err}");

        let err = point_recorded_instance(
            &root.join("instances.json"),
            "ghost",
            &["anthropic"],
            "https://x",
        )
        .unwrap_err();
        assert!(err.to_string().contains("no recorded instance"), "{err}");
        drop(std::fs::remove_dir_all(&root));
    }
}
