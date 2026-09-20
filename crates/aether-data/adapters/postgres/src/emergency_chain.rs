use crate::error::SqlxResultExt;
use crate::{PostgresTransaction, PostgresTransactionRunner};
use aether_data_contracts::repository::audit::AuditLogWriteOutcome;
use aether_data_contracts::{repository::emergency_chain::*, DataLayerError};
use async_trait::async_trait;
use sqlx::{PgPool, Row};

#[derive(Debug, Clone)]
pub struct PostgresEmergencyChainGrantRepository {
    pool: PgPool,
    transaction_runner: PostgresTransactionRunner,
}

impl PostgresEmergencyChainGrantRepository {
    pub fn new(pool: PgPool) -> Self {
        Self {
            transaction_runner: PostgresTransactionRunner::new(pool.clone()),
            pool,
        }
    }
}

#[async_trait]
impl EmergencyChainGrantRepository for PostgresEmergencyChainGrantRepository {
    async fn issue_emergency_chain_grant(
        &self,
        record: IssueEmergencyChainGrant,
    ) -> Result<IssueEmergencyChainGrantOutcome, DataLayerError> {
        record.validate()?;
        self.transaction_runner
            .run_read_write(|transaction| Box::pin(issue(transaction, record)))
            .await
    }

    async fn read_emergency_chain_grant(
        &self,
        grant_id: &str,
    ) -> Result<Option<StoredEmergencyChainGrant>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  grant_id,
  principal,
  operations,
  request_id,
  request_fingerprint,
  session_nonce,
  chain_hash,
  issued_at_unix_secs,
  expires_at_unix_secs,
  revoked_at_unix_secs,
  consumed_at_unix_secs
FROM emergency_chain_grants
WHERE grant_id = $1
"#,
        )
        .bind(grant_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        match row {
            Some(row) => load(&self.pool, row).await.map(Some),
            None => Ok(None),
        }
    }

    async fn revoke_emergency_chain_grant(
        &self,
        record: RevokeEmergencyChainGrant,
    ) -> Result<RevokeEmergencyChainGrantOutcome, DataLayerError> {
        record.validate()?;
        self.transaction_runner
            .run_read_write(|transaction| Box::pin(revoke(transaction, record)))
            .await
    }

    async fn consume_emergency_chain_grant(
        &self,
        record: ConsumeEmergencyChainGrant,
    ) -> Result<ConsumeEmergencyChainGrantOutcome, DataLayerError> {
        record.validate()?;
        self.transaction_runner
            .run_read_write(|transaction| Box::pin(consume(transaction, record)))
            .await
    }
}

async fn issue(
    transaction: &mut PostgresTransaction,
    record: IssueEmergencyChainGrant,
) -> Result<IssueEmergencyChainGrantOutcome, DataLayerError> {
    let grant = record.grant;
    let operations = serde_json::to_value(&grant.operations).map_err(|error| {
        DataLayerError::UnexpectedValue(format!("failed to encode emergency operations: {error}"))
    })?;
    let inserted: Option<String> = sqlx::query_scalar(
        r#"
INSERT INTO emergency_chain_grants (
  grant_id,
  principal,
  operations,
  request_id,
  request_fingerprint,
  session_nonce,
  chain_hash,
  issued_at_unix_secs,
  expires_at_unix_secs,
  revoked_at_unix_secs
) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
ON CONFLICT (grant_id) DO NOTHING
RETURNING grant_id
"#,
    )
    .bind(&grant.grant_id)
    .bind(&grant.principal)
    .bind(operations)
    .bind(&grant.request_id)
    .bind(&grant.request_fingerprint)
    .bind(&grant.session_nonce)
    .bind(&grant.chain_hash)
    .bind(to_i64(grant.issued_at_unix_secs, "issued_at_unix_secs")?)
    .bind(to_i64(grant.expires_at_unix_secs, "expires_at_unix_secs")?)
    .bind(
        grant
            .revoked_at_unix_secs
            .map(|value| to_i64(value, "revoked_at_unix_secs"))
            .transpose()?,
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_postgres_err()?;
    if inserted.is_none() {
        return Ok(IssueEmergencyChainGrantOutcome::AlreadyExists);
    }

    for (position, target) in grant.targets.iter().enumerate() {
        sqlx::query(
            r#"
INSERT INTO emergency_chain_grant_targets (
  grant_id,
  chain_position,
  provider_id,
  endpoint_id,
  key_id
) VALUES ($1,$2,$3,$4,$5)
"#,
        )
        .bind(&grant.grant_id)
        .bind(i32::try_from(position).map_err(|_| {
            DataLayerError::InvalidInput("emergency target position exceeds integer range".into())
        })?)
        .bind(&target.provider_id)
        .bind(&target.endpoint_id)
        .bind(&target.key_id)
        .execute(&mut **transaction)
        .await
        .map_postgres_err()?;
    }

    insert_required_audit(transaction, &record.audit).await?;
    Ok(IssueEmergencyChainGrantOutcome::Issued)
}

async fn revoke(
    transaction: &mut PostgresTransaction,
    record: RevokeEmergencyChainGrant,
) -> Result<RevokeEmergencyChainGrantOutcome, DataLayerError> {
    let row = sqlx::query(
        r#"
SELECT principal, issued_at_unix_secs, revoked_at_unix_secs
FROM emergency_chain_grants
WHERE grant_id = $1
FOR UPDATE
"#,
    )
    .bind(&record.grant_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_postgres_err()?;
    let Some(row) = row else {
        return Ok(RevokeEmergencyChainGrantOutcome::NotFound);
    };

    let principal: String = row.try_get("principal").map_postgres_err()?;
    if principal != record.principal {
        return Ok(RevokeEmergencyChainGrantOutcome::PrincipalDenied);
    }
    let issued_at_unix_secs = to_u64(
        row.try_get("issued_at_unix_secs").map_postgres_err()?,
        "issued_at_unix_secs",
    )?;
    if record.revoked_at_unix_secs < issued_at_unix_secs {
        return Err(DataLayerError::InvalidInput(
            "emergency revocation predates issuance".into(),
        ));
    }
    let revoked_at_unix_secs = row
        .try_get::<Option<i64>, _>("revoked_at_unix_secs")
        .map_postgres_err()?
        .map(|value| to_u64(value, "revoked_at_unix_secs"))
        .transpose()?;
    if let Some(effective_at_unix_secs) = revoked_at_unix_secs {
        return Ok(RevokeEmergencyChainGrantOutcome::AlreadyRevoked {
            effective_at_unix_secs,
        });
    }

    sqlx::query(
        r#"
UPDATE emergency_chain_grants
SET revoked_at_unix_secs = $2,
    updated_at = clock_timestamp()
WHERE grant_id = $1
"#,
    )
    .bind(&record.grant_id)
    .bind(to_i64(record.revoked_at_unix_secs, "revoked_at_unix_secs")?)
    .execute(&mut **transaction)
    .await
    .map_postgres_err()?;
    insert_required_audit(transaction, &record.audit).await?;

    Ok(RevokeEmergencyChainGrantOutcome::Revoked {
        effective_at_unix_secs: record.revoked_at_unix_secs,
    })
}

async fn insert_required_audit(
    transaction: &mut PostgresTransaction,
    audit: &aether_data_contracts::repository::audit::CreateAdminAuditLog,
) -> Result<(), DataLayerError> {
    match crate::audit::insert_admin_audit_log(&mut **transaction, audit).await? {
        AuditLogWriteOutcome::Inserted => Ok(()),
        AuditLogWriteOutcome::AlreadyExists => Err(DataLayerError::InvalidInput(
            "emergency grant mutation requires a new admin audit log".into(),
        )),
    }
}

async fn consume(
    transaction: &mut PostgresTransaction,
    record: ConsumeEmergencyChainGrant,
) -> Result<ConsumeEmergencyChainGrantOutcome, DataLayerError> {
    let row = sqlx::query(
        r#"
SELECT principal, issued_at_unix_secs, expires_at_unix_secs, revoked_at_unix_secs, consumed_at_unix_secs
FROM emergency_chain_grants
WHERE grant_id = $1
FOR UPDATE
"#,
    )
    .bind(&record.grant_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_postgres_err()?;
    let Some(row) = row else {
        return Ok(ConsumeEmergencyChainGrantOutcome::NotFound);
    };
    if row.try_get::<String, _>("principal").map_postgres_err()? != record.principal {
        return Ok(ConsumeEmergencyChainGrantOutcome::PrincipalDenied);
    }
    let issued = to_u64(
        row.try_get("issued_at_unix_secs").map_postgres_err()?,
        "issued_at_unix_secs",
    )?;
    if record.consumed_at_unix_secs < issued {
        return Ok(ConsumeEmergencyChainGrantOutcome::NotYetValid);
    }
    let expires = to_u64(
        row.try_get("expires_at_unix_secs").map_postgres_err()?,
        "expires_at_unix_secs",
    )?;
    if record.consumed_at_unix_secs >= expires {
        return Ok(ConsumeEmergencyChainGrantOutcome::Expired);
    }
    if row
        .try_get::<Option<i64>, _>("revoked_at_unix_secs")
        .map_postgres_err()?
        .is_some()
    {
        return Ok(ConsumeEmergencyChainGrantOutcome::Revoked);
    }
    if let Some(value) = row
        .try_get::<Option<i64>, _>("consumed_at_unix_secs")
        .map_postgres_err()?
    {
        return Ok(ConsumeEmergencyChainGrantOutcome::AlreadyConsumed {
            effective_at_unix_secs: to_u64(value, "consumed_at_unix_secs")?,
        });
    }
    sqlx::query(
        "UPDATE emergency_chain_grants SET consumed_at_unix_secs=$2,updated_at=clock_timestamp() WHERE grant_id=$1",
    )
    .bind(&record.grant_id)
    .bind(to_i64(record.consumed_at_unix_secs, "consumed_at_unix_secs")?)
    .execute(&mut **transaction)
    .await
    .map_postgres_err()?;
    Ok(ConsumeEmergencyChainGrantOutcome::Consumed)
}

async fn load(
    pool: &PgPool,
    row: sqlx::postgres::PgRow,
) -> Result<StoredEmergencyChainGrant, DataLayerError> {
    let grant_id: String = row.try_get("grant_id").map_postgres_err()?;
    let target_rows = sqlx::query(
        r#"
SELECT provider_id, endpoint_id, key_id
FROM emergency_chain_grant_targets
WHERE grant_id = $1
ORDER BY chain_position
"#,
    )
    .bind(&grant_id)
    .fetch_all(pool)
    .await
    .map_postgres_err()?;
    let targets = target_rows
        .into_iter()
        .map(|row| {
            Ok(EmergencyChainTarget {
                provider_id: row.try_get("provider_id").map_postgres_err()?,
                endpoint_id: row.try_get("endpoint_id").map_postgres_err()?,
                key_id: row.try_get("key_id").map_postgres_err()?,
            })
        })
        .collect::<Result<Vec<_>, DataLayerError>>()?;
    let grant = StoredEmergencyChainGrant {
        grant_id,
        principal: row.try_get("principal").map_postgres_err()?,
        operations: serde_json::from_value(row.try_get("operations").map_postgres_err()?).map_err(
            |error| {
                DataLayerError::UnexpectedValue(format!("invalid emergency operations: {error}"))
            },
        )?,
        request_id: row.try_get("request_id").map_postgres_err()?,
        request_fingerprint: row.try_get("request_fingerprint").map_postgres_err()?,
        session_nonce: row.try_get("session_nonce").map_postgres_err()?,
        chain_hash: row.try_get("chain_hash").map_postgres_err()?,
        targets,
        issued_at_unix_secs: to_u64(
            row.try_get("issued_at_unix_secs").map_postgres_err()?,
            "issued_at_unix_secs",
        )?,
        expires_at_unix_secs: to_u64(
            row.try_get("expires_at_unix_secs").map_postgres_err()?,
            "expires_at_unix_secs",
        )?,
        revoked_at_unix_secs: row
            .try_get::<Option<i64>, _>("revoked_at_unix_secs")
            .map_postgres_err()?
            .map(|value| to_u64(value, "revoked_at_unix_secs"))
            .transpose()?,
        consumed_at_unix_secs: row
            .try_get::<Option<i64>, _>("consumed_at_unix_secs")
            .map_postgres_err()?
            .map(|value| to_u64(value, "consumed_at_unix_secs"))
            .transpose()?,
    };
    grant.validate()?;
    Ok(grant)
}

fn to_i64(value: u64, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field} exceeds the integer range")))
}

fn to_u64(value: i64, field: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("{field} must not be negative")))
}

#[cfg(test)]
mod tests;
