use std::sync::Arc;

use aether_data::repository::emergency_chain::EmergencyChainGrantRepository;

use super::GatewayDataState;

impl GatewayDataState {
    pub(crate) fn emergency_chain_grant_repository(
        &self,
    ) -> Option<Arc<dyn EmergencyChainGrantRepository>> {
        self.backends
            .as_ref()?
            .postgres()
            .map(|backend| backend.emergency_chain_grant_repository())
    }
}
