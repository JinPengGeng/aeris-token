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
        prune_terminal_map(&mut self.openai);
        prune_terminal_map(&mut self.gemini);
    }

    pub(crate) fn sanitize_persisted_diagnostics(&mut self) -> bool {
        let mut changed = false;
        for snapshot in self.openai.values_mut().chain(self.gemini.values_mut()) {
            changed = snapshot.sanitize_persisted_diagnostics() || changed;
        }
        changed
    }
}

fn prune_terminal_map(map: &mut BTreeMap<String, LocalVideoTaskSnapshot>) {
    let mut terminal: Vec<(String, u64)> = map
        .iter()
        .filter(|(_, snapshot)| !snapshot.is_active_for_refresh())
        .map(|(key, snapshot)| (key.clone(), snapshot.created_at_unix_ms()))
        .collect();
    terminal.sort_by_key(|(_, created_at)| *created_at);
    let excess = terminal
        .len()
        .saturating_sub(VIDEO_TASK_MAX_TERMINAL_ENTRIES);
    for (key, _) in terminal.into_iter().take(excess) {
        map.remove(&key);
    }
}
