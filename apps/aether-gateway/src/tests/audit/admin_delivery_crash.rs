//! SIGKILL at real PostgreSQL barriers around the authenticated HTTP mutation.
//! Database-side cleanup of a dead client's blocked backend is explicit: a
//! disconnected client alone does not cancel SQL already running in PostgreSQL.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::{
    fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    process::ExitStatusExt,
};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Duration;

use futures_util::FutureExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPoolOptions, PgPool};

use crate::data::{GatewayDataConfig, GatewayDataState};
use crate::tests::{
    authenticated_operational_client_with_builder, build_router_with_state, start_server, AppState,
    OPERATIONAL_ADMIN_DEVICE_ID,
};

const CHILD_TARGET: &str = "tests::audit::admin_delivery_crash::admin_audit_delivery_process_child";
const CHILD_ROLE: &str = "admin-audit-sigkill-fixture-v1";
const MARKER: &str = "AETHER_ADMIN_AUDIT_CRASH_EVIDENCE ";
const CONFIG_KEY: &str = "enable_format_conversion";
const PRECOMMIT_LOCK: i32 = 255931;
const AUDIT_LOCK: i32 = 255932;

fn emit(value: Value) {
    // Only phase names, PIDs, signal numbers, event IDs and bounded counts.
    // Never forward raw child output, SQL errors, auth tokens or database URLs.
    println!("{MARKER}{value}");
    std::io::stdout().flush().unwrap();
}

fn database_url() -> String {
    let value = std::env::var("AETHER_TEST_AUDIT_DATABASE_URL")
        .expect("requires an explicit fresh task-owned administrator audit database");
    let parsed = url::Url::parse(&value).expect("invalid audit fixture URL");
    assert!(matches!(parsed.scheme(), "postgres" | "postgresql"));
    assert!(matches!(
        parsed.host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let name = parsed.path().trim_start_matches('/');
    assert!(name.starts_with("aether_admin_audit_"));
    assert!(
        name.len() <= 63
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
    value
}

fn gateway_state(database_url: String) -> AppState {
    AppState::new()
        .unwrap()
        .without_auth_user_store_for_tests()
        .without_auth_session_store_for_tests()
        .with_data_state_for_tests(
            GatewayDataState::from_config(GatewayDataConfig::from_postgres_url(
                database_url,
                false,
            ))
            .unwrap(),
        )
}

struct PrivateDirectory(PathBuf);
impl PrivateDirectory {
    fn new(nonce: &str) -> Self {
        let base = std::env::var_os("AGENT_TMP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&base).unwrap();
        let path = base.join(format!("aether-admin-audit-crash-{nonce}"));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
    fn write_token(&self, token: &str) {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.0.join("access-token"))
            .unwrap();
        file.write_all(token.as_bytes()).unwrap();
        file.sync_all().unwrap();
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Database {
    pool: PgPool,
    admin: PgPool,
    name: String,
}
impl Database {
    async fn attach_empty() -> Self {
        let url = database_url();
        let options = url.parse::<sqlx::postgres::PgConnectOptions>().unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(options.clone().options([("statement_timeout", "5000")]))
            .await
            .unwrap();
        let name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            name,
            url::Url::parse(&url)
                .unwrap()
                .path()
                .trim_start_matches('/')
        );
        let tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg_temp%' AND c.relkind IN ('r','p','v','m','S','f')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            tables, 0,
            "never migrate, clear or drop a nonempty caller database"
        );
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(
                options
                    .database("postgres")
                    .options([("statement_timeout", "10000")]),
            )
            .await
            .unwrap();
        Self { pool, admin, name }
    }
    async fn close(self) {
        tokio::time::timeout(Duration::from_secs(10), self.pool.close())
            .await
            .expect("parent pool cleanup deadline");
        // This exact prefixed database was empty before this fixture migrated it.
        tokio::time::timeout(
            Duration::from_secs(15),
            sqlx::query(&format!("DROP DATABASE {} WITH (FORCE)", self.name)).execute(&self.admin),
        )
        .await
        .expect("owned database cleanup deadline")
        .unwrap();
        self.admin.close().await;
    }
}

struct GatewayProcess {
    child: Child,
    input: ChildStdin,
    messages: tokio::sync::mpsc::UnboundedReceiver<Value>,
    reader: Option<std::thread::JoinHandle<()>>,
    application_name: String,
}
impl GatewayProcess {
    fn spawn(directory: &Path, nonce: &str, phase: &str) -> Self {
        assert!(matches!(
            phase,
            "precommit" | "postcommit" | "claimed" | "recovery"
        ));
        let application_name = format!("audit-crash-{nonce}-{phase}");
        let mut parsed = url::Url::parse(&database_url()).unwrap();
        parsed
            .query_pairs_mut()
            .append_pair("application_name", &application_name);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                CHILD_TARGET,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("AETHER_AUDIT_CRASH_CHILD_ROLE", CHILD_ROLE)
            .env("AETHER_AUDIT_CRASH_CHILD_NONCE", nonce)
            .env("AETHER_AUDIT_CRASH_CHILD_DIRECTORY", directory)
            .env(
                "AETHER_AUDIT_CRASH_PARENT_PID",
                std::process::id().to_string(),
            )
            .env("AETHER_AUDIT_CRASH_PHASE", phase)
            .env("AETHER_TEST_AUDIT_DATABASE_URL", parsed.as_str())
            .env("RUST_MIN_STACK", "16777216")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn protected administrator audit Gateway child");
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (sender, messages) = tokio::sync::mpsc::unbounded_channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
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
            application_name,
        }
    }
    fn send(&mut self, value: Value) {
        serde_json::to_writer(&mut self.input, &value).unwrap();
        self.input.write_all(b"\n").unwrap();
        self.input.flush().unwrap();
    }
    async fn receive(&mut self, op: &str) -> Value {
        let value = tokio::time::timeout(Duration::from_secs(20), self.messages.recv())
            .await
            .expect("bounded child response deadline")
            .expect("child exited before safe evidence");
        assert_eq!(value["op"].as_str(), Some(op), "safe child status: {value}");
        value
    }
    async fn ready(&mut self) -> u32 {
        let value = self.receive("ready").await;
        let pid = self.child.id();
        assert_eq!(value["pid"], pid);
        pid
    }
    async fn kill_and_wait(&mut self) {
        self.child.kill().expect("SIGKILL only this fixture child");
        let status = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    break status;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("SIGKILL child reap deadline");
        assert_eq!(status.signal(), Some(9), "must prove real process death");
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
        .expect("clean child exit deadline");
        assert!(status.success());
    }
}
impl Drop for GatewayProcess {
    fn drop(&mut self) {
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

#[tokio::test]
#[ignore = "protected exact child entry; execute only through live_admin_audit_process_kill_restart"]
async fn admin_audit_delivery_process_child() {
    std::panic::set_hook(Box::new(|info| {
        emit(json!({"op":"child_failed","line":info.location().map(|location|location.line())}));
    }));
    assert_eq!(
        std::env::var("AETHER_AUDIT_CRASH_CHILD_ROLE")
            .ok()
            .as_deref(),
        Some(CHILD_ROLE)
    );
    let nonce = std::env::var("AETHER_AUDIT_CRASH_CHILD_NONCE").unwrap();
    uuid::Uuid::parse_str(&nonce).unwrap();
    let parent: u32 = std::env::var("AETHER_AUDIT_CRASH_PARENT_PID")
        .unwrap()
        .parse()
        .unwrap();
    assert!(parent > 1 && parent != std::process::id());
    let directory = PathBuf::from(std::env::var_os("AETHER_AUDIT_CRASH_CHILD_DIRECTORY").unwrap());
    assert_eq!(
        directory.file_name().unwrap().to_string_lossy(),
        format!("aether-admin-audit-crash-{nonce}")
    );
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o077,
        0
    );
    let token_path = directory.join("access-token");
    assert_eq!(
        std::fs::metadata(&token_path).unwrap().permissions().mode() & 0o077,
        0
    );
    let token = std::fs::read_to_string(token_path).unwrap();
    let phase = std::env::var("AETHER_AUDIT_CRASH_PHASE").unwrap();
    assert!(matches!(
        phase.as_str(),
        "precommit" | "postcommit" | "claimed" | "recovery"
    ));
    let state = gateway_state(database_url());
    assert!(state.data.has_admin_audit_delivery_backend());
    let (gateway, server) = start_server(build_router_with_state(state.clone())).await;
    let server = AbortTask(server);
    let (sender, mut commands) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            let command: Value = serde_json::from_str(&line).expect("invalid parent protocol");
            if sender.send(command).is_err() {
                break;
            }
        }
    });
    let mut request = None;
    let mut worker = None;
    emit(json!({"op":"ready","pid":std::process::id()}));
    // Prevent a detached exact child from living indefinitely if its parent dies.
    let result = tokio::time::timeout(Duration::from_secs(140), async {
        while let Some(command) = commands.recv().await {
            match command["op"].as_str().unwrap() {
                "put" => {
                    assert!(matches!(phase.as_str(), "precommit" | "postcommit"));
                    assert!(
                        request.is_none(),
                        "each crash child executes exactly one real PUT"
                    );
                    let endpoint = format!("{gateway}/api/admin/system/configs/{CONFIG_KEY}");
                    let client = authenticated_operational_client_with_builder(
                        reqwest::Client::builder()
                            .no_proxy()
                            .timeout(Duration::from_secs(30)),
                        &token,
                    );
                    request = Some(AbortTask(tokio::spawn(async move {
                        let response = client
                            .put(endpoint)
                            .json(&json!({"value":false}))
                            .send()
                            .await;
                        // This must stay in the child. Parent proof comes from PG,
                        // not a request future's cancellation or return status.
                        response.map(|response| response.status().as_u16()).ok()
                    })));
                    emit(json!({"op":"put_started"}));
                }
                "worker" => {
                    assert!(matches!(phase.as_str(), "claimed" | "recovery"));
                    assert!(worker.is_none());
                    worker = Some(AbortTask(
                        crate::maintenance::spawn_admin_audit_delivery_worker(state.clone())
                            .expect("real durable delivery worker should spawn"),
                    ));
                    emit(json!({"op":"worker_started"}));
                }
                "observe_next_tick" => {
                    assert_eq!(phase, "recovery");
                    tokio::time::sleep(Duration::from_millis(2200)).await;
                    assert!(!worker.as_ref().unwrap().0.is_finished());
                    emit(json!({"op":"next_tick"}));
                }
                "stop" => {
                    drop(worker.take());
                    drop(request.take());
                    server.0.abort();
                    state
                        .usage_runtime
                        .shutdown(Duration::from_secs(5))
                        .await
                        .unwrap();
                    emit(json!({"op":"stopped"}));
                    return;
                }
                _ => panic!("unsupported parent operation"),
            }
        }
        // Closing the control pipe also ends this fixture child.
    })
    .await;
    assert!(result.is_ok(), "bounded child lifetime exceeded");
}

async fn business(pool: &PgPool) -> (Value, String) {
    sqlx::query_as("SELECT value::jsonb,updated_at::text FROM system_configs WHERE key=$1")
        .bind(CONFIG_KEY)
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn counts(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT COUNT(*) FROM admin_audit_delivery),(SELECT COUNT(*) FROM audit_logs WHERE event_type='admin_mutation')")
        .fetch_one(pool).await.unwrap()
}
async fn wait_blocked(pool: &PgPool, application: &str, lock: i32) -> i32 {
    wait_blocked_with_deadline(pool, application, lock, Duration::from_secs(8)).await
}

async fn wait_blocked_with_deadline(
    pool: &PgPool,
    application: &str,
    lock: i32,
    deadline: Duration,
) -> i32 {
    tokio::time::timeout(deadline, async {
        loop {
            let pid: Option<i32> = sqlx::query_scalar("SELECT a.pid FROM pg_stat_activity a JOIN pg_locks l ON l.pid=a.pid WHERE a.datname=current_database() AND a.application_name=$1 AND l.locktype='advisory' AND l.classid=255::oid AND l.objid=$2::bigint::oid AND NOT l.granted LIMIT 1")
                .bind(application).bind(i64::from(lock)).fetch_optional(pool).await.unwrap();
            if let Some(pid) = pid { return pid; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("child must reach the intended real PostgreSQL barrier")
}
async fn retire_dead_backend(pool: &PgPool, pid: i32, application: &str) {
    // The OS process has already been reaped with signal 9. PostgreSQL may
    // continue its blocked statement after client death; remove that exact
    // orphan while the barrier remains held so it cannot commit a late INSERT.
    let _terminated: Option<bool> = sqlx::query_scalar("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid=$1 AND datname=current_database() AND application_name=$2")
        .bind(pid).bind(application).fetch_optional(pool).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND datname=current_database() AND application_name=$2)")
                .bind(pid).bind(application).fetch_one(pool).await.unwrap();
            if !exists { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("dead child PostgreSQL backend must be gone before barrier release");
}

#[tokio::test]
#[ignore = "requires fresh task-owned aether_admin_audit_* PostgreSQL; real SIGKILL and new-PID recovery"]
async fn live_admin_audit_process_kill_restart() {
    let fixture = Database::attach_empty().await;
    let result = std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(180), async {
        aether_data::lifecycle::migrate::prepare_database_for_startup(&fixture.pool).await.unwrap();
        aether_data::lifecycle::migrate::run_migrations(&fixture.pool).await.unwrap();
        sqlx::query("INSERT INTO system_configs(id,key,value,created_at,updated_at) VALUES($1,$2,'true'::json,to_timestamp(1),to_timestamp(2)) ON CONFLICT(key) DO UPDATE SET value='true'::json,updated_at=to_timestamp(2)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(CONFIG_KEY).execute(&fixture.pool).await.unwrap();
        let nonce = uuid::Uuid::new_v4().to_string();
        let directory = PrivateDirectory::new(&nonce);
        let state = gateway_state(database_url());
        let (token, admin_user) = crate::tests::operational_auth::issue_operational_session_access_token_and_user(
            &state, OPERATIONAL_ADMIN_DEVICE_ID, "admin",
        ).await;
        directory.write_token(&token);
        drop(token);
        state.usage_runtime.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(state);
        let initial = business(&fixture.pool).await;
        assert_eq!(initial.0, json!(true));
        assert_eq!(counts(&fixture.pool).await, (0, 0));

        // Stage 1: kill after the business UPDATE but before the enqueue INSERT
        // completes. Neither uncommitted row may survive the process death.
        sqlx::raw_sql("CREATE FUNCTION audit_crash_enqueue_barrier() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NOT EXISTS(SELECT 1 FROM system_configs WHERE key='enable_format_conversion' AND value::jsonb='false'::jsonb) THEN RAISE EXCEPTION 'fixture business update did not precede enqueue'; END IF; PERFORM pg_advisory_xact_lock(255,255931); RETURN NEW; END $$; CREATE TRIGGER audit_crash_enqueue_barrier BEFORE INSERT ON admin_audit_delivery FOR EACH ROW EXECUTE FUNCTION audit_crash_enqueue_barrier();")
            .execute(&fixture.pool).await.unwrap();
        let mut enqueue_lock = fixture.pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(255,$1)").bind(PRECOMMIT_LOCK).execute(&mut *enqueue_lock).await.unwrap();
        let mut before = GatewayProcess::spawn(&directory.0, &nonce, "precommit");
        let before_pid = before.ready().await;
        before.send(json!({"op":"put"}));before.receive("put_started").await;
        let before_backend = wait_blocked(&fixture.pool, &before.application_name, PRECOMMIT_LOCK).await;
        assert_eq!(business(&fixture.pool).await, initial);
        assert_eq!(counts(&fixture.pool).await, (0, 0));
        before.kill_and_wait().await;
        retire_dead_backend(&fixture.pool, before_backend, &before.application_name).await;
        assert_eq!(business(&fixture.pool).await, initial);
        assert_eq!(counts(&fixture.pool).await, (0, 0));
        enqueue_lock.rollback().await.unwrap();
        drop(before);
        sqlx::raw_sql("DROP TRIGGER audit_crash_enqueue_barrier ON admin_audit_delivery; DROP FUNCTION audit_crash_enqueue_barrier();")
            .execute(&fixture.pool).await.unwrap();
        emit(json!({"phase":"killed_before_commit","pid":before_pid,"signal":9,"business_unchanged":true,"events":0,"audits":0}));

        // Stage 2: the actual HTTP mutation has committed business + intent;
        // the direct response finalizer is blocked before audit INSERT.
        sqlx::raw_sql("CREATE FUNCTION audit_crash_insert_barrier() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(255,255932); RETURN NEW; END $$; CREATE TRIGGER audit_crash_insert_barrier BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION audit_crash_insert_barrier();")
            .execute(&fixture.pool).await.unwrap();
        let mut audit_lock = fixture.pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(255,$1)").bind(AUDIT_LOCK).execute(&mut *audit_lock).await.unwrap();
        let mut after = GatewayProcess::spawn(&directory.0, &nonce, "postcommit");
        let after_pid = after.ready().await;
        assert_ne!(before_pid, after_pid);
        after.send(json!({"op":"put"}));after.receive("put_started").await;
        let after_backend = wait_blocked(&fixture.pool, &after.application_name, AUDIT_LOCK).await;
        let committed = business(&fixture.pool).await;
        assert_eq!(committed.0, json!(false));
        assert_ne!(committed.1, initial.1);
        assert_eq!(counts(&fixture.pool).await, (1, 0));
        let (event_id, delivery_state, payload_id): (String, String, String) = sqlx::query_as("SELECT event_id,state,payload->>'id' FROM admin_audit_delivery")
            .fetch_one(&fixture.pool).await.unwrap();
        assert_eq!(delivery_state,"pending");assert_eq!(payload_id,event_id);
        after.kill_and_wait().await;
        retire_dead_backend(&fixture.pool, after_backend, &after.application_name).await;
        assert_eq!(business(&fixture.pool).await, committed);
        assert_eq!(counts(&fixture.pool).await, (1, 0));
        drop(after);
        emit(json!({"phase":"killed_after_commit","pid":after_pid,"signal":9,"event_id":event_id,"events":1,"audits":0}));

        // Stage 3: kill an actual worker after its durable claim has committed,
        // while its INSERT+ACK transaction is still inside the audit barrier.
        let mut claimed = GatewayProcess::spawn(&directory.0, &nonce, "claimed");
        let claimed_pid = claimed.ready().await;
        assert_ne!(before_pid,claimed_pid);assert_ne!(after_pid,claimed_pid);
        claimed.send(json!({"op":"worker"}));claimed.receive("worker_started").await;
        let claimed_backend = wait_blocked(&fixture.pool, &claimed.application_name, AUDIT_LOCK).await;
        let (leased_state, old_token, old_expiry, lease_seconds): (String,String,chrono::DateTime<chrono::Utc>,f64) = sqlx::query_as(
            "SELECT state,lease_token::text,lease_expires_at,EXTRACT(EPOCH FROM lease_expires_at-updated_at)::double precision FROM admin_audit_delivery WHERE event_id=$1")
            .bind(&event_id).fetch_one(&fixture.pool).await.unwrap();
        assert_eq!(leased_state,"leased");
        assert!((29.0..=31.0).contains(&lease_seconds),"fixture must exercise the production 30-second lease");
        assert_eq!(counts(&fixture.pool).await,(1,0));
        assert_eq!(business(&fixture.pool).await,committed);
        claimed.kill_and_wait().await;
        retire_dead_backend(&fixture.pool, claimed_backend, &claimed.application_name).await;
        // Terminating the dead client's open INSERT transaction must preserve
        // the previously committed claim, including its original expiry/token.
        let claim_after_kill: (String,String,chrono::DateTime<chrono::Utc>,i32) = sqlx::query_as(
            "SELECT state,lease_token::text,lease_expires_at,attempt_count FROM admin_audit_delivery WHERE event_id=$1")
            .bind(&event_id).fetch_one(&fixture.pool).await.unwrap();
        assert_eq!(claim_after_kill.0,"leased");
        assert!(claim_after_kill.1==old_token,"crash cleanup must preserve the original token");
        assert_eq!(claim_after_kill.2,old_expiry);assert_eq!(claim_after_kill.3,0);
        assert_eq!(counts(&fixture.pool).await,(1,0));
        assert_eq!(business(&fixture.pool).await,committed);
        drop(claimed);
        let old_token_sha256=format!("{:x}",Sha256::digest(old_token.as_bytes()));
        emit(json!({"phase":"killed_after_claim","pid":claimed_pid,"signal":9,"event_id":event_id,"events":1,"audits":0,"state":"leased","lease_token_sha256":old_token_sha256,"lease_expires_at":old_expiry}));

        // Stage 4: a fourth PID runs only the production worker. Do not shorten
        // the persisted lease: wait for real PostgreSQL clock time to pass it.
        let mut restarted = GatewayProcess::spawn(&directory.0, &nonce, "recovery");
        let restarted_pid = restarted.ready().await;
        assert_ne!(after_pid,restarted_pid);assert_ne!(before_pid,restarted_pid);assert_ne!(claimed_pid,restarted_pid);
        let still_leased: (String,String,bool) = sqlx::query_as(
            "SELECT state,lease_token::text,lease_expires_at>clock_timestamp() FROM admin_audit_delivery WHERE event_id=$1")
            .bind(&event_id).fetch_one(&fixture.pool).await.unwrap();
        assert_eq!(still_leased.0,"leased");
        assert!(still_leased.1==old_token,"restart must preserve the dead worker's token before expiry");
        assert!(still_leased.2,"restart must begin while the dead worker's lease is still live");
        restarted.send(json!({"op":"worker"}));restarted.receive("worker_started").await;
        // The 2-second production tick may observe expiry just after 30 seconds.
        // This poll is bounded independently of the unchanged 180-second parent.
        let recovery_backend = wait_blocked_with_deadline(&fixture.pool, &restarted.application_name, AUDIT_LOCK, Duration::from_secs(40)).await;
        assert_ne!(recovery_backend, after_backend);assert_ne!(recovery_backend,claimed_backend);
        let (reclaimed_state, new_token, previous_expired): (String,String,bool) = sqlx::query_as(
            "SELECT state,lease_token::text,$2::timestamptz<=clock_timestamp() FROM admin_audit_delivery WHERE event_id=$1")
            .bind(&event_id).bind(old_expiry).fetch_one(&fixture.pool).await.unwrap();
        assert_eq!(reclaimed_state,"leased");
        assert!(previous_expired,"new worker must not reach INSERT before the old lease expires in real time");
        assert!(new_token!=old_token,"expired lease must be fenced by a new token");
        let new_token_sha256=format!("{:x}",Sha256::digest(new_token.as_bytes()));
        assert_eq!(counts(&fixture.pool).await,(1,0));
        assert_eq!(business(&fixture.pool).await,committed);
        audit_lock.rollback().await.unwrap();
        tokio::time::timeout(Duration::from_secs(15),async {
            loop {
                let delivered: bool = sqlx::query_scalar("SELECT state='delivered' AND delivered_at IS NOT NULL AND lease_token IS NULL FROM admin_audit_delivery WHERE event_id=$1")
                    .bind(&event_id).fetch_one(&fixture.pool).await.unwrap();
                if delivered { break; }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("new-PID production worker must insert and acknowledge durable event");
        let audits: (i64, i64) = sqlx::query_as("SELECT COUNT(*),COUNT(*) FILTER(WHERE id=$1) FROM audit_logs WHERE event_type='admin_mutation'")
            .bind(&event_id).fetch_one(&fixture.pool).await.unwrap();
        assert_eq!(audits,(1,1));assert_eq!(business(&fixture.pool).await,committed);
        let identity: (String,i32,String) = sqlx::query_as("SELECT event_type,status_code,user_id FROM audit_logs WHERE id=$1")
            .bind(&event_id).fetch_one(&fixture.pool).await.unwrap();
        assert_eq!(identity,("admin_mutation".into(),200,admin_user.id));
        restarted.send(json!({"op":"observe_next_tick"}));restarted.receive("next_tick").await;
        assert_eq!(counts(&fixture.pool).await,(1,1));assert_eq!(business(&fixture.pool).await,committed);
        let final_row: (String,i32,bool) = sqlx::query_as("SELECT state,attempt_count,lease_token IS NULL FROM admin_audit_delivery WHERE event_id=$1")
            .bind(&event_id).fetch_one(&fixture.pool).await.unwrap();
        assert_eq!(final_row,("delivered".into(),0,true));
        restarted.stop().await;
        emit(json!({"phase":"recovered_new_pid","pid":restarted_pid,"event_id":event_id,"events":1,"audits":1,"state":"delivered","previous_lease_expired":true,"old_lease_token_sha256":old_token_sha256,"new_lease_token_sha256":new_token_sha256,"business_value_and_timestamp_unchanged":true}));
    })).catch_unwind().await;
    fixture.close().await;
    match result {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("bounded administrator audit process crash test timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
