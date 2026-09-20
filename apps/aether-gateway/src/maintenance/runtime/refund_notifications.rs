use crate::important_notification::{
    send_user_important_notification_email, ImportantNotification,
    ImportantNotificationDeliveryReport, USER_REFUND_STATUS_ITEM_KEY,
};
use crate::{AppState, GatewayError};
use aether_data_contracts::repository::wallet::{
    CompleteRefundStatusNotificationInput, RefundNotificationOutcome, RefundStatusNotification,
};
use std::time::Duration;

#[cfg(test)]
#[path = "refund_notifications/live_tests.rs"]
mod live_tests;

pub(crate) fn spawn_refund_notification_worker(
    app: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_refund_notification_backend() {
        return None;
    }
    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_REFUND_NOTIFICATIONS,
        |app| async move {
            let mut timer = tokio::time::interval(Duration::from_secs(5));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                timer.tick().await;
                if deliver_refund_notifications(&app).await.is_err() {
                    tracing::warn!(
                        event_name = "refund_notification_outbox_failed",
                        "refund notification remains durable for retry"
                    );
                }
            }
        },
    ))
}

fn delivery_outcome(report: &ImportantNotificationDeliveryReport) -> RefundNotificationOutcome {
    if report.success
        && !report.channels.is_empty()
        && report.channels.iter().all(|entry| entry.success)
    {
        RefundNotificationOutcome::Delivered
    } else if report
        .channels
        .iter()
        .all(|entry| matches!(entry.channel, "module" | "item" | "none"))
    {
        RefundNotificationOutcome::Skipped
    } else {
        RefundNotificationOutcome::Retry
    }
}

async fn send_refund_notification(
    state: &AppState,
    entry: &RefundStatusNotification,
) -> Result<RefundNotificationOutcome, GatewayError> {
    let Some(user_id) = entry.user_id.as_deref().filter(|id| !id.trim().is_empty()) else {
        return Ok(RefundNotificationOutcome::Skipped);
    };
    if !matches!(entry.terminal_status.as_str(), "succeeded" | "failed") {
        return Ok(RefundNotificationOutcome::Skipped);
    }
    let Some(user) = state.find_user_auth_by_id(user_id).await? else {
        return Ok(RefundNotificationOutcome::Skipped);
    };
    if user.is_deleted || !user.is_active {
        return Ok(RefundNotificationOutcome::Skipped);
    }
    let Some(email) = user
        .email
        .as_deref()
        .filter(|email| !email.trim().is_empty())
    else {
        return Ok(RefundNotificationOutcome::Skipped);
    };
    // Preserve refund mail consent. usage_alerts is a separate low-balance policy.
    if state
        .read_user_preferences(user_id)
        .await?
        .is_some_and(|prefs| !prefs.email_notifications)
    {
        return Ok(RefundNotificationOutcome::Skipped);
    }
    // Never forward raw administrative/provider diagnostics, including snapshots
    // imported by older software. Use the same safe text with or without a template.
    let failure_reason = if entry.terminal_status == "failed" {
        "退款未完成，请在账户中查看退款详情或联系管理员。"
    } else {
        ""
    };
    let mut text = format!(
        "退款编号：{}\n金额：${}\n状态：{}\n通知参考：{}",
        entry.refund_no, entry.amount_usd, entry.terminal_status, entry.id
    );
    if !failure_reason.is_empty() {
        text.push('\n');
        text.push_str(failure_reason);
    }
    let report = send_user_important_notification_email(
        state,
        USER_REFUND_STATUS_ITEM_KEY,
        email,
        ImportantNotification {
            title: "退款状态更新".into(),
            text_body: text.clone(),
            markdown_body: text,
        },
        &[
            ("refund_no", entry.refund_no.clone()),
            ("amount_usd", entry.amount_usd.clone()),
            ("status", entry.terminal_status.clone()),
            ("failure_reason", failure_reason.into()),
            ("event_id", entry.id.clone()),
        ],
    )
    .await?;
    Ok(delivery_outcome(&report))
}

async fn deliver_refund_notifications(
    state: &AppState,
) -> Result<(), aether_data_contracts::DataLayerError> {
    for _ in 0..8 {
        // Claim one immediately before delivery; no financial method is called.
        let Some(entry) = state
            .data
            .claim_refund_status_notifications(1)
            .await?
            .into_iter()
            .next()
        else {
            break;
        };
        let outcome = match tokio::time::timeout(
            Duration::from_secs(30),
            send_refund_notification(state, &entry),
        )
        .await
        {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) | Err(_) => RefundNotificationOutcome::Retry,
        };
        state
            .data
            .complete_refund_status_notification(CompleteRefundStatusNotificationInput {
                id: entry.id,
                lease_token: entry.lease_token,
                outcome,
            })
            .await?;
    }
    Ok(())
}
