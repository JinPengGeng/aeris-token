use std::time::Duration;

use aether_admin::provider::ops::{
    admin_provider_ops_config_object, parse_sub2api_remote_quota_at,
    parse_sub2api_remote_quota_config, Sub2ApiRemoteQuotaConfig, Sub2ApiRemoteQuotaState,
    Sub2ApiRemoteQuotaWindow, DEFAULT_REMOTE_QUOTA_FETCH_INTERVAL_SECS,
    MIN_REMOTE_QUOTA_FETCH_INTERVAL_SECS,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyHealthStateUpdate, ProviderCatalogKeyStatusSnapshotUpdate,
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_scheduler_core::provider_key_circuit_payload_is_active_open_at;
use futures_util::stream::{self, StreamExt};
use serde_json::{json, Value};
use tracing::{debug, warn};

use crate::admin_api::{
    admin_provider_ops_sub2api_remote_quota_fetch, AdminAppState,
    AdminProviderOpsRemoteQuotaFetchError,
};
use crate::handlers::shared::unix_secs_to_rfc3339;
use crate::{AppState, GatewayError};

use super::REMOTE_QUOTA_SYNC_CONCURRENCY;

const REMOTE_QUOTA_SYNC_LOCK_PREFIX: &str = "ap:remote_quota_sync:";
const REMOTE_QUOTA_SYNC_LOCK_TTL: Duration = Duration::from_secs(300);
const REMOTE_QUOTA_SYNC_CONSERVATIVE_COOLDOWN_SECS: u64 = 24 * 60 * 60;
const REMOTE_QUOTA_CIRCUIT_MAX_PROBE_INTERVAL_MINUTES: i32 = 24 * 60;
const REMOTE_QUOTA_CAS_MAX_ATTEMPTS: usize = 3;
const REMOTE_QUOTA_ERROR_MESSAGE_MAX_CHARS: usize = 200;
const REMOTE_QUOTA_SNAPSHOT_SOURCE: &str = "remote_quota_sync";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RemoteQuotaSyncRunSummary {
    pub(crate) attempted: usize,
    pub(crate) applied: usize,
    pub(crate) blocked: usize,
    pub(crate) recovered: usize,
    pub(crate) skipped: usize,
    pub(crate) failed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteQuotaSyncProviderOutcome {
    AppliedAvailable,
    AppliedBlocked,
    AppliedRecovered,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteQuotaBlockReason {
    WindowExhausted,
    SubscriptionInvalid,
}

impl RemoteQuotaBlockReason {
    fn circuit_reason(self) -> &'static str {
        match self {
            Self::WindowExhausted => "remote_quota_exhausted",
            Self::SubscriptionInvalid => "remote_quota_subscription_invalid",
        }
    }

    fn snapshot_reason(self) -> &'static str {
        match self {
            Self::WindowExhausted => "window_exhausted",
            Self::SubscriptionInvalid => "subscription_invalid",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteQuotaDecision {
    Available,
    Blocked {
        until_unix_secs: u64,
        reason: RemoteQuotaBlockReason,
        conservative: bool,
    },
}

impl RemoteQuotaDecision {
    fn blocked_until(self) -> Option<u64> {
        match self {
            Self::Available => None,
            Self::Blocked {
                until_unix_secs, ..
            } => Some(until_unix_secs),
        }
    }
}

enum RemoteQuotaConfigSelection {
    Disabled,
    Enabled(Box<Sub2ApiRemoteQuotaConfig>),
    Invalid(String),
}

fn remote_quota_sync_config(
    provider: &StoredProviderCatalogProvider,
) -> RemoteQuotaConfigSelection {
    let Some(provider_ops_config) = admin_provider_ops_config_object(provider) else {
        return RemoteQuotaConfigSelection::Disabled;
    };
    match parse_sub2api_remote_quota_config(provider_ops_config) {
        Ok(Some(config)) => RemoteQuotaConfigSelection::Enabled(Box::new(config)),
        Ok(None) => RemoteQuotaConfigSelection::Disabled,
        Err(message) => RemoteQuotaConfigSelection::Invalid(message),
    }
}

pub(crate) async fn remote_quota_sync_worker_interval(state: &AppState) -> Duration {
    let seconds = match state.list_provider_catalog_providers(true).await {
        Ok(providers) => providers
            .iter()
            .filter_map(|provider| match remote_quota_sync_config(provider) {
                RemoteQuotaConfigSelection::Enabled(config) => Some(config.fetch_interval_seconds),
                _ => None,
            })
            .min()
            .unwrap_or(DEFAULT_REMOTE_QUOTA_FETCH_INTERVAL_SECS),
        Err(_) => DEFAULT_REMOTE_QUOTA_FETCH_INTERVAL_SECS,
    };
    Duration::from_secs(seconds.max(MIN_REMOTE_QUOTA_FETCH_INTERVAL_SECS))
}

pub(crate) async fn perform_remote_quota_sync_once(
    state: &AppState,
) -> Result<RemoteQuotaSyncRunSummary, GatewayError> {
    let mut summary = RemoteQuotaSyncRunSummary::default();
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(summary);
    }

    let mut targets = Vec::new();
    for provider in state.list_provider_catalog_providers(true).await? {
        match remote_quota_sync_config(&provider) {
            RemoteQuotaConfigSelection::Disabled => {}
            RemoteQuotaConfigSelection::Enabled(config) => targets.push((provider, *config)),
            RemoteQuotaConfigSelection::Invalid(message) => {
                summary.attempted += 1;
                summary.failed += 1;
                warn!(
                    provider_id = %provider.id,
                    error = %message,
                    "provider remote quota config invalid; sync skipped"
                );
            }
        }
    }
    summary.attempted += targets.len();
    if targets.is_empty() {
        return Ok(summary);
    }

    let mut results = stream::iter(targets.into_iter().map(|(provider, config)| {
        let state = state.clone();
        async move { sync_remote_quota_for_provider(&state, provider, &config).await }
    }))
    .buffer_unordered(REMOTE_QUOTA_SYNC_CONCURRENCY);

    while let Some(outcome) = results.next().await {
        match outcome {
            RemoteQuotaSyncProviderOutcome::AppliedAvailable => summary.applied += 1,
            RemoteQuotaSyncProviderOutcome::AppliedBlocked => {
                summary.applied += 1;
                summary.blocked += 1;
            }
            RemoteQuotaSyncProviderOutcome::AppliedRecovered => {
                summary.applied += 1;
                summary.recovered += 1;
            }
            RemoteQuotaSyncProviderOutcome::Skipped => summary.skipped += 1,
            RemoteQuotaSyncProviderOutcome::Failed => summary.failed += 1,
        }
    }
    Ok(summary)
}

pub(crate) async fn perform_remote_quota_sync_for_provider(
    state: &AppState,
    provider_id: &str,
) -> Result<RemoteQuotaSyncProviderOutcome, GatewayError> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(RemoteQuotaSyncProviderOutcome::Skipped);
    }
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(RemoteQuotaSyncProviderOutcome::Skipped);
    };
    match remote_quota_sync_config(&provider) {
        RemoteQuotaConfigSelection::Disabled => Ok(RemoteQuotaSyncProviderOutcome::Skipped),
        RemoteQuotaConfigSelection::Invalid(message) => {
            warn!(
                provider_id = %provider.id,
                error = %message,
                "provider remote quota config invalid; sync skipped"
            );
            Ok(RemoteQuotaSyncProviderOutcome::Failed)
        }
        RemoteQuotaConfigSelection::Enabled(config) => {
            Ok(sync_remote_quota_for_provider(state, provider, &config).await)
        }
    }
}

async fn sync_remote_quota_for_provider(
    state: &AppState,
    provider: StoredProviderCatalogProvider,
    config: &Sub2ApiRemoteQuotaConfig,
) -> RemoteQuotaSyncProviderOutcome {
    let provider_id = provider.id.clone();
    let lock = acquire_remote_quota_sync_lock(state, &provider_id).await;
    let Some(lock) = lock else {
        return RemoteQuotaSyncProviderOutcome::Skipped;
    };
    let outcome = sync_remote_quota_for_provider_locked(state, &provider, config).await;
    if let Err(err) = state.runtime_state().lock_release(&lock).await {
        debug!(
            provider_id = %provider_id,
            error = %err,
            "failed to release remote quota sync lock"
        );
    }
    outcome
}

async fn acquire_remote_quota_sync_lock(
    state: &AppState,
    provider_id: &str,
) -> Option<aether_runtime_state::RuntimeLockLease> {
    let owner = format!("aether-gateway-remote-quota-sync-{}", std::process::id());
    match state
        .runtime_state()
        .lock_try_acquire(
            &format!("{REMOTE_QUOTA_SYNC_LOCK_PREFIX}{provider_id}"),
            &owner,
            REMOTE_QUOTA_SYNC_LOCK_TTL,
        )
        .await
    {
        Ok(lease) => lease,
        Err(err) => {
            debug!(
                provider_id = %provider_id,
                error = %err,
                "failed to acquire remote quota sync lock"
            );
            None
        }
    }
}

async fn sync_remote_quota_for_provider_locked(
    state: &AppState,
    provider: &StoredProviderCatalogProvider,
    config: &Sub2ApiRemoteQuotaConfig,
) -> RemoteQuotaSyncProviderOutcome {
    let provider_id = provider.id.clone();
    let now_unix_secs = now_unix_secs();
    let admin_state = AdminAppState::new(state);
    let fetch = match admin_provider_ops_sub2api_remote_quota_fetch(
        &admin_state,
        provider,
        &config.progress_endpoint,
    )
    .await
    {
        Ok(fetch) => fetch,
        Err(err) => {
            let message = match err {
                AdminProviderOpsRemoteQuotaFetchError::NotConfigured => {
                    "provider_ops_not_configured"
                }
                AdminProviderOpsRemoteQuotaFetchError::Auth => "auth_failed",
                AdminProviderOpsRemoteQuotaFetchError::Transport => "network_error",
            };
            warn!(
                provider_id = %provider_id,
                error_kind = message,
                error_message = err.message(),
                "provider remote quota fetch failed; keeping last synced state"
            );
            record_remote_quota_sync_failure(state, provider, config, message).await;
            return RemoteQuotaSyncProviderOutcome::Failed;
        }
    };

    let quota_state = match parse_sub2api_remote_quota_at(
        &fetch.summary_json,
        fetch.progress_json.as_ref(),
        &config.group_id,
        now_unix_secs,
    ) {
        Ok(quota_state) => quota_state,
        Err(message) => {
            warn!(
                provider_id = %provider_id,
                error = %message,
                "provider remote quota response rejected; keeping last synced state"
            );
            record_remote_quota_sync_failure(state, provider, config, &message).await;
            return RemoteQuotaSyncProviderOutcome::Failed;
        }
    };
    let decision = remote_quota_decision(&quota_state, now_unix_secs);

    let endpoints = match state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
        .await
    {
        Ok(endpoints) => endpoints,
        Err(err) => {
            warn!(
                provider_id = %provider_id,
                error = ?err,
                "provider remote quota sync failed to load endpoints"
            );
            return RemoteQuotaSyncProviderOutcome::Failed;
        }
    };
    let keys = match state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider_id))
        .await
    {
        Ok(keys) => keys,
        Err(err) => {
            warn!(
                provider_id = %provider_id,
                error = ?err,
                "provider remote quota sync failed to load keys"
            );
            return RemoteQuotaSyncProviderOutcome::Failed;
        }
    };
    let endpoint_api_formats = crate::provider_key_auth::provider_active_api_formats(&endpoints);

    let mut managed_circuit_cleared = false;
    for key in &keys {
        if let Some(true) = apply_remote_quota_decision_to_key(
            state,
            key,
            &endpoint_api_formats,
            &decision,
            now_unix_secs,
        )
        .await
        {
            managed_circuit_cleared = true;
        }
    }

    let quota_payload =
        build_remote_quota_synced_quota_snapshot(&quota_state, &decision, config, now_unix_secs);
    for key in &keys {
        persist_key_quota_snapshot(state, &key.id, quota_payload.clone()).await;
    }

    match decision {
        RemoteQuotaDecision::Available => {
            if managed_circuit_cleared {
                RemoteQuotaSyncProviderOutcome::AppliedRecovered
            } else {
                RemoteQuotaSyncProviderOutcome::AppliedAvailable
            }
        }
        RemoteQuotaDecision::Blocked { .. } => RemoteQuotaSyncProviderOutcome::AppliedBlocked,
    }
}

pub(crate) fn remote_quota_decision(
    quota_state: &Sub2ApiRemoteQuotaState,
    now_unix_secs: u64,
) -> RemoteQuotaDecision {
    match quota_state {
        Sub2ApiRemoteQuotaState::SubscriptionInvalid {
            expires_at_unix_secs,
            ..
        } => match expires_at_unix_secs.filter(|expires_at| *expires_at > now_unix_secs) {
            Some(until_unix_secs) => RemoteQuotaDecision::Blocked {
                until_unix_secs,
                reason: RemoteQuotaBlockReason::SubscriptionInvalid,
                conservative: false,
            },
            None => RemoteQuotaDecision::Blocked {
                until_unix_secs: now_unix_secs
                    .saturating_add(REMOTE_QUOTA_SYNC_CONSERVATIVE_COOLDOWN_SECS),
                reason: RemoteQuotaBlockReason::SubscriptionInvalid,
                conservative: true,
            },
        },
        Sub2ApiRemoteQuotaState::Active(observation) => {
            let until_unix_secs = observation
                .exhausted_windows_at(now_unix_secs)
                .map(|window| window.resets_at_unix_secs)
                .max();
            match until_unix_secs {
                Some(until_unix_secs) => RemoteQuotaDecision::Blocked {
                    until_unix_secs,
                    reason: RemoteQuotaBlockReason::WindowExhausted,
                    conservative: false,
                },
                None => RemoteQuotaDecision::Available,
            }
        }
    }
}

fn key_remote_quota_api_formats(
    key: &StoredProviderCatalogKey,
    endpoint_api_formats: &[String],
) -> Vec<String> {
    let configured = crate::provider_key_auth::provider_key_configured_api_formats(key);
    if configured.is_empty() {
        endpoint_api_formats.to_vec()
    } else {
        configured
    }
}

/// 返回 Some(true)=解除了本 worker 熔断，Some(false)=写入/更新了熔断，None=无变化或失败。
async fn apply_remote_quota_decision_to_key(
    state: &AppState,
    key: &StoredProviderCatalogKey,
    endpoint_api_formats: &[String],
    decision: &RemoteQuotaDecision,
    now_unix_secs: u64,
) -> Option<bool> {
    let api_formats = key_remote_quota_api_formats(key, endpoint_api_formats);
    if api_formats.is_empty() {
        return None;
    }
    let mut current = key.clone();
    for attempt in 0..REMOTE_QUOTA_CAS_MAX_ATTEMPTS {
        let Some(next_circuit) = merge_remote_quota_circuit_breaker(
            current.circuit_breaker_by_format.as_ref(),
            &api_formats,
            decision,
            now_unix_secs,
            current.max_probe_interval_minutes,
        ) else {
            return None;
        };
        let cleared_managed = matches!(decision, RemoteQuotaDecision::Available);
        match state
            .compare_and_update_provider_catalog_key_health_state(
                &ProviderCatalogKeyHealthStateUpdate {
                    key_id: current.id.clone(),
                    expected_encrypted_auth_config: None,
                    expected_health_by_format: current.health_by_format.clone(),
                    expected_circuit_breaker_by_format: current.circuit_breaker_by_format.clone(),
                    health_by_format: current.health_by_format.clone(),
                    circuit_breaker_by_format: Some(next_circuit.clone()),
                },
            )
            .await
        {
            Ok(true) => return Some(cleared_managed),
            Ok(false) => {
                if attempt + 1 >= REMOTE_QUOTA_CAS_MAX_ATTEMPTS {
                    warn!(
                        key_id = %current.id,
                        "provider remote quota circuit CAS conflict; giving up"
                    );
                    return None;
                }
                match state
                    .list_provider_catalog_keys_by_ids_strong(std::slice::from_ref(&current.id))
                    .await
                {
                    Ok(keys) if keys.len() == 1 => {
                        current = keys.into_iter().next().expect("length checked");
                    }
                    Ok(_) => return None,
                    Err(err) => {
                        warn!(
                            key_id = %current.id,
                            error = ?err,
                            "provider remote quota circuit reload failed"
                        );
                        return None;
                    }
                }
            }
            Err(err) => {
                warn!(
                    key_id = %current.id,
                    error = ?err,
                    "provider remote quota circuit write failed"
                );
                return None;
            }
        }
    }
    None
}

fn merge_remote_quota_circuit_breaker(
    current_circuit_by_format: Option<&Value>,
    api_formats: &[String],
    decision: &RemoteQuotaDecision,
    now_unix_secs: u64,
    max_probe_interval_minutes: i32,
) -> Option<Value> {
    let mut circuit_by_format = current_circuit_by_format
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut changed = false;
    for api_format in api_formats {
        let current_payload = circuit_by_format.get(api_format);
        let remote_quota_managed = current_payload
            .and_then(|payload| payload.get("remote_quota_managed"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match decision {
            RemoteQuotaDecision::Blocked {
                until_unix_secs,
                reason,
                ..
            } => {
                let active_open = current_payload.is_some_and(|payload| {
                    provider_key_circuit_payload_is_active_open_at(payload, now_unix_secs)
                });
                // 其他机制（如上游 429）持有的活跃熔断不覆盖、也不接管。
                if active_open && !remote_quota_managed {
                    continue;
                }
                if current_payload.is_some_and(|payload| {
                    remote_quota_circuit_payload_matches(payload, *reason, *until_unix_secs)
                }) {
                    continue;
                }
                circuit_by_format.insert(
                    api_format.clone(),
                    build_remote_quota_circuit_open_payload(
                        current_payload,
                        *reason,
                        *until_unix_secs,
                        now_unix_secs,
                        max_probe_interval_minutes,
                    ),
                );
                changed = true;
            }
            RemoteQuotaDecision::Available => {
                // 恢复只解除本 worker 写入的熔断。
                if !remote_quota_managed {
                    continue;
                }
                let Some(current) = current_payload.and_then(Value::as_object).cloned() else {
                    continue;
                };
                if !current
                    .get("open")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    continue;
                }
                let mut next = current;
                next.insert("open".to_string(), Value::Bool(false));
                circuit_by_format.insert(api_format.clone(), Value::Object(next));
                changed = true;
            }
        }
    }
    changed.then_some(Value::Object(circuit_by_format))
}

fn remote_quota_circuit_payload_matches(
    payload: &Value,
    reason: RemoteQuotaBlockReason,
    until_unix_secs: u64,
) -> bool {
    payload.get("remote_quota_managed").and_then(Value::as_bool) == Some(true)
        && payload.get("open").and_then(Value::as_bool) == Some(true)
        && payload.get("reason").and_then(Value::as_str) == Some(reason.circuit_reason())
        && payload
            .get("next_probe_at_unix_secs")
            .and_then(Value::as_u64)
            == Some(until_unix_secs)
}

fn build_remote_quota_circuit_open_payload(
    current_payload: Option<&Value>,
    reason: RemoteQuotaBlockReason,
    until_unix_secs: u64,
    now_unix_secs: u64,
    max_probe_interval_minutes: i32,
) -> Value {
    let current = current_payload.and_then(Value::as_object);
    let was_open = current
        .and_then(|payload| payload.get("open"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let open_at = current
        .and_then(|payload| payload.get("open_at"))
        .filter(|_| was_open)
        .cloned()
        .unwrap_or_else(|| json!(unix_secs_to_rfc3339(now_unix_secs)));
    let request_results_window = current
        .and_then(|payload| payload.get("request_results_window"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let probe_interval_minutes = until_unix_secs
        .saturating_sub(now_unix_secs)
        .saturating_add(59)
        / 60;
    json!({
        "open": true,
        "open_at": open_at,
        "reason": reason.circuit_reason(),
        "next_probe_at": unix_secs_to_rfc3339(until_unix_secs),
        "next_probe_at_unix_secs": until_unix_secs,
        "probe_interval_minutes": probe_interval_minutes.max(1),
        "max_probe_interval_minutes": max_probe_interval_minutes
            .clamp(0, REMOTE_QUOTA_CIRCUIT_MAX_PROBE_INTERVAL_MINUTES),
        "last_failure_at": unix_secs_to_rfc3339(now_unix_secs),
        "last_probe_failure_at": Value::Null,
        "half_open_until": Value::Null,
        "half_open_successes": 0,
        "half_open_failures": 0,
        "request_results_window": request_results_window,
        "remote_quota_managed": true,
        "remote_quota_blocked_until_unix_secs": until_unix_secs,
    })
}

fn build_remote_quota_synced_quota_snapshot(
    quota_state: &Sub2ApiRemoteQuotaState,
    decision: &RemoteQuotaDecision,
    config: &Sub2ApiRemoteQuotaConfig,
    now_unix_secs: u64,
) -> Value {
    let blocked_until = decision.blocked_until();
    let (windows, usage_ratio, subscription_id, subscription_status, expires_at_unix_secs) =
        match quota_state {
            Sub2ApiRemoteQuotaState::Active(observation) => (
                observation
                    .windows
                    .iter()
                    .map(|window| build_remote_quota_window_snapshot(window, now_unix_secs))
                    .collect::<Vec<_>>(),
                observation.max_usage_ratio(),
                Some(observation.subscription_id.clone()),
                Some(observation.status.clone()),
                observation.expires_at_unix_secs,
            ),
            Sub2ApiRemoteQuotaState::SubscriptionInvalid {
                subscription_status,
                expires_at_unix_secs,
                ..
            } => (
                Vec::new(),
                None,
                None,
                subscription_status.clone(),
                *expires_at_unix_secs,
            ),
        };
    let exhausted = blocked_until.is_some();
    let reset_at = blocked_until.or_else(|| match quota_state {
        Sub2ApiRemoteQuotaState::Active(observation) => observation
            .windows
            .iter()
            .map(|window| window.resets_at_unix_secs)
            .min(),
        Sub2ApiRemoteQuotaState::SubscriptionInvalid { .. } => None,
    });
    let group_name = match quota_state {
        Sub2ApiRemoteQuotaState::Active(observation) if !observation.group_name.is_empty() => {
            Some(observation.group_name.clone())
        }
        _ => None,
    };
    let (block_reason, conservative) = match decision {
        RemoteQuotaDecision::Available => (None, false),
        RemoteQuotaDecision::Blocked {
            reason,
            conservative,
            ..
        } => (Some(reason.snapshot_reason()), *conservative),
    };
    let reason_text = match decision {
        RemoteQuotaDecision::Available => Value::Null,
        RemoteQuotaDecision::Blocked {
            reason: RemoteQuotaBlockReason::WindowExhausted,
            ..
        } => json!("远程配额窗口已耗尽"),
        RemoteQuotaDecision::Blocked {
            reason: RemoteQuotaBlockReason::SubscriptionInvalid,
            ..
        } => json!("远程订阅已失效或分组不可用"),
    };

    json!({
        "version": 2,
        "provider_type": "sub2api",
        "code": if exhausted { "exhausted" } else { "ok" },
        "label": if exhausted { Some("额度耗尽") } else { None::<&str> },
        "reason": reason_text,
        "freshness": "fresh",
        "source": REMOTE_QUOTA_SNAPSHOT_SOURCE,
        "observed_at": now_unix_secs,
        "exhausted": exhausted,
        "usage_ratio": usage_ratio,
        "updated_at": now_unix_secs,
        "reset_at": reset_at,
        "reset_seconds": reset_at.map(|reset_at| reset_at.saturating_sub(now_unix_secs)),
        "plan_type": group_name,
        "windows": windows,
        "remote_quota": {
            "sync_status": "ok",
            "synced_at_unix_secs": now_unix_secs,
            "source": REMOTE_QUOTA_SNAPSHOT_SOURCE,
            "group_id": config.group_id,
            "group_name": group_name,
            "subscription_id": subscription_id,
            "subscription_status": subscription_status,
            "subscription_active": matches!(quota_state, Sub2ApiRemoteQuotaState::Active(_)),
            "expires_at_unix_secs": expires_at_unix_secs,
            "blocked": exhausted,
            "block_reason": block_reason,
            "blocked_until_unix_secs": blocked_until,
            "conservative_cooldown": conservative,
            "last_error": Value::Null,
        },
    })
}

fn build_remote_quota_window_snapshot(
    window: &Sub2ApiRemoteQuotaWindow,
    now_unix_secs: u64,
) -> Value {
    let used_ratio = window.used_ratio();
    json!({
        "code": window.kind.as_str(),
        "label": window.kind.display_name_zh(),
        "scope": "account",
        "unit": "usd",
        "used_ratio": used_ratio,
        "remaining_ratio": (1.0 - used_ratio).max(0.0),
        "used_value": window.used_usd,
        "remaining_value": (window.limit_usd - window.used_usd).max(0.0),
        "limit_value": window.limit_usd,
        "reset_at": window.resets_at_unix_secs,
        "reset_seconds": window.resets_at_unix_secs.saturating_sub(now_unix_secs),
        "is_exhausted": window.is_exhausted_at(now_unix_secs),
    })
}

fn build_remote_quota_error_quota_snapshot(
    current_status_snapshot: Option<&Value>,
    config: &Sub2ApiRemoteQuotaConfig,
    error_message: &str,
    now_unix_secs: u64,
) -> Value {
    let mut quota = current_status_snapshot
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    quota
        .entry("version".to_string())
        .or_insert_with(|| json!(2));
    quota
        .entry("provider_type".to_string())
        .or_insert_with(|| json!("sub2api"));
    quota
        .entry("code".to_string())
        .or_insert_with(|| json!("unknown"));
    quota
        .entry("exhausted".to_string())
        .or_insert_with(|| json!(false));
    quota
        .entry("source".to_string())
        .or_insert_with(|| json!(REMOTE_QUOTA_SNAPSHOT_SOURCE));

    let mut remote_quota = quota
        .get("remote_quota")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    remote_quota.insert("sync_status".to_string(), json!("error"));
    remote_quota.insert(
        "last_error".to_string(),
        json!(sanitize_remote_quota_error_message(error_message)),
    );
    remote_quota.insert("last_error_at_unix_secs".to_string(), json!(now_unix_secs));
    remote_quota
        .entry("group_id".to_string())
        .or_insert_with(|| json!(config.group_id));
    remote_quota
        .entry("source".to_string())
        .or_insert_with(|| json!(REMOTE_QUOTA_SNAPSHOT_SOURCE));
    quota.insert("remote_quota".to_string(), Value::Object(remote_quota));
    Value::Object(quota)
}

fn sanitize_remote_quota_error_message(message: &str) -> String {
    message
        .chars()
        .take(REMOTE_QUOTA_ERROR_MESSAGE_MAX_CHARS)
        .collect()
}

async fn record_remote_quota_sync_failure(
    state: &AppState,
    provider: &StoredProviderCatalogProvider,
    config: &Sub2ApiRemoteQuotaConfig,
    error_message: &str,
) {
    let provider_id = &provider.id;
    let keys = match state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(provider_id))
        .await
    {
        Ok(keys) => keys,
        Err(err) => {
            warn!(
                provider_id = %provider_id,
                error = ?err,
                "provider remote quota sync failure snapshot skipped: cannot load keys"
            );
            return;
        }
    };
    let now_unix_secs = now_unix_secs();
    for key in &keys {
        let quota_payload = build_remote_quota_error_quota_snapshot(
            key.status_snapshot.as_ref(),
            config,
            error_message,
            now_unix_secs,
        );
        persist_key_quota_snapshot(state, &key.id, quota_payload).await;
    }
}

async fn persist_key_quota_snapshot(state: &AppState, key_id: &str, quota_payload: Value) {
    let patch = json!({ "quota": quota_payload });
    match state
        .update_provider_catalog_key_status_snapshot(&ProviderCatalogKeyStatusSnapshotUpdate {
            key_id: key_id.to_string(),
            status_snapshot_patch: patch,
            updated_at_unix_secs: None,
        })
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            debug!(
                key_id = %key_id,
                "provider remote quota status snapshot write skipped: key missing"
            );
        }
        Err(err) => {
            warn!(
                key_id = %key_id,
                error = ?err,
                "provider remote quota status snapshot write failed"
            );
        }
    }
}

fn now_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::{
        build_remote_quota_circuit_open_payload, build_remote_quota_error_quota_snapshot,
        build_remote_quota_synced_quota_snapshot, merge_remote_quota_circuit_breaker,
        remote_quota_decision, RemoteQuotaBlockReason, RemoteQuotaDecision,
        REMOTE_QUOTA_SYNC_CONSERVATIVE_COOLDOWN_SECS,
    };
    use aether_admin::provider::ops::{
        Sub2ApiQuotaWindowKind, Sub2ApiRemoteQuotaConfig, Sub2ApiRemoteQuotaObservation,
        Sub2ApiRemoteQuotaState, Sub2ApiRemoteQuotaWindow,
    };
    use aether_scheduler_core::provider_key_circuit_payload_is_active_open_at;
    use serde_json::{json, Value};

    const NOW: u64 = 1_896_004_800;

    fn config() -> Sub2ApiRemoteQuotaConfig {
        Sub2ApiRemoteQuotaConfig {
            group_id: "42".to_string(),
            progress_endpoint: "/api/v1/subscriptions/progress".to_string(),
            fetch_interval_seconds: 300,
        }
    }

    fn window(used: f64, limit: f64, resets_at: u64) -> Sub2ApiRemoteQuotaWindow {
        Sub2ApiRemoteQuotaWindow {
            kind: Sub2ApiQuotaWindowKind::Monthly,
            limit_usd: limit,
            used_usd: used,
            window_start_unix_secs: NOW - 86_400,
            resets_at_unix_secs: resets_at,
        }
    }

    fn active_state(windows: Vec<Sub2ApiRemoteQuotaWindow>) -> Sub2ApiRemoteQuotaState {
        Sub2ApiRemoteQuotaState::Active(Sub2ApiRemoteQuotaObservation {
            subscription_id: "9".to_string(),
            group_id: "42".to_string(),
            group_name: "Pro".to_string(),
            status: "active".to_string(),
            expires_at_unix_secs: Some(NOW + 86_400),
            windows,
        })
    }

    #[test]
    fn decision_blocks_until_exhausted_window_reset() {
        let state = active_state(vec![
            window(100.0, 100.0, NOW + 3_600),
            Sub2ApiRemoteQuotaWindow {
                kind: Sub2ApiQuotaWindowKind::Weekly,
                ..window(50.0, 50.0, NOW + 7_200)
            },
        ]);
        assert_eq!(
            remote_quota_decision(&state, NOW),
            RemoteQuotaDecision::Blocked {
                until_unix_secs: NOW + 7_200,
                reason: RemoteQuotaBlockReason::WindowExhausted,
                conservative: false,
            }
        );
    }

    #[test]
    fn decision_ignores_window_exhausted_after_reset() {
        let state = active_state(vec![window(100.0, 100.0, NOW - 1)]);
        assert_eq!(
            remote_quota_decision(&state, NOW),
            RemoteQuotaDecision::Available
        );
    }

    #[test]
    fn decision_blocks_invalid_subscription_until_expiry_or_conservative_fallback() {
        let state = Sub2ApiRemoteQuotaState::SubscriptionInvalid {
            group_id: "42".to_string(),
            subscription_status: Some("expired".to_string()),
            expires_at_unix_secs: Some(NOW + 10_000),
        };
        assert_eq!(
            remote_quota_decision(&state, NOW),
            RemoteQuotaDecision::Blocked {
                until_unix_secs: NOW + 10_000,
                reason: RemoteQuotaBlockReason::SubscriptionInvalid,
                conservative: false,
            }
        );

        let state = Sub2ApiRemoteQuotaState::SubscriptionInvalid {
            group_id: "42".to_string(),
            subscription_status: None,
            expires_at_unix_secs: None,
        };
        assert_eq!(
            remote_quota_decision(&state, NOW),
            RemoteQuotaDecision::Blocked {
                until_unix_secs: NOW + REMOTE_QUOTA_SYNC_CONSERVATIVE_COOLDOWN_SECS,
                reason: RemoteQuotaBlockReason::SubscriptionInvalid,
                conservative: true,
            }
        );
    }

    #[test]
    fn exhausted_decision_writes_managed_circuit_until_reset() {
        let decision = RemoteQuotaDecision::Blocked {
            until_unix_secs: NOW + 3_600,
            reason: RemoteQuotaBlockReason::WindowExhausted,
            conservative: false,
        };
        let merged = merge_remote_quota_circuit_breaker(
            None,
            &["openai:chat".to_string()],
            &decision,
            NOW,
            32,
        )
        .expect("circuit should be written");
        let payload = &merged["openai:chat"];
        assert_eq!(payload["open"], json!(true));
        assert_eq!(payload["remote_quota_managed"], json!(true));
        assert_eq!(payload["reason"], json!("remote_quota_exhausted"));
        assert_eq!(payload["next_probe_at_unix_secs"], json!(NOW + 3_600));
        assert!(provider_key_circuit_payload_is_active_open_at(payload, NOW));
        assert!(!provider_key_circuit_payload_is_active_open_at(
            payload,
            NOW + 3_600
        ));

        // 相同决策再次合并：无变化，避免无意义 CAS 写入。
        let unchanged = merge_remote_quota_circuit_breaker(
            Some(&merged),
            &["openai:chat".to_string()],
            &decision,
            NOW + 60,
            32,
        );
        assert!(unchanged.is_none());
    }

    #[test]
    fn blocked_merge_does_not_override_foreign_active_circuit() {
        let foreign = json!({
            "openai:chat": {
                "open": true,
                "reason": "upstream_429",
                "next_probe_at_unix_secs": NOW + 1_000,
            }
        });
        let decision = RemoteQuotaDecision::Blocked {
            until_unix_secs: NOW + 3_600,
            reason: RemoteQuotaBlockReason::WindowExhausted,
            conservative: false,
        };
        let merged = merge_remote_quota_circuit_breaker(
            Some(&foreign),
            &["openai:chat".to_string()],
            &decision,
            NOW,
            32,
        );
        assert!(merged.is_none());

        // 外来熔断已过期（冷却结束）→ 可以接管写入远程配额熔断。
        let expired_foreign = json!({
            "openai:chat": {
                "open": true,
                "reason": "upstream_429",
                "next_probe_at_unix_secs": NOW - 1,
            }
        });
        let merged = merge_remote_quota_circuit_breaker(
            Some(&expired_foreign),
            &["openai:chat".to_string()],
            &decision,
            NOW,
            32,
        )
        .expect("expired foreign circuit should be replaced");
        assert_eq!(
            merged["openai:chat"]["reason"],
            json!("remote_quota_exhausted")
        );
    }

    #[test]
    fn recovery_clears_only_remote_quota_managed_circuits() {
        let current = json!({
            "openai:chat": {
                "open": true,
                "reason": "remote_quota_exhausted",
                "next_probe_at_unix_secs": NOW + 3_600,
                "remote_quota_managed": true
            },
            "openai:responses": {
                "open": true,
                "reason": "upstream_429",
                "next_probe_at_unix_secs": NOW + 3_600
            }
        });
        let merged = merge_remote_quota_circuit_breaker(
            Some(&current),
            &["openai:chat".to_string(), "openai:responses".to_string()],
            &RemoteQuotaDecision::Available,
            NOW,
            32,
        )
        .expect("managed circuit should be cleared");
        assert_eq!(merged["openai:chat"]["open"], json!(false));
        assert_eq!(merged["openai:chat"]["remote_quota_managed"], json!(true));
        assert_eq!(merged["openai:responses"]["open"], json!(true));
        assert!(provider_key_circuit_payload_is_active_open_at(
            &merged["openai:responses"],
            NOW
        ));
        assert!(!provider_key_circuit_payload_is_active_open_at(
            &merged["openai:chat"],
            NOW
        ));
    }

    #[test]
    fn circuit_payload_preserves_open_at_and_result_window() {
        let current = json!({
            "open": true,
            "open_at": "2030-01-01T00:00:00Z",
            "reason": "remote_quota_exhausted",
            "next_probe_at_unix_secs": NOW + 100,
            "remote_quota_managed": true,
            "request_results_window": [{"at": 1, "ok": false}]
        });
        let payload = build_remote_quota_circuit_open_payload(
            Some(&current),
            RemoteQuotaBlockReason::WindowExhausted,
            NOW + 3_600,
            NOW,
            32,
        );
        assert_eq!(payload["open_at"], json!("2030-01-01T00:00:00Z"));
        assert_eq!(
            payload["request_results_window"],
            json!([{"at": 1, "ok": false}])
        );
        assert_eq!(payload["probe_interval_minutes"], json!(60));
    }

    #[test]
    fn synced_snapshot_matches_quota_schema() {
        let state = active_state(vec![window(100.0, 100.0, NOW + 3_600)]);
        let decision = remote_quota_decision(&state, NOW);
        let payload = build_remote_quota_synced_quota_snapshot(&state, &decision, &config(), NOW);
        assert_eq!(payload["version"], json!(2));
        assert_eq!(payload["provider_type"], json!("sub2api"));
        assert_eq!(payload["code"], json!("exhausted"));
        assert_eq!(payload["exhausted"], json!(true));
        assert_eq!(payload["source"], json!("remote_quota_sync"));
        assert_eq!(payload["reset_at"], json!(NOW + 3_600));
        assert_eq!(payload["reset_seconds"], json!(3_600));
        assert_eq!(payload["plan_type"], json!("Pro"));
        assert_eq!(payload["usage_ratio"], json!(1.0));
        let window = &payload["windows"][0];
        assert_eq!(window["code"], json!("monthly"));
        assert_eq!(window["unit"], json!("usd"));
        assert_eq!(window["used_value"], json!(100.0));
        assert_eq!(window["limit_value"], json!(100.0));
        assert_eq!(window["reset_at"], json!(NOW + 3_600));
        assert_eq!(window["is_exhausted"], json!(true));
        let remote_quota = &payload["remote_quota"];
        assert_eq!(remote_quota["sync_status"], json!("ok"));
        assert_eq!(remote_quota["group_id"], json!("42"));
        assert_eq!(remote_quota["blocked"], json!(true));
        assert_eq!(remote_quota["block_reason"], json!("window_exhausted"));
        assert_eq!(remote_quota["blocked_until_unix_secs"], json!(NOW + 3_600));
    }

    #[test]
    fn synced_snapshot_for_invalid_subscription_marks_conservative_cooldown() {
        let state = Sub2ApiRemoteQuotaState::SubscriptionInvalid {
            group_id: "42".to_string(),
            subscription_status: Some("disabled".to_string()),
            expires_at_unix_secs: None,
        };
        let decision = remote_quota_decision(&state, NOW);
        let payload = build_remote_quota_synced_quota_snapshot(&state, &decision, &config(), NOW);
        assert_eq!(payload["code"], json!("exhausted"));
        let remote_quota = &payload["remote_quota"];
        assert_eq!(remote_quota["subscription_active"], json!(false));
        assert_eq!(remote_quota["block_reason"], json!("subscription_invalid"));
        assert_eq!(remote_quota["conservative_cooldown"], json!(true));
        assert_eq!(
            remote_quota["blocked_until_unix_secs"],
            json!(NOW + REMOTE_QUOTA_SYNC_CONSERVATIVE_COOLDOWN_SECS)
        );
    }

    #[test]
    fn error_snapshot_preserves_previous_values() {
        let previous = json!({
            "quota": {
                "version": 2,
                "provider_type": "sub2api",
                "code": "exhausted",
                "exhausted": true,
                "usage_ratio": 1.0,
                "windows": [{"code": "monthly", "used_value": 100.0}],
                "remote_quota": {
                    "sync_status": "ok",
                    "synced_at_unix_secs": NOW - 300,
                    "group_id": "42",
                    "blocked": true
                }
            },
            "oauth": {"code": "none"}
        });
        let payload = build_remote_quota_error_quota_snapshot(
            Some(&previous),
            &config(),
            "network_error",
            NOW,
        );
        assert_eq!(payload["code"], json!("exhausted"));
        assert_eq!(payload["exhausted"], json!(true));
        assert_eq!(payload["usage_ratio"], json!(1.0));
        assert_eq!(
            payload["windows"],
            json!([{"code": "monthly", "used_value": 100.0}])
        );
        let remote_quota = &payload["remote_quota"];
        assert_eq!(remote_quota["sync_status"], json!("error"));
        assert_eq!(remote_quota["last_error"], json!("network_error"));
        assert_eq!(remote_quota["last_error_at_unix_secs"], json!(NOW));
        assert_eq!(remote_quota["synced_at_unix_secs"], json!(NOW - 300));
        assert_eq!(remote_quota["blocked"], json!(true));
    }

    #[test]
    fn error_snapshot_builds_minimal_payload_without_history() {
        let payload = build_remote_quota_error_quota_snapshot(None, &config(), "auth_failed", NOW);
        assert_eq!(payload["code"], json!("unknown"));
        assert_eq!(payload["exhausted"], json!(false));
        assert_eq!(payload["provider_type"], json!("sub2api"));
        let remote_quota = &payload["remote_quota"];
        assert_eq!(remote_quota["sync_status"], json!("error"));
        assert_eq!(remote_quota["group_id"], json!("42"));
    }

    #[test]
    fn error_message_is_truncated_and_never_contains_credentials() {
        let long = "x".repeat(500);
        let payload = build_remote_quota_error_quota_snapshot(None, &config(), &long, NOW);
        let message = payload["remote_quota"]["last_error"]
            .as_str()
            .expect("message should be a string");
        assert_eq!(message.chars().count(), 200);
        let serialized = payload.to_string();
        for secret in ["Authorization", "Cookie", "Bearer", "refresh_token"] {
            assert!(!serialized.contains(secret), "leaked {secret}");
        }
    }

    #[test]
    fn decision_types_stay_copyable() {
        let decision: RemoteQuotaDecision = RemoteQuotaDecision::Available;
        assert_eq!(decision.blocked_until(), None);
        let blocked = RemoteQuotaDecision::Blocked {
            until_unix_secs: 10,
            reason: RemoteQuotaBlockReason::WindowExhausted,
            conservative: false,
        };
        assert_eq!(blocked.blocked_until(), Some(10));
    }

    #[test]
    fn value_merge_never_returns_empty_diff() {
        let current: Value = json!({});
        let merged = merge_remote_quota_circuit_breaker(
            Some(&current),
            &["openai:chat".to_string()],
            &RemoteQuotaDecision::Available,
            NOW,
            32,
        );
        assert!(merged.is_none());
    }
}
