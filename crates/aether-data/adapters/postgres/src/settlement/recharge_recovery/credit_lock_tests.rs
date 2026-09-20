//! Real standalone-wallet callback/recovery contention at an API-key row lock.
use super::tests::{
    balances, credit, debt, due_now, jobs, pending_payment, receipt_totals, run_due, Fixture,
};
use crate::SqlxWalletRepository;
use aether_data_contracts::repository::settlement::SettlementWriteRepository;
use aether_data_contracts::repository::wallet::{
    ProcessPaymentCallbackOutcome, WalletWriteRepository,
};
use futures_util::FutureExt;
use sqlx::{Connection, PgPool};
use std::panic::AssertUnwindSafe;
use std::time::Duration;

async fn wait_for_callback_key_lock(pool: &PgPool, callback_pid: i32, barrier_pid: i32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity a JOIN pg_locks l ON l.pid=a.pid \
                 WHERE a.pid=$1 AND a.wait_event_type='Lock' AND NOT l.granted \
                 AND a.query ILIKE '%FROM api_keys%' AND $2=ANY(pg_blocking_pids(a.pid)))",
            )
            .bind(callback_pid)
            .bind(barrier_pid)
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("callback must hold wallet and wait on the synthetic API-key barrier");
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; standalone callback and recovery API-key lock barrier"]
async fn live_recharge_recovery_callback_key_contention_rolls_back_then_replays_once() {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        sqlx::raw_sql("UPDATE api_keys SET is_standalone=true WHERE id='key-a'; INSERT INTO wallets(id,api_key_id,balance,gift_balance,status,limit_mode,created_at,updated_at) VALUES('standalone','key-a',0,0.03,'active','finite',now(),now())")
            .execute(&f.first).await.unwrap();
        debt(&f.first, "key-lock-debt", "key-a", 0.08, 10).await;
        let first_credit = credit(&f.first, "standalone", 0.04).await;
        let old_job = jobs(&f.first).await.remove(0).0;
        let second_credit = pending_payment(&f.first, "standalone", 0.04).await;
        let callback_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&f.second).await.unwrap();
        let mut barrier = sqlx::PgConnection::connect_with(&f.first.connect_options()).await.unwrap();
        let barrier_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut barrier).await.unwrap();
        let mut barrier_tx = barrier.begin().await.unwrap();
        sqlx::query("SELECT id FROM api_keys WHERE id='key-a' FOR UPDATE")
            .fetch_one(&mut *barrier_tx).await.unwrap();

        // First collision: recovery can lock its wallet but cannot lock the key.
        // The entire attempted collection must roll back into one durable retry.
        let first_retry = tokio::time::timeout(Duration::from_secs(3), f.repo().process_recharge_recovery_batch(1))
            .await.unwrap().unwrap();
        assert_eq!(first_retry.len(), 1);
        assert_eq!(first_retry[0].state, "retry");
        assert_eq!(first_retry[0].retry_count, 1);
        assert_eq!(first_retry[0].collected_cost_units, 0);
        assert_eq!(balances(&f.first, "standalone").await, (4_000_000, 3_000_000, 0));
        assert_eq!(receipt_totals(&f.first).await, (0, 0, 0, 0));
        let recovery_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM request_fund_recoveries")
            .fetch_one(&f.first).await.unwrap();
        assert_eq!(recovery_count, 0);
        sqlx::query("SAVEPOINT wallet_probe").execute(&mut *barrier_tx).await.unwrap();
        sqlx::query("SELECT id FROM wallets WHERE id='standalone' FOR UPDATE NOWAIT")
            .fetch_one(&mut *barrier_tx).await.expect("failed key lock must release the scoped wallet lock");
        sqlx::query("ROLLBACK TO SAVEPOINT wallet_probe").execute(&mut *barrier_tx).await.unwrap();

        // Second collision: the real new-credit callback holds the same wallet
        // and waits on the key; recovery must roll back without blocking it.
        let callback_repo = SqlxWalletRepository::new(f.second.clone());
        let callback_input = second_credit.clone();
        let callback = tokio::spawn(async move {
            callback_repo.process_payment_callback(callback_input).await.unwrap()
        });
        wait_for_callback_key_lock(&f.admin, callback_pid, barrier_pid).await;
        due_now(&f.first, "recharge_recovery_jobs", &old_job).await;
        let second_retry = tokio::time::timeout(Duration::from_secs(3), f.repo().process_recharge_recovery_batch(1))
            .await.unwrap().unwrap();
        assert_eq!(second_retry.len(), 1);
        assert_eq!(second_retry[0].state, "retry");
        assert_eq!(second_retry[0].retry_count, 2);
        assert_eq!(second_retry[0].collected_cost_units, 0);
        assert_eq!(jobs(&f.first).await.len(), 1, "uncommitted callback cannot mint another budget");
        assert_eq!(balances(&f.first, "standalone").await, (4_000_000, 3_000_000, 0));
        assert_eq!(receipt_totals(&f.first).await, (0, 0, 0, 0));
        assert!(!callback.is_finished(), "callback must still be at the explicit key barrier");

        barrier_tx.commit().await.unwrap();
        assert!(matches!(tokio::time::timeout(Duration::from_secs(10), callback).await.unwrap().unwrap(),
            ProcessPaymentCallbackOutcome::Applied { duplicate: false, .. }));
        assert_eq!(balances(&f.first, "standalone").await, (8_000_000, 3_000_000, 0));
        assert_eq!(jobs(&f.first).await.len(), 2);
        due_now(&f.first, "recharge_recovery_jobs", &old_job).await;
        run_due(&f).await;
        let final_jobs = jobs(&f.first).await;
        assert_eq!(final_jobs.len(), 2);
        assert!(final_jobs.iter().all(|job| job.2 == 4_000_000 && job.3 == 4_000_000));
        assert_eq!(balances(&f.first, "standalone").await, (0, 3_000_000, 8_000_000));
        assert_eq!(receipt_totals(&f.first).await, (2, 8_000_000, 2, 2));
        let callback_repo = SqlxWalletRepository::new(f.first.clone());
        for input in [first_credit, second_credit] {
            assert!(matches!(callback_repo.process_payment_callback(input).await.unwrap(),
                ProcessPaymentCallbackOutcome::DuplicateProcessed { .. } | ProcessPaymentCallbackOutcome::AlreadyCredited { .. }));
        }
        assert!(f.repo().process_recharge_recovery_batch(10).await.unwrap().is_empty());
        assert_eq!(jobs(&f.first).await, final_jobs);
        assert_eq!(balances(&f.first, "standalone").await, (0, 3_000_000, 8_000_000));
        assert_eq!(receipt_totals(&f.first).await, (2, 8_000_000, 2, 2));
        assert_eq!(balances(&f.first, "wallet").await, (10_000_000, 0, 0));
        drop(barrier);
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
