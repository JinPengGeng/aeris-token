use aether_contracts::ExecutionPlan;
use aether_runtime::AdmissionPermitHealth;
use aether_runtime_state::{RuntimeSemaphoreConfig, RuntimeSemaphoreError, RuntimeSemaphorePermit};
use aether_scheduler_core::{
    SchedulerMinimalCandidateSelectionCandidate, SendAdmissionSkip, SendAdmissionSkipReason,
    SendAdmissionStop, SendAdmissionStopReason,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::orchestration::{HalfOpenProbeClaim, HalfOpenProbeClaimOutcome};
use crate::scheduler::candidate::{
    current_candidate_runtime_skip_reason,
    current_candidate_runtime_skip_reason_excluding_pending_candidate,
    current_candidate_runtime_skip_reason_for_revalidation,
    read_candidate_runtime_selection_snapshot_with_fresh_catalog,
};
use crate::scheduler::state::SchedulerRuntimeState;
use crate::AppState;

const PROVIDER_KEY_CONCURRENCY_GATE: &str = "provider_key_send_concurrency";

#[derive(Debug)]
pub(crate) enum GatewaySendAdmissionDecision<T = GatewaySendAdmissionGuard> {
    Admit(T),
    Skip(SendAdmissionSkip),
    Stop(SendAdmissionStop),
}

/// Kept alive across the physical send. Half-open probe and reservation guards
/// are attached here so admission cannot be separated from dispatch.
pub(crate) struct GatewaySendAdmissionGuard {
    state: Option<AppState>,
    half_open_probe: Option<HalfOpenProbeClaim>,
    provider_key_concurrency_permit: Option<RuntimeSemaphorePermit>,
    probe_alive: Arc<AtomicBool>,
    renewal_task: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for GatewaySendAdmissionGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GatewaySendAdmissionGuard")
            .field("has_half_open_probe", &self.half_open_probe.is_some())
            .field(
                "has_provider_key_concurrency_permit",
                &self.provider_key_concurrency_permit.is_some(),
            )
            .field("probe_alive", &self.probe_alive.load(Ordering::Acquire))
            .finish()
    }
}

impl GatewaySendAdmissionGuard {
    #[cfg(test)]
    pub(crate) fn without_probe() -> Self {
        Self {
            state: None,
            half_open_probe: None,
            provider_key_concurrency_permit: None,
            probe_alive: Arc::new(AtomicBool::new(true)),
            renewal_task: None,
        }
    }

    pub(crate) fn ensure_alive(&self) -> Result<(), SendAdmissionStopReason> {
        (self.probe_alive.load(Ordering::Acquire)
            && self
                .provider_key_concurrency_permit
                .as_ref()
                .map(AdmissionPermitHealth::is_healthy)
                .unwrap_or(true))
        .then_some(())
        .ok_or(SendAdmissionStopReason::LeaseLost)
    }

    pub(crate) async fn release(mut self) {
        if let Some(task) = self.renewal_task.take() {
            task.abort();
        }
        if let Some(permit) = self.provider_key_concurrency_permit.take() {
            let _ = permit.release().await;
        }
        if let (Some(state), Some(claim)) = (self.state.take(), self.half_open_probe.take()) {
            let _ = state.release_half_open_probe(claim).await;
        }
    }

    fn with_provider_key_concurrency_permit(
        provider_key_concurrency_permit: Option<RuntimeSemaphorePermit>,
    ) -> Self {
        Self {
            state: None,
            half_open_probe: None,
            provider_key_concurrency_permit,
            probe_alive: Arc::new(AtomicBool::new(true)),
            renewal_task: None,
        }
    }
}

impl Drop for GatewaySendAdmissionGuard {
    fn drop(&mut self) {
        if let Some(task) = self.renewal_task.take() {
            task.abort();
        }
        let (Some(state), Some(claim)) = (self.state.take(), self.half_open_probe.take()) else {
            return;
        };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = state.release_half_open_probe(claim).await;
            });
        }
    }
}

pub(crate) async fn request_gateway_send_admission(
    state: &AppState,
    plan: &ExecutionPlan,
) -> GatewaySendAdmissionDecision {
    match request_gateway_send_admission_inner(state, plan).await {
        Ok(AdmissionOutcome::Admit(guard)) => GatewaySendAdmissionDecision::Admit(guard),
        Ok(AdmissionOutcome::Skip(reason)) => {
            GatewaySendAdmissionDecision::Skip(SendAdmissionSkip::new(reason))
        }
        Err(AdmissionFailure::Stop(reason)) => {
            GatewaySendAdmissionDecision::Stop(SendAdmissionStop::new(reason))
        }
    }
}

/// Rechecks a plan immediately before an internal retry sends it. The existing
/// guard retains its shared concurrency permit and half-open claim, so this
/// only refreshes authority and runtime state.
pub(crate) async fn revalidate_gateway_send_admission(
    state: &AppState,
    plan: &ExecutionPlan,
    guard: &GatewaySendAdmissionGuard,
) -> GatewaySendAdmissionDecision<()> {
    if let Err(reason) = guard.ensure_alive() {
        return GatewaySendAdmissionDecision::Stop(SendAdmissionStop::new(reason));
    }
    match validate_gateway_send_admission(state, plan, Some(plan.request_id.as_str()), None).await {
        Ok(AdmissionValidation::Ready(_)) => GatewaySendAdmissionDecision::Admit(()),
        Ok(AdmissionValidation::Skip(reason)) => {
            GatewaySendAdmissionDecision::Skip(SendAdmissionSkip::new(reason))
        }
        Err(AdmissionFailure::Stop(reason)) => {
            GatewaySendAdmissionDecision::Stop(SendAdmissionStop::new(reason))
        }
    }
}

enum AdmissionFailure {
    Stop(SendAdmissionStopReason),
}

enum AdmissionOutcome {
    Admit(GatewaySendAdmissionGuard),
    Skip(SendAdmissionSkipReason),
}

enum AdmissionValidation {
    Ready(ValidatedSendAdmission),
    Skip(SendAdmissionSkipReason),
}

struct ValidatedSendAdmission {
    candidate: SchedulerMinimalCandidateSelectionCandidate,
    provider_key_concurrent_limit: Option<i32>,
    circuit_breaker_by_format: Option<serde_json::Value>,
}

async fn request_gateway_send_admission_inner(
    state: &AppState,
    plan: &ExecutionPlan,
) -> Result<AdmissionOutcome, AdmissionFailure> {
    let validated =
        match validate_gateway_send_admission(state, plan, None, plan.candidate_id.as_deref())
            .await?
        {
            AdmissionValidation::Ready(validated) => validated,
            AdmissionValidation::Skip(reason) => return Ok(AdmissionOutcome::Skip(reason)),
        };

    let provider_key_concurrency_permit = match try_acquire_provider_key_concurrency_permit(
        state,
        validated.candidate.key_id.as_str(),
        validated.provider_key_concurrent_limit,
    )
    .await
    {
        Ok(permit) => permit,
        Err(ProviderKeyConcurrencyReservationFailure::Saturated) => {
            return Ok(AdmissionOutcome::Skip(
                SendAdmissionSkipReason::CredentialConcurrencyExhausted,
            ));
        }
        Err(ProviderKeyConcurrencyReservationFailure::Unavailable) => {
            return Err(AdmissionFailure::Stop(
                SendAdmissionStopReason::ReservationBackendUnavailable,
            ));
        }
    };

    match state
        .try_acquire_half_open_probe(
            validated.candidate.key_id.as_str(),
            validated.candidate.endpoint_api_format.as_str(),
            validated.circuit_breaker_by_format.as_ref(),
            crate::clock::current_unix_secs(),
        )
        .await
    {
        HalfOpenProbeClaimOutcome::NotRequired => Ok(AdmissionOutcome::Admit(
            GatewaySendAdmissionGuard::with_provider_key_concurrency_permit(
                provider_key_concurrency_permit,
            ),
        )),
        HalfOpenProbeClaimOutcome::Busy => Ok(AdmissionOutcome::Skip(
            SendAdmissionSkipReason::CredentialCircuitOpen,
        )),
        HalfOpenProbeClaimOutcome::Unavailable => Err(AdmissionFailure::Stop(
            SendAdmissionStopReason::ReservationBackendUnavailable,
        )),
        HalfOpenProbeClaimOutcome::Acquired(claim) => {
            if !state.renew_half_open_probe(&claim).await {
                let _ = state.release_half_open_probe(claim).await;
                return Err(AdmissionFailure::Stop(SendAdmissionStopReason::LeaseLost));
            }
            let probe_alive = Arc::new(AtomicBool::new(true));
            let renewal_alive = probe_alive.clone();
            let renewal_state = state.clone();
            let renewal_claim = claim.clone();
            let renewal_task = tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(10));
                interval.tick().await;
                loop {
                    interval.tick().await;
                    if !renewal_state.renew_half_open_probe(&renewal_claim).await {
                        renewal_alive.store(false, Ordering::Release);
                        break;
                    }
                }
            });
            Ok(AdmissionOutcome::Admit(GatewaySendAdmissionGuard {
                state: Some(state.clone()),
                half_open_probe: Some(claim),
                provider_key_concurrency_permit,
                probe_alive,
                renewal_task: Some(renewal_task),
            }))
        }
    }
}

async fn validate_gateway_send_admission(
    state: &AppState,
    plan: &ExecutionPlan,
    revalidating_request_id: Option<&str>,
    pending_candidate_id: Option<&str>,
) -> Result<AdmissionValidation, AdmissionFailure> {
    let providers = state
        .read_provider_catalog_providers_by_ids_strong(std::slice::from_ref(&plan.provider_id))
        .await
        .map_err(|_| AdmissionFailure::Stop(SendAdmissionStopReason::AuthorityReadFailed))?;
    let Some(provider) = providers.into_iter().find(|row| row.id == plan.provider_id) else {
        return Ok(AdmissionValidation::Skip(
            SendAdmissionSkipReason::BindingMissing,
        ));
    };
    if !provider.is_active {
        return Ok(AdmissionValidation::Skip(
            SendAdmissionSkipReason::ProviderInactive,
        ));
    }

    let endpoints = state
        .read_provider_catalog_endpoints_by_ids_strong(std::slice::from_ref(&plan.endpoint_id))
        .await
        .map_err(|_| AdmissionFailure::Stop(SendAdmissionStopReason::AuthorityReadFailed))?;
    let Some(endpoint) = endpoints
        .into_iter()
        .find(|row| row.id == plan.endpoint_id && row.provider_id == plan.provider_id)
    else {
        return Ok(AdmissionValidation::Skip(
            SendAdmissionSkipReason::BindingMissing,
        ));
    };
    if !endpoint.is_active {
        return Ok(AdmissionValidation::Skip(
            SendAdmissionSkipReason::EndpointInactive,
        ));
    }
    if endpoint.health_score <= 0.0 {
        return Ok(AdmissionValidation::Skip(
            SendAdmissionSkipReason::EndpointUnhealthy,
        ));
    }

    let keys = state
        .list_provider_catalog_keys_by_ids_strong(std::slice::from_ref(&plan.key_id))
        .await
        .map_err(|_| AdmissionFailure::Stop(SendAdmissionStopReason::AuthorityReadFailed))?;
    let Some(key) = keys
        .into_iter()
        .find(|row| row.id == plan.key_id && row.provider_id == plan.provider_id)
    else {
        return Ok(AdmissionValidation::Skip(
            SendAdmissionSkipReason::BindingMissing,
        ));
    };
    if !key.is_active {
        return Ok(AdmissionValidation::Skip(
            SendAdmissionSkipReason::CredentialInactive,
        ));
    }

    let now_unix_secs = crate::clock::current_unix_secs();
    if key
        .expires_at_unix_secs
        .is_some_and(|expires_at| expires_at <= now_unix_secs)
    {
        return Ok(AdmissionValidation::Skip(
            SendAdmissionSkipReason::CredentialExpired,
        ));
    }

    let candidate = SchedulerMinimalCandidateSelectionCandidate {
        provider_id: plan.provider_id.clone(),
        provider_name: provider.name.clone(),
        provider_type: provider.provider_type.clone(),
        provider_priority: provider.provider_priority,
        endpoint_id: plan.endpoint_id.clone(),
        endpoint_api_format: plan.provider_api_format.clone(),
        key_id: plan.key_id.clone(),
        key_name: key.name.clone(),
        key_auth_type: key.auth_type.clone(),
        key_internal_priority: key.internal_priority,
        key_global_priority_for_format: None,
        key_capabilities: key.capabilities.clone(),
        model_id: String::new(),
        global_model_id: String::new(),
        global_model_name: plan.model_name.clone().unwrap_or_default(),
        selected_provider_model_name: plan.model_name.clone().unwrap_or_default(),
        supports_streaming: plan.stream,
        mapping_matched_model: None,
    };
    let snapshot = read_candidate_runtime_selection_snapshot_with_fresh_catalog(
        state,
        std::slice::from_ref(&candidate),
        None,
        now_unix_secs,
        std::slice::from_ref(&provider),
        std::slice::from_ref(&key),
    )
    .await
    .map_err(|_| AdmissionFailure::Stop(SendAdmissionStopReason::AuthorityReadFailed))?;

    let runtime_skip_reason = match (revalidating_request_id, pending_candidate_id) {
        (Some(request_id), _) => current_candidate_runtime_skip_reason_for_revalidation(
            &candidate,
            &snapshot,
            now_unix_secs,
            request_id,
            plan.candidate_id.as_deref(),
        ),
        (None, Some(candidate_id)) => {
            current_candidate_runtime_skip_reason_excluding_pending_candidate(
                &candidate,
                &snapshot,
                now_unix_secs,
                plan.request_id.as_str(),
                candidate_id,
            )
        }
        (None, None) => current_candidate_runtime_skip_reason(&candidate, &snapshot, now_unix_secs),
    };
    if let Some(reason) = runtime_skip_reason {
        return Ok(AdmissionValidation::Skip(map_runtime_skip_reason(reason)));
    }

    Ok(AdmissionValidation::Ready(ValidatedSendAdmission {
        candidate,
        provider_key_concurrent_limit: key.concurrent_limit,
        circuit_breaker_by_format: key.circuit_breaker_by_format.clone(),
    }))
}

enum ProviderKeyConcurrencyReservationFailure {
    Saturated,
    Unavailable,
}

async fn try_acquire_provider_key_concurrency_permit(
    state: &AppState,
    provider_key_id: &str,
    concurrent_limit: Option<i32>,
) -> Result<Option<RuntimeSemaphorePermit>, ProviderKeyConcurrencyReservationFailure> {
    let Some(limit) = concurrent_limit
        .filter(|limit| *limit > 0)
        .and_then(|limit| usize::try_from(limit).ok())
    else {
        return Ok(None);
    };
    let gate = state
        .runtime_state()
        .keyed_semaphore(
            PROVIDER_KEY_CONCURRENCY_GATE,
            format!("provider-key:{{{provider_key_id}}}"),
            limit,
            RuntimeSemaphoreConfig::default(),
        )
        .map_err(|_| ProviderKeyConcurrencyReservationFailure::Unavailable)?;
    match gate.try_acquire().await {
        Ok(permit) => Ok(Some(permit)),
        Err(RuntimeSemaphoreError::Saturated { .. }) => {
            Err(ProviderKeyConcurrencyReservationFailure::Saturated)
        }
        Err(
            RuntimeSemaphoreError::Unavailable { .. }
            | RuntimeSemaphoreError::InvalidConfiguration(_),
        ) => Err(ProviderKeyConcurrencyReservationFailure::Unavailable),
    }
}

fn map_runtime_skip_reason(reason: &str) -> SendAdmissionSkipReason {
    match reason {
        "provider_quota_blocked" => SendAdmissionSkipReason::ProviderQuotaExhausted,
        "provider_concurrency_limit_reached" => {
            SendAdmissionSkipReason::ProviderConcurrencyExhausted
        }
        "account_quota_exhausted" => SendAdmissionSkipReason::AccountQuotaExhausted,
        "oauth_invalid" => SendAdmissionSkipReason::OAuthInvalid,
        "key_circuit_open" => SendAdmissionSkipReason::CredentialCircuitOpen,
        "key_health_score_zero" => SendAdmissionSkipReason::CredentialUnhealthy,
        "key_rpm_exhausted" => SendAdmissionSkipReason::CredentialRpmExhausted,
        "provider_key_concurrency_limit_reached" => {
            SendAdmissionSkipReason::CredentialConcurrencyExhausted
        }
        _ => SendAdmissionSkipReason::BindingMissing,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_contracts::{ExecutionPlan, RequestBody};
    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, StoredRequestCandidate,
    };
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogWriteRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
        StoredProviderCatalogProvider,
    };
    use aether_runtime_state::{RedisClientConfig, RuntimeState};
    use aether_testkit::ManagedRedisServer;
    use serde_json::json;

    use super::*;
    use crate::data::GatewayDataState;

    fn provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "provider".to_string(),
            None,
            "custom".to_string(),
        )
        .expect("provider")
    }

    fn endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-1".to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint")
    }

    fn key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key")
    }

    fn plan() -> ExecutionPlan {
        ExecutionPlan {
            request_id: "request-1".to_string(),
            candidate_id: Some("candidate-1".to_string()),
            provider_name: Some("provider".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/chat/completions".to_string(),
            headers: Default::default(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(serde_json::json!({})),
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("gpt-test".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn state_with_repository(repository: Arc<InMemoryProviderCatalogReadRepository>) -> AppState {
        let data = GatewayDataState::with_provider_catalog_reader_for_tests(repository.clone())
            .with_cached_provider_catalog_reader_for_tests(repository);
        let mut state = AppState::new().expect("state");
        state.replace_data_state(Arc::new(data));
        state
    }

    fn state_with_repository_and_request_candidates(
        repository: Arc<InMemoryProviderCatalogReadRepository>,
        request_candidates: Arc<InMemoryRequestCandidateRepository>,
    ) -> AppState {
        let data = GatewayDataState::with_provider_catalog_reader_for_tests(repository.clone())
            .with_cached_provider_catalog_reader_for_tests(repository)
            .with_request_candidate_repository(request_candidates);
        let mut state = AppState::new().expect("state");
        state.replace_data_state(Arc::new(data));
        state
    }

    fn running_candidate(request_id: &str) -> StoredRequestCandidate {
        let now_unix_ms =
            i64::try_from(crate::clock::current_unix_secs()).expect("unix seconds fit i64") * 1_000;
        StoredRequestCandidate::new(
            "candidate-1".to_string(),
            request_id.to_string(),
            None,
            None,
            None,
            None,
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            RequestCandidateStatus::Streaming,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            now_unix_ms,
            Some(now_unix_ms),
            None,
        )
        .expect("running request candidate")
    }

    fn pending_candidate(request_id: &str) -> StoredRequestCandidate {
        let now_unix_ms =
            i64::try_from(crate::clock::current_unix_secs()).expect("unix seconds fit i64") * 1_000;
        StoredRequestCandidate::new(
            "candidate-1".to_string(),
            request_id.to_string(),
            None,
            None,
            None,
            None,
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            RequestCandidateStatus::Pending,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            now_unix_ms,
            Some(now_unix_ms),
            None,
        )
        .expect("pending request candidate")
    }

    fn state_with_repository_and_runtime(
        repository: Arc<InMemoryProviderCatalogReadRepository>,
        runtime: Arc<RuntimeState>,
        instance_id: &str,
    ) -> AppState {
        let data = GatewayDataState::with_provider_catalog_reader_for_tests(repository.clone())
            .with_cached_provider_catalog_reader_for_tests(repository);
        let mut state = AppState::new().expect("state").with_runtime_state(runtime);
        state.replace_data_state(Arc::new(data));
        state.with_tunnel_identity(instance_id, None::<String>)
    }

    fn due_probe_key() -> StoredProviderCatalogKey {
        let mut result = key();
        result.circuit_breaker_by_format = Some(json!({
            "openai:chat": {
                "open": true,
                "next_probe_at_unix_secs": 0
            }
        }));
        result
    }

    fn concurrent_limited_key() -> StoredProviderCatalogKey {
        let mut result = key();
        result.concurrent_limit = Some(1);
        result
    }

    #[tokio::test]
    async fn closed_candidate_is_admitted() {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider()],
            vec![endpoint()],
            vec![key()],
        ));
        let state = state_with_repository(repository);

        assert!(matches!(
            request_gateway_send_admission(&state, &plan()).await,
            GatewaySendAdmissionDecision::Admit(_)
        ));
    }

    #[tokio::test]
    async fn initial_admission_ignores_its_own_pending_candidate_placeholder() {
        let mut limited_key = concurrent_limited_key();
        limited_key.rpm_limit = Some(1);
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider()],
            vec![endpoint()],
            vec![limited_key],
        ));
        let state = state_with_repository_and_request_candidates(
            repository,
            Arc::new(InMemoryRequestCandidateRepository::seed(vec![
                pending_candidate("request-1"),
            ])),
        );

        assert!(matches!(
            request_gateway_send_admission(&state, &plan()).await,
            GatewaySendAdmissionDecision::Admit(_)
        ));
    }

    #[tokio::test]
    async fn unconfigured_key_still_allows_parallel_admission() {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider()],
            vec![endpoint()],
            vec![key()],
        ));
        let first_gateway = state_with_repository(Arc::clone(&repository));
        let second_gateway = state_with_repository(repository);
        let selected_plan = plan();
        let (first, second) = tokio::join!(
            request_gateway_send_admission(&first_gateway, &selected_plan),
            request_gateway_send_admission(&second_gateway, &selected_plan)
        );

        assert!(matches!(first, GatewaySendAdmissionDecision::Admit(_)));
        assert!(matches!(second, GatewaySendAdmissionDecision::Admit(_)));
    }

    #[tokio::test]
    async fn strong_read_rejects_key_disabled_after_plan_selection() {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider()],
            vec![endpoint()],
            vec![key()],
        ));
        let state = state_with_repository(Arc::clone(&repository));
        assert!(matches!(
            request_gateway_send_admission(&state, &plan()).await,
            GatewaySendAdmissionDecision::Admit(_)
        ));

        let mut disabled = key();
        disabled.is_active = false;
        repository.update_key(&disabled).await.expect("disable key");

        assert!(matches!(
            request_gateway_send_admission(&state, &plan()).await,
            GatewaySendAdmissionDecision::Skip(skip)
                if skip.reason() == SendAdmissionSkipReason::CredentialInactive
        ));
    }

    #[tokio::test]
    async fn fresh_runtime_rejects_backend_key_health_change_despite_warm_cache() {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider()],
            vec![endpoint()],
            vec![key()],
        ));
        let state = state_with_repository(Arc::clone(&repository));
        let cached = state
            .read_provider_catalog_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("warm ordinary key cache");
        assert!(cached[0].health_by_format.is_none());

        let unhealthy_health_by_format = json!({
            "openai:chat": {"health_score": 0.0}
        });
        repository
            .update_key_health_state("key-1", true, Some(&unhealthy_health_by_format), None)
            .await
            .expect("update backing key health");

        assert!(matches!(
            request_gateway_send_admission(&state, &plan()).await,
            GatewaySendAdmissionDecision::Skip(skip)
                if skip.reason() == SendAdmissionSkipReason::CredentialUnhealthy
        ));
    }

    #[tokio::test]
    async fn revalidation_ignores_its_own_concurrency_row_but_keeps_its_rpm_history() {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider()],
            vec![endpoint()],
            vec![concurrent_limited_key()],
        ));
        let admission_state = state_with_repository(Arc::clone(&repository));
        let admission_guard = match request_gateway_send_admission(&admission_state, &plan()).await
        {
            GatewaySendAdmissionDecision::Admit(guard) => guard,
            _ => panic!("initial send admission should acquire its only key permit"),
        };
        let revalidation_state = state_with_repository_and_request_candidates(
            Arc::clone(&repository),
            Arc::new(InMemoryRequestCandidateRepository::seed(vec![
                running_candidate("request-1"),
            ])),
        );

        assert!(matches!(
            revalidate_gateway_send_admission(&revalidation_state, &plan(), &admission_guard).await,
            GatewaySendAdmissionDecision::Admit(())
        ));

        let mut rpm_exhausted = concurrent_limited_key();
        rpm_exhausted.rpm_limit = Some(1);
        repository
            .update_key(&rpm_exhausted)
            .await
            .expect("set one-RPM limit after first send");
        assert!(matches!(
            revalidate_gateway_send_admission(&revalidation_state, &plan(), &admission_guard).await,
            GatewaySendAdmissionDecision::Skip(skip)
                if skip.reason() == SendAdmissionSkipReason::CredentialRpmExhausted
        ));
    }

    #[tokio::test]
    async fn strong_read_rejects_endpoint_health_exhausted_after_plan_selection() {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider()],
            vec![endpoint()],
            vec![key()],
        ));
        let state = state_with_repository(Arc::clone(&repository));
        assert!(matches!(
            request_gateway_send_admission(&state, &plan()).await,
            GatewaySendAdmissionDecision::Admit(_)
        ));

        let mut unhealthy = endpoint();
        unhealthy.health_score = 0.0;
        repository
            .update_endpoint(&unhealthy)
            .await
            .expect("exhaust endpoint health");

        assert!(matches!(
            request_gateway_send_admission(&state, &plan()).await,
            GatewaySendAdmissionDecision::Skip(skip)
                if skip.reason() == SendAdmissionSkipReason::EndpointUnhealthy
        ));
    }

    #[tokio::test]
    async fn authority_open_failure_stops_dispatch() {
        let mut unreadable_key = key();
        unreadable_key.encrypted_api_key = Some("not-valid-ciphertext".to_string());
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider()],
            vec![endpoint()],
            vec![unreadable_key],
        ));
        let state = state_with_repository(repository);

        assert!(matches!(
            request_gateway_send_admission(&state, &plan()).await,
            GatewaySendAdmissionDecision::Stop(stop)
                if stop.reason() == SendAdmissionStopReason::AuthorityReadFailed
        ));
    }

    #[tokio::test]
    #[ignore = "requires a shared isolated Redis server"]
    async fn redis_two_gateways_admit_one_due_half_open_probe_and_recheck_authority() {
        let redis_url = std::env::var("AETHER_TEST_REDIS_URL")
            .ok()
            .filter(|url| !url.trim().is_empty());
        let mut managed_redis = if redis_url.is_none() {
            Some(
                ManagedRedisServer::start()
                    .await
                    .expect("ignored Redis send-admission test requires an isolated Redis server"),
            )
        } else {
            None
        };
        let redis_url = redis_url.unwrap_or_else(|| {
            managed_redis
                .as_ref()
                .expect("managed Redis server should be available")
                .redis_url()
                .to_owned()
        });
        let key_prefix = format!("aether-send-admission-test-{}", uuid::Uuid::new_v4());
        let runtime = || async {
            Arc::new(
                RuntimeState::redis(
                    RedisClientConfig {
                        url: redis_url.clone(),
                        key_prefix: Some(key_prefix.clone()),
                    },
                    Some(1_000),
                )
                .await
                .expect("gateway RuntimeState should connect to the shared Redis namespace"),
            )
        };
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider()],
            vec![endpoint()],
            vec![due_probe_key()],
        ));
        let first_gateway = state_with_repository_and_runtime(
            Arc::clone(&repository),
            runtime().await,
            "send-admission-gateway-a",
        );
        let second_gateway = state_with_repository_and_runtime(
            Arc::clone(&repository),
            runtime().await,
            "send-admission-gateway-b",
        );

        let selected_plan = plan();
        let (first, second) = tokio::join!(
            request_gateway_send_admission(&first_gateway, &selected_plan),
            request_gateway_send_admission(&second_gateway, &selected_plan)
        );
        let (owner, waiter, guard) = match (first, second) {
            (
                GatewaySendAdmissionDecision::Admit(guard),
                GatewaySendAdmissionDecision::Skip(skip),
            ) if skip.reason() == SendAdmissionSkipReason::CredentialCircuitOpen => {
                (&first_gateway, &second_gateway, guard)
            }
            (
                GatewaySendAdmissionDecision::Skip(skip),
                GatewaySendAdmissionDecision::Admit(guard),
            ) if skip.reason() == SendAdmissionSkipReason::CredentialCircuitOpen => {
                (&second_gateway, &first_gateway, guard)
            }
            _ => panic!("two gateways sharing Redis must admit exactly one due half-open probe"),
        };
        assert!(guard.ensure_alive().is_ok());
        drop(guard);

        let replacement_guard = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            async {
                loop {
                    match request_gateway_send_admission(waiter, &plan()).await {
                        GatewaySendAdmissionDecision::Admit(guard) => break guard,
                        GatewaySendAdmissionDecision::Skip(skip)
                            if skip.reason() == SendAdmissionSkipReason::CredentialCircuitOpen =>
                        {
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                        _ => {
                            panic!(
                                "the waiting gateway should admit after the first admission guard releases"
                            );
                        }
                    }
                }
            },
        )
        .await
        .expect("the first admission guard should release its shared Redis probe promptly");
        assert!(replacement_guard.ensure_alive().is_ok());
        drop(replacement_guard);

        let mut disabled = due_probe_key();
        disabled.is_active = false;
        repository
            .update_key(&disabled)
            .await
            .expect("disable key after plan selection");
        assert!(matches!(
            request_gateway_send_admission(owner, &plan()).await,
            GatewaySendAdmissionDecision::Skip(skip)
                if skip.reason() == SendAdmissionSkipReason::CredentialInactive
        ));
        repository
            .update_key(&due_probe_key())
            .await
            .expect("restore due key before simulating Redis outage");
        if let Some(redis) = managed_redis.as_mut() {
            redis
                .stop()
                .expect("isolated Redis server should stop for admission outage");
            assert!(matches!(
                request_gateway_send_admission(owner, &plan()).await,
                GatewaySendAdmissionDecision::Stop(stop)
                    if stop.reason() == SendAdmissionStopReason::ReservationBackendUnavailable
            ));
        }
        drop(managed_redis);
    }

    #[tokio::test]
    #[ignore = "requires a shared isolated Redis server"]
    async fn redis_two_gateways_key_concurrent_limit_admits_one_then_recovers_after_release() {
        let redis_url = std::env::var("AETHER_TEST_REDIS_URL")
            .ok()
            .filter(|url| !url.trim().is_empty());
        let managed_redis = if redis_url.is_none() {
            Some(
                ManagedRedisServer::start()
                    .await
                    .expect("ignored Redis send-admission test requires an isolated Redis server"),
            )
        } else {
            None
        };
        let redis_url = redis_url.unwrap_or_else(|| {
            managed_redis
                .as_ref()
                .expect("managed Redis server should be available")
                .redis_url()
                .to_owned()
        });
        let key_prefix = format!(
            "aether-send-admission-key-concurrency-test-{}",
            uuid::Uuid::new_v4()
        );
        let runtime = || async {
            Arc::new(
                RuntimeState::redis(
                    RedisClientConfig {
                        url: redis_url.clone(),
                        key_prefix: Some(key_prefix.clone()),
                    },
                    Some(1_000),
                )
                .await
                .expect("gateway RuntimeState should connect to the shared Redis namespace"),
            )
        };
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider()],
            vec![endpoint()],
            vec![concurrent_limited_key()],
        ));
        let first_gateway = state_with_repository_and_runtime(
            Arc::clone(&repository),
            runtime().await,
            "send-admission-key-concurrency-gateway-a",
        );
        let second_gateway = state_with_repository_and_runtime(
            Arc::clone(&repository),
            runtime().await,
            "send-admission-key-concurrency-gateway-b",
        );

        let selected_plan = plan();
        // Simultaneous pool selection can arrive at either Gateway. Retain the
        // winning guard until every contender has observed the shared permit.
        let outcomes = futures_util::future::join_all((0..16).map(|index| {
            let gateway = if index % 2 == 0 {
                &first_gateway
            } else {
                &second_gateway
            };
            request_gateway_send_admission(gateway, &selected_plan)
        }))
        .await;
        let mut guards = Vec::new();
        let mut exhausted = 0usize;
        for outcome in outcomes {
            match outcome {
                GatewaySendAdmissionDecision::Admit(guard) => guards.push(guard),
                GatewaySendAdmissionDecision::Skip(skip)
                    if skip.reason()
                        == SendAdmissionSkipReason::CredentialConcurrencyExhausted =>
                {
                    exhausted += 1;
                }
                _ => panic!(
                    "every pool contender must either hold the only key permit or be concurrency exhausted"
                ),
            }
        }
        assert_eq!(
            guards.len(),
            1,
            "exactly one gateway request may hold the key permit"
        );
        assert_eq!(
            exhausted, 15,
            "all other concurrent requests must observe the shared limit"
        );
        let guard = guards.pop().expect("one shared key permit owner");
        assert!(guard.ensure_alive().is_ok());
        drop(guard);

        let replacement_guard = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                match request_gateway_send_admission(&second_gateway, &plan()).await {
                    GatewaySendAdmissionDecision::Admit(guard) => break guard,
                    GatewaySendAdmissionDecision::Skip(skip)
                        if skip.reason()
                            == SendAdmissionSkipReason::CredentialConcurrencyExhausted =>
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    _ => panic!(
                        "the waiting gateway should admit after the first key reservation releases"
                    ),
                }
            }
        })
        .await
        .expect("the first admission guard should release its shared Redis key permit promptly");
        assert!(replacement_guard.ensure_alive().is_ok());
        drop(replacement_guard);
        drop(managed_redis);
    }
}
