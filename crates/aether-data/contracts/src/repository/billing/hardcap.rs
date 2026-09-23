//! 营收保护硬顶的产品化默认配置:并发硬顶默认 unlimited,预冻结硬顶默认关闭。
//!
//! 解析来源分层:key 显式配置 > 用户组配置 > 环境默认。全部缺省时保持既有
//! 行为(`concurrent_limit = None` 不限制,预冻结硬顶不启用)。
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const USAGE_HARDCAP_DEFAULT_API_KEY_CONCURRENT_LIMIT_ENV: &str =
    "AETHER_USAGE_HARDCAP_DEFAULT_API_KEY_CONCURRENT_LIMIT";
pub const USAGE_HARDCAP_PRE_FREEZE_ENABLED_ENV: &str = "AETHER_USAGE_HARDCAP_PRE_FREEZE_ENABLED";
/// `feature_settings` 里承载单 key 硬顶覆盖的字段名。
pub const API_KEY_HARDCAP_FEATURE_SETTINGS_FIELD: &str = "hardcap";
/// 单 key 覆盖里预冻结硬顶开关的字段名。
pub const API_KEY_PRE_FREEZE_HARDCAP_ENABLED_FIELD: &str = "pre_freeze_hardcap_enabled";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageHardcapValidationError {
    #[error("default_api_key_concurrent_limit must be a positive integer")]
    InvalidDefaultConcurrentLimit,
    #[error("pre_freeze_hardcap_enabled must be a boolean")]
    InvalidPreFreezeHardcapFlag,
    #[error("{field} must be an object")]
    InvalidOverridesShape { field: String },
    #[error("{field} contains unsupported keys: {keys}")]
    UnsupportedOverrideKeys { field: String, keys: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageHardcapDefaults {
    /// 新建 API key 未显式指定并发硬顶时套用的默认值;`None` 表示不限制。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_api_key_concurrent_limit: Option<i32>,
    /// 预冻结硬顶总开关,默认关闭;开启后冻结金额超过可用余额的请求在准入时拒绝。
    #[serde(default)]
    pub pre_freeze_hardcap_enabled: bool,
}

impl UsageHardcapDefaults {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn validate(&self) -> Result<(), UsageHardcapValidationError> {
        if self
            .default_api_key_concurrent_limit
            .is_some_and(|limit| limit <= 0)
        {
            return Err(UsageHardcapValidationError::InvalidDefaultConcurrentLimit);
        }
        Ok(())
    }

    /// 从环境变量读取默认值;变量缺失时按"未配置"处理(保持关闭/不限制),
    /// 变量存在但非法时返回错误,避免错误配置被静默吞掉。
    pub fn from_env() -> Result<Self, UsageHardcapValidationError> {
        Self::from_env_values(
            std::env::var(USAGE_HARDCAP_DEFAULT_API_KEY_CONCURRENT_LIMIT_ENV).ok(),
            std::env::var(USAGE_HARDCAP_PRE_FREEZE_ENABLED_ENV).ok(),
        )
    }

    pub fn from_env_values(
        default_api_key_concurrent_limit: Option<String>,
        pre_freeze_hardcap_enabled: Option<String>,
    ) -> Result<Self, UsageHardcapValidationError> {
        let default_api_key_concurrent_limit = default_api_key_concurrent_limit
            .map(|value| {
                value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| UsageHardcapValidationError::InvalidDefaultConcurrentLimit)
            })
            .transpose()?;
        let pre_freeze_hardcap_enabled = pre_freeze_hardcap_enabled
            .map(|value| parse_bool_flag(&value))
            .transpose()?;
        let defaults = Self {
            default_api_key_concurrent_limit,
            pre_freeze_hardcap_enabled: pre_freeze_hardcap_enabled.unwrap_or(false),
        };
        defaults.validate()?;
        Ok(defaults)
    }
}

fn parse_bool_flag(value: &str) -> Result<bool, UsageHardcapValidationError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(UsageHardcapValidationError::InvalidPreFreezeHardcapFlag),
    }
}

/// 解析单个 API key 存在 `feature_settings` 里的硬顶覆盖;字段缺失表示未覆盖。
pub fn parse_api_key_hardcap_overrides(
    feature_settings: Option<&Value>,
) -> Result<Option<bool>, UsageHardcapValidationError> {
    let Some(settings) = feature_settings else {
        return Ok(None);
    };
    let Some(overrides) = settings.get(API_KEY_HARDCAP_FEATURE_SETTINGS_FIELD) else {
        return Ok(None);
    };
    let Some(object) = overrides.as_object() else {
        return Err(UsageHardcapValidationError::InvalidOverridesShape {
            field: API_KEY_HARDCAP_FEATURE_SETTINGS_FIELD.to_string(),
        });
    };
    let unsupported: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|key| *key != API_KEY_PRE_FREEZE_HARDCAP_ENABLED_FIELD)
        .collect();
    if !unsupported.is_empty() {
        return Err(UsageHardcapValidationError::UnsupportedOverrideKeys {
            field: API_KEY_HARDCAP_FEATURE_SETTINGS_FIELD.to_string(),
            keys: unsupported.join(", "),
        });
    }
    match object.get(API_KEY_PRE_FREEZE_HARDCAP_ENABLED_FIELD) {
        None => Ok(None),
        Some(Value::Bool(enabled)) => Ok(Some(*enabled)),
        Some(_) => Err(UsageHardcapValidationError::InvalidPreFreezeHardcapFlag),
    }
}

/// key 显式值优先;未显式指定时套用部署默认值,默认 `None` = 不限制。
pub fn resolve_api_key_concurrent_limit(
    explicit: Option<i32>,
    defaults: &UsageHardcapDefaults,
) -> Option<i32> {
    explicit.or(defaults.default_api_key_concurrent_limit)
}

/// 预冻结硬顶生效判定:key 覆盖 > 用户组配置 > 部署默认(默认关闭)。
pub fn resolve_pre_freeze_hardcap_enabled(
    defaults: &UsageHardcapDefaults,
    key_override: Option<bool>,
    group_enabled: Option<bool>,
) -> bool {
    key_override
        .or(group_enabled)
        .unwrap_or(defaults.pre_freeze_hardcap_enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_keep_existing_behavior_when_unset() {
        let defaults = UsageHardcapDefaults::from_env_values(None, None).unwrap();
        assert_eq!(defaults.default_api_key_concurrent_limit, None);
        assert!(!defaults.pre_freeze_hardcap_enabled);
        assert_eq!(resolve_api_key_concurrent_limit(None, &defaults), None);
        assert!(!resolve_pre_freeze_hardcap_enabled(&defaults, None, None));
    }

    #[test]
    fn parses_env_values_and_rejects_invalid_ones() {
        let defaults =
            UsageHardcapDefaults::from_env_values(Some("8".into()), Some("on".into())).unwrap();
        assert_eq!(defaults.default_api_key_concurrent_limit, Some(8));
        assert!(defaults.pre_freeze_hardcap_enabled);

        for value in ["0".to_string(), "-3".to_string(), "abc".to_string()] {
            assert!(matches!(
                UsageHardcapDefaults::from_env_values(Some(value), None),
                Err(UsageHardcapValidationError::InvalidDefaultConcurrentLimit)
            ));
        }
        assert!(matches!(
            UsageHardcapDefaults::from_env_values(None, Some("maybe".into())),
            Err(UsageHardcapValidationError::InvalidPreFreezeHardcapFlag)
        ));
        let zero = UsageHardcapDefaults {
            default_api_key_concurrent_limit: Some(0),
            ..UsageHardcapDefaults::disabled()
        };
        assert!(matches!(
            zero.validate(),
            Err(UsageHardcapValidationError::InvalidDefaultConcurrentLimit)
        ));
    }

    #[test]
    fn concurrent_limit_resolution_prefers_explicit_value() {
        let defaults = UsageHardcapDefaults {
            default_api_key_concurrent_limit: Some(4),
            ..UsageHardcapDefaults::disabled()
        };
        assert_eq!(
            resolve_api_key_concurrent_limit(Some(12), &defaults),
            Some(12)
        );
        assert_eq!(resolve_api_key_concurrent_limit(None, &defaults), Some(4));
    }

    #[test]
    fn pre_freeze_resolution_prefers_key_then_group_then_default() {
        let defaults = UsageHardcapDefaults {
            pre_freeze_hardcap_enabled: true,
            ..UsageHardcapDefaults::disabled()
        };
        // key 覆盖优先于用户组与默认,可显式关闭。
        assert!(!resolve_pre_freeze_hardcap_enabled(
            &defaults,
            Some(false),
            Some(true)
        ));
        assert!(resolve_pre_freeze_hardcap_enabled(
            &UsageHardcapDefaults::disabled(),
            None,
            Some(true)
        ));
        assert!(resolve_pre_freeze_hardcap_enabled(&defaults, None, None));
        assert!(!resolve_pre_freeze_hardcap_enabled(
            &UsageHardcapDefaults::disabled(),
            None,
            None
        ));
    }

    #[test]
    fn parses_key_overrides_from_feature_settings() {
        assert_eq!(parse_api_key_hardcap_overrides(None).unwrap(), None);
        assert_eq!(
            parse_api_key_hardcap_overrides(Some(&json!({"theme": "dark"}))).unwrap(),
            None
        );
        let enabled = json!({"hardcap": {"pre_freeze_hardcap_enabled": true}});
        assert_eq!(
            parse_api_key_hardcap_overrides(Some(&enabled)).unwrap(),
            Some(true)
        );

        let wrong_shape = json!({"hardcap": "enabled"});
        assert!(matches!(
            parse_api_key_hardcap_overrides(Some(&wrong_shape)),
            Err(UsageHardcapValidationError::InvalidOverridesShape { .. })
        ));
        let unknown_key = json!({"hardcap": {"other": true}});
        assert!(matches!(
            parse_api_key_hardcap_overrides(Some(&unknown_key)),
            Err(UsageHardcapValidationError::UnsupportedOverrideKeys { .. })
        ));
        let wrong_type = json!({"hardcap": {"pre_freeze_hardcap_enabled": "yes"}});
        assert!(matches!(
            parse_api_key_hardcap_overrides(Some(&wrong_type)),
            Err(UsageHardcapValidationError::InvalidPreFreezeHardcapFlag)
        ));
    }
}
