//! Live retention tests require a fresh, migrated, task-owned audit database.

use std::time::Duration;

use aether_data_contracts::repository::audit::{
    AuditLogReadRepository, AuditLogWriteRepository, CreateAdminAuditLog,
};
use chrono::{TimeZone, Utc};
use serde_json::json;

use super::*;

fn record(id: &str) -> CreateAdminAuditLog {
    CreateAdminAuditLog {
        id: id.to_string(),
        event_type: "admin_mutation".to_string(),
        user_id: Some("retention-admin".to_string()),
        api_key_id: None,
        description: "admin action: retention_fixture".to_string(),
        ip_address: Some("127.0.0.1".to_string()),
        user_agent: Some("retention-test".to_string()),
        request_id: Some(format!("request-{id}")),
        event_metadata: Some(json!({
            "schema_version": 1,
            "event_name": "admin_mutation_completed",
            "status": "completed",
            "route_family": "admin",
            "route_kind": "retention_fixture",
            "method": "PUT",
            "path": "/api/admin/retention-fixture",
            "action": "retention_fixture",
            "target_type": "fixture",
            "target_id": id,
        })),
        status_code: Some(200),
        error_message: None,
        created_at: Utc::now(),
    }
}

async fn pool() -> PostgresPool {
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PostgresPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("aether_admin_audit_"));
    pool
}

async fn insert_audit(
    pool: &PostgresPool,
    record: &CreateAdminAuditLog,
    created_at: chrono::DateTime<Utc>,
) {
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    repository.create_admin_audit_log(record).await.unwrap();
    sqlx::query("UPDATE audit_logs SET created_at=$2 WHERE id=$1")
        .bind(&record.id)
        .bind(created_at)
        .execute(pool)
        .await
        .unwrap();
}

async fn insert_delivery(
    pool: &PostgresPool,
    event_id: &str,
    payload: serde_json::Value,
    state: &str,
) {
    let sql = match state {
        "pending" => {
            "INSERT INTO admin_audit_delivery(event_id,payload,state) VALUES($1,$2,'pending')"
        }
        "leased" => {
            "INSERT INTO admin_audit_delivery(event_id,payload,state,lease_token,lease_expires_at) \
             VALUES($1,$2,'leased',gen_random_uuid(),clock_timestamp()+interval '1 hour')"
        }
        "delivered" => {
            "INSERT INTO admin_audit_delivery(event_id,payload,state,delivered_at) \
             VALUES($1,$2,'delivered',clock_timestamp())"
        }
        "dead_letter" => {
            "INSERT INTO admin_audit_delivery(event_id,payload,state,dead_lettered_at,last_error_code) \
             VALUES($1,$2,'dead_letter',clock_timestamp(),'invalid_payload')"
        }
        _ => panic!("unsupported fixture state"),
    };
    sqlx::query(sql)
        .bind(event_id)
        .bind(payload)
        .execute(pool)
        .await
        .unwrap();
}

async fn exists(pool: &PostgresPool, table: &str, id_column: &str, id: &str) -> bool {
    let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {id_column}=$1)");
    sqlx::query_scalar(&query)
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn audit_retention_deletes_only_eligible_rows_with_exact_cutoff_and_batches() {
    let pool = pool().await;
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    let cutoff = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let old = cutoff - chrono::Duration::seconds(1);

    let delivered = record(&uuid::Uuid::now_v7().to_string());
    insert_audit(&pool, &delivered, old).await;
    insert_delivery(
        &pool,
        &delivered.id,
        serde_json::to_value(&delivered).unwrap(),
        "delivered",
    )
    .await;

    let ordinary = record(&uuid::Uuid::now_v7().to_string());
    insert_audit(&pool, &ordinary, old).await;

    let second_delivered = record(&uuid::Uuid::now_v7().to_string());
    insert_audit(&pool, &second_delivered, old).await;
    insert_delivery(
        &pool,
        &second_delivered.id,
        serde_json::to_value(&second_delivered).unwrap(),
        "delivered",
    )
    .await;

    let exact_cutoff = record(&uuid::Uuid::now_v7().to_string());
    insert_audit(&pool, &exact_cutoff, cutoff).await;

    assert_eq!(
        repository
            .delete_audit_logs_before(cutoff.timestamp() as u64, 0)
            .await
            .unwrap(),
        0
    );
    for item in [&delivered, &ordinary, &second_delivered, &exact_cutoff] {
        assert!(exists(&pool, "audit_logs", "id", &item.id).await);
    }

    assert_eq!(
        repository
            .delete_audit_logs_before(cutoff.timestamp() as u64, 2)
            .await
            .unwrap(),
        2
    );
    let mut ordered_ids = [&delivered.id, &ordinary.id, &second_delivered.id];
    ordered_ids.sort();
    for (index, id) in ordered_ids.into_iter().enumerate() {
        assert_eq!(exists(&pool, "audit_logs", "id", id).await, index == 2);
    }
    assert_eq!(
        repository
            .delete_audit_logs_before(cutoff.timestamp() as u64, 2)
            .await
            .unwrap(),
        1
    );
    for item in [&delivered, &ordinary, &second_delivered] {
        assert!(!exists(&pool, "audit_logs", "id", &item.id).await);
    }
    for item in [&delivered, &second_delivered] {
        assert!(!exists(&pool, "admin_audit_delivery", "event_id", &item.id).await);
    }
    assert!(exists(&pool, "audit_logs", "id", &exact_cutoff.id).await);
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn audit_retention_preserves_unresolved_orphan_and_malformed_delivery_rows() {
    let pool = pool().await;
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    let cutoff = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let old = cutoff - chrono::Duration::seconds(1);

    let mut unresolved = Vec::new();
    for state in ["pending", "leased", "dead_letter"] {
        let item = record(&uuid::Uuid::now_v7().to_string());
        insert_audit(&pool, &item, old).await;
        insert_delivery(&pool, &item.id, serde_json::to_value(&item).unwrap(), state).await;
        unresolved.push(item);
    }

    let malformed = record(&uuid::Uuid::now_v7().to_string());
    insert_audit(&pool, &malformed, old).await;
    let wrong_payload = serde_json::to_value(record(&uuid::Uuid::now_v7().to_string())).unwrap();
    insert_delivery(&pool, &malformed.id, wrong_payload, "delivered").await;

    let missing_id = record(&uuid::Uuid::now_v7().to_string());
    insert_audit(&pool, &missing_id, old).await;
    insert_delivery(
        &pool,
        &missing_id.id,
        json!({"schema_version": 1}),
        "delivered",
    )
    .await;

    let non_string_id = record(&uuid::Uuid::now_v7().to_string());
    insert_audit(&pool, &non_string_id, old).await;
    let mut non_string_payload = serde_json::to_value(&non_string_id).unwrap();
    non_string_payload["id"] = json!(123);
    insert_delivery(&pool, &non_string_id.id, non_string_payload, "delivered").await;

    let orphan_id = uuid::Uuid::now_v7().to_string();
    let orphan = record(&orphan_id);
    insert_delivery(
        &pool,
        &orphan_id,
        serde_json::to_value(orphan).unwrap(),
        "delivered",
    )
    .await;

    assert_eq!(
        repository
            .delete_audit_logs_before(cutoff.timestamp() as u64, 20)
            .await
            .unwrap(),
        0
    );
    for state in ["pending", "leased", "dead_letter"] {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM admin_audit_delivery WHERE state=$1")
                .bind(state)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
    }
    for item in &unresolved {
        assert!(exists(&pool, "audit_logs", "id", &item.id).await);
        assert!(exists(&pool, "admin_audit_delivery", "event_id", &item.id).await);
    }
    assert!(exists(&pool, "audit_logs", "id", &malformed.id).await);
    assert!(exists(&pool, "admin_audit_delivery", "event_id", &malformed.id).await);
    for item in [&missing_id, &non_string_id] {
        assert!(exists(&pool, "audit_logs", "id", &item.id).await);
        assert!(exists(&pool, "admin_audit_delivery", "event_id", &item.id).await);
    }
    assert!(exists(&pool, "admin_audit_delivery", "event_id", &orphan_id).await);

    // Invalid identities must not consume the bounded candidate page and
    // permanently hide a later eligible pair behind them.
    let eligible = record(&uuid::Uuid::now_v7().to_string());
    insert_audit(&pool, &eligible, old + chrono::Duration::milliseconds(500)).await;
    insert_delivery(
        &pool,
        &eligible.id,
        serde_json::to_value(&eligible).unwrap(),
        "delivered",
    )
    .await;
    assert_eq!(
        repository
            .delete_audit_logs_before(cutoff.timestamp() as u64, 1)
            .await
            .unwrap(),
        1
    );
    assert!(!exists(&pool, "audit_logs", "id", &eligible.id).await);
    assert!(!exists(&pool, "admin_audit_delivery", "event_id", &eligible.id).await);
    for item in [&malformed, &missing_id, &non_string_id] {
        assert!(exists(&pool, "audit_logs", "id", &item.id).await);
        assert!(exists(&pool, "admin_audit_delivery", "event_id", &item.id).await);
    }
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn audit_retention_rolls_back_canonical_and_delivery_when_delete_fails() {
    let pool = pool().await;
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    let cutoff = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let item = record(&uuid::Uuid::now_v7().to_string());
    insert_audit(&pool, &item, cutoff - chrono::Duration::seconds(1)).await;
    insert_delivery(
        &pool,
        &item.id,
        serde_json::to_value(&item).unwrap(),
        "delivered",
    )
    .await;
    sqlx::raw_sql(
        "CREATE FUNCTION reject_audit_retention_delete() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'retention-delete-fixture'; END $$; \
         CREATE TRIGGER reject_audit_retention_delete BEFORE DELETE ON audit_logs \
         FOR EACH ROW EXECUTE FUNCTION reject_audit_retention_delete();",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert!(repository
        .delete_audit_logs_before(cutoff.timestamp() as u64, 1)
        .await
        .is_err());
    assert!(exists(&pool, "audit_logs", "id", &item.id).await);
    assert!(exists(&pool, "admin_audit_delivery", "event_id", &item.id).await);
    sqlx::raw_sql(
        "DROP TRIGGER reject_audit_retention_delete ON audit_logs; \
         DROP FUNCTION reject_audit_retention_delete();",
    )
    .execute(&pool)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn audit_retention_concurrent_cleanups_are_disjoint() {
    let pool = pool().await;
    let cutoff = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let item = record(&uuid::Uuid::now_v7().to_string());
        insert_audit(&pool, &item, cutoff - chrono::Duration::seconds(1)).await;
        insert_delivery(
            &pool,
            &item.id,
            serde_json::to_value(&item).unwrap(),
            "delivered",
        )
        .await;
        ids.push(item.id);
    }
    ids.sort();
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION block_first_retention_delete() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF OLD.event_id = '{}' THEN PERFORM pg_advisory_xact_lock(255, 782); \
         END IF; RETURN OLD; END $$; \
         CREATE TRIGGER block_first_retention_delete BEFORE DELETE ON admin_audit_delivery \
         FOR EACH ROW EXECUTE FUNCTION block_first_retention_delete();",
        ids[0],
    ))
    .execute(&pool)
    .await
    .unwrap();
    let mut barrier = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(255, 782)")
        .execute(&mut *barrier)
        .await
        .unwrap();
    let first = PostgresAuditLogReadRepository::new(pool.clone());
    let second = PostgresAuditLogReadRepository::new(pool.clone());
    let first = tokio::spawn(async move {
        first
            .delete_audit_logs_before(cutoff.timestamp() as u64, 1)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity \
                 WHERE datname=current_database() AND wait_event='advisory' \
                 AND query LIKE 'DELETE FROM admin_audit_delivery%')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first cleanup must hold its pair locks before the second starts");
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(1),
            second.delete_audit_logs_before(cutoff.timestamp() as u64, 1),
        )
        .await
        .expect("second cleanup must skip the first cleanup's locked pair")
        .unwrap(),
        1
    );
    assert!(exists(&pool, "audit_logs", "id", &ids[0]).await);
    assert!(!exists(&pool, "audit_logs", "id", &ids[1]).await);
    barrier.rollback().await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), first)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM admin_audit_delivery")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::raw_sql(
        "DROP TRIGGER block_first_retention_delete ON admin_audit_delivery; \
         DROP FUNCTION block_first_retention_delete();",
    )
    .execute(&pool)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn audit_retention_skips_locked_delivered_pair_then_reclaims_it() {
    let pool = pool().await;
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    let cutoff = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let item = record(&uuid::Uuid::now_v7().to_string());
    insert_audit(&pool, &item, cutoff - chrono::Duration::seconds(1)).await;
    insert_delivery(
        &pool,
        &item.id,
        serde_json::to_value(&item).unwrap(),
        "delivered",
    )
    .await;

    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT event_id FROM admin_audit_delivery WHERE event_id=$1 FOR UPDATE")
        .bind(&item.id)
        .execute(&mut *holder)
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(1),
            repository.delete_audit_logs_before(cutoff.timestamp() as u64, 1),
        )
        .await
        .expect("cleanup must skip a locked delivered delivery row")
        .unwrap(),
        0
    );
    assert!(exists(&pool, "audit_logs", "id", &item.id).await);
    holder.rollback().await.unwrap();
    assert_eq!(
        repository
            .delete_audit_logs_before(cutoff.timestamp() as u64, 1)
            .await
            .unwrap(),
        1
    );
    assert!(!exists(&pool, "audit_logs", "id", &item.id).await);
    assert!(!exists(&pool, "admin_audit_delivery", "event_id", &item.id).await);
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn audit_retention_skips_live_delivery_lock_then_reclaims_after_delivery() {
    let pool = pool().await;
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    let cutoff = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let item = record(&uuid::Uuid::now_v7().to_string());
    insert_audit(&pool, &item, cutoff - chrono::Duration::seconds(1)).await;
    insert_delivery(
        &pool,
        &item.id,
        serde_json::to_value(&item).unwrap(),
        "pending",
    )
    .await;
    let claim = repository
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();

    sqlx::raw_sql(
        "CREATE FUNCTION block_retention_delivery_insert() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN PERFORM pg_advisory_xact_lock(255, 781); RETURN NEW; END $$; \
         CREATE TRIGGER block_retention_delivery_insert BEFORE INSERT ON audit_logs \
         FOR EACH ROW EXECUTE FUNCTION block_retention_delivery_insert();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut barrier = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(255, 781)")
        .execute(&mut *barrier)
        .await
        .unwrap();
    let delivery_pool = pool.clone();
    let event_id = item.id.clone();
    let delivery = tokio::spawn(async move {
        PostgresAuditLogReadRepository::new(delivery_pool)
            .deliver_admin_audit(&event_id, claim.lease_token)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity \
                 WHERE datname=current_database() AND wait_event='advisory' \
                 AND query LIKE '%INSERT INTO audit_logs%')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("delivery must hold its delivery row before waiting at the audit barrier");

    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(1),
            repository.delete_audit_logs_before(cutoff.timestamp() as u64, 1),
        )
        .await
        .expect("cleanup must skip the live delivery instead of waiting on its lock")
        .unwrap(),
        0
    );
    assert!(exists(&pool, "audit_logs", "id", &item.id).await);
    barrier.rollback().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), delivery)
        .await
        .unwrap()
        .unwrap()
        .unwrap());
    assert_eq!(
        repository
            .delete_audit_logs_before(cutoff.timestamp() as u64, 1)
            .await
            .unwrap(),
        1
    );
    assert!(!exists(&pool, "audit_logs", "id", &item.id).await);
    assert!(!exists(&pool, "admin_audit_delivery", "event_id", &item.id).await);
    sqlx::raw_sql(
        "DROP TRIGGER block_retention_delivery_insert ON audit_logs; \
         DROP FUNCTION block_retention_delivery_insert();",
    )
    .execute(&pool)
    .await
    .unwrap();
}
