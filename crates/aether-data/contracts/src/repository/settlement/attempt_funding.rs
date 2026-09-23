//! Attempt accounting keeps the public request identifier stable across retries.
use serde::{Deserialize, Serialize};

use crate::repository::provider_cost::ProviderCostCertainty;

use super::{
    RequestFundsIdentity, ReserveRequestFundsInput, ReserveUsagePolicyCostInput,
    StoredRequestFundsReservation, UsagePolicyCostWindow, MAX_REQUEST_FUNDS_UNITS,
};
use crate::DataLayerError;

fn invalid(message: &str) -> DataLayerError {
    DataLayerError::InvalidInput(message.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestAttemptFundsIdentity {
    pub request: RequestFundsIdentity,
    pub attempt_id: String,
}

impl RequestAttemptFundsIdentity {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        self.request.validate()?;
        let parsed = uuid::Uuid::parse_str(&self.attempt_id)
            .map_err(|_| invalid("attempt funds identity requires a UUID"))?;
        if parsed.hyphenated().to_string() != self.attempt_id {
            return Err(invalid(
                "attempt funds UUID must use canonical lowercase format",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestAttemptProvider {
    pub provider_id: String,
    pub provider_api_key_id: Option<String>,
    pub model_id: Option<String>,
    pub candidate_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReserveRequestAttemptFundsInput {
    pub attempt_id: String,
    pub provider: RequestAttemptProvider,
    pub quote: ReserveRequestFundsInput,
    /// Frozen server admission policy. Absence is also frozen for this request.
    #[serde(default)]
    pub usage_policy: Option<RequestFundsUsagePolicy>,
}

/// Hard usage-policy cost is independent of wallet and entitlement funding.
/// The context token identifies the original server admission, while each child
/// quota reservation uses its own attempt funds token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFundsUsagePolicy {
    pub subject_id: String,
    pub reservation_token: String,
    pub admitted_at_unix_secs: u64,
    pub retain_until_unix_secs: u64,
    pub windows: Vec<UsagePolicyCostWindow>,
}

impl RequestFundsUsagePolicy {
    pub fn cost_reservation(
        &self,
        quote: &ReserveRequestFundsInput,
    ) -> ReserveUsagePolicyCostInput {
        ReserveUsagePolicyCostInput {
            request_id: quote.identity.request_id.clone(),
            subject_id: self.subject_id.clone(),
            reservation_token: quote.identity.reservation_token.clone(),
            admitted_at_unix_secs: self.admitted_at_unix_secs,
            reserved_cost_units: quote.authorized_cost_units,
            // Linked unresolved attempts do not expire. The field preserves the
            // legacy shape; their explicit linkage owns accounting and retention.
            reservation_expires_at_unix_secs: self.retain_until_unix_secs,
            retain_until_unix_secs: self.retain_until_unix_secs,
            windows: self.windows.clone(),
        }
    }

    fn validate(&self, quote: &ReserveRequestFundsInput) -> Result<(), DataLayerError> {
        if quote.identity.api_key_is_standalone
            || quote.identity.user_id.as_deref() != Some(self.subject_id.as_str())
            || self.reservation_token.trim().is_empty()
            || self.reservation_token.len() > 128
            || self.reservation_token == quote.identity.reservation_token
            || self.admitted_at_unix_secs > quote.admitted_at_unix_secs
            || self.retain_until_unix_secs > i64::MAX as u64
            || self.windows.iter().any(|window| {
                window.ends_at_unix_secs > self.retain_until_unix_secs
                    || window.ends_at_unix_secs > i64::MAX as u64
            })
        {
            return Err(invalid(
                "attempt usage policy has an invalid owner, context or window",
            ));
        }
        if serde_json::to_vec(self)
            .map_err(|_| invalid("attempt usage policy cannot be serialized"))?
            .len()
            > 32_768
        {
            return Err(invalid("attempt usage policy exceeds the storage bound"));
        }
        self.cost_reservation(quote).validate()
    }
}

impl ReserveRequestAttemptFundsInput {
    pub fn identity(&self) -> RequestAttemptFundsIdentity {
        RequestAttemptFundsIdentity {
            request: self.quote.identity.clone(),
            attempt_id: self.attempt_id.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), DataLayerError> {
        self.identity().validate()?;
        self.quote.validate()?;
        if let Some(policy) = &self.usage_policy {
            policy.validate(&self.quote)?;
        }
        for value in [
            Some(self.provider.provider_id.as_str()),
            self.provider.provider_api_key_id.as_deref(),
            self.provider.model_id.as_deref(),
            self.provider.candidate_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if value.trim().is_empty() || value.len() > 128 {
                return Err(invalid(
                    "attempt provider identity must contain 1 to 128 bytes",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestAttemptExecutionStatus {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestAttemptExecutionFacts {
    pub status: RequestAttemptExecutionStatus,
    pub response_time_ms: u64,
}

/// Pricing has already been calculated from the stored quote. Monetary fields
/// are integer units; actual includes both frozen provider and customer multipliers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestAttemptBilledUsage {
    pub total_cost_units: u64,
    pub actual_cost_units: u64,
    /// The observation differs from the frozen quote, even when the price fits
    /// its authorization. Older stored facts without the flag retain their meaning.
    #[serde(default)]
    pub requires_reconciliation: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    /// Frozen provider-cost snapshot taken at outcome settlement. Absence means
    /// no cost basis was configured; older stored records never rewrite it.
    #[serde(default)]
    pub provider_cost: Option<RequestAttemptCostSnapshot>,
}

/// Provider cost frozen at the reservation's admission time. Later catalog
/// price changes must not rewrite an already settled attempt's cost basis.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestAttemptCostSnapshot {
    #[serde(default)]
    pub certainty: Option<ProviderCostCertainty>,
    #[serde(default)]
    pub amount_units: Option<u64>,
    #[serde(default)]
    pub currency: Option<String>,
    /// Effective price version(s) used for the estimate, joined when several
    /// dimensions resolved to different catalog versions.
    #[serde(default)]
    pub price_version: Option<String>,
    #[serde(default)]
    pub source_reference: Option<String>,
}

impl RequestAttemptBilledUsage {
    pub fn reconciliation_facts(&self, authorized_cost_units: u64) -> Option<serde_json::Value> {
        let excess_units = self.actual_cost_units.saturating_sub(authorized_cost_units);
        (self.requires_reconciliation || excess_units > 0).then(|| {
            serde_json::json!({
                "reason": if excess_units > 0 { "authorization_exceeded" } else { "quote_mismatch" },
                "excess_units": excess_units,
                "quote_requires_reconciliation": self.requires_reconciliation,
            })
        })
    }

    pub fn total_tokens(&self) -> Option<u64> {
        self.input_tokens
            .checked_add(self.output_tokens)?
            .checked_add(self.cache_creation_tokens)?
            .checked_add(self.cache_read_tokens)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestAttemptFinancialOutcome {
    Unknown,
    NoCharge,
    Charged { usage: RequestAttemptBilledUsage },
}

/// No raw provider body, request body or credentials belong in `evidence`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestAttemptTerminalFacts {
    pub schema_version: u32,
    pub execution: RequestAttemptExecutionFacts,
    pub outcome: RequestAttemptFinancialOutcome,
    pub evidence: serde_json::Value,
}

impl RequestAttemptTerminalFacts {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        if self.schema_version != 1
            || self.execution.response_time_ms > i64::MAX as u64
            || !self.evidence.is_object()
            || self.evidence.to_string().len() > 16 * 1024
        {
            return Err(invalid("invalid versioned attempt terminal facts"));
        }
        if let RequestAttemptFinancialOutcome::Charged { usage } = &self.outcome {
            if usage.total_cost_units > MAX_REQUEST_FUNDS_UNITS
                || usage.actual_cost_units > MAX_REQUEST_FUNDS_UNITS
                || usage
                    .total_tokens()
                    .is_none_or(|tokens| tokens > i32::MAX as u64)
                || usage
                    .provider_cost
                    .as_ref()
                    .and_then(|cost| cost.amount_units)
                    .is_some_and(|units| units > i64::MAX as u64)
            {
                return Err(invalid("attempt billing facts exceed supported range"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordRequestAttemptFundsOutcomeInput {
    pub identity: RequestAttemptFundsIdentity,
    pub facts: RequestAttemptTerminalFacts,
    pub finalized_at_unix_secs: u64,
}

impl RecordRequestAttemptFundsOutcomeInput {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        self.identity.validate()?;
        self.facts.validate()?;
        if self.finalized_at_unix_secs > i64::MAX as u64 {
            return Err(invalid("attempt terminal timestamp overflow"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredRequestAttemptFunds {
    pub attempt_id: String,
    pub provider: RequestAttemptProvider,
    pub funds: StoredRequestFundsReservation,
    pub dispatched_at_unix_secs: Option<u64>,
    pub terminal_facts: Option<RequestAttemptTerminalFacts>,
    #[serde(default)]
    pub usage_policy: Option<RequestFundsUsagePolicy>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReserveRequestAttemptFundsOutcome {
    Reserved {
        reservation: Box<StoredRequestAttemptFunds>,
    },
    Insufficient {
        available_cost_units: u64,
    },
    UsagePolicyRejected {
        window_index: usize,
        limit_cost_units: u64,
        used_cost_units: u64,
    },
    WalletUnavailable,
    Conflict,
    AdmissionClosed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFundsSummary {
    pub schema_version: u32,
    pub attempt_count: u64,
    pub unknown_attempts: u64,
    pub prepared_attempts: u64,
    pub known_total_cost_units: u64,
    pub known_actual_cost_units: u64,
    pub collected_cost_units: u64,
    pub held_cost_units: u64,
    pub admission_closed: bool,
    /// First durable logical completion time, retained for late financial outcomes.
    #[serde(default)]
    pub admission_closed_at_unix_secs: Option<u64>,
    pub requires_reconciliation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseRequestFundsAdmissionInput {
    /// A token owned by this request is required; a public request ID is insufficient.
    pub identity: RequestAttemptFundsIdentity,
    pub closed_at_unix_secs: u64,
}

/// Shared arithmetic for adapters; unresolved observations never become zero-cost completion.
pub fn summarize_request_attempt_funds<'a>(
    attempts: impl IntoIterator<Item = &'a StoredRequestAttemptFunds>,
    admission_closed: bool,
) -> Result<RequestFundsSummary, DataLayerError> {
    fn add(target: &mut u64, amount: u64) -> Result<(), DataLayerError> {
        *target = target
            .checked_add(amount)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or_else(|| invalid("attempt aggregate overflow"))?;
        Ok(())
    }
    let mut summary = RequestFundsSummary {
        schema_version: 1,
        admission_closed,
        ..Default::default()
    };
    for attempt in attempts {
        add(&mut summary.attempt_count, 1)?;
        add(
            &mut summary.collected_cost_units,
            attempt.funds.collected_cost_units,
        )?;
        if attempt.funds.settlement.is_none() && attempt.funds.state.holds_funds() {
            add(
                &mut summary.held_cost_units,
                attempt.funds.quote.authorized_cost_units,
            )?;
            if attempt.dispatched_at_unix_secs.is_some() {
                add(&mut summary.unknown_attempts, 1)?;
            } else {
                add(&mut summary.prepared_attempts, 1)?;
            }
        }
        if let Some(RequestAttemptTerminalFacts {
            outcome: RequestAttemptFinancialOutcome::Charged { usage },
            ..
        }) = &attempt.terminal_facts
        {
            add(&mut summary.known_total_cost_units, usage.total_cost_units)?;
            add(
                &mut summary.known_actual_cost_units,
                usage.actual_cost_units,
            )?;
            summary.requires_reconciliation |= usage.requires_reconciliation;
        }
        summary.requires_reconciliation |=
            attempt.funds.state == super::RequestFundsState::ReconciliationPending;
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ReserveRequestAttemptFundsInput {
        ReserveRequestAttemptFundsInput {
            attempt_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            provider: RequestAttemptProvider {
                provider_id: "provider".into(),
                provider_api_key_id: None,
                model_id: None,
                candidate_id: None,
            },
            quote: ReserveRequestFundsInput {
                identity: RequestFundsIdentity {
                    reservation_token: "child-token".into(),
                    request_id: "request".into(),
                    user_id: Some("owner".into()),
                    api_key_id: Some("key".into()),
                    api_key_is_standalone: false,
                },
                authorized_cost_units: 8_000_000,
                pricing_snapshot: serde_json::json!({"version":1}),
                admitted_at_unix_secs: 101,
            },
            usage_policy: Some(RequestFundsUsagePolicy {
                subject_id: "owner".into(),
                reservation_token: "parent-context".into(),
                admitted_at_unix_secs: 100,
                retain_until_unix_secs: 200,
                windows: vec![UsagePolicyCostWindow {
                    window_id: "day".into(),
                    starts_at_unix_secs: 0,
                    ends_at_unix_secs: 200,
                    limit_cost_units: 10_000_000,
                }],
            }),
        }
    }

    #[test]
    fn legacy_billed_usage_without_provider_cost_still_deserializes() {
        let legacy = serde_json::json!({
            "total_cost_units": 5,
            "actual_cost_units": 5,
            "input_tokens": 1,
            "output_tokens": 2,
            "cache_creation_tokens": 0,
            "cache_read_tokens": 0,
        });
        let usage: RequestAttemptBilledUsage = serde_json::from_value(legacy).unwrap();
        assert_eq!(usage.provider_cost, None);
    }

    #[test]
    fn billed_usage_cost_snapshot_round_trips_with_frozen_provenance() {
        let mut usage = RequestAttemptBilledUsage {
            total_cost_units: 5,
            actual_cost_units: 5,
            input_tokens: 1,
            output_tokens: 2,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            ..Default::default()
        };
        usage.provider_cost = Some(RequestAttemptCostSnapshot {
            certainty: Some(ProviderCostCertainty::Estimated),
            amount_units: Some(3),
            currency: Some("USD".into()),
            price_version: Some("2026-09".into()),
            source_reference: Some("frozen-attempt-cost".into()),
        });
        let decoded: RequestAttemptBilledUsage =
            serde_json::from_value(serde_json::to_value(&usage).unwrap()).unwrap();
        assert_eq!(decoded, usage);
    }

    #[test]
    fn attempt_policy_capability_preserves_child_token_and_original_day() {
        let input = input();
        input.validate().unwrap();
        let quota = input
            .usage_policy
            .as_ref()
            .unwrap()
            .cost_reservation(&input.quote);
        assert_eq!(quota.reservation_token, "child-token");
        assert_eq!(quota.admitted_at_unix_secs, 100);
        assert_eq!(quota.reserved_cost_units, 8_000_000);
        let mut legacy = serde_json::to_value(&input).unwrap();
        legacy.as_object_mut().unwrap().remove("usage_policy");
        let legacy: ReserveRequestAttemptFundsInput = serde_json::from_value(legacy).unwrap();
        assert!(legacy.usage_policy.is_none());
        legacy.validate().unwrap();
    }

    #[test]
    fn attempt_policy_rejects_foreign_owner_reused_context_and_truncated_retention() {
        for change in 0..7 {
            let mut input = input();
            let policy = input.usage_policy.as_mut().unwrap();
            match change {
                0 => policy.subject_id = "other".into(),
                1 => policy.reservation_token = "child-token".into(),
                2 => policy.admitted_at_unix_secs = 102,
                3 => policy.retain_until_unix_secs = 199,
                4 => policy.windows.clear(),
                5 => policy.windows[0].limit_cost_units = 0,
                _ => input.quote.identity.api_key_is_standalone = true,
            }
            assert!(
                input.validate().is_err(),
                "mutation {change} must not be admitted"
            );
        }
    }
}
