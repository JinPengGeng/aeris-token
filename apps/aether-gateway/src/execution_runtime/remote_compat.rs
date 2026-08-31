use aether_ai_serving::AttemptTarget;
use aether_contracts::{
    ExecutionAttemptBudgetConsumption, ExecutionPlan, ExecutionResult, ExecutionRuntimeRequest,
};

use crate::constants::TRACE_ID_HEADER;
use crate::execution_runtime::transport::{gateway_transport_error, request_attempt_budget};
use crate::{AppState, GatewayError};

async fn dispatch_remote_execution_runtime_request(
    request: reqwest::RequestBuilder,
    plan: &ExecutionPlan,
    conservative_reconcile: bool,
) -> Result<reqwest::Response, GatewayError> {
    let budget = request_attempt_budget().map_err(gateway_transport_error)?;
    let target = AttemptTarget::from(plan);
    // The gateway-to-runtime RPC is an infrastructure hop, not a provider attempt. The grant is
    // the atomic outstanding reservation for the provider-facing send executed by the runtime.
    let delegation = budget
        .reserve_delegation_grant(&target)
        .map_err(GatewayError::AttemptBudget)?;
    let response_future = request
        .json(&ExecutionRuntimeRequest {
            plan: plan.clone(),
            attempt_budget: delegation.grant().clone(),
        })
        .send();
    match response_future.await {
        Ok(response) => {
            if conservative_reconcile {
                delegation
                    .reconcile_conservative()
                    .map_err(GatewayError::AttemptBudget)?;
            } else {
                reconcile_remote_consumption(delegation, &response)?;
            }
            Ok(response)
        }
        Err(err) => {
            // Once send() has started the remote may have accepted and dispatched the request.
            delegation
                .reconcile_conservative()
                .map_err(GatewayError::AttemptBudget)?;
            Err(GatewayError::Internal(err.to_string()))
        }
    }
}

fn reconcile_remote_consumption(
    delegation: aether_ai_serving::AttemptBudgetDelegation,
    response: &reqwest::Response,
) -> Result<(), GatewayError> {
    let Some(consumption) = remote_consumption_from_headers(response.headers()) else {
        // Streaming response headers are fixed before body execution completes. Missing or
        // malformed accounting therefore means "possibly consumed", bounded to this one grant.
        return delegation
            .reconcile_conservative()
            .map_err(GatewayError::AttemptBudget);
    };
    delegation
        .reconcile(consumption)
        .map_err(GatewayError::AttemptBudget)
}

fn remote_consumption_from_headers(
    headers: &reqwest::header::HeaderMap,
) -> Option<ExecutionAttemptBudgetConsumption> {
    let value =
        |name: &str| -> Option<u64> { headers.get(name)?.to_str().ok()?.parse::<u64>().ok() };
    match (
        value("x-aether-attempt-total-dispatches"),
        value("x-aether-attempt-credential-entries"),
        value("x-aether-attempt-provider-switches"),
    ) {
        (Some(total_dispatches), Some(credential_entries), Some(provider_switches)) => {
            Some(ExecutionAttemptBudgetConsumption {
                total_dispatches,
                credential_entries,
                provider_switches,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::remote_consumption_from_headers;

    #[test]
    fn remote_consumption_requires_all_three_valid_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "x-aether-attempt-total-dispatches",
            reqwest::header::HeaderValue::from_static("1"),
        );
        assert!(remote_consumption_from_headers(&headers).is_none());

        headers.insert(
            "x-aether-attempt-credential-entries",
            reqwest::header::HeaderValue::from_static("1"),
        );
        headers.insert(
            "x-aether-attempt-provider-switches",
            reqwest::header::HeaderValue::from_static("0"),
        );
        let usage = remote_consumption_from_headers(&headers).unwrap();
        assert_eq!(usage.total_dispatches, 1);
        assert_eq!(usage.credential_entries, 1);
        assert_eq!(usage.provider_switches, 0);

        headers.insert(
            "x-aether-attempt-total-dispatches",
            reqwest::header::HeaderValue::from_static("invalid"),
        );
        assert!(remote_consumption_from_headers(&headers).is_none());
    }
}

fn build_remote_execution_runtime_request(
    state: &AppState,
    remote_execution_runtime_base_url: &str,
    path: &str,
    trace_id: Option<&str>,
    _plan: &ExecutionPlan,
) -> reqwest::RequestBuilder {
    let mut request = state
        .client
        .post(format!("{remote_execution_runtime_base_url}{path}"));
    if let Some(trace_id) = trace_id.map(str::trim).filter(|value| !value.is_empty()) {
        request = request.header(TRACE_ID_HEADER, trace_id);
    }
    request
}

pub(crate) async fn post_sync_plan_to_remote_execution_runtime(
    state: &AppState,
    remote_execution_runtime_base_url: &str,
    trace_id: Option<&str>,
    plan: &ExecutionPlan,
) -> Result<reqwest::Response, GatewayError> {
    dispatch_remote_execution_runtime_request(
        build_remote_execution_runtime_request(
            state,
            remote_execution_runtime_base_url,
            "/v1/execute/sync",
            trace_id,
            plan,
        ),
        plan,
        false,
    )
    .await
}

pub(crate) async fn post_stream_plan_to_remote_execution_runtime(
    state: &AppState,
    remote_execution_runtime_base_url: &str,
    trace_id: Option<&str>,
    plan: &ExecutionPlan,
) -> Result<reqwest::Response, GatewayError> {
    dispatch_remote_execution_runtime_request(
        build_remote_execution_runtime_request(
            state,
            remote_execution_runtime_base_url,
            "/v1/execute/stream",
            trace_id,
            plan,
        ),
        plan,
        true,
    )
    .await
}

pub(crate) async fn execute_sync_plan_via_remote_execution_runtime(
    state: &AppState,
    remote_execution_runtime_base_url: &str,
    trace_id: Option<&str>,
    plan: &ExecutionPlan,
) -> Result<ExecutionResult, GatewayError> {
    let response = post_sync_plan_to_remote_execution_runtime(
        state,
        remote_execution_runtime_base_url,
        trace_id,
        plan,
    )
    .await?;
    if response.status() != http::StatusCode::OK {
        return Err(GatewayError::Internal(format!(
            "execution runtime returned HTTP {}",
            response.status()
        )));
    }

    response
        .json::<ExecutionResult>()
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))
}
