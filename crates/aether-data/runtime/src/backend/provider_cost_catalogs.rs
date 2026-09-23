use crate::repository::provider_cost_catalog::{
    ProviderCostCatalogDeleteOutcome, ProviderCostCatalogListQuery, ProviderCostCatalogRecord,
    ProviderCostCatalogRepository, ProviderCostCatalogUpsertOutcome, ProviderCostTaskType,
};
use crate::{DataBackends, DataLayerError};

#[derive(Debug, Clone, Copy)]
pub struct ProviderCostCatalogDataState<'a> {
    backends: Option<&'a DataBackends>,
}

impl<'a> ProviderCostCatalogDataState<'a> {
    pub fn new(backends: Option<&'a DataBackends>) -> Self {
        Self { backends }
    }

    pub fn has_provider_cost_catalog_backend(&self) -> bool {
        #[cfg(feature = "postgres")]
        {
            self.backends.and_then(DataBackends::postgres).is_some()
        }
        #[cfg(not(feature = "postgres"))]
        {
            false
        }
    }

    #[cfg(feature = "postgres")]
    fn repository(&self) -> Option<std::sync::Arc<dyn ProviderCostCatalogRepository>> {
        self.backends
            .and_then(DataBackends::postgres)
            .map(crate::backend::PostgresBackend::provider_cost_catalog_repository)
    }

    pub async fn upsert(
        &self,
        record: ProviderCostCatalogRecord,
    ) -> Result<Option<ProviderCostCatalogUpsertOutcome>, DataLayerError> {
        record.validate()?;
        #[cfg(feature = "postgres")]
        if let Some(repository) = self.repository() {
            return repository
                .upsert_provider_cost_catalog(record)
                .await
                .map(Some);
        }
        Ok(None)
    }

    pub async fn get(
        &self,
        cost_id: &str,
    ) -> Result<Option<Option<ProviderCostCatalogRecord>>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(repository) = self.repository() {
            return repository
                .get_provider_cost_catalog(cost_id)
                .await
                .map(Some);
        }
        Ok(None)
    }

    pub async fn list(
        &self,
        query: &ProviderCostCatalogListQuery,
    ) -> Result<Option<Vec<ProviderCostCatalogRecord>>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(repository) = self.repository() {
            return repository
                .list_provider_cost_catalogs(query)
                .await
                .map(Some);
        }
        Ok(None)
    }

    pub async fn find_effective(
        &self,
        provider_id: &str,
        model: &str,
        task_type: ProviderCostTaskType,
        at_unix_secs: u64,
    ) -> Result<Option<Option<ProviderCostCatalogRecord>>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(repository) = self.repository() {
            return repository
                .find_effective_provider_cost_catalog(provider_id, model, task_type, at_unix_secs)
                .await
                .map(Some);
        }
        Ok(None)
    }

    pub async fn delete(
        &self,
        cost_id: &str,
    ) -> Result<Option<ProviderCostCatalogDeleteOutcome>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(repository) = self.repository() {
            return repository
                .delete_provider_cost_catalog(cost_id)
                .await
                .map(Some);
        }
        Ok(None)
    }
}
