use std::net::IpAddr;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, Method};
use axum::response::Response;

use crate::control::{GatewayAdminPrincipalContext, GatewayControlDecision};
use crate::state::AppState;

use super::OperationalPermission;

/// Only authenticated forensic reads carry this context. Credentials and query
/// strings are deliberately not captured; the target is an operational path.
pub(super) struct Context {
    decision: GatewayControlDecision,
    action: &'static str,
    method: Method,
    path: String,
    client_ip: IpAddr,
    read_id: String,
}

impl Context {
    pub(super) fn new(
        permission: &OperationalPermission,
        principal: &GatewayAdminPrincipalContext,
        request: &Request,
        client_ip: IpAddr,
    ) -> Option<Self> {
        let action = permission.audit_action?;
        // Axum's GET routes also execute their handlers for HEAD requests.
        if !matches!(*request.method(), Method::GET | Method::HEAD) {
            return None;
        }
        // The outer request lifecycle owns both handler and audit finalizer.
        // Its producer guard also retains this work through usage shutdown.
        crate::request_lifecycle::configure_client_disconnect(
            aether_routing_core::RoutingExecutionPolicy::default(),
        );
        let path = request.uri().path().to_string();
        let mut decision = GatewayControlDecision::synthetic(
            path.clone(),
            Some("operational".into()),
            Some("admin".into()),
            Some(action.into()),
            None,
        );
        decision.admin_principal = Some(principal.clone());
        Some(Self {
            decision,
            action,
            method: request.method().clone(),
            path,
            client_ip,
            read_id: uuid::Uuid::now_v7().to_string(),
        })
    }
}

pub(super) async fn finish(
    state: &AppState,
    context: Option<Context>,
    mut response: Response<Body>,
) -> Response<Body> {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(context) = context {
        crate::audit::attach_admin_audit_event(
            &mut response,
            "admin_operational_read",
            context.action,
            "operational_audit_target",
            context.path.clone(),
        );
        crate::audit::emit_admin_audit(
            &mut response,
            &context.read_id,
            &context.method,
            &context.path,
            Some(&context.decision),
            context.client_ip,
        );
        if let Some(crate::audit::PendingAdminAudit(mut record)) = response
            .extensions_mut()
            .remove::<crate::audit::PendingAdminAudit>(
        ) {
            // Correlate the viewing operation independently of the historical
            // target request ID, which remains in the sanitized target path.
            record.request_id = Some(context.read_id);
            crate::audit::persist_admin_audit(&state.data, &state.admin_audit_metrics, record)
                .await;
        }
    }
    response
}
