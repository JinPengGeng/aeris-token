use crate::ai_serving::{
    conversation_history_scope, hydrate_response_history, record_converted_response_history,
    response_history_is_loaded, response_history_storage_key, ConversationHistoryCapability,
    ConversationHistoryResolutionError, ConversationHistoryResolver, ResponseHistoryRecord,
};
use aether_runtime_state::RuntimeState;
use axum::http::StatusCode;
use serde_json::Value;
use tracing::warn;

use crate::GatewayError;

pub(crate) async fn resolve_openai_response_history(
    runtime_state: &RuntimeState,
    request: &Value,
    client_api_format: &str,
    provider_api_format: &str,
    user_id: &str,
    api_key_id: &str,
) -> Result<(), GatewayError> {
    let resolution =
        ConversationHistoryResolver::resolve(request, client_api_format, provider_api_format)
            .map_err(map_history_resolution_error)?;
    let Some(resolution) = resolution else {
        return Ok(());
    };
    // Native IDs are provider-owned continuation handles. The local cache is
    // only required when the gateway must reconstruct a cross-format request.
    if resolution.capability == ConversationHistoryCapability::Native {
        return Ok(());
    }
    let history_scope = conversation_history_scope(user_id, api_key_id).ok_or_else(|| {
        GatewayError::Internal("conversation history requester identity is incomplete".to_string())
    })?;
    if response_history_is_loaded(
        resolution.previous_response_id,
        Some(history_scope.as_str()),
    ) {
        return Ok(());
    }

    let storage_key = response_history_storage_key(
        resolution.previous_response_id,
        Some(history_scope.as_str()),
    );
    let payload = runtime_state.kv_get(&storage_key).await.map_err(|error| {
        warn!(
            event_name = "openai_response_history_read_failed",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            error = ?error,
            "gateway failed to read shared OpenAI response history"
        );
        GatewayError::Internal("OpenAI response history lookup failed".to_string())
    })?;
    let Some(payload) = payload else {
        return Err(GatewayError::Client {
            status: StatusCode::CONFLICT,
            message: format!(
                "conversation history is unavailable for this tenant and API key ({})",
                history_capability_name(resolution.capability)
            ),
        });
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
        return Err(GatewayError::Internal(
            "OpenAI response history validation failed".to_string(),
        ));
    }
    Ok(())
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

const fn history_capability_name(capability: ConversationHistoryCapability) -> &'static str {
    match capability {
        ConversationHistoryCapability::Native => "native",
        ConversationHistoryCapability::Hydrate => "hydrate",
        ConversationHistoryCapability::Translate => "translate",
        ConversationHistoryCapability::Unsupported => "unsupported",
    }
}

pub(crate) async fn persist_response_history_record(
    runtime_state: &RuntimeState,
    record: ResponseHistoryRecord,
) {
    if let Err(error) = runtime_state
        .kv_set(&record.storage_key, record.payload, Some(record.ttl))
        .await
    {
        warn!(
            event_name = "openai_response_history_write_failed",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            error = ?error,
            "gateway failed to persist shared OpenAI response history"
        );
    }
}

pub(crate) async fn persist_converted_response_history(
    runtime_state: &RuntimeState,
    report_context: &Value,
    response: Option<&Value>,
) {
    let Some(response) = response else {
        return;
    };
    if let Some(record) = record_converted_response_history(report_context, response) {
        persist_response_history_record(runtime_state, record).await;
    }
}

#[cfg(test)]
mod tests {
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};
    use axum::http::StatusCode;
    use serde_json::json;

    use super::resolve_openai_response_history;
    use crate::GatewayError;

    #[tokio::test]
    async fn native_http_and_websocket_continuations_skip_gateway_history() {
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
            )
            .await
            .expect(
                "native provider-owned response IDs must not require identity or local history",
            );
        }
    }

    #[tokio::test]
    async fn local_http_and_websocket_continuations_fail_closed_without_scoped_history() {
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
            let error = resolve_openai_response_history(
                &runtime,
                &request,
                "openai:responses",
                provider_api_format,
                "tenant-a",
                "key-a",
            )
            .await
            .expect_err("hydrate and translate paths require scoped gateway history");
            match error {
                GatewayError::Client { status, .. } => {
                    assert_eq!(status, StatusCode::CONFLICT);
                }
                other => panic!("expected a client conflict, got {other:?}"),
            }
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
        )
        .await
        .expect_err("unsupported continuation pairs must still be rejected");
        match unsupported {
            GatewayError::Client { status, .. } => {
                assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            }
            other => panic!("expected an unprocessable entity, got {other:?}"),
        }
    }
}
