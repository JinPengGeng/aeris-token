#[cfg(test)]
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use crate::ai_serving::{
    commit_response_history_record, conversation_history_scope,
    hydrate_native_response_history_if_materialized, hydrate_response_history,
    record_converted_response_history, response_history_is_loaded, response_history_storage_key,
    try_record_converted_response_history, validate_native_response_history,
    ConversationHistoryCapability, ConversationHistoryResolutionError, ConversationHistoryResolver,
    NativeResponseHistoryValidation, ResponseHistoryRecord,
};
use aether_runtime_state::{KvCreateResult, RuntimeState};
use axum::http::StatusCode;
use serde_json::Value;
use tracing::warn;

use crate::GatewayError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConversationHistoryCandidateResolution {
    Ready,
    Skip(&'static str),
}

pub(crate) async fn resolve_openai_response_history(
    runtime_state: &RuntimeState,
    request: &Value,
    client_api_format: &str,
    provider_api_format: &str,
    user_id: &str,
    api_key_id: &str,
    provider_id: &str,
    endpoint_id: &str,
    provider_key_id: &str,
) -> Result<ConversationHistoryCandidateResolution, GatewayError> {
    let resolution =
        match ConversationHistoryResolver::resolve(request, client_api_format, provider_api_format)
        {
            Ok(resolution) => resolution,
            Err(ConversationHistoryResolutionError::Unsupported { .. }) => {
                return Ok(ConversationHistoryCandidateResolution::Skip(
                    "conversation_history_unsupported",
                ));
            }
            Err(error) => return Err(map_history_resolution_error(error)),
        };
    let Some(resolution) = resolution else {
        return Ok(ConversationHistoryCandidateResolution::Ready);
    };
    if resolution.capability == ConversationHistoryCapability::Native {
        resolve_native_response_history_advisory(
            runtime_state,
            resolution.previous_response_id,
            provider_api_format,
            user_id,
            api_key_id,
            provider_id,
            endpoint_id,
            provider_key_id,
        )
        .await;
        return Ok(ConversationHistoryCandidateResolution::Ready);
    }
    let Some(history_scope) = conversation_history_scope(user_id, api_key_id) else {
        return Ok(ConversationHistoryCandidateResolution::Skip(
            "conversation_history_scope_unavailable",
        ));
    };
    if response_history_is_loaded(
        resolution.previous_response_id,
        Some(history_scope.as_str()),
    ) {
        return Ok(ConversationHistoryCandidateResolution::Ready);
    }

    let storage_key = response_history_storage_key(
        resolution.previous_response_id,
        Some(history_scope.as_str()),
    );
    #[cfg(test)]
    if take_response_history_lookup_failure(&storage_key) {
        return Ok(ConversationHistoryCandidateResolution::Skip(
            "conversation_history_lookup_failed",
        ));
    }
    let payload = match runtime_state.kv_get(&storage_key).await {
        Ok(payload) => payload,
        Err(error) => {
            warn!(
                event_name = "openai_response_history_read_failed",
                log_type = "ops",
                backend = runtime_state.backend_kind().as_str(),
                error = ?error,
                "gateway skipped a candidate whose OpenAI response history could not be read"
            );
            return Ok(ConversationHistoryCandidateResolution::Skip(
                "conversation_history_lookup_failed",
            ));
        }
    };
    let Some(payload) = payload else {
        return Ok(ConversationHistoryCandidateResolution::Skip(
            "conversation_history_unavailable",
        ));
    };
    if let Err(error) = hydrate_response_history(
        resolution.previous_response_id,
        Some(history_scope.as_str()),
        &payload,
    ) {
        let _ = runtime_state.kv_delete(&storage_key).await;
        warn!(
            event_name = "openai_response_history_invalid",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            error = %error,
            "gateway rejected invalid shared OpenAI response history"
        );
        return Ok(ConversationHistoryCandidateResolution::Skip(
            "conversation_history_unmaterializable",
        ));
    }
    Ok(ConversationHistoryCandidateResolution::Ready)
}

#[allow(clippy::too_many_arguments)]
async fn resolve_native_response_history_advisory(
    runtime_state: &RuntimeState,
    previous_response_id: &str,
    provider_api_format: &str,
    user_id: &str,
    api_key_id: &str,
    provider_id: &str,
    endpoint_id: &str,
    provider_key_id: &str,
) {
    let Some(history_scope) = conversation_history_scope(user_id, api_key_id) else {
        warn!(
            event_name = "openai_native_response_history_scope_unavailable",
            log_type = "ops",
            "gateway continued with a provider-native handle without a local history scope"
        );
        return;
    };
    let storage_key = response_history_storage_key(previous_response_id, Some(&history_scope));
    #[cfg(test)]
    if take_response_history_lookup_failure(&storage_key) {
        warn!(
            event_name = "openai_native_response_history_read_failed",
            log_type = "ops",
            "gateway continued with a provider-native handle after a forced local history lookup failure"
        );
        return;
    }
    let payload = match runtime_state.kv_get(&storage_key).await {
        Ok(Some(payload)) => payload,
        Ok(None) => return,
        Err(error) => {
            warn!(
                event_name = "openai_native_response_history_read_failed",
                log_type = "ops",
                backend = runtime_state.backend_kind().as_str(),
                error = ?error,
                "gateway continued with a provider-native handle after local history lookup failed"
            );
            return;
        }
    };
    match validate_native_response_history(
        previous_response_id,
        &history_scope,
        provider_api_format,
        provider_id,
        endpoint_id,
        provider_key_id,
        &payload,
    ) {
        Ok(NativeResponseHistoryValidation::Match) => {
            if let Err(error) = hydrate_native_response_history_if_materialized(
                previous_response_id,
                &history_scope,
                &payload,
            ) {
                warn!(
                    event_name = "openai_native_response_history_transcript_invalid",
                    log_type = "ops",
                    error = %error,
                    "gateway continued with a provider-native handle after advisory transcript hydration failed"
                );
            }
        }
        Ok(NativeResponseHistoryValidation::CandidateMismatch) => {
            warn!(
                event_name = "openai_native_response_history_binding_mismatch",
                log_type = "ops",
                "gateway continued with a provider-native handle despite an advisory binding mismatch"
            );
        }
        Err(error) => {
            warn!(
                event_name = "openai_native_response_history_binding_invalid",
                log_type = "ops",
                error = %error,
                "gateway continued with a provider-native handle despite invalid advisory history"
            );
        }
    }
}

fn map_history_resolution_error(error: ConversationHistoryResolutionError) -> GatewayError {
    let status = match &error {
        ConversationHistoryResolutionError::InvalidPreviousResponseId => StatusCode::BAD_REQUEST,
        ConversationHistoryResolutionError::Unsupported { .. } => StatusCode::UNPROCESSABLE_ENTITY,
    };
    GatewayError::Client {
        status,
        message: error.to_string(),
    }
}

pub(crate) fn conversation_history_candidates_exhausted_error(
    request: &Value,
) -> Option<GatewayError> {
    let previous_response_id = request.get("previous_response_id")?;
    if previous_response_id
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Some(GatewayError::Client {
            status: StatusCode::BAD_REQUEST,
            message: ConversationHistoryResolutionError::InvalidPreviousResponseId.to_string(),
        });
    }
    Some(GatewayError::Client {
        status: StatusCode::CONFLICT,
        message: "no provider candidate can continue the requested conversation history"
            .to_string(),
    })
}

#[cfg(test)]
fn response_history_persistence_failures() -> &'static Mutex<HashSet<String>> {
    static FAILURES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    FAILURES.get_or_init(|| Mutex::new(HashSet::new()))
}

#[cfg(test)]
fn response_history_lookup_failures() -> &'static Mutex<HashSet<String>> {
    static FAILURES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    FAILURES.get_or_init(|| Mutex::new(HashSet::new()))
}

#[cfg(test)]
pub(crate) struct ResponseHistoryPersistenceFailureGuard {
    storage_key: String,
}

#[cfg(test)]
pub(crate) struct ResponseHistoryLookupFailureGuard {
    storage_key: String,
}

#[cfg(test)]
impl Drop for ResponseHistoryLookupFailureGuard {
    fn drop(&mut self) {
        response_history_lookup_failures()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.storage_key);
    }
}

#[cfg(test)]
impl Drop for ResponseHistoryPersistenceFailureGuard {
    fn drop(&mut self) {
        response_history_persistence_failures()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.storage_key);
    }
}

#[cfg(test)]
pub(crate) fn fail_next_response_history_persistence_for_tests(
    storage_key: impl Into<String>,
) -> ResponseHistoryPersistenceFailureGuard {
    let storage_key = storage_key.into();
    let inserted = response_history_persistence_failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(storage_key.clone());
    assert!(
        inserted,
        "history persistence failure already armed for this key"
    );
    ResponseHistoryPersistenceFailureGuard { storage_key }
}

#[cfg(test)]
pub(crate) fn fail_next_response_history_lookup_for_tests(
    storage_key: impl Into<String>,
) -> ResponseHistoryLookupFailureGuard {
    let storage_key = storage_key.into();
    let inserted = response_history_lookup_failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(storage_key.clone());
    assert!(
        inserted,
        "history lookup failure already armed for this key"
    );
    ResponseHistoryLookupFailureGuard { storage_key }
}

#[cfg(test)]
fn take_response_history_persistence_failure(storage_key: &str) -> bool {
    response_history_persistence_failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(storage_key)
}

#[cfg(test)]
fn take_response_history_lookup_failure(storage_key: &str) -> bool {
    response_history_lookup_failures()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(storage_key)
}

pub(crate) async fn persist_response_history_record(
    runtime_state: &RuntimeState,
    record: ResponseHistoryRecord,
) -> Result<(), GatewayError> {
    #[cfg(test)]
    if take_response_history_persistence_failure(&record.storage_key) {
        return persist_response_history_record_with(record, |_, _, _| async {
            Err("forced response history KV failure".to_string())
        })
        .await;
    }
    persist_response_history_record_with(record, |storage_key, payload, ttl| async move {
        runtime_state
            .kv_create_if_absent_or_same(&storage_key, payload, Some(ttl))
            .await
            .map_err(|error| error.to_string())
    })
    .await
}

pub(crate) async fn persist_response_history_record_advisory(
    runtime_state: &RuntimeState,
    record: ResponseHistoryRecord,
) {
    let _ = persist_response_history_record(runtime_state, record).await;
}

async fn persist_response_history_record_with<F, Fut>(
    record: ResponseHistoryRecord,
    write: F,
) -> Result<(), GatewayError>
where
    F: FnOnce(String, String, std::time::Duration) -> Fut,
    Fut: std::future::Future<Output = Result<KvCreateResult, String>>,
{
    let outcome = write(
        record.storage_key.clone(),
        record.payload.clone(),
        record.ttl,
    )
    .await
    .map_err(|error| {
        warn!(
            event_name = "openai_response_history_write_failed",
            log_type = "ops",
            error = %error,
            "gateway failed to persist shared OpenAI response history"
        );
        GatewayError::Internal("OpenAI response history persistence failed".to_string())
    })?;
    if outcome == KvCreateResult::Conflict {
        warn!(
            event_name = "openai_response_history_conflict",
            log_type = "ops",
            storage_key = %record.storage_key,
            "gateway refused to overwrite divergent OpenAI response history"
        );
        return Err(GatewayError::Internal(
            "OpenAI response history conflicts with an existing record".to_string(),
        ));
    }
    commit_response_history_record(&record);
    Ok(())
}

pub(crate) async fn persist_converted_response_history(
    runtime_state: &RuntimeState,
    report_context: &Value,
    response: Option<&Value>,
) -> Result<(), GatewayError> {
    let Some(response) = response else {
        return Ok(());
    };
    let record = match try_record_converted_response_history(report_context, response) {
        Ok(record) => record,
        Err(error) => {
            warn!(
                event_name = "openai_response_history_record_build_failed",
                log_type = "ops",
                error = %error,
                "gateway could not construct advisory OpenAI response history"
            );
            return Ok(());
        }
    };
    if let Some(record) = record {
        persist_response_history_record_advisory(runtime_state, record).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};
    use axum::http::StatusCode;
    use serde_json::json;
    use tokio::sync::Barrier;

    use super::{
        conversation_history_candidates_exhausted_error,
        fail_next_response_history_lookup_for_tests,
        fail_next_response_history_persistence_for_tests, persist_converted_response_history,
        persist_response_history_record, persist_response_history_record_with,
        resolve_openai_response_history, ConversationHistoryCandidateResolution,
    };
    use crate::ai_serving::{
        build_standard_request_body_with_model_directives_request_headers_and_history_scope,
        conversation_history_scope, evict_response_history, record_converted_response_history,
        response_history_is_loaded,
    };
    use crate::GatewayError;

    #[test]
    fn continuation_candidate_exhaustion_maps_to_one_client_conflict() {
        assert!(
            conversation_history_candidates_exhausted_error(&json!({"input": "new"})).is_none()
        );
        let error = conversation_history_candidates_exhausted_error(&json!({
            "previous_response_id": "resp_exhausted",
            "input": "continue"
        }))
        .expect("continuation exhaustion should produce a client error");
        assert!(matches!(
            error,
            GatewayError::Client { status, message }
                if status == StatusCode::CONFLICT && message.contains("no provider candidate")
        ));
    }

    #[tokio::test]
    async fn history_persistence_failure_is_not_reported_as_success() {
        let response_id = "resp_history_write_failure";
        let scope = conversation_history_scope("history-write-user", "history-write-key")
            .expect("complete identity should produce a scope");
        evict_response_history(response_id, Some(scope.as_str()));
        let record = record_converted_response_history(
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:chat",
                "user_id": "history-write-user",
                "api_key_id": "history-write-key",
                "original_request_body": {"input": "must not be cached"}
            }),
            &json!({
                "id": response_id,
                "status": "completed",
                "output": []
            }),
        )
        .expect("completed converted response should produce a record");
        assert!(!response_history_is_loaded(
            response_id,
            Some(scope.as_str())
        ));

        let error = persist_response_history_record_with(record, |_, _, _| async {
            Err("forced KV failure".to_string())
        })
        .await
        .expect_err("KV persistence failure must fail closed");
        assert!(
            matches!(error, GatewayError::Internal(message) if message.contains("persistence failed"))
        );
        assert!(!response_history_is_loaded(
            response_id,
            Some(scope.as_str())
        ));
    }

    #[tokio::test]
    async fn native_http_success_ignores_advisory_kv_write_failure() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let response_id = "resp_native_http_advisory_write";
        let scope = conversation_history_scope("native-http-user", "native-http-key").unwrap();
        let storage_key = response_history_storage_key(response_id, Some(&scope));
        evict_response_history(response_id, Some(&scope));
        let _failure = fail_next_response_history_persistence_for_tests(storage_key.clone());
        let response = json!({
            "id": response_id,
            "status": "completed",
            "output": []
        });

        persist_converted_response_history(
            &runtime,
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "user_id": "native-http-user",
                "api_key_id": "native-http-key",
                "provider_id": "provider-native",
                "endpoint_id": "endpoint-native",
                "key_id": "credential-native",
                "original_request_body": {"input": "native HTTP request"}
            }),
            Some(&response),
        )
        .await
        .expect("successful HTTP response must not fail because advisory history did not persist");

        assert!(!response_history_is_loaded(response_id, Some(&scope)));
        assert!(runtime
            .kv_get(&storage_key)
            .await
            .expect("memory KV lookup should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn concurrent_divergent_history_never_overwrites_the_winner_or_cache() {
        let response_id = "resp_history_atomic_publication";
        let scope = conversation_history_scope("atomic-user", "atomic-key").unwrap();
        evict_response_history(response_id, Some(scope.as_str()));
        let build_record = |text: &str| {
            record_converted_response_history(
                &json!({
                    "client_api_format": "openai:responses",
                    "provider_api_format": "openai:chat",
                    "user_id": "atomic-user",
                    "api_key_id": "atomic-key",
                    "original_request_body": {"input": "atomic request"}
                }),
                &json!({
                    "id": response_id,
                    "status": "completed",
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}]
                    }]
                }),
            )
            .expect("completed response should produce history")
        };
        let record_a = build_record("atomic-response-a");
        let record_b = build_record("atomic-response-b");
        assert_eq!(record_a.storage_key, record_b.storage_key);
        let storage_key = record_a.storage_key.clone();
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let barrier = Arc::new(Barrier::new(3));
        let spawn_write =
            |runtime: RuntimeState,
             barrier: Arc<Barrier>,
             record: crate::ai_serving::ResponseHistoryRecord| {
                tokio::spawn(async move {
                    barrier.wait().await;
                    persist_response_history_record(&runtime, record).await
                })
            };
        let first = spawn_write(runtime.clone(), Arc::clone(&barrier), record_a.clone());
        let second = spawn_write(runtime.clone(), Arc::clone(&barrier), record_b.clone());
        barrier.wait().await;
        let first = first.await.unwrap();
        let second = second.await.unwrap();
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        assert_eq!(
            usize::from(first.is_err()) + usize::from(second.is_err()),
            1
        );
        let persisted_payload = runtime.kv_get(&storage_key).await.unwrap().unwrap();
        let (winner, winner_text, loser_text) = if persisted_payload == record_a.payload {
            (record_a, "atomic-response-a", "atomic-response-b")
        } else {
            assert_eq!(persisted_payload, record_b.payload);
            (record_b, "atomic-response-b", "atomic-response-a")
        };

        persist_response_history_record(&runtime, winner)
            .await
            .expect("replaying the exact history record must be idempotent");

        let continuation = json!({
            "previous_response_id": response_id,
            "input": "continue after atomic writes"
        });
        let provider_body =
            build_standard_request_body_with_model_directives_request_headers_and_history_scope(
                &continuation,
                "openai:responses",
                "mapped-model",
                "custom",
                "openai:chat",
                "/v1/responses",
                false,
                None,
                Some("atomic-key"),
                Some(scope.as_str()),
                None,
                false,
            )
            .expect("the cache should expose only the atomically persisted winner");
        let provider_body = provider_body.to_string();
        assert!(provider_body.contains(winner_text));
        assert!(!provider_body.contains(loser_text));
    }

    #[tokio::test]
    async fn divergent_native_binding_conflicts_without_overwriting_shared_history() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let response_id = "resp_native_binding_conflict";
        let build_record = |provider_id: &str| {
            record_converted_response_history(
                &json!({
                    "client_api_format": "openai:responses",
                    "provider_api_format": "openai:responses",
                    "user_id": "binding-conflict-user",
                    "api_key_id": "binding-conflict-key",
                    "provider_id": provider_id,
                    "endpoint_id": "binding-conflict-endpoint",
                    "key_id": "binding-conflict-credential",
                    "original_request_body": {"input": "same native request"}
                }),
                &json!({"id": response_id, "status": "completed", "output": []}),
            )
            .expect("owned native response should produce history")
        };
        let first = build_record("provider-first");
        let conflicting = build_record("provider-conflicting");
        let storage_key = first.storage_key.clone();
        let first_payload = first.payload.clone();
        persist_response_history_record(&runtime, first)
            .await
            .expect("first native binding should win");
        let error = persist_response_history_record(&runtime, conflicting)
            .await
            .expect_err("a different native binding must conflict");
        assert!(matches!(
            error,
            GatewayError::Internal(message) if message.contains("conflicts")
        ));
        assert_eq!(
            runtime.kv_get(&storage_key).await.expect("history read"),
            Some(first_payload)
        );
    }

    #[tokio::test]
    async fn http_history_round_trip_restores_only_the_requesting_tenant() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let response_id = "resp_http_persisted_scope";

        for (user_id, api_key_id, prompt, call_id) in [
            (
                "tenant-http-a",
                "key-http",
                "tenant-a-private",
                "call-http-a",
            ),
            (
                "tenant-http-b",
                "key-http",
                "tenant-b-private",
                "call-http-b",
            ),
        ] {
            let record = record_converted_response_history(
                &json!({
                    "client_api_format": "openai:responses",
                    "provider_api_format": "openai:chat",
                    "user_id": user_id,
                    "api_key_id": api_key_id,
                    "original_request_body": {
                        "model": "source-model",
                        "input": prompt
                    }
                }),
                &json!({
                    "id": response_id,
                    "status": "completed",
                    "output": [{
                        "type": "function_call",
                        "id": format!("fc-{call_id}"),
                        "call_id": call_id,
                        "name": "inspect_repository",
                        "arguments": "{}"
                    }]
                }),
            )
            .expect("a fully scoped HTTP response should produce history");
            persist_response_history_record(&runtime, record)
                .await
                .expect("memory history persistence should succeed");
        }

        for (user_id, call_id, own_prompt, other_prompt) in [
            (
                "tenant-http-a",
                "call-http-a",
                "tenant-a-private",
                "tenant-b-private",
            ),
            (
                "tenant-http-b",
                "call-http-b",
                "tenant-b-private",
                "tenant-a-private",
            ),
        ] {
            let scope = conversation_history_scope(user_id, "key-http").unwrap();
            evict_response_history(response_id, Some(scope.as_str()));
            assert!(!response_history_is_loaded(
                response_id,
                Some(scope.as_str())
            ));

            let continuation = json!({
                "model": "source-model",
                "previous_response_id": response_id,
                "input": [{
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": "inspection-complete"
                }]
            });
            resolve_openai_response_history(
                &runtime,
                &continuation,
                "openai:responses",
                "openai:chat",
                user_id,
                "key-http",
                "",
                "",
                "",
            )
            .await
            .expect("the HTTP resolver should hydrate the requester's persisted history");

            let provider_body =
                build_standard_request_body_with_model_directives_request_headers_and_history_scope(
                    &continuation,
                    "openai:responses",
                    "mapped-model",
                    "custom",
                    "openai:chat",
                    "/v1/responses",
                    false,
                    None,
                    Some("key-http"),
                    Some(scope.as_str()),
                    None,
                    false,
                )
                .expect("hydrated history should build the provider request");
            let serialized = provider_body.to_string();
            assert!(serialized.contains(own_prompt));
            assert!(serialized.contains(call_id));
            assert!(serialized.contains("inspection-complete"));
            assert!(!serialized.contains(other_prompt));
        }
    }

    #[tokio::test]
    async fn native_http_and_websocket_continuations_do_not_require_gateway_history() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        for request in [
            json!({
                "model": "gpt-5",
                "input": "continue over HTTP",
                "previous_response_id": "resp_owned_by_provider_http"
            }),
            json!({
                "type": "response.create",
                "model": "gpt-5",
                "input": "continue over WebSocket",
                "previous_response_id": "resp_owned_by_provider_ws"
            }),
        ] {
            resolve_openai_response_history(
                &runtime,
                &request,
                "openai:responses",
                "openai:responses",
                "",
                "",
                "provider-native",
                "endpoint-native",
                "key-native",
            )
            .await
            .map(|resolution| assert_eq!(resolution, ConversationHistoryCandidateResolution::Ready))
            .expect("provider-native handles must not depend on local identity or history");
        }
    }

    #[tokio::test]
    async fn native_binding_is_advisory_for_every_provider_candidate() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let response_id = "resp_native_owned";
        let record = record_converted_response_history(
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "user_id": "tenant-native",
                "api_key_id": "api-key-native",
                "provider_id": "provider-native",
                "endpoint_id": "endpoint-native",
                "key_id": "credential-native",
                "original_request_body": {"input": "native turn"}
            }),
            &json!({
                "id": response_id,
                "status": "completed",
                "output": []
            }),
        )
        .expect("fully owned native history should produce a record");
        persist_response_history_record(&runtime, record)
            .await
            .expect("memory history persistence should succeed");
        let continuation = json!({
            "previous_response_id": response_id,
            "input": "continue"
        });

        let resolution = resolve_openai_response_history(
            &runtime,
            &continuation,
            "openai:responses",
            "openai:responses",
            "tenant-native",
            "api-key-native",
            "provider-native",
            "endpoint-native",
            "credential-native",
        )
        .await
        .expect("the exact requester and provider credential should own the native response");
        assert_eq!(resolution, ConversationHistoryCandidateResolution::Ready);

        for (provider_id, endpoint_id, provider_key_id) in [
            ("other-provider", "endpoint-native", "credential-native"),
            ("provider-native", "other-endpoint", "credential-native"),
            ("provider-native", "endpoint-native", "other-credential"),
        ] {
            let resolution = resolve_openai_response_history(
                &runtime,
                &continuation,
                "openai:responses",
                "openai:responses",
                "tenant-native",
                "api-key-native",
                provider_id,
                endpoint_id,
                provider_key_id,
            )
            .await
            .expect("a mismatched advisory binding must not reject a provider candidate");
            assert_eq!(resolution, ConversationHistoryCandidateResolution::Ready);
        }

        let other_tenant = resolve_openai_response_history(
            &runtime,
            &continuation,
            "openai:responses",
            "openai:responses",
            "other-tenant",
            "api-key-native",
            "provider-native",
            "endpoint-native",
            "credential-native",
        )
        .await
        .expect("a missing scoped binding must preserve the provider-owned handle");
        assert_eq!(other_tenant, ConversationHistoryCandidateResolution::Ready);
    }

    #[tokio::test]
    async fn native_provider_handle_is_ready_when_scoped_binding_is_missing() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let continuation = json!({
            "previous_response_id": "resp_owned_by_provider_but_not_cached_here",
            "input": "continue on provider"
        });
        let resolution = resolve_openai_response_history(
            &runtime,
            &continuation,
            "openai:responses",
            "openai:responses",
            "native-missing-tenant",
            "native-missing-key",
            "provider-native",
            "endpoint-native",
            "credential-native",
        )
        .await
        .expect("provider-owned native handles do not require local history");
        assert_eq!(resolution, ConversationHistoryCandidateResolution::Ready);
    }

    #[tokio::test]
    async fn native_http_and_websocket_ignore_local_kv_and_record_failures() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let scope = conversation_history_scope("native-advisory-user", "native-advisory-key")
            .expect("complete identity should produce a scope");
        for (suffix, request) in [
            (
                "http",
                json!({
                    "model": "gpt-5",
                    "previous_response_id": "resp_native_advisory_http",
                    "input": "continue over HTTP"
                }),
            ),
            (
                "ws",
                json!({
                    "type": "response.create",
                    "model": "gpt-5",
                    "previous_response_id": "resp_native_advisory_ws",
                    "input": "continue over WebSocket"
                }),
            ),
        ] {
            let response_id = request["previous_response_id"].as_str().unwrap();
            let storage_key = response_history_storage_key(response_id, Some(&scope));
            let _lookup_failure = fail_next_response_history_lookup_for_tests(storage_key.clone());
            let resolution = resolve_openai_response_history(
                &runtime,
                &request,
                "openai:responses",
                "openai:responses",
                "native-advisory-user",
                "native-advisory-key",
                "provider-native",
                "endpoint-native",
                "credential-native",
            )
            .await
            .expect("native continuation must ignore local KV read failure");
            assert_eq!(resolution, ConversationHistoryCandidateResolution::Ready);

            runtime
                .kv_set(
                    &storage_key,
                    format!("corrupt-{suffix}"),
                    Some(Duration::from_secs(60)),
                )
                .await
                .expect("test history should be written");
            let resolution = resolve_openai_response_history(
                &runtime,
                &request,
                "openai:responses",
                "openai:responses",
                "native-advisory-user",
                "native-advisory-key",
                "provider-native",
                "endpoint-native",
                "credential-native",
            )
            .await
            .expect("corrupt local history must remain advisory for native continuation");
            assert_eq!(resolution, ConversationHistoryCandidateResolution::Ready);
        }

        let response_id = "resp_native_advisory_expired";
        let mut record = record_converted_response_history(
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "user_id": "native-advisory-user",
                "api_key_id": "native-advisory-key",
                "provider_id": "provider-native",
                "endpoint_id": "endpoint-native",
                "key_id": "credential-native",
                "original_request_body": {"input": "expired native turn"}
            }),
            &json!({"id": response_id, "status": "completed", "output": []}),
        )
        .expect("complete native response should produce advisory history");
        let mut payload: serde_json::Value = serde_json::from_str(&record.payload).unwrap();
        payload["expires_at_unix_secs"] = json!(0);
        record.payload = payload.to_string();
        runtime
            .kv_set(
                &record.storage_key,
                record.payload,
                Some(Duration::from_secs(60)),
            )
            .await
            .expect("expired test history should be written");
        let resolution = resolve_openai_response_history(
            &runtime,
            &json!({"previous_response_id": response_id, "input": "continue"}),
            "openai:responses",
            "openai:responses",
            "native-advisory-user",
            "native-advisory-key",
            "provider-native",
            "endpoint-native",
            "credential-native",
        )
        .await
        .expect("expired local history must remain advisory for native continuation");
        assert_eq!(resolution, ConversationHistoryCandidateResolution::Ready);
    }

    #[tokio::test]
    async fn native_provider_resolved_record_validates_without_entering_materialization_cache() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let response_id = "resp_native_provider_resolved_child";
        let scope = conversation_history_scope("provider-resolved-user", "provider-resolved-key")
            .expect("complete identity should produce a scope");
        evict_response_history(response_id, Some(scope.as_str()));
        let record = record_converted_response_history(
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "user_id": "provider-resolved-user",
                "api_key_id": "provider-resolved-key",
                "provider_id": "provider-resolved-provider",
                "endpoint_id": "provider-resolved-endpoint",
                "key_id": "provider-resolved-credential",
                "original_request_body": {
                    "previous_response_id": "resp_native_parent_only_on_provider",
                    "input": "continue from provider-owned context"
                }
            }),
            &json!({
                "id": response_id,
                "status": "completed",
                "output": []
            }),
        )
        .expect("native terminal construction should not require a local parent transcript");
        assert!(record.payload.contains("\"transcript_materialized\":false"));
        persist_response_history_record(&runtime, record)
            .await
            .expect("provider-resolved native history should persist");
        assert!(!response_history_is_loaded(
            response_id,
            Some(scope.as_str())
        ));

        let continuation = json!({
            "previous_response_id": response_id,
            "input": "continue natively"
        });
        let resolution = resolve_openai_response_history(
            &runtime,
            &continuation,
            "openai:responses",
            "openai:responses",
            "provider-resolved-user",
            "provider-resolved-key",
            "provider-resolved-provider",
            "provider-resolved-endpoint",
            "provider-resolved-credential",
        )
        .await
        .expect("matching native binding should accept provider-resolved history");
        assert_eq!(resolution, ConversationHistoryCandidateResolution::Ready);
        assert!(!response_history_is_loaded(
            response_id,
            Some(scope.as_str())
        ));

        let converted_resolution = resolve_openai_response_history(
            &runtime,
            &continuation,
            "openai:responses",
            "openai:chat",
            "provider-resolved-user",
            "provider-resolved-key",
            "provider-resolved-provider",
            "provider-resolved-endpoint",
            "provider-resolved-credential",
        )
        .await
        .expect("provider-resolved history should skip only this converted candidate");
        assert_eq!(
            converted_resolution,
            ConversationHistoryCandidateResolution::Skip("conversation_history_unmaterializable")
        );
    }

    #[tokio::test]
    async fn native_advisory_binding_preserves_original_candidate_order() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let response_id = "resp_native_candidate_fallback";
        let record = record_converted_response_history(
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "user_id": "tenant-fallback",
                "api_key_id": "api-key-fallback",
                "provider_id": "provider-bound",
                "endpoint_id": "endpoint-bound",
                "key_id": "credential-bound",
                "original_request_body": {"input": "first turn"}
            }),
            &json!({"id": response_id, "status": "completed", "output": []}),
        )
        .expect("bound native response should produce history");
        persist_response_history_record(&runtime, record)
            .await
            .expect("memory history persistence should succeed");
        let continuation = json!({
            "previous_response_id": response_id,
            "input": "continue"
        });

        let first = resolve_openai_response_history(
            &runtime,
            &continuation,
            "openai:responses",
            "openai:responses",
            "tenant-fallback",
            "api-key-fallback",
            "provider-other",
            "endpoint-other",
            "credential-other",
        )
        .await
        .expect("the first candidate must remain usable despite an advisory mismatch");
        assert_eq!(first, ConversationHistoryCandidateResolution::Ready);

        let fallback = resolve_openai_response_history(
            &runtime,
            &continuation,
            "openai:responses",
            "openai:responses",
            "tenant-fallback",
            "api-key-fallback",
            "provider-bound",
            "endpoint-bound",
            "credential-bound",
        )
        .await
        .expect("the bound candidate should be accepted");
        assert_eq!(fallback, ConversationHistoryCandidateResolution::Ready);
    }

    #[tokio::test]
    async fn native_two_node_chain_is_complete_for_a_later_translated_turn() {
        let node_one = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let node_two = node_one.clone();
        let scope = conversation_history_scope("tenant-chain", "api-key-chain").unwrap();
        let binding = json!({
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:responses",
            "user_id": "tenant-chain",
            "api_key_id": "api-key-chain",
            "provider_id": "provider-native",
            "endpoint_id": "endpoint-native",
            "key_id": "credential-native"
        });

        let mut turn_one_context = binding.clone();
        turn_one_context["original_request_body"] = json!({"input": "native turn one request"});
        let turn_one = record_converted_response_history(
            &turn_one_context,
            &json!({
                "id": "resp_native_chain_one",
                "status": "completed",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "native turn one response"}]
                }]
            }),
        )
        .expect("the first native turn should produce history");
        persist_response_history_record(&node_one, turn_one)
            .await
            .expect("the first node should persist the native turn");

        evict_response_history("resp_native_chain_one", Some(scope.as_str()));
        let turn_two_request = json!({
            "previous_response_id": "resp_native_chain_one",
            "input": "native turn two request"
        });
        let resolution = resolve_openai_response_history(
            &node_two,
            &turn_two_request,
            "openai:responses",
            "openai:responses",
            "tenant-chain",
            "api-key-chain",
            "provider-native",
            "endpoint-native",
            "credential-native",
        )
        .await
        .expect("the second node should validate and hydrate the native parent");
        assert_eq!(resolution, ConversationHistoryCandidateResolution::Ready);

        let mut turn_two_context = binding;
        turn_two_context["original_request_body"] = turn_two_request;
        let turn_two = record_converted_response_history(
            &turn_two_context,
            &json!({
                "id": "resp_native_chain_two",
                "status": "completed",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "native turn two response"}]
                }]
            }),
        )
        .expect("the second native turn should inherit the complete parent transcript");
        persist_response_history_record(&node_two, turn_two)
            .await
            .expect("the second node should persist the inherited native chain");

        evict_response_history("resp_native_chain_two", Some(scope.as_str()));
        let translated_request = json!({
            "previous_response_id": "resp_native_chain_two",
            "input": "translated turn three request"
        });
        let resolution = resolve_openai_response_history(
            &node_one,
            &translated_request,
            "openai:responses",
            "openai:chat",
            "tenant-chain",
            "api-key-chain",
            "",
            "",
            "",
        )
        .await
        .expect("a later translated turn should hydrate the complete child chain");
        assert_eq!(resolution, ConversationHistoryCandidateResolution::Ready);

        let provider_body =
            build_standard_request_body_with_model_directives_request_headers_and_history_scope(
                &translated_request,
                "openai:responses",
                "mapped-model",
                "custom",
                "openai:chat",
                "/v1/responses",
                false,
                None,
                Some("api-key-chain"),
                Some(scope.as_str()),
                None,
                false,
            )
            .expect("the translated request should build from the inherited transcript")
            .to_string();
        for expected in [
            "native turn one request",
            "native turn one response",
            "native turn two request",
            "native turn two response",
            "translated turn three request",
        ] {
            assert!(provider_body.contains(expected), "missing {expected}");
        }
    }

    #[tokio::test]
    async fn local_http_and_websocket_continuations_skip_without_scoped_history() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        for (request, provider_api_format) in [
            (
                json!({
                    "model": "chat-model",
                    "input": "continue over HTTP",
                    "previous_response_id": "resp_missing_http"
                }),
                "openai:chat",
            ),
            (
                json!({
                    "type": "response.create",
                    "model": "claude-model",
                    "input": "continue over WebSocket",
                    "previous_response_id": "resp_missing_ws"
                }),
                "claude:messages",
            ),
        ] {
            let resolution = resolve_openai_response_history(
                &runtime,
                &request,
                "openai:responses",
                provider_api_format,
                "tenant-a",
                "key-a",
                "",
                "",
                "",
            )
            .await
            .expect("hydrate and translate failures should be candidate-local");
            assert_eq!(
                resolution,
                ConversationHistoryCandidateResolution::Skip("conversation_history_unavailable")
            );
        }
    }

    #[tokio::test]
    async fn websocket_continuation_validation_remains_fail_closed() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let invalid = resolve_openai_response_history(
            &runtime,
            &json!({
                "type": "response.create",
                "previous_response_id": {"unexpected": true}
            }),
            "openai:responses",
            "openai:responses",
            "tenant-a",
            "key-a",
            "provider-a",
            "endpoint-a",
            "key-a",
        )
        .await
        .expect_err("invalid native IDs must still be rejected");
        match invalid {
            GatewayError::Client { status, .. } => {
                assert_eq!(status, StatusCode::BAD_REQUEST);
            }
            other => panic!("expected a bad request, got {other:?}"),
        }

        let unsupported = resolve_openai_response_history(
            &runtime,
            &json!({
                "type": "response.create",
                "previous_response_id": "resp_unsupported"
            }),
            "openai:responses",
            "openai:search",
            "tenant-a",
            "key-a",
            "",
            "",
            "",
        )
        .await
        .expect("unsupported continuation pairs should skip the candidate");
        assert_eq!(
            unsupported,
            ConversationHistoryCandidateResolution::Skip("conversation_history_unsupported")
        );
    }
}
