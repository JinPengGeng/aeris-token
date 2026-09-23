use serde_json::json;

use super::*;

pub(crate) fn sample_record(
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

#[tokio::test]
async fn upsert_get_list_delete_round_trip() {
    let repository = InMemoryProviderCostCatalogRepository::new();
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
    updated.price_per_request = Some(0.25);
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

    let other = sample_record("cost-b", "provider-b", 2_000);
    repository
        .upsert_provider_cost_catalog(other)
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
async fn list_filters_effective_window_and_paginates() {
    let repository = InMemoryProviderCostCatalogRepository::new();
    for (cost_id, provider_id, from) in [
        ("cost-1", "provider-a", 1_000),
        ("cost-2", "provider-a", 2_000),
        ("cost-3", "provider-a", 3_000),
    ] {
        let mut record = sample_record(cost_id, provider_id, from);
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
async fn find_effective_picks_latest_window() {
    let repository = InMemoryProviderCostCatalogRepository::new();
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
async fn upsert_rejects_invalid_records() {
    let repository = InMemoryProviderCostCatalogRepository::new();
    let mut record = sample_record("cost-a", "provider-a", 1_000);
    record.effective_to_unix_secs = Some(500);
    assert!(repository
        .upsert_provider_cost_catalog(record)
        .await
        .is_err());
}
