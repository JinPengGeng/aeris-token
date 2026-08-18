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

#[derive(Debug, PartialEq, Eq)]
pub struct SendAdmissionReservation {
    reservation_id: String,
    fencing_token: u64,
    expires_at_unix_ms: u64,
}

impl SendAdmissionReservation {
    pub fn new(
        reservation_id: impl Into<String>,
        fencing_token: u64,
        expires_at_unix_ms: u64,
    ) -> Result<Self, SendAdmissionEvidenceError> {
        let reservation_id = reservation_id.into();
        if reservation_id.trim().is_empty() {
            return Err(SendAdmissionEvidenceError::ReservationIdMissing);
        }
        if fencing_token == 0 {
            return Err(SendAdmissionEvidenceError::FencingTokenMissing);
        }
        Ok(Self {
            reservation_id,
            fencing_token,
            expires_at_unix_ms,
        })
    }

    pub fn reservation_id(&self) -> &str {
        &self.reservation_id
    }

    pub const fn fencing_token(&self) -> u64 {
        self.fencing_token
    }

    pub const fn expires_at_unix_ms(&self) -> u64 {
        self.expires_at_unix_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAdmissionEvidenceError {
    IncompleteBinding,
    ReservationIdMissing,
    FencingTokenMissing,
    InvalidValidityWindow,
    ReservationExpiresBeforeAdmission,
}

/// Proof that one physical upstream dispatch passed send-time admission.
///
/// This type intentionally does not implement `Clone`. Transport entrypoints should
/// take it by value so the same grant cannot accidentally authorize multiple sends.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "an admitted send must be consumed by exactly one physical dispatch"]
pub struct AdmittedSend {
    binding: SendAdmissionBinding,
    authority_revision: SendAuthorityRevision,
    reservation: SendAdmissionReservation,
    admitted_at_unix_ms: u64,
    valid_until_unix_ms: u64,
}

impl AdmittedSend {
    pub(crate) fn new(
        binding: SendAdmissionBinding,
        authority_revision: SendAuthorityRevision,
        reservation: SendAdmissionReservation,
        admitted_at_unix_ms: u64,
        valid_until_unix_ms: u64,
    ) -> Result<Self, SendAdmissionEvidenceError> {
        if valid_until_unix_ms <= admitted_at_unix_ms {
            return Err(SendAdmissionEvidenceError::InvalidValidityWindow);
        }
        if reservation.expires_at_unix_ms() < valid_until_unix_ms {
            return Err(SendAdmissionEvidenceError::ReservationExpiresBeforeAdmission);
        }
        Ok(Self {
            binding,
            authority_revision,
            reservation,
            admitted_at_unix_ms,
            valid_until_unix_ms,
        })
    }

    pub fn binding(&self) -> &SendAdmissionBinding {
        &self.binding
    }

    pub const fn authority_revision(&self) -> SendAuthorityRevision {
        self.authority_revision
    }

    pub fn reservation(&self) -> &SendAdmissionReservation {
        &self.reservation
    }

    pub const fn admitted_at_unix_ms(&self) -> u64 {
        self.admitted_at_unix_ms
    }

    pub const fn valid_until_unix_ms(&self) -> u64 {
        self.valid_until_unix_ms
    }

    pub const fn is_valid_at(&self, now_unix_ms: u64) -> bool {
        now_unix_ms >= self.admitted_at_unix_ms && now_unix_ms < self.valid_until_unix_ms
    }
}

/// Issues the linear transport token from already-verified authority and reservation evidence.
///
/// Runtime integration should centralize calls to this factory in the send-admission adapter;
/// ordinary transport code should only receive the returned token by value.
pub fn issue_admitted_send(
    binding: SendAdmissionBinding,
    authority_revision: SendAuthorityRevision,
    reservation: SendAdmissionReservation,
    admitted_at_unix_ms: u64,
    valid_until_unix_ms: u64,
) -> Result<AdmittedSend, SendAdmissionEvidenceError> {
    AdmittedSend::new(
        binding,
        authority_revision,
        reservation,
        admitted_at_unix_ms,
        valid_until_unix_ms,
    )
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
    PhysicalDispatch,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendAdmissionDecision {
    Admit(AdmittedSend),
    Skip(SendAdmissionSkip),
    Stop(SendAdmissionStop),
}

impl SendAdmissionDecision {
    pub const fn budget_effect(&self) -> SendAdmissionBudgetEffect {
        match self {
            Self::Admit(_) => SendAdmissionBudgetEffect::PhysicalDispatch,
            Self::Skip(_) | Self::Stop(_) => SendAdmissionBudgetEffect::AdmissionDisposition,
        }
    }

    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Stop(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admitted_send() -> AdmittedSend {
        issue_admitted_send(
            SendAdmissionBinding::new("provider", "endpoint", "key", "openai:responses")
                .expect("binding"),
            SendAuthorityRevision::new(17),
            SendAdmissionReservation::new("reservation", 42, 2_000).expect("reservation"),
            1_000,
            1_500,
        )
        .expect("admitted send")
    }

    #[test]
    fn admitted_send_retains_authority_and_fencing_evidence() {
        let admitted = admitted_send();
        assert_eq!(admitted.binding().provider_id(), "provider");
        assert_eq!(admitted.binding().endpoint_id(), "endpoint");
        assert_eq!(admitted.binding().key_id(), "key");
        assert_eq!(admitted.binding().api_format(), "openai:responses");
        assert_eq!(admitted.authority_revision().get(), 17);
        assert_eq!(admitted.reservation().fencing_token(), 42);
        assert!(admitted.is_valid_at(1_000));
        assert!(admitted.is_valid_at(1_499));
        assert!(!admitted.is_valid_at(1_500));
    }

    #[test]
    fn invalid_admission_evidence_fails_closed() {
        assert_eq!(
            SendAdmissionBinding::new("", "endpoint", "key", "format"),
            Err(SendAdmissionEvidenceError::IncompleteBinding)
        );
        assert_eq!(
            SendAdmissionReservation::new("reservation", 0, 2_000),
            Err(SendAdmissionEvidenceError::FencingTokenMissing)
        );
        assert_eq!(
            issue_admitted_send(
                SendAdmissionBinding::new("provider", "endpoint", "key", "format")
                    .expect("binding"),
                SendAuthorityRevision::new(1),
                SendAdmissionReservation::new("reservation", 1, 1_100).expect("reservation"),
                1_000,
                1_200,
            ),
            Err(SendAdmissionEvidenceError::ReservationExpiresBeforeAdmission)
        );
    }

    #[test]
    fn skip_reasons_have_fixed_retry_scopes() {
        for reason in [
            SendAdmissionSkipReason::ProviderInactive,
            SendAdmissionSkipReason::ProviderQuotaExhausted,
            SendAdmissionSkipReason::ProviderConcurrencyExhausted,
        ] {
            assert_eq!(reason.retry_scope(), SendAdmissionRetryScope::Provider);
        }
        for reason in [
            SendAdmissionSkipReason::EndpointInactive,
            SendAdmissionSkipReason::EndpointUnhealthy,
        ] {
            assert_eq!(reason.retry_scope(), SendAdmissionRetryScope::Endpoint);
        }
        for reason in [
            SendAdmissionSkipReason::CredentialInactive,
            SendAdmissionSkipReason::CredentialExpired,
            SendAdmissionSkipReason::CredentialCircuitOpen,
            SendAdmissionSkipReason::CredentialUnhealthy,
            SendAdmissionSkipReason::CredentialRpmExhausted,
            SendAdmissionSkipReason::CredentialConcurrencyExhausted,
            SendAdmissionSkipReason::AccountQuotaExhausted,
            SendAdmissionSkipReason::OAuthInvalid,
        ] {
            assert_eq!(reason.retry_scope(), SendAdmissionRetryScope::Credential);
        }
        for reason in [
            SendAdmissionSkipReason::ModelUnavailable,
            SendAdmissionSkipReason::BindingMissing,
        ] {
            assert_eq!(reason.retry_scope(), SendAdmissionRetryScope::Candidate);
        }
    }

    #[test]
    fn every_decision_declares_its_budget_effect() {
        assert_eq!(
            SendAdmissionDecision::Admit(admitted_send()).budget_effect(),
            SendAdmissionBudgetEffect::PhysicalDispatch
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
    fn reason_codes_are_stable_for_metrics_and_audit() {
        assert_eq!(
            SendAdmissionSkipReason::CredentialRpmExhausted.as_str(),
            "credential_rpm_exhausted"
        );
        assert_eq!(
            SendAdmissionStopReason::UnsupportedDistributedMemoryState.as_str(),
            "unsupported_distributed_memory_state"
        );
    }
}
