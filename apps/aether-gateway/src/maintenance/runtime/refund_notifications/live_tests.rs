//! Real PostgreSQL outbox and loopback SMTP; all data are synthetic.
use super::*;
use crate::data::GatewayDataState;
use crate::important_notification::{
    IMPORTANT_NOTIFICATION_ENABLED_KEY, IMPORTANT_NOTIFICATION_ITEMS_KEY,
};
use aether_data::driver::postgres::{run_migrations, SqlxUserReadRepository, SqlxWalletRepository};
use aether_data_contracts::repository::wallet::{
    CompleteAdminWalletRefundInput, FailAdminWalletRefundInput, ProcessAdminWalletRefundInput,
    WalletMutationOutcome, WalletWriteRepository,
};
use base64::Engine;
use futures_util::FutureExt;
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct IsolatedDatabase {
    admin: PgPool,
    pool: PgPool,
    name: String,
}
impl IsolatedDatabase {
    async fn new() -> Self {
        let url = std::env::var("AETHER_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("AETHER_TEST_POSTGRES_URL"))
            .expect("requires disposable PostgreSQL with CREATEDB permission");
        let options = url.parse::<sqlx::postgres::PgConnectOptions>().unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await
            .unwrap();
        let name = format!("gateway_refund_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(
                options
                    .database(&name)
                    .options([("statement_timeout", "15000")]),
            )
            .await
            .unwrap();
        Self { admin, pool, name }
    }
    async fn close(self) {
        tokio::time::timeout(Duration::from_secs(15), self.pool.close())
            .await
            .expect("pool cleanup timeout");
        tokio::time::timeout(
            Duration::from_secs(15),
            // This UUID-named database belongs exclusively to this fixture.
            // Server-side sessions can outlive client-side pool shutdown.
            sqlx::query(&format!("DROP DATABASE {} WITH (FORCE)", self.name)).execute(&self.admin),
        )
        .await
        .expect("database cleanup timeout")
        .unwrap();
        self.admin.close().await;
    }
}

struct AbortOnDrop(tokio::task::AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn smtp_server(
    connections: usize,
    fail_first: bool,
) -> (u16, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let mut bodies = Vec::new();
        for attempt in 0..connections {
            let (socket, _) = listener.accept().await.unwrap();
            let (input, mut output) = socket.into_split();
            let mut reader = BufReader::new(input);
            output
                .write_all(b"220 local synthetic SMTP\r\n")
                .await
                .unwrap();
            let mut in_body = false;
            let mut body = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                if in_body {
                    if line == ".\r\n" {
                        output
                            .write_all(if fail_first && attempt == 0 {
                                b"550 synthetic delivery failure\r\n"
                            } else {
                                b"250 accepted\r\n"
                            })
                            .await
                            .unwrap();
                        bodies.push(body.clone());
                        in_body = false;
                    } else {
                        body.push_str(&line);
                    }
                } else if line.starts_with("EHLO")
                    || line.starts_with("MAIL FROM")
                    || line.starts_with("RCPT TO")
                {
                    output.write_all(b"250 ok\r\n").await.unwrap();
                } else if line.starts_with("DATA") {
                    in_body = true;
                    output.write_all(b"354 continue\r\n").await.unwrap();
                } else if line.starts_with("QUIT") {
                    output.write_all(b"221 bye\r\n").await.unwrap();
                    break;
                } else {
                    panic!("unexpected synthetic SMTP command");
                }
            }
        }
        bodies
    });
    (port, server)
}

pub(super) fn decoded_mime_bodies(message: &str) -> Vec<String> {
    let (headers, body) = message.split_once("\r\n\r\n").expect("MIME headers");
    let content_type = headers
        .lines()
        .find(|line| line.starts_with("Content-Type: multipart/alternative;"))
        .expect("multipart content type");
    let boundary = content_type
        .split("boundary=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let delimiter = format!("--{boundary}");
    let mut types = Vec::new();
    let mut decoded = Vec::new();
    for part in body.split(&delimiter).skip(1) {
        if part.starts_with("--") {
            break;
        }
        let (headers, encoded) = part
            .trim_start_matches("\r\n")
            .split_once("\r\n\r\n")
            .expect("part headers");
        assert!(headers
            .lines()
            .any(|line| line == "Content-Transfer-Encoding: base64"));
        let content_type = headers
            .lines()
            .find(|line| line.starts_with("Content-Type:"))
            .unwrap();
        types.push(content_type.split(';').next().unwrap());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded.split_whitespace().collect::<String>())
            .expect("valid base64 MIME part");
        decoded.push(String::from_utf8(bytes).expect("UTF-8 MIME part"));
    }
    assert_eq!(
        types,
        ["Content-Type: text/plain", "Content-Type: text/html"]
    );
    decoded
}

async fn seed(pool: &PgPool, succeeded: bool) {
    run_migrations(pool).await.unwrap();
    sqlx::raw_sql("INSERT INTO users(id,username,email,email_verified,is_active,is_deleted,auth_source,created_at,updated_at) VALUES('refund-owner','refund-owner','owner@example.invalid',true,true,false,'local',now(),now()); INSERT INTO wallets(id,user_id,balance,gift_balance,status,limit_mode,created_at,updated_at) VALUES('private-wallet','refund-owner',0.10,0,'active','finite',now(),now()); INSERT INTO refund_requests(id,refund_no,wallet_id,user_id,amount_usd,status,source_type,refund_mode,payout_proof,created_at,updated_at) VALUES('synthetic-refund','PUBLIC-REFUND-001','private-wallet','refund-owner',0.03000001,'approved','wallet','offline_payout','{\"private_provider_proof\":true}',now(),now())")
        .execute(pool).await.unwrap();
    let wallet = SqlxWalletRepository::new(pool.clone());
    if succeeded {
        assert!(matches!(
            wallet
                .process_admin_wallet_refund(ProcessAdminWalletRefundInput {
                    wallet_id: "private-wallet".into(),
                    refund_id: "synthetic-refund".into(),
                    operator_id: None
                })
                .await
                .unwrap(),
            WalletMutationOutcome::Applied(_)
        ));
        assert!(matches!(
            wallet
                .complete_admin_wallet_refund(CompleteAdminWalletRefundInput {
                    wallet_id: "private-wallet".into(),
                    refund_id: "synthetic-refund".into(),
                    gateway_refund_id: None,
                    payout_reference: None,
                    payout_proof: None
                })
                .await
                .unwrap(),
            WalletMutationOutcome::Applied(_)
        ));
    } else {
        assert!(matches!(
            wallet
                .fail_admin_wallet_refund(FailAdminWalletRefundInput {
                    wallet_id: "private-wallet".into(),
                    refund_id: "synthetic-refund".into(),
                    reason: "internal SQL error Authorization: Bearer synthetic-secret-token"
                        .into(),
                    operator_id: None
                })
                .await
                .unwrap(),
            WalletMutationOutcome::Applied(_)
        ));
    }
}

fn app(pool: &PgPool, port: u16, module: bool, item: bool, email: bool, smtp: bool) -> AppState {
    let mut config = vec![
        (IMPORTANT_NOTIFICATION_ENABLED_KEY.into(), json!(module)),
        (
            IMPORTANT_NOTIFICATION_ITEMS_KEY.into(),
            json!([{"key":USER_REFUND_STATUS_ITEM_KEY,"enabled":item,"user_email_enabled":email}]),
        ),
    ];
    if smtp {
        config.extend([
            ("smtp_host".into(), json!("127.0.0.1")),
            ("smtp_port".into(), json!(port)),
            ("smtp_use_tls".into(), json!(false)),
            ("smtp_use_ssl".into(), json!(false)),
            ("smtp_from_email".into(), json!("refund@example.invalid")),
        ]);
    }
    AppState::new().unwrap().with_data_state_for_tests(
        GatewayDataState::with_user_reader_for_tests(Arc::new(SqlxUserReadRepository::new(
            pool.clone(),
        )))
        .with_refund_notification_repository_for_tests(Arc::new(SqlxWalletRepository::new(
            pool.clone(),
        )))
        .with_system_config_values_for_tests(config),
    )
}
async fn financial(pool: &PgPool) -> Value {
    let wallet:(i64,i64,i64)=sqlx::query_as("SELECT ROUND(balance*100000000)::bigint,ROUND(total_refunded*100000000)::bigint,(SELECT COUNT(*) FROM wallet_transactions) FROM wallets WHERE id='private-wallet'").fetch_one(pool).await.unwrap();
    let refund: (String, Value) = sqlx::query_as(
        "SELECT status,payout_proof FROM refund_requests WHERE id='synthetic-refund'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    json!({"wallet":wallet,"refund":refund})
}
async fn notification(pool: &PgPool) -> (String, i32, bool) {
    sqlx::query_as(
        "SELECT state,attempts,delivered_at IS NOT NULL FROM refund_status_notifications",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}
async fn due(pool: &PgPool) {
    sqlx::query("UPDATE refund_status_notifications SET next_attempt_at=clock_timestamp()-interval '1 second'").execute(pool).await.unwrap();
}
async fn tick(app: &AppState) {
    deliver_refund_notifications(app).await.unwrap();
}
fn check_message(message: &str, status: &str) {
    assert!(message.contains("owner@example.invalid"));
    for body in decoded_mime_bodies(message) {
        assert!(body.contains("PUBLIC-REFUND-001"));
        assert!(body.contains("0.03000001"));
        assert!(body.contains(status));
        assert!(!body.contains("private-wallet"));
        assert!(!body.contains("private_provider_proof"));
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL with CREATEDB; real SMTP rejection and durable retry"]
async fn live_gateway_refund_notification_restarts_after_smtp_failure_without_repeating_money() {
    let f = IsolatedDatabase::new().await;
    let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(120), async {
        seed(&f.pool, true).await;
        let before = financial(&f.pool).await;
        assert_eq!(before["wallet"], json!([6_999_999, 3_000_001, 1]));
        let (port, server) = smtp_server(2, true).await;
        let _smtp_abort = AbortOnDrop(server.abort_handle());
        let first = app(&f.pool, port, true, true, true, true);
        assert_eq!(notification(&f.pool).await, ("pending".into(), 0, false));
        tick(&first).await;
        assert_eq!(notification(&f.pool).await, ("retry".into(), 1, false));
        assert_eq!(financial(&f.pool).await, before);
        assert!(first
            .data
            .claim_refund_status_notifications(1)
            .await
            .unwrap()
            .is_empty());
        first
            .usage_runtime
            .shutdown(Duration::from_secs(5))
            .await
            .unwrap();
        drop(first);
        // A new process facade and repository recover the committed obligation.
        let restarted = app(&f.pool, port, true, true, true, true);
        due(&f.pool).await;
        tick(&restarted).await;
        assert_eq!(notification(&f.pool).await, ("delivered".into(), 1, true));
        let messages = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(messages.len(), 2);
        for message in messages {
            check_message(&message, "succeeded");
        }
        tick(&restarted).await;
        let replay = SqlxWalletRepository::new(f.pool.clone())
            .complete_admin_wallet_refund(CompleteAdminWalletRefundInput {
                wallet_id: "private-wallet".into(),
                refund_id: "synthetic-refund".into(),
                gateway_refund_id: None,
                payout_reference: None,
                payout_proof: None,
            })
            .await
            .unwrap();
        assert!(matches!(replay, WalletMutationOutcome::Applied(_)));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM refund_status_notifications")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(financial(&f.pool).await, before);
        restarted
            .usage_runtime
            .shutdown(Duration::from_secs(5))
            .await
            .unwrap();
    }))
    .catch_unwind()
    .await;
    f.close().await;
    match result {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("bounded refund SMTP recovery test timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL with CREATEDB; consent/configuration boundaries and real SMTP"]
async fn live_gateway_refund_notification_preserves_consent_skips_and_retries_preference_read_failure(
) {
    let f = IsolatedDatabase::new().await;
    let result=AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(120),async {
        seed(&f.pool,false).await;
        let before=financial(&f.pool).await;
        let (port,server)=smtp_server(1,false).await;
        let _smtp_abort=AbortOnDrop(server.abort_handle());
        for (module,item,email,smtp) in [(false,true,true,true),(true,false,true,true),(true,true,false,true),(true,true,true,false)] {
            let disabled=app(&f.pool,port,module,item,email,smtp);
            due(&f.pool).await;tick(&disabled).await;
            assert_eq!(notification(&f.pool).await,("skipped".into(),0,false));
            assert_eq!(financial(&f.pool).await,before);
            disabled.usage_runtime.shutdown(Duration::from_secs(5)).await.unwrap();
        }
        let active=app(&f.pool,port,true,true,true,true);
        sqlx::query("INSERT INTO user_preferences(id,user_id,email_notifications,usage_alerts) VALUES('refund-prefs','refund-owner',false,true)").execute(&f.pool).await.unwrap();
        due(&f.pool).await;tick(&active).await;
        assert_eq!(notification(&f.pool).await,("skipped".into(),0,false));
        sqlx::query("UPDATE user_preferences SET email_notifications=true,usage_alerts=false").execute(&f.pool).await.unwrap();
        for update in ["UPDATE users SET is_active=false","UPDATE users SET is_active=true,is_deleted=true","UPDATE users SET is_deleted=false,email=NULL"] {
            sqlx::query(update).execute(&f.pool).await.unwrap();
            due(&f.pool).await;tick(&active).await;
            assert_eq!(notification(&f.pool).await,("skipped".into(),0,false));
        }
        // Legacy refund consent does not require verified email. usage_alerts is
        // independent: false must not suppress an opted-in billing message.
        sqlx::query("UPDATE users SET email='owner@example.invalid',email_verified=false").execute(&f.pool).await.unwrap();
        sqlx::query("ALTER TABLE user_preferences RENAME TO synthetic_unavailable_preferences").execute(&f.pool).await.unwrap();
        due(&f.pool).await;tick(&active).await;
        assert_eq!(notification(&f.pool).await,("retry".into(),1,false));
        assert_eq!(financial(&f.pool).await,before);
        sqlx::query("ALTER TABLE synthetic_unavailable_preferences RENAME TO user_preferences").execute(&f.pool).await.unwrap();
        // A new application facade recovers the persisted retry state.
        active.usage_runtime.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(active);
        let restarted=app(&f.pool,port,true,true,true,true);
        due(&f.pool).await;tick(&restarted).await;
        assert_eq!(notification(&f.pool).await,("delivered".into(),1,true));
        let messages=tokio::time::timeout(Duration::from_secs(5),server).await.unwrap().unwrap();
        assert_eq!(messages.len(),1,"configuration and consent skips must never submit SMTP DATA");
        check_message(&messages[0],"failed");
        for body in decoded_mime_bodies(&messages[0]) {assert!(body.contains("退款未完成"));
                assert!(!body.contains("synthetic-secret-token"));
                assert!(!body.contains("Authorization"));
                assert!(!body.contains("internal SQL error"));}
        assert_eq!(financial(&f.pool).await,before);
        restarted.usage_runtime.shutdown(Duration::from_secs(5)).await.unwrap();
    })).catch_unwind().await;
    f.close().await;
    match result {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("bounded refund consent test timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
