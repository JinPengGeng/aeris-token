use std::sync::Arc;
use std::time::Duration;

use aether_gateway::GatewayDataConfig;
use aether_runtime_state::{RedisClientConfig, RuntimeState};
use aether_testkit::{
    prepare_aether_postgres_schema, AcceptanceNamespace, CountingUpstreamRecorder, GatewayHarness,
    GatewayHarnessConfig, GATEWAY_HARNESS_API_KEY,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

const HARD_TIMEOUT: Duration = Duration::from_secs(30);

macro_rules! blocked_scenario {
    ($name:ident, $scenario:literal, $blocker:literal) => {
        #[tokio::test]
        #[ignore = "Issue #47 production adapter is not integrated; explicit execution fails closed"]
        async fn $name() {
            tokio::time::timeout(HARD_TIMEOUT, run_blocked_entry($scenario, $blocker))
                .await
                .expect("Issue #47 E2E entry exceeded its 30 second hard timeout")
                .expect("Issue #47 E2E entry is blocked");
        }
    };
}

blocked_scenario!(
    exactly_one_half_open_probe_e2e,
    "half-open-probe",
    "#52 must expose probe claim/rejection, fencing token, lease lifecycle and SQL completion observation"
);
blocked_scenario!(
    affinity_order_and_failover_e2e,
    "affinity-failover",
    "the production gateway must expose deterministic affinity/failover observation and fault injection"
);
blocked_scenario!(
    pool_stampede_prevention_e2e,
    "pool-stampede",
    "the production pool must expose credential lease ownership and a pre-send concurrency checkpoint"
);
blocked_scenario!(
    frozen_snapshot_pagination_and_skip_e2e,
    "frozen-pagination",
    "#50 must expose real generation, rank, page cursor and current-page skip observation"
);
blocked_scenario!(
    send_time_admission_no_send_e2e,
    "send-admission",
    "#51 must expose a pre-send barrier plus authoritative admission decision events"
);
blocked_scenario!(
    post_client_commit_no_replay_http_and_ws_e2e,
    "post-commit-no-replay",
    "#46/#49 must expose HTTP and WebSocket client-commit boundaries and post-commit fault injection"
);
blocked_scenario!(
    request_wide_attempt_budget_and_deadline_e2e,
    "attempt-budget",
    "#53 must expose request-wide budget charges, terminal reason and injectable monotonic clock"
);

async fn run_blocked_entry(
    scenario: &str,
    blocker: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let redis_url = required_env("AETHER_SCHEDULER_ACCEPTANCE_REDIS_URL")?;
    let postgres_url = required_env("AETHER_SCHEDULER_ACCEPTANCE_POSTGRES_URL")?;
    prepare_aether_postgres_schema(&postgres_url).await?;

    let namespace = AcceptanceNamespace::new(scenario);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&postgres_url)
        .await?;
    let schema = namespace.sql_identifier();
    sqlx::query(&format!("CREATE SCHEMA {}", quote_identifier(&schema)))
        .execute(&pool)
        .await?;
    let mut sql_cleanup = SqlCleanup::new(pool, schema);
    let marker_table = format!("{}.scenario_seed", quote_identifier(&sql_cleanup.schema));
    sqlx::query(&format!(
        "CREATE TABLE {marker_table} (scenario TEXT NOT NULL)"
    ))
    .execute(&sql_cleanup.pool)
    .await?;
    sqlx::query(&format!("INSERT INTO {marker_table}(scenario) VALUES ($1)"))
        .bind(scenario)
        .execute(&sql_cleanup.pool)
        .await?;

    let redis_config = RedisClientConfig {
        url: redis_url,
        key_prefix: Some(namespace.as_str().to_string()),
    };
    let runtime_a = Arc::new(RuntimeState::redis(redis_config.clone(), Some(2_000)).await?);
    let runtime_b = Arc::new(RuntimeState::redis(redis_config, Some(2_000)).await?);
    let seed_key = namespace.key("scenario-seed");
    runtime_a
        .kv_set(&seed_key, scenario, Some(Duration::from_secs(60)))
        .await?;
    let mut redis_cleanup = RedisCleanup::new(runtime_a.clone(), seed_key.clone());
    if runtime_b.kv_get(&seed_key).await?.as_deref() != Some(scenario) {
        return Err("gateway-b runtime did not observe gateway-a Redis scenario seed".into());
    }

    let recorder = CountingUpstreamRecorder::start(format!("{scenario}-candidate")).await?;
    let data_config = GatewayDataConfig::from_postgres_url(&postgres_url, false);
    let gateway_a = start_gateway(
        recorder.base_url(),
        data_config.clone(),
        runtime_a.clone(),
        format!("{}:gateway-a", namespace.as_str()),
    )
    .await?;
    let gateway_b = start_gateway(
        recorder.base_url(),
        data_config,
        runtime_b,
        format!("{}:gateway-b", namespace.as_str()),
    )
    .await?;

    send_baseline_request(gateway_a.base_url(), &format!("{scenario}-a")).await?;
    send_baseline_request(gateway_b.base_url(), &format!("{scenario}-b")).await?;
    let network = recorder.snapshot();
    if network.len() != 2 {
        return Err(format!(
            "gateway-to-recorder bootstrap expected 2 network sends, observed {}",
            network.len()
        )
        .into());
    }

    redis_cleanup.cleanup().await?;
    sql_cleanup.cleanup().await?;
    Err(format!(
        "scenario '{scenario}' intentionally fails closed: {blocker}; baseline HTTP plumbing passed but is not acceptance"
    )
    .into())
}

async fn start_gateway(
    upstream: &str,
    data_config: GatewayDataConfig,
    runtime_state: Arc<RuntimeState>,
    instance_id: String,
) -> Result<GatewayHarness, std::io::Error> {
    GatewayHarness::start(GatewayHarnessConfig {
        upstream_base_url: upstream.to_string(),
        data_config: Some(data_config),
        max_in_flight_requests: Some(4),
        distributed_request_gate: None,
        runtime_state: Some(runtime_state),
        seed_pressure_catalog: true,
        tunnel_instance_id: Some(instance_id),
        tunnel_relay_base_url: None,
    })
    .await
    .map_err(std::io::Error::other)
}

async fn send_baseline_request(
    gateway_base_url: &str,
    request_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = reqwest::Client::new()
        .post(format!("{gateway_base_url}/v1/chat/completions"))
        .bearer_auth(GATEWAY_HARNESS_API_KEY)
        .header("x-aether-acceptance-request", request_id)
        .header("x-aether-acceptance-attempt", "1")
        .header("x-aether-acceptance-credential", "baseline")
        .json(&json!({
            "model": "gpt-5",
            "stream": false,
            "messages": [{"role": "user", "content": "issue47 acceptance bootstrap"}]
        }))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(format!(
            "gateway bootstrap request failed with {}",
            response.status()
        )
        .into());
    }
    Ok(())
}

fn required_env(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} is mandatory; backend absence is not a skip").into())
}

struct SqlCleanup {
    pool: sqlx::PgPool,
    schema: String,
    cleaned: bool,
}

struct RedisCleanup {
    runtime: Arc<RuntimeState>,
    key: String,
    cleaned: bool,
}

impl RedisCleanup {
    fn new(runtime: Arc<RuntimeState>, key: String) -> Self {
        Self {
            runtime,
            key,
            cleaned: false,
        }
    }

    async fn cleanup(&mut self) -> Result<(), aether_runtime_state::DataLayerError> {
        self.runtime.kv_delete(&self.key).await?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for RedisCleanup {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        let runtime = self.runtime.clone();
        let key = self.key.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = runtime.kv_delete(&key).await;
            });
        }
    }
}

impl SqlCleanup {
    fn new(pool: sqlx::PgPool, schema: String) -> Self {
        Self {
            pool,
            schema,
            cleaned: false,
        }
    }

    async fn cleanup(&mut self) -> Result<(), sqlx::Error> {
        sqlx::query(&format!(
            "DROP SCHEMA {} CASCADE",
            quote_identifier(&self.schema)
        ))
        .execute(&self.pool)
        .await?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for SqlCleanup {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        let pool = self.pool.clone();
        let statement = format!("DROP SCHEMA {} CASCADE", quote_identifier(&self.schema));
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = sqlx::query(&statement).execute(&pool).await;
            });
        }
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
