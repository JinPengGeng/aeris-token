use std::collections::BTreeMap;

use super::InMemorySettlementRepository;
use crate::repository::wallet::StoredWalletSnapshot;
use crate::DataLayerError;
use aether_data_contracts::repository::settlement::*;

fn invalid(message: &str) -> DataLayerError {
    DataLayerError::InvalidInput(message.to_string())
}

pub(crate) fn held_wallet_units(
    reservations: &BTreeMap<String, StoredRequestFundsReservation>,
    wallet_id: &str,
) -> (u64, u64) {
    let mut recharge = 0;
    let mut gift = 0;
    for reservation in reservations
        .values()
        .filter(|reservation| reservation.state.holds_funds() && reservation.settlement.is_none())
    {
        let mut collected = reservation.collected_cost_units;
        for allocation in &reservation.allocations {
            let used = collected.min(allocation.reserved_cost_units);
            collected -= used;
            let held = allocation.reserved_cost_units - used;
            match &allocation.source {
                RequestFundingSource::WalletRecharge { wallet_id: id } if id == wallet_id => {
                    recharge += held
                }
                RequestFundingSource::WalletGift { wallet_id: id } if id == wallet_id => {
                    gift += held
                }
                _ => {}
            }
        }
    }
    (recharge, gift)
}

fn wallet_id(
    wallets: &BTreeMap<String, StoredWalletSnapshot>,
    identity: &RequestFundsIdentity,
) -> Option<String> {
    identity
        .api_key_id
        .as_ref()
        .and_then(|key| {
            wallets
                .values()
                .find(|wallet| wallet.api_key_id.as_ref() == Some(key))
        })
        .or_else(|| {
            (!identity.api_key_is_standalone)
                .then(|| {
                    wallets.values().find(|wallet| {
                        wallet.user_id == identity.user_id && wallet.api_key_id.is_none()
                    })
                })
                .flatten()
        })
        .map(|wallet| wallet.id.clone())
}

pub(super) fn reserve(
    repo: &InMemorySettlementRepository,
    input: ReserveRequestFundsInput,
) -> Result<ReserveRequestFundsOutcome, DataLayerError> {
    input.validate()?;
    repo.wallets.with_mut(|wallets| {
        let mut funds = repo.funds.write().expect("request funds lock");
        if let Some(stored) = funds.get(&input.identity.reservation_token) {
            return Ok(if stored.quote == input {
                ReserveRequestFundsOutcome::Reserved {
                    reservation: stored.clone(),
                }
            } else {
                ReserveRequestFundsOutcome::Conflict
            });
        }
        if funds
            .values()
            .any(|stored| stored.quote.identity.request_id == input.identity.request_id)
        {
            return Ok(ReserveRequestFundsOutcome::Conflict);
        }
        let id = wallet_id(wallets, &input.identity);
        let mut allocations = Vec::new();
        let mut remaining = input.authorized_cost_units;
        if let Some(wallet) = id.as_ref().and_then(|id| wallets.get(id)) {
            if wallet.status != "active" {
                return Ok(ReserveRequestFundsOutcome::WalletUnavailable);
            }
            if wallet.limit_mode.eq_ignore_ascii_case("unlimited") {
                if remaining > 0 {
                    allocations.push(RequestFundsAllocation {
                        source: RequestFundingSource::Postpaid {
                            wallet_id: wallet.id.clone(),
                        },
                        reserved_cost_units: remaining,
                    });
                    remaining = 0;
                }
            } else if wallet.balance >= 0.0 {
                let (held_recharge, held_gift) = held_wallet_units(&funds, &wallet.id);
                let recharge =
                    request_funds_available_units(wallet.balance)?.saturating_sub(held_recharge);
                let gift =
                    request_funds_available_units(wallet.gift_balance)?.saturating_sub(held_gift);
                for (source, available) in [
                    (
                        RequestFundingSource::WalletRecharge {
                            wallet_id: wallet.id.clone(),
                        },
                        recharge,
                    ),
                    (
                        RequestFundingSource::WalletGift {
                            wallet_id: wallet.id.clone(),
                        },
                        gift,
                    ),
                ] {
                    let amount = remaining.min(available);
                    remaining -= amount;
                    if amount > 0 {
                        allocations.push(RequestFundsAllocation {
                            source,
                            reserved_cost_units: amount,
                        });
                    }
                }
            }
        }
        if remaining > 0 {
            return Ok(ReserveRequestFundsOutcome::Insufficient {
                available_cost_units: input.authorized_cost_units - remaining,
            });
        }
        let reservation = StoredRequestFundsReservation {
            quote: input,
            wallet_id: id,
            allocations,
            state: RequestFundsState::Prepared,
            actual_cost_units: None,
            collected_cost_units: 0,
            reconciliation_facts: None,
            settlement: None,
        };
        funds.insert(
            reservation.quote.identity.reservation_token.clone(),
            reservation.clone(),
        );
        Ok(ReserveRequestFundsOutcome::Reserved { reservation })
    })
}

pub(super) fn dispatch(
    repo: &InMemorySettlementRepository,
    identity: RequestFundsIdentity,
) -> Result<Option<StoredRequestFundsReservation>, DataLayerError> {
    identity.validate()?;
    let mut funds = repo.funds.write().expect("request funds lock");
    let Some(reservation) = funds.get_mut(&identity.reservation_token) else {
        return Ok(None);
    };
    if reservation.quote.identity != identity {
        return Err(invalid("request funds identity conflict"));
    }
    match reservation.state {
        RequestFundsState::Prepared | RequestFundsState::Dispatched => {
            reservation.state = RequestFundsState::Dispatched
        }
        _ => return Err(invalid("terminal request funds cannot dispatch")),
    }
    Ok(Some(reservation.clone()))
}

pub(super) fn release(
    repo: &InMemorySettlementRepository,
    input: ReleaseRequestFundsInput,
) -> Result<Option<StoredRequestFundsReservation>, DataLayerError> {
    input.identity.validate()?;
    let mut funds = repo.funds.write().expect("request funds lock");
    let Some(reservation) = funds.get_mut(&input.identity.reservation_token) else {
        return Ok(None);
    };
    if reservation.quote.identity != input.identity {
        return Err(invalid("request funds identity conflict"));
    }
    match reservation.state {
        RequestFundsState::Prepared | RequestFundsState::Released => {}
        RequestFundsState::Dispatched if input.terminal_no_charge => {}
        _ => {
            return Err(invalid(
                "request funds require an authoritative no-charge terminal before release",
            ))
        }
    }
    reservation.state = RequestFundsState::Released;
    reservation.actual_cost_units = Some(0);
    Ok(Some(reservation.clone()))
}

pub(super) fn finalize(
    repo: &InMemorySettlementRepository,
    input: FinalizeRequestFundsInput,
) -> Result<Option<StoredRequestFundsReservation>, DataLayerError> {
    input.validate()?;
    let _guard = repo.settlement_lock.lock().expect("settlement lock");
    repo.wallets.with_mut(|wallets| {
        let mut funds = repo.funds.write().expect("request funds lock");
        let Some(reservation) = funds.get(&input.identity.reservation_token) else {
            return Ok(None);
        };
        if reservation.quote.identity != input.identity {
            return Err(invalid("request funds identity conflict"));
        }
        let actual = request_funds_authorized_units(input.usage.actual_total_cost_usd)?;
        if reservation.settlement.is_some() {
            if reservation.actual_cost_units != Some(actual)
                || reservation.reconciliation_facts != input.reconciliation_facts
            {
                return Err(invalid("conflicting terminal cost or facts"));
            }
            return Ok(Some(reservation.clone()));
        }
        if reservation.state != RequestFundsState::Dispatched
            || !matches!(input.usage.status.as_str(), "completed" | "cancelled")
        {
            return Err(invalid("only dispatched billable usage may settle funds"));
        }
        let mut next = reservation.clone();
        let collect = actual.min(next.quote.authorized_cost_units);
        let mut remaining = collect;
        let mut recharge_debit = 0;
        let mut gift_debit = 0;
        let mut wallet_debit = 0;
        for allocation in &next.allocations {
            let amount = remaining.min(allocation.reserved_cost_units);
            remaining -= amount;
            match allocation.source {
                RequestFundingSource::WalletRecharge { .. } => {
                    recharge_debit += amount;
                    wallet_debit += amount;
                }
                RequestFundingSource::WalletGift { .. } => {
                    gift_debit += amount;
                    wallet_debit += amount;
                }
                RequestFundingSource::Postpaid { .. } => wallet_debit += amount,
                RequestFundingSource::Entitlement { .. } => {
                    return Err(invalid("memory funds do not implement entitlement storage"))
                }
            }
        }
        if remaining > 0 {
            return Err(invalid("incomplete funding allocation"));
        }
        let mut settlement = StoredUsageSettlement {
            request_id: input.identity.request_id.clone(),
            wallet_id: next.wallet_id.clone(),
            billing_status: "settled".to_string(),
            wallet_balance_before: None,
            wallet_balance_after: None,
            wallet_recharge_balance_before: None,
            wallet_recharge_balance_after: None,
            wallet_gift_balance_before: None,
            wallet_gift_balance_after: None,
            provider_monthly_used_usd: None,
            finalized_at_unix_secs: input.usage.finalized_at_unix_secs,
        };
        let mut next_wallet = next
            .wallet_id
            .as_ref()
            .and_then(|id| wallets.get(id))
            .cloned();
        if let Some(wallet) = next_wallet.as_mut() {
            let before = wallet.balance;
            let before_gift = wallet.gift_balance;
            if recharge_debit > 0 {
                wallet.balance = request_funds_usd(
                    request_funds_available_units(before)?
                        .checked_sub(recharge_debit)
                        .ok_or_else(|| invalid("reserved funds missing"))?,
                );
            }
            if gift_debit > 0 {
                wallet.gift_balance = request_funds_usd(
                    request_funds_available_units(before_gift)?
                        .checked_sub(gift_debit)
                        .ok_or_else(|| invalid("reserved gift missing"))?,
                );
            }
            validate_wallet_settlement_values(
                wallet.balance,
                wallet.gift_balance,
                wallet.total_consumed,
                request_funds_usd(wallet_debit),
            )?;
            wallet.total_consumed += request_funds_usd(wallet_debit);
            settlement.wallet_balance_before = Some(before + before_gift);
            settlement.wallet_balance_after = Some(wallet.balance + wallet.gift_balance);
            settlement.wallet_recharge_balance_before = Some(before);
            settlement.wallet_recharge_balance_after = Some(wallet.balance);
            settlement.wallet_gift_balance_before = Some(before_gift);
            settlement.wallet_gift_balance_after = Some(wallet.gift_balance);
        } else if wallet_debit > 0 {
            return Err(invalid("reserved wallet missing"));
        }
        let mut provider_quotas = repo
            .provider_monthly_used
            .write()
            .expect("provider quota lock");
        if let Some(provider) = input.usage.provider_id.as_ref() {
            let amount = provider_quotas.get(provider).copied().unwrap_or(0.0)
                + input.usage.actual_total_cost_usd;
            if !amount.is_finite() {
                return Err(invalid("provider cost overflow"));
            }
            provider_quotas.insert(provider.clone(), amount);
            settlement.provider_monthly_used_usd = Some(amount);
        }
        if let Some(wallet) = next_wallet {
            wallets.insert(wallet.id.clone(), wallet);
        }
        next.actual_cost_units = Some(actual);
        next.collected_cost_units = collect;
        let has_reconciliation_facts = input.reconciliation_facts.is_some();
        next.reconciliation_facts = input.reconciliation_facts;
        next.settlement = Some(settlement.clone());
        next.state = if actual > collect || has_reconciliation_facts {
            RequestFundsState::ReconciliationPending
        } else {
            RequestFundsState::Settled
        };
        repo.settlements
            .write()
            .expect("settlement snapshot lock")
            .insert(settlement.request_id.clone(), settlement);
        funds.insert(input.identity.reservation_token, next.clone());
        Ok(Some(next))
    })
}

pub(super) fn recover(
    repo: &InMemorySettlementRepository,
    input: RecoverInsufficientQuotaInput,
) -> Result<Option<RequestFundsRecoveryOutcome>, DataLayerError> {
    let _guard = repo.settlement_lock.lock().expect("settlement lock");
    let Some(usage) = repo
        .recoverable_usage
        .read()
        .expect("recoverable usage lock")
        .get(&input.request_id)
        .cloned()
    else {
        return Ok(None);
    };
    let actual = request_funds_authorized_units(usage.actual_total_cost_usd)?;
    repo.wallets.with_mut(|wallets| {
        let identity = RequestFundsIdentity {
            reservation_token: String::new(),
            request_id: input.request_id.clone(),
            user_id: usage.user_id.clone(),
            api_key_id: usage.api_key_id.clone(),
            api_key_is_standalone: usage.api_key_is_standalone,
        };
        let Some(id) = wallet_id(wallets, &identity) else {
            return Ok(None);
        };
        let wallet = wallets.get(&id).expect("resolved recovery wallet");
        if wallet.status != "active" {
            return Err(invalid("recovery wallet is unavailable"));
        }
        let mut next_wallet = wallet.clone();
        let mut recovered = repo.recovered_units.write().expect("recovered funds lock");
        let previous = recovered.get(&input.request_id).copied().unwrap_or(0);
        let due = actual
            .checked_sub(previous)
            .ok_or_else(|| invalid("invalid recovery receipt total"))?;
        let funds = repo.funds.read().expect("request funds lock");
        let (held_recharge, held_gift) = held_wallet_units(&funds, &id);
        let available_recharge =
            request_funds_available_units(wallet.balance.max(0.0))?.saturating_sub(held_recharge);
        let available_gift =
            request_funds_available_units(wallet.gift_balance)?.saturating_sub(held_gift);
        let collect = if wallet.balance < 0.0 {
            0
        } else {
            due.min(available_recharge + available_gift)
        };
        let recharge_debit = collect.min(available_recharge);
        let gift_debit = collect - recharge_debit;
        if recharge_debit > 0 {
            next_wallet.balance =
                request_funds_usd(request_funds_available_units(wallet.balance)? - recharge_debit);
        }
        if gift_debit > 0 {
            next_wallet.gift_balance =
                request_funds_usd(request_funds_available_units(wallet.gift_balance)? - gift_debit);
        }
        validate_wallet_settlement_values(
            next_wallet.balance,
            next_wallet.gift_balance,
            wallet.total_consumed,
            request_funds_usd(collect),
        )?;
        next_wallet.total_consumed += request_funds_usd(collect);
        let mut settlements = repo.settlements.write().expect("settlement snapshots lock");
        let Some(previous_settlement) = settlements.get(&input.request_id) else {
            return Err(invalid("recovery has no frozen settlement"));
        };
        let mut next_settlement = previous_settlement.clone();
        if collect > 0 {
            next_settlement.wallet_id = Some(id.clone());
            next_settlement.wallet_balance_before = Some(wallet.balance + wallet.gift_balance);
            next_settlement.wallet_balance_after =
                Some(next_wallet.balance + next_wallet.gift_balance);
            next_settlement.wallet_recharge_balance_before = Some(wallet.balance);
            next_settlement.wallet_recharge_balance_after = Some(next_wallet.balance);
            next_settlement.wallet_gift_balance_before = Some(wallet.gift_balance);
            next_settlement.wallet_gift_balance_after = Some(next_wallet.gift_balance);
        }
        if due == collect && previous_settlement.billing_status != "settled" {
            if let Some(provider) = usage.provider_id.as_ref() {
                let mut quotas = repo
                    .provider_monthly_used
                    .write()
                    .expect("provider quota lock");
                let next =
                    quotas.get(provider).copied().unwrap_or(0.0) + usage.actual_total_cost_usd;
                if !next.is_finite() {
                    return Err(invalid("provider cost overflow"));
                }
                quotas.insert(provider.clone(), next);
                next_settlement.provider_monthly_used_usd = Some(next);
            }
            next_settlement.billing_status = "settled".to_string();
        }
        wallets.insert(id, next_wallet);
        recovered.insert(input.request_id.clone(), previous + collect);
        settlements.insert(input.request_id.clone(), next_settlement.clone());
        Ok(Some(RequestFundsRecoveryOutcome {
            request_id: input.request_id,
            collected_cost_units: previous + collect,
            outstanding_cost_units: due - collect,
            settlement: next_settlement,
        }))
    })
}
