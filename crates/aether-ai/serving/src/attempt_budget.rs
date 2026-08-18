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

    fn same_provider(&self, other: &Self) -> bool {
        self.provider_id == other.provider_id
    }
}

impl From<&aether_contracts::ExecutionPlan> for AttemptTarget {
    fn from(plan: &aether_contracts::ExecutionPlan) -> Self {
        Self::new(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptRetryIntent {
    /// The first target entered by this request.
    Initial,
    /// A different candidate target; any identity dimension may change.
    Candidate,
    /// A different key on the same provider and endpoint.
    Credential,
    /// A different endpoint on the same provider. The key may stay the same or change; an actual
    /// key change consumes another credential entry.
    Endpoint,
    /// A different provider. Its endpoint and key are independently selected.
    Provider,
    /// An internal replay against exactly the same provider, endpoint, and key.
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
    CandidateUnchanged,
    CredentialScopeChanged,
    CredentialUnchanged,
    EndpointProviderChanged,
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

    /// Performs an advisory deadline check before planning or other auxiliary work.
    /// `try_reserve` always checks again after acquiring the shared state lock.
    pub fn ensure_dispatch_deadline(&self) -> Result<(), AttemptBudgetError> {
        self.check_dispatch_deadline(Instant::now())
    }

    fn check_dispatch_deadline(&self, observed_at: Instant) -> Result<(), AttemptBudgetError> {
        if observed_at >= self.limits.dispatch_deadline {
            return Err(AttemptBudgetError::DeadlineExceeded {
                deadline: self.limits.dispatch_deadline,
                observed_at,
            });
        }
        Ok(())
    }

    /// Atomically records budget consumption for a proposed dispatch.
    ///
    /// The returned evidence is not authority to send. A gateway-owned consuming send boundary
    /// must compose it with the request lifecycle before any physical dispatch is allowed.
    pub fn try_reserve(
        &self,
        target: &AttemptTarget,
        intent: AttemptRetryIntent,
    ) -> Result<AttemptBudgetReservation, AttemptBudgetError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AttemptBudgetError::StateUnavailable)?;
        let reserved_at = Instant::now();
        self.check_dispatch_deadline(reserved_at)?;
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

        Ok(AttemptBudgetReservation {
            ordinal: state.usage.total_dispatches,
            reserved_at,
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
        (Some(previous), AttemptRetryIntent::Candidate) => {
            (previous == target).then_some(AttemptTransitionViolation::CandidateUnchanged)
        }
        (Some(previous), AttemptRetryIntent::Credential) => {
            if !previous.same_provider(target) || previous.endpoint_id != target.endpoint_id {
                Some(AttemptTransitionViolation::CredentialScopeChanged)
            } else {
                (previous.key_id == target.key_id)
                    .then_some(AttemptTransitionViolation::CredentialUnchanged)
            }
        }
        (Some(previous), AttemptRetryIntent::Endpoint) => {
            if !previous.same_provider(target) {
                Some(AttemptTransitionViolation::EndpointProviderChanged)
            } else {
                (previous.endpoint_id == target.endpoint_id)
                    .then_some(AttemptTransitionViolation::EndpointUnchanged)
            }
        }
        (Some(previous), AttemptRetryIntent::Provider) => previous
            .same_provider(target)
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
#[must_use = "reservation evidence should be recorded by a gateway-owned dispatch boundary"]
/// Non-authoritative evidence that budget dimensions were charged.
///
/// This value carries no target identity and cannot authorize or perform a dispatch by itself.
pub struct AttemptBudgetReservation {
    ordinal: u64,
    reserved_at: Instant,
    usage: AttemptBudgetUsage,
}

impl AttemptBudgetReservation {
    pub fn ordinal(&self) -> u64 {
        self.ordinal
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

    fn limits(total: u64, credentials: u64, providers: u64) -> AttemptBudgetLimits {
        AttemptBudgetLimits::new(
            total,
            credentials,
            providers,
            Instant::now() + Duration::from_secs(60),
        )
    }

    fn exhausted(error: AttemptBudgetError) -> AttemptBudgetExhausted {
        match error {
            AttemptBudgetError::BudgetExhausted(exhausted) => exhausted,
            other => panic!("expected budget exhaustion, got {other:?}"),
        }
    }

    fn assert_violation(
        result: Result<AttemptBudgetReservation, AttemptBudgetError>,
        intent: AttemptRetryIntent,
        violation: AttemptTransitionViolation,
    ) {
        assert_eq!(
            result.expect_err("transition should be rejected"),
            AttemptBudgetError::InvalidTransition { intent, violation }
        );
    }

    #[test]
    fn first_dispatch_consumes_total_and_credential_but_not_provider_switch() {
        let budget = AttemptBudget::new(limits(3, 3, 3));
        let target = target("provider-a", "endpoint-a", "key-a");

        let reservation = budget
            .try_reserve(&target, AttemptRetryIntent::Initial)
            .expect("first dispatch should fit");

        assert_eq!(reservation.ordinal(), 1);
        assert_eq!(reservation.usage().total_dispatches(), 1);
        assert_eq!(reservation.usage().credential_entries(), 1);
        assert_eq!(reservation.usage().provider_switches(), 0);

        let evidence = format!("{reservation:?}");
        assert!(!evidence.contains("provider-a"));
        assert!(!evidence.contains("endpoint-a"));
        assert!(!evidence.contains("key-a"));
    }

    #[test]
    fn same_target_replay_only_consumes_total_dispatches() {
        let budget = AttemptBudget::new(limits(3, 1, 0));
        let target = target("provider-a", "endpoint-a", "key-a");
        let _reservation = budget
            .try_reserve(&target, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");

        let replay = budget
            .try_reserve(&target, AttemptRetryIntent::SameTargetReplay)
            .expect("same target replay should not re-enter credential");

        assert_eq!(replay.ordinal(), 2);
        assert_eq!(replay.usage().credential_entries(), 1);
        assert_eq!(replay.usage().provider_switches(), 0);
    }

    #[test]
    fn actual_target_transitions_determine_all_dimension_deltas() {
        let budget = AttemptBudget::new(limits(5, 5, 5));
        let a = target("provider-a", "endpoint-a", "key-a");
        let new_credential = target("provider-a", "endpoint-a", "key-b");
        let new_endpoint_same_credential = target("provider-a", "endpoint-b", "key-b");
        let new_endpoint_and_credential = target("provider-a", "endpoint-c", "key-c");
        let new_provider = target("provider-b", "endpoint-d", "key-d");

        let _reservation = budget
            .try_reserve(&a, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");
        let _reservation = budget
            .try_reserve(&new_credential, AttemptRetryIntent::Credential)
            .expect("credential transition should fit");
        let _reservation = budget
            .try_reserve(&new_endpoint_same_credential, AttemptRetryIntent::Endpoint)
            .expect("endpoint transition may retain the current credential");
        let _reservation = budget
            .try_reserve(&new_endpoint_and_credential, AttemptRetryIntent::Endpoint)
            .expect("endpoint transition may enter a different credential");
        let reservation = budget
            .try_reserve(&new_provider, AttemptRetryIntent::Provider)
            .expect("provider transition should fit");

        assert_eq!(reservation.usage().total_dispatches(), 5);
        assert_eq!(reservation.usage().credential_entries(), 4);
        assert_eq!(reservation.usage().provider_switches(), 1);
    }

    #[test]
    fn returning_to_a_previous_target_consumes_transitions_again() {
        let budget = AttemptBudget::new(limits(3, 3, 2));
        let a = target("provider-a", "endpoint-a", "key-a");
        let b = target("provider-b", "endpoint-b", "key-b");

        let _reservation = budget
            .try_reserve(&a, AttemptRetryIntent::Initial)
            .expect("A should fit");
        let _reservation = budget
            .try_reserve(&b, AttemptRetryIntent::Provider)
            .expect("A to B should fit");
        let reservation = budget
            .try_reserve(&a, AttemptRetryIntent::Provider)
            .expect("B to A should fit");

        assert_eq!(reservation.usage().credential_entries(), 3);
        assert_eq!(reservation.usage().provider_switches(), 2);
    }

    #[test]
    fn failed_multidimension_reservation_has_no_partial_charge() {
        let budget = AttemptBudget::new(limits(4, 1, 4));
        let a = target("provider-a", "endpoint-a", "key-a");
        let b = target("provider-b", "endpoint-b", "key-b");
        let _reservation = budget
            .try_reserve(&a, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");

        let error = budget
            .try_reserve(&b, AttemptRetryIntent::Provider)
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
            .try_reserve(&a, AttemptRetryIntent::SameTargetReplay)
            .expect("failed transition must not change the previous target");
        assert_eq!(replay.ordinal(), 2);
    }

    #[test]
    fn exhaustion_dimension_order_is_deterministic_and_limits_allow_zero() {
        let target = target("provider-a", "endpoint-a", "key-a");

        let zero_total = AttemptBudget::new(limits(0, 0, 0));
        assert_eq!(
            exhausted(
                zero_total
                    .try_reserve(&target, AttemptRetryIntent::Initial)
                    .expect_err("zero total budget must reject dispatch")
            )
            .dimension(),
            AttemptBudgetDimension::TotalDispatches
        );

        let zero_credentials = AttemptBudget::new(limits(1, 0, 0));
        assert_eq!(
            exhausted(
                zero_credentials
                    .try_reserve(&target, AttemptRetryIntent::Initial)
                    .expect_err("zero credential budget must reject dispatch")
            )
            .dimension(),
            AttemptBudgetDimension::CredentialEntries
        );
    }

    #[test]
    fn provider_limit_failure_does_not_charge_total_or_credential() {
        let budget = AttemptBudget::new(limits(3, 3, 0));
        let a = target("provider-a", "endpoint-a", "key-a");
        let b = target("provider-b", "endpoint-b", "key-b");
        let _reservation = budget
            .try_reserve(&a, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");

        let error = budget
            .try_reserve(&b, AttemptRetryIntent::Provider)
            .expect_err("provider switch should be rejected");
        assert_eq!(
            exhausted(error),
            AttemptBudgetExhausted::new(AttemptBudgetDimension::ProviderSwitches, 0, 0)
        );
        assert_eq!(budget.usage().unwrap().total_dispatches(), 1);
        assert_eq!(budget.usage().unwrap().credential_entries(), 1);
    }

    #[test]
    fn expired_deadline_fails_closed_without_a_caller_supplied_clock() {
        let deadline = Instant::now() - Duration::from_millis(1);
        let budget = AttemptBudget::new(AttemptBudgetLimits::new(2, 2, 2, deadline));
        let target = target("provider-a", "endpoint-a", "key-a");
        let error = budget
            .try_reserve(&target, AttemptRetryIntent::Initial)
            .expect_err("expired deadline must reject the reservation");

        match error {
            AttemptBudgetError::DeadlineExceeded {
                deadline: actual_deadline,
                observed_at,
            } => {
                assert_eq!(actual_deadline, deadline);
                assert!(observed_at >= deadline);
            }
            other => panic!("expected deadline exhaustion, got {other:?}"),
        }
        assert_eq!(budget.usage().unwrap().total_dispatches(), 0);
    }

    #[test]
    fn lock_wait_counts_toward_the_absolute_deadline() {
        let deadline = Instant::now() + Duration::from_millis(100);
        let budget = AttemptBudget::new(AttemptBudgetLimits::new(1, 1, 0, deadline));
        let target = target("provider-a", "endpoint-a", "key-a");
        let state_guard = budget.state.lock().expect("state should start available");
        let started = Arc::new(Barrier::new(2));
        let waiting_budget = budget.clone();
        let waiting_started = started.clone();
        let waiter = thread::spawn(move || {
            waiting_started.wait();
            waiting_budget.try_reserve(&target, AttemptRetryIntent::Initial)
        });

        started.wait();
        thread::sleep(Duration::from_millis(150));
        drop(state_guard);

        assert!(matches!(
            waiter.join().expect("waiter should not panic"),
            Err(AttemptBudgetError::DeadlineExceeded { observed_at, .. })
                if observed_at >= deadline
        ));
        assert_eq!(budget.usage().unwrap().total_dispatches(), 0);
    }

    #[test]
    fn strict_retry_intents_reject_out_of_scope_targets_without_consuming_budget() {
        let budget = AttemptBudget::new(limits(8, 8, 8));
        let a = target("provider-a", "endpoint-a", "key-a");
        assert_violation(
            budget.try_reserve(&a, AttemptRetryIntent::Candidate),
            AttemptRetryIntent::Candidate,
            AttemptTransitionViolation::InitialRequired,
        );
        let _reservation = budget
            .try_reserve(&a, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");

        let cases = [
            (
                a.clone(),
                AttemptRetryIntent::Initial,
                AttemptTransitionViolation::AlreadyStarted,
            ),
            (
                a.clone(),
                AttemptRetryIntent::Candidate,
                AttemptTransitionViolation::CandidateUnchanged,
            ),
            (
                a.clone(),
                AttemptRetryIntent::Credential,
                AttemptTransitionViolation::CredentialUnchanged,
            ),
            (
                target("provider-a", "endpoint-b", "key-b"),
                AttemptRetryIntent::Credential,
                AttemptTransitionViolation::CredentialScopeChanged,
            ),
            (
                target("provider-b", "endpoint-a", "key-b"),
                AttemptRetryIntent::Credential,
                AttemptTransitionViolation::CredentialScopeChanged,
            ),
            (
                target("provider-b", "endpoint-b", "key-b"),
                AttemptRetryIntent::Endpoint,
                AttemptTransitionViolation::EndpointProviderChanged,
            ),
            (
                target("provider-a", "endpoint-a", "key-b"),
                AttemptRetryIntent::Endpoint,
                AttemptTransitionViolation::EndpointUnchanged,
            ),
            (
                target("provider-a", "endpoint-b", "key-b"),
                AttemptRetryIntent::Provider,
                AttemptTransitionViolation::ProviderUnchanged,
            ),
            (
                target("provider-a", "endpoint-b", "key-a"),
                AttemptRetryIntent::SameTargetReplay,
                AttemptTransitionViolation::SameTargetRequired,
            ),
        ];
        for (next, intent, violation) in cases {
            assert_violation(budget.try_reserve(&next, intent), intent, violation);
        }
        assert_eq!(
            budget.usage().unwrap(),
            AttemptBudgetUsage {
                total_dispatches: 1,
                credential_entries: 1,
                provider_switches: 0,
            }
        );
    }

    #[test]
    fn dropping_reservation_evidence_never_refunds_its_charge() {
        let budget = AttemptBudget::new(limits(1, 1, 0));
        let target = target("provider-a", "endpoint-a", "key-a");
        drop(
            budget
                .try_reserve(&target, AttemptRetryIntent::Initial)
                .expect("initial dispatch should fit"),
        );

        let error = budget
            .try_reserve(&target, AttemptRetryIntent::SameTargetReplay)
            .expect_err("dropped reservation evidence must remain charged");
        assert_eq!(
            exhausted(error),
            AttemptBudgetExhausted::new(AttemptBudgetDimension::TotalDispatches, 1, 1)
        );
    }

    #[test]
    fn cloned_handles_share_one_atomic_concurrent_limit() {
        const THREADS: usize = 16;
        const LIMIT: u64 = 5;

        let budget = AttemptBudget::new(limits(LIMIT, 1, 0));
        let target = target("provider-a", "endpoint-a", "key-a");
        let _reservation = budget
            .try_reserve(&target, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");
        let barrier = Arc::new(Barrier::new(THREADS));

        let handles = (0..THREADS)
            .map(|_| {
                let budget = budget.clone();
                let target = target.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    budget.try_reserve(&target, AttemptRetryIntent::SameTargetReplay)
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
        let budget = AttemptBudget::new(limits(2, 2, 2));
        let poisoned = budget.clone();
        let _ = thread::spawn(move || {
            let _state = poisoned.state.lock().expect("state should start available");
            panic!("poison attempt budget state");
        })
        .join();

        assert_eq!(budget.usage(), Err(AttemptBudgetError::StateUnavailable));
        assert!(matches!(
            budget.try_reserve(
                &target("provider-a", "endpoint-a", "key-a"),
                AttemptRetryIntent::Initial,
            ),
            Err(AttemptBudgetError::StateUnavailable)
        ));
    }

    #[test]
    fn exhaustive_small_candidate_sequences_match_transition_counts() {
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
            let budget = AttemptBudget::new(limits(5, 5, 4));

            for (index, target) in sequence.iter().enumerate() {
                let intent = if index == 0 {
                    AttemptRetryIntent::Initial
                } else if sequence[index - 1] == *target {
                    AttemptRetryIntent::SameTargetReplay
                } else {
                    AttemptRetryIntent::Candidate
                };
                let _reservation = budget
                    .try_reserve(target, intent)
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
