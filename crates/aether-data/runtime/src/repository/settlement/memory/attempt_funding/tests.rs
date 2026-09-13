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
