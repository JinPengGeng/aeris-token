use std::collections::BTreeMap;

/// Constant: billing snapshot schema version.
pub const BILLING_SNAPSHOT_SCHEMA_VERSION: &str = "2.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
/// Enumeration: billing snapshot status.
pub enum BillingSnapshotStatus {
    /// Variant: complete.
    Complete,
    /// Variant: incomplete.
    Incomplete,
    /// Variant: no rule.
    NoRule,
    /// Variant: legacy.
    Legacy,
}

impl BillingSnapshotStatus {
    /// Method: as str.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::NoRule => "no_rule",
            Self::Legacy => "legacy",
        }
    }
}

impl std::fmt::Display for BillingSnapshotStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
/// Data type: billing snapshot.
pub struct BillingSnapshot {
    /// Field: schema version.
    pub schema_version: String,
    /// Field: rule id.
    pub rule_id: Option<String>,
    /// Field: rule name.
    pub rule_name: Option<String>,
    /// Field: scope.
    pub scope: Option<String>,
    /// Field: expression.
    pub expression: Option<String>,
    /// Field: resolved dimensions.
    pub resolved_dimensions: BTreeMap<String, serde_json::Value>,
    /// Field: resolved variables.
    pub resolved_variables: BTreeMap<String, serde_json::Value>,
    /// Field: cost breakdown.
    pub cost_breakdown: BTreeMap<String, f64>,
    /// Field: total cost.
    pub total_cost: f64,
    /// Field: tier index.
    pub tier_index: Option<i64>,
    /// Field: tier info.
    pub tier_info: Option<serde_json::Value>,
    /// Field: missing required.
    pub missing_required: Vec<String>,
    /// Field: status.
    pub status: BillingSnapshotStatus,
    /// Field: calculated at.
    pub calculated_at: String,
    /// Field: engine version.
    pub engine_version: String,
}

impl Default for BillingSnapshot {
    fn default() -> Self {
        Self {
            schema_version: BILLING_SNAPSHOT_SCHEMA_VERSION.to_string(),
            rule_id: None,
            rule_name: None,
            scope: None,
            expression: None,
            resolved_dimensions: BTreeMap::new(),
            resolved_variables: BTreeMap::new(),
            cost_breakdown: BTreeMap::new(),
            total_cost: 0.0,
            tier_index: None,
            tier_info: None,
            missing_required: Vec::new(),
            status: BillingSnapshotStatus::NoRule,
            calculated_at: String::new(),
            engine_version: "2.0".to_string(),
        }
    }
}

impl BillingSnapshot {
    /// Method: dimensions used.
    pub fn dimensions_used(&self) -> &BTreeMap<String, serde_json::Value> {
        &self.resolved_dimensions
    }

    /// Method: cost.
    pub fn cost(&self) -> f64 {
        self.total_cost
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
/// Data type: cost result.
pub struct CostResult {
    /// Field: cost.
    pub cost: f64,
    /// Field: status.
    pub status: BillingSnapshotStatus,
    /// Field: snapshot.
    pub snapshot: BillingSnapshot,
}

#[cfg(test)]
mod tests {
    use super::{BillingSnapshot, BillingSnapshotStatus};

    #[test]
    fn snapshot_aliases_dimensions_and_cost() {
        let mut snapshot = BillingSnapshot {
            total_cost: 1.25,
            status: BillingSnapshotStatus::Complete,
            ..BillingSnapshot::default()
        };
        snapshot
            .resolved_dimensions
            .insert("input_tokens".to_string(), serde_json::json!(10));

        assert_eq!(snapshot.cost(), 1.25);
        assert_eq!(
            snapshot.dimensions_used().get("input_tokens"),
            Some(&serde_json::json!(10))
        );
    }
}
