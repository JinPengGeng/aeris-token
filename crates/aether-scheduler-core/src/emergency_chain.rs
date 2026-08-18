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
    Consumed,
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

    pub fn complete_attempt(
        &mut self,
        permit: EmergencyChainAttemptPermit,
        completion: EmergencyChainAttemptCompletion,
        completed_at: ServerEmergencyChainInstant,
    ) -> Result<EmergencyChainSessionProgress, EmergencyChainAttemptCompletionError> {
        let EmergencyChainSessionState::Outstanding {
            position,
            permit_serial,
        } = self.session_state
        else {
            return Err(EmergencyChainAttemptCompletionError::NoOutstandingPermit);
        };
        if permit.grant_id != self.id
            || permit.chain_hash != self.chain_hash
            || permit.request_scope != self.request_scope
            || permit.chain_position != position
            || permit.permit_serial != permit_serial
            || self.targets.get(position) != Some(&permit.target)
        {
            return Err(EmergencyChainAttemptCompletionError::PermitMismatch);
        }
        if completed_at.unix_secs < permit.authorized_at_unix_secs {
            return Err(EmergencyChainAttemptCompletionError::CompletionBeforeAuthorization);
        }

        match completion {
            EmergencyChainAttemptCompletion::RetryableFailure
            | EmergencyChainAttemptCompletion::AuthoritativeSafeSkip => {
                let next_position = position + 1;
                if next_position >= self.targets.len() {
                    self.session_state = EmergencyChainSessionState::Exhausted;
                    Ok(EmergencyChainSessionProgress::Exhausted)
                } else {
                    self.session_state = EmergencyChainSessionState::Ready { next_position };
                    Ok(EmergencyChainSessionProgress::ReadyForNextTarget { next_position })
                }
            }
            EmergencyChainAttemptCompletion::TerminalSuccess
            | EmergencyChainAttemptCompletion::TerminalFailure => {
                self.session_state = EmergencyChainSessionState::Consumed;
                Ok(EmergencyChainSessionProgress::Consumed)
            }
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

/// Server-only route selection proof. Do not construct this from request
/// headers, body fields, query parameters, or other client-controlled input.
#[derive(Debug)]
pub struct ServerNormalRoutingActivation {
    _private: (),
}

impl ServerNormalRoutingActivation {
    pub fn from_server_route_selection() -> Self {
        Self { _private: () }
    }
}

/// A timestamp captured from the server clock. Client-provided timestamps must
/// never be passed to this constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerEmergencyChainInstant {
    unix_secs: i64,
}

impl ServerEmergencyChainInstant {
    pub fn from_server_clock(unix_secs: i64) -> Self {
        Self { unix_secs }
    }

    pub fn unix_secs(self) -> i64 {
        self.unix_secs
    }
}

/// Trusted context assembled from authenticated server identity, the
/// server-selected operation and grant record, and the server clock. It has no
/// serde deserialization contract and must not be mapped from client input.
#[derive(Debug)]
pub struct TrustedEmergencyChainContext {
    grant_id: EmergencyChainGrantId,
    chain_hash: EmergencyChainHash,
    principal: EmergencyChainPrincipal,
    operation: EmergencyChainOperation,
    request_scope: EmergencyChainRequestScope,
    gate_at: ServerEmergencyChainInstant,
}

impl TrustedEmergencyChainContext {
    pub fn from_authenticated_server_context(
        grant_id: EmergencyChainGrantId,
        chain_hash: EmergencyChainHash,
        principal: EmergencyChainPrincipal,
        operation: EmergencyChainOperation,
        request_scope: EmergencyChainRequestScope,
        gate_at: ServerEmergencyChainInstant,
    ) -> Self {
        Self {
            grant_id,
            chain_hash,
            principal,
            operation,
            request_scope,
            gate_at,
        }
    }
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
    GrantConsumed,
    PermitSerialExhausted,
}

#[derive(Debug)]
pub struct EmergencyChainAttemptPermit {
    grant_id: EmergencyChainGrantId,
    chain_hash: EmergencyChainHash,
    request_scope: EmergencyChainRequestScope,
    chain_position: usize,
    target: EmergencyChainTargetIdentity,
    permit_serial: u64,
    authorized_at_unix_secs: i64,
}

impl EmergencyChainAttemptPermit {
    pub fn grant_id(&self) -> &EmergencyChainGrantId {
        &self.grant_id
    }

    pub fn chain_hash(&self) -> &EmergencyChainHash {
        &self.chain_hash
    }

    pub fn request_scope(&self) -> &EmergencyChainRequestScope {
        &self.request_scope
    }

    pub fn chain_position(&self) -> usize {
        self.chain_position
    }

    pub fn target(&self) -> &EmergencyChainTargetIdentity {
        &self.target
    }

    pub fn authorized_at_unix_secs(&self) -> i64 {
        self.authorized_at_unix_secs
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainAttemptCompletion {
    RetryableFailure,
    /// The trusted integration proved from authoritative materialization state
    /// that this exact slot cannot be attempted safely. Client input alone must
    /// never produce this outcome.
    AuthoritativeSafeSkip,
    TerminalSuccess,
    TerminalFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainSessionProgress {
    ReadyForNextTarget { next_position: usize },
    Exhausted,
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainAttemptCompletionError {
    NoOutstandingPermit,
    PermitMismatch,
    CompletionBeforeAuthorization,
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
    let grant = match loaded_grant {
        Some(grant) if grant.id == context.grant_id => grant,
        _ => return Err(EmergencyChainGateError::UnknownGrant),
    };

    if grant.principal != context.principal {
        return Err(EmergencyChainGateError::PrincipalDenied);
    }
    if grant.operations.binary_search(&context.operation).is_err() {
        return Err(EmergencyChainGateError::OperationDenied);
    }
    if grant.request_scope != context.request_scope {
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
    if grant.chain_hash != context.chain_hash {
        return Err(EmergencyChainGateError::ChainHashMismatch);
    }

    let next_position = match grant.session_state {
        EmergencyChainSessionState::Ready { next_position } => next_position,
        EmergencyChainSessionState::Outstanding { .. } => {
            return Err(EmergencyChainGateError::AttemptAlreadyOutstanding)
        }
        EmergencyChainSessionState::Exhausted => {
            return Err(EmergencyChainGateError::ChainExhausted)
        }
        EmergencyChainSessionState::Consumed => return Err(EmergencyChainGateError::GrantConsumed),
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
        TrustedEmergencyChainContext::from_authenticated_server_context(
            grant.id().clone(),
            grant.chain_hash().clone(),
            grant.principal().clone(),
            operation("responses.create"),
            grant.request_scope().clone(),
            ServerEmergencyChainInstant::from_server_clock(gate_at_unix_secs),
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
        assert_eq!(
            forward.chain_hash().as_str(),
            "8f0589f900456a262847902c3a103651801510fbb32283368ba22e321ba8f350"
        );
        assert_ne!(forward.chain_hash(), reverse.chain_hash());
    }

    #[test]
    fn normal_route_requires_server_activation_and_never_consumes_a_grant() {
        let activation = ServerNormalRoutingActivation::from_server_route_selection();
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
                grant.request_scope().clone(),
                grant.chain_hash().clone(),
                EmergencyChainGateError::PrincipalDenied,
            ),
            (
                grant.principal().clone(),
                operation("embeddings.create"),
                grant.request_scope().clone(),
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
                grant.request_scope().clone(),
                EmergencyChainHash::parse("0".repeat(64)).unwrap(),
                EmergencyChainGateError::ChainHashMismatch,
            ),
        ];
        for (principal, operation, request_scope, chain_hash, expected) in cases {
            let context = TrustedEmergencyChainContext::from_authenticated_server_context(
                grant.id().clone(),
                chain_hash,
                principal,
                operation,
                request_scope,
                ServerEmergencyChainInstant::from_server_clock(ISSUED_AT),
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
            let context = TrustedEmergencyChainContext::from_authenticated_server_context(
                grant.id().clone(),
                grant.chain_hash().clone(),
                grant.principal().clone(),
                operation("responses.create"),
                scope,
                ServerEmergencyChainInstant::from_server_clock(ISSUED_AT),
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
        assert_eq!(permit_a.chain_position(), 0);
        assert_eq!(
            authorize(&mut grant, &target_a, ISSUED_AT).unwrap_err(),
            EmergencyChainGateError::AttemptAlreadyOutstanding
        );
        assert_eq!(
            grant
                .complete_attempt(
                    permit_a,
                    EmergencyChainAttemptCompletion::RetryableFailure,
                    ServerEmergencyChainInstant::from_server_clock(ISSUED_AT + 1),
                )
                .unwrap(),
            EmergencyChainSessionProgress::ReadyForNextTarget { next_position: 1 }
        );
        assert_eq!(
            authorize(&mut grant, &target_a, ISSUED_AT + 1).unwrap_err(),
            EmergencyChainGateError::TargetOutOfOrder
        );
        let permit_b = permit(authorize(&mut grant, &target_b, ISSUED_AT + 1).unwrap());
        assert_eq!(permit_b.chain_position(), 1);
        grant
            .complete_attempt(
                permit_b,
                EmergencyChainAttemptCompletion::TerminalSuccess,
                ServerEmergencyChainInstant::from_server_clock(ISSUED_AT + 2),
            )
            .unwrap();
        assert_eq!(
            authorize(&mut grant, &target_b, ISSUED_AT + 2).unwrap_err(),
            EmergencyChainGateError::GrantConsumed
        );
    }

    #[test]
    fn retrying_last_target_exhausts_chain_and_cannot_fallback() {
        let target_a = target("a");
        let target_outside = target("outside");
        let mut grant = issue_grant(vec![target_a.clone()]);
        assert_eq!(
            authorize(&mut grant, &target_outside, ISSUED_AT).unwrap_err(),
            EmergencyChainGateError::TargetOutsideChain
        );
        let permit_a = permit(authorize(&mut grant, &target_a, ISSUED_AT).unwrap());
        assert_eq!(
            grant
                .complete_attempt(
                    permit_a,
                    EmergencyChainAttemptCompletion::RetryableFailure,
                    ServerEmergencyChainInstant::from_server_clock(ISSUED_AT + 1),
                )
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
        assert_eq!(
            grant
                .complete_attempt(
                    missing_a_permit,
                    EmergencyChainAttemptCompletion::AuthoritativeSafeSkip,
                    ServerEmergencyChainInstant::from_server_clock(ISSUED_AT + 1),
                )
                .unwrap(),
            EmergencyChainSessionProgress::ReadyForNextTarget { next_position: 1 }
        );
        assert!(authorize(&mut grant, &target_b, ISSUED_AT + 1).is_ok());
    }

    #[test]
    fn trusted_context_is_the_only_source_for_auth_scope_and_gate_time() {
        let target_a = target("a");
        let mut grant = issue_grant(vec![target_a.clone()]);
        let server_context = TrustedEmergencyChainContext::from_authenticated_server_context(
            grant.id().clone(),
            grant.chain_hash().clone(),
            grant.principal().clone(),
            operation("responses.create"),
            grant.request_scope().clone(),
            ServerEmergencyChainInstant::from_server_clock(ISSUED_AT),
        );
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
            emergency_chain_candidate_order(&grant, &[candidate(&target_a), candidate(&target_a)]),
            Err(EmergencyChainCandidateOrderError::AmbiguousTarget)
        );
    }
}
