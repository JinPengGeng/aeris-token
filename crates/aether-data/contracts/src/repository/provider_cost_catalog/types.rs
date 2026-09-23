use serde_json::Value;

use crate::DataLayerError;

pub const PROVIDER_COST_CATALOG_MAX_ID_LEN: usize = 128;
pub const PROVIDER_COST_CATALOG_MAX_MODEL_LEN: usize = 255;
pub const PROVIDER_COST_CATALOG_MAX_CURRENCY_LEN: usize = 16;
pub const PROVIDER_COST_CATALOG_MAX_OPERATOR_LEN: usize = 128;
pub const PROVIDER_COST_CATALOG_MAX_TIERED_PRICING_BYTES: usize = 64 * 1024;
pub const PROVIDER_COST_CATALOG_MAX_LIST_LIMIT: usize = 200;

/// Token price fields shared with the sales-side `models.tiered_pricing` catalog.
const TOKEN_PRICE_FIELDS: &[&str] = &[
    "input_price_per_1m",
    "output_price_per_1m",
    "cache_read_price_per_1m",
    "cache_write_price_per_1m",
    "cache_creation_price_per_1m",
    "total_input_price_per_1m",
];
/// Flat image price fields shared with the sales-side image output catalog.
const IMAGE_PRICE_FIELDS: &[&str] = &["price_per_image", "low", "medium", "high", "price", "value"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCostTaskType {
    Text,
    Image,
}

impl ProviderCostTaskType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DataLayerError> {
        match value {
            "text" => Ok(Self::Text),
            "image" => Ok(Self::Image),
            _ => Err(DataLayerError::UnexpectedValue(format!(
                "unknown provider cost task type: {value}"
            ))),
        }
    }
}

/// A stored provider cost catalog entry. The `price_per_request` /
/// `tiered_pricing` pair is deliberately isomorphic to the sales-side
/// `BillingModelPricingSnapshot` catalog shape so PR-B can run the same
/// formula engine over cost and price catalogs without translation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCostCatalogRecord {
    pub cost_id: String,
    pub provider_id: String,
    pub model: String,
    pub task_type: ProviderCostTaskType,
    pub currency: String,
    pub price_per_request: Option<f64>,
    pub tiered_pricing: Option<Value>,
    pub effective_from_unix_secs: u64,
    pub effective_to_unix_secs: Option<u64>,
    pub created_by: String,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCostCatalogUpsertOutcome {
    Inserted,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCostCatalogListQuery {
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub task_type: Option<ProviderCostTaskType>,
    /// When set, only catalogs whose effective window contains this instant are returned.
    pub effective_at_unix_secs: Option<u64>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCostCatalogDeleteOutcome {
    Deleted,
    NotFound,
}

impl ProviderCostCatalogRecord {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        bounded_id("cost_id", &self.cost_id, PROVIDER_COST_CATALOG_MAX_ID_LEN)?;
        bounded_id(
            "provider_id",
            &self.provider_id,
            PROVIDER_COST_CATALOG_MAX_ID_LEN,
        )?;
        bounded_value(
            "model",
            &self.model,
            PROVIDER_COST_CATALOG_MAX_MODEL_LEN,
            false,
        )?;
        bounded_value(
            "currency",
            &self.currency,
            PROVIDER_COST_CATALOG_MAX_CURRENCY_LEN,
            true,
        )?;
        bounded_value(
            "created_by",
            &self.created_by,
            PROVIDER_COST_CATALOG_MAX_OPERATOR_LEN,
            true,
        )?;
        if let Some(price) = self.price_per_request {
            validate_price("price_per_request", &Value::from(price))?;
        }
        if let Some(catalog) = &self.tiered_pricing {
            validate_provider_cost_catalog_tiered_pricing(catalog)?;
        }
        if self.price_per_request.is_none() && self.tiered_pricing.is_none() {
            return Err(DataLayerError::InvalidInput(
                "provider cost catalog requires price_per_request or tiered_pricing".into(),
            ));
        }
        timestamp("effective_from_unix_secs", self.effective_from_unix_secs)?;
        if let Some(effective_to_unix_secs) = self.effective_to_unix_secs {
            timestamp("effective_to_unix_secs", effective_to_unix_secs)?;
            if effective_to_unix_secs <= self.effective_from_unix_secs {
                return Err(DataLayerError::InvalidInput(
                    "provider cost catalog effective_to must be after effective_from".into(),
                ));
            }
        }
        timestamp("created_at_unix_secs", self.created_at_unix_secs)?;
        timestamp("updated_at_unix_secs", self.updated_at_unix_secs)?;
        Ok(())
    }
}

/// Validates that a cost catalog uses the same JSON shape as the sales-side
/// `models.tiered_pricing` catalog: an object with an optional `tiers` array of
/// objects carrying non-negative finite numeric price fields, an optional
/// `image_output_tiers`-style nested price structure, an optional
/// `cache_ttl_pricing` array, and an optional `processing_tiers` overlay object.
pub fn validate_provider_cost_catalog_tiered_pricing(value: &Value) -> Result<(), DataLayerError> {
    let encoded = serde_json::to_vec(value).map_err(|error| {
        DataLayerError::UnexpectedValue(format!(
            "provider cost catalog tiered pricing is not encodable: {error}"
        ))
    })?;
    if encoded.len() > PROVIDER_COST_CATALOG_MAX_TIERED_PRICING_BYTES {
        return Err(DataLayerError::InvalidInput(format!(
            "provider cost catalog tiered pricing exceeds {} bytes",
            PROVIDER_COST_CATALOG_MAX_TIERED_PRICING_BYTES
        )));
    }
    validate_catalog_object(value)
}

fn validate_catalog_object(value: &Value) -> Result<(), DataLayerError> {
    let object = value.as_object().ok_or_else(|| {
        DataLayerError::UnexpectedValue("tiered_pricing must be an object".into())
    })?;
    if let Some(tiers) = object.get("tiers") {
        if tiers.is_null() {
            return Ok(());
        }
        let tiers = tiers.as_array().ok_or_else(|| {
            DataLayerError::UnexpectedValue("tiered_pricing.tiers must be an array".into())
        })?;
        for (index, tier) in tiers.iter().enumerate() {
            validate_catalog_tier(tier)
                .map_err(|error| annotate(error, &format!("tiers[{index}]")))?;
        }
    }
    validate_overlay_fields(object)
}

fn validate_catalog_tier(tier: &Value) -> Result<(), DataLayerError> {
    let object = tier.as_object().ok_or_else(|| {
        DataLayerError::UnexpectedValue("tiered_pricing tier must be an object".into())
    })?;
    validate_overlay_fields(object)
}

fn validate_overlay_fields(object: &serde_json::Map<String, Value>) -> Result<(), DataLayerError> {
    for (key, value) in object {
        if value.is_null() {
            continue;
        }
        if TOKEN_PRICE_FIELDS.contains(&key.as_str()) || IMAGE_PRICE_FIELDS.contains(&key.as_str())
        {
            validate_price(key, value)?;
        } else if key == "price_multiplier" {
            let multiplier = value.as_f64().ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "tiered_pricing.{key} must be a non-negative finite number"
                ))
            })?;
            if !multiplier.is_finite() || multiplier < 0.0 {
                return Err(DataLayerError::UnexpectedValue(format!(
                    "tiered_pricing.{key} must be a non-negative finite number"
                )));
            }
        } else if key == "cache_ttl_pricing" {
            let entries = value.as_array().ok_or_else(|| {
                DataLayerError::UnexpectedValue(
                    "tiered_pricing.cache_ttl_pricing must be an array".into(),
                )
            })?;
            for (index, entry) in entries.iter().enumerate() {
                let entry_object = entry.as_object().ok_or_else(|| {
                    DataLayerError::UnexpectedValue(format!(
                        "tiered_pricing.cache_ttl_pricing[{index}] must be an object"
                    ))
                })?;
                if let Some(ttl) = entry_object.get("ttl_minutes") {
                    let ttl = ttl.as_i64().ok_or_else(|| {
                        DataLayerError::UnexpectedValue(format!(
                            "tiered_pricing.cache_ttl_pricing[{index}].ttl_minutes must be an integer"
                        ))
                    })?;
                    if ttl < 0 {
                        return Err(DataLayerError::UnexpectedValue(format!(
                            "tiered_pricing.cache_ttl_pricing[{index}].ttl_minutes must be non-negative"
                        )));
                    }
                }
                validate_overlay_fields(entry_object)
                    .map_err(|error| annotate(error, &format!("cache_ttl_pricing[{index}]")))?;
            }
        } else if key == "processing_tiers" {
            let overlays = value.as_object().ok_or_else(|| {
                DataLayerError::UnexpectedValue(
                    "tiered_pricing.processing_tiers must be an object".into(),
                )
            })?;
            for (tier_name, overlay) in overlays {
                let overlay_object = overlay.as_object().ok_or_else(|| {
                    DataLayerError::UnexpectedValue(format!(
                        "tiered_pricing.processing_tiers.{tier_name} must be an object"
                    ))
                })?;
                if overlay_object.keys().any(|key| key == "tiers") {
                    validate_catalog_object(overlay).map_err(|error| {
                        annotate(error, &format!("processing_tiers.{tier_name}"))
                    })?;
                } else {
                    validate_overlay_fields(overlay_object).map_err(|error| {
                        annotate(error, &format!("processing_tiers.{tier_name}"))
                    })?;
                    if !overlay_object.keys().any(|key| key == "price_multiplier") {
                        return Err(DataLayerError::UnexpectedValue(format!(
                            "tiered_pricing.processing_tiers.{tier_name} requires price_multiplier or an explicit tiers catalog"
                        )));
                    }
                }
            }
        } else if let Some(entries) = value.as_array() {
            // Nested structures such as image_output_tiers carry per-entry price fields.
            for (index, entry) in entries.iter().enumerate() {
                if let Some(entry_object) = entry.as_object() {
                    validate_overlay_fields(entry_object)
                        .map_err(|error| annotate(error, &format!("{key}[{index}]")))?;
                }
            }
        }
    }
    Ok(())
}

fn validate_price(field: &str, value: &Value) -> Result<(), DataLayerError> {
    if value.is_null() {
        return Ok(());
    }
    let price = value.as_f64().ok_or_else(|| {
        DataLayerError::UnexpectedValue(format!(
            "provider cost catalog {field} must be a non-negative finite number"
        ))
    })?;
    if !price.is_finite() || price < 0.0 {
        return Err(DataLayerError::UnexpectedValue(format!(
            "provider cost catalog {field} must be a non-negative finite number"
        )));
    }
    Ok(())
}

fn annotate(error: DataLayerError, segment: &str) -> DataLayerError {
    match error {
        DataLayerError::UnexpectedValue(detail) => {
            DataLayerError::UnexpectedValue(format!("tiered_pricing.{segment}: {detail}"))
        }
        other => other,
    }
}

fn timestamp(field: &str, value: u64) -> Result<(), DataLayerError> {
    if value > i64::MAX as u64 {
        return Err(DataLayerError::InvalidInput(format!(
            "provider cost catalog {field} exceeds the integer range"
        )));
    }
    Ok(())
}

fn bounded_id(field: &str, value: &str, max_len: usize) -> Result<(), DataLayerError> {
    bounded_value(field, value, max_len, false)
}

fn bounded_value(
    field: &str,
    value: &str,
    max_len: usize,
    allow_empty: bool,
) -> Result<(), DataLayerError> {
    if (value.is_empty() && !allow_empty)
        || value.len() > max_len
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(DataLayerError::InvalidInput(format!(
            "invalid provider cost catalog {field}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record() -> ProviderCostCatalogRecord {
        ProviderCostCatalogRecord {
            cost_id: "cost-a".to_string(),
            provider_id: "provider-a".to_string(),
            model: "gpt-x".to_string(),
            task_type: ProviderCostTaskType::Text,
            currency: "USD".to_string(),
            price_per_request: None,
            tiered_pricing: Some(json!({
                "tiers": [{
                    "input_price_per_1m": 0.5,
                    "output_price_per_1m": 1.5,
                    "cache_read_price_per_1m": 0.1,
                    "cache_ttl_pricing": [
                        {"ttl_minutes": 5, "cache_write_price_per_1m": 0.2}
                    ]
                }],
                "processing_tiers": {
                    "priority": {"price_multiplier": 2.0}
                }
            })),
            effective_from_unix_secs: 1_000,
            effective_to_unix_secs: Some(2_000),
            created_by: "admin-a".to_string(),
            created_at_unix_secs: 900,
            updated_at_unix_secs: 900,
        }
    }

    #[test]
    fn task_type_round_trips() {
        assert_eq!(ProviderCostTaskType::Text.as_str(), "text");
        assert_eq!(ProviderCostTaskType::Image.as_str(), "image");
        assert_eq!(
            ProviderCostTaskType::parse("text").expect("text parses"),
            ProviderCostTaskType::Text
        );
        assert!(ProviderCostTaskType::parse("video").is_err());
    }

    #[test]
    fn valid_record_passes() {
        assert!(record().validate().is_ok());
    }

    #[test]
    fn record_requires_some_pricing() {
        let mut value = record();
        value.tiered_pricing = None;
        assert!(value.validate().is_err());
    }

    #[test]
    fn record_rejects_invalid_effective_window_and_overflow() {
        let mut value = record();
        value.effective_to_unix_secs = Some(value.effective_from_unix_secs);
        assert!(value.validate().is_err());

        let mut value = record();
        value.created_at_unix_secs = i64::MAX as u64 + 1;
        assert!(value.validate().is_err());
    }

    #[test]
    fn record_rejects_bad_ids_and_prices() {
        let mut value = record();
        value.provider_id = " has whitespace ".to_string();
        assert!(value.validate().is_err());

        let mut value = record();
        value.price_per_request = Some(-1.0);
        assert!(value.validate().is_err());
    }

    #[test]
    fn catalog_accepts_sales_isomorphic_shapes() {
        assert!(validate_provider_cost_catalog_tiered_pricing(&json!({
            "tiers": [{"input_price_per_1m": 1.0, "output_price_per_1m": 2.0}],
            "image_output_tiers": [
                {"size": "1K", "qualities": {"high": {"price_per_image": 0.04}}}
            ]
        }))
        .is_ok());
        assert!(validate_provider_cost_catalog_tiered_pricing(&json!({
            "processing_tiers": {
                "priority": {"tiers": [{"input_price_per_1m": 3.0}]}
            }
        }))
        .is_ok());
    }

    #[test]
    fn catalog_rejects_malformed_shapes() {
        assert!(validate_provider_cost_catalog_tiered_pricing(&json!(["not-object"])).is_err());
        assert!(validate_provider_cost_catalog_tiered_pricing(&json!({"tiers": "nope"})).is_err());
        assert!(validate_provider_cost_catalog_tiered_pricing(
            &json!({"tiers": [{"input_price_per_1m": -0.5}]})
        )
        .is_err());
        assert!(validate_provider_cost_catalog_tiered_pricing(
            &json!({"tiers": [{"input_price_per_1m": "1 + 1"}]})
        )
        .is_err());
        assert!(validate_provider_cost_catalog_tiered_pricing(
            &json!({"tiers": [{"cache_ttl_pricing": [{"ttl_minutes": -5}]}]})
        )
        .is_err());
        assert!(validate_provider_cost_catalog_tiered_pricing(
            &json!({"processing_tiers": {"priority": {}}})
        )
        .is_err());
        assert!(validate_provider_cost_catalog_tiered_pricing(
            &json!({"processing_tiers": {"priority": {"price_multiplier": -1.0}}})
        )
        .is_err());
        assert!(
            validate_provider_cost_catalog_tiered_pricing(&json!({"tiers": [{}]})).is_ok(),
            "empty tiers mirror the sales catalog legacy rows"
        );
    }
}
