//! Durable per-operation accounting under one public request ID.
use super::*;

async fn lock_parent(
    tx: &mut PostgresTransaction,
    identity: &RequestFundsIdentity,
) -> Result<Option<PgRow>, DataLayerError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::BIGINT)")
        .bind(&identity.request_id)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
    let row = sqlx::query("SELECT user_id, api_key_id, request_metadata, billing_mode, billing_status, funds_admission_closed_at IS NOT NULL AS admission_closed FROM usage WHERE request_id = $1 FOR UPDATE")
        .bind(&identity.request_id).fetch_optional(&mut **tx).await.map_postgres_err()?;
    if let Some(row) = &row {
        let metadata: Option<Value> = row.try_get("request_metadata").map_postgres_err()?;
        let standalone = metadata
            .as_ref()
            .and_then(|m| m.get("api_key_is_standalone"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if row
            .try_get::<Option<String>, _>("user_id")
            .map_postgres_err()?
            != identity.user_id
            || row
                .try_get::<Option<String>, _>("api_key_id")
                .map_postgres_err()?
                != identity.api_key_id
            || standalone != identity.api_key_is_standalone
        {
            return Err(invalid("attempt funds parent owner conflict"));
        }
    }
    Ok(row)
}

async fn load_attempt(
    tx: &mut PostgresTransaction,
    token: &str,
) -> Result<Option<StoredRequestAttemptFunds>, DataLayerError> {
    let Some(funds) = find_reservation(tx, token).await? else {
        return Ok(None);
    };
    let Some(attempt_id) = funds.attempt_id.clone() else {
        return Err(invalid("legacy reservation is not an attempt"));
    };
    let row = sqlx::query("SELECT candidate_id, provider_id, provider_api_key_id, model_id, terminal_facts, EXTRACT(EPOCH FROM dispatched_at)::bigint AS dispatched_at_unix_secs FROM request_fund_reservations WHERE reservation_token = $1")
        .bind(token).fetch_one(&mut **tx).await.map_postgres_err()?;
    Ok(Some(StoredRequestAttemptFunds {
        attempt_id,
        provider: RequestAttemptProvider {
            provider_id: row.try_get("provider_id").map_postgres_err()?,
            provider_api_key_id: row.try_get("provider_api_key_id").map_postgres_err()?,
            model_id: row.try_get("model_id").map_postgres_err()?,
            candidate_id: row.try_get("candidate_id").map_postgres_err()?,
        },
        funds,
        dispatched_at_unix_secs: row
            .try_get::<Option<i64>, _>("dispatched_at_unix_secs")
            .map_postgres_err()?
            .map(|v| v as u64),
        terminal_facts: row
            .try_get::<Option<Value>, _>("terminal_facts")
            .map_postgres_err()?
            .map(decode)
            .transpose()?,
    }))
}

async fn owned_attempt(
    tx: &mut PostgresTransaction,
    identity: &RequestAttemptFundsIdentity,
) -> Result<Option<StoredRequestAttemptFunds>, DataLayerError> {
    let attempt = load_attempt(tx, &identity.request.reservation_token).await?;
    if attempt.as_ref().is_some_and(|a| {
        a.attempt_id != identity.attempt_id || a.funds.quote.identity != identity.request
    }) {
        return Err(invalid("attempt funds identity conflict"));
    }
    Ok(attempt)
}

/// Lock financial rows before reservation rows, matching every debit operation.
async fn lock_finances(tx: &mut PostgresTransaction, token: &str) -> Result<(), DataLayerError> {
    sqlx::query("SELECT w.id FROM wallets w JOIN request_fund_reservations r ON r.wallet_id = w.id WHERE r.reservation_token = $1 FOR UPDATE OF w")
        .bind(token).fetch_all(&mut **tx).await.map_postgres_err()?;
    sqlx::query("SELECT e.id FROM user_plan_entitlements e WHERE e.id IN (SELECT source_id FROM request_fund_allocations WHERE reservation_token = $1 AND source_kind = 'entitlement') ORDER BY e.expires_at, e.created_at, e.id FOR UPDATE")
        .bind(token).fetch_all(&mut **tx).await.map_postgres_err()?;
    Ok(())
}

pub(crate) async fn reserve(
    tx: &mut PostgresTransaction,
    input: ReserveRequestAttemptFundsInput,
) -> Result<ReserveRequestAttemptFundsOutcome, DataLayerError> {
    let Some(parent) = lock_parent(tx, &input.quote.identity).await? else {
        return Err(invalid("attempt admission requires persisted parent usage"));
    };
    let mode: String = parent.try_get("billing_mode").map_postgres_err()?;
    if mode == "legacy" {
        let prior: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM request_fund_reservations WHERE request_id = $1)",
        )
        .bind(&input.quote.identity.request_id)
        .fetch_one(&mut **tx)
        .await
        .map_postgres_err()?;
        if parent
            .try_get::<String, _>("billing_status")
            .map_postgres_err()?
            != "pending"
            || prior
        {
            return Ok(ReserveRequestAttemptFundsOutcome::Conflict);
        }
    }
    // A read before financial locks is deliberately non-locking. Admission holds
    // the parent lock, so another attempt for this request cannot mutate it.
    let existing: Option<(String, Option<String>)> = sqlx::query_as("SELECT reservation_token, attempt_id::text FROM request_fund_reservations WHERE reservation_token = $1 OR attempt_id = $2::text::uuid")
        .bind(&input.quote.identity.reservation_token).bind(&input.attempt_id).fetch_optional(&mut **tx).await.map_postgres_err()?;
    if let Some((token, attempt_id)) = existing {
        if token != input.quote.identity.reservation_token
            || attempt_id.as_deref() != Some(input.attempt_id.as_str())
        {
            return Ok(ReserveRequestAttemptFundsOutcome::Conflict);
        }
        lock_finances(tx, &token).await?;
        let Some(stored) = owned_attempt(tx, &input.identity()).await? else {
            return Ok(ReserveRequestAttemptFundsOutcome::Conflict);
        };
        return Ok(
            if stored.funds.quote == input.quote && stored.provider == input.provider {
                ReserveRequestAttemptFundsOutcome::Reserved {
                    reservation: Box::new(stored),
                }
            } else {
                ReserveRequestAttemptFundsOutcome::Conflict
            },
        );
    }
    if parent
        .try_get::<bool, _>("admission_closed")
        .map_postgres_err()?
    {
        return Ok(ReserveRequestAttemptFundsOutcome::AdmissionClosed);
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM request_fund_reservations WHERE request_id = $1")
            .bind(&input.quote.identity.request_id)
            .fetch_one(&mut **tx)
            .await
            .map_postgres_err()?;
    if count >= 64 {
        return Err(invalid("request attempt count exceeds 64"));
    }
    match reserve_inner(tx, input.quote.clone(), Some(&input)).await? {
        ReserveRequestFundsOutcome::Reserved { .. } => {
            sqlx::query("UPDATE usage SET billing_mode = 'attempt_funds' WHERE request_id = $1")
                .bind(&input.quote.identity.request_id)
                .execute(&mut **tx)
                .await
                .map_postgres_err()?;
            // A pending parent has no customer contribution, but it may have a
            // provider pending count. Remove it before switching provider counts
            // to actual dispatched operations.
            if mode == "legacy" {
                crate::usage::remove_parent_provider_contribution_for_attempts(
                    tx,
                    &input.quote.identity.request_id,
                )
                .await?;
            }
            refresh_summary(tx, &input.quote.identity.request_id).await?;
            Ok(ReserveRequestAttemptFundsOutcome::Reserved {
                reservation: Box::new(
                    owned_attempt(tx, &input.identity())
                        .await?
                        .ok_or_else(|| invalid("reserved attempt disappeared"))?,
                ),
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
}

pub(crate) async fn read(
    tx: &mut PostgresTransaction,
    identity: RequestAttemptFundsIdentity,
) -> Result<Option<StoredRequestAttemptFunds>, DataLayerError> {
    if lock_parent(tx, &identity.request).await?.is_none() {
        return Ok(None);
    }
    lock_finances(tx, &identity.request.reservation_token).await?;
    owned_attempt(tx, &identity).await
}

pub(crate) async fn dispatch(
    tx: &mut PostgresTransaction,
    identity: RequestAttemptFundsIdentity,
) -> Result<Option<StoredRequestAttemptFunds>, DataLayerError> {
    let Some(parent) = lock_parent(tx, &identity.request).await? else {
        return Ok(None);
    };
    lock_finances(tx, &identity.request.reservation_token).await?;
    let Some(mut attempt) = owned_attempt(tx, &identity).await? else {
        return Ok(None);
    };
    match attempt.funds.state {
        RequestFundsState::Prepared => {
            if parent
                .try_get::<bool, _>("admission_closed")
                .map_postgres_err()?
            {
                return Err(invalid(
                    "closed request admission cannot dispatch prepared work",
                ));
            }
            sqlx::query("UPDATE request_fund_reservations SET state = 'dispatched', dispatched_at = NOW(), updated_at = NOW() WHERE reservation_token = $1")
                .bind(&identity.request.reservation_token).execute(&mut **tx).await.map_postgres_err()?;
            attempt.funds.state = RequestFundsState::Dispatched;
            provider_delta(tx, &attempt, 1, 0, 0, 0, 0, 0).await?;
            refresh_summary(tx, &identity.request.request_id).await?;
        }
        RequestFundsState::Dispatched => {}
        _ => return Err(invalid("terminal attempt cannot dispatch")),
    }
    owned_attempt(tx, &identity).await
}

pub(crate) async fn outcome(
    tx: &mut PostgresTransaction,
    input: RecordRequestAttemptFundsOutcomeInput,
) -> Result<Option<StoredRequestAttemptFunds>, DataLayerError> {
    if lock_parent(tx, &input.identity.request).await?.is_none() {
        return Ok(None);
    }
    lock_finances(tx, &input.identity.request.reservation_token).await?;
    let Some(mut previous) = owned_attempt(tx, &input.identity).await? else {
        return Ok(None);
    };
    if let Some(facts) = &previous.terminal_facts {
        if facts == &input.facts {
            return Ok(Some(previous));
        }
        if !matches!(facts.outcome, RequestAttemptFinancialOutcome::Unknown) {
            return Err(invalid("conflicting terminal attempt facts"));
        }
        if facts.execution != input.facts.execution {
            return Err(invalid(
                "attempt reconciliation cannot change execution facts",
            ));
        }
    }
    if previous.funds.settlement.is_some() || previous.funds.state == RequestFundsState::Released {
        return Err(invalid("attempt is already financially terminal"));
    }
    match &input.facts.outcome {
        RequestAttemptFinancialOutcome::Unknown => {
            if previous.dispatched_at_unix_secs.is_none() {
                return Err(invalid(
                    "undispatched work cannot have unknown upstream cost",
                ));
            }
            previous.funds.state = RequestFundsState::ReconciliationPending;
            persist_reservation(tx, &previous.funds).await?;
        }
        RequestAttemptFinancialOutcome::NoCharge => {
            previous.funds.state = RequestFundsState::Released;
            previous.funds.actual_cost_units = Some(0);
            persist_reservation(tx, &previous.funds).await?;
        }
        RequestAttemptFinancialOutcome::Charged { usage } => {
            if previous.dispatched_at_unix_secs.is_none() {
                return Err(invalid("undispatched attempt cannot charge"));
            }
            let settlement = FinalizeRequestFundsInput {
                identity: input.identity.request.clone(),
                usage: UsageSettlementInput {
                    request_id: input.identity.request.request_id.clone(),
                    user_id: input.identity.request.user_id.clone(), api_key_id: input.identity.request.api_key_id.clone(),
                    api_key_is_standalone: input.identity.request.api_key_is_standalone,
                    provider_id: Some(previous.provider.provider_id.clone()),
                    status: "completed".to_string(), billing_status: "pending".to_string(),
                    total_cost_usd: request_funds_usd(usage.total_cost_units),
                    actual_total_cost_usd: request_funds_usd(usage.actual_cost_units),
                    finalized_at_unix_secs: Some(input.finalized_at_unix_secs),
                },
                reconciliation_facts: (usage.actual_cost_units > previous.funds.quote.authorized_cost_units)
                    .then(|| serde_json::json!({"reason":"authorization_exceeded", "excess_units": usage.actual_cost_units - previous.funds.quote.authorized_cost_units})),
            };
            finalize_inner(tx, settlement, Some(&input.identity.attempt_id))
                .await?
                .ok_or_else(|| invalid("attempt finalize lost parent usage"))?;
        }
    }
    if previous.dispatched_at_unix_secs.is_some() {
        let first_terminal = previous.terminal_facts.is_none();
        let success = input.facts.execution.status == RequestAttemptExecutionStatus::Completed;
        let (tokens, cost) = match &input.facts.outcome {
            RequestAttemptFinancialOutcome::Charged { usage } => (
                usage
                    .total_tokens()
                    .ok_or_else(|| invalid("attempt tokens overflow"))?,
                usage.total_cost_units,
            ),
            _ => (0, 0),
        };
        provider_delta(
            tx,
            &previous,
            0,
            i64::from(first_terminal && success),
            i64::from(first_terminal && !success),
            tokens as i64,
            cost,
            if first_terminal && success {
                input.facts.execution.response_time_ms as i64
            } else {
                0
            },
        )
        .await?;
    }
    sqlx::query("UPDATE request_fund_reservations SET terminal_facts = $2, updated_at = NOW() WHERE reservation_token = $1")
        .bind(&input.identity.request.reservation_token).bind(encode(&input.facts)?).execute(&mut **tx).await.map_postgres_err()?;
    refresh_summary(tx, &input.identity.request.request_id).await?;
    owned_attempt(tx, &input.identity).await
}

#[allow(clippy::too_many_arguments)]
async fn provider_delta(
    tx: &mut PostgresTransaction,
    attempt: &StoredRequestAttemptFunds,
    requests: i64,
    success: i64,
    errors: i64,
    tokens: i64,
    cost: u64,
    response_ms: i64,
) -> Result<(), DataLayerError> {
    let Some(key) = attempt.provider.provider_api_key_id.as_deref() else {
        return Ok(());
    };
    if requests == 0 && success == 0 && errors == 0 && tokens == 0 && cost == 0 && response_ms == 0
    {
        return Ok(());
    }
    sqlx::query("INSERT INTO usage_counter_deltas (id, request_id, kind, target_id, request_count_delta, success_count_delta, error_count_delta, total_tokens_delta, total_cost_usd_delta, total_response_time_ms_delta, candidate_last_used_at_unix_secs, usage_created_at_unix_secs) VALUES ($1,$2,'provider_api_key',$3,$4,$5,$6,$7,$8,$9,$10,$10)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(&attempt.funds.quote.identity.request_id).bind(key)
        .bind(requests).bind(success).bind(errors).bind(tokens).bind(request_funds_usd(cost)).bind(response_ms)
        .bind(attempt.funds.quote.admitted_at_unix_secs as i64).execute(&mut **tx).await.map_postgres_err()?;
    Ok(())
}

pub(crate) async fn close(
    tx: &mut PostgresTransaction,
    input: CloseRequestFundsAdmissionInput,
) -> Result<Option<RequestFundsSummary>, DataLayerError> {
    if lock_parent(tx, &input.identity.request).await?.is_none() {
        return Ok(None);
    }
    lock_finances(tx, &input.identity.request.reservation_token).await?;
    if owned_attempt(tx, &input.identity).await?.is_none() {
        return Ok(None);
    }
    sqlx::query("UPDATE usage SET funds_admission_closed_at = COALESCE(funds_admission_closed_at, to_timestamp($2::double precision)) WHERE request_id = $1")
        .bind(&input.identity.request.request_id).bind(input.closed_at_unix_secs as i64).execute(&mut **tx).await.map_postgres_err()?;
    refresh_summary(tx, &input.identity.request.request_id)
        .await
        .map(Some)
}

pub(crate) async fn refresh_summary(
    tx: &mut PostgresTransaction,
    request: &str,
) -> Result<RequestFundsSummary, DataLayerError> {
    let closed: bool = sqlx::query_scalar(
        "SELECT funds_admission_closed_at IS NOT NULL FROM usage WHERE request_id = $1",
    )
    .bind(request)
    .fetch_one(&mut **tx)
    .await
    .map_postgres_err()?;
    let tokens: Vec<String> = sqlx::query_scalar("SELECT reservation_token FROM request_fund_reservations WHERE request_id = $1 AND attempt_id IS NOT NULL ORDER BY reservation_token")
        .bind(request).fetch_all(&mut **tx).await.map_postgres_err()?;
    let mut attempts = Vec::new();
    for token in tokens {
        attempts.push(
            load_attempt(tx, &token)
                .await?
                .ok_or_else(|| invalid("attempt summary lost row"))?,
        );
    }
    let summary = summarize_request_attempt_funds(&attempts, closed)?;
    let mut usage = RequestAttemptBilledUsage::default();
    for attempt in &attempts {
        if let Some(RequestAttemptTerminalFacts {
            outcome: RequestAttemptFinancialOutcome::Charged { usage: billed },
            ..
        }) = &attempt.terminal_facts
        {
            usage.input_tokens = usage
                .input_tokens
                .checked_add(billed.input_tokens)
                .ok_or_else(|| invalid("aggregate input tokens overflow"))?;
            usage.output_tokens = usage
                .output_tokens
                .checked_add(billed.output_tokens)
                .ok_or_else(|| invalid("aggregate output tokens overflow"))?;
            usage.cache_creation_tokens = usage
                .cache_creation_tokens
                .checked_add(billed.cache_creation_tokens)
                .ok_or_else(|| invalid("aggregate cache tokens overflow"))?;
            usage.cache_read_tokens = usage
                .cache_read_tokens
                .checked_add(billed.cache_read_tokens)
                .ok_or_else(|| invalid("aggregate cache tokens overflow"))?;
        }
    }
    if usage
        .total_tokens()
        .is_none_or(|tokens| tokens > i32::MAX as u64)
    {
        return Err(invalid("aggregate tokens overflow"));
    }
    crate::usage::apply_attempt_funds_summary_in_tx(tx, request, &summary, &usage).await?;
    Ok(summary)
}

#[cfg(test)]
mod tests;
