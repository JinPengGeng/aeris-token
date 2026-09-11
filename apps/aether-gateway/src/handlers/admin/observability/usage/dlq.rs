use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{attach_admin_audit_response, query_param_value};
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_data_unavailable_response,
};
use aether_data_contracts::DataLayerError;
use aether_runtime_state::RuntimeQueueRedriveOutcome;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

const MAX_DLQ_PAGE: usize = 100;

fn dlq_error_response(error: DataLayerError) -> Response<Body> {
    match error {
        DataLayerError::InvalidInput(detail) => admin_usage_bad_request_response(detail),
        _ => admin_usage_data_unavailable_response("Usage DLQ backend is unavailable"),
    }
}

fn page_limit(query: Option<&str>) -> Result<usize, String> {
    let raw = query_param_value(query, "limit");
    let value = raw
        .as_deref()
        .unwrap_or("50")
        .parse::<usize>()
        .map_err(|_| "limit must be a positive integer".to_string())?;
    if value == 0 || value > MAX_DLQ_PAGE {
        return Err(format!("limit must be between 1 and {MAX_DLQ_PAGE}"));
    }
    Ok(value)
}

pub(super) async fn maybe_build_local_admin_usage_dlq_response(
    state: &AdminAppState<'_>,
    context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    match context.route_kind() {
        Some("dlq_list") if context.method() == http::Method::GET => {
            let limit = match page_limit(context.query_string()) {
                Ok(limit) => limit,
                Err(detail) => return Ok(Some(admin_usage_bad_request_response(detail))),
            };
            let cursor = query_param_value(context.query_string(), "cursor")
                .unwrap_or_else(|| "0-0".to_string());
            let page = match state
                .app()
                .usage_runtime
                .dead_letter_page(state.app().background_data.as_ref(), &cursor, limit)
                .await
            {
                Ok(page) => page,
                Err(error) => return Ok(Some(dlq_error_response(error))),
            };
            let next_cursor = page.next_start_id;
            let has_more = page.has_more;
            let entries = page
                .entries
                .into_iter()
                .map(|entry| {
                    let payload = entry
                        .fields
                        .get("payload")
                        .and_then(|value| serde_json::from_str::<Value>(value).ok());
                    let payload_object = payload.as_ref().and_then(Value::as_object);
                    json!({
                        "id": entry.id,
                        "entry_id": payload_object.and_then(|object| object.get("entry_id")),
                        "error": payload_object.and_then(|object| object.get("error")),
                        "payload_valid": payload.is_some(),
                    })
                })
                .collect::<Vec<_>>();
            let response = Json(json!({
                "stream": state.app().usage_runtime.dlq_stream_key(),
                "cursor": cursor,
                "next_cursor": next_cursor,
                "has_more": has_more,
                "entries": entries,
            }))
            .into_response();
            return Ok(Some(attach_admin_audit_response(
                response,
                "admin_usage_dlq_listed",
                "list_usage_dead_letters",
                "usage_dlq",
                state.app().usage_runtime.dlq_stream_key(),
            )));
        }
        Some("dlq_redrive") if context.method() == http::Method::POST => {
            let Some(id) = context
                .path()
                .strip_prefix("/api/admin/usage/dlq/")
                .and_then(|suffix| suffix.strip_suffix("/redrive"))
                .filter(|value| !value.is_empty())
            else {
                return Ok(Some(admin_usage_bad_request_response(
                    "DLQ entry ID is required",
                )));
            };
            let outcome = match state
                .app()
                .usage_runtime
                .redrive_dead_letter(state.app().background_data.as_ref(), id)
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => return Ok(Some(dlq_error_response(error))),
            };
            let (status, state_label, destination_id) = match outcome {
                RuntimeQueueRedriveOutcome::Redriven { destination_id } => {
                    ("redriven", "redriven", Some(destination_id))
                }
                RuntimeQueueRedriveOutcome::AlreadyRedriven { destination_id } => {
                    ("already_redriven", "already_redriven", Some(destination_id))
                }
                RuntimeQueueRedriveOutcome::NotFound => ("not_found", "not_found", None),
            };
            let response = Json(json!({
                "ok": status != "not_found",
                "status": state_label,
                "entry_id": id,
                "destination_id": destination_id,
            }))
            .into_response();
            return Ok(Some(attach_admin_audit_response(
                response,
                "admin_usage_dlq_redriven",
                "redrive_usage_dead_letter",
                "usage_dlq",
                id,
            )));
        }
        _ => {}
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::page_limit;

    #[test]
    fn dlq_page_limit_is_bounded() {
        assert_eq!(page_limit(None).unwrap(), 50);
        assert_eq!(page_limit(Some("limit=100")).unwrap(), 100);
        assert!(page_limit(Some("limit=101")).is_err());
        assert!(page_limit(Some("limit=0")).is_err());
        assert!(page_limit(Some("limit=nope")).is_err());
    }
}
