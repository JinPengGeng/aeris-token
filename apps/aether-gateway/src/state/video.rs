use super::{AppState, GatewayError};

use crate::{async_task, video_tasks};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskClaim, VideoTaskLookupKey, VideoTaskModelCount,
    VideoTaskQueryFilter, VideoTaskStatusCount,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoTaskRouteAccess {
    Allowed,
    NotFound,
    Denied,
}

#[derive(Debug)]
pub(crate) enum VideoTaskSnapshotWrite {
    Applied(StoredVideoTask),
    Conflict,
    NoWriter,
}

impl VideoTaskSnapshotWrite {
    pub(crate) fn stored(&self) -> Option<&StoredVideoTask> {
        match self {
            Self::Applied(stored) => Some(stored),
            Self::Conflict | Self::NoWriter => None,
        }
    }

    pub(crate) fn accepted(&self) -> bool {
        !matches!(self, Self::Conflict)
    }
}

impl AppState {
    pub(crate) async fn read_data_backed_video_task_response(
        &self,
        route_family: Option<&str>,
        request_path: &str,
    ) -> Result<Option<video_tasks::LocalVideoTaskReadResponse>, GatewayError> {
        self.data
            .read_video_task_response(route_family, request_path)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn read_data_backed_video_task_response_for_user(
        &self,
        route_family: Option<&str>,
        request_path: &str,
        user_id: &str,
    ) -> Result<Option<video_tasks::LocalVideoTaskReadResponse>, GatewayError> {
        self.data
            .read_video_task_response_for_user(route_family, request_path, user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_video_task_by_id(
        &self,
        task_id: &str,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        self.data
            .find_video_task(VideoTaskLookupKey::Id(task_id))
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_video_task_by_short_id(
        &self,
        short_id: &str,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        self.data
            .find_video_task(VideoTaskLookupKey::ShortId(short_id))
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_video_task_by_id_for_user(
        &self,
        task_id: &str,
        user_id: &str,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        self.data
            .find_video_task_for_user(VideoTaskLookupKey::Id(task_id), user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_video_task_by_short_id_for_user(
        &self,
        short_id: &str,
        user_id: &str,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        self.data
            .find_video_task_for_user(VideoTaskLookupKey::ShortId(short_id), user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn upsert_video_task_snapshot(
        &self,
        snapshot: &video_tasks::LocalVideoTaskSnapshot,
    ) -> Result<VideoTaskSnapshotWrite, GatewayError> {
        if !self.data.has_video_task_writer() {
            self.video_tasks.record_snapshot(snapshot.clone());
            return Ok(VideoTaskSnapshotWrite::NoWriter);
        }
        let mut record = snapshot.to_upsert_record();
        // Reconstructed snapshots intentionally omit sensitive/request-only fields. Preserve the
        // persisted row's immutable identity and request-shape scalars before writing lifecycle
        // changes back, so the repository can continue enforcing immutable-field integrity.
        let existing_by_id = self
            .data
            .find_video_task(VideoTaskLookupKey::Id(record.id.as_str()))
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let existing = if existing_by_id.is_some() {
            existing_by_id
        } else if let Some(short_id) = record.short_id.as_deref() {
            self.data
                .find_video_task(VideoTaskLookupKey::ShortId(short_id))
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
        } else {
            None
        };
        if let Some(existing) = existing {
            record.id = existing.id;
            record.short_id = existing.short_id;
            record.request_id = existing.request_id;
            record.user_id = existing.user_id;
            record.api_key_id = existing.api_key_id;
            record.external_task_id = existing.external_task_id;
            record.provider_id = existing.provider_id;
            record.endpoint_id = existing.endpoint_id;
            record.key_id = existing.key_id;
            record.client_api_format = existing.client_api_format;
            record.provider_api_format = existing.provider_api_format;
            record.format_converted = existing.format_converted;
            record.model = existing.model;
            record.duration_seconds = existing.duration_seconds;
            record.resolution = existing.resolution;
            record.aspect_ratio = existing.aspect_ratio;
            record.size = existing.size;

            record.retry_count = existing.retry_count;
            record.poll_count = existing.poll_count;
            record.max_poll_count = existing.max_poll_count;
            record.poll_interval_seconds = existing.poll_interval_seconds;
            let task_id = record.id.clone();
            let stored = self
                .data
                .update_active_video_task(record, None)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            if let Some(stored) = &stored {
                self.publish_stored_video_task(stored, Some(snapshot))
                    .await?;
            } else {
                self.restore_video_task_registry(&task_id, Some(snapshot))
                    .await?;
            }
            return Ok(match stored {
                Some(stored) => VideoTaskSnapshotWrite::Applied(stored),
                None => VideoTaskSnapshotWrite::Conflict,
            });
        }
        // A snapshot that has already observed a database row cannot recreate it.
        if record.row_revision > 0 {
            return Ok(VideoTaskSnapshotWrite::Conflict);
        }
        let task_id = record.id.clone();
        match self.data.upsert_video_task(record).await {
            Ok(stored) => {
                if let Some(stored) = &stored {
                    self.publish_stored_video_task(stored, Some(snapshot))
                        .await?;
                }
                Ok(match stored {
                    Some(stored) => VideoTaskSnapshotWrite::Applied(stored),
                    None => VideoTaskSnapshotWrite::Conflict,
                })
            }
            Err(err) => {
                // Another creator may have inserted between lookup and INSERT. Never
                // publish the losing create snapshot or retry it with the winner's revision.
                if self
                    .restore_video_task_registry(&task_id, Some(snapshot))
                    .await?
                {
                    Ok(VideoTaskSnapshotWrite::Conflict)
                } else {
                    Err(GatewayError::Internal(err.to_string()))
                }
            }
        }
    }

    pub(crate) async fn publish_stored_video_task(
        &self,
        task: &StoredVideoTask,
        source: Option<&video_tasks::LocalVideoTaskSnapshot>,
    ) -> Result<(), GatewayError> {
        let cached = match task.effective_api_format() {
            Some("openai:video") => self
                .video_tasks
                .snapshot_for_route(Some("openai"), &format!("/v1/videos/{}", task.id)),
            Some("gemini:video") => self.video_tasks.snapshot_for_route(
                Some("gemini"),
                &format!(
                    "/v1beta/models/{}/operations/{}",
                    task.model.as_deref().unwrap_or_default(),
                    task.short_id.as_deref().unwrap_or(&task.id)
                ),
            ),
            _ => None,
        };
        let snapshot = source
            .and_then(|snapshot| snapshot.with_committed_stored_task(task))
            .or_else(|| {
                cached.as_ref().and_then(|snapshot| {
                    if snapshot.row_revision() == task.row_revision {
                        snapshot.with_committed_stored_task(task)
                    } else {
                        snapshot.with_stored_task(task)
                    }
                })
            })
            .or_else(|| video_tasks::LocalVideoTaskSnapshot::from_stored_task(task));
        let snapshot = match snapshot {
            Some(snapshot) => Some(snapshot),
            None => self.reconstruct_video_task_snapshot(task).await?,
        };
        if let Some(snapshot) = snapshot {
            self.video_tasks.record_snapshot(snapshot);
        }
        Ok(())
    }

    pub(crate) async fn restore_video_task_registry(
        &self,
        task_id: &str,
        source: Option<&video_tasks::LocalVideoTaskSnapshot>,
    ) -> Result<bool, GatewayError> {
        let Some(task) = self.find_video_task_by_id(task_id).await? else {
            return Ok(false);
        };
        // A rejected update is only a transport fallback, never an authority for
        // native presentation or lifecycle fields. Prefer the current cache below.
        if let Some(source) = source {
            if let Some(snapshot) = source.with_stored_task(&task) {
                self.video_tasks.record_snapshot(snapshot);
            }
        }
        self.publish_stored_video_task(&task, None).await?;
        Ok(true)
    }

    pub(crate) async fn enrich_video_task_terminal_presentation(
        &self,
        expected: &video_tasks::LocalVideoTaskSnapshot,
        projected: &video_tasks::LocalVideoTaskSnapshot,
    ) -> Result<bool, GatewayError> {
        if self.data.has_video_task_writer() {
            let record = expected.to_upsert_record();
            let Some(current) = self.find_video_task_by_id(&record.id).await? else {
                return Ok(false);
            };
            if current.row_revision != expected.row_revision() || current.status != record.status {
                self.publish_stored_video_task(&current, None).await?;
                return Ok(false);
            }
        }
        // The registry compares the entire original observation under its lock. This
        // only fills provider presentation; database lifecycle and revision stay intact.
        Ok(self
            .video_tasks
            .enrich_terminal_presentation(expected, projected))
    }

    pub(crate) async fn persist_video_task_finalize(
        &self,
        route_family: Option<&str>,
        request_path: &str,
        report_kind: &str,
        report_context: Option<&serde_json::Value>,
    ) -> Result<bool, GatewayError> {
        let expected_revision = report_context
            .and_then(|context| context.get("video_task_row_revision"))
            .and_then(serde_json::Value::as_i64);
        if self
            .video_tasks
            .snapshot_for_route(route_family, request_path)
            .is_none()
        {
            self.hydrate_video_task_for_route(route_family, request_path)
                .await?;
        }
        let Some(mut snapshot) = self
            .video_tasks
            .snapshot_for_route(route_family, request_path)
        else {
            return Ok(false);
        };
        if self.data.has_video_task_writer()
            && (expected_revision != Some(snapshot.row_revision()) || snapshot.row_revision() <= 0)
        {
            let task_id = snapshot.to_upsert_record().id;
            self.restore_video_task_registry(&task_id, Some(&snapshot))
                .await?;
            return Ok(false);
        }
        let expected_local = snapshot.clone();
        if !snapshot.apply_finalize_report(report_kind) {
            return Ok(false);
        }
        if !self.data.has_video_task_writer() {
            return Ok(self
                .video_tasks
                .replace_local_snapshot(&expected_local, snapshot));
        }
        Ok(self.upsert_video_task_snapshot(&snapshot).await?.accepted())
    }

    pub(crate) async fn hydrate_video_task_for_route(
        &self,
        route_family: Option<&str>,
        request_path: &str,
    ) -> Result<bool, GatewayError> {
        let lookup =
            video_tasks::resolve_video_task_hydration_lookup_key(route_family, request_path);
        let Some(lookup) = lookup else {
            return Ok(false);
        };
        let Some(task) = self
            .data
            .find_video_task(lookup)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
        else {
            return Ok(false);
        };
        if self.video_tasks.hydrate_from_stored_task(&task) {
            return Ok(true);
        }

        let Some(snapshot) = self.reconstruct_video_task_snapshot(&task).await? else {
            return Ok(false);
        };
        self.video_tasks.record_snapshot(snapshot);
        Ok(true)
    }

    pub(crate) async fn hydrate_video_task_for_route_for_user(
        &self,
        route_family: Option<&str>,
        request_path: &str,
        user_id: &str,
    ) -> Result<VideoTaskRouteAccess, GatewayError> {
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return Ok(VideoTaskRouteAccess::Denied);
        }
        let Some(lookup) =
            video_tasks::resolve_video_task_hydration_lookup_key(route_family, request_path)
        else {
            return Ok(VideoTaskRouteAccess::NotFound);
        };

        if let Some(task) = self
            .data
            .find_video_task_for_user(lookup, user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
        {
            if !self.video_tasks.hydrate_from_stored_task(&task) {
                if let Some(snapshot) = self.reconstruct_video_task_snapshot(&task).await? {
                    self.video_tasks.record_snapshot(snapshot);
                }
            }
            return Ok(VideoTaskRouteAccess::Allowed);
        }

        Ok(
            match self
                .video_tasks
                .snapshot_for_route(route_family, request_path)
            {
                Some(snapshot) if snapshot.belongs_to_user(user_id) => {
                    VideoTaskRouteAccess::Allowed
                }
                Some(_) => VideoTaskRouteAccess::Denied,
                None => VideoTaskRouteAccess::NotFound,
            },
        )
    }

    pub(crate) async fn reconstruct_video_task_snapshot(
        &self,
        task: &StoredVideoTask,
    ) -> Result<Option<video_tasks::LocalVideoTaskSnapshot>, GatewayError> {
        crate::provider_transport::reconstruct_local_video_task_snapshot(self, task)
            .await
            .map_err(GatewayError::Internal)
    }

    pub(crate) async fn claim_due_video_tasks(
        &self,
        now_unix_secs: u64,
        claim_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<VideoTaskClaim>, GatewayError> {
        self.data
            .claim_due_video_tasks(now_unix_secs, claim_until_unix_secs, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_active_video_task(
        &self,
        task: UpsertVideoTask,
        fencing_token: Option<i64>,
    ) -> Result<Option<StoredVideoTask>, GatewayError> {
        self.data
            .update_active_video_task(task, fencing_token)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_video_task_page(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, GatewayError> {
        self.data
            .list_video_task_page(filter, offset, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_video_task_page_summary(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, GatewayError> {
        self.data
            .list_video_task_page_summary(filter, offset, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_video_tasks(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_video_tasks(filter)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_video_tasks_by_status(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<Vec<VideoTaskStatusCount>, GatewayError> {
        self.data
            .count_video_tasks_by_status(filter)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_distinct_video_task_users(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_distinct_video_task_users(filter)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn top_video_task_models(
        &self,
        filter: &VideoTaskQueryFilter,
        limit: usize,
    ) -> Result<Vec<VideoTaskModelCount>, GatewayError> {
        self.data
            .top_video_task_models(filter, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_video_tasks_created_since(
        &self,
        filter: &VideoTaskQueryFilter,
        created_since_unix_secs: u64,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_video_tasks_created_since(filter, created_since_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn execute_video_task_refresh_plan(
        &self,
        refresh_plan: &video_tasks::LocalVideoTaskReadRefreshPlan,
    ) -> Result<bool, GatewayError> {
        async_task::execute_video_task_refresh_plan(self, refresh_plan).await
    }
}
