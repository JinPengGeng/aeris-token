// Gateway-backed benchmark scenarios live outside the reusable testkit.
use std::path::PathBuf;
use std::time::Duration;

use aether_gateway::tunnel_protocol as protocol;
use aether_testkit::{
    init_test_runtime_for, insert_tunnel_harness_auth_headers, run_http_load_probe, wait_until,
    HttpLoadProbeConfig, HttpLoadProbeResponseMode, HttpLoadProbeResult, TUNNEL_HARNESS_GENERATION,
    TUNNEL_HARNESS_MANAGEMENT_TOKEN,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Method;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const PROXY_TUNNEL_PATH: &str = "/api/internal/proxy-tunnel";
const TUNNEL_RELAY_PATH_PREFIX: &str = "/api/internal/tunnel/relay";
const NODE_ID: &str = "node-owner-relay-baseline";
const RELAY_AUTH_SECRET: &[u8] = b"tunnel-harness-relay-secret-32-bytes-minimum";

#[derive(Debug, Clone)]
struct DeployedOwnerRelayConfig {
    total_requests: usize,
    concurrency: usize,
    timeout: Duration,
    chunk_delay: Duration,
    output_path: Option<PathBuf>,
    redis_url: String,
    postgres_url: String,
    owner_url: String,
    forwarder_url: String,
}

async fn relay_load(
    base_url: &str,
    config: &DeployedOwnerRelayConfig,
    stream: bool,
    owner_instance_id: &str,
) -> Result<HttpLoadProbeResult, Box<dyn std::error::Error>> {
    let envelope = relay_envelope(stream);
    run_http_load_probe(&HttpLoadProbeConfig {
        url: format!("{base_url}{TUNNEL_RELAY_PATH_PREFIX}/{NODE_ID}"),
        method: Method::POST,
        header_sets: relay_header_sets(&envelope, owner_instance_id, config.total_requests),
        body: Some(envelope),
        total_requests: config.total_requests,
        concurrency: config.concurrency,
        timeout: config.timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
        ..HttpLoadProbeConfig::default()
    })
    .await
    .map_err(|error| std::io::Error::other(error).into())
}

async fn assert_redis_reachable(redis_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let authority = redis_url
        .strip_prefix("redis://")
        .ok_or("expected redis:// URL")?;
    let authority = authority
        .split('/')
        .next()
        .ok_or("Redis URL has no authority")?;
    if authority.contains('@') {
        return Err("probe currently requires an unauthenticated Redis fixture".into());
    }
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or("Redis fixture URL must include a port")?;
    let port: u16 = port.parse()?;
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut socket = tokio::net::TcpStream::connect((host, port)).await?;
        socket.write_all(b"*1\r\n$4\r\nPING\r\n").await?;
        let mut response = [0_u8; 16];
        let len = socket.read(&mut response).await?;
        if !response[..len].starts_with(b"+PONG") {
            return Err(std::io::Error::other("Redis PING did not return PONG"));
        }
        Ok::<(), std::io::Error>(())
    })
    .await??;
    Ok(())
}

impl Default for DeployedOwnerRelayConfig {
    fn default() -> Self {
        Self {
            total_requests: 200,
            concurrency: 20,
            timeout: Duration::from_secs(10),
            chunk_delay: Duration::ZERO,
            output_path: None,
            redis_url: String::new(),
            postgres_url: String::new(),
            owner_url: String::new(),
            forwarder_url: String::new(),
        }
    }
}

#[derive(Debug, Serialize)]
struct DeployedOwnerRelayReport {
    suite: &'static str,
    redis_url: String,
    postgres_url: String,
    owner_url: String,
    forwarder_url: String,
    direct_owner_relay: HttpLoadProbeResult,
    remote_owner_relay: HttpLoadProbeResult,
    streaming_remote_owner_relay: HttpLoadProbeResult,
    disconnected_status: Option<u16>,
    disconnected_error: Option<String>,
    disconnected_elapsed_ms: u64,
    recovered_remote_owner_relay: HttpLoadProbeResult,
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
    init_test_runtime_for("deployed-owner-relay-probe");
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
    config: &DeployedOwnerRelayConfig,
) -> Result<DeployedOwnerRelayReport, Box<dyn std::error::Error>> {
    assert_redis_reachable(&config.redis_url).await?;
    seed_tunnel_auth(&config.postgres_url).await?;
    let peer = connect_protocol_peer(&config.owner_url, config.chunk_delay).await?;
    wait_for_owner_attachment(&config.forwarder_url, "frontdoor-2").await?;

    let direct_envelope = relay_envelope(false);
    let direct_owner_relay = run_http_load_probe(&HttpLoadProbeConfig {
        url: format!(
            "{owner_base}{TUNNEL_RELAY_PATH_PREFIX}/{NODE_ID}",
            owner_base = config.owner_url
        ),
        method: Method::POST,
        header_sets: relay_header_sets(&direct_envelope, "frontdoor-1", config.total_requests),
        body: Some(direct_envelope),
        total_requests: config.total_requests,
        concurrency: config.concurrency,
        timeout: config.timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
        ..HttpLoadProbeConfig::default()
    })
    .await
    .map_err(std::io::Error::other)?;
    assert_load_success("direct owner relay", &direct_owner_relay)?;

    let remote_envelope = relay_envelope(false);
    let remote_owner_relay = run_http_load_probe(&HttpLoadProbeConfig {
        url: format!(
            "{forwarder_base}{TUNNEL_RELAY_PATH_PREFIX}/{NODE_ID}",
            forwarder_base = config.forwarder_url
        ),
        method: Method::POST,
        header_sets: relay_header_sets(&remote_envelope, "frontdoor-2", config.total_requests),
        body: Some(remote_envelope),
        total_requests: config.total_requests,
        concurrency: config.concurrency,
        timeout: config.timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
        ..HttpLoadProbeConfig::default()
    })
    .await
    .map_err(std::io::Error::other)?;
    assert_load_success("remote owner relay", &remote_owner_relay)?;

    let streaming_remote_owner_relay =
        relay_load(&config.forwarder_url, config, true, "frontdoor-2").await?;
    assert_load_success(
        "streaming remote owner relay",
        &streaming_remote_owner_relay,
    )?;
    assert_relay_body(&config.forwarder_url, true, "frontdoor-2", config.timeout).await?;
    peer.abort();
    let _ = peer.await;
    let started = std::time::Instant::now();
    let disconnected_envelope = relay_envelope(false);
    let disconnected_headers = relay_headers(&disconnected_envelope, "frontdoor-2");
    let disconnected = reqwest::Client::builder()
        .timeout(config.timeout)
        .build()?
        .post(format!(
            "{}{}/{}",
            config.forwarder_url, TUNNEL_RELAY_PATH_PREFIX, NODE_ID
        ))
        .header("content-type", "application/octet-stream")
        .headers(relay_header_map(disconnected_headers)?)
        .body(disconnected_envelope)
        .send()
        .await;
    let disconnected_elapsed_ms = started.elapsed().as_millis() as u64;
    let (disconnected_status, disconnected_error) = match disconnected {
        Ok(response) if response.status().is_success() => {
            return Err("relay succeeded after owner disconnect".into());
        }
        Ok(response) => (Some(response.status().as_u16()), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let peer = connect_protocol_peer(&config.owner_url, config.chunk_delay).await?;
    wait_for_owner_attachment(&config.forwarder_url, "frontdoor-2").await?;
    let recovered_remote_owner_relay =
        relay_load(&config.forwarder_url, config, false, "frontdoor-2").await?;
    assert_load_success(
        "recovered remote owner relay",
        &recovered_remote_owner_relay,
    )?;
    peer.abort();

    Ok(DeployedOwnerRelayReport {
        suite: "deployed_owner_relay_probe",
        redis_url: config.redis_url.clone(),
        postgres_url: config.postgres_url.clone(),
        owner_url: config.owner_url.clone(),
        forwarder_url: config.forwarder_url.clone(),
        streaming_remote_owner_relay,
        disconnected_status,
        disconnected_error,
        disconnected_elapsed_ms,
        recovered_remote_owner_relay,
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

fn assert_load_success(
    phase: &str,
    result: &HttpLoadProbeResult,
) -> Result<(), Box<dyn std::error::Error>> {
    if result.failed_requests != 0 || result.completed_requests != result.total_requests {
        return Err(format!(
            "{phase} failed: completed={}, failed={}, total={}",
            result.completed_requests, result.failed_requests, result.total_requests
        )
        .into());
    }
    if result
        .status_counts
        .keys()
        .any(|status| !(200..300).contains(status))
    {
        return Err(format!(
            "{phase} returned non-success status: {:?}",
            result.status_counts
        )
        .into());
    }
    Ok(())
}

async fn assert_relay_body(
    base_url: &str,
    stream: bool,
    owner_instance_id: &str,
    timeout: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let envelope = relay_envelope(stream);
    let response = reqwest::Client::builder()
        .timeout(timeout)
        .build()?
        .post(format!("{base_url}{TUNNEL_RELAY_PATH_PREFIX}/{NODE_ID}"))
        .headers(relay_header_map(relay_headers(
            &envelope,
            owner_instance_id,
        ))?)
        .body(envelope)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(format!("relay body check returned {}", response.status()).into());
    }
    if response.bytes().await?.as_ref() != b"owner-relay-ok" {
        return Err("relay body check returned unexpected bytes".into());
    }
    Ok(())
}

async fn wait_for_owner_attachment(
    forwarder_base_url: &str,
    owner_instance_id: &str,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|err| format!("failed to build readiness client: {err}"))?;
    let target_url = format!("{forwarder_base_url}{TUNNEL_RELAY_PATH_PREFIX}/{NODE_ID}");
    let ready = wait_until(Duration::from_secs(10), Duration::from_millis(100), || {
        let client = client.clone();
        let target_url = target_url.clone();
        let owner_instance_id = owner_instance_id.to_string();
        async move {
            let envelope = relay_envelope(false);
            let headers = match relay_header_map(relay_headers(&envelope, &owner_instance_id)) {
                Ok(headers) => headers,
                Err(_) => return false,
            };
            let response = client
                .post(target_url)
                .headers(headers)
                .body(envelope)
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => match response.text().await {
                    Ok(body) => body == "owner-relay-ok",
                    Err(_) => false,
                },
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

fn relay_header_sets(
    envelope: &[u8],
    owner_instance_id: &str,
    total_requests: usize,
) -> Vec<std::collections::BTreeMap<String, String>> {
    (0..total_requests.max(1))
        .map(|_| relay_headers(envelope, owner_instance_id))
        .collect()
}

fn relay_header_map(
    headers: std::collections::BTreeMap<String, String>,
) -> Result<HeaderMap, String> {
    let mut result = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::try_from(name.as_str())
            .map_err(|error| format!("invalid relay header name: {error}"))?;
        let value = HeaderValue::try_from(value.as_str())
            .map_err(|error| format!("invalid relay header value: {error}"))?;
        result.insert(name, value);
    }
    Ok(result)
}

fn relay_headers(
    envelope: &[u8],
    owner_instance_id: &str,
) -> std::collections::BTreeMap<String, String> {
    let metadata_len = u32::from_be_bytes(
        envelope[..4]
            .try_into()
            .expect("relay envelope header should be present"),
    ) as usize;
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

fn relay_envelope(stream: bool) -> Vec<u8> {
    let meta = protocol::RequestMeta {
        method: "POST".to_string(),
        url: "https://owner-relay.example/v1/chat/completions".to_string(),
        headers: std::collections::HashMap::from([(
            "content-type".to_string(),
            "application/json".to_string(),
        )]),
        stream,
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
    // The deployed gateway waits for the protocol HELLO and SETTINGS frames
    // before registering an authenticated tunnel as routable.  Without this
    // negotiation the HTTP upgrade succeeds, but the owner is never attached.
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
) VALUES ($1, $2, $3, $4, 'ae-tunnel-ha', $5, 0, TRUE, now(), now())
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
        protocol::REQUEST_BODY if header.flags & protocol::FLAG_END_STREAM != 0 => {
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

fn parse_args(args: Vec<String>) -> Result<DeployedOwnerRelayConfig, Box<dyn std::error::Error>> {
    let mut config = DeployedOwnerRelayConfig::default();
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
            "--redis-url" => config.redis_url = next_value(&mut iter, "--redis-url")?,
            "--postgres-url" => config.postgres_url = next_value(&mut iter, "--postgres-url")?,
            "--owner-url" => config.owner_url = next_value(&mut iter, "--owner-url")?,
            "--forwarder-url" => config.forwarder_url = next_value(&mut iter, "--forwarder-url")?,
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
    if [
        &config.redis_url,
        &config.postgres_url,
        &config.owner_url,
        &config.forwarder_url,
    ]
    .iter()
    .any(|value| value.is_empty())
    {
        return Err(
            "--redis-url, --postgres-url, --owner-url and --forwarder-url are required".into(),
        );
    }
    for (name, value) in [
        ("redis", &config.redis_url),
        ("postgres", &config.postgres_url),
        ("owner", &config.owner_url),
        ("forwarder", &config.forwarder_url),
    ] {
        let parsed =
            reqwest::Url::parse(value).map_err(|error| format!("invalid {name} URL: {error}"))?;
        if !matches!(parsed.host_str(), Some("127.0.0.1" | "localhost")) {
            return Err(format!("{name} URL must target the owned loopback fixture").into());
        }
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
        "usage: deployed_owner_relay_probe --owner-url URL --forwarder-url URL --postgres-url URL --redis-url URL [--requests 200] [--concurrency 20] [--timeout-ms 10000] [--chunk-delay-ms 0] [--output report.json]"
    );
}
