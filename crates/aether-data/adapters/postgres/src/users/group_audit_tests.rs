//! Run only against a fresh, migrated, task-owned audit database.
use super::*;
use crate::PostgresAuditLogReadRepository;
use aether_data_contracts::repository::audit::{
    AdminAuditDeliveryFailureCode, AdminAuditDeliveryFailureOutcome, AuditLogWriteRepository,
    CreateAdminAuditLog,
};
use serde_json::{json, Value};

fn audit_record() -> CreateAdminAuditLog {
    let id = uuid::Uuid::now_v7().to_string();
    CreateAdminAuditLog {
        id: id.clone(),
        event_type: "admin_mutation".to_string(),
        user_id: None,
        api_key_id: None,
        description: "admin action: update_user_group_members".to_string(),
        ip_address: None,
        user_agent: None,
        request_id: Some(id),
        event_metadata: Some(json!({
            "schema_version": 1,
            "event_name": "admin_user_group_members_updated",
            "status": "completed",
            "method": "PUT",
            "path": "/api/admin/user-groups/[group_id]/members",
            "action": "update_user_group_members",
            "target_type": "user_group",
            "target_id": "group-audit-target"
        })),
        status_code: Some(200),
        error_message: None,
        created_at: chrono::Utc::now(),
    }
}

async fn business_snapshot(pool: &PgPool) -> (Vec<Value>, Vec<Value>) {
    let members = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM user_group_members m ORDER BY group_id,user_id",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let writes = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM group_audit_write_probe p ORDER BY operation,user_id,created_at",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    (members, writes)
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn group_members_audit_enqueue_rolls_back_and_delivery_retry_never_rewrites_members() {
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("aether_admin_audit_"));
    let intents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_audit_delivery")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(intents, 0, "use a separate fresh database per ignored test");
    sqlx::raw_sql(
        "INSERT INTO users (id,username,email_verified,is_active,is_deleted,role)
         VALUES ('group-old-user','group_old',true,true,false,'user'),
                ('group-new-user','group_new',true,true,false,'user');
         INSERT INTO user_groups (id,name,normalized_name)
         VALUES ('group-audit-target','Group audit','group audit');
         INSERT INTO user_group_members (group_id,user_id,created_at)
         VALUES ('group-audit-target','group-old-user','2020-01-01T00:00:00Z');
         CREATE TABLE group_audit_write_probe (
             operation text NOT NULL, user_id text NOT NULL, created_at timestamptz NOT NULL);
         CREATE FUNCTION group_audit_probe() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           IF TG_OP='DELETE' THEN
             INSERT INTO group_audit_write_probe VALUES (TG_OP,OLD.user_id,OLD.created_at);
             RETURN OLD;
           END IF;
           INSERT INTO group_audit_write_probe VALUES (TG_OP,NEW.user_id,NEW.created_at);
           RETURN NEW;
         END $$;
         CREATE TRIGGER group_audit_probe AFTER INSERT OR DELETE ON user_group_members
         FOR EACH ROW EXECUTE FUNCTION group_audit_probe();
         CREATE FUNCTION group_audit_reject_intent() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'group-intent-test-rejected'; END $$;
         CREATE TRIGGER group_audit_reject_intent BEFORE INSERT ON admin_audit_delivery
         FOR EACH ROW EXECUTE FUNCTION group_audit_reject_intent();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let repository = SqlxUserReadRepository::new(pool.clone());
    let record = audit_record();
    let before = business_snapshot(&pool).await;
    assert!(repository
        .replace_user_group_members_with_audit(
            "group-audit-target",
            &["group-new-user".to_string()],
            &record,
        )
        .await
        .is_err());
    assert_eq!(
        business_snapshot(&pool).await,
        before,
        "including timestamps and trigger writes"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM admin_audit_delivery")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::raw_sql(
        "DROP TRIGGER group_audit_reject_intent ON admin_audit_delivery;
         DROP FUNCTION group_audit_reject_intent();",
    )
    .execute(&pool)
    .await
    .unwrap();

    let returned = repository
        .replace_user_group_members_with_audit(
            "group-audit-target",
            &["group-new-user".to_string(), "group-new-user".to_string()],
            &record,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(returned.len(), 1);
    assert_eq!(returned[0].user_id, "group-new-user");
    let persisted = repository
        .list_user_group_members("group-audit-target")
        .await
        .unwrap();
    assert_eq!(returned[0].created_at, persisted[0].created_at);
    let committed = business_snapshot(&pool).await;
    assert_eq!(committed.1.len(), 2, "one delete and one normalized insert");
    let payload: Value =
        sqlx::query_scalar("SELECT payload FROM admin_audit_delivery WHERE event_id=$1")
            .bind(&record.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(payload, serde_json::to_value(&record).unwrap());

    // Replaying the same intent must not silently commit another delete/insert.
    assert!(repository
        .replace_user_group_members_with_audit(
            "group-audit-target",
            &["group-old-user".to_string()],
            &record,
        )
        .await
        .is_err());
    assert_eq!(business_snapshot(&pool).await, committed);

    let delivery = PostgresAuditLogReadRepository::new(pool.clone());
    let claim = delivery
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(claim.event_id, record.id);
    sqlx::raw_sql(
        "CREATE FUNCTION group_audit_reject_delivery() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'group-delivery-test-rejected'; END $$;
         CREATE TRIGGER group_audit_reject_delivery BEFORE INSERT ON audit_logs
         FOR EACH ROW EXECUTE FUNCTION group_audit_reject_delivery();",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(delivery
        .deliver_admin_audit(&record.id, claim.lease_token)
        .await
        .is_err());
    assert_eq!(
        delivery
            .fail_admin_audit_delivery(
                &record.id,
                claim.lease_token,
                AdminAuditDeliveryFailureCode::AuditInsertFailed,
            )
            .await
            .unwrap(),
        AdminAuditDeliveryFailureOutcome::RetryScheduled
    );
    assert_eq!(business_snapshot(&pool).await, committed);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(&record.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::raw_sql(
        "DROP TRIGGER group_audit_reject_delivery ON audit_logs;
         DROP FUNCTION group_audit_reject_delivery();",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Exercise retry scheduling, not the separate real lease-expiry/crash contract.
    sqlx::query(
        "UPDATE admin_audit_delivery SET next_attempt_at=clock_timestamp() WHERE event_id=$1",
    )
    .bind(&record.id)
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
        .deliver_admin_audit(&record.id, retry.lease_token)
        .await
        .unwrap());
    assert!(!restarted
        .deliver_admin_audit(&record.id, retry.lease_token)
        .await
        .unwrap());
    assert_eq!(business_snapshot(&pool).await, committed);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(&record.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM admin_audit_delivery WHERE event_id=$1 AND state='delivered'",
        )
        .bind(&record.id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    pool.close().await;
}
