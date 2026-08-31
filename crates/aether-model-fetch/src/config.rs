const MODEL_FETCH_INTERVAL_MINUTES_DEFAULT: u64 = 1440;
const MODEL_FETCH_INTERVAL_MINUTES_MIN: u64 = 60;
const MODEL_FETCH_INTERVAL_MINUTES_MAX: u64 = 10080;
const MODEL_FETCH_STARTUP_DELAY_SECONDS_DEFAULT: u64 = 10;
const MODEL_FETCH_REMOVAL_GRACE_COUNT_DEFAULT: u64 = 2;
const MODEL_FETCH_REMOVAL_GRACE_COUNT_MIN: u64 = 1;
const MODEL_FETCH_REMOVAL_GRACE_COUNT_MAX: u64 = 30;

pub fn model_fetch_interval_minutes() -> u64 {
    std::env::var("MODEL_FETCH_INTERVAL_MINUTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| {
            value.clamp(
                MODEL_FETCH_INTERVAL_MINUTES_MIN,
                MODEL_FETCH_INTERVAL_MINUTES_MAX,
            )
        })
        .unwrap_or(MODEL_FETCH_INTERVAL_MINUTES_DEFAULT)
}

pub fn model_fetch_startup_enabled() -> bool {
    std::env::var("MODEL_FETCH_STARTUP_ENABLED")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(true)
}

pub fn model_fetch_startup_delay_seconds() -> u64 {
    std::env::var("MODEL_FETCH_STARTUP_DELAY_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(MODEL_FETCH_STARTUP_DELAY_SECONDS_DEFAULT)
}

/// Number of consecutive complete model-fetch snapshots in which a previously
/// allowed model may be missing before it is removed from the key whitelist.
pub fn model_fetch_removal_grace_count() -> u64 {
    std::env::var("MODEL_FETCH_REMOVAL_GRACE_COUNT")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| {
            value.clamp(
                MODEL_FETCH_REMOVAL_GRACE_COUNT_MIN,
                MODEL_FETCH_REMOVAL_GRACE_COUNT_MAX,
            )
        })
        .unwrap_or(MODEL_FETCH_REMOVAL_GRACE_COUNT_DEFAULT)
}

#[cfg(test)]
mod tests {
    use super::{
        model_fetch_interval_minutes, model_fetch_removal_grace_count,
        model_fetch_startup_delay_seconds, model_fetch_startup_enabled,
    };

    struct TestEnvVarGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl Drop for TestEnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.as_deref() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn set_test_env_var(key: &'static str, value: &str) -> TestEnvVarGuard {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        TestEnvVarGuard { key, previous }
    }

    #[test]
    fn interval_minutes_clamps_to_supported_bounds() {
        let _interval = set_test_env_var("MODEL_FETCH_INTERVAL_MINUTES", "5");
        assert_eq!(model_fetch_interval_minutes(), 60);

        let _interval = set_test_env_var("MODEL_FETCH_INTERVAL_MINUTES", "20000");
        assert_eq!(model_fetch_interval_minutes(), 10080);
    }

    #[test]
    fn startup_flags_read_from_environment() {
        let _enabled = set_test_env_var("MODEL_FETCH_STARTUP_ENABLED", "false");
        let _delay = set_test_env_var("MODEL_FETCH_STARTUP_DELAY_SECONDS", "3");
        assert!(!model_fetch_startup_enabled());
        assert_eq!(model_fetch_startup_delay_seconds(), 3);
    }

    #[test]
    fn removal_grace_count_defaults_and_clamps() {
        std::env::remove_var("MODEL_FETCH_REMOVAL_GRACE_COUNT");
        assert_eq!(model_fetch_removal_grace_count(), 2);

        let _grace = set_test_env_var("MODEL_FETCH_REMOVAL_GRACE_COUNT", "0");
        assert_eq!(model_fetch_removal_grace_count(), 1);

        let _grace = set_test_env_var("MODEL_FETCH_REMOVAL_GRACE_COUNT", "5");
        assert_eq!(model_fetch_removal_grace_count(), 5);

        let _grace = set_test_env_var("MODEL_FETCH_REMOVAL_GRACE_COUNT", "99");
        assert_eq!(model_fetch_removal_grace_count(), 30);
    }
}
