//! HTTP acceptance of the two administrator session-revocation audit events.
use std::{sync::Arc, time::Duration};

use aether_data::driver::postgres::PostgresAuditLogReadRepository;
use aether_data::repository::users::{InMemoryUserReadRepository, UserReadRepository};
use aether_data_contracts::repository::audit::{
    AdminAuditDeliveryFailureCode, AdminAuditDeliveryFailureOutcome, AuditLogWriteRepository,
};
use axum::http::StatusCode;
use serde_json::Value;
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

async fn assert_session_access(client: &reqwest::Client, gateway: &str, status: StatusCode) {
    let response = client
        .get(format!("{gateway}/api/auth/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), status);
}

async fn session_snapshot(pool: &PgPool) -> (Vec<Value>, Vec<Value>) {
    // Authentication may touch last_seen_at. Compare revocation state separately
    // and capture every UPDATE OF revoked_at/revoke_reason in the transaction.
    let sessions = sqlx::query_scalar(
        "SELECT jsonb_build_object('id',id,'user_id',user_id,'revoked_at',revoked_at,
           'revoke_reason',revoke_reason) FROM user_sessions ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let writes = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM session_http_write_probe p ORDER BY session_id,updated_at",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    (sessions, writes)
}

async fn intents(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT payload FROM admin_audit_delivery
         WHERE payload->'event_metadata'->>'event_name'
           IN ('admin_user_session_deleted','admin_user_sessions_deleted')
         ORDER BY created_at,event_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn canonical_count(pool: &PgPool, id: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn assert_intent(audit: &Value, actor: &str, target: &str, revoke_all: bool, secrets: &[&str]) {
    let metadata = &audit["event_metadata"];
    assert_eq!(audit["user_id"], actor);
    assert_eq!(audit["request_id"], audit["id"]);
    assert_eq!(audit["status_code"], 200);
    assert_eq!(metadata["status"], "completed");
    assert_eq!(metadata["method"], "DELETE");
    assert_eq!(metadata["route_family"], "users_manage");
    assert_eq!(metadata["target_id"], target);
    assert_eq!(
        metadata["event_name"],
        if revoke_all {
            "admin_user_sessions_deleted"
        } else {
            "admin_user_session_deleted"
        }
    );
    assert_eq!(
        metadata["route_kind"],
        if revoke_all {
            "delete_user_sessions"
        } else {
            "delete_user_session"
        }
    );
    assert_eq!(metadata["action"], metadata["route_kind"]);
    assert_eq!(
        metadata["target_type"],
        if revoke_all { "user" } else { "user_session" }
    );
    assert_eq!(
        metadata["path"],
        if revoke_all {
            "/api/admin/users/[user_id]/sessions"
        } else {
            "/api/admin/users/[user_id]/sessions/[session_id]"
        }
    );
    assert!(audit["user_agent"].is_null());
    assert!(audit["error_message"].is_null());
    let serialized = audit.to_string();
    for secret in secrets {
        assert!(
            !serialized.contains(secret),
            "credential/request data must not enter the intent"
        );
    }
}

#[tokio::test]
#[ignore = "requires a fresh task-owned aether_admin_audit_* PostgreSQL database"]
async fn authenticated_session_revocations_commit_audit_and_recover_without_business_replay() {
    let database_url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PgPool::connect(&database_url).await.unwrap();
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
    assert_eq!(tables, 0, "never clear an existing database");
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
    let (admin_token, admin) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "admin",
        )
        .await;
    let (ordinary_token, ordinary) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "user",
        )
        .await;
    let (all_token, all_user) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "audit_admin",
        )
        .await;
    let single_session = state
        .list_user_sessions(&ordinary.id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let all_primary = state
        .list_user_sessions(&all_user.id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(single_session.user_id, ordinary.id);
    assert_eq!(all_primary.user_id, all_user.id);
    sqlx::query(
        "INSERT INTO user_sessions(id,user_id,client_device_id,refresh_token_hash,expires_at,security_version)
         SELECT 'session-http-extra',id,'session-http-extra-device','private-extra-refresh-hash',
           now()+interval '1 day',security_version FROM users WHERE id=$1",
    ).bind(&all_user.id).execute(&pool).await.unwrap();
    assert_eq!(
        state.list_user_sessions(&all_user.id).await.unwrap().len(),
        2
    );
    sqlx::raw_sql(
        "CREATE TABLE session_http_write_probe (
           session_id text NOT NULL, old_revoked_at timestamptz, new_revoked_at timestamptz,
           updated_at timestamptz);
         CREATE FUNCTION session_http_probe() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           INSERT INTO session_http_write_probe VALUES (NEW.id,OLD.revoked_at,NEW.revoked_at,NEW.updated_at);
           RETURN NEW;
         END $$;
         CREATE TRIGGER session_http_probe AFTER UPDATE OF revoked_at,revoke_reason ON user_sessions
         FOR EACH ROW EXECUTE FUNCTION session_http_probe();",
    ).execute(&pool).await.unwrap();
    let admin_client = client(&admin_token);
    let ordinary_client = client(&ordinary_token);
    let all_client = client(&all_token);
    let (gateway, server) = start_server(build_router_with_state(state.clone())).await;
    let single_endpoint = format!(
        "{gateway}/api/admin/users/{}/sessions/{}",
        ordinary.id, single_session.id
    );
    let all_endpoint = format!("{gateway}/api/admin/users/{}/sessions", all_user.id);
    assert_session_access(&ordinary_client, &gateway, StatusCode::OK).await;
    assert_session_access(&all_client, &gateway, StatusCode::OK).await;
    let before = session_snapshot(&pool).await;
    let anonymous = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    for endpoint in [&single_endpoint, &all_endpoint] {
        assert_eq!(
            anonymous.delete(endpoint).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            ordinary_client
                .delete(endpoint)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            all_client.delete(endpoint).send().await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    for endpoint in [
        format!(
            "{gateway}/api/admin/users/{}/sessions/missing-session",
            ordinary.id
        ),
        format!(
            "{gateway}/api/admin/users/{}/sessions/{}",
            admin.id, single_session.id
        ),
        format!("{gateway}/api/admin/users/missing-user/sessions"),
    ] {
        assert_eq!(
            admin_client.delete(endpoint).send().await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }
    assert!(intents(&pool).await.is_empty());
    assert_eq!(session_snapshot(&pool).await, before);

    sqlx::raw_sql(
        "CREATE FUNCTION session_http_reject_intent() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'private-session-enqueue-error'; END $$;
         CREATE TRIGGER session_http_reject_intent BEFORE INSERT ON admin_audit_delivery
         FOR EACH ROW EXECUTE FUNCTION session_http_reject_intent();",
    )
    .execute(&pool)
    .await
    .unwrap();
    for endpoint in [&single_endpoint, &all_endpoint] {
        assert_eq!(
            admin_client.delete(endpoint).send().await.unwrap().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            session_snapshot(&pool).await,
            before,
            "both revoke transactions must roll back"
        );
        assert!(intents(&pool).await.is_empty());
        assert_session_access(&ordinary_client, &gateway, StatusCode::OK).await;
        assert_session_access(&all_client, &gateway, StatusCode::OK).await;
    }
    sqlx::raw_sql(
        "DROP TRIGGER session_http_reject_intent ON admin_audit_delivery;
         DROP FUNCTION session_http_reject_intent();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let query_secret = "private-session-query-token";
    let cookie_secret = "private-session-cookie-token";
    let trace_secret = "external-session-trace-".repeat(20);
    let response = admin_client
        .delete(format!("{single_endpoint}?token={query_secret}"))
        .header("cookie", format!("fixture_cookie={cookie_secret}"))
        .header(crate::constants::TRACE_ID_HEADER, &trace_secret)
        .header("user-agent", "private-session-user-agent")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_session_access(&ordinary_client, &gateway, StatusCode::UNAUTHORIZED).await;
    assert_session_access(&all_client, &gateway, StatusCode::OK).await;
    let first_snapshot = session_snapshot(&pool).await;
    assert_eq!(first_snapshot.1.len(), 1);
    let queued = intents(&pool).await;
    assert_eq!(queued.len(), 1);
    let first_id = queued[0]["id"].as_str().unwrap().to_string();
    let secrets = [
        query_secret,
        cookie_secret,
        &trace_secret,
        &admin_token,
        &ordinary_token,
        &all_token,
        "private-session-user-agent",
        "private-extra-refresh-hash",
        single_session.refresh_token_hash.as_str(),
    ];
    assert_intent(&queued[0], &admin.id, &single_session.id, false, &secrets);
    assert_eq!(
        queued[0]["event_metadata"]["session_id"],
        "session-operational-admin"
    );
    assert_eq!(
        canonical_count(&pool, &first_id).await,
        1,
        "response awaits canonical persistence"
    );
    let first_canonical: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM audit_logs a WHERE id=$1")
            .bind(&first_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_intent(
        &first_canonical,
        &admin.id,
        &single_session.id,
        false,
        &secrets,
    );

    // A second HTTP request is a new admin event, but AlreadyRevoked must not
    // issue another business UPDATE or alter the original revocation time.
    assert_eq!(
        admin_client
            .delete(&single_endpoint)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(session_snapshot(&pool).await, first_snapshot);
    let queued = intents(&pool).await;
    assert_eq!(queued.len(), 2);
    let delivery = PostgresAuditLogReadRepository::new(pool.clone());
    let claims = delivery.claim_admin_audit_deliveries(2, 30).await.unwrap();
    assert_eq!(claims.len(), 2);
    for claim in claims {
        assert!(queued
            .iter()
            .any(|item| item["id"].as_str() == Some(claim.event_id.as_str())));
        assert!(delivery
            .deliver_admin_audit(&claim.event_id, claim.lease_token)
            .await
            .unwrap());
        assert_eq!(canonical_count(&pool, &claim.event_id).await, 1);
    }
    assert_eq!(session_snapshot(&pool).await, first_snapshot);
    sqlx::raw_sql(
        "CREATE FUNCTION session_http_reject_audit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'private-session-delivery-error'; END $$;
         CREATE TRIGGER session_http_reject_audit BEFORE INSERT ON audit_logs
         FOR EACH ROW EXECUTE FUNCTION session_http_reject_audit();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let response = admin_client
        .delete(format!("{all_endpoint}?token={query_secret}"))
        .header("cookie", format!("fixture_cookie={cookie_secret}"))
        .header(crate::constants::TRACE_ID_HEADER, &trace_secret)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["revoked_count"], 2);
    assert_session_access(&all_client, &gateway, StatusCode::UNAUTHORIZED).await;
    assert_session_access(&admin_client, &gateway, StatusCode::OK).await;
    let all_snapshot = session_snapshot(&pool).await;
    assert_eq!(
        all_snapshot.1.len(),
        3,
        "one single revoke plus two all-session revokes"
    );
    let queued = intents(&pool).await;
    assert_eq!(queued.len(), 3);
    let all_audit = queued
        .iter()
        .find(|item| item["event_metadata"]["event_name"] == "admin_user_sessions_deleted")
        .unwrap();
    assert_intent(all_audit, &admin.id, &all_user.id, true, &secrets);
    let all_id = all_audit["id"].as_str().unwrap();
    assert_eq!(canonical_count(&pool, all_id).await, 0);
    let claim = delivery
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(claim.event_id, all_id);
    assert!(delivery
        .deliver_admin_audit(all_id, claim.lease_token)
        .await
        .is_err());
    assert_eq!(
        delivery
            .fail_admin_audit_delivery(
                all_id,
                claim.lease_token,
                AdminAuditDeliveryFailureCode::AuditInsertFailed
            )
            .await
            .unwrap(),
        AdminAuditDeliveryFailureOutcome::RetryScheduled
    );
    assert_eq!(session_snapshot(&pool).await, all_snapshot);
    sqlx::raw_sql(
        "DROP TRIGGER session_http_reject_audit ON audit_logs;
         DROP FUNCTION session_http_reject_audit();",
    )
    .execute(&pool)
    .await
    .unwrap();
    // This is a technical retry test; real lease expiry is covered separately.
    sqlx::query(
        "UPDATE admin_audit_delivery SET next_attempt_at=clock_timestamp() WHERE event_id=$1",
    )
    .bind(all_id)
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
    assert_eq!(retry.event_id, all_id);
    assert!(restarted
        .deliver_admin_audit(all_id, retry.lease_token)
        .await
        .unwrap());
    assert!(!restarted
        .deliver_admin_audit(all_id, retry.lease_token)
        .await
        .unwrap());
    assert_eq!(canonical_count(&pool, all_id).await, 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM admin_audit_delivery WHERE event_id=$1")
            .bind(all_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "delivered"
    );
    assert_eq!(session_snapshot(&pool).await, all_snapshot);
    let all_canonical: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM audit_logs a WHERE id=$1")
            .bind(all_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_intent(&all_canonical, &admin.id, &all_user.id, true, &secrets);
    let canonical_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE event_metadata->>'event_name'
           IN ('admin_user_session_deleted','admin_user_sessions_deleted')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        canonical_events, 3,
        "marker must prevent a second differently identified finalizer record"
    );
    server.abort();
    pool.close().await;
}

#[tokio::test]
async fn authenticated_session_revocations_keep_memory_fallback() {
    let repository = Arc::new(InMemoryUserReadRepository::default());
    let state = AppState::new()
        .unwrap()
        .without_auth_user_store_for_tests()
        .without_auth_session_store_for_tests()
        .with_data_state_for_tests(GatewayDataState::with_user_reader_for_tests(
            repository.clone(),
        ));
    let (admin_token, admin) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "admin",
        )
        .await;
    let (ordinary_token, ordinary) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "user",
        )
        .await;
    let (all_token, all_user) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "audit_admin",
        )
        .await;
    let single_session = repository
        .list_user_sessions(&ordinary.id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let all_session = repository
        .list_user_sessions(&all_user.id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let (gateway, server) = start_server(build_router_with_state(state)).await;
    let admin_client = client(&admin_token);
    let ordinary_client = client(&ordinary_token);
    let all_client = client(&all_token);
    assert_session_access(&ordinary_client, &gateway, StatusCode::OK).await;
    assert_session_access(&all_client, &gateway, StatusCode::OK).await;
    let wrong_owner = format!(
        "{gateway}/api/admin/users/{}/sessions/{}",
        admin.id, single_session.id
    );
    assert_eq!(
        admin_client
            .delete(wrong_owner)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let single_endpoint = format!(
        "{gateway}/api/admin/users/{}/sessions/{}",
        ordinary.id, single_session.id
    );
    assert_eq!(
        admin_client
            .delete(single_endpoint)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(repository
        .find_user_session(&ordinary.id, &single_session.id)
        .await
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    assert_session_access(&ordinary_client, &gateway, StatusCode::UNAUTHORIZED).await;
    let response = admin_client
        .delete(format!(
            "{gateway}/api/admin/users/{}/sessions",
            all_user.id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["revoked_count"], 1);
    assert!(repository
        .find_user_session(&all_user.id, &all_session.id)
        .await
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    assert_session_access(&all_client, &gateway, StatusCode::UNAUTHORIZED).await;
    assert_session_access(&admin_client, &gateway, StatusCode::OK).await;
    server.abort();
}
