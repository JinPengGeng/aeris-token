//! Real transport proves legacy fallback and durable backend exclusion.
use super::{notify_user_refund_status, refund_status_notification_should_send};
use crate::data::GatewayDataState;
use crate::important_notification::{
    IMPORTANT_NOTIFICATION_ENABLED_KEY, IMPORTANT_NOTIFICATION_ITEMS_KEY,
    USER_REFUND_STATUS_ITEM_KEY,
};
use crate::{AdminWalletRefundRecord, AppState, GatewayUserPreferenceView};
use aether_data::driver::postgres::SqlxWalletRepository;
use aether_data::repository::users::StoredUserAuthRecord;
use base64::Engine;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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

fn refund() -> AdminWalletRefundRecord {
    AdminWalletRefundRecord {
        id: "refund-1".to_string(),
        refund_no: "rf-refund-1".to_string(),
        wallet_id: "wallet-1".to_string(),
        user_id: Some("user-1".to_string()),
        payment_order_id: None,
        source_type: "manual".to_string(),
        source_id: None,
        refund_mode: "manual".to_string(),
        amount_usd: 10.0,
        status: "failed".to_string(),
        reason: None,
        failure_reason: Some(
            "internal SQL error Authorization: Bearer synthetic-secret-token".into(),
        ),
        gateway_refund_id: None,
        payout_method: None,
        payout_reference: None,
        payout_proof: None,
        requested_by: Some("user-1".to_string()),
        approved_by: None,
        processed_by: None,
        created_at_unix_ms: 1_710_000_000,
        updated_at_unix_secs: 1_710_000_000,
        processed_at_unix_secs: Some(1_710_000_000),
        completed_at_unix_secs: Some(1_710_000_000),
    }
}
fn app(port: u16, durable: bool) -> AppState {
    let mut data=GatewayDataState::disabled().with_user_preferences_for_tests(std::iter::empty())
        .with_system_config_values_for_tests([
            (IMPORTANT_NOTIFICATION_ENABLED_KEY.into(),json!(true)),
            (IMPORTANT_NOTIFICATION_ITEMS_KEY.into(),json!([{"key":USER_REFUND_STATUS_ITEM_KEY,"enabled":true,"user_email_enabled":true}])),
            ("smtp_host".into(),json!("127.0.0.1")),("smtp_port".into(),json!(port)),
            ("smtp_use_tls".into(),json!(false)),("smtp_use_ssl".into(),json!(false)),
            ("smtp_from_email".into(),json!("refund@example.invalid"))
        ]);
    if durable {
        // Capability selection needs no live database. Any attempt to query
        // this pool is a bug: PG notification delivery belongs to its worker.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        data = data.with_refund_notification_repository_for_tests(Arc::new(
            SqlxWalletRepository::new(pool),
        ));
    }
    AppState::new()
        .unwrap()
        .with_data_state_for_tests(data)
        .with_auth_users_for_tests([StoredUserAuthRecord::new(
            "user-1".into(),
            Some("alice@example.com".into()),
            true,
            "alice".into(),
            None,
            "user".into(),
            "local".into(),
            None,
            None,
            None,
            true,
            false,
            None,
            None,
        )
        .unwrap()])
}

#[tokio::test]
async fn refund_notification_legacy_fallback_delivers_real_smtp_but_pg_never_dispatches_directly() {
    let (port, server) = smtp_server(2, true).await;
    struct AbortOnDrop(tokio::task::AbortHandle);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _guard = AbortOnDrop(server.abort_handle());
    let fallback = app(port, false);
    let durable = app(port, true);
    let fallback_state = crate::admin_api::AdminAppState::new(&fallback);
    let durable_state = crate::admin_api::AdminAppState::new(&durable);
    let refund = refund();
    assert!(refund_status_notification_should_send(
        Some("processing"),
        &refund.status
    ));
    assert!(!refund_status_notification_should_send(
        Some("failed"),
        &refund.status
    ));
    assert!(!fallback.data.has_refund_notification_backend());
    assert!(durable.data.has_refund_notification_backend());
    // Opted-in PG must not touch SMTP, even if the helper is called directly.
    assert!(!notify_user_refund_status(&durable_state, &refund).await);
    let mut prefs = GatewayUserPreferenceView::default_for_user("user-1");
    prefs.email_notifications = false;
    fallback
        .write_user_preferences(prefs.clone())
        .await
        .unwrap();
    assert!(!notify_user_refund_status(&fallback_state, &refund).await);
    prefs.email_notifications = true;
    prefs.usage_alerts = false;
    fallback.write_user_preferences(prefs).await.unwrap();
    // First actual DATA gets 550; only the second 250 is delivery success.
    assert!(!tokio::time::timeout(
        Duration::from_secs(10),
        notify_user_refund_status(&fallback_state, &refund)
    )
    .await
    .unwrap());
    assert!(tokio::time::timeout(
        Duration::from_secs(10),
        notify_user_refund_status(&fallback_state, &refund)
    )
    .await
    .unwrap());
    let messages = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(messages.len(), 2);
    for message in messages {
        assert!(message.contains("alice@example.com"));
        for body in decoded_mime_bodies(&message) {
            assert!(body.contains("rf-refund-1"));
            assert!(body.contains("failed"));
            assert!(body.contains("退款未完成"));
            assert!(!body.contains("synthetic-secret-token"));
            assert!(!body.contains("Authorization"));
            assert!(!body.contains("internal SQL error"));
        }
    }
    fallback
        .usage_runtime
        .shutdown(Duration::from_secs(5))
        .await
        .unwrap();
    durable
        .usage_runtime
        .shutdown(Duration::from_secs(5))
        .await
        .unwrap();
}
