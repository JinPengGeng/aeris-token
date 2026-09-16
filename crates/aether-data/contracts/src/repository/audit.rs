use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub const SUSPICIOUS_EVENT_TYPES: &[&str] = &[
    "suspicious_activity",
    "unauthorized_access",
    "login_failed",
    "request_rate_limited",
];

/// Metadata is intentionally a flat, small object.  The gateway owns the
/// allowlisted keys and sanitizes values before constructing an audit record;
/// this contract validation prevents another caller from persisting arbitrary
/// request or credential payloads through the generic `Value` field.
pub const ADMIN_AUDIT_METADATA_MAX_BYTES: usize = 4096;
pub const ADMIN_AUDIT_METADATA_MAX_STRING_BYTES: usize = 512;
pub const ADMIN_AUDIT_METADATA_MAX_TARGET_BYTES: usize = 256;

const ADMIN_AUDIT_METADATA_KEYS: &[&str] = &[
    "schema_version",
    "event_name",
    "status",
    "admin_role",
    "session_id",
    "management_token_id",
    "route_family",
    "route_kind",
    "method",
    "path",
    "action",
    "target_type",
    "target_id",
    "target_truncated",
    // Provider-delete terminal events are emitted by the task runtime rather
    // than the HTTP audit producer, so their lifecycle fields are included in
    // the same bounded contract.
    "task_id",
    "task_status",
    "origin_trace_id",
    "stage",
    "deleted_keys",
    "total_keys",
    "deleted_endpoints",
    "total_endpoints",
];

const ADMIN_AUDIT_METADATA_STRING_KEYS: &[&str] = &[
    "event_name",
    "status",
    "admin_role",
    "session_id",
    "management_token_id",
    "route_family",
    "route_kind",
    "method",
    "path",
    "action",
    "target_type",
    "target_id",
    "task_id",
    "task_status",
    "origin_trace_id",
    "stage",
];

const ADMIN_AUDIT_METADATA_COUNT_KEYS: &[&str] = &[
    "deleted_keys",
    "total_keys",
    "deleted_endpoints",
    "total_endpoints",
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
        self.validate_event_metadata()?;
        Ok(())
    }

    /// Validate the flat, versioned metadata contract at the data boundary.
    ///
    /// `event_metadata` remains an `Option<Value>` for compatibility with
    /// historical rows and callers, but new values may only contain the
    /// explicitly documented scalar fields.  In particular, nested objects,
    /// arrays, unknown keys, negative counters, and oversized strings are
    /// rejected before reaching PostgreSQL.
    pub fn validate_event_metadata(&self) -> Result<(), crate::DataLayerError> {
        let Some(metadata) = self.event_metadata.as_ref() else {
            return Ok(());
        };
        let serde_json::Value::Object(fields) = metadata else {
            return Err(crate::DataLayerError::InvalidInput(
                "audit log metadata must be a JSON object".to_string(),
            ));
        };
        let schema_version = fields
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                crate::DataLayerError::InvalidInput(
                    "audit log metadata schema_version must be 1".to_string(),
                )
            })?;
        if schema_version != 1 {
            return Err(crate::DataLayerError::InvalidInput(
                "audit log metadata schema_version must be 1".to_string(),
            ));
        }
        let event_name = fields
            .get("event_name")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                crate::DataLayerError::InvalidInput(
                    "audit log metadata event_name must be a non-empty string".to_string(),
                )
            })?;
        if event_name.len() > ADMIN_AUDIT_METADATA_MAX_STRING_BYTES {
            return Err(crate::DataLayerError::InvalidInput(
                "audit log metadata string value exceeds its limit".to_string(),
            ));
        }

        for (key, value) in fields {
            if !ADMIN_AUDIT_METADATA_KEYS.contains(&key.as_str()) {
                return Err(crate::DataLayerError::InvalidInput(format!(
                    "audit log metadata key is not allowlisted: {key}"
                )));
            }
            if value.is_null() {
                continue;
            }
            if ADMIN_AUDIT_METADATA_STRING_KEYS.contains(&key.as_str()) {
                let Some(text) = value.as_str() else {
                    return Err(crate::DataLayerError::InvalidInput(format!(
                        "audit log metadata field {key} must be a string or null"
                    )));
                };
                if text.trim().is_empty()
                    || text.len() > ADMIN_AUDIT_METADATA_MAX_STRING_BYTES
                    || (key == "target_id" && text.len() > ADMIN_AUDIT_METADATA_MAX_TARGET_BYTES)
                {
                    return Err(crate::DataLayerError::InvalidInput(format!(
                        "audit log metadata field {key} exceeds its limit"
                    )));
                }
                continue;
            }
            if ADMIN_AUDIT_METADATA_COUNT_KEYS.contains(&key.as_str()) {
                if value.as_u64().is_none() {
                    return Err(crate::DataLayerError::InvalidInput(format!(
                        "audit log metadata field {key} must be a non-negative integer"
                    )));
                }
                continue;
            }
            if key == "schema_version" {
                if value.as_u64() != Some(1) {
                    return Err(crate::DataLayerError::InvalidInput(
                        "audit log metadata schema_version must be 1".to_string(),
                    ));
                }
                continue;
            }
            if key == "target_truncated" {
                if !value.is_boolean() {
                    return Err(crate::DataLayerError::InvalidInput(
                        "audit log metadata target_truncated must be a boolean or null".to_string(),
                    ));
                }
                continue;
            }
            unreachable!("every allowlisted metadata key has a type category");
        }
        if serde_json::to_vec(metadata)
            .map(|encoded| encoded.len() > ADMIN_AUDIT_METADATA_MAX_BYTES)
            .unwrap_or(true)
        {
            return Err(crate::DataLayerError::InvalidInput(
                "audit log metadata exceeds its size limit".to_string(),
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

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::CreateAdminAuditLog;

    fn record(metadata: serde_json::Value) -> CreateAdminAuditLog {
        CreateAdminAuditLog {
            id: "audit-1".to_string(),
            event_type: "admin_mutation".to_string(),
            user_id: None,
            api_key_id: None,
            description: "admin action: update".to_string(),
            ip_address: None,
            user_agent: None,
            request_id: None,
            event_metadata: Some(metadata),
            status_code: Some(200),
            error_message: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn validates_generic_metadata_shape_and_nullable_fields() {
        let value = json!({
            "schema_version": 1,
            "event_name": "admin_mutation_completed",
            "status": "completed",
            "session_id": null,
            "management_token_id": null,
            "path": "/api/admin/system/configs/example",
            "target_id": "/api/admin/system/configs/example",
            "target_truncated": false,
        });
        assert!(record(value).validate().is_ok());
    }

    #[test]
    fn validates_provider_delete_counters_and_rejects_negative_values() {
        let valid = json!({
            "schema_version": 1,
            "event_name": "admin_provider_delete_task_terminal",
            "action": "provider_delete_task",
            "target_type": "provider",
            "target_id": "provider-1",
            "task_id": "task-1",
            "task_status": "completed",
            "deleted_keys": 2,
            "total_keys": 3,
            "deleted_endpoints": 1,
            "total_endpoints": 1,
        });
        assert!(record(valid).validate().is_ok());

        let negative = json!({
            "schema_version": 1,
            "event_name": "admin_provider_delete_task_terminal",
            "deleted_keys": -1,
        });
        assert!(record(negative).validate().is_err());
    }

    #[test]
    fn rejects_unknown_nested_and_sensitive_metadata_fields() {
        for metadata in [
            json!({"schema_version": 1, "event_name": "x", "authorization": "secret"}),
            json!({"schema_version": 1, "event_name": "x", "details": {"token": "secret"}}),
            json!({"schema_version": 1, "event_name": "x", "payload": ["body"]}),
        ] {
            assert!(record(metadata).validate().is_err());
        }
    }

    #[test]
    fn rejects_wrong_schema_version_and_oversized_values() {
        assert!(record(json!({"schema_version": 2, "event_name": "x"}))
            .validate()
            .is_err());
        assert!(record(json!({"schema_version": 1, "event_name": ""}))
            .validate()
            .is_err());
        assert!(record(json!({
            "schema_version": 1,
            "event_name": "x",
            "target_id": "x".repeat(257),
        }))
        .validate()
        .is_err());
    }
}
