use thiserror::Error;

/// Constant: billing storage precision.
pub const BILLING_STORAGE_PRECISION: u32 = 8;
/// Constant: billing display precision.
pub const BILLING_DISPLAY_PRECISION: u32 = 6;

/// Enumeration: quantization error.
#[derive(Debug, Error)]
/// Error returned when a value cannot be safely quantized.
pub enum PrecisionError {
    /// Variant: non-finite input.
    #[error("non-finite value rejected at quantization: {0}")]
    NonFinite(f64),
    /// Variant: non-finite result.
    #[error("non-finite quantized result for finite input: {0}")]
    QuantizedOverflow(f64),
}

/// Function: quantize value.
///
/// Fails closed on any non-finite input or non-finite intermediate result
/// instead of silently passing NaN/±inf through to billing storage.
pub fn quantize_value(value: f64, precision: u32) -> Result<f64, PrecisionError> {
    if !value.is_finite() {
        return Err(PrecisionError::NonFinite(value));
    }
    let factor = 10_f64.powi(precision as i32);
    let quantized = (value * factor).round() / factor;
    if !quantized.is_finite() {
        return Err(PrecisionError::QuantizedOverflow(value));
    }
    Ok(quantized)
}

/// Function: quantize cost.
pub fn quantize_cost(value: f64) -> Result<f64, PrecisionError> {
    quantize_value(value, BILLING_STORAGE_PRECISION)
}

/// Function: quantize display.
pub fn quantize_display(value: f64) -> Result<f64, PrecisionError> {
    quantize_value(value, BILLING_DISPLAY_PRECISION)
}

#[cfg(test)]
mod tests {
    use super::{
        quantize_cost, quantize_display, quantize_value, PrecisionError, BILLING_STORAGE_PRECISION,
    };

    #[test]
    fn quantizes_cost_to_storage_precision() {
        assert_eq!(quantize_cost(1.234567891).expect("finite"), 1.23456789);
    }

    #[test]
    fn quantizes_display_to_display_precision() {
        assert_eq!(quantize_display(1.23456789).expect("finite"), 1.234568);
    }

    #[test]
    fn rejects_nan_input() {
        let error = quantize_cost(f64::NAN).expect_err("NaN must be rejected");
        assert!(matches!(error, PrecisionError::NonFinite(_)));
    }

    #[test]
    fn rejects_positive_and_negative_infinity() {
        for value in [f64::INFINITY, f64::NEG_INFINITY] {
            let error = quantize_cost(value).expect_err("infinity must be rejected");
            assert!(matches!(error, PrecisionError::NonFinite(_)));
        }
    }

    #[test]
    fn rejects_multiplication_overflow_of_finite_input() {
        // A finite input near f64::MAX overflows when scaled by the precision factor.
        let error = quantize_value(f64::MAX / 2.0, BILLING_STORAGE_PRECISION)
            .expect_err("overflowing quantization must be rejected");
        assert!(matches!(error, PrecisionError::QuantizedOverflow(_)));
    }

    #[test]
    fn preserves_zero_negative_and_boundary_finite_values() {
        assert_eq!(quantize_cost(0.0).expect("zero"), 0.0);
        assert_eq!(quantize_cost(-1.5).expect("negative"), -1.5);
        let boundary = quantize_cost(f64::MAX / 1_000_000_000.0).expect("large finite");
        assert!(boundary.is_finite());
    }
}
