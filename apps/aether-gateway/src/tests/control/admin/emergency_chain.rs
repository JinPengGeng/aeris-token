use std::sync::{Arc, Mutex};

use aether_contracts::ExecutionPlan;
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use axum::routing::any;
use axum::{Json, Router};
use http::StatusCode;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::super::{
    build_router_with_state, build_state_with_execution_runtime_override, sample_bound_key,
    sample_endpoint, sample_provider, start_server,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::{GatewayDataConfig, GatewayDataState};

fn contains_emergency_forbidden_tool_field(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "tools" | "tool_choice" | "functions" | "function_call"
            ) || contains_emergency_forbidden_tool_field(value)
        }),
        Value::Array(items) => items.iter().any(contains_emergency_forbidden_tool_field),
        _ => false,
    }
}

#[tokio::test]
#[ignore = "requires a migrated AETHER_TEST_DATABASE_URL"]
async fn live_admin_emergency_chain_uses_declared_order_and_stops_after_success() {
    let database_url = std::env::var("AETHER_TEST_DATABASE_URL")
        .expect("explicit migrated emergency-chain test database is required");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("emergency-chain test database should connect");
    let principal = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO users(id,username,email_verified) VALUES($1,$2,false)")
        .bind(&principal)
        .bind(format!("emergency-http-{}", &principal[..8]))
        .execute(&pool)
        .await
        .expect("administrator fixture should persist");

    let provider_id = format!("provider-emergency-{}", uuid::Uuid::new_v4());
    let endpoint_ids = [
        format!("endpoint-emergency-429-{}", uuid::Uuid::new_v4()),
        format!("endpoint-emergency-200-{}", uuid::Uuid::new_v4()),
        format!("endpoint-emergency-unused-{}", uuid::Uuid::new_v4()),
    ];
    let key_ids = [
        format!("key-emergency-429-{}", uuid::Uuid::new_v4()),
        format!("key-emergency-200-{}", uuid::Uuid::new_v4()),
        format!("key-emergency-unused-{}", uuid::Uuid::new_v4()),
    ];
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let calls_for_runtime = Arc::clone(&calls);
    let first_key_id = key_ids[0].clone();
    let second_key_id = key_ids[1].clone();
    let third_key_id = key_ids[2].clone();
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let calls = Arc::clone(&calls_for_runtime);
            let first_key_id = first_key_id.clone();
            let second_key_id = second_key_id.clone();
            let third_key_id = third_key_id.clone();
            async move {
                calls
                    .lock()
                    .expect("execution call log should lock")
                    .push(plan.key_id.clone());
                assert!(!plan.stream, "emergency execution must be non-streaming");
                let request_body = plan
                    .body
                    .json_body
                    .as_ref()
                    .expect("emergency execution must have a JSON body");
                assert_eq!(request_body.get("stream"), Some(&json!(false)));
                assert!(!contains_emergency_forbidden_tool_field(request_body));
                let (status_code, body) = if plan.key_id == first_key_id {
                    (429, json!({ "error": { "message": "rate limited" } }))
                } else if plan.key_id == second_key_id {
                    (
                        200,
                        json!({
                            "id": "chatcmpl-emergency-success",
                            "object": "chat.completion",
                            "choices": [{
                                "message": {
                                    "role": "assistant",
                                    "content": "emergency target two succeeded"
                                }
                            }]
                        }),
                    )
                } else if plan.key_id == third_key_id {
                    (500, json!({ "error": { "message": "must not be called" } }))
                } else {
                    panic!("unexpected emergency key {}", plan.key_id);
                };
                Json(json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": status_code,
                    "headers": { "content-type": "application/json" },
                    "body": { "json_body": body }
                }))
            }
        }),
    );
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider(&provider_id, "Emergency HTTP provider", 10)],
        endpoint_ids
            .iter()
            .map(|endpoint_id| {
                sample_endpoint(
                    endpoint_id,
                    &provider_id,
                    "openai:chat",
                    "https://emergency.example/v1",
                )
            })
            .collect(),
        // Lower internal priority wins in the ordinary scheduler, so these
        // values deliberately oppose the declared emergency target order.
        key_ids
            .iter()
            .enumerate()
            .map(|(index, key_id)| {
                let mut key =
                    sample_bound_key(key_id, &provider_id, "openai:chat", "sk-emergency-test");
                key.internal_priority = [100, 10, 0][index];
                key
            })
            .collect(),
    ));
    let data_state = GatewayDataState::from_config(
        GatewayDataConfig::from_postgres_url(database_url, false)
            .with_encryption_key(DEVELOPMENT_ENCRYPTION_KEY),
    )
    .expect("PostgreSQL gateway data state should build")
    .attach_provider_catalog_repository_for_tests(provider_catalog_repository);
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let targets: Vec<Value> = endpoint_ids
        .iter()
        .zip(&key_ids)
        .map(|(endpoint_id, key_id)| {
            json!({
                "endpoint_id": endpoint_id,
                "key_id": key_id,
            })
        })
        .collect();
    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/emergency-chain/execute"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, &principal)
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "emergency-http-session")
        .json(&json!({
            "provider_id": provider_id,
            "model": "gpt-4.1",
            "targets": targets,
        }))
        .send()
        .await
        .expect("emergency-chain request should complete");

    let status = response.status();
    let payload: Value = response.json().await.expect("response body should parse");
    assert_eq!(status, StatusCode::OK, "emergency response: {payload}");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["attempts"].as_array().map(Vec::len), Some(2));
    assert_eq!(payload["attempts"][0]["status_code"], json!(429));
    assert_eq!(payload["attempts"][1]["status_code"], json!(200));
    assert_eq!(
        payload["data"]["choices"][0]["message"]["content"],
        json!("emergency target two succeeded")
    );
    assert_eq!(
        *calls.lock().expect("execution call log should lock"),
        vec![key_ids[0].clone(), key_ids[1].clone()]
    );

    let grant_id = payload["grant_id"]
        .as_str()
        .expect("grant id should be returned");
    let request_id = payload["request_id"]
        .as_str()
        .expect("request id should be returned");
    let consumed_at: Option<i64> = sqlx::query_scalar(
        "SELECT consumed_at_unix_secs FROM emergency_chain_grants WHERE grant_id = $1",
    )
    .bind(grant_id)
    .fetch_one(&pool)
    .await
    .expect("persisted grant should be readable");
    assert!(
        consumed_at.is_some(),
        "grant must be consumed before sending"
    );

    let persisted_targets: Vec<(String, String)> = sqlx::query_as(
        "SELECT endpoint_id,key_id FROM emergency_chain_grant_targets WHERE grant_id=$1 ORDER BY chain_position",
    )
    .bind(grant_id)
    .fetch_all(&pool)
    .await
    .expect("persisted target chain should be readable");
    let expected_targets: Vec<(String, String)> = endpoint_ids
        .iter()
        .cloned()
        .zip(key_ids.iter().cloned())
        .collect();
    assert_eq!(persisted_targets, expected_targets);

    let issue_audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_logs WHERE request_id=$1 AND user_id=$2 AND description='admin action: issue_emergency_chain_grant'",
    )
    .bind(request_id)
    .bind(&principal)
    .fetch_one(&pool)
    .await
    .expect("issue audit should be readable");
    assert_eq!(issue_audit_count, 1);

    gateway_handle.abort();
    execution_runtime_handle.abort();
}
