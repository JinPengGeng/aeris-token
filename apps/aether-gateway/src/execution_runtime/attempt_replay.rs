//! Replay-safety contract for one logical request.
//!
//! Capability construction stays sealed behind [`AttemptReplayHandle`]. The
//! sync/stream scheduler and Responses WebSocket adapters can mark real send,
//! client-commit and quiescence boundaries without minting owners, policy
//! approvals, quiescence proofs or generation permits themselves.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::body::Body;
use axum::response::Response;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

static NEXT_LOGICAL_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct LogicalRequestId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct AttemptGeneration(u64);

impl AttemptGeneration {
    fn next(self) -> Result<Self, AttemptDispatchLifecycleError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(AttemptDispatchLifecycleError::GenerationExhausted)
    }
}

/// One physical upstream dispatch's observable replay-safety state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AttemptDispatchState {
    Prepared,
    SentButUncommitted,
    ClientCommitted,
    Terminal,
}

/// A monotonic reason why this logical request must not be replayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReplayBarrierReason {
    CompactOperation,
    ToolCall,
    SideEffectingRequest,
    AmbiguousDispatchOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReplayBarrier {
    Open,
    Closed(ReplayBarrierReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum AttemptDispatchLifecycleError {
    #[error("invalid attempt dispatch transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: AttemptDispatchState,
        to: AttemptDispatchState,
    },
    #[error("replay is forbidden after client commit")]
    ClientCommitted,
    #[error("replay barrier is closed: {0:?}")]
    ReplayBarrierClosed(ReplayBarrierReason),
    #[error("logical request replay authority is poisoned")]
    ReplayAuthorityPoisoned,
    #[error("the logical request already issued its first attempt")]
    FirstAttemptAlreadyIssued,
    #[error("attempt generation is not the authority's active generation")]
    StaleAttemptGeneration,
    #[error("a physical dispatch is still in flight")]
    DispatchStillInFlight,
    #[error("policy approval does not match the request, generation, or barrier revision")]
    PolicyApprovalMismatch,
    #[error("quiescence proof does not match the request or generation")]
    QuiescenceProofMismatch,
    #[error("a replay permit was already issued for this attempt generation")]
    ReplayPermitAlreadyIssued,
    #[error("the replay permit was already consumed")]
    ReplayPermitAlreadyConsumed,
    #[error("the replay permit is stale")]
    ReplayPermitStale,
    #[error("attempt generation overflowed")]
    GenerationExhausted,
    #[error("no physical dispatch is active")]
    NoDispatchInFlight,
}

/// Serializable observation only. It cannot be deserialized into replay
/// authority or any capability that can start a physical dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AttemptReplaySnapshot {
    pub(crate) request_id: LogicalRequestId,
    pub(crate) generation: AttemptGeneration,
    pub(crate) state: AttemptDispatchState,
    pub(crate) barrier: ReplayBarrier,
}

#[derive(Debug)]
struct ReplayAuthorityState {
    barrier: ReplayBarrier,
    barrier_revision: u64,
    first_attempt_issued: bool,
    active_generation: Option<AttemptGeneration>,
    in_flight_generation: Option<AttemptGeneration>,
    fenced_through: Option<AttemptGeneration>,
    replay_permit_issued_for: Option<AttemptGeneration>,
}

#[derive(Debug)]
struct ReplayAuthority {
    request_id: LogicalRequestId,
    state: Mutex<ReplayAuthorityState>,
}

impl ReplayAuthority {
    fn lock(&self) -> Result<MutexGuard<'_, ReplayAuthorityState>, AttemptDispatchLifecycleError> {
        self.state
            .lock()
            .map_err(|_| AttemptDispatchLifecycleError::ReplayAuthorityPoisoned)
    }
}

/// The unique owner of replay authority for one logical request.
///
/// There is intentionally no crate-visible constructor yet. The transport
/// integration must bind construction to its canonical logical-request owner,
/// otherwise creating two owners would recreate the bypass this type prevents.
#[derive(Debug)]
pub(crate) struct LogicalRequestReplayOwner {
    authority: Arc<ReplayAuthority>,
}

impl LogicalRequestReplayOwner {
    fn new_disabled() -> Self {
        let request_id = LogicalRequestId(NEXT_LOGICAL_REQUEST_ID.fetch_add(1, Ordering::Relaxed));
        Self {
            authority: Arc::new(ReplayAuthority {
                request_id,
                state: Mutex::new(ReplayAuthorityState {
                    barrier: ReplayBarrier::Open,
                    barrier_revision: 0,
                    first_attempt_issued: false,
                    active_generation: None,
                    in_flight_generation: None,
                    fenced_through: None,
                    replay_permit_issued_for: None,
                }),
            }),
        }
    }

    fn begin_first_attempt(
        &mut self,
    ) -> Result<(AttemptDispatchLifecycle, ReplayBarrierHandle), AttemptDispatchLifecycleError>
    {
        let generation = AttemptGeneration(0);
        let mut authority = self.authority.lock()?;
        if authority.first_attempt_issued {
            return Err(AttemptDispatchLifecycleError::FirstAttemptAlreadyIssued);
        }
        authority.first_attempt_issued = true;
        authority.active_generation = Some(generation);
        drop(authority);

        Ok((
            AttemptDispatchLifecycle::from_authority(self.authority.clone(), generation),
            ReplayBarrierHandle {
                authority: self.authority.clone(),
            },
        ))
    }
}

/// A clonable observation handle. Closing it remains valid after an attempt is
/// terminal so late conservative facts can revoke an outstanding permit or a
/// prepared replay before its send admission.
#[derive(Debug, Clone)]
pub(crate) struct ReplayBarrierHandle {
    authority: Arc<ReplayAuthority>,
}

impl ReplayBarrierHandle {
    pub(crate) fn close(
        &self,
        reason: ReplayBarrierReason,
    ) -> Result<bool, AttemptDispatchLifecycleError> {
        let mut authority = self.authority.lock()?;
        if matches!(authority.barrier, ReplayBarrier::Closed(_)) {
            return Ok(false);
        }
        authority.barrier = ReplayBarrier::Closed(reason);
        authority.barrier_revision = authority.barrier_revision.wrapping_add(1);
        Ok(true)
    }

    pub(crate) fn barrier(&self) -> Result<ReplayBarrier, AttemptDispatchLifecycleError> {
        Ok(self.authority.lock()?.barrier)
    }
}

/// Non-cloneable ownership of a physical dispatch that has not been proven
/// stopped. Dropping or forgetting it never clears the shared in-flight fence.
#[derive(Debug)]
pub(crate) struct InFlightDispatch {
    authority: Arc<ReplayAuthority>,
    request_id: LogicalRequestId,
    generation: AttemptGeneration,
}

impl InFlightDispatch {
    /// Called only by the sealed adapter after joining or closing the physical
    /// dispatch and observing its completion.
    fn confirm_quiesced(self) -> DispatchQuiescenceProof {
        DispatchQuiescenceProof {
            authority: self.authority,
            request_id: self.request_id,
            generation: self.generation,
        }
    }
}

/// Opaque proof that a specific physical generation can no longer execute.
#[derive(Debug)]
struct DispatchQuiescenceProof {
    authority: Arc<ReplayAuthority>,
    request_id: LogicalRequestId,
    generation: AttemptGeneration,
}

/// Opaque policy approval bound to one request generation and barrier snapshot.
#[derive(Debug)]
struct ReplayPolicyApproval {
    request_id: LogicalRequestId,
    generation: AttemptGeneration,
    barrier_revision: u64,
}

impl ReplayPolicyApproval {
    /// Issued only after evaluating retry scope, attempt budget, request kind,
    /// and the live replay barrier.
    fn approve(
        lifecycle: &AttemptDispatchLifecycle,
    ) -> Result<Self, AttemptDispatchLifecycleError> {
        let authority = lifecycle.authority.lock()?;
        if let ReplayBarrier::Closed(reason) = authority.barrier {
            return Err(AttemptDispatchLifecycleError::ReplayBarrierClosed(reason));
        }
        Ok(Self {
            request_id: lifecycle.request_id,
            generation: lifecycle.generation,
            barrier_revision: authority.barrier_revision,
        })
    }
}

/// The production adapter around the sealed authority/capability model.
///
/// All methods are synchronous and short: callers never hold the mutex over an
/// upstream or client await. A handle is safe to share with a response body or
/// WebSocket relay task, while generation changes remain serialized.
#[derive(Debug, Clone)]
pub(crate) struct AttemptReplayHandle {
    inner: Arc<Mutex<AttemptReplaySession>>,
}

#[derive(Debug)]
struct AttemptReplaySession {
    _owner: LogicalRequestReplayOwner,
    current: AttemptDispatchLifecycle,
    barrier: ReplayBarrierHandle,
    in_flight: Option<InFlightDispatch>,
}

impl AttemptReplayHandle {
    pub(crate) fn new() -> Result<Self, AttemptDispatchLifecycleError> {
        let mut owner = LogicalRequestReplayOwner::new_disabled();
        let (current, barrier) = owner.begin_first_attempt()?;
        Ok(Self {
            inner: Arc::new(Mutex::new(AttemptReplaySession {
                _owner: owner,
                current,
                barrier,
                in_flight: None,
            })),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, AttemptReplaySession>, AttemptDispatchLifecycleError> {
        self.inner
            .lock()
            .map_err(|_| AttemptDispatchLifecycleError::ReplayAuthorityPoisoned)
    }

    pub(crate) fn close_barrier(
        &self,
        reason: ReplayBarrierReason,
    ) -> Result<bool, AttemptDispatchLifecycleError> {
        self.lock()?.barrier.close(reason)
    }

    /// Applies the conservative request-kind hooks before the physical send.
    pub(crate) fn apply_request_policy(
        &self,
        plan_kind: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(), AttemptDispatchLifecycleError> {
        let normalized = plan_kind.trim().to_ascii_lowercase();
        if normalized.contains("compact") {
            self.close_barrier(ReplayBarrierReason::CompactOperation)?;
        }
        if body.is_some_and(request_has_tool_surface) {
            self.close_barrier(ReplayBarrierReason::ToolCall)?;
        }
        Ok(())
    }

    /// Must run immediately before the adapter admits the physical send.
    pub(crate) fn mark_sent(&self) -> Result<(), AttemptDispatchLifecycleError> {
        let mut session = self.lock()?;
        if session.in_flight.is_some() {
            return Err(AttemptDispatchLifecycleError::DispatchStillInFlight);
        }
        let dispatch = session.current.mark_sent()?;
        session.in_flight = Some(dispatch);
        Ok(())
    }

    /// Called only after the old transport task/socket is joined or closed and
    /// the typed failure classifier plus request-wide budget authorize replay.
    pub(crate) fn authorize_next_after_quiescence(
        &self,
    ) -> Result<(), AttemptDispatchLifecycleError> {
        let mut session = self.lock()?;
        let in_flight = session
            .in_flight
            .as_ref()
            .ok_or(AttemptDispatchLifecycleError::NoDispatchInFlight)?;
        let approval = ReplayPolicyApproval::approve(&session.current)?;
        let proof = DispatchQuiescenceProof {
            authority: in_flight.authority.clone(),
            request_id: in_flight.request_id,
            generation: in_flight.generation,
        };
        let mut permit = session.current.settle_for_replay(approval, proof)?;
        session.in_flight.take();
        session.current = permit.start_next_attempt()?;
        Ok(())
    }

    /// First-party scheduler adapter: a retry result already represents a
    /// classified, budget-admitted and fully collected physical attempt.
    pub(crate) fn authorize_classified_scheduler_retry(
        &self,
    ) -> Result<(), AttemptDispatchLifecycleError> {
        self.authorize_next_after_quiescence()
    }

    /// WebSocket quota retry adapter. The old socket must already be closed and
    /// the old attempt finalizer awaited before this is called.
    pub(crate) fn authorize_websocket_quota_retry(
        &self,
    ) -> Result<(), AttemptDispatchLifecycleError> {
        self.authorize_classified_scheduler_retry()
    }

    pub(crate) fn ensure_uncommitted(&self) -> Result<(), AttemptDispatchLifecycleError> {
        let session = self.lock()?;
        match session.current.state() {
            AttemptDispatchState::Prepared | AttemptDispatchState::SentButUncommitted => Ok(()),
            AttemptDispatchState::ClientCommitted => {
                Err(AttemptDispatchLifecycleError::ClientCommitted)
            }
            AttemptDispatchState::Terminal => {
                Err(AttemptDispatchLifecycleError::InvalidTransition {
                    from: AttemptDispatchState::Terminal,
                    to: AttemptDispatchState::SentButUncommitted,
                })
            }
        }
    }

    pub(crate) fn mark_client_committed(&self) -> Result<(), AttemptDispatchLifecycleError> {
        let mut session = self.lock()?;
        match session.current.state() {
            AttemptDispatchState::ClientCommitted | AttemptDispatchState::Terminal => Ok(()),
            _ => session.current.mark_client_committed(),
        }
    }

    /// Finishes a response whose upstream operation is known to be quiescent.
    pub(crate) fn finish_quiesced(&self) -> Result<(), AttemptDispatchLifecycleError> {
        let mut session = self.lock()?;
        session.in_flight.take();
        if session.current.state() == AttemptDispatchState::Terminal {
            return Ok(());
        }
        if session.current.state() == AttemptDispatchState::Prepared {
            session.current.settle_without_send()
        } else {
            session.current.settle()
        }
    }

    /// Cancellation, serialization failure, and any other ambiguous exit close
    /// replay permanently. The in-flight fence is intentionally retained.
    pub(crate) fn finish_ambiguous(&self) -> Result<(), AttemptDispatchLifecycleError> {
        let mut session = self.lock()?;
        session
            .barrier
            .close(ReplayBarrierReason::AmbiguousDispatchOutcome)?;
        if session.current.state() != AttemptDispatchState::Terminal {
            if session.current.state() == AttemptDispatchState::Prepared {
                session.current.settle_without_send()?;
            } else {
                session.current.settle()?;
            }
        }
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> Result<AttemptReplaySnapshot, AttemptDispatchLifecycleError> {
        let session = self.lock()?;
        Ok(AttemptReplaySnapshot {
            request_id: session.current.request_id(),
            generation: session.current.generation(),
            state: session.current.state(),
            barrier: session.current.replay_barrier()?,
        })
    }

    /// Marks the response handoff on first body poll and terminal on EOF. A
    /// dropped body closes the ambiguity barrier in `Drop`.
    pub(crate) fn guard_response(&self, response: Response<Body>) -> Response<Body> {
        let (parts, body) = response.into_parts();
        let handle = self.clone();
        let guarded = async_stream::stream! {
            let mut guard = ClientBodyReplayGuard::new(handle.clone());
            let mut body = body.into_data_stream();
            while let Some(frame) = body.next().await {
                match frame {
                    Ok(frame) => match handle.mark_client_committed() {
                        Ok(()) => yield Ok::<_, std::io::Error>(frame),
                        Err(error) => {
                            yield Err(replay_body_error(error));
                            return;
                        }
                    },
                    Err(error) => {
                        yield Err(std::io::Error::other(error.to_string()));
                        return;
                    }
                }
            }
            if let Err(error) = handle.finish_quiesced() {
                yield Err(replay_body_error(error));
                return;
            }
            guard.completed = true;
        };
        Response::from_parts(parts, Body::from_stream(guarded))
    }
}

fn request_has_tool_surface(body: &serde_json::Value) -> bool {
    let non_empty = |value: Option<&serde_json::Value>| {
        value.is_some_and(|value| match value {
            serde_json::Value::Array(items) => !items.is_empty(),
            serde_json::Value::Null => false,
            _ => true,
        })
    };
    non_empty(body.get("tools"))
        || non_empty(body.get("tool_choice"))
        || non_empty(body.get("functions"))
        || body
            .pointer("/input")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    matches!(
                        item.get("type").and_then(serde_json::Value::as_str),
                        Some(
                            "function_call" | "function_call_output" | "tool_call" | "tool_result"
                        )
                    ) || non_empty(item.get("tool_calls"))
                })
            })
}

fn replay_body_error(error: AttemptDispatchLifecycleError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

struct ClientBodyReplayGuard {
    handle: AttemptReplayHandle,
    completed: bool,
}

impl ClientBodyReplayGuard {
    fn new(handle: AttemptReplayHandle) -> Self {
        Self {
            handle,
            completed: false,
        }
    }
}

impl Drop for ClientBodyReplayGuard {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.handle.finish_ambiguous();
        }
    }
}

/// An opaque, non-cloneable authorization for exactly one next generation.
#[derive(Debug)]
struct ReplayPermit {
    authority: Arc<ReplayAuthority>,
    request_id: LogicalRequestId,
    previous_generation: AttemptGeneration,
    barrier_revision: u64,
    consumed: bool,
}

impl ReplayPermit {
    /// The adapter retains the barrier handle through the next physical send.
    /// The next lifecycle re-checks it in `mark_sent`, closing the
    /// permit-to-send race.
    fn start_next_attempt(
        &mut self,
    ) -> Result<AttemptDispatchLifecycle, AttemptDispatchLifecycleError> {
        if self.consumed {
            return Err(AttemptDispatchLifecycleError::ReplayPermitAlreadyConsumed);
        }

        let mut authority = self.authority.lock()?;
        if self.authority.request_id != self.request_id
            || authority.active_generation != Some(self.previous_generation)
            || authority.fenced_through != Some(self.previous_generation)
            || authority.in_flight_generation.is_some()
        {
            return Err(AttemptDispatchLifecycleError::ReplayPermitStale);
        }
        if let ReplayBarrier::Closed(reason) = authority.barrier {
            return Err(AttemptDispatchLifecycleError::ReplayBarrierClosed(reason));
        }
        if authority.barrier_revision != self.barrier_revision {
            return Err(AttemptDispatchLifecycleError::ReplayPermitStale);
        }

        let next_generation = self.previous_generation.next()?;
        authority.active_generation = Some(next_generation);
        authority.replay_permit_issued_for = None;
        self.consumed = true;
        drop(authority);

        Ok(AttemptDispatchLifecycle::from_authority(
            self.authority.clone(),
            next_generation,
        ))
    }
}

/// Transport-neutral lifecycle for one physical dispatch generation.
///
/// It has no `Default`, no public constructor, and contains the request and
/// generation binding required to reject stale proofs and permits.
#[derive(Debug)]
pub(crate) struct AttemptDispatchLifecycle {
    authority: Arc<ReplayAuthority>,
    request_id: LogicalRequestId,
    generation: AttemptGeneration,
    state: AttemptDispatchState,
}

impl AttemptDispatchLifecycle {
    fn from_authority(authority: Arc<ReplayAuthority>, generation: AttemptGeneration) -> Self {
        Self {
            request_id: authority.request_id,
            authority,
            generation,
            state: AttemptDispatchState::Prepared,
        }
    }

    pub(crate) const fn request_id(&self) -> LogicalRequestId {
        self.request_id
    }

    pub(crate) const fn generation(&self) -> AttemptGeneration {
        self.generation
    }

    pub(crate) const fn state(&self) -> AttemptDispatchState {
        self.state
    }

    pub(crate) fn replay_barrier(&self) -> Result<ReplayBarrier, AttemptDispatchLifecycleError> {
        Ok(self.authority.lock()?.barrier)
    }

    /// Record the physical send and return its non-cloneable in-flight owner.
    pub(crate) fn mark_sent(&mut self) -> Result<InFlightDispatch, AttemptDispatchLifecycleError> {
        if self.state != AttemptDispatchState::Prepared {
            return Err(AttemptDispatchLifecycleError::InvalidTransition {
                from: self.state,
                to: AttemptDispatchState::SentButUncommitted,
            });
        }

        let mut authority = self.authority.lock()?;
        if authority.active_generation != Some(self.generation) {
            return Err(AttemptDispatchLifecycleError::StaleAttemptGeneration);
        }
        if self.generation.0 > 0 {
            if let ReplayBarrier::Closed(reason) = authority.barrier {
                return Err(AttemptDispatchLifecycleError::ReplayBarrierClosed(reason));
            }
            if authority.fenced_through != Some(AttemptGeneration(self.generation.0 - 1)) {
                return Err(AttemptDispatchLifecycleError::ReplayPermitStale);
            }
        }
        if authority.in_flight_generation.is_some() {
            return Err(AttemptDispatchLifecycleError::DispatchStillInFlight);
        }

        authority.in_flight_generation = Some(self.generation);
        self.state = AttemptDispatchState::SentButUncommitted;
        Ok(InFlightDispatch {
            authority: self.authority.clone(),
            request_id: self.request_id,
            generation: self.generation,
        })
    }

    pub(crate) fn mark_client_committed(&mut self) -> Result<(), AttemptDispatchLifecycleError> {
        self.transition_to(AttemptDispatchState::ClientCommitted)
    }

    pub(crate) fn settle(&mut self) -> Result<(), AttemptDispatchLifecycleError> {
        self.transition_to(AttemptDispatchState::Terminal)
    }

    /// Replay edge. Both arguments are opaque and cannot be minted by sibling
    /// modules; only [`AttemptReplayHandle`] supplies trusted issuers.
    fn settle_for_replay(
        &mut self,
        approval: ReplayPolicyApproval,
        proof: DispatchQuiescenceProof,
    ) -> Result<ReplayPermit, AttemptDispatchLifecycleError> {
        match self.state {
            AttemptDispatchState::ClientCommitted => {
                return Err(AttemptDispatchLifecycleError::ClientCommitted);
            }
            AttemptDispatchState::Terminal | AttemptDispatchState::Prepared => {
                return Err(AttemptDispatchLifecycleError::InvalidTransition {
                    from: self.state,
                    to: AttemptDispatchState::Terminal,
                });
            }
            AttemptDispatchState::SentButUncommitted => {}
        }

        if !Arc::ptr_eq(&self.authority, &proof.authority)
            || proof.request_id != self.request_id
            || proof.generation != self.generation
        {
            return Err(AttemptDispatchLifecycleError::QuiescenceProofMismatch);
        }

        let mut authority = self.authority.lock()?;
        if let ReplayBarrier::Closed(reason) = authority.barrier {
            return Err(AttemptDispatchLifecycleError::ReplayBarrierClosed(reason));
        }
        if approval.request_id != self.request_id
            || approval.generation != self.generation
            || approval.barrier_revision != authority.barrier_revision
        {
            return Err(AttemptDispatchLifecycleError::PolicyApprovalMismatch);
        }
        if authority.active_generation != Some(self.generation) {
            return Err(AttemptDispatchLifecycleError::StaleAttemptGeneration);
        }
        if authority.in_flight_generation != Some(self.generation) {
            return Err(AttemptDispatchLifecycleError::QuiescenceProofMismatch);
        }
        if authority.replay_permit_issued_for == Some(self.generation) {
            return Err(AttemptDispatchLifecycleError::ReplayPermitAlreadyIssued);
        }

        authority.in_flight_generation = None;
        authority.fenced_through = Some(self.generation);
        authority.replay_permit_issued_for = Some(self.generation);
        self.state = AttemptDispatchState::Terminal;
        Ok(ReplayPermit {
            authority: self.authority.clone(),
            request_id: self.request_id,
            previous_generation: self.generation,
            barrier_revision: authority.barrier_revision,
            consumed: false,
        })
    }

    fn settle_without_send(&mut self) -> Result<(), AttemptDispatchLifecycleError> {
        if self.state != AttemptDispatchState::Prepared {
            return Err(AttemptDispatchLifecycleError::InvalidTransition {
                from: self.state,
                to: AttemptDispatchState::Terminal,
            });
        }
        let mut authority = self.authority.lock()?;
        if authority.active_generation != Some(self.generation) {
            return Err(AttemptDispatchLifecycleError::StaleAttemptGeneration);
        }
        authority.fenced_through = Some(self.generation);
        self.state = AttemptDispatchState::Terminal;
        Ok(())
    }

    fn transition_to(
        &mut self,
        next: AttemptDispatchState,
    ) -> Result<(), AttemptDispatchLifecycleError> {
        let allowed = matches!(
            (self.state, next),
            (
                AttemptDispatchState::Prepared,
                AttemptDispatchState::SentButUncommitted | AttemptDispatchState::Terminal
            ) | (
                AttemptDispatchState::SentButUncommitted,
                AttemptDispatchState::ClientCommitted | AttemptDispatchState::Terminal
            ) | (
                AttemptDispatchState::ClientCommitted,
                AttemptDispatchState::Terminal
            )
        );
        if !allowed {
            return Err(AttemptDispatchLifecycleError::InvalidTransition {
                from: self.state,
                to: next,
            });
        }
        self.state = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::mem;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;

    fn first_attempt() -> (
        LogicalRequestReplayOwner,
        AttemptDispatchLifecycle,
        ReplayBarrierHandle,
    ) {
        let mut owner = LogicalRequestReplayOwner::new_disabled();
        let (attempt, barrier) = owner
            .begin_first_attempt()
            .expect("the owner issues exactly one first attempt");
        (owner, attempt, barrier)
    }

    fn replay_permit(
        attempt: &mut AttemptDispatchLifecycle,
        in_flight: InFlightDispatch,
    ) -> ReplayPermit {
        let approval =
            ReplayPolicyApproval::approve(attempt).expect("the test policy snapshot is open");
        let proof = in_flight.confirm_quiesced();
        attempt
            .settle_for_replay(approval, proof)
            .expect("matching policy and quiescence proof permit replay")
    }

    #[test]
    fn transition_matrix_allows_only_forward_lifecycle_edges() {
        use AttemptDispatchState::{ClientCommitted, Prepared, SentButUncommitted, Terminal};

        for from in [Prepared, SentButUncommitted, ClientCommitted, Terminal] {
            for to in [Prepared, SentButUncommitted, ClientCommitted, Terminal] {
                let expected = matches!(
                    (from, to),
                    (Prepared, SentButUncommitted | Terminal)
                        | (SentButUncommitted, ClientCommitted | Terminal)
                        | (ClientCommitted, Terminal)
                );
                let (_, mut attempt, _) = first_attempt();
                attempt.state = from;
                let result = attempt.transition_to(to);
                assert_eq!(result.is_ok(), expected, "unexpected {from:?} -> {to:?}");
                assert_eq!(attempt.state(), if expected { to } else { from });
            }
        }
    }

    #[test]
    fn owner_issues_only_one_first_attempt() {
        let (mut owner, attempt, _) = first_attempt();
        assert_eq!(attempt.generation(), AttemptGeneration(0));
        assert_eq!(
            owner.begin_first_attempt().unwrap_err(),
            AttemptDispatchLifecycleError::FirstAttemptAlreadyIssued
        );
    }

    #[test]
    fn replay_is_generation_bound_and_consumed_once() {
        let (_, mut first, _) = first_attempt();
        let in_flight = first.mark_sent().expect("first dispatch starts");
        let mut permit = replay_permit(&mut first, in_flight);
        let second = permit
            .start_next_attempt()
            .expect("the matching permit advances one generation");

        assert_eq!(first.state(), AttemptDispatchState::Terminal);
        assert_eq!(second.request_id(), first.request_id());
        assert_eq!(second.generation(), AttemptGeneration(1));
        assert_eq!(
            permit.start_next_attempt().unwrap_err(),
            AttemptDispatchLifecycleError::ReplayPermitAlreadyConsumed
        );
    }

    #[test]
    fn wrong_request_quiescence_proof_is_rejected() {
        let (_, mut first, _) = first_attempt();
        let in_flight = first.mark_sent().expect("first dispatch starts");
        let approval = ReplayPolicyApproval::approve(&first).unwrap();

        let (_, mut other, _) = first_attempt();
        let other_in_flight = other.mark_sent().expect("other dispatch starts");
        let wrong_proof = other_in_flight.confirm_quiesced();
        assert_eq!(
            first.settle_for_replay(approval, wrong_proof).unwrap_err(),
            AttemptDispatchLifecycleError::QuiescenceProofMismatch
        );

        drop(in_flight);
    }

    #[test]
    fn wrong_generation_quiescence_proof_cannot_advance_the_fence() {
        let (_, mut first, _) = first_attempt();
        let in_flight = first.mark_sent().expect("first dispatch starts");
        let approval = ReplayPolicyApproval::approve(&first).unwrap();
        let wrong_proof = DispatchQuiescenceProof {
            authority: first.authority.clone(),
            request_id: first.request_id(),
            generation: AttemptGeneration(1),
        };

        assert_eq!(
            first.settle_for_replay(approval, wrong_proof).unwrap_err(),
            AttemptDispatchLifecycleError::QuiescenceProofMismatch
        );
        let authority = first.authority.lock().unwrap();
        assert_eq!(authority.fenced_through, None);
        assert_eq!(authority.in_flight_generation, Some(AttemptGeneration(0)));
        drop(authority);
        drop(in_flight);
    }

    #[test]
    fn late_barrier_fact_revokes_an_issued_permit_concurrently() {
        let (_, mut first, barrier_handle) = first_attempt();
        let in_flight = first.mark_sent().expect("first dispatch starts");
        let mut permit = replay_permit(&mut first, in_flight);
        let rendezvous = Arc::new(Barrier::new(2));
        let worker_rendezvous = rendezvous.clone();

        let worker = thread::spawn(move || {
            worker_rendezvous.wait();
            barrier_handle
                .close(ReplayBarrierReason::AmbiguousDispatchOutcome)
                .expect("late observation closes the shared barrier")
        });
        rendezvous.wait();
        assert!(worker.join().expect("barrier worker does not panic"));

        assert_eq!(
            permit.start_next_attempt().unwrap_err(),
            AttemptDispatchLifecycleError::ReplayBarrierClosed(
                ReplayBarrierReason::AmbiguousDispatchOutcome
            )
        );
    }

    #[test]
    fn late_barrier_fact_blocks_a_prepared_replay_at_send_admission() {
        let (_, mut first, barrier_handle) = first_attempt();
        let in_flight = first.mark_sent().expect("first dispatch starts");
        let mut permit = replay_permit(&mut first, in_flight);
        let mut second = permit.start_next_attempt().expect("second is prepared");

        barrier_handle
            .close(ReplayBarrierReason::ToolCall)
            .expect("late tool-call fact closes the logical request barrier");
        assert_eq!(
            second.mark_sent().unwrap_err(),
            AttemptDispatchLifecycleError::ReplayBarrierClosed(ReplayBarrierReason::ToolCall)
        );
        assert_eq!(second.state(), AttemptDispatchState::Prepared);
    }

    #[test]
    fn barrier_is_logical_request_wide_monotonic_and_first_reason_wins() {
        let (_, attempt, barrier_handle) = first_attempt();
        assert!(barrier_handle
            .close(ReplayBarrierReason::CompactOperation)
            .unwrap());
        assert!(!barrier_handle
            .close(ReplayBarrierReason::SideEffectingRequest)
            .unwrap());
        assert_eq!(
            attempt.replay_barrier().unwrap(),
            ReplayBarrier::Closed(ReplayBarrierReason::CompactOperation)
        );
        assert_eq!(attempt.state(), AttemptDispatchState::Prepared);
    }

    #[test]
    fn dropping_or_forgetting_in_flight_ownership_never_clears_the_fence() {
        let (_, mut dropped_attempt, _) = first_attempt();
        let dropped = dropped_attempt.mark_sent().expect("dispatch starts");
        drop(dropped);
        assert_eq!(
            dropped_attempt
                .authority
                .lock()
                .unwrap()
                .in_flight_generation,
            Some(AttemptGeneration(0))
        );

        let (_, mut forgotten_attempt, _) = first_attempt();
        let forgotten = forgotten_attempt.mark_sent().expect("dispatch starts");
        mem::forget(forgotten);
        assert_eq!(
            forgotten_attempt
                .authority
                .lock()
                .unwrap()
                .in_flight_generation,
            Some(AttemptGeneration(0))
        );
    }

    #[test]
    fn dropping_or_forgetting_permit_cannot_advance_generation() {
        for forget in [false, true] {
            let (_, mut first, _) = first_attempt();
            let in_flight = first.mark_sent().expect("dispatch starts");
            let permit = replay_permit(&mut first, in_flight);
            if forget {
                mem::forget(permit);
            } else {
                drop(permit);
            }
            assert_eq!(
                first.authority.lock().unwrap().active_generation,
                Some(AttemptGeneration(0))
            );
        }
    }

    #[test]
    fn client_commit_and_closed_barrier_both_fail_closed() {
        let (_, mut committed, _) = first_attempt();
        let in_flight = committed.mark_sent().expect("dispatch starts");
        committed
            .mark_client_committed()
            .expect("commit follows send");
        let approval = ReplayPolicyApproval::approve(&committed).unwrap();
        let proof = in_flight.confirm_quiesced();
        assert_eq!(
            committed.settle_for_replay(approval, proof).unwrap_err(),
            AttemptDispatchLifecycleError::ClientCommitted
        );

        let (_, mut closed, barrier_handle) = first_attempt();
        let in_flight = closed.mark_sent().expect("dispatch starts");
        let approval = ReplayPolicyApproval::approve(&closed).unwrap();
        let proof = in_flight.confirm_quiesced();
        barrier_handle
            .close(ReplayBarrierReason::SideEffectingRequest)
            .unwrap();
        assert_eq!(
            closed.settle_for_replay(approval, proof).unwrap_err(),
            AttemptDispatchLifecycleError::ReplayBarrierClosed(
                ReplayBarrierReason::SideEffectingRequest
            )
        );
    }

    #[test]
    fn snapshot_round_trip_is_observation_only() {
        let replay = AttemptReplayHandle::new().unwrap();
        replay.mark_sent().unwrap();
        replay.mark_client_committed().unwrap();
        let snapshot = replay.snapshot().unwrap();
        let encoded = serde_json::to_string(&snapshot).unwrap();
        let decoded: AttemptReplaySnapshot = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, snapshot);
        assert_eq!(decoded.state, AttemptDispatchState::ClientCommitted);
    }

    #[test]
    fn compact_and_tool_request_hooks_close_before_send() {
        for (plan_kind, body, reason) in [
            (
                "openai_responses_compact",
                serde_json::json!({"input": "x"}),
                ReplayBarrierReason::CompactOperation,
            ),
            (
                "openai_responses",
                serde_json::json!({"tools": [{"type": "function", "name": "lookup"}]}),
                ReplayBarrierReason::ToolCall,
            ),
        ] {
            let replay = AttemptReplayHandle::new().unwrap();
            replay.apply_request_policy(plan_kind, Some(&body)).unwrap();
            assert_eq!(
                replay.snapshot().unwrap().barrier,
                ReplayBarrier::Closed(reason)
            );
            replay.mark_sent().unwrap();
            assert_eq!(
                replay.authorize_classified_scheduler_retry().unwrap_err(),
                AttemptDispatchLifecycleError::ReplayBarrierClosed(reason)
            );
        }
    }

    #[test]
    fn concurrent_late_commit_and_retry_never_start_two_generations() {
        for _ in 0..128 {
            let replay = AttemptReplayHandle::new().unwrap();
            replay.mark_sent().unwrap();
            let retry = replay.clone();
            let commit = replay.clone();
            let gate = Arc::new(Barrier::new(3));
            let retry_gate = gate.clone();
            let commit_gate = gate.clone();
            let retry_thread = thread::spawn(move || {
                retry_gate.wait();
                retry.authorize_classified_scheduler_retry()
            });
            let commit_thread = thread::spawn(move || {
                commit_gate.wait();
                commit.mark_client_committed()
            });
            gate.wait();
            let retry_result = retry_thread.join().unwrap();
            let _ = commit_thread.join().unwrap();
            let snapshot = replay.snapshot().unwrap();
            if retry_result.is_ok() {
                assert_eq!(snapshot.generation, AttemptGeneration(1));
                assert!(matches!(
                    snapshot.state,
                    AttemptDispatchState::Prepared | AttemptDispatchState::ClientCommitted
                ));
            } else {
                assert_eq!(snapshot.generation, AttemptGeneration(0));
                assert_eq!(snapshot.state, AttemptDispatchState::ClientCommitted);
            }
        }
    }

    #[tokio::test]
    async fn client_body_drop_closes_ambiguity_barrier() {
        let replay = AttemptReplayHandle::new().unwrap();
        replay.mark_sent().unwrap();
        let response = Response::new(Body::from("first frame"));
        let guarded = replay.guard_response(response);
        let mut body = guarded.into_body().into_data_stream();
        assert_eq!(body.next().await.unwrap().unwrap(), "first frame");
        assert_eq!(
            replay.snapshot().unwrap().state,
            AttemptDispatchState::ClientCommitted
        );
        drop(body);
        let snapshot = replay.snapshot().unwrap();
        assert_eq!(snapshot.state, AttemptDispatchState::Terminal);
        assert_eq!(
            snapshot.barrier,
            ReplayBarrier::Closed(ReplayBarrierReason::AmbiguousDispatchOutcome)
        );
    }
}
