use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::SchedulerRankableCandidate;

pub const MAX_EMERGENCY_CHAIN_GRANT_TTL_SECS: i64 = 24 * 60 * 60;

const MAX_OPAQUE_VALUE_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EmergencyChainGrantId(String);

impl EmergencyChainGrantId {
    pub fn new(value: impl Into<String>) -> Result<Self, EmergencyChainGrantBuildError> {
        let value = value.into();
        validate_opaque_value(&value, "grant_id")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EmergencyChainPrincipal(String);

impl EmergencyChainPrincipal {
    pub fn new(value: impl Into<String>) -> Result<Self, EmergencyChainGrantBuildError> {
        let value = value.into();
        validate_opaque_value(&value, "principal")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EmergencyChainOperation(String);

impl EmergencyChainOperation {
    pub fn new(value: impl Into<String>) -> Result<Self, EmergencyChainGrantBuildError> {
        let value = value.into();
        validate_opaque_value(&value, "operation")?;
        if value == "*" {
            return Err(EmergencyChainGrantBuildError::WildcardOperation);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EmergencyChainTargetIdentity {
    provider_id: String,
    endpoint_id: String,
    key_id: String,
}

impl EmergencyChainTargetIdentity {
    pub fn new(
        provider_id: impl Into<String>,
        endpoint_id: impl Into<String>,
        key_id: impl Into<String>,
    ) -> Result<Self, EmergencyChainGrantBuildError> {
        let provider_id = provider_id.into();
        let endpoint_id = endpoint_id.into();
        let key_id = key_id.into();
        validate_opaque_value(&provider_id, "provider_id")?;
        validate_opaque_value(&endpoint_id, "endpoint_id")?;
        validate_opaque_value(&key_id, "key_id")?;
        Ok(Self {
            provider_id,
            endpoint_id,
            key_id,
        })
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

    fn matches_candidate(&self, candidate: &SchedulerRankableCandidate) -> bool {
        self.provider_id == candidate.provider_id
            && self.endpoint_id == candidate.endpoint_id
            && self.key_id == candidate.key_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmergencyChainHash(String);

impl EmergencyChainHash {
    pub fn parse(value: impl Into<String>) -> Result<Self, EmergencyChainGrantBuildError> {
        let value = value.into();
        validate_sha256_hex(&value, EmergencyChainGrantBuildError::InvalidChainHash)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmergencyChainRequestFingerprint(String);

impl EmergencyChainRequestFingerprint {
    pub fn parse(value: impl Into<String>) -> Result<Self, EmergencyChainGrantBuildError> {
        let value = value.into();
        validate_sha256_hex(
            &value,
            EmergencyChainGrantBuildError::InvalidRequestFingerprint,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmergencyChainSessionNonce(String);

impl EmergencyChainSessionNonce {
    pub fn new(value: impl Into<String>) -> Result<Self, EmergencyChainGrantBuildError> {
        let value = value.into();
        validate_opaque_value(&value, "session_nonce")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmergencyChainRequestScope {
    request_id: String,
    request_fingerprint: EmergencyChainRequestFingerprint,
    session_nonce: EmergencyChainSessionNonce,
}

impl EmergencyChainRequestScope {
    pub fn new(
        request_id: impl Into<String>,
        request_fingerprint: EmergencyChainRequestFingerprint,
        session_nonce: EmergencyChainSessionNonce,
    ) -> Result<Self, EmergencyChainGrantBuildError> {
        let request_id = request_id.into();
        validate_opaque_value(&request_id, "request_id")?;
        Ok(Self {
            request_id,
            request_fingerprint,
            session_nonce,
        })
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn request_fingerprint(&self) -> &EmergencyChainRequestFingerprint {
        &self.request_fingerprint
    }

    pub fn session_nonce(&self) -> &EmergencyChainSessionNonce {
        &self.session_nonce
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmergencyChainGrantBuildError {
    InvalidOpaqueValue { field: &'static str },
    WildcardOperation,
    EmptyOperationScope,
    DuplicateOperation,
    EmptyTargetChain,
    DuplicateTarget,
    InvalidValidityWindow,
    ValidityWindowTooLong,
    InvalidChainHash,
    InvalidRequestFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmergencyChainSessionState {
    Ready { next_position: usize },
    Outstanding { position: usize, permit_serial: u64 },
    Exhausted,
}

#[derive(Debug)]
pub struct EmergencyChainGrant {
    id: EmergencyChainGrantId,
    principal: EmergencyChainPrincipal,
    operations: Vec<EmergencyChainOperation>,
    request_scope: EmergencyChainRequestScope,
    targets: Vec<EmergencyChainTargetIdentity>,
    chain_hash: EmergencyChainHash,
    issued_at_unix_secs: i64,
    expires_at_unix_secs: i64,
    revoked_at_unix_secs: Option<i64>,
    next_permit_serial: u64,
    session_state: EmergencyChainSessionState,
}

pub struct IssueEmergencyChainGrant {
    pub id: EmergencyChainGrantId,
    pub principal: EmergencyChainPrincipal,
    pub operations: Vec<EmergencyChainOperation>,
    pub request_scope: EmergencyChainRequestScope,
    pub targets: Vec<EmergencyChainTargetIdentity>,
    pub issued_at_unix_secs: i64,
    pub expires_at_unix_secs: i64,
}

impl EmergencyChainGrant {
    pub fn issue(input: IssueEmergencyChainGrant) -> Result<Self, EmergencyChainGrantBuildError> {
        if input.operations.is_empty() {
            return Err(EmergencyChainGrantBuildError::EmptyOperationScope);
        }
        if input.targets.is_empty() {
            return Err(EmergencyChainGrantBuildError::EmptyTargetChain);
        }
        if input.expires_at_unix_secs <= input.issued_at_unix_secs {
            return Err(EmergencyChainGrantBuildError::InvalidValidityWindow);
        }
        let ttl = input
            .expires_at_unix_secs
            .checked_sub(input.issued_at_unix_secs)
            .ok_or(EmergencyChainGrantBuildError::InvalidValidityWindow)?;
        if ttl > MAX_EMERGENCY_CHAIN_GRANT_TTL_SECS {
            return Err(EmergencyChainGrantBuildError::ValidityWindowTooLong);
        }

        let mut operations = BTreeSet::new();
        for operation in input.operations {
            if !operations.insert(operation) {
                return Err(EmergencyChainGrantBuildError::DuplicateOperation);
            }
        }
        let operations = operations.into_iter().collect::<Vec<_>>();

        let mut unique_targets = BTreeSet::new();
        for target in &input.targets {
            if !unique_targets.insert(target.clone()) {
                return Err(EmergencyChainGrantBuildError::DuplicateTarget);
            }
        }

        let chain_hash = hash_target_chain(&input.targets);
        Ok(Self {
            id: input.id,
            principal: input.principal,
            operations,
            request_scope: input.request_scope,
            targets: input.targets,
            chain_hash,
            issued_at_unix_secs: input.issued_at_unix_secs,
            expires_at_unix_secs: input.expires_at_unix_secs,
            revoked_at_unix_secs: None,
            next_permit_serial: 0,
            session_state: EmergencyChainSessionState::Ready { next_position: 0 },
        })
    }

    pub fn id(&self) -> &EmergencyChainGrantId {
        &self.id
    }

    pub fn principal(&self) -> &EmergencyChainPrincipal {
        &self.principal
    }

    pub fn operations(&self) -> &[EmergencyChainOperation] {
        &self.operations
    }

    pub fn request_scope(&self) -> &EmergencyChainRequestScope {
        &self.request_scope
    }

    pub fn targets(&self) -> &[EmergencyChainTargetIdentity] {
        &self.targets
    }

    pub fn chain_hash(&self) -> &EmergencyChainHash {
        &self.chain_hash
    }

    pub fn issued_at_unix_secs(&self) -> i64 {
        self.issued_at_unix_secs
    }

    pub fn expires_at_unix_secs(&self) -> i64 {
        self.expires_at_unix_secs
    }

    pub fn revoked_at_unix_secs(&self) -> Option<i64> {
        self.revoked_at_unix_secs
    }

    pub fn revoke(
        &mut self,
        revoked_at_unix_secs: i64,
    ) -> Result<EmergencyChainRevokeOutcome, EmergencyChainRevokeError> {
        if revoked_at_unix_secs < self.issued_at_unix_secs {
            return Err(EmergencyChainRevokeError::BeforeIssue);
        }
        match self.revoked_at_unix_secs {
            None => {
                self.revoked_at_unix_secs = Some(revoked_at_unix_secs);
                Ok(EmergencyChainRevokeOutcome::Revoked)
            }
            Some(existing) if revoked_at_unix_secs < existing => {
                self.revoked_at_unix_secs = Some(revoked_at_unix_secs);
                Ok(EmergencyChainRevokeOutcome::EffectiveTimeTightened)
            }
            Some(_) => Ok(EmergencyChainRevokeOutcome::AlreadyRevoked),
        }
    }

    pub fn apply_authoritative_safe_skip(
        &mut self,
        permit: EmergencyChainAttemptPermit,
        proof: EmergencyChainSafeSkipProof,
        context: &TrustedEmergencyChainContext,
    ) -> Result<EmergencyChainSessionProgress, EmergencyChainSafeSkipError> {
        validate_trusted_context(self, context).map_err(EmergencyChainSafeSkipError::Gate)?;
        let (position, _) = self
            .outstanding_permit_identity(&permit)
            .map_err(EmergencyChainSafeSkipError::Permit)?;
        if !proof.matches(&permit)
            || proof.observed_at_unix_secs < permit.authorized_at_unix_secs
            || proof.observed_at_unix_secs > context.gate_at.unix_secs
        {
            return Err(EmergencyChainSafeSkipError::ProofMismatch);
        }
        Ok(self.advance_after_retryable(position))
    }

    fn outstanding_permit_identity(
        &self,
        permit: &EmergencyChainAttemptPermit,
    ) -> Result<(usize, u64), EmergencyChainPermitError> {
        let EmergencyChainSessionState::Outstanding {
            position,
            permit_serial,
        } = self.session_state
        else {
            return Err(EmergencyChainPermitError::NoOutstandingPermit);
        };
        if permit.grant_id != self.id
            || permit.chain_hash != self.chain_hash
            || permit.request_scope != self.request_scope
            || permit.chain_position != position
            || permit.permit_serial != permit_serial
            || self.targets.get(position) != Some(&permit.target)
        {
            return Err(EmergencyChainPermitError::PermitMismatch);
        }
        Ok((position, permit_serial))
    }

    fn advance_after_retryable(&mut self, position: usize) -> EmergencyChainSessionProgress {
        let next_position = position + 1;
        if next_position >= self.targets.len() {
            self.session_state = EmergencyChainSessionState::Exhausted;
            EmergencyChainSessionProgress::Exhausted
        } else {
            self.session_state = EmergencyChainSessionState::Ready { next_position };
            EmergencyChainSessionProgress::ReadyForNextTarget { next_position }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainRevokeOutcome {
    Revoked,
    EffectiveTimeTightened,
    AlreadyRevoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainRevokeError {
    BeforeIssue,
}

/// Sealed authority capability. This crate intentionally exposes no safe
/// constructor. The future gateway integration must own the only instance.
///
/// ```compile_fail
/// use aether_scheduler_core::GatewayEmergencyChainAuthority;
/// let _forged = GatewayEmergencyChainAuthority { _sealed: () };
/// ```
#[derive(Debug)]
pub struct GatewayEmergencyChainAuthority {
    _sealed: (),
}

/// Separate sealed capability owned by the authoritative attempt/materialization
/// ledger. The gateway dispatch authority cannot mint skip proofs.
///
/// ```compile_fail
/// use aether_scheduler_core::EmergencyChainLedgerAuthority;
/// let _forged = EmergencyChainLedgerAuthority { _sealed: () };
/// ```
#[derive(Debug)]
pub struct EmergencyChainLedgerAuthority {
    _sealed: (),
}

#[derive(Debug)]
pub struct ServerNormalRoutingActivation {
    _private: (),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerEmergencyChainInstant {
    unix_secs: i64,
}

impl ServerEmergencyChainInstant {
    pub fn unix_secs(self) -> i64 {
        self.unix_secs
    }
}

#[derive(Debug)]
pub struct AuthenticatedEmergencyChainPrincipal(EmergencyChainPrincipal);

#[derive(Debug)]
pub struct ServerSelectedEmergencyChainOperation(EmergencyChainOperation);

#[derive(Debug)]
pub struct ServerEmergencyChainGrantActivation {
    grant_id: EmergencyChainGrantId,
    chain_hash: EmergencyChainHash,
}

/// Scope derived from the live authenticated request, independently of the
/// stored grant record.
#[derive(Debug)]
pub struct LiveEmergencyChainRequestContext(EmergencyChainRequestScope);

#[derive(Debug)]
pub struct TrustedEmergencyChainContext {
    activation: ServerEmergencyChainGrantActivation,
    principal: AuthenticatedEmergencyChainPrincipal,
    operation: ServerSelectedEmergencyChainOperation,
    live_request: LiveEmergencyChainRequestContext,
    gate_at: ServerEmergencyChainInstant,
}

impl GatewayEmergencyChainAuthority {
    pub fn normal_routing_activation(&self) -> ServerNormalRoutingActivation {
        ServerNormalRoutingActivation { _private: () }
    }

    pub fn server_clock_instant(&self, unix_secs: i64) -> ServerEmergencyChainInstant {
        ServerEmergencyChainInstant { unix_secs }
    }

    pub fn authenticated_principal(
        &self,
        principal: EmergencyChainPrincipal,
    ) -> AuthenticatedEmergencyChainPrincipal {
        AuthenticatedEmergencyChainPrincipal(principal)
    }

    pub fn selected_operation(
        &self,
        operation: EmergencyChainOperation,
    ) -> ServerSelectedEmergencyChainOperation {
        ServerSelectedEmergencyChainOperation(operation)
    }

    pub fn grant_activation(
        &self,
        grant_id: EmergencyChainGrantId,
        chain_hash: EmergencyChainHash,
    ) -> ServerEmergencyChainGrantActivation {
        ServerEmergencyChainGrantActivation {
            grant_id,
            chain_hash,
        }
    }

    pub fn live_request_context(
        &self,
        request_scope: EmergencyChainRequestScope,
    ) -> LiveEmergencyChainRequestContext {
        LiveEmergencyChainRequestContext(request_scope)
    }

    pub fn trusted_context(
        &self,
        activation: ServerEmergencyChainGrantActivation,
        principal: AuthenticatedEmergencyChainPrincipal,
        operation: ServerSelectedEmergencyChainOperation,
        live_request: LiveEmergencyChainRequestContext,
        gate_at: ServerEmergencyChainInstant,
    ) -> TrustedEmergencyChainContext {
        TrustedEmergencyChainContext {
            activation,
            principal,
            operation,
            live_request,
            gate_at,
        }
    }
}

impl EmergencyChainLedgerAuthority {
    /// Refuses to mint a proof until the authoritative ledger integration is
    /// present. A ledger entry ID alone is not evidence that a slot was
    /// recorded as safely unmaterializable.
    pub fn authoritative_safe_skip_proof(
        &self,
        _permit: &EmergencyChainAttemptPermit,
        _ledger_entry_id: impl Into<String>,
        _observed_at: ServerEmergencyChainInstant,
    ) -> Result<EmergencyChainSafeSkipProof, EmergencyChainLedgerError> {
        Err(EmergencyChainLedgerError::AuthoritativeLedgerUnavailable)
    }

    #[cfg(test)]
    fn test_verified_safe_skip_proof(
        &self,
        permit: &EmergencyChainAttemptPermit,
        ledger_entry_id: impl Into<String>,
        observed_at: ServerEmergencyChainInstant,
    ) -> Result<EmergencyChainSafeSkipProof, EmergencyChainGrantBuildError> {
        let ledger_entry_id = ledger_entry_id.into();
        validate_opaque_value(&ledger_entry_id, "ledger_entry_id")?;
        Ok(EmergencyChainSafeSkipProof {
            grant_id: permit.grant_id.clone(),
            chain_hash: permit.chain_hash.clone(),
            request_scope: permit.request_scope.clone(),
            chain_position: permit.chain_position,
            target: permit.target.clone(),
            permit_serial: permit.permit_serial,
            ledger_entry_id,
            observed_at_unix_secs: observed_at.unix_secs,
        })
    }
}

/// The preparatory domain slice has no authoritative materialization ledger.
/// Production code must keep emergency progress locked until that integration
/// can verify a committed record for the exact permit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainLedgerError {
    AuthoritativeLedgerUnavailable,
}

pub enum EmergencyChainGateRequest<'a> {
    NormalRouting(&'a ServerNormalRoutingActivation),
    Emergency(EmergencyChainUseRequest<'a>),
}

pub struct EmergencyChainUseRequest<'a> {
    pub trusted_context: &'a TrustedEmergencyChainContext,
    pub requested_target: &'a EmergencyChainTargetIdentity,
}

#[derive(Debug)]
pub enum EmergencyChainGateDecision {
    NormalRouting,
    EmergencyTargetAuthorized(Box<EmergencyChainAttemptPermit>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainGateError {
    UnknownGrant,
    PrincipalDenied,
    OperationDenied,
    RequestScopeDenied,
    NotYetValid,
    Expired,
    Revoked,
    ChainHashMismatch,
    TargetOutsideChain,
    TargetOutOfOrder,
    AttemptAlreadyOutstanding,
    ChainExhausted,
    PermitSerialExhausted,
}

/// ```compile_fail
/// use aether_scheduler_core::EmergencyChainAttemptPermit;
/// let _forged = EmergencyChainAttemptPermit {};
/// ```
///
/// ```compile_fail
/// use aether_scheduler_core::EmergencyChainAttemptPermit;
/// fn bypass_domain(permit: EmergencyChainAttemptPermit) {
///     permit.dispatch_once(|_| Ok::<(), ()>(()));
/// }
/// ```
pub struct EmergencyChainAttemptPermit {
    grant_id: EmergencyChainGrantId,
    chain_hash: EmergencyChainHash,
    request_scope: EmergencyChainRequestScope,
    chain_position: usize,
    target: EmergencyChainTargetIdentity,
    permit_serial: u64,
    authorized_at_unix_secs: i64,
}

impl std::fmt::Debug for EmergencyChainAttemptPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EmergencyChainAttemptPermit(<opaque>)")
    }
}

/// ```compile_fail
/// use aether_scheduler_core::EmergencyChainSafeSkipProof;
/// let _forged = EmergencyChainSafeSkipProof {};
/// ```
pub struct EmergencyChainSafeSkipProof {
    grant_id: EmergencyChainGrantId,
    chain_hash: EmergencyChainHash,
    request_scope: EmergencyChainRequestScope,
    chain_position: usize,
    target: EmergencyChainTargetIdentity,
    permit_serial: u64,
    ledger_entry_id: String,
    observed_at_unix_secs: i64,
}

impl std::fmt::Debug for EmergencyChainSafeSkipProof {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EmergencyChainSafeSkipProof(<opaque>)")
    }
}

impl EmergencyChainSafeSkipProof {
    fn matches(&self, permit: &EmergencyChainAttemptPermit) -> bool {
        !self.ledger_entry_id.is_empty()
            && self.grant_id == permit.grant_id
            && self.chain_hash == permit.chain_hash
            && self.request_scope == permit.request_scope
            && self.chain_position == permit.chain_position
            && self.target == permit.target
            && self.permit_serial == permit.permit_serial
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainPermitError {
    NoOutstandingPermit,
    PermitMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainSafeSkipError {
    Gate(EmergencyChainGateError),
    Permit(EmergencyChainPermitError),
    ProofMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainSessionProgress {
    ReadyForNextTarget { next_position: usize },
    Exhausted,
}

fn validate_trusted_context(
    grant: &EmergencyChainGrant,
    context: &TrustedEmergencyChainContext,
) -> Result<(), EmergencyChainGateError> {
    if grant.id != context.activation.grant_id {
        return Err(EmergencyChainGateError::UnknownGrant);
    }
    if grant.principal != context.principal.0 {
        return Err(EmergencyChainGateError::PrincipalDenied);
    }
    if grant
        .operations
        .binary_search(&context.operation.0)
        .is_err()
    {
        return Err(EmergencyChainGateError::OperationDenied);
    }
    if grant.request_scope != context.live_request.0 {
        return Err(EmergencyChainGateError::RequestScopeDenied);
    }
    if context.gate_at.unix_secs < grant.issued_at_unix_secs {
        return Err(EmergencyChainGateError::NotYetValid);
    }
    if context.gate_at.unix_secs >= grant.expires_at_unix_secs {
        return Err(EmergencyChainGateError::Expired);
    }
    if grant
        .revoked_at_unix_secs
        .is_some_and(|revoked_at| revoked_at <= context.gate_at.unix_secs)
    {
        return Err(EmergencyChainGateError::Revoked);
    }
    if grant.chain_hash != context.activation.chain_hash {
        return Err(EmergencyChainGateError::ChainHashMismatch);
    }
    Ok(())
}

/// Evaluates the emergency gate at the server-clock instant in the trusted
/// context and reserves exactly one target attempt.
///
/// Validity is `[issued_at, expires_at)`. A revocation is effective when
/// `revoked_at <= gate_at`. Dropping the returned non-cloneable permit leaves
/// the grant locked, which fails closed. The eventual integration must
/// strong-read and persist this state transition at the send boundary.
pub fn evaluate_emergency_chain_gate(
    request: EmergencyChainGateRequest<'_>,
    loaded_grant: Option<&mut EmergencyChainGrant>,
) -> Result<EmergencyChainGateDecision, EmergencyChainGateError> {
    let EmergencyChainGateRequest::Emergency(request) = request else {
        return Ok(EmergencyChainGateDecision::NormalRouting);
    };
    let context = request.trusted_context;
    let grant = loaded_grant.ok_or(EmergencyChainGateError::UnknownGrant)?;
    validate_trusted_context(grant, context)?;

    let next_position = match grant.session_state {
        EmergencyChainSessionState::Ready { next_position } => next_position,
        EmergencyChainSessionState::Outstanding { .. } => {
            return Err(EmergencyChainGateError::AttemptAlreadyOutstanding)
        }
        EmergencyChainSessionState::Exhausted => {
            return Err(EmergencyChainGateError::ChainExhausted)
        }
    };
    let expected_target = &grant.targets[next_position];
    if !grant.targets.contains(request.requested_target) {
        return Err(EmergencyChainGateError::TargetOutsideChain);
    }
    if expected_target != request.requested_target {
        return Err(EmergencyChainGateError::TargetOutOfOrder);
    }

    let permit_serial = grant.next_permit_serial;
    grant.next_permit_serial = grant
        .next_permit_serial
        .checked_add(1)
        .ok_or(EmergencyChainGateError::PermitSerialExhausted)?;
    grant.session_state = EmergencyChainSessionState::Outstanding {
        position: next_position,
        permit_serial,
    };
    Ok(EmergencyChainGateDecision::EmergencyTargetAuthorized(
        Box::new(EmergencyChainAttemptPermit {
            grant_id: grant.id.clone(),
            chain_hash: grant.chain_hash.clone(),
            request_scope: grant.request_scope.clone(),
            chain_position: next_position,
            target: expected_target.clone(),
            permit_serial,
            authorized_at_unix_secs: context.gate_at.unix_secs,
        }),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmergencyChainCandidateMatch {
    pub chain_position: usize,
    pub candidate_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainCandidateOrderError {
    AmbiguousTarget,
}

/// Returns only candidates named by the grant, in immutable chain order.
/// Every grant slot is returned. A missing target has `candidate_index: None`;
/// callers must not compress it away. Duplicate identities fail closed.
pub fn emergency_chain_candidate_order(
    grant: &EmergencyChainGrant,
    candidates: &[SchedulerRankableCandidate],
) -> Result<Vec<EmergencyChainCandidateMatch>, EmergencyChainCandidateOrderError> {
    let mut ordered = Vec::with_capacity(grant.targets.len());
    for (chain_position, target) in grant.targets.iter().enumerate() {
        let mut matches = candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| target.matches_candidate(candidate));
        let first = matches.next();
        if matches.next().is_some() {
            return Err(EmergencyChainCandidateOrderError::AmbiguousTarget);
        }
        ordered.push(EmergencyChainCandidateMatch {
            chain_position,
            candidate_index: first.map(|(candidate_index, _)| candidate_index),
        });
    }
    Ok(ordered)
}

fn hash_target_chain(targets: &[EmergencyChainTargetIdentity]) -> EmergencyChainHash {
    let mut hasher = Sha256::new();
    hasher.update(b"aether-emergency-chain-v1\0");
    hasher.update((targets.len() as u64).to_be_bytes());
    for target in targets {
        hash_length_prefixed(&mut hasher, target.provider_id.as_bytes());
        hash_length_prefixed(&mut hasher, target.endpoint_id.as_bytes());
        hash_length_prefixed(&mut hasher, target.key_id.as_bytes());
    }
    EmergencyChainHash(hex_digest(hasher.finalize()))
}

fn hash_length_prefixed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_sha256_hex(
    value: &str,
    error: EmergencyChainGrantBuildError,
) -> Result<(), EmergencyChainGrantBuildError> {
    if value.len() != 64
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(error);
    }
    Ok(())
}

fn validate_opaque_value(
    value: &str,
    field: &'static str,
) -> Result<(), EmergencyChainGrantBuildError> {
    if value.is_empty()
        || value.len() > MAX_OPAQUE_VALUE_LEN
        || value.trim() != value
        || value.chars().any(char::is_control)
        || value.chars().any(char::is_whitespace)
        || value.contains("://")
    {
        return Err(EmergencyChainGrantBuildError::InvalidOpaqueValue { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProviderKeyHealthBucket, SchedulerTunnelAffinityBucket};

    const ISSUED_AT: i64 = 1_000_000;

    fn authority() -> GatewayEmergencyChainAuthority {
        GatewayEmergencyChainAuthority { _sealed: () }
    }

    fn ledger_authority() -> EmergencyChainLedgerAuthority {
        EmergencyChainLedgerAuthority { _sealed: () }
    }

    fn test_verified_safe_skip_proof(
        permit: &EmergencyChainAttemptPermit,
        ledger_entry_id: &str,
        observed_at: ServerEmergencyChainInstant,
    ) -> EmergencyChainSafeSkipProof {
        ledger_authority()
            .test_verified_safe_skip_proof(permit, ledger_entry_id, observed_at)
            .unwrap()
    }

    fn instant(unix_secs: i64) -> ServerEmergencyChainInstant {
        authority().server_clock_instant(unix_secs)
    }

    fn grant_id(value: &str) -> EmergencyChainGrantId {
        EmergencyChainGrantId::new(value).unwrap()
    }

    fn principal(value: &str) -> EmergencyChainPrincipal {
        EmergencyChainPrincipal::new(value).unwrap()
    }

    fn operation(value: &str) -> EmergencyChainOperation {
        EmergencyChainOperation::new(value).unwrap()
    }

    fn fingerprint(value: char) -> EmergencyChainRequestFingerprint {
        EmergencyChainRequestFingerprint::parse(value.to_string().repeat(64)).unwrap()
    }

    fn request_scope(id: &str, fingerprint_value: char, nonce: &str) -> EmergencyChainRequestScope {
        EmergencyChainRequestScope::new(
            id,
            fingerprint(fingerprint_value),
            EmergencyChainSessionNonce::new(nonce).unwrap(),
        )
        .unwrap()
    }

    fn target(id: &str) -> EmergencyChainTargetIdentity {
        EmergencyChainTargetIdentity::new(
            format!("provider-{id}"),
            format!("endpoint-{id}"),
            format!("key-{id}"),
        )
        .unwrap()
    }

    fn issue_grant(targets: Vec<EmergencyChainTargetIdentity>) -> EmergencyChainGrant {
        issue_grant_with_expiry(targets, ISSUED_AT + 300)
    }

    fn issue_grant_with_expiry(
        targets: Vec<EmergencyChainTargetIdentity>,
        expires_at_unix_secs: i64,
    ) -> EmergencyChainGrant {
        EmergencyChainGrant::issue(IssueEmergencyChainGrant {
            id: grant_id("grant-opaque-000001"),
            principal: principal("operator:alice"),
            operations: vec![operation("responses.create"), operation("chat.create")],
            request_scope: request_scope("request-000001", 'a', "nonce-000001"),
            targets,
            issued_at_unix_secs: ISSUED_AT,
            expires_at_unix_secs,
        })
        .unwrap()
    }

    fn context(
        grant: &EmergencyChainGrant,
        gate_at_unix_secs: i64,
    ) -> TrustedEmergencyChainContext {
        context_with(
            grant,
            grant.chain_hash().clone(),
            grant.principal().clone(),
            operation("responses.create"),
            request_scope("request-000001", 'a', "nonce-000001"),
            gate_at_unix_secs,
        )
    }

    fn context_with(
        grant: &EmergencyChainGrant,
        chain_hash: EmergencyChainHash,
        principal: EmergencyChainPrincipal,
        operation: EmergencyChainOperation,
        live_request_scope: EmergencyChainRequestScope,
        gate_at_unix_secs: i64,
    ) -> TrustedEmergencyChainContext {
        let authority = authority();
        authority.trusted_context(
            authority.grant_activation(grant.id().clone(), chain_hash),
            authority.authenticated_principal(principal),
            authority.selected_operation(operation),
            authority.live_request_context(live_request_scope),
            authority.server_clock_instant(gate_at_unix_secs),
        )
    }

    fn authorize(
        grant: &mut EmergencyChainGrant,
        requested_target: &EmergencyChainTargetIdentity,
        gate_at_unix_secs: i64,
    ) -> Result<EmergencyChainGateDecision, EmergencyChainGateError> {
        let context = context(grant, gate_at_unix_secs);
        evaluate_emergency_chain_gate(
            EmergencyChainGateRequest::Emergency(EmergencyChainUseRequest {
                trusted_context: &context,
                requested_target,
            }),
            Some(grant),
        )
    }

    fn permit(decision: EmergencyChainGateDecision) -> EmergencyChainAttemptPermit {
        let EmergencyChainGateDecision::EmergencyTargetAuthorized(permit) = decision else {
            panic!("expected emergency permit")
        };
        *permit
    }

    fn candidate(identity: &EmergencyChainTargetIdentity) -> SchedulerRankableCandidate {
        SchedulerRankableCandidate {
            provider_id: identity.provider_id().to_string(),
            endpoint_id: identity.endpoint_id().to_string(),
            key_id: identity.key_id().to_string(),
            selected_provider_model_name: "gpt-5".to_string(),
            provider_priority: 0,
            key_internal_priority: 0,
            key_global_priority_for_format: Some(0),
            capability_priority: (0, 0),
            cached_affinity_match: false,
            affinity_hash: None,
            tunnel_bucket: SchedulerTunnelAffinityBucket::Neutral,
            demote_cross_format: false,
            format_preference: (0, 0),
            health_bucket: None,
            health_score: 1.0,
            original_index: 0,
        }
    }

    #[test]
    fn issue_requires_scoped_bounded_non_duplicate_values() {
        let build = |operations, targets, expires_at_unix_secs| {
            EmergencyChainGrant::issue(IssueEmergencyChainGrant {
                id: grant_id("grant-opaque-000001"),
                principal: principal("operator:alice"),
                operations,
                request_scope: request_scope("request-000001", 'a', "nonce-000001"),
                targets,
                issued_at_unix_secs: ISSUED_AT,
                expires_at_unix_secs,
            })
        };

        assert_eq!(
            build(vec![], vec![target("a")], ISSUED_AT + 1).unwrap_err(),
            EmergencyChainGrantBuildError::EmptyOperationScope
        );
        assert_eq!(
            build(vec![operation("chat.create")], vec![], ISSUED_AT + 1).unwrap_err(),
            EmergencyChainGrantBuildError::EmptyTargetChain
        );
        assert_eq!(
            build(
                vec![operation("chat.create"), operation("chat.create")],
                vec![target("a")],
                ISSUED_AT + 1,
            )
            .unwrap_err(),
            EmergencyChainGrantBuildError::DuplicateOperation
        );
        assert_eq!(
            build(
                vec![operation("chat.create")],
                vec![target("a"), target("a")],
                ISSUED_AT + 1,
            )
            .unwrap_err(),
            EmergencyChainGrantBuildError::DuplicateTarget
        );
        assert_eq!(
            build(vec![operation("chat.create")], vec![target("a")], ISSUED_AT).unwrap_err(),
            EmergencyChainGrantBuildError::InvalidValidityWindow
        );
        assert_eq!(
            build(
                vec![operation("chat.create")],
                vec![target("a")],
                ISSUED_AT + MAX_EMERGENCY_CHAIN_GRANT_TTL_SECS + 1,
            )
            .unwrap_err(),
            EmergencyChainGrantBuildError::ValidityWindowTooLong
        );
        assert_eq!(
            EmergencyChainOperation::new("*"),
            Err(EmergencyChainGrantBuildError::WildcardOperation)
        );
    }

    #[test]
    fn exact_ttl_ceiling_is_accepted_and_one_second_more_is_rejected() {
        assert!(EmergencyChainGrant::issue(IssueEmergencyChainGrant {
            id: grant_id("grant-opaque-000001"),
            principal: principal("operator:alice"),
            operations: vec![operation("chat.create")],
            request_scope: request_scope("request-000001", 'a', "nonce-000001"),
            targets: vec![target("a")],
            issued_at_unix_secs: ISSUED_AT,
            expires_at_unix_secs: ISSUED_AT + MAX_EMERGENCY_CHAIN_GRANT_TTL_SECS,
        })
        .is_ok());
        assert_eq!(
            EmergencyChainGrant::issue(IssueEmergencyChainGrant {
                id: grant_id("grant-opaque-000001"),
                principal: principal("operator:alice"),
                operations: vec![operation("chat.create")],
                request_scope: request_scope("request-000001", 'a', "nonce-000001"),
                targets: vec![target("a")],
                issued_at_unix_secs: ISSUED_AT,
                expires_at_unix_secs: ISSUED_AT + MAX_EMERGENCY_CHAIN_GRANT_TTL_SECS + 1,
            })
            .unwrap_err(),
            EmergencyChainGrantBuildError::ValidityWindowTooLong
        );
    }

    #[test]
    fn partial_target_and_invalid_hash_inputs_are_rejected() {
        assert!(EmergencyChainTargetIdentity::new("", "endpoint-a", "key-a").is_err());
        assert!(EmergencyChainTargetIdentity::new("provider-a", "", "key-a").is_err());
        assert!(EmergencyChainTargetIdentity::new("provider-a", "endpoint-a", "").is_err());
        assert!(EmergencyChainTargetIdentity::new("https://provider", "endpoint", "key").is_err());
        assert_eq!(
            EmergencyChainHash::parse("0".repeat(63)),
            Err(EmergencyChainGrantBuildError::InvalidChainHash)
        );
        assert_eq!(
            EmergencyChainHash::parse("A".repeat(64)),
            Err(EmergencyChainGrantBuildError::InvalidChainHash)
        );
        assert_eq!(
            EmergencyChainRequestFingerprint::parse("z".repeat(64)),
            Err(EmergencyChainGrantBuildError::InvalidRequestFingerprint)
        );
    }

    #[test]
    fn chain_hash_has_golden_value_and_binds_strict_order() {
        let forward = issue_grant(vec![target("a"), target("b")]);
        let reverse = issue_grant(vec![target("b"), target("a")]);
        let changed_provider = issue_grant(vec![
            EmergencyChainTargetIdentity::new("provider-x", "endpoint-a", "key-a").unwrap(),
            target("b"),
        ]);
        let changed_endpoint = issue_grant(vec![
            EmergencyChainTargetIdentity::new("provider-a", "endpoint-x", "key-a").unwrap(),
            target("b"),
        ]);
        let changed_key = issue_grant(vec![
            EmergencyChainTargetIdentity::new("provider-a", "endpoint-a", "key-x").unwrap(),
            target("b"),
        ]);
        assert_eq!(
            forward.chain_hash().as_str(),
            "8f0589f900456a262847902c3a103651801510fbb32283368ba22e321ba8f350"
        );
        assert_ne!(forward.chain_hash(), reverse.chain_hash());
        assert_ne!(forward.chain_hash(), changed_provider.chain_hash());
        assert_ne!(forward.chain_hash(), changed_endpoint.chain_hash());
        assert_ne!(forward.chain_hash(), changed_key.chain_hash());
        assert!(EmergencyChainHash::parse(forward.chain_hash().as_str()).is_ok());
    }

    #[test]
    fn normal_route_requires_server_activation_and_never_consumes_a_grant() {
        let activation = authority().normal_routing_activation();
        let mut grant = issue_grant(vec![target("a")]);
        assert!(matches!(
            evaluate_emergency_chain_gate(
                EmergencyChainGateRequest::NormalRouting(&activation),
                Some(&mut grant),
            ),
            Ok(EmergencyChainGateDecision::NormalRouting)
        ));
        assert!(authorize(&mut grant, &target("a"), ISSUED_AT).is_ok());
    }

    #[test]
    fn gate_rejects_unknown_principal_operation_scope_and_hash_drift() {
        let requested_target = target("a");
        let mut grant = issue_grant(vec![requested_target.clone()]);
        let valid = context(&grant, ISSUED_AT);
        assert_eq!(
            evaluate_emergency_chain_gate(
                EmergencyChainGateRequest::Emergency(EmergencyChainUseRequest {
                    trusted_context: &valid,
                    requested_target: &requested_target,
                }),
                None,
            )
            .unwrap_err(),
            EmergencyChainGateError::UnknownGrant
        );

        let cases = [
            (
                principal("operator:bob"),
                operation("responses.create"),
                request_scope("request-000001", 'a', "nonce-000001"),
                grant.chain_hash().clone(),
                EmergencyChainGateError::PrincipalDenied,
            ),
            (
                grant.principal().clone(),
                operation("embeddings.create"),
                request_scope("request-000001", 'a', "nonce-000001"),
                grant.chain_hash().clone(),
                EmergencyChainGateError::OperationDenied,
            ),
            (
                grant.principal().clone(),
                operation("responses.create"),
                request_scope("request-OTHER", 'a', "nonce-000001"),
                grant.chain_hash().clone(),
                EmergencyChainGateError::RequestScopeDenied,
            ),
            (
                grant.principal().clone(),
                operation("responses.create"),
                request_scope("request-000001", 'a', "nonce-000001"),
                EmergencyChainHash::parse("0".repeat(64)).unwrap(),
                EmergencyChainGateError::ChainHashMismatch,
            ),
        ];
        for (principal, operation, request_scope, chain_hash, expected) in cases {
            let context = context_with(
                &grant,
                chain_hash,
                principal,
                operation,
                request_scope,
                ISSUED_AT,
            );
            assert_eq!(
                evaluate_emergency_chain_gate(
                    EmergencyChainGateRequest::Emergency(EmergencyChainUseRequest {
                        trusted_context: &context,
                        requested_target: &requested_target,
                    }),
                    Some(&mut grant),
                )
                .unwrap_err(),
                expected
            );
        }
    }

    #[test]
    fn request_scope_binds_request_fingerprint_and_nonce() {
        let requested_target = target("a");
        let mut grant = issue_grant(vec![requested_target.clone()]);
        for scope in [
            request_scope("request-OTHER", 'a', "nonce-000001"),
            request_scope("request-000001", 'b', "nonce-000001"),
            request_scope("request-000001", 'a', "nonce-OTHER"),
        ] {
            let context = context_with(
                &grant,
                grant.chain_hash().clone(),
                grant.principal().clone(),
                operation("responses.create"),
                scope,
                ISSUED_AT,
            );
            assert_eq!(
                evaluate_emergency_chain_gate(
                    EmergencyChainGateRequest::Emergency(EmergencyChainUseRequest {
                        trusted_context: &context,
                        requested_target: &requested_target,
                    }),
                    Some(&mut grant),
                )
                .unwrap_err(),
                EmergencyChainGateError::RequestScopeDenied
            );
        }
    }

    #[test]
    fn server_gate_time_uses_half_open_validity_and_revocation_boundaries() {
        let target_a = target("a");
        let mut grant = issue_grant(vec![target_a.clone()]);
        assert_eq!(
            authorize(&mut grant, &target_a, ISSUED_AT - 1).unwrap_err(),
            EmergencyChainGateError::NotYetValid
        );
        assert!(authorize(&mut grant, &target_a, ISSUED_AT).is_ok());

        let mut expired = issue_grant(vec![target_a.clone()]);
        let expires_at = expired.expires_at_unix_secs();
        assert_eq!(
            authorize(&mut expired, &target_a, expires_at).unwrap_err(),
            EmergencyChainGateError::Expired
        );

        let mut revoked = issue_grant(vec![target_a.clone()]);
        revoked.revoke(ISSUED_AT + 10).unwrap();
        assert!(authorize(&mut revoked, &target_a, ISSUED_AT + 9).is_ok());
        let mut revoked_at_boundary = issue_grant(vec![target_a.clone()]);
        revoked_at_boundary.revoke(ISSUED_AT + 10).unwrap();
        assert_eq!(
            authorize(&mut revoked_at_boundary, &target_a, ISSUED_AT + 10).unwrap_err(),
            EmergencyChainGateError::Revoked
        );
    }

    #[test]
    fn revoke_is_monotonic_tightening_and_idempotent_after_effective_time() {
        let mut grant = issue_grant(vec![target("a")]);
        assert_eq!(
            grant.revoke(ISSUED_AT - 1),
            Err(EmergencyChainRevokeError::BeforeIssue)
        );
        assert_eq!(
            grant.revoke(ISSUED_AT + 20),
            Ok(EmergencyChainRevokeOutcome::Revoked)
        );
        assert_eq!(
            grant.revoke(ISSUED_AT + 10),
            Ok(EmergencyChainRevokeOutcome::EffectiveTimeTightened)
        );
        assert_eq!(
            grant.revoke(ISSUED_AT + 10),
            Ok(EmergencyChainRevokeOutcome::AlreadyRevoked)
        );
        assert_eq!(
            grant.revoke(ISSUED_AT + 30),
            Ok(EmergencyChainRevokeOutcome::AlreadyRevoked)
        );
        assert_eq!(grant.revoked_at_unix_secs(), Some(ISSUED_AT + 10));

        let mut expired = issue_grant(vec![target("a")]);
        let expires_at = expired.expires_at_unix_secs();
        assert_eq!(
            expired.revoke(expires_at),
            Ok(EmergencyChainRevokeOutcome::Revoked)
        );
        assert_eq!(
            expired.revoke(expires_at + 1),
            Ok(EmergencyChainRevokeOutcome::AlreadyRevoked)
        );
    }

    #[test]
    fn internal_cursor_prevents_skip_replay_parallel_permits_and_cross_request_reuse() {
        let target_a = target("a");
        let target_b = target("b");
        let mut grant = issue_grant(vec![target_a.clone(), target_b.clone()]);

        assert_eq!(
            authorize(&mut grant, &target_b, ISSUED_AT).unwrap_err(),
            EmergencyChainGateError::TargetOutOfOrder
        );
        let permit_a = permit(authorize(&mut grant, &target_a, ISSUED_AT).unwrap());
        assert_eq!(permit_a.chain_position, 0);
        assert_eq!(
            authorize(&mut grant, &target_a, ISSUED_AT).unwrap_err(),
            EmergencyChainGateError::AttemptAlreadyOutstanding
        );
        let proof_a =
            test_verified_safe_skip_proof(&permit_a, "ledger-entry-a", instant(ISSUED_AT + 1));
        let context_a = context(&grant, ISSUED_AT + 1);
        assert_eq!(
            grant
                .apply_authoritative_safe_skip(permit_a, proof_a, &context_a)
                .unwrap(),
            EmergencyChainSessionProgress::ReadyForNextTarget { next_position: 1 }
        );
        assert_eq!(
            authorize(&mut grant, &target_a, ISSUED_AT + 1).unwrap_err(),
            EmergencyChainGateError::TargetOutOfOrder
        );
        let permit_b = permit(authorize(&mut grant, &target_b, ISSUED_AT + 1).unwrap());
        assert_eq!(permit_b.chain_position, 1);
        assert_eq!(
            authorize(&mut grant, &target_b, ISSUED_AT + 2).unwrap_err(),
            EmergencyChainGateError::AttemptAlreadyOutstanding
        );
    }

    #[test]
    fn reserved_permit_has_no_domain_send_or_completion_path() {
        let target_a = target("a");
        let mut grant = issue_grant(vec![target_a.clone()]);
        let reserved = permit(authorize(&mut grant, &target_a, ISSUED_AT).unwrap());
        assert_eq!(reserved.chain_position, 0);
        assert_eq!(
            authorize(&mut grant, &target_a, ISSUED_AT + 1).unwrap_err(),
            EmergencyChainGateError::AttemptAlreadyOutstanding
        );
    }

    #[test]
    fn safe_skip_requires_authorization_then_ledger_observation_then_gate_order() {
        let target_a = target("a");
        let mut grant = issue_grant(vec![target_a.clone(), target("b")]);
        let future_permit = permit(authorize(&mut grant, &target_a, ISSUED_AT).unwrap());
        let proof = test_verified_safe_skip_proof(
            &future_permit,
            "ledger-entry-future",
            instant(ISSUED_AT + 2),
        );
        let future_context = context(&grant, ISSUED_AT + 1);
        assert_eq!(
            grant.apply_authoritative_safe_skip(future_permit, proof, &future_context),
            Err(EmergencyChainSafeSkipError::ProofMismatch)
        );

        let mut before_authorization = issue_grant(vec![target_a.clone(), target("b")]);
        let permit = permit(authorize(&mut before_authorization, &target_a, ISSUED_AT).unwrap());
        let proof =
            test_verified_safe_skip_proof(&permit, "ledger-entry-before", instant(ISSUED_AT - 1));
        let context = context(&before_authorization, ISSUED_AT + 1);
        assert_eq!(
            before_authorization.apply_authoritative_safe_skip(permit, proof, &context),
            Err(EmergencyChainSafeSkipError::ProofMismatch)
        );
    }

    #[test]
    fn production_ledger_authority_refuses_forged_entry_ids() {
        let target_a = target("a");
        let mut grant = issue_grant(vec![target_a.clone()]);
        let permit = permit(authorize(&mut grant, &target_a, ISSUED_AT).unwrap());

        assert_eq!(
            ledger_authority()
                .authoritative_safe_skip_proof(&permit, "forged-ledger-entry", instant(ISSUED_AT))
                .unwrap_err(),
            EmergencyChainLedgerError::AuthoritativeLedgerUnavailable
        );
        assert_eq!(
            authorize(&mut grant, &target_a, ISSUED_AT).unwrap_err(),
            EmergencyChainGateError::AttemptAlreadyOutstanding
        );
    }

    #[test]
    fn safe_skip_rejects_wrong_slot_or_scope_and_expired_context() {
        let target_a = target("a");
        let mut wrong_slot_grant = issue_grant(vec![target_a.clone(), target("b")]);
        let wrong_slot_permit =
            permit(authorize(&mut wrong_slot_grant, &target_a, ISSUED_AT).unwrap());
        let mut wrong_slot_proof = test_verified_safe_skip_proof(
            &wrong_slot_permit,
            "ledger-entry-wrong-slot",
            instant(ISSUED_AT),
        );
        wrong_slot_proof.chain_position = 1;
        assert_eq!(
            wrong_slot_grant.apply_authoritative_safe_skip(
                wrong_slot_permit,
                wrong_slot_proof,
                &context(&wrong_slot_grant, ISSUED_AT),
            ),
            Err(EmergencyChainSafeSkipError::ProofMismatch)
        );

        let mut wrong_scope_grant = issue_grant(vec![target_a.clone()]);
        let wrong_scope_permit =
            permit(authorize(&mut wrong_scope_grant, &target_a, ISSUED_AT).unwrap());
        let mut wrong_scope_proof = test_verified_safe_skip_proof(
            &wrong_scope_permit,
            "ledger-entry-wrong-scope",
            instant(ISSUED_AT),
        );
        wrong_scope_proof.request_scope = request_scope("other-request", 'b', "other-nonce");
        assert_eq!(
            wrong_scope_grant.apply_authoritative_safe_skip(
                wrong_scope_permit,
                wrong_scope_proof,
                &context(&wrong_scope_grant, ISSUED_AT),
            ),
            Err(EmergencyChainSafeSkipError::ProofMismatch)
        );

        let mut expired_grant = issue_grant(vec![target_a.clone()]);
        let expired_permit = permit(authorize(&mut expired_grant, &target_a, ISSUED_AT).unwrap());
        let expired_proof = test_verified_safe_skip_proof(
            &expired_permit,
            "ledger-entry-expired",
            instant(ISSUED_AT),
        );
        let expires_at = expired_grant.expires_at_unix_secs();
        assert_eq!(
            expired_grant.apply_authoritative_safe_skip(
                expired_permit,
                expired_proof,
                &context(&expired_grant, expires_at),
            ),
            Err(EmergencyChainSafeSkipError::Gate(
                EmergencyChainGateError::Expired
            ))
        );
    }

    #[test]
    fn authoritative_skip_of_last_target_exhausts_chain_and_cannot_fallback() {
        let target_a = target("a");
        let target_outside = target("outside");
        let mut grant = issue_grant(vec![target_a.clone()]);
        assert_eq!(
            authorize(&mut grant, &target_outside, ISSUED_AT).unwrap_err(),
            EmergencyChainGateError::TargetOutsideChain
        );
        let permit_a = permit(authorize(&mut grant, &target_a, ISSUED_AT).unwrap());
        let proof_a =
            test_verified_safe_skip_proof(&permit_a, "ledger-entry-last", instant(ISSUED_AT + 1));
        let context_a = context(&grant, ISSUED_AT + 1);
        assert_eq!(
            grant
                .apply_authoritative_safe_skip(permit_a, proof_a, &context_a)
                .unwrap(),
            EmergencyChainSessionProgress::Exhausted
        );
        assert_eq!(
            authorize(&mut grant, &target_a, ISSUED_AT + 1).unwrap_err(),
            EmergencyChainGateError::ChainExhausted
        );
    }

    #[test]
    fn missing_first_materialization_does_not_compress_chain_or_unlock_second_target() {
        let target_a = target("a");
        let target_b = target("b");
        let mut grant = issue_grant(vec![target_a.clone(), target_b.clone()]);
        let materialized = vec![candidate(&target_b)];
        let slots = emergency_chain_candidate_order(&grant, &materialized).unwrap();

        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].chain_position, 0);
        assert_eq!(slots[0].candidate_index, None);
        assert_eq!(slots[1].chain_position, 1);
        assert_eq!(slots[1].candidate_index, Some(0));
        assert_eq!(
            authorize(&mut grant, &target_b, ISSUED_AT).unwrap_err(),
            EmergencyChainGateError::TargetOutOfOrder
        );

        let missing_a_permit = permit(authorize(&mut grant, &target_a, ISSUED_AT).unwrap());
        let proof = test_verified_safe_skip_proof(
            &missing_a_permit,
            "ledger-entry-000001",
            instant(ISSUED_AT + 1),
        );
        let context = context(&grant, ISSUED_AT + 1);
        assert_eq!(
            grant
                .apply_authoritative_safe_skip(missing_a_permit, proof, &context,)
                .unwrap(),
            EmergencyChainSessionProgress::ReadyForNextTarget { next_position: 1 }
        );
        assert!(authorize(&mut grant, &target_b, ISSUED_AT + 1).is_ok());
    }

    #[test]
    fn missing_first_middle_or_final_target_preserves_every_original_slot() {
        let targets = [target("a"), target("b"), target("c")];
        let grant = issue_grant(targets.to_vec());

        for missing_position in 0..targets.len() {
            let candidates = targets
                .iter()
                .enumerate()
                .filter(|(position, _)| *position != missing_position)
                .map(|(_, target)| candidate(target))
                .collect::<Vec<_>>();
            let slots = emergency_chain_candidate_order(&grant, &candidates).unwrap();
            assert_eq!(slots.len(), targets.len());
            for (position, slot) in slots.iter().enumerate() {
                assert_eq!(slot.chain_position, position);
                assert_eq!(slot.candidate_index.is_none(), position == missing_position);
            }
        }
    }

    #[test]
    fn trusted_context_is_the_only_source_for_auth_scope_and_gate_time() {
        let target_a = target("a");
        let mut grant = issue_grant(vec![target_a.clone()]);
        let server_context = context(&grant, ISSUED_AT);
        let request = EmergencyChainUseRequest {
            trusted_context: &server_context,
            requested_target: &target_a,
        };
        assert!(matches!(
            evaluate_emergency_chain_gate(
                EmergencyChainGateRequest::Emergency(request),
                Some(&mut grant),
            ),
            Ok(EmergencyChainGateDecision::EmergencyTargetAuthorized(_))
        ));
    }

    #[test]
    fn emergency_order_ignores_seed_priority_health_and_excludes_outside_targets() {
        let identities = [target("a"), target("b"), target("c")];
        let grant = issue_grant(identities.to_vec());
        let outside = target("outside");

        for seed in 0..128usize {
            let mut candidates = identities
                .iter()
                .chain(std::iter::once(&outside))
                .map(candidate)
                .collect::<Vec<_>>();
            let candidate_count = candidates.len();
            candidates.rotate_left(seed % candidate_count);
            for (index, candidate) in candidates.iter_mut().enumerate() {
                candidate.provider_priority = (seed as i32) - (index as i32 * 17);
                candidate.key_internal_priority = (index as i32 * 31) - seed as i32;
                candidate.key_global_priority_for_format = Some((seed ^ index) as i32);
                candidate.health_bucket = Some(match (seed + index) % 3 {
                    0 => ProviderKeyHealthBucket::Low,
                    1 => ProviderKeyHealthBucket::Degraded,
                    _ => ProviderKeyHealthBucket::Healthy,
                });
                candidate.health_score = ((seed + index) % 101) as f64 / 100.0;
                candidate.affinity_hash = Some((seed as u64).wrapping_mul(31) + index as u64);
            }

            let ordered = emergency_chain_candidate_order(&grant, &candidates).unwrap();
            let ordered_ids = ordered
                .iter()
                .map(|entry| {
                    candidates[entry
                        .candidate_index
                        .expect("chain target should materialize")]
                    .provider_id
                    .as_str()
                })
                .collect::<Vec<_>>();
            assert_eq!(ordered_ids, ["provider-a", "provider-b", "provider-c"]);
            assert!(!ordered_ids.contains(&"provider-outside"));
        }

        let outside_only = emergency_chain_candidate_order(&grant, &[candidate(&outside)]).unwrap();
        assert_eq!(outside_only.len(), 3);
        assert!(outside_only
            .iter()
            .all(|slot| slot.candidate_index.is_none()));
    }

    #[test]
    fn duplicate_candidate_identity_fails_closed() {
        let target_a = target("a");
        let grant = issue_grant(vec![target_a.clone()]);
        assert_eq!(
            emergency_chain_candidate_order(
                &grant,
                &[
                    candidate(&target("outside")),
                    candidate(&target_a),
                    candidate(&target_a),
                ],
            ),
            Err(EmergencyChainCandidateOrderError::AmbiguousTarget)
        );
    }
}
