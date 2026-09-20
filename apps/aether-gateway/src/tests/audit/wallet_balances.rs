//! Authenticated wallet adjustment and manual-recharge audit acceptance.
use std::time::Duration;

use aether_data::driver::postgres::PostgresAuditLogReadRepository;
use aether_data::repository::wallet::{
    AdjustWalletBalanceInput, CreateManualWalletRechargeInput, InMemoryWalletRepository,
    StoredWalletSnapshot, WalletLookupKey, WalletReadRepository, WalletWriteRepository,
};
use aether_data_contracts::repository::audit::{
    AdminAuditDeliveryFailureCode, AdminAuditDeliveryFailureOutcome, AuditLogWriteRepository,
    CreateAdminAuditLog,
};
use axum::http::StatusCode;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::data::{GatewayDataConfig, GatewayDataState};
use crate::tests::{
    authenticated_operational_client_with_builder, build_router_with_state, start_server, AppState,
    OPERATIONAL_ADMIN_DEVICE_ID,
};

fn client(token: &str) -> reqwest::Client {
    authenticated_operational_client_with_builder(
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10)),
        token,
    )
}

async fn snapshot(pool: &PgPool) -> [Vec<Value>; 14] {
    let mut rows: [Vec<Value>; 14] = Default::default();
    for (slot, query) in rows.iter_mut().zip([
        "SELECT to_jsonb(w) FROM wallets w ORDER BY id",
        "SELECT to_jsonb(t) FROM wallet_transactions t ORDER BY id",
        "SELECT to_jsonb(o) FROM payment_orders o ORDER BY id",
        "SELECT to_jsonb(p) FROM wallet_http_write_probe p ORDER BY changed_at,balance",
        "SELECT to_jsonb(j) FROM recharge_recovery_jobs j ORDER BY id",
        "SELECT to_jsonb(c) FROM recharge_recovery_candidates c ORDER BY job_id,request_id",
        "SELECT to_jsonb(o) FROM recharge_recovery_operations o ORDER BY job_id,operation_seq",
        "SELECT to_jsonb(n) FROM recharge_recovery_notifications n ORDER BY id",
        "SELECT to_jsonb(r) FROM request_fund_recoveries r ORDER BY request_id",
        "SELECT to_jsonb(r) FROM request_fund_collection_receipts r ORDER BY id",
        "SELECT to_jsonb(u) FROM usage u ORDER BY id",
        "SELECT to_jsonb(s) FROM usage_settlement_snapshots s ORDER BY request_id",
        "SELECT to_jsonb(a) FROM recharge_recovery_activation a ORDER BY version",
        "SELECT jsonb_build_object('event_id',event_id,'payload',payload) FROM admin_audit_delivery ORDER BY event_id",
    ]) {
        *slot = sqlx::query_scalar(query).fetch_all(pool).await.unwrap();
    }
    rows
}

async fn intents(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT payload FROM admin_audit_delivery WHERE payload->'event_metadata'->>'event_name'
           IN ('admin_wallet_balance_adjusted','admin_wallet_manual_recharge_created')
         ORDER BY created_at,event_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

fn assert_audit(audit: &Value, actor: &str, recharge: bool, secrets: &[&str]) {
    assert_eq!(audit["user_id"], actor);
    assert_eq!(audit["request_id"], audit["id"]);
    assert_eq!(audit["status_code"], 200);
    let metadata = &audit["event_metadata"];
    assert_eq!(metadata["status"], "completed");
    assert_eq!(metadata["method"], "POST");
    assert_eq!(metadata["route_family"], "wallets_manage");
    assert_eq!(metadata["target_type"], "wallet");
    assert_eq!(metadata["target_id"], "wallet-http-audit");
    assert_eq!(
        metadata["event_name"],
        if recharge {
            "admin_wallet_manual_recharge_created"
        } else {
            "admin_wallet_balance_adjusted"
        }
    );
    assert_eq!(
        metadata["action"],
        if recharge {
            "create_manual_wallet_recharge"
        } else {
            "adjust_wallet_balance"
        }
    );
    assert_eq!(
        metadata["route_kind"],
        if recharge {
            "recharge_balance"
        } else {
            "adjust_balance"
        }
    );
    assert_eq!(
        metadata["path"],
        if recharge {
            "/api/admin/wallets/[wallet_id]/recharge"
        } else {
            "/api/admin/wallets/[wallet_id]/adjust"
        }
    );
    assert!(audit["user_agent"].is_null());
    assert!(audit["error_message"].is_null());
    let serialized = audit.to_string();
    for secret in secrets {
        assert!(
            !serialized.contains(secret),
            "unapproved body or credential data in audit"
        );
    }
}

async fn delivery_snapshot(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar("SELECT to_jsonb(d) FROM admin_audit_delivery d ORDER BY event_id")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn assert_recovery_jobs(pool: &PgPool, expected: i64, owner: &str) {
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM recharge_recovery_jobs")
            .fetch_one(pool)
            .await
            .unwrap(),
        expected
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM recharge_recovery_candidates")
            .fetch_one(pool)
            .await
            .unwrap(),
        expected
    );
    let valid: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM recharge_recovery_jobs j
         JOIN recharge_recovery_candidates c ON c.job_id=j.id
         JOIN payment_orders p ON p.id=j.payment_order_id
         JOIN wallet_transactions t ON t.id=j.source_transaction_id
         WHERE c.request_id='wallet-audit-legacy-debt' AND j.user_id=$1
           AND j.id=t.id AND t.wallet_id=j.wallet_id AND t.link_id=p.id
           AND p.wallet_id=j.wallet_id AND p.status='credited'
           AND t.category='recharge' AND t.reason_code='topup_admin_manual'
           AND j.principal_cost_units=500000000 AND j.collected_cost_units=0
           AND j.state='pending' AND j.activation_version=1",
    )
    .bind(owner)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        valid, expected,
        "one correct job/candidate per real manual-recharge receipt"
    );
}

async fn enable_recovery_with_legacy_debt(pool: &PgPool, owner: &str) {
    // Use the real migrated/bootstrap trigger, never a replacement enqueue.
    sqlx::query("UPDATE recharge_recovery_activation SET enabled=true,activated_at=clock_timestamp()-interval '1 second' WHERE version=1")
        .execute(pool).await.unwrap();
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT enabled FROM recharge_recovery_activation WHERE version=1"
    )
    .fetch_one(pool)
    .await
    .unwrap());
    let trigger: (bool, bool, String) = sqlx::query_as(
        "SELECT tgdeferrable,tginitdeferred,tgenabled::text FROM pg_trigger
         WHERE tgrelid='public.wallet_transactions'::regclass AND tgname='enqueue_recharge_debt_recovery'",
    ).fetch_one(pool).await.unwrap();
    assert_eq!(trigger, (true, true, "O".to_string()));
    sqlx::query("INSERT INTO api_keys(id,user_id,key_hash,name) VALUES('wallet-audit-debt-key',$1,'wallet-audit-fixture-hash','audit debt fixture')")
        .bind(owner).execute(pool).await.unwrap();
    // Same evidenced historical liability shape as native_restore_tests::debt.
    sqlx::query(
        "INSERT INTO usage(id,request_id,user_id,api_key_id,provider_name,model,status,
         billing_status,billing_mode,actual_total_cost_usd,total_cost_usd,request_metadata)
         VALUES('wallet-audit-legacy-usage','wallet-audit-legacy-debt',$1,'wallet-audit-debt-key',
         'synthetic-provider','synthetic-image','completed','insufficient_quota','legacy',0.03,0.03,$2)",
    ).bind(owner).bind(serde_json::json!({"settlement_snapshot":{
        "status":"complete","synthetic":true,"frozen_cost_units":3000000
    }})).execute(pool).await.unwrap();
    assert_recovery_jobs(pool, 0, owner).await;
}

async fn reject_deferred_recovery_commit(pool: &PgPool) {
    sqlx::raw_sql(
        "CREATE SEQUENCE wallet_audit_deferred_commit_seen;
         CREATE FUNCTION wallet_audit_reject_deferred_commit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           IF NEW.request_id <> 'wallet-audit-legacy-debt'
              OR NOT EXISTS (SELECT 1 FROM recharge_recovery_jobs j
                 JOIN payment_orders p ON p.id=j.payment_order_id
                 JOIN wallet_transactions t ON t.id=j.source_transaction_id
                 WHERE j.id=NEW.job_id AND p.status='credited' AND t.link_id=p.id)
              OR NOT EXISTS (SELECT 1 FROM admin_audit_delivery
                 WHERE payload->'event_metadata'->>'event_name'='admin_wallet_manual_recharge_created')
           THEN RAISE EXCEPTION 'wallet-deferred-fixture-preconditions-not-met'; END IF;
           -- Sequence writes intentionally survive rollback: prove HTTP failure
           -- reached the real deferred enqueue after business AND intent writes.
           PERFORM nextval('wallet_audit_deferred_commit_seen');
           RAISE EXCEPTION 'wallet-audit-deferred-commit-rejected';
         END $$;
         CREATE TRIGGER wallet_audit_reject_deferred_commit AFTER INSERT ON recharge_recovery_candidates
           FOR EACH ROW EXECUTE FUNCTION wallet_audit_reject_deferred_commit();",
    ).execute(pool).await.unwrap();
}

async fn remove_deferred_recovery_rejection(pool: &PgPool) {
    let reached: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM wallet_audit_deferred_commit_seen")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        reached,
        (1, true),
        "the failing COMMIT reached one real recovery candidate after intent insertion"
    );
    sqlx::raw_sql(
        "DROP TRIGGER wallet_audit_reject_deferred_commit ON recharge_recovery_candidates;
         DROP FUNCTION wallet_audit_reject_deferred_commit();
         DROP SEQUENCE wallet_audit_deferred_commit_seen;",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires a fresh task-owned aether_admin_audit_* PostgreSQL database"]
async fn authenticated_wallet_mutations_enqueue_atomically_and_delivery_never_repeats_money_writes()
{
    let database_url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PgPool::connect(&database_url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("aether_admin_audit_"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pg_catalog.pg_tables WHERE schemaname='public'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0,
        "never clear an existing database"
    );
    aether_data::lifecycle::migrate::prepare_database_for_startup(&pool)
        .await
        .unwrap();
    aether_data::lifecycle::migrate::run_migrations(&pool)
        .await
        .unwrap();
    let mut state = AppState::new()
        .unwrap()
        .without_auth_user_store_for_tests()
        .without_auth_session_store_for_tests()
        .with_data_state_for_tests(
            GatewayDataState::from_config(GatewayDataConfig::from_postgres_url(
                database_url,
                false,
            ))
            .unwrap(),
        );
    // The default test wallet store otherwise bypasses real PostgreSQL mutations.
    state.auth_wallet_store = None;
    let (admin_token, admin) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "admin",
        )
        .await;
    let (ordinary_token, owner) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "user",
        )
        .await;
    let (restricted_token, _) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "audit_admin",
        )
        .await;
    sqlx::query("INSERT INTO wallets(id,user_id,balance,gift_balance,total_recharged,created_at,updated_at) VALUES('wallet-http-audit',$1,10,3,20,now(),now())")
        .bind(&owner.id).execute(&pool).await.unwrap();
    sqlx::raw_sql(
        "CREATE TABLE wallet_http_write_probe(balance numeric,changed_at timestamptz);
         CREATE FUNCTION wallet_http_probe() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN INSERT INTO wallet_http_write_probe VALUES(NEW.balance,NEW.updated_at); RETURN NEW; END $$;
         CREATE TRIGGER wallet_http_probe AFTER UPDATE ON wallets
           FOR EACH ROW EXECUTE FUNCTION wallet_http_probe();",
    ).execute(&pool).await.unwrap();
    let admin_client = client(&admin_token);
    let (gateway, server) = start_server(build_router_with_state(state.clone())).await;
    let adjust_endpoint = format!("{gateway}/api/admin/wallets/wallet-http-audit/adjust");
    let recharge_endpoint = format!("{gateway}/api/admin/wallets/wallet-http-audit/recharge");
    let body_secret = "private-wallet-body-description";
    let adjust_payload =
        json!({"amount_usd":4.0,"balance_type":"recharge","description":body_secret});
    let recharge_payload =
        json!({"amount_usd":5.0,"payment_method":"admin_manual","description":body_secret});
    enable_recovery_with_legacy_debt(&pool, &owner.id).await;
    let initial_deliveries = delivery_snapshot(&pool).await;
    let initial = snapshot(&pool).await;
    let anonymous = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    for (endpoint, payload) in [
        (&adjust_endpoint, &adjust_payload),
        (&recharge_endpoint, &recharge_payload),
    ] {
        assert_eq!(
            anonymous
                .post(endpoint)
                .json(payload)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client(&ordinary_token)
                .post(endpoint)
                .json(payload)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client(&restricted_token)
                .post(endpoint)
                .json(payload)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            admin_client
                .post(endpoint)
                .json(&json!({"amount_usd":0}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        admin_client
            .post(format!("{gateway}/api/admin/wallets/missing-wallet/adjust"))
            .json(&adjust_payload)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(snapshot(&pool).await, initial);
    assert!(intents(&pool).await.is_empty());
    sqlx::raw_sql(
        "CREATE FUNCTION wallet_http_reject_intent() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'private-wallet-intent-failure'; END $$;
         CREATE TRIGGER wallet_http_reject_intent BEFORE INSERT ON admin_audit_delivery
           FOR EACH ROW EXECUTE FUNCTION wallet_http_reject_intent();",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (endpoint, payload) in [
        (&adjust_endpoint, &adjust_payload),
        (&recharge_endpoint, &recharge_payload),
    ] {
        assert_eq!(
            admin_client
                .post(endpoint)
                .json(payload)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            snapshot(&pool).await,
            initial,
            "no wallet/ledger/order/probe commit without an intent"
        );
        assert!(intents(&pool).await.is_empty());
    }
    sqlx::raw_sql(
        "DROP TRIGGER wallet_http_reject_intent ON admin_audit_delivery;
         DROP FUNCTION wallet_http_reject_intent();",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(delivery_snapshot(&pool).await, initial_deliveries);
    reject_deferred_recovery_commit(&pool).await;
    let response = admin_client
        .post(&recharge_endpoint)
        .json(&recharge_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    remove_deferred_recovery_rejection(&pool).await;
    assert_eq!(
        snapshot(&pool).await,
        initial,
        "real deferred recovery COMMIT error must roll back wallet/order/intent/jobs/candidates"
    );
    assert_eq!(delivery_snapshot(&pool).await, initial_deliveries);
    let query_secret = "private-wallet-query-token";
    let cookie_secret = "private-wallet-cookie-token";
    let trace_secret = "external-wallet-trace-".repeat(20);
    let secrets = [
        body_secret,
        query_secret,
        cookie_secret,
        trace_secret.as_str(),
        admin_token.as_str(),
        "private-wallet-user-agent",
    ];
    let response = admin_client
        .post(format!("{adjust_endpoint}?token={query_secret}"))
        .header("cookie", format!("fixture_cookie={cookie_secret}"))
        .header(crate::constants::TRACE_ID_HEADER, &trace_secret)
        .header("user-agent", "private-wallet-user-agent")
        .json(&adjust_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["wallet"]["recharge_balance"], 14.0);
    assert_eq!(body["wallet"]["gift_balance"], 3.0);
    assert_eq!(body["wallet"]["total_adjusted"], 4.0);
    let queued = intents(&pool).await;
    assert_eq!(queued.len(), 1);
    assert_audit(&queued[0], &admin.id, false, &secrets);
    let first_id = queued[0]["id"].as_str().unwrap();
    let canonical: Value = sqlx::query_scalar("SELECT to_jsonb(a) FROM audit_logs a WHERE id=$1")
        .bind(first_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_audit(&canonical, &admin.id, false, &secrets);
    let delivery = PostgresAuditLogReadRepository::new(pool.clone());
    let claim = delivery
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(claim.event_id, first_id);
    let adjusted = snapshot(&pool).await;
    assert!(delivery
        .deliver_admin_audit(first_id, claim.lease_token)
        .await
        .unwrap());
    assert_eq!(snapshot(&pool).await, adjusted);
    sqlx::raw_sql(
        "CREATE FUNCTION wallet_http_reject_delivery() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'private-wallet-delivery-failure'; END $$;
         CREATE TRIGGER wallet_http_reject_delivery BEFORE INSERT ON audit_logs
           FOR EACH ROW EXECUTE FUNCTION wallet_http_reject_delivery();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let response = admin_client
        .post(format!("{recharge_endpoint}?token={query_secret}"))
        .header("cookie", format!("fixture_cookie={cookie_secret}"))
        .header(crate::constants::TRACE_ID_HEADER, &trace_secret)
        .json(&recharge_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["wallet"]["recharge_balance"], 19.0);
    assert_eq!(body["wallet"]["total_recharged"], 25.0);
    assert_eq!(body["payment_order"]["status"], "credited");
    let first_order = body["payment_order"]["order_no"]
        .as_str()
        .unwrap()
        .to_string();
    let queued = intents(&pool).await;
    assert_eq!(queued.len(), 2);
    let recharge_audit = queued
        .iter()
        .find(|audit| {
            audit["event_metadata"]["event_name"] == "admin_wallet_manual_recharge_created"
        })
        .unwrap();
    assert_audit(recharge_audit, &admin.id, true, &secrets);
    let recharge_id = recharge_audit["id"].as_str().unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(recharge_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    let committed = snapshot(&pool).await;
    assert_eq!(committed[1].len(), 2);
    assert_eq!(committed[2].len(), 1);
    assert_eq!(committed[3].len(), 2);
    assert_recovery_jobs(&pool, 1, &owner.id).await;
    let claim = delivery
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(claim.event_id, recharge_id);
    assert!(delivery
        .deliver_admin_audit(recharge_id, claim.lease_token)
        .await
        .is_err());
    assert_eq!(
        delivery
            .fail_admin_audit_delivery(
                recharge_id,
                claim.lease_token,
                AdminAuditDeliveryFailureCode::AuditInsertFailed
            )
            .await
            .unwrap(),
        AdminAuditDeliveryFailureOutcome::RetryScheduled
    );
    assert_eq!(snapshot(&pool).await, committed);
    sqlx::raw_sql(
        "DROP TRIGGER wallet_http_reject_delivery ON audit_logs;
         DROP FUNCTION wallet_http_reject_delivery();",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE admin_audit_delivery SET next_attempt_at=clock_timestamp() WHERE event_id=$1",
    )
    .bind(recharge_id)
    .execute(&pool)
    .await
    .unwrap();
    drop(delivery);
    let restarted = PostgresAuditLogReadRepository::new(pool.clone());
    let retry = restarted
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(restarted
        .deliver_admin_audit(recharge_id, retry.lease_token)
        .await
        .unwrap());
    assert!(!restarted
        .deliver_admin_audit(recharge_id, retry.lease_token)
        .await
        .unwrap());
    assert_eq!(snapshot(&pool).await, committed);
    assert_recovery_jobs(&pool, 1, &owner.id).await;
    let canonical: Value = sqlx::query_scalar("SELECT to_jsonb(a) FROM audit_logs a WHERE id=$1")
        .bind(recharge_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_audit(&canonical, &admin.id, true, &secrets);

    // Preserve the current HTTP contract: identical requests are new monetary
    // operations, and manual recharge generates a fresh order number each time.
    let response = admin_client
        .post(&recharge_endpoint)
        .json(&recharge_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_ne!(body["payment_order"]["order_no"], first_order);
    assert_eq!(body["wallet"]["recharge_balance"], 24.0);
    assert_eq!(body["wallet"]["total_recharged"], 30.0);
    let response = admin_client
        .post(&adjust_endpoint)
        .json(&adjust_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["wallet"]["recharge_balance"], 28.0);
    assert_eq!(body["wallet"]["total_adjusted"], 8.0);
    assert_eq!(intents(&pool).await.len(), 4);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM audit_logs WHERE event_metadata->>'event_name'
         IN ('admin_wallet_balance_adjusted','admin_wallet_manual_recharge_created')",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        4,
        "no duplicate finalizer event ID"
    );
    let final_snapshot = snapshot(&pool).await;
    assert_eq!(final_snapshot[1].len(), 4);
    assert_eq!(final_snapshot[2].len(), 2);
    assert_eq!(final_snapshot[3].len(), 4);
    assert_recovery_jobs(&pool, 2, &owner.id).await;
    // Fail only the post-commit quota projection: manual recharge does not
    // reference this column, while the response enrichment SELECT does.
    let known_ids = intents(&pool)
        .await
        .into_iter()
        .map(|audit| audit["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    sqlx::query("ALTER TABLE user_plan_entitlements RENAME COLUMN entitlements_snapshot TO audit_fixture_hidden_snapshot")
        .execute(&pool).await.unwrap();
    let failed_projection = admin_client
        .post(&recharge_endpoint)
        .json(&recharge_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(failed_projection.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        failed_projection
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("1")
    );
    let failure_body: Value = failed_projection.json().await.unwrap();
    assert_eq!(failure_body["error"]["code"], "control_unavailable");
    assert_eq!(failure_body["error"]["retryable"], true);
    assert_eq!(
        failure_body["error"]["failover_disposition"],
        "retry_request"
    );
    sqlx::query("ALTER TABLE user_plan_entitlements RENAME COLUMN audit_fixture_hidden_snapshot TO entitlements_snapshot")
        .execute(&pool).await.unwrap();
    let after_projection_failure = snapshot(&pool).await;
    assert_eq!(after_projection_failure[1].len(), 5);
    assert_eq!(after_projection_failure[2].len(), 3);
    assert_eq!(after_projection_failure[3].len(), 5);
    assert_recovery_jobs(&pool, 3, &owner.id).await;
    assert_eq!(
        state
            .find_wallet(WalletLookupKey::WalletId("wallet-http-audit"))
            .await
            .unwrap()
            .unwrap()
            .balance,
        33.0,
        "HTTP 502 did not roll back committed credit"
    );
    let queued = intents(&pool).await;
    assert_eq!(queued.len(), 5);
    let committed_intent = queued
        .iter()
        .find(|audit| {
            !known_ids
                .iter()
                .any(|id| audit["id"].as_str() == Some(id.as_str()))
        })
        .unwrap();
    assert_audit(committed_intent, &admin.id, true, &secrets);
    let committed_id = committed_intent["id"].as_str().unwrap();
    let claims = restarted
        .claim_admin_audit_deliveries(10, 30)
        .await
        .unwrap();
    assert_eq!(
        claims.len(),
        3,
        "two earlier successful requests plus the failed projection"
    );
    for claim in claims {
        assert!(restarted
            .deliver_admin_audit(&claim.event_id, claim.lease_token)
            .await
            .unwrap());
    }
    assert_eq!(
        snapshot(&pool).await,
        after_projection_failure,
        "recovery must not repeat the credit after HTTP 502"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(committed_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    let response = admin_client
        .post(&recharge_endpoint)
        .json(&recharge_payload)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["wallet"]["recharge_balance"], 38.0,
        "retrying the HTTP request after 502 remains a new monetary operation"
    );
    assert_eq!(body["wallet"]["total_recharged"], 40.0);
    assert_recovery_jobs(&pool, 4, &owner.id).await;
    server.abort();
    pool.close().await;
}

#[tokio::test]
async fn wallet_audit_unsupported_adapter_is_non_mutating_and_http_fallback_is_preserved() {
    let wallet = StoredWalletSnapshot::new(
        "wallet-memory-audit".to_string(),
        Some("memory-owner".to_string()),
        None,
        10.0,
        3.0,
        "finite".to_string(),
        "USD".to_string(),
        "active".to_string(),
        20.0,
        0.0,
        0.0,
        0.0,
        1_710_000_000,
    )
    .unwrap();
    let repository = InMemoryWalletRepository::seed([wallet.clone()]);
    let audit = CreateAdminAuditLog {
        id: uuid::Uuid::now_v7().to_string(),
        event_type: "admin_mutation".to_string(),
        user_id: None,
        api_key_id: None,
        description: "fallback capability fixture".to_string(),
        ip_address: None,
        user_agent: None,
        request_id: None,
        event_metadata: None,
        status_code: Some(200),
        error_message: None,
        created_at: chrono::Utc::now(),
    };
    assert!(repository
        .adjust_wallet_balance_with_audit(
            AdjustWalletBalanceInput {
                wallet_id: wallet.id.clone(),
                amount_usd: 4.0,
                balance_type: "recharge".to_string(),
                operator_id: None,
                description: None,
            },
            &audit
        )
        .await
        .unwrap()
        .is_none());
    assert!(repository
        .create_manual_wallet_recharge_with_audit(
            CreateManualWalletRechargeInput {
                wallet_id: wallet.id.clone(),
                amount_usd: 5.0,
                payment_method: "admin_manual".to_string(),
                operator_id: None,
                description: None,
                order_no: "memory-order".to_string(),
            },
            &audit
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repository
            .find(WalletLookupKey::WalletId(&wallet.id))
            .await
            .unwrap()
            .unwrap(),
        wallet
    );
    // The existing in-memory repository does not implement monetary writes;
    // Gateway's established test store exercises the legacy successful fallback.
    let state = AppState::new()
        .unwrap()
        .with_auth_wallets_for_tests([wallet]);
    let (token, _) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "admin",
        )
        .await;
    let (gateway, server) = start_server(build_router_with_state(state.clone())).await;
    let admin_client = client(&token);
    for (suffix, payload, balance) in [
        (
            "adjust",
            json!({"amount_usd":4.0,"balance_type":"recharge"}),
            14.0,
        ),
        (
            "recharge",
            json!({"amount_usd":5.0,"payment_method":"admin_manual"}),
            19.0,
        ),
    ] {
        let response = admin_client
            .post(format!(
                "{gateway}/api/admin/wallets/wallet-memory-audit/{suffix}"
            ))
            .json(&payload)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["wallet"]["recharge_balance"], balance);
    }
    assert_eq!(
        state
            .find_wallet(WalletLookupKey::WalletId("wallet-memory-audit"))
            .await
            .unwrap()
            .unwrap()
            .balance,
        19.0
    );
    server.abort();
}
