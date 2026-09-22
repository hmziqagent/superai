use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{HarnessId, InstanceId, InstanceName, TemplateId, TemplateVersion};
use crate::paths::{AbsolutePath, ExecutableRef, WrapperPath};
use crate::state::{InstanceOrigin, Isolation, Ownership};

/// The template an instance came from, and the version it was built at;
/// version is tracked separately from the harness's own version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateRef {
    /// Template identifier, e.g. `claude-code-glm`.
    pub name: TemplateId,
    /// Template version the instance was built from.
    pub version: TemplateVersion,
}

impl fmt::Display for TemplateRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

/// Reference to the generated wrapper for an instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrapperRef {
    /// Absolute path to the wrapper executable on disk.
    pub path: WrapperPath,
    /// Command name that invokes the wrapper, e.g. `work` or `claude-glm`.
    pub command_name: InstanceName,
    /// Version of the generator that created the wrapper.
    pub generator_version: String,
    /// Hex digest of the wrapper content for drift detection.
    pub content_digest: String,
}

/// A named, isolated setup of a harness: its own config dir, wrapper, provenance.
/// Superai's own record; harness-owned values (model, base URL, key) are never mirrored here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    /// Immutable generated identity for the instance; rename does not change it.
    pub id: InstanceId,
    /// Mutable user-chosen label and default wrapper command.
    pub name: InstanceName,
    /// Which harness this instance is an install of, e.g. `claude-code`.
    pub harness: HarnessId,
    /// Config directory the wrapper points the harness at (normalized absolute).
    pub config_root: AbsolutePath,
    /// Binary the wrapper execs, when it is not the one on `PATH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<ExecutableRef>,
    /// Generated wrapper that isolates the instance, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapper: Option<WrapperRef>,
    /// How the config is isolated from the default location.
    pub isolation: Isolation,
    /// How the instance record came to be.
    pub origin: InstanceOrigin,
    /// Who owns the config directory on disk.
    pub ownership: Ownership,
    /// Template this instance was built from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<TemplateRef>,
    /// When the record was created (ISO8601 UTC, e.g. `2026-08-26T12:00:00Z`).
    pub created_at: String,
    /// Version of the adapter/revision that last wrote the record.
    pub adapter_revision: String,
}

impl Instance {
    /// Validate that required string fields are non-empty and well-formed.
    /// Covers the free-form strings with no newtype: `created_at` and friends.
    pub fn validate(&self) -> Result<(), crate::error::CoreError> {
        if self.created_at.is_empty() {
            return Err(crate::error::CoreError::Validation {
                field: "created_at".to_owned(),
                reason: "must not be empty".to_owned(),
            });
        }
        if self.adapter_revision.is_empty() {
            return Err(crate::error::CoreError::Validation {
                field: "adapter_revision".to_owned(),
                reason: "must not be empty".to_owned(),
            });
        }
        if let Some(wrapper) = &self.wrapper {
            if wrapper.generator_version.is_empty() {
                return Err(crate::error::CoreError::Validation {
                    field: "wrapper.generator_version".to_owned(),
                    reason: "must not be empty".to_owned(),
                });
            }
            if wrapper.content_digest.is_empty() {
                return Err(crate::error::CoreError::Validation {
                    field: "wrapper.content_digest".to_owned(),
                    reason: "must not be empty".to_owned(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::ids::{HarnessId, InstanceId, InstanceName, TemplateId, TemplateVersion};
    use crate::instance::{Instance, TemplateRef, WrapperRef};
    use crate::paths::{AbsolutePath, ExecutableRef, WrapperPath};
    use crate::state::{InstanceOrigin, Isolation, Ownership};

    fn sample_instance() -> Instance {
        Instance {
            id: InstanceId::new("test-id-1").unwrap(),
            name: InstanceName::new("work").unwrap(),
            harness: HarnessId::new("claude-code").unwrap(),
            config_root: AbsolutePath::new(&crate::test_util::tmp_abs_str("user/.claude-work"))
                .unwrap(),
            binary: Some(ExecutableRef::new("claude").unwrap()),
            wrapper: Some(WrapperRef {
                path: WrapperPath::new(&crate::test_util::tmp_abs_str("user/.local/bin/work"))
                    .unwrap(),
                command_name: InstanceName::new("work").unwrap(),
                generator_version: "1.0.0".to_owned(),
                content_digest: "abc123".to_owned(),
            }),
            isolation: Isolation::RelocatedRoot,
            origin: InstanceOrigin::Created,
            ownership: Ownership::SuperaiCreated,
            template: Some(TemplateRef {
                name: TemplateId::new("claude-glm").unwrap(),
                version: TemplateVersion::new("1.2.0").unwrap(),
            }),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            adapter_revision: "0.1.0".to_owned(),
        }
    }

    #[test]
    fn round_trip() {
        let inst = sample_instance();
        let json = serde_json::to_string(&inst).unwrap();
        let back: Instance = serde_json::from_str(&json).unwrap();
        assert_eq!(inst, back);
    }

    #[test]
    fn forbidden_fields_are_never_emitted() {
        let inst = sample_instance();
        let json = serde_json::to_string(&inst).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let forbidden = [
            "model",
            "endpoint",
            "api_key",
            "apiKey",
            "key",
            "skill",
            "skills",
            "plugin",
            "plugins",
            "mcp",
            "capability",
            "baseUrl",
            "base_url",
        ];
        let text = json.to_lowercase();
        for field in forbidden {
            if let serde_json::Value::Object(map) = &v {
                assert!(
                    !map.contains_key(field),
                    "forbidden field `{field}` must not be emitted, json: {json}"
                );
            }
            assert!(
                !text.contains(&format!("\"{field}\"")),
                "forbidden field `{field}` appears in serialized json: {json}"
            );
        }
    }

    #[test]
    fn template_ref_round_trip() {
        let t = TemplateRef {
            name: TemplateId::new("my-template").unwrap(),
            version: TemplateVersion::new("2.0.0").unwrap(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: TemplateRef = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn wrapper_ref_round_trip() {
        let w = WrapperRef {
            path: WrapperPath::new(&crate::test_util::tmp_abs_str("wrapper")).unwrap(),
            command_name: InstanceName::new("work").unwrap(),
            generator_version: "0.1.0".to_owned(),
            content_digest: "deadbeef".to_owned(),
        };
        let json = serde_json::to_string(&w).unwrap();
        let back: WrapperRef = serde_json::from_str(&json).unwrap();
        assert_eq!(w, back);
    }
}
