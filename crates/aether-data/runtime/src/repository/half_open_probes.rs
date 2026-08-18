use aether_data_contracts::repository::half_open_probes::{
    HalfOpenProbeCompletion, HalfOpenProbeCompletionRepository, HalfOpenProbeCompletionWrite,
};
use async_trait::async_trait;

use crate::DataLayerError;

#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlHalfOpenProbeCompletionRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::PostgresHalfOpenProbeCompletionRepository;
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteHalfOpenProbeCompletionRepository;

/// Deliberately unavailable distributed completion store for the memory backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct FailClosedMemoryHalfOpenProbeCompletionRepository;

#[async_trait]
impl HalfOpenProbeCompletionRepository for FailClosedMemoryHalfOpenProbeCompletionRepository {
    async fn complete_if_newer(
        &self,
        _completion: HalfOpenProbeCompletion,
    ) -> Result<HalfOpenProbeCompletionWrite, DataLayerError> {
        Err(DataLayerError::InvalidConfiguration(
            "distributed half-open probe completion requires a SQL backend; memory is fail-closed"
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_data_contracts::repository::half_open_probes::{
        HalfOpenProbeOutcome, HalfOpenProbeScope,
    };

    #[tokio::test]
    async fn memory_completion_store_fails_closed() {
        let repository = FailClosedMemoryHalfOpenProbeCompletionRepository;
        let completion = HalfOpenProbeCompletion {
            completion_id: "completion-1".to_string(),
            scope: HalfOpenProbeScope::new("key-1", "openai").expect("scope"),
            owner: "node-1".to_string(),
            fencing_token: 1,
            completed_at_unix_ms: 1,
            outcome: HalfOpenProbeOutcome::Succeeded,
        };
        let error = repository
            .complete_if_newer(completion)
            .await
            .expect_err("memory must not persist distributed completions");
        assert!(error.to_string().contains("memory is fail-closed"));
    }
}
