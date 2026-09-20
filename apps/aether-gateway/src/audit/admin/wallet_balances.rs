use aether_data::repository::audit::CreateAdminAuditLog;
use chrono::Utc;
use serde_json::json;

use crate::control::GatewayControlDecision;

use super::sanitize_admin_audit_target_id_with_truncation;

pub(crate) fn build_admin_wallet_balance_audit(
    decision: &GatewayControlDecision,
    target_id: &str,
    manual_recharge: bool,
    client_ip: Option<&str>,
) -> Option<CreateAdminAuditLog> {
    let principal = decision.admin_principal.as_ref()?;
    let (event_name, action, target_type, route_kind, path) = if manual_recharge {
        (
            "admin_wallet_manual_recharge_created",
            "create_manual_wallet_recharge",
            "wallet",
            "recharge_balance",
            "/api/admin/wallets/[wallet_id]/recharge",
        )
    } else {
        (
            "admin_wallet_balance_adjusted",
            "adjust_wallet_balance",
            "wallet",
            "adjust_balance",
            "/api/admin/wallets/[wallet_id]/adjust",
        )
    };
    let (target_id, target_truncated) =
        sanitize_admin_audit_target_id_with_truncation(target_id.to_string());
    let id = uuid::Uuid::now_v7().to_string();
    Some(CreateAdminAuditLog {
        id: id.clone(),
        event_type: "admin_mutation".to_string(),
        user_id: Some(principal.user_id.clone()),
        api_key_id: None,
        description: format!("admin action: {action}"),
        ip_address: client_ip
            .and_then(|value| value.parse::<std::net::IpAddr>().ok())
            .map(|value| value.to_string()),
        user_agent: None,
        request_id: Some(id),
        event_metadata: Some(json!({
            "schema_version": 1,
            "event_name": event_name,
            "status": "completed",
            "admin_role": principal.user_role.as_str(),
            "session_id": principal.session_id.as_deref(),
            "management_token_id": principal.management_token_id.as_deref(),
            "route_family": "wallets_manage",
            "route_kind": route_kind,
            "method": "POST",
            "path": path,
            "action": action,
            "target_type": target_type,
            "target_id": target_id,
            "target_truncated": target_truncated.then_some(true),
        })),
        status_code: Some(200),
        error_message: None,
        created_at: Utc::now(),
    })
}
