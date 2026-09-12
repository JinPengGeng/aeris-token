//! Bounded image quotes. Callers must reserve funds before using a quote to admit
//! paid work; the legacy gateway estimate deliberately continues to reject it.

use serde::{Deserialize, Serialize};

use super::{
    authorization_pricing_candidates, billing_computation_is_bounded, image_output_pricing_state,
    normalize_image_output_quality, normalize_image_output_size, parse_image_size_pixels,
    resolve_image_output_price_resolution, BillingService,
};
use crate::{
    normalize_total_input_context_for_cache_hit_rate, BillingComputation,
    BillingModelPricingSnapshot, BillingUsageInput, ExpressionEvaluationError,
};

const COST_UNITS_PER_USD: f64 = 100_000_000.0;
const MAX_OUTPUT_VARIANTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingImageOutputDimensions {
    pub size: String,
    pub quality: String,
}

impl BillingImageOutputDimensions {
    fn normalized(&self) -> Option<Self> {
        let size = normalize_image_output_size(&self.size);
        let quality = normalize_image_output_quality(&self.quality);
        parse_image_size_pixels(&size)?;
        if !matches!(quality.as_str(), "low" | "medium" | "high") {
            return None;
        }
        Some(Self { size, quality })
    }
}

/// Bounds for the entire upstream operation, including edits, previews and all
/// returned images. A request's text length is not proof of an image token bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillingImageTokenBounds {
    pub max_total_input_tokens: i64,
    pub max_output_tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingImageAuthorizationInput {
    pub image_count: u32,
    /// The limit from the final provider projection, never a client assertion.
    pub max_image_count: u32,
    pub operation: String,
    pub size: Option<String>,
    pub quality: Option<String>,
    pub output_format: Option<String>,
    pub partial_images: u8,
    /// The exhaustive set allowed by the final provider/model projection. For
    /// auto/default dimensions, callers unable to prove this set cannot quote.
    pub possible_outputs: Vec<BillingImageOutputDimensions>,
    pub api_format: Option<String>,
    pub requested_processing_tier: Option<String>,
    pub api_key_multiplier: f64,
    pub token_bounds: Option<BillingImageTokenBounds>,
}

/// Frozen inputs and pricing for a financial reservation. Keep the complete
/// serialized quote with the server-owned reservation; it is not a client token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingImageAuthorizationQuote {
    schema_version: u32,
    pricing: BillingModelPricingSnapshot,
    input: BillingImageAuthorizationInput,
    upper_bound_units: i64,
}

impl BillingImageAuthorizationQuote {
    pub fn upper_bound_units(&self) -> i64 {
        self.upper_bound_units
    }

    pub fn input(&self) -> &BillingImageAuthorizationInput {
        &self.input
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingImageQuotedCalculation {
    pub computation: BillingComputation,
    pub calculated_units: i64,
    pub collectible_units: i64,
    pub excess_units: i64,
    /// Preserve actual cost and collect no more than the authorized ceiling.
    pub requires_reconciliation: bool,
}

fn ceil_cost_units(usd: f64) -> Option<i64> {
    let units = (usd * COST_UNITS_PER_USD).ceil();
    // i64::MAX rounds to 2^63 as f64, so equality must also be rejected.
    (usd.is_finite() && usd >= 0.0 && units.is_finite() && units < i64::MAX as f64)
        .then_some(units as i64)
}

fn settled_cost_units(usd: f64) -> Option<i64> {
    let units = (crate::quantize_cost(usd) * COST_UNITS_PER_USD).round();
    (usd.is_finite() && usd >= 0.0 && units.is_finite() && units < i64::MAX as f64)
        .then_some(units as i64)
}

fn valid_input(input: &BillingImageAuthorizationInput) -> bool {
    input.image_count > 0
        && input.image_count <= input.max_image_count
        && input.max_image_count <= 10
        && matches!(input.operation.as_str(), "generate" | "edit")
        && input.partial_images <= 3
        && input.api_key_multiplier.is_finite()
        && input.api_key_multiplier >= 0.0
        && input
            .output_format
            .as_deref()
            .is_none_or(|format| matches!(format, "png" | "jpeg" | "webp"))
        && input.token_bounds.as_ref().is_none_or(|bounds| {
            bounds.max_total_input_tokens >= 0 && bounds.max_output_tokens >= 0
        })
        && !input.possible_outputs.is_empty()
        && input.possible_outputs.len() <= MAX_OUTPUT_VARIANTS
}

fn requested_dimension_matches(requested: Option<&str>, actual: &str, size: bool) -> bool {
    let Some(requested) = requested else {
        return true;
    };
    let normalized = if size {
        normalize_image_output_size(requested)
    } else {
        normalize_image_output_quality(requested)
    };
    normalized == "auto" || normalized == actual
}

fn token_catalog_has_cost(catalog: Option<&serde_json::Value>) -> Option<bool> {
    fn inspect(value: &serde_json::Value, has_cost: &mut bool) -> Option<()> {
        match value {
            serde_json::Value::Array(values) => {
                for value in values {
                    inspect(value, has_cost)?;
                }
            }
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    if key.ends_with("_price_per_1m") && !value.is_null() {
                        let price = value.as_f64()?;
                        if !price.is_finite() || price < 0.0 {
                            return None;
                        }
                        *has_cost |= price > 0.0;
                    } else if key == "cache_ttl_pricing" {
                        inspect(value, has_cost)?;
                    }
                }
            }
            _ => {}
        }
        Some(())
    }
    let mut has_cost = false;
    for tier in catalog
        .and_then(|catalog| catalog.get("tiers"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        tier.as_object()?;
        inspect(tier, &mut has_cost)?;
    }
    Some(has_cost)
}

fn scenarios(
    base: BillingUsageInput,
    catalog: Option<&serde_json::Value>,
) -> Vec<BillingUsageInput> {
    let mut result = vec![base.clone()];
    if base.input_tokens == 0 {
        return result;
    }
    // Cover all cache classifications using the same settlement calculator.
    // For formats with exclusive input counts this deliberately overestimates.
    let mut creation = base.clone();
    creation.cache_creation_tokens = base.input_tokens;
    result.push(creation.clone());
    let mut creation_5m = creation.clone();
    creation_5m.cache_creation_ephemeral_5m_tokens = base.input_tokens;
    creation_5m.cache_ttl_minutes = Some(5);
    result.push(creation_5m);
    creation.cache_creation_ephemeral_1h_tokens = base.input_tokens;
    creation.cache_ttl_minutes = Some(60);
    result.push(creation);
    let mut ttls = std::collections::BTreeSet::from([5, 60]);
    for ttl in catalog
        .and_then(|catalog| catalog.get("tiers"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tier| {
            tier.get("cache_ttl_pricing")
                .and_then(serde_json::Value::as_array)
        })
        .flatten()
        .filter_map(|price| price.get("ttl_minutes").and_then(serde_json::Value::as_i64))
        .filter(|ttl| *ttl >= 0)
    {
        ttls.insert(ttl);
    }
    let mut read = base.clone();
    read.cache_read_tokens = read.input_tokens;
    result.push(read.clone());
    for ttl in ttls {
        let mut ttl_creation = base.clone();
        ttl_creation.cache_creation_tokens = base.input_tokens;
        ttl_creation.cache_ttl_minutes = Some(ttl);
        result.push(ttl_creation);
        let mut ttl_read = read.clone();
        ttl_read.cache_ttl_minutes = Some(ttl);
        result.push(ttl_read);
    }
    result
}

impl BillingService {
    /// Returns None when any reachable price or token bound is unproven. This
    /// method performs no admission or wallet mutation.
    pub fn quote_image_authorization(
        &self,
        pricing: &BillingModelPricingSnapshot,
        input: &BillingImageAuthorizationInput,
    ) -> Result<Option<BillingImageAuthorizationQuote>, ExpressionEvaluationError> {
        if !valid_input(input) {
            return Ok(None);
        }
        let mut input = input.clone();
        let Some(outputs) = input
            .possible_outputs
            .iter()
            .map(BillingImageOutputDimensions::normalized)
            .collect::<Option<Vec<_>>>()
        else {
            return Ok(None);
        };
        if outputs.iter().any(|output| {
            !requested_dimension_matches(input.size.as_deref(), &output.size, true)
                || !requested_dimension_matches(input.quality.as_deref(), &output.quality, false)
        }) {
            return Ok(None);
        }
        input.possible_outputs = outputs;
        let Some(catalogs) = pricing
            .resolve_authorization_pricing_candidates(input.requested_processing_tier.as_deref())
            .map_err(|error| ExpressionEvaluationError::Failed(error.to_string()))?
        else {
            return Ok(None);
        };
        let mut upper_bound_units = 0;
        if !pricing.is_free_tier() && input.api_key_multiplier != 0.0 {
            for catalog in catalogs {
                let Some(has_token_cost) = token_catalog_has_cost(catalog.tiered_pricing.as_ref())
                else {
                    return Ok(None);
                };
                if (has_token_cost && input.token_bounds.is_none())
                    || catalog
                        .price_per_request
                        .is_some_and(|price| !price.is_finite() || price < 0.0)
                {
                    return Ok(None);
                }
                let has_image_catalog =
                    image_output_pricing_state(catalog.tiered_pricing.as_ref()).enabled;
                if !(has_image_catalog || has_token_cost && input.token_bounds.is_some()) {
                    return Ok(None);
                }
                for output in &input.possible_outputs {
                    let base = BillingUsageInput {
                        image_count: i64::from(input.image_count),
                        request_count: i64::from(input.image_count),
                        image_size: Some(output.size.clone()),
                        image_quality: Some(output.quality.clone()),
                        image_output_format: input.output_format.clone(),
                        api_format: input.api_format.clone(),
                        requested_processing_tier: input.requested_processing_tier.clone(),
                        input_tokens: input
                            .token_bounds
                            .as_ref()
                            .map_or(0, |bounds| bounds.max_total_input_tokens),
                        output_tokens: input
                            .token_bounds
                            .as_ref()
                            .map_or(0, |bounds| bounds.max_output_tokens),
                        ..BillingUsageInput::new("image")
                    };
                    if has_image_catalog {
                        let resolved = resolve_image_output_price_resolution(
                            catalog.tiered_pricing.as_ref(),
                            &base,
                        );
                        if resolved.pricing_mode == "none"
                            || !resolved.price_per_image.is_finite()
                            || resolved.price_per_image < 0.0
                        {
                            return Ok(None);
                        }
                    }
                    for scenario in scenarios(base, catalog.tiered_pricing.as_ref()) {
                        let context = normalize_total_input_context_for_cache_hit_rate(
                            scenario.api_format.as_deref(),
                            scenario.input_tokens,
                            scenario.cache_creation_tokens,
                            scenario.cache_read_tokens,
                        );
                        let selected =
                            self.calculate_with_resolution(pricing, &scenario, catalog.clone())?;
                        if !billing_computation_is_bounded(&selected) {
                            return Ok(None);
                        }
                        let Some(candidates) = authorization_pricing_candidates(&catalog, context)
                        else {
                            return Ok(None);
                        };
                        for candidate in candidates {
                            let calculated =
                                self.calculate_with_resolution(pricing, &scenario, candidate)?;
                            if !billing_computation_is_bounded(&calculated) {
                                return Ok(None);
                            }
                            let Some(units) = ceil_cost_units(
                                calculated.cost_before_final_rounding(input.api_key_multiplier),
                            ) else {
                                return Ok(None);
                            };
                            upper_bound_units = upper_bound_units.max(units);
                        }
                    }
                }
            }
        }
        Ok(Some(BillingImageAuthorizationQuote {
            schema_version: 1,
            pricing: pricing.clone(),
            input,
            upper_bound_units,
        }))
    }

    /// Calculate from actual completed-output evidence and the frozen quote.
    /// None means missing/unpriced evidence: retain the hold for reconciliation.
    /// A priced overrun preserves actual cost while capping collectible funds.
    pub fn calculate_image_with_quote(
        &self,
        quote: &BillingImageAuthorizationQuote,
        usage: &BillingUsageInput,
    ) -> Result<Option<BillingImageQuotedCalculation>, ExpressionEvaluationError> {
        if quote.schema_version != 1
            || quote.upper_bound_units < 0
            || usage.task_type != "image"
            || usage.image_count < 0
            || usage.request_count < 0
            || usage.api_format != quote.input.api_format
            || usage.requested_processing_tier != quote.input.requested_processing_tier
            || usage.input_tokens < 0
            || usage.output_tokens < 0
            || usage.cache_creation_tokens < 0
            || usage.cache_read_tokens < 0
            || usage.cache_creation_ephemeral_5m_tokens < 0
            || usage.cache_creation_ephemeral_1h_tokens < 0
            || usage
                .cache_creation_ephemeral_5m_tokens
                .checked_add(usage.cache_creation_ephemeral_1h_tokens)
                .is_none_or(|classified| classified > usage.cache_creation_tokens)
        {
            return Ok(None);
        }
        let actual_output = usage
            .image_size
            .as_ref()
            .zip(usage.image_quality.as_ref())
            .and_then(|(size, quality)| {
                BillingImageOutputDimensions {
                    size: size.clone(),
                    quality: quality.clone(),
                }
                .normalized()
            });
        if usage.image_count > 0 && actual_output.is_none() {
            return Ok(None);
        }
        let actual_format = usage.image_output_format.as_deref().map(|format| {
            match format.trim().to_ascii_lowercase().as_str() {
                "jpg" | "jpeg" => "jpeg",
                "png" => "png",
                "webp" => "webp",
                _ => "",
            }
        });
        if usage.image_count > 0
            && (actual_format == Some("")
                || quote.input.output_format.is_some() && actual_format.is_none())
        {
            return Ok(None);
        }
        let calculation = self.calculate(&quote.pricing, usage)?;
        if !billing_computation_is_bounded(&calculation) {
            return Ok(None);
        }
        let Some(calculated_units) = settled_cost_units(
            calculation.cost_before_final_rounding(quote.input.api_key_multiplier),
        ) else {
            return Ok(None);
        };
        let context = normalize_total_input_context_for_cache_hit_rate(
            usage.api_format.as_deref(),
            usage.input_tokens,
            usage.cache_creation_tokens,
            usage.cache_read_tokens,
        );
        let tokens_exceeded = quote.input.token_bounds.as_ref().is_some_and(|bounds| {
            context > bounds.max_total_input_tokens
                || usage.output_tokens > bounds.max_output_tokens
        });
        let facts_exceeded = usage.image_count > i64::from(quote.input.image_count)
            || usage.request_count > i64::from(quote.input.image_count)
            || tokens_exceeded
            || (usage.image_count > 0
                && quote
                    .input
                    .output_format
                    .as_deref()
                    .is_some_and(|expected| actual_format != Some(expected)))
            || (usage.image_count > 0
                && !quote
                    .input
                    .possible_outputs
                    .iter()
                    .any(|output| Some(output) == actual_output.as_ref()));
        let collectible_units = calculated_units.min(quote.upper_bound_units);
        Ok(Some(BillingImageQuotedCalculation {
            computation: calculation,
            calculated_units,
            collectible_units,
            excess_units: calculated_units - collectible_units,
            requires_reconciliation: facts_exceeded || calculated_units > quote.upper_bound_units,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pricing(catalog: serde_json::Value) -> BillingModelPricingSnapshot {
        BillingModelPricingSnapshot {
            provider_id: "provider".into(),
            provider_billing_type: Some("pay_as_you_go".into()),
            provider_api_key_id: Some("provider-key".into()),
            provider_api_key_rate_multipliers: None,
            provider_api_key_cache_ttl_minutes: None,
            global_model_id: "image-model".into(),
            global_model_name: "image-model".into(),
            global_model_config: None,
            default_price_per_request: Some(0.01),
            default_tiered_pricing: Some(catalog),
            model_id: None,
            model_provider_model_name: None,
            model_config: None,
            model_price_per_request: None,
            model_tiered_pricing: None,
        }
    }

    fn input() -> BillingImageAuthorizationInput {
        BillingImageAuthorizationInput {
            image_count: 3,
            max_image_count: 10,
            operation: "generate".into(),
            size: Some("1024x1024".into()),
            quality: Some("medium".into()),
            output_format: Some("png".into()),
            partial_images: 0,
            possible_outputs: vec![BillingImageOutputDimensions {
                size: "1024x1024".into(),
                quality: "medium".into(),
            }],
            api_format: Some("openai:image".into()),
            requested_processing_tier: None,
            api_key_multiplier: 1.0,
            token_bounds: None,
        }
    }

    fn usage() -> BillingUsageInput {
        BillingUsageInput {
            image_count: 1,
            request_count: 1,
            image_size: Some("1024x1024".into()),
            image_quality: Some("medium".into()),
            image_output_format: Some("png".into()),
            api_format: Some("openai:image".into()),
            ..BillingUsageInput::new("image")
        }
    }

    #[test]
    fn quote_counts_all_images_request_fees_and_both_multipliers() {
        let service = BillingService::new();
        let mut pricing = pricing(json!({"image_output_price_default": 0.05}));
        pricing.provider_api_key_rate_multipliers = Some(json!({"openai:image": 2.0}));
        let mut input = input();
        input.api_key_multiplier = 1.5;
        for count in [1, 10] {
            input.image_count = count;
            let quote = service
                .quote_image_authorization(&pricing, &input)
                .unwrap()
                .unwrap();
            // Each image incurs both 0.05 output and 0.01 request charges.
            assert_eq!(quote.upper_bound_units(), i64::from(count) * 18_000_000);
        }
        input.image_count = 2;
        input.max_image_count = 1;
        assert!(service
            .quote_image_authorization(&pricing, &input)
            .unwrap()
            .is_none());
    }

    #[test]
    fn quote_and_frozen_cost_match_the_single_combined_multiplier_rounding() {
        let service = BillingService::new();
        for (price, expected_units) in [(0.050_000_04, 5_000_004), (0.000_000_01, 1)] {
            let mut pricing = pricing(json!({"image_output_price_default": price}));
            pricing.default_price_per_request = Some(0.0);
            pricing.provider_api_key_rate_multipliers = Some(json!({"openai:image": 0.1}));
            let mut input = input();
            input.image_count = 1;
            input.api_key_multiplier = 10.0;
            let quote = service
                .quote_image_authorization(&pricing, &input)
                .unwrap()
                .unwrap();
            let actual = service
                .calculate_image_with_quote(&quote, &usage())
                .unwrap()
                .unwrap();
            assert_eq!(actual.calculated_units, expected_units);
            assert!(quote.upper_bound_units() >= expected_units);
            assert_eq!(actual.collectible_units, expected_units);
            assert!(!actual.requires_reconciliation);
        }
    }

    #[test]
    fn auto_quote_requires_complete_coverage_and_uses_most_expensive_outcome() {
        let service = BillingService::new();
        let mut input = input();
        input.size = Some("auto".into());
        input.quality = None;
        input.possible_outputs.push(BillingImageOutputDimensions {
            size: "1536 × 1024".into(),
            quality: "high".into(),
        });
        let complete = pricing(json!({"image_output_prices": {
            "1024x1024": {"medium": 0.02}, "1536x1024": {"high": 0.08}
        }}));
        let quote = service
            .quote_image_authorization(&complete, &input)
            .unwrap()
            .unwrap();
        assert_eq!(quote.upper_bound_units(), 27_000_000);
        let missing = pricing(json!({"image_output_prices": {"1024x1024": {"medium": 0.02}}}));
        assert!(service
            .quote_image_authorization(&missing, &input)
            .unwrap()
            .is_none());
        input.possible_outputs.clear();
        assert!(service
            .quote_image_authorization(&complete, &input)
            .unwrap()
            .is_none());
    }

    #[test]
    fn quote_and_settlement_share_matrix_then_range_then_explicit_default() {
        let service = BillingService::new();
        let pricing = pricing(json!({
            "image_output_prices": {"1024x1024": {"medium": 0.04}},
            "image_output_price_ranges": [{"up_to_pixels": 2_000_000, "prices": {"medium": 0.07}}],
            "image_output_price_default": 0.11
        }));
        for (size, expected) in [
            ("1024x1024", 5_000_000),
            ("1536x1024", 8_000_000),
            ("2048x2048", 12_000_000),
        ] {
            let mut input = input();
            input.image_count = 1;
            input.size = Some(size.into());
            input.possible_outputs[0].size = size.into();
            let quote = service
                .quote_image_authorization(&pricing, &input)
                .unwrap()
                .unwrap();
            assert_eq!(quote.upper_bound_units(), expected);
            let mut actual = usage();
            actual.image_size = Some(size.into());
            let settled = service
                .calculate_image_with_quote(&quote, &actual)
                .unwrap()
                .unwrap();
            assert_eq!(settled.collectible_units, expected);
            assert!(!settled.requires_reconciliation);
        }
    }

    #[test]
    fn unmatched_catalog_is_not_a_complete_zero_cost_settlement() {
        let service = BillingService::new();
        let pricing = pricing(json!({"image_output_prices": {"2048x2048": {"high": 0.3}}}));
        assert!(service
            .quote_image_authorization(&pricing, &input())
            .unwrap()
            .is_none());
        let actual = service.calculate(&pricing, &usage()).unwrap();
        assert_eq!(
            actual.cost_result.status,
            crate::BillingSnapshotStatus::NoRule
        );
        assert_eq!(
            actual.cost_result.snapshot.missing_required,
            ["image_output_pricing"]
        );
    }

    #[test]
    fn token_pricing_needs_a_proven_bound_and_includes_cache_and_output() {
        let service = BillingService::new();
        let pricing = pricing(json!({
            "image_output_price_default": 0.05,
            "tiers": [{"up_to": null, "input_price_per_1m": 1.0,
                "output_price_per_1m": 2.0, "cache_creation_price_per_1m": 3.0,
                "cache_read_price_per_1m": 0.1}]
        }));
        let mut input = input();
        assert!(service
            .quote_image_authorization(&pricing, &input)
            .unwrap()
            .is_none());
        input.token_bounds = Some(BillingImageTokenBounds {
            max_total_input_tokens: 1000,
            max_output_tokens: 2000,
        });
        let quote = service
            .quote_image_authorization(&pricing, &input)
            .unwrap()
            .unwrap();
        assert!(quote.upper_bound_units() >= 18_700_000);
        for output_tokens in [0, 500, 2000] {
            let mut actual = usage();
            actual.input_tokens = 800;
            actual.output_tokens = output_tokens;
            let settled = service
                .calculate_image_with_quote(&quote, &actual)
                .unwrap()
                .unwrap();
            assert!(settled.calculated_units <= quote.upper_bound_units());
            assert!(!settled.requires_reconciliation);
        }
    }

    #[test]
    fn inconsistent_cache_subtotals_keep_the_hold_for_reconciliation() {
        let service = BillingService::new();
        let pricing = pricing(json!({
            "image_output_price_default": 0.05,
            "tiers": [{"up_to": null, "input_price_per_1m": 1.0,
                "output_price_per_1m": 0.0, "cache_creation_price_per_1m": 3.0}]
        }));
        let mut input = input();
        input.token_bounds = Some(BillingImageTokenBounds {
            max_total_input_tokens: 1000,
            max_output_tokens: 0,
        });
        let quote = service
            .quote_image_authorization(&pricing, &input)
            .unwrap()
            .unwrap();
        for (aggregate, five_minute, hour) in
            [(0, 2000, 0), (1000, 600, 500), (i64::MAX, i64::MAX, 1)]
        {
            let mut actual = usage();
            actual.cache_creation_tokens = aggregate;
            actual.cache_creation_ephemeral_5m_tokens = five_minute;
            actual.cache_creation_ephemeral_1h_tokens = hour;
            assert!(service
                .calculate_image_with_quote(&quote, &actual)
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn output_format_changes_are_audited_even_when_the_cost_does_not_change() {
        let service = BillingService::new();
        let quote = service
            .quote_image_authorization(
                &pricing(json!({"image_output_price_default": 0.05})),
                &input(),
            )
            .unwrap()
            .unwrap();
        let mut actual = usage();
        actual.image_output_format = Some("jpeg".into());
        let changed = service
            .calculate_image_with_quote(&quote, &actual)
            .unwrap()
            .unwrap();
        assert_eq!(changed.excess_units, 0);
        assert!(changed.requires_reconciliation);
        actual.image_output_format = Some(" PNG ".into());
        assert!(
            !service
                .calculate_image_with_quote(&quote, &actual)
                .unwrap()
                .unwrap()
                .requires_reconciliation
        );
        for format in [None, Some("unknown".into())] {
            actual.image_output_format = format;
            assert!(service
                .calculate_image_with_quote(&quote, &actual)
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn frozen_quote_survives_price_changes_and_charges_only_actual_output() {
        let service = BillingService::new();
        let mut pricing = pricing(json!({"image_output_price_default": 0.05}));
        let quote = service
            .quote_image_authorization(&pricing, &input())
            .unwrap()
            .unwrap();
        pricing.default_tiered_pricing = Some(json!({"image_output_price_default": 10.0}));
        let roundtrip: BillingImageAuthorizationQuote =
            serde_json::from_value(serde_json::to_value(&quote).unwrap()).unwrap();
        let actual = service
            .calculate_image_with_quote(&roundtrip, &usage())
            .unwrap()
            .unwrap();
        assert_eq!(actual.calculated_units, 6_000_000);
        assert_eq!(actual.collectible_units, 6_000_000);
        assert_eq!(quote.upper_bound_units(), 18_000_000);
        assert!(!actual.requires_reconciliation);
        assert!(
            service
                .calculate(&pricing, &usage())
                .unwrap()
                .actual_total_cost
                > 10.0
        );
    }

    #[test]
    fn cache_ttl_only_prices_require_bounds_and_all_catalog_ttls_are_covered() {
        let service = BillingService::new();
        let pricing = pricing(json!({
            "image_output_price_default": 0.05,
            "tiers": [{"up_to": null, "input_price_per_1m": 0.0,
                "output_price_per_1m": 0.0, "cache_creation_price_per_1m": null,
                "cache_read_price_per_1m": 0.0,
                "cache_ttl_pricing": [{"ttl_minutes": 120,
                    "cache_creation_price_per_1m": 100.0, "cache_read_price_per_1m": 200.0}]}]
        }));
        let mut input = input();
        assert!(service
            .quote_image_authorization(&pricing, &input)
            .unwrap()
            .is_none());
        input.token_bounds = Some(BillingImageTokenBounds {
            max_total_input_tokens: 1000,
            max_output_tokens: 0,
        });
        let quote = service
            .quote_image_authorization(&pricing, &input)
            .unwrap()
            .unwrap();
        let mut actual = usage();
        actual.cache_ttl_minutes = Some(120);
        actual.cache_read_tokens = 1000;
        let result = service
            .calculate_image_with_quote(&quote, &actual)
            .unwrap()
            .unwrap();
        assert_eq!(result.calculated_units, 26_000_000);
        assert!(quote.upper_bound_units() >= result.calculated_units);
        assert!(!result.requires_reconciliation);
    }

    #[test]
    fn processing_catalog_multiplier_and_nonmonotonic_token_tiers_are_bounded() {
        let service = BillingService::new();
        let pricing = pricing(json!({
            "image_output_price_default": 0.05,
            "tiers": [
                {"up_to": 1000, "input_price_per_1m": 100.0, "output_price_per_1m": 0.0},
                {"up_to": null, "input_price_per_1m": 1.0, "output_price_per_1m": 0.0}
            ],
            "processing_tiers": {"flex": {"price_multiplier": 2.0}}
        }));
        let mut input = input();
        input.requested_processing_tier = Some("flex".into());
        input.token_bounds = Some(BillingImageTokenBounds {
            max_total_input_tokens: 2000,
            max_output_tokens: 0,
        });
        let quote = service
            .quote_image_authorization(&pricing, &input)
            .unwrap()
            .unwrap();
        for tokens in [0, 500, 1000, 2000] {
            let mut actual = usage();
            actual.input_tokens = tokens;
            actual.requested_processing_tier = input.requested_processing_tier.clone();
            let result = service
                .calculate_image_with_quote(&quote, &actual)
                .unwrap()
                .unwrap();
            assert!(quote.upper_bound_units() >= result.calculated_units);
            assert!(!result.requires_reconciliation);
        }
    }

    #[test]
    fn priced_overrun_preserves_actual_cost_and_caps_collection() {
        let service = BillingService::new();
        let quote = service
            .quote_image_authorization(
                &pricing(json!({"image_output_price_default": 0.05})),
                &input(),
            )
            .unwrap()
            .unwrap();
        let mut actual = usage();
        actual.image_count = 4;
        actual.request_count = 4;
        let result = service
            .calculate_image_with_quote(&quote, &actual)
            .unwrap()
            .unwrap();
        assert_eq!(result.calculated_units, 24_000_000);
        assert_eq!(result.collectible_units, 18_000_000);
        assert_eq!(result.excess_units, 6_000_000);
        assert!(result.requires_reconciliation);
        actual.image_size = None;
        assert!(service
            .calculate_image_with_quote(&quote, &actual)
            .unwrap()
            .is_none());
    }

    #[test]
    fn quote_rejects_invalid_dimensions_counts_and_financial_values() {
        let service = BillingService::new();
        let pricing = pricing(json!({"image_output_price_default": 0.05}));
        for count in [0, 11, u32::MAX] {
            let mut input = input();
            input.image_count = count;
            assert!(service
                .quote_image_authorization(&pricing, &input)
                .unwrap()
                .is_none());
        }
        for multiplier in [f64::NAN, f64::INFINITY, -1.0, f64::MAX] {
            let mut input = input();
            input.api_key_multiplier = multiplier;
            assert!(service
                .quote_image_authorization(&pricing, &input)
                .unwrap()
                .is_none());
        }
        let mut bad = input();
        bad.possible_outputs[0].size = "9223372036854775807x2".into();
        assert!(service
            .quote_image_authorization(&pricing, &bad)
            .unwrap()
            .is_none());
        bad = input();
        bad.possible_outputs[0].quality = "auto".into();
        assert!(service
            .quote_image_authorization(&pricing, &bad)
            .unwrap()
            .is_none());
        bad = input();
        bad.possible_outputs[0].size = "2048x2048".into();
        assert!(service
            .quote_image_authorization(&pricing, &bad)
            .unwrap()
            .is_none());
    }

    #[test]
    fn explicit_zero_image_price_is_valid_and_legacy_paid_admission_stays_closed() {
        let service = BillingService::new();
        let mut pricing = pricing(json!({"image_output_price_default": 0.0}));
        pricing.default_price_per_request = Some(0.0);
        let quote = service
            .quote_image_authorization(&pricing, &input())
            .unwrap()
            .unwrap();
        assert_eq!(quote.upper_bound_units(), 0);
        assert!(service
            .estimate_authorization_cost_upper_bound(
                &pricing,
                &crate::BillingAuthorizationEstimateInput::new("image", 0)
            )
            .unwrap()
            .is_none());
        assert_eq!(ceil_cost_units(0.000_000_001), Some(1));
        assert_eq!(settled_cost_units(0.000_000_001), Some(0));
        assert_eq!(ceil_cost_units(i64::MAX as f64 / COST_UNITS_PER_USD), None);
    }

    #[test]
    fn explicit_free_tier_and_zero_key_multiplier_do_not_require_token_bounds() {
        let service = BillingService::new();
        let mut pricing = pricing(json!({"image_output_price_default": 0.05,
            "tiers": [{"up_to": null, "input_price_per_1m": 1.0, "output_price_per_1m": 2.0}]}));
        let mut input = input();
        pricing.provider_billing_type = Some("free_tier".into());
        let free = service
            .quote_image_authorization(&pricing, &input)
            .unwrap()
            .unwrap();
        assert_eq!(free.upper_bound_units(), 0);
        pricing.provider_billing_type = Some("pay_as_you_go".into());
        input.api_key_multiplier = 0.0;
        let zero = service
            .quote_image_authorization(&pricing, &input)
            .unwrap()
            .unwrap();
        assert_eq!(zero.upper_bound_units(), 0);
        let mut actual = usage();
        actual.input_tokens = 1000;
        let calculated = service
            .calculate_image_with_quote(&zero, &actual)
            .unwrap()
            .unwrap();
        assert_eq!(calculated.collectible_units, 0);
    }
}
