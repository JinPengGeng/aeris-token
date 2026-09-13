use super::GatewayDataState;
use aether_billing::{BillingImageAuthorizationQuote, BillingService, BillingUsageInput};
use aether_data_contracts::repository::settlement::*;
use aether_data_contracts::DataLayerError;
use aether_usage_runtime::{
    UsageAttemptChargeEvidence, UsageAttemptFundsAction, UsageAttemptFundsEvent,
};

fn invalid(message: &str) -> DataLayerError {
    DataLayerError::InvalidInput(message.to_string())
}

impl GatewayDataState {
    pub(crate) async fn read_request_attempt_funds(
        &self,
        identity: RequestAttemptFundsIdentity,
    ) -> Result<StoredRequestAttemptFunds, DataLayerError> {
        self.settlement_writer
            .as_ref()
            .ok_or_else(|| invalid("attempt funds writer is unavailable"))?
            .read_request_attempt_funds(identity)
            .await?
            .ok_or_else(|| invalid("attempt funds reservation is unavailable"))
    }

    pub(crate) async fn reserve_request_attempt_funds(
        &self,
        input: ReserveRequestAttemptFundsInput,
    ) -> Result<ReserveRequestAttemptFundsOutcome, DataLayerError> {
        self.settlement_writer
            .as_ref()
            .ok_or_else(|| invalid("attempt funds writer is unavailable"))?
            .reserve_request_attempt_funds(input)
            .await
    }

    pub(crate) async fn dispatch_request_attempt_funds(
        &self,
        identity: RequestAttemptFundsIdentity,
    ) -> Result<StoredRequestAttemptFunds, DataLayerError> {
        self.settlement_writer
            .as_ref()
            .ok_or_else(|| invalid("attempt funds writer is unavailable"))?
            .mark_request_attempt_funds_dispatched(identity)
            .await?
            .ok_or_else(|| invalid("attempt dispatch did not persist a reservation"))
    }

    pub(crate) async fn close_request_attempt_admission(
        &self,
        identity: RequestAttemptFundsIdentity,
    ) -> Result<RequestFundsSummary, DataLayerError> {
        self.settlement_writer
            .as_ref()
            .ok_or_else(|| invalid("attempt funds writer is unavailable"))?
            .close_request_funds_admission(CloseRequestFundsAdmissionInput {
                identity,
                closed_at_unix_secs: crate::clock::current_unix_ms() / 1000,
            })
            .await?
            .ok_or_else(|| invalid("attempt admission close did not find its reservation"))
    }

    pub(crate) async fn apply_request_attempt_funds_event(
        &self,
        event: &UsageAttemptFundsEvent,
        finalized_at_unix_secs: u64,
    ) -> Result<(), DataLayerError> {
        event.validate(&event.identity.request.request_id)?;
        let repository = self
            .settlement_writer
            .as_ref()
            .ok_or_else(|| invalid("attempt funds writer is unavailable"))?;
        let stored = repository
            .read_request_attempt_funds(event.identity.clone())
            .await?
            .ok_or_else(|| invalid("attempt financial capability is not owned by a reservation"))?;
        let UsageAttemptFundsAction::Outcome {
            execution,
            evidence,
        } = &event.action
        else {
            return Ok(());
        };
        // A delayed unknown observation cannot undo an already-known charge.
        if matches!(evidence, UsageAttemptChargeEvidence::Unknown)
            && stored.terminal_facts.as_ref().is_some_and(|facts| {
                !matches!(facts.outcome, RequestAttemptFinancialOutcome::Unknown)
            })
        {
            return Ok(());
        }
        let outcome = match evidence {
            UsageAttemptChargeEvidence::Unknown => RequestAttemptFinancialOutcome::Unknown,
            UsageAttemptChargeEvidence::NoCharge { reason } => {
                if reason != "prepared_cancelled" || stored.dispatched_at_unix_secs.is_some() {
                    return Err(invalid(
                        "no-charge evidence does not prove the dispatched operation was free",
                    ));
                }
                RequestAttemptFinancialOutcome::NoCharge
            }
            UsageAttemptChargeEvidence::ImageOutput { usage } => {
                let quote: BillingImageAuthorizationQuote = serde_json::from_value(
                    stored.funds.quote.pricing_snapshot.clone(),
                )
                .map_err(|_| invalid("stored attempt quote is not a supported image quote"))?;
                if u64::try_from(quote.upper_bound_units()).ok()
                    != Some(stored.funds.quote.authorized_cost_units)
                {
                    return Err(invalid(
                        "stored attempt quote ceiling conflicts with its reservation",
                    ));
                }
                let input = BillingUsageInput {
                    task_type: "image".to_string(),
                    image_count: i64::from(usage.image_count),
                    request_count: i64::from(usage.image_count),
                    image_size: Some(usage.size.clone()),
                    image_quality: Some(usage.quality.clone()),
                    image_output_format: usage.output_format.clone(),
                    input_tokens: usage.input_tokens as i64,
                    output_tokens: usage.output_tokens as i64,
                    cache_creation_tokens: usage.cache_creation_tokens as i64,
                    cache_read_tokens: usage.cache_read_tokens as i64,
                    api_format: quote.input().api_format.clone(),
                    requested_processing_tier: quote.input().requested_processing_tier.clone(),
                    ..BillingUsageInput::new("image")
                };
                match BillingService::new()
                    .calculate_image_with_quote(&quote, &input)
                    .map_err(|_| invalid("attempt quote could not price observed output"))?
                {
                    None => RequestAttemptFinancialOutcome::Unknown,
                    Some(priced) => RequestAttemptFinancialOutcome::Charged {
                        usage: RequestAttemptBilledUsage {
                            requires_reconciliation: priced.requires_reconciliation,
                            total_cost_units: request_funds_authorized_units(
                                aether_billing::quantize_cost(priced.computation.cost_result.cost),
                            )?,
                            actual_cost_units: u64::try_from(priced.calculated_units)
                                .map_err(|_| invalid("negative calculated attempt cost"))?,
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                            cache_creation_tokens: usage.cache_creation_tokens,
                            cache_read_tokens: usage.cache_read_tokens,
                        },
                    },
                }
            }
        };
        repository
            .record_request_attempt_funds_outcome(RecordRequestAttemptFundsOutcomeInput {
                identity: event.identity.clone(),
                finalized_at_unix_secs,
                facts: RequestAttemptTerminalFacts {
                    schema_version: 1,
                    execution: execution.clone(),
                    outcome,
                    evidence: serde_json::to_value(evidence)
                        .map_err(|_| invalid("invalid attempt evidence"))?,
                },
            })
            .await?
            .ok_or_else(|| invalid("attempt outcome did not persist"))?;
        Ok(())
    }
}
