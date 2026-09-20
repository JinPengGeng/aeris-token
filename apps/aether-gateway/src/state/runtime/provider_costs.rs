use super::AppState;
use crate::GatewayError;
use aether_data::repository::provider_cost::{
    ProviderCostImportOutcome, ProviderCostListQuery, ProviderCostPrice,
    ProviderCostSnapshotImport, ProviderCostSummaryQuery, StoredProviderCostSnapshot,
    StoredProviderCostSummaryRow,
};

impl AppState {
    pub(crate) fn has_provider_cost_data_backend(&self) -> bool {
        self.data.has_provider_cost_data_backend()
    }

    pub(crate) async fn import_provider_cost_price(
        &self,
        price: &ProviderCostPrice,
    ) -> Result<Option<ProviderCostImportOutcome<ProviderCostPrice>>, aether_data::DataLayerError>
    {
        self.data.import_provider_cost_price(price).await
    }

    pub(crate) async fn list_provider_cost_prices(
        &self,
        query: &ProviderCostListQuery,
    ) -> Result<Option<Vec<ProviderCostPrice>>, GatewayError> {
        self.data
            .list_provider_cost_prices(query)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))
    }

    pub(crate) async fn import_provider_cost_snapshot(
        &self,
        snapshot: &ProviderCostSnapshotImport,
    ) -> Result<
        Option<ProviderCostImportOutcome<StoredProviderCostSnapshot>>,
        aether_data::DataLayerError,
    > {
        self.data.import_provider_cost_snapshot(snapshot).await
    }

    pub(crate) async fn summarize_provider_cost_snapshots(
        &self,
        query: &ProviderCostSummaryQuery,
    ) -> Result<Option<Vec<StoredProviderCostSummaryRow>>, GatewayError> {
        self.data
            .summarize_provider_cost_snapshots(query)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))
    }
}
