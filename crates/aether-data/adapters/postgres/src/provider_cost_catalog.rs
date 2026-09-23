use crate::error::SqlxResultExt;
use aether_data_contracts::repository::provider_cost_catalog::{
    ProviderCostCatalogDeleteOutcome, ProviderCostCatalogListQuery, ProviderCostCatalogRecord,
    ProviderCostCatalogRepository, ProviderCostCatalogUpsertOutcome, ProviderCostTaskType,
};
use aether_data_contracts::DataLayerError;
use async_trait::async_trait;
use sqlx::{PgPool, Row};

#[derive(Debug, Clone)]
pub struct PostgresProviderCostCatalogRepository {
    pool: PgPool,
}

impl PostgresProviderCostCatalogRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ProviderCostCatalogRepository for PostgresProviderCostCatalogRepository {
    async fn upsert_provider_cost_catalog(
        &self,
        record: ProviderCostCatalogRecord,
    ) -> Result<ProviderCostCatalogUpsertOutcome, DataLayerError> {
        record.validate()?;
        let tiered_pricing = record
            .tiered_pricing
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| {
                DataLayerError::UnexpectedValue(format!(
                    "failed to encode provider cost catalog: {error}"
                ))
            })?;
        let upserted: Option<(String, bool)> = sqlx::query_as(
            r#"
INSERT INTO provider_costs (
  cost_id,
  provider_id,
  model,
  task_type,
  currency,
  price_per_request,
  tiered_pricing,
  effective_from_unix_secs,
  effective_to_unix_secs,
  created_by,
  created_at_unix_secs,
  updated_at_unix_secs
) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
ON CONFLICT (cost_id) DO UPDATE SET
  provider_id = EXCLUDED.provider_id,
  model = EXCLUDED.model,
  task_type = EXCLUDED.task_type,
  currency = EXCLUDED.currency,
  price_per_request = EXCLUDED.price_per_request,
  tiered_pricing = EXCLUDED.tiered_pricing,
  effective_from_unix_secs = EXCLUDED.effective_from_unix_secs,
  effective_to_unix_secs = EXCLUDED.effective_to_unix_secs,
  created_by = EXCLUDED.created_by,
  created_at_unix_secs = EXCLUDED.created_at_unix_secs,
  updated_at_unix_secs = EXCLUDED.updated_at_unix_secs
WHERE provider_costs.* IS DISTINCT FROM EXCLUDED.*
RETURNING cost_id, (xmax = 0) AS inserted
"#,
        )
        .bind(&record.cost_id)
        .bind(&record.provider_id)
        .bind(&record.model)
        .bind(record.task_type.as_str())
        .bind(&record.currency)
        .bind(record.price_per_request)
        .bind(tiered_pricing)
        .bind(to_i64(
            record.effective_from_unix_secs,
            "effective_from_unix_secs",
        )?)
        .bind(
            record
                .effective_to_unix_secs
                .map(|value| to_i64(value, "effective_to_unix_secs"))
                .transpose()?,
        )
        .bind(&record.created_by)
        .bind(to_i64(record.created_at_unix_secs, "created_at_unix_secs")?)
        .bind(to_i64(record.updated_at_unix_secs, "updated_at_unix_secs")?)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(match upserted {
            Some((_, true)) => ProviderCostCatalogUpsertOutcome::Inserted,
            _ => ProviderCostCatalogUpsertOutcome::Updated,
        })
    }

    async fn get_provider_cost_catalog(
        &self,
        cost_id: &str,
    ) -> Result<Option<ProviderCostCatalogRecord>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  cost_id,
  provider_id,
  model,
  task_type,
  currency,
  price_per_request,
  tiered_pricing,
  effective_from_unix_secs,
  effective_to_unix_secs,
  created_by,
  created_at_unix_secs,
  updated_at_unix_secs
FROM provider_costs
WHERE cost_id = $1
"#,
        )
        .bind(cost_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        row.map(|row| decode_record(&row)).transpose()
    }

    async fn list_provider_cost_catalogs(
        &self,
        query: &ProviderCostCatalogListQuery,
    ) -> Result<Vec<ProviderCostCatalogRecord>, DataLayerError> {
        let task_type = query.task_type.map(|task_type| task_type.as_str());
        let effective_at = query
            .effective_at_unix_secs
            .map(|value| to_i64(value, "effective_at_unix_secs"))
            .transpose()?;
        let rows = sqlx::query(
            r#"
SELECT
  cost_id,
  provider_id,
  model,
  task_type,
  currency,
  price_per_request,
  tiered_pricing,
  effective_from_unix_secs,
  effective_to_unix_secs,
  created_by,
  created_at_unix_secs,
  updated_at_unix_secs
FROM provider_costs
WHERE ($1::varchar IS NULL OR provider_id = $1)
  AND ($2::varchar IS NULL OR model = $2)
  AND ($3::varchar IS NULL OR task_type = $3)
  AND ($4::bigint IS NULL OR (effective_from_unix_secs <= $4 AND (effective_to_unix_secs IS NULL OR $4 < effective_to_unix_secs)))
ORDER BY provider_id, model, effective_from_unix_secs, cost_id
LIMIT $5 OFFSET $6
"#,
        )
        .bind(query.provider_id.as_deref())
        .bind(query.model.as_deref())
        .bind(task_type)
        .bind(effective_at)
        .bind(i64::try_from(query.limit).unwrap_or(i64::MAX))
        .bind(i64::try_from(query.offset).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_postgres_err()?;
        rows.iter().map(decode_record).collect()
    }

    async fn find_effective_provider_cost_catalog(
        &self,
        provider_id: &str,
        model: &str,
        task_type: ProviderCostTaskType,
        at_unix_secs: u64,
    ) -> Result<Option<ProviderCostCatalogRecord>, DataLayerError> {
        let at_unix_secs = to_i64(at_unix_secs, "at_unix_secs")?;
        let row = sqlx::query(
            r#"
SELECT
  cost_id,
  provider_id,
  model,
  task_type,
  currency,
  price_per_request,
  tiered_pricing,
  effective_from_unix_secs,
  effective_to_unix_secs,
  created_by,
  created_at_unix_secs,
  updated_at_unix_secs
FROM provider_costs
WHERE provider_id = $1
  AND model = $2
  AND task_type = $3
  AND effective_from_unix_secs <= $4
  AND (effective_to_unix_secs IS NULL OR $4 < effective_to_unix_secs)
ORDER BY effective_from_unix_secs DESC, cost_id DESC
LIMIT 1
"#,
        )
        .bind(provider_id)
        .bind(model)
        .bind(task_type.as_str())
        .bind(at_unix_secs)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        row.map(|row| decode_record(&row)).transpose()
    }

    async fn delete_provider_cost_catalog(
        &self,
        cost_id: &str,
    ) -> Result<ProviderCostCatalogDeleteOutcome, DataLayerError> {
        let deleted = sqlx::query("DELETE FROM provider_costs WHERE cost_id = $1")
            .bind(cost_id)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(if deleted.rows_affected() == 0 {
            ProviderCostCatalogDeleteOutcome::NotFound
        } else {
            ProviderCostCatalogDeleteOutcome::Deleted
        })
    }
}

fn decode_record(row: &sqlx::postgres::PgRow) -> Result<ProviderCostCatalogRecord, DataLayerError> {
    let tiered_pricing: Option<serde_json::Value> =
        row.try_get("tiered_pricing").map_postgres_err()?;
    let record = ProviderCostCatalogRecord {
        cost_id: row.try_get("cost_id").map_postgres_err()?,
        provider_id: row.try_get("provider_id").map_postgres_err()?,
        model: row.try_get("model").map_postgres_err()?,
        task_type: ProviderCostTaskType::parse(
            &row.try_get::<String, _>("task_type").map_postgres_err()?,
        )?,
        currency: row.try_get("currency").map_postgres_err()?,
        price_per_request: row.try_get("price_per_request").map_postgres_err()?,
        tiered_pricing,
        effective_from_unix_secs: to_u64(
            row.try_get("effective_from_unix_secs").map_postgres_err()?,
            "effective_from_unix_secs",
        )?,
        effective_to_unix_secs: row
            .try_get::<Option<i64>, _>("effective_to_unix_secs")
            .map_postgres_err()?
            .map(|value| to_u64(value, "effective_to_unix_secs"))
            .transpose()?,
        created_by: row.try_get("created_by").map_postgres_err()?,
        created_at_unix_secs: to_u64(
            row.try_get("created_at_unix_secs").map_postgres_err()?,
            "created_at_unix_secs",
        )?,
        updated_at_unix_secs: to_u64(
            row.try_get("updated_at_unix_secs").map_postgres_err()?,
            "updated_at_unix_secs",
        )?,
    };
    record.validate()?;
    Ok(record)
}

fn to_i64(value: u64, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field} exceeds the integer range")))
}

fn to_u64(value: i64, field: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("{field} must not be negative")))
}

#[cfg(test)]
mod tests;
