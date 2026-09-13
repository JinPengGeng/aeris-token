use crate::{error::SqlxResultExt, DataLayerError};
use aether_data_contracts::repository::{
    settlement::RequestFundsSummary,
    usage::{
        next_daily_cost_contribution, DailyActualCostCounts, DailyActualCostQuery,
        DailyCostContribution, StoredRequestUsageAudit,
    },
};
use sqlx::{Postgres, Row, Transaction};

// Both aggregates see the same committed snapshot and can use independent scope indexes.
const READ_SCOPES_SQL: &str = r#"
SELECT
 (SELECT COALESCE(SUM(actual_cost_units), 0)::bigint
  FROM usage_daily_cost_contributions
  WHERE user_id = $1 AND NOT api_key_is_standalone
    AND api_key_id IS NOT NULL AND btrim(api_key_id) <> ''
    AND accounting_at >= to_timestamp($3::double precision)
    AND accounting_at < to_timestamp($4::double precision)) AS user_units,
 (SELECT COALESCE(SUM(actual_cost_units), 0)::bigint
  FROM usage_daily_cost_contributions
  WHERE api_key_id = $2
    AND accounting_at >= to_timestamp($3::double precision)
    AND accounting_at < to_timestamp($4::double precision)) AS key_units
"#;

pub(super) async fn read_scopes(
    pool: &sqlx::PgPool,
    query: &DailyActualCostQuery,
) -> Result<DailyActualCostCounts, DataLayerError> {
    query.validate()?;
    let row = sqlx::query(READ_SCOPES_SQL)
        .bind(&query.user_id)
        .bind(&query.api_key_id)
        .bind(query.start_unix_secs as i64)
        .bind(query.end_unix_secs as i64)
        .fetch_one(pool)
        .await
        .map_postgres_err()?;
    Ok(DailyActualCostCounts {
        user_units: row.try_get::<i64, _>("user_units").map_postgres_err()? as u64,
        key_units: row.try_get::<i64, _>("key_units").map_postgres_err()? as u64,
    })
}

/// The caller already holds the parent request lock. This write commits or rolls
/// back with the usage/child financial facts; no post-commit counter hook exists.
pub(super) async fn sync_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    parent: &StoredRequestUsageAudit,
    funds: Option<&RequestFundsSummary>,
) -> Result<(), DataLayerError> {
    let previous = sqlx::query("SELECT request_id, usage_id, user_id, api_key_id, api_key_is_standalone, attempt_funds, actual_cost_units, FLOOR(EXTRACT(EPOCH FROM accounting_at))::bigint AS accounting_at FROM usage_daily_cost_contributions WHERE request_id = $1 FOR UPDATE")
        .bind(&parent.request_id).fetch_optional(&mut **tx).await.map_postgres_err()?
        .map(|row| -> Result<DailyCostContribution, DataLayerError> {
            Ok(DailyCostContribution {
                request_id: row.try_get("request_id").map_postgres_err()?,
                usage_id: row.try_get("usage_id").map_postgres_err()?,
                user_id: row.try_get("user_id").map_postgres_err()?,
                api_key_id: row.try_get("api_key_id").map_postgres_err()?,
                api_key_is_standalone: row.try_get("api_key_is_standalone").map_postgres_err()?,
                attempt_funds: row.try_get("attempt_funds").map_postgres_err()?,
                actual_cost_units: row.try_get::<i64,_>("actual_cost_units").map_postgres_err()? as u64,
                accounting_at_unix_secs: row.try_get::<Option<i64>,_>("accounting_at").map_postgres_err()?.map(|at| at.max(0) as u64),
            })
        }).transpose()?;
    let next = next_daily_cost_contribution(previous.as_ref(), parent, funds)?;
    if previous.as_ref() == Some(&next) {
        return Ok(());
    }
    // Keep subsecond imported/backfilled anchors exactly. Rounding a legacy
    // 23:59:59.9 timestamp through integer seconds must not move cost into tomorrow.
    sqlx::query("INSERT INTO usage_daily_cost_contributions (request_id,usage_id,user_id,api_key_id,api_key_is_standalone,attempt_funds,actual_cost_units,accounting_at) VALUES ($1,$2,$3,$4,$5,$6,$7,to_timestamp($8::double precision)) ON CONFLICT (request_id) DO UPDATE SET attempt_funds=EXCLUDED.attempt_funds,actual_cost_units=EXCLUDED.actual_cost_units,accounting_at=COALESCE(usage_daily_cost_contributions.accounting_at,EXCLUDED.accounting_at),updated_at=now()")
        .bind(&next.request_id).bind(&next.usage_id).bind(&next.user_id).bind(&next.api_key_id)
        .bind(next.api_key_is_standalone).bind(next.attempt_funds)
        .bind(next.actual_cost_units as i64).bind(next.accounting_at_unix_secs.map(|at| at as i64))
        .execute(&mut **tx).await.map_postgres_err()?;
    Ok(())
}

/// Preserve the pending/first-byte batch path: identities need no full audit hydration.
/// Parent request locks are already held, so checking retained identities and
/// inserting missing zero-cost contributions needs two statements per batch.
pub(super) async fn freeze_pending_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    request_ids: &[String],
) -> Result<(), DataLayerError> {
    let reused: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM usage u JOIN usage_daily_cost_contributions d USING (request_id) WHERE u.request_id = ANY($1) AND u.id <> d.usage_id)")
        .bind(request_ids).fetch_one(&mut **tx).await.map_postgres_err()?;
    if reused {
        return Err(DataLayerError::InvalidInput(
            "daily cost request identity was already used by another audit".to_string(),
        ));
    }
    sqlx::query("INSERT INTO usage_daily_cost_contributions (request_id,usage_id,user_id,api_key_id,api_key_is_standalone,attempt_funds,actual_cost_units,accounting_at) SELECT u.request_id,u.id,u.user_id,u.api_key_id,COALESCE(u.request_metadata->>'api_key_is_standalone','false')='true',false,0,NULL FROM usage u WHERE u.request_id=ANY($1) AND u.billing_mode='legacy' AND u.status IN ('pending','streaming') ON CONFLICT (request_id) DO NOTHING")
        .bind(request_ids).execute(&mut **tx).await.map_postgres_err()?;
    Ok(())
}
