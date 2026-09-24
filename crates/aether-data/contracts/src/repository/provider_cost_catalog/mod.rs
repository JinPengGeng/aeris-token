mod types;

use async_trait::async_trait;

pub use types::{
    deserialize_optional_price, validate_provider_cost_catalog_tiered_pricing,
    ProviderCostCatalogDeleteOutcome, ProviderCostCatalogListQuery, ProviderCostCatalogRecord,
    ProviderCostCatalogUpsertOutcome, ProviderCostTaskType, PROVIDER_COST_CATALOG_MAX_CURRENCY_LEN,
    PROVIDER_COST_CATALOG_MAX_ID_LEN, PROVIDER_COST_CATALOG_MAX_LIST_LIMIT,
    PROVIDER_COST_CATALOG_MAX_MODEL_LEN, PROVIDER_COST_CATALOG_MAX_OPERATOR_LEN,
    PROVIDER_COST_CATALOG_MAX_TIERED_PRICING_BYTES,
};

/// Stores provider-side cost catalogs. The catalog JSON is isomorphic to the
/// sales-side `BillingModelPricingSnapshot` pricing catalog so cost and price
/// run through the same formula engine at consumption time.
///
/// Implementations must treat `effective_from_unix_secs` (inclusive) and
/// `effective_to_unix_secs` (exclusive) as the catalog validity window.
#[async_trait]
pub trait ProviderCostCatalogRepository: Send + Sync {
    async fn upsert_provider_cost_catalog(
        &self,
        record: ProviderCostCatalogRecord,
    ) -> Result<ProviderCostCatalogUpsertOutcome, crate::DataLayerError>;

    async fn get_provider_cost_catalog(
        &self,
        cost_id: &str,
    ) -> Result<Option<ProviderCostCatalogRecord>, crate::DataLayerError>;

    async fn list_provider_cost_catalogs(
        &self,
        query: &ProviderCostCatalogListQuery,
    ) -> Result<Vec<ProviderCostCatalogRecord>, crate::DataLayerError>;

    async fn find_effective_provider_cost_catalog(
        &self,
        provider_id: &str,
        model: &str,
        task_type: ProviderCostTaskType,
        at_unix_secs: u64,
    ) -> Result<Option<ProviderCostCatalogRecord>, crate::DataLayerError>;

    async fn delete_provider_cost_catalog(
        &self,
        cost_id: &str,
    ) -> Result<ProviderCostCatalogDeleteOutcome, crate::DataLayerError>;
}
