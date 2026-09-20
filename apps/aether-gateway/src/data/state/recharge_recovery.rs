use super::GatewayDataState;
use aether_data_contracts::repository::settlement::{
    CompleteRechargeRecoveryNotificationInput, RechargeRecoveryNotification,
    SettlementWriteRepository, StoredRechargeRecoveryJob,
};
use aether_data_contracts::DataLayerError;

impl GatewayDataState {
    #[cfg(test)]
    pub(crate) fn with_recharge_recovery_repository_for_tests(
        mut self,
        repository: std::sync::Arc<dyn SettlementWriteRepository>,
    ) -> Self {
        self.settlement_writer = Some(repository);
        self
    }

    pub(crate) fn has_recharge_recovery_backend(&self) -> bool {
        self.settlement_writer
            .as_ref()
            .is_some_and(|repository| repository.supports_recharge_recovery())
    }

    fn recharge_recovery_repository(
        &self,
    ) -> Result<&dyn SettlementWriteRepository, DataLayerError> {
        self.settlement_writer
            .as_deref()
            .filter(|repository| repository.supports_recharge_recovery())
            .ok_or_else(|| DataLayerError::InvalidInput("recharge recovery is unavailable".into()))
    }

    pub(crate) async fn process_recharge_recovery_batch(
        &self,
        batch_size: usize,
    ) -> Result<Vec<StoredRechargeRecoveryJob>, DataLayerError> {
        self.recharge_recovery_repository()?
            .process_recharge_recovery_batch(batch_size)
            .await
    }

    pub(crate) async fn list_recharge_recovery_jobs_for_user(
        &self,
        user_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredRechargeRecoveryJob>, DataLayerError> {
        self.recharge_recovery_repository()?
            .list_recharge_recovery_jobs_for_user(user_id, limit)
            .await
    }

    pub(crate) async fn claim_recharge_recovery_notifications(
        &self,
        limit: usize,
    ) -> Result<Vec<RechargeRecoveryNotification>, DataLayerError> {
        self.recharge_recovery_repository()?
            .claim_recharge_recovery_notifications(limit)
            .await
    }

    pub(crate) async fn complete_recharge_recovery_notification(
        &self,
        input: CompleteRechargeRecoveryNotificationInput,
    ) -> Result<bool, DataLayerError> {
        self.recharge_recovery_repository()?
            .complete_recharge_recovery_notification(input)
            .await
    }
}
