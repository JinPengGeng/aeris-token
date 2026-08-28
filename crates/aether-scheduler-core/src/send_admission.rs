use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendAdmissionBinding {
    provider_id: String,
    endpoint_id: String,
    key_id: String,
    api_format: String,
}

impl SendAdmissionBinding {
    pub fn new(
        provider_id: impl Into<String>,
        endpoint_id: impl Into<String>,
        key_id: impl Into<String>,
        api_format: impl Into<String>,
    ) -> Result<Self, SendAdmissionEvidenceError> {
        let binding = Self {
            provider_id: provider_id.into(),
            endpoint_id: endpoint_id.into(),
            key_id: key_id.into(),
            api_format: api_format.into(),
        };
        if binding.provider_id.trim().is_empty()
            || binding.endpoint_id.trim().is_empty()
            || binding.key_id.trim().is_empty()
            || binding.api_format.trim().is_empty()
        {
            return Err(SendAdmissionEvidenceError::IncompleteBinding);
        }
        Ok(binding)
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

    pub fn api_format(&self) -> &str {
        &self.api_format
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendAdmissionIdentity {
    request_id: String,
    candidate_id: String,
    candidate_generation: String,
    attempt_ordinal: u64,
    binding: SendAdmissionBinding,
}

impl SendAdmissionIdentity {
    pub fn new(
        request_id: impl Into<String>,
        candidate_id: impl Into<String>,
        candidate_generation: impl Into<String>,
        attempt_ordinal: u64,
        binding: SendAdmissionBinding,
    ) -> Result<Self, SendAdmissionEvidenceError> {
        let identity = Self {
            request_id: request_id.into(),
            candidate_id: candidate_id.into(),
            candidate_generation: candidate_generation.into(),
            attempt_ordinal,
            binding,
        };
        if identity.request_id.trim().is_empty()
            || identity.candidate_id.trim().is_empty()
            || identity.candidate_generation.trim().is_empty()
        {
            return Err(SendAdmissionEvidenceError::IncompleteIdentity);
        }
        if identity.attempt_ordinal == 0 {
            return Err(SendAdmissionEvidenceError::AttemptOrdinalMissing);
        }
        Ok(identity)
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn candidate_id(&self) -> &str {
        &self.candidate_id
    }

    pub fn candidate_generation(&self) -> &str {
        &self.candidate_generation
    }

    pub const fn attempt_ordinal(&self) -> u64 {
        self.attempt_ordinal
    }

    pub fn binding(&self) -> &SendAdmissionBinding {
        &self.binding
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SendAuthorityRevision(u64);

impl SendAuthorityRevision {
    pub const fn new(revision: u64) -> Self {
        Self(revision)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SendAdmissionProof {
    identity: SendAdmissionIdentity,
    authority_revision: SendAuthorityRevision,
    reservation_id: String,
    fencing_token: u64,
    admitted_at_unix_ms: u64,
    valid_until_unix_ms: u64,
    reservation_expires_at_unix_ms: u64,
    request_deadline_unix_ms: u64,
}

impl SendAdmissionProof {
    fn reservation_id(&self) -> &str {
        &self.reservation_id
    }

    const fn fencing_token(&self) -> u64 {
        self.fencing_token
    }

    const fn valid_until_unix_ms(&self) -> u64 {
        self.valid_until_unix_ms
    }

    const fn reservation_expires_at_unix_ms(&self) -> u64 {
        self.reservation_expires_at_unix_ms
    }

    const fn request_deadline_unix_ms(&self) -> u64 {
        self.request_deadline_unix_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAdmissionEvidenceError {
    IncompleteBinding,
    IncompleteIdentity,
    AttemptOrdinalMissing,
    ReservationIdMissing,
    FencingTokenMissing,
    InvalidValidityWindow,
    ReservationExpiresBeforeAdmission,
    BudgetIdentityMismatch,
    BudgetAttemptMismatch,
    BudgetDeadlineMismatch,
    ReservationIdentityMismatch,
    ReservationRevisionMismatch,
    AdmissionExceedsRequestDeadline,
    AuthorityUnavailable,
    AuthorityChanged,
    ReservationInvalid,
    AdmissionExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAdmissionDispatchError {
    AuthorityUnavailable,
    AuthorityChanged,
    ReservationInvalid,
    AdmissionExpired,
}

trait AuthoritativeConsumption: fmt::Debug + Send {}

trait FinalAuthorityGuard: fmt::Debug + Send + Sync {
    #[cfg(test)]
    fn validate_for_admission(
        &self,
        proof: &SendAdmissionProof,
    ) -> Result<(), SendAdmissionDispatchError>;

    fn consume_for_dispatch(
        &self,
        proof: &SendAdmissionProof,
    ) -> Result<Box<dyn AuthoritativeConsumption>, SendAdmissionDispatchError>;
}

/// One-shot authority for exactly one call through the physical dispatch boundary.
///
/// There is intentionally no public constructor. Until the authoritative repository/reservation
/// adapter and the request-wide budget live in the same dependency direction, production issuance
/// remains unavailable and fails closed through [`request_send_admission`].
#[must_use = "an admitted send must be moved into exactly one physical dispatch"]
pub struct AdmittedSend {
    proof: SendAdmissionProof,
    final_guard: Arc<dyn FinalAuthorityGuard>,
}

impl fmt::Debug for AdmittedSend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdmittedSend")
            .field("identity", &self.proof.identity)
            .field("authority_revision", &self.proof.authority_revision)
            .field("reservation_id", &self.proof.reservation_id)
            .field("fencing_token", &self.proof.fencing_token)
            .field("admitted_at_unix_ms", &self.proof.admitted_at_unix_ms)
            .field("valid_until_unix_ms", &self.proof.valid_until_unix_ms)
            .field(
                "reservation_expires_at_unix_ms",
                &self.proof.reservation_expires_at_unix_ms,
            )
            .field(
                "request_deadline_unix_ms",
                &self.proof.request_deadline_unix_ms,
            )
            .finish_non_exhaustive()
    }
}

impl AdmittedSend {
    /// Consumes the only dispatch capability, revalidates authority with its trusted clock, and
    /// invokes the physical transport operation exactly once. The authority guard must atomically
    /// consume the reservation before returning; the resulting RAII marker is held inside
    /// [`AuthorizedPhysicalSend`] through transport entry, so concurrent revocation observes the
    /// send as already committed instead of racing a completed validation.
    ///
    /// ```compile_fail
    /// use aether_scheduler_core::AdmittedSend;
    ///
    /// fn send_twice(admitted: AdmittedSend) {
    ///     admitted.dispatch(|_| ());
    ///     admitted.dispatch(|_| ());
    /// }
    /// ```
    pub fn dispatch<R>(
        self,
        transport: impl FnOnce(AuthorizedPhysicalSend) -> R,
    ) -> Result<R, SendAdmissionDispatchError> {
        let consumption = self.final_guard.consume_for_dispatch(&self.proof)?;
        Ok(transport(AuthorizedPhysicalSend {
            proof: self.proof,
            _consumption: consumption,
        }))
    }
}

/// Target-bound input presented to the one physical transport invocation.
///
/// This type cannot be constructed or cloned by callers. A transport boundary should accept it by
/// value and derive the upstream destination only from the bound identity.
#[must_use = "the authorized physical send must be consumed by the transport operation"]
pub struct AuthorizedPhysicalSend {
    proof: SendAdmissionProof,
    _consumption: Box<dyn AuthoritativeConsumption>,
}

impl fmt::Debug for AuthorizedPhysicalSend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizedPhysicalSend")
            .field("identity", &self.proof.identity)
            .field("authority_revision", &self.proof.authority_revision)
            .field("reservation_id", &self.proof.reservation_id)
            .field("fencing_token", &self.proof.fencing_token)
            .finish_non_exhaustive()
    }
}

impl AuthorizedPhysicalSend {
    pub fn identity(&self) -> &SendAdmissionIdentity {
        &self.proof.identity
    }

    pub const fn authority_revision(&self) -> SendAuthorityRevision {
        self.proof.authority_revision
    }

    pub fn reservation_id(&self) -> &str {
        self.proof.reservation_id()
    }

    pub const fn fencing_token(&self) -> u64 {
        self.proof.fencing_token()
    }

    pub const fn valid_until_unix_ms(&self) -> u64 {
        self.proof.valid_until_unix_ms()
    }

    pub const fn reservation_expires_at_unix_ms(&self) -> u64 {
        self.proof.reservation_expires_at_unix_ms()
    }

    pub const fn request_deadline_unix_ms(&self) -> u64 {
        self.proof.request_deadline_unix_ms()
    }
}

#[cfg(test)]
#[derive(Debug)]
struct VerifiedBudgetReservation {
    identity: SendAdmissionIdentity,
    attempt_ordinal: u64,
    request_deadline_unix_ms: u64,
}

#[cfg(test)]
#[derive(Debug)]
struct VerifiedAuthorityLease {
    proof: SendAdmissionProof,
    reservation_identity: SendAdmissionIdentity,
    reservation_authority_revision: SendAuthorityRevision,
    final_guard: Arc<dyn FinalAuthorityGuard>,
}

#[cfg(test)]
fn admit_from_verified_parts(
    budget: VerifiedBudgetReservation,
    authority: VerifiedAuthorityLease,
) -> Result<AdmittedSend, SendAdmissionEvidenceError> {
    if budget.identity != authority.proof.identity {
        return Err(SendAdmissionEvidenceError::BudgetIdentityMismatch);
    }
    if budget.attempt_ordinal != authority.proof.identity.attempt_ordinal {
        return Err(SendAdmissionEvidenceError::BudgetAttemptMismatch);
    }
    if budget.request_deadline_unix_ms != authority.proof.request_deadline_unix_ms {
        return Err(SendAdmissionEvidenceError::BudgetDeadlineMismatch);
    }
    if authority.reservation_identity != authority.proof.identity {
        return Err(SendAdmissionEvidenceError::ReservationIdentityMismatch);
    }
    if authority.reservation_authority_revision != authority.proof.authority_revision {
        return Err(SendAdmissionEvidenceError::ReservationRevisionMismatch);
    }
    if authority.proof.reservation_id.trim().is_empty() {
        return Err(SendAdmissionEvidenceError::ReservationIdMissing);
    }
    if authority.proof.fencing_token == 0 {
        return Err(SendAdmissionEvidenceError::FencingTokenMissing);
    }
    if authority.proof.valid_until_unix_ms <= authority.proof.admitted_at_unix_ms {
        return Err(SendAdmissionEvidenceError::InvalidValidityWindow);
    }
    if authority.proof.reservation_expires_at_unix_ms < authority.proof.valid_until_unix_ms {
        return Err(SendAdmissionEvidenceError::ReservationExpiresBeforeAdmission);
    }
    if authority.proof.request_deadline_unix_ms < authority.proof.valid_until_unix_ms {
        return Err(SendAdmissionEvidenceError::AdmissionExceedsRequestDeadline);
    }
    authority
        .final_guard
        .validate_for_admission(&authority.proof)
        .map_err(|error| match error {
            SendAdmissionDispatchError::AuthorityUnavailable => {
                SendAdmissionEvidenceError::AuthorityUnavailable
            }
            SendAdmissionDispatchError::AuthorityChanged => {
                SendAdmissionEvidenceError::AuthorityChanged
            }
            SendAdmissionDispatchError::ReservationInvalid => {
                SendAdmissionEvidenceError::ReservationInvalid
            }
            SendAdmissionDispatchError::AdmissionExpired => {
                SendAdmissionEvidenceError::AdmissionExpired
            }
        })?;

    Ok(AdmittedSend {
        proof: authority.proof,
        final_guard: authority.final_guard,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAdmissionRetryScope {
    Candidate,
    Credential,
    Endpoint,
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAdmissionSkipReason {
    ProviderInactive,
    ProviderQuotaExhausted,
    ProviderConcurrencyExhausted,
    EndpointInactive,
    EndpointUnhealthy,
    CredentialInactive,
    CredentialExpired,
    CredentialCircuitOpen,
    CredentialUnhealthy,
    CredentialRpmExhausted,
    CredentialConcurrencyExhausted,
    AccountQuotaExhausted,
    OAuthInvalid,
    ModelUnavailable,
    BindingMissing,
}

impl SendAdmissionSkipReason {
    pub const fn retry_scope(self) -> SendAdmissionRetryScope {
        match self {
            Self::ProviderInactive
            | Self::ProviderQuotaExhausted
            | Self::ProviderConcurrencyExhausted => SendAdmissionRetryScope::Provider,
            Self::EndpointInactive | Self::EndpointUnhealthy => SendAdmissionRetryScope::Endpoint,
            Self::CredentialInactive
            | Self::CredentialExpired
            | Self::CredentialCircuitOpen
            | Self::CredentialUnhealthy
            | Self::CredentialRpmExhausted
            | Self::CredentialConcurrencyExhausted
            | Self::AccountQuotaExhausted
            | Self::OAuthInvalid => SendAdmissionRetryScope::Credential,
            Self::ModelUnavailable | Self::BindingMissing => SendAdmissionRetryScope::Candidate,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderInactive => "provider_inactive",
            Self::ProviderQuotaExhausted => "provider_quota_exhausted",
            Self::ProviderConcurrencyExhausted => "provider_concurrency_exhausted",
            Self::EndpointInactive => "endpoint_inactive",
            Self::EndpointUnhealthy => "endpoint_unhealthy",
            Self::CredentialInactive => "credential_inactive",
            Self::CredentialExpired => "credential_expired",
            Self::CredentialCircuitOpen => "credential_circuit_open",
            Self::CredentialUnhealthy => "credential_unhealthy",
            Self::CredentialRpmExhausted => "credential_rpm_exhausted",
            Self::CredentialConcurrencyExhausted => "credential_concurrency_exhausted",
            Self::AccountQuotaExhausted => "account_quota_exhausted",
            Self::OAuthInvalid => "oauth_invalid",
            Self::ModelUnavailable => "model_unavailable",
            Self::BindingMissing => "binding_missing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendAdmissionSkip {
    reason: SendAdmissionSkipReason,
}

impl SendAdmissionSkip {
    pub const fn new(reason: SendAdmissionSkipReason) -> Self {
        Self { reason }
    }

    pub const fn reason(self) -> SendAdmissionSkipReason {
        self.reason
    }

    pub const fn retry_scope(self) -> SendAdmissionRetryScope {
        self.reason.retry_scope()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAdmissionStopReason {
    AuthorityUnavailable,
    AuthorityReadFailed,
    AuthorityInconsistent,
    ReservationBackendUnavailable,
    BudgetExhausted,
    DeadlineExceeded,
    LeaseLost,
    Cancelled,
    UnsupportedDistributedMemoryState,
}

impl SendAdmissionStopReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthorityUnavailable => "authority_unavailable",
            Self::AuthorityReadFailed => "authority_read_failed",
            Self::AuthorityInconsistent => "authority_inconsistent",
            Self::ReservationBackendUnavailable => "reservation_backend_unavailable",
            Self::BudgetExhausted => "budget_exhausted",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::LeaseLost => "lease_lost",
            Self::Cancelled => "cancelled",
            Self::UnsupportedDistributedMemoryState => "unsupported_distributed_memory_state",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendAdmissionStop {
    reason: SendAdmissionStopReason,
}

impl SendAdmissionStop {
    pub const fn new(reason: SendAdmissionStopReason) -> Self {
        Self { reason }
    }

    pub const fn reason(self) -> SendAdmissionStopReason {
        self.reason
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAdmissionBudgetEffect {
    AdmissionDisposition,
    /// The Admit disposition requires the request budget to reserve a dispatch.
    /// It does not assert that a physical dispatch already happened.
    DispatchReservation,
}

#[derive(Debug)]
pub enum SendAdmissionDecision {
    Admit(Box<AdmittedSend>),
    Skip(SendAdmissionSkip),
    Stop(SendAdmissionStop),
}

/// Requests production send-time admission.
///
/// This contract intentionally stays fail closed until the authoritative repository/reservation
/// adapter can consume the request-wide budget permit by value. Exposing a data-only factory here
/// would let ordinary callers forge admission or reorder budget consumption after admission.
pub fn request_send_admission(_identity: SendAdmissionIdentity) -> SendAdmissionDecision {
    SendAdmissionDecision::Stop(SendAdmissionStop::new(
        SendAdmissionStopReason::AuthorityUnavailable,
    ))
}

impl SendAdmissionDecision {
    pub const fn budget_effect(&self) -> SendAdmissionBudgetEffect {
        match self {
            Self::Admit(_) => SendAdmissionBudgetEffect::DispatchReservation,
            Self::Skip(_) | Self::Stop(_) => SendAdmissionBudgetEffect::AdmissionDisposition,
        }
    }

    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Stop(_))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::{Barrier, Mutex};
    use std::thread;

    use super::*;

    #[derive(Debug, Clone)]
    struct TestAuthoritySnapshot {
        now_unix_ms: u64,
        identity: SendAdmissionIdentity,
        authority_revision: SendAuthorityRevision,
        reservation_id: String,
        fencing_token: u64,
        reservation_active: bool,
        consumed: bool,
    }

    #[derive(Debug)]
    struct TestAuthorityGuard {
        snapshot: Mutex<TestAuthoritySnapshot>,
    }

    impl TestAuthorityGuard {
        fn update(&self, update: impl FnOnce(&mut TestAuthoritySnapshot)) {
            update(&mut self.snapshot.lock().expect("test authority lock"));
        }
    }

    #[derive(Debug)]
    struct TestAuthoritativeConsumption;

    impl AuthoritativeConsumption for TestAuthoritativeConsumption {}

    fn validate_snapshot(
        snapshot: &TestAuthoritySnapshot,
        proof: &SendAdmissionProof,
    ) -> Result<(), SendAdmissionDispatchError> {
        if snapshot.identity != proof.identity
            || snapshot.authority_revision != proof.authority_revision
        {
            return Err(SendAdmissionDispatchError::AuthorityChanged);
        }
        if !snapshot.reservation_active
            || snapshot.reservation_id != proof.reservation_id
            || snapshot.fencing_token != proof.fencing_token
            || snapshot.consumed
        {
            return Err(SendAdmissionDispatchError::ReservationInvalid);
        }
        if snapshot.now_unix_ms < proof.admitted_at_unix_ms
            || snapshot.now_unix_ms >= proof.valid_until_unix_ms
            || snapshot.now_unix_ms >= proof.reservation_expires_at_unix_ms
            || snapshot.now_unix_ms >= proof.request_deadline_unix_ms
        {
            return Err(SendAdmissionDispatchError::AdmissionExpired);
        }
        Ok(())
    }

    impl FinalAuthorityGuard for TestAuthorityGuard {
        #[cfg(test)]
        fn validate_for_admission(
            &self,
            proof: &SendAdmissionProof,
        ) -> Result<(), SendAdmissionDispatchError> {
            let snapshot = self.snapshot.lock().expect("test authority lock");
            validate_snapshot(&snapshot, proof)
        }

        fn consume_for_dispatch(
            &self,
            proof: &SendAdmissionProof,
        ) -> Result<Box<dyn AuthoritativeConsumption>, SendAdmissionDispatchError> {
            let mut snapshot = self.snapshot.lock().expect("test authority lock");
            validate_snapshot(&snapshot, proof)?;
            snapshot.consumed = true;
            Ok(Box::new(TestAuthoritativeConsumption))
        }
    }

    #[derive(Debug)]
    struct BlockingAuthorityGuard {
        snapshot: Mutex<TestAuthoritySnapshot>,
        consume_entered: Arc<Barrier>,
        allow_consume: Arc<Barrier>,
    }

    impl BlockingAuthorityGuard {
        fn try_revoke(&self) -> bool {
            let mut snapshot = self.snapshot.lock().expect("blocking authority lock");
            if snapshot.consumed {
                return false;
            }
            snapshot.reservation_active = false;
            true
        }
    }

    impl FinalAuthorityGuard for BlockingAuthorityGuard {
        #[cfg(test)]
        fn validate_for_admission(
            &self,
            proof: &SendAdmissionProof,
        ) -> Result<(), SendAdmissionDispatchError> {
            validate_snapshot(
                &self.snapshot.lock().expect("blocking authority lock"),
                proof,
            )
        }

        fn consume_for_dispatch(
            &self,
            proof: &SendAdmissionProof,
        ) -> Result<Box<dyn AuthoritativeConsumption>, SendAdmissionDispatchError> {
            let mut snapshot = self.snapshot.lock().expect("blocking authority lock");
            validate_snapshot(&snapshot, proof)?;
            self.consume_entered.wait();
            self.allow_consume.wait();
            snapshot.consumed = true;
            Ok(Box::new(TestAuthoritativeConsumption))
        }
    }

    fn identity(
        request_id: &str,
        candidate_id: &str,
        generation: &str,
        attempt_ordinal: u64,
    ) -> SendAdmissionIdentity {
        SendAdmissionIdentity::new(
            request_id,
            candidate_id,
            generation,
            attempt_ordinal,
            SendAdmissionBinding::new("provider", "endpoint", "key", "openai:responses")
                .expect("binding"),
        )
        .expect("identity")
    }

    fn fixture(
        identity: SendAdmissionIdentity,
    ) -> (
        VerifiedBudgetReservation,
        VerifiedAuthorityLease,
        Arc<TestAuthorityGuard>,
    ) {
        let revision = SendAuthorityRevision::new(17);
        let proof = SendAdmissionProof {
            identity: identity.clone(),
            authority_revision: revision,
            reservation_id: "reservation".to_string(),
            fencing_token: 42,
            admitted_at_unix_ms: 1_000,
            valid_until_unix_ms: 1_400,
            reservation_expires_at_unix_ms: 1_500,
            request_deadline_unix_ms: 1_450,
        };
        let guard = Arc::new(TestAuthorityGuard {
            snapshot: Mutex::new(TestAuthoritySnapshot {
                now_unix_ms: 1_000,
                identity: identity.clone(),
                authority_revision: revision,
                reservation_id: proof.reservation_id.clone(),
                fencing_token: proof.fencing_token,
                reservation_active: true,
                consumed: false,
            }),
        });
        (
            VerifiedBudgetReservation {
                attempt_ordinal: identity.attempt_ordinal,
                request_deadline_unix_ms: proof.request_deadline_unix_ms,
                identity: identity.clone(),
            },
            VerifiedAuthorityLease {
                reservation_identity: identity,
                reservation_authority_revision: revision,
                proof,
                final_guard: guard.clone(),
            },
            guard,
        )
    }

    fn admitted_send() -> (AdmittedSend, Arc<TestAuthorityGuard>) {
        let (budget, authority, guard) = fixture(identity("request", "candidate", "generation", 1));
        (
            admit_from_verified_parts(budget, authority).expect("verified admission"),
            guard,
        )
    }

    #[test]
    fn public_identity_validation_fails_closed() {
        assert_eq!(
            SendAdmissionBinding::new("", "endpoint", "key", "format"),
            Err(SendAdmissionEvidenceError::IncompleteBinding)
        );
        assert_eq!(
            SendAdmissionIdentity::new(
                "",
                "candidate",
                "generation",
                1,
                SendAdmissionBinding::new("provider", "endpoint", "key", "format")
                    .expect("binding"),
            ),
            Err(SendAdmissionEvidenceError::IncompleteIdentity)
        );
        assert_eq!(
            SendAdmissionIdentity::new(
                "request",
                "candidate",
                "generation",
                0,
                SendAdmissionBinding::new("provider", "endpoint", "key", "format")
                    .expect("binding"),
            ),
            Err(SendAdmissionEvidenceError::AttemptOrdinalMissing)
        );
    }

    #[test]
    fn production_admission_is_unavailable_until_authority_and_budget_are_integrated() {
        let decision = request_send_admission(identity("request", "candidate", "generation", 1));
        assert!(matches!(
            decision,
            SendAdmissionDecision::Stop(stop)
                if stop.reason() == SendAdmissionStopReason::AuthorityUnavailable
        ));
    }

    #[test]
    fn verified_capability_binds_every_identity_and_reservation_dimension() {
        let (admitted, _) = admitted_send();
        let calls = Cell::new(0);
        let result = admitted
            .dispatch(|authorized| {
                calls.set(calls.get() + 1);
                assert_eq!(authorized.identity().request_id(), "request");
                assert_eq!(authorized.identity().candidate_id(), "candidate");
                assert_eq!(authorized.identity().candidate_generation(), "generation");
                assert_eq!(authorized.identity().attempt_ordinal(), 1);
                assert_eq!(authorized.identity().binding().provider_id(), "provider");
                assert_eq!(authorized.identity().binding().endpoint_id(), "endpoint");
                assert_eq!(authorized.identity().binding().key_id(), "key");
                assert_eq!(
                    authorized.identity().binding().api_format(),
                    "openai:responses"
                );
                assert_eq!(authorized.authority_revision().get(), 17);
                assert_eq!(authorized.reservation_id(), "reservation");
                assert_eq!(authorized.fencing_token(), 42);
                assert_eq!(authorized.valid_until_unix_ms(), 1_400);
                assert_eq!(authorized.reservation_expires_at_unix_ms(), 1_500);
                assert_eq!(authorized.request_deadline_unix_ms(), 1_450);
                "sent"
            })
            .expect("authority remains valid");
        assert_eq!(result, "sent");
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn cross_request_candidate_generation_and_attempt_reuse_are_rejected() {
        let original = identity("request", "candidate", "generation", 1);
        let substitutions = [
            identity("other-request", "candidate", "generation", 1),
            identity("request", "other-candidate", "generation", 1),
            identity("request", "candidate", "other-generation", 1),
            identity("request", "candidate", "generation", 2),
        ];

        for substituted in substitutions {
            let (_, authority, _) = fixture(original.clone());
            let error = admit_from_verified_parts(
                VerifiedBudgetReservation {
                    attempt_ordinal: substituted.attempt_ordinal,
                    request_deadline_unix_ms: authority.proof.request_deadline_unix_ms,
                    identity: substituted,
                },
                authority,
            )
            .expect_err("budget identity from another request/generation/attempt must not splice");
            assert_eq!(error, SendAdmissionEvidenceError::BudgetIdentityMismatch);
        }
    }

    #[test]
    fn full_target_and_attempt_ordinal_must_match_the_budget_reservation() {
        let original = identity("request", "candidate", "generation", 1);
        let (_, authority, _) = fixture(original.clone());
        let different_target = SendAdmissionIdentity::new(
            "request",
            "candidate",
            "generation",
            1,
            SendAdmissionBinding::new("provider", "endpoint", "other-key", "openai:responses")
                .expect("binding"),
        )
        .expect("identity");
        assert_eq!(
            admit_from_verified_parts(
                VerifiedBudgetReservation {
                    identity: different_target,
                    attempt_ordinal: 1,
                    request_deadline_unix_ms: authority.proof.request_deadline_unix_ms,
                },
                authority,
            )
            .expect_err("a different physical target must fail closed"),
            SendAdmissionEvidenceError::BudgetIdentityMismatch
        );

        let (mut budget, authority, _) = fixture(original);
        budget.attempt_ordinal = 2;
        assert_eq!(
            admit_from_verified_parts(budget, authority)
                .expect_err("attempt ordinal must match the target-bound budget permit"),
            SendAdmissionEvidenceError::BudgetAttemptMismatch
        );
    }

    #[test]
    fn reservation_proof_cannot_be_spliced_across_identity_or_revision() {
        let original = identity("request", "candidate", "generation", 1);
        let (budget, mut authority, _) = fixture(original.clone());
        authority.reservation_identity = identity("other-request", "candidate", "generation", 1);
        assert_eq!(
            admit_from_verified_parts(budget, authority)
                .expect_err("reservation from another request must not splice"),
            SendAdmissionEvidenceError::ReservationIdentityMismatch
        );

        let (budget, mut authority, _) = fixture(original);
        authority.reservation_authority_revision = SendAuthorityRevision::new(18);
        assert_eq!(
            admit_from_verified_parts(budget, authority)
                .expect_err("reservation from another authority revision must not splice"),
            SendAdmissionEvidenceError::ReservationRevisionMismatch
        );
    }

    #[test]
    fn request_budget_deadline_is_bound_and_caps_the_admission_lease() {
        let original = identity("request", "candidate", "generation", 1);
        let (mut budget, authority, _) = fixture(original.clone());
        budget.request_deadline_unix_ms += 1;
        assert_eq!(
            admit_from_verified_parts(budget, authority)
                .expect_err("a proof for another request deadline must not splice"),
            SendAdmissionEvidenceError::BudgetDeadlineMismatch
        );

        let (budget, mut authority, _) = fixture(original.clone());
        authority.proof.valid_until_unix_ms = authority.proof.reservation_expires_at_unix_ms + 1;
        assert_eq!(
            admit_from_verified_parts(budget, authority)
                .expect_err("reservation must cover the complete admission lease"),
            SendAdmissionEvidenceError::ReservationExpiresBeforeAdmission
        );

        let (budget, mut authority, _) = fixture(original.clone());
        authority.proof.valid_until_unix_ms = authority.proof.request_deadline_unix_ms + 1;
        assert_eq!(
            admit_from_verified_parts(budget, authority)
                .expect_err("admission lease must not extend beyond the request deadline"),
            SendAdmissionEvidenceError::AdmissionExceedsRequestDeadline
        );

        let (mut budget, mut authority, _) = fixture(original);
        authority.proof.request_deadline_unix_ms = authority.proof.valid_until_unix_ms;
        budget.request_deadline_unix_ms = authority.proof.request_deadline_unix_ms;
        let _admitted = admit_from_verified_parts(budget, authority)
            .expect("an admission lease ending exactly at the request deadline is valid");
    }

    #[test]
    fn expired_or_invalid_reservation_fails_before_capability_issuance() {
        let original = identity("request", "candidate", "generation", 1);
        let (budget, authority, guard) = fixture(original.clone());
        guard.update(|state| state.now_unix_ms = 1_500);
        assert_eq!(
            admit_from_verified_parts(budget, authority)
                .expect_err("trusted authority clock must reject an expired lease"),
            SendAdmissionEvidenceError::AdmissionExpired
        );

        let (budget, authority, guard) = fixture(original);
        guard.update(|state| state.reservation_active = false);
        assert_eq!(
            admit_from_verified_parts(budget, authority)
                .expect_err("inactive reservation must fail closed"),
            SendAdmissionEvidenceError::ReservationInvalid
        );
    }

    #[test]
    fn final_consumption_rechecks_revision_generation_fencing_and_expiry() {
        type Mutation = fn(&mut TestAuthoritySnapshot);
        let mutations: [(Mutation, SendAdmissionDispatchError); 4] = [
            (
                |state| state.authority_revision = SendAuthorityRevision::new(18),
                SendAdmissionDispatchError::AuthorityChanged,
            ),
            (
                |state| state.identity.candidate_generation = "next-generation".to_string(),
                SendAdmissionDispatchError::AuthorityChanged,
            ),
            (
                |state| state.fencing_token += 1,
                SendAdmissionDispatchError::ReservationInvalid,
            ),
            (
                |state| state.now_unix_ms = 1_500,
                SendAdmissionDispatchError::AdmissionExpired,
            ),
        ];

        for (mutation, expected) in mutations {
            let (admitted, guard) = admitted_send();
            guard.update(mutation);
            let called = Cell::new(false);
            let error = admitted
                .dispatch(|_| called.set(true))
                .expect_err("TOCTOU change must prevent physical transport invocation");
            assert_eq!(error, expected);
            assert!(!called.get());
        }
    }

    #[test]
    fn concurrent_revocation_cannot_enter_between_authority_consumption_and_transport_entry() {
        let original = identity("request", "candidate", "generation", 1);
        let (budget, mut authority, source_guard) = fixture(original);
        let snapshot = source_guard
            .snapshot
            .lock()
            .expect("source authority lock")
            .clone();
        let consume_entered = Arc::new(Barrier::new(2));
        let allow_consume = Arc::new(Barrier::new(2));
        let guard = Arc::new(BlockingAuthorityGuard {
            snapshot: Mutex::new(snapshot),
            consume_entered: consume_entered.clone(),
            allow_consume: allow_consume.clone(),
        });
        authority.final_guard = guard.clone();
        let admitted = admit_from_verified_parts(budget, authority).expect("verified admission");

        let dispatch = thread::spawn(move || admitted.dispatch(|_| "transport-entered"));
        consume_entered.wait();

        let revoke_guard = guard.clone();
        let revoke_started = Arc::new(Barrier::new(2));
        let revoke_started_in_thread = revoke_started.clone();
        let revoke = thread::spawn(move || {
            revoke_started_in_thread.wait();
            revoke_guard.try_revoke()
        });
        revoke_started.wait();
        allow_consume.wait();

        assert_eq!(
            dispatch.join().expect("dispatch thread"),
            Ok("transport-entered")
        );
        assert!(
            !revoke.join().expect("revocation thread"),
            "revocation after the atomic authority consumption must not invalidate the granted send"
        );
        let snapshot = guard.snapshot.lock().expect("blocking authority lock");
        assert!(snapshot.consumed);
        assert!(snapshot.reservation_active);
    }

    #[test]
    fn admitted_send_is_consumed_by_the_only_dispatch_method() {
        let (admitted, _) = admitted_send();
        let calls = Cell::new(0);
        admitted
            .dispatch(|_| calls.set(calls.get() + 1))
            .expect("single consumption should dispatch");
        assert_eq!(calls.get(), 1);
        // A second call is impossible in safe Rust because dispatch takes self by value and neither
        // AdmittedSend nor AuthorizedPhysicalSend implements Clone.
    }

    #[test]
    fn every_skip_reason_has_stable_scope_and_code() {
        let cases = [
            (
                SendAdmissionSkipReason::ProviderInactive,
                SendAdmissionRetryScope::Provider,
                "provider_inactive",
            ),
            (
                SendAdmissionSkipReason::ProviderQuotaExhausted,
                SendAdmissionRetryScope::Provider,
                "provider_quota_exhausted",
            ),
            (
                SendAdmissionSkipReason::ProviderConcurrencyExhausted,
                SendAdmissionRetryScope::Provider,
                "provider_concurrency_exhausted",
            ),
            (
                SendAdmissionSkipReason::EndpointInactive,
                SendAdmissionRetryScope::Endpoint,
                "endpoint_inactive",
            ),
            (
                SendAdmissionSkipReason::EndpointUnhealthy,
                SendAdmissionRetryScope::Endpoint,
                "endpoint_unhealthy",
            ),
            (
                SendAdmissionSkipReason::CredentialInactive,
                SendAdmissionRetryScope::Credential,
                "credential_inactive",
            ),
            (
                SendAdmissionSkipReason::CredentialExpired,
                SendAdmissionRetryScope::Credential,
                "credential_expired",
            ),
            (
                SendAdmissionSkipReason::CredentialCircuitOpen,
                SendAdmissionRetryScope::Credential,
                "credential_circuit_open",
            ),
            (
                SendAdmissionSkipReason::CredentialUnhealthy,
                SendAdmissionRetryScope::Credential,
                "credential_unhealthy",
            ),
            (
                SendAdmissionSkipReason::CredentialRpmExhausted,
                SendAdmissionRetryScope::Credential,
                "credential_rpm_exhausted",
            ),
            (
                SendAdmissionSkipReason::CredentialConcurrencyExhausted,
                SendAdmissionRetryScope::Credential,
                "credential_concurrency_exhausted",
            ),
            (
                SendAdmissionSkipReason::AccountQuotaExhausted,
                SendAdmissionRetryScope::Credential,
                "account_quota_exhausted",
            ),
            (
                SendAdmissionSkipReason::OAuthInvalid,
                SendAdmissionRetryScope::Credential,
                "oauth_invalid",
            ),
            (
                SendAdmissionSkipReason::ModelUnavailable,
                SendAdmissionRetryScope::Candidate,
                "model_unavailable",
            ),
            (
                SendAdmissionSkipReason::BindingMissing,
                SendAdmissionRetryScope::Candidate,
                "binding_missing",
            ),
        ];
        for (reason, scope, code) in cases {
            assert_eq!(reason.retry_scope(), scope);
            assert_eq!(reason.as_str(), code);
        }
    }

    #[test]
    fn every_decision_declares_its_budget_effect() {
        let (admitted, _) = admitted_send();
        assert_eq!(
            SendAdmissionDecision::Admit(Box::new(admitted)).budget_effect(),
            SendAdmissionBudgetEffect::DispatchReservation
        );
        assert_eq!(
            SendAdmissionDecision::Skip(SendAdmissionSkip::new(
                SendAdmissionSkipReason::CredentialCircuitOpen,
            ))
            .budget_effect(),
            SendAdmissionBudgetEffect::AdmissionDisposition
        );
        let stop = SendAdmissionDecision::Stop(SendAdmissionStop::new(
            SendAdmissionStopReason::AuthorityReadFailed,
        ));
        assert_eq!(
            stop.budget_effect(),
            SendAdmissionBudgetEffect::AdmissionDisposition
        );
        assert!(stop.is_terminal());
    }

    #[test]
    fn every_stop_reason_has_a_stable_code() {
        let cases = [
            (
                SendAdmissionStopReason::AuthorityUnavailable,
                "authority_unavailable",
            ),
            (
                SendAdmissionStopReason::AuthorityReadFailed,
                "authority_read_failed",
            ),
            (
                SendAdmissionStopReason::AuthorityInconsistent,
                "authority_inconsistent",
            ),
            (
                SendAdmissionStopReason::ReservationBackendUnavailable,
                "reservation_backend_unavailable",
            ),
            (SendAdmissionStopReason::BudgetExhausted, "budget_exhausted"),
            (
                SendAdmissionStopReason::DeadlineExceeded,
                "deadline_exceeded",
            ),
            (SendAdmissionStopReason::LeaseLost, "lease_lost"),
            (SendAdmissionStopReason::Cancelled, "cancelled"),
            (
                SendAdmissionStopReason::UnsupportedDistributedMemoryState,
                "unsupported_distributed_memory_state",
            ),
        ];
        for (reason, code) in cases {
            assert_eq!(reason.as_str(), code);
        }
    }
}
