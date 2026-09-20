//! Candidate integrity and fully paid debt retention against real PostgreSQL.
use super::tests::{balances, credit, debt, jobs, receipt_totals, Fixture};
use crate::SqlxUsageReadRepository;
use aether_data_contracts::repository::settlement::SettlementWriteRepository;
use aether_data_contracts::repository::usage::{
    UsageCleanupExecutionMode, UsageCleanupTargets, UsageCleanupWindow,
};
use futures_util::FutureExt;
use serde_json::Value;
use std::panic::AssertUnwindSafe;

async fn assert_changed_candidate_needs_review(mutation: &str) {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        sqlx::raw_sql("UPDATE api_keys SET is_standalone=true WHERE id='key-a'; INSERT INTO wallets(id,api_key_id,balance,gift_balance,status,limit_mode,created_at,updated_at) VALUES('standalone','key-a',0,0.03,'active','finite',now(),now()); INSERT INTO users(id,username,email_verified) VALUES('other','other',false)")
            .execute(&f.first).await.unwrap();
        debt(&f.first, "changed-candidate", "key-a", 0.04, 10).await;
        credit(&f.first, "standalone", 0.04).await;
        let job = jobs(&f.first).await.remove(0).0;
        let candidates: Vec<String> = sqlx::query_scalar("SELECT request_id FROM recharge_recovery_candidates WHERE job_id=$1")
            .bind(&job).fetch_all(&f.first).await.unwrap();
        assert_eq!(candidates, vec!["changed-candidate"]);
        sqlx::query(mutation).execute(&f.first).await.unwrap();
        let result = f.repo().process_recharge_recovery_batch(1).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].state, "manual_review");
        assert_eq!(result[0].error_code.as_deref(), Some("candidate_evidence_changed"));
        assert_eq!(result[0].collected_cost_units, 0);
        assert!(result[0].next_attempt_at_unix_secs.is_none());
        assert_eq!(balances(&f.first, "standalone").await, (4_000_000, 3_000_000, 0));
        assert_eq!(balances(&f.first, "wallet").await, (10_000_000, 0, 0));
        assert_eq!(receipt_totals(&f.first).await, (0, 0, 0, 0));
        let notifications: Vec<(String, Value)> = sqlx::query_as("SELECT audience,summary FROM recharge_recovery_notifications WHERE job_id=$1 ORDER BY audience")
            .bind(&job).fetch_all(&f.first).await.unwrap();
        assert_eq!(notifications.len(), 2);
        assert_eq!(notifications[0].0, "admin");
        assert_eq!(notifications[1].0, "user");
        assert!(notifications.iter().all(|(_, summary)| summary["state"] == "manual_review"));
        assert!(f.repo().process_recharge_recovery_batch(1).await.unwrap().is_empty());
        assert_eq!(receipt_totals(&f.first).await, (0, 0, 0, 0));
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; candidate owner mutation"]
async fn live_recharge_recovery_candidate_owner_change_requires_review() {
    assert_changed_candidate_needs_review(
        "UPDATE usage SET user_id='other' WHERE request_id='changed-candidate'",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; candidate billing mode mutation"]
async fn live_recharge_recovery_candidate_mode_change_requires_review() {
    assert_changed_candidate_needs_review(
        "UPDATE usage SET billing_mode='attempt_funds' WHERE request_id='changed-candidate'",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; candidate key wallet mutation"]
async fn live_recharge_recovery_candidate_key_change_requires_review() {
    assert_changed_candidate_needs_review(
        "UPDATE usage SET api_key_id='key-b' WHERE request_id='changed-candidate'",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; candidate deletion without financial evidence"]
async fn live_recharge_recovery_candidate_missing_without_receipts_requires_review() {
    assert_changed_candidate_needs_review("DELETE FROM usage WHERE request_id='changed-candidate'")
        .await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; real retention preserves paid receipts for continuing recovery"]
async fn live_recharge_recovery_paid_candidate_retention_allows_remaining_collection() {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        // LIKE INCLUDING ALL does not copy foreign keys. Restore the real
        // cascade so this drill also removes the paid request's snapshot.
        sqlx::query("ALTER TABLE usage_settlement_snapshots ADD FOREIGN KEY (request_id) REFERENCES usage(request_id) ON DELETE CASCADE")
            .execute(&f.first).await.unwrap();
        debt(&f.first, "paid-oldest", "key-a", 0.02, 20).await;
        debt(&f.first, "remaining-later", "key-a", 0.03, 10).await;
        credit(&f.first, "wallet", 0.05).await;
        let first = f.repo().process_recharge_recovery_batch(1).await.unwrap().remove(0);
        assert_eq!(first.state, "pending");
        assert_eq!(first.collected_cost_units, 2_000_000);
        assert_eq!(receipt_totals(&f.first).await, (1, 2_000_000, 1, 1));
        let cutoff = chrono::Utc::now();
        let cleanup = SqlxUsageReadRepository::new(f.first.clone()).cleanup_usage(
            &UsageCleanupWindow { detail_cutoff: cutoff, compressed_cutoff: cutoff, header_cutoff: cutoff, log_cutoff: cutoff },
            1, false,
            UsageCleanupTargets { detail_body: false, compressed_body: false, headers: false, records: true, expired_keys: false },
            UsageCleanupExecutionMode::Policy,
        ).await.unwrap();
        assert_eq!(cleanup.records_deleted, 1);
        let remaining: Vec<String> = sqlx::query_scalar("SELECT request_id FROM usage ORDER BY request_id")
            .fetch_all(&f.first).await.unwrap();
        assert_eq!(remaining, vec!["remaining-later"], "retention removes paid usage but preserves unpaid candidate");
        let old_snapshot: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_settlement_snapshots WHERE request_id='paid-oldest'")
            .fetch_one(&f.first).await.unwrap();
        assert_eq!(old_snapshot, 0, "the real snapshot cascade must execute");
        let paid: (i64,i64,i64,i64) = sqlx::query_as("SELECT frozen_actual_cost_units,prior_entitlement_cost_units,collected_cost_units,(SELECT SUM(collected_cost_units)::bigint FROM request_fund_collection_receipts WHERE request_id=r.request_id) FROM request_fund_recoveries r WHERE request_id='paid-oldest'")
            .fetch_one(&f.first).await.unwrap();
        assert_eq!(paid, (2_000_000,0,2_000_000,2_000_000));
        let candidate_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recharge_recovery_candidates WHERE job_id=$1")
            .bind(&first.id).fetch_one(&f.first).await.unwrap();
        assert_eq!(candidate_count, 2, "retention must not erase authorization evidence");
        let second = f.repo().process_recharge_recovery_batch(1).await.unwrap().remove(0);
        assert_eq!(second.state, "completed");
        assert_eq!(second.collected_cost_units, 5_000_000);
        assert_eq!(second.outstanding_cost_units, 0);
        assert_eq!(balances(&f.first, "wallet").await, (10_000_000,0,5_000_000));
        assert_eq!(receipt_totals(&f.first).await, (2,5_000_000,2,2));
        assert!(f.repo().process_recharge_recovery_batch(1).await.unwrap().is_empty());
        assert_eq!(receipt_totals(&f.first).await, (2,5_000_000,2,2));
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; settled candidate frozen amount mismatch"]
async fn live_recharge_recovery_settled_candidate_changed_cost_requires_review() {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        debt(&f.first, "paid-oldest", "key-a", 0.02, 20).await;
        debt(&f.first, "remaining-later", "key-a", 0.03, 10).await;
        credit(&f.first, "wallet", 0.05).await;
        let first = f.repo().process_recharge_recovery_batch(1).await.unwrap().remove(0);
        assert_eq!(first.state, "pending");
        assert_eq!(first.collected_cost_units, 2_000_000);
        let changed = sqlx::query("UPDATE usage_settlement_snapshots SET billing_actual_total_cost_usd=0.03 WHERE request_id='paid-oldest' AND billing_status='settled'")
            .execute(&f.first).await.unwrap().rows_affected();
        assert_eq!(changed, 1, "change real settled snapshot after its receipt committed");
        sqlx::query("UPDATE usage SET actual_total_cost_usd=0.03 WHERE request_id='paid-oldest'")
            .execute(&f.first).await.unwrap();
        let second = f.repo().process_recharge_recovery_batch(1).await.unwrap().remove(0);
        assert_eq!(second.state, "manual_review");
        assert_eq!(second.error_code.as_deref(), Some("candidate_evidence_changed"));
        assert_eq!(second.collected_cost_units, 2_000_000);
        assert_eq!(balances(&f.first, "wallet").await, (13_000_000,0,2_000_000));
        assert_eq!(receipt_totals(&f.first).await, (1,2_000_000,1,1));
        let unpaid: String = sqlx::query_scalar("SELECT billing_status FROM usage WHERE request_id='remaining-later'")
            .fetch_one(&f.first).await.unwrap();
        assert_eq!(unpaid, "insufficient_quota");
        assert!(f.repo().process_recharge_recovery_batch(1).await.unwrap().is_empty());
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; individually valid debts exceed JSON integer sum boundary"]
async fn live_recharge_recovery_unsafe_json_debt_total_requires_review_after_exact_debit() {
    let f = Fixture::new().await;
    let result = AssertUnwindSafe(async {
        for (request, age) in [("large-oldest",30),("large-middle",20),("large-newest",10)] {
            debt(&f.first, request, "key-a", 40_000_000.0, age).await;
        }
        // Payment rails store pay_amount in cents. Use a valid one-cent credit
        // and reserve all but one atomic unit, rather than inventing a sub-cent
        // gateway payment that the real callback correctly rejects.
        credit(&f.first, "wallet", 0.01).await;
        let held = crate::settlement::funding::tests::quote("precision-held", "key-b", 10_999_999);
        assert!(matches!(f.repo().reserve_request_funds(held).await.unwrap(),
            aether_data_contracts::repository::settlement::ReserveRequestFundsOutcome::Reserved { .. }));
        let summary = f.repo().process_recharge_recovery_batch(1).await.unwrap().remove(0);
        assert_eq!(summary.state, "manual_review");
        assert_eq!(summary.error_code.as_deref(), Some("incomplete_debt_evidence"));
        assert_eq!(summary.principal_cost_units, 1_000_000);
        assert_eq!(summary.collected_cost_units, 1);
        assert!(summary.next_attempt_at_unix_secs.is_none());
        assert_eq!(balances(&f.first, "wallet").await, (10_999_999,0,1));
        assert_eq!(receipt_totals(&f.first).await, (1,1,1,1));
        let outstanding: i64 = sqlx::query_scalar("SELECT SUM(CEIL(u.actual_total_cost_usd*100000000)-COALESCE(r.collected_cost_units,0))::bigint FROM usage u LEFT JOIN request_fund_recoveries r USING(request_id)")
            .fetch_one(&f.first).await.unwrap();
        assert_eq!(outstanding, 11_999_999_999_999_999);
        let unpaid: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage WHERE billing_status='insufficient_quota'")
            .fetch_one(&f.first).await.unwrap();
        assert_eq!(unpaid, 3);
        let notifications: Vec<Value> = sqlx::query_scalar("SELECT summary FROM recharge_recovery_notifications WHERE job_id=$1")
            .bind(&summary.id).fetch_all(&f.first).await.unwrap();
        assert_eq!(notifications.len(),2);
        assert!(notifications.iter().all(|value| value["state"]=="manual_review" && value["collected_cost_units"]==1));
        assert!(f.repo().process_recharge_recovery_batch(1).await.unwrap().is_empty());
        assert_eq!(receipt_totals(&f.first).await, (1,1,1,1));
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
