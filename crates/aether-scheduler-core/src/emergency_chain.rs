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
        if value.len() != 64
            || !value
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return Err(EmergencyChainGrantBuildError::InvalidChainHash);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
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
}

#[derive(Debug)]
pub struct EmergencyChainGrant {
    id: EmergencyChainGrantId,
    principal: EmergencyChainPrincipal,
    operations: Vec<EmergencyChainOperation>,
    targets: Vec<EmergencyChainTargetIdentity>,
    chain_hash: EmergencyChainHash,
    issued_at_unix_secs: i64,
    expires_at_unix_secs: i64,
    revoked_at_unix_secs: Option<i64>,
}

pub struct IssueEmergencyChainGrant {
    pub id: EmergencyChainGrantId,
    pub principal: EmergencyChainPrincipal,
    pub operations: Vec<EmergencyChainOperation>,
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
            targets: input.targets,
            chain_hash,
            issued_at_unix_secs: input.issued_at_unix_secs,
            expires_at_unix_secs: input.expires_at_unix_secs,
            revoked_at_unix_secs: None,
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
        if self.revoked_at_unix_secs.is_some() {
            return Ok(EmergencyChainRevokeOutcome::AlreadyRevoked);
        }
        self.revoked_at_unix_secs = Some(revoked_at_unix_secs);
        Ok(EmergencyChainRevokeOutcome::Revoked)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainRevokeOutcome {
    Revoked,
    AlreadyRevoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainRevokeError {
    BeforeIssue,
}

pub enum EmergencyChainGateRequest<'a> {
    NormalRouting,
    Emergency(EmergencyChainUseRequest<'a>),
}

pub struct EmergencyChainUseRequest<'a> {
    pub grant_id: &'a EmergencyChainGrantId,
    pub principal: &'a EmergencyChainPrincipal,
    pub operation: &'a EmergencyChainOperation,
    pub chain_hash: &'a EmergencyChainHash,
    pub attempted_targets: &'a [EmergencyChainTargetIdentity],
    pub requested_target: &'a EmergencyChainTargetIdentity,
    /// The single instant at which every grant predicate is evaluated.
    pub gate_at_unix_secs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmergencyChainGateDecision {
    NormalRouting,
    EmergencyTargetAuthorized {
        grant_id: EmergencyChainGrantId,
        chain_hash: EmergencyChainHash,
        chain_position: usize,
        target: EmergencyChainTargetIdentity,
        linearized_at_unix_secs: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainGateError {
    UnknownGrant,
    PrincipalDenied,
    OperationDenied,
    NotYetValid,
    Expired,
    Revoked,
    ChainHashMismatch,
    AttemptedTargetOutsideChain,
    AttemptSequenceOutOfOrder,
    TargetOutsideChain,
    TargetOutOfOrder,
    ChainExhausted,
}

/// Evaluates the emergency gate at exactly `gate_at_unix_secs`.
///
/// Validity is `[issued_at, expires_at)`. A revocation is effective when
/// `revoked_at <= gate_at`. The caller must perform a new strong read and gate
/// evaluation at the eventual send boundary; an earlier decision is not a
/// durable authorization lease.
pub fn evaluate_emergency_chain_gate(
    request: EmergencyChainGateRequest<'_>,
    loaded_grant: Option<&EmergencyChainGrant>,
) -> Result<EmergencyChainGateDecision, EmergencyChainGateError> {
    let EmergencyChainGateRequest::Emergency(request) = request else {
        return Ok(EmergencyChainGateDecision::NormalRouting);
    };
    let grant = loaded_grant
        .filter(|grant| grant.id == *request.grant_id)
        .ok_or(EmergencyChainGateError::UnknownGrant)?;

    if grant.principal != *request.principal {
        return Err(EmergencyChainGateError::PrincipalDenied);
    }
    if grant.operations.binary_search(request.operation).is_err() {
        return Err(EmergencyChainGateError::OperationDenied);
    }
    if request.gate_at_unix_secs < grant.issued_at_unix_secs {
        return Err(EmergencyChainGateError::NotYetValid);
    }
    if request.gate_at_unix_secs >= grant.expires_at_unix_secs {
        return Err(EmergencyChainGateError::Expired);
    }
    if grant
        .revoked_at_unix_secs
        .is_some_and(|revoked_at| revoked_at <= request.gate_at_unix_secs)
    {
        return Err(EmergencyChainGateError::Revoked);
    }
    if grant.chain_hash != *request.chain_hash {
        return Err(EmergencyChainGateError::ChainHashMismatch);
    }

    for (position, attempted_target) in request.attempted_targets.iter().enumerate() {
        if !grant.targets.contains(attempted_target) {
            return Err(EmergencyChainGateError::AttemptedTargetOutsideChain);
        }
        if grant.targets.get(position) != Some(attempted_target) {
            return Err(EmergencyChainGateError::AttemptSequenceOutOfOrder);
        }
    }

    let chain_position = request.attempted_targets.len();
    let Some(expected_target) = grant.targets.get(chain_position) else {
        return Err(EmergencyChainGateError::ChainExhausted);
    };
    if !grant.targets.contains(request.requested_target) {
        return Err(EmergencyChainGateError::TargetOutsideChain);
    }
    if expected_target != request.requested_target {
        return Err(EmergencyChainGateError::TargetOutOfOrder);
    }

    Ok(EmergencyChainGateDecision::EmergencyTargetAuthorized {
        grant_id: grant.id.clone(),
        chain_hash: grant.chain_hash.clone(),
        chain_position,
        target: expected_target.clone(),
        linearized_at_unix_secs: request.gate_at_unix_secs,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmergencyChainCandidateMatch {
    pub chain_position: usize,
    pub candidate_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyChainCandidateOrderError {
    AmbiguousTarget,
}

/// Returns only candidates named by the grant, in immutable chain order.
/// Missing targets are omitted; duplicate candidate identities fail closed.
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
        if let Some((candidate_index, _)) = first {
            ordered.push(EmergencyChainCandidateMatch {
                chain_position,
                candidate_index,
            });
        }
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
    let digest = hasher.finalize();
    let value = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    EmergencyChainHash(value)
}

fn hash_length_prefixed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
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
    use std::sync::OnceLock;

    const ISSUED_AT: i64 = 1_000_000;

    fn grant_id(value: &str) -> EmergencyChainGrantId {
        EmergencyChainGrantId::new(value).expect("valid grant id")
    }

    fn principal(value: &str) -> EmergencyChainPrincipal {
        EmergencyChainPrincipal::new(value).expect("valid principal")
    }

    fn operation(value: &str) -> EmergencyChainOperation {
        EmergencyChainOperation::new(value).expect("valid operation")
    }

    fn responses_operation() -> &'static EmergencyChainOperation {
        static OPERATION: OnceLock<EmergencyChainOperation> = OnceLock::new();
        OPERATION.get_or_init(|| operation("responses.create"))
    }

    fn target(id: &str) -> EmergencyChainTargetIdentity {
        EmergencyChainTargetIdentity::new(
            format!("provider-{id}"),
            format!("endpoint-{id}"),
            format!("key-{id}"),
        )
        .expect("valid target")
    }

    fn issue_grant(targets: Vec<EmergencyChainTargetIdentity>) -> EmergencyChainGrant {
        EmergencyChainGrant::issue(IssueEmergencyChainGrant {
            id: grant_id("grant-opaque-000001"),
            principal: principal("operator:alice"),
            operations: vec![operation("responses.create"), operation("chat.create")],
            targets,
            issued_at_unix_secs: ISSUED_AT,
            expires_at_unix_secs: ISSUED_AT + 300,
        })
        .expect("grant should issue")
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

    fn emergency_request<'a>(
        grant: &'a EmergencyChainGrant,
        attempted_targets: &'a [EmergencyChainTargetIdentity],
        requested_target: &'a EmergencyChainTargetIdentity,
        gate_at_unix_secs: i64,
    ) -> EmergencyChainGateRequest<'a> {
        EmergencyChainGateRequest::Emergency(EmergencyChainUseRequest {
            grant_id: grant.id(),
            principal: grant.principal(),
            operation: responses_operation(),
            chain_hash: grant.chain_hash(),
            attempted_targets,
            requested_target,
            gate_at_unix_secs,
        })
    }

    #[test]
    fn issue_requires_scoped_bounded_non_duplicate_values() {
        let build = |operations, targets, expires_at_unix_secs| {
            EmergencyChainGrant::issue(IssueEmergencyChainGrant {
                id: grant_id("grant-opaque-000001"),
                principal: principal("operator:alice"),
                operations,
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
            build(vec![operation("chat.create")], vec![target("a")], ISSUED_AT,).unwrap_err(),
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
        assert!(EmergencyChainTargetIdentity::new("https://provider", "endpoint", "key").is_err());
    }

    #[test]
    fn chain_hash_binds_identity_and_strict_order() {
        let forward = issue_grant(vec![target("a"), target("b")]);
        let reverse = issue_grant(vec![target("b"), target("a")]);
        let changed = issue_grant(vec![target("a"), target("c")]);

        assert_ne!(forward.chain_hash(), reverse.chain_hash());
        assert_ne!(forward.chain_hash(), changed.chain_hash());
        assert_eq!(forward.targets(), &[target("a"), target("b")]);
    }

    #[test]
    fn normal_requests_are_unaffected_without_a_grant() {
        assert_eq!(
            evaluate_emergency_chain_gate(EmergencyChainGateRequest::NormalRouting, None),
            Ok(EmergencyChainGateDecision::NormalRouting)
        );
    }

    #[test]
    fn gate_rejects_unknown_and_mismatched_grants_without_disclosure() {
        let requested = issue_grant(vec![target("a")]);
        let loaded = EmergencyChainGrant::issue(IssueEmergencyChainGrant {
            id: grant_id("grant-opaque-000002"),
            principal: principal("operator:alice"),
            operations: vec![operation("responses.create")],
            targets: vec![target("a")],
            issued_at_unix_secs: ISSUED_AT,
            expires_at_unix_secs: ISSUED_AT + 300,
        })
        .unwrap();

        assert_eq!(
            evaluate_emergency_chain_gate(
                emergency_request(&requested, &[], &target("a"), ISSUED_AT),
                None,
            ),
            Err(EmergencyChainGateError::UnknownGrant)
        );
        assert_eq!(
            evaluate_emergency_chain_gate(
                emergency_request(&requested, &[], &target("a"), ISSUED_AT),
                Some(&loaded),
            ),
            Err(EmergencyChainGateError::UnknownGrant)
        );
    }

    #[test]
    fn gate_rejects_wrong_principal_and_operation() {
        let grant = issue_grant(vec![target("a")]);
        let wrong_principal = principal("operator:bob");
        let wrong_operation = operation("embeddings.create");
        let target_a = target("a");

        let request = EmergencyChainGateRequest::Emergency(EmergencyChainUseRequest {
            grant_id: grant.id(),
            principal: &wrong_principal,
            operation: &operation("responses.create"),
            chain_hash: grant.chain_hash(),
            attempted_targets: &[],
            requested_target: &target_a,
            gate_at_unix_secs: ISSUED_AT,
        });
        assert_eq!(
            evaluate_emergency_chain_gate(request, Some(&grant)),
            Err(EmergencyChainGateError::PrincipalDenied)
        );

        let request = EmergencyChainGateRequest::Emergency(EmergencyChainUseRequest {
            grant_id: grant.id(),
            principal: grant.principal(),
            operation: &wrong_operation,
            chain_hash: grant.chain_hash(),
            attempted_targets: &[],
            requested_target: &target_a,
            gate_at_unix_secs: ISSUED_AT,
        });
        assert_eq!(
            evaluate_emergency_chain_gate(request, Some(&grant)),
            Err(EmergencyChainGateError::OperationDenied)
        );
    }

    #[test]
    fn gate_uses_closed_expiry_and_revocation_boundaries() {
        let mut grant = issue_grant(vec![target("a")]);
        let target_a = target("a");
        assert_eq!(
            evaluate_emergency_chain_gate(
                emergency_request(&grant, &[], &target_a, ISSUED_AT - 1),
                Some(&grant),
            ),
            Err(EmergencyChainGateError::NotYetValid)
        );
        assert!(evaluate_emergency_chain_gate(
            emergency_request(&grant, &[], &target_a, ISSUED_AT),
            Some(&grant),
        )
        .is_ok());
        assert_eq!(
            evaluate_emergency_chain_gate(
                emergency_request(&grant, &[], &target_a, grant.expires_at_unix_secs()),
                Some(&grant),
            ),
            Err(EmergencyChainGateError::Expired)
        );

        assert_eq!(
            grant.revoke(ISSUED_AT + 10),
            Ok(EmergencyChainRevokeOutcome::Revoked)
        );
        assert!(evaluate_emergency_chain_gate(
            emergency_request(&grant, &[], &target_a, ISSUED_AT + 9),
            Some(&grant),
        )
        .is_ok());
        assert_eq!(
            evaluate_emergency_chain_gate(
                emergency_request(&grant, &[], &target_a, ISSUED_AT + 10),
                Some(&grant),
            ),
            Err(EmergencyChainGateError::Revoked)
        );
        assert_eq!(
            grant.revoke(ISSUED_AT + 20),
            Ok(EmergencyChainRevokeOutcome::AlreadyRevoked)
        );
        assert_eq!(grant.revoked_at_unix_secs(), Some(ISSUED_AT + 10));
    }

    #[test]
    fn gate_rejects_hash_drift_outside_targets_and_order_skips() {
        let grant = issue_grant(vec![target("a"), target("b"), target("c")]);
        let target_a = target("a");
        let target_b = target("b");
        let target_c = target("c");
        let target_x = target("x");
        let wrong_hash = EmergencyChainHash::parse("0".repeat(64)).unwrap();

        let request = EmergencyChainGateRequest::Emergency(EmergencyChainUseRequest {
            grant_id: grant.id(),
            principal: grant.principal(),
            operation: &operation("responses.create"),
            chain_hash: &wrong_hash,
            attempted_targets: &[],
            requested_target: &target_a,
            gate_at_unix_secs: ISSUED_AT,
        });
        assert_eq!(
            evaluate_emergency_chain_gate(request, Some(&grant)),
            Err(EmergencyChainGateError::ChainHashMismatch)
        );
        assert_eq!(
            evaluate_emergency_chain_gate(
                emergency_request(&grant, &[], &target_x, ISSUED_AT),
                Some(&grant),
            ),
            Err(EmergencyChainGateError::TargetOutsideChain)
        );
        assert_eq!(
            evaluate_emergency_chain_gate(
                emergency_request(&grant, &[], &target_b, ISSUED_AT),
                Some(&grant),
            ),
            Err(EmergencyChainGateError::TargetOutOfOrder)
        );
        assert_eq!(
            evaluate_emergency_chain_gate(
                emergency_request(
                    &grant,
                    std::slice::from_ref(&target_b),
                    &target_c,
                    ISSUED_AT
                ),
                Some(&grant),
            ),
            Err(EmergencyChainGateError::AttemptSequenceOutOfOrder)
        );
        assert_eq!(
            evaluate_emergency_chain_gate(
                emergency_request(&grant, &[target_x], &target_b, ISSUED_AT),
                Some(&grant),
            ),
            Err(EmergencyChainGateError::AttemptedTargetOutsideChain)
        );
        assert!(matches!(
            evaluate_emergency_chain_gate(
                emergency_request(&grant, &[target_a], &target_b, ISSUED_AT + 1),
                Some(&grant),
            ),
            Ok(EmergencyChainGateDecision::EmergencyTargetAuthorized {
                chain_position: 1,
                linearized_at_unix_secs,
                ..
            }) if linearized_at_unix_secs == ISSUED_AT + 1
        ));
    }

    #[test]
    fn emergency_order_ignores_seed_priority_health_and_input_order() {
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
                .map(|entry| candidates[entry.candidate_index].provider_id.as_str())
                .collect::<Vec<_>>();
            assert_eq!(ordered_ids, ["provider-a", "provider-b", "provider-c"]);
            assert!(!ordered_ids.contains(&"provider-outside"));
        }
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
