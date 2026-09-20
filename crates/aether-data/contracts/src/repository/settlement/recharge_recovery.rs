use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredRechargeRecoveryJob {
    pub id: String,
    pub payment_order_id: String,
    pub source_transaction_id: String,
    pub wallet_id: String,
    pub user_id: Option<String>,
    pub state: String,
    pub principal_cost_units: u64,
    pub collected_cost_units: u64,
    pub outstanding_cost_units: u64,
    pub available_recharge_cost_units: u64,
    pub retry_count: u32,
    pub next_attempt_at_unix_secs: Option<u64>,
    pub error_code: Option<String>,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RechargeRecoveryNotificationAudience {
    User,
    Admin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RechargeRecoveryNotification {
    pub id: String,
    pub job_id: String,
    pub user_id: Option<String>,
    pub audience: RechargeRecoveryNotificationAudience,
    pub lease_token: i64,
    pub summary: StoredRechargeRecoveryJob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RechargeRecoveryNotificationOutcome {
    Delivered,
    /// Preferences or unavailable configuration prevented delivery; retained as
    /// skipped and retried later. It is never recorded as a successful delivery.
    Skipped,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteRechargeRecoveryNotificationInput {
    pub id: String,
    pub lease_token: i64,
    pub outcome: RechargeRecoveryNotificationOutcome,
    /// A bounded machine code, never an exception, SQL, email address or token.
    pub error_code: Option<String>,
}
