//! Restore uses the real importer and payment callback in a disposable database.
use super::*;
use aether_data_contracts::repository::wallet::{
    ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome, WalletWriteRepository,
};
use aether_data_postgres::SqlxWalletRepository;
use futures_util::FutureExt;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::panic::AssertUnwindSafe;
use std::time::Duration;

struct IsolatedDatabase {
    admin: PgPool,
    pool: PgPool,
    name: String,
}

impl IsolatedDatabase {
    async fn new() -> Self {
        let url = std::env::var("AETHER_TEST_POSTGRES_URL")
            .or_else(|_| std::env::var("AETHER_TEST_DATABASE_URL"))
            .expect("a disposable PostgreSQL URL with CREATEDB permission is required");
        let options = url.parse::<sqlx::postgres::PgConnectOptions>().unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await
            .unwrap();
        let name = format!("recharge_restore_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&admin)
            .await
            .unwrap();
        // One backend makes transaction-local setting leakage observable on reuse.
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.database(&name))
            .await
            .unwrap();
        Self { admin, pool, name }
    }

    async fn close(self) {
        self.pool.close().await;
        tokio::time::timeout(
            Duration::from_secs(15),
            sqlx::query(&format!("DROP DATABASE {}", self.name)).execute(&self.admin),
        )
        .await
        .expect("disposable restore database cleanup timed out")
        .unwrap();
        self.admin.close().await;
    }
}

fn historical_records(order: &str, receipt: &str) -> Vec<DataExportRecord> {
    vec![
        DataExportRecord::manifest(DataExportManifest::new(
            1_700_000_000,
            Some(DatabaseDriver::Postgres),
            vec![ExportDomain::Wallets],
        )),
        DataExportRecord::row(
            ExportDomain::Wallets,
            format!("payment_orders:{order}"),
            json!({
                "__table":"payment_orders", "id":order, "order_no":format!("order-{order}"),
                "wallet_id":"restore-wallet", "user_id":"restore-owner", "amount_usd":0.05,
                "pay_amount":0.05, "pay_currency":"USD", "payment_method":"stripe",
                "payment_provider":"stripe", "payment_channel":"card", "order_kind":"wallet_recharge",
                "status":"credited", "created_at":"2020-01-01T00:00:00Z",
                "paid_at":"2020-01-01T00:00:01Z", "credited_at":"2020-01-01T00:00:02Z",
                "refunded_amount_usd":0, "refundable_amount_usd":0.05
            }),
        ),
        DataExportRecord::row(
            ExportDomain::Wallets,
            format!("wallet_transactions:{receipt}"),
            json!({
                "__table":"wallet_transactions", "id":receipt, "wallet_id":"restore-wallet",
                "category":"recharge", "reason_code":"topup_gateway", "amount":0.05,
                "balance_before":0.10, "balance_after":0.15,
                "recharge_balance_before":0.10, "recharge_balance_after":0.15,
                "gift_balance_before":0, "gift_balance_after":0,
                "link_type":"payment_order", "link_id":order, "created_at":"2020-01-01T00:00:02Z"
            }),
        ),
    ]
}

async fn live_credit(pool: &PgPool, usd: f64) {
    let id = uuid::Uuid::new_v4().to_string();
    let order_no = format!("order-{id}");
    sqlx::query("INSERT INTO payment_orders(id,order_no,wallet_id,user_id,amount_usd,pay_amount,pay_currency,payment_method,payment_provider,payment_channel,order_kind,status,created_at,expires_at) VALUES($1,$2,'restore-wallet','restore-owner',$3,$3,'USD','stripe','stripe','card','wallet_recharge','pending',clock_timestamp(),clock_timestamp()+interval '1 hour')")
        .bind(&id).bind(&order_no).bind(usd).execute(pool).await.unwrap();
    let outcome = SqlxWalletRepository::new(pool.clone())
        .process_payment_callback(ProcessPaymentCallbackInput {
            payment_method: "stripe".into(),
            payment_provider: Some("stripe".into()),
            payment_channel: Some("card".into()),
            callback_key: format!("stripe:event-{id}"),
            order_no: Some(order_no),
            gateway_order_id: Some(format!("gateway-{id}")),
            amount_usd: usd,
            pay_amount: Some(usd),
            pay_currency: Some("USD".into()),
            exchange_rate: Some(1.0),
            payload_hash: "restore-regression-credit".into(),
            payload: json!({"status":"success"}),
            signature_valid: true,
        })
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        ProcessPaymentCallbackOutcome::Applied {
            duplicate: false,
            ..
        }
    ));
}

async fn assert_restore_flag_cleared(pool: &PgPool, expected_pid: i32) {
    let (pid, active): (i32, bool) = sqlx::query_as("SELECT pg_backend_pid(), COALESCE(current_setting('aether.recharge_recovery_restore',true),'')='on'")
        .fetch_one(pool).await.unwrap();
    assert_eq!(
        pid, expected_pid,
        "must test reuse of the same database session"
    );
    assert!(
        !active,
        "restore suppression must never leak beyond its transaction"
    );
}

async fn recovery_totals(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT COUNT(*) FROM recharge_recovery_jobs),(SELECT COALESCE(SUM(principal_cost_units),0)::bigint FROM recharge_recovery_jobs),(SELECT COUNT(*) FROM recharge_recovery_candidates)")
        .fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL with CREATEDB; real importer and wallet callbacks"]
async fn live_recharge_restore_never_reauthorizes_history_and_does_not_suppress_later_callbacks() {
    let f = IsolatedDatabase::new().await;
    let result = AssertUnwindSafe(async {
        crate::lifecycle::migrate::run_migrations(&f.pool).await.unwrap();
        sqlx::raw_sql("INSERT INTO users(id,username,email_verified) VALUES('restore-owner','restore-owner',false); INSERT INTO api_keys(id,user_id,key_hash,name) VALUES('restore-key','restore-owner','restore-key-hash','restore'); INSERT INTO wallets(id,user_id,balance,gift_balance,status,limit_mode,created_at,updated_at) VALUES('restore-wallet','restore-owner',0.15,0,'active','finite',now(),now()); INSERT INTO usage(id,request_id,user_id,api_key_id,provider_name,model,status,billing_status,actual_total_cost_usd,total_cost_usd,request_metadata) VALUES('restore-debt','restore-debt','restore-owner','restore-key','test-provider','image','completed','insufficient_quota',0.20,0.20,'{\"settlement_snapshot\":{\"status\":\"complete\"}}'::jsonb)")
            .execute(&f.pool).await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&f.pool).await.unwrap();
        let history = encode_jsonl(&historical_records("historical-order", "historical-receipt")).unwrap();
        assert_eq!(import_postgres_jsonl(&f.pool, &history).await.unwrap(), 2);
        assert_eq!(recovery_totals(&f.pool).await, (0, 0, 0));
        assert_restore_flag_cleared(&f.pool, pid).await;
        let plan = build_import_plan(&history).unwrap();
        assert_eq!(import_postgres_plan(&f.pool, &plan).await.unwrap(), 2);
        assert_eq!(recovery_totals(&f.pool).await, (0, 0, 0), "repeated restore cannot manufacture a budget");
        let preserved: (i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM payment_orders WHERE id='historical-order'),(SELECT COUNT(*) FROM wallet_transactions WHERE id='historical-receipt')")
            .fetch_one(&f.pool).await.unwrap();
        assert_eq!(preserved, (1, 1));
        live_credit(&f.pool, 0.01).await;
        assert_eq!(recovery_totals(&f.pool).await, (1, 1_000_000, 1));

        let mut failed_records = historical_records("rolled-back-order", "rolled-back-receipt");
        let mut invalid = historical_records("rolled-back-order", "invalid-receipt").pop().unwrap();
        if let DataExportRecord::Row { payload, .. } = &mut invalid {
            // Database balance constraint fails after earlier rows were inserted.
            payload["balance_after"] = json!(99);
        }
        failed_records.push(invalid);
        let failed_plan = build_import_plan(&encode_jsonl(&failed_records).unwrap()).unwrap();
        assert!(import_postgres_plan(&f.pool, &failed_plan).await.is_err());
        assert_restore_flag_cleared(&f.pool, pid).await;
        let rolled_back: (i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM payment_orders WHERE id='rolled-back-order'),(SELECT COUNT(*) FROM wallet_transactions WHERE id IN ('rolled-back-receipt','invalid-receipt'))")
            .fetch_one(&f.pool).await.unwrap();
        assert_eq!(rolled_back, (0, 0));
        assert_eq!(recovery_totals(&f.pool).await, (1, 1_000_000, 1));
        live_credit(&f.pool, 0.02).await;
        assert_restore_flag_cleared(&f.pool, pid).await;
        assert_eq!(recovery_totals(&f.pool).await, (2, 3_000_000, 2));
        assert_eq!(import_postgres_jsonl(&f.pool, &history).await.unwrap(), 2);
        assert_eq!(recovery_totals(&f.pool).await, (2, 3_000_000, 2));
        let balance: i64 = sqlx::query_scalar("SELECT ROUND(balance*100000000)::bigint FROM wallets WHERE id='restore-wallet'")
            .fetch_one(&f.pool).await.unwrap();
        assert_eq!(balance, 18_000_000, "restore replay cannot credit or collect money");
    }).catch_unwind().await;
    f.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}
