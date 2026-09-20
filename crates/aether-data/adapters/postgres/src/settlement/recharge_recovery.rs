//! Durable, bounded principal-only recovery. Each transaction handles at most one debt.
use aether_data_contracts::repository::settlement::*;
use aether_data_contracts::DataLayerError;
use serde_json::Value;
use sqlx::{postgres::PgRow, Row};

use super::{funding, SqlxSettlementRepository};
use crate::error::SqlxResultExt;
use crate::{PostgresTransaction, PostgresTransactionOptions};

#[cfg(test)]
mod boundary_tests;
#[cfg(test)]
mod credit_lock_tests;
#[cfg(test)]
mod integrity_tests;
#[cfg(test)]
mod native_restore_tests;
#[cfg(test)]
mod refund_tests;
#[cfg(test)]
mod tests;

const JOB_COLUMNS: &str = "j.*, EXTRACT(EPOCH FROM j.created_at)::bigint AS created_secs, EXTRACT(EPOCH FROM j.updated_at)::bigint AS updated_secs, EXTRACT(EPOCH FROM j.next_attempt_at)::bigint AS next_secs";
// Re-resolve the wallet exactly as legacy recovery does. Never transfer a debt to
// an arbitrary current owner or to a user's wallet when a key wallet exists.
const DEBT_FROM: &str = r#"
FROM usage u
JOIN recharge_recovery_candidates candidate ON candidate.request_id=u.request_id AND candidate.job_id=$3
LEFT JOIN usage_settlement_snapshots s ON s.request_id = u.request_id
LEFT JOIN request_fund_recoveries r ON r.request_id = u.request_id
JOIN wallets w ON w.id = $1
WHERE COALESCE(s.billing_status,u.billing_status) = 'insufficient_quota'
AND u.billing_mode = 'legacy'
AND u.user_id IS NOT DISTINCT FROM $2::varchar
AND ((w.api_key_id IS NOT NULL AND u.api_key_id = w.api_key_id)
 OR (w.api_key_id IS NULL AND w.user_id = u.user_id
     AND NOT EXISTS (SELECT 1 FROM wallets kw WHERE kw.api_key_id = u.api_key_id)))
"#;

pub(super) fn transaction_options() -> PostgresTransactionOptions {
    PostgresTransactionOptions {
        statement_timeout_ms: Some(5_000),
        lock_timeout_ms: Some(500),
        ..PostgresTransactionOptions::read_write()
    }
}

fn invalid(message: &str) -> DataLayerError {
    DataLayerError::InvalidInput(message.into())
}
fn safe_error(code: Option<String>) -> Option<String> {
    code.map(|code| {
        if !code.is_empty()
            && code.len() <= 64
            && code
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            code
        } else {
            "notification_failed".into()
        }
    })
}
fn job(row: &PgRow) -> Result<StoredRechargeRecoveryJob, DataLayerError> {
    let unsigned = |name: &str| -> Result<u64, DataLayerError> {
        u64::try_from(row.try_get::<i64, _>(name).map_postgres_err()?)
            .map_err(|_| invalid("negative recovery state"))
    };
    Ok(StoredRechargeRecoveryJob {
        id: row.try_get("id").map_postgres_err()?,
        payment_order_id: row.try_get("payment_order_id").map_postgres_err()?,
        source_transaction_id: row.try_get("source_transaction_id").map_postgres_err()?,
        wallet_id: row.try_get("wallet_id").map_postgres_err()?,
        user_id: row.try_get("user_id").map_postgres_err()?,
        state: row.try_get("state").map_postgres_err()?,
        principal_cost_units: unsigned("principal_cost_units")?,
        collected_cost_units: unsigned("collected_cost_units")?,
        outstanding_cost_units: unsigned("outstanding_cost_units")?,
        available_recharge_cost_units: unsigned("available_recharge_cost_units")?,
        retry_count: u32::try_from(row.try_get::<i32, _>("retry_count").map_postgres_err()?)
            .map_err(|_| invalid("negative retry count"))?,
        next_attempt_at_unix_secs: row
            .try_get::<Option<i64>, _>("next_secs")
            .map_postgres_err()?
            .map(|v| v.max(0) as u64),
        error_code: row.try_get("error_code").map_postgres_err()?,
        created_at_unix_secs: unsigned("created_secs")?,
        updated_at_unix_secs: unsigned("updated_secs")?,
    })
}
async fn read_job(
    tx: &mut PostgresTransaction,
    id: &str,
) -> Result<StoredRechargeRecoveryJob, DataLayerError> {
    let row = sqlx::query(&format!(
        "SELECT {JOB_COLUMNS} FROM recharge_recovery_jobs j WHERE j.id=$1"
    ))
    .bind(id)
    .fetch_one(&mut **tx)
    .await
    .map_postgres_err()?;
    job(&row)
}
async fn enqueue_notifications(
    tx: &mut PostgresTransaction,
    summary: &StoredRechargeRecoveryJob,
) -> Result<(), DataLayerError> {
    if summary.state == "pending" || summary.state == "retry" {
        return Ok(());
    }
    let payload = serde_json::to_value(summary).map_err(|_| invalid("invalid recovery summary"))?;
    // One aggregate notification per recharge, only once the job stops collecting.
    for audience in if summary.state == "manual_review" {
        vec!["user", "admin"]
    } else {
        vec!["user"]
    } {
        sqlx::query("INSERT INTO recharge_recovery_notifications (id,job_id,audience,summary) VALUES ($1,$2,$3,$4) ON CONFLICT(job_id,audience) DO NOTHING")
            .bind(uuid::Uuid::new_v4().to_string()).bind(&summary.id).bind(audience).bind(&payload)
            .execute(&mut **tx).await.map_postgres_err()?;
    }
    Ok(())
}
async fn finish(
    tx: &mut PostgresTransaction,
    id: &str,
    state: &str,
    code: Option<&str>,
) -> Result<StoredRechargeRecoveryJob, DataLayerError> {
    sqlx::query("UPDATE recharge_recovery_jobs SET state=$2,error_code=$3,next_attempt_at=NULL,operation_seq=operation_seq+1,updated_at=clock_timestamp() WHERE id=$1")
        .bind(id).bind(state).bind(code).execute(&mut **tx).await.map_postgres_err()?;
    let summary = read_job(tx, id).await?;
    enqueue_notifications(tx, &summary).await?;
    Ok(summary)
}

impl SqlxSettlementRepository {
    pub(super) async fn process_recharge_jobs(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRechargeRecoveryJob>, DataLayerError> {
        let rows = sqlx::query("SELECT id,payment_order_id,operation_seq FROM recharge_recovery_jobs WHERE state IN ('pending','retry') AND next_attempt_at <= now() ORDER BY created_at,id LIMIT $1")
            .bind(limit.min(100) as i64).fetch_all(self.tx_runner.pool()).await.map_postgres_err()?;
        let mut outcomes = Vec::new();
        for row in rows {
            let id: String = row.try_get("id").map_postgres_err()?;
            let payment: String = row.try_get("payment_order_id").map_postgres_err()?;
            let seq: i64 = row.try_get("operation_seq").map_postgres_err()?;
            let task_id = id.clone();
            let result = self
                .tx_runner
                .run(transaction_options(), |tx| {
                    Box::pin(process_one(tx, task_id, payment, seq))
                })
                .await;
            match result {
                Ok(Some(summary)) => outcomes.push(summary),
                Ok(None) => {}
                Err(error) => {
                    // A commit response may have been lost. The sequence compare
                    // prevents recording a retry (or collecting twice) after commit.
                    let permanent = matches!(error, DataLayerError::InvalidInput(_));
                    let summary = self
                        .tx_runner
                        .run(transaction_options(), |tx| {
                            Box::pin(record_failure(tx, id, seq, permanent))
                        })
                        .await?;
                    if let Some(summary) = summary {
                        outcomes.push(summary);
                    }
                }
            }
        }
        Ok(outcomes)
    }
    pub(super) async fn list_recharge_jobs(
        &self,
        user: &str,
        limit: usize,
    ) -> Result<Vec<StoredRechargeRecoveryJob>, DataLayerError> {
        let rows=sqlx::query(&format!("SELECT {JOB_COLUMNS} FROM recharge_recovery_jobs j JOIN wallets w ON w.id=j.wallet_id LEFT JOIN api_keys k ON k.id=w.api_key_id WHERE j.user_id=$1 AND CASE WHEN w.api_key_id IS NULL THEN w.user_id ELSE k.user_id END=$1 ORDER BY j.created_at DESC,j.id DESC LIMIT $2"))
            .bind(user).bind(limit.min(100) as i64).fetch_all(self.tx_runner.pool()).await.map_postgres_err()?;
        rows.iter().map(job).collect()
    }
}

async fn process_one(
    tx: &mut PostgresTransaction,
    id: String,
    payment: String,
    seq: i64,
) -> Result<Option<StoredRechargeRecoveryJob>, DataLayerError> {
    // Payment precedes wallet as in callbacks/refunds. Scoped recovery then
    // claims wallet/API key without waiting, before validating and collecting.
    let source=sqlx::query("SELECT status,order_kind,refunded_amount_usd::double precision AS refunded,paid_at IS NOT NULL AND credited_at IS NOT NULL AS paid,wallet_id,amount_usd::double precision AS amount FROM payment_orders WHERE id=$1 FOR UPDATE")
        .bind(&payment).fetch_optional(&mut **tx).await.map_postgres_err()?;
    let row=sqlx::query(&format!("SELECT {JOB_COLUMNS},j.operation_seq FROM recharge_recovery_jobs j WHERE j.id=$1 AND j.operation_seq=$2 AND j.state IN ('pending','retry') AND j.next_attempt_at<=now() FOR UPDATE SKIP LOCKED"))
        .bind(&id).bind(seq).fetch_optional(&mut **tx).await.map_postgres_err()?;
    let Some(row) = row else { return Ok(None) };
    let current = job(&row)?;
    let source_valid = match source {
        Some(source) => {
            source.try_get::<String, _>("status").map_postgres_err()? == "credited"
                && source
                    .try_get::<String, _>("order_kind")
                    .map_postgres_err()?
                    == "wallet_recharge"
                && source.try_get::<f64, _>("refunded").map_postgres_err()? == 0.0
                && source.try_get::<bool, _>("paid").map_postgres_err()?
                && source
                    .try_get::<String, _>("wallet_id")
                    .map_postgres_err()?
                    == current.wallet_id
                && request_funds_available_units(source.try_get("amount").map_postgres_err()?)?
                    >= current.principal_cost_units
        }
        None => false,
    };
    if !source_valid {
        return Ok(Some(
            finish(tx, &id, "source_unavailable", Some("source_unavailable")).await?,
        ));
    }
    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM recharge_recovery_activation WHERE version=1")
            .fetch_one(&mut **tx)
            .await
            .map_postgres_err()?;
    if !enabled {
        return Ok(None);
    }
    let owner: Option<String>=sqlx::query_scalar("SELECT CASE WHEN w.api_key_id IS NULL THEN w.user_id ELSE k.user_id END FROM wallets w LEFT JOIN api_keys k ON k.id=w.api_key_id WHERE w.id=$1")
        .bind(&current.wallet_id).fetch_optional(&mut **tx).await.map_postgres_err()?.flatten();
    if owner != current.user_id {
        return Ok(Some(
            finish(tx, &id, "manual_review", Some("owner_changed")).await?,
        ));
    }
    if !candidates_are_accounted_for(tx, &current).await? {
        return Ok(Some(
            finish(tx, &id, "manual_review", Some("candidate_evidence_changed")).await?,
        ));
    }
    let request: Option<String> = sqlx::query_scalar(&format!(
        "SELECT u.request_id {DEBT_FROM} ORDER BY u.created_at,u.request_id LIMIT 1"
    ))
    .bind(&current.wallet_id)
    .bind(&current.user_id)
    .bind(&id)
    .fetch_optional(&mut **tx)
    .await
    .map_postgres_err()?;
    let Some(request) = request else {
        let wallet=sqlx::query("SELECT balance::double precision AS balance,gift_balance::double precision AS gift FROM wallets WHERE id=$1 FOR UPDATE")
            .bind(&current.wallet_id).fetch_one(&mut **tx).await.map_postgres_err()?;
        let (available, _) = funding::wallet_available_units(
            tx,
            &current.wallet_id,
            wallet.try_get("balance").map_postgres_err()?,
            wallet.try_get("gift").map_postgres_err()?,
        )
        .await?;
        sqlx::query("UPDATE recharge_recovery_jobs SET outstanding_cost_units=0,available_recharge_cost_units=$2 WHERE id=$1")
            .bind(&id).bind(available as i64).execute(&mut **tx).await.map_postgres_err()?;
        return Ok(Some(finish(tx, &id, "completed", None).await?));
    };
    let receipt = uuid::Uuid::new_v4().to_string();
    let outcome = funding::recover_scoped(
        tx,
        RecoverInsufficientQuotaInput {
            request_id: request.clone(),
        },
        Some(funding::RechargeRecoveryScope {
            wallet_id: current.wallet_id.clone(),
            user_id: current.user_id.clone(),
            max_cost_units: current.principal_cost_units - current.collected_cost_units,
            receipt_id: receipt.clone(),
        }),
    )
    .await?
    .ok_or_else(|| invalid("recovery wallet or usage missing"))?;
    let receipt_row = sqlx::query("SELECT * FROM request_fund_collection_receipts WHERE id=$1")
        .bind(&receipt)
        .fetch_optional(&mut **tx)
        .await
        .map_postgres_err()?;
    let mut delta = 0_i64;
    if let Some(row) = receipt_row {
        delta = row.try_get("collected_cost_units").map_postgres_err()?;
        let transaction = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO wallet_transactions (id,wallet_id,category,reason_code,amount,balance_before,balance_after,recharge_balance_before,recharge_balance_after,gift_balance_before,gift_balance_after,link_type,link_id,description,created_at) SELECT $1,$2,'adjustment','historical_debt_recovery',-collected_cost_units::numeric/100000000,recharge_before+gift_before,recharge_after+gift_after,recharge_before,recharge_after,gift_before,gift_after,'usage',$3,'Historical usage debt recovery',clock_timestamp() FROM request_fund_collection_receipts WHERE id=$4")
            .bind(&transaction).bind(&current.wallet_id).bind(&request).bind(&receipt).execute(&mut **tx).await.map_postgres_err()?;
        sqlx::query("INSERT INTO recharge_recovery_operations (job_id,operation_seq,request_id,receipt_id,wallet_transaction_id,collected_cost_units) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(&id).bind(seq).bind(&request).bind(&receipt).bind(transaction).bind(delta).execute(&mut **tx).await.map_postgres_err()?;
    }
    // Unknown or contradictory evidence must never become a zero debt total.
    // Keep a successful evidenced debit, but stop this job for reconciliation
    // if another eligible liability cannot be summarized truthfully.
    let outstanding = if candidates_are_accounted_for(tx, &current).await? {
        remaining_debt(tx, &current.wallet_id, &current.user_id, &id).await?
    } else {
        None
    };
    let wallet=sqlx::query("SELECT balance::double precision AS balance,gift_balance::double precision AS gift FROM wallets WHERE id=$1")
        .bind(&current.wallet_id).fetch_one(&mut **tx).await.map_postgres_err()?;
    let (available, _) = funding::wallet_available_units(
        tx,
        &current.wallet_id,
        wallet.try_get("balance").map_postgres_err()?,
        wallet.try_get("gift").map_postgres_err()?,
    )
    .await?;
    let total = current.collected_cost_units + delta as u64;
    let has_remaining: bool = sqlx::query_scalar(&format!("SELECT EXISTS (SELECT 1 {DEBT_FROM})"))
        .bind(&current.wallet_id)
        .bind(&current.user_id)
        .bind(&id)
        .fetch_one(&mut **tx)
        .await
        .map_postgres_err()?;
    let state = if outstanding.is_none() {
        "manual_review"
    } else if !has_remaining {
        "completed"
    } else if (delta == 0 && outcome.outstanding_cost_units > 0)
        || available == 0
        || total == current.principal_cost_units
    {
        "waiting_next_recharge"
    } else {
        "pending"
    };
    sqlx::query("UPDATE recharge_recovery_jobs SET collected_cost_units=$2,outstanding_cost_units=COALESCE($3,outstanding_cost_units),available_recharge_cost_units=$4,state=$5,operation_seq=operation_seq+1,retry_count=0,error_code=CASE WHEN $5='manual_review' THEN 'incomplete_debt_evidence' ELSE NULL END,next_attempt_at=CASE WHEN $5='pending' THEN clock_timestamp() ELSE NULL END,updated_at=clock_timestamp() WHERE id=$1")
        .bind(&id).bind(total as i64).bind(outstanding).bind(available as i64).bind(state).execute(&mut **tx).await.map_postgres_err()?;
    let summary = read_job(tx, &id).await?;
    enqueue_notifications(tx, &summary).await?;
    Ok(Some(summary))
}

async fn candidates_are_accounted_for(
    tx: &mut PostgresTransaction,
    current: &StoredRechargeRecoveryJob,
) -> Result<bool, DataLayerError> {
    sqlx::query_scalar(r#"
SELECT NOT EXISTS (
  SELECT 1 FROM recharge_recovery_candidates c
  LEFT JOIN usage u ON u.request_id=c.request_id
  LEFT JOIN usage_settlement_snapshots s ON s.request_id=c.request_id
  LEFT JOIN request_fund_recoveries r ON r.request_id=c.request_id
  JOIN wallets w ON w.id=$1
  WHERE c.job_id=$3 AND NOT COALESCE(
    CASE WHEN u.request_id IS NULL THEN
      r.wallet_id=$1 AND r.frozen_actual_cost_units=r.prior_entitlement_cost_units+r.collected_cost_units
      AND r.collected_cost_units=(SELECT COALESCE(SUM(collected_cost_units),0) FROM request_fund_collection_receipts WHERE request_id=c.request_id)
    ELSE
      u.billing_mode='legacy' AND u.user_id IS NOT DISTINCT FROM $2::varchar
      AND ((w.api_key_id IS NOT NULL AND u.api_key_id=w.api_key_id)
        OR (w.api_key_id IS NULL AND w.user_id=u.user_id
          AND NOT EXISTS (SELECT 1 FROM wallets kw WHERE kw.api_key_id=u.api_key_id)))
      AND (COALESCE(s.billing_status,u.billing_status)='insufficient_quota'
        OR (COALESCE(s.billing_status,u.billing_status)='settled'
          AND r.wallet_id=$1 AND r.frozen_actual_cost_units=r.prior_entitlement_cost_units+r.collected_cost_units
          AND COALESCE(s.settlement_snapshot,(u.request_metadata->'settlement_snapshot')::jsonb)->>'status'='complete'
          AND CEIL(COALESCE(s.billing_actual_total_cost_usd,u.actual_total_cost_usd)*100000000)=r.frozen_actual_cost_units
          AND r.collected_cost_units=(SELECT COALESCE(SUM(collected_cost_units),0) FROM request_fund_collection_receipts WHERE request_id=c.request_id)))
    END,FALSE)
)
"#).bind(&current.wallet_id).bind(&current.user_id).bind(&current.id)
        .fetch_one(&mut **tx).await.map_postgres_err()
}

async fn remaining_debt(
    tx: &mut PostgresTransaction,
    wallet: &str,
    user: &Option<String>,
    id: &str,
) -> Result<Option<i64>, DataLayerError> {
    sqlx::query_scalar(&format!(r#"
WITH debts AS (
  SELECT COALESCE(s.billing_actual_total_cost_usd,u.actual_total_cost_usd) AS actual,
    COALESCE(s.settlement_snapshot,(u.request_metadata->'settlement_snapshot')::jsonb)->>'status' AS price_status,
    r.frozen_actual_cost_units AS frozen, r.wallet_id AS recorded_wallet,
    COALESCE(r.prior_entitlement_cost_units,(SELECT CEIL(COALESCE(SUM(e.amount_usd),0)*100000000) FROM entitlement_usage_ledgers e WHERE e.request_id=u.request_id)) AS prior,
    COALESCE(r.collected_cost_units,0) AS collected
  {DEBT_FROM}
), amounts AS (
  SELECT *, CEIL(actual*100000000) AS actual_units,
    COALESCE(frozen,CEIL(actual*100000000))-prior-collected AS remaining
  FROM debts
)
SELECT CASE WHEN
  COALESCE(BOOL_AND(COALESCE(price_status='complete' AND actual IS NOT NULL
    AND actual>=0 AND actual<=45035996.27370495
    AND (frozen IS NULL OR frozen=actual_units)
    AND (recorded_wallet IS NULL OR recorded_wallet=$1)
    AND prior>=0 AND collected>=0 AND remaining>=0,FALSE)),TRUE)
  -- The public wallet contract uses JSON integers; never emit an inexact total.
  AND COALESCE(SUM(remaining),0)<=9007199254740991
  THEN COALESCE(SUM(remaining),0)::bigint ELSE NULL END FROM amounts
"#))
        .bind(wallet).bind(user).bind(id).fetch_one(&mut **tx).await.map_postgres_err()
}

async fn record_failure(
    tx: &mut PostgresTransaction,
    id: String,
    seq: i64,
    permanent: bool,
) -> Result<Option<StoredRechargeRecoveryJob>, DataLayerError> {
    let row=sqlx::query("UPDATE recharge_recovery_jobs SET retry_count=retry_count+1,state=CASE WHEN $3 OR retry_count>=6 THEN 'manual_review' ELSE 'retry' END,error_code=CASE WHEN $3 THEN 'evidence_or_owner_invalid' ELSE 'storage_retry' END,next_attempt_at=CASE WHEN $3 OR retry_count>=6 THEN NULL ELSE now()+make_interval(secs=>CASE retry_count WHEN 0 THEN 60 WHEN 1 THEN 300 WHEN 2 THEN 1800 WHEN 3 THEN 7200 WHEN 4 THEN 21600 ELSE 86400 END) END,operation_seq=operation_seq+1,updated_at=clock_timestamp() WHERE id=$1 AND operation_seq=$2 AND state IN ('pending','retry') RETURNING id")
        .bind(&id).bind(seq).bind(permanent).fetch_optional(&mut **tx).await.map_postgres_err()?;
    if row.is_none() {
        return Ok(None);
    }
    let summary = read_job(tx, &id).await?;
    enqueue_notifications(tx, &summary).await?;
    Ok(Some(summary))
}

pub(super) async fn claim_notifications(
    tx: &mut PostgresTransaction,
    limit: usize,
) -> Result<Vec<RechargeRecoveryNotification>, DataLayerError> {
    let rows=sqlx::query("SELECT n.id,n.job_id,n.audience,n.summary,n.lease_token,j.user_id,CASE WHEN w.api_key_id IS NULL THEN w.user_id ELSE k.user_id END AS current_owner FROM recharge_recovery_notifications n JOIN recharge_recovery_jobs j ON j.id=n.job_id JOIN wallets w ON w.id=j.wallet_id LEFT JOIN api_keys k ON k.id=w.api_key_id WHERE n.state IN ('pending','retry','skipped') AND n.next_attempt_at<=now() AND (n.lease_until IS NULL OR n.lease_until<=now()) ORDER BY n.next_attempt_at,n.id LIMIT $1 FOR UPDATE OF n SKIP LOCKED")
        .bind(limit.min(100) as i64).fetch_all(&mut **tx).await.map_postgres_err()?;
    let mut claims = Vec::new();
    for row in rows {
        let id: String = row.try_get("id").map_postgres_err()?;
        let audience: String = row.try_get("audience").map_postgres_err()?;
        let owner: Option<String> = row.try_get("current_owner").map_postgres_err()?;
        let user: Option<String> = row.try_get("user_id").map_postgres_err()?;
        if audience == "user" && (user.is_none() || owner != user) {
            sqlx::query("UPDATE recharge_recovery_notifications SET state='manual_review',error_code='owner_unavailable',lease_until=NULL WHERE id=$1")
                .bind(&id).execute(&mut **tx).await.map_postgres_err()?;
            continue;
        }
        let lease: i64 = row.try_get("lease_token").map_postgres_err()?;
        sqlx::query("UPDATE recharge_recovery_notifications SET lease_token=lease_token+1,lease_until=now()+interval '5 minutes' WHERE id=$1")
            .bind(&id).execute(&mut **tx).await.map_postgres_err()?;
        claims.push(RechargeRecoveryNotification {
            id,
            job_id: row.try_get("job_id").map_postgres_err()?,
            user_id: user,
            audience: if audience == "admin" {
                RechargeRecoveryNotificationAudience::Admin
            } else {
                RechargeRecoveryNotificationAudience::User
            },
            lease_token: lease + 1,
            summary: serde_json::from_value(row.try_get::<Value, _>("summary").map_postgres_err()?)
                .map_err(|_| invalid("invalid notification summary"))?,
        });
    }
    Ok(claims)
}

pub(super) async fn complete_notification(
    tx: &mut PostgresTransaction,
    input: CompleteRechargeRecoveryNotificationInput,
) -> Result<bool, DataLayerError> {
    let state = match input.outcome {
        RechargeRecoveryNotificationOutcome::Delivered => "delivered",
        RechargeRecoveryNotificationOutcome::Skipped => "skipped",
        RechargeRecoveryNotificationOutcome::Retry => "retry",
    };
    let result=sqlx::query("UPDATE recharge_recovery_notifications SET state=CASE WHEN $3='retry' AND attempts>=6 THEN 'manual_review' ELSE $3 END,attempts=attempts+CASE WHEN $3='retry' THEN 1 ELSE 0 END,error_code=$4,delivered_at=CASE WHEN $3='delivered' THEN now() ELSE NULL END,lease_until=NULL,next_attempt_at=now()+make_interval(secs=>CASE WHEN $3='skipped' THEN 86400 ELSE CASE attempts WHEN 0 THEN 60 WHEN 1 THEN 300 WHEN 2 THEN 1800 WHEN 3 THEN 7200 WHEN 4 THEN 21600 ELSE 86400 END END) WHERE id=$1 AND lease_token=$2 AND lease_until>now() AND state IN ('pending','retry','skipped')")
        .bind(input.id).bind(input.lease_token).bind(state).bind(safe_error(input.error_code)).execute(&mut **tx).await.map_postgres_err()?;
    Ok(result.rows_affected() == 1)
}
