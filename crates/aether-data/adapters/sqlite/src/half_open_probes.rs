use aether_data_contracts::repository::half_open_probes::{
    HalfOpenProbeCompletion, HalfOpenProbeCompletionRepository, HalfOpenProbeCompletionWrite,
    HalfOpenProbeOutcome, HalfOpenProbeScope, StoredHalfOpenProbeCompletion,
};
use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use crate::error::SqlResultExt;
use crate::DataLayerError;

const UPSERT_SQL: &str = r#"
INSERT INTO half_open_probe_completions (
  provider_key_id, api_format, completion_id, owner,
  fencing_token, completed_at_unix_ms, outcome
)
VALUES (?, ?, ?, ?, ?, ?, ?)
ON CONFLICT (provider_key_id, api_format) DO UPDATE SET
  completion_id = excluded.completion_id,
  owner = excluded.owner,
  fencing_token = excluded.fencing_token,
  completed_at_unix_ms = excluded.completed_at_unix_ms,
  outcome = excluded.outcome
WHERE half_open_probe_completions.fencing_token < excluded.fencing_token
RETURNING completion_id, provider_key_id, api_format, owner,
          fencing_token, completed_at_unix_ms, outcome
"#;

#[derive(Debug, Clone)]
pub struct SqliteHalfOpenProbeCompletionRepository {
    pool: SqlitePool,
}

impl SqliteHalfOpenProbeCompletionRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl HalfOpenProbeCompletionRepository for SqliteHalfOpenProbeCompletionRepository {
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
            .map_sql_err()?;
        if let Some(row) = row {
            return Ok(HalfOpenProbeCompletionWrite::Applied(map_row(&row)?));
        }
        let current: i64 = sqlx::query_scalar(
            "SELECT fencing_token FROM half_open_probe_completions WHERE provider_key_id = ? AND api_format = ?",
        )
        .bind(&completion.scope.provider_key_id)
        .bind(&completion.scope.api_format)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        Ok(HalfOpenProbeCompletionWrite::RejectedStale {
            current_fencing_token: unsigned(current, "half-open probe stored fencing_token")?,
        })
    }
}

fn map_row(row: &sqlx::sqlite::SqliteRow) -> Result<StoredHalfOpenProbeCompletion, DataLayerError> {
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
    use super::*;
    use crate::run_migrations;

    fn completion(id: &str, fence: u64, outcome: HalfOpenProbeOutcome) -> HalfOpenProbeCompletion {
        HalfOpenProbeCompletion {
            completion_id: id.to_string(),
            scope: HalfOpenProbeScope::new("provider-key-1", "openai").expect("scope"),
            owner: "node-1".to_string(),
            fencing_token: fence,
            completed_at_unix_ms: 1_000 + fence,
            outcome,
        }
    }

    async fn repository() -> SqliteHalfOpenProbeCompletionRepository {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool");
        run_migrations(&pool).await.expect("migrations");
        SqliteHalfOpenProbeCompletionRepository::new(pool)
    }

    #[tokio::test]
    async fn completion_accepts_only_strictly_increasing_fences() {
        let repository = repository().await;
        let first = repository
            .complete_if_newer(completion("first", 10, HalfOpenProbeOutcome::Succeeded))
            .await
            .expect("first write");
        assert!(matches!(first, HalfOpenProbeCompletionWrite::Applied(_)));

        let equal = repository
            .complete_if_newer(completion("equal", 10, HalfOpenProbeOutcome::Failed))
            .await
            .expect("equal write");
        assert_eq!(
            equal,
            HalfOpenProbeCompletionWrite::RejectedStale {
                current_fencing_token: 10
            }
        );
        let stale = repository
            .complete_if_newer(completion("stale", 9, HalfOpenProbeOutcome::Failed))
            .await
            .expect("stale write");
        assert_eq!(
            stale,
            HalfOpenProbeCompletionWrite::RejectedStale {
                current_fencing_token: 10
            }
        );

        let newer = repository
            .complete_if_newer(completion("newer", 11, HalfOpenProbeOutcome::Failed))
            .await
            .expect("newer write");
        let HalfOpenProbeCompletionWrite::Applied(stored) = newer else {
            panic!("newer fence must apply");
        };
        assert_eq!(stored.fencing_token, 11);
        assert_eq!(stored.completion_id, "newer");
        assert_eq!(stored.outcome, HalfOpenProbeOutcome::Failed);
    }

    #[tokio::test]
    async fn missing_completion_table_fails_closed() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool");
        let repository = SqliteHalfOpenProbeCompletionRepository::new(pool);
        assert!(repository
            .complete_if_newer(completion("first", 1, HalfOpenProbeOutcome::Succeeded))
            .await
            .is_err());
    }

    #[test]
    fn sqlite_upsert_is_strictly_monotonic() {
        assert!(UPSERT_SQL
            .contains("half_open_probe_completions.fencing_token < excluded.fencing_token"));
        assert!(!UPSERT_SQL.contains("<="));
    }
}
