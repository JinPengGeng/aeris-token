//! Credit authorization follows committed database visibility, never event time.
use super::tests::{balances, credit, jobs, pending_payment, receipt_totals, run_due, Fixture};
use crate::settlement::funding::tests::{persist_usage, quote};
use crate::SqlxWalletRepository;
use aether_data_contracts::repository::wallet::{
    ProcessPaymentCallbackOutcome, WalletWriteRepository,
};
use futures_util::FutureExt;
use sqlx::PgPool;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

async fn committed_debt(pool: &PgPool, request: &str, usd: f64) {
    persist_usage(pool, &quote(request, "key-a", 1).identity, usd).await;
    sqlx::query("UPDATE usage SET billing_status='insufficient_quota' WHERE request_id=$1")
        .bind(request)
        .execute(pool)
        .await
        .unwrap();
}

async fn candidates(pool: &PgPool, job: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT request_id FROM recharge_recovery_candidates WHERE job_id=$1 ORDER BY request_id",
    )
    .bind(job)
    .fetch_all(pool)
    .await
    .unwrap()
}

struct AbortOnDrop(tokio::task::AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; real callback blocked across activation"]
async fn live_recharge_recovery_transaction_started_before_activation_can_credit_after_activation()
{
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        sqlx::query("UPDATE recharge_recovery_activation SET enabled=false WHERE version=1")
            .execute(&f.first).await.unwrap();
        committed_debt(&f.first, "prior-activation-debt", 0.03).await;
        let input = pending_payment(&f.first, "wallet", 0.05).await;
        let first_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&f.first).await.unwrap();
        let mut blocker = f.second.begin().await.unwrap();
        let second_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker).await.unwrap();
        sqlx::query("SELECT id FROM payment_orders WHERE order_no=$1 FOR UPDATE")
            .bind(input.order_no.as_deref()).fetch_one(&mut *blocker).await.unwrap();
        let callback_pool = f.first.clone();
        let callback = tokio::spawn(async move {
            SqlxWalletRepository::new(callback_pool).process_payment_callback(input).await
        });
        let _abort = AbortOnDrop(callback.abort_handle());
        // Observe the actual transaction waiting on our row lock before enabling;
        // no synthetic receipt timestamp or elapsed-time assumption is involved.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT $2=ANY(pg_blocking_pids($1))")
                    .bind(first_pid).bind(second_pid).fetch_one(&f.admin).await.unwrap();
                if blocked { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("real callback did not block on the payment-order lock");
        sqlx::query(&format!("UPDATE {}.recharge_recovery_activation SET enabled=true,activated_at=clock_timestamp() WHERE version=1", f.schema))
            .execute(&f.admin).await.unwrap();
        blocker.commit().await.unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(10), callback)
            .await.unwrap().unwrap().unwrap();
        assert!(matches!(outcome, ProcessPaymentCallbackOutcome::Applied { duplicate: false, .. }));
        let predates_activation: bool = sqlx::query_scalar("SELECT t.created_at<a.activated_at FROM wallet_transactions t CROSS JOIN recharge_recovery_activation a WHERE t.reason_code='topup_gateway'")
            .fetch_one(&f.first).await.unwrap();
        assert!(predates_activation, "NOW() must expose the genuine earlier transaction start");
        let job = jobs(&f.first).await.remove(0);
        assert_eq!(candidates(&f.first, &job.0).await, vec!["prior-activation-debt"]);
        run_due(&f).await;
        assert_eq!(receipt_totals(&f.first).await, (1, 3_000_000, 1, 1));
        assert_eq!(balances(&f.first, "wallet").await, (12_000_000, 0, 3_000_000));
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; immutable debt membership at credit commit"]
async fn live_recharge_recovery_excludes_later_failure_and_backdated_insert_from_old_credit() {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        committed_debt(&f.first, "true-prior-debt", 0.03).await;
        persist_usage(&f.first, &quote("old-pending-request", "key-a", 1).identity, 0.04).await;
        let first_credit = credit(&f.first, "wallet", 0.10).await;
        let first_job = jobs(&f.first).await.remove(0).0;
        assert_eq!(candidates(&f.first, &first_job).await, vec!["true-prior-debt"]);
        // The request was old, but its debt became visible only after this credit.
        sqlx::query("UPDATE usage SET billing_status='insufficient_quota' WHERE request_id='old-pending-request'")
            .execute(&f.first).await.unwrap();
        committed_debt(&f.first, "late-backdated-debt", 0.02).await;
        // Deliberately backdate event metadata to prove it cannot create authority.
        sqlx::query("UPDATE usage SET created_at=clock_timestamp()-interval '1 day' WHERE request_id='late-backdated-debt'")
            .execute(&f.first).await.unwrap();
        SqlxWalletRepository::new(f.first.clone()).process_payment_callback(first_credit).await.unwrap();
        assert_eq!(candidates(&f.first, &first_job).await, vec!["true-prior-debt"], "duplicate callbacks must never augment the snapshot");
        run_due(&f).await;
        assert_eq!(receipt_totals(&f.first).await, (1, 3_000_000, 1, 1));
        let remaining: Vec<String> = sqlx::query_scalar("SELECT request_id FROM usage WHERE billing_status='insufficient_quota' ORDER BY request_id")
            .fetch_all(&f.first).await.unwrap();
        assert_eq!(remaining, vec!["late-backdated-debt", "old-pending-request"]);
        // A later genuine credit can authorize the now-visible failures.
        credit(&f.first, "wallet", 0.10).await;
        let second_job = jobs(&f.first).await.into_iter().find(|row|row.0!=first_job).unwrap().0;
        assert_eq!(candidates(&f.first, &second_job).await, remaining);
        run_due(&f).await;
        let order: Vec<String> = sqlx::query_scalar("SELECT request_id FROM recharge_recovery_operations WHERE job_id=$1 ORDER BY operation_seq")
            .bind(&second_job).fetch_all(&f.first).await.unwrap();
        assert_eq!(order, vec!["late-backdated-debt", "old-pending-request"], "created_at still controls FIFO within authorized candidates");
        assert_eq!(receipt_totals(&f.first).await, (3, 9_000_000, 3, 3));
        assert_eq!(candidates(&f.first, &first_job).await, vec!["true-prior-debt"]);
        assert_eq!(balances(&f.first, "wallet").await, (21_000_000, 0, 9_000_000));
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; a debt transaction commits after credit"]
async fn live_recharge_recovery_excludes_uncommitted_debt_even_when_its_transaction_started_first()
{
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        committed_debt(&f.first, "committed-prior-debt", 0.03).await;
        let mut late = f.second.begin().await.unwrap();
        sqlx::query("INSERT INTO usage(id,request_id,user_id,api_key_id,provider_name,model,status,billing_status,actual_total_cost_usd,total_cost_usd,request_metadata) VALUES('late-uncommitted','late-uncommitted','owner','key-a','test-provider','image','completed','insufficient_quota',0.04,0.04,'{\"settlement_snapshot\":{\"status\":\"complete\"}}'::jsonb)")
            .execute(&mut *late).await.unwrap();
        credit(&f.first, "wallet", 0.10).await;
        let first_job = jobs(&f.first).await.remove(0).0;
        assert_eq!(candidates(&f.first, &first_job).await, vec!["committed-prior-debt"]);
        late.commit().await.unwrap();
        let old_timestamp: bool = sqlx::query_scalar("SELECT u.created_at<t.created_at FROM usage u CROSS JOIN wallet_transactions t WHERE u.request_id='late-uncommitted' AND t.reason_code='topup_gateway'")
            .fetch_one(&f.first).await.unwrap();
        assert!(old_timestamp, "the excluded debt really has an earlier transaction timestamp");
        run_due(&f).await;
        assert_eq!(receipt_totals(&f.first).await, (1, 3_000_000, 1, 1));
        let status: String = sqlx::query_scalar("SELECT billing_status FROM usage WHERE request_id='late-uncommitted'")
            .fetch_one(&f.first).await.unwrap();
        assert_eq!(status, "insufficient_quota");
        credit(&f.first, "wallet", 0.04).await;
        run_due(&f).await;
        assert_eq!(receipt_totals(&f.first).await, (2, 7_000_000, 2, 2));
        assert_eq!(candidates(&f.first, &first_job).await, vec!["committed-prior-debt"]);
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}
