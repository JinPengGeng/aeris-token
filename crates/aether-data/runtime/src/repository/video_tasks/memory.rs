use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    StoredVideoTask, UpsertVideoTask, VideoTaskClaim, VideoTaskLookupKey, VideoTaskModelCount,
    VideoTaskQueryFilter, VideoTaskReadRepository, VideoTaskStatus, VideoTaskStatusCount,
    VideoTaskWriteRepository,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
struct MemoryVideoTaskIndex {
    by_id: BTreeMap<String, StoredVideoTask>,
    short_to_id: BTreeMap<String, String>,
    request_to_id: BTreeMap<String, String>,
    user_external_to_id: BTreeMap<(String, String), String>,
    fencing_tokens: BTreeMap<String, i64>,
}

#[derive(Debug, Default)]
pub struct InMemoryVideoTaskRepository {
    index: RwLock<MemoryVideoTaskIndex>,
}

impl InMemoryVideoTaskRepository {
    fn store_locked(index: &mut MemoryVideoTaskIndex, task: StoredVideoTask) -> StoredVideoTask {
        index.fencing_tokens.entry(task.id.clone()).or_insert(0);
        if let Some(previous) = index.by_id.insert(task.id.clone(), task.clone()) {
            if let Some(short_id) = previous.short_id {
                index.short_to_id.remove(&short_id);
            }
            index.request_to_id.remove(&previous.request_id);
            if let (Some(user_id), Some(external_task_id)) =
                (previous.user_id, previous.external_task_id)
            {
                index
                    .user_external_to_id
                    .remove(&(user_id, external_task_id));
            }
        }

        if let Some(short_id) = &task.short_id {
            index.short_to_id.insert(short_id.clone(), task.id.clone());
        }
        index
            .request_to_id
            .insert(task.request_id.clone(), task.id.clone());
        if let (Some(user_id), Some(external_task_id)) = (&task.user_id, &task.external_task_id) {
            index
                .user_external_to_id
                .insert((user_id.clone(), external_task_id.clone()), task.id.clone());
        }

        task
    }

    fn ensure_unique_keys_available(
        index: &MemoryVideoTaskIndex,
        task: &UpsertVideoTask,
    ) -> Result<(), DataLayerError> {
        if let Some(short_id) = task.short_id.as_deref() {
            if index
                .short_to_id
                .get(short_id)
                .is_some_and(|existing_id| existing_id != &task.id)
            {
                return Err(DataLayerError::InvalidInput(format!(
                    "video task {} conflicts with existing short_id {short_id}",
                    task.id
                )));
            }
        }
        if index
            .request_to_id
            .get(&task.request_id)
            .is_some_and(|existing_id| existing_id != &task.id)
        {
            return Err(DataLayerError::InvalidInput(format!(
                "video task {} conflicts with existing request_id {}",
                task.id, task.request_id
            )));
        }
        Ok(())
    }

    fn matches_filter(task: &StoredVideoTask, filter: &VideoTaskQueryFilter) -> bool {
        if let Some(user_id) = filter.user_id.as_deref() {
            if task.user_id.as_deref() != Some(user_id) {
                return false;
            }
        }
        if let Some(status) = filter.status {
            if task.status != status {
                return false;
            }
        }
        if let Some(model_substring) = filter.model_substring.as_deref() {
            let needle = model_substring.trim().to_ascii_lowercase();
            let Some(model) = task.model.as_deref() else {
                return false;
            };
            if !model.to_ascii_lowercase().contains(&needle) {
                return false;
            }
        }
        if let Some(client_api_format) = filter.client_api_format.as_deref() {
            if task.client_api_format.as_deref() != Some(client_api_format) {
                return false;
            }
        }
        true
    }
}

#[async_trait]
impl VideoTaskReadRepository for InMemoryVideoTaskRepository {
    async fn find(
        &self,
        key: VideoTaskLookupKey<'_>,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        let index = self.index.read().expect("video task repository lock");
        Ok(match key {
            VideoTaskLookupKey::Id(id) => index.by_id.get(id).cloned(),
            VideoTaskLookupKey::ShortId(short_id) => index
                .short_to_id
                .get(short_id)
                .and_then(|id| index.by_id.get(id))
                .cloned(),
            VideoTaskLookupKey::UserExternal {
                user_id,
                external_task_id,
            } => index
                .user_external_to_id
                .get(&(user_id.to_string(), external_task_id.to_string()))
                .and_then(|id| index.by_id.get(id))
                .cloned(),
        })
    }

    async fn find_for_user(
        &self,
        key: VideoTaskLookupKey<'_>,
        user_id: &str,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        let index = self.index.read().expect("video task repository lock");
        let task = match key {
            VideoTaskLookupKey::Id(id) => index.by_id.get(id),
            VideoTaskLookupKey::ShortId(short_id) => index
                .short_to_id
                .get(short_id)
                .and_then(|id| index.by_id.get(id)),
            VideoTaskLookupKey::UserExternal {
                user_id: lookup_user_id,
                external_task_id,
            } => {
                if lookup_user_id != user_id {
                    return Ok(None);
                }
                index
                    .user_external_to_id
                    .get(&(lookup_user_id.to_string(), external_task_id.to_string()))
                    .and_then(|id| index.by_id.get(id))
            }
        };
        Ok(task
            .filter(|task| task.user_id.as_deref() == Some(user_id))
            .cloned())
    }

    async fn list_active(&self, limit: usize) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut tasks = self
            .index
            .read()
            .expect("video task repository lock")
            .by_id
            .values()
            .filter(|task| task.status.is_active())
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by_key(|entry| std::cmp::Reverse(entry.updated_at_unix_secs));
        tasks.truncate(limit);
        Ok(tasks)
    }

    async fn list_due(
        &self,
        now_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut tasks = self
            .index
            .read()
            .expect("video task repository lock")
            .by_id
            .values()
            .filter(|task| {
                matches!(
                    task.status,
                    super::VideoTaskStatus::Submitted
                        | super::VideoTaskStatus::Queued
                        | super::VideoTaskStatus::Processing
                ) && task.poll_count < task.max_poll_count
                    && task
                        .next_poll_at_unix_secs
                        .is_some_and(|value| value <= now_unix_secs)
            })
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| {
            left.next_poll_at_unix_secs
                .cmp(&right.next_poll_at_unix_secs)
                .then_with(|| left.updated_at_unix_secs.cmp(&right.updated_at_unix_secs))
        });
        tasks.truncate(limit);
        Ok(tasks)
    }

    async fn list_page(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut tasks = self
            .index
            .read()
            .expect("video task repository lock")
            .by_id
            .values()
            .filter(|task| Self::matches_filter(task, filter))
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| {
            right
                .created_at_unix_ms
                .cmp(&left.created_at_unix_ms)
                .then_with(|| right.updated_at_unix_secs.cmp(&left.updated_at_unix_secs))
        });
        Ok(tasks.into_iter().skip(offset).take(limit).collect())
    }

    async fn list_page_summary(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, DataLayerError> {
        Self::list_page(self, filter, offset, limit).await
    }

    async fn count(&self, filter: &VideoTaskQueryFilter) -> Result<u64, DataLayerError> {
        Ok(self
            .index
            .read()
            .expect("video task repository lock")
            .by_id
            .values()
            .filter(|task| Self::matches_filter(task, filter))
            .count() as u64)
    }

    async fn count_by_status(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<Vec<VideoTaskStatusCount>, DataLayerError> {
        let mut counts = BTreeMap::<VideoTaskStatus, u64>::new();
        for task in self
            .index
            .read()
            .expect("video task repository lock")
            .by_id
            .values()
            .filter(|task| Self::matches_filter(task, filter))
        {
            *counts.entry(task.status).or_default() += 1;
        }
        Ok(counts
            .into_iter()
            .map(|(status, count)| VideoTaskStatusCount { status, count })
            .collect())
    }

    async fn count_distinct_users(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<u64, DataLayerError> {
        let index = self.index.read().expect("video task repository lock");
        let users = index
            .by_id
            .values()
            .filter(|task| Self::matches_filter(task, filter))
            .filter_map(|task| task.user_id.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<std::collections::BTreeSet<_>>();
        Ok(users.len() as u64)
    }

    async fn top_models(
        &self,
        filter: &VideoTaskQueryFilter,
        limit: usize,
    ) -> Result<Vec<VideoTaskModelCount>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut counts = BTreeMap::<String, u64>::new();
        for task in self
            .index
            .read()
            .expect("video task repository lock")
            .by_id
            .values()
            .filter(|task| Self::matches_filter(task, filter))
        {
            let Some(model) = task.model.as_deref() else {
                continue;
            };
            if model.trim().is_empty() {
                continue;
            }
            *counts.entry(model.to_string()).or_default() += 1;
        }

        let mut models = counts
            .into_iter()
            .map(|(model, count)| VideoTaskModelCount { model, count })
            .collect::<Vec<_>>();
        models.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.model.cmp(&right.model))
        });
        models.truncate(limit);
        Ok(models)
    }

    async fn count_created_since(
        &self,
        filter: &VideoTaskQueryFilter,
        created_since_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        Ok(self
            .index
            .read()
            .expect("video task repository lock")
            .by_id
            .values()
            .filter(|task| {
                Self::matches_filter(task, filter)
                    && task.created_at_unix_ms >= created_since_unix_secs
            })
            .count() as u64)
    }
}

#[async_trait]
impl VideoTaskWriteRepository for InMemoryVideoTaskRepository {
    async fn upsert(&self, mut task: UpsertVideoTask) -> Result<StoredVideoTask, DataLayerError> {
        let mut index = self.index.write().expect("video task repository lock");
        Self::ensure_unique_keys_available(&index, &task)?;
        if let Some(existing) = index.by_id.get(&task.id) {
            existing.ensure_immutable_identity_matches(&task)?;
            task.created_at_unix_ms = existing.created_at_unix_ms;
            if task.row_revision != existing.row_revision {
                return Err(DataLayerError::InvalidInput(
                    "video task row revision conflict".to_string(),
                ));
            }
            if !allows_transition(existing.status, task.status) {
                return Err(DataLayerError::InvalidInput(
                    "video task terminal state conflict".to_string(),
                ));
            }
            task.updated_at_unix_secs =
                task.updated_at_unix_secs.max(existing.updated_at_unix_secs);
            task.row_revision = next_revision(existing.row_revision)?;
        } else {
            task.row_revision = 1;
        }
        Ok(Self::store_locked(&mut index, task.into_stored()))
    }

    async fn update_if_active(
        &self,
        mut task: UpsertVideoTask,
        fencing_token: Option<i64>,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        let mut index = self.index.write().expect("video task repository lock");
        if Self::ensure_unique_keys_available(&index, &task).is_err() {
            return Ok(None);
        }
        let Some(existing) = index.by_id.get(&task.id) else {
            return Ok(None);
        };
        if fencing_token.is_some_and(|token| {
            index
                .fencing_tokens
                .get(&task.id)
                .copied()
                .unwrap_or_default()
                != token
        }) {
            return Ok(None);
        }
        if task.row_revision != existing.row_revision {
            return Ok(None);
        }
        if !allows_transition(existing.status, task.status) {
            return Ok(None);
        }
        if existing.ensure_immutable_identity_matches(&task).is_err() {
            return Ok(None);
        }
        task.created_at_unix_ms = existing.created_at_unix_ms;
        task.updated_at_unix_secs = task.updated_at_unix_secs.max(existing.updated_at_unix_secs);
        task.row_revision = next_revision(existing.row_revision)?;
        Ok(Some(Self::store_locked(&mut index, task.into_stored())))
    }

    async fn cleanup_terminal_before(
        &self,
        completed_before_unix_secs: u64,
        limit: usize,
    ) -> Result<u64, DataLayerError> {
        if limit == 0 {
            return Ok(0);
        }
        let mut index = self.index.write().expect("video task repository lock");
        let expired_ids: Vec<String> = index
            .by_id
            .values()
            .filter(|task| {
                !task.status.is_active()
                    && task
                        .completed_at_unix_secs
                        .unwrap_or(task.updated_at_unix_secs)
                        < completed_before_unix_secs
            })
            .take(limit)
            .map(|task| task.id.clone())
            .collect();
        let deleted = u64::try_from(expired_ids.len()).unwrap_or(u64::MAX);
        for id in &expired_ids {
            if let Some(previous) = index.by_id.remove(id) {
                if let Some(short_id) = previous.short_id {
                    index.short_to_id.remove(&short_id);
                }
                index.request_to_id.remove(&previous.request_id);
                if let (Some(user_id), Some(external_task_id)) =
                    (previous.user_id, previous.external_task_id)
                {
                    index
                        .user_external_to_id
                        .remove(&(user_id, external_task_id));
                }
            }
            index.fencing_tokens.remove(id);
        }
        Ok(deleted)
    }

    async fn claim_due(
        &self,
        now_unix_secs: u64,
        claim_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<VideoTaskClaim>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut index = self.index.write().expect("video task repository lock");
        let mut due_ids = index
            .by_id
            .values()
            .filter(|task| {
                matches!(
                    task.status,
                    VideoTaskStatus::Submitted
                        | VideoTaskStatus::Queued
                        | VideoTaskStatus::Processing
                ) && task.poll_count < task.max_poll_count
                    && task
                        .next_poll_at_unix_secs
                        .is_some_and(|value| value <= now_unix_secs)
            })
            .map(|task| task.id.clone())
            .collect::<Vec<_>>();
        due_ids.sort_by(|left_id, right_id| {
            let left = index.by_id.get(left_id).expect("task should exist");
            let right = index.by_id.get(right_id).expect("task should exist");
            left.next_poll_at_unix_secs
                .cmp(&right.next_poll_at_unix_secs)
                .then_with(|| left.updated_at_unix_secs.cmp(&right.updated_at_unix_secs))
        });
        due_ids.truncate(limit);

        // Validate the whole batch before mutating it, as the SQL statement does.
        for id in &due_ids {
            next_revision(index.by_id[id].row_revision)?;
            next_revision(index.fencing_tokens.get(id).copied().unwrap_or_default())?;
        }

        let mut claimed = Vec::with_capacity(due_ids.len());
        for id in due_ids {
            let token = index.fencing_tokens.entry(id.clone()).or_insert(0);
            *token += 1;
            let fencing_token = *token;
            let Some(task) = index.by_id.get_mut(&id) else {
                continue;
            };
            task.next_poll_at_unix_secs = Some(claim_until_unix_secs);
            task.row_revision += 1;
            task.updated_at_unix_secs = now_unix_secs.max(task.updated_at_unix_secs);
            claimed.push(VideoTaskClaim {
                task: task.clone(),
                fencing_token,
            });
        }
        Ok(claimed)
    }
}

fn allows_transition(from: VideoTaskStatus, to: VideoTaskStatus) -> bool {
    (from.is_active() && to != VideoTaskStatus::Deleted)
        || (matches!(from, VideoTaskStatus::Completed | VideoTaskStatus::Failed)
            && to == VideoTaskStatus::Deleted)
}

fn next_revision(value: i64) -> Result<i64, DataLayerError> {
    value
        .checked_add(1)
        .ok_or_else(|| DataLayerError::UnexpectedValue("video task revision exhausted".to_string()))
}

#[cfg(test)]
mod tests {
    use super::InMemoryVideoTaskRepository;
    use crate::repository::video_tasks::{
        UpsertVideoTask, VideoTaskLookupKey, VideoTaskQueryFilter, VideoTaskReadRepository,
        VideoTaskStatus, VideoTaskWriteRepository,
    };

    fn sample_task(
        id: &str,
        status: VideoTaskStatus,
        updated_at_unix_secs: u64,
    ) -> UpsertVideoTask {
        UpsertVideoTask {
            row_revision: 0,
            id: id.to_string(),
            short_id: Some(format!("short-{id}")),
            request_id: format!("request-{id}"),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: Some("user".to_string()),
            api_key_name: Some("primary".to_string()),
            external_task_id: Some(format!("ext-{id}")),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("provider-key-1".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora-2".to_string()),
            prompt: Some("hello".to_string()),
            original_request_body: Some(serde_json::json!({"prompt": "hello"})),
            duration_seconds: Some(4),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("1280x720".to_string()),
            status,
            progress_percent: 0,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: Some(updated_at_unix_secs),
            poll_count: 0,
            max_poll_count: 360,
            created_at_unix_ms: updated_at_unix_secs.saturating_sub(10),
            submitted_at_unix_secs: Some(updated_at_unix_secs.saturating_sub(10)),
            completed_at_unix_secs: None,
            updated_at_unix_secs,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: None,
        }
    }

    #[tokio::test]
    async fn cleanup_terminal_before_is_bounded_and_idempotent() {
        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("old-done", VideoTaskStatus::Completed, 100))
            .await
            .expect("upsert should succeed");
        repo.upsert(sample_task("old-failed", VideoTaskStatus::Failed, 200))
            .await
            .expect("upsert should succeed");
        repo.upsert(sample_task("recent-done", VideoTaskStatus::Completed, 900))
            .await
            .expect("upsert should succeed");
        repo.upsert(sample_task("active", VideoTaskStatus::Processing, 100))
            .await
            .expect("upsert should succeed");

        // Cutoff 500: only terminal tasks at/under 200 qualify; limit 1 bounds the batch.
        let deleted = repo
            .cleanup_terminal_before(500, 1)
            .await
            .expect("cleanup should succeed");
        assert_eq!(deleted, 1);
        let deleted = repo
            .cleanup_terminal_before(500, 100)
            .await
            .expect("cleanup should succeed");
        assert_eq!(deleted, 1);
        // Idempotent: a third pass deletes nothing.
        let deleted = repo
            .cleanup_terminal_before(500, 100)
            .await
            .expect("cleanup should succeed");
        assert_eq!(deleted, 0);

        assert!(repo
            .find(VideoTaskLookupKey::Id("recent-done"))
            .await
            .expect("find should succeed")
            .is_some());
        assert!(repo
            .find(VideoTaskLookupKey::Id("active"))
            .await
            .expect("find should succeed")
            .is_some());
        assert!(repo
            .find(VideoTaskLookupKey::Id("old-done"))
            .await
            .expect("find should succeed")
            .is_none());
        assert!(repo
            .find(VideoTaskLookupKey::Id("old-failed"))
            .await
            .expect("find should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn reads_task_by_all_supported_lookup_keys() {
        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Submitted, 100))
            .await
            .expect("upsert should succeed");

        assert!(repo
            .find(VideoTaskLookupKey::Id("task-1"))
            .await
            .expect("find by id should succeed")
            .is_some());
        assert!(repo
            .find(VideoTaskLookupKey::ShortId("short-task-1"))
            .await
            .expect("find by short id should succeed")
            .is_some());
        assert!(repo
            .find(VideoTaskLookupKey::UserExternal {
                user_id: "user-1",
                external_task_id: "ext-task-1",
            })
            .await
            .expect("find by user/external should succeed")
            .is_some());
    }

    #[tokio::test]
    async fn owner_scoped_lookup_rejects_foreign_user_for_every_identifier() {
        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Submitted, 100))
            .await
            .expect("upsert should succeed");

        for key in [
            VideoTaskLookupKey::Id("task-1"),
            VideoTaskLookupKey::ShortId("short-task-1"),
            VideoTaskLookupKey::UserExternal {
                user_id: "user-1",
                external_task_id: "ext-task-1",
            },
        ] {
            assert!(repo
                .find_for_user(key, "user-1")
                .await
                .expect("owner lookup should succeed")
                .is_some());
            assert!(repo
                .find_for_user(key, "user-2")
                .await
                .expect("foreign lookup should succeed")
                .is_none());
        }
    }

    #[tokio::test]
    async fn list_active_only_returns_active_tasks_in_descending_update_order() {
        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Completed, 100))
            .await
            .expect("upsert should succeed");
        repo.upsert(sample_task("task-2", VideoTaskStatus::Processing, 200))
            .await
            .expect("upsert should succeed");
        repo.upsert(sample_task("task-3", VideoTaskStatus::Queued, 150))
            .await
            .expect("upsert should succeed");

        let active = repo
            .list_active(10)
            .await
            .expect("list active should succeed");
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].id, "task-2");
        assert_eq!(active[1].id, "task-3");
    }

    #[tokio::test]
    async fn upsert_rejects_immutable_identity_replacement() {
        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Submitted, 100))
            .await
            .expect("upsert should succeed");

        let conflict = repo
            .upsert(UpsertVideoTask {
                row_revision: 0,
                id: "task-1".to_string(),
                short_id: Some("short-task-1b".to_string()),
                request_id: "request-task-1b".to_string(),
                user_id: Some("user-2".to_string()),
                api_key_id: Some("api-key-2".to_string()),
                username: Some("user-2".to_string()),
                api_key_name: Some("secondary".to_string()),
                external_task_id: Some("ext-task-1b".to_string()),
                provider_id: Some("provider-2".to_string()),
                endpoint_id: Some("endpoint-2".to_string()),
                key_id: Some("provider-key-2".to_string()),
                client_api_format: Some("gemini:video".to_string()),
                provider_api_format: Some("gemini:video".to_string()),
                format_converted: false,
                model: Some("veo-3".to_string()),
                prompt: Some("remix".to_string()),
                original_request_body: Some(serde_json::json!({"prompt": "remix"})),
                duration_seconds: Some(8),
                resolution: Some("1080p".to_string()),
                aspect_ratio: Some("16:9".to_string()),
                size: Some("720p".to_string()),
                status: VideoTaskStatus::Processing,
                progress_percent: 50,
                progress_message: Some("processing".to_string()),
                retry_count: 1,
                poll_interval_seconds: 10,
                next_poll_at_unix_secs: Some(200),
                poll_count: 2,
                max_poll_count: 360,
                created_at_unix_ms: 150,
                submitted_at_unix_secs: Some(150),
                completed_at_unix_secs: None,
                updated_at_unix_secs: 200,
                error_code: None,
                error_message: None,
                video_url: None,
                request_metadata: None,
            })
            .await
            .expect_err("identity replacement should be rejected");
        assert!(conflict.to_string().contains("immutable field short_id"));

        let stored = repo
            .find(VideoTaskLookupKey::Id("task-1"))
            .await
            .expect("find should succeed")
            .expect("original task should remain");
        assert_eq!(stored.request_id, "request-task-1");
        assert_eq!(stored.user_id.as_deref(), Some("user-1"));
        assert_eq!(stored.status, VideoTaskStatus::Submitted);
        assert!(repo
            .find(VideoTaskLookupKey::ShortId("short-task-1"))
            .await
            .expect("find should succeed")
            .is_some());
        assert!(repo
            .find(VideoTaskLookupKey::UserExternal {
                user_id: "user-1",
                external_task_id: "ext-task-1",
            })
            .await
            .expect("find should succeed")
            .is_some());
        assert!(repo
            .find(VideoTaskLookupKey::ShortId("short-task-1b"))
            .await
            .expect("find should succeed")
            .is_none());

        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Processing, 100))
            .await
            .expect("upsert should succeed");
        assert!(repo
            .update_if_active(
                UpsertVideoTask {
                    row_revision: 1,
                    status: VideoTaskStatus::Deleted,
                    ..sample_task("task-1", VideoTaskStatus::Deleted, 200)
                },
                None,
            )
            .await
            .expect("active delete update should execute")
            .is_none());
    }

    #[tokio::test]
    async fn upsert_allows_same_identity_status_update() {
        let repo = InMemoryVideoTaskRepository::default();
        let task = sample_task("task-1", VideoTaskStatus::Submitted, 100);
        repo.upsert(task.clone())
            .await
            .expect("initial upsert should succeed");

        let updated = repo
            .upsert(UpsertVideoTask {
                row_revision: 1,
                status: VideoTaskStatus::Processing,
                progress_percent: 50,
                poll_count: 2,
                created_at_unix_ms: 999,
                updated_at_unix_secs: 200,
                ..task
            })
            .await
            .expect("same identity update should succeed");

        assert_eq!(updated.status, VideoTaskStatus::Processing);
        assert_eq!(updated.progress_percent, 50);
        assert_eq!(updated.poll_count, 2);
        assert_eq!(updated.created_at_unix_ms, 90);
        assert_eq!(updated.updated_at_unix_secs, 200);
    }

    #[tokio::test]
    async fn update_if_active_rejects_identity_conflict_without_modification() {
        let repo = InMemoryVideoTaskRepository::default();
        let task = sample_task("task-1", VideoTaskStatus::Submitted, 100);
        repo.upsert(task.clone())
            .await
            .expect("initial upsert should succeed");

        let result = repo
            .update_if_active(
                UpsertVideoTask {
                    row_revision: 1,
                    user_id: Some("attacker".to_string()),
                    status: VideoTaskStatus::Completed,
                    progress_percent: 100,
                    updated_at_unix_secs: 200,
                    ..task
                },
                None,
            )
            .await
            .expect("guarded update should execute");
        assert!(result.is_none());

        let stored = repo
            .find(VideoTaskLookupKey::Id("task-1"))
            .await
            .expect("find should succeed")
            .expect("original task should remain");
        assert_eq!(stored.user_id.as_deref(), Some("user-1"));
        assert_eq!(stored.status, VideoTaskStatus::Submitted);
        assert_eq!(stored.progress_percent, 0);
        assert_eq!(stored.updated_at_unix_secs, 100);
    }

    #[tokio::test]
    async fn upsert_rejects_secondary_unique_key_takeover() {
        let repo = InMemoryVideoTaskRepository::default();
        let original = sample_task("task-1", VideoTaskStatus::Submitted, 100);
        repo.upsert(original.clone())
            .await
            .expect("initial upsert should succeed");

        let short_id_conflict = repo
            .upsert(UpsertVideoTask {
                row_revision: 0,
                id: "task-2".to_string(),
                request_id: "request-task-2".to_string(),
                ..original.clone()
            })
            .await
            .expect_err("a short id must not be reassigned to another task");
        assert!(short_id_conflict.to_string().contains("existing short_id"));

        let request_id_conflict = repo
            .upsert(UpsertVideoTask {
                row_revision: 0,
                id: "task-3".to_string(),
                short_id: Some("short-task-3".to_string()),
                ..original
            })
            .await
            .expect_err("a request id must not be reassigned to another task");
        assert!(request_id_conflict
            .to_string()
            .contains("existing request_id"));

        assert!(repo
            .find(VideoTaskLookupKey::ShortId("short-task-1"))
            .await
            .expect("find should succeed")
            .is_some());
        assert!(repo
            .find(VideoTaskLookupKey::Id("task-2"))
            .await
            .expect("find should succeed")
            .is_none());
        assert!(repo
            .find(VideoTaskLookupKey::Id("task-3"))
            .await
            .expect("find should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn list_due_returns_due_active_tasks_in_next_poll_order() {
        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Submitted, 300))
            .await
            .expect("upsert should succeed");
        repo.upsert(sample_task("task-2", VideoTaskStatus::Processing, 100))
            .await
            .expect("upsert should succeed");
        repo.upsert(UpsertVideoTask {
            row_revision: 0,
            next_poll_at_unix_secs: Some(500),
            ..sample_task("task-3", VideoTaskStatus::Queued, 200)
        })
        .await
        .expect("upsert should succeed");

        let due = repo
            .list_due(300, 10)
            .await
            .expect("list due should succeed");
        assert_eq!(due.len(), 2);
        assert_eq!(due[0].id, "task-2");
        assert_eq!(due[1].id, "task-1");
    }

    #[tokio::test]
    async fn update_if_active_skips_terminal_tasks() {
        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Completed, 100))
            .await
            .expect("upsert should succeed");

        let updated = repo
            .update_if_active(
                UpsertVideoTask {
                    row_revision: 1,
                    progress_percent: 50,
                    ..sample_task("task-1", VideoTaskStatus::Processing, 200)
                },
                None,
            )
            .await
            .expect("update should succeed");

        assert!(updated.is_none());
    }

    #[tokio::test]
    async fn update_if_active_allows_terminal_deletion_only() {
        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Completed, 100))
            .await
            .expect("upsert should succeed");

        let deleted = repo
            .update_if_active(
                UpsertVideoTask {
                    row_revision: 1,
                    status: VideoTaskStatus::Deleted,
                    updated_at_unix_secs: 200,
                    ..sample_task("task-1", VideoTaskStatus::Deleted, 200)
                },
                None,
            )
            .await
            .expect("delete update should execute")
            .expect("terminal deletion should be accepted");
        assert_eq!(deleted.status, VideoTaskStatus::Deleted);

        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Completed, 100))
            .await
            .expect("upsert should succeed");
        assert!(repo
            .update_if_active(
                UpsertVideoTask {
                    row_revision: 1,
                    status: VideoTaskStatus::Processing,
                    ..sample_task("task-1", VideoTaskStatus::Processing, 200)
                },
                None,
            )
            .await
            .expect("stale active update should execute")
            .is_none());
    }

    #[tokio::test]
    async fn list_page_and_stats_apply_filters() {
        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Submitted, 100))
            .await
            .expect("upsert should succeed");
        repo.upsert(UpsertVideoTask {
            row_revision: 0,
            model: Some("veo-3-fast".to_string()),
            user_id: Some("user-2".to_string()),
            client_api_format: Some("gemini:video".to_string()),
            created_at_unix_ms: 250,
            updated_at_unix_secs: 250,
            ..sample_task("task-2", VideoTaskStatus::Completed, 250)
        })
        .await
        .expect("upsert should succeed");
        repo.upsert(UpsertVideoTask {
            row_revision: 0,
            model: Some("veo-3-fast".to_string()),
            user_id: Some("user-2".to_string()),
            client_api_format: Some("gemini:video".to_string()),
            created_at_unix_ms: 260,
            updated_at_unix_secs: 260,
            ..sample_task("task-3", VideoTaskStatus::Completed, 260)
        })
        .await
        .expect("upsert should succeed");

        let filter = VideoTaskQueryFilter {
            user_id: Some("user-2".to_string()),
            status: Some(VideoTaskStatus::Completed),
            model_substring: Some("veo".to_string()),
            client_api_format: Some("gemini:video".to_string()),
        };

        let page = repo
            .list_page(&filter, 0, 10)
            .await
            .expect("list page should succeed");
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].id, "task-3");
        assert_eq!(page[1].id, "task-2");

        let count = repo.count(&filter).await.expect("count should succeed");
        assert_eq!(count, 2);

        let by_status = repo
            .count_by_status(&filter)
            .await
            .expect("status count should succeed");
        assert_eq!(by_status.len(), 1);
        assert_eq!(by_status[0].status, VideoTaskStatus::Completed);
        assert_eq!(by_status[0].count, 2);

        let top_models = repo
            .top_models(&filter, 10)
            .await
            .expect("top models should succeed");
        assert_eq!(top_models.len(), 1);
        assert_eq!(top_models[0].model, "veo-3-fast");
        assert_eq!(top_models[0].count, 2);

        let today_count = repo
            .count_created_since(&filter, 255)
            .await
            .expect("today count should succeed");
        assert_eq!(today_count, 1);
    }

    #[tokio::test]
    async fn claim_due_advances_claimed_tasks_until_claim_deadline() {
        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Submitted, 100))
            .await
            .expect("upsert should succeed");
        repo.upsert(sample_task("task-2", VideoTaskStatus::Processing, 90))
            .await
            .expect("upsert should succeed");

        let claimed = repo
            .claim_due(100, 130, 1)
            .await
            .expect("claim should succeed");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].task.id, "task-2");
        assert_eq!(claimed[0].task.next_poll_at_unix_secs, Some(130));
        assert_eq!(claimed[0].fencing_token, 1);

        let remaining = repo
            .list_due(100, 10)
            .await
            .expect("list due should succeed");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "task-1");
    }

    #[tokio::test]
    async fn stale_claim_cannot_overwrite_a_newer_claim() {
        let repo = InMemoryVideoTaskRepository::default();
        repo.upsert(sample_task("task-1", VideoTaskStatus::Processing, 100))
            .await
            .expect("upsert should succeed");

        let first = repo
            .claim_due(100, 130, 1)
            .await
            .expect("first claim should succeed")
            .pop()
            .expect("first claim should be present");
        let second = repo
            .claim_due(130, 160, 1)
            .await
            .expect("expired task should be reclaimable")
            .pop()
            .expect("second claim should be present");
        assert_eq!(first.fencing_token, 1);
        assert_eq!(second.fencing_token, 2);

        let mut stale_update: UpsertVideoTask = first.task.into();
        stale_update.status = VideoTaskStatus::Completed;
        stale_update.progress_percent = 100;
        stale_update.updated_at_unix_secs = 140;
        assert!(repo
            .update_if_active(stale_update, Some(first.fencing_token))
            .await
            .expect("stale update should execute")
            .is_none());

        let mut current_update: UpsertVideoTask = second.task.into();
        current_update.status = VideoTaskStatus::Completed;
        current_update.progress_percent = 100;
        current_update.updated_at_unix_secs = 141;
        assert!(repo
            .update_if_active(current_update, Some(second.fencing_token))
            .await
            .expect("current update should execute")
            .is_some());
    }

    #[tokio::test]
    async fn revisions_fence_same_second_and_unbound_snapshot_writes() {
        let repo = InMemoryVideoTaskRepository::default();
        let input = sample_task("revision-task", VideoTaskStatus::Processing, 100);
        let initial = repo.upsert(input.clone()).await.unwrap();
        assert_eq!(initial.row_revision, 1);
        assert!(repo.upsert(input.clone()).await.is_err());
        assert!(repo.update_if_active(input, None).await.unwrap().is_none());

        let first = UpsertVideoTask {
            progress_percent: 40,
            ..initial.clone().into()
        };
        let second = UpsertVideoTask {
            status: VideoTaskStatus::Completed,
            progress_percent: 100,
            ..initial.into()
        };
        let (a, b) = tokio::join!(
            repo.update_if_active(first, None),
            repo.update_if_active(second, None)
        );
        assert_eq!(
            usize::from(a.unwrap().is_some()) + usize::from(b.unwrap().is_some()),
            1
        );
        let latest = repo
            .find(VideoTaskLookupKey::Id("revision-task"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(latest.row_revision, 2);
        assert_eq!(latest.updated_at_unix_secs, 100);
    }

    #[tokio::test]
    async fn claims_advance_revision_and_clock_skew_does_not_block_current_owner() {
        let repo = InMemoryVideoTaskRepository::default();
        let initial = repo
            .upsert(sample_task(
                "revision-task",
                VideoTaskStatus::Processing,
                100,
            ))
            .await
            .unwrap();
        let claim = repo.claim_due(100, 130, 1).await.unwrap().pop().unwrap();
        assert_eq!(claim.task.row_revision, initial.row_revision + 1);
        let stale = UpsertVideoTask {
            status: VideoTaskStatus::Failed,
            updated_at_unix_secs: 999,
            ..initial.into()
        };
        assert!(repo.update_if_active(stale, None).await.unwrap().is_none());
        let current = UpsertVideoTask {
            status: VideoTaskStatus::Completed,
            updated_at_unix_secs: 99,
            ..claim.task.into()
        };
        let stored = repo
            .update_if_active(current, Some(claim.fencing_token))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.row_revision, 3);
        assert_eq!(stored.updated_at_unix_secs, 100);
    }

    #[tokio::test]
    async fn deletion_transition_matrix_and_tombstones_are_enforced_by_both_writers() {
        for from in [
            VideoTaskStatus::Pending,
            VideoTaskStatus::Submitted,
            VideoTaskStatus::Queued,
            VideoTaskStatus::Processing,
            VideoTaskStatus::Completed,
            VideoTaskStatus::Failed,
            VideoTaskStatus::Cancelled,
            VideoTaskStatus::Expired,
            VideoTaskStatus::Deleted,
        ] {
            for guarded in [false, true] {
                let repo = InMemoryVideoTaskRepository::default();
                let stored = repo
                    .upsert(sample_task("deletion-task", from, 100))
                    .await
                    .unwrap();
                let update = UpsertVideoTask {
                    status: VideoTaskStatus::Deleted,
                    ..stored.into()
                };
                let allowed = matches!(from, VideoTaskStatus::Completed | VideoTaskStatus::Failed);
                if guarded {
                    assert_eq!(
                        repo.update_if_active(update, None).await.unwrap().is_some(),
                        allowed,
                        "{from:?}"
                    );
                } else {
                    assert_eq!(repo.upsert(update).await.is_ok(), allowed, "{from:?}");
                }
                let latest = repo
                    .find(VideoTaskLookupKey::Id("deletion-task"))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    latest.status,
                    if allowed {
                        VideoTaskStatus::Deleted
                    } else {
                        from
                    }
                );
                if latest.status == VideoTaskStatus::Deleted {
                    let resurrect = UpsertVideoTask {
                        status: VideoTaskStatus::Processing,
                        ..latest.into()
                    };
                    assert!(repo.upsert(resurrect.clone()).await.is_err());
                    assert!(repo
                        .update_if_active(resurrect, None)
                        .await
                        .unwrap()
                        .is_none());
                }
            }
        }
    }
}
