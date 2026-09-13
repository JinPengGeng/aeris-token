use super::{funding, InMemorySettlementRepository};
use crate::DataLayerError;
use aether_data_contracts::repository::settlement::*;
use aether_data_contracts::repository::usage::{
    ProviderApiKeyUsageContribution, StoredRequestUsageAudit,
};

fn invalid(message: &str) -> DataLayerError {
    DataLayerError::InvalidInput(message.to_string())
}

fn parent_matches(
    parent: &StoredRequestUsageAudit,
    identity: &RequestFundsIdentity,
) -> Result<(), DataLayerError> {
    let standalone = parent
        .request_metadata
        .as_ref()
        .and_then(|m| m.get("api_key_is_standalone"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if parent.user_id != identity.user_id
        || parent.api_key_id != identity.api_key_id
        || standalone != identity.api_key_is_standalone
    {
        return Err(invalid("attempt funds parent owner conflict"));
    }
    Ok(())
}

fn get(
    repo: &InMemorySettlementRepository,
    identity: &RequestAttemptFundsIdentity,
) -> Result<Option<StoredRequestAttemptFunds>, DataLayerError> {
    let mut stored = repo
        .attempt_metadata
        .read()
        .expect("attempt metadata lock")
        .get(&identity.request.reservation_token)
        .cloned();
    if let Some(stored) = &mut stored {
        if stored.attempt_id != identity.attempt_id
            || stored.funds.quote.identity != identity.request
        {
            return Err(invalid("attempt funds identity conflict"));
        }
        stored.funds = repo
            .funds
            .read()
            .expect("funds lock")
            .get(&identity.request.reservation_token)
            .cloned()
            .ok_or_else(|| invalid("attempt funds missing"))?;
    }
    Ok(stored)
}

fn all(repo: &InMemorySettlementRepository, request: &str) -> Vec<StoredRequestAttemptFunds> {
    let metadata = repo.attempt_metadata.read().expect("attempt metadata lock");
    let funds = repo.funds.read().expect("funds lock");
    metadata
        .values()
        .filter(|a| a.funds.quote.identity.request_id == request)
        .map(|a| {
            let mut a = a.clone();
            if let Some(funds) = funds.get(&a.funds.quote.identity.reservation_token) {
                a.funds = funds.clone();
            }
            a
        })
        .collect()
}

fn apply_summary(
    parent: &mut StoredRequestUsageAudit,
    summary: &mut Option<RequestFundsSummary>,
    attempts: &[StoredRequestAttemptFunds],
) -> Result<(), DataLayerError> {
    let closed = summary.as_ref().is_some_and(|s| s.admission_closed);
    let next = summarize_request_attempt_funds(attempts, closed)?;
    let mut tokens = RequestAttemptBilledUsage::default();
    for a in attempts {
        if let Some(RequestAttemptTerminalFacts {
            outcome: RequestAttemptFinancialOutcome::Charged { usage },
            ..
        }) = &a.terminal_facts
        {
            tokens.input_tokens = tokens
                .input_tokens
                .checked_add(usage.input_tokens)
                .ok_or_else(|| invalid("aggregate token overflow"))?;
            tokens.output_tokens = tokens
                .output_tokens
                .checked_add(usage.output_tokens)
                .ok_or_else(|| invalid("aggregate token overflow"))?;
            tokens.cache_creation_tokens = tokens
                .cache_creation_tokens
                .checked_add(usage.cache_creation_tokens)
                .ok_or_else(|| invalid("aggregate token overflow"))?;
            tokens.cache_read_tokens = tokens
                .cache_read_tokens
                .checked_add(usage.cache_read_tokens)
                .ok_or_else(|| invalid("aggregate token overflow"))?;
        }
    }
    let total_tokens = tokens
        .total_tokens()
        .filter(|n| *n <= i32::MAX as u64)
        .ok_or_else(|| invalid("aggregate token overflow"))?;
    parent.total_cost_usd = request_funds_usd(next.known_total_cost_units);
    parent.actual_total_cost_usd = request_funds_usd(next.known_actual_cost_units);
    parent.input_tokens = tokens.input_tokens;
    parent.output_tokens = tokens.output_tokens;
    parent.cache_creation_input_tokens = tokens.cache_creation_tokens;
    parent.cache_read_input_tokens = tokens.cache_read_tokens;
    parent.cache_creation_ephemeral_5m_input_tokens = 0;
    parent.cache_creation_ephemeral_1h_input_tokens = 0;
    parent.total_tokens = total_tokens;
    let metadata = parent
        .request_metadata
        .get_or_insert_with(|| serde_json::json!({}));
    if let Some(metadata) = metadata.as_object_mut() {
        metadata.insert("usage_available".to_string(), serde_json::Value::Bool(true));
        metadata.insert(
            "usage_pricing_available".to_string(),
            serde_json::Value::Bool(true),
        );
    }
    parent.billing_status = if next.admission_closed
        && next.prepared_attempts == 0
        && next.unknown_attempts == 0
        && !next.requires_reconciliation
    {
        "settled"
    } else {
        "pending"
    }
    .to_string();
    *summary = Some(next);
    Ok(())
}

fn contribution(a: &StoredRequestAttemptFunds) -> Option<ProviderApiKeyUsageContribution> {
    a.dispatched_at_unix_secs?;
    let key_id = a.provider.provider_api_key_id.clone()?;
    let success = a
        .terminal_facts
        .as_ref()
        .is_some_and(|f| f.execution.status == RequestAttemptExecutionStatus::Completed);
    let (total_tokens, total_cost_usd) = match a.terminal_facts.as_ref().map(|f| &f.outcome) {
        Some(RequestAttemptFinancialOutcome::Charged { usage }) => (
            usage.total_tokens()? as i64,
            request_funds_usd(usage.total_cost_units),
        ),
        _ => (0, 0.0),
    };
    Some(ProviderApiKeyUsageContribution {
        key_id,
        request_count: 1,
        success_count: i64::from(success),
        error_count: i64::from(a.terminal_facts.is_some() && !success),
        total_tokens,
        total_cost_usd,
        total_response_time_ms: if success {
            a.terminal_facts.as_ref()?.execution.response_time_ms as i64
        } else {
            0
        },
        last_used_at_unix_secs: Some(a.funds.quote.admitted_at_unix_secs),
        usage_created_at_unix_secs: Some(a.funds.quote.admitted_at_unix_secs),
    })
}

pub(super) fn reserve(
    repo: &InMemorySettlementRepository,
    input: ReserveRequestAttemptFundsInput,
) -> Result<ReserveRequestAttemptFundsOutcome, DataLayerError> {
    input.validate()?;
    let _lock = repo.settlement_lock.lock().expect("settlement lock");
    let usage = repo
        .usage
        .as_ref()
        .ok_or_else(|| invalid("attempt admission requires usage repository"))?;
    usage.with_attempt_parent(&input.quote.identity.request_id, |parent, summary| {
        parent_matches(parent, &input.quote.identity)?;
        if let Some(existing) = get(repo, &input.identity())? {
            return Ok(
                if existing.provider == input.provider && existing.funds.quote == input.quote {
                    ReserveRequestAttemptFundsOutcome::Reserved {
                        reservation: Box::new(existing),
                    }
                } else {
                    ReserveRequestAttemptFundsOutcome::Conflict
                },
            );
        }
        if summary.as_ref().is_some_and(|s| s.admission_closed) {
            return Ok(ReserveRequestAttemptFundsOutcome::AdmissionClosed);
        }
        if summary.is_none() && parent.billing_status != "pending" {
            return Ok(ReserveRequestAttemptFundsOutcome::Conflict);
        }
        if all(repo, &input.quote.identity.request_id).len() >= 64 {
            return Err(invalid("request attempt count exceeds 64"));
        }
        match funding::reserve_inner(repo, input.quote.clone(), Some(&input.attempt_id))? {
            ReserveRequestFundsOutcome::Reserved { reservation } => {
                let stored = StoredRequestAttemptFunds {
                    attempt_id: input.attempt_id.clone(),
                    provider: input.provider.clone(),
                    funds: *reservation,
                    dispatched_at_unix_secs: None,
                    terminal_facts: None,
                };
                repo.attempt_metadata
                    .write()
                    .expect("attempt metadata lock")
                    .insert(
                        input.quote.identity.reservation_token.clone(),
                        stored.clone(),
                    );
                apply_summary(
                    parent,
                    summary,
                    &all(repo, &input.quote.identity.request_id),
                )?;
                Ok(ReserveRequestAttemptFundsOutcome::Reserved {
                    reservation: Box::new(stored),
                })
            }
            ReserveRequestFundsOutcome::Insufficient {
                available_cost_units,
            } => Ok(ReserveRequestAttemptFundsOutcome::Insufficient {
                available_cost_units,
            }),
            ReserveRequestFundsOutcome::WalletUnavailable => {
                Ok(ReserveRequestAttemptFundsOutcome::WalletUnavailable)
            }
            ReserveRequestFundsOutcome::Conflict => Ok(ReserveRequestAttemptFundsOutcome::Conflict),
        }
    })
}

pub(super) fn read(
    repo: &InMemorySettlementRepository,
    identity: RequestAttemptFundsIdentity,
) -> Result<Option<StoredRequestAttemptFunds>, DataLayerError> {
    identity.validate()?;
    let _lock = repo.settlement_lock.lock().expect("settlement lock");
    get(repo, &identity)
}

pub(super) fn dispatch(
    repo: &InMemorySettlementRepository,
    identity: RequestAttemptFundsIdentity,
) -> Result<Option<StoredRequestAttemptFunds>, DataLayerError> {
    identity.validate()?;
    let _lock = repo.settlement_lock.lock().expect("settlement lock");
    let usage = repo
        .usage
        .as_ref()
        .ok_or_else(|| invalid("attempt usage repository unavailable"))?;
    usage.with_attempt_parent(&identity.request.request_id, |parent, summary| {
        parent_matches(parent, &identity.request)?;
        let Some(mut stored) = get(repo, &identity)? else {
            return Ok(None);
        };
        if stored.funds.state == RequestFundsState::Dispatched {
            return Ok(Some(stored));
        }
        if stored.funds.state != RequestFundsState::Prepared
            || summary.as_ref().is_some_and(|s| s.admission_closed)
        {
            return Err(invalid("terminal or closed attempt cannot dispatch"));
        }
        stored.funds.state = RequestFundsState::Dispatched;
        stored.dispatched_at_unix_secs = Some(chrono::Utc::now().timestamp().max(0) as u64);
        repo.funds.write().expect("funds lock").insert(
            identity.request.reservation_token.clone(),
            stored.funds.clone(),
        );
        repo.attempt_metadata
            .write()
            .expect("attempt metadata lock")
            .insert(identity.request.reservation_token.clone(), stored.clone());
        apply_summary(parent, summary, &all(repo, &identity.request.request_id))?;
        if let Some(after) = contribution(&stored) {
            usage.apply_attempt_provider_contribution(&stored.attempt_id, None, &after);
        }
        Ok(Some(stored))
    })
}

pub(super) fn outcome(
    repo: &InMemorySettlementRepository,
    input: RecordRequestAttemptFundsOutcomeInput,
) -> Result<Option<StoredRequestAttemptFunds>, DataLayerError> {
    input.validate()?;
    let _lock = repo.settlement_lock.lock().expect("settlement lock");
    let usage_repo = repo
        .usage
        .as_ref()
        .ok_or_else(|| invalid("attempt usage repository unavailable"))?;
    usage_repo.with_attempt_parent(&input.identity.request.request_id, |parent, summary| {
        parent_matches(parent, &input.identity.request)?;
        let Some(mut stored) = get(repo, &input.identity)? else {
            return Ok(None);
        };
        if let Some(facts) = &stored.terminal_facts {
            if facts == &input.facts {
                return Ok(Some(stored));
            }
            if !matches!(facts.outcome, RequestAttemptFinancialOutcome::Unknown)
                || facts.execution != input.facts.execution
            {
                return Err(invalid("conflicting terminal attempt facts"));
            }
        }
        let before = contribution(&stored);
        if stored.funds.settlement.is_some() || stored.funds.state == RequestFundsState::Released {
            return Err(invalid("attempt already terminal"));
        }
        // Validate aggregate token arithmetic on staged copies before touching wallets.
        let mut candidates = all(repo, &input.identity.request.request_id);
        if let Some(candidate) = candidates
            .iter_mut()
            .find(|a| a.attempt_id == input.identity.attempt_id)
        {
            candidate.terminal_facts = Some(input.facts.clone());
        }
        apply_summary(&mut parent.clone(), &mut summary.clone(), &candidates)?;
        match &input.facts.outcome {
            RequestAttemptFinancialOutcome::Unknown => {
                if stored.dispatched_at_unix_secs.is_none() {
                    return Err(invalid(
                        "undispatched work cannot have unknown upstream cost",
                    ));
                }
                stored.funds.state = RequestFundsState::ReconciliationPending;
            }
            RequestAttemptFinancialOutcome::NoCharge => {
                stored.funds.state = RequestFundsState::Released;
                stored.funds.actual_cost_units = Some(0);
            }
            RequestAttemptFinancialOutcome::Charged { usage } => {
                if stored.dispatched_at_unix_secs.is_none() {
                    return Err(invalid("undispatched attempt cannot charge"));
                }
                stored.funds = funding::finalize_inner(
                    repo,
                    FinalizeRequestFundsInput {
                        identity: input.identity.request.clone(),
                        usage: UsageSettlementInput {
                            request_id: input.identity.request.request_id.clone(),
                            user_id: input.identity.request.user_id.clone(),
                            api_key_id: input.identity.request.api_key_id.clone(),
                            api_key_is_standalone: input.identity.request.api_key_is_standalone,
                            provider_id: Some(stored.provider.provider_id.clone()),
                            status: "completed".to_string(),
                            billing_status: "pending".to_string(),
                            total_cost_usd: request_funds_usd(usage.total_cost_units),
                            actual_total_cost_usd: request_funds_usd(usage.actual_cost_units),
                            finalized_at_unix_secs: Some(input.finalized_at_unix_secs),
                        },
                        reconciliation_facts: (usage.actual_cost_units
                            > stored.funds.quote.authorized_cost_units)
                            .then(|| serde_json::json!({"reason":"authorization_exceeded"})),
                    },
                    Some(&input.identity.attempt_id),
                )?
                .ok_or_else(|| invalid("attempt funds missing"))?;
            }
        }
        stored.terminal_facts = Some(input.facts.clone());
        repo.funds.write().expect("funds lock").insert(
            input.identity.request.reservation_token.clone(),
            stored.funds.clone(),
        );
        repo.attempt_metadata
            .write()
            .expect("attempt metadata lock")
            .insert(
                input.identity.request.reservation_token.clone(),
                stored.clone(),
            );
        apply_summary(
            parent,
            summary,
            &all(repo, &input.identity.request.request_id),
        )?;
        if let Some(after) = contribution(&stored) {
            usage_repo.apply_attempt_provider_contribution(
                &stored.attempt_id,
                before.as_ref(),
                &after,
            );
        }
        Ok(Some(stored))
    })
}

pub(super) fn close(
    repo: &InMemorySettlementRepository,
    input: CloseRequestFundsAdmissionInput,
) -> Result<Option<RequestFundsSummary>, DataLayerError> {
    input.identity.validate()?;
    if input.closed_at_unix_secs > i64::MAX as u64 {
        return Err(invalid("admission close time overflow"));
    }
    let _lock = repo.settlement_lock.lock().expect("settlement lock");
    let usage = repo
        .usage
        .as_ref()
        .ok_or_else(|| invalid("attempt usage repository unavailable"))?;
    usage.with_attempt_parent(&input.identity.request.request_id, |parent, summary| {
        parent_matches(parent, &input.identity.request)?;
        if get(repo, &input.identity)?.is_none() {
            return Ok(None);
        }
        summary
            .as_mut()
            .ok_or_else(|| invalid("attempt parent mode missing"))?
            .admission_closed = true;
        apply_summary(
            parent,
            summary,
            &all(repo, &input.identity.request.request_id),
        )?;
        Ok(summary.clone())
    })
}

#[cfg(test)]
mod tests;
