use crate::ai_serving::{
    commit_response_history_record, conversation_history_scope, hydrate_response_history,
    response_history_is_loaded, response_history_storage_key,
    try_record_converted_response_history, validate_native_response_history,
    ConversationHistoryCapability, ConversationHistoryResolutionError, ConversationHistoryResolver,
    NativeResponseHistoryValidation, ResponseHistoryRecord,
};
use axum::http::StatusCode;
use serde_json::Value;
use tracing::warn;

use crate::{AppState, GatewayError};

const RESPONSE_HISTORY_SECRET_PURPOSE: &str = "openai-response-history";

#[allow(clippy::too_many_arguments)]
pub(crate) async fn hydrate_openai_response_history(
    state: &AppState,
    request: &Value,
    client_api_format: &str,
    provider_api_format: &str,
    user_id: &str,
    api_key_id: &str,
    provider_id: &str,
    endpoint_id: &str,
    provider_key_id: &str,
) -> Result<Option<&'static str>, GatewayError> {
    let resolution =
        match ConversationHistoryResolver::resolve(request, client_api_format, provider_api_format)
        {
            Ok(value) => value,
            Err(ConversationHistoryResolutionError::Unsupported { .. }) => {
                return Ok(Some("conversation_history_unsupported"));
            }
            Err(error) => {
                return Err(GatewayError::Client {
                    status: StatusCode::BAD_REQUEST,
                    message: error.to_string(),
                });
            }
        };
    let Some(resolution) = resolution else {
        return Ok(None);
    };
    let Some(history_scope) = conversation_history_scope(user_id, api_key_id) else {
        // Native IDs are provider-owned continuation handles. No local
        // transcript or scope is required before forwarding them upstream.
        return Ok(
            (resolution.capability != ConversationHistoryCapability::Native)
                .then_some("conversation_history_scope_unavailable"),
        );
    };
    if resolution.capability != ConversationHistoryCapability::Native
        && response_history_is_loaded(resolution.previous_response_id, Some(&history_scope))
    {
        return Ok(None);
    }

    let storage_key =
        response_history_storage_key(resolution.previous_response_id, Some(&history_scope));
    let runtime_state = state.runtime_state();
    let payload = match runtime_state.kv_get(&storage_key).await {
        Ok(payload) => payload,
        Err(error) => {
            warn!(
                event_name = "openai_response_history_read_failed",
                log_type = "ops",
                backend = runtime_state.backend_kind().as_str(),
                error = ?error,
                "gateway skipped a candidate whose response history could not be read"
            );
            return Ok(
                if resolution.capability == ConversationHistoryCapability::Native {
                    None
                } else {
                    Some("conversation_history_lookup_failed")
                },
            );
        }
    };
    let Some(payload) = payload else {
        return Ok(
            if resolution.capability == ConversationHistoryCapability::Native {
                None
            } else {
                Some("conversation_history_unavailable")
            },
        );
    };
    let Some(payload) = crate::handlers::shared::open_runtime_secret_payload(
        state,
        RESPONSE_HISTORY_SECRET_PURPOSE,
        &payload,
    ) else {
        let _ = runtime_state.kv_delete(&storage_key).await;
        warn!(
            event_name = "openai_response_history_decryption_failed",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            "gateway rejected undecryptable shared OpenAI response history"
        );
        return Ok(Some("conversation_history_unmaterializable"));
    };
    if resolution.capability == ConversationHistoryCapability::Native {
        return Ok(
            match validate_native_response_history(
                resolution.previous_response_id,
                &history_scope,
                provider_api_format,
                provider_id,
                endpoint_id,
                provider_key_id,
                &payload,
            ) {
                Ok(NativeResponseHistoryValidation::CandidateMismatch) => {
                    Some("conversation_history_binding_mismatch")
                }
                _ => None,
            },
        );
    }
    if let Err(error) = hydrate_response_history(
        resolution.previous_response_id,
        Some(&history_scope),
        payload.as_str(),
    ) {
        let _ = runtime_state.kv_delete(&storage_key).await;
        warn!(
            event_name = "openai_response_history_invalid",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            error = %error,
            "gateway rejected invalid shared OpenAI response history"
        );
        return Ok(Some("conversation_history_unmaterializable"));
    }
    Ok(None)
}

pub(crate) async fn persist_response_history_record(
    state: &AppState,
    record: ResponseHistoryRecord,
) {
    let runtime_state = state.runtime_state();
    let Some(sealed_payload) = crate::handlers::shared::seal_runtime_secret_payload(
        state,
        RESPONSE_HISTORY_SECRET_PURPOSE,
        &record.payload,
    ) else {
        warn!(
            event_name = "openai_response_history_encryption_unavailable",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            "gateway refused to persist unencrypted OpenAI response history"
        );
        return;
    };
    if let Err(error) = runtime_state
        .kv_set(&record.storage_key, sealed_payload, Some(record.ttl))
        .await
    {
        warn!(
            event_name = "openai_response_history_write_failed",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            error = ?error,
            "gateway failed to persist shared OpenAI response history"
        );
    } else {
        commit_response_history_record(&record);
    }
}

pub(crate) async fn persist_converted_response_history(
    state: &AppState,
    report_context: &Value,
    response: Option<&Value>,
) {
    let Some(response) = response else {
        return;
    };
    if let Ok(Some(record)) = try_record_converted_response_history(report_context, response) {
        persist_response_history_record(state, record).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::{
        hydrate_openai_response_history, persist_response_history_record, ResponseHistoryRecord,
    };
    use crate::{ai_serving::response_history_storage_key, data::GatewayDataState, AppState};

    fn response_history_test_state() -> AppState {
        AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_runtime_state(Arc::new(RuntimeState::memory(
                MemoryRuntimeStateConfig::default(),
            )))
    }

    fn response_history_payload(response_id: &str, scope: &str, marker: &str) -> String {
        let expires_at_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(3600);
        json!({
            "version": 1,
            "response_id": response_id,
            "scope_fingerprint": format!("{:x}", Sha256::digest(scope.trim().as_bytes())),
            "expires_at_unix_secs": expires_at_unix_secs,
            "transcript": [{"type": "message", "content": marker}],
        })
        .to_string()
    }

    #[tokio::test]
    async fn response_history_is_encrypted_at_rest_and_hydrates() {
        let state = response_history_test_state();
        let response_id = "resp_gateway_encrypted_history_v1";
        let api_key_id = "response-history-encrypted-scope";
        let scope = crate::ai_serving::conversation_history_scope("tenant", api_key_id).unwrap();
        let marker = "private-response-history-marker";
        let storage_key = response_history_storage_key(response_id, Some(&scope));
        let payload = response_history_payload(response_id, &scope, marker);

        persist_response_history_record(
            &state,
            ResponseHistoryRecord::from_persisted(
                storage_key.clone(),
                payload,
                Duration::from_secs(6 * 60 * 60),
            ),
        )
        .await;

        let stored = state
            .runtime_kv_get(&storage_key)
            .await
            .expect("history lookup should succeed")
            .expect("history should be persisted");
        assert!(crate::handlers::shared::runtime_secret_payload_is_sealed(
            &stored
        ));
        assert!(!stored.contains(marker));

        hydrate_openai_response_history(
            &state,
            &json!({"previous_response_id": response_id}),
            "openai:responses",
            "openai:chat",
            "tenant",
            api_key_id,
            "provider",
            "endpoint",
            "key",
        )
        .await
        .expect("encrypted history should hydrate");
        assert!(crate::ai_serving::response_history_is_loaded(
            response_id,
            Some(&scope)
        ));
    }

    #[tokio::test]
    async fn response_history_reader_rejects_and_deletes_legacy_plaintext() {
        let state = response_history_test_state();
        let response_id = "resp_gateway_legacy_history_v1";
        let api_key_id = "response-history-legacy-scope";
        let scope = crate::ai_serving::conversation_history_scope("tenant", api_key_id).unwrap();
        let storage_key = response_history_storage_key(response_id, Some(&scope));
        let payload = response_history_payload(response_id, &scope, "legacy-private-history");
        state
            .runtime_kv_setex(&storage_key, &payload, 6 * 60 * 60)
            .await
            .expect("legacy history should store");

        let result = hydrate_openai_response_history(
            &state,
            &json!({"previous_response_id": response_id}),
            "openai:responses",
            "openai:chat",
            "tenant",
            api_key_id,
            "provider",
            "endpoint",
            "key",
        )
        .await;
        assert_eq!(
            result.unwrap(),
            Some("conversation_history_unmaterializable")
        );
        assert!(!crate::ai_serving::response_history_is_loaded(
            response_id,
            Some(&scope)
        ));
        assert!(state
            .runtime_kv_get(&storage_key)
            .await
            .expect("history lookup should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn native_continuation_allows_missing_local_history() {
        let state = response_history_test_state();
        let result = hydrate_openai_response_history(
            &state,
            &json!({"previous_response_id": "resp_native_provider_owned_only"}),
            "openai:responses",
            "openai:responses",
            "native-history-user",
            "native-history-key",
            "native-provider",
            "native-endpoint",
            "native-credential",
        )
        .await
        .expect("native continuation must not require local history");

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn native_continuation_allows_missing_local_scope() {
        let state = response_history_test_state();
        let result = hydrate_openai_response_history(
            &state,
            &json!({"previous_response_id": "resp_native_provider_owned_without_scope"}),
            "openai:responses",
            "openai:responses",
            "",
            "",
            "native-provider",
            "native-endpoint",
            "native-credential",
        )
        .await
        .expect("native continuation must not require a local history scope");

        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn native_continuation_rejects_persisted_binding_mismatch() {
        let state = response_history_test_state();
        let user_id = "native-binding-user";
        let api_key_id = "native-binding-api-key";
        let response_id = "resp_native_binding_mismatch";
        let record = crate::ai_serving::try_record_converted_response_history(
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "user_id": user_id,
                "api_key_id": api_key_id,
                "provider_id": "native-provider",
                "endpoint_id": "native-endpoint",
                "key_id": "native-credential-a",
                "original_request_body": {"input": "provider-owned turn"}
            }),
            &json!({
                "id": response_id,
                "status": "completed",
                "output": []
            }),
        )
        .expect("complete native response should prepare a history record")
        .expect("completed native response should produce history");

        persist_response_history_record(&state, record).await;

        let result = hydrate_openai_response_history(
            &state,
            &json!({"previous_response_id": response_id}),
            "openai:responses",
            "openai:responses",
            user_id,
            api_key_id,
            "native-provider",
            "native-endpoint",
            "native-credential-b",
        )
        .await
        .expect("native binding mismatch should be a candidate skip");

        assert_eq!(result, Some("conversation_history_binding_mismatch"));
    }
}
