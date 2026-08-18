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

#[derive(Debug, Clone, PartialEq, Eq)]
/// Observed metadata from a reservation decision.
///
/// This is not the live reservation lease or a capability accepted by transport.
pub struct SendAdmissionReservationEvidence {
    reservation_id: String,
    fencing_token: u64,
    expires_at_unix_ms: u64,
}

impl SendAdmissionReservationEvidence {
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

/// Non-authoritative evidence captured when send-time admission returned `Admit`.
///
/// This value is diagnostic contract data. It is not a transport capability, does
/// not authorize a physical send, and does not prove that a send occurred. The
/// gateway-owned dispatch port must define and consume the real one-shot capability.
/// This evidence is deliberately cloneable because cloning it conveys no authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendAdmissionEvidence {
    binding: SendAdmissionBinding,
    authority_revision: SendAuthorityRevision,
    reservation: SendAdmissionReservationEvidence,
    admitted_at_unix_ms: u64,
    valid_until_unix_ms: u64,
}

impl SendAdmissionEvidence {
    pub fn new(
        binding: SendAdmissionBinding,
        authority_revision: SendAuthorityRevision,
        reservation: SendAdmissionReservationEvidence,
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
    /// The Admit disposition requires the request budget to reserve a dispatch.
    /// It does not assert that a physical dispatch already happened.
    DispatchReservation,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendAdmissionDecision {
    /// Dynamic checks passed at observation time. The enclosed evidence is not
    /// authorization to send and must not be accepted by a transport API.
    Admit(SendAdmissionEvidence),
    Skip(SendAdmissionSkip),
    Stop(SendAdmissionStop),
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
    use super::*;

    fn admission_evidence() -> SendAdmissionEvidence {
        SendAdmissionEvidence::new(
            SendAdmissionBinding::new("provider", "endpoint", "key", "openai:responses")
                .expect("binding"),
            SendAuthorityRevision::new(17),
            SendAdmissionReservationEvidence::new("reservation", 42, 2_000)
                .expect("reservation evidence"),
            1_000,
            1_500,
        )
        .expect("admission evidence")
    }

    #[test]
    fn admission_evidence_retains_authority_and_fencing_facts() {
        let evidence = admission_evidence();
        assert_eq!(evidence.binding.provider_id(), "provider");
        assert_eq!(evidence.binding.endpoint_id(), "endpoint");
        assert_eq!(evidence.binding.key_id(), "key");
        assert_eq!(evidence.binding.api_format(), "openai:responses");
        assert_eq!(evidence.authority_revision.get(), 17);
        assert_eq!(evidence.reservation.fencing_token(), 42);
    }

    fn evidence_window_contains(evidence: &SendAdmissionEvidence, now_unix_ms: u64) -> bool {
        now_unix_ms >= evidence.admitted_at_unix_ms && now_unix_ms < evidence.valid_until_unix_ms
    }

    #[test]
    fn invalid_admission_evidence_fails_closed() {
        assert_eq!(
            SendAdmissionBinding::new("", "endpoint", "key", "format"),
            Err(SendAdmissionEvidenceError::IncompleteBinding)
        );
        assert_eq!(
            SendAdmissionReservationEvidence::new("reservation", 0, 2_000),
            Err(SendAdmissionEvidenceError::FencingTokenMissing)
        );
        assert_eq!(
            SendAdmissionEvidence::new(
                SendAdmissionBinding::new("provider", "endpoint", "key", "format")
                    .expect("binding"),
                SendAuthorityRevision::new(1),
                SendAdmissionReservationEvidence::new("reservation", 1, 1_000)
                    .expect("reservation evidence"),
                1_000,
                1_000,
            ),
            Err(SendAdmissionEvidenceError::InvalidValidityWindow)
        );
        assert_eq!(
            SendAdmissionEvidence::new(
                SendAdmissionBinding::new("provider", "endpoint", "key", "format")
                    .expect("binding"),
                SendAuthorityRevision::new(1),
                SendAdmissionReservationEvidence::new("reservation", 1, 1_100)
                    .expect("reservation evidence"),
                1_000,
                1_200,
            ),
            Err(SendAdmissionEvidenceError::ReservationExpiresBeforeAdmission)
        );
    }

    #[test]
    fn reservation_expiry_equal_to_evidence_end_is_accepted() {
        let evidence = SendAdmissionEvidence::new(
            SendAdmissionBinding::new("provider", "endpoint", "key", "format").expect("binding"),
            SendAuthorityRevision::new(1),
            SendAdmissionReservationEvidence::new("reservation", 1, 1_500)
                .expect("reservation evidence"),
            1_000,
            1_500,
        )
        .expect("equal expiry should be accepted");
        assert_eq!(evidence.reservation.expires_at_unix_ms(), 1_500);
    }

    #[test]
    fn evidence_window_uses_inclusive_start_and_exclusive_end() {
        let evidence = admission_evidence();
        assert!(!evidence_window_contains(&evidence, 999));
        assert!(evidence_window_contains(&evidence, 1_000));
        assert!(evidence_window_contains(&evidence, 1_499));
        assert!(!evidence_window_contains(&evidence, 1_500));
        assert!(!evidence_window_contains(&evidence, 1_501));
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
        assert_eq!(
            SendAdmissionDecision::Admit(admission_evidence()).budget_effect(),
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
