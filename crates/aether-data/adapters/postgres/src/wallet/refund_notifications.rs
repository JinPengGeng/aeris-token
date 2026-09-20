use crate::{error::SqlxResultExt, PostgresTransaction, PostgresTransactionOptions};
use aether_data_contracts::repository::wallet::{
    CompleteRefundStatusNotificationInput, RefundNotificationOutcome, RefundStatusNotification,
};
use aether_data_contracts::DataLayerError;
use sqlx::Row;

#[cfg(test)]
mod tests;

pub(super) fn transaction_options() -> PostgresTransactionOptions {
    PostgresTransactionOptions {
        statement_timeout_ms: Some(5_000),
        lock_timeout_ms: Some(500),
        ..PostgresTransactionOptions::read_write()
    }
}

/// Called before the refund transaction commits, never after a successful response.
/// The stable identifier/unique refund key also fence unknown commit responses.
pub(super) async fn enqueue(
    tx: &mut PostgresTransaction,
    refund_id: &str,
) -> Result<(), DataLayerError> {
    sqlx::query("INSERT INTO refund_status_notifications(id,refund_id,wallet_id,user_id,refund_no,amount_usd,terminal_status,failure_reason) SELECT 'refund:'||id||':'||status,id,wallet_id,user_id,refund_no,amount_usd,status,CASE WHEN status='failed' THEN '退款未完成，请在账户中查看退款详情或联系管理员。' ELSE NULL END FROM refund_requests WHERE id=$1 AND status IN ('succeeded','failed') ON CONFLICT(refund_id) DO NOTHING")
        .bind(refund_id).execute(&mut **tx).await.map_postgres_err()?;
    Ok(())
}

pub(super) async fn claim(
    tx: &mut PostgresTransaction,
    limit: usize,
) -> Result<Vec<RefundStatusNotification>, DataLayerError> {
    let rows=sqlx::query("SELECT n.*,n.amount_usd::text AS amount_text,r.wallet_id AS current_wallet,r.user_id AS refund_owner,r.status AS current_status,w.user_id AS wallet_owner,w.api_key_id FROM refund_status_notifications n JOIN refund_requests r ON r.id=n.refund_id JOIN wallets w ON w.id=n.wallet_id WHERE n.state IN ('pending','retry','skipped') AND n.next_attempt_at<=clock_timestamp() AND (n.lease_until IS NULL OR n.lease_until<=clock_timestamp()) ORDER BY n.next_attempt_at,n.id LIMIT $1 FOR UPDATE OF n SKIP LOCKED")
        .bind(limit.min(100) as i64).fetch_all(&mut **tx).await.map_postgres_err()?;
    let mut claimed = Vec::new();
    for row in rows {
        let id: String = row.try_get("id").map_postgres_err()?;
        let owner: Option<String> = row.try_get("user_id").map_postgres_err()?;
        let wallet: String = row.try_get("wallet_id").map_postgres_err()?;
        let status: String = row.try_get("terminal_status").map_postgres_err()?;
        if owner.is_none()
            || row
                .try_get::<Option<String>, _>("refund_owner")
                .map_postgres_err()?
                != owner
            || row
                .try_get::<Option<String>, _>("wallet_owner")
                .map_postgres_err()?
                != owner
            || row
                .try_get::<Option<String>, _>("api_key_id")
                .map_postgres_err()?
                .is_some()
            || row
                .try_get::<String, _>("current_wallet")
                .map_postgres_err()?
                != wallet
            || row
                .try_get::<String, _>("current_status")
                .map_postgres_err()?
                != status
        {
            sqlx::query("UPDATE refund_status_notifications SET state='manual_review',error_code='refund_owner_or_state_changed',lease_until=NULL WHERE id=$1")
                .bind(&id).execute(&mut **tx).await.map_postgres_err()?;
            continue;
        }
        let lease:i64=sqlx::query_scalar("UPDATE refund_status_notifications SET lease_token=lease_token+1,lease_until=clock_timestamp()+interval '5 minutes' WHERE id=$1 RETURNING lease_token")
            .bind(&id).fetch_one(&mut **tx).await.map_postgres_err()?;
        claimed.push(RefundStatusNotification {
            id,
            refund_id: row.try_get("refund_id").map_postgres_err()?,
            wallet_id: wallet,
            user_id: owner,
            refund_no: row.try_get("refund_no").map_postgres_err()?,
            amount_usd: row.try_get("amount_text").map_postgres_err()?,
            terminal_status: status,
            failure_reason: row.try_get("failure_reason").map_postgres_err()?,
            lease_token: lease,
        });
    }
    Ok(claimed)
}

pub(super) async fn complete(
    tx: &mut PostgresTransaction,
    input: CompleteRefundStatusNotificationInput,
) -> Result<bool, DataLayerError> {
    let (state, code) = match input.outcome {
        RefundNotificationOutcome::Delivered => ("delivered", None),
        RefundNotificationOutcome::Skipped => ("skipped", Some("notification_unavailable")),
        RefundNotificationOutcome::Retry => ("retry", Some("notification_delivery_failed")),
    };
    let result=sqlx::query("UPDATE refund_status_notifications SET state=CASE WHEN $3='retry' AND attempts>=6 THEN 'manual_review' ELSE $3 END,attempts=attempts+CASE WHEN $3='retry' THEN 1 ELSE 0 END,error_code=$4,delivered_at=CASE WHEN $3='delivered' THEN clock_timestamp() ELSE NULL END,lease_until=NULL,next_attempt_at=clock_timestamp()+make_interval(secs=>CASE WHEN $3='skipped' THEN 86400 ELSE CASE attempts WHEN 0 THEN 60 WHEN 1 THEN 300 WHEN 2 THEN 1800 WHEN 3 THEN 7200 WHEN 4 THEN 21600 ELSE 86400 END END) WHERE id=$1 AND lease_token=$2 AND lease_until>clock_timestamp() AND state IN ('pending','retry','skipped')")
        .bind(input.id).bind(input.lease_token).bind(state).bind(code).execute(&mut **tx).await.map_postgres_err()?;
    Ok(result.rows_affected() == 1)
}
