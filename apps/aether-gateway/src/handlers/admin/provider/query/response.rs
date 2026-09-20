use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) const ADMIN_PROVIDER_QUERY_INVALID_JSON_DETAIL: &str = "Invalid JSON request body";
pub(crate) const ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL: &str = "provider_id is required";
pub(crate) const ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL: &str = "model is required";
pub(crate) const ADMIN_PROVIDER_QUERY_FAILOVER_MODELS_REQUIRED_DETAIL: &str =
    "failover_models should not be empty";
pub(crate) const ADMIN_PROVIDER_QUERY_PROVIDER_NOT_FOUND_DETAIL: &str = "Provider not found";
pub(crate) const ADMIN_PROVIDER_QUERY_API_KEY_NOT_FOUND_DETAIL: &str = "API Key not found";
pub(crate) const ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL: &str =
    "No active API Key found for this provider";
pub(crate) const ADMIN_PROVIDER_QUERY_NO_LOCAL_MODELS_DETAIL: &str =
    "No models available from local provider catalog";
pub(crate) const ADMIN_PROVIDER_QUERY_EMERGENCY_TARGETS_REQUIRED_DETAIL: &str =
    "targets must contain 1 to 32 ordered endpoint_id/key_id pairs";
pub(crate) const ADMIN_PROVIDER_QUERY_EMERGENCY_GRANT_ID_REQUIRED_DETAIL: &str =
    "emergency grant_id is required";
pub(crate) const ADMIN_PROVIDER_QUERY_EMERGENCY_UNAVAILABLE_DETAIL: &str =
    "Emergency chain persistence is unavailable";

pub(crate) fn build_admin_provider_query_bad_request_response(
    detail: &'static str,
) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

pub(crate) fn build_admin_provider_query_not_found_response(
    detail: &'static str,
) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}
