//! Synthetic end-to-end recharge: real PostgreSQL, authenticated HTTP and SMTP.
//! The historical completed usage is synthetic evidence, never a provider invoice.
use super::tests::{decoded_mime_bodies, smtp_server};
use super::*;
use crate::data::{state::StoredUserSessionRecord, GatewayDataState};
use crate::important_notification::{
    IMPORTANT_NOTIFICATION_ENABLED_KEY, IMPORTANT_NOTIFICATION_ITEMS_KEY,
};
use crate::local_auth_token::{create_local_auth_token, LocalAuthTokenType};
use aether_data::driver::postgres::{
    run_migrations, SqlxSettlementRepository, SqlxUserReadRepository, SqlxWalletRepository,
};
use aether_data_contracts::repository::wallet::{
    ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome, WalletWriteRepository,
};
use futures_util::FutureExt;
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

struct IsolatedDatabase {
    admin: PgPool,
    pool: PgPool,
    name: String,
}
impl IsolatedDatabase {
    async fn new() -> Self {
        let url = std::env::var("AETHER_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("AETHER_TEST_POSTGRES_URL"))
            .expect("requires disposable PostgreSQL with CREATEDB permission");
        let options = url.parse::<sqlx::postgres::PgConnectOptions>().unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await
            .unwrap();
        let name = format!("gateway_recharge_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(
                options
                    .database(&name)
                    .options([("statement_timeout", "15000")]),
            )
            .await
            .unwrap();
        Self { admin, pool, name }
    }
    async fn close(self) {
        tokio::time::timeout(Duration::from_secs(15), self.pool.close())
            .await
            .expect("pool cleanup timeout");
        tokio::time::timeout(
            Duration::from_secs(15),
            // This UUID-named database belongs exclusively to this fixture.
            // Server-side sessions can outlive client-side pool shutdown.
            sqlx::query(&format!("DROP DATABASE {} WITH (FORCE)", self.name)).execute(&self.admin),
        )
        .await
        .expect("database cleanup timeout")
        .unwrap();
        self.admin.close().await;
    }
}

struct AbortOnDrop(tokio::task::AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn financial_snapshot(pool: &PgPool) -> Value {
    let wallet: (i64, i64, i64) = sqlx::query_as("SELECT ROUND(balance*100000000)::bigint,ROUND(gift_balance*100000000)::bigint,ROUND(total_consumed*100000000)::bigint FROM wallets WHERE id='synthetic-wallet'")
        .fetch_one(pool).await.unwrap();
    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM request_fund_collection_receipts),(SELECT COUNT(*) FROM recharge_recovery_operations),(SELECT COUNT(*) FROM wallet_transactions),(SELECT COUNT(*) FROM payment_orders),(SELECT COUNT(*) FROM recharge_recovery_jobs)")
        .fetch_one(pool).await.unwrap();
    json!({"wallet":wallet,"counts":counts})
}

async fn notification_state(pool: &PgPool) -> (String, i32, bool) {
    sqlx::query_as(
        "SELECT state,attempts,delivered_at IS NOT NULL FROM recharge_recovery_notifications",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL with CREATEDB; authenticated loopback HTTP and SMTP"]
async fn live_gateway_recharge_callback_collects_once_exposes_history_and_acks_smtp_retry() {
    let f = IsolatedDatabase::new().await;
    let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(120), async {
        run_migrations(&f.pool).await.unwrap();
        sqlx::raw_sql("INSERT INTO users(id,username,email,email_verified,is_active,is_deleted,auth_source,created_at,updated_at) VALUES('synthetic-owner','synthetic-owner','owner@example.invalid',true,true,false,'local',now(),now()); INSERT INTO api_keys(id,user_id,key_hash,name) VALUES('synthetic-key','synthetic-owner','synthetic-key-hash','synthetic'); INSERT INTO wallets(id,user_id,balance,gift_balance,status,limit_mode,created_at,updated_at) VALUES('synthetic-wallet','synthetic-owner',0.10,0.03,'active','finite',now(),now()); INSERT INTO usage(id,request_id,user_id,api_key_id,provider_name,model,status,billing_status,billing_mode,actual_total_cost_usd,total_cost_usd,request_metadata) VALUES('synthetic-usage','synthetic-prior-debt','synthetic-owner','synthetic-key','synthetic-provider','synthetic-image','completed','insufficient_quota','legacy',0.20,0.20,'{\"settlement_snapshot\":{\"status\":\"complete\",\"synthetic\":true,\"price_per_request\":0.20}}'::jsonb)")
            .execute(&f.pool).await.unwrap();
        let (smtp_port, smtp_task) = smtp_server(2, true).await;
        let _smtp_abort = AbortOnDrop(smtp_task.abort_handle());
        let data = GatewayDataState::with_user_reader_for_tests(Arc::new(SqlxUserReadRepository::new(f.pool.clone())))
            .with_recharge_recovery_repository_for_tests(Arc::new(SqlxSettlementRepository::new(f.pool.clone())))
            .with_system_config_values_for_tests(vec![
                (IMPORTANT_NOTIFICATION_ENABLED_KEY.into(), json!(true)),
                (IMPORTANT_NOTIFICATION_ITEMS_KEY.into(), json!([{"key":USER_RECHARGE_RECOVERY_ITEM_KEY,"enabled":true,"user_email_enabled":true}])),
                ("smtp_host".into(), json!("127.0.0.1")), ("smtp_port".into(), json!(smtp_port)),
                ("smtp_use_tls".into(), json!(false)), ("smtp_use_ssl".into(), json!(false)),
                ("smtp_from_email".into(), json!("recovery@example.invalid")),
            ]);
        let user = data.find_user_auth_by_id("synthetic-owner").await.unwrap().unwrap();
        let now = chrono::Utc::now();
        let session = StoredUserSessionRecord::new(
            "synthetic-session".into(), user.id.clone(), "synthetic-device".into(), None,
            StoredUserSessionRecord::hash_refresh_token("synthetic-refresh-token"), None, None,
            Some(now), Some(now+chrono::Duration::hours(1)), None, None,
            Some("127.0.0.1".into()), Some("AetherTest/1.0".into()), Some(now), Some(now),
        ).unwrap();
        let token = create_local_auth_token(LocalAuthTokenType::Access, serde_json::Map::from_iter([
            ("user_id".into(), json!(user.id)), ("role".into(), json!(user.role)),
            ("created_at".into(), json!(user.created_at.unwrap().to_rfc3339())),
            ("session_id".into(), json!("synthetic-session")),
        ]), now+chrono::Duration::hours(1)).unwrap();
        let app = AppState::new().unwrap().with_data_state_for_tests(data).with_auth_sessions_for_tests(vec![session]);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/api/wallet/recharge-recoveries", listener.local_addr().unwrap());
        let router = crate::build_router_with_state(app.clone());
        let http_task = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        });
        let _http_abort = AbortOnDrop(http_task.abort_handle());
        let payment = uuid::Uuid::new_v4().to_string();
        let order_no = format!("synthetic-order-{payment}");
        sqlx::query("INSERT INTO payment_orders(id,order_no,wallet_id,user_id,amount_usd,pay_amount,pay_currency,payment_method,payment_provider,payment_channel,order_kind,status,created_at,expires_at) VALUES($1,$2,'synthetic-wallet','synthetic-owner',0.05,0.05,'USD','stripe','stripe','card','wallet_recharge','pending',now(),now()+interval '1 hour')")
            .bind(&payment).bind(&order_no).execute(&f.pool).await.unwrap();
        let callback = ProcessPaymentCallbackInput {
            payment_method:"stripe".into(), payment_provider:Some("stripe".into()), payment_channel:Some("card".into()),
            callback_key:format!("stripe:synthetic-{payment}"), order_no:Some(order_no), gateway_order_id:Some(format!("synthetic-{payment}")),
            amount_usd:0.05, pay_amount:Some(0.05), pay_currency:Some("USD".into()), exchange_rate:Some(1.0),
            payload_hash:"synthetic-recharge-e2e".into(), payload:json!({"status":"success","synthetic":true}), signature_valid:true,
        };
        let wallet = SqlxWalletRepository::new(f.pool.clone());
        let credited = wallet.process_payment_callback(callback.clone()).await.unwrap();
        assert!(matches!(credited, ProcessPaymentCallbackOutcome::Applied { duplicate:false, .. }));
        let enqueued: (String, i64) = sqlx::query_as("SELECT state,principal_cost_units FROM recharge_recovery_jobs")
            .fetch_one(&f.pool).await.unwrap();
        assert_eq!(enqueued, ("pending".into(), 5_000_000));
        let summary = app.data.process_recharge_recovery_batch(1).await.unwrap().remove(0);
        assert_eq!(summary.state, "waiting_next_recharge");
        assert_eq!(summary.collected_cost_units, 5_000_000);
        assert_eq!(summary.outstanding_cost_units, 15_000_000);
        assert_eq!(summary.available_recharge_cost_units, 10_000_000);
        let debit: (String, String, i64, i64, i64, i64) = sqlx::query_as("SELECT t.link_type,t.link_id,ROUND(t.amount*100000000)::bigint,ROUND(t.recharge_balance_before*100000000)::bigint,ROUND(t.recharge_balance_after*100000000)::bigint,r.collected_cost_units FROM recharge_recovery_operations o JOIN wallet_transactions t ON t.id=o.wallet_transaction_id JOIN request_fund_collection_receipts r ON r.id=o.receipt_id WHERE t.reason_code='historical_debt_recovery'")
            .fetch_one(&f.pool).await.unwrap();
        assert_eq!(debit, ("usage".into(),"synthetic-prior-debt".into(),-5_000_000,15_000_000,10_000_000,5_000_000));
        let financial = financial_snapshot(&f.pool).await;
        assert_eq!(financial, json!({"wallet":[10_000_000,3_000_000,5_000_000],"counts":[1,1,2,1,1]}));
        let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap();
        let response = client.get(&endpoint).bearer_auth(&token).header("x-client-device-id","synthetic-device").header("user-agent","AetherTest/1.0")
            .send().await.unwrap();
        assert_eq!(response.status(), http::StatusCode::OK);
        let history: Value = response.json().await.unwrap();
        assert_eq!(history["items"].as_array().unwrap().len(), 1);
        let item = &history["items"][0];
        assert_eq!(item["id"], summary.id);
        assert_eq!(item["payment_order_id"], payment);
        assert_eq!(item["collected_cost_units"], 5_000_000);
        assert_eq!(item["outstanding_cost_units"], 15_000_000);
        assert_eq!(item["available_recharge_cost_units"], 10_000_000);
        assert!(item.get("source_transaction_id").is_none());
        assert_eq!(notification_state(&f.pool).await, ("pending".into(),0,false));
        deliver_recharge_recovery_notifications(&app).await.unwrap();
        assert_eq!(notification_state(&f.pool).await, ("retry".into(),1,false));
        assert_eq!(financial_snapshot(&f.pool).await, financial);
        assert!(app.data.claim_recharge_recovery_notifications(1).await.unwrap().is_empty(), "persisted retry backoff must prevent immediate resend");
        sqlx::query("UPDATE recharge_recovery_notifications SET next_attempt_at=clock_timestamp() WHERE state='retry'")
            .execute(&f.pool).await.unwrap();
        deliver_recharge_recovery_notifications(&app).await.unwrap();
        assert_eq!(notification_state(&f.pool).await, ("delivered".into(),1,true));
        let messages = tokio::time::timeout(Duration::from_secs(5), smtp_task).await.unwrap().unwrap();
        assert_eq!(messages.len(), 2);
        for message in messages {
            assert!(message.contains("owner@example.invalid"));
            for body in decoded_mime_bodies(&message) {
                assert!(body.contains(&payment));
                for amount in ["0.05000000", "0.15000000", "0.10000000"] { assert!(body.contains(amount)); }
                assert!(!body.contains(&summary.source_transaction_id));
            }
        }
        // Replay ACK, worker tick, and the original payment callback after success.
        let (notification_id, lease): (String,i64) = sqlx::query_as("SELECT id,lease_token FROM recharge_recovery_notifications")
            .fetch_one(&f.pool).await.unwrap();
        assert!(!app.data.complete_recharge_recovery_notification(CompleteRechargeRecoveryNotificationInput {
            id:notification_id, lease_token:lease, outcome:RechargeRecoveryNotificationOutcome::Delivered, error_code:None,
        }).await.unwrap());
        deliver_recharge_recovery_notifications(&app).await.unwrap();
        assert!(app.data.process_recharge_recovery_batch(1).await.unwrap().is_empty());
        let replay = wallet.process_payment_callback(callback).await.unwrap();
        assert!(matches!(replay, ProcessPaymentCallbackOutcome::DuplicateProcessed {..} | ProcessPaymentCallbackOutcome::AlreadyCredited {..}));
        assert_eq!(financial_snapshot(&f.pool).await, financial);
        assert_eq!(notification_state(&f.pool).await, ("delivered".into(),1,true));
        http_task.abort();
        let _ = http_task.await;
        app.usage_runtime.shutdown(Duration::from_secs(5)).await.unwrap();
    })).catch_unwind().await;
    f.close().await;
    match result {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("bounded recharge end-to-end test timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
