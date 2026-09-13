use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data_contracts::repository::settlement::{
    RequestAttemptExecutionFacts, RequestAttemptExecutionStatus, RequestAttemptFundsIdentity,
    RequestFundsIdentity, StoredUsageSettlement, UsageSettlementInput,
};
use aether_data_contracts::repository::usage::{
    StoredRequestUsageAudit, UpsertUsageRecord, UsageWriteRepository,
};
use aether_data_contracts::DataLayerError;
use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeQueueStore, RuntimeState};
use async_trait::async_trait;
use serde_json::json;

use crate::{
    UsageAttemptChargeEvidence, UsageAttemptFundsAction, UsageAttemptFundsEvent,
    UsageBillingEventEnricher, UsageEvent, UsageEventData, UsageEventType, UsageRecordWriter,
    UsageRuntime, UsageRuntimeAccess, UsageRuntimeConfig, UsageSettlementWriter,
};

#[derive(Default)]
struct Store {
    queue: Option<Arc<dyn RuntimeQueueStore>>,
    usage: InMemoryUsageReadRepository,
    fail_financial: AtomicBool,
    omit_parent: AtomicBool,
    financial: Mutex<Vec<UsageAttemptFundsEvent>>,
    parents: AtomicUsize,
    legacy: AtomicUsize,
    enrichment: AtomicUsize,
}

#[async_trait]
impl UsageSettlementWriter for Store {
    fn has_usage_settlement_writer(&self) -> bool {
        true
    }

    async fn write_request_attempt_funds_event(
        &self,
        event: &UsageAttemptFundsEvent,
        _finalized_at_unix_secs: u64,
    ) -> Result<(), DataLayerError> {
        if self.fail_financial.load(Ordering::Acquire) {
            return Err(DataLayerError::TimedOut(
                "financial database unavailable".into(),
            ));
        }
        self.financial.lock().unwrap().push(event.clone());
        Ok(())
    }

    async fn settle_usage(
        &self,
        _input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
        self.legacy.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }
}

#[async_trait]
impl UsageRecordWriter for Store {
    async fn upsert_usage_record(
        &self,
        record: UpsertUsageRecord,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        self.parents.fetch_add(1, Ordering::Relaxed);
        if self.omit_parent.load(Ordering::Acquire) {
            return Ok(None);
        }
        self.usage.upsert(record).await.map(Some)
    }
}

#[async_trait]
impl UsageBillingEventEnricher for Store {
    async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
        self.enrichment.fetch_add(1, Ordering::Relaxed);
        Err(DataLayerError::InvalidInput(
            "legacy pricing must not be consulted".into(),
        ))
    }
}

impl UsageRuntimeAccess for Store {
    fn has_usage_writer(&self) -> bool {
        true
    }
    fn has_usage_worker_queue(&self) -> bool {
        self.queue.is_some()
    }
    fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
        self.queue.clone()
    }
}

fn runtime() -> UsageRuntime {
    UsageRuntime::new(UsageRuntimeConfig {
        enabled: true,
        queue_terminal_events: true,
        ..Default::default()
    })
    .unwrap()
}

fn event() -> UsageEvent {
    UsageEvent::new(
        UsageEventType::Cancelled,
        "request",
        UsageEventData {
            user_id: Some("owner".into()),
            api_key_id: Some("key".into()),
            provider_name: "provider-a".into(),
            model: "image".into(),
            total_cost_usd: Some(999.0),
            actual_total_cost_usd: Some(999.0),
            attempt_funds: Some(Box::new(UsageAttemptFundsEvent {
                schema_version: 1,
                identity: RequestAttemptFundsIdentity {
                    request: RequestFundsIdentity {
                        reservation_token: "server-token-a".into(),
                        request_id: "request".into(),
                        user_id: Some("owner".into()),
                        api_key_id: Some("key".into()),
                        api_key_is_standalone: false,
                    },
                    attempt_id: "550e8400-e29b-41d4-a716-446655440000".into(),
                },
                action: UsageAttemptFundsAction::Outcome {
                    execution: RequestAttemptExecutionFacts {
                        status: RequestAttemptExecutionStatus::Cancelled,
                        response_time_ms: 100,
                    },
                    evidence: UsageAttemptChargeEvidence::Unknown,
                },
            })),
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn attempt_outcomes_reach_worker_and_direct_without_finalizing_parent_or_legacy_billing() {
    for direct in [false, true] {
        let store = Store::default();
        let event = event();
        if direct {
            runtime()
                .record_terminal_event_direct(&store, event.clone())
                .await;
        } else {
            crate::worker::write_event_record(&store, &event)
                .await
                .unwrap();
        }
        assert_eq!(
            *store.financial.lock().unwrap(),
            vec![*event.data.attempt_funds.unwrap()]
        );
        assert_eq!(store.parents.load(Ordering::Relaxed), 0);
        assert_eq!(store.legacy.load(Ordering::Relaxed), 0);
        assert_eq!(store.enrichment.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test]
async fn attempt_parent_lifecycle_requires_a_returned_row_and_never_uses_legacy_settlement() {
    for missing in [false, true] {
        let store = Store::default();
        store.omit_parent.store(missing, Ordering::Release);
        let mut event = event();
        event.data.attempt_funds.as_mut().unwrap().action =
            UsageAttemptFundsAction::ParentLifecycle;
        assert_eq!(
            crate::worker::write_event_record(&store, &event)
                .await
                .is_err(),
            missing
        );
        assert_eq!(store.financial.lock().unwrap().len(), 1);
        assert_eq!(store.parents.load(Ordering::Relaxed), 1);
        assert_eq!(store.legacy.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test]
async fn successful_queue_append_cannot_hide_a_failed_financial_repository_write() {
    let store = Store {
        queue: Some(Arc::new(RuntimeState::memory(
            MemoryRuntimeStateConfig::default(),
        ))),
        ..Default::default()
    };
    store.fail_financial.store(true, Ordering::Release);
    assert!(runtime()
        .persist_attempt_funds_event(&store, event())
        .await
        .is_err());
    assert!(store.financial.lock().unwrap().is_empty());
    store.fail_financial.store(false, Ordering::Release);
    runtime()
        .persist_attempt_funds_event(&store, event())
        .await
        .unwrap();
    assert_eq!(store.financial.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn mismatched_request_and_financial_failures_stop_before_any_parent_or_legacy_write() {
    let store = Store::default();
    let mut mismatch = event();
    mismatch.request_id = "other-request".into();
    assert!(crate::worker::write_event_record(&store, &mismatch)
        .await
        .is_err());
    assert!(store.financial.lock().unwrap().is_empty());
    store.fail_financial.store(true, Ordering::Release);
    assert!(!runtime().write_event_direct(&store, &event()).await);
    assert_eq!(store.parents.load(Ordering::Relaxed), 0);
    assert_eq!(store.legacy.load(Ordering::Relaxed), 0);
}

#[test]
fn clone_and_bounded_wire_preserve_financial_capability_when_capture_is_omitted() {
    let mut event = event();
    event.data.response_body = Some(json!({"diagnostic": "x".repeat(32_768)}));
    let expected = event.data.attempt_funds.clone();
    assert_eq!(event.clone().data.attempt_funds, expected);
    let encoded = event.to_bounded_stream_fields(4096).unwrap();
    assert!(encoded.diagnostics_omitted);
    let decoded = UsageEvent::from_stream_fields(&encoded.fields).unwrap();
    assert_eq!(decoded.data.attempt_funds, expected);
    assert!(decoded.data.response_body.is_none());
    assert_eq!(decoded.request_id, event.request_id);
}

#[test]
fn financial_wire_rejects_reporter_supplied_money_and_unknown_schema() {
    let mut value = serde_json::to_value(event().data.attempt_funds.unwrap()).unwrap();
    value["actual_cost_units"] = json!(1);
    assert!(serde_json::from_value::<UsageAttemptFundsEvent>(value).is_err());
    let mut event = event();
    event.data.attempt_funds.as_mut().unwrap().schema_version = 2;
    assert!(event
        .data
        .attempt_funds
        .unwrap()
        .validate("request")
        .is_err());
}
