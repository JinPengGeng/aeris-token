//! Financial admission for one independently billable, synchronous image operation.
//! Capabilities live in server execution state, never in report metadata.
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aether_billing::{
    BillingImageAuthorizationInput, BillingImageAuthorizationQuote, BillingImageOutputDimensions,
    BillingModelPricingSnapshot, BillingService,
};
use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::settlement::{
    RequestAttemptExecutionFacts, RequestAttemptExecutionStatus, RequestAttemptFundsIdentity,
    RequestAttemptProvider, RequestFundsIdentity, ReserveRequestAttemptFundsInput,
    ReserveRequestAttemptFundsOutcome, ReserveRequestFundsInput,
};
use aether_usage_runtime::{
    build_lifecycle_usage_seed, build_sync_terminal_usage_event,
    build_usage_event_data_seed_describing_request_bodies, UsageAttemptChargeEvidence,
    UsageAttemptFundsAction, UsageAttemptFundsEvent, UsageAttemptFundsRetention,
    UsageAttemptImageEvidence, UsageEvent, UsageEventData, UsageEventType,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::control::{GatewayControlAuthContext, GatewayControlDecision};
use crate::plan_usage_policy::PlanUsageReservationContext;
use crate::usage::GatewaySyncReportRequest;
use crate::{AppState, GatewayError};

tokio::task_local! {
    static REQUEST: Arc<RequestAdmission>;
    static ATTEMPT: Option<Arc<FundedImageAttempt>>;
}

fn unavailable(message: &'static str) -> GatewayError {
    GatewayError::Internal(message.to_string())
}

fn unsupported(message: &'static str) -> GatewayError {
    GatewayError::Client {
        status: http::StatusCode::UNPROCESSABLE_ENTITY,
        message: message.to_string(),
    }
}

struct FrozenImageQuote {
    quote: BillingImageAuthorizationQuote,
    model_id: Option<String>,
}

fn is_image_plan(plan: &ExecutionPlan, context: Option<&Value>) -> bool {
    plan.provider_api_format
        .eq_ignore_ascii_case("openai:image")
        || plan.client_api_format.eq_ignore_ascii_case("openai:image")
        || context
            .and_then(|value| value.get("image_request"))
            .is_some()
}

fn quote_key(
    plan: &ExecutionPlan,
    auth: &GatewayControlAuthContext,
    context: Option<&Value>,
) -> Result<Vec<u8>, GatewayError> {
    let mut projection = serde_json::to_value(plan)
        .map_err(|_| unavailable("image execution projection serialization failed"))?;
    // This bookkeeping slot can be assigned between candidate admission and
    // execution. Every transport/body/provider/owner field still binds the quote.
    projection
        .as_object_mut()
        .expect("execution plan is an object")
        .remove("candidate_id");
    let material = serde_json::to_vec(&(
        projection,
        &auth.user_id,
        &auth.api_key_id,
        auth.api_key_is_standalone,
        auth.api_key_billing_multiplier,
        context.and_then(|value| value.get("provider_type")),
        context.and_then(|value| value.get("chatgpt_web_image")),
        context.and_then(|value| value.get(aether_ai_serving::UPSTREAM_IS_STREAM_KEY)),
        context.and_then(|value| value.get("model_id")),
        context.and_then(|value| value.get("global_model_name")),
    ))
    .map_err(|_| unavailable("image authorization identity serialization failed"))?;
    Ok(Sha256::digest(material).to_vec())
}

async fn resolve_image_quote(
    state: &AppState,
    plan: &ExecutionPlan,
    auth: &GatewayControlAuthContext,
    context: Option<&Value>,
) -> Result<Option<FrozenImageQuote>, GatewayError> {
    let field = |name| context.and_then(|v| v.get(name)).and_then(Value::as_str);
    let pricing = if let Some(model_id) = field("model_id") {
        state
            .data
            .find_billing_model_context_by_model_id(&plan.provider_id, Some(&plan.key_id), model_id)
            .await
    } else if let Some(model_name) = field("global_model_name") {
        state
            .data
            .find_billing_model_context(&plan.provider_id, Some(&plan.key_id), model_name)
            .await
    } else {
        return Err(unsupported(
            "Image billing requires a resolved model and explicit pricing",
        ));
    }
    .map_err(GatewayError::from_data_layer_error)?
    .ok_or_else(|| unsupported("Image billing pricing is unavailable"))?;
    let pricing = BillingModelPricingSnapshot::from(pricing);
    if pricing.is_free_tier() {
        return Ok(None);
    }
    request_policy_context(context)?;
    if field("provider_type").is_some_and(|kind| {
        matches!(
            kind.to_ascii_lowercase().as_str(),
            "grok" | "chatgpt_web" | "windsurf" | "codex"
        )
    }) || context
        .and_then(|v| v.get("chatgpt_web_image"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || context
            .and_then(|v| v.get(aether_ai_serving::UPSTREAM_IS_STREAM_KEY))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Err(unsupported(
            "Paid multi-stage or streamed image execution has no supported billing boundary",
        ));
    }
    let quote = quote_final_image_projection(plan, &pricing, auth.api_key_billing_multiplier)
        .map_err(|_| unsupported("Paid images require a supported fixed-output JSON request with a provable cost bound"))?;
    Ok(Some(FrozenImageQuote {
        quote,
        model_id: pricing.model_id,
    }))
}

/// Runs before standalone/unlimited balance shortcuts. The capability remains
/// in server request state and is consumed only by the funded sync executor.
pub(crate) async fn authorize_image_plan(
    state: &AppState,
    plan: &ExecutionPlan,
    decision: &GatewayControlDecision,
    context: Option<&Value>,
) -> Result<bool, GatewayError> {
    if !is_image_plan(plan, context) {
        return Ok(false);
    }
    let Some(auth) = decision.auth_context.as_ref() else {
        return Err(unsupported(
            "Image billing requires authenticated request ownership",
        ));
    };
    let Some(quote) = resolve_image_quote(state, plan, auth, context).await? else {
        return Ok(true);
    };
    if !state.usage_runtime.is_enabled()
        || !state.data.has_usage_writer()
        || !state.data.has_settlement_writer()
    {
        return Err(unavailable("paid image billing persistence is unavailable"));
    }
    let key = quote_key(plan, auth, context)?;
    REQUEST
        .try_with(|request| {
            request
                .financial
                .lock()
                .expect("request funds mutex poisoned")
                .quotes
                .insert(key, quote);
        })
        .map_err(|_| unsupported("Paid images require the local synchronous funding path"))?;
    Ok(true)
}

#[derive(Default)]
struct RequestFinancialState {
    quotes: BTreeMap<Vec<u8>, FrozenImageQuote>,
    active_owners: usize,
    preparations: usize,
    cancelled: bool,
    latest: Option<RequestAttemptFundsIdentity>,
    terminal: Option<UsageEvent>,
    fallback_terminal: Option<UsageEvent>,
    terminal_timestamp_ms: Option<u64>,
}

struct RequestAdmission {
    state: AppState,
    usage_policy: Option<PlanUsageReservationContext>,
    financial: Mutex<RequestFinancialState>,
    preparation_finished: tokio::sync::Notify,
}

impl RequestAdmission {
    fn record_rejection(
        &self,
        request_id: &str,
        mut data: UsageEventData,
        status: u16,
        category: &str,
        message: &str,
    ) {
        let mut financial = self.financial.lock().expect("request funds mutex poisoned");
        // A denied reservation has sent no work. When earlier attempts exist,
        // use their durable identity and keep their financial facts authoritative.
        data.attempt_funds = financial.latest.as_ref().map(|identity| {
            Box::new(UsageAttemptFundsEvent {
                schema_version: 1,
                identity: identity.clone(),
                action: UsageAttemptFundsAction::ParentLifecycle,
            })
        });
        if data.attempt_funds.is_none() {
            if let Some(metadata) = data
                .request_metadata
                .as_mut()
                .and_then(Value::as_object_mut)
            {
                metadata.remove("plan_usage_reservation_token");
            }
            data.total_cost_usd = Some(0.0);
            data.actual_total_cost_usd = Some(0.0);
        }
        data.status_code = Some(status);
        data.error_category = Some(category.to_string());
        data.error_message = Some(message.to_string());
        financial.terminal = Some(UsageEvent::new(UsageEventType::Failed, request_id, data));
        financial.terminal_timestamp_ms = None;
    }

    async fn close(&self, cancelled: bool) -> Result<(), GatewayError> {
        loop {
            let notified = self.preparation_finished.notified();
            if self
                .financial
                .lock()
                .expect("request funds mutex poisoned")
                .preparations
                == 0
            {
                break;
            }
            // A cancelled caller cannot race admission close ahead of an
            // in-flight reserve commit whose result is still being delivered.
            notified.await;
        }
        let (identity, terminal, scope_cancelled) = {
            let financial = self.financial.lock().expect("request funds mutex poisoned");
            (
                financial.latest.clone(),
                financial
                    .terminal
                    .clone()
                    .or_else(|| financial.fallback_terminal.clone()),
                financial.cancelled,
            )
        };
        let cancelled = cancelled || scope_cancelled;
        // Close is awaited even if the final public usage update fails. Unknown
        // dispatched holds remain owned by their persisted reservations.
        if let Some(identity) = identity {
            self.state
                .data
                .close_request_attempt_admission(identity)
                .await
                .map_err(GatewayError::from_data_layer_error)?;
        }
        if let Some(mut event) = terminal {
            // This is the terminal observation for the entire request, after
            // all candidate sources and admission close. A reserve-time fallback
            // timestamp may precede the durable pending row; freeze it only now.
            event.timestamp_ms = *self
                .financial
                .lock()
                .expect("request funds mutex poisoned")
                .terminal_timestamp_ms
                .get_or_insert_with(crate::clock::current_unix_ms);
            if cancelled {
                event.event_type = UsageEventType::Cancelled;
                event.data.status_code = Some(499);
                event.data.error_category = Some("cancelled".to_string());
                event.data.error_message = Some("Image request was cancelled".to_string());
            }
            if event.data.attempt_funds.is_some() {
                self.state
                    .usage_runtime
                    .persist_attempt_funds_event(
                        self.state.usage_lifecycle_data_state().as_ref(),
                        event,
                    )
                    .await
                    .map_err(GatewayError::from_data_layer_error)?;
            } else {
                // The persisted pending row still needs a terminal when the
                // first reservation is denied. No financial attempt exists.
                aether_usage_runtime::write_event_record(
                    self.state.usage_lifecycle_data_state().as_ref(),
                    &event,
                )
                .await
                .map_err(GatewayError::from_data_layer_error)?;
            }
            self.financial
                .lock()
                .expect("request funds mutex poisoned")
                .terminal
                .take();
        }
        Ok(())
    }
}

struct RequestAdmissionGuard {
    request: Option<Arc<RequestAdmission>>,
    cancelled: bool,
    owns_scope: bool,
}

impl RequestAdmissionGuard {
    fn new(request: Arc<RequestAdmission>) -> Self {
        request
            .financial
            .lock()
            .expect("request funds mutex poisoned")
            .active_owners += 1;
        Self {
            request: Some(request),
            cancelled: true,
            owns_scope: true,
        }
    }

    fn release_scope(&mut self) {
        if !self.owns_scope {
            return;
        }
        self.owns_scope = false;
        let last = {
            let mut financial = self
                .request
                .as_ref()
                .expect("an owned scope has a request")
                .financial
                .lock()
                .expect("request funds mutex poisoned");
            financial.active_owners -= 1;
            financial.active_owners == 0
        };
        if !last {
            self.request = None;
        }
    }

    async fn finish(&mut self) -> Result<(), GatewayError> {
        self.cancelled = false;
        self.release_scope();
        if let Some(request) = self.request.as_ref() {
            request.close(false).await?;
            self.request = None;
        }
        Ok(())
    }
}

impl Drop for RequestAdmissionGuard {
    fn drop(&mut self) {
        self.release_scope();
        let Some(request) = self.request.take() else {
            return;
        };
        let cancelled = self.cancelled;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let producer = request.state.usage_runtime.track_producer();
            handle.spawn(async move {
                let _producer = producer;
                if request.close(cancelled).await.is_err() {
                    tracing::error!(
                        event_name = "image_funds_admission_close_failed",
                        "request funds admission requires reconciliation"
                    );
                }
            });
        }
    }
}

struct PreparationGuard(Arc<RequestAdmission>);

impl Drop for PreparationGuard {
    fn drop(&mut self) {
        self.0
            .financial
            .lock()
            .expect("request funds mutex poisoned")
            .preparations -= 1;
        self.0.preparation_finished.notify_one();
    }
}

fn new_request(
    state: &AppState,
    usage_policy: Option<PlanUsageReservationContext>,
) -> Arc<RequestAdmission> {
    Arc::new(RequestAdmission {
        state: state.clone(),
        usage_policy,
        financial: Mutex::new(RequestFinancialState::default()),
        preparation_finished: tokio::sync::Notify::new(),
    })
}

/// Acquire the child owner before spawning: task locals are not inherited and
/// returning response headers must not close admission ahead of paid body work.
pub(crate) fn spawn_request_scope<T: Send + 'static>(
    state: &AppState,
    future: impl Future<Output = Result<T, GatewayError>> + Send + 'static,
) -> tokio::task::JoinHandle<Result<T, GatewayError>> {
    let request = REQUEST
        .try_with(Arc::clone)
        .unwrap_or_else(|_| new_request(state, None));
    let guard = RequestAdmissionGuard::new(request.clone());
    tokio::spawn(owned_request_scope(request, guard, future))
}

/// One scope surrounds the candidate loop, preserving the external request ID.
pub(crate) async fn request_scope<T>(
    state: &AppState,
    future: impl Future<Output = Result<T, GatewayError>>,
) -> Result<T, GatewayError> {
    if REQUEST.try_with(|_| ()).is_ok() {
        return future.await;
    }
    let request = new_request(state, None);
    let guard = RequestAdmissionGuard::new(request.clone());
    owned_request_scope(request, guard, future).await
}

/// Only request extensions supply policy authority. Nested scopes and heartbeat
/// tasks inherit it; report metadata cannot replace the original context.
pub(crate) async fn request_scope_with_policy<T>(
    state: &AppState,
    usage_policy: Option<&PlanUsageReservationContext>,
    future: impl Future<Output = Result<T, GatewayError>>,
) -> Result<T, GatewayError> {
    if let Ok(request) = REQUEST.try_with(Arc::clone) {
        if usage_policy.is_some() && request.usage_policy.as_ref() != usage_policy {
            return Err(unavailable(
                "image request policy context changed within admission",
            ));
        }
        return future.await;
    }
    let request = new_request(state, usage_policy.cloned());
    let guard = RequestAdmissionGuard::new(request.clone());
    owned_request_scope(request, guard, future).await
}

fn request_policy_context(
    context: Option<&Value>,
) -> Result<Option<PlanUsageReservationContext>, GatewayError> {
    let policy = REQUEST
        .try_with(|request| request.usage_policy.clone())
        .ok()
        .flatten();
    if let Some(token) = context
        .and_then(|value| value.get("plan_usage_reservation_token"))
        .and_then(Value::as_str)
    {
        if policy.as_ref().is_none_or(|policy| policy.token() != token) {
            return Err(unsupported(
                "Paid image quota requires the trusted request admission context",
            ));
        }
    }
    Ok(policy)
}

/// A quote admitted by the image gate owns atomic attempt quota admission.
/// Legacy candidate cost reservation must not also own this operation.
pub(crate) fn has_funded_image_quote(
    plan: &ExecutionPlan,
    decision: &GatewayControlDecision,
    context: Option<&Value>,
) -> Result<bool, GatewayError> {
    let Some(auth) = decision
        .auth_context
        .as_ref()
        .filter(|_| is_image_plan(plan, context))
    else {
        return Ok(false);
    };
    let key = quote_key(plan, auth, context)?;
    Ok(REQUEST
        .try_with(|request| {
            request
                .financial
                .lock()
                .expect("request funds mutex poisoned")
                .quotes
                .contains_key(&key)
        })
        .unwrap_or(false))
}

async fn owned_request_scope<T>(
    request: Arc<RequestAdmission>,
    mut guard: RequestAdmissionGuard,
    future: impl Future<Output = Result<T, GatewayError>>,
) -> Result<T, GatewayError> {
    let result = REQUEST.scope(request, future).await;
    let closed = guard.finish().await;
    match result {
        Err(error) => Err(error),
        Ok(value) => {
            closed?;
            Ok(value)
        }
    }
}

pub(crate) async fn attempt_scope<T>(
    attempt: Option<Arc<FundedImageAttempt>>,
    future: impl Future<Output = T>,
) -> T {
    ATTEMPT.scope(attempt, future).await
}

pub(crate) fn current_attempt() -> Option<Arc<FundedImageAttempt>> {
    ATTEMPT.try_with(Clone::clone).ok().flatten()
}

pub(crate) fn mark_request_cancelled() {
    let _ = REQUEST.try_with(|request| {
        request
            .financial
            .lock()
            .expect("request funds mutex poisoned")
            .cancelled = true;
    });
}

pub(crate) struct FundedImageAttempt {
    state: AppState,
    pub(crate) identity: RequestAttemptFundsIdentity,
    started: Instant,
    terminal: Mutex<Option<UsageEvent>>,
    pending: Mutex<Option<UsageEvent>>,
}

impl Drop for FundedImageAttempt {
    fn drop(&mut self) {
        if self
            .terminal
            .get_mut()
            .expect("attempt funds mutex poisoned")
            .is_some()
        {
            return;
        }
        let state = self.state.clone();
        let identity = self.identity.clone();
        let pending = self
            .pending
            .get_mut()
            .expect("attempt funds mutex poisoned")
            .clone();
        let elapsed = self
            .started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let producer = state.usage_runtime.track_producer();
            handle.spawn(async move {
                let _producer = producer;
                let event = if let Some(event) = pending {
                    event
                } else {
                    let Ok(stored) = state
                        .data
                        .read_request_attempt_funds(identity.clone())
                        .await
                    else {
                        return;
                    };
                    let evidence = if stored.dispatched_at_unix_secs.is_some() {
                        UsageAttemptChargeEvidence::Unknown
                    } else {
                        UsageAttemptChargeEvidence::NoCharge {
                            reason: "prepared_cancelled".into(),
                        }
                    };
                    UsageEvent::new(
                        UsageEventType::Cancelled,
                        identity.request.request_id.clone(),
                        UsageEventData {
                            attempt_funds: Some(Box::new(UsageAttemptFundsEvent {
                                schema_version: 1,
                                identity,
                                action: UsageAttemptFundsAction::Outcome {
                                    execution: RequestAttemptExecutionFacts {
                                        status: RequestAttemptExecutionStatus::Cancelled,
                                        response_time_ms: elapsed,
                                    },
                                    evidence,
                                },
                            })),
                            ..UsageEventData::default()
                        },
                    )
                };
                if state
                    .usage_runtime
                    .persist_attempt_funds_event(
                        state.usage_lifecycle_data_state().as_ref(),
                        event.clone(),
                    )
                    .await
                    .is_err()
                {
                    // Only Persisted acknowledges a financial commit. Queue or
                    // local retry acceptance retains work without promising disk durability.
                    match state
                        .usage_runtime
                        .defer_attempt_funds_event(
                            state.usage_lifecycle_data_state().as_ref(),
                            event,
                        )
                        .await
                    {
                        Ok(UsageAttemptFundsRetention::Persisted) => tracing::debug!(
                            event_name = "image_funds_drop_recovery_committed",
                            "image financial outcome committed during recovery"
                        ),
                        Ok(UsageAttemptFundsRetention::Queued) => tracing::warn!(
                            event_name = "image_funds_drop_recovery_queued",
                            "image financial outcome accepted by queue; commit remains pending"
                        ),
                        Ok(UsageAttemptFundsRetention::BufferedForRetry) => tracing::warn!(
                            event_name = "image_funds_drop_recovery_buffered",
                            "image financial outcome retained in local retry buffer; commit remains pending"
                        ),
                        Err(_) => tracing::error!(
                            event_name = "image_funds_drop_recovery_failed",
                            "image financial outcome was neither committed nor retained for retry"
                        ),
                    }
                }
            });
        }
    }
}

impl FundedImageAttempt {
    pub(crate) async fn prepare(
        state: &AppState,
        plan: &ExecutionPlan,
        decision: &GatewayControlDecision,
        report_context: Option<&Value>,
    ) -> Result<Option<Arc<Self>>, GatewayError> {
        let is_image = is_image_plan(plan, report_context);
        let Some(auth) = decision.auth_context.as_ref().filter(|_| is_image) else {
            return Ok(None);
        };
        REQUEST
            .try_with(|_| ())
            .map_err(|_| unavailable("image operation requires a request admission scope"))?;
        let key = quote_key(plan, auth, report_context)?;
        let frozen = REQUEST
            .try_with(|request| {
                request
                    .financial
                    .lock()
                    .expect("request funds mutex poisoned")
                    .quotes
                    .remove(&key)
            })
            .map_err(|_| unavailable("image operation requires a request admission scope"))?;
        let frozen = match frozen {
            Some(frozen) => frozen,
            None => match resolve_image_quote(state, plan, auth, report_context).await? {
                Some(frozen) => frozen,
                None => return Ok(None),
            },
        };
        let FrozenImageQuote { quote, model_id } = frozen;
        let usage_policy_context = request_policy_context(report_context)?;
        let usage_policy = match usage_policy_context.as_ref() {
            Some(policy) if !auth.admin_bypass_limits && !auth.api_key_is_standalone => {
                if policy.subject_id() != auth.user_id {
                    return Err(unavailable(
                        "image quota subject does not match request owner",
                    ));
                }
                Some(policy.attempt_funds_policy()?)
            }
            _ => None,
        };
        let mut seed = build_lifecycle_usage_seed(plan, report_context);
        // Standalone keys still belong to their creating user. The standalone
        // flag selects the key wallet; removing the owner breaks the repository
        // identity check and must not be used to choose the funding source.
        seed.user_id = Some(auth.user_id.clone());
        seed.api_key_id = Some(auth.api_key_id.clone());
        seed.api_key_billing_multiplier = Some(auth.api_key_billing_multiplier);
        seed.request_metadata
            .get_or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .expect("lifecycle request metadata is an object")
            .insert(
                "api_key_is_standalone".into(),
                Value::Bool(auth.api_key_is_standalone),
            );
        let input = ReserveRequestAttemptFundsInput {
            usage_policy,
            attempt_id: uuid::Uuid::new_v4().to_string(),
            provider: RequestAttemptProvider {
                provider_id: plan.provider_id.clone(),
                provider_api_key_id: Some(plan.key_id.clone()),
                model_id,
                candidate_id: plan.candidate_id.clone(),
            },
            quote: ReserveRequestFundsInput {
                identity: RequestFundsIdentity {
                    reservation_token: uuid::Uuid::new_v4().to_string(),
                    request_id: plan.request_id.clone(),
                    user_id: Some(auth.user_id.clone()),
                    api_key_id: Some(auth.api_key_id.clone()),
                    api_key_is_standalone: auth.api_key_is_standalone,
                },
                authorized_cost_units: u64::try_from(quote.upper_bound_units())
                    .map_err(|_| unavailable("invalid image authorization ceiling"))?,
                pricing_snapshot: serde_json::to_value(&quote)
                    .map_err(|_| unavailable("image quote serialization failed"))?,
                admitted_at_unix_secs: crate::clock::current_unix_ms() / 1000,
            },
        };
        let identity = input.identity();
        let mut fallback_data =
            build_usage_event_data_seed_describing_request_bodies(plan, report_context);
        fallback_data.user_id = identity.request.user_id.clone();
        fallback_data.api_key_id = identity.request.api_key_id.clone();
        fallback_data.status_code = Some(502);
        fallback_data.error_category = Some("upstream_error".to_string());
        fallback_data.error_message =
            Some("Image request ended without a completed response".to_string());
        fallback_data.attempt_funds = Some(Box::new(UsageAttemptFundsEvent {
            schema_version: 1,
            identity: identity.clone(),
            action: UsageAttemptFundsAction::ParentLifecycle,
        }));
        let request = REQUEST
            .try_with(Arc::clone)
            .map_err(|_| unavailable("image operation requires a request admission scope"))?;
        request
            .financial
            .lock()
            .expect("request funds mutex poisoned")
            .preparations += 1;
        let preparation = PreparationGuard(request.clone());
        let state = state.clone();
        let producer = state.usage_runtime.track_producer();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        // Shield durable admission through handoff. Dropping the caller only
        // closes the receiver; this tracked task still learns the reserve result
        // and releases a committed Prepared reservation before admission closes.
        tokio::spawn(async move {
            let _producer = producer;
            let _preparation = preparation;
            let result = async {
                state
                    .usage_runtime
                    .admit_pending_durable(state.usage_lifecycle_data_state().as_ref(), seed)
                    .await
                    .map_err(GatewayError::from_data_layer_error)?;
                match state
                    .data
                    .reserve_request_attempt_funds(input)
                    .await
                    .map_err(GatewayError::from_data_layer_error)?
                {
                    ReserveRequestAttemptFundsOutcome::Reserved { .. } => {}
                    ReserveRequestAttemptFundsOutcome::Insufficient { .. } => {
                        request.record_rejection(
                            &identity.request.request_id,
                            fallback_data.clone(),
                            402,
                            "insufficient_quota",
                            "Insufficient available balance for image authorization",
                        );
                        return Err(GatewayError::Client {
                            status: http::StatusCode::PAYMENT_REQUIRED,
                            message: "Insufficient available balance for image authorization"
                                .into(),
                        });
                    }
                    ReserveRequestAttemptFundsOutcome::UsagePolicyRejected {
                        window_index,
                        limit_cost_units,
                        ..
                    } => {
                        let policy = usage_policy_context.as_ref().ok_or_else(|| {
                            unavailable("image quota rejection has no admitted policy")
                        })?;
                        request.record_rejection(
                            &identity.request.request_id,
                            fallback_data.clone(),
                            429,
                            "plan_usage_limit_exceeded",
                            "Plan cost allowance is insufficient for image authorization",
                        );
                        return Err(GatewayError::PlanUsageLimited(
                            policy.attempt_cost_rejection(window_index, limit_cost_units)?,
                        ));
                    }
                    _ => return Err(unavailable("image funds reservation was denied")),
                }
                let mut financial = request
                    .financial
                    .lock()
                    .expect("request funds mutex poisoned");
                financial.latest = Some(identity.clone());
                // A newly admitted operation owns the eventual client outcome.
                // Do not replay the previous candidate's failure if it is cancelled.
                financial.terminal = None;
                financial.terminal_timestamp_ms = None;
                financial.fallback_terminal = Some(UsageEvent::new(
                    UsageEventType::Failed,
                    identity.request.request_id.clone(),
                    fallback_data,
                ));
                Ok(Arc::new(Self {
                    state: state.clone(),
                    identity,
                    started: Instant::now(),
                    terminal: Mutex::new(None),
                    pending: Mutex::new(None),
                }))
            }
            .await;
            if let Err(Ok(attempt)) = sender.send(result) {
                // The client cancelled while its reserve was committing. No
                // upstream operation has started; cleanup uses the stored fact.
                let _ = attempt.finish_unobserved(true).await;
            }
        });
        receiver
            .await
            .map_err(|_| unavailable("image admission task ended before reserve handoff"))?
            .map(Some)
    }

    pub(crate) async fn dispatch(&self) -> Result<(), GatewayError> {
        self.state
            .data
            .dispatch_request_attempt_funds(self.identity.clone())
            .await
            .map_err(GatewayError::from_data_layer_error)?;
        Ok(())
    }

    pub(crate) async fn observe(
        &self,
        plan: &ExecutionPlan,
        context: Option<&Value>,
        payload: &GatewaySyncReportRequest,
    ) -> Result<(), GatewayError> {
        let mut parent = build_sync_terminal_usage_event(plan, context, payload)
            .map_err(GatewayError::from_data_layer_error)?;
        let execution = RequestAttemptExecutionFacts {
            status: match parent.event_type {
                UsageEventType::Completed => RequestAttemptExecutionStatus::Completed,
                UsageEventType::Cancelled => RequestAttemptExecutionStatus::Cancelled,
                _ => RequestAttemptExecutionStatus::Failed,
            },
            response_time_ms: self
                .started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        };
        let evidence = image_output_evidence(payload.body_json.as_ref());
        let outcome = self.event(execution, evidence);
        *self.pending.lock().expect("attempt funds mutex poisoned") = Some(outcome.clone());
        parent.data.attempt_funds = Some(Box::new(UsageAttemptFundsEvent {
            schema_version: 1,
            identity: self.identity.clone(),
            action: UsageAttemptFundsAction::ParentLifecycle,
        }));
        REQUEST
            .try_with(|request| {
                request
                    .financial
                    .lock()
                    .expect("request funds mutex poisoned")
                    .terminal = Some(parent);
            })
            .map_err(|_| unavailable("image operation lost its request admission scope"))?;
        self.persist(outcome.clone()).await?;
        *self.terminal.lock().expect("attempt funds mutex poisoned") = Some(outcome);
        self.pending
            .lock()
            .expect("attempt funds mutex poisoned")
            .take();
        Ok(())
    }

    fn event(
        &self,
        execution: RequestAttemptExecutionFacts,
        evidence: UsageAttemptChargeEvidence,
    ) -> UsageEvent {
        UsageEvent::new(
            UsageEventType::Failed,
            self.identity.request.request_id.clone(),
            UsageEventData {
                attempt_funds: Some(Box::new(UsageAttemptFundsEvent {
                    schema_version: 1,
                    identity: self.identity.clone(),
                    action: UsageAttemptFundsAction::Outcome {
                        execution,
                        evidence,
                    },
                })),
                ..UsageEventData::default()
            },
        )
    }

    async fn persist(&self, event: UsageEvent) -> Result<(), GatewayError> {
        self.state
            .usage_runtime
            .persist_attempt_funds_event(self.state.usage_lifecycle_data_state().as_ref(), event)
            .await
            .map_err(GatewayError::from_data_layer_error)
    }

    pub(crate) async fn finish_unobserved(&self, cancelled: bool) -> Result<(), GatewayError> {
        if self
            .terminal
            .lock()
            .expect("attempt funds mutex poisoned")
            .is_some()
        {
            return Ok(());
        }
        let pending = self
            .pending
            .lock()
            .expect("attempt funds mutex poisoned")
            .clone();
        if let Some(event) = pending {
            self.persist(event.clone()).await?;
            *self.terminal.lock().expect("attempt funds mutex poisoned") = Some(event);
            self.pending
                .lock()
                .expect("attempt funds mutex poisoned")
                .take();
            return Ok(());
        }
        let stored = self
            .state
            .data
            .read_request_attempt_funds(self.identity.clone())
            .await
            .map_err(GatewayError::from_data_layer_error)?;
        let evidence = if stored.dispatched_at_unix_secs.is_some() {
            UsageAttemptChargeEvidence::Unknown
        } else {
            UsageAttemptChargeEvidence::NoCharge {
                reason: "prepared_cancelled".to_string(),
            }
        };
        let event = self.event(
            RequestAttemptExecutionFacts {
                status: if cancelled {
                    RequestAttemptExecutionStatus::Cancelled
                } else {
                    RequestAttemptExecutionStatus::Failed
                },
                response_time_ms: self
                    .started
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX),
            },
            evidence,
        );
        self.persist(event.clone()).await?;
        *self.terminal.lock().expect("attempt funds mutex poisoned") = Some(event);
        Ok(())
    }
}

/// A request's count and fixed dimensions are taken from the final provider body.
/// Auto/default dimensions, token-priced work and multi-stage adapters stay closed.
fn quote_final_image_projection(
    plan: &ExecutionPlan,
    pricing: &BillingModelPricingSnapshot,
    multiplier: f64,
) -> Result<BillingImageAuthorizationQuote, GatewayError> {
    let body = plan
        .body
        .json_body
        .as_ref()
        .ok_or_else(|| unavailable("image quote requires final JSON projection"))?;
    let url = url::Url::parse(&plan.url).map_err(|_| unavailable("invalid image provider URL"))?;
    let operation = if url.path().ends_with("/images/generations") {
        "generate"
    } else if url.path().ends_with("/images/edits") {
        "edit"
    } else {
        return Err(unavailable(
            "image provider operation has no proven billing boundary",
        ));
    };
    if plan.provider_api_format != "openai:image"
        || plan.stream
        || plan.proxy.is_some()
        || plan.transport_profile.is_some()
        || plan.content_encoding.is_some()
        || plan.body.body_bytes_b64.is_some()
        || !plan.method.eq_ignore_ascii_case("POST")
        || body.get("stream").and_then(Value::as_bool).unwrap_or(false)
        || body.get("service_tier").is_some()
        || body.get("model").and_then(Value::as_str) != pricing.model_provider_model_name.as_deref()
        || plan
            .headers
            .keys()
            .any(|key| key.to_ascii_lowercase().starts_with("x-aether-"))
    {
        return Err(unavailable(
            "image transport has no supported financial boundary",
        ));
    }
    let size = body
        .get("size")
        .and_then(Value::as_str)
        .filter(|v| *v != "auto")
        .ok_or_else(|| unavailable("image authorization requires fixed output size"))?;
    let quality = body
        .get("quality")
        .and_then(Value::as_str)
        .filter(|v| matches!(*v, "low" | "medium" | "high"))
        .ok_or_else(|| unavailable("image authorization requires fixed output quality"))?;
    let count = body
        .get("n")
        .map_or(Some(1), Value::as_u64)
        .filter(|n| (1..=10).contains(n))
        .ok_or_else(|| unavailable("invalid final image count"))? as u32;
    let partial =
        body.get("partial_images")
            .map_or(Some(0), Value::as_u64)
            .filter(|n| *n == 0)
            .ok_or_else(|| unavailable("partial image billing is unsupported"))? as u8;
    let input = BillingImageAuthorizationInput {
        image_count: count,
        max_image_count: count,
        operation: operation.to_string(),
        size: Some(size.to_string()),
        quality: Some(quality.to_string()),
        output_format: body
            .get("output_format")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        partial_images: partial,
        possible_outputs: vec![BillingImageOutputDimensions {
            size: size.to_string(),
            quality: quality.to_string(),
        }],
        api_format: Some(plan.provider_api_format.clone()),
        requested_processing_tier: None,
        api_key_multiplier: multiplier,
        token_bounds: None,
    };
    BillingService::new()
        .quote_image_authorization(pricing, &input)
        .map_err(|_| unavailable("image authorization pricing failed"))?
        .ok_or_else(|| unavailable("image authorization cost is not bounded"))
}

fn image_output_evidence(body: Option<&Value>) -> UsageAttemptChargeEvidence {
    let extract = || {
        let body = body?;
        if body
            .get("service_tier")
            .and_then(Value::as_str)
            .is_some_and(|tier| tier != "standard")
        {
            return None;
        }
        let outputs = body.get("data")?.as_array()?;
        if outputs.is_empty()
            || outputs.len() > 64
            || outputs.iter().any(|output| {
                !["url", "b64_json"].into_iter().any(|key| {
                    output
                        .get(key)
                        .and_then(Value::as_str)
                        .is_some_and(|v| !v.is_empty())
                })
            })
        {
            return None;
        }
        let size = body.get("size")?.as_str()?.to_string();
        let quality = body.get("quality")?.as_str()?.to_string();
        let tokens = |key| {
            body.get("usage")
                .and_then(|v| v.get(key))
                .map_or(Some(0), Value::as_u64)
        };
        Some(UsageAttemptImageEvidence {
            image_count: outputs.len() as u32,
            size,
            quality,
            output_format: body
                .get("output_format")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            input_tokens: tokens("input_tokens")?,
            output_tokens: tokens("output_tokens")?,
            cache_creation_tokens: tokens("cache_creation_tokens")?,
            cache_read_tokens: tokens("cache_read_tokens")?,
        })
    };
    extract().map_or(UsageAttemptChargeEvidence::Unknown, |usage| {
        UsageAttemptChargeEvidence::ImageOutput { usage }
    })
}

#[cfg(test)]
#[path = "funded_image/tests/mod.rs"]
mod tests;
