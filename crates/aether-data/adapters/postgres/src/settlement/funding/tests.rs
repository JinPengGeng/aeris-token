use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};

use super::*;
use crate::{SqlxSettlementRepository, SqlxWalletRepository};
use aether_data_contracts::repository::wallet::{
    AdjustWalletBalanceInput, ProcessAdminWalletRefundInput, WalletMutationOutcome,
    WalletWriteRepository,
};

async fn fixture() -> (PgPool, PgPool, PgPool, String) {
    let database_url = std::env::var("AETHER_TEST_DATABASE_URL")
        .expect("AETHER_TEST_DATABASE_URL must name a disposable PostgreSQL database");
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .unwrap();
    crate::run_migrations(&admin).await.unwrap();
    let schema = format!("request_funds_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = database_url
        .parse::<sqlx::postgres::PgConnectOptions>()
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let first = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .unwrap();
    let second = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    for table in [
        "users",
        "api_keys",
        "wallets",
        "billing_plans",
        "user_plan_entitlements",
        "entitlement_usage_ledgers",
        "usage",
        "usage_settlement_snapshots",
        "usage_counter_deltas",
        "request_fund_reservations",
        "request_fund_allocations",
        "request_fund_recoveries",
        "request_fund_collection_receipts",
        "wallet_transactions",
        "refund_requests",
    ] {
        sqlx::query(&format!(
            "CREATE TABLE \"{table}\" (LIKE public.\"{table}\" INCLUDING ALL)"
        ))
        .execute(&first)
        .await
        .unwrap();
    }
    sqlx::raw_sql("INSERT INTO users (id,username,email_verified) VALUES ('owner','owner',false); \
        INSERT INTO api_keys (id,user_id,key_hash,name) VALUES ('key-a','owner','hash-a','a'),('key-b','owner','hash-b','b'); \
        INSERT INTO wallets (id,user_id,balance,gift_balance,status,limit_mode,created_at,updated_at) VALUES ('wallet','owner',0.10,0,'active','finite',NOW(),NOW())")
        .execute(&first).await.unwrap();
    let first_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&first)
        .await
        .unwrap();
    let second_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&second)
        .await
        .unwrap();
    assert_ne!(
        first_pid, second_pid,
        "race must use independent PostgreSQL connections"
    );
    (admin, first, second, schema)
}

fn quote(request: &str, key: &str, units: u64) -> ReserveRequestFundsInput {
    ReserveRequestFundsInput {
        identity: RequestFundsIdentity {
            reservation_token: format!("token-{request}"),
            request_id: request.to_string(),
            user_id: Some("owner".to_string()),
            api_key_id: Some(key.to_string()),
            api_key_is_standalone: false,
        },
        authorized_cost_units: units,
        pricing_snapshot: json!({"version":1,"unit_price":0.08}),
        admitted_at_unix_secs: chrono::Utc::now().timestamp() as u64,
    }
}

async fn persist_usage(
    pool: &PgPool,
    identity: &RequestFundsIdentity,
    cost: f64,
) -> UsageSettlementInput {
    sqlx::query("INSERT INTO \"usage\" (id,request_id,user_id,api_key_id,provider_name,model,status,billing_status,actual_total_cost_usd,total_cost_usd,request_metadata) \
        VALUES ($1,$2,$3,$4,'test-provider','image','completed','pending',$5,$5,$6)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(&identity.request_id).bind(&identity.user_id)
        .bind(&identity.api_key_id).bind(cost).bind(json!({"settlement_snapshot":{"status":"complete"}}))
        .execute(pool).await.unwrap();
    UsageSettlementInput {
        request_id: identity.request_id.clone(),
        user_id: identity.user_id.clone(),
        api_key_id: identity.api_key_id.clone(),
        api_key_is_standalone: identity.api_key_is_standalone,
        provider_id: None,
        status: "completed".to_string(),
        billing_status: "pending".to_string(),
        total_cost_usd: cost,
        actual_total_cost_usd: cost,
        finalized_at_unix_secs: Some(1_800_000_001),
    }
}

async fn balance(pool: &PgPool) -> f64 {
    sqlx::query_scalar("SELECT balance::double precision FROM wallets WHERE id='wallet'")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires an isolated AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn live_request_funds_reserve_settle_release_protect_shared_wallet_and_rollback() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let a = SqlxSettlementRepository::new(first.clone());
        let b = SqlxSettlementRepository::new(second.clone());
        let quote_a = quote("request-a", "key-a", 8_000_000);
        let quote_b = quote("request-b", "key-b", 8_000_000);
        let (result_a, result_b) = tokio::join!(a.reserve_request_funds(quote_a.clone()), b.reserve_request_funds(quote_b.clone()));
        let results = [result_a.unwrap(), result_b.unwrap()];
        assert_eq!(results.iter().filter(|outcome| matches!(outcome, ReserveRequestFundsOutcome::Reserved { .. })).count(), 1);
        assert_eq!(results.iter().filter(|outcome| matches!(outcome, ReserveRequestFundsOutcome::Insufficient { .. })).count(), 1);
        let winner = match &results[0] { ReserveRequestFundsOutcome::Reserved { .. } => quote_a, _ => quote_b };
        let replay = a.reserve_request_funds(winner.clone()).await.unwrap();
        assert!(matches!(replay, ReserveRequestFundsOutcome::Reserved { .. }));
        let mut changed = winner.clone(); changed.authorized_cost_units += 1;
        assert_eq!(a.reserve_request_funds(changed).await.unwrap(), ReserveRequestFundsOutcome::Conflict);
        assert_eq!(balance(&first).await, 0.10, "holds must not masquerade as consumption");

        let wallet = SqlxWalletRepository::new(first.clone());
        assert!(wallet.adjust_wallet_balance(AdjustWalletBalanceInput { wallet_id: "wallet".to_string(),
            amount_usd: -0.03, balance_type: "recharge".to_string(), operator_id: None, description: None }).await.is_err());
        sqlx::query("INSERT INTO refund_requests (id,refund_no,wallet_id,user_id,amount_usd,status,source_type,refund_mode,created_at,updated_at) VALUES ('refund','refund-no','wallet','owner',0.03,'approved','wallet','offline',NOW(),NOW())")
            .execute(&first).await.unwrap();
        assert!(matches!(wallet.process_admin_wallet_refund(ProcessAdminWalletRefundInput {
            wallet_id:"wallet".to_string(),refund_id:"refund".to_string(),operator_id:None }).await.unwrap(), WalletMutationOutcome::Invalid(_)));
        assert_eq!(balance(&first).await, 0.10);
        let ordinary = quote("ordinary", "key-a", 3_000_000);
        let ordinary_usage = persist_usage(&first, &ordinary.identity, 0.03).await;
        assert_eq!(a.settle_usage(ordinary_usage).await.unwrap().unwrap().billing_status, "insufficient_quota");
        assert_eq!(balance(&first).await, 0.10);

        a.mark_request_funds_dispatched(winner.identity.clone()).await.unwrap().unwrap();
        assert!(a.release_request_funds(ReleaseRequestFundsInput { identity: winner.identity.clone(), terminal_no_charge:false }).await.is_err());
        let usage = persist_usage(&first, &winner.identity, 0.04).await;
        assert!(a.settle_usage(usage.clone()).await.is_err(), "legacy settlement cannot bypass reservation identity");
        sqlx::raw_sql("CREATE FUNCTION reject_terminal_funds() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.state = 'settled' THEN RAISE EXCEPTION 'injected terminal write failure'; END IF; RETURN NEW; END $$; \
            CREATE TRIGGER reject_terminal_funds BEFORE UPDATE ON request_fund_reservations FOR EACH ROW EXECUTE FUNCTION reject_terminal_funds()")
            .execute(&first).await.unwrap();
        let finalize = FinalizeRequestFundsInput { identity: winner.identity.clone(), usage, reconciliation_facts: None };
        assert!(a.finalize_request_funds(finalize.clone()).await.is_err());
        assert_eq!(balance(&first).await, 0.10, "failure after wallet UPDATE must roll the debit back");
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT COALESCE(SUM(collected_cost_units),0)::bigint FROM request_fund_allocations").fetch_one(&first).await.unwrap(), 0);
        sqlx::query("DROP TRIGGER reject_terminal_funds ON request_fund_reservations").execute(&first).await.unwrap();
        let settled = a.finalize_request_funds(finalize.clone()).await.unwrap().unwrap();
        assert_eq!(settled.state, RequestFundsState::Settled);
        assert_eq!(settled.collected_cost_units, 4_000_000);
        assert!((balance(&first).await - 0.06).abs() < 1e-12);
        assert_eq!(b.finalize_request_funds(finalize).await.unwrap().unwrap(), settled);
        let followup = quote("followup", "key-b", 5_000_000);
        assert!(matches!(a.reserve_request_funds(followup.clone()).await.unwrap(), ReserveRequestFundsOutcome::Reserved { .. }));
        let released = a.release_request_funds(ReleaseRequestFundsInput { identity: followup.identity.clone(), terminal_no_charge:false }).await.unwrap().unwrap();
        assert_eq!(released.state, RequestFundsState::Released);
        assert_eq!(b.release_request_funds(ReleaseRequestFundsInput { identity:followup.identity.clone(),terminal_no_charge:false }).await.unwrap().unwrap(),released);
        assert!(a.mark_request_funds_dispatched(followup.identity).await.is_err());
    }).catch_unwind().await;
    first.close().await;
    second.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[tokio::test]
#[ignore = "requires an isolated AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn live_request_funds_recovery_collects_only_unreserved_funds_once() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let repo = SqlxSettlementRepository::new(first.clone());
        let debt = quote("debt", "key-a", 15_000_000);
        let usage = persist_usage(&first, &debt.identity, 0.15).await;
        assert_eq!(
            repo.settle_usage(usage)
                .await
                .unwrap()
                .unwrap()
                .billing_status,
            "insufficient_quota"
        );
        let other = quote("held", "key-b", 8_000_000);
        repo.reserve_request_funds(other.clone()).await.unwrap();
        let recover = RecoverInsufficientQuotaInput {
            request_id: "debt".to_string(),
        };
        let first_result = repo
            .recover_insufficient_quota(recover.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first_result.collected_cost_units, 2_000_000);
        assert_eq!(first_result.outstanding_cost_units, 13_000_000);
        assert_eq!(
            repo.recover_insufficient_quota(recover.clone())
                .await
                .unwrap()
                .unwrap(),
            first_result
        );
        // This is a fixture-only top-up; the real recovery operation never trusts a caller cost.
        sqlx::query("UPDATE wallets SET balance=balance+0.13 WHERE id='wallet'")
            .execute(&first)
            .await
            .unwrap();
        let second_result = repo
            .recover_insufficient_quota(recover.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second_result.collected_cost_units, 15_000_000);
        assert_eq!(second_result.outstanding_cost_units, 0);
        assert_eq!(second_result.settlement.billing_status, "settled");
        assert!(
            (balance(&first).await - 0.08).abs() < 1e-12,
            "other request's hold must survive recovery"
        );
        assert_eq!(
            repo.recover_insufficient_quota(recover)
                .await
                .unwrap()
                .unwrap(),
            second_result
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM request_fund_collection_receipts")
                .fetch_one(&first)
                .await
                .unwrap(),
            2
        );
    })
    .catch_unwind()
    .await;
    first.close().await;
    second.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[tokio::test]
#[ignore = "requires an isolated AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn live_request_funds_freeze_entitlement_day_and_recover_legacy_partial_debit() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let entitlements = json!([{"type":"daily_quota","daily_quota_usd":0.05,"reset_timezone":"UTC","allow_wallet_overage":true}]);
        sqlx::query("INSERT INTO billing_plans (id,title,price_amount,duration_unit,duration_value,entitlements_json,created_at,updated_at) VALUES ('plan','plan',1,'day',1,$1,NOW(),NOW())")
            .bind(&entitlements).execute(&first).await.unwrap();
        sqlx::query("INSERT INTO user_plan_entitlements (id,user_id,plan_id,payment_order_id,starts_at,expires_at,entitlements_snapshot,status,created_at,updated_at) VALUES ('grant','owner','plan','order',NOW()-INTERVAL '1 hour',NOW()+INTERVAL '1 hour',$1,'active',NOW(),NOW())")
            .bind(&entitlements).execute(&first).await.unwrap();
        let repo=SqlxSettlementRepository::new(first.clone());
        let request=quote("grant-funded","key-a",8_000_000);
        let reservation=match repo.reserve_request_funds(request.clone()).await.unwrap(){ReserveRequestFundsOutcome::Reserved{reservation}=>reservation,other=>panic!("unexpected {other:?}")};
        assert_eq!(reservation.allocations.len(),2);
        let date=match &reservation.allocations[0].source{RequestFundingSource::Entitlement{usage_date,..}=>usage_date.clone(),other=>panic!("unexpected {other:?}")};
        assert_eq!(reservation.allocations[0].reserved_cost_units,5_000_000);
        assert_eq!(reservation.allocations[1].reserved_cost_units,3_000_000);
        // New requests cannot borrow this grant, but the admitted request retains its frozen day.
        sqlx::query("UPDATE user_plan_entitlements SET status='expired',expires_at=NOW()-INTERVAL '1 minute' WHERE id='grant'").execute(&first).await.unwrap();
        repo.mark_request_funds_dispatched(request.identity.clone()).await.unwrap();
        let usage=persist_usage(&first,&request.identity,0.06).await;
        let settled=repo.finalize_request_funds(FinalizeRequestFundsInput{identity:request.identity,usage,reconciliation_facts:None}).await.unwrap().unwrap();
        assert_eq!(settled.collected_cost_units,6_000_000);
        assert!((balance(&first).await-0.09).abs()<1e-12);
        let ledger:(String,f64)=sqlx::query_as("SELECT usage_date,amount_usd::double precision FROM entitlement_usage_ledgers WHERE request_id='grant-funded'").fetch_one(&first).await.unwrap();
        assert_eq!(ledger,(date.clone(),0.05));

        let legacy=quote("legacy-partial","key-b",15_000_000);
        persist_usage(&first,&legacy.identity,0.15).await;
        sqlx::query("UPDATE \"usage\" SET billing_status='insufficient_quota' WHERE request_id='legacy-partial'").execute(&first).await.unwrap();
        sqlx::query("INSERT INTO entitlement_usage_ledgers (id,user_entitlement_id,user_id,request_id,amount_usd,balance_before,balance_after,usage_date,created_at) VALUES ('old-partial','grant','owner','legacy-partial',0.05,0.05,0,$1,NOW())")
            .bind(&date).execute(&first).await.unwrap();
        sqlx::query("UPDATE wallets SET balance=0.10 WHERE id='wallet'").execute(&first).await.unwrap();
        let input=RecoverInsufficientQuotaInput{request_id:"legacy-partial".to_string()};
        let recovered=repo.recover_insufficient_quota(input.clone()).await.unwrap().unwrap();
        assert_eq!(recovered.collected_cost_units,10_000_000);
        assert_eq!(recovered.outstanding_cost_units,0);
        assert_eq!(balance(&first).await,0.0);
        assert_eq!(repo.recover_insufficient_quota(input).await.unwrap().unwrap(),recovered);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM entitlement_usage_ledgers WHERE request_id='legacy-partial'").fetch_one(&first).await.unwrap(),1);
    }).catch_unwind().await;
    first.close().await;
    second.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[tokio::test]
#[ignore = "requires an isolated AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn live_request_funds_admission_time_controls_grant_eligibility_and_frozen_day() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let entitlements = json!([{"type":"daily_quota","daily_quota_usd":0.05,"reset_timezone":"UTC","allow_wallet_overage":true}]);
        sqlx::query("INSERT INTO billing_plans (id,title,price_amount,duration_unit,duration_value,entitlements_json,created_at,updated_at) VALUES ('plan','plan',1,'day',1,$1,NOW(),NOW())")
            .bind(&entitlements).execute(&first).await.unwrap();
        let database_now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT NOW()")
            .fetch_one(&first).await.unwrap();
        let midnight = database_now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
        let admitted_at = midnight - chrono::Duration::seconds(1);
        let admission_date = admitted_at.date_naive().to_string();
        assert_ne!(admission_date, database_now.date_naive().to_string());
        // Validity is starts_at <= admission < expires_at. The eligible grant
        // starts at admission exactly and has expired by the actual DB clock.
        // A second grant is eligible now but had not started at admission; a
        // third ends at admission exactly and must also be excluded.
        for (id, starts_at, expires_at) in [
            ("at-admission", admitted_at, midnight),
            ("not-started", midnight, database_now + chrono::Duration::days(1)),
            ("already-expired", admitted_at - chrono::Duration::days(1), admitted_at),
        ] {
            sqlx::query("INSERT INTO user_plan_entitlements (id,user_id,plan_id,payment_order_id,starts_at,expires_at,entitlements_snapshot,status,created_at,updated_at) VALUES ($1,'owner','plan',$1,$2,$3,$4,'active',NOW(),NOW())")
                .bind(id).bind(starts_at).bind(expires_at).bind(&entitlements)
                .execute(&first).await.unwrap();
        }
        sqlx::query("UPDATE wallets SET balance=0 WHERE id='wallet'")
            .execute(&first).await.unwrap();
        let repo = SqlxSettlementRepository::new(first.clone());
        let mut too_large = quote("midnight-insufficient", "key-a", 8_000_000);
        too_large.admitted_at_unix_secs = admitted_at.timestamp() as u64;
        assert_eq!(repo.reserve_request_funds(too_large).await.unwrap(),
            ReserveRequestFundsOutcome::Insufficient { available_cost_units: 5_000_000 });
        let mut request = quote("midnight-admission", "key-a", 3_000_000);
        request.admitted_at_unix_secs = admitted_at.timestamp() as u64;
        let reservation = match repo.reserve_request_funds(request.clone()).await.unwrap() {
            ReserveRequestFundsOutcome::Reserved { reservation } => reservation,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(reservation.allocations, vec![RequestFundsAllocation {
            source: RequestFundingSource::Entitlement {
                entitlement_id: "at-admission".to_string(),
                usage_date: admission_date.clone(),
                quota_cost_units: 5_000_000,
            },
            reserved_cost_units: 3_000_000,
        }]);
        assert_eq!(repo.reserve_request_funds(request.clone()).await.unwrap(),
            ReserveRequestFundsOutcome::Reserved { reservation });
        repo.mark_request_funds_dispatched(request.identity.clone()).await.unwrap();
        // Admission on the next day uses the newly started grant instead.
        let mut next_day = quote("next-day-admission", "key-b", 5_000_000);
        next_day.admitted_at_unix_secs = midnight.timestamp() as u64;
        let next_day_reservation = match repo.reserve_request_funds(next_day.clone()).await.unwrap() {
            ReserveRequestFundsOutcome::Reserved { reservation } => reservation,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(next_day_reservation.allocations[0].source,
            RequestFundingSource::Entitlement {
                entitlement_id: "not-started".to_string(),
                usage_date: midnight.date_naive().to_string(),
                quota_cost_units: 5_000_000,
            });
        repo.release_request_funds(ReleaseRequestFundsInput {
            identity: next_day.identity, terminal_no_charge: false,
        }).await.unwrap();
        sqlx::query("UPDATE user_plan_entitlements SET status='expired' WHERE id='at-admission'")
            .execute(&first).await.unwrap();
        let mut usage = persist_usage(&first, &request.identity, 0.03).await;
        usage.finalized_at_unix_secs = Some((midnight + chrono::Duration::seconds(1)).timestamp() as u64);
        let finalize = FinalizeRequestFundsInput {
            identity: request.identity,
            usage,
            reconciliation_facts: None,
        };
        let settled = repo.finalize_request_funds(finalize.clone()).await.unwrap().unwrap();
        assert_eq!(settled.collected_cost_units, 3_000_000);
        assert_eq!(repo.finalize_request_funds(finalize).await.unwrap(), Some(settled));
        let ledgers: Vec<(String, String, f64)> = sqlx::query_as("SELECT user_entitlement_id,usage_date,amount_usd::double precision FROM entitlement_usage_ledgers ORDER BY id")
            .fetch_all(&first).await.unwrap();
        assert_eq!(ledgers, vec![("at-admission".to_string(), admission_date, 0.03)]);
        assert_eq!(balance(&first).await, 0.0);
    }).catch_unwind().await;
    first.close().await;
    second.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}
