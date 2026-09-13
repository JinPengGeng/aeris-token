use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::routing::any;
use axum::{extract::Request, Json, Router};
use http::StatusCode;
use serde_json::json;

use super::super::{
    build_router_with_state, hash_api_key, sample_currently_usable_auth_snapshot, start_server,
    wait_until, AppState, GatewayDataState, InMemoryAuthApiKeySnapshotRepository,
};
use crate::constants::{
    EXECUTION_PATH_HEADER, EXECUTION_PATH_LOCAL_AUTH_DENIED,
    EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS, TRACE_ID_HEADER,
};

const VALID_KEY: &str = "sk-public-control-deadline";
const CONTROL_TIMEOUT: Duration = Duration::from_millis(40);
const RECOVERY_TIMEOUT: Duration = Duration::from_millis(500);

struct DeadlineFixture {
    state: AppState,
    upstream_hits: Arc<AtomicUsize>,
    upstream_handle: tokio::task::JoinHandle<()>,
}

async fn deadline_fixture(
    repository: Arc<InMemoryAuthApiKeySnapshotRepository>,
    timeout: Duration,
    system_config_delay: Option<Duration>,
) -> DeadlineFixture {
    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let upstream_hits_for_handler = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let hits = Arc::clone(&upstream_hits_for_handler);
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, Json(json!({"unexpected_upstream": true})))
            }
        }),
    );
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let mut data = GatewayDataState::with_auth_api_key_reader_for_tests(repository)
        .with_system_default_routing_group_for_tests();
    if let Some(delay) = system_config_delay {
        data.set_system_config_read_delay_for_tests(delay);
    }
    let state = AppState::new()
        .expect("gateway state should build")
        .with_execution_runtime_override_base_url(upstream_url)
        .with_public_control_context_timeout_for_tests(timeout)
        .with_data_state_for_tests(data);
    DeadlineFixture {
        state,
        upstream_hits,
        upstream_handle,
    }
}

fn valid_auth_repository(
    lookup_delay: Option<Duration>,
) -> Arc<InMemoryAuthApiKeySnapshotRepository> {
    let repository = InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(VALID_KEY)),
        sample_currently_usable_auth_snapshot(
            "key-public-control-deadline",
            "user-public-control-deadline",
        ),
    )]);
    Arc::new(match lookup_delay {
        Some(delay) => repository.with_lookup_delay_for_tests(delay),
        None => repository,
    })
}

async fn send_valid_chat(
    client: &reqwest::Client,
    gateway_url: &str,
    trace_id: &str,
) -> reqwest::Response {
    client
        .post(format!("{gateway_url}/v1/chat/completions"))
        .bearer_auth(VALID_KEY)
        .header(TRACE_ID_HEADER, trace_id)
        .json(&json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .send()
        .await
        .expect("public chat request should complete")
}

async fn send_missing_credential_chat(
    client: &reqwest::Client,
    gateway_url: &str,
    trace_id: &str,
) -> reqwest::Response {
    client
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(TRACE_ID_HEADER, trace_id)
        .json(&json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .send()
        .await
        .expect("missing-credential request should complete")
}

async fn assert_control_timeout(response: reqwest::Response, trace_id: &str) {
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(response.headers()[TRACE_ID_HEADER], trace_id);
    assert_eq!(response.headers()["retry-after"], "1");
    let payload: serde_json::Value = response.json().await.expect("control error JSON");
    assert_eq!(payload["error"]["type"], "server_error");
    assert_eq!(payload["error"]["code"], "control_unavailable");
    assert_eq!(payload["error"]["trace_id"], trace_id);
    assert_eq!(payload["error"]["message"], "gateway control unavailable");
    assert!(!payload
        .to_string()
        .contains("public control context resolution timed out"));
}

async fn assert_missing_credential_401(response: reqwest::Response, trace_id: &str) {
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()[TRACE_ID_HEADER], trace_id);
    assert_eq!(
        response.headers()[EXECUTION_PATH_HEADER],
        EXECUTION_PATH_LOCAL_AUTH_DENIED
    );
    let payload: serde_json::Value = response.json().await.expect("authentication JSON");
    assert_eq!(payload["error"]["type"], "authentication_error");
    assert_eq!(payload["error"]["message"], "无效的API密钥");
    assert!(payload["error"]["code"].is_null());
}

#[tokio::test]
async fn public_control_deadline_bounds_policy_follower_and_recovers_loader() {
    let fixture = deadline_fixture(
        valid_auth_repository(None),
        CONTROL_TIMEOUT,
        Some(Duration::from_millis(200)),
    )
    .await;
    let state = fixture.state;
    let baseline_gate = state
        .auth_snapshot_load_concurrency_snapshot()
        .expect("auth snapshot gate should be configured");
    assert_eq!(baseline_gate.in_flight, 0);
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state.clone())).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("client should build");
    let first_client = client.clone();
    let first_url = gateway_url.clone();
    let started_at = Instant::now();
    let first = tokio::spawn(async move {
        send_valid_chat(&first_client, &first_url, "trace-control-policy-timeout").await
    });
    tokio::time::sleep(Duration::from_millis(5)).await;
    let follower = send_valid_chat(
        &client,
        &gateway_url,
        "trace-control-policy-follower-timeout",
    )
    .await;
    let response = first.await.expect("policy leader request should join");
    assert!(
        started_at.elapsed() < Duration::from_secs(2),
        "policy hang must be bounded"
    );
    assert_control_timeout(response, "trace-control-policy-timeout").await;
    assert_control_timeout(follower, "trace-control-policy-follower-timeout").await;
    wait_until(500, || {
        state
            .auth_snapshot_load_concurrency_snapshot()
            .is_some_and(|snapshot| snapshot.in_flight == 0)
    })
    .await;
    let after_timeout = state
        .auth_snapshot_load_concurrency_snapshot()
        .expect("auth snapshot gate should remain configured");
    assert_eq!(after_timeout.in_flight, 0);
    assert_eq!(
        after_timeout.available_permits,
        baseline_gate.available_permits
    );
    assert_eq!(fixture.upstream_hits.load(Ordering::SeqCst), 0);

    state
        .data
        .set_system_config_read_delay_for_tests(Duration::ZERO);
    gateway_handle.abort();

    let recovered_state = state.with_public_control_context_timeout_for_tests(RECOVERY_TIMEOUT);
    let (recovered_url, recovered_handle) =
        start_server(build_router_with_state(recovered_state.clone())).await;
    let recovered = send_valid_chat(&client, &recovered_url, "trace-control-policy-recovery").await;
    assert_eq!(recovered.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        recovered.headers()[TRACE_ID_HEADER],
        "trace-control-policy-recovery"
    );
    assert_eq!(
        recovered.headers()[EXECUTION_PATH_HEADER],
        EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS
    );
    let recovered_payload: serde_json::Value =
        recovered.json().await.expect("recovery response JSON");
    assert_ne!(recovered_payload["error"]["code"], "control_unavailable");
    let missing =
        send_missing_credential_chat(&client, &recovered_url, "trace-control-policy-401").await;
    assert_missing_credential_401(missing, "trace-control-policy-401").await;
    assert_eq!(fixture.upstream_hits.load(Ordering::SeqCst), 0);
    recovered_handle.abort();
    fixture.upstream_handle.abort();
}

#[tokio::test]
async fn public_control_deadline_cancels_auth_leader_and_follower_and_restores_capacity() {
    let repository = valid_auth_repository(Some(Duration::from_millis(200)));
    let fixture = deadline_fixture(Arc::clone(&repository), CONTROL_TIMEOUT, None).await;
    let state = fixture.state;
    let baseline_gate = state
        .auth_snapshot_load_concurrency_snapshot()
        .expect("auth snapshot gate should be configured");
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state.clone())).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("client should build");

    let first_client = client.clone();
    let first_url = gateway_url.clone();
    let first = tokio::spawn(async move {
        send_valid_chat(
            &first_client,
            &first_url,
            "trace-control-auth-leader-timeout",
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(5)).await;
    let follower =
        send_valid_chat(&client, &gateway_url, "trace-control-auth-follower-timeout").await;
    let leader = first.await.expect("leader request should join");
    assert_control_timeout(leader, "trace-control-auth-leader-timeout").await;
    assert_control_timeout(follower, "trace-control-auth-follower-timeout").await;
    wait_until(1_000, || {
        state
            .auth_snapshot_load_concurrency_snapshot()
            .is_some_and(|snapshot| snapshot.in_flight == 0)
    })
    .await;
    let after_cancel = state
        .auth_snapshot_load_concurrency_snapshot()
        .expect("auth snapshot gate should remain configured");
    assert_eq!(after_cancel.in_flight, 0);
    assert_eq!(
        after_cancel.available_permits,
        baseline_gate.available_permits
    );
    assert_eq!(fixture.upstream_hits.load(Ordering::SeqCst), 0);
    gateway_handle.abort();

    let recovered_state = state.with_public_control_context_timeout_for_tests(RECOVERY_TIMEOUT);
    let (recovered_url, recovered_handle) =
        start_server(build_router_with_state(recovered_state.clone())).await;
    let recovered = send_valid_chat(&client, &recovered_url, "trace-control-auth-recovery").await;
    assert_eq!(recovered.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        recovered.headers()[TRACE_ID_HEADER],
        "trace-control-auth-recovery"
    );
    assert_eq!(
        recovered.headers()[EXECUTION_PATH_HEADER],
        EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS
    );
    assert_eq!(
        repository.snapshot_lookup_count("key-public-control-deadline"),
        1
    );
    let missing =
        send_missing_credential_chat(&client, &recovered_url, "trace-control-auth-401").await;
    assert_missing_credential_401(missing, "trace-control-auth-401").await;
    wait_until(1_000, || {
        recovered_state
            .auth_snapshot_load_concurrency_snapshot()
            .is_some_and(|snapshot| snapshot.in_flight == 0)
    })
    .await;
    assert_eq!(fixture.upstream_hits.load(Ordering::SeqCst), 0);
    recovered_handle.abort();
    fixture.upstream_handle.abort();
}
