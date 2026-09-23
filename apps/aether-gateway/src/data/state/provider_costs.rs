use aether_billing::{
    estimate_provider_request_cost, provider_cost_token_quantities_from_standardized_usage,
    resolve_provider_cost_price, ProviderCostEstimateInput, ProviderCostInputPriceMode,
    StandardizedUsage,
};
use aether_data::backend::ProviderCostDataState;
use aether_data::repository::provider_cost::{
    ProviderCostImportOutcome, ProviderCostListQuery, ProviderCostPrice,
    ProviderCostSnapshotImport, ProviderCostSummaryQuery, StoredProviderCostSnapshot,
    StoredProviderCostSummaryRow,
};
use aether_data::DataLayerError;
use aether_data_contracts::repository::billing::nonnegative_usd_to_usage_policy_cost_units;
use aether_data_contracts::repository::usage::{
    StoredRequestUsageAudit, USAGE_AVAILABLE_METADATA_KEY, USAGE_PRICING_AVAILABLE_METADATA_KEY,
};
use serde::Deserialize;
use serde_json::Value;

pub(crate) use aether_data::repository::provider_cost::{
    ProviderCostCertainty, ProviderCostDimension, ProviderCostReconciliationStatus,
    ProviderCostSourceKind, ProviderCostUnit,
};

use super::GatewayDataState;

pub(super) const PROVIDER_COST_SUPPLIER_BINDINGS_CONFIG_KEY: &str =
    "provider_cost_supplier_bindings";
const GATEWAY_AUTO_PROVIDER_COST_IMPORTER: &str = "gateway_auto_capture";

#[derive(Debug, Deserialize)]
pub(super) struct ProviderCostSupplierBinding {
    pub(super) supplier: String,
    pub(super) currency: String,
    pub(super) input_price_mode: ProviderCostInputPriceMode,
}

impl ProviderCostSupplierBinding {
    fn is_complete(&self) -> bool {
        !self.supplier.trim().is_empty() && !self.currency.trim().is_empty()
    }
}

impl GatewayDataState {
    fn provider_costs(&self) -> ProviderCostDataState<'_> {
        ProviderCostDataState::new(self.backends.as_ref())
    }

    pub(crate) fn has_provider_cost_data_backend(&self) -> bool {
        self.provider_costs().has_provider_cost_data_backend()
    }

    pub(crate) async fn import_provider_cost_price(
        &self,
        price: &ProviderCostPrice,
    ) -> Result<Option<ProviderCostImportOutcome<ProviderCostPrice>>, DataLayerError> {
        self.provider_costs().import_price(price).await
    }

    pub(crate) async fn list_provider_cost_prices(
        &self,
        query: &ProviderCostListQuery,
    ) -> Result<Option<Vec<ProviderCostPrice>>, DataLayerError> {
        self.provider_costs().list_prices(query).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn find_effective_provider_cost_price(
        &self,
        supplier: &str,
        provider: &str,
        model: &str,
        dimension: ProviderCostDimension,
        currency: &str,
        unit: ProviderCostUnit,
        at_unix_secs: u64,
    ) -> Result<Option<ProviderCostPrice>, DataLayerError> {
        self.provider_costs()
            .find_effective_price(
                supplier,
                provider,
                model,
                dimension,
                currency,
                unit,
                at_unix_secs,
            )
            .await
    }

    pub(crate) async fn import_provider_cost_snapshot(
        &self,
        snapshot: &ProviderCostSnapshotImport,
    ) -> Result<Option<ProviderCostImportOutcome<StoredProviderCostSnapshot>>, DataLayerError> {
        self.provider_costs().import_snapshot(snapshot).await
    }

    async fn list_provider_cost_snapshots_for_request(
        &self,
        request_id: &str,
    ) -> Result<Option<Vec<StoredProviderCostSnapshot>>, DataLayerError> {
        self.provider_costs()
            .list_snapshots_for_request(request_id)
            .await
    }

    pub(crate) async fn summarize_provider_cost_snapshots(
        &self,
        query: &ProviderCostSummaryQuery,
    ) -> Result<Option<Vec<StoredProviderCostSummaryRow>>, DataLayerError> {
        self.provider_costs().summarize_snapshots(query).await
    }

    pub(crate) async fn capture_provider_cost_for_usage(
        &self,
        usage: &StoredRequestUsageAudit,
    ) -> Result<(), DataLayerError> {
        if !self.has_provider_cost_data_backend() {
            return Ok(());
        }

        let Some(provider_id) = non_empty(usage.provider_id.as_deref()) else {
            // A provider name is display data. Never use it as a supplier-price identity.
            return Ok(());
        };
        let Some(model) = non_empty(usage.target_model.as_deref())
            .or_else(|| non_empty(Some(usage.model.as_str())))
        else {
            return Ok(());
        };
        let Some(sales_amount_units) =
            nonnegative_usd_to_usage_policy_cost_units(usage.actual_total_cost_usd)
        else {
            return Ok(());
        };

        let Some(existing) = self
            .list_provider_cost_snapshots_for_request(&usage.request_id)
            .await?
        else {
            return Ok(());
        };
        if existing.iter().any(|snapshot| {
            snapshot.import.provider == provider_id
                && snapshot.import.model == model
                && snapshot.import.dimension == ProviderCostDimension::Request
        }) {
            return Ok(());
        }

        let binding_value = self
            .find_system_config_value(PROVIDER_COST_SUPPLIER_BINDINGS_CONFIG_KEY)
            .await?;
        let binding = provider_cost_supplier_binding(binding_value.as_ref(), provider_id);
        let occurred_at_unix_secs = usage.created_at_unix_ms / 1_000;
        let prices = match binding
            .as_ref()
            .filter(|_| !usage_is_image(usage) && provider_cost_usage_is_trustworthy(usage))
        {
            Some(binding) => match stored_usage_token_quantities(usage, binding.input_price_mode) {
                Some(quantities) => {
                    let mut prices = Vec::with_capacity(4);
                    for dimension in provider_cost_price_dimensions(&quantities) {
                        let Some(price) = self
                            .find_effective_provider_cost_price(
                                &binding.supplier,
                                provider_id,
                                model,
                                dimension,
                                &binding.currency,
                                ProviderCostUnit::PerMillionTokens,
                                occurred_at_unix_secs,
                            )
                            .await?
                        else {
                            break;
                        };
                        prices.push(price);
                    }
                    prices
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        let snapshot = build_gateway_auto_provider_cost_snapshot(
            usage,
            provider_id,
            model,
            sales_amount_units,
            binding.as_ref(),
            &prices,
        );
        self.import_provider_cost_snapshot(&snapshot).await?;
        Ok(())
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

pub(super) fn provider_cost_supplier_binding(
    value: Option<&Value>,
    provider_id: &str,
) -> Option<ProviderCostSupplierBinding> {
    let value = value
        .and_then(Value::as_object)
        .and_then(|bindings| bindings.get(provider_id))?;
    let binding = serde_json::from_value::<ProviderCostSupplierBinding>(value.clone()).ok()?;
    binding.is_complete().then_some(binding)
}

fn provider_cost_usage_is_trustworthy(usage: &StoredRequestUsageAudit) -> bool {
    let metadata = usage.request_metadata.as_ref().and_then(Value::as_object);
    metadata
        .and_then(|metadata| metadata.get(USAGE_AVAILABLE_METADATA_KEY))
        .and_then(Value::as_bool)
        == Some(true)
        && metadata
            .and_then(|metadata| metadata.get(USAGE_PRICING_AVAILABLE_METADATA_KEY))
            .and_then(Value::as_bool)
            != Some(false)
}

fn stored_usage_token_quantities(
    usage: &StoredRequestUsageAudit,
    input_price_mode: ProviderCostInputPriceMode,
) -> Option<aether_billing::ProviderCostTokenQuantities> {
    let mut standardized = StandardizedUsage::new();
    standardized.input_tokens = i64::try_from(usage.input_tokens).ok()?;
    standardized.output_tokens = i64::try_from(usage.output_tokens).ok()?;
    standardized.cache_creation_tokens = i64::try_from(usage.cache_creation_input_tokens).ok()?;
    standardized.cache_creation_ephemeral_5m_tokens =
        i64::try_from(usage.cache_creation_ephemeral_5m_input_tokens).ok()?;
    standardized.cache_creation_ephemeral_1h_tokens =
        i64::try_from(usage.cache_creation_ephemeral_1h_input_tokens).ok()?;
    standardized.cache_read_tokens = i64::try_from(usage.cache_read_input_tokens).ok()?;
    provider_cost_token_quantities_from_standardized_usage(
        usage
            .endpoint_api_format
            .as_deref()
            .or(usage.api_format.as_deref()),
        &standardized,
        input_price_mode,
    )
}

fn provider_cost_price_dimensions(
    quantities: &aether_billing::ProviderCostTokenQuantities,
) -> Vec<ProviderCostDimension> {
    let mut dimensions = vec![ProviderCostDimension::Input, ProviderCostDimension::Output];
    if quantities.cache_write > 0 {
        dimensions.push(ProviderCostDimension::CacheWrite);
    }
    if quantities.cache_read > 0 {
        dimensions.push(ProviderCostDimension::CacheRead);
    }
    dimensions
}

fn usage_is_image(usage: &StoredRequestUsageAudit) -> bool {
    [
        usage.request_type.as_deref(),
        usage.endpoint_kind.as_deref(),
        usage.endpoint_api_format.as_deref(),
        usage.api_format.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| {
        let value = value.trim();
        value.eq_ignore_ascii_case("image")
            || value.eq_ignore_ascii_case("images")
            || value.rsplit_once(':').is_some_and(|(_, kind)| {
                kind.eq_ignore_ascii_case("image") || kind.eq_ignore_ascii_case("images")
            })
    })
}

fn build_gateway_auto_provider_cost_snapshot(
    usage: &StoredRequestUsageAudit,
    provider_id: &str,
    model: &str,
    sales_amount_units: u64,
    binding: Option<&ProviderCostSupplierBinding>,
    prices: &[ProviderCostPrice],
) -> ProviderCostSnapshotImport {
    let occurred_at_unix_secs = usage.created_at_unix_ms / 1_000;
    let mut snapshot = ProviderCostSnapshotImport {
        import_id: format!("gateway-auto-provider-cost/{}", usage.request_id),
        request_id: usage.request_id.clone(),
        provider: provider_id.to_string(),
        model: model.to_string(),
        dimension: ProviderCostDimension::Request,
        sales_amount_units,
        sales_currency: "USD".to_string(),
        provider_cost_amount_units: None,
        provider_currency: None,
        certainty: ProviderCostCertainty::Unknown,
        source_kind: ProviderCostSourceKind::Estimate,
        reconciliation_status: ProviderCostReconciliationStatus::Unreconciled,
        price_import_id: None,
        price_version: None,
        source_reference: Some("gateway_auto_capture:unavailable".to_string()),
        price_components: Vec::new(),
        occurred_at_unix_secs,
        imported_by: GATEWAY_AUTO_PROVIDER_COST_IMPORTER.to_string(),
    };

    let Some(binding) = binding else {
        snapshot.source_reference =
            Some("gateway_auto_capture:missing_supplier_binding".to_string());
        return snapshot;
    };
    if !provider_cost_usage_is_trustworthy(usage) {
        snapshot.source_reference = Some("gateway_auto_capture:untrusted_token_usage".to_string());
        return snapshot;
    }
    if usage_is_image(usage) {
        snapshot.source_reference =
            Some("gateway_auto_capture:unsupported_image_usage".to_string());
        return snapshot;
    }
    let Some(quantities) = stored_usage_token_quantities(usage, binding.input_price_mode) else {
        snapshot.source_reference =
            Some("gateway_auto_capture:unsupported_token_usage".to_string());
        return snapshot;
    };

    let mut inputs = Vec::with_capacity(5);
    for (dimension, quantity) in [
        (ProviderCostDimension::Input, quantities.input),
        (ProviderCostDimension::Output, quantities.output),
    ] {
        let Ok(price) = resolve_provider_cost_price(
            prices,
            &binding.supplier,
            provider_id,
            model,
            dimension,
            &binding.currency,
            ProviderCostUnit::PerMillionTokens,
            occurred_at_unix_secs,
        ) else {
            snapshot.source_reference =
                Some("gateway_auto_capture:missing_token_price".to_string());
            return snapshot;
        };
        inputs.push(ProviderCostEstimateInput {
            price: Some(price.clone()),
            quantity: Some(quantity),
        });
    }
    for (dimension, quantity) in [
        (ProviderCostDimension::CacheWrite, quantities.cache_write),
        (ProviderCostDimension::CacheRead, quantities.cache_read),
    ] {
        if quantity == 0 {
            continue;
        }
        let Ok(price) = resolve_provider_cost_price(
            prices,
            &binding.supplier,
            provider_id,
            model,
            dimension,
            &binding.currency,
            ProviderCostUnit::PerMillionTokens,
            occurred_at_unix_secs,
        ) else {
            snapshot.source_reference =
                Some("gateway_auto_capture:missing_token_price".to_string());
            return snapshot;
        };
        inputs.push(ProviderCostEstimateInput {
            price: Some(price.clone()),
            quantity: Some(quantity),
        });
    }

    let Ok(Some(estimate)) = estimate_provider_request_cost(&inputs) else {
        snapshot.source_reference = Some("gateway_auto_capture:unsupported_price_book".to_string());
        return snapshot;
    };
    snapshot.provider_cost_amount_units = Some(estimate.amount_units);
    snapshot.provider_currency = Some(estimate.currency);
    snapshot.certainty = ProviderCostCertainty::Estimated;
    snapshot.source_reference = Some(format!("gateway_auto_estimate:{}", binding.supplier.trim()));
    snapshot.price_components = estimate
        .components
        .into_iter()
        .map(
            |component| aether_data::repository::provider_cost::ProviderCostPriceComponent {
                dimension: component.dimension,
                quantity: component.quantity,
                unit: component.unit,
                price_import_id: component.price_import_id,
                price_version: component.price_version,
                price_source_reference: component.price_source_reference,
                amount_units: component.amount_units,
            },
        )
        .collect();
    snapshot
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn usage() -> StoredRequestUsageAudit {
        StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            "request-1".to_string(),
            Some("user-1".to_string()),
            Some("key-1".to_string()),
            None,
            None,
            "display-only-provider-name".to_string(),
            "customer-model".to_string(),
            Some("supplier-model".to_string()),
            Some("provider-real-id".to_string()),
            None,
            None,
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            false,
            false,
            120,
            40,
            160,
            0.24,
            0.36,
            Some(200),
            None,
            None,
            Some(50),
            Some(10),
            "completed".to_string(),
            "settled".to_string(),
            100_000,
            101,
            Some(102),
        )
        .expect("usage should build")
        .with_cache_input_tokens(20, 30)
    }

    fn price(
        dimension: ProviderCostDimension,
        import_id: &str,
        price_units: u64,
    ) -> ProviderCostPrice {
        ProviderCostPrice {
            import_id: import_id.to_string(),
            supplier: "supplier-a".to_string(),
            provider: "provider-real-id".to_string(),
            model: "supplier-model".to_string(),
            dimension,
            currency: "USD".to_string(),
            unit: ProviderCostUnit::PerMillionTokens,
            version: "2026-09".to_string(),
            price_units,
            effective_from_unix_secs: 1,
            effective_to_unix_secs: None,
            source_reference: format!("price-book-{import_id}"),
            imported_by: "admin-1".to_string(),
        }
    }

    fn binding() -> ProviderCostSupplierBinding {
        ProviderCostSupplierBinding {
            supplier: "supplier-a".to_string(),
            currency: "USD".to_string(),
            input_price_mode: ProviderCostInputPriceMode::ExclusiveOfCache,
        }
    }

    #[test]
    fn automatic_snapshot_uses_provider_id_and_aggregates_token_components_once() {
        let mut usage = usage();
        usage.request_metadata = Some(json!({
            "usage_available": true
        }));
        let prices = vec![
            price(ProviderCostDimension::Input, "input", 100_000_000),
            price(ProviderCostDimension::Output, "output", 200_000_000),
            price(ProviderCostDimension::CacheWrite, "cache-write", 50_000_000),
            price(ProviderCostDimension::CacheRead, "cache-read", 25_000_000),
        ];

        let snapshot = build_gateway_auto_provider_cost_snapshot(
            &usage,
            "provider-real-id",
            "supplier-model",
            36_000_000,
            Some(&binding()),
            &prices,
        );

        assert_eq!(snapshot.dimension, ProviderCostDimension::Request);
        assert_eq!(snapshot.provider, "provider-real-id");
        assert_ne!(snapshot.provider, usage.provider_name);
        assert_eq!(snapshot.certainty, ProviderCostCertainty::Estimated);
        assert_eq!(snapshot.source_kind, ProviderCostSourceKind::Estimate);
        assert_eq!(snapshot.sales_amount_units, 36_000_000);
        assert_eq!(snapshot.price_components.len(), 4);
        assert_eq!(snapshot.price_components[0].quantity, 70);
        assert_eq!(snapshot.price_components[1].quantity, 40);
        assert_eq!(snapshot.price_components[2].quantity, 20);
        assert_eq!(snapshot.price_components[3].quantity, 30);
        assert_eq!(snapshot.provider_cost_amount_units, Some(16_750));
    }

    #[test]
    fn explicit_zero_token_usage_is_estimated_without_treating_missing_usage_as_zero() {
        let mut usage = usage();
        usage.input_tokens = 0;
        usage.output_tokens = 0;
        usage.total_tokens = 0;
        usage.cache_creation_input_tokens = 0;
        usage.cache_read_input_tokens = 0;
        let prices = vec![
            price(ProviderCostDimension::Input, "input", 100_000_000),
            price(ProviderCostDimension::Output, "output", 200_000_000),
        ];

        usage.request_metadata = Some(json!({
            "usage_available": true
        }));
        let zero = build_gateway_auto_provider_cost_snapshot(
            &usage,
            "provider-real-id",
            "supplier-model",
            0,
            Some(&binding()),
            &prices,
        );
        assert_eq!(zero.certainty, ProviderCostCertainty::Estimated);
        assert_eq!(zero.provider_cost_amount_units, Some(0));

        usage.request_metadata = None;
        let missing = build_gateway_auto_provider_cost_snapshot(
            &usage,
            "provider-real-id",
            "supplier-model",
            0,
            Some(&binding()),
            &prices,
        );
        assert_eq!(missing.certainty, ProviderCostCertainty::Unknown);
        assert_eq!(missing.provider_cost_amount_units, None);
    }

    #[test]
    fn supplier_binding_requires_the_exact_provider_id() {
        let binding = provider_cost_supplier_binding(
            Some(&json!({
                "provider-real-id": {
                    "supplier": "supplier-a",
                    "currency": "USD",
                    "input_price_mode": "exclusive_of_cache"
                }
            })),
            "display-only-provider-name",
        );
        assert!(binding.is_none());
    }

    #[test]
    fn image_usage_remains_unknown_until_per_image_capture_is_connected() {
        let mut usage = usage();
        usage.request_type = Some("image".to_string());
        usage.request_metadata = Some(json!({"usage_available": true}));

        let snapshot = build_gateway_auto_provider_cost_snapshot(
            &usage,
            "provider-real-id",
            "supplier-model",
            36_000_000,
            Some(&binding()),
            &[],
        );

        assert_eq!(snapshot.certainty, ProviderCostCertainty::Unknown);
        assert_eq!(
            snapshot.source_reference.as_deref(),
            Some("gateway_auto_capture:unsupported_image_usage")
        );
    }

    #[test]
    fn image_detection_accepts_contract_and_plural_endpoint_kinds() {
        let mut usage = usage();
        usage.endpoint_kind = Some("images".to_string());
        assert!(usage_is_image(&usage));

        usage.endpoint_kind = Some("chat".to_string());
        usage.endpoint_api_format = Some("openai:image".to_string());
        assert!(usage_is_image(&usage));
    }
}
