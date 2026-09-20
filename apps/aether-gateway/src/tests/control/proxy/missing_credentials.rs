use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aether_runtime_state::{RedisClientConfig, RuntimeSemaphoreConfig, RuntimeState};
use aether_test_support::ManagedRedisServer;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::json;

use super::super::{
    build_router_with_state, hash_api_key, sample_currently_usable_auth_snapshot, start_server,
    AppState, GatewayDataState, InMemoryAuthApiKeySnapshotRepository,
};
use crate::constants::{
    EXECUTION_PATH_DISTRIBUTED_OVERLOADED, EXECUTION_PATH_HEADER, EXECUTION_PATH_LOCAL_AUTH_DENIED,
    EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS, LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER,
    TRACE_ID_HEADER,
};

const VALID_KEY: &str = "sk-public-auth-contract";

async fn auth_contract_state() -> (AppState, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
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
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(VALID_KEY)),
        sample_currently_usable_auth_snapshot("key-auth-contract", "user-auth-contract"),
    )]));
    let state = AppState::new()
        .expect("gateway state should build")
        .with_execution_runtime_override_base_url(upstream_url)
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_reader_for_tests(repository)
                .with_system_default_routing_group_for_tests(),
        );
    (state, upstream_hits, upstream_handle)
}

#[tokio::test]
async fn gateway_public_chat_rejects_missing_malformed_and_invalid_credentials() {
    let (state, upstream_hits, upstream_handle) = auth_contract_state().await;
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = reqwest::Client::new();
    let mut cases = vec![("missing", HeaderMap::new())];
    for (name, value) in [
        ("empty", ""),
        ("whitespace", "   "),
        ("wrong-scheme", "Basic sk-public-auth-contract"),
        ("bare-bearer", "Bearer"),
        ("empty-bearer", "Bearer   "),
        ("multiple-tokens", "Bearer first second"),
        ("invalid-key", "Bearer sk-not-in-the-auth-repository"),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(value).unwrap(),
        );
        cases.push((name, headers));
    }
    let mut duplicate = HeaderMap::new();
    duplicate.append(http::header::AUTHORIZATION, "Bearer first".parse().unwrap());
    duplicate.append(
        http::header::AUTHORIZATION,
        "Bearer second".parse().unwrap(),
    );
    cases.push(("duplicate-authorization", duplicate));
    let mut empty_key = HeaderMap::new();
    empty_key.insert("x-api-key", "   ".parse().unwrap());
    cases.push(("empty-api-key", empty_key));

    for (name, headers) in cases {
        for stream in [false, true] {
            let trace_id = format!("trace-auth-{name}-{stream}");
            let response = client
                .post(format!("{gateway_url}/v1/chat/completions?key="))
                .headers(headers.clone())
                .header(TRACE_ID_HEADER, &trace_id)
                .json(&json!({
                    "model": "gpt-5",
                    "messages": [{"role": "user", "content": "hello"}],
                    "stream": stream
                }))
                .send()
                .await
                .expect("unauthenticated Chat request should complete");
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{name}/{stream}"
            );
            assert_eq!(response.headers()[TRACE_ID_HEADER], trace_id);
            assert_eq!(
                response.headers()[EXECUTION_PATH_HEADER],
                EXECUTION_PATH_LOCAL_AUTH_DENIED
            );
            assert!(!response.headers().contains_key("retry-after"));
            assert!(!response
                .headers()
                .contains_key(LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER));
            let payload: serde_json::Value = response.json().await.expect("authentication JSON");
            assert!(payload.get("type").is_none());
            assert_eq!(payload["error"]["type"], "authentication_error");
            assert_eq!(payload["error"]["message"], "Invalid API key");
            assert_eq!(payload["trace_id"], trace_id);
            assert!(payload["error"]["code"].is_null());
            assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
        }
    }
    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_adjacent_public_routes_reject_missing_credentials_in_their_existing_envelopes() {
    let (state, upstream_hits, upstream_handle) = auth_contract_state().await;
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = reqwest::Client::new();
    for (index, (path, claude, expected_type)) in [
        ("/v1/responses", false, "authentication_error"),
        ("/v1/responses/compact", false, "authentication_error"),
        ("/v1/embeddings", false, "authentication_error"),
        ("/v1/images/generations", false, "authentication_error"),
        ("/v1/images/edits", false, "authentication_error"),
        ("/v1/messages", true, "authentication_error"),
        ("/v1/messages/count_tokens", true, "authentication_error"),
        (
            "/v1beta/models/gemini-test:generateContent",
            false,
            "http_error",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let trace_id = format!("trace-missing-auth-route-{index}");
        let response = client
            .post(format!("{gateway_url}{path}"))
            .header(TRACE_ID_HEADER, &trace_id)
            .json(&json!({}))
            .send()
            .await
            .expect("missing-credential request should complete");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(response.headers()[TRACE_ID_HEADER], trace_id, "{path}");
        assert_eq!(
            response.headers()[EXECUTION_PATH_HEADER],
            EXECUTION_PATH_LOCAL_AUTH_DENIED
        );
        assert!(!response.headers().contains_key("retry-after"), "{path}");
        let payload: serde_json::Value = response.json().await.expect("authentication JSON");
        assert_eq!(payload["trace_id"], trace_id, "{path}");
        assert_eq!(payload["error"]["type"], expected_type, "{path}");
        if claude {
            assert_eq!(payload["type"], "error", "{path}");
        } else {
            assert!(payload.get("type").is_none(), "{path}");
        }
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 0, "{path}");
    }
    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_public_chat_preserves_existing_api_key_carriers() {
    let (state, upstream_hits, upstream_handle) = auth_contract_state().await;
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = reqwest::Client::new();
    for carrier in [
        "authorization",
        "x-api-key",
        "api-key",
        "x-goog-api-key",
        "query",
    ] {
        let trace_id = format!("trace-valid-auth-{carrier}");
        let mut request = client
            .post(format!("{gateway_url}/v1/chat/completions"))
            .header(TRACE_ID_HEADER, &trace_id);
        request = match carrier {
            "authorization" => request.bearer_auth(VALID_KEY),
            "query" => request.query(&[("key", VALID_KEY)]),
            header => request.header(header, VALID_KEY),
        };
        let response = request
            .json(&json!({"model":"gpt-5","messages":[{"role":"user","content":"hello"}]}))
            .send()
            .await
            .expect("authenticated request should complete");
        // Authentication succeeds; this intentionally provider-free fixture
        // stops at candidate selection, rather than producing an auth denial.
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{carrier}"
        );
        assert_eq!(response.headers()[TRACE_ID_HEADER], trace_id);
        assert_eq!(
            response.headers()[EXECUTION_PATH_HEADER],
            EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS
        );
        let miss_reason = response
            .headers()
            .get(LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER)
            .expect("authenticated candidate miss should include its reason")
            .to_str()
            .expect("candidate miss reason should be text");
        assert_ne!(
            miss_reason, "missing_auth_context",
            "{carrier} must still establish authentication"
        );
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
    }
    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_redis_admission_outage_still_precedes_missing_credentials() {
    let mut redis = match ManagedRedisServer::start().await {
        Ok(redis) => redis,
        Err(error) if error.to_string().contains("No such file or directory") => {
            eprintln!("skipping Redis admission outage test: {error}");
            return;
        }
        Err(error) => panic!("isolated Redis should start: {error}"),
    };
    let runtime = RuntimeState::redis(
        RedisClientConfig {
            url: redis.redis_url().to_string(),
            key_prefix: Some("missing-auth-admission".to_string()),
        },
        Some(250),
    )
    .await
    .expect("Redis runtime should connect");
    let gate = runtime
        .semaphore(
            "gateway_requests_distributed",
            1,
            RuntimeSemaphoreConfig {
                command_timeout_ms: Some(250),
                ..RuntimeSemaphoreConfig::default()
            },
        )
        .expect("distributed request gate should build");
    let (state, upstream_hits, upstream_handle) = auth_contract_state().await;
    let gateway = build_router_with_state(state.with_distributed_request_concurrency_gate(gate));
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    redis.stop().expect("isolated Redis should stop");
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("client should build")
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(TRACE_ID_HEADER, "trace-missing-auth-redis-outage")
        .json(&json!({"model":"gpt-5","messages":[]}))
        .send()
        .await
        .expect("admission outage should return promptly");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers()[TRACE_ID_HEADER],
        "trace-missing-auth-redis-outage"
    );
    assert_eq!(
        response.headers()[EXECUTION_PATH_HEADER],
        EXECUTION_PATH_DISTRIBUTED_OVERLOADED
    );
    let payload: serde_json::Value = response.json().await.expect("overload JSON");
    assert_eq!(payload["error"]["type"], "server_error");
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
    gateway_handle.abort();
    upstream_handle.abort();
}
