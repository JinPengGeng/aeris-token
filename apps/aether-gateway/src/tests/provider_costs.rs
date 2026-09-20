//! Provider-cost HTTP acceptance against a fresh PostgreSQL database.

use aether_data::repository::provider_cost::{
    ProviderCostCertainty, ProviderCostDimension, ProviderCostPrice,
    ProviderCostReconciliationStatus, ProviderCostSnapshotImport, ProviderCostSourceKind,
    ProviderCostUnit,
};
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use aether_usage_runtime::UsageSettlementWriter;
use axum::http::StatusCode;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::data::{GatewayDataConfig, GatewayDataState};
use crate::tests::{
    authenticated_operational_client, build_router_with_state, start_server, AppState,
    OPERATIONAL_ADMIN_DEVICE_ID,
};

fn price_import(imported_by: &str) -> Value {
    json!({
        "prices": [{
            "import_id": "provider-cost-price-http-1",
            "supplier": "fixture-supplier",
            "provider": "fixture-provider",
            "model": "fixture-model",
            "dimension": "input",
            "currency": "USD",
            "unit": "per_million_tokens",
            "version": "v1",
            "price_units": 125_000_000_u64,
            "effective_from_unix_secs": 1_000_u64,
            "effective_to_unix_secs": null,
            "source_reference": "fixture-price-book",
            "imported_by": imported_by,
        }]
    })
}

fn automatic_price(
    dimension: ProviderCostDimension,
    import_id: &str,
    price_units: u64,
) -> ProviderCostPrice {
    ProviderCostPrice {
        import_id: import_id.to_string(),
        supplier: "fixture-supplier".to_string(),
        provider: "fixture-provider".to_string(),
        model: "fixture-upstream-model".to_string(),
        dimension,
        currency: "USD".to_string(),
        unit: ProviderCostUnit::PerMillionTokens,
        version: "fixture-v1".to_string(),
        price_units,
        effective_from_unix_secs: 1_000,
        effective_to_unix_secs: None,
        source_reference: format!("fixture-{import_id}"),
        imported_by: "fixture-admin".to_string(),
    }
}

fn settled_usage_for_automatic_capture() -> StoredRequestUsageAudit {
    let mut usage = StoredRequestUsageAudit::new(
        "fixture-usage-auto-1".to_string(),
        "fixture-request-auto-1".to_string(),
        Some("fixture-user".to_string()),
        Some("fixture-key".to_string()),
        None,
        None,
        "display-only-provider".to_string(),
        "fixture-model".to_string(),
        Some("fixture-upstream-model".to_string()),
        Some("fixture-provider".to_string()),
        None,
        None,
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        false,
        false,
        120,
        40,
        160,
        0.36,
        0.36,
        Some(200),
        None,
        None,
        None,
        None,
        "completed".to_string(),
        "settled".to_string(),
        2_000,
        2_001,
        Some(2_002),
    )
    .expect("settled fixture usage should build");
    usage.created_at_unix_ms = 2_000_000;
    usage.finalized_at_unix_secs = Some(2_500);
    usage.request_metadata = Some(json!({"usage_available": true}));
    usage
}

#[tokio::test]
#[ignore = "requires a fresh task-owned aether_provider_cost_* PostgreSQL database"]
async fn settled_usage_capture_uses_effective_prices_and_replay_keeps_one_estimate() {
    let database_url = std::env::var("AETHER_TEST_PROVIDER_COST_DATABASE_URL")
        .expect("explicit disposable provider-cost database is required");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("provider-cost fixture database should connect");
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

    let state =
        GatewayDataState::from_config(GatewayDataConfig::from_postgres_url(database_url, false))
            .expect("PostgreSQL gateway data state should build")
            .with_system_config_values_for_tests([(
                "provider_cost_supplier_bindings".to_string(),
                json!({
                    "fixture-provider": {
                        "supplier": "fixture-supplier",
                        "currency": "USD",
                        "input_price_mode": "exclusive_of_cache"
                    }
                }),
            )]);
    let mut old_input = automatic_price(
        ProviderCostDimension::Input,
        "fixture-auto-input-old",
        100_000_000,
    );
    old_input.effective_to_unix_secs = Some(2_100);
    let mut old_output = automatic_price(
        ProviderCostDimension::Output,
        "fixture-auto-output-old",
        200_000_000,
    );
    old_output.effective_to_unix_secs = Some(2_100);
    let mut new_input = automatic_price(
        ProviderCostDimension::Input,
        "fixture-auto-input-new",
        900_000_000,
    );
    new_input.version = "fixture-v2".to_string();
    new_input.effective_from_unix_secs = 2_100;
    let mut new_output = automatic_price(
        ProviderCostDimension::Output,
        "fixture-auto-output-new",
        1_800_000_000,
    );
    new_output.version = "fixture-v2".to_string();
    new_output.effective_from_unix_secs = 2_100;
    for price in [old_input, old_output, new_input, new_output] {
        state
            .import_provider_cost_price(&price)
            .await
            .expect("price import should succeed")
            .expect("PostgreSQL provider cost backend should be available");
    }

    let usage = settled_usage_for_automatic_capture();
    UsageSettlementWriter::capture_provider_cost_for_usage(&state, &usage)
        .await
        .expect("settled usage should create a provider-cost receipt");
    let first: (String, i64, i64, Value, i64) = sqlx::query_as(
        "SELECT certainty, provider_cost_amount_units, occurred_at_unix_secs, price_components, sales_amount_units \
         FROM provider_cost_snapshots WHERE request_id = $1",
    )
    .bind(&usage.request_id)
    .fetch_one(&pool)
    .await
    .expect("automatic snapshot should be stored");
    assert_eq!(first.0, "estimated");
    assert_eq!(first.1, 20_000);
    assert_eq!(first.2, 2_000);
    assert_eq!(first.3.as_array().map(Vec::len), Some(2));
    assert!(first.3.as_array().is_some_and(|components| {
        components.iter().all(|component| {
            component.get("price_version") == Some(&Value::String("fixture-v1".to_string()))
        })
    }));
    assert_eq!(first.4, 36_000_000);

    let automatic_receipt: (String, i64, Value) = sqlx::query_as(
        "SELECT certainty, provider_cost_amount_units, price_components \
         FROM provider_cost_snapshot_imports WHERE import_id = $1",
    )
    .bind(format!("gateway-auto-provider-cost/{}", usage.request_id))
    .fetch_one(&pool)
    .await
    .expect("automatic immutable receipt should be stored");
    assert_eq!(automatic_receipt.0, "estimated");
    assert_eq!(automatic_receipt.1, first.1);
    assert_eq!(automatic_receipt.2, first.3);

    let invoice = ProviderCostSnapshotImport {
        import_id: "fixture-supplier-invoice-1".to_string(),
        request_id: usage.request_id.clone(),
        provider: "fixture-provider".to_string(),
        model: "fixture-upstream-model".to_string(),
        dimension: ProviderCostDimension::Request,
        sales_amount_units: 36_000_000,
        sales_currency: "USD".to_string(),
        provider_cost_amount_units: Some(99_999),
        provider_currency: Some("USD".to_string()),
        certainty: ProviderCostCertainty::Known,
        source_kind: ProviderCostSourceKind::SupplierBill,
        reconciliation_status: ProviderCostReconciliationStatus::Matched,
        price_import_id: None,
        price_version: None,
        source_reference: Some("fixture-supplier-invoice".to_string()),
        price_components: Vec::new(),
        occurred_at_unix_secs: 2_000,
        imported_by: "fixture-admin".to_string(),
    };
    state
        .import_provider_cost_snapshot(&invoice)
        .await
        .expect("supplier invoice should advance the current receipt")
        .expect("PostgreSQL provider cost backend should be available");

    UsageSettlementWriter::capture_provider_cost_for_usage(&state, &usage)
        .await
        .expect("replayed settled usage should be idempotent");
    let replay_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM provider_cost_snapshots WHERE request_id = $1")
            .bind(&usage.request_id)
            .fetch_one(&pool)
            .await
            .expect("automatic snapshot count should be readable");
    assert_eq!(replay_count, 1);
    let replayed_current: (String, i64) = sqlx::query_as(
        "SELECT certainty, provider_cost_amount_units FROM provider_cost_snapshots WHERE request_id = $1",
    )
    .bind(&usage.request_id)
    .fetch_one(&pool)
    .await
    .expect("replayed current receipt should be readable");
    assert_eq!(replayed_current, ("known".to_string(), 99_999));
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM provider_cost_snapshot_imports WHERE request_id = $1",
    )
    .bind(&usage.request_id)
    .fetch_one(&pool)
    .await
    .expect("immutable receipt count should be readable");
    assert_eq!(receipt_count, 2);
}

#[tokio::test]
#[ignore = "requires a fresh task-owned aether_provider_cost_* PostgreSQL database"]
async fn live_provider_cost_http_imports_are_authorized_idempotent_and_keep_unknown_null() {
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
    let (admin_token, admin_user) =
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
    let price_import_url = format!("{gateway}/api/admin/billing/provider-costs/prices/import");

    // A regular user is rejected during admin-principal resolution with 401.
    // Use the non-writing audit_admin role to exercise the authenticated 403
    // branch that protects the provider-cost mutation.
    let forbidden = readonly_client
        .post(&price_import_url)
        .json(&price_import("forged-readonly"))
        .send()
        .await
        .expect("restricted request should complete");
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let imported = admin_client
        .post(&price_import_url)
        .json(&price_import("forged-importer"))
        .send()
        .await
        .expect("admin import should complete");
    assert_eq!(imported.status(), StatusCode::OK);
    let imported: Value = imported
        .json()
        .await
        .expect("import response should decode");
    assert_eq!(imported["inserted"], 1);
    assert_eq!(imported["already_exists"], 0);
    assert_eq!(imported["items"][0]["record"]["imported_by"], admin_user.id);

    let replay = admin_client
        .post(&price_import_url)
        .json(&price_import("different-forged-importer"))
        .send()
        .await
        .expect("idempotent replay should complete");
    assert_eq!(replay.status(), StatusCode::OK);
    let replay: Value = replay.json().await.expect("replay response should decode");
    assert_eq!(replay["inserted"], 0);
    assert_eq!(replay["already_exists"], 1);
    let stored_importers: Vec<String> =
        sqlx::query_scalar("SELECT imported_by FROM provider_cost_prices ORDER BY import_id")
            .fetch_all(&pool)
            .await
            .expect("stored importer should be readable");
    assert_eq!(
        stored_importers.as_slice(),
        std::slice::from_ref(&admin_user.id)
    );

    let invalid = admin_client
        .post(&price_import_url)
        .json(&json!({
            "prices": [{
                "import_id": "provider-cost-price-invalid",
                "supplier": "fixture-supplier",
                "provider": "fixture-provider",
                "model": "fixture-model-invalid",
                "dimension": "input",
                "currency": "USD",
                "unit": "per_million_tokens",
                "version": "v1",
                "price_units": 1_u64,
                "effective_from_unix_secs": 2_000_u64,
                "effective_to_unix_secs": 2_000_u64,
                "source_reference": "fixture-invalid-window",
                "imported_by": "forged-importer",
            }]
        }))
        .send()
        .await
        .expect("invalid import should complete");
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let snapshot_import_url =
        format!("{gateway}/api/admin/billing/provider-costs/snapshots/import");
    let unknown = admin_client
        .post(&snapshot_import_url)
        .json(&json!({
            "snapshots": [{
                "import_id": "provider-cost-snapshot-http-1",
                "request_id": "provider-cost-request-http-1",
                "provider": "fixture-provider",
                "model": "fixture-model",
                "dimension": "input",
                "sales_amount_units": 900_u64,
                "sales_currency": "USD",
                "provider_cost_amount_units": null,
                "provider_currency": null,
                "certainty": "unknown",
                "source_kind": "manual_import",
                "reconciliation_status": "unreconciled",
                "price_import_id": null,
                "price_version": null,
                "source_reference": null,
                "occurred_at_unix_secs": 1_050_u64,
                "imported_by": "forged-importer",
            }]
        }))
        .send()
        .await
        .expect("unknown snapshot import should complete");
    assert_eq!(unknown.status(), StatusCode::OK);
    let unknown: Value = unknown
        .json()
        .await
        .expect("unknown snapshot response should decode");
    assert_eq!(
        unknown["items"][0]["record"]["import"]["imported_by"],
        admin_user.id
    );

    let summary = admin_client
        .get(format!(
            "{gateway}/api/admin/billing/provider-costs/snapshots?from=1000&until=1100"
        ))
        .send()
        .await
        .expect("summary request should complete");
    assert_eq!(summary.status(), StatusCode::OK);
    let summary: Value = summary
        .json()
        .await
        .expect("summary response should decode");
    assert_eq!(summary["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(summary["items"][0]["certainty"], "unknown");
    assert!(summary["items"][0]["provider_cost_amount_units"].is_null());
    assert!(summary["items"][0]["margin_amount_units"].is_null());
    assert_eq!(summary["items"][0]["unknown_count"], 1);

    server.abort();
}
