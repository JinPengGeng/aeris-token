//! Public HTTP mutation -> lifecycle finalizer -> PostgreSQL -> protected audit API.
use std::time::Duration;

use aether_data::driver::postgres::PostgresAuditLogReadRepository;
use aether_data_contracts::repository::audit::{
    AuditLogWriteOutcome, AuditLogWriteRepository, CreateAdminAuditLog,
};
use axum::http::StatusCode;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::data::{GatewayDataConfig, GatewayDataState};
use crate::tests::{
    authenticated_operational_client, build_router_with_state, start_server, AppState,
    OPERATIONAL_ADMIN_DEVICE_ID,
};

async fn audit_rows(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT row_to_json(a)::jsonb FROM audit_logs a
         WHERE event_type='admin_mutation' ORDER BY created_at,id",
    )
    .fetch_all(pool)
    .await
    .expect("committed audit rows should be readable")
}

async fn assert_audit_metrics(
    client: &reqwest::Client,
    gateway: &str,
    attempts: u64,
    failures: u64,
    timeouts: u64,
) {
    let response = client
        .get(format!("{gateway}/_gateway/metrics"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = response.text().await.unwrap();
    let mut observed = Vec::new();
    for (name, kind, value) in [
        ("admin_audit_persist_attempts_total", "counter", attempts),
        ("admin_audit_persist_failures_total", "counter", failures),
        ("admin_audit_persist_timeouts_total", "counter", timeouts),
        ("durable_admin_audit_available", "gauge", 1),
    ] {
        let name = format!("aether_gateway_{name}");
        let declarations: Vec<_> = body
            .lines()
            .filter(|line| line.starts_with(&format!("# TYPE {name} ")))
            .collect();
        assert_eq!(declarations, [format!("# TYPE {name} {kind}")]);
        let samples: Vec<_> = body
            .lines()
            .filter(|line| line.starts_with(&format!("{name}{{")))
            .collect();
        assert_eq!(
            samples,
            [format!("{name}{{component=\"gateway\"}} {value}")]
        );
        observed.extend(body.lines().filter(|line| {
            line.starts_with(&format!("# HELP {name} "))
                || line.starts_with(&format!("# TYPE {name} "))
                || line.starts_with(&format!("{name}{{"))
        }));
    }
    // Keep bounded, real HTTP exposition for independent promtool validation.
    println!(
        "\nAUDIT_METRICS_BEGIN\n{}\nAUDIT_METRICS_END",
        observed.join("\n")
    );
}

#[tokio::test]
async fn missing_audit_writer_is_visible_across_app_state_clones() {
    let state = AppState::new().unwrap().with_data_state_for_tests(
        GatewayDataState::from_config(GatewayDataConfig::default()).unwrap(),
    );
    let request_state = state.clone();
    let record = CreateAdminAuditLog {
        id: uuid::Uuid::now_v7().to_string(),
        event_type: "admin_mutation".into(),
        user_id: None,
        api_key_id: None,
        description: "unavailable writer fixture".into(),
        ip_address: None,
        user_agent: None,
        request_id: None,
        event_metadata: None,
        status_code: Some(200),
        error_message: None,
        created_at: chrono::Utc::now(),
    };
    crate::audit::persist_admin_audit(
        &request_state.data,
        &request_state.admin_audit_metrics,
        record,
    )
    .await;
    let samples = state.metric_samples().await;
    for (name, value) in [
        ("admin_audit_persist_attempts_total", 1),
        ("admin_audit_persist_failures_total", 1),
        ("admin_audit_persist_timeouts_total", 0),
        ("durable_admin_audit_available", 0),
    ] {
        let matching: Vec<_> = samples
            .iter()
            .filter(|sample| sample.name == name)
            .collect();
        assert_eq!(matching.len(), 1, "each family appears once");
        assert_eq!(matching[0].value, value, "{name}");
    }
}

#[tokio::test]
#[ignore = "requires a fresh task-owned aether_admin_audit_* PostgreSQL database"]
async fn live_admin_mutations_persist_before_response_and_protected_readback() {
    aether_runtime::metrics::init_metrics(
        aether_runtime::ServiceRuntimeConfig::new("gateway", "warn")
            .with_metrics_namespace("aether_gateway"),
    );
    let database_url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL")
        .expect("explicit disposable administrator audit database is required");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("audit fixture database should connect");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("aether_admin_audit_"));
    let tables: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pg_catalog.pg_tables WHERE schemaname='public'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(tables, 0, "fixture never clears an existing database");
    aether_data::lifecycle::migrate::prepare_database_for_startup(&pool)
        .await
        .unwrap();
    aether_data::lifecycle::migrate::run_migrations(&pool)
        .await
        .unwrap();

    let state = AppState::new()
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
    // Persist both identities and sessions in this PostgreSQL backend. The
    // default AppState test stores bypass PostgreSQL and cannot prove this join.
    let (token, admin_user) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "admin",
        )
        .await;
    let (ordinary_token, _) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "user",
        )
        .await;
    let (audit_admin_token, _) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "audit_admin",
        )
        .await;
    let persisted_users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(persisted_users, 3);
    let client = authenticated_operational_client(&token);
    let (gateway, server) = start_server(build_router_with_state(state.clone())).await;
    let endpoint = format!("{gateway}/api/admin/system/configs/enable_format_conversion");
    let query_secret = "audit-fixture-query-secret";
    let cookie_secret = "audit-fixture-cookie-secret";
    let body_secret = "audit-fixture-description-secret";
    assert_audit_metrics(&client, &gateway, 0, 0, 0).await;

    let response = client
        .put(format!("{endpoint}?token={query_secret}"))
        .header("cookie", format!("fixture_cookie={cookie_secret}"))
        .json(&json!({"value": false, "description": body_secret}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = response.text().await.unwrap();
    assert!(!response_body.contains(query_secret));
    assert_eq!(
        state
            .data
            .find_system_config_value_strong("enable_format_conversion")
            .await
            .unwrap(),
        Some(json!(false))
    );
    // No polling: the business response must already have awaited audit persistence.
    let rows = audit_rows(&pool).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["status_code"], 200);
    assert_eq!(rows[0]["user_id"], admin_user.id);
    assert_eq!(rows[0]["event_type"], "admin_mutation");
    assert_eq!(rows[0]["event_metadata"]["method"], "PUT");
    assert_eq!(rows[0]["event_metadata"]["status"], "completed");
    for secret in [query_secret, cookie_secret, body_secret, token.as_str()] {
        assert!(!serde_json::to_string(&rows).unwrap().contains(secret));
    }
    assert!(rows[0]["user_agent"].is_null());
    assert!(rows[0]["error_message"].is_null());
    assert_audit_metrics(&client, &gateway, 1, 0, 0).await;

    let audit_url = format!("{gateway}/api/admin/monitoring/audit-logs?event_type=admin_mutation");
    let readback = client.get(&audit_url).send().await.unwrap();
    assert_eq!(readback.status(), StatusCode::OK);
    let readback: Value = readback.json().await.unwrap();
    assert_eq!(readback["meta"]["total"], 1);
    assert_eq!(readback["items"][0]["id"], rows[0]["id"]);
    assert_eq!(readback["items"][0]["user_username"], admin_user.username);
    assert_eq!(readback["items"][0]["user_id"], admin_user.id);
    let sensitive_reads: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs
         WHERE event_type='admin_sensitive_read' AND user_id=$1 AND status_code=200",
    )
    .bind(&admin_user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(sensitive_reads, 1, "protected readback itself is audited");
    assert_audit_metrics(&client, &gateway, 2, 0, 0).await;
    assert_eq!(
        reqwest::Client::new()
            .get(&audit_url)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        authenticated_operational_client(&ordinary_token)
            .get(&audit_url)
            .send()
            .await
            .unwrap()
            .status(),
        // Existing admin-principal resolution does not accept ordinary users.
        StatusCode::UNAUTHORIZED
    );
    let restricted = authenticated_operational_client(&audit_admin_token)
        .get(format!("{gateway}/_gateway/audit/request-audit/not-found"))
        .send()
        .await
        .unwrap();
    assert_eq!(restricted.status(), StatusCode::FORBIDDEN);
    let restricted: Value = restricted.json().await.unwrap();
    assert_eq!(restricted["required_permission"], "admin:monitoring:admin");

    // Invalid input still has an identified administrator and an audited final status.
    let invalid = client
        .put(&endpoint)
        .json(&json!({"value": true, "description": {"unsupported": true}}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        state
            .data
            .find_system_config_value_strong("enable_format_conversion")
            .await
            .unwrap(),
        Some(json!(false)),
        "rejected configuration input must not apply the business mutation"
    );
    let failed_rows = audit_rows(&pool).await;
    assert_eq!(failed_rows.len(), 2);
    let failed = failed_rows
        .iter()
        .find(|row| row["status_code"] == 400)
        .unwrap();
    assert_eq!(failed["user_id"], rows[0]["user_id"]);
    assert_eq!(failed["event_metadata"]["status"], "failed");
    assert_audit_metrics(&client, &gateway, 3, 0, 0).await;

    let record: CreateAdminAuditLog = serde_json::from_value(rows[0].clone()).unwrap();
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    assert_eq!(
        repository.create_admin_audit_log(&record).await.unwrap(),
        AuditLogWriteOutcome::AlreadyExists
    );
    let mut conflicting_replay = record.clone();
    conflicting_replay.description = "must not replace the original event".into();
    assert_eq!(
        repository
            .create_admin_audit_log(&conflicting_replay)
            .await
            .unwrap(),
        AuditLogWriteOutcome::AlreadyExists
    );
    assert_eq!(audit_rows(&pool).await, failed_rows);
    // Repository-only replay above is outside the observed persistence boundary.
    assert_audit_metrics(&client, &gateway, 3, 0, 0).await;
    crate::audit::persist_admin_audit(&state.data, &state.admin_audit_metrics, record).await;
    assert_eq!(audit_rows(&pool).await, failed_rows);
    assert_audit_metrics(&client, &gateway, 4, 0, 0).await;

    // A real database error must not turn an applied mutation into a retryable 5xx.
    sqlx::raw_sql(
        "CREATE FUNCTION reject_audit_fixture() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'audit-fixture-database-secret'; END $$;
         CREATE TRIGGER reject_audit_fixture BEFORE INSERT ON audit_logs
         FOR EACH ROW EXECUTE FUNCTION reject_audit_fixture();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let response = client
        .put(&endpoint)
        .json(&json!({"value": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response
        .text()
        .await
        .unwrap()
        .contains("audit-fixture-database-secret"));
    assert_eq!(
        state
            .data
            .find_system_config_value_strong("enable_format_conversion")
            .await
            .unwrap(),
        Some(json!(true))
    );
    assert_eq!(audit_rows(&pool).await, failed_rows);
    // A configured writer remains available as a capability even when INSERT fails.
    assert_audit_metrics(&client, &gateway, 5, 1, 0).await;

    // Hold the writer in a real PostgreSQL BEFORE INSERT trigger until after
    // the HTTP response. This proves the production timeout, without a mock
    // writer or relying on an arbitrary sleep being long enough.
    sqlx::raw_sql(
        "DROP TRIGGER reject_audit_fixture ON audit_logs;
         CREATE FUNCTION wait_audit_fixture() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(255,391); RETURN NEW; END $$;
         CREATE TRIGGER wait_audit_fixture BEFORE INSERT ON audit_logs
         FOR EACH ROW EXECUTE FUNCTION wait_audit_fixture();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(255,391)")
        .execute(&mut *lock)
        .await
        .unwrap();
    let timeout_client = client.clone();
    let request_started = std::time::Instant::now();
    let request = tokio::spawn(async move {
        timeout_client
            .put(&endpoint)
            .json(&json!({"value": false}))
            .send()
            .await
            .unwrap()
    });
    let writer_pid = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let pid: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity
                 WHERE datname=current_database() AND wait_event='advisory'
                 AND query LIKE '%INSERT INTO audit_logs%'",
            )
            .fetch_optional(&pool)
            .await
            .unwrap();
            if let Some(pid) = pid {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("audit writer should reach the database lock");
    // The writer is known to be blocked. Bound both the remaining wait and
    // the total HTTP duration (including persistence) around two seconds;
    // this is not a measurement of database lock time alone.
    let response = tokio::time::timeout(Duration::from_millis(2800), request)
        .await
        .expect("two-second audit timeout must return while the database remains blocked")
        .unwrap();
    let elapsed = request_started.elapsed();
    assert!(
        (Duration::from_millis(1800)..Duration::from_millis(2800)).contains(&elapsed),
        "the blocked request must preserve the two-second timeout; observed {elapsed:?}"
    );
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.bytes().await.unwrap();
    assert_eq!(
        state
            .data
            .find_system_config_value_strong("enable_format_conversion")
            .await
            .unwrap(),
        Some(json!(false))
    );
    // Absence is provable only while this BEFORE INSERT lock is held.
    assert_eq!(audit_rows(&pool).await, failed_rows);
    assert_audit_metrics(&client, &gateway, 6, 2, 1).await;
    lock.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let active: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND state='active')",
            )
            .bind(writer_pid)
            .fetch_one(&pool)
            .await
            .unwrap();
            if !active {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("timed-out writer should finish once the fixture releases the lock");
    // Dropping the SQL future does not prove whether PostgreSQL committed.
    // Preserve either legitimate outcome, but never lose/replace earlier facts.
    let settled = audit_rows(&pool).await;
    assert!((failed_rows.len()..=failed_rows.len() + 1).contains(&settled.len()));
    for prior in &failed_rows {
        assert!(settled.contains(prior));
    }
    if let Some(late) = settled.iter().find(|row| !failed_rows.contains(row)) {
        assert_eq!(late["user_id"], admin_user.id);
        assert_eq!(late["status_code"], 200);
        assert_eq!(late["event_metadata"]["status"], "completed");
    }
    // A possible late commit does not erase the timeout, and scrapes add no attempts.
    assert_audit_metrics(&client, &gateway, 6, 2, 1).await;

    server.abort();
    let _ = server.await;
    drop(state);
    pool.close().await;
}
