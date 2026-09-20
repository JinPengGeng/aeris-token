use aether_data_contracts::repository::video_tasks::{StoredVideoTask, UpsertVideoTask};
use serde_json::{json, Map, Value};

use crate::transport::{gemini_metadata_video_url, gemini_video_metadata};
use crate::types::sanitize_video_task_error_code;
use crate::{
    local_status_from_stored, non_empty_owned, request_body_string, GeminiVideoTaskSeed,
    LocalVideoTaskPersistence, LocalVideoTaskReadResponse, LocalVideoTaskSnapshot,
    LocalVideoTaskStatus, LocalVideoTaskTransport, OpenAiVideoTaskSeed,
};

impl LocalVideoTaskSnapshot {
    pub(crate) fn created_at_unix_secs(&self) -> u64 {
        match self {
            // The legacy field name is retained for the persisted/API contract;
            // current video-task records store this value in Unix seconds.
            Self::OpenAi(seed) => normalize_unix_timestamp_secs(seed.created_at_unix_ms),
            Self::Gemini(seed) => normalize_unix_timestamp_secs(seed.created_at_unix_secs),
        }
    }

    pub fn to_upsert_record(&self) -> UpsertVideoTask {
        match self {
            Self::OpenAi(seed) => seed.to_upsert_record(),
            Self::Gemini(seed) => seed.to_upsert_record(),
        }
    }

    pub fn from_stored_task(task: &StoredVideoTask) -> Option<Self> {
        let snapshot = task
            .request_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("rust_local_snapshot"))
            .cloned()
            .and_then(|value| serde_json::from_value::<LocalVideoTaskSnapshot>(value).ok())?;
        snapshot.with_stored_task(task)
    }

    /// Preserve transport capability, but take identity and lifecycle exclusively from the row.
    pub fn with_stored_task(&self, task: &StoredVideoTask) -> Option<Self> {
        let transport = match self {
            Self::OpenAi(seed) => seed.transport.clone(),
            Self::Gemini(seed) => seed.transport.clone(),
        };
        let mut task = task.clone();
        task.provider_api_format = task
            .provider_api_format
            .or_else(|| task.client_api_format.clone());
        let mut snapshot = Self::from_stored_task_with_transport(&task, transport)?;
        if let (Self::OpenAi(source), Self::OpenAi(target)) = (self, &mut snapshot) {
            target.xai_provider = source.xai_provider;
        }
        Some(snapshot)
    }

    /// Only use after the source update was accepted, or for a cache observation of
    /// this exact row revision. These provider fields are deliberately not persisted
    /// in the database, but remain part of the native in-memory response contract.
    pub fn with_committed_stored_task(&self, task: &StoredVideoTask) -> Option<Self> {
        let mut snapshot = self.with_stored_task(task)?;
        if let (Self::OpenAi(source), Self::OpenAi(target)) = (self, &mut snapshot) {
            if source.status.as_database_status() == task.status {
                target.native_response = source.native_response.clone();
                target.expires_at_unix_secs = source.expires_at_unix_secs;
                target.remixed_from_video_id = source.remixed_from_video_id.clone();
            }
        }
        Some(snapshot)
    }

    /// Enrich a terminal native view without changing any authoritative row fields.
    pub fn with_terminal_presentation(&self, projected: &Self) -> Option<Self> {
        let (Self::OpenAi(current), Self::OpenAi(projected)) = (self, projected) else {
            return None;
        };
        if !current.uses_xai_provider()
            || current.native_response.is_some()
            || !matches!(
                current.status,
                LocalVideoTaskStatus::Completed
                    | LocalVideoTaskStatus::Failed
                    | LocalVideoTaskStatus::Expired
            )
            || current.persistence.row_revision != projected.persistence.row_revision
            || current.local_task_id != projected.local_task_id
            || current.status != projected.status
            || current.video_url != projected.video_url
            || projected.native_response.is_none()
        {
            return None;
        }
        let mut enriched = current.clone();
        enriched.native_response = projected.native_response.clone();
        enriched.expires_at_unix_secs = projected.expires_at_unix_secs;
        Some(Self::OpenAi(enriched))
    }

    pub fn row_revision(&self) -> i64 {
        match self {
            Self::OpenAi(seed) => seed.persistence.row_revision,
            Self::Gemini(seed) => seed.persistence.row_revision,
        }
    }

    /// Project a finalize report without changing the shared registry.
    pub fn apply_finalize_report(&mut self, report_kind: &str) -> bool {
        let status = match self {
            Self::OpenAi(seed) => &mut seed.status,
            Self::Gemini(seed) => &mut seed.status,
        };
        let next = match report_kind {
            "openai_video_delete_sync_finalize"
                if matches!(
                    *status,
                    LocalVideoTaskStatus::Completed | LocalVideoTaskStatus::Failed
                ) =>
            {
                LocalVideoTaskStatus::Deleted
            }
            "openai_video_cancel_sync_finalize" | "gemini_video_cancel_sync_finalize"
                if matches!(
                    *status,
                    LocalVideoTaskStatus::Submitted
                        | LocalVideoTaskStatus::Queued
                        | LocalVideoTaskStatus::Processing
                ) =>
            {
                LocalVideoTaskStatus::Cancelled
            }
            _ => return false,
        };
        *status = next;
        true
    }

    pub fn from_stored_task_with_transport(
        task: &StoredVideoTask,
        transport: LocalVideoTaskTransport,
    ) -> Option<Self> {
        let provider_api_format = task.provider_api_format.as_deref()?.trim();
        let persistence = LocalVideoTaskPersistence::from_stored_task(task)?;

        match provider_api_format {
            "openai:video" => {
                let upstream_task_id = non_empty_owned(task.external_task_id.as_ref())?;
                Some(Self::OpenAi(OpenAiVideoTaskSeed {
                    local_short_id: task.short_id.clone(),
                    native_response: None,
                    xai_provider: persistence.client_api_format == "xai:video",
                    local_task_id: task.id.clone(),
                    upstream_task_id,
                    created_at_unix_ms: task.created_at_unix_ms,
                    user_id: task.user_id.clone(),
                    api_key_id: task.api_key_id.clone(),
                    model: non_empty_owned(task.model.as_ref()),
                    prompt: non_empty_owned(task.prompt.as_ref()).or_else(|| {
                        request_body_string(&persistence.original_request_body, "prompt")
                    }),
                    size: non_empty_owned(task.size.as_ref()).or_else(|| {
                        request_body_string(&persistence.original_request_body, "size")
                    }),
                    seconds: task
                        .duration_seconds
                        .map(|value| value.to_string())
                        .or_else(|| {
                            request_body_string(&persistence.original_request_body, "seconds")
                        }),
                    remixed_from_video_id: request_body_string(
                        &persistence.original_request_body,
                        "remix_video_id",
                    )
                    .or_else(|| {
                        request_body_string(
                            &persistence.original_request_body,
                            "remixed_from_video_id",
                        )
                    }),
                    status: local_status_from_stored(task.status),
                    progress_percent: task.progress_percent,
                    completed_at_unix_secs: task.completed_at_unix_secs,
                    expires_at_unix_secs: None,
                    error_code: task.error_code.clone(),
                    error_message: task.error_message.clone(),
                    video_url: non_empty_owned(task.video_url.as_ref()),
                    persistence,
                    transport,
                }))
            }
            "gemini:video" => {
                let local_short_id =
                    non_empty_owned(task.short_id.as_ref()).unwrap_or_else(|| task.id.clone());
                let upstream_operation_name = non_empty_owned(task.external_task_id.as_ref())?;
                let model = non_empty_owned(task.model.as_ref())?;
                Some(Self::Gemini(GeminiVideoTaskSeed {
                    local_short_id,
                    upstream_operation_name,
                    created_at_unix_secs: task.created_at_unix_ms,
                    user_id: task.user_id.clone(),
                    api_key_id: task.api_key_id.clone(),
                    model,
                    status: local_status_from_stored(task.status),
                    progress_percent: task.progress_percent,
                    error_code: task.error_code.clone(),
                    error_message: task.error_message.clone(),
                    metadata: gemini_video_metadata(task.video_url.as_deref()),
                    persistence,
                    transport,
                }))
            }
            _ => None,
        }
    }

    pub(crate) fn sanitize_persisted_diagnostics(&mut self) -> bool {
        match self {
            Self::OpenAi(seed) => {
                let previous_error_code = seed.error_code.clone();
                let error_code =
                    sanitized_error_code_for_status(seed.status, seed.error_code.take());
                let changed = previous_error_code != error_code || seed.error_message.is_some();
                seed.error_code = error_code;
                seed.error_message = None;
                changed
            }
            Self::Gemini(seed) => {
                let previous_error_code = seed.error_code.clone();
                let error_code =
                    sanitized_error_code_for_status(seed.status, seed.error_code.take());
                let safe_metadata =
                    gemini_video_metadata(gemini_metadata_video_url(&seed.metadata).as_deref());
                let changed = previous_error_code != error_code
                    || seed.error_message.is_some()
                    || seed.metadata != safe_metadata;
                seed.error_code = error_code;
                seed.error_message = None;
                seed.metadata = safe_metadata;
                changed
            }
        }
    }

    pub fn read_response_for_path(&self, path: &str) -> LocalVideoTaskReadResponse {
        if let Self::OpenAi(seed) = self {
            let mut seed = seed.clone();
            if path.starts_with("/openai/v1/videos/") {
                seed.persistence.client_api_format = "openai:video".to_string();
            } else if path.starts_with("/v1/videos/") && seed.is_xai_native() {
                seed.persistence.client_api_format = "xai:video".to_string();
            }
            return Self::OpenAi(seed).read_response();
        }
        self.read_response()
    }

    pub fn read_response(&self) -> LocalVideoTaskReadResponse {
        match self {
            Self::OpenAi(seed) => match seed.status {
                LocalVideoTaskStatus::Cancelled => LocalVideoTaskReadResponse {
                    status_code: 404,
                    body_json: json!({"detail": "Video task was cancelled"}),
                },
                LocalVideoTaskStatus::Deleted => LocalVideoTaskReadResponse {
                    status_code: 404,
                    body_json: json!({"detail": "Video task not found"}),
                },
                _ => LocalVideoTaskReadResponse {
                    status_code: 200,
                    body_json: seed.client_body_json(),
                },
            },
            Self::Gemini(seed) => match seed.status {
                LocalVideoTaskStatus::Cancelled => LocalVideoTaskReadResponse {
                    status_code: 404,
                    body_json: json!({"detail": "Video task was cancelled"}),
                },
                LocalVideoTaskStatus::Deleted => LocalVideoTaskReadResponse {
                    status_code: 404,
                    body_json: json!({"detail": "Video task not found"}),
                },
                _ => LocalVideoTaskReadResponse {
                    status_code: 200,
                    body_json: seed.client_body_json(),
                },
            },
        }
    }

    pub fn belongs_to_user(&self, user_id: &str) -> bool {
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return false;
        }
        let owner = match self {
            Self::OpenAi(seed) => seed.user_id.as_deref(),
            Self::Gemini(seed) => seed.user_id.as_deref(),
        };
        owner.map(str::trim) == Some(user_id)
    }

    pub fn is_active_for_refresh(&self) -> bool {
        match self {
            Self::OpenAi(seed) => matches!(
                seed.status,
                LocalVideoTaskStatus::Submitted
                    | LocalVideoTaskStatus::Queued
                    | LocalVideoTaskStatus::Processing
            ),
            Self::Gemini(seed) => matches!(
                seed.status,
                LocalVideoTaskStatus::Submitted
                    | LocalVideoTaskStatus::Queued
                    | LocalVideoTaskStatus::Processing
            ),
        }
    }

    pub fn apply_provider_body(&mut self, provider_body: &Map<String, Value>) {
        match self {
            Self::OpenAi(seed) => seed.apply_provider_body(provider_body),
            Self::Gemini(seed) => seed.apply_provider_body(provider_body),
        }
    }

    pub fn provider_name(&self) -> Option<&str> {
        match self {
            Self::OpenAi(seed) => seed.transport.provider_name.as_deref(),
            Self::Gemini(seed) => seed.transport.provider_name.as_deref(),
        }
    }
}

fn normalize_unix_timestamp_secs(value: u64) -> u64 {
    // A small number of pre-retention stores used the legacy field name
    // literally and persisted milliseconds. Accept both encodings so those
    // records receive the same expiry policy instead of becoming immortal.
    // Unix seconds remain below this bound until year 2286, while practical
    // millisecond timestamps have exceeded it since April 1970.
    if value >= 10_000_000_000 {
        value / 1_000
    } else {
        value
    }
}

fn sanitized_error_code_for_status(
    status: LocalVideoTaskStatus,
    error_code: Option<String>,
) -> Option<String> {
    match status {
        LocalVideoTaskStatus::Failed => sanitize_video_task_error_code(error_code)
            .or_else(|| Some("provider_error".to_string())),
        LocalVideoTaskStatus::Expired => Some("expired".to_string()),
        LocalVideoTaskStatus::Cancelled => Some("cancelled".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod revision_display_tests {
    use super::*;

    #[test]
    fn only_committed_sources_preserve_native_display_fields() {
        let source: LocalVideoTaskSnapshot = serde_json::from_value(json!({
            "OpenAi": {
                "local_task_id":"native-task", "upstream_task_id":"upstream-task",
                "created_at_unix_ms":100, "user_id":"owner", "api_key_id":"key",
                "model":"grok-imagine-video", "prompt":null, "size":null, "seconds":"6",
                "remixed_from_video_id":null, "status":"Completed", "progress_percent":100,
                "completed_at_unix_secs":200, "expires_at_unix_secs":300,
                "error_code":null, "error_message":null, "video_url":"https://example.test/video.mp4",
                "native_response":{"status":"done","video":{"url":"https://example.test/video.mp4","respect_moderation":true},"provider_extension":"preserved"},
                "xai_provider":true,
                "persistence":{"row_revision":3,"request_id":"request-native","username":null,"api_key_name":null,
                    "client_api_format":"xai:video","provider_api_format":"openai:video","original_request_body":{},"format_converted":false},
                "transport":{"upstream_base_url":"https://api.example.test","provider_name":"xai","provider_id":"provider",
                    "endpoint_id":"endpoint","key_id":"key","headers":{},"content_type":null,"model_name":null,
                    "proxy":null,"transport_profile":null,"timeouts":null}
            }
        })).unwrap();
        let mut row = source.to_upsert_record().into_stored();
        row.row_revision = 4;
        let accepted = source.with_committed_stored_task(&row).unwrap();
        let LocalVideoTaskSnapshot::OpenAi(accepted) = accepted else {
            unreachable!()
        };
        assert_eq!(accepted.persistence.row_revision, 4);
        assert_eq!(
            accepted.native_response.as_ref().unwrap()["provider_extension"],
            "preserved"
        );
        assert_eq!(
            accepted.native_response.as_ref().unwrap()["video"]["respect_moderation"],
            true
        );
        assert_eq!(accepted.expires_at_unix_secs, Some(300));

        row.status = aether_data_contracts::repository::video_tasks::VideoTaskStatus::Cancelled;
        row.progress_percent = 73;
        let recovered = source.with_stored_task(&row).unwrap();
        let LocalVideoTaskSnapshot::OpenAi(recovered) = recovered else {
            unreachable!()
        };
        assert_eq!(recovered.status, LocalVideoTaskStatus::Cancelled);
        assert_eq!(recovered.progress_percent, 73);
        assert_eq!(recovered.persistence.row_revision, 4);
        assert!(recovered.native_response.is_none());
        assert!(recovered.expires_at_unix_secs.is_none());
    }
}
