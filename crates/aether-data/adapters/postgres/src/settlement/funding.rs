//! Financial holds share the wallet/entitlement locks used by every debit path.
use std::collections::BTreeSet;

use aether_data_contracts::repository::settlement::*;
use aether_data_contracts::DataLayerError;
use serde_json::Value;
use sqlx::{postgres::PgRow, Row};

use crate::error::SqlxResultExt;
use crate::PostgresTransaction;

const ACTIVE_STATES: &str = "('prepared', 'dispatched', 'reconciliation_pending')";

#[cfg(test)]
mod tests;

fn invalid(message: &str) -> DataLayerError {
    DataLayerError::InvalidInput(message.to_string())
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, DataLayerError> {
    serde_json::from_value(value).map_err(|_| {
        DataLayerError::UnexpectedValue("invalid stored request funds data".to_string())
    })
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Value, DataLayerError> {
    serde_json::to_value(value).map_err(|_| invalid("request funds data cannot be serialized"))
}

pub(crate) async fn held_source_units(
    tx: &mut PostgresTransaction,
    source_id: &str,
    kind: &str,
    usage_date: Option<&str>,
) -> Result<u64, DataLayerError> {
    let sql = format!(
        "SELECT CAST(COALESCE(SUM(a.reserved_cost_units - a.collected_cost_units), 0) AS BIGINT) \
         FROM request_fund_allocations a JOIN request_fund_reservations r \
         ON r.reservation_token = a.reservation_token \
         WHERE a.source_id = $1 AND a.source_kind = $2 \
         AND a.usage_date IS NOT DISTINCT FROM $3 AND r.state IN {ACTIVE_STATES} \
         AND r.settlement IS NULL"
    );
    let units: i64 = sqlx::query_scalar(&sql)
        .bind(source_id)
        .bind(kind)
        .bind(usage_date)
        .fetch_one(&mut **tx)
        .await
        .map_postgres_err()?;
    u64::try_from(units).map_err(|_| invalid("negative held request funds"))
}

/// Caller holds the wallet row lock. Negative legacy balances are debts, never capacity.
pub(crate) async fn wallet_available_units(
    tx: &mut PostgresTransaction,
    wallet_id: &str,
    recharge: f64,
    gift: f64,
) -> Result<(u64, u64), DataLayerError> {
    if !recharge.is_finite() || !gift.is_finite() || gift < 0.0 {
        return Err(invalid("wallet financial state is invalid"));
    }
    if recharge < 0.0 {
        return Ok((0, 0));
    }
    let recharge = request_funds_available_units(recharge)?
        .saturating_sub(held_source_units(tx, wallet_id, "wallet_recharge", None).await?);
    let gift = request_funds_available_units(gift)?
        .saturating_sub(held_source_units(tx, wallet_id, "wallet_gift", None).await?);
    Ok((recharge, gift))
}

/// Validate an external debit while its wallet row is locked. Holds are bucket-specific.
pub(crate) async fn ensure_wallet_holds_preserved(
    tx: &mut PostgresTransaction,
    wallet_id: &str,
    after_recharge: f64,
    after_gift: f64,
) -> Result<(), DataLayerError> {
    let held_recharge = held_source_units(tx, wallet_id, "wallet_recharge", None).await?;
    let held_gift = held_source_units(tx, wallet_id, "wallet_gift", None).await?;
    if held_recharge > request_funds_available_units(after_recharge.max(0.0))?
        || held_gift > request_funds_available_units(after_gift.max(0.0))?
    {
        return Err(invalid(
            "wallet debit would consume funds reserved for a request",
        ));
    }
    Ok(())
}

async fn locked_wallet(
    tx: &mut PostgresTransaction,
    identity: &RequestFundsIdentity,
) -> Result<Option<PgRow>, DataLayerError> {
    if let Some(key_id) = identity.api_key_id.as_deref() {
        let key = sqlx::query("SELECT user_id, is_standalone FROM api_keys WHERE id = $1")
            .bind(key_id)
            .fetch_optional(&mut **tx)
            .await
            .map_postgres_err()?;
        let Some(key) = key else {
            return Err(invalid("request funds API key no longer exists"));
        };
        let owner: Option<String> = key.try_get("user_id").map_postgres_err()?;
        let standalone: bool = key.try_get("is_standalone").map_postgres_err()?;
        if owner.as_ref() != identity.user_id.as_ref()
            || standalone != identity.api_key_is_standalone
        {
            return Err(invalid(
                "request funds API key does not belong to its stated owner",
            ));
        }
    }
    sqlx::query(
        "SELECT id, balance::double precision AS balance, gift_balance::double precision AS gift_balance, \
         total_consumed::double precision AS total_consumed, status, limit_mode FROM wallets \
         WHERE (api_key_id = $1) OR (NOT $2 AND user_id = $3 AND api_key_id IS NULL) \
         ORDER BY CASE WHEN api_key_id = $1 THEN 0 ELSE 1 END LIMIT 1 FOR UPDATE",
    )
    .bind(&identity.api_key_id)
    .bind(identity.api_key_is_standalone)
    .bind(&identity.user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_postgres_err()
}

async fn find_reservation(
    tx: &mut PostgresTransaction,
    token: &str,
) -> Result<Option<StoredRequestFundsReservation>, DataLayerError> {
    let row = sqlx::query(
        "SELECT * FROM request_fund_reservations WHERE reservation_token = $1 FOR UPDATE",
    )
    .bind(token)
    .fetch_optional(&mut **tx)
    .await
    .map_postgres_err()?;
    let Some(row) = row else { return Ok(None) };
    let allocations = sqlx::query(
        "SELECT source_kind, source_id, usage_date, reserved_cost_units, quota_cost_units FROM request_fund_allocations \
         WHERE reservation_token = $1 ORDER BY ordinal",
    )
    .bind(token)
    .fetch_all(&mut **tx)
    .await
    .map_postgres_err()?
    .into_iter()
    .map(|row| {
        let source_id: String = row.try_get("source_id").map_postgres_err()?;
        let kind: String = row.try_get("source_kind").map_postgres_err()?;
        let source = match kind.as_str() {
            "wallet_recharge" => RequestFundingSource::WalletRecharge { wallet_id: source_id },
            "wallet_gift" => RequestFundingSource::WalletGift { wallet_id: source_id },
            "postpaid" => RequestFundingSource::Postpaid { wallet_id: source_id },
            "entitlement" => RequestFundingSource::Entitlement {
                entitlement_id: source_id,
                usage_date: row.try_get("usage_date").map_postgres_err()?,
                quota_cost_units: row.try_get::<i64, _>("quota_cost_units").map_postgres_err()? as u64,
            },
            _ => return Err(invalid("unknown stored funding source")),
        };
        Ok(RequestFundsAllocation {
            source,
            reserved_cost_units: u64::try_from(row.try_get::<i64, _>("reserved_cost_units").map_postgres_err()?)
                .map_err(|_| invalid("negative stored funds allocation"))?,
        })
    })
    .collect::<Result<Vec<_>, DataLayerError>>()?;
    let state: String = row.try_get("state").map_postgres_err()?;
    Ok(Some(StoredRequestFundsReservation {
        quote: decode(row.try_get("quote").map_postgres_err()?)?,
        wallet_id: row.try_get("wallet_id").map_postgres_err()?,
        allocations,
        state: decode(Value::String(state))?,
        actual_cost_units: row
            .try_get::<Option<i64>, _>("actual_cost_units")
            .map_postgres_err()?
            .map(|value| u64::try_from(value).map_err(|_| invalid("negative actual funds cost")))
            .transpose()?,
        collected_cost_units: u64::try_from(
            row.try_get::<i64, _>("collected_cost_units")
                .map_postgres_err()?,
        )
        .map_err(|_| invalid("negative collected funds cost"))?,
        reconciliation_facts: row
            .try_get::<Option<Value>, _>("reconciliation_facts")
            .map_postgres_err()?,
        settlement: row
            .try_get::<Option<Value>, _>("settlement")
            .map_postgres_err()?
            .map(decode)
            .transpose()?,
    }))
}

async fn identity_reservation(
    tx: &mut PostgresTransaction,
    identity: &RequestFundsIdentity,
) -> Result<Option<StoredRequestFundsReservation>, DataLayerError> {
    let reservation = find_reservation(tx, &identity.reservation_token).await?;
    if reservation
        .as_ref()
        .is_some_and(|stored| &stored.quote.identity != identity)
    {
        return Err(invalid("request funds reservation identity conflict"));
    }
    Ok(reservation)
}

async fn persist_reservation(
    tx: &mut PostgresTransaction,
    reservation: &StoredRequestFundsReservation,
) -> Result<(), DataLayerError> {
    sqlx::query(
        "UPDATE request_fund_reservations SET state = $2, actual_cost_units = $3, \
         collected_cost_units = $4, reconciliation_facts = $5, settlement = $6, updated_at = NOW() WHERE reservation_token = $1",
    )
    .bind(&reservation.quote.identity.reservation_token)
    .bind(reservation.state.as_str())
    .bind(reservation.actual_cost_units.map(|value| value as i64))
    .bind(reservation.collected_cost_units as i64)
    .bind(reservation.reconciliation_facts.as_ref())
    .bind(reservation.settlement.as_ref().map(encode).transpose()?)
    .execute(&mut **tx)
    .await
    .map_postgres_err()?;
    Ok(())
}

async fn grant_capacity(
    tx: &mut PostgresTransaction,
    identity: &RequestFundsIdentity,
    admitted_at_unix_secs: u64,
) -> Result<(Vec<(RequestFundingSource, u64)>, bool), DataLayerError> {
    if identity.api_key_is_standalone {
        return Ok((Vec::new(), true));
    }
    let admitted_at = chrono::DateTime::<chrono::Utc>::from_timestamp(
        i64::try_from(admitted_at_unix_secs)
            .map_err(|_| invalid("admission timestamp overflow"))?,
        0,
    )
    .ok_or_else(|| invalid("invalid admission timestamp"))?;
    let rows = sqlx::query(
        "SELECT e.id, e.entitlements_snapshot, p.entitlements_json FROM user_plan_entitlements e \
         JOIN billing_plans p ON p.id = e.plan_id WHERE e.user_id = $1 AND e.status = 'active' \
         AND e.starts_at <= NOW() AND e.expires_at > NOW() \
         ORDER BY e.expires_at, e.created_at, e.id FOR UPDATE OF e",
    )
    .bind(&identity.user_id)
    .fetch_all(&mut **tx)
    .await
    .map_postgres_err()?;
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut allow_overage = true;
    for row in rows {
        let id: String = row.try_get("id").map_postgres_err()?;
        let snapshot: Value = row.try_get("entitlements_snapshot").map_postgres_err()?;
        let plan: Value = row.try_get("entitlements_json").map_postgres_err()?;
        for grant in super::daily_quota_grants_from_entitlement(
            &id,
            &snapshot,
            super::daily_quota_wallet_overage_policy(&plan),
            admitted_at,
        )? {
            if !seen.insert((id.clone(), grant.usage_date.clone())) {
                return Err(invalid(
                    "duplicate daily quota allocation for an entitlement date",
                ));
            }
            allow_overage &= grant.allow_wallet_overage;
            let spent: f64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(amount_usd), 0)::double precision FROM entitlement_usage_ledgers \
                 WHERE user_entitlement_id = $1 AND usage_date = $2",
            )
            .bind(&id).bind(&grant.usage_date)
            .fetch_one(&mut **tx).await.map_postgres_err()?;
            let held = held_source_units(tx, &id, "entitlement", Some(&grant.usage_date)).await?;
            let capacity = request_funds_available_units(grant.daily_quota_usd)?
                .saturating_sub(request_funds_authorized_units(spent)?)
                .saturating_sub(held);
            out.push((
                RequestFundingSource::Entitlement {
                    entitlement_id: id.clone(),
                    usage_date: grant.usage_date,
                    quota_cost_units: request_funds_available_units(grant.daily_quota_usd)?,
                },
                capacity,
            ));
        }
    }
    Ok((out, allow_overage))
}

pub(super) async fn reserve(
    tx: &mut PostgresTransaction,
    input: ReserveRequestFundsInput,
) -> Result<ReserveRequestFundsOutcome, DataLayerError> {
    let wallet = locked_wallet(tx, &input.identity).await?;
    let (mut sources, allow_overage) =
        grant_capacity(tx, &input.identity, input.admitted_at_unix_secs).await?;
    if let Some(stored) = find_reservation(tx, &input.identity.reservation_token).await? {
        return Ok(if stored.quote == input {
            ReserveRequestFundsOutcome::Reserved {
                reservation: stored,
            }
        } else {
            ReserveRequestFundsOutcome::Conflict
        });
    }
    let wallet_id = wallet
        .as_ref()
        .map(|row| row.try_get::<String, _>("id").map_postgres_err())
        .transpose()?;
    if let Some(row) = wallet {
        let status: String = row.try_get("status").map_postgres_err()?;
        if status != "active" {
            return Ok(ReserveRequestFundsOutcome::WalletUnavailable);
        }
        let recharge: f64 = row.try_get("balance").map_postgres_err()?;
        let gift: f64 = row.try_get("gift_balance").map_postgres_err()?;
        let consumed: f64 = row.try_get("total_consumed").map_postgres_err()?;
        validate_wallet_settlement_values(recharge, gift, consumed, 0.0)?;
        if recharge < 0.0 && input.authorized_cost_units > 0 {
            return Ok(ReserveRequestFundsOutcome::Insufficient {
                available_cost_units: 0,
            });
        }
        let mode: String = row.try_get("limit_mode").map_postgres_err()?;
        let id = wallet_id.as_ref().expect("resolved wallet has an id");
        if allow_overage {
            if mode.eq_ignore_ascii_case("unlimited") {
                sources.push((
                    RequestFundingSource::Postpaid {
                        wallet_id: id.clone(),
                    },
                    input.authorized_cost_units,
                ));
            } else {
                let (recharge, gift) = wallet_available_units(
                    tx,
                    id,
                    row.try_get("balance").map_postgres_err()?,
                    row.try_get("gift_balance").map_postgres_err()?,
                )
                .await?;
                sources.push((
                    RequestFundingSource::WalletRecharge {
                        wallet_id: id.clone(),
                    },
                    recharge,
                ));
                sources.push((
                    RequestFundingSource::WalletGift {
                        wallet_id: id.clone(),
                    },
                    gift,
                ));
            }
        }
    }
    let mut remaining = input.authorized_cost_units;
    let mut allocations = Vec::new();
    for (source, available) in sources {
        let amount = available.min(remaining);
        if amount > 0 {
            allocations.push(RequestFundsAllocation {
                source,
                reserved_cost_units: amount,
            });
        }
        remaining -= amount;
        if remaining == 0 {
            break;
        }
    }
    if remaining > 0 {
        return Ok(ReserveRequestFundsOutcome::Insufficient {
            available_cost_units: input.authorized_cost_units - remaining,
        });
    }
    let inserted = sqlx::query(
        "INSERT INTO request_fund_reservations (reservation_token, request_id, wallet_id, quote, state) \
         VALUES ($1, $2, $3, $4, 'prepared') ON CONFLICT DO NOTHING",
    )
    .bind(&input.identity.reservation_token).bind(&input.identity.request_id)
    .bind(&wallet_id).bind(encode(&input)?)
    .execute(&mut **tx).await.map_postgres_err()?.rows_affected();
    if inserted == 0 {
        return Ok(
            match find_reservation(tx, &input.identity.reservation_token).await? {
                Some(stored) if stored.quote == input => ReserveRequestFundsOutcome::Reserved {
                    reservation: stored,
                },
                _ => ReserveRequestFundsOutcome::Conflict,
            },
        );
    }
    for (ordinal, allocation) in allocations.iter().enumerate() {
        let (kind, source_id, date) = match &allocation.source {
            RequestFundingSource::WalletRecharge { wallet_id } => {
                ("wallet_recharge", wallet_id, None)
            }
            RequestFundingSource::WalletGift { wallet_id } => ("wallet_gift", wallet_id, None),
            RequestFundingSource::Postpaid { wallet_id } => ("postpaid", wallet_id, None),
            RequestFundingSource::Entitlement {
                entitlement_id,
                usage_date,
                ..
            } => ("entitlement", entitlement_id, Some(usage_date)),
        };
        let quota_units = match &allocation.source {
            RequestFundingSource::Entitlement {
                quota_cost_units, ..
            } => Some(*quota_cost_units as i64),
            _ => None,
        };
        sqlx::query("INSERT INTO request_fund_allocations (reservation_token, ordinal, source_kind, source_id, usage_date, reserved_cost_units, quota_cost_units) VALUES ($1, $2, $3, $4, $5, $6, $7)")
            .bind(&input.identity.reservation_token).bind(ordinal as i32).bind(kind).bind(source_id).bind(date)
            .bind(allocation.reserved_cost_units as i64).bind(quota_units).execute(&mut **tx).await.map_postgres_err()?;
    }
    Ok(ReserveRequestFundsOutcome::Reserved {
        reservation: StoredRequestFundsReservation {
            quote: input,
            wallet_id,
            allocations,
            state: RequestFundsState::Prepared,
            actual_cost_units: None,
            collected_cost_units: 0,
            reconciliation_facts: None,
            settlement: None,
        },
    })
}

pub(super) async fn dispatch(
    tx: &mut PostgresTransaction,
    identity: RequestFundsIdentity,
) -> Result<Option<StoredRequestFundsReservation>, DataLayerError> {
    let Some(mut reservation) = identity_reservation(tx, &identity).await? else {
        return Ok(None);
    };
    match reservation.state {
        RequestFundsState::Prepared => {
            reservation.state = RequestFundsState::Dispatched;
            persist_reservation(tx, &reservation).await?;
        }
        RequestFundsState::Dispatched => {}
        _ => return Err(invalid("terminal request funds cannot be dispatched")),
    }
    Ok(Some(reservation))
}

pub(super) async fn release(
    tx: &mut PostgresTransaction,
    input: ReleaseRequestFundsInput,
) -> Result<Option<StoredRequestFundsReservation>, DataLayerError> {
    let Some(mut reservation) = identity_reservation(tx, &input.identity).await? else {
        return Ok(None);
    };
    match reservation.state {
        RequestFundsState::Released => return Ok(Some(reservation)),
        RequestFundsState::Prepared => {}
        RequestFundsState::Dispatched if input.terminal_no_charge => {}
        _ => {
            return Err(invalid(
                "request funds require an authoritative no-charge terminal before release",
            ))
        }
    }
    reservation.state = RequestFundsState::Released;
    reservation.actual_cost_units = Some(0);
    persist_reservation(tx, &reservation).await?;
    Ok(Some(reservation))
}

pub(super) async fn finalize(
    tx: &mut PostgresTransaction,
    input: FinalizeRequestFundsInput,
) -> Result<Option<StoredRequestFundsReservation>, DataLayerError> {
    // This lock is acquired before all financial locks, matching legacy settlement.
    let usage = sqlx::query(
        "SELECT user_id, api_key_id, provider_id, status, billing_status FROM \"usage\" WHERE request_id = $1 FOR UPDATE",
    ).bind(&input.identity.request_id).fetch_optional(&mut **tx).await.map_postgres_err()?;
    let Some(usage) = usage else { return Ok(None) };
    if usage
        .try_get::<Option<String>, _>("user_id")
        .map_postgres_err()?
        != input.identity.user_id
        || usage
            .try_get::<Option<String>, _>("api_key_id")
            .map_postgres_err()?
            != input.identity.api_key_id
        || usage
            .try_get::<Option<String>, _>("provider_id")
            .map_postgres_err()?
            != input.usage.provider_id
        || usage.try_get::<String, _>("status").map_postgres_err()? != input.usage.status
    {
        return Err(invalid(
            "persisted usage does not match request funds terminal identity",
        ));
    }
    let wallet_id: Option<String> = sqlx::query_scalar(
        "SELECT wallet_id FROM request_fund_reservations WHERE reservation_token = $1",
    )
    .bind(&input.identity.reservation_token)
    .fetch_optional(&mut **tx)
    .await
    .map_postgres_err()?
    .flatten();
    let wallet = match wallet_id.as_deref() {
        Some(id) => sqlx::query("SELECT id, balance::double precision AS balance, gift_balance::double precision AS gift_balance, total_consumed::double precision AS total_consumed FROM wallets WHERE id = $1 FOR UPDATE")
            .bind(id).fetch_optional(&mut **tx).await.map_postgres_err()?,
        None => None,
    };
    // Frozen grants remain valid for their admitted day even after expiry or policy changes.
    sqlx::query("SELECT e.id FROM user_plan_entitlements e WHERE e.id IN \
        (SELECT source_id FROM request_fund_allocations WHERE reservation_token = $1 AND source_kind = 'entitlement') \
        ORDER BY e.expires_at, e.created_at, e.id FOR UPDATE")
        .bind(&input.identity.reservation_token).fetch_all(&mut **tx).await.map_postgres_err()?;
    let Some(mut reservation) = identity_reservation(tx, &input.identity).await? else {
        return Ok(None);
    };
    let actual = request_funds_authorized_units(input.usage.actual_total_cost_usd)?;
    if reservation.settlement.is_some() {
        if reservation.actual_cost_units != Some(actual)
            || reservation.reconciliation_facts != input.reconciliation_facts
        {
            return Err(invalid("conflicting request funds terminal cost or facts"));
        }
        return Ok(Some(reservation));
    }
    if reservation.state != RequestFundsState::Dispatched {
        return Err(invalid("only dispatched funds may settle billable usage"));
    }
    let prior_status: String = usage.try_get("billing_status").map_postgres_err()?;
    if matches!(prior_status.as_str(), "settled" | "void") {
        return Err(invalid(
            "usage has already settled outside its funds reservation",
        ));
    }
    if !matches!(input.usage.status.as_str(), "completed" | "cancelled") {
        return Err(invalid(
            "non-billable terminal must explicitly release request funds",
        ));
    }
    let collectible = actual.min(reservation.quote.authorized_cost_units);
    let mut remaining = collectible;
    let mut wallet_debit = 0_u64;
    let mut recharge_debit = 0_u64;
    let mut gift_debit = 0_u64;
    for (ordinal, allocation) in reservation.allocations.iter().enumerate() {
        let amount = remaining.min(allocation.reserved_cost_units);
        remaining -= amount;
        match &allocation.source {
            RequestFundingSource::WalletRecharge { .. } => {
                recharge_debit += amount;
                wallet_debit += amount;
            }
            RequestFundingSource::WalletGift { .. } => {
                gift_debit += amount;
                wallet_debit += amount;
            }
            RequestFundingSource::Postpaid { .. } => {
                wallet_debit += amount;
            }
            RequestFundingSource::Entitlement {
                entitlement_id,
                usage_date,
                quota_cost_units,
            } if amount > 0 => {
                let existing: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM entitlement_usage_ledgers WHERE user_entitlement_id = $1 AND request_id = $2)")
                    .bind(entitlement_id).bind(&input.identity.request_id).fetch_one(&mut **tx).await.map_postgres_err()?;
                if existing {
                    return Err(invalid(
                        "request funds entitlement was already charged outside reservation",
                    ));
                }
                let spent: f64 = sqlx::query_scalar("SELECT COALESCE(SUM(amount_usd), 0)::double precision FROM entitlement_usage_ledgers WHERE user_entitlement_id = $1 AND usage_date = $2")
                    .bind(entitlement_id).bind(usage_date).fetch_one(&mut **tx).await.map_postgres_err()?;
                let balance_before =
                    quota_cost_units.saturating_sub(request_funds_authorized_units(spent)?);
                let balance_after = balance_before
                    .checked_sub(amount)
                    .ok_or_else(|| invalid("frozen entitlement funds are missing"))?;
                sqlx::query("INSERT INTO entitlement_usage_ledgers (id, user_entitlement_id, user_id, request_id, amount_usd, balance_before, balance_after, usage_date, created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,NOW())")
                    .bind(uuid::Uuid::new_v4().to_string()).bind(entitlement_id).bind(&input.identity.user_id)
                    .bind(&input.identity.request_id).bind(request_funds_usd(amount))
                    .bind(request_funds_usd(balance_before))
                    .bind(request_funds_usd(balance_after)).bind(usage_date)
                    .execute(&mut **tx).await.map_postgres_err()?;
            }
            RequestFundingSource::Entitlement { .. } => {}
        }
        sqlx::query("UPDATE request_fund_allocations SET collected_cost_units = $3 WHERE reservation_token = $1 AND ordinal = $2")
            .bind(&input.identity.reservation_token).bind(ordinal as i32).bind(amount as i64)
            .execute(&mut **tx).await.map_postgres_err()?;
    }
    if remaining != 0 {
        return Err(invalid(
            "stored request funds allocations do not cover authorization",
        ));
    }
    let finalized_at = input
        .usage
        .finalized_at_unix_secs
        .unwrap_or_else(|| chrono::Utc::now().timestamp().max(0) as u64);
    let mut settlement = StoredUsageSettlement {
        request_id: input.identity.request_id.clone(),
        wallet_id: wallet_id.clone(),
        billing_status: "settled".to_string(),
        wallet_balance_before: None,
        wallet_balance_after: None,
        wallet_recharge_balance_before: None,
        wallet_recharge_balance_after: None,
        wallet_gift_balance_before: None,
        wallet_gift_balance_after: None,
        provider_monthly_used_usd: None,
        finalized_at_unix_secs: Some(finalized_at),
    };
    if let Some(wallet) = wallet {
        let recharge: f64 = wallet.try_get("balance").map_postgres_err()?;
        let gift: f64 = wallet.try_get("gift_balance").map_postgres_err()?;
        let total_consumed: f64 = wallet.try_get("total_consumed").map_postgres_err()?;
        // The reservation owns these units; other debit paths must have preserved them.
        if request_funds_available_units(recharge.max(0.0))? < recharge_debit
            || request_funds_available_units(gift)? < gift_debit
        {
            return Err(invalid(
                "reserved wallet funds are missing; reconciliation required",
            ));
        }
        let after_recharge = if recharge_debit == 0 {
            recharge
        } else {
            request_funds_usd(request_funds_available_units(recharge)? - recharge_debit)
        };
        let after_gift = if gift_debit == 0 {
            gift
        } else {
            request_funds_usd(request_funds_available_units(gift)? - gift_debit)
        };
        validate_wallet_settlement_values(
            after_recharge,
            after_gift,
            total_consumed,
            request_funds_usd(wallet_debit),
        )?;
        sqlx::query("UPDATE wallets SET balance = $2, gift_balance = $3, total_consumed = $4, updated_at = NOW() WHERE id = $1")
            .bind(&wallet_id).bind(after_recharge).bind(after_gift).bind(total_consumed + request_funds_usd(wallet_debit))
            .execute(&mut **tx).await.map_postgres_err()?;
        settlement.wallet_balance_before = Some(recharge + gift);
        settlement.wallet_balance_after = Some(after_recharge + after_gift);
        settlement.wallet_recharge_balance_before = Some(recharge);
        settlement.wallet_recharge_balance_after = Some(after_recharge);
        settlement.wallet_gift_balance_before = Some(gift);
        settlement.wallet_gift_balance_after = Some(after_gift);
    } else if wallet_debit > 0 {
        return Err(invalid("reserved wallet no longer exists"));
    }
    if let Some(provider) = input
        .usage
        .provider_id
        .as_deref()
        .filter(|id| !id.is_empty())
    {
        super::enqueue_provider_monthly_usage_delta(
            &mut **tx,
            &input.identity.request_id,
            provider,
            input.usage.actual_total_cost_usd,
        )
        .await?;
    }
    super::sync_usage_settlement_snapshot(&mut **tx, &settlement).await?;
    sqlx::query(super::FINALIZE_USAGE_BILLING_SQL)
        .bind(&input.identity.request_id)
        .bind("settled")
        .bind(finalized_at as i64)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
    reservation.state = if actual > collectible || input.reconciliation_facts.is_some() {
        RequestFundsState::ReconciliationPending
    } else {
        RequestFundsState::Settled
    };
    reservation.actual_cost_units = Some(actual);
    reservation.collected_cost_units = collectible;
    reservation.reconciliation_facts = input.reconciliation_facts;
    reservation.settlement = Some(settlement);
    persist_reservation(tx, &reservation).await?;
    Ok(Some(reservation))
}

pub(super) async fn recover(
    tx: &mut PostgresTransaction,
    input: RecoverInsufficientQuotaInput,
) -> Result<Option<RequestFundsRecoveryOutcome>, DataLayerError> {
    let row = sqlx::query(
        "SELECT u.user_id, u.api_key_id, u.provider_id, u.status, \
         COALESCE(s.billing_status, u.billing_status) AS billing_status, \
         COALESCE(s.billing_actual_total_cost_usd, u.actual_total_cost_usd)::double precision AS actual_cost, \
         COALESCE(s.settlement_snapshot, (u.request_metadata->'settlement_snapshot')::jsonb) AS price_evidence \
         FROM \"usage\" u LEFT JOIN usage_settlement_snapshots s ON s.request_id = u.request_id \
         WHERE u.request_id = $1 FOR UPDATE OF u",
    ).bind(&input.request_id).fetch_optional(&mut **tx).await.map_postgres_err()?;
    let Some(row) = row else { return Ok(None) };
    let status: String = row.try_get("billing_status").map_postgres_err()?;
    let previous = sqlx::query("SELECT * FROM request_fund_recoveries WHERE request_id = $1")
        .bind(&input.request_id)
        .fetch_optional(&mut **tx)
        .await
        .map_postgres_err()?;
    if status != "insufficient_quota" && !(status == "settled" && previous.is_some()) {
        return Err(invalid(
            "only documented insufficient-quota liabilities are recoverable",
        ));
    }
    let price_evidence: Option<Value> = row.try_get("price_evidence").map_postgres_err()?;
    if previous.is_none()
        && price_evidence
            .as_ref()
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str)
            != Some("complete")
    {
        return Err(invalid(
            "historical usage requires complete frozen pricing evidence before recovery",
        ));
    }
    let actual: Option<f64> = row.try_get("actual_cost").map_postgres_err()?;
    let actual = request_funds_authorized_units(
        actual.ok_or_else(|| invalid("historical usage has no evidenced actual cost"))?,
    )?;
    let api_key_id: Option<String> = row.try_get("api_key_id").map_postgres_err()?;
    let standalone = match api_key_id.as_deref() {
        Some(key) => {
            sqlx::query_scalar::<_, bool>("SELECT is_standalone FROM api_keys WHERE id = $1")
                .bind(key)
                .fetch_optional(&mut **tx)
                .await
                .map_postgres_err()?
                .unwrap_or(false)
        }
        None => false,
    };
    let identity = RequestFundsIdentity {
        reservation_token: format!("recovery:{}", input.request_id),
        request_id: input.request_id.clone(),
        user_id: row.try_get("user_id").map_postgres_err()?,
        api_key_id,
        api_key_is_standalone: standalone,
    };
    let Some(wallet) = locked_wallet(tx, &identity).await? else {
        return Ok(None);
    };
    let wallet_id: String = wallet.try_get("id").map_postgres_err()?;
    let wallet_status: String = wallet.try_get("status").map_postgres_err()?;
    if wallet_status != "active" {
        return Err(invalid("recovery wallet is unavailable"));
    }
    let (frozen_actual, prior_entitlement, previously_collected) = if let Some(previous) = previous
    {
        if previous
            .try_get::<String, _>("wallet_id")
            .map_postgres_err()?
            != wallet_id
        {
            return Err(invalid("recovery wallet identity changed"));
        }
        let frozen = previous
            .try_get::<i64, _>("frozen_actual_cost_units")
            .map_postgres_err()? as u64;
        if frozen != actual {
            return Err(invalid("frozen recovery cost changed"));
        }
        (
            frozen,
            previous
                .try_get::<i64, _>("prior_entitlement_cost_units")
                .map_postgres_err()? as u64,
            previous
                .try_get::<i64, _>("collected_cost_units")
                .map_postgres_err()? as u64,
        )
    } else {
        let prior: f64 = sqlx::query_scalar("SELECT COALESCE(SUM(amount_usd), 0)::double precision FROM entitlement_usage_ledgers WHERE request_id = $1")
            .bind(&input.request_id).fetch_one(&mut **tx).await.map_postgres_err()?;
        let prior = request_funds_authorized_units(prior)?;
        if prior > actual {
            return Err(invalid(
                "historical entitlement charges exceed frozen request cost",
            ));
        }
        sqlx::query("INSERT INTO request_fund_recoveries (request_id, wallet_id, frozen_actual_cost_units, prior_entitlement_cost_units) VALUES ($1,$2,$3,$4)")
            .bind(&input.request_id).bind(&wallet_id).bind(actual as i64).bind(prior as i64)
            .execute(&mut **tx).await.map_postgres_err()?;
        (actual, prior, 0)
    };
    let due = frozen_actual
        .checked_sub(prior_entitlement)
        .and_then(|cost| cost.checked_sub(previously_collected))
        .ok_or_else(|| invalid("invalid historical recovery receipt totals"))?;
    let recharge: f64 = wallet.try_get("balance").map_postgres_err()?;
    let gift: f64 = wallet.try_get("gift_balance").map_postgres_err()?;
    let consumed: f64 = wallet.try_get("total_consumed").map_postgres_err()?;
    let (available_recharge, available_gift) =
        wallet_available_units(tx, &wallet_id, recharge, gift).await?;
    let collect = due.min(available_recharge + available_gift);
    let recharge_debit = collect.min(available_recharge);
    let gift_debit = collect - recharge_debit;
    let after_recharge = if recharge_debit == 0 {
        recharge
    } else {
        request_funds_usd(request_funds_available_units(recharge)? - recharge_debit)
    };
    let after_gift = if gift_debit == 0 {
        gift
    } else {
        request_funds_usd(request_funds_available_units(gift)? - gift_debit)
    };
    validate_wallet_settlement_values(
        after_recharge,
        after_gift,
        consumed,
        request_funds_usd(collect),
    )?;
    let existing = sqlx::query(super::FIND_USAGE_FOR_SETTLEMENT_SQL)
        .bind(&input.request_id)
        .fetch_one(&mut **tx)
        .await
        .map_postgres_err()?;
    let mut settlement = super::settlement_from_row(&existing)?;
    if collect > 0 {
        sqlx::query("UPDATE wallets SET balance = $2, gift_balance = $3, total_consumed = $4, updated_at = NOW() WHERE id = $1")
            .bind(&wallet_id).bind(after_recharge).bind(after_gift).bind(consumed + request_funds_usd(collect))
            .execute(&mut **tx).await.map_postgres_err()?;
        sqlx::query("UPDATE request_fund_recoveries SET collected_cost_units = collected_cost_units + $2, updated_at = NOW() WHERE request_id = $1")
            .bind(&input.request_id).bind(collect as i64).execute(&mut **tx).await.map_postgres_err()?;
        sqlx::query("INSERT INTO request_fund_collection_receipts (id, request_id, collected_cost_units, recharge_before, recharge_after, gift_before, gift_after) VALUES ($1,$2,$3,$4,$5,$6,$7)")
            .bind(uuid::Uuid::new_v4().to_string()).bind(&input.request_id).bind(collect as i64)
            .bind(recharge).bind(after_recharge).bind(gift).bind(after_gift)
            .execute(&mut **tx).await.map_postgres_err()?;
        settlement.wallet_id = Some(wallet_id);
        settlement.wallet_balance_before = Some(recharge + gift);
        settlement.wallet_balance_after = Some(after_recharge + after_gift);
        settlement.wallet_recharge_balance_before = Some(recharge);
        settlement.wallet_recharge_balance_after = Some(after_recharge);
        settlement.wallet_gift_balance_before = Some(gift);
        settlement.wallet_gift_balance_after = Some(after_gift);
    }
    let outstanding = due - collect;
    if outstanding == 0 && status != "settled" {
        settlement.billing_status = "settled".to_string();
        let finalized_at = chrono::Utc::now().timestamp();
        settlement.finalized_at_unix_secs = Some(finalized_at.max(0) as u64);
        let provider: Option<String> = row.try_get("provider_id").map_postgres_err()?;
        if let Some(provider) = provider.as_deref() {
            super::enqueue_provider_monthly_usage_delta(
                &mut **tx,
                &input.request_id,
                provider,
                request_funds_usd(frozen_actual),
            )
            .await?;
        }
        sqlx::query(super::FINALIZE_USAGE_BILLING_SQL)
            .bind(&input.request_id)
            .bind("settled")
            .bind(finalized_at)
            .execute(&mut **tx)
            .await
            .map_postgres_err()?;
    }
    if collect > 0 || outstanding == 0 {
        super::sync_usage_settlement_snapshot(&mut **tx, &settlement).await?;
    }
    Ok(Some(RequestFundsRecoveryOutcome {
        request_id: input.request_id,
        collected_cost_units: previously_collected + collect,
        outstanding_cost_units: outstanding,
        settlement,
    }))
}
