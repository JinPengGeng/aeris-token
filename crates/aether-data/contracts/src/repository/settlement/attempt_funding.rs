//! Attempt accounting keeps the public request identifier stable across retries.
use serde::{Deserialize, Serialize};

use super::{
    RequestFundsIdentity, ReserveRequestFundsInput, StoredRequestFundsReservation,
    MAX_REQUEST_FUNDS_UNITS,
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
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

impl RequestAttemptBilledUsage {
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
        }
        summary.requires_reconciliation |=
            attempt.funds.state == super::RequestFundsState::ReconciliationPending;
    }
    Ok(summary)
}
