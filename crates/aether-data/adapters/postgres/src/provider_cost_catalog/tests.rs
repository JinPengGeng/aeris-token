use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::str::FromStr;

use super::*;
use aether_data_contracts::repository::provider_cost_catalog::ProviderCostTaskType;

fn sample_record(
    cost_id: &str,
    provider_id: &str,
    effective_from: u64,
) -> ProviderCostCatalogRecord {
    ProviderCostCatalogRecord {
        cost_id: cost_id.to_string(),
        provider_id: provider_id.to_string(),
        model: "gpt-x".to_string(),
        task_type: ProviderCostTaskType::Text,
        currency: "USD".to_string(),
        price_per_request: None,
        tiered_pricing: Some(json!({
            "tiers": [{"input_price_per_1m": 0.5, "output_price_per_1m": 1.5}]
        })),
        effective_from_unix_secs: effective_from,
        effective_to_unix_secs: Some(effective_from + 500),
        created_by: "admin-a".to_string(),
        created_at_unix_secs: effective_from - 10,
        updated_at_unix_secs: effective_from - 10,
    }
}

async fn fixture() -> (PgPool, String) {
    let database_url = std::env::var("AETHER_TEST_DATABASE_URL")
        .expect("AETHER_TEST_DATABASE_URL must name a disposable PostgreSQL database");
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .unwrap();
    crate::run_migrations(&admin).await.unwrap();
    let schema = format!("provider_cost_catalog_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = database_url
        .parse::<sqlx::postgres::PgConnectOptions>()
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE provider_costs (LIKE public.provider_costs INCLUDING ALL)")
        .execute(&pool)
        .await
        .unwrap();
    (pool, schema)
}

#[tokio::test]
#[ignore = "requires a disposable AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn upsert_get_list_delete_round_trip() {
    let (pool, _schema) = fixture().await;
    let repository = PostgresProviderCostCatalogRepository::new(pool);
    let record = sample_record("cost-a", "provider-a", 1_000);

    assert_eq!(
        repository
            .upsert_provider_cost_catalog(record.clone())
            .await
            .expect("insert succeeds"),
        ProviderCostCatalogUpsertOutcome::Inserted
    );
    assert_eq!(
        repository
            .get_provider_cost_catalog("cost-a")
            .await
            .expect("get succeeds"),
        Some(record.clone())
    );

    let mut updated = record.clone();
    updated.price_per_request = Some(bigdecimal::BigDecimal::from_str("0.25").unwrap());
    updated.updated_at_unix_secs = 1_100;
    assert_eq!(
        repository
            .upsert_provider_cost_catalog(updated.clone())
            .await
            .expect("update succeeds"),
        ProviderCostCatalogUpsertOutcome::Updated
    );
    assert_eq!(
        repository
            .get_provider_cost_catalog("cost-a")
            .await
            .expect("get succeeds"),
        Some(updated)
    );

    repository
        .upsert_provider_cost_catalog(sample_record("cost-b", "provider-b", 2_000))
        .await
        .expect("second insert succeeds");

    let page = repository
        .list_provider_cost_catalogs(&ProviderCostCatalogListQuery {
            provider_id: Some("provider-a".to_string()),
            model: None,
            task_type: None,
            effective_at_unix_secs: None,
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list succeeds");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].cost_id, "cost-a");

    assert_eq!(
        repository
            .delete_provider_cost_catalog("cost-a")
            .await
            .expect("delete succeeds"),
        ProviderCostCatalogDeleteOutcome::Deleted
    );
    assert_eq!(
        repository
            .delete_provider_cost_catalog("cost-a")
            .await
            .expect("second delete succeeds"),
        ProviderCostCatalogDeleteOutcome::NotFound
    );
}

#[tokio::test]
#[ignore = "requires a disposable AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn list_filters_effective_window_and_paginates() {
    let (pool, _schema) = fixture().await;
    let repository = PostgresProviderCostCatalogRepository::new(pool);
    for (cost_id, from) in [("cost-1", 1_000_u64), ("cost-2", 2_000), ("cost-3", 3_000)] {
        let mut record = sample_record(cost_id, "provider-a", from);
        record.effective_to_unix_secs = None;
        repository
            .upsert_provider_cost_catalog(record)
            .await
            .expect("seed insert succeeds");
    }

    let active = repository
        .list_provider_cost_catalogs(&ProviderCostCatalogListQuery {
            provider_id: None,
            model: None,
            task_type: None,
            effective_at_unix_secs: Some(2_500),
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list succeeds");
    assert_eq!(
        active
            .iter()
            .map(|record| record.cost_id.as_str())
            .collect::<Vec<_>>(),
        vec!["cost-1", "cost-2"]
    );

    let page = repository
        .list_provider_cost_catalogs(&ProviderCostCatalogListQuery {
            provider_id: None,
            model: None,
            task_type: None,
            effective_at_unix_secs: None,
            limit: 2,
            offset: 1,
        })
        .await
        .expect("list succeeds");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].cost_id, "cost-2");
}

#[tokio::test]
#[ignore = "requires a disposable AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn find_effective_picks_latest_window() {
    let (pool, _schema) = fixture().await;
    let repository = PostgresProviderCostCatalogRepository::new(pool);
    repository
        .upsert_provider_cost_catalog(sample_record("cost-old", "provider-a", 1_000))
        .await
        .expect("insert succeeds");
    let mut current = sample_record("cost-new", "provider-a", 2_000);
    current.effective_to_unix_secs = None;
    repository
        .upsert_provider_cost_catalog(current)
        .await
        .expect("insert succeeds");

    let found = repository
        .find_effective_provider_cost_catalog(
            "provider-a",
            "gpt-x",
            ProviderCostTaskType::Text,
            2_500,
        )
        .await
        .expect("lookup succeeds");
    assert_eq!(
        found.map(|record| record.cost_id),
        Some("cost-new".to_string())
    );

    let inside_old = repository
        .find_effective_provider_cost_catalog(
            "provider-a",
            "gpt-x",
            ProviderCostTaskType::Text,
            1_200,
        )
        .await
        .expect("lookup succeeds");
    assert_eq!(
        inside_old.map(|record| record.cost_id),
        Some("cost-old".to_string())
    );

    let before_any = repository
        .find_effective_provider_cost_catalog(
            "provider-a",
            "gpt-x",
            ProviderCostTaskType::Text,
            100,
        )
        .await
        .expect("lookup succeeds");
    assert!(before_any.is_none());
}

#[tokio::test]
#[ignore = "requires a disposable AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn price_per_request_round_trips_exact_decimal() {
    let (pool, _schema) = fixture().await;
    let repository = PostgresProviderCostCatalogRepository::new(pool);
    let mut record = sample_record("cost-a", "provider-a", 1_000);
    record.tiered_pricing = None;
    // 0.1 has no exact float8 representation; NUMERIC(20,8) must preserve it.
    record.price_per_request = Some(bigdecimal::BigDecimal::from_str("0.1").unwrap());
    repository
        .upsert_provider_cost_catalog(record.clone())
        .await
        .expect("insert succeeds");
    let fetched = repository
        .get_provider_cost_catalog("cost-a")
        .await
        .expect("get succeeds")
        .expect("record exists");
    assert_eq!(
        fetched.price_per_request,
        Some(bigdecimal::BigDecimal::from_str("0.1").unwrap())
    );
    assert_eq!(
        serde_json::to_value(&fetched.price_per_request)
            .expect("serializes")
            .as_str(),
        Some("0.1")
    );

    let mut too_precise = sample_record("cost-b", "provider-a", 2_000);
    too_precise.tiered_pricing = None;
    too_precise.price_per_request = Some(bigdecimal::BigDecimal::from_str("0.123456789").unwrap());
    assert!(repository
        .upsert_provider_cost_catalog(too_precise)
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "requires a disposable AETHER_TEST_DATABASE_URL; runs real migrations"]
async fn upsert_rejects_invalid_records() {
    let (pool, _schema) = fixture().await;
    let repository = PostgresProviderCostCatalogRepository::new(pool);
    let mut record = sample_record("cost-a", "provider-a", 1_000);
    record.effective_to_unix_secs = Some(500);
    assert!(repository
        .upsert_provider_cost_catalog(record)
        .await
        .is_err());
}
