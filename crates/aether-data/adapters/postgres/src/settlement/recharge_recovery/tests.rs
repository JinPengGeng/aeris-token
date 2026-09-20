//! Disposable PostgreSQL acceptance: real wallet credit commits enqueue jobs.
//! Usage rows represent evidenced historical liabilities, never caller prices.
use super::*;
use crate::settlement::funding::tests::{fixture, persist_usage, quote};
use crate::SqlxWalletRepository;
use aether_data_contracts::repository::wallet::{
    ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome, WalletWriteRepository,
};
use futures_util::FutureExt;
use serde_json::json;
use sqlx::PgPool;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

pub(super) struct Fixture {
    pub(super) admin: PgPool,
    pub(super) first: PgPool,
    pub(super) second: PgPool,
    pub(super) schema: String,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        let (admin, first, second, schema) = fixture().await;
        for table in [
            "payment_orders",
            "payment_callbacks",
            "recharge_recovery_activation",
            "recharge_recovery_jobs",
            "recharge_recovery_candidates",
            "recharge_recovery_operations",
            "recharge_recovery_notifications",
        ] {
            sqlx::query(&format!(
                "CREATE TABLE {table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&first)
            .await
            .unwrap();
        }
        sqlx::query("INSERT INTO recharge_recovery_activation VALUES(1,true,clock_timestamp()-interval '1 second')")
            .execute(&first).await.unwrap();
        // LIKE does not copy triggers. Install the exact migration function and
        // deferred constraint trigger, changing only their isolated schema.
        let migration =
            include_str!("../../../migrations/20260917060000_add_recharge_debt_recovery.sql");
        let (_, function) = migration.split_once("CREATE OR REPLACE FUNCTION").unwrap();
        let trigger_sql = format!("CREATE OR REPLACE FUNCTION{function}")
            .replace("public.", &format!("{schema}."))
            .replace(
                "search_path = public, pg_temp",
                &format!("search_path = {schema}, pg_temp"),
            );
        sqlx::raw_sql(&trigger_sql).execute(&first).await.unwrap();
        Self {
            admin,
            first,
            second,
            schema,
        }
    }

    pub(super) async fn close(self) {
        self.first.close().await;
        self.second.close().await;
        tokio::time::timeout(
            Duration::from_secs(15),
            sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema)).execute(&self.admin),
        )
        .await
        .expect("isolated schema cleanup timeout")
        .unwrap();
        self.admin.close().await;
    }

    pub(super) fn repo(&self) -> SqlxSettlementRepository {
        SqlxSettlementRepository::new(self.first.clone())
    }
    pub(super) fn other(&self) -> SqlxSettlementRepository {
        SqlxSettlementRepository::new(self.second.clone())
    }
}

pub(super) async fn debt(pool: &PgPool, request: &str, key: &str, usd: f64, age_minutes: i32) {
    let identity = quote(request, key, 1).identity;
    persist_usage(pool, &identity, usd).await;
    sqlx::query("UPDATE usage SET billing_status='insufficient_quota',created_at=clock_timestamp()-make_interval(mins=>$2) WHERE request_id=$1")
        .bind(request).bind(age_minutes).execute(pool).await.unwrap();
}

pub(super) async fn pending_payment(
    pool: &PgPool,
    wallet: &str,
    usd: f64,
) -> ProcessPaymentCallbackInput {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO payment_orders(id,order_no,wallet_id,user_id,amount_usd,pay_amount,pay_currency,payment_method,payment_provider,payment_channel,order_kind,status,created_at,expires_at) VALUES($1,$2,$3,'owner',$4,$4,'USD','stripe','stripe','card','wallet_recharge','pending',clock_timestamp(),clock_timestamp()+interval '1 hour')")
        .bind(&id).bind(format!("order-{id}")).bind(wallet).bind(usd).execute(pool).await.unwrap();
    ProcessPaymentCallbackInput {
        payment_method: "stripe".into(),
        payment_provider: Some("stripe".into()),
        payment_channel: Some("card".into()),
        callback_key: format!("stripe:event-{id}"),
        order_no: Some(format!("order-{id}")),
        gateway_order_id: Some(format!("gateway-{id}")),
        amount_usd: usd,
        pay_amount: Some(usd),
        pay_currency: Some("USD".into()),
        exchange_rate: Some(1.0),
        payload_hash: "synthetic-recovery-credit".into(),
        payload: json!({"status":"success"}),
        signature_valid: true,
    }
}

pub(super) async fn credit(pool: &PgPool, wallet: &str, usd: f64) -> ProcessPaymentCallbackInput {
    let input = pending_payment(pool, wallet, usd).await;
    let result = SqlxWalletRepository::new(pool.clone())
        .process_payment_callback(input.clone())
        .await
        .unwrap();
    assert!(matches!(
        result,
        ProcessPaymentCallbackOutcome::Applied {
            duplicate: false,
            ..
        }
    ));
    input
}

pub(super) async fn jobs(pool: &PgPool) -> Vec<(String, String, i64, i64, i64)> {
    sqlx::query_as("SELECT id,state,principal_cost_units,collected_cost_units,operation_seq FROM recharge_recovery_jobs ORDER BY created_at,id")
        .fetch_all(pool).await.unwrap()
}

pub(super) async fn balances(pool: &PgPool, wallet: &str) -> (i64, i64, i64) {
    sqlx::query_as("SELECT ROUND(balance*100000000)::bigint,ROUND(gift_balance*100000000)::bigint,ROUND(total_consumed*100000000)::bigint FROM wallets WHERE id=$1")
        .bind(wallet).fetch_one(pool).await.unwrap()
}

pub(super) async fn receipt_totals(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT COUNT(*) FROM request_fund_collection_receipts),(SELECT COALESCE(SUM(collected_cost_units),0)::bigint FROM request_fund_collection_receipts),(SELECT COUNT(*) FROM recharge_recovery_operations),(SELECT COUNT(*) FROM wallet_transactions WHERE reason_code='historical_debt_recovery')")
        .fetch_one(pool).await.unwrap()
}

pub(super) async fn run_due(fixture: &Fixture) {
    for _ in 0..8 {
        if fixture
            .repo()
            .process_recharge_recovery_batch(10)
            .await
            .unwrap()
            .is_empty()
        {
            return;
        }
    }
    panic!("bounded fixture jobs did not terminate");
}

#[tokio::test]
#[ignore = "requires disposable AETHER_TEST_DATABASE_URL; real callback credits and concurrent recovery"]
async fn live_recharge_recovery_callback_deduplicates_and_preserves_gifts_holds_and_budgets() {
    let f = Fixture::new().await;
    let result=AssertUnwindSafe(async {
        sqlx::query("UPDATE wallets SET gift_balance=0.03 WHERE id='wallet'").execute(&f.first).await.unwrap();
        debt(&f.first,"historical-debt","key-a",0.20,10).await;
        let hold=quote("unrelated-held-request","key-b",8_000_000);
        let repo=f.repo();
        assert!(matches!(repo.reserve_request_funds(hold.clone()).await.unwrap(),ReserveRequestFundsOutcome::Reserved{..}));
        repo.mark_request_funds_dispatched(hold.identity.clone()).await.unwrap();
        let input=pending_payment(&f.first,"wallet",0.05).await;
        let a=SqlxWalletRepository::new(f.first.clone());
        let b=SqlxWalletRepository::new(f.second.clone());
        let (left,right)=tokio::join!(a.process_payment_callback(input.clone()),b.process_payment_callback(input.clone()));
        let outcomes=[left.unwrap(),right.unwrap()];
        assert_eq!(outcomes.iter().filter(|value|matches!(value,ProcessPaymentCallbackOutcome::Applied{duplicate:false,..})).count(),1);
        assert_eq!(outcomes.iter().filter(|value|matches!(value,ProcessPaymentCallbackOutcome::DuplicateProcessed{..}|ProcessPaymentCallbackOutcome::AlreadyCredited{..})).count(),1);
        assert_eq!(jobs(&f.first).await.len(),1);
        assert_eq!(balances(&f.first,"wallet").await,(15_000_000,3_000_000,0));
        credit(&f.first,"wallet",0.04).await;
        assert_eq!(jobs(&f.first).await.len(),2);
        let other=f.other();
        let (left,right)=tokio::join!(repo.process_recharge_recovery_batch(10),other.process_recharge_recovery_batch(10));
        left.unwrap();right.unwrap();
        run_due(&f).await;
        let summaries=jobs(&f.first).await;
        assert_eq!(summaries.iter().map(|row|row.3).sum::<i64>(),9_000_000);
        assert!(summaries.iter().all(|row|row.1=="waiting_next_recharge"&&row.2==row.3));
        assert_eq!(balances(&f.first,"wallet").await,(10_000_000,3_000_000,9_000_000));
        assert_eq!(receipt_totals(&f.first).await,(2,9_000_000,2,2));
        let held:(String,i64)=sqlx::query_as("SELECT state,SUM(a.reserved_cost_units)::bigint FROM request_fund_reservations r JOIN request_fund_allocations a USING(reservation_token) WHERE r.request_id='unrelated-held-request' GROUP BY state")
            .fetch_one(&f.first).await.unwrap();
        assert_eq!(held,("dispatched".into(),8_000_000));
        let collected:i64=sqlx::query_scalar("SELECT collected_cost_units FROM request_fund_recoveries WHERE request_id='historical-debt'").fetch_one(&f.first).await.unwrap();
        assert_eq!(collected,9_000_000);
        // Duplicate source events and exhausted jobs never reuse the principal.
        a.process_payment_callback(input).await.unwrap();
        assert!(repo.process_recharge_recovery_batch(10).await.unwrap().is_empty());
        assert_eq!(receipt_totals(&f.first).await,(2,9_000_000,2,2));
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; activation, source cutoff and FIFO liabilities"]
async fn live_recharge_recovery_activation_skips_history_and_collects_only_prior_debts_fifo() {
    let f = Fixture::new().await;
    let result=AssertUnwindSafe(async {
        sqlx::query("UPDATE recharge_recovery_activation SET enabled=false WHERE version=1").execute(&f.first).await.unwrap();
        let historical=credit(&f.first,"wallet",0.02).await;
        assert!(jobs(&f.first).await.is_empty());
        sqlx::query("UPDATE recharge_recovery_activation SET enabled=true,activated_at=clock_timestamp() WHERE version=1").execute(&f.first).await.unwrap();
        SqlxWalletRepository::new(f.first.clone()).process_payment_callback(historical).await.unwrap();
        assert!(jobs(&f.first).await.is_empty(),"activation must not replay prior credits");
        debt(&f.first,"debt-old","key-a",0.03,20).await;
        debt(&f.first,"debt-newer","key-a",0.04,10).await;
        credit(&f.first,"wallet",0.10).await;
        debt(&f.first,"debt-after-credit","key-a",0.50,-10).await;
        let first=f.repo().process_recharge_recovery_batch(1).await.unwrap();
        assert_eq!(first.len(),1);
        assert_eq!(first[0].collected_cost_units,3_000_000);
        assert_eq!(first[0].state,"pending");
        let first_request:String=sqlx::query_scalar("SELECT request_id FROM recharge_recovery_operations").fetch_one(&f.first).await.unwrap();
        assert_eq!(first_request,"debt-old");
        run_due(&f).await;
        let order:Vec<String>=sqlx::query_scalar("SELECT request_id FROM recharge_recovery_operations ORDER BY operation_seq").fetch_all(&f.first).await.unwrap();
        assert_eq!(order,vec!["debt-old","debt-newer"]);
        let job=jobs(&f.first).await.remove(0);
        assert_eq!((job.1,job.3),("completed".into(),7_000_000));
        let future:String=sqlx::query_scalar("SELECT billing_status FROM usage WHERE request_id='debt-after-credit'").fetch_one(&f.first).await.unwrap();
        assert_eq!(future,"insufficient_quota");
        assert_eq!(receipt_totals(&f.first).await,(2,7_000_000,2,2));
        assert_eq!(balances(&f.first,"wallet").await,(15_000_000,0,7_000_000));
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

async fn inject_failure(pool: &PgPool) {
    // Fail after the debit/receipt but before its operation commits: every part
    // must roll back, while the source recharge remains committed.
    sqlx::raw_sql("CREATE FUNCTION fail_recovery_operation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic transient write failure'; END $$; CREATE TRIGGER fail_recovery_operation BEFORE INSERT ON recharge_recovery_operations FOR EACH ROW EXECUTE FUNCTION fail_recovery_operation()")
        .execute(pool).await.unwrap();
}

pub(super) async fn due_now(pool: &PgPool, table: &str, id: &str) {
    assert!(matches!(
        table,
        "recharge_recovery_jobs" | "recharge_recovery_notifications"
    ));
    sqlx::query(&format!(
        "UPDATE {table} SET next_attempt_at=clock_timestamp() WHERE id=$1"
    ))
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}

async fn assert_delay(pool: &PgPool, table: &str, id: &str, expected: i64) {
    assert!(matches!(
        table,
        "recharge_recovery_jobs" | "recharge_recovery_notifications"
    ));
    let seconds:i64=sqlx::query_scalar(&format!("SELECT EXTRACT(EPOCH FROM next_attempt_at-clock_timestamp())::bigint FROM {table} WHERE id=$1"))
        .bind(id).fetch_one(pool).await.unwrap();
    assert!(
        (expected - 3..=expected + 1).contains(&seconds),
        "delay={seconds},expected={expected}"
    );
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; rollback, sequence fencing and bounded retry"]
async fn live_recharge_recovery_write_failure_rolls_back_and_sequence_replay_cannot_double_collect()
{
    let f = Fixture::new().await;
    let result=AssertUnwindSafe(async {
        debt(&f.first,"rollback-debt","key-a",0.20,10).await;
        credit(&f.first,"wallet",0.05).await;
        let initial=jobs(&f.first).await.remove(0);
        let payment:String=sqlx::query_scalar("SELECT payment_order_id FROM recharge_recovery_jobs WHERE id=$1").bind(&initial.0).fetch_one(&f.first).await.unwrap();
        inject_failure(&f.first).await;
        let failed=f.repo().process_recharge_recovery_batch(1).await.unwrap().remove(0);
        assert_eq!(failed.state,"retry");assert_eq!(failed.retry_count,1);
        assert_delay(&f.first,"recharge_recovery_jobs",&initial.0,60).await;
        assert_eq!(balances(&f.first,"wallet").await,(15_000_000,0,0));
        assert_eq!(receipt_totals(&f.first).await,(0,0,0,0));
        assert!(f.repo().process_recharge_recovery_batch(1).await.unwrap().is_empty());
        sqlx::query("DROP TRIGGER fail_recovery_operation ON recharge_recovery_operations").execute(&f.first).await.unwrap();
        due_now(&f.first,"recharge_recovery_jobs",&initial.0).await;
        run_due(&f).await;
        assert_eq!(receipt_totals(&f.first).await,(1,5_000_000,1,1));
        // Reexecute the stale selected operation exactly as a worker with a lost
        // commit response would: both its work and failure bookkeeping are fenced.
        let repo=f.repo();
        let id=initial.0.clone();
        let stale=repo.tx_runner.run_read_write(|tx|Box::pin(process_one(tx,id,payment,0))).await.unwrap();
        assert!(stale.is_none());
        let id=initial.0.clone();
        assert!(repo.tx_runner.run_read_write(|tx|Box::pin(record_failure(tx,id,0,false))).await.unwrap().is_none());
        assert_eq!(receipt_totals(&f.first).await,(1,5_000_000,1,1));
        assert_eq!(balances(&f.first,"wallet").await,(10_000_000,0,5_000_000));

        // A separate genuine recharge repeatedly fails; no sleeping or modifying
        // retry_count is needed to prove the full persisted backoff schedule.
        credit(&f.first,"wallet",0.01).await;
        sqlx::query("CREATE TRIGGER fail_recovery_operation BEFORE INSERT ON recharge_recovery_operations FOR EACH ROW EXECUTE FUNCTION fail_recovery_operation()")
            .execute(&f.first).await.unwrap();
        let id=jobs(&f.first).await.into_iter().find(|row|row.1=="pending").unwrap().0;
        for (attempt,delay) in [60,300,1800,7200,21600,86400,0].into_iter().enumerate() {
            let failed=repo.process_recharge_recovery_batch(1).await.unwrap().remove(0);
            assert_eq!(failed.retry_count,attempt as u32+1);
            assert_eq!(failed.state,if attempt==6 {"manual_review"} else {"retry"});
            if delay>0 {assert_delay(&f.first,"recharge_recovery_jobs",&id,delay).await;due_now(&f.first,"recharge_recovery_jobs",&id).await;}
            else {assert!(failed.next_attempt_at_unix_secs.is_none());}
            assert_eq!(receipt_totals(&f.first).await,(1,5_000_000,1,1));
        }
        let audiences:Vec<String>=sqlx::query_scalar("SELECT audience FROM recharge_recovery_notifications WHERE job_id=$1 ORDER BY audience")
            .bind(&id).fetch_all(&f.first).await.unwrap();
        assert_eq!(audiences,vec!["admin","user"]);
        assert_eq!(balances(&f.first,"wallet").await,(11_000_000,0,5_000_000));
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; standalone wallet and changed ownership boundaries"]
async fn live_recharge_recovery_standalone_wallet_does_not_pay_owner_or_foreign_key_debt() {
    let f = Fixture::new().await;
    let result=AssertUnwindSafe(async {
        sqlx::raw_sql("UPDATE api_keys SET is_standalone=true WHERE id='key-a'; INSERT INTO wallets(id,api_key_id,balance,gift_balance,status,limit_mode,created_at,updated_at) VALUES('standalone','key-a',0,0.03,'active','finite',now(),now())")
            .execute(&f.first).await.unwrap();
        debt(&f.first,"standalone-debt","key-a",0.08,10).await;
        debt(&f.first,"owner-debt","key-b",0.10,20).await;
        credit(&f.first,"standalone",0.04).await;
        run_due(&f).await;
        assert_eq!(balances(&f.first,"wallet").await,(10_000_000,0,0));
        assert_eq!(balances(&f.first,"standalone").await,(0,3_000_000,4_000_000));
        let recovered:Vec<String>=sqlx::query_scalar("SELECT request_id FROM request_fund_recoveries").fetch_all(&f.first).await.unwrap();
        assert_eq!(recovered,vec!["standalone-debt"]);
        assert_eq!(f.repo().list_recharge_recovery_jobs_for_user("owner",50).await.unwrap().len(),1);
        assert!(f.repo().list_recharge_recovery_jobs_for_user("other",50).await.unwrap().is_empty());
        credit(&f.first,"standalone",0.04).await;
        sqlx::raw_sql("INSERT INTO users(id,username,email_verified) VALUES('other','other',false); UPDATE api_keys SET user_id='other' WHERE id='key-a'")
            .execute(&f.first).await.unwrap();
        let summary=f.repo().process_recharge_recovery_batch(10).await.unwrap().remove(0);
        assert_eq!(summary.state,"manual_review");
        assert_eq!(summary.error_code.as_deref(),Some("owner_changed"));
        assert_eq!(balances(&f.first,"standalone").await,(4_000_000,3_000_000,4_000_000));
        assert_eq!(receipt_totals(&f.first).await,(1,4_000_000,1,1));
        assert!(f.repo().list_recharge_recovery_jobs_for_user("owner",50).await.unwrap().is_empty());
        assert!(f.repo().list_recharge_recovery_jobs_for_user("other",50).await.unwrap().is_empty());
        let claims=f.repo().claim_recharge_recovery_notifications(10).await.unwrap();
        assert_eq!(claims.len(),1);
        assert_eq!(claims[0].audience,RechargeRecoveryNotificationAudience::Admin);
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; incomplete debt must never appear cleared"]
async fn live_recharge_recovery_retains_known_collection_but_flags_unknown_remaining_debt() {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        debt(&f.first, "known-oldest", "key-a", 0.02, 20).await;
        debt(&f.first, "unverified-later", "key-a", 0.03, 10).await;
        sqlx::query("UPDATE usage SET actual_total_cost_usd=NULL,request_metadata='{}' WHERE request_id='unverified-later'")
            .execute(&f.first).await.unwrap();
        credit(&f.first, "wallet", 0.02).await;
        let summary = f.repo().process_recharge_recovery_batch(1).await.unwrap().remove(0);
        assert_eq!(summary.state, "manual_review");
        assert_eq!(summary.error_code.as_deref(), Some("incomplete_debt_evidence"));
        assert_eq!(summary.collected_cost_units, 2_000_000);
        assert_eq!(receipt_totals(&f.first).await, (1, 2_000_000, 1, 1));
        assert_eq!(balances(&f.first, "wallet").await, (10_000_000, 0, 2_000_000));
        let status: String = sqlx::query_scalar("SELECT billing_status FROM usage WHERE request_id='unverified-later'")
            .fetch_one(&f.first).await.unwrap();
        assert_eq!(status, "insufficient_quota");
        let summaries: Vec<Value> = sqlx::query_scalar("SELECT summary FROM recharge_recovery_notifications")
            .fetch_all(&f.first).await.unwrap();
        assert_eq!(summaries.len(), 2);
        assert!(summaries.iter().all(|value| value["state"] == "manual_review"));
        assert!(f.repo().process_recharge_recovery_batch(1).await.unwrap().is_empty());
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

fn completion(
    claim: &RechargeRecoveryNotification,
    outcome: RechargeRecoveryNotificationOutcome,
) -> CompleteRechargeRecoveryNotificationInput {
    CompleteRechargeRecoveryNotificationInput {
        id: claim.id.clone(),
        lease_token: claim.lease_token,
        outcome,
        error_code: None,
    }
}

async fn notification_state(pool: &PgPool, id: &str) -> (String, i32, bool) {
    sqlx::query_as("SELECT state,attempts,delivered_at IS NOT NULL FROM recharge_recovery_notifications WHERE id=$1")
        .bind(id).fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; notification lease fencing, skip, retry and ACK"]
async fn live_recharge_recovery_notifications_fence_leases_and_count_only_failed_delivery_retries()
{
    let f = Fixture::new().await;
    let result=AssertUnwindSafe(async {
        credit(&f.first,"wallet",0.05).await;
        run_due(&f).await;
        let repo=f.repo();let other=f.other();
        let (left,right)=tokio::join!(repo.claim_recharge_recovery_notifications(10),other.claim_recharge_recovery_notifications(10));
        let mut claims=left.unwrap();claims.extend(right.unwrap());
        assert_eq!(claims.len(),1);
        let first=claims.remove(0);
        assert_eq!(notification_state(&f.first,&first.id).await,("pending".into(),0,false));
        assert!(repo.claim_recharge_recovery_notifications(10).await.unwrap().is_empty());
        assert!(repo.complete_recharge_recovery_notification(completion(&first,RechargeRecoveryNotificationOutcome::Skipped)).await.unwrap());
        assert_eq!(notification_state(&f.first,&first.id).await,("skipped".into(),0,false));
        assert_delay(&f.first,"recharge_recovery_notifications",&first.id,86400).await;
        assert!(!repo.complete_recharge_recovery_notification(completion(&first,RechargeRecoveryNotificationOutcome::Delivered)).await.unwrap());
        due_now(&f.first,"recharge_recovery_notifications",&first.id).await;
        let second=repo.claim_recharge_recovery_notifications(1).await.unwrap().remove(0);
        assert!(second.lease_token>first.lease_token);
        let mut failed=completion(&second,RechargeRecoveryNotificationOutcome::Retry);
        failed.error_code=Some("SQL credentials must never escape".into());
        assert!(repo.complete_recharge_recovery_notification(failed).await.unwrap());
        assert_eq!(notification_state(&f.first,&first.id).await,("retry".into(),1,false));
        assert_delay(&f.first,"recharge_recovery_notifications",&first.id,60).await;
        let code:String=sqlx::query_scalar("SELECT error_code FROM recharge_recovery_notifications WHERE id=$1").bind(&first.id).fetch_one(&f.first).await.unwrap();
        assert_eq!(code,"notification_failed");
        due_now(&f.first,"recharge_recovery_notifications",&first.id).await;
        let expired=repo.claim_recharge_recovery_notifications(1).await.unwrap().remove(0);
        sqlx::query("UPDATE recharge_recovery_notifications SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1")
            .bind(&first.id).execute(&f.first).await.unwrap();
        let current=other.claim_recharge_recovery_notifications(1).await.unwrap().remove(0);
        assert!(current.lease_token>expired.lease_token);
        assert!(!repo.complete_recharge_recovery_notification(completion(&expired,RechargeRecoveryNotificationOutcome::Delivered)).await.unwrap());
        assert!(repo.complete_recharge_recovery_notification(completion(&current,RechargeRecoveryNotificationOutcome::Delivered)).await.unwrap());
        assert!(!repo.complete_recharge_recovery_notification(completion(&current,RechargeRecoveryNotificationOutcome::Delivered)).await.unwrap());
        assert_eq!(notification_state(&f.first,&first.id).await,("delivered".into(),1,true));
        assert!(repo.claim_recharge_recovery_notifications(1).await.unwrap().is_empty());

        credit(&f.first,"wallet",0.01).await;run_due(&f).await;
        for (attempt,delay) in [60,300,1800,7200,21600,86400,0].into_iter().enumerate() {
            let claim=repo.claim_recharge_recovery_notifications(1).await.unwrap().remove(0);
            assert_eq!(notification_state(&f.first,&claim.id).await.1,attempt as i32);
            assert!(repo.complete_recharge_recovery_notification(completion(&claim,RechargeRecoveryNotificationOutcome::Retry)).await.unwrap());
            let state=notification_state(&f.first,&claim.id).await;
            assert_eq!(state.1,attempt as i32+1);assert!(!state.2);
            assert_eq!(state.0,if attempt==6 {"manual_review"} else {"retry"});
            if delay>0 {assert_delay(&f.first,"recharge_recovery_notifications",&claim.id,delay).await;due_now(&f.first,"recharge_recovery_notifications",&claim.id).await;}
        }
        assert!(repo.claim_recharge_recovery_notifications(10).await.unwrap().is_empty());
        assert_eq!(receipt_totals(&f.first).await,(0,0,0,0),"notifications never collect money");
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}
