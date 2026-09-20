// Gateway-backed benchmark scenarios live outside the reusable testkit.
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aether_gateway::tunnel_protocol as protocol;
use aether_gateway::GatewayDataConfig;
use aether_runtime_state::{RedisClientConfig, RuntimeState};
use aether_testkit::{
    init_test_runtime_for, insert_tunnel_harness_auth_headers, prepare_aether_postgres_schema,
    run_http_load_probe, wait_until, GatewayHarness, GatewayHarnessConfig, HttpLoadProbeConfig,
    HttpLoadProbeResponseMode, HttpLoadProbeResult, ManagedPostgresServer, ManagedRedisServer,
    ReservedListener, TUNNEL_HARNESS_GENERATION, TUNNEL_HARNESS_MANAGEMENT_TOKEN,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::Method;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const PROXY_TUNNEL_PATH: &str = "/api/internal/proxy-tunnel";
const TUNNEL_RELAY_PATH_PREFIX: &str = "/api/internal/tunnel/relay";
const NODE_ID: &str = "node-owner-relay-baseline";
const RELAY_AUTH_SECRET: &[u8] = b"tunnel-harness-relay-secret-32-bytes-minimum";

#[derive(Debug, Clone)]
struct MultiInstanceOwnerRelayBaselineConfig {
    total_requests: usize,
    concurrency: usize,
    timeout: Duration,
    chunk_delay: Duration,
    output_path: Option<PathBuf>,
    redis_url: Option<String>,
    postgres_url: Option<String>,
}

impl Default for MultiInstanceOwnerRelayBaselineConfig {
    fn default() -> Self {
        Self {
            total_requests: 200,
            concurrency: 20,
            timeout: Duration::from_secs(10),
            chunk_delay: Duration::ZERO,
            output_path: None,
            redis_url: None,
            postgres_url: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct MultiInstanceOwnerRelayBaselineReport {
    suite: &'static str,
    redis_url: String,
    runtime_key_prefix: String,
    owner_runtime_backend: &'static str,
    forwarder_runtime_backend: &'static str,
    redis_attachment_observed: bool,
    postgres_url: String,
    owner_instance_id: &'static str,
    forwarder_instance_id: &'static str,
    direct_owner_relay: HttpLoadProbeResult,
    remote_owner_relay: HttpLoadProbeResult,
    relay_overhead_ms: RelayOverheadSnapshot,
}

#[derive(Debug, Serialize)]
struct RelayOverheadSnapshot {
    p50_delta_ms: i64,
    p95_delta_ms: i64,
    max_delta_ms: i64,
    mean_delta_ms: i64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _log_shutdown = aether_runtime::LogShutdownGuard::new();
    run()
}

#[tokio::main]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    init_test_runtime_for("multi-instance-owner-relay-baseline");
    let config = parse_args(std::env::args().skip(1).collect())?;
    let report = run_suite(&config).await?;
    let raw = serde_json::to_string_pretty(&report)?;
    println!("{raw}");
    if let Some(path) = config.output_path.as_ref() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("{raw}\n"))?;
    }
    Ok(())
}

async fn run_suite(
    config: &MultiInstanceOwnerRelayBaselineConfig,
) -> Result<MultiInstanceOwnerRelayBaselineReport, Box<dyn std::error::Error>> {
    let managed_redis = if config.redis_url.is_none() {
        Some(ManagedRedisServer::start().await?)
    } else {
        None
    };
    let redis_url = config
        .redis_url
        .clone()
        .or_else(|| {
            managed_redis
                .as_ref()
                .map(|server| server.redis_url().to_string())
        })
        .expect("redis url should be resolved");

    let runtime_key_prefix = format!("aether-owner-relay-baseline-{}", uuid::Uuid::now_v7());
    // Each Gateway owns independent Redis clients in the same run-scoped keyspace.
    // Construction and explicit PING must succeed; there is no memory fallback.
    let owner_runtime = redis_runtime_state(&redis_url, &runtime_key_prefix).await?;
    let forwarder_runtime = redis_runtime_state(&redis_url, &runtime_key_prefix).await?;
    let owner_runtime_backend = owner_runtime.backend_kind().as_str();
    let forwarder_runtime_backend = forwarder_runtime.backend_kind().as_str();

    let managed_postgres = if config.postgres_url.is_none() {
        Some(ManagedPostgresServer::start().await?)
    } else {
        None
    };
    let postgres_url = config
        .postgres_url
        .clone()
        .or_else(|| {
            managed_postgres
                .as_ref()
                .map(|server| server.database_url().to_string())
        })
        .expect("postgres url should be resolved");

    prepare_aether_postgres_schema(&postgres_url).await?;
    seed_tunnel_auth(&postgres_url).await?;

    let shared_data = GatewayDataConfig::from_postgres_url(postgres_url.clone(), false);

    let owner_listener = ReservedListener::bind().await?;
    let forwarder_listener = ReservedListener::bind().await?;
    let owner_base_url = owner_listener.base_url();
    let forwarder_base_url = forwarder_listener.base_url();

    let owner_gateway = GatewayHarness::start_with_listener_and_runtime_state(
        GatewayHarnessConfig {
            upstream_base_url: "http://127.0.0.1:1".to_string(),
            data_config: Some(shared_data.clone()),
            max_in_flight_requests: None,
            distributed_request_gate: None,
            tunnel_instance_id: Some("gateway-owner".to_string()),
            tunnel_relay_base_url: Some(owner_base_url.clone()),
        },
        owner_listener,
        owner_runtime,
    )
    .await?;
    let forwarder_gateway = GatewayHarness::start_with_listener_and_runtime_state(
        GatewayHarnessConfig {
            upstream_base_url: "http://127.0.0.1:1".to_string(),
            data_config: Some(shared_data),
            max_in_flight_requests: None,
            distributed_request_gate: None,
            tunnel_instance_id: Some("gateway-forwarder".to_string()),
            tunnel_relay_base_url: Some(forwarder_base_url.clone()),
        },
        forwarder_listener,
        forwarder_runtime.clone(),
    )
    .await?;

    let peer = connect_protocol_peer(owner_gateway.base_url(), config.chunk_delay).await?;

    wait_for_owner_attachment(&forwarder_base_url).await?;
    let attachment = forwarder_runtime
        .kv_get(&format!("tunnel:attachments:{NODE_ID}"))
        .await?
        .ok_or_else(|| std::io::Error::other("owner attachment missing from Redis"))?;
    let attachment: serde_json::Value = serde_json::from_str(&attachment)?;
    if attachment["gateway_instance_id"] != "gateway-owner"
        || attachment["relay_base_url"] != owner_base_url
    {
        return Err(std::io::Error::other("Redis owner attachment identity mismatch").into());
    }

    let direct_owner_relay = run_http_load_probe(&HttpLoadProbeConfig {
        url: format!(
            "{owner_base}{TUNNEL_RELAY_PATH_PREFIX}/{NODE_ID}",
            owner_base = owner_gateway.base_url()
        ),
        method: Method::POST,
        headers: relay_headers(&relay_envelope(), "gateway-owner"),
        body: Some(relay_envelope()),
        total_requests: config.total_requests,
        concurrency: config.concurrency,
        timeout: config.timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
        ..HttpLoadProbeConfig::default()
    })
    .await
    .map_err(std::io::Error::other)?;

    let remote_owner_relay = run_http_load_probe(&HttpLoadProbeConfig {
        url: format!(
            "{forwarder_base}{TUNNEL_RELAY_PATH_PREFIX}/{NODE_ID}",
            forwarder_base = forwarder_gateway.base_url()
        ),
        method: Method::POST,
        headers: relay_headers(&relay_envelope(), "gateway-forwarder"),
        body: Some(relay_envelope()),
        total_requests: config.total_requests,
        concurrency: config.concurrency,
        timeout: config.timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
        ..HttpLoadProbeConfig::default()
    })
    .await
    .map_err(std::io::Error::other)?;

    drop(peer);
    drop(forwarder_gateway);
    drop(owner_gateway);

    Ok(MultiInstanceOwnerRelayBaselineReport {
        suite: "multi_instance_owner_relay_baseline",
        redis_url,
        runtime_key_prefix,
        owner_runtime_backend,
        forwarder_runtime_backend,
        redis_attachment_observed: true,
        postgres_url,
        owner_instance_id: "gateway-owner",
        forwarder_instance_id: "gateway-forwarder",
        relay_overhead_ms: RelayOverheadSnapshot {
            p50_delta_ms: remote_owner_relay.p50_ms as i64 - direct_owner_relay.p50_ms as i64,
            p95_delta_ms: remote_owner_relay.p95_ms as i64 - direct_owner_relay.p95_ms as i64,
            max_delta_ms: remote_owner_relay.max_ms as i64 - direct_owner_relay.max_ms as i64,
            mean_delta_ms: remote_owner_relay.mean_ms as i64 - direct_owner_relay.mean_ms as i64,
        },
        direct_owner_relay,
        remote_owner_relay,
    })
}

async fn redis_runtime_state(
    redis_url: &str,
    key_prefix: &str,
) -> Result<Arc<RuntimeState>, Box<dyn std::error::Error>> {
    let runtime = RuntimeState::redis(
        RedisClientConfig {
            url: redis_url.to_string(),
            key_prefix: Some(key_prefix.to_string()),
        },
        Some(1_000),
    )
    .await?;
    runtime.ping().await?;
    Ok(Arc::new(runtime))
}

async fn wait_for_owner_attachment(forwarder_base_url: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|err| format!("failed to build readiness client: {err}"))?;
    let target_url = format!("{forwarder_base_url}{TUNNEL_RELAY_PATH_PREFIX}/{NODE_ID}");
    let ready = wait_until(Duration::from_secs(10), Duration::from_millis(100), || {
        let client = client.clone();
        let target_url = target_url.clone();
        async move {
            let envelope = relay_envelope();
            let response = client
                .post(target_url)
                .headers(
                    relay_headers(&envelope, "gateway-forwarder")
                        .into_iter()
                        .map(|(key, value)| {
                            (
                                http::header::HeaderName::from_bytes(key.as_bytes()).unwrap(),
                                http::header::HeaderValue::from_str(&value).unwrap(),
                            )
                        })
                        .collect(),
                )
                .header("content-type", "application/octet-stream")
                .body(envelope)
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => match response.text().await {
                    Ok(body) => body == "owner-relay-ok",
                    Err(_) => false,
                },
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    eprintln!("owner attachment probe returned {status}: {body}");
                    false
                }
                _ => false,
            }
        }
    })
    .await;
    if ready {
        Ok(())
    } else {
        Err("timed out waiting for owner attachment propagation".to_string())
    }
}

fn relay_headers(
    envelope: &[u8],
    owner_instance_id: &str,
) -> std::collections::BTreeMap<String, String> {
    let metadata_len =
        u32::from_be_bytes(envelope[..4].try_into().expect("relay envelope header")) as usize;
    let metadata_end = 4 + metadata_len;
    let metadata = &envelope[..metadata_end];
    let body = &envelope[metadata_end..];
    let digest = aether_contracts::tunnel::tunnel_relay_payload_digest(metadata, body);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after epoch")
        .as_secs();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let signature = aether_contracts::tunnel::sign_tunnel_relay_request(
        RELAY_AUTH_SECRET,
        "load-probe",
        owner_instance_id,
        NODE_ID,
        "",
        false,
        timestamp,
        &nonce,
        &digest,
    );
    std::collections::BTreeMap::from([
        (
            "content-type".to_string(),
            "application/octet-stream".to_string(),
        ),
        (
            aether_contracts::tunnel::TUNNEL_RELAY_AUTH_SENDER_HEADER.to_string(),
            "load-probe".to_string(),
        ),
        (
            aether_contracts::tunnel::TUNNEL_RELAY_OWNER_INSTANCE_HEADER.to_string(),
            owner_instance_id.to_string(),
        ),
        (
            aether_contracts::tunnel::TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER.to_string(),
            timestamp.to_string(),
        ),
        (
            aether_contracts::tunnel::TUNNEL_RELAY_AUTH_NONCE_HEADER.to_string(),
            nonce,
        ),
        (
            aether_contracts::tunnel::TUNNEL_RELAY_AUTH_PAYLOAD_HEADER.to_string(),
            digest.encode_header_value(),
        ),
        (
            aether_contracts::tunnel::TUNNEL_RELAY_AUTH_SIGNATURE_HEADER.to_string(),
            signature,
        ),
    ])
}

fn relay_envelope() -> Vec<u8> {
    let meta = protocol::RequestMeta {
        method: "POST".to_string(),
        url: "https://owner-relay.example/v1/chat/completions".to_string(),
        headers: std::collections::HashMap::from([(
            "content-type".to_string(),
            "application/json".to_string(),
        )]),
        stream: false,
        request_timeout_ms: None,
        stream_first_byte_timeout_ms: None,
        timeout: 30,
        follow_redirects: None,
        http1_only: false,
        provider_id: None,
        endpoint_id: None,
        key_id: None,
        transport_profile: None,
    };
    let meta_json = serde_json::to_vec(&meta).expect("owner relay metadata should serialize");
    let body = br#"{"model":"gpt-5","messages":[{"role":"user","content":"owner relay"}]}"#;
    let mut envelope = Vec::with_capacity(4 + meta_json.len() + body.len());
    envelope.extend_from_slice(&(meta_json.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&meta_json);
    envelope.extend_from_slice(body);
    envelope
}

async fn connect_protocol_peer(
    gateway_base_url: &str,
    chunk_delay: Duration,
) -> Result<tokio::task::JoinHandle<()>, Box<dyn std::error::Error>> {
    let ws_url = format!(
        "{}{}",
        gateway_base_url.replace("http://", "ws://"),
        PROXY_TUNNEL_PATH
    );
    let request = ws_url.into_client_request()?;
    let mut request = request;
    insert_tunnel_harness_auth_headers(request.headers_mut(), NODE_ID)?;
    request.headers_mut().insert(
        aether_contracts::tunnel::TUNNEL_PROTOCOL_VERSION_HEADER,
        http::HeaderValue::from_static(
            aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION_STR,
        ),
    );
    request.headers_mut().insert(
        "x-node-name",
        http::HeaderValue::from_static("proxy-owner-relay-baseline"),
    );
    request.headers_mut().insert(
        "x-tunnel-max-streams",
        http::HeaderValue::from_static("256"),
    );

    let (socket, _response) = tokio_tungstenite::connect_async(request).await?;
    let (mut sink, mut stream) = socket.split();
    sink.send(Message::Binary(
        protocol::encode_hello(&protocol::HelloPayload {
            protocol_version: aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION,
            capabilities: vec![
                "flow-control".to_string(),
                "reset-stream".to_string(),
                "graceful-drain".to_string(),
            ],
            session_id: Some("owner-relay-baseline-session".to_string()),
            replica_id: Some("owner-relay-baseline-replica".to_string()),
        })
        .into(),
    ))
    .await?;
    sink.send(Message::Binary(
        protocol::encode_settings(&protocol::SettingsPayload {
            initial_stream_window_bytes: 4 * 1024 * 1024,
            min_window_update_bytes: 1024 * 1024,
            drain_deadline_ms: 30_000,
        })
        .into(),
    ))
    .await?;
    Ok(tokio::spawn(async move {
        while let Some(message) = stream.next().await {
            let Ok(message) = message else {
                break;
            };
            match message {
                Message::Binary(data)
                    if handle_binary_frame(&mut sink, data.to_vec(), chunk_delay)
                        .await
                        .is_err() =>
                {
                    break;
                }
                Message::Ping(payload)
                    if sink.send(Message::Pong(payload.clone())).await.is_err() =>
                {
                    break;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = sink.close().await;
    }))
}

async fn seed_tunnel_auth(postgres_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    const USER_ID: &str = "user-owner-relay-baseline";
    const TOKEN_ID: &str = "token-owner-relay-baseline";

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(postgres_url)
        .await?;
    let mut transaction = pool.begin().await?;
    sqlx::query(
        r#"
INSERT INTO users (
  id, email, username, role, auth_source, email_verified, is_active, is_deleted,
  created_at, updated_at
) VALUES ($1, $2, $3, 'admin', 'local', TRUE, TRUE, FALSE, now(), now())
ON CONFLICT (id) DO UPDATE SET
  email = EXCLUDED.email,
  username = EXCLUDED.username,
  role = 'admin',
  auth_source = 'local',
  email_verified = TRUE,
  is_active = TRUE,
  is_deleted = FALSE,
  updated_at = EXCLUDED.updated_at
"#,
    )
    .bind(USER_ID)
    .bind("owner-relay-baseline@example.com")
    .bind("owner_relay_baseline_admin")
    .execute(&mut *transaction)
    .await?;

    let token_hash = format!(
        "{:x}",
        Sha256::digest(TUNNEL_HARNESS_MANAGEMENT_TOKEN.as_bytes())
    );
    sqlx::query(
        r#"
INSERT INTO management_tokens (
  id, user_id, name, token_hash, token_prefix, permissions, usage_count,
  is_active, created_at, updated_at
) VALUES ($1, $2, $3, $4, $6, $5, 0, TRUE, now(), now())
ON CONFLICT (id) DO UPDATE SET
  user_id = EXCLUDED.user_id,
  name = EXCLUDED.name,
  token_hash = EXCLUDED.token_hash,
  token_prefix = EXCLUDED.token_prefix,
  permissions = EXCLUDED.permissions,
  is_active = TRUE,
  updated_at = EXCLUDED.updated_at
"#,
    )
    .bind(TOKEN_ID)
    .bind(USER_ID)
    .bind("owner relay tunnel token")
    .bind(token_hash)
    .bind(serde_json::json!(["admin:proxy_nodes:admin"]))
    .bind(
        TUNNEL_HARNESS_MANAGEMENT_TOKEN
            .chars()
            .take(12)
            .collect::<String>(),
    )
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        r#"
INSERT INTO proxy_nodes (
  id, tunnel_generation, name, ip, port, status, heartbeat_interval,
  active_connections, total_requests, is_manual, created_at, updated_at,
  config_version, tunnel_mode, tunnel_connected, failed_requests, dns_failures,
  stream_errors
) VALUES (
  $1, $2, 'owner relay baseline node', '127.0.0.1', 0, 'offline', 30,
  0, 0, FALSE, now(), now(), 0, TRUE, FALSE, 0, 0, 0
)
ON CONFLICT (id) DO UPDATE SET
  tunnel_generation = EXCLUDED.tunnel_generation,
  name = EXCLUDED.name,
  ip = EXCLUDED.ip,
  port = EXCLUDED.port,
  status = 'offline',
  active_connections = 0,
  tunnel_mode = TRUE,
  tunnel_connected = FALSE,
  updated_at = EXCLUDED.updated_at
"#,
    )
    .bind(NODE_ID)
    .bind(TUNNEL_HARNESS_GENERATION)
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;
    Ok(())
}

async fn handle_binary_frame<S>(
    sink: &mut S,
    data: Vec<u8>,
    chunk_delay: Duration,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let Some(header) = protocol::FrameHeader::parse(&data) else {
        return Ok(());
    };
    match header.msg_type {
        protocol::PING => {
            let payload = protocol::frame_payload_by_header(&data, &header).unwrap_or(&[]);
            sink.send(Message::Binary(protocol::encode_pong(payload).into()))
                .await?;
        }
        protocol::REQUEST_HEADERS => {
            let payload = protocol::decode_payload(&data, &header).unwrap_or_default();
            let _ = serde_json::from_slice::<protocol::RequestMeta>(&payload);
        }
        protocol::REQUEST_BODY => {
            let payload = protocol::decode_payload(&data, &header).unwrap_or_default();
            if !payload.is_empty() {
                sink.send(Message::Binary(
                    protocol::encode_window_update(header.stream_id, payload.len() as u32).into(),
                ))
                .await?;
            }
            if header.flags & protocol::FLAG_END_STREAM == 0 {
                return Ok(());
            }
            let response_meta = protocol::ResponseMeta {
                status: 200,
                headers: vec![(
                    "content-type".to_string(),
                    "text/plain; charset=utf-8".to_string(),
                )],
            };
            let response_meta_json =
                serde_json::to_vec(&response_meta).expect("response metadata should serialize");
            sink.send(Message::Binary(
                protocol::encode_frame(
                    header.stream_id,
                    protocol::RESPONSE_HEADERS,
                    0,
                    &response_meta_json,
                )
                .into(),
            ))
            .await?;

            for chunk in [b"owner-".as_slice(), b"relay-".as_slice(), b"ok".as_slice()] {
                if !chunk_delay.is_zero() {
                    tokio::time::sleep(chunk_delay).await;
                }
                sink.send(Message::Binary(
                    protocol::encode_frame(header.stream_id, protocol::RESPONSE_BODY, 0, chunk)
                        .into(),
                ))
                .await?;
            }

            sink.send(Message::Binary(
                protocol::encode_frame(header.stream_id, protocol::STREAM_END, 0, &[]).into(),
            ))
            .await?;
        }
        _ => {}
    }
    Ok(())
}

fn parse_args(
    args: Vec<String>,
) -> Result<MultiInstanceOwnerRelayBaselineConfig, Box<dyn std::error::Error>> {
    let mut config = MultiInstanceOwnerRelayBaselineConfig::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--requests" => config.total_requests = next_value(&mut iter, "--requests")?.parse()?,
            "--concurrency" => {
                config.concurrency = next_value(&mut iter, "--concurrency")?.parse()?
            }
            "--timeout-ms" => {
                config.timeout =
                    Duration::from_millis(next_value(&mut iter, "--timeout-ms")?.parse()?)
            }
            "--chunk-delay-ms" => {
                config.chunk_delay =
                    Duration::from_millis(next_value(&mut iter, "--chunk-delay-ms")?.parse()?)
            }
            "--redis-url" => config.redis_url = Some(next_value(&mut iter, "--redis-url")?),
            "--postgres-url" => {
                config.postgres_url = Some(next_value(&mut iter, "--postgres-url")?)
            }
            "--output" => {
                config.output_path = Some(PathBuf::from(next_value(&mut iter, "--output")?))
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown argument: {other}"),
                )
                .into());
            }
        }
    }

    if config.total_requests == 0 || config.concurrency == 0 || config.timeout.is_zero() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "owner relay baseline numeric settings must be positive",
        )
        .into());
    }

    Ok(config)
}

fn next_value(
    iter: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    iter.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("missing value for {flag}"),
        )
        .into()
    })
}

fn print_usage() {
    println!(
        "usage: cargo run -p aether-integration-tests --bin multi_instance_owner_relay_baseline -- [--requests 200] [--concurrency 20] [--timeout-ms 10000] [--chunk-delay-ms 0] [--redis-url redis://127.0.0.1:6379/0] [--postgres-url postgres://127.0.0.1:5432/postgres] [--output /tmp/multi_instance_owner_relay_baseline.json]"
    );
}
