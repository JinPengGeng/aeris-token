use std::collections::HashMap;

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use futures_util::stream::{self, StreamExt};
use tracing::{debug, warn};

use crate::admin_api::{admin_provider_ops_local_action_response, AdminAppState};
use crate::important_notification::{
    important_notification_dispatch_ready_for_item, send_important_notification_for_item,
    ImportantNotification, PROVIDER_POOL_ABNORMAL_ITEM_KEY,
};
use crate::{AppState, GatewayError};

use super::{system_config_bool, PROVIDER_CHECKIN_CONCURRENCY};

const PROVIDER_POOL_ABNORMAL_STATE_PREFIX: &str = "provider_ops:pool_abnormal:";
const PROVIDER_POOL_ABNORMAL_REPEAT_COOLDOWN_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderCheckinRunSummary {
    pub(crate) attempted: usize,
    pub(crate) succeeded: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderCheckinStatus {
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderCheckinOutcome {
    provider_id: String,
    status: ProviderCheckinStatus,
    message: String,
}

pub(crate) async fn perform_provider_checkin_once(
    state: &AppState,
) -> Result<ProviderCheckinRunSummary, GatewayError> {
    if !system_config_bool(&state.data, "enable_provider_checkin", true)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
    {
        return Ok(ProviderCheckinRunSummary {
            attempted: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
        });
    }

    let providers = state
        .list_provider_catalog_providers(true)
        .await?
        .into_iter()
        .filter(provider_has_ops_config)
        .collect::<Vec<_>>();
    if providers.is_empty() {
        return Ok(ProviderCheckinRunSummary {
            attempted: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
        });
    }

    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let provider_names = providers
        .iter()
        .map(|provider| (provider.id.clone(), provider.name.clone()))
        .collect::<HashMap<_, _>>();
    let mut endpoints_by_provider = HashMap::<String, Vec<StoredProviderCatalogEndpoint>>::new();
    for endpoint in state
        .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
        .await?
    {
        endpoints_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .push(endpoint);
    }

    let mut results = stream::iter(providers.into_iter().map(|provider| {
        let state = state.clone();
        let provider_id = provider.id.clone();
        let endpoints = endpoints_by_provider
            .remove(&provider_id)
            .unwrap_or_default();
        async move { run_provider_checkin_for_provider(&state, provider, endpoints).await }
    }))
    .buffer_unordered(PROVIDER_CHECKIN_CONCURRENCY);

    let mut summary = ProviderCheckinRunSummary {
        attempted: provider_ids.len(),
        succeeded: 0,
        failed: 0,
        skipped: 0,
    };
    while let Some(outcome) = results.next().await {
        match outcome.status {
            ProviderCheckinStatus::Succeeded => summary.succeeded += 1,
            ProviderCheckinStatus::Failed => {
                summary.failed += 1;
                warn!(
                    provider_id = %outcome.provider_id,
                    message = %outcome.message,
                    "gateway provider checkin failed"
                );
                if let Err(err) = maybe_notify_provider_pool_abnormal(
                    state,
                    &outcome.provider_id,
                    provider_names
                        .get(&outcome.provider_id)
                        .map(String::as_str)
                        .unwrap_or(&outcome.provider_id),
                    &outcome.message,
                )
                .await
                {
                    warn!(
                        error = %crate::error::redact_error_debug(&err),
                        provider_id = %outcome.provider_id,
                        "provider pool abnormal notification failed"
                    );
                }
            }
            ProviderCheckinStatus::Skipped => {
                summary.skipped += 1;
                debug!(
                    provider_id = %outcome.provider_id,
                    message = %outcome.message,
                    "gateway provider checkin skipped"
                );
            }
        }
    }

    Ok(summary)
}

fn provider_has_ops_config(provider: &StoredProviderCatalogProvider) -> bool {
    provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("provider_ops"))
        .and_then(serde_json::Value::as_object)
        .is_some_and(|config| !config.is_empty())
}

async fn run_provider_checkin_for_provider(
    state: &AppState,
    provider: StoredProviderCatalogProvider,
    endpoints: Vec<StoredProviderCatalogEndpoint>,
) -> ProviderCheckinOutcome {
    let provider_id = provider.id.clone();
    let admin_state = AdminAppState::new(state);
    let payload = admin_provider_ops_local_action_response(
        &admin_state,
        &provider_id,
        Some(&provider),
        &endpoints,
        "query_balance",
        None,
    )
    .await;
    provider_checkin_outcome_from_payload(&provider_id, &payload)
}

fn provider_checkin_outcome_from_payload(
    provider_id: &str,
    payload: &serde_json::Value,
) -> ProviderCheckinOutcome {
    let default_message = || "未执行签到".to_string();
    let payload_status = payload
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let payload_message = payload
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    let (status, message) = if payload_status == "success" {
        let extra = payload
            .get("data")
            .and_then(|value| value.get("extra"))
            .and_then(serde_json::Value::as_object);
        let checkin_message = extra
            .and_then(|extra| extra.get("checkin_message"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(default_message);
        match extra
            .and_then(|extra| extra.get("checkin_success"))
            .and_then(serde_json::Value::as_bool)
        {
            Some(true) => (ProviderCheckinStatus::Succeeded, checkin_message),
            Some(false) => (ProviderCheckinStatus::Failed, checkin_message),
            None => (ProviderCheckinStatus::Skipped, checkin_message),
        }
    } else if payload_status == "not_supported" {
        (
            ProviderCheckinStatus::Skipped,
            if payload_message.is_empty() {
                default_message()
            } else {
                payload_message
            },
        )
    } else {
        (
            ProviderCheckinStatus::Failed,
            if payload_message.is_empty() {
                "签到失败".to_string()
            } else {
                payload_message
            },
        )
    };

    ProviderCheckinOutcome {
        provider_id: provider_id.to_string(),
        status,
        message,
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ProviderPoolAbnormalRuntimeState {
    #[serde(default)]
    last_notified_at: Option<u64>,
}

/// Dispatches the administrator-facing `provider_pool_abnormal` notification
/// when a provider checkin keeps failing. A per-provider cooldown bounds
/// repeats while a provider stays unhealthy.
async fn maybe_notify_provider_pool_abnormal(
    state: &AppState,
    provider_id: &str,
    provider_name: &str,
    message: &str,
) -> Result<(), GatewayError> {
    if !important_notification_dispatch_ready_for_item(state, PROVIDER_POOL_ABNORMAL_ITEM_KEY)
        .await?
    {
        return Ok(());
    }
    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    let state_key = format!("{PROVIDER_POOL_ABNORMAL_STATE_PREFIX}{provider_id}");
    let previous = state
        .runtime_kv_get(&state_key)
        .await
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<ProviderPoolAbnormalRuntimeState>(&raw).ok());
    if previous
        .and_then(|state| state.last_notified_at)
        .is_some_and(|notified_at| {
            now_unix_secs.saturating_sub(notified_at) < PROVIDER_POOL_ABNORMAL_REPEAT_COOLDOWN_SECS
        })
    {
        return Ok(());
    }

    let report = send_important_notification_for_item(
        state,
        PROVIDER_POOL_ABNORMAL_ITEM_KEY,
        ImportantNotification {
            title: format!("号池异常：{provider_name}"),
            markdown_body: format!(
                "号池 `{provider_name}` 出现异常，请检查服务状态。\n\n检测详情：{message}"
            ),
            text_body: format!(
                "号池 {provider_name} 出现异常，请检查服务状态。检测详情：{message}"
            ),
        },
        &[
            ("provider_name", provider_name.to_string()),
            ("provider_id", provider_id.to_string()),
            ("message", message.to_string()),
        ],
    )
    .await?;
    if !report.success {
        return Ok(());
    }
    if let Ok(serialized) = serde_json::to_string(&ProviderPoolAbnormalRuntimeState {
        last_notified_at: Some(now_unix_secs),
    }) {
        if let Err(err) = state
            .runtime_state()
            .kv_set(&state_key, serialized, None)
            .await
        {
            warn!(
                error = %err,
                provider_id,
                "failed to write provider pool abnormal runtime state"
            );
        }
    }
    Ok(())
}
