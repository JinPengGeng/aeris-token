use super::{
    build_auth_error_response, build_auth_json_response, http, parse_wallet_limit,
    resolve_authenticated_local_user, AppState, Body, GatewayPublicRequestContext, Response,
};
use aether_data_contracts::repository::settlement::StoredRechargeRecoveryJob;
use serde_json::{json, Value};

fn public_recovery_summary(job: &StoredRechargeRecoveryJob) -> Value {
    // The monetary values describe this job's last execution, not the live
    // account balance. Never expose the source transaction or notification lease.
    json!({
        "id": job.id,
        "payment_order_id": job.payment_order_id,
        "wallet_id": job.wallet_id,
        "state": job.state,
        "principal_cost_units": job.principal_cost_units,
        "collected_cost_units": job.collected_cost_units,
        "outstanding_cost_units": job.outstanding_cost_units,
        "available_recharge_cost_units": job.available_recharge_cost_units,
        "retry_count": job.retry_count,
        "next_attempt_at_unix_secs": job.next_attempt_at_unix_secs,
        "error_code": job.error_code,
        "created_at_unix_secs": job.created_at_unix_secs,
        "updated_at_unix_secs": job.updated_at_unix_secs,
    })
}

pub(super) async fn handle_wallet_recharge_recoveries(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let limit = match parse_wallet_limit(request_context.request_query_string.as_deref()) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    if !state.data.has_recharge_recovery_backend() {
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "历史欠费追扣记录暂不可用",
            false,
        );
    }
    match state
        .data
        .list_recharge_recovery_jobs_for_user(&auth.user.id, limit)
        .await
    {
        Ok(jobs) => {
            // Repository ownership is authoritative; also fail closed if an
            // adapter returns a row for a different or missing owner.
            if jobs
                .iter()
                .any(|job| job.user_id.as_deref() != Some(auth.user.id.as_str()))
            {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    "历史欠费追扣记录暂不可用",
                    false,
                );
            }
            build_auth_json_response(
                http::StatusCode::OK,
                json!({"items": jobs.iter().map(public_recovery_summary).collect::<Vec<_>>(), "limit": limit}),
                None,
            )
        }
        Err(_) => build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "历史欠费追扣记录暂不可用",
            false,
        ),
    }
}
