use serde_json::{Map, Value};

use crate::{
    GeminiVideoTaskSeed, LocalVideoTaskReadResponse, LocalVideoTaskRegistryMutation,
    LocalVideoTaskSnapshot, OpenAiVideoTaskSeed,
};

pub trait VideoTaskStore: std::fmt::Debug + Send + Sync {
    /// Returns true when the snapshot was durably recorded (persisted for the file store).
    /// A false result means the mutation was rejected and in-memory state was left unchanged.
    fn insert(&self, snapshot: LocalVideoTaskSnapshot) -> bool;
    fn replace_local_snapshot(
        &self,
        expected: &LocalVideoTaskSnapshot,
        replacement: LocalVideoTaskSnapshot,
    ) -> bool;
    fn enrich_terminal_presentation(
        &self,
        expected: &LocalVideoTaskSnapshot,
        projected: &LocalVideoTaskSnapshot,
    ) -> bool;
    fn read_openai(&self, task_id: &str) -> Option<LocalVideoTaskReadResponse>;
    fn read_gemini(&self, short_id: &str) -> Option<LocalVideoTaskReadResponse>;
    fn clone_openai(&self, task_id: &str) -> Option<OpenAiVideoTaskSeed>;
    fn clone_gemini(&self, short_id: &str) -> Option<GeminiVideoTaskSeed>;
    fn list_active_snapshots(&self, limit: usize) -> Vec<LocalVideoTaskSnapshot>;
    /// Returns true when the mutation was durably applied (persisted for the file store).
    /// A false result means the mutation was rejected and in-memory state was left unchanged.
    fn apply_mutation(&self, mutation: LocalVideoTaskRegistryMutation) -> bool;
    fn project_openai(&self, task_id: &str, provider_body: &Map<String, Value>) -> bool;
    fn project_gemini(&self, short_id: &str, provider_body: &Map<String, Value>) -> bool;
}
