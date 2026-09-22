use crate::config::ServiceRuntimeConfig;
use crate::error::RuntimeBootstrapError;

/// Executes `init_service_runtime`.
pub fn init_service_runtime(config: ServiceRuntimeConfig) -> Result<(), RuntimeBootstrapError> {
    crate::tracing::init_tracing(config.clone())?;
    crate::metrics::init_metrics(config);
    Ok(())
}
