use aether_data_contracts::repository::half_open_probes::{
    HalfOpenProbeCompletion, HalfOpenProbeCompletionRepository, HalfOpenProbeCompletionWrite,
    HalfOpenProbeOutcome, HalfOpenProbeScope, StoredHalfOpenProbeCompletion,
};
use async_trait::async_trait;
use sqlx::{MySqlPool, Row};

use crate::error::SqlResultExt;
use crate::DataLayerError;

const UPSERT_SQL: &str = r#"
INSERT INTO half_open_probe_completions (
  provider_key_id, api_format, completion_id, owner,
  fencing_token, completed_at_unix_ms, outcome
)
VALUES (?, ?, ?, ?, ?, ?, ?)
ON DUPLICATE KEY UPDATE
  completion_id = IF(VALUES(fencing_token) > fencing_token, VALUES(completion_id), completion_id),
  owner = IF(VALUES(fencing_token) > fencing_token, VALUES(owner), owner),
  completed_at_unix_ms = IF(VALUES(fencing_token) > fencing_token, VALUES(completed_at_unix_ms), completed_at_unix_ms),
  outcome = IF(VALUES(fencing_token) > fencing_token, VALUES(outcome), outcome),
  fencing_token = IF(VALUES(fencing_token) > fencing_token, VALUES(fencing_token), fencing_token)
"#;

#[derive(Debug, Clone)]
pub struct MysqlHalfOpenProbeCompletionRepository {
    pool: MySqlPool,
}

impl MysqlHalfOpenProbeCompletionRepository {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl HalfOpenProbeCompletionRepository for MysqlHalfOpenProbeCompletionRepository {
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
        let mut tx = self.pool.begin().await.map_sql_err()?;
        sqlx::query(UPSERT_SQL)
            .bind(&completion.scope.provider_key_id)
            .bind(&completion.scope.api_format)
            .bind(&completion.completion_id)
            .bind(&completion.owner)
            .bind(fence)
            .bind(completed_at)
            .bind(completion.outcome.as_database())
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        let row = sqlx::query(
            r#"SELECT completion_id, provider_key_id, api_format, owner,
                      fencing_token, completed_at_unix_ms, outcome
               FROM half_open_probe_completions
               WHERE provider_key_id = ? AND api_format = ?
               FOR UPDATE"#,
        )
        .bind(&completion.scope.provider_key_id)
        .bind(&completion.scope.api_format)
        .fetch_one(&mut *tx)
        .await
        .map_sql_err()?;
        let stored = map_row(&row)?;
        tx.commit().await.map_sql_err()?;

        if stored.completion_id == completion.completion_id
            && stored.fencing_token == completion.fencing_token
        {
            Ok(HalfOpenProbeCompletionWrite::Applied(stored))
        } else {
            Ok(HalfOpenProbeCompletionWrite::RejectedStale {
                current_fencing_token: stored.fencing_token,
            })
        }
    }
}

fn map_row(row: &sqlx::mysql::MySqlRow) -> Result<StoredHalfOpenProbeCompletion, DataLayerError> {
    Ok(StoredHalfOpenProbeCompletion {
        completion_id: row.try_get("completion_id").map_sql_err()?,
        scope: HalfOpenProbeScope {
            provider_key_id: row.try_get("provider_key_id").map_sql_err()?,
            api_format: row.try_get("api_format").map_sql_err()?,
        },
        owner: row.try_get("owner").map_sql_err()?,
        fencing_token: unsigned(
            row.try_get("fencing_token").map_sql_err()?,
            "half-open probe stored fencing_token",
        )?,
        completed_at_unix_ms: unsigned(
            row.try_get("completed_at_unix_ms").map_sql_err()?,
            "half-open probe stored completed_at_unix_ms",
        )?,
        outcome: HalfOpenProbeOutcome::from_database(
            row.try_get::<String, _>("outcome").map_sql_err()?.as_str(),
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
    fn mysql_upsert_guards_every_mutation_with_strict_fence() {
        assert_eq!(
            UPSERT_SQL
                .matches("VALUES(fencing_token) > fencing_token")
                .count(),
            5
        );
        assert!(!UPSERT_SQL.contains(">="));
    }
}
