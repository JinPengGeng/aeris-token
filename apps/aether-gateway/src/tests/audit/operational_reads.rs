//! Run from the required administrator audit live target, on its owned database.
use std::time::Duration;

use aether_data::repository::management_tokens::{
    CreateManagementTokenRecord, StoredManagementTokenUserSummary,
};
use aether_data::repository::users::StoredUserAuthRecord;
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::io::AsyncWriteExt;

use crate::tests::{authenticated_operational_client, AppState, OPERATIONAL_ADMIN_DEVICE_ID};

async fn read_events(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT row_to_json(a)::jsonb FROM audit_logs a
         WHERE event_metadata->>'event_name'='admin_operational_read'
         ORDER BY created_at,id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn create_token(
    state: &AppState,
    admin: &StoredUserAuthRecord,
    raw: &str,
    permissions: &[&str],
) -> String {
    let id = uuid::Uuid::now_v7().to_string();
    let result = state
        .data
        .create_management_token(&CreateManagementTokenRecord {
            id: id.clone(),
            user_id: admin.id.clone(),
            user: StoredManagementTokenUserSummary::new(
                admin.id.clone(),
                admin.email.clone(),
                admin.username.clone(),
                admin.role.clone(),
            )
            .unwrap(),
            token_hash: format!("{:x}", Sha256::digest(raw.as_bytes())),
            token_prefix: None,
            name: id.clone(),
            description: None,
            allowed_ips: None,
            permissions: Some(json!(permissions)),
            expires_at_unix_secs: None,
            is_active: true,
        })
        .await
        .unwrap();
    assert!(matches!(result, crate::LocalMutationOutcome::Applied(_)));
    id
}

pub(super) async fn verify_live_reads(
    pool: &PgPool,
    state: &AppState,
    gateway: &str,
    admin: &StoredUserAuthRecord,
    session_token: &str,
    restricted_session_token: &str,
) {
    println!("OPERATIONAL_READS_BEGIN");
    let session = authenticated_operational_client(session_token);
    let session_id: String = sqlx::query_scalar("SELECT id FROM user_sessions WHERE user_id=$1")
        .bind(&admin.id)
        .fetch_one(pool)
        .await
        .unwrap();
    let key_id = uuid::Uuid::now_v7().to_string();
    sqlx::query("INSERT INTO api_keys (id,user_id,key_hash,name) VALUES ($1,$2,$1,'forensic')")
        .bind(&key_id)
        .bind(&admin.id)
        .execute(pool)
        .await
        .unwrap();
    // A partial bundle with real usage/current policy but no candidate history.
    // Captured content must never be copied into the audit event.
    sqlx::query(
        "INSERT INTO usage (id,request_id,user_id,api_key_id,provider_name,model,request_body)
         VALUES ($1,'forensic-target',$2,$3,'fixture','fixture',$4)",
    )
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(&admin.id)
    .bind(&key_id)
    .bind(json!({"prompt":"forensic-stored-body-secret"}))
    .execute(pool)
    .await
    .unwrap();
    let full_raw = "ae-forensic-live-full-secret";
    let limited_raw = "ae-forensic-live-limited-secret";
    let full_id = create_token(
        state,
        admin,
        full_raw,
        &[
            "admin:monitoring:admin",
            "admin:usage:read",
            "admin:api_keys:read",
        ],
    )
    .await;
    let limited_id = create_token(state, admin, limited_raw, &["admin:usage:read"]).await;
    let full = authenticated_operational_client(full_raw);
    let limited = authenticated_operational_client(limited_raw);
    let auth_path = format!("/_gateway/audit/auth/users/{}/api-keys/{key_id}", admin.id);
    let mut request_ids = std::collections::HashSet::new();

    for (client, token_id) in [(&session, None), (&full, Some(&full_id))] {
        for method in [Method::GET, Method::HEAD] {
            for (path, status, action) in [
                (
                    auth_path.as_str(),
                    StatusCode::OK,
                    "read_current_auth_policy",
                ),
                (
                    "/_gateway/audit/request-audit/forensic-target",
                    StatusCode::OK,
                    "read_request_audit_bundle",
                ),
                (
                    "/_gateway/audit/request-usage/forensic-target",
                    StatusCode::OK,
                    "read_request_usage",
                ),
                (
                    "/_gateway/audit/request-candidates/missing",
                    StatusCode::NOT_FOUND,
                    "read_request_candidates",
                ),
                (
                    "/_gateway/audit/decision-trace/missing",
                    StatusCode::NOT_FOUND,
                    "read_request_decision_trace",
                ),
                (
                    "/_gateway/audit/request-audit/missing",
                    StatusCode::NOT_FOUND,
                    "read_request_audit_bundle",
                ),
            ] {
                let before = read_events(pool).await.len();
                let response = client
                    .request(
                        method.clone(),
                        format!("{gateway}{path}?token=forensic-query-secret"),
                    )
                    .header("cookie", "forensic-cookie-secret")
                    .header("user-agent", "forensic-user-agent-secret")
                    .body("forensic-request-body-secret")
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), status, "{method} {path}");
                assert_eq!(response.headers()["cache-control"], "no-store");
                let body = response.bytes().await.unwrap();
                if method == Method::HEAD {
                    assert!(body.is_empty(), "HEAD retains its original empty body");
                } else if path.ends_with("request-audit/forensic-target") {
                    let payload: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(payload["request_id"], "forensic-target");
                    assert!(payload["usage"].is_object());
                    assert!(payload["decision_trace"].is_null());
                    assert_eq!(payload["auth_snapshot"]["api_key_id"], key_id);
                }
                let events = read_events(pool).await;
                assert_eq!(events.len(), before + 1, "audit committed before response");
                let row = events.last().unwrap();
                assert_eq!(row["event_type"], "admin_sensitive_read");
                assert_eq!(row["user_id"], admin.id);
                assert_eq!(row["status_code"], status.as_u16());
                let metadata = &row["event_metadata"];
                assert_eq!(metadata["method"], method.as_str());
                assert_eq!(metadata["action"], action);
                assert_eq!(metadata["target_id"], path);
                assert_eq!(metadata["path"], path);
                assert_eq!(metadata["management_token_id"], json!(token_id));
                assert_eq!(
                    metadata["session_id"],
                    if token_id.is_none() {
                        json!(session_id)
                    } else {
                        Value::Null
                    }
                );
                let read_id = row["request_id"].as_str().unwrap();
                uuid::Uuid::parse_str(read_id).unwrap();
                assert_ne!(read_id, "forensic-target");
                assert!(
                    request_ids.insert(read_id.to_owned()),
                    "every viewing operation has its own identity"
                );
                let serialized = row.to_string();
                for secret in [
                    session_token,
                    full_raw,
                    "forensic-stored-body-secret",
                    "forensic-query-secret",
                    "forensic-cookie-secret",
                    "forensic-user-agent-secret",
                    "forensic-request-body-secret",
                    "admin:monitoring:admin",
                ] {
                    assert!(
                        !serialized.contains(secret),
                        "audit metadata must remain allowlisted"
                    );
                }
                assert!(row["api_key_id"].is_null());
                assert!(row["user_agent"].is_null());
                assert!(row["error_message"].is_null());
            }
        }
    }

    let restricted = authenticated_operational_client(restricted_session_token);
    for client in [&restricted, &limited] {
        for method in [Method::GET, Method::HEAD] {
            let before = read_events(pool).await.len();
            let response = client
                .request(
                    method.clone(),
                    format!("{gateway}/_gateway/audit/request-audit/forensic-target"),
                )
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(response.headers()["cache-control"], "no-store");
            let _ = response.bytes().await.unwrap();
            let events = read_events(pool).await;
            assert_eq!(events.len(), before + 1);
            let row = events.last().unwrap();
            assert_eq!(row["status_code"], 403);
            assert_eq!(row["event_metadata"]["method"], method.as_str());
            assert_eq!(row["event_metadata"]["status"], "failed");
            if std::ptr::eq(client, &limited) {
                assert_eq!(row["user_id"], admin.id);
                assert_eq!(row["event_metadata"]["management_token_id"], limited_id);
                assert!(row["event_metadata"]["session_id"].is_null());
                assert!(!row.to_string().contains(limited_raw));
            } else {
                assert_eq!(row["event_metadata"]["admin_role"], "audit_admin");
                assert!(row["event_metadata"]["session_id"].is_string());
            }
        }
    }
    // Permission-denied tokens retain the existing usage-tracking semantics.
    // Production token deltas are batched; flush through the same repository
    // boundary before inspecting the durable projection.
    state.data.flush_usage_counter_deltas(100).await.unwrap();
    for (id, expected_used) in [(&full_id, true), (&limited_id, false)] {
        let used: bool = sqlx::query_scalar(
            "SELECT last_used_at IS NOT NULL FROM management_tokens WHERE id=$1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(used, expected_used);
    }
    let before = read_events(pool).await.len();
    for method in [Method::GET, Method::HEAD] {
        let response = Client::new()
            .request(method, format!("{gateway}{auth_path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let _ = response.bytes().await.unwrap();
    }
    assert_eq!(
        read_events(pool).await.len(),
        before,
        "no invented anonymous principal"
    );

    verify_persistence_failures(pool, state, gateway, &session).await;
    verify_disconnect(pool, state, gateway, session_token).await;
}

fn metric(state: &AppState, name: &str) -> u64 {
    state
        .admin_audit_metrics
        .metric_samples(true)
        .into_iter()
        .find(|sample| sample.name == name)
        .unwrap()
        .value
}

async fn wait_for_blocked_insert(pool: &PgPool) -> i32 {
    tokio::time::timeout(Duration::from_millis(1500), async {
        loop {
            let pid: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity
                 WHERE datname=current_database() AND wait_event='advisory'
                 AND query LIKE '%INSERT INTO audit_logs%'",
            )
            .fetch_optional(pool)
            .await
            .unwrap();
            if let Some(pid) = pid {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("forensic INSERT must reach the real PostgreSQL lock before its timeout")
}

async fn verify_persistence_failures(
    pool: &PgPool,
    state: &AppState,
    gateway: &str,
    client: &Client,
) {
    let before = read_events(pool).await;
    let failures = metric(state, "admin_audit_persist_failures_total");
    let timeouts = metric(state, "admin_audit_persist_timeouts_total");
    sqlx::query("CREATE TRIGGER reject_forensic_audit_fixture BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION reject_audit_fixture()")
        .execute(pool).await.unwrap();
    let response = client
        .get(format!(
            "{gateway}/_gateway/audit/request-audit/forensic-target"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["request_id"], "forensic-target");
    assert!(!body.to_string().contains("audit-fixture-database-secret"));
    assert_eq!(read_events(pool).await, before);
    assert_eq!(
        metric(state, "admin_audit_persist_failures_total"),
        failures + 1
    );
    assert_eq!(
        metric(state, "admin_audit_persist_timeouts_total"),
        timeouts
    );
    sqlx::raw_sql(
        "DROP TRIGGER reject_forensic_audit_fixture ON audit_logs;
         CREATE TRIGGER wait_forensic_timeout BEFORE INSERT ON audit_logs
         FOR EACH ROW EXECUTE FUNCTION wait_audit_fixture();",
    )
    .execute(pool)
    .await
    .unwrap();
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(255,391)")
        .execute(&mut *lock)
        .await
        .unwrap();
    let timeout_client = client.clone();
    let url = format!("{gateway}/_gateway/audit/request-audit/forensic-timeout-missing");
    let started = std::time::Instant::now();
    let request = tokio::spawn(async move { timeout_client.head(url).send().await.unwrap() });
    let pid = wait_for_blocked_insert(pool).await;
    let response = tokio::time::timeout(Duration::from_millis(2800), request)
        .await
        .unwrap()
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        (Duration::from_millis(1800)..Duration::from_millis(2800)).contains(&elapsed),
        "forensic HEAD audit timeout: {elapsed:?}"
    );
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(response.bytes().await.unwrap().is_empty());
    assert_eq!(read_events(pool).await, before);
    assert_eq!(
        metric(state, "admin_audit_persist_failures_total"),
        failures + 2
    );
    assert_eq!(
        metric(state, "admin_audit_persist_timeouts_total"),
        timeouts + 1
    );
    lock.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let active: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND state='active')",
            )
            .bind(pid)
            .fetch_one(pool)
            .await
            .unwrap();
            if !active {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let after = read_events(pool).await;
    assert!((before.len()..=before.len() + 1).contains(&after.len()));
    for row in &before {
        assert!(after.contains(row));
    }
    if let Some(late) = after.iter().find(|row| !before.contains(row)) {
        assert_eq!(late["event_metadata"]["method"], "HEAD");
        assert_eq!(late["status_code"], 404);
    }
    assert_eq!(
        metric(state, "admin_audit_persist_timeouts_total"),
        timeouts + 1
    );
    sqlx::query("DROP TRIGGER wait_forensic_timeout ON audit_logs")
        .execute(pool)
        .await
        .unwrap();
}

async fn verify_disconnect(pool: &PgPool, state: &AppState, gateway: &str, token: &str) {
    sqlx::raw_sql(
        "CREATE FUNCTION wait_forensic_audit_fixture() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(255,397); RETURN NEW; END $$;
         CREATE TRIGGER wait_forensic_audit_fixture BEFORE INSERT ON audit_logs
         FOR EACH ROW EXECUTE FUNCTION wait_forensic_audit_fixture();",
    )
    .execute(pool)
    .await
    .unwrap();
    let before = read_events(pool).await.len();
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(255,397)")
        .execute(&mut *lock)
        .await
        .unwrap();
    let address = gateway.strip_prefix("http://").unwrap();
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(format!(
        "GET /_gateway/audit/request-audit/forensic-target HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nx-client-device-id: {OPERATIONAL_ADMIN_DEVICE_ID}\r\nConnection: close\r\n\r\n"
    ).as_bytes()).await.unwrap();
    wait_for_blocked_insert(pool).await;
    drop(socket); // No connection pool: close the actual HTTP client transport.
    assert!(state
        .usage_runtime
        .shutdown(Duration::from_millis(80))
        .await
        .is_err());
    assert_eq!(
        state.usage_runtime.metrics_snapshot().producers_in_flight,
        1
    );
    assert_eq!(
        read_events(pool).await.len(),
        before,
        "INSERT is still locked"
    );
    lock.rollback().await.unwrap();
    state
        .usage_runtime
        .shutdown(Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        state.usage_runtime.metrics_snapshot().producers_in_flight,
        0
    );
    let events = read_events(pool).await;
    assert_eq!(
        events.len(),
        before + 1,
        "disconnected read finalizer committed before shutdown"
    );
    let row = events.last().unwrap();
    assert_eq!(row["status_code"], 200);
    assert_eq!(
        row["event_metadata"]["target_id"],
        "/_gateway/audit/request-audit/forensic-target"
    );
}
