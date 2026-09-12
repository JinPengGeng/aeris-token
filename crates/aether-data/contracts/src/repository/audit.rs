use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub const SUSPICIOUS_EVENT_TYPES: &[&str] = &[
    "suspicious_activity",
    "unauthorized_access",
    "login_failed",
    "request_rate_limited",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditLogListQuery {
    pub cutoff_unix_secs: u64,
    pub username_pattern: Option<String>,
    pub event_type: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

/// A validated, redacted audit record prepared by the gateway boundary.
///
/// The write contract deliberately accepts a fixed shape rather than arbitrary
/// tracing fields so callers cannot accidentally persist request or credential
/// payloads.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateAdminAuditLog {
    pub id: String,
    pub event_type: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub description: String,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub request_id: Option<String>,
    pub event_metadata: Option<Value>,
    pub status_code: Option<i32>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl CreateAdminAuditLog {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.id.trim().is_empty() || self.id.len() > 36 {
            return Err(crate::DataLayerError::InvalidInput(
                "audit log id must be 1..=36 characters".to_string(),
            ));
        }
        if self.event_type.trim().is_empty() || self.event_type.len() > 50 {
            return Err(crate::DataLayerError::InvalidInput(
                "audit log event_type must be 1..=50 characters".to_string(),
            ));
        }
        if self.description.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "audit log description must not be empty".to_string(),
            ));
        }
        if self.user_id.as_ref().is_some_and(|value| value.len() > 36)
            || self
                .api_key_id
                .as_ref()
                .is_some_and(|value| value.len() > 36)
            || self
                .ip_address
                .as_ref()
                .is_some_and(|value| value.len() > 45)
            || self
                .user_agent
                .as_ref()
                .is_some_and(|value| value.len() > 500)
            || self
                .request_id
                .as_ref()
                .is_some_and(|value| value.len() > 100)
        {
            return Err(crate::DataLayerError::InvalidInput(
                "audit log bounded text field exceeds its database limit".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditLogWriteOutcome {
    Inserted,
    AlreadyExists,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminAuditLog {
    pub id: String,
    pub event_type: String,
    pub user_id: Option<String>,
    pub user_email: Option<String>,
    pub user_username: Option<String>,
    pub description: Option<String>,
    pub ip_address: Option<String>,
    pub status_code: Option<i32>,
    pub error_message: Option<String>,
    pub metadata: Option<Value>,
    pub created_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredSuspiciousActivity {
    pub id: String,
    pub event_type: String,
    pub user_id: Option<String>,
    pub description: Option<String>,
    pub ip_address: Option<String>,
    pub metadata: Option<Value>,
    pub created_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserAuditLog {
    pub id: String,
    pub event_type: String,
    pub description: Option<String>,
    pub ip_address: Option<String>,
    pub status_code: Option<i32>,
    pub created_at_unix_secs: u64,
}

fn unix_secs_to_rfc3339(secs: u64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(secs.min(i64::MAX as u64) as i64, 0)
        .map(|value| value.to_rfc3339())
}

impl StoredAdminAuditLog {
    pub fn created_at_rfc3339(&self) -> Option<String> {
        unix_secs_to_rfc3339(self.created_at_unix_secs)
    }
}

impl StoredSuspiciousActivity {
    pub fn created_at_rfc3339(&self) -> Option<String> {
        unix_secs_to_rfc3339(self.created_at_unix_secs)
    }
}

impl StoredUserAuditLog {
    pub fn created_at_rfc3339(&self) -> Option<String> {
        unix_secs_to_rfc3339(self.created_at_unix_secs)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredAdminAuditLogPage {
    pub items: Vec<StoredAdminAuditLog>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredUserAuditLogPage {
    pub items: Vec<StoredUserAuditLog>,
    pub total: u64,
}

#[async_trait]
pub trait AuditLogReadRepository: Send + Sync {
    async fn list_admin_audit_logs(
        &self,
        query: &AuditLogListQuery,
    ) -> Result<StoredAdminAuditLogPage, crate::DataLayerError>;

    async fn list_admin_suspicious_activities(
        &self,
        cutoff_unix_secs: u64,
    ) -> Result<Vec<StoredSuspiciousActivity>, crate::DataLayerError>;

    async fn read_admin_user_behavior_event_counts(
        &self,
        user_id: &str,
        cutoff_unix_secs: u64,
    ) -> Result<std::collections::BTreeMap<String, u64>, crate::DataLayerError>;

    async fn list_user_audit_logs(
        &self,
        user_id: &str,
        query: &AuditLogListQuery,
    ) -> Result<StoredUserAuditLogPage, crate::DataLayerError>;

    async fn delete_audit_logs_before(
        &self,
        cutoff_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, crate::DataLayerError>;
}

#[async_trait]
pub trait AuditLogWriteRepository: Send + Sync {
    async fn create_admin_audit_log(
        &self,
        record: &CreateAdminAuditLog,
    ) -> Result<AuditLogWriteOutcome, crate::DataLayerError>;
}

pub fn optional_json_from_text(
    value: Option<String>,
) -> Result<Option<Value>, crate::DataLayerError> {
    value
        .filter(|raw| !raw.trim().is_empty())
        .map(|raw| {
            serde_json::from_str(&raw).map_err(|err| {
                crate::DataLayerError::UnexpectedValue(format!(
                    "invalid audit log metadata json: {err}"
                ))
            })
        })
        .transpose()
}
