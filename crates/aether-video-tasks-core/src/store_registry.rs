use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    GeminiVideoTaskSeed, LocalVideoTaskReadResponse, LocalVideoTaskRegistryMutation,
    LocalVideoTaskSnapshot, LocalVideoTaskStatus, OpenAiVideoTaskSeed,
};

/// Completed task snapshots contain prompts and provider metadata, so retain only a
/// bounded recent history. Active tasks are never evicted by this policy.
pub const VIDEO_TASK_TERMINAL_RETENTION_SECS: u64 = 24 * 60 * 60;
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
        self.prune_terminal(current_unix_secs());
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
        self.prune_terminal(current_unix_secs());
    }

    pub fn project_openai(&mut self, task_id: &str, provider_body: &Map<String, Value>) -> bool {
        let Some(LocalVideoTaskSnapshot::OpenAi(seed)) = self.openai.get_mut(task_id) else {
            return false;
        };
        seed.apply_provider_body(provider_body);
        self.prune_terminal(current_unix_secs());
        true
    }

    pub fn project_gemini(&mut self, short_id: &str, provider_body: &Map<String, Value>) -> bool {
        let Some(LocalVideoTaskSnapshot::Gemini(seed)) = self.gemini.get_mut(short_id) else {
            return false;
        };
        seed.apply_provider_body(provider_body);
        self.prune_terminal(current_unix_secs());
        true
    }

    fn prune_terminal(&mut self, now_unix_secs: u64) {
        prune_terminal_map(&mut self.openai, now_unix_secs);
        prune_terminal_map(&mut self.gemini, now_unix_secs);
    }

    pub(crate) fn sanitize_persisted_diagnostics(&mut self) -> bool {
        let mut changed = false;
        for snapshot in self.openai.values_mut().chain(self.gemini.values_mut()) {
            changed = snapshot.sanitize_persisted_diagnostics() || changed;
        }
        changed
    }
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn prune_terminal_map(map: &mut BTreeMap<String, LocalVideoTaskSnapshot>, now_unix_secs: u64) {
    let cutoff = now_unix_secs.saturating_sub(VIDEO_TASK_TERMINAL_RETENTION_SECS);
    map.retain(|_, snapshot| {
        snapshot.is_active_for_refresh()
            // A missing or legacy synthetic timestamp cannot establish an
            // expiry boundary. Keep it for the capacity bound below instead
            // of deleting an otherwise addressable task on the next write.
            || snapshot_created_at_secs(snapshot) < MIN_REALISTIC_UNIX_SECS
            || snapshot_created_at_secs(snapshot) >= cutoff
    });

    let mut terminal: Vec<(String, u64)> = map
        .iter()
        .filter(|(_, snapshot)| !snapshot.is_active_for_refresh())
        .map(|(key, snapshot)| (key.clone(), snapshot_created_at_secs(snapshot)))
        .collect();
    terminal.sort_by_key(|(_, created_at)| *created_at);
    let excess = terminal
        .len()
        .saturating_sub(VIDEO_TASK_MAX_TERMINAL_ENTRIES);
    for (key, _) in terminal.into_iter().take(excess) {
        map.remove(&key);
    }
}

// Unix seconds before 2000 are treated as unknown/legacy values. Existing
// persisted fixtures and old records may use a zero or placeholder timestamp;
// retaining those records until the per-provider capacity bound is reached is
// safer than making them disappear solely because their timestamp is invalid.
const MIN_REALISTIC_UNIX_SECS: u64 = 946_684_800;

fn snapshot_created_at_secs(snapshot: &LocalVideoTaskSnapshot) -> u64 {
    let value = snapshot.created_at_unix_ms();
    if value >= 1_000_000_000_000 {
        value / 1_000
    } else {
        value
    }
}
