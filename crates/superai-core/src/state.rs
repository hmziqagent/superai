//! Lifecycle and ownership states.
//!
//! Explicit enums model every distinct state without collapsing into booleans.
//! No `isolated` or `supported` boolean may stand in for these enums.

use std::fmt;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// InstallPresence
// ---------------------------------------------------------------------------

/// Whether the harness binary is present on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallPresence {
    /// No binary or package found.
    Absent,
    /// Binary found and version is probeable.
    Present,
    /// Binary found but not executable or version probe failed.
    Broken,
    /// Binary found but version could not be determined.
    UnknownVersion,
}

impl fmt::Display for InstallPresence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Absent => "absent",
            Self::Present => "present",
            Self::Broken => "broken",
            Self::UnknownVersion => "unknown_version",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// InstanceOrigin
// ---------------------------------------------------------------------------

/// How the instance record came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceOrigin {
    /// The default install that existed before superai, e.g. `~/.claude`.
    Default,
    /// Created fresh through superai without mirroring another instance.
    Created,
    /// Mirrored from an existing instance, then isolated.
    Mirrored,
    /// Adopted from an existing unmanaged config directory.
    Adopted,
    /// Adopted from a legacy record or directory without stable IDs.
    AdoptedLegacy,
}

impl fmt::Display for InstanceOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Default => "default",
            Self::Created => "created",
            Self::Mirrored => "mirrored",
            Self::Adopted => "adopted",
            Self::AdoptedLegacy => "adopted_legacy",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// Ownership
// ---------------------------------------------------------------------------

/// Who owns the config directory on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ownership {
    /// Created and owned by superai.
    SuperaiCreated,
    /// Explicitly adopted into superai management.
    ExplicitlyAdopted,
    /// Owned and actively managed by another tool.
    ForeignManaged,
    /// Present on disk with no owner.
    Unmanaged,
    /// Previously owned but now detached from management.
    Detached,
}

impl fmt::Display for Ownership {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::SuperaiCreated => "superai_created",
            Self::ExplicitlyAdopted => "explicitly_adopted",
            Self::ForeignManaged => "foreign_managed",
            Self::Unmanaged => "unmanaged",
            Self::Detached => "detached",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Operational lifecycle state of an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    /// Ready to use.
    Ready,
    /// Requires authentication before use.
    NeedsAuth,
    /// Present but degraded (partial config, warnings).
    Degraded,
    /// Conflicting state requires user resolution.
    Conflict,
    /// Config directory or required file missing.
    MissingConfig,
    /// Binary missing or not executable.
    MissingBinary,
}

impl fmt::Display for Lifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Ready => "ready",
            Self::NeedsAuth => "needs_auth",
            Self::Degraded => "degraded",
            Self::Conflict => "conflict",
            Self::MissingConfig => "missing_config",
            Self::MissingBinary => "missing_binary",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// Isolation
// ---------------------------------------------------------------------------

/// How an instance's config is isolated from the default location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// Entire config root relocated via env var (e.g. `CLAUDE_CONFIG_DIR`).
    RelocatedRoot,
    /// Single config file path overridden via flag or env.
    ExplicitConfig,
    /// Scoped to a project directory.
    ProjectScope,
    /// Uses IDE user-data directory isolation.
    IdeUserData,
    /// Isolated only via environment variables.
    EnvOnly,
    /// Served through a daemon or background service.
    DaemonService,
    /// Fixed single path, only one instance possible.
    FixedPathSingle,
    /// Isolation bound to OS facilities (e.g. sandbox, container).
    OsBound,
    /// Isolation not supported for this harness.
    Unsupported,
    /// Isolation kind not yet determined.
    Unknown,
}

impl fmt::Display for Isolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::RelocatedRoot => "relocated_root",
            Self::ExplicitConfig => "explicit_config",
            Self::ProjectScope => "project_scope",
            Self::IdeUserData => "ide_user_data",
            Self::EnvOnly => "env_only",
            Self::DaemonService => "daemon_service",
            Self::FixedPathSingle => "fixed_path_single",
            Self::OsBound => "os_bound",
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// AdapterSupport
// ---------------------------------------------------------------------------

/// What the adapter can do for a harness on this platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterSupport {
    /// Fully supported with multi-instance isolation.
    Full,
    /// Supported with known constraints.
    Constrained,
    /// Only a single instance can exist.
    SingleInstance,
    /// Read-only: can inspect but not mutate.
    ReadOnly,
    /// Only migration from existing installs is supported.
    MigrationOnly,
    /// Blocked pending further research.
    ResearchBlocked,
    /// Not supported.
    Unsupported,
}

impl fmt::Display for AdapterSupport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Full => "full",
            Self::Constrained => "constrained",
            Self::SingleInstance => "single_instance",
            Self::ReadOnly => "read_only",
            Self::MigrationOnly => "migration_only",
            Self::ResearchBlocked => "research_blocked",
            Self::Unsupported => "unsupported",
        };
        f.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helpers to prove exhaustive matching without wildcards.

    fn describe_presence(v: InstallPresence) -> &'static str {
        match v {
            InstallPresence::Absent => "absent",
            InstallPresence::Present => "present",
            InstallPresence::Broken => "broken",
            InstallPresence::UnknownVersion => "unknown_version",
        }
    }

    fn describe_origin(v: InstanceOrigin) -> &'static str {
        match v {
            InstanceOrigin::Default => "default",
            InstanceOrigin::Created => "created",
            InstanceOrigin::Mirrored => "mirrored",
            InstanceOrigin::Adopted => "adopted",
            InstanceOrigin::AdoptedLegacy => "adopted_legacy",
        }
    }

    fn describe_ownership(v: Ownership) -> &'static str {
        match v {
            Ownership::SuperaiCreated => "superai_created",
            Ownership::ExplicitlyAdopted => "explicitly_adopted",
            Ownership::ForeignManaged => "foreign_managed",
            Ownership::Unmanaged => "unmanaged",
            Ownership::Detached => "detached",
        }
    }

    fn describe_lifecycle(v: Lifecycle) -> &'static str {
        match v {
            Lifecycle::Ready => "ready",
            Lifecycle::NeedsAuth => "needs_auth",
            Lifecycle::Degraded => "degraded",
            Lifecycle::Conflict => "conflict",
            Lifecycle::MissingConfig => "missing_config",
            Lifecycle::MissingBinary => "missing_binary",
        }
    }

    fn describe_isolation(v: Isolation) -> &'static str {
        match v {
            Isolation::RelocatedRoot => "relocated_root",
            Isolation::ExplicitConfig => "explicit_config",
            Isolation::ProjectScope => "project_scope",
            Isolation::IdeUserData => "ide_user_data",
            Isolation::EnvOnly => "env_only",
            Isolation::DaemonService => "daemon_service",
            Isolation::FixedPathSingle => "fixed_path_single",
            Isolation::OsBound => "os_bound",
            Isolation::Unsupported => "unsupported",
            Isolation::Unknown => "unknown",
        }
    }

    fn describe_adapter(v: AdapterSupport) -> &'static str {
        match v {
            AdapterSupport::Full => "full",
            AdapterSupport::Constrained => "constrained",
            AdapterSupport::SingleInstance => "single_instance",
            AdapterSupport::ReadOnly => "read_only",
            AdapterSupport::MigrationOnly => "migration_only",
            AdapterSupport::ResearchBlocked => "research_blocked",
            AdapterSupport::Unsupported => "unsupported",
        }
    }

    #[test]
    fn install_presence_exhaustive_and_display() {
        let cases = [
            (InstallPresence::Absent, "absent"),
            (InstallPresence::Present, "present"),
            (InstallPresence::Broken, "broken"),
            (InstallPresence::UnknownVersion, "unknown_version"),
        ];
        for (variant, expected) in cases {
            assert_eq!(describe_presence(variant), expected);
            assert_eq!(variant.to_string(), expected);
        }
    }

    #[test]
    fn instance_origin_exhaustive_and_display() {
        let cases = [
            (InstanceOrigin::Default, "default"),
            (InstanceOrigin::Created, "created"),
            (InstanceOrigin::Mirrored, "mirrored"),
            (InstanceOrigin::Adopted, "adopted"),
            (InstanceOrigin::AdoptedLegacy, "adopted_legacy"),
        ];
        for (variant, expected) in cases {
            assert_eq!(describe_origin(variant), expected);
            assert_eq!(variant.to_string(), expected);
        }
    }

    #[test]
    fn ownership_exhaustive_and_display() {
        let cases = [
            (Ownership::SuperaiCreated, "superai_created"),
            (Ownership::ExplicitlyAdopted, "explicitly_adopted"),
            (Ownership::ForeignManaged, "foreign_managed"),
            (Ownership::Unmanaged, "unmanaged"),
            (Ownership::Detached, "detached"),
        ];
        for (variant, expected) in cases {
            assert_eq!(describe_ownership(variant), expected);
            assert_eq!(variant.to_string(), expected);
        }
    }

    #[test]
    fn lifecycle_exhaustive_and_display() {
        let cases = [
            (Lifecycle::Ready, "ready"),
            (Lifecycle::NeedsAuth, "needs_auth"),
            (Lifecycle::Degraded, "degraded"),
            (Lifecycle::Conflict, "conflict"),
            (Lifecycle::MissingConfig, "missing_config"),
            (Lifecycle::MissingBinary, "missing_binary"),
        ];
        for (variant, expected) in cases {
            assert_eq!(describe_lifecycle(variant), expected);
            assert_eq!(variant.to_string(), expected);
        }
    }

    #[test]
    fn isolation_exhaustive_and_display() {
        let cases = [
            (Isolation::RelocatedRoot, "relocated_root"),
            (Isolation::ExplicitConfig, "explicit_config"),
            (Isolation::ProjectScope, "project_scope"),
            (Isolation::IdeUserData, "ide_user_data"),
            (Isolation::EnvOnly, "env_only"),
            (Isolation::DaemonService, "daemon_service"),
            (Isolation::FixedPathSingle, "fixed_path_single"),
            (Isolation::OsBound, "os_bound"),
            (Isolation::Unsupported, "unsupported"),
            (Isolation::Unknown, "unknown"),
        ];
        for (variant, expected) in cases {
            assert_eq!(describe_isolation(variant), expected);
            assert_eq!(variant.to_string(), expected);
        }
    }

    #[test]
    fn adapter_support_exhaustive_and_display() {
        let cases = [
            (AdapterSupport::Full, "full"),
            (AdapterSupport::Constrained, "constrained"),
            (AdapterSupport::SingleInstance, "single_instance"),
            (AdapterSupport::ReadOnly, "read_only"),
            (AdapterSupport::MigrationOnly, "migration_only"),
            (AdapterSupport::ResearchBlocked, "research_blocked"),
            (AdapterSupport::Unsupported, "unsupported"),
        ];
        for (variant, expected) in cases {
            assert_eq!(describe_adapter(variant), expected);
            assert_eq!(variant.to_string(), expected);
        }
    }

    #[test]
    fn serialization_round_trip_install_presence() {
        let variants = [
            InstallPresence::Absent,
            InstallPresence::Present,
            InstallPresence::Broken,
            InstallPresence::UnknownVersion,
        ];
        for v in variants {
            let json = serde_json::to_string(&v).unwrap();
            let back: InstallPresence = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back);
            assert_eq!(json, format!("\"{v}\""));
        }
        // Unknown value fails.
        let err: Result<InstallPresence, _> = serde_json::from_str("\"unknown\"");
        err.unwrap_err();
    }

    #[test]
    fn serialization_round_trip_instance_origin() {
        let variants = [
            InstanceOrigin::Default,
            InstanceOrigin::Created,
            InstanceOrigin::Mirrored,
            InstanceOrigin::Adopted,
            InstanceOrigin::AdoptedLegacy,
        ];
        for v in variants {
            let json = serde_json::to_string(&v).unwrap();
            let back: InstanceOrigin = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back);
            assert_eq!(json, format!("\"{v}\""));
        }
        let err: Result<InstanceOrigin, _> = serde_json::from_str("\"not_a_variant\"");
        err.unwrap_err();
    }

    #[test]
    fn serialization_round_trip_ownership() {
        let variants = [
            Ownership::SuperaiCreated,
            Ownership::ExplicitlyAdopted,
            Ownership::ForeignManaged,
            Ownership::Unmanaged,
            Ownership::Detached,
        ];
        for v in variants {
            let json = serde_json::to_string(&v).unwrap();
            let back: Ownership = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back);
            assert_eq!(json, format!("\"{v}\""));
        }
        let err: Result<Ownership, _> = serde_json::from_str("\"garbage\"");
        err.unwrap_err();
    }

    #[test]
    fn serialization_round_trip_lifecycle() {
        let variants = [
            Lifecycle::Ready,
            Lifecycle::NeedsAuth,
            Lifecycle::Degraded,
            Lifecycle::Conflict,
            Lifecycle::MissingConfig,
            Lifecycle::MissingBinary,
        ];
        for v in variants {
            let json = serde_json::to_string(&v).unwrap();
            let back: Lifecycle = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back);
            assert_eq!(json, format!("\"{v}\""));
        }
        let err: Result<Lifecycle, _> = serde_json::from_str("\"nope\"");
        err.unwrap_err();
    }

    #[test]
    fn serialization_round_trip_isolation() {
        let variants = [
            Isolation::RelocatedRoot,
            Isolation::ExplicitConfig,
            Isolation::ProjectScope,
            Isolation::IdeUserData,
            Isolation::EnvOnly,
            Isolation::DaemonService,
            Isolation::FixedPathSingle,
            Isolation::OsBound,
            Isolation::Unsupported,
            Isolation::Unknown,
        ];
        for v in variants {
            let json = serde_json::to_string(&v).unwrap();
            let back: Isolation = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back);
            assert_eq!(json, format!("\"{v}\""));
        }
        let err: Result<Isolation, _> = serde_json::from_str("\"bogus\"");
        err.unwrap_err();
    }

    #[test]
    fn serialization_round_trip_adapter_support() {
        let variants = [
            AdapterSupport::Full,
            AdapterSupport::Constrained,
            AdapterSupport::SingleInstance,
            AdapterSupport::ReadOnly,
            AdapterSupport::MigrationOnly,
            AdapterSupport::ResearchBlocked,
            AdapterSupport::Unsupported,
        ];
        for v in variants {
            let json = serde_json::to_string(&v).unwrap();
            let back: AdapterSupport = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back);
            assert_eq!(json, format!("\"{v}\""));
        }
        let err: Result<AdapterSupport, _> = serde_json::from_str("\"invalid\"");
        err.unwrap_err();
    }

    #[test]
    fn enums_are_copy_and_clone() {
        let presence = InstallPresence::Present;
        let presence_copy = presence;
        assert_eq!(presence, presence_copy);
        let presence_again = presence;
        assert_eq!(presence_again, InstallPresence::Present);

        let ownership = Ownership::SuperaiCreated;
        let ownership_copy = ownership;
        assert_eq!(ownership, ownership_copy);

        let isolation = Isolation::RelocatedRoot;
        let isolation_copy = isolation;
        assert_eq!(isolation, isolation_copy);
    }

    #[test]
    fn debug_does_not_leak_secrets() {
        // No variant contains secret material; ensure Debug is the variant name.
        assert_eq!(format!("{:?}", InstallPresence::Present), "Present");
        assert_eq!(format!("{:?}", Ownership::ForeignManaged), "ForeignManaged");
        assert_eq!(format!("{:?}", Lifecycle::NeedsAuth), "NeedsAuth");
        assert!(format!("{:?}", Isolation::Unknown).contains("Unknown"));
        assert!(format!("{:?}", AdapterSupport::ResearchBlocked).contains("ResearchBlocked"));
    }

    #[test]
    fn struct_containing_all_states_round_trips() {
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
        struct Snapshot {
            presence: InstallPresence,
            origin: InstanceOrigin,
            ownership: Ownership,
            lifecycle: Lifecycle,
            isolation: Isolation,
            support: AdapterSupport,
        }

        let original = Snapshot {
            presence: InstallPresence::UnknownVersion,
            origin: InstanceOrigin::AdoptedLegacy,
            ownership: Ownership::ExplicitlyAdopted,
            lifecycle: Lifecycle::Degraded,
            isolation: Isolation::EnvOnly,
            support: AdapterSupport::Constrained,
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);

        // Verify snake_case wire format, no PascalCase leakage.
        assert!(json.contains("\"unknown_version\""));
        assert!(json.contains("\"adopted_legacy\""));
        assert!(json.contains("\"explicitly_adopted\""));
        assert!(json.contains("\"degraded\""));
        assert!(json.contains("\"env_only\""));
        assert!(json.contains("\"constrained\""));
    }

    #[test]
    fn wire_format_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&InstallPresence::UnknownVersion).unwrap(),
            "\"unknown_version\""
        );
        assert_eq!(
            serde_json::to_string(&InstanceOrigin::AdoptedLegacy).unwrap(),
            "\"adopted_legacy\""
        );
        assert_eq!(
            serde_json::to_string(&Ownership::SuperaiCreated).unwrap(),
            "\"superai_created\""
        );
        assert_eq!(
            serde_json::to_string(&Lifecycle::NeedsAuth).unwrap(),
            "\"needs_auth\""
        );
        assert_eq!(
            serde_json::to_string(&Isolation::FixedPathSingle).unwrap(),
            "\"fixed_path_single\""
        );
        assert_eq!(
            serde_json::to_string(&AdapterSupport::SingleInstance).unwrap(),
            "\"single_instance\""
        );
        assert_eq!(
            serde_json::to_string(&AdapterSupport::ResearchBlocked).unwrap(),
            "\"research_blocked\""
        );
        assert_eq!(
            serde_json::to_string(&Isolation::IdeUserData).unwrap(),
            "\"ide_user_data\""
        );
    }
}
