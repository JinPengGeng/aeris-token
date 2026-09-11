use super::super::config::admin_provider_ops_config_object;
use super::responses::{admin_provider_ops_action_error, admin_provider_ops_action_response};
use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::ops::parse_sub2api_remote_quota_config;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
use serde_json::{json, Value};

pub(super) async fn admin_provider_ops_run_sync_remote_quota_action(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider: &StoredProviderCatalogProvider,
) -> Value {
    let start = std::time::Instant::now();
    let Some(provider_ops_config) = admin_provider_ops_config_object(provider) else {
        return admin_provider_ops_action_error(
            "not_configured",
            "sync_remote_quota",
            "未配置操作设置",
            None,
        );
    };
    match parse_sub2api_remote_quota_config(provider_ops_config) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return admin_provider_ops_action_error(
                "not_configured",
                "sync_remote_quota",
                "该 Provider 未启用远程配额同步（provider_ops.remote_quota.enabled）",
                None,
            )
        }
        Err(message) => {
            return admin_provider_ops_action_error(
                "not_configured",
                "sync_remote_quota",
                message,
                None,
            )
        }
    }
    if !state.app().has_provider_catalog_data_writer() {
        return admin_provider_ops_action_error(
            "not_supported",
            "sync_remote_quota",
            "当前部署缺少 Provider Catalog 写通道",
            None,
        );
    }

    let report = match crate::maintenance::perform_remote_quota_sync_once_for_provider(
        state.app(),
        provider_id,
    )
    .await
    {
        Ok(report) => report,
        Err(_) => {
            return admin_provider_ops_action_error(
                "unknown_error",
                "sync_remote_quota",
                "远程配额同步执行失败",
                Some(start.elapsed().as_millis() as u64),
            )
        }
    };
    let response_time_ms = Some(start.elapsed().as_millis() as u64);
    let summary = report.summary;
    let data = json!({
        "attempted": summary.attempted,
        "applied": summary.applied,
        "blocked": summary.blocked,
        "recovered": summary.recovered,
        "skipped": summary.skipped,
        "failed": summary.failed,
        "last_error": report
            .failure
            .as_ref()
            .map(|failure| failure.message.clone()),
    });

    if summary.failed > 0 {
        let (status, message) = match report.failure.as_ref() {
            Some(failure) => (
                match failure.kind {
                    "auth_failed" => "auth_failed",
                    "network_error" => "network_error",
                    "invalid_data" => "parse_error",
                    "invalid_config" | "not_configured" => "not_configured",
                    _ => "unknown_error",
                },
                failure.message.clone(),
            ),
            None => ("unknown_error", "远程配额同步失败".to_string()),
        };
        return admin_provider_ops_action_response(
            status,
            "sync_remote_quota",
            data,
            Some(message),
            response_time_ms,
            0,
        );
    }
    if summary.applied == 0 {
        return admin_provider_ops_action_response(
            "pending",
            "sync_remote_quota",
            data,
            Some("另一个同步任务正在执行，请稍后重试".to_string()),
            response_time_ms,
            0,
        );
    }

    let message = if summary.blocked > 0 {
        "同步完成：远程配额已耗尽或订阅失效，已熔断该 Provider".to_string()
    } else if summary.recovered > 0 {
        "同步完成：远程配额已恢复，已解除熔断".to_string()
    } else {
        "同步完成：远程配额正常".to_string()
    };
    admin_provider_ops_action_response(
        "success",
        "sync_remote_quota",
        data,
        Some(message),
        response_time_ms,
        0,
    )
}
