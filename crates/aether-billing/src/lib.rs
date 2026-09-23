//! Billing engine for Aether: pricing resolution, cost estimation, formula
//! evaluation, and billing snapshot schema.
#![warn(missing_docs)]

mod default_rule;
mod event_enrichment;
mod formula_engine;
mod models;
mod precision;
mod pricing;
mod provider_cost;
mod schema;
mod service;
mod token_normalization;

pub use aether_usage_runtime::{
    map_usage, map_usage_from_response, StandardizedUsage, UsageMapper,
};
pub use default_rule::{normalize_task_type, DefaultBillingRuleGenerator, VirtualBillingRule};
pub use event_enrichment::{enrich_usage_event_with_billing, BillingModelContextLookup};
pub use formula_engine::{
    extract_variable_names, is_formula_function_allowed, BillingIncompleteError,
    ExpressionEvaluationError, FormulaEngine, FormulaEvaluationResult, FormulaEvaluationStatus,
    UnsafeExpressionError, FORMULA_ALLOWED_FUNCTIONS,
};
pub use models::{BillingDimension, BillingUnit, CostBreakdown};
pub use precision::{
    quantize_cost, quantize_display, quantize_value, PrecisionError, BILLING_DISPLAY_PRECISION,
    BILLING_STORAGE_PRECISION,
};
pub use pricing::{
    BillingAuthorizationEstimateInput, BillingComputation, BillingModelPricingSnapshot,
    BillingPricingConfigurationError, BillingPricingResolution, BillingPricingSource,
    BillingUsageInput,
};
pub use provider_cost::{
    calculate_provider_cost_amount, estimate_attempt_provider_cost,
    estimate_provider_cost_component, estimate_provider_request_cost,
    provider_cost_token_quantities_from_standardized_usage, resolve_provider_cost_price,
    CostCertainty, ProviderCostDimension, ProviderCostEstimateComponent, ProviderCostEstimateError,
    ProviderCostEstimateInput, ProviderCostInputPriceMode, ProviderCostPrice,
    ProviderCostPriceError, ProviderCostRequestEstimate, ProviderCostSnapshot,
    ProviderCostTokenQuantities, ProviderCostUnit, PROVIDER_COST_SCALE,
};
pub use schema::{
    BillingSnapshot, BillingSnapshotStatus, CostResult, BILLING_SNAPSHOT_SCHEMA_VERSION,
};
pub use service::image_authorization::{
    BillingImageAuthorizationInput, BillingImageAuthorizationQuote, BillingImageOutputDimensions,
    BillingImageQuotedCalculation, BillingImageTokenBounds,
};
pub use service::BillingService;
pub use token_normalization::{
    normalize_input_tokens_for_billing,
    normalize_input_tokens_for_billing_with_known_cache_semantics,
    normalize_total_input_context_for_cache_hit_rate,
};
