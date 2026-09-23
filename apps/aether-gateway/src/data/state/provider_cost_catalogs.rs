use aether_data::backend::ProviderCostCatalogDataState;
use aether_data::repository::provider_cost_catalog::{
    ProviderCostCatalogDeleteOutcome, ProviderCostCatalogListQuery, ProviderCostCatalogRecord,
    ProviderCostCatalogUpsertOutcome, ProviderCostTaskType,
};
use aether_data::DataLayerError;

use super::GatewayDataState;

impl GatewayDataState {
    fn provider_cost_catalogs(&self) -> ProviderCostCatalogDataState<'_> {
        ProviderCostCatalogDataState::new(self.backends.as_ref())
    }

    pub(crate) fn has_provider_cost_catalog_backend(&self) -> bool {
        self.provider_cost_catalogs()
            .has_provider_cost_catalog_backend()
    }

    pub(crate) async fn upsert_provider_cost_catalog(
        &self,
        record: ProviderCostCatalogRecord,
    ) -> Result<Option<ProviderCostCatalogUpsertOutcome>, DataLayerError> {
        self.provider_cost_catalogs().upsert(record).await
    }

    pub(crate) async fn get_provider_cost_catalog(
        &self,
        cost_id: &str,
    ) -> Result<Option<Option<ProviderCostCatalogRecord>>, DataLayerError> {
        self.provider_cost_catalogs().get(cost_id).await
    }

    pub(crate) async fn list_provider_cost_catalogs(
        &self,
        query: &ProviderCostCatalogListQuery,
    ) -> Result<Option<Vec<ProviderCostCatalogRecord>>, DataLayerError> {
        self.provider_cost_catalogs().list(query).await
    }

    pub(crate) async fn find_effective_provider_cost_catalog(
        &self,
        provider_id: &str,
        model: &str,
        task_type: ProviderCostTaskType,
        at_unix_secs: u64,
    ) -> Result<Option<Option<ProviderCostCatalogRecord>>, DataLayerError> {
        self.provider_cost_catalogs()
            .find_effective(provider_id, model, task_type, at_unix_secs)
            .await
    }

    pub(crate) async fn delete_provider_cost_catalog(
        &self,
        cost_id: &str,
    ) -> Result<Option<ProviderCostCatalogDeleteOutcome>, DataLayerError> {
        self.provider_cost_catalogs().delete(cost_id).await
    }
}
