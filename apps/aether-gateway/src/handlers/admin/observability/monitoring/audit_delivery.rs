use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::monitoring::{
    admin_monitoring_bad_request_response, admin_monitoring_not_found_response,
};
use aether_data_contracts::repository::audit::{
    AdminAuditDeliveryListQuery, AdminAuditDeliveryRedriveOutcome, AdminAuditDeliveryState,
    ADMIN_AUDIT_DELIVERY_MAX_PAGE,
};
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde_json::json;

pub(super) async fn build_audit_delivery_list_response(
    state: &AdminAppState<'_>,
    context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let query = context.request_query_string.as_deref();
    let limit = match query_param_value(query, "limit")
        .as_deref()
        .unwrap_or("50")
        .parse::<usize>()
    {
        Ok(v) if (1..=ADMIN_AUDIT_DELIVERY_MAX_PAGE).contains(&v) => v,
        _ => {
            return Ok(admin_monitoring_bad_request_response(
                "limit must be between 1 and 100",
            ))
        }
    };
    let state_filter = match query_param_value(query, "state").as_deref() {
        None => None,
        Some("pending") => Some(AdminAuditDeliveryState::Pending),
        Some("leased") => Some(AdminAuditDeliveryState::Leased),
        Some("delivered") => Some(AdminAuditDeliveryState::Delivered),
        Some("dead_letter") => Some(AdminAuditDeliveryState::DeadLetter),
        Some(_) => {
            return Ok(admin_monitoring_bad_request_response(
                "invalid delivery state",
            ))
        }
    };
    let before_created_at = match query_param_value(query, "before_created_at") {
        None => None,
        Some(v) => match DateTime::parse_from_rfc3339(&v) {
            Ok(v) => Some(v.with_timezone(&Utc)),
            Err(_) => {
                return Ok(admin_monitoring_bad_request_response(
                    "before_created_at must be RFC3339",
                ))
            }
        },
    };
    let before_event_id = query_param_value(query, "before_event_id");
    if before_created_at.is_some() != before_event_id.is_some() {
        return Ok(admin_monitoring_bad_request_response(
            "cursor requires before_created_at and before_event_id",
        ));
    }
    let page = state
        .as_ref()
        .list_admin_audit_deliveries(&AdminAuditDeliveryListQuery {
            state: state_filter,
            limit,
            before_created_at,
            before_event_id,
        })
        .await?;
    let summary = state.as_ref().admin_audit_delivery_summary().await?;
    let next = page.items.last().filter(|_| page.has_more).map(|item| {
        json!({
            "before_created_at": item.created_at.to_rfc3339(), "before_event_id": item.event_id,
        })
    });
    Ok(Json(json!({"items": page.items, "has_more": page.has_more, "next_cursor": next, "summary": summary})).into_response())
}

pub(super) async fn build_audit_delivery_redrive_response(
    state: &AdminAppState<'_>,
    context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let event_id = context
        .request_path
        .trim_end_matches('/')
        .strip_prefix("/api/admin/monitoring/audit-deliveries/")
        .and_then(|v| v.strip_suffix("/redrive"))
        .filter(|v| !v.is_empty());
    let Some(event_id) = event_id else {
        return Ok(admin_monitoring_bad_request_response(
            "event_id is required",
        ));
    };
    let outcome = match state.as_ref().redrive_admin_audit_delivery(event_id).await {
        Ok(outcome) => outcome,
        Err(error) => {
            state.app().admin_audit_metrics.record_redrive_error();
            return Err(error);
        }
    };
    state.app().admin_audit_metrics.record_redrive(outcome);
    match outcome {
        AdminAuditDeliveryRedriveOutcome::Redriven => {
            Ok(Json(json!({"status":"redriven","event_id":event_id})).into_response())
        }
        AdminAuditDeliveryRedriveOutcome::NotFound => Ok(admin_monitoring_not_found_response(
            "Audit delivery not found",
        )),
        AdminAuditDeliveryRedriveOutcome::NotDeadLetter => Ok((
            axum::http::StatusCode::CONFLICT,
            Json(json!({"detail":"Audit delivery is not dead-lettered","event_id":event_id})),
        )
            .into_response()),
    }
}
