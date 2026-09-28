use std::collections::BTreeMap;

use aether_contracts::billing_multiplier::{
    BillingMultiplier, BILLING_MULTIPLIER_CLAMP_MAX_UNITS, BILLING_MULTIPLIER_CLAMP_MIN_UNITS,
};

pub(crate) const USER_GROUP_BILLING_MULTIPLIERS_CONFIG_KEY: &str =
    "user_group_billing_multipliers";
pub(crate) const BILLING_MULTIPLIER_CLAMP_MIN_CONFIG_KEY: &str = "billing_multiplier_clamp_min";
pub(crate) const BILLING_MULTIPLIER_CLAMP_MAX_CONFIG_KEY: &str = "billing_multiplier_clamp_max";

pub(crate) const DEFAULT_BILLING_MULTIPLIER_CLAMP_MIN: f64 = 0.01;
pub(crate) const DEFAULT_BILLING_MULTIPLIER_CLAMP_MAX: f64 = 100.0;

/// Parses the stored system-config JSON object into per-group multipliers.
/// Entries that do not decode as a legal multiplier are ignored so one bad
/// row can never break resolution for other groups.
pub(crate) fn parse_group_billing_multipliers(
    value: Option<&serde_json::Value>,
) -> BTreeMap<String, BillingMultiplier> {
    let mut map = BTreeMap::new();
    let Some(entries) = value.and_then(serde_json::Value::as_object) else {
        return map;
    };
    for (group_id, raw) in entries {
        if let Some(multiplier) = raw
            .as_f64()
            .and_then(|number| BillingMultiplier::from_f64_rounded(number).ok())
        {
            map.insert(group_id.clone(), multiplier);
        }
    }
    map
}

/// Single-layer overlay: a user in several groups inherits the multiplier of
/// the highest-priority group that has one configured; ties fall to the
/// name-ascending order used by `effective_user_groups_for_user`.
pub(crate) fn resolve_group_billing_multiplier(
    groups: &[aether_data::repository::users::StoredUserGroup],
    configured: &BTreeMap<String, BillingMultiplier>,
) -> Option<BillingMultiplier> {
    let mut best: Option<(&aether_data::repository::users::StoredUserGroup, BillingMultiplier)> =
        None;
    for group in groups {
        let Some(multiplier) = configured.get(&group.id) else {
            continue;
        };
        match best {
            Some((best_group, _)) if best_group.priority > group.priority => {}
            _ => best = Some((group, *multiplier)),
        }
    }
    best.map(|(_, multiplier)| multiplier)
}

/// Resolves the risk-control clamp bounds from system settings. Invalid or
/// inverted bounds fall back to the defaults.
pub(crate) fn resolve_billing_multiplier_clamp(
    min_value: Option<&serde_json::Value>,
    max_value: Option<&serde_json::Value>,
) -> (BillingMultiplier, BillingMultiplier) {
    let min = min_value
        .and_then(serde_json::Value::as_f64)
        .and_then(|value| BillingMultiplier::from_f64_rounded(value).ok())
        .filter(|value| !value.is_zero())
        .unwrap_or_else(|| BillingMultiplier::from_units(BILLING_MULTIPLIER_CLAMP_MIN_UNITS).expect("default clamp min"));
    let max = max_value
        .and_then(serde_json::Value::as_f64)
        .and_then(|value| BillingMultiplier::from_f64_rounded(value).ok())
        .filter(|value| !value.is_zero())
        .unwrap_or_else(|| BillingMultiplier::from_units(BILLING_MULTIPLIER_CLAMP_MAX_UNITS).expect("default clamp max"));
    if min.units() <= max.units() {
        (min, max)
    } else {
        (
            BillingMultiplier::from_units(BILLING_MULTIPLIER_CLAMP_MIN_UNITS)
                .expect("default clamp min"),
            BillingMultiplier::from_units(BILLING_MULTIPLIER_CLAMP_MAX_UNITS)
                .expect("default clamp max"),
        )
    }
}

/// Validates an administrator-supplied multiplier and applies the clamp.
/// An explicit 0 (free) passes through untouched.
pub(crate) fn clamp_admin_billing_multiplier(
    value: f64,
    clamp: (BillingMultiplier, BillingMultiplier),
) -> Result<BillingMultiplier, String> {
    let multiplier = BillingMultiplier::from_f64_rounded(value)
        .map_err(|_| "billing_multiplier 必须是 0 到 9999.9999 之间的有限数值".to_string())?;
    Ok(multiplier.clamp(clamp.0, clamp.1))
}

pub(crate) async fn read_billing_multiplier_clamp(
    state: &crate::state::AppState,
) -> (BillingMultiplier, BillingMultiplier) {
    let min = state
        .read_system_config_json_value(BILLING_MULTIPLIER_CLAMP_MIN_CONFIG_KEY)
        .await
        .ok()
        .flatten();
    let max = state
        .read_system_config_json_value(BILLING_MULTIPLIER_CLAMP_MAX_CONFIG_KEY)
        .await
        .ok()
        .flatten();
    resolve_billing_multiplier_clamp(min.as_ref(), max.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn group(id: &str, priority: i32) -> aether_data::repository::users::StoredUserGroup {
        aether_data::repository::users::StoredUserGroup {
            id: id.to_string(),
            name: id.to_string(),
            normalized_name: id.to_string(),
            description: None,
            priority,
            allowed_providers: None,
            allowed_providers_mode: "inherit".to_string(),
            allowed_api_formats: None,
            allowed_api_formats_mode: "inherit".to_string(),
            allowed_models: None,
            allowed_models_mode: "inherit".to_string(),
            rate_limit: None,
            rate_limit_mode: "inherit".to_string(),
            daily_usage_limit_usd: None,
            daily_usage_limit_mode: "inherit".to_string(),
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn highest_priority_group_wins_and_missing_entries_are_skipped() {
        let groups = vec![group("g-low", 1), group("g-high", 9), group("g-none", 99)];
        let configured = parse_group_billing_multipliers(Some(&json!({
            "g-low": 1.5,
            "g-high": 2.0,
            "g-broken": "nope"
        })));
        assert_eq!(
            resolve_group_billing_multiplier(&groups, &configured),
            Some(BillingMultiplier::from_f64_rounded(2.0).expect("value"))
        );
        let no_match = vec![group("g-none", 99)];
        assert_eq!(resolve_group_billing_multiplier(&no_match, &configured), None);
    }

    #[test]
    fn clamp_uses_settings_and_keeps_free_zero() {
        let clamp = resolve_billing_multiplier_clamp(Some(&json!(0.5)), Some(&json!(10.0)));
        assert_eq!(
            clamp_admin_billing_multiplier(0.1, clamp).expect("clamped"),
            BillingMultiplier::from_f64_rounded(0.5).expect("value")
        );
        assert_eq!(
            clamp_admin_billing_multiplier(50.0, clamp).expect("clamped"),
            BillingMultiplier::from_f64_rounded(10.0).expect("value")
        );
        assert!(clamp_admin_billing_multiplier(0.0, clamp)
            .expect("free passes")
            .is_zero());
        let fallback = resolve_billing_multiplier_clamp(Some(&json!(50.0)), Some(&json!(10.0)));
        assert_eq!(fallback.0.units(), BILLING_MULTIPLIER_CLAMP_MIN_UNITS);
        assert_eq!(fallback.1.units(), BILLING_MULTIPLIER_CLAMP_MAX_UNITS);
    }
}
