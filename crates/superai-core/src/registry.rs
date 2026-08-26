use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};
use crate::instance::Instance;

const INSTANCES_KEY: &str = "instances";

/// The set of instances superai knows about, stored in its own records file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    instances: Vec<Instance>,
}

impl Registry {
    /// Default records path: `$HOME/.superai/instances.json`.
    pub fn default_path() -> Result<PathBuf> {
        let home = std::env::home_dir().ok_or(CoreError::NoHomeDir)?;
        Ok(home.join(".superai").join("instances.json"))
    }

    /// Read the records file fresh. A missing file is an empty registry.
    pub fn load(path: &Path) -> Result<Self> {
        let map = superai_config::json::load(path)?;
        let Some(raw) = map.get(INSTANCES_KEY) else {
            return Ok(Self::default());
        };
        let instances = serde_json::from_value(raw.clone())?;
        Ok(Self { instances })
    }

    /// Back up and write the records file, leaving any other key in it untouched.
    pub fn store(&self, path: &Path) -> Result<()> {
        let instances = serde_json::to_value(&self.instances)?;
        superai_config::json::edit(path, |map| {
            map.insert(INSTANCES_KEY.to_owned(), instances);
        })?;
        Ok(())
    }

    /// Every known instance.
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// Look an instance up by name.
    pub fn get(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.name == name)
    }

    /// Add an instance, or fail if the name is taken.
    pub fn insert(&mut self, instance: Instance) -> Result<()> {
        if self.get(&instance.name).is_some() {
            return Err(CoreError::DuplicateInstance {
                name: instance.name,
            });
        }
        self.instances.push(instance);
        Ok(())
    }

    /// Remove an instance by name, returning it. This touches no files on disk.
    pub fn remove(&mut self, name: &str) -> Option<Instance> {
        let idx = self.instances.iter().position(|i| i.name == name)?;
        Some(self.instances.remove(idx))
    }
}

/// Config dirs on disk that no record and no wrapper accounts for.
///
/// Adoption or removal is the user's call — superai only reports what it found.
pub fn unmanaged_dirs(registry: &Registry, candidates: &[PathBuf]) -> Vec<PathBuf> {
    candidates
        .iter()
        .filter(|dir| !registry.instances.iter().any(|i| &&i.config_dir == dir))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::TemplateRef;

    fn instance(name: &str) -> Instance {
        Instance {
            name: name.to_owned(),
            harness: "claude-code".to_owned(),
            config_dir: PathBuf::from(format!("/home/u/.claude-{name}")),
            binary_path: None,
            template: Some(TemplateRef {
                name: "glm".to_owned(),
                version: "1.2.0".to_owned(),
            }),
        }
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let mut r = Registry::default();
        r.insert(instance("work")).unwrap();
        assert!(r.insert(instance("work")).is_err());
        assert_eq!(r.instances().len(), 1);
    }

    #[test]
    fn round_trips_through_disk_keeping_foreign_keys() {
        let path = std::env::temp_dir().join("superai-core-tests/instances.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"schema":7}"#).unwrap();

        let mut r = Registry::default();
        r.insert(instance("work")).unwrap();
        r.store(&path).unwrap();

        assert_eq!(Registry::load(&path).unwrap(), r);
        let raw = superai_config::json::load(&path).unwrap();
        assert_eq!(raw["schema"], serde_json::json!(7));
    }

    #[test]
    fn unmanaged_dirs_excludes_recorded_ones() {
        let mut r = Registry::default();
        r.insert(instance("work")).unwrap();

        let found = unmanaged_dirs(
            &r,
            &[
                PathBuf::from("/home/u/.claude-work"),
                PathBuf::from("/home/u/.claude-aaa"),
            ],
        );
        assert_eq!(found, vec![PathBuf::from("/home/u/.claude-aaa")]);
    }
}
