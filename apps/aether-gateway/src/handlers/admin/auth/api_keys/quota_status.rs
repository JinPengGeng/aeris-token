use super::shared::{
    admin_api_keys_parse_limit, build_admin_api_keys_bad_request_response,
    build_admin_api_keys_data_unavailable_response,
};
use crate::app_timezone::{app_timezone, local_day_window};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_data_contracts::repository::auth::StandaloneApiKeyExportListQuery;
use aether_data_contracts::repository::usage::DailyActualCostQuery;
use aether_scheduler_core::count_recent_active_requests_for_api_key;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde_json::json;

const RPM_WINDOW_SECS: u64 = 60;
const QUOTA_STATUS_DEFAULT_LIMIT: usize = 100;
const SYSTEM_RPM_CONFIG_KEY: &str = "rate_limit_per_minute";
const SYSTEM_DAILY_USAGE_CONFIG_KEY: &str = "daily_usage_limit_usd";
/// Matches the 1e-8 USD unit scale used by `daily_actual_cost_units`.
const COST_UNITS_PER_USD: f64 = 100_000_000.0;

pub(super) async fn maybe_build_local_admin_quota_status_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("api_keys_manage")
        || decision.route_kind.as_deref() != Some("quota_status")
        || request_context.method() != http::Method::GET
        || !matches!(
            request_context.path(),
            "/api/admin/quota/status" | "/api/admin/quota/status/"
        )
    {
        return Ok(None);
    }

    Ok(Some(
        build_admin_quota_status_response(state, request_context).await?,
    ))
}

async fn build_admin_quota_status_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let limit = match admin_api_keys_parse_limit(request_context.query_string()) {
        Ok(limit) => limit,
        Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
    };
    let limit = if limit == 0 {
        QUOTA_STATUS_DEFAULT_LIMIT
    } else {
        limit
    };

    let records = match state
        .list_auth_api_key_export_standalone_records_page(&StandaloneApiKeyExportListQuery {
            skip: 0,
            limit,
            is_active: None,
        })
        .await
    {
        Ok(records) => records,
        Err(_) => return Ok(build_admin_api_keys_data_unavailable_response()),
    };

    let timezone = app_timezone();
    let now = Utc::now();
    let (_, window_start, window_end) = local_day_window(now, timezone);
    let start_unix_secs = window_start.timestamp().max(0) as u64;
    let end_unix_secs = window_end.timestamp().max(0) as u64;
    let now_unix_secs = now.timestamp().max(0) as u64;

    // Best-effort runtime snapshots: a degraded runtime or usage reader must
    // not take the whole aggregate endpoint down.
    let recent_candidates = state
        .read_recent_request_candidates(1024)
        .await
        .unwrap_or_default();
    let system_rpm_limit = read_system_u32_config(state, SYSTEM_RPM_CONFIG_KEY).await;
    let system_daily_limit_usd = read_system_f64_config(state, SYSTEM_DAILY_USAGE_CONFIG_KEY).await;

    let mut keys = Vec::with_capacity(records.len());
    for record in &records {
        let rpm_limit = record
            .rate_limit
            .filter(|limit| *limit > 0)
            .map(|limit| limit as u64)
            .or(system_rpm_limit);
        let current_rpm = count_recent_rpm_requests_for_api_key(
            &recent_candidates,
            record.api_key_id.as_str(),
            now_unix_secs,
        );
        let concurrency_limit = record
            .concurrent_limit
            .filter(|limit| *limit > 0)
            .map(|limit| limit as u64);
        let current_concurrency = count_recent_active_requests_for_api_key(
            &recent_candidates,
            record.api_key_id.as_str(),
            now_unix_secs,
        );
        let daily_limit_usd = record.daily_usage_limit_usd.or(system_daily_limit_usd);
        let daily_used_usd = state
            .read_daily_actual_cost_units_for_key(&DailyActualCostQuery {
                user_id: None,
                api_key_id: record.api_key_id.clone(),
                start_unix_secs,
                end_unix_secs,
            })
            .await
            .map(|counts| counts.key_units as f64 / COST_UNITS_PER_USD)
            .unwrap_or(0.0);

        keys.push(json!({
            "api_key_id": record.api_key_id,
            "user_id": record.user_id,
            "name": record.name,
            "is_active": record.is_active,
            "rpm": {
                "current": current_rpm,
                "limit": rpm_limit,
                "limited": is_limited(current_rpm as u64, rpm_limit),
            },
            "concurrency": {
                "current": current_concurrency,
                "limit": concurrency_limit,
                "limited": is_limited(current_concurrency as u64, concurrency_limit),
            },
            "daily_usage": {
                "used_usd": daily_used_usd,
                "limit_usd": daily_limit_usd,
                "limited": daily_limit_usd
                    .filter(|limit| *limit > 0.0)
                    .is_some_and(|limit| daily_used_usd >= limit),
            },
        }));
    }

    Ok(Json(json!({
        "keys": keys,
        "total": keys.len(),
        "limit": limit,
        "window": {
            "start": rfc3339(window_start),
            "end": rfc3339(window_end),
            "timezone": timezone.name(),
        },
    }))
    .into_response())
}

fn is_limited(current: u64, limit: Option<u64>) -> bool {
    limit
        .filter(|limit| *limit > 0)
        .is_some_and(|limit| current >= limit)
}

fn count_recent_rpm_requests_for_api_key(
    recent_candidates: &[aether_data_contracts::repository::candidates::StoredRequestCandidate],
    api_key_id: &str,
    now_unix_secs: u64,
) -> usize {
    let window_start_unix_secs = now_unix_secs.saturating_sub(RPM_WINDOW_SECS);
    recent_candidates
        .iter()
        .filter(|candidate| candidate.api_key_id.as_deref() == Some(api_key_id))
        .filter(|candidate| {
            let observed_at_unix_secs = candidate
                .started_at_unix_ms
                .map(|ms| ms / 1000)
                .unwrap_or(candidate.created_at_unix_ms / 1000);
            observed_at_unix_secs >= window_start_unix_secs
                && observed_at_unix_secs <= now_unix_secs
        })
        .count()
}

async fn read_system_u32_config(state: &AdminAppState<'_>, key: &str) -> Option<u64> {
    state
        .app()
        .read_system_config_json_value(key)
        .await
        .ok()
        .flatten()
        .and_then(|value| value.as_u64())
        .filter(|limit| *limit > 0)
}

async fn read_system_f64_config(state: &AdminAppState<'_>, key: &str) -> Option<f64> {
    state
        .app()
        .read_system_config_json_value(key)
        .await
        .ok()
        .flatten()
        .and_then(|value| value.as_f64())
        .filter(|limit| limit.is_finite() && *limit > 0.0)
}

fn rfc3339(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
