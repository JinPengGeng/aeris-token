mod types;

#[cfg(test)]
mod tests;

pub use aether_data_contracts::repository::audit::{
    optional_json_from_text, AuditLogListQuery, AuditLogReadRepository, AuditLogWriteOutcome,
    AuditLogWriteRepository, CreateAdminAuditLog, StoredAdminAuditLog, StoredAdminAuditLogPage,
    StoredSuspiciousActivity, StoredUserAuditLog, StoredUserAuditLogPage,
    ADMIN_AUDIT_METADATA_MAX_BYTES, ADMIN_AUDIT_METADATA_MAX_STRING_BYTES,
    ADMIN_AUDIT_METADATA_MAX_TARGET_BYTES, SUSPICIOUS_EVENT_TYPES,
};
#[cfg(feature = "postgres")]
pub use aether_data_postgres::PostgresAuditLogReadRepository;
pub use types::{read_request_audit_bundle, RequestAuditBundle, RequestAuditReader};
