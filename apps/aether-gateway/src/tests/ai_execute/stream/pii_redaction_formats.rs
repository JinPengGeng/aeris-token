use super::*;
use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::candidates::{
    RequestCandidateReadRepository, RequestCandidateStatus,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use sha2::{Digest, Sha256};

const ORIGINAL_EMAIL: &str = "alice@example.com";
const ORIGINAL_PHONE: &str = "13812345678";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SseRedactionFormat {
    OpenAiResponses,
    ClaudeMessages,
}

impl SseRedactionFormat {
    fn test_id(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "openai-responses-stream-pii-redaction",
            Self::ClaudeMessages => "claude-messages-stream-pii-redaction",
        }
    }

    fn trace_id(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "trace-openai-responses-stream-pii-redaction",
            Self::ClaudeMessages => "trace-claude-messages-stream-pii-redaction",
        }
    }

    fn api_format(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "openai:responses",
            Self::ClaudeMessages => "claude:messages",
        }
    }

    fn provider_name(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "openai",
            Self::ClaudeMessages => "claude",
        }
    }

    fn endpoint_kind(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "cli",
            Self::ClaudeMessages => "chat",
        }
    }

    fn client_path(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "/v1/responses",
            Self::ClaudeMessages => "/v1/messages",
        }
    }

    fn provider_route(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "/responses",
            Self::ClaudeMessages => "/messages",
        }
    }

    fn client_model(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "gpt-5",
            Self::ClaudeMessages => "claude-sonnet-4-5",
        }
    }

    fn provider_model(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "gpt-5-upstream",
            Self::ClaudeMessages => "claude-sonnet-4-5-upstream",
        }
    }

    fn uses_bearer_auth(self) -> bool {
        match self {
            Self::OpenAiResponses => true,
            Self::ClaudeMessages => false,
        }
    }

    fn client_request_body(self) -> serde_json::Value {
        match self {
            Self::OpenAiResponses => json!({
                "model": self.client_model(),
                "instructions": "Keep answers short.",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": format!("Email {ORIGINAL_EMAIL} phone {ORIGINAL_PHONE}")
                    }]
                }],
                "store": false,
                "stream": true
            }),
            Self::ClaudeMessages => json!({
                "model": self.client_model(),
                "system": "Keep answers short.",
                "messages": [{
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": format!("Email {ORIGINAL_EMAIL} phone {ORIGINAL_PHONE}")
                    }]
                }],
                "max_tokens": 64,
                "stream": true
            }),
        }
    }

    fn delta_event(self, sequence: usize, text: &str) -> String {
        match self {
            Self::OpenAiResponses => {
                let data = serde_json::to_string(&json!({
                    "type": "response.output_text.delta",
                    "sequence_number": sequence,
                    "item_id": "msg_pii_stream",
                    "output_index": 0,
                    "content_index": 0,
                    "delta": text,
                }))
                .expect("delta payload should serialize");
                format!("event: response.output_text.delta\ndata: {data}\n\n")
            }
            Self::ClaudeMessages => {
                let data = serde_json::to_string(&json!({
                    "type": "content_block_delta",
                    "index": sequence - 1,
                    "delta": {"type": "text_delta", "text": text},
                }))
                .expect("delta payload should serialize");
                format!("event: content_block_delta\ndata: {data}\n\n")
            }
        }
    }

    fn leading_event(self) -> String {
        match self {
            Self::OpenAiResponses => {
                let data = serde_json::to_string(&json!({
                    "type": "response.created",
                    "response": {
                        "id": "resp_pii_stream",
                        "object": "response",
                        "status": "in_progress",
                        "model": self.provider_model(),
                        "output": []
                    },
                }))
                .expect("response.created payload should serialize");
                format!("event: response.created\ndata: {data}\n\n")
            }
            Self::ClaudeMessages => {
                let data = serde_json::to_string(&json!({
                    "type": "message_start",
                    "message": {
                        "id": "msg_pii_stream",
                        "type": "message",
                        "role": "assistant",
                        "model": self.provider_model(),
                        "content": [],
                        "stop_reason": null,
                        "usage": {"input_tokens": 1, "output_tokens": 0}
                    },
                }))
                .expect("message_start payload should serialize");
                format!("event: message_start\ndata: {data}\n\n")
            }
        }
    }

    fn trailing_events(self) -> String {
        match self {
            Self::OpenAiResponses => {
                let data = serde_json::to_string(&json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_pii_stream",
                        "object": "response",
                        "status": "completed",
                        "model": self.provider_model(),
                        "output": [{
                            "type": "message",
                            "id": "msg_pii_stream",
                            "role": "assistant",
                            "status": "completed",
                            "content": [{
                                "type": "output_text",
                                "text": "done",
                                "annotations": []
                            }]
                        }],
                        "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3}
                    },
                }))
                .expect("response.completed payload should serialize");
                format!("event: response.completed\ndata: {data}\n\n")
            }
            Self::ClaudeMessages => {
                let message_delta = serde_json::to_string(&json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                    "usage": {"output_tokens": 3}
                }))
                .expect("message_delta payload should serialize");
                let message_stop = serde_json::to_string(&json!({"type": "message_stop"}))
                    .expect("message_stop payload should serialize");
                format!(
                    "event: message_delta\ndata: {message_delta}\n\n\
                     event: message_stop\ndata: {message_stop}\n\n"
                )
            }
        }
    }
}

#[derive(Debug, Clone)]
struct SeenProviderStreamRequest {
    body: serde_json::Value,
    accept: String,
    accept_encoding: String,
    authorization: String,
    x_api_key: String,
}

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn client_api_key(format: SseRedactionFormat) -> String {
    format!("sk-client-{}", format.test_id())
}

fn upstream_api_key(format: SseRedactionFormat) -> String {
    format!("sk-upstream-{}", format.test_id())
}

fn collect_sentinels(text: &str, kind: &str) -> Vec<String> {
    let prefix = format!("<AETHER:{kind}:");
    let mut sentinels = Vec::new();
    let mut offset = 0;
    while let Some(relative_start) = text[offset..].find(&prefix) {
        let start = offset + relative_start;
        let Some(relative_end) = text[start..].find('>') else {
            break;
        };
        let end = start + relative_end + 1;
        sentinels.push(text[start..end].to_string());
        offset = end;
    }
    sentinels
}

/// Builds the provider SSE chunk sequence.
///
/// The construction reuses the cross-chunk sentinel splitting technique from
/// `stream/pii_redaction.rs` and extends it with two further split shapes:
/// - the phone sentinel is split inside the delta JSON string mid data line,
///   and the blank line that closes that event is emitted as its own chunk, so
///   the event-boundary flush only happens once the next chunk arrives;
/// - the email sentinel is split inside the delta JSON string of the following
///   event, and a multibyte CJK character right after it is split mid-sequence
///   across two chunks.
fn provider_stream_chunks(
    format: SseRedactionFormat,
    phone_sentinel: &str,
    email_sentinel: &str,
) -> Vec<Vec<u8>> {
    let phone_event = format.delta_event(1, &format!("回复 {phone_sentinel} 已收到"));
    let email_event = format.delta_event(2, &format!("邮箱 {email_sentinel} 好文完"));

    let phone_bytes = phone_event.as_bytes();
    let phone_sentinel_start = phone_event
        .find(phone_sentinel)
        .expect("phone sentinel should appear in the delta event");
    let phone_split_at = phone_sentinel_start + phone_sentinel.len() / 2;

    let email_bytes = email_event.as_bytes();
    let email_sentinel_start = email_event
        .find(email_sentinel)
        .expect("email sentinel should appear in the delta event");
    let email_split_at = email_sentinel_start + email_sentinel.len() / 2;
    let multibyte_char_index = email_event
        .find("好文完")
        .expect("multibyte tail should appear in the delta event")
        + "好".len();

    vec![
        format.leading_event().into_bytes(),
        phone_bytes[..phone_split_at].to_vec(),
        // Ends with the data line's "\n"; the blank line that terminates the
        // SSE event is delivered by the next chunk.
        phone_bytes[phone_split_at..phone_bytes.len() - 1].to_vec(),
        b"\n".to_vec(),
        email_bytes[..email_split_at].to_vec(),
        email_bytes[email_split_at..multibyte_char_index + 2].to_vec(),
        email_bytes[multibyte_char_index + 2..].to_vec(),
        format.trailing_events().into_bytes(),
    ]
}

const STREAM_PII_REDACTION_FORMAT_TEST_STACK_BYTES: usize = 16 * 1024 * 1024;

fn run_stream_pii_redaction_format_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(STREAM_PII_REDACTION_FORMAT_TEST_STACK_BYTES)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("stream pii redaction format test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

#[test]
fn ai_execute_stream_openai_responses_pii_redaction_round_trip() {
    run_stream_pii_redaction_format_test(
        "ai_execute_stream_openai_responses_pii_redaction_round_trip",
        || run_stream_pii_redaction_format_case(SseRedactionFormat::OpenAiResponses),
    );
}

#[test]
fn ai_execute_stream_claude_messages_pii_redaction_round_trip() {
    run_stream_pii_redaction_format_test(
        "ai_execute_stream_claude_messages_pii_redaction_round_trip",
        || run_stream_pii_redaction_format_case(SseRedactionFormat::ClaudeMessages),
    );
}

async fn run_stream_pii_redaction_format_case(format: SseRedactionFormat) {
    let seen_provider_request = Arc::new(Mutex::new(None::<SeenProviderStreamRequest>));
    let seen_provider_request_clone = Arc::clone(&seen_provider_request);
    let provider_app = Router::new().route(
        format.provider_route(),
        any(move |request: Request| {
            let seen_provider_request_inner = Arc::clone(&seen_provider_request_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("provider payload should parse");
                let payload_text =
                    serde_json::to_string(&payload).expect("payload should serialize");
                let email_sentinel = collect_sentinels(&payload_text, "EMAIL")
                    .into_iter()
                    .next()
                    .expect("email sentinel should exist in provider payload");
                let phone_sentinel = collect_sentinels(&payload_text, "CN_PHONE")
                    .into_iter()
                    .next()
                    .expect("phone sentinel should exist in provider payload");
                *seen_provider_request_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenProviderStreamRequest {
                    body: payload,
                    accept: parts
                        .headers
                        .get(http::header::ACCEPT)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    accept_encoding: parts
                        .headers
                        .get(http::header::ACCEPT_ENCODING)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    authorization: parts
                        .headers
                        .get(http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    x_api_key: parts
                        .headers
                        .get("x-api-key")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                });

                let stream = futures_util::stream::iter(
                    provider_stream_chunks(format, &phone_sentinel, &email_sentinel)
                        .into_iter()
                        .map(|chunk| Ok::<_, Infallible>(Bytes::from(chunk))),
                );
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from_stream(stream))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                response
            }
        }),
    );
    let (provider_url, provider_handle) = start_server(provider_app).await;
    let auth_repository = auth_repository(format);
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            candidate_row(format),
        ]));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider(format)],
        vec![endpoint(format, provider_url)],
        vec![key(format)],
    ));
    let data_state = crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
        auth_repository,
        candidate_selection_repository,
        provider_catalog_repository,
        Arc::clone(&request_candidate_repository),
        DEVELOPMENT_ENCRYPTION_KEY,
    )
    .with_system_config_values_for_tests(redaction_config());
    let gateway_state = AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(data_state);
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let mut request = client
        .post(format!("{gateway_url}{}", format.client_path()))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::ACCEPT_ENCODING, "gzip")
        .header(TRACE_ID_HEADER, format.trace_id())
        .body(format.client_request_body().to_string());
    request = if format.uses_bearer_auth() {
        request.header(
            http::header::AUTHORIZATION,
            format!("Bearer {}", client_api_key(format)),
        )
    } else {
        request
            .header("x-api-key", client_api_key(format))
            .header("anthropic-version", "2023-06-01")
    };
    let response = request.send().await.expect("request should succeed");

    let status = response.status();
    let execution_path = response
        .headers()
        .get(EXECUTION_PATH_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let response_text = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::OK, "{response_text}");
    assert_eq!(
        execution_path.as_deref(),
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_STREAM)
    );
    assert_eq!(
        content_type.as_deref(),
        Some("text/event-stream"),
        "the SSE restorer keys off the response content type"
    );
    assert!(
        response_text.contains(&format!("回复 {ORIGINAL_PHONE} 已收到")),
        "phone sentinel should be restored in the streamed delta: {response_text}"
    );
    assert!(
        response_text.contains(&format!("邮箱 {ORIGINAL_EMAIL} 好文完")),
        "email sentinel and the multibyte tail should be restored in the streamed delta: {response_text}"
    );
    assert!(!response_text.contains("<AETHER:"));
    match format {
        SseRedactionFormat::OpenAiResponses => {
            assert!(response_text.contains("event: response.completed"));
        }
        SseRedactionFormat::ClaudeMessages => {
            assert!(response_text.contains("event: message_stop"));
        }
    }

    let seen = seen_provider_request
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("provider stream request should be captured");
    assert_eq!(seen.accept, "text/event-stream");
    assert_eq!(seen.accept_encoding, "identity");
    if format.uses_bearer_auth() {
        assert_eq!(
            seen.authorization,
            format!("Bearer {}", upstream_api_key(format))
        );
    } else {
        assert_eq!(seen.x_api_key, upstream_api_key(format));
    }
    let provider_body_text = serde_json::to_string(&seen.body).expect("body should serialize");
    assert!(
        !provider_body_text.contains(ORIGINAL_EMAIL),
        "provider payload leaked original email: {provider_body_text}"
    );
    assert!(
        !provider_body_text.contains(ORIGINAL_PHONE),
        "provider payload leaked original phone: {provider_body_text}"
    );
    assert!(provider_body_text.contains("<AETHER:EMAIL:"));
    assert!(provider_body_text.contains("<AETHER:CN_PHONE:"));

    let stored_candidates = request_candidate_repository
        .list_by_request_id(format.trace_id())
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    gateway_handle.abort();
    provider_handle.abort();
}

fn auth_repository(format: SseRedactionFormat) -> Arc<InMemoryAuthApiKeySnapshotRepository> {
    let snapshot = auth_snapshot(format);
    let key_hash = hash_api_key(&client_api_key(format));
    Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some(key_hash.clone()),
            snapshot.clone(),
        )])
        .with_export_records(vec![auth_export_record(
            &snapshot,
            key_hash,
            Some(json!({
                "chat_pii_redaction": {
                    "enabled": true,
                }
            })),
        )]),
    )
}

fn auth_snapshot(format: SseRedactionFormat) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        format!("user-{}", format.test_id()),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(json!([format.provider_name()])),
        Some(json!([format.api_format()])),
        Some(json!([format.client_model()])),
        format!("api-key-{}", format.test_id()),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(4_102_444_800),
        Some(json!([format.provider_name()])),
        Some(json!([format.api_format()])),
        Some(json!([format.client_model()])),
    )
    .expect("auth snapshot should build")
}

fn auth_export_record(
    snapshot: &StoredAuthApiKeySnapshot,
    key_hash: String,
    feature_settings: Option<serde_json::Value>,
) -> StoredAuthApiKeyExportRecord {
    aether_data::repository::auth::StoredAuthApiKeyExportRecord::new(
        snapshot.user_id.clone(),
        snapshot.api_key_id.clone(),
        key_hash,
        None,
        snapshot.api_key_name.clone(),
        snapshot
            .api_key_allowed_providers
            .as_ref()
            .map(|value| serde_json::json!(value)),
        snapshot
            .api_key_allowed_api_formats
            .as_ref()
            .map(|value| serde_json::json!(value)),
        snapshot
            .api_key_allowed_models
            .as_ref()
            .map(|value| serde_json::json!(value)),
        snapshot.api_key_rate_limit,
        snapshot.api_key_concurrent_limit,
        None,
        snapshot.api_key_is_active,
        snapshot
            .api_key_expires_at_unix_secs
            .map(|value| value as i64),
        false,
        0,
        0,
        0.0,
        snapshot.api_key_is_standalone,
    )
    .expect("auth api key export record should build")
    .with_feature_settings(feature_settings)
}

fn candidate_row(format: SseRedactionFormat) -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: format!("provider-{}", format.test_id()),
        provider_name: format.provider_name().to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: format!("endpoint-{}", format.test_id()),
        endpoint_api_format: format.api_format().to_string(),
        endpoint_api_family: Some(format.provider_name().to_string()),
        endpoint_kind: Some(format.endpoint_kind().to_string()),
        endpoint_is_active: true,
        key_id: format!("key-{}", format.test_id()),
        key_name: "prod".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec![format.api_format().to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 5,
        key_global_priority_by_format: Some(json!({format.api_format(): 1})),
        model_id: format!("model-{}", format.test_id()),
        global_model_id: format!("global-model-{}", format.test_id()),
        global_model_name: format.client_model().to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(true),
        model_provider_model_name: format.provider_model().to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: format.provider_model().to_string(),
            priority: 1,
            api_formats: Some(vec![format.api_format().to_string()]),
            endpoint_ids: Some(vec![format!("endpoint-{}", format.test_id())]),
            operations: None,
        }]),
        model_supports_streaming: Some(true),
        model_is_active: true,
        model_is_available: true,
    }
}

fn provider(format: SseRedactionFormat) -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        format!("provider-{}", format.test_id()),
        format.provider_name().to_string(),
        Some("https://example.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
    .with_transport_fields(
        true,
        false,
        false,
        None,
        Some(2),
        None,
        Some(20.0),
        None,
        Some(serde_json::json!({"chat_pii_redaction": {"enabled": true}})),
    )
}

fn endpoint(format: SseRedactionFormat, base_url: String) -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        format!("endpoint-{}", format.test_id()),
        format!("provider-{}", format.test_id()),
        format.api_format().to_string(),
        Some(format.provider_name().to_string()),
        Some(format.endpoint_kind().to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(base_url, None, None, Some(2), None, None, None, None)
    .expect("endpoint transport should build")
}

fn key(format: SseRedactionFormat) -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        format!("key-{}", format.test_id()),
        format!("provider-{}", format.test_id()),
        "prod".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(serde_json::json!([format.api_format()])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, &upstream_api_key(format))
            .expect("api key should encrypt"),
        None,
        None,
        Some(serde_json::json!({format.api_format(): 1})),
        None,
        None,
        None,
        None,
    )
    .expect("key transport should build")
}

fn redaction_config() -> Vec<(String, serde_json::Value)> {
    vec![
        ("module.chat_pii_redaction.enabled".to_string(), json!(true)),
        (
            "module.chat_pii_redaction.rules".to_string(),
            json!([
                {
                    "id": "email",
                    "name": "邮箱",
                    "pattern": r"(?i)[A-Z0-9._%+-]{1,64}@[A-Z0-9.-]{1,253}\.[A-Z]{2,63}",
                    "enabled": true,
                    "features": {"validator": "email"},
                    "system": true
                },
                {
                    "id": "cn_phone",
                    "name": "手机号",
                    "pattern": r"(?:\+?86[- ]?)?(?:1[3-9]\d[- ]?\d{4}[- ]?\d{4}|0\d{2,3}[- ]\d{7,8}(?:-\d{1,6})?)",
                    "enabled": true,
                    "features": {"validator": "cn_phone"},
                    "system": true
                }
            ]),
        ),
        (
            "module.chat_pii_redaction.cache_ttl_seconds".to_string(),
            json!(300),
        ),
    ]
}
