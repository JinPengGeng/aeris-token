use super::*;
use serde_json::json;
use std::time::Duration;

fn audit(id: &str, target_id: &str) -> CreateAdminAuditLog {
    CreateAdminAuditLog {
        id: id.to_string(),
        event_type: "admin_mutation".to_string(),
        user_id: Some("audit-admin".to_string()),
        api_key_id: None,
        description: "admin action: delete_user_sessions".to_string(),
        ip_address: None,
        user_agent: None,
        request_id: Some(id.to_string()),
        event_metadata: Some(json!({
            "schema_version": 1,
            "event_name": "admin_user_sessions_deleted",
            "status": "completed",
            "method": "DELETE",
            "path": "/api/admin/users/[user_id]/sessions",
            "action": "delete_user_sessions",
            "target_type": "user",
            "target_id": target_id
        })),
        status_code: Some(200),
        error_message: None,
        created_at: chrono::Utc::now(),
    }
}

async fn wait_for_lock_wait(pool: &PgPool, query_fragment: &str) {
    for _ in 0..200 {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM pg_stat_activity
               WHERE datname=current_database()
                 AND wait_event_type='Lock'
                 AND query LIKE '%' || $1 || '%'
             )",
        )
        .bind(query_fragment)
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("query did not reach the expected PostgreSQL lock wait: {query_fragment}");
}

fn session(user_id: &str) -> StoredUserSessionRecord {
    let now = chrono::Utc::now();
    StoredUserSessionRecord {
        id: "existence-login-session".to_string(),
        user_id: user_id.to_string(),
        client_device_id: "existence-device".to_string(),
        device_label: None,
        refresh_token_hash: "existence-refresh-hash".to_string(),
        prev_refresh_token_hash: None,
        rotated_at: None,
        last_seen_at: Some(now),
        expires_at: Some(now + chrono::Duration::days(1)),
        revoked_at: None,
        revoke_reason: None,
        ip_address: None,
        user_agent: None,
        created_at: Some(now),
        updated_at: Some(now),
        security_version: 0,
    }
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn revoke_all_sessions_locks_user_and_rejects_a_concurrently_deleted_target() {
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("aether_admin_audit_"));
    sqlx::query(
        "INSERT INTO users(id,username,email_verified) VALUES('existence-delete-user','existence_delete',false)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut delete_tx = pool.begin().await.unwrap();
    sqlx::query("DELETE FROM users WHERE id='existence-delete-user'")
        .execute(&mut *delete_tx)
        .await
        .unwrap();
    let repository = SqlxUserReadRepository::new(pool.clone());
    let record = audit(
        "018f2000-0000-7000-8000-000000000001",
        "existence-delete-user",
    );
    let task = tokio::spawn(async move {
        repository
            .admin_revoke_all_user_sessions_with_audit(
                "existence-delete-user",
                chrono::Utc::now(),
                "admin_revoke_all_sessions",
                &record,
            )
            .await
    });
    wait_for_lock_wait(&pool, "SELECT id FROM users WHERE id=$1 FOR UPDATE").await;
    delete_tx.commit().await.unwrap();
    assert_eq!(
        task.await.unwrap().unwrap(),
        AdminUserSessionsRevocationOutcome::NotFound
    );
    let intents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_audit_delivery")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(intents, 0);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn revoke_all_sessions_orders_after_an_inflight_password_login() {
    const ADVISORY_KEY: i64 = 8_411_551;
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("aether_admin_audit_"));
    sqlx::query(
        "INSERT INTO users(id,username,password_hash,email_verified)
         VALUES('existence-login-user','existence_login','password-hash',false)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION pause_existence_login() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock({ADVISORY_KEY}); RETURN NEW; END $$;
         CREATE TRIGGER pause_existence_login BEFORE INSERT ON user_sessions
         FOR EACH ROW EXECUTE FUNCTION pause_existence_login();"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let mut gate = pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(ADVISORY_KEY)
        .execute(&mut *gate)
        .await
        .unwrap();

    let create_repository = SqlxUserReadRepository::new(pool.clone());
    let pending_session = session("existence-login-user");
    let create_task = tokio::spawn(async move {
        create_repository
            .create_user_session_if_password_matches(&pending_session, "password-hash")
            .await
    });
    wait_for_lock_wait(&pool, "INSERT INTO user_sessions").await;

    let revoke_repository = SqlxUserReadRepository::new(pool.clone());
    let record = audit(
        "018f2000-0000-7000-8000-000000000002",
        "existence-login-user",
    );
    let revoke_task = tokio::spawn(async move {
        revoke_repository
            .admin_revoke_all_user_sessions_with_audit(
                "existence-login-user",
                chrono::Utc::now(),
                "admin_revoke_all_sessions",
                &record,
            )
            .await
    });
    wait_for_lock_wait(&pool, "SELECT id FROM users WHERE id=$1 FOR UPDATE").await;
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(ADVISORY_KEY)
        .execute(&mut *gate)
        .await
        .unwrap();
    assert!(create_task.await.unwrap().unwrap().is_some());
    assert_eq!(
        revoke_task.await.unwrap().unwrap(),
        AdminUserSessionsRevocationOutcome::Revoked(1)
    );
    let revoked_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT revoked_at FROM user_sessions WHERE id='existence-login-session'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(revoked_at.is_some());
    let intents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_audit_delivery")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(intents, 1);
    drop(gate);
    pool.close().await;
}
