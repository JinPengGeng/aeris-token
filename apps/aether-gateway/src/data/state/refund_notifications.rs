use super::GatewayDataState;
use aether_data_contracts::repository::wallet::{
    CompleteRefundStatusNotificationInput, RefundStatusNotification, WalletWriteRepository,
};
use aether_data_contracts::DataLayerError;

impl GatewayDataState {
    #[cfg(test)]
    pub(crate) fn with_refund_notification_repository_for_tests(
        mut self,
        repository: std::sync::Arc<dyn WalletWriteRepository>,
    ) -> Self {
        self.wallet_writer = Some(repository);
        self
    }
    pub(crate) fn has_refund_notification_backend(&self) -> bool {
        self.wallet_writer
            .as_ref()
            .is_some_and(|repo| repo.supports_refund_status_notifications())
    }
    fn refund_notification_repository(&self) -> Result<&dyn WalletWriteRepository, DataLayerError> {
        self.wallet_writer
            .as_deref()
            .filter(|repo| repo.supports_refund_status_notifications())
            .ok_or_else(|| {
                DataLayerError::InvalidInput("refund notification outbox unavailable".into())
            })
    }
    pub(crate) async fn claim_refund_status_notifications(
        &self,
        limit: usize,
    ) -> Result<Vec<RefundStatusNotification>, DataLayerError> {
        self.refund_notification_repository()?
            .claim_refund_status_notifications(limit)
            .await
    }
    pub(crate) async fn complete_refund_status_notification(
        &self,
        input: CompleteRefundStatusNotificationInput,
    ) -> Result<bool, DataLayerError> {
        self.refund_notification_repository()?
            .complete_refund_status_notification(input)
            .await
    }
}
