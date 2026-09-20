//! Per-turn resource admission for the Responses WebSocket bridge.
//!
//! A WebSocket connection may live for a long time, but each `response.create`
//! is still one active upstream execution.  Keep the resource leases attached
//! to the turn instead of the socket so idle connections do not consume
//! upstream capacity.

use std::time::Instant;

use aether_contracts::ExecutionPlan;
use aether_scheduler_core::SendAdmissionSkipReason;

use crate::execution_runtime::acquire_upstream_execution_gate;
use crate::provider_pool_demand::{
    acquire_provider_pool_execution_guard, ProviderPoolInFlightAdmission, ProviderPoolInFlightGuard,
};
use crate::scheduler::send_admission::{
    request_gateway_send_admission, GatewaySendAdmissionDecision, GatewaySendAdmissionGuard,
};
use crate::upstream_admission::UpstreamTargetAdmissionPermit;
use crate::{AppState, GatewayError};

pub(crate) struct ResponsesWebSocketTurnAdmission {
    upstream_execution: Option<aether_runtime::ConcurrencyPermit>,
    upstream_target: Option<UpstreamTargetAdmissionPermit>,
    provider_pool: Option<ProviderPoolInFlightGuard>,
    send_admission: Option<GatewaySendAdmissionGuard>,
    acquired_at: Instant,
}

impl ResponsesWebSocketTurnAdmission {
    pub(crate) async fn acquire(
        state: &AppState,
        plan: &ExecutionPlan,
        trace_id: &str,
    ) -> Result<Self, GatewayError> {
        let upstream_execution = acquire_upstream_execution_gate(state, trace_id).await?;
        let upstream_target = match state
            .upstream_target_admission
            .acquire(plan, trace_id)
            .await
        {
            Ok(permit) => permit,
            Err(error) => {
                drop(upstream_execution);
                return Err(error);
            }
        };
        let provider_pool = match acquire_provider_pool_execution_guard(state, plan).await? {
            ProviderPoolInFlightAdmission::Acquired(guard) => guard,
            ProviderPoolInFlightAdmission::Saturated { limit } => {
                drop(upstream_target);
                drop(upstream_execution);
                return Err(GatewayError::Client {
                    status: http::StatusCode::TOO_MANY_REQUESTS,
                    message: format!("上游账号并发已达上限 ({limit})"),
                });
            }
        };
        let send_admission = match request_gateway_send_admission(state, plan).await {
            GatewaySendAdmissionDecision::Admit(guard) => guard,
            GatewaySendAdmissionDecision::Skip(skip) => {
                return Err(gateway_send_admission_skip_error(skip.reason()));
            }
            GatewaySendAdmissionDecision::Stop(stop) => {
                return Err(GatewayError::ControlUnavailable {
                    trace_id: trace_id.to_string(),
                    message: format!(
                        "send admission stopped before upstream dispatch: {}",
                        stop.reason().as_str()
                    ),
                });
            }
        };

        Ok(Self {
            upstream_execution,
            upstream_target,
            provider_pool,
            send_admission: Some(send_admission),
            acquired_at: Instant::now(),
        })
    }

    /// Release the distributed provider token before the turn's persistence
    /// work. The remaining permits are local RAII guards and are dropped with
    /// this value.
    pub(crate) async fn release(mut self) {
        if let Some(admission) = self.send_admission.take() {
            admission.release().await;
        }
        if let Some(provider_pool) = self.provider_pool.take() {
            provider_pool.release().await;
        }
        drop(self.upstream_target.take());
        drop(self.upstream_execution.take());
    }
}

fn gateway_send_admission_skip_error(reason: SendAdmissionSkipReason) -> GatewayError {
    let status = match reason {
        SendAdmissionSkipReason::ProviderQuotaExhausted
        | SendAdmissionSkipReason::ProviderConcurrencyExhausted
        | SendAdmissionSkipReason::CredentialRpmExhausted
        | SendAdmissionSkipReason::CredentialConcurrencyExhausted
        | SendAdmissionSkipReason::AccountQuotaExhausted => http::StatusCode::TOO_MANY_REQUESTS,
        _ => http::StatusCode::SERVICE_UNAVAILABLE,
    };
    GatewayError::Client {
        status,
        message: format!("上游账号当前不可用 ({})", reason.as_str()),
    }
}

impl Drop for ResponsesWebSocketTurnAdmission {
    fn drop(&mut self) {
        crate::stage_metrics::observe_gateway_stage_ms(
            "websocket_turn_admission_held",
            self.acquired_at.elapsed().as_millis() as u64,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_contracts::{ExecutionPlan, RequestBody};
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };

    use super::*;
    use crate::data::GatewayDataState;

    fn provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "provider".to_string(),
            None,
            "custom".to_string(),
        )
        .expect("provider")
    }

    fn endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-1".to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint")
    }

    fn key() -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key");
        key.concurrent_limit = Some(1);
        key
    }

    fn plan(request_id: &str) -> ExecutionPlan {
        ExecutionPlan {
            request_id: request_id.to_string(),
            candidate_id: Some(format!("candidate-{request_id}")),
            provider_name: Some("provider".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/chat/completions".to_string(),
            headers: Default::default(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(serde_json::json!({})),
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("gpt-test".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn state() -> AppState {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider()],
            vec![endpoint()],
            vec![key()],
        ));
        let data = GatewayDataState::with_provider_catalog_reader_for_tests(repository.clone())
            .with_cached_provider_catalog_reader_for_tests(repository);
        let mut state = AppState::new().expect("state");
        state.replace_data_state(Arc::new(data));
        state
    }

    #[tokio::test]
    async fn key_limit_releases_before_next_websocket_turn() {
        let state = state();
        let first = ResponsesWebSocketTurnAdmission::acquire(&state, &plan("request-1"), "trace-1")
            .await
            .expect("first turn should acquire both independent gates");

        let second =
            ResponsesWebSocketTurnAdmission::acquire(&state, &plan("request-2"), "trace-2").await;
        assert!(matches!(
            second,
            Err(GatewayError::Client { status, .. }) if status == http::StatusCode::TOO_MANY_REQUESTS
        ));

        first.release().await;
        let replacement =
            ResponsesWebSocketTurnAdmission::acquire(&state, &plan("request-3"), "trace-3")
                .await
                .expect("released turn should restore both independent gates");
        replacement.release().await;
    }
}
