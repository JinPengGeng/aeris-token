use std::time::Duration;

use aether_data_contracts::repository::settlement::{
    CompleteRechargeRecoveryNotificationInput, RechargeRecoveryNotification,
    RechargeRecoveryNotificationAudience, RechargeRecoveryNotificationOutcome,
    StoredRechargeRecoveryJob, REQUEST_FUNDS_UNITS_PER_USD,
};

use crate::important_notification::{
    important_notification_dispatch_ready_for_item, send_important_notification_for_item,
    send_user_important_notification_email, ImportantNotification,
    ImportantNotificationDeliveryReport, RECHARGE_RECOVERY_REVIEW_ITEM_KEY,
    USER_RECHARGE_RECOVERY_ITEM_KEY,
};
use crate::{AppState, GatewayError};

const RECOVERY_INTERVAL: Duration = Duration::from_secs(5);
const RECOVERY_BATCH_SIZE: usize = 16;
const NOTIFICATIONS_PER_TICK: usize = 8;
const NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn spawn_recharge_recovery_worker(app: AppState) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_recharge_recovery_backend() {
        return None;
    }
    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_RECHARGE_RECOVERY,
        |app| async move {
            let mut interval = tokio::time::interval(RECOVERY_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                // Retry schedule, operation identity and transaction limits are
                // persisted by the repository; never emulate them in this loop.
                if app
                    .data
                    .process_recharge_recovery_batch(RECOVERY_BATCH_SIZE)
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        event_name = "recharge_recovery_batch_failed",
                        "recharge recovery will retry from durable state"
                    );
                }
                // A failed collection batch must not suppress notifications for
                // already committed collections or jobs requiring review.
                if deliver_recharge_recovery_notifications(&app).await.is_err() {
                    tracing::warn!(
                        event_name = "recharge_recovery_notification_failed",
                        "recharge recovery notification remains pending"
                    );
                }
            }
        },
    ))
}

fn usd(units: u64) -> String {
    format!(
        "{}.{:08}",
        units / REQUEST_FUNDS_UNITS_PER_USD,
        units % REQUEST_FUNDS_UNITS_PER_USD
    )
}

fn recovery_snapshot_amount(job: &StoredRechargeRecoveryJob, units: u64) -> String {
    if matches!(job.state.as_str(), "manual_review" | "source_unavailable") {
        "待核对".into()
    } else {
        format!("${}", usd(units))
    }
}

fn recovery_notification(job: &StoredRechargeRecoveryJob, admin: bool) -> ImportantNotification {
    let title = if admin {
        "历史欠费追扣需要处理"
    } else {
        "充值后历史欠费处理结果"
    };
    let text = format!(
        "充值参考：{}\n到账本金：${}\n本次充值累计追扣：${}\n本次充值对应的剩余欠费：{}\n本次处理后可用本金：{}\n金额为本次处理时的快照，最新余额请查看钱包。{}",
        job.payment_order_id,
        usd(job.principal_cost_units),
        usd(job.collected_cost_units),
        recovery_snapshot_amount(job, job.outstanding_cost_units),
        recovery_snapshot_amount(job, job.available_recharge_cost_units),
        if admin { "\n请在后台核对该充值的追扣记录。" } else { "\n每笔扣款可在钱包流水中查看。" },
    );
    ImportantNotification {
        title: title.into(),
        markdown_body: text.clone(),
        text_body: text,
    }
}

fn delivery_outcome(
    report: &ImportantNotificationDeliveryReport,
) -> RechargeRecoveryNotificationOutcome {
    // Aggregate notification success can mean only one channel succeeded. This
    // financial outbox acknowledges delivery only when every attempted channel
    // succeeded. A repeat after partial delivery keeps the same recharge ID.
    if report.success
        && !report.channels.is_empty()
        && report.channels.iter().all(|channel| channel.success)
    {
        RechargeRecoveryNotificationOutcome::Delivered
    } else if report
        .channels
        .iter()
        .all(|channel| matches!(channel.channel, "module" | "item" | "none"))
    {
        RechargeRecoveryNotificationOutcome::Skipped
    } else {
        RechargeRecoveryNotificationOutcome::Retry
    }
}

async fn send_recovery_notification(
    state: &AppState,
    entry: &RechargeRecoveryNotification,
) -> Result<RechargeRecoveryNotificationOutcome, GatewayError> {
    let admin = entry.audience == RechargeRecoveryNotificationAudience::Admin;
    let notification = recovery_notification(&entry.summary, admin);
    let variables = [
        ("payment_order_id", entry.summary.payment_order_id.clone()),
        ("principal_usd", usd(entry.summary.principal_cost_units)),
        ("collected_usd", usd(entry.summary.collected_cost_units)),
        (
            "outstanding_usd",
            recovery_snapshot_amount(&entry.summary, entry.summary.outstanding_cost_units)
                .trim_start_matches('$')
                .to_string(),
        ),
        (
            "available_principal_usd",
            recovery_snapshot_amount(&entry.summary, entry.summary.available_recharge_cost_units)
                .trim_start_matches('$')
                .to_string(),
        ),
    ];
    let report = if admin {
        if !important_notification_dispatch_ready_for_item(state, RECHARGE_RECOVERY_REVIEW_ITEM_KEY)
            .await?
        {
            return Ok(RechargeRecoveryNotificationOutcome::Skipped);
        }
        send_important_notification_for_item(
            state,
            RECHARGE_RECOVERY_REVIEW_ITEM_KEY,
            notification,
            &variables,
        )
        .await?
    } else {
        let Some(user_id) = entry
            .user_id
            .as_deref()
            .filter(|id| entry.summary.user_id.as_deref() == Some(*id))
        else {
            return Ok(RechargeRecoveryNotificationOutcome::Skipped);
        };
        let Some(user) = state
            .data
            .find_user_auth_by_id(user_id)
            .await
            .map_err(|_| {
                GatewayError::Internal("recovery notification owner lookup failed".into())
            })?
        else {
            return Ok(RechargeRecoveryNotificationOutcome::Skipped);
        };
        if user.is_deleted || !user.is_active || !user.email_verified {
            return Ok(RechargeRecoveryNotificationOutcome::Skipped);
        }
        let Some(email) = user
            .email
            .as_deref()
            .filter(|email| !email.trim().is_empty())
        else {
            return Ok(RechargeRecoveryNotificationOutcome::Skipped);
        };
        send_user_important_notification_email(
            state,
            USER_RECHARGE_RECOVERY_ITEM_KEY,
            email,
            notification,
            &variables,
        )
        .await?
    };
    Ok(delivery_outcome(&report))
}

async fn deliver_recharge_recovery_notifications(
    state: &AppState,
) -> Result<(), aether_data_contracts::DataLayerError> {
    for _ in 0..NOTIFICATIONS_PER_TICK {
        // Claim immediately before delivery so slow earlier SMTP calls do not
        // consume another notification's lease while it waits in a local queue.
        let entries = state.data.claim_recharge_recovery_notifications(1).await?;
        let Some(entry) = entries.into_iter().next() else {
            break;
        };
        let outcome = match tokio::time::timeout(
            NOTIFICATION_TIMEOUT,
            send_recovery_notification(state, &entry),
        )
        .await
        {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) | Err(_) => RechargeRecoveryNotificationOutcome::Retry,
        };
        let error_code = match outcome {
            RechargeRecoveryNotificationOutcome::Delivered => None,
            RechargeRecoveryNotificationOutcome::Skipped => Some("notification_unavailable".into()),
            RechargeRecoveryNotificationOutcome::Retry => {
                Some("notification_delivery_failed".into())
            }
        };
        state
            .data
            .complete_recharge_recovery_notification(CompleteRechargeRecoveryNotificationInput {
                id: entry.id,
                lease_token: entry.lease_token,
                outcome,
                error_code,
            })
            .await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "recharge_recovery/live_tests.rs"]
mod live_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::GatewayDataState;
    use crate::important_notification::{
        ImportantNotificationChannelReport, IMPORTANT_NOTIFICATION_DEFAULT_CHANNEL_KEY,
        IMPORTANT_NOTIFICATION_EMAIL_ENABLED_KEY, IMPORTANT_NOTIFICATION_EMAIL_RECIPIENTS_KEY,
        IMPORTANT_NOTIFICATION_ENABLED_KEY, IMPORTANT_NOTIFICATION_ITEMS_KEY,
    };
    use base64::Engine;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn job() -> StoredRechargeRecoveryJob {
        StoredRechargeRecoveryJob {
            id: "synthetic-recovery".into(),
            payment_order_id: "synthetic-credit".into(),
            source_transaction_id: "private-credit-transaction".into(),
            wallet_id: "private-wallet".into(),
            user_id: Some("synthetic-owner".into()),
            state: "manual_review".into(),
            principal_cost_units: 1_000_000_000,
            collected_cost_units: 800_000_001,
            outstanding_cost_units: 499_999_999,
            available_recharge_cost_units: 199_999_999,
            retry_count: 6,
            next_attempt_at_unix_secs: None,
            error_code: Some("synthetic_failure".into()),
            created_at_unix_secs: 1,
            updated_at_unix_secs: 2,
        }
    }

    #[test]
    fn recovery_notification_preserves_smallest_money_unit_without_private_fields() {
        let mut job = job();
        job.state = "waiting_next_recharge".into();
        let body = recovery_notification(&job, false).text_body;
        for amount in ["$10.00000000", "$8.00000001", "$4.99999999", "$1.99999999"] {
            assert!(body.contains(amount), "missing exact amount {amount}");
        }
        assert!(body.contains("synthetic-credit"));
        assert!(body.contains("快照"));
        assert!(!body.contains("private-"));
        assert!(!body.contains("synthetic_failure"));
    }

    #[test]
    fn recovery_notification_never_reports_unverified_debt_as_zero() {
        for state in ["manual_review", "source_unavailable"] {
            let mut job = job();
            job.state = state.into();
            job.outstanding_cost_units = 0;
            job.available_recharge_cost_units = 0;
            let body = recovery_notification(&job, true).text_body;
            assert!(body.contains("剩余欠费：待核对"));
            assert!(body.contains("可用本金：待核对"));
            assert!(!body.contains("$0.00000000"));
            assert!(body.contains("$8.00000001"));
        }
    }

    #[test]
    fn recovery_notification_does_not_acknowledge_partial_channel_delivery() {
        let report = ImportantNotificationDeliveryReport {
            success: true,
            channels: vec![
                ImportantNotificationChannelReport {
                    channel: "email",
                    success: true,
                    message: String::new(),
                },
                ImportantNotificationChannelReport {
                    channel: "bark",
                    success: false,
                    message: String::new(),
                },
            ],
        };
        assert_eq!(
            delivery_outcome(&report),
            RechargeRecoveryNotificationOutcome::Retry
        );
    }

    #[tokio::test]
    async fn recovery_notification_disabled_delivery_is_retained_and_backend_does_not_spawn() {
        let state = AppState::new()
            .unwrap()
            .with_data_state_for_tests(GatewayDataState::disabled());
        assert!(spawn_recharge_recovery_worker(state.clone()).is_none());
        let entry = RechargeRecoveryNotification {
            id: "synthetic-notification".into(),
            job_id: "synthetic-recovery".into(),
            user_id: Some("synthetic-owner".into()),
            audience: RechargeRecoveryNotificationAudience::Admin,
            lease_token: 1,
            summary: job(),
        };
        assert_eq!(
            send_recovery_notification(&state, &entry).await.unwrap(),
            RechargeRecoveryNotificationOutcome::Skipped
        );
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

    #[tokio::test]
    async fn recovery_notification_retries_partial_smtp_fanout_with_stable_credit_reference() {
        let (port, server) = smtp_server(4, true).await;
        let state = AppState::new().unwrap().with_data_state_for_tests(
            GatewayDataState::disabled().with_system_config_values_for_tests(vec![
                (IMPORTANT_NOTIFICATION_ENABLED_KEY.into(), json!(true)),
                (IMPORTANT_NOTIFICATION_EMAIL_ENABLED_KEY.into(), json!(true)),
                (
                    IMPORTANT_NOTIFICATION_DEFAULT_CHANNEL_KEY.into(),
                    json!("email"),
                ),
                (
                    IMPORTANT_NOTIFICATION_EMAIL_RECIPIENTS_KEY.into(),
                    json!(["first@example.invalid", "second@example.invalid"]),
                ),
                ("smtp_host".into(), json!("127.0.0.1")),
                ("smtp_port".into(), json!(port)),
                ("smtp_use_tls".into(), json!(false)),
                ("smtp_use_ssl".into(), json!(false)),
                ("smtp_from_email".into(), json!("recovery@example.invalid")),
            ]),
        );
        let entry = RechargeRecoveryNotification {
            id: "synthetic-notification".into(),
            job_id: "synthetic-recovery".into(),
            user_id: Some("synthetic-owner".into()),
            audience: RechargeRecoveryNotificationAudience::Admin,
            lease_token: 1,
            summary: job(),
        };
        for expected in [
            RechargeRecoveryNotificationOutcome::Retry,
            RechargeRecoveryNotificationOutcome::Delivered,
        ] {
            assert_eq!(
                tokio::time::timeout(
                    Duration::from_secs(10),
                    send_recovery_notification(&state, &entry)
                )
                .await
                .unwrap()
                .unwrap(),
                expected
            );
        }
        let bodies = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bodies.len(), 4);
        for message in bodies {
            for body in decoded_mime_bodies(&message) {
                assert!(body.contains("synthetic-credit"));
                assert!(!body.contains("private-credit-transaction"));
                assert!(!body.contains("private-wallet"));
            }
        }
    }

    #[tokio::test]
    async fn recovery_user_email_skips_unavailable_settings_then_retries_real_smtp_failure() {
        let (port, server) = smtp_server(2, true).await;
        // All initial sends are skipped, so the first actual DATA must receive
        // the synthetic 550 after configuration is restored.
        for (module_enabled, item_enabled, user_email_enabled, smtp_ready, email, expected) in [
            (
                false,
                true,
                true,
                true,
                "owner@example.invalid",
                RechargeRecoveryNotificationOutcome::Skipped,
            ),
            (
                true,
                false,
                true,
                true,
                "owner@example.invalid",
                RechargeRecoveryNotificationOutcome::Skipped,
            ),
            (
                true,
                true,
                false,
                true,
                "owner@example.invalid",
                RechargeRecoveryNotificationOutcome::Skipped,
            ),
            (
                true,
                true,
                true,
                false,
                "owner@example.invalid",
                RechargeRecoveryNotificationOutcome::Skipped,
            ),
            (
                true,
                true,
                true,
                true,
                " ",
                RechargeRecoveryNotificationOutcome::Skipped,
            ),
            (
                true,
                true,
                true,
                true,
                "owner@example.invalid",
                RechargeRecoveryNotificationOutcome::Retry,
            ),
            (
                true,
                true,
                true,
                true,
                "owner@example.invalid",
                RechargeRecoveryNotificationOutcome::Delivered,
            ),
        ] {
            let mut config = vec![
                (
                    IMPORTANT_NOTIFICATION_ENABLED_KEY.into(),
                    json!(module_enabled),
                ),
                (
                    IMPORTANT_NOTIFICATION_ITEMS_KEY.into(),
                    json!([{
                        "key": USER_RECHARGE_RECOVERY_ITEM_KEY,
                        "enabled": item_enabled,
                        "user_email_enabled": user_email_enabled
                    }]),
                ),
            ];
            if smtp_ready {
                config.extend([
                    ("smtp_host".into(), json!("127.0.0.1")),
                    ("smtp_port".into(), json!(port)),
                    ("smtp_use_tls".into(), json!(false)),
                    ("smtp_use_ssl".into(), json!(false)),
                    ("smtp_from_email".into(), json!("recovery@example.invalid")),
                ]);
            }
            let state = AppState::new().unwrap().with_data_state_for_tests(
                GatewayDataState::disabled().with_system_config_values_for_tests(config),
            );
            let report = tokio::time::timeout(
                Duration::from_secs(10),
                send_user_important_notification_email(
                    &state,
                    USER_RECHARGE_RECOVERY_ITEM_KEY,
                    email,
                    recovery_notification(&job(), false),
                    &[],
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(delivery_outcome(&report), expected);
        }
        let bodies = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bodies.len(), 2, "configuration skips must not attempt SMTP");
        for message in bodies {
            for body in decoded_mime_bodies(&message) {
                assert!(body.contains("synthetic-credit"));
                assert!(!body.contains("private-"));
            }
        }
    }
}
