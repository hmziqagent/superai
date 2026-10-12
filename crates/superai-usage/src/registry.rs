//! Usage provider registry (USG-07): data-only entries keyed by provider id.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use superai_core::ids::ProviderId;

use crate::error::{Result, UsageError};

/// One provider's usage-truth declaration. Data only: labels, locators,
/// auth-material kinds, documentation links. Never secret values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageProviderEntry {
    /// Provider this entry describes.
    pub id: ProviderId,
    /// Human-readable provider label.
    pub label: String,
}

/// Registry of usage provider entries, keyed by provider id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageRegistry {
    entries: BTreeMap<ProviderId, UsageProviderEntry>,
}

impl UsageRegistry {
    /// Build from entries; duplicate provider ids are rejected.
    pub fn from_entries(entries: impl IntoIterator<Item = UsageProviderEntry>) -> Result<Self> {
        let mut map = BTreeMap::new();
        for entry in entries {
            if map.contains_key(&entry.id) {
                return Err(UsageError::DuplicateProvider {
                    id: entry.id.as_str().to_owned(),
                });
            }
            map.insert(entry.id.clone(), entry);
        }
        Ok(Self { entries: map })
    }

    /// Look up an entry by provider id.
    pub fn get(&self, id: &ProviderId) -> Option<&UsageProviderEntry> {
        self.entries.get(id)
    }

    /// Iterate entries ordered by provider id.
    pub fn entries(&self) -> impl Iterator<Item = &UsageProviderEntry> {
        self.entries.values()
    }

    /// Number of registered entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, label: &str) -> UsageProviderEntry {
        let id = ProviderId::new(id).unwrap_or_else(|e| panic!("invalid id `{id}`: {e:?}"));
        UsageProviderEntry {
            id,
            label: label.to_owned(),
        }
    }

    fn provider_id(id: &str) -> ProviderId {
        ProviderId::new(id).unwrap_or_else(|e| panic!("invalid id `{id}`: {e:?}"))
    }

    #[test]
    fn lookup_returns_the_entry_for_its_own_id_only() {
        let registry = UsageRegistry::from_entries([entry("anthropic", "Anthropic")])
            .unwrap_or_else(|e| panic!("valid entries rejected: {e:?}"));
        let found = registry.get(&provider_id("anthropic"));
        assert_eq!(found.map(|e| e.label.as_str()), Some("Anthropic"));
        assert!(registry.get(&provider_id("openrouter")).is_none());
    }

    #[test]
    fn duplicate_provider_ids_are_rejected() {
        let result = UsageRegistry::from_entries([
            entry("zed-cloud", "Zed"),
            entry("zed-cloud", "Zed duplicate"),
        ]);
        assert_eq!(
            result.unwrap_err().to_string(),
            "duplicate usage provider entry `zed-cloud`"
        );
    }

    #[test]
    fn entries_iterate_in_provider_id_order() {
        let registry = UsageRegistry::from_entries([
            entry("zed-cloud", "Zed"),
            entry("anthropic", "Anthropic"),
            entry("openrouter", "OpenRouter"),
        ])
        .unwrap_or_else(|e| panic!("valid entries rejected: {e:?}"));
        let ids: Vec<&str> = registry.entries().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["anthropic", "openrouter", "zed-cloud"]);
        assert_eq!(registry.len(), 3);
        assert!(!registry.is_empty());
    }

    #[test]
    fn empty_registry_is_constructible_and_queries_none() {
        let registry =
            UsageRegistry::from_entries([]).unwrap_or_else(|e| panic!("empty rejected: {e:?}"));
        assert!(registry.is_empty());
        assert!(registry.get(&provider_id("anthropic")).is_none());
        assert_eq!(registry.entries().count(), 0);
    }
}
