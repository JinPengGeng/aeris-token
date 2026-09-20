use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::response::Response;
use axum::routing::any;
use axum::{extract::Request, Router};
use http::header::{HeaderName, HeaderValue};
use http::StatusCode;

use crate::constants::{
    CONTROL_EXECUTED_HEADER, CONTROL_EXECUTE_FALLBACK_HEADER,
    EXECUTION_PATH_EXECUTION_RUNTIME_SYNC, EXECUTION_PATH_HEADER, EXECUTION_PATH_LOCAL_AUTH_DENIED,
};

use super::{build_router, start_server};

// Keep the task-read fixtures in the unresolved internal context case: the
// caller supplies a credential, while the gateway has no auth reader. GET
// task reads must remain an enumerability-safe 404 rather than falling
// through to a public or control-execute route.
const INTERNAL_AUTH_CONTEXT_TEST_BEARER: &str = "Bearer sk-context-reader-unavailable";

#[tokio::test]
async fn gateway_hides_video_task_when_auth_context_is_unavailable_even_with_opt_in_header() {
    let execute_hits = Arc::new(Mutex::new(0usize));
    let execute_hits_clone = Arc::clone(&execute_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/execute-sync",
            any(move |_request: Request| {
                let execute_hits_inner = Arc::clone(&execute_hits_clone);
                async move {
                    *execute_hits_inner.lock().expect("mutex should lock") += 1;
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("{\"status\":\"queued\"}"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        http::header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response.headers_mut().insert(
                        HeaderName::from_static(CONTROL_EXECUTED_HEADER),
                        HeaderValue::from_static("true"),
                    );
                    response
                }
            }),
        )
        .route(
            "/v1/videos/task-123",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/v1/videos/task-123"))
        .header(
            http::header::AUTHORIZATION,
            INTERNAL_AUTH_CONTEXT_TEST_BEARER,
        )
        .header(CONTROL_EXECUTE_FALLBACK_HEADER, "true")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers()[EXECUTION_PATH_HEADER],
        EXECUTION_PATH_EXECUTION_RUNTIME_SYNC
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload, crate::video_tasks::not_found_body());
    assert_eq!(*execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_hides_video_task_when_auth_context_is_unavailable_without_opt_in_header() {
    let execute_hits = Arc::new(Mutex::new(0usize));
    let execute_hits_clone = Arc::clone(&execute_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let public_execution_path = Arc::new(Mutex::new(None::<String>));
    let public_execution_path_clone = Arc::clone(&public_execution_path);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/execute-sync",
            any(move |_request: Request| {
                let execute_hits_inner = Arc::clone(&execute_hits_clone);
                async move {
                    *execute_hits_inner.lock().expect("mutex should lock") += 1;
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("{\"unexpected\":true}"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        HeaderName::from_static(CONTROL_EXECUTED_HEADER),
                        HeaderValue::from_static("true"),
                    );
                    response
                }
            }),
        )
        .route(
            "/v1/videos/task-123",
            any(move |request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                let public_execution_path_inner = Arc::clone(&public_execution_path_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    *public_execution_path_inner
                        .lock()
                        .expect("mutex should lock") = Some(
                        request
                            .headers()
                            .get(EXECUTION_PATH_HEADER)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    );
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/v1/videos/task-123"))
        .header(
            http::header::AUTHORIZATION,
            INTERNAL_AUTH_CONTEXT_TEST_BEARER,
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers()[EXECUTION_PATH_HEADER],
        EXECUTION_PATH_EXECUTION_RUNTIME_SYNC
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload, crate::video_tasks::not_found_body());
    assert_eq!(*execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        public_execution_path
            .lock()
            .expect("mutex should lock")
            .clone()
            .as_deref(),
        None
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_video_get_without_credentials_at_auth_boundary() {
    let execute_hits = Arc::new(Mutex::new(0usize));
    let execute_hits_clone = Arc::clone(&execute_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/execute-sync",
            any(move |_request: Request| {
                let execute_hits_inner = Arc::clone(&execute_hits_clone);
                async move {
                    *execute_hits_inner.lock().expect("mutex should lock") += 1;
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("{\"status\":\"queued\"}"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        HeaderName::from_static(CONTROL_EXECUTED_HEADER),
                        HeaderValue::from_static("true"),
                    );
                    response
                }
            }),
        )
        .route(
            "/v1/videos/task-123",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/v1/videos/task-123"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers()[EXECUTION_PATH_HEADER],
        EXECUTION_PATH_LOCAL_AUTH_DENIED
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["error"]["type"], "authentication_error");
    assert_eq!(payload["error"]["message"], "Invalid API key");
    assert_eq!(*execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
