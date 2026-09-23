use super::AppState;
use crate::GatewayError;
use aether_data::repository::provider_cost_catalog::{
    ProviderCostCatalogDeleteOutcome, ProviderCostCatalogListQuery, ProviderCostCatalogRecord,
    ProviderCostCatalogUpsertOutcome, ProviderCostTaskType,
};

impl AppState {
    pub(crate) fn has_provider_cost_catalog_backend(&self) -> bool {
        self.data.has_provider_cost_catalog_backend()
    }

    pub(crate) async fn upsert_provider_cost_catalog(
        &self,
        record: ProviderCostCatalogRecord,
    ) -> Result<Option<ProviderCostCatalogUpsertOutcome>, GatewayError> {
        self.data
            .upsert_provider_cost_catalog(record)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))
    }

    pub(crate) async fn get_provider_cost_catalog(
        &self,
        cost_id: &str,
    ) -> Result<Option<Option<ProviderCostCatalogRecord>>, GatewayError> {
        self.data
            .get_provider_cost_catalog(cost_id)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))
    }

    pub(crate) async fn list_provider_cost_catalogs(
        &self,
        query: &ProviderCostCatalogListQuery,
    ) -> Result<Option<Vec<ProviderCostCatalogRecord>>, GatewayError> {
        self.data
            .list_provider_cost_catalogs(query)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))
    }

    pub(crate) async fn find_effective_provider_cost_catalog(
        &self,
        provider_id: &str,
        model: &str,
        task_type: ProviderCostTaskType,
        at_unix_secs: u64,
    ) -> Result<Option<Option<ProviderCostCatalogRecord>>, GatewayError> {
        self.data
            .find_effective_provider_cost_catalog(provider_id, model, task_type, at_unix_secs)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))
    }

    pub(crate) async fn delete_provider_cost_catalog(
        &self,
        cost_id: &str,
    ) -> Result<Option<ProviderCostCatalogDeleteOutcome>, GatewayError> {
        self.data
            .delete_provider_cost_catalog(cost_id)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))
    }
}
