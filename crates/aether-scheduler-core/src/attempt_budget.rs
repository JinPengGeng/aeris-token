use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub const DEFAULT_MAX_ATTEMPTS: usize = 32;
pub const DEFAULT_MAX_CREDENTIAL_ATTEMPTS: usize = 16;
pub const DEFAULT_MAX_PROVIDER_SWITCHES: usize = 16;
pub const DEFAULT_REQUEST_DEADLINE: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptBudgetError {
    AttemptsExhausted,
    CredentialAttemptsExhausted,
    ProviderSwitchesExhausted,
    DeadlineExceeded,
}

impl AttemptBudgetError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AttemptsExhausted => "attempts_exhausted",
            Self::CredentialAttemptsExhausted => "credential_attempts_exhausted",
            Self::ProviderSwitchesExhausted => "provider_switches_exhausted",
            Self::DeadlineExceeded => "deadline_exceeded",
        }
    }
}

#[derive(Debug)]
pub struct AttemptBudget {
    started_at: Instant,
    deadline: Option<Instant>,
    deadline_initialized: bool,
    attempts: usize,
    credential_attempts: BTreeMap<String, usize>,
    provider_switches: usize,
    last_provider: Option<String>,
    max_attempts: usize,
    max_credential_attempts: usize,
    max_provider_switches: usize,
}

impl AttemptBudget {
    pub fn new(now: Instant) -> Self {
        Self {
            started_at: now,
            deadline: None,
            deadline_initialized: false,
            attempts: 0,
            credential_attempts: BTreeMap::new(),
            provider_switches: 0,
            last_provider: None,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            max_credential_attempts: DEFAULT_MAX_CREDENTIAL_ATTEMPTS,
            max_provider_switches: DEFAULT_MAX_PROVIDER_SWITCHES,
        }
    }

    pub fn with_limits(
        mut self,
        max_attempts: usize,
        max_credential_attempts: usize,
        max_provider_switches: usize,
    ) -> Self {
        self.max_attempts = max_attempts.max(1);
        self.max_credential_attempts = max_credential_attempts.max(1);
        self.max_provider_switches = max_provider_switches;
        self
    }

    pub fn reserve(
        &mut self,
        provider_id: &str,
        key_id: &str,
        request_timeout_ms: Option<u64>,
        now: Instant,
    ) -> Result<(), AttemptBudgetError> {
        if !self.deadline_initialized {
            let timeout = request_timeout_ms
                .filter(|timeout_ms| *timeout_ms > 0)
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_REQUEST_DEADLINE);
            self.deadline = Some(self.started_at + timeout);
            self.deadline_initialized = true;
        }
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            return Err(AttemptBudgetError::DeadlineExceeded);
        }
        if self.attempts >= self.max_attempts {
            return Err(AttemptBudgetError::AttemptsExhausted);
        }
        let credential_attempts = self.credential_attempts.get(key_id).copied().unwrap_or(0);
        if credential_attempts >= self.max_credential_attempts {
            return Err(AttemptBudgetError::CredentialAttemptsExhausted);
        }
        if let Some(previous) = self.last_provider.as_ref() {
            if previous != provider_id && self.provider_switches >= self.max_provider_switches {
                return Err(AttemptBudgetError::ProviderSwitchesExhausted);
            }
        }
        if self
            .last_provider
            .as_deref()
            .is_some_and(|provider| provider != provider_id)
        {
            self.provider_switches += 1;
        }
        self.attempts += 1;
        self.credential_attempts
            .insert(key_id.to_string(), credential_attempts + 1);
        self.last_provider = Some(provider_id.to_string());
        Ok(())
    }

    pub fn remaining(&self, now: Instant) -> Result<Option<Duration>, AttemptBudgetError> {
        let Some(deadline) = self.deadline else {
            return Ok(None);
        };
        deadline
            .checked_duration_since(now)
            .filter(|remaining| !remaining.is_zero())
            .map(Some)
            .ok_or(AttemptBudgetError::DeadlineExceeded)
    }

    pub const fn attempts(&self) -> usize {
        self.attempts
    }
    pub fn credential_attempts(&self, key_id: &str) -> usize {
        self.credential_attempts.get(key_id).copied().unwrap_or(0)
    }
    pub const fn provider_switches(&self) -> usize {
        self.provider_switches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_budget_counts_switches_and_deadline() {
        let now = Instant::now();
        let mut budget = AttemptBudget::new(now).with_limits(2, 1, 1);
        assert!(budget.reserve("p", "k", Some(1_000), now).is_ok());
        assert!(budget.reserve("p2", "k2", None, now).is_ok());
        assert_eq!(budget.attempts(), 2);
        assert_eq!(budget.credential_attempts("k"), 1);
        assert_eq!(budget.credential_attempts("k2"), 1);
        assert_eq!(budget.provider_switches(), 1);
        assert_eq!(
            budget.reserve("p3", "k3", None, now),
            Err(AttemptBudgetError::AttemptsExhausted)
        );

        let mut budget = AttemptBudget::new(now);
        assert_eq!(
            budget.reserve("p", "k", Some(1), now + Duration::from_millis(2)),
            Err(AttemptBudgetError::DeadlineExceeded)
        );
    }

    #[test]
    fn credential_attempt_limit_is_per_key_and_counts_same_key_retries() {
        let now = Instant::now();
        let mut credential_budget = AttemptBudget::new(now).with_limits(10, 2, 10);
        credential_budget.reserve("p", "k1", None, now).unwrap();
        credential_budget.reserve("p", "k1", None, now).unwrap();
        assert_eq!(
            credential_budget.reserve("p", "k1", None, now),
            Err(AttemptBudgetError::CredentialAttemptsExhausted)
        );
        credential_budget.reserve("p", "k2", None, now).unwrap();
        credential_budget.reserve("p", "k2", None, now).unwrap();
        assert_eq!(credential_budget.credential_attempts("k1"), 2);
        assert_eq!(credential_budget.credential_attempts("k2"), 2);

        let mut provider_budget = AttemptBudget::new(now).with_limits(10, 10, 0);
        provider_budget.reserve("p1", "k", None, now).unwrap();
        assert_eq!(
            provider_budget.reserve("p2", "k", None, now),
            Err(AttemptBudgetError::ProviderSwitchesExhausted)
        );
    }

    #[test]
    fn zero_timeout_uses_the_compatibility_default_instead_of_expiring_immediately() {
        let now = Instant::now();
        let mut budget = AttemptBudget::new(now);
        budget.reserve("p", "k", Some(0), now).unwrap();
        assert_eq!(budget.remaining(now), Ok(Some(DEFAULT_REQUEST_DEADLINE)));
    }
}
