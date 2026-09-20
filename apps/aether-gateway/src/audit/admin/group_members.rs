use super::{
    sanitize_admin_audit_target_id_with_truncation, CreateAdminAuditLog, GatewayControlDecision,
};
use chrono::Utc;
use serde_json::json;

pub(crate) fn build_user_group_members_update_audit(
    decision: &GatewayControlDecision,
    target_id: &str,
    client_ip: Option<&str>,
) -> Option<CreateAdminAuditLog> {
    let principal = decision.admin_principal.as_ref()?;
    let (target_id, target_truncated) =
        sanitize_admin_audit_target_id_with_truncation(target_id.to_string());
    let id = uuid::Uuid::now_v7().to_string();
    Some(CreateAdminAuditLog {
        id: id.clone(),
        event_type: "admin_mutation".to_string(),
        user_id: Some(principal.user_id.clone()),
        api_key_id: None,
        description: "admin action: update_user_group_members".to_string(),
        ip_address: client_ip
            .and_then(|value| value.parse::<std::net::IpAddr>().ok())
            .map(|value| value.to_string()),
        user_agent: None,
        // Client trace headers are not length bounded. Keep the durable request
        // correlation server-controlled while structured logs retain trace_id.
        request_id: Some(id),
        event_metadata: Some(json!({
            "schema_version": 1,
            "event_name": "admin_user_group_members_updated",
            "status": "completed",
            "admin_role": principal.user_role.as_str(),
            "session_id": principal.session_id.as_deref(),
            "management_token_id": principal.management_token_id.as_deref(),
            "route_family": "users_manage",
            "route_kind": "replace_user_group_members",
            "method": "PUT",
            "path": "/api/admin/user-groups/[group_id]/members",
            "action": "update_user_group_members",
            "target_type": "user_group",
            "target_id": target_id,
            "target_truncated": target_truncated.then_some(true),
        })),
        status_code: Some(200),
        error_message: None,
        created_at: Utc::now(),
    })
}
