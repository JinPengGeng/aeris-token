use std::sync::Arc;

use aether_gateway::{build_router_with_state, AppState, GatewayDataConfig};
use aether_runtime_state::{RuntimeSemaphore, RuntimeState};

use crate::server::{ReservedListener, SpawnedServer};

pub const GATEWAY_HARNESS_API_KEY: &str = "sk-aether-openai-chat-pressure";

#[derive(Debug, Clone)]
pub struct GatewayHarnessConfig {
    pub upstream_base_url: String,
    pub data_config: Option<GatewayDataConfig>,
    pub max_in_flight_requests: Option<usize>,
    pub distributed_request_gate: Option<RuntimeSemaphore>,
    pub tunnel_instance_id: Option<String>,
    pub tunnel_relay_base_url: Option<String>,
}

impl GatewayHarnessConfig {
    pub fn new(upstream_base_url: impl Into<String>) -> Self {
        Self {
            upstream_base_url: upstream_base_url.into(),
            data_config: None,
            max_in_flight_requests: None,
            distributed_request_gate: None,
            tunnel_instance_id: None,
            tunnel_relay_base_url: None,
        }
    }
}

#[derive(Debug)]
pub struct GatewayHarness {
    server: SpawnedServer,
    state: AppState,
}

impl GatewayHarness {
    pub async fn start(config: GatewayHarnessConfig) -> Result<Self, String> {
        Self::start_with_server(config, None, None, None).await
    }

    pub async fn start_on_port(config: GatewayHarnessConfig, port: u16) -> Result<Self, String> {
        Self::start_with_server(config, Some(port), None, None).await
    }

    pub async fn start_with_listener(
        config: GatewayHarnessConfig,
        listener: ReservedListener,
    ) -> Result<Self, String> {
        Self::start_with_server(config, None, Some(listener), None).await
    }

    /// Injects runtime state before applying tunnel identity, which rebuilds the
    /// tunnel using the selected runtime backend.
    pub async fn start_with_listener_and_runtime_state(
        config: GatewayHarnessConfig,
        listener: ReservedListener,
        runtime_state: Arc<RuntimeState>,
    ) -> Result<Self, String> {
        Self::start_with_server(config, None, Some(listener), Some(runtime_state)).await
    }

    async fn start_with_server(
        config: GatewayHarnessConfig,
        port: Option<u16>,
        listener: Option<ReservedListener>,
        runtime_state: Option<Arc<RuntimeState>>,
    ) -> Result<Self, String> {
        let mut state = match config.data_config {
            Some(data_config) => AppState::new()
                .map_err(|err| format!("failed to build gateway harness state: {err}"))?
                .with_data_config(data_config)
                .map_err(|err| format!("failed to configure gateway harness data state: {err}"))?,
            None => aether_gateway::testkit::build_openai_chat_pressure_state(
                aether_gateway::testkit::OpenAiChatPressureStateConfig::new(vec![format!(
                    "{}/v1",
                    config.upstream_base_url.trim_end_matches('/')
                )]),
            )?,
        };
        if let Some(runtime_state) = runtime_state {
            state = state.with_runtime_state(runtime_state);
        }
        if let Some(instance_id) = config.tunnel_instance_id {
            state = state.with_tunnel_identity(instance_id, config.tunnel_relay_base_url);
        }
        if let Some(limit) = config.max_in_flight_requests {
            state = state.with_request_concurrency_limit(limit);
        }
        if let Some(gate) = config.distributed_request_gate {
            state = state.with_distributed_request_concurrency_gate(gate);
        }
        let router = build_router_with_state(state.clone());
        let server = match (listener, port) {
            (Some(listener), None) => SpawnedServer::start_with_listener(listener, router)
                .map_err(|err| format!("failed to start gateway harness: {err}"))?,
            (None, Some(port)) => SpawnedServer::start_on_port(port, router)
                .await
                .map_err(|err| format!("failed to start gateway harness: {err}"))?,
            (None, None) => SpawnedServer::start(router)
                .await
                .map_err(|err| format!("failed to start gateway harness: {err}"))?,
            (Some(_), Some(_)) => unreachable!("listener and port are mutually exclusive"),
        };
        Ok(Self { server, state })
    }

    pub fn base_url(&self) -> &str {
        self.server.base_url()
    }

    pub fn port(&self) -> u16 {
        self.server.port()
    }

    pub async fn metric_samples(&self) -> Result<Vec<crate::PrometheusSample>, String> {
        let samples = aether_gateway::testkit::gateway_metric_samples(&self.state).await?;
        Ok(samples
            .into_iter()
            .map(|sample| crate::PrometheusSample {
                name: sample.name.to_string(),
                labels: sample
                    .labels
                    .into_iter()
                    .map(|label| (label.key.to_string(), label.value))
                    .collect(),
                value: sample.value.to_string(),
            })
            .collect())
    }
}
