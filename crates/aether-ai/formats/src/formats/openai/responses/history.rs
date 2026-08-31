use std::{
    collections::{HashMap, VecDeque},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::formats::{context::FormatContext, registry};

const HISTORY_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_HISTORY_ENTRIES: usize = 2_048;
const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_HISTORY_ENTRY_BYTES: usize = 8 * 1024 * 1024;
const HISTORY_STORAGE_VERSION: u8 = 1;
const HISTORY_STORAGE_KEY_PREFIX: &str = "ai:responses:history:v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationHistoryCapability {
    /// Preserve the provider-owned continuation handle without local history.
    Native,
    /// Expand stored Responses transcript into OpenAI Chat input.
    Hydrate,
    /// Expand stored Responses transcript before emitting another wire format.
    Translate,
    /// Reject the continuation because its semantics cannot be preserved.
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConversationHistoryResolution<'a> {
    pub capability: ConversationHistoryCapability,
    pub previous_response_id: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversationHistoryResolutionError {
    InvalidPreviousResponseId,
    Unsupported {
        client_api_format: String,
        provider_api_format: String,
    },
}

impl std::fmt::Display for ConversationHistoryResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPreviousResponseId => formatter
                .write_str("previous_response_id must be a non-empty string when it is supplied"),
            Self::Unsupported {
                client_api_format,
                provider_api_format,
            } => write!(
                formatter,
                "conversation history cannot continue from {client_api_format} to {provider_api_format}"
            ),
        }
    }
}

pub struct ConversationHistoryResolver;

impl ConversationHistoryResolver {
    pub fn capability(
        client_api_format: &str,
        provider_api_format: &str,
    ) -> ConversationHistoryCapability {
        let client_api_format = crate::normalize_api_format_alias(client_api_format);
        let provider_api_format = crate::normalize_api_format_alias(provider_api_format);
        if client_api_format != "openai:responses" {
            return ConversationHistoryCapability::Unsupported;
        }
        match provider_api_format.as_str() {
            "openai:responses" => ConversationHistoryCapability::Native,
            "openai:chat" => ConversationHistoryCapability::Hydrate,
            "claude:messages" | "gemini:generate_content" => {
                ConversationHistoryCapability::Translate
            }
            _ => ConversationHistoryCapability::Unsupported,
        }
    }

    pub fn resolve<'a>(
        request: &'a Value,
        client_api_format: &str,
        provider_api_format: &str,
    ) -> Result<Option<ConversationHistoryResolution<'a>>, ConversationHistoryResolutionError> {
        let Some(previous_response_id) = request.get("previous_response_id") else {
            return Ok(None);
        };
        let previous_response_id = previous_response_id
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(ConversationHistoryResolutionError::InvalidPreviousResponseId)?;
        let capability = Self::capability(client_api_format, provider_api_format);
        if capability == ConversationHistoryCapability::Unsupported {
            return Err(ConversationHistoryResolutionError::Unsupported {
                client_api_format: crate::normalize_api_format_alias(client_api_format),
                provider_api_format: crate::normalize_api_format_alias(provider_api_format),
            });
        }
        Ok(Some(ConversationHistoryResolution {
            capability,
            previous_response_id,
        }))
    }
}

pub fn conversation_history_scope(user_id: &str, api_key_id: &str) -> Option<String> {
    let user_id = user_id.trim();
    let api_key_id = api_key_id.trim();
    if user_id.is_empty() || api_key_id.is_empty() {
        return None;
    }
    Some(format!(
        "tenant:{}:{user_id}:api-key:{}:{api_key_id}",
        user_id.len(),
        api_key_id.len()
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseHistoryRecord {
    pub storage_key: String,
    pub payload: String,
    pub ttl: Duration,
    cache_response_id: String,
    cache_history_scope: Option<String>,
    cache_transcript: Option<Vec<Value>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeResponseHistoryValidation {
    Match,
    CandidateMismatch,
}

#[derive(Serialize, Deserialize)]
struct PersistedResponseHistory {
    version: u8,
    response_id: String,
    scope_fingerprint: String,
    expires_at_unix_secs: u64,
    transcript: Vec<Value>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    transcript_materialized: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_binding: Option<NativeResponseHistoryBinding>,
}

const fn default_true() -> bool {
    true
}

const fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct NativeResponseHistoryBinding {
    provider_api_format: String,
    provider_id: String,
    endpoint_id: String,
    key_id: String,
}

#[derive(Clone)]
struct ResponseHistoryEntry {
    transcript: Vec<Value>,
    inserted_at: Instant,
    expires_at: Instant,
    size_bytes: usize,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct ResponseHistoryKey {
    scope: Option<String>,
    response_id: String,
}

#[derive(Default)]
struct ResponseHistoryStore {
    entries: HashMap<ResponseHistoryKey, ResponseHistoryEntry>,
    insertion_order: VecDeque<(ResponseHistoryKey, Instant)>,
    total_bytes: usize,
}

impl ResponseHistoryStore {
    fn remove(&mut self, key: &ResponseHistoryKey) {
        if let Some(entry) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.size_bytes);
        }
    }

    fn prune(&mut self, now: Instant) {
        let expired_keys = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.expires_at <= now)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in expired_keys {
            self.remove(&key);
        }

        while self.entries.len() > MAX_HISTORY_ENTRIES || self.total_bytes > MAX_HISTORY_BYTES {
            let Some((key, inserted_at)) = self.insertion_order.pop_front() else {
                break;
            };
            if self
                .entries
                .get(&key)
                .is_some_and(|entry| entry.inserted_at == inserted_at)
            {
                self.remove(&key);
            }
        }

        while let Some((key, inserted_at)) = self.insertion_order.front() {
            if self
                .entries
                .get(key)
                .is_some_and(|entry| entry.inserted_at == *inserted_at)
            {
                break;
            }
            self.insertion_order.pop_front();
        }
    }

    fn get(
        &mut self,
        response_id: &str,
        history_scope: Option<&str>,
        now: Instant,
    ) -> Option<Vec<Value>> {
        self.prune(now);
        self.entries
            .get(&response_history_key(response_id, history_scope))
            .map(|entry| entry.transcript.clone())
    }

    fn insert(
        &mut self,
        response_id: String,
        history_scope: Option<&str>,
        transcript: Vec<Value>,
        now: Instant,
        ttl: Duration,
    ) {
        if ttl.is_zero() {
            return;
        }
        let size_bytes = serde_json::to_vec(&transcript)
            .map(|bytes| bytes.len())
            .unwrap_or(MAX_HISTORY_ENTRY_BYTES.saturating_add(1));
        if size_bytes > MAX_HISTORY_ENTRY_BYTES {
            return;
        }
        let key = response_history_key(&response_id, history_scope);
        self.remove(&key);
        self.total_bytes = self.total_bytes.saturating_add(size_bytes);
        self.entries.insert(
            key.clone(),
            ResponseHistoryEntry {
                transcript,
                inserted_at: now,
                expires_at: now.checked_add(ttl).unwrap_or(now),
                size_bytes,
            },
        );
        self.insertion_order.push_back((key, now));
        self.prune(now);
    }
}

fn response_history_key(response_id: &str, history_scope: Option<&str>) -> ResponseHistoryKey {
    ResponseHistoryKey {
        scope: history_scope
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(ToOwned::to_owned),
        response_id: response_id.to_string(),
    }
}

fn normalized_history_scope(history_scope: Option<&str>) -> Option<&str> {
    history_scope
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
}

fn sha256_hex(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn history_scope_fingerprint(history_scope: Option<&str>) -> String {
    sha256_hex(
        normalized_history_scope(history_scope)
            .unwrap_or("<unscoped>")
            .as_bytes(),
    )
}

pub fn response_history_storage_key(response_id: &str, history_scope: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(history_scope_fingerprint(history_scope));
    hasher.update([0]);
    hasher.update(response_id.trim().as_bytes());
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{HISTORY_STORAGE_KEY_PREFIX}:{digest}")
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn response_history_store() -> &'static Mutex<ResponseHistoryStore> {
    static STORE: OnceLock<Mutex<ResponseHistoryStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(ResponseHistoryStore::default()))
}

pub fn response_history_is_loaded(response_id: &str, history_scope: Option<&str>) -> bool {
    if normalized_history_scope(history_scope).is_none() {
        return false;
    }
    response_history_store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(response_id, history_scope, Instant::now())
        .is_some()
}

pub fn evict_response_history(response_id: &str, history_scope: Option<&str>) {
    response_history_store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&response_history_key(response_id.trim(), history_scope));
}

pub fn hydrate_response_history(
    response_id: &str,
    history_scope: Option<&str>,
    payload: &str,
) -> Result<(), String> {
    if normalized_history_scope(history_scope).is_none() {
        return Err("response history requires a tenant and API-key scope".to_string());
    }
    if payload.len() > MAX_HISTORY_ENTRY_BYTES {
        return Err("persisted response history exceeds the maximum entry size".to_string());
    }
    let persisted: PersistedResponseHistory = serde_json::from_str(payload)
        .map_err(|error| format!("invalid persisted response history: {error}"))?;
    if persisted.version != HISTORY_STORAGE_VERSION {
        return Err(format!(
            "unsupported response history version {}",
            persisted.version
        ));
    }
    if persisted.response_id != response_id.trim() {
        return Err(
            "persisted response history id does not match the requested response".to_string(),
        );
    }
    if persisted.scope_fingerprint != history_scope_fingerprint(history_scope) {
        return Err("persisted response history scope does not match the requester".to_string());
    }
    if !persisted.transcript_materialized {
        return Err(
            "persisted response history is provider-resolved and cannot be materialized"
                .to_string(),
        );
    }
    let now_unix_secs = current_unix_secs();
    let remaining_ttl = persisted
        .expires_at_unix_secs
        .checked_sub(now_unix_secs)
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .ok_or_else(|| "persisted response history has expired".to_string())?;
    response_history_store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            response_id.trim().to_string(),
            history_scope,
            persisted.transcript,
            Instant::now(),
            remaining_ttl.min(HISTORY_TTL),
        );
    Ok(())
}

pub fn hydrate_native_response_history_if_materialized(
    response_id: &str,
    history_scope: &str,
    payload: &str,
) -> Result<bool, String> {
    if normalized_history_scope(Some(history_scope)).is_none() {
        return Err("response history requires a tenant and API-key scope".to_string());
    }
    if payload.len() > MAX_HISTORY_ENTRY_BYTES {
        return Err("persisted response history exceeds the maximum entry size".to_string());
    }
    let persisted: PersistedResponseHistory = serde_json::from_str(payload)
        .map_err(|error| format!("invalid persisted response history: {error}"))?;
    if persisted.version != HISTORY_STORAGE_VERSION {
        return Err(format!(
            "unsupported response history version {}",
            persisted.version
        ));
    }
    if persisted.response_id != response_id.trim() {
        return Err(
            "persisted response history id does not match the requested response".to_string(),
        );
    }
    if persisted.scope_fingerprint != history_scope_fingerprint(Some(history_scope)) {
        return Err("persisted response history scope does not match the requester".to_string());
    }
    if persisted.expires_at_unix_secs <= current_unix_secs() {
        return Err("persisted response history has expired".to_string());
    }
    if !persisted.transcript_materialized {
        return Ok(false);
    }
    hydrate_response_history(response_id, Some(history_scope), payload)?;
    Ok(true)
}

fn request_input_items(request: &Value) -> Vec<Value> {
    match request.get("input") {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::String(text)) if !text.is_empty() => vec![json!({
            "type": "message",
            "role": "user",
            "content": text,
        })],
        _ if request.get("messages").and_then(Value::as_array).is_some() => {
            registry::convert_request(
                "openai:chat",
                "openai:responses",
                request,
                &FormatContext::default(),
            )
            .ok()
            .and_then(|converted| converted.get("input").and_then(Value::as_array).cloned())
            .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

fn history_recording_capability(report_context: &Value) -> Option<ConversationHistoryCapability> {
    let client_api_format = report_context.get("client_api_format")?.as_str()?;
    let provider_api_format = report_context.get("provider_api_format")?.as_str()?;
    let capability =
        ConversationHistoryResolver::capability(client_api_format, provider_api_format);
    (capability != ConversationHistoryCapability::Unsupported).then_some(capability)
}

fn history_scope_from_report_context(report_context: &Value) -> Option<String> {
    let user_id = report_context
        .get("user_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let api_key_id = report_context
        .get("api_key_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    conversation_history_scope(user_id?, api_key_id?)
}

fn native_binding_from_report_context(
    report_context: &Value,
) -> Option<NativeResponseHistoryBinding> {
    let field = |name| {
        report_context
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    Some(NativeResponseHistoryBinding {
        provider_api_format: crate::normalize_api_format_alias(&field("provider_api_format")?),
        provider_id: field("provider_id")?,
        endpoint_id: field("endpoint_id")?,
        key_id: field("key_id")?,
    })
}

pub fn validate_native_response_history(
    response_id: &str,
    history_scope: &str,
    provider_api_format: &str,
    provider_id: &str,
    endpoint_id: &str,
    key_id: &str,
    payload: &str,
) -> Result<NativeResponseHistoryValidation, String> {
    if payload.len() > MAX_HISTORY_ENTRY_BYTES {
        return Err("persisted response history exceeds the maximum entry size".to_string());
    }
    let persisted: PersistedResponseHistory = serde_json::from_str(payload)
        .map_err(|error| format!("invalid persisted response history: {error}"))?;
    if persisted.version != HISTORY_STORAGE_VERSION
        || persisted.response_id != response_id.trim()
        || persisted.scope_fingerprint != history_scope_fingerprint(Some(history_scope))
    {
        return Err("persisted native response history ownership does not match".to_string());
    }
    let expected = NativeResponseHistoryBinding {
        provider_api_format: crate::normalize_api_format_alias(provider_api_format),
        provider_id: provider_id.trim().to_string(),
        endpoint_id: endpoint_id.trim().to_string(),
        key_id: key_id.trim().to_string(),
    };
    if expected.provider_id.is_empty()
        || expected.endpoint_id.is_empty()
        || expected.key_id.is_empty()
    {
        return Err("native response history candidate identity is incomplete".to_string());
    }
    if persisted.native_binding.as_ref() != Some(&expected) {
        return Ok(NativeResponseHistoryValidation::CandidateMismatch);
    }
    if persisted.expires_at_unix_secs <= current_unix_secs() {
        return Err("persisted response history has expired".to_string());
    }
    Ok(NativeResponseHistoryValidation::Match)
}

pub fn previous_response_id_fingerprint(previous_response_id: &str) -> String {
    sha256_hex(previous_response_id.trim().as_bytes())[..16].to_string()
}

pub(crate) fn expand_previous_response_for_conversion(
    request: &Value,
    history_scope: Option<&str>,
) -> Result<Value, String> {
    let history_scope = normalized_history_scope(history_scope)
        .ok_or_else(|| "response history requires a tenant and API-key scope".to_string())?;
    let Some(raw_previous_response_id) = request.get("previous_response_id") else {
        return Ok(request.clone());
    };
    let previous_response_id = raw_previous_response_id
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "previous_response_id must be a non-empty string when it is supplied".to_string()
        })?;
    let mut store = response_history_store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(mut transcript) = store.get(previous_response_id, Some(history_scope), Instant::now())
    else {
        return Err(format!(
            "response history not found for previous_response_id fingerprint {}",
            previous_response_id_fingerprint(previous_response_id)
        ));
    };
    transcript.extend(request_input_items(request));
    let mut expanded = request.clone();
    let Some(object) = expanded.as_object_mut() else {
        return Err("OpenAI Responses request must be a JSON object".to_string());
    };
    object.remove("previous_response_id");
    object.insert("input".to_string(), Value::Array(transcript));
    Ok(expanded)
}

pub fn record_converted_response_history(
    report_context: &Value,
    response: &Value,
) -> Option<ResponseHistoryRecord> {
    try_record_converted_response_history(report_context, response)
        .ok()
        .flatten()
}

pub fn try_record_converted_response_history(
    report_context: &Value,
    response: &Value,
) -> Result<Option<ResponseHistoryRecord>, String> {
    let Some(capability) = history_recording_capability(report_context) else {
        return Ok(None);
    };
    if response.get("status").and_then(Value::as_str) != Some("completed") {
        return Ok(None);
    }
    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "completed response history is missing a response id".to_string())?;
    let request = report_context
        .get("original_request_body")
        .ok_or_else(|| "completed response history is missing the original request".to_string())?;
    if !request.is_object() {
        return Err(
            "completed response history original request must be a JSON object".to_string(),
        );
    }
    let history_scope = history_scope_from_report_context(report_context).ok_or_else(|| {
        "completed response history requires a tenant and API-key scope".to_string()
    })?;
    let native_binding = if capability == ConversationHistoryCapability::Native {
        Some(
            native_binding_from_report_context(report_context).ok_or_else(|| {
                "completed native response history requires a provider binding".to_string()
            })?,
        )
    } else {
        None
    };
    let (mut transcript, transcript_materialized) =
        if request.get("messages").and_then(Value::as_array).is_some() {
            (Vec::new(), true)
        } else {
            match request
                .get("previous_response_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(previous_response_id) => {
                    let parent = response_history_store()
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .get(
                            previous_response_id,
                            Some(history_scope.as_str()),
                            Instant::now(),
                        );
                    match parent {
                        Some(parent) => (parent, true),
                        None if capability == ConversationHistoryCapability::Native => {
                            (Vec::new(), false)
                        }
                        None => return Err(
                            "completed response history cannot inherit a missing parent transcript"
                                .to_string(),
                        ),
                    }
                }
                None => (Vec::new(), true),
            }
        };
    transcript.extend(request_input_items(request));
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| "completed response history is missing a valid output array".to_string())?;
    transcript.extend(output.iter().cloned());
    let expires_at_unix_secs = current_unix_secs().saturating_add(HISTORY_TTL.as_secs());
    let payload = serde_json::to_string(&PersistedResponseHistory {
        version: HISTORY_STORAGE_VERSION,
        response_id: response_id.to_string(),
        scope_fingerprint: history_scope_fingerprint(Some(history_scope.as_str())),
        expires_at_unix_secs,
        transcript: transcript.clone(),
        transcript_materialized,
        native_binding,
    })
    .map_err(|error| format!("completed response history serialization failed: {error}"))?;
    if payload.len() > MAX_HISTORY_ENTRY_BYTES {
        return Err("completed response history exceeds the maximum entry size".to_string());
    }
    Ok(Some(ResponseHistoryRecord {
        storage_key: response_history_storage_key(response_id, Some(history_scope.as_str())),
        payload,
        ttl: HISTORY_TTL,
        cache_response_id: response_id.to_string(),
        cache_history_scope: Some(history_scope),
        cache_transcript: transcript_materialized.then_some(transcript),
    }))
}

pub fn commit_response_history_record(record: &ResponseHistoryRecord) {
    let Some(transcript) = record.cache_transcript.as_ref() else {
        return;
    };
    response_history_store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            record.cache_response_id.clone(),
            record.cache_history_scope.as_deref(),
            transcript.clone(),
            Instant::now(),
            record.ttl,
        );
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::formats::{context::FormatContext, registry::convert_request};

    use super::{
        commit_response_history_record, conversation_history_scope,
        expand_previous_response_for_conversion, hydrate_native_response_history_if_materialized,
        hydrate_response_history,
        record_converted_response_history as build_response_history_record, response_history_key,
        response_history_storage_key, response_history_store, validate_native_response_history,
        ConversationHistoryCapability, ConversationHistoryResolver,
        NativeResponseHistoryValidation, ResponseHistoryRecord, MAX_HISTORY_ENTRY_BYTES,
    };

    fn record_converted_response_history(
        report_context: &Value,
        response: &Value,
    ) -> Option<ResponseHistoryRecord> {
        let record = build_response_history_record(report_context, response)?;
        commit_response_history_record(&record);
        Some(record)
    }

    fn conversion_report_context(original_request_body: serde_json::Value) -> serde_json::Value {
        json!({
            "needs_conversion": true,
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:chat",
            "user_id": "conversion-test-user",
            "api_key_id": "conversion-test-key",
            "original_request_body": original_request_body,
        })
    }

    #[test]
    fn resolver_declares_each_history_capability_and_rejects_lossy_pairs() {
        assert_eq!(
            ConversationHistoryResolver::capability("openai:responses", "openai:responses"),
            ConversationHistoryCapability::Native
        );
        assert_eq!(
            ConversationHistoryResolver::capability("openai:responses", "openai:chat"),
            ConversationHistoryCapability::Hydrate
        );
        for provider in ["claude:messages", "gemini:generate_content"] {
            assert_eq!(
                ConversationHistoryResolver::capability("openai:responses", provider),
                ConversationHistoryCapability::Translate
            );
        }
        assert_eq!(
            ConversationHistoryResolver::capability("openai:responses", "openai:search"),
            ConversationHistoryCapability::Unsupported
        );

        let invalid = ConversationHistoryResolver::resolve(
            &json!({"previous_response_id": {"unexpected": true}}),
            "openai:responses",
            "openai:chat",
        )
        .expect_err("non-string history IDs must not be silently dropped");
        assert!(invalid.to_string().contains("non-empty string"));

        let unsupported = ConversationHistoryResolver::resolve(
            &json!({"previous_response_id": "resp_unsupported_pair"}),
            "openai:responses",
            "openai:search",
        )
        .expect_err("unsupported continuation pairs must fail closed");
        assert!(unsupported
            .to_string()
            .contains("openai:responses to openai:search"));
    }

    #[test]
    fn tenant_and_api_key_both_partition_history() {
        let owner_scope = conversation_history_scope("tenant-a", "key-shared").unwrap();
        let other_tenant_scope = conversation_history_scope("tenant-b", "key-shared").unwrap();
        let other_key_scope = conversation_history_scope("tenant-a", "key-other").unwrap();
        assert_ne!(owner_scope, other_tenant_scope);
        assert_ne!(owner_scope, other_key_scope);

        let report_context = json!({
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:responses",
            "user_id": "tenant-a",
            "api_key_id": "key-shared",
            "provider_id": "provider-native",
            "endpoint_id": "endpoint-native",
            "key_id": "credential-native",
            "original_request_body": {"input": "private turn"}
        });
        let record = record_converted_response_history(
            &report_context,
            &json!({
                "id": "resp_tenant_and_key_scope",
                "status": "completed",
                "output": [{"type": "message", "role": "assistant", "content": []}]
            }),
        )
        .expect("native Responses history should be recorded");
        assert_eq!(
            record.storage_key,
            response_history_storage_key("resp_tenant_and_key_scope", Some(owner_scope.as_str()))
        );

        let request = json!({
            "previous_response_id": "resp_tenant_and_key_scope",
            "input": "continue"
        });
        assert!(
            expand_previous_response_for_conversion(&request, Some(owner_scope.as_str())).is_ok()
        );
        assert!(expand_previous_response_for_conversion(
            &request,
            Some(other_tenant_scope.as_str())
        )
        .is_err());
        assert!(
            expand_previous_response_for_conversion(&request, Some(other_key_scope.as_str()))
                .is_err()
        );

        let other_tenant_record = record_converted_response_history(
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "user_id": "tenant-b",
                "api_key_id": "key-shared",
                "provider_id": "provider-native",
                "endpoint_id": "endpoint-native",
                "key_id": "credential-native",
                "original_request_body": {"input": "other tenant turn"}
            }),
            &json!({
                "id": "resp_tenant_and_key_scope",
                "status": "completed",
                "output": [{"type": "message", "role": "assistant", "content": []}]
            }),
        )
        .expect("the same provider response ID may exist in another tenant scope");
        assert_ne!(record.storage_key, other_tenant_record.storage_key);

        let owner = expand_previous_response_for_conversion(&request, Some(owner_scope.as_str()))
            .expect("owner history should remain available");
        let other_tenant =
            expand_previous_response_for_conversion(&request, Some(other_tenant_scope.as_str()))
                .expect("other tenant should resolve only its own scoped record");
        assert_eq!(owner["input"][0]["content"], "private turn");
        assert_eq!(other_tenant["input"][0]["content"], "other tenant turn");
    }

    #[test]
    fn partial_gateway_identity_does_not_create_an_orphaned_history_record() {
        let record = record_converted_response_history(
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:chat",
                "api_key_id": "key-without-tenant",
                "original_request_body": {"input": "must not be persisted"}
            }),
            &json!({
                "id": "resp_partial_gateway_identity",
                "status": "completed",
                "output": []
            }),
        );

        assert!(record.is_none());
    }

    #[test]
    fn native_history_requires_complete_gateway_identity() {
        for report_context in [
            json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "original_request_body": {"input": "missing identity"}
            }),
            json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "user_id": "tenant-without-key",
                "original_request_body": {"input": "partial identity"}
            }),
        ] {
            assert!(record_converted_response_history(
                &report_context,
                &json!({
                    "id": "resp_native_without_complete_identity",
                    "status": "completed",
                    "output": []
                })
            )
            .is_none());
        }
    }

    #[test]
    fn all_history_capabilities_reject_unscoped_records_and_lookups() {
        for (provider_api_format, response_id) in [
            ("openai:responses", "resp_unscoped_native"),
            ("openai:chat", "resp_unscoped_hydrate"),
            ("claude:messages", "resp_unscoped_translate"),
        ] {
            let record = record_converted_response_history(
                &json!({
                    "client_api_format": "openai:responses",
                    "provider_api_format": provider_api_format,
                    "original_request_body": {"input": "must remain unrecorded"}
                }),
                &json!({
                    "id": response_id,
                    "status": "completed",
                    "output": []
                }),
            );
            assert!(
                record.is_none(),
                "{provider_api_format} created unscoped history"
            );
            assert!(!super::response_history_is_loaded(response_id, None));
            assert!(expand_previous_response_for_conversion(
                &json!({"previous_response_id": response_id, "input": "continue"}),
                None,
            )
            .is_err());
        }

        assert!(hydrate_response_history("resp_unscoped_hydration", None, "{}",).is_err());
    }

    #[test]
    fn native_history_validation_binds_scope_and_provider_credential() {
        let scope = conversation_history_scope("tenant-native", "api-key-native").unwrap();
        let response_id = "resp_native_binding";
        let record = record_converted_response_history(
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:responses",
                "user_id": "tenant-native",
                "api_key_id": "api-key-native",
                "provider_id": "provider-native",
                "endpoint_id": "endpoint-native",
                "key_id": "credential-native",
                "original_request_body": {"input": "private native turn"}
            }),
            &json!({
                "id": response_id,
                "status": "completed",
                "output": []
            }),
        )
        .expect("complete native ownership should produce a record");

        validate_native_response_history(
            response_id,
            &scope,
            "openai:responses",
            "provider-native",
            "endpoint-native",
            "credential-native",
            &record.payload,
        )
        .map(|validation| assert_eq!(validation, NativeResponseHistoryValidation::Match))
        .expect("the exact scope and provider credential should validate");

        assert!(validate_native_response_history(
            response_id,
            &conversation_history_scope("other-tenant", "api-key-native").unwrap(),
            "openai:responses",
            "provider-native",
            "endpoint-native",
            "credential-native",
            &record.payload,
        )
        .is_err());

        for (provider_api_format, provider_id, endpoint_id, key_id) in [
            (
                "openai:chat",
                "provider-native",
                "endpoint-native",
                "credential-native",
            ),
            (
                "openai:responses",
                "other-provider",
                "endpoint-native",
                "credential-native",
            ),
            (
                "openai:responses",
                "provider-native",
                "other-endpoint",
                "credential-native",
            ),
            (
                "openai:responses",
                "provider-native",
                "endpoint-native",
                "other-credential",
            ),
        ] {
            assert_eq!(
                validate_native_response_history(
                    response_id,
                    &scope,
                    provider_api_format,
                    provider_id,
                    endpoint_id,
                    key_id,
                    &record.payload,
                )
                .unwrap(),
                NativeResponseHistoryValidation::CandidateMismatch
            );
        }
    }

    #[test]
    fn oversized_native_history_is_rejected_before_deserialization() {
        let payload = "x".repeat(MAX_HISTORY_ENTRY_BYTES + 1);
        let error = validate_native_response_history(
            "resp_oversized_native",
            "scope",
            "openai:responses",
            "provider",
            "endpoint",
            "credential",
            &payload,
        )
        .expect_err("oversized native history must fail closed");

        assert_eq!(
            error,
            "persisted response history exceeds the maximum entry size"
        );
    }

    #[test]
    fn continuation_recording_rejects_a_missing_parent_transcript() {
        let history_scope = conversation_history_scope("tenant-chain", "key-chain").unwrap();
        let report_context = json!({
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:chat",
            "user_id": "tenant-chain",
            "api_key_id": "key-chain",
            "original_request_body": {
                "previous_response_id": "resp_missing_parent",
                "input": "must not become a truncated chain"
            }
        });
        let response = json!({
            "id": "resp_child_without_parent",
            "status": "completed",
            "output": []
        });
        let record = record_converted_response_history(&report_context, &response);

        assert!(record.is_none());
        assert!(
            super::try_record_converted_response_history(&report_context, &response)
                .expect_err("a missing parent transcript must be an explicit construction failure")
                .contains("missing parent transcript")
        );
        assert!(!super::response_history_is_loaded(
            "resp_child_without_parent",
            Some(history_scope.as_str())
        ));
    }

    #[test]
    fn native_continuation_records_provider_resolved_history_without_materializing_it() {
        let parent_response_id = "resp_native_provider_owned_parent";
        let child_response_id = "resp_native_provider_owned_child";
        let history_scope =
            conversation_history_scope("tenant-native-chain", "key-native-chain").unwrap();
        super::evict_response_history(parent_response_id, Some(history_scope.as_str()));
        super::evict_response_history(child_response_id, Some(history_scope.as_str()));
        let report_context = json!({
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:responses",
            "user_id": "tenant-native-chain",
            "api_key_id": "key-native-chain",
            "provider_id": "provider-native-chain",
            "endpoint_id": "endpoint-native-chain",
            "key_id": "credential-native-chain",
            "original_request_body": {
                "previous_response_id": parent_response_id,
                "input": "current provider-owned turn"
            }
        });
        let response = json!({
            "id": child_response_id,
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "current output"}]
            }]
        });

        let record = super::try_record_converted_response_history(&report_context, &response)
            .expect("native construction may rely on the provider-owned parent")
            .expect("completed native response should produce history");
        assert!(record.payload.contains("\"transcript_materialized\":false"));
        assert_eq!(
            validate_native_response_history(
                child_response_id,
                history_scope.as_str(),
                "openai:responses",
                "provider-native-chain",
                "endpoint-native-chain",
                "credential-native-chain",
                &record.payload,
            )
            .unwrap(),
            NativeResponseHistoryValidation::Match
        );

        commit_response_history_record(&record);
        assert!(!super::response_history_is_loaded(
            child_response_id,
            Some(history_scope.as_str())
        ));
        assert!(!hydrate_native_response_history_if_materialized(
            child_response_id,
            history_scope.as_str(),
            &record.payload,
        )
        .expect("validated provider-resolved history is valid for native routing"));
        assert!(hydrate_response_history(
            child_response_id,
            Some(history_scope.as_str()),
            &record.payload,
        )
        .expect_err("provider-resolved history must not enter the materialization cache")
        .contains("cannot be materialized"));

        let converted_child_context = json!({
            "needs_conversion": true,
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:chat",
            "user_id": "tenant-native-chain",
            "api_key_id": "key-native-chain",
            "original_request_body": {
                "previous_response_id": child_response_id,
                "input": "must not lose provider-owned context"
            }
        });
        let converted_child = json!({
            "id": "resp_converted_from_provider_resolved_parent",
            "status": "completed",
            "output": []
        });
        assert!(super::try_record_converted_response_history(
            &converted_child_context,
            &converted_child
        )
        .expect_err("converted continuation still requires a complete local parent")
        .contains("missing parent transcript"));
    }

    #[test]
    fn completed_history_rejects_missing_or_invalid_output() {
        let report_context = conversion_report_context(json!({
            "input": "must not become truncated history"
        }));

        for response in [
            json!({
                "id": "resp_missing_history_output",
                "status": "completed"
            }),
            json!({
                "id": "resp_invalid_history_output",
                "status": "completed",
                "output": {"unexpected": true}
            }),
        ] {
            let error = super::try_record_converted_response_history(&report_context, &response)
                .expect_err("completed history requires a valid output array");
            assert!(error.contains("valid output array"));
        }
    }

    #[test]
    fn completed_history_rejects_a_non_object_original_request() {
        let error = super::try_record_converted_response_history(
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:chat",
                "user_id": "invalid-request-user",
                "api_key_id": "invalid-request-key",
                "original_request_body": "not-an-object"
            }),
            &json!({
                "id": "resp_invalid_original_request",
                "status": "completed",
                "output": []
            }),
        )
        .expect_err("a non-object original request must not create history");

        assert!(error.contains("must be a JSON object"));
    }

    #[test]
    fn translated_provider_responses_are_recorded_for_later_hydration() {
        let report_context = json!({
            "needs_conversion": true,
            "client_api_format": "openai:responses",
            "provider_api_format": "claude:messages",
            "user_id": "translated-history-user",
            "api_key_id": "translated-history-key",
            "original_request_body": {"input": "translate this turn"}
        });
        assert!(record_converted_response_history(
            &report_context,
            &json!({
                "id": "resp_translated_history",
                "status": "completed",
                "output": [{"type": "message", "role": "assistant", "content": []}]
            })
        )
        .is_some());

        assert!(record_converted_response_history(
            &json!({
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:search",
                "original_request_body": {"input": "unsupported history"}
            }),
            &json!({
                "id": "resp_unsupported_history",
                "status": "completed",
                "output": []
            })
        )
        .is_none());
    }

    #[test]
    fn native_continuation_preserves_the_provider_owned_response_id() {
        let converted = convert_request(
            "openai:responses",
            "openai:responses",
            &json!({
                "model": "gpt-5",
                "input": "continue",
                "previous_response_id": "resp_owned_by_provider"
            }),
            &FormatContext::default(),
        )
        .expect("native continuation must not require gateway-local history");

        assert_eq!(converted["previous_response_id"], "resp_owned_by_provider");
    }

    #[test]
    fn translates_tool_history_to_claude_and_gemini_wire_formats() {
        let history_scope = conversation_history_scope("tenant-translate", "key-translate")
            .expect("test identities should produce a scope");
        let report_context = json!({
            "client_api_format": "openai:responses",
            "provider_api_format": "claude:messages",
            "user_id": "tenant-translate",
            "api_key_id": "key-translate",
            "original_request_body": {
                "model": "source-model",
                "input": "find the deployment"
            }
        });
        record_converted_response_history(
            &report_context,
            &json!({
                "id": "resp_translate_tool_history",
                "status": "completed",
                "output": [{
                    "type": "function_call",
                    "id": "fc_translate_tool_history",
                    "call_id": "call_translate_tool_history",
                    "name": "find_deployment",
                    "arguments": "{\"environment\":\"prod\"}"
                }]
            }),
        )
        .expect("completed translated response should record history");
        let continuation = json!({
            "model": "source-model",
            "previous_response_id": "resp_translate_tool_history",
            "input": [{
                "type": "function_call_output",
                "call_id": "call_translate_tool_history",
                "output": "deployment-42"
            }]
        });
        let context = FormatContext::default().with_history_scope(history_scope);

        let claude = convert_request(
            "openai:responses",
            "claude:messages",
            &continuation,
            &context,
        )
        .expect("stored Responses history should translate to Claude");
        let claude_blocks = claude["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|message| message.get("content").and_then(Value::as_array))
            .flatten()
            .collect::<Vec<_>>();
        assert!(claude_blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("tool_use")
                && block.get("id").and_then(Value::as_str) == Some("call_translate_tool_history")
        }));
        assert!(claude_blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("tool_result")
                && block.get("tool_use_id").and_then(Value::as_str)
                    == Some("call_translate_tool_history")
        }));

        let gemini = convert_request(
            "openai:responses",
            "gemini:generate_content",
            &continuation,
            &context,
        )
        .expect("stored Responses history should translate to Gemini");
        let gemini_parts = gemini["contents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|content| content.get("parts").and_then(Value::as_array))
            .flatten()
            .collect::<Vec<_>>();
        assert!(gemini_parts.iter().any(|part| {
            part.get("functionCall")
                .and_then(|call| call.get("id"))
                .and_then(Value::as_str)
                == Some("call_translate_tool_history")
        }));
        assert!(gemini_parts.iter().any(|part| {
            part.get("functionResponse")
                .and_then(|response| response.get("id"))
                .and_then(Value::as_str)
                == Some("call_translate_tool_history")
        }));
    }

    #[test]
    fn expands_previous_response_with_assistant_call_before_tool_output() {
        let history_scope =
            conversation_history_scope("conversion-test-user", "conversion-test-key").unwrap();
        let report_context = conversion_report_context(json!({
            "model": "deepseek-v4-flash",
            "input": [{"role": "user", "content": "scan the repository"}]
        }));
        record_converted_response_history(
            &report_context,
            &json!({
                "id": "resp_history_expand_test_1",
                "status": "completed",
                "output": [{
                    "type": "function_call",
                    "id": "fc_history_expand_test_1",
                    "call_id": "call_history_expand_test_1",
                    "name": "security_scan",
                    "arguments": "{\"depth\":\"deep\"}"
                }]
            }),
        );

        let expanded = expand_previous_response_for_conversion(
            &json!({
                "model": "deepseek-v4-flash",
                "previous_response_id": "resp_history_expand_test_1",
                "input": [{
                    "type": "function_call_output",
                    "call_id": "call_history_expand_test_1",
                    "output": "manifest-created"
                }]
            }),
            Some(history_scope.as_str()),
        )
        .expect("stored previous response should expand");

        assert!(expanded.get("previous_response_id").is_none());
        assert_eq!(expanded["input"][1]["type"], "function_call");
        assert_eq!(expanded["input"][1]["id"], "fc_history_expand_test_1");
        assert_eq!(
            expanded["input"][1]["call_id"],
            "call_history_expand_test_1"
        );
        assert_eq!(expanded["input"][2]["type"], "function_call_output");
        assert_eq!(
            expanded["input"][2]["call_id"],
            "call_history_expand_test_1"
        );
    }

    #[test]
    fn restores_persisted_history_after_local_cache_reset() {
        let report_context = json!({
            "needs_conversion": true,
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:chat",
            "user_id": "distributed-history-user-a",
            "api_key_id": "distributed-history-key-a",
            "original_request_body": {
                "model": "deepseek-v4-flash",
                "input": [{"role": "user", "content": "inspect the repository"}]
            }
        });
        let history_scope =
            conversation_history_scope("distributed-history-user-a", "distributed-history-key-a")
                .unwrap();
        let record = record_converted_response_history(
            &report_context,
            &json!({
                "id": "resp_distributed_history_1",
                "status": "completed",
                "output": [{
                    "type": "function_call",
                    "id": "fc_distributed_history_1",
                    "call_id": "call_distributed_history_1",
                    "name": "inspect_repository",
                    "arguments": "{}"
                }]
            }),
        )
        .expect("completed conversion should produce a persistence record");
        assert_eq!(
            record.storage_key,
            response_history_storage_key(
                "resp_distributed_history_1",
                Some(history_scope.as_str())
            )
        );
        assert!(!record.storage_key.contains("distributed-history-key-a"));
        assert!(!record.storage_key.contains("resp_distributed_history_1"));

        response_history_store()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&response_history_key(
                "resp_distributed_history_1",
                Some(history_scope.as_str()),
            ));
        let continuation = json!({
            "model": "deepseek-v4-flash",
            "previous_response_id": "resp_distributed_history_1",
            "input": [{
                "type": "function_call_output",
                "call_id": "call_distributed_history_1",
                "output": "inspection-complete"
            }]
        });
        assert!(expand_previous_response_for_conversion(
            &continuation,
            Some(history_scope.as_str())
        )
        .is_err());

        hydrate_response_history(
            "resp_distributed_history_1",
            Some(history_scope.as_str()),
            &record.payload,
        )
        .expect("another instance should hydrate the persisted transcript");
        let expanded =
            expand_previous_response_for_conversion(&continuation, Some(history_scope.as_str()))
                .expect("hydrated history should support the continuation");
        assert_eq!(
            expanded["input"][1]["call_id"],
            "call_distributed_history_1"
        );
        assert_eq!(
            expanded["input"][2]["call_id"],
            "call_distributed_history_1"
        );
    }

    #[test]
    fn uncommitted_record_is_invisible_until_persisted_payload_is_hydrated() {
        let response_id = "resp_uncommitted_cross_node_history";
        let history_scope =
            conversation_history_scope("uncommitted-user", "uncommitted-api-key").unwrap();
        response_history_store()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&response_history_key(
                response_id,
                Some(history_scope.as_str()),
            ));
        let record = build_response_history_record(
            &json!({
                "needs_conversion": true,
                "client_api_format": "openai:responses",
                "provider_api_format": "openai:chat",
                "user_id": "uncommitted-user",
                "api_key_id": "uncommitted-api-key",
                "original_request_body": {"input": "cross-node request"}
            }),
            &json!({
                "id": response_id,
                "status": "completed",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "persisted"}]
                }]
            }),
        )
        .expect("completed conversion should build a persistence record");

        assert!(!super::response_history_is_loaded(
            response_id,
            Some(history_scope.as_str())
        ));
        hydrate_response_history(response_id, Some(history_scope.as_str()), &record.payload)
            .expect("another node should hydrate only the persisted payload");
        assert!(super::response_history_is_loaded(
            response_id,
            Some(history_scope.as_str())
        ));
    }

    #[test]
    fn restores_history_recorded_from_redacted_chat_messages() {
        let history_scope =
            conversation_history_scope("conversion-test-user", "conversion-test-key").unwrap();
        record_converted_response_history(
            &conversion_report_context(json!({
                "model": "deepseek-v4-flash",
                "messages": [
                    {"role": "user", "content": "scan the redacted repository"},
                    {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_redacted_previous_1",
                            "type": "function",
                            "function": {
                                "name": "security_scan",
                                "arguments": "{\"depth\":\"quick\"}"
                            }
                        }]
                    },
                    {
                        "role": "tool",
                        "tool_call_id": "call_redacted_previous_1",
                        "content": "quick-scan-complete"
                    }
                ]
            })),
            &json!({
                "id": "resp_history_redacted_test_1",
                "status": "completed",
                "output": [{
                    "type": "function_call",
                    "id": "fc_history_redacted_test_1",
                    "call_id": "call_redacted_next_1",
                    "name": "deep_scan",
                    "arguments": "{\"scope\":\"changed-files\"}"
                }]
            }),
        );

        let expanded = expand_previous_response_for_conversion(
            &json!({
                "model": "deepseek-v4-flash",
                "previous_response_id": "resp_history_redacted_test_1",
                "input": [{
                    "type": "function_call_output",
                    "call_id": "call_redacted_next_1",
                    "output": "deep-scan-complete"
                }]
            }),
            Some(history_scope.as_str()),
        )
        .expect("redacted Chat history should expand");
        let input = expanded["input"]
            .as_array()
            .expect("expanded input should be an array");

        assert_eq!(input.len(), 5);
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_redacted_previous_1");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call_redacted_previous_1");
        assert_eq!(input[3]["id"], "fc_history_redacted_test_1");
        assert_eq!(input[3]["call_id"], "call_redacted_next_1");
        assert_eq!(input[4]["type"], "function_call_output");
        assert_eq!(input[4]["call_id"], "call_redacted_next_1");
    }

    #[test]
    fn isolates_response_history_by_tenant_and_api_key_scope() {
        let report_context = json!({
            "needs_conversion": true,
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:chat",
            "user_id": "user-history-scope-a",
            "api_key_id": "key-history-scope-a",
            "original_request_body": {
                "model": "deepseek-v4-flash",
                "input": [{"role": "user", "content": "private request"}]
            }
        });
        record_converted_response_history(
            &report_context,
            &json!({
                "id": "resp_history_scope_test_1",
                "status": "completed",
                "output": [{"type": "message", "role": "assistant", "content": []}]
            }),
        );
        let continuation = json!({
            "model": "deepseek-v4-flash",
            "previous_response_id": "resp_history_scope_test_1",
            "input": "continue"
        });
        let owner_scope =
            conversation_history_scope("user-history-scope-a", "key-history-scope-a").unwrap();
        let other_scope =
            conversation_history_scope("user-history-scope-b", "key-history-scope-a").unwrap();

        let owner_result = convert_request(
            "openai:responses",
            "openai:chat",
            &continuation,
            &FormatContext::default().with_history_scope(owner_scope),
        );
        assert!(owner_result.is_ok());

        let other_user_error = convert_request(
            "openai:responses",
            "openai:chat",
            &continuation,
            &FormatContext::default().with_history_scope(other_scope),
        )
        .expect_err("another tenant must not recover scoped history");
        assert!(matches!(
            other_user_error,
            crate::formats::context::FormatError::UnsupportedField { ref field, .. }
                if field == "previous_response_id"
        ));
    }

    #[test]
    fn converts_all_forty_two_tools_and_forces_late_tools() {
        let tools = (0..42)
            .map(|index| {
                json!({
                    "type": "function",
                    "name": format!("security_tool_{index:02}"),
                    "description": format!("Security tool {index}"),
                    "parameters": {
                        "type": "object",
                        "properties": {"path": {"type": "string"}},
                        "required": ["path"]
                    }
                })
            })
            .collect::<Vec<_>>();

        for forced_index in [18usize, 41usize] {
            let body = json!({
                "model": "deepseek-v4-flash",
                "input": [{"role": "user", "content": "run the selected scan"}],
                "tools": tools,
                "tool_choice": {
                    "type": "function",
                    "name": format!("security_tool_{forced_index:02}")
                }
            });
            let converted = convert_request(
                "openai:responses",
                "openai:chat",
                &body,
                &FormatContext::default(),
            )
            .expect("all Responses tools should convert to Chat");
            let converted_tools = converted["tools"]
                .as_array()
                .expect("chat tools should be an array");

            assert_eq!(converted_tools.len(), 42);
            for (index, tool) in converted_tools.iter().enumerate() {
                assert_eq!(
                    tool["function"]["name"],
                    json!(format!("security_tool_{index:02}"))
                );
            }
            assert_eq!(
                converted["tool_choice"]["function"]["name"],
                json!(format!("security_tool_{forced_index:02}"))
            );
        }
    }

    #[test]
    fn restores_two_consecutive_tool_call_rounds() {
        let history_scope =
            conversation_history_scope("conversion-test-user", "conversion-test-key").unwrap();
        let first_request = json!({
            "model": "deepseek-v4-flash",
            "input": [{"role": "user", "content": "perform a deep scan"}],
            "parallel_tool_calls": true
        });
        record_converted_response_history(
            &conversion_report_context(first_request.clone()),
            &json!({
                "id": "resp_history_multiturn_test_1",
                "status": "completed",
                "output": [{
                    "type": "function_call",
                    "id": "fc_history_multiturn_test_1",
                    "call_id": "call_discovery_manifest_1",
                    "name": "create_discovery_manifest",
                    "arguments": "{\"root\":\"src\"}"
                }]
            }),
        );

        let second_request = json!({
            "model": "deepseek-v4-flash",
            "previous_response_id": "resp_history_multiturn_test_1",
            "input": [{
                "type": "function_call_output",
                "call_id": "call_discovery_manifest_1",
                "output": "manifest-1"
            }],
            "parallel_tool_calls": true
        });
        let second_chat = convert_request(
            "openai:responses",
            "openai:chat",
            &second_request,
            &FormatContext::default().with_history_scope(history_scope.clone()),
        )
        .expect("first continuation should convert");
        assert_eq!(second_chat["messages"][1]["role"], "assistant");
        assert_eq!(
            second_chat["messages"][1]["tool_calls"][0]["id"],
            "call_discovery_manifest_1"
        );
        assert_eq!(second_chat["messages"][2]["role"], "tool");
        assert_eq!(
            second_chat["messages"][2]["tool_call_id"],
            "call_discovery_manifest_1"
        );

        record_converted_response_history(
            &conversion_report_context(second_request.clone()),
            &json!({
                "id": "resp_history_multiturn_test_2",
                "status": "completed",
                "output": [
                    {
                        "type": "function_call",
                        "id": "fc_history_multiturn_test_2a",
                        "call_id": "call_deep_scan_2a",
                        "name": "deep_scan",
                        "arguments": "{\"manifest\":\"manifest-1\"}"
                    },
                    {
                        "type": "function_call",
                        "id": "fc_history_multiturn_test_2b",
                        "call_id": "call_audit_2b",
                        "name": "audit_results",
                        "arguments": "{\"manifest\":\"manifest-1\"}"
                    }
                ]
            }),
        );

        let third_chat = convert_request(
            "openai:responses",
            "openai:chat",
            &json!({
                "model": "deepseek-v4-flash",
                "previous_response_id": "resp_history_multiturn_test_2",
                "input": [
                    {
                        "type": "function_call_output",
                        "call_id": "call_deep_scan_2a",
                        "output": "scan-complete"
                    },
                    {
                        "type": "function_call_output",
                        "call_id": "call_audit_2b",
                        "output": "audit-complete"
                    }
                ]
            }),
            &FormatContext::default().with_history_scope(history_scope),
        )
        .expect("second continuation should convert");
        let messages = third_chat["messages"]
            .as_array()
            .expect("chat messages should be an array");
        assert_eq!(messages.len(), 6);
        assert_eq!(messages[3]["role"], "assistant");
        assert_eq!(messages[3]["tool_calls"].as_array().map(Vec::len), Some(2));
        assert_eq!(messages[3]["tool_calls"][0]["id"], "call_deep_scan_2a");
        assert_eq!(messages[3]["tool_calls"][1]["id"], "call_audit_2b");
        assert_eq!(messages[4]["tool_call_id"], "call_deep_scan_2a");
        assert_eq!(messages[5]["tool_call_id"], "call_audit_2b");
    }
}
