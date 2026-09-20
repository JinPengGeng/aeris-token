//! Synthetic provider output, real loopback HTTP and PostgreSQL settlement.
//! These fixtures are not provider invoices. Receipt bodies contain output facts;
//! the production evidence adapter calculates all charges from the frozen quote.
use super::*;
use futures_util::FutureExt;
use sha2::{Digest, Sha256};
use std::sync::RwLock;

struct AbortTask<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortTask<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct MutablePricing(RwLock<StoredBillingModelContext>);

impl MutablePricing {
    fn new(high: f64, medium: f64) -> Self {
        let mut model = pricing("a");
        model.default_tiered_pricing = Some(json!({
            "image_output_prices":{"1024x1024":{"high":high,"medium":medium,"low":0.01}}
        }));
        Self(RwLock::new(model))
    }

    fn replace(&self, high: f64, medium: f64) {
        *self.0.write().unwrap() = Self::new(high, medium).0.into_inner().unwrap();
    }

    fn snapshot(&self) -> InMemoryBillingReadRepository {
        InMemoryBillingReadRepository::seed([self.0.read().unwrap().clone()])
    }
}

#[async_trait::async_trait]
impl BillingReadRepository for MutablePricing {
    async fn find_model_context(
        &self,
        provider: &str,
        key: Option<&str>,
        model: &str,
    ) -> Result<Option<StoredBillingModelContext>, aether_data::DataLayerError> {
        self.snapshot()
            .find_model_context(provider, key, model)
            .await
    }

    async fn find_model_context_by_model_id(
        &self,
        provider: &str,
        key: Option<&str>,
        model: &str,
    ) -> Result<Option<StoredBillingModelContext>, aether_data::DataLayerError> {
        self.snapshot()
            .find_model_context_by_model_id(provider, key, model)
            .await
    }
}

struct SyntheticUpstream {
    root: String,
    calls: Arc<AtomicUsize>,
    seen: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    _task: AbortTask<()>,
}

async fn synthetic_upstream(receipt: Value) -> SyntheticUpstream {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (counter, notifier, gate) = (calls.clone(), seen.clone(), release.clone());
    let router = axum::Router::new().route(
        "/v1/images/generations",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let (counter, notifier, gate, receipt) = (
                counter.clone(),
                notifier.clone(),
                gate.clone(),
                receipt.clone(),
            );
            async move {
                assert_eq!(body["n"], 8);
                counter.fetch_add(1, Ordering::SeqCst);
                notifier.notify_one();
                gate.notified().await;
                axum::Json(receipt)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let root = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    SyntheticUpstream {
        root,
        calls,
        seen,
        release,
        _task: AbortTask(task),
    }
}

async fn request_bytes(gateway: &str, request_id: &str) -> (http::StatusCode, Result<Vec<u8>, ()>) {
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(25))
        .build()
        .unwrap()
        .post(format!("{gateway}/v1/images/generations"))
        .bearer_auth("sk-public-image-fixture")
        .header(crate::constants::TRACE_ID_HEADER, request_id)
        .json(&request_body())
        .send()
        .await
        .expect("synthetic public request transport failed");
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| {
            assert!(!error.is_timeout(), "public response deadline exceeded");
        });
    (status, bytes)
}

async fn request(gateway: &str, request_id: &str) -> (http::StatusCode, Value) {
    let (status, bytes) = request_bytes(gateway, request_id).await;
    let bytes = bytes.expect("synthetic public response did not complete");
    let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        panic!(
            "public response is not JSON; status={status}, bytes={}",
            bytes.len()
        )
    });
    (status, body)
}

async fn quote_hash(fixture: &Fixture, request_id: &str) -> String {
    let quote: Value =
        sqlx::query_scalar("SELECT quote FROM request_fund_reservations WHERE request_id=$1")
            .bind(request_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    format!("{:x}", Sha256::digest(serde_json::to_vec(&quote).unwrap()))
}

async fn wait_for_closed_admission(fixture: &Fixture, request_id: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let closed: Option<bool> = sqlx::query_scalar(
                "SELECT funds_admission_closed_at IS NOT NULL FROM usage WHERE request_id=$1",
            )
            .bind(request_id)
            .fetch_optional(&fixture.pool)
            .await
            .unwrap();
            if closed == Some(true) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("public request financial admission did not close");
}

async fn snapshot(fixture: &Fixture, request_id: &str) -> Value {
    let parent: (String, String, i64) = sqlx::query_as(
        "SELECT status,billing_status,ROUND(COALESCE(actual_total_cost_usd,0)*100000000)::bigint FROM usage WHERE request_id=$1")
        .bind(request_id).fetch_one(&fixture.pool).await.unwrap();
    let wallet: (i64, i64) = sqlx::query_as(
        "SELECT ROUND(balance*100000000)::bigint,ROUND(total_consumed*100000000)::bigint FROM wallets WHERE id='wallet'")
        .fetch_one(&fixture.pool).await.unwrap();
    let attempt: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),SUM((quote->>'authorized_cost_units')::bigint)::bigint,COALESCE(SUM(actual_cost_units),0)::bigint,SUM(collected_cost_units)::bigint FROM request_fund_reservations WHERE request_id=$1")
        .bind(request_id).fetch_one(&fixture.pool).await.unwrap();
    let allocations: (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),SUM(a.reserved_cost_units)::bigint,SUM(a.collected_cost_units)::bigint FROM request_fund_allocations a JOIN request_fund_reservations r USING(reservation_token) WHERE r.request_id=$1")
        .bind(request_id).fetch_one(&fixture.pool).await.unwrap();
    let provider: (i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(request_count,0)::bigint,COALESCE(error_count,0)::bigint,ROUND(COALESCE(total_cost_usd,0)*100000000)::bigint FROM provider_api_keys WHERE id='pk-a'")
        .fetch_one(&fixture.pool).await.unwrap();
    let daily: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(actual_cost_units),0)::bigint FROM usage_daily_cost_contributions WHERE request_id=$1")
        .bind(request_id).fetch_one(&fixture.pool).await.unwrap();
    let requests: i64 =
        sqlx::query_scalar("SELECT total_requests::bigint FROM api_keys WHERE id='key-a'")
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    let outbox: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM usage_counter_deltas WHERE request_id=$1")
            .bind(request_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    json!({"parent":parent,"wallet":wallet,"attempt":attempt,"allocations":allocations,
        "provider":provider,"api_key_requests":requests,"daily_units":daily,"outbox_rows":outbox,
        "quota":fixture.quota_totals().await,"summary":fixture.summary(request_id).await,
        "quote_sha256":quote_hash(fixture,request_id).await})
}

fn assert_finances(audit: &Value, initial: i64, actual: Option<i64>, reconciliation: bool) {
    let collected = actual.unwrap_or(0).min(8_000_000);
    let held = if actual.is_some() { 0 } else { 8_000_000 };
    assert_eq!(
        audit["attempt"],
        json!([1, 8_000_000, actual.unwrap_or(0), collected])
    );
    assert_eq!(audit["allocations"], json!([1, 8_000_000, collected]));
    assert_eq!(audit["wallet"], json!([initial - collected, collected]));
    assert_eq!(
        audit["summary"]["known_actual_cost_units"],
        actual.unwrap_or(0)
    );
    assert_eq!(audit["summary"]["collected_cost_units"], collected);
    assert_eq!(audit["summary"]["held_cost_units"], held);
    assert_eq!(
        audit["summary"]["unknown_attempts"],
        i64::from(actual.is_none())
    );
    assert_eq!(audit["summary"]["requires_reconciliation"], reconciliation);
    assert_eq!(audit["summary"]["admission_closed"], true);
    assert_eq!(
        audit["parent"][1],
        if reconciliation { "pending" } else { "settled" }
    );
    assert_eq!(audit["parent"][2], actual.unwrap_or(0));
    assert_eq!(audit["quota"], json!([1, held, actual.unwrap_or(0)]));
    assert_eq!(audit["daily_units"], actual.unwrap_or(0));
    assert_eq!(audit["provider"][0], 1);
    assert_eq!(audit["provider"][2], actual.unwrap_or(0));
    assert_eq!(audit["api_key_requests"], 1);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; uses synthetic provider receipts over real loopback HTTP"]
async fn live_public_synthetic_image_receipt_matrix_uses_frozen_prices_and_retains_unknown() {
    let mut cheaper_output = image(3);
    cheaper_output["quality"] = json!("medium");
    let mut dearer_output = image(6);
    dearer_output["quality"] = json!("medium");
    let mut cases = vec![
        ("full", image(8), Some(8_000_000), false),
        ("partial", image(6), Some(6_000_000), false),
        ("over-count", image(9), Some(9_000_000), true),
        ("different-rate", cheaper_output, Some(6_000_000), true),
        ("over-price", dearer_output, Some(12_000_000), true),
    ];
    for field in ["size", "quality", "output_format"] {
        let mut receipt = image(6);
        receipt.as_object_mut().unwrap().remove(field);
        cases.push((field, receipt, None, true));
    }
    let mut malformed = image(6);
    malformed["data"][0] = json!({});
    cases.push(("malformed-output", malformed, None, true));
    let mut unknown_size = image(6);
    unknown_size["size"] = json!("4096x4096");
    cases.push(("unknown-size", unknown_size, None, true));
    let mut invalid_usage = image(6);
    invalid_usage["usage"] = json!({"input_tokens":-1});
    cases.push(("invalid-usage", invalid_usage, None, true));

    for (name, receipt, actual, reconciliation) in cases {
        let fixture = Fixture::new(0.20).await;
        fixture.hard_cost_policy(0.20).await;
        let upstream = synthetic_upstream(receipt).await;
        let state = fixture
            .public_state_with_models(
                Account::User,
                &upstream.root,
                Arc::new(MutablePricing::new(0.01, 0.02)),
            )
            .await;
        let (gateway, gateway_task) = public_server(state.clone()).await;
        let gateway_task = AbortTask(gateway_task);
        let result = std::panic::AssertUnwindSafe(async {
            let request_id = format!("synthetic-{name}");
            upstream.release.notify_one();
            let (status, _) = request(&gateway, &request_id).await;
            assert_eq!(status, http::StatusCode::OK, "case={name}");
            SqlxUsageReadRepository::new(fixture.pool.clone())
                .flush_usage_counter_deltas(1000)
                .await
                .unwrap();
            let audit = snapshot(&fixture, &request_id).await;
            assert_finances(&audit, 20_000_000, actual, reconciliation);
            assert_eq!(upstream.calls.load(Ordering::SeqCst), 1, "case={name}");
            println!(
                "AETHER_SYNTHETIC_RECEIPT_EVIDENCE {}",
                json!({"case":name,"synthetic":true,"upstream_calls":1,"audit":audit})
            );
        })
        .catch_unwind()
        .await;
        drop(gateway_task);
        drop(upstream);
        fixture.close(state).await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; changes only a test pricing catalog during local HTTP"]
async fn live_public_synthetic_image_receipt_preserves_quote_across_catalog_price_change() {
    let fixture = Fixture::new(0.20).await;
    fixture.hard_cost_policy(0.20).await;
    let upstream = synthetic_upstream(image(6)).await;
    let models = Arc::new(MutablePricing::new(0.01, 0.02));
    let state = fixture
        .public_state_with_models(Account::User, &upstream.root, models.clone())
        .await;
    let (gateway, gateway_task) = public_server(state.clone()).await;
    let gateway_task = AbortTask(gateway_task);
    let result = std::panic::AssertUnwindSafe(async {
        let request_id = "synthetic-catalog-change";
        let request_gateway = gateway.clone();
        let mut inflight = AbortTask(tokio::spawn(async move { request(&request_gateway,request_id).await }));
        tokio::select! {
            _ = upstream.seen.notified() => (),
            response = &mut inflight.0 => panic!("request ended before held quote inspection; joined={}",response.is_ok()),
            _ = tokio::time::sleep(Duration::from_secs(15)) => panic!("synthetic upstream dispatch deadline"),
        }
        let frozen_hash = quote_hash(&fixture,request_id).await;
        assert_eq!(fixture.summary(request_id).await.held_cost_units,8_000_000);
        models.replace(0.50,0.75);
        upstream.release.notify_one();
        let (status,body) = (&mut inflight.0).await.unwrap();
        assert_eq!(status,http::StatusCode::OK);
        assert_eq!(body["data"].as_array().map(Vec::len),Some(6));
        SqlxUsageReadRepository::new(fixture.pool.clone()).flush_usage_counter_deltas(1000).await.unwrap();
        let audit = snapshot(&fixture,request_id).await;
        assert_finances(&audit,20_000_000,Some(6_000_000),false);
        assert_eq!(audit["quote_sha256"],frozen_hash);
        // Model contexts have a normal 30-second cache. Use a new Gateway state
        // with the same durable funds and mutated catalog to prove the new price
        // is effective, without adding a test-only production cache bypass.
        let fresh_state = fixture.public_state_with_models(Account::User,&upstream.root,models.clone()).await;
        let (fresh_gateway,fresh_task) = public_server(fresh_state.clone()).await;
        let fresh_task = AbortTask(fresh_task);
        // Its USD 4.00 quote exceeds both remaining cash and quota.
        let (_,denied) = request(&fresh_gateway,"synthetic-catalog-new-request").await;
        assert!(denied.get("error").is_some());
        let admitted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM request_fund_reservations WHERE request_id='synthetic-catalog-new-request'")
            .fetch_one(&fixture.pool).await.unwrap();
        assert_eq!(admitted,0);
        // Replay through a Gateway whose model cache now holds the changed price.
        // Repricing from that cache would conflict with the original USD .06
        // terminal facts; only the persisted frozen quote can reproduce them.
        let duplicate = late_receipt_event(&fixture,&fresh_state,request_id,&image(6)).await;
        fresh_state.usage_runtime.persist_attempt_funds_event(fresh_state.usage_lifecycle_data_state().as_ref(),duplicate).await.unwrap();
        assert_eq!(fixture.summary(request_id).await.known_actual_cost_units,6_000_000);
        assert_eq!(upstream.calls.load(Ordering::SeqCst),1);
        assert_eq!(quote_hash(&fixture,request_id).await,frozen_hash);
        assert_eq!(fixture.balance().await,0.14);
        drop(fresh_task);
        fresh_state.usage_runtime.shutdown(Duration::from_secs(10)).await.unwrap();
        println!("AETHER_SYNTHETIC_RECEIPT_EVIDENCE {}",json!({"case":"catalog-price-change","synthetic":true,"upstream_calls":1,"new_request_admitted":false,"audit":audit}));
    }).catch_unwind().await;
    drop(gateway_task);
    drop(upstream);
    fixture.close(state).await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

async fn truncated_upstream(short_http_body: bool) -> (String, Arc<AtomicUsize>, AbortTask<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let root = format!("http://{}/v1", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            let header_end = loop {
                let size = socket.read(&mut chunk).await.unwrap();
                assert!(size > 0);
                request.extend_from_slice(&chunk[..size]);
                if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    break index + 4;
                }
                assert!(request.len() < 65_536);
            };
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            assert!(headers.starts_with("POST /v1/images/generations HTTP/1.1\r\n"));
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .expect("complete synthetic upstream request length");
            while request.len() < header_end + length {
                let size = socket.read(&mut chunk).await.unwrap();
                assert!(size > 0);
                request.extend_from_slice(&chunk[..size]);
            }
            let body: Value =
                serde_json::from_slice(&request[header_end..header_end + length]).unwrap();
            assert_eq!(body["n"], 8);
            counter.fetch_add(1, Ordering::SeqCst);
            let partial = br#"{"data":[{"b64_json":"dGVzdA=="}"#;
            let declared = partial.len() + if short_http_body { 100 } else { 0 };
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n");
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.write_all(partial).await.unwrap();
            socket.shutdown().await.unwrap();
        }
    });
    (root, calls, AbortTask(task))
}

async fn late_receipt_event(
    fixture: &Fixture,
    state: &AppState,
    request_id: &str,
    receipt: &Value,
) -> UsageEvent {
    let identity = fixture.identity(request_id, "p-a").await;
    let execution = state
        .data
        .read_request_attempt_funds(identity.clone())
        .await
        .unwrap()
        .terminal_facts
        .unwrap()
        .execution;
    UsageEvent::new(
        UsageEventType::Failed,
        request_id,
        UsageEventData {
            attempt_funds: Some(Box::new(UsageAttemptFundsEvent {
                schema_version: 1,
                identity,
                action: UsageAttemptFundsAction::Outcome {
                    execution,
                    evidence: image_output_evidence(Some(receipt)),
                },
            })),
            ..UsageEventData::default()
        },
    )
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; synthetic truncated HTTP and late receipt replay"]
async fn live_public_synthetic_image_truncated_receipt_retains_hold_then_settles_once() {
    for short_http_body in [false, true] {
        // One USD .08 hold leaves only .02: a retry cannot issue another send.
        let fixture = Fixture::new(0.10).await;
        fixture.hard_cost_policy(0.20).await;
        let (root, calls, upstream_task) = truncated_upstream(short_http_body).await;
        let state = fixture.public_state(Account::User, &root).await;
        let (gateway, gateway_task) = public_server(state.clone()).await;
        let gateway_task = AbortTask(gateway_task);
        let result = std::panic::AssertUnwindSafe(async {
            let request_id = "synthetic-truncated-receipt";
            // A truncated upstream may also truncate the public body; neither
            // incomplete transport nor invalid JSON proves the operation free.
            let _ = request_bytes(&gateway,request_id).await;
            wait_for_closed_admission(&fixture,request_id).await;
            let usage = SqlxUsageReadRepository::new(fixture.pool.clone());
            usage.flush_usage_counter_deltas(1000).await.unwrap();
            let held = snapshot(&fixture,request_id).await;
            assert_finances(&held,10_000_000,None,true);
            assert_eq!(calls.load(Ordering::SeqCst),1);
            let late = late_receipt_event(&fixture,&state,request_id,&image(6)).await;
            state.usage_runtime.persist_attempt_funds_event(state.usage_lifecycle_data_state().as_ref(),late.clone()).await.unwrap();
            usage.flush_usage_counter_deltas(1000).await.unwrap();
            let settled = snapshot(&fixture,request_id).await;
            assert_finances(&settled,10_000_000,Some(6_000_000),false);
            assert_eq!(settled["quote_sha256"],held["quote_sha256"]);
            assert_eq!(settled["parent"][0],held["parent"][0]);
            let deleted = usage.cleanup_processed_usage_counter_deltas(crate::clock::current_unix_ms()/1000+3600,1000).await.unwrap();
            assert!(deleted>0);
            let cleaned = snapshot(&fixture,request_id).await;
            assert_eq!(cleaned["outbox_rows"],0);
            let mut delayed_unknown = late.clone();
            let UsageAttemptFundsAction::Outcome { evidence,.. } = &mut delayed_unknown.data.attempt_funds.as_mut().unwrap().action else { unreachable!() };
            *evidence = UsageAttemptChargeEvidence::Unknown;
            for event in [late.clone(),delayed_unknown,late] {
                state.usage_runtime.persist_attempt_funds_event(state.usage_lifecycle_data_state().as_ref(),event).await.unwrap();
            }
            usage.flush_usage_counter_deltas(1000).await.unwrap();
            assert_eq!(snapshot(&fixture,request_id).await,cleaned);
            assert_eq!(calls.load(Ordering::SeqCst),1);
            println!("AETHER_SYNTHETIC_RECEIPT_EVIDENCE {}",json!({"case":if short_http_body {"short-http-body"} else {"truncated-json"},"synthetic":true,"upstream_calls":1,"audit":cleaned}));
        }).catch_unwind().await;
        drop(gateway_task);
        drop(upstream_task);
        fixture.close(state).await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
