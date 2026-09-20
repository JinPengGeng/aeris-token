use super::*;

async fn persisted_state(pool: &PgPool, request: &str) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'usage', to_jsonb(u),
            'snapshot', (SELECT to_jsonb(s) FROM usage_settlement_snapshots s WHERE s.request_id=u.request_id),
            'attempts', (SELECT jsonb_agg(to_jsonb(r) ORDER BY r.reservation_token) FROM request_fund_reservations r WHERE r.request_id=u.request_id),
            'candidates', (SELECT jsonb_agg(to_jsonb(c) ORDER BY c.id) FROM request_candidates c WHERE c.request_id=u.request_id)
         ) FROM usage u WHERE u.request_id=$1",
    )
    .bind(request)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires an isolated AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn live_stale_pending_cleanup_preserves_attempt_facts_and_processes_legacy_rows() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        sqlx::query("CREATE TABLE request_candidates (LIKE public.request_candidates INCLUDING ALL)")
            .execute(&first).await.unwrap();
        sqlx::query("UPDATE wallets SET balance=100 WHERE id='wallet'")
            .execute(&first).await.unwrap();
        let repo = SqlxSettlementRepository::new(first.clone());
        let usage = SqlxUsageReadRepository::new(first.clone());
        let cases = ["prepared", "dispatched", "unknown", "charged-open", "reconcile-closed", "settled-parent"];
        let mut before = Vec::new();
        for case in cases {
            let mut initial = parent(case);
            initial.status = if case == "charged-open" { "streaming" } else { "pending" }.to_string();
            initial.billing_status = "pending".to_string();
            usage.upsert(initial).await.unwrap();
            let q = quote(case, "a");
            assert!(matches!(repo.reserve_request_attempt_funds(q.clone()).await.unwrap(),
                ReserveRequestAttemptFundsOutcome::Reserved { .. }));
            if case != "prepared" {
                repo.mark_request_attempt_funds_dispatched(q.identity()).await.unwrap().unwrap();
            }
            if !matches!(case, "prepared" | "dispatched") {
                let mut outcome = facts(&q, (case != "unknown").then_some(6_000_000), true);
                if case == "reconcile-closed" {
                    let RequestAttemptFinancialOutcome::Charged { usage } = &mut outcome.facts.outcome else { unreachable!() };
                    usage.requires_reconciliation = true;
                }
                repo.record_request_attempt_funds_outcome(outcome).await.unwrap().unwrap();
            }
            if matches!(case, "reconcile-closed" | "settled-parent") {
                repo.close_request_funds_admission(CloseRequestFundsAdmissionInput {
                    identity: q.identity(), closed_at_unix_secs: chrono::Utc::now().timestamp() as u64,
                }).await.unwrap().unwrap();
            }
            let funds = summary(&first, case).await;
            if matches!(case, "charged-open" | "reconcile-closed") {
                assert_eq!((funds.held_cost_units, funds.collected_cost_units), (0, 6_000_000),
                    "zero hold must not allow the legacy sweeper to void a known charge");
            }
            if matches!(case, "prepared" | "settled-parent") {
                sqlx::query("INSERT INTO request_candidates (id,request_id,candidate_index,status) VALUES ($1,$1,0,'streaming')")
                    .bind(case).execute(&first).await.unwrap();
            }
            sqlx::query("UPDATE usage SET created_at=NOW()-INTERVAL '2 hours' WHERE request_id=$1")
                .bind(case).execute(&first).await.unwrap();
            before.push(persisted_state(&first, case).await);
        }
        // Legacy requests are younger than all attempt parents. With a one-row
        // batch they still must be processed, without looping over skipped parents.
        for request in ["legacy-failed", "legacy-recovered"] {
            let mut initial = parent(request);
            initial.status = "pending".to_string();
            initial.billing_status = "pending".to_string();
            usage.upsert(initial).await.unwrap();
            sqlx::query("UPDATE usage SET created_at=NOW()-INTERVAL '1 hour' WHERE request_id=$1")
                .bind(request).execute(&first).await.unwrap();
        }
        sqlx::query("INSERT INTO request_candidates (id,request_id,candidate_index,status) VALUES ('legacy-recovered','legacy-recovered',0,'streaming')")
            .execute(&first).await.unwrap();
        let before_balance = balance(&first).await;
        let now = chrono::Utc::now().timestamp() as u64;
        let cleaned = tokio::time::timeout(std::time::Duration::from_secs(10),
            usage.cleanup_stale_pending_requests(now + 1, now + 601, 10, 1))
            .await.expect("legacy batches must terminate").unwrap();
        assert_eq!((cleaned.failed, cleaned.recovered), (1, 1));
        for (case, previous) in cases.into_iter().zip(before) {
            assert_eq!(persisted_state(&first, case).await, previous,
                "timeout cleanup changed attempt-owned lifecycle or financial facts: {case}");
        }
        assert_eq!(balance(&first).await, before_balance);
        let failed = usage.find_by_request_id("legacy-failed").await.unwrap().unwrap();
        assert_eq!((failed.status.as_str(), failed.billing_status.as_str()), ("failed", "void"));
        assert_eq!(usage.find_by_request_id("legacy-recovered").await.unwrap().unwrap().status, "completed");
        let repeated = usage.cleanup_stale_pending_requests(now + 1, now + 602, 10, 1).await.unwrap();
        assert_eq!((repeated.failed, repeated.recovered), (0, 0));
    }).catch_unwind().await;
    cleanup(admin, first, second, schema).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
