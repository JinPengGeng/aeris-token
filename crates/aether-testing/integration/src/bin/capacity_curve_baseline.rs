// Gateway-backed benchmark scenarios live outside the reusable testkit.
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
use aether_gateway::tunnel_protocol as protocol;
use aether_testkit::{
    fetch_prometheus_samples, find_metric_value_u64, init_test_runtime_for,
    insert_tunnel_harness_auth_headers, run_http_load_probe, BenchmarkRuntimeSnapshot,
    ExecutionRuntimeHarness, ExecutionRuntimeHarnessConfig, GatewayHarness, GatewayHarnessConfig,
    HttpLoadProbeConfig, HttpLoadProbeResponseMode, HttpLoadProbeResult, SpawnedServer,
    TunnelHarness, TunnelHarnessConfig, GATEWAY_HARNESS_API_KEY, TUNNEL_HARNESS_NODE_ID,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use futures_util::{stream::FuturesUnordered, SinkExt, StreamExt};
use reqwest::Method;
use serde::Serialize;
use serde_json::json;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const PROXY_TUNNEL_PATH: &str = "/api/internal/proxy-tunnel";
const TUNNEL_RELAY_PATH_PREFIX: &str = "/api/internal/tunnel/relay";

#[derive(Debug, Clone)]
struct CapacityCurveBaselineConfig {
    points: Vec<usize>,
    requests_per_point_multiplier: usize,
    sync_delay: Duration,
    stream_chunk_delay: Duration,
    tunnel_hold: Duration,
    timeout: Duration,
    saturation_latency_multiplier: u64,
    output_path: Option<PathBuf>,
}

impl Default for CapacityCurveBaselineConfig {
    fn default() -> Self {
        Self {
            points: vec![8, 16, 32, 64, 128, 256],
            requests_per_point_multiplier: 8,
            sync_delay: Duration::from_millis(75),
            stream_chunk_delay: Duration::from_millis(25),
            tunnel_hold: Duration::from_millis(75),
            timeout: Duration::from_secs(10),
            saturation_latency_multiplier: 4,
            output_path: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct CapacityCurveBaselineReport {
    suite: &'static str,
    gateway_sync: CapacityCurveScenarioReport,
    gateway_stream: CapacityCurveScenarioReport,
    execution_runtime_sync: CapacityCurveScenarioReport,
    execution_runtime_stream: CapacityCurveScenarioReport,
    gateway_tunnel_stream: CapacityCurveScenarioReport,
}

#[derive(Debug, Serialize)]
struct CapacityCurveScenarioReport {
    name: String,
    gate: String,
    latency_budget_ms: u64,
    points: Vec<CapacityCurvePointResult>,
    saturation_point: Option<CapacityCurveSaturationPoint>,
}

#[derive(Debug, Serialize)]
struct CapacityCurvePointResult {
    limit: usize,
    concurrency: usize,
    total_requests: usize,
    duration_ms: u64,
    successful_requests: usize,
    rejected_requests: usize,
    error_status_requests: usize,
    failed_requests: usize,
    status_counts: BTreeMap<u16, usize>,
    non_success_status_samples: serde_json::Value,
    throughput_rps: u64,
    p50_ms: u64,
    p95_ms: u64,
    p99_ms: u64,
    max_ms: u64,
    mean_ms: u64,
    metrics: GateMetricSnapshot,
    runtime: BenchmarkRuntimeSnapshot,
}

#[derive(Debug, Serialize)]
struct CapacityCurveSaturationPoint {
    limit: usize,
    concurrency: usize,
    reason: String,
    p95_ms: u64,
    rejected_requests: usize,
    error_status_requests: usize,
    failed_requests: usize,
    high_watermark: u64,
}

#[derive(Debug, Serialize)]
struct GateMetricSnapshot {
    in_flight: u64,
    available_permits: u64,
    high_watermark: u64,
    rejected_total: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _log_shutdown = aether_runtime::LogShutdownGuard::new();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()?;
    runtime.block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    init_test_runtime_for("capacity-curve-baseline");
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
    config: &CapacityCurveBaselineConfig,
) -> Result<CapacityCurveBaselineReport, Box<dyn std::error::Error>> {
    let upstream = SpawnedServer::start(build_delayed_upstream(
        config.sync_delay,
        config.stream_chunk_delay,
    ))
    .await?;

    Ok(CapacityCurveBaselineReport {
        suite: "capacity_curve_baseline",
        gateway_sync: run_gateway_curve(
            "gateway_proxy_sync",
            "gateway_requests",
            false,
            upstream.base_url(),
            config,
        )
        .await?,
        gateway_stream: run_gateway_curve(
            "gateway_proxy_stream",
            "gateway_requests",
            true,
            upstream.base_url(),
            config,
        )
        .await?,
        execution_runtime_sync: run_execution_runtime_curve(
            "execution_runtime_sync",
            "execution_runtime_requests",
            false,
            upstream.base_url(),
            config,
        )
        .await?,
        execution_runtime_stream: run_execution_runtime_curve(
            "execution_runtime_stream",
            "execution_runtime_requests",
            true,
            upstream.base_url(),
            config,
        )
        .await?,
        gateway_tunnel_stream: run_tunnel_curve("gateway_tunnel_stream", "tunnel_requests", config)
            .await?,
    })
}

async fn run_gateway_curve(
    scenario_name: &str,
    gate_name: &str,
    stream: bool,
    upstream_base_url: &str,
    config: &CapacityCurveBaselineConfig,
) -> Result<CapacityCurveScenarioReport, Box<dyn std::error::Error>> {
    let latency_budget_ms = scenario_latency_budget_ms(
        if stream {
            config.stream_chunk_delay.saturating_mul(3u32)
        } else {
            config.sync_delay
        },
        config.saturation_latency_multiplier,
    );
    let mut points = Vec::new();
    for limit in &config.points {
        let gateway = GatewayHarness::start(GatewayHarnessConfig {
            upstream_base_url: upstream_base_url.to_string(),
            data_config: None,
            max_in_flight_requests: Some(*limit),
            distributed_request_gate: None,
            tunnel_instance_id: None,
            tunnel_relay_base_url: None,
        })
        .await?;
        let total_requests = total_requests_for_limit(*limit, config.requests_per_point_multiplier);
        let probe = chat_probe_config(
            format!("{}/v1/chat/completions", gateway.base_url()),
            stream,
            total_requests,
            *limit,
            config.timeout,
        );
        let started_at = Instant::now();
        let result = run_http_load_probe(&probe)
            .await
            .map_err(std::io::Error::other)?;
        let duration_ms = started_at.elapsed().as_millis() as u64;
        let samples = gateway
            .metric_samples()
            .await
            .map_err(std::io::Error::other)?;
        let metrics = gate_metrics(&samples, gate_name)?;
        points.push(capacity_point(
            *limit,
            total_requests,
            duration_ms,
            result,
            metrics,
        ));
    }

    Ok(CapacityCurveScenarioReport {
        name: scenario_name.to_string(),
        gate: gate_name.to_string(),
        latency_budget_ms,
        saturation_point: detect_saturation_point(&points, latency_budget_ms),
        points,
    })
}

async fn run_execution_runtime_curve(
    scenario_name: &str,
    gate_name: &str,
    stream: bool,
    upstream_base_url: &str,
    config: &CapacityCurveBaselineConfig,
) -> Result<CapacityCurveScenarioReport, Box<dyn std::error::Error>> {
    let latency_budget_ms = scenario_latency_budget_ms(
        if stream {
            config.stream_chunk_delay.saturating_mul(3u32)
        } else {
            config.sync_delay
        },
        config.saturation_latency_multiplier,
    );
    let mut points = Vec::new();
    for limit in &config.points {
        let runtime = ExecutionRuntimeHarness::start(ExecutionRuntimeHarnessConfig {
            max_in_flight_requests: Some(*limit),
            distributed_request_gate: None,
        })
        .await?;
        let total_requests = total_requests_for_limit(*limit, config.requests_per_point_multiplier);
        let probe = execution_probe_config(
            format!(
                "{}/v1/execute/{}",
                runtime.base_url(),
                if stream { "stream" } else { "sync" }
            ),
            execution_plan(format!("{upstream_base_url}/v1/chat/completions"), stream),
            total_requests,
            *limit,
            config.timeout,
        );
        let started_at = Instant::now();
        let result = run_http_load_probe(&probe)
            .await
            .map_err(std::io::Error::other)?;
        let duration_ms = started_at.elapsed().as_millis() as u64;
        let metrics =
            capture_gate_metrics(&format!("{}/metrics", runtime.base_url()), gate_name).await?;
        points.push(capacity_point(
            *limit,
            total_requests,
            duration_ms,
            result,
            metrics,
        ));
    }

    Ok(CapacityCurveScenarioReport {
        name: scenario_name.to_string(),
        gate: gate_name.to_string(),
        latency_budget_ms,
        saturation_point: detect_saturation_point(&points, latency_budget_ms),
        points,
    })
}

async fn run_tunnel_curve(
    scenario_name: &str,
    gate_name: &str,
    config: &CapacityCurveBaselineConfig,
) -> Result<CapacityCurveScenarioReport, Box<dyn std::error::Error>> {
    let latency_budget_ms =
        scenario_latency_budget_ms(config.tunnel_hold, config.saturation_latency_multiplier);
    let mut points = Vec::new();
    for limit in &config.points {
        let relay_concurrency = (*limit).saturating_sub(1).max(1);
        let tunnel = TunnelHarness::start(TunnelHarnessConfig {
            node_id: TUNNEL_HARNESS_NODE_ID.to_string(),
            max_streams: (*limit).max(128),
            ping_interval: Duration::from_secs(15),
            outbound_queue_capacity: 128,
            max_in_flight_requests: Some(*limit),
            distributed_request_gate: None,
        })
        .await?;
        let peer = connect_protocol_peer(tunnel.base_url(), config.tunnel_hold).await?;
        let total_requests =
            total_requests_for_limit(relay_concurrency, config.requests_per_point_multiplier);
        let envelope = relay_envelope();
        let body_offset =
            4 + u32::from_be_bytes(envelope[..4].try_into().expect("metadata length")) as usize;
        verify_tunnel_fixture(&tunnel, &envelope, body_offset, config.timeout).await?;
        let header_sets = (0..total_requests)
            .map(|_| {
                let mut headers =
                    tunnel.relay_headers(&envelope[..body_offset], &envelope[body_offset..]);
                headers.insert(
                    "content-type".to_string(),
                    "application/octet-stream".to_string(),
                );
                headers
            })
            .collect();
        let probe = HttpLoadProbeConfig {
            url: format!(
                "{tunnel_base}{TUNNEL_RELAY_PATH_PREFIX}/node-baseline",
                tunnel_base = tunnel.base_url()
            ),
            method: Method::POST,
            headers: BTreeMap::from([(
                "content-type".to_string(),
                "application/octet-stream".to_string(),
            )]),
            header_sets,
            body: Some(envelope),
            total_requests,
            concurrency: relay_concurrency,
            timeout: config.timeout,
            response_mode: HttpLoadProbeResponseMode::FullBody,
            ..HttpLoadProbeConfig::default()
        };
        let started_at = Instant::now();
        let result = run_http_load_probe(&probe)
            .await
            .map_err(std::io::Error::other)?;
        let duration_ms = started_at.elapsed().as_millis() as u64;
        let metrics =
            capture_gate_metrics(&format!("{}/metrics", tunnel.base_url()), gate_name).await?;
        points.push(capacity_point(
            *limit,
            total_requests,
            duration_ms,
            result,
            metrics,
        ));
        peer.abort();
        let _ = peer.await;
    }

    Ok(CapacityCurveScenarioReport {
        name: scenario_name.to_string(),
        gate: gate_name.to_string(),
        latency_budget_ms,
        saturation_point: detect_saturation_point(&points, latency_budget_ms),
        points,
    })
}

async fn verify_tunnel_fixture(
    tunnel: &TunnelHarness,
    envelope: &[u8],
    body_offset: usize,
    timeout: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder().timeout(timeout).build()?;
    let url = format!(
        "{}{TUNNEL_RELAY_PATH_PREFIX}/{TUNNEL_HARNESS_NODE_ID}",
        tunnel.base_url()
    );
    let unsigned = client.post(&url).body(envelope.to_vec()).send().await?;
    if unsigned.status() != StatusCode::FORBIDDEN {
        return Err(std::io::Error::other("unsigned tunnel preflight was not rejected").into());
    }
    let mut signed = client.post(&url).body(envelope.to_vec());
    for (name, value) in tunnel.relay_headers(&envelope[..body_offset], &envelope[body_offset..]) {
        signed = signed.header(name, value);
    }
    let signed = signed.build()?;
    let mut tampered = signed
        .try_clone()
        .expect("buffered relay request should clone");
    let mut tampered_body = envelope.to_vec();
    *tampered_body
        .last_mut()
        .expect("relay body should be nonempty") ^= 1;
    *tampered.body_mut() = Some(tampered_body.into());
    if client.execute(tampered).await?.status() != StatusCode::FORBIDDEN {
        return Err(std::io::Error::other("tampered tunnel preflight was not rejected").into());
    }
    let response = client
        .execute(
            signed
                .try_clone()
                .expect("buffered relay request should clone"),
        )
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if status != StatusCode::OK || body != "capacity-tunnel-stream" {
        return Err(std::io::Error::other(format!(
            "signed tunnel preflight failed: {status}: {body}"
        ))
        .into());
    }
    if client.execute(signed).await?.status() != StatusCode::FORBIDDEN {
        return Err(std::io::Error::other("replayed tunnel preflight was not rejected").into());
    }
    Ok(())
}

fn capacity_point(
    limit: usize,
    total_requests: usize,
    duration_ms: u64,
    result: HttpLoadProbeResult,
    metrics: GateMetricSnapshot,
) -> CapacityCurvePointResult {
    let rejected_requests = result.status_counts.get(&503).copied().unwrap_or_default();
    let successful_statuses = result
        .status_counts
        .iter()
        .filter(|(status, _)| **status >= 200 && **status < 300)
        .map(|(_, count)| *count)
        .sum::<usize>();
    // Non-2xx, non-503 statuses (500/429/502 storms) carry a complete response
    // body, so the probe does not count them as failed — but they are not
    // useful capacity either. Surface them explicitly for saturation
    // detection instead of letting a gateway fault pass as clean.
    let error_status_requests = result
        .status_counts
        .iter()
        .filter(|(status, _)| !(**status >= 200 && **status < 300) && **status != 503)
        .map(|(_, count)| *count)
        .sum::<usize>();
    let responses_received = result.status_counts.values().sum::<usize>();
    let failures_without_response = result.total_requests.saturating_sub(responses_received);
    let failures_after_response = result
        .failed_requests
        .saturating_sub(failures_without_response);
    let successful_requests = successful_statuses.saturating_sub(failures_after_response);
    let throughput_rps = if duration_ms == 0 {
        successful_requests as u64
    } else {
        ((successful_requests as u64) * 1_000) / duration_ms.max(1)
    };

    CapacityCurvePointResult {
        limit,
        concurrency: result.concurrency,
        total_requests,
        duration_ms,
        successful_requests,
        rejected_requests,
        error_status_requests,
        failed_requests: total_requests.saturating_sub(successful_requests + rejected_requests),
        status_counts: result.status_counts,
        non_success_status_samples: serde_json::to_value(result.non_success_status_samples)
            .expect("HTTP status samples should serialize"),
        throughput_rps,
        p50_ms: result.p50_ms,
        p95_ms: result.p95_ms,
        p99_ms: result.p99_ms,
        max_ms: result.max_ms,
        mean_ms: result.mean_ms,
        metrics,
        runtime: result.runtime,
    }
}

fn detect_saturation_point(
    points: &[CapacityCurvePointResult],
    latency_budget_ms: u64,
) -> Option<CapacityCurveSaturationPoint> {
    points.iter().find_map(|point| {
        let reason = if point.error_status_requests > 0 {
            Some("error_statuses_observed")
        } else if point.failed_requests > 0 {
            Some("failures_observed")
        } else if point.rejected_requests > 0 {
            Some("admission_rejections_observed")
        } else if point.p95_ms > latency_budget_ms {
            Some("latency_budget_exceeded")
        } else {
            None
        }?;
        Some(CapacityCurveSaturationPoint {
            limit: point.limit,
            concurrency: point.concurrency,
            reason: reason.to_string(),
            p95_ms: point.p95_ms,
            rejected_requests: point.rejected_requests,
            error_status_requests: point.error_status_requests,
            failed_requests: point.failed_requests,
            high_watermark: point.metrics.high_watermark,
        })
    })
}

async fn capture_gate_metrics(
    metrics_url: &str,
    gate_name: &str,
) -> Result<GateMetricSnapshot, Box<dyn std::error::Error>> {
    let samples = fetch_prometheus_samples(metrics_url)
        .await
        .map_err(std::io::Error::other)?;
    gate_metrics(&samples, gate_name)
}

fn gate_metrics(
    samples: &[aether_testkit::PrometheusSample],
    gate_name: &str,
) -> Result<GateMetricSnapshot, Box<dyn std::error::Error>> {
    let required = |name| {
        find_metric_value_u64(samples, name, &[("gate", gate_name)])
            .or_else(|| {
                find_metric_value_u64(
                    samples,
                    &format!("aether_testkit_{name}"),
                    &[("gate", gate_name)],
                )
            })
            .ok_or_else(|| std::io::Error::other(format!("missing {name} for gate {gate_name}")))
    };
    Ok(GateMetricSnapshot {
        in_flight: required("concurrency_in_flight")?,
        available_permits: required("concurrency_available_permits")?,
        high_watermark: required("concurrency_high_watermark")?,
        rejected_total: required("concurrency_rejected_total")?,
    })
}

fn scenario_latency_budget_ms(base: Duration, multiplier: u64) -> u64 {
    (base.as_millis() as u64).saturating_mul(multiplier.max(1))
}

fn total_requests_for_limit(limit: usize, multiplier: usize) -> usize {
    limit.saturating_mul(multiplier.max(1))
}

fn execution_probe_config(
    url: String,
    plan: ExecutionPlan,
    total_requests: usize,
    concurrency: usize,
    timeout: Duration,
) -> HttpLoadProbeConfig {
    HttpLoadProbeConfig {
        url,
        method: Method::POST,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(
            serde_json::to_vec(&plan).expect("execution plan should serialize for capacity curve"),
        ),
        total_requests,
        concurrency,
        timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
        ..HttpLoadProbeConfig::default()
    }
}

fn chat_probe_config(
    url: String,
    stream: bool,
    total_requests: usize,
    concurrency: usize,
    timeout: Duration,
) -> HttpLoadProbeConfig {
    HttpLoadProbeConfig {
        url,
        method: Method::POST,
        headers: BTreeMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            (
                "authorization".to_string(),
                format!("Bearer {GATEWAY_HARNESS_API_KEY}"),
            ),
        ]),
        body: Some(
            serde_json::to_vec(&json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": stream,
            }))
            .expect("chat body should serialize"),
        ),
        total_requests,
        concurrency,
        timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
        ..HttpLoadProbeConfig::default()
    }
}

fn execution_plan(url: String, stream: bool) -> ExecutionPlan {
    ExecutionPlan {
        request_id: if stream {
            "capacity-curve-stream-request".to_string()
        } else {
            "capacity-curve-sync-request".to_string()
        },
        candidate_id: Some(if stream {
            "capacity-curve-stream-candidate".to_string()
        } else {
            "capacity-curve-sync-candidate".to_string()
        }),
        provider_name: Some("openai".to_string()),
        provider_id: "provider-capacity".to_string(),
        endpoint_id: "endpoint-capacity".to_string(),
        key_id: "key-capacity".to_string(),
        method: "POST".to_string(),
        url,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody::from_json(json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}],
            "stream": stream,
        })),
        stream,
        client_api_format: "openai:chat".to_string(),
        provider_api_format: "openai:chat".to_string(),
        model_name: Some("gpt-5".to_string()),
        proxy: None,
        transport_profile: None,
        timeouts: Some(ExecutionTimeouts {
            connect_ms: Some(2_000),
            read_ms: Some(10_000),
            first_byte_ms: Some(5_000),
            total_ms: Some(10_000),
            ..ExecutionTimeouts::default()
        }),
    }
}

fn build_delayed_upstream(sync_delay: Duration, stream_chunk_delay: Duration) -> Router {
    Router::new().route(
        "/v1/chat/completions",
        any(move |request: Request| {
            let sync_delay = sync_delay;
            let stream_chunk_delay = stream_chunk_delay;
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX)
                    .await
                    .expect("capacity upstream body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).unwrap_or_else(|_| json!({}));
                let stream = payload
                    .get("stream")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                if stream {
                    let body = async_stream::stream! {
                        tokio::time::sleep(stream_chunk_delay).await;
                        yield Ok::<_, Infallible>(Bytes::from_static(
                            b"data: {\"id\":\"chunk-1\",\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
                        ));
                        tokio::time::sleep(stream_chunk_delay).await;
                        yield Ok::<_, Infallible>(Bytes::from_static(
                            b"data: {\"id\":\"chunk-2\",\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
                        ));
                        tokio::time::sleep(stream_chunk_delay).await;
                        yield Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"));
                    };
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(http::header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from_stream(body))
                        .expect("capacity upstream stream response should build")
                } else {
                    tokio::time::sleep(sync_delay).await;
                    Json(json!({
                        "id": "chatcmpl-capacity",
                        "object": "chat.completion",
                        "model": payload.get("model").and_then(|value| value.as_str()).unwrap_or("gpt-5"),
                        "choices": [{"message": {"role": "assistant", "content": "hello"}}]
                    }))
                    .into_response()
                }
            }
        }),
    )
}

fn relay_envelope() -> Vec<u8> {
    let meta = protocol::RequestMeta {
        method: "POST".to_string(),
        url: "https://capacity.example/v1/chat/completions".to_string(),
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
    let meta_json = serde_json::to_vec(&meta).expect("hub relay metadata should serialize");
    let body = br#"{"model":"gpt-5","messages":[{"role":"user","content":"hello"}]}"#;
    let mut envelope = Vec::with_capacity(4 + meta_json.len() + body.len());
    envelope.extend_from_slice(&(meta_json.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&meta_json);
    envelope.extend_from_slice(body);
    envelope
}

async fn connect_protocol_peer(
    tunnel_base_url: &str,
    hold: Duration,
) -> Result<tokio::task::JoinHandle<()>, Box<dyn std::error::Error>> {
    let ws_url = format!(
        "{}{}",
        tunnel_base_url.replace("http://", "ws://"),
        PROXY_TUNNEL_PATH
    );
    let request = ws_url.into_client_request()?;
    let mut request = request;
    insert_tunnel_harness_auth_headers(request.headers_mut(), TUNNEL_HARNESS_NODE_ID)?;
    request.headers_mut().insert(
        aether_contracts::tunnel::TUNNEL_PROTOCOL_VERSION_HEADER,
        http::HeaderValue::from_static(
            aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION_STR,
        ),
    );
    request.headers_mut().insert(
        "x-node-name",
        http::HeaderValue::from_static("proxy-baseline"),
    );
    request.headers_mut().insert(
        "x-tunnel-max-streams",
        http::HeaderValue::from_static("512"),
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
            session_id: Some("capacity-curve-session".to_string()),
            replica_id: Some("capacity-curve-replica".to_string()),
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
        let mut responses = FuturesUnordered::new();
        loop {
            tokio::select! {
                message = stream.next() => {
                    match message {
                        Some(Ok(Message::Binary(data))) => {
                            match handle_binary_frame(&mut sink, data.to_vec()).await {
                                Ok(Some(stream_id)) => responses.push(async move {
                                    tokio::time::sleep(hold).await;
                                    stream_id
                                }),
                                Ok(None) => {},
                                Err(_) => break,
                            }
                        }
                        Some(Ok(Message::Ping(payload)))
                            if sink.send(Message::Pong(payload.clone())).await.is_err() =>
                        {
                            break;
                        }
                        Some(Ok(Message::Ping(_))) => {}
                        None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                        _ => {},
                    }
                }
                Some(stream_id) = responses.next(), if !responses.is_empty() => {
                    if send_protocol_response(&mut sink, stream_id).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = sink.close().await;
    }))
}

async fn handle_binary_frame<S>(
    sink: &mut S,
    data: Vec<u8>,
) -> Result<Option<u32>, tokio_tungstenite::tungstenite::Error>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let Some(header) = protocol::FrameHeader::parse(&data) else {
        return Ok(None);
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
                return Ok(None);
            }
            return Ok(Some(header.stream_id));
        }
        _ => {}
    }
    Ok(None)
}

async fn send_protocol_response<S>(
    sink: &mut S,
    stream_id: u32,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
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
            stream_id,
            protocol::RESPONSE_HEADERS,
            0,
            &response_meta_json,
        )
        .into(),
    ))
    .await?;

    for chunk in [
        b"capacity-".as_slice(),
        b"tunnel-".as_slice(),
        b"stream".as_slice(),
    ] {
        sink.send(Message::Binary(
            protocol::encode_frame(stream_id, protocol::RESPONSE_BODY, 0, chunk).into(),
        ))
        .await?;
    }

    sink.send(Message::Binary(
        protocol::encode_frame(stream_id, protocol::STREAM_END, 0, &[]).into(),
    ))
    .await?;
    Ok(())
}

/// Command-line surface of `capacity_curve_baseline`. Flag names and value
/// formats intentionally match the previous hand-written parser so existing
/// scripts keep working (issue #212).
#[derive(Debug, Clone, clap::Parser)]
#[command(
    name = "capacity_curve_baseline",
    about = "Gateway-backed capacity curve baseline suite",
    disable_help_flag = false
)]
struct CapacityCurveCli {
    /// Comma-separated concurrency points, e.g. 8,16,32,64,128,256
    #[arg(long, value_delimiter = ',', default_value = "8,16,32,64,128,256")]
    points: Vec<usize>,
    #[arg(long, default_value_t = 8)]
    requests_per_point_multiplier: usize,
    #[arg(long, default_value_t = 75)]
    sync_delay_ms: u64,
    #[arg(long, default_value_t = 25)]
    stream_chunk_delay_ms: u64,
    #[arg(long, default_value_t = 75)]
    tunnel_hold_ms: u64,
    #[arg(long, default_value_t = 10000)]
    timeout_ms: u64,
    #[arg(long, default_value_t = 4)]
    saturation_latency_multiplier: u64,
    #[arg(long)]
    output: Option<PathBuf>,
}

impl CapacityCurveCli {
    fn parse_from<I, T>(args: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        use clap::Parser as _;
        Self::try_parse_from(args)
    }
}

fn parse_args(
    args: Vec<String>,
) -> Result<CapacityCurveBaselineConfig, Box<dyn std::error::Error>> {
    let cli = CapacityCurveCli::parse_from(
        std::iter::once("capacity_curve_baseline".to_string()).chain(args),
    )?;
    if cli.points.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "capacity curve requires at least one point",
        )
        .into());
    }
    Ok(CapacityCurveBaselineConfig {
        points: cli.points,
        requests_per_point_multiplier: cli.requests_per_point_multiplier,
        sync_delay: Duration::from_millis(cli.sync_delay_ms),
        stream_chunk_delay: Duration::from_millis(cli.stream_chunk_delay_ms),
        tunnel_hold: Duration::from_millis(cli.tunnel_hold_ms),
        timeout: Duration::from_millis(cli.timeout_ms),
        saturation_latency_multiplier: cli.saturation_latency_multiplier,
        output_path: cli.output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn parse(argv: &[&str]) -> Result<CapacityCurveBaselineConfig, Box<dyn std::error::Error>> {
        parse_args(argv.iter().map(|arg| arg.to_string()).collect())
    }

    #[test]
    fn parse_defaults_match_previous_handwritten_defaults() {
        let config = parse(&[]).unwrap();
        let expected = CapacityCurveBaselineConfig::default();
        assert_eq!(config.points, expected.points);
        assert_eq!(
            config.requests_per_point_multiplier,
            expected.requests_per_point_multiplier
        );
        assert_eq!(config.sync_delay, expected.sync_delay);
        assert_eq!(config.stream_chunk_delay, expected.stream_chunk_delay);
        assert_eq!(config.tunnel_hold, expected.tunnel_hold);
        assert_eq!(config.timeout, expected.timeout);
        assert_eq!(
            config.saturation_latency_multiplier,
            expected.saturation_latency_multiplier
        );
        assert_eq!(config.output_path, expected.output_path);
    }

    #[test]
    fn parse_accepts_all_scripted_flags() {
        let config = parse(&[
            "--points",
            "4,8,16",
            "--requests-per-point-multiplier",
            "3",
            "--sync-delay-ms",
            "10",
            "--stream-chunk-delay-ms",
            "5",
            "--tunnel-hold-ms",
            "20",
            "--timeout-ms",
            "3000",
            "--saturation-latency-multiplier",
            "9",
            "--output",
            "/tmp/report.json",
        ])
        .unwrap();
        assert_eq!(config.points, vec![4, 8, 16]);
        assert_eq!(config.requests_per_point_multiplier, 3);
        assert_eq!(config.sync_delay, Duration::from_millis(10));
        assert_eq!(config.stream_chunk_delay, Duration::from_millis(5));
        assert_eq!(config.tunnel_hold, Duration::from_millis(20));
        assert_eq!(config.timeout, Duration::from_millis(3000));
        assert_eq!(config.saturation_latency_multiplier, 9);
        assert_eq!(config.output_path, Some(PathBuf::from("/tmp/report.json")));
    }

    #[test]
    fn parse_rejects_empty_points() {
        // Either clap rejects the empty value, or the post-parse guard does.
        assert!(parse(&["--points", ""]).is_err());
    }

    #[test]
    fn parse_rejects_unknown_flags_and_bad_values() {
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--points", "8,x"]).is_err());
        assert!(parse(&["--timeout-ms", "abc"]).is_err());
    }

    #[tokio::test]
    async fn non_2xx_complete_responses_are_capacity_failures() {
        // Exercise real HTTP and the shared probe: a fully read error response
        // is transport-complete, but must not count as useful capacity.
        struct Case {
            name: &'static str,
            statuses: &'static [u16],
            successful: usize,
            rejected: usize,
            failed: usize,
            reason: Option<&'static str>,
        }
        let cases = [
            Case {
                name: "429",
                statuses: &[429, 429, 429],
                successful: 0,
                rejected: 0,
                failed: 3,
                reason: Some("error_statuses_observed"),
            },
            Case {
                name: "500",
                statuses: &[500, 500, 500],
                successful: 0,
                rejected: 0,
                failed: 3,
                reason: Some("error_statuses_observed"),
            },
            Case {
                name: "502",
                statuses: &[502, 502, 502],
                successful: 0,
                rejected: 0,
                failed: 3,
                reason: Some("error_statuses_observed"),
            },
            Case {
                name: "503",
                statuses: &[503, 503, 503],
                successful: 0,
                rejected: 3,
                failed: 0,
                reason: Some("admission_rejections_observed"),
            },
            Case {
                name: "mixed",
                statuses: &[200, 429, 500, 502, 201],
                successful: 2,
                rejected: 0,
                failed: 3,
                reason: Some("error_statuses_observed"),
            },
            Case {
                name: "mixed rejection",
                statuses: &[200, 503, 500],
                successful: 1,
                rejected: 1,
                failed: 1,
                reason: Some("error_statuses_observed"),
            },
            Case {
                name: "healthy",
                statuses: &[200, 201, 202],
                successful: 3,
                rejected: 0,
                failed: 0,
                reason: None,
            },
        ];
        for case in cases {
            let requests = Arc::new(AtomicUsize::new(0));
            let seen = Arc::clone(&requests);
            let statuses = case.statuses;
            let server = SpawnedServer::start(Router::new().route(
                "/probe",
                any(move || {
                    let index = seen.fetch_add(1, Ordering::SeqCst);
                    async move {
                        (
                            StatusCode::from_u16(statuses[index]).unwrap(),
                            "complete fixture body",
                        )
                    }
                }),
            ))
            .await
            .unwrap();
            let total = statuses.len();
            let result = run_http_load_probe(&HttpLoadProbeConfig {
                url: format!("{}/probe", server.base_url()),
                total_requests: total,
                concurrency: 2,
                response_mode: HttpLoadProbeResponseMode::FullBody,
                timeout: Duration::from_secs(5),
                ..Default::default()
            })
            .await
            .unwrap();
            assert_eq!(requests.load(Ordering::SeqCst), total, "{}", case.name);
            assert_eq!(result.completed_requests, total, "{}", case.name);
            assert_eq!(result.failed_requests, 0, "{}", case.name);
            let point = capacity_point(
                2,
                total,
                1_000,
                result,
                GateMetricSnapshot {
                    in_flight: 0,
                    available_permits: 2,
                    high_watermark: 2,
                    rejected_total: 0,
                },
            );
            assert_eq!(point.successful_requests, case.successful, "{}", case.name);
            assert_eq!(point.failed_requests, case.failed, "{}", case.name);
            assert_eq!(point.rejected_requests, case.rejected, "{}", case.name);
            assert_eq!(
                point.throughput_rps, case.successful as u64,
                "{}",
                case.name
            );
            let saturation = detect_saturation_point(&[point], u64::MAX);
            assert_eq!(
                saturation.as_ref().map(|s| s.reason.as_str()),
                case.reason,
                "{}",
                case.name
            );
        }
    }
}
