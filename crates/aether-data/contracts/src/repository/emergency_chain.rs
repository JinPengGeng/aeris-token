use crate::repository::audit::CreateAdminAuditLog;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const EMERGENCY_CHAIN_MAX_TTL_SECS: u64 = 86_400;
const EMERGENCY_CHAIN_MAX_TARGETS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EmergencyChainTarget {
    pub provider_id: String,
    pub endpoint_id: String,
    pub key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredEmergencyChainGrant {
    pub grant_id: String,
    pub principal: String,
    pub operations: Vec<String>,
    pub request_id: String,
    pub request_fingerprint: String,
    pub session_nonce: String,
    pub chain_hash: String,
    pub targets: Vec<EmergencyChainTarget>,
    pub issued_at_unix_secs: u64,
    pub expires_at_unix_secs: u64,
    pub revoked_at_unix_secs: Option<u64>,
    pub consumed_at_unix_secs: Option<u64>,
}

impl StoredEmergencyChainGrant {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        value("grant_id", &self.grant_id)?;
        value("principal", &self.principal)?;
        value("request_id", &self.request_id)?;
        value("session_nonce", &self.session_nonce)?;
        hash("request_fingerprint", &self.request_fingerprint)?;
        hash("chain_hash", &self.chain_hash)?;
        timestamp("issued_at_unix_secs", self.issued_at_unix_secs)?;
        timestamp("expires_at_unix_secs", self.expires_at_unix_secs)?;
        if let Some(revoked_at_unix_secs) = self.revoked_at_unix_secs {
            timestamp("revoked_at_unix_secs", revoked_at_unix_secs)?;
            if revoked_at_unix_secs < self.issued_at_unix_secs {
                return invalid("emergency revocation predates issuance");
            }
        }
        if let Some(consumed_at_unix_secs) = self.consumed_at_unix_secs {
            timestamp("consumed_at_unix_secs", consumed_at_unix_secs)?;
            if consumed_at_unix_secs < self.issued_at_unix_secs {
                return invalid("emergency consumption predates issuance");
            }
        }
        if self.operations.is_empty()
            || self.targets.is_empty()
            || self.targets.len() > EMERGENCY_CHAIN_MAX_TARGETS
            || self.expires_at_unix_secs <= self.issued_at_unix_secs
            || self.expires_at_unix_secs - self.issued_at_unix_secs > EMERGENCY_CHAIN_MAX_TTL_SECS
        {
            return invalid("invalid emergency grant bounds");
        }

        let mut operations = BTreeSet::new();
        for operation in &self.operations {
            value("operation", operation)?;
            if operation == "*" || !operations.insert(operation) {
                return invalid("operations must be explicit and unique");
            }
        }

        let mut targets = BTreeSet::new();
        for target in &self.targets {
            value("provider_id", &target.provider_id)?;
            value("endpoint_id", &target.endpoint_id)?;
            value("key_id", &target.key_id)?;
            if !targets.insert((&target.provider_id, &target.endpoint_id, &target.key_id)) {
                return invalid("targets must be unique");
            }
        }
        if self.chain_hash != emergency_chain_target_hash(&self.targets) {
            return invalid("emergency chain hash does not bind targets");
        }
        Ok(())
    }
}

/// Produces the ordered target-chain digest used by scheduler-core grants.
pub fn emergency_chain_target_hash(targets: &[EmergencyChainTarget]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"aether-emergency-chain-v1\0");
    hasher.update((targets.len() as u64).to_be_bytes());
    for target in targets {
        hash_length_prefixed(&mut hasher, target.provider_id.as_bytes());
        hash_length_prefixed(&mut hasher, target.endpoint_id.as_bytes());
        hash_length_prefixed(&mut hasher, target.key_id.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[derive(Debug, Clone)]
pub struct IssueEmergencyChainGrant {
    pub grant: StoredEmergencyChainGrant,
    pub audit: CreateAdminAuditLog,
}

#[derive(Debug, Clone)]
pub struct RevokeEmergencyChainGrant {
    pub grant_id: String,
    pub principal: String,
    pub revoked_at_unix_secs: u64,
    pub audit: CreateAdminAuditLog,
}
#[derive(Debug, Clone)]
pub struct ConsumeEmergencyChainGrant {
    pub grant_id: String,
    pub principal: String,
    pub consumed_at_unix_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueEmergencyChainGrantOutcome {
    Issued,
    AlreadyExists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeEmergencyChainGrantOutcome {
    NotFound,
    PrincipalDenied,
    Revoked { effective_at_unix_secs: u64 },
    AlreadyRevoked { effective_at_unix_secs: u64 },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumeEmergencyChainGrantOutcome {
    NotFound,
    PrincipalDenied,
    NotYetValid,
    Expired,
    Revoked,
    AlreadyConsumed { effective_at_unix_secs: u64 },
    Consumed,
}

impl IssueEmergencyChainGrant {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        self.grant.validate()?;
        if self.grant.revoked_at_unix_secs.is_some() {
            return invalid("new emergency grants must not be revoked");
        }
        if self.grant.consumed_at_unix_secs.is_some() {
            return invalid("new emergency grants must not be consumed");
        }
        self.audit.validate()?;
        if self.audit.user_id.as_deref() != Some(&self.grant.principal)
            || self.audit.request_id.as_deref() != Some(&self.grant.request_id)
        {
            return invalid("emergency issue audit binding mismatch");
        }
        Ok(())
    }
}
impl ConsumeEmergencyChainGrant {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        value("grant_id", &self.grant_id)?;
        value("principal", &self.principal)?;
        timestamp("consumed_at_unix_secs", self.consumed_at_unix_secs)
    }
}

impl RevokeEmergencyChainGrant {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        value("grant_id", &self.grant_id)?;
        value("principal", &self.principal)?;
        timestamp("revoked_at_unix_secs", self.revoked_at_unix_secs)?;
        self.audit.validate()?;
        if self.audit.user_id.as_deref() != Some(&self.principal) {
            return invalid("emergency revoke audit principal mismatch");
        }
        Ok(())
    }
}

#[async_trait]
pub trait EmergencyChainGrantRepository: Send + Sync {
    async fn issue_emergency_chain_grant(
        &self,
        record: IssueEmergencyChainGrant,
    ) -> Result<IssueEmergencyChainGrantOutcome, crate::DataLayerError>;

    async fn read_emergency_chain_grant(
        &self,
        grant_id: &str,
    ) -> Result<Option<StoredEmergencyChainGrant>, crate::DataLayerError>;

    async fn revoke_emergency_chain_grant(
        &self,
        record: RevokeEmergencyChainGrant,
    ) -> Result<RevokeEmergencyChainGrantOutcome, crate::DataLayerError>;
    async fn consume_emergency_chain_grant(
        &self,
        record: ConsumeEmergencyChainGrant,
    ) -> Result<ConsumeEmergencyChainGrantOutcome, crate::DataLayerError>;
}

fn hash_length_prefixed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn timestamp(field: &str, value: u64) -> Result<(), crate::DataLayerError> {
    if value > i64::MAX as u64 {
        return invalid(&format!("{field} exceeds the integer range"));
    }
    Ok(())
}

fn value(field: &str, value: &str) -> Result<(), crate::DataLayerError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
        || value.contains("://")
    {
        return invalid(&format!("invalid emergency {field}"));
    }
    Ok(())
}

fn hash(field: &str, value: &str) -> Result<(), crate::DataLayerError> {
    if value.len() != 64
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return invalid(&format!("invalid emergency {field}"));
    }
    Ok(())
}

fn invalid<T>(message: &str) -> Result<T, crate::DataLayerError> {
    Err(crate::DataLayerError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn grant() -> StoredEmergencyChainGrant {
        let targets = vec![
            EmergencyChainTarget {
                provider_id: "provider-a".to_string(),
                endpoint_id: "endpoint-a".to_string(),
                key_id: "key-a".to_string(),
            },
            EmergencyChainTarget {
                provider_id: "provider-b".to_string(),
                endpoint_id: "endpoint-b".to_string(),
                key_id: "key-b".to_string(),
            },
        ];
        StoredEmergencyChainGrant {
            grant_id: "grant-a".to_string(),
            principal: "principal-a".to_string(),
            operations: vec!["responses.create".to_string()],
            request_id: "request-a".to_string(),
            request_fingerprint: "a".repeat(64),
            session_nonce: "nonce-a".to_string(),
            chain_hash: emergency_chain_target_hash(&targets),
            targets,
            issued_at_unix_secs: 10,
            expires_at_unix_secs: 20,
            revoked_at_unix_secs: None,
            consumed_at_unix_secs: None,
        }
    }

    fn audit(principal: &str, request_id: &str) -> CreateAdminAuditLog {
        CreateAdminAuditLog {
            id: "audit-a".to_string(),
            event_type: "admin_mutation".to_string(),
            user_id: Some(principal.to_string()),
            api_key_id: None,
            description: "admin action: issue_emergency_chain_grant".to_string(),
            ip_address: None,
            user_agent: None,
            request_id: Some(request_id.to_string()),
            event_metadata: None,
            status_code: Some(200),
            error_message: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn chain_hash_binds_the_ordered_immutable_targets() {
        let mut issued = grant();
        issued.targets.swap(0, 1);
        assert!(issued.validate().is_err());

        let issued = grant();
        assert_eq!(
            issued.chain_hash,
            "8f0589f900456a262847902c3a103651801510fbb32283368ba22e321ba8f350"
        );
    }

    #[test]
    fn issue_rejects_database_overflow_and_unbound_audit() {
        let mut issued = grant();
        issued.expires_at_unix_secs = i64::MAX as u64 + 1;
        let request = IssueEmergencyChainGrant {
            audit: audit(&issued.principal, &issued.request_id),
            grant: issued,
        };
        assert!(request.validate().is_err());

        let issued = grant();
        let request = IssueEmergencyChainGrant {
            audit: audit("another-principal", &issued.request_id),
            grant: issued,
        };
        assert!(request.validate().is_err());
    }
}
