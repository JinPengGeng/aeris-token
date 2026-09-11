use super::super::actions::{
    admin_provider_ops_is_valid_action_type, admin_provider_ops_local_action_response,
};
use super::super::balance_cache::{
    admin_provider_ops_pending_balance_response, read_admin_provider_ops_balance_cache,
    spawn_admin_provider_ops_balance_refresh, store_admin_provider_ops_balance_cache,
    AdminProviderOpsBalanceCacheLookup,
};
use super::super::config::admin_provider_ops_config_object;
use super::super::support::AdminProviderOpsExecuteActionRequest;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn handle_admin_provider_ops_action(
    state: &AdminAppState<'_>,
    provider_id: &str,
    route_kind: &str,
    action_route: Option<&(String, String)>,
    query_string: Option<&str>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let action_type = if route_kind == "provider_checkin" {
        "checkin".to_string()
    } else if matches!(
        route_kind,
        "get_provider_balance" | "refresh_provider_balance"
    ) {
        "query_balance".to_string()
    } else {
        let Some((_, action_type)) = action_route else {
            return Ok(None);
        };
        if !admin_provider_ops_is_valid_action_type(action_type) {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": format!("无效的操作类型: {action_type}") })),
                )
                    .into_response(),
            ));
        }
        action_type.clone()
    };

    let request_config = if route_kind == "execute_provider_action" {
        match request_body {
            Some(body) if !body.is_empty() => {
                let raw_value = match serde_json::from_slice::<serde_json::Value>(body) {
                    Ok(raw_value) => raw_value,
                    Err(_) => {
                        return Ok(Some(bad_request_detail_response(
                            "请求体必须是合法的 JSON 对象",
                        )));
                    }
                };
                let payload =
                    match serde_json::from_value::<AdminProviderOpsExecuteActionRequest>(raw_value)
                    {
                        Ok(payload) => payload,
                        Err(_) => {
                            return Ok(Some(bad_request_detail_response(
                                "请求体必须是合法的 JSON 对象",
                            )));
                        }
                    };
                payload.config
            }
            _ => None,
        }
    } else {
        None
    };

    let provider_ids = [provider_id.to_string()];
    let providers = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await?;
    let provider = providers.first();
    let endpoints = if provider.is_some() {
        state
            .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
            .await?
    } else {
        Vec::new()
    };
    let payload = if action_type == "query_balance"
        && route_kind == "get_provider_balance"
        && provider.is_some_and(|provider| admin_provider_ops_config_object(provider).is_some())
    {
        match read_admin_provider_ops_balance_cache(state, provider_id).await {
            AdminProviderOpsBalanceCacheLookup::Hit(cached) => {
                if query_param_bool(query_string, "refresh", true) {
                    spawn_admin_provider_ops_balance_refresh(state, provider_id).await;
                }
                cached
            }
            AdminProviderOpsBalanceCacheLookup::Miss => {
                if query_param_bool(query_string, "refresh", true)
                    && !state.runtime_state().is_memory()
                {
                    spawn_admin_provider_ops_balance_refresh(state, provider_id).await;
                    admin_provider_ops_pending_balance_response("余额数据加载中，请稍后刷新")
                } else {
                    let payload = admin_provider_ops_local_action_response(
                        state,
                        provider_id,
                        provider,
                        &endpoints,
                        &action_type,
                        request_config.as_ref(),
                    )
                    .await;
                    store_admin_provider_ops_balance_cache(state, provider_id, &payload).await;
                    payload
                }
            }
            AdminProviderOpsBalanceCacheLookup::Unavailable => {
                let payload = admin_provider_ops_local_action_response(
                    state,
                    provider_id,
                    provider,
                    &endpoints,
                    &action_type,
                    request_config.as_ref(),
                )
                .await;
                store_admin_provider_ops_balance_cache(state, provider_id, &payload).await;
                payload
            }
        }
    } else {
        if action_type == "sync_remote_quota" && provider.is_none() {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        }
        let payload = admin_provider_ops_local_action_response(
            state,
            provider_id,
            provider,
            &endpoints,
            &action_type,
            request_config.as_ref(),
        )
        .await;
        if action_type == "query_balance" && route_kind == "refresh_provider_balance" {
            store_admin_provider_ops_balance_cache(state, provider_id, &payload).await;
        }
        // 未启用远程配额同步的 Provider 调用 sync_remote_quota 属于客户端语义错误，
        // 明确返回 400 而不是统一 200，避免被当作服务端故障。
        if action_type == "sync_remote_quota"
            && payload.get("status").and_then(serde_json::Value::as_str) == Some("not_configured")
        {
            return Ok(Some(
                (http::StatusCode::BAD_REQUEST, Json(payload)).into_response(),
            ));
        }
        payload
    };

    Ok(Some(Json(payload).into_response()))
}

fn query_param_bool(query: Option<&str>, key: &str, default: bool) -> bool {
    let Some(query) = query else {
        return default;
    };
    for (entry_key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if entry_key == key {
            let normalized = value.trim().to_ascii_lowercase();
            return matches!(normalized.as_str(), "1" | "true" | "yes" | "on");
        }
    }
    default
}

fn bad_request_detail_response(detail: &str) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::handle_admin_provider_ops_action;
    use crate::data::GatewayDataState;
    use crate::handlers::admin::request::AdminAppState;
    use crate::AppState;
    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
    use serde_json::json;
    use std::sync::Arc;

    fn provider_with_ops_config(
        provider_id: &str,
        remote_quota: serde_json::Value,
    ) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            provider_id.to_string(),
            "Sub2API 中转".to_string(),
            None,
            "custom".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            Some(json!({
                "provider_ops": {
                    "architecture_id": "sub2api",
                    "base_url": "http://127.0.0.1:9",
                    "connector": {
                        "auth_type": "api_key",
                        "config": {},
                        "credentials": {
                            "refresh_token": encrypt_python_fernet_plaintext(
                                DEVELOPMENT_ENCRYPTION_KEY,
                                "initial-refresh-token",
                            ).expect("refresh token should encrypt"),
                        }
                    },
                    "remote_quota": remote_quota,
                }
            })),
        )
    }

    fn state_with_providers(providers: Vec<StoredProviderCatalogProvider>) -> AppState {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            providers,
            vec![],
            vec![],
        ));
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository)
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    async fn execute_sync_remote_quota(
        state: &AppState,
        provider_id: &str,
    ) -> (http::StatusCode, serde_json::Value) {
        let admin_state = AdminAppState::new(state);
        let action_route = (provider_id.to_string(), "sync_remote_quota".to_string());
        let response = handle_admin_provider_ops_action(
            &admin_state,
            provider_id,
            "execute_provider_action",
            Some(&action_route),
            None,
            None,
        )
        .await
        .expect("route should handle")
        .expect("route should match");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let payload = serde_json::from_slice(&body).expect("body should be JSON");
        (status, payload)
    }

    #[tokio::test]
    async fn sync_remote_quota_without_enable_returns_400_not_configured() {
        let provider = provider_with_ops_config(
            "provider-disabled",
            json!({"enabled": false, "group_id": "42"}),
        );
        let state = state_with_providers(vec![provider]);

        let (status, payload) = execute_sync_remote_quota(&state, "provider-disabled").await;

        assert_eq!(status, http::StatusCode::BAD_REQUEST);
        assert_eq!(payload["status"], json!("not_configured"));
        assert_eq!(payload["action_type"], json!("sync_remote_quota"));
        assert!(payload["message"]
            .as_str()
            .is_some_and(|message| message.contains("未启用远程配额同步")));
    }

    #[tokio::test]
    async fn sync_remote_quota_without_provider_ops_returns_400() {
        let provider = StoredProviderCatalogProvider::new(
            "provider-plain".to_string(),
            "Plain".to_string(),
            None,
            "custom".to_string(),
        )
        .expect("provider should build");
        let state = state_with_providers(vec![provider]);

        let (status, payload) = execute_sync_remote_quota(&state, "provider-plain").await;

        assert_eq!(status, http::StatusCode::BAD_REQUEST);
        assert_eq!(payload["status"], json!("not_configured"));
    }

    #[tokio::test]
    async fn sync_remote_quota_with_invalid_config_returns_400() {
        let provider = provider_with_ops_config(
            "provider-invalid",
            json!({"enabled": true, "fetch_interval_seconds": 0}),
        );
        let state = state_with_providers(vec![provider]);

        let (status, payload) = execute_sync_remote_quota(&state, "provider-invalid").await;

        assert_eq!(status, http::StatusCode::BAD_REQUEST);
        assert_eq!(payload["status"], json!("not_configured"));
        assert!(payload["message"]
            .as_str()
            .is_some_and(|message| message.contains("group_id")));
    }

    #[tokio::test]
    async fn sync_remote_quota_with_unknown_provider_returns_404() {
        let state = state_with_providers(vec![]);

        let (status, payload) = execute_sync_remote_quota(&state, "provider-missing").await;

        assert_eq!(status, http::StatusCode::NOT_FOUND);
        assert_eq!(payload["detail"], json!("Provider 不存在"));
    }
}
