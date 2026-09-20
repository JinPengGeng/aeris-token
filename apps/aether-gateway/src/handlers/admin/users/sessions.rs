use super::{
    build_admin_users_bad_request_response, build_admin_users_permission_denied_response,
    format_optional_datetime_iso8601, management_token_may_administer_user_accounts,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::{GatewayError, GatewayUserSessionView};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn mark_durable_admin_audit(
    response: &mut Response<Body>,
    audit: aether_data::repository::audit::CreateAdminAuditLog,
) {
    response
        .extensions_mut()
        .insert(crate::audit::DurableAdminAuditEnqueued);
    response
        .extensions_mut()
        .insert(crate::audit::PendingAdminAudit(audit));
}

fn admin_user_id_from_sessions_path(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/users/")?
        .strip_suffix("/sessions")
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

fn admin_user_session_parts(request_path: &str) -> Option<(String, String)> {
    let raw = request_path.strip_prefix("/api/admin/users/")?;
    let (user_id, session_id) = raw.split_once("/sessions/")?;
    let user_id = user_id.trim().trim_matches('/');
    let session_id = session_id.trim().trim_matches('/');
    if user_id.is_empty()
        || session_id.is_empty()
        || user_id.contains('/')
        || session_id.contains('/')
    {
        None
    } else {
        Some((user_id.to_string(), session_id.to_string()))
    }
}

fn format_required_session_datetime_iso8601(session: &GatewayUserSessionView) -> String {
    session
        .created_at
        .or(session.updated_at)
        .or(session.last_seen_at)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339()
}

pub(super) async fn build_admin_list_user_sessions_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_id) = admin_user_id_from_sessions_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };

    if state.find_user_auth_by_id(&user_id).await?.is_none() {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    }

    let sessions = state.list_user_sessions(&user_id).await?;
    let payload = sessions
        .into_iter()
        .map(|session| {
            json!({
                "id": session.id,
                "device_label": session
                    .device_label
                    .clone()
                    .unwrap_or_else(|| "未知设备".to_string()),
                "device_type": "unknown",
                "browser_name": serde_json::Value::Null,
                "browser_version": serde_json::Value::Null,
                "os_name": serde_json::Value::Null,
                "os_version": serde_json::Value::Null,
                "device_model": serde_json::Value::Null,
                "ip_address": session.ip_address,
                "last_seen_at": format_optional_datetime_iso8601(session.last_seen_at),
                "created_at": format_required_session_datetime_iso8601(&session),
                "is_current": false,
                "revoked_at": format_optional_datetime_iso8601(session.revoked_at),
                "revoke_reason": session.revoke_reason,
            })
        })
        .collect::<Vec<_>>();

    Ok(Json(payload).into_response())
}

pub(super) async fn build_admin_delete_user_session_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some((user_id, session_id)) = admin_user_session_parts(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response(
            "缺少 user_id 或 session_id",
        ));
    };

    let Some(user) = state.find_user_auth_by_id(&user_id).await? else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    };
    if crate::roles::can_access_admin_console(&user.role)
        && !management_token_may_administer_user_accounts(request_context)
    {
        return Ok(build_admin_users_permission_denied_response(
            request_context,
        ));
    }

    let revoked_at = chrono::Utc::now();
    let audit = request_context.decision().and_then(|decision| {
        crate::audit::build_admin_session_revocation_audit(
            decision,
            &session_id,
            false,
            request_context.public().client_ip.as_deref(),
        )
    });
    let durable = if let Some(audit) = audit.as_ref() {
        state
            .admin_revoke_user_session_with_audit(
                &user_id,
                &session_id,
                revoked_at,
                "admin_session_revoked",
                audit,
            )
            .await?
    } else {
        None
    };
    if matches!(
        durable,
        Some(aether_data::repository::users::AdminUserSessionRevocationOutcome::NotFound)
    ) {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "会话不存在" })),
        )
            .into_response());
    }
    if durable.is_none() {
        if state
            .find_user_session(&user_id, &session_id)
            .await?
            .is_none()
        {
            return Ok((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "会话不存在" })),
            )
                .into_response());
        }
        state
            .revoke_user_session(&user_id, &session_id, revoked_at, "admin_session_revoked")
            .await?;
    }

    let mut response = attach_admin_audit_response(
        Json(json!({ "message": "用户设备已强制下线" })).into_response(),
        "admin_user_session_deleted",
        "delete_user_session",
        "user_session",
        &session_id,
    );
    if durable.is_some() {
        mark_durable_admin_audit(
            &mut response,
            audit.expect("durable audit enqueue requires an audit intent"),
        );
    }
    Ok(response)
}

pub(super) async fn build_admin_delete_user_sessions_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_id) = admin_user_id_from_sessions_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };

    let Some(user) = state.find_user_auth_by_id(&user_id).await? else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    };
    if crate::roles::can_access_admin_console(&user.role)
        && !management_token_may_administer_user_accounts(request_context)
    {
        return Ok(build_admin_users_permission_denied_response(
            request_context,
        ));
    }

    let revoked_at = chrono::Utc::now();
    let audit = request_context.decision().and_then(|decision| {
        crate::audit::build_admin_session_revocation_audit(
            decision,
            &user_id,
            true,
            request_context.public().client_ip.as_deref(),
        )
    });
    let durable = if let Some(audit) = audit.as_ref() {
        state
            .admin_revoke_all_user_sessions_with_audit(
                &user_id,
                revoked_at,
                "admin_revoke_all_sessions",
                audit,
            )
            .await?
    } else {
        None
    };
    let revoked_count = match durable {
        Some(aether_data::repository::users::AdminUserSessionsRevocationOutcome::Revoked(
            count,
        )) => count,
        Some(aether_data::repository::users::AdminUserSessionsRevocationOutcome::NotFound) => {
            return Ok((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "用户不存在" })),
            )
                .into_response());
        }
        None => {
            state
                .revoke_all_user_sessions(&user_id, revoked_at, "admin_revoke_all_sessions")
                .await?
        }
    };

    let mut response = attach_admin_audit_response(
        Json(json!({
            "message": "已强制下线该用户所有设备",
            "revoked_count": revoked_count,
        }))
        .into_response(),
        "admin_user_sessions_deleted",
        "delete_user_sessions",
        "user",
        &user_id,
    );
    if durable.is_some() {
        mark_durable_admin_audit(
            &mut response,
            audit.expect("durable audit enqueue requires an audit intent"),
        );
    }
    Ok(response)
}
