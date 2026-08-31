use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static NEXT_NAMESPACE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceNamespace(String);

impl AcceptanceNamespace {
    pub fn new(label: &str) -> Self {
        let label: String = label
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect();
        let serial = NEXT_NAMESPACE.fetch_add(1, Ordering::Relaxed);
        Self(format!("issue47:{label}:{}:{serial}", std::process::id()))
    }

    pub fn key(&self, suffix: &str) -> String {
        format!("{}:{suffix}", self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn sql_identifier(&self) -> String {
        self.0
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TraceKind {
    ProbeAcquired,
    ProbeRejected,
    ProbeCompletionPersisted,
    CandidatePlanned,
    CandidateSkipped,
    AdmissionGranted,
    AdmissionDenied,
    UpstreamSendStarted,
    UpstreamSendFinished,
    ClientCommitted,
    RetryRequested,
    AttemptCharged,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceEvent {
    pub sequence: u64,
    pub now_ms: u64,
    pub gateway: String,
    pub request: String,
    pub attempt: u32,
    pub kind: TraceKind,
    pub candidate: Option<String>,
    pub credential: Option<String>,
    pub provider: Option<String>,
    pub page: Option<u32>,
    pub generation: Option<String>,
    pub rank: Option<u64>,
    pub cursor: Option<String>,
    pub lease_id: Option<String>,
    pub fencing_token: Option<u64>,
    pub network_request_id: Option<String>,
    pub reason: Option<String>,
}

impl AcceptanceEvent {
    pub fn new(
        gateway: impl Into<String>,
        request: impl Into<String>,
        attempt: u32,
        kind: TraceKind,
    ) -> Self {
        Self {
            sequence: 0,
            now_ms: 0,
            gateway: gateway.into(),
            request: request.into(),
            attempt,
            kind,
            candidate: None,
            credential: None,
            provider: None,
            page: None,
            generation: None,
            rank: None,
            cursor: None,
            lease_id: None,
            fencing_token: None,
            network_request_id: None,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSend {
    pub request: String,
    pub attempt: u32,
    pub candidate: String,
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkObservation {
    sends: Vec<NetworkSend>,
}

impl NetworkObservation {
    pub fn len(&self) -> usize {
        self.sends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sends.is_empty()
    }

    pub fn sends(&self) -> &[NetworkSend] {
        &self.sends
    }

    pub(crate) fn record(
        &mut self,
        request: impl Into<String>,
        attempt: u32,
        candidate: impl Into<String>,
        credential: Option<String>,
    ) {
        self.sends.push(NetworkSend {
            request: request.into(),
            attempt,
            candidate: candidate.into(),
            credential,
        });
    }
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticTrace {
    next_sequence: Arc<AtomicU64>,
    events: Arc<Mutex<Vec<AcceptanceEvent>>>,
}

pub trait AcceptanceEventSink: Send + Sync {
    fn record(&self, event: AcceptanceEvent);
}

impl AcceptanceEventSink for DiagnosticTrace {
    fn record(&self, event: AcceptanceEvent) {
        DiagnosticTrace::record(self, event);
    }
}

#[derive(Debug, Clone, Default)]
pub struct FaultInjector {
    armed: Arc<Mutex<BTreeSet<String>>>,
}

impl FaultInjector {
    pub fn arm(&self, point: impl Into<String>) {
        self.armed
            .lock()
            .expect("fault injector mutex poisoned")
            .insert(point.into());
    }

    pub fn hit(&self, point: &str) -> bool {
        self.armed
            .lock()
            .expect("fault injector mutex poisoned")
            .remove(point)
    }
}

impl DiagnosticTrace {
    pub fn record(&self, mut event: AcceptanceEvent) {
        event.sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        self.events
            .lock()
            .expect("trace mutex poisoned")
            .push(event);
    }

    pub fn snapshot(&self) -> Vec<AcceptanceEvent> {
        let mut events = self.events.lock().expect("trace mutex poisoned").clone();
        events.sort_by_key(|event| event.sequence);
        events
    }

    pub fn render(&self) -> String {
        render_trace(&self.snapshot())
    }
}

#[derive(Debug, Clone)]
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    pub fn new(now_ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(now_ms)))
    }

    pub fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    pub fn advance_ms(&self, delta_ms: u64) -> u64 {
        self.0.fetch_add(delta_ms, Ordering::SeqCst) + delta_ms
    }
}

#[derive(Debug, Clone)]
pub struct ControlledCheckpoint {
    name: Arc<str>,
    arrived: Arc<tokio::sync::Barrier>,
    released: Arc<tokio::sync::Barrier>,
    timeout: Duration,
}

impl ControlledCheckpoint {
    pub fn new(name: impl Into<Arc<str>>, participants: usize, timeout: Duration) -> Self {
        assert!(participants > 0, "checkpoint requires participants");
        Self {
            name: name.into(),
            arrived: Arc::new(tokio::sync::Barrier::new(participants + 1)),
            released: Arc::new(tokio::sync::Barrier::new(participants + 1)),
            timeout,
        }
    }

    pub async fn participant_wait(&self) -> Result<(), String> {
        self.wait(&self.arrived, "arrival").await?;
        self.wait(&self.released, "release").await
    }

    pub async fn wait_until_all_arrived(&self) -> Result<(), String> {
        self.wait(&self.arrived, "arrival").await
    }

    pub async fn release(&self) -> Result<(), String> {
        self.wait(&self.released, "release").await
    }

    async fn wait(&self, barrier: &tokio::sync::Barrier, phase: &str) -> Result<(), String> {
        tokio::time::timeout(self.timeout, barrier.wait())
            .await
            .map(|_| ())
            .map_err(|_| {
                format!(
                    "checkpoint '{}' timed out during {phase} after {:?}",
                    self.name, self.timeout
                )
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptBudgetExpectation {
    pub max_total_attempts: usize,
    pub max_credentials: usize,
    pub max_provider_switches: usize,
    pub deadline_ms: u64,
}

pub fn validate_exactly_one_half_open_probe(events: &[AcceptanceEvent]) -> Result<(), String> {
    validate_trace_send_prerequisites(events, true)?;
    let acquired: Vec<_> = events
        .iter()
        .filter(|event| event.kind == TraceKind::ProbeAcquired)
        .collect();
    if acquired.len() != 1 {
        return failure(
            format!(
                "expected exactly one HalfOpen probe acquisition, got {}",
                acquired.len()
            ),
            events,
        );
    }
    let rejected: Vec<_> = events
        .iter()
        .filter(|event| event.kind == TraceKind::ProbeRejected)
        .collect();
    if rejected.is_empty() {
        return failure(
            "expected at least one contender probe rejection".to_string(),
            events,
        );
    }
    if acquired[0].lease_id.is_none() || acquired[0].fencing_token.is_none() {
        return failure(
            "probe acquisition must include lease_id and fencing_token".to_string(),
            events,
        );
    }
    if rejected.iter().any(|event| {
        event.lease_id != acquired[0].lease_id || event.fencing_token != acquired[0].fencing_token
    }) {
        return failure(
            "probe rejection must identify the active lease and fencing token".to_string(),
            events,
        );
    }
    let completion = events.iter().find(|event| {
        event.kind == TraceKind::ProbeCompletionPersisted
            && event.lease_id == acquired[0].lease_id
            && event.fencing_token == acquired[0].fencing_token
    });
    if completion.is_none() {
        return failure(
            "probe lease/fencing token has no persisted SQL completion observation".to_string(),
            events,
        );
    }
    let probe_sends = events
        .iter()
        .filter(|event| {
            event.kind == TraceKind::UpstreamSendStarted
                && event.request == acquired[0].request
                && event.attempt == acquired[0].attempt
        })
        .count();
    if probe_sends != 1 {
        return failure(format!("probe attempt sent {probe_sends} times"), events);
    }
    Ok(())
}

pub fn validate_half_open_probe_e2e(
    events: &[AcceptanceEvent],
    network: &NetworkObservation,
) -> Result<(), String> {
    validate_send_contract(events, network)?;
    validate_exactly_one_half_open_probe(events)?;
    if network.sends.len() != 1 {
        return failure(
            format!(
                "authoritative recorder observed {} HalfOpen sends, expected 1",
                network.sends.len()
            ),
            events,
        );
    }
    Ok(())
}

pub fn validate_attempt_order(
    events: &[AcceptanceEvent],
    request: &str,
    expected_candidates: &[&str],
) -> Result<(), String> {
    validate_trace_send_prerequisites(events, true)?;
    if expected_candidates.is_empty() {
        return failure(
            "expected candidate order cannot be empty".to_string(),
            events,
        );
    }
    let actual: Vec<_> = events
        .iter()
        .filter(|event| event.request == request && event.kind == TraceKind::UpstreamSendStarted)
        .filter_map(|event| event.candidate.as_deref())
        .collect();
    if actual != expected_candidates {
        return failure(
            format!("attempt order mismatch: expected {expected_candidates:?}, got {actual:?}"),
            events,
        );
    }
    Ok(())
}

pub fn validate_send_contract(
    events: &[AcceptanceEvent],
    network: &NetworkObservation,
) -> Result<(), String> {
    validate_trace_send_prerequisites(events, true)?;
    let sends: Vec<_> = events
        .iter()
        .filter(|event| event.kind == TraceKind::UpstreamSendStarted)
        .collect();
    if sends.is_empty() {
        return failure("scenario produced no upstream send".to_string(), events);
    }
    if sends.len() != network.sends.len() {
        return failure(
            format!(
                "trace/network send count mismatch: trace={}, network={}",
                sends.len(),
                network.sends.len()
            ),
            events,
        );
    }
    for event in sends {
        let Some(network_send) = network
            .sends
            .iter()
            .find(|send| send.request == event.request && send.attempt == event.attempt)
        else {
            return failure(
                format!(
                    "network recorder has no send for request '{}' attempt {}",
                    event.request, event.attempt
                ),
                events,
            );
        };
        if event.candidate.as_deref() != Some(network_send.candidate.as_str()) {
            return failure(
                "trace candidate disagrees with network recorder".to_string(),
                events,
            );
        }
    }
    Ok(())
}

pub fn validate_no_pool_stampede(events: &[AcceptanceEvent]) -> Result<(), String> {
    validate_trace_send_prerequisites(events, true)?;
    if !events
        .iter()
        .any(|event| event.kind == TraceKind::UpstreamSendStarted)
    {
        return failure(
            "pool scenario produced no upstream send".to_string(),
            events,
        );
    }
    let mut active: BTreeMap<&str, (&str, u32)> = BTreeMap::new();
    for event in events {
        let Some(credential) = event.credential.as_deref() else {
            continue;
        };
        match event.kind {
            TraceKind::UpstreamSendStarted => {
                if let Some((owner_request, owner_attempt)) =
                    active.insert(credential, (&event.request, event.attempt))
                {
                    return failure(
                        format!(
                            "credential '{credential}' had overlapping sends owned by request '{owner_request}' attempt {owner_attempt}"
                        ),
                        events,
                    );
                }
            }
            TraceKind::UpstreamSendFinished | TraceKind::Terminal => {
                if active.get(credential) == Some(&(&event.request, event.attempt)) {
                    active.remove(credential);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn validate_frozen_pages(events: &[AcceptanceEvent]) -> Result<(), String> {
    validate_trace_send_prerequisites(events, true)?;
    if !events
        .iter()
        .any(|event| event.kind == TraceKind::CandidatePlanned)
        || !events
            .iter()
            .any(|event| event.kind == TraceKind::CandidateSkipped)
    {
        return failure(
            "pagination scenario requires both planned and skipped candidate events".to_string(),
            events,
        );
    }
    let mut requests: BTreeMap<&str, (u32, BTreeMap<u32, &str>)> = BTreeMap::new();
    for event in events.iter().filter(|event| {
        matches!(
            event.kind,
            TraceKind::CandidatePlanned | TraceKind::CandidateSkipped
        )
    }) {
        let page = event
            .page
            .ok_or_else(|| "candidate event missing page".to_string())?;
        let generation = event
            .generation
            .as_deref()
            .ok_or_else(|| "candidate event missing generation".to_string())?;
        if event.rank.is_none() || event.cursor.is_none() {
            return failure(
                format!(
                    "request '{}' candidate event missing rank or cursor",
                    event.request
                ),
                events,
            );
        }
        let (highest_page, page_generations) = requests
            .entry(&event.request)
            .or_insert((0, BTreeMap::new()));
        if page < *highest_page {
            return failure(
                format!(
                    "request '{}' candidate cursor moved backwards to page {page}",
                    event.request
                ),
                events,
            );
        }
        *highest_page = page;
        match page_generations.insert(page, generation) {
            Some(existing) if existing != generation => {
                return failure(
                    format!("page {page} changed generation from '{existing}' to '{generation}'"),
                    events,
                );
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn validate_denied_admission_never_sends(events: &[AcceptanceEvent]) -> Result<(), String> {
    if !events
        .iter()
        .any(|event| event.kind == TraceKind::AdmissionDenied)
    {
        return failure("admission scenario produced no denial".to_string(), events);
    }
    for denied in events
        .iter()
        .filter(|event| event.kind == TraceKind::AdmissionDenied)
    {
        if events.iter().any(|event| {
            event.request == denied.request
                && event.attempt == denied.attempt
                && event.sequence > denied.sequence
                && event.kind == TraceKind::UpstreamSendStarted
        }) {
            return failure(
                format!(
                    "request '{}' attempt {} sent after admission denial",
                    denied.request, denied.attempt
                ),
                events,
            );
        }
    }
    validate_trace_send_prerequisites(events, false)
}

pub fn validate_denied_admission_with_network(
    events: &[AcceptanceEvent],
    network: &NetworkObservation,
) -> Result<(), String> {
    validate_denied_admission_never_sends(events)?;
    for denied in events
        .iter()
        .filter(|event| event.kind == TraceKind::AdmissionDenied)
    {
        if network
            .sends
            .iter()
            .any(|send| send.request == denied.request && send.attempt == denied.attempt)
        {
            return failure(
                format!(
                    "authoritative recorder observed denied request '{}' attempt {}",
                    denied.request, denied.attempt
                ),
                events,
            );
        }
    }
    Ok(())
}

pub fn validate_no_replay_after_client_commit(events: &[AcceptanceEvent]) -> Result<(), String> {
    validate_trace_send_prerequisites(events, true)?;
    if !events
        .iter()
        .any(|event| event.kind == TraceKind::ClientCommitted)
    {
        return failure(
            "scenario produced no ClientCommitted boundary".to_string(),
            events,
        );
    }
    for committed in events
        .iter()
        .filter(|event| event.kind == TraceKind::ClientCommitted)
    {
        if events.iter().any(|event| {
            event.request == committed.request
                && event.sequence > committed.sequence
                && matches!(
                    event.kind,
                    TraceKind::RetryRequested | TraceKind::UpstreamSendStarted
                )
        }) {
            return failure(
                format!(
                    "request '{}' replayed after client commit",
                    committed.request
                ),
                events,
            );
        }
    }
    Ok(())
}

pub fn validate_no_replay_after_commit_with_network(
    events: &[AcceptanceEvent],
    network: &NetworkObservation,
) -> Result<(), String> {
    validate_no_replay_after_client_commit(events)?;
    for committed in events
        .iter()
        .filter(|event| event.kind == TraceKind::ClientCommitted)
    {
        if network
            .sends
            .iter()
            .any(|send| send.request == committed.request && send.attempt > committed.attempt)
        {
            return failure(
                format!(
                    "authoritative recorder observed replay after commit for request '{}'",
                    committed.request
                ),
                events,
            );
        }
    }
    Ok(())
}

pub fn validate_attempt_budget(
    events: &[AcceptanceEvent],
    expected: &AttemptBudgetExpectation,
) -> Result<(), String> {
    validate_trace_send_prerequisites(events, false)?;
    if !events
        .iter()
        .any(|event| event.kind == TraceKind::AttemptCharged)
    {
        return failure(
            "scenario produced no AttemptCharged event".to_string(),
            events,
        );
    }
    let attempts: Vec<_> = events
        .iter()
        .filter(|event| event.kind == TraceKind::AttemptCharged)
        .collect();
    let requests: BTreeSet<_> = events.iter().map(|event| event.request.as_str()).collect();
    for request in requests {
        let request_attempts: Vec<_> = attempts
            .iter()
            .copied()
            .filter(|event| event.request == request)
            .collect();
        let credentials: BTreeSet<_> = request_attempts
            .iter()
            .filter_map(|event| event.credential.as_deref())
            .collect();
        let providers: Vec<_> = request_attempts
            .iter()
            .filter_map(|event| event.provider.as_deref())
            .collect();
        let switches = providers
            .windows(2)
            .filter(|pair| pair[0] != pair[1])
            .count();
        let deadline_breached = events.iter().any(|event| {
            event.request == request
                && matches!(
                    event.kind,
                    TraceKind::AttemptCharged | TraceKind::UpstreamSendStarted
                )
                && event.now_ms > expected.deadline_ms
        });
        if request_attempts.len() > expected.max_total_attempts
            || credentials.len() > expected.max_credentials
            || switches > expected.max_provider_switches
            || deadline_breached
        {
            return failure(
                format!(
                    "request '{request}' attempt budget exceeded: attempts={}/{}, credentials={}/{}, switches={}/{}, deadline_breached={deadline_breached}",
                    request_attempts.len(),
                    expected.max_total_attempts,
                    credentials.len(),
                    expected.max_credentials,
                    switches,
                    expected.max_provider_switches
                ),
                events,
            );
        }
    }
    Ok(())
}

pub fn validate_network_authority(
    events: &[AcceptanceEvent],
    network: &NetworkObservation,
) -> Result<(), String> {
    validate_send_contract(events, network)
}

fn validate_trace_send_prerequisites(
    events: &[AcceptanceEvent],
    require_send: bool,
) -> Result<(), String> {
    let sends: Vec<_> = events
        .iter()
        .filter(|event| event.kind == TraceKind::UpstreamSendStarted)
        .collect();
    if require_send && sends.is_empty() {
        return failure("scenario produced no upstream send".to_string(), events);
    }
    for send in sends {
        let has_admission = events.iter().any(|prior| {
            prior.request == send.request
                && prior.attempt == send.attempt
                && prior.sequence < send.sequence
                && prior.kind == TraceKind::AdmissionGranted
        });
        let has_charge = events.iter().any(|prior| {
            prior.request == send.request
                && prior.attempt == send.attempt
                && prior.sequence < send.sequence
                && prior.kind == TraceKind::AttemptCharged
        });
        if !has_admission || !has_charge {
            return failure(
                format!(
                    "send request '{}' attempt {} lacks prior admission grant or attempt charge",
                    send.request, send.attempt
                ),
                events,
            );
        }
    }
    Ok(())
}

fn failure(reason: String, events: &[AcceptanceEvent]) -> Result<(), String> {
    Err(format!(
        "{reason}\nacceptance trace:\n{}",
        render_trace(events)
    ))
}

fn render_trace(events: &[AcceptanceEvent]) -> String {
    let mut rendered = String::new();
    for event in events {
        let _ = writeln!(
            rendered,
            "#{:04} t={:>6}ms gateway={} request={} attempt={} kind={:?} candidate={} credential={} provider={} page={} generation={} rank={} cursor={} lease={} fence={} network_request={} reason={}",
            event.sequence,
            event.now_ms,
            event.gateway,
            event.request,
            event.attempt,
            event.kind,
            event.candidate.as_deref().unwrap_or("-"),
            event.credential.as_deref().unwrap_or("-"),
            event.provider.as_deref().unwrap_or("-"),
            event.page.map(|value| value.to_string()).as_deref().unwrap_or("-"),
            event.generation.as_deref().unwrap_or("-"),
            event.rank.map(|value| value.to_string()).as_deref().unwrap_or("-"),
            event.cursor.as_deref().unwrap_or("-"),
            event.lease_id.as_deref().unwrap_or("-"),
            event.fencing_token.map(|value| value.to_string()).as_deref().unwrap_or("-"),
            event.network_request_id.as_deref().unwrap_or("-"),
            event.reason.as_deref().unwrap_or("-")
        );
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(sequence: u64, kind: TraceKind, attempt: u32) -> AcceptanceEvent {
        let mut event = AcceptanceEvent::new("gateway-a", "request-1", attempt, kind);
        event.sequence = sequence;
        event.now_ms = sequence * 10;
        event
    }

    #[tokio::test]
    async fn controlled_checkpoint_is_two_phase_and_bounded() {
        let checkpoint = ControlledCheckpoint::new("before-send", 2, Duration::from_secs(1));
        let left = tokio::spawn({
            let checkpoint = checkpoint.clone();
            async move { checkpoint.participant_wait().await }
        });
        let right = tokio::spawn({
            let checkpoint = checkpoint.clone();
            async move { checkpoint.participant_wait().await }
        });
        checkpoint.wait_until_all_arrived().await.expect("arrival");
        assert!(!left.is_finished());
        assert!(!right.is_finished());
        checkpoint.release().await.expect("release");
        left.await.expect("left task").expect("left checkpoint");
        right.await.expect("right task").expect("right checkpoint");
    }

    #[test]
    fn validators_accept_complete_safe_trace() {
        let mut events = Vec::new();
        fn push(
            events: &mut Vec<AcceptanceEvent>,
            kind: TraceKind,
            attempt: u32,
            candidate: Option<&str>,
        ) {
            let mut event = AcceptanceEvent::new("gateway-a", "request-1", attempt, kind);
            event.sequence = events.len() as u64 + 1;
            event.now_ms = event.sequence * 10;
            event.candidate = candidate.map(str::to_string);
            event.credential = Some(format!("credential-{attempt}"));
            event.provider = Some(
                if attempt == 1 {
                    "provider-a"
                } else {
                    "provider-b"
                }
                .into(),
            );
            event.page = Some(attempt.saturating_sub(1));
            event.generation = Some(format!("generation-{}", attempt.saturating_sub(1)));
            event.rank = Some(u64::from(attempt));
            event.cursor = Some(format!("cursor-{attempt}"));
            events.push(event);
        }
        push(
            &mut events,
            TraceKind::ProbeAcquired,
            1,
            Some("candidate-a"),
        );
        events[0].lease_id = Some("lease-a".into());
        events[0].fencing_token = Some(1);
        push(
            &mut events,
            TraceKind::ProbeRejected,
            2,
            Some("candidate-a"),
        );
        events[1].lease_id = Some("lease-a".into());
        events[1].fencing_token = Some(1);
        push(
            &mut events,
            TraceKind::CandidatePlanned,
            1,
            Some("candidate-a"),
        );
        push(
            &mut events,
            TraceKind::AdmissionGranted,
            1,
            Some("candidate-a"),
        );
        push(
            &mut events,
            TraceKind::AttemptCharged,
            1,
            Some("candidate-a"),
        );
        push(
            &mut events,
            TraceKind::UpstreamSendStarted,
            1,
            Some("candidate-a"),
        );
        push(
            &mut events,
            TraceKind::UpstreamSendFinished,
            1,
            Some("candidate-a"),
        );
        push(
            &mut events,
            TraceKind::ProbeCompletionPersisted,
            1,
            Some("candidate-a"),
        );
        let completion = events.last_mut().expect("completion event");
        completion.lease_id = Some("lease-a".into());
        completion.fencing_token = Some(1);
        push(
            &mut events,
            TraceKind::ClientCommitted,
            1,
            Some("candidate-a"),
        );
        push(
            &mut events,
            TraceKind::CandidateSkipped,
            2,
            Some("candidate-b"),
        );
        push(
            &mut events,
            TraceKind::AdmissionDenied,
            2,
            Some("candidate-b"),
        );
        push(&mut events, TraceKind::Terminal, 2, Some("candidate-b"));

        validate_exactly_one_half_open_probe(&events).expect("single probe");
        validate_attempt_order(&events, "request-1", &["candidate-a"]).expect("order");
        let mut network = NetworkObservation::default();
        network.record("request-1", 1, "candidate-a", Some("credential-1".into()));
        validate_network_authority(&events, &network).expect("network authority");
        validate_half_open_probe_e2e(&events, &network).expect("half-open E2E contract");
        validate_no_pool_stampede(&events).expect("pool");
        validate_frozen_pages(&events).expect("pages");
        validate_denied_admission_never_sends(&events).expect("admission");
        validate_no_replay_after_client_commit(&events).expect("commit");
        validate_attempt_budget(
            &events,
            &AttemptBudgetExpectation {
                max_total_attempts: 1,
                max_credentials: 1,
                max_provider_switches: 0,
                deadline_ms: 100,
            },
        )
        .expect("budget");
    }

    #[test]
    fn failures_include_diagnostic_trace() {
        let admission = event(1, TraceKind::AdmissionGranted, 1);
        let charge = event(2, TraceKind::AttemptCharged, 1);
        let send = event(3, TraceKind::UpstreamSendStarted, 1);
        let rejected = event(4, TraceKind::ProbeRejected, 1);
        let error = validate_exactly_one_half_open_probe(&[admission, charge, send, rejected])
            .expect_err("must fail");
        assert!(error.contains("expected exactly one HalfOpen probe"));
        assert!(error.contains("acceptance trace:"));
    }

    #[test]
    fn affinity_order_validator_rejects_reordering() {
        let admission = event(1, TraceKind::AdmissionGranted, 1);
        let charge = event(2, TraceKind::AttemptCharged, 1);
        let mut first = event(3, TraceKind::UpstreamSendStarted, 1);
        first.candidate = Some("candidate-b".into());
        let error =
            validate_attempt_order(&[admission, charge, first], "request-1", &["candidate-a"])
                .expect_err("reordered affinity must fail");
        assert!(error.contains("attempt order mismatch"));
    }

    #[test]
    fn pool_validator_rejects_overlapping_credential_owners() {
        let admission_a = event(1, TraceKind::AdmissionGranted, 1);
        let charge_a = event(2, TraceKind::AttemptCharged, 1);
        let mut first = event(3, TraceKind::UpstreamSendStarted, 1);
        first.credential = Some("credential-a".into());
        let mut admission_b = event(4, TraceKind::AdmissionGranted, 1);
        admission_b.gateway = "gateway-b".into();
        admission_b.request = "request-2".into();
        let mut charge_b = event(5, TraceKind::AttemptCharged, 1);
        charge_b.gateway = "gateway-b".into();
        charge_b.request = "request-2".into();
        let mut second = event(6, TraceKind::UpstreamSendStarted, 1);
        second.gateway = "gateway-b".into();
        second.request = "request-2".into();
        second.credential = Some("credential-a".into());
        let error = validate_no_pool_stampede(&[
            admission_a,
            charge_a,
            first,
            admission_b,
            charge_b,
            second,
        ])
        .expect_err("overlapping credential owners must fail");
        assert!(error.contains("overlapping sends"));
    }

    #[test]
    fn pagination_validator_rejects_generation_drift_within_page() {
        let admission = event(1, TraceKind::AdmissionGranted, 1);
        let charge = event(2, TraceKind::AttemptCharged, 1);
        let send = event(3, TraceKind::UpstreamSendStarted, 1);
        let mut first = event(4, TraceKind::CandidatePlanned, 1);
        first.page = Some(0);
        first.generation = Some("generation-a".into());
        first.rank = Some(1);
        first.cursor = Some("cursor-a".into());
        let mut second = event(5, TraceKind::CandidateSkipped, 1);
        second.page = Some(0);
        second.generation = Some("generation-b".into());
        second.rank = Some(1);
        second.cursor = Some("cursor-a".into());
        let error = validate_frozen_pages(&[admission, charge, send, first, second])
            .expect_err("same page generation drift must fail");
        assert!(error.contains("changed generation"));
    }

    #[test]
    fn admission_validator_rejects_send_after_denial() {
        let denied = event(1, TraceKind::AdmissionDenied, 1);
        let send = event(2, TraceKind::UpstreamSendStarted, 1);
        let error = validate_denied_admission_never_sends(&[denied, send])
            .expect_err("send after denial must fail");
        assert!(error.contains("sent after admission denial"));
    }

    #[test]
    fn lifecycle_validator_rejects_retry_after_commit() {
        let admission = event(1, TraceKind::AdmissionGranted, 1);
        let charge = event(2, TraceKind::AttemptCharged, 1);
        let send = event(3, TraceKind::UpstreamSendStarted, 1);
        let committed = event(4, TraceKind::ClientCommitted, 1);
        let retry = event(5, TraceKind::RetryRequested, 1);
        let error =
            validate_no_replay_after_client_commit(&[admission, charge, send, committed, retry])
                .expect_err("retry after client commit must fail");
        assert!(error.contains("replayed after client commit"));
    }

    #[test]
    fn budget_validator_rejects_attempt_after_deadline() {
        let mut charged = event(1, TraceKind::AttemptCharged, 1);
        charged.now_ms = 101;
        charged.credential = Some("credential-a".into());
        charged.provider = Some("provider-a".into());
        let error = validate_attempt_budget(
            &[charged],
            &AttemptBudgetExpectation {
                max_total_attempts: 1,
                max_credentials: 1,
                max_provider_switches: 0,
                deadline_ms: 100,
            },
        )
        .expect_err("attempt after deadline must fail");
        assert!(error.contains("deadline_breached=true"));
    }

    #[test]
    fn fake_clock_and_fault_injector_are_deterministic() {
        let clock = ManualClock::new(100);
        assert_eq!(clock.advance_ms(25), 125);
        assert_eq!(clock.now_ms(), 125);

        let faults = FaultInjector::default();
        faults.arm("after-client-commit");
        assert!(faults.hit("after-client-commit"));
        assert!(!faults.hit("after-client-commit"));
    }
}
