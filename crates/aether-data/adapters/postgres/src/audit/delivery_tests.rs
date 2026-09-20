use super::*;
use chrono::Utc;
use serde_json::json;

fn record(id: &str) -> CreateAdminAuditLog {
    CreateAdminAuditLog {
        id: id.to_string(),
        event_type: "admin_mutation".to_string(),
        user_id: None,
        api_key_id: None,
        description: "admin action: update_system_config".to_string(),
        ip_address: None,
        user_agent: None,
        request_id: Some("delivery-test".to_string()),
        event_metadata: Some(json!({
            "schema_version": 1,
            "event_name": "admin_system_config_updated",
            "status": "completed",
            "method": "PUT",
            "path": "/api/admin/system/configs/[key]",
            "action": "update_system_config",
            "target_type": "system_config",
            "target_id": "delivery.test"
        })),
        status_code: Some(200),
        error_message: None,
        created_at: Utc::now(),
    }
}

async fn enqueue(pool: &PostgresPool, record: &CreateAdminAuditLog) {
    sqlx::query("INSERT INTO admin_audit_delivery(event_id,payload) VALUES($1,$2)")
        .bind(&record.id)
        .bind(serde_json::to_value(record).unwrap())
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn operator_redrive_is_single_winner_payload_preserving_and_fenced() {
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PostgresPool::connect(&url).await.unwrap();
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    let record = record(&uuid::Uuid::now_v7().to_string());
    let business_before: serde_json::Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(to_jsonb(c) ORDER BY key),'[]'::jsonb) FROM system_configs c",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    enqueue(&pool, &record).await;
    let claimed = repository
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let lease_before: (uuid::Uuid, chrono::DateTime<Utc>) = sqlx::query_as(
        "SELECT lease_token,lease_expires_at FROM admin_audit_delivery WHERE event_id=$1",
    )
    .bind(&record.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        repository
            .redrive_admin_audit_delivery(&record.id)
            .await
            .unwrap(),
        AdminAuditDeliveryRedriveOutcome::NotDeadLetter
    );
    let lease_after: (uuid::Uuid, chrono::DateTime<Utc>) = sqlx::query_as(
        "SELECT lease_token,lease_expires_at FROM admin_audit_delivery WHERE event_id=$1",
    )
    .bind(&record.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(lease_after, lease_before);
    sqlx::query("UPDATE admin_audit_delivery SET state='dead_letter',attempt_count=12,dead_lettered_at=clock_timestamp(),lease_token=NULL,lease_expires_at=NULL,last_error_code='invalid_payload' WHERE event_id=$1")
        .bind(&record.id).execute(&pool).await.unwrap();
    let payload_before: serde_json::Value =
        sqlx::query_scalar("SELECT payload FROM admin_audit_delivery WHERE event_id=$1")
            .bind(&record.id)
            .fetch_one(&pool)
            .await
            .unwrap();

    let (a, b) = tokio::join!(
        repository.redrive_admin_audit_delivery(&record.id),
        repository.redrive_admin_audit_delivery(&record.id),
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|v| **v == AdminAuditDeliveryRedriveOutcome::Redriven)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|v| **v == AdminAuditDeliveryRedriveOutcome::NotDeadLetter)
            .count(),
        1
    );
    let (payload_after, state, attempts): (serde_json::Value, String, i32) = sqlx::query_as(
        "SELECT payload,state,attempt_count FROM admin_audit_delivery WHERE event_id=$1",
    )
    .bind(&record.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(payload_after, payload_before);
    assert_eq!((state.as_str(), attempts), ("pending", 0));
    assert!(!repository
        .deliver_admin_audit(&record.id, claimed.lease_token)
        .await
        .unwrap());
    let delivered = repository
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(repository
        .deliver_admin_audit(&record.id, delivered.lease_token)
        .await
        .unwrap());
    let canonical: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
        .bind(&record.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(canonical, 1);
    let business_after: serde_json::Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_agg(to_jsonb(c) ORDER BY key),'[]'::jsonb) FROM system_configs c",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(business_after, business_before);

    let page = repository
        .list_admin_audit_deliveries(&AdminAuditDeliveryListQuery {
            state: Some(AdminAuditDeliveryState::Delivered),
            limit: 1,
            before_created_at: None,
            before_event_id: None,
        })
        .await
        .unwrap();
    assert_eq!(page.items[0].event_id, record.id);
    assert_eq!(
        repository
            .redrive_admin_audit_delivery("missing")
            .await
            .unwrap(),
        AdminAuditDeliveryRedriveOutcome::NotFound
    );
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn delivery_keyset_is_stable_for_ties_and_concurrent_new_rows() {
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PostgresPool::connect(&url).await.unwrap();
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    let tied_at = Utc::now() + chrono::Duration::days(3_650);
    let mut ids = Vec::new();
    for _ in 0..3 {
        let item = record(&uuid::Uuid::now_v7().to_string());
        enqueue(&pool, &item).await;
        sqlx::query("UPDATE admin_audit_delivery SET created_at=$2 WHERE event_id=$1")
            .bind(&item.id)
            .bind(tied_at)
            .execute(&pool)
            .await
            .unwrap();
        ids.push(item.id);
    }
    ids.sort_by(|a, b| b.cmp(a));
    let first = repository
        .list_admin_audit_deliveries(&AdminAuditDeliveryListQuery {
            state: Some(AdminAuditDeliveryState::Pending),
            limit: 2,
            before_created_at: None,
            before_event_id: None,
        })
        .await
        .unwrap();
    assert!(first.has_more);
    let newer = record(&uuid::Uuid::now_v7().to_string());
    enqueue(&pool, &newer).await;
    sqlx::query("UPDATE admin_audit_delivery SET created_at=$2 WHERE event_id=$1")
        .bind(&newer.id)
        .bind(tied_at + chrono::Duration::hours(1))
        .execute(&pool)
        .await
        .unwrap();
    let cursor = first.items.last().unwrap();
    let second = repository
        .list_admin_audit_deliveries(&AdminAuditDeliveryListQuery {
            state: Some(AdminAuditDeliveryState::Pending),
            limit: 2,
            before_created_at: Some(cursor.created_at),
            before_event_id: Some(cursor.event_id.clone()),
        })
        .await
        .unwrap();
    let seen = first
        .items
        .iter()
        .chain(&second.items)
        .map(|v| v.event_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(seen, ids);
    assert!(!seen.contains(&newer.id));
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn durable_delivery_retries_fences_and_converges_to_one_row() {
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PostgresPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("aether_admin_audit_"));
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    let record = record(&uuid::Uuid::now_v7().to_string());
    enqueue(&pool, &record).await;

    let first = repository
        .claim_admin_audit_deliveries(1, 1)
        .await
        .unwrap()
        .pop()
        .unwrap();
    sqlx::query(
        "UPDATE admin_audit_delivery
         SET lease_expires_at=clock_timestamp()-interval '1 second'
         WHERE event_id=$1",
    )
    .bind(&record.id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!repository
        .deliver_admin_audit(&record.id, first.lease_token)
        .await
        .unwrap());
    assert_eq!(
        repository
            .fail_admin_audit_delivery(
                &record.id,
                first.lease_token,
                AdminAuditDeliveryFailureCode::AuditInsertFailed,
            )
            .await
            .unwrap(),
        AdminAuditDeliveryFailureOutcome::StaleLease
    );
    let second = repository
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_ne!(first.lease_token, second.lease_token);
    let remaining: f64 = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM lease_expires_at-clock_timestamp())::double precision
         FROM admin_audit_delivery WHERE event_id=$1",
    )
    .bind(&record.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        (25.0..=30.0).contains(&remaining),
        "lease must last actual seconds"
    );
    assert!(!repository
        .deliver_admin_audit(&record.id, first.lease_token)
        .await
        .unwrap());

    sqlx::raw_sql(
        "CREATE FUNCTION reject_durable_audit_test() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'private-test-error'; END $$;
         CREATE TRIGGER reject_durable_audit_test BEFORE INSERT ON audit_logs
         FOR EACH ROW EXECUTE FUNCTION reject_durable_audit_test();",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(repository
        .deliver_admin_audit(&record.id, second.lease_token)
        .await
        .is_err());
    assert_eq!(
        repository
            .fail_admin_audit_delivery(
                &record.id,
                second.lease_token,
                AdminAuditDeliveryFailureCode::AuditInsertFailed,
            )
            .await
            .unwrap(),
        AdminAuditDeliveryFailureOutcome::RetryScheduled
    );
    sqlx::raw_sql(
        "DROP TRIGGER reject_durable_audit_test ON audit_logs;
         DROP FUNCTION reject_durable_audit_test();
         UPDATE admin_audit_delivery SET next_attempt_at=clock_timestamp()
         WHERE event_id IN (SELECT event_id FROM admin_audit_delivery);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let retry = repository
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(repository
        .deliver_admin_audit(&record.id, retry.lease_token)
        .await
        .unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(&record.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM admin_audit_delivery WHERE event_id=$1")
            .bind(&record.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "delivered"
    );
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn expired_lease_after_blocked_insert_rolls_back_audit_and_ack() {
    use std::time::Duration;
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PostgresPool::connect(&url).await.unwrap();
    let record = record(&uuid::Uuid::now_v7().to_string());
    enqueue(&pool, &record).await;
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    let claimed = repository
        .claim_admin_audit_deliveries(1, 1)
        .await
        .unwrap()
        .pop()
        .unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION block_expiring_audit_test() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(255,392); RETURN NEW; END $$;
         CREATE TRIGGER block_expiring_audit_test BEFORE INSERT ON audit_logs
         FOR EACH ROW EXECUTE FUNCTION block_expiring_audit_test();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut barrier = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(255,392)")
        .execute(&mut *barrier)
        .await
        .unwrap();
    let worker_pool = pool.clone();
    let worker_event = claimed.event_id.clone();
    let task = tokio::spawn(async move {
        PostgresAuditLogReadRepository::new(worker_pool)
            .deliver_admin_audit(&worker_event, claimed.lease_token)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                 WHERE datname=current_database() AND wait_event='advisory'
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
    .expect("delivery must reach the real audit INSERT barrier");
    // Wait against the actual database expiry while the INSERT owns the row
    // lock; a claimant cannot steal that lock or fabricate this boundary.
    sqlx::query(
        "SELECT pg_sleep(GREATEST(0, EXTRACT(EPOCH FROM lease_expires_at-clock_timestamp()))::double precision + 0.05)
         FROM admin_audit_delivery WHERE event_id=$1",
    ).bind(&record.id).execute(&pool).await.unwrap();
    barrier.rollback().await.unwrap();
    assert!(!tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(&record.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM admin_audit_delivery WHERE event_id=$1")
            .bind(&record.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "leased"
    );
    let reclaimed = repository
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_ne!(reclaimed.lease_token, claimed.lease_token);
    assert!(repository
        .deliver_admin_audit(&record.id, reclaimed.lease_token)
        .await
        .unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(&record.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn claim_bounds_and_strict_row_decode_fail_closed() {
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PostgresPool::connect(&url).await.unwrap();
    let repository = PostgresAuditLogReadRepository::new(pool);
    assert!(repository
        .claim_admin_audit_deliveries(0, 30)
        .await
        .is_err());
    assert!(repository
        .claim_admin_audit_deliveries(ADMIN_AUDIT_DELIVERY_MAX_CLAIM + 1, 30)
        .await
        .is_err());
    assert!(repository
        .claim_admin_audit_deliveries(1, ADMIN_AUDIT_DELIVERY_MAX_LEASE_SECONDS + 1)
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn concurrent_claims_are_disjoint_and_failures_dead_letter_at_the_bound() {
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PostgresPool::connect(&url).await.unwrap();
    let first_record = record(&uuid::Uuid::now_v7().to_string());
    let second_record = record(&uuid::Uuid::now_v7().to_string());
    enqueue(&pool, &first_record).await;
    enqueue(&pool, &second_record).await;
    let first_repository = PostgresAuditLogReadRepository::new(pool.clone());
    let second_repository = PostgresAuditLogReadRepository::new(pool.clone());
    let (first, second) = tokio::join!(
        first_repository.claim_admin_audit_deliveries(1, 30),
        second_repository.claim_admin_audit_deliveries(1, 30)
    );
    let first = first.unwrap().pop().unwrap();
    let second = second.unwrap().pop().unwrap();
    assert_ne!(first.event_id, second.event_id);

    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    assert!(repository
        .deliver_admin_audit(&second.event_id, second.lease_token)
        .await
        .unwrap());
    sqlx::query("UPDATE admin_audit_delivery SET payload='{}'::jsonb WHERE event_id=$1")
        .bind(&first.event_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut claimed = first;
    for attempt in 1..=ADMIN_AUDIT_DELIVERY_MAX_ATTEMPTS {
        assert!(matches!(
            repository
                .deliver_admin_audit(&claimed.event_id, claimed.lease_token)
                .await,
            Err(DataLayerError::InvalidInput(_))
        ));
        let outcome = repository
            .fail_admin_audit_delivery(
                &claimed.event_id,
                claimed.lease_token,
                AdminAuditDeliveryFailureCode::InvalidPayload,
            )
            .await
            .unwrap();
        if attempt == ADMIN_AUDIT_DELIVERY_MAX_ATTEMPTS {
            assert_eq!(outcome, AdminAuditDeliveryFailureOutcome::DeadLettered);
            break;
        }
        assert_eq!(outcome, AdminAuditDeliveryFailureOutcome::RetryScheduled);
        sqlx::query(
            "UPDATE admin_audit_delivery SET next_attempt_at=clock_timestamp()
             WHERE event_id=$1",
        )
        .bind(&claimed.event_id)
        .execute(&pool)
        .await
        .unwrap();
        claimed = repository
            .claim_admin_audit_deliveries(1, 30)
            .await
            .unwrap()
            .into_iter()
            .find(|item| item.event_id == claimed.event_id)
            .unwrap();
    }
    let (state, attempts, code): (String, i32, Option<String>) = sqlx::query_as(
        "SELECT state,attempt_count,last_error_code
         FROM admin_audit_delivery WHERE event_id=$1",
    )
    .bind(&claimed.event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "dead_letter");
    assert_eq!(attempts, ADMIN_AUDIT_DELIVERY_MAX_ATTEMPTS);
    assert_eq!(code.as_deref(), Some("invalid_payload"));
}
