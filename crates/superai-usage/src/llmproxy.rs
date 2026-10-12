//! Client for a deployed llmproxy's `/usage` ledger endpoints (USG-08,
//! subplan 15; endpoint contract in docs/plans/export/llmproxy-usage-ledger.md).

use serde::{Deserialize, Serialize};

/// Aggregate endpoint a deployed ledger serves (PRX-03).
pub const SUMMARY_PATH: &str = "/usage/summary";
/// Raw-row endpoint a deployed ledger serves (PRX-03); responses carry no
/// request bodies, ever.
pub const REQUESTS_PATH: &str = "/usage/requests";

/// Grouping `/usage/summary` accepts for its `group_by` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryGroupBy {
    /// One row per provider.
    Provider,
    /// One row per model.
    Model,
    /// One row per client key id.
    Caller,
}

impl SummaryGroupBy {
    /// Wire value for the `group_by` query parameter.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Model => "model",
            Self::Caller => "caller",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_by_wire_values_match_the_ledger_contract() {
        assert_eq!(SummaryGroupBy::Provider.as_str(), "provider");
        assert_eq!(SummaryGroupBy::Model.as_str(), "model");
        assert_eq!(SummaryGroupBy::Caller.as_str(), "caller");
    }
}
