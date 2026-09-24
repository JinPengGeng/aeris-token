//! Provider cost catalog HTTP acceptance against a fresh PostgreSQL database.

use axum::http::StatusCode;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::data::{GatewayDataConfig, GatewayDataState};
use crate::tests::{
    authenticated_operational_client, build_router_with_state, start_server, AppState,
    OPERATIONAL_ADMIN_DEVICE_ID,
};

fn catalog_write() -> Value {
    json!({
        "provider_id": "fixture-provider",
        "model": "fixture-model",
        "task_type": "text",
        "currency": "USD",
        "tiered_pricing": {
            "tiers": [{
                "input_price_per_1m": 0.5,
                "output_price_per_1m": 1.5
            }]
        },
        "effective_from_unix_secs": 1_000,
        "effective_to_unix_secs": null
    })
}

#[tokio::test]
#[ignore = "requires a fresh task-owned aether_provider_cost_* PostgreSQL database"]
async fn provider_cost_catalog_http_crud_round_trip() {
    let database_url = std::env::var("AETHER_TEST_PROVIDER_COST_DATABASE_URL")
        .expect("explicit disposable provider-cost database is required");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("provider-cost fixture database should connect");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("fixture database name should be readable");
    assert!(database.starts_with("aether_provider_cost_"));
    let tables: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pg_catalog.pg_tables WHERE schemaname='public'")
            .fetch_one(&pool)
            .await
            .expect("fixture table count should be readable");
    assert_eq!(tables, 0, "fixture never clears an existing database");
    aether_data::lifecycle::migrate::prepare_database_for_startup(&pool)
        .await
        .expect("fixture database should bootstrap");
    aether_data::lifecycle::migrate::run_migrations(&pool)
        .await
        .expect("fixture database should migrate");

    let state = AppState::new()
        .expect("gateway state should build")
        .without_auth_user_store_for_tests()
        .without_auth_session_store_for_tests()
        .with_data_state_for_tests(
            GatewayDataState::from_config(GatewayDataConfig::from_postgres_url(
                database_url,
                false,
            ))
            .expect("PostgreSQL gateway data state should build"),
        );
    let (admin_token, _) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "admin",
        )
        .await;
    let (readonly_token, _) =
        crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state,
            OPERATIONAL_ADMIN_DEVICE_ID,
            "audit_admin",
        )
        .await;
    let admin_client = authenticated_operational_client(&admin_token);
    let readonly_client = authenticated_operational_client(&readonly_token);
    let (gateway, server) = start_server(build_router_with_state(state)).await;
    let base = format!("{gateway}/api/admin/billing/provider-cost-catalogs");

    let forbidden = readonly_client
        .post(&base)
        .json(&catalog_write())
        .send()
        .await
        .expect("restricted request should complete");
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let invalid = admin_client
        .post(&base)
        .json(&json!({
            "provider_id": "fixture-provider",
            "model": "fixture-model",
            "task_type": "video",
            "effective_from_unix_secs": 1_000
        }))
        .send()
        .await
        .expect("invalid request should complete");
    let invalid_status = invalid.status();
    let invalid_body = invalid.text().await.expect("invalid body should read");
    assert_eq!(
        invalid_status,
        StatusCode::BAD_REQUEST,
        "invalid body: {invalid_body}"
    );

    let created = admin_client
        .post(&base)
        .json(&catalog_write())
        .send()
        .await
        .expect("admin create should complete");
    assert_eq!(created.status(), StatusCode::OK, "create: {created:?}");
    let created: Value = created.json().await.expect("create body should decode");
    assert_eq!(created["outcome"], "inserted");

    let listed = admin_client
        .get(&base)
        .query(&[
            ("provider_id", "fixture-provider"),
            ("task_type", "text"),
            ("effective_at", "1500"),
        ])
        .send()
        .await
        .expect("admin list should complete");
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: Value = listed.json().await.expect("list body should decode");
    let items = listed["items"]
        .as_array()
        .expect("items should be an array");
    assert_eq!(items.len(), 1);
    let cost_id = items[0]["cost_id"]
        .as_str()
        .expect("cost id should be present")
        .to_string();

    let fetched = admin_client
        .get(format!("{base}/{cost_id}"))
        .send()
        .await
        .expect("admin get should complete");
    assert_eq!(fetched.status(), StatusCode::OK);
    let fetched: Value = fetched.json().await.expect("get body should decode");
    assert_eq!(
        fetched["item"]["tiered_pricing"]["tiers"][0]["input_price_per_1m"],
        0.5
    );

    let effective = admin_client
        .get(format!("{base}/effective"))
        .query(&[
            ("provider_id", "fixture-provider"),
            ("model", "fixture-model"),
            ("task_type", "text"),
            ("at", "1500"),
        ])
        .send()
        .await
        .expect("effective lookup should complete");
    assert_eq!(effective.status(), StatusCode::OK);
    let effective: Value = effective
        .json()
        .await
        .expect("effective body should decode");
    assert_eq!(effective["item"]["cost_id"], cost_id);

    let updated = admin_client
        .put(format!("{base}/{cost_id}"))
        .json(&json!({
            "provider_id": "fixture-provider",
            "model": "fixture-model",
            "task_type": "text",
            "price_per_request": 0.01,
            "effective_from_unix_secs": 1_000,
            "effective_to_unix_secs": 2_000
        }))
        .send()
        .await
        .expect("admin update should complete");
    assert_eq!(updated.status(), StatusCode::OK, "update: {updated:?}");
    let updated: Value = updated.json().await.expect("update body should decode");
    assert_eq!(updated["outcome"], "updated");

    let refetched = admin_client
        .get(format!("{base}/{cost_id}"))
        .send()
        .await
        .expect("admin get should complete");
    let refetched: Value = refetched.json().await.expect("get body should decode");
    assert_eq!(refetched["item"]["price_per_request"], "0.01");
    assert_eq!(refetched["item"]["tiered_pricing"], Value::Null);

    let deleted = admin_client
        .delete(format!("{base}/{cost_id}"))
        .send()
        .await
        .expect("admin delete should complete");
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted: Value = deleted.json().await.expect("delete body should decode");
    assert_eq!(deleted["deleted"], true);

    let missing = admin_client
        .get(format!("{base}/{cost_id}"))
        .send()
        .await
        .expect("admin get should complete");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    server.abort();
}
