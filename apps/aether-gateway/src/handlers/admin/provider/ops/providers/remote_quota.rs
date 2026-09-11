use super::config::{
    admin_provider_ops_credential_snapshot, persist_admin_provider_ops_runtime_credentials,
};
use super::verify::{
    admin_provider_ops_execute_json_request, admin_provider_ops_resolve_proxy_snapshot,
    admin_provider_ops_sub2api_exchange_token, admin_provider_ops_sub2api_request_url,
    AdminProviderOpsExecuteJsonError,
};
use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::ops::{
    admin_provider_ops_config_object, admin_provider_ops_connector_object,
    ADMIN_PROVIDER_OPS_USER_AGENT,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
use serde_json::{Map, Value};
use tracing::warn;

const SUB2API_SUBSCRIPTIONS_SUMMARY_ENDPOINT: &str = "/api/v1/subscriptions/summary";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminProviderOpsRemoteQuotaFetchError {
    NotConfigured,
    Auth,
    Transport,
}

impl AdminProviderOpsRemoteQuotaFetchError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::NotConfigured => "Provider Ops 未配置或凭据不可用",
            Self::Auth => "认证失败，请检查凭据配置",
            Self::Transport => "网络错误",
        }
    }
}

#[derive(Debug)]
pub(crate) struct AdminProviderOpsRemoteQuotaFetch {
    pub(crate) summary_json: Value,
    pub(crate) progress_json: Option<Value>,
}

enum FetchAttempt {
    Done(Box<AdminProviderOpsRemoteQuotaFetch>),
    AuthRejected,
    TransportFailed,
}

pub(crate) async fn admin_provider_ops_sub2api_remote_quota_fetch(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    progress_endpoint: &str,
) -> Result<AdminProviderOpsRemoteQuotaFetch, AdminProviderOpsRemoteQuotaFetchError> {
    let credential_snapshot = admin_provider_ops_credential_snapshot(state, provider)
        .await
        .map_err(|_| AdminProviderOpsRemoteQuotaFetchError::NotConfigured)?;
    let provider = credential_snapshot.provider.clone();
    let base_url = credential_snapshot
        .binding
        .destination
        .base_url()
        .to_string();
    let summary_url =
        admin_provider_ops_sub2api_request_url(&base_url, SUB2API_SUBSCRIPTIONS_SUMMARY_ENDPOINT)
            .map_err(|_| AdminProviderOpsRemoteQuotaFetchError::NotConfigured)?;
    let progress_url = admin_provider_ops_sub2api_request_url(&base_url, progress_endpoint)
        .map_err(|_| AdminProviderOpsRemoteQuotaFetchError::NotConfigured)?;
    let connector_config = admin_provider_ops_config_object(&provider)
        .and_then(admin_provider_ops_connector_object)
        .and_then(|connector| connector.get("config"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let proxy_snapshot =
        admin_provider_ops_resolve_proxy_snapshot(state, Some(&connector_config)).await;

    let mut credentials = credential_snapshot.credentials.clone();
    let mut access_token = exchange_and_persist_token(
        state,
        &provider,
        &base_url,
        &credentials,
        proxy_snapshot.as_ref(),
    )
    .await?;

    match fetch_remote_quota_payloads(
        state,
        &provider.id,
        &summary_url,
        &progress_url,
        &access_token,
        proxy_snapshot.as_ref(),
    )
    .await
    {
        FetchAttempt::Done(fetch) => Ok(*fetch),
        FetchAttempt::TransportFailed => Err(AdminProviderOpsRemoteQuotaFetchError::Transport),
        FetchAttempt::AuthRejected => {
            // 401/403：强制刷新一次访问令牌后重试；仍失败则 fail-closed。
            credentials.remove("_cached_access_token");
            credentials.remove("_cached_token_expires_at");
            access_token = exchange_and_persist_token(
                state,
                &provider,
                &base_url,
                &credentials,
                proxy_snapshot.as_ref(),
            )
            .await?;
            match fetch_remote_quota_payloads(
                state,
                &provider.id,
                &summary_url,
                &progress_url,
                &access_token,
                proxy_snapshot.as_ref(),
            )
            .await
            {
                FetchAttempt::Done(fetch) => Ok(*fetch),
                FetchAttempt::AuthRejected => Err(AdminProviderOpsRemoteQuotaFetchError::Auth),
                FetchAttempt::TransportFailed => {
                    Err(AdminProviderOpsRemoteQuotaFetchError::Transport)
                }
            }
        }
    }
}

async fn exchange_and_persist_token(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    base_url: &str,
    credentials: &Map<String, Value>,
    proxy_snapshot: Option<&aether_contracts::ProxySnapshot>,
) -> Result<String, AdminProviderOpsRemoteQuotaFetchError> {
    let (access_token, updated_credentials, _frontend_updated) =
        admin_provider_ops_sub2api_exchange_token(state, base_url, credentials, proxy_snapshot)
            .await
            .map_err(|_| AdminProviderOpsRemoteQuotaFetchError::Auth)?;
    if !updated_credentials.is_empty() {
        if let Err(err) =
            persist_admin_provider_ops_runtime_credentials(state, provider, &updated_credentials)
                .await
        {
            warn!(
                provider_id = %provider.id,
                error = ?err,
                "failed to persist sub2api remote quota runtime credentials"
            );
        }
    }
    Ok(access_token)
}

async fn fetch_remote_quota_payloads(
    state: &AdminAppState<'_>,
    provider_id: &str,
    summary_url: &str,
    progress_url: &str,
    access_token: &str,
    proxy_snapshot: Option<&aether_contracts::ProxySnapshot>,
) -> FetchAttempt {
    let auth_value = match reqwest::header::HeaderValue::from_str(&format!("Bearer {access_token}"))
    {
        Ok(value) => value,
        Err(_) => return FetchAttempt::AuthRejected,
    };
    let auth_headers = reqwest::header::HeaderMap::from_iter([
        (reqwest::header::AUTHORIZATION, auth_value),
        (
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static(ADMIN_PROVIDER_OPS_USER_AGENT),
        ),
        (
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("*/*"),
        ),
    ]);

    let summary_request_id = format!("provider-ops-remote-quota:summary:{provider_id}");
    let summary = match admin_provider_ops_execute_json_request(
        state,
        &summary_request_id,
        reqwest::Method::GET,
        summary_url,
        &auth_headers,
        None,
        proxy_snapshot,
    )
    .await
    {
        Ok((status, payload)) => match status {
            http::StatusCode::OK => payload,
            http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN => {
                return FetchAttempt::AuthRejected
            }
            _ => return FetchAttempt::TransportFailed,
        },
        Err(AdminProviderOpsExecuteJsonError::InvalidJson(_))
        | Err(AdminProviderOpsExecuteJsonError::Transport(_)) => {
            return FetchAttempt::TransportFailed
        }
    };

    let progress_request_id = format!("provider-ops-remote-quota:progress:{provider_id}");
    let progress = match admin_provider_ops_execute_json_request(
        state,
        &progress_request_id,
        reqwest::Method::GET,
        progress_url,
        &auth_headers,
        None,
        proxy_snapshot,
    )
    .await
    {
        Ok((status, payload)) => match status {
            http::StatusCode::OK => Some(payload),
            http::StatusCode::NOT_FOUND => None,
            http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN => {
                return FetchAttempt::AuthRejected
            }
            _ => return FetchAttempt::TransportFailed,
        },
        Err(AdminProviderOpsExecuteJsonError::InvalidJson(_))
        | Err(AdminProviderOpsExecuteJsonError::Transport(_)) => {
            return FetchAttempt::TransportFailed
        }
    };

    FetchAttempt::Done(Box::new(AdminProviderOpsRemoteQuotaFetch {
        summary_json: summary,
        progress_json: progress,
    }))
}
