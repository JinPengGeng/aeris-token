use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use sqlx::{postgres::PgRow, Row};

use aether_data_contracts::repository::audit::*;
use aether_data_contracts::DataLayerError;

use crate::error::SqlxResultExt;
use crate::PostgresPool;

#[derive(Debug, Clone)]
pub struct PostgresAuditLogReadRepository {
    pool: PostgresPool,
}

impl PostgresAuditLogReadRepository {
    pub fn new(pool: PostgresPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AuditLogWriteRepository for PostgresAuditLogReadRepository {
    async fn create_admin_audit_log(
        &self,
        record: &CreateAdminAuditLog,
    ) -> Result<AuditLogWriteOutcome, DataLayerError> {
        insert_admin_audit_log(&self.pool, record).await
    }

    async fn claim_admin_audit_deliveries(
        &self,
        limit: usize,
        lease_seconds: u64,
    ) -> Result<Vec<ClaimedAdminAuditDelivery>, DataLayerError> {
        if limit == 0 || limit > ADMIN_AUDIT_DELIVERY_MAX_CLAIM {
            return Err(DataLayerError::InvalidInput(format!(
                "admin audit claim limit must be 1..={ADMIN_AUDIT_DELIVERY_MAX_CLAIM}"
            )));
        }
        if lease_seconds == 0 || lease_seconds > ADMIN_AUDIT_DELIVERY_MAX_LEASE_SECONDS {
            return Err(DataLayerError::InvalidInput(format!(
                "admin audit lease must be 1..={ADMIN_AUDIT_DELIVERY_MAX_LEASE_SECONDS} seconds"
            )));
        }
        let lease_token = uuid::Uuid::new_v4();
        let rows = sqlx::query(
            r#"
WITH ready AS (
  SELECT event_id
  FROM admin_audit_delivery
  WHERE (state = 'pending' AND next_attempt_at <= clock_timestamp())
     OR (state = 'leased' AND lease_expires_at <= clock_timestamp())
  ORDER BY next_attempt_at, created_at, event_id
  FOR UPDATE SKIP LOCKED
  LIMIT $1
)
UPDATE admin_audit_delivery AS delivery
SET state = 'leased',
    lease_token = $2,
    lease_expires_at = clock_timestamp() + make_interval(secs => $3::bigint::double precision),
    updated_at = clock_timestamp()
FROM ready
WHERE delivery.event_id = ready.event_id
RETURNING delivery.event_id, delivery.attempt_count
"#,
        )
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .bind(lease_token)
        .bind(i64::try_from(lease_seconds).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_postgres_err()?;
        rows.into_iter()
            .map(|row| {
                Ok(ClaimedAdminAuditDelivery {
                    event_id: row.try_get("event_id").map_postgres_err()?,
                    lease_token,
                    attempt_count: row.try_get("attempt_count").map_postgres_err()?,
                })
            })
            .collect()
    }

    async fn deliver_admin_audit(
        &self,
        event_id: &str,
        lease_token: uuid::Uuid,
    ) -> Result<bool, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let payload: Option<serde_json::Value> = sqlx::query_scalar(
            r#"
SELECT payload
FROM admin_audit_delivery
WHERE event_id = $1
  AND state = 'leased'
  AND lease_token = $2
  AND lease_expires_at > clock_timestamp()
FOR UPDATE
"#,
        )
        .bind(event_id)
        .bind(lease_token)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        let Some(payload) = payload else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        };
        let record: CreateAdminAuditLog = serde_json::from_value(payload).map_err(|_| {
            DataLayerError::InvalidInput("invalid durable admin audit payload".to_string())
        })?;
        if record.id != event_id {
            return Err(DataLayerError::InvalidInput(
                "durable admin audit payload id mismatch".to_string(),
            ));
        }
        insert_admin_audit_log(&mut *tx, &record).await?;
        let acked = sqlx::query(
            r#"
UPDATE admin_audit_delivery
SET state = 'delivered',
    delivered_at = clock_timestamp(),
    lease_token = NULL,
    lease_expires_at = NULL,
    last_error_code = NULL,
    updated_at = clock_timestamp()
WHERE event_id = $1 AND state = 'leased' AND lease_token = $2
  AND lease_expires_at > clock_timestamp()
"#,
        )
        .bind(event_id)
        .bind(lease_token)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?
        .rows_affected()
            == 1;
        if !acked {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        tx.commit().await.map_postgres_err()?;
        Ok(true)
    }

    async fn fail_admin_audit_delivery(
        &self,
        event_id: &str,
        lease_token: uuid::Uuid,
        code: AdminAuditDeliveryFailureCode,
    ) -> Result<AdminAuditDeliveryFailureOutcome, DataLayerError> {
        let row = sqlx::query(
            r#"
UPDATE admin_audit_delivery
SET attempt_count = attempt_count + 1,
    state = CASE
      WHEN attempt_count + 1 >= $3 THEN 'dead_letter'
      ELSE 'pending'
    END,
    next_attempt_at = CASE
      WHEN attempt_count + 1 >= $3 THEN next_attempt_at
      ELSE clock_timestamp() + make_interval(
        secs => LEAST(3600, POWER(2, LEAST(attempt_count + 1, 11))::bigint)
      )
    END,
    dead_lettered_at = CASE
      WHEN attempt_count + 1 >= $3 THEN clock_timestamp()
      ELSE NULL
    END,
    lease_token = NULL,
    lease_expires_at = NULL,
    last_error_code = $4,
    updated_at = clock_timestamp()
WHERE event_id = $1 AND state = 'leased' AND lease_token = $2
  AND lease_expires_at > clock_timestamp()
RETURNING state
"#,
        )
        .bind(event_id)
        .bind(lease_token)
        .bind(ADMIN_AUDIT_DELIVERY_MAX_ATTEMPTS)
        .bind(code.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        let Some(row) = row else {
            return Ok(AdminAuditDeliveryFailureOutcome::StaleLease);
        };
        let state: String = row.try_get("state").map_postgres_err()?;
        match state.as_str() {
            "pending" => Ok(AdminAuditDeliveryFailureOutcome::RetryScheduled),
            "dead_letter" => Ok(AdminAuditDeliveryFailureOutcome::DeadLettered),
            unexpected => Err(DataLayerError::UnexpectedValue(format!(
                "unexpected admin audit delivery state '{unexpected}'"
            ))),
        }
    }

    async fn list_admin_audit_deliveries(
        &self,
        query: &AdminAuditDeliveryListQuery,
    ) -> Result<AdminAuditDeliveryPage, DataLayerError> {
        if query.limit == 0 || query.limit > ADMIN_AUDIT_DELIVERY_MAX_PAGE {
            return Err(DataLayerError::InvalidInput(format!(
                "admin audit delivery page limit must be 1..={ADMIN_AUDIT_DELIVERY_MAX_PAGE}"
            )));
        }
        if query.before_created_at.is_some() != query.before_event_id.is_some() {
            return Err(DataLayerError::InvalidInput(
                "admin audit delivery cursor requires both created_at and event_id".to_string(),
            ));
        }
        let fetch_limit = i64::try_from(query.limit.saturating_add(1)).unwrap_or(i64::MAX);
        let rows = match (
            query.state,
            query.before_created_at.as_ref(),
            query.before_event_id.as_deref(),
        ) {
            (Some(state), Some(before_created_at), Some(before_event_id)) => sqlx::query(
                r#"SELECT event_id,state,attempt_count,next_attempt_at,lease_expires_at,
                          last_error_code,delivered_at,dead_lettered_at,created_at,updated_at
                   FROM admin_audit_delivery
                   WHERE state=$1
                     AND (created_at,event_id) < ($2,$3)
                   ORDER BY created_at DESC,event_id DESC LIMIT $4"#,
            )
            .bind(state.as_str())
            .bind(before_created_at)
            .bind(before_event_id)
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?,
            (Some(state), None, None) => sqlx::query(
                r#"SELECT event_id,state,attempt_count,next_attempt_at,lease_expires_at,
                          last_error_code,delivered_at,dead_lettered_at,created_at,updated_at
                   FROM admin_audit_delivery
                   WHERE state=$1
                   ORDER BY created_at DESC,event_id DESC LIMIT $2"#,
            )
            .bind(state.as_str())
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?,
            (None, Some(before_created_at), Some(before_event_id)) => sqlx::query(
                r#"SELECT event_id,state,attempt_count,next_attempt_at,lease_expires_at,
                          last_error_code,delivered_at,dead_lettered_at,created_at,updated_at
                   FROM admin_audit_delivery
                   WHERE (created_at,event_id) < ($1,$2)
                   ORDER BY created_at DESC,event_id DESC LIMIT $3"#,
            )
            .bind(before_created_at)
            .bind(before_event_id)
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?,
            (None, None, None) => sqlx::query(
                r#"SELECT event_id,state,attempt_count,next_attempt_at,lease_expires_at,
                          last_error_code,delivered_at,dead_lettered_at,created_at,updated_at
                   FROM admin_audit_delivery
                   ORDER BY created_at DESC,event_id DESC LIMIT $1"#,
            )
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?,
            _ => unreachable!("partial admin audit delivery cursor was rejected"),
        };
        let has_more = rows.len() > query.limit;
        let items = rows
            .into_iter()
            .take(query.limit)
            .map(map_delivery_row)
            .collect::<Result<_, _>>()?;
        Ok(AdminAuditDeliveryPage { items, has_more })
    }

    async fn admin_audit_delivery_summary(
        &self,
    ) -> Result<AdminAuditDeliverySummary, DataLayerError> {
        let row = sqlx::query(
            r#"SELECT COUNT(*) FILTER (WHERE state='pending')::bigint pending,
                      COUNT(*) FILTER (WHERE state='leased')::bigint leased,
                      COUNT(*) FILTER (WHERE state='dead_letter')::bigint dead_letter,
                      MIN(created_at) oldest
               FROM admin_audit_delivery
               WHERE state IN ('pending','leased','dead_letter')"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(AdminAuditDeliverySummary {
            pending: row.try_get::<i64, _>("pending").map_postgres_err()?.max(0) as u64,
            leased: row.try_get::<i64, _>("leased").map_postgres_err()?.max(0) as u64,
            dead_letter: row
                .try_get::<i64, _>("dead_letter")
                .map_postgres_err()?
                .max(0) as u64,
            oldest_unresolved_created_at: row.try_get("oldest").map_postgres_err()?,
        })
    }

    async fn redrive_admin_audit_delivery(
        &self,
        event_id: &str,
    ) -> Result<AdminAuditDeliveryRedriveOutcome, DataLayerError> {
        if event_id.trim().is_empty() || event_id.len() > 36 {
            return Err(DataLayerError::InvalidInput(
                "admin audit delivery event_id must be 1..=36 characters".to_string(),
            ));
        }
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let updated = sqlx::query_scalar::<_, String>(
            r#"UPDATE admin_audit_delivery SET state='pending',attempt_count=0,
                      next_attempt_at=clock_timestamp(),lease_token=NULL,lease_expires_at=NULL,
                      last_error_code=NULL,delivered_at=NULL,dead_lettered_at=NULL,
                      updated_at=clock_timestamp()
               WHERE event_id=$1 AND state='dead_letter' RETURNING event_id"#,
        )
        .bind(event_id)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        let outcome = if updated.is_some() {
            AdminAuditDeliveryRedriveOutcome::Redriven
        } else if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM admin_audit_delivery WHERE event_id=$1)",
        )
        .bind(event_id)
        .fetch_one(&mut *tx)
        .await
        .map_postgres_err()?
        {
            AdminAuditDeliveryRedriveOutcome::NotDeadLetter
        } else {
            AdminAuditDeliveryRedriveOutcome::NotFound
        };
        tx.commit().await.map_postgres_err()?;
        Ok(outcome)
    }
}

fn map_delivery_row(row: PgRow) -> Result<StoredAdminAuditDelivery, DataLayerError> {
    let state: String = row.try_get("state").map_postgres_err()?;
    let state = match state.as_str() {
        "pending" => AdminAuditDeliveryState::Pending,
        "leased" => AdminAuditDeliveryState::Leased,
        "delivered" => AdminAuditDeliveryState::Delivered,
        "dead_letter" => AdminAuditDeliveryState::DeadLetter,
        other => {
            return Err(DataLayerError::UnexpectedValue(format!(
                "unexpected admin audit delivery state '{other}'"
            )))
        }
    };
    Ok(StoredAdminAuditDelivery {
        event_id: row.try_get("event_id").map_postgres_err()?,
        state,
        attempt_count: row.try_get("attempt_count").map_postgres_err()?,
        next_attempt_at: row.try_get("next_attempt_at").map_postgres_err()?,
        lease_expires_at: row.try_get("lease_expires_at").map_postgres_err()?,
        last_error_code: row.try_get("last_error_code").map_postgres_err()?,
        delivered_at: row.try_get("delivered_at").map_postgres_err()?,
        dead_lettered_at: row.try_get("dead_lettered_at").map_postgres_err()?,
        created_at: row.try_get("created_at").map_postgres_err()?,
        updated_at: row.try_get("updated_at").map_postgres_err()?,
    })
}

pub(crate) async fn insert_admin_audit_log<'e, E>(
    executor: E,
    record: &CreateAdminAuditLog,
) -> Result<AuditLogWriteOutcome, DataLayerError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    record.validate()?;
    let inserted = sqlx::query(
        r#"
INSERT INTO audit_logs (
  id, event_type, user_id, api_key_id, description, ip_address,
  user_agent, request_id, event_metadata, status_code, error_message, created_at
) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
ON CONFLICT (id) DO NOTHING
"#,
    )
    .bind(&record.id)
    .bind(&record.event_type)
    .bind(record.user_id.as_deref())
    .bind(record.api_key_id.as_deref())
    .bind(&record.description)
    .bind(record.ip_address.as_deref())
    .bind(record.user_agent.as_deref())
    .bind(record.request_id.as_deref())
    .bind(record.event_metadata.clone())
    .bind(record.status_code)
    .bind(record.error_message.as_deref())
    .bind(record.created_at)
    .execute(executor)
    .await
    .map_postgres_err()?
    .rows_affected();
    Ok(if inserted == 1 {
        AuditLogWriteOutcome::Inserted
    } else {
        AuditLogWriteOutcome::AlreadyExists
    })
}

#[async_trait]
impl AuditLogReadRepository for PostgresAuditLogReadRepository {
    async fn list_admin_audit_logs(
        &self,
        query: &AuditLogListQuery,
    ) -> Result<StoredAdminAuditLogPage, DataLayerError> {
        let cutoff_time = postgres_cutoff_time(query.cutoff_unix_secs);
        let total = sqlx::query_scalar::<_, i64>(
            r#"
SELECT COUNT(*)
FROM audit_logs AS a
LEFT JOIN users AS u ON a.user_id = u.id
WHERE a.created_at >= $1
  AND ($2::text IS NULL OR u.username ILIKE $2 ESCAPE '\')
  AND ($3::text IS NULL OR a.event_type = $3)
"#,
        )
        .bind(cutoff_time)
        .bind(query.username_pattern.as_deref())
        .bind(query.event_type.as_deref())
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;

        let mut rows = sqlx::query(
            r#"
SELECT
  a.id,
  a.event_type,
  a.user_id,
  u.email AS user_email,
  u.username AS user_username,
  a.description,
  a.ip_address,
  a.status_code,
  a.error_message,
  a.event_metadata AS metadata,
  a.created_at
FROM audit_logs AS a
LEFT JOIN users AS u ON a.user_id = u.id
WHERE a.created_at >= $1
  AND ($2::text IS NULL OR u.username ILIKE $2 ESCAPE '\')
  AND ($3::text IS NULL OR a.event_type = $3)
ORDER BY a.created_at DESC
LIMIT $4 OFFSET $5
"#,
        )
        .bind(cutoff_time)
        .bind(query.username_pattern.as_deref())
        .bind(query.event_type.as_deref())
        .bind(i64::try_from(query.limit).unwrap_or(i64::MAX))
        .bind(i64::try_from(query.offset).unwrap_or(i64::MAX))
        .fetch(&self.pool);

        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_postgres_admin_audit_log_row(&row)?);
        }

        Ok(StoredAdminAuditLogPage {
            items,
            total: total.max(0) as u64,
        })
    }

    async fn list_admin_suspicious_activities(
        &self,
        cutoff_unix_secs: u64,
    ) -> Result<Vec<StoredSuspiciousActivity>, DataLayerError> {
        let cutoff_time = postgres_cutoff_time(cutoff_unix_secs);
        let mut rows = sqlx::query(
            r#"
SELECT
  id,
  event_type,
  user_id,
  description,
  ip_address,
  event_metadata AS metadata,
  created_at
FROM audit_logs
WHERE created_at >= $1
  AND event_type = ANY($2)
ORDER BY created_at DESC
LIMIT 100
"#,
        )
        .bind(cutoff_time)
        .bind(SUSPICIOUS_EVENT_TYPES.to_vec())
        .fetch(&self.pool);

        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_postgres_suspicious_activity_row(&row)?);
        }
        Ok(items)
    }

    async fn read_admin_user_behavior_event_counts(
        &self,
        user_id: &str,
        cutoff_unix_secs: u64,
    ) -> Result<std::collections::BTreeMap<String, u64>, DataLayerError> {
        let cutoff_time = postgres_cutoff_time(cutoff_unix_secs);
        let mut rows = sqlx::query(
            r#"
SELECT event_type, COUNT(*)::bigint AS count
FROM audit_logs
WHERE user_id = $1
  AND created_at >= $2
GROUP BY event_type
"#,
        )
        .bind(user_id)
        .bind(cutoff_time)
        .fetch(&self.pool);

        let mut counts = std::collections::BTreeMap::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            if let Ok((event_type, count)) = event_count_from_postgres_row(&row) {
                counts.insert(event_type, count);
            }
        }
        Ok(counts)
    }

    async fn list_user_audit_logs(
        &self,
        user_id: &str,
        query: &AuditLogListQuery,
    ) -> Result<StoredUserAuditLogPage, DataLayerError> {
        let cutoff_time = postgres_cutoff_time(query.cutoff_unix_secs);
        let total = sqlx::query_scalar::<_, i64>(
            r#"
SELECT COUNT(*)
FROM audit_logs
WHERE user_id = $1
  AND created_at >= $2
  AND ($3::text IS NULL OR event_type = $3)
"#,
        )
        .bind(user_id)
        .bind(cutoff_time)
        .bind(query.event_type.as_deref())
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;

        let mut rows = sqlx::query(
            r#"
SELECT id, event_type, description, ip_address, status_code, created_at
FROM audit_logs
WHERE user_id = $1
  AND created_at >= $2
  AND ($3::text IS NULL OR event_type = $3)
ORDER BY created_at DESC
LIMIT $4 OFFSET $5
"#,
        )
        .bind(user_id)
        .bind(cutoff_time)
        .bind(query.event_type.as_deref())
        .bind(i64::try_from(query.limit).unwrap_or(i64::MAX))
        .bind(i64::try_from(query.offset).unwrap_or(i64::MAX))
        .fetch(&self.pool);

        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_postgres_user_audit_log_row(&row)?);
        }

        Ok(StoredUserAuditLogPage {
            items,
            total: total.max(0) as u64,
        })
    }

    async fn delete_audit_logs_before(
        &self,
        cutoff_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, DataLayerError> {
        if limit == 0 {
            return Ok(0);
        }

        // Delivery takes its row lock before inserting the canonical audit row.
        // Claim delivered pairs in that same order so cleanup skips a live
        // delivery rather than holding its audit row while waiting on it.
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let cutoff_time = postgres_cutoff_time(cutoff_unix_secs);
        // Take at most one bounded page from each disjoint candidate class,
        // then retain the original global `(created_at, id)` ordering.  The
        // first `limit` rows of that global order must be within those two
        // pages, while `SKIP LOCKED` keeps another cleanup from waiting.
        let requested = i64::try_from(limit).unwrap_or(i64::MAX);
        let delivered_rows: Vec<(String, DateTime<Utc>)> = sqlx::query_as(
            r#"
SELECT delivery.event_id, audit.created_at
FROM admin_audit_delivery AS delivery
JOIN audit_logs AS audit ON audit.id = delivery.event_id
WHERE audit.created_at < $1
  AND delivery.state = 'delivered'
  AND jsonb_typeof(delivery.payload -> 'id') = 'string'
  AND delivery.payload ->> 'id' = delivery.event_id
ORDER BY audit.created_at ASC, audit.id ASC
FOR UPDATE OF delivery, audit SKIP LOCKED
LIMIT $2
"#,
        )
        .bind(cutoff_time)
        .bind(requested)
        .fetch_all(&mut *tx)
        .await
        .map_postgres_err()?;

        let ordinary_rows: Vec<(String, DateTime<Utc>)> = sqlx::query_as(
            r#"
SELECT audit.id, audit.created_at
FROM audit_logs AS audit
WHERE audit.created_at < $1
  AND NOT EXISTS (
      SELECT 1
      FROM admin_audit_delivery AS delivery
      WHERE delivery.event_id = audit.id
  )
ORDER BY audit.created_at ASC, audit.id ASC
FOR UPDATE OF audit SKIP LOCKED
LIMIT $2
"#,
        )
        .bind(cutoff_time)
        .bind(requested)
        .fetch_all(&mut *tx)
        .await
        .map_postgres_err()?;

        let mut candidates = delivered_rows
            .into_iter()
            .map(|(id, created_at)| (created_at, id, true))
            .chain(
                ordinary_rows
                    .into_iter()
                    .map(|(id, created_at)| (created_at, id, false)),
            )
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        candidates.truncate(limit);
        let mut delivered_ids = Vec::new();
        let mut audit_ids = Vec::with_capacity(candidates.len());
        for (_, id, has_delivered_payload) in candidates {
            if has_delivered_payload {
                delivered_ids.push(id.clone());
            }
            audit_ids.push(id);
        }

        if !delivered_ids.is_empty() {
            sqlx::query(
                "DELETE FROM admin_audit_delivery WHERE event_id = ANY($1) AND state = 'delivered'",
            )
            .bind(&delivered_ids)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        }

        if audit_ids.is_empty() {
            tx.commit().await.map_postgres_err()?;
            return Ok(0);
        }
        let deleted = sqlx::query("DELETE FROM audit_logs WHERE id = ANY($1)")
            .bind(&audit_ids)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?
            .rows_affected();
        tx.commit().await.map_postgres_err()?;
        Ok(usize::try_from(deleted).unwrap_or(usize::MAX))
    }
}

fn postgres_cutoff_time(cutoff_unix_secs: u64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(cutoff_unix_secs.min(i64::MAX as u64) as i64, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).expect("unix epoch is valid"))
}

fn postgres_created_at_unix_secs(row: &PgRow) -> Result<u64, DataLayerError> {
    let value = row
        .try_get::<DateTime<Utc>, _>("created_at")
        .map_postgres_err()?;
    Ok(value.timestamp().max(0) as u64)
}

fn map_postgres_admin_audit_log_row(row: &PgRow) -> Result<StoredAdminAuditLog, DataLayerError> {
    Ok(StoredAdminAuditLog {
        id: row.try_get("id").map_postgres_err()?,
        event_type: row.try_get("event_type").map_postgres_err()?,
        user_id: row.try_get("user_id").map_postgres_err()?,
        user_email: row.try_get("user_email").map_postgres_err()?,
        user_username: row.try_get("user_username").map_postgres_err()?,
        description: row.try_get("description").map_postgres_err()?,
        ip_address: row.try_get("ip_address").map_postgres_err()?,
        status_code: row.try_get("status_code").map_postgres_err()?,
        error_message: row.try_get("error_message").map_postgres_err()?,
        metadata: row.try_get("metadata").map_postgres_err()?,
        created_at_unix_secs: postgres_created_at_unix_secs(row)?,
    })
}

fn map_postgres_suspicious_activity_row(
    row: &PgRow,
) -> Result<StoredSuspiciousActivity, DataLayerError> {
    Ok(StoredSuspiciousActivity {
        id: row.try_get("id").map_postgres_err()?,
        event_type: row.try_get("event_type").map_postgres_err()?,
        user_id: row.try_get("user_id").map_postgres_err()?,
        description: row.try_get("description").map_postgres_err()?,
        ip_address: row.try_get("ip_address").map_postgres_err()?,
        metadata: row.try_get("metadata").map_postgres_err()?,
        created_at_unix_secs: postgres_created_at_unix_secs(row)?,
    })
}

fn map_postgres_user_audit_log_row(row: &PgRow) -> Result<StoredUserAuditLog, DataLayerError> {
    Ok(StoredUserAuditLog {
        id: row.try_get("id").map_postgres_err()?,
        event_type: row.try_get("event_type").map_postgres_err()?,
        description: row.try_get("description").map_postgres_err()?,
        ip_address: row.try_get("ip_address").map_postgres_err()?,
        status_code: row.try_get("status_code").map_postgres_err()?,
        created_at_unix_secs: postgres_created_at_unix_secs(row)?,
    })
}

fn event_count_from_postgres_row(row: &PgRow) -> Result<(String, u64), DataLayerError> {
    let event_type = row.try_get("event_type").map_postgres_err()?;
    let count = row.try_get::<i64, _>("count").map_postgres_err()?.max(0) as u64;
    Ok((event_type, count))
}

#[cfg(test)]
mod delivery_tests;

#[cfg(test)]
mod retention_tests;
