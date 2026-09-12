use serde::{Deserialize, Serialize};

use super::{StoredUsageSettlement, UsageSettlementInput};
use crate::DataLayerError;

pub const REQUEST_FUNDS_UNITS_PER_USD: u64 = 100_000_000;
// Wallet balances are currently f64. Below this ceiling a USD value's ULP is
// smaller than one 1e-8 USD unit, permitting canonical decimal round trips.
pub const MAX_REQUEST_FUNDS_UNITS: u64 = (1_u64 << 52) - 1;

pub fn request_funds_available_units(usd: f64) -> Result<u64, DataLayerError> {
    request_funds_units(usd, false)
}

pub fn request_funds_authorized_units(usd: f64) -> Result<u64, DataLayerError> {
    request_funds_units(usd, true)
}

fn request_funds_units(usd: f64, round_up: bool) -> Result<u64, DataLayerError> {
    if !usd.is_finite() || usd < 0.0 {
        return Err(DataLayerError::InvalidInput(
            "request funds amount is not a finite supported nonnegative amount".to_string(),
        ));
    }
    // Interpret the canonical decimal representation. Multiplying an f64 by 1e8
    // and flooring can lose a whole unit for exact eight-decimal wallet values.
    let text = usd.to_string();
    let (mantissa, exponent) = text
        .split_once('e')
        .map_or((text.as_str(), 0_i32), |(m, e)| {
            (
                m,
                e.parse::<i32>()
                    .expect("finite f64 display has a valid exponent"),
            )
        });
    let mut digits = 0_u128;
    let mut places = 0_i32;
    let mut fractional = false;
    for character in mantissa.bytes() {
        if character == b'.' {
            fractional = true;
            continue;
        }
        if character == b'-' {
            continue;
        } // negative zero
        digits = digits
            .checked_mul(10)
            .and_then(|value| value.checked_add((character - b'0') as u128))
            .ok_or_else(|| {
                DataLayerError::InvalidInput(
                    "request funds amount exceeds supported range".to_string(),
                )
            })?;
        if fractional {
            places += 1;
        }
    }
    let shift = exponent - places + 8;
    let units = if shift >= 0 {
        digits.checked_mul(10_u128.checked_pow(shift as u32).unwrap_or(u128::MAX))
    } else {
        match 10_u128.checked_pow((-shift) as u32) {
            Some(divisor) => Some(digits / divisor + u128::from(round_up && digits % divisor != 0)),
            None => Some(u128::from(round_up && digits != 0)),
        }
    }
    .filter(|units| *units <= MAX_REQUEST_FUNDS_UNITS as u128)
    .ok_or_else(|| {
        DataLayerError::InvalidInput("request funds amount exceeds supported range".to_string())
    })?;
    Ok(units as u64)
}

pub fn request_funds_usd(units: u64) -> f64 {
    units as f64 / REQUEST_FUNDS_UNITS_PER_USD as f64
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFundsIdentity {
    pub reservation_token: String,
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub api_key_is_standalone: bool,
}

impl RequestFundsIdentity {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        for (name, value) in [
            ("reservation_token", Some(self.reservation_token.as_str())),
            ("request_id", Some(self.request_id.as_str())),
            ("user_id", self.user_id.as_deref()),
            ("api_key_id", self.api_key_id.as_deref()),
        ] {
            if value.is_some_and(|value| value.trim().is_empty() || value.len() > 128) {
                return Err(DataLayerError::InvalidInput(format!(
                    "request funds {name} must contain 1 to 128 bytes"
                )));
            }
        }
        if self.api_key_is_standalone && self.api_key_id.is_none()
            || !self.api_key_is_standalone && self.user_id.is_none()
        {
            return Err(DataLayerError::InvalidInput(
                "request funds identity is missing its funding owner".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReserveRequestFundsInput {
    pub identity: RequestFundsIdentity,
    pub authorized_cost_units: u64,
    /// A server-produced, non-secret price and dimension snapshot, never raw request content.
    pub pricing_snapshot: serde_json::Value,
    pub admitted_at_unix_secs: u64,
}

impl ReserveRequestFundsInput {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        self.identity.validate()?;
        if self.authorized_cost_units > MAX_REQUEST_FUNDS_UNITS
            || self.admitted_at_unix_secs > i64::MAX as u64
            || !self.pricing_snapshot.is_object()
            || self.pricing_snapshot.to_string().len() > 64 * 1024
        {
            return Err(DataLayerError::InvalidInput(
                "request funds quote has invalid amount, time or pricing snapshot".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestFundsState {
    Prepared,
    Dispatched,
    Settled,
    Released,
    ReconciliationPending,
}

impl RequestFundsState {
    pub fn holds_funds(self) -> bool {
        matches!(
            self,
            Self::Prepared | Self::Dispatched | Self::ReconciliationPending
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Dispatched => "dispatched",
            Self::Settled => "settled",
            Self::Released => "released",
            Self::ReconciliationPending => "reconciliation_pending",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestFundingSource {
    WalletRecharge {
        wallet_id: String,
    },
    WalletGift {
        wallet_id: String,
    },
    Entitlement {
        entitlement_id: String,
        usage_date: String,
        quota_cost_units: u64,
    },
    Postpaid {
        wallet_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFundsAllocation {
    pub source: RequestFundingSource,
    pub reserved_cost_units: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredRequestFundsReservation {
    pub quote: ReserveRequestFundsInput,
    pub wallet_id: Option<String>,
    pub allocations: Vec<RequestFundsAllocation>,
    pub state: RequestFundsState,
    pub actual_cost_units: Option<u64>,
    pub collected_cost_units: u64,
    /// Bounded, non-secret terminal facts retained for reconciliation even when
    /// the collected amount stays within the quote.
    #[serde(default)]
    pub reconciliation_facts: Option<serde_json::Value>,
    pub settlement: Option<StoredUsageSettlement>,
}

pub fn request_funds_wallet_held_units<'a>(
    reservations: impl IntoIterator<Item = &'a StoredRequestFundsReservation>,
    wallet_id: &str,
) -> Result<(u64, u64), DataLayerError> {
    let mut recharge = 0_u64;
    let mut gift = 0_u64;
    for reservation in reservations
        .into_iter()
        .filter(|reservation| reservation.state.holds_funds() && reservation.settlement.is_none())
    {
        let mut collected = reservation.collected_cost_units;
        for allocation in &reservation.allocations {
            let used = collected.min(allocation.reserved_cost_units);
            collected -= used;
            let held = allocation.reserved_cost_units - used;
            let bucket = match &allocation.source {
                RequestFundingSource::WalletRecharge { wallet_id: id } if id == wallet_id => {
                    Some(&mut recharge)
                }
                RequestFundingSource::WalletGift { wallet_id: id } if id == wallet_id => {
                    Some(&mut gift)
                }
                _ => None,
            };
            if let Some(bucket) = bucket {
                *bucket = bucket.checked_add(held).ok_or_else(|| {
                    DataLayerError::UnexpectedValue("request funds holds overflowed".to_string())
                })?;
            }
        }
    }
    Ok((recharge, gift))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReserveRequestFundsOutcome {
    Reserved {
        reservation: StoredRequestFundsReservation,
    },
    Insufficient {
        available_cost_units: u64,
    },
    WalletUnavailable,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRequestFundsInput {
    pub identity: RequestFundsIdentity,
    /// Only the trusted execution terminal path may assert this after dispatch.
    pub terminal_no_charge: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinalizeRequestFundsInput {
    pub identity: RequestFundsIdentity,
    pub usage: UsageSettlementInput,
    /// Server-produced dimensions or anomaly facts. This must never contain
    /// raw request content, credentials, or provider response bodies.
    #[serde(default)]
    pub reconciliation_facts: Option<serde_json::Value>,
}

impl FinalizeRequestFundsInput {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        self.identity.validate()?;
        self.usage.validate()?;
        if self.identity.request_id != self.usage.request_id
            || self.identity.user_id != self.usage.user_id
            || self.identity.api_key_id != self.usage.api_key_id
            || self.identity.api_key_is_standalone != self.usage.api_key_is_standalone
        {
            return Err(DataLayerError::InvalidInput(
                "request funds settlement identity does not match the reserved request".to_string(),
            ));
        }
        if let Some(facts) = &self.reconciliation_facts {
            if !facts.is_object() || facts.to_string().len() > 16 * 1024 {
                return Err(DataLayerError::InvalidInput(
                    "request funds reconciliation facts must be an object under 16 KiB".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverInsufficientQuotaInput {
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestFundsRecoveryOutcome {
    pub request_id: String,
    pub collected_cost_units: u64,
    pub outstanding_cost_units: u64,
    pub settlement: StoredUsageSettlement,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_wallet_units_preserve_eight_digit_money_across_the_supported_range() {
        for units in [0, 1, 29, 100_000_029, MAX_REQUEST_FUNDS_UNITS] {
            let decimal = format!(
                "{}.{:08}",
                units / REQUEST_FUNDS_UNITS_PER_USD,
                units % REQUEST_FUNDS_UNITS_PER_USD
            );
            let value = decimal.parse::<f64>().unwrap();
            assert_eq!(
                request_funds_available_units(value).unwrap(),
                units,
                "{decimal}"
            );
            assert_eq!(
                request_funds_authorized_units(value).unwrap(),
                units,
                "{decimal}"
            );
        }
        let mut sample = 42_u64;
        for _ in 0..4096 {
            sample = sample.wrapping_mul(6364136223846793005).wrapping_add(1);
            let units = sample % MAX_REQUEST_FUNDS_UNITS;
            let decimal = format!(
                "{}.{:08}",
                units / REQUEST_FUNDS_UNITS_PER_USD,
                units % REQUEST_FUNDS_UNITS_PER_USD
            );
            let value = decimal.parse::<f64>().unwrap();
            assert_eq!(
                request_funds_available_units(value).unwrap(),
                units,
                "{decimal}"
            );
            assert_eq!(
                request_funds_authorized_units(value).unwrap(),
                units,
                "{decimal}"
            );
        }
    }

    #[test]
    fn authorization_rounds_up_and_capacity_down_without_accepting_invalid_money() {
        assert_eq!(request_funds_authorized_units(0.000_000_001).unwrap(), 1);
        assert_eq!(request_funds_available_units(0.000_000_001).unwrap(), 0);
        assert_eq!(
            request_funds_authorized_units(f64::MIN_POSITIVE).unwrap(),
            1
        );
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01, f64::MAX] {
            assert!(request_funds_authorized_units(value).is_err());
            assert!(request_funds_available_units(value).is_err());
        }
    }
}
