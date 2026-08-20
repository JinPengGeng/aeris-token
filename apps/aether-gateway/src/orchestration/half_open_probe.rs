use std::sync::{Arc, LazyLock, Weak};
use std::time::Duration;

use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::half_open_probes::{
    HalfOpenProbeCompletion, HalfOpenProbeCompletionWrite, HalfOpenProbeOutcome, HalfOpenProbeScope,
};
use aether_data_contracts::repository::provider_catalog::ProviderCatalogKeyHealthStateUpdate;
use aether_runtime_state::{
    DistributedHalfOpenProbeCoordinator, HalfOpenProbeLease, RuntimeStateBackendKind,
};
use dashmap::DashMap;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::warn;
use uuid::Uuid;

use crate::clock::current_unix_ms;
use crate::{AppState, GatewayError};

const PROBE_LEASE_MIN_TTL: Duration = Duration::from_secs(60);
const PROBE_LEASE_TIMEOUT_MARGIN: Duration = Duration::from_secs(60);
const DEFAULT_PROBE_EXECUTION_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const COMPLETION_LEASE_TTL: Duration = Duration::from_secs(30);
const HEALTH_CAS_MAX_ATTEMPTS: usize = 4;

static ACTIVE_PROBES: LazyLock<DashMap<String, Arc<ActiveHalfOpenProbe>>> =
    LazyLock::new(DashMap::new);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HalfOpenProbeAdmissionDeniedReason {
    Contended,
    ActiveDurableClaim,
    CircuitNotDue,
    CircuitChanged,
}

#[derive(Debug)]
pub(crate) enum HalfOpenProbeAdmission {
    NotRequired,
    Claimed(HalfOpenProbeSession),
    Denied(HalfOpenProbeAdmissionDeniedReason),
    FailClosed(String),
}

pub(crate) struct HalfOpenProbeSession {
    correlation_key: String,
    coordinator: DistributedHalfOpenProbeCoordinator,
    lease: HalfOpenProbeLease,
    durable_fencing_token: u64,
}

struct ActiveHalfOpenProbe {
    session: Mutex<Option<HalfOpenProbeSession>>,
    ttl: Duration,
}

pub(crate) struct HalfOpenProbeTerminalGuard {
    state: AppState,
    plan: ExecutionPlan,
}

impl HalfOpenProbeTerminalGuard {
    pub(crate) fn new(state: &AppState, plan: &ExecutionPlan) -> Self {
        Self {
            state: state.clone(),
            plan: plan.clone(),
        }
    }
}

impl Drop for HalfOpenProbeTerminalGuard {
    fn drop(&mut self) {
        let state = self.state.clone();
        let plan = self.plan.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                isolate_active_half_open_probe(&state, &plan).await;
            });
        } else {
            warn!(
                request_id = %self.plan.request_id,
                candidate_id = ?self.plan.candidate_id,
                "half-open probe terminal guard dropped without a Tokio runtime"
            );
        }
    }
}

pub(crate) struct PreparedHalfOpenProbeCompletion {
    session: HalfOpenProbeSession,
    completion: HalfOpenProbeCompletion,
}

impl PreparedHalfOpenProbeCompletion {
    pub(crate) fn durable_fencing_token(&self) -> u64 {
        self.session.durable_fencing_token
    }

    pub(crate) fn completion(&self) -> &HalfOpenProbeCompletion {
        &self.completion
    }
}

pub(crate) async fn isolate_aborted_half_open_completion(
    state: &AppState,
    prepared: Option<&PreparedHalfOpenProbeCompletion>,
) {
    let Some(prepared) = prepared else {
        return;
    };
    if let Err(error) = isolate_durable_claim(state, &prepared.session).await {
        warn!(
            error,
            "failed to isolate aborted half-open probe completion"
        );
    }
}

pub(crate) async fn isolate_active_half_open_probe(state: &AppState, plan: &ExecutionPlan) {
    let key = probe_correlation_key(plan);
    let Some((_, active)) = ACTIVE_PROBES.remove(&key) else {
        return;
    };
    let Some(session) = active.session.lock().await.take() else {
        return;
    };
    if let Err(error) = isolate_durable_claim(state, &session).await {
        warn!(error, "failed to isolate active half-open probe");
    }
}

impl std::fmt::Debug for HalfOpenProbeSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HalfOpenProbeSession")
            .field("correlation_key", &self.correlation_key)
            .field("scope", self.lease.scope())
            .field("owner", &self.lease.owner())
            .field("durable_fencing_token", &self.durable_fencing_token)
            .finish_non_exhaustive()
    }
}

pub(crate) async fn enforce_half_open_probe_admission(
    state: &AppState,
    plan: &ExecutionPlan,
) -> Result<(), GatewayError> {
    let correlation_key = probe_correlation_key(plan);
    match renew_active_probe_for_reuse(state, &correlation_key).await {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(message) => {
            return Err(GatewayError::UpstreamUnavailable {
                trace_id: plan.request_id.clone(),
                message,
            })
        }
    }
    match admit_half_open_probe(state, plan).await {
        HalfOpenProbeAdmission::NotRequired => Ok(()),
        HalfOpenProbeAdmission::Claimed(session) => {
            register_active_probe(state.clone(), session);
            Ok(())
        }
        HalfOpenProbeAdmission::Denied(reason) => Err(GatewayError::UpstreamUnavailable {
            trace_id: plan.request_id.clone(),
            message: format!("half-open probe admission denied: {reason:?}"),
        }),
        HalfOpenProbeAdmission::FailClosed(message) => Err(GatewayError::UpstreamUnavailable {
            trace_id: plan.request_id.clone(),
            message,
        }),
    }
}

async fn admit_half_open_probe(state: &AppState, plan: &ExecutionPlan) -> HalfOpenProbeAdmission {
    let api_format = plan.provider_api_format.trim();
    if api_format.is_empty() || plan.key_id.trim().is_empty() {
        return HalfOpenProbeAdmission::FailClosed(
            "half-open probe admission requires key_id and provider_api_format".to_string(),
        );
    }
    let now_ms = current_unix_ms();
    let mut current_key = match read_provider_key(state, &plan.key_id).await {
        Ok(Some(key)) => key,
        Ok(None) => {
            return HalfOpenProbeAdmission::FailClosed(
                "half-open probe final send gate could not find provider key".to_string(),
            )
        }
        Err(message) => return HalfOpenProbeAdmission::FailClosed(message),
    };
    match pending_completion_state(current_key.circuit_breaker_by_format.as_ref(), api_format) {
        PendingCompletionState::Absent => {}
        PendingCompletionState::Valid(_) => {
            if let Err(message) = replay_pending_completion(state, &current_key, api_format).await {
                return HalfOpenProbeAdmission::FailClosed(message);
            }
            current_key = match read_provider_key(state, &plan.key_id).await {
                Ok(Some(key)) => key,
                Ok(None) => {
                    return HalfOpenProbeAdmission::FailClosed(
                        "provider key disappeared after half-open completion replay".to_string(),
                    )
                }
                Err(message) => return HalfOpenProbeAdmission::FailClosed(message),
            };
        }
        PendingCompletionState::Malformed => {
            return HalfOpenProbeAdmission::FailClosed(
                "half-open completion pending marker is malformed".to_string(),
            )
        }
    }
    match probe_requirement(
        current_key.circuit_breaker_by_format.as_ref(),
        api_format,
        now_ms,
    ) {
        Ok(ProbeRequirement::Closed) => return HalfOpenProbeAdmission::NotRequired,
        Ok(ProbeRequirement::OpenNotDue) => {
            return HalfOpenProbeAdmission::Denied(
                HalfOpenProbeAdmissionDeniedReason::CircuitNotDue,
            )
        }
        Ok(ProbeRequirement::Due) => {}
        Err(message) => return HalfOpenProbeAdmission::FailClosed(message),
    }
    if state.runtime_state.backend_kind() != RuntimeStateBackendKind::Redis {
        return HalfOpenProbeAdmission::FailClosed(
            "distributed half-open probe admission requires Redis runtime state".to_string(),
        );
    }

    let scope = match HalfOpenProbeScope::new(&plan.key_id, api_format) {
        Ok(scope) => scope,
        Err(error) => return HalfOpenProbeAdmission::FailClosed(error.to_string()),
    };
    let coordinator =
        match DistributedHalfOpenProbeCoordinator::new(state.runtime_state.as_ref().clone()) {
            Ok(coordinator) => coordinator,
            Err(error) => return HalfOpenProbeAdmission::FailClosed(error.to_string()),
        };
    let owner = Uuid::now_v7().to_string();
    let probe_lease_ttl = probe_lease_ttl(plan);
    let lease = match coordinator
        .try_acquire(scope, owner.as_str(), probe_lease_ttl)
        .await
    {
        Ok(Some(lease)) => lease,
        Ok(None) => {
            return HalfOpenProbeAdmission::Denied(HalfOpenProbeAdmissionDeniedReason::Contended)
        }
        Err(error) => return HalfOpenProbeAdmission::FailClosed(error.to_string()),
    };

    for _ in 0..HEALTH_CAS_MAX_ATTEMPTS {
        let current_key = match read_provider_key(state, &plan.key_id).await {
            Ok(Some(key)) => key,
            Ok(None) => {
                let _ = coordinator.release(&lease).await;
                return HalfOpenProbeAdmission::FailClosed(
                    "half-open probe admission lost provider key during CAS".to_string(),
                );
            }
            Err(message) => {
                let _ = coordinator.release(&lease).await;
                return HalfOpenProbeAdmission::FailClosed(message);
            }
        };
        match probe_requirement(
            current_key.circuit_breaker_by_format.as_ref(),
            api_format,
            now_ms,
        ) {
            Ok(ProbeRequirement::Due) => {}
            Ok(ProbeRequirement::Closed | ProbeRequirement::OpenNotDue) => {
                let _ = coordinator.release(&lease).await;
                return HalfOpenProbeAdmission::Denied(
                    HalfOpenProbeAdmissionDeniedReason::CircuitChanged,
                );
            }
            Err(message) => {
                let _ = coordinator.release(&lease).await;
                return HalfOpenProbeAdmission::FailClosed(message);
            }
        }
        if durable_claim_is_active(
            current_key.circuit_breaker_by_format.as_ref(),
            api_format,
            now_ms,
        ) {
            let _ = coordinator.release(&lease).await;
            return HalfOpenProbeAdmission::Denied(
                HalfOpenProbeAdmissionDeniedReason::ActiveDurableClaim,
            );
        }
        let previous_fence =
            durable_fencing_token(current_key.circuit_breaker_by_format.as_ref(), api_format);
        let Some(durable_fencing_token) = previous_fence.checked_add(1) else {
            let _ = coordinator.release(&lease).await;
            return HalfOpenProbeAdmission::FailClosed(
                "half-open probe durable fencing token exhausted".to_string(),
            );
        };
        let expires_at_unix_ms = now_ms.saturating_add(probe_lease_ttl.as_millis() as u64);
        let circuit_breaker_by_format = project_durable_claim(
            current_key.circuit_breaker_by_format.as_ref(),
            api_format,
            owner.as_str(),
            durable_fencing_token,
            expires_at_unix_ms,
        );
        let update = ProviderCatalogKeyHealthStateUpdate {
            key_id: plan.key_id.clone(),
            expected_encrypted_auth_config: None,
            expected_health_by_format: current_key.health_by_format.clone(),
            expected_circuit_breaker_by_format: current_key.circuit_breaker_by_format.clone(),
            health_by_format: current_key.health_by_format,
            circuit_breaker_by_format: Some(circuit_breaker_by_format),
        };
        match state
            .compare_and_update_provider_catalog_key_health_state(&update)
            .await
        {
            Ok(true) => {
                return HalfOpenProbeAdmission::Claimed(HalfOpenProbeSession {
                    correlation_key: probe_correlation_key(plan),
                    coordinator,
                    lease,
                    durable_fencing_token,
                });
            }
            Ok(false) => tokio::task::yield_now().await,
            Err(error) => {
                let _ = coordinator.release(&lease).await;
                return HalfOpenProbeAdmission::FailClosed(format!("{error:?}"));
            }
        }
    }
    let _ = coordinator.release(&lease).await;
    HalfOpenProbeAdmission::FailClosed(
        "half-open probe durable claim CAS retries exhausted".to_string(),
    )
}

pub(crate) async fn prepare_half_open_probe_completion(
    state: &AppState,
    plan: &ExecutionPlan,
    outcome: HalfOpenProbeOutcome,
) -> Result<Option<PreparedHalfOpenProbeCompletion>, ()> {
    let correlation_key = probe_correlation_key(plan);
    let Some((_, active)) = ACTIVE_PROBES.remove(&correlation_key) else {
        return Ok(None);
    };
    let Some(mut session) = active.session.lock().await.take() else {
        return Err(());
    };
    let permit = match session
        .coordinator
        .authorize_completion(
            &mut session.lease,
            session.durable_fencing_token,
            COMPLETION_LEASE_TTL,
        )
        .await
    {
        Ok(Some(permit)) => permit,
        Ok(None) => return Err(()),
        Err(error) => {
            warn!(error = ?error, "half-open probe completion authorization failed closed");
            return Err(());
        }
    };
    let completion = match permit.into_completion(outcome, current_unix_ms()) {
        Ok(completion) => completion,
        Err(error) => {
            warn!(error = ?error, "half-open probe completion payload was invalid");
            return Err(());
        }
    };
    Ok(Some(PreparedHalfOpenProbeCompletion {
        session,
        completion,
    }))
}

pub(crate) async fn commit_half_open_probe_after_health_cas(
    state: &AppState,
    prepared: Option<PreparedHalfOpenProbeCompletion>,
) {
    let Some(prepared) = prepared else {
        return;
    };
    let PreparedHalfOpenProbeCompletion {
        session,
        completion,
    } = prepared;
    let pending_completion = match read_provider_key(state, &completion.scope.provider_key_id).await
    {
        Ok(Some(current_key)) => validate_pending_completion(
            current_key.circuit_breaker_by_format.as_ref(),
            &current_key.id,
            &completion.scope.api_format,
        ),
        Ok(None) => Err("provider key disappeared before completion audit write".to_string()),
        Err(error) => Err(error),
    };
    let pending_completion = match pending_completion {
        Ok(pending_completion) if pending_completion == completion => pending_completion,
        Ok(_) => {
            let isolation = isolate_durable_claim(state, &session).await;
            warn!(isolation = ?isolation, "half-open completion outbox changed before audit write; durable claim isolated fail closed");
            return;
        }
        Err(error) => {
            let isolation = isolate_durable_claim(state, &session).await;
            warn!(error, isolation = ?isolation, "half-open completion outbox validation failed before audit write; durable claim isolated fail closed");
            return;
        }
    };
    match state
        .data
        .complete_half_open_probe_if_newer(pending_completion.clone())
        .await
    {
        Ok(HalfOpenProbeCompletionWrite::Applied(_)) => {
            if let Err(error) = clear_pending_completion(state, &pending_completion).await {
                warn!(
                    error,
                    "half-open probe completion outbox cleanup failed closed"
                );
                return;
            }
            if let Err(error) = session.coordinator.release(&session.lease).await {
                warn!(error = ?error, "half-open probe lease release failed closed");
            }
        }
        Ok(HalfOpenProbeCompletionWrite::RejectedStale {
            current_fencing_token,
        }) => {
            if let Err(error) = clear_pending_completion(state, &pending_completion).await {
                warn!(error, "stale half-open probe outbox cleanup failed closed");
            }
            warn!(
                durable_fencing_token = session.durable_fencing_token,
                current_fencing_token,
                "half-open probe completion audit rejected a stale durable fence"
            );
        }
        Err(error) => {
            warn!(error = ?error, "half-open probe completion audit failed closed");
        }
    }
}

async fn renew_active_probe_for_reuse(state: &AppState, key: &str) -> Result<bool, String> {
    let active = ACTIVE_PROBES
        .get(key)
        .map(|entry| Arc::clone(entry.value()));
    let Some(active) = active else {
        return Ok(false);
    };
    renew_exact_active_probe(state, key, &active).await
}

pub(crate) async fn half_open_probe_is_active(plan: &ExecutionPlan) -> bool {
    ACTIVE_PROBES.contains_key(&probe_correlation_key(plan))
}

fn register_active_probe(state: AppState, session: HalfOpenProbeSession) {
    let key = session.correlation_key.clone();
    let ttl = Duration::from_millis(session.lease.ttl_ms());
    let active = Arc::new(ActiveHalfOpenProbe {
        session: Mutex::new(Some(session)),
        ttl,
    });
    ACTIVE_PROBES.insert(key.clone(), Arc::clone(&active));
    let active = Arc::downgrade(&active);
    tokio::spawn(async move {
        renew_active_probe_until_completion(state, key, active, ttl).await;
    });
}

async fn renew_active_probe_until_completion(
    state: AppState,
    key: String,
    active: Weak<ActiveHalfOpenProbe>,
    ttl: Duration,
) {
    let interval_ms = (ttl.as_millis() / 3).clamp(1_000, u64::MAX as u128) as u64;
    let interval = Duration::from_millis(interval_ms);
    loop {
        tokio::time::sleep(interval).await;
        let Some(active) = active.upgrade() else {
            return;
        };
        if let Err(error) = renew_exact_active_probe(&state, &key, &active).await {
            warn!(error, "half-open probe background renewal failed closed");
            return;
        }
    }
}

async fn renew_exact_active_probe(
    state: &AppState,
    key: &str,
    active: &Arc<ActiveHalfOpenProbe>,
) -> Result<bool, String> {
    let mut guard = active.session.lock().await;
    let Some(session) = guard.as_mut() else {
        drop(guard);
        ACTIVE_PROBES.remove(key);
        return Ok(false);
    };
    let redis_renewed = session
        .coordinator
        .renew(&mut session.lease, active.ttl)
        .await
        .map_err(|error| format!("half-open probe Redis renewal failed: {error:?}"));
    if !matches!(redis_renewed, Ok(true)) {
        let isolation = isolate_durable_claim(state, session).await;
        warn!(result = ?redis_renewed, isolation = ?isolation, "half-open probe Redis renewal failed; durable claim isolated fail closed");
        *guard = None;
        drop(guard);
        ACTIVE_PROBES.remove(key);
        return Err("half-open probe active lease could not be renewed".to_string());
    }

    let expires_at_unix_ms = current_unix_ms().saturating_add(active.ttl.as_millis() as u64);
    let durable_renewed = update_durable_claim_expiry(
        state,
        session.lease.scope().provider_key_id.as_str(),
        session.lease.scope().api_format.as_str(),
        session.lease.owner(),
        session.durable_fencing_token,
        expires_at_unix_ms,
    )
    .await;
    if !matches!(durable_renewed, Ok(true)) {
        let isolation = isolate_durable_claim(state, session).await;
        warn!(result = ?durable_renewed, isolation = ?isolation, "half-open probe durable renewal failed; claim isolated fail closed");
        *guard = None;
        drop(guard);
        ACTIVE_PROBES.remove(key);
        return Err("half-open probe durable claim could not be renewed".to_string());
    }
    Ok(true)
}

async fn isolate_durable_claim(
    state: &AppState,
    session: &HalfOpenProbeSession,
) -> Result<bool, String> {
    update_durable_claim_expiry(
        state,
        session.lease.scope().provider_key_id.as_str(),
        session.lease.scope().api_format.as_str(),
        session.lease.owner(),
        session.durable_fencing_token,
        u64::MAX,
    )
    .await
}

async fn update_durable_claim_expiry(
    state: &AppState,
    key_id: &str,
    api_format: &str,
    owner: &str,
    fencing_token: u64,
    expires_at_unix_ms: u64,
) -> Result<bool, String> {
    for _ in 0..HEALTH_CAS_MAX_ATTEMPTS {
        let Some(current_key) = read_provider_key(state, key_id).await? else {
            return Err("half-open probe provider key disappeared during renewal".to_string());
        };
        let mut circuit_by_format = current_key
            .circuit_breaker_by_format
            .clone()
            .ok_or_else(|| "half-open probe circuit disappeared during renewal".to_string())?;
        let Some(circuit) = circuit_by_format
            .as_object_mut()
            .and_then(|circuits| circuits.get_mut(api_format))
            .and_then(Value::as_object_mut)
        else {
            return Err("half-open probe circuit entry disappeared during renewal".to_string());
        };
        let claim_matches = circuit
            .get("half_open_claim")
            .and_then(Value::as_object)
            .is_some_and(|claim| {
                claim.get("owner").and_then(Value::as_str) == Some(owner)
                    && claim.get("fencing_token").and_then(Value::as_u64) == Some(fencing_token)
            });
        if current_u64(circuit, "half_open_fencing_token") != fencing_token || !claim_matches {
            return Ok(false);
        }
        let claim = circuit
            .get_mut("half_open_claim")
            .and_then(Value::as_object_mut)
            .expect("claim was validated above");
        claim.insert("expires_at_unix_ms".to_string(), json!(expires_at_unix_ms));
        circuit.insert(
            "half_open_until_unix_ms".to_string(),
            json!(expires_at_unix_ms),
        );
        let update = ProviderCatalogKeyHealthStateUpdate {
            key_id: key_id.to_string(),
            expected_encrypted_auth_config: None,
            expected_health_by_format: current_key.health_by_format.clone(),
            expected_circuit_breaker_by_format: current_key.circuit_breaker_by_format,
            health_by_format: current_key.health_by_format,
            circuit_breaker_by_format: Some(circuit_by_format),
        };
        match state
            .compare_and_update_provider_catalog_key_health_state(&update)
            .await
        {
            Ok(true) => return Ok(true),
            Ok(false) => tokio::task::yield_now().await,
            Err(error) => return Err(format!("{error:?}")),
        }
    }
    Ok(false)
}

pub(crate) fn attach_half_open_completion_pending(
    circuit_by_format: &mut Value,
    api_format: &str,
    prepared: &PreparedHalfOpenProbeCompletion,
) -> bool {
    let Some(circuit) = circuit_by_format
        .as_object_mut()
        .and_then(|circuits| circuits.get_mut(api_format))
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    if current_u64(circuit, "half_open_fencing_token") != prepared.durable_fencing_token() {
        return false;
    }
    let Ok(pending) = serde_json::to_value(prepared.completion()) else {
        return false;
    };
    circuit.insert("half_open_completion_pending".to_string(), pending);
    true
}

enum PendingCompletionState {
    Absent,
    Valid(HalfOpenProbeCompletion),
    Malformed,
}

fn pending_completion_state(
    circuit_by_format: Option<&Value>,
    api_format: &str,
) -> PendingCompletionState {
    let Some(value) = circuit_entry(circuit_by_format, api_format)
        .and_then(|circuit| circuit.get("half_open_completion_pending"))
        .cloned()
    else {
        return PendingCompletionState::Absent;
    };
    match serde_json::from_value(value) {
        Ok(completion) => PendingCompletionState::Valid(completion),
        Err(_) => PendingCompletionState::Malformed,
    }
}

fn validate_pending_completion(
    circuit_by_format: Option<&Value>,
    carrying_provider_key_id: &str,
    api_format: &str,
) -> Result<HalfOpenProbeCompletion, String> {
    let completion = match pending_completion_state(circuit_by_format, api_format) {
        PendingCompletionState::Valid(completion) => completion,
        PendingCompletionState::Absent => {
            return Err("half-open completion pending marker disappeared".to_string())
        }
        PendingCompletionState::Malformed => {
            return Err("half-open completion pending marker is malformed".to_string())
        }
    };
    completion
        .validate()
        .map_err(|error| format!("half-open completion pending marker is invalid: {error}"))?;
    if completion.scope.provider_key_id != carrying_provider_key_id {
        return Err(
            "half-open completion pending marker is bound to a different provider key".to_string(),
        );
    }
    if completion.scope.api_format != api_format {
        return Err(
            "half-open completion pending marker is bound to a different API format".to_string(),
        );
    }
    let current_fence = durable_fencing_token(circuit_by_format, api_format);
    if completion.fencing_token != current_fence {
        return Err(
            "half-open completion pending marker fence does not match the durable circuit fence"
                .to_string(),
        );
    }
    Ok(completion)
}

async fn replay_pending_completion(
    state: &AppState,
    current_key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
    api_format: &str,
) -> Result<(), String> {
    let completion = validate_pending_completion(
        current_key.circuit_breaker_by_format.as_ref(),
        &current_key.id,
        api_format,
    )?;
    match state
        .data
        .complete_half_open_probe_if_newer(completion.clone())
        .await
        .map_err(|error| format!("{error:?}"))?
    {
        HalfOpenProbeCompletionWrite::Applied(_)
        | HalfOpenProbeCompletionWrite::RejectedStale { .. } => {
            clear_pending_completion(state, &completion).await
        }
    }
}

async fn clear_pending_completion(
    state: &AppState,
    completion: &HalfOpenProbeCompletion,
) -> Result<(), String> {
    for _ in 0..HEALTH_CAS_MAX_ATTEMPTS {
        let Some(current_key) = read_provider_key(state, &completion.scope.provider_key_id).await?
        else {
            return Err("provider key disappeared while clearing completion outbox".to_string());
        };
        let mut circuit_by_format = current_key
            .circuit_breaker_by_format
            .clone()
            .ok_or_else(|| "circuit disappeared while clearing completion outbox".to_string())?;
        if matches!(
            pending_completion_state(Some(&circuit_by_format), &completion.scope.api_format),
            PendingCompletionState::Absent
        ) {
            return confirm_pending_completion_absent(state, completion).await;
        }
        let stored_pending = validate_pending_completion(
            Some(&circuit_by_format),
            &current_key.id,
            &completion.scope.api_format,
        )?;
        if stored_pending != *completion {
            return Err("completion outbox was replaced by a different completion".to_string());
        }
        let Some(circuit) = circuit_by_format
            .as_object_mut()
            .and_then(|circuits| circuits.get_mut(&completion.scope.api_format))
            .and_then(Value::as_object_mut)
        else {
            return Err("circuit entry disappeared while clearing completion outbox".to_string());
        };
        circuit.remove("half_open_completion_pending");
        let update = ProviderCatalogKeyHealthStateUpdate {
            key_id: completion.scope.provider_key_id.clone(),
            expected_encrypted_auth_config: None,
            expected_health_by_format: current_key.health_by_format.clone(),
            expected_circuit_breaker_by_format: current_key.circuit_breaker_by_format,
            health_by_format: current_key.health_by_format,
            circuit_breaker_by_format: Some(circuit_by_format),
        };
        match state
            .compare_and_update_provider_catalog_key_health_state(&update)
            .await
        {
            Ok(true) => return confirm_pending_completion_absent(state, completion).await,
            Ok(false) => tokio::task::yield_now().await,
            Err(error) => return Err(format!("{error:?}")),
        }
    }
    Err("completion outbox cleanup CAS retries exhausted".to_string())
}

async fn confirm_pending_completion_absent(
    state: &AppState,
    completion: &HalfOpenProbeCompletion,
) -> Result<(), String> {
    let Some(current_key) = read_provider_key(state, &completion.scope.provider_key_id).await?
    else {
        return Err(
            "provider key disappeared while confirming completion outbox cleanup".to_string(),
        );
    };
    match pending_completion_state(
        current_key.circuit_breaker_by_format.as_ref(),
        &completion.scope.api_format,
    ) {
        PendingCompletionState::Absent => Ok(()),
        PendingCompletionState::Valid(_) => {
            Err("completion outbox marker remains after cleanup".to_string())
        }
        PendingCompletionState::Malformed => {
            Err("completion outbox marker became malformed after cleanup".to_string())
        }
    }
}

fn probe_lease_ttl(plan: &ExecutionPlan) -> Duration {
    let configured_timeout_ms = plan.timeouts.as_ref().and_then(|timeouts| {
        [
            timeouts.connect_ms,
            timeouts.read_ms,
            timeouts.first_byte_ms,
            timeouts.write_ms,
            timeouts.pool_ms,
            timeouts.total_ms,
        ]
        .into_iter()
        .flatten()
        .max()
    });
    let execution_timeout = configured_timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_PROBE_EXECUTION_TIMEOUT);
    execution_timeout
        .saturating_add(PROBE_LEASE_TIMEOUT_MARGIN)
        .max(PROBE_LEASE_MIN_TTL)
}

async fn read_provider_key(
    state: &AppState,
    key_id: &str,
) -> Result<
    Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
    String,
> {
    state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await
        .map(|mut keys| keys.drain(..).next())
        .map_err(|error| format!("{error:?}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeRequirement {
    Closed,
    OpenNotDue,
    Due,
}

fn probe_requirement(
    circuit_by_format: Option<&Value>,
    api_format: &str,
    now_ms: u64,
) -> Result<ProbeRequirement, String> {
    let Some(circuits) = circuit_by_format else {
        return Ok(ProbeRequirement::Closed);
    };
    let circuits = circuits
        .as_object()
        .ok_or_else(|| "provider circuit map is malformed at final send gate".to_string())?;
    let Some(value) = circuits.get(api_format) else {
        return Ok(ProbeRequirement::Closed);
    };
    let circuit = value
        .as_object()
        .ok_or_else(|| "provider circuit entry is malformed at final send gate".to_string())?;
    let open = circuit
        .get("open")
        .and_then(Value::as_bool)
        .ok_or_else(|| "provider circuit open flag is missing or malformed".to_string())?;
    if !open {
        return Ok(ProbeRequirement::Closed);
    }
    let next_probe = circuit
        .get("next_probe_at_unix_secs")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            "open provider circuit is missing a valid next probe deadline".to_string()
        })?;
    if next_probe.saturating_mul(1_000) <= now_ms {
        Ok(ProbeRequirement::Due)
    } else {
        Ok(ProbeRequirement::OpenNotDue)
    }
}

fn durable_claim_is_active(
    circuit_by_format: Option<&Value>,
    api_format: &str,
    now_ms: u64,
) -> bool {
    circuit_entry(circuit_by_format, api_format)
        .and_then(|circuit| circuit.get("half_open_claim"))
        .and_then(Value::as_object)
        .and_then(|claim| claim.get("expires_at_unix_ms"))
        .and_then(Value::as_u64)
        .is_some_and(|expires_at| expires_at > now_ms)
}

fn durable_fencing_token(circuit_by_format: Option<&Value>, api_format: &str) -> u64 {
    circuit_entry(circuit_by_format, api_format)
        .and_then(|circuit| circuit.get("half_open_fencing_token"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn current_u64(current: &serde_json::Map<String, Value>, field: &str) -> u64 {
    current.get(field).and_then(Value::as_u64).unwrap_or(0)
}

fn project_durable_claim(
    current_circuit_by_format: Option<&Value>,
    api_format: &str,
    owner: &str,
    fencing_token: u64,
    expires_at_unix_ms: u64,
) -> Value {
    let mut circuits = current_circuit_by_format
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut circuit = circuits
        .get(api_format)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    circuit.insert("half_open_fencing_token".to_string(), json!(fencing_token));
    circuit.insert(
        "half_open_claim".to_string(),
        json!({
            "owner": owner,
            "fencing_token": fencing_token,
            "expires_at_unix_ms": expires_at_unix_ms,
        }),
    );
    circuit.insert(
        "half_open_until_unix_ms".to_string(),
        json!(expires_at_unix_ms),
    );
    circuits.insert(api_format.to_string(), Value::Object(circuit));
    Value::Object(circuits)
}

fn circuit_entry<'a>(
    circuit_by_format: Option<&'a Value>,
    api_format: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    circuit_by_format?.as_object()?.get(api_format)?.as_object()
}

fn probe_correlation_key(plan: &ExecutionPlan) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        plan.request_id,
        plan.candidate_id.as_deref().unwrap_or(""),
        plan.key_id,
        plan.provider_api_format
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_claim_projection_preserves_circuit_and_increments_authoritative_fence() {
        let current = json!({
            "openai:chat": {
                "open": true,
                "next_probe_at_unix_secs": 10,
                "half_open_fencing_token": 7
            }
        });
        let projected = project_durable_claim(Some(&current), "openai:chat", "owner-1", 8, 20_000);
        assert_eq!(projected["openai:chat"]["open"], json!(true));
        assert_eq!(
            projected["openai:chat"]["half_open_fencing_token"],
            json!(8)
        );
        assert!(durable_claim_is_active(
            Some(&projected),
            "openai:chat",
            19_999
        ));
        assert!(!durable_claim_is_active(
            Some(&projected),
            "openai:chat",
            20_000
        ));
    }

    #[test]
    fn durable_fence_continues_after_redis_counter_reset() {
        let persisted_circuit = json!({
            "openai:chat": {
                "open": true,
                "next_probe_at_unix_secs": 10,
                "half_open_fencing_token": 800
            }
        });
        // A restarted Redis may issue local fence 1, but admission allocates
        // exclusively from the persisted circuit value.
        let redis_local_fence_after_restart = 1;
        let next_durable_fence = durable_fencing_token(Some(&persisted_circuit), "openai:chat")
            .checked_add(1)
            .expect("durable fence");
        assert_eq!(redis_local_fence_after_restart, 1);
        assert_eq!(next_durable_fence, 801);
    }

    #[test]
    fn final_gate_distinguishes_closed_not_due_and_due_circuits() {
        let circuit = json!({
            "openai:chat": {"open": true, "next_probe_at_unix_secs": 20}
        });
        assert_eq!(
            probe_requirement(Some(&circuit), "openai:chat", 19_999),
            Ok(ProbeRequirement::OpenNotDue)
        );
        assert_eq!(
            probe_requirement(Some(&circuit), "openai:chat", 20_000),
            Ok(ProbeRequirement::Due)
        );
        assert_eq!(
            probe_requirement(Some(&circuit), "gemini", 20_000),
            Ok(ProbeRequirement::Closed)
        );
        assert!(probe_requirement(
            Some(&json!({"openai:chat": {"open": true}})),
            "openai:chat",
            20_000
        )
        .is_err());
    }

    #[test]
    fn probe_lease_ttl_covers_transport_timeout_and_margin() {
        let mut plan: ExecutionPlan = serde_json::from_value(json!({
            "request_id": "request-1",
            "provider_id": "provider-1",
            "endpoint_id": "endpoint-1",
            "key_id": "key-1",
            "method": "POST",
            "url": "https://example.invalid",
            "body": {},
            "client_api_format": "openai:chat",
            "provider_api_format": "openai:chat"
        }))
        .expect("plan");
        plan.timeouts = Some(aether_contracts::ExecutionTimeouts {
            read_ms: Some(120_000),
            ..aether_contracts::ExecutionTimeouts::default()
        });
        assert_eq!(probe_lease_ttl(&plan), Duration::from_secs(180));
    }

    #[test]
    fn malformed_pending_completion_marker_is_not_treated_as_absent() {
        let circuits = json!({
            "openai:chat": {
                "open": true,
                "half_open_completion_pending": {"completion_id": 7}
            }
        });
        assert!(matches!(
            pending_completion_state(Some(&circuits), "openai:chat"),
            PendingCompletionState::Malformed
        ));
        assert!(matches!(
            pending_completion_state(Some(&circuits), "gemini"),
            PendingCompletionState::Absent
        ));
    }

    fn pending_completion(provider_key_id: &str, api_format: &str, fence: u64) -> Value {
        serde_json::to_value(HalfOpenProbeCompletion {
            completion_id: "completion-1".to_string(),
            scope: HalfOpenProbeScope::new(provider_key_id, api_format).expect("scope"),
            owner: "owner-1".to_string(),
            fencing_token: fence,
            completed_at_unix_ms: 10,
            outcome: HalfOpenProbeOutcome::Succeeded,
        })
        .expect("completion")
    }

    #[test]
    fn pending_completion_validation_rejects_cross_key_and_cross_format_payloads() {
        let cross_key = json!({
            "openai:chat": {
                "half_open_fencing_token": 7,
                "half_open_completion_pending": pending_completion("key-b", "openai:chat", 7)
            }
        });
        assert!(
            validate_pending_completion(Some(&cross_key), "key-a", "openai:chat")
                .expect_err("cross-key payload must fail closed")
                .contains("different provider key")
        );

        let cross_format = json!({
            "openai:chat": {
                "half_open_fencing_token": 7,
                "half_open_completion_pending": pending_completion("key-a", "gemini", 7)
            }
        });
        assert!(
            validate_pending_completion(Some(&cross_format), "key-a", "openai:chat")
                .expect_err("cross-format payload must fail closed")
                .contains("different API format")
        );
    }

    #[test]
    fn pending_completion_validation_requires_current_durable_fence() {
        let circuits = json!({
            "openai:chat": {
                "half_open_fencing_token": 8,
                "half_open_completion_pending": pending_completion("key-a", "openai:chat", 7)
            }
        });
        assert!(
            validate_pending_completion(Some(&circuits), "key-a", "openai:chat")
                .expect_err("stale outbox fence must fail closed")
                .contains("durable circuit fence")
        );
    }

    #[test]
    fn oauth_retry_physical_send_has_a_second_final_admission_gate() {
        let stream = include_str!("../execution_runtime/stream/execution.rs");
        let start = stream
            .find("async fn execute_in_process_stream_with_oauth_retry")
            .expect("OAuth retry entrypoint");
        let stream = &stream[start..];
        let refresh = stream
            .find("refresh_oauth_plan_auth_for_retry(")
            .expect("OAuth refresh");
        let retry = &stream[refresh..];
        let retry_gate = retry
            .find("enforce_half_open_probe_admission(state, plan)")
            .expect("OAuth retry final gate");
        let retry_send = retry
            .find("execution = execute_in_process_stream(state, plan, trace_id).await?")
            .expect("OAuth retry physical send");
        assert!(retry_gate < retry_send);
    }

    #[test]
    fn candidate_final_gates_precede_every_special_transport_dispatch() {
        let sync = include_str!("../execution_runtime/sync/execution.rs");
        let sync_start = sync
            .find("async fn execute_execution_runtime_sync_impl")
            .expect("sync candidate entrypoint");
        let sync = &sync[sync_start..];
        let sync_gate = sync
            .find("enforce_half_open_probe_admission(state, &plan)")
            .expect("sync final gate");
        for dispatch in [
            "maybe_execute_grok_sync(&plan",
            "maybe_execute_chatgpt_web_image_sync(state, &plan",
            "execute_direct_sync_runtime_candidate(",
        ] {
            assert!(
                sync_gate < sync.find(dispatch).expect("sync special transport"),
                "sync gate must precede {dispatch}"
            );
        }

        let stream = include_str!("../execution_runtime/stream/execution.rs");
        let stream_start = stream
            .find("async fn execute_execution_runtime_stream_inner")
            .expect("stream candidate entrypoint");
        let stream = &stream[stream_start..];
        let stream_gate = stream
            .find("enforce_half_open_probe_admission(state, &plan)")
            .expect("stream final gate");
        for dispatch in [
            "maybe_execute_grok_stream(&plan",
            "maybe_execute_windsurf_stream(state, &plan",
            "maybe_execute_kiro_web_search_stream(state, &plan",
            "maybe_execute_chatgpt_web_image_stream(state, &plan",
            "execute_in_process_stream_with_oauth_retry(",
            "post_stream_plan_to_remote_execution_runtime(",
        ] {
            assert!(
                stream_gate < stream.find(dispatch).expect("stream special transport"),
                "stream gate must precede {dispatch}"
            );
        }
    }

    #[test]
    fn stream_final_gate_guard_is_transferred_to_special_direct_and_remote_pumps() {
        let stream = include_str!("../execution_runtime/stream/execution.rs");
        let start = stream
            .find("async fn execute_execution_runtime_stream_inner")
            .expect("stream candidate entrypoint");
        let stream = &stream[start..];
        let guard = stream
            .find("let mut half_open_probe_terminal_guard = Some(")
            .expect("single transferable terminal guard");
        for dispatch in [
            "grok_stream.frame_stream",
            "windsurf_stream.frame_stream",
            "kiro_web_search.frame_stream",
            "chatgpt_web_image.frame_stream",
            "Some(remote_fallback_observation)",
        ] {
            let dispatch = stream
                .find(dispatch)
                .expect("guarded frame-stream dispatch");
            assert!(guard < dispatch);
            let suffix = &stream[dispatch..];
            assert!(
                suffix[..suffix.find(").await").unwrap_or(suffix.len())]
                    .contains("half_open_probe_terminal_guard.take()"),
                "{dispatch} must transfer the terminal guard"
            );
        }
    }

    #[test]
    fn admin_chatgpt_web_image_model_test_uses_the_gated_sync_dispatcher() {
        let model_test = include_str!("../handlers/admin/provider/query/models/model_test.rs");
        let start = model_test
            .find("} else if is_chatgpt_web {")
            .expect("ChatGPT-Web image model-test branch");
        let branch = &model_test[start..];
        let end = branch
            .find("} else {")
            .expect("next image model-test branch");
        let branch = &branch[..end];
        assert!(branch.contains("execute_execution_runtime_sync_plan_with_report_context("));
        assert!(!branch.contains("maybe_execute_chatgpt_web_image_sync("));
    }
}
