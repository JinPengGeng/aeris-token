use async_trait::async_trait;
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};

use aether_data_contracts::repository::provider_cost::{
    ProviderCostCertainty, ProviderCostDimension, ProviderCostImportOutcome, ProviderCostListQuery,
    ProviderCostPrice, ProviderCostReconciliationStatus, ProviderCostRepository,
    ProviderCostSnapshotImport, ProviderCostSourceKind, ProviderCostSummaryQuery, ProviderCostUnit,
    StoredProviderCostSnapshot, StoredProviderCostSummaryRow,
};
use aether_data_contracts::DataLayerError;

use crate::error::SqlxResultExt;

#[derive(Debug, Clone)]
pub struct SqlxProviderCostRepository {
    pool: PgPool,
}

impl SqlxProviderCostRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn to_i64(value: u64, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field} exceeds the integer range")))
}

fn to_u64(value: i64, field: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("{field} must not be negative")))
}

fn map_price(row: &PgRow) -> Result<ProviderCostPrice, DataLayerError> {
    Ok(ProviderCostPrice {
        import_id: row.try_get("import_id").map_postgres_err()?,
        supplier: row.try_get("supplier").map_postgres_err()?,
        provider: row.try_get("provider").map_postgres_err()?,
        model: row.try_get("model").map_postgres_err()?,
        dimension: ProviderCostDimension::parse(
            &row.try_get::<String, _>("dimension").map_postgres_err()?,
        )?,
        currency: row.try_get("currency").map_postgres_err()?,
        unit: ProviderCostUnit::parse(&row.try_get::<String, _>("unit").map_postgres_err()?)?,
        version: row.try_get("version").map_postgres_err()?,
        price_units: to_u64(
            row.try_get("price_units").map_postgres_err()?,
            "price_units",
        )?,
        effective_from_unix_secs: to_u64(
            row.try_get("effective_from_unix_secs").map_postgres_err()?,
            "effective_from_unix_secs",
        )?,
        effective_to_unix_secs: row
            .try_get::<Option<i64>, _>("effective_to_unix_secs")
            .map_postgres_err()?
            .map(|value| to_u64(value, "effective_to_unix_secs"))
            .transpose()?,
        source_reference: row.try_get("source_reference").map_postgres_err()?,
        imported_by: row.try_get("imported_by").map_postgres_err()?,
    })
}

fn map_snapshot(row: &PgRow) -> Result<StoredProviderCostSnapshot, DataLayerError> {
    let import = ProviderCostSnapshotImport {
        import_id: row.try_get("import_id").map_postgres_err()?,
        request_id: row.try_get("request_id").map_postgres_err()?,
        provider: row.try_get("provider").map_postgres_err()?,
        model: row.try_get("model").map_postgres_err()?,
        dimension: ProviderCostDimension::parse(
            &row.try_get::<String, _>("dimension").map_postgres_err()?,
        )?,
        sales_amount_units: to_u64(
            row.try_get("sales_amount_units").map_postgres_err()?,
            "sales_amount_units",
        )?,
        sales_currency: row.try_get("sales_currency").map_postgres_err()?,
        provider_cost_amount_units: row
            .try_get::<Option<i64>, _>("provider_cost_amount_units")
            .map_postgres_err()?
            .map(|value| to_u64(value, "provider_cost_amount_units"))
            .transpose()?,
        provider_currency: row.try_get("provider_currency").map_postgres_err()?,
        certainty: ProviderCostCertainty::parse(
            &row.try_get::<String, _>("certainty").map_postgres_err()?,
        )?,
        source_kind: ProviderCostSourceKind::parse(
            &row.try_get::<String, _>("source_kind").map_postgres_err()?,
        )?,
        reconciliation_status: ProviderCostReconciliationStatus::parse(
            &row.try_get::<String, _>("reconciliation_status")
                .map_postgres_err()?,
        )?,
        price_import_id: row.try_get("price_import_id").map_postgres_err()?,
        price_version: row.try_get("price_version").map_postgres_err()?,
        source_reference: row.try_get("source_reference").map_postgres_err()?,
        price_components: serde_json::from_value(
            row.try_get("price_components").map_postgres_err()?,
        )
        .map_err(|error| {
            DataLayerError::UnexpectedValue(format!(
                "invalid provider cost price components: {error}"
            ))
        })?,
        occurred_at_unix_secs: to_u64(
            row.try_get("occurred_at_unix_secs").map_postgres_err()?,
            "occurred_at_unix_secs",
        )?,
        imported_by: row.try_get("imported_by").map_postgres_err()?,
    };
    import.validate()?;
    Ok(StoredProviderCostSnapshot {
        import,
        imported_at_unix_secs: to_u64(
            row.try_get("imported_at_unix_secs").map_postgres_err()?,
            "imported_at_unix_secs",
        )?,
    })
}

const PRICE_COLUMNS: &str = "import_id, supplier, provider, model, dimension, currency, unit, version, price_units, effective_from_unix_secs, effective_to_unix_secs, source_reference, imported_by";
const SNAPSHOT_COLUMNS: &str = "import_id, request_id, provider, model, dimension, sales_amount_units, sales_currency, provider_cost_amount_units, provider_currency, certainty, source_kind, reconciliation_status, price_import_id, price_version, source_reference, price_components, occurred_at_unix_secs, imported_by, FLOOR(EXTRACT(EPOCH FROM imported_at))::bigint AS imported_at_unix_secs";

async fn lock_import(
    tx: &mut Transaction<'_, Postgres>,
    namespace: &str,
    key: &str,
) -> Result<(), DataLayerError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1 || ':' || $2, 0))")
        .bind(namespace)
        .bind(key)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
    Ok(())
}

fn snapshot_can_advance(current: ProviderCostCertainty, incoming: ProviderCostCertainty) -> bool {
    matches!(
        (current, incoming),
        (
            ProviderCostCertainty::Unknown,
            ProviderCostCertainty::Estimated | ProviderCostCertainty::Known
        ) | (
            ProviderCostCertainty::Estimated,
            ProviderCostCertainty::Known
        )
    )
}

fn snapshot_price_components(
    snapshot: &ProviderCostSnapshotImport,
) -> Result<serde_json::Value, DataLayerError> {
    serde_json::to_value(&snapshot.price_components).map_err(|error| {
        DataLayerError::InvalidInput(format!(
            "provider cost price components are invalid: {error}"
        ))
    })
}

fn component_amount_units(
    price_units: u64,
    quantity: u64,
    unit: ProviderCostUnit,
) -> Result<u64, DataLayerError> {
    let denominator = match unit {
        ProviderCostUnit::PerMillionTokens => 1_000_000_u128,
        ProviderCostUnit::PerImage | ProviderCostUnit::PerRequest => 1,
    };
    let numerator = u128::from(price_units) * u128::from(quantity);
    u64::try_from(numerator.div_ceil(denominator)).map_err(|_| {
        DataLayerError::InvalidInput("provider cost component amount exceeds u64".into())
    })
}

async fn validate_snapshot_price_components(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &ProviderCostSnapshotImport,
    provider_amount: Option<u64>,
    occurred_at: i64,
) -> Result<(), DataLayerError> {
    if snapshot.price_components.is_empty() {
        return Ok(());
    }

    let mut supplier = None;
    let mut total = 0_u64;
    for component in &snapshot.price_components {
        total = total.checked_add(component.amount_units).ok_or_else(|| {
            DataLayerError::InvalidInput("provider cost component total exceeds u64".into())
        })?;
        let (matched_supplier, price_units): (String, i64) = sqlx::query_as(
            r#"SELECT supplier, price_units FROM provider_cost_prices
                WHERE import_id = $1 AND provider = $2 AND model = $3
                  AND dimension = $4 AND currency = $5 AND unit = $6
                  AND version = $7 AND source_reference = $8
                  AND effective_from_unix_secs <= $9
                  AND (effective_to_unix_secs IS NULL OR effective_to_unix_secs > $9)"#,
        )
        .bind(&component.price_import_id)
        .bind(&snapshot.provider)
        .bind(&snapshot.model)
        .bind(component.dimension.as_str())
        .bind(snapshot.provider_currency.as_deref())
        .bind(component.unit.as_str())
        .bind(&component.price_version)
        .bind(&component.price_source_reference)
        .bind(occurred_at)
        .fetch_optional(&mut **tx)
        .await
        .map_postgres_err()?
        .ok_or_else(|| {
            DataLayerError::InvalidInput(
                "provider cost component does not reference an effective matching price".into(),
            )
        })?;
        let expected_amount = component_amount_units(
            to_u64(price_units, "price_units")?,
            component.quantity,
            component.unit,
        )?;
        if snapshot.certainty == ProviderCostCertainty::Estimated
            && component.amount_units != expected_amount
        {
            return Err(DataLayerError::InvalidInput(
                "estimated provider cost component amount does not match price and quantity".into(),
            ));
        }
        if let Some(expected) = &supplier {
            if expected != &matched_supplier {
                return Err(DataLayerError::InvalidInput(
                    "provider cost components must reference one supplier".into(),
                ));
            }
        } else {
            supplier = Some(matched_supplier);
        }
    }
    if snapshot.certainty == ProviderCostCertainty::Estimated && provider_amount != Some(total) {
        return Err(DataLayerError::InvalidInput(
            "provider cost component total must equal provider cost amount".into(),
        ));
    }
    Ok(())
}

async fn insert_snapshot_import(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &ProviderCostSnapshotImport,
    sales_amount: i64,
    provider_amount: Option<i64>,
    occurred_at: i64,
) -> Result<(), DataLayerError> {
    sqlx::query(
        r#"INSERT INTO provider_cost_snapshot_imports (
            import_id, request_id, provider, model, dimension, sales_amount_units,
            sales_currency, provider_cost_amount_units, provider_currency, certainty,
            source_kind, reconciliation_status, price_import_id, price_version, source_reference,
            price_components, occurred_at_unix_secs, imported_by
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)"#,
    )
    .bind(&snapshot.import_id)
    .bind(&snapshot.request_id)
    .bind(&snapshot.provider)
    .bind(&snapshot.model)
    .bind(snapshot.dimension.as_str())
    .bind(sales_amount)
    .bind(&snapshot.sales_currency)
    .bind(provider_amount)
    .bind(&snapshot.provider_currency)
    .bind(snapshot.certainty.as_str())
    .bind(snapshot.source_kind.as_str())
    .bind(snapshot.reconciliation_status.as_str())
    .bind(&snapshot.price_import_id)
    .bind(&snapshot.price_version)
    .bind(&snapshot.source_reference)
    .bind(snapshot_price_components(snapshot)?)
    .bind(occurred_at)
    .bind(&snapshot.imported_by)
    .execute(&mut **tx)
    .await
    .map_postgres_err()?;
    Ok(())
}

#[async_trait]
impl ProviderCostRepository for SqlxProviderCostRepository {
    async fn import_price(
        &self,
        price: &ProviderCostPrice,
    ) -> Result<ProviderCostImportOutcome<ProviderCostPrice>, DataLayerError> {
        price.validate()?;
        let price_units = to_i64(price.price_units, "price_units")?;
        let effective_from = to_i64(price.effective_from_unix_secs, "effective_from_unix_secs")?;
        let effective_to = price
            .effective_to_unix_secs
            .map(|value| to_i64(value, "effective_to_unix_secs"))
            .transpose()?;
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        lock_import(&mut tx, "provider_cost_price_import", &price.import_id).await?;
        let existing_sql =
            format!("SELECT {PRICE_COLUMNS} FROM provider_cost_prices WHERE import_id = $1");
        if let Some(row) = sqlx::query(&existing_sql)
            .bind(&price.import_id)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?
        {
            let existing = map_price(&row)?;
            if existing != *price {
                return Err(DataLayerError::InvalidInput(
                    "provider cost price import_id already has different content".into(),
                ));
            }
            tx.commit().await.map_postgres_err()?;
            return Ok(ProviderCostImportOutcome {
                record: existing,
                inserted: false,
            });
        }

        let identity = format!(
            "{}:{}:{}:{}:{}:{}",
            price.supplier,
            price.provider,
            price.model,
            price.dimension.as_str(),
            price.currency,
            price.unit.as_str()
        );
        lock_import(&mut tx, "provider_cost_price_identity", &identity).await?;
        let overlaps = sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS (
                SELECT 1 FROM provider_cost_prices
                WHERE supplier = $1 AND provider = $2 AND model = $3
                  AND dimension = $4 AND currency = $5 AND unit = $6
                  AND (effective_to_unix_secs IS NULL OR effective_to_unix_secs > $7)
                  AND ($8::bigint IS NULL OR effective_from_unix_secs < $8)
            )"#,
        )
        .bind(&price.supplier)
        .bind(&price.provider)
        .bind(&price.model)
        .bind(price.dimension.as_str())
        .bind(&price.currency)
        .bind(price.unit.as_str())
        .bind(effective_from)
        .bind(effective_to)
        .fetch_one(&mut *tx)
        .await
        .map_postgres_err()?;
        if overlaps {
            return Err(DataLayerError::InvalidInput(
                "provider cost price effective window overlaps an existing version".into(),
            ));
        }

        sqlx::query(
            r#"INSERT INTO provider_cost_prices (
                import_id, supplier, provider, model, dimension, currency, unit, version,
                price_units, effective_from_unix_secs, effective_to_unix_secs,
                source_reference, imported_by
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)"#,
        )
        .bind(&price.import_id)
        .bind(&price.supplier)
        .bind(&price.provider)
        .bind(&price.model)
        .bind(price.dimension.as_str())
        .bind(&price.currency)
        .bind(price.unit.as_str())
        .bind(&price.version)
        .bind(price_units)
        .bind(effective_from)
        .bind(effective_to)
        .bind(&price.source_reference)
        .bind(&price.imported_by)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        tx.commit().await.map_postgres_err()?;
        Ok(ProviderCostImportOutcome {
            record: price.clone(),
            inserted: true,
        })
    }

    async fn find_effective_price(
        &self,
        supplier: &str,
        provider: &str,
        model: &str,
        dimension: ProviderCostDimension,
        currency: &str,
        unit: ProviderCostUnit,
        at_unix_secs: u64,
    ) -> Result<Option<ProviderCostPrice>, DataLayerError> {
        let at = to_i64(at_unix_secs, "at_unix_secs")?;
        let sql = format!(
            "SELECT {PRICE_COLUMNS} FROM provider_cost_prices WHERE supplier=$1 AND provider=$2 AND model=$3 AND dimension=$4 AND currency=$5 AND unit=$6 AND effective_from_unix_secs <= $7 AND (effective_to_unix_secs IS NULL OR effective_to_unix_secs > $7) ORDER BY effective_from_unix_secs DESC LIMIT 1"
        );
        sqlx::query(&sql)
            .bind(supplier)
            .bind(provider)
            .bind(model)
            .bind(dimension.as_str())
            .bind(currency)
            .bind(unit.as_str())
            .bind(at)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?
            .as_ref()
            .map(map_price)
            .transpose()
    }

    async fn list_prices(
        &self,
        query: &ProviderCostListQuery,
    ) -> Result<Vec<ProviderCostPrice>, DataLayerError> {
        let sql = format!(
            "SELECT {PRICE_COLUMNS} FROM provider_cost_prices ORDER BY effective_from_unix_secs DESC, import_id ASC LIMIT $1 OFFSET $2"
        );
        sqlx::query(&sql)
            .bind(i64::from(query.limit.min(500)))
            .bind(i64::from(query.offset))
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?
            .iter()
            .map(map_price)
            .collect()
    }

    async fn import_snapshot(
        &self,
        snapshot: &ProviderCostSnapshotImport,
    ) -> Result<ProviderCostImportOutcome<StoredProviderCostSnapshot>, DataLayerError> {
        snapshot.validate()?;
        let sales_amount = to_i64(snapshot.sales_amount_units, "sales_amount_units")?;
        let provider_amount = snapshot
            .provider_cost_amount_units
            .map(|value| to_i64(value, "provider_cost_amount_units"))
            .transpose()?;
        let occurred_at = to_i64(snapshot.occurred_at_unix_secs, "occurred_at_unix_secs")?;
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        lock_import(
            &mut tx,
            "provider_cost_snapshot_import",
            &snapshot.import_id,
        )
        .await?;
        let existing_sql = format!(
            "SELECT {SNAPSHOT_COLUMNS} FROM provider_cost_snapshot_imports WHERE import_id = $1"
        );
        if let Some(row) = sqlx::query(&existing_sql)
            .bind(&snapshot.import_id)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?
        {
            let existing = map_snapshot(&row)?;
            if existing.import != *snapshot {
                return Err(DataLayerError::InvalidInput(
                    "provider cost snapshot import_id already has different content".into(),
                ));
            }
            tx.commit().await.map_postgres_err()?;
            return Ok(ProviderCostImportOutcome {
                record: existing,
                inserted: false,
            });
        }

        validate_snapshot_price_components(
            &mut tx,
            snapshot,
            snapshot.provider_cost_amount_units,
            occurred_at,
        )
        .await?;

        if snapshot.certainty == ProviderCostCertainty::Estimated
            && snapshot.price_components.is_empty()
        {
            let price_matches = sqlx::query_scalar::<_, bool>(
                r#"SELECT EXISTS (
                    SELECT 1 FROM provider_cost_prices
                    WHERE import_id = $1
                      AND provider = $2
                      AND model = $3
                      AND dimension = $4
                      AND currency = $5
                      AND version = $6
                      AND effective_from_unix_secs <= $7
                      AND (effective_to_unix_secs IS NULL OR effective_to_unix_secs > $7)
                )"#,
            )
            .bind(snapshot.price_import_id.as_deref())
            .bind(&snapshot.provider)
            .bind(&snapshot.model)
            .bind(snapshot.dimension.as_str())
            .bind(snapshot.provider_currency.as_deref())
            .bind(snapshot.price_version.as_deref())
            .bind(occurred_at)
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?;
            if !price_matches {
                return Err(DataLayerError::InvalidInput(
                    "estimated provider cost does not reference an effective matching price".into(),
                ));
            }
        }

        let identity = format!(
            "{}:{}:{}:{}",
            snapshot.request_id,
            snapshot.provider,
            snapshot.model,
            snapshot.dimension.as_str()
        );
        lock_import(&mut tx, "provider_cost_snapshot_identity", &identity).await?;
        let current_sql = format!(
            "SELECT {SNAPSHOT_COLUMNS} FROM provider_cost_snapshots WHERE request_id = $1 AND provider = $2 AND model = $3 AND dimension = $4 FOR UPDATE"
        );
        let current = sqlx::query(&current_sql)
            .bind(&snapshot.request_id)
            .bind(&snapshot.provider)
            .bind(&snapshot.model)
            .bind(snapshot.dimension.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?
            .as_ref()
            .map(map_snapshot)
            .transpose()?;

        let row = if let Some(current) = current {
            if current.import.sales_amount_units != snapshot.sales_amount_units
                || current.import.sales_currency != snapshot.sales_currency
                || current.import.occurred_at_unix_secs != snapshot.occurred_at_unix_secs
            {
                return Err(DataLayerError::InvalidInput(
                    "provider cost snapshot logical identity has immutable sales facts".into(),
                ));
            }
            if !snapshot_can_advance(current.import.certainty, snapshot.certainty) {
                return Err(DataLayerError::InvalidInput(
                    "provider cost snapshot cannot replace the current certainty".into(),
                ));
            }
            sqlx::query(&format!(
                r#"UPDATE provider_cost_snapshots SET
                    import_id = $1, provider_cost_amount_units = $2, provider_currency = $3,
                    certainty = $4, source_kind = $5, reconciliation_status = $6,
                    price_import_id = $7, price_version = $8, source_reference = $9,
                    price_components = $10, imported_by = $11, imported_at = now()
                WHERE request_id = $12 AND provider = $13 AND model = $14 AND dimension = $15
                RETURNING {SNAPSHOT_COLUMNS}"#
            ))
            .bind(&snapshot.import_id)
            .bind(provider_amount)
            .bind(&snapshot.provider_currency)
            .bind(snapshot.certainty.as_str())
            .bind(snapshot.source_kind.as_str())
            .bind(snapshot.reconciliation_status.as_str())
            .bind(&snapshot.price_import_id)
            .bind(&snapshot.price_version)
            .bind(&snapshot.source_reference)
            .bind(snapshot_price_components(snapshot)?)
            .bind(&snapshot.imported_by)
            .bind(&snapshot.request_id)
            .bind(&snapshot.provider)
            .bind(&snapshot.model)
            .bind(snapshot.dimension.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?
        } else {
            sqlx::query(&format!(
                r#"INSERT INTO provider_cost_snapshots (
                    import_id, request_id, provider, model, dimension, sales_amount_units,
                    sales_currency, provider_cost_amount_units, provider_currency, certainty,
                    source_kind, reconciliation_status, price_import_id, price_version, source_reference,
                    price_components, occurred_at_unix_secs, imported_by
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
                RETURNING {SNAPSHOT_COLUMNS}"#
            ))
            .bind(&snapshot.import_id)
            .bind(&snapshot.request_id)
            .bind(&snapshot.provider)
            .bind(&snapshot.model)
            .bind(snapshot.dimension.as_str())
            .bind(sales_amount)
            .bind(&snapshot.sales_currency)
            .bind(provider_amount)
            .bind(&snapshot.provider_currency)
            .bind(snapshot.certainty.as_str())
            .bind(snapshot.source_kind.as_str())
            .bind(snapshot.reconciliation_status.as_str())
            .bind(&snapshot.price_import_id)
            .bind(&snapshot.price_version)
            .bind(&snapshot.source_reference)
            .bind(snapshot_price_components(snapshot)?)
            .bind(occurred_at)
            .bind(&snapshot.imported_by)
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?
        };
        insert_snapshot_import(
            &mut tx,
            snapshot,
            sales_amount,
            provider_amount,
            occurred_at,
        )
        .await?;
        let stored = map_snapshot(&row)?;
        tx.commit().await.map_postgres_err()?;
        Ok(ProviderCostImportOutcome {
            record: stored,
            inserted: true,
        })
    }

    async fn list_snapshots_for_request(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredProviderCostSnapshot>, DataLayerError> {
        let sql = format!(
            "SELECT {SNAPSHOT_COLUMNS} FROM provider_cost_snapshots WHERE request_id = $1 ORDER BY occurred_at_unix_secs ASC, import_id ASC"
        );
        sqlx::query(&sql)
            .bind(request_id)
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?
            .iter()
            .map(map_snapshot)
            .collect()
    }

    async fn summarize_snapshots(
        &self,
        query: &ProviderCostSummaryQuery,
    ) -> Result<Vec<StoredProviderCostSummaryRow>, DataLayerError> {
        if query.occurred_from_unix_secs >= query.occurred_until_unix_secs {
            return Err(DataLayerError::InvalidInput(
                "provider cost summary window is invalid".into(),
            ));
        }
        let rows = sqlx::query(
            r#"SELECT
                sales_currency, provider_currency, certainty, source_kind, reconciliation_status, price_version,
                SUM(sales_amount_units)::bigint AS sales_amount_units,
                SUM(provider_cost_amount_units)::bigint AS provider_cost_amount_units,
                CASE
                    WHEN provider_currency IS NOT NULL AND sales_currency = provider_currency
                    THEN (SUM(sales_amount_units) - SUM(provider_cost_amount_units))::bigint
                    ELSE NULL
                END AS margin_amount_units,
                COUNT(*)::bigint AS snapshot_count,
                COUNT(*) FILTER (WHERE reconciliation_status = 'unreconciled')::bigint AS unreconciled_count,
                COUNT(*) FILTER (WHERE certainty = 'unknown')::bigint AS unknown_count
            FROM provider_cost_snapshots
            WHERE occurred_at_unix_secs >= $1 AND occurred_at_unix_secs < $2
            GROUP BY sales_currency, provider_currency, certainty, source_kind, reconciliation_status, price_version
            ORDER BY sales_currency, provider_currency NULLS FIRST, certainty, source_kind, reconciliation_status, price_version NULLS FIRST"#,
        )
        .bind(to_i64(query.occurred_from_unix_secs, "occurred_from_unix_secs")?)
        .bind(to_i64(query.occurred_until_unix_secs, "occurred_until_unix_secs")?)
        .fetch_all(&self.pool)
        .await
        .map_postgres_err()?;
        rows.iter()
            .map(|row| {
                Ok(StoredProviderCostSummaryRow {
                    sales_currency: row.try_get("sales_currency").map_postgres_err()?,
                    provider_currency: row.try_get("provider_currency").map_postgres_err()?,
                    certainty: ProviderCostCertainty::parse(
                        &row.try_get::<String, _>("certainty").map_postgres_err()?,
                    )?,
                    source_kind: ProviderCostSourceKind::parse(
                        &row.try_get::<String, _>("source_kind").map_postgres_err()?,
                    )?,
                    reconciliation_status: ProviderCostReconciliationStatus::parse(
                        &row.try_get::<String, _>("reconciliation_status")
                            .map_postgres_err()?,
                    )?,
                    price_version: row.try_get("price_version").map_postgres_err()?,
                    sales_amount_units: to_u64(
                        row.try_get("sales_amount_units").map_postgres_err()?,
                        "sales_amount_units",
                    )?,
                    provider_cost_amount_units: row
                        .try_get::<Option<i64>, _>("provider_cost_amount_units")
                        .map_postgres_err()?
                        .map(|value| to_u64(value, "provider_cost_amount_units"))
                        .transpose()?,
                    margin_amount_units: row.try_get("margin_amount_units").map_postgres_err()?,
                    snapshot_count: to_u64(
                        row.try_get("snapshot_count").map_postgres_err()?,
                        "snapshot_count",
                    )?,
                    unreconciled_count: to_u64(
                        row.try_get("unreconciled_count").map_postgres_err()?,
                        "unreconciled_count",
                    )?,
                    unknown_count: to_u64(
                        row.try_get("unknown_count").map_postgres_err()?,
                        "unknown_count",
                    )?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_data_contracts::repository::provider_cost::ProviderCostPriceComponent;

    #[tokio::test]
    #[ignore = "requires an isolated migrated AETHER_TEST_DATABASE_URL"]
    async fn provider_cost_imports_are_idempotent_and_unknown_stays_null() {
        let pool = PgPool::connect(
            &std::env::var("AETHER_TEST_DATABASE_URL")
                .expect("AETHER_TEST_DATABASE_URL must point at the test database"),
        )
        .await
        .unwrap();
        let repository = SqlxProviderCostRepository::new(pool.clone());
        let suffix = uuid::Uuid::new_v4().to_string();
        let import_id = format!("provider-cost-price-{suffix}");
        let price = ProviderCostPrice {
            import_id: import_id.clone(),
            supplier: format!("supplier-{suffix}"),
            provider: "fixture-provider".into(),
            model: "fixture-model".into(),
            dimension: ProviderCostDimension::Input,
            currency: "USD".into(),
            unit: ProviderCostUnit::PerMillionTokens,
            version: "v1".into(),
            price_units: 125_000_000,
            effective_from_unix_secs: 100,
            effective_to_unix_secs: Some(200),
            source_reference: "synthetic-price-book".into(),
            imported_by: "fixture-admin".into(),
        };
        assert!(repository.import_price(&price).await.unwrap().inserted);
        assert!(!repository.import_price(&price).await.unwrap().inserted);
        let mut conflicting = price.clone();
        conflicting.price_units += 1;
        assert!(repository.import_price(&conflicting).await.is_err());
        assert_eq!(
            repository
                .find_effective_price(
                    &price.supplier,
                    &price.provider,
                    &price.model,
                    price.dimension,
                    &price.currency,
                    price.unit,
                    150,
                )
                .await
                .unwrap()
                .unwrap()
                .version,
            "v1"
        );

        let mut price_v2 = price.clone();
        price_v2.import_id = format!("provider-cost-price-v2-{suffix}");
        price_v2.version = "v2".into();
        price_v2.effective_from_unix_secs = 200;
        price_v2.effective_to_unix_secs = Some(300);
        price_v2.price_units = 150_000_000;
        assert!(repository.import_price(&price_v2).await.unwrap().inserted);

        let snapshot_id = format!("provider-cost-snapshot-{suffix}");
        let snapshot = ProviderCostSnapshotImport {
            import_id: snapshot_id.clone(),
            request_id: format!("synthetic-request-{suffix}"),
            provider: price.provider.clone(),
            model: price.model.clone(),
            dimension: ProviderCostDimension::Input,
            sales_amount_units: 50_000_000,
            sales_currency: "USD".into(),
            provider_cost_amount_units: None,
            provider_currency: None,
            certainty: ProviderCostCertainty::Unknown,
            source_kind: ProviderCostSourceKind::ManualImport,
            reconciliation_status: ProviderCostReconciliationStatus::Unreconciled,
            price_import_id: None,
            price_version: None,
            source_reference: None,
            price_components: Vec::new(),
            occurred_at_unix_secs: 150,
            imported_by: "fixture-admin".into(),
        };
        assert!(
            repository
                .import_snapshot(&snapshot)
                .await
                .unwrap()
                .inserted
        );
        assert!(
            !repository
                .import_snapshot(&snapshot)
                .await
                .unwrap()
                .inserted
        );
        let mut conflicting_snapshot = snapshot.clone();
        conflicting_snapshot.sales_amount_units += 1;
        assert!(repository
            .import_snapshot(&conflicting_snapshot)
            .await
            .is_err());
        let persisted_nulls = sqlx::query_as::<
            _,
            (Option<i64>, Option<String>, Option<String>, Option<String>),
        >(
            "SELECT provider_cost_amount_units, provider_currency, price_import_id, price_version FROM provider_cost_snapshots WHERE import_id = $1",
        )
        .bind(&snapshot_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(persisted_nulls, (None, None, None, None));

        let estimated_upgrade = ProviderCostSnapshotImport {
            import_id: format!("provider-cost-upgrade-estimated-{suffix}"),
            request_id: snapshot.request_id.clone(),
            provider: snapshot.provider.clone(),
            model: snapshot.model.clone(),
            dimension: snapshot.dimension,
            sales_amount_units: snapshot.sales_amount_units,
            sales_currency: snapshot.sales_currency.clone(),
            provider_cost_amount_units: Some(20_000_000),
            provider_currency: Some("USD".into()),
            certainty: ProviderCostCertainty::Estimated,
            source_kind: ProviderCostSourceKind::Estimate,
            reconciliation_status: ProviderCostReconciliationStatus::Unreconciled,
            price_import_id: Some(price.import_id.clone()),
            price_version: Some(price.version.clone()),
            source_reference: Some(price.source_reference.clone()),
            price_components: Vec::new(),
            occurred_at_unix_secs: snapshot.occurred_at_unix_secs,
            imported_by: "fixture-admin".into(),
        };
        assert!(
            repository
                .import_snapshot(&estimated_upgrade)
                .await
                .unwrap()
                .inserted
        );

        let known_upgrade = ProviderCostSnapshotImport {
            import_id: format!("provider-cost-upgrade-known-{suffix}"),
            provider_cost_amount_units: Some(25_000_000),
            provider_currency: Some("USD".into()),
            certainty: ProviderCostCertainty::Known,
            source_kind: ProviderCostSourceKind::SupplierBill,
            reconciliation_status: ProviderCostReconciliationStatus::Matched,
            price_import_id: None,
            price_version: None,
            source_reference: Some("synthetic-upgrade-supplier-bill".into()),
            ..estimated_upgrade.clone()
        };
        assert!(
            repository
                .import_snapshot(&known_upgrade)
                .await
                .unwrap()
                .inserted
        );
        let current = repository
            .list_snapshots_for_request(&snapshot.request_id)
            .await
            .unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].import, known_upgrade);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM provider_cost_snapshots WHERE request_id = $1"
            )
            .bind(&snapshot.request_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );
        let replayed_estimate = repository
            .import_snapshot(&estimated_upgrade)
            .await
            .unwrap();
        assert!(!replayed_estimate.inserted);
        assert_eq!(
            replayed_estimate.record.import.certainty,
            ProviderCostCertainty::Estimated
        );
        assert_eq!(
            repository
                .list_snapshots_for_request(&snapshot.request_id)
                .await
                .unwrap()[0]
                .import
                .certainty,
            ProviderCostCertainty::Known
        );
        assert!(
            !repository
                .import_snapshot(&known_upgrade)
                .await
                .unwrap()
                .inserted
        );
        let mut conflicting_invoice = known_upgrade.clone();
        conflicting_invoice.import_id =
            format!("provider-cost-upgrade-conflicting-invoice-{suffix}");
        conflicting_invoice.provider_cost_amount_units = Some(26_000_000);
        assert!(repository
            .import_snapshot(&conflicting_invoice)
            .await
            .is_err());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM provider_cost_snapshot_imports WHERE request_id = $1"
            )
            .bind(&snapshot.request_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            3
        );

        let independent_unknown = ProviderCostSnapshotImport {
            import_id: format!("provider-cost-independent-unknown-{suffix}"),
            request_id: format!("synthetic-unknown-{suffix}"),
            dimension: ProviderCostDimension::CacheRead,
            ..snapshot.clone()
        };
        assert!(
            repository
                .import_snapshot(&independent_unknown)
                .await
                .unwrap()
                .inserted
        );
        let independent_nulls = sqlx::query_as::<
            _,
            (Option<i64>, Option<String>, Option<String>, Option<String>),
        >(
            "SELECT provider_cost_amount_units, provider_currency, price_import_id, price_version FROM provider_cost_snapshots WHERE import_id = $1",
        )
        .bind(&independent_unknown.import_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(independent_nulls, (None, None, None, None));
        let mut backfill_tx = pool.begin().await.unwrap();
        sqlx::query("DELETE FROM provider_cost_snapshot_imports WHERE import_id = $1")
            .bind(&independent_unknown.import_id)
            .execute(&mut *backfill_tx)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../migrations/20260919000000_add_provider_cost_snapshot_imports.sql"
        ))
        .execute(&mut *backfill_tx)
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM provider_cost_snapshot_imports WHERE import_id = $1"
            )
            .bind(&independent_unknown.import_id)
            .fetch_one(&mut *backfill_tx)
            .await
            .unwrap(),
            1
        );
        sqlx::raw_sql(include_str!(
            "../migrations/20260919000000_add_provider_cost_snapshot_imports.sql"
        ))
        .execute(&mut *backfill_tx)
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM provider_cost_snapshot_imports WHERE import_id = $1"
            )
            .bind(&independent_unknown.import_id)
            .fetch_one(&mut *backfill_tx)
            .await
            .unwrap(),
            1
        );
        backfill_tx.rollback().await.unwrap();

        let estimated_v1 = ProviderCostSnapshotImport {
            import_id: format!("provider-cost-estimated-v1-{suffix}"),
            request_id: format!("synthetic-estimated-v1-{suffix}"),
            provider: price.provider.clone(),
            model: price.model.clone(),
            dimension: ProviderCostDimension::Input,
            sales_amount_units: 60_000_000,
            sales_currency: "USD".into(),
            provider_cost_amount_units: Some(30_000_000),
            provider_currency: Some("USD".into()),
            certainty: ProviderCostCertainty::Estimated,
            source_kind: ProviderCostSourceKind::Estimate,
            reconciliation_status: ProviderCostReconciliationStatus::Unreconciled,
            price_import_id: Some(price.import_id.clone()),
            price_version: Some(price.version.clone()),
            source_reference: Some(price.source_reference.clone()),
            price_components: Vec::new(),
            occurred_at_unix_secs: 150,
            imported_by: "fixture-admin".into(),
        };
        repository.import_snapshot(&estimated_v1).await.unwrap();
        let mut missing_price = estimated_v1.clone();
        missing_price.import_id = format!("provider-cost-missing-price-{suffix}");
        missing_price.request_id = format!("synthetic-missing-price-{suffix}");
        missing_price.price_import_id = Some(format!("missing-price-{suffix}"));
        assert!(repository.import_snapshot(&missing_price).await.is_err());
        let mut expired_price = estimated_v1.clone();
        expired_price.import_id = format!("provider-cost-expired-price-{suffix}");
        expired_price.request_id = format!("synthetic-expired-price-{suffix}");
        expired_price.occurred_at_unix_secs = 250;
        assert!(repository.import_snapshot(&expired_price).await.is_err());

        let mut estimated_v2 = estimated_v1.clone();
        estimated_v2.import_id = format!("provider-cost-estimated-v2-{suffix}");
        estimated_v2.request_id = format!("synthetic-estimated-v2-{suffix}");
        estimated_v2.price_import_id = Some(price_v2.import_id.clone());
        estimated_v2.price_version = Some(price_v2.version.clone());
        estimated_v2.source_reference = Some(price_v2.source_reference.clone());
        estimated_v2.occurred_at_unix_secs = 250;
        repository.import_snapshot(&estimated_v2).await.unwrap();

        let known_same_currency = ProviderCostSnapshotImport {
            import_id: format!("provider-cost-known-usd-{suffix}"),
            request_id: format!("synthetic-known-usd-{suffix}"),
            provider: price.provider.clone(),
            model: price.model.clone(),
            dimension: ProviderCostDimension::Output,
            sales_amount_units: 90_000_000,
            sales_currency: "USD".into(),
            provider_cost_amount_units: Some(40_000_000),
            provider_currency: Some("USD".into()),
            certainty: ProviderCostCertainty::Known,
            source_kind: ProviderCostSourceKind::SupplierBill,
            reconciliation_status: ProviderCostReconciliationStatus::Matched,
            price_import_id: None,
            price_version: None,
            source_reference: Some("synthetic-supplier-bill".into()),
            price_components: Vec::new(),
            occurred_at_unix_secs: 175,
            imported_by: "fixture-admin".into(),
        };
        repository
            .import_snapshot(&known_same_currency)
            .await
            .unwrap();
        let mut known_cross_currency = known_same_currency.clone();
        known_cross_currency.import_id = format!("provider-cost-known-eur-{suffix}");
        known_cross_currency.request_id = format!("synthetic-known-eur-{suffix}");
        known_cross_currency.provider_currency = Some("EUR".into());
        repository
            .import_snapshot(&known_cross_currency)
            .await
            .unwrap();

        let summary = repository
            .summarize_snapshots(&ProviderCostSummaryQuery {
                occurred_from_unix_secs: 100,
                occurred_until_unix_secs: 300,
            })
            .await
            .unwrap();
        let estimated_versions: std::collections::BTreeSet<_> = summary
            .iter()
            .filter(|row| row.certainty == ProviderCostCertainty::Estimated)
            .filter_map(|row| row.price_version.as_deref())
            .collect();
        assert_eq!(
            estimated_versions,
            std::collections::BTreeSet::from(["v1", "v2"])
        );
        assert!(summary.iter().any(|row| {
            row.certainty == ProviderCostCertainty::Estimated
                && row.sales_currency == "USD"
                && row.provider_currency.as_deref() == Some("USD")
                && row.margin_amount_units == Some(30_000_000)
        }));
        assert!(summary.iter().any(|row| {
            row.certainty == ProviderCostCertainty::Known
                && row.sales_currency == "USD"
                && row.provider_currency.as_deref() == Some("USD")
                && row.sales_amount_units == 140_000_000
                && row.provider_cost_amount_units == Some(65_000_000)
                && row.margin_amount_units == Some(75_000_000)
                && row.snapshot_count == 2
        }));
        assert!(summary.iter().any(|row| {
            row.certainty == ProviderCostCertainty::Unknown && row.margin_amount_units.is_none()
        }));
        assert!(summary.iter().any(|row| {
            row.sales_currency == "USD"
                && row.provider_currency.as_deref() == Some("EUR")
                && row.margin_amount_units.is_none()
        }));

        let suffix_pattern = format!("%{suffix}");
        sqlx::query("DELETE FROM provider_cost_snapshot_imports WHERE request_id LIKE $1")
            .bind(&suffix_pattern)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM provider_cost_snapshots WHERE request_id LIKE $1")
            .bind(&suffix_pattern)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM provider_cost_prices WHERE import_id IN ($1, $2)")
            .bind(import_id)
            .bind(&price_v2.import_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires an isolated migrated AETHER_TEST_DATABASE_URL"]
    async fn request_aggregate_keeps_component_receipts_and_frozen_prices() {
        let pool = PgPool::connect(
            &std::env::var("AETHER_TEST_DATABASE_URL")
                .expect("AETHER_TEST_DATABASE_URL must point at the test database"),
        )
        .await
        .unwrap();
        let repository = SqlxProviderCostRepository::new(pool.clone());
        let suffix = uuid::Uuid::new_v4().to_string();
        let make_price = |dimension, id: &str, price_units| ProviderCostPrice {
            import_id: format!("provider-cost-component-{id}-{suffix}"),
            supplier: format!("supplier-{suffix}"),
            provider: "component-provider".into(),
            model: "component-model".into(),
            dimension,
            currency: "USD".into(),
            unit: ProviderCostUnit::PerMillionTokens,
            version: "v1".into(),
            price_units,
            effective_from_unix_secs: 100,
            effective_to_unix_secs: Some(200),
            source_reference: "component-price-book".into(),
            imported_by: "fixture-admin".into(),
        };
        let prices = [
            make_price(ProviderCostDimension::Input, "input", 1_000_000),
            make_price(ProviderCostDimension::Output, "output", 2_000_000),
            make_price(ProviderCostDimension::CacheRead, "cache-read", 3_000_000),
        ];
        for price in &prices {
            assert!(repository.import_price(price).await.unwrap().inserted);
        }
        let components = prices
            .iter()
            .zip([10_u64, 20, 30])
            .map(|(price, amount_units)| ProviderCostPriceComponent {
                dimension: price.dimension,
                quantity: 10,
                unit: price.unit,
                price_import_id: price.import_id.clone(),
                price_version: price.version.clone(),
                price_source_reference: price.source_reference.clone(),
                amount_units,
            })
            .collect::<Vec<_>>();
        let aggregate = ProviderCostSnapshotImport {
            import_id: format!("provider-cost-aggregate-{suffix}"),
            request_id: format!("component-request-{suffix}"),
            provider: prices[0].provider.clone(),
            model: prices[0].model.clone(),
            dimension: ProviderCostDimension::Request,
            sales_amount_units: 100,
            sales_currency: "USD".into(),
            provider_cost_amount_units: Some(60),
            provider_currency: Some("USD".into()),
            certainty: ProviderCostCertainty::Estimated,
            source_kind: ProviderCostSourceKind::Estimate,
            reconciliation_status: ProviderCostReconciliationStatus::Unreconciled,
            price_import_id: None,
            price_version: None,
            source_reference: Some("aggregate-price-book".into()),
            price_components: components.clone(),
            occurred_at_unix_secs: 150,
            imported_by: "fixture-admin".into(),
        };
        assert!(
            repository
                .import_snapshot(&aggregate)
                .await
                .unwrap()
                .inserted
        );
        let mut duplicate_dimension = aggregate.clone();
        duplicate_dimension.import_id = format!("provider-cost-aggregate-duplicate-{suffix}");
        duplicate_dimension.request_id = format!("component-duplicate-{suffix}");
        duplicate_dimension.price_components[1].dimension = ProviderCostDimension::Input;
        assert!(repository
            .import_snapshot(&duplicate_dimension)
            .await
            .is_err());
        let mut incorrect_amount = aggregate.clone();
        incorrect_amount.import_id = format!("provider-cost-aggregate-amount-{suffix}");
        incorrect_amount.request_id = format!("component-amount-{suffix}");
        incorrect_amount.price_components[0].amount_units += 1;
        assert!(repository.import_snapshot(&incorrect_amount).await.is_err());
        let mut missing_price = aggregate.clone();
        missing_price.import_id = format!("provider-cost-aggregate-price-{suffix}");
        missing_price.request_id = format!("component-price-{suffix}");
        missing_price.price_components[0].price_version = "missing".into();
        assert!(repository.import_snapshot(&missing_price).await.is_err());

        let mut replacement = prices[1].clone();
        replacement.import_id = format!("provider-cost-component-output-v2-{suffix}");
        replacement.version = "v2".into();
        replacement.effective_from_unix_secs = 200;
        replacement.effective_to_unix_secs = None;
        assert!(
            repository
                .import_price(&replacement)
                .await
                .unwrap()
                .inserted
        );

        let mut known = aggregate.clone();
        known.import_id = format!("provider-cost-aggregate-invoice-{suffix}");
        known.certainty = ProviderCostCertainty::Known;
        known.source_kind = ProviderCostSourceKind::SupplierBill;
        known.reconciliation_status = ProviderCostReconciliationStatus::Matched;
        known.source_reference = Some("supplier-invoice".into());
        known.provider_cost_amount_units = Some(65);
        assert!(repository.import_snapshot(&known).await.unwrap().inserted);
        let replayed = repository.import_snapshot(&aggregate).await.unwrap();
        assert!(!replayed.inserted);
        assert_eq!(replayed.record.import.price_components, components);
        let current = repository
            .list_snapshots_for_request(&aggregate.request_id)
            .await
            .unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].import, known);
        assert_eq!(
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT COUNT(*), SUM(sales_amount_units)::bigint FROM provider_cost_snapshots WHERE request_id = $1"
            )
            .bind(&aggregate.request_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            (1, 100),
            "request aggregate must retain sales once in its current row"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM provider_cost_snapshot_imports WHERE request_id = $1"
            )
            .bind(&aggregate.request_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            2
        );
    }
}
