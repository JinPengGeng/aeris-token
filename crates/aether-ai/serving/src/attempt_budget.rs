use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptBudgetLimits {
    max_total_dispatches: u64,
    max_credential_entries: u64,
    max_provider_switches: u64,
    dispatch_deadline: Instant,
}

impl AttemptBudgetLimits {
    pub fn new(
        max_total_dispatches: u64,
        max_credential_entries: u64,
        max_provider_switches: u64,
        dispatch_deadline: Instant,
    ) -> Self {
        Self {
            max_total_dispatches,
            max_credential_entries,
            max_provider_switches,
            dispatch_deadline,
        }
    }

    pub fn max_total_dispatches(self) -> u64 {
        self.max_total_dispatches
    }

    pub fn max_credential_entries(self) -> u64 {
        self.max_credential_entries
    }

    pub fn max_provider_switches(self) -> u64 {
        self.max_provider_switches
    }

    pub fn dispatch_deadline(self) -> Instant {
        self.dispatch_deadline
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptTarget {
    provider_id: String,
    endpoint_id: String,
    key_id: String,
}

impl AttemptTarget {
    pub fn new(
        provider_id: impl Into<String>,
        endpoint_id: impl Into<String>,
        key_id: impl Into<String>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            endpoint_id: endpoint_id.into(),
            key_id: key_id.into(),
        }
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub fn endpoint_id(&self) -> &str {
        &self.endpoint_id
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    fn same_credential(&self, other: &Self) -> bool {
        self.provider_id == other.provider_id && self.key_id == other.key_id
    }

    fn same_endpoint(&self, other: &Self) -> bool {
        self.provider_id == other.provider_id && self.endpoint_id == other.endpoint_id
    }
}

impl From<&aether_contracts::ExecutionPlan> for AttemptTarget {
    fn from(plan: &aether_contracts::ExecutionPlan) -> Self {
        Self::new(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptRetryIntent {
    Initial,
    Candidate,
    Credential,
    Endpoint,
    Provider,
    SameTargetReplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptBudgetDimension {
    TotalDispatches,
    CredentialEntries,
    ProviderSwitches,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptBudgetExhausted {
    dimension: AttemptBudgetDimension,
    used: u64,
    limit: u64,
}

impl AttemptBudgetExhausted {
    fn new(dimension: AttemptBudgetDimension, used: u64, limit: u64) -> Self {
        Self {
            dimension,
            used,
            limit,
        }
    }

    pub fn dimension(self) -> AttemptBudgetDimension {
        self.dimension
    }

    pub fn used(self) -> u64 {
        self.used
    }

    pub fn limit(self) -> u64 {
        self.limit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptTransitionViolation {
    InitialRequired,
    AlreadyStarted,
    CredentialUnchanged,
    EndpointUnchanged,
    ProviderUnchanged,
    SameTargetRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptBudgetError {
    BudgetExhausted(AttemptBudgetExhausted),
    DeadlineExceeded {
        deadline: Instant,
        observed_at: Instant,
    },
    InvalidTransition {
        intent: AttemptRetryIntent,
        violation: AttemptTransitionViolation,
    },
    StateUnavailable,
}

impl fmt::Display for AttemptBudgetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BudgetExhausted(exhausted) => write!(
                formatter,
                "attempt budget {:?} exhausted at {} of {}",
                exhausted.dimension, exhausted.used, exhausted.limit
            ),
            Self::DeadlineExceeded {
                deadline,
                observed_at,
            } => write!(
                formatter,
                "attempt dispatch deadline {deadline:?} exceeded at {observed_at:?}"
            ),
            Self::InvalidTransition { intent, violation } => write!(
                formatter,
                "attempt retry intent {intent:?} rejected: {violation:?}"
            ),
            Self::StateUnavailable => formatter.write_str("attempt budget state unavailable"),
        }
    }
}

impl Error for AttemptBudgetError {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttemptBudgetUsage {
    total_dispatches: u64,
    credential_entries: u64,
    provider_switches: u64,
}

impl AttemptBudgetUsage {
    pub fn total_dispatches(self) -> u64 {
        self.total_dispatches
    }

    pub fn credential_entries(self) -> u64 {
        self.credential_entries
    }

    pub fn provider_switches(self) -> u64 {
        self.provider_switches
    }
}

#[derive(Debug)]
struct AttemptBudgetState {
    usage: AttemptBudgetUsage,
    previous_target: Option<AttemptTarget>,
}

#[derive(Debug, Clone)]
pub struct AttemptBudget {
    limits: AttemptBudgetLimits,
    state: Arc<Mutex<AttemptBudgetState>>,
}

impl AttemptBudget {
    pub fn new(limits: AttemptBudgetLimits) -> Self {
        Self {
            limits,
            state: Arc::new(Mutex::new(AttemptBudgetState {
                usage: AttemptBudgetUsage::default(),
                previous_target: None,
            })),
        }
    }

    pub fn limits(&self) -> AttemptBudgetLimits {
        self.limits
    }

    pub fn usage(&self) -> Result<AttemptBudgetUsage, AttemptBudgetError> {
        self.state
            .lock()
            .map(|state| state.usage)
            .map_err(|_| AttemptBudgetError::StateUnavailable)
    }

    pub fn ensure_dispatch_deadline(&self, now: Instant) -> Result<(), AttemptBudgetError> {
        if now >= self.limits.dispatch_deadline {
            return Err(AttemptBudgetError::DeadlineExceeded {
                deadline: self.limits.dispatch_deadline,
                observed_at: now,
            });
        }
        Ok(())
    }

    /// Atomically reserves every dimension required by one physical dispatch.
    /// A successful reservation is permanent, even when the returned permit is dropped.
    pub fn try_reserve(
        &self,
        now: Instant,
        target: &AttemptTarget,
        intent: AttemptRetryIntent,
    ) -> Result<AttemptDispatchPermit, AttemptBudgetError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AttemptBudgetError::StateUnavailable)?;
        self.ensure_dispatch_deadline(now)?;
        validate_transition(state.previous_target.as_ref(), target, intent)?;

        let credential_delta = u64::from(
            state
                .previous_target
                .as_ref()
                .is_none_or(|previous| !previous.same_credential(target)),
        );
        let provider_switch_delta = u64::from(
            state
                .previous_target
                .as_ref()
                .is_some_and(|previous| previous.provider_id != target.provider_id),
        );

        check_dimension(
            state.usage.total_dispatches,
            1,
            self.limits.max_total_dispatches,
            AttemptBudgetDimension::TotalDispatches,
        )?;
        check_dimension(
            state.usage.credential_entries,
            credential_delta,
            self.limits.max_credential_entries,
            AttemptBudgetDimension::CredentialEntries,
        )?;
        check_dimension(
            state.usage.provider_switches,
            provider_switch_delta,
            self.limits.max_provider_switches,
            AttemptBudgetDimension::ProviderSwitches,
        )?;

        state.usage.total_dispatches += 1;
        state.usage.credential_entries += credential_delta;
        state.usage.provider_switches += provider_switch_delta;
        state.previous_target = Some(target.clone());

        Ok(AttemptDispatchPermit {
            ordinal: state.usage.total_dispatches,
            target: target.clone(),
            reserved_at: now,
            usage: state.usage,
        })
    }
}

fn validate_transition(
    previous: Option<&AttemptTarget>,
    target: &AttemptTarget,
    intent: AttemptRetryIntent,
) -> Result<(), AttemptBudgetError> {
    let violation = match (previous, intent) {
        (None, AttemptRetryIntent::Initial) => None,
        (None, _) => Some(AttemptTransitionViolation::InitialRequired),
        (Some(_), AttemptRetryIntent::Initial) => Some(AttemptTransitionViolation::AlreadyStarted),
        (Some(_), AttemptRetryIntent::Candidate) => None,
        (Some(previous), AttemptRetryIntent::Credential) => previous
            .same_credential(target)
            .then_some(AttemptTransitionViolation::CredentialUnchanged),
        (Some(previous), AttemptRetryIntent::Endpoint) => previous
            .same_endpoint(target)
            .then_some(AttemptTransitionViolation::EndpointUnchanged),
        (Some(previous), AttemptRetryIntent::Provider) => (previous.provider_id
            == target.provider_id)
            .then_some(AttemptTransitionViolation::ProviderUnchanged),
        (Some(previous), AttemptRetryIntent::SameTargetReplay) => {
            (previous != target).then_some(AttemptTransitionViolation::SameTargetRequired)
        }
    };

    match violation {
        Some(violation) => Err(AttemptBudgetError::InvalidTransition { intent, violation }),
        None => Ok(()),
    }
}

fn check_dimension(
    used: u64,
    delta: u64,
    limit: u64,
    dimension: AttemptBudgetDimension,
) -> Result<(), AttemptBudgetError> {
    if delta > limit.saturating_sub(used) {
        return Err(AttemptBudgetError::BudgetExhausted(
            AttemptBudgetExhausted::new(dimension, used, limit),
        ));
    }
    Ok(())
}

#[derive(Debug)]
#[must_use = "a reserved dispatch permit must be consumed by exactly one physical dispatch"]
pub struct AttemptDispatchPermit {
    ordinal: u64,
    target: AttemptTarget,
    reserved_at: Instant,
    usage: AttemptBudgetUsage,
}

impl AttemptDispatchPermit {
    pub fn ordinal(&self) -> u64 {
        self.ordinal
    }

    pub fn target(&self) -> &AttemptTarget {
        &self.target
    }

    pub fn reserved_at(&self) -> Instant {
        self.reserved_at
    }

    pub fn usage(&self) -> AttemptBudgetUsage {
        self.usage
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn target(provider: &str, endpoint: &str, key: &str) -> AttemptTarget {
        AttemptTarget::new(provider, endpoint, key)
    }

    fn limits(now: Instant, total: u64, credentials: u64, providers: u64) -> AttemptBudgetLimits {
        AttemptBudgetLimits::new(total, credentials, providers, now + Duration::from_secs(60))
    }

    fn exhausted(error: AttemptBudgetError) -> AttemptBudgetExhausted {
        match error {
            AttemptBudgetError::BudgetExhausted(exhausted) => exhausted,
            other => panic!("expected budget exhaustion, got {other:?}"),
        }
    }

    #[test]
    fn first_dispatch_consumes_total_and_credential_but_not_provider_switch() {
        let now = Instant::now();
        let budget = AttemptBudget::new(limits(now, 3, 3, 3));
        let target = target("provider-a", "endpoint-a", "key-a");

        let permit = budget
            .try_reserve(now, &target, AttemptRetryIntent::Initial)
            .expect("first dispatch should fit");

        assert_eq!(permit.ordinal(), 1);
        assert_eq!(permit.target(), &target);
        assert_eq!(permit.usage().total_dispatches(), 1);
        assert_eq!(permit.usage().credential_entries(), 1);
        assert_eq!(permit.usage().provider_switches(), 0);
    }

    #[test]
    fn same_target_replay_only_consumes_total_dispatches() {
        let now = Instant::now();
        let budget = AttemptBudget::new(limits(now, 3, 1, 0));
        let target = target("provider-a", "endpoint-a", "key-a");
        let _permit = budget
            .try_reserve(now, &target, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");

        let replay = budget
            .try_reserve(
                now + Duration::from_millis(1),
                &target,
                AttemptRetryIntent::SameTargetReplay,
            )
            .expect("same target replay should not re-enter credential");

        assert_eq!(replay.ordinal(), 2);
        assert_eq!(replay.usage().credential_entries(), 1);
        assert_eq!(replay.usage().provider_switches(), 0);
    }

    #[test]
    fn actual_target_transitions_determine_all_dimension_deltas() {
        let now = Instant::now();
        let budget = AttemptBudget::new(limits(now, 5, 5, 5));
        let a = target("provider-a", "endpoint-a", "key-a");
        let same_credential_new_endpoint = target("provider-a", "endpoint-b", "key-a");
        let new_credential = target("provider-a", "endpoint-b", "key-b");
        let new_provider = target("provider-b", "endpoint-c", "key-c");

        let _permit = budget
            .try_reserve(now, &a, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");
        let _permit = budget
            .try_reserve(
                now,
                &same_credential_new_endpoint,
                AttemptRetryIntent::Endpoint,
            )
            .expect("endpoint transition should fit");
        let _permit = budget
            .try_reserve(now, &new_credential, AttemptRetryIntent::Credential)
            .expect("credential transition should fit");
        let permit = budget
            .try_reserve(now, &new_provider, AttemptRetryIntent::Provider)
            .expect("provider transition should fit");

        assert_eq!(permit.usage().total_dispatches(), 4);
        assert_eq!(permit.usage().credential_entries(), 3);
        assert_eq!(permit.usage().provider_switches(), 1);
    }

    #[test]
    fn returning_to_a_previous_target_consumes_transitions_again() {
        let now = Instant::now();
        let budget = AttemptBudget::new(limits(now, 3, 3, 2));
        let a = target("provider-a", "endpoint-a", "key-a");
        let b = target("provider-b", "endpoint-b", "key-b");

        let _permit = budget
            .try_reserve(now, &a, AttemptRetryIntent::Initial)
            .expect("A should fit");
        let _permit = budget
            .try_reserve(now, &b, AttemptRetryIntent::Provider)
            .expect("A to B should fit");
        let permit = budget
            .try_reserve(now, &a, AttemptRetryIntent::Provider)
            .expect("B to A should fit");

        assert_eq!(permit.usage().credential_entries(), 3);
        assert_eq!(permit.usage().provider_switches(), 2);
    }

    #[test]
    fn failed_multidimension_reservation_has_no_partial_charge() {
        let now = Instant::now();
        let budget = AttemptBudget::new(limits(now, 4, 1, 4));
        let a = target("provider-a", "endpoint-a", "key-a");
        let b = target("provider-b", "endpoint-b", "key-b");
        let _permit = budget
            .try_reserve(now, &a, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");

        let error = budget
            .try_reserve(now, &b, AttemptRetryIntent::Provider)
            .expect_err("credential limit should reject the entire transition");
        assert_eq!(
            exhausted(error),
            AttemptBudgetExhausted::new(AttemptBudgetDimension::CredentialEntries, 1, 1)
        );
        assert_eq!(
            budget.usage().expect("state should remain available"),
            AttemptBudgetUsage {
                total_dispatches: 1,
                credential_entries: 1,
                provider_switches: 0,
            }
        );

        let replay = budget
            .try_reserve(now, &a, AttemptRetryIntent::SameTargetReplay)
            .expect("failed transition must not change the previous target");
        assert_eq!(replay.ordinal(), 2);
    }

    #[test]
    fn exhaustion_dimension_order_is_deterministic_and_limits_allow_zero() {
        let now = Instant::now();
        let target = target("provider-a", "endpoint-a", "key-a");

        let zero_total = AttemptBudget::new(limits(now, 0, 0, 0));
        assert_eq!(
            exhausted(
                zero_total
                    .try_reserve(now, &target, AttemptRetryIntent::Initial)
                    .expect_err("zero total budget must reject dispatch")
            )
            .dimension(),
            AttemptBudgetDimension::TotalDispatches
        );

        let zero_credentials = AttemptBudget::new(limits(now, 1, 0, 0));
        assert_eq!(
            exhausted(
                zero_credentials
                    .try_reserve(now, &target, AttemptRetryIntent::Initial)
                    .expect_err("zero credential budget must reject dispatch")
            )
            .dimension(),
            AttemptBudgetDimension::CredentialEntries
        );
    }

    #[test]
    fn provider_limit_failure_does_not_charge_total_or_credential() {
        let now = Instant::now();
        let budget = AttemptBudget::new(limits(now, 3, 3, 0));
        let a = target("provider-a", "endpoint-a", "key-a");
        let b = target("provider-b", "endpoint-b", "key-b");
        let _permit = budget
            .try_reserve(now, &a, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");

        let error = budget
            .try_reserve(now, &b, AttemptRetryIntent::Provider)
            .expect_err("provider switch should be rejected");
        assert_eq!(
            exhausted(error),
            AttemptBudgetExhausted::new(AttemptBudgetDimension::ProviderSwitches, 0, 0)
        );
        assert_eq!(budget.usage().unwrap().total_dispatches(), 1);
        assert_eq!(budget.usage().unwrap().credential_entries(), 1);
    }

    #[test]
    fn deadline_is_absolute_and_exact_deadline_is_closed() {
        let now = Instant::now();
        let deadline = now + Duration::from_millis(10);
        let budget = AttemptBudget::new(AttemptBudgetLimits::new(2, 2, 2, deadline));
        let target = target("provider-a", "endpoint-a", "key-a");
        let _permit = budget
            .try_reserve(now, &target, AttemptRetryIntent::Initial)
            .expect("dispatch before deadline should fit");

        let error = budget
            .try_reserve(deadline, &target, AttemptRetryIntent::SameTargetReplay)
            .expect_err("dispatch at the deadline must fail closed");
        assert_eq!(
            error,
            AttemptBudgetError::DeadlineExceeded {
                deadline,
                observed_at: deadline,
            }
        );
        assert_eq!(budget.usage().unwrap().total_dispatches(), 1);
    }

    #[test]
    fn invalid_retry_intents_never_consume_budget() {
        let now = Instant::now();
        let budget = AttemptBudget::new(limits(now, 4, 4, 4));
        let a = target("provider-a", "endpoint-a", "key-a");
        let b = target("provider-a", "endpoint-b", "key-a");

        assert_eq!(
            budget
                .try_reserve(now, &a, AttemptRetryIntent::Candidate)
                .unwrap_err(),
            AttemptBudgetError::InvalidTransition {
                intent: AttemptRetryIntent::Candidate,
                violation: AttemptTransitionViolation::InitialRequired,
            }
        );
        let _permit = budget
            .try_reserve(now, &a, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");
        assert_eq!(
            budget
                .try_reserve(now, &a, AttemptRetryIntent::Credential)
                .unwrap_err(),
            AttemptBudgetError::InvalidTransition {
                intent: AttemptRetryIntent::Credential,
                violation: AttemptTransitionViolation::CredentialUnchanged,
            }
        );
        assert_eq!(
            budget
                .try_reserve(now, &b, AttemptRetryIntent::SameTargetReplay)
                .unwrap_err(),
            AttemptBudgetError::InvalidTransition {
                intent: AttemptRetryIntent::SameTargetReplay,
                violation: AttemptTransitionViolation::SameTargetRequired,
            }
        );
        assert_eq!(budget.usage().unwrap().total_dispatches(), 1);
    }

    #[test]
    fn dropping_a_permit_never_refunds_its_reservation() {
        let now = Instant::now();
        let budget = AttemptBudget::new(limits(now, 1, 1, 0));
        let target = target("provider-a", "endpoint-a", "key-a");
        drop(
            budget
                .try_reserve(now, &target, AttemptRetryIntent::Initial)
                .expect("initial dispatch should fit"),
        );

        let error = budget
            .try_reserve(now, &target, AttemptRetryIntent::SameTargetReplay)
            .expect_err("dropped permit must remain charged");
        assert_eq!(
            exhausted(error),
            AttemptBudgetExhausted::new(AttemptBudgetDimension::TotalDispatches, 1, 1)
        );
    }

    #[test]
    fn cloned_handles_share_one_atomic_concurrent_limit() {
        const THREADS: usize = 16;
        const LIMIT: u64 = 5;

        let now = Instant::now();
        let budget = AttemptBudget::new(limits(now, LIMIT, 1, 0));
        let target = target("provider-a", "endpoint-a", "key-a");
        let _permit = budget
            .try_reserve(now, &target, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");
        let barrier = Arc::new(Barrier::new(THREADS));

        let handles = (0..THREADS)
            .map(|_| {
                let budget = budget.clone();
                let target = target.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    budget.try_reserve(now, &target, AttemptRetryIntent::SameTargetReplay)
                })
            })
            .collect::<Vec<_>>();

        let successes = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread should not panic"))
            .filter(Result::is_ok)
            .count();
        assert_eq!(successes, LIMIT as usize - 1);
        assert_eq!(budget.usage().unwrap().total_dispatches(), LIMIT);
    }

    #[test]
    fn poisoned_state_fails_closed() {
        let now = Instant::now();
        let budget = AttemptBudget::new(limits(now, 2, 2, 2));
        let poisoned = budget.clone();
        let _ = thread::spawn(move || {
            let _state = poisoned.state.lock().expect("state should start available");
            panic!("poison attempt budget state");
        })
        .join();

        assert_eq!(budget.usage(), Err(AttemptBudgetError::StateUnavailable));
        assert!(matches!(
            budget.try_reserve(
                now,
                &target("provider-a", "endpoint-a", "key-a"),
                AttemptRetryIntent::Initial,
            ),
            Err(AttemptBudgetError::StateUnavailable)
        ));
    }

    #[test]
    fn exhaustive_small_candidate_sequences_match_transition_counts() {
        let now = Instant::now();
        let targets = [
            target("provider-a", "endpoint-a", "key-a"),
            target("provider-a", "endpoint-b", "key-b"),
            target("provider-b", "endpoint-c", "key-c"),
        ];

        for encoded_sequence in 0usize..3usize.pow(5) {
            let mut encoded = encoded_sequence;
            let sequence = (0..5)
                .map(|_| {
                    let target = targets[encoded % targets.len()].clone();
                    encoded /= targets.len();
                    target
                })
                .collect::<Vec<_>>();
            let budget = AttemptBudget::new(limits(now, 5, 5, 4));

            for (index, target) in sequence.iter().enumerate() {
                let _permit = budget
                    .try_reserve(
                        now,
                        target,
                        if index == 0 {
                            AttemptRetryIntent::Initial
                        } else {
                            AttemptRetryIntent::Candidate
                        },
                    )
                    .expect("unbounded small sequence should fit");
            }

            let expected_credentials = 1 + sequence
                .windows(2)
                .filter(|pair| !pair[0].same_credential(&pair[1]))
                .count() as u64;
            let expected_provider_switches = sequence
                .windows(2)
                .filter(|pair| pair[0].provider_id != pair[1].provider_id)
                .count() as u64;
            assert_eq!(
                budget.usage().unwrap(),
                AttemptBudgetUsage {
                    total_dispatches: sequence.len() as u64,
                    credential_entries: expected_credentials,
                    provider_switches: expected_provider_switches,
                }
            );
        }
    }
}
