use super::*;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use aether_data::driver::postgres::{
    run_migrations, SqlxSettlementRepository, SqlxUsageReadRepository, SqlxWalletRepository,
};
use aether_data::repository::billing::InMemoryBillingReadRepository;
use aether_data_contracts::repository::billing::StoredBillingModelContext;
use aether_data_contracts::repository::settlement::RequestFundsSummary;
use aether_data_contracts::repository::usage::UsageWriteRepository;
use aether_usage_runtime::UsageRuntimeConfig;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::control::GatewayControlAuthContext;
use crate::data::GatewayDataState;
use crate::execution_runtime::sync::execute_execution_runtime_sync_with_retry_scope;

#[path = "funded_image_public_tests.rs"]
mod public_tests;

struct Fixture {
    admin: PgPool,
    pool: PgPool,
    schema: String,
}

impl Fixture {
    async fn new(balance: f64) -> Self {
        let url = std::env::var("AETHER_TEST_DATABASE_URL")
            .expect("requires an isolated AETHER_TEST_DATABASE_URL");
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        run_migrations(&admin).await.unwrap();
        let schema = format!("gateway_attempt_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let options = url
            .parse::<sqlx::postgres::PgConnectOptions>()
            .unwrap()
            .options([("search_path", schema.as_str())]);
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .unwrap();
        for table in [
            "users",
            "api_keys",
            "wallets",
            "billing_plans",
            "user_plan_entitlements",
            "entitlement_usage_ledgers",
            "usage",
            "usage_http_audits",
            "usage_body_blobs",
            "usage_routing_snapshots",
            "usage_settlement_snapshots",
            "usage_counter_deltas",
            "request_fund_reservations",
            "request_fund_allocations",
            "request_fund_recoveries",
            "request_fund_collection_receipts",
            "wallet_transactions",
            "refund_requests",
            "providers",
            "provider_api_keys",
            "global_models",
        ] {
            sqlx::query(&format!(
                "CREATE TABLE {table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&pool)
            .await
            .unwrap();
        }
        let view: String = sqlx::query_scalar(
            "SELECT pg_get_viewdef('public.usage_billing_facts'::regclass, true)",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        sqlx::raw_sql(&format!(
            "CREATE VIEW usage_billing_facts AS {}",
            view.replace("public.", &format!("{schema}."))
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql("INSERT INTO users(id,username,email_verified) VALUES('owner','owner',false); INSERT INTO api_keys(id,user_id,key_hash,name) VALUES('key-a','owner','placeholder','a'); INSERT INTO providers(id,name,provider_type,created_at,updated_at) VALUES('p-a','a','custom',NOW(),NOW()),('p-b','b','custom',NOW(),NOW()); INSERT INTO provider_api_keys(id,provider_id,name,api_key,total_tokens,total_cost_usd,created_at,updated_at) VALUES('pk-a','p-a','a','placeholder',0,0,NOW(),NOW()),('pk-b','p-b','b','placeholder',0,0,NOW(),NOW());").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO wallets(id,user_id,balance,gift_balance,status,limit_mode,created_at,updated_at) VALUES('wallet','owner',$1,0,'active','finite',NOW(),NOW())").bind(balance).execute(&pool).await.unwrap();
        Self {
            admin,
            pool,
            schema,
        }
    }

    fn state(&self) -> AppState {
        let billing = Arc::new(InMemoryBillingReadRepository::seed([
            pricing("a"),
            pricing("b"),
        ]));
        let usage = Arc::new(SqlxUsageReadRepository::new(self.pool.clone()));
        let wallet = Arc::new(SqlxWalletRepository::new(self.pool.clone()));
        let settlement = Arc::new(SqlxSettlementRepository::new(self.pool.clone()));
        AppState::new()
            .unwrap()
            .with_data_state_for_tests(
                GatewayDataState::with_usage_billing_and_wallet_for_tests(usage, billing, wallet)
                    .with_settlement_writer_for_tests(settlement),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                queue_terminal_events: false,
                ..UsageRuntimeConfig::default()
            })
    }

    async fn summary(&self, request: &str) -> RequestFundsSummary {
        let value: Value = sqlx::query_scalar(
            "SELECT request_funds_summary FROM usage_settlement_snapshots WHERE request_id=$1",
        )
        .bind(request)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        serde_json::from_value(value).unwrap()
    }

    async fn balance(&self) -> f64 {
        sqlx::query_scalar("SELECT balance::double precision FROM wallets WHERE id='wallet'")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn identity(&self, request: &str, provider: &str) -> RequestAttemptFundsIdentity {
        let (attempt_id,token):(String,String) = sqlx::query_as("SELECT attempt_id::text,reservation_token FROM request_fund_reservations WHERE request_id=$1 AND provider_id=$2").bind(request).bind(provider).fetch_one(&self.pool).await.unwrap();
        RequestAttemptFundsIdentity {
            attempt_id,
            request: RequestFundsIdentity {
                reservation_token: token,
                request_id: request.to_string(),
                user_id: Some("owner".into()),
                api_key_id: Some("key-a".into()),
                api_key_is_standalone: false,
            },
        }
    }

    async fn close(self, state: AppState) {
        state
            .usage_runtime
            .shutdown(Duration::from_secs(10))
            .await
            .unwrap();
        drop(state);
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

fn pricing(suffix: &str) -> StoredBillingModelContext {
    StoredBillingModelContext {
        provider_id: format!("p-{suffix}"),
        provider_billing_type: Some("pay_as_you_go".into()),
        provider_api_key_id: Some(format!("pk-{suffix}")),
        provider_api_key_rate_multipliers: None,
        provider_api_key_cache_ttl_minutes: None,
        global_model_id: "image-model".into(),
        global_model_name: "image-model".into(),
        global_model_config: None,
        default_price_per_request: Some(0.0),
        default_tiered_pricing: Some(
            json!({"image_output_prices":{"1024x1024":{"high":0.01,"medium":0.01,"low":0.01}}}),
        ),
        model_id: Some("image".into()),
        model_provider_model_name: Some("image".into()),
        model_config: None,
        model_price_per_request: None,
        model_tiered_pricing: None,
    }
}

fn decision() -> GatewayControlDecision {
    let mut decision = GatewayControlDecision::synthetic(
        "/v1/images/generations",
        Some("ai_public".into()),
        Some("openai".into()),
        Some("image".into()),
        Some("openai:image".into()),
    )
    .with_execution_runtime_candidate(true);
    decision.auth_context = Some(GatewayControlAuthContext {
        user_id: "owner".into(),
        api_key_id: "key-a".into(),
        username: None,
        api_key_name: None,
        api_key_billing_multiplier: 1.0,
        balance_remaining: None,
        access_allowed: true,
        user_rate_limit: None,
        api_key_rate_limit: None,
        user_daily_usage_limit_usd: None,
        api_key_daily_usage_limit_usd: None,
        api_key_is_standalone: false,
        admin_bypass_limits: false,
        ip_bypass_limits: false,
        local_rejection: None,
        allowed_models: None,
        ip_rules: None,
        verified_api_key_hash: None,
    });
    decision
}

fn plan(request: &str, suffix: &str, url: &str) -> ExecutionPlan {
    ExecutionPlan {
        request_id: request.into(),
        candidate_id: None,
        provider_name: Some(format!("provider-{suffix}")),
        provider_id: format!("p-{suffix}"),
        endpoint_id: format!("e-{suffix}"),
        key_id: format!("pk-{suffix}"),
        method: "POST".into(),
        url: url.into(),
        headers: BTreeMap::new(),
        content_type: Some("application/json".into()),
        content_encoding: None,
        body: aether_contracts::RequestBody::from_json(
            json!({"model":"image","prompt":"test","n":8,"size":"1024x1024","quality":"high","output_format":"png"}),
        ),
        stream: false,
        client_api_format: "openai:image".into(),
        provider_api_format: "openai:image".into(),
        model_name: Some("image".into()),
        proxy: None,
        transport_profile: None,
        timeouts: None,
    }
}

fn context() -> Value {
    json!({"user_id":"owner","api_key_id":"key-a","model_id":"image","global_model_name":"image-model","client_api_format":"openai:image","provider_api_format":"openai:image","image_request":{"operation":"generate"},"request_path":"/v1/images/generations","mapped_model":"image","candidate_index":0,"retry_index":0})
}

fn image(count: usize) -> Value {
    json!({"created":1,"data":vec![json!({"b64_json":"dGVzdA=="});count],"size":"1024x1024","quality":"high","output_format":"png"})
}

async fn upstream(
    responses: Vec<(u16, Value)>,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/images/generations",
        listener.local_addr().unwrap()
    );
    let count = Arc::new(AtomicUsize::new(0));
    let counter = count.clone();
    let task = tokio::spawn(async move {
        // Keep listening after the planned responses: a forbidden retry must
        // produce an observable HTTP hit, rather than a refused connection.
        let mut responses = VecDeque::from(responses);
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 65536];
            let size = socket.read(&mut bytes).await.unwrap();
            assert!(size > 0);
            let (status, body) = if bytes[..size].starts_with(b"GET /__barrier ") {
                (200, json!({"barrier":true}))
            } else {
                counter.fetch_add(1, Ordering::SeqCst);
                responses.pop_front().unwrap_or_else(|| (200, image(6)))
            };
            let body = body.to_string();
            let response=format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (url, count, task)
}

async fn assert_upstream_calls(url: &str, count: &AtomicUsize, expected: usize) {
    let parsed = url::Url::parse(url).unwrap();
    let mut socket =
        tokio::net::TcpStream::connect((parsed.host_str().unwrap(), parsed.port().unwrap()))
            .await
            .unwrap();
    socket
        .write_all(b"GET /__barrier HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    assert!(String::from_utf8(response).unwrap().contains("barrier"));
    assert_eq!(count.load(Ordering::SeqCst), expected);
}

async fn execute(
    state: &AppState,
    request: &str,
    suffix: &str,
    url: &str,
) -> Result<
    aether_ai_serving::AiAttemptExecutionOutcome<http::Response<axum::body::Body>>,
    GatewayError,
> {
    let mut report_context = context();
    report_context["candidate_index"] = json!(if suffix == "a" { 0 } else { 1 });
    execute_execution_runtime_sync_with_retry_scope(
        state,
        "/v1/images/generations",
        plan(request, suffix, url),
        request,
        &decision(),
        "openai_image_sync",
        None,
        Some(report_context),
    )
    .await
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL and performs actual local HTTP calls"]
async fn live_gateway_image_attempts_retry_late_charge_and_replay() {
    let fixture = Fixture::new(0.20).await;
    let state = fixture.state();
    let (url, count, server) = upstream(vec![
        (500, json!({"error":{"message":"temporarily unavailable"}})),
        (200, image(6)),
    ])
    .await;
    request_scope(&state, async {
        let first = execute(&state, "gateway-retry", "a", &url).await?;
        assert!(matches!(
            first,
            aether_ai_serving::AiAttemptExecutionOutcome::Retry { .. }
        ));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture.summary("gateway-retry").await.held_cost_units,
            8_000_000
        );
        let second = execute(&state, "gateway-retry", "b", &url).await?;
        assert!(matches!(
            second,
            aether_ai_serving::AiAttemptExecutionOutcome::Responded(_)
        ));
        Ok(())
    })
    .await
    .unwrap();
    assert_upstream_calls(&url, &count, 2).await;
    server.abort();
    let summary = fixture.summary("gateway-retry").await;
    assert_eq!(
        (
            summary.known_actual_cost_units,
            summary.held_cost_units,
            summary.unknown_attempts
        ),
        (6_000_000, 8_000_000, 1)
    );
    assert!(summary.admission_closed);
    assert_eq!(fixture.balance().await, 0.14);
    let first_identity = fixture.identity("gateway-retry", "p-a").await;
    let execution = state
        .data
        .read_request_attempt_funds(first_identity.clone())
        .await
        .unwrap()
        .terminal_facts
        .unwrap()
        .execution;
    let late = UsageEvent::new(
        UsageEventType::Failed,
        "gateway-retry",
        UsageEventData {
            attempt_funds: Some(Box::new(UsageAttemptFundsEvent {
                schema_version: 1,
                identity: first_identity,
                action: UsageAttemptFundsAction::Outcome {
                    execution,
                    evidence: image_output_evidence(Some(&image(7))),
                },
            })),
            ..UsageEventData::default()
        },
    );
    state
        .usage_runtime
        .persist_attempt_funds_event(state.usage_lifecycle_data_state().as_ref(), late.clone())
        .await
        .unwrap();
    assert_eq!(fixture.balance().await, 0.07);
    let summary = fixture.summary("gateway-retry").await;
    assert!(!summary.requires_reconciliation);
    let lifecycle: (String, String) =
        sqlx::query_as("SELECT status,billing_status FROM usage WHERE request_id='gateway-retry'")
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    assert_eq!(lifecycle, ("completed".to_string(), "settled".to_string()));
    assert_eq!(
        (
            summary.known_actual_cost_units,
            summary.collected_cost_units,
            summary.held_cost_units
        ),
        (13_000_000, 13_000_000, 0)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM usage WHERE request_id='gateway-retry'")
            .fetch_one(&fixture.pool)
            .await
            .unwrap(),
        1
    );
    let by_provider:Vec<(String,f64)>=sqlx::query_as("SELECT provider_id,SUM((terminal_facts->'outcome'->'usage'->>'actual_cost_units')::double precision)/100000000 FROM request_fund_reservations WHERE request_id='gateway-retry' GROUP BY provider_id ORDER BY provider_id").fetch_all(&fixture.pool).await.unwrap();
    assert_eq!(
        by_provider,
        vec![("p-a".into(), 0.07), ("p-b".into(), 0.06)]
    );
    SqlxUsageReadRepository::new(fixture.pool.clone())
        .flush_usage_counter_deltas(1000)
        .await
        .unwrap();
    let counters: Vec<(String,i64,f64)> = sqlx::query_as("SELECT id,request_count::bigint,total_cost_usd::double precision FROM provider_api_keys ORDER BY id")
        .fetch_all(&fixture.pool).await.unwrap();
    assert_eq!(
        counters,
        vec![("pk-a".into(), 1, 0.07), ("pk-b".into(), 1, 0.06)]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT total_requests::bigint FROM api_keys WHERE id='key-a'"
        )
        .fetch_one(&fixture.pool)
        .await
        .unwrap(),
        1
    );
    sqlx::query("DELETE FROM usage_counter_deltas WHERE processed_at IS NOT NULL")
        .execute(&fixture.pool)
        .await
        .unwrap();
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_counter_deltas")
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
    state
        .usage_runtime
        .persist_attempt_funds_event(state.usage_lifecycle_data_state().as_ref(), late)
        .await
        .unwrap();
    assert_eq!(fixture.balance().await, 0.07);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM usage_counter_deltas")
            .fetch_one(&fixture.pool)
            .await
            .unwrap(),
        before
    );
    fixture.close(state).await;
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL and performs actual local HTTP calls"]
async fn live_gateway_image_attempts_reject_second_upstream_when_held() {
    let fixture = Fixture::new(0.10).await;
    let state = fixture.state();
    let (url, count, server) =
        upstream(vec![(500, json!({"error":{"message":"unknown charge"}}))]).await;
    request_scope(&state, async {
        let first = execute(&state, "gateway-low", "a", &url).await?;
        assert!(matches!(
            first,
            aether_ai_serving::AiAttemptExecutionOutcome::Retry { .. }
        ));
        assert!(execute(&state, "gateway-low", "b", &url).await.is_err());
        Ok(())
    })
    .await
    .unwrap();
    assert_upstream_calls(&url, &count, 1).await;
    server.abort();
    let summary = fixture.summary("gateway-low").await;
    assert_eq!(
        (summary.attempt_count, summary.held_cost_units),
        (1, 8_000_000)
    );
    assert!(summary.admission_closed);
    assert_eq!(fixture.balance().await, 0.10);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM usage WHERE request_id='gateway-low'")
            .fetch_one(&fixture.pool)
            .await
            .unwrap(),
        "failed"
    );
    fixture.close(state).await;
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL and performs actual local HTTP calls"]
async fn live_gateway_image_attempts_persistence_failure_sends_no_upstream() {
    for target in ["parent", "dispatch"] {
        let fixture = Fixture::new(0.20).await;
        let state = fixture.state();
        let trigger = if target == "parent" {
            "CREATE FUNCTION deny_parent() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected parent write failure'; END $$; CREATE TRIGGER deny_parent BEFORE INSERT ON usage FOR EACH ROW EXECUTE FUNCTION deny_parent();"
        } else {
            "CREATE FUNCTION deny_dispatch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.dispatched_at IS NOT NULL THEN RAISE EXCEPTION 'injected dispatch write failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER deny_dispatch BEFORE UPDATE ON request_fund_reservations FOR EACH ROW EXECUTE FUNCTION deny_dispatch();"
        };
        sqlx::raw_sql(trigger).execute(&fixture.pool).await.unwrap();
        let (url, count, server) = upstream(vec![(200, image(6))]).await;
        assert!(execute(&state, "gateway-failure", "a", &url).await.is_err());
        assert_upstream_calls(&url, &count, 0).await;
        server.abort();
        assert_eq!(fixture.balance().await, 0.20);
        if target == "dispatch" {
            let summary = fixture.summary("gateway-failure").await;
            assert_eq!((summary.held_cost_units, summary.prepared_attempts), (0, 0));
            assert!(summary.admission_closed);
        }
        fixture.close(state).await;
    }
}

#[test]
fn image_quote_rejects_unproven_projections_and_evidence() {
    let mut plan = plan("quote", "a", "http://localhost/v1/images/generations");
    let pricing = BillingModelPricingSnapshot::from(pricing("a"));
    assert_eq!(
        quote_final_image_projection(&plan, &pricing, 1.0)
            .unwrap()
            .upper_bound_units(),
        8_000_000
    );
    plan.body.json_body.as_mut().unwrap()["size"] = json!("auto");
    assert!(quote_final_image_projection(&plan, &pricing, 1.0).is_err());
    assert_eq!(
        image_output_evidence(Some(&json!({"data":[]}))),
        UsageAttemptChargeEvidence::Unknown
    );
    assert_eq!(
        image_output_evidence(Some(&json!({"error":{"message":"cancelled"}}))),
        UsageAttemptChargeEvidence::Unknown
    );
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL and performs an actual local HTTP call"]
async fn live_gateway_image_attempt_cancellation_preserves_dispatched_hold() {
    let fixture = Fixture::new(0.20).await;
    let state = fixture.state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/images/generations",
        listener.local_addr().unwrap()
    );
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = vec![0; 65536];
        assert!(socket.read(&mut bytes).await.unwrap() > 0);
        seen_tx.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    let executing_state = state.clone();
    let executing =
        tokio::spawn(async move { execute(&executing_state, "gateway-cancel", "a", &url).await });
    tokio::time::timeout(Duration::from_secs(10), seen_rx)
        .await
        .unwrap()
        .unwrap();
    executing.abort();
    assert!(executing.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let summary = fixture.summary("gateway-cancel").await;
            if summary.admission_closed && summary.unknown_attempts == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let summary = fixture.summary("gateway-cancel").await;
    assert_eq!(
        (summary.held_cost_units, summary.collected_cost_units),
        (8_000_000, 0)
    );
    assert_eq!(fixture.balance().await, 0.20);
    let execution_status: String = sqlx::query_scalar("SELECT terminal_facts->'execution'->>'status' FROM request_fund_reservations WHERE request_id='gateway-cancel'")
        .fetch_one(&fixture.pool).await.unwrap();
    assert_eq!(execution_status, "cancelled");
    server.abort();
    fixture.close(state).await;
}

async fn pause_reserve(
    fixture: &Fixture,
    provider: &str,
) -> (sqlx::pool::PoolConnection<sqlx::Postgres>, i64) {
    assert!(matches!(provider, "p-a" | "p-b"));
    let lock = i64::from(uuid::Uuid::new_v4().as_fields().0 & 0x7fff_ffff);
    let mut connection = fixture.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(lock)
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION pause_reserve() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.provider_id='{provider}' THEN PERFORM pg_advisory_xact_lock({lock}::bigint); END IF; RETURN NEW; END $$; CREATE TRIGGER pause_reserve BEFORE INSERT ON request_fund_reservations FOR EACH ROW EXECUTE FUNCTION pause_reserve();"
    ))
    .execute(&fixture.pool)
    .await
    .unwrap();
    (connection, lock)
}

async fn wait_for_blocked_reserve(fixture: &Fixture, lock: i64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND objid=$1::bigint::oid AND NOT granted)",
            )
            .bind(lock)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_parent_terminal(fixture: &Fixture, request: &str, status: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let terminal: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM usage WHERE request_id=$1 AND status=$2 AND funds_admission_closed_at IS NOT NULL)",
            )
            .bind(request)
            .bind(status)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
            if terminal {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL and performs actual local HTTP calls"]
async fn live_gateway_image_attempt_reserve_commit_after_cancellation_is_released() {
    let fixture = Fixture::new(0.20).await;
    let state = fixture.state();
    let (mut locked, lock) = pause_reserve(&fixture, "p-a").await;
    let (url, count, server) = upstream(vec![(200, image(6))]).await;
    let executing_state = state.clone();
    let executing_url = url.clone();
    let executing = tokio::spawn(async move {
        execute(
            &executing_state,
            "gateway-reserve-cancel",
            "a",
            &executing_url,
        )
        .await
    });
    wait_for_blocked_reserve(&fixture, lock).await;
    executing.abort();
    assert!(executing.await.unwrap_err().is_cancelled());
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(lock)
        .execute(&mut *locked)
        .await
        .unwrap();
    drop(locked);
    wait_for_parent_terminal(&fixture, "gateway-reserve-cancel", "cancelled").await;
    let summary = fixture.summary("gateway-reserve-cancel").await;
    assert_eq!(
        (
            summary.attempt_count,
            summary.prepared_attempts,
            summary.held_cost_units
        ),
        (1, 0, 0)
    );
    assert_eq!(fixture.balance().await, 0.20);
    assert_upstream_calls(&url, &count, 0).await;
    server.abort();
    fixture.close(state).await;
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL and performs actual local HTTP calls"]
async fn live_gateway_image_attempt_close_and_parent_write_retry_preserve_terminal() {
    for target in ["close", "parent"] {
        let fixture = Fixture::new(0.20).await;
        let state = fixture.state();
        let condition = if target == "close" {
            "NEW.funds_admission_closed_at IS NOT NULL AND OLD.funds_admission_closed_at IS NULL"
        } else {
            "NEW.status='completed' AND OLD.status<>'completed'"
        };
        // Sequence increments survive the injected transaction rollback, so only
        // the first matching write fails and the real drop retry can recover.
        sqlx::raw_sql(&format!(
            "CREATE SEQUENCE fail_once; CREATE FUNCTION fail_terminal_once() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF {condition} THEN IF nextval('fail_once')=1 THEN RAISE EXCEPTION 'injected terminal failure'; END IF; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_terminal_once BEFORE UPDATE ON usage FOR EACH ROW EXECUTE FUNCTION fail_terminal_once();"
        ))
        .execute(&fixture.pool)
        .await
        .unwrap();
        let (url, count, server) = upstream(vec![(200, image(6))]).await;
        assert!(execute(&state, "gateway-close-retry", "a", &url)
            .await
            .is_err());
        wait_for_parent_terminal(&fixture, "gateway-close-retry", "completed").await;
        let summary = fixture.summary("gateway-close-retry").await;
        assert_eq!(
            (summary.held_cost_units, summary.collected_cost_units),
            (0, 6_000_000)
        );
        assert_upstream_calls(&url, &count, 1).await;
        server.abort();
        fixture.close(state).await;
    }
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL and performs actual local HTTP calls"]
async fn live_gateway_image_attempt_changed_specs_require_reconciliation_without_overrun() {
    for (field, changed) in [("quality", "medium"), ("output_format", "jpeg")] {
        let fixture = Fixture::new(0.20).await;
        let state = fixture.state();
        let mut output = image(6);
        output[field] = json!(changed);
        let (url, count, server) = upstream(vec![(200, output)]).await;
        execute(&state, "gateway-changed-specs", "a", &url)
            .await
            .unwrap();
        let summary = fixture.summary("gateway-changed-specs").await;
        assert!(summary.requires_reconciliation);
        assert_eq!(
            (
                summary.known_actual_cost_units,
                summary.collected_cost_units,
                summary.held_cost_units
            ),
            (6_000_000, 6_000_000, 0)
        );
        let facts: Value = sqlx::query_scalar("SELECT terminal_facts FROM request_fund_reservations WHERE request_id='gateway-changed-specs'")
            .fetch_one(&fixture.pool).await.unwrap();
        assert_eq!(
            facts.pointer("/outcome/usage/requires_reconciliation"),
            Some(&json!(true))
        );
        assert_upstream_calls(&url, &count, 1).await;
        server.abort();
        fixture.close(state).await;
    }
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL and performs actual local HTTP calls"]
async fn live_gateway_image_heartbeat_keeps_admission_after_response_headers() {
    let fixture = Fixture::new(0.20).await;
    let state = fixture.state();
    let (mut locked, lock) = pause_reserve(&fixture, "p-b").await;
    let (url, count, server) = upstream(vec![
        (500, json!({"error":{"message":"unknown charge"}})),
        (200, image(6)),
    ])
    .await;
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        request_scope(&state, async {
            assert!(matches!(
                execute(&state, "gateway-heartbeat", "a", &url).await?,
                aether_ai_serving::AiAttemptExecutionOutcome::Retry { .. }
            ));
            crate::execution_runtime::sync::build_openai_image_sync_json_heartbeat_response(
                state.clone(),
                "/v1/images/generations".into(),
                plan("gateway-heartbeat", "b", &url),
                "gateway-heartbeat".into(),
                decision(),
                "openai_image_sync".into(),
                None,
                Some(context()),
            )
        }),
    )
    .await
    .expect("response headers must not wait for the background reserve")
    .unwrap();
    wait_for_blocked_reserve(&fixture, lock).await;
    assert!(!fixture.summary("gateway-heartbeat").await.admission_closed);
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(lock)
        .execute(&mut *locked)
        .await
        .unwrap();
    drop(locked);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 6);
    wait_for_parent_terminal(&fixture, "gateway-heartbeat", "completed").await;
    let summary = fixture.summary("gateway-heartbeat").await;
    assert_eq!(
        (
            summary.attempt_count,
            summary.held_cost_units,
            summary.collected_cost_units
        ),
        (2, 8_000_000, 6_000_000)
    );
    assert_upstream_calls(&url, &count, 2).await;
    server.abort();
    fixture.close(state).await;
}

fn memory_state() -> (
    AppState,
    Arc<aether_data::repository::usage::InMemoryUsageReadRepository>,
) {
    use aether_data::repository::{
        settlement::InMemorySettlementRepository,
        usage::InMemoryUsageReadRepository,
        wallet::{InMemoryWalletRepository, StoredWalletSnapshot},
    };
    let usage = Arc::new(InMemoryUsageReadRepository::default());
    let wallet = StoredWalletSnapshot::new(
        "wallet".into(),
        Some("owner".into()),
        None,
        0.20,
        0.0,
        "finite".into(),
        "USD".into(),
        "active".into(),
        0.0,
        0.0,
        0.0,
        0.0,
        100,
    )
    .unwrap();
    let wallets = Arc::new(InMemoryWalletRepository::seed([wallet]));
    let settlement = Arc::new(
        InMemorySettlementRepository::from_wallet_repository(wallets.clone())
            .with_usage_repository(usage.clone()),
    );
    let billing = Arc::new(InMemoryBillingReadRepository::seed([
        pricing("a"),
        pricing("b"),
    ]));
    let state = AppState::new()
        .unwrap()
        .with_data_state_for_tests(
            GatewayDataState::with_usage_billing_and_wallet_for_tests(
                usage.clone(),
                billing,
                wallets,
            )
            .with_settlement_writer_for_tests(settlement),
        )
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: false,
            ..UsageRuntimeConfig::default()
        });
    (state, usage)
}

#[tokio::test]
async fn memory_gateway_image_attempt_uses_shared_parent_usage_repository() {
    use aether_data_contracts::repository::usage::UsageReadRepository;
    let (state, usage) = memory_state();
    let (url, count, server) = upstream(vec![(200, image(6))]).await;
    execute(&state, "gateway-memory", "a", &url).await.unwrap();
    let stored = usage
        .find_by_request_id("gateway-memory")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.actual_total_cost_usd, 0.06);
    assert_eq!(stored.status, "completed");
    assert_eq!(stored.billing_status, "settled");
    assert_upstream_calls(&url, &count, 1).await;
    state
        .usage_runtime
        .shutdown(Duration::from_secs(10))
        .await
        .unwrap();
    server.abort();
}

#[tokio::test]
async fn paid_image_rejects_legacy_hard_quota_before_funds_admission() {
    use aether_data_contracts::repository::usage::UsageReadRepository;
    let (state, usage) = memory_state();
    let (url, count, server) = upstream(vec![(200, image(6))]).await;
    let mut context = context();
    context["plan_usage_reservation_token"] = json!("server-quota-token");
    let result = request_scope(&state, async {
        FundedImageAttempt::prepare(
            &state,
            &plan("gateway-quota", "a", &url),
            &decision(),
            Some(&context),
        )
        .await
    })
    .await;
    assert!(result.is_err());
    assert!(usage
        .find_by_request_id("gateway-quota")
        .await
        .unwrap()
        .is_none());
    assert_upstream_calls(&url, &count, 0).await;
    state
        .usage_runtime
        .shutdown(Duration::from_secs(10))
        .await
        .unwrap();
    server.abort();
}

#[tokio::test]
async fn production_image_heartbeat_executes_funded_public_attempt() {
    use crate::ai_serving::api::AiSyncAttempt;
    use crate::executor::ProviderTransferTracker;
    let (state, _) = memory_state();
    let (url, count, server) = upstream(vec![(200, image(6))]).await;
    let response = request_scope(&state, async {
        crate::executor::build_openai_image_sync_heartbeat_shell_response(
            state.clone(),
            "/v1/images/generations".into(),
            "gateway-public-gate".into(),
            decision(),
            "openai_image_sync".into(),
            vec![AiSyncAttempt {
                plan: plan("gateway-public-gate", "a", &url),
                report_kind: None,
                report_context: Some(context()),
            }],
            ProviderTransferTracker::default(),
        )
    })
    .await
    .unwrap();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["data"].as_array().map(Vec::len), Some(6), "{body}");
    state
        .usage_runtime
        .shutdown(Duration::from_secs(10))
        .await
        .unwrap();
    assert_upstream_calls(&url, &count, 1).await;
    server.abort();
}
