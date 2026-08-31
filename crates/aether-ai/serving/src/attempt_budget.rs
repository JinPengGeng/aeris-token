use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

trait AttemptClock: fmt::Debug + Send + Sync {
    fn now(&self) -> Instant;
}

static NEXT_ATTEMPT_GRANT_ID: AtomicU64 = AtomicU64::new(1);
pub const MAX_REQUEST_ATTEMPT_TOTAL_DISPATCHES: u64 = 64;
pub const MAX_REQUEST_ATTEMPT_CREDENTIAL_ENTRIES: u64 = 32;
pub const MAX_REQUEST_ATTEMPT_PROVIDER_SWITCHES: u64 = 16;

#[derive(Debug)]
struct SystemAttemptClock;

impl AttemptClock for SystemAttemptClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

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
    /// A different endpoint and key on the same provider.
    ///
    /// Credential-only, endpoint-only, and provider transitions use their dedicated intents so
    /// callers cannot hide a narrower retry disposition behind this variant.
    Candidate,
    /// A different key on the same provider and endpoint.
    Credential,
    /// A different endpoint on the same provider using the same key; only the endpoint changes.
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
    CandidateProviderChanged,
    CandidateEndpointUnchanged,
    CandidateCredentialUnchanged,
    CredentialProviderChanged,
    CredentialEndpointChanged,
    CredentialUnchanged,
    EndpointProviderChanged,
    EndpointCredentialChanged,
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
    DispatchTargetMismatch,
    InvalidGrant,
    ScopeMissing,
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
            Self::DispatchTargetMismatch => {
                formatter.write_str("attempt budget dispatch target does not match reservation")
            }
            Self::InvalidGrant => formatter.write_str("attempt budget delegation grant is invalid"),
            Self::ScopeMissing => formatter.write_str("request attempt budget scope is missing"),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptBudgetTerminalCause {
    BudgetExhausted(AttemptBudgetDimension),
    DeadlineExceeded,
    InvalidTransition,
    DispatchTargetMismatch,
    InvalidGrant,
    ScopeMissing,
    StateUnavailable,
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
    last_target_sequence: u64,
    outstanding_delegations: BTreeMap<String, OutstandingDelegation>,
    terminal_cause: Option<AttemptBudgetTerminalCause>,
}

#[derive(Debug, Clone)]
struct OutstandingDelegation {
    sequence: u64,
    reserved_usage: AttemptBudgetUsage,
    target: AttemptTarget,
}

#[derive(Debug, Clone)]
pub struct AttemptBudget {
    limits: AttemptBudgetLimits,
    state: Arc<Mutex<AttemptBudgetState>>,
    clock: Arc<dyn AttemptClock>,
}

impl AttemptBudget {
    pub fn new(limits: AttemptBudgetLimits) -> Self {
        Self::with_clock(limits, Arc::new(SystemAttemptClock))
    }

    pub fn from_delegation_grant(
        grant: &aether_contracts::ExecutionAttemptBudgetGrant,
    ) -> Result<Self, AttemptBudgetError> {
        let now_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        if grant.grant_id.trim().is_empty() {
            return Err(AttemptBudgetError::InvalidGrant);
        }
        if grant.max_total_dispatches == 0
            || grant.max_total_dispatches > MAX_REQUEST_ATTEMPT_TOTAL_DISPATCHES
            || grant.max_credential_entries > MAX_REQUEST_ATTEMPT_CREDENTIAL_ENTRIES
            || grant.max_provider_switches > MAX_REQUEST_ATTEMPT_PROVIDER_SWITCHES
            || grant.deadline_unix_ms.saturating_sub(now_unix_ms)
                > aether_contracts::MAX_EXECUTION_REQUEST_TIMEOUT_MS
        {
            return Err(AttemptBudgetError::InvalidGrant);
        }
        if grant.deadline_unix_ms <= now_unix_ms {
            let now = Instant::now();
            return Err(AttemptBudgetError::DeadlineExceeded {
                deadline: now,
                observed_at: now,
            });
        }
        let deadline =
            Instant::now() + std::time::Duration::from_millis(grant.deadline_unix_ms - now_unix_ms);
        Ok(Self::new(AttemptBudgetLimits::new(
            grant.max_total_dispatches,
            grant.max_credential_entries,
            grant.max_provider_switches,
            deadline,
        )))
    }

    fn with_clock(limits: AttemptBudgetLimits, clock: Arc<dyn AttemptClock>) -> Self {
        Self {
            limits,
            state: Arc::new(Mutex::new(AttemptBudgetState {
                usage: AttemptBudgetUsage::default(),
                previous_target: None,
                last_target_sequence: 0,
                outstanding_delegations: BTreeMap::new(),
                terminal_cause: None,
            })),
            clock,
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

    pub fn terminal_cause(&self) -> Option<AttemptBudgetTerminalCause> {
        self.state
            .lock()
            .map(|state| state.terminal_cause)
            .unwrap_or(Some(AttemptBudgetTerminalCause::StateUnavailable))
    }

    pub fn remaining(&self) -> Result<std::time::Duration, AttemptBudgetError> {
        let observed_at = self.check_dispatch_deadline()?;
        Ok(self
            .limits
            .dispatch_deadline
            .saturating_duration_since(observed_at))
    }

    /// Atomically reserves one remote provider dispatch from this request.
    ///
    /// A remote execution-runtime request carries exactly one already-selected plan, so granting
    /// more than one provider dispatch would let a missing stream trailer consume the entire
    /// request budget. The gateway-to-runtime hop is charged separately at its physical send
    /// boundary. This grant accounts for the one provider-facing send performed by the runtime;
    /// the gateway-to-runtime RPC is an infrastructure hop and is not counted again.
    pub fn reserve_delegation_grant(
        &self,
        target: &AttemptTarget,
    ) -> Result<AttemptBudgetDelegation, AttemptBudgetError> {
        let remaining = self.remaining()?;
        let now_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut state = self
            .state
            .lock()
            .map_err(|_| AttemptBudgetError::StateUnavailable)?;
        if let Err(error) = self.check_dispatch_deadline() {
            state.terminal_cause = Some(AttemptBudgetTerminalCause::DeadlineExceeded);
            return Err(error);
        }
        let sequence = NEXT_ATTEMPT_GRANT_ID.fetch_add(1, Ordering::Relaxed);
        let grant_id = format!("{now_unix_ms}-{sequence}");
        let previous = effective_previous_target(&state);
        let reserved = AttemptBudgetUsage {
            total_dispatches: 1,
            credential_entries: u64::from(
                previous.is_none_or(|previous| !previous.same_credential(target)),
            ),
            provider_switches: u64::from(
                previous.is_some_and(|previous| !previous.same_provider(target)),
            ),
        };
        let outstanding = outstanding_usage(&state);
        for (used, delta, limit, dimension) in [
            (
                state
                    .usage
                    .total_dispatches
                    .saturating_add(outstanding.total_dispatches),
                reserved.total_dispatches,
                self.limits.max_total_dispatches,
                AttemptBudgetDimension::TotalDispatches,
            ),
            (
                state
                    .usage
                    .credential_entries
                    .saturating_add(outstanding.credential_entries),
                reserved.credential_entries,
                self.limits.max_credential_entries,
                AttemptBudgetDimension::CredentialEntries,
            ),
            (
                state
                    .usage
                    .provider_switches
                    .saturating_add(outstanding.provider_switches),
                reserved.provider_switches,
                self.limits.max_provider_switches,
                AttemptBudgetDimension::ProviderSwitches,
            ),
        ] {
            if let Err(error) = check_dimension(used, delta, limit, dimension) {
                state.terminal_cause = Some(AttemptBudgetTerminalCause::BudgetExhausted(dimension));
                return Err(error);
            }
        }
        state.outstanding_delegations.insert(
            grant_id.clone(),
            OutstandingDelegation {
                sequence,
                reserved_usage: reserved,
                target: target.clone(),
            },
        );
        Ok(AttemptBudgetDelegation {
            budget: self.clone(),
            grant: aether_contracts::ExecutionAttemptBudgetGrant {
                grant_id,
                max_total_dispatches: 1,
                max_credential_entries: 1,
                max_provider_switches: 0,
                deadline_unix_ms: now_unix_ms.saturating_add(remaining.as_millis() as u64),
            },
            settled: false,
        })
    }

    fn reconcile_delegated_consumption(
        &self,
        grant_id: &str,
        consumption: aether_contracts::ExecutionAttemptBudgetConsumption,
    ) -> Result<(), AttemptBudgetError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AttemptBudgetError::StateUnavailable)?;
        let Some(outstanding) = state.outstanding_delegations.remove(grant_id) else {
            state.terminal_cause = Some(AttemptBudgetTerminalCause::InvalidGrant);
            return Err(AttemptBudgetError::InvalidGrant);
        };
        if consumption.total_dispatches > outstanding.reserved_usage.total_dispatches
            || consumption.credential_entries > 1
            || consumption.provider_switches > 0
        {
            state
                .outstanding_delegations
                .insert(grant_id.to_string(), outstanding);
            state.terminal_cause = Some(AttemptBudgetTerminalCause::InvalidGrant);
            return Err(AttemptBudgetError::InvalidGrant);
        }
        let dispatched = u64::from(consumption.total_dispatches > 0);
        let credential_delta = outstanding.reserved_usage.credential_entries * dispatched;
        let provider_delta = outstanding.reserved_usage.provider_switches * dispatched;
        let remaining_outstanding = outstanding_usage(&state);
        for (used, delta, limit, dimension) in [
            (
                state
                    .usage
                    .total_dispatches
                    .saturating_add(remaining_outstanding.total_dispatches),
                consumption.total_dispatches,
                self.limits.max_total_dispatches,
                AttemptBudgetDimension::TotalDispatches,
            ),
            (
                state
                    .usage
                    .credential_entries
                    .saturating_add(remaining_outstanding.credential_entries),
                credential_delta,
                self.limits.max_credential_entries,
                AttemptBudgetDimension::CredentialEntries,
            ),
            (
                state
                    .usage
                    .provider_switches
                    .saturating_add(remaining_outstanding.provider_switches),
                provider_delta,
                self.limits.max_provider_switches,
                AttemptBudgetDimension::ProviderSwitches,
            ),
        ] {
            if let Err(error) = check_dimension(used, delta, limit, dimension) {
                state.terminal_cause = Some(AttemptBudgetTerminalCause::BudgetExhausted(dimension));
                return Err(error);
            }
        }
        state.usage.total_dispatches += consumption.total_dispatches;
        state.usage.credential_entries += credential_delta;
        state.usage.provider_switches += provider_delta;
        if dispatched > 0 && outstanding.sequence > state.last_target_sequence {
            state.last_target_sequence = outstanding.sequence;
            state.previous_target = Some(outstanding.target);
        }
        Ok(())
    }

    fn release_delegation(&self, grant_id: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.outstanding_delegations.remove(grant_id);
        }
    }

    /// Performs an advisory deadline check before planning or other auxiliary work.
    /// `try_reserve` always checks again after acquiring the shared state lock.
    pub fn ensure_dispatch_deadline(&self) -> Result<(), AttemptBudgetError> {
        self.check_dispatch_deadline().map(|_| ())
    }

    fn check_dispatch_deadline(&self) -> Result<Instant, AttemptBudgetError> {
        let observed_at = self.clock.now();
        if observed_at >= self.limits.dispatch_deadline {
            return Err(AttemptBudgetError::DeadlineExceeded {
                deadline: self.limits.dispatch_deadline,
                observed_at,
            });
        }
        Ok(observed_at)
    }

    /// Prepares a target-bound reservation without charging the budget.
    ///
    /// Charging and transition ordering happen only when the reservation is consumed at the
    /// physical send boundary. This prevents concurrently prepared sends from undercounting
    /// credential or provider transitions when they reach the transport out of order.
    pub fn try_reserve(
        &self,
        target: &AttemptTarget,
        intent: AttemptRetryIntent,
    ) -> Result<AttemptBudgetReservation, AttemptBudgetError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AttemptBudgetError::StateUnavailable)?;
        let reserved_at = match self.check_dispatch_deadline() {
            Ok(observed_at) => observed_at,
            Err(error) => {
                state.terminal_cause = Some(AttemptBudgetTerminalCause::DeadlineExceeded);
                return Err(error);
            }
        };
        let previous = effective_previous_target(&state);
        if let Err(error) = validate_transition(previous, target, intent) {
            state.terminal_cause = Some(AttemptBudgetTerminalCause::InvalidTransition);
            return Err(error);
        }
        let credential_delta =
            u64::from(previous.is_none_or(|previous| !previous.same_credential(target)));
        let provider_switch_delta =
            u64::from(previous.is_some_and(|previous| previous.provider_id != target.provider_id));
        let outstanding = outstanding_usage(&state);
        for (used, delta, limit, dimension) in [
            (
                state
                    .usage
                    .total_dispatches
                    .saturating_add(outstanding.total_dispatches),
                1,
                self.limits.max_total_dispatches,
                AttemptBudgetDimension::TotalDispatches,
            ),
            (
                state
                    .usage
                    .credential_entries
                    .saturating_add(outstanding.credential_entries),
                credential_delta,
                self.limits.max_credential_entries,
                AttemptBudgetDimension::CredentialEntries,
            ),
            (
                state
                    .usage
                    .provider_switches
                    .saturating_add(outstanding.provider_switches),
                provider_switch_delta,
                self.limits.max_provider_switches,
                AttemptBudgetDimension::ProviderSwitches,
            ),
        ] {
            if let Err(error) = check_dimension(used, delta, limit, dimension) {
                state.terminal_cause = Some(AttemptBudgetTerminalCause::BudgetExhausted(dimension));
                return Err(error);
            }
        }
        let predicted_usage = AttemptBudgetUsage {
            total_dispatches: state.usage.total_dispatches + 1,
            credential_entries: state.usage.credential_entries + credential_delta,
            provider_switches: state.usage.provider_switches + provider_switch_delta,
        };

        Ok(AttemptBudgetReservation {
            budget: self.clone(),
            target: target.clone(),
            intent,
            reserved_at,
            predicted_usage,
        })
    }

    /// Atomically charges and authorizes the next physical dispatch using its actual target.
    ///
    /// This is the sealed transport integration entrypoint. Transition intent is derived from the
    /// last physically authorized target, so concurrent callers linearize in send-admission order.
    pub fn authorize_next_dispatch(
        &self,
        target: &AttemptTarget,
    ) -> Result<AttemptDispatchPermit, AttemptBudgetError> {
        let state = self
            .state
            .lock()
            .map_err(|_| AttemptBudgetError::StateUnavailable)?;
        let authorized_at = self.check_dispatch_deadline()?;
        Ok(AttemptDispatchPermit {
            budget: self.clone(),
            target: target.clone(),
            intent: None,
            reserved_at: authorized_at,
            authorized_at,
            predicted_ordinal: state.usage.total_dispatches.saturating_add(1),
            predicted_usage: predicted_usage(&state, target),
        })
    }

    fn charge_dispatch(
        &self,
        state: &mut AttemptBudgetState,
        target: &AttemptTarget,
        intent: AttemptRetryIntent,
    ) -> Result<(u64, AttemptBudgetUsage), AttemptBudgetError> {
        let previous = effective_previous_target(state);
        if let Err(error) = validate_transition(previous, target, intent) {
            state.terminal_cause = Some(AttemptBudgetTerminalCause::InvalidTransition);
            return Err(error);
        }
        let credential_delta =
            u64::from(previous.is_none_or(|previous| !previous.same_credential(target)));
        let provider_switch_delta =
            u64::from(previous.is_some_and(|previous| previous.provider_id != target.provider_id));
        let outstanding = outstanding_usage(state);
        for (used, delta, limit, dimension) in [
            (
                state
                    .usage
                    .total_dispatches
                    .saturating_add(outstanding.total_dispatches),
                1,
                self.limits.max_total_dispatches,
                AttemptBudgetDimension::TotalDispatches,
            ),
            (
                state
                    .usage
                    .credential_entries
                    .saturating_add(outstanding.credential_entries),
                credential_delta,
                self.limits.max_credential_entries,
                AttemptBudgetDimension::CredentialEntries,
            ),
            (
                state
                    .usage
                    .provider_switches
                    .saturating_add(outstanding.provider_switches),
                provider_switch_delta,
                self.limits.max_provider_switches,
                AttemptBudgetDimension::ProviderSwitches,
            ),
        ] {
            if let Err(error) = check_dimension(used, delta, limit, dimension) {
                state.terminal_cause = Some(AttemptBudgetTerminalCause::BudgetExhausted(dimension));
                return Err(error);
            }
        }
        state.usage.total_dispatches += 1;
        state.usage.credential_entries += credential_delta;
        state.usage.provider_switches += provider_switch_delta;
        state.last_target_sequence = NEXT_ATTEMPT_GRANT_ID.fetch_add(1, Ordering::Relaxed);
        state.previous_target = Some(target.clone());
        Ok((state.usage.total_dispatches, state.usage))
    }
}

fn predicted_usage(state: &AttemptBudgetState, target: &AttemptTarget) -> AttemptBudgetUsage {
    let previous = effective_previous_target(state);
    let credential_delta =
        u64::from(previous.is_none_or(|previous| !previous.same_credential(target)));
    let provider_switch_delta =
        u64::from(previous.is_some_and(|previous| previous.provider_id != target.provider_id));
    AttemptBudgetUsage {
        total_dispatches: state.usage.total_dispatches.saturating_add(1),
        credential_entries: state
            .usage
            .credential_entries
            .saturating_add(credential_delta),
        provider_switches: state
            .usage
            .provider_switches
            .saturating_add(provider_switch_delta),
    }
}

fn add_usage(left: AttemptBudgetUsage, right: AttemptBudgetUsage) -> AttemptBudgetUsage {
    AttemptBudgetUsage {
        total_dispatches: left.total_dispatches.saturating_add(right.total_dispatches),
        credential_entries: left
            .credential_entries
            .saturating_add(right.credential_entries),
        provider_switches: left
            .provider_switches
            .saturating_add(right.provider_switches),
    }
}

fn outstanding_usage(state: &AttemptBudgetState) -> AttemptBudgetUsage {
    state
        .outstanding_delegations
        .values()
        .map(|delegation| delegation.reserved_usage)
        .fold(AttemptBudgetUsage::default(), add_usage)
}

fn effective_previous_target(state: &AttemptBudgetState) -> Option<&AttemptTarget> {
    let outstanding_tail = state
        .outstanding_delegations
        .values()
        .max_by_key(|delegation| delegation.sequence);
    match outstanding_tail {
        Some(delegation) if delegation.sequence > state.last_target_sequence => {
            Some(&delegation.target)
        }
        _ => state.previous_target.as_ref(),
    }
}

/// One outstanding remote-runtime reservation. Dropping it before a send releases capacity;
/// callers must use conservative reconciliation once the send outcome becomes uncertain.
pub struct AttemptBudgetDelegation {
    budget: AttemptBudget,
    grant: aether_contracts::ExecutionAttemptBudgetGrant,
    settled: bool,
}

impl fmt::Debug for AttemptBudgetDelegation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttemptBudgetDelegation")
            .field("grant_id", &self.grant.grant_id)
            .field("settled", &self.settled)
            .finish_non_exhaustive()
    }
}

impl AttemptBudgetDelegation {
    pub fn grant(&self) -> &aether_contracts::ExecutionAttemptBudgetGrant {
        &self.grant
    }

    pub fn reconcile(
        mut self,
        consumption: aether_contracts::ExecutionAttemptBudgetConsumption,
    ) -> Result<(), AttemptBudgetError> {
        self.budget
            .reconcile_delegated_consumption(&self.grant.grant_id, consumption)?;
        self.settled = true;
        Ok(())
    }

    pub fn reconcile_conservative(self) -> Result<(), AttemptBudgetError> {
        let consumption = aether_contracts::ExecutionAttemptBudgetConsumption {
            total_dispatches: self.grant.max_total_dispatches,
            credential_entries: self.grant.max_credential_entries,
            provider_switches: self.grant.max_provider_switches,
        };
        self.reconcile(consumption)
    }
}

impl Drop for AttemptBudgetDelegation {
    fn drop(&mut self) {
        if !self.settled {
            self.budget.release_delegation(&self.grant.grant_id);
        }
    }
}

fn infer_transition_intent(
    previous: Option<&AttemptTarget>,
    target: &AttemptTarget,
) -> AttemptRetryIntent {
    match previous {
        None => AttemptRetryIntent::Initial,
        Some(previous) if previous == target => AttemptRetryIntent::SameTargetReplay,
        Some(previous) if !previous.same_provider(target) => AttemptRetryIntent::Provider,
        Some(previous)
            if previous.endpoint_id != target.endpoint_id && previous.key_id != target.key_id =>
        {
            AttemptRetryIntent::Candidate
        }
        Some(previous) if previous.endpoint_id != target.endpoint_id => {
            AttemptRetryIntent::Endpoint
        }
        Some(_) => AttemptRetryIntent::Credential,
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
            if !previous.same_provider(target) {
                Some(AttemptTransitionViolation::CandidateProviderChanged)
            } else if previous.endpoint_id == target.endpoint_id {
                Some(AttemptTransitionViolation::CandidateEndpointUnchanged)
            } else {
                (previous.key_id == target.key_id)
                    .then_some(AttemptTransitionViolation::CandidateCredentialUnchanged)
            }
        }
        (Some(previous), AttemptRetryIntent::Credential) => {
            if !previous.same_provider(target) {
                Some(AttemptTransitionViolation::CredentialProviderChanged)
            } else if previous.endpoint_id != target.endpoint_id {
                Some(AttemptTransitionViolation::CredentialEndpointChanged)
            } else {
                (previous.key_id == target.key_id)
                    .then_some(AttemptTransitionViolation::CredentialUnchanged)
            }
        }
        (Some(previous), AttemptRetryIntent::Endpoint) => {
            if !previous.same_provider(target) {
                Some(AttemptTransitionViolation::EndpointProviderChanged)
            } else if previous.key_id != target.key_id {
                Some(AttemptTransitionViolation::EndpointCredentialChanged)
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

#[must_use = "reservation evidence should be recorded by a gateway-owned dispatch boundary"]
/// Budget evidence that must be consumed immediately before one physical dispatch.
///
/// Construction and fields stay private so callers cannot forge reservations. The consuming
/// boundary checks the trusted clock again and issues one non-cloneable, target-bound permit.
pub struct AttemptBudgetReservation {
    budget: AttemptBudget,
    target: AttemptTarget,
    intent: AttemptRetryIntent,
    reserved_at: Instant,
    predicted_usage: AttemptBudgetUsage,
}

impl fmt::Debug for AttemptBudgetReservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttemptBudgetReservation")
            .field("reserved_at", &self.reserved_at)
            .field("predicted_usage", &self.predicted_usage)
            .finish_non_exhaustive()
    }
}

impl AttemptBudgetReservation {
    pub fn ordinal(&self) -> u64 {
        self.predicted_usage.total_dispatches
    }

    pub fn reserved_at(&self) -> Instant {
        self.reserved_at
    }

    pub fn usage(&self) -> AttemptBudgetUsage {
        self.predicted_usage
    }

    /// Consumes this reservation's one-shot authority at the physical dispatch boundary.
    ///
    /// Transport entrypoints must accept the returned permit by value. An expired or mismatched
    /// reservation fails closed without charging a dispatch that never reached admission.
    pub fn consume_for_dispatch(
        self,
        actual_target: &AttemptTarget,
    ) -> Result<AttemptDispatchPermit, AttemptBudgetError> {
        if self.target != *actual_target {
            if let Ok(mut state) = self.budget.state.lock() {
                state.terminal_cause = Some(AttemptBudgetTerminalCause::DispatchTargetMismatch);
            }
            return Err(AttemptBudgetError::DispatchTargetMismatch);
        }
        let authorized_at = self.budget.check_dispatch_deadline()?;
        Ok(AttemptDispatchPermit {
            budget: self.budget,
            target: self.target,
            intent: Some(self.intent),
            reserved_at: self.reserved_at,
            authorized_at,
            predicted_ordinal: self.predicted_usage.total_dispatches,
            predicted_usage: self.predicted_usage,
        })
    }
}

#[must_use = "a dispatch permit must be moved into exactly one physical transport send"]
/// One-shot, target-bound authority issued immediately before physical dispatch.
///
/// This type intentionally does not implement `Clone`. Transport integrations must take it by
/// value so safe Rust cannot authorize two sends with the same permit.
pub struct AttemptDispatchPermit {
    budget: AttemptBudget,
    target: AttemptTarget,
    intent: Option<AttemptRetryIntent>,
    reserved_at: Instant,
    authorized_at: Instant,
    predicted_ordinal: u64,
    predicted_usage: AttemptBudgetUsage,
}

impl fmt::Debug for AttemptDispatchPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttemptDispatchPermit")
            .field("predicted_ordinal", &self.predicted_ordinal)
            .field("reserved_at", &self.reserved_at)
            .field("authorized_at", &self.authorized_at)
            .field("predicted_usage", &self.predicted_usage)
            .finish_non_exhaustive()
    }
}

impl AttemptDispatchPermit {
    /// Consumes this permit at the physical I/O boundary after checking the logical target.
    pub fn consume_for_physical_dispatch(
        self,
        actual_target: &AttemptTarget,
    ) -> Result<AttemptDispatchEvidence, AttemptBudgetError> {
        if self.target != *actual_target {
            if let Ok(mut state) = self.budget.state.lock() {
                state.terminal_cause = Some(AttemptBudgetTerminalCause::DispatchTargetMismatch);
            }
            return Err(AttemptBudgetError::DispatchTargetMismatch);
        }
        let mut state = self
            .budget
            .state
            .lock()
            .map_err(|_| AttemptBudgetError::StateUnavailable)?;
        let dispatched_at = match self.budget.check_dispatch_deadline() {
            Ok(dispatched_at) => dispatched_at,
            Err(error) => {
                state.terminal_cause = Some(AttemptBudgetTerminalCause::DeadlineExceeded);
                return Err(error);
            }
        };
        let intent = self.intent.unwrap_or_else(|| {
            infer_transition_intent(effective_previous_target(&state), actual_target)
        });
        let (ordinal, usage) = self
            .budget
            .charge_dispatch(&mut state, actual_target, intent)?;
        Ok(AttemptDispatchEvidence {
            ordinal,
            target: self.target,
            reserved_at: self.reserved_at,
            authorized_at: self.authorized_at,
            dispatched_at,
            usage,
        })
    }

    pub fn ordinal(&self) -> u64 {
        self.predicted_ordinal
    }

    pub fn target(&self) -> &AttemptTarget {
        &self.target
    }

    pub fn reserved_at(&self) -> Instant {
        self.reserved_at
    }

    pub fn authorized_at(&self) -> Instant {
        self.authorized_at
    }

    pub fn usage(&self) -> AttemptBudgetUsage {
        self.predicted_usage
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct AttemptDispatchEvidence {
    ordinal: u64,
    target: AttemptTarget,
    reserved_at: Instant,
    authorized_at: Instant,
    dispatched_at: Instant,
    usage: AttemptBudgetUsage,
}

impl AttemptDispatchEvidence {
    pub fn ordinal(&self) -> u64 {
        self.ordinal
    }

    pub fn target(&self) -> &AttemptTarget {
        &self.target
    }

    pub fn reserved_at(&self) -> Instant {
        self.reserved_at
    }

    pub fn authorized_at(&self) -> Instant {
        self.authorized_at
    }

    pub fn dispatched_at(&self) -> Instant {
        self.dispatched_at
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

    #[derive(Debug)]
    struct TestClock {
        observed_at: Mutex<Instant>,
    }

    impl TestClock {
        fn new(observed_at: Instant) -> Self {
            Self {
                observed_at: Mutex::new(observed_at),
            }
        }

        fn set(&self, observed_at: Instant) {
            *self
                .observed_at
                .lock()
                .expect("test clock should be available") = observed_at;
        }
    }

    impl AttemptClock for TestClock {
        fn now(&self) -> Instant {
            *self
                .observed_at
                .lock()
                .expect("test clock should be available")
        }
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

    fn dispatch_reservation(
        reservation: AttemptBudgetReservation,
        target: &AttemptTarget,
    ) -> AttemptDispatchEvidence {
        reservation
            .consume_for_dispatch(target)
            .expect("reservation should authorize")
            .consume_for_physical_dispatch(target)
            .expect("physical dispatch should be admitted")
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
        let permit = dispatch_reservation(reservation, &target);

        assert_eq!(permit.ordinal(), 1);
        assert_eq!(permit.usage().total_dispatches(), 1);
        assert_eq!(permit.usage().credential_entries(), 1);
        assert_eq!(permit.usage().provider_switches(), 0);

        let evidence = format!("{permit:?}");
        assert!(!evidence.contains("provider-a"));
        assert!(!evidence.contains("endpoint-a"));
        assert!(!evidence.contains("key-a"));
    }

    #[test]
    fn same_target_replay_only_consumes_total_dispatches() {
        let budget = AttemptBudget::new(limits(3, 1, 0));
        let target = target("provider-a", "endpoint-a", "key-a");
        let reservation = budget
            .try_reserve(&target, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");
        let _permit = dispatch_reservation(reservation, &target);

        let replay = budget
            .try_reserve(&target, AttemptRetryIntent::SameTargetReplay)
            .expect("same target replay should not re-enter credential");
        let replay = dispatch_reservation(replay, &target);

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
        let new_candidate = target("provider-a", "endpoint-c", "key-c");
        let new_provider = target("provider-b", "endpoint-d", "key-d");

        for (target, intent) in [
            (&a, AttemptRetryIntent::Initial),
            (&new_credential, AttemptRetryIntent::Credential),
            (&new_endpoint_same_credential, AttemptRetryIntent::Endpoint),
            (&new_candidate, AttemptRetryIntent::Candidate),
            (&new_provider, AttemptRetryIntent::Provider),
        ] {
            let reservation = budget
                .try_reserve(target, intent)
                .expect("transition should fit");
            dispatch_reservation(reservation, target);
        }

        assert_eq!(
            budget.usage().unwrap(),
            AttemptBudgetUsage {
                total_dispatches: 5,
                credential_entries: 4,
                provider_switches: 1,
            }
        );
    }

    #[test]
    fn returning_to_a_previous_target_consumes_transitions_again() {
        let budget = AttemptBudget::new(limits(3, 3, 2));
        let a = target("provider-a", "endpoint-a", "key-a");
        let b = target("provider-b", "endpoint-b", "key-b");

        let reservation = budget
            .try_reserve(&a, AttemptRetryIntent::Initial)
            .expect("A should fit");
        dispatch_reservation(reservation, &a);
        let reservation = budget
            .try_reserve(&b, AttemptRetryIntent::Provider)
            .expect("A to B should fit");
        dispatch_reservation(reservation, &b);
        let reservation = budget
            .try_reserve(&a, AttemptRetryIntent::Provider)
            .expect("B to A should fit");
        let permit = dispatch_reservation(reservation, &a);

        assert_eq!(permit.usage().credential_entries(), 3);
        assert_eq!(permit.usage().provider_switches(), 2);
    }

    #[test]
    fn failed_multidimension_reservation_has_no_partial_charge() {
        let budget = AttemptBudget::new(limits(4, 1, 4));
        let a = target("provider-a", "endpoint-a", "key-a");
        let b = target("provider-b", "endpoint-b", "key-b");
        let reservation = budget
            .try_reserve(&a, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");
        dispatch_reservation(reservation, &a);

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
        assert_eq!(
            zero_total.terminal_cause(),
            Some(AttemptBudgetTerminalCause::BudgetExhausted(
                AttemptBudgetDimension::TotalDispatches
            ))
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
        let reservation = budget
            .try_reserve(&a, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");
        dispatch_reservation(reservation, &a);

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
    fn stale_caller_time_cannot_bypass_the_internal_clock() {
        let stale_caller_time = Instant::now();
        let deadline = stale_caller_time + Duration::from_secs(10);
        let clock = Arc::new(TestClock::new(stale_caller_time));
        let budget =
            AttemptBudget::with_clock(AttemptBudgetLimits::new(2, 2, 2, deadline), clock.clone());
        let target = target("provider-a", "endpoint-a", "key-a");

        clock.set(deadline);
        let error = budget
            .try_reserve(&target, AttemptRetryIntent::Initial)
            .expect_err("the budget must observe its trusted clock, not stale caller state");

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
        assert!(stale_caller_time < deadline);
        assert_eq!(budget.usage().unwrap().total_dispatches(), 0);
    }

    #[test]
    fn lock_wait_counts_toward_the_absolute_deadline() {
        let before_deadline = Instant::now();
        let deadline = before_deadline + Duration::from_secs(10);
        let clock = Arc::new(TestClock::new(before_deadline));
        let budget =
            AttemptBudget::with_clock(AttemptBudgetLimits::new(1, 1, 0, deadline), clock.clone());
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
        thread::sleep(Duration::from_millis(10));
        clock.set(deadline);
        drop(state_guard);

        assert!(matches!(
            waiter.join().expect("waiter should not panic"),
            Err(AttemptBudgetError::DeadlineExceeded { observed_at, .. })
                if observed_at >= deadline
        ));
        assert_eq!(budget.usage().unwrap().total_dispatches(), 0);
    }

    #[test]
    fn reservation_cannot_be_consumed_after_the_dispatch_deadline() {
        let before_deadline = Instant::now();
        let deadline = before_deadline + Duration::from_secs(10);
        let clock = Arc::new(TestClock::new(before_deadline));
        let budget =
            AttemptBudget::with_clock(AttemptBudgetLimits::new(1, 1, 0, deadline), clock.clone());
        let target = target("provider-a", "endpoint-a", "key-a");
        let reservation = budget
            .try_reserve(&target, AttemptRetryIntent::Initial)
            .expect("reservation before deadline should fit");

        clock.set(deadline);
        assert!(matches!(
            reservation.consume_for_dispatch(&target),
            Err(AttemptBudgetError::DeadlineExceeded {
                deadline: actual_deadline,
                observed_at,
            }) if actual_deadline == deadline && observed_at == deadline
        ));
        assert_eq!(budget.usage().unwrap().total_dispatches(), 0);
        assert_eq!(
            budget.terminal_cause(),
            Some(AttemptBudgetTerminalCause::DeadlineExceeded)
        );
    }

    #[test]
    fn reservation_issues_exactly_one_target_bound_dispatch_permit() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(10);
        let clock = Arc::new(TestClock::new(now));
        let budget = AttemptBudget::with_clock(AttemptBudgetLimits::new(1, 1, 0, deadline), clock);
        let target = target("provider-a", "endpoint-a", "key-a");
        let reservation = budget
            .try_reserve(&target, AttemptRetryIntent::Initial)
            .expect("reservation should fit");

        let permit = reservation
            .consume_for_dispatch(&target)
            .expect("consumption should issue a permit");
        assert_eq!(permit.ordinal(), 1);
        assert_eq!(permit.target(), &target);
        assert_eq!(permit.reserved_at(), now);
        assert_eq!(permit.authorized_at(), now);
        assert_eq!(permit.usage().total_dispatches(), 1);
        let evidence = format!("{permit:?}");
        assert!(!evidence.contains("provider-a"));
        assert!(!evidence.contains("endpoint-a"));
        assert!(!evidence.contains("key-a"));
        permit
            .consume_for_physical_dispatch(&target)
            .expect("permit should charge only at physical dispatch");
    }

    #[test]
    fn reservation_rejects_a_different_physical_dispatch_target() {
        let budget = AttemptBudget::new(limits(1, 1, 0));
        let reserved_target = target("provider-a", "endpoint-a", "key-a");
        let actual_target = target("provider-a", "endpoint-a", "key-b");
        let reservation = budget
            .try_reserve(&reserved_target, AttemptRetryIntent::Initial)
            .expect("reservation should fit");

        assert!(matches!(
            reservation.consume_for_dispatch(&actual_target),
            Err(AttemptBudgetError::DispatchTargetMismatch)
        ));
        assert_eq!(budget.usage().unwrap(), AttemptBudgetUsage::default());
        assert_eq!(
            budget.terminal_cause(),
            Some(AttemptBudgetTerminalCause::DispatchTargetMismatch)
        );
    }

    #[test]
    fn authorization_order_controls_transition_charging() {
        let budget = AttemptBudget::new(limits(3, 3, 2));
        let initial = target("provider-initial", "endpoint-initial", "key-initial");
        let prepared_first = target("provider-first", "endpoint-first", "key-first");
        let dispatched_first = target(
            "provider-dispatched",
            "endpoint-dispatched",
            "key-dispatched",
        );
        budget
            .authorize_next_dispatch(&initial)
            .expect("initial dispatch should fit")
            .consume_for_physical_dispatch(&initial)
            .expect("initial physical dispatch should fit");

        let prepared_first_reservation = budget
            .try_reserve(&prepared_first, AttemptRetryIntent::Provider)
            .expect("first provider transition should prepare");
        let dispatched_first_reservation = budget
            .try_reserve(&dispatched_first, AttemptRetryIntent::Provider)
            .expect("second provider transition should prepare concurrently");

        let dispatched_first_permit = dispatched_first_reservation
            .consume_for_dispatch(&dispatched_first)
            .expect("second preparation may reach physical admission first");
        dispatched_first_permit
            .consume_for_physical_dispatch(&dispatched_first)
            .expect("second preparation should charge first");
        let final_permit = prepared_first_reservation
            .consume_for_dispatch(&prepared_first)
            .expect("later physical admission must charge from the actual previous target");
        let final_evidence = final_permit
            .consume_for_physical_dispatch(&prepared_first)
            .expect("later physical admission must charge from the actual previous target");

        assert_eq!(
            final_evidence.usage(),
            AttemptBudgetUsage {
                total_dispatches: 3,
                credential_entries: 3,
                provider_switches: 2,
            }
        );
    }

    #[test]
    fn dispatch_permit_rejects_a_different_physical_target() {
        let budget = AttemptBudget::new(limits(1, 1, 0));
        let authorized_target = target("provider-a", "endpoint-a", "key-a");
        let actual_target = target("provider-a", "endpoint-b", "key-a");
        let permit = budget
            .authorize_next_dispatch(&authorized_target)
            .expect("dispatch should authorize");

        assert_eq!(
            permit.consume_for_physical_dispatch(&actual_target),
            Err(AttemptBudgetError::DispatchTargetMismatch)
        );
        assert_eq!(
            budget.terminal_cause(),
            Some(AttemptBudgetTerminalCause::DispatchTargetMismatch)
        );
    }

    #[test]
    fn dispatch_permit_rechecks_the_exact_physical_io_deadline() {
        let before_deadline = Instant::now();
        let deadline = before_deadline + Duration::from_secs(10);
        let clock = Arc::new(TestClock::new(before_deadline));
        let budget =
            AttemptBudget::with_clock(AttemptBudgetLimits::new(1, 1, 0, deadline), clock.clone());
        let target = target("provider-a", "endpoint-a", "key-a");
        let permit = budget
            .authorize_next_dispatch(&target)
            .expect("admission before the deadline should fit");

        clock.set(deadline);
        assert!(matches!(
            permit.consume_for_physical_dispatch(&target),
            Err(AttemptBudgetError::DeadlineExceeded {
                deadline: actual_deadline,
                observed_at,
            }) if actual_deadline == deadline && observed_at == deadline
        ));
        assert_eq!(
            budget.terminal_cause(),
            Some(AttemptBudgetTerminalCause::DeadlineExceeded)
        );
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
        let reservation = budget
            .try_reserve(&a, AttemptRetryIntent::Initial)
            .expect("initial dispatch should fit");
        dispatch_reservation(reservation, &a);

        let cases = [
            (
                a.clone(),
                AttemptRetryIntent::Initial,
                AttemptTransitionViolation::AlreadyStarted,
            ),
            (
                a.clone(),
                AttemptRetryIntent::Candidate,
                AttemptTransitionViolation::CandidateEndpointUnchanged,
            ),
            (
                target("provider-a", "endpoint-b", "key-a"),
                AttemptRetryIntent::Candidate,
                AttemptTransitionViolation::CandidateCredentialUnchanged,
            ),
            (
                target("provider-b", "endpoint-b", "key-b"),
                AttemptRetryIntent::Candidate,
                AttemptTransitionViolation::CandidateProviderChanged,
            ),
            (
                a.clone(),
                AttemptRetryIntent::Credential,
                AttemptTransitionViolation::CredentialUnchanged,
            ),
            (
                target("provider-a", "endpoint-b", "key-b"),
                AttemptRetryIntent::Credential,
                AttemptTransitionViolation::CredentialEndpointChanged,
            ),
            (
                target("provider-b", "endpoint-a", "key-b"),
                AttemptRetryIntent::Credential,
                AttemptTransitionViolation::CredentialProviderChanged,
            ),
            (
                target("provider-b", "endpoint-b", "key-b"),
                AttemptRetryIntent::Endpoint,
                AttemptTransitionViolation::EndpointProviderChanged,
            ),
            (
                target("provider-a", "endpoint-a", "key-b"),
                AttemptRetryIntent::Endpoint,
                AttemptTransitionViolation::EndpointCredentialChanged,
            ),
            (
                a.clone(),
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
    fn dropping_a_prepared_reservation_does_not_charge_the_budget() {
        let budget = AttemptBudget::new(limits(1, 1, 0));
        let target = target("provider-a", "endpoint-a", "key-a");
        drop(
            budget
                .try_reserve(&target, AttemptRetryIntent::Initial)
                .expect("initial dispatch should fit"),
        );

        assert_eq!(budget.usage().unwrap(), AttemptBudgetUsage::default());
        budget
            .authorize_next_dispatch(&target)
            .expect("an unconsumed preparation must leave dispatch capacity available");
    }

    #[test]
    fn dropping_an_authorized_permit_does_not_charge_or_advance_physical_order() {
        let budget = AttemptBudget::new(limits(2, 3, 2));
        let dropped = target("provider-a", "endpoint-a", "key-a");
        let dispatched = target("provider-b", "endpoint-b", "key-b");

        drop(
            budget
                .authorize_next_dispatch(&dropped)
                .expect("authorization should prepare a permit"),
        );
        assert_eq!(budget.usage().unwrap(), AttemptBudgetUsage::default());

        let evidence = budget
            .authorize_next_dispatch(&dispatched)
            .expect("dropped permit must leave capacity")
            .consume_for_physical_dispatch(&dispatched)
            .expect("first physical send should be the initial transition");
        assert_eq!(evidence.ordinal(), 1);
        assert_eq!(evidence.usage().credential_entries(), 1);
        assert_eq!(evidence.usage().provider_switches(), 0);
    }

    #[test]
    fn reordered_authorizations_charge_in_physical_send_order() {
        let budget = AttemptBudget::new(limits(3, 3, 2));
        let a = target("provider-a", "endpoint-a", "key-a");
        let b = target("provider-b", "endpoint-b", "key-b");
        let c = target("provider-c", "endpoint-c", "key-c");

        let permit_a = budget.authorize_next_dispatch(&a).unwrap();
        let permit_b = budget.authorize_next_dispatch(&b).unwrap();
        let permit_c = budget.authorize_next_dispatch(&c).unwrap();
        permit_c.consume_for_physical_dispatch(&c).unwrap();
        permit_a.consume_for_physical_dispatch(&a).unwrap();
        let evidence = permit_b.consume_for_physical_dispatch(&b).unwrap();

        assert_eq!(
            evidence.usage(),
            AttemptBudgetUsage {
                total_dispatches: 3,
                credential_entries: 3,
                provider_switches: 2,
            }
        );
    }

    #[test]
    fn delegated_grant_is_bounded_and_reconciliation_consumes_parent_capacity() {
        let budget = AttemptBudget::new(limits(4, 3, 2));
        let initial = target("provider-a", "endpoint-a", "key-a");
        budget
            .authorize_next_dispatch(&initial)
            .unwrap()
            .consume_for_physical_dispatch(&initial)
            .unwrap();

        let delegation = budget.reserve_delegation_grant(&initial).unwrap();
        assert_eq!(delegation.grant().max_total_dispatches, 1);
        assert_eq!(delegation.grant().max_credential_entries, 1);
        assert_eq!(delegation.grant().max_provider_switches, 0);
        let remote = AttemptBudget::from_delegation_grant(delegation.grant()).unwrap();
        assert_eq!(remote.limits().max_total_dispatches(), 1);

        delegation
            .reconcile(aether_contracts::ExecutionAttemptBudgetConsumption {
                total_dispatches: 1,
                credential_entries: 1,
                provider_switches: 0,
            })
            .unwrap();
        assert_eq!(
            budget.usage().unwrap(),
            AttemptBudgetUsage {
                total_dispatches: 2,
                credential_entries: 1,
                provider_switches: 0,
            }
        );
    }

    #[test]
    fn concurrent_remote_grants_cannot_overbook_parent_capacity() {
        let budget = AttemptBudget::new(limits(2, 2, 1));
        let first_target = target("provider-a", "endpoint-a", "key-a");
        let second_target = target("provider-b", "endpoint-b", "key-b");
        let first = budget.reserve_delegation_grant(&first_target).unwrap();
        let second = budget.reserve_delegation_grant(&second_target).unwrap();
        assert!(matches!(
            budget.reserve_delegation_grant(&first_target),
            Err(AttemptBudgetError::BudgetExhausted(_))
        ));
        drop(first);
        let replacement = budget.reserve_delegation_grant(&first_target).unwrap();
        second.reconcile_conservative().unwrap();
        replacement.reconcile_conservative().unwrap();
        assert_eq!(budget.usage().unwrap().total_dispatches(), 2);
    }

    #[test]
    fn outstanding_remote_grant_blocks_competing_local_dispatch() {
        let budget = AttemptBudget::new(limits(1, 1, 0));
        let remote_target = target("provider-a", "endpoint-a", "key-a");
        let grant = budget.reserve_delegation_grant(&remote_target).unwrap();

        let local = budget
            .authorize_next_dispatch(&remote_target)
            .unwrap()
            .consume_for_physical_dispatch(&remote_target);
        assert!(matches!(
            local,
            Err(AttemptBudgetError::BudgetExhausted(
                AttemptBudgetExhausted {
                    dimension: AttemptBudgetDimension::TotalDispatches,
                    ..
                }
            ))
        ));

        grant.reconcile_conservative().unwrap();
        assert_eq!(budget.usage().unwrap().total_dispatches(), 1);
    }

    #[test]
    fn out_of_order_remote_reconcile_preserves_send_order_target() {
        let budget = AttemptBudget::new(limits(3, 3, 2));
        let first_target = target("provider-a", "endpoint-a", "key-a");
        let second_target = target("provider-b", "endpoint-b", "key-b");
        let first = budget.reserve_delegation_grant(&first_target).unwrap();
        let second = budget.reserve_delegation_grant(&second_target).unwrap();

        second.reconcile_conservative().unwrap();
        first.reconcile_conservative().unwrap();
        budget
            .authorize_next_dispatch(&second_target)
            .unwrap()
            .consume_for_physical_dispatch(&second_target)
            .unwrap();

        assert_eq!(
            budget.usage().unwrap(),
            AttemptBudgetUsage {
                total_dispatches: 3,
                credential_entries: 2,
                provider_switches: 1,
            }
        );
    }

    #[test]
    fn one_conservative_remote_reconcile_consumes_only_one_dispatch() {
        let budget = AttemptBudget::new(limits(4, 4, 3));
        let remote_target = target("provider-a", "endpoint-a", "key-a");
        budget
            .reserve_delegation_grant(&remote_target)
            .unwrap()
            .reconcile_conservative()
            .unwrap();
        assert_eq!(budget.usage().unwrap().total_dispatches(), 1);
        assert!(budget.reserve_delegation_grant(&remote_target).is_ok());
    }

    #[test]
    fn local_and_remote_topologies_count_one_provider_send_identically() {
        let local = AttemptBudget::new(limits(4, 4, 3));
        let remote = AttemptBudget::new(limits(4, 4, 3));
        let provider_target = target("provider-a", "endpoint-a", "key-a");

        local
            .authorize_next_dispatch(&provider_target)
            .unwrap()
            .consume_for_physical_dispatch(&provider_target)
            .unwrap();
        remote
            .reserve_delegation_grant(&provider_target)
            .unwrap()
            .reconcile_conservative()
            .unwrap();

        assert_eq!(local.usage().unwrap(), remote.usage().unwrap());
        assert_eq!(local.usage().unwrap().total_dispatches(), 1);
        assert_eq!(local.usage().unwrap().credential_entries(), 1);
    }

    #[test]
    fn invalid_or_expired_delegated_grant_fails_closed() {
        let now_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let missing_id = aether_contracts::ExecutionAttemptBudgetGrant {
            grant_id: String::new(),
            max_total_dispatches: 1,
            max_credential_entries: 1,
            max_provider_switches: 0,
            deadline_unix_ms: now_unix_ms + 1_000,
        };
        assert_eq!(
            AttemptBudget::from_delegation_grant(&missing_id).unwrap_err(),
            AttemptBudgetError::InvalidGrant
        );

        let expired = aether_contracts::ExecutionAttemptBudgetGrant {
            grant_id: "expired".to_string(),
            max_total_dispatches: 1,
            max_credential_entries: 1,
            max_provider_switches: 0,
            deadline_unix_ms: now_unix_ms,
        };
        assert!(matches!(
            AttemptBudget::from_delegation_grant(&expired),
            Err(AttemptBudgetError::DeadlineExceeded { .. })
        ));

        let unbounded = aether_contracts::ExecutionAttemptBudgetGrant {
            grant_id: "unbounded".to_string(),
            max_total_dispatches: MAX_REQUEST_ATTEMPT_TOTAL_DISPATCHES + 1,
            max_credential_entries: 1,
            max_provider_switches: 0,
            deadline_unix_ms: now_unix_ms + 1_000,
        };
        assert_eq!(
            AttemptBudget::from_delegation_grant(&unbounded).unwrap_err(),
            AttemptBudgetError::InvalidGrant
        );
    }

    #[test]
    fn cloned_handles_share_one_atomic_concurrent_limit() {
        const THREADS: usize = 16;
        const LIMIT: u64 = 5;

        let budget = AttemptBudget::new(limits(LIMIT, 1, 0));
        let target = target("provider-a", "endpoint-a", "key-a");
        let barrier = Arc::new(Barrier::new(THREADS));

        let handles = (0..THREADS)
            .map(|_| {
                let budget = budget.clone();
                let target = target.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    budget
                        .authorize_next_dispatch(&target)?
                        .consume_for_physical_dispatch(&target)
                })
            })
            .collect::<Vec<_>>();

        let successes = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread should not panic"))
            .filter(Result::is_ok)
            .count();
        assert_eq!(successes, LIMIT as usize);
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

            for target in &sequence {
                budget
                    .authorize_next_dispatch(target)
                    .expect("unbounded small sequence should fit")
                    .consume_for_physical_dispatch(target)
                    .expect("unbounded small physical sequence should fit");
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
