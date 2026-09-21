use super::StoredRequestUsageAudit;
use crate::{repository::settlement::RequestFundsSummary, DataLayerError};

/// Read both scopes from one committed database snapshot. The window is [start, end).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyActualCostQuery {
    pub user_id: Option<String>,
    pub api_key_id: String,
    pub start_unix_secs: u64,
    pub end_unix_secs: u64,
}

impl DailyActualCostQuery {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        if self.api_key_id.trim().is_empty()
            || self.start_unix_secs >= self.end_unix_secs
            || self.end_unix_secs > i64::MAX as u64
        {
            return Err(invalid("invalid daily actual cost scope or window"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DailyActualCostCounts {
    pub user_units: u64,
    pub key_units: u64,
}

pub const INSUFFICIENT_QUOTA_WRITEOFF_MAX_LIMIT: usize = 1000;

/// Read-only daily writeoff report for usage finalized as `insufficient_quota`
/// (delivered service recorded at no charge). The window is [start, end).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsufficientQuotaWriteoffQuery {
    pub finalized_from_unix_secs: u64,
    pub finalized_until_unix_secs: u64,
    pub limit: usize,
}

impl InsufficientQuotaWriteoffQuery {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        if self.finalized_from_unix_secs >= self.finalized_until_unix_secs
            || self.finalized_until_unix_secs > i64::MAX as u64
            || self.limit == 0
            || self.limit > INSUFFICIENT_QUOTA_WRITEOFF_MAX_LIMIT
        {
            return Err(invalid(
                "invalid insufficient quota writeoff window or limit",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredInsufficientQuotaWriteoff {
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub provider_id: Option<String>,
    pub model: String,
    pub total_cost_usd: f64,
    pub actual_total_cost_usd: f64,
    pub finalized_at_unix_secs: Option<u64>,
}

/// Independent of audit retention. A deleted audit must not make its request ID reusable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyCostContribution {
    pub request_id: String,
    pub usage_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub api_key_is_standalone: bool,
    pub attempt_funds: bool,
    pub actual_cost_units: u64,
    pub accounting_at_unix_secs: Option<u64>,
}

fn invalid(message: &str) -> DataLayerError {
    DataLayerError::InvalidInput(message.to_string())
}

/// Legacy input uses the existing nearest 1e-8 USD precision, without silent clamps.
pub fn daily_actual_cost_units(usd: f64) -> Result<u64, DataLayerError> {
    let units = (usd * 100_000_000.0).round();
    if !usd.is_finite() || usd < 0.0 || units >= i64::MAX as f64 {
        return Err(invalid(
            "daily actual cost is negative, non-finite or overflowing",
        ));
    }
    Ok(units as u64)
}

/// Called under the parent write lock, after accepting a lifecycle or financial transition.
/// Attempt amounts always come from integer financial facts, never parent display floats.
pub fn next_daily_cost_contribution(
    previous: Option<&DailyCostContribution>,
    parent: &StoredRequestUsageAudit,
    funds: Option<&RequestFundsSummary>,
) -> Result<DailyCostContribution, DataLayerError> {
    let standalone = parent
        .request_metadata
        .as_ref()
        .and_then(|m| m.get("api_key_is_standalone"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut next = previous.cloned().unwrap_or_else(|| DailyCostContribution {
        request_id: parent.request_id.clone(),
        usage_id: parent.id.clone(),
        user_id: parent.user_id.clone(),
        api_key_id: parent.api_key_id.clone(),
        api_key_is_standalone: standalone,
        attempt_funds: false,
        actual_cost_units: 0,
        accounting_at_unix_secs: None,
    });
    if next.request_id != parent.request_id || next.usage_id != parent.id {
        return Err(invalid(
            "daily cost request identity was already used by another audit",
        ));
    }
    if let Some(funds) = funds {
        if !next.attempt_funds
            && (next.accounting_at_unix_secs.is_some() || next.actual_cost_units != 0)
        {
            return Err(invalid(
                "cannot convert an accounted legacy request to attempt funds",
            ));
        }
        if funds.known_actual_cost_units > i64::MAX as u64 {
            return Err(invalid("daily attempt cost overflow"));
        }
        next.attempt_funds = true;
        next.actual_cost_units = funds.known_actual_cost_units;
        if funds.admission_closed && next.accounting_at_unix_secs.is_none() {
            next.accounting_at_unix_secs = Some(
                funds
                    .admission_closed_at_unix_secs
                    .ok_or_else(|| invalid("closed attempt request lacks its accounting time"))?,
            );
        }
    } else if !next.attempt_funds && parent.status == "completed" {
        next.actual_cost_units = daily_actual_cost_units(parent.actual_total_cost_usd)?;
        next.accounting_at_unix_secs.get_or_insert(
            parent
                .finalized_at_unix_secs
                .unwrap_or(parent.updated_at_unix_secs),
        );
    }
    if next
        .accounting_at_unix_secs
        .is_some_and(|at| at > i64::MAX as u64)
    {
        return Err(invalid("daily cost accounting time overflow"));
    }
    Ok(next)
}
