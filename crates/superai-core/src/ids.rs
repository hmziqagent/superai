//! Validated identifiers and names.
//!
//! Each identifier is a newtype around `String` with strict validation.
//! Validation rejects empty, `"."` / `".."`, path separators, NUL/control,
//! Windows reserved device names, and trailing dots/spaces. Comparison
//! collisions should use case folding ([`ToString::to_lowercase`]) while
//! preserving the original display form.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use crate::error::CoreError;

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Validate an identifier string for the given `kind`.
///
/// Rejects empty, `"."` / `".."`, separators (`/`, `\`, `:`), NUL/control
/// characters, reserved Windows device names, and trailing dots/spaces.
fn validate(kind: &str, value: &str) -> Result<(), CoreError> {
    if value.is_empty() {
        return Err(CoreError::InvalidIdentifier {
            kind: kind.to_owned(),
            value: value.to_owned(),
            reason: "must not be empty".to_owned(),
        });
    }
    if value == "." || value == ".." {
        return Err(CoreError::InvalidIdentifier {
            kind: kind.to_owned(),
            value: value.to_owned(),
            reason: "must not be '.' or '..'".to_owned(),
        });
    }
    if value.contains('/') || value.contains('\\') || value.contains(':') {
        return Err(CoreError::InvalidIdentifier {
            kind: kind.to_owned(),
            value: value.to_owned(),
            reason: "must not contain '/', '\\', or ':'".to_owned(),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(CoreError::InvalidIdentifier {
            kind: kind.to_owned(),
            value: value.to_owned(),
            reason: "must not contain NUL or control characters".to_owned(),
        });
    }
    if value.ends_with('.') || value.ends_with(' ') {
        return Err(CoreError::InvalidIdentifier {
            kind: kind.to_owned(),
            value: value.to_owned(),
            reason: "must not end with '.' or ' '".to_owned(),
        });
    }
    let stem = value.split('.').next().unwrap_or_default();
    let stem_lower = stem.to_lowercase();
    for reserved in RESERVED {
        if &stem_lower == reserved {
            return Err(CoreError::InvalidIdentifier {
                kind: kind.to_owned(),
                value: value.to_owned(),
                reason: "reserved Windows device name".to_owned(),
            });
        }
    }
    Ok(())
}

macro_rules! define_id {
    ($ty:ident, $kind:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
        pub struct $ty(String);

        impl $ty {
            #[doc = concat!("Create a validated `", $kind, "` from a string slice.")]
            pub fn new(value: &str) -> Result<Self, CoreError> {
                validate($kind, value)?;
                Ok(Self(value.to_owned()))
            }

            /// Borrow the inner string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Lowercase normalized form for case-folded collision checks.
            ///
            /// The original case is preserved in [`Self::as_str`]; this helper
            /// provides the canonical form for duplicate detection.
            pub fn normalized(&self) -> String {
                self.0.to_lowercase()
            }

            /// Case-insensitive equality with another identifier of the same kind.
            pub fn eq_case_fold(&self, other: &Self) -> bool {
                self.0.to_lowercase() == other.0.to_lowercase()
            }

            /// Case-insensitive equality with a raw string.
            pub fn eq_case_fold_str(&self, other: &str) -> bool {
                self.0.to_lowercase() == other.to_lowercase()
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $ty {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Deref for $ty {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl Borrow<str> for $ty {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $ty {
            type Err = CoreError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::new(s)
            }
        }

        impl TryFrom<String> for $ty {
            type Error = CoreError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(&value)
            }
        }

        impl TryFrom<&str> for $ty {
            type Error = CoreError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                Self::new(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

define_id!(
    HarnessId,
    "HarnessId",
    "Stable lowercase slug identifying a harness, independent of executable or display name."
);
define_id!(
    InstanceId,
    "InstanceId",
    "Immutable generated identity for an instance; rename does not break references."
);
define_id!(
    InstanceName,
    "InstanceName",
    "User-chosen label and default wrapper command. Preserves original case, compares case-folded."
);
define_id!(
    TemplateId,
    "TemplateId",
    "Identifier for a template preset (harness + provider)."
);
define_id!(
    TemplateVersion,
    "TemplateVersion",
    "Version string for a template, e.g. `1.2.0`."
);
define_id!(
    ProviderId,
    "ProviderId",
    "Stable identifier for a provider."
);
define_id!(CapabilityId, "CapabilityId", "Identifier for a capability.");
define_id!(SkillId, "SkillId", "Identifier for a skill.");
define_id!(PluginId, "PluginId", "Identifier for a plugin.");
define_id!(McpServerId, "McpServerId", "Identifier for an MCP server.");
define_id!(
    OperationId,
    "OperationId",
    "Identifier for a mutating operation preview/commit cycle."
);
define_id!(BackupId, "BackupId", "Identifier for a backup artifact.");

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_valid<T>(value: &str)
    where
        T: FromStr<Err = CoreError> + AsRef<str> + fmt::Debug,
    {
        let id = T::from_str(value).unwrap_or_else(|e| panic!("expected valid `{value}`: {e:?}"));
        assert_eq!(id.as_ref(), value);
    }

    fn assert_invalid<T>(value: &str)
    where
        T: FromStr<Err = CoreError> + fmt::Debug,
    {
        assert!(
            T::from_str(value).is_err(),
            "expected invalid for `{value:?}`"
        );
    }

    #[test]
    fn valid_simple_names() {
        assert_valid::<HarnessId>("work");
        assert_valid::<HarnessId>("glm");
        assert_valid::<InstanceName>("work");
        assert_valid::<InstanceName>("My-Instance_123");
        assert_valid::<TemplateId>("claude-code-glm");
        assert_valid::<TemplateVersion>("1.2.0");
        assert_valid::<ProviderId>("anthropic");
        assert_valid::<CapabilityId>("web-search");
        assert_valid::<SkillId>("my-skill");
        assert_valid::<PluginId>("my-plugin");
        assert_valid::<McpServerId>("my-mcp");
        assert_valid::<OperationId>("op-123");
        assert_valid::<BackupId>("backup-2026-08-26");
    }

    #[test]
    fn unicode_where_safe() {
        assert_valid::<InstanceName>("café");
        assert_valid::<InstanceName>("テスト");
        assert_valid::<InstanceName>("my_instance-üñî");
        assert_valid::<HarnessId>("harness-测试");
        assert_valid::<TemplateId>("template-αβγ");
        assert_valid::<ProviderId>("prov-δelta");
        assert_valid::<SkillId>("スキル");
    }

    #[test]
    fn rejects_empty_and_dots() {
        assert_invalid::<InstanceName>("");
        assert_invalid::<InstanceName>(".");
        assert_invalid::<InstanceName>("..");
        assert_valid::<TemplateVersion>("1.2.0");
        assert_valid::<InstanceName>("a.b");
        assert_valid::<InstanceName>("a..b");
    }

    #[test]
    fn rejects_separators() {
        assert_invalid::<HarnessId>("a/b");
        assert_invalid::<HarnessId>("a\\b");
        assert_invalid::<HarnessId>("a:b");
        assert_invalid::<InstanceName>("foo/bar");
        assert_invalid::<TemplateId>("x:y");
        assert_invalid::<ProviderId>("/leading");
        assert_invalid::<ProviderId>("trailing/");
    }

    #[test]
    fn rejects_nul_and_control() {
        assert_invalid::<InstanceName>("a\0b");
        assert_invalid::<InstanceName>("a\nb");
        assert_invalid::<InstanceName>("a\rb");
        assert_invalid::<InstanceName>("a\tb");
        assert_invalid::<HarnessId>("a\u{0001}b");
        assert_invalid::<HarnessId>("a\u{001F}b");
        assert_invalid::<HarnessId>("a\u{007F}b");
        assert_invalid::<InstanceName>("a\u{0080}b");
    }

    #[test]
    fn rejects_trailing_dots_and_spaces() {
        assert_invalid::<InstanceName>("work.");
        assert_invalid::<InstanceName>("work ");
        assert_invalid::<InstanceName>("hello. ");
        assert_valid::<InstanceName>("work .inner");
        assert_invalid::<HarnessId>("trailing- ");
        assert_invalid::<TemplateVersion>("1.0. ");
        assert_invalid::<TemplateVersion>("1.0.");
    }

    #[test]
    fn reserved_windows_names() {
        let reserved = [
            "CON", "con", "Con", "PRN", "prn", "AUX", "aux", "NUL", "nul", "COM1", "com1", "COM9",
            "LPT1", "lpt9", "LPT9", "com2", "lpt5",
        ];
        for name in reserved {
            assert_invalid::<InstanceName>(name);
            assert_invalid::<HarnessId>(name);
        }
        assert_invalid::<InstanceName>("CON.txt");
        assert_invalid::<InstanceName>("con.json");
        assert_invalid::<InstanceName>("nul.json");
        assert_invalid::<InstanceName>("COM1.log");
        assert_invalid::<InstanceName>("lpt9.bak");
        assert_invalid::<ProviderId>("Aux.md");
        assert_valid::<InstanceName>("CONN");
        assert_valid::<InstanceName>("con1");
        assert_valid::<InstanceName>("my-CON");
        assert_valid::<InstanceName>("CON-2");
        assert_valid::<InstanceName>("aCON");
    }

    #[test]
    fn valid_harness_prefix_not_required() {
        assert_valid::<InstanceName>("work");
        assert_valid::<InstanceName>("glm");
        assert_valid::<HarnessId>("work");
        assert_valid::<HarnessId>("glm");
    }

    #[test]
    fn case_folding_preserves_original() {
        let a = InstanceName::new("Work").unwrap();
        let b = InstanceName::new("work").unwrap();
        let c = InstanceName::new("WORK").unwrap();
        assert_eq!(a.as_str(), "Work");
        assert_eq!(b.as_str(), "work");
        assert_ne!(a, b);
        assert!(a.eq_case_fold(&b));
        assert!(a.eq_case_fold(&c));
        assert!(b.eq_case_fold_str("WORK"));
        assert!(a.eq_case_fold_str("work"));
        assert_eq!(a.normalized(), "work");
        assert_eq!(c.normalized(), "work");
        let h1 = HarnessId::new("Claude-Code").unwrap();
        let h2 = HarnessId::new("claude-code").unwrap();
        assert_ne!(h1, h2);
        assert!(h1.eq_case_fold(&h2));
    }

    #[test]
    fn serialization_round_trip() {
        let ids = [
            HarnessId::new("claude-code").unwrap().as_str().to_owned(),
            InstanceName::new("work").unwrap().as_str().to_owned(),
            TemplateVersion::new("1.2.0").unwrap().as_str().to_owned(),
        ];
        for original in ids {
            let json = serde_json::to_string(&original).unwrap();
            let back: String = serde_json::from_str(&json).unwrap();
            assert_eq!(original, back);
        }
        let original = InstanceName::new("my-instance-1").unwrap();
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, "\"my-instance-1\"");
        let decoded: InstanceName = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
        let harness = HarnessId::new("kilo-code").unwrap();
        let json = serde_json::to_string(&harness).unwrap();
        let decoded: HarnessId = serde_json::from_str(&json).unwrap();
        assert_eq!(harness, decoded);
        let ver = TemplateVersion::new("2.0.0-beta.1").unwrap();
        let json = serde_json::to_string(&ver).unwrap();
        let decoded: TemplateVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(ver, decoded);
        let uni = InstanceName::new("café-測試").unwrap();
        let json = serde_json::to_string(&uni).unwrap();
        let decoded: InstanceName = serde_json::from_str(&json).unwrap();
        assert_eq!(uni, decoded);
    }

    #[test]
    fn deserialize_rejects_invalid() {
        let bad_json = "\"CON\"";
        let res: Result<InstanceName, _> = serde_json::from_str(bad_json);
        res.unwrap_err();
        let bad_json = "\".\"";
        let res: Result<HarnessId, _> = serde_json::from_str(bad_json);
        res.unwrap_err();
        let bad_json = "\"a/b\"";
        let res: Result<ProviderId, _> = serde_json::from_str(bad_json);
        res.unwrap_err();
        let bad_json = "\"trailing \"";
        let res: Result<SkillId, _> = serde_json::from_str(bad_json);
        res.unwrap_err();
    }

    #[test]
    fn all_types_validate_consistently() {
        let invalids = [
            "", ".", "..", "a/b", "a:b", "x\\y", "bad.", "bad ", "CON", "nul", "COM1",
        ];
        for val in invalids {
            assert!(
                HarnessId::new(val).is_err(),
                "HarnessId should reject {val:?}"
            );
            assert!(
                InstanceId::new(val).is_err(),
                "InstanceId should reject {val:?}"
            );
            assert!(
                InstanceName::new(val).is_err(),
                "InstanceName should reject {val:?}"
            );
            assert!(
                TemplateId::new(val).is_err(),
                "TemplateId should reject {val:?}"
            );
            assert!(
                TemplateVersion::new(val).is_err(),
                "TemplateVersion should reject {val:?}"
            );
            assert!(
                ProviderId::new(val).is_err(),
                "ProviderId should reject {val:?}"
            );
            assert!(
                CapabilityId::new(val).is_err(),
                "CapabilityId should reject {val:?}"
            );
            assert!(SkillId::new(val).is_err(), "SkillId should reject {val:?}");
            assert!(
                PluginId::new(val).is_err(),
                "PluginId should reject {val:?}"
            );
            assert!(
                McpServerId::new(val).is_err(),
                "McpServerId should reject {val:?}"
            );
            assert!(
                OperationId::new(val).is_err(),
                "OperationId should reject {val:?}"
            );
            assert!(
                BackupId::new(val).is_err(),
                "BackupId should reject {val:?}"
            );
        }
    }

    #[test]
    fn all_types_accept_valid() {
        let valids = ["work", "glm", "my-id_123", "café", "1.2.0", "a-b.c_d"];
        for val in valids {
            HarnessId::new(val).unwrap();
            InstanceId::new(val).unwrap();
            InstanceName::new(val).unwrap();
            TemplateId::new(val).unwrap();
            TemplateVersion::new(val).unwrap();
            ProviderId::new(val).unwrap();
            CapabilityId::new(val).unwrap();
            SkillId::new(val).unwrap();
            PluginId::new(val).unwrap();
            McpServerId::new(val).unwrap();
            OperationId::new(val).unwrap();
            BackupId::new(val).unwrap();
        }
    }

    #[test]
    fn display_and_deref() {
        let id = InstanceName::new("work").unwrap();
        assert_eq!(format!("{id}"), "work");
        assert_eq!(&*id, "work");
        assert_eq!(id.as_ref() as &str, "work");
        let borrowed: &str = &id;
        assert_eq!(borrowed, "work");
    }

    #[test]
    fn try_from_and_from_str() {
        let a = InstanceName::try_from("hello").unwrap();
        assert_eq!(a.as_str(), "hello");
        let b = InstanceName::try_from(String::from("hello2")).unwrap();
        assert_eq!(b.as_str(), "hello2");
        let c: InstanceName = "hello3".parse().unwrap();
        assert_eq!(c.as_str(), "hello3");
        "".parse::<InstanceName>().unwrap_err();
    }

    #[test]
    fn json_struct_round_trip() {
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
        struct Wrapper {
            harness: HarnessId,
            instance: InstanceName,
            template: TemplateId,
            version: TemplateVersion,
        }
        let w = Wrapper {
            harness: HarnessId::new("claude-code").unwrap(),
            instance: InstanceName::new("work-café").unwrap(),
            template: TemplateId::new("claude-glm").unwrap(),
            version: TemplateVersion::new("1.2.3").unwrap(),
        };
        let json = serde_json::to_string(&w).unwrap();
        let decoded: Wrapper = serde_json::from_str(&json).unwrap();
        assert_eq!(w, decoded);
    }
}
