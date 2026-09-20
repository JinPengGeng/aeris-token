use crate::repository::provider_cost::{
    ProviderCostImportOutcome, ProviderCostListQuery, ProviderCostPrice, ProviderCostRepository,
    ProviderCostSnapshotImport, ProviderCostSummaryQuery, StoredProviderCostSnapshot,
    StoredProviderCostSummaryRow,
};
use crate::{DataBackends, DataLayerError};

#[derive(Debug, Clone, Copy)]
pub struct ProviderCostDataState<'a> {
    backends: Option<&'a DataBackends>,
}

impl<'a> ProviderCostDataState<'a> {
    pub fn new(backends: Option<&'a DataBackends>) -> Self {
        Self { backends }
    }

    pub fn has_provider_cost_data_backend(&self) -> bool {
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
    fn repository(&self) -> Option<std::sync::Arc<dyn ProviderCostRepository>> {
        self.backends
            .and_then(DataBackends::postgres)
            .map(crate::backend::PostgresBackend::provider_cost_repository)
    }

    pub async fn import_price(
        &self,
        price: &ProviderCostPrice,
    ) -> Result<Option<ProviderCostImportOutcome<ProviderCostPrice>>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(repository) = self.repository() {
            return repository.import_price(price).await.map(Some);
        }
        Ok(None)
    }

    pub async fn list_prices(
        &self,
        query: &ProviderCostListQuery,
    ) -> Result<Option<Vec<ProviderCostPrice>>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(repository) = self.repository() {
            return repository.list_prices(query).await.map(Some);
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn find_effective_price(
        &self,
        supplier: &str,
        provider: &str,
        model: &str,
        dimension: crate::repository::provider_cost::ProviderCostDimension,
        currency: &str,
        unit: crate::repository::provider_cost::ProviderCostUnit,
        at_unix_secs: u64,
    ) -> Result<Option<ProviderCostPrice>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(repository) = self.repository() {
            return repository
                .find_effective_price(
                    supplier,
                    provider,
                    model,
                    dimension,
                    currency,
                    unit,
                    at_unix_secs,
                )
                .await;
        }
        Ok(None)
    }

    pub async fn import_snapshot(
        &self,
        snapshot: &ProviderCostSnapshotImport,
    ) -> Result<Option<ProviderCostImportOutcome<StoredProviderCostSnapshot>>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(repository) = self.repository() {
            return repository.import_snapshot(snapshot).await.map(Some);
        }
        Ok(None)
    }

    pub async fn list_snapshots_for_request(
        &self,
        request_id: &str,
    ) -> Result<Option<Vec<StoredProviderCostSnapshot>>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(repository) = self.repository() {
            return repository
                .list_snapshots_for_request(request_id)
                .await
                .map(Some);
        }
        Ok(None)
    }

    pub async fn summarize_snapshots(
        &self,
        query: &ProviderCostSummaryQuery,
    ) -> Result<Option<Vec<StoredProviderCostSummaryRow>>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(repository) = self.repository() {
            return repository.summarize_snapshots(query).await.map(Some);
        }
        Ok(None)
    }
}
