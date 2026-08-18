use aether_data_contracts::repository::half_open_probes::{
    HalfOpenProbeCompletion, HalfOpenProbeCompletionRepository, HalfOpenProbeCompletionWrite,
    HalfOpenProbeOutcome, HalfOpenProbeScope, StoredHalfOpenProbeCompletion,
};
use async_trait::async_trait;
use sqlx::{PgPool, Row};

use crate::error::SqlxResultExt;
use crate::DataLayerError;

const UPSERT_SQL: &str = r#"
INSERT INTO half_open_probe_completions (
  provider_key_id, api_format, completion_id, owner,
  fencing_token, completed_at_unix_ms, outcome
)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (provider_key_id, api_format) DO UPDATE SET
  completion_id = EXCLUDED.completion_id,
  owner = EXCLUDED.owner,
  fencing_token = EXCLUDED.fencing_token,
  completed_at_unix_ms = EXCLUDED.completed_at_unix_ms,
  outcome = EXCLUDED.outcome
WHERE half_open_probe_completions.fencing_token < EXCLUDED.fencing_token
RETURNING completion_id, provider_key_id, api_format, owner,
          fencing_token, completed_at_unix_ms, outcome
"#;

#[derive(Debug, Clone)]
pub struct PostgresHalfOpenProbeCompletionRepository {
    pool: PgPool,
}

impl PostgresHalfOpenProbeCompletionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl HalfOpenProbeCompletionRepository for PostgresHalfOpenProbeCompletionRepository {
    async fn complete_if_newer(
        &self,
        completion: HalfOpenProbeCompletion,
    ) -> Result<HalfOpenProbeCompletionWrite, DataLayerError> {
        completion.validate()?;
        let fence = signed(completion.fencing_token, "half-open probe fencing_token")?;
        let completed_at = signed(
            completion.completed_at_unix_ms,
            "half-open probe completed_at_unix_ms",
        )?;
        let row = sqlx::query(UPSERT_SQL)
            .bind(&completion.scope.provider_key_id)
            .bind(&completion.scope.api_format)
            .bind(&completion.completion_id)
            .bind(&completion.owner)
            .bind(fence)
            .bind(completed_at)
            .bind(completion.outcome.as_database())
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        if let Some(row) = row {
            return Ok(HalfOpenProbeCompletionWrite::Applied(map_row(&row)?));
        }
        let current: i64 = sqlx::query_scalar(
            "SELECT fencing_token FROM half_open_probe_completions WHERE provider_key_id = $1 AND api_format = $2",
        )
        .bind(&completion.scope.provider_key_id)
        .bind(&completion.scope.api_format)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(HalfOpenProbeCompletionWrite::RejectedStale {
            current_fencing_token: unsigned(current, "half-open probe stored fencing_token")?,
        })
    }
}

fn map_row(row: &sqlx::postgres::PgRow) -> Result<StoredHalfOpenProbeCompletion, DataLayerError> {
    Ok(StoredHalfOpenProbeCompletion {
        completion_id: row.try_get("completion_id").map_postgres_err()?,
        scope: HalfOpenProbeScope {
            provider_key_id: row.try_get("provider_key_id").map_postgres_err()?,
            api_format: row.try_get("api_format").map_postgres_err()?,
        },
        owner: row.try_get("owner").map_postgres_err()?,
        fencing_token: unsigned(
            row.try_get("fencing_token").map_postgres_err()?,
            "half-open probe stored fencing_token",
        )?,
        completed_at_unix_ms: unsigned(
            row.try_get("completed_at_unix_ms").map_postgres_err()?,
            "half-open probe stored completed_at_unix_ms",
        )?,
        outcome: HalfOpenProbeOutcome::from_database(
            row.try_get::<String, _>("outcome")
                .map_postgres_err()?
                .as_str(),
        )?,
    })
}

fn signed(value: u64, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field} exceeds signed 64-bit range")))
}

fn unsigned(value: i64, field: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("{field} is negative")))
}

#[cfg(test)]
mod tests {
    use super::UPSERT_SQL;

    #[test]
    fn postgres_upsert_is_strictly_monotonic() {
        assert!(UPSERT_SQL
            .contains("half_open_probe_completions.fencing_token < EXCLUDED.fencing_token"));
        assert!(!UPSERT_SQL.contains("<="));
    }
}
