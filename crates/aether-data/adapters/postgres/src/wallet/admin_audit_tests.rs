use super::*;
use aether_data_contracts::repository::audit::{
    AdminAuditDeliveryFailureCode, AdminAuditDeliveryFailureOutcome, AuditLogWriteRepository,
    CreateAdminAuditLog,
};
use serde_json::{json, Value};

fn record(recharge: bool) -> CreateAdminAuditLog {
    let id = Uuid::now_v7().to_string();
    CreateAdminAuditLog {
        id: id.clone(),
        event_type: "admin_mutation".to_string(),
        user_id: None,
        api_key_id: None,
        description: "admin wallet mutation".to_string(),
        ip_address: None,
        user_agent: None,
        request_id: Some(id),
        event_metadata: Some(json!({
            "schema_version": 1, "status": "completed",
            "event_name": if recharge { "admin_wallet_manual_recharge_created" } else { "admin_wallet_balance_adjusted" },
            "target_type": "wallet", "target_id": "audit-wallet"
        })),
        status_code: Some(200),
        error_message: None,
        created_at: Utc::now(),
    }
}

async fn snapshot(pool: &PgPool) -> [Vec<Value>; 14] {
    let mut result: [Vec<Value>; 14] = Default::default();
    for (slot, query) in result.iter_mut().zip([
        "SELECT to_jsonb(w) FROM wallets w ORDER BY id",
        "SELECT to_jsonb(t) FROM wallet_transactions t ORDER BY id",
        "SELECT to_jsonb(o) FROM payment_orders o ORDER BY id",
        "SELECT to_jsonb(p) FROM wallet_audit_write_probe p ORDER BY changed_at,balance",
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
    result
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
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn wallet_adjust_and_manual_recharge_audit_are_atomic_and_preserve_replay_contracts() {
    let pool = PgPool::connect(&std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("aether_admin_audit_"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM admin_audit_delivery")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::raw_sql(
        "INSERT INTO users(id,username,email_verified) VALUES('audit-wallet-owner','audit-wallet-owner',true);
         INSERT INTO wallets(id,user_id,balance,gift_balance,total_recharged,created_at,updated_at)
           VALUES('audit-wallet','audit-wallet-owner',10,3,20,now(),now());
         CREATE TABLE wallet_audit_write_probe(balance numeric, changed_at timestamptz);
         CREATE FUNCTION wallet_audit_probe() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN INSERT INTO wallet_audit_write_probe VALUES(NEW.balance,NEW.updated_at); RETURN NEW; END $$;
         CREATE TRIGGER wallet_audit_probe AFTER UPDATE ON wallets
           FOR EACH ROW EXECUTE FUNCTION wallet_audit_probe();
         CREATE FUNCTION wallet_audit_reject_intent() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'private-wallet-enqueue-error'; END $$;
         CREATE TRIGGER wallet_audit_reject_intent BEFORE INSERT ON admin_audit_delivery
           FOR EACH ROW EXECUTE FUNCTION wallet_audit_reject_intent();",
    ).execute(&pool).await.unwrap();
    let repository = SqlxWalletRepository::new(pool.clone());
    let adjust = AdjustWalletBalanceInput {
        wallet_id: "audit-wallet".to_string(),
        amount_usd: 4.0,
        balance_type: "recharge".to_string(),
        operator_id: None,
        description: Some("private-wallet-description".to_string()),
    };
    let recharge = CreateManualWalletRechargeInput {
        wallet_id: "audit-wallet".to_string(),
        amount_usd: 5.0,
        payment_method: "admin_manual".to_string(),
        operator_id: None,
        description: Some("private-recharge-description".to_string()),
        order_no: "wallet-audit-order-1".to_string(),
    };
    let adjust_audit = record(false);
    let recharge_audit = record(true);
    enable_recovery_with_legacy_debt(&pool, "audit-wallet-owner").await;
    let initial_deliveries = delivery_snapshot(&pool).await;
    let initial = snapshot(&pool).await;
    assert!(repository
        .adjust_wallet_balance_with_audit(adjust.clone(), &adjust_audit)
        .await
        .is_err());
    assert_eq!(snapshot(&pool).await, initial);
    assert!(repository
        .create_manual_wallet_recharge_with_audit(recharge.clone(), &recharge_audit)
        .await
        .is_err());
    assert_eq!(
        snapshot(&pool).await,
        initial,
        "wallet, ledger, order and probe all roll back"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM admin_audit_delivery")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::raw_sql(
        "DROP TRIGGER wallet_audit_reject_intent ON admin_audit_delivery;
         DROP FUNCTION wallet_audit_reject_intent();",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(delivery_snapshot(&pool).await, initial_deliveries);
    reject_deferred_recovery_commit(&pool).await;
    let commit_error = repository
        .create_manual_wallet_recharge_with_audit(
            CreateManualWalletRechargeInput {
                order_no: "wallet-audit-commit-fail".to_string(),
                ..recharge.clone()
            },
            &record(true),
        )
        .await
        .unwrap_err();
    assert!(commit_error
        .to_string()
        .contains("wallet-audit-deferred-commit-rejected"));
    remove_deferred_recovery_rejection(&pool).await;
    assert_eq!(
        snapshot(&pool).await,
        initial,
        "COMMIT failure rolls back money/order/intent and actual deferred recovery rows"
    );
    assert_eq!(delivery_snapshot(&pool).await, initial_deliveries);

    let Some(WalletMutationOutcome::Applied((wallet, transaction))) = repository
        .adjust_wallet_balance_with_audit(adjust.clone(), &adjust_audit)
        .await
        .unwrap()
    else {
        panic!("PG capability must apply the existing wallet mutation")
    };
    assert_eq!(wallet.balance, 14.0);
    assert_eq!(wallet.gift_balance, 3.0);
    assert_eq!(wallet.total_adjusted, 4.0);
    assert_eq!(transaction.amount, 4.0);
    let after_adjust = snapshot(&pool).await;
    assert_eq!(after_adjust[1].len(), 1);
    assert_recovery_jobs(&pool, 0, "audit-wallet-owner").await;
    assert!(repository
        .adjust_wallet_balance_with_audit(adjust, &adjust_audit)
        .await
        .is_err());
    assert_eq!(
        snapshot(&pool).await,
        after_adjust,
        "duplicate intent ID cannot repeat adjustment"
    );
    let Some(WalletMutationOutcome::Applied((wallet, order))) = repository
        .create_manual_wallet_recharge_with_audit(recharge.clone(), &recharge_audit)
        .await
        .unwrap()
    else {
        panic!("PG capability must apply the existing recharge mutation")
    };
    assert_eq!(wallet.balance, 19.0);
    assert_eq!(wallet.gift_balance, 3.0);
    assert_eq!(wallet.total_recharged, 25.0);
    assert_eq!(order.order_no, recharge.order_no);
    assert_eq!(order.status, "credited");
    let committed = snapshot(&pool).await;
    assert_eq!(committed[1].len(), 2);
    assert_eq!(committed[2].len(), 1);
    assert_eq!(committed[3].len(), 2);
    assert_recovery_jobs(&pool, 1, "audit-wallet-owner").await;
    let committed_deliveries = delivery_snapshot(&pool).await;
    // Fresh order_no gets past the order uniqueness gate; the committed event
    // ID must instead reject at the outbox and roll back all financial work.
    let repeated_intent = repository
        .create_manual_wallet_recharge_with_audit(
            CreateManualWalletRechargeInput {
                order_no: "wallet-audit-fresh-order-same-intent".to_string(),
                ..recharge.clone()
            },
            &recharge_audit,
        )
        .await
        .unwrap_err();
    assert!(repeated_intent.to_string().contains("23505"));
    assert!(repeated_intent.to_string().contains("admin_audit_delivery"));
    assert_eq!(snapshot(&pool).await, committed);
    assert_eq!(delivery_snapshot(&pool).await, committed_deliveries);

    // Same order_no continues to fail, even with a fresh audit ID. The audit
    // capability must not turn this legacy error into a second successful credit.
    assert!(repository
        .create_manual_wallet_recharge_with_audit(recharge.clone(), &record(true))
        .await
        .is_err());
    assert_eq!(snapshot(&pool).await, committed);
    assert_eq!(delivery_snapshot(&pool).await, committed_deliveries);
    assert!(matches!(
        repository
            .create_manual_wallet_recharge_with_audit(
                CreateManualWalletRechargeInput {
                    wallet_id: "missing-wallet".to_string(),
                    ..recharge.clone()
                },
                &record(true),
            )
            .await
            .unwrap(),
        Some(WalletMutationOutcome::NotFound)
    ));
    assert!(repository
        .create_manual_wallet_recharge_with_audit(
            CreateManualWalletRechargeInput {
                amount_usd: f64::NAN,
                ..recharge
            },
            &record(true),
        )
        .await
        .is_err());
    assert_eq!(snapshot(&pool).await, committed);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM admin_audit_delivery")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );

    sqlx::raw_sql(
        "CREATE FUNCTION wallet_audit_reject_delivery() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'private-wallet-delivery-error'; END $$;
         CREATE TRIGGER wallet_audit_reject_delivery BEFORE INSERT ON audit_logs
           FOR EACH ROW EXECUTE FUNCTION wallet_audit_reject_delivery();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let delivery = crate::PostgresAuditLogReadRepository::new(pool.clone());
    let claims = delivery.claim_admin_audit_deliveries(2, 30).await.unwrap();
    assert_eq!(claims.len(), 2);
    for claim in claims {
        assert!(delivery
            .deliver_admin_audit(&claim.event_id, claim.lease_token)
            .await
            .is_err());
        assert_eq!(
            delivery
                .fail_admin_audit_delivery(
                    &claim.event_id,
                    claim.lease_token,
                    AdminAuditDeliveryFailureCode::AuditInsertFailed
                )
                .await
                .unwrap(),
            AdminAuditDeliveryFailureOutcome::RetryScheduled
        );
    }
    assert_eq!(snapshot(&pool).await, committed);
    sqlx::raw_sql(
        "DROP TRIGGER wallet_audit_reject_delivery ON audit_logs;
         DROP FUNCTION wallet_audit_reject_delivery();
         UPDATE admin_audit_delivery SET next_attempt_at=clock_timestamp();",
    )
    .execute(&pool)
    .await
    .unwrap();
    drop(delivery);
    let restarted = crate::PostgresAuditLogReadRepository::new(pool.clone());
    let retries = restarted.claim_admin_audit_deliveries(2, 30).await.unwrap();
    assert_eq!(retries.len(), 2);
    for claim in retries {
        assert!(restarted
            .deliver_admin_audit(&claim.event_id, claim.lease_token)
            .await
            .unwrap());
        assert!(!restarted
            .deliver_admin_audit(&claim.event_id, claim.lease_token)
            .await
            .unwrap());
    }
    assert_eq!(
        snapshot(&pool).await,
        committed,
        "audit delivery cannot re-run either monetary mutation"
    );
    for audit in [&adjust_audit, &recharge_audit] {
        let payload: Value = sqlx::query_scalar(
            "SELECT payload FROM admin_audit_delivery WHERE event_id=$1 AND state='delivered'",
        )
        .bind(&audit.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(payload, serde_json::to_value(audit).unwrap());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
                .bind(&audit.id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }
    assert_recovery_jobs(&pool, 1, "audit-wallet-owner").await;
    pool.close().await;
}
