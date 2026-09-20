use super::*;
use aether_data_contracts::repository::audit::{
    AdminAuditDeliveryFailureCode, AdminAuditDeliveryFailureOutcome, AuditLogWriteRepository,
};
use serde_json::{json, Value};

async fn session_snapshot(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar("SELECT to_jsonb(s) FROM user_sessions s ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

fn audit(
    id: &str,
    event_name: &str,
    action: &str,
    target_type: &str,
    target_id: &str,
) -> CreateAdminAuditLog {
    CreateAdminAuditLog {
        id: id.to_string(),
        event_type: "admin_mutation".to_string(),
        user_id: Some("audit-admin".to_string()),
        api_key_id: None,
        description: format!("admin action: {action}"),
        ip_address: Some("127.0.0.1".to_string()),
        user_agent: None,
        request_id: Some(id.to_string()),
        event_metadata: Some(json!({
            "schema_version": 1,
            "event_name": event_name,
            "status": "completed",
            "admin_role": "admin",
            "session_id": "admin-session",
            "route_family": "users_manage",
            "route_kind": action,
            "method": "DELETE",
            "path": "/api/admin/users/[user_id]/sessions",
            "action": action,
            "target_type": target_type,
            "target_id": target_id
        })),
        status_code: Some(200),
        error_message: None,
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn admin_session_revocation_is_atomic_idempotent_and_redelivers_audit_only() {
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("aether_admin_audit_"));
    sqlx::raw_sql(
        "INSERT INTO users(id,username,email_verified) VALUES('session-user','session-user',false);
         INSERT INTO user_sessions(id,user_id,client_device_id,refresh_token_hash,expires_at)
         VALUES
           ('session-one','session-user','device-one','redacted-hash-one',now()+interval '1 day'),
           ('session-two','session-user','device-two','redacted-hash-two',now()+interval '1 day');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let repository = SqlxUserReadRepository::new(pool.clone());
    let missing = audit(
        "018f1000-0000-7000-8000-000000000001",
        "admin_user_session_deleted",
        "delete_user_session",
        "user_session",
        "missing-session",
    );
    assert_eq!(
        repository
            .admin_revoke_user_session_with_audit(
                "session-user",
                "missing-session",
                chrono::Utc::now(),
                "admin_session_revoked",
                &missing,
            )
            .await
            .unwrap(),
        AdminUserSessionRevocationOutcome::NotFound
    );
    let queued_missing: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM admin_audit_delivery WHERE event_id=$1")
            .bind(&missing.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(queued_missing, 0);

    let first = audit(
        "018f1000-0000-7000-8000-000000000002",
        "admin_user_session_deleted",
        "delete_user_session",
        "user_session",
        "session-one",
    );
    assert_eq!(
        repository
            .admin_revoke_user_session_with_audit(
                "session-user",
                "session-one",
                chrono::Utc::now(),
                "admin_session_revoked",
                &first,
            )
            .await
            .unwrap(),
        AdminUserSessionRevocationOutcome::Revoked
    );
    let revoked_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT revoked_at FROM user_sessions WHERE id='session-one'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let repeated = audit(
        "018f1000-0000-7000-8000-000000000003",
        "admin_user_session_deleted",
        "delete_user_session",
        "user_session",
        "session-one",
    );
    assert_eq!(
        repository
            .admin_revoke_user_session_with_audit(
                "session-user",
                "session-one",
                chrono::Utc::now(),
                "admin_session_revoked",
                &repeated,
            )
            .await
            .unwrap(),
        AdminUserSessionRevocationOutcome::AlreadyRevoked
    );
    assert_eq!(
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT revoked_at FROM user_sessions WHERE id='session-one'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        revoked_at,
        "repeated administration must not rewrite revoked_at"
    );

    let all = audit(
        "018f1000-0000-7000-8000-000000000004",
        "admin_user_sessions_deleted",
        "delete_user_sessions",
        "user",
        "session-user",
    );
    assert_eq!(
        repository
            .admin_revoke_all_user_sessions_with_audit(
                "session-user",
                chrono::Utc::now(),
                "admin_revoke_all_sessions",
                &all,
            )
            .await
            .unwrap(),
        AdminUserSessionsRevocationOutcome::Revoked(1)
    );

    sqlx::query(
        "INSERT INTO user_sessions(id,user_id,client_device_id,refresh_token_hash,expires_at) \
         VALUES('session-rollback','session-user','device-three','redacted-hash-three',now()+interval '1 day')",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::raw_sql(
        "CREATE FUNCTION reject_session_audit_enqueue() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'private-session-audit-enqueue-error'; END $$;
         CREATE TRIGGER reject_session_audit_enqueue BEFORE INSERT ON admin_audit_delivery
         FOR EACH ROW EXECUTE FUNCTION reject_session_audit_enqueue();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let before_rejected_mutations = session_snapshot(&pool).await;
    let rejected = audit(
        "018f1000-0000-7000-8000-000000000005",
        "admin_user_session_deleted",
        "delete_user_session",
        "user_session",
        "session-rollback",
    );
    assert!(repository
        .admin_revoke_user_session_with_audit(
            "session-user",
            "session-rollback",
            chrono::Utc::now(),
            "admin_session_revoked",
            &rejected,
        )
        .await
        .is_err());
    assert_eq!(
        session_snapshot(&pool).await,
        before_rejected_mutations,
        "single-session enqueue failure must roll back every session field"
    );
    let rejected_all = audit(
        "018f1000-0000-7000-8000-000000000006",
        "admin_user_sessions_deleted",
        "delete_user_sessions",
        "user",
        "session-user",
    );
    assert!(repository
        .admin_revoke_all_user_sessions_with_audit(
            "session-user",
            chrono::Utc::now(),
            "admin_revoke_all_sessions",
            &rejected_all,
        )
        .await
        .is_err());
    assert_eq!(
        session_snapshot(&pool).await,
        before_rejected_mutations,
        "all-session enqueue failure must roll back every session field"
    );
    sqlx::raw_sql(
        "DROP TRIGGER reject_session_audit_enqueue ON admin_audit_delivery;
         DROP FUNCTION reject_session_audit_enqueue();",
    )
    .execute(&pool)
    .await
    .unwrap();

    let audit_repository = crate::PostgresAuditLogReadRepository::new(pool.clone());
    let delivery_snapshot = session_snapshot(&pool).await;
    let claim = audit_repository
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION reject_session_audit_delivery() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'private-session-audit-delivery-error'; END $$;
         CREATE TRIGGER reject_session_audit_delivery BEFORE INSERT ON audit_logs
         FOR EACH ROW EXECUTE FUNCTION reject_session_audit_delivery();",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(audit_repository
        .deliver_admin_audit(&claim.event_id, claim.lease_token)
        .await
        .is_err());
    assert_eq!(
        audit_repository
            .fail_admin_audit_delivery(
                &claim.event_id,
                claim.lease_token,
                AdminAuditDeliveryFailureCode::AuditInsertFailed,
            )
            .await
            .unwrap(),
        AdminAuditDeliveryFailureOutcome::RetryScheduled
    );
    assert_eq!(session_snapshot(&pool).await, delivery_snapshot);
    sqlx::raw_sql(
        "DROP TRIGGER reject_session_audit_delivery ON audit_logs;
         DROP FUNCTION reject_session_audit_delivery();",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE admin_audit_delivery SET next_attempt_at=clock_timestamp() WHERE event_id=$1",
    )
    .bind(&claim.event_id)
    .execute(&pool)
    .await
    .unwrap();
    drop(audit_repository);
    let restarted = crate::PostgresAuditLogReadRepository::new(pool.clone());
    // Older untouched events remain due before this newly scheduled retry.
    // Claim all three and identify the failed event without assuming queue order.
    let mut pending_claims = restarted.claim_admin_audit_deliveries(3, 30).await.unwrap();
    assert_eq!(pending_claims.len(), 3);
    let retry_index = pending_claims
        .iter()
        .position(|pending| pending.event_id == claim.event_id)
        .expect("the failed event must be claimable after retry scheduling");
    let retry = pending_claims.remove(retry_index);
    assert_eq!(retry.event_id, claim.event_id);
    assert_ne!(retry.lease_token, claim.lease_token);
    assert!(!restarted
        .deliver_admin_audit(&claim.event_id, claim.lease_token)
        .await
        .unwrap());
    assert!(restarted
        .deliver_admin_audit(&retry.event_id, retry.lease_token)
        .await
        .unwrap());
    assert!(!restarted
        .deliver_admin_audit(&retry.event_id, retry.lease_token)
        .await
        .unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(&retry.event_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM admin_audit_delivery WHERE event_id=$1",
        )
        .bind(&retry.event_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "delivered"
    );
    assert_eq!(session_snapshot(&pool).await, delivery_snapshot);
    assert_eq!(pending_claims.len(), 2);
    for claim in pending_claims {
        assert!(restarted
            .deliver_admin_audit(&claim.event_id, claim.lease_token)
            .await
            .unwrap());
    }
    assert_eq!(session_snapshot(&pool).await, delivery_snapshot);
    assert_eq!(
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT revoked_at FROM user_sessions WHERE id='session-one'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        revoked_at,
        "audit redelivery must not repeat the session mutation"
    );
    let payloads: Vec<String> =
        sqlx::query_scalar("SELECT payload::text FROM admin_audit_delivery ORDER BY event_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let payloads = payloads.join("\n");
    assert!(payloads.contains("audit-admin"));
    for secret in [
        "redacted-hash-one",
        "redacted-hash-two",
        "redacted-hash-three",
    ] {
        assert!(!payloads.contains(secret));
    }
    pool.close().await;
}
