//! 金额定点表示(API 边界层)。
//!
//! 内部计算仍可使用 f64,但 API 出入参统一使用 8 位小数字符串定点
//! (如 `"12.34567800"`,单位 USD,最小粒度 1e-8),与 DB 的
//! NUMERIC(20,8) 精度一致,消除浮点展示/比较误差。

use serde::Deserialize;

pub const MONEY_DECIMALS: u32 = 8;
pub const MONEY_SCALE: i64 = 100_000_000; // 1e8 微单位/USD

/// f64 金额 → 微单位(i64),四舍五入到 1e-8,越界饱和。
pub fn money_to_units(value: f64) -> i64 {
    if !value.is_finite() {
        return 0;
    }
    (value * MONEY_SCALE as f64)
        .round()
        .clamp(i64::MIN as f64, i64::MAX as f64) as i64
}

/// 微单位 → f64 金额。
pub fn units_to_money(units: i64) -> f64 {
    units as f64 / MONEY_SCALE as f64
}

/// 微单位 → 定点字符串(固定 8 位小数)。
pub fn format_money_units(units: i64) -> String {
    let sign = if units < 0 { "-" } else { "" };
    let abs = units.unsigned_abs();
    format!(
        "{}{}.{:08}",
        sign,
        abs / MONEY_SCALE as u64,
        abs % MONEY_SCALE as u64
    )
}

/// f64 金额 → 定点字符串(先量化到 1e-8 再格式化)。
pub fn format_money(value: f64) -> String {
    format_money_units(money_to_units(value))
}

/// 解析定点金额字符串为微单位。要求:纯十进制,至多 8 位小数,
/// 不允许指数/空白/千分位。兼容 JSON number 输入(经 `as_str` 之外的调用方
/// 先用 `money_units_from_json` 归一)。
pub fn parse_money(value: &str) -> Result<i64, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("amount must not be empty".to_string());
    }
    let (negative, rest) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value),
    };
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(format!("invalid fixed-point amount: {value:?}"));
    }
    let mut parts = rest.split('.');
    let int_part = parts.next().unwrap_or("");
    let frac_part = parts.next().unwrap_or("");
    if parts.next().is_some() || int_part.is_empty() && frac_part.is_empty() {
        return Err(format!("invalid fixed-point amount: {value:?}"));
    }
    if frac_part.len() > MONEY_DECIMALS as usize {
        return Err(format!(
            "amount {value:?} exceeds {MONEY_DECIMALS}-decimal precision"
        ));
    }
    let int_units: i128 = int_part
        .parse::<i128>()
        .map_err(|_| format!("amount out of range: {value:?}"))?;
    let frac_units: i128 = if frac_part.is_empty() {
        0
    } else {
        let padded = format!("{:0<width$}", frac_part, width = MONEY_DECIMALS as usize);
        padded
            .parse::<i128>()
            .map_err(|_| format!("amount out of range: {value:?}"))?
    };
    let units = int_units
        .checked_mul(MONEY_SCALE as i128)
        .and_then(|v| v.checked_add(frac_units))
        .ok_or_else(|| format!("amount out of range: {value:?}"))?;
    let units: i64 = units
        .try_into()
        .map_err(|_| format!("amount out of range: {value:?}"))?;
    Ok(if negative { -units } else { units })
}

/// 从 JSON 值解析金额:接受定点字符串或 JSON number(数字最多 8 位小数,
/// 超精度即拒绝,避免静默截断)。
pub fn money_units_from_json(value: &serde_json::Value) -> Result<i64, String> {
    match value {
        serde_json::Value::String(text) => parse_money(text),
        serde_json::Value::Number(number) => {
            let text = number.to_string();
            if text.contains('e') || text.contains('E') {
                return Err("amount must be a plain decimal, not scientific notation".to_string());
            }
            parse_money(&text)
        }
        _ => Err("amount must be a decimal string or number".to_string()),
    }
}

/// 从 JSON 值读取金额(定点字符串或 number)→ f64;无法解析返回 None。
pub fn json_money_f64(value: Option<&serde_json::Value>) -> Option<f64> {
    value.and_then(|value| {
        value
            .as_str()
            .and_then(|text| parse_money(text).ok())
            .map(units_to_money)
            .or_else(|| value.as_f64())
    })
}

/// serde 反序列化:金额入参接受定点字符串或 JSON number(≤8 位小数),输出 f64。
pub fn deserialize_money<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    money_units_from_json(&value)
        .map(units_to_money)
        .map_err(serde::de::Error::custom)
}

/// `deserialize_money` 的可选版本。
pub fn deserialize_optional_money<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    value
        .map(|value| {
            money_units_from_json(&value)
                .map(units_to_money)
                .map_err(serde::de::Error::custom)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_fixed_eight_decimals() {
        assert_eq!(format_money(12.345678), "12.34567800");
        assert_eq!(format_money(0.0), "0.00000000");
        assert_eq!(format_money(-1.5), "-1.50000000");
    }

    #[test]
    fn avoids_float_addition_error() {
        // 0.1 + 0.2 = 0.30000000000000004 in f64; quantizing to 1e-8 keeps it exact.
        let sum = format_money(0.1 + 0.2);
        assert_eq!(sum, "0.30000000");
        let diff = money_to_units(0.3) - money_to_units(0.1) - money_to_units(0.2);
        assert_eq!(diff, 0);
    }

    #[test]
    fn eight_decimal_round_trip() {
        for text in [
            "0.00000001",
            "1.23456789",
            "99999999.99999999",
            "92233720368.54775807", // i64 微单位上限
        ] {
            let units = parse_money(text).unwrap();
            assert_eq!(format_money_units(units), text);
        }
    }

    #[test]
    fn handles_huge_amounts() {
        let units = parse_money("90000000000.00000001").unwrap();
        assert_eq!(format_money_units(units), "90000000000.00000001");
        assert!(parse_money("92233720369.00000000").is_err()); // 超出 i64 微单位上限
    }

    #[test]
    fn parses_negative_amounts() {
        assert_eq!(parse_money("-2.5").unwrap(), -250_000_000);
        assert_eq!(parse_money("-0.00000001").unwrap(), -1);
    }

    #[test]
    fn rejects_invalid_inputs() {
        assert!(parse_money("").is_err());
        assert!(parse_money("abc").is_err());
        assert!(parse_money("1.2.3").is_err());
        assert!(parse_money("1.234567891").is_err()); // 9 decimals
        assert!(parse_money("1e3").is_err());
        assert!(parse_money(" 1").is_ok()); // leading/trailing whitespace tolerated
        assert!(parse_money("-").is_err());
        assert!(parse_money(".").is_err());
    }

    #[test]
    fn json_number_and_string_agree() {
        assert_eq!(
            money_units_from_json(&serde_json::json!("3.25")).unwrap(),
            money_units_from_json(&serde_json::json!(3.25)).unwrap()
        );
        assert!(money_units_from_json(&serde_json::json!(true)).is_err());
        assert!(money_units_from_json(&serde_json::json!({})).is_err());
    }

    #[test]
    fn deserializer_accepts_string_and_number() {
        #[derive(serde::Deserialize)]
        struct Payload {
            #[serde(deserialize_with = "super::deserialize_money")]
            amount: f64,
            #[serde(default, deserialize_with = "super::deserialize_optional_money")]
            fee: Option<f64>,
        }
        let from_string: Payload = serde_json::from_str(r#"{"amount":"12.34567890"}"#).unwrap();
        assert_eq!(
            crate::money_fixed::format_money(from_string.amount),
            "12.34567890"
        );
        let from_number: Payload = serde_json::from_str(r#"{"amount":0.3}"#).unwrap();
        assert_eq!(
            crate::money_fixed::format_money(from_number.amount),
            "0.30000000"
        );
        assert!(serde_json::from_str::<Payload>(r#"{"amount":"0.000000001"}"#).is_err());
        let with_fee: Payload = serde_json::from_str(r#"{"amount":1,"fee":"0.5"}"#).unwrap();
        assert_eq!(with_fee.fee, Some(0.5));
    }

    #[test]
    fn units_to_money_and_back() {
        let units = money_to_units(units_to_money(123_456_789));
        assert_eq!(units, 123_456_789);
    }
}
