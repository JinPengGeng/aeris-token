//! Lock-order regression against real production refund and recovery methods.
//! Wire beside tests.rs with #[cfg(test)] mod refund_tests.
use super::tests::{balances, credit, debt, due_now, receipt_totals, Fixture};
use crate::SqlxWalletRepository;
use aether_data_contracts::repository::settlement::SettlementWriteRepository;
use aether_data_contracts::repository::wallet::{
    CreateWalletRefundRequestInput, CreateWalletRefundRequestOutcome, FailAdminWalletRefundInput,
    ProcessAdminWalletRefundInput, WalletMutationOutcome, WalletWriteRepository,
};
use futures_util::FutureExt;
use sqlx::{Connection, PgPool};
use std::panic::AssertUnwindSafe;
use std::time::Duration;

async fn wait_for_payment_lock(pool: &PgPool, pid: i32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity a \
                 JOIN pg_locks l ON l.pid=a.pid WHERE a.pid=$1 \
                 AND a.wait_event_type='Lock' AND NOT l.granted \
                 AND a.query ILIKE '%FROM payment_orders%')",
            )
            .bind(pid)
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
    .expect("refund/recovery must reach the payment lock barrier");
}

async fn assert_refund_recovery_lock_order(fail_processing: bool) {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        sqlx::query("UPDATE wallets SET balance=0,gift_balance=0,total_consumed=0,total_refunded=0 WHERE id='wallet'")
            .execute(&f.first).await.unwrap();
        debt(&f.first, "refund-race-debt", "key-a", 0.02, 10).await;
        credit(&f.first, "wallet", 0.10).await;
        let payment: String = sqlx::query_scalar("SELECT payment_order_id FROM recharge_recovery_jobs")
            .fetch_one(&f.first).await.unwrap();
        sqlx::query("INSERT INTO refund_requests(id,refund_no,wallet_id,user_id,payment_order_id,source_type,source_id,refund_mode,amount_usd,status,created_at,updated_at) VALUES('refund-race','refund-race','wallet','owner',$1,'payment_order',$1,'offline_payout',0.03,'approved',clock_timestamp(),clock_timestamp())")
            .bind(&payment).execute(&f.first).await.unwrap();
        let input = ProcessAdminWalletRefundInput {
            wallet_id: "wallet".into(), refund_id: "refund-race".into(), operator_id: None,
        };
        if fail_processing {
            assert!(matches!(SqlxWalletRepository::new(f.first.clone())
                .process_admin_wallet_refund(input.clone()).await.unwrap(), WalletMutationOutcome::Applied(_)));
            assert_eq!(balances(&f.first, "wallet").await, (7_000_000, 0, 0));
        }
        let refund_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&f.first).await.unwrap();
        let recovery_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&f.second).await.unwrap();
        let mut barrier = sqlx::PgConnection::connect_with(&f.first.connect_options()).await.unwrap();
        let mut barrier_tx = barrier.begin().await.unwrap();
        sqlx::query("SELECT id FROM payment_orders WHERE id=$1 FOR UPDATE")
            .bind(&payment).fetch_one(&mut *barrier_tx).await.unwrap();
        let refund_repo = SqlxWalletRepository::new(f.first.clone());
        let refund = tokio::spawn(async move {
            if fail_processing {
                assert!(matches!(refund_repo.fail_admin_wallet_refund(FailAdminWalletRefundInput {
                    wallet_id: "wallet".into(), refund_id: "refund-race".into(),
                    reason: "synthetic offline cancellation".into(), operator_id: None,
                }).await.unwrap(), WalletMutationOutcome::Applied((_, _, Some(_)))));
            } else {
                assert!(matches!(refund_repo.process_admin_wallet_refund(input).await.unwrap(), WalletMutationOutcome::Applied(_)));
            }
        });
        wait_for_payment_lock(&f.admin, refund_pid).await;
        // This is the regression assertion: the old refund code already owned
        // the wallet while waiting for payment and fails NOWAIT deterministically.
        sqlx::query("SAVEPOINT wallet_probe").execute(&mut *barrier_tx).await.unwrap();
        sqlx::query("SELECT id FROM wallets WHERE id='wallet' FOR UPDATE NOWAIT")
            .fetch_one(&mut *barrier_tx).await.expect("payment waiter must not own wallet");
        sqlx::query("ROLLBACK TO SAVEPOINT wallet_probe").execute(&mut *barrier_tx).await.unwrap();
        let recovery_repo = f.other();
        let recovery = tokio::spawn(async move {
            recovery_repo.process_recharge_recovery_batch(1).await.unwrap()
        });
        wait_for_payment_lock(&f.admin, recovery_pid).await;
        // Both production operations are now waiting on the same real row lock.
        // The refund is first in PostgreSQL's lock queue, without holding wallet.
        barrier_tx.commit().await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), refund).await.unwrap().unwrap();
        let mut summaries = tokio::time::timeout(Duration::from_secs(10), recovery).await.unwrap().unwrap();
        assert_eq!(summaries.len(), 1);
        // A slow runner may hit the production 500 ms lock limit at this
        // deliberate barrier. Release first, then drive the durable retry.
        if summaries[0].state == "retry" {
            due_now(&f.first, "recharge_recovery_jobs", &summaries[0].id).await;
            summaries = f.repo().process_recharge_recovery_batch(1).await.unwrap();
            assert_eq!(summaries.len(), 1);
        }
        let status: String = sqlx::query_scalar("SELECT status FROM refund_requests WHERE id='refund-race'")
            .fetch_one(&f.first).await.unwrap();
        let order_amounts: (i64, i64) = sqlx::query_as("SELECT ROUND(refunded_amount_usd*100000000)::bigint,ROUND(refundable_amount_usd*100000000)::bigint FROM payment_orders WHERE id=$1")
            .bind(&payment).fetch_one(&f.first).await.unwrap();
        if fail_processing {
            assert_eq!(status, "failed");
            assert_eq!(order_amounts, (0, 10_000_000));
            assert_eq!(summaries[0].state, "completed");
            assert_eq!(summaries[0].collected_cost_units, 2_000_000);
            assert_eq!(balances(&f.first, "wallet").await, (8_000_000, 0, 2_000_000));
            assert_eq!(receipt_totals(&f.first).await, (1, 2_000_000, 1, 1));
        } else {
            assert_eq!(status, "processing");
            assert_eq!(order_amounts, (3_000_000, 7_000_000));
            assert_eq!(summaries[0].state, "source_unavailable");
            assert_eq!(balances(&f.first, "wallet").await, (7_000_000, 0, 0));
            assert_eq!(receipt_totals(&f.first).await, (0, 0, 0, 0));
        }
        let history: (i64, i64) = sqlx::query_as("SELECT COUNT(*),ROUND(COALESCE(SUM(amount),0)*100000000)::bigint FROM wallet_transactions WHERE category='refund'")
            .fetch_one(&f.first).await.unwrap();
        assert_eq!(history, if fail_processing { (2, 0) } else { (1, -3_000_000) });
        drop(barrier);
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; real refund and recovery lock barrier"]
async fn live_recharge_recovery_refund_processing_locks_payment_before_wallet() {
    assert_refund_recovery_lock_order(false).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; real failed refund rollback and recovery lock barrier"]
async fn live_recharge_recovery_failed_refund_locks_payment_before_wallet() {
    assert_refund_recovery_lock_order(true).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; refund creation payment lock barrier"]
async fn live_recharge_recovery_refund_creation_locks_payment_before_wallet() {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        credit(&f.first, "wallet", 0.10).await;
        let payment: String =
            sqlx::query_scalar("SELECT payment_order_id FROM recharge_recovery_jobs")
                .fetch_one(&f.first)
                .await
                .unwrap();
        let refund_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&f.first)
            .await
            .unwrap();
        let mut barrier = sqlx::PgConnection::connect_with(&f.first.connect_options())
            .await
            .unwrap();
        let mut tx = barrier.begin().await.unwrap();
        sqlx::query("SELECT id FROM payment_orders WHERE id=$1 FOR UPDATE")
            .bind(&payment)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let repo = SqlxWalletRepository::new(f.first.clone());
        let refund = tokio::spawn(async move {
            assert!(matches!(
                repo.create_wallet_refund_request(CreateWalletRefundRequestInput {
                    wallet_id: "wallet".into(),
                    user_id: "owner".into(),
                    amount_usd: 0.03,
                    payment_order_id: Some(payment),
                    source_type: None,
                    source_id: None,
                    refund_mode: None,
                    reason: None,
                    idempotency_key: None,
                    refund_no: "synthetic-refund-create".into(),
                })
                .await
                .unwrap(),
                CreateWalletRefundRequestOutcome::Created(_)
            ));
        });
        wait_for_payment_lock(&f.admin, refund_pid).await;
        sqlx::query("SELECT id FROM wallets WHERE id='wallet' FOR UPDATE NOWAIT")
            .fetch_one(&mut *tx)
            .await
            .expect("refund creation must not hold wallet while waiting for payment");
        tx.commit().await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), refund)
            .await
            .unwrap()
            .unwrap();
        let status: String = sqlx::query_scalar(
            "SELECT status FROM refund_requests WHERE refund_no='synthetic-refund-create'",
        )
        .fetch_one(&f.first)
        .await
        .unwrap();
        assert_eq!(status, "pending_approval");
        assert_eq!(balances(&f.first, "wallet").await, (20_000_000, 0, 0));
        assert_eq!(receipt_totals(&f.first).await, (0, 0, 0, 0));
        drop(barrier);
    })
    .catch_unwind()
    .await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
