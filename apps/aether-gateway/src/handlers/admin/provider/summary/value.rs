use crate::handlers::admin::provider::shared::support::{
    provider_transfer_limit_from_config, PROVIDER_MAX_TRANSFER_COUNT_CONFIG_KEY,
    PROVIDER_MAX_TRANSFER_TIMEOUT_SECONDS_CONFIG_KEY,
};
use crate::handlers::admin::shared::unix_secs_to_rfc3339;
use crate::handlers::public::{request_candidate_event_unix_ms, request_candidate_status_label};
use crate::orchestration::{codex_cyber_flag_passthrough_enabled, responses_websocket_adapter};
use crate::provider_key_auth::provider_key_effective_api_formats;
use aether_admin::provider::redaction::{admin_secret_safe_json, admin_secret_safe_proxy};
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_scheduler_core::provider_key_health_score;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn json_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        serde_json::Value::String(value) => !value.trim().is_empty(),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => !value.is_empty(),
    }
}

fn endpoint_timestamp_or_now(value: Option<u64>, now_unix_secs: u64) -> serde_json::Value {
    unix_secs_to_rfc3339(value.unwrap_or(now_unix_secs))
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) fn build_admin_provider_summary_value(
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    keys: &[StoredProviderCatalogKey],
    quota_snapshot: Option<&aether_data_contracts::repository::quota::StoredProviderQuotaSnapshot>,
    model_stats: Option<
        &aether_data_contracts::repository::global_models::StoredProviderModelStats,
    >,
    active_global_model_ids: Vec<String>,
    now_unix_secs: u64,
) -> serde_json::Value {
    let total_endpoints = endpoints.len();
    let active_endpoints = endpoints
        .iter()
        .filter(|endpoint| endpoint.is_active)
        .count();
    let total_keys = keys.len();
    let active_keys = keys.iter().filter(|key| key.is_active).count();
    let total_models = model_stats
        .map(|stats| stats.total_models as usize)
        .unwrap_or(0);
    let active_models = model_stats
        .map(|stats| stats.active_models as usize)
        .unwrap_or(0);
    let api_formats = endpoints
        .iter()
        .map(|endpoint| endpoint.api_format.clone())
        .collect::<Vec<_>>();

    let format_to_endpoint_ids = endpoints.iter().fold(
        BTreeMap::<String, BTreeSet<String>>::new(),
        |mut formats, endpoint| {
            formats
                .entry(endpoint.api_format.clone())
                .or_default()
                .insert(endpoint.id.clone());
            formats
        },
    );
    let mut keys_by_endpoint = BTreeMap::<String, Vec<&StoredProviderCatalogKey>>::new();
    for endpoint in endpoints {
        keys_by_endpoint.entry(endpoint.id.clone()).or_default();
    }
    for key in keys {
        let mut key_endpoint_ids = BTreeSet::new();
        for api_format in
            provider_key_effective_api_formats(key, &provider.provider_type, endpoints)
        {
            if let Some(endpoint_ids) = format_to_endpoint_ids.get(&api_format) {
                for endpoint_id in endpoint_ids {
                    if key_endpoint_ids.insert(endpoint_id.clone()) {
                        keys_by_endpoint
                            .entry(endpoint_id.clone())
                            .or_default()
                            .push(key);
                    }
                }
            }
        }
    }

    let mut endpoint_health_scores = Vec::with_capacity(endpoints.len());
    let endpoint_health_details = endpoints
        .iter()
        .map(|endpoint| {
            let endpoint_keys = keys_by_endpoint
                .get(&endpoint.id)
                .cloned()
                .unwrap_or_default();
            let scores = endpoint_keys
                .iter()
                .filter(|key| endpoint.is_active && key.is_active)
                .map(|key| {
                    provider_key_health_score(key, &endpoint.api_format)
                        .filter(|score| score.is_finite())
                        .unwrap_or(1.0)
                })
                .collect::<Vec<_>>();
            let health_score =
                (!scores.is_empty()).then(|| scores.iter().sum::<f64>() / scores.len() as f64);
            if let Some(score) = health_score {
                endpoint_health_scores.push(score);
            }
            json!({
                "api_format": endpoint.api_format,
                "health_score": health_score,
                "is_active": endpoint.is_active,
                "total_keys": endpoint_keys.len(),
                "active_keys": endpoint_keys.iter().filter(|key| key.is_active).count(),
            })
        })
        .collect::<Vec<_>>();
    let avg_health_score = (!endpoint_health_scores.is_empty())
        .then(|| endpoint_health_scores.iter().sum::<f64>() / endpoint_health_scores.len() as f64);
    let unhealthy_endpoints = endpoint_health_scores
        .iter()
        .filter(|score| **score < 0.5)
        .count();

    let provider_config = provider.config.clone();
    let config = provider_config
        .as_ref()
        .and_then(serde_json::Value::as_object);
    let max_transfer_count =
        provider_transfer_limit_from_config(config, PROVIDER_MAX_TRANSFER_COUNT_CONFIG_KEY);
    let max_transfer_timeout_seconds = provider_transfer_limit_from_config(
        config,
        PROVIDER_MAX_TRANSFER_TIMEOUT_SECONDS_CONFIG_KEY,
    );
    let provider_ops_config = config.and_then(|cfg| cfg.get("provider_ops"));
    let ops_configured = provider_ops_config.is_some_and(json_truthy);
    let ops_architecture_id = provider_ops_config
        .and_then(serde_json::Value::as_object)
        .and_then(|cfg| cfg.get("architecture_id"))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    let kiro_simulated_cache_enabled = config
        .and_then(|cfg| cfg.get("kiro"))
        .and_then(serde_json::Value::as_object)
        .and_then(|cfg| cfg.get("simulated_cache_enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let ops_quota_alert_enabled = provider_ops_config
        .and_then(serde_json::Value::as_object)
        .and_then(|cfg| cfg.get("quota_alert"))
        .and_then(serde_json::Value::as_object)
        .and_then(|cfg| cfg.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let key_status_snapshots = keys
        .iter()
        .map(|key| key.status_snapshot.as_ref())
        .collect::<Vec<_>>();
    let ops_remote_quota = aether_admin::provider::ops::build_sub2api_remote_quota_admin_status(
        provider.config.as_ref(),
        &key_status_snapshots,
    );
    let billing_type = quota_snapshot
        .map(|quota| quota.billing_type.clone())
        .or_else(|| provider.billing_type.clone());
    let monthly_quota_usd = quota_snapshot
        .and_then(|quota| quota.monthly_quota_usd)
        .or(provider.monthly_quota_usd);
    let monthly_used_usd = quota_snapshot
        .map(|quota| quota.monthly_used_usd)
        .or(provider.monthly_used_usd);
    let quota_reset_day = quota_snapshot
        .and_then(|quota| quota.quota_reset_day)
        .or(provider.quota_reset_day);
    let quota_last_reset_at = quota_snapshot
        .and_then(|quota| quota.quota_last_reset_at_unix_secs)
        .or(provider.quota_last_reset_at_unix_secs)
        .and_then(unix_secs_to_rfc3339);
    let quota_expires_at = quota_snapshot
        .and_then(|quota| quota.quota_expires_at_unix_secs)
        .or(provider.quota_expires_at_unix_secs)
        .and_then(unix_secs_to_rfc3339);

    json!({
        "id": provider.id.clone(),
        "name": provider.name.clone(),
        "provider_type": provider.provider_type.clone(),
        "description": provider.description.clone(),
        "website": provider.website.clone(),
        "provider_priority": provider.provider_priority,
        "keep_priority_on_conversion": provider.keep_priority_on_conversion,
        "enable_format_conversion": provider.enable_format_conversion,
        "is_active": provider.is_active,
        "billing_type": billing_type,
        "monthly_quota_usd": monthly_quota_usd,
        "monthly_used_usd": monthly_used_usd,
        "quota_reset_day": quota_reset_day,
        "quota_last_reset_at": quota_last_reset_at,
        "quota_expires_at": quota_expires_at,
        "max_retries": provider.max_retries,
        "max_transfer_count": max_transfer_count,
        "max_transfer_timeout_seconds": max_transfer_timeout_seconds,
        "proxy": admin_secret_safe_proxy(provider.proxy.as_ref()),
        "stream_first_byte_timeout": provider.stream_first_byte_timeout_secs,
        "request_timeout": provider.request_timeout_secs,
        "claude_code_advanced": admin_secret_safe_json(config.and_then(|cfg| cfg.get("claude_code_advanced"))),
        "pool_advanced": admin_secret_safe_json(config.and_then(|cfg| cfg.get("pool_advanced"))),
        "failover_rules": admin_secret_safe_json(config.and_then(|cfg| cfg.get("failover_rules"))),
        "chat_pii_redaction": admin_secret_safe_json(config.and_then(|cfg| cfg.get("chat_pii_redaction"))),
        "total_endpoints": total_endpoints,
        "active_endpoints": active_endpoints,
        "total_keys": total_keys,
        "active_keys": active_keys,
        "total_models": total_models,
        "active_models": active_models,
        "global_model_ids": active_global_model_ids,
        "avg_health_score": avg_health_score,
        "unhealthy_endpoints": unhealthy_endpoints,
        "api_formats": api_formats,
        "endpoint_health_details": endpoint_health_details,
        "ops_configured": ops_configured,
        "ops_architecture_id": ops_architecture_id,
        "kiro_simulated_cache_enabled": kiro_simulated_cache_enabled,
        "codex_cyber_flag_passthrough_enabled": codex_cyber_flag_passthrough_enabled(&provider.provider_type, provider.config.as_ref()),
        "codex_fingerprint_convergence_enabled": crate::provider_transport::codex_fingerprint_convergence_enabled(
            &provider.provider_type,
            provider.config.as_ref(),
        ),
        "responses_websocket_enabled": responses_websocket_adapter(&provider.provider_type, provider.config.as_ref()).is_some(),
        "ops_quota_alert_enabled": ops_quota_alert_enabled,
        "ops_remote_quota": ops_remote_quota,
        "created_at": endpoint_timestamp_or_now(provider.created_at_unix_ms, now_unix_secs),
        "updated_at": endpoint_timestamp_or_now(provider.updated_at_unix_secs, now_unix_secs),
    })
}

#[cfg(test)]
mod tests {
    use super::build_admin_provider_summary_value;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use serde_json::json;

    const NOW_UNIX_SECS: u64 = 1_896_004_800;

    fn provider_with_remote_quota(
        remote_quota: serde_json::Value,
    ) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
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
                    "base_url": "https://sub2api.example.com",
                    "connector": {
                        "auth_type": "api_key",
                        "config": {},
                        "credentials": {"refresh_token": "ciphertext"},
                    },
                    "remote_quota": remote_quota,
                }
            })),
        )
    }

    fn key_with_status_snapshot(
        status_snapshot: Option<serde_json::Value>,
    ) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.status_snapshot = status_snapshot;
        key
    }

    fn summary_value(
        provider: &StoredProviderCatalogProvider,
        keys: &[StoredProviderCatalogKey],
    ) -> serde_json::Value {
        build_admin_provider_summary_value(
            provider,
            &[],
            keys,
            None,
            None,
            Vec::new(),
            NOW_UNIX_SECS,
        )
    }

    #[test]
    fn ops_remote_quota_disabled_by_default() {
        let provider = StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "Plain".to_string(),
            None,
            "custom".to_string(),
        )
        .expect("provider should build");
        let value = summary_value(&provider, &[]);
        assert_eq!(value["ops_remote_quota"], json!({"enabled": false}));
    }

    #[test]
    fn ops_remote_quota_projects_latest_sync_status() {
        let provider = provider_with_remote_quota(json!({
            "enabled": true,
            "group_id": "42",
            "fetch_interval_seconds": 60
        }));
        let key = key_with_status_snapshot(Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "sub2api",
                "code": "exhausted",
                "exhausted": true,
                "usage_ratio": 1.0,
                "updated_at": 1_896_000_000_u64,
                "reset_at": 1_896_048_000_u64,
                "windows": [{
                    "code": "monthly",
                    "label": "月",
                    "used_value": 100.0,
                    "limit_value": 100.0,
                    "used_ratio": 1.0,
                    "reset_at": 1_896_048_000_u64,
                    "is_exhausted": true
                }],
                "remote_quota": {
                    "sync_status": "ok",
                    "synced_at_unix_secs": 1_896_000_000_u64,
                    "group_id": "42",
                    "group_name": "Pro",
                    "subscription_id": "9",
                    "subscription_status": "active",
                    "subscription_active": true,
                    "blocked": true,
                    "block_reason": "window_exhausted",
                    "blocked_until_unix_secs": 1_896_048_000_u64,
                    "conservative_cooldown": false
                }
            }
        })));

        let value = summary_value(&provider, std::slice::from_ref(&key));
        let remote_quota = &value["ops_remote_quota"];
        assert_eq!(remote_quota["enabled"], json!(true));
        assert_eq!(remote_quota["group_id"], json!("42"));
        assert_eq!(remote_quota["fetch_interval_seconds"], json!(60));
        let sync = &remote_quota["sync"];
        assert_eq!(sync["sync_status"], json!("ok"));
        assert_eq!(sync["exhausted"], json!(true));
        assert_eq!(sync["blocked"], json!(true));
        assert_eq!(sync["windows"][0]["used_value"], json!(100.0));
        // 凭据与内部字段不得透出
        let serialized = remote_quota.to_string();
        assert!(!serialized.contains("ciphertext"));
        assert!(!serialized.contains("refresh_token"));
    }

    #[test]
    fn ops_remote_quota_without_sync_history_returns_null_sync() {
        let provider = provider_with_remote_quota(json!({
            "enabled": true,
            "group_id": "42"
        }));
        let value = summary_value(&provider, &[key_with_status_snapshot(None)]);
        assert_eq!(value["ops_remote_quota"]["enabled"], json!(true));
        assert_eq!(
            value["ops_remote_quota"]["sync"],
            json!(serde_json::Value::Null)
        );
    }
}
