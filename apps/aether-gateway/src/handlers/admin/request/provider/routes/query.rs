use crate::handlers::admin::provider::query::{
    models::{
        build_admin_provider_query_emergency_chain_execute_response,
        build_admin_provider_query_emergency_chain_revoke_response,
        build_admin_provider_query_models_response,
        build_admin_provider_query_test_model_failover_local_response,
        build_admin_provider_query_test_model_local_response,
    },
    payload::{
        parse_admin_provider_query_body, provider_query_extract_emergency_targets,
        provider_query_extract_failover_models, provider_query_extract_model,
        provider_query_extract_provider_id, provider_query_extract_request_id,
        provider_query_payload_keys,
    },
    response::{
        build_admin_provider_query_bad_request_response,
        ADMIN_PROVIDER_QUERY_EMERGENCY_GRANT_ID_REQUIRED_DETAIL,
        ADMIN_PROVIDER_QUERY_EMERGENCY_TARGETS_REQUIRED_DETAIL,
        ADMIN_PROVIDER_QUERY_FAILOVER_MODELS_REQUIRED_DETAIL,
        ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL,
        ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
    },
};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::AdminRequestContext;
use crate::log_ids::short_request_id;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use tracing::warn;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn maybe_build_admin_provider_query_route_response(
        &self,
        request_context: &AdminRequestContext<'_>,
        request_body: Option<&Bytes>,
    ) -> Result<Option<Response<Body>>, GatewayError> {
        let Some(decision) = request_context.decision() else {
            return Ok(None);
        };

        if decision.route_family.as_deref() != Some("provider_query_manage") {
            return Ok(None);
        }

        if request_context.method() != http::Method::POST {
            return Ok(None);
        }

        let payload = match parse_admin_provider_query_body(request_body) {
            Ok(value) => value,
            Err(response) => return Ok(Some(response)),
        };

        let route_kind = decision.route_kind.as_deref().unwrap_or("query_models");
        match route_kind {
            "query_models" => Ok(Some(
                build_admin_provider_query_models_response(self, &payload).await?,
            )),
            "test_model" => {
                let Some(_provider_id) = provider_query_extract_provider_id(&payload) else {
                    log_admin_provider_query_validation_failure(
                        request_context,
                        route_kind,
                        ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
                        &payload,
                    );
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
                    )));
                };
                let Some(_model) = provider_query_extract_model(&payload) else {
                    log_admin_provider_query_validation_failure(
                        request_context,
                        route_kind,
                        ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL,
                        &payload,
                    );
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL,
                    )));
                };
                Ok(Some(
                    build_admin_provider_query_test_model_local_response(self, &payload).await?,
                ))
            }
            "test_model_failover" => {
                let Some(_provider_id) = provider_query_extract_provider_id(&payload) else {
                    log_admin_provider_query_validation_failure(
                        request_context,
                        route_kind,
                        ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
                        &payload,
                    );
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
                    )));
                };
                let failover_models = provider_query_extract_failover_models(&payload);
                if failover_models.is_empty() {
                    log_admin_provider_query_validation_failure(
                        request_context,
                        route_kind,
                        ADMIN_PROVIDER_QUERY_FAILOVER_MODELS_REQUIRED_DETAIL,
                        &payload,
                    );
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_FAILOVER_MODELS_REQUIRED_DETAIL,
                    )));
                }
                Ok(Some(
                    build_admin_provider_query_test_model_failover_local_response(self, &payload)
                        .await?,
                ))
            }
            "emergency_chain_execute" => {
                let Some(principal) = decision
                    .admin_principal
                    .as_ref()
                    .filter(|principal| principal.user_role.eq_ignore_ascii_case("admin"))
                else {
                    return Ok(Some(
                        (
                            http::StatusCode::FORBIDDEN,
                            Json(json!({ "detail": "Administrator role is required" })),
                        )
                            .into_response(),
                    ));
                };
                let Some(provider_id) = provider_query_extract_provider_id(&payload) else {
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
                    )));
                };
                let Some(model) = provider_query_extract_model(&payload) else {
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL,
                    )));
                };
                let Some(targets) =
                    provider_query_extract_emergency_targets(&payload, &provider_id)
                else {
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_EMERGENCY_TARGETS_REQUIRED_DETAIL,
                    )));
                };
                Ok(Some(
                    build_admin_provider_query_emergency_chain_execute_response(
                        self,
                        &principal.user_id,
                        provider_id,
                        model,
                        targets,
                    )
                    .await?,
                ))
            }
            "emergency_chain_revoke" => {
                let Some(principal) = decision
                    .admin_principal
                    .as_ref()
                    .filter(|principal| principal.user_role.eq_ignore_ascii_case("admin"))
                else {
                    return Ok(Some(
                        (
                            http::StatusCode::FORBIDDEN,
                            Json(json!({ "detail": "Administrator role is required" })),
                        )
                            .into_response(),
                    ));
                };
                let Some(grant_id) = emergency_chain_grant_id_from_path(request_context.path())
                else {
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_EMERGENCY_GRANT_ID_REQUIRED_DETAIL,
                    )));
                };
                Ok(Some(
                    build_admin_provider_query_emergency_chain_revoke_response(
                        self,
                        &principal.user_id,
                        grant_id,
                    )
                    .await?,
                ))
            }
            _ => Ok(Some(
                build_admin_provider_query_models_response(self, &payload).await?,
            )),
        }
    }
}

fn emergency_chain_grant_id_from_path(path: &str) -> Option<&str> {
    let value = path
        .trim_end_matches('/')
        .strip_prefix("/api/admin/provider-query/emergency-chain/")?
        .strip_suffix("/revoke")?
        .trim_end_matches('/');
    (!value.is_empty()
        && value.len() <= 256
        && !value.contains('/')
        && !value.contains("://")
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control))
    .then_some(value)
}

fn log_admin_provider_query_validation_failure(
    request_context: &AdminRequestContext<'_>,
    route_kind: &str,
    detail: &'static str,
    payload: &serde_json::Value,
) {
    let provider_id =
        provider_query_extract_provider_id(payload).unwrap_or_else(|| "-".to_string());
    let model = provider_query_extract_model(payload).unwrap_or_else(|| "-".to_string());
    let request_id = provider_query_extract_request_id(payload).unwrap_or_else(|| "-".to_string());
    let request_id_for_log = short_request_id(request_id.as_str());
    let payload_keys = provider_query_payload_keys(payload);
    warn!(
        event_name = "admin_provider_query_request_rejected",
        log_type = "validation",
        route_kind,
        path = %request_context.path(),
        request_id = %request_id_for_log,
        provider_id = %provider_id,
        model = %model,
        payload_keys = ?payload_keys,
        detail,
        "admin provider query request rejected"
    );
}

#[cfg(test)]
mod tests {
    use super::emergency_chain_grant_id_from_path;

    #[test]
    fn emergency_chain_revoke_path_accepts_one_opaque_grant_segment() {
        assert_eq!(
            emergency_chain_grant_id_from_path(
                "/api/admin/provider-query/emergency-chain/grant-123/revoke"
            ),
            Some("grant-123")
        );
        assert_eq!(
            emergency_chain_grant_id_from_path(
                "/api/admin/provider-query/emergency-chain/grant/extra/revoke"
            ),
            None
        );
    }
}
