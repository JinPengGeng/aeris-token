use aether_contracts::ExecutionPlan;
use aether_routing_core::{rank_vector_for_candidate, CandidateKind, RoutingCandidateFacts};
use aether_scheduler_core::{parse_request_candidate_report_context, SchedulerRankingOutcome};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ai_serving::planner::candidate_resolution::{
    EligibleLocalExecutionCandidate, LocalExecutionCandidateKind, SkippedLocalExecutionCandidate,
};
use crate::orchestration::{
    FailureDisposition, FailureRetryAction, FailureScope, FailureTokenAction,
    LocalFailoverClassification,
};

const TRACE_SCHEMA_VERSION: u8 = 1;
const MAX_PAGE_DECISIONS: usize = 64;
const MAX_LABEL_BYTES: usize = 96;
const REF_HEX_BYTES: usize = 12;
const ATTEMPT_ORDINAL_REPORT_FIELD: &str = "scheduling_attempt_ordinal";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CandidateRef {
    provider_ref: String,
    endpoint_ref: String,
    model_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RankVector {
    #[serde(skip_serializing_if = "Option::is_none")]
    original_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ranking_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority_slot: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_priority_before: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_priority_after: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key_priority_before: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key_priority_after: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    promoted_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    demoted_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PageCandidateDecision {
    candidate_index: u32,
    candidate: CandidateRef,
    candidate_kind: &'static str,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    skip_reason: Option<String>,
    rank_vector: RankVector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct AttemptIndex {
    ordinal: u32,
    candidate_index: u32,
    retry_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pool_key_index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SchedulingDecision {
    CandidatePage {
        generation: u64,
        generation_source: &'static str,
        page_index: u32,
        candidate_start_index: u32,
        selection_source: String,
        candidate_count: usize,
        skipped_count: usize,
        decisions: Vec<PageCandidateDecision>,
        omitted_decision_count: usize,
    },
    AttemptSelected {
        attempt: AttemptIndex,
        candidate: CandidateRef,
        selection_source: String,
        rank_vector: RankVector,
        budget_source: &'static str,
        attempts_consumed: u32,
    },
    DynamicGate {
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<AttemptIndex>,
        gate: &'static str,
        result: &'static str,
        reason: &'static str,
    },
    ClassifierDisposition {
        attempt: AttemptIndex,
        status_code: u16,
        classification: &'static str,
        retry_action: &'static str,
        failure_scope: &'static str,
        token_action: &'static str,
        preserve_upstream_error: bool,
    },
    BudgetDecision {
        budget: &'static str,
        result: &'static str,
        consumed: u64,
        limit: u64,
        reason: &'static str,
    },
    Termination {
        reason: &'static str,
        attempt_count: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_attempt: Option<AttemptIndex>,
    },
}

#[derive(Debug, Serialize)]
struct SchedulingDecisionEnvelope<'a> {
    schema_version: u8,
    trace_id: &'a str,
    decision: SchedulingDecision,
}

pub(crate) fn emit_candidate_page(
    trace_id: &str,
    generation: u64,
    page_index: u32,
    candidate_start_index: u32,
    selection_source: &str,
    routing_policy: Option<&aether_routing_core::ResolvedRoutingPolicy>,
    candidates: &[EligibleLocalExecutionCandidate],
    skipped: &[SkippedLocalExecutionCandidate],
) {
    let candidate_count = candidates.len();
    let skipped_count = skipped.len();
    let decisions = candidates
        .iter()
        .enumerate()
        .map(|(offset, candidate)| {
            page_decision(
                candidate_start_index
                    .saturating_add(u32::try_from(offset).unwrap_or(u32::MAX)),
                &candidate.candidate,
                candidate.kind,
                candidate.ranking.as_ref(),
                routing_policy,
                "eligible",
                None,
            )
        })
        .chain(skipped.iter().enumerate().map(|(offset, skipped)| {
            page_decision(
                candidate_start_index.saturating_add(
                    u32::try_from(candidate_count.saturating_add(offset)).unwrap_or(u32::MAX),
                ),
                &skipped.candidate,
                if skipped.transport.as_ref().is_some_and(|transport| {
                    crate::handlers::shared::provider_pool::admin_provider_pool_config_from_config_value(
                        transport.provider.config.as_ref(),
                    )
                    .is_some()
                }) {
                    LocalExecutionCandidateKind::PoolGroup
                } else {
                    LocalExecutionCandidateKind::SingleKey
                },
                skipped.ranking.as_ref(),
                routing_policy,
                "skipped",
                Some(skipped.skip_reason),
            )
        }))
        .take(MAX_PAGE_DECISIONS)
        .collect::<Vec<_>>();
    let omitted_decision_count = candidate_count
        .saturating_add(skipped_count)
        .saturating_sub(decisions.len());
    emit(
        trace_id,
        SchedulingDecision::CandidatePage {
            generation,
            generation_source: "scheduler_affinity_epoch",
            page_index,
            candidate_start_index,
            selection_source: bounded_label(selection_source),
            candidate_count,
            skipped_count,
            decisions,
            omitted_decision_count,
        },
    );
}

pub(crate) fn emit_attempt_selected(
    trace_id: &str,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    ordinal: u32,
) {
    let attempt = attempt_index(report_context, ordinal);
    let selection_source = report_context
        .and_then(|value| value.get("pool_selection_source"))
        .and_then(Value::as_str)
        .or_else(|| {
            report_context
                .and_then(|value| value.pointer("/routing_trace/selection_source"))
                .and_then(Value::as_str)
        })
        .or_else(|| attempt.pool_key_index.is_some().then_some("pool_cursor"))
        .unwrap_or("scheduler_rank");
    emit(
        trace_id,
        SchedulingDecision::AttemptSelected {
            attempt,
            candidate: candidate_ref_from_plan(plan, report_context),
            selection_source: bounded_label(selection_source),
            rank_vector: rank_vector_from_report_context(report_context),
            budget_source: "observed_attempt_count",
            attempts_consumed: ordinal,
        },
    );
}

pub(crate) fn emit_dynamic_gate(
    trace_id: &str,
    report_context: Option<&Value>,
    ordinal: u32,
    result: &'static str,
    reason: &'static str,
) {
    emit(
        trace_id,
        SchedulingDecision::DynamicGate {
            attempt: report_context.map(|context| attempt_index(Some(context), ordinal)),
            gate: "gateway_upstream_execution",
            result,
            reason,
        },
    );
}

pub(crate) fn emit_classifier_disposition(
    trace_id: &str,
    report_context: Option<&Value>,
    ordinal: u32,
    status_code: u16,
    classification: LocalFailoverClassification,
    disposition: FailureDisposition,
) {
    emit(
        trace_id,
        SchedulingDecision::ClassifierDisposition {
            attempt: attempt_index(report_context, ordinal),
            status_code,
            classification: classification.as_str(),
            retry_action: retry_action_label(disposition.retry_action),
            failure_scope: failure_scope_label(disposition.failure_scope),
            token_action: token_action_label(disposition.token_action),
            preserve_upstream_error: disposition.preserve_upstream_error,
        },
    );
}

pub(crate) fn emit_provider_transfer_budget(
    trace_id: &str,
    consumed: u64,
    limit: u64,
    reason: &'static str,
) {
    emit(
        trace_id,
        SchedulingDecision::BudgetDecision {
            budget: "provider_transfer",
            result: "exhausted",
            consumed,
            limit,
            reason,
        },
    );
}

pub(crate) fn emit_termination(
    trace_id: &str,
    reason: &'static str,
    attempt_count: u32,
    last_attempt: Option<AttemptIndex>,
) {
    emit(
        trace_id,
        SchedulingDecision::Termination {
            reason,
            attempt_count,
            last_attempt,
        },
    );
}

pub(crate) fn attempt_index_from_report_context(
    report_context: Option<&Value>,
    ordinal: u32,
) -> AttemptIndex {
    attempt_index(report_context, ordinal)
}

pub(crate) fn annotate_report_context_with_attempt_ordinal(
    report_context: &mut Option<Value>,
    ordinal: u32,
) {
    if let Some(object) = report_context.as_mut().and_then(Value::as_object_mut) {
        object.insert(
            ATTEMPT_ORDINAL_REPORT_FIELD.to_string(),
            Value::Number(ordinal.into()),
        );
    }
}

fn emit(trace_id: &str, decision: SchedulingDecision) {
    let envelope = SchedulingDecisionEnvelope {
        schema_version: TRACE_SCHEMA_VERSION,
        trace_id,
        decision,
    };
    match serde_json::to_string(&envelope) {
        Ok(payload) => tracing::info!(
            event_name = "scheduling_decision_trace",
            log_type = "event",
            trace_id,
            schema_version = TRACE_SCHEMA_VERSION,
            decision_trace = %payload,
            "gateway scheduling decision trace"
        ),
        Err(error) => tracing::warn!(
            event_name = "scheduling_decision_trace_serialization_failed",
            log_type = "ops",
            trace_id,
            error = %error,
            "gateway could not serialize scheduling decision trace"
        ),
    }
}

fn page_decision(
    candidate_index: u32,
    candidate: &aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
    kind: LocalExecutionCandidateKind,
    ranking: Option<&SchedulerRankingOutcome>,
    routing_policy: Option<&aether_routing_core::ResolvedRoutingPolicy>,
    outcome: &'static str,
    skip_reason: Option<&str>,
) -> PageCandidateDecision {
    PageCandidateDecision {
        candidate_index,
        candidate: candidate_ref(
            &candidate.provider_id,
            &candidate.endpoint_id,
            &candidate.model_id,
            matches!(kind, LocalExecutionCandidateKind::SingleKey)
                .then_some(candidate.key_id.as_str()),
        ),
        candidate_kind: match kind {
            LocalExecutionCandidateKind::SingleKey => "credential",
            LocalExecutionCandidateKind::PoolGroup => "pool_group",
        },
        outcome,
        skip_reason: skip_reason.map(bounded_label),
        rank_vector: rank_vector_for_page_candidate(candidate, kind, ranking, routing_policy),
    }
}

fn rank_vector_for_page_candidate(
    candidate: &aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
    kind: LocalExecutionCandidateKind,
    ranking: Option<&SchedulerRankingOutcome>,
    routing_policy: Option<&aether_routing_core::ResolvedRoutingPolicy>,
) -> RankVector {
    let routing = routing_policy.map(|policy| {
        let candidate_kind = match kind {
            LocalExecutionCandidateKind::SingleKey => CandidateKind::Provider,
            LocalExecutionCandidateKind::PoolGroup => CandidateKind::PoolGroup,
        };
        rank_vector_for_candidate(
            &policy.ranking_overlay,
            &RoutingCandidateFacts {
                candidate_kind,
                provider_id: candidate.provider_id.clone(),
                endpoint_id: candidate.endpoint_id.clone(),
                model_id: candidate.model_id.clone(),
                key_id: matches!(candidate_kind, CandidateKind::Provider)
                    .then(|| candidate.key_id.clone()),
                provider_priority: candidate.provider_priority,
                key_priority: candidate
                    .key_global_priority_for_format
                    .unwrap_or(candidate.key_internal_priority),
            },
        )
    });
    RankVector {
        original_index: ranking.map(|value| value.original_index),
        ranking_index: ranking.map(|value| value.ranking_index),
        priority_slot: ranking.map(|value| value.priority_slot),
        provider_priority_before: routing.as_ref().map(|value| value.provider_priority_before),
        provider_priority_after: routing.as_ref().map(|value| value.provider_priority_after),
        key_priority_before: routing.as_ref().map(|value| value.key_priority_before),
        key_priority_after: routing.as_ref().map(|value| value.key_priority_after),
        promoted_by: ranking
            .and_then(|value| value.promoted_by)
            .map(bounded_label),
        demoted_by: ranking
            .and_then(|value| value.demoted_by)
            .map(bounded_label),
    }
}

fn rank_vector_from_report_context(report_context: Option<&Value>) -> RankVector {
    let value = report_context.unwrap_or(&Value::Null);
    let routing = value
        .pointer("/routing_trace/global_candidates/0/ranking_vector")
        .unwrap_or(&Value::Null);
    RankVector {
        original_index: value
            .get("original_index")
            .and_then(Value::as_u64)
            .map(|v| v as usize),
        ranking_index: value
            .get("ranking_index")
            .and_then(Value::as_u64)
            .map(|v| v as usize),
        priority_slot: value
            .get("priority_slot")
            .and_then(Value::as_i64)
            .map(|v| v as i32),
        provider_priority_before: routing
            .get("provider_priority_before")
            .and_then(Value::as_i64)
            .map(|v| v as i32),
        provider_priority_after: routing
            .get("provider_priority_after")
            .and_then(Value::as_i64)
            .map(|v| v as i32),
        key_priority_before: routing
            .get("key_priority_before")
            .and_then(Value::as_i64)
            .map(|v| v as i32),
        key_priority_after: routing
            .get("key_priority_after")
            .and_then(Value::as_i64)
            .map(|v| v as i32),
        promoted_by: value
            .get("promoted_by")
            .and_then(Value::as_str)
            .map(bounded_label),
        demoted_by: value
            .get("demoted_by")
            .and_then(Value::as_str)
            .map(bounded_label),
    }
}

fn candidate_ref_from_plan(plan: &ExecutionPlan, report_context: Option<&Value>) -> CandidateRef {
    let model_id = report_context
        .and_then(|value| value.get("model_id"))
        .and_then(Value::as_str)
        .or(plan.model_name.as_deref())
        .unwrap_or("unknown");
    candidate_ref(
        &plan.provider_id,
        &plan.endpoint_id,
        model_id,
        Some(&plan.key_id),
    )
}

fn candidate_ref(
    provider_id: &str,
    endpoint_id: &str,
    model_id: &str,
    credential_id: Option<&str>,
) -> CandidateRef {
    CandidateRef {
        provider_ref: opaque_ref("provider", provider_id),
        endpoint_ref: opaque_ref("endpoint", endpoint_id),
        model_ref: opaque_ref("model", model_id),
        credential_ref: credential_id.map(|value| opaque_ref("credential", value)),
    }
}

fn attempt_index(report_context: Option<&Value>, ordinal: u32) -> AttemptIndex {
    let parsed = parse_request_candidate_report_context(report_context);
    let candidate_index = parsed
        .as_ref()
        .and_then(|value| value.candidate_index)
        .unwrap_or_default();
    AttemptIndex {
        ordinal: (ordinal != 0)
            .then_some(ordinal)
            .or_else(|| {
                report_context
                    .and_then(|value| value.get(ATTEMPT_ORDINAL_REPORT_FIELD))
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
            })
            .unwrap_or_else(|| candidate_index.saturating_add(1)),
        candidate_index,
        retry_index: parsed
            .as_ref()
            .map(|value| value.retry_index)
            .unwrap_or_default(),
        pool_key_index: parsed.and_then(|value| value.pool_key_index),
    }
}

fn opaque_ref(namespace: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"aether-scheduling-trace-v1\0");
    hasher.update(namespace.as_bytes());
    hasher.update(b"\0");
    hasher.update(value.trim().as_bytes());
    let digest = hasher.finalize();
    let mut output = String::with_capacity(REF_HEX_BYTES * 2);
    for byte in digest.iter().take(REF_HEX_BYTES) {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn bounded_label(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(MAX_LABEL_BYTES));
    for character in value.chars().filter(|character| !character.is_control()) {
        if output.len().saturating_add(character.len_utf8()) > MAX_LABEL_BYTES {
            break;
        }
        output.push(character);
    }
    output
}

const fn retry_action_label(value: FailureRetryAction) -> &'static str {
    match value {
        FailureRetryAction::Stop => "stop",
        FailureRetryAction::SameCredential => "same_credential",
        FailureRetryAction::NextCandidate => "next_candidate",
        FailureRetryAction::NextCredential => "next_credential",
        FailureRetryAction::NextEndpoint => "next_endpoint",
    }
}

const fn failure_scope_label(value: FailureScope) -> &'static str {
    match value {
        FailureScope::None => "none",
        FailureScope::Credential => "credential",
        FailureScope::CredentialModel => "credential_model",
        FailureScope::Endpoint => "endpoint",
        FailureScope::Provider => "provider",
    }
}

const fn token_action_label(value: FailureTokenAction) -> &'static str {
    match value {
        FailureTokenAction::None => "none",
        FailureTokenAction::ForceRefresh => "force_refresh",
        FailureTokenAction::Quarantine => "quarantine",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::prelude::*;

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    struct SharedBufferWriter(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn lines(&self) -> Vec<serde_json::Value> {
            String::from_utf8(self.0.lock().expect("buffer should lock").clone())
                .expect("buffer should contain valid utf-8")
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| serde_json::from_str(line).expect("json log line should parse"))
                .collect()
        }
    }

    impl std::io::Write for SharedBufferWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("buffer should lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for SharedBuffer {
        type Writer = SharedBufferWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedBufferWriter(Arc::clone(&self.0))
        }
    }

    #[test]
    fn opaque_references_are_stable_and_do_not_expose_source_ids() {
        let first = candidate_ref(
            "provider-secret",
            "endpoint-secret",
            "model-secret",
            Some("credential-token-secret"),
        );
        let second = candidate_ref(
            "provider-secret",
            "endpoint-secret",
            "model-secret",
            Some("credential-token-secret"),
        );

        assert_eq!(first, second);
        let serialized = serde_json::to_string(&first).expect("candidate ref serializes");
        for forbidden in [
            "provider-secret",
            "endpoint-secret",
            "model-secret",
            "credential-token-secret",
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn selected_attempt_uses_report_model_id_for_page_correlation() {
        let plan = ExecutionPlan {
            request_id: "request".to_string(),
            candidate_id: None,
            provider_name: None,
            provider_id: "provider".to_string(),
            endpoint_id: "endpoint".to_string(),
            key_id: "credential".to_string(),
            method: "POST".to_string(),
            url: "https://example.invalid".to_string(),
            headers: Default::default(),
            content_type: None,
            content_encoding: None,
            body: aether_contracts::RequestBody::from_json(serde_json::json!({})),
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("provider-model-name".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let report_context = serde_json::json!({"model_id": "scheduler-model-id"});

        assert_eq!(
            candidate_ref_from_plan(&plan, Some(&report_context)),
            candidate_ref(
                "provider",
                "endpoint",
                "scheduler-model-id",
                Some("credential")
            )
        );
    }

    #[test]
    fn labels_are_bounded_and_strip_control_characters() {
        let label = bounded_label(&format!(
            "secret\n{}{}",
            "x".repeat(90),
            "\u{754c}".repeat(20)
        ));
        assert!(!label.contains('\n'));
        assert!(label.len() <= MAX_LABEL_BYTES);
        assert!(label.is_char_boundary(label.len()));
    }

    #[test]
    fn serialized_trace_never_contains_report_context_secrets() {
        let context = serde_json::json!({
            "candidate_index": 3,
            "retry_index": 2,
            "pool_key_index": 1,
            "pool_key_lease_token": "lease-token-secret",
            "original_request_body": {"api_key": "request-secret"},
            "authorization": "Bearer credential-secret",
            "pool_selection_source": "pool_score"
        });
        let decision = SchedulingDecision::AttemptSelected {
            attempt: attempt_index(Some(&context), 4),
            candidate: candidate_ref("provider", "endpoint", "model", Some("credential-secret")),
            selection_source: "pool_score".to_string(),
            rank_vector: rank_vector_from_report_context(Some(&context)),
            budget_source: "observed_attempt_count",
            attempts_consumed: 4,
        };
        let serialized = serde_json::to_string(&SchedulingDecisionEnvelope {
            schema_version: TRACE_SCHEMA_VERSION,
            trace_id: "trace-correlation",
            decision,
        })
        .expect("trace serializes");

        for forbidden in [
            "lease-token-secret",
            "request-secret",
            "Bearer credential-secret",
            "credential-secret",
            "original_request_body",
            "pool_key_lease_token",
        ] {
            assert!(!serialized.contains(forbidden), "leaked {forbidden}");
        }
        assert!(serialized.contains("trace-correlation"));
        assert!(serialized.contains("pool_score"));
    }

    #[test]
    fn emitted_page_explains_selected_and_skipped_candidates() {
        let writer = SharedBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(false)
                .with_writer(writer.clone())
                .with_filter(LevelFilter::INFO),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let selected = candidate_ref(
            "provider-selected",
            "endpoint-selected",
            "model-selected",
            Some("credential-selected"),
        );
        let skipped = candidate_ref(
            "provider-skipped",
            "endpoint-skipped",
            "model-skipped",
            Some("credential-skipped"),
        );
        let rank_vector = RankVector {
            original_index: Some(1),
            ranking_index: Some(0),
            priority_slot: Some(10),
            provider_priority_before: Some(20),
            provider_priority_after: Some(10),
            key_priority_before: Some(5),
            key_priority_after: Some(5),
            promoted_by: Some("routing_policy".to_string()),
            demoted_by: None,
        };

        tracing::dispatcher::with_default(&dispatch, || {
            emit(
                "trace-explainability",
                SchedulingDecision::CandidatePage {
                    generation: 42,
                    generation_source: "scheduler_affinity_epoch",
                    page_index: 0,
                    candidate_start_index: 0,
                    selection_source: "routing_policy".to_string(),
                    candidate_count: 1,
                    skipped_count: 1,
                    decisions: vec![
                        PageCandidateDecision {
                            candidate_index: 0,
                            candidate: selected.clone(),
                            candidate_kind: "credential",
                            outcome: "eligible",
                            skip_reason: None,
                            rank_vector: rank_vector.clone(),
                        },
                        PageCandidateDecision {
                            candidate_index: 1,
                            candidate: skipped,
                            candidate_kind: "credential",
                            outcome: "skipped",
                            skip_reason: Some("capability_mismatch".to_string()),
                            rank_vector: RankVector {
                                original_index: Some(0),
                                ranking_index: None,
                                priority_slot: None,
                                provider_priority_before: None,
                                provider_priority_after: None,
                                key_priority_before: None,
                                key_priority_after: None,
                                promoted_by: None,
                                demoted_by: None,
                            },
                        },
                    ],
                    omitted_decision_count: 0,
                },
            );
            emit(
                "trace-explainability",
                SchedulingDecision::AttemptSelected {
                    attempt: AttemptIndex {
                        ordinal: 1,
                        candidate_index: 0,
                        retry_index: 0,
                        pool_key_index: None,
                    },
                    candidate: selected,
                    selection_source: "routing_policy".to_string(),
                    rank_vector,
                    budget_source: "observed_attempt_count",
                    attempts_consumed: 1,
                },
            );
        });

        let events = writer
            .lines()
            .into_iter()
            .map(|line| {
                serde_json::from_str::<Value>(
                    line.get("decision_trace")
                        .and_then(Value::as_str)
                        .expect("decision trace field should be present"),
                )
                .expect("decision trace should parse")
            })
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].pointer("/decision/kind").and_then(Value::as_str),
            Some("candidate_page")
        );
        assert_eq!(
            events[0]
                .pointer("/decision/decisions/0/outcome")
                .and_then(Value::as_str),
            Some("eligible")
        );
        assert_eq!(
            events[0]
                .pointer("/decision/decisions/1/outcome")
                .and_then(Value::as_str),
            Some("skipped")
        );
        assert_eq!(
            events[0].pointer("/decision/decisions/0/candidate"),
            events[1].pointer("/decision/candidate")
        );
        let serialized = serde_json::to_string(&events).expect("events should serialize");
        for raw_identifier in [
            "provider-selected",
            "endpoint-selected",
            "credential-selected",
            "provider-skipped",
            "credential-skipped",
        ] {
            assert!(!serialized.contains(raw_identifier));
        }
    }

    #[test]
    fn page_decisions_have_a_hard_bound() {
        let decisions = (0..MAX_PAGE_DECISIONS + 10)
            .take(MAX_PAGE_DECISIONS)
            .collect::<Vec<_>>();
        assert_eq!(decisions.len(), MAX_PAGE_DECISIONS);
    }

    #[test]
    fn classifier_disposition_labels_are_stable_table_driven() {
        let cases = [
            (FailureRetryAction::Stop, "stop"),
            (FailureRetryAction::SameCredential, "same_credential"),
            (FailureRetryAction::NextCandidate, "next_candidate"),
            (FailureRetryAction::NextCredential, "next_credential"),
            (FailureRetryAction::NextEndpoint, "next_endpoint"),
        ];
        for (action, expected) in cases {
            assert_eq!(retry_action_label(action), expected);
        }
    }

    #[test]
    fn attempt_index_preserves_full_width_indexes_and_final_ordinal() {
        let context = serde_json::json!({
            "candidate_index": u32::MAX,
            "retry_index": u32::MAX,
            "pool_key_index": u32::MAX
        });
        assert_eq!(
            attempt_index(Some(&context), 9),
            AttemptIndex {
                ordinal: 9,
                candidate_index: u32::MAX,
                retry_index: u32::MAX,
                pool_key_index: Some(u32::MAX),
            }
        );
    }

    #[test]
    fn annotated_attempt_ordinal_is_used_by_downstream_events() {
        let mut context = Some(serde_json::json!({"candidate_index": 7}));
        annotate_report_context_with_attempt_ordinal(&mut context, 23);

        assert_eq!(attempt_index(context.as_ref(), 0).ordinal, 23);
    }
}
