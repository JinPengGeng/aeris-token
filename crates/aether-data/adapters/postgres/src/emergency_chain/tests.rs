use super::*;
use aether_data_contracts::repository::audit::CreateAdminAuditLog;
use chrono::Utc;
use sqlx::PgPool;

fn audit(id: String, principal: &str, request_id: &str) -> CreateAdminAuditLog {
    CreateAdminAuditLog {
        id,
        event_type: "admin_mutation".to_string(),
        user_id: Some(principal.to_string()),
        api_key_id: None,
        description: "admin action: emergency_chain_grant".to_string(),
        ip_address: None,
        user_agent: None,
        request_id: Some(request_id.to_string()),
        event_metadata: None,
        status_code: Some(200),
        error_message: None,
        created_at: Utc::now(),
    }
}

fn grant(grant_id: String, principal: String, request_id: String) -> StoredEmergencyChainGrant {
    let targets = vec![EmergencyChainTarget {
        provider_id: "provider-a".to_string(),
        endpoint_id: "endpoint-a".to_string(),
        key_id: "key-a".to_string(),
    }];
    StoredEmergencyChainGrant {
        grant_id,
        principal,
        operations: vec!["responses.create".to_string()],
        request_id,
        request_fingerprint: "a".repeat(64),
        session_nonce: "nonce-a".to_string(),
        chain_hash: emergency_chain_target_hash(&targets),
        targets,
        issued_at_unix_secs: 1_000,
        expires_at_unix_secs: 1_300,
        revoked_at_unix_secs: None,
        consumed_at_unix_secs: None,
    }
}

async fn count(pool: &PgPool, table: &str, value: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table} WHERE grant_id = $1"))
        .bind(value)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn issue_test_grant(
    repository: &PostgresEmergencyChainGrantRepository,
    principal: &str,
    grant_id: &str,
) {
    let request_id = format!("issue-{grant_id}");
    assert_eq!(
        repository
            .issue_emergency_chain_grant(IssueEmergencyChainGrant {
                grant: grant(
                    grant_id.to_string(),
                    principal.to_string(),
                    request_id.clone(),
                ),
                audit: audit(uuid::Uuid::new_v4().to_string(), principal, &request_id),
            })
            .await
            .unwrap(),
        IssueEmergencyChainGrantOutcome::Issued
    );
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_DATABASE_URL"]
async fn live_emergency_chain_consume_is_single_use_and_checks_time_and_revocation() {
    let url = std::env::var("AETHER_TEST_DATABASE_URL").unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let repository = PostgresEmergencyChainGrantRepository::new(pool.clone());
    let principal = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO users(id,username,email_verified) VALUES($1,$2,false)")
        .bind(&principal)
        .bind(format!("ec-consume-{}", &principal[..8]))
        .execute(&pool)
        .await
        .unwrap();

    let future_id = uuid::Uuid::new_v4().to_string();
    issue_test_grant(&repository, &principal, &future_id).await;
    assert_eq!(
        repository
            .consume_emergency_chain_grant(ConsumeEmergencyChainGrant {
                grant_id: future_id,
                principal: principal.clone(),
                consumed_at_unix_secs: 999,
            })
            .await
            .unwrap(),
        ConsumeEmergencyChainGrantOutcome::NotYetValid
    );

    let expired_id = uuid::Uuid::new_v4().to_string();
    issue_test_grant(&repository, &principal, &expired_id).await;
    assert_eq!(
        repository
            .consume_emergency_chain_grant(ConsumeEmergencyChainGrant {
                grant_id: expired_id,
                principal: principal.clone(),
                consumed_at_unix_secs: 1_300,
            })
            .await
            .unwrap(),
        ConsumeEmergencyChainGrantOutcome::Expired
    );

    let revoked_id = uuid::Uuid::new_v4().to_string();
    issue_test_grant(&repository, &principal, &revoked_id).await;
    repository
        .revoke_emergency_chain_grant(RevokeEmergencyChainGrant {
            grant_id: revoked_id.clone(),
            principal: principal.clone(),
            revoked_at_unix_secs: 1_050,
            audit: audit(
                uuid::Uuid::new_v4().to_string(),
                &principal,
                "revoke-before-consume",
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        repository
            .consume_emergency_chain_grant(ConsumeEmergencyChainGrant {
                grant_id: revoked_id,
                principal: principal.clone(),
                consumed_at_unix_secs: 1_100,
            })
            .await
            .unwrap(),
        ConsumeEmergencyChainGrantOutcome::Revoked
    );

    let repeated_id = uuid::Uuid::new_v4().to_string();
    issue_test_grant(&repository, &principal, &repeated_id).await;
    let consume = ConsumeEmergencyChainGrant {
        grant_id: repeated_id,
        principal: principal.clone(),
        consumed_at_unix_secs: 1_100,
    };
    assert_eq!(
        repository
            .consume_emergency_chain_grant(consume.clone())
            .await
            .unwrap(),
        ConsumeEmergencyChainGrantOutcome::Consumed
    );
    assert_eq!(
        repository
            .consume_emergency_chain_grant(consume)
            .await
            .unwrap(),
        ConsumeEmergencyChainGrantOutcome::AlreadyConsumed {
            effective_at_unix_secs: 1_100,
        }
    );

    let concurrent_id = uuid::Uuid::new_v4().to_string();
    issue_test_grant(&repository, &principal, &concurrent_id).await;
    let record = ConsumeEmergencyChainGrant {
        grant_id: concurrent_id,
        principal,
        consumed_at_unix_secs: 1_100,
    };
    let first_repository = repository.clone();
    let second_repository = repository.clone();
    let (first, second) = tokio::join!(
        first_repository.consume_emergency_chain_grant(record.clone()),
        second_repository.consume_emergency_chain_grant(record),
    );
    let outcomes = [first.unwrap(), second.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == ConsumeEmergencyChainGrantOutcome::Consumed)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(
                outcome,
                ConsumeEmergencyChainGrantOutcome::AlreadyConsumed { .. }
            ))
            .count(),
        1
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires a fresh migrated AETHER_TEST_DATABASE_URL"]
async fn live_emergency_chain_grants_commit_audit_with_issue_and_revoke_or_roll_back_together() {
    let url = std::env::var("AETHER_TEST_DATABASE_URL").unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let repository = PostgresEmergencyChainGrantRepository::new(pool.clone());
    let principal = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO users(id,username,email_verified) VALUES($1,$2,false)")
        .bind(&principal)
        .bind(format!("ec-{}", &principal[..8]))
        .execute(&pool)
        .await
        .unwrap();

    let issue_grant_id = uuid::Uuid::new_v4().to_string();
    let issue_request_id = format!("issue-{issue_grant_id}");
    let issue_audit_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        repository
            .issue_emergency_chain_grant(IssueEmergencyChainGrant {
                grant: grant(
                    issue_grant_id.clone(),
                    principal.clone(),
                    issue_request_id.clone(),
                ),
                audit: audit(issue_audit_id.clone(), &principal, &issue_request_id),
            })
            .await
            .unwrap(),
        IssueEmergencyChainGrantOutcome::Issued
    );
    assert_eq!(
        count(&pool, "emergency_chain_grants", &issue_grant_id).await,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id = $1")
            .bind(&issue_audit_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    let stored = repository
        .read_emergency_chain_grant(&issue_grant_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.principal, principal);
    assert_eq!(stored.targets.len(), 1);
    assert_eq!(stored.targets[0].endpoint_id, "endpoint-a");
    let revoke_audit_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        repository
            .revoke_emergency_chain_grant(RevokeEmergencyChainGrant {
                grant_id: issue_grant_id.clone(),
                principal: principal.clone(),
                revoked_at_unix_secs: 1_200,
                audit: audit(revoke_audit_id.clone(), &principal, "revoke-request"),
            })
            .await
            .unwrap(),
        RevokeEmergencyChainGrantOutcome::Revoked {
            effective_at_unix_secs: 1_200
        }
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT revoked_at_unix_secs FROM emergency_chain_grants WHERE grant_id = $1",
        )
        .bind(&issue_grant_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        Some(1_200)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id = $1")
            .bind(&revoke_audit_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );

    let failed_grant_id = uuid::Uuid::new_v4().to_string();
    let failed_request_id = format!("issue-{failed_grant_id}");
    let failed_audit_id = uuid::Uuid::new_v4().to_string();
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION reject_emergency_chain_audit() RETURNS trigger LANGUAGE plpgsql AS $$\
         BEGIN IF NEW.id = '{failed_audit_id}' THEN RAISE EXCEPTION 'emergency audit rejected'; END IF; RETURN NEW; END $$;\
         CREATE TRIGGER reject_emergency_chain_audit BEFORE INSERT ON audit_logs \
         FOR EACH ROW EXECUTE FUNCTION reject_emergency_chain_audit();"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(repository
        .issue_emergency_chain_grant(IssueEmergencyChainGrant {
            grant: grant(
                failed_grant_id.clone(),
                principal.clone(),
                failed_request_id.clone(),
            ),
            audit: audit(failed_audit_id.clone(), &principal, &failed_request_id),
        })
        .await
        .is_err());
    assert_eq!(
        count(&pool, "emergency_chain_grants", &failed_grant_id).await,
        0
    );
    assert_eq!(
        count(&pool, "emergency_chain_grant_targets", &failed_grant_id).await,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id = $1")
            .bind(&failed_audit_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );

    sqlx::raw_sql(
        "DROP TRIGGER reject_emergency_chain_audit ON audit_logs;\
         DROP FUNCTION reject_emergency_chain_audit();",
    )
    .execute(&pool)
    .await
    .unwrap();

    let revoke_failure_grant_id = uuid::Uuid::new_v4().to_string();
    let revoke_failure_request_id = format!("issue-{revoke_failure_grant_id}");
    let revoke_failure_issue_audit_id = uuid::Uuid::new_v4().to_string();
    repository
        .issue_emergency_chain_grant(IssueEmergencyChainGrant {
            grant: grant(
                revoke_failure_grant_id.clone(),
                principal.clone(),
                revoke_failure_request_id.clone(),
            ),
            audit: audit(
                revoke_failure_issue_audit_id,
                &principal,
                &revoke_failure_request_id,
            ),
        })
        .await
        .unwrap();
    let revoke_failure_audit_id = uuid::Uuid::new_v4().to_string();
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION reject_emergency_chain_revoke_audit() RETURNS trigger LANGUAGE plpgsql AS $$\
         BEGIN IF NEW.id = '{revoke_failure_audit_id}' THEN RAISE EXCEPTION 'emergency revoke audit rejected'; END IF; RETURN NEW; END $$;\
         CREATE TRIGGER reject_emergency_chain_revoke_audit BEFORE INSERT ON audit_logs \
         FOR EACH ROW EXECUTE FUNCTION reject_emergency_chain_revoke_audit();"
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert!(repository
        .revoke_emergency_chain_grant(RevokeEmergencyChainGrant {
            grant_id: revoke_failure_grant_id.clone(),
            principal: principal.clone(),
            revoked_at_unix_secs: 1_200,
            audit: audit(
                revoke_failure_audit_id.clone(),
                &principal,
                "revoke-failure-request",
            ),
        })
        .await
        .is_err());
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT revoked_at_unix_secs FROM emergency_chain_grants WHERE grant_id = $1",
        )
        .bind(&revoke_failure_grant_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id = $1")
            .bind(&revoke_failure_audit_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::raw_sql(
        "DROP TRIGGER reject_emergency_chain_revoke_audit ON audit_logs;\
         DROP FUNCTION reject_emergency_chain_revoke_audit();",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
}
