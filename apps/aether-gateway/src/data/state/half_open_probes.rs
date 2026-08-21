use aether_data_contracts::repository::half_open_probes::{
    HalfOpenProbeCompletion, HalfOpenProbeCompletionWrite,
};

use super::{DataLayerError, GatewayDataState};

impl GatewayDataState {
    pub(crate) async fn complete_half_open_probe_if_newer(
        &self,
        completion: HalfOpenProbeCompletion,
    ) -> Result<HalfOpenProbeCompletionWrite, DataLayerError> {
        let repository = self
            .backends
            .as_ref()
            .and_then(|backends| backends.write().half_open_probe_completions())
            .ok_or_else(|| {
                DataLayerError::InvalidConfiguration(
                    "distributed half-open probe completion requires a SQL write backend"
                        .to_string(),
                )
            })?;
        repository.complete_if_newer(completion).await
    }
}
