//! Server-issued financial capabilities travel separately from usage metadata.
//! Events carry observed work, never prices or amounts supplied by a reporter.
use aether_data_contracts::repository::settlement::{
    RequestAttemptExecutionFacts, RequestAttemptFundsIdentity,
};
use aether_data_contracts::DataLayerError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageAttemptImageEvidence {
    pub image_count: u32,
    pub size: String,
    pub quality: String,
    pub output_format: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UsageAttemptChargeEvidence {
    Unknown,
    /// Only an undispatched cancellation or an authoritative upstream receipt
    /// may establish this fact. A failed HTTP status alone does not establish it.
    NoCharge {
        reason: String,
    },
    ImageOutput {
        usage: UsageAttemptImageEvidence,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UsageAttemptFundsAction {
    /// Update the one public request's lifecycle, without ordinary settlement.
    ParentLifecycle,
    /// An attempt's financial fact does not finalize the public request.
    Outcome {
        execution: RequestAttemptExecutionFacts,
        evidence: UsageAttemptChargeEvidence,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageAttemptFundsEvent {
    pub schema_version: u32,
    pub identity: RequestAttemptFundsIdentity,
    pub action: UsageAttemptFundsAction,
}

impl UsageAttemptFundsEvent {
    pub fn validate(&self, request_id: &str) -> Result<(), DataLayerError> {
        self.identity.validate()?;
        if self.schema_version != 1 || self.identity.request.request_id != request_id {
            return Err(DataLayerError::InvalidInput(
                "attempt event version or request identity conflict".to_string(),
            ));
        }
        match &self.action {
            UsageAttemptFundsAction::ParentLifecycle => {}
            UsageAttemptFundsAction::Outcome {
                execution,
                evidence,
            } => {
                if execution.response_time_ms > i64::MAX as u64 {
                    return Err(DataLayerError::InvalidInput(
                        "attempt response time overflow".to_string(),
                    ));
                }
                match evidence {
                    UsageAttemptChargeEvidence::Unknown => {}
                    UsageAttemptChargeEvidence::NoCharge { reason }
                        if !reason.trim().is_empty() && reason.len() <= 128 => {}
                    UsageAttemptChargeEvidence::ImageOutput { usage }
                        if usage.image_count > 0
                            && usage.image_count <= 64
                            && !usage.size.is_empty()
                            && usage.size.len() <= 32
                            && !usage.quality.is_empty()
                            && usage.quality.len() <= 32
                            && usage.output_format.as_ref().is_none_or(|v| v.len() <= 16)
                            && [
                                usage.input_tokens,
                                usage.output_tokens,
                                usage.cache_creation_tokens,
                                usage.cache_read_tokens,
                            ]
                            .into_iter()
                            .try_fold(0u64, |total, n| total.checked_add(n))
                            .is_some_and(|total| total <= i32::MAX as u64) => {}
                    _ => {
                        return Err(DataLayerError::InvalidInput(
                            "invalid bounded attempt evidence".to_string(),
                        ))
                    }
                }
            }
        }
        Ok(())
    }

    pub fn is_outcome(&self) -> bool {
        matches!(self.action, UsageAttemptFundsAction::Outcome { .. })
    }
}
