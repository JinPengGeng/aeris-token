/// Constant: billing storage precision.
pub const BILLING_STORAGE_PRECISION: u32 = 8;
/// Constant: billing display precision.
pub const BILLING_DISPLAY_PRECISION: u32 = 6;

/// Function: quantize value.
pub fn quantize_value(value: f64, precision: u32) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let factor = 10_f64.powi(precision as i32);
    (value * factor).round() / factor
}

/// Function: quantize cost.
pub fn quantize_cost(value: f64) -> f64 {
    quantize_value(value, BILLING_STORAGE_PRECISION)
}

/// Function: quantize display.
pub fn quantize_display(value: f64) -> f64 {
    quantize_value(value, BILLING_DISPLAY_PRECISION)
}

#[cfg(test)]
mod tests {
    use super::{quantize_cost, quantize_display};

    #[test]
    fn quantizes_cost_to_storage_precision() {
        assert_eq!(quantize_cost(1.234567891), 1.23456789);
    }

    #[test]
    fn quantizes_display_to_display_precision() {
        assert_eq!(quantize_display(1.23456789), 1.234568);
    }
}
