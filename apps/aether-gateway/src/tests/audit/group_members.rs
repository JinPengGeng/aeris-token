//! Authenticated HTTP coverage of the group-members-only durable audit slice.
use std::{sync::Arc, time::Duration};

use aether_data::driver::postgres::PostgresAuditLogReadRepository;
use aether_data::repository::users::{
    InMemoryUserReadRepository, UpsertUserGroupRecord, UserReadRepository,
};
use aether_data_contracts::repository::audit::{
    AdminAuditDeliveryFailureCode, AdminAuditDeliveryFailureOutcome, AuditLogWriteRepository,
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

fn group(name: &str) -> UpsertUserGroupRecord {
    UpsertUserGroupRecord {
        name: name.to_string(),
        description: None,
        priority: 0,
        allowed_providers: None,
        allowed_providers_mode: "unrestricted".to_string(),
        allowed_api_formats: None,
        allowed_api_formats_mode: "unrestricted".to_string(),
        allowed_models: None,
        allowed_models_mode: "unrestricted".to_string(),
        rate_limit: None,
        rate_limit_mode: "system".to_string(),
        daily_usage_limit_usd: None,
        daily_usage_limit_mode: "inherit".to_string(),
    }
}

async fn member_snapshot(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar("SELECT to_jsonb(m) FROM user_group_members m ORDER BY group_id,user_id")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn successful_intents(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT payload FROM admin_audit_delivery
         WHERE payload->'event_metadata'->>'event_name'='admin_user_group_members_updated'
         ORDER BY created_at,event_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires a fresh task-owned aether_admin_audit_* PostgreSQL database"]
async fn authenticated_group_members_mutation_commits_intent_and_retries_without_business_replay() {
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
    let (restricted_token, _) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "audit_admin",
        )
        .await;
    let next_user = state
        .create_local_auth_user_with_settings(
            Some("group-body-private@example.com".to_string()),
            true,
            "group_body_private".to_string(),
            "hash".to_string(),
            "user".to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap()
        .unwrap();
    let target = state
        .create_user_group(group("Audit target"))
        .await
        .unwrap()
        .unwrap();
    let spare = state
        .create_user_group(group("Audit spare"))
        .await
        .unwrap()
        .unwrap();
    state
        .replace_user_groups_for_user(&ordinary.id, std::slice::from_ref(&target.id))
        .await
        .unwrap();
    state
        .data
        .upsert_system_config_value(
            crate::constants::DEFAULT_USER_GROUP_CONFIG_KEY,
            &json!(target.id),
            None,
        )
        .await
        .unwrap();
    let admin_client = client(&admin_token);
    let (gateway, server) = start_server(build_router_with_state(state.clone())).await;
    let endpoint = format!("{gateway}/api/admin/user-groups/{}/members", target.id);
    let initial = member_snapshot(&pool).await;
    let anonymous = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    assert_eq!(
        anonymous
            .put(&endpoint)
            .json(&json!({"user_ids": []}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client(&ordinary_token)
            .put(&endpoint)
            .json(&json!({"user_ids": []}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client(&restricted_token)
            .put(&endpoint)
            .json(&json!({"user_ids": []}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    // Existing default-group safety policy must run before creating an intent.
    assert_eq!(
        admin_client
            .put(&endpoint)
            .json(&json!({"user_ids": []}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        admin_client
            .put(&endpoint)
            .json(&json!({"user_ids": [ordinary.id, "missing-group-user"]}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(successful_intents(&pool).await.is_empty());
    assert_eq!(member_snapshot(&pool).await, initial);
    state
        .add_user_to_group(&spare.id, &ordinary.id)
        .await
        .unwrap();
    let before = member_snapshot(&pool).await;
    // Prime the initiating Gateway's membership cache. This test makes no
    // cross-Gateway cache-invalidation claim.
    assert_eq!(
        state
            .list_user_groups_for_user(&ordinary.id)
            .await
            .unwrap()
            .len(),
        2
    );
    sqlx::raw_sql(
        "CREATE FUNCTION group_http_reject_intent() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'private-group-intent-error'; END $$;
         CREATE TRIGGER group_http_reject_intent BEFORE INSERT ON admin_audit_delivery
         FOR EACH ROW EXECUTE FUNCTION group_http_reject_intent();",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        admin_client
            .put(&endpoint)
            .json(&json!({"user_ids": [next_user.id]}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(member_snapshot(&pool).await, before);
    assert!(successful_intents(&pool).await.is_empty());
    sqlx::raw_sql(
        "DROP TRIGGER group_http_reject_intent ON admin_audit_delivery;
         DROP FUNCTION group_http_reject_intent();
         CREATE FUNCTION group_http_reject_audit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'private-group-delivery-error'; END $$;
         CREATE TRIGGER group_http_reject_audit BEFORE INSERT ON audit_logs
         FOR EACH ROW EXECUTE FUNCTION group_http_reject_audit();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let query_secret = "private-group-query-token";
    let cookie_secret = "private-group-cookie-token";
    let response = admin_client
        .put(format!("{endpoint}?token={query_secret}"))
        .header("cookie", format!("fixture_cookie={cookie_secret}"))
        .json(&json!({"user_ids": [next_user.id, next_user.id]}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "audit delivery failure preserves the committed mutation response"
    );
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["user_id"], next_user.id);
    let actual_members = state
        .data
        .list_user_group_members(&target.id)
        .await
        .unwrap();
    assert_eq!(actual_members.len(), 1);
    assert_eq!(actual_members[0].user_id, next_user.id);
    let cached_groups = state.list_user_groups_for_user(&ordinary.id).await.unwrap();
    assert_eq!(cached_groups.len(), 1);
    assert_eq!(cached_groups[0].id, spare.id);
    let committed = member_snapshot(&pool).await;
    let intents = successful_intents(&pool).await;
    assert_eq!(intents.len(), 1);
    let audit = &intents[0];
    assert_eq!(audit["user_id"], admin.id);
    assert_eq!(audit["request_id"], audit["id"]);
    assert_eq!(audit["event_metadata"]["target_id"], target.id);
    assert_eq!(
        audit["event_metadata"]["path"],
        "/api/admin/user-groups/[group_id]/members"
    );
    assert_eq!(audit["event_metadata"]["route_family"], "users_manage");
    assert_eq!(
        audit["event_metadata"]["route_kind"],
        "replace_user_group_members"
    );
    assert_eq!(audit["event_metadata"]["status"], "completed");
    assert_eq!(audit["event_metadata"]["method"], "PUT");
    assert_eq!(audit["status_code"], 200);
    let serialized = audit.to_string();
    for secret in [
        query_secret,
        cookie_secret,
        &admin_token,
        &next_user.id,
        "group-body-private@example.com",
        "private-group-delivery-error",
    ] {
        assert!(
            !serialized.contains(secret),
            "audit must contain only approved metadata"
        );
    }
    assert!(audit["user_agent"].is_null());
    assert!(audit["error_message"].is_null());
    let event_id = audit["id"].as_str().unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(event_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    let worker = PostgresAuditLogReadRepository::new(pool.clone());
    let claim = worker
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(claim.event_id, event_id);
    assert!(worker
        .deliver_admin_audit(event_id, claim.lease_token)
        .await
        .is_err());
    assert_eq!(
        worker
            .fail_admin_audit_delivery(
                event_id,
                claim.lease_token,
                AdminAuditDeliveryFailureCode::AuditInsertFailed
            )
            .await
            .unwrap(),
        AdminAuditDeliveryFailureOutcome::RetryScheduled
    );
    assert_eq!(member_snapshot(&pool).await, committed);
    sqlx::raw_sql(
        "DROP TRIGGER group_http_reject_audit ON audit_logs;
         DROP FUNCTION group_http_reject_audit();",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE admin_audit_delivery SET next_attempt_at=clock_timestamp() WHERE event_id=$1",
    )
    .bind(event_id)
    .execute(&pool)
    .await
    .unwrap();
    drop(worker);
    let restarted = PostgresAuditLogReadRepository::new(pool.clone());
    let retry = restarted
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(restarted
        .deliver_admin_audit(event_id, retry.lease_token)
        .await
        .unwrap());
    assert_eq!(member_snapshot(&pool).await, committed);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(event_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM admin_audit_delivery WHERE event_id=$1")
            .bind(event_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "delivered"
    );

    // A separate real request exercises immediate persistence and outbox ACK
    // convergence on the same stable event ID. It is a new business mutation.
    let response = admin_client
        .put(&endpoint)
        .json(&json!({"user_ids": [next_user.id]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let intents = successful_intents(&pool).await;
    assert_eq!(intents.len(), 2);
    let second_id = intents
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .find(|id| *id != event_id)
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(second_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1,
        "the outer request awaited persistence"
    );
    let second_committed = member_snapshot(&pool).await;
    let second_claim = restarted
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(second_claim.event_id, second_id);
    assert!(restarted
        .deliver_admin_audit(second_id, second_claim.lease_token)
        .await
        .unwrap());
    assert_eq!(member_snapshot(&pool).await, second_committed);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(second_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    server.abort();
    pool.close().await;
}

#[tokio::test]
async fn authenticated_group_members_keep_non_postgres_fallback() {
    let repository = Arc::new(InMemoryUserReadRepository::default());
    let state = AppState::new()
        .unwrap()
        .without_auth_user_store_for_tests()
        .without_auth_session_store_for_tests()
        .with_data_state_for_tests(GatewayDataState::with_user_reader_for_tests(
            repository.clone(),
        ));
    let (token, admin) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "admin",
        )
        .await;
    let target = repository
        .create_user_group(group("Fallback target"))
        .await
        .unwrap()
        .unwrap();
    // A backend-free test state intentionally reports user mutations as read-only
    // unless its user fixture is present. Keep the same authenticated actor in
    // that fixture; memberships and sessions still use the memory repository.
    let state = state.with_auth_users_for_tests([admin.clone()]);
    let (gateway, server) = start_server(build_router_with_state(state.clone())).await;
    let response = client(&token)
        .put(format!(
            "{gateway}/api/admin/user-groups/{}/members",
            target.id
        ))
        .json(&json!({"user_ids": [admin.id]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["user_id"], admin.id);
    let members = repository
        .list_user_group_members(&target.id)
        .await
        .unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].user_id, admin.id);
    server.abort();
}
