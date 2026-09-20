use std::time::Duration;

use aether_data::repository::audit::AdminAuditDeliveryFailureCode;

use crate::AppState;

const INTERVAL: Duration = Duration::from_secs(2);
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);
const LEASE_SECONDS: u64 = 30;
const MAX_PER_TICK: usize = 32;

pub(crate) fn spawn_admin_audit_delivery_worker(
    app: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !app.data.has_admin_audit_delivery_backend() {
        return None;
    }
    Some(crate::task_runtime::spawn_singleton_worker(
        app,
        crate::task_runtime::TASK_KEY_ADMIN_AUDIT_DELIVERY,
        |app| async move {
            let mut interval = tokio::time::interval(INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                for _ in 0..MAX_PER_TICK {
                    let claimed = match app
                        .data
                        .claim_admin_audit_deliveries(1, LEASE_SECONDS)
                        .await
                    {
                        Ok(claimed) => claimed,
                        Err(_) => {
                            tracing::warn!(
                                event_name = "admin_audit_delivery_claim_failed",
                                "durable admin audit claim failed"
                            );
                            break;
                        }
                    };
                    let Some(item) = claimed.into_iter().next() else {
                        break;
                    };
                    let result = tokio::time::timeout(
                        DELIVERY_TIMEOUT,
                        app.data
                            .deliver_admin_audit(&item.event_id, item.lease_token),
                    )
                    .await;
                    let failure_code = match result {
                        Ok(Ok(true)) => continue,
                        Ok(Ok(false)) => {
                            tracing::warn!(
                                event_name = "admin_audit_delivery_stale_lease",
                                audit_event_id = %item.event_id,
                                "durable admin audit lease was superseded"
                            );
                            continue;
                        }
                        Ok(Err(aether_data::DataLayerError::InvalidInput(_))) => {
                            AdminAuditDeliveryFailureCode::InvalidPayload
                        }
                        Ok(Err(_)) => AdminAuditDeliveryFailureCode::AuditInsertFailed,
                        Err(_) => AdminAuditDeliveryFailureCode::DeliveryTimedOut,
                    };
                    if app
                        .data
                        .fail_admin_audit_delivery(&item.event_id, item.lease_token, failure_code)
                        .await
                        .is_err()
                    {
                        tracing::warn!(
                            event_name = "admin_audit_delivery_retry_schedule_failed",
                            audit_event_id = %item.event_id,
                            "durable admin audit retry state update failed"
                        );
                    }
                }
            }
        },
    ))
}
