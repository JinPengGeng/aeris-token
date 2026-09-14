use crate::handlers::admin::provider::shared::paths::{
    admin_provider_delete_task_parts, admin_provider_id_for_manage_path,
};
use crate::handlers::admin::provider::shared::support::build_admin_provider_delete_task_payload;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::task_runtime::ProviderDeleteAuditOrigin;
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn build_admin_provider_not_found_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn build_admin_provider_delete_task_terminal_audit(
    provider_id: &str,
    task_id: &str,
    task_status: &str,
    response: Response<Body>,
) -> Response<Body> {
    match task_status {
        "completed" => attach_admin_audit_response(
            response,
            "admin_provider_delete_task_completed_viewed",
            "view_provider_delete_task_terminal_state",
            "provider_delete_task",
            &format!("{provider_id}:{task_id}"),
        ),
        "failed" => attach_admin_audit_response(
            response,
            "admin_provider_delete_task_failed_viewed",
            "view_provider_delete_task_terminal_state",
            "provider_delete_task",
            &format!("{provider_id}:{task_id}"),
        ),
        _ => response,
    }
}

pub(crate) async fn maybe_build_local_admin_provider_delete_task_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    route_kind: Option<&str>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if route_kind == Some("delete_provider") && request_context.method() == http::Method::DELETE {
        let Some(provider_id) = admin_provider_id_for_manage_path(request_context.path()) else {
            return Ok(Some(build_admin_provider_not_found_response(
                "Provider 不存在",
            )));
        };
        let audit_origin = request_context
            .decision()
            .and_then(|decision| decision.admin_principal.as_ref())
            .map(|principal| ProviderDeleteAuditOrigin {
                user_id: Some(principal.user_id.clone()),
                session_id: principal.session_id.clone(),
                management_token_id: principal.management_token_id.clone(),
                trace_id: Some(request_context.trace_id().to_string()),
                client_ip: request_context.public().client_ip.clone(),
            })
            .unwrap_or_default();
        let Some(task_id) = crate::task_runtime::submit_provider_delete_task(
            state,
            &provider_id,
            Some("admin"),
            audit_origin,
        )
        .await?
        else {
            return Ok(Some(build_admin_provider_not_found_response(
                "提供商不存在",
            )));
        };
        return Ok(Some(attach_admin_audit_response(
            Json(json!({
                "task_id": task_id,
                "run_id": task_id,
                "status": "pending",
                "message": "删除任务已提交，提供商已进入后台删除队列",
            }))
            .into_response(),
            "admin_provider_delete_queued",
            "delete_provider",
            "provider",
            &provider_id,
        )));
    }

    if route_kind == Some("delete_provider_task") && request_context.method() == http::Method::GET {
        let Some((provider_id, task_id)) = admin_provider_delete_task_parts(request_context.path())
        else {
            return Ok(Some(build_admin_provider_not_found_response(
                "Task not found",
            )));
        };
        let Some(task) = state.get_provider_delete_task(&task_id) else {
            return Ok(Some(build_admin_provider_not_found_response(
                "Task not found",
            )));
        };
        if task.provider_id != provider_id {
            return Ok(Some(build_admin_provider_not_found_response(
                "Task not found",
            )));
        }
        return Ok(Some(build_admin_provider_delete_task_terminal_audit(
            &provider_id,
            &task_id,
            task.status.as_str(),
            Json(build_admin_provider_delete_task_payload(&task)).into_response(),
        )));
    }

    Ok(None)
}
