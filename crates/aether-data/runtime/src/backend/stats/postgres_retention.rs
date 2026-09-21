use crate::backend::PostgresBackend;
use crate::{error::postgres_error, DataLayerError, StatsRetentionCleanupSummary};

/// Time-bucketed hourly aggregate tables pruned by the stats retention pass,
/// each keyed by the `hour_utc` unix-second bucket column.
const HOURLY_RETENTION_TABLES: &[&str] = &[
    "stats_hourly",
    "stats_hourly_user",
    "stats_hourly_user_model",
    "stats_hourly_model",
    "stats_hourly_provider",
];

/// Time-bucketed daily aggregate tables pruned by the stats retention pass,
/// each keyed by the `date` unix-second bucket column. Summary/counter tables
/// (`stats_summary`, `stats_user_summary`) are intentionally absent: they are
/// cumulative snapshots, not per-day buckets.
const DAILY_RETENTION_TABLES: &[&str] = &[
    "stats_daily",
    "stats_daily_model",
    "stats_daily_provider",
    "stats_daily_api_key",
    "stats_daily_error",
    "stats_daily_model_provider",
    "stats_user_daily",
    "stats_user_daily_model",
    "stats_user_daily_provider",
    "stats_user_daily_api_format",
    "stats_user_daily_model_provider",
    "stats_daily_cost_savings",
    "stats_daily_cost_savings_provider",
    "stats_daily_cost_savings_model",
    "stats_daily_cost_savings_model_provider",
    "stats_user_daily_cost_savings",
    "stats_user_daily_cost_savings_provider",
    "stats_user_daily_cost_savings_model",
    "stats_user_daily_cost_savings_model_provider",
];

impl PostgresBackend {
    /// Delete aggregate rows whose time bucket is older than the configured
    /// retention cutoff. Each table is pruned in bounded batches so a first
    /// run after a long gap cannot monopolize the pool. The statement is a
    /// fixed whitelist; table names never come from callers.
    pub async fn cleanup_stats_aggregates(
        &self,
        hourly_before_unix_secs: u64,
        daily_before_unix_secs: u64,
        batch_limit: usize,
    ) -> Result<StatsRetentionCleanupSummary, DataLayerError> {
        let batch_limit = i64::try_from(batch_limit.max(1)).unwrap_or(i64::MAX);
        let mut summary = StatsRetentionCleanupSummary::default();
        for table in HOURLY_RETENTION_TABLES {
            let deleted = delete_expired_buckets(
                self.pool(),
                table,
                "hour_utc",
                hourly_before_unix_secs as i64,
                batch_limit,
            )
            .await
            .map_err(postgres_error)?;
            summary.hourly_rows_deleted = summary.hourly_rows_deleted.saturating_add(deleted);
        }
        for table in DAILY_RETENTION_TABLES {
            let deleted = delete_expired_buckets(
                self.pool(),
                table,
                "date",
                daily_before_unix_secs as i64,
                batch_limit,
            )
            .await
            .map_err(postgres_error)?;
            summary.daily_rows_deleted = summary.daily_rows_deleted.saturating_add(deleted);
        }
        Ok(summary)
    }
}

async fn delete_expired_buckets(
    pool: &crate::driver::postgres::PostgresPool,
    table: &str,
    bucket_column: &str,
    before_unix_secs: i64,
    batch_limit: i64,
) -> Result<u64, sqlx::Error> {
    let sql = format!(
        "DELETE FROM {table} WHERE ctid IN (\
            SELECT ctid FROM {table} WHERE {bucket_column} < $1 LIMIT $2)"
    );
    let result = sqlx::query(&sql)
        .bind(before_unix_secs)
        .bind(batch_limit)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::{DAILY_RETENTION_TABLES, HOURLY_RETENTION_TABLES};

    #[test]
    fn retention_tables_are_whitelisted_and_disjoint() {
        let mut all: Vec<&str> = HOURLY_RETENTION_TABLES
            .iter()
            .chain(DAILY_RETENTION_TABLES.iter())
            .copied()
            .collect();
        let unique = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(unique, all.len(), "retention tables must not repeat");
        assert!(!all.contains(&"stats_summary"));
        assert!(!all.contains(&"stats_user_summary"));
        for table in &all {
            assert!(
                table
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
                "retention table name must be a plain identifier: {table}"
            );
        }
    }
}
