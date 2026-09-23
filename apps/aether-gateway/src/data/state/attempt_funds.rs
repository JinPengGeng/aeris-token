use super::provider_costs::{
    provider_cost_supplier_binding, PROVIDER_COST_SUPPLIER_BINDINGS_CONFIG_KEY,
};
use super::GatewayDataState;
use aether_billing::{
    estimate_attempt_provider_cost, provider_cost_token_quantities_from_standardized_usage,
    BillingImageAuthorizationQuote, BillingService, BillingUsageInput, StandardizedUsage,
};
use aether_data_contracts::repository::provider_cost::{ProviderCostCertainty, ProviderCostPrice};
use aether_data_contracts::repository::settlement::*;
use aether_data_contracts::DataLayerError;
use aether_usage_runtime::{
    UsageAttemptChargeEvidence, UsageAttemptFundsAction, UsageAttemptFundsEvent,
};

fn invalid(message: &str) -> DataLayerError {
    DataLayerError::InvalidInput(message.to_string())
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
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
                    Some(priced) => {
                        let provider_cost = self
                            .attempt_provider_cost_snapshot(
                                &stored,
                                usage,
                                quote.input().api_format.as_deref(),
                            )
                            .await?;
                        RequestAttemptFinancialOutcome::Charged {
                            usage: RequestAttemptBilledUsage {
                                requires_reconciliation: priced.requires_reconciliation,
                                total_cost_units: request_funds_authorized_units(
                                    aether_billing::quantize_cost(
                                        priced.computation.cost_result.cost,
                                    )
                                    .map_err(|_| invalid("attempt cost is not finite"))?,
                                )?,
                                actual_cost_units: u64::try_from(priced.calculated_units)
                                    .map_err(|_| invalid("negative calculated attempt cost"))?,
                                input_tokens: usage.input_tokens,
                                output_tokens: usage.output_tokens,
                                cache_creation_tokens: usage.cache_creation_tokens,
                                cache_read_tokens: usage.cache_read_tokens,
                                provider_cost,
                            },
                        }
                    }
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

    /// Freezes the provider-cost basis at the reservation's admission time and
    /// settles with it: catalog price changes after reserve never rewrite a
    /// settled attempt. Without a supplier binding or with untrusted usage the
    /// snapshot is left absent or unknown instead of inventing a zero cost.
    async fn attempt_provider_cost_snapshot(
        &self,
        stored: &StoredRequestAttemptFunds,
        usage: &aether_usage_runtime::UsageAttemptImageEvidence,
        api_format: Option<&str>,
    ) -> Result<Option<RequestAttemptCostSnapshot>, DataLayerError> {
        if !self.has_provider_cost_data_backend() {
            return Ok(None);
        }
        let Some(provider_id) = non_empty(Some(stored.provider.provider_id.as_str())) else {
            return Ok(None);
        };
        let Some(model) = stored
            .provider
            .model_id
            .as_deref()
            .and_then(|value| non_empty(Some(value)))
        else {
            return Ok(None);
        };
        let binding_value = self
            .find_system_config_value(PROVIDER_COST_SUPPLIER_BINDINGS_CONFIG_KEY)
            .await?;
        let Some(binding) = provider_cost_supplier_binding(binding_value.as_ref(), provider_id)
        else {
            return Ok(None);
        };
        let frozen_at_unix_secs = stored.funds.quote.admitted_at_unix_secs;
        let standardized = StandardizedUsage {
            input_tokens: i64::try_from(usage.input_tokens).unwrap_or(i64::MAX),
            output_tokens: i64::try_from(usage.output_tokens).unwrap_or(i64::MAX),
            cache_creation_tokens: i64::try_from(usage.cache_creation_tokens).unwrap_or(i64::MAX),
            cache_read_tokens: i64::try_from(usage.cache_read_tokens).unwrap_or(i64::MAX),
            ..StandardizedUsage::default()
        };
        let quantities = provider_cost_token_quantities_from_standardized_usage(
            api_format,
            &standardized,
            binding.input_price_mode,
        );
        // A pure image attempt with zero observed tokens must not require token prices.
        let quantities = quantities.filter(|quantities| {
            quantities.input + quantities.output + quantities.cache_write + quantities.cache_read
                > 0
                || usage.image_count == 0
        });

        let Some(prices) = self
            .attempt_frozen_cost_prices(
                &binding.supplier,
                provider_id,
                model,
                quantities.as_ref(),
                u64::from(usage.image_count),
                &binding.currency,
                frozen_at_unix_secs,
            )
            .await?
        else {
            return Ok(Some(unknown_attempt_cost_snapshot(
                "attempt_funds_cost:missing_price",
            )));
        };
        Ok(Some(build_attempt_cost_snapshot(
            estimate_attempt_provider_cost(
                &prices,
                &binding.supplier,
                provider_id,
                model,
                quantities.as_ref(),
                u64::from(usage.image_count),
                &binding.currency,
                frozen_at_unix_secs,
            ),
        )))
    }

    /// Gathers the price book rows that can price the frozen usage. Returns
    /// `None` when a required dimension has no price effective at the frozen time.
    #[allow(clippy::too_many_arguments)]
    async fn attempt_frozen_cost_prices(
        &self,
        supplier: &str,
        provider: &str,
        model: &str,
        quantities: Option<&aether_billing::ProviderCostTokenQuantities>,
        image_count: u64,
        currency: &str,
        frozen_at_unix_secs: u64,
    ) -> Result<Option<Vec<ProviderCostPrice>>, DataLayerError> {
        use aether_data_contracts::repository::provider_cost::{
            ProviderCostDimension, ProviderCostUnit,
        };
        let mut wanted = Vec::with_capacity(5);
        if let Some(quantities) = quantities {
            wanted.extend([
                (
                    ProviderCostDimension::Input,
                    ProviderCostUnit::PerMillionTokens,
                ),
                (
                    ProviderCostDimension::Output,
                    ProviderCostUnit::PerMillionTokens,
                ),
            ]);
            if quantities.cache_write > 0 {
                wanted.push((
                    ProviderCostDimension::CacheWrite,
                    ProviderCostUnit::PerMillionTokens,
                ));
            }
            if quantities.cache_read > 0 {
                wanted.push((
                    ProviderCostDimension::CacheRead,
                    ProviderCostUnit::PerMillionTokens,
                ));
            }
        }
        if image_count > 0 {
            wanted.push((ProviderCostDimension::Image, ProviderCostUnit::PerImage));
        }
        if wanted.is_empty() {
            return Ok(None);
        }
        let mut prices = Vec::with_capacity(wanted.len());
        for (dimension, unit) in wanted {
            let Some(price) = self
                .find_effective_provider_cost_price(
                    supplier,
                    provider,
                    model,
                    dimension,
                    currency,
                    unit,
                    frozen_at_unix_secs,
                )
                .await?
            else {
                return Ok(None);
            };
            prices.push(price);
        }
        Ok(Some(prices))
    }
}

fn unknown_attempt_cost_snapshot(source_reference: &str) -> RequestAttemptCostSnapshot {
    RequestAttemptCostSnapshot {
        certainty: Some(ProviderCostCertainty::Unknown),
        amount_units: None,
        currency: None,
        price_version: None,
        source_reference: Some(source_reference.to_string()),
    }
}

/// Maps the frozen estimate onto the stored snapshot; a failed estimate keeps
/// the cost unknown instead of encoding a zero-cost completion.
fn build_attempt_cost_snapshot(
    estimate: Result<
        Option<aether_billing::ProviderCostRequestEstimate>,
        aether_billing::ProviderCostEstimateError,
    >,
) -> RequestAttemptCostSnapshot {
    match estimate {
        Ok(Some(estimate)) => {
            let mut versions: Vec<&str> = estimate
                .components
                .iter()
                .map(|component| component.price_version.as_str())
                .collect();
            versions.sort_unstable();
            versions.dedup();
            RequestAttemptCostSnapshot {
                certainty: Some(ProviderCostCertainty::Estimated),
                amount_units: Some(estimate.amount_units),
                currency: Some(estimate.currency),
                price_version: (!versions.is_empty()).then(|| versions.join(",")),
                source_reference: Some("frozen_attempt_cost_estimate".to_string()),
            }
        }
        _ => unknown_attempt_cost_snapshot("attempt_funds_cost:unsupported_price_book"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_billing::ProviderCostUnit;

    fn price(
        dimension: aether_billing::ProviderCostDimension,
        unit: ProviderCostUnit,
        version: &str,
        from: u64,
        to: Option<u64>,
        price_units: u64,
    ) -> ProviderCostPrice {
        ProviderCostPrice {
            import_id: format!("import-{version}"),
            supplier: "supplier-a".to_string(),
            provider: "provider-real-id".to_string(),
            model: "supplier-model".to_string(),
            dimension,
            currency: "USD".to_string(),
            unit,
            version: version.to_string(),
            price_units,
            effective_from_unix_secs: from,
            effective_to_unix_secs: to,
            source_reference: format!("price-book-{version}"),
            imported_by: "admin-1".to_string(),
        }
    }

    fn quantities(
        input: u64,
        output: u64,
        cache_write: u64,
        cache_read: u64,
    ) -> aether_billing::ProviderCostTokenQuantities {
        aether_billing::ProviderCostTokenQuantities {
            input,
            output,
            cache_write,
            cache_read,
        }
    }

    #[test]
    fn frozen_attempt_cost_snapshot_survives_price_changes() {
        // Reserve was admitted while v1 was effective; a v2 price taking effect
        // afterwards must not rewrite the settled cost basis.
        let prices = [
            price(
                aether_billing::ProviderCostDimension::Input,
                ProviderCostUnit::PerMillionTokens,
                "v1",
                0,
                Some(200),
                1_000_000,
            ),
            price(
                aether_billing::ProviderCostDimension::Input,
                ProviderCostUnit::PerMillionTokens,
                "v2",
                200,
                None,
                9_000_000,
            ),
            price(
                aether_billing::ProviderCostDimension::Output,
                ProviderCostUnit::PerMillionTokens,
                "out-v1",
                0,
                None,
                1_000_000,
            ),
        ];
        let quantities = quantities(2_000_000, 0, 0, 0);
        let snapshot = build_attempt_cost_snapshot(estimate_attempt_provider_cost(
            &prices,
            "supplier-a",
            "provider-real-id",
            "supplier-model",
            Some(&quantities),
            0,
            "USD",
            100,
        ));
        assert_eq!(snapshot.certainty, Some(ProviderCostCertainty::Estimated));
        assert_eq!(snapshot.amount_units, Some(2_000_000));
        assert_eq!(snapshot.price_version.as_deref(), Some("out-v1,v1"));
        assert_eq!(snapshot.currency.as_deref(), Some("USD"));
    }

    #[test]
    fn missing_frozen_price_stays_unknown_instead_of_zero() {
        let prices = [price(
            aether_billing::ProviderCostDimension::Input,
            ProviderCostUnit::PerMillionTokens,
            "v2",
            200,
            None,
            9_000_000,
        )];
        let quantities = quantities(1, 0, 0, 0);
        let snapshot = build_attempt_cost_snapshot(estimate_attempt_provider_cost(
            &prices,
            "supplier-a",
            "provider-real-id",
            "supplier-model",
            Some(&quantities),
            0,
            "USD",
            100,
        ));
        assert_eq!(snapshot.certainty, Some(ProviderCostCertainty::Unknown));
        assert_eq!(snapshot.amount_units, None);
    }

    #[test]
    fn image_attempt_cost_adds_per_image_component_to_token_cost() {
        let prices = [
            price(
                aether_billing::ProviderCostDimension::Input,
                ProviderCostUnit::PerMillionTokens,
                "input-v1",
                0,
                None,
                1_000_000,
            ),
            price(
                aether_billing::ProviderCostDimension::Image,
                ProviderCostUnit::PerImage,
                "image-v1",
                0,
                None,
                40,
            ),
            price(
                aether_billing::ProviderCostDimension::Output,
                ProviderCostUnit::PerMillionTokens,
                "output-v1",
                0,
                None,
                1_000_000,
            ),
        ];
        let quantities = quantities(1_000_000, 0, 0, 0);
        let snapshot = build_attempt_cost_snapshot(estimate_attempt_provider_cost(
            &prices,
            "supplier-a",
            "provider-real-id",
            "supplier-model",
            Some(&quantities),
            3,
            "USD",
            50,
        ));
        assert_eq!(snapshot.amount_units, Some(1_000_000 + 120));
        assert_eq!(
            snapshot.price_version.as_deref(),
            Some("image-v1,input-v1,output-v1")
        );
    }
}
