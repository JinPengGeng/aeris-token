use aether_data::repository::audit::CreateAdminAuditLog;
use axum::body::Body;
use axum::http::{self, Response, StatusCode};
use chrono::Utc;
use serde_json::json;
use tracing::{info, warn};

use crate::control::GatewayControlDecision;

#[derive(Debug, Clone)]
pub(crate) struct AdminAuditEvent {
    pub(crate) event_name: &'static str,
    pub(crate) action: &'static str,
    pub(crate) target_type: &'static str,
    pub(crate) target_id: String,
}

/// A response extension consumed by the outer request future after the normal
/// finalizer has emitted access logs. Keeping this pending record on the
/// response lets the finalizer stay synchronous while persistence is awaited
/// before Hyper receives the response.
#[derive(Debug, Clone)]
pub(crate) struct PendingAdminAudit(pub(crate) CreateAdminAuditLog);

pub(crate) fn attach_admin_audit_event(
    response: &mut Response<Body>,
    event_name: &'static str,
    action: &'static str,
    target_type: &'static str,
    target_id: impl Into<String>,
) {
    response.extensions_mut().insert(AdminAuditEvent {
        event_name,
        action,
        target_type,
        target_id: target_id.into(),
    });
}

pub(crate) fn emit_admin_audit(
    response: &mut Response<Body>,
    trace_id: &str,
    method: &http::Method,
    path_and_query: &str,
    control_decision: Option<&GatewayControlDecision>,
    client_ip: std::net::IpAddr,
) {
    let sanitized_path_and_query = sanitize_admin_audit_path(path_and_query);
    let Some(decision) = control_decision else {
        return;
    };
    let Some(admin_principal) = decision.admin_principal.as_ref() else {
        return;
    };

    let attached_event = response.extensions_mut().remove::<AdminAuditEvent>();
    let route_family = decision.route_family.as_deref().unwrap_or("unknown");
    let route_kind = decision.route_kind.as_deref().unwrap_or("unknown");
    let status_code = response.status().as_u16();
    if attached_event.is_none() && !is_admin_mutation_method(method) {
        return;
    }
    let (event_name, action, target_type, target_id) = if let Some(event) = attached_event {
        (
            event.event_name,
            event.action,
            event.target_type,
            event.target_id,
        )
    } else {
        (
            if response.status().is_success() {
                "admin_mutation_completed"
            } else {
                "admin_mutation_failed"
            },
            route_kind,
            default_target_type(route_family),
            sanitized_path_and_query.clone(),
        )
    };
    let target_id = sanitize_admin_audit_target_id(target_id);

    let (audit_status, log_level) = classify_admin_audit_response(method, response.status());
    if log_level == AdminAuditLogLevel::Info {
        info!(
            event_name,
            log_type = "audit",
            status = audit_status,
            status_code,
            trace_id = %trace_id,
            admin_user_id = admin_principal.user_id.as_str(),
            admin_user_role = admin_principal.user_role.as_str(),
            admin_session_id = admin_principal.session_id.as_deref().unwrap_or("-"),
            admin_management_token_id = admin_principal.management_token_id.as_deref().unwrap_or("-"),
            route_family,
            route_kind,
            method = %method,
            path = %sanitized_path_and_query,
            action,
            target_type,
            target_id = %target_id,
            "admin audit event"
        );
    } else {
        warn!(
            event_name,
            log_type = "audit",
            status = audit_status,
            status_code,
            trace_id = %trace_id,
            admin_user_id = admin_principal.user_id.as_str(),
            admin_user_role = admin_principal.user_role.as_str(),
            admin_session_id = admin_principal.session_id.as_deref().unwrap_or("-"),
            admin_management_token_id = admin_principal.management_token_id.as_deref().unwrap_or("-"),
            route_family,
            route_kind,
            method = %method,
            path = %sanitized_path_and_query,
            action,
            target_type,
            target_id = %target_id,
            "admin audit event"
        );
    }

    let request_id = response
        .headers()
        .get(crate::constants::CONTROL_REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let event_type = if is_admin_read_method(method) {
        "admin_sensitive_read"
    } else {
        "admin_mutation"
    };
    let metadata = json!({
        "schema_version": 1,
        "event_name": event_name,
        "status": audit_status,
        "admin_role": admin_principal.user_role.as_str(),
        "session_id": admin_principal.session_id.as_deref(),
        "management_token_id": admin_principal.management_token_id.as_deref(),
        "route_family": route_family,
        "route_kind": route_kind,
        "method": method.as_str(),
        "path": sanitized_path_and_query,
        "action": action,
        "target_type": target_type,
        "target_id": target_id,
    });
    let record = CreateAdminAuditLog {
        id: uuid::Uuid::now_v7().to_string(),
        event_type: event_type.to_string(),
        user_id: Some(admin_principal.user_id.clone()),
        api_key_id: None,
        description: format!("admin action: {action}"),
        ip_address: Some(client_ip.to_string()),
        user_agent: None,
        request_id,
        event_metadata: Some(metadata),
        status_code: Some(i32::from(status_code)),
        error_message: None,
        created_at: Utc::now(),
    };
    response.extensions_mut().insert(PendingAdminAudit(record));
}

pub(crate) async fn persist_admin_audit(
    data: &crate::data::GatewayDataState,
    record: CreateAdminAuditLog,
) {
    use std::time::Duration;

    let event_id = record.id.clone();
    let event_name = record
        .event_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("event_name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let action = record
        .event_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("action"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    match tokio::time::timeout(Duration::from_secs(2), data.create_admin_audit_log(&record)).await {
        Ok(Ok(_outcome)) => {}
        Ok(Err(_error)) => warn!(
            event_name = "admin_audit_persist_failed",
            audit_event_id = %event_id,
            audit_event = event_name,
            action,
            "admin audit persistence failed"
        ),
        Err(_elapsed) => warn!(
            event_name = "admin_audit_persist_timeout",
            audit_event_id = %event_id,
            audit_event = event_name,
            action,
            "admin audit persistence timed out"
        ),
    }
}

fn sanitize_admin_audit_path(path_and_query: &str) -> String {
    crate::middleware::sanitize_access_log_path(path_and_query)
}

fn sanitize_admin_audit_target_id(target_id: String) -> String {
    if target_id.trim_start().starts_with('/') {
        return sanitize_admin_audit_path(&target_id);
    }
    target_id
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminAuditLogLevel {
    Info,
    Warn,
}

fn classify_admin_audit_response(
    method: &http::Method,
    status: StatusCode,
) -> (&'static str, AdminAuditLogLevel) {
    if status.is_success() {
        return ("completed", AdminAuditLogLevel::Info);
    }
    if status == StatusCode::NOT_FOUND && is_admin_read_method(method) {
        return ("not_found", AdminAuditLogLevel::Info);
    }
    ("failed", AdminAuditLogLevel::Warn)
}

fn default_target_type(route_family: &str) -> &str {
    route_family
        .strip_suffix("_manage")
        .unwrap_or(route_family)
        .trim()
}

fn is_admin_mutation_method(method: &http::Method) -> bool {
    matches!(
        *method,
        http::Method::POST | http::Method::PUT | http::Method::PATCH | http::Method::DELETE
    )
}

fn is_admin_read_method(method: &http::Method) -> bool {
    matches!(*method, http::Method::GET | http::Method::HEAD)
}

#[cfg(test)]
mod tests {
    use super::{
        classify_admin_audit_response, sanitize_admin_audit_path, sanitize_admin_audit_target_id,
        AdminAuditLogLevel,
    };
    use axum::http::{Method, StatusCode};

    #[test]
    fn classifies_read_not_found_as_info_not_found() {
        assert_eq!(
            classify_admin_audit_response(&Method::GET, StatusCode::NOT_FOUND),
            ("not_found", AdminAuditLogLevel::Info)
        );
    }

    #[test]
    fn classifies_mutation_not_found_as_warn_failed() {
        assert_eq!(
            classify_admin_audit_response(&Method::DELETE, StatusCode::NOT_FOUND),
            ("failed", AdminAuditLogLevel::Warn)
        );
    }

    #[test]
    fn audit_paths_drop_sensitive_query_values() {
        assert_eq!(
            sanitize_admin_audit_path(
                "/api/admin/providers?token=secret&api_key=live-key&limit=25"
            ),
            "/api/admin/providers?limit=25"
        );
        assert_eq!(
            sanitize_admin_audit_path("/install/one-time-secret?view=raw"),
            "/install/[redacted]?view=raw"
        );
    }

    #[test]
    fn path_shaped_audit_targets_drop_sensitive_query_values() {
        assert_eq!(
            sanitize_admin_audit_target_id(
                "/api/admin/monitoring/trace/request-1?token=secret&limit=25".to_string(),
            ),
            "/api/admin/monitoring/trace/request-1?limit=25"
        );
        assert_eq!(
            sanitize_admin_audit_target_id("resource-id?literal".to_string()),
            "resource-id?literal"
        );
    }
}
