//! Each ignored test requires its own fresh, migrated, task-owned audit database.
use super::*;
use aether_data_contracts::repository::audit::CreateAdminAuditLog;
use serde_json::{json, Value};
use std::time::Duration;

const GROUP: &str = "group-concurrency-target";

async fn fixture() -> (String, PgPool) {
    let url = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL").unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("aether_admin_audit_"));
    let intents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_audit_delivery")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(intents, 0, "use a separate fresh database per ignored test");
    sqlx::raw_sql(
        "INSERT INTO users (id,username,email_verified,is_active,is_deleted,role)
         VALUES ('group-a1','group_a1',true,true,false,'user'),
                ('group-a2','group_a2',true,true,false,'user'),
                ('group-b1','group_b1',true,true,false,'user'),
                ('group-b2','group_b2',true,true,false,'user'),
                ('group-late','group_late',true,true,false,'user');
         INSERT INTO user_groups (id,name,normalized_name)
         VALUES ('group-concurrency-target','Group concurrency','group concurrency');",
    )
    .execute(&pool)
    .await
    .unwrap();
    (url, pool)
}

async fn worker_pool(url: &str, name: &str) -> PgPool {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('application_name', $1, false)")
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
    pool
}

fn intent() -> CreateAdminAuditLog {
    let id = uuid::Uuid::now_v7().to_string();
    CreateAdminAuditLog {
        id: id.clone(),
        event_type: "admin_mutation".to_string(),
        user_id: None,
        api_key_id: None,
        description: "admin action: update_user_group_members".to_string(),
        ip_address: None,
        user_agent: None,
        request_id: Some(id),
        event_metadata: Some(json!({
            "schema_version": 1, "event_name": "admin_user_group_members_updated",
            "status": "completed", "method": "PUT", "target_type": "user_group",
            "target_id": GROUP
        })),
        status_code: Some(200),
        error_message: None,
        created_at: chrono::Utc::now(),
    }
}

// Observe actual PostgreSQL lock waits, not an assumed scheduler interleaving.
async fn wait_for_query_locks(pool: &PgPool, applications: &[&str], query: &str, expected: i64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname=current_database() AND application_name=ANY($1::text[])
                   AND wait_event_type='Lock' AND query LIKE $2",
            )
            .bind(applications)
            .bind(format!("%{query}%"))
            .fetch_one(pool)
            .await
            .unwrap();
            if count >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("workers must reach the actual PostgreSQL lock barrier");
}

async fn stored_members(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT user_id FROM user_group_members WHERE group_id=$1 ORDER BY user_id")
        .bind(GROUP)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn concurrent_empty_group_replaces_commit_complete_sets_and_relock_late_members() {
    let (url, pool) = fixture().await;
    assert!(stored_members(&pool).await.is_empty());
    sqlx::raw_sql(
        "CREATE TABLE group_replace_commit_probe (
            ordinal bigserial PRIMARY KEY, event_id text NOT NULL, members text[] NOT NULL);
         CREATE FUNCTION group_replace_commit_probe() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           PERFORM pg_advisory_xact_lock(794253,1);
           INSERT INTO group_replace_commit_probe (event_id,members)
           VALUES (NEW.event_id, ARRAY(SELECT user_id FROM user_group_members
                   WHERE group_id='group-concurrency-target' ORDER BY user_id));
           RETURN NEW;
         END $$;
         CREATE TRIGGER group_replace_commit_probe BEFORE INSERT ON admin_audit_delivery
         FOR EACH ROW EXECUTE FUNCTION group_replace_commit_probe();",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Both requests acquire their disjoint users while this gate prevents either
    // from changing the empty group's membership. KEY SHARE remains compatible,
    // allowing a late member to commit before either replacer gets the group.
    let mut group_gate = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM user_groups WHERE id=$1 FOR NO KEY UPDATE")
        .bind(GROUP)
        .execute(&mut *group_gate)
        .await
        .unwrap();
    let mut audit_gate = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(794253,1)")
        .execute(&mut *audit_gate)
        .await
        .unwrap();
    let pool_a = worker_pool(&url, "group-replace-a").await;
    let pool_b = worker_pool(&url, "group-replace-b").await;
    let ids_a = vec!["group-a1".to_string(), "group-a2".to_string()];
    let ids_b = vec!["group-b1".to_string(), "group-b2".to_string()];
    let audit_a = intent();
    let audit_b = intent();
    let task_a = {
        let pool = pool_a.clone();
        let ids = ids_a.clone();
        let audit = audit_a.clone();
        tokio::spawn(async move {
            SqlxUserReadRepository::new(pool)
                .replace_user_group_members_with_audit(GROUP, &ids, &audit)
                .await
        })
    };
    let task_b = {
        let pool = pool_b.clone();
        let ids = ids_b.clone();
        let audit = audit_b.clone();
        tokio::spawn(async move {
            SqlxUserReadRepository::new(pool)
                .replace_user_group_members_with_audit(GROUP, &ids, &audit)
                .await
        })
    };
    wait_for_query_locks(
        &pool,
        &["group-replace-a", "group-replace-b"],
        "SELECT id FROM user_groups WHERE id = $1 FOR UPDATE",
        2,
    )
    .await;
    sqlx::query("INSERT INTO user_group_members (group_id,user_id) VALUES ($1,'group-late')")
        .bind(GROUP)
        .execute(&mut *group_gate)
        .await
        .unwrap();
    group_gate.commit().await.unwrap();

    // The successful replacement must have retried with group-late included in
    // its user locks. Pausing at audit enqueue keeps those locks observable.
    wait_for_query_locks(
        &pool,
        &["group-replace-a", "group-replace-b"],
        "INSERT INTO admin_audit_delivery",
        1,
    )
    .await;
    let mut probe = pool.begin().await.unwrap();
    let error = sqlx::query("SELECT id FROM users WHERE id='group-late' FOR UPDATE NOWAIT")
        .execute(&mut *probe)
        .await
        .expect_err("late member must be locked before deletion");
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("55P03")
    );
    probe.rollback().await.unwrap();
    audit_gate.commit().await.unwrap();
    let (returned_a, returned_b) = tokio::time::timeout(Duration::from_secs(10), async {
        let a = task_a.await.unwrap().unwrap().unwrap();
        let b = task_b.await.unwrap().unwrap().unwrap();
        (a, b)
    })
    .await
    .expect("both replaces finish without a deadlock");
    assert_eq!(
        returned_a
            .iter()
            .map(|member| member.user_id.clone())
            .collect::<Vec<_>>(),
        ids_a
    );
    assert_eq!(
        returned_b
            .iter()
            .map(|member| member.user_id.clone())
            .collect::<Vec<_>>(),
        ids_b
    );
    let probes: Vec<(String, Vec<String>)> =
        sqlx::query_as("SELECT event_id,members FROM group_replace_commit_probe ORDER BY ordinal")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(probes.len(), 2, "retries must not write any success intent");
    for (event_id, members) in &probes {
        let (audit, requested) = if event_id == &audit_a.id {
            (&audit_a, &ids_a)
        } else {
            assert_eq!(event_id, &audit_b.id);
            (&audit_b, &ids_b)
        };
        assert_eq!(
            members, requested,
            "each intent observes its own complete replacement"
        );
        let payload: Value =
            sqlx::query_scalar("SELECT payload FROM admin_audit_delivery WHERE event_id=$1")
                .bind(event_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(payload, serde_json::to_value(audit).unwrap());
    }
    assert_eq!(
        stored_members(&pool).await,
        probes.last().unwrap().1,
        "final members are the last complete request, never a union"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_audit_delivery")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    pool_a.close().await;
    pool_b.close().await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn deleted_group_rejects_empty_replace_without_phantom_success_intent() {
    let (url, pool) = fixture().await;
    let repository = SqlxUserReadRepository::new(pool.clone());
    assert!(repository
        .find_user_group_by_id(GROUP)
        .await
        .unwrap()
        .is_some());
    let mut deletion = pool.begin().await.unwrap();
    sqlx::query("DELETE FROM user_groups WHERE id=$1")
        .bind(GROUP)
        .execute(&mut *deletion)
        .await
        .unwrap();
    let worker = worker_pool(&url, "group-replace-deleted").await;
    let audit = intent();
    let task = {
        let worker = worker.clone();
        let audit = audit.clone();
        tokio::spawn(async move {
            SqlxUserReadRepository::new(worker)
                .replace_user_group_members_with_audit(GROUP, &[], &audit)
                .await
        })
    };
    wait_for_query_locks(
        &pool,
        &["group-replace-deleted"],
        "SELECT id FROM user_groups WHERE id = $1 FOR UPDATE",
        1,
    )
    .await;
    deletion.commit().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("delete/replacement must settle")
        .unwrap();
    assert!(
        matches!(result, Err(DataLayerError::InvalidInput(ref message))
        if message == "user group does not exist")
    );
    assert!(matches!(
        repository.replace_user_group_members(GROUP, &[]).await,
        Err(DataLayerError::InvalidInput(_))
    ));
    assert!(repository
        .find_user_group_by_id(GROUP)
        .await
        .unwrap()
        .is_none());
    assert!(stored_members(&pool).await.is_empty());
    let intents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_audit_delivery")
        .fetch_one(&pool)
        .await
        .unwrap();
    let audits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
        .bind(&audit.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(intents, 0);
    assert_eq!(audits, 0);
    worker.close().await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn group_replace_allows_later_per_user_cas_without_a_lock_cycle() {
    let (url, pool) = fixture().await;
    sqlx::raw_sql(
        "CREATE FUNCTION group_cas_audit_gate() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_advisory_xact_lock(794253,2); RETURN NEW; END $$;
         CREATE TRIGGER group_cas_audit_gate BEFORE INSERT ON admin_audit_delivery
         FOR EACH ROW EXECUTE FUNCTION group_cas_audit_gate();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut gate = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(794253,2)")
        .execute(&mut *gate)
        .await
        .unwrap();
    let replacer = worker_pool(&url, "group-replace-before-cas").await;
    let cas_pool = worker_pool(&url, "group-user-cas").await;
    let ids = vec!["group-a1".to_string()];
    let audit = intent();
    let replacement = {
        let pool = replacer.clone();
        let ids = ids.clone();
        let audit = audit.clone();
        tokio::spawn(async move {
            SqlxUserReadRepository::new(pool)
                .replace_user_group_members_with_audit(GROUP, &ids, &audit)
                .await
        })
    };
    wait_for_query_locks(
        &pool,
        &["group-replace-before-cas"],
        "INSERT INTO admin_audit_delivery",
        1,
    )
    .await;
    let cas = {
        let pool = cas_pool.clone();
        tokio::spawn(async move {
            SqlxUserReadRepository::new(pool)
                .restore_user_groups_if_matches("group-b1", &[], &[GROUP.to_string()])
                .await
        })
    };
    wait_for_query_locks(
        &pool,
        &["group-user-cas"],
        "INSERT INTO user_group_members",
        1,
    )
    .await;
    gate.commit().await.unwrap();
    let (returned, restored) = tokio::time::timeout(Duration::from_secs(10), async {
        (
            replacement.await.unwrap().unwrap().unwrap(),
            cas.await.unwrap().unwrap(),
        )
    })
    .await
    .expect("per-user FK wait must not create a group/user lock cycle");
    assert!(restored);
    assert_eq!(
        returned
            .iter()
            .map(|member| member.user_id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(
        stored_members(&pool).await,
        vec!["group-a1", "group-b1"],
        "a distinct later user CAS is allowed to modify the committed replacement"
    );
    let payloads: Vec<Value> = sqlx::query_scalar("SELECT payload FROM admin_audit_delivery")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(payloads, vec![serde_json::to_value(&audit).unwrap()]);
    replacer.close().await;
    cas_pool.close().await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_AUDIT_DATABASE_URL"]
async fn group_replace_contention_exhaustion_leaves_members_and_intents_unchanged() {
    let (_, pool) = fixture().await;
    sqlx::query("INSERT INTO user_group_members (group_id,user_id,created_at) VALUES ($1,'group-b1','2020-01-01T00:00:00Z')")
        .bind(GROUP).execute(&pool).await.unwrap();
    let before: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM user_group_members m ORDER BY group_id,user_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let mut user_holder = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id='group-a2' FOR UPDATE")
        .execute(&mut *user_holder)
        .await
        .unwrap();
    let repository = SqlxUserReadRepository::new(pool.clone());
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        repository.replace_user_group_members_with_audit(
            GROUP,
            &["group-a1".to_string(), "group-a2".to_string()],
            &intent(),
        ),
    )
    .await
    .expect("user-lock contention retries must be bounded");
    assert!(matches!(result, Err(DataLayerError::TimedOut(_))));
    let after: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM user_group_members m ORDER BY group_id,user_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        after, before,
        "failed retries preserve exact rows and timestamps"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_audit_delivery")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    // No partial earlier user lock survives a failed attempt.
    sqlx::query("SELECT id FROM users WHERE id='group-a1' FOR UPDATE NOWAIT")
        .execute(&mut *user_holder)
        .await
        .unwrap();
    user_holder.rollback().await.unwrap();
    pool.close().await;
}
