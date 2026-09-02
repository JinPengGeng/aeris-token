use crate::handlers::admin::admin_provider_pool_config;
use crate::handlers::admin::provider::shared::paths::admin_provider_id_for_keys;
use crate::handlers::admin::provider::shared::payloads::AdminProviderKeyCreateRequest;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::maintenance::ensure_provider_key_pool_scores_for_keys;
use crate::provider_key_auth::provider_key_effective_api_formats;
use crate::{model_fetch::perform_model_fetch_for_key, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("create_provider_key")
        || request_context.method() != http::Method::POST
        || !request_context
            .path()
            .starts_with("/api/admin/endpoints/providers/")
        || !request_context.path().ends_with("/keys")
    {
        return Ok(None);
    }

    let Some(provider_id) = admin_provider_id_for_keys(request_context.path()) else {
        return Ok(Some(not_found_response("Provider 不存在")));
    };
    let Some(request_body) = request_body else {
        return Ok(Some(bad_request_response("请求体不能为空")));
    };
    if !state.has_provider_catalog_data_reader() {
        return Ok(None);
    }
    let payload = match serde_json::from_slice::<AdminProviderKeyCreateRequest>(request_body) {
        Ok(payload) => payload,
        Err(_) => return Ok(Some(bad_request_response("请求体必须是合法的 JSON 对象"))),
    };
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(not_found_response(format!(
            "Provider {provider_id} 不存在"
        ))));
    };
    let record = match state
        .build_admin_create_provider_key_record(&provider, payload)
        .await
    {
        Ok(record) => record,
        Err(detail) => return Ok(Some(bad_request_response(detail))),
    };
    let Some(created) = state.create_provider_catalog_key(&record).await? else {
        return Ok(None);
    };
    let key_id = created.id.clone();
    let mut model_sync_warning: Option<String> = None;
    let created = if created.auto_fetch_models {
        match perform_model_fetch_for_key(state.as_ref(), &provider.id, &created.id).await {
            Ok(summary) if summary.succeeded > 0 => {}
            Ok(_) => {
                let detail = state
                    .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
                    .await?
                    .into_iter()
                    .next()
                    .and_then(|key| key.last_models_fetch_error)
                    .unwrap_or_else(|| "未获取到可用上游模型".to_string());
                model_sync_warning = Some(format!("开启自动获取模型后同步上游模型失败: {detail}"));
            }
            Err(err) => {
                model_sync_warning = Some(format!(
                    "开启自动获取模型后同步上游模型失败: {}",
                    err.into_message()
                ));
            }
        }

        state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
            .await?
            .into_iter()
            .next()
            .unwrap_or(created)
    } else {
        created
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    if let Some(pool_config) = admin_provider_pool_config(&provider) {
        let score_ensure_budget = (pool_config.score_fallback_scan_limit as usize).clamp(1, 50_000);
        if let Err(err) = ensure_provider_key_pool_scores_for_keys(
            state.as_ref(),
            &provider,
            &pool_config,
            &endpoints,
            std::slice::from_ref(&created),
            now_unix_secs,
            score_ensure_budget,
        )
        .await
        {
            tracing::debug!(
                provider_id = %provider.id,
                key_id = %created.id,
                error = ?err,
                "gateway admin provider key create: failed to seed pool score rows"
            );
        }
    }
    let api_formats =
        provider_key_effective_api_formats(&created, &provider.provider_type, &endpoints);

    let mut payload = state.build_admin_provider_key_response(
        &created,
        &provider.provider_type,
        &api_formats,
        now_unix_secs,
    );
    if let Some(warning) = model_sync_warning {
        if let Some(object) = payload.as_object_mut() {
            object.insert("model_sync_warning".to_string(), json!(warning));
        }
    }

    Ok(Some(attach_admin_audit_response(
        Json(payload).into_response(),
        "admin_provider_key_created",
        "create_provider_key",
        "provider_key",
        &created.id,
    )))
}

fn bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn not_found_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}
