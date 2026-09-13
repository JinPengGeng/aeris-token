use super::*;
use crate::{SqlxSettlementRepository, SqlxUsageReadRepository};
use aether_data_contracts::repository::usage::{UpsertUsageRecord, UsageBodyCaptureState};
use futures_util::FutureExt;
use sqlx::PgPool;
use std::panic::AssertUnwindSafe;

async fn fixture() -> (PgPool, PgPool, PgPool, String) {
    let (admin, first, second, schema) = super::super::tests::fixture().await;
    for table in [
        "usage_http_audits",
        "usage_routing_snapshots",
        "usage_body_blobs",
        "providers",
        "provider_api_keys",
        "global_models",
    ] {
        sqlx::query(&format!(
            "CREATE TABLE {table} (LIKE public.{table} INCLUDING ALL)"
        ))
        .execute(&first)
        .await
        .unwrap();
    }
    let view: String =
        sqlx::query_scalar("SELECT pg_get_viewdef('public.usage_billing_facts'::regclass, true)")
            .fetch_one(&admin)
            .await
            .unwrap();
    sqlx::raw_sql(&format!(
        "CREATE VIEW usage_billing_facts AS {}",
        view.replace("public.", &format!("{schema}."))
    ))
    .execute(&first)
    .await
    .unwrap();
    sqlx::raw_sql("INSERT INTO providers (id,name,provider_type,created_at,updated_at) VALUES ('p-a','p-a','custom',NOW(),NOW()),('p-b','p-b','custom',NOW(),NOW()); INSERT INTO provider_api_keys (id,provider_id,name,api_key,total_tokens,total_cost_usd,created_at,updated_at) VALUES ('pk-a','p-a','a','placeholder',0,0,NOW(),NOW()),('pk-b','p-b','b','placeholder',0,0,NOW(),NOW()); UPDATE wallets SET balance=0.20 WHERE id='wallet'").execute(&first).await.unwrap();
    (admin, first, second, schema)
}

fn parent(request: &str) -> UpsertUsageRecord {
    let now = chrono::Utc::now().timestamp() as u64;
    let mut usage = crate::usage::tests::fast_clear_usage_record(
        request,
        "test",
        now,
        false,
        UsageBodyCaptureState::None,
        None,
    );
    usage.user_id = Some("owner".to_string());
    usage.api_key_id = Some("key-a".to_string());
    usage.provider_id = Some("p-a".to_string());
    usage.provider_api_key_id = Some("pk-a".to_string());
    usage
}

fn quote(request: &str, suffix: &str) -> ReserveRequestAttemptFundsInput {
    ReserveRequestAttemptFundsInput {
        attempt_id: uuid::Uuid::new_v4().to_string(),
        provider: RequestAttemptProvider {
            provider_id: format!("p-{suffix}"),
            provider_api_key_id: Some(format!("pk-{suffix}")),
            model_id: Some("image".to_string()),
            candidate_id: None,
        },
        quote: ReserveRequestFundsInput {
            identity: RequestFundsIdentity {
                reservation_token: format!("{request}-{suffix}"),
                request_id: request.to_string(),
                user_id: Some("owner".to_string()),
                api_key_id: Some("key-a".to_string()),
                api_key_is_standalone: false,
            },
            authorized_cost_units: 8_000_000,
            pricing_snapshot: serde_json::json!({"version":1}),
            admitted_at_unix_secs: chrono::Utc::now().timestamp() as u64,
        },
    }
}

fn facts(
    q: &ReserveRequestAttemptFundsInput,
    charged: Option<u64>,
    success: bool,
) -> RecordRequestAttemptFundsOutcomeInput {
    RecordRequestAttemptFundsOutcomeInput {
        identity: q.identity(),
        finalized_at_unix_secs: chrono::Utc::now().timestamp() as u64,
        facts: RequestAttemptTerminalFacts {
            schema_version: 1,
            execution: RequestAttemptExecutionFacts {
                status: if success {
                    RequestAttemptExecutionStatus::Completed
                } else {
                    RequestAttemptExecutionStatus::Failed
                },
                response_time_ms: 100,
            },
            outcome: charged.map_or(RequestAttemptFinancialOutcome::Unknown, |cost| {
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
            evidence: serde_json::json!({"source":"test_receipt"}),
        },
    }
}

async fn summary(pool: &PgPool, request: &str) -> RequestFundsSummary {
    let value: Value = sqlx::query_scalar(
        "SELECT request_funds_summary FROM usage_settlement_snapshots WHERE request_id=$1",
    )
    .bind(request)
    .fetch_one(pool)
    .await
    .unwrap();
    serde_json::from_value(value).unwrap()
}

async fn balance(pool: &PgPool) -> f64 {
    sqlx::query_scalar("SELECT balance::double precision FROM wallets WHERE id='wallet'")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn cleanup(admin: PgPool, first: PgPool, second: PgPool, schema: String) {
    first.close().await;
    second.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
#[ignore = "requires an isolated AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn live_attempt_funds_retry_late_charge_and_provider_rebuild_are_idempotent() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let repo = SqlxSettlementRepository::new(first.clone()); let other = SqlxSettlementRepository::new(second.clone());
        let usage = SqlxUsageReadRepository::new(first.clone());
        usage.upsert(parent("retry")).await.unwrap();
        let a=quote("retry","a"); let b=quote("retry","b");
        assert!(matches!(repo.reserve_request_attempt_funds(a.clone()).await.unwrap(), ReserveRequestAttemptFundsOutcome::Reserved{..}));
        repo.mark_request_attempt_funds_dispatched(a.identity()).await.unwrap().unwrap();
        repo.record_request_attempt_funds_outcome(facts(&a,None,false)).await.unwrap().unwrap();
        assert_eq!(summary(&first,"retry").await.held_cost_units,8_000_000);
        assert!(matches!(repo.reserve_request_attempt_funds(b.clone()).await.unwrap(), ReserveRequestAttemptFundsOutcome::Reserved{..}));
        repo.mark_request_attempt_funds_dispatched(b.identity()).await.unwrap().unwrap();
        let terminal_b=facts(&b,Some(6_000_000),true);
        let (left,right)=tokio::join!(repo.record_request_attempt_funds_outcome(terminal_b.clone()),other.record_request_attempt_funds_outcome(terminal_b.clone()));
        assert_eq!(left.unwrap(),right.unwrap());
        assert_eq!(balance(&first).await,0.14);
        let closed=repo.close_request_funds_admission(CloseRequestFundsAdmissionInput { identity:b.identity(),closed_at_unix_secs:terminal_b.finalized_at_unix_secs }).await.unwrap().unwrap();
        assert_eq!((closed.unknown_attempts,closed.held_cost_units,closed.known_actual_cost_units),(1,8_000_000,6_000_000));
        assert!(matches!(repo.reserve_request_attempt_funds(quote("retry","c")).await.unwrap(),ReserveRequestAttemptFundsOutcome::AdmissionClosed));
        let mut first_byte=parent("retry");first_byte.status="streaming".to_string();first_byte.user_id=Some("forged".to_string());first_byte.provider_api_key_id=Some("pk-b".to_string());
        usage.upsert_first_byte(first_byte.clone()).await.unwrap();usage.upsert_first_byte_many(vec![first_byte]).await.unwrap();
        let mut late=parent("retry"); late.status="completed".to_string(); late.billing_status="settled".to_string(); late.total_cost_usd=Some(999.0); late.actual_total_cost_usd=Some(999.0); late.input_tokens=Some(999); late.total_tokens=Some(999); late.user_id=Some("forged".to_string());
        let read=usage.upsert(late).await.unwrap(); assert_eq!(read.user_id.as_deref(),Some("owner")); assert_eq!(read.actual_total_cost_usd,0.06); assert_eq!(read.total_tokens,5); assert_eq!(read.billing_status,"pending");
        let terminal_a=facts(&a,Some(7_000_000),false);
        repo.record_request_attempt_funds_outcome(terminal_a.clone()).await.unwrap().unwrap();
        let done=summary(&first,"retry").await; assert_eq!((done.held_cost_units,done.unknown_attempts,done.known_actual_cost_units,done.collected_cost_units),(0,0,13_000_000,13_000_000)); assert_eq!(balance(&first).await,0.07);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM usage").fetch_one(&first).await.unwrap(),1);
        assert!(repo.settle_usage(UsageSettlementInput { request_id:"retry".to_string(),user_id:Some("owner".to_string()),api_key_id:Some("key-a".to_string()),api_key_is_standalone:false,provider_id:Some("p-b".to_string()),status:"completed".to_string(),billing_status:"pending".to_string(),total_cost_usd:0.13,actual_total_cost_usd:0.13,finalized_at_unix_secs:Some(terminal_a.finalized_at_unix_secs) }).await.is_err());
        assert!(repo.recover_insufficient_quota(RecoverInsufficientQuotaInput{request_id:"retry".to_string()}).await.is_err());
        assert!(repo.release_request_funds(ReleaseRequestFundsInput{identity:a.quote.identity.clone(),terminal_no_charge:true}).await.is_err());
        usage.flush_usage_counter_deltas(1000).await.unwrap();
        let requested_windows: Vec<_> = ["pk-a", "pk-b"].into_iter().map(|key| aether_data_contracts::repository::usage::ProviderApiKeyWindowUsageRequest { provider_api_key_id:key.to_string(),window_code:"5h".to_string(),start_unix_secs:terminal_a.finalized_at_unix_secs-300,end_unix_secs:terminal_a.finalized_at_unix_secs+300 }).collect();
        let online_windows=usage.summarize_usage_by_provider_api_key_windows(&requested_windows).await.unwrap();
        assert_eq!(online_windows.iter().map(|row|(row.request_count,row.total_tokens,row.total_cost_usd)).collect::<Vec<_>>(),vec![(1,5,0.07),(1,5,0.06)]);
        let online: Vec<(String,i64,i64,i64,i64,f64)> = sqlx::query_as("SELECT id,request_count::bigint,success_count::bigint,error_count::bigint,total_tokens::bigint,total_cost_usd::double precision FROM provider_api_keys ORDER BY id").fetch_all(&first).await.unwrap();
        assert_eq!(online,vec![("pk-a".to_string(),1,0,1,5,0.07),("pk-b".to_string(),1,1,0,5,0.06)]);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM provider_api_keys WHERE last_used_at IS NOT NULL").fetch_one(&first).await.unwrap(),2);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT total_requests::bigint FROM api_keys WHERE id='key-a'").fetch_one(&first).await.unwrap(),1);
        let window=serde_json::json!({"quota":{"provider_type":"codex","windows":[{"code":"5h","window_minutes":300,"reset_at":terminal_a.finalized_at_unix_secs+300}]}});
        sqlx::query("UPDATE provider_api_keys SET status_snapshot=$1").bind(window).execute(&first).await.unwrap();
        usage.rebuild_provider_api_key_usage_stats().await.unwrap();
        let rebuilt: Vec<(String,i64,i64,i64,i64,f64)> = sqlx::query_as("SELECT id,request_count::bigint,success_count::bigint,error_count::bigint,total_tokens::bigint,total_cost_usd::double precision FROM provider_api_keys ORDER BY id").fetch_all(&first).await.unwrap(); assert_eq!(rebuilt,online);
        let windows:Vec<(i64,i64,f64)>=sqlx::query_as("SELECT (status_snapshot::jsonb #>> '{quota,windows,0,usage,request_count}')::bigint,(status_snapshot::jsonb #>> '{quota,windows,0,usage,total_tokens}')::bigint,(status_snapshot::jsonb #>> '{quota,windows,0,usage,total_cost_usd}')::double precision FROM provider_api_keys ORDER BY id").fetch_all(&first).await.unwrap();assert_eq!(windows,vec![(1,5,0.07),(1,5,0.06)]);
        // Retiring all delivery rows must not retire durable financial replay identity.
        usage.cleanup_processed_usage_counter_deltas(terminal_a.finalized_at_unix_secs+3600,1000).await.unwrap();
        repo.record_request_attempt_funds_outcome(terminal_a).await.unwrap(); repo.record_request_attempt_funds_outcome(terminal_b).await.unwrap();
        assert_eq!(balance(&first).await,0.07); assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM usage_counter_deltas").fetch_one(&first).await.unwrap(),0);
    }).catch_unwind().await;
    cleanup(admin, first, second, schema).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires an isolated AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn live_attempt_funds_admission_entitlements_and_concurrent_settlement() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let repo=SqlxSettlementRepository::new(first.clone()); let other=SqlxSettlementRepository::new(second.clone()); let usage=SqlxUsageReadRepository::new(first.clone());
        assert!(repo.reserve_request_attempt_funds(quote("missing","a")).await.is_err());
        usage.upsert(parent("limited")).await.unwrap(); sqlx::query("UPDATE wallets SET balance=0.10 WHERE id='wallet'").execute(&first).await.unwrap();
        let a=quote("limited","a"); let b=quote("limited","b"); repo.reserve_request_attempt_funds(a.clone()).await.unwrap(); repo.mark_request_attempt_funds_dispatched(a.identity()).await.unwrap(); repo.record_request_attempt_funds_outcome(facts(&a,None,false)).await.unwrap();
        assert_eq!(repo.reserve_request_attempt_funds(b).await.unwrap(),ReserveRequestAttemptFundsOutcome::Insufficient{available_cost_units:2_000_000});
        let mut nocharge=facts(&a,None,false); nocharge.facts.outcome=RequestAttemptFinancialOutcome::NoCharge; repo.record_request_attempt_funds_outcome(nocharge).await.unwrap();
        usage.upsert(parent("collision")).await.unwrap();let mut collision=quote("collision","b");collision.attempt_id=a.attempt_id.clone();
        assert_eq!(repo.reserve_request_attempt_funds(collision.clone()).await.unwrap(),ReserveRequestAttemptFundsOutcome::Conflict);
        let mut legacy=collision.quote.clone();legacy.identity.request_id="legacy-token".to_string();repo.reserve_request_funds(legacy.clone()).await.unwrap();collision.attempt_id=uuid::Uuid::new_v4().to_string();
        assert_eq!(repo.reserve_request_attempt_funds(collision).await.unwrap(),ReserveRequestAttemptFundsOutcome::Conflict);repo.release_request_funds(ReleaseRequestFundsInput{identity:legacy.identity,terminal_no_charge:false}).await.unwrap();
        let mut wrong=a.identity();wrong.request.user_id=Some("forged".to_string());assert!(repo.read_request_attempt_funds(wrong).await.is_err());
        let grant=serde_json::json!([{"type":"daily_quota","daily_quota_usd":0.20,"reset_timezone":"UTC","allow_wallet_overage":false}]);
        sqlx::query("INSERT INTO billing_plans (id,title,price_amount,duration_unit,duration_value,entitlements_json,created_at,updated_at) VALUES ('plan','plan',1,'day',1,$1,NOW(),NOW())").bind(&grant).execute(&first).await.unwrap();
        sqlx::query("INSERT INTO user_plan_entitlements (id,user_id,plan_id,payment_order_id,starts_at,expires_at,entitlements_snapshot,status,created_at,updated_at) VALUES ('grant','owner','plan','order',NOW()-INTERVAL '1 hour',NOW()+INTERVAL '1 hour',$1,'active',NOW(),NOW())").bind(&grant).execute(&first).await.unwrap();
        usage.upsert(parent("entitled")).await.unwrap(); let a=quote("entitled","a"); let b=quote("entitled","b");
        for q in [&a,&b] { repo.reserve_request_attempt_funds(q.clone()).await.unwrap(); repo.mark_request_attempt_funds_dispatched(q.identity()).await.unwrap(); }
        let (left,right)=tokio::join!(repo.record_request_attempt_funds_outcome(facts(&a,Some(7_000_000),true)),other.record_request_attempt_funds_outcome(facts(&b,Some(6_000_000),true))); left.unwrap();right.unwrap();
        let rows: (i64,i64) = sqlx::query_as("SELECT COUNT(*), (SUM(amount_usd)*100000000)::bigint FROM entitlement_usage_ledgers WHERE request_id='entitled'").fetch_one(&first).await.unwrap(); assert_eq!(rows,(2,13_000_000)); assert_eq!(balance(&first).await,0.10);
        sqlx::raw_sql("INSERT INTO entitlement_usage_ledgers (id,user_entitlement_id,user_id,request_id,amount_usd,balance_before,balance_after,usage_date,created_at) VALUES ('legacy-a','grant','owner','legacy',0.01,0.02,0.01,'2026-01-01',NOW()) ON CONFLICT (user_entitlement_id,request_id) WHERE attempt_id IS NULL DO NOTHING; INSERT INTO entitlement_usage_ledgers (id,user_entitlement_id,user_id,request_id,amount_usd,balance_before,balance_after,usage_date,created_at) VALUES ('legacy-b','grant','owner','legacy',0.01,0.02,0.01,'2026-01-01',NOW()) ON CONFLICT (user_entitlement_id,request_id) WHERE attempt_id IS NULL DO NOTHING").execute(&first).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM entitlement_usage_ledgers WHERE request_id='legacy'").fetch_one(&first).await.unwrap(),1);
        usage.upsert(parent("capped")).await.unwrap();let mut capped=quote("capped","a");capped.quote.authorized_cost_units=5_000_000;repo.reserve_request_attempt_funds(capped.clone()).await.unwrap();repo.mark_request_attempt_funds_dispatched(capped.identity()).await.unwrap();
        let excess=facts(&capped,Some(6_000_000),true);let stored=repo.record_request_attempt_funds_outcome(excess.clone()).await.unwrap().unwrap();assert_eq!(stored.funds.collected_cost_units,5_000_000);assert_eq!(stored.funds.state,RequestFundsState::ReconciliationPending);
        let capped_summary=repo.close_request_funds_admission(CloseRequestFundsAdmissionInput{identity:capped.identity(),closed_at_unix_secs:excess.finalized_at_unix_secs}).await.unwrap().unwrap();assert!(capped_summary.requires_reconciliation);assert_eq!(capped_summary.held_cost_units,0);repo.record_request_attempt_funds_outcome(excess).await.unwrap();
        usage.upsert(parent("prepared")).await.unwrap();let mut prepared=quote("prepared","a");prepared.quote.authorized_cost_units=1_000_000;repo.reserve_request_attempt_funds(prepared.clone()).await.unwrap();let mut cancelled=facts(&prepared,None,false);cancelled.facts.outcome=RequestAttemptFinancialOutcome::NoCharge;repo.record_request_attempt_funds_outcome(cancelled).await.unwrap();assert_eq!(summary(&first,"prepared").await.held_cost_units,0);
    }).catch_unwind().await;
    cleanup(admin, first, second, schema).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
