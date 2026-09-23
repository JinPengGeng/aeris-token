use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data::repository::wallet::InMemoryWalletRepository;
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Router};
use http::StatusCode;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::super::{build_router_with_state, start_server, AppState};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

fn admin_request(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
}

const QUOTA_STATUS_TEST_STACK_BYTES: usize = 16 * 1024 * 1024;

fn run_quota_status_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(QUOTA_STATUS_TEST_STACK_BYTES)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("quota status test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("current time should be after epoch")
        .as_secs()
}

async fn start_quota_status_upstream(
    path: &'static str,
) -> (String, Arc<Mutex<usize>>, tokio::task::JoinHandle<()>) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        path,
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    (upstream_url, upstream_hits, upstream_handle)
}

fn sample_standalone_api_key_snapshot(
    api_key_id: &str,
    user_id: &str,
    is_active: bool,
) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "standalone-owner".to_string(),
        Some("owner@example.com".to_string()),
        "admin".to_string(),
        "local".to_string(),
        true,
        false,
        None,
        None,
        None,
        api_key_id.to_string(),
        Some(format!("key-{api_key_id}")),
        is_active,
        false,
        true,
        Some(120),
        Some(5),
        Some(4_102_444_800),
        Some(json!(["openai"])),
        Some(json!(["openai:chat"])),
        Some(json!(["gpt-4.1"])),
    )
    .expect("snapshot should build")
}

fn sample_standalone_export_record(
    api_key_id: &str,
    user_id: &str,
    plaintext_key: &str,
    is_active: bool,
    rate_limit: Option<i32>,
    concurrent_limit: Option<i32>,
    daily_usage_limit_usd: Option<f64>,
) -> StoredAuthApiKeyExportRecord {
    let key_hash = hash_api_key(plaintext_key);
    let bootstrap = AppState::new()
        .expect("bootstrap state should build")
        .with_data_state_for_tests(
            GatewayDataState::disabled().with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        );
    let key_encrypted = crate::handlers::shared::seal_auth_api_key_secret(
        &bootstrap,
        user_id,
        api_key_id,
        &key_hash,
        true,
        plaintext_key,
    )
    .expect("key should encrypt");
    let mut record = StoredAuthApiKeyExportRecord::new(
        user_id.to_string(),
        api_key_id.to_string(),
        key_hash,
        Some(key_encrypted),
        Some(format!("key-{api_key_id}")),
        Some(json!(["openai"])),
        Some(json!(["openai:chat"])),
        Some(json!(["gpt-4.1"])),
        rate_limit,
        concurrent_limit,
        None,
        is_active,
        Some(4_102_444_800),
        false,
        7,
        0,
        1.25,
        true,
    )
    .expect("export record should build");
    record.daily_usage_limit_usd = daily_usage_limit_usd;
    record
}

fn sample_finished_rpm_candidate(
    id: &str,
    request_id: &str,
    api_key_id: &str,
    now_unix_secs: i64,
) -> StoredRequestCandidate {
    StoredRequestCandidate::new(
        id.to_string(),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some(api_key_id.to_string()),
        Some("alice".to_string()),
        Some("default".to_string()),
        0,
        0,
        Some("provider-1".to_string()),
        Some("endpoint-1".to_string()),
        Some("provider-key-1".to_string()),
        RequestCandidateStatus::Success,
        None,
        false,
        Some(200),
        None,
        None,
        Some(120),
        Some(1),
        None,
        None,
        (now_unix_secs - 10) * 1_000,
        Some((now_unix_secs - 10) * 1_000),
        Some((now_unix_secs - 8) * 1_000),
    )
    .expect("request candidate should build")
}

fn sample_active_candidate(
    id: &str,
    request_id: &str,
    api_key_id: &str,
    now_unix_secs: i64,
) -> StoredRequestCandidate {
    StoredRequestCandidate::new(
        id.to_string(),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some(api_key_id.to_string()),
        Some("alice".to_string()),
        Some("default".to_string()),
        0,
        0,
        Some("provider-1".to_string()),
        Some("endpoint-1".to_string()),
        Some("provider-key-1".to_string()),
        RequestCandidateStatus::Pending,
        None,
        false,
        None,
        None,
        None,
        Some(120),
        Some(1),
        None,
        None,
        (now_unix_secs - 5) * 1_000,
        Some((now_unix_secs - 5) * 1_000),
        None,
    )
    .expect("request candidate should build")
}

fn sample_usage_row(
    id: &str,
    request_id: &str,
    api_key_id: &str,
    actual_total_cost_usd: f64,
    finalized_at_unix_secs: u64,
) -> StoredRequestUsageAudit {
    let mut row = StoredRequestUsageAudit::new(
        id.to_string(),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some(api_key_id.to_string()),
        Some("user-user-1".to_string()),
        Some(format!("key-{api_key_id}")),
        "openai".to_string(),
        "gpt-4.1".to_string(),
        Some("gpt-4.1".to_string()),
        Some("provider-1".to_string()),
        Some("endpoint-1".to_string()),
        Some("provider-key-1".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        false,
        false,
        45,
        45,
        90,
        actual_total_cost_usd,
        actual_total_cost_usd,
        Some(200),
        None,
        None,
        Some(240),
        Some(80),
        "completed".to_string(),
        "settled".to_string(),
        (finalized_at_unix_secs as i64 - 60) * 1_000,
        finalized_at_unix_secs as i64 - 30,
        Some(finalized_at_unix_secs as i64),
    )
    .expect("usage row should build");
    row.finalized_at_unix_secs = Some(finalized_at_unix_secs);
    row
}

#[test]
fn gateway_reports_admin_quota_status_for_multiple_keys_with_limited_key() {
    run_quota_status_test(
        "quota-status-limited",
        gateway_reports_admin_quota_status_for_multiple_keys_with_limited_key_inner,
    );
}

async fn gateway_reports_admin_quota_status_for_multiple_keys_with_limited_key_inner() {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_quota_status_upstream("/api/admin/quota/status").await;
    let now = now_unix_secs() as i64;

    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![
            (
                None,
                sample_standalone_api_key_snapshot("key-1", "user-1", true),
            ),
            (
                None,
                sample_standalone_api_key_snapshot("key-2", "user-2", true),
            ),
        ])
        .with_export_records([
            sample_standalone_export_record(
                "key-1",
                "user-1",
                "sk-key-1-plaintext",
                true,
                Some(1),
                Some(1),
                Some(0.30),
            ),
            sample_standalone_export_record(
                "key-2",
                "user-2",
                "sk-key-2-plaintext",
                true,
                Some(120),
                None,
                None,
            ),
        ]),
    );
    let candidate_repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_finished_rpm_candidate("cand-1", "req-1", "key-1", now),
        sample_active_candidate("cand-2", "req-2", "key-1", now),
    ]));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage_row(
        "usage-1", "req-1", "key-1", 0.42, now as u64,
    )]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_auth_wallet_and_usage_for_tests(
                    auth_repository,
                    Arc::new(InMemoryWalletRepository::seed(vec![])),
                    usage_repository,
                )
                .with_request_candidate_repository(candidate_repository)
                .with_system_config_values_for_tests([
                    ("rate_limit_per_minute".to_string(), json!(60)),
                    ("daily_usage_limit_usd".to_string(), json!(0.10)),
                ])
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/quota/status")))
            .send()
            .await
            .expect("request should succeed");

    let status = response.status();
    let body = response
        .text()
        .await
        .expect("response body should be readable");
    assert_eq!(status, StatusCode::OK, "unexpected response body: {body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json body should parse");

    assert_eq!(payload["total"], json!(2));
    assert!(payload["window"]["start"].is_string());
    assert!(payload["window"]["end"].is_string());

    let keys = payload["keys"].as_array().expect("keys should be an array");
    let key_1 = keys
        .iter()
        .find(|key| key["api_key_id"] == json!("key-1"))
        .expect("key-1 should be present");
    assert_eq!(key_1["rpm"]["current"], json!(2));
    assert_eq!(key_1["rpm"]["limit"], json!(1));
    assert_eq!(key_1["rpm"]["limited"], json!(true));
    assert_eq!(key_1["concurrency"]["current"], json!(1));
    assert_eq!(key_1["concurrency"]["limit"], json!(1));
    assert_eq!(key_1["concurrency"]["limited"], json!(true));
    assert_eq!(key_1["daily_usage"]["used_usd"], 0.42);
    assert_eq!(key_1["daily_usage"]["limit_usd"], json!(0.30));
    assert_eq!(key_1["daily_usage"]["limited"], json!(true));

    let key_2 = keys
        .iter()
        .find(|key| key["api_key_id"] == json!("key-2"))
        .expect("key-2 should be present");
    assert_eq!(key_2["rpm"]["current"], json!(0));
    assert_eq!(key_2["rpm"]["limit"], json!(120));
    assert_eq!(key_2["rpm"]["limited"], json!(false));
    assert_eq!(key_2["concurrency"]["current"], json!(0));
    assert_eq!(key_2["concurrency"]["limit"], json!(null));
    assert_eq!(key_2["concurrency"]["limited"], json!(false));
    assert_eq!(key_2["daily_usage"]["used_usd"], json!(0.0));
    // key-2 has no explicit daily limit: falls back to the system default.
    assert_eq!(key_2["daily_usage"]["limit_usd"], json!(0.10));
    assert_eq!(key_2["daily_usage"]["limited"], json!(false));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_reports_admin_quota_status_with_zero_usage_and_no_keys() {
    run_quota_status_test(
        "quota-status-zero-usage",
        gateway_reports_admin_quota_status_with_zero_usage_and_no_keys_inner,
    );
}

async fn gateway_reports_admin_quota_status_with_zero_usage_and_no_keys_inner() {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_quota_status_upstream("/api/admin/quota/status").await;

    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            None,
            sample_standalone_api_key_snapshot("key-idle", "user-1", true),
        )])
        .with_export_records([sample_standalone_export_record(
            "key-idle",
            "user-1",
            "sk-key-idle-plaintext",
            true,
            Some(5),
            Some(2),
            Some(1.0),
        )]),
    );
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_auth_wallet_and_usage_for_tests(
                    auth_repository,
                    Arc::new(InMemoryWalletRepository::seed(vec![])),
                    Arc::new(InMemoryUsageReadRepository::seed(vec![])),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/quota/status")))
            .send()
            .await
            .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");

    assert_eq!(payload["total"], json!(1));
    let key = &payload["keys"][0];
    assert_eq!(key["api_key_id"], json!("key-idle"));
    assert_eq!(key["rpm"]["current"], json!(0));
    assert_eq!(key["rpm"]["limit"], json!(5));
    assert_eq!(key["rpm"]["limited"], json!(false));
    assert_eq!(key["concurrency"]["current"], json!(0));
    assert_eq!(key["concurrency"]["limit"], json!(2));
    assert_eq!(key["concurrency"]["limited"], json!(false));
    assert_eq!(key["daily_usage"]["used_usd"], 0.0);
    assert_eq!(key["daily_usage"]["limit_usd"], json!(1.0));
    assert_eq!(key["daily_usage"]["limited"], json!(false));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_reports_admin_quota_status_empty_when_no_standalone_keys() {
    run_quota_status_test(
        "quota-status-empty",
        gateway_reports_admin_quota_status_empty_when_no_standalone_keys_inner,
    );
}

async fn gateway_reports_admin_quota_status_empty_when_no_standalone_keys_inner() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_auth_wallet_and_usage_for_tests(
                    auth_repository,
                    Arc::new(InMemoryWalletRepository::seed(vec![])),
                    Arc::new(InMemoryUsageReadRepository::seed(vec![])),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/quota/status")))
            .send()
            .await
            .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], json!(0));
    assert_eq!(payload["keys"], json!([]));

    gateway_handle.abort();
}
