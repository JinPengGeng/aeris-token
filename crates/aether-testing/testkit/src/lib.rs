mod fixtures;
mod server;
mod tracing;
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
    json_body, run_http_load_probe, run_http_load_probe_with_options,
    run_multi_url_http_load_probe, test_http_client, test_http_client_config, HttpLoadProbeConfig,
    HttpLoadProbeOptions, HttpLoadProbeResponseMode, HttpLoadProbeResult,
    MultiUrlHttpLoadProbeResult,
};
pub use aether_loadtools::{BenchmarkRuntimeSampler, BenchmarkRuntimeSnapshot};
pub use aether_test_support::ManagedRedisServer;
// ManagedRedisServer has a single implementation in `aether-test-support`;
// this crate and `aether-loadtools` only re-export it (issue #212). Keep it
// that way instead of growing a second copy here.
pub use fixtures::test_trace_id;
pub use server::{reserve_local_port, PortReservation, ReservedListener, SpawnedServer};
pub use tracing::{init_test_runtime, init_test_runtime_for, test_runtime_config};
pub use wait::wait_until;

#[cfg(feature = "gateway")]
pub use execution_runtime::{ExecutionRuntimeHarness, ExecutionRuntimeHarnessConfig};
#[cfg(feature = "gateway")]
pub use gateway::{GatewayHarness, GatewayHarnessConfig, GATEWAY_HARNESS_API_KEY};
#[cfg(feature = "postgres")]
pub use postgres::{prepare_aether_postgres_schema, ManagedPostgresServer};
#[cfg(feature = "gateway")]
pub use tunnel::{
    insert_tunnel_harness_auth_headers, TunnelHarness, TunnelHarnessConfig,
    TUNNEL_HARNESS_GENERATION, TUNNEL_HARNESS_MANAGEMENT_TOKEN, TUNNEL_HARNESS_NODE_ID,
};

#[cfg(test)]
mod managed_redis_single_impl_tests {
    // Compile-time proof that the re-exported type IS the single
    // aether-test-support implementation, not a parallel copy.
    #[test]
    fn reexported_managed_redis_is_the_support_implementation() {
        fn assert_same_type(
            server: crate::ManagedRedisServer,
        ) -> aether_test_support::ManagedRedisServer {
            server
        }
        let _ = assert_same_type
            as fn(crate::ManagedRedisServer) -> aether_test_support::ManagedRedisServer;
    }
}
