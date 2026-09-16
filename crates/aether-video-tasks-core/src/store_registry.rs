use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    GeminiVideoTaskSeed, LocalVideoTaskReadResponse, LocalVideoTaskRegistryMutation,
    LocalVideoTaskSnapshot, LocalVideoTaskStatus, OpenAiVideoTaskSeed,
};

/// Completed task snapshots contain prompts and provider metadata, so retain only a
/// bounded recent history. Active tasks are never evicted by this policy.
pub const VIDEO_TASK_MAX_TERMINAL_ENTRIES: usize = 4096;
/// Persisted terminal snapshots are useful for a short follow-up window, but must
/// not keep prompts and provider metadata indefinitely when traffic stays below the
/// capacity bound. The file store applies this policy when it loads a registry.
pub const VIDEO_TASK_TERMINAL_RETENTION_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VideoTaskRegistry {
    openai: BTreeMap<String, LocalVideoTaskSnapshot>,
    gemini: BTreeMap<String, LocalVideoTaskSnapshot>,
}

impl VideoTaskRegistry {
    pub fn insert(&mut self, mut snapshot: LocalVideoTaskSnapshot) {
        snapshot.sanitize_persisted_diagnostics();
        match &snapshot {
            LocalVideoTaskSnapshot::OpenAi(seed) => {
                self.openai.insert(seed.local_task_id.clone(), snapshot);
            }
            LocalVideoTaskSnapshot::Gemini(seed) => {
                self.gemini.insert(seed.local_short_id.clone(), snapshot);
            }
        }
        self.prune_terminal();
    }

    pub fn read_openai(&self, task_id: &str) -> Option<LocalVideoTaskReadResponse> {
        self.openai
            .get(task_id)
            .map(LocalVideoTaskSnapshot::read_response)
    }

    pub fn read_gemini(&self, short_id: &str) -> Option<LocalVideoTaskReadResponse> {
        self.gemini
            .get(short_id)
            .map(LocalVideoTaskSnapshot::read_response)
    }

    pub fn clone_openai(&self, task_id: &str) -> Option<OpenAiVideoTaskSeed> {
        let LocalVideoTaskSnapshot::OpenAi(seed) = self.openai.get(task_id)?.clone() else {
            return None;
        };
        Some(seed)
    }

    pub fn clone_gemini(&self, short_id: &str) -> Option<GeminiVideoTaskSeed> {
        let LocalVideoTaskSnapshot::Gemini(seed) = self.gemini.get(short_id)?.clone() else {
            return None;
        };
        Some(seed)
    }

    pub fn list_active_snapshots(&self, limit: usize) -> Vec<LocalVideoTaskSnapshot> {
        self.openai
            .values()
            .chain(self.gemini.values())
            .filter(|snapshot| snapshot.is_active_for_refresh())
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn apply_mutation(&mut self, mutation: LocalVideoTaskRegistryMutation) {
        match mutation {
            LocalVideoTaskRegistryMutation::OpenAiCancelled { task_id } => {
                if let Some(LocalVideoTaskSnapshot::OpenAi(seed)) = self.openai.get_mut(&task_id) {
                    seed.status = LocalVideoTaskStatus::Cancelled;
                }
            }
            LocalVideoTaskRegistryMutation::OpenAiDeleted { task_id } => {
                if let Some(LocalVideoTaskSnapshot::OpenAi(seed)) = self.openai.get_mut(&task_id) {
                    seed.status = LocalVideoTaskStatus::Deleted;
                }
            }
            LocalVideoTaskRegistryMutation::GeminiCancelled { short_id } => {
                if let Some(LocalVideoTaskSnapshot::Gemini(seed)) = self.gemini.get_mut(&short_id) {
                    seed.status = LocalVideoTaskStatus::Cancelled;
                }
            }
        }
        self.prune_terminal();
    }

    pub fn project_openai(&mut self, task_id: &str, provider_body: &Map<String, Value>) -> bool {
        let Some(LocalVideoTaskSnapshot::OpenAi(seed)) = self.openai.get_mut(task_id) else {
            return false;
        };
        seed.apply_provider_body(provider_body);
        self.prune_terminal();
        true
    }

    pub fn project_gemini(&mut self, short_id: &str, provider_body: &Map<String, Value>) -> bool {
        let Some(LocalVideoTaskSnapshot::Gemini(seed)) = self.gemini.get_mut(short_id) else {
            return false;
        };
        seed.apply_provider_body(provider_body);
        self.prune_terminal();
        true
    }

    fn prune_terminal(&mut self) {
        prune_terminal_map(&mut self.openai, None);
        prune_terminal_map(&mut self.gemini, None);
    }

    /// Remove terminal snapshots older than the retention window and enforce the
    /// per-provider capacity bound. A zero creation timestamp is retained because
    /// it denotes a legacy snapshot whose age cannot be established safely.
    pub fn prune_terminal_at(&mut self, now_unix_secs: u64) -> bool {
        prune_terminal_map(&mut self.openai, Some(now_unix_secs))
            | prune_terminal_map(&mut self.gemini, Some(now_unix_secs))
    }

    pub(crate) fn sanitize_persisted_diagnostics(&mut self) -> bool {
        let mut changed = false;
        for snapshot in self.openai.values_mut().chain(self.gemini.values_mut()) {
            changed = snapshot.sanitize_persisted_diagnostics() || changed;
        }
        changed
    }
}

fn prune_terminal_map(
    map: &mut BTreeMap<String, LocalVideoTaskSnapshot>,
    now_unix_secs: Option<u64>,
) -> bool {
    let mut terminal: Vec<(String, u64)> = map
        .iter()
        .filter(|(_, snapshot)| !snapshot.is_active_for_refresh())
        .map(|(key, snapshot)| (key.clone(), snapshot.created_at_unix_secs()))
        .collect();
    let mut changed = false;
    if let Some(now_unix_secs) = now_unix_secs {
        let expired: Vec<String> = terminal
            .iter()
            .filter(|(_, created_at)| {
                *created_at != 0
                    && now_unix_secs.saturating_sub(*created_at)
                        >= VIDEO_TASK_TERMINAL_RETENTION_SECS
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            changed |= map.remove(&key).is_some();
        }
        terminal.retain(|(key, _)| map.contains_key(key));
    }
    terminal.sort_by_key(|(_, created_at)| *created_at);
    let excess = terminal
        .len()
        .saturating_sub(VIDEO_TASK_MAX_TERMINAL_ENTRIES);
    for (key, _) in terminal.into_iter().take(excess) {
        changed |= map.remove(&key).is_some();
    }
    changed
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{LocalVideoTaskPersistence, LocalVideoTaskTransport};

    fn gemini_snapshot(
        local_short_id: &str,
        created_at_unix_secs: u64,
        status: LocalVideoTaskStatus,
    ) -> LocalVideoTaskSnapshot {
        LocalVideoTaskSnapshot::Gemini(GeminiVideoTaskSeed {
            local_short_id: local_short_id.to_string(),
            upstream_operation_name: format!("operations/{local_short_id}"),
            created_at_unix_secs,
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            model: "veo-3".to_string(),
            status,
            progress_percent: 0,
            error_code: None,
            error_message: None,
            metadata: json!({}),
            persistence: LocalVideoTaskPersistence {
                request_id: format!("request-{local_short_id}"),
                username: None,
                api_key_name: None,
                client_api_format: "gemini:video".to_string(),
                provider_api_format: "gemini:video".to_string(),
                original_request_body: json!({"prompt": "test"}),
                format_converted: false,
            },
            transport: LocalVideoTaskTransport {
                upstream_base_url: "https://generativelanguage.googleapis.com".to_string(),
                provider_name: Some("gemini".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                headers: Default::default(),
                content_type: Some("application/json".to_string()),
                model_name: Some("veo-3".to_string()),
                proxy: None,
                transport_profile: None,
                timeouts: None,
            },
        })
    }

    #[test]
    fn terminal_age_pruning_keeps_active_recent_and_unknown_age_snapshots() {
        let now = 2_000_000;
        let mut registry = VideoTaskRegistry::default();
        registry.insert(gemini_snapshot(
            "expired",
            now - VIDEO_TASK_TERMINAL_RETENTION_SECS - 1,
            LocalVideoTaskStatus::Completed,
        ));
        registry.insert(gemini_snapshot(
            "recent",
            now - VIDEO_TASK_TERMINAL_RETENTION_SECS + 1,
            LocalVideoTaskStatus::Completed,
        ));
        registry.insert(gemini_snapshot(
            "active",
            now - VIDEO_TASK_TERMINAL_RETENTION_SECS - 1,
            LocalVideoTaskStatus::Processing,
        ));
        registry.insert(gemini_snapshot(
            "legacy",
            0,
            LocalVideoTaskStatus::Completed,
        ));

        assert!(registry.prune_terminal_at(now));
        assert!(registry.read_gemini("expired").is_none());
        assert!(registry.read_gemini("recent").is_some());
        assert!(registry.read_gemini("active").is_some());
        assert!(registry.read_gemini("legacy").is_some());
    }

    #[test]
    fn terminal_age_pruning_is_idempotent() {
        let now = 2_000_000;
        let mut registry = VideoTaskRegistry::default();
        registry.insert(gemini_snapshot(
            "expired",
            now - VIDEO_TASK_TERMINAL_RETENTION_SECS - 1,
            LocalVideoTaskStatus::Failed,
        ));

        assert!(registry.prune_terminal_at(now));
        assert!(!registry.prune_terminal_at(now));
    }

    #[test]
    fn terminal_age_pruning_uses_openai_created_at_seconds_contract() {
        let now = 2_000_000;
        let mut registry = VideoTaskRegistry::default();
        registry.insert(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
            local_short_id: None,
            native_response: None,
            xai_provider: false,
            local_task_id: "openai-expired".to_string(),
            upstream_task_id: "upstream-expired".to_string(),
            created_at_unix_ms: now - VIDEO_TASK_TERMINAL_RETENTION_SECS - 1,
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            model: Some("sora-2".to_string()),
            prompt: None,
            size: None,
            seconds: None,
            remixed_from_video_id: None,
            status: LocalVideoTaskStatus::Failed,
            progress_percent: 100,
            completed_at_unix_secs: Some(now - VIDEO_TASK_TERMINAL_RETENTION_SECS - 1),
            expires_at_unix_secs: None,
            error_code: Some("provider_error".to_string()),
            error_message: None,
            video_url: None,
            persistence: LocalVideoTaskPersistence {
                request_id: "request-openai-expired".to_string(),
                username: None,
                api_key_name: None,
                client_api_format: "openai:video".to_string(),
                provider_api_format: "openai:video".to_string(),
                original_request_body: json!({"prompt": "test"}),
                format_converted: false,
            },
            transport: LocalVideoTaskTransport {
                upstream_base_url: "https://api.openai.example".to_string(),
                provider_name: Some("openai".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                headers: Default::default(),
                content_type: Some("application/json".to_string()),
                model_name: Some("sora-2".to_string()),
                proxy: None,
                transport_profile: None,
                timeouts: None,
            },
        }));

        assert!(registry.prune_terminal_at(now));
        assert!(registry.read_openai("openai-expired").is_none());
    }

    #[test]
    fn terminal_age_pruning_normalizes_legacy_openai_millisecond_timestamps() {
        let now = 1_000_000_000;
        let created_at_secs = now - VIDEO_TASK_TERMINAL_RETENTION_SECS - 1;
        let mut registry = VideoTaskRegistry::default();
        registry.insert(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
            local_task_id: "openai-legacy-ms".to_string(),
            upstream_task_id: "upstream-legacy-ms".to_string(),
            created_at_unix_ms: created_at_secs * 1_000,
            user_id: None,
            api_key_id: None,
            model: Some("sora-2".to_string()),
            prompt: None,
            size: None,
            seconds: None,
            remixed_from_video_id: None,
            status: LocalVideoTaskStatus::Completed,
            progress_percent: 100,
            completed_at_unix_secs: Some(created_at_secs),
            expires_at_unix_secs: None,
            error_code: None,
            error_message: None,
            video_url: None,
            persistence: LocalVideoTaskPersistence {
                request_id: "request-openai-legacy-ms".to_string(),
                username: None,
                api_key_name: None,
                client_api_format: "openai:video".to_string(),
                provider_api_format: "openai:video".to_string(),
                original_request_body: json!({}),
                format_converted: false,
            },
            transport: LocalVideoTaskTransport {
                upstream_base_url: "https://api.openai.example".to_string(),
                provider_name: Some("openai".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                headers: Default::default(),
                content_type: Some("application/json".to_string()),
                model_name: Some("sora-2".to_string()),
                proxy: None,
                transport_profile: None,
                timeouts: None,
            },
        }));

        assert!(registry.prune_terminal_at(now));
        assert!(registry.read_openai("openai-legacy-ms").is_none());
    }
}
