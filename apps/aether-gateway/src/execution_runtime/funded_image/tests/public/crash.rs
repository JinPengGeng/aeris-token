//! Real process death, not cancellation/Drop recovery. The child hosts the public
//! Gateway router with SQL financial repositories; only the parent owns the schema.
use super::*;
use aether_data_contracts::repository::settlement::SettlementWriteRepository;
use aether_data_contracts::repository::usage::{
    UsageCleanupExecutionMode, UsageCleanupTargets, UsageCleanupWindow,
};
use futures_util::FutureExt;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdin, Command, Stdio};

const REQUEST: &str = "public-process-crash";
const CHILD_TARGET: &str =
    "execution_runtime::funded_image::tests::public_tests::crash_tests::image_process_crash_child";
const CHILD_ROLE: &str = "funded-image-kill-restart-v1";
const MARKER: &str = "AETHER_IMAGE_CRASH_EVIDENCE ";

fn emit(value: Value) {
    // Deliberately whitelist evidence at call sites: no token, quote contents,
    // credentials, upstream receipt body, or raw child output is forwarded.
    println!("{MARKER}{value}");
    std::io::stdout().flush().unwrap();
}

fn loopback_database_url() -> String {
    let value = std::env::var("AETHER_TEST_DATABASE_URL")
        .expect("requires disposable loopback AETHER_TEST_DATABASE_URL");
    let parsed = url::Url::parse(&value).expect("invalid test database URL");
    assert!(matches!(parsed.scheme(), "postgres" | "postgresql"));
    assert!(matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    value
}

struct GatewayProcess {
    child: Child,
    input: ChildStdin,
    messages: tokio::sync::mpsc::UnboundedReceiver<Value>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl GatewayProcess {
    fn spawn(fixture: &Fixture, upstream: &str) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                CHILD_TARGET,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("AETHER_IMAGE_CRASH_CHILD_ROLE", CHILD_ROLE)
            .env("AETHER_IMAGE_CRASH_CHILD_SCHEMA", &fixture.schema)
            .env(
                "AETHER_IMAGE_CRASH_CHILD_NONCE",
                fixture.schema.strip_prefix("gateway_attempt_").unwrap(),
            )
            .env(
                "AETHER_IMAGE_CRASH_PARENT_PID",
                std::process::id().to_string(),
            )
            .env("AETHER_IMAGE_CRASH_UPSTREAM", upstream)
            .env("AETHER_TEST_DATABASE_URL", loopback_database_url())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn real Gateway test process");
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (sender, messages) = tokio::sync::mpsc::unbounded_channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                // libtest can prefix its test name on the first output line.
                if let Some((_, payload)) = line.split_once(MARKER) {
                    if let Ok(value) = serde_json::from_str::<Value>(payload) {
                        if sender.send(value).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            child,
            input,
            messages,
            reader: Some(reader),
        }
    }

    fn send(&mut self, command: Value) {
        serde_json::to_writer(&mut self.input, &command).expect("write child command");
        self.input.write_all(b"\n").unwrap();
        self.input.flush().unwrap();
    }

    async fn receive(&mut self, operation: &str) -> Value {
        let message = tokio::time::timeout(Duration::from_secs(30), self.messages.recv())
            .await
            .expect("child response deadline exceeded")
            .expect("child exited before its safe response");
        assert_eq!(
            message["op"].as_str(),
            Some(operation),
            "safe child status: {message}"
        );
        message
    }

    fn kill_and_wait(&mut self) -> std::process::ExitStatus {
        self.child.kill().expect("SIGKILL child Gateway");
        let status = self.child.wait().expect("reap killed Gateway");
        assert_eq!(status.signal(), Some(9), "must exercise real SIGKILL");
        status
    }

    async fn stop(&mut self) {
        self.send(json!({"op":"stop"}));
        self.receive("stopped").await;
        let status = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    break status;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("child exit deadline exceeded");
        assert!(status.success());
    }
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        // Also runs during parent assertion failure/unwind. Reap before joining
        // the pipe reader so an idle child cannot strand the test process.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

struct AbortTask<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortTask<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn public_request_completion(gateway: String) -> Value {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(25))
        .build()
        .unwrap();
    let response = match client
        .post(format!("{gateway}/v1/images/generations"))
        .bearer_auth("sk-public-image-fixture")
        .header(crate::constants::TRACE_ID_HEADER, REQUEST)
        .json(&request_body())
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return json!({"phase":"send_failed","timeout":error.is_timeout(),"connect":error.is_connect()});
        }
    };
    let status = response.status().as_u16();
    // Public image responses may send heartbeat headers before execution ends.
    // Keep consuming the body so task completion means the request really ended.
    match response.bytes().await {
        Ok(bytes) => {
            let body = serde_json::from_slice::<Value>(&bytes).ok();
            let error = body.as_ref().and_then(|body| body.get("error"));
            let error_kind = error
                .and_then(|error| error.get("type"))
                .and_then(Value::as_str)
                .map(|kind| match kind {
                    "authentication_error"
                    | "invalid_api_key"
                    | "permission_error"
                    | "invalid_request_error"
                    | "plan_usage_limit_exceeded"
                    | "daily_usage_limit_exceeded"
                    | "insufficient_balance"
                    | "upstream_error"
                    | "api_error"
                    | "internal_error" => kind,
                    _ => "other",
                });
            // Never return response text, error messages, headers, or credentials.
            json!({"phase":"response_complete","status":status,"body_bytes":bytes.len(),
                "json":body.is_some(),"error":error.is_some(),"error_kind":error_kind})
        }
        Err(error) => {
            json!({"phase":"body_failed","status":status,"timeout":error.is_timeout(),"body":error.is_body()})
        }
    }
}

async fn audit(fixture: &Fixture) -> Value {
    let (attempt, state, dispatched, terminal, quote): (String, String, bool, bool, Value) =
        sqlx::query_as("SELECT attempt_id::text,state,dispatched_at IS NOT NULL,terminal_facts IS NOT NULL,quote FROM request_fund_reservations WHERE request_id=$1")
            .bind(REQUEST).fetch_one(&fixture.pool).await.unwrap();
    let parent: (String, String, bool) = sqlx::query_as(
        "SELECT status,billing_status,funds_admission_closed_at IS NOT NULL FROM usage WHERE request_id=$1")
        .bind(REQUEST).fetch_one(&fixture.pool).await.unwrap();
    let balances: (i64, i64) = sqlx::query_as(
        "SELECT ROUND(balance*100000000)::bigint,ROUND(total_consumed*100000000)::bigint FROM wallets WHERE id='wallet'")
        .fetch_one(&fixture.pool).await.unwrap();
    let provider: (i64, i64, i64) = sqlx::query_as(
        "SELECT COALESCE(request_count,0)::bigint,COALESCE(error_count,0)::bigint,ROUND(COALESCE(total_cost_usd,0)*100000000)::bigint FROM provider_api_keys WHERE id='pk-a'")
        .fetch_one(&fixture.pool).await.unwrap();
    let key_requests: i64 = sqlx::query_scalar(
        "SELECT COALESCE(total_requests,0)::bigint FROM api_keys WHERE id='key-a'",
    )
    .fetch_one(&fixture.pool)
    .await
    .unwrap();
    let daily: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(actual_cost_units),0)::bigint FROM usage_daily_cost_contributions WHERE request_id=$1")
        .bind(REQUEST).fetch_one(&fixture.pool).await.unwrap();
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM usage WHERE request_id=$1),(SELECT COUNT(*) FROM request_fund_reservations WHERE request_id=$1),(SELECT COUNT(*) FROM usage_counter_deltas WHERE request_id=$1)")
        .bind(REQUEST).fetch_one(&fixture.pool).await.unwrap();
    let allocations: (i64, i64, i64) = sqlx::query_as("SELECT COUNT(*),COALESCE(SUM(a.reserved_cost_units),0)::bigint,COALESCE(SUM(a.collected_cost_units),0)::bigint FROM request_fund_allocations a JOIN request_fund_reservations r USING(reservation_token) WHERE r.request_id=$1")
        .bind(REQUEST).fetch_one(&fixture.pool).await.unwrap();
    json!({
        "request_id":REQUEST,"attempt_id":attempt,"state":state,
        "dispatched":dispatched,"terminal_facts_present":terminal,
        "quote_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&quote).unwrap())),
        "summary":fixture.summary(REQUEST).await,
        "wallet_balance_units":balances.0,"wallet_consumed_units":balances.1,
        "parent_status":parent.0,"billing_status":parent.1,"admission_closed":parent.2,
        "provider_requests":provider.0,"provider_errors":provider.1,"provider_cost_units":provider.2,
        "api_key_requests":key_requests,"daily_cost_units":daily,
        "parent_count":counts.0,"attempt_count":counts.1,"outbox_rows":counts.2,
        "quota":fixture.quota_totals().await,"allocations":allocations,
    })
}

fn assert_held(value: &Value) {
    assert!(matches!(
        value["state"].as_str(),
        Some("dispatched" | "reconciliation_pending")
    ));
    assert_eq!(value["dispatched"], true);
    assert_eq!(value["summary"]["held_cost_units"], 8_000_000);
    assert_eq!(value["summary"]["unknown_attempts"], 1);
    assert_eq!(value["summary"]["collected_cost_units"], 0);
    assert_eq!(value["wallet_balance_units"], 20_000_000);
    assert_eq!(value["parent_count"], 1);
    assert_eq!(value["attempt_count"], 1);
    assert_eq!(value["quota"], json!([1, 8_000_000, 0]));
    assert_eq!(value["allocations"], json!([1, 8_000_000, 0]));
}

fn assert_settled(value: &Value) {
    assert_eq!(value["summary"]["held_cost_units"], 0);
    assert_eq!(value["summary"]["unknown_attempts"], 0);
    assert_eq!(value["summary"]["collected_cost_units"], 7_000_000);
    assert_eq!(value["summary"]["known_actual_cost_units"], 7_000_000);
    assert_eq!(value["wallet_balance_units"], 13_000_000);
    assert_eq!(value["wallet_consumed_units"], 7_000_000);
    assert_eq!(value["parent_status"], "failed");
    assert_eq!(value["billing_status"], "settled");
    assert_eq!(value["admission_closed"], true);
    assert_eq!(value["provider_requests"], 1);
    assert_eq!(value["provider_errors"], 1);
    assert_eq!(value["provider_cost_units"], 7_000_000);
    assert_eq!(value["api_key_requests"], 1);
    assert_eq!(value["daily_cost_units"], 7_000_000);
    assert_eq!(value["parent_count"], 1);
    assert_eq!(value["attempt_count"], 1);
    assert_eq!(value["quota"], json!([1, 0, 7_000_000]));
    assert_eq!(value["allocations"], json!([1, 8_000_000, 7_000_000]));
}

/// Never add this target to the CI inventory. Only its parent supplies all of
/// the per-schema capability markers and owns process/schema cleanup.
#[tokio::test]
#[ignore = "protected child entrypoint; run only through the process-crash parent"]
async fn image_process_crash_child() {
    assert_eq!(
        std::env::var("AETHER_IMAGE_CRASH_CHILD_ROLE")
            .ok()
            .as_deref(),
        Some(CHILD_ROLE)
    );
    std::panic::set_hook(Box::new(|info| {
        // Child panic payloads can contain repository values. Report location only.
        emit(json!({"op":"child_failed","line":info.location().map(|location|location.line())}));
    }));
    let nonce = std::env::var("AETHER_IMAGE_CRASH_CHILD_NONCE").expect("missing child nonce");
    uuid::Uuid::parse_str(&nonce).expect("invalid child nonce");
    let schema = std::env::var("AETHER_IMAGE_CRASH_CHILD_SCHEMA").expect("missing child schema");
    assert_eq!(schema, format!("gateway_attempt_{nonce}"));
    let parent: u32 = std::env::var("AETHER_IMAGE_CRASH_PARENT_PID")
        .unwrap()
        .parse()
        .unwrap();
    assert!(parent > 1 && parent != std::process::id());
    let upstream = std::env::var("AETHER_IMAGE_CRASH_UPSTREAM").expect("missing child upstream");
    let upstream_url = url::Url::parse(&upstream).unwrap();
    assert_eq!(upstream_url.scheme(), "http");
    assert_eq!(upstream_url.host_str(), Some("127.0.0.1"));
    let database = loopback_database_url();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database)
        .await
        .unwrap();
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=$1)")
            .bind(&schema)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(
        exists,
        "child only attaches its parent's existing isolated schema"
    );
    let options = database
        .parse::<sqlx::postgres::PgConnectOptions>()
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let fixture = Fixture {
        admin,
        pool,
        schema,
    };
    let state = fixture.public_state(Account::User, &upstream).await;
    let (gateway, server) = public_server(state.clone()).await;
    let _server = AbortTask(server);
    let (sender, mut commands) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            let command: Value = serde_json::from_str(&line).expect("invalid parent command");
            if sender.send(command).is_err() {
                break;
            }
        }
    });
    emit(json!({"op":"ready","pid":std::process::id(),"url":gateway}));
    let usage = SqlxUsageReadRepository::new(fixture.pool.clone());
    let mut recovery: Option<(UsageEvent, UsageEvent)> = None;
    let mut receipt: Option<UsageEvent> = None;
    while let Some(command) = commands.recv().await {
        match command["op"].as_str().expect("child operation required") {
            "inspect" => emit(json!({"op":"inspect","audit":audit(&fixture).await})),
            "retain" => {
                let future = chrono::Utc::now() + chrono::Duration::days(4000);
                let stale = usage
                    .cleanup_stale_pending_requests(
                        future.timestamp() as u64,
                        future.timestamp() as u64,
                        1,
                        100,
                    )
                    .await
                    .unwrap();
                assert_eq!((stale.failed, stale.recovered), (0, 0));
                let result = usage
                    .cleanup_usage(
                        &UsageCleanupWindow {
                            detail_cutoff: future,
                            compressed_cutoff: future,
                            header_cutoff: future,
                            log_cutoff: future,
                        },
                        100,
                        false,
                        UsageCleanupTargets {
                            detail_body: false,
                            compressed_body: false,
                            headers: false,
                            records: true,
                            expired_keys: false,
                        },
                        UsageCleanupExecutionMode::Policy,
                    )
                    .await
                    .unwrap();
                assert_eq!(result.records_deleted, 0);
                let deleted = SqlxSettlementRepository::new(fixture.pool.clone())
                    .cleanup_usage_policy_cost_reservations(future.timestamp() as u64, 100)
                    .await
                    .unwrap();
                assert_eq!(deleted, 0);
                emit(
                    json!({"op":"retain","audit":audit(&fixture).await,"records_deleted":result.records_deleted,"quota_rows_deleted":deleted}),
                );
            }
            "recover" => {
                assert!(recovery.is_none());
                let identity = fixture.identity(REQUEST, "p-a").await;
                state
                    .data
                    .close_request_attempt_admission(identity.clone())
                    .await
                    .unwrap();
                let execution = RequestAttemptExecutionFacts {
                    // The supervisor witnessed local process death before delivery.
                    // This is separate from the provider's later charge evidence.
                    status: RequestAttemptExecutionStatus::Failed,
                    response_time_ms: command["elapsed_ms"].as_u64().unwrap(),
                };
                let mut parent = UsageEvent::new(
                    UsageEventType::Failed,
                    REQUEST,
                    UsageEventData {
                        user_id: Some("owner".into()),
                        api_key_id: Some("key-a".into()),
                        provider_name: "images".into(),
                        provider_id: Some("p-a".into()),
                        provider_api_key_id: Some("pk-a".into()),
                        model: "image-model".into(),
                        api_format: Some("openai:image".into()),
                        status_code: Some(500),
                        error_category: Some("process_crash".into()),
                        response_time_ms: Some(execution.response_time_ms),
                        attempt_funds: Some(Box::new(UsageAttemptFundsEvent {
                            schema_version: 1,
                            identity: identity.clone(),
                            action: UsageAttemptFundsAction::ParentLifecycle,
                        })),
                        ..UsageEventData::default()
                    },
                );
                parent.timestamp_ms = command["observed_at_ms"].as_u64().unwrap();
                let mut unknown = UsageEvent::new(
                    UsageEventType::Failed,
                    REQUEST,
                    UsageEventData {
                        attempt_funds: Some(Box::new(UsageAttemptFundsEvent {
                            schema_version: 1,
                            identity,
                            action: UsageAttemptFundsAction::Outcome {
                                execution,
                                evidence: UsageAttemptChargeEvidence::Unknown,
                            },
                        })),
                        ..UsageEventData::default()
                    },
                );
                unknown.timestamp_ms = parent.timestamp_ms;
                for event in [&parent, &unknown] {
                    state
                        .usage_runtime
                        .persist_attempt_funds_event(
                            state.usage_lifecycle_data_state().as_ref(),
                            event.clone(),
                        )
                        .await
                        .unwrap();
                }
                recovery = Some((parent, unknown));
                emit(json!({"op":"recover","audit":audit(&fixture).await}));
            }
            "settle" => {
                assert!(receipt.is_none());
                let mut event = recovery.as_ref().unwrap().1.clone();
                let UsageAttemptFundsAction::Outcome { evidence, .. } =
                    &mut event.data.attempt_funds.as_mut().unwrap().action
                else {
                    unreachable!()
                };
                *evidence = image_output_evidence(Some(&command["receipt"]));
                assert!(matches!(
                    evidence,
                    UsageAttemptChargeEvidence::ImageOutput { .. }
                ));
                state
                    .usage_runtime
                    .persist_attempt_funds_event(
                        state.usage_lifecycle_data_state().as_ref(),
                        event.clone(),
                    )
                    .await
                    .unwrap();
                usage.flush_usage_counter_deltas(1000).await.unwrap();
                receipt = Some(event);
                emit(json!({"op":"settle","audit":audit(&fixture).await}));
            }
            "cleanup" => {
                usage.flush_usage_counter_deltas(1000).await.unwrap();
                let removed = usage
                    .cleanup_processed_usage_counter_deltas(
                        crate::clock::current_unix_ms() / 1000 + 3600,
                        1000,
                    )
                    .await
                    .unwrap();
                assert!(removed > 0);
                emit(json!({"op":"cleanup","removed":removed,"audit":audit(&fixture).await}));
            }
            "replay" => {
                let (parent, unknown) = recovery.as_ref().unwrap();
                for event in [
                    receipt.as_ref().unwrap(),
                    parent,
                    unknown,
                    receipt.as_ref().unwrap(),
                ] {
                    state
                        .usage_runtime
                        .persist_attempt_funds_event(
                            state.usage_lifecycle_data_state().as_ref(),
                            event.clone(),
                        )
                        .await
                        .unwrap();
                }
                usage.flush_usage_counter_deltas(1000).await.unwrap();
                emit(json!({"op":"replay","audit":audit(&fixture).await}));
            }
            "stop" => {
                state
                    .usage_runtime
                    .shutdown(Duration::from_secs(10))
                    .await
                    .unwrap();
                fixture.pool.close().await;
                fixture.admin.close().await;
                emit(json!({"op":"stopped"}));
                return;
            }
            _ => panic!("unsupported child operation"),
        }
    }
    panic!("parent command pipe closed without orderly stop");
}

#[tokio::test]
#[ignore = "requires disposable loopback PostgreSQL; SIGKILLs only its own protected Gateway child"]
async fn live_public_image_process_kill_restart_unknown_hold_late_receipt_and_replay() {
    loopback_database_url();
    let fixture = Fixture::new(0.20).await;
    let result=std::panic::AssertUnwindSafe(async {
        fixture.hard_cost_policy(0.20).await;
        let calls=Arc::new(AtomicUsize::new(0));
        let seen=Arc::new(tokio::sync::Notify::new());
        let receipt=Arc::new(std::sync::Mutex::new(None::<Value>));
        let (counter,notifier,captured)=(calls.clone(),seen.clone(),receipt.clone());
        let upstream=axum::Router::new().route("/v1/images/generations",axum::routing::post(move |axum::Json(body):axum::Json<Value>| {
            let (counter,notifier,captured)=(counter.clone(),notifier.clone(),captured.clone());
            async move {
                assert_eq!(body["n"],8);
                counter.fetch_add(1,Ordering::SeqCst);
                *captured.lock().unwrap()=Some(image(7));
                notifier.notify_one();
                // Full HTTP request reached the independent upstream. No response
                // is delivered before the Gateway process is killed.
                std::future::pending::<axum::Json<Value>>().await
            }
        }));
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        // public_state accepts an endpoint API root. Passing the complete operation
        // URL makes that helper strip /v1 as well, sending /images/generations to
        // this strict router and returning 404 before the dispatch notification.
        let upstream_url=format!("http://{}/v1",listener.local_addr().unwrap());
        let _upstream=AbortTask(tokio::spawn(async move { axum::serve(listener,upstream).await.unwrap(); }));
        let mut first=GatewayProcess::spawn(&fixture,&upstream_url);
        let ready=first.receive("ready").await;
        let first_pid=first.child.id();
        assert_eq!(ready["pid"],first_pid);
        let gateway=ready["url"].as_str().unwrap().to_string();
        let started=std::time::Instant::now();
        let mut request=AbortTask(tokio::spawn(public_request_completion(gateway)));
        tokio::select! {
            _ = seen.notified() => (),
            completed = &mut request.0 => {
                let safe_status = match completed {
                    Ok(value) => value,
                    Err(error) => json!({"phase":"request_task_failed","panic":error.is_panic(),"cancelled":error.is_cancelled()}),
                };
                emit(json!({"op":"request_ended_before_dispatch","status":safe_status,"upstream_calls":calls.load(Ordering::SeqCst)}));
                panic!("public request ended before upstream dispatch: {safe_status}");
            },
            _ = tokio::time::sleep(Duration::from_secs(15)) => {
                emit(json!({"op":"dispatch_deadline","request_finished":request.0.is_finished(),"upstream_calls":calls.load(Ordering::SeqCst)}));
                panic!("real upstream dispatch was not observed");
            },
        }
        assert_eq!(calls.load(Ordering::SeqCst),1);
        let before=audit(&fixture).await;
        assert_held(&before);
        assert_eq!(before["terminal_facts_present"],false);
        assert!(!request.0.is_finished(),"Gateway must still be awaiting upstream output");
        let killed=first.kill_and_wait();
        let elapsed_ms=started.elapsed().as_millis() as u64;
        let observed_at_ms=crate::clock::current_unix_ms();
        let after_kill=audit(&fixture).await;
        assert_held(&after_kill);
        assert_eq!(after_kill["terminal_facts_present"],false,"SIGKILL must not run Drop recovery");
        assert_eq!(after_kill["admission_closed"],false);
        assert_eq!(after_kill["quote_sha256"],before["quote_sha256"]);
        emit(json!({"op":"killed","pid":first_pid,"signal":killed.signal(),"upstream_calls":1,"audit":after_kill}));
        drop(first);

        let mut restarted=GatewayProcess::spawn(&fixture,&upstream_url);
        let ready=restarted.receive("ready").await;
        let second_pid=restarted.child.id();
        assert_ne!(first_pid,second_pid);
        assert_eq!(ready["pid"],second_pid);
        for operation in ["inspect","retain"] {
            restarted.send(json!({"op":operation}));
            let message=restarted.receive(operation).await;
            assert_held(&message["audit"]);
            assert_eq!(message["audit"]["quote_sha256"],before["quote_sha256"]);
            assert_eq!(message["audit"]["terminal_facts_present"],false);
            assert_eq!(calls.load(Ordering::SeqCst),1,"restart must not redispatch");
            emit(json!({"op":operation,"pid":second_pid,"upstream_calls":1,"audit":message["audit"]}));
        }
        restarted.send(json!({"op":"recover","elapsed_ms":elapsed_ms,"observed_at_ms":observed_at_ms}));
        let recovered=restarted.receive("recover").await;
        assert_held(&recovered["audit"]);
        assert_eq!(recovered["audit"]["terminal_facts_present"],true);
        assert_eq!(recovered["audit"]["admission_closed"],true);
        emit(recovered);
        let receipt=receipt.lock().unwrap().clone().expect("upstream captured a fixture receipt");
        restarted.send(json!({"op":"settle","receipt":receipt}));
        let settled=restarted.receive("settle").await;
        assert_settled(&settled["audit"]);
        assert_eq!(settled["audit"]["quote_sha256"],before["quote_sha256"]);
        emit(settled);
        restarted.send(json!({"op":"cleanup"}));
        let cleaned=restarted.receive("cleanup").await;
        assert_settled(&cleaned["audit"]);
        assert_eq!(cleaned["audit"]["outbox_rows"],0);
        emit(cleaned.clone());
        restarted.send(json!({"op":"replay"}));
        let replayed=restarted.receive("replay").await;
        assert_settled(&replayed["audit"]);
        assert_eq!(replayed["audit"],cleaned["audit"],"receipt, parent, and late Unknown replay must not duplicate contributions");
        assert_eq!(calls.load(Ordering::SeqCst),1);
        emit(json!({"op":"verified","first_pid":first_pid,"restart_pid":second_pid,"kill_signal":9,"upstream_calls":1,"audit":replayed["audit"]}));
        restarted.stop().await;
    }).catch_unwind().await;
    // Guard drops above have killed/reaped children before database cleanup.
    fixture.pool.close().await;
    let cleanup = tokio::time::timeout(
        Duration::from_secs(15),
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", fixture.schema)).execute(&fixture.admin),
    )
    .await;
    fixture.admin.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    cleanup
        .expect("isolated schema cleanup deadline")
        .expect("isolated schema cleanup");
}
