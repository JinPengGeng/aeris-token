use super::*;
use crate::repository::{
    usage::{InMemoryUsageReadRepository, UsageReadRepository},
    wallet::StoredWalletSnapshot,
};
use std::sync::Arc;

fn fixture(
    balance: f64,
) -> (
    InMemorySettlementRepository,
    Arc<InMemoryUsageReadRepository>,
) {
    let parent = StoredRequestUsageAudit::new(
        "usage".to_string(),
        "request".to_string(),
        Some("owner".to_string()),
        Some("key".to_string()),
        None,
        None,
        "provider".to_string(),
        "image".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        false,
        0,
        0,
        0,
        0.0,
        0.0,
        None,
        None,
        None,
        None,
        None,
        "pending".to_string(),
        "pending".to_string(),
        100,
        100,
        None,
    )
    .unwrap();
    let usage = Arc::new(InMemoryUsageReadRepository::seed([parent]));
    let wallet = StoredWalletSnapshot::new(
        "wallet".to_string(),
        Some("owner".to_string()),
        None,
        balance,
        0.0,
        "finite".to_string(),
        "USD".to_string(),
        "active".to_string(),
        0.0,
        0.0,
        0.0,
        0.0,
        100,
    )
    .unwrap();
    (
        InMemorySettlementRepository::seed([wallet]).with_usage_repository(usage.clone()),
        usage,
    )
}

fn quote(suffix: &str) -> ReserveRequestAttemptFundsInput {
    ReserveRequestAttemptFundsInput {
        usage_policy: None,
        attempt_id: uuid::Uuid::new_v4().to_string(),
        provider: RequestAttemptProvider {
            provider_id: format!("provider-{suffix}"),
            provider_api_key_id: Some(format!("pk-{suffix}")),
            model_id: None,
            candidate_id: None,
        },
        quote: ReserveRequestFundsInput {
            identity: RequestFundsIdentity {
                reservation_token: format!("token-{suffix}"),
                request_id: "request".to_string(),
                user_id: Some("owner".to_string()),
                api_key_id: Some("key".to_string()),
                api_key_is_standalone: false,
            },
            authorized_cost_units: 8_000_000,
            pricing_snapshot: serde_json::json!({"version":1}),
            admitted_at_unix_secs: 100,
        },
    }
}

fn policy_quote(suffix: &str, limit_cost_units: u64) -> ReserveRequestAttemptFundsInput {
    let mut q = quote(suffix);
    q.usage_policy = Some(RequestFundsUsagePolicy {
        subject_id: "owner".into(),
        reservation_token: "request-policy-context".into(),
        admitted_at_unix_secs: 100,
        retain_until_unix_secs: 1_000_000,
        windows: vec![UsagePolicyCostWindow {
            window_id: "month".into(),
            starts_at_unix_secs: 0,
            ends_at_unix_secs: 1_000_000,
            limit_cost_units,
        }],
    });
    q
}

#[tokio::test]
async fn attempt_quota_retries_count_known_actual_plus_unknown_holds() {
    for limit in [10_000_000, 20_000_000] {
        let (repo, _) = fixture(0.20);
        let a = policy_quote("a", limit);
        let b = policy_quote("b", limit);
        assert!(matches!(
            repo.reserve_request_attempt_funds(a.clone()).await.unwrap(),
            ReserveRequestAttemptFundsOutcome::Reserved { .. }
        ));
        repo.mark_request_attempt_funds_dispatched(a.identity())
            .await
            .unwrap();
        repo.record_request_attempt_funds_outcome(facts(&a, None))
            .await
            .unwrap();
        let admission = repo.reserve_request_attempt_funds(b.clone()).await.unwrap();
        if limit == 10_000_000 {
            assert_eq!(
                admission,
                ReserveRequestAttemptFundsOutcome::UsagePolicyRejected {
                    window_index: 0,
                    limit_cost_units: limit,
                    used_cost_units: 8_000_000,
                }
            );
            assert_eq!(repo.funds.read().unwrap().len(), 1);
            assert_eq!(repo.cost_reservations.read().unwrap().len(), 1);
            continue;
        }
        assert!(matches!(
            admission,
            ReserveRequestAttemptFundsOutcome::Reserved { .. }
        ));
        repo.mark_request_attempt_funds_dispatched(b.identity())
            .await
            .unwrap();
        let mut b_facts = facts(&b, Some(6_000_000));
        b_facts.facts.execution.status = RequestAttemptExecutionStatus::Cancelled;
        repo.record_request_attempt_funds_outcome(b_facts)
            .await
            .unwrap();
        repo.close_request_funds_admission(CloseRequestFundsAdmissionInput {
            identity: b.identity(),
            closed_at_unix_secs: 102,
        })
        .await
        .unwrap();
        assert_eq!(
            repo.cost_reservations.read().unwrap()[&a.quote.identity.reservation_token].state,
            UsagePolicyCostReservationState::Reserved
        );
        let late = facts(&a, Some(7_000_000));
        repo.record_request_attempt_funds_outcome(late.clone())
            .await
            .unwrap();
        let quotas = repo.cost_reservations.read().unwrap().clone();
        assert_eq!(
            quotas
                .values()
                .map(|r| r.actual_cost_units.unwrap())
                .sum::<u64>(),
            13_000_000
        );
        assert!(quotas
            .values()
            .all(|r| r.state == UsagePolicyCostReservationState::Finalized));
        repo.record_request_attempt_funds_outcome(late)
            .await
            .unwrap();
        assert_eq!(*repo.cost_reservations.read().unwrap(), quotas);
        assert!(matches!(
            repo.reserve_request_attempt_funds(a).await.unwrap(),
            ReserveRequestAttemptFundsOutcome::Reserved { .. }
        ));
        assert_eq!(*repo.cost_reservations.read().unwrap(), quotas);
    }
}

#[tokio::test]
async fn attempt_quota_freezes_policy_absence_and_owns_legacy_retention() {
    let (repo, _) = fixture(0.20);
    let a = policy_quote("a", 10_000_000);
    repo.reserve_request_attempt_funds(a.clone()).await.unwrap();
    repo.mark_request_attempt_funds_dispatched(a.identity())
        .await
        .unwrap();
    repo.record_request_attempt_funds_outcome(facts(&a, None))
        .await
        .unwrap();
    for change in 0..4 {
        let mut b = policy_quote("b", 10_000_000);
        match change {
            0 => b.usage_policy = None,
            1 => b.usage_policy.as_mut().unwrap().reservation_token = "changed-context".into(),
            2 => b.usage_policy.as_mut().unwrap().windows[0].limit_cost_units = 20_000_000,
            _ => b.usage_policy.as_mut().unwrap().admitted_at_unix_secs = 99,
        }
        assert_eq!(
            repo.reserve_request_attempt_funds(b).await.unwrap(),
            ReserveRequestAttemptFundsOutcome::Conflict
        );
    }
    let legacy = a.usage_policy.as_ref().unwrap().cost_reservation(&a.quote);
    assert_eq!(
        repo.reserve_usage_policy_cost(legacy.clone())
            .await
            .unwrap(),
        ReserveUsagePolicyCostOutcome::Conflict
    );
    assert!(repo
        .reconcile_usage_policy_cost(ReconcileUsagePolicyCostInput {
            request_id: legacy.request_id.clone(),
            subject_id: legacy.subject_id.clone(),
            reservation_token: legacy.reservation_token.clone(),
            actual_cost_units: 0,
            terminal_state: UsagePolicyCostReservationState::Released,
            finalized_at_unix_secs: 200,
        })
        .await
        .is_err());
    // Simulate both old TTL/retention dates being elapsed. The linked Unknown
    // must still influence a later admission in the same calendar window.
    {
        let mut quotas = repo.cost_reservations.write().unwrap();
        let quota = quotas.get_mut(&legacy.reservation_token).unwrap();
        quota.reservation_expires_at_unix_secs = 101;
        quota.retain_until_unix_secs = 101;
    }
    assert_eq!(
        repo.cleanup_usage_policy_cost_reservations(200_000, 100)
            .await
            .unwrap(),
        0
    );
    let mut next = legacy;
    next.request_id = "another-request".into();
    next.reservation_token = "another-token".into();
    next.admitted_at_unix_secs = 200_000;
    assert_eq!(
        repo.reserve_usage_policy_cost(next).await.unwrap(),
        ReserveUsagePolicyCostOutcome::Rejected {
            window_index: 0,
            limit_cost_units: 10_000_000,
            used_cost_units: 8_000_000,
        }
    );
    let (no_policy_repo, _) = fixture(0.20);
    no_policy_repo
        .reserve_request_attempt_funds(quote("a"))
        .await
        .unwrap();
    assert_eq!(
        no_policy_repo
            .reserve_request_attempt_funds(policy_quote("b", 20_000_000))
            .await
            .unwrap(),
        ReserveRequestAttemptFundsOutcome::Conflict
    );
}

#[tokio::test]
async fn attempt_quota_wallet_rejection_and_prepared_cancellation_leave_no_leak() {
    let (repo, _) = fixture(0.01);
    let a = policy_quote("a", 20_000_000);
    assert!(matches!(
        repo.reserve_request_attempt_funds(a.clone()).await.unwrap(),
        ReserveRequestAttemptFundsOutcome::Insufficient { .. }
    ));
    assert!(repo.cost_reservations.read().unwrap().is_empty());
    assert!(repo.funds.read().unwrap().is_empty());
    let (repo, _) = fixture(0.20);
    repo.reserve_request_attempt_funds(a.clone()).await.unwrap();
    let mut cancelled = facts(&a, None);
    cancelled.facts.execution.status = RequestAttemptExecutionStatus::Cancelled;
    cancelled.facts.outcome = RequestAttemptFinancialOutcome::NoCharge;
    repo.record_request_attempt_funds_outcome(cancelled.clone())
        .await
        .unwrap();
    repo.record_request_attempt_funds_outcome(cancelled)
        .await
        .unwrap();
    let quota = repo.cost_reservations.read().unwrap()[&a.quote.identity.reservation_token].clone();
    assert_eq!(
        (quota.state, quota.actual_cost_units),
        (UsagePolicyCostReservationState::Released, Some(0))
    );
    let mut b = policy_quote("b", 20_000_000);
    b.quote.authorized_cost_units = 20_000_000;
    assert!(matches!(
        repo.reserve_request_attempt_funds(b).await.unwrap(),
        ReserveRequestAttemptFundsOutcome::Reserved { .. }
    ));
}

#[tokio::test]
async fn attempt_quota_counts_over_quote_actual_without_refusing_incurred_cost() {
    let (repo, _) = fixture(0.20);
    let a = policy_quote("a", 10_000_000);
    repo.reserve_request_attempt_funds(a.clone()).await.unwrap();
    repo.mark_request_attempt_funds_dispatched(a.identity())
        .await
        .unwrap();
    repo.record_request_attempt_funds_outcome(facts(&a, Some(12_000_000)))
        .await
        .unwrap();
    let quota = repo.cost_reservations.read().unwrap()[&a.quote.identity.reservation_token].clone();
    assert_eq!(quota.actual_cost_units, Some(12_000_000));
    assert_eq!(
        repo.funds.read().unwrap()[&a.quote.identity.reservation_token].collected_cost_units,
        8_000_000
    );
    assert_eq!(
        repo.reserve_request_attempt_funds(policy_quote("b", 10_000_000))
            .await
            .unwrap(),
        ReserveRequestAttemptFundsOutcome::UsagePolicyRejected {
            window_index: 0,
            limit_cost_units: 10_000_000,
            used_cost_units: 12_000_000,
        }
    );
}

#[tokio::test]
async fn attempt_quota_serializes_distinct_external_requests() {
    let (repo, initial_usage) = fixture(0.20);
    let a_parent = initial_usage
        .find_by_request_id("request")
        .await
        .unwrap()
        .unwrap();
    let mut b_parent = a_parent.clone();
    b_parent.id = "usage-b".into();
    b_parent.request_id = "request-b".into();
    let usage = Arc::new(InMemoryUsageReadRepository::seed([a_parent, b_parent]));
    let repo = Arc::new(repo.with_usage_repository(usage));
    let a = policy_quote("a", 10_000_000);
    let mut b = policy_quote("b", 10_000_000);
    b.quote.identity.request_id = "request-b".into();
    b.usage_policy.as_mut().unwrap().reservation_token = "context-b".into();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let run = |q| {
        let repo = Arc::clone(&repo);
        let barrier = Arc::clone(&barrier);
        tokio::task::spawn_blocking(move || {
            barrier.wait();
            super::reserve(&repo, q).unwrap()
        })
    };
    let (a, b) = tokio::join!(run(a), run(b));
    let outcomes = [a.unwrap(), b.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, ReserveRequestAttemptFundsOutcome::Reserved { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(
                r,
                ReserveRequestAttemptFundsOutcome::UsagePolicyRejected {
                    used_cost_units: 8_000_000,
                    ..
                }
            ))
            .count(),
        1
    );
    assert_eq!(repo.funds.read().unwrap().len(), 1);
    assert_eq!(repo.cost_reservations.read().unwrap().len(), 1);
}

fn facts(
    q: &ReserveRequestAttemptFundsInput,
    cost: Option<u64>,
) -> RecordRequestAttemptFundsOutcomeInput {
    RecordRequestAttemptFundsOutcomeInput {
        identity: q.identity(),
        finalized_at_unix_secs: 101,
        facts: RequestAttemptTerminalFacts {
            schema_version: 1,
            execution: RequestAttemptExecutionFacts {
                status: RequestAttemptExecutionStatus::Failed,
                response_time_ms: 100,
            },
            outcome: cost.map_or(RequestAttemptFinancialOutcome::Unknown, |cost| {
                RequestAttemptFinancialOutcome::Charged {
                    usage: RequestAttemptBilledUsage {
                        total_cost_units: cost,
                        actual_cost_units: cost,
                        input_tokens: 2,
                        output_tokens: 3,
                        ..Default::default()
                    },
                }
            }),
            evidence: serde_json::json!({"receipt":"trusted"}),
        },
    }
}

fn balance(repo: &InMemorySettlementRepository) -> f64 {
    repo.wallets.with_mut(|wallets| wallets["wallet"].balance)
}

#[tokio::test]
async fn priced_quote_mismatch_preserves_charge_and_pending_audit() {
    let (repo, usage_repo) = fixture(0.20);
    let q = quote("format-change");
    repo.reserve_request_attempt_funds(q.clone()).await.unwrap();
    repo.mark_request_attempt_funds_dispatched(q.identity())
        .await
        .unwrap();
    let mut observed = facts(&q, Some(6_000_000));
    let RequestAttemptFinancialOutcome::Charged { usage } = &mut observed.facts.outcome else {
        unreachable!()
    };
    usage.requires_reconciliation = true;
    let stored = repo
        .record_request_attempt_funds_outcome(observed.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.funds.state, RequestFundsState::ReconciliationPending);
    assert_eq!(stored.funds.actual_cost_units, Some(6_000_000));
    assert_eq!(stored.funds.collected_cost_units, 6_000_000);
    assert_eq!(
        stored.funds.reconciliation_facts.as_ref().unwrap()["excess_units"],
        0
    );
    let summary = repo
        .close_request_funds_admission(CloseRequestFundsAdmissionInput {
            identity: q.identity(),
            closed_at_unix_secs: 102,
        })
        .await
        .unwrap()
        .unwrap();
    assert!(summary.requires_reconciliation);
    assert_eq!((summary.held_cost_units, summary.unknown_attempts), (0, 0));
    assert_eq!(summary.known_actual_cost_units, 6_000_000);
    let parent = usage_repo
        .find_by_request_id("request")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent.billing_status, "pending");
    repo.record_request_attempt_funds_outcome(observed.clone())
        .await
        .unwrap();
    assert_eq!(balance(&repo), 0.14);
    let RequestAttemptFinancialOutcome::Charged { usage } = &mut observed.facts.outcome else {
        unreachable!()
    };
    usage.requires_reconciliation = false;
    assert!(repo
        .record_request_attempt_funds_outcome(observed)
        .await
        .is_err());
    assert_eq!(balance(&repo), 0.14);
}

#[tokio::test]
async fn attempts_keep_unknown_holds_across_retry_and_late_charge() {
    let (repo, usage) = fixture(0.20);
    let a = quote("a");
    let b = quote("b");
    assert!(matches!(
        repo.reserve_request_attempt_funds(a.clone()).await.unwrap(),
        ReserveRequestAttemptFundsOutcome::Reserved { .. }
    ));
    repo.mark_request_attempt_funds_dispatched(a.identity())
        .await
        .unwrap()
        .unwrap();
    repo.record_request_attempt_funds_outcome(facts(&a, None))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        repo.reserve_request_attempt_funds(b.clone()).await.unwrap(),
        ReserveRequestAttemptFundsOutcome::Reserved { .. }
    ));
    repo.mark_request_attempt_funds_dispatched(b.identity())
        .await
        .unwrap()
        .unwrap();
    let charged_b = facts(&b, Some(6_000_000));
    repo.record_request_attempt_funds_outcome(charged_b.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(balance(&repo), 0.14);
    let closed = repo
        .close_request_funds_admission(CloseRequestFundsAdmissionInput {
            identity: b.identity(),
            closed_at_unix_secs: 102,
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            closed.known_actual_cost_units,
            closed.held_cost_units,
            closed.unknown_attempts
        ),
        (6_000_000, 8_000_000, 1)
    );
    assert_eq!(
        repo.reserve_request_attempt_funds(quote("c"))
            .await
            .unwrap(),
        ReserveRequestAttemptFundsOutcome::AdmissionClosed
    );
    let charged_a = facts(&a, Some(7_000_000));
    repo.record_request_attempt_funds_outcome(charged_a.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(balance(&repo), 0.07);
    let stored = usage.find_by_request_id("request").await.unwrap().unwrap();
    assert_eq!(stored.actual_total_cost_usd, 0.13);
    assert_eq!(stored.total_tokens, 10);
    assert_eq!(stored.billing_status, "settled");
    let by_key = usage
        .summarize_usage_by_provider_api_key_ids(&["pk-a".to_string(), "pk-b".to_string()])
        .await
        .unwrap();
    assert_eq!(
        (by_key["pk-a"].request_count, by_key["pk-a"].total_cost_usd),
        (1, 0.07)
    );
    assert_eq!(
        (by_key["pk-b"].request_count, by_key["pk-b"].total_cost_usd),
        (1, 0.06)
    );
    let windows: Vec<_> = ["pk-a", "pk-b"]
        .into_iter()
        .map(
            |key| aether_data_contracts::repository::usage::ProviderApiKeyWindowUsageRequest {
                provider_api_key_id: key.to_string(),
                window_code: "5h".to_string(),
                start_unix_secs: 99,
                end_unix_secs: 102,
            },
        )
        .collect();
    let windows = usage
        .summarize_usage_by_provider_api_key_windows(&windows)
        .await
        .unwrap();
    assert_eq!(
        windows
            .iter()
            .map(|row| (row.request_count, row.total_tokens, row.total_cost_usd))
            .collect::<Vec<_>>(),
        vec![(1, 5, 0.07), (1, 5, 0.06)]
    );
    let done = repo
        .close_request_funds_admission(CloseRequestFundsAdmissionInput {
            identity: a.identity(),
            closed_at_unix_secs: 103,
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            done.collected_cost_units,
            done.held_cost_units,
            done.unknown_attempts
        ),
        (13_000_000, 0, 0)
    );
    repo.record_request_attempt_funds_outcome(charged_a)
        .await
        .unwrap();
    repo.record_request_attempt_funds_outcome(charged_b)
        .await
        .unwrap();
    assert_eq!(balance(&repo), 0.07);
    assert!(repo
        .release_request_funds(ReleaseRequestFundsInput {
            identity: a.quote.identity.clone(),
            terminal_no_charge: true
        })
        .await
        .is_err());
    assert!(repo
        .recover_insufficient_quota(RecoverInsufficientQuotaInput {
            request_id: "request".to_string()
        })
        .await
        .is_err());
}

#[tokio::test]
async fn attempts_reject_cross_identity_and_insufficient_retry_without_mutation() {
    let (repo, _) = fixture(0.10);
    let a = quote("a");
    repo.reserve_request_attempt_funds(a.clone()).await.unwrap();
    repo.mark_request_attempt_funds_dispatched(a.identity())
        .await
        .unwrap();
    repo.record_request_attempt_funds_outcome(facts(&a, None))
        .await
        .unwrap();
    assert_eq!(
        repo.reserve_request_attempt_funds(quote("b"))
            .await
            .unwrap(),
        ReserveRequestAttemptFundsOutcome::Insufficient {
            available_cost_units: 2_000_000
        }
    );
    let mut stolen = a.identity();
    stolen.request.api_key_id = Some("other".to_string());
    assert!(repo.read_request_attempt_funds(stolen).await.is_err());
    let mut overflow = facts(&a, Some(7_000_000));
    if let RequestAttemptFinancialOutcome::Charged { usage } = &mut overflow.facts.outcome {
        usage.input_tokens = i32::MAX as u64;
    }
    assert!(repo
        .record_request_attempt_funds_outcome(overflow)
        .await
        .is_err());
    assert_eq!(balance(&repo), 0.10);
    let mut nocharge = facts(&a, None);
    nocharge.facts.outcome = RequestAttemptFinancialOutcome::NoCharge;
    repo.record_request_attempt_funds_outcome(nocharge)
        .await
        .unwrap();
    assert!(matches!(
        repo.reserve_request_attempt_funds(quote("b"))
            .await
            .unwrap(),
        ReserveRequestAttemptFundsOutcome::Reserved { .. }
    ));
    assert_eq!(balance(&repo), 0.10);
}

#[tokio::test]
async fn attempts_cap_debit_at_quote_and_keep_reconciliation_difference() {
    let (repo, _) = fixture(0.20);
    let a = quote("a");
    repo.reserve_request_attempt_funds(a.clone()).await.unwrap();
    repo.mark_request_attempt_funds_dispatched(a.identity())
        .await
        .unwrap();
    let input = facts(&a, Some(12_000_000));
    let stored = repo
        .record_request_attempt_funds_outcome(input.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.funds.collected_cost_units, 8_000_000);
    assert_eq!(stored.funds.state, RequestFundsState::ReconciliationPending);
    assert_eq!(balance(&repo), 0.12);
    let summary = repo
        .close_request_funds_admission(CloseRequestFundsAdmissionInput {
            identity: a.identity(),
            closed_at_unix_secs: 102,
        })
        .await
        .unwrap()
        .unwrap();
    assert!(summary.requires_reconciliation);
    assert_eq!(summary.held_cost_units, 0);
    assert_eq!(summary.known_actual_cost_units, 12_000_000);
    repo.record_request_attempt_funds_outcome(input)
        .await
        .unwrap();
    assert_eq!(balance(&repo), 0.12);
}
