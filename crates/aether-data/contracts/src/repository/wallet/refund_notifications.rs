use serde::{Deserialize, Serialize};

/// Immutable user-safe snapshot of one committed terminal refund transition.
/// Monetary value is a decimal string, preserving PostgreSQL's exact scale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundStatusNotification {
    pub id: String,
    pub refund_id: String,
    pub wallet_id: String,
    pub user_id: Option<String>,
    pub refund_no: String,
    pub amount_usd: String,
    pub terminal_status: String,
    pub failure_reason: Option<String>,
    pub lease_token: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefundNotificationOutcome {
    Delivered,
    Skipped,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteRefundStatusNotificationInput {
    pub id: String,
    pub lease_token: i64,
    pub outcome: RefundNotificationOutcome,
}
