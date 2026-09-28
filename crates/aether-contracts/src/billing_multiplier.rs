use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Error returned when a billing multiplier value cannot be represented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BillingMultiplierError {
    /// Non-finite or negative input.
    InvalidValue(String),
    /// Integer units outside the representable range.
    OutOfRange(i32),
}

impl std::fmt::Display for BillingMultiplierError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidValue(message) => write!(formatter, "{message}"),
            Self::OutOfRange(units) => {
                write!(formatter, "billing multiplier units out of range: {units}")
            }
        }
    }
}

impl std::error::Error for BillingMultiplierError {}

/// Unit scale for fixed-point billing multipliers: 4 decimal places.
pub const BILLING_MULTIPLIER_SCALE: i64 = 10_000;
/// Maximum representable multiplier (9999.9999) in 1e-4 units.
pub const BILLING_MULTIPLIER_MAX_UNITS: i32 = 99_999_999;
/// Default clamp floor for administrator-configured multipliers (0.01).
pub const BILLING_MULTIPLIER_CLAMP_MIN_UNITS: i32 = 100;
/// Default clamp ceiling for administrator-configured multipliers (100.0).
pub const BILLING_MULTIPLIER_CLAMP_MAX_UNITS: i32 = 1_000_000;

/// Fixed-point billing multiplier stored as integer 1e-4 units.
///
/// The value is always exact at 4 decimal places in the inclusive range
/// 0.0001..=9999.9999, with 0 reserved for the explicit free (零费率) case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BillingMultiplier(i32);

impl BillingMultiplier {
    /// Constant: explicit free-tier multiplier (0.0).
    pub const ZERO: Self = Self(0);
    /// Constant: neutral multiplier (1.0).
    pub const ONE: Self = Self(BILLING_MULTIPLIER_SCALE as i32);
    /// Constant: multiplier used when nothing is configured.
    pub const DEFAULT: Self = Self::ONE;

    /// Parses a finite non-negative decimal, rounding half-away-from-zero to
    /// 4 decimal places. Rejects non-finite, negative, or out-of-range input.
    pub fn from_f64_rounded(value: f64) -> Result<Self, BillingMultiplierError> {
        if !value.is_finite() || value < 0.0 {
            return Err(BillingMultiplierError::InvalidValue(format!(
                "billing multiplier must be a finite value >= 0: {value}"
            )));
        }
        let units = (value * BILLING_MULTIPLIER_SCALE as f64).round();
        if units > BILLING_MULTIPLIER_MAX_UNITS as f64 {
            return Err(BillingMultiplierError::InvalidValue(format!(
                "billing multiplier exceeds {}",
                BILLING_MULTIPLIER_MAX_UNITS as f64 / BILLING_MULTIPLIER_SCALE as f64
            )));
        }
        Ok(Self(units as i32))
    }

    /// Function: builds a multiplier from raw 1e-4 units.
    pub fn from_units(units: i32) -> Result<Self, BillingMultiplierError> {
        if !(0..=BILLING_MULTIPLIER_MAX_UNITS).contains(&units) {
            return Err(BillingMultiplierError::OutOfRange(units));
        }
        Ok(Self(units))
    }

    /// Method: raw 1e-4 units.
    pub fn units(self) -> i32 {
        self.0
    }

    /// Exact value as f64 for multiplication against the quantized cost
    /// pipeline. The representation itself never carries rounding error.
    pub fn to_f64(self) -> f64 {
        f64::from(self.0) / BILLING_MULTIPLIER_SCALE as f64
    }

    /// Method: whether this is the explicit free-tier multiplier.
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Method: whether this is the neutral default multiplier.
    pub fn is_one(self) -> bool {
        self.0 == Self::ONE.0
    }

    /// Risk-control clamp: values outside [min, max] are pulled to the
    /// nearest boundary. An explicit 0 (free tier) always passes through.
    pub fn clamp(self, min: Self, max: Self) -> Self {
        if self.is_zero() {
            return self;
        }
        Self(self.0.clamp(min.0, max.0))
    }
}

impl Default for BillingMultiplier {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Serialize for BillingMultiplier {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.to_f64())
    }
}

impl<'de> Deserialize<'de> for BillingMultiplier {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = f64::deserialize(deserializer)?;
        BillingMultiplier::from_f64_rounded(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_rounds_to_four_decimals() {
        let value = BillingMultiplier::from_f64_rounded(1.23456).expect("finite");
        assert_eq!(value.units(), 12_346);
        assert_eq!(value.to_f64(), 1.2346);
    }

    #[test]
    fn boundary_values_are_legal() {
        let min = BillingMultiplier::from_f64_rounded(0.0001).expect("min");
        assert_eq!(min.units(), 1);
        let max = BillingMultiplier::from_f64_rounded(9999.9999).expect("max");
        assert_eq!(max.units(), BILLING_MULTIPLIER_MAX_UNITS);
        assert!(BillingMultiplier::from_f64_rounded(10_000.0).is_err());
        assert!(BillingMultiplier::from_f64_rounded(f64::NAN).is_err());
        assert!(BillingMultiplier::from_f64_rounded(-0.5).is_err());
    }

    #[test]
    fn clamp_pulls_to_bounds_but_keeps_explicit_zero() {
        let min = BillingMultiplier::from_units(100).expect("min");
        let max = BillingMultiplier::from_units(1_000_000).expect("max");
        assert_eq!(
            BillingMultiplier::from_f64_rounded(0.001)
                .expect("value")
                .clamp(min, max),
            min
        );
        assert_eq!(
            BillingMultiplier::from_f64_rounded(500.0)
                .expect("value")
                .clamp(min, max),
            max
        );
        assert!(BillingMultiplier::ZERO.clamp(min, max).is_zero());
    }

    #[test]
    fn serde_round_trips_as_number() {
        let value = BillingMultiplier::from_f64_rounded(2.5).expect("value");
        let json = serde_json::to_string(&value).expect("serialize");
        assert_eq!(json, "2.5");
        let decoded: BillingMultiplier = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, value);
    }
}
