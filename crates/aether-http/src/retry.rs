use std::time::Duration;

use rand::Rng as _;

use crate::HttpRetryConfig;

/// Full jitter is capped so a clamped `base + jitter` never exceeds `max_delay_ms`.
const MAX_JITTER_MS: u64 = 100;

pub fn jittered_delay_for_retry(config: HttpRetryConfig, retry_index: u32) -> Duration {
    let base = config.delay_for_retry(retry_index);
    if base.is_zero() {
        return base;
    }
    let max_delay = Duration::from_millis(config.normalized().max_delay_ms);
    let jitter_cap = MAX_JITTER_MS.min(max_delay.saturating_sub(base).as_millis() as u64);
    if jitter_cap == 0 {
        return base;
    }
    let jitter_ms = rand::thread_rng().gen_range(0..=jitter_cap);
    std::cmp::min(base + Duration::from_millis(jitter_ms), max_delay)
}

#[cfg(test)]
mod tests {
    use super::jittered_delay_for_retry;
    use crate::HttpRetryConfig;

    #[test]
    fn jittered_delay_is_at_least_base_delay() {
        let config = HttpRetryConfig {
            max_attempts: 3,
            base_delay_ms: 200,
            max_delay_ms: 400,
        };

        assert!(jittered_delay_for_retry(config, 0) >= std::time::Duration::from_millis(200));
    }

    #[test]
    fn jittered_delay_never_exceeds_max_delay() {
        let config = HttpRetryConfig {
            max_attempts: 8,
            base_delay_ms: 200,
            max_delay_ms: 250,
        };
        let base = config.delay_for_retry(7);
        assert_eq!(base, std::time::Duration::from_millis(250));
        for _ in 0..64 {
            assert!(
                jittered_delay_for_retry(config, 7) <= std::time::Duration::from_millis(250),
                "base + jitter must be clamped to max_delay_ms"
            );
        }
    }

    #[test]
    fn jittered_delay_stays_within_jitter_window() {
        let config = HttpRetryConfig {
            max_attempts: 3,
            base_delay_ms: 100,
            max_delay_ms: 10_000,
        };
        for retry_index in 0..3 {
            let base = config.delay_for_retry(retry_index);
            for _ in 0..32 {
                let delay = jittered_delay_for_retry(config, retry_index);
                assert!(delay >= base);
                assert!(delay <= base + std::time::Duration::from_millis(100));
            }
        }
    }
}
