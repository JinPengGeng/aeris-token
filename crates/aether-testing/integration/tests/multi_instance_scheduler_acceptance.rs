use std::sync::Arc;
use std::time::Duration;

use aether_gateway::GatewayDataConfig;
use aether_runtime_state::{RedisClientConfig, RuntimeState};
use aether_testkit::{
    prepare_aether_postgres_schema, AcceptanceNamespace, GatewayHarness, GatewayHarnessConfig,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

const HARD_TIMEOUT: Duration = Duration::from_secs(30);

fn required_env(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| panic!("{name} is mandatory for multi-instance acceptance tests"))
}

#[tokio::test]
#[ignore = "requires mandatory Redis and Postgres services; CI runs this test explicitly"]
async fn backend_reachability_two_gateways_share_redis_and_postgres() {
    tokio::time::timeout(HARD_TIMEOUT, run_backend_substrate())
        .await
        .expect("multi-instance substrate exceeded its 30 second hard timeout")
        .expect("multi-instance substrate failed");
}

async fn run_backend_substrate() -> Result<(), Box<dyn std::error::Error>> {
    let redis_url = required_env("AETHER_SCHEDULER_ACCEPTANCE_REDIS_URL");
    let postgres_url = required_env("AETHER_SCHEDULER_ACCEPTANCE_POSTGRES_URL");
    let namespace = AcceptanceNamespace::new("shared-backends");

    prepare_aether_postgres_schema(&postgres_url).await?;

    let redis_config = RedisClientConfig {
        url: redis_url,
        key_prefix: Some(namespace.as_str().to_string()),
    };
    let redis_a = Arc::new(RuntimeState::redis(redis_config.clone(), Some(2_000)).await?);
    let redis_b = Arc::new(RuntimeState::redis(redis_config, Some(2_000)).await?);
    let redis_key = namespace.key("cross-instance");
    redis_a
        .kv_set(
            &redis_key,
            "written-by-gateway-a",
            Some(Duration::from_secs(60)),
        )
        .await?;
    let observed = redis_b.kv_get(&redis_key).await?;
    if observed.as_deref() != Some("written-by-gateway-a") {
        return Err(
            format!("gateway-b did not observe gateway-a Redis write: {observed:?}").into(),
        );
    }

    let sql_a = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&postgres_url)
        .await?;
    let sql_b = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&postgres_url)
        .await?;
    let sql_namespace = SqlNamespace::create(sql_a.clone(), namespace.sql_identifier()).await?;
    let create_marker = format!(
        "CREATE TABLE {}.shared_gateway_marker (value TEXT NOT NULL)",
        sql_namespace.quoted()
    );
    sqlx::query(&create_marker).execute(&sql_a).await?;
    let insert_marker = format!(
        "INSERT INTO {}.shared_gateway_marker(value) VALUES ($1)",
        sql_namespace.quoted()
    );
    sqlx::query(&insert_marker)
        .bind("seeded-for-both-gateways")
        .execute(&sql_a)
        .await?;
    let read_marker = format!(
        "SELECT value FROM {}.shared_gateway_marker",
        sql_namespace.quoted()
    );
    let marker: String = sqlx::query_scalar(&read_marker).fetch_one(&sql_b).await?;
    if marker != "seeded-for-both-gateways" {
        return Err("gateway-b SQL client did not observe the isolated seed".into());
    }
    let mut connection_a = sql_a.acquire().await?;
    sqlx::query("SELECT set_config('application_name', $1, false)")
        .bind(namespace.as_str())
        .execute(&mut *connection_a)
        .await?;
    let backend_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *connection_a)
        .await?;
    let observed: Option<String> = sqlx::query_scalar(
        "SELECT application_name FROM pg_stat_activity WHERE pid = $1 AND datname = current_database()",
    )
    .bind(backend_pid)
    .fetch_one(&sql_b)
    .await?;
    if observed.as_deref() != Some(namespace.as_str()) {
        return Err(
            format!("gateway-b did not observe gateway-a SQL session: {observed:?}").into(),
        );
    }

    let data_config = GatewayDataConfig::from_postgres_url(&postgres_url, false);
    let gateway_a = GatewayHarness::start(GatewayHarnessConfig {
        upstream_base_url: "http://127.0.0.1:9".to_string(),
        data_config: Some(data_config.clone()),
        max_in_flight_requests: Some(4),
        distributed_request_gate: None,
        runtime_state: Some(redis_a.clone()),
        seed_pressure_catalog: false,
        tunnel_instance_id: Some(format!("{}:gateway-a", namespace.as_str())),
        tunnel_relay_base_url: None,
    })
    .await
    .map_err(std::io::Error::other)?;
    let gateway_b = GatewayHarness::start(GatewayHarnessConfig {
        upstream_base_url: "http://127.0.0.1:9".to_string(),
        data_config: Some(data_config),
        max_in_flight_requests: Some(4),
        distributed_request_gate: None,
        runtime_state: Some(redis_b.clone()),
        seed_pressure_catalog: false,
        tunnel_instance_id: Some(format!("{}:gateway-b", namespace.as_str())),
        tunnel_relay_base_url: None,
    })
    .await
    .map_err(std::io::Error::other)?;
    if gateway_a.port() == gateway_b.port() {
        return Err("gateway instances unexpectedly share a listener".into());
    }

    redis_a.kv_delete(&redis_key).await?;
    sql_namespace.cleanup().await?;
    Ok(())
}

struct SqlNamespace {
    pool: PgPool,
    name: String,
    cleaned: bool,
}

impl SqlNamespace {
    async fn create(pool: PgPool, name: String) -> Result<Self, sqlx::Error> {
        let quoted = quote_identifier(&name);
        sqlx::query(&format!("CREATE SCHEMA {quoted}"))
            .execute(&pool)
            .await?;
        Ok(Self {
            pool,
            name,
            cleaned: false,
        })
    }

    fn quoted(&self) -> String {
        quote_identifier(&self.name)
    }

    async fn cleanup(mut self) -> Result<(), sqlx::Error> {
        let result = sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.quoted()))
            .execute(&self.pool)
            .await
            .map(|_| ());
        self.cleaned = result.is_ok();
        result
    }
}

impl Drop for SqlNamespace {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        let pool = self.pool.clone();
        let statement = format!("DROP SCHEMA {} CASCADE", self.quoted());
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
