use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
/// Enumeration: billing unit.
pub enum BillingUnit {
    /// Variant: per1 mtokens.
    Per1MTokens,
    /// Variant: per1 mtokens hour.
    Per1MTokensHour,
    /// Variant: per request.
    PerRequest,
    /// Variant: fixed.
    Fixed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
/// Data type: billing dimension.
pub struct BillingDimension {
    /// Field: name.
    pub name: String,
    /// Field: usage field.
    pub usage_field: String,
    /// Field: price field.
    pub price_field: String,
    /// Field: unit.
    pub unit: BillingUnit,
    /// Field: default price.
    pub default_price: f64,
}

impl BillingDimension {
    /// Method: calculate.
    pub fn calculate(&self, usage_value: f64, price: f64) -> f64 {
        if usage_value <= 0.0 || price <= 0.0 {
            return 0.0;
        }
        match self.unit {
            BillingUnit::Per1MTokens | BillingUnit::Per1MTokensHour => {
                (usage_value / 1_000_000.0) * price
            }
            BillingUnit::PerRequest => usage_value * price,
            BillingUnit::Fixed => price,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
/// Data type: cost breakdown.
pub struct CostBreakdown {
    /// Field: costs.
    pub costs: BTreeMap<String, f64>,
    /// Field: total cost.
    pub total_cost: f64,
    /// Field: tier index.
    pub tier_index: Option<i64>,
    /// Field: effective prices.
    pub effective_prices: BTreeMap<String, f64>,
}

#[cfg(test)]
mod tests {
    use super::{BillingDimension, BillingUnit};

    #[test]
    fn dimension_calculates_per_million_tokens() {
        let dimension = BillingDimension {
            name: "input".to_string(),
            usage_field: "input_tokens".to_string(),
            price_field: "input_price_per_1m".to_string(),
            unit: BillingUnit::Per1MTokens,
            default_price: 0.0,
        };
        assert_eq!(dimension.calculate(500_000.0, 2.0), 1.0);
    }
}
