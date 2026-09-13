use super::*;

fn policy_quote(request: &str, suffix: &str, limit: u64) -> ReserveRequestAttemptFundsInput {
    let mut q = quote(request, suffix);
    let admitted = q.quote.admitted_at_unix_secs;
    q.usage_policy = Some(RequestFundsUsagePolicy {
        subject_id: "owner".into(),
        reservation_token: format!("{request}-policy-context"),
        admitted_at_unix_secs: admitted,
        retain_until_unix_secs: admitted + 32 * 86400,
        windows: vec![UsagePolicyCostWindow {
            window_id: "month".into(),
            starts_at_unix_secs: admitted - 60,
            ends_at_unix_secs: admitted + 30 * 86400,
            limit_cost_units: limit,
        }],
    });
    q
}

async fn quota_rows(pool: &PgPool) -> Vec<(String, String, i64, Option<i64>)> {
    sqlx::query_as("SELECT reservation_token,state,reserved_cost_units,actual_cost_units FROM usage_cost_reservations ORDER BY reservation_token")
        .fetch_all(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires an isolated AETHER_TEST_DATABASE_URL; performs joint financial transactions"]
async fn live_attempt_quota_unknown_retry_late_charge_and_replay() {
    for limit in [10_000_000, 20_000_000] {
        let (admin, first, second, schema) = fixture().await;
        let result = AssertUnwindSafe(async {
            let repo = SqlxSettlementRepository::new(first.clone());
            let usage = SqlxUsageReadRepository::new(first.clone());
            usage.upsert(parent("quota-retry")).await.unwrap();
            let a = policy_quote("quota-retry", "a", limit);
            let mut b = policy_quote("quota-retry", "b", limit);
            b.usage_policy = a.usage_policy.clone();
            assert!(matches!(
                repo.reserve_request_attempt_funds(a.clone()).await.unwrap(),
                ReserveRequestAttemptFundsOutcome::Reserved { .. }
            ));
            repo.mark_request_attempt_funds_dispatched(a.identity())
                .await
                .unwrap();
            repo.record_request_attempt_funds_outcome(facts(&a, None, false))
                .await
                .unwrap();
            let result = repo.reserve_request_attempt_funds(b.clone()).await.unwrap();
            if limit == 10_000_000 {
                assert_eq!(
                    result,
                    ReserveRequestAttemptFundsOutcome::UsagePolicyRejected {
                        window_index: 0,
                        limit_cost_units: limit,
                        used_cost_units: 8_000_000,
                    }
                );
                assert_eq!(quota_rows(&first).await.len(), 1);
                assert_eq!(
                    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM request_fund_reservations")
                        .fetch_one(&first)
                        .await
                        .unwrap(),
                    1
                );
                assert_eq!(balance(&first).await, 0.20);
                return;
            }
            assert!(matches!(
                result,
                ReserveRequestAttemptFundsOutcome::Reserved { .. }
            ));
            repo.mark_request_attempt_funds_dispatched(b.identity())
                .await
                .unwrap();
            let mut terminal_b = facts(&b, Some(6_000_000), false);
            terminal_b.facts.execution.status = RequestAttemptExecutionStatus::Cancelled;
            repo.record_request_attempt_funds_outcome(terminal_b.clone())
                .await
                .unwrap();
            let closed = repo
                .close_request_funds_admission(CloseRequestFundsAdmissionInput {
                    identity: b.identity(),
                    closed_at_unix_secs: terminal_b.finalized_at_unix_secs,
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                (closed.known_actual_cost_units, closed.held_cost_units),
                (6_000_000, 8_000_000)
            );
            let late_a = facts(&a, Some(7_000_000), false);
            repo.record_request_attempt_funds_outcome(late_a.clone())
                .await
                .unwrap();
            let rows = quota_rows(&first).await;
            assert_eq!(
                rows.iter().map(|row| row.3.unwrap()).sum::<i64>(),
                13_000_000
            );
            assert!(rows.iter().all(|row| row.1 == "finalized"));
            assert_eq!(
                summary(&first, "quota-retry").await.known_actual_cost_units,
                13_000_000
            );
            assert_eq!(balance(&first).await, 0.07);
            usage.flush_usage_counter_deltas(1000).await.unwrap();
            usage
                .cleanup_processed_usage_counter_deltas(late_a.finalized_at_unix_secs + 3600, 1000)
                .await
                .unwrap();
            repo.record_request_attempt_funds_outcome(late_a)
                .await
                .unwrap();
            repo.record_request_attempt_funds_outcome(terminal_b)
                .await
                .unwrap();
            assert!(matches!(
                repo.reserve_request_attempt_funds(a).await.unwrap(),
                ReserveRequestAttemptFundsOutcome::Reserved { .. }
            ));
            assert_eq!(quota_rows(&first).await, rows);
            assert_eq!(balance(&first).await, 0.07);
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM usage_counter_deltas")
                    .fetch_one(&first)
                    .await
                    .unwrap(),
                0
            );
        })
        .catch_unwind()
        .await;
        cleanup(admin, first, second, schema).await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}

#[tokio::test]
#[ignore = "requires an isolated AETHER_TEST_DATABASE_URL; tests quota identity and legacy cleanup"]
async fn live_attempt_quota_freezes_policy_and_survives_legacy_expiry() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let repo = SqlxSettlementRepository::new(first.clone());
        let usage = SqlxUsageReadRepository::new(first.clone());
        usage.upsert(parent("quota-policy")).await.unwrap();
        let a = policy_quote("quota-policy", "a", 10_000_000);
        repo.reserve_request_attempt_funds(a.clone()).await.unwrap();
        repo.mark_request_attempt_funds_dispatched(a.identity()).await.unwrap();
        repo.record_request_attempt_funds_outcome(facts(&a, None, false)).await.unwrap();
        for change in 0..5 {
            let mut b = policy_quote("quota-policy", "b", 10_000_000);
            b.usage_policy = a.usage_policy.clone();
            match change {
                0 => b.usage_policy = None,
                1 => b.usage_policy.as_mut().unwrap().reservation_token.push_str("-changed"),
                2 => b.usage_policy.as_mut().unwrap().windows[0].limit_cost_units = 20_000_000,
                3 => b.usage_policy.as_mut().unwrap().admitted_at_unix_secs -= 1,
                _ => b.usage_policy.as_mut().unwrap().retain_until_unix_secs += 1,
            }
            assert_eq!(repo.reserve_request_attempt_funds(b).await.unwrap(), ReserveRequestAttemptFundsOutcome::Conflict);
        }
        let mut forged = a.clone();
        forged.usage_policy.as_mut().unwrap().subject_id = "another-owner".into();
        assert!(repo.reserve_request_attempt_funds(forged).await.is_err());
        let legacy = a.usage_policy.as_ref().unwrap().cost_reservation(&a.quote);
        assert_eq!(repo.reserve_usage_policy_cost(legacy.clone()).await.unwrap(), ReserveUsagePolicyCostOutcome::Conflict);
        assert!(repo.reconcile_usage_policy_cost(ReconcileUsagePolicyCostInput {
            request_id: legacy.request_id.clone(), subject_id: legacy.subject_id.clone(),
            reservation_token: legacy.reservation_token.clone(), actual_cost_units: 0,
            terminal_state: UsagePolicyCostReservationState::Released, finalized_at_unix_secs: legacy.admitted_at_unix_secs + 1,
        }).await.is_err());
        sqlx::query("UPDATE usage_cost_reservations SET reservation_expires_at=admitted_at+INTERVAL '1 second', retain_until=admitted_at+INTERVAL '1 second'")
            .execute(&first).await.unwrap();
        assert_eq!(repo.cleanup_usage_policy_cost_reservations(legacy.admitted_at_unix_secs + 3 * 86400, 100).await.unwrap(), 0);
        let mut next = legacy.clone();
        next.request_id = "later-request".into(); next.reservation_token = "later-token".into();
        next.admitted_at_unix_secs += 3 * 86400;
        assert_eq!(repo.reserve_usage_policy_cost(next).await.unwrap(), ReserveUsagePolicyCostOutcome::Rejected {
            window_index: 0, limit_cost_units: 10_000_000, used_cost_units: 8_000_000,
        });
        repo.close_request_funds_admission(CloseRequestFundsAdmissionInput {
            identity: a.identity(), closed_at_unix_secs: legacy.admitted_at_unix_secs + 2,
        }).await.unwrap();
        assert_eq!(quota_rows(&first).await[0].1, "reserved");
        // Absence is a frozen policy too, and cannot be replaced later.
        usage.upsert(parent("without-policy")).await.unwrap();
        repo.reserve_request_attempt_funds(quote("without-policy", "a")).await.unwrap();
        assert_eq!(repo.reserve_request_attempt_funds(policy_quote("without-policy", "b", 20_000_000)).await.unwrap(), ReserveRequestAttemptFundsOutcome::Conflict);
    }).catch_unwind().await;
    cleanup(admin, first, second, schema).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires an isolated AETHER_TEST_DATABASE_URL; races distinct PostgreSQL sessions"]
async fn live_attempt_quota_concurrent_requests_and_atomic_rollback() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let repo = SqlxSettlementRepository::new(first.clone());
        let other = SqlxSettlementRepository::new(second.clone());
        let usage = SqlxUsageReadRepository::new(first.clone());
        usage.upsert(parent("race-a")).await.unwrap();
        usage.upsert(parent("race-b")).await.unwrap();
        let a = policy_quote("race-a", "a", 10_000_000);
        let b = policy_quote("race-b", "b", 10_000_000);
        let (left, right) = tokio::join!(repo.reserve_request_attempt_funds(a.clone()), other.reserve_request_attempt_funds(b.clone()));
        let results = [left.unwrap(), right.unwrap()];
        assert_eq!(results.iter().filter(|r| matches!(r, ReserveRequestAttemptFundsOutcome::Reserved { .. })).count(), 1);
        assert_eq!(results.iter().filter(|r| matches!(r, ReserveRequestAttemptFundsOutcome::UsagePolicyRejected { used_cost_units: 8_000_000, .. })).count(), 1);
        assert_eq!(quota_rows(&first).await.len(), 1);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM request_fund_allocations").fetch_one(&first).await.unwrap(), 1);
        let winner = if matches!(results[0], ReserveRequestAttemptFundsOutcome::Reserved { .. }) { a } else { b };
        let mut cancel = facts(&winner, None, false);
        cancel.facts.execution.status = RequestAttemptExecutionStatus::Cancelled;
        cancel.facts.outcome = RequestAttemptFinancialOutcome::NoCharge;
        repo.record_request_attempt_funds_outcome(cancel.clone()).await.unwrap();
        repo.record_request_attempt_funds_outcome(cancel).await.unwrap();
        assert_eq!(quota_rows(&first).await[0].1, "released");
        assert_eq!(balance(&first).await, 0.20);

        usage.upsert(parent("wallet-refused")).await.unwrap();
        sqlx::query("UPDATE wallets SET balance=0.01").execute(&first).await.unwrap();
        let refused = policy_quote("wallet-refused", "a", 20_000_000);
        assert!(matches!(repo.reserve_request_attempt_funds(refused).await.unwrap(), ReserveRequestAttemptFundsOutcome::Insufficient { .. }));
        assert_eq!(quota_rows(&first).await.len(), 1);
        sqlx::query("UPDATE wallets SET balance=0.20").execute(&first).await.unwrap();

        // Inject failure after funding insertion: the same transaction must
        // roll back both financial rows and quota admission, with no compensation.
        usage.upsert(parent("write-failed")).await.unwrap();
        sqlx::raw_sql("CREATE FUNCTION reject_attempt_quota() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected quota persistence failure'; END $$; CREATE TRIGGER reject_attempt_quota BEFORE INSERT ON usage_cost_reservations FOR EACH ROW EXECUTE FUNCTION reject_attempt_quota()")
            .execute(&first).await.unwrap();
        let failed = policy_quote("write-failed", "a", 20_000_000);
        assert!(repo.reserve_request_attempt_funds(failed.clone()).await.is_err());
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM request_fund_reservations WHERE request_id='write-failed'").fetch_one(&first).await.unwrap(), 0);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM request_fund_allocations WHERE reservation_token=$1").bind(&failed.quote.identity.reservation_token).fetch_one(&first).await.unwrap(), 0);
        assert_eq!(quota_rows(&first).await.len(), 1);
        assert_eq!(balance(&first).await, 0.20);
        sqlx::query("DROP TRIGGER reject_attempt_quota ON usage_cost_reservations").execute(&first).await.unwrap();
        repo.reserve_request_attempt_funds(failed.clone()).await.unwrap();
        repo.mark_request_attempt_funds_dispatched(failed.identity()).await.unwrap();
        let over = facts(&failed, Some(22_000_000), false);
        let stored = repo.record_request_attempt_funds_outcome(over).await.unwrap().unwrap();
        assert_eq!(stored.funds.collected_cost_units, 8_000_000);
        assert_eq!(quota_rows(&first).await.iter().find(|r| r.0 == failed.quote.identity.reservation_token).unwrap().3, Some(22_000_000));
        let mut next = policy_quote("write-failed", "b", 20_000_000);
        next.usage_policy = failed.usage_policy;
        assert_eq!(repo.reserve_request_attempt_funds(next).await.unwrap(), ReserveRequestAttemptFundsOutcome::UsagePolicyRejected {
            window_index: 0, limit_cost_units: 20_000_000, used_cost_units: 22_000_000,
        });
    }).catch_unwind().await;
    cleanup(admin, first, second, schema).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires an isolated AETHER_TEST_DATABASE_URL; tests quota and entitlement atomicity"]
async fn live_attempt_quota_entitlement_admission_and_outcome_rollback() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let repo = SqlxSettlementRepository::new(first.clone());
        let usage = SqlxUsageReadRepository::new(first.clone());
        let grant = serde_json::json!([{"type":"daily_quota","daily_quota_usd":0.20,"reset_timezone":"UTC","allow_wallet_overage":false}]);
        sqlx::query("INSERT INTO billing_plans (id,title,price_amount,duration_unit,duration_value,entitlements_json,created_at,updated_at) VALUES ('plan','plan',1,'day',1,$1,NOW(),NOW())")
            .bind(&grant).execute(&first).await.unwrap();
        sqlx::query("INSERT INTO user_plan_entitlements (id,user_id,plan_id,payment_order_id,starts_at,expires_at,entitlements_snapshot,status,created_at,updated_at) VALUES ('grant','owner','plan','order',NOW()-INTERVAL '1 hour',NOW()+INTERVAL '1 hour',$1,'active',NOW(),NOW())")
            .bind(&grant).execute(&first).await.unwrap();
        sqlx::query("UPDATE wallets SET balance=0").execute(&first).await.unwrap();
        usage.upsert(parent("quota-entitled")).await.unwrap();
        let a = policy_quote("quota-entitled", "a", 10_000_000);
        let mut b = policy_quote("quota-entitled", "b", 10_000_000);
        b.usage_policy = a.usage_policy.clone();
        repo.reserve_request_attempt_funds(a.clone()).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT source_kind FROM request_fund_allocations").fetch_one(&first).await.unwrap(), "entitlement");
        assert!(matches!(repo.reserve_request_attempt_funds(b.clone()).await.unwrap(), ReserveRequestAttemptFundsOutcome::UsagePolicyRejected { .. }));
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM request_fund_allocations").fetch_one(&first).await.unwrap(), 1);
        repo.mark_request_attempt_funds_dispatched(a.identity()).await.unwrap();
        let terminal = facts(&a, Some(6_000_000), false);
        sqlx::raw_sql("CREATE FUNCTION reject_quota_outcome() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected quota outcome failure'; END $$; CREATE TRIGGER reject_quota_outcome BEFORE UPDATE ON usage_cost_reservations FOR EACH ROW EXECUTE FUNCTION reject_quota_outcome()")
            .execute(&first).await.unwrap();
        assert!(repo.record_request_attempt_funds_outcome(terminal.clone()).await.is_err());
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM entitlement_usage_ledgers").fetch_one(&first).await.unwrap(), 0);
        assert_eq!(quota_rows(&first).await[0].1, "reserved");
        let stored = repo.read_request_attempt_funds(a.identity()).await.unwrap().unwrap();
        assert_eq!(stored.funds.state, RequestFundsState::Dispatched);
        assert!(stored.terminal_facts.is_none());
        sqlx::query("DROP TRIGGER reject_quota_outcome ON usage_cost_reservations").execute(&first).await.unwrap();
        repo.record_request_attempt_funds_outcome(terminal.clone()).await.unwrap();
        repo.record_request_attempt_funds_outcome(terminal).await.unwrap();
        assert_eq!(quota_rows(&first).await[0].3, Some(6_000_000));
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT (SUM(amount_usd)*100000000)::bigint FROM entitlement_usage_ledgers").fetch_one(&first).await.unwrap(), 6_000_000);
        assert_eq!(balance(&first).await, 0.0);
        b.quote.authorized_cost_units = 4_000_000;
        assert!(matches!(repo.reserve_request_attempt_funds(b.clone()).await.unwrap(), ReserveRequestAttemptFundsOutcome::Reserved { .. }));
        let mut cancel = facts(&b, None, false);
        cancel.facts.execution.status = RequestAttemptExecutionStatus::Cancelled;
        cancel.facts.outcome = RequestAttemptFinancialOutcome::NoCharge;
        repo.record_request_attempt_funds_outcome(cancel).await.unwrap();
        assert_eq!(quota_rows(&first).await.iter().find(|r| r.0 == b.quote.identity.reservation_token).unwrap().1, "released");
    }).catch_unwind().await;
    cleanup(admin, first, second, schema).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
