mod fixtures;
mod redis;
mod scheduler_acceptance;
mod server;
mod tracing;
mod upstream_recorder;
mod wait;

#[cfg(feature = "gateway")]
mod execution_runtime;
#[cfg(feature = "gateway")]
mod gateway;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "gateway")]
mod tunnel;

pub use aether_loadtools::{
    fetch_prometheus_samples, find_metric_value_u64, parse_prometheus_samples, PrometheusSample,
};
pub use aether_loadtools::{
    json_body, run_http_load_probe, run_multi_url_http_load_probe, test_http_client,
    test_http_client_config, HttpLoadProbeConfig, HttpLoadProbeResponseMode, HttpLoadProbeResult,
    MultiUrlHttpLoadProbeResult,
};
pub use aether_loadtools::{BenchmarkRuntimeSampler, BenchmarkRuntimeSnapshot};
pub use fixtures::test_trace_id;
pub use redis::ManagedRedisServer;
pub use scheduler_acceptance::{
    validate_attempt_budget, validate_attempt_order, validate_denied_admission_never_sends,
    validate_denied_admission_with_network, validate_exactly_one_half_open_probe,
    validate_frozen_pages, validate_half_open_probe_e2e, validate_network_authority,
    validate_no_pool_stampede, validate_no_replay_after_client_commit,
    validate_no_replay_after_commit_with_network, validate_send_contract, AcceptanceEvent,
    AcceptanceEventSink, AcceptanceNamespace, AttemptBudgetExpectation, ControlledCheckpoint,
    DiagnosticTrace, FaultInjector, ManualClock, NetworkObservation, NetworkSend, TraceKind,
};
pub use server::{reserve_local_port, SpawnedServer};
pub use tracing::{init_test_runtime, init_test_runtime_for, test_runtime_config};
pub use upstream_recorder::CountingUpstreamRecorder;
pub use wait::wait_until;

#[cfg(feature = "gateway")]
pub use execution_runtime::{ExecutionRuntimeHarness, ExecutionRuntimeHarnessConfig};
#[cfg(feature = "gateway")]
pub use gateway::{GatewayHarness, GatewayHarnessConfig, GATEWAY_HARNESS_API_KEY};
#[cfg(feature = "postgres")]
pub use postgres::{prepare_aether_postgres_schema, ManagedPostgresServer};
#[cfg(feature = "gateway")]
pub use tunnel::{TunnelHarness, TunnelHarnessConfig};
